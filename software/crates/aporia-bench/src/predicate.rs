//! The truth language for regions that are not boxes.
//!
//! `aporia.truth/1` can declare only a union of axis-aligned boxes, which is exactly the shape the
//! atlas partitions along — so a corpus of boxes measures the instrument on its home ground rather
//! than at the edge of its reach (0029, and the frozen protocol in
//! `benchmarks/protocols/e1-geometry.json`). This module adds the smallest thing that makes a curved
//! region *declerable*: a boolean arithmetic expression over the model's own parameter names, carved
//! out of a required box envelope.
//!
//! Two properties are load-bearing, and together they are why this is a separate language rather
//! than the DSL's own expression syntax.
//!
//! - **It is an oracle, so it stays independent of the instrument.** Nothing here reads the atlas,
//!   the calibration or the fusion pipeline. Ground truth that was produced by the machinery it
//!   scores would make a detection rate measure the pipeline agreeing with itself, which is the
//!   distinction `corpus::violates` is built on.
//! - **It is small and total.** No function calls, no exponent operator, no units, no conditionals.
//!   Every operator is one the runtime also implements on `f64`, so the only arithmetic question left
//!   is whether the two evaluators agree — and `cross_check_with_the_runtime` answers that over a
//!   grid instead of asserting it. A `where` that could call `sqrt` or `pow` would have to agree with
//!   the frontend's builtin table, which is a much larger surface to keep honest.
//!
//! Division follows IEEE-754 as the rest of the project does: `1/0` is infinity and `0/0` is NaN,
//! and a comparison with a NaN operand is false. That is not a silent answer —
//! [`Predicate::undefined`] reports it, and a region whose predicate is undefined anywhere inside
//! its envelope is refused by the corpus audit rather than measured as an empty set.

/// One node of a parsed `where` expression.
#[derive(Clone, Debug, PartialEq)]
enum Node {
    Const(f64),
    /// A variable, addressed by its position in [`Predicate::vars`], so evaluation needs no lookup
    /// by name and this module never needs to know what a model is.
    Var(usize),
    Add(Box<(Node, Node)>),
    Sub(Box<(Node, Node)>),
    Mul(Box<(Node, Node)>),
    Div(Box<(Node, Node)>),
    /// `left op right`, with [`Cmp`] naming which. The only boolean leaves of the tree.
    Cmp(Cmp, Box<(Node, Node)>),
    And(Box<(Node, Node)>),
    Or(Box<(Node, Node)>),
    Not(Box<Node>),
}

/// The comparison vocabulary. Six, because the DSL has six and no more is needed to write a region.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cmp {
    Gt,
    Ge,
    Lt,
    Le,
    Eq,
    Ne,
}

impl Cmp {
    fn holds(self, a: f64, b: f64) -> bool {
        match self {
            // Comparisons with a NaN operand are false for every operator *including* `!=`, which is
            // the IEEE relation rather than Rust's: `NaN != NaN` coming back true would make an
            // undefined point look like it was inside the region.
            Self::Gt => a > b,
            Self::Ge => a >= b,
            Self::Lt => a < b,
            Self::Le => a <= b,
            Self::Eq => a == b,
            Self::Ne => !a.is_nan() && !b.is_nan() && a != b,
        }
    }

    fn token(self) -> Tok<'static> {
        match self {
            Self::Ge => Tok::Ge,
            Self::Le => Tok::Le,
            Self::Ne => Tok::Ne,
            Self::Eq => Tok::Eq,
            Self::Gt => Tok::Gt,
            Self::Lt => Tok::Lt,
        }
    }
}

/// A parsed region predicate: an expression over named variables, with those names listed.
#[derive(Clone, Debug, PartialEq)]
pub struct Predicate {
    node: Node,
    /// Every variable the expression names, in first-seen order. [`Predicate::holds`] takes one
    /// value per entry in this order, and the corpus audit checks the names against the model's
    /// parameters — which is where a typo like `pp + q > 1` is refused rather than quietly becoming
    /// a region no point can be inside. A truth file is read before a model exists, so the names
    /// cannot be resolved at parse time; they can still be *listed*, which is what makes that check
    /// possible at all.
    vars: Vec<String>,
    /// The text it was parsed from, kept for reports and refusals.
    source: String,
}

