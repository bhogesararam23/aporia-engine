//! The experiment backlog through the real control tools: what the model is told after an
//! update must be the state the record now holds, not the state before the update.

use aporia_agent::investigation::{ExperimentStatus, Investigation};
use aporia_agent::orchestrator::Delivery;
use aporia_agent::provider::ToolCall;
use aporia_agent::scope::{ApprovalRequest, ScopeLimits};
use aporia_agent::tools::context::Session;
use aporia_agent::tools::{Registry, ToolContext, control, dispatch};
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
            "aporia-backlog-{tag}-{}-{}",
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
            allow_write: false,
            max_turns: 8,
            max_tool_calls: 16,
            timeout_ms: 60_000,
        }
    }

    fn deliver(&mut self, _delivery: Delivery) -> bool {
        true
    }
}

fn call(id: &str, name: &str, arguments: &str) -> ToolCall {
    ToolCall {
        id: id.to_string(),
        name: name.to_string(),
        arguments: arguments.to_string(),
    }
}

const PROPOSAL: &str = r#"{
    "title": "measure the variance",
    "question": "how much does the result vary between seeds?",
    "reason": "only one seed was ever run",
    "evidence": ["I1"],
    "priority": "high",
    "methodology": "run the pipeline with five seeds",
    "variables": ["seed"],
    "dependencies": [],
    "required_data": [],
    "expected_outcomes": "a spread of results",
    "metrics": ["standard deviation"],
    "estimated_cost": "a minute",
    "success_criteria": "the spread is measured",
    "risks": "none"
}"#;

#[test]
fn an_update_reports_the_state_the_record_now_holds() {
    let source = Temp::new("src");
    fs::write(source.path().join("a.txt"), b"a").expect("file");
    let workspace = Temp::new("ws");
    let (sandbox, _) =
        Sandbox::create(source.path(), &workspace.path().join("copy")).expect("sandbox");
    let instrument = std::env::current_exe().expect("exe");
    let mut registry = Registry::new();
    control::register(&mut registry).expect("control tools");
    let mut session = TestSession {
        investigation: Investigation::default(),
        events: Vec::new(),
    };
    session.investigation.add_evidence("invocation", "a run");
    let cancel = AtomicBool::new(false);
    let mut context = ToolContext::new(&sandbox, source.path(), &instrument, None, &cancel)
        .with_session(&mut session);

    let output = dispatch(
        &registry,
        &mut context,
        &call("c1", "propose_experiment", PROPOSAL),
    )
    .expect("propose");
    assert!(output.text.contains("X1"), "{}", output.text);

    let output = dispatch(
        &registry,
        &mut context,
        &call(
            "c2",
            "update_experiment",
            r#"{"id":"X1","status":"running"}"#,
        ),
    )
    .expect("running");
    assert!(
        output.text.contains("is now running"),
        "the model was told the wrong status: {}",
        output.text
    );

    let output = dispatch(
        &registry,
        &mut context,
        &call(
            "c3",
            "update_experiment",
            r#"{"id":"X1","status":"completed","result":"the spread was 0.02 (I1)"}"#,
        ),
    )
    .expect("completed");
    assert!(
        output.text.contains("is now completed"),
        "the model was told the wrong status: {}",
        output.text
    );

    // A finished experiment keeps its outcome; the answer must say so, not echo the request.
    let output = dispatch(
        &registry,
        &mut context,
        &call(
            "c4",
            "update_experiment",
            r#"{"id":"X1","status":"proposed"}"#,
        ),
    )
    .expect("downgrade attempt");
    assert!(
        output.text.contains("is now completed"),
        "a kept outcome was reported as the requested downgrade: {}",
        output.text
    );
    assert_eq!(
        session.investigation.experiment("X1").expect("X1").status,
        ExperimentStatus::Completed
    );
    assert!(
        session
            .events
            .iter()
            .filter(|event| event.starts_with("experiment.changed"))
            .count()
            >= 4
    );
}

#[test]
fn a_finished_experiment_must_cite_recorded_evidence() {
    let source = Temp::new("src3");
    fs::write(source.path().join("a.txt"), b"a").expect("file");
    let workspace = Temp::new("ws3");
    let (sandbox, _) =
        Sandbox::create(source.path(), &workspace.path().join("copy")).expect("sandbox");
    let instrument = std::env::current_exe().expect("exe");
    let mut registry = Registry::new();
    control::register(&mut registry).expect("control tools");
    let mut session = TestSession {
        investigation: Investigation::default(),
        events: Vec::new(),
    };
    session.investigation.add_evidence("invocation", "a run");
    let cancel = AtomicBool::new(false);
    {
        let mut context = ToolContext::new(&sandbox, source.path(), &instrument, None, &cancel)
            .with_session(&mut session);
        dispatch(
            &registry,
            &mut context,
            &call("c1", "propose_experiment", PROPOSAL),
        )
        .expect("propose");

        // No result at all: completing is a claim, and a claim needs evidence.
        let failure = dispatch(
            &registry,
            &mut context,
            &call(
                "c2",
                "update_experiment",
                r#"{"id":"X1","status":"completed"}"#,
            ),
        )
        .expect_err("no result");
        assert_eq!(
            failure.class,
            aporia_agent::tools::ErrorClass::InvalidArguments
        );

        // A result that cites nothing recorded is still not an outcome.
        let failure = dispatch(
            &registry,
            &mut context,
            &call(
                "c3",
                "update_experiment",
                r#"{"id":"X1","status":"completed","result":"it worked great (I999)"}"#,
            ),
        )
        .expect_err("unknown evidence");
        assert_eq!(
            failure.class,
            aporia_agent::tools::ErrorClass::InvalidArguments
        );
    }

    // The item is still a proposal: the refused updates changed nothing.
    assert_eq!(
        session.investigation.experiment("X1").expect("X1").status,
        ExperimentStatus::Proposed
    );

    // A grounded outcome is accepted, for every terminal status.
    let mut context = ToolContext::new(&sandbox, source.path(), &instrument, None, &cancel)
        .with_session(&mut session);
    for (id, status) in [
        ("c4", "completed"),
        ("c5", "failed"),
        ("c6", "inconclusive"),
    ] {
        dispatch(
            &registry,
            &mut context,
            &call(
                id,
                "update_experiment",
                &format!(r#"{{"id":"X1","status":"{status}","result":"the run showed it (I1)"}}"#),
            ),
        )
        .expect("grounded outcome");
    }
}

#[test]
fn an_update_of_an_unknown_experiment_is_a_not_found_failure() {
    let source = Temp::new("src2");
    fs::write(source.path().join("a.txt"), b"a").expect("file");
    let workspace = Temp::new("ws2");
    let (sandbox, _) =
        Sandbox::create(source.path(), &workspace.path().join("copy")).expect("sandbox");
    let instrument = std::env::current_exe().expect("exe");
    let mut registry = Registry::new();
    control::register(&mut registry).expect("control tools");
    let mut session = TestSession {
        investigation: Investigation::default(),
        events: Vec::new(),
    };
    let cancel = AtomicBool::new(false);
    let mut context = ToolContext::new(&sandbox, source.path(), &instrument, None, &cancel)
        .with_session(&mut session);
    let failure = dispatch(
        &registry,
        &mut context,
        &call(
            "c1",
            "update_experiment",
            r#"{"id":"X9","status":"running"}"#,
        ),
    )
    .expect_err("unknown item");
    assert_eq!(failure.class, aporia_agent::tools::ErrorClass::NotFound);
}
