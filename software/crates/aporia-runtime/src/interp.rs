//! The scalar interpreter: one candidate, one execution, everything in order.
//!
//! This is the reference implementation of A-IR semantics. The batched interpreter has to agree
//! with it, the assembly kernels are validated against it, and a CUDA port would be checked the
//! same way. Deliberately straightforward: an array indexed by instruction id, a `for` loop over
//! each block, no tricks.

use crate::ops;
use crate::value::{ExecConfig, Flags, Value};
use aporia_ir::{BlockId, Id, Instr, InstrKind, Lit, Model, NumType, Operand};

/// What one execution produced.
#[derive(Clone, Debug, PartialEq)]
pub struct Outcome {
    /// Model outputs, in declaration order.
    pub outputs: Vec<f64>,
    /// One vector per trace, holding the value recorded at each iteration of its loop.
    pub traces: Vec<Vec<f64>>,
    pub flags: Flags,
    /// Instructions executed, which is the unit the cost metrics count in.
    pub steps: u64,
}

impl Outcome {
    #[must_use]
    pub fn output(&self, index: usize) -> f64 {
        self.outputs[index]
    }
}

/// Evaluate a model at one point in its parameter space.
///
/// `x` is in the units the parameters were declared in; conversion to SI is the caller's business
/// and is recorded on the parameter, not applied here.
pub fn run(model: &Model, x: &[f64], cfg: ExecConfig) -> Outcome {
    Machine {
        model,
        cfg,
        x,
        env: vec![Value::Unit; model.instrs.len()],
        slots: vec![Value::Unit; model.slots.len()],
        flags: Flags::default(),
        steps: 0,
        traces: model.traces.iter().map(|_| Vec::new()).collect(),
        aborted: false,
    }
    .execute()
}

struct Machine<'a> {
    model: &'a Model,
    cfg: ExecConfig,
    x: &'a [f64],
    env: Vec<Value>,
    slots: Vec<Value>,
    flags: Flags,
    steps: u64,
    traces: Vec<Vec<f64>>,
    aborted: bool,
}

