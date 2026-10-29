//! A record is complete only when the manifest is last, and it is trusted only when every byte
//! still agrees with it.

use aporia_agent::journal::RecordError;
use aporia_agent::record::{RecordDir, verify_record};
use aporia_store::Json;
use aporia_store::digest::sha256_hex;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Temp(PathBuf);

impl Temp {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "aporia-agent-record-{}-{}-{tag}",
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

fn manifest(head: &str, events: u64, sandbox: Option<&str>) -> Json {
    Json::object(vec![
        ("schema", Json::count(1)),
        ("outcome", Json::text("completed")),
        ("provider", Json::text("scripted transcript (in memory)")),
        (
            "journal",
            Json::object(vec![
                ("events", Json::count(events)),
                ("head", Json::text(head)),
            ]),
        ),
        ("sandbox_sha256", sandbox.map_or(Json::Null, Json::text)),
    ])
}

fn finished(temp: &Temp) -> RecordDir {
    let mut record = RecordDir::create(temp.path()).expect("record");
    for index in 0..3 {
        record
            .journal_mut()
            .append(
                "investigation.step",
                Json::object(vec![("index", Json::count(index))]),
            )
            .expect("append");
    }
    record.write_text("sandbox.json", "{}").expect("sandbox");
    let sandbox = sha256_hex(b"{}");
    let (head, events) = {
        let journal = record.journal_mut();
        (journal.head().to_string(), journal.events())
    };
    record
        .finish(&manifest(&head, events, Some(&sandbox)))
        .expect("finish");
    record
}

#[test]
fn two_investigations_in_the_same_second_do_not_collide() {
    let temp = Temp::new("names");
    let first = RecordDir::create(temp.path()).expect("first");
    let second = RecordDir::create(temp.path()).expect("second");
    assert_ne!(first.root(), second.root());
    assert!(first.root().is_dir());
    assert!(second.root().is_dir());
}

#[test]
fn a_finished_record_verifies_and_reports_its_outcome() {
    let temp = Temp::new("good");
    let record = finished(&temp);

    let summary = verify_record(record.root()).expect("verify");

    assert_eq!(summary.outcome, "completed");
    assert_eq!(summary.provider, "scripted transcript (in memory)");
    assert_eq!(summary.events, 3);
    assert_eq!(summary.head.len(), 64);
}

#[test]
fn a_middle_edit_breaks_the_chain_with_the_line_number() {
    let temp = Temp::new("middle");
    let record = finished(&temp);
    let path = record.root().join("journal.jsonl");
    let text = std::fs::read_to_string(&path).expect("journal");
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    lines[0] = lines[0].replace("investigation.step", "investigation.stepped");
    std::fs::write(&path, lines.join("\n") + "\n").expect("tamper");

    let error = verify_record(record.root()).expect_err("must fail");

    assert!(
        matches!(error, RecordError::Chain { line: 1, .. }),
        "{error:?}"
    );
}

#[test]
fn an_edit_to_the_last_line_is_caught_by_the_manifest_head() {
    let temp = Temp::new("last");
    let record = finished(&temp);
    let path = record.root().join("journal.jsonl");
    let text = std::fs::read_to_string(&path).expect("journal");
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    lines[2] = lines[2].replace("investigation.step", "investigation.step!");
    std::fs::write(&path, lines.join("\n") + "\n").expect("tamper");

    let error = verify_record(record.root()).expect_err("must fail");

    assert!(matches!(error, RecordError::Head { .. }), "{error:?}");
}

#[test]
fn a_record_without_a_manifest_is_not_finalised() {
    let temp = Temp::new("unfinished");
    let mut record = RecordDir::create(temp.path()).expect("record");
    record
        .journal_mut()
        .append("investigation.started", Json::Null)
        .expect("append");

    let error = verify_record(record.root()).expect_err("must fail");

    assert!(matches!(error, RecordError::NotFinalised(_)), "{error:?}");
}

#[test]
fn a_mismatched_sandbox_digest_is_caught() {
    let temp = Temp::new("sandbox");
    let mut record = RecordDir::create(temp.path()).expect("record");
    record.write_text("sandbox.json", "{}").expect("sandbox");
    let (head, events) = {
        let journal = record.journal_mut();
        (journal.head().to_string(), journal.events())
    };
    let wrong = sha256_hex(b"[]");
    record
        .finish(&manifest(&head, events, Some(&wrong)))
        .expect("finish");

    let error = verify_record(record.root()).expect_err("must fail");

    assert!(
        matches!(error, RecordError::SandboxDigest { .. }),
        "{error:?}"
    );
}

#[test]
fn the_event_count_must_match_the_journal() {
    let temp = Temp::new("count");
    let mut record = RecordDir::create(temp.path()).expect("record");
    record
        .journal_mut()
        .append("investigation.started", Json::Null)
        .expect("append");
    let head = record.journal_mut().head().to_string();
    record.finish(&manifest(&head, 9, None)).expect("finish");

    let error = verify_record(record.root()).expect_err("must fail");

    assert!(matches!(error, RecordError::Events { .. }), "{error:?}");
}

#[test]
fn an_artefact_name_cannot_escape_the_record() {
    let temp = Temp::new("name");
    let record = RecordDir::create(temp.path()).expect("record");
    let error = record
        .write_text("../escape.txt", "x")
        .expect_err("must fail");
    assert!(matches!(error, RecordError::Io { .. }), "{error:?}");
}
