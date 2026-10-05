//! What the Behavioral channel's scale is actually made of, and what it does to a claim.
//!
//! The open question this measures, rather than argues: `coupled_coils` declares
//! `check symmetric(reactance wrt (turns_a, turns_b))` and its arithmetic is left-associated, so a
//! swapped evaluation can differ from the base by one unit in the last place -- around 1e-16,
//! relative. That single reading is the only Behavioral evidence the campaign gathers, so it is what
//! `Calibrator::fit` sees for the whole channel; with fewer than eight measurements of its own the
//! claim has no stratum, and the channel-wide reference falls back to `MIN_SCALE` = 1e-12. Is that
//! guard protecting the report from the evaluator's own rounding, or is it the number that decides
//! whether a real violation gets believed?
//!
//! A dose-response run answers that with measurements instead of a chosen constant. Five models, same
//! declared relation, same seed and budget, differing only in how far from symmetry the arithmetic
//! actually is:
//!
//! | case | the model computes | what it means |
//! |---|---|---|
//! | exact | `0.3 * (a * b)` | the claim holds and the arithmetic agrees with it |
//! | reassociation | `((0.3 * a) * b)` vs `((0.3 * b) * a)` | the claim holds; the *evaluations* differ by ulps |
//! | `1e-14` | claim violated by a term of that size | a real violation at round-off scale |
//! | `1e-10` | ditto, larger | a real violation, small |
//! | `1e-3` | ditto, obvious | a real violation |
//!
//! What is recorded for each: the swap count, the largest and median relative asymmetry actually
//! observed, the Behavioral channel scale the campaign fitted, whether the claim earned its own
//! stratum, the loudest calibrated strength the symmetry evidence reached, and how much of the map
//! ended up SUSPICIOUS. The assertions are on the two things that must not happen -- the ulp artifact
//! must not become suspicion, and the obvious violation must not become silence -- and everything else
//! is written down as measured, including where the transition between them falls.

use aporia_dsl::lower::compile;
use aporia_evidence::{Channel, Subject, calibrate::median};
use aporia_ir::RelationKind;
use aporia_search::{Config, Strategy, run};

/// Every case is the same two-parameter model with the same declared relation; only the expression
/// differs, and only by the size of the term that breaks the symmetry.
fn model_source(tail: &str) -> String {
    format!(
        "model scale_probe \"\" {{\n input a : count in [1, 40]\n input b : count in [1, 40]\n let y = 0.5 * (a * a + b * b) + {tail}\n require y > 0\n check symmetric(y wrt (a, b))\n}}\n"
    )
}

/// What one case produced. Every field is read out of the campaign, not typed in.
#[derive(Clone, Debug)]
struct Reading {
    case: &'static str,
    swaps: usize,
    items: usize,
    largest: f64,
    median: f64,
    channel_scale: f64,
    own_stratum: bool,
    loudest: f64,
    suspicious: f64,
    cells_flagged: usize,
    findings: usize,
    peak_risk: f64,
}

impl Reading {
    fn row(&self) -> String {
        use std::fmt::Write as _;
        let mut line = String::new();
        let _ = write!(
            line,
            "{:<14} swaps {:>4}  items {:>4}  largest {:>10.3e}  median {:>10.3e}  scale {:>10.3e}  stratum {:<5}  loudest {:>6.3}  peak risk {:>6.3}  suspicious {:>6.4}  flagged cells {}  findings {}",
            self.case,
            self.swaps,
            self.items,
            self.largest,
            self.median,
            self.channel_scale,
            if self.own_stratum { "yes" } else { "no" },
            self.loudest,
            self.peak_risk,
            self.suspicious,
            self.cells_flagged,
            self.findings
        );
        line
    }
}

