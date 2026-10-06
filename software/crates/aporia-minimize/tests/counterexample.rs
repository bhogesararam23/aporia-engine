//! Minimisation, exercised the way a caller would use it: through the public API only.
//!
//! Two kinds of test live here. The synthetic ones put the failure in the oracle and keep the model
//! empty, so what is being checked is the algorithm — which parameters survive, how tight an
//! interval gets, how few digits hold. The model ones compile real `.ap` sources and drive the
//! physical-channel oracle, so what is being checked is that the thing being preserved is a fact
//! about a model rather than a predicate I wrote for the test.

use aporia_dsl::lower::compile;
use aporia_ir::{Dimension, Domain, Model, NumType, Param, Ty};
use aporia_minimize::{AxisState, Case, Config, FailureOracle, Oracle, Verdict, minimize};

/// A model of `arity` dimensionless parameters over `[-2, 2]`, with no computation at all.
fn synthetic(arity: usize) -> Model {
    let mut m = Model::new("s");
    for i in 0..arity {
        m.params.push(Param {
            name: format!("p{i}"),
            ty: Ty {
                num: NumType::F64,
                dim: Dimension::dimensionless(),
            },
            domain: Domain::interval(-2.0, 2.0),
            to_si: 1.0,
            doc: String::new(),
        });
    }
    m
}

fn model(text: &str) -> Model {
    let c = compile("t.ap", text);
    assert!(!c.diagnostics.has_errors(), "{}\n{text}", c.diagnostics);
    c.model
}

/// A synthetic failure lives in the predicate, not in the model, so it executed nothing to answer.
/// Wrapping is two lines and keeps the distinction the crate is careful about — queries against
/// executions — visible in these tests instead of papered over by an assumed cost of one.
fn predicate(
    f: impl Fn(&[f64]) -> bool,
) -> impl Fn(&[f64], &mut dyn aporia_runtime::Executor) -> Verdict {
    move |x: &[f64], _engine| Verdict::new(f(x), 0)
}

#[test]
fn a_failure_needing_three_of_eleven_parameters_minimises_to_three() {
    // The spec's own example (§11): an eleven-parameter failure reduced to what it needs.
    let m = synthetic(11);
    let oracle = predicate(|x: &[f64]| x[0] > 1.0 && x[3] < -1.0 && x[7].abs() > 0.5);
    let start = vec![1.9, 0.4, -0.2, -1.8, 0.0, 1.1, -0.7, 1.4, 0.3, -0.9, 1.2];
    let out = minimize(
        &oracle,
        &mut aporia_runtime::Interp,
        &m,
        &start,
        Config::default(),
    );

    assert!(out.verified, "the final case was not re-verified");
    assert_eq!(out.case.dimensions(), 3, "{}", out.case.describe());
    assert_eq!(out.dropped.len(), 8);
    let kept: Vec<&str> = out
        .case
        .axes
        .iter()
        .filter(|a| !matches!(a.state, AxisState::Dropped))
        .map(|a| a.name.as_str())
        .collect();
    assert_eq!(kept, ["p0", "p3", "p7"], "wrong parameters survived");
}

#[test]
fn a_parameter_the_failure_needs_is_never_dropped() {
    let m = synthetic(2);
    let oracle = predicate(|x: &[f64]| x[0] > 0.0);
    let out = minimize(
        &oracle,
        &mut aporia_runtime::Interp,
        &m,
        &[1.0, 0.0],
        Config::default(),
    );
    assert!(out.verified);
    assert!(!matches!(out.case.axes[0].state, AxisState::Dropped));
    assert!(matches!(out.case.axes[1].state, AxisState::Dropped));
    // The drop is only a claim because every sample behind it actually failed.
    assert!(
        out.case
            .witnesses()
            .iter()
            .all(|x| oracle(x, &mut aporia_runtime::Interp).violating),
        "{:?}",
        out.case.witnesses()
    );
}

#[test]
fn narrowing_finds_the_edge_of_a_one_sided_failure() {
    let m = synthetic(1);
    let oracle = predicate(|x: &[f64]| x[0].abs() < 0.4);
    let out = minimize(
        &oracle,
        &mut aporia_runtime::Interp,
        &m,
        &[0.1],
        Config::default(),
    );
    let AxisState::Band { lo, hi, .. } = out.case.axes[0].state else {
        panic!("expected an interval, got {:?}", out.case.axes[0].state);
    };
    assert!(lo < -0.39 && lo > -0.41, "lower edge {lo}");
    assert!(hi > 0.39 && hi < 0.41, "upper edge {hi}");
}

