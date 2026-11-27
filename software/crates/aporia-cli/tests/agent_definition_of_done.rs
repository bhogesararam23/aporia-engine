//! The definition of done, in one chain: an imperfect real project, grounded recommendations,
//! an approved plan, useful experiments executed in the sandbox, a verified improvement, an
//! authorised export that round-trips through git, an interruption survived by resume, and a
//! complete reproducible record at the end.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Temp(PathBuf);

impl Temp {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "aporia-done-{tag}-{}-{}",
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

fn aporia() -> Command {
    Command::new(env!("CARGO_BIN_EXE_aporia"))
}

fn node_available() -> bool {
    Command::new("node")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
}

fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
}

/// Run the first six turns interactively and let the terminal die: an open record mid-workflow.
fn interrupt_six_turns(project: &Path, workdir: &Path, t1: &Path) -> PathBuf {
    let mut child = aporia()
        .arg("agent")
        .arg(project)
        .args([
            "--objective",
            "find out why the variance is wrong, fix the estimator, and check the fix",
        ])
        .arg("--provider")
        .arg(format!("scripted:{}", t1.display()))
        .args(["--allow-exec", "node", "--allow-write"])
        .arg("--workdir")
        .arg(workdir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(b"continue\ncontinue\ny\ncontinue\ncontinue\ncontinue\n")
        .expect("write");
    drop(child.stdin.take());
    let out = child.wait_with_output().expect("wait");
    assert_eq!(out.status.code(), Some(0));
    let mut records: Vec<PathBuf> = std::fs::read_dir(workdir)
        .expect("records")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    records.sort();
    assert_eq!(records.len(), 1);
    records.remove(0)
}

/// Export with authorisation and round-trip the patch onto a pristine copy of the source.
fn export_and_roundtrip(record: &Path, project: &Path, root: &Path) {
    let patch = root.join("fix.patch");
    let out = aporia()
        .args(["agent", "export"])
        .arg(record)
        .arg("--out")
        .arg(&patch)
        .arg("--yes")
        .stdin(Stdio::null())
        .output()
        .expect("export");
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let target = root.join("target");
    std::fs::create_dir_all(&target).expect("dir");
    for entry in std::fs::read_dir(project).expect("read") {
        let entry = entry.expect("entry");
        if entry.path().is_file() {
            std::fs::copy(entry.path(), target.join(entry.file_name())).expect("copy");
        }
    }
    let applied = Command::new("git")
        .args(["-c", "core.autocrlf=false", "apply"])
        .arg(&patch)
        .current_dir(&target)
        .output()
        .expect("git apply");
    assert!(
        applied.status.success(),
        "git apply: {}",
        String::from_utf8_lossy(&applied.stderr)
    );
    assert_eq!(
        std::fs::read(target.join("pipeline.js")).expect("file"),
        std::fs::read(record.join("workspace/pipeline.js")).expect("file"),
        "the exported patch did not reproduce the verified workspace"
    );
}

#[test]
fn the_complete_chain_from_imperfect_project_to_authorised_patch() {
    if !node_available() || !git_available() {
        eprintln!("skipped: this machine has no node or git");
        return;
    }
    let project = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/agent-demo-node")
        .canonicalize()
        .expect("node demo");
    let root = Temp::new("chain");
    let workdir = root.path().join("investigations");

    // Split the committed transcript: six turns, then the terminal dies, then the rest resumes.
    let committed = std::fs::read_to_string(project.join("transcript.json")).expect("transcript");
    let document = aporia_store::Json::parse(&committed).expect("json");
    let fields = aporia_agent::json::fields(&document).expect("object");
    let responses = aporia_agent::json::array(fields, "responses").expect("responses");
    let t1 = root.path().join("t1.json");
    let t2 = root.path().join("t2.json");
    for (path, part) in [(&t1, &responses[..6]), (&t2, &responses[6..])] {
        let doc = aporia_store::Json::object(vec![
            ("note", aporia_store::Json::text("definition of done")),
            ("responses", aporia_store::Json::Arr(part.to_vec())),
        ]);
        std::fs::write(path, doc.to_pretty()).expect("transcript");
    }

    // 1. Open, inspect, plan, approve, measure, fix, measure again -- then the process dies.
    let record = interrupt_six_turns(&project, &workdir, &t1);
    assert!(
        !record.join("manifest.json").exists(),
        "interrupted, not finished"
    );

    // 2. Resume: the remaining turns run, the completed ones do not run again.
    let out = aporia()
        .args(["agent", "resume"])
        .arg(&record)
        .arg("--provider")
        .arg(format!("scripted:{}", t2.display()))
        .arg("--batch")
        .stdin(Stdio::null())
        .output()
        .expect("resume");
    assert_eq!(
        out.status.code(),
        Some(1),
        "critical finding; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let journal = std::fs::read_to_string(record.join("journal.jsonl")).expect("journal");
    assert_eq!(
        journal
            .lines()
            .filter(|line| line.contains("\"kind\":\"tool.started\"") && line.contains("call-3"))
            .count(),
        1,
        "the first experiment ran again after the resume"
    );

    // 3. Export with explicit authorisation, and round-trip the patch through git.
    export_and_roundtrip(&record, &project, root.path());

    // 4. The record is complete and verifiable, and the original project is untouched.
    aporia_agent::verify_record(&record).expect("record verifies");
    let journal = std::fs::read_to_string(record.join("journal.jsonl")).expect("journal");
    assert!(journal.contains("\"kind\":\"session.resumed\""));
    assert!(journal.contains("\"kind\":\"changeset.exported\""));
    assert!(journal.contains("\"by\":\"cli\""));
    let report = std::fs::read_to_string(record.join("report.md")).expect("report");
    assert!(report.contains("## Findings"));
    assert!(report.contains("## Experiment backlog"));
    assert!(report.contains("## Delivery"));
    assert!(report.contains("Journal:"));
    let original = std::fs::read_to_string(project.join("pipeline.js")).expect("original");
    assert!(
        original.contains("variance /= n;"),
        "the original was written"
    );
    let state = aporia_agent::read_checkpoint(&record)
        .expect("read")
        .expect("checkpoint");
    assert_eq!(state.status, aporia_agent::SessionStatus::Completed);
}
