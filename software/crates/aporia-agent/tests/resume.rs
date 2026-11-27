//! Crash-safe resume, exercised against the bytes a dead process actually leaves behind.
//!
//! A crash is simulated two ways: dropping a live engine without finishing it (the journal and
//! workspace stay exactly as the last step left them), and hand-appending the events a process
//! would have written between `tool.started` and `tool.completed` before dying. The assertions
//! are about what the resumed session does: completed work is never run again, interrupted work
//! is reported as interrupted, never-started work runs once, and a broken record is refused.

use aporia_agent::Journal;
use aporia_agent::engine::{AutoApprove, Engine, EngineConfig, Outcome, Wait};
use aporia_agent::provider::scripted::ScriptedProvider;
use aporia_agent::provider::{ModelResponse, ToolCall};
use aporia_agent::resume::{ResumeConfig, resume_session};
use aporia_agent::scope::Decision;
use aporia_agent::tools::{Registry, command, control, edit, files};
use aporia_store::Json;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Temp(PathBuf);

impl Temp {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "aporia-resume-{tag}-{}-{}",
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
    command::register(&mut registry).expect("command tool");
    edit::register(&mut registry).expect("edit tools");
    registry
}

fn project(tag: &str) -> Temp {
    let source = Temp::new(tag);
    fs::write(
        source.path().join("notes.md"),
        b"# notes\nthe rule may be biased\n",
    )
    .expect("file");
    source
}

