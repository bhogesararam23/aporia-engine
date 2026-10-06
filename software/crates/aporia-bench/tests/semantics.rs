//! Semantics of a lowered model, checked by running it.
//!
//! The structural tests in `aporia-dsl` prove a loop temporary became its own slot. This proves the
//! thing a reader actually cares about: that a model written as explicit Euler *computes* explicit
//! Euler, and that the integrator entries in the corpus flip at the step size their derivation
//! claims. Those two claims were both false before the step-temporary fix, and no structural test
//! would have noticed.

use aporia_bench::corpus::{scan_axis, violates};
use aporia_dsl::lower::compile;
use aporia_ir::Model;
use aporia_runtime::{ExecConfig, interp};

fn model(src: &str) -> Model {
    let c = compile("t.ap", src);
    assert!(
        !c.diagnostics.has_errors(),
        "{}\n{src}",
        c.diagnostics
            .render_all(&aporia_dsl::span::Source::new("t.ap", src.to_string()))
    );
    c.model
}

#[test]
fn explicit_euler_computes_explicit_euler() {
    // Two steps of x' = v, v' = -x with dt = 0.5 and the value captured before each advance:
    //   step 1: x = 1 + 0   *0.5 = 1.0,   v = 0 - 1.0*0.5 = -0.5
    //   step 2: x = 1 - 0.5 *0.5 = 0.75,  v = -0.5 - 1.0*0.5 = -1.0
    // If `let x0 = x` were re-read at the use site instead of bound here, x would be 0.875 and v
    // would be -0.75, which is the semi-implicit scheme, not this one.
    let m = model(
        "model e \"\" {\n input dt in [0, 1]\n state x = 1.0\n state v = 0.0\n loop 2 {\n let x0 = x\n advance x = x + v * dt\n advance v = v - x0 * dt\n }\n let fx = x\n let fv = v\n}\n",
    );
    let out = interp::run(&m, &[0.5], ExecConfig::default());
    assert!(
        (out.outputs[0] - 0.75).abs() < 1e-12,
        "x after two steps: {:?}, expected 0.75",
        out.outputs
    );
    assert!(
        (out.outputs[1] - (-1.0)).abs() < 1e-12,
        "v after two steps: {:?}",
        out.outputs
    );
}

#[test]
fn the_naive_integrator_entry_flips_near_its_derived_step_size() {
    let src = include_str!("../../../benchmarks/control/naive_euler_spring/model.ap");
    let m = model(src.trim_end());
    let crossings = scan_axis(&m, 0, 4000, 1e-6);
    assert_eq!(crossings.len(), 1, "one rule, one crossing: {crossings:?}");
    let at = crossings[0];
    assert!(
        (at - 0.0997).abs() < 0.002,
        "measured crossing {at}, declared boundary 0.0997"
    );
    // The declared box has to start above the crossing, or the entry would be claiming a region the
    // model does not have.
    assert!(
        !violates(&m, &[at - 0.001]),
        "just below the crossing the rule should still hold"
    );
    assert!(violates(&m, &[at + 0.001]), "just above it should not");
}

#[test]
fn the_symplectic_twin_stays_clean_over_its_whole_domain() {
    let src = include_str!("../../../benchmarks/control/symplectic_spring/model.ap");
    let m = model(src.trim_end());
    for k in 0..=300 {
        let dt = 0.001 + (0.3 - 0.001) * k as f64 / 300.0;
        assert!(
            !violates(&m, &[dt]),
            "the stable ordering failed at dt = {dt}, which would make this entry no longer a control"
        );
    }
}

/// One measured outcome, with every field that is not the point of this test set to something a real
/// campaign could have produced.
fn outcome(entry: &str, strategy: &'static str) -> aporia_bench::metrics::Outcome {
    aporia_bench::metrics::Outcome {
        entry: entry.to_string(),
        family: "synthetic".into(),
        fault: "declared_well".into(),
        control: false,
        strategy,
        seed: 1,
        budget: 80,
        evaluations: 80,
        instruction_steps: 1_600,
        wall_ms: 0,
        arity: 1,
        declared_regions: 1,
        detected_regions: 1,
        localised_regions: 1,
        first_true_failure: Some(3),
        false_positive_fraction: Some(0.0),
        suspicious_volume: 0.1,
        trusted_volume: 0.8,
        unknown_volume: 0.1,
        findings: 1,
        duplicates: Some(0.0),
        boundaries: Vec::new(),
        counterexamples: Vec::new(),
        replay: None,
        replay_error: None,
    }
}

#[test]
fn the_comparison_block_reports_every_arm_the_batch_measured() {
    // Written because this block used to be built from three strategy names written out in the code
    // that printed it. A plan that measured a fourth arm therefore produced a results document whose
    // comparison described three of the four arms it had actually run — and nothing failed, because
    // the three it did describe were all correct.
    use aporia_bench::metrics::compare;
    // `compare` aggregates a whole run, so the arms have to arrive as one batch.
    let four = compare(&[
        outcome("synthetic/x", "adaptive"),
        outcome("synthetic/x", "levelset"),
        outcome("synthetic/x", "stratified"),
        outcome("synthetic/x", "random"),
    ]);
    let row = four.first().expect("one entry, one row");
    for arm in ["adaptive", "levelset", "stratified", "random"] {
        assert!(
            row.get(arm).is_some(),
            "{arm} is missing from the comparison table: {row:?}"
        );
    }
    // An arm that was not measured is absent rather than invented, and the order is the enum's, not
    // the order the outcomes happened to arrive in.
    let three = compare(&[
        outcome("synthetic/x", "random"),
        outcome("synthetic/x", "adaptive"),
    ]);
    let row = three.first().expect("one entry, one row");
    assert!(row.get("adaptive").is_some() && row.get("random").is_some());
    assert!(
        row.get("levelset").is_none() && row.get("stratified").is_none(),
        "an arm that ran nothing cannot have a row: {row:?}"
    );
}