impl Predicate {
    /// Parse a `where` expression. Names are collected, not resolved — see [`Predicate::vars`] for
    /// why a truth file cannot be checked against a model at the moment it is read, and where that
    /// check happens instead.
    pub fn parse(text: &str) -> Result<Self, String> {
        let tokens = lex(text).map_err(|e| format!("region predicate {e}"))?;
        let mut vars = Vec::new();
        let mut parser = Parser {
            tokens,
            at: 0,
            vars: &mut vars,
        };
        let node = parser
            .or()
            .map_err(|e| format!("region predicate {text:?}: {e}"))?;
        if !parser.at_end() {
            return Err(format!(
                "region predicate {text:?}: trailing input at {:?}",
                parser.current()
            ));
        }
        Ok(Self {
            node,
            vars,
            source: text.to_string(),
        })
    }

    /// The names this expression uses, in first-seen order: one per value that [`Self::holds`] and
    /// [`Self::undefined`] expect, and the list an audit checks against the model's parameters.
    #[must_use]
    pub fn vars(&self) -> &[String] {
        &self.vars
    }

    /// The text the region's owner wrote, for reports and refusals.
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Does the point satisfy the predicate? `values` is one `f64` per entry of [`Self::vars`], in
    /// that order. An undefined point answers `false`; ask [`Self::undefined`] when the difference
    /// matters, which the corpus audit does.
    #[must_use]
    pub fn holds(&self, values: &[f64]) -> bool {
        eval(&self.node, values)
    }

    /// Could the predicate be evaluated at this point at all? True when some arithmetic it performs
    /// produces NaN, which a comparison would otherwise answer "false" to — turning a region the
    /// declaration cannot describe into one that looks empty.
    #[must_use]
    pub fn undefined(&self, values: &[f64]) -> bool {
        !defined(&self.node, values)
    }
}

/// The numeric value of an arithmetic node. `None` when the node is a boolean, which the parser
/// never asks for.
fn value(node: &Node, x: &[f64]) -> Option<f64> {
    match node {
        Node::Const(v) => Some(*v),
        Node::Var(slot) => x.get(*slot).copied(),
        Node::Add(p) => Some(value(&p.0, x)? + value(&p.1, x)?),
        Node::Sub(p) => Some(value(&p.0, x)? - value(&p.1, x)?),
        Node::Mul(p) => Some(value(&p.0, x)? * value(&p.1, x)?),
        // IEEE-754 division, exactly as the runtime does it: `x/0` is infinity, `0/0` is NaN.
        Node::Div(p) => Some(value(&p.0, x)? / value(&p.1, x)?),
        Node::Cmp(_, _) | Node::And(_) | Node::Or(_) | Node::Not(_) => None,
    }
}

/// The whole predicate's boolean answer at a point.
fn eval(node: &Node, x: &[f64]) -> bool {
    match node {
        Node::Cmp(op, p) => value(&p.0, x)
            .and_then(|a| value(&p.1, x).map(|b| op.holds(a, b)))
            .unwrap_or(false),
        Node::And(p) => eval(&p.0, x) && eval(&p.1, x),
        Node::Or(p) => eval(&p.0, x) || eval(&p.1, x),
        Node::Not(a) => !eval(a, x),
        // A root that computes a number rather than a comparison is refused by the parser, so
        // reaching here means the parser and this function disagree about the grammar.
        Node::Const(_)
        | Node::Var(_)
        | Node::Add(_)
        | Node::Sub(_)
        | Node::Mul(_)
        | Node::Div(_) => false,
    }
}

