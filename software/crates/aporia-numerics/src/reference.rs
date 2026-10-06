//! Reference evaluation of a model at double-double precision.
//!
//! This path is deliberately not shared with `aporia-runtime`. Production paths must agree, so the
//! scalar and batched interpreters call one definition of every operation. A reference path has the
//! opposite requirement: if it reused the same arithmetic, agreement would prove nothing about the
//! model and everything about the code. The duplication is the point, and it is why the two are in
//! different crates.
//!
//! Transcendental functions are evaluated in f64. A model built only of `+ - * /` gets a genuinely
//! higher-precision comparator; one full of `sin` and `exp` gets f64 accuracy at those nodes and the
//! disagreement measured at them is an understatement, which the report must not present as a bound.

use crate::dd::Dd;
use aporia_ir::{Binop, BlockId, Builtin, Id, InstrKind, Lit, Model, Operand, Unop};

/// What happened while evaluating a model in reference precision.
#[derive(Clone, Debug)]
pub struct Reference {
    /// Outputs as double-double pairs, so the low word is available to anyone who wants the
    /// residual rather than the rounded answer.
    pub outputs: Vec<Dd>,
    /// Instructions executed.
    pub steps: u64,
    pub budget_exceeded: bool,
    pub non_finite: bool,
}

impl Reference {
    /// The f64 reading of each output.
    #[must_use]
    pub fn values(&self) -> Vec<f64> {
        self.outputs.iter().map(|d| d.to_f64()).collect()
    }
}

/// Evaluate `model` at `x` using double-double arithmetic, with the same step budget semantics as
/// the runtime so a runaway loop behaves the same on both paths.
#[must_use]
pub fn evaluate(model: &Model, x: &[f64], max_steps: u64) -> Reference {
    let mut e = Evaluator {
        model,
        x,
        env: vec![Dd::zero(); model.instrs.len()],
        visited: vec![false; model.instrs.len()],
        slots: vec![Dd::zero(); model.slots.len()],
        steps: 0,
        budget_exceeded: false,
        non_finite: false,
        max_steps,
    };
    e.init_slots();
    e.block(0);
    let outputs = model.outputs.iter().map(|o| e.operand(&o.value)).collect();
    Reference {
        outputs,
        steps: e.steps,
        budget_exceeded: e.budget_exceeded,
        non_finite: e.non_finite,
    }
}

struct Evaluator<'a> {
    model: &'a Model,
    x: &'a [f64],
    env: Vec<Dd>,
    /// Which nodes have actually been evaluated. `env` cannot carry that itself: a double-double has no
    /// spare encoding for "not yet computed", so an unvisited node and a node whose value is zero are
    /// indistinguishable here exactly as they are in the runtime, where `Value::Unit` keeps the
    /// difference.
    visited: Vec<bool>,
    slots: Vec<Dd>,
    steps: u64,
    max_steps: u64,
    budget_exceeded: bool,
    non_finite: bool,
}

