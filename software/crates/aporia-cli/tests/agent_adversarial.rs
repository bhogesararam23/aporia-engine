//! Adversarial and recovery workflows through the real binary: an interrupted investigation
//! that resumes to completion, a spent budget that leaves an honest incomplete record, a timed-out
//! command the investigation survives, a reverted change with nothing to export, and a credential
//! in the parent environment that never reaches the child or the record.

use aporia_cli::run::Exit;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Temp(PathBuf);

impl Temp {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "aporia-adversarial-{tag}-{}-{}",
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

fn node_demo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..//examples/agent-demo-node")
        .canonicalize()
        .expect("node demo")
}

fn write_transcript(path: &Path, responses: &[aporia_store::Json]) {
    let document = aporia_store::Json::object(vec![
        ("note", aporia_store::Json::text("adversarial test")),
        ("responses", aporia_store::Json::Arr(responses.to_vec())),
    ]);
    std::fs::write(path, document.to_pretty()).expect("transcript");
}

fn record_dir(workdir: &Path) -> PathBuf {
    let mut records: Vec<PathBuf> = std::fs::read_dir(workdir)
        .expect("records")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    records.sort();
    assert_eq!(records.len(), 1, "one investigation directory");
    records.remove(0)
}

/// Scan every byte of a directory tree for the credential.
fn scan(dir: &Path, needle: &str) {
    for entry in std::fs::read_dir(dir).expect("dir") {
        let entry = entry.expect("entry");
        let path = entry.path();
        if path.is_dir() {
            scan(&path, needle);
        } else if let Ok(bytes) = std::fs::read(&path) {
            assert!(
                !String::from_utf8_lossy(&bytes).contains(needle),
                "the credential leaked into {}",
                path.display()
            );
        }
    }
}

#[test]
fn an_interrupted_node_workflow_resumes_and_completes() {
    if !node_available() {
        eprintln!("skipped: this machine has no node interpreter");
        return;
    }
    let project = node_demo();
    let root = Temp::new("interrupt");
    let workdir = root.path().join("investigations");
    let committed = std::fs::read_to_string(project.join("transcript.json")).expect("transcript");
    let document = aporia_store::Json::parse(&committed).expect("json");
    let fields = aporia_agent::json::fields(&document).expect("object");
    let responses = aporia_agent::json::array(fields, "responses").expect("responses");
    assert_eq!(responses.len(), 10);
    let t1 = root.path().join("t1.json");
    let t2 = root.path().join("t2.json");
    write_transcript(&t1, &responses[..6]);
    write_transcript(&t2, &responses[6..]);

    // Run the first six turns interactively, then let the terminal die (EOF). The plan approval
    // is answered with "y"; the other lines are conversation that advances the loop.
    let mut child = aporia()
        .arg("agent")
        .arg(&project)
        .args([
            "--objective",
            "find out why the variance is wrong, fix the estimator, and check the fix",
        ])
        .arg("--provider")
        .arg(format!("scripted:{}", t1.display()))
        .args(["--allow-exec", "node", "--allow-write"])
        .arg("--workdir")
        .arg(&workdir)
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
    assert_eq!(
        out.status.code(),
        Some(0),
        "stdout: {} stderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let record = record_dir(&workdir);
    let state = aporia_agent::read_checkpoint(&record)
        .expect("read")
        .expect("checkpoint");
    assert!(
        state.status.is_open(),
        "the record should be open after EOF"
    );
    assert!(!record.join("manifest.json").exists(), "no manifest yet");
    // The fix already landed before the interruption.
    let fixed = std::fs::read_to_string(record.join("workspace/pipeline.js")).expect("workspace");
    assert!(fixed.contains("variance /= n - 1;"));

    // Resume with the remaining responses and finish the investigation.
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
        "a critical finding means exit 1; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let journal = std::fs::read_to_string(record.join("journal.jsonl")).expect("journal");
    assert!(
        journal.contains("\"kind\":\"session.resumed\""),
        "{journal}"
    );
    assert!(
        journal.contains("-0.5714285714285712"),
        "the before measurement"
    );
    assert!(
        journal.contains("variance_error\\\": 0"),
        "the after measurement"
    );
    assert!(journal.contains("\"kind\":\"delivery\""), "{journal}");
    aporia_agent::verify_record(&record).expect("the resumed record verifies");
    let original = std::fs::read_to_string(project.join("pipeline.js")).expect("original");
    assert!(
        original.contains("variance /= n;"),
        "the original was written"
    );
}

#[test]
fn a_spent_turn_budget_leaves_an_incomplete_but_verifiable_record() {
    let root = Temp::new("budget");
    let source = root.path().join("src");
    std::fs::create_dir_all(&source).expect("dir");
    std::fs::write(source.join("notes.md"), b"# notes\n").expect("file");
    let transcript = root.path().join("t.json");
    std::fs::write(
        &transcript,
        r#"{"note":"t","responses":[
            {"content":null,"tool_calls":[{"id":"c1","name":"read_file","arguments":{"path":"notes.md"}}]},
            {"content":null,"tool_calls":[{"id":"c2","name":"read_file","arguments":{"path":"notes.md"}}]},
            {"content":null,"tool_calls":[{"id":"c3","name":"finish","arguments":{"summary":"never reached","next_steps":[],"unresolved":[]}}]}
        ]}"#,
    )
    .expect("transcript");
    let workdir = root.path().join("work");
    let out = aporia()
        .arg("agent")
        .arg(&source)
        .args(["--objective", "read the notes"])
        .arg("--provider")
        .arg(format!("scripted:{}", transcript.display()))
        .args(["--max-turns", "2"])
        .arg("--workdir")
        .arg(&workdir)
        .arg("--batch")
        .stdin(Stdio::null())
        .output()
        .expect("aporia runs");
    assert_eq!(out.status.code(), Some(12), "budget exhausted is exit 12");
    let record = record_dir(&workdir);
    let manifest = std::fs::read_to_string(record.join("manifest.json")).expect("manifest");
    assert!(
        manifest.contains("\"outcome\": \"budget_exhausted\""),
        "{manifest}"
    );
    let report = std::fs::read_to_string(record.join("report.md")).expect("report");
    assert!(report.contains("incomplete"), "{report}");
    aporia_agent::verify_record(&record).expect("an incomplete record still verifies");
}

