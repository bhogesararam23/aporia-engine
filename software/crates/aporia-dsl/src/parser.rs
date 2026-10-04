//! The parser: tokens in, syntax tree out.
//!
//! Recursive descent with a precedence ladder for expressions. Two properties matter more than
//! elegance here.
//!
//! **Recovery.** A statement that fails to parse is reported and then skipped to the next line
//! boundary, so a file with five mistakes yields five messages in one run. Editing a model should
//! feel like editing code, not like talking to a compiler that gives up on the first surprise.
//!
//! **One expression grammar.** Unit annotations, domain bounds and model bodies all parse through
//! the same `expr`, which is why there is only one precedence table to reason about and only one
//! place where `-x^2` is decided.

use crate::ast::{
    AdvanceStmt, AstModel, BinOp, CheckStmt, CmpSyntax, DomainSyntax, Expr, InputDecl, LetDecl,
    LitValue, LoopStmt, Member, MonotoneDir, RelationSyntax, RequireKind, RequireStmt, StateDecl,
    UnOp, WatchStmt,
};
use crate::builtins;
use crate::span::{Diagnostic, Diagnostics, Span};
use crate::token::{Keyword, Punct, Token, TokenKind};

/// A statement or expression that could not be parsed. The diagnostic is already recorded; this
/// just unwinds to the next recovery point.
struct Bail;

type Parsed<T> = Result<T, Bail>;

/// Parse a token stream into a model, appending findings to `out`.
pub fn parse(tokens: &[Token], out: &mut Diagnostics) -> Option<AstModel> {
    if tokens.is_empty() {
        out.error("the file is empty", Span::new(0, 0));
        return None;
    }
    let mut p = Parser {
        toks: tokens,
        i: 0,
        errors: Vec::new(),
    };
    let model = p.model();
    out.items.append(&mut p.errors);
    match model {
        Ok(m) if !out.has_errors() => Some(m),
        _ => None,
    }
}

struct Parser<'a> {
    toks: &'a [Token],
    i: usize,
    errors: Vec<Diagnostic>,
}

