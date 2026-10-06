//! The controlled suite for APORIA's level-set baseline: four small models, and the claims the
//! baseline has to keep true before any of its numbers are worth comparing with anything.
//!
//! These are not benchmark runs. They are the checks that make the pilot comparison meaningful:
//! that the arm reads one scalar, cuts where that scalar changes sign, spends exactly what it is
//! given, repeats itself on the same seed, and — on a model with no transition — does something
//! honest instead of drawing a boundary that is not there.

use aporia_dsl::lower::compile;
use aporia_ir::Model;
use aporia_runtime::value::{ExecConfig, Flags};
use aporia_runtime::{Executor, Outcome};
use aporia_search::{Config, Strategy, run, run_with};

fn model(text: &str) -> Model {
    let c = compile("t.ap", text);
    assert!(!c.diagnostics.has_errors(), "{}\n{text}", c.diagnostics);
    c.model
}

fn cfg(strategy: Strategy, budget: u64, seed: u64) -> Config {
    Config {
        budget,
        seed,
        strategy,
        // Probes belong to the instrument, and this suite is about placement. Turning them off keeps a
        // failure here pointing at the strategy rather than at the evidence machinery it shares with
        // every other arm.
        probe_every: 0,
        symmetric_every: 0,
        ..Config::default()
    }
}

/// A one-dimensional crossing at `x = 0.7`, declared as the model's own rule.
const THRESHOLD: &str =
    "model threshold \"\" {\n input x in [0.0, 1.0]\n let y = x - 0.7\n require y >= 0\n}\n";

/// The same shape with no crossing anywhere in the declared domain: the rule holds at every point.
const CONTROL: &str =
    "model control \"\" {\n input x in [0.0, 1.0]\n let y = x * x\n require y >= 0\n}\n";

/// Two crossings, four hundredths apart — the narrow-region shape the ladder already knows loses on.
const NARROW: &str = "model narrow \"\" {\n input x in [0.0, 1.0]\n let lo = x - 0.48\n let hi = 0.52 - x\n require lo >= 0\n require hi >= 0\n}\n";

/// A diagonal boundary in two dimensions.
const PLANE: &str = "model plane \"\" {\n input a in [0.0, 10.0]\n input b in [1.0, 10.0]\n let y = a - 2.0 * b\n require y >= 0\n}\n";

#[test]
fn the_bracket_closes_on_the_declared_crossing() {
    let m = model(THRESHOLD);
    let c = run(&m, cfg(Strategy::LevelSet, 400, 7));
    let cover = run(&m, cfg(Strategy::Stratified, 400, 7));
    let near = |c: &aporia_search::Campaign, tol: f64| {
        c.decisions
            .iter()
            .filter(|d| (d.x[0] - 0.7).abs() <= tol)
            .count()
    };
    // Every placed point sits inside the declared domain, and the arm is not merely sampling it: on the
    // line the model declared it puts more points than the same seed's coverage design does. Scored
    // against that design rather than against a figure invented here, because the claim a baseline has
    // to support is "I concentrate where this model changes", and that is a comparison.
    assert!(
        c.decisions.iter().all(|d| (0.0..=1.0).contains(&d.x[0])),
        "a placed point left the declared domain"
    );
    let (here, there) = (near(&c, 0.01), near(&cover, 0.01));
    assert!(
        here as f64 >= 4.0 * there.max(1) as f64,
        "{here} of {} points within 0.01 of the crossing, where coverage placed {there}",
        c.decisions.len()
    );
    let first = (c.decisions[0].x[0] - 0.7).abs();
    let closest = c
        .decisions
        .iter()
        .map(|d| (d.x[0] - 0.7).abs())
        .fold(f64::INFINITY, f64::min);
    // Two orders of magnitude tighter than where the run started. How tight is bounded from below by
    // the atlas rather than by the bisection: a bracket is only worth cutting while its leaf still has
    // width to split, and the labelling policy's `max_depth` is where that stops. Measured at 7.8e-4,
    // which is one leaf of a depth-10 partition of this domain.
    assert!(
        closest * 100.0 < first,
        "the frontier closed only to {closest}, having started at {first}"
    );
    assert!(
        closest < 1e-3,
        "the frontier never got closer than {closest}"
    );
}

#[test]
fn a_model_with_no_transition_never_gets_an_invented_boundary() {
    // The important half of the baseline: with nothing straddling the level, the arm must fall back to
    // the coverage design it started from — which is exactly the trajectory `stratified` follows from
    // the same seed. If LevelSet cut a bracket here it would be searching for a boundary this model
    // does not have, and E1 would be comparing a search against a fiction.
    let m = model(CONTROL);
    let level = run(&m, cfg(Strategy::LevelSet, 320, 3));
    let stratified = run(&m, cfg(Strategy::Stratified, 320, 3));
    assert_eq!(
        level
            .decisions
            .iter()
            .map(|d| d.x.clone())
            .collect::<Vec<_>>(),
        stratified
            .decisions
            .iter()
            .map(|d| d.x.clone())
            .collect::<Vec<_>>(),
        "a run with no level crossing placed points that were not pure coverage"
    );
    assert!(
        level.findings.is_empty(),
        "the control was reported as a region anyway"
    );
}

