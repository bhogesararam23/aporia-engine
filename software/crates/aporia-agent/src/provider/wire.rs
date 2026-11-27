//! The OpenAI-compatible wire shape, shared by the plain-HTTP and HTTPS providers.
//!
//! One request, one `Content-Length` response, no chunked transfer. The encoding and decoding
//! live here so the two transports cannot drift apart; only the way a byte stream is obtained
//! differs between them.

use super::{ModelRequest, ModelResponse, ProviderError, Role, ToolCall};
use crate::json;
use aporia_store::Json;
use std::fmt::Write as _;
use std::io::{Read, Write};

pub const MAX_HEADER_BYTES: usize = 64 * 1024;
pub const MAX_BODY_BYTES: u64 = 4 * 1024 * 1024;
pub const ERROR_BODY_BYTES: usize = 4096;

/// The endpoint a base URL names: scheme, host, port and the chat-completions path.
#[derive(Debug)]
pub struct Endpoint {
    pub https: bool,
    pub host: String,
    pub port: u16,
    pub path: String,
}

/// Parse `http(s)://host[:port][/v1]`; `/chat/completions` is appended unless already present.
pub fn parse_base(base: &str) -> Result<Endpoint, ProviderError> {
    let (https, rest) = if let Some(rest) = base.strip_prefix("https://") {
        (true, rest)
    } else if let Some(rest) = base.strip_prefix("http://") {
        (false, rest)
    } else {
        return Err(ProviderError::Malformed(
            "the endpoint must start with http:// or https://".to_string(),
        ));
    };
    let (authority, mut path) = match rest.find('/') {
        Some(at) => (&rest[..at], rest[at..].to_string()),
        None => (rest, String::new()),
    };
    if authority.is_empty() {
        return Err(ProviderError::Malformed(
            "the endpoint has no host".to_string(),
        ));
    }
    if authority.contains('@') {
        return Err(ProviderError::Malformed(
            "userinfo in the endpoint is not supported".to_string(),
        ));
    }
    let (host, port) = split_authority(authority, if https { 443 } else { 80 })?;
    if path.contains('?') || path.contains('#') {
        return Err(ProviderError::Malformed(
            "a query or fragment in the endpoint is not supported".to_string(),
        ));
    }
    while path.ends_with('/') {
        path.pop();
    }
    if !path.ends_with("/chat/completions") {
        path.push_str("/chat/completions");
    }
    if !path.starts_with('/') {
        path.insert(0, '/');
    }
    Ok(Endpoint {
        https,
        host,
        port,
        path,
    })
}

fn split_authority(authority: &str, default_port: u16) -> Result<(String, u16), ProviderError> {
    if let Some(rest) = authority.strip_prefix('[') {
        let end = rest.find(']').ok_or_else(|| {
            ProviderError::Malformed("an IPv6 host must close its bracket".to_string())
        })?;
        let host = &rest[..end];
        let after = &rest[end + 1..];
        let port = match after.strip_prefix(':') {
            Some(port) => parse_port(port)?,
            None if after.is_empty() => default_port,
            None => {
                return Err(ProviderError::Malformed(
                    "unexpected text after an IPv6 host".to_string(),
                ));
            }
        };
        return Ok((host.to_string(), port));
    }
    match authority.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') => Ok((host.to_string(), parse_port(port)?)),
        Some(_) => Err(ProviderError::Malformed(
            "an IPv6 host must be written in brackets".to_string(),
        )),
        None => Ok((authority.to_string(), default_port)),
    }
}

fn parse_port(text: &str) -> Result<u16, ProviderError> {
    text.parse::<u16>()
        .map_err(|_| ProviderError::Malformed(format!("`{text}` is not a port")))
}

