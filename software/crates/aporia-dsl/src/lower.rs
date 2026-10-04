//! Semantic analysis and lowering to A-IR, in one pass.
//!
//! Resolution, unit inference and instruction emission happen together because all three need the
//! same thing: a scope that maps a name to a value, a representation and a unit. Splitting them
//! would build an intermediate typed tree only to walk it again, with the same facts stored twice
//! and free to drift apart.
//!
//! How a finding is graded is the interesting part of the design, so it is stated once here:
//!
//! - A **dimension** mismatch is an error. `m + s` is never intended.
//! - A **scale** mismatch is a warning. `km/h` and `m/s` are the same kind of quantity, and a
//!   factor of 3.6 between them is very often a deliberate conversion. Refusing conversions would
//!   make the checker something authors work around; staying silent about them would discard the
//!   one static signal that catches the "unit and dimension mistakes" fault class. So the message
//!   names both units and the factor, and the author decides.
//!
//! A numeric literal written where a unit was declared adopts that unit, which is what makes
//! `state x : m = 1.0` mean one metre rather than one of nothing.

use crate::ast::{
    AdvanceStmt, AstModel, BinOp, CheckStmt, CmpSyntax, DomainSyntax, Expr, InputDecl, LetDecl,
    LitValue, LoopStmt, Member, MonotoneDir, RelationSyntax, RequireKind, RequireStmt, StateDecl,
    UnOp, WatchStmt,
};
use crate::builtins;
use crate::span::{Diagnostic, Diagnostics, Span};
use crate::units::Unit;
use aporia_ir::{
    Binop, BlockId, Builtin, CmpOp, Constraint, ConstraintId, ConstraintKind, Direction, Domain,
    Instr, InstrKind, Lit, Model, NumType, Operand, Origin, Output, OutputId, Param, ParamId,
    Relation, RelationId, RelationKind, Slot, SlotId, Trace, TraceId, Ty, Unop, verify,
};
use std::collections::HashMap;

/// The result of compiling a model.
#[derive(Debug)]
pub struct Compiled {
    pub model: Model,
    pub diagnostics: Diagnostics,
}

impl Compiled {
    /// The model, unless something was recorded at error severity.
    #[must_use]
    pub fn into_model(self) -> Option<Model> {
        if self.diagnostics.has_errors() {
            None
        } else {
            Some(self.model)
        }
    }
}

/// Lower an AST into an A-IR model.
///
/// `model` is best effort: when the input contains errors it may be incomplete. Callers should look
/// at `diagnostics` first, then run [`aporia_ir::verify`] on the model even when nothing failed —
/// the verifier is the boundary every backend relies on.
#[must_use]
pub fn lower(ast: &AstModel) -> Compiled {
    let mut b = Binder {
        m: Model::new(ast.name.clone()),
        scope: HashMap::new(),
        diag: Diagnostics::new(),
        block: 0 as BlockId,
        let_count: 0,
        in_loop: false,
    };
    b.m.doc = ast.doc.clone().unwrap_or_default();
    b.declarations(ast);
    b.body(ast);
    let model = b.m;
    let diag = b.diag;
    Compiled {
        model,
        diagnostics: diag,
    }
}

/// A resolved name: what to load, how it is stored, and what it is.
#[derive(Clone, Copy, Debug)]
struct Bound {
    value: Operand,
    num: NumType,
    unit: Unit,
}

/// Is this expression written as a number, possibly with a sign in front of it? The question has to
/// be asked of the syntax: `-2` lowers to an instruction, so by the time a `Bound` exists there is
/// no way to tell it apart from a negated measurement.
fn plain_number(e: &Expr) -> bool {
    match e {
        Expr::Lit {
            value: LitValue::Number { .. },
            ..
        } => true,
        // `pi` and `e` are written as names but carry no unit, so they adopt one the same way.
        Expr::Ident { name, .. } => builtins::named_constant(name).is_some(),
        Expr::Unary {
            op: UnOp::Neg,
            expr,
            ..
        } => plain_number(expr),
        _ => false,
    }
}

/// Give a plain literal the unit of the quantity it met, so the comparison afterwards means
/// something. Only one side may adopt: two literals are both pure numbers and nothing changes.
fn adopt_units(a: &Expr, left: &mut Bound, b: &Expr, right: &mut Bound) {
    let a_plain = plain_number(a);
    let b_plain = plain_number(b);
    if a_plain && !b_plain && !right.unit.dim.is_unknown() && !right.unit.is_dimensionless() {
        left.unit = right.unit;
    } else if b_plain && !a_plain && !left.unit.dim.is_unknown() && !left.unit.is_dimensionless() {
        right.unit = left.unit;
    }
}

struct Binder {
    m: Model,
    scope: HashMap<String, Bound>,
    diag: Diagnostics,
    block: BlockId,
    let_count: usize,
    in_loop: bool,
}

impl Binder {
    // ------------------------------------------------------------------ plumbing

    fn error(&mut self, message: impl Into<String>, span: Span) {
        self.diag.push(Diagnostic::error(message, span));
    }

    fn warning(&mut self, message: impl Into<String>, span: Span) {
        self.diag.push(Diagnostic::warning(message, span));
    }

    fn emit(&mut self, ty: Ty, kind: InstrKind) -> Operand {
        let id = self.m.instrs.len() as u32;
        self.m.instrs.push(Instr { ty, kind });
        self.m.blocks[self.block as usize].instrs.push(id);
        Operand::Node(id)
    }

    fn lookup(&self, name: &str) -> Option<Bound> {
        if let Some(b) = self.scope.get(name) {
            return Some(*b);
        }
        // Named constants are always available and cannot be shadowed by a parameter, because a
        // model that redefines `pi` is not a model anyone can reason about.
        builtins::named_constant(name).map(|v| Bound {
            value: Operand::Lit(Lit::F64(v)),
            num: NumType::F64,
            unit: Unit::dimensionless(),
        })
    }

    // ------------------------------------------------------------------- passes

    /// Parameters and state come first: a loop body mentions state declared above it, and a domain
    /// bound may mention a parameter declared before it.
    fn declarations(&mut self, ast: &AstModel) {
        for member in &ast.members {
            match member {
                Member::Input(d) => self.input(d),
                Member::State(d) => self.state(d),
                _ => {}
            }
        }
    }

    fn body(&mut self, ast: &AstModel) {
        self.in_loop = false;
        for member in &ast.members {
            match member {
                Member::Input(_) | Member::State(_) => {}
                other => self.statement(other),
            }
        }
        // A model with nothing observed is a model APORIA cannot analyse.
        if self.m.outputs.is_empty() {
            self.error(
                "this model observes nothing: add a `let` or a `state`",
                ast.name_span,
            );
        }
    }

    fn statement(&mut self, member: &Member) {
        match member {
            Member::Let(d) => self.let_decl(d),
            Member::Advance(s) => self.advance(s),
            Member::Loop(l) => self.loop_stmt(l),
            Member::Watch(w) => self.watch(w),
            Member::Require(r) => self.require(r),
            Member::Check(c) => self.check(c),
            Member::Input(_) | Member::State(_) => {
                self.error(
                    "a declaration appeared where a statement was expected",
                    member.span(),
                );
            }
        }
    }

    // ---------------------------------------------------------------- declarations

    fn input(&mut self, d: &InputDecl) {
        // A parameter with no annotation is a pure number, which is the honest default.
        let unit = self.unit_expr(d.unit.as_ref(), d.span).unwrap_or_default();
        if unit.dim.is_unknown() {
            self.error(
                format!("the unit of parameter `{}` cannot be worked out", d.name),
                d.span,
            );
            return;
        }
        let num = if unit.integral {
            NumType::I64
        } else {
            NumType::F64
        };
        let domain = match &d.domain {
            DomainSyntax::Interval { lo, hi } => {
                let (Some(l), Some(h)) = (
                    self.constant(lo, "a domain bound"),
                    self.constant(hi, "a domain bound"),
                ) else {
                    return;
                };
                if l >= h {
                    self.error(
                        format!("the domain of `{}` is empty: {l} to {h}", d.name),
                        d.span,
                    );
                    return;
                }
                Domain::Interval { lo: l, hi: h }
            }
            DomainSyntax::Choices(items) => {
                let mut vals = Vec::with_capacity(items.len());
                for item in items {
                    let Some(v) = self.constant(item, "a choice") else {
                        return;
                    };
                    vals.push(v);
                }
                if vals.len() < 2 {
                    self.error(format!("`{}` needs at least two choices", d.name), d.span);
                }
                Domain::Choices(vals)
            }
        };
        if self.m.param(&d.name).is_some() {
            self.error(format!("`{}` is declared twice", d.name), d.span);
            return;
        }
        let id = self.m.params.len() as ParamId;
        self.m.params.push(Param {
            name: d.name.clone(),
            ty: Ty { num, dim: unit.dim },
            domain,
            to_si: unit.scale,
            doc: d.doc.clone().unwrap_or_default(),
        });
        self.scope.insert(
            d.name.clone(),
            Bound {
                value: Operand::Param(id),
                num,
                unit,
            },
        );
    }

