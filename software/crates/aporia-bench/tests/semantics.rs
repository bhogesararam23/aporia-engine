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
