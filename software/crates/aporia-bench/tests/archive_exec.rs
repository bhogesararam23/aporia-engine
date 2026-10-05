//! What a benchmark archive says about how to reproduce the run it holds.
//!
//! The archive's `exec` block is the recipe `aporia replay` re-runs the model under, so it has to be
//! the recipe the campaign actually used. It was not: the harness wrote `ExecConfig::default()`, whose
//! step budget is 50,000,000, for campaigns that ran with 2,000,000 -- so an archive of a model that
//! hit its budget recorded a replay that would not hit it, and the two runs being compared were not
//! the same experiment. These tests pin the recorded fields to the campaign's own configuration rather
//! than to whatever the writing code had in scope.

use aporia_bench::{corpus, harness};
use aporia_search::{Config, Strategy, run};
use aporia_store::Loaded;
use std::path::PathBuf;

fn scratch(name: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    dir.push(format!(
        "aporia-bench-archive-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn an_archive_records_the_step_budget_its_campaign_actually_ran_under() {
    // A number that is neither the default (50,000,000) nor the harness's own (2,000,000), so a test
    // failure cannot be explained by "the two defaults happen to agree today".
    let steps = 123_456_u64;
    let entries = corpus::load(&aporia_bench::corpus_root()).expect("the corpus loads");
    let entry = entries
        .iter()
        .find(|e| e.id() == "analytic/sqrt_domain")
        .expect("the corpus holds analytic/sqrt_domain");
    let model = entry.model.as_ref().expect("this entry compiles");
    let campaign = run(
        model,
        Config {
            budget: 40,
            strategy: Strategy::Adaptive,
            seed: 1,
            max_steps_per_evaluation: steps,
            ..Config::default()
        },
    );

    let dir = scratch("step-budget");
    let (reproduced, matched, total) = harness::archive_and_replay(
        entry,
        &campaign,
        &harness::Plan::default(),
        Strategy::Adaptive,
        1,
        &dir,
    )
    .expect("the archive is written and replayed");
    let loaded = Loaded::open(&dir).expect("the archive reads back");

    assert_eq!(
        loaded.manifest.exec_max_steps, steps,
        "the archive recorded a different step budget from the campaign it holds"
    );
    assert_eq!(loaded.manifest.exec_fp, "f64");
    assert!(
        reproduced,
        "a freshly written archive did not reproduce: {matched}/{total}"
    );
    assert_eq!(matched, total);
    assert!(loaded.integrity_problems().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_campaign_left_at_its_defaults_archives_its_own_defaults_not_something_else() {
    // The distinction the fix turns on: `aporia_search::Config` and `aporia_runtime::ExecConfig` have
    // *different* step defaults (20,000,000 and 50,000,000), so writing the second for a campaign run
    // with the first was never going to be a harmless typo. The archive must agree with the campaign,
    // whatever each happens to default to.
    let entries = corpus::load(&aporia_bench::corpus_root()).expect("the corpus loads");
    let entry = entries
        .iter()
        .find(|e| e.id() == "analytic/sqrt_domain")
        .expect("the corpus holds analytic/sqrt_domain");
    let model = entry.model.as_ref().expect("this entry compiles");
    let campaign = run(
        model,
        Config {
            budget: 40,
            strategy: Strategy::Adaptive,
            seed: 1,
            ..Config::default()
        },
    );
    let dir = scratch("default-budget");
    harness::archive_and_replay(
        entry,
        &campaign,
        &harness::Plan::default(),
        Strategy::Adaptive,
        1,
        &dir,
    )
    .expect("the archive is written and replayed");
    let loaded = Loaded::open(&dir).expect("the archive reads back");
    assert_eq!(
        loaded.manifest.exec_max_steps, campaign.config.max_steps_per_evaluation,
        "the archive recorded an execution guard the campaign never used"
    );
    assert_ne!(
        aporia_search::Config::default().max_steps_per_evaluation,
        aporia_runtime::ExecConfig::default().max_steps,
        "if these two defaults ever converge, this test's premise needs re-reading"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
