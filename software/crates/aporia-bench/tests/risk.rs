//! The risk-threshold oracle, checked against the report it is supposed to reproduce.
//!
//! `aporia-minimize` could previously only ask "does a declared rule fail here?", so a finding made
//! by the measurement channels had no criterion to shrink against and the harness reported no smaller
//! counterexample for it. [`RiskScorer`] asks the question the *report* asked — is this point still at
//! or above the bar that made a cell SUSPICIOUS — against the campaign's frozen calibrator,
//! correlation and ordinary slopes. These tests are about whether that question is the same one, not
//! about whether it is convenient.

use aporia_bench::corpus;
use aporia_bench::risk::RiskScorer;
use aporia_boundary::Policy;
use aporia_ir::Model;
use aporia_search::{Campaign, Config, Strategy, run};
use std::path::PathBuf;

fn benchmarks() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../benchmarks")
}

/// The rates the committed ladder uses, so a measurement here is about the build that produced the
/// published numbers rather than about a configuration nothing else runs.
fn ladder(budget: u64) -> Config {
    Config {
        budget,
        strategy: Strategy::Adaptive,
        seed: 1,
        probe_every: 7,
        calibrate_every: 25,
        numerical_every: 11,
        symmetric_every: 1,
        differential_every: 0,
        refine_every: 40,
        policy: Policy::default(),
        max_steps_per_evaluation: 2_000_000,
    }
}

fn campaign(entry_id: &str, budget: u64) -> (Model, Campaign) {
    let entries = corpus::load(&benchmarks()).expect("corpus loads");
    let entry = entries
        .iter()
        .find(|e| e.id() == entry_id)
        .unwrap_or_else(|| panic!("no corpus entry {entry_id}"));
    let model = entry
        .model
        .clone()
        .unwrap_or_else(|| panic!("{entry_id} does not compile"));
    let campaign = run(&model, ladder(budget));
    (model, campaign)
}

#[test]
fn the_scorer_reproduces_the_risk_that_made_each_finding() {
    // `projectile_zero_gravity` is the entry whose findings the rule oracle could never shrink: its
    // suspicious cells are made of Sensitivity readings and violate no rule. If the frozen scorer
    // cannot flag the very point the report flagged, minimisation with it would be verifying a
    // different claim than the one that was found.
    let (m, c) = campaign("aerospace/projectile_zero_gravity", 640);
    assert!(
        !c.findings.is_empty(),
        "this entry is expected to produce findings"
    );
    let scorer = RiskScorer::from_campaign(&c);
    let missed: Vec<String> = c
        .findings
        .iter()
        .filter(|f| !scorer.violating(&m, &f.representative))
        .map(|f| {
            format!(
                "cell {} final risk {:.3} scorer risk at representative: {}",
                f.cell,
                f.final_risk,
                scorer.risk_at(&m, &f.representative).0
            )
        })
        .collect();
    assert!(
        missed.is_empty(),
        "the scorer disagreed with the report about the points the report flagged:\n{}",
        missed.join("\n")
    );
}

#[test]
fn a_frozen_scorer_answers_the_same_question_in_any_order() {
    // The reference statistics come from the campaign, not from the queries. If asking about a
    // candidate moved the ordinary slope or the fitted scale, ddmin could improve its own yardstick
    // while shrinking the case, and "verified" would mean the reduction graded itself.
    let (m, c) = campaign("aerospace/projectile_zero_gravity", 640);
    let scorer = RiskScorer::from_campaign(&c);
    let points: Vec<Vec<f64>> = c
        .records
        .items
        .iter()
        .take(24)
        .map(|o| o.x.clone())
        .collect();
    let forward: Vec<f64> = points.iter().map(|x| scorer.risk_at(&m, x).0).collect();
    let reverse: Vec<f64> = points
        .iter()
        .rev()
        .map(|x| scorer.risk_at(&m, x).0)
        .rev()
        .collect();
    assert_eq!(
        forward, reverse,
        "the answer depended on the question order"
    );
    // And it is the campaign's own record set that defined the reference: two scorers from the same
    // campaign agree even if one has already been asked about other points.
    let other = RiskScorer::from_campaign(&c);
    for x in &points {
        assert_eq!(scorer.risk_at(&m, x).0, other.risk_at(&m, x).0);
    }
}

