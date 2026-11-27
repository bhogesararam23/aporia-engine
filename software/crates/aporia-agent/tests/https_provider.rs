//! The HTTPS provider's contract, against a real local TLS server.
//!
//! The server is `native-tls`'s own acceptor with a test-only self-signed localhost certificate
//! (committed under `fixtures/tls`, never a real credential). The client verifies it through an
//! added root -- verification is never disabled. These tests exercise the wire, the bearer
//! credential, redaction, bounded retries, and the promise that the key never reaches a record.

use aporia_agent::engine::{AutoApprove, Engine, EngineConfig, Wait};
use aporia_agent::provider::https::HttpsProvider;
use aporia_agent::provider::{Message, ModelProvider, ModelRequest, ProviderError};
use aporia_agent::tools::{Registry, control, files};
use native_tls::{Identity, TlsAcceptor};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

const KEY: &str = "test-key-9f8e7d6c5b4a3210";
const CERT: &[u8] = include_bytes!("fixtures/tls/cert.pem");

struct Received {
    authorization: Option<String>,
    body: String,
}

struct MockTls {
    addr: SocketAddr,
    received: Arc<Mutex<Vec<Received>>>,
    handle: Option<JoinHandle<()>>,
}

impl MockTls {
    fn join(mut self) -> Vec<Received> {
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        let mut received = self.received.lock().expect("lock");
        std::mem::take(&mut *received)
    }
}

