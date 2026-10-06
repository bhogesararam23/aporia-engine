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
fn the_report_says_who_computed_the_numbers_and_from_which_source() {
    // The archive has always held its environment notes and, since the toolchain and source fields
    // were added, the compiler and the tree the numbers came from. A reader of `report` is the person
    // deciding whether to trust a quoted number, so the line belongs here rather than in a manifest
    // they have to open by hand — and what it prints has to be what the archive holds, including
    // where the archive holds nothing.
    let dir = fixture("archive-sqrt_domain");
    let out = aporia()
        .arg("report")
        .arg(&dir)
        .output()
        .expect("aporia runs");
    let body = text(&out.stdout);
    let manifest =
        std::fs::read_to_string(dir.join("manifest.json")).expect("the manifest is there");
    let line = body
        .lines()
        .find(|l| l.starts_with("build  "))
        .unwrap_or_else(|| panic!("no build line in:\n{body}"));
    assert!(
        line.contains("os=windows") || line.contains("os=linux"),
        "{line}"
    );
    assert!(line.contains("arch="), "{line}");
    // report repeats what the archive says — including a field this fixture predates.
    let quoted = |key: &str| -> Option<String> {
        let at = manifest.find(&format!("\"{key}\""))? + key.len() + 2;
        let rest = &manifest[at..];
        let open = rest.find('"')?;
        let rest = &rest[open + 1..];
        let close = rest.find('"')?;
        Some(rest[..close].to_string())
    };
    match quoted("rust_channel").as_deref() {
        Some("") | None => assert!(!line.contains("toolchain="), "{line}"),
        Some(value) => assert!(line.contains(&format!("toolchain={value}")), "{line}"),
    }
    match quoted("source_commit").as_deref() {
        Some("") | None => assert!(!line.contains("source="), "{line}"),
        Some(value) => assert!(line.contains(&format!("source={value}")), "{line}"),
    }
    // And the note that answers the question this line exists for: who did the arithmetic.
    assert!(
        line.contains("execution=scalar interpreter"),
        "the archive's own execution note was not printed: {line}"
    );
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