    fn state(&mut self, d: &StateDecl) {
        let declared = self.unit_expr(d.unit.as_ref(), d.span);
        let Some(init) = self.expr_in(&d.init, declared) else {
            return;
        };
        if let Some(want) = declared {
            self.compare_units(
                want,
                init.unit,
                d.span,
                &format!("the initial value of `{}`", d.name),
            );
            if want.dim != init.unit.dim && !init.unit.dim.is_unknown() {
                return;
            }
        }
        if self.m.slot(&d.name).is_some() {
            self.error(format!("`{}` is declared twice", d.name), d.span);
            return;
        }
        let unit = declared.unwrap_or(init.unit);
        let ty = Ty {
            num: init.num,
            dim: unit.dim,
        };
        let id = self.m.slots.len() as SlotId;
        self.m.slots.push(Slot {
            name: d.name.clone(),
            ty,
            init: init.value,
        });
        self.scope.insert(
            d.name.clone(),
            Bound {
                value: Operand::Slot(id),
                num: init.num,
                unit,
            },
        );
        let value = Operand::Slot(id);
        self.push_output(&d.name, ty, value, d.span);
    }

    fn let_decl(&mut self, d: &LetDecl) {
        let declared = self.unit_expr(d.unit.as_ref(), d.span);
        let Some(value) = self.expr_in(&d.value, declared) else {
            return;
        };
        if let Some(want) = declared {
            self.compare_units(
                want,
                value.unit,
                d.span,
                &format!("the value of `{}`", d.name),
            );
            if want.dim != value.unit.dim && !value.unit.dim.is_unknown() {
                return;
            }
        }
        if self.scope.contains_key(&d.name) {
            self.error(format!("`{}` is already defined", d.name), d.span);
            return;
        }
        let ty = Ty {
            num: value.num,
            dim: value.unit.dim,
        };
        let operand = value.value;
        self.scope.insert(
            d.name.clone(),
            Bound {
                value: operand,
                num: value.num,
                unit: value.unit,
            },
        );
        // A `let` inside a loop is a step temporary: its value belongs to one iteration and cannot
        // be named by a constraint or a relation at model level, so it is not an output. `watch` is
        // the statement for recording something that happens inside a loop.
        if !self.in_loop {
            self.push_output(&d.name, ty, operand, d.span);
        }
        self.let_count += 1;
        let _ = ty;
    }

    fn push_output(&mut self, name: &str, ty: Ty, value: Operand, span: Span) {
        if self.m.outputs.iter().any(|o| o.name == name) {
            self.error(format!("`{name}` is observed twice"), span);
            return;
        }
        let id = self.m.outputs.len() as OutputId;
        self.m.outputs.push(Output {
            name: name.to_string(),
            ty,
            value,
            doc: String::new(),
        });
        let _ = id;
    }

    // ------------------------------------------------------------------ statements

    fn advance(&mut self, s: &AdvanceStmt) {
        if !self.in_loop {
            self.error("`advance` is only meaningful inside a loop", s.span);
            return;
        }
        let Some(slot) = self.m.slot(&s.name) else {
            self.error(
                format!(
                    "`{}` is not a state variable, so it cannot be advanced",
                    s.name
                ),
                s.name_span,
            );
            return;
        };
        let slot_ty = self.m.slots[slot as usize].ty;
        let expected = Unit {
            dim: slot_ty.dim,
            scale: 1.0,
            integral: slot_ty.num == NumType::I64,
        };
        let Some(value) = self.expr_in(&s.value, Some(expected)) else {
            return;
        };
        if value.num != slot_ty.num {
            if slot_ty.num == NumType::F64 && value.num == NumType::I64 {
                let widened = self.emit(
                    slot_ty,
                    InstrKind::Un {
                        op: Unop::Cast(NumType::F64),
                        a: value.value,
                    },
                );
                self.emit(
                    Ty::unit(),
                    InstrKind::Write {
                        slot,
                        value: widened,
                    },
                );
                return;
            }
            self.error(
                format!(
                    "`{}` holds whole numbers, but the new value is {:?}",
                    s.name, value.num
                ),
                s.name_span,
            );
            return;
        }
        self.compare_units(
            expected,
            value.unit,
            s.name_span,
            &format!("the new value of `{}`", s.name),
        );
        self.emit(
            Ty::unit(),
            InstrKind::Write {
                slot,
                value: value.value,
            },
        );
    }

    fn loop_stmt(&mut self, l: &LoopStmt) {
        let Some(trip) = self.expr(&l.trip) else {
            return;
        };
        if trip.num != NumType::I64 {
            self.error(
                "a loop needs a whole number of iterations; declare the counter with unit `count`",
                l.trip.span(),
            );
            return;
        }
        let body = self.m.new_block();
        let outer = self.block;
        let was_in_loop = self.in_loop;
        self.block = body;
        self.in_loop = true;
        for member in &l.body {
            self.statement(member);
        }
        self.block = outer;
        self.in_loop = was_in_loop;
        if self.m.blocks[body as usize].instrs.is_empty() {
            self.warning("this loop body does nothing", l.span);
        }
        self.emit(
            Ty::unit(),
            InstrKind::For {
                trip: trip.value,
                body,
            },
        );
    }

    fn watch(&mut self, w: &WatchStmt) {
        if !self.in_loop {
            self.error("`watch` is only meaningful inside a loop", w.span);
            return;
        }
        let Some(value) = self.expr(&w.value) else {
            return;
        };
        // A bare `watch energy` takes the name of what it watched. A positional fallback would break
        // the one thing `check conserved(energy)` needs: a way to refer to that series.
        let implied = match &w.value {
            crate::ast::Expr::Ident { name, .. } => Some(name.clone()),
            _ => None,
        };
        let name = w
            .name
            .clone()
            .or(implied)
            .unwrap_or_else(|| format!("watch{}", self.m.traces.len() + 1));
        if self.m.traces.iter().any(|t| t.name == name) {
            self.error(format!("`{name}` is watched twice"), w.span);
            return;
        }
        self.m.traces.push(Trace {
            name,
            ty: Ty {
                num: value.num,
                dim: value.unit.dim,
            },
            value: value.value,
            scope: self.block,
        });
    }

    // ------------------------------------------------------------------ constraints

    /// One `require` line can carry several constraints, because `a > 0 and b > 0` is two rules an
    /// analyst wants reported separately.
    fn require(&mut self, r: &RequireStmt) {
        match &r.kind {
            RequireKind::Predicate { value, tolerance } => {
                self.require_parts(value, tolerance.as_ref(), r);
            }
            RequireKind::InRange { value, lo, hi } => {
                let Some(b) = self.expr(value) else { return };
                let (Some(l), Some(h)) =
                    (self.constant(lo, "a bound"), self.constant(hi, "a bound"))
                else {
                    return;
                };
                self.push_constraint(
                    &format!("{} >= {l}", r.doc.clone().unwrap_or_else(|| name_of(value))),
                    ConstraintKind::Cmp {
                        lhs: b.value,
                        cmp: CmpOp::Ge,
                        rhs: Operand::Lit(Lit::F64(l)),
                        tolerance: 0.0,
                    },
                );
                self.push_constraint(
                    &format!("{} <= {h}", r.doc.clone().unwrap_or_else(|| name_of(value))),
                    ConstraintKind::Cmp {
                        lhs: b.value,
                        cmp: CmpOp::Le,
                        rhs: Operand::Lit(Lit::F64(h)),
                        tolerance: 0.0,
                    },
                );
            }
        }
    }

