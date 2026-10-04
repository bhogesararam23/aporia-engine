//! The batched interpreter: many candidates through the same model, lane-major.
//!
//! Layout matters more here than speed. Every value is an `f64` lane array indexed
//! `[instruction][lane]`, the inner loop is always over lanes, and that is what makes the shape
//! portable: the same structure becomes an AVX2 kernel on the CPU and a thread-indexed kernel on a
//! GPU, where `lane` is the thread id. Nothing in this file is intrinsically x86.
//!
//! Integers are carried as f64. That is exact for anything a loop counter or a step index reaches
//! (below 2^53), and it keeps the lane array homogeneous. Where it matters, `interp.rs` is the
//! reference and the two are compared by test.

use crate::ops;
use crate::value::{ExecConfig, Flags};
use aporia_ir::{Binop, BlockId, Id, InstrKind, Lit, Model, Operand, Unop};

/// One batch execution: `count` candidates evaluated together.
#[derive(Clone, Debug)]
pub struct BatchOutcome {
    /// `[output][candidate]`.
    pub outputs: Vec<Vec<f64>>,
    /// `[trace][iteration][candidate]`, ragged because each candidate can loop a different number
    /// of times, and that difference is itself evidence.
    pub traces: Vec<Vec<Vec<f64>>>,
    /// One flag set per candidate, so a single NaN in a batch of a million is attributable.
    pub flags: Vec<Flags>,
    pub steps: u64,
}

impl BatchOutcome {
    #[must_use]
    pub fn candidate(&self, i: usize) -> Vec<f64> {
        self.outputs.iter().map(|o| o[i]).collect()
    }
}

/// Evaluate `count` candidates. `xs` is `[candidate][parameter]`, row major.
///
/// Panics if a row of `xs` does not have one value per parameter: a short row is a caller bug, not
/// data.
#[must_use]
pub fn run_batch(model: &Model, xs: &[f64], count: usize, cfg: ExecConfig) -> BatchOutcome {
    let arity = model.params.len();
    assert_eq!(
        xs.len(),
        count * arity,
        "expected {} values for {} candidates of arity {arity}",
        count * arity,
        count
    );
    let n = model.instrs.len();
    let mut b = Batch {
        model,
        cfg,
        xs,
        arity,
        count,
        env: vec![vec![0.0; count]; n],
        slots: model
            .slots
            .iter()
            .map(|s| vec![init_lane(s.init, count); count])
            .collect(),
        active: vec![true; count],
        flags: vec![Flags::default(); count],
        steps: 0,
        traces: model.traces.iter().map(|_| Vec::new()).collect(),
        computed: vec![false; model.instrs.len()],
        aborted: false,
    };
    b.execute();
    let outputs = b
        .model
        .outputs
        .iter()
        .map(|o| (0..count).map(|lane| b.read(&o.value, lane)).collect())
        .collect();
    BatchOutcome {
        outputs,
        traces: b.traces,
        flags: b.flags,
        steps: b.steps,
    }
}

struct Batch<'a> {
    model: &'a Model,
    cfg: ExecConfig,
    xs: &'a [f64],
    arity: usize,
    count: usize,
    env: Vec<Vec<f64>>,
    slots: Vec<Vec<f64>>,
    active: Vec<bool>,
    flags: Vec<Flags>,
    steps: u64,
    traces: Vec<Vec<Vec<f64>>>,
    computed: Vec<bool>,
    aborted: bool,
}