/// Every number the node computes at this point, real or NaN — walking both sides of a conjunction
/// rather than short-circuiting, because a side that is skipped is a NaN the audit would never see.
fn defined(node: &Node, x: &[f64]) -> bool {
    let numeric = |n: &Node| value(n, x).is_some_and(|v| !v.is_nan());
    match node {
        Node::Const(v) => !v.is_nan(),
        Node::Var(slot) => x.get(*slot).copied().is_some_and(|v| !v.is_nan()),
        Node::Add(p) | Node::Sub(p) | Node::Mul(p) | Node::Div(p) => {
            defined(&p.0, x) && defined(&p.1, x) && numeric(node)
        }
        // A boolean node is defined wherever both of its sides are; it produces no number of its own.
        Node::Cmp(_, p) | Node::And(p) | Node::Or(p) => defined(&p.0, x) && defined(&p.1, x),
        Node::Not(a) => defined(a, x),
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Tok<'a> {
    Num(f64),
    Ident(&'a str),
    Plus,
    Minus,
    Star,
    Slash,
    Gt,
    Ge,
    Lt,
    Le,
    Eq,
    Ne,
    And,
    Or,
    Not,
    Open,
    Close,
}

/// Tokenise the restricted grammar. Anything outside it is refused with the byte offset, because a
/// truth file that silently dropped a character would declare a different region than its author
/// wrote.
fn lex(text: &str) -> Result<Vec<Tok<'_>>, String> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at < text.len() {
        let rest = &text[at..];
        let first = rest
            .chars()
            .next()
            .ok_or_else(|| "ran out mid-expression".to_string())?;
        if first.is_whitespace() {
            at += first.len_utf8();
            continue;
        }
        // Two-character operators first, so `>=` never lexes as `>` with an `=` nothing eats.
        let mut two = None;
        for (symbol, tok) in [
            (">=", Tok::Ge),
            ("<=", Tok::Le),
            ("==", Tok::Eq),
            ("!=", Tok::Ne),
        ] {
            if rest.starts_with(symbol) {
                two = Some((symbol, tok));
                break;
            }
        }
        if let Some((symbol, tok)) = two {
            out.push(tok);
            at += symbol.len();
            continue;
        }
        if let Some(tok) = match first {
            '>' => Some(Tok::Gt),
            '<' => Some(Tok::Lt),
            '+' => Some(Tok::Plus),
            '-' => Some(Tok::Minus),
            '*' => Some(Tok::Star),
            '/' => Some(Tok::Slash),
            '(' => Some(Tok::Open),
            ')' => Some(Tok::Close),
            _ => None,
        } {
            out.push(tok);
            at += first.len_utf8();
            continue;
        }
        // Everything else is a number or a name, taken as one maximal run.
        if first.is_ascii_digit() || first == '.' {
            let len = number_len(rest);
            let text = &rest[..len];
            let Ok(v) = text.parse::<f64>() else {
                return Err(format!("{text:?} is not a number (at byte {at})"));
            };
            out.push(Tok::Num(v));
            at += len;
            continue;
        }
        let end = rest
            .find(|c: char| !(c.is_alphanumeric() || c == '_'))
            .unwrap_or(rest.len());
        let word = &rest[..end];
        if word == "and" {
            out.push(Tok::And);
        } else if word == "or" {
            out.push(Tok::Or);
        } else if word == "not" {
            out.push(Tok::Not);
        } else if word
            .chars()
            .next()
            .is_some_and(|c| c.is_alphabetic() || c == '_')
        {
            out.push(Tok::Ident(word));
        } else {
            return Err(format!(
                "{word:?} is not a word this language has (at byte {at})"
            ));
        }
        at += end;
    }
    Ok(out)
}