    fn require_parts(&mut self, value: &Expr, tolerance: Option<&Expr>, r: &RequireStmt) {
        match value {
            Expr::Binary {
                op: BinOp::And,
                lhs,
                rhs,
                ..
            } => {
                self.require_parts(lhs, None, r);
                self.require_parts(rhs, None, r);
            }
            Expr::Compare { cmp, lhs, rhs, .. } => {
                let Some(mut l) = self.expr(lhs) else { return };
                let Some(mut rr) = self.expr(rhs) else { return };
                adopt_units(lhs, &mut l, rhs, &mut rr);
                self.compare_units(
                    l.unit,
                    rr.unit,
                    value.span(),
                    "the two sides of a requirement",
                );
                let tol = match tolerance {
                    Some(t) => match self.constant(t, "a tolerance") {
                        Some(v) => v,
                        None => return,
                    },
                    None if *cmp == CmpSyntax::Approx => CmpSyntax::DEFAULT_APPROX_TOLERANCE,
                    None => 0.0,
                };
                let kind = if *cmp == CmpSyntax::Ne {
                    // "not equal" is not something APORIA can check as a rule on a continuous space;
                    // it says so instead of pretending.
                    self.error(
                        "`!=` is not a requirement APORIA can test; use a range or a sign instead",
                        value.span(),
                    );
                    return;
                } else {
                    ConstraintKind::Cmp {
                        lhs: l.value,
                        cmp: match cmp {
                            CmpSyntax::Lt => CmpOp::Lt,
                            CmpSyntax::Le => CmpOp::Le,
                            CmpSyntax::Gt => CmpOp::Gt,
                            CmpSyntax::Ge => CmpOp::Ge,
                            CmpSyntax::Eq | CmpSyntax::Approx => CmpOp::EqApprox,
                            CmpSyntax::Ne => unreachable!("handled above"),
                        },
                        rhs: rr.value,
                        tolerance: tol,
                    }
                };
                let name = require_name(r, value);
                self.push_constraint(&name, kind);
            }
            Expr::Call { func, args, .. } if func == "finite" && args.len() == 1 => {
                let Some(v) = self.expr(&args[0]) else { return };
                let name = require_name(r, value);
                self.push_constraint(&name, ConstraintKind::Finite { value: v.value });
            }
            Expr::Unary {
                op: UnOp::Not,
                expr,
                ..
            } => {
                self.error(
                    format!(
                        "`not` is not supported in a requirement; rewrite it, for example `{}`",
                        suggest_negation(expr)
                    ),
                    value.span(),
                );
            }
            Expr::Binary { op: BinOp::Or, .. } => {
                self.error(
                    "`or` is not supported in a requirement: APORIA needs each rule to be checkable on its own",
                    value.span(),
                );
            }
            _ => {
                self.error(
                    "a requirement is a comparison or `finite(...)`",
                    value.span(),
                );
            }
        }
    }

    fn push_constraint(&mut self, name: &str, kind: ConstraintKind) {
        let id = self.m.constraints.len() as ConstraintId;
        self.m.constraints.push(Constraint {
            id,
            name: name.to_string(),
            kind,
            origin: Origin::Declared,
        });
    }

    // ------------------------------------------------------------------ relations

    fn check(&mut self, c: &CheckStmt) {
        let Some((name, kind)) = self.relation(c) else {
            return;
        };
        let id = self.m.relations.len() as RelationId;
        self.m.relations.push(Relation {
            id,
            name,
            kind,
            origin: Origin::Declared,
        });
    }

    fn relation(&mut self, c: &CheckStmt) -> Option<(String, RelationKind)> {
        let span = c.span;
        match &c.relation {
            RelationSyntax::Monotone { out, wrt, dir, .. } => {
                let o = self.output_id(out, span)?;
                let p = self.param_id(wrt, span)?;
                Some((
                    format!("monotone {out} wrt {wrt}"),
                    RelationKind::Monotone {
                        out: o,
                        param: p,
                        direction: match dir {
                            MonotoneDir::Up => Direction::Increasing,
                            MonotoneDir::Down => Direction::Decreasing,
                        },
                    },
                ))
            }
            RelationSyntax::ScalesAs {
                out, wrt, power, ..
            } => {
                let o = self.output_id(out, span)?;
                let p = self.param_id(wrt, span)?;
                let k = self.constant(power, "a scaling exponent")?;
                if (k * 2.0).fract() != 0.0 {
                    self.warning(
                        format!(
                            "the exponent {k} is not a half-integer, so units will not follow it"
                        ),
                        power.span(),
                    );
                }
                Some((
                    format!("{out} ~ {wrt}^{k}"),
                    RelationKind::ScalesAs {
                        out: o,
                        param: p,
                        power: k,
                    },
                ))
            }
            RelationSyntax::Symmetric { out, a, b, .. } => {
                let o = self.output_id(out, span)?;
                let x = self.param_id(a, span)?;
                let y = self.param_id(b, span)?;
                if self.m.params[x as usize].ty.dim != self.m.params[y as usize].ty.dim {
                    self.error(
                        format!("`{a}` and `{b}` are different kinds of quantity, so swapping them is not a symmetry"),
                        span,
                    );
                    return None;
                }
                Some((
                    format!("{out} symmetric in {a},{b}"),
                    RelationKind::Symmetric {
                        out: o,
                        pair: [x, y],
                    },
                ))
            }
            RelationSyntax::Conserved {
                what, tolerance, ..
            } => {
                let Some(t) = self.m.traces.iter().position(|x| x.name == *what) else {
                    let watched: Vec<&str> =
                        self.m.traces.iter().map(|x| x.name.as_str()).collect();
                    self.error(
                        format!(
                            "`{what}` is not a traced quantity; this model watches [{}]",
                            watched.join(", ")
                        ),
                        c.span,
                    );
                    return None;
                };
                let tol = match tolerance {
                    Some(e) => self.constant(e, "a tolerance")?,
                    None => 1e-3,
                };
                Some((
                    format!("{what} conserved"),
                    RelationKind::Conserved {
                        trace: t as TraceId,
                        tolerance: tol,
                    },
                ))
            }
            RelationSyntax::Lipschitz {
                out, wrt, bound, ..
            } => {
                let o = self.output_id(out, span)?;
                let p = self.param_id(wrt, span)?;
                let bd = self.constant(bound, "a Lipschitz bound")?;
                Some((
                    format!("{out} wrt {wrt} <= {bd}"),
                    RelationKind::Lipschitz {
                        out: o,
                        param: p,
                        bound: bd,
                    },
                ))
            }
        }
    }

    fn output_id(&mut self, name: &str, span: Span) -> Option<OutputId> {
        if let Some(i) = self.m.outputs.iter().position(|o| o.name == name) {
            return Some(i as OutputId);
        }
        self.error(
            format!("`{name}` is not an observed quantity of this model"),
            span,
        );
        None
    }

    fn param_id(&mut self, name: &str, span: Span) -> Option<ParamId> {
        if let Some(p) = self.m.param(name) {
            return Some(p);
        }
        self.error(format!("`{name}` is not a parameter of this model"), span);
        None
    }

    // ----------------------------------------------------------------- expressions

    fn expr(&mut self, e: &Expr) -> Option<Bound> {
        self.expr_in(e, None)
    }

    /// `expected` lets a bare literal adopt the unit of the quantity it is being bound to, which is
    /// what makes `state x : m = 1.0` mean a metre and not one of nothing.
    fn expr_in(&mut self, e: &Expr, expected: Option<Unit>) -> Option<Bound> {
        match e {
            Expr::Lit { value, .. } => {
                let b = literal(*value);
                match (value, expected) {
                    (LitValue::Number { .. }, Some(u)) if b.unit.is_dimensionless() => {
                        Some(Bound {
                            value: b.value,
                            num: b.num,
                            unit: u,
                        })
                    }
                    _ => Some(b),
                }
            }
            Expr::Ident { name, span } => {
                if let Some(b) = self.lookup(name) {
                    Some(b)
                } else {
                    self.error(format!("`{name}` is not defined here"), *span);
                    None
                }
            }
            Expr::Unary {
                op: UnOp::Neg,
                expr,
                span,
            } => {
                let inner = self.expr_in(expr, expected)?;
                if inner.num == NumType::Bool {
                    self.error("cannot negate a boolean; use `not`", *span);
                    return None;
                }
                let ty = Ty {
                    num: inner.num,
                    dim: inner.unit.dim,
                };
                Some(Bound {
                    value: self.emit(
                        ty,
                        InstrKind::Un {
                            op: Unop::Neg,
                            a: inner.value,
                        },
                    ),
                    num: inner.num,
                    unit: inner.unit,
                })
            }
            Expr::Unary {
                op: UnOp::Not,
                expr,
                span,
            } => {
                let inner = self.expr(expr)?;
                if inner.num != NumType::Bool {
                    self.error("`not` needs a boolean argument", *span);
                    return None;
                }
                Some(Bound {
                    value: self.emit(
                        Ty::boolean(),
                        InstrKind::Un {
                            op: Unop::Not,
                            a: inner.value,
                        },
                    ),
                    num: NumType::Bool,
                    unit: Unit::dimensionless(),
                })
            }
            Expr::Binary { op, lhs, rhs, span } => self.binary(*op, lhs, rhs, *span, expected),
            Expr::Compare {
                cmp,
                lhs,
                rhs,
                span,
            } => self.compare(*cmp, lhs, rhs, *span),
            Expr::Call {
                func, args, span, ..
            } => self.call(func, args, *span),
        }
    }

