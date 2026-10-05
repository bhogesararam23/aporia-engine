//! Reading an archive back: is it intact, and does the run reproduce?
//!
//! Two questions, deliberately separate answers. `aporia-store` digests every artefact against the
//! manifest, so a file that no longer matches is an *integrity* problem — the archive is not the one
//! that was written. Re-executing the archived A-IR at the archived points and comparing bit patterns
//! is *reproduction* — the same archive, but the arithmetic no longer agrees. A corrupted file and a
//! changed interpreter look nothing alike as actions to take, so they do not share an exit status, and
//! the second is the one that would otherwise hide inside the first as "something is wrong with the
//! archive".
//!
//! A run executed by an external program is reported honestly: its integrity is checked, and
//! reproduction is *not attempted*, because re-running a program APORIA does not have is not the same
//! experiment. The archive from `--program` still records the points and the answers, so it is
//! auditable; it is not re-executable by this command, and saying "0 mismatches" while quietly
//! producing NaNs would be the worst kind of pass.

use aporia_ir::Model;
use aporia_runtime::value::{ExecConfig, FpMode};
use aporia_store::{Loaded, digest::sha256_hex, replay};

use crate::run::Exit;

/// `aporia replay <archive-dir>`.
pub fn command(dir: &std::path::Path, out: &mut impl std::io::Write) -> Exit {
    let loaded = match Loaded::open(dir) {
        Ok(loaded) => loaded,
        Err(e) => {
            eprintln!(
                "aporia: cannot read the archive at `{}`: {e}",
                dir.display()
            );
            return Exit::Input;
        }
    };
    let model: Model = match aporia_ir::from_text(&loaded.air_text) {
        Ok(model) => model,
        Err(e) => {
            eprintln!("aporia: the archived A-IR does not parse: {e}");
            return Exit::Integrity;
        }
    };
    // Integrity first, because a broken archive makes any statement about its contents untrue.
    let integrity = loaded.integrity_problems();
    let _ = writeln!(
        out,
        "archive  {}  model {}  {} executions",
        dir.display(),
        loaded.manifest.model_name,
        loaded.records.len()
    );
    let _ = writeln!(
        out,
        "integrity  {} file(s) digested against the manifest: {}",
        loaded.manifest.files.len(),
        if integrity.is_empty() {
            "all match".to_string()
        } else {
            format!("{} problem(s)", integrity.len())
        }
    );
    if !integrity.is_empty() {
        for problem in &integrity {
            let _ = writeln!(out, "  {problem}");
        }
        let _ = writeln!(
            out,
            "verdict  INTEGRITY FAILURE: this is not the archive that was written"
        );
        return Exit::Integrity;
    }
    if aporia_runtime::needs_adapter(&model) {
        let _ = writeln!(
            out,
            "reproduction  not attempted: this run was executed by a program, and re-running a \
             program this command does not have is not the same experiment"
        );
        let _ = writeln!(
            out,
            "verdict  INTACT (the archive's own digests and records were checked)"
        );
        return Exit::Clean;
    }
    let cfg = ExecConfig {
        fp: if loaded.manifest.exec_fp == "f32" {
            FpMode::F32
        } else {
            FpMode::F64
        },
        max_steps: loaded.manifest.exec_max_steps,
    };
    let result = replay::replay(&model, &loaded.records, cfg);
    let _ = writeln!(out, "reproduction  {}", one_line(&result.summary()));
    for mismatch in result.mismatches.iter().take(5) {
        let _ = writeln!(
            out,
            "  execution {} {}: expected {}, found {}",
            mismatch.observation, mismatch.field, mismatch.expected, mismatch.found
        );
    }
    if result.mismatches.len() > 5 {
        let _ = writeln!(
            out,
            "  … and {} further mismatch(es)",
            result.mismatches.len() - 5
        );
    }
    // Recompute the digest of the A-IR the archive stored as text, so the reader can see the archive
    // agrees with itself about what model it holds. Cheap, and it is the check that catches a manifest
    // edited to match a swapped model file.
    let _ = writeln!(
        out,
        "air digest  {}",
        &sha256_hex(loaded.air_text.as_bytes())[..16]
    );
    if result.reproduced {
        let _ = writeln!(
            out,
            "verdict  REPRODUCED: {} executions identical, archive intact",
            result.matched
        );
        Exit::Clean
    } else {
        let _ = writeln!(
            out,
            "verdict  REPRODUCTION FAILURE: the archive is intact and the arithmetic disagrees"
        );
        Exit::Mismatch
    }
}

/// The store's own summary is one line by construction; collapsed anyway so a stray newline in a
/// future message cannot break the "one fact per line" shape of this report.
fn one_line(text: &str) -> String {
    text.lines().collect::<Vec<_>>().join(" ")
}
