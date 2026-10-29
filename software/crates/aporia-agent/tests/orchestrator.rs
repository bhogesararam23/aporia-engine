//! The whole state machine, with a scripted model and a real sandbox.
//!
//! These tests are where the honesty rules meet: approval runs nothing, a bad evidence id is
//! refused, a budget is an incomplete record, a dependency failure skips its dependent, and a
//! delivered investigation verifies down to its journal head.

use aporia_agent::inspect::{FactKind, inspect};
use aporia_agent::provider::scripted::ScriptedProvider;
use aporia_agent::{
    Approve, ModelProvider, ModelResponse, Outcome, RunConfig, ToolCall, run, verify_record,
};
use aporia_store::Json;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Temp(PathBuf);

impl Temp {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "aporia-agent-orchestrator-{}-{}-{tag}",
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

struct Harness {
    temp: Temp,
    source: PathBuf,
    workdir: PathBuf,
}

fn harness() -> Harness {
    let temp = Temp::new("work");
    let source = temp.path().join("source");
    let workdir = temp.path().join("workdir");
    std::fs::create_dir_all(&source).expect("source");
    std::fs::write(source.join("model.ap"), b"model x { input a in [0, 1] }\n").expect("model");
    Harness {
        temp,
        source,
        workdir,
    }
}

fn fact_id(source: &Path) -> String {
    let inspection = inspect(source).expect("inspect");
    inspection
        .facts
        .iter()
        .find(|fact| fact.kind == FactKind::Model)
        .expect("model fact")
        .id
        .clone()
}

fn call(id: &str, name: &str, arguments: &Json) -> ToolCall {
    ToolCall {
        id: id.to_string(),
        name: name.to_string(),
        arguments: arguments.to_compact(),
    }
}

fn one_task_plan(fact: &str) -> Json {
    Json::parse(&format!(
        r#"{{
            "gaps": [{{
                "id": "G1", "title": "The rule has not been exercised",
                "priority": "high", "why": "the model file is the only evidence",
                "condition": "the declared rule is only meaningful over a range",
                "evidence": ["{fact}"]
            }}],
            "tasks": [{{
                "id": "T1", "title": "Run the atlas",
                "question": "where does the rule fail?",
                "gap_ids": ["G1"], "depends_on": [],
                "method": "run the instrument on the model",
                "tool": "instrument_run",
                "arguments": {{"model": "model.ap", "budget": 10}},
                "expected_evidence": "an archived atlas",
                "success_criteria": "the instrument exits and writes an archive",
                "estimated_cost": "10 evaluations", "risk": "low"
            }}]
        }}"#
    ))
    .expect("plan json")
}

fn dependency_plan(fact: &str) -> Json {
    Json::parse(&format!(
        r#"{{
            "gaps": [{{
                "id": "G1", "title": "A dependency",
                "priority": "recommended", "why": "there is a tool to run",
                "condition": "the command is available", "evidence": ["{fact}"]
            }}],
            "tasks": [
                {{
                    "id": "T1", "title": "A missing program",
                    "question": "does it run?", "gap_ids": ["G1"], "depends_on": [],
                    "method": "exec", "tool": "exec",
                    "arguments": {{"argv": ["nonexistent-program"]}},
                    "expected_evidence": "an exit", "success_criteria": "the process runs",
                    "estimated_cost": "one process", "risk": "low"
                }},
                {{
                    "id": "T2", "title": "The instrument afterwards",
                    "question": "does the model fail?", "gap_ids": ["G1"],
                    "depends_on": ["T1"], "method": "instrument",
                    "tool": "instrument_run",
                    "arguments": {{"model": "model.ap", "budget": 10}},
                    "expected_evidence": "an archive",
                    "success_criteria": "the instrument exits",
                    "estimated_cost": "10 evaluations", "risk": "low"
                }}
            ]
        }}"#
    ))
    .expect("plan json")
}

fn finding(fact: &str, evidence: &str) -> Json {
    Json::parse(&format!(
        r#"{{"kind":"observation","severity":"high","text":"The run exited and wrote an archive.","evidence":["{fact}","{evidence}"],"uncertainty":"one budget; no repeated run"}}"#
    ))
    .expect("finding json")
}

fn finish() -> Json {
    Json::parse(
        r#"{"summary":"The investigation completed.","next_steps":["Re-run the archive replay."],"unresolved":["No repeated budget was run."]}"#,
    )
    .expect("finish json")
}

fn finish_no_plan() -> Json {
    Json::parse(
        r#"{"summary":"No investigation was warranted.","next_steps":["None."],"unresolved":[]}"#,
    )
    .expect("finish json")
}

fn provider(responses: Vec<ModelResponse>) -> ScriptedProvider {
    ScriptedProvider::from_responses(responses)
}