#[test]
fn the_scorer_only_consults_channels_the_campaign_actually_ran() {
    // Evidence the report never gathered must not be what a smaller counterexample is verified
    // against: that would flag a candidate for a reason no finding was ever made of, and let a
    // reduction be "verified" by an oracle the atlas does not contain.
    let (m, c) = campaign("aerospace/projectile_zero_gravity", 640);
    let all = RiskScorer::from_campaign(&c);
    let x = &c.findings[0].representative;
    let kinds = |s: &RiskScorer| -> Vec<String> {
        let mut v: Vec<String> = s
            .evidence_at(&m, x)
            .iter()
            .map(|e| e.channel.name().to_string())
            .collect();
        v.sort();
        v.dedup();
        v
    };
    assert!(
        kinds(&all).contains(&"sensitivity".to_string()),
        "the campaign probed, so the candidate's star should be judged: {:?}",
        kinds(&all)
    );
    assert!(
        !kinds(&all).contains(&"differential".to_string()),
        "the campaign never ran the reference path"
    );

    let no_probes = RiskScorer::from_campaign(&run(
        &m,
        Config {
            probe_every: 0,
            numerical_every: 0,
            ..ladder(640)
        },
    ));
    let quiet = kinds(&no_probes);
    assert!(
        !quiet.contains(&"sensitivity".to_string()),
        "no probes were taken, yet sensitivity was claimed: {quiet:?}"
    );
    assert!(
        !quiet.contains(&"numerical".to_string()),
        "no precision pair was run, yet numerical evidence appeared: {quiet:?}"
    );
}

#[test]
fn a_model_the_report_trusted_everywhere_gives_the_scorer_nothing_to_flag() {
    // `coupled_coils` is trusted over its whole domain in every campaign measured. Its scorer has a
    // real reference and a real calibrator, so if the oracle invented suspicion it would show up
    // here -- across the declared grid, not at one lucky point.
    let (m, c) = campaign("electromagnetics/coupled_coils", 320);
    assert_eq!(
        c.atlas.coverage().suspicious,
        0.0,
        "the report flagged part of a control"
    );
    let scorer = RiskScorer::from_campaign(&c);
    let mut flagged = Vec::new();
    for i in 0..40 {
        let a = 1.0 + 39.0 * (i as f64) / 40.0;
        let b = 1.0 + 39.0 * ((i * 7) % 40) as f64 / 40.0;
        let x = vec![a, b];
        let (risk, _) = scorer.risk_at(&m, &x);
        if risk >= scorer.threshold() {
            flagged.push((x, risk));
        }
    }
    assert!(
        flagged.is_empty(),
        "the oracle went suspicious where the atlas is trusted: {flagged:?}"
    );
}

#[test]
fn a_sensitivity_finding_can_now_be_made_smaller_and_verified() {
    // The capability, end to end: before the risk oracle this entry produced findings and no verified
    // minimisation at all, because a Sensitivity reading violates no rule. `counterexamples` is the
    // production path the ladder records, so this asserts on the rows the results JSON will contain.
    let entries = corpus::load(&benchmarks()).expect("corpus loads");
    let entry = entries
        .iter()
        .find(|e| e.id() == "aerospace/projectile_zero_gravity")
        .expect("entry present");
    let model = entry.model.clone().expect("entry compiles");
    let c = run(&model, ladder(640));
    let rows = aporia_bench::metrics::counterexamples(entry, &c, 4000);
    assert!(!rows.is_empty(), "the campaign produced no findings");
    let risk_rows: Vec<&_> = rows.iter().filter(|r| r.oracle == "risk").collect();
    assert!(
        !risk_rows.is_empty(),
        "nothing was attributed to the risk oracle: {:?}",
        rows.iter()
            .map(|r| (r.oracle, r.verified, r.dimensions, r.digits))
            .collect::<Vec<_>>()
    );
    for r in &risk_rows {
        assert!(
            r.verified,
            "a row was attributed to the risk oracle without verifying: {r:?}"
        );
        assert!(
            r.dimensions > 0 && r.digits > 0,
            "an empty case was reported as verified: {:?}",
            r.description
        );
    }
    // And the reduction has to be a reduction: this entry has three parameters, all of them pinned at
    // the start, so a verified risk case that still names three full-precision parameters would have
    // proved only that the oracle fires, not that anything was made smaller.
    assert!(
        risk_rows.iter().any(|r| r.dimensions < 3),
        "nothing was dropped from any case: {:?}",
        risk_rows
            .iter()
            .map(|r| (r.dimensions, r.digits, &r.description))
            .collect::<Vec<_>>()
    );
}