impl Machine<'_> {
    fn execute(mut self) -> Outcome {
        self.init_slots();
        self.block(0);
        let outputs = self
            .model
            .outputs
            .iter()
            .map(|o| self.operand(&o.value).as_f64())
            .collect();
        Outcome {
            outputs,
            traces: self.traces,
            flags: self.flags,
            steps: self.steps,
        }
    }

    /// A slot may be initialised from an expression over parameters, so the instructions it needs
    /// are evaluated on demand. Entry-block instructions are pure, which is what makes evaluating
    /// them out of turn safe.
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
        for instr_id in instrs {
            self.instr(instr_id);
        }
        self.capture_traces(id);
    }

    fn capture_traces(&mut self, id: BlockId) {
        let scopes: Vec<(usize, Operand)> = self
            .model
            .traces
            .iter()
            .enumerate()
            .filter(|(_, t)| t.scope == id)
            .map(|(i, t)| (i, t.value))
            .collect();
        for (i, value) in scopes {
            let v = self.operand(&value).as_f64();
            self.traces[i].push(v);
        }
    }

    fn instr(&mut self, id: Id) -> Value {
        if self.aborted && id < self.env.len() as Id {
            return self.env[id as usize];
        }
        self.steps += 1;
        if self.steps > self.cfg.max_steps {
            self.aborted = true;
            self.flags.step_budget = true;
            return Value::F64(f64::NAN);
        }
        let Some(instr) = self.model.instrs.get(id as usize) else {
            return Value::F64(f64::NAN);
        };
        let v = match &instr.kind {
            InstrKind::For { trip, body } => {
                let n = self.trip_count(trip);
                for _ in 0..n {
                    if self.aborted {
                        break;
                    }
                    self.block(*body);
                }
                Value::Unit
            }
            InstrKind::Write { slot, value } => {
                let v = self.operand(value);
                if let Some(s) = self.slots.get_mut(*slot as usize) {
                    *s = v;
                }
                Value::Unit
            }
            InstrKind::Bin { op, a, b } => {
                let (x, y) = (self.operand(a), self.operand(b));
                self.apply_bin(*op, x, y, instr)
            }
            InstrKind::Un { op, a } => {
                let x = self.operand(a);
                let (v, f) = match (op, x) {
                    (aporia_ir::Unop::Not, Value::Bool(b)) => (Value::Bool(!b), Flags::default()),
                    (aporia_ir::Unop::IsFinite, Value::Bool(_)) => {
                        (Value::Bool(true), Flags::default())
                    }
                    (aporia_ir::Unop::Cast(t), v) => (widen_to(v, *t), Flags::default()),
                    (_, other) => {
                        let (r, f) = ops::un_f64(*op, other.as_f64(), self.cfg.fp);
                        (r.to_value(instr.ty.num), f)
                    }
                };
                self.flags = self.flags.merge(f);
                v
            }
            InstrKind::Call { builtin, args } => {
                let vals: Vec<f64> = args.iter().map(|a| self.operand(a).as_f64()).collect();
                let (r, f) = ops::call_f64(*builtin, &vals, self.cfg.fp);
                self.flags = self.flags.merge(f);
                r.to_value(instr.ty.num)
            }
        };
        if (id as usize) < self.env.len() {
            self.env[id as usize] = v;
        }
        v
    }

    fn apply_bin(&mut self, op: aporia_ir::Binop, a: Value, b: Value, instr: &Instr) -> Value {
        if instr.ty.num == NumType::Bool {
            let (r, f) = ops::compare_f64(op, a.as_f64(), b.as_f64());
            self.flags = self.flags.merge(f);
            return Value::Bool(r != 0.0);
        }
        if matches!(a, Value::I64(_)) && matches!(b, Value::I64(_)) && instr.ty.num == NumType::I64
        {
            let (r, f) = ops::bin_i64(
                op,
                a.as_i64().unwrap_or_default(),
                b.as_i64().unwrap_or_default(),
            );
            self.flags = self.flags.merge(f);
            return Value::I64(r);
        }
        let (r, f) = ops::bin_f64(op, a.as_f64(), b.as_f64(), self.cfg.fp);
        self.flags = self.flags.merge(f);
        r.to_value(instr.ty.num)
    }

    fn trip_count(&mut self, trip: &Operand) -> u64 {
        let v = self.operand(trip);
        let n = match v {
            Value::I64(i) => i,
            other => other.as_f64() as i64,
        };
        if n < 0 {
            self.flags.invalid_domain = true;
            return 0;
        }
        n as u64
    }

    fn operand(&mut self, o: &Operand) -> Value {
        match o {
            Operand::Lit(l) => lit_value(*l),
            Operand::Param(i) => {
                self.model
                    .params
                    .get(*i as usize)
                    .map_or(Value::F64(f64::NAN), |p| {
                        let raw = self.x.get(*i as usize).copied().unwrap_or(f64::NAN);
                        if p.ty.num == NumType::I64 {
                            Value::I64(raw as i64)
                        } else {
                            Value::F64(raw)
                        }
                    })
            }
            Operand::Slot(i) => self
                .slots
                .get(*i as usize)
                .copied()
                .unwrap_or(Value::F64(f64::NAN)),
            Operand::Node(id) => {
                let cached = self.env.get(*id as usize).copied();
                if let Some(v) = cached {
                    if !matches!(v, Value::Unit) && !self.is_loop_node(*id) {
                        return v;
                    }
                }
                self.instr(*id)
            }
        }
    }

    /// A `For` or `Write` node has no value of its own, so a cached `Unit` must not be treated as
    /// an answer; it has to be re-run or reported as absent.
    fn is_loop_node(&self, id: Id) -> bool {
        matches!(
            self.model.instrs.get(id as usize).map(|i| &i.kind),
            Some(InstrKind::For { .. } | InstrKind::Write { .. })
        )
    }
}

fn lit_value(l: Lit) -> Value {
    Value::from_lit(l)
}

fn widen_to(v: Value, to: NumType) -> Value {
    match to {
        NumType::F64 => Value::F64(v.as_f64()),
        NumType::F32 => Value::F32(v.as_f64() as f32),
        NumType::I64 => Value::I64(v.as_f64().trunc() as i64),
        NumType::Bool => Value::Bool(v.as_f64() != 0.0),
        NumType::Unit => Value::Unit,
    }
}

/// The numeric pipeline carries everything as f64; this is where a value takes the representation
/// its instruction declared.
trait AsValue {
    fn to_value(self, ty: NumType) -> Value;
}

