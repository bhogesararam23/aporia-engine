//! The command tool: what the model may run, where, and what it learns when it fails.
//!
//! The fixture binary is copied into a temporary directory and reached by a bare program name
//! through PATH, so the tests exercise the same rule the tool enforces: the program is a bare
//! name, the host approves it, and the path to it is not the model's business.

use aporia_agent::provider::ToolCall;
use aporia_agent::scope::Scope;
use aporia_agent::tools::command::register;
use aporia_agent::tools::result::ErrorClass;
use aporia_agent::tools::{Registry, ToolContext, ToolResult, dispatch};
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
            "aporia-command-{tag}-{}-{}",
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

/// A directory holding the fixture under a bare name, reachable through PATH.
fn program_dir(temp: &Temp) -> PathBuf {
    let dir = temp.path().join("bin");
    fs::create_dir_all(&dir).expect("bin");
    let name = if cfg!(windows) {
        "agent-fixture.exe"
    } else {
        "agent-fixture"
    };
    fs::copy(env!("CARGO_BIN_EXE_agent-fixture"), dir.join(name)).expect("copy the fixture");
    dir
}

fn fixture() -> (Temp, Temp, Sandbox, PathBuf) {
    let source = Temp::new("src");
    fs::create_dir_all(source.0.join("work")).expect("dir");
    fs::write(source.0.join("work/input.txt"), b"data\n").expect("file");
    let workspace = Temp::new("ws");
    let (sandbox, _) = Sandbox::create(&source.0, &workspace.0.join("copy")).expect("sandbox");
    let instrument = std::env::current_exe().expect("exe");
    (source, workspace, sandbox, instrument)
}

fn scope(program: &str) -> Scope {
    Scope {
        allow_exec: vec![program.to_string()],
        allow_write: true,
        max_turns: 8,
        max_tool_calls: 16,
        timeout_ms: 60_000,
    }
}

fn run(
    sandbox: &Sandbox,
    source: &Path,
    instrument: &Path,
    scope: Option<&Scope>,
    arguments: &str,
) -> ToolResult {
    let mut registry = Registry::new();
    register(&mut registry).expect("register");
    let cancel = AtomicBool::new(false);
    let mut context = ToolContext::new(sandbox, source, instrument, scope, &cancel);
    let call = ToolCall {
        id: "call-1".to_string(),
        name: "run_command".to_string(),
        arguments: arguments.to_string(),
    };
    dispatch(&registry, &mut context, &call)
}

/// An argv that runs the fixture by bare name, with PATH pointing at it. `extra` adds another
/// top-level field to the arguments object.
fn command_json(
    program: &str,
    bin: &Path,
    arguments: &[&str],
    env: &[(&str, &str)],
    extra: &str,
) -> String {
    let mut items = vec![format!("\"{program}\"")];
    for argument in arguments {
        items.push(format!("\"{argument}\""));
    }
    let mut variables = vec![format!(
        "\"PATH\":\"{}\"",
        bin.to_string_lossy().replace('\\', "/")
    )];
    for (name, value) in env {
        variables.push(format!("\"{name}\":\"{value}\""));
    }
    format!(
        "{{\"argv\":[{}],\"env\":{{{}}}{extra}}}",
        items.join(","),
        variables.join(",")
    )
}

fn argv(program: &str, bin: &Path, arguments: &[&str]) -> String {
    command_json(program, bin, arguments, &[], "")
}

#[test]
fn an_unapproved_program_is_denied_before_it_starts() {
    let (source, workspace, sandbox, instrument) = fixture();
    let bin = program_dir(&workspace);
    let failure = run(
        &sandbox,
        source.path(),
        &instrument,
        None,
        &argv("agent-fixture", &bin, &["echo", "hi"]),
    )
    .expect_err("no scope");
    assert_eq!(failure.class, ErrorClass::Denied);
}

#[test]
fn a_program_outside_the_approved_scope_is_denied() {
    let (source, workspace, sandbox, instrument) = fixture();
    let bin = program_dir(&workspace);
    let approved = scope("python");
    let failure = run(
        &sandbox,
        source.path(),
        &instrument,
        Some(&approved),
        &argv("agent-fixture", &bin, &["echo", "hi"]),
    )
    .expect_err("not approved");
    assert_eq!(failure.class, ErrorClass::Denied);
    assert!(
        failure.message.contains("not in the approved scope"),
        "{}",
        failure.message
    );
}

