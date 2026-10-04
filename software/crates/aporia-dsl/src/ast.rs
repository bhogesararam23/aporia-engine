//! The syntax tree produced by the parser.
//!
//! This is deliberately a *syntax* tree, not a typed one: identifiers are still strings, unit
//! annotations are still expressions, and nothing here has been checked against the model's scope.
//! Keeping resolution out of the parser is what lets the parser report several unrelated mistakes in
//! one file instead of stopping at the first.
//!
//! A unit is an expression (`m/s^2` parses as `m / s ^ 2`) and a domain bound is an expression too
//! (`[0, pi/2]`). There is therefore one expression grammar in the language, not three.

use crate::span::Span;

/// A literal as written.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LitValue {
    Number { value: f64, int: bool },
    Bool(bool),
}

/// The comparison operators available in `require`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CmpSyntax {
    Lt,
    Le,
    Gt,
    Ge,
    /// `==` — exact equality, which APORIA treats as a strict requirement.
    Eq,
    Ne,
    /// `~` — approximate equality, with a tolerance.
    Approx,
}

impl CmpSyntax {
    /// The tolerance used when a `~` requirement does not state one.
    pub const DEFAULT_APPROX_TOLERANCE: f64 = 1.0e-9;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Pow,
    And,
    Or,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Lit {
        value: LitValue,
        span: Span,
    },
    Ident {
        name: String,
        span: Span,
    },
    Unary {
        op: UnOp,
        expr: Box<Expr>,
        span: Span,
    },
    Binary {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
        span: Span,
    },
    Compare {
        cmp: CmpSyntax,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
        span: Span,
    },
    Call {
        func: String,
        func_span: Span,
        args: Vec<Expr>,
        span: Span,
    },
}

impl Expr {
    #[must_use]
    pub fn span(&self) -> Span {
        match self {
            Self::Lit { span, .. }
            | Self::Ident { span, .. }
            | Self::Unary { span, .. }
            | Self::Binary { span, .. }
            | Self::Compare { span, .. }
            | Self::Call { span, .. } => *span,
        }
    }

    /// True for the forms the constant folder can evaluate: literals, named constants, and the
    /// arithmetic over them. Used for domains and unit annotations.
    #[must_use]
    pub fn is_constant_shape(&self) -> bool {
        match self {
            Self::Lit { .. } | Self::Ident { .. } => true,
            Self::Unary { expr, .. } => expr.is_constant_shape(),
            Self::Binary { lhs, rhs, op, .. } => {
                *op != BinOp::And
                    && *op != BinOp::Or
                    && lhs.is_constant_shape()
                    && rhs.is_constant_shape()
            }
            Self::Compare { .. } => false,
            Self::Call { func, args, .. } => {
                crate::builtins::is_constant_callable(func)
                    && args.iter().all(Self::is_constant_shape)
            }
        }
    }
}

/// The right-hand side of an `input` declaration: what the sampler is allowed to draw.
#[derive(Clone, Debug, PartialEq)]
pub enum DomainSyntax {
    Interval { lo: Expr, hi: Expr },
    Choices(Vec<Expr>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct InputDecl {
    pub name: String,
    pub span: Span,
    /// Unit annotation, still an expression, resolved by the checker.
    pub unit: Option<Expr>,
    pub domain: DomainSyntax,
    pub doc: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StateDecl {
    pub name: String,
    pub span: Span,
    pub unit: Option<Expr>,
    pub init: Expr,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LetDecl {
    pub name: String,
    pub span: Span,
    pub unit: Option<Expr>,
    pub value: Expr,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AdvanceStmt {
    pub name: String,
    pub name_span: Span,
    pub value: Expr,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LoopStmt {
    pub trip: Expr,
    pub span: Span,
    pub body: Vec<Member>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WatchStmt {
    /// Explicit name when written as `watch energy = ...`.
    pub name: Option<String>,
    pub value: Expr,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RequireKind {
    /// Any boolean expression: `require gravity > 0`, `require finite(range)`, or
    /// `require a > 0 and b < 1`. The checker decides what it is.
    ///
    /// Keeping the whole expression here rather than pre-splitting it into comparison parts is
    /// what lets `and`/`or` nest arbitrarily without a second grammar.
    Predicate {
        value: Expr,
        tolerance: Option<Expr>,
    },
    /// `require range in [0, 1000]`
    InRange { value: Expr, lo: Expr, hi: Expr },
}

impl RequireKind {
    #[must_use]
    pub fn span(&self) -> Span {
        match self {
            Self::Predicate { value, tolerance } => match tolerance {
                Some(t) => value.span().merge(t.span()),
                None => value.span(),
            },
            Self::InRange { value, lo, hi } => value.span().merge(lo.span()).merge(hi.span()),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RequireStmt {
    pub kind: RequireKind,
    pub span: Span,
    pub doc: Option<String>,
}

/// Direction of a monotonicity relation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MonotoneDir {
    Up,
    Down,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RelationSyntax {
    Monotone {
        out: String,
        out_span: Span,
        wrt: String,
        wrt_span: Span,
        dir: MonotoneDir,
    },
    ScalesAs {
        out: String,
        out_span: Span,
        wrt: String,
        wrt_span: Span,
        power: Expr,
    },
    Symmetric {
        out: String,
        out_span: Span,
        a: String,
        b: String,
        b_span: Span,
    },
    Conserved {
        what: String,
        what_span: Span,
        tolerance: Option<Expr>,
    },
    Lipschitz {
        out: String,
        out_span: Span,
        wrt: String,
        wrt_span: Span,
        bound: Expr,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct CheckStmt {
    pub relation: RelationSyntax,
    pub span: Span,
    pub doc: Option<String>,
}

/// Anything that can appear in a model body or a loop body.
#[derive(Clone, Debug, PartialEq)]
pub enum Member {
    Input(InputDecl),
    State(StateDecl),
    Let(LetDecl),
    Advance(AdvanceStmt),
    Loop(LoopStmt),
    Watch(WatchStmt),
    Require(RequireStmt),
    Check(CheckStmt),
}

impl Member {
    #[must_use]
    pub fn span(&self) -> Span {
        match self {
            Self::Input(d) => d.span,
            Self::State(d) => d.span,
            Self::Let(d) => d.span,
            Self::Advance(s) => s.span,
            Self::Loop(s) => s.span,
            Self::Watch(s) => s.span,
            Self::Require(s) => s.span,
            Self::Check(s) => s.span,
        }
    }

    /// Statements that are only legal inside a `loop`.
    #[must_use]
    pub fn needs_loop(&self) -> bool {
        matches!(self, Self::Advance(_) | Self::Watch(_))
    }

    /// Statements that describe the model rather than compute in it, and so are not allowed in a
    /// loop body.
    #[must_use]
    pub fn is_declaration(&self) -> bool {
        matches!(self, Self::Input(_) | Self::State(_))
    }
}

/// A parsed model. A file holds exactly one.
#[derive(Clone, Debug, PartialEq)]
pub struct AstModel {
    pub name: String,
    pub name_span: Span,
    pub doc: Option<String>,
    pub span: Span,
    pub members: Vec<Member>,
}