impl Drop for MockTls {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn identity() -> Identity {
    #[cfg(windows)]
    {
        Identity::from_pkcs12(include_bytes!("fixtures/tls/identity.pfx"), "test")
            .expect("test identity (pkcs12)")
    }
    #[cfg(not(windows))]
    {
        Identity::from_pkcs8(include_bytes!("fixtures/tls/identity.pem"))
            .expect("test identity (pkcs8)")
    }
}

/// A TLS server that answers each request with the next scripted response.
fn spawn(responses: Vec<(u16, String)>) -> MockTls {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let acceptor = TlsAcceptor::new(identity()).expect("acceptor");
    let received = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&received);
    let handle = std::thread::spawn(move || {
        for (status, body) in responses {
            let Ok((stream, _)) = listener.accept() else {
                return;
            };
            let Ok(mut stream) = acceptor.accept(stream) else {
                return;
            };
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            let header_end = loop {
                if let Some(at) = find(&buf, b"\r\n\r\n") {
                    break at + 4;
                }
                let Ok(read) = stream.read(&mut chunk) else {
                    return;
                };
                if read == 0 {
                    return;
                }
                buf.extend_from_slice(&chunk[..read]);
            };
            let headers = String::from_utf8_lossy(&buf[..header_end]).to_string();
            let length: usize = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    if name.trim().eq_ignore_ascii_case("content-length") {
                        value.trim().parse().ok()
                    } else {
                        None
                    }
                })
                .unwrap_or(0);
            while buf.len() < header_end + length {
                let Ok(read) = stream.read(&mut chunk) else {
                    return;
                };
                if read == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..read]);
            }
            let authorization = headers.lines().find_map(|line| {
                let (name, value) = line.split_once(':')?;
                if name.trim().eq_ignore_ascii_case("authorization") {
                    Some(value.trim().to_string())
                } else {
                    None
                }
            });
            sink.lock().expect("lock").push(Received {
                authorization,
                body: String::from_utf8_lossy(&buf[header_end..header_end + length]).to_string(),
            });
            let reason = if status == 200 { "OK" } else { "Error" };
            let response = format!(
                "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
    });
    MockTls {
        addr,
        received,
        handle: Some(handle),
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn client(server: &MockTls) -> HttpsProvider {
    HttpsProvider::new(
        &format!("https://localhost:{}/v1", server.addr.port()),
        "test-model",
        5_000,
    )
    .expect("client")
    .with_root_certificate_pem(CERT)
    .expect("root")
    .with_api_key(KEY.to_string())
    .expect("key")
}

fn request() -> ModelRequest {
    ModelRequest {
        messages: vec![Message::system("you are a test"), Message::user("hello")],
        tools: Vec::new(),
    }
}

fn completion(content: &str) -> String {
    format!(
        r#"{{"choices":[{{"message":{{"role":"assistant","content":{}}}}}]}}"#,
        aporia_store::Json::text(content).to_compact()
    )
}

/// Scan every byte of a directory tree for the credential.
fn scan(dir: &std::path::Path) {
    for entry in std::fs::read_dir(dir).expect("dir") {
        let entry = entry.expect("entry");
        let path = entry.path();
        if path.is_dir() {
            scan(&path);
        } else if let Ok(bytes) = std::fs::read(&path) {
            let text = String::from_utf8_lossy(&bytes);
            assert!(
                !text.contains(KEY),
                "the API key leaked into {}",
                path.display()
            );
        }
    }
}

#[test]
fn a_tls_round_trip_carries_the_bearer_credential_and_parses_the_answer() {
    let server = spawn(vec![(200, completion("the answer"))]);
    let mut provider = client(&server);
    let response = provider.complete(&request()).expect("complete");
    assert_eq!(response.content.as_deref(), Some("the answer"));
    let received = server.join();
    assert_eq!(received.len(), 1);
    assert_eq!(
        received[0].authorization.as_deref(),
        Some(format!("Bearer {KEY}").as_str())
    );
    assert!(received[0].body.contains("\"model\":\"test-model\""));
    assert!(received[0].body.contains("\"stream\":false"));
}

#[test]
fn tool_calls_survive_the_tls_wire() {
    let body = r#"{"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":"read_file","arguments":"{\"path\":\"a.txt\"}"}}]}}]}"#;
    let server = spawn(vec![(200, body.to_string())]);
    let mut provider = client(&server);
    let response = provider.complete(&request()).expect("complete");
    assert_eq!(response.tool_calls.len(), 1);
    assert_eq!(response.tool_calls[0].name, "read_file");
    assert_eq!(response.tool_calls[0].id, "call_1");
    let _ = server.join();
}

#[test]
fn an_error_body_that_echoes_the_key_is_redacted() {
    let echo = format!("{{\"error\":\"invalid key {KEY}\"}}");
    let server = spawn(vec![(401, echo)]);
    let mut provider = client(&server);
    let error = provider.complete(&request()).expect_err("401");
    let ProviderError::Status(code, body) = &error else {
        panic!("expected a status error, got {error:?}");
    };
    assert_eq!(*code, 401);
    assert!(body.contains("[redacted]"), "{body}");
    assert!(!body.contains(KEY), "the key leaked into the error: {body}");
    assert!(
        !provider.describe().contains(KEY),
        "{}",
        provider.describe()
    );
    assert!(!format!("{provider:?}").contains(KEY), "{provider:?}");
    let received = server.join();
    assert_eq!(
        received.len(),
        1,
        "a 401 is a decision, not a transient failure"
    );
}

#[test]
fn a_503_is_retried_and_then_succeeds() {
    let server = spawn(vec![
        (503, "{\"error\":\"busy\"}".to_string()),
        (200, completion("recovered")),
    ]);
    let mut provider = client(&server);
    let response = provider.complete(&request()).expect("complete");
    assert_eq!(response.content.as_deref(), Some("recovered"));
    assert_eq!(server.join().len(), 2);
}

#[test]
fn retries_are_bounded_and_the_last_error_is_returned() {
    let server = spawn(vec![
        (503, "{\"error\":\"busy\"}".to_string()),
        (503, "{\"error\":\"busy\"}".to_string()),
        (503, "{\"error\":\"busy\"}".to_string()),
    ]);
    let mut provider = client(&server);
    let error = provider.complete(&request()).expect_err("exhausted");
    assert!(matches!(error, ProviderError::Status(503, _)), "{error:?}");
    assert_eq!(server.join().len(), 3, "the retry budget is three attempts");
}

#[test]
fn a_malformed_answer_is_not_retried() {
    let server = spawn(vec![(200, "this is not json".to_string())]);
    let mut provider = client(&server);
    let error = provider.complete(&request()).expect_err("malformed");
    assert!(matches!(error, ProviderError::Malformed(_)), "{error:?}");
    assert_eq!(server.join().len(), 1);
}

#[test]
fn an_untrusted_certificate_is_refused() {
    let server = spawn(vec![(200, completion("never seen"))]);
    // No added root: the platform store does not know the test CA, so the handshake must fail.
    let mut provider = HttpsProvider::new(
        &format!("https://localhost:{}/v1", server.addr.port()),
        "test-model",
        2_000,
    )
    .expect("client");
    let error = provider.complete(&request()).expect_err("untrusted");
    assert!(matches!(error, ProviderError::Unreachable(_)), "{error:?}");
    let _ = server.join();
}

#[test]
fn a_named_but_unset_key_variable_is_a_refusal() {
    let provider = HttpsProvider::new("https://example.test/v1", "m", 100).expect("client");
    let error = provider
        .with_api_key_env("APORIA_NO_SUCH_KEY_VARIABLE")
        .expect_err("unset");
    assert!(matches!(error, ProviderError::Malformed(_)), "{error:?}");
}

#[test]
fn the_key_never_reaches_the_investigation_record() {
    // Two turns over real TLS: a read_file call, then finish. The record must not contain the key.
    let read_turn = r#"{"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call-read","type":"function","function":{"name":"read_file","arguments":"{\"path\":\"notes.md\"}"}}]}}]}"#;
    let finish_turn = r#"{"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call-finish","type":"function","function":{"name":"finish","arguments":"{\"summary\":\"done\",\"next_steps\":[],\"unresolved\":[]}"}}]}}]}"#;
    let server = spawn(vec![
        (200, read_turn.to_string()),
        (200, finish_turn.to_string()),
    ]);
    let provider = client(&server);

    let root = std::env::temp_dir().join(format!("aporia-https-leak-{}", std::process::id()));
    let source = root.join("src");
    let workdir = root.join("work");
    std::fs::create_dir_all(&source).expect("dir");
    std::fs::write(source.join("notes.md"), b"# notes\n").expect("file");
    let mut registry = Registry::new();
    control::register(&mut registry).expect("control");
    files::register(&mut registry).expect("files");
    let config = EngineConfig {
        source: source.clone(),
        objective: "read the notes over TLS".to_string(),
        provider: Box::new(provider),
        allow_exec: Vec::new(),
        allow_write: false,
        max_turns: 8,
        max_tool_calls: 16,
        total_timeout_ms: 60_000,
        workdir: workdir.clone(),
        instrument: std::env::current_exe().expect("exe"),
    };
    let mut engine = Engine::start(config, Box::new(AutoApprove), registry).expect("engine");
    let outcome = match engine.resume().expect("resume") {
        Wait::Done(outcome) => outcome,
        Wait::Approval(_) => panic!("auto controller"),
    };
    assert_eq!(outcome, aporia_agent::engine::Outcome::Completed);
    let received = server.join();
    assert_eq!(received.len(), 2);
    assert!(
        received
            .iter()
            .all(|r| r.authorization.as_deref() == Some(format!("Bearer {KEY}").as_str()))
    );

    // Every byte of the record is scanned for the key.
    scan(&workdir);
    let _ = std::fs::remove_dir_all(&root);
}