impl Batch<'_> {
    fn execute(&mut self) {
        self.init_slots();
        let entry = self.model.blocks.get(0).cloned().unwrap_or_default();
        let instrs = entry.instrs.clone();
        for id in instrs {
            self.instr(id);
        }
        self.capture(0);
    }

    /// A slot can be initialised from an expression over parameters, exactly as in the scalar
    /// interpreter, so the instructions it needs are computed on demand before the main pass.
    fn init_slots(&mut self) {
        let inits: Vec<(usize, Operand)> = self
            .model
            .slots
            .iter()
            .enumerate()
            .map(|(i, sl)| (i, sl.init))
            .collect();
        for (slot, init) in inits {
            for lane in 0..self.count {
                let v = self.pre_read(&init, lane);
                self.slots[slot][lane] = v;
            }
        }
    }

    fn pre_read(&mut self, o: &Operand, lane: usize) -> f64 {
        if let Operand::Node(id) = o {
            self.ensure_computed(*id, lane);
        }
        self.read(o, lane)
    }

    /// Recursively evaluate the pure entry-block instruction behind `id` for one lane. Only pure
    /// operations qualify: a loop or a write must keep running in program order, not on demand.
    fn ensure_computed(&mut self, id: Id, lane: usize) {
        if id as usize >= self.model.instrs.len() || self.computed[id as usize] {
            return;
        }
        let operands: Vec<Operand> = match &self.model.instrs[id as usize].kind {
            InstrKind::Bin { a, b, .. } => vec![*a, *b],
            InstrKind::Un { a, .. } => vec![*a],
            InstrKind::Call { args, .. } => args.clone(),
            _ => return,
        };
        for o in operands {
            self.pre_read(&o, lane);
        }
        self.instr(id);
        self.computed[id as usize] = true;
    }

    fn block(&mut self, id: BlockId) {
        let Some(block) = self.model.blocks.get(id as usize) else {
            return;
        };
        let instrs = block.instrs.clone();
        for step in instrs {
            self.instr(step);
        }
        self.capture(id);
    }

    /// A trace records one row per iteration, holding the value in every lane that is still inside
    /// its loop.
    fn capture(&mut self, id: BlockId) {
        let scopes: Vec<(usize, Operand)> = self
            .model
            .traces
            .iter()
            .enumerate()
            .filter(|(_, t)| t.scope == id)
            .map(|(i, t)| (i, t.value))
            .collect();
        for (i, value) in scopes {
            let row: Vec<f64> = (0..self.count)
                .map(|lane| {
                    if self.active[lane] {
                        self.read(&value, lane)
                    } else {
                        f64::NAN
                    }
                })
                .collect();
            self.traces[i].push(row);
        }
    }

    fn instr(&mut self, id: Id) {
        if self.aborted {
            return;
        }
        self.steps += self.count as u64;
        if self.steps > self.cfg.max_steps {
            self.aborted = true;
            for f in &mut self.flags {
                f.step_budget = true;
            }
            return;
        }
        let Some(instr) = self.model.instrs.get(id as usize) else {
            return;
        };
        match &instr.kind {
            InstrKind::For { trip, body } => {
                let trips: Vec<i64> = (0..self.count).map(|l| self.read(trip, l) as i64).collect();
                let max = trips.iter().copied().max().unwrap_or(0).max(0) as u64;
                let mut remaining: Vec<u64> = trips
                    .iter()
                    .map(|t| if *t < 0 { 0 } else { *t as u64 })
                    .collect();
                for _ in 0..max {
                    for lane in 0..self.count {
                        // Inactive lanes are skipped rather than masked, because the cost of
                        // branching is smaller than the cost of pretending they still run.
                        self.active[lane] = remaining[lane] > 0;
                    }
                    if !self.active.iter().any(|a| *a) {
                        break;
                    }
                    self.block(*body);
                    for lane in 0..self.count {
                        remaining[lane] = remaining[lane].saturating_sub(1);
                    }
                }
                for lane in 0..self.count {
                    self.active[lane] = true;
                }
            }
            InstrKind::Write { slot, value } => {
                let slot = *slot as usize;
                if slot >= self.slots.len() {
                    return;
                }
                // The lane array is taken out, written, and put back: holding a mutable borrow of
                // `self.slots` while `self.read` needs an immutable one is not expressible, and
                // taking avoids copying the buffer.
                let mut row = std::mem::take(&mut self.slots[slot]);
                for lane in 0..self.count {
                    if self.active[lane] {
                        row[lane] = self.read(value, lane);
                    }
                }
                self.slots[slot] = row;
            }
            InstrKind::Bin { op, a, b } => {
                let (op, a, b) = (*op, *a, *b);
                let mut row = std::mem::take(&mut self.env[id as usize]);
                for lane in 0..self.count {
                    if !self.active[lane] {
                        continue;
                    }
                    let x = self.read(&a, lane);
                    let y = self.read(&b, lane);
                    let (v, f) = apply_bin(op, x, y, self.cfg.fp);
                    row[lane] = v;
                    self.flags[lane] = self.flags[lane].merge(f);
                }
                self.env[id as usize] = row;
            }
            InstrKind::Un { op, a } => {
                let (op, a) = (*op, *a);
                let mut row = std::mem::take(&mut self.env[id as usize]);
                for lane in 0..self.count {
                    if !self.active[lane] {
                        continue;
                    }
                    let x = self.read(&a, lane);
                    if op == Unop::IsFinite {
                        row[lane] = if x.is_finite() { 1.0 } else { 0.0 };
                        continue;
                    }
                    let (v, f) = ops::un_f64(op, x, self.cfg.fp);
                    row[lane] = v;
                    self.flags[lane] = self.flags[lane].merge(f);
                }
                self.env[id as usize] = row;
            }
            InstrKind::Call { builtin, args } => {
                let builtin = *builtin;
                let mut row = std::mem::take(&mut self.env[id as usize]);
                let nargs = args.len();
                for lane in 0..self.count {
                    if !self.active[lane] {
                        continue;
                    }
                    // The argument list is read one operand at a time; no Vec is allocated inside
                    // the lane loop, because this is the hottest line in the batch path.
                    let mut vals = [0.0f64; 3];
                    for (i, arg) in args.iter().enumerate().take(3) {
                        vals[i] = self.read(arg, lane);
                    }
                    let (v, f) = ops::call_f64(builtin, &vals[..nargs], self.cfg.fp);
                    row[lane] = v;
                    self.flags[lane] = self.flags[lane].merge(f);
                }
                self.env[id as usize] = row;
            }
        }
    }

    fn read(&self, o: &Operand, lane: usize) -> f64 {
        match o {
            Operand::Lit(l) => match l {
                Lit::F64(v) => *v,
                Lit::F32(v) => f64::from(*v),
                Lit::I64(v) => *v as f64,
                Lit::Bool(b) => {
                    if *b {
                        1.0
                    } else {
                        0.0
                    }
                }
            },
            Operand::Param(i) => self.xs[lane * self.arity + *i as usize],
            Operand::Slot(i) => self
                .slots
                .get(*i as usize)
                .map(|s| s[lane])
                .unwrap_or(f64::NAN),
            Operand::Node(id) => self
                .env
                .get(*id as usize)
                .map(|r| r[lane])
                .unwrap_or(f64::NAN),
        }
    }
}