impl AsValue for f64 {
    fn to_value(self, ty: NumType) -> Value {
        match ty {
            NumType::F64 => Value::F64(self),
            NumType::F32 => Value::F32(self as f32),
            NumType::I64 => Value::I64(self.trunc() as i64),
            NumType::Bool => Value::Bool(self != 0.0),
            NumType::Unit => Value::Unit,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::FpMode;
    use aporia_dsl::lower::compile;

    /// Models are built through the real front end: the runtime must execute what the language
    /// produces, not a hand-written approximation of it.
    fn model(text: &str) -> Model {
        let c = compile("t.ap", text);
        assert!(
            !c.diagnostics.has_errors(),
            "{}
{text}",
            c.diagnostics
        );
        c.model
    }

    #[test]
    fn a_constant_model_returns_its_value() {
        let m = model("model c \"\" {\n input x in [0, 1]\n let y = 2.5\n}\n");
        let out = run(&m, &[0.0], ExecConfig::default());
        assert_eq!(out.outputs, vec![2.5]);
        assert!(out.flags.is_clean(), "{:?}", out.flags);
    }

    #[test]
    fn division_by_zero_survives_as_a_flag_not_a_panic() {
        let m = model("model d \"\" {\n input x in [0, 1]\n let y = 1 / x\n}\n");
        let out = run(&m, &[0.0], ExecConfig::default());
        assert!(out.outputs[0].is_infinite());
        assert!(out.flags.zero_division && out.flags.inf);
        let ok = run(&m, &[4.0], ExecConfig::default());
        assert_eq!(ok.outputs[0], 0.25);
        assert!(ok.flags.is_clean());
    }

    #[test]
    fn sqrt_of_a_negative_is_reported_as_an_invalid_domain() {
        let m = model("model s \"\" {\n input x in [-4, 4]\n let y = sqrt(x)\n}\n");
        let out = run(&m, &[-4.0], ExecConfig::default());
        assert!(out.outputs[0].is_nan());
        assert!(out.flags.invalid_domain);
    }

    #[test]
    fn loops_advance_state_in_order() {
        let m = model(
            "model e \"\" {\n input n : count in [0, 10]\n state acc = 0\n loop n {\n advance acc = acc + 1\n }\n let y = acc\n}\n",
        );
        let out = run(&m, &[3.0], ExecConfig::default());
        assert_eq!(out.outputs[0], 3.0);
    }

    #[test]
    fn a_traced_quantity_records_one_value_per_iteration() {
        let m = model(
            "model t \"\" {\n input n : count in [1, 10]\n state x = 1.0\n loop n {\n advance x = x * 2\n watch x\n }\n let y = x\n}\n",
        );
        let out = run(&m, &[4.0], ExecConfig::default());
        assert_eq!(out.outputs[0], 16.0);
        assert_eq!(out.traces[0], vec![2.0, 4.0, 8.0, 16.0]);
    }

    #[test]
    fn the_step_budget_stops_a_runaway_loop() {
        let m = model(
            "model r \"\" {\n input n : count in [1, 10000000]\n state x = 0\n loop n {\n advance x = x + 1\n }\n let y = x\n}\n",
        );
        let cfg = ExecConfig {
            fp: FpMode::F64,
            max_steps: 500,
        };
        let out = run(&m, &[9_000_000.0], cfg);
        assert!(out.flags.step_budget, "the guard must fire");
        assert!(out.steps <= 501, "steps {}", out.steps);
    }

    #[test]
    fn reduced_precision_changes_a_delicate_sum() {
        let m = model("model p \"\" {\n input eps in [0, 1]\n let y = 1 + eps\n}\n");
        let full = run(&m, &[1e-9], ExecConfig::default());
        let reduced = run(&m, &[1e-9], ExecConfig::default().with_fp(FpMode::F32));
        assert!(full.outputs[0] > 1.0);
        assert_eq!(reduced.outputs[0], 1.0, "f32 cannot see a billionth");
    }

    #[test]
    #[allow(dead_code)]
    fn comparisons_carry_booleans_through_arithmetic() {
        let m = model("model b \"\" {\n input x in [-1, 1]\n let y = min(x, 0)\n}\n");
        assert_eq!(run(&m, &[-0.5], ExecConfig::default()).outputs[0], -0.5);
        assert_eq!(run(&m, &[0.5], ExecConfig::default()).outputs[0], 0.0);
    }

    #[test]
    fn steps_count_instructions_so_cost_is_measurable() {
        let m = model(
            "model k \"\" {\n input n : count in [1, 100]\n state x = 0\n loop n {\n advance x = x + 1\n }\n let y = x\n}\n",
        );
        let out = run(&m, &[10.0], ExecConfig::default());
        // Ten iterations of one write, plus the entry pass.
        assert!(out.steps >= 11 && out.steps < 30, "{}", out.steps);
    }
}
