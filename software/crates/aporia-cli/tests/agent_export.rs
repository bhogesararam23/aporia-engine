//! The export verb through the real application: a person authorises the write, the patch
//! round-trips through git, the record stays verifiable, and a refusal writes nothing.

use aporia_cli::agent;
use aporia_cli::run::Exit;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Temp(PathBuf);

impl Temp {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "aporia-cli-export-{tag}-{}-{}",
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

fn aporia() -> Command {
    Command::new(env!("CARGO_BIN_EXE_aporia"))
}

fn project(tag: &str) -> Temp {
    let source = Temp::new(tag);
    fs::write(source.path().join("rule.txt"), b"original line\n").expect("file");
    source
}

const PLAN: &str = r#"{"objective":"improve the rule","gaps":[{"id":"G1","title":"the rule is wrong","priority":"high","why":"it says original","condition":"no test checks it","evidence":["F1"]}],"tasks":[{"id":"T1","title":"fix the rule","question":"does the fix hold?","gap_ids":["G1"],"depends_on":[],"method":"edit","expected_evidence":"a patch","success_criteria":"the patch applies","estimated_cost":"a second","risk":"none"}],"scope":{"allow_exec":[],"allow_write":true,"max_turns":8,"max_tool_calls":16,"timeout_ms":60000},"success_criteria":"the change is exportable"}"#;

fn transcript_with_change(path: &Path) {
    let responses = format!(
        r#"[
        {{"content":null,"tool_calls":[{{"id":"call-plan","name":"propose_plan","arguments":{PLAN}}}] }},
        {{"content":null,"tool_calls":[{{"id":"call-write","name":"write_file","arguments":{{"path":"rule.txt","content":"fixed line\n"}}}}] }},
        {{"content":null,"tool_calls":[{{"id":"call-finish","name":"finish","arguments":{{"summary":"done","next_steps":[],"unresolved":[]}}}}] }}
    ]"#
    );
    fs::write(
        path,
        format!("{{\"note\":\"test\",\"responses\":{responses}}}"),
    )
    .expect("transcript");
}

fn finish_only(path: &Path) {
    fs::write(
        path,
        r#"{"note":"test","responses":[{"content":null,"tool_calls":[{"id":"call-finish","name":"finish","arguments":{"summary":"done","next_steps":[],"unresolved":[]}}]}]}"#,
    )
    .expect("transcript");
}

