//! Structural and dimensional verification of an A-IR model.
//!
//! The front end is allowed to produce a model that APORIA dislikes (a variable exponent, an
//! unknown dimension) and the checker is allowed to warn about it. What must never happen is a
//! model that is structurally unsound reaching the runtime: a dangling operand, an instruction used
//! before it is defined, a loop body that reaches outside its scope. Those are verifier errors, and
//! every backend may assume the verifier passed.

use crate::ir::{
    Binop, BlockId, Builtin, ConstraintKind, Domain, Id, Instr, InstrKind, Model, Operand, Unop,
};
use std::fmt;

/// A problem found by the verifier.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerError {
    /// Instruction the problem was found at, when there is one.
    pub at: Option<Id>,
    pub message: String,
}

impl VerError {
    fn at(id: Id, message: impl Into<String>) -> Self {
        Self {
            at: Some(id),
            message: message.into(),
        }
    }

    fn global(message: impl Into<String>) -> Self {
        Self {
            at: None,
            message: message.into(),
        }
    }
}

impl fmt::Display for VerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.at {
            Some(id) => write!(f, "n{id}: {}", self.message),
            None => write!(f, "{}", self.message),
        }
    }
}

impl std::error::Error for VerError {}

/// Result of verifying a model: the errors, and the non-fatal findings.
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub errors: Vec<VerError>,
    pub warnings: Vec<VerError>,
}

impl Report {
    #[must_use]
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }
}

/// Verify a model.
#[must_use]
pub fn verify(model: &Model) -> Report {
    let mut report = Report::default();
    let defs = collect_defs(model, &mut report);
    check_structure(model, &defs, &mut report);
    check_names(model, &mut report);
    check_domains(model, &mut report);
    check_types(model, &mut report);
    check_declarations(model, &defs, &mut report);
    report
}

/// Which block defines each instruction, and whether it is a loop body of another block.
struct Scope {
    /// `def_block[id]` = block that contains instruction `id`.
    def_block: Vec<BlockId>,
    /// `parent[b]` = block that owns block `b` through a `For`, or `None` for the entry block.
    parent: Vec<Option<BlockId>>,
}

fn collect_defs(model: &Model, report: &mut Report) -> Scope {
    let mut def_block = vec![u32::MAX; model.instrs.len()];
    let mut parent = vec![None; model.blocks.len()];
    for (bid, block) in model.blocks.iter().enumerate() {
        for id in &block.instrs {
            let id = *id as usize;
            if id >= model.instrs.len() {
                report.errors.push(VerError::global(format!(
                    "block {bid} refers to missing instruction n{id}"
                )));
                continue;
            }
            if def_block[id] != u32::MAX {
                report.errors.push(VerError::at(
                    id as Id,
                    format!(
                        "instruction defined twice (blocks {} and {bid})",
                        def_block[id]
                    ),
                ));
            }
            def_block[id] = bid as BlockId;
        }
        // A `For` names its body block; record the nesting edge here.
        for id in &block.instrs {
            if let Some(InstrKind::For { body, .. }) =
                model.instrs.get(*id as usize).map(|i| &i.kind)
            {
                let b = *body as usize;
                if b >= model.blocks.len() {
                    report.errors.push(VerError::at(
                        *id,
                        format!("loop body block {b} does not exist"),
                    ));
                } else if parent[b].is_some() {
                    report.errors.push(VerError::at(
                        *id,
                        format!("block {b} is used as two different loop bodies"),
                    ));
                } else {
                    if b == 0 {
                        report
                            .errors
                            .push(VerError::at(*id, "loop body may not be the entry block"));
                    }
                    parent[b] = Some(bid as BlockId);
                }
            }
        }
    }
    for (id, instr) in model.instrs.iter().enumerate() {
        if def_block[id] == u32::MAX {
            report.errors.push(VerError::at(
                id as Id,
                format!("instruction is in no block: {}", describe(instr)),
            ));
        }
    }
    Scope { def_block, parent }
}

