//! Execution records what the child did: a real archive, a nonzero exit, a launch failure, a
//! timeout -- and never turns any of them into a different story.

use aporia_agent::execute::execute;
use aporia_agent::plan::{Operation, Task, TaskTool, ValidatedTask};
use aporia_sandbox::Sandbox;
use aporia_store::Json;
use aporia_store::digest::sha256_hex;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Temp(PathBuf);

impl Temp {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "aporia-agent-execute-{}-{}-{tag}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).expect("temp");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn sandbox() -> (Temp, Sandbox) {
    let temp = Temp::new("work");
    let source = temp.path().join("source");
    std::fs::create_dir_all(&source).expect("source");
    std::fs::write(source.join("model.ap"), b"model x { }\n").expect("model");
    let workspace = temp.path().join("workspace");
    let (sandbox, _) = Sandbox::create(&source, &workspace).expect("sandbox");
    (temp, sandbox)
}

fn fixture() -> String {
    env!("CARGO_BIN_EXE_agent-fixture").to_string()
}

fn task(id: &str, tool: TaskTool) -> Task {
    Task {
        id: id.to_string(),
        title: "a task".to_string(),
        question: "a question".to_string(),
        gap_ids: vec!["G1".to_string()],
        depends_on: Vec::new(),
        method: "a method".to_string(),
        tool,
        arguments: Json::object(vec![]),
        expected_evidence: "evidence".to_string(),
        success_criteria: "success".to_string(),
        estimated_cost: "one run".to_string(),
        risk: "low".to_string(),
    }
}

fn instrument(archive: &str) -> ValidatedTask {
    ValidatedTask {
        task: task("T1", TaskTool::InstrumentRun),
        operation: Operation::Instrument {
            model: "model.ap".to_string(),
            budget: 80,
            archive: archive.to_string(),
        },
    }
}

fn exec(argv: Vec<String>, timeout_ms: u64) -> ValidatedTask {
    ValidatedTask {
        task: task("T1", TaskTool::Exec),
        operation: Operation::Exec { argv, timeout_ms },
    }
}

#[test]
fn an_instrument_run_records_the_archive_it_wrote() {
    let (_temp, sandbox) = sandbox();
    let task = instrument("archives/T1");

    let result = execute("R1", &task, &sandbox, Path::new(&fixture()), "workspace");

    assert_eq!(result.status, "completed", "{result:?}");
    assert_eq!(result.exit_code, Some(0));
    assert_eq!(result.artifacts.len(), 1);
    let artifact = &result.artifacts[0];
    assert_eq!(artifact.kind, "instrument_archive");
    assert_eq!(artifact.path, "workspace/archives/T1");
    assert_eq!(artifact.files, 2);
    let manifest =
        std::fs::read(sandbox.root().join("archives/T1/manifest.json")).expect("manifest");
    assert_eq!(artifact.sha256, sha256_hex(&manifest));
}

#[test]
fn a_nonzero_exit_is_recorded_as_an_observation() {
    let (_temp, sandbox) = sandbox();
    let task = exec(vec![fixture(), "fail".to_string(), "7".to_string()], 5_000);

    let result = execute("R1", &task, &sandbox, Path::new(&fixture()), "workspace");

    assert_eq!(result.status, "completed", "{result:?}");
    assert_eq!(result.exit_code, Some(7));
}

#[test]
fn a_missing_instrument_is_a_launch_failure_with_a_reason() {
    let (_temp, sandbox) = sandbox();
    let task = instrument("archives/T1");

    let result = execute(
        "R1",
        &task,
        &sandbox,
        Path::new("definitely-not-the-instrument"),
        "workspace",
    );

    assert_eq!(result.status, "launch_failed", "{result:?}");
    assert!(result.note.contains("cannot start"), "{}", result.note);
    assert!(result.artifacts.is_empty());
}

#[test]
fn a_timeout_is_reported_as_a_timeout() {
    let (_temp, sandbox) = sandbox();
    let task = exec(
        vec![fixture(), "sleep".to_string(), "30000".to_string()],
        50,
    );

    let result = execute("R1", &task, &sandbox, Path::new(&fixture()), "workspace");

    assert_eq!(result.status, "timeout", "{result:?}");
    assert_eq!(result.exit_code, None);
    assert!(
        result.duration_ms < 10_000,
        "the timeout took {} ms",
        result.duration_ms
    );
}

#[test]
fn the_result_serialises_the_stream_identity_for_the_record() {
    let (_temp, sandbox) = sandbox();
    let task = exec(
        vec![fixture(), "echo".to_string(), "hello".to_string()],
        5_000,
    );
    let result = execute("R1", &task, &sandbox, Path::new(&fixture()), "workspace");

    let json = result.to_json();
    let Json::Obj(fields) = json else {
        panic!("the result must be an object");
    };
    let find = |name: &str| {
        fields
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value)
    };
    assert_eq!(find("id").and_then(Json::as_str), Some("R1"));
    assert_eq!(find("status").and_then(Json::as_str), Some("completed"));
    let stdout = find("stdout")
        .and_then(|value| match value {
            Json::Obj(fields) => Some(fields),
            _ => None,
        })
        .expect("stdout object");
    let hash = stdout
        .iter()
        .find(|(key, _)| key == "sha256")
        .and_then(|(_, value)| value.as_str())
        .expect("sha256");
    assert_eq!(hash.len(), 64);
}
