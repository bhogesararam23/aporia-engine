//! The A-IR object model.
//!
//! A-IR is deliberately tiny. A model is a list of parameters with domains, a few mutable slots,
//! blocks of three-address instructions, and the declarations APORIA analyses against: outputs,
//! traces, constraints and relations. Everything APORIA does downstream — interpreting, batching
//! across candidates, vectorising, emitting native code, running the same maths on a GPU — consumes
//! this shape and nothing richer.
//!
//! Two decisions shape the whole representation and are worth stating here rather than leaving to
//! be inferred:
//!
//! 1. **Values are referenced by [`Operand`], not by node index alone.** A leaf is either a
//!    parameter, a slot read, an instruction result, or a literal. That keeps the instruction list
//!    free of pass-through nodes and makes the dominance rule trivial to check: an operand may
//!    only refer to an earlier instruction in the same block or an enclosing block.
//! 2. **Slots are mutable; A-IR is not SSA.** Scientific models are written as state that is
//!    advanced (`v = v + a*dt`), and an executable IR that mirrors that is both easier to interpret
//!    and easier to prove right by comparison against a reference implementation. The cost is that
//!    a vectorised backend must give every candidate its own slot array, which is exactly what the
//!    batch evaluator does anyway.

use crate::dim::{NumType, Ty};

/// Index of an instruction in [`Model::instrs`].
pub type Id = u32;
/// Index of a declared parameter.
pub type ParamId = u16;
/// Index of a declared slot.
pub type SlotId = u16;
/// Index of an output.
pub type OutputId = u16;
/// Index of a traced quantity.
pub type TraceId = u16;
/// Index of a block.
pub type BlockId = u32;
/// Index of a constraint.
pub type ConstraintId = u16;
/// Index of a relation.
pub type RelationId = u16;

/// A literal value baked into the model.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Lit {
    F64(f64),
    F32(f32),
    I64(i64),
    Bool(bool),
}

impl Lit {
    #[must_use]
    pub fn ty(&self) -> NumType {
        match self {
            Self::F64(_) => NumType::F64,
            Self::F32(_) => NumType::F32,
            Self::I64(_) => NumType::I64,
            Self::Bool(_) => NumType::Bool,
        }
    }

    #[must_use]
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::F64(v) => Some(*v),
            Self::F32(v) => Some(f64::from(*v)),
            Self::I64(v) => Some(*v as f64),
            Self::Bool(v) => Some(if *v { 1.0 } else { 0.0 }),
        }
    }

    /// Exact, reversible text form. Floats are written as bit patterns so a round trip through the
    /// textual IR cannot lose a last-bit difference — those differences are what APORIA is looking
    /// for in the numerical channel.
    #[must_use]
    pub fn to_canonical(&self) -> String {
        match self {
            Self::F64(v) => format!("lf64:{:#018x}", v.to_bits()),
            Self::F32(v) => format!("lf32:{:#010x}", v.to_bits()),
            Self::I64(v) => format!("li64:{v}"),
            Self::Bool(v) => format!("lb:{v}"),
        }
    }
}

/// A reference to a value available at some program point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Operand {
    Param(ParamId),
    Slot(SlotId),
    Node(Id),
    Lit(Lit),
}

impl Operand {
    /// Textual form used by the `.air` dump.
    #[must_use]
    pub fn to_canonical(&self) -> String {
        match self {
            Self::Param(i) => format!("p{i}"),
            Self::Slot(i) => format!("s{i}"),
            Self::Node(i) => format!("n{i}"),
            Self::Lit(l) => l.to_canonical(),
        }
    }
}

/// Two-input arithmetic and comparisons.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Binop {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    /// `base^exponent`. Dimension is only preserved for a literal integer exponent; anything else
    /// makes the result dimension unknown and the verifier records that rather than guessing.
    Pow,
    Min,
    Max,
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
    And,
    Or,
}