#[test]
fn two_close_crossings_are_both_cut() {
    // One bracket bisected to death would look like success on this model and still miss half the
    // region: the failure set is the band between 0.48 and 0.52, and both of its edges are transitions.
    // Choosing the coarsest mixed leaf rather than the nearest point is what makes this happen.
    let m = model(NARROW);
    let c = run(&m, cfg(Strategy::LevelSet, 600, 5));
    let edge = |e: f64| {
        c.decisions
            .iter()
            .filter(|d| (d.x[0] - e).abs() <= 0.01)
            .count()
    };
    assert!(
        edge(0.48) > 5,
        "the lower edge was cut {} times",
        edge(0.48)
    );
    assert!(
        edge(0.52) > 5,
        "the upper edge was cut {} times",
        edge(0.52)
    );
    assert!(
        c.decisions.iter().any(|d| d.x[0] > 0.48 && d.x[0] < 0.52),
        "nothing was ever placed inside the declared band"
    );
}

#[test]
fn a_diagonal_boundary_is_localised_in_two_dimensions() {
    let m = model(PLANE);
    let c = run(&m, cfg(Strategy::LevelSet, 600, 9));
    let cover = run(&m, cfg(Strategy::Stratified, 600, 9));
    // The declared line is a = 2b. Scored as distance to it, not as a cell label, because the label
    // comes from the instrument and this test is about where the arm chose to put points.
    let distance = |d: &[f64]| (d[0] - 2.0 * d[1]).abs() / 10.0;
    let first = distance(&c.decisions[0].x);
    let closest = c
        .decisions
        .iter()
        .map(|d| distance(&d.x))
        .fold(f64::MAX, f64::min);
    assert!(
        closest * 50.0 < first,
        "the best point was {closest} from the line, having started at {first}"
    );
    let band =
        |c: &aporia_search::Campaign| c.decisions.iter().filter(|d| distance(&d.x) < 0.01).count();
    let (here, there) = (band(&c), band(&cover));
    assert!(
        here as f64 >= 2.0 * there.max(1) as f64,
        "{here} points sat within 0.01 of the diagonal, where coverage placed {there}"
    );
}

#[test]
fn the_baseline_spends_its_budget_and_repeats_itself_exactly() {
    let m = model(THRESHOLD);
    for budget in [120u64, 400] {
        let once = run(&m, cfg(Strategy::LevelSet, budget, 21));
        let twice = run(&m, cfg(Strategy::LevelSet, budget, 21));
        assert_eq!(
            once.evaluations, twice.evaluations,
            "{budget} was spent twice"
        );
        assert_eq!(
            once.instruction_steps, twice.instruction_steps,
            "{budget} cost twice"
        );
        assert_eq!(
            once.decisions
                .iter()
                .map(|d| d.x.clone())
                .collect::<Vec<_>>(),
            twice
                .decisions
                .iter()
                .map(|d| d.x.clone())
                .collect::<Vec<_>>(),
            "the same seed placed different points"
        );
        assert!(
            once.evaluations <= budget,
            "the budget was overspent: {} > {budget}",
            once.evaluations
        );
        assert_eq!(
            once.records.len() as u64,
            once.evaluations,
            "a charged evaluation was not a recorded execution"
        );
    }
}

#[test]
fn the_level_of_a_foreign_program_is_that_programs_answer_and_no_other() {
    // A program that deflects backwards past 60, executed by the adapter path. The A-IR for this model
    // declares no arithmetic at all, so the only way the baseline can find the crossing is by reading
    // the numbers the program sent back. If the level came from anywhere else, no bracket would ever
    // form and the run would be indistinguishable from the coverage design below — which is why this
    // test measures the arm against that design instead of against a number.
    struct Beam;

    impl Executor for Beam {
        fn execute(&mut self, _model: &Model, x: &[f64], _cfg: ExecConfig) -> Outcome {
            let load = x.first().copied().unwrap_or(0.0);
            Outcome {
                outputs: vec![if load > 60.0 { -1.4 } else { 2.1 }],
                traces: Vec::new(),
                flags: Flags::default(),
                steps: 0,
                rule_values: Vec::new(),
            }
        }
    }

    let m = model(
        "model beam \"\" {\n input load : N in [0, 100]\n output deflection : mm\n require deflection >= 0\n}\n",
    );
    let c = run_with(&m, cfg(Strategy::LevelSet, 400, 13), &mut Beam);
    let cover = run_with(&m, cfg(Strategy::Stratified, 400, 13), &mut Beam);
    let near = |c: &aporia_search::Campaign| {
        c.decisions
            .iter()
            .filter(|d| (d.x[0] - 60.0).abs() <= 1.0)
            .count()
    };
    let (here, there) = (near(&c), near(&cover));
    assert!(
        here as f64 >= 4.0 * there.max(1) as f64,
        "{here} of {} points came near the crossing the program computes, where coverage placed {there}",
        c.decisions.len()
    );
    let closest = c
        .decisions
        .iter()
        .map(|d| (d.x[0] - 60.0).abs())
        .fold(f64::INFINITY, f64::min);
    assert!(
        closest < 0.5,
        "the bracket never tightened on the program's own boundary: {closest}"
    );
    // And the map agrees with what was found, because the instrument still produced the labels.
    assert!(
        !c.findings.is_empty(),
        "a violated program rule produced no finding at all"
    );
}
