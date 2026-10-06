//! The E2 comparison, tested on the documents the harness actually writes.
//!
//! Two kinds of test live here. The real round-trip runs two campaigns through the harness, writes
//! both results documents, and compares them as the command would — the fidelity test, since it
//! exercises the writer, the reader and the comparison as one path. The fabricated documents
//! exist because the comparison's job is *classification*: same, changed, not measurable, not
//! applicable, vacuous, trust-increasing — and those states are easier to pin precisely on
//! outcomes built to contain them than on outcomes that merely happened.

use aporia_bench::corpus;
use aporia_bench::e2::{Arm, compare};
use aporia_bench::harness::{Plan, results_json, sweep};
use aporia_bench::metrics::{Census, ChannelCensus, Outcome};
use aporia_evidence::Channel;
use aporia_search::Strategy;
use aporia_store::{Environment, Json};
use std::path::PathBuf;

fn benchmarks() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../benchmarks")
}

fn environment() -> Environment {
    let mut environment = Environment::current();
    environment.notes.push((
        "corpus".to_string(),
        aporia_bench::corpus_root().display().to_string(),
    ));
    environment
}

fn document(plan: &Plan, entries: &[corpus::Entry]) -> Json {
    results_json(&[], entries, plan, &environment())
}

fn arm_of(doc: &Json) -> Arm {
    Arm::from_document("doc.json", doc).expect("a harness-written document is a readable arm")
}

#[test]
fn a_real_full_arm_and_a_real_ablated_arm_pair_end_to_end() {
    // The whole path: two campaigns, two results documents, one comparison. `sqrt_domain` is the
    // entry where Physical certainly has something to compute — its NaN region is 44% of the
    // domain — so silencing Physical removes something real, and the comparison has to show the
    // removal in the census rather than as a quiet zero.
    let entries = corpus::load(&benchmarks()).expect("corpus loads");
    let entry = entries
        .iter()
        .find(|e| e.id() == "analytic/sqrt_domain")
        .expect("sqrt_domain is in the corpus");
    let base = Plan {
        budgets: vec![80],
        strategies: vec![Strategy::Adaptive],
        seeds: vec![1],
        archive_dir: PathBuf::new(),
        ..Plan::default()
    };
    let full_sweeps = [sweep(entry, &base, 1, Strategy::Adaptive).expect("the full arm runs")];
    let ablated = Plan {
        ablate: vec![Channel::Physical],
        ..base.clone()
    };
    let arm_sweeps = [sweep(entry, &ablated, 1, Strategy::Adaptive).expect("the arm runs")];
    let full_doc = results_json(&full_sweeps, &[entry.clone()], &base, &environment());
    let arm_doc = results_json(&arm_sweeps, &[entry.clone()], &ablated, &environment());
    assert_ne!(
        full_doc.get("identity"),
        arm_doc.get("identity"),
        "the arms share a measurement identity, so one would overwrite the other"
    );

    let full = arm_of(&full_doc);
    let arm = arm_of(&arm_doc);
    let comparison = compare(&full, &arm).expect("one experiment with two masks compares");

    assert_eq!(comparison.label, "-physical");
    assert_eq!(comparison.pairs.len(), 1, "one entry, one seed, one budget");
    let pair = &comparison.pairs[0];
    assert_eq!(pair.budget, 80);
    let physical = pair
        .census
        .iter()
        .find(|d| d.channel == Channel::Physical)
        .expect("the census lists every channel");
    assert!(
        physical.full.computed > 0,
        "Physical computed nothing on sqrt_domain"
    );
    assert_eq!(physical.full.readings, physical.full.computed);
    assert!(
        physical.arm.silenced,
        "the arm's own census says it was masked"
    );
    assert_eq!(physical.arm.readings, 0, "a silenced channel kept readings");
    // The localisation tally accounts for exactly the one region-bearing sweep.
    let l = &comparison.localisation;
    assert_eq!(
        (l.full_only.len() + l.arm_only.len()) as u64
            + l.both_same
            + l.full_earlier
            + l.arm_earlier
            + l.neither,
        1
    );
}