fn config(
    source: &Temp,
    workdir: &Temp,
    responses: Vec<ModelResponse>,
    write: bool,
) -> EngineConfig {
    EngineConfig {
        source: source.path().to_path_buf(),
        objective: "find out whether the rule is biased".to_string(),
        provider: Box::new(ScriptedProvider::from_responses(responses)),
        allow_exec: Vec::new(),
        allow_write: write,
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
        .collect();
    records.sort();
    assert_eq!(records.len(), 1, "one investigation directory");
    records.remove(0)
}

fn resume_with(root: &Path, responses: Vec<ModelResponse>) -> Engine {
    resume_session(
        ResumeConfig {
            record: root.to_path_buf(),
            provider: Box::new(ScriptedProvider::from_responses(responses)),
            instrument: std::env::current_exe().expect("exe"),
        },
        Box::new(AutoApprove),
        registry(),
    )
    .expect("resume")
}

fn run_to_end(engine: &mut Engine) -> Outcome {
    match engine.resume().expect("resume loop") {
        Wait::Done(outcome) => outcome,
        Wait::Approval(_) => panic!("the auto controller should have decided"),
    }
}

fn finish_call() -> ToolCall {
    call(
        "call-finish",
        "finish",
        r#"{"summary":"done","next_steps":[],"unresolved":[]}"#,
    )
}

/// A controller that never decides: the approval stays pending.
struct Pause;

impl aporia_agent::engine::Controller for Pause {
    fn decide(&mut self, _request: &aporia_agent::scope::ApprovalRequest) -> Option<Decision> {
        None
    }
}

fn journal_text(root: &Path) -> String {
    fs::read_to_string(root.join("journal.jsonl")).expect("journal")
}

fn count_started(root: &Path, call_id: &str) -> usize {
    journal_text(root)
        .lines()
        .filter(|line| line.contains("\"kind\":\"tool.started\"") && line.contains(call_id))
        .count()
}

#[test]
fn a_dropped_session_resumes_and_completed_calls_are_not_run_again() {
    let source = project("src1");
    let workdir = Temp::new("work1");
    let responses = vec![
        ModelResponse::calls(vec![call(
            "call-read",
            "read_file",
            r#"{"path":"notes.md"}"#,
        )]),
        ModelResponse::calls(vec![finish_call()]),
    ];
    let mut engine = Engine::start(
        config(&source, &workdir, responses, false),
        Box::new(AutoApprove),
        registry(),
    )
    .expect("engine");
    // One turn runs the read; then the process dies without finishing.
    assert!(matches!(
        engine.step().expect("step"),
        aporia_agent::engine::Step::Turn
    ));
    let root = record_dir(&workdir);
    assert_eq!(count_started(&root, "call-read"), 1);
    drop(engine);

    let mut resumed = resume_with(&root, vec![ModelResponse::calls(vec![finish_call()])]);
    let outcome = run_to_end(&mut resumed);
    assert_eq!(outcome, Outcome::Completed);
    assert_eq!(
        count_started(&root, "call-read"),
        1,
        "the completed read was run again on resume"
    );
    let journal = journal_text(&root);
    assert!(
        journal.contains("\"kind\":\"session.resumed\""),
        "{journal}"
    );
    // The evidence the first process recorded is still citable.
    assert!(resumed.investigation().has_evidence("I1"));
}

#[test]
fn a_started_but_unfinished_call_is_reported_interrupted_not_rerun() {
    let source = project("src2");
    let workdir = Temp::new("work2");
    // A real start writes session.started, inspection.json and sandbox.json.
    let engine = Engine::start(
        config(&source, &workdir, Vec::new(), false),
        Box::new(AutoApprove),
        registry(),
    )
    .expect("engine");
    let root = record_dir(&workdir);
    drop(engine);

    // The bytes a process writes before dying mid-call: the model asked for a read, the host
    // journalled the request and the start, and no completion ever arrived.
    let mut journal = Journal::open(&root.join("journal.jsonl")).expect("reopen");
    let arguments = r#"{"path":"notes.md"}"#;
    journal
        .append(
            "model.request",
            Json::object(vec![
                ("turn", Json::count(1)),
                ("messages", Json::count(2)),
                ("tools", Json::Arr(vec![Json::text("read_file")])),
            ]),
        )
        .expect("append");
    journal
        .append(
            "model.response",
            Json::object(vec![
                ("content", Json::Null),
                (
                    "tool_calls",
                    Json::Arr(vec![Json::object(vec![
                        ("id", Json::text("call-mid")),
                        ("name", Json::text("read_file")),
                        ("arguments", Json::text(arguments)),
                    ])]),
                ),
            ]),
        )
        .expect("append");
    journal
        .append(
            "tool.requested",
            Json::object(vec![
                ("call", Json::text("call-mid")),
                ("name", Json::text("read_file")),
                ("arguments", Json::text(arguments)),
            ]),
        )
        .expect("append");
    journal
        .append(
            "tool.started",
            Json::object(vec![
                ("call", Json::text("call-mid")),
                ("name", Json::text("read_file")),
            ]),
        )
        .expect("append");
    drop(journal);

    let mut resumed = resume_with(&root, vec![ModelResponse::calls(vec![finish_call()])]);
    let journal = journal_text(&root);
    assert!(
        journal.contains("\"interrupted\":[\"call-mid\"]"),
        "the resume did not report the interrupted call: {journal}"
    );
    let outcome = run_to_end(&mut resumed);
    assert_eq!(outcome, Outcome::Completed);
    assert_eq!(
        count_started(&root, "call-mid"),
        1,
        "the interrupted call was run again"
    );
    let journal = journal_text(&root);
    assert!(
        journal.contains("error[interrupted]"),
        "the model was not told the call was interrupted: {journal}"
    );
    // The transcript answers every call, including the interrupted one.
    for message in resumed.transcript() {
        for asked in &message.tool_calls {
            assert!(
                resumed.transcript().iter().any(|other| {
                    other.role == aporia_agent::provider::Role::Tool
                        && other.tool_call_id.as_deref() == Some(asked.id.as_str())
                }),
                "call {} has no answer",
                asked.id
            );
        }
    }
}

#[test]
fn a_requested_but_never_started_call_runs_once_on_resume() {
    let source = project("src3");
    let workdir = Temp::new("work3");
    let engine = Engine::start(
        config(&source, &workdir, Vec::new(), false),
        Box::new(AutoApprove),
        registry(),
    )
    .expect("engine");
    let root = record_dir(&workdir);
    drop(engine);

    // The process died after journalling the request but before starting it: the call never ran.
    let mut journal = Journal::open(&root.join("journal.jsonl")).expect("reopen");
    let arguments = r#"{"path":"notes.md"}"#;
    journal
        .append(
            "model.request",
            Json::object(vec![
                ("turn", Json::count(1)),
                ("messages", Json::count(2)),
                ("tools", Json::Arr(vec![Json::text("read_file")])),
            ]),
        )
        .expect("append");
    journal
        .append(
            "model.response",
            Json::object(vec![
                ("content", Json::Null),
                (
                    "tool_calls",
                    Json::Arr(vec![Json::object(vec![
                        ("id", Json::text("call-never")),
                        ("name", Json::text("read_file")),
                        ("arguments", Json::text(arguments)),
                    ])]),
                ),
            ]),
        )
        .expect("append");
    journal
        .append(
            "tool.requested",
            Json::object(vec![
                ("call", Json::text("call-never")),
                ("name", Json::text("read_file")),
                ("arguments", Json::text(arguments)),
            ]),
        )
        .expect("append");
    drop(journal);

    let mut resumed = resume_with(&root, vec![ModelResponse::calls(vec![finish_call()])]);
    let outcome = run_to_end(&mut resumed);
    assert_eq!(outcome, Outcome::Completed);
    assert_eq!(
        count_started(&root, "call-never"),
        1,
        "the never-started call did not run exactly once"
    );
    let journal = journal_text(&root);
    assert!(
        journal.contains("the rule may be biased"),
        "the read's real output is not in the record: {journal}"
    );
}

#[test]
fn findings_experiments_changes_and_scope_survive_a_resume() {
    let source = project("src4");
    let workdir = Temp::new("work4");
    let plan = call(
        "call-plan",
        "propose_plan",
        r#"{"objective":"find out whether the rule is biased","gaps":[{"id":"G1","title":"no measurement of the bias","priority":"high","why":"the bias is unmeasured","condition":"an error estimate exists but no study","evidence":["F1"]}],"tasks":[{"id":"T1","title":"read the notes","question":"what does the project say?","gap_ids":["G1"],"depends_on":[],"method":"read the file","expected_evidence":"the notes","success_criteria":"the notes are read","estimated_cost":"a second","risk":"none"}],"scope":{"allow_exec":[],"allow_write":true,"max_turns":8,"max_tool_calls":16,"timeout_ms":60000},"success_criteria":"the bias is characterised"}"#,
    );
    let responses = vec![
        ModelResponse::calls(vec![plan]),
        ModelResponse::calls(vec![call(
            "call-write",
            "write_file",
            r#"{"path":"extra.txt","content":"a new file\n"}"#,
        )]),
        ModelResponse::calls(vec![call(
            "call-finding",
            "record_finding",
            r#"{"kind":"observation","severity":"high","text":"the notes mention a possible bias","evidence":["F1"],"uncertainty":"the magnitude is unknown"}"#,
        )]),
        ModelResponse::calls(vec![call(
            "call-experiment",
            "propose_experiment",
            r#"{"title":"measure the bias","question":"how large is it?","reason":"never measured","evidence":["I2"],"priority":"high","methodology":"run the sweep","variables":[],"dependencies":[],"required_data":[],"expected_outcomes":"a number","metrics":["bias"],"estimated_cost":"a minute","success_criteria":"the bias is measured","risks":"none"}"#,
        )]),
        ModelResponse::calls(vec![finish_call()]),
    ];
    let mut engine = Engine::start(
        config(&source, &workdir, responses, true),
        Box::new(AutoApprove),
        registry(),
    )
    .expect("engine");
    // Four turns: plan (approved at the start of turn 2), write, finding, experiment.
    for _ in 0..4 {
        assert!(matches!(
            engine.step().expect("step"),
            aporia_agent::engine::Step::Turn
        ));
    }
    let root = record_dir(&workdir);
    drop(engine);

    let mut resumed = resume_with(&root, vec![ModelResponse::calls(vec![finish_call()])]);
    let investigation = resumed.investigation();
    assert_eq!(investigation.approved_version, Some(1));
    assert!(investigation.scope.as_ref().expect("scope").allow_write);
    assert_eq!(investigation.findings.len(), 1);
    assert_eq!(investigation.experiments.len(), 1);
    assert_eq!(investigation.changes.len(), 1);
    assert_eq!(investigation.changes[0].path, "extra.txt");
    assert!(investigation.has_evidence("I2"));
    // The workspace edit the first process made is still there.
    assert_eq!(
        fs::read_to_string(root.join("workspace/extra.txt")).expect("file"),
        "a new file\n"
    );
    let outcome = run_to_end(&mut resumed);
    assert_eq!(outcome, Outcome::Completed);
}

#[test]
fn a_pending_approval_survives_a_resume() {
    let source = project("src5");
    let workdir = Temp::new("work5");
    // A controller that pauses leaves the approval genuinely pending on disk.
    let plan = call(
        "call-plan",
        "propose_plan",
        r#"{"objective":"find out whether the rule is biased","gaps":[{"id":"G1","title":"no measurement of the bias","priority":"high","why":"the bias is unmeasured","condition":"an error estimate exists but no study","evidence":["F1"]}],"tasks":[{"id":"T1","title":"read the notes","question":"what does the project say?","gap_ids":["G1"],"depends_on":[],"method":"read the file","expected_evidence":"the notes","success_criteria":"the notes are read","estimated_cost":"a second","risk":"none"}],"scope":{"allow_exec":[],"allow_write":false,"max_turns":8,"max_tool_calls":16,"timeout_ms":60000},"success_criteria":"the bias is characterised"}"#,
    );
    let mut engine = Engine::start(
        config(
            &source,
            &workdir,
            vec![ModelResponse::calls(vec![plan])],
            false,
        ),
        Box::new(Pause),
        registry(),
    )
    .expect("engine");
    match engine.resume().expect("resume") {
        Wait::Approval(_) => {}
        other @ Wait::Done(_) => panic!("expected the approval, got {other:?}"),
    }
    let root = record_dir(&workdir);
    drop(engine);

    let mut resumed = resume_with(&root, vec![ModelResponse::calls(vec![finish_call()])]);
    assert!(
        resumed.pending_approval().is_some(),
        "the pending approval did not survive the resume"
    );
    resumed.decide(Decision::Approve).expect("decide");
    let outcome = run_to_end(&mut resumed);
    assert_eq!(outcome, Outcome::Completed);
    assert_eq!(resumed.investigation().approved_version, Some(1));
}

#[test]
fn a_broken_chain_is_a_named_refusal() {
    let source = project("src6");
    let workdir = Temp::new("work6");
    let mut engine = Engine::start(
        config(
            &source,
            &workdir,
            vec![ModelResponse::calls(vec![call(
                "call-read",
                "read_file",
                r#"{"path":"notes.md"}"#,
            )])],
            false,
        ),
        Box::new(AutoApprove),
        registry(),
    )
    .expect("engine");
    engine.step().expect("step");
    let root = record_dir(&workdir);
    drop(engine);

    // Flip a byte in the middle of the journal: the chain must notice.
    let path = root.join("journal.jsonl");
    let mut text = fs::read_to_string(&path).expect("journal");
    let middle = text.len() / 2;
    let byte = text.as_bytes()[middle];
    text.replace_range(middle..=middle, if byte == b'x' { "y" } else { "x" });
    fs::write(&path, text).expect("corrupt");

    let error = resume_session(
        ResumeConfig {
            record: root.clone(),
            provider: Box::new(ScriptedProvider::from_responses(Vec::new())),
            instrument: std::env::current_exe().expect("exe"),
        },
        Box::new(AutoApprove),
        registry(),
    )
    .expect_err("a broken chain must refuse");
    let message = error.to_string();
    assert!(
        message.contains("chain") || message.contains("not an event"),
        "the refusal did not name the broken record: {message}"
    );
}

#[test]
fn a_finished_record_is_history_not_work_to_resume() {
    let source = project("src7");
    let workdir = Temp::new("work7");
    let mut engine = Engine::start(
        config(
            &source,
            &workdir,
            vec![ModelResponse::calls(vec![finish_call()])],
            false,
        ),
        Box::new(AutoApprove),
        registry(),
    )
    .expect("engine");
    run_to_end(&mut engine);
    let root = record_dir(&workdir);
    let error = resume_session(
        ResumeConfig {
            record: root.clone(),
            provider: Box::new(ScriptedProvider::from_responses(Vec::new())),
            instrument: std::env::current_exe().expect("exe"),
        },
        Box::new(AutoApprove),
        registry(),
    )
    .expect_err("finished records must not resume");
    assert!(error.to_string().contains("finished"), "{error}");
}

#[test]
fn a_corrupt_checkpoint_is_rebuilt_from_the_journal() {
    let source = project("src8");
    let workdir = Temp::new("work8");
    let mut engine = Engine::start(
        config(
            &source,
            &workdir,
            vec![ModelResponse::calls(vec![call(
                "call-read",
                "read_file",
                r#"{"path":"notes.md"}"#,
            )])],
            false,
        ),
        Box::new(AutoApprove),
        registry(),
    )
    .expect("engine");
    engine.step().expect("step");
    let root = record_dir(&workdir);
    drop(engine);

    fs::write(root.join("session.json"), b"not json at all").expect("corrupt checkpoint");
    let mut resumed = resume_with(&root, vec![ModelResponse::calls(vec![finish_call()])]);
    let outcome = run_to_end(&mut resumed);
    assert_eq!(outcome, Outcome::Completed);
    let state = aporia_agent::read_checkpoint(&root)
        .expect("read")
        .expect("the checkpoint was rebuilt");
    assert_eq!(state.status, aporia_agent::SessionStatus::Completed);
}

#[test]
fn the_users_words_survive_a_resume() {
    let source = project("src9");
    let workdir = Temp::new("work9");
    let mut engine = Engine::start(
        config(&source, &workdir, Vec::new(), false),
        Box::new(AutoApprove),
        registry(),
    )
    .expect("engine");
    engine
        .say("please also check the boundary case")
        .expect("say");
    let root = record_dir(&workdir);
    drop(engine);

    let resumed = resume_with(&root, vec![ModelResponse::calls(vec![finish_call()])]);
    assert!(
        resumed.transcript().iter().any(|message| message
            .content
            .contains("please also check the boundary case")),
        "the user's words were lost across the resume"
    );
}