fn config(harness: &Harness, responses: ScriptedProvider, approve: Approve) -> RunConfig {
    RunConfig {
        source: harness.source.clone(),
        objective: "Find where the declared rule stops holding.".to_string(),
        provider: Box::new(responses),
        approve,
        allow_exec: Vec::new(),
        max_turns: 10,
        max_tool_calls: 20,
        total_timeout_ms: 60_000,
        workdir: harness.workdir.clone(),
        instrument: PathBuf::from(env!("CARGO_BIN_EXE_agent-fixture")),
    }
}

fn all_responses(fact: &str) -> Vec<ModelResponse> {
    vec![
        ModelResponse::calls(vec![call("c1", "propose_plan", &one_task_plan(fact))]),
        ModelResponse::calls(vec![call("c2", "record_finding", &finding(fact, "R1"))]),
        ModelResponse::calls(vec![call("c3", "finish", &finish())]),
    ]
}

#[test]
fn a_complete_investigation_produces_a_verifiable_grounded_record() {
    let harness = harness();
    let fact = fact_id(&harness.source);
    let before = std::fs::read(harness.source.join("model.ap")).expect("model");
    let mut console = Vec::new();

    let report = run(
        config(&harness, provider(all_responses(&fact)), Approve::Flag),
        &mut console,
    )
    .expect("run");

    assert_eq!(report.outcome, Outcome::Completed, "{:?}", report.failure);
    assert_eq!(report.results.len(), 1);
    assert_eq!(report.results[0].status, "completed");
    assert_eq!(report.results[0].artifacts.len(), 1);
    assert_eq!(report.findings.len(), 1);
    assert_eq!(
        report.findings[0].evidence,
        vec![fact.clone(), "R1".to_string()]
    );
    let summary = verify_record(&report.root).expect("record verifies");
    assert_eq!(summary.outcome, "completed");
    let text = std::fs::read_to_string(report.root.join("report.md")).expect("report");
    assert!(text.contains("scripted"), "{text}");
    assert!(text.contains("No model was consulted"), "{text}");
    assert!(text.contains("Finding 1"), "{text}");
    assert!(text.contains("workspace/archives/T1"), "{text}");
    assert!(text.contains("R1"), "{text}");
    let console = String::from_utf8(console).expect("console");
    assert!(console.contains("Proposed plan"), "{console}");
    assert!(console.contains("approved"), "{console}");
    assert_eq!(
        std::fs::read(harness.source.join("model.ap")).expect("model"),
        before,
        "the source file changed"
    );
    assert_eq!(
        std::fs::read_dir(&harness.source).expect("source").count(),
        1,
        "the source tree gained a file"
    );
}

#[test]
fn a_refused_plan_runs_nothing_and_says_so() {
    let harness = harness();
    let fact = fact_id(&harness.source);
    let mut console = Vec::new();

    let report = run(
        config(
            &harness,
            provider(vec![ModelResponse::calls(vec![call(
                "c1",
                "propose_plan",
                &one_task_plan(&fact),
            )])]),
            Approve::Never,
        ),
        &mut console,
    )
    .expect("run");

    assert_eq!(report.outcome, Outcome::Refused);
    assert!(report.results.is_empty());
    assert!(!report.root.join("workspace/archives/T1").exists());
    let text = std::fs::read_to_string(report.root.join("report.md")).expect("report");
    assert!(text.contains("No task ran"), "{text}");
    assert_eq!(
        verify_record(&report.root).expect("verify").outcome,
        "refused"
    );
}

#[test]
fn an_invalid_plan_is_returned_and_a_correction_is_accepted() {
    let harness = harness();
    let fact = fact_id(&harness.source);
    let invalid = Json::parse(
        r#"{"gaps":[{"id":"G1","title":"t","priority":"high","why":"w","condition":"c","evidence":["F999"]}],"tasks":[{"id":"T1","title":"t","question":"q","gap_ids":["G1"],"depends_on":[],"method":"m","tool":"instrument_run","arguments":{"model":"model.ap","budget":10},"expected_evidence":"e","success_criteria":"s","estimated_cost":"c","risk":"r"}]}"#,
    )
    .expect("json");
    let responses = vec![
        ModelResponse::calls(vec![call("c1", "propose_plan", &invalid)]),
        ModelResponse::calls(vec![call("c2", "propose_plan", &one_task_plan(&fact))]),
        ModelResponse::calls(vec![call("c3", "finish", &finish())]),
    ];
    let mut console = Vec::new();

    let report = run(
        config(&harness, provider(responses), Approve::Flag),
        &mut console,
    )
    .expect("run");

    assert_eq!(report.outcome, Outcome::Completed, "{:?}", report.failure);
    assert!(report.plan.is_some());
    let journal = std::fs::read_to_string(report.root.join("journal.jsonl")).expect("journal");
    assert!(journal.contains("F999"), "{journal}");
    assert!(journal.contains("rejected"), "{journal}");
}

