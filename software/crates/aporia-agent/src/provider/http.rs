//! An OpenAI-compatible endpoint over plain HTTP, with no dependency.
//!
//! The first slice runs a model locally: llama.cpp, Ollama's `OpenAI` compatibility, LM Studio and
//! friends all speak this shape. The client is deliberately small and strict -- one request, one
//! `Content-Length` response, no chunked transfer, no TLS. A provider that needs HTTPS belongs
//! behind the same trait with a TLS-capable implementation rather than a half-supported one here.

use super::{ModelProvider, ModelRequest, ModelResponse, ProviderError, Role, ToolCall};
use crate::json;
use aporia_store::Json;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

const MAX_HEADER_BYTES: usize = 64 * 1024;
const MAX_BODY_BYTES: u64 = 4 * 1024 * 1024;
const ERROR_BODY_BYTES: usize = 4096;

/// A client for a local OpenAI-compatible chat-completions endpoint.
#[derive(Debug)]
pub struct HttpProvider {
    host: String,
    port: u16,
    path: String,
    model: String,
    timeout: Duration,
    label: String,
}

impl HttpProvider {
    /// `base` is `http://host[:port]/v1`; `/chat/completions` is appended unless already present.
    pub fn new(base: &str, model: &str, timeout_ms: u64) -> Result<Self, ProviderError> {
        if model.trim().is_empty() {
            return Err(ProviderError::Malformed(
                "the HTTP provider needs a model name".to_string(),
            ));
        }
        let (host, port, path) = parse_base(base)?;
        let timeout = Duration::from_millis(timeout_ms.max(1));
        let label = format!(
            "openai-compatible http://{host}:{port}{path} model {model} (timeout {} ms)",
            timeout.as_millis()
        );
        Ok(Self {
            host,
            port,
            path,
            model: model.to_string(),
            timeout,
            label,
        })
    }

    fn connect(&self) -> Result<TcpStream, ProviderError> {
        let address = format!("{}:{}", self.host, self.port);
        let addresses = address
            .to_socket_addrs()
            .map_err(|error| ProviderError::Unreachable(format!("{address}: {error}")))?;
        let mut last = None;
        for address in addresses {
            match TcpStream::connect_timeout(&address, self.timeout) {
                Ok(stream) => {
                    stream
                        .set_read_timeout(Some(self.timeout))
                        .map_err(|error| ProviderError::Unreachable(error.to_string()))?;
                    stream
                        .set_write_timeout(Some(self.timeout))
                        .map_err(|error| ProviderError::Unreachable(error.to_string()))?;
                    return Ok(stream);
                }
                Err(error) => last = Some(error),
            }
        }
        Err(ProviderError::Unreachable(format!(
            "could not connect to {address}: {}",
            last.map_or_else(|| "no address".to_string(), |error| error.to_string())
        )))
    }

    fn exchange(&self, body: &str) -> Result<(u16, String), ProviderError> {
        let mut stream = self.connect()?;
        let host = if self.host.contains(':') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        };
        let request = format!(
            "POST {} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nAccept: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            self.path,
            body.len()
        );
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
            ProviderError::Limit(
                "the declared response length does not fit this machine".to_string(),
            )
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
}

impl ModelProvider for HttpProvider {
    fn complete(&mut self, request: &ModelRequest) -> Result<ModelResponse, ProviderError> {
        let body = encode_request(&self.model, request).to_compact();
        let (_, text) = self.exchange(&body)?;
        let document = Json::parse(&text).map_err(|error| {
            ProviderError::Malformed(format!("the answer is not JSON: {error}"))
        })?;
        decode_response(&document)
    }

    fn describe(&self) -> String {
        self.label.clone()
    }
}

fn map_io(what: &'static str) -> impl FnOnce(std::io::Error) -> ProviderError {
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

fn parse_base(base: &str) -> Result<(String, u16, String), ProviderError> {
    if base.starts_with("https://") {
        return Err(ProviderError::Malformed(
            "this build speaks plain HTTP only; an HTTPS endpoint needs a TLS-capable provider"
                .to_string(),
        ));
    }
    let rest = base.strip_prefix("http://").ok_or_else(|| {
        ProviderError::Malformed("the endpoint must start with http://".to_string())
    })?;
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
    let (host, port) = split_authority(authority)?;
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
    Ok((host, port, path))
}

fn split_authority(authority: &str) -> Result<(String, u16), ProviderError> {
    if let Some(rest) = authority.strip_prefix('[') {
        let end = rest.find(']').ok_or_else(|| {
            ProviderError::Malformed("an IPv6 host must close its bracket".to_string())
        })?;
        let host = &rest[..end];
        let after = &rest[end + 1..];
        let port = match after.strip_prefix(':') {
            Some(port) => parse_port(port)?,
            None if after.is_empty() => 80,
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
        None => Ok((authority.to_string(), 80)),
    }
}

fn parse_port(text: &str) -> Result<u16, ProviderError> {
    text.parse::<u16>()
        .map_err(|_| ProviderError::Malformed(format!("`{text}` is not a port")))
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

fn encode_request(model: &str, request: &ModelRequest) -> Json {
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

fn encode_message(message: &crate::provider::Message) -> Json {
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

fn encode_tool(tool: &crate::provider::ToolSchema) -> Json {
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

fn decode_response(document: &Json) -> Result<ModelResponse, ProviderError> {
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