fn describe(instr: &Instr) -> String {
    match &instr.kind {
        InstrKind::Bin { op, a, b } => {
            format!("bin {op:?} {} {}", a.to_canonical(), b.to_canonical())
        }
        InstrKind::Un { op, a } => format!("un {op:?} {}", a.to_canonical()),
        InstrKind::Call { builtin, args } => format!("call {builtin:?} ({})", args.len()),
        InstrKind::For { trip, body } => format!("for {} block {body}", trip.to_canonical()),
        InstrKind::Write { slot, value } => {
            format!("write s{slot} {}", value.to_canonical())
        }
        InstrKind::Opaque => "opaque".to_string(),
    }
}

/// An operand may name a parameter, a slot, a literal, or an instruction that is already visible.
fn operand_visible(
    model: &Model,
    scope: &Scope,
    use_block: BlockId,
    use_id: Id,
    operand: &Operand,
    report: &mut Report,
) {
    match operand {
        Operand::Param(i) => {
            if *i as usize >= model.params.len() {
                report.errors.push(VerError::at(
                    use_id,
                    format!("parameter p{i} is not declared"),
                ));
            }
        }
        Operand::Slot(i) => {
            if *i as usize >= model.slots.len() {
                report
                    .errors
                    .push(VerError::at(use_id, format!("slot s{i} is not declared")));
            }
        }
        Operand::Lit(_) => {}
        Operand::Node(id) => {
            let Some(&def) = scope.def_block.get(*id as usize) else {
                report
                    .errors
                    .push(VerError::at(use_id, format!("n{id} does not exist")));
                return;
            };
            if def == u32::MAX {
                report
                    .errors
                    .push(VerError::at(use_id, format!("n{id} is not in any block")));
                return;
            }
            if def == use_block {
                if *id >= use_id {
                    report.errors.push(VerError::at(
                        use_id,
                        format!("n{id} is used before it is defined in block {use_block}"),
                    ));
                }
            } else if !is_ancestor(scope, def, use_block) {
                report.errors.push(VerError::at(
                    use_id,
                    format!("n{id} is defined in block {def}, which is not visible from block {use_block}"),
                ));
            }
        }
    }
}

fn is_ancestor(scope: &Scope, candidate: BlockId, mut here: BlockId) -> bool {
    if candidate == here {
        return true;
    }
    while let Some(Some(p)) = scope.parent.get(here as usize) {
        if *p == candidate {
            return true;
        }
        here = *p;
    }
    false
}

fn check_structure(model: &Model, scope: &Scope, report: &mut Report) {
    if model.blocks.is_empty() {
        report.errors.push(VerError::global("model has no blocks"));
        return;
    }
    for (bid, block) in model.blocks.iter().enumerate() {
        for pos in 0..block.instrs.len() {
            let id = block.instrs[pos];
            let Some(instr) = model.instrs.get(id as usize) else {
                continue;
            };
            let block_id = bid as BlockId;
            match &instr.kind {
                InstrKind::Bin { a, b, .. } => {
                    operand_visible(model, scope, block_id, id, a, report);
                    operand_visible(model, scope, block_id, id, b, report);
                }
                InstrKind::Un { a, .. } => operand_visible(model, scope, block_id, id, a, report),
                InstrKind::Call { args, builtin } => {
                    if args.len() != builtin.arity() {
                        report.errors.push(VerError::at(
                            id,
                            format!(
                                "{} takes {} arguments, found {}",
                                builtin.name(),
                                builtin.arity(),
                                args.len()
                            ),
                        ));
                    }
                    for arg in args {
                        operand_visible(model, scope, block_id, id, arg, report);
                    }
                }
                InstrKind::For { trip, .. } => {
                    operand_visible(model, scope, block_id, id, trip, report);
                }
                InstrKind::Write { value, .. } => {
                    operand_visible(model, scope, block_id, id, value, report);
                }
                InstrKind::Opaque => {
                    // Nothing to resolve: the value comes from outside the representation, which is
                    // exactly why an interpreter is not allowed to invent one.
                }
            }
        }
    }
    for slot in &model.slots {
        if let Operand::Node(id) = slot.init {
            let visible = scope
                .def_block
                .get(id as usize)
                .is_some_and(|&b| b != u32::MAX && is_ancestor(scope, b, 0));
            if !visible {
                report.errors.push(VerError::global(format!(
                    "slot {} initialiser n{id} is not visible at entry",
                    slot.name
                )));
            }
        }
    }
}

