//! A transcript is consumed in order, its arguments are not retyped, and it does not invent a
//! reply when it runs out.

use aporia_agent::provider::scripted::ScriptedProvider;
use aporia_agent::{ModelProvider, ModelRequest, ProviderError};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn temp_file(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "aporia-agent-scripted-{}-{}-{name}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

fn request() -> ModelRequest {
    ModelRequest {
        messages: vec![],
        tools: vec![],
    }
}

#[test]
fn a_transcript_is_consumed_in_order_and_arguments_are_kept_as_given() {
    let path = temp_file("good.json");
    std::fs::write(
        &path,
        r#"{
            "note": "human-authored",
            "responses": [
                {"tool_calls": [{"id": "c1", "name": "propose_plan", "arguments": {"gaps": []}}]},
                {"content": "done"}
            ]
        }"#,
    )
    .expect("transcript");

    let mut provider = ScriptedProvider::load(&path).expect("load");
    let first = provider.complete(&request()).expect("first");
    assert_eq!(first.tool_calls.len(), 1);
    assert_eq!(first.tool_calls[0].name, "propose_plan");
    assert_eq!(first.tool_calls[0].arguments, r#"{"gaps":[]}"#);
    let second = provider.complete(&request()).expect("second");
    assert_eq!(second.content.as_deref(), Some("done"));
    assert_eq!(provider.complete(&request()), Err(ProviderError::Exhausted));

    let described = provider.describe();
    assert!(described.contains("scripted transcript"), "{described}");
    assert!(described.contains("sha256"), "{described}");
    let _ = std::fs::remove_file(path);
}

#[test]
fn a_string_argument_is_passed_through_unchanged() {
    let path = temp_file("string.json");
    std::fs::write(
        &path,
        r#"{"responses": [{"tool_calls": [{"id": "c1", "name": "read_file", "arguments": "{\"path\":\"a.ap\"}"}]}]}"#,
    )
    .expect("transcript");
    let mut provider = ScriptedProvider::load(&path).expect("load");
    let response = provider.complete(&request()).expect("response");
    assert_eq!(response.tool_calls[0].arguments, r#"{"path":"a.ap"}"#);
    let _ = std::fs::remove_file(path);
}

#[test]
fn a_missing_transcript_is_a_named_error() {
    let error = ScriptedProvider::load(&temp_file("absent.json")).expect_err("must fail");
    assert!(matches!(error, ProviderError::Malformed(_)), "{error:?}");
}

#[test]
fn an_empty_response_is_refused_at_load_time() {
    let path = temp_file("empty.json");
    std::fs::write(&path, r#"{"responses": [{}]}"#).expect("transcript");
    let error = ScriptedProvider::load(&path).expect_err("must fail");
    let ProviderError::Malformed(reason) = error else {
        panic!("wrong error: {error:?}");
    };
    assert!(
        reason.contains("text or at least one tool call"),
        "{reason}"
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn an_unknown_field_is_refused_rather_than_ignored() {
    let path = temp_file("unknown.json");
    std::fs::write(
        &path,
        r#"{"responses": [{"content": "x", "toool_calls": []}]}"#,
    )
    .expect("transcript");
    let error = ScriptedProvider::load(&path).expect_err("must fail");
    let ProviderError::Malformed(reason) = error else {
        panic!("wrong error: {error:?}");
    };
    assert!(reason.contains("unknown field"), "{reason}");
    let _ = std::fs::remove_file(path);
}
