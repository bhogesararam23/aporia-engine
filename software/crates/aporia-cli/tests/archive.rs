//! Archiving a run and reading it back.
//!
//! The distinction these tests exist to keep is the one `aporia-store` was designed around: an archive
//! whose bytes no longer match its manifest is an *integrity* failure (status 5), and an archive that is
//! intact but whose arithmetic no longer agrees is a *reproduction* failure (status 6). Collapsing
//! them would be the interesting lie — "something is wrong with the archive" — because the second one
//! is a result about the build and the first is a result about the disk.

use std::path::{Path, PathBuf};
use std::process::Command;

const PROGRAM: &str = env!("CARGO_BIN_EXE_aporia-test-program");

fn aporia() -> Command {
    Command::new(env!("CARGO_BIN_EXE_aporia"))
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).to_string()
}

/// A scratch directory, removed first if a previous interrupted run left one behind. `std` has no
/// temp-dir helper and adding a dependency for one is not worth it; the name carries the test's own
/// label so two runs never share a directory.
fn scratch(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("aporia-cli-{}-{}", label, std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    dir
}

/// An archive of the interpreted model at a small budget, written by this test binary's own CLI.
fn archived(label: &str, model: &Path, extra: &[&str]) -> PathBuf {
    let dir = scratch(label);
    let out = aporia()
        .arg("run")
        .arg(model)
        .arg("--budget")
        .arg("40")
        .arg("--archive")
        .arg(&dir)
        .args(extra)
        .output()
        .expect("aporia runs");
    assert!(
        out.status.code() == Some(0) || out.status.code() == Some(1),
        "run failed: {}",
        text(&out.stderr)
    );
    assert!(
        text(&out.stdout).contains("archive  "),
        "no archive line in:\n{}",
        text(&out.stdout)
    );
    dir
}

#[test]
fn an_archived_run_replays_from_the_command_line() {
    let dir = archived("replay", &fixture("clean.ap"), &[]);
    let out = aporia()
        .arg("replay")
        .arg(&dir)
        .output()
        .expect("aporia runs");
    let body = text(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "stdout was:\n{body}");
    assert!(body.contains("integrity"), "{body}");
    assert!(body.contains("all match"), "{body}");
    assert!(
        body.contains("replayed 40/40 executions identically"),
        "the reproduction line is wrong:\n{body}"
    );
    assert!(body.contains("verdict  REPRODUCED"), "{body}");
}

#[test]
fn a_committed_archive_reproduces_two_versions_later() {
    // The real committed example: an archive written by the build at this commit, checked in, and
    // replayed by every test run after it. If the arithmetic moves, this fails, and it is supposed
    // to -- that is the only way "replay" means something beyond "we re-ran the same code once".
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("archive-sqrt_domain");
    assert!(
        dir.join("manifest.json").exists(),
        "the committed archive is missing from {:?}",
        dir
    );
    let out = aporia()
        .arg("replay")
        .arg(&dir)
        .output()
        .expect("aporia runs");
    let body = text(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "stdout was:\n{body}");
    assert!(body.contains("model sqrt_domain"), "{body}");
    assert!(body.contains("verdict  REPRODUCED"), "{body}");
}

#[test]
fn writing_the_same_run_twice_produces_the_same_digests() {
    // Determinism, measured on the artefact rather than asserted about the code: the manifests must
    // agree byte for byte, which they can only do if the records, the atlas table, the decisions and
    // the calibration all came out identical -- and if the timestamp field stayed out of the digests.
    let first = archived("determinism-a", &fixture("clean.ap"), &[]);
    let second = archived("determinism-b", &fixture("clean.ap"), &[]);
    let a = std::fs::read(first.join("manifest.json")).expect("manifest");
    let b = std::fs::read(second.join("manifest.json")).expect("manifest");
    assert_eq!(
        text(&a),
        text(&b),
        "two runs of one model produced different archives"
    );
    for file in [
        "observations.bin",
        "atlas.csv",
        "decisions.jsonl",
        "model.air",
    ] {
        let x = std::fs::read(first.join(file)).expect("file");
        let y = std::fs::read(second.join(file)).expect("file");
        assert_eq!(x.len(), y.len(), "{file} differs in length");
        assert_eq!(x, y, "{file} differs");
    }
}

#[test]
fn a_changed_byte_is_an_integrity_failure_not_a_reproduction_failure() {
    let dir = archived("integrity", &fixture("clean.ap"), &[]);
    let records = dir.join("observations.bin");
    let mut bytes = std::fs::read(&records).expect("the archive's records");
    let last = bytes.len() - 1;
    bytes[last] ^= 0x01;
    std::fs::write(&records, &bytes).expect("write back");

    let out = aporia()
        .arg("replay")
        .arg(&dir)
        .output()
        .expect("aporia runs");
    let body = text(&out.stdout);
    assert_eq!(out.status.code(), Some(5), "stdout was:\n{body}");
    assert!(body.contains("INTEGRITY FAILURE"), "{body}");
    // The words matter: a reader must not be sent looking for a change in the arithmetic.
    assert!(!body.contains("REPRODUCTION FAILURE"), "{body}");
    assert!(!body.contains("REPRODUCED"), "{body}");
    assert!(body.contains("observations.bin"), "{body}");
}

#[test]
fn an_intact_archive_that_no_longer_agrees_is_status_six() {
    // The constructed case, and it has to be constructed: to separate the two failures you need an
    // archive that is internally consistent -- every digest matches -- and whose stored answer is not
    // what the model now produces. So the record is edited *and* the manifest re-hashed over it,
    // which is exactly what a stale archive copied between machines can look like.
    let dir = archived("mismatch", &fixture("clean.ap"), &[]);
    let records = dir.join("observations.bin");
    let mut bytes = std::fs::read(&records).expect("the archive's records");
    // The last byte of the file is the low byte of the last stored value in the last record. Flipping
    // it moves that answer by one ulp without touching the record layout, so the archive stays
    // readable and simply disagrees with the model.
    let last = bytes.len() - 1;
    bytes[last] ^= 0x01;
    std::fs::write(&records, &bytes).expect("write back");

    // Re-digest the file inside the manifest, so integrity passes and only reproduction can fail.
    let manifest_path = dir.join("manifest.json");
    let manifest_text = std::fs::read_to_string(&manifest_path).expect("manifest");
    let digest = aporia_store::digest::sha256_hex(&bytes);
    let mut tree = aporia_store::Json::parse(&manifest_text).expect("the manifest is JSON");
    let replaced = match &mut tree {
        aporia_store::Json::Obj(fields) => match fields.iter_mut().find(|(k, _)| k == "files") {
            Some((_, aporia_store::Json::Obj(entries))) => {
                match entries
                    .iter_mut()
                    .find(|(path, _)| path == "observations.bin")
                {
                    Some((_, value)) => {
                        *value = aporia_store::Json::text(digest);
                        true
                    }
                    None => false,
                }
            }
            _ => false,
        },
        _ => false,
    };
    assert!(replaced, "the manifest did not list its records file");
    std::fs::write(&manifest_path, tree.to_pretty()).expect("write the manifest back");

    let out = aporia()
        .arg("replay")
        .arg(&dir)
        .output()
        .expect("aporia runs");
    let body = text(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(6),
        "integrity should have passed; stdout was:\n{body}"
    );
    assert!(body.contains("all match"), "integrity failed: {body}");
    assert!(body.contains("REPRODUCTION FAILURE"), "{body}");
    assert!(
        !body.contains("INTEGRITY FAILURE"),
        "a consistent archive was reported as corrupt: {body}"
    );
}

#[test]
fn an_external_archive_is_intact_but_not_replayed() {
    // A run answered by a program cannot be reproduced by a command that does not have the program,
    // and "0 mismatches" would be a pass earned by producing nothing comparable. The archive is still
    // checked and still useful -- its points, answers and calibration are auditable -- and the report
    // says which of the two happened.
    let dir = scratch("external");
    let run = aporia()
        .arg("run")
        .arg(fixture("beam.ap"))
        .arg("--program")
        .arg(PROGRAM)
        .arg("--budget")
        .arg("40")
        .arg("--archive")
        .arg(&dir)
        .output()
        .expect("aporia runs");
    assert!(
        run.status.code() == Some(1),
        "the external run should have found the region: {}",
        text(&run.stderr)
    );
    let out = aporia()
        .arg("replay")
        .arg(&dir)
        .output()
        .expect("aporia runs");
    let body = text(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "stdout was:\n{body}");
    assert!(body.contains("not attempted"), "{body}");
    assert!(body.contains("INTACT"), "{body}");
    assert!(
        !body.contains("REPRODUCED"),
        "the report claimed a reproduction it did not perform:\n{body}"
    );
}

#[test]
fn an_archive_directory_that_does_not_exist_is_input_error() {
    let out = aporia()
        .arg("replay")
        .arg("no-such-archive-directory")
        .output()
        .expect("aporia runs");
    assert_eq!(out.status.code(), Some(3));
    assert!(text(&out.stderr).contains("cannot read the archive"));
}

#[test]
fn replay_without_an_argument_says_what_it_needs() {
    let out = aporia().arg("replay").output().expect("aporia runs");
    assert_eq!(out.status.code(), Some(2));
    assert!(text(&out.stderr).contains("replay needs an archive directory"));
}