impl Binop {
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Add => "add",
            Self::Sub => "sub",
            Self::Mul => "mul",
            Self::Div => "div",
            Self::Rem => "rem",
            Self::Pow => "pow",
            Self::Min => "min",
            Self::Max => "max",
            Self::Lt => "lt",
            Self::Le => "le",
            Self::Gt => "gt",
            Self::Ge => "ge",
            Self::Eq => "eq",
            Self::Ne => "ne",
            Self::And => "and",
            Self::Or => "or",
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.iter().find(|op| op.name() == name).copied()
    }

    /// Comparisons and logic produce booleans, arithmetic produces numbers.
    #[must_use]
    pub const fn is_predicate(&self) -> bool {
        matches!(
            self,
            Self::Lt | Self::Le | Self::Gt | Self::Ge | Self::Eq | Self::Ne | Self::And | Self::Or
        )
    }

    /// Additive: requires equal dimensions. Multiplicative: combines them.
    #[must_use]
    pub const fn is_additive(&self) -> bool {
        matches!(self, Self::Add | Self::Sub | Self::Min | Self::Max)
    }

    #[must_use]
    pub fn result_num(&self) -> NumType {
        if self.is_predicate() {
            NumType::Bool
        } else {
            NumType::F64
        }
    }

    pub const ALL: &'static [Self] = &[
        Self::Add,
        Self::Sub,
        Self::Mul,
        Self::Div,
        Self::Rem,
        Self::Pow,
        Self::Min,
        Self::Max,
        Self::Lt,
        Self::Le,
        Self::Gt,
        Self::Ge,
        Self::Eq,
        Self::Ne,
        Self::And,
        Self::Or,
    ];
}

/// One-input operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Unop {
    Neg,
    Abs,
    Sqrt,
    Cbrt,
    Exp,
    Ln,
    Log2,
    Log10,
    Sin,
    Cos,
    Tan,
    Asin,
    Acos,
    Atan,
    Sinh,
    Cosh,
    Tanh,
    Floor,
    Ceil,
    Round,
    Trunc,
    Not,
    /// "this value is neither NaN nor infinite". A predicate, because a scientific model reaching
    /// infinity is a legitimate outcome and APORIA needs to record where it happens.
    IsFinite,
    /// Widening or narrowing conversion; the verifier rejects lossy ones unless explicit.
    Cast(NumType),
}

impl Unop {
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Neg => "neg",
            Self::Abs => "abs",
            Self::Sqrt => "sqrt",
            Self::Cbrt => "cbrt",
            Self::Exp => "exp",
            Self::Ln => "ln",
            Self::Log2 => "log2",
            Self::Log10 => "log10",
            Self::Sin => "sin",
            Self::Cos => "cos",
            Self::Tan => "tan",
            Self::Asin => "asin",
            Self::Acos => "acos",
            Self::Atan => "atan",
            Self::Sinh => "sinh",
            Self::Cosh => "cosh",
            Self::Tanh => "tanh",
            Self::Floor => "floor",
            Self::Ceil => "ceil",
            Self::Round => "round",
            Self::Trunc => "trunc",
            Self::IsFinite => "is_finite",
            Self::Not => "not",
            Self::Cast(t) => match t {
                NumType::F64 => "cast_f64",
                NumType::F32 => "cast_f32",
                NumType::I64 => "cast_i64",
                NumType::Bool => "cast_bool",
                // Casting to unit is not something the front end emits; named so the opcode table
                // stays total and a bad cast is visible in a text dump instead of panicking.
                NumType::Unit => "cast_unit",
            },
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.iter().find(|op| op.name() == name).copied()
    }

    /// Transcendentals are only meaningful on dimensionless input; `sqrt`/`cbrt` change dimension.
    #[must_use]
    pub const fn requires_dimensionless(&self) -> bool {
        matches!(
            self,
            Self::Exp
                | Self::Ln
                | Self::Log2
                | Self::Log10
                | Self::Sin
                | Self::Cos
                | Self::Tan
                | Self::Asin
                | Self::Acos
                | Self::Atan
                | Self::Sinh
                | Self::Cosh
                | Self::Tanh
        )
    }

    #[must_use]
    pub const fn is_predicate(&self) -> bool {
        matches!(self, Self::Not | Self::IsFinite)
    }

    pub const ALL: &'static [Self] = &[
        Self::Neg,
        Self::Abs,
        Self::Sqrt,
        Self::Cbrt,
        Self::Exp,
        Self::Ln,
        Self::Log2,
        Self::Log10,
        Self::Sin,
        Self::Cos,
        Self::Tan,
        Self::Asin,
        Self::Acos,
        Self::Atan,
        Self::Sinh,
        Self::Cosh,
        Self::Tanh,
        Self::Floor,
        Self::Ceil,
        Self::Round,
        Self::Trunc,
        Self::Not,
        Self::IsFinite,
        Self::Cast(NumType::F64),
        Self::Cast(NumType::F32),
        Self::Cast(NumType::I64),
        Self::Cast(NumType::Bool),
    ];
}

