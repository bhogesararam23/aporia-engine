//! What the instrument owes a discrete axis: only values the model declared, and an honest address
//! for each of them.
//!
//! No committed corpus entry had a `Choices` axis before E1.2, which is why these behaviours were
//! never exercised: a probe that invented a value between two branches, and a unit-space address that
//! said `0.5` for every choice, were both invisible. They are not invisible now — the model under test
//! is a branch table, so an off-grid value is a branch that does not exist, and a mis-aimed
//! refinement spends charged evaluations pointing at a leaf it never reaches.

use aporia_dsl::lower::compile;
use aporia_ir::Model;
use aporia_search::{Campaign, Config, Strategy, campaign::perturb, run, to_parameters, to_unit};

fn model(text: &str) -> Model {
    let c = compile("t.ap", text);
    assert!(!c.diagnostics.has_errors(), "{}\n{text}", c.diagnostics);
    c.model
}

const BRANCH: &str = "model branch \"\" {\n  input scheme in {1, 2, 4, 8}\n  input dt in [0.01, 1]\n  \
                       let growth = 1 - scheme * dt\n  require growth > -1\n}\n";

fn choices(model: &Model, axis: usize) -> Vec<f64> {
    use aporia_ir::Domain;
    match &model.params[axis].domain {
        Domain::Choices(v) => v.clone(),
        Domain::Interval { .. } => panic!("axis {axis} is not a discrete one"),
    }
}

#[test]
fn a_discrete_perturbation_lands_on_a_declared_choice() {
    let m = model(BRANCH);
    let values = choices(&m, 0);
    for at in 0..values.len() {
        let base = [values[at], 0.5];
        let (y, step) = perturb(&m, &base, 0).expect("four choices, so a neighbour exists");
        assert!(
            values.contains(&y[0]),
            "the probe produced scheme {}, which {BRANCH:?} never declares",
            y[0]
        );
        assert_ne!(y[0], values[at], "a perturbation that changes nothing");
        assert!(step > 0.0 && step <= 1.0, "relative step {step}");
        // The step is the gap it actually moved, in the span the atlas partitions.
        let width = values.last().copied().unwrap() - values.first().copied().unwrap();
        assert!(
            (step - ((y[0] - base[0]).abs() / width)).abs() < 1e-12,
            "{step}"
        );
    }
}

#[test]
fn a_choice_set_with_one_value_cannot_be_perturbed() {
    let m = model(
        "model one \"\" {
  input mode in {3, 3}
  input x in [0, 1]
  let y = x * mode
           require y < 1
}
",
    );
    // A duplicate list is a single distinct choice, and there is no neighbour to move to. Saying so is
    // better than returning the same point and calling it a probe.
    assert!(perturb(&m, &[3.0, 0.5], 0).is_none());
}

#[test]
fn a_choices_unit_address_round_trips_to_the_value_it_came_from() {
    let m = model(BRANCH);
    let values = choices(&m, 0);
    for (at, value) in values.iter().enumerate() {
        let x = [*value, 0.4];
        let unit = to_unit(&m, &x);
        let back = to_parameters(&m, &unit);
        assert_eq!(
            back[0], *value,
            "choice {value} at index {at} addressed as {} came back as {}",
            unit[0], back[0]
        );
        assert!(
            (unit[0] - (at as f64 + 0.5) / values.len() as f64).abs() < 1e-12,
            "the address should be the centre of the choice's own slice"
        );
    }
    // And the continuous axis is unaffected by any of this.
    let unit = to_unit(&m, &[2.0, 0.5]);
    assert!((unit[1] - (0.5 - 0.01) / (1.0 - 0.01)).abs() < 1e-12);
}

#[test]
fn a_discrete_axis_aims_refinements_at_the_leaf_it_names() {
    // The bug this pins: with `to_unit` answering 0.5 for every choice, a point aimed at a cell in
    // the upper part of a discrete axis was quantised back to a choice in the middle of the set, so
    // the search repeatedly spent evaluations believing it was probing a different leaf.
    let m = model(BRANCH);
    let values = choices(&m, 0);
    for target in &values {
        let aimed = to_parameters(&m, &to_unit(&m, &[*target, 0.9]));
        assert_eq!(
            aimed[0], *target,
            "aiming at choice {target} landed on {}",
            aimed[0]
        );
    }
}

#[test]
fn a_campaign_over_a_discrete_axis_never_visits_an_undeclared_value() {
    // The real guarantee, end to end: whatever the strategy, every point the model was charged for
    // sits on a branch the model declares, and every one of those points reached a cell of the
    // atlas. This is 0029's E12 metric — unplaced measurements must be zero — read off a run rather
    // than assumed from the mask.
    let m = model(BRANCH);
    let values = choices(&m, 0);
    for strategy in Strategy::ALL {
        let config = Config {
            budget: 320,
            strategy,
            seed: 1,
            ..Config::default()
        };
        let campaign: Campaign = run(&m, config);
        assert!(
            campaign.records.items.len() > 10,
            "{strategy:?} barely sampled"
        );
        for record in &campaign.records.items {
            let scheme = record.x[0];
            assert!(
                values.contains(&scheme),
                "{strategy:?} evaluated scheme {scheme}, which the model does not declare:                  {values:?}"
            );
        }
        assert_eq!(
            campaign.atlas.coverage().unplaced,
            0,
            "{strategy:?} left measurements the partition could not place"
        );
    }
}

#[test]
fn sensitivity_probes_on_a_discrete_axis_are_two_real_branches() {
    // The pair a discrete probe produces has to be evaluable on both sides, and its relative step is
    // one choice-gap — the honest size, and the reason a slope on a discrete axis is not comparable
    // with a slope on a continuous one.
    let m = model(BRANCH);
    let values = choices(&m, 0);
    let (a, step) = perturb(&m, &[values[0], 0.1], 0).expect("a neighbour exists");
    let b = perturb(&m, &[values[1], 0.1], 0)
        .expect("a neighbour exists")
        .0;
    assert_eq!(a[0], values[1], "from the first choice the only move is up");
    assert_eq!(b[0], values[0], "from the second choice the move is down");
    let gap = values[1] - values[0];
    let width = values.last().copied().unwrap() - values.first().copied().unwrap();
    assert!(
        (step - gap / width).abs() < 1e-12,
        "{step} vs {gap}/{width}"
    );
}