#[test]
fn a_timed_out_command_is_a_failure_the_investigation_survives() {
    if !node_available() {
        eprintln!("skipped: this machine has no node interpreter");
        return;
    }
    let root = Temp::new("timeout");
    let source = root.path().join("src");
    std::fs::create_dir_all(&source).expect("dir");
    std::fs::write(source.join("notes.md"), b"# notes\n").expect("file");
    let transcript = root.path().join("t.json");
    std::fs::write(
        &transcript,
        r#"{"note":"t","responses":[
            {"content":null,"tool_calls":[{"id":"p","name":"propose_plan","arguments":{
                "objective":"run a slow program","gaps":[{"id":"G1","title":"the program may hang","priority":"high","why":"it never answers","condition":"no timeout is known","evidence":["F1"]}],
                "tasks":[{"id":"T1","title":"run it","question":"does it finish?","gap_ids":["G1"],"depends_on":[],"method":"run with a deadline","expected_evidence":"a timeout or an answer","success_criteria":"the behaviour is observed","estimated_cost":"a second","risk":"none"}],
                "scope":{"allow_exec":["node"],"allow_write":false,"max_turns":8,"max_tool_calls":16,"timeout_ms":60000},
                "success_criteria":"the hang is observed"}}]},
            {"content":null,"tool_calls":[{"id":"r","name":"run_command","arguments":{"argv":["node","-e","setTimeout(()=>{},10000)"],"timeout_ms":500}}]},
            {"content":null,"tool_calls":[{"id":"f","name":"record_finding","arguments":{"kind":"observation","severity":"high","text":"the program did not finish within the deadline and was killed","evidence":["I2"],"uncertainty":"what it would have printed is unknown"}}]},
            {"content":null,"tool_calls":[{"id":"done","name":"finish","arguments":{"summary":"the hang was observed","next_steps":[],"unresolved":["the program's intended output"]}}]}
        ]}"#,
    )
    .expect("transcript");
    let workdir = root.path().join("work");
    let out = aporia()
        .arg("agent")
        .arg(&source)
        .args(["--objective", "run a slow program"])
        .arg("--provider")
        .arg(format!("scripted:{}", transcript.display()))
        .args(["--allow-exec", "node"])
        .arg("--workdir")
        .arg(&workdir)
        .arg("--batch")
        .stdin(Stdio::null())
        .output()
        .expect("aporia runs");
    assert_eq!(
        out.status.code(),
        Some(1),
        "a high finding means exit 1; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let record = record_dir(&workdir);
    let journal = std::fs::read_to_string(record.join("journal.jsonl")).expect("journal");
    assert!(
        journal.contains("did not finish within 500 ms"),
        "the timeout is not in the record: {journal}"
    );
    assert!(
        journal.contains("\"kind\":\"delivery\""),
        "the investigation finished"
    );
    aporia_agent::verify_record(&record).expect("record verifies");
}