fn check_names(model: &Model, report: &mut Report) {
    let mut seen: Vec<(String, String)> = Vec::new();
    let mut push = |kind: &str, name: &str, report: &mut Report| {
        if name.is_empty() {
            report
                .errors
                .push(VerError::global(format!("{kind} has an empty name")));
        }
        if seen.iter().any(|(k, n)| n == name && k == kind) {
            report
                .errors
                .push(VerError::global(format!("duplicate {kind} name `{name}`")));
        }
        seen.push((kind.to_string(), name.to_string()));
    };
    for p in &model.params {
        push("parameter", &p.name, report);
    }
    for s in &model.slots {
        push("slot", &s.name, report);
    }
    for o in &model.outputs {
        push("output", &o.name, report);
    }
    if model.name.is_empty() {
        report.errors.push(VerError::global("model has no name"));
    }
}

/// Every parameter's declared extent, checked against what an instrument can sample.
///
/// This is a refusal, not a representation, and the reason is measured rather than assumed. Handed
/// `input x in [0, inf]`, the campaign does not fail loudly: the sampler maps its unit coordinates
/// onto the interval and produces `x = inf`, the atlas bisects an infinite cell into children whose
/// bounds come out degenerate, and 32 of 80 evaluations then land outside every leaf. What is printed
/// is
///
/// ```text
/// atlas 1 cells (1 leaves)  trusted 0.0000  suspicious NaN  unknown 0.0000  resolved NaN
/// ```
///
/// — a coverage statement of `NaN`, because every volume fraction divides by the root cell's infinite
/// extent. The same run then reported a SUSPICIOUS finding whose printed risk was 0.000 with no
/// evidence attached to it, which is a claim the report cannot reproduce from its own numbers.
///
/// A model with an unbounded domain is a real thing a scientist means: "this quantity is not
/// bounded by physics, explore what you can". APORIA's answer to that is not to analyse it and print
/// arithmetic about nothing; it has no exploration window separate from the declared domain, so it
/// asks for the extent that will actually be sampled. Trust over a bounded explored region is a
/// claim; trust over the reals is not one this instrument has earned.
fn check_domains(model: &Model, report: &mut Report) {
    for p in &model.params {
        match &p.domain {
            Domain::Interval { lo, hi } => {
                if p.domain.is_unbounded() {
                    report.errors.push(VerError::global(format!(
                        "parameter `{}` has an unbounded domain [{lo}, {hi}]: volumes, coverage and \
                         sampling are undefined over an infinite extent. Declare the range you \
                         intend to explore.",
                        p.name
                    )));
                } else if lo > hi {
                    report.errors.push(VerError::global(format!(
                        "parameter `{}` has an inverted domain [{lo}, {hi}]",
                        p.name
                    )));
                }
            }
            Domain::Choices(values) => {
                if values.is_empty() {
                    report.errors.push(VerError::global(format!(
                        "parameter `{}` declares no choices, so there is nothing to sample",
                        p.name
                    )));
                }
                if let Some(v) = values.iter().find(|v| !v.is_finite()) {
                    report.errors.push(VerError::global(format!(
                        "parameter `{}` offers {v}, which is not a value a bounded axis can hold",
                        p.name
                    )));
                }
            }
        }
    }
}

/// Look up the representation and dimension of an operand against the declarations it points at.
///
/// Both lookups return `None` for a dangling reference; the structure pass already reported those,
/// and the rules here must not pile a second error on top of a missing declaration.
struct Ctx<'a> {
    model: &'a Model,
}