impl Parser<'_> {
    // ------------------------------------------------------------------ cursor

    fn peek_kind(&self) -> &TokenKind {
        &self.toks[self.i.min(self.toks.len() - 1)].kind
    }

    fn peek_span(&self) -> Span {
        self.toks[self.i.min(self.toks.len() - 1)].span
    }

    fn at_punct(&self, p: Punct) -> bool {
        matches!(self.peek_kind(), TokenKind::Punct(x) if *x == p)
    }

    fn at_keyword(&self, k: Keyword) -> bool {
        matches!(self.peek_kind(), TokenKind::Keyword(x) if *x == k)
    }

    fn at_newline(&self) -> bool {
        matches!(self.peek_kind(), TokenKind::Newline)
    }

    fn at_eof(&self) -> bool {
        matches!(self.peek_kind(), TokenKind::Eof)
    }

    fn bump(&mut self) -> Token {
        let t = self.toks[self.i.min(self.toks.len() - 1)].clone();
        if self.i < self.toks.len() {
            self.i += 1;
        }
        t
    }

    fn eat_punct(&mut self, p: Punct) -> bool {
        if self.at_punct(p) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn eat_newlines(&mut self) {
        while self.at_newline() {
            self.bump();
        }
    }

    fn expect_punct(&mut self, p: Punct) -> Parsed<Span> {
        if self.at_punct(p) {
            Ok(self.bump().span)
        } else {
            Err(self.unexpected(&format!("`{}`", p.text())))
        }
    }

    fn expect_ident(&mut self) -> Parsed<(String, Span)> {
        match self.peek_kind().clone() {
            TokenKind::Ident(name) => {
                let span = self.bump().span;
                Ok((name, span))
            }
            other => {
                let span = self.peek_span();
                self.errors.push(Diagnostic::error(
                    format!("expected a name, found {}", other.describe()),
                    span,
                ));
                Err(Bail)
            }
        }
    }

    fn unexpected(&mut self, expected: &str) -> Bail {
        let kind = self.peek_kind().clone();
        let span = self.peek_span();
        self.errors.push(Diagnostic::error(
            format!("expected {expected}, found {}", kind.describe()),
            span,
        ));
        Bail
    }

    /// After a failed statement, drop tokens until the next plausible statement start.
    fn recover(&mut self) {
        loop {
            if self.at_eof() {
                return;
            }
            if self.at_newline() || self.at_punct(Punct::Semi) {
                self.bump();
                return;
            }
            // A `}` on its own line ends the block; do not swallow it.
            if self.at_punct(Punct::RBrace) {
                return;
            }
            self.bump();
        }
    }

    // ------------------------------------------------------------------- model

    fn model(&mut self) -> Parsed<AstModel> {
        self.eat_newlines();
        if !self.at_keyword(Keyword::Model) {
            return Err(self.unexpected("the `model` keyword"));
        }
        let start = self.bump().span;
        let (name, name_span) = self.expect_ident()?;
        let doc = self.optional_string();
        self.expect_punct(Punct::LBrace)?;
        let mut members = Vec::new();
        loop {
            self.eat_newlines();
            if self.at_punct(Punct::RBrace) {
                break;
            }
            if self.at_eof() {
                let span = self.peek_span().merge(start);
                self.errors.push(
                    Diagnostic::error("the model body is never closed", span)
                        .with_label(start, "`model {name}` starts here"),
                );
                return Err(Bail);
            }
            match self.member(false) {
                Ok(m) => members.push(m),
                // `member` already recorded the diagnostic.
                Err(_) => self.recover(),
            }
        }
        let end = self.bump().span;
        Ok(AstModel {
            name,
            name_span,
            doc,
            span: start.merge(end),
            members,
        })
    }

    fn optional_string(&mut self) -> Option<String> {
        if let TokenKind::Str(s) = self.peek_kind().clone() {
            self.bump();
            Some(s)
        } else {
            None
        }
    }

    // ----------------------------------------------------------------- members

    /// Parse one statement. `inside_loop` restricts which members are legal.
    fn member(&mut self, inside_loop: bool) -> Parsed<Member> {
        let span = self.peek_span();
        let keyword = match self.peek_kind() {
            TokenKind::Keyword(k) => Some(*k),
            _ => None,
        };
        let member = match keyword {
            Some(Keyword::Input) => Member::Input(self.input_decl()?),
            Some(Keyword::State) => Member::State(self.state_decl()?),
            Some(Keyword::Let) => Member::Let(self.let_decl()?),
            Some(Keyword::Advance) => Member::Advance(self.advance()?),
            Some(Keyword::Loop) => Member::Loop(self.loop_stmt()?),
            Some(Keyword::Watch) => Member::Watch(self.watch()?),
            Some(Keyword::Require) => Member::Require(self.require()?),
            Some(Keyword::Check) => Member::Check(self.check()?),
            Some(k) => {
                let found = k.text();
                self.errors.push(
                    Diagnostic::error(format!("`{found}` cannot start a statement"), span)
                        .with_help("statements start with input, state, let, advance, loop, watch, require or check"),
                );
                return Err(Bail);
            }
            None => {
                let kind = self.peek_kind().clone();
                self.errors.push(Diagnostic::error(
                    format!("expected a statement, found {}", kind.describe()),
                    span,
                ));
                return Err(Bail);
            }
        };
        if inside_loop && member.is_declaration() {
            self.errors.push(
                Diagnostic::error(
                    "a parameter or state cannot be declared inside a loop",
                    member.span(),
                )
                .with_help("declare parameters and state at model level; a loop body computes"),
            );
            return Err(Bail);
        }
        if !inside_loop && member.needs_loop() {
            let what = if matches!(member, Member::Advance(_)) {
                "advance"
            } else {
                "watch"
            };
            self.errors.push(
                Diagnostic::error(
                    format!("`{what}` is only meaningful inside a loop"),
                    member.span(),
                )
                .with_help("wrap the statement in `loop steps { ... }`"),
            );
            return Err(Bail);
        }
        // A semicolon is an accepted, if unnecessary, statement end.
        if self.at_punct(Punct::Semi) {
            self.bump();
        }
        if !self.at_newline() && !self.at_eof() && !self.at_punct(Punct::RBrace) {
            let kind = self.peek_kind().clone();
            self.errors.push(
                Diagnostic::error(
                    format!("unexpected {} after the statement", kind.describe()),
                    self.peek_span(),
                )
                .with_help("statements are separated by line breaks"),
            );
            return Err(Bail);
        }
        Ok(member)
    }

    /// `unit` and domain bounds are ordinary expressions, so they reuse `expr`.
    fn annotated_unit(&mut self) -> Parsed<Option<Expr>> {
        if self.eat_punct(Punct::Colon) {
            Ok(Some(self.expr()?))
        } else {
            Ok(None)
        }
    }

    fn input_decl(&mut self) -> Parsed<InputDecl> {
        let start = self.bump().span;
        let (name, name_span) = self.expect_ident()?;
        let unit = self.annotated_unit()?;
        if !self.at_keyword(Keyword::In) {
            return Err(self.unexpected("`in` followed by the domain"));
        }
        self.bump();
        let domain = self.domain()?;
        let doc = self.optional_string();
        Ok(InputDecl {
            name,
            span: start.merge(name_span),
            unit,
            domain,
            doc,
        })
    }

    /// `[lo, hi]` for a continuous range, `{a, b, c}` for a fixed set of settings.
    fn domain(&mut self) -> Parsed<DomainSyntax> {
        if self.at_punct(Punct::LBracket) {
            self.bump();
            let lo = self.expr()?;
            self.expect_punct(Punct::Comma)?;
            let hi = self.expr()?;
            self.expect_punct(Punct::RBracket)?;
            return Ok(DomainSyntax::Interval { lo, hi });
        }
        if self.at_punct(Punct::LBrace) {
            self.bump();
            let mut items = vec![self.expr()?];
            while self.eat_punct(Punct::Comma) {
                items.push(self.expr()?);
            }
            self.expect_punct(Punct::RBrace)?;
            return Ok(DomainSyntax::Choices(items));
        }
        Err(self.unexpected("a domain: `[low, high]` or `{a, b, c}`"))
    }

    fn state_decl(&mut self) -> Parsed<StateDecl> {
        let start = self.bump().span;
        let (name, name_span) = self.expect_ident()?;
        let leading = self.annotated_unit()?;
        self.expect_punct(Punct::Equals)?;
        let init = self.expr()?;
        let trailing = self.annotated_unit()?;
        let unit = match (leading, trailing) {
            (Some(_), Some(_)) => {
                self.errors.push(
                    Diagnostic::error(
                        format!("`{name}` has a unit annotation on both sides of the `=`"),
                        start.merge(init.span()),
                    )
                    .with_help("state is declared as `state name : unit = value`"),
                );
                return Err(Bail);
            }
            (l, t) => l.or(t),
        };
        Ok(StateDecl {
            name,
            span: start.merge(name_span),
            unit,
            init,
        })
    }

    fn let_decl(&mut self) -> Parsed<LetDecl> {
        let start = self.bump().span;
        let (name, name_span) = self.expect_ident()?;
        let leading = self.annotated_unit()?;
        self.expect_punct(Punct::Equals)?;
        let value = self.expr()?;
        let trailing = self.annotated_unit()?;
        let unit = match (leading, trailing) {
            (Some(_), Some(_)) => {
                self.errors.push(Diagnostic::error(
                    format!("`{name}` has a unit annotation on both sides of the `=`"),
                    start.merge(value.span()),
                ));
                return Err(Bail);
            }
            (l, t) => l.or(t),
        };
        Ok(LetDecl {
            name,
            span: start.merge(name_span),
            unit,
            value,
        })
    }

    fn advance(&mut self) -> Parsed<AdvanceStmt> {
        let start = self.bump().span;
        let (name, name_span) = self.expect_ident()?;
        self.expect_punct(Punct::Equals)?;
        let value = self.expr()?;
        Ok(AdvanceStmt {
            name,
            name_span,
            value,
            span: start.merge(name_span),
        })
    }

    fn loop_stmt(&mut self) -> Parsed<LoopStmt> {
        let start = self.bump().span;
        let trip = self.expr()?;
        self.expect_punct(Punct::LBrace)?;
        let mut body = Vec::new();
        loop {
            self.eat_newlines();
            if self.at_punct(Punct::RBrace) {
                break;
            }
            if self.at_eof() {
                let span = self.peek_span().merge(start);
                self.errors
                    .push(Diagnostic::error("this loop body is never closed", span));
                return Err(Bail);
            }
            match self.member(true) {
                Ok(m) => body.push(m),
                Err(_) => self.recover(),
            }
        }
        let end = self.bump().span;
        Ok(LoopStmt {
            trip,
            span: start.merge(end),
            body,
        })
    }

    /// `watch energy = 0.5*k*x*x` names the traced quantity; `watch energy` traces the value of an
    /// existing expression. The two forms are told apart by what follows the first identifier.
    fn watch(&mut self) -> Parsed<WatchStmt> {
        let start = self.bump().span;
        let named = if matches!(self.peek_kind(), TokenKind::Ident(_)) {
            let mark = self.i;
            let name = self.expect_ident()?.0;
            if self.at_punct(Punct::Equals) {
                self.bump();
                Some(name)
            } else {
                // It was the start of an expression, not a binding name.
                self.i = mark;
                None
            }
        } else {
            None
        };
        let value = self.expr()?;
        let span = start.merge(value.span());
        Ok(WatchStmt {
            name: named,
            value,
            span,
        })
    }

    /// `require <boolean expression> [within <number>] [\"doc\"]`, plus the `in [lo, hi]` form.
    fn require(&mut self) -> Parsed<RequireStmt> {
        let start = self.bump().span;
        let value = self.expr()?;
        let kind = if self.at_keyword(Keyword::In) {
            self.bump();
            self.expect_punct(Punct::LBracket)?;
            let lo = self.expr()?;
            self.expect_punct(Punct::Comma)?;
            let hi = self.expr()?;
            self.expect_punct(Punct::RBracket)?;
            RequireKind::InRange { value, lo, hi }
        } else {
            let tolerance = if matches!(self.peek_kind(), TokenKind::Ident(n) if n == "within") {
                self.bump();
                Some(self.expr()?)
            } else {
                None
            };
            // A requirement is a boolean expression. These are the shapes that qualify; the
            // checker decides what each one means.
            let boolean_shape = match &value {
                Expr::Compare { .. }
                | Expr::Unary { .. }
                | Expr::Binary {
                    op: BinOp::And | BinOp::Or,
                    ..
                } => true,
                Expr::Call { func, .. } => func == "finite",
                _ => false,
            };
            if !boolean_shape {
                self.errors.push(
                    Diagnostic::error(
                        "a require needs a comparison, `in [lo, hi]`, or finite(...)",
                        start.merge(value.span()),
                    )
                    .with_help("for example: require energy >= 0"),
                );
                return Err(Bail);
            }
            if tolerance.is_some() && !matches!(value, Expr::Compare { .. }) {
                self.errors.push(
                    Diagnostic::error(
                        "`within` belongs to a single comparison, not a compound requirement",
                        value.span(),
                    )
                    .with_help("write one requirement per line"),
                );
                return Err(Bail);
            }
            RequireKind::Predicate { value, tolerance }
        };
        let doc = self.optional_string();
        let span = start.merge(kind.span());
        Ok(RequireStmt { kind, span, doc })
    }

    fn comparison_here(&self) -> Option<CmpSyntax> {
        let p = match self.peek_kind() {
            TokenKind::Punct(p) => *p,
            _ => return None,
        };
        Some(match p {
            Punct::Lt => CmpSyntax::Lt,
            Punct::Le => CmpSyntax::Le,
            Punct::Gt => CmpSyntax::Gt,
            Punct::Ge => CmpSyntax::Ge,
            Punct::Eq => CmpSyntax::Eq,
            Punct::Ne => CmpSyntax::Ne,
            Punct::Approx => CmpSyntax::Approx,
            _ => return None,
        })
    }

    /// `check scales_as(range wrt velocity, 2)` and its four siblings.
    ///
    /// One arm per relation: the shapes differ enough that a shared prologue would hide more than
    /// it saves.
    #[expect(clippy::too_many_lines)]
    fn check(&mut self) -> Parsed<CheckStmt> {
        let start = self.bump().span;
        let (name, name_span) = self.expect_ident()?;
        let relation = match name.as_str() {
            "monotone_up" | "monotone_down" => {
                let dir = if name == "monotone_up" {
                    MonotoneDir::Up
                } else {
                    MonotoneDir::Down
                };
                self.expect_punct(Punct::LParen)?;
                let (out, out_span) = self.expect_ident()?;
                self.expect_wrt()?;
                let (wrt, wrt_span) = self.expect_ident()?;
                self.expect_punct(Punct::RParen)?;
                RelationSyntax::Monotone {
                    out,
                    out_span,
                    wrt,
                    wrt_span,
                    dir,
                }
            }
            "scales_as" => {
                self.expect_punct(Punct::LParen)?;
                let (out, out_span) = self.expect_ident()?;
                self.expect_wrt()?;
                let (wrt, wrt_span) = self.expect_ident()?;
                self.expect_punct(Punct::Comma)?;
                let power = self.expr()?;
                self.expect_punct(Punct::RParen)?;
                RelationSyntax::ScalesAs {
                    out,
                    out_span,
                    wrt,
                    wrt_span,
                    power,
                }
            }
            "lipschitz" => {
                self.expect_punct(Punct::LParen)?;
                let (out, out_span) = self.expect_ident()?;
                self.expect_wrt()?;
                let (wrt, wrt_span) = self.expect_ident()?;
                self.expect_punct(Punct::Comma)?;
                let bound = self.expr()?;
                self.expect_punct(Punct::RParen)?;
                RelationSyntax::Lipschitz {
                    out,
                    out_span,
                    wrt,
                    wrt_span,
                    bound,
                }
            }
            "symmetric" => {
                self.expect_punct(Punct::LParen)?;
                let (out, out_span) = self.expect_ident()?;
                self.expect_wrt()?;
                self.expect_punct(Punct::LParen)?;
                let (a, _) = self.expect_ident()?;
                self.expect_punct(Punct::Comma)?;
                let (b, b_span) = self.expect_ident()?;
                self.expect_punct(Punct::RParen)?;
                self.expect_punct(Punct::RParen)?;
                RelationSyntax::Symmetric {
                    out,
                    out_span,
                    a,
                    b,
                    b_span,
                }
            }
            "conserved" => {
                self.expect_punct(Punct::LParen)?;
                let (what, what_span) = self.expect_ident()?;
                let tolerance = if self.eat_punct(Punct::Comma) {
                    Some(self.expr()?)
                } else {
                    None
                };
                self.expect_punct(Punct::RParen)?;
                RelationSyntax::Conserved {
                    what,
                    what_span,
                    tolerance,
                }
            }
            other => {
                self.errors.push(
                    Diagnostic::error(
                        format!("`{other}` is not a relation APORIA checks"),
                        name_span,
                    )
                    .with_help(
                        "the relations are monotone_up, monotone_down, scales_as, symmetric, \
                         conserved and lipschitz",
                    ),
                );
                return Err(Bail);
            }
        };
        let doc = self.optional_string();
        let last = self.toks[(self.i - 1).min(self.toks.len() - 1)].span;
        Ok(CheckStmt {
            relation,
            span: start.merge(last),
            doc,
        })
    }

    /// `wrt` only appears inside a relation, where forgetting it is the commonest mistake.
    fn expect_wrt(&mut self) -> Parsed<Span> {
        if self.at_keyword(Keyword::Wrt) {
            Ok(self.bump().span)
        } else {
            Err(self.unexpected("`wrt <parameter>`"))
        }
    }

    // ------------------------------------------------------------- expressions

    fn expr(&mut self) -> Parsed<Expr> {
        self.or_expr()
    }

    fn or_expr(&mut self) -> Parsed<Expr> {
        let mut lhs = self.and_expr()?;
        while self.at_keyword(Keyword::Or) {
            let op_span = self.bump().span;
            let rhs = self.and_expr()?;
            let span = lhs.span().merge(rhs.span()).merge(op_span);
            lhs = Expr::Binary {
                op: BinOp::Or,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                span,
            };
        }
        Ok(lhs)
    }

    fn and_expr(&mut self) -> Parsed<Expr> {
        let mut lhs = self.cmp_expr()?;
        while self.at_keyword(Keyword::And) {
            let op_span = self.bump().span;
            let rhs = self.cmp_expr()?;
            let span = lhs.span().merge(rhs.span()).merge(op_span);
            lhs = Expr::Binary {
                op: BinOp::And,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                span,
            };
        }
        Ok(lhs)
    }

    /// Comparisons do not chain: `a < b < c` is rejected rather than silently parsed as one of the
    /// two meanings people would give it.
    fn cmp_expr(&mut self) -> Parsed<Expr> {
        let lhs = self.add_expr()?;
        let Some(cmp) = self.comparison_here() else {
            return Ok(lhs);
        };
        let op_span = self.bump().span;
        let rhs = self.add_expr()?;
        let span = lhs.span().merge(rhs.span()).merge(op_span);
        let after = self.comparison_here();
        if after.is_some() {
            self.errors.push(
                Diagnostic::error("comparisons do not chain in APORIA", span)
                    .with_help("write `a < b and b < c` instead"),
            );
            return Err(Bail);
        }
        Ok(Expr::Compare {
            cmp,
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
            span,
        })
    }

    fn add_expr(&mut self) -> Parsed<Expr> {
        let mut lhs = self.mul_expr()?;
        loop {
            let op = if self.at_punct(Punct::Plus) {
                Some(BinOp::Add)
            } else if self.at_punct(Punct::Minus) {
                Some(BinOp::Sub)
            } else {
                None
            };
            let Some(op) = op else { break };
            let op_span = self.bump().span;
            let rhs = self.mul_expr()?;
            let span = lhs.span().merge(rhs.span()).merge(op_span);
            lhs = Expr::Binary {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                span,
            };
        }
        Ok(lhs)
    }

    fn mul_expr(&mut self) -> Parsed<Expr> {
        let mut lhs = self.unary_expr()?;
        loop {
            let op = if self.at_punct(Punct::Star) {
                Some(BinOp::Mul)
            } else if self.at_punct(Punct::Slash) {
                Some(BinOp::Div)
            } else if self.at_punct(Punct::Percent) {
                Some(BinOp::Rem)
            } else {
                None
            };
            let Some(op) = op else { break };
            let op_span = self.bump().span;
            let rhs = self.unary_expr()?;
            let span = lhs.span().merge(rhs.span()).merge(op_span);
            lhs = Expr::Binary {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                span,
            };
        }
        Ok(lhs)
    }

    fn unary_expr(&mut self) -> Parsed<Expr> {
        if self.at_punct(Punct::Minus) {
            let span = self.bump().span;
            let inner = self.unary_expr()?;
            let full = span.merge(inner.span());
            return Ok(Expr::Unary {
                op: UnOp::Neg,
                expr: Box::new(inner),
                span: full,
            });
        }
        if self.at_keyword(Keyword::Not) {
            let span = self.bump().span;
            let inner = self.unary_expr()?;
            let full = span.merge(inner.span());
            return Ok(Expr::Unary {
                op: UnOp::Not,
                expr: Box::new(inner),
                span: full,
            });
        }
        self.power_expr()
    }

    /// `^` binds tighter than unary minus on its left, so `-x^2` is `-(x^2)` as a scientist writes
    /// it, and `2^-1` is still accepted.
    fn power_expr(&mut self) -> Parsed<Expr> {
        let base = self.atom()?;
        if self.eat_punct(Punct::Caret) {
            let op_span = self.peek_span();
            let exponent = self.unary_expr()?;
            let span = base.span().merge(exponent.span()).merge(op_span);
            return Ok(Expr::Binary {
                op: BinOp::Pow,
                lhs: Box::new(base),
                rhs: Box::new(exponent),
                span,
            });
        }
        Ok(base)
    }

    fn atom(&mut self) -> Parsed<Expr> {
        let span = self.peek_span();
        match self.peek_kind().clone() {
            TokenKind::Number { value, int } => {
                self.bump();
                Ok(Expr::Lit {
                    value: LitValue::Number { value, int },
                    span,
                })
            }
            TokenKind::Keyword(Keyword::True | Keyword::False) => {
                let value = matches!(self.bump().kind, TokenKind::Keyword(Keyword::True));
                Ok(Expr::Lit {
                    value: LitValue::Bool(value),
                    span,
                })
            }
            TokenKind::Punct(Punct::LParen) => {
                self.bump();
                let inner = self.expr()?;
                self.expect_punct(Punct::RParen)?;
                Ok(inner)
            }
            TokenKind::Ident(name) => {
                self.bump();
                if self.at_punct(Punct::LParen) {
                    return self.call(name, span);
                }
                Ok(Expr::Ident { name, span })
            }
            other => Err(self.unexpected(&format!(
                "a number, a name or `(`, found {}",
                other.describe()
            ))),
        }
    }

    fn call(&mut self, func: String, name_span: Span) -> Parsed<Expr> {
        let open = self.expect_punct(Punct::LParen)?;
        let mut args = Vec::new();
        if !self.at_punct(Punct::RParen) {
            args.push(self.expr()?);
            while self.eat_punct(Punct::Comma) {
                args.push(self.expr()?);
            }
        }
        let close = self.expect_punct(Punct::RParen)?;
        if let Some(f) = builtins::lookup(&func)
            && f.arity != args.len()
        {
            self.errors.push(
                Diagnostic::error(
                    format!(
                        "`{}` takes {} argument{}, found {}",
                        f.name,
                        f.arity,
                        if f.arity == 1 { "" } else { "s" },
                        args.len()
                    ),
                    name_span.merge(open).merge(close),
                )
                .with_help("the builtins are listed in the language reference"),
            );
            return Err(Bail);
        }
        if builtins::lookup(&func).is_none() && builtins::named_constant(&func).is_none() {
            self.errors.push(Diagnostic::error(
                format!("`{func}` is not a function APORIA knows"),
                name_span,
            ));
            return Err(Bail);
        }
        let span = name_span.merge(open).merge(close);
        Ok(Expr::Call {
            func,
            func_span: name_span,
            args,
            span,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::lex;
    use crate::span::Source;

    /// Lex and parse, failing the test if either stage produced an error.
    fn model(text: &str) -> AstModel {
        let src = Source::new("t.ap", text);
        let mut d = Diagnostics::new();
        let toks = lex(&src, &mut d).unwrap_or_else(|| panic!("lex: {d}"));
        parse(&toks, &mut d).unwrap_or_else(|| panic!("parse: {d}"))
    }

    /// Parse expecting failure, and return every message in order.
    fn messages(text: &str) -> Vec<String> {
        let src = Source::new("t.ap", text);
        let mut d = Diagnostics::new();
        if let Some(toks) = lex(&src, &mut d) {
            parse(&toks, &mut d);
        }
        d.items
            .iter()
            .map(|x| match &x.help {
                Some(h) => format!("{} {}", x.message, h),
                None => x.message.clone(),
            })
            .collect()
    }

    fn members(text: &str) -> Vec<Member> {
        model(&format!("model x \"\" {{\n{text}\n}}\n")).members
    }

    fn one(text: &str) -> Member {
        let mut ms = members(text);
        assert_eq!(ms.len(), 1, "{ms:?}");
        ms.remove(0)
    }

    #[test]
    fn a_minimal_model_parses() {
        let m = model("model tiny \"doc\" {\n}\n");
        assert_eq!(m.name, "tiny");
        assert_eq!(m.doc.as_deref(), Some("doc"));
        assert_eq!(m.members, Vec::new(), "a bare model has no members");
    }

    #[test]
    fn input_takes_a_unit_and_a_domain() {
        let Member::Input(d) = one("input velocity : m/s in [0, 1000]") else {
            panic!("not an input");
        };
        assert_eq!(d.name, "velocity");
        assert!(d.unit.is_some());
        match d.domain {
            DomainSyntax::Interval { lo, hi } => {
                assert!(
                    matches!(
                        lo,
                        Expr::Lit {
                            value: LitValue::Number {
                                value: 0.0,
                                int: true
                            },
                            ..
                        }
                    ),
                    "{lo:?}"
                );
                assert!(hi.span().end > lo.span().start);
            }
            DomainSyntax::Choices(v) => panic!("expected an interval, got {} choices", v.len()),
        }
    }

    #[test]
    fn a_choices_domain_is_a_set_of_expressions() {
        let Member::Input(d) = one("input scheme in {1, 2, 3}") else {
            panic!()
        };
        match d.domain {
            DomainSyntax::Choices(v) => assert_eq!(v.len(), 3),
            DomainSyntax::Interval { .. } => panic!("expected a choice set, got an interval"),
        }
    }

    #[test]
    fn a_domain_bound_may_be_an_expression() {
        let Member::Input(d) = one("input angle : rad in [0, pi/2]") else {
            panic!()
        };
        match d.domain {
            DomainSyntax::Interval { hi, .. } => {
                assert!(matches!(hi, Expr::Binary { op: BinOp::Div, .. }), "{hi:?}");
            }
            DomainSyntax::Choices(v) => panic!("expected an interval, got {} choices", v.len()),
        }
    }

    #[test]
    fn multiplication_binds_tighter_than_addition() {
        let e = match one("let y = 1 + 2 * 3") {
            Member::Let(d) => d.value,
            other => panic!("{other:?}"),
        };
        let Expr::Binary { op, lhs, rhs, .. } = e else {
            panic!()
        };
        assert_eq!(op, BinOp::Add);
        assert!(matches!(*lhs, Expr::Lit { .. }));
        assert!(matches!(*rhs, Expr::Binary { op: BinOp::Mul, .. }));
    }

    #[test]
    fn exponentiation_is_right_associative() {
        let e = match one("let y = 2 ^ 3 ^ 2") {
            Member::Let(d) => d.value,
            other => panic!("{other:?}"),
        };
        let Expr::Binary { rhs, .. } = e else {
            panic!()
        };
        assert!(
            matches!(*rhs, Expr::Binary { op: BinOp::Pow, .. }),
            "2^(3^2)"
        );
    }

    #[test]
    fn unary_minus_binds_looser_than_exponent() {
        // A scientist writing `-x^2` means `-(x^2)`; the other reading is a notation error.
        let e = match one("let y = -x^2") {
            Member::Let(d) => d.value,
            other => panic!("{other:?}"),
        };
        let Expr::Unary {
            op: UnOp::Neg,
            expr,
            ..
        } = e
        else {
            panic!("{e:?}")
        };
        assert!(matches!(*expr, Expr::Binary { op: BinOp::Pow, .. }));
    }

    #[test]
    fn an_exponent_may_be_negative() {
        let e = match one("let y = 2 ^ -1") {
            Member::Let(d) => d.value,
            other => panic!("{other:?}"),
        };
        let Expr::Binary {
            op: BinOp::Pow,
            rhs,
            ..
        } = e
        else {
            panic!("{e:?}")
        };
        assert!(matches!(*rhs, Expr::Unary { op: UnOp::Neg, .. }));
    }

    #[test]
    fn comparisons_do_not_chain_and_say_so() {
        let msgs = messages("model x \"\" {\n require 0 < a < 1\n}\n");
        assert!(msgs.iter().any(|m| m.contains("do not chain")), "{msgs:?}");
    }

    #[test]
    fn and_or_are_expressions_not_statements() {
        let r = match one("require a > 0 and b > 0") {
            Member::Require(s) => s.kind,
            other => panic!("{other:?}"),
        };
        match r {
            RequireKind::Predicate { value, .. } => {
                assert!(
                    matches!(value, Expr::Binary { op: BinOp::And, .. }),
                    "{value:?}"
                );
            }
            RequireKind::InRange { .. } => panic!("expected a predicate, got a range"),
        }
    }

    #[test]
    fn require_accepts_predicate_and_range_forms() {
        let pred = match one("require finite(range)") {
            Member::Require(s) => s.kind,
            _ => unreachable!(),
        };
        assert!(matches!(pred, RequireKind::Predicate { .. }), "{pred:?}");
        let rng = match one("require range in [0, 1000]") {
            Member::Require(s) => s.kind,
            _ => unreachable!(),
        };
        assert!(matches!(rng, RequireKind::InRange { .. }), "{rng:?}");
    }

    #[test]
    fn approximate_requirement_can_carry_a_tolerance() {
        let k = match one("require energy ~ 1 within 1e-6") {
            Member::Require(s) => s.kind,
            _ => unreachable!(),
        };
        match k {
            RequireKind::Predicate {
                value: Expr::Compare { cmp, .. },
                tolerance: Some(t),
                ..
            } => {
                assert_eq!(cmp, CmpSyntax::Approx);
                assert!(matches!(t, Expr::Lit { .. }), "{t:?}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn state_let_and_advance_are_distinguished() {
        assert!(matches!(one("state x : m = 1.0"), Member::State(_)));
        assert!(matches!(one("let v = x / dt"), Member::Let(_)));
        let Member::Loop(l) = one("loop 10 {\n advance x = x + 1\n}") else {
            panic!()
        };
        assert!(matches!(l.body[0], Member::Advance(_)));
    }

    #[test]
    fn a_loop_body_holds_nested_members() {
        let Member::Loop(l) = one("loop steps {\n let a = -k * x\n advance v = v + a * dt\n}")
        else {
            panic!()
        };
        assert_eq!(l.body.len(), 2);
        assert!(matches!(l.body[1], Member::Advance(_)));
    }

    #[test]
    fn both_watch_forms_parse() {
        let watched = |text: &str| match one(text) {
            Member::Loop(l) => match &l.body[0] {
                Member::Watch(w) => w.clone(),
                other => panic!("expected a watch, got {other:?}"),
            },
            other => panic!("expected a loop, got {other:?}"),
        };
        let w = watched("loop 4 {\n watch energy = 0.5 * k * x\n}");
        assert_eq!(w.name.as_deref(), Some("energy"));
        let w = watched("loop 4 {\n watch energy\n}");
        assert_eq!(w.name, None);
        assert!(matches!(w.value, Expr::Ident { .. }), "{:?}", w.value);
    }

    #[test]
    fn watch_beyond_the_end_of_a_loop_is_rejected() {
        let msgs = messages("model x \"\" {\n watch q\n}\n");
        assert!(
            msgs.iter()
                .any(|m| m.contains("only meaningful inside a loop")),
            "{msgs:?}"
        );
    }

    #[test]
    fn a_parameter_cannot_be_declared_inside_a_loop() {
        let msgs = messages("model x \"\" {\n loop 4 {\n input y in [0, 1]\n }\n}\n");
        assert!(
            msgs.iter()
                .any(|m| m.contains("cannot be declared inside a loop")),
            "{msgs:?}"
        );
    }

    #[test]
    fn each_relation_has_its_own_shape() {
        assert!(matches!(
            match one("check monotone_up(range wrt velocity)") {
                Member::Check(c) => c.relation,
                _ => unreachable!(),
            },
            RelationSyntax::Monotone {
                dir: MonotoneDir::Up,
                ..
            }
        ));
        assert!(matches!(
            match one("check scales_as(range wrt velocity, 2)") {
                Member::Check(c) => c.relation,
                _ => unreachable!(),
            },
            RelationSyntax::ScalesAs { .. }
        ));
        assert!(matches!(
            match one("check symmetric(apex wrt (angle, azimuth))") {
                Member::Check(c) => c.relation,
                _ => unreachable!(),
            },
            RelationSyntax::Symmetric { .. }
        ));
        assert!(matches!(
            match one("check conserved(energy, 0.01)") {
                Member::Check(c) => c.relation,
                _ => unreachable!(),
            },
            RelationSyntax::Conserved {
                tolerance: Some(_),
                ..
            }
        ));
        assert!(matches!(
            match one("check lipschitz(range wrt velocity, 400)") {
                Member::Check(c) => c.relation,
                _ => unreachable!(),
            },
            RelationSyntax::Lipschitz { .. }
        ));
    }

    #[test]
    fn an_unknown_relation_names_the_ones_that_exist() {
        let msgs = messages("model x \"\" {\n check wobbly(a wrt b)\n}\n");
        let joined = msgs.join(" | ");
        assert!(joined.contains("not a relation APORIA checks"), "{joined}");
        assert!(joined.contains("lipschitz"), "{joined}");
    }

    #[test]
    fn a_call_checks_arity_against_the_builtin_table() {
        let msgs = messages("model x \"\" {\n let y = sqrt(1, 2)\n}\n");
        assert!(
            msgs.iter().any(|m| m.contains("takes 1 argument")),
            "{msgs:?}"
        );
    }

    #[test]
    fn an_unknown_function_is_reported() {
        let msgs = messages("model x \"\" {\n let y = frobnicate(1)\n}\n");
        assert!(
            msgs.iter().any(|m| m.contains("not a function")),
            "{msgs:?}"
        );
    }

    #[test]
    fn a_triple_argument_builtin_is_accepted() {
        assert!(matches!(
            one("let y = clamp(x, 0, 1)"),
            Member::Let(LetDecl { .. })
        ));
    }

    #[test]
    fn an_unclosed_body_is_reported_at_the_model() {
        let msgs = messages("model x \"\" {\n input a in [0, 1]\n");
        assert!(msgs.iter().any(|m| m.contains("never closed")), "{msgs:?}");
    }

    #[test]
    fn several_broken_statements_yield_several_messages() {
        // Recovery is the point: one bad line must not hide the mistakes on the next three.
        let text = "model x \"\" {\n input ~ 1\n let = 2\n check wobbly(a)\n state q : m = \n}\n";
        let msgs = messages(text);
        assert!(msgs.len() >= 4, "{msgs:?}");
    }

    #[test]
    fn a_valid_statement_after_a_broken_one_is_still_kept() {
        // Recovery moves to the next line and no further: the good statement after the damage must
        // not be reported as a second problem.
        let msgs = messages(
            "model x \"\" {
 let = 2
input good : m in [0, 1]
}
",
        );
        assert!(
            msgs.iter().any(|m| m.contains("expected a name")),
            "{msgs:?}"
        );
        assert_eq!(
            msgs.len(),
            1,
            "the recovery ate a valid statement: {msgs:?}"
        );
    }

    #[test]
    fn semicolons_are_accepted_as_statement_ends() {
        assert_eq!(members("let a = 1;\nlet b = 2;\n").len(), 2);
    }

    #[test]
    fn a_string_after_a_statement_documents_it() {
        let Member::Input(d) = one("input mass : kg in [1, 10] \"the payload\"") else {
            panic!()
        };
        assert_eq!(d.doc.as_deref(), Some("the payload"));
    }

    #[test]
    fn spans_cover_the_whole_expression() {
        let src = Source::new("t.ap", "model x \"\" {\n let v = 2*a + b\n}\n");
        let mut d = Diagnostics::new();
        let toks = lex(&src, &mut d).unwrap();
        let m = parse(&toks, &mut d).unwrap();
        let Member::Let(l) = &m.members[0] else {
            panic!()
        };
        let text = src.span_text(l.value.span());
        assert_eq!(text.trim(), "2*a + b", "{text:?}");
    }
}