fn measure(case: &'static str, tail: &str) -> Reading {
    let source = model_source(tail);
    let compiled = compile("scale_probe.ap", &source);
    assert!(
        !compiled.diagnostics.has_errors(),
        "{case}: {}",
        compiled.diagnostics
    );
    let campaign = run(
        &compiled.model,
        Config {
            budget: 400,
            strategy: Strategy::Adaptive,
            seed: 1,
            // Every base point is swapped: the artifact is rare, and a rate that sampled it once in
            // forty would be measuring the sampler rather than the channel.
            symmetric_every: 1,
            ..Config::default()
        },
    );

    // Only the declared relation's own evidence. The model has a `require` too, and mixing the two
    // channels in one table would report Physical's behaviour as if it were the symmetry's.
    let relation = compiled
        .model
        .relations
        .iter()
        .find(|r| matches!(r.kind, RelationKind::Symmetric { .. }))
        .expect("the model declares the symmetry the campaign is measuring");
    let key = Subject::Relation(relation.id).key();
    let mut magnitudes: Vec<f64> = campaign
        .evidence
        .iter()
        .filter(|e| e.channel == Channel::Behavioral && e.subject.key() == key)
        .map(|e| e.magnitude)
        .collect();
    magnitudes.sort_by(|x, y| x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal));

    let calibrator = &campaign.calibrator;
    let loudest = campaign
        .evidence
        .iter()
        .filter(|e| e.channel == Channel::Behavioral && e.subject.key() == key)
        .map(|e| calibrator.strength_of(e))
        .fold(0.0f64, f64::max);
    let coverage = campaign.atlas.coverage();
    let flagged = (coverage.suspicious * coverage.cells as f64).round() as usize;

    Reading {
        case,
        swaps: campaign.probes.swaps.len(),
        items: magnitudes.len(),
        largest: magnitudes.last().copied().unwrap_or(0.0),
        median: median(&magnitudes),
        channel_scale: calibrator.scale_of(Channel::Behavioral),
        own_stratum: calibrator.has_claim_scale(Channel::Behavioral, &key),
        loudest,
        suspicious: coverage.suspicious,
        cells_flagged: flagged,
        findings: campaign.findings.len(),
        peak_risk: campaign
            .final_risk
            .iter()
            .fold(0.0f64, |acc, r| if r > &acc { *r } else { acc }),
    }
}

#[test]
fn a_last_ulp_artifact_does_not_become_suspicion_and_an_obvious_violation_does_not_vanish() {
    let exact = measure("exact", "0.3 * (a * b)");
    let reassociation = measure("reassociation", "0.3 * a * b");
    let tiny = measure("eps 1e-14", "0.3 * (a * b) + 1e-14 * (a - b)");
    let small = measure("eps 1e-10", "0.3 * (a * b) + 1e-10 * (a - b)");
    let obvious = measure("eps 1e-3", "0.3 * (a * b) + 1e-3 * (a - b)");

    let rows = [&exact, &reassociation, &tiny, &small, &obvious];
    println!(
        "\nBehavioral channel scale, measured per case (budget 400, seed 1, symmetric_every 1):"
    );
    for r in rows {
        println!("  {}", r.row());
    }

    // Both artifacts must be silent. A model whose declared symmetry is mathematically true must not
    // acquire a suspicious region because the evaluator rounds, whatever the fitted scale ended up as.
    for artifact in [&exact, &reassociation] {
        assert!(
            artifact.largest < 1e-15,
            "{} produced a relative asymmetry of {:e}, which is not a last-ulp artifact",
            artifact.case,
            artifact.largest
        );
        assert_eq!(
            artifact.cells_flagged, 0,
            "{} flagged {} cell(s) on an arithmetic artifact; loudest calibrated strength {:e}, \
             channel scale {:e}",
            artifact.case, artifact.cells_flagged, artifact.loudest, artifact.channel_scale
        );
    }
    // The violation a reader would want to know about is not missed by the *evidence* stage: both
    // real violations saturate the strength. They are compared against a floor rather than against
    // each other because `1 - exp(-x)` saturates: `small` and `obvious` differ in the last digits of
    // 1.0, and an assertion that they must be strictly ordered would be testing the exponential, not
    // the instrument.
    assert!(
        obvious.largest > 1e-6,
        "a 1e-3 asymmetry measured as {:e}",
        obvious.largest
    );
    assert!(
        small.loudest > 0.9 && obvious.loudest > 0.9,
        "a real violation did not reach evidence strength: {} / {}",
        small.row(),
        obvious.row()
    );
    // The 1e-14 case is the one the guard decides. Below the guard it is scored as round-off, which is
    // the guard doing its job -- and it is also why a genuine violation that small is invisible, which
    // is recorded rather than glossed.
    assert_eq!(
        tiny.loudest,
        0.0,
        "the round-off-scale violation was not calibrated away: {}",
        tiny.row()
    );
    assert_eq!(
        tiny.channel_scale,
        aporia_evidence::MIN_SCALE,
        "the channel scale was expected to sit on the guard, and it did not"
    );
}

