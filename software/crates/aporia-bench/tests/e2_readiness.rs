//! Is E2 actually runnable, checked one arm at a time rather than in principle.
//!
//! The experiment design lists the arms it needs — the full instrument, each single channel removed,
//! and each single channel kept — and the existing ablation control proves the properties it was
//! written for on three channels silenced together. This file asks the narrower question the run
//! would otherwise discover late: does each arm *on its own* still cost exactly what the full arm
//! costs, still say which channels it had, and still get its own measurement identity?
//!
//! The channels do not buy the same things, which is why this is not a loop over one assertion.
//! Differential buys reference evaluations, Numerical buys f32 re-runs, Sensitivity buys probe stars,
//! and Physical and Behavioral buy nothing beyond the point they are read from. A change that quietly
//! skipped the work for any one of them would still pass a three-at-once test, because the other two
//! would hide it.

use aporia_bench::corpus;
use aporia_bench::harness::Plan;
use aporia_evidence::Channel;
use aporia_ir::Model;
use aporia_search::{Config, Strategy, run};
use std::path::PathBuf;

fn benchmarks() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../benchmarks")
}

/// The ladder's own rates, including the two channels that cost reference and f32 evaluations, so an
/// arm here is measured on the configuration the published numbers came from.
fn ladder(budget: u64) -> Config {
    Config {
        budget,
        strategy: Strategy::Adaptive,
        seed: 1,
        probe_every: 7,
        calibrate_every: 25,
        numerical_every: 11,
        symmetric_every: 1,
        differential_every: 11,
        refine_every: 40,
        max_steps_per_evaluation: 2_000_000,
        silenced: 0,
        policy: aporia_boundary::Policy::default(),
    }
}

/// An entry whose readings exercise all five channels, so "the arm lost a channel" is observable
/// rather than vacuous. `euler_decay_2d` is the entry that carries a declared relation, a divergence
/// risk, sensitivity readings and — with these rates — both paid probes.
const FULL_INSTRUMENT_ENTRY: &str = "ode/euler_decay";

fn model(entry_id: &str) -> Model {
    let entries = corpus::load(&benchmarks()).expect("corpus loads");
    let entry = entries
        .iter()
        .find(|e| e.id() == entry_id)
        .unwrap_or_else(|| panic!("no corpus entry {entry_id}"));
    entry
        .model
        .clone()
        .unwrap_or_else(|| panic!("{entry_id} does not compile"))
}

#[test]
fn every_single_channel_arm_costs_exactly_what_the_full_arm_costs() {
    let m = model(FULL_INSTRUMENT_ENTRY);
    let full = run(&m, ladder(320));
    assert!(
        full.evaluations >= 300,
        "the budget went unspent, so a cost comparison here would prove nothing: {}",
        full.evaluations
    );
    for channel in Channel::ALL {
        let arm = run(&m, ladder(320).without(channel));
        assert_eq!(
            full.evaluations, arm.evaluations,
            "silencing {channel:?} changed what the run was charged"
        );
        assert_eq!(
            full.records.items.len(),
            arm.records.items.len(),
            "silencing {channel:?} changed the number of executions recorded"
        );
        assert_eq!(
            full.instruction_steps, arm.instruction_steps,
            "silencing {channel:?} changed the instruction steps executed — the arithmetic ran or \
             it did not, and a reading cannot be withheld by not doing the work"
        );
        // Same seed, same first point: the arms are allowed to diverge later, because the evidence a
        // campaign acts on is what steers it, and that divergence is the effect E2 measures. What is
        // not allowed is an arm that starts somewhere else.
        assert_eq!(
            full.records.items[0].x, arm.records.items[0].x,
            "silencing {channel:?} moved the first placed point"
        );
        let json = arm.config.json().to_compact();
        assert!(
            json.contains(&format!("\"silenced\":[\"{}\"]", channel.name())),
            "the arm did not record its own mask by name: {json}"
        );
    }
}

#[test]
fn a_silenced_channel_leaves_no_reading_anywhere_in_the_report() {
    let m = model(FULL_INSTRUMENT_ENTRY);
    let full = run(&m, ladder(320));
    for channel in Channel::ALL {
        let arm = run(&m, ladder(320).without(channel));
        let leaked: Vec<&str> = arm
            .evidence
            .iter()
            .filter(|e| e.channel == channel)
            .map(|e| e.detail.as_str())
            .collect();
        assert!(
            leaked.is_empty(),
            "the {channel:?} arm still reports {} reading(s) of that channel: {:?}",
            leaked.len(),
            leaked
        );
        // The control is only meaningful if the channel was there to remove. Recorded rather than
        // assumed, because an entry that produces no Behavioral readings makes "removing Behavioral
        // changed nothing" a vacuous result, and E2 has to say which of its arms are vacuous.
        let present = full
            .evidence
            .iter()
            .filter(|e| e.channel == channel)
            .count();
        if present == 0 {
            // Nothing to remove on this entry: an arm that cannot distinguish itself is not evidence
            // about the channel, and the experiment record must say so per entry rather than per arm.
            continue;
        }
        assert!(
            arm.findings.len() <= full.findings.len() + full.findings.len().max(1),
            "removing a channel that produced {present} readings increased the findings by more \
             than the whole: {} against {}",
            arm.findings.len(),
            full.findings.len()
        );
    }
}

