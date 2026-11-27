//! The resume verbs through the real binary: an interrupted session continues from its journal,
//! a paused one resumes, a finished one refuses, and a broken record is a named refusal.

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
            "aporia-cli-resume-{tag}-{}-{}",
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
    fs::write(
        source.path().join("notes.md"),
        b"# notes\nthe rule may be biased\n",
    )
    .expect("file");
    source
}

fn transcript(path: &Path, responses: &str) {
    fs::write(
        path,
        format!("{{\"note\":\"test\",\"responses\":{responses}}}"),
    )
    .expect("transcript");
}

const READ_AND_FINISH: &str = r#"[
    {"content":null,"tool_calls":[{"id":"call-read","name":"read_file","arguments":{"path":"notes.md"}}]},
    {"content":null,"tool_calls":[{"id":"call-finish","name":"finish","arguments":{"summary":"done","next_steps":[],"unresolved":[]}}]}
]"#;

const FINISH_ONLY: &str = r#"[
    {"content":null,"tool_calls":[{"id":"call-finish","name":"finish","arguments":{"summary":"done","next_steps":[],"unresolved":[]}}]}
]"#;

fn record_dir(workdir: &Path) -> PathBuf {
    let mut records: Vec<PathBuf> = fs::read_dir(workdir)
        .expect("records")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    records.sort();
    assert_eq!(records.len(), 1, "one investigation directory");
    records.remove(0)
}

/// Start a session whose stdin is closed: the first turn runs, then the conversation ends at EOF
/// and the process leaves the record open -- exactly what a dead terminal leaves behind.
fn start_open_session(source: &Temp, workdir: &Temp, t1: &Path) -> PathBuf {
    let out = aporia()
        .arg("agent")
        .arg(source.path())
        .args(["--objective", "find out whether the rule is biased"])
        .arg("--provider")
        .arg(format!("scripted:{}", t1.display()))
        .arg("--workdir")
        .arg(workdir.path())
        .stdin(Stdio::null())
        .output()
        .expect("aporia runs");
    assert_eq!(
        out.status.code(),
        Some(0),
        "stdout: {} stderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    record_dir(workdir.path())
}

#[test]
fn an_open_session_resumes_from_the_cli_and_finishes() {
    let source = project("src1");
    let workdir = Temp::new("work1");
    let t1 = workdir.path().join("t1.json");
    let t2 = workdir.path().join("t2.json");
    transcript(&t1, READ_AND_FINISH);
    transcript(&t2, FINISH_ONLY);
    let root = start_open_session(&source, &workdir, &t1);

    let status = aporia()
        .args(["agent", "status"])
        .arg(&root)
        .output()
        .expect("status runs");
    assert_eq!(status.status.code(), Some(0));
    let body = String::from_utf8_lossy(&status.stdout);
    assert!(body.contains("status active"), "{body}");
    assert!(body.contains("resumable"), "{body}");

    let out = aporia()
        .args(["agent", "resume"])
        .arg(&root)
        .arg("--provider")
        .arg(format!("scripted:{}", t2.display()))
        .arg("--batch")
        .stdin(Stdio::null())
        .output()
        .expect("resume runs");
    assert_eq!(
        out.status.code(),
        Some(0),
        "stdout: {} stderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let journal = fs::read_to_string(root.join("journal.jsonl")).expect("journal");
    assert!(
        journal.contains("\"kind\":\"session.resumed\""),
        "{journal}"
    );
    assert_eq!(
        journal
            .lines()
            .filter(|line| line.contains("\"kind\":\"tool.started\"") && line.contains("call-read"))
            .count(),
        1,
        "the completed read ran again"
    );
    let manifest = fs::read_to_string(root.join("manifest.json")).expect("manifest");
    assert!(
        manifest.contains("\"outcome\": \"completed\""),
        "{manifest}"
    );
}

#[test]
fn pause_from_the_conversation_leaves_a_resumable_record() {
    let source = project("src2");
    let workdir = Temp::new("work2");
    let t1 = workdir.path().join("t1.json");
    let t2 = workdir.path().join("t2.json");
    transcript(&t1, READ_AND_FINISH);
    transcript(&t2, FINISH_ONLY);

    let mut child = aporia()
        .arg("agent")
        .arg(source.path())
        .args(["--objective", "find out whether the rule is biased"])
        .arg("--provider")
        .arg(format!("scripted:{}", t1.display()))
        .arg("--workdir")
        .arg(workdir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(b"/pause\n")
        .expect("write pause");
    drop(child.stdin.take());
    let out = child.wait_with_output().expect("wait");
    assert_eq!(
        out.status.code(),
        Some(0),
        "stdout: {} stderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let root = record_dir(workdir.path());
    let state = aporia_agent::read_checkpoint(&root)
        .expect("read")
        .expect("checkpoint");
    assert_eq!(state.status, aporia_agent::SessionStatus::Paused);

    let out = aporia()
        .args(["agent", "resume"])
        .arg(&root)
        .arg("--provider")
        .arg(format!("scripted:{}", t2.display()))
        .arg("--batch")
        .stdin(Stdio::null())
        .output()
        .expect("resume runs");
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let manifest = fs::read_to_string(root.join("manifest.json")).expect("manifest");
    assert!(
        manifest.contains("\"outcome\": \"completed\""),
        "{manifest}"
    );
}

#[test]
fn a_finished_record_refuses_resume() {
    let source = project("src3");
    let workdir = Temp::new("work3");
    let t1 = workdir.path().join("t1.json");
    transcript(&t1, FINISH_ONLY);
    let out = aporia()
        .arg("agent")
        .arg(source.path())
        .args(["--objective", "find out whether the rule is biased"])
        .arg("--provider")
        .arg(format!("scripted:{}", t1.display()))
        .arg("--workdir")
        .arg(workdir.path())
        .arg("--batch")
        .stdin(Stdio::null())
        .output()
        .expect("aporia runs");
    assert_eq!(out.status.code(), Some(0));
    let root = record_dir(workdir.path());

    let out = aporia()
        .args(["agent", "resume"])
        .arg(&root)
        .arg("--provider")
        .arg(format!("scripted:{}", t1.display()))
        .arg("--batch")
        .stdin(Stdio::null())
        .output()
        .expect("resume runs");
    assert_eq!(out.status.code(), Some(3));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("finished"), "{err}");
}

#[test]
fn a_corrupt_journal_refuses_resume_with_the_integrity_status() {
    let source = project("src4");
    let workdir = Temp::new("work4");
    let t1 = workdir.path().join("t1.json");
    transcript(&t1, READ_AND_FINISH);
    let root = start_open_session(&source, &workdir, &t1);

    let path = root.join("journal.jsonl");
    let mut text = fs::read_to_string(&path).expect("journal");
    let middle = text.len() / 2;
    let byte = text.as_bytes()[middle];
    text.replace_range(middle..=middle, if byte == b'x' { "y" } else { "x" });
    fs::write(&path, text).expect("corrupt");

    let out = aporia()
        .args(["agent", "resume"])
        .arg(&root)
        .arg("--provider")
        .arg(format!("scripted:{}", t1.display()))
        .arg("--batch")
        .stdin(Stdio::null())
        .output()
        .expect("resume runs");
    assert_eq!(out.status.code(), Some(5));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("chain") || err.contains("not an event"),
        "{err}"
    );
}