impl Ctx<'_> {
    fn num(&self, o: &Operand) -> Option<crate::NumType> {
        match o {
            Operand::Param(p) => self.model.params.get(*p as usize).map(|p| p.ty.num),
            Operand::Slot(s) => self.model.slots.get(*s as usize).map(|s| s.ty.num),
            Operand::Node(n) => self.model.instrs.get(*n as usize).map(|i| i.ty.num),
            Operand::Lit(l) => Some(l.ty()),
        }
    }

    fn dim(&self, o: &Operand) -> Option<crate::Dimension> {
        match o {
            Operand::Param(p) => self.model.params.get(*p as usize).map(|p| p.ty.dim),
            Operand::Slot(s) => self.model.slots.get(*s as usize).map(|s| s.ty.dim),
            Operand::Node(n) => self.model.instrs.get(*n as usize).map(|i| i.ty.dim),
            Operand::Lit(_) => Some(crate::Dimension::dimensionless()),
        }
    }

    /// Two operands whose dimensions are both known and different. This is the single most useful
    /// structural check in the whole verifier: it is how a unit mistake stops being an empirical
    /// question and becomes a compile failure.
    fn dims_differ(&self, a: &Operand, b: &Operand) -> bool {
        match (self.dim(a), self.dim(b)) {
            (Some(x), Some(y)) => !x.is_unknown() && !y.is_unknown() && x != y,
            _ => false,
        }
    }

    fn bin(&self, id: Id, instr: &Instr, op: Binop, a: &Operand, b: &Operand, report: &mut Report) {
        let (ta, tb) = (self.num(a), self.num(b));
        if ta != tb {
            report.warnings.push(VerError::at(
                id,
                format!("{} mixes {ta:?} and {tb:?}", op.name()),
            ));
        }
        if op.is_additive() && self.dims_differ(a, b) {
            let (da, db) = (self.dim(a).unwrap(), self.dim(b).unwrap());
            report.errors.push(VerError::at(
                id,
                format!("{} adds different dimensions: {da} and {db}", op.name()),
            ));
        }
        if op.is_predicate() {
            if instr.ty.num != crate::NumType::Bool {
                report
                    .errors
                    .push(VerError::at(id, format!("{} must produce Bool", op.name())));
            }
        } else if let Some(n) = self.num(a)
            && instr.ty.num != n
        {
            // Arithmetic keeps the representation of its input, so integer loop counters and
            // indices stay integers instead of silently widening.
            report.errors.push(VerError::at(
                id,
                format!("{} on {n:?} declares result {:?}", op.name(), instr.ty.num),
            ));
        }
    }

    fn un(&self, id: Id, instr: &Instr, op: Unop, a: &Operand, report: &mut Report) {
        if op.requires_dimensionless()
            && let Some(d) = self.dim(a)
            && !d.is_dimensionless()
            && !d.is_unknown()
        {
            report.errors.push(VerError::at(
                id,
                format!("{} needs a dimensionless argument, got {d}", op.name()),
            ));
        }
        // Casting is exactly the place where representation differs on purpose.
        if !matches!(op, Unop::Cast(_))
            && self.num(a).is_some_and(|n| !n.is_float())
            && instr.ty.num.is_float()
        {
            report.warnings.push(VerError::at(
                id,
                format!("{} applied to non-float input", op.name()),
            ));
        }
    }

    fn call(&self, id: Id, builtin: Builtin, args: &[Operand], report: &mut Report) {
        if builtin == Builtin::Hypot && args.len() == 2 && self.dims_differ(&args[0], &args[1]) {
            let (da, db) = (self.dim(&args[0]).unwrap(), self.dim(&args[1]).unwrap());
            report.errors.push(VerError::at(
                id,
                format!("hypot of {da} and {db} is meaningless"),
            ));
        }
    }

    fn loop_instr(
        &self,
        id: Id,
        instr: &Instr,
        trip: &Operand,
        body: BlockId,
        report: &mut Report,
    ) {
        if instr.ty.num != crate::NumType::Unit {
            report.errors.push(VerError::at(
                id,
                "a loop computes no value, so it must be typed unit",
            ));
        }
        if self.num(trip).is_some_and(|n| n != crate::NumType::I64) {
            report.errors.push(VerError::at(
                id,
                "loop trip count must be i64, an accidental float loop is never intended",
            ));
        }
        if body as usize >= self.model.blocks.len() {
            report
                .errors
                .push(VerError::at(id, format!("block {body} missing")));
        }
    }

    fn write(&self, id: Id, instr: &Instr, slot: u16, value: &Operand, report: &mut Report) {
        if instr.ty.num != crate::NumType::Unit {
            report.errors.push(VerError::at(
                id,
                "a slot write computes no value, so it must be typed unit",
            ));
        }
        let Some(s) = self.model.slots.get(slot as usize) else {
            report
                .errors
                .push(VerError::at(id, format!("slot s{slot} missing")));
            return;
        };
        if let Some(v) = self.num(value)
            && v != s.ty.num
        {
            report.errors.push(VerError::at(
                id,
                format!("writing {v:?} into slot {} of type {:?}", s.name, s.ty.num),
            ));
        }
        if !s.ty.dim.is_unknown()
            && let Some(vd) = self.dim(value)
            && !vd.is_unknown()
            && vd != s.ty.dim
        {
            report.errors.push(VerError::at(
                id,
                format!("slot {} is {}, written with {vd}", s.name, s.ty.dim),
            ));
        }
    }
}