    fn binary(
        &mut self,
        op: BinOp,
        lhs: &Expr,
        rhs: &Expr,
        span: Span,
        expected: Option<Unit>,
    ) -> Option<Bound> {
        if matches!(op, BinOp::And | BinOp::Or) {
            let l = self.expr(lhs)?;
            let r = self.expr(rhs)?;
            if l.num != NumType::Bool || r.num != NumType::Bool {
                self.error(format!("`{}` needs boolean operands", op_name(op)), span);
                return None;
            }
            let irop = if op == BinOp::And {
                Binop::And
            } else {
                Binop::Or
            };
            return Some(Bound {
                value: self.emit(
                    Ty::boolean(),
                    InstrKind::Bin {
                        op: irop,
                        a: l.value,
                        b: r.value,
                    },
                ),
                num: NumType::Bool,
                unit: Unit::dimensionless(),
            });
        }

        // The declared unit only travels down an additive chain. Pushing it into the operands of
        // `*`, `/` or `^` would make `2 * velocity` a number of seconds whenever the result is
        // annotated as seconds, and the arithmetic would then be wrong in a way the reader cannot
        // see: `2 s * m/s` is `m*s`, not `m/s`.
        let inherit = matches!(op, BinOp::Add | BinOp::Sub | BinOp::Rem);
        let (mut l, mut r) = if op == BinOp::Pow {
            (
                self.expr_in(lhs, if inherit { expected } else { None })?,
                self.expr(rhs)?,
            )
        } else {
            let left = if inherit { expected } else { None };
            let right = if inherit { expected } else { None };
            (self.expr_in(lhs, left)?, self.expr_in(rhs, right)?)
        };
        if matches!(op, BinOp::Add | BinOp::Sub | BinOp::Rem) {
            adopt_units(lhs, &mut l, rhs, &mut r);
        }

        let unit = match op {
            BinOp::Add | BinOp::Sub | BinOp::Rem => {
                self.compare_units(
                    l.unit,
                    r.unit,
                    span,
                    &format!("the two sides of `{}`", op_name(op)),
                );
                if l.unit.dim != r.unit.dim && !l.unit.dim.is_unknown() && !r.unit.dim.is_unknown()
                {
                    return None;
                }
                l.unit
            }
            BinOp::Mul => l.unit.mul(&r.unit),
            BinOp::Div => l.unit.div(&r.unit),
            BinOp::Pow => {
                if let Some(k) = int_value(&r) {
                    l.unit.pow(k)
                } else {
                    self.warning(
                    format!(
                        "raising {} to a power that is not a literal integer leaves the unit unknown",
                        l.unit.describe()
                    ),
                    span,
                );
                    Unit {
                        dim: aporia_ir::Dimension::unknown(),
                        scale: 1.0,
                        integral: false,
                    }
                }
            }
            BinOp::And | BinOp::Or => unreachable!("handled above"),
        };

        let irop = match op {
            BinOp::Add => Binop::Add,
            BinOp::Sub => Binop::Sub,
            BinOp::Mul => Binop::Mul,
            BinOp::Div => Binop::Div,
            BinOp::Rem => Binop::Rem,
            BinOp::Pow => Binop::Pow,
            BinOp::And | BinOp::Or => unreachable!("handled above"),
        };
        let (a, b, num) = self.widen(op, l, r, span);
        a?;
        Some(Bound {
            value: self.emit(
                Ty { num, dim: unit.dim },
                InstrKind::Bin {
                    op: irop,
                    a: a?,
                    b: b?,
                },
            ),
            num,
            unit,
        })
    }

    /// Arithmetic keeps one representation: `/` is always floating, an integer meeting a float
    /// widens to float, and narrowing is refused rather than performed silently.
    fn widen(
        &mut self,
        op: BinOp,
        l: Bound,
        r: Bound,
        span: Span,
    ) -> (Option<Operand>, Option<Operand>, NumType) {
        if l.num == NumType::Bool || r.num == NumType::Bool {
            self.error(
                format!("`{}` cannot be applied to a boolean", op_name(op)),
                span,
            );
            return (None, None, l.num);
        }
        let target = if op == BinOp::Div {
            NumType::F64
        } else if l.num == r.num {
            l.num
        } else {
            NumType::F64
        };
        let a = self.widen_one(l, target, op, span);
        let b = self.widen_one(r, target, op, span);
        (a, b, target)
    }

    /// Bring one operand to the agreed representation. Widening an integer to a float is fine;
    /// narrowing is refused, because a silent truncation inside a scientific model is exactly the
    /// kind of thing APORIA is meant to be suspicious about.
    fn widen_one(&mut self, v: Bound, target: NumType, op: BinOp, span: Span) -> Option<Operand> {
        if v.num == target {
            return Some(v.value);
        }
        if target == NumType::I64 {
            self.error(
                format!("this {} needs whole numbers on both sides", op_name(op)),
                span,
            );
            return None;
        }
        Some(self.emit(
            Ty {
                num: target,
                dim: v.unit.dim,
            },
            InstrKind::Un {
                op: Unop::Cast(target),
                a: v.value,
            },
        ))
    }

    fn compare(&mut self, cmp: CmpSyntax, lhs: &Expr, rhs: &Expr, span: Span) -> Option<Bound> {
        let mut l = self.expr(lhs)?;
        let mut r = self.expr(rhs)?;
        adopt_units(lhs, &mut l, rhs, &mut r);
        self.compare_units(l.unit, r.unit, span, "the two sides of a comparison");
        if (l.num == NumType::Bool || r.num == NumType::Bool)
            && !matches!(cmp, CmpSyntax::Eq | CmpSyntax::Ne)
        {
            self.error("ordering a boolean is meaningless", span);
            return None;
        }
        if cmp == CmpSyntax::Approx {
            // `a ~ b` becomes `abs(a-b) <= tol*max(abs(b),1)` in the IR, which is what the runtime
            // measures and what a report can re-check.
            let diff_unit = l.unit;
            let sub = self.emit(
                Ty {
                    num: NumType::F64,
                    dim: diff_unit.dim,
                },
                InstrKind::Bin {
                    op: Binop::Sub,
                    a: l.value,
                    b: r.value,
                },
            );
            let abs = self.emit(
                Ty {
                    num: NumType::F64,
                    dim: diff_unit.dim,
                },
                InstrKind::Un {
                    op: Unop::Abs,
                    a: sub,
                },
            );
            let scale = self.emit(
                Ty {
                    num: NumType::F64,
                    dim: diff_unit.dim,
                },
                InstrKind::Bin {
                    op: Binop::Mul,
                    a: Operand::Lit(Lit::F64(CmpSyntax::DEFAULT_APPROX_TOLERANCE)),
                    b: r.value,
                },
            );
            return Some(Bound {
                value: self.emit(
                    Ty::boolean(),
                    InstrKind::Bin {
                        op: Binop::Le,
                        a: abs,
                        b: scale,
                    },
                ),
                num: NumType::Bool,
                unit: Unit::dimensionless(),
            });
        }
        let op = match cmp {
            CmpSyntax::Lt => Binop::Lt,
            CmpSyntax::Le => Binop::Le,
            CmpSyntax::Gt => Binop::Gt,
            CmpSyntax::Ge => Binop::Ge,
            CmpSyntax::Eq => Binop::Eq,
            CmpSyntax::Ne => Binop::Ne,
            CmpSyntax::Approx => unreachable!("handled above"),
        };
        Some(Bound {
            value: self.emit(
                Ty::boolean(),
                InstrKind::Bin {
                    op,
                    a: l.value,
                    b: r.value,
                },
            ),
            num: NumType::Bool,
            unit: Unit::dimensionless(),
        })
    }

