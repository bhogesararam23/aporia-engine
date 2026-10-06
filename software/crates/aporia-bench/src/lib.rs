//! # aporia-bench
//!
//! The measurement harness: the corpus, the ground-truth checks, and the metrics the research
//! question is answered with.
//!
//! - [`cli`] — the six benchmark commands, reachable as a library so the front door and the
//!   harness binary are one implementation
//! - [`corpus`] — loading entries and verifying that a declared region really is one
//! - [`truth`] — the declaration format and the geometry it needs
//! - [`metrics`] — the metric definitions, each next to the code that computes it
//! - [`harness`] — the budget sweep across strategies, and the results document
//! - [`e2`] — the ablation comparison: arms against the full instrument, from committed files
//!
//! The ordering matters: verification runs before measurement, because a detection rate against a
//! wrong ground truth is worse than no number at all. `aporia-bench verify` is not a convenience —
//! it is the step that turns `benchmarks/` from a collection of opinions into a measuring instrument.

pub mod cli;
pub mod corpus;
pub mod e2;
pub mod harness;
pub mod metrics;
pub mod risk;
pub mod truth;

use std::path::PathBuf;

/// Where the corpus lives. `APORIA_BENCHMARKS` wins, then `./benchmarks` next to the working
/// directory, then the source tree location the harness runs from during development.
#[must_use]
pub fn corpus_root() -> PathBuf {
    if let Ok(dir) = std::env::var("APORIA_BENCHMARKS") {
        return PathBuf::from(dir);
    }
    let here = PathBuf::from("benchmarks");
    if here.join("registry.json").exists() {
        return here;
    }
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.join("benchmarks"))
        .unwrap_or(here.clone());
    if workspace.join("registry.json").exists() {
        return workspace;
    }
    here
}

/// Where experiment archives go during a benchmark run. Regenerateable, so git ignores it; the
/// numbers they produce are what `results/` records.
#[must_use]
pub fn archives_dir() -> PathBuf {
    corpus_root().join("archives")
}

/// Where measured results are written, so they can be committed alongside the corpus.
#[must_use]
pub fn results_dir() -> PathBuf {
    corpus_root().join("results")
}