#[test]
fn the_guard_sits_four_orders_above_what_reassociation_can_produce() {
    // The claim the open question turns on: a ulp artifact leaves the Behavioral channel reference on
    // the `MIN_SCALE` guard, so the guard is only safe if a real reassociation artifact cannot reach
    // it. Measured across a whole campaign rather than assumed.
    //
    // The artifact is also *rare*: at this seed and budget the un-parenthesised product produced no
    // differing swap at all, which is the honest version of the corpus's note ("159 of 160 swaps were
    // bit-identical"). A test that required an artifact to appear would be testing the sampler.
    let reassociation = measure("reassociation", "0.3 * a * b");
    assert!(
        reassociation.largest < aporia_evidence::MIN_SCALE / 1e4,
        "a reassociation artifact reached {:e}, within four orders of the {:e} guard",
        reassociation.largest,
        aporia_evidence::MIN_SCALE
    );
    assert_eq!(reassociation.cells_flagged, 0);
    assert_eq!(reassociation.findings, 0, "{}", reassociation.row());

    // And where the guard is actually load-bearing -- a claim whose own measurements all sit below it
    // -- the fitted reference is the guard and nothing else.
    let tiny = measure("eps 1e-14", "0.3 * (a * b) + 1e-14 * (a - b)");
    assert!(
        tiny.median < aporia_evidence::MIN_SCALE,
        "this case stopped exercising the guard: median {:e}",
        tiny.median
    );
    assert_eq!(tiny.channel_scale, aporia_evidence::MIN_SCALE);
    assert_eq!(tiny.loudest, 0.0, "{}", tiny.row());

    // One scale above, the median clears the guard and the channel reference becomes the campaign's
    // own measurement again -- the guard stops being the answer.
    let small = measure("eps 1e-10", "0.3 * (a * b) + 1e-10 * (a - b)");
    assert!(
        small.median > small.channel_scale * 0.99
            && small.channel_scale > aporia_evidence::MIN_SCALE,
        "expected the fitted median to replace the guard: {}",
        small.row()
    );
}

#[test]
fn one_relation_violated_everywhere_still_needs_a_second_channel_to_be_suspicious() {
    // The consequence this measurement turned up, pinned as tested behaviour rather than left as a
    // surprise: suspicion needs corroboration from two channels, and a `check symmetric(...)` is one
    // channel. So a model whose *only* problem is that its declared symmetry fails -- at 97 of 100
    // sampled swaps, with the evidence strength saturating at 1.0 and the peak risk above the
    // suspicious mean -- leaves every cell UNKNOWN, not SUSPICIOUS.
    //
    // This is the corroboration rule applied consistently, not an oversight: a single channel's opinion
    // is one opinion. But it is also a real limit on what the instrument can report, and it was not
    // written down before this run. The assertions here are on the measured outcome so that any future
    // change to the corroboration rule has to come through this test on purpose, with a decision
    // recorded, rather than arriving as a side effect.
    let obvious = measure("eps 1e-3", "0.3 * (a * b) + 1e-3 * (a - b)");
    assert_eq!(obvious.items, 97, "{}", obvious.row());
    assert!(
        obvious.loudest > 0.99,
        "the strength should saturate: {}",
        obvious.row()
    );
    assert!(
        obvious.peak_risk >= 0.45,
        "a violation this large should raise the risk score past the suspicious mean: {}",
        obvious.row()
    );
    assert_eq!(
        obvious.suspicious, 0.0,
        "a single channel became suspicious, which the corroboration rule forbids"
    );
    assert_eq!(obvious.cells_flagged, 0, "{}", obvious.row());
    assert_eq!(
        obvious.findings, 0,
        "a pure symmetry violation produced a finding, contradicting the rule above"
    );
}

#[test]
fn a_uniform_violation_earns_its_own_ruler_and_scores_itself_down() {
    // The second consequence nobody wrote down: a relation violated at *every* sampled point gets
    // eight or more measurements of its own, so `fit` gives the claim a stratum whose scale is the
    // median of its own magnitudes. A constant violation therefore measures itself against itself, and
    // only the points worse than typical count as excess. The stratum rule and the guard interact: at
    // 1e-10 the claim's own median (4.14e-12) is above the guard, so the stratum is the median; at
    // 1e-14 it is below, so the guard wins and everything scores zero.
    let small = measure("eps 1e-10", "0.3 * (a * b) + 1e-10 * (a - b)");
    assert!(
        small.items >= 8 && small.own_stratum,
        "expected a stratum from {} items: {}",
        small.items,
        small.row()
    );
    // The stratum is the claim's own median, not the channel default and not the guard.
    assert!(
        (small.channel_scale - small.median).abs() < f64::EPSILON.max(small.median * 1e-9),
        "channel scale {:e} should equal the measured median {:e}",
        small.channel_scale,
        small.median
    );
    // Half the items are at or below their own median, so half of a genuine violation scores zero
    // excess. That is the rule working as designed and it is why `loudest` (a maximum over the
    // campaign) is the number to read, not a mean.
    assert!(small.loudest > 0.9, "{}", small.row());
    let exact = measure("exact", "0.3 * (a * b)");
    assert_eq!(exact.cells_flagged, 0, "{}", exact.row());
    assert_eq!(exact.items, 0, "{}", exact.row());
}
