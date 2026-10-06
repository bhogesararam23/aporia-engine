//! The risk-threshold oracle, checked against the report it is supposed to reproduce.
//!
//! `aporia-minimize` could previously only ask "does a declared rule fail here?", so a finding made
//! by the measurement channels had no criterion to shrink against and the harness reported no smaller
//! counterexample for it. [`RiskScorer`] asks the question the *report* asked — is this point still at
//! or above the bar that made a cell SUSPICIOUS — against the campaign's frozen calibrator,
//! correlation and ordinary slopes. These tests are about whether that question is the same one, not
//! about whether it is convenient.

use aporia_bench::corpus;
use aporia_bench::risk::{RiskOracle, RiskScorer};
use aporia_boundary::Policy;
use aporia_ir::Model;
use aporia_minimize::Oracle;
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
        silenced: 0,
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
    let scorer = RiskScorer::from_campaign(&c, &aporia_runtime::Interp);
    let missed: Vec<String> = c
        .findings
        .iter()
        .filter(|f| {
            scorer
                .read(&m, &f.representative, &mut aporia_runtime::Interp)
                .risk
                < scorer.threshold()
        })
        .map(|f| {
            format!(
                "cell {} final risk {:.3} scorer risk at representative: {}",
                f.cell,
                f.final_risk,
                scorer
                    .read(&m, &f.representative, &mut aporia_runtime::Interp)
                    .risk
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
    let scorer = RiskScorer::from_campaign(&c, &aporia_runtime::Interp);
    let points: Vec<Vec<f64>> = c
        .records
        .items
        .iter()
        .take(24)
        .map(|o| o.x.clone())
        .collect();
    let forward: Vec<f64> = points
        .iter()
        .map(|x| scorer.read(&m, x, &mut aporia_runtime::Interp).risk)
        .collect();
    let reverse: Vec<f64> = points
        .iter()
        .rev()
        .map(|x| scorer.read(&m, x, &mut aporia_runtime::Interp).risk)
        .rev()
        .collect();
    assert_eq!(
        forward, reverse,
        "the answer depended on the question order"
    );
    // And it is the campaign's own record set that defined the reference: two scorers from the same
    // campaign agree even if one has already been asked about other points.
    let other = RiskScorer::from_campaign(&c, &aporia_runtime::Interp);
    for x in &points {
        assert_eq!(
            scorer.read(&m, x, &mut aporia_runtime::Interp).risk,
            other.read(&m, x, &mut aporia_runtime::Interp).risk
        );
    }
}

#[test]
fn the_scorer_only_consults_channels_the_campaign_actually_ran() {
    // Evidence the report never gathered must not be what a smaller counterexample is verified
    // against: that would flag a candidate for a reason no finding was ever made of, and let a
    // reduction be "verified" by an oracle the atlas does not contain.
    let (m, c) = campaign("aerospace/projectile_zero_gravity", 640);
    let all = RiskScorer::from_campaign(&c, &aporia_runtime::Interp);
    let x = &c.findings[0].representative;
    let kinds = |s: &RiskScorer| -> Vec<String> {
        let mut v: Vec<String> = s
            .read(&m, x, &mut aporia_runtime::Interp)
            .items
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

    let no_probes = RiskScorer::from_campaign(
        &run(
            &m,
            Config {
                probe_every: 0,
                numerical_every: 0,
                ..ladder(640)
            },
        ),
        &aporia_runtime::Interp,
    );
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
    let scorer = RiskScorer::from_campaign(&c, &aporia_runtime::Interp);
    let mut flagged = Vec::new();
    for i in 0..40 {
        let a = 1.0 + 39.0 * (i as f64) / 40.0;
        let b = 1.0 + 39.0 * ((i * 7) % 40) as f64 / 40.0;
        let x = vec![a, b];
        let risk = scorer.read(&m, &x, &mut aporia_runtime::Interp).risk;
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
    let rows = aporia_bench::metrics::counterexamples(entry, &c, 4000, &mut aporia_runtime::Interp);
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
        let scorer = RiskScorer::for_finding(&c, f, &aporia_runtime::Interp);
        if !scorer.agrees_with(&model, f, &mut aporia_runtime::Interp) {
            continue;
        }
        let risk_oracle = RiskOracle::new(&model, &scorer);
        let minimal = aporia_minimize::minimize(
            &risk_oracle,
            &mut aporia_runtime::Interp,
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
        let check = minimal
            .case
            .verify(&risk_oracle, &mut aporia_runtime::Interp, 4000);
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
fn a_risk_query_is_charged_for_every_execution_it_performs() {
    // The unit price, measured rather than derived from a formula in a doc comment. A scorer told to
    // consult nothing but the Physical channel runs the model once per answer, exactly like the rule
    // oracle; everything above one is a channel that costs evaluations, and the ladder's rates decide
    // how many there are. `projectile_zero_gravity` runs probes every seventh round and a precision
    // pair every eleventh, and the finding taken here is made of Physical, Sensitivity and Numerical
    // evidence, so its query is: the point, one perturbed run per axis (three), and the f32 re-run.
    let (m, c) = campaign("aerospace/projectile_zero_gravity", 640);
    let finding = c
        .findings
        .iter()
        .find(|f| {
            let s = RiskScorer::for_finding(&c, f, &aporia_runtime::Interp);
            s.agrees_with(&m, f, &mut aporia_runtime::Interp)
        })
        .expect("a finding the frozen scorer reproduces");
    let scorer = RiskScorer::for_finding(&c, finding, &aporia_runtime::Interp);
    let oracle = RiskOracle::new(&m, &scorer);
    let answer = oracle.query(&finding.representative, &mut aporia_runtime::Interp);
    assert!(
        answer.violating,
        "the scorer stopped reproducing its own finding"
    );
    assert_eq!(
        answer.executions,
        1 + m.params.len() as u64 + 1,
        "base point, one probe per axis, one reduced-precision re-run"
    );
    // A rule question about the same point costs exactly one, which is the asymmetry the two
    // published columns exist to stop hiding.
    let rule = aporia_minimize::FailureOracle::new(&m)
        .query(&finding.representative, &mut aporia_runtime::Interp);
    assert_eq!(rule.executions, 1);
    assert!(
        !rule.violating,
        "this entry's findings violate no declared rule; that is why the fallback exists"
    );

    // And the price follows the channels, not the oracle's name: with nothing but the model's own
    // rules to consult, a risk query is one execution, starless.
    let quiet = run(
        &m,
        Config {
            probe_every: 0,
            numerical_every: 0,
            differential_every: 0,
            symmetric_every: 0,
            ..ladder(640)
        },
    );
    let physical_only = RiskScorer::from_campaign(&quiet, &aporia_runtime::Interp);
    assert_eq!(
        RiskOracle::new(&m, &physical_only)
            .query(&finding.representative, &mut aporia_runtime::Interp)
            .executions,
        1,
        "a scorer with no measurement channel was charged for evaluations it did not run"
    );
}

#[test]
fn the_counterexample_column_carries_both_units_and_never_undersells_the_work() {
    // What the published rows assert: an answer cannot cost fewer executions than calls, and a row
    // whose description came from the risk oracle paid for its probe star in the difference.
    let entries = corpus::load(&benchmarks()).expect("corpus loads");
    let entry = entries
        .iter()
        .find(|e| e.id() == "aerospace/projectile_zero_gravity")
        .expect("entry present");
    let model = entry.model.clone().expect("entry compiles");
    let c = run(&model, ladder(640));
    let rows = aporia_bench::metrics::counterexamples(entry, &c, 4000, &mut aporia_runtime::Interp);
    assert!(!rows.is_empty(), "the campaign produced no findings");
    for r in &rows {
        assert!(
            r.executions >= r.queries,
            "a row was charged fewer executions than answers: {r:?}"
        );
    }
    let paid = rows
        .iter()
        .filter(|r| r.oracle == "risk" && r.executions > r.queries)
        .count();
    assert!(
        paid > 0,
        "every risk row was priced as one execution per answer: {:?}",
        rows.iter()
            .map(|r| (r.oracle, r.verified, r.queries, r.executions))
            .collect::<Vec<_>>()
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
    let scorer = RiskScorer::for_finding(&c, &fabricated, &aporia_runtime::Interp);
    assert!(
        !scorer.agrees_with(&m, &fabricated, &mut aporia_runtime::Interp),
        "the oracle accepted a relation finding it cannot measure at a point"
    );
    // And with no channel left to consult, it reports nothing anywhere: silence, not a clean bill.
    let quiet = scorer.read(&m, &[7.0, 13.0], &mut aporia_runtime::Interp);
    assert!(quiet.items.is_empty());
    assert!(
        quiet.risk < scorer.threshold(),
        "an oracle with no channel still cleared the bar: {quiet:?}"
    );
    assert_eq!(
        RiskOracle::new(&m, &scorer)
            .query(&[7.0, 13.0], &mut aporia_runtime::Interp)
            .executions,
        1,
        "an oracle with nothing to consult was charged a full measurement"
    );
}

/// One answer, one implementation, one precision — which is what a program APORIA did not parse is.
///
/// The values come from the interpreter so that the evidence under test is real readings, but both
/// capability flags say no, and that is the entire thing these two tests are about: a scorer must
/// inherit the limits of the path it measures through, and must refuse rather than quietly measure
/// less when it is handed a path that cannot answer what its finding was made of.
#[derive(Default)]
struct SinglePath {
    inner: aporia_runtime::Interp,
    runs: usize,
}

impl aporia_runtime::Executor for SinglePath {
    fn execute(
        &mut self,
        model: &Model,
        x: &[f64],
        cfg: aporia_runtime::ExecConfig,
    ) -> aporia_runtime::Outcome {
        self.runs += 1;
        self.inner.execute(model, x, cfg)
    }

    fn varies_with_precision(&self) -> bool {
        false
    }

    fn has_reference_path(&self) -> bool {
        false
    }
}

#[test]
fn a_scorer_inherits_the_channels_its_path_can_measure() {
    // The campaign gated its own sensors on these flags, so a scorer that ignored them would claim
    // channels the report never had — and for a program's campaign that means comparing the program
    // against itself at a precision it was never told to use, then calling the agreement evidence.
    //
    // Asserted against the mask, not against emitted items: a consulted channel that finds nothing
    // emits nothing, so "no numerical item here" would pass for the wrong reason. The first run of
    // this test made exactly that mistake and failed on a model where the reference path agrees.
    let entries = corpus::load(&benchmarks()).expect("corpus loads");
    let entry = entries
        .iter()
        .find(|e| e.id() == "aerospace/projectile_zero_gravity")
        .expect("entry present");
    let m = entry.model.clone().expect("entry compiles");
    let c = run(
        &m,
        Config {
            numerical_every: 11,
            differential_every: 3,
            ..ladder(640)
        },
    );
    let names = |s: &RiskScorer| {
        let mut v: Vec<String> = s
            .channels()
            .iter()
            .map(|ch| ch.name().to_string())
            .collect();
        v.sort();
        v
    };
    assert_eq!(
        names(&RiskScorer::from_campaign(&c, &aporia_runtime::Interp)),
        vec![
            "differential".to_string(),
            "numerical".to_string(),
            "physical".to_string(),
            "sensitivity".to_string()
        ],
        "the interpreter offers both path-dependent channels, so the scorer must claim both"
    );
    let foreign = names(&RiskScorer::from_campaign(&c, &SinglePath::default()));
    assert_eq!(
        foreign,
        vec!["physical".to_string(), "sensitivity".to_string()],
        "a single-implementation, single-precision path was handed a channel it cannot answer"
    );
    // And the mask is not just a claim: measured at a real finding, that path emits no item whose
    // channel requires a second implementation or a second rounding.
    let x = &c.findings.first().expect("findings").representative;
    let emitted: Vec<String> = RiskScorer::from_campaign(&c, &SinglePath::default())
        .read(&m, x, &mut SinglePath::default())
        .items
        .iter()
        .map(|e| e.channel.name().to_string())
        .collect();
    assert!(
        !emitted.contains(&"numerical".to_string())
            && !emitted.contains(&"differential".to_string()),
        "the foreign path produced path-dependent evidence anyway: {emitted:?}"
    );
}

#[test]
fn a_path_that_cannot_answer_a_wanted_channel_gets_no_verdict_not_a_lower_score() {
    // The trap this closes is silent: fusing fewer items gives a *smaller* risk, so a scorer that
    // dropped the channel it could not measure would look conservative while grading a reduction by a
    // question the report never asked. `Reading::answered` is the difference between "not suspicious
    // here" and "I could not ask".
    let entries = corpus::load(&benchmarks()).expect("corpus loads");
    let entry = entries
        .iter()
        .find(|e| e.id() == "aerospace/projectile_zero_gravity")
        .expect("entry present");
    let m = entry.model.clone().expect("entry compiles");
    let c = run(
        &m,
        Config {
            numerical_every: 11,
            differential_every: 3,
            ..ladder(640)
        },
    );
    // Built from the interpreter's campaign, asked of a path that offers neither second route.
    let scorer = RiskScorer::from_campaign(&c, &aporia_runtime::Interp);
    let mut path = SinglePath::default();
    let x = &c.findings.first().expect("findings").representative;
    let reading = scorer.read(&m, x, &mut path);
    let mut names: Vec<String> = reading
        .unmeasurable
        .iter()
        .map(|ch| ch.name().to_string())
        .collect();
    names.sort();
    assert_eq!(
        names,
        vec!["differential".to_string(), "numerical".to_string()],
        "the reading did not say which channels the path could not answer"
    );
    assert!(
        !reading.answered(),
        "a reading missing two of the finding's channels claimed to answer"
    );
    assert!(
        !RiskOracle::new(&m, &scorer)
            .query(x, &mut SinglePath::default())
            .violating,
        "an unanswerable reading was allowed to verify a reduction"
    );
    // The base point and its probe star, and nothing for the two channels it could not measure. The
    // equality is the invariant worth asserting: executions are counted where they are spent, so a
    // channel that was skipped must not have been charged either.
    assert_eq!(
        reading.executions, path.runs as u64,
        "the reading charged executions the path did not perform"
    );
    assert!(
        reading.executions >= 1,
        "a path that was asked about a point ran nothing at all"
    );
}