#[test]
fn a_risk_verified_reduction_is_rechecked_at_every_witness_it_claims() {
    // What minimising against risk does and does not claim. It is *not* a zoom into the flagged cell:
    // measured on `electromagnetics/rlc_resonance`, a Physical-driven finding's reduced case pinned
    // axis 0 at 1.29898 while the cell the atlas labelled was [2.039219, 2.041658]. That is ddmin
    // reporting that the failure does not depend on that parameter — dropping an axis hands the axis
    // back to the whole declared domain, which is the whole content of the claim — so the property
    // worth testing is the one the row actually asserts: every witness of the reduced case is
    // re-checked by the same frozen oracle that verified it, within the budget, and the case really
    // did shrink.
    let entries = corpus::load(&benchmarks()).expect("corpus loads");
    let entry = entries
        .iter()
        .find(|e| e.id() == "electromagnetics/rlc_resonance")
        .expect("entry present");
    let model = entry.model.clone().expect("entry compiles");
    let c = run(&model, ladder(640));
    let mut reduced = 0usize;
    for f in c.findings.iter().take(3) {
        let scorer = RiskScorer::for_finding(&c, f);
        if !scorer.agrees_with(&model, f) {
            continue;
        }
        let risk_oracle = |x: &[f64]| scorer.violating(&model, x);
        let minimal = aporia_minimize::minimize(
            &risk_oracle,
            &model,
            &f.representative,
            aporia_minimize::Config {
                budget: 4000,
                ..aporia_minimize::Config::default()
            },
        );
        if !minimal.verified {
            continue;
        }
        // The start is every parameter pinned at full precision, so minimisation may shrink the case
        // but must never grow it. Measured on this entry the honest answer for one finding is that a
        // parameter cannot be dropped at all: the reduced case is `w = 2`, one axis, which is already
        // minimal, so shrinking is asserted where it is achievable and non-growth is asserted here.
        let start = aporia_minimize::Case::from_model(&model, &f.representative);
        assert!(
            minimal.case.dimensions() <= start.dimensions()
                && minimal.case.digits() <= start.digits(),
            "a case grew while being minimised: {:?} -> {:?}",
            start.describe(),
            minimal.case.describe()
        );
        let check = minimal.case.verify(&risk_oracle, 4000);
        assert!(
            check.holds && !check.over_budget,
            "the reduced case did not survive its own witnesses: {check:?} in {:?}",
            minimal.case.describe()
        );
        reduced += 1;
    }
    assert!(
        reduced > 0,
        "no verified risk reduction was produced to check"
    );
}

#[test]
fn a_finding_made_only_of_declared_relations_is_not_claimed_to_be_minimisable() {
    // The coverage limit, tested rather than footnoted. A relation claim is judged over the whole
    // record set, so a candidate's three-point neighbourhood cannot reproduce it, and the scorer must
    // say it cannot answer rather than quietly grading the reduction by a weaker question.
    let (m, c) = campaign("electromagnetics/coupled_coils", 320);
    let fabricated = aporia_search::Finding {
        cell: 0,
        cells: vec![0],
        signature: vec!["B|check0".to_string()],
        bounds: vec![[1.0, 40.0], [1.0, 40.0]],
        representative: vec![7.0, 13.0],
        observation: 0,
        online_risk: 0.6,
        final_risk: 0.6,
        samples: 1,
        evidence: vec![aporia_evidence::Evidence::new(
            aporia_evidence::Channel::Behavioral,
            aporia_evidence::Subject::Relation(0),
            0.6,
            vec![0, 1],
            "declared symmetry judged over the record set".to_string(),
        )],
    };
    let scorer = RiskScorer::for_finding(&c, &fabricated);
    assert!(
        !scorer.agrees_with(&m, &fabricated),
        "the oracle accepted a relation finding it cannot measure at a point"
    );
    // And with no channel left to consult, it reports nothing anywhere: silence, not a clean bill.
    assert!(scorer.evidence_at(&m, &[7.0, 13.0]).is_empty());
    assert!(!scorer.violating(&m, &[7.0, 13.0]));
}