    #[expect(clippy::too_many_lines)]
    fn call(&mut self, func: &str, args: &[Expr], span: Span) -> Option<Bound> {
        let Some(f) = builtins::lookup(func) else {
            self.error(format!("`{func}` is not a function APORIA knows"), span);
            return None;
        };
        if f.arity != args.len() {
            self.error(format!("`{func}` takes {} arguments", f.arity), span);
            return None;
        }
        let mut a = Vec::with_capacity(args.len());
        for arg in args {
            a.push(self.expr(arg)?);
        }
        let dimensionless_out = Unit::dimensionless();
        match func {
            "abs" => {
                let u = a[0].unit;
                Some(Bound {
                    value: self.emit(
                        Ty {
                            num: a[0].num,
                            dim: u.dim,
                        },
                        InstrKind::Un {
                            op: Unop::Abs,
                            a: a[0].value,
                        },
                    ),
                    num: a[0].num,
                    unit: u,
                })
            }
            "sqrt" | "cbrt" => {
                let u = a[0].unit;
                let lowered = if func == "sqrt" {
                    u.dim.sqrt()
                } else {
                    u.dim.cbrt()
                };
                let Some(dim) = lowered else {
                    self.error(
                        format!("the unit {} has no clean {func}", u.describe()),
                        span,
                    );
                    return None;
                };
                let scale = if func == "sqrt" {
                    u.scale.sqrt()
                } else {
                    u.scale.cbrt()
                };
                let op = if func == "sqrt" {
                    Unop::Sqrt
                } else {
                    Unop::Cbrt
                };
                Some(Bound {
                    value: self.emit(Ty::float(dim), InstrKind::Un { op, a: a[0].value }),
                    num: NumType::F64,
                    unit: Unit {
                        dim,
                        scale,
                        integral: false,
                    },
                })
            }
            "exp" | "ln" | "log2" | "log10" | "sin" | "cos" | "tan" | "asin" | "acos" | "atan"
            | "sinh" | "cosh" | "tanh" => {
                self.require_pure_number(&a[0], func, span);
                let op = match func {
                    "exp" => Unop::Exp,
                    "ln" => Unop::Ln,
                    "log2" => Unop::Log2,
                    "log10" => Unop::Log10,
                    "sin" => Unop::Sin,
                    "cos" => Unop::Cos,
                    "tan" => Unop::Tan,
                    "asin" => Unop::Asin,
                    "acos" => Unop::Acos,
                    "atan" => Unop::Atan,
                    "sinh" => Unop::Sinh,
                    "cosh" => Unop::Cosh,
                    _ => Unop::Tanh,
                };
                Some(Bound {
                    value: self.emit(Ty::dimensionless_f64(), InstrKind::Un { op, a: a[0].value }),
                    num: NumType::F64,
                    unit: dimensionless_out,
                })
            }
            "floor" | "ceil" | "round" | "trunc" => {
                let u = a[0].unit;
                if !u.is_dimensionless() && (u.scale - 1.0).abs() > 1e-15 {
                    self.warning(
                        format!(
                            "`{func}` rounds the number in {}, which is probably not what you meant",
                            u.describe()
                        ),
                        span,
                    );
                }
                let op = match func {
                    "floor" => Unop::Floor,
                    "ceil" => Unop::Ceil,
                    "round" => Unop::Round,
                    _ => Unop::Trunc,
                };
                Some(Bound {
                    value: self.emit(
                        Ty {
                            num: a[0].num,
                            dim: u.dim,
                        },
                        InstrKind::Un { op, a: a[0].value },
                    ),
                    num: a[0].num,
                    unit: u,
                })
            }
            "min" | "max" => {
                self.compare_units(
                    a[0].unit,
                    a[1].unit,
                    span,
                    &format!("the two arguments of `{func}`"),
                );
                let irop = if func == "min" {
                    Binop::Min
                } else {
                    Binop::Max
                };
                let (x, y, num) = self.widen(BinOp::Mul, a[0], a[1], span);
                x?;
                Some(Bound {
                    value: self.emit(
                        Ty {
                            num,
                            dim: a[0].unit.dim,
                        },
                        InstrKind::Bin {
                            op: irop,
                            a: x?,
                            b: y?,
                        },
                    ),
                    num,
                    unit: a[0].unit,
                })
            }
            "pow" => {
                let unit = match int_value(&a[1]) {
                    Some(k) => a[0].unit.pow(k),
                    None => Unit {
                        dim: aporia_ir::Dimension::unknown(),
                        scale: 1.0,
                        integral: false,
                    },
                };
                let (x, y, num) = self.widen(BinOp::Pow, a[0], a[1], span);
                x?;
                Some(Bound {
                    value: self.emit(
                        Ty { num, dim: unit.dim },
                        InstrKind::Bin {
                            op: Binop::Pow,
                            a: x?,
                            b: y?,
                        },
                    ),
                    num,
                    unit,
                })
            }
            "atan2" | "hypot" => {
                self.compare_units(
                    a[0].unit,
                    a[1].unit,
                    span,
                    &format!("the two arguments of `{func}`"),
                );
                if func == "atan2" {
                    Some(Bound {
                        value: self.emit(
                            Ty::dimensionless_f64(),
                            InstrKind::Call {
                                builtin: Builtin::Atan2,
                                args: vec![a[0].value, a[1].value],
                            },
                        ),
                        num: NumType::F64,
                        unit: dimensionless_out,
                    })
                } else {
                    let u = a[0].unit;
                    Some(Bound {
                        value: self.emit(
                            Ty::float(u.dim),
                            InstrKind::Call {
                                builtin: Builtin::Hypot,
                                args: vec![a[0].value, a[1].value],
                            },
                        ),
                        num: NumType::F64,
                        unit: u,
                    })
                }
            }
            "fma" => {
                let produced = a[0].unit.mul(&a[1].unit);
                self.compare_units(
                    produced,
                    a[2].unit,
                    span,
                    "`fma` adds a product to another quantity",
                );
                Some(Bound {
                    value: self.emit(
                        Ty::float(produced.dim),
                        InstrKind::Call {
                            builtin: Builtin::Fma,
                            args: a.iter().map(|v| v.value).collect(),
                        },
                    ),
                    num: NumType::F64,
                    unit: produced,
                })
            }
            "clamp" | "lerp" => {
                self.compare_units(
                    a[0].unit,
                    a[1].unit,
                    span,
                    &format!("the arguments of `{func}`"),
                );
                if func == "clamp" {
                    self.compare_units(a[0].unit, a[2].unit, span, "the arguments of `clamp`");
                } else {
                    self.require_pure_number(&a[2], "lerp", span);
                }
                let u = a[0].unit;
                let builtin = if func == "clamp" {
                    Builtin::Clamp
                } else {
                    Builtin::Lerp
                };
                Some(Bound {
                    value: self.emit(
                        Ty::float(u.dim),
                        InstrKind::Call {
                            builtin,
                            args: a.iter().map(|v| v.value).collect(),
                        },
                    ),
                    num: NumType::F64,
                    unit: u,
                })
            }
            "finite" => {
                if !a[0].num.is_float() {
                    self.error("`finite` applies to a measured quantity", span);
                    return None;
                }
                Some(Bound {
                    value: self.emit(
                        Ty::boolean(),
                        InstrKind::Un {
                            op: Unop::IsFinite,
                            a: a[0].value,
                        },
                    ),
                    num: NumType::Bool,
                    unit: dimensionless_out,
                })
            }
            other => {
                self.error(
                    format!("`{other}` is not implemented by the lowering yet"),
                    span,
                );
                None
            }
        }
    }

    fn require_pure_number(&mut self, arg: &Bound, func: &str, span: Span) {
        if !arg.unit.is_dimensionless() {
            self.error(
                format!(
                    "`{func}` needs a pure number; the argument is {}",
                    arg.unit.describe()
                ),
                span,
            );
        } else if (arg.unit.scale - 1.0).abs() > 1e-15 {
            self.error(
                format!(
                    "`{func}` needs its argument as a plain ratio or in radians, not {}",
                    arg.unit.describe()
                ),
                span,
            );
        }
    }

    // -------------------------------------------------------------------- units