#[test]
fn a_reverted_change_leaves_nothing_to_export() {
    let root = Temp::new("revert");
    let source = root.path().join("src");
    std::fs::create_dir_all(&source).expect("dir");
    std::fs::write(source.join("rule.txt"), b"original\n").expect("file");
    let transcript = root.path().join("t.json");
    std::fs::write(
        &transcript,
        r#"{"note":"t","responses":[
            {"content":null,"tool_calls":[{"id":"p","name":"propose_plan","arguments":{
                "objective":"try a change and put it back","gaps":[{"id":"G1","title":"the rule may be wrong","priority":"high","why":"unverified","condition":"no test","evidence":["F1"]}],
                "tasks":[{"id":"T1","title":"try and revert","question":"does the change hold?","gap_ids":["G1"],"depends_on":[],"method":"edit then revert","expected_evidence":"a reverted workspace","success_criteria":"the workspace matches the original","estimated_cost":"a second","risk":"none"}],
                "scope":{"allow_exec":[],"allow_write":true,"max_turns":8,"max_tool_calls":16,"timeout_ms":60000},
                "success_criteria":"the workspace is back to the original"}}]},
            {"content":null,"tool_calls":[{"id":"w","name":"write_file","arguments":{"path":"rule.txt","content":"changed\n"}}]},
            {"content":null,"tool_calls":[{"id":"v","name":"revert_file","arguments":{"path":"rule.txt"}}]},
            {"content":null,"tool_calls":[{"id":"done","name":"finish","arguments":{"summary":"reverted","next_steps":[],"unresolved":[]}}]}
        ]}"#,
    )
    .expect("transcript");
    let workdir = root.path().join("work");
    let out = aporia()
        .arg("agent")
        .arg(&source)
        .args(["--objective", "try a change and put it back"])
        .arg("--provider")
        .arg(format!("scripted:{}", transcript.display()))
        .arg("--allow-write")
        .arg("--workdir")
        .arg(&workdir)
        .arg("--batch")
        .stdin(Stdio::null())
        .output()
        .expect("aporia runs");
    assert_eq!(out.status.code(), Some(0));
    let record = record_dir(&workdir);
    assert_eq!(
        std::fs::read_to_string(record.join("workspace/rule.txt")).expect("file"),
        "original\n"
    );
    let out_file = root.path().join("patch.diff");
    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let exit = aporia_cli::agent::command(
        &[
            "export".to_string(),
            record.display().to_string(),
            "--out".to_string(),
            out_file.display().to_string(),
            "--yes".to_string(),
        ],
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(exit, Exit::Clean);
    assert!(
        String::from_utf8_lossy(&stdout).contains("nothing to export"),
        "{}",
        String::from_utf8_lossy(&stdout)
    );
    assert!(!out_file.exists());
}

#[test]
fn a_parent_credential_never_reaches_the_child_or_the_record() {
    const CANARY: &str = "canary-7f3e9d2c1b";
    if !node_available() {
        eprintln!("skipped: this machine has no node interpreter");
        return;
    }
    let root = Temp::new("leak");
    let source = root.path().join("src");
    std::fs::create_dir_all(&source).expect("dir");
    std::fs::write(source.join("notes.md"), b"# notes\n").expect("file");
    let transcript = root.path().join("t.json");
    std::fs::write(
        &transcript,
        r#"{"note":"t","responses":[
            {"content":null,"tool_calls":[{"id":"p","name":"propose_plan","arguments":{
                "objective":"inspect the child environment","gaps":[{"id":"G1","title":"the child environment is unknown","priority":"high","why":"credentials may leak","condition":"no inspection of the env","evidence":["F1"]}],
                "tasks":[{"id":"T1","title":"dump the env","question":"what does the child see?","gap_ids":["G1"],"depends_on":[],"method":"print process.env","expected_evidence":"the environment as JSON","success_criteria":"the environment is observed","estimated_cost":"a second","risk":"none"}],
                "scope":{"allow_exec":["node"],"allow_write":false,"max_turns":8,"max_tool_calls":16,"timeout_ms":60000},
                "success_criteria":"the environment is observed"}}]},
            {"content":null,"tool_calls":[{"id":"r","name":"run_command","arguments":{"argv":["node","-e","console.log(JSON.stringify(process.env))"]}}]},
            {"content":null,"tool_calls":[{"id":"done","name":"finish","arguments":{"summary":"the environment was observed","next_steps":[],"unresolved":[]}}]}
        ]}"#,
    )
    .expect("transcript");
    let workdir = root.path().join("work");
    let out = aporia()
        .arg("agent")
        .arg(&source)
        .args(["--objective", "inspect the child environment"])
        .arg("--provider")
        .arg(format!("scripted:{}", transcript.display()))
        .args(["--allow-exec", "node"])
        .arg("--workdir")
        .arg(&workdir)
        .arg("--batch")
        .env("APORIA_ADVERSARIAL_SECRET", CANARY)
        .stdin(Stdio::null())
        .output()
        .expect("aporia runs");
    assert_eq!(out.status.code(), Some(0));
    assert!(!String::from_utf8_lossy(&out.stdout).contains(CANARY));
    assert!(!String::from_utf8_lossy(&out.stderr).contains(CANARY));
    let record = record_dir(&workdir);
    let journal = std::fs::read_to_string(record.join("journal.jsonl")).expect("journal");
    // The command really ran and dumped its environment...
    assert!(
        journal.contains("APORIA_SANDBOX"),
        "the env dump did not run"
    );
    // ...and the credential is nowhere in the record.
    scan(&record, CANARY);
}
