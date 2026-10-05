//! `aporia report` over a stored run.
//!
//! The property these tests hold the command to: nothing is recomputed. A report that re-derived a
//! number would quietly disagree with the archive it claims to describe, which is the failure mode the
//! whole `report`/`replay` split exists to avoid.

use std::path::PathBuf;
use std::process::Command;

fn aporia() -> Command {
    Command::new(env!("CARGO_BIN_EXE_aporia"))
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).to_string()
}

#[test]
fn a_stored_run_reports_without_executing_anything() {
    let out = aporia()
        .arg("report")
        .arg(fixture("archive-sqrt_domain"))
        .output()
        .expect("aporia runs");
    let body = text(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "stdout was:\n{body}");
    assert!(body.contains("model sqrt_domain"), "{body}");
    assert!(body.contains("budget=40"), "config not read back:\n{body}");
    assert!(
        body.contains("40 evaluations  40 instruction steps"),
        "counts not read back:\n{body}"
    );
    assert!(
        body.contains("integrity  8 file(s) digested against the manifest: all match"),
        "{body}"
    );
    assert!(body.contains("APORIA FINDING #000000"), "{body}");
    // The finding block carries the archived rule text, not a fresh evaluation of the model.
    assert!(body.contains("y is NaN"), "{body}");
    assert!(body.contains("not recomputed"), "{body}");
}

#[test]
fn report_and_replay_disagree_only_about_reproducing() {
    // Same archive, two questions: the report says nothing about whether the arithmetic still agrees,
    // and the replay says nothing about the narrative. If these two outputs ever converge, one of the
    // commands has stopped minding its own business.
    let report = aporia()
        .arg("report")
        .arg(fixture("archive-sqrt_domain"))
        .output()
        .expect("report runs");
    let replay = aporia()
        .arg("replay")
        .arg(fixture("archive-sqrt_domain"))
        .output()
        .expect("replay runs");
    let report_body = text(&report.stdout);
    let replay_body = text(&replay.stdout);
    assert!(!report_body.contains("replayed"), "{report_body}");
    assert!(replay_body.contains("replayed"), "{replay_body}");
    assert!(!replay_body.contains("APORIA FINDING"), "{replay_body}");
    assert!(report_body.contains("APORIA FINDING"), "{report_body}");
}

#[test]
fn a_corrupt_archive_is_refused_by_the_report_too() {
    let dir = fixture("archive-sqrt_domain");
    let target =
        std::env::temp_dir().join(format!("aporia-cli-report-corrupt-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&target);
    copy_dir(&dir, &target);
    let bands = target.join("bands.csv");
    let mut bytes = std::fs::read(&bands).expect("bands file");
    bytes.push(b'#');
    std::fs::write(&bands, &bytes).expect("write back");

    let out = aporia()
        .arg("report")
        .arg(&target)
        .output()
        .expect("aporia runs");
    let body = text(&out.stdout);
    assert_eq!(out.status.code(), Some(5), "stdout was:\n{body}");
    assert!(body.contains("integrity"), "{body}");
    assert!(body.contains("problem"), "{body}");
    let _ = std::fs::remove_dir_all(&target);
}

#[test]
fn report_without_an_argument_names_the_command() {
    let out = aporia().arg("report").output().expect("aporia runs");
    assert_eq!(out.status.code(), Some(2));
    let err = text(&out.stderr);
    assert!(err.contains("report needs an archive directory"), "{err}");
    // And the same shape as `replay`, because both take one directory: no separate error wording to
    // drift out of sync.
    assert!(err.contains("run-0001"), "{err}");
}

/// A minimal recursive copy, because the corrupt-archive test must not edit the committed fixture.
fn copy_dir(from: &PathBuf, to: &PathBuf) {
    std::fs::create_dir_all(to).expect("target directory");
    for entry in std::fs::read_dir(from).expect("read source") {
        let entry = entry.expect("entry");
        let path = entry.path();
        let target = to.join(entry.file_name());
        if path.is_dir() {
            copy_dir(&path, &target);
        } else {
            std::fs::copy(&path, &target).expect("copy file");
        }
    }
}