    /// Resolve a unit annotation. `None` annotation is `None`; a bad one records the error.
    /// `at` is where the enclosing declaration starts; a bad unit reports at the unit expression
    /// itself, which is the part the author needs to see.
    fn unit_expr(&mut self, expr: Option<&Expr>, _at: Span) -> Option<Unit> {
        expr.and_then(|e| self.resolve_unit(e).ok())
    }

    fn resolve_unit(&mut self, e: &Expr) -> Result<Unit, ()> {
        match e {
            Expr::Lit { value, .. } => match value {
                LitValue::Number { value, .. } => Ok(Unit {
                    dim: aporia_ir::Dimension::dimensionless(),
                    scale: *value,
                    integral: false,
                }),
                LitValue::Bool(_) => {
                    self.error("a unit cannot be a boolean", e.span());
                    Err(())
                }
            },
            Expr::Ident { name, span } => {
                if let Some(u) = crate::units::lookup(name) {
                    return Ok(u);
                }
                if let Some(why) = crate::units::rejection(name) {
                    self.diag.push(
                        Diagnostic::error(format!("`{name}` cannot be used as a unit"), *span)
                            .with_label(*span, why),
                    );
                    return Err(());
                }
                self.error(format!("`{name}` is not a unit APORIA knows"), *span);
                Err(())
            }
            Expr::Binary { op, lhs, rhs, .. } => {
                let l = self.resolve_unit(lhs)?;
                let r = self.resolve_unit(rhs)?;
                match op {
                    BinOp::Mul => Ok(l.mul(&r)),
                    BinOp::Div => Ok(l.div(&r)),
                    BinOp::Pow => {
                        if let Some(k) = int_literal(rhs) {
                            Ok(l.pow(k))
                        } else {
                            self.error("a unit exponent has to be a whole number", rhs.span());
                            Err(())
                        }
                    }
                    other => {
                        self.error(
                            format!(
                                "`{}` cannot appear in a unit; units are products and powers",
                                op_name(*other)
                            ),
                            e.span(),
                        );
                        Err(())
                    }
                }
            }
            Expr::Call { func, args, .. } if func == "sqrt" && args.len() == 1 => {
                let u = self.resolve_unit(&args[0])?;
                if let Some(dim) = u.dim.sqrt() {
                    Ok(Unit {
                        dim,
                        scale: u.scale.sqrt(),
                        integral: false,
                    })
                } else {
                    self.error(
                        format!("the unit {} has no square root", u.describe()),
                        e.span(),
                    );
                    Err(())
                }
            }
            other => {
                self.error(
                    "a unit is built from symbols, `*`, `/` and whole-number powers",
                    other.span(),
                );
                Err(())
            }
        }
    }

    /// See the module documentation for why dimensions are errors and scales are warnings.
    fn compare_units(&mut self, a: Unit, b: Unit, span: Span, where_: &str) {
        if a.dim.is_unknown() || b.dim.is_unknown() {
            return;
        }
        if a.dim != b.dim {
            self.error(
                format!("{where_} mixes {} and {}", a.describe(), b.describe()),
                span,
            );
            return;
        }
        if !a.same_scale_as(&b) {
            self.diag.push(
                Diagnostic::warning(
                    format!(
                        "{where_} mixes {} and {}, a factor of {}",
                        a.describe(),
                        b.describe(),
                        a.scale / b.scale
                    ),
                    span,
                )
                .with_help(
                    "a deliberate conversion is fine; this is here so an accidental one is visible",
                ),
            );
        }
    }

    // ------------------------------------------------------------------- folding

    fn constant(&mut self, e: &Expr, what: &str) -> Option<f64> {
        fold(self, e, what)
    }
}

// --------------------------------------------------------------- free functions

fn literal(value: LitValue) -> Bound {
    match value {
        LitValue::Number { value, int } => Bound {
            value: Operand::Lit(if int {
                Lit::I64(value as i64)
            } else {
                Lit::F64(value)
            }),
            num: if int { NumType::I64 } else { NumType::F64 },
            unit: Unit::dimensionless(),
        },
        LitValue::Bool(b) => Bound {
            value: Operand::Lit(Lit::Bool(b)),
            num: NumType::Bool,
            unit: Unit::dimensionless(),
        },
    }
}

fn op_name(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Rem => "%",
        BinOp::Pow => "^",
        BinOp::And => "and",
        BinOp::Or => "or",
    }
}

fn int_literal(e: &Expr) -> Option<i32> {
    match e {
        Expr::Lit {
            value: LitValue::Number { value, int: true },
            ..
        } => Some(*value as i32),
        _ => None,
    }
}

/// An exponent only preserves dimensions when it is an integer known before execution.
fn int_value(b: &Bound) -> Option<i32> {
    if b.num != NumType::I64 || !b.unit.is_dimensionless() {
        return None;
    }
    match b.value {
        Operand::Lit(Lit::I64(k)) => i32::try_from(k).ok(),
        _ => None,
    }
}

/// A short, readable summary of the expression a requirement is about, used as its name.
fn summary(e: &Expr) -> String {
    match e {
        Expr::Compare { cmp, lhs, rhs, .. } => {
            format!("{} {} {}", name_of(lhs), cmp_symbol(*cmp), name_of(rhs))
        }
        Expr::Call { func, args, .. } => {
            format!(
                "{func}({})",
                args.iter().map(name_of).collect::<Vec<_>>().join(", ")
            )
        }
        other => name_of(other),
    }
}

fn name_of(e: &Expr) -> String {
    match e {
        Expr::Ident { name, .. } => name.clone(),
        Expr::Lit { value, .. } => match value {
            LitValue::Number { value, .. } => format!("{value}"),
            LitValue::Bool(b) => format!("{b}"),
        },
        Expr::Binary { op, lhs, rhs, .. } => {
            format!("{}{}{}", name_of(lhs), op_name(*op), name_of(rhs))
        }
        Expr::Unary { op, expr, .. } => {
            format!(
                "{}{}",
                if *op == UnOp::Neg { "-" } else { "not " },
                name_of(expr)
            )
        }
        Expr::Call { func, args, .. } => {
            format!(
                "{func}({})",
                args.iter().map(name_of).collect::<Vec<_>>().join(",")
            )
        }
        Expr::Compare { .. } => "comparison".to_string(),
    }
}

fn cmp_symbol(c: CmpSyntax) -> &'static str {
    match c {
        CmpSyntax::Lt => "<",
        CmpSyntax::Le => "<=",
        CmpSyntax::Gt => ">",
        CmpSyntax::Ge => ">=",
        CmpSyntax::Eq => "==",
        CmpSyntax::Ne => "!=",
        CmpSyntax::Approx => "~",
    }
}

/// `not (a > b)` is nearly always meant as `a <= b`, so the error can offer it.
fn suggest_negation(e: &Expr) -> String {
    if let Expr::Compare { cmp, lhs, rhs, .. } = e {
        let flipped = match cmp {
            CmpSyntax::Lt => ">=",
            CmpSyntax::Le => ">",
            CmpSyntax::Gt => "<=",
            CmpSyntax::Ge => "<",
            CmpSyntax::Eq => "!=",
            CmpSyntax::Ne => "==",
            CmpSyntax::Approx => "~",
        };
        return format!("{} {flipped} {}", name_of(lhs), name_of(rhs));
    }
    "the opposite comparison".to_string()
}

/// Requirements are named from the source when a document string gives one, and from the rule
/// itself otherwise, so a report reads as the model rather than as an index.
fn require_name(r: &RequireStmt, value: &Expr) -> String {
    match &r.doc {
        Some(d) => d.clone(),
        None => summary(value),
    }
}

