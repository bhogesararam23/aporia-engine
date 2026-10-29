//! The agent command as a process, against the real corpus project.
//!
//! This is the slice's end-to-end proof: a real instrument run inside a copy, a real archive, a
//! verifiable journal, an untouched source, a refusal that runs nothing, and a tamper that is caught.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

const OBJECTIVE: &str = "Determine where this model's declared r >= 0 rule stops holding, using the existing instrument, and leave a replayable record.";

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Temp(PathBuf);

impl Temp {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "aporia-cli-investigate-{}-{}-{tag}",
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
    let mut command = Command::new(env!("CARGO_BIN_EXE_aporia"));
    command.stdin(Stdio::null());
    command
}

fn corpus() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../benchmarks/aerospace/projectile_sign_mutant")
}

fn transcript() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/investigate/projectile.json")
}

fn run_demo(workdir: &Path, approve: bool) -> Output {
    let mut command = aporia();
    command
        .arg("investigate")
        .arg(corpus())
        .arg("--objective")
        .arg(OBJECTIVE)
        .arg("--provider")
        .arg(format!("scripted:{}", transcript().display()))
        .arg("--workdir")
        .arg(workdir)
        .arg("--max-turns")
        .arg("10")
        .arg("--max-tool-calls")
        .arg("20")
        .arg("--timeout")
        .arg("120000");
    if approve {
        command.arg("--approve");
    }
    command.output().expect("aporia runs")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).to_string()
}

fn records(workdir: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(workdir)
        .expect("workdir")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("inv-"))
        })
        .collect();
    found.sort();
    found
}

#[test]
fn the_real_corpus_project_completes_and_leaves_a_verifiable_record() {
    let work = Temp::new("complete");
    let model_before = std::fs::read(corpus().join("model.ap")).expect("model");
    let truth_before = std::fs::read(corpus().join("truth.json")).expect("truth");

    let out = run_demo(work.path(), true);
    let body = text(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(1),
        "stdout:\n{body}\nstderr:\n{}",
        text(&out.stderr)
    );
    assert_eq!(text(&out.stderr), "", "a successful run wrote to stderr");
    assert!(body.contains("Proposed plan"), "{body}");
    assert!(body.contains("approved"), "{body}");
    assert!(body.contains("outcome completed"), "{body}");

    let records = records(work.path());
    assert_eq!(records.len(), 1, "{records:?}");
    let record = &records[0];
    let report = std::fs::read_to_string(record.join("report.md")).expect("report");
    assert!(report.contains("scripted"), "{report}");
    assert!(report.contains("No model was consulted"), "{report}");
    assert!(report.contains("Finding 1"), "{report}");
    assert!(report.contains("workspace/archives/T1"), "{report}");
    assert!(
        report.contains("r >= 0 violated"),
        "the instrument's own report is missing"
    );
    assert!(record.join("workspace/archives/T1/manifest.json").is_file());

    let verified = aporia()
        .arg("investigate")
        .arg("report")
        .arg(record)
        .output()
        .expect("report runs");
    assert_eq!(
        verified.status.code(),
        Some(0),
        "stderr:\n{}",
        text(&verified.stderr)
    );
    assert!(text(&verified.stdout).contains("Finding 1"));
    assert_eq!(text(&verified.stderr), "");

    assert_eq!(
        std::fs::read(corpus().join("model.ap")).expect("model"),
        model_before,
        "the source model changed"
    );
    assert_eq!(
        std::fs::read(corpus().join("truth.json")).expect("truth"),
        truth_before,
        "the source truth file changed"
    );
}

#[test]
fn a_second_run_does_not_overwrite_the_first() {
    let work = Temp::new("twice");
    let first = run_demo(work.path(), true);
    assert_eq!(first.status.code(), Some(1), "{}", text(&first.stderr));
    let second = run_demo(work.path(), true);
    assert_eq!(second.status.code(), Some(1), "{}", text(&second.stderr));

    let records = records(work.path());
    assert_eq!(records.len(), 2, "{records:?}");
    for record in records {
        assert!(record.join("report.md").is_file());
        assert!(record.join("manifest.json").is_file());
    }
}

#[test]
fn a_tampered_journal_is_refused_with_integrity_status() {
    let work = Temp::new("tamper");
    let out = run_demo(work.path(), true);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
    let record = records(work.path()).remove(0);
    let journal = record.join("journal.jsonl");
    let journal_text = std::fs::read_to_string(&journal).expect("journal");
    let tampered = journal_text.replacen("investigation.started", "investigation.started!", 1);
    assert_ne!(
        tampered, journal_text,
        "the journal did not contain the event to tamper with"
    );
    std::fs::write(&journal, tampered).expect("tamper");

    let checked = aporia()
        .arg("investigate")
        .arg("report")
        .arg(&record)
        .output()
        .expect("report runs");

    assert_eq!(checked.status.code(), Some(5), "{}", text(&checked.stderr));
    assert!(
        text(&checked.stderr).contains("chain"),
        "{}",
        text(&checked.stderr)
    );
}

#[test]
fn without_approval_nothing_runs() {
    let work = Temp::new("refused");
    let out = run_demo(work.path(), false);
    let body = text(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(10),
        "stdout:\n{body}\nstderr:\n{}",
        text(&out.stderr)
    );
    assert!(body.contains("refused"), "{body}");
    assert_eq!(text(&out.stderr), "");

    let records = records(work.path());
    assert_eq!(records.len(), 1);
    assert!(
        !records[0].join("workspace/archives").exists(),
        "a refused plan executed a task"
    );
    let verified = aporia()
        .arg("investigate")
        .arg("report")
        .arg(&records[0])
        .output()
        .expect("report runs");
    assert_eq!(verified.status.code(), Some(0));
    assert!(text(&verified.stdout).contains("outcome refused"));
}

#[test]
fn missing_required_words_are_usage_errors() {
    let no_project = aporia().arg("investigate").output().expect("run");
    assert_eq!(no_project.status.code(), Some(2));
    assert!(text(&no_project.stderr).contains("project"));

    let no_provider = aporia()
        .arg("investigate")
        .arg(corpus())
        .arg("--objective")
        .arg("x")
        .output()
        .expect("run");
    assert_eq!(no_provider.status.code(), Some(2));
    assert!(text(&no_provider.stderr).contains("--provider"));

    let unknown = aporia()
        .arg("investigate")
        .arg(corpus())
        .arg("--objective")
        .arg("x")
        .arg("--provider")
        .arg("scripted:nope.json")
        .arg("--nonsense")
        .output()
        .expect("run");
    assert_eq!(unknown.status.code(), Some(2));
    assert!(text(&unknown.stderr).contains("unknown flag"));
}

#[test]
fn a_missing_transcript_is_a_provider_failure() {
    let out = aporia()
        .arg("investigate")
        .arg(corpus())
        .arg("--objective")
        .arg("x")
        .arg("--provider")
        .arg("scripted:definitely-absent.json")
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(11));
    assert!(text(&out.stderr).contains("cannot read"));
}

#[test]
fn a_missing_project_is_an_input_failure() {
    let out = aporia()
        .arg("investigate")
        .arg("definitely-absent-project")
        .arg("--objective")
        .arg("x")
        .arg("--provider")
        .arg(format!("scripted:{}", transcript().display()))
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(3));
    assert!(
        text(&out.stderr).contains("not a directory"),
        "{}",
        text(&out.stderr)
    );
}
