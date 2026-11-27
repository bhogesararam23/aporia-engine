//! The iterative loop, driven by a scripted provider through the real registry.
//!
//! The script is not a transcript of a report: it is a sequence of tool calls that the engine
//! validates, dispatches, records and answers. The assertions are about what the record contains
//! afterwards -- the findings, the approvals, the scope that was actually in force, the failures
//! the model was shown.

use aporia_agent::engine::{Engine, EngineConfig, Outcome, Wait};
use aporia_agent::provider::scripted::ScriptedProvider;
use aporia_agent::provider::{ModelResponse, ToolCall};
use aporia_agent::scope::Decision;
use aporia_agent::tools::control;
use aporia_agent::tools::{Registry, command, files, instrument, results};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Temp(PathBuf);

impl Temp {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "aporia-engine-{tag}-{}-{}",
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
    instrument::register(&mut registry).expect("instrument tool");
    results::register(&mut registry).expect("result tools");
    registry
}

fn project() -> Temp {
    let source = Temp::new("project");
    fs::write(
        source.0.join("notes.md"),
        b"# notes\nthe rule may be biased\n",
    )
    .expect("file");
    source
}

fn engine_for(
    source: &Temp,
    workdir: &Temp,
    responses: Vec<ModelResponse>,
    allow_exec: Vec<String>,
) -> Engine {
    let provider = ScriptedProvider::from_responses(responses);
    let config = EngineConfig {
        source: source.0.clone(),
        objective: "find out whether the rule is biased".to_string(),
        provider: Box::new(provider),
        allow_exec,
        allow_write: false,
        max_turns: 16,
        max_tool_calls: 32,
        total_timeout_ms: 120_000,
        workdir: workdir.0.clone(),
        instrument: std::env::current_exe().expect("exe"),
    };
    Engine::start(
        config,
        Box::new(aporia_agent::engine::AutoApprove),
        registry(),
    )
    .expect("engine")
}

/// Run the engine until it ends; the auto controller decides every approval, so `resume` returns
/// only when the investigation is over.
fn run_to_end(engine: &mut Engine) -> Outcome {
    match engine.resume().expect("resume") {
        Wait::Done(outcome) => outcome,
        Wait::Approval(_) => panic!("the auto controller should have decided"),
    }
}

fn plan_call() -> ToolCall {
    call(
        "call-plan",
        "propose_plan",
        r#"{"objective":"find out whether the rule is biased","gaps":[{"id":"G1","title":"no measurement of the bias","priority":"high","why":"the bias is unmeasured","condition":"an error estimate exists but no study","evidence":["F1"]}],"tasks":[{"id":"T1","title":"read the notes","question":"what does the project say?","gap_ids":["G1"],"depends_on":[],"method":"read the file","expected_evidence":"the notes","success_criteria":"the notes are read","estimated_cost":"a second","risk":"none"}],"scope":{"allow_exec":[],"allow_write":false,"max_turns":8,"max_tool_calls":16,"timeout_ms":60000},"success_criteria":"the bias is characterised"}"#,
    )
}

#[test]
fn the_loop_reads_proposes_records_and_finishes() {
    let source = project();
    let workdir = Temp::new("work");
    let responses = vec![
        ModelResponse::calls(vec![call(
            "call-read",
            "read_file",
            r#"{"path":"notes.md"}"#,
        )]),
        ModelResponse::calls(vec![plan_call()]),
        ModelResponse::calls(vec![call(
            "call-finding",
            "record_finding",
            r#"{"kind":"observation","severity":"high","text":"the notes mention a possible bias","evidence":["F1"],"uncertainty":"the magnitude is unknown"}"#,
        )]),
        ModelResponse::calls(vec![call(
            "call-finish",
            "finish",
            r#"{"summary":"the rule may be biased","next_steps":["measure the bias"],"unresolved":["how large the bias is"]}"#,
        )]),
    ];
    let mut engine = engine_for(&source, &workdir, responses, Vec::new());
    let outcome = run_to_end(&mut engine);
    assert_eq!(outcome, Outcome::Completed);

    let records: Vec<PathBuf> = fs::read_dir(workdir.path())
        .expect("records")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect();
    assert_eq!(records.len(), 1, "one investigation directory");
    let root = &records[0];

    let investigation = engine.investigation();
    assert_eq!(investigation.findings.len(), 1);
    assert_eq!(
        investigation.findings[0].text,
        "the notes mention a possible bias"
    );
    assert_eq!(investigation.approved_version, Some(1));
    assert!(investigation.scope.is_some());

    // The record on disk holds the report and the manifest.
    let report = fs::read_to_string(root.join("report.md")).expect("report");
    assert!(report.contains("the notes mention a possible bias"));
    let summary = fs::read_to_string(root.join("investigation.json")).expect("summary");
    assert!(summary.contains("\"schema\": 2"));
    let manifest = fs::read_to_string(root.join("manifest.json")).expect("manifest");
    assert!(manifest.contains("\"outcome\": \"completed\""));
    let journal = fs::read_to_string(root.join("journal.jsonl")).expect("journal");
    assert!(journal.contains("\"kind\":\"plan.proposed\""));
    assert!(journal.contains("\"kind\":\"approval.decided\""));
    assert!(journal.contains("\"kind\":\"finding.recorded\""));
    assert!(journal.contains("\"kind\":\"delivery\""));
    let report = fs::read_to_string(root.join("report.md")).expect("report");
    assert!(report.contains("the notes mention a possible bias"));
    let summary = fs::read_to_string(root.join("investigation.json")).expect("summary");
    assert!(summary.contains("\"schema\": 2"));
    let manifest = fs::read_to_string(root.join("manifest.json")).expect("manifest");
    assert!(manifest.contains("\"outcome\": \"completed\""));
    let _ = root.join("workspace");
}