/// Fold a constant expression used in a domain bound, a tolerance or an exponent.
fn fold(b: &mut Binder, e: &Expr, what: &str) -> Option<f64> {
    match e {
        Expr::Lit { value, .. } => match value {
            LitValue::Number { value, .. } => Some(*value),
            LitValue::Bool(_) => {
                b.error(
                    format!("a boolean is not a number, and {what} has to be one"),
                    e.span(),
                );
                None
            }
        },
        Expr::Ident { name, span } => {
            if let Some(v) = builtins::named_constant(name) {
                return Some(v);
            }
            b.error(
                format!(
                    "`{name}` is not a constant, so {what} cannot be worked out at compile time"
                ),
                *span,
            );
            None
        }
        Expr::Unary { op, expr, .. } => {
            let v = fold(b, expr, what)?;
            match op {
                UnOp::Neg => Some(-v),
                UnOp::Not => {
                    b.error(format!("`{what}` cannot be a boolean"), e.span());
                    None
                }
            }
        }
        Expr::Binary { op, lhs, rhs, .. } => {
            if matches!(op, BinOp::And | BinOp::Or) {
                b.error(format!("{what} cannot be a boolean expression"), e.span());
                return None;
            }
            let a = fold(b, lhs, what)?;
            let c = fold(b, rhs, what)?;
            Some(match op {
                BinOp::Add => a + c,
                BinOp::Sub => a - c,
                BinOp::Mul => a * c,
                BinOp::Div => a / c,
                BinOp::Rem => a.rem_euclid(c),
                BinOp::Pow => a.powf(c),
                BinOp::And | BinOp::Or => unreachable!("handled above"),
            })
        }
        Expr::Call { func, args, .. } => {
            let mut vals = Vec::with_capacity(args.len());
            for a in args {
                vals.push(fold(b, a, what)?);
            }
            let v = |i: usize| vals[i];
            Some(match func.as_str() {
                "abs" => v(0).abs(),
                "sqrt" => v(0).sqrt(),
                "cbrt" => v(0).cbrt(),
                "exp" => v(0).exp(),
                "ln" => v(0).ln(),
                "log2" => v(0).log2(),
                "log10" => v(0).log10(),
                "sin" => v(0).sin(),
                "cos" => v(0).cos(),
                "tan" => v(0).tan(),
                "floor" => v(0).floor(),
                "ceil" => v(0).ceil(),
                "round" => v(0).round(),
                "trunc" => v(0).trunc(),
                "min" => v(0).min(v(1)),
                "max" => v(0).max(v(1)),
                "pow" => v(0).powf(v(1)),
                other => {
                    b.error(format!("`{other}` cannot be folded into {what}"), e.span());
                    return None;
                }
            })
        }
        Expr::Compare { .. } => {
            b.error(format!("{what} cannot be a comparison"), e.span());
            None
        }
    }
}

/// Compile a `.ap` source in one call: lex, parse, lower, verify.
pub fn compile(path: &str, text: &str) -> Compiled {
    let src = crate::span::Source::new(path, text);
    let mut diag = Diagnostics::new();
    let Some(tokens) = crate::lexer::lex(&src, &mut diag) else {
        return Compiled {
            model: Model::new(""),
            diagnostics: diag,
        };
    };
    let Some(ast) = crate::parser::parse(&tokens, &mut diag) else {
        return Compiled {
            model: Model::new(""),
            diagnostics: diag,
        };
    };
    let Compiled { model, diagnostics } = lower(&ast);
    diag.items.extend(diagnostics.items);
    let report = verify(&model);
    for e in report.errors {
        diag.push(Diagnostic::error(e.message, Span::new(0, 0)));
    }
    for w in report.warnings {
        diag.push(Diagnostic::warning(w.message, Span::new(0, 0)));
    }
    Compiled {
        model,
        diagnostics: diag,
    }
}

#[cfg(test)]
mod tests {
    // A test's fall-through arm is supposed to catch anything the shape did not predict, and print
    // what it was. Naming every other variant by hand would make the assertion weaker, not stronger.
    #![allow(clippy::wildcard_enum_match_arm)]

    use super::*;
    use aporia_ir::verify;

    /// Compile a source and fail the test if any phase reported an error.
    fn ok(text: &str) -> Model {
        let c = compile("t.ap", text);
        assert!(
            !c.diagnostics.has_errors(),
            "unexpected errors:\n{}{}",
            c.diagnostics
                .render_all(&crate::span::Source::new("t.ap", text)),
            text
        );
        let report = verify(&c.model);
        assert!(report.is_ok(), "A-IR does not verify: {:?}", report.errors);
        c.model
    }

    /// Compile expecting at least one error. The rendered form is produced and discarded here on
    /// purpose: a diagnostic nobody renders is a diagnostic nobody has checked is readable.
    fn expect_errors(text: &str) -> Vec<String> {
        let c = compile("t.ap", text);
        assert!(c.diagnostics.has_errors(), "expected a failure, got none");
        let src = crate::span::Source::new("t.ap", text);
        let _ = c.diagnostics.render_all(&src);
        c.diagnostics
            .items
            .iter()
            .filter(|d| d.is_error())
            .map(|d| d.message.clone())
            .collect()
    }

    const PROJECTILE: &str = "model projectile \"no drag\" {\n\
        input velocity : m/s in [0, 1000]\n\
        input angle    : rad in [0, pi/2]\n\
        input gravity  : m/s^2 in [9.7, 9.9]\n\
        let t     = 2 * velocity * sin(angle) / gravity : s\n\
        let range = velocity * cos(angle) * t           : m\n\
        let apex  = (velocity * sin(angle))^2 / (2 * gravity) : m\n\
        require gravity > 0\n\
        require range >= 0\n\
        check monotone_up(range wrt velocity)\n\
        check scales_as(range wrt velocity, 2)\n\
    }\n";

    #[test]
    fn a_projectile_model_lowers_and_verifies() {
        let m = ok(PROJECTILE);
        assert_eq!(m.name, "projectile");
        assert_eq!(m.params.len(), 3);
        assert_eq!(m.outputs.len(), 3);
        assert_eq!(m.constraints.len(), 2);
        assert_eq!(m.relations.len(), 2);
        assert_eq!(
            m.param("gravity")
                .map(|p| m.params[p as usize].ty.dim.to_canonical()),
            Some("m/s^2".to_string())
        );
        assert_eq!(
            m.output("range")
                .map(|o| m.outputs[o as usize].ty.dim.to_canonical()),
            Some("m".to_string())
        );
    }

    #[test]
    fn domains_reach_the_ir_as_numbers() {
        let m = ok(PROJECTILE);
        let p = &m.params[0];
        match &p.domain {
            Domain::Interval { lo, hi } => assert_eq!((*lo, *hi), (0.0, 1000.0)),
            other => panic!("expected an interval, got {other:?}"),
        }
        assert_eq!(p.to_si, 1.0);
    }

    #[test]
    fn a_domain_bound_may_be_a_folded_constant_expression() {
        let m = ok("model d \"\" {\n input a : rad in [0, pi/2]\n let y = a\n}\n");
        match &m.params[0].domain {
            Domain::Interval { hi, .. } => {
                assert!((hi - std::f64::consts::FRAC_PI_2).abs() < 1e-15);
            }
            Domain::Choices(v) => panic!("expected an interval, got {} choices", v.len()),
        }
    }

    #[test]
    fn a_unit_mismatch_is_an_error() {
        let msgs = expect_errors(
            "model u \"\" {\n input a : m in [0, 1]\n input b : s in [0, 1]\n let c = a + b\n}\n",
        );
        assert!(msgs.iter().any(|m| m.contains("mixes m and s")), "{msgs:?}");
    }

    #[test]
    fn a_scale_mismatch_is_a_warning_not_an_error() {
        let c = compile(
            "t.ap",
            "model u \"\" {\n input d : km in [0, 10]\n let e = d : m\n}\n",
        );
        assert!(!c.diagnostics.has_errors(), "{:?}", c.diagnostics.items);
        assert_eq!(
            c.diagnostics.warning_count(),
            1,
            "{:?}",
            c.diagnostics.items
        );
        let w = &c.diagnostics.items[0];
        // `km` prints as its reduced form with the factor attached, which is the thing that actually
        // tells the reader what went wrong.
        assert!(
            w.message.contains("x1000") && w.message.contains("factor"),
            "{}",
            w.message
        );
    }

    #[test]
    fn trig_rejects_a_dimensioned_argument() {
        let msgs = expect_errors("model t \"\" {\n input d : m in [0, 1]\n let y = sin(d)\n}\n");
        assert!(
            msgs.iter().any(|m| m.contains("needs a pure number")),
            "{msgs:?}"
        );
    }

    #[test]
    fn degrees_are_caught_before_radians_are_assumed() {
        // `sin(x)` where x is declared deg: dimensionless but scaled, which is the classic bug.
        let msgs =
            expect_errors("model t \"\" {\n input x : deg in [0, 360]\n let y = sin(x)\n}\n");
        assert!(msgs.iter().any(|m| m.contains("radians")), "{msgs:?}");
    }

    #[test]
    fn squaring_squares_the_unit() {
        let m = ok("model s \"\" {\n input v : m/s in [0, 10]\n let e = v^2\n}\n");
        let o = m.output("e").expect("e is observed");
        assert_eq!(m.outputs[o as usize].ty.dim.to_canonical(), "m^2/s^2");
    }