/// Builtins that take a fixed, non-homogeneous argument list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Builtin {
    /// `atan2(y, x)`, two arguments, dimensionless result.
    Atan2,
    /// `hypot(a, b)`, two arguments of equal dimension, same dimension result.
    Hypot,
    /// `fma(a, b, c)` — `a*b + c` rounded once. Present because a fused multiply-add is an
    /// independent numerical path: comparing it against separate multiply then add is a
    /// legitimate source of numerical evidence.
    Fma,
    /// `clamp(x, lo, hi)`.
    Clamp,
    /// `lerp(a, b, t)` with dimensionless `t`.
    Lerp,
}

impl Builtin {
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Atan2 => "atan2",
            Self::Hypot => "hypot",
            Self::Fma => "fma",
            Self::Clamp => "clamp",
            Self::Lerp => "lerp",
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.iter().find(|b| b.name() == name).copied()
    }

    #[must_use]
    pub const fn arity(&self) -> usize {
        match self {
            Self::Atan2 | Self::Hypot => 2,
            Self::Fma | Self::Clamp | Self::Lerp => 3,
        }
    }

    pub const ALL: &'static [Self] =
        &[Self::Atan2, Self::Hypot, Self::Fma, Self::Clamp, Self::Lerp];
}

/// A computed value.
#[derive(Clone, Debug, PartialEq)]
pub enum InstrKind {
    Bin {
        op: Binop,
        a: Operand,
        b: Operand,
    },
    Un {
        op: Unop,
        a: Operand,
    },
    Call {
        builtin: Builtin,
        args: Vec<Operand>,
    },
    /// Execute `body` `trip` times. The body may write slots; A-IR loops carry state in slots
    /// instead of threading region results.
    For {
        trip: Operand,
        body: BlockId,
    },
    /// Store into a slot. Produces no value.
    Write {
        slot: SlotId,
        value: Operand,
    },
    /// A value this model does not define how to compute: an external program produces it.
    ///
    /// This exists so that "I cannot run this" is a fact about the representation rather than a
    /// behaviour. A model adapted from a program APORIA did not parse declares its parameters, its
    /// outputs, and the rules it expects those outputs to satisfy — and says nothing about how the
    /// numbers arise. Every path that interprets A-IR has to stop at this instruction by the type, so
    /// none of them can quietly return a plausible zero: the scalar interpreter, the batched
    /// interpreter and the double-double reference all refuse, and only an executor that was actually
    /// given the program can answer for it.
    ///
    /// It carries no operands on purpose. The values an external program needs are the model's
    /// parameters, which the caller already passes to the execution path; repeating them here would
    /// create a second, contradictable description of the same inputs.
    Opaque,
}

/// One instruction plus the type inferred for its result.
#[derive(Clone, Debug, PartialEq)]
pub struct Instr {
    pub ty: Ty,
    pub kind: InstrKind,
}

/// A straight-line sequence of instructions.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Block {
    pub instrs: Vec<Id>,
}