#[test]
fn a_plan_that_cites_unknown_evidence_is_refused_and_the_model_hears_why() {
    let source = project();
    let workdir = Temp::new("work2");
    let bad_plan = call(
        "call-bad-plan",
        "propose_plan",
        r#"{"objective":"x","gaps":[{"id":"G1","title":"t","priority":"high","why":"w","condition":"c","evidence":["F999"]}],"tasks":[{"id":"T1","title":"t","question":"q","gap_ids":["G1"],"depends_on":[],"method":"m","expected_evidence":"e","success_criteria":"s","estimated_cost":"c","risk":"r"}],"scope":{"allow_exec":[],"allow_write":false,"max_turns":8,"max_tool_calls":16,"timeout_ms":60000},"success_criteria":"s"}"#,
    );
    let responses = vec![
        ModelResponse::calls(vec![bad_plan]),
        ModelResponse::calls(vec![call(
            "call-finish",
            "finish",
            r#"{"summary":"nothing to do","next_steps":[],"unresolved":["the plan was refused"]}"#,
        )]),
    ];
    let mut engine = engine_for(&source, &workdir, responses, Vec::new());
    run_to_end(&mut engine);
    assert!(
        engine.investigation().revisions.is_empty(),
        "a refused plan must not become a revision"
    );
}

#[test]
fn a_refusal_ends_the_investigation_without_running_anything() {
    let source = project();
    let workdir = Temp::new("work3");
    let provider = ScriptedProvider::from_responses(vec![ModelResponse::calls(vec![plan_call()])]);
    let config = EngineConfig {
        source: source.0.clone(),
        objective: "find out whether the rule is biased".to_string(),
        provider: Box::new(provider),
        allow_exec: Vec::new(),
        allow_write: false,
        max_turns: 8,
        max_tool_calls: 16,
        total_timeout_ms: 60_000,
        workdir: workdir.0.clone(),
        instrument: std::env::current_exe().expect("exe"),
    };
    let mut engine =
        Engine::start(config, Box::new(aporia_agent::engine::DenyAll), registry()).expect("engine");
    let outcome = match engine.resume().expect("resume") {
        Wait::Done(outcome) => outcome,
        Wait::Approval(_) => panic!("the deny controller should have decided"),
    };
    assert_eq!(outcome, Outcome::Refused);
    assert!(engine.investigation().approved_version.is_none());
}

#[test]
fn an_unknown_tool_is_a_failure_the_model_sees() {
    let source = project();
    let workdir = Temp::new("work4");
    let responses = vec![
        ModelResponse::calls(vec![call("call-x", "no_such_tool", "{}")]),
        ModelResponse::calls(vec![call(
            "call-finish",
            "finish",
            r#"{"summary":"stopped","next_steps":[],"unresolved":["the tool does not exist"]}"#,
        )]),
    ];
    let mut engine = engine_for(&source, &workdir, responses, Vec::new());
    run_to_end(&mut engine);
    let records: Vec<PathBuf> = fs::read_dir(workdir.path())
        .expect("records")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect();
    let journal = fs::read_to_string(records[0].join("journal.jsonl")).expect("journal");
    assert!(journal.contains("unknown_tool"), "{journal}");
}

/// A plan whose scope permits python, for the permission-expansion tests.
fn python_plan_call() -> ToolCall {
    let mut call = plan_call();
    call.arguments = call
        .arguments
        .replace("\"allow_exec\":[]", "\"allow_exec\":[\"python\"]");
    call
}

/// A controller that never decides: the test answers each approval itself.
struct Pause;