#[test]
fn digits_shrink_to_the_shortest_interval_that_still_fails() {
    let m = synthetic(1);
    // A region a little wider than 0.06 so the narrowed edges, which sit just inside the true
    // boundary, can be rounded to two digits and stay inside it. Rounding is only ever accepted
    // when the rounded description still verifies, which is the point of the test.
    let oracle = predicate(|x: &[f64]| (x[0] - 0.25).abs() < 0.061);
    let out = minimize(
        &oracle,
        &mut aporia_runtime::Interp,
        &m,
        &[0.249_999_999_999_999_97],
        Config {
            shrink_tol: 1e-6,
            ..Config::default()
        },
    );
    let AxisState::Band { lo, hi, digits } = out.case.axes[0].state else {
        panic!("expected an interval, got {:?}", out.case.axes[0].state);
    };
    assert_eq!(digits, Some(2), "the shortest verified description");
    assert!(
        (lo - 0.25).abs() < 0.061 && (hi - 0.25).abs() < 0.061,
        "{lo}..{hi} is no longer a failure"
    );
    assert!(out.verified);
    assert_eq!(
        out.case.describe(),
        "p0 in [0.19, 0.31]",
        "a report should read like a measurement"
    );
    assert_eq!(out.case.digits(), 4, "two edges at two digits each");
}

#[test]
fn a_failure_that_needs_no_parameters_reports_zero_dimensions() {
    let m = synthetic(4);
    let oracle = predicate(|_x: &[f64]| true);
    let out = minimize(
        &oracle,
        &mut aporia_runtime::Interp,
        &m,
        &[1.0, 1.0, 1.0, 1.0],
        Config::default(),
    );
    assert_eq!(out.case.dimensions(), 0, "{}", out.case.describe());
    assert!(out.verified);
    // The description still says which parameters were checked and found irrelevant, because that
    // is the finding: the model fails wherever its inputs go.
    assert_eq!(out.dropped.len(), 4);
}

#[test]
fn a_start_that_is_not_a_failure_is_said_so_rather_than_minimised() {
    let m = synthetic(2);
    let oracle = predicate(|x: &[f64]| x[0] > 5.0);
    let out = minimize(
        &oracle,
        &mut aporia_runtime::Interp,
        &m,
        &[1.0, 1.0],
        Config::default(),
    );
    assert!(!out.verified);
    assert!(!out.over_budget);
    assert_eq!(out.case.dimensions(), 2, "nothing should have been removed");
}

#[test]
fn running_out_of_budget_is_reported_instead_of_claiming_a_result() {
    let m = synthetic(3);
    let oracle = predicate(|x: &[f64]| x[0] > 0.0);
    let out = minimize(
        &oracle,
        &mut aporia_runtime::Interp,
        &m,
        &[1.0, 0.0, 0.0],
        Config {
            budget: 4,
            ..Config::default()
        },
    );
    assert!(out.over_budget, "four calls cannot sample three domains");
    assert!(!out.verified);
    assert!(out.queries >= 4);
}

#[test]
fn cost_is_attributed_to_oracle_calls_and_nothing_else() {
    use std::cell::Cell;
    let m = synthetic(2);
    let calls = Cell::new(0u64);
    let oracle = |x: &[f64], _engine: &mut dyn aporia_runtime::Executor| {
        calls.set(calls.get() + 1);
        // Nothing executed: the budget and the reported query count are both stated in calls.
        Verdict::new(x[0] > 0.0, 0)
    };
    let out = minimize(
        &oracle,
        &mut aporia_runtime::Interp,
        &m,
        &[1.0, 0.0],
        Config::default(),
    );
    assert_eq!(
        out.queries,
        calls.get(),
        "the reported query count is not the number of oracle calls"
    );
    assert_eq!(
        out.executions, 0,
        "an oracle that ran nothing was charged nothing"
    );
    assert!(out.queries > 0);
}