/// Representation types and physical dimensions.
fn check_types(model: &Model, report: &mut Report) {
    let ctx = Ctx { model };
    for (id, instr) in model.instrs.iter().enumerate() {
        let id = id as Id;
        match &instr.kind {
            InstrKind::Bin { op, a, b } => ctx.bin(id, instr, *op, a, b, report),
            InstrKind::Un { op, a } => ctx.un(id, instr, *op, a, report),
            InstrKind::Call { builtin, args } => ctx.call(id, *builtin, args, report),
            InstrKind::For { trip, body } => ctx.loop_instr(id, instr, trip, *body, report),
            InstrKind::Write { slot, value } => ctx.write(id, instr, *slot, value, report),
            InstrKind::Opaque => {
                // The declared type is the whole story: there is no operand arithmetic to check it
                // against, and a rule over this value type-checks against the declaration just the
                // same as one over a computed quantity.
            }
        }
    }
}

fn check_declarations(model: &Model, scope: &Scope, report: &mut Report) {
    for o in &model.outputs {
        check_decl_operand(
            model,
            scope,
            &o.value,
            report,
            &format!("output {}", o.name),
        );
    }
    for t in &model.traces {
        if t.scope as usize >= model.blocks.len() {
            report.errors.push(VerError::global(format!(
                "trace {} names missing block {}",
                t.name, t.scope
            )));
        }
    }
    for c in &model.constraints {
        match &c.kind {
            ConstraintKind::Cmp { lhs, rhs, .. } => {
                check_decl_operand(model, scope, lhs, report, &c.name);
                check_decl_operand(model, scope, rhs, report, &c.name);
            }
            ConstraintKind::Finite { value } => {
                check_decl_operand(model, scope, value, report, &c.name);
            }
        }
    }
    for r in &model.relations {
        let bad_out = |id: u16, report: &mut Report, what: &str| {
            if id as usize >= model.outputs.len() {
                report.errors.push(VerError::global(format!(
                    "relation {what} refers to missing output o{id}"
                )));
            }
        };
        let bad_param = |id: u16, report: &mut Report, what: &str| {
            if id as usize >= model.params.len() {
                report.errors.push(VerError::global(format!(
                    "relation {what} refers to missing parameter p{id}"
                )));
            }
        };
        match &r.kind {
            crate::RelationKind::Monotone { out, param, .. }
            | crate::RelationKind::ScalesAs { out, param, .. }
            | crate::RelationKind::Lipschitz { out, param, .. } => {
                bad_out(*out, report, &r.name);
                bad_param(*param, report, &r.name);
            }
            crate::RelationKind::Symmetric { out, pair } => {
                bad_out(*out, report, &r.name);
                bad_param(pair[0], report, &r.name);
                bad_param(pair[1], report, &r.name);
            }
            crate::RelationKind::Conserved { trace, .. } => {
                if *trace as usize >= model.traces.len() {
                    report.errors.push(VerError::global(format!(
                        "relation {} refers to missing trace",
                        r.name
                    )));
                }
            }
        }
    }
}

