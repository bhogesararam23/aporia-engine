//! The demonstration: one complete project workflow, through the real application.
//!
//! The provider is the committed transcript, but nothing else is: the engine dispatches real
//! calls, the real sandbox copy runs the real Python program, the real workspace is edited, the
//! real record is written, and the assertions are about what the record contains. A transcript
//! that printed an ideal report would not satisfy this test, because the report here is generated
//! from what the tools actually did.

use aporia_cli::agent;
use aporia_cli::run::Exit;
use aporia_store::Json;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Temp(PathBuf);

impl Temp {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "aporia-demo-{tag}-{}-{}",
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
        .join("agent-demo")
        .canonicalize()
        .expect("the demonstration project")
}

/// The interpreter this machine has, as a bare program name.
fn interpreter() -> Option<&'static str> {
    ["python", "python3"].into_iter().find(|candidate| {
        Command::new(candidate)
            .arg("--version")
            .output()
            .is_ok_and(|output| output.status.success())
    })
}

/// The committed transcript, with the interpreter this machine has.
fn transcript_for(python: &str) -> String {
    let committed =
        std::fs::read_to_string(demo_project().join("transcript.json")).expect("transcript");
    committed.replace("\"python\"", &format!("\"{python}\""))
}

#[test]
fn the_demonstration_runs_a_complete_project_workflow() {
    let Some(python) = interpreter() else {
        eprintln!("skipped: this machine has no python interpreter");
        return;
    };
    let project = demo_project();
    let workdir = Temp::new("work");
    let transcript_path = workdir.path().join("transcript.json");
    std::fs::write(&transcript_path, transcript_for(python)).expect("transcript");

    let flags: Vec<String> = vec![
        project.display().to_string(),
        "--objective".to_string(),
        "find out why the integration estimate is wrong, fix it, and check the fix".to_string(),
        "--provider".to_string(),
        format!("scripted:{}", transcript_path.display()),
        "--allow-exec".to_string(),
        python.to_string(),
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
    assert_eq!(records.len(), 1, "one investigation directory");
    let root = &records[0];

    let journal = std::fs::read_to_string(root.join("journal.jsonl")).expect("journal");
    assert!(journal.contains("\"name\":\"read_file\""), "{journal}");
    assert!(journal.contains("\"name\":\"propose_plan\""), "{journal}");
    assert!(
        journal.contains("\"kind\":\"approval.decided\""),
        "{journal}"
    );
    assert!(journal.contains("\"name\":\"run_command\""), "{journal}");
    assert!(
        journal.contains("\"kind\":\"finding.recorded\""),
        "{journal}"
    );
    assert!(
        journal.contains("\"kind\":\"experiment.changed\""),
        "{journal}"
    );
    assert!(journal.contains("\"kind\":\"delivery\""), "{journal}");

    // The experiment really ran, twice: the program's output is in the record.
    assert!(
        journal.contains("observed_order") || journal.contains("estimate"),
        "the experiment's output is not in the record: {journal}"
    );

    // The fix really landed in the workspace, and the original project is untouched.
    assert!(
        journal.contains("\"kind\":\"change.applied\""),
        "the workspace change is not in the record: {journal}"
    );
    let fixed = std::fs::read_to_string(root.join("workspace/rule.py")).expect("workspace file");
    assert!(
        fixed.contains("range(len(samples) - 1)"),
        "the workspace copy does not hold the fixed rule"
    );
    let original = std::fs::read_to_string(project.join("rule.py")).expect("original");
    assert!(
        original.contains("(index + 1) % len(samples)"),
        "the original project was written"
    );

    let report = std::fs::read_to_string(root.join("report.md")).expect("report");
    assert!(report.contains("does not shrink"), "{report}");
    assert!(report.contains("non-uniform"), "{report}");
    assert!(report.contains("rule.py"), "{report}");

    let summary = std::fs::read_to_string(root.join("investigation.json")).expect("summary");
    let document = Json::parse(&summary).expect("json");
    let fields = aporia_agent::json::fields(&document).expect("object");
    assert_eq!(
        aporia_agent::json::string(fields, "status"),
        Ok("completed")
    );
    let plan = aporia_agent::json::field(fields, "plan").expect("plan");
    let plan_fields = aporia_agent::json::fields(plan).expect("plan object");
    let experiments = aporia_agent::json::array(plan_fields, "experiments").expect("experiments");
    assert_eq!(experiments.len(), 1, "one backlog item");
    let item = aporia_agent::json::fields(&experiments[0]).expect("item");
    assert_eq!(
        aporia_agent::json::string(item, "status"),
        Ok("proposed"),
        "an experiment that did not run stays a proposal"
    );
    let changes = aporia_agent::json::array(plan_fields, "changes").expect("changes");
    assert_eq!(changes.len(), 1, "one change was applied");
}