#[test]
fn a_successful_command_returns_its_output_and_duration() {
    let (source, workspace, sandbox, instrument) = fixture();
    let bin = program_dir(&workspace);
    let approved = scope("agent-fixture");
    let output = run(
        &sandbox,
        source.path(),
        &instrument,
        Some(&approved),
        &argv("agent-fixture", &bin, &["echo", "hello there"]),
    )
    .expect("ok");
    assert!(output.text.contains("hello there"), "{}", output.text);
    let fields = aporia_agent::json::fields(&output.data).expect("data");
    assert_eq!(aporia_agent::json::u64_value(fields, "exit_code"), Ok(0));
    assert!(aporia_agent::json::u64_value(fields, "duration_ms").is_ok());
    assert_eq!(
        aporia_agent::json::string(fields, "stdout_sha256").map(str::len),
        Ok(64)
    );
}

#[test]
fn a_nonzero_exit_is_a_failure_with_the_streams_attached() {
    let (source, workspace, sandbox, instrument) = fixture();
    let bin = program_dir(&workspace);
    let approved = scope("agent-fixture");
    let failure = run(
        &sandbox,
        source.path(),
        &instrument,
        Some(&approved),
        &argv("agent-fixture", &bin, &["fail", "3"]),
    )
    .expect_err("exit 3");
    assert_eq!(failure.class, ErrorClass::Execution);
    assert!(failure.message.contains("exited 3"), "{}", failure.message);
    let fields = aporia_agent::json::fields(&failure.data).expect("data");
    assert_eq!(aporia_agent::json::u64_value(fields, "exit_code"), Ok(3));
}

#[test]
fn a_declared_artefact_that_was_not_produced_is_reported() {
    let (source, workspace, sandbox, instrument) = fixture();
    let bin = program_dir(&workspace);
    let approved = scope("agent-fixture");
    let arguments = command_json(
        "agent-fixture",
        &bin,
        &["echo", "x"],
        &[],
        ",\"artifacts\":[\"out/result.json\"]",
    );
    let failure = run(
        &sandbox,
        source.path(),
        &instrument,
        Some(&approved),
        &arguments,
    )
    .expect_err("missing artefact");
    assert_eq!(failure.class, ErrorClass::NotFound);
}

#[test]
fn a_credential_shaped_variable_is_refused() {
    let (source, workspace, sandbox, instrument) = fixture();
    let bin = program_dir(&workspace);
    let approved = scope("agent-fixture");
    let arguments = command_json(
        "agent-fixture",
        &bin,
        &["echo", "x"],
        &[("OPENAI_API_KEY", "sk-x")],
        "",
    );
    let failure = run(
        &sandbox,
        source.path(),
        &instrument,
        Some(&approved),
        &arguments,
    )
    .expect_err("credential");
    assert_eq!(failure.class, ErrorClass::Denied);
}

#[test]
fn a_path_that_leaves_the_workspace_is_refused() {
    let (source, workspace, sandbox, instrument) = fixture();
    let bin = program_dir(&workspace);
    let approved = scope("agent-fixture");
    let arguments = command_json(
        "agent-fixture",
        &bin,
        &["echo", "x"],
        &[],
        ",\"cwd\":\"../elsewhere\"",
    );
    let failure = run(
        &sandbox,
        source.path(),
        &instrument,
        Some(&approved),
        &arguments,
    )
    .expect_err("escape");
    assert_eq!(failure.class, ErrorClass::Denied);
}

#[test]
fn a_command_runs_in_the_directory_it_names() {
    let (source, workspace, sandbox, instrument) = fixture();
    let bin = program_dir(&workspace);
    let approved = scope("agent-fixture");
    let output = run(
        &sandbox,
        source.path(),
        &instrument,
        Some(&approved),
        &argv("agent-fixture", &bin, &["cwd"]),
    )
    .expect("ok");
    assert!(output.text.contains("copy"), "{}", output.text);
}

#[test]
fn the_schema_of_the_command_tool_is_what_the_model_is_offered() {
    let mut registry = Registry::new();
    register(&mut registry).expect("register");
    let schemas = registry.schemas();
    assert_eq!(schemas.len(), 1);
    assert_eq!(schemas[0].name, "run_command");
    let fields = aporia_agent::json::fields(&schemas[0].parameters).expect("schema");
    assert!(aporia_agent::json::field(fields, "required").is_some());
    let _ = Json::Null;
}