/// One HTTP exchange over an already-connected stream. `extra_headers` carry the authorization
/// the HTTPS provider adds; their values never appear in an error message.
pub fn exchange(
    stream: &mut (impl Read + Write),
    host: &str,
    port: u16,
    path: &str,
    body: &str,
    extra_headers: &[(String, String)],
) -> Result<(u16, String), ProviderError> {
    let host_header = if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    };
    let mut request = format!(
        "POST {path} HTTP/1.1\r\nHost: {host_header}\r\nContent-Type: application/json\r\nAccept: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    for (name, value) in extra_headers {
        let _ = write!(request, "{name}: {value}\r\n");
    }
    request.push_str("\r\n");
    request.push_str(body);
    stream
        .write_all(request.as_bytes())
        .map_err(map_io("sending the request"))?;

    let mut received = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        if let Some(at) = find(&received, b"\r\n\r\n") {
            break at + 4;
        }
        if received.len() > MAX_HEADER_BYTES {
            return Err(ProviderError::Limit(format!(
                "response headers exceed {MAX_HEADER_BYTES} bytes"
            )));
        }
        let read = stream
            .read(&mut chunk)
            .map_err(map_io("reading the response"))?;
        if read == 0 {
            return Err(ProviderError::Malformed(
                "the endpoint closed before the response headers were complete".to_string(),
            ));
        }
        received.extend_from_slice(&chunk[..read]);
    };

    let header_text = String::from_utf8_lossy(&received[..header_end - 4]).to_string();
    let (status, length) = parse_headers(&header_text)?;
    let mut body_bytes = received[header_end..].to_vec();
    let length = usize::try_from(length).map_err(|_| {
        ProviderError::Limit("the declared response length does not fit this machine".to_string())
    })?;
    if body_bytes.len() > length {
        body_bytes.truncate(length);
    }
    while body_bytes.len() < length {
        let read = stream
            .read(&mut chunk)
            .map_err(map_io("reading the response body"))?;
        if read == 0 {
            return Err(ProviderError::Malformed(format!(
                "the endpoint closed with {} of {length} body bytes",
                body_bytes.len()
            )));
        }
        body_bytes.extend_from_slice(&chunk[..read]);
    }
    let text = String::from_utf8_lossy(&body_bytes).to_string();
    if !(200..300).contains(&status) {
        let mut shown = text;
        shown.truncate(ERROR_BODY_BYTES);
        return Err(ProviderError::Status(status, shown));
    }
    Ok((status, text))
}

pub fn map_io(what: &'static str) -> impl FnOnce(std::io::Error) -> ProviderError {
    move |error| {
        if error.kind() == std::io::ErrorKind::TimedOut
            || error.kind() == std::io::ErrorKind::WouldBlock
        {
            ProviderError::Timeout
        } else {
            ProviderError::Unreachable(format!("{what}: {error}"))
        }
    }
}

fn parse_headers(header_text: &str) -> Result<(u16, u64), ProviderError> {
    let mut lines = header_text.split("\r\n");
    let status_line = lines
        .next()
        .ok_or_else(|| ProviderError::Malformed("the response has no status line".to_string()))?;
    let mut parts = status_line.splitn(3, ' ');
    let version = parts.next().unwrap_or("");
    if !version.starts_with("HTTP/") {
        return Err(ProviderError::Malformed(
            "the response does not start with an HTTP status line".to_string(),
        ));
    }
    let status = parts
        .next()
        .and_then(|text| text.parse::<u16>().ok())
        .ok_or_else(|| ProviderError::Malformed("the status code is not a number".to_string()))?;
    let mut length = None;
    let mut chunked = false;
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let (name, value) = line.split_once(':').ok_or_else(|| {
            ProviderError::Malformed(format!("malformed response header `{line}`"))
        })?;
        let name = name.trim().to_ascii_lowercase();
        let value = value.trim();
        match name.as_str() {
            "content-length" => {
                length = Some(value.parse::<u64>().map_err(|_| {
                    ProviderError::Malformed("content-length is not a number".to_string())
                })?);
            }
            "transfer-encoding" if value.to_ascii_lowercase().contains("chunked") => chunked = true,
            _ => {}
        }
    }
    if chunked {
        return Err(ProviderError::Malformed(
            "chunked responses are not supported; disable streaming".to_string(),
        ));
    }
    let length = length.ok_or_else(|| {
        ProviderError::Malformed("the response carries no content-length".to_string())
    })?;
    if length > MAX_BODY_BYTES {
        return Err(ProviderError::Limit(format!(
            "the response body exceeds {MAX_BODY_BYTES} bytes"
        )));
    }
    Ok((status, length))
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

pub fn encode_request(model: &str, request: &ModelRequest) -> Json {
    Json::object(vec![
        ("model", Json::text(model)),
        (
            "messages",
            Json::Arr(request.messages.iter().map(encode_message).collect()),
        ),
        (
            "tools",
            Json::Arr(request.tools.iter().map(encode_tool).collect()),
        ),
        ("tool_choice", Json::text("auto")),
        ("temperature", Json::Num(0.0)),
        ("stream", Json::Bool(false)),
    ])
}

