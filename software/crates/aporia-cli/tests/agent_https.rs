//! The HTTPS path through the real binary: the bearer key comes from the environment the user
//! names, the session runs over TLS against a local test server, and the key never reaches the
//! record or the command's own output.

use native_tls::{Identity, TlsAcceptor};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

const KEY: &str = "cli-test-key-0123456789abcdef";

struct Temp(PathBuf);

impl Temp {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "aporia-cli-https-{tag}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).expect("temp");
        Self(path)
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn identity() -> Identity {
    #[cfg(windows)]
    {
        Identity::from_pkcs12(
            include_bytes!("../../aporia-agent/tests/fixtures/tls/identity.pfx"),
            "test",
        )
        .expect("test identity")
    }
    #[cfg(not(windows))]
    {
        Identity::from_pkcs8(include_bytes!(
            "../../aporia-agent/tests/fixtures/tls/identity.pem"
        ))
        .expect("test identity")
    }
}

/// One TLS server answering one chat-completions request with a `finish` call.
fn spawn_finish_server() -> (u16, std::thread::JoinHandle<Option<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let acceptor = TlsAcceptor::new(identity()).expect("acceptor");
    let handle = std::thread::spawn(move || {
        let Ok((stream, _)) = listener.accept() else {
            return None;
        };
        let Ok(mut stream) = acceptor.accept(stream) else {
            return None;
        };
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        let header_end = loop {
            if let Some(at) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break at + 4;
            }
            let Ok(read) = stream.read(&mut chunk) else {
                return None;
            };
            if read == 0 {
                return None;
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
                return None;
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
        let body = r#"{"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call-finish","type":"function","function":{"name":"finish","arguments":"{\"summary\":\"done over TLS\",\"next_steps\":[],\"unresolved\":[]}"}}]}}]}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.flush();
        authorization
    });
    (port, handle)
}

/// Scan every byte of a directory tree for the credential.
fn scan(dir: &std::path::Path) {
    for entry in std::fs::read_dir(dir).expect("dir") {
        let entry = entry.expect("entry");
        let path = entry.path();
        if path.is_dir() {
            scan(&path);
        } else if let Ok(bytes) = std::fs::read(&path) {
            assert!(
                !String::from_utf8_lossy(&bytes).contains(KEY),
                "the key leaked into {}",
                path.display()
            );
        }
    }
}

#[test]
fn the_binary_runs_a_session_over_tls_with_an_environment_credential() {
    let (port, server) = spawn_finish_server();
    let root = Temp::new("run");
    let source = root.0.join("src");
    let workdir = root.0.join("work");
    std::fs::create_dir_all(&source).expect("dir");
    std::fs::write(source.join("notes.md"), b"# notes\n").expect("file");

    let out = Command::new(env!("CARGO_BIN_EXE_aporia"))
        .arg("agent")
        .arg(&source)
        .args(["--objective", "read the notes over TLS"])
        .arg("--provider")
        .arg(format!("https://localhost:{port}/v1"))
        .args(["--model", "test-model"])
        .args(["--api-key-env", "APORIA_CLI_HTTPS_TEST_KEY"])
        .arg("--ca-file")
        .arg(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../aporia-agent/tests/fixtures/tls/cert.pem"),
        )
        .arg("--workdir")
        .arg(&workdir)
        .arg("--batch")
        .env("APORIA_CLI_HTTPS_TEST_KEY", KEY)
        .stdin(Stdio::null())
        .output()
        .expect("aporia runs");
    assert_eq!(
        out.status.code(),
        Some(0),
        "stdout: {} stderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let authorization = server.join().expect("server thread");
    assert_eq!(
        authorization.as_deref(),
        Some(format!("Bearer {KEY}").as_str()),
        "the environment credential did not reach the request"
    );

    // The key is in neither the command's output nor any byte of the record.
    assert!(!String::from_utf8_lossy(&out.stdout).contains(KEY));
    assert!(!String::from_utf8_lossy(&out.stderr).contains(KEY));
    scan(&workdir);
    let mut records: Vec<PathBuf> = std::fs::read_dir(&workdir)
        .expect("records")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    records.sort();
    assert_eq!(records.len(), 1);
    let journal = std::fs::read_to_string(records[0].join("journal.jsonl")).expect("journal");
    assert!(journal.contains("https://localhost"), "{journal}");
    assert!(
        journal.contains("bearer auth from environment"),
        "{journal}"
    );
}

#[test]
fn an_api_key_env_without_https_is_a_usage_error() {
    let root = Temp::new("usage");
    let source = root.0.join("src");
    std::fs::create_dir_all(&source).expect("dir");
    let transcript = root.0.join("t.json");
    std::fs::write(
        &transcript,
        r#"{"note":"t","responses":[{"content":null,"tool_calls":[{"id":"f","name":"finish","arguments":{"summary":"s","next_steps":[],"unresolved":[]}}]}]}"#,
    )
    .expect("transcript");
    let out = Command::new(env!("CARGO_BIN_EXE_aporia"))
        .arg("agent")
        .arg(&source)
        .args(["--objective", "x"])
        .arg("--provider")
        .arg(format!("scripted:{}", transcript.display()))
        .args(["--api-key-env", "SOME_VAR"])
        .stdin(Stdio::null())
        .output()
        .expect("aporia runs");
    assert_eq!(out.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&out.stderr)
            .contains("--api-key-env is only used by an HTTPS provider"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
