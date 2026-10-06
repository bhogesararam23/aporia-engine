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

/// True when the model declares a value no A-IR instruction computes.
///
/// A caller that is about to spend an evaluation budget on such a model should ask first and refuse
/// loudly. The interpreters do return NaN when asked anyway — they cannot invent a number — but a
/// campaign that discovers that halfway through reports a domain full of divergence instead of the
/// one sentence "this model needs its program", which is a worse outcome for the reader and a wasted
/// run for everyone.
///
/// **Where that refusal lives: the input boundaries, and only them.** `aporia run` asks before it
/// spends a budget, and `aporia-bench` refuses such a corpus entry during its ground-truth gate.
/// `aporia_search::run` does not check, deliberately: it is a driver rather than a front door, it
/// takes an `Executor` it cannot inspect for this property (the capability flags describe the path, not
/// the model), and a driver that returned a half-budget campaign to signal a refusal would be inventing
/// a result — which is the thing this function exists to stop.
#[must_use]
pub fn needs_adapter(model: &Model) -> bool {
    model
        .instrs
        .iter()
        .any(|i| matches!(i.kind, aporia_ir::InstrKind::Opaque))
}

/// The scalar interpreter, as an [`Executor`]. This is what `aporia-search::run` uses unless it is
/// told otherwise.
///
/// Its capabilities are unconditional because they are properties of the *path*: the interpreter does
/// change rounding with `FpMode`, and `aporia_numerics::reference` is an independent evaluator of the
/// same A-IR. A model that the interpreter cannot execute at all — one containing `Opaque` — is caught
/// by [`needs_adapter`] before a budget is spent on it, not by a capability flag that cannot see the
/// model.
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

    /// A model as an adapter would declare it: one parameter, one output that no instruction
    /// computes, and a rule over that output. Hand-built because the DSL cannot express it until the
    /// next layer exists -- and tests are the one place the IR doc allows a `Model` to be assembled
    /// directly.
    fn external_model() -> Model {
        use aporia_ir::{
            CmpOp, Constraint, ConstraintKind, Domain, Instr, InstrKind, Lit, Operand, Origin,
            Output, Param, Ty,
        };
        let mut m = Model::new("external");
        m.params.push(Param {
            name: "load".into(),
            ty: Ty::dimensionless_f64(),
            domain: Domain::interval(0.0, 10.0),
            to_si: 1.0,
            doc: String::new(),
        });
        let value = m.push_entry(Instr {
            ty: Ty::dimensionless_f64(),
            kind: InstrKind::Opaque,
        });
        m.outputs.push(Output {
            name: "deflection".into(),
            ty: Ty::dimensionless_f64(),
            value: Operand::Node(value),
            doc: String::new(),
        });
        m.constraints.push(Constraint {
            id: 0,
            name: "sag".into(),
            kind: ConstraintKind::Cmp {
                lhs: Operand::Node(value),
                cmp: CmpOp::Ge,
                rhs: Operand::Lit(Lit::F64(0.5)),
                tolerance: 0.0,
            },
            origin: Origin::Declared,
        });
        m
    }

    #[test]
    fn a_model_that_needs_an_adapter_says_so_before_any_budget_is_spent() {
        assert!(needs_adapter(&external_model()));
        assert!(
            !needs_adapter(&model(
                "model d \"\" {\n input x in [0, 10]\n let y = x * x + 1.0\n}\n"
            )),
            "an ordinary A-IR model must not be treated as external"
        );
    }

    #[test]
    fn the_interpreter_refuses_a_value_it_does_not_compute() {
        let m = external_model();
        let mut engine = Interp;
        let out = engine.execute(&m, &[3.0], ExecConfig::default());
        assert_eq!(out.outputs.len(), 1);
        assert!(
            out.outputs[0].is_nan(),
            "the interpreter answered {} for a value it cannot compute",
            out.outputs[0]
        );
        // The crux of the design: the failure mode `Opaque` exists to prevent is a plausible number.
        assert_ne!(out.outputs[0], 0.0);
        assert!(out.flags.nan, "{:?}", out.flags);
    }

    #[test]
    fn the_batched_path_refuses_the_same_way_the_scalar_one_does() {
        // Both paths failing identically is not decoration. The Differential channel between scalar
        // and batch is only meaningful if a difference there means arithmetic; if one path returned
        // zero where the other returned NaN, the channel would be reporting a difference in failure
        // behaviour as a scientific disagreement.
        let m = external_model();
        let batch = crate::batch::run_batch(&m, &[1.0, 3.0, 7.0], 3, ExecConfig::default());
        for (lane, flags) in batch.flags.iter().enumerate() {
            assert!(
                batch.outputs[0][lane].is_nan(),
                "lane {lane} produced {}",
                batch.outputs[0][lane]
            );
            assert!(flags.nan, "lane {lane} raised no flag: {flags:?}");
        }
    }
}