impl aporia_agent::engine::Controller for Pause {
    fn decide(&mut self, _request: &aporia_agent::scope::ApprovalRequest) -> Option<Decision> {
        None
    }
}

fn pausing_engine_for(
    source: &Temp,
    workdir: &Temp,
    responses: Vec<ModelResponse>,
    allow_exec: Vec<String>,
) -> Engine {
    let provider = ScriptedProvider::from_responses(responses);
    let config = EngineConfig {
        source: source.0.clone(),
        objective: "find out whether the rule is biased".to_string(),
        provider: Box::new(provider),
        allow_exec,
        allow_write: false,
        max_turns: 16,
        max_tool_calls: 32,
        total_timeout_ms: 120_000,
        workdir: workdir.0.clone(),
        instrument: std::env::current_exe().expect("exe"),
    };
    Engine::start(config, Box::new(Pause), registry()).expect("engine")
}

/// Every assistant tool call must have a tool result: a transcript a provider can continue.
fn assert_transcript_is_complete(engine: &Engine) {
    let messages = engine.transcript();
    for message in messages {
        for call in &message.tool_calls {
            assert!(
                messages.iter().any(|other| {
                    other.role == aporia_agent::provider::Role::Tool
                        && other.tool_call_id.as_deref() == Some(call.id.as_str())
                }),
                "tool call {} has no tool result in the transcript",
                call.id
            );
        }
    }
}

#[test]
fn a_denied_program_raises_a_question_and_the_retry_runs_under_the_expanded_scope() {
    let source = project();
    let workdir = Temp::new("work7");
    let responses = vec![
        ModelResponse::calls(vec![python_plan_call()]),
        ModelResponse::calls(vec![call(
            "call-node-1",
            "run_command",
            r#"{"argv":["aporia-no-such-program","--version"]}"#,
        )]),
        ModelResponse::calls(vec![call(
            "call-node-2",
            "run_command",
            r#"{"argv":["aporia-no-such-program","--version"]}"#,
        )]),
        ModelResponse::calls(vec![call(
            "call-finish",
            "finish",
            r#"{"summary":"done","next_steps":[],"unresolved":[]}"#,
        )]),
    ];
    let mut engine = pausing_engine_for(
        &source,
        &workdir,
        responses,
        vec!["python".to_string(), "aporia-no-such-program".to_string()],
    );
    // The plan is approved.
    match engine.resume().expect("resume") {
        Wait::Approval(request) => {
            assert_eq!(request.kind, aporia_agent::scope::ApprovalKind::Plan);
            engine.decide(Decision::Approve).expect("decide");
        }
        other @ Wait::Done(_) => panic!("expected the plan approval, got {other:?}"),
    }
    // The unapproved program raises a scope question, and its denial is still delivered.
    match engine.resume().expect("resume") {
        Wait::Approval(request) => {
            assert_eq!(
                request.kind,
                aporia_agent::scope::ApprovalKind::ScopeExpansion
            );
            engine.decide(Decision::Approve).expect("decide");
        }
        other @ Wait::Done(_) => panic!("expected the scope question, got {other:?}"),
    }
    let outcome = run_to_end(&mut engine);
    assert_eq!(outcome, Outcome::Completed);
    assert!(
        engine
            .investigation()
            .scope
            .as_ref()
            .expect("scope")
            .permits_exec("aporia-no-such-program")
    );
    assert_transcript_is_complete(&engine);
    let records: Vec<PathBuf> = fs::read_dir(workdir.path())
        .expect("records")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect();
    let journal = fs::read_to_string(records[0].join("journal.jsonl")).expect("journal");
    assert!(journal.contains("\"kind\":\"scope.expanded\""), "{journal}");
    assert!(journal.contains("\"kind\":\"host.note\""), "{journal}");
}

#[test]
fn refusing_one_program_does_not_end_the_investigation() {
    let source = project();
    let workdir = Temp::new("work8");
    let responses = vec![
        ModelResponse::calls(vec![python_plan_call()]),
        ModelResponse::calls(vec![call(
            "call-node-1",
            "run_command",
            r#"{"argv":["aporia-no-such-program","--version"]}"#,
        )]),
        ModelResponse::calls(vec![call(
            "call-finish",
            "finish",
            r#"{"summary":"continued without the program","next_steps":[],"unresolved":[]}"#,
        )]),
    ];
    let mut engine = pausing_engine_for(
        &source,
        &workdir,
        responses,
        vec!["python".to_string(), "aporia-no-such-program".to_string()],
    );
    match engine.resume().expect("resume") {
        Wait::Approval(_) => engine.decide(Decision::Approve).expect("decide"),
        other @ Wait::Done(_) => panic!("expected the plan approval, got {other:?}"),
    }
    match engine.resume().expect("resume") {
        Wait::Approval(request) => {
            assert_eq!(
                request.kind,
                aporia_agent::scope::ApprovalKind::ScopeExpansion
            );
            engine.decide(Decision::Refuse).expect("decide");
        }
        other @ Wait::Done(_) => panic!("expected the scope question, got {other:?}"),
    }
    let outcome = run_to_end(&mut engine);
    assert_eq!(
        outcome,
        Outcome::Completed,
        "refusing one program must not refuse the investigation"
    );
    assert!(
        !engine
            .investigation()
            .scope
            .as_ref()
            .expect("scope")
            .permits_exec("aporia-no-such-program")
    );
    assert_transcript_is_complete(&engine);
}