#[test]
fn arms_of_different_experiments_are_refused_rather_than_paired() {
    // The same corpus entry under two plans that differ in more than the mask: different budgets.
    // The refusal names the field, because "these were not one experiment" is actionable only when
    // the reader is told what actually differed.
    let entries = corpus::load(&benchmarks()).expect("corpus loads");
    let entry = entries
        .iter()
        .find(|e| e.id() == "analytic/sqrt_domain")
        .expect("sqrt_domain is in the corpus");
    let full_plan = Plan {
        budgets: vec![320],
        strategies: vec![Strategy::Adaptive],
        seeds: vec![1],
        archive_dir: PathBuf::new(),
        ..Plan::default()
    };
    let other_budgets = Plan {
        budgets: vec![160],
        ..full_plan.clone()
    };
    let full = arm_of(&document(&full_plan, &[entry.clone()]));
    let other = arm_of(&document(&other_budgets, &[entry.clone()]));
    let err = compare(&full, &other).expect_err("different budgets are not one experiment");
    assert!(
        err.contains("budgets"),
        "the refusal must name the field: {err}"
    );

    // And an ablation arm offered as the full arm is refused by name.
    let ablated = Plan {
        ablate: vec![Channel::Numerical],
        ..full_plan.clone()
    };
    let arm = arm_of(&document(&ablated, &[entry.clone()]));
    let err = compare(&arm, &full).expect_err("the first document must be the full arm");
    assert!(err.contains("not the full arm"), "{err}");
}

// ---------------------------------------------------------------- fabricated documents

/// A census with one field that varies and everything else fixed, for outcomes built to contain a
/// specific comparison state.
fn census(computed: &[u64; 5], silenced: &[bool; 5]) -> Census {
    Census {
        channels: Channel::ALL
            .into_iter()
            .enumerate()
            .map(|(i, c)| ChannelCensus {
                channel: c,
                applied: 10,
                computed: computed[i],
                readings: computed[i],
                strong: 0,
                findings: 0,
                silenced: silenced[i],
            })
            .collect(),
    }
}

fn entry(name: &str, control: bool, boundaries: usize) -> corpus::Entry {
    corpus::Entry {
        family: "synthetic".to_string(),
        name: name.to_string(),
        dir: PathBuf::new(),
        source: String::new(),
        model: None,
        diagnostics: Vec::new(),
        truth: aporia_bench::truth::Truth {
            method: "analytic".to_string(),
            fault: "declared".to_string(),
            derivation: String::new(),
            regions: if control {
                Vec::new()
            } else {
                vec![aporia_bench::truth::Declared {
                    reason: "declared".to_string(),
                    axes: vec![("x".to_string(), [0.0, 1.0])],
                }]
            },
            boundaries: (0..boundaries)
                .map(|_| aporia_bench::truth::Boundary {
                    axis: "x".to_string(),
                    at: 0.5,
                    tolerance: 0.01,
                })
                .collect(),
            control,
            static_expected: false,
            narrow: false,
            curved: false,
            degenerate: false,
            expects: String::new(),
        },
    }
}