/// How a quantity may vary, and therefore what a sampler is allowed to draw.
#[derive(Clone, Debug, PartialEq)]
pub enum Domain {
    /// Continuous inclusive range.
    Interval { lo: f64, hi: f64 },
    /// A discrete setting, for things like a solver choice index or a fixed list of test points.
    Choices(Vec<f64>),
}

impl Domain {
    #[must_use]
    pub fn interval(lo: f64, hi: f64) -> Self {
        Self::Interval { lo, hi }
    }

    #[must_use]
    pub fn midpoint(&self) -> f64 {
        match self {
            Self::Interval { lo, hi } => lo + (hi - lo) / 2.0,
            Self::Choices(v) if v.is_empty() => 0.0,
            Self::Choices(v) => v[v.len() / 2],
        }
    }

    #[must_use]
    pub fn is_unbounded(&self) -> bool {
        match self {
            Self::Interval { lo, hi } => !lo.is_finite() || !hi.is_finite(),
            Self::Choices(_) => false,
        }
    }
}

/// A declared input of the model.
#[derive(Clone, Debug, PartialEq)]
pub struct Param {
    pub name: String,
    pub ty: Ty,
    pub domain: Domain,
    /// Scale factor from the declared unit to SI base units, `1.0` when the parameter is already
    /// in base units. Recorded so an adapter or reference implementation can agree on units.
    pub to_si: f64,
    pub doc: String,
}

/// A mutable cell, used for state that a loop advances.
#[derive(Clone, Debug, PartialEq)]
pub struct Slot {
    pub name: String,
    pub ty: Ty,
    pub init: Operand,
}

/// A named quantity the analyst wants to see.
#[derive(Clone, Debug, PartialEq)]
pub struct Output {
    pub name: String,
    pub ty: Ty,
    pub value: Operand,
    pub doc: String,
}

/// A quantity sampled once per iteration of the block it is declared in.
///
/// Traces exist because a single final value hides the thing APORIA cares most about: a trajectory
/// that quietly diverges and returns, or an energy that drifts by one part in ten thousand per
/// step.
#[derive(Clone, Debug, PartialEq)]
pub struct Trace {
    pub name: String,
    pub ty: Ty,
    pub value: Operand,
    pub scope: BlockId,
}

/// Comparison used by constraints.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CmpOp {
    Lt,
    Le,
    Gt,
    Ge,
    EqApprox,
}

impl CmpOp {
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Lt => "lt",
            Self::Le => "le",
            Self::Gt => "gt",
            Self::Ge => "ge",
            Self::EqApprox => "~~",
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "lt" | "<" => Self::Lt,
            "le" | "<=" => Self::Le,
            "gt" | ">" => Self::Gt,
            "ge" | ">=" => Self::Ge,
            "~~" | "==" => Self::EqApprox,
            _ => return None,
        })
    }

    /// Evaluate the predicate with an absolute-plus-relative tolerance, which is what makes
    /// `EqApprox` usable across quantities of very different magnitude.
    #[must_use]
    pub fn holds(&self, lhs: f64, rhs: f64, tol: f64) -> bool {
        match self {
            Self::Lt => lhs < rhs,
            Self::Le => lhs <= rhs,
            Self::Gt => lhs > rhs,
            Self::Ge => lhs >= rhs,
            Self::EqApprox => (lhs - rhs).abs() <= tol * rhs.abs().max(1.0),
        }
    }
}