#[test]
fn an_expensive_oracle_is_charged_for_every_execution_its_answers_used() {
    // The distinction the column used to hide: the same search, the same budget, asked of a
    // predicate that runs the model twice per answer. The budget is still spent in calls, and the
    // executions are twice the calls — so a cost read from `queries` alone understates the work.
    let m = synthetic(2);
    let out = minimize(
        &|x: &[f64], _engine: &mut dyn aporia_runtime::Executor| Verdict::new(x[0] > 0.0, 2),
        &mut aporia_runtime::Interp,
        &m,
        &[1.0, 0.0],
        Config::default(),
    );
    assert!(out.verified, "{}", out.case.describe());
    assert!(out.queries > 0);
    assert_eq!(out.executions, 2 * out.queries);
}

#[test]
fn a_budget_is_spent_in_calls_even_when_a_call_costs_several_executions() {
    // An oracle five times as expensive per answer, asked under a small budget. If the budget were
    // charged in executions the first verification would already be unaffordable and the run would
    // stop being comparable with one that used a cheaper oracle, so the refusal has to come from the
    // call count while the executions are still reported for what they were.
    let m = synthetic(2);
    let out = minimize(
        &|_: &[f64], _engine: &mut dyn aporia_runtime::Executor| Verdict::new(true, 5),
        &mut aporia_runtime::Interp,
        &m,
        &[1.0, 0.0],
        Config {
            budget: 4,
            ..Config::default()
        },
    );
    assert!(out.over_budget);
    assert_eq!(out.executions, 5 * out.queries);
    assert!(
        out.queries > 1,
        "the budget is spent in calls, so a call's price must not stop the search early"
    );
}

#[test]
fn minimisation_never_adds_a_parameter_to_the_description() {
    let m = synthetic(5);
    let oracle = predicate(|x: &[f64]| x[1] > 0.0 && x[4] > 1.0);
    let start = vec![0.0, 1.5, 0.0, 0.0, 1.9];
    let out = minimize(
        &oracle,
        &mut aporia_runtime::Interp,
        &m,
        &start,
        Config::default(),
    );
    assert!(out.case.dimensions() <= Case::from_model(&m, &start).dimensions());
}

#[test]
fn the_same_failure_minimises_the_same_way_twice() {
    let m = synthetic(4);
    let oracle = predicate(|x: &[f64]| x[0] < -1.0 && x[2].abs() > 0.5);
    let start = vec![-1.9, 0.0, 1.4, 0.0];
    let a = minimize(
        &oracle,
        &mut aporia_runtime::Interp,
        &m,
        &start,
        Config::default(),
    );
    let b = minimize(
        &oracle,
        &mut aporia_runtime::Interp,
        &m,
        &start,
        Config::default(),
    );
    assert_eq!(a.case, b.case, "minimisation is not deterministic");
    assert_eq!(a.queries, b.queries);
    assert_eq!(a.executions, b.executions);
}

#[test]
fn the_rule_oracle_charges_exactly_one_execution_per_answer() {
    // `FailureOracle` asks the model one question per point, so its two cost numbers must agree, and
    // a report that prints either one says the same thing. The risk oracle is where they part, and
    // that is measured in `aporia-bench/tests/risk.rs` because it needs a finished campaign.
    let m =
        model("model s \"\" {\n input x in [-10, 10]\n let y = sqrt(x)\n require finite(y)\n}\n");
    let oracle = FailureOracle::new(&m);
    let out = minimize(
        &oracle,
        &mut aporia_runtime::Interp,
        &m,
        &[-9.0],
        Config::default(),
    );
    assert!(out.verified, "{}", out.case.describe());
    assert_eq!(
        out.executions, out.queries,
        "one question per point means one execution per point"
    );
    // And a point the model cannot be asked about at all is refused without spending anything.
    let refused = oracle.query(&[1.0, 2.0], &mut aporia_runtime::Interp);
    assert!(
        !refused.violating && refused.executions == 0,
        "an arity mismatch ran the model: {refused:?}"
    );
}

#[test]
fn a_declared_rule_violation_minimises_to_the_parameter_that_breaks_it() {
    let m = model(
        "model g \"\" {\n input v : m/s in [0, 1000]\n input g : m/s^2 in [-20, 20]\n let r = v * v / g\n require g > 0\n}\n",
    );
    let oracle = FailureOracle::new(&m);
    let out = minimize(
        &oracle,
        &mut aporia_runtime::Interp,
        &m,
        &[100.0, -5.0],
        Config::default(),
    );
    assert!(out.verified, "the minimal case stopped failing");
    assert_eq!(
        out.dropped,
        vec!["v".to_string()],
        "{}",
        out.case.describe()
    );
    // The surviving claim is about g, and its upper edge is the rule's own boundary.
    let AxisState::Band { lo, hi, .. } = out.case.axes[1].state else {
        panic!(
            "expected an interval on g, got {:?}",
            out.case.axes[1].state
        );
    };
    assert!(lo < -19.0, "lower edge {lo} did not reach the domain bound");
    assert!(hi.abs() < 0.01, "upper edge {hi} is not near the boundary");
}

