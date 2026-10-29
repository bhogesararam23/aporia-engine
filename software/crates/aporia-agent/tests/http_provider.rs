//! The HTTP provider against a real local socket: the request is the `OpenAI` shape, the answer is
//! decoded strictly, and every refusal is named.

use aporia_agent::provider::http::HttpProvider;
use aporia_agent::{Message, ModelProvider, ModelRequest, ProviderError, ToolCall, ToolSchema};
use aporia_store::Json;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

fn response(status: u16, reason: &str, body: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

fn read_request(stream: &mut TcpStream) -> String {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("timeout");
    let mut data = Vec::new();
    let mut buffer = [0u8; 4096];
    loop {
        let read = stream.read(&mut buffer).expect("read request");
        if read == 0 {
            break;
        }
        data.extend_from_slice(&buffer[..read]);
        if let Some(end) = find(&data, b"\r\n\r\n") {
            let header = String::from_utf8_lossy(&data[..end]).to_string();
            let length = header
                .lines()
                .find_map(|line| line.strip_prefix("Content-Length: "))
                .and_then(|value| value.trim().parse::<usize>().ok())
                .unwrap_or(0);
            while data.len() < end + 4 + length {
                let read = stream.read(&mut buffer).expect("read body");
                if read == 0 {
                    break;
                }
                data.extend_from_slice(&buffer[..read]);
            }
            break;
        }
    }
    String::from_utf8_lossy(&data).to_string()
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn serve(response: Vec<u8>) -> (String, thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let request = read_request(&mut stream);
        stream.write_all(&response).expect("write response");
        request
    });
    (format!("http://127.0.0.1:{port}/v1"), handle)
}

fn request() -> ModelRequest {
    ModelRequest {
        messages: vec![
            Message::system("you are an investigator"),
            Message::user("inspect this"),
        ],
        tools: vec![ToolSchema {
            name: "read_file".to_string(),
            description: "read one file".to_string(),
            parameters: Json::object(vec![("type", Json::text("object"))]),
        }],
    }
}