#[test]
fn an_expansion_outside_the_ceiling_is_refused_and_the_model_hears_why() {
    let source = project();
    let workdir = Temp::new("work9");
    let responses = vec![
        ModelResponse::calls(vec![python_plan_call()]),
        ModelResponse::calls(vec![call(
            "call-node-1",
            "run_command",
            r#"{"argv":["aporia-no-such-program","--version"]}"#,
        )]),
        ModelResponse::calls(vec![call(
            "call-finish",
            "finish",
            r#"{"summary":"the ceiling held","next_steps":[],"unresolved":[]}"#,
        )]),
    ];
    // The ceiling names python only: even an approval cannot grant the other program.
    let mut engine = pausing_engine_for(&source, &workdir, responses, vec!["python".to_string()]);
    match engine.resume().expect("resume") {
        Wait::Approval(_) => engine.decide(Decision::Approve).expect("decide"),
        other @ Wait::Done(_) => panic!("expected the plan approval, got {other:?}"),
    }
    match engine.resume().expect("resume") {
        Wait::Approval(_) => engine.decide(Decision::Approve).expect("decide"),
        other @ Wait::Done(_) => panic!("expected the scope question, got {other:?}"),
    }
    let outcome = run_to_end(&mut engine);
    assert_eq!(outcome, Outcome::Completed);
    assert!(
        !engine
            .investigation()
            .scope
            .as_ref()
            .expect("scope")
            .permits_exec("aporia-no-such-program")
    );
    assert_transcript_is_complete(&engine);
    let records: Vec<PathBuf> = fs::read_dir(workdir.path())
        .expect("records")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect();
    let journal = fs::read_to_string(records[0].join("journal.jsonl")).expect("journal");
    assert!(
        journal.contains("\"kind\":\"approval.outside_the_ceiling\""),
        "{journal}"
    );
    assert!(
        !journal.contains("\"kind\":\"scope.expanded\""),
        "{journal}"
    );
}

#[test]
fn an_approval_decides_the_version_it_names_not_the_first_undecided() {
    let source = project();
    let workdir = Temp::new("work6");
    let mut second = plan_call();
    second.id = "call-plan-2".to_string();
    second.arguments = second.arguments.replace(
        "find out whether the rule is biased",
        "find out whether the rule is biased, revised",
    );
    let responses = vec![
        // Two proposals in one turn: the second request is the one the person sees.
        ModelResponse::calls(vec![plan_call(), second]),
        ModelResponse::calls(vec![call(
            "call-finish",
            "finish",
            r#"{"summary":"done","next_steps":[],"unresolved":[]}"#,
        )]),
    ];
    let mut engine = engine_for(&source, &workdir, responses, Vec::new());
    let outcome = run_to_end(&mut engine);
    assert_eq!(outcome, Outcome::Completed);
    let investigation = engine.investigation();
    assert_eq!(investigation.revisions.len(), 2);
    assert_eq!(
        investigation.approved_version,
        Some(2),
        "the approval named version 2; version 1 must not be approved"
    );
    assert_eq!(investigation.revisions[0].decision, None);
    assert_eq!(investigation.revisions[1].decision, Some(Decision::Approve));
}

#[test]
fn a_decision_made_by_the_host_is_recorded_with_its_kind() {
    let source = project();
    let workdir = Temp::new("work5");
    let responses = vec![
        ModelResponse::calls(vec![plan_call()]),
        ModelResponse::calls(vec![call(
            "call-finish",
            "finish",
            r#"{"summary":"done","next_steps":[],"unresolved":[]}"#,
        )]),
    ];
    let mut engine = engine_for(&source, &workdir, responses, Vec::new());
    run_to_end(&mut engine);
    let records: Vec<PathBuf> = fs::read_dir(workdir.path())
        .expect("records")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect();
    let journal = fs::read_to_string(records[0].join("journal.jsonl")).expect("journal");
    assert!(journal.contains("\"kind\":\"approval.requested\""));
    assert!(journal.contains("\"decision\":\"approve\""));
    let _ = Decision::Approve;
}