#[test]
fn each_arm_of_e2_is_a_different_measurement() {
    // Identities first, because an arm that shares one would overwrite the arm it is compared against,
    // and the harness would report it as a duplicate rather than as a design error.
    let entries = corpus::load(&benchmarks()).expect("corpus loads");
    let mut environment = aporia_store::Environment::current();
    environment.notes.push((
        "corpus".to_string(),
        aporia_bench::corpus_root().display().to_string(),
    ));
    let identity = |plan: &Plan| -> String {
        aporia_bench::harness::results_json(&[], &entries, plan, &environment, None)
            .get("identity")
            .and_then(aporia_store::Json::as_str)
            .expect("a results document records its own identity")
            .to_string()
    };
    let mut seen = std::collections::HashSet::new();
    let arms: Vec<Vec<Channel>> = std::iter::once(Vec::new())
        .chain(Channel::ALL.iter().map(|c| vec![*c]))
        .chain(
            Channel::ALL
                .iter()
                .map(|keep| Channel::ALL.into_iter().filter(|c| c != keep).collect()),
        )
        .collect();
    for ablate in arms {
        let plan = Plan {
            ablate: ablate.clone(),
            ..Plan::default()
        };
        let name = if ablate.is_empty() {
            "full".to_string()
        } else if ablate.len() == Channel::ALL.len() - 1 {
            format!(
                "only-{}",
                Channel::ALL
                    .iter()
                    .find(|c| !ablate.contains(c))
                    .expect("kept")
                    .name()
            )
        } else {
            format!("-{}", ablate[0].name())
        };
        let id = identity(&plan);
        assert!(
            seen.insert(id.clone()),
            "the {name} arm shares measurement identity {id} with another arm"
        );
    }
    // 1 full + 5 single removals + 5 single channels kept.
    assert_eq!(seen.len(), 11, "E2 needs eleven distinguishable arms");
}

#[test]
fn an_arm_that_keeps_one_channel_is_expressible_and_says_so() {
    // The five single-channel arms are the ones that answer "does any one channel do this on its own",
    // and they are expressed as silencing the other four — legal, because four is not all five.
    let m = model(FULL_INSTRUMENT_ENTRY);
    for keep in Channel::ALL {
        let mut cfg = ladder(320);
        for c in Channel::ALL {
            if c != keep {
                cfg = cfg.without(c);
            }
        }
        assert_eq!(
            cfg.consulted(),
            vec![keep],
            "the arm meant to keep only {keep:?} consulted something else"
        );
        let arm = run(&m, cfg);
        assert_eq!(
            arm.evaluations,
            run(&m, ladder(320)).evaluations,
            "the {keep:?}-only arm cost a different number of evaluations, so it is not comparable with the full instrument"
        );
        // And its report cannot credit a channel it was not allowed to have.
        assert!(
            arm.evidence.iter().all(|e| e.channel == keep),
            "the {keep:?}-only arm reported evidence from another channel: {:?}",
            arm.evidence.iter().map(|e| e.channel).collect::<Vec<_>>()
        );
    }
}

#[test]
fn the_fitted_calibrator_and_correlation_never_saw_the_silenced_channel() {
    // E2 grades each arm by the evidence it was allowed to have, and the grading machinery itself
    // is fitted from that same evidence: the calibrator's per-channel scales and the channel
    // correlation are estimated from the evidence log after the mask's retain. If a silenced
    // channel's magnitudes still entered either fit, the ablated arm would be calibrated by the
    // readings it was supposed to be without — the ablation would leak through the scales, and a
    // "no effect" null would be an artifact of grading both arms with the full instrument's
    // ruler.
    //
    // `euler_decay` produces Numerical readings at these rates (f32 against f64 on a decaying
    // exponential), so an arm that silences Numerical must report that channel's scale as
    // unfitted — the fit had no magnitudes to fit from.
    let m = model(FULL_INSTRUMENT_ENTRY);
    let full = run(&m, ladder(320));
    assert!(
        full.calibrator.is_fitted(Channel::Numerical),
        "the full arm fitted no Numerical scale, so this entry stops proving the exclusion"
    );
    let arm = run(&m, ladder(320).without(Channel::Numerical));
    assert!(
        !arm.calibrator.is_fitted(Channel::Numerical),
        "the -numerical arm fitted a scale for the channel it silenced: {}",
        arm.calibrator.describe()
    );
    // The channels it kept stay fitted — the exclusion is per channel, not a refusal to calibrate.
    for kept in Channel::ALL {
        if kept == Channel::Numerical {
            continue;
        }
        if full.calibrator.is_fitted(kept) {
            assert!(
                arm.calibrator.is_fitted(kept),
                "silencing Numerical unfitted the {kept:?} scale too: {}",
                arm.calibrator.describe()
            );
        }
    }
    // And the correlation estimate never saw a silenced pair: a channel that cannot appear in the
    // log cannot appear in a correlated pair with it. The samples count is over the retained log,
    // so the silenced arm's population is at most the full arm's.
    assert!(
        arm.correlation.samples() <= full.correlation.samples(),
        "the silenced arm's correlation was fitted from more evidence sets than the full arm's"
    );
}