fn outcome(
    entry: &corpus::Entry,
    evaluations: u64,
    detected: u64,
    localised: u64,
    findings: u64,
    volumes: [f64; 3],
    boundary_error: Option<f64>,
    census: Census,
) -> Outcome {
    Outcome {
        entry: entry.id(),
        family: entry.family.clone(),
        fault: entry.truth.fault.clone(),
        control: entry.truth.control,
        strategy: "adaptive",
        seed: 1,
        budget: 320,
        evaluations,
        instruction_steps: evaluations * 20,
        wall_ms: 0,
        arity: 1,
        declared_regions: entry.truth.regions.len() as u64,
        detected_regions: detected,
        localised_regions: localised,
        first_true_failure: None,
        false_positive_fraction: None,
        suspicious_volume: volumes[0],
        trusted_volume: volumes[1],
        unknown_volume: volumes[2],
        findings,
        duplicates: None,
        boundaries: if entry.truth.boundaries.is_empty() {
            Vec::new()
        } else {
            // A real results row lists every boundary the entry declares, with `error: null` and
            // no band when the run could not see one — the state the comparison must call not
            // measurable rather than zero.
            vec![aporia_bench::metrics::BoundaryHit {
                axis: "x".to_string(),
                declared_at: 0.5,
                tolerance: 0.01,
                band: boundary_error.map(|_| [0.4, 0.6]),
                error: boundary_error,
                within_tolerance: boundary_error.is_some_and(|e| e <= 0.01),
            }]
        },
        counterexamples: Vec::new(),
        census,
        replay: None,
        replay_error: None,
    }
}

/// The fabricated E2 pair: three entries — one where full localises and the arm does not, one
/// where the arm localises and full does not, and one control whose trust grows after ablation —
/// plus a boundary one side cannot see and a channel that computed nothing on one entry.
fn fabricated(entry_evaluations: u64) -> (Json, Json) {
    let region = entry("region", false, 1);
    let quiet = entry("quiet", false, 0);
    let control = entry("control", true, 0);
    let entries = vec![region.clone(), quiet.clone(), control.clone()];

    let full_census = census(&[3, 5, 2, 4, 6], &[false; 5]);
    // Physical and Sensitivity are the two this arm silences; on `quiet`, Sensitivity computed
    // nothing even on the full arm, so the arm is vacuous there and its null is a corpus fact
    // rather than a finding about the channel.
    let arm_census = census(&[3, 0, 2, 4, 0], &[false, true, false, false, true]);
    // On `quiet`, both channels this arm silences computed nothing even on the full arm, so the
    // arm is vacuous there and its null is a corpus fact rather than a finding about the channels.
    let quiet_full_census = census(&[3, 0, 2, 4, 0], &[false; 5]);

    let sweep_for =
        |e: &corpus::Entry, o: Outcome, detected: Option<u64>, localised: Option<u64>| {
            aporia_bench::harness::Sweep {
                entry: e.id(),
                strategy: "adaptive",
                seed: 1,
                outcomes: vec![o],
                detected_at: detected,
                localised_at: localised,
                clean_at: None,
                control_suspicion: None,
                replay: None,
                replay_error: None,
            }
        };

    let full_sweeps = vec![
        sweep_for(
            &region,
            outcome(
                &region,
                entry_evaluations,
                2,
                1,
                3,
                [0.10, 0.80, 0.10],
                Some(0.005),
                full_census.clone(),
            ),
            Some(320),
            Some(320),
        ),
        sweep_for(
            &quiet,
            outcome(
                &quiet,
                entry_evaluations,
                0,
                0,
                0,
                [0.00, 0.95, 0.05],
                None,
                quiet_full_census,
            ),
            None,
            None,
        ),
        sweep_for(
            &control,
            outcome(
                &control,
                entry_evaluations,
                0,
                0,
                0,
                [0.01, 0.50, 0.49],
                None,
                full_census,
            ),
            None,
            None,
        ),
    ];
    let arm_sweeps = vec![
        sweep_for(
            &region,
            outcome(
                &region,
                entry_evaluations,
                1,
                0,
                1,
                [0.20, 0.70, 0.10],
                None,
                arm_census.clone(),
            ),
            Some(320),
            None,
        ),
        sweep_for(
            &quiet,
            outcome(
                &quiet,
                entry_evaluations,
                0,
                1,
                0,
                [0.00, 0.95, 0.05],
                None,
                arm_census.clone(),
            ),
            None,
            Some(320),
        ),
        // The control trusts 0.2 more of the domain after ablation, which cannot be a result.
        sweep_for(
            &control,
            outcome(
                &control,
                entry_evaluations,
                0,
                0,
                0,
                [0.03, 0.70, 0.27],
                None,
                arm_census,
            ),
            None,
            None,
        ),
    ];

    let full_plan = Plan {
        budgets: vec![320],
        strategies: vec![Strategy::Adaptive],
        seeds: vec![1],
        archive_dir: PathBuf::new(),
        ..Plan::default()
    };
    let arm_plan = Plan {
        ablate: vec![Channel::Physical, Channel::Sensitivity],
        ..full_plan.clone()
    };
    let full_doc = results_json(&full_sweeps, &entries, &full_plan, &environment());
    let arm_doc = results_json(&arm_sweeps, &entries, &arm_plan, &environment());
    (full_doc, arm_doc)
}