/// A rule that must hold at every evaluation. Violating it is physical/mathematical evidence.
#[derive(Clone, Debug, PartialEq)]
pub struct Constraint {
    pub id: ConstraintId,
    pub name: String,
    pub kind: ConstraintKind,
    pub origin: Origin,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ConstraintKind {
    /// `lhs cmp rhs`, with `tolerance` used only by [`CmpOp::EqApprox`].
    Cmp {
        lhs: Operand,
        cmp: CmpOp,
        rhs: Operand,
        tolerance: f64,
    },
    /// The value must be neither NaN nor infinite.
    Finite { value: Operand },
}

/// A expected behaviour across the parameter space rather than at one point.
#[derive(Clone, Debug, PartialEq)]
pub struct Relation {
    pub id: RelationId,
    pub name: String,
    pub kind: RelationKind,
    pub origin: Origin,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Increasing,
    Decreasing,
}

impl Direction {
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Increasing => "up",
            Self::Decreasing => "down",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum RelationKind {
    /// `out` never decreases / never increases as `param` grows, holding others fixed.
    Monotone {
        out: OutputId,
        param: ParamId,
        direction: Direction,
    },
    /// `out ~ param^power` over the declared domain.
    ScalesAs {
        out: OutputId,
        param: ParamId,
        power: f64,
    },
    /// Swapping the values of two parameters leaves `out` unchanged.
    Symmetric { out: OutputId, pair: [ParamId; 2] },
    /// A traced quantity stays within `tolerance` relative drift of its first value.
    Conserved { trace: TraceId, tolerance: f64 },
    /// Local change in `out` with respect to `param` stays under `bound` per unit of `param`.
    Lipschitz {
        out: OutputId,
        param: ParamId,
        bound: f64,
    },
}

/// Where a declaration came from: a human wrote it, or APORIA inferred it and wants it tested.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    Declared,
    Inferred,
}

/// A complete A-IR model.
#[derive(Clone, Debug, PartialEq)]
pub struct Model {
    pub name: String,
    pub doc: String,
    pub params: Vec<Param>,
    pub slots: Vec<Slot>,
    /// Block `0` is the entry block.
    pub blocks: Vec<Block>,
    pub instrs: Vec<Instr>,
    pub outputs: Vec<Output>,
    pub traces: Vec<Trace>,
    pub constraints: Vec<Constraint>,
    pub relations: Vec<Relation>,
}

impl Model {
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            doc: String::new(),
            params: Vec::new(),
            slots: Vec::new(),
            blocks: vec![Block::default()],
            instrs: Vec::new(),
            outputs: Vec::new(),
            traces: Vec::new(),
            constraints: Vec::new(),
            relations: Vec::new(),
        }
    }

    /// Append an instruction to the end of the entry block and return its id.
    pub fn push_entry(&mut self, instr: Instr) -> Id {
        let id = self.instrs.len() as Id;
        self.instrs.push(instr);
        self.blocks[0].instrs.push(id);
        id
    }

    /// Create a block and return its id.
    pub fn new_block(&mut self) -> BlockId {
        let id = self.blocks.len() as BlockId;
        self.blocks.push(Block::default());
        id
    }

    /// Append an instruction to an arbitrary block.
    pub fn push(&mut self, block: BlockId, instr: Instr) -> Id {
        let id = self.instrs.len() as Id;
        self.instrs.push(instr);
        self.blocks[block as usize].instrs.push(id);
        id
    }

    /// Total number of distinct values reachable as parameters, in declaration order.
    #[must_use]
    pub fn arity(&self) -> usize {
        self.params.len()
    }

    /// Locate a parameter by name.
    #[must_use]
    pub fn param(&self, name: &str) -> Option<ParamId> {
        self.params
            .iter()
            .position(|p| p.name == name)
            .map(|i| i as ParamId)
    }

    /// Locate an output by name.
    #[must_use]
    pub fn output(&self, name: &str) -> Option<OutputId> {
        self.outputs
            .iter()
            .position(|o| o.name == name)
            .map(|i| i as OutputId)
    }

    /// Locate a slot by name.
    #[must_use]
    pub fn slot(&self, name: &str) -> Option<SlotId> {
        self.slots
            .iter()
            .position(|s| s.name == name)
            .map(|i| i as SlotId)
    }

    /// Every instruction result that a declared rule reads, in first-mention order.
    ///
    /// A rule like `require abs(root - stable) < bound` compares two computed values. They are not
    /// outputs — the author asked for a check, not a measurement — and an observation that only
    /// carries parameter and output values cannot answer whether the rule held. This is the list an
    /// execution has to record so the physical channel can evaluate the rules that were actually
    /// written, instead of only the ones that happened to name a top-level value.
    ///
    /// Relations are not included: they name outputs and traces, both of which an observation
    /// already carries.
    #[must_use]
    pub fn rule_nodes(&self) -> Vec<Id> {
        let mut out: Vec<Id> = Vec::new();
        let mut mention = |operand: &Operand| {
            if let Operand::Node(id) = operand
                && !out.contains(id)
            {
                out.push(*id);
            }
        };
        for c in &self.constraints {
            match &c.kind {
                ConstraintKind::Cmp { lhs, rhs, .. } => {
                    mention(lhs);
                    mention(rhs);
                }
                ConstraintKind::Finite { value } => mention(value),
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dim::{Dimension, LENGTH};

    #[test]
    fn operand_text_is_unambiguous() {
        assert_eq!(Operand::Param(3).to_canonical(), "p3");
        assert_eq!(Operand::Node(12).to_canonical(), "n12");
        assert_eq!(
            Operand::Lit(Lit::F64(1.5)).to_canonical(),
            format!("lf64:{:#018x}", 1.5f64.to_bits())
        );
    }

    #[test]
    fn literal_floats_round_trip_bit_exactly() {
        let awkward = [0.1f64, 1.0 / 3.0, -0.0, f64::MIN, 5e-324];
        for v in awkward {
            let text = Lit::F64(v).to_canonical();
            let bits = u64::from_str_radix(
                text.trim_start_matches("lf64:").trim_start_matches("0x"),
                16,
            )
            .unwrap();
            assert_eq!(f64::from_bits(bits), v, "exact bits for {v}");
        }
    }

    #[test]
    fn predicates_type_as_bool_and_arithmetic_as_f64() {
        assert_eq!(Binop::Le.result_num(), NumType::Bool);
        assert_eq!(Binop::Mul.result_num(), NumType::F64);
        assert!(Binop::Add.is_additive());
        assert!(!Binop::Mul.is_additive());
    }

    #[test]
    fn every_op_name_round_trips() {
        for op in Binop::ALL {
            assert_eq!(Binop::from_name(op.name()), Some(*op));
        }
        for op in Unop::ALL {
            assert_eq!(Unop::from_name(op.name()), Some(*op));
        }
        for b in Builtin::ALL {
            assert_eq!(Builtin::from_name(b.name()), Some(*b));
        }
    }

    #[test]
    fn cmp_holds_uses_relative_tolerance() {
        assert!(CmpOp::EqApprox.holds(1.0, 1.0 + 1e-12, 1e-9));
        assert!(!CmpOp::EqApprox.holds(1.0, 1.1, 1e-9));
        assert!(CmpOp::Ge.holds(0.0, 0.0, 0.0));
        assert!(!CmpOp::Gt.holds(0.0, 0.0, 0.0));
    }

    #[test]
    fn domain_midpoint_handles_choices() {
        assert_eq!(Domain::interval(0.0, 10.0).midpoint(), 5.0);
        assert_eq!(Domain::Choices(vec![1.0, 2.0, 3.0]).midpoint(), 2.0);
        assert!(Domain::interval(0.0, f64::INFINITY).is_unbounded());
    }

    #[test]
    fn push_entry_allocates_in_order() {
        let mut m = Model::new("t");
        let a = m.push_entry(Instr {
            ty: Ty::float(Dimension::base(LENGTH, 1)),
            kind: InstrKind::Un {
                op: Unop::Abs,
                a: Operand::Param(0),
            },
        });
        let b = m.push_entry(Instr {
            ty: Ty::boolean(),
            kind: InstrKind::Bin {
                op: Binop::Lt,
                a: Operand::Node(a),
                b: Operand::Lit(Lit::F64(1.0)),
            },
        });
        assert_eq!((a, b), (0, 1));
        assert_eq!(m.blocks[0].instrs, vec![0, 1]);
        assert_eq!(m.instrs[b as usize].ty.num, NumType::Bool);
    }
}