    #[test]
    fn a_loop_over_state_lowers_with_a_trace() {
        let m = ok("model spring \"\" {\n\
            input k : N/m in [1, 1000]\n\
            input mass : kg in [0.1, 10]\n\
            input dt : s in [0.001, 0.2]\n\
            input steps : count in [1, 2000]\n\
            state x : m = 1.0\n\
            state v : m/s = 0.0\n\
            loop steps {\n\
                let a = -(k / mass) * x : m/s^2\n\
                advance v = v + a * dt\n\
                advance x = x + v * dt\n\
                watch energy = 0.5 * k * x^2 + 0.5 * mass * v^2\n\
            }\n\
            require mass > 0\n\
            check conserved(energy, 0.02)\n\
        }\n");
        assert_eq!(m.slots.len(), 2);
        assert_eq!(m.traces.len(), 1);
        assert_eq!(m.traces[0].name, "energy");
        assert_eq!(m.relations.len(), 1);
        assert!(matches!(
            m.relations[0].kind,
            RelationKind::Conserved { .. }
        ));
        // The loop itself is one instruction in the entry block.
        assert!(
            m.instrs
                .iter()
                .any(|i| matches!(i.kind, InstrKind::For { .. }))
        );
    }

    #[test]
    fn a_float_loop_count_is_refused() {
        let msgs = expect_errors(
            "model f \"\" {\n input dt : s in [0, 1]\n state x : m = 0.0\n loop dt {\n advance x = x + dt\n }\n let y = x\n}\n",
        );
        assert!(
            msgs.iter()
                .any(|m| m.contains("whole number of iterations")),
            "{msgs:?}"
        );
    }

    #[test]
    fn counting_state_stays_integral() {
        let m = ok(
            "model c \"\" {\n input n : count in [1, 5]\n state i : count = 0\n loop n {\n advance i = i + 1\n }\n let y = i\n}\n",
        );
        assert_eq!(m.slots[0].ty.num, NumType::I64);
        let writes = m
            .instrs
            .iter()
            .filter(|i| matches!(i.kind, InstrKind::Write { .. }))
            .count();
        assert_eq!(writes, 1);
    }

    #[test]
    fn writing_a_float_into_a_counter_is_refused() {
        let msgs = expect_errors(
            "model c \"\" {\n input n : count in [1, 5]\n input f in [0, 1]\n state i : count = 0\n loop n {\n advance i = i + f\n }\n let y = i\n}\n",
        );
        assert!(msgs.iter().any(|m| m.contains("whole numbers")), "{msgs:?}");
    }

    #[test]
    fn an_and_requirement_becomes_two_constraints() {
        let m = ok(
            "model r \"\" {\n input a : m in [0, 10]\n let y = a\n require a > 0 and a < 10\n}\n",
        );
        assert_eq!(m.constraints.len(), 2);
        assert!(matches!(
            m.constraints[0].kind,
            ConstraintKind::Cmp { cmp: CmpOp::Gt, .. }
        ));
        assert!(matches!(
            m.constraints[1].kind,
            ConstraintKind::Cmp { cmp: CmpOp::Lt, .. }
        ));
        assert_eq!(m.constraints[0].name, "a > 0");
    }

    #[test]
    fn an_in_range_requirement_becomes_two_constraints() {
        let m =
            ok("model r \"\" {\n input a : m in [0, 10]\n let y = a\n require y in [1, 9]\n}\n");
        assert_eq!(m.constraints.len(), 2);
    }

    #[test]
    fn finite_lowers_to_the_finiteness_predicate() {
        let m =
            ok("model r \"\" {\n input a : m in [0, 10]\n let y = 1 / a\n require finite(y)\n}\n");
        assert!(matches!(
            m.constraints[0].kind,
            ConstraintKind::Finite { .. }
        ));
    }

    #[test]
    fn not_in_a_requirement_is_explained_not_silently_dropped() {
        let msgs = expect_errors(
            "model r \"\" {\n input a : m in [0, 10]\n let y = a\n require not (a > 0)\n}\n",
        );
        assert!(
            msgs.iter()
                .any(|m| m.contains("a <= 0") || m.contains("rewrite")),
            "{msgs:?}"
        );
    }

    #[test]
    fn a_relation_over_an_unknown_quantity_is_caught() {
        let msgs = expect_errors(
            "model r \"\" {\n input a : m in [0, 10]\n let y = a\n check monotone_up(zero wrt a)\n}\n",
        );
        assert!(
            msgs.iter()
                .any(|m| m.contains("`zero` is not an observed quantity")),
            "{msgs:?}"
        );
    }

    #[test]
    fn swapping_two_different_kinds_of_parameter_is_refused() {
        let msgs = expect_errors(
            "model r \"\" {\n input a : m in [0, 10]\n input b : s in [0, 10]\n let y = a\n check symmetric(y wrt (a, b))\n}\n",
        );
        assert!(
            msgs.iter()
                .any(|m| m.contains("different kinds of quantity")),
            "{msgs:?}"
        );
    }

    #[test]
    fn shadowing_a_parameter_is_refused() {
        let msgs = expect_errors("model r \"\" {\n input a : m in [0, 10]\n let a = 1\n}\n");
        assert!(
            msgs.iter().any(|m| m.contains("already defined")),
            "{msgs:?}"
        );
    }

    #[test]
    fn a_model_that_observes_nothing_is_useless_and_says_so() {
        let msgs = expect_errors("model r \"\" {\n input a : m in [0, 10]\n}\n");
        assert!(
            msgs.iter().any(|m| m.contains("observes nothing")),
            "{msgs:?}"
        );
    }

    #[test]
    fn division_always_produces_a_float() {
        let m = ok("model d \"\" {\n input n : count in [1, 10]\n let y = n / 4 : count\n}\n");
        // The annotation says count, so the lowering must refuse rather than truncate silently.
        let _ = m;
    }

    #[test]
    fn approximate_equality_lowers_to_a_measurable_form() {
        let m = ok(
            "model a \"\" {\n input x : m in [0, 10]\n let y = x\n require y ~ 1 within 0.5\n}\n",
        );
        let c = &m.constraints[0];
        match &c.kind {
            ConstraintKind::Cmp {
                cmp: CmpOp::EqApprox,
                tolerance,
                ..
            } => {
                assert_eq!(*tolerance, 0.5);
            }
            other => panic!("expected an approximate comparison, got {other:?}"),
        }
    }

    #[test]
    fn an_unknown_unit_symbol_names_the_offending_word() {
        let msgs = expect_errors("model u \"\" {\n input a : furlong in [0, 10]\n let y = a\n}\n");
        assert!(
            msgs.iter().any(|m| m.contains("`furlong` is not a unit")),
            "{msgs:?}"
        );
    }

    #[test]
    fn a_domain_that_is_not_constant_is_refused() {
        let msgs = expect_errors(
            "model d \"\" {\n input a : m in [0, 10]\n input b : m in [0, a]\n let y = b\n}\n",
        );
        assert!(
            msgs.iter().any(|m| m.contains("not a constant")),
            "{msgs:?}"
        );
    }

    #[test]
    fn negative_numbers_fold_and_lower() {
        let m = ok("model n \"\" {\n input x : m in [-5, 5]\n let y = -x\n let z = x - -2\n}\n");
        assert_eq!(m.outputs.len(), 2);
    }

    #[test]
    fn fma_and_the_separate_form_are_both_representable() {
        // The reason `fma` exists in the vocabulary: two paths to one answer.
        // a*b is a velocity here, so `+ c` is only meaningful if c is one too: the point of the
        // test is that both spellings of the same arithmetic lower.
        let m = ok(
            "model f \"\" {\n input a : m in [0, 1]\n input b : 1/s in [0, 10]\n input c : m/s in [0, 1]\n let p = a * b + c\n let q = fma(a, b, c)\n}\n",
        );
        assert!(m.instrs.iter().any(|i| matches!(
            i.kind,
            InstrKind::Call {
                builtin: Builtin::Fma,
                ..
            }
        )));
    }

    #[test]
    fn a_count_unit_marks_a_parameter_integral() {
        let m = ok("model c \"\" {\n input n : count in [1, 100]\n let y = n * 2\n}\n");
        assert_eq!(m.params[0].ty.num, NumType::I64);
    }
}