impl Evaluator<'_> {
    /// Give every slot the value the model says it starts at, evaluated the way the runtime
    /// evaluates it.
    ///
    /// This used to handle a literal initial value and produce `NaN` for everything else, which
    /// meant a model with `state acc = k * dt` had no Differential channel at all: the reference's
    /// outputs went non-finite, non-finite pairs are filtered before they can be compared, and the
    /// absence was reported as agreement rather than as a missing second opinion. The two paths
    /// share no arithmetic — that is the point of the channel — but they have to share the meaning
    /// of the model, including where its state begins.
    fn init_slots(&mut self) {
        let inits: Vec<(usize, Operand)> = self
            .model
            .slots
            .iter()
            .enumerate()
            .map(|(i, s)| (i, s.init))
            .collect();
        for (slot, init) in inits {
            let v = self.operand(&init);
            self.slots[slot] = v;
        }
    }

    fn block(&mut self, id: BlockId) {
        let Some(block) = self.model.blocks.get(id as usize) else {
            return;
        };
        let instrs = block.instrs.clone();
        for instr in instrs {
            self.instr(instr);
        }
    }

    fn instr(&mut self, id: Id) {
        if self.budget_exceeded {
            return;
        }
        self.steps += 1;
        if self.steps > self.max_steps {
            self.budget_exceeded = true;
            return;
        }
        let Some(instr) = self.model.instrs.get(id as usize) else {
            return;
        };
        let v = match &instr.kind {
            InstrKind::For { trip, body } => {
                let n = self.operand(trip).to_f64() as i64;
                for _ in 0..n.max(0) {
                    if self.budget_exceeded {
                        break;
                    }
                    self.block(*body);
                }
                Dd::zero()
            }
            InstrKind::Write { slot, value } => {
                let v = self.operand(value);
                if let Some(s) = self.slots.get_mut(*slot as usize) {
                    *s = v;
                }
                Dd::zero()
            }
            InstrKind::Opaque => {
                // Refuses like the runtime does. This evaluator is an independent implementation of
                // *A-IR instructions*; there are no instructions behind this value, so there is no
                // second opinion to compare against and the honest answer is not-a-number.
                Dd::from_f64(f64::NAN)
            }
            InstrKind::Bin { op, a, b } => {
                let x = self.operand(a);
                let y = self.operand(b);
                self.bin(*op, x, y, instr.ty.num == aporia_ir::NumType::Bool)
            }
            InstrKind::Un { op, a } => {
                let x = self.operand(a);
                self.un(*op, x)
            }
            InstrKind::Call { builtin, args } => {
                let vals: Vec<Dd> = args.iter().map(|a| self.operand(a)).collect();
                self.call(*builtin, &vals)
            }
        };
        if !v.is_finite() {
            self.non_finite = true;
        }
        self.env[id as usize] = v;
        if (id as usize) < self.visited.len() {
            self.visited[id as usize] = true;
        }
    }

    /// `self` is unused here, and the method shape is kept anyway so every operation in this
    /// evaluator is reached the same way.
    #[expect(clippy::unused_self)]
    fn bin(&mut self, op: Binop, a: Dd, b: Dd, predicate: bool) -> Dd {
        if predicate {
            let (x, y) = (a.to_f64(), b.to_f64());
            let r = match op {
                Binop::Lt => x < y,
                Binop::Le => x <= y,
                Binop::Gt => x > y,
                Binop::Ge => x >= y,
                Binop::Eq => x == y,
                Binop::Ne => x != y,
                Binop::And => x != 0.0 && y != 0.0,
                Binop::Or => x != 0.0 || y != 0.0,
                other => unreachable!("{other:?} is not a predicate"),
            };
            return Dd::from_f64(if r { 1.0 } else { 0.0 });
        }
        match op {
            Binop::Add => a.add(b),
            Binop::Sub => a.sub(b),
            Binop::Mul => a.mul(b),
            Binop::Div => a.div(b),
            // `rem_euclid` and `powf` have no double-double implementation here. The comparison
            // made at these nodes measures whatever accuracy the two paths share, not more.
            Binop::Rem => Dd::from_f64(a.to_f64().rem_euclid(b.to_f64())),
            Binop::Pow => {
                let (x, y) = (a.to_f64(), b.to_f64());
                if y.fract() == 0.0 && y.abs() < 32.0 {
                    // An integral power can be built by repeated double-double multiplication,
                    // which keeps the comparison honest for the common `x^2` and `x^3` cases.
                    let mut acc = Dd::from_f64(1.0);
                    let base = if y < 0.0 { Dd::from_f64(1.0).div(a) } else { a };
                    for _ in 0..y.abs() as u64 {
                        acc = acc.mul(base);
                    }
                    acc
                } else {
                    Dd::from_f64(x.powf(y))
                }
            }
            Binop::Min | Binop::Max => {
                let (x, y) = (a.to_f64(), b.to_f64());
                let keep = if op == Binop::Min { x <= y } else { x >= y };
                if keep { a } else { b }
            }
            other => unreachable!("{other:?} handled as a predicate"),
        }
    }

    #[expect(clippy::unused_self)]
    fn un(&mut self, op: Unop, a: Dd) -> Dd {
        match op {
            Unop::Neg => a.neg(),
            Unop::Abs => a.abs(),
            Unop::Sqrt => {
                // Newton refinement of the f64 root in double-double: enough for a comparison
                // path, and no transcendental machinery.
                let x = a.to_f64();
                if x <= 0.0 {
                    return Dd::from_f64(x.sqrt());
                }
                let q0 = Dd::from_f64(x.sqrt());
                q0.add(a.div(q0)).mul(Dd::from_f64(0.5))
            }
            Unop::Cbrt => Dd::from_f64(a.to_f64().cbrt()),
            Unop::Exp
            | Unop::Ln
            | Unop::Log2
            | Unop::Log10
            | Unop::Sin
            | Unop::Cos
            | Unop::Tan
            | Unop::Asin
            | Unop::Acos
            | Unop::Atan
            | Unop::Sinh
            | Unop::Cosh
            | Unop::Tanh => {
                let v = match op {
                    Unop::Exp => a.to_f64().exp(),
                    Unop::Ln => a.to_f64().ln(),
                    Unop::Log2 => a.to_f64().log2(),
                    Unop::Log10 => a.to_f64().log10(),
                    Unop::Sin => a.to_f64().sin(),
                    Unop::Cos => a.to_f64().cos(),
                    Unop::Tan => a.to_f64().tan(),
                    Unop::Asin => a.to_f64().asin(),
                    Unop::Acos => a.to_f64().acos(),
                    Unop::Atan => a.to_f64().atan(),
                    Unop::Sinh => a.to_f64().sinh(),
                    Unop::Cosh => a.to_f64().cosh(),
                    _ => a.to_f64().tanh(),
                };
                Dd::from_f64(v)
            }
            Unop::Floor => Dd::from_f64(a.to_f64().floor()),
            Unop::Ceil => Dd::from_f64(a.to_f64().ceil()),
            Unop::Round => Dd::from_f64(a.to_f64().round()),
            Unop::Trunc => Dd::from_f64(a.to_f64().trunc()),
            Unop::Not => Dd::from_f64(if a.to_f64() == 0.0 { 1.0 } else { 0.0 }),
            Unop::IsFinite => Dd::from_f64(if a.is_finite() { 1.0 } else { 0.0 }),
            Unop::Cast(_) => a,
        }
    }

    #[expect(clippy::unused_self)]
    fn call(&mut self, builtin: Builtin, args: &[Dd]) -> Dd {
        let f: Vec<f64> = args.iter().map(|d| d.to_f64()).collect();
        match builtin {
            Builtin::Atan2 => Dd::from_f64(f[0].atan2(f[1])),
            Builtin::Hypot => Dd::from_f64(f[0].hypot(f[1])),
            // `fma` in reference precision is a product and a sum, each with its own error term,
            // which is strictly better than the single rounding the production path gets.
            Builtin::Fma => args[0].mul(args[1]).add(args[2]),
            Builtin::Clamp => {
                let v = f[0].clamp(f[1], f[2]);
                Dd::from_f64(v)
            }
            Builtin::Lerp => args[0].add(args[1].sub(args[0]).mul(args[2])),
        }
    }

    fn operand(&mut self, o: &Operand) -> Dd {
        match o {
            Operand::Lit(l) => Dd::from_f64(lit(*l)),
            Operand::Param(i) => Dd::from_f64(self.x.get(*i as usize).copied().unwrap_or(f64::NAN)),
            Operand::Slot(i) => self.slots.get(*i as usize).copied().unwrap_or(Dd::zero()),
            Operand::Node(id) => {
                let cached = self.env.get(*id as usize).copied();
                match cached {
                    Some(v) if !self.needs_eval(*id, v) => v,
                    _ => {
                        self.instr(*id);
                        self.env.get(*id as usize).copied().unwrap_or(Dd::zero())
                    }
                }
            }
        }
    }

    /// Has this node been evaluated at all, and is the cached value therefore trustworthy?
    ///
    /// A node that has never run is always evaluated, because an unvisited node holds a zero here and
    /// reading that as an answer is what made a state initialiser over an expression start at zero.
    /// The second clause keeps the older rule for loop and write nodes, whose legitimate value can be
    /// zero on a path that has run: re-evaluating one is harmless, and both paths count the step.
    fn needs_eval(&self, id: Id, cached: Dd) -> bool {
        let Some(instr) = self.model.instrs.get(id as usize) else {
            return false;
        };
        let unvisited = self.visited.get(id as usize).is_none_or(|v| !*v);
        unvisited
            || (matches!(instr.kind, InstrKind::For { .. } | InstrKind::Write { .. })
                && cached == Dd::zero())
    }
}