fn encode_message(message: &super::Message) -> Json {
    let mut fields = vec![("role", Json::text(message.role.as_str()))];
    match message.role {
        Role::Tool => {
            fields.push((
                "tool_call_id",
                Json::text(message.tool_call_id.clone().unwrap_or_default()),
            ));
            fields.push(("content", Json::text(message.content.clone())));
        }
        Role::Assistant if !message.tool_calls.is_empty() => {
            fields.push((
                "content",
                if message.content.is_empty() {
                    Json::Null
                } else {
                    Json::text(message.content.clone())
                },
            ));
            fields.push((
                "tool_calls",
                Json::Arr(message.tool_calls.iter().map(encode_call).collect()),
            ));
        }
        _ => fields.push(("content", Json::text(message.content.clone()))),
    }
    Json::object(fields)
}

fn encode_call(call: &ToolCall) -> Json {
    Json::object(vec![
        ("id", Json::text(call.id.clone())),
        ("type", Json::text("function")),
        (
            "function",
            Json::object(vec![
                ("name", Json::text(call.name.clone())),
                ("arguments", Json::text(call.arguments.clone())),
            ]),
        ),
    ])
}

fn encode_tool(tool: &super::ToolSchema) -> Json {
    Json::object(vec![
        ("type", Json::text("function")),
        (
            "function",
            Json::object(vec![
                ("name", Json::text(tool.name.clone())),
                ("description", Json::text(tool.description.clone())),
                ("parameters", tool.parameters.clone()),
            ]),
        ),
    ])
}

pub fn decode_response(document: &Json) -> Result<ModelResponse, ProviderError> {
    let fields = json::fields(document)
        .ok_or_else(|| ProviderError::Malformed("the answer is not a JSON object".to_string()))?;
    let choices = json::array(fields, "choices").map_err(ProviderError::Malformed)?;
    let first = choices
        .first()
        .ok_or_else(|| ProviderError::Malformed("the answer has no choices".to_string()))?;
    let message = json::fields(first)
        .and_then(|first| json::field(first, "message"))
        .and_then(json::fields)
        .ok_or_else(|| {
            ProviderError::Malformed("the first choice has no message object".to_string())
        })?;
    let content = match json::field(message, "content") {
        None | Some(Json::Null) => None,
        Some(Json::Str(text)) => Some(text.clone()),
        Some(_) => {
            return Err(ProviderError::Malformed(
                "message content is not a string; content parts are not supported".to_string(),
            ));
        }
    };
    let tool_calls = match json::field(message, "tool_calls") {
        None | Some(Json::Null) => Vec::new(),
        Some(value) => {
            let calls = value.as_array().ok_or_else(|| {
                ProviderError::Malformed("message tool_calls is not an array".to_string())
            })?;
            calls
                .iter()
                .enumerate()
                .map(|(index, call)| {
                    decode_call(call).map_err(|reason| {
                        ProviderError::Malformed(format!("tool_calls[{index}]: {reason}"))
                    })
                })
                .collect::<Result<Vec<_>, _>>()?
        }
    };
    if content.as_deref().is_none_or(str::is_empty) && tool_calls.is_empty() {
        return Err(ProviderError::Malformed(
            "the model returned neither text nor a tool call".to_string(),
        ));
    }
    Ok(ModelResponse {
        content,
        tool_calls,
    })
}

fn decode_call(value: &Json) -> Result<ToolCall, String> {
    let call = json::fields(value).ok_or_else(|| "a tool call must be an object".to_string())?;
    if let Some(kind) = json::field(call, "type").and_then(Json::as_str)
        && kind != "function"
    {
        return Err(format!("unsupported tool call type `{kind}`"));
    }
    let id = json::string(call, "id")?.to_string();
    let function = json::required(call, "function").and_then(|value| {
        json::fields(value).ok_or_else(|| "field `function` must be an object".to_string())
    })?;
    let name = json::string(function, "name")?.to_string();
    let arguments = match json::required(function, "arguments")? {
        Json::Str(text) => text.clone(),
        value if json::fields(value).is_some() => value.to_compact(),
        _ => return Err("field `function.arguments` must be a string or an object".to_string()),
    };
    Ok(ToolCall {
        id,
        name,
        arguments,
    })
}