#[test]
fn three_invalid_plans_end_as_a_named_failure() {
    let harness = harness();
    let invalid = Json::parse(r#"{"gaps":[],"tasks":[]}"#).expect("json");
    let responses = (0..3)
        .map(|index| {
            ModelResponse::calls(vec![call(&format!("c{index}"), "propose_plan", &invalid)])
        })
        .collect();
    let mut console = Vec::new();

    let report = run(
        config(&harness, provider(responses), Approve::Flag),
        &mut console,
    )
    .expect("run");

    assert_eq!(report.outcome, Outcome::Failed);
    assert!(
        report
            .failure
            .as_deref()
            .is_some_and(|reason| reason.contains("valid plan"))
    );
    assert!(report.results.is_empty());
    assert_eq!(
        verify_record(&report.root).expect("verify").outcome,
        "failed"
    );
}

#[test]
fn a_finding_cannot_cite_evidence_the_host_did_not_record() {
    let harness = harness();
    let fact = fact_id(&harness.source);
    let responses = vec![
        ModelResponse::calls(vec![call("c1", "propose_plan", &one_task_plan(&fact))]),
        ModelResponse::calls(vec![call("c2", "record_finding", &finding(&fact, "R9"))]),
        ModelResponse::calls(vec![call("c3", "finish", &finish())]),
    ];
    let mut console = Vec::new();

    let report = run(
        config(&harness, provider(responses), Approve::Flag),
        &mut console,
    )
    .expect("run");

    assert_eq!(report.outcome, Outcome::Completed, "{:?}", report.failure);
    assert!(
        report.findings.is_empty(),
        "the ungrounded finding was kept"
    );
    let journal = std::fs::read_to_string(report.root.join("journal.jsonl")).expect("journal");
    assert!(journal.contains("R9"), "{journal}");
}

#[test]
fn a_turn_budget_is_an_incomplete_record_not_a_conclusion() {
    let harness = harness();
    let fact = fact_id(&harness.source);
    let mut config = config(
        &harness,
        provider(vec![ModelResponse::calls(vec![call(
            "c1",
            "propose_plan",
            &one_task_plan(&fact),
        )])]),
        Approve::Flag,
    );
    config.max_turns = 1;
    let mut console = Vec::new();

    let report = run(config, &mut console).expect("run");

    assert_eq!(report.outcome, Outcome::BudgetExhausted);
    assert!(report.results.is_empty());
    assert!(
        report
            .failure
            .as_deref()
            .is_some_and(|reason| reason.contains("budget"))
    );
    let text = std::fs::read_to_string(report.root.join("report.md")).expect("report");
    assert!(text.contains("deliberately incomplete"), "{text}");
    assert_eq!(
        verify_record(&report.root).expect("verify").outcome,
        "budget_exhausted"
    );
}

#[test]
fn a_provider_failure_is_recorded_as_a_failure() {
    let harness = harness();
    let mut console = Vec::new();

    let report = run(
        config(&harness, provider(Vec::new()), Approve::Flag),
        &mut console,
    )
    .expect("run");

    assert_eq!(report.outcome, Outcome::Failed);
    assert!(
        report
            .failure
            .as_deref()
            .is_some_and(|reason| reason.contains("scripted transcript"))
    );
    assert_eq!(
        verify_record(&report.root).expect("verify").outcome,
        "failed"
    );
}

#[test]
fn a_failed_dependency_skips_its_dependent() {
    let harness = harness();
    let fact = fact_id(&harness.source);
    let responses = vec![
        ModelResponse::calls(vec![call("c1", "propose_plan", &dependency_plan(&fact))]),
        ModelResponse::calls(vec![call("c2", "finish", &finish())]),
    ];
    let mut config = config(&harness, provider(responses), Approve::Flag);
    config.allow_exec = vec!["nonexistent-program".to_string()];
    let mut console = Vec::new();

    let report = run(config, &mut console).expect("run");

    assert_eq!(report.outcome, Outcome::Completed, "{:?}", report.failure);
    assert_eq!(report.results.len(), 2);
    assert_eq!(report.results[0].status, "launch_failed");
    assert_eq!(report.results[1].status, "skipped");
    assert!(report.results[1].note.contains("dependency"));
}

#[test]
fn finish_before_a_plan_is_a_no_plan_outcome() {
    let harness = harness();
    let mut console = Vec::new();

    let report = run(
        config(
            &harness,
            provider(vec![ModelResponse::calls(vec![call(
                "c1",
                "finish",
                &finish_no_plan(),
            )])]),
            Approve::Flag,
        ),
        &mut console,
    )
    .expect("run");

    assert_eq!(report.outcome, Outcome::NoPlan);
    assert!(report.plan.is_none());
    assert!(report.results.is_empty());
    assert!(report.delivery.is_some());
    assert_eq!(
        verify_record(&report.root).expect("verify").outcome,
        "no_plan"
    );
}

#[test]
fn the_scripted_provider_reports_its_identity_in_the_record() {
    let harness = harness();
    let fact = fact_id(&harness.source);
    let scripted = provider(all_responses(&fact));
    let described = scripted.describe();
    assert!(described.contains("scripted"), "{described}");
    let _ = harness.temp;
    let _ = ModelProvider::describe(&scripted);
}
