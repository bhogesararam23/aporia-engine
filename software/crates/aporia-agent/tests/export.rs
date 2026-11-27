//! The export lifecycle through the real engine: the patch is prepared in the record, the write
//! to the user's destination happens only after a recorded approval, and a refusal keeps the
//! investigation alive.

use aporia_agent::engine::{AutoApprove, Engine, EngineConfig, Outcome, ScriptedDecisions, Wait};
use aporia_agent::provider::scripted::ScriptedProvider;
use aporia_agent::provider::{ModelResponse, ToolCall};
use aporia_agent::scope::Decision;
use aporia_agent::tools::{Registry, control, edit, export, files};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Temp(PathBuf);

impl Temp {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "aporia-export-{tag}-{}-{}",
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

fn call(id: &str, name: &str, arguments: &str) -> ToolCall {
    ToolCall {
        id: id.to_string(),
        name: name.to_string(),
        arguments: arguments.to_string(),
    }
}

fn registry() -> Registry {
    let mut registry = Registry::new();
    control::register(&mut registry).expect("control tools");
    files::register(&mut registry).expect("file tools");
    edit::register(&mut registry).expect("edit tools");
    export::register(&mut registry).expect("export tool");
    registry
}

fn project(tag: &str) -> Temp {
    let source = Temp::new(tag);
    fs::write(source.path().join("rule.txt"), b"original line\n").expect("file");
    source
}

fn plan_call() -> ToolCall {
    call(
        "call-plan",
        "propose_plan",
        r#"{"objective":"improve the rule","gaps":[{"id":"G1","title":"the rule is wrong","priority":"high","why":"it says original","condition":"no test checks it","evidence":["F1"]}],"tasks":[{"id":"T1","title":"fix the rule","question":"does the fix hold?","gap_ids":["G1"],"depends_on":[],"method":"edit and export","expected_evidence":"a patch","success_criteria":"the patch applies","estimated_cost":"a second","risk":"none"}],"scope":{"allow_exec":[],"allow_write":true,"max_turns":8,"max_tool_calls":16,"timeout_ms":60000},"success_criteria":"the change is exportable"}"#,
    )
}

fn responses(destination: &Path) -> Vec<ModelResponse> {
    vec![
        ModelResponse::calls(vec![plan_call()]),
        ModelResponse::calls(vec![call(
            "call-write",
            "write_file",
            r#"{"path":"rule.txt","content":"fixed line\n"}"#,
        )]),
        ModelResponse::calls(vec![call(
            "call-export",
            "export_changeset",
            &format!(
                "{{\"destination\":{}}}",
                aporia_store::Json::text(destination.display().to_string()).to_compact()
            ),
        )]),
        ModelResponse::calls(vec![call(
            "call-finish",
            "finish",
            r#"{"summary":"done","next_steps":[],"unresolved":[]}"#,
        )]),
    ]
}

fn config(source: &Temp, workdir: &Temp, responses: Vec<ModelResponse>) -> EngineConfig {
    EngineConfig {
        source: source.path().to_path_buf(),
        objective: "improve the rule".to_string(),
        provider: Box::new(ScriptedProvider::from_responses(responses)),
        allow_exec: Vec::new(),
        allow_write: true,
        max_turns: 16,
        max_tool_calls: 32,
        total_timeout_ms: 120_000,
        workdir: workdir.path().to_path_buf(),
        instrument: std::env::current_exe().expect("exe"),
    }
}

fn record_dir(workdir: &Temp) -> PathBuf {
    let mut records: Vec<PathBuf> = fs::read_dir(workdir.path())
        .expect("records")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    records.sort();
    assert_eq!(records.len(), 1);
    records.remove(0)
}

#[test]
fn an_approved_export_writes_the_patch_and_journals_it() {
    let source = project("src1");
    let workdir = Temp::new("work1");
    let out = Temp::new("out1");
    let destination = out.path().join("patch.diff");
    let mut engine = Engine::start(
        config(&source, &workdir, responses(&destination)),
        Box::new(AutoApprove),
        registry(),
    )
    .expect("engine");
    let outcome = match engine.resume().expect("resume") {
        Wait::Done(outcome) => outcome,
        Wait::Approval(_) => panic!("auto controller decides"),
    };
    assert_eq!(outcome, Outcome::Completed);

    let patch = fs::read_to_string(&destination).expect("the exported patch");
    assert!(patch.contains("--- a/rule.txt"), "{patch}");
    assert!(patch.contains("+++ b/rule.txt"), "{patch}");
    assert!(patch.contains("-original line"), "{patch}");
    assert!(patch.contains("+fixed line"), "{patch}");
    // The original project was never written.
    assert_eq!(
        fs::read_to_string(source.path().join("rule.txt")).expect("original"),
        "original line\n"
    );
    let root = record_dir(&workdir);
    let journal = fs::read_to_string(root.join("journal.jsonl")).expect("journal");
    assert!(
        journal.contains("\"kind\":\"changeset.exported\""),
        "{journal}"
    );
    assert!(journal.contains("\"by\":\"approval\""), "{journal}");
    assert!(
        journal.contains("\"kind\":\"approval.decided\"") && journal.contains("changeset_export"),
        "{journal}"
    );
    // The prepared patch is also kept in the record.
    assert!(root.join("changeset.patch").is_file());
}

#[test]
fn a_refused_export_keeps_the_patch_in_the_record_and_the_session_alive() {
    let source = project("src2");
    let workdir = Temp::new("work2");
    let out = Temp::new("out2");
    let destination = out.path().join("patch.diff");
    let mut engine = Engine::start(
        config(&source, &workdir, responses(&destination)),
        Box::new(ScriptedDecisions::new(vec![
            Decision::Approve,
            Decision::Refuse,
        ])),
        registry(),
    )
    .expect("engine");
    let outcome = match engine.resume().expect("resume") {
        Wait::Done(outcome) => outcome,
        Wait::Approval(_) => panic!("the scripted controller decides"),
    };
    assert_eq!(
        outcome,
        Outcome::Completed,
        "a refused export must not end the investigation"
    );
    assert!(
        !destination.exists(),
        "the destination was written without approval"
    );
    let root = record_dir(&workdir);
    assert!(root.join("changeset.patch").is_file(), "the patch is kept");
    let journal = fs::read_to_string(root.join("journal.jsonl")).expect("journal");
    assert!(
        !journal.contains("\"kind\":\"changeset.exported\""),
        "{journal}"
    );
    assert!(journal.contains("was refused"), "{journal}");
}

#[test]
fn a_destination_inside_the_source_is_refused() {
    let source = project("src3");
    let workdir = Temp::new("work3");
    let inside = source.path().join("patch.diff");
    let mut engine = Engine::start(
        config(&source, &workdir, responses(&inside)),
        Box::new(AutoApprove),
        registry(),
    )
    .expect("engine");
    let outcome = match engine.resume().expect("resume") {
        Wait::Done(outcome) => outcome,
        Wait::Approval(_) => panic!("auto controller decides"),
    };
    assert_eq!(outcome, Outcome::Completed);
    assert!(!inside.exists(), "the source project was written");
    let root = record_dir(&workdir);
    let journal = fs::read_to_string(root.join("journal.jsonl")).expect("journal");
    assert!(journal.contains("inside the source project"), "{journal}");
    assert!(
        !journal.contains("\"kind\":\"changeset.exported\""),
        "{journal}"
    );
}

#[test]
fn exporting_without_changes_is_a_not_found_failure() {
    let source = project("src4");
    let workdir = Temp::new("work4");
    let out = Temp::new("out4");
    let destination = out.path().join("patch.diff");
    let responses = vec![
        ModelResponse::calls(vec![plan_call()]),
        ModelResponse::calls(vec![call(
            "call-export",
            "export_changeset",
            &format!(
                "{{\"destination\":{}}}",
                aporia_store::Json::text(destination.display().to_string()).to_compact()
            ),
        )]),
        ModelResponse::calls(vec![call(
            "call-finish",
            "finish",
            r#"{"summary":"nothing to export","next_steps":[],"unresolved":[]}"#,
        )]),
    ];
    let mut engine = Engine::start(
        config(&source, &workdir, responses),
        Box::new(AutoApprove),
        registry(),
    )
    .expect("engine");
    let outcome = match engine.resume().expect("resume") {
        Wait::Done(outcome) => outcome,
        Wait::Approval(_) => panic!("auto controller decides"),
    };
    assert_eq!(outcome, Outcome::Completed);
    assert!(!destination.exists());
    let root = record_dir(&workdir);
    let journal = fs::read_to_string(root.join("journal.jsonl")).expect("journal");
    assert!(journal.contains("nothing to export"), "{journal}");
}