#[test]
fn the_comparison_classifies_every_state_it_was_built_to_tell_apart() {
    let (full_doc, arm_doc) = fabricated(320);
    let full = arm_of(&full_doc);
    let arm = arm_of(&arm_doc);
    let c = compare(&full, &arm).expect("the fabricated pair is one experiment with two masks");

    // Localisation: full-only on `region`, arm-only on `quiet`, and the control is not
    // applicable rather than tied.
    assert_eq!(
        c.localisation.full_only,
        vec![("synthetic/region".to_string(), 1)]
    );
    assert_eq!(
        c.localisation.arm_only,
        vec![("synthetic/quiet".to_string(), 1)]
    );
    assert_eq!(
        c.localisation.not_applicable, 1,
        "the control has no regions"
    );
    assert_eq!(c.detection.not_applicable, 1);

    // Boundaries: full had one within tolerance, the arm had no band at all, so the state is not
    // measurable — not a zero error and not a lost tolerance.
    assert_eq!(c.boundaries.not_measurable, 1);
    assert_eq!(c.boundaries.lost_within, 0);

    // Counts and volumes: same findings where equal, changed where not.
    let region_pair = c
        .pairs
        .iter()
        .find(|p| p.entry == "synthetic/region")
        .unwrap();
    assert_eq!(region_pair.detected, (2, 1), "changed");
    assert_eq!(region_pair.localised, (1, 0), "changed");
    assert_eq!(region_pair.findings, (3, 1), "changed");
    assert!((region_pair.suspicious.1 - region_pair.suspicious.0 - 0.10).abs() < 1e-12);

    // Census: a silenced channel shows computed-then-dropped, and the entry where Sensitivity
    // computed nothing on the full arm is named vacuous.
    let sens = region_pair
        .census
        .iter()
        .find(|d| d.channel == Channel::Sensitivity)
        .unwrap();
    assert_eq!(
        (sens.full.computed, sens.arm.readings, sens.arm.silenced),
        (6, 0, true)
    );
    assert_eq!(c.vacuous, vec!["synthetic/quiet".to_string()]);

    // Controls: the suspicion grew and the trust grew, and both are named rather than averaged.
    assert_eq!(c.controls.len(), 1);
    assert!(
        (c.controls[0].mean_delta - 0.02).abs() < 1e-12,
        "{:?}",
        c.controls[0]
    );
    assert_eq!(
        c.trust_increases.len(),
        1,
        "ablation bought trust on the control"
    );
    assert!((c.trust_increases[0].gain - 0.20).abs() < 1e-12);

    // The arm's label names both silenced channels.
    assert_eq!(c.label, "-physical+sensitivity");
}

#[test]
fn a_pair_that_charged_a_different_cost_is_refused() {
    let (_full_doc, arm_doc) = fabricated(320);
    // Rebuild the arm's document with one fewer charged evaluation: same plan, same entries, one
    // number different. That arm did not run the same experiment and must not be paired.
    let arm = Arm::from_document("arm.json", &arm_doc).unwrap();
    let (full_doc_cheaper, _) = fabricated(319);
    let cheaper = Arm::from_document("full.json", &full_doc_cheaper).unwrap();
    let err = compare(&cheaper, &arm).expect_err("unequal cost is not a pair");
    assert!(
        err.contains("evaluations"),
        "the refusal must name the cost: {err}"
    );
}
