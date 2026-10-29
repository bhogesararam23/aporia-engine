//! A plan is checked, not read: priorities, evidence ids, dependencies and permissions all have
//! refusals, and a finding cannot cite something the investigation never recorded.

use aporia_agent::inspect::{Fact, FactKind, Inspection, InspectionSummary};
use aporia_agent::plan::{
    FindingKind, Gap, Operation, Plan, Priority, Severity, Task, TaskTool, parse_finding,
    parse_plan, validate_plan,
};
use aporia_sandbox::Sandbox;
use aporia_store::Json;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Temp(PathBuf);

impl Temp {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "aporia-agent-plan-{}-{}-{tag}",
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

struct Fixture {
    sandbox: Sandbox,
    inspection: Inspection,
    _temp: Temp,
}

fn fixture() -> Fixture {
    let temp = Temp::new("work");
    let source = temp.path().join("source");
    std::fs::create_dir_all(&source).expect("source");
    std::fs::write(source.join("model.ap"), b"model x { }\n").expect("model");
    let workspace = temp.path().join("workspace");
    let (sandbox, _) = Sandbox::create(&source, &workspace).expect("sandbox");
    let inspection = Inspection {
        facts: vec![Fact {
            id: "F1".to_string(),
            kind: FactKind::Model,
            path: "model.ap".to_string(),
            detail: "model x".to_string(),
        }],
        summary: InspectionSummary {
            entries: 1,
            files: 1,
            bytes: 13,
            truncated: false,
            skipped: Vec::new(),
            git_head: None,
        },
    };
    Fixture {
        sandbox,
        inspection,
        _temp: temp,
    }
}

fn gap(id: &str) -> Gap {
    Gap {
        id: id.to_string(),
        title: "A gap".to_string(),
        priority: Priority::High,
        why: "because the fact says so".to_string(),
        condition: "if the model file is the only artefact".to_string(),
        evidence: vec!["F1".to_string()],
    }
}

fn instrument_task(id: &str) -> Task {
    Task {
        id: id.to_string(),
        title: "Run the atlas".to_string(),
        question: "where does the rule fail?".to_string(),
        gap_ids: vec!["G1".to_string()],
        depends_on: Vec::new(),
        method: "instrument_run".to_string(),
        tool: TaskTool::InstrumentRun,
        arguments: Json::object(vec![
            ("model", Json::text("model.ap")),
            ("budget", Json::count(80)),
        ]),
        expected_evidence: "an archive".to_string(),
        success_criteria: "a completed run".to_string(),
        estimated_cost: "80 evaluations".to_string(),
        risk: "low".to_string(),
    }
}

fn plan_with(tasks: Vec<Task>) -> Plan {
    Plan {
        gaps: vec![gap("G1")],
        tasks,
    }
}

#[test]
fn a_valid_plan_is_ordered_and_resolves_to_an_operation() {
    let fixture = fixture();
    let plan = plan_with(vec![instrument_task("T1")]);
    let validated =
        validate_plan(&plan, &fixture.inspection, &[], &fixture.sandbox).expect("valid");
    assert_eq!(validated.order, vec![0]);
    assert_eq!(
        validated.tasks[0].operation,
        Operation::Instrument {
            model: "model.ap".to_string(),
            budget: 80,
            archive: "archives/T1".to_string(),
        }
    );
}

#[test]
fn dependencies_are_ordered_not_trusted() {
    let fixture = fixture();
    let mut first = instrument_task("T1");
    first.depends_on = vec!["T2".to_string()];
    let second = instrument_task("T2");
    let validated = validate_plan(
        &plan_with(vec![first, second]),
        &fixture.inspection,
        &[],
        &fixture.sandbox,
    )
    .expect("valid");
    assert_eq!(validated.order, vec![1, 0]);
}

#[test]
fn a_cycle_is_refused_by_name() {
    let fixture = fixture();
    let mut first = instrument_task("T1");
    first.depends_on = vec!["T2".to_string()];
    let mut second = instrument_task("T2");
    second.depends_on = vec!["T1".to_string()];
    let error = validate_plan(
        &plan_with(vec![first, second]),
        &fixture.inspection,
        &[],
        &fixture.sandbox,
    )
    .expect_err("cycle");
    assert!(error.contains("cycle"), "{error}");
}

#[test]
fn a_gap_cannot_cite_an_uninspected_fact() {
    let fixture = fixture();
    let mut gap = gap("G1");
    gap.evidence = vec!["F9".to_string()];
    let plan = Plan {
        gaps: vec![gap],
        tasks: vec![instrument_task("T1")],
    };
    let error =
        validate_plan(&plan, &fixture.inspection, &[], &fixture.sandbox).expect_err("unknown fact");
    assert!(error.contains("F9"), "{error}");
}

#[test]
fn a_priority_outside_the_charter_is_refused_at_parse_time() {
    let value = Json::parse(
        r#"{"gaps":[{"id":"G1","title":"t","priority":"mandatory","why":"w","condition":"c","evidence":["F1"]}],"tasks":[]}"#,
    )
    .expect("json");
    let error = parse_plan(&value).expect_err("priority");
    assert!(error.contains("priority"), "{error}");
}

#[test]
fn exec_is_refused_when_the_investigation_was_not_allowed_to_run_it() {
    let fixture = fixture();
    let mut task = instrument_task("T1");
    task.tool = TaskTool::Exec;
    task.arguments = Json::object(vec![(
        "argv",
        Json::Arr(vec![Json::text("python"), Json::text("-V")]),
    )]);
    let error = validate_plan(
        &plan_with(vec![task.clone()]),
        &fixture.inspection,
        &[],
        &fixture.sandbox,
    )
    .expect_err("not allowed");
    assert!(error.contains("--allow-exec python"), "{error}");

    let validated = validate_plan(
        &plan_with(vec![task]),
        &fixture.inspection,
        &["python".to_string()],
        &fixture.sandbox,
    )
    .expect("allowed");
    assert_eq!(
        validated.tasks[0].operation,
        Operation::Exec {
            argv: vec!["python".to_string(), "-V".to_string()],
            timeout_ms: 120_000,
        }
    );
}

#[test]
fn a_model_path_cannot_leave_the_workspace() {
    let fixture = fixture();
    let mut task = instrument_task("T1");
    task.arguments = Json::object(vec![
        ("model", Json::text("../model.ap")),
        ("budget", Json::count(80)),
    ]);
    let error = validate_plan(
        &plan_with(vec![task]),
        &fixture.inspection,
        &[],
        &fixture.sandbox,
    )
    .expect_err("escape");
    assert!(error.contains("leaves the workspace"), "{error}");
}

#[test]
fn the_budget_is_bounded() {
    let fixture = fixture();
    for budget in [0, 100_001] {
        let mut task = instrument_task("T1");
        task.arguments = Json::object(vec![
            ("model", Json::text("model.ap")),
            ("budget", Json::count(budget)),
        ]);
        let error = validate_plan(
            &plan_with(vec![task]),
            &fixture.inspection,
            &[],
            &fixture.sandbox,
        )
        .expect_err("budget");
        assert!(error.contains("budget"), "{error}");
    }
}

#[test]
fn an_archive_that_already_exists_is_not_replaced() {
    let fixture = fixture();
    std::fs::create_dir_all(fixture.sandbox.root().join("archives/T1")).expect("archive");
    let error = validate_plan(
        &plan_with(vec![instrument_task("T1")]),
        &fixture.inspection,
        &[],
        &fixture.sandbox,
    )
    .expect_err("exists");
    assert!(error.contains("already exists"), "{error}");
}

#[test]
fn a_finding_must_cite_known_evidence() {
    let known: BTreeSet<String> = ["F1".to_string(), "R1".to_string()].into_iter().collect();
    let value = Json::parse(
        r#"{"kind":"observation","severity":"high","text":"a region failed","evidence":["R1","F1"],"uncertainty":"sampling only"}"#,
    )
    .expect("json");
    let finding = parse_finding(&value, &known).expect("valid");
    assert_eq!(finding.kind, FindingKind::Observation);
    assert_eq!(finding.severity, Severity::High);

    let unknown = Json::parse(
        r#"{"kind":"observation","severity":"high","text":"a region failed","evidence":["R9"],"uncertainty":"none"}"#,
    )
    .expect("json");
    let error = parse_finding(&unknown, &known).expect_err("unknown");
    assert!(error.contains("R9"), "{error}");
}

#[test]
fn an_unknown_field_is_refused_rather_than_dropped() {
    let value = Json::parse(r#"{"gaps":[],"tasks":[],"confidence":"high"}"#).expect("json");
    let error = parse_plan(&value).expect_err("unknown");
    assert!(error.contains("unknown field"), "{error}");
}

#[test]
fn a_plan_round_trips_through_json() {
    let value = Json::parse(
        r#"{"gaps":[{"id":"G1","title":"t","priority":"recommended","why":"w","condition":"c","evidence":["F1"]}],"tasks":[{"id":"T1","title":"t","question":"q","gap_ids":["G1"],"depends_on":[],"method":"m","tool":"instrument_run","arguments":{"model":"model.ap","budget":80},"expected_evidence":"e","success_criteria":"s","estimated_cost":"c","risk":"r"}]}"#,
    )
    .expect("json");
    let plan = parse_plan(&value).expect("parse");
    let json = plan.to_json();
    let again = parse_plan(&json).expect("reparse");
    assert_eq!(plan, again);
}
