//! The editing lifecycle: a change lands in the workspace, the original is untouched, and the
//! change can be put back.

use aporia_agent::investigation::{FileChange, Investigation};
use aporia_agent::orchestrator::Delivery;
use aporia_agent::provider::ToolCall;
use aporia_agent::scope::{ApprovalRequest, Scope, ScopeLimits};
use aporia_agent::tools::context::Session;
use aporia_agent::tools::{Registry, ToolContext, dispatch, edit};
use aporia_sandbox::Sandbox;
use aporia_store::Json;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Temp(PathBuf);

impl Temp {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "aporia-edit-{tag}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("temp");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A session that keeps the changes and writes them down.
struct TestSession {
    investigation: Investigation,
    events: Vec<String>,
}

impl Session for TestSession {
    fn investigation(&mut self) -> &mut Investigation {
        &mut self.investigation
    }

    fn journal(&mut self, kind: &str, payload: Json) -> Result<(), String> {
        self.events.push(format!("{kind} {}", payload.to_compact()));
        Ok(())
    }

    fn request_approval(&mut self, _request: ApprovalRequest) {}

    fn limits(&self) -> ScopeLimits {
        ScopeLimits {
            allow_exec: Vec::new(),
            allow_write: true,
            max_turns: 8,
            max_tool_calls: 16,
            timeout_ms: 60_000,
        }
    }

    fn deliver(&mut self, _delivery: Delivery) -> bool {
        true
    }
}

fn call(name: &str, arguments: &str) -> ToolCall {
    ToolCall {
        id: "call-1".to_string(),
        name: name.to_string(),
        arguments: arguments.to_string(),
    }
}

#[test]
fn a_write_lands_in_the_workspace_and_the_original_is_untouched() {
    let source = Temp::new("src");
    fs::write(source.path().join("rule.py"), b"original\n").expect("file");
    let workspace = Temp::new("ws");
    let (sandbox, _) =
        Sandbox::create(source.path(), &workspace.path().join("copy")).expect("sandbox");
    let instrument = std::env::current_exe().expect("exe");
    let scope = Scope {
        allow_exec: Vec::new(),
        allow_write: true,
        max_turns: 8,
        max_tool_calls: 16,
        timeout_ms: 60_000,
    };
    let mut registry = Registry::new();
    edit::register(&mut registry).expect("edit tools");
    let mut session = TestSession {
        investigation: Investigation::default(),
        events: Vec::new(),
    };
    let cancel = AtomicBool::new(false);
    let mut context = ToolContext::new(&sandbox, source.path(), &instrument, Some(&scope), &cancel)
        .with_session(&mut session);

    let output = dispatch(
        &registry,
        &mut context,
        &call("write_file", r#"{"path":"rule.py","content":"changed\n"}"#),
    )
    .expect("write");
    assert!(output.text.contains("wrote rule.py"), "{}", output.text);

    let in_workspace = fs::read_to_string(workspace.path().join("copy/rule.py")).expect("read");
    assert_eq!(in_workspace, "changed\n");
    let in_source = fs::read_to_string(source.path().join("rule.py")).expect("read");
    assert_eq!(in_source, "original\n", "the original project was written");

    assert_eq!(session.investigation.changes.len(), 1);
    let change: &FileChange = &session.investigation.changes[0];
    assert_eq!(change.path, "rule.py");
    assert!(change.before.is_some());
    assert!(change.after.is_some());
    assert_ne!(change.before, change.after);
    assert!(
        session
            .events
            .iter()
            .any(|event| event.contains("change.applied"))
    );
}

#[test]
fn a_write_without_approved_write_permission_is_refused() {
    let source = Temp::new("src2");
    fs::write(source.path().join("rule.py"), b"original\n").expect("file");
    let workspace = Temp::new("ws2");
    let (sandbox, _) =
        Sandbox::create(source.path(), &workspace.path().join("copy")).expect("sandbox");
    let instrument = std::env::current_exe().expect("exe");
    let mut registry = Registry::new();
    edit::register(&mut registry).expect("edit tools");
    let cancel = AtomicBool::new(false);
    let mut context = ToolContext::new(&sandbox, source.path(), &instrument, None, &cancel);
    let failure = dispatch(
        &registry,
        &mut context,
        &call("write_file", r#"{"path":"rule.py","content":"changed\n"}"#),
    )
    .expect_err("no write permission");
    assert_eq!(failure.class, aporia_agent::tools::ErrorClass::Denied);
    assert_eq!(
        fs::read_to_string(source.path().join("rule.py")).expect("read"),
        "original\n"
    );
}

#[test]
fn a_change_can_be_put_back_and_a_new_file_can_be_removed() {
    let source = Temp::new("src3");
    fs::write(source.path().join("rule.py"), b"original\n").expect("file");
    let workspace = Temp::new("ws3");
    let (sandbox, _) =
        Sandbox::create(source.path(), &workspace.path().join("copy")).expect("sandbox");
    let instrument = std::env::current_exe().expect("exe");
    let scope = Scope {
        allow_exec: Vec::new(),
        allow_write: true,
        max_turns: 8,
        max_tool_calls: 16,
        timeout_ms: 60_000,
    };
    let mut registry = Registry::new();
    edit::register(&mut registry).expect("edit tools");
    let mut session = TestSession {
        investigation: Investigation::default(),
        events: Vec::new(),
    };
    let cancel = AtomicBool::new(false);
    let mut context = ToolContext::new(&sandbox, source.path(), &instrument, Some(&scope), &cancel)
        .with_session(&mut session);

    dispatch(
        &registry,
        &mut context,
        &call("write_file", r#"{"path":"rule.py","content":"changed\n"}"#),
    )
    .expect("write");
    dispatch(
        &registry,
        &mut context,
        &call(
            "write_file",
            r#"{"path":"extra.txt","content":"new file\n"}"#,
        ),
    )
    .expect("create");
    dispatch(
        &registry,
        &mut context,
        &call("revert_file", r#"{"path":"rule.py"}"#),
    )
    .expect("revert");
    dispatch(
        &registry,
        &mut context,
        &call("revert_file", r#"{"path":"extra.txt"}"#),
    )
    .expect("remove the new file");

    assert_eq!(
        fs::read_to_string(workspace.path().join("copy/rule.py")).expect("read"),
        "original\n",
        "the file was not put back"
    );
    assert!(
        !workspace.path().join("copy/extra.txt").exists(),
        "the created file was not removed"
    );
    assert_eq!(session.investigation.changes.len(), 4);
}

#[test]
fn a_path_that_leaves_the_workspace_is_refused() {
    let source = Temp::new("src4");
    fs::write(source.path().join("rule.py"), b"original\n").expect("file");
    let workspace = Temp::new("ws4");
    let (sandbox, _) =
        Sandbox::create(source.path(), &workspace.path().join("copy")).expect("sandbox");
    let instrument = std::env::current_exe().expect("exe");
    let scope = Scope {
        allow_exec: Vec::new(),
        allow_write: true,
        max_turns: 8,
        max_tool_calls: 16,
        timeout_ms: 60_000,
    };
    let mut registry = Registry::new();
    edit::register(&mut registry).expect("edit tools");
    let cancel = AtomicBool::new(false);
    let mut context = ToolContext::new(&sandbox, source.path(), &instrument, Some(&scope), &cancel);
    let failure = dispatch(
        &registry,
        &mut context,
        &call("write_file", r#"{"path":"../outside.py","content":"x"}"#),
    )
    .expect_err("escape");
    assert_eq!(failure.class, aporia_agent::tools::ErrorClass::Denied);
}
