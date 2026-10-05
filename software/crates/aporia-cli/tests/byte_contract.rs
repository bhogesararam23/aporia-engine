//! A committed archive is a contract about bytes, and a checkout is allowed to break it silently.
//!
//! An archive's manifest digests every artefact it holds. That makes the archive's identity a fact
//! about its exact bytes — and `core.autocrlf` is a setting that rewrites bytes on checkout, per file,
//! whenever `.gitattributes` does not say otherwise. In this repository that happened: `decisions.jsonl`
//! and `findings/*.apx` had no rule, so a fresh clone rewrote their line endings and every committed
//! archive failed its own integrity check. No test in the working tree could see it, because the
//! working tree is where those files had been written.
//!
//! So this test asks git what a checkout *would put on disk*, and digests that. It is the same question
//! a reader on another machine asks, and it is answered without trusting the files sitting next to it.
//!
//! When git is not available — a source tarball, a container with no git — the check cannot be made,
//! and it says so on stderr rather than passing quietly. A green result from a test that did not run is
//! exactly the kind of pass this project refuses elsewhere: an archive re-executed by a program the
//! caller does not have is reported as *not replayed*, not as "0 mismatches".

use aporia_store::{Loaded, Manifest, digest::sha256_hex};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The crate's own committed archive fixtures.
fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Ask git for the bytes a checkout of `rel` would produce, filters and all.
///
/// `--filters` is the point: without it `cat-file` returns the blob as stored, which is LF-clean even
/// in a repository that hands its readers CRLF. The question this test has to answer is what arrives on
/// disk, not what is in the object database.
fn checkout_bytes(repo: &Path, rel: &str) -> Result<Vec<u8>, String> {
    let out = Command::new("git")
        .current_dir(repo)
        .arg("cat-file")
        .arg("--filters")
        .arg(format!("HEAD:{rel}"))
        .output()
        .map_err(|e| format!("git cat-file: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git has no HEAD version of {rel}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(out.stdout)
}

/// Every committed archive directory, by the manifest that makes it an archive.
fn archives() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(fixtures())
        .expect("the fixture directory is part of the repository")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir() && p.join("manifest.json").is_file())
        .collect();
    out.sort();
    out
}

fn repo_root() -> Option<PathBuf> {
    let out = Command::new("git")
        .current_dir(fixtures())
        .arg("rev-parse")
        .arg("--show-toplevel")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(PathBuf::from(String::from_utf8_lossy(&out.stdout).trim()))
}

#[test]
fn a_checked_out_archive_still_matches_its_own_manifest() {
    let Some(repo) = repo_root() else {
        eprintln!(
            "SKIP: this checkout has no git metadata, so the bytes a fresh clone would receive \
             cannot be asked for. The archives' own digests are unverified by this run."
        );
        return;
    };
    let mut problems = Vec::new();
    let mut checked = 0usize;
    for dir in archives() {
        // The manifest is read from the working tree, because it is the promise; the artefacts are
        // read from what git says a checkout would write, because that is whether the promise holds.
        let text =
            std::fs::read_to_string(dir.join("manifest.json")).expect("a committed manifest");
        let manifest = Manifest::from_json(&aporia_store::Json::parse(&text).expect("valid JSON"))
            .unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
        let rel_dir = dir
            .strip_prefix(&repo)
            .unwrap_or_else(|_| panic!("{} is not inside the repository", dir.display()))
            .to_string_lossy()
            .replace('\\', "/");
        for (name, expected) in &manifest.files {
            let rel = format!("{rel_dir}/{name}");
            match checkout_bytes(&repo, &rel) {
                Ok(bytes) => {
                    checked += 1;
                    let actual = sha256_hex(&bytes);
                    if &actual != expected {
                        problems.push(format!(
                            "{rel}: a checkout writes bytes digesting to {actual}, the manifest \
                             promises {expected}"
                        ));
                    }
                }
                Err(e) => problems.push(e),
            }
        }
    }
    assert!(
        checked > 0,
        "no archive artefact was checked, so this test proved nothing"
    );
    assert!(
        problems.is_empty(),
        "{} of {checked} archived artefacts do not survive a checkout:\n{}",
        problems.len(),
        problems.join("\n")
    );
}

#[test]
fn an_archive_reassembled_from_gits_own_bytes_opens_and_passes_its_integrity_check() {
    // The stronger claim, and the one a reader actually makes: not "the digests match" but "the
    // command that grades an archive accepts the copy git would have handed me". This writes the
    // checkout's bytes into a scratch directory, in the layout an archive has, and opens it.
    let Some(repo) = repo_root() else {
        eprintln!("SKIP: no git metadata, so there is no checkout to read an archive from.");
        return;
    };
    for dir in archives() {
        let text =
            std::fs::read_to_string(dir.join("manifest.json")).expect("a committed manifest");
        let manifest = Manifest::from_json(&aporia_store::Json::parse(&text).expect("valid JSON"))
            .unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
        let rel_dir = dir
            .strip_prefix(&repo)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let scratch = std::env::temp_dir().join(format!(
            "aporia-checkout-{}-{}",
            dir.file_name().unwrap().to_string_lossy(),
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&scratch);
        // The artefacts the manifest digests, plus the manifest itself: an archive is the directory.
        let mut names: Vec<String> = manifest.files.iter().map(|(n, _)| n.clone()).collect();
        names.push("manifest.json".to_string());
        for name in &names {
            let rel = format!("{rel_dir}/{name}");
            let bytes = checkout_bytes(&repo, &rel).unwrap_or_else(|e| panic!("{rel}: {e}"));
            let target = scratch.join(name);
            std::fs::create_dir_all(target.parent().expect("an artefact sits in a directory"))
                .expect("scratch directories");
            std::fs::write(&target, bytes).expect("writing a checked-out artefact");
        }
        let loaded = Loaded::open(&scratch)
            .unwrap_or_else(|e| panic!("{} as checked out by git: {e}", dir.display()));
        let problems = loaded.integrity_problems();
        let _ = std::fs::remove_dir_all(&scratch);
        assert!(
            problems.is_empty(),
            "{} fails its own integrity check when read from git's checkout bytes:\n{}",
            dir.display(),
            problems.join("\n")
        );
    }
}

/// The failure mode this file exists for, demonstrated rather than described: a text artefact written
/// with CRLF is a different digest, and the only thing between that and a silently broken clone is the
/// rule in `.gitattributes`.
#[test]
fn a_line_ending_change_is_a_different_archive() {
    let lf = "{\"evaluation\":0}\n{\"evaluation\":1}\n";
    let crlf = lf.replace('\n', "\r\n");
    assert_ne!(
        sha256_hex(lf.as_bytes()),
        sha256_hex(crlf.as_bytes()),
        "the digest does not care how a file is broken into lines, so nothing else will notice"
    );
}