#[test]
fn a_failure_that_needs_a_parameter_to_be_nonzero_keeps_that_parameter() {
    // The same model with the rule on the *output*. With g negative, r is negative for any nonzero
    // v — but at v = 0 it is -0.0, which satisfies `r >= 0`. So velocity really is part of the
    // failure and ddmin must not remove it, while the honest description is "nonzero velocity"
    // rather than "velocity 100".
    let m = model(
        "model h \"\" {\n input v : m/s in [0, 1000]\n input g : m/s^2 in [-20, 20]\n let r = v * v / g\n require r >= 0\n}\n",
    );
    let oracle = FailureOracle::new(&m);
    let out = minimize(
        &oracle,
        &mut aporia_runtime::Interp,
        &m,
        &[100.0, -5.0],
        Config::default(),
    );
    assert!(out.verified, "{}", out.case.describe());
    assert!(
        out.dropped.is_empty(),
        "nothing here is irrelevant: {}",
        out.case.describe()
    );
    let AxisState::Band { lo, .. } = out.case.axes[0].state else {
        panic!(
            "expected a narrowed velocity, got {:?}",
            out.case.axes[0].state
        );
    };
    assert!(lo > 0.0, "lower edge {lo} should exclude the safe zero");
}

#[test]
fn a_divergence_minimises_to_the_interval_where_the_model_leaves_the_reals() {
    let m =
        model("model s \"\" {\n input x in [-10, 10]\n let y = sqrt(x)\n require finite(y)\n}\n");
    let oracle = FailureOracle::new(&m);
    let out = minimize(
        &oracle,
        &mut aporia_runtime::Interp,
        &m,
        &[-9.0],
        Config::default(),
    );
    assert!(out.verified);
    let AxisState::Band { lo, hi, .. } = out.case.axes[0].state else {
        panic!("expected an interval, got {:?}", out.case.axes[0].state);
    };
    assert_eq!(lo, -10.0, "the failure reaches the declared bound");
    assert!(
        hi < 0.0 && hi > -0.01,
        "the boundary at zero came out as {hi}"
    );
}

#[test]
fn a_discrete_parameter_is_dropped_by_testing_every_choice() {
    let m = model(
        "model d \"\" {\n input scheme in {1, 2, 3}\n input dt : s in [0.001, 0.2]\n let y = dt * dt\n require y > 0.01\n}\n",
    );
    let oracle = FailureOracle::new(&m);
    // dt = 0.05 gives y = 0.0025, which violates; the scheme has nothing to do with it.
    let out = minimize(
        &oracle,
        &mut aporia_runtime::Interp,
        &m,
        &[2.0, 0.05],
        Config::default(),
    );
    assert!(out.verified, "{}", out.case.describe());
    assert!(
        matches!(out.case.axes[0].state, AxisState::Dropped),
        "the choice list was not dropped: {}",
        out.case.describe()
    );
    // Every choice was executed, because that is what dropping a discrete axis claims.
    let seen: Vec<f64> = out.case.witnesses().iter().map(|x| x[0]).collect();
    for choice in [1.0, 2.0, 3.0] {
        assert!(
            seen.iter().any(|v| (v - choice).abs() < 1e-12),
            "choice {choice} was never executed: {seen:?}"
        );
    }
}

#[test]
fn an_interval_that_collapsed_is_printed_as_a_value() {
    let m = synthetic(1);
    // The failure needs this exact point and nowhere near it, so no band can be verified and the
    // description must stay a value rather than a degenerate `x in [0.4, 0.4]`.
    let oracle = predicate(|x: &[f64]| (x[0] - 0.4).abs() < 1e-12);
    let out = minimize(
        &oracle,
        &mut aporia_runtime::Interp,
        &m,
        &[0.4],
        Config {
            shrink_tol: 1e-9,
            ..Config::default()
        },
    );
    assert!(matches!(out.case.axes[0].state, AxisState::Pinned { .. }));
    assert!(
        !out.case.describe().contains(" in ["),
        "{}",
        out.case.describe()
    );
}