fn init_lane(o: Operand, count: usize) -> f64 {
    let v = match o {
        Operand::Lit(Lit::F64(x)) => x,
        Operand::Lit(Lit::F32(x)) => f64::from(x),
        Operand::Lit(Lit::I64(x)) => x as f64,
        Operand::Lit(Lit::Bool(b)) => {
            if b {
                1.0
            } else {
                0.0
            }
        }
        // Resolved for real by `init_slots`, which can reach parameters and expressions.
        _ => f64::NAN,
    };
    let _ = count;
    v
}

#[must_use]
fn apply_bin(op: Binop, a: f64, b: f64, mode: crate::value::FpMode) -> (f64, Flags) {
    if matches!(
        op,
        Binop::Lt
            | Binop::Le
            | Binop::Gt
            | Binop::Ge
            | Binop::Eq
            | Binop::Ne
            | Binop::And
            | Binop::Or
    ) {
        return ops::compare_f64(op, a, b);
    }
    ops::bin_f64(op, a, b, mode)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interp::run;
    use aporia_dsl::lower::compile;

    fn model(text: &str) -> Model {
        let c = compile("t.ap", text);
        assert!(!c.diagnostics.has_errors(), "{}", c.diagnostics);
        c.model
    }

    const CATCH_ALL: &str = "model c \"\" {\n input x in [0, 10]\n let y = x * x - 3\n}\n";

    #[test]
    fn batch_matches_scalar_on_a_straight_model() {
        let m = model(CATCH_ALL);
        let xs: Vec<f64> = (0..64).map(|i| i as f64 * 0.13).collect();
        let batch = run_batch(&m, &xs, 64, ExecConfig::default());
        for lane in 0..64 {
            let one = run(&m, &[xs[lane]], ExecConfig::default());
            assert_eq!(
                batch.outputs[0][lane], one.outputs[0],
                "lane {lane} disagrees"
            );
        }
    }

    #[test]
    fn lanes_may_loop_different_numbers_of_times() {
        let m = model(
            "model l \"\" {\n input n : count in [0, 8]\n state x = 0\n loop n {\n advance x = x + 2\n }\n let y = x\n}\n",
        );
        let xs = vec![0.0, 1.0, 3.0, 8.0];
        let batch = run_batch(&m, &xs, 4, ExecConfig::default());
        let expect: Vec<f64> = vec![0.0, 2.0, 6.0, 16.0];
        assert_eq!(batch.outputs[0], expect);
        for (lane, x) in xs.iter().enumerate() {
            assert_eq!(
                batch.outputs[0][lane],
                run(&m, &[*x], ExecConfig::default()).outputs[0]
            );
        }
    }

    #[test]
    fn traces_keep_their_ragged_shape_per_lane() {
        let m = model(
            "model t \"\" {\n input n : count in [1, 5]\n state x = 1.0\n loop n {\n advance x = x * 2\n watch x\n }\n let y = x\n}\n",
        );
        let xs = vec![3.0, 1.0];
        let batch = run_batch(&m, &xs, 2, ExecConfig::default());
        // Two iterations max, with the second candidate inactive in the later rows.
        assert_eq!(batch.traces[0].len(), 3);
        assert_eq!(batch.traces[0][0], vec![2.0, 2.0]);
        assert!(
            batch.traces[0][2][1].is_nan(),
            "the finished lane records nothing"
        );
        assert_eq!(batch.traces[0][2][0], 8.0);
    }

    #[test]
    fn one_bad_lane_does_not_contaminate_the_batch() {
        let m = model("model d \"\" {\n input x in [-4, 4]\n let y = sqrt(x)\n}\n");
        let xs = vec![4.0, -1.0, 9.0];
        let batch = run_batch(&m, &xs, 3, ExecConfig::default());
        assert_eq!(batch.outputs[0][0], 2.0);
        assert!(batch.outputs[0][1].is_nan());
        assert_eq!(batch.outputs[0][2], 3.0);
        assert!(batch.flags[0].is_clean());
        assert!(batch.flags[1].invalid_domain);
        assert!(batch.flags[2].is_clean());
    }

    #[test]
    fn a_short_batch_is_a_caller_error_and_says_so() {
        let m = model(CATCH_ALL);
        let r = std::panic::catch_unwind(|| run_batch(&m, &[1.0, 2.0], 5, ExecConfig::default()));
        assert!(r.is_err(), "arity 1 with 5 candidates needs 5 values");
    }

    #[test]
    fn steps_are_counted_per_lane_so_cost_scales() {
        let m = model(CATCH_ALL);
        let one = run_batch(&m, &[1.0], 1, ExecConfig::default());
        let many = run_batch(&m, &[1.0; 16], 16, ExecConfig::default());
        assert_eq!(many.steps, one.steps * 16);
    }

    #[test]
    fn comparisons_are_lane_wise() {
        let m =
            model("model b \"\" {\n input x in [-5, 5]\n let s = x > 0\n let y = min(x, 0)\n}\n");
        let xs = vec![-2.0, 3.0];
        let batch = run_batch(&m, &xs, 2, ExecConfig::default());
        assert_eq!(batch.outputs[0], vec![0.0, 1.0]);
        assert_eq!(batch.outputs[1], vec![-2.0, 0.0]);
    }
}