/// Run a batch investigation to completion and return its record directory.
fn finished_record(source: &Temp, workdir: &Temp, transcript: &Path) -> PathBuf {
    let flags: Vec<String> = vec![
        source.path().display().to_string(),
        "--objective".to_string(),
        "improve the rule".to_string(),
        "--provider".to_string(),
        format!("scripted:{}", transcript.display()),
        "--allow-write".to_string(),
        "--workdir".to_string(),
        workdir.path().display().to_string(),
        "--batch".to_string(),
    ];
    let mut out: Vec<u8> = Vec::new();
    let mut err: Vec<u8> = Vec::new();
    let exit = agent::command(&flags, &mut out, &mut err);
    assert!(
        matches!(exit, Exit::Clean | Exit::Suspicious),
        "stderr: {}",
        String::from_utf8_lossy(&err)
    );
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
fn an_authorised_export_round_trips_and_keeps_the_record_verifiable() {
    let source = project("src1");
    let workdir = Temp::new("work1");
    let transcript = workdir.path().join("t.json");
    transcript_with_change(&transcript);
    let root = finished_record(&source, &workdir, &transcript);

    let out_dir = Temp::new("out1");
    let patch_path = out_dir.path().join("change.patch");
    let mut out: Vec<u8> = Vec::new();
    let mut err: Vec<u8> = Vec::new();
    let exit = agent::command(
        &[
            "export".to_string(),
            root.display().to_string(),
            "--out".to_string(),
            patch_path.display().to_string(),
            "--yes".to_string(),
        ],
        &mut out,
        &mut err,
    );
    assert_eq!(
        exit,
        Exit::Clean,
        "stderr: {}",
        String::from_utf8_lossy(&err)
    );
    let patch = fs::read_to_string(&patch_path).expect("patch");
    assert!(patch.contains("-original line"), "{patch}");
    assert!(patch.contains("+fixed line"), "{patch}");

    // The record still verifies: the manifest was refreshed to the new head.
    aporia_agent::verify_record(&root).expect("the record verifies after export");
    let journal = fs::read_to_string(root.join("journal.jsonl")).expect("journal");
    assert!(
        journal.contains("\"kind\":\"changeset.exported\""),
        "{journal}"
    );
    assert!(journal.contains("\"by\":\"cli\""), "{journal}");

    // The patch applies to a pristine copy and reproduces the workspace bytes.
    if Command::new("git").arg("--version").output().is_ok() {
        let target = out_dir.path().join("target");
        fs::create_dir_all(&target).expect("dir");
        fs::copy(source.path().join("rule.txt"), target.join("rule.txt")).expect("copy");
        let applied = Command::new("git")
            .args(["-c", "core.autocrlf=false", "apply"])
            .arg(&patch_path)
            .current_dir(&target)
            .output()
            .expect("git apply");
        assert!(
            applied.status.success(),
            "git apply: {}",
            String::from_utf8_lossy(&applied.stderr)
        );
        assert_eq!(
            fs::read(target.join("rule.txt")).expect("file"),
            fs::read(root.join("workspace/rule.txt")).expect("file"),
            "the exported patch did not reproduce the workspace"
        );
    }
}

#[test]
fn a_refused_export_writes_nothing_and_journals_nothing() {
    let source = project("src2");
    let workdir = Temp::new("work2");
    let transcript = workdir.path().join("t.json");
    transcript_with_change(&transcript);
    let root = finished_record(&source, &workdir, &transcript);

    let out_dir = Temp::new("out2");
    let patch_path = out_dir.path().join("change.patch");
    let mut child = aporia()
        .args(["agent", "export"])
        .arg(&root)
        .arg("--out")
        .arg(&patch_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(b"n\n")
        .expect("answer");
    drop(child.stdin.take());
    let out = child.wait_with_output().expect("wait");
    assert_eq!(out.status.code(), Some(0));
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("refused"),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        !patch_path.exists(),
        "the patch was written without approval"
    );
    let journal = fs::read_to_string(root.join("journal.jsonl")).expect("journal");
    assert!(
        !journal.contains("\"kind\":\"changeset.exported\""),
        "{journal}"
    );
}

#[test]
fn a_record_without_changes_says_there_is_nothing_to_export() {
    let source = project("src3");
    let workdir = Temp::new("work3");
    let transcript = workdir.path().join("t.json");
    finish_only(&transcript);
    let root = finished_record(&source, &workdir, &transcript);
    let out_dir = Temp::new("out3");
    let mut out: Vec<u8> = Vec::new();
    let mut err: Vec<u8> = Vec::new();
    let exit = agent::command(
        &[
            "export".to_string(),
            root.display().to_string(),
            "--out".to_string(),
            out_dir.path().join("p.patch").display().to_string(),
            "--yes".to_string(),
        ],
        &mut out,
        &mut err,
    );
    assert_eq!(exit, Exit::Clean);
    assert!(
        String::from_utf8_lossy(&out).contains("nothing to export"),
        "{}",
        String::from_utf8_lossy(&out)
    );
}

#[test]
fn a_destination_inside_the_source_is_refused() {
    let source = project("src4");
    let workdir = Temp::new("work4");
    let transcript = workdir.path().join("t.json");
    transcript_with_change(&transcript);
    let root = finished_record(&source, &workdir, &transcript);
    let inside = source.path().join("p.patch");
    let mut out: Vec<u8> = Vec::new();
    let mut err: Vec<u8> = Vec::new();
    let exit = agent::command(
        &[
            "export".to_string(),
            root.display().to_string(),
            "--out".to_string(),
            inside.display().to_string(),
            "--yes".to_string(),
        ],
        &mut out,
        &mut err,
    );
    assert_eq!(exit, Exit::Usage);
    assert!(!inside.exists(), "the source was written");
}