fn check_decl_operand(
    model: &Model,
    scope: &Scope,
    operand: &Operand,
    report: &mut Report,
    what: &str,
) {
    match operand {
        Operand::Node(id) => {
            let visible = scope
                .def_block
                .get(*id as usize)
                .is_some_and(|&b| b != u32::MAX && is_ancestor(scope, b, 0));
            if !visible {
                report.errors.push(VerError::global(format!(
                    "constraint {what} refers to n{id}, which is not visible at entry"
                )));
            }
        }
        other => operand_visible(model, scope, 0, u32::MAX, other, report),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dim::{Dimension, LENGTH, TIME, Ty};
    use crate::ir::*;

    fn f64d() -> Ty {
        Ty::float(Dimension::dimensionless())
    }

    fn model_with(a: Id) -> Model {
        let mut m = Model::new("t");
        m.params.push(Param {
            name: "x".into(),
            ty: Ty::float(Dimension::base(LENGTH, 1)),
            domain: Domain::interval(0.0, 1.0),
            to_si: 1.0,
            doc: String::new(),
        });
        m.push_entry(Instr {
            ty: f64d(),
            kind: InstrKind::Un {
                op: Unop::Abs,
                a: Operand::Param(0),
            },
        });
        let _ = a;
        m
    }

    #[test]
    fn an_outside_computed_value_verifies_and_types_its_rules() {
        // A model adapted from a foreign program declares outputs and the rules they must satisfy. The
        // verifier is the gate every backend trusts, so the declaration has to pass it: the output
        // resolves, and a rule comparing it against a literal type-checks like any other.
        let mut m = Model::new("external");
        m.params.push(Param {
            name: "load".into(),
            ty: f64d(),
            domain: Domain::interval(0.0, 10.0),
            to_si: 1.0,
            doc: String::new(),
        });
        let value = m.push_entry(Instr {
            ty: f64d(),
            kind: InstrKind::Opaque,
        });
        m.outputs.push(crate::Output {
            name: "deflection".into(),
            ty: f64d(),
            value: Operand::Node(value),
            doc: String::new(),
        });
        m.constraints.push(crate::Constraint {
            id: 0,
            name: "sag".into(),
            kind: crate::ConstraintKind::Cmp {
                lhs: Operand::Node(value),
                cmp: crate::CmpOp::Ge,
                rhs: Operand::Lit(crate::Lit::F64(0.5)),
                tolerance: 0.0,
            },
            origin: crate::Origin::Declared,
        });
        let r = verify(&m);
        assert!(r.is_ok(), "{:?}", r.errors);
        // And a rule over a value that was never declared is still caught: the refusal is about
        // computation, not about skipping the checks.
        let mut broken = m.clone();
        broken.constraints.push(crate::Constraint {
            id: 1,
            name: "phantom".into(),
            kind: crate::ConstraintKind::Cmp {
                lhs: Operand::Node(99),
                cmp: crate::CmpOp::Ge,
                rhs: Operand::Lit(crate::Lit::F64(0.5)),
                tolerance: 0.0,
            },
            origin: crate::Origin::Declared,
        });
        assert!(!verify(&broken).is_ok());
    }

    #[test]
    fn a_domain_that_cannot_be_sampled_is_refused_before_anything_runs() {
        // Measured, not theorised: `input x in [0, inf]` used to be accepted, and the campaign then
        // produced `suspicious NaN  unknown 0.0000  resolved NaN` for its coverage line, lost 32 of 80
        // evaluations outside every leaf, and reported a SUSPICIOUS finding at printed risk 0.000 with
        // no evidence attached. Every volume fraction divides by the root cell's infinite extent, so
        // what was printed was arithmetic about nothing. Refusing at the declaration is the same
        // decision `aporia run` makes about a model it has no program for.
        let cases = [
            (Domain::interval(0.0, f64::INFINITY), "unbounded domain"),
            (Domain::interval(f64::NEG_INFINITY, 1.0), "unbounded domain"),
            (Domain::interval(5.0, 1.0), "inverted domain"),
            (Domain::Choices(vec![]), "declares no choices"),
            (Domain::Choices(vec![1.0, f64::INFINITY]), "not a value"),
        ];
        for (domain, needle) in cases {
            let mut m = model_with(0);
            m.params[0].domain = domain.clone();
            let report = verify(&m);
            let text: Vec<String> = report.errors.iter().map(|e| e.message.clone()).collect();
            assert!(
                !report.is_ok()
                    && text
                        .iter()
                        .any(|t| t.contains("parameter `x`") && t.contains(needle)),
                "{domain:?} should be refused for {needle:?}, got {text:?}"
            );
        }
        // The rule is about extents, not about parameters: a bounded interval and a real choice list
        // still pass, so this cannot become a way to reject models nobody meant to widen.
        for domain in [Domain::interval(0.0, 1.0), Domain::Choices(vec![1.0, 2.0])] {
            let mut m = model_with(0);
            m.params[0].domain = domain.clone();
            assert!(verify(&m).is_ok(), "{domain:?} should be accepted");
        }
    }

    #[test]
    fn empty_model_is_rejected_for_lack_of_name_only() {
        let mut m = Model::new("");
        m.params.push(Param {
            name: "x".into(),
            ty: f64d(),
            domain: Domain::interval(0.0, 1.0),
            to_si: 1.0,
            doc: String::new(),
        });
        let r = verify(&m);
        assert!(!r.is_ok());
        assert!(r.errors.iter().any(|e| e.message == "model has no name"));
    }

    #[test]
    fn forward_reference_is_an_error() {
        let mut m = model_with(0);
        m.push_entry(Instr {
            ty: f64d(),
            kind: InstrKind::Un {
                op: Unop::Neg,
                a: Operand::Node(5),
            },
        });
        let r = verify(&m);
        assert!(
            r.errors
                .iter()
                .any(|e| e.message.contains("n5 does not exist")),
            "{:?}",
            r.errors
        );
    }

    #[test]
    fn use_before_definition_inside_a_block_is_caught() {
        let mut m = model_with(0);
        // Deliberately place n1 before n0 in the same block by hand.
        let one = m.push_entry(Instr {
            ty: f64d(),
            kind: InstrKind::Bin {
                op: Binop::Add,
                a: Operand::Node(1),
                b: Operand::Node(0),
            },
        });
        m.blocks[0].instrs.swap(0, 1);
        let r = verify(&m);
        assert!(
            r.errors
                .iter()
                .any(|e| e.message.contains("used before it is defined")),
            "{one} {:?}",
            r.errors
        );
    }

    #[test]
    fn dimension_mismatch_in_addition_is_an_error() {
        let mut m = Model::new("dim");
        m.params.push(Param {
            name: "d".into(),
            ty: Ty::float(Dimension::base(LENGTH, 1)),
            domain: Domain::interval(0.0, 1.0),
            to_si: 1.0,
            doc: String::new(),
        });
        m.params.push(Param {
            name: "t".into(),
            ty: Ty::float(Dimension::base(TIME, 1)),
            domain: Domain::interval(0.0, 1.0),
            to_si: 1.0,
            doc: String::new(),
        });
        m.push_entry(Instr {
            ty: f64d(),
            kind: InstrKind::Bin {
                op: Binop::Add,
                a: Operand::Param(0),
                b: Operand::Param(1),
            },
        });
        let r = verify(&m);
        assert!(
            r.errors
                .iter()
                .any(|e| e.message.contains("adds different dimensions")),
            "{:?}",
            r.errors
        );
    }

    #[test]
    fn trig_on_a_dimensioned_argument_is_rejected() {
        let mut m = Model::new("trig");
        m.params.push(Param {
            name: "d".into(),
            ty: Ty::float(Dimension::base(LENGTH, 1)),
            domain: Domain::interval(0.0, 1.0),
            to_si: 1.0,
            doc: String::new(),
        });
        m.push_entry(Instr {
            ty: f64d(),
            kind: InstrKind::Un {
                op: Unop::Sin,
                a: Operand::Param(0),
            },
        });
        let r = verify(&m);
        assert!(r.errors.iter().any(|e| e.message.contains("dimensionless")));
    }

    #[test]
    fn body_cannot_escape_its_loop() {
        let mut m = Model::new("esc");
        m.slots.push(Slot {
            name: "acc".into(),
            ty: f64d(),
            init: Operand::Lit(Lit::F64(0.0)),
        });
        let body = m.new_block();
        // n0 lives inside the loop body; the entry instruction below tries to read it.
        let inside = m.push(
            body,
            Instr {
                ty: f64d(),
                kind: InstrKind::Un {
                    op: Unop::Abs,
                    a: Operand::Slot(0),
                },
            },
        );
        m.push_entry(Instr {
            ty: Ty::unit(),
            kind: InstrKind::For {
                trip: Operand::Lit(Lit::I64(3)),
                body,
            },
        });
        m.push_entry(Instr {
            ty: f64d(),
            kind: InstrKind::Bin {
                op: Binop::Add,
                a: Operand::Node(inside),
                b: Operand::Lit(Lit::F64(1.0)),
            },
        });
        let r = verify(&m);
        assert!(
            r.errors
                .iter()
                .any(|e| e.message.contains("not visible from block")),
            "{:?}",
            r.errors
        );
    }

    #[test]
    fn clean_model_verifies() {
        let m = model_with(0);
        let r = verify(&m);
        assert!(r.is_ok(), "{:?}", r.errors);
    }
}
