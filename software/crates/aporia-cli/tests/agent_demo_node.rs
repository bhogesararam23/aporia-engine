//! The second demonstration: a complete project workflow on a Node.js data pipeline, through
//! the real application. The defect is an estimator bias (population vs sample variance), the
//! experiment measures it, the fix lands in the workspace, the project's own tests run, and the
//! before/after numbers are in the record. Nothing here is simulated except the provider.

use aporia_cli::agent;
use aporia_cli::run::Exit;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Temp(PathBuf);

impl Temp {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "aporia-demo-node-{tag}-{}-{}",
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

fn demo_project() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("examples")
        .join("agent-demo-node")
        .canonicalize()
        .expect("the demonstration project")
}

fn node_available() -> bool {
    Command::new("node")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
}

#[test]
fn the_node_pipeline_workflow_runs_end_to_end() {
    if !node_available() {
        eprintln!("skipped: this machine has no node interpreter");
        return;
    }
    let project = demo_project();
    let workdir = Temp::new("work");
    let transcript = project.join("transcript.json");

    let flags: Vec<String> = vec![
        project.display().to_string(),
        "--objective".to_string(),
        "find out why the variance is wrong, fix the estimator, and check the fix".to_string(),
        "--provider".to_string(),
        format!("scripted:{}", transcript.display()),
        "--allow-exec".to_string(),
        "node".to_string(),
        "--allow-write".to_string(),
        "--workdir".to_string(),
        workdir.path().join("investigations").display().to_string(),
        "--batch".to_string(),
    ];
    let mut out: Vec<u8> = Vec::new();
    let mut err: Vec<u8> = Vec::new();
    let exit = agent::command(&flags, &mut out, &mut err);
    assert_eq!(
        exit,
        Exit::Suspicious,
        "the investigation should finish with a critical finding; stderr: {}",
        String::from_utf8_lossy(&err)
    );

    let records: Vec<PathBuf> = std::fs::read_dir(workdir.path().join("investigations"))
        .expect("records")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect();
    assert_eq!(records.len(), 1);
    let root = &records[0];
    let journal = std::fs::read_to_string(root.join("journal.jsonl")).expect("journal");

    // The workflow really happened: inspection, plan, approval, execution, findings, backlog.
    for needle in [
        "\"name\":\"read_file\"",
        "\"name\":\"propose_plan\"",
        "\"kind\":\"approval.decided\"",
        "\"name\":\"run_command\"",
        "\"kind\":\"finding.recorded\"",
        "\"kind\":\"experiment.changed\"",
        "\"kind\":\"delivery\"",
        "\"kind\":\"change.applied\"",
    ] {
        assert!(journal.contains(needle), "missing {needle}");
    }

    // The before and after measurements are both in the record, and they differ.
    assert!(
        journal.contains("-0.5714285714285712"),
        "the defective measurement is not in the record"
    );
    assert!(
        journal.contains("variance_error\\\": 0"),
        "the fixed measurement is not in the record"
    );
    // The project's own tests ran and passed inside the sandbox.
    assert!(journal.contains("pass 2"), "the project tests did not run");

    // The fix landed in the workspace; the original project is untouched.
    let fixed = std::fs::read_to_string(root.join("workspace/pipeline.js")).expect("workspace");
    assert!(fixed.contains("variance /= n - 1;"), "{fixed}");
    let original = std::fs::read_to_string(project.join("pipeline.js")).expect("original");
    assert!(
        original.contains("variance /= n;"),
        "the original was written"
    );

    // The backlog keeps the genuinely open question as a proposal.
    let summary = std::fs::read_to_string(root.join("investigation.json")).expect("summary");
    let document = aporia_store::Json::parse(&summary).expect("json");
    let fields = aporia_agent::json::fields(&document).expect("object");
    let plan = aporia_agent::json::field(fields, "plan").expect("plan");
    let plan_fields = aporia_agent::json::fields(plan).expect("plan object");
    let experiments = aporia_agent::json::array(plan_fields, "experiments").expect("experiments");
    assert_eq!(experiments.len(), 1);
    let item = aporia_agent::json::fields(&experiments[0]).expect("item");
    assert_eq!(
        aporia_agent::json::string(item, "status"),
        Ok("proposed"),
        "an experiment that did not run stays a proposal"
    );

    // The record verifies, and the report carries the comparison and the way home.
    aporia_agent::verify_record(root).expect("record verifies");
    let report = std::fs::read_to_string(root.join("report.md")).expect("report");
    assert!(report.contains("variance_error"), "{report}");
    assert!(report.contains("## Delivery"), "{report}");
    assert!(report.contains("aporia agent export"), "{report}");
}