/// How much of `rest` is one number: digits and at most one dot, then an exponent if a valid one
/// follows. Written out rather than grabbed greedily, because `0.5+2` has to lex as three tokens
/// and `1e` is not a number at all.
fn number_len(rest: &str) -> usize {
    let b = rest.as_bytes();
    let mut i = 0;
    let mut dots = 0;
    while i < b.len() && (b[i].is_ascii_digit() || (b[i] == b'.' && dots < 1)) {
        if b[i] == b'.' {
            dots += 1;
        }
        i += 1;
    }
    if i < b.len() && (b[i] | 0x20 == b'e') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        let digits_at = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        // Only a complete exponent is part of the number; `1e` on its own stays `1` and the `e` is
        // reported as the unknown word it is.
        if j > digits_at {
            i = j;
        }
    }
    i
}

/// Recursive descent over the frozen precedence: `or` loosest, then `and`, then `not`, then
/// comparisons, then `+ -`, then `* /`, then atoms.
struct Parser<'a> {
    tokens: Vec<Tok<'a>>,
    at: usize,
    vars: &'a mut Vec<String>,
}

impl<'a> Parser<'a> {
    fn at_end(&self) -> bool {
        self.at >= self.tokens.len()
    }

    fn current(&self) -> Option<Tok<'a>> {
        self.tokens.get(self.at).copied()
    }

    fn eat(&mut self, tok: Tok<'a>) -> bool {
        if self.current() == Some(tok) {
            self.at += 1;
            return true;
        }
        false
    }

    fn or(&mut self) -> Result<Node, String> {
        let mut left = self.and()?;
        while self.eat(Tok::Or) {
            left = Node::Or(Box::new((left, self.and()?)));
        }
        Ok(left)
    }

    fn and(&mut self) -> Result<Node, String> {
        let mut left = self.not()?;
        while self.eat(Tok::And) {
            left = Node::And(Box::new((left, self.not()?)));
        }
        Ok(left)
    }

    fn not(&mut self) -> Result<Node, String> {
        if self.eat(Tok::Not) {
            return Ok(Node::Not(Box::new(self.not()?)));
        }
        self.boolean_atom()
    }

    /// A boolean is either a comparison or a parenthesised boolean. Parentheses around *arithmetic*
    /// are the other production, [`Parser::primary`], and the two are told apart by looking into the
    /// group: `(p + q) * 2 > 1` and `not (p > 1)` both open with a parenthesis and mean very
    /// different things.
    fn boolean_atom(&mut self) -> Result<Node, String> {
        if self.current() == Some(Tok::Open) && self.group_is_boolean() {
            self.at += 1;
            let inner = self.or()?;
            if !self.eat(Tok::Close) {
                return Err("an unclosed parenthesis around a comparison".to_string());
            }
            return Ok(inner);
        }
        self.cmp()
    }

    /// Does the parenthesised group starting here hold a boolean at its own top level? Scanned
    /// rather than parsed, so the answer costs a lookahead and the grammar keeps both productions.
    fn group_is_boolean(&self) -> bool {
        let mut depth = 0usize;
        for tok in &self.tokens[self.at..] {
            match tok {
                Tok::Open => depth += 1,
                Tok::Close => {
                    if depth <= 1 {
                        return false;
                    }
                    depth -= 1;
                }
                Tok::Gt
                | Tok::Ge
                | Tok::Lt
                | Tok::Le
                | Tok::Eq
                | Tok::Ne
                | Tok::And
                | Tok::Or
                | Tok::Not
                    if depth == 1 =>
                {
                    return true;
                }
                _ => {}
            }
        }
        false
    }

    /// A comparison is the only leaf that makes a boolean out of numbers, so an expression that ends
    /// here without one is refused: `p + q` computes a number and declares no region.
    fn cmp(&mut self) -> Result<Node, String> {
        let left = self.additive()?;
        for op in [Cmp::Ge, Cmp::Le, Cmp::Ne, Cmp::Eq, Cmp::Gt, Cmp::Lt] {
            if self.eat(op.token()) {
                let right = self.additive()?;
                return Ok(Node::Cmp(op, Box::new((left, right))));
            }
        }
        Err(
            "a region predicate must be a comparison; expected `>`, `>=`, `<`, `<=`, `==` or `!=`"
                .to_string(),
        )
    }

    fn additive(&mut self) -> Result<Node, String> {
        let mut left = self.multiplicative()?;
        loop {
            if self.eat(Tok::Plus) {
                left = Node::Add(Box::new((left, self.multiplicative()?)));
            } else if self.eat(Tok::Minus) {
                left = Node::Sub(Box::new((left, self.multiplicative()?)));
            } else {
                break;
            }
        }
        Ok(left)
    }

    fn multiplicative(&mut self) -> Result<Node, String> {
        let mut left = self.primary()?;
        loop {
            if self.eat(Tok::Star) {
                left = Node::Mul(Box::new((left, self.primary()?)));
            } else if self.eat(Tok::Slash) {
                left = Node::Div(Box::new((left, self.primary()?)));
            } else {
                break;
            }
        }
        Ok(left)
    }

    fn primary(&mut self) -> Result<Node, String> {
        match self.current() {
            Some(Tok::Num(v)) => {
                self.at += 1;
                Ok(Node::Const(v))
            }
            // Unary minus, written as `0 -` so no separate node is needed for it.
            Some(Tok::Minus) => {
                self.at += 1;
                Ok(Node::Sub(Box::new((Node::Const(0.0), self.primary()?))))
            }
            Some(Tok::Open) => {
                self.at += 1;
                // Arithmetic grouping only. A boolean group is read by [`Parser::boolean_atom`], and
                // letting this level take one would accept `p > (q > 1)`, which compares a number to
                // a yes/no answer.
                let inner = self.additive()?;
                if !self.eat(Tok::Close) {
                    return Err("an unclosed parenthesis around an arithmetic group".to_string());
                }
                Ok(inner)
            }
            Some(Tok::Ident(name)) => {
                self.at += 1;
                let slot = if let Some(at) = self.vars.iter().position(|seen| seen == name) {
                    at
                } else {
                    self.vars.push(name.to_string());
                    self.vars.len() - 1
                };
                Ok(Node::Var(slot))
            }
            other => Err(format!(
                "expected a number, a parameter name or `(`, found {other:?}"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Predicate {
        Predicate::parse(text).unwrap_or_else(|e| panic!("{e}"))
    }

    /// Evaluate with `p` first and `q` second — the order the fixtures name them in, which is the
    /// order [`Predicate::vars`] reports.
    fn holds(text: &str, values: &[f64]) -> bool {
        parse(text).holds(values)
    }

    #[test]
    fn the_four_shape_classes_parse_and_decide() {
        // A diagonal half-plane, which no axis-aligned box describes.
        assert!(holds("p + q > 1", &[0.8, 0.4]));
        assert!(!holds("p + q > 1", &[0.4, 0.4]));
        // A hyperbola, curved in both axes.
        assert!(holds("p * q > 2", &[4.0, 1.0]));
        assert!(!holds("p * q > 2", &[1.0, 1.0]));
        // A circle, written multiplicatively because there is no square-root call.
        let ring = "(p - 0.5) * (p - 0.5) + (q - 0.5) * (q - 0.5) > 0.25";
        assert!(holds(ring, &[1.5, 1.5]));
        assert!(!holds(ring, &[0.5, 0.5]));
        // A narrow oblique strip, the shape whose volume a search has to find.
        let strip = "(p - q - 0.5) * (p - q - 0.5) < 0.0004";
        assert!(holds(strip, &[1.0, 0.5]));
        assert!(!holds(strip, &[1.0, 0.4]));
    }

    #[test]
    fn precedence_and_grouping_are_the_ones_the_grammar_promises() {
        assert!(holds("1 + 2 * 3 > 6", &[0.0, 0.0]));
        assert!(!holds("(1 + 2) * 3 > 9", &[0.0, 0.0]));
        // `not` binds tighter than `and`, which binds tighter than `or`. The same words grouped
        // differently answer differently, which is why the parenthesis production exists at the
        // boolean level as well as the arithmetic one.
        assert!(holds("not 1 > 2 and 3 > 2", &[0.0, 0.0]));
        assert!(!holds("not 1 > 2 and 3 > 4", &[0.0, 0.0]));
        assert!(holds("not (1 > 2 and 3 > 4)", &[0.0, 0.0]));
        // `false or (true and false)` is false; the same words with the `or` branch true are true.
        assert!(!holds("1 > 2 or 2 > 1 and 0 > 1", &[0.0, 0.0]));
        assert!(holds("1 > 0 or 2 > 1 and 0 > 1", &[0.0, 0.0]));
        assert!(!holds("2 > 1 and 0 > 1 or 1 > 2", &[0.0, 0.0]));
    }

    #[test]
    fn comparisons_cover_the_six_the_dsl_has() {
        assert!(holds("p >= 1", &[1.0, 0.0]) && holds("p <= 1", &[1.0, 0.0]));
        assert!(!holds("p > 1", &[1.0, 0.0]) && !holds("p < 1", &[1.0, 0.0]));
        assert!(holds("p == q", &[1.0, 1.0]) && !holds("p == q", &[1.0, 2.0]));
        assert!(holds("p != q", &[1.0, 2.0]));
    }

    #[test]
    fn values_are_read_in_first_seen_name_order() {
        // `holds` takes one value per entry of `vars`, so the two have to agree on what "first" means
        // or a region would be evaluated with its axes swapped — a wrong answer that looks plausible
        // on a symmetric expression and is invisible on an asymmetric one.
        let p = parse("q > p");
        assert_eq!(p.vars(), &["q".to_string(), "p".to_string()]);
        assert!(p.holds(&[2.0, 1.0]));
        assert!(!p.holds(&[1.0, 2.0]));
        let reused = parse("p + q > 1 and p < 2");
        assert_eq!(
            reused.vars(),
            &["p".to_string(), "q".to_string()],
            "a name repeated is still one slot"
        );
    }

    #[test]
    fn a_predicate_lists_every_name_it_uses_so_the_auditor_can_refuse_a_typo() {
        // A truth file is read before a model exists, so `pp + q > 1` cannot be refused here. What
        // makes the refusal possible at all is that every name it uses is listed, which is what
        // `corpus::verify` checks against the model's parameters.
        let p = parse("pp + q > 1 and pp < 3");
        assert_eq!(p.vars(), &["pp".to_string(), "q".to_string()]);
    }

    #[test]
    fn the_language_has_no_functions_no_exponent_and_no_units() {
        for text in [
            "sqrt(p) > 1",
            "p ^ 2 > 1",
            "pow(p, 2) > 1",
            "p > 1 km",
            "abs(p - q) < 0.1",
            "p > 1; q < 2",
            "if p > 1 then q > 2",
        ] {
            let error = Predicate::parse(text).expect_err("{text:?} must not parse");
            assert!(!error.is_empty(), "{text:?} produced an empty refusal");
        }
    }

    #[test]
    fn an_expression_that_computes_a_number_declares_nothing() {
        // Refused rather than read as "always false" or "always true".
        let error = Predicate::parse("p + q").expect_err("a number is not a region");
        assert!(error.contains("comparison"), "{error}");
    }

    #[test]
    fn a_division_that_cannot_be_answered_is_reported_rather_than_read_as_outside() {
        let pole = parse("1 / (p - 1) > 1000");
        // One hundredth from the pole is only 100, which is genuinely not above 1000.
        assert!(!pole.holds(&[1.01]));
        assert!(pole.holds(&[1.0001]));
        assert!(!pole.holds(&[2.0]));
        // At the pole `1/0` is infinity, which is above 1000: a real answer about the set, not a
        // failure to answer.
        assert!(!pole.undefined(&[1.0]));
        assert!(pole.holds(&[1.0]));
        // `0/0` is not an answer, and a region built on it must not be scored as an empty one.
        let nan = parse("0 / (p - 1) > 1000");
        assert!(nan.undefined(&[1.0]));
        assert!(!nan.holds(&[1.0]));
        // Both sides of a conjunction are walked, so a NaN that short-circuiting would skip is seen.
        let both = parse("p == 1 and 0 / (p - 1) > 1");
        assert!(both.undefined(&[1.0]));
    }

    #[test]
    fn malformed_text_is_refused_with_the_problem_named() {
        for text in [
            "p +",
            "p > ",
            "(p > 1",
            "p > 1)",
            "p q > 1",
            "p > 1 and",
            "",
            "p > 1 % 2",
            "1e",
        ] {
            let error = Predicate::parse(text).expect_err("{text:?} must be refused");
            assert!(!error.is_empty(), "{text:?} was accepted");
        }
    }

    #[test]
    fn a_number_with_an_exponent_lexes_as_one_number() {
        assert!(holds("p > 1e-3", &[0.002]));
        assert!(!holds("p > 1e-3", &[0.0001]));
        assert!(holds("p > 2.5e2", &[300.0]));
    }

    #[test]
    fn cross_check_with_the_runtime() {
        // The oracle and the instrument must not disagree about arithmetic; if they did, a boundary
        // disagreement would be a property of two evaluators rather than a fact about the model. Each
        // expression is compiled into a real model whose declared rule is the negation of the
        // comparison, run by the interpreter over a grid, and this module's answer must match its at
        // every point.
        use crate::corpus::violates;
        use aporia_dsl::lower::compile;
        let lefts = [
            "p + q",
            "p * q",
            "(p - 0.5) * (p - 0.5) + (q - 0.5) * (q - 0.5)",
            "p / (q + 1.0)",
            "(p + q) * (p - q)",
            "p - q - 0.5",
        ];
        let threshold = 1.5;
        for left in lefts {
            let source = format!(
                "model probe \"\" {{\n  input p in [0, 4]\n  input q in [0, 4]\n  let a = {left}\n  \
                 require a <= {threshold}\n}}\n"
            );
            let compiled = compile("probe.ap", &source);
            assert!(
                !compiled.diagnostics.has_errors(),
                "the cross-check model for {left} did not compile: {}\n{source}",
                compiled.diagnostics
            );
            let model = compiled.model;
            let predicate = parse(&format!("{left} > {threshold}"));
            assert_eq!(predicate.vars(), &["p".to_string(), "q".to_string()]);
            let steps = 17;
            let mut checked = 0;
            for i in 0..steps {
                for j in 0..steps {
                    let p = 4.0 * i as f64 / (steps - 1) as f64;
                    let q = 4.0 * j as f64 / (steps - 1) as f64;
                    assert!(
                        !predicate.undefined(&[p, q]),
                        "{left} is undefined at p={p} q={q}"
                    );
                    assert_eq!(
                        predicate.holds(&[p, q]),
                        violates(&model, &[p, q]),
                        "{left} > {threshold} at p={p} q={q}: the predicate and the runtime \
                         disagreed"
                    );
                    checked += 1;
                }
            }
            assert_eq!(
                checked,
                steps * steps,
                "every fixture was crossed over the grid"
            );
        }
    }

    #[test]
    fn a_diagonal_region_splits_the_domain_the_way_the_shape_says() {
        // The negative side of the cross-check, and the reason the corpus needs this at all: a region
        // defined by `p + q > 1` covers a share of a square that no axis-aligned box can, so the
        // count of lattice points inside it is a fact about the shape rather than about the atlas.
        // The lattice is 21x21 over [0,2]^2, so `p + q > 1` is exactly the points with i + j > 10:
        // 441 minus the 66 on or below the diagonal. Pinned rather than bounded, because a parser
        // that grouped the sum wrongly would land on a different count and say nothing.
        let predicate = parse("p + q > 1");
        let mut inside = 0;
        for i in 0..21 {
            for j in 0..21 {
                let p = 2.0 * i as f64 / 20.0;
                let q = 2.0 * j as f64 / 20.0;
                inside += u64::from(predicate.holds(&[p, q]));
            }
        }
        assert_eq!(inside, 441 - 66, "diagonal inside count");
    }
}