#[test]
fn the_request_is_openai_shaped_and_a_tool_call_is_decoded() {
    let body = r#"{"choices":[{"message":{"content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":"propose_plan","arguments":"{\"gaps\":[]}"}}]}}]}"#;
    let (base, server) = serve(response(200, "OK", body));
    let mut provider = HttpProvider::new(&base, "test-model", 5_000).expect("provider");

    let answer = provider.complete(&request()).expect("answer");

    assert_eq!(answer.tool_calls.len(), 1);
    assert_eq!(answer.tool_calls[0].id, "call_1");
    assert_eq!(answer.tool_calls[0].name, "propose_plan");
    assert_eq!(answer.tool_calls[0].arguments, r#"{"gaps":[]}"#);

    let sent = server.join().expect("server");
    assert!(
        sent.starts_with("POST /v1/chat/completions HTTP/1.1\r\n"),
        "{sent}"
    );
    assert!(sent.contains("Content-Type: application/json"), "{sent}");
    assert!(sent.contains(r#""model":"test-model""#), "{sent}");
    assert!(sent.contains(r#""stream":false"#), "{sent}");
    assert!(sent.contains(r#""tools":[{"type":"function""#), "{sent}");
    assert!(sent.contains(r#""role":"system""#), "{sent}");
}

#[test]
fn tool_history_is_encoded_with_call_ids_and_the_function_type() {
    let body = r#"{"choices":[{"message":{"content":"I will look","tool_calls":[]}}]}"#;
    let (base, server) = serve(response(200, "OK", body));
    let mut provider = HttpProvider::new(&base, "m", 5_000).expect("provider");
    let mut req = request();
    req.messages.push(Message::assistant(
        "",
        vec![ToolCall {
            id: "call_9".to_string(),
            name: "read_file".to_string(),
            arguments: r#"{"path":"a.ap"}"#.to_string(),
        }],
    ));
    req.messages.push(Message::tool("call_9", "model a { }"));

    let answer = provider.complete(&req).expect("answer");

    assert_eq!(answer.content.as_deref(), Some("I will look"));
    let sent = server.join().expect("server");
    assert!(sent.contains(r#""tool_call_id":"call_9""#), "{sent}");
    assert!(
        sent.contains(r#""tool_calls":[{"id":"call_9","type":"function""#),
        "{sent}"
    );
}

#[test]
fn object_arguments_from_a_server_are_accepted_and_compacted() {
    let body = r#"{"choices":[{"message":{"tool_calls":[{"id":"c","function":{"name":"read_file","arguments":{"path":"a.ap"}}}]}}]}"#;
    let (base, handle) = serve(response(200, "OK", body));
    let mut provider = HttpProvider::new(&base, "m", 5_000).expect("provider");
    let answer = provider.complete(&request()).expect("answer");
    assert_eq!(answer.tool_calls[0].arguments, r#"{"path":"a.ap"}"#);
    let _ = handle.join();
}

#[test]
fn a_non_success_status_carries_its_body() {
    let (base, handle) = serve(response(500, "Internal Server Error", "boom"));
    let mut provider = HttpProvider::new(&base, "m", 5_000).expect("provider");
    let error = provider.complete(&request()).expect_err("must fail");
    assert_eq!(error, ProviderError::Status(500, "boom".to_string()));
    let _ = handle.join();
}

#[test]
fn malformed_json_is_refused() {
    let (base, handle) = serve(response(200, "OK", "{"));
    let mut provider = HttpProvider::new(&base, "m", 5_000).expect("provider");
    let error = provider.complete(&request()).expect_err("must fail");
    assert!(matches!(error, ProviderError::Malformed(_)), "{error:?}");
    let _ = handle.join();
}

#[test]
fn content_parts_are_refused_rather_than_guessed() {
    let body = r#"{"choices":[{"message":{"content":[{"type":"text","text":"hi"}]}}]}"#;
    let (base, handle) = serve(response(200, "OK", body));
    let mut provider = HttpProvider::new(&base, "m", 5_000).expect("provider");
    let error = provider.complete(&request()).expect_err("must fail");
    let ProviderError::Malformed(reason) = error else {
        panic!("wrong error: {error:?}");
    };
    assert!(reason.contains("content parts"), "{reason}");
    let _ = handle.join();
}

#[test]
fn a_response_without_content_length_is_refused() {
    let raw = b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n{}".to_vec();
    let (base, handle) = serve(raw);
    let mut provider = HttpProvider::new(&base, "m", 5_000).expect("provider");
    let error = provider.complete(&request()).expect_err("must fail");
    let ProviderError::Malformed(reason) = error else {
        panic!("wrong error: {error:?}");
    };
    assert!(reason.contains("content-length"), "{reason}");
    let _ = handle.join();
}

#[test]
fn chunked_transfer_is_refused_by_name() {
    let raw =
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n".to_vec();
    let (base, handle) = serve(raw);
    let mut provider = HttpProvider::new(&base, "m", 5_000).expect("provider");
    let error = provider.complete(&request()).expect_err("must fail");
    let ProviderError::Malformed(reason) = error else {
        panic!("wrong error: {error:?}");
    };
    assert!(reason.contains("chunked"), "{reason}");
    let _ = handle.join();
}

#[test]
fn an_oversized_declared_body_is_refused_before_it_is_read() {
    let raw = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        5 * 1024 * 1024
    )
    .into_bytes();
    let (base, handle) = serve(raw);
    let mut provider = HttpProvider::new(&base, "m", 5_000).expect("provider");
    let error = provider.complete(&request()).expect_err("must fail");
    assert!(matches!(error, ProviderError::Limit(_)), "{error:?}");
    let _ = handle.join();
}

#[test]
fn a_silent_endpoint_times_out() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let _ = read_request(&mut stream);
        thread::sleep(Duration::from_secs(2));
    });
    let mut provider =
        HttpProvider::new(&format!("http://127.0.0.1:{port}/v1"), "m", 100).expect("provider");
    let error = provider.complete(&request()).expect_err("must fail");
    assert_eq!(error, ProviderError::Timeout, "{error:?}");
}

#[test]
fn https_is_refused_at_construction_with_a_reason() {
    let error = HttpProvider::new("https://example.test/v1", "m", 100).expect_err("must fail");
    let ProviderError::Malformed(reason) = error else {
        panic!("wrong error: {error:?}");
    };
    assert!(reason.contains("plain HTTP only"), "{reason}");
}

#[test]
fn a_missing_model_name_is_refused() {
    let error = HttpProvider::new("http://127.0.0.1:1/v1", " ", 100).expect_err("must fail");
    assert!(matches!(error, ProviderError::Malformed(_)), "{error:?}");
}
