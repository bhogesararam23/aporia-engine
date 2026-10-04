//! The execution boundary: the one thing a campaign asks to have done to it.
//!
//! Until now the search driver called [`crate::interp::run`] directly, which meant "running a model"
//! and "running the scalar interpreter" were the same sentence in the code. They are not the same
//! thing: a scientific program that APORIA did not parse also produces outputs at a point, and the
//! whole value of the instrument is in analysing those. So the driver asks an [`Executor`] instead,
//! and the interpreter becomes the default one — a backend among backends rather than an assumption.
//!
//! Two capabilities travel with an executor, and they exist to stop the campaign buying evidence that
//! cannot exist. A numerical probe asks whether the *same* arithmetic done at a lower precision
//! disagrees; a differential probe asks whether an *independent implementation* disagrees. Neither
//! question is answerable about a program APORIA has only one path to. An executor that cannot answer
//! one says so, and the campaign spends no evaluations on it — which is the honest alternative to
//! running the same path twice, observing perfect agreement, and recording that as evidence.

use crate::interp::Outcome;
use crate::value::ExecConfig;
use aporia_ir::Model;

/// Something that can evaluate a model at a point.
///
/// Taking `&mut` is deliberate: an executor over a process or a device may keep state — a running
/// program, a warm context, a cache of points already asked for — and a campaign that handed out
/// `&self` would push that state into interior mutability and hide the cost of an evaluation.
pub trait Executor {
    /// Evaluate `model` at `x`, which is in the units the parameters were declared in.
    ///
    /// An outcome is a claim about what happened, so it has to say so completely: values, the flags
    /// that were raised, and the work it cost. `steps` is the currency the cost metrics read; an
    /// executor that cannot count its own work reports 0 rather than inventing a number.
    fn execute(&mut self, model: &Model, x: &[f64], cfg: ExecConfig) -> Outcome;

    /// Whether a second run at `FpMode::F32` is an independent numerical path.
    ///
    /// True for the scalar interpreter, which changes the rounding of every operation. False for a
    /// program that ignores the requested precision: running it twice and comparing the results
    /// measures the repeatability of one implementation, which is not the question the Numerical
    /// channel asks.
    fn varies_with_precision(&self) -> bool {
        true
    }

    /// Whether the model's equations are in the A-IR, so that an independent evaluator of the same
    /// A-IR (`aporia_numerics::reference`) is a second opinion rather than a restatement.
    ///
    /// True for anything that executes A-IR instructions. False for an external program whose
    /// equations are not in the model at all: there is nothing for the reference path to re-do, and
    /// comparing the program against an interpreter of a model that does not describe it would
    /// produce evidence about APORIA's own guess at its arithmetic.
    fn has_reference_path(&self) -> bool {
        true
    }
}

/// The scalar interpreter, as an [`Executor`]. This is what `aporia-search::run` uses unless it is
/// told otherwise.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Interp;

impl Executor for Interp {
    fn execute(&mut self, model: &Model, x: &[f64], cfg: ExecConfig) -> Outcome {
        crate::interp::run(model, x, cfg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::FpMode;
    use aporia_dsl::lower::compile;

    fn model(text: &str) -> Model {
        let c = compile("t.ap", text);
        assert!(!c.diagnostics.has_errors(), "{}\n{text}", c.diagnostics);
        c.model
    }

    #[test]
    fn the_interpreter_as_an_executor_is_the_interpreter() {
        // The seam must not change the answer. If `Interp` did anything except hand the call to
        // `interp::run`, every published measurement would now be describing a different execution
        // path than the one that produced it.
        let m = model("model c \"\" {\n input x in [0, 10]\n let y = x * x + 1.0\n}\n");
        let cfg = ExecConfig::default();
        for x in [0.0, 0.5, 3.25, 10.0] {
            let direct = crate::interp::run(&m, &[x], cfg);
            let through = Interp.execute(&m, &[x], cfg);
            assert_eq!(direct.outputs, through.outputs, "x = {x}");
            assert_eq!(direct.steps, through.steps);
            assert_eq!(direct.flags, through.flags);
        }
        assert!(Interp::varies_with_precision(&Interp));
        assert!(Interp::has_reference_path(&Interp));
    }

    /// A stand-in for a program APORIA did not parse: it declares its own outputs and answers from
    /// arithmetic no A-IR instruction produced.
    struct Doubler {
        calls: usize,
    }

    impl Executor for Doubler {
        fn execute(&mut self, model: &Model, x: &[f64], _cfg: ExecConfig) -> Outcome {
            self.calls += 1;
            Outcome {
                outputs: x
                    .iter()
                    .map(|v| v * 2.0)
                    .take(model.outputs.len())
                    .collect(),
                traces: Vec::new(),
                flags: crate::value::Flags::default(),
                steps: 0,
                rule_values: Vec::new(),
            }
        }

        fn varies_with_precision(&self) -> bool {
            false
        }

        fn has_reference_path(&self) -> bool {
            false
        }
    }

    #[test]
    fn a_foreign_executor_answers_from_its_own_program() {
        // The trait is the whole contract: no A-IR instruction is evaluated, and the values still
        // arrive shaped like an execution. `steps` is 0 because this program does not count its work
        // in APORIA's units -- reporting a number would be inventing one.
        let m = model("model d \"\" {\n input x in [0, 10]\n let y = x * x + 1.0\n}\n");
        let mut exec = Doubler { calls: 0 };
        let out = exec.execute(
            &m,
            &[3.0],
            ExecConfig {
                fp: FpMode::F64,
                max_steps: 10,
            },
        );
        assert_eq!(out.outputs, vec![6.0]);
        assert_eq!(exec.calls, 1);
        assert_eq!(out.steps, 0);
        assert!(!exec.varies_with_precision());
        assert!(!exec.has_reference_path());
    }

    #[test]
    fn the_same_foreign_point_answers_the_same_way_twice() {
        // Determinism is a property the adapter tests will rely on, so it belongs at the boundary
        // rather than only in the subprocess implementation.
        let m = model("model d \"\" {\n input x in [0, 10]\n let y = x * x + 1.0\n}\n");
        let mut a = Doubler { calls: 0 };
        let mut b = Doubler { calls: 0 };
        let cfg = ExecConfig::default();
        assert_eq!(
            a.execute(&m, &[4.5], cfg).outputs,
            b.execute(&m, &[4.5], cfg).outputs
        );
    }
}