fn lit(l: Lit) -> f64 {
    match l {
        Lit::F64(v) => v,
        Lit::F32(v) => f64::from(v),
        Lit::I64(v) => v as f64,
        Lit::Bool(b) => {
            if b {
                1.0
            } else {
                0.0
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aporia_dsl::lower::compile;

    fn model(text: &str) -> Model {
        let c = compile("t.ap", text);
        assert!(!c.diagnostics.has_errors(), "{}\n{text}", c.diagnostics);
        c.model
    }

    #[test]
    fn a_polynomial_matches_the_hand_computed_answer_exactly() {
        let m = model("model p \"\" {\n input x in [0, 10]\n let y = x * x - 3 * x + 2\n}\n");
        let r = evaluate(&m, &[4.0], 10_000);
        assert_eq!(r.values()[0], 6.0);
    }

    #[test]
    fn a_catastrophic_cancellation_survives_in_the_reference_path() {
        // (1 + e) - 1 for a tiny e: plain f64 loses it, double-double keeps it.
        let m = model("model c \"\" {\n input e in [0, 1]\n let y = (1 + e) - 1\n}\n");
        let e = 1e-16f64;
        let naive = (1.0f64 + e) - 1.0;
        let r = evaluate(&m, &[e], 10_000);
        assert_eq!(naive, 0.0, "the plain path has nothing left");
        assert!(r.values()[0] > 0.0, "reference keeps {}", r.values()[0]);
    }

    #[test]
    fn a_state_that_starts_from_an_expression_starts_the_same_way_here() {
        // The defect this pins: `state acc = <literal>` was understood and anything else became
        // `NaN`, so a perfectly ordinary model lost its Differential channel in silence — the
        // reference's outputs went non-finite, the pair was filtered, and the missing second
        // opinion looked like agreement.
        let m = model(
            "model i \"\" {\n input k in [0, 5]\n input dt : s in [0.001, 0.2]\n \
             state acc = k * dt\n let y = acc\n}\n",
        );
        let r = evaluate(&m, &[3.0, 0.25], 10_000);
        assert!(
            r.values()[0].is_finite(),
            "the reference could not evaluate its own starting value: {:?}",
            r.values()
        );
        assert_eq!(r.values()[0], 0.75, "k * dt at the given point");
        assert!(!r.non_finite);
        // A loop that advances from that start still accumulates, which is the shape of every
        // integration model in the corpus.
        let stepped = model(
            "model j \"\" {\n input k in [0, 5]\n input n : count in [0, 10]\n \
             state acc = k\n loop n {\n advance acc = acc + k\n }\n let y = acc\n}\n",
        );
        let r = evaluate(&stepped, &[2.0, 4.0], 10_000);
        assert_eq!(r.values()[0], 10.0, "2 + 4*2");
    }

    #[test]
    fn a_loop_in_the_reference_path_agrees_with_the_arithmetic() {
        let m = model(
            "model s \"\" {\n input n : count in [0, 100]\n state acc = 0\n loop n {\n advance acc = acc + 1\n }\n let y = acc\n}\n",
        );
        let r = evaluate(&m, &[10.0], 1_000_000);
        assert_eq!(r.values()[0], 10.0);
        assert!(!r.budget_exceeded);
    }

    #[test]
    fn the_step_budget_stops_both_paths_the_same_way() {
        let m = model(
            "model b \"\" {\n input n : count in [1, 100000]\n state x = 0\n loop n {\n advance x = x + 1\n }\n let y = x\n}\n",
        );
        let r = evaluate(&m, &[100_000.0], 500);
        assert!(
            r.budget_exceeded,
            "the reference must respect the same guard"
        );
    }

    #[test]
    fn integral_powers_are_computed_in_double_double() {
        let m = model("model q \"\" {\n input x in [0, 4]\n let y = x^3\n}\n");
        let r = evaluate(&m, &[1.1], 10_000);
        assert!((r.values()[0] - 1.331).abs() < 1e-15, "{}", r.values()[0]);
    }

    #[test]
    fn a_division_by_zero_shows_up_as_not_finite_rather_than_as_a_number() {
        let m = model("model z \"\" {\n input x in [0, 1]\n let y = 1 / x\n}\n");
        let r = evaluate(&m, &[0.0], 10_000);
        assert!(r.non_finite);
        assert!(r.values()[0].is_infinite());
    }

    #[test]
    fn sqrt_of_a_square_returns_the_magnitude() {
        let m = model("model t \"\" {\n input x in [0, 9]\n let y = sqrt(x)\n}\n");
        assert_eq!(evaluate(&m, &[9.0], 10_000).values()[0], 3.0);
    }

    #[test]
    fn the_reference_refuses_a_value_it_has_no_instructions_for() {
        // The Differential channel's whole claim is that this evaluator repeats the *same* arithmetic
        // independently. For a value the model does not describe there is no arithmetic to repeat, and
        // answering with a double-double zero would manufacture a disagreement out of nothing and file
        // it as evidence about a program APORIA cannot see.
        let mut m = aporia_ir::Model::new("external");
        m.params.push(aporia_ir::Param {
            name: "load".into(),
            ty: aporia_ir::Ty::dimensionless_f64(),
            domain: aporia_ir::Domain::interval(0.0, 10.0),
            to_si: 1.0,
            doc: String::new(),
        });
        let value = m.push_entry(aporia_ir::Instr {
            ty: aporia_ir::Ty::dimensionless_f64(),
            kind: aporia_ir::InstrKind::Opaque,
        });
        m.outputs.push(aporia_ir::Output {
            name: "deflection".into(),
            ty: aporia_ir::Ty::dimensionless_f64(),
            value: aporia_ir::Operand::Node(value),
            doc: String::new(),
        });
        let out = evaluate(&m, &[3.0], 1000);
        let values = out.values();
        assert_eq!(values.len(), 1);
        assert!(values[0].is_nan(), "the reference answered {}", values[0]);
    }
}
