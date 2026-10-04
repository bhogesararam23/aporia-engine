//! The lexer.
//!
//! Tokens carry byte spans so every later phase can point back at the source. Lexing collects all
//! of its own errors instead of stopping at the first: a mistyped quote or a stray character usually
//! comes in threes, and reporting them together is what makes a benchmark file editable.

use crate::span::{Diagnostic, Diagnostics, Source, Span};
use crate::token::{Keyword, Punct, Token, TokenKind};

/// Lex a whole source file, appending findings to `out`.
///
/// Returns the token stream unless something was recorded at error severity. Warnings travel the
/// same channel and never suppress the tokens.
pub fn lex(src: &Source, out: &mut Diagnostics) -> Option<Vec<Token>> {
    let mut lexer = Lexer {
        text: src.text(),
        pos: 0,
        tokens: Vec::new(),
        errors: Vec::new(),
    };
    lexer.run();
    let failed = lexer.errors.iter().any(Diagnostic::is_error);
    out.items.append(&mut lexer.errors);
    if failed { None } else { Some(lexer.tokens) }
}

struct Lexer<'a> {
    text: &'a str,
    pos: usize,
    tokens: Vec<Token>,
    errors: Vec<Diagnostic>,
}

impl Lexer<'_> {
    fn error(&mut self, span: Span, message: impl Into<String>) {
        self.errors.push(Diagnostic::error(message, span));
    }

    fn peek(&self, ahead: usize) -> Option<u8> {
        self.text.as_bytes().get(self.pos + ahead).copied()
    }

    /// The character starting at `offset`, assuming `offset` is a char boundary. Every place that
    /// advances `pos` moves by whole characters, so it always is.
    fn char_at(&self, offset: usize) -> Option<char> {
        self.text[offset..].chars().next()
    }

    fn run(&mut self) {
        let mut at_line_start = true;
        while self.pos < self.text.len() {
            if self.skip_trivia() {
                if !at_line_start {
                    self.tokens.push(Token {
                        kind: TokenKind::Newline,
                        span: Span::new(self.pos as u32, self.pos as u32),
                    });
                }
                at_line_start = true;
                continue;
            }
            at_line_start = false;
            if !self.one_token() {
                // The problem is recorded. Skip one character so a single stray byte does not
                // cascade into one error per remaining token.
                if let Some(ch) = self.char_at(self.pos) {
                    self.pos += ch.len_utf8();
                } else {
                    self.pos += 1;
                }
            }
        }
        self.tokens.push(Token {
            kind: TokenKind::Eof,
            span: Span::new(self.pos as u32, self.pos as u32),
        });
    }

    /// Consume whitespace and comments; true when a line boundary was crossed.
    fn skip_trivia(&mut self) -> bool {
        let mut crossed = false;
        loop {
            match self.peek(0) {
                Some(b'\n') => {
                    self.pos += 1;
                    crossed = true;
                }
                Some(b' ' | b'\t' | b'\r') => self.pos += 1,
                Some(b'#') => self.skip_line(),
                Some(b'/') if self.peek(1) == Some(b'/') => self.skip_line(),
                Some(b'/') if self.peek(1) == Some(b'*') => self.skip_block_comment(),
                _ => break,
            }
        }
        crossed
    }

    fn skip_line(&mut self) {
        while self.peek(0).is_some_and(|c| c != b'\n') {
            self.pos += 1;
        }
    }

    fn skip_block_comment(&mut self) {
        let start = self.pos;
        self.pos += 2;
        let mut depth = 1usize;
        while self.pos < self.text.len() && depth > 0 {
            match (self.peek(0), self.peek(1)) {
                (Some(b'/'), Some(b'*')) => {
                    depth += 1;
                    self.pos += 2;
                }
                (Some(b'*'), Some(b'/')) => {
                    depth -= 1;
                    self.pos += 2;
                }
                (Some(_), _) => self.pos += 1,
                (None, _) => break,
            }
        }
        if depth > 0 {
            self.error(
                Span::new(start as u32, self.pos as u32),
                "block comment is never closed",
            );
        }
    }

    fn one_token(&mut self) -> bool {
        let start = self.pos;
        let Some(c) = self.peek(0) else {
            return false;
        };
        match c {
            b'"' => self.string(start),
            b'0'..=b'9' => self.number(start),
            b'.' if self.peek(1).is_some_and(|d| d.is_ascii_digit()) => self.number(start),
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => self.word(start),
            other if !other.is_ascii() => {
                self.non_ascii_word(start);
                true
            }
            other => self.punct(start, other),
        }
    }

    /// A word that begins with a non-ASCII letter: a Greek symbol, a typographic operator, or an
    /// accented name. Naming the character is far more useful than reporting an invalid byte.
    fn non_ascii_word(&mut self, start: usize) {
        let ch = self.char_at(start).unwrap_or('\u{fffd}');
        self.errors.push(
            Diagnostic::error(
                format!("`{ch}` is not a legal character in an identifier"),
                Span::new(start as u32, (start + ch.len_utf8()) as u32),
            )
            .with_help("write the quantity in ASCII, for example `alpha` for the Greek letter"),
        );
        self.pos = start + ch.len_utf8();
        // Swallow whatever ASCII follows so the rest of the line still lexes sensibly.
        while self
            .peek(0)
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == b'_')
        {
            self.pos += 1;
        }
    }

    fn word(&mut self, start: usize) -> bool {
        while self
            .peek(0)
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == b'_')
        {
            self.pos += 1;
        }
        if self.peek(0).is_some_and(|c| !c.is_ascii()) {
            // Almost always a Greek letter or a typographic operator. Name the character: "invalid
            // byte 0xCE" tells a scientist nothing.
            let ch = self.char_at(self.pos).unwrap_or('\u{fffd}');
            self.errors.push(
                Diagnostic::error(
                    format!("`{ch}` is not a legal character in an identifier"),
                    Span::new(self.pos as u32, (self.pos + ch.len_utf8()) as u32),
                )
                .with_help("write the quantity in ASCII, for example `alpha` for the Greek letter"),
            );
            self.pos += ch.len_utf8();
            return true;
        }
        let text = self.text[start..self.pos].to_string();
        let span = Span::new(start as u32, self.pos as u32);
        if text.contains("__") || text.trim_start_matches('_').ends_with('_') {
            self.errors.push(
                Diagnostic::warning(
                    format!("identifier `{text}` uses an unusual underscore pattern"),
                    span,
                )
                .with_help("APORIA identifiers are words, optionally joined by one underscore"),
            );
        }
        let kind = match Keyword::from_word(&text) {
            Some(kw) => TokenKind::Keyword(kw),
            None => TokenKind::Ident(text),
        };
        self.tokens.push(Token { kind, span });
        true
    }

    fn number(&mut self, start: usize) -> bool {
        let mut int = true;
        self.take_digits();
        if self.peek(0) == Some(b'.')
            && self
                .peek(1)
                .is_some_and(|c| c.is_ascii_digit() || c == b'_')
        {
            int = false;
            self.pos += 1;
            self.take_digits();
        }
        if self.at_exponent() {
            int = false;
            self.pos += 1;
            if matches!(self.peek(0), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            self.take_digits();
        }
        let raw: String = self.text[start..self.pos]
            .chars()
            .filter(|c| *c != '_')
            .collect();
        let span = Span::new(start as u32, self.pos as u32);
        if let Ok(value) = raw.parse::<f64>() {
            self.tokens.push(Token {
                kind: TokenKind::Number { value, int },
                span,
            });
        } else {
            self.error(
                span,
                format!("`{raw}` is not a number APORIA can hold in an f64"),
            );
        }
        true
    }

    /// Is `pos` sitting on an exponent part, i.e. `e`/`E` followed by digits or a sign and digits?
    fn at_exponent(&self) -> bool {
        let Some(head) = self.peek(0) else {
            return false;
        };
        if head != b'e' && head != b'E' {
            return false;
        }
        let Some(next) = self.peek(1) else {
            return false;
        };
        // `e3`, `e+3` and `e-3` are exponents. A trailing `e` on its own is the constant.
        if next.is_ascii_digit() {
            return true;
        }
        (next == b'+' || next == b'-') && self.peek(2).is_some_and(|d| d.is_ascii_digit())
    }

    fn take_digits(&mut self) {
        while self
            .peek(0)
            .is_some_and(|c| c.is_ascii_digit() || c == b'_')
        {
            self.pos += 1;
        }
    }

    fn string(&mut self, start: usize) -> bool {
        self.pos += 1;
        let mut value = String::new();
        loop {
            match self.peek(0) {
                None => {
                    self.error(
                        Span::new(start as u32, self.pos as u32),
                        "string literal is never closed",
                    );
                    return true;
                }
                Some(b'\n') => {
                    self.error(
                        Span::new(start as u32, self.pos as u32),
                        "string literal ends at the line break",
                    );
                    return true;
                }
                Some(b'"') => {
                    self.pos += 1;
                    self.tokens.push(Token {
                        kind: TokenKind::Str(value),
                        span: Span::new(start as u32, self.pos as u32),
                    });
                    return true;
                }
                Some(b'\\') => {
                    self.pos += 1;
                    let at = self.pos;
                    match self.peek(0) {
                        Some(b'n') => value.push('\n'),
                        Some(b't') => value.push('\t'),
                        Some(b'"') => value.push('"'),
                        Some(b'\\') => value.push('\\'),
                        _ => {
                            self.error(
                                Span::new(at as u32, (at + 1) as u32),
                                "unknown escape in a string: only backslash-n, backslash-t, backslash-quote and backslash-backslash are recognised",
                            );
                            self.pos += 1;
                            continue;
                        }
                    }
                    self.pos += 1;
                }
                Some(_) => {
                    let ch = self.char_at(self.pos).unwrap();
                    value.push(ch);
                    self.pos += ch.len_utf8();
                }
            }
        }
    }

    fn punct(&mut self, start: usize, c: u8) -> bool {
        let next = self.peek(1);
        let (punct, width) = match (c, next) {
            (b'{', _) => (Punct::LBrace, 1),
            (b'}', _) => (Punct::RBrace, 1),
            (b'(', _) => (Punct::LParen, 1),
            (b')', _) => (Punct::RParen, 1),
            (b'[', _) => (Punct::LBracket, 1),
            (b']', _) => (Punct::RBracket, 1),
            (b',', _) => (Punct::Comma, 1),
            (b':', _) => (Punct::Colon, 1),
            (b';', _) => (Punct::Semi, 1),
            (b'+', _) => (Punct::Plus, 1),
            (b'-', Some(b'-')) => {
                self.error(
                    Span::new(start as u32, (start + 2) as u32),
                    "`--` is not an operator; a comment starts with `#` or `//`",
                );
                return false;
            }
            (b'-', _) => (Punct::Minus, 1),
            (b'*', _) => (Punct::Star, 1),
            (b'/', _) => (Punct::Slash, 1),
            (b'^', _) => (Punct::Caret, 1),
            (b'%', _) => (Punct::Percent, 1),
            (b'~', _) => (Punct::Approx, 1),
            (b'=', Some(b'=')) => (Punct::Eq, 2),
            (b'=', _) => (Punct::Equals, 1),
            (b'!', Some(b'=')) => (Punct::Ne, 2),
            (b'<', Some(b'=')) => (Punct::Le, 2),
            (b'<', _) => (Punct::Lt, 1),
            (b'>', Some(b'=')) => (Punct::Ge, 2),
            (b'>', _) => (Punct::Gt, 1),
            (b'!', _) => {
                self.error(
                    Span::new(start as u32, (start + 1) as u32),
                    "`!` only appears as part of `!=`; negation is `not`",
                );
                return false;
            }
            _ => {
                let ch = self.char_at(start).unwrap_or('\u{fffd}');
                self.error(
                    Span::new(start as u32, (start + ch.len_utf8()) as u32),
                    format!("unexpected character `{ch}`"),
                );
                return false;
            }
        };
        self.pos += width;
        self.tokens.push(Token {
            kind: TokenKind::Punct(punct),
            span: Span::new(start as u32, self.pos as u32),
        });
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str) -> Vec<TokenKind> {
        let src = Source::new("t.ap", text);
        let mut d = Diagnostics::new();
        lex(&src, &mut d)
            .unwrap_or_else(|| panic!("expected {text:?} to lex, got: {d}"))
            .into_iter()
            .map(|t| t.kind)
            .collect()
    }

    /// Lex expecting failure, and return the first error-severity message.
    fn first_error(text: &str) -> String {
        let src = Source::new("t.ap", text);
        let mut d = Diagnostics::new();
        assert!(lex(&src, &mut d).is_none(), "expected {text:?} to fail");
        d.items
            .iter()
            .find(|x| x.is_error())
            .unwrap_or_else(|| panic!("expected an error severity from {text:?}: {d}"))
            .message
            .clone()
    }

    #[test]
    fn keywords_and_identifiers_are_told_apart() {
        let ks = kinds("input velocity\n");
        assert_eq!(ks[0], TokenKind::Keyword(Keyword::Input));
        assert_eq!(ks[1], TokenKind::Ident("velocity".into()));
        assert_eq!(ks[2], TokenKind::Newline);
        assert_eq!(ks[3], TokenKind::Eof);
    }

    #[test]
    fn numbers_carry_their_written_form() {
        let ks = kinds("1 1.0 1e3 1_000 .5\n");
        let ints: Vec<bool> = ks
            .iter()
            .filter_map(|k| match k {
                TokenKind::Number { int, .. } => Some(*int),
                _ => None,
            })
            .collect();
        assert_eq!(ints, vec![true, false, false, true, false]);
    }

    #[test]
    fn digit_separators_are_stripped_but_do_not_change_the_value() {
        assert_eq!(
            kinds("1_000_000\n")[0],
            TokenKind::Number {
                value: 1e6,
                int: true
            }
        );
    }

    #[test]
    fn two_character_operators_win_over_one() {
        let ks = kinds("a <= b >= c == d != e ~ f\n");
        for p in [Punct::Le, Punct::Ge, Punct::Eq, Punct::Ne, Punct::Approx] {
            assert!(ks.contains(&TokenKind::Punct(p)), "missing {p:?}");
        }
    }

    #[test]
    fn every_operator_the_grammar_needs_is_a_token() {
        let ks = kinds("- + * / ^ % ( ) [ ] { } , : = < > <= >= == != ~\n");
        let puncts: Vec<Punct> = ks
            .iter()
            .filter_map(|k| match k {
                TokenKind::Punct(p) => Some(*p),
                _ => None,
            })
            .collect();
        assert_eq!(puncts.len(), 22, "{puncts:?}");
        assert!(puncts.contains(&Punct::Minus));
    }

    #[test]
    fn units_lex_as_ordinary_operator_sequences() {
        // `m/s^2` needs no special unit token: the parser reads it as an expression and the checker
        // evaluates it in the unit algebra.
        let ks = kinds("m/s^2\n");
        let puncts: Vec<Punct> = ks
            .iter()
            .filter_map(|k| match k {
                TokenKind::Punct(p) => Some(*p),
                _ => None,
            })
            .collect();
        assert_eq!(puncts, vec![Punct::Slash, Punct::Caret]);
    }

    #[test]
    fn both_comment_styles_stop_at_the_line_end() {
        assert_eq!(kinds("let x = 1 # note\n"), kinds("let x = 1 // note\n"));
        assert_eq!(kinds("a /* gone */ b\n").len(), 4);
    }

    #[test]
    fn block_comments_nest() {
        let ks = kinds("a /* x /* y */ z */ b\n");
        assert_eq!(ks.len(), 4, "ident, ident, newline, eof");
    }

    #[test]
    fn several_blank_lines_are_one_statement_break() {
        let newlines = kinds("a\n\n\nb\n")
            .iter()
            .filter(|k| **k == TokenKind::Newline)
            .count();
        assert_eq!(newlines, 2);
    }

    #[test]
    fn a_leading_blank_line_does_not_make_an_empty_statement() {
        let newlines = kinds("\n\na\n")
            .iter()
            .filter(|k| **k == TokenKind::Newline)
            .count();
        assert_eq!(newlines, 1);
    }

    #[test]
    fn unterminated_string_is_reported_not_panicked() {
        assert!(first_error("model \"oops\n").contains("string literal"));
    }

    #[test]
    fn unclosed_block_comment_is_reported() {
        assert!(first_error("a /* never ends").contains("block comment"));
    }

    #[test]
    fn greek_letters_get_a_specific_message() {
        let msg = first_error("let \u{03b1} = 1\n");
        assert!(msg.contains("not a legal character"), "{msg}");
    }

    #[test]
    fn double_dash_suggests_a_comment() {
        assert!(first_error("a -- b").contains("comment"));
    }

    #[test]
    fn a_bare_bang_explains_itself() {
        assert!(first_error("a ! b").contains("negation is `not`"));
    }

    #[test]
    fn spans_point_at_the_exact_token() {
        let src = Source::new("t.ap", "input gravity : m/s^2\n");
        let mut d = Diagnostics::new();
        let toks = lex(&src, &mut d).unwrap();
        let gravity = toks
            .iter()
            .find(|t| t.kind == TokenKind::Ident("gravity".into()))
            .unwrap();
        assert_eq!(
            &src.text()[gravity.span.start as usize..gravity.span.end as usize],
            "gravity"
        );
    }

    #[test]
    fn escapes_in_strings_are_understood() {
        let strs: Vec<String> = kinds("\"a\\nb\" \"c\\\"d\" \"e\\\\f\"\n")
            .iter()
            .filter_map(|k| match k {
                TokenKind::Str(s) => Some(s.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            strs,
            vec!["a\nb".to_string(), "c\"d".to_string(), "e\\f".to_string()]
        );
    }

    #[test]
    fn several_errors_on_a_line_are_all_reported() {
        let src = Source::new("t.ap", "let a = 1 ? ? ?\n");
        let mut d = Diagnostics::new();
        assert!(lex(&src, &mut d).is_none());
        assert_eq!(d.error_count(), 3, "{:?}", d.items);
    }

    #[test]
    fn a_non_ascii_string_body_is_kept_not_rejected() {
        // Model documentation may legitimately contain any character.
        let ks = kinds("\"energy \u{03b1} drift\"\n");
        assert!(matches!(&ks[0], TokenKind::Str(s) if s == "energy \u{03b1} drift"));
    }
}

#[test]
fn a_warning_does_not_lose_the_tokens() {
    let src = Source::new("t.ap", "let a__b = 1\n");
    let mut d = Diagnostics::new();
    let toks = lex(&src, &mut d).expect("a warning is not an error");
    assert_eq!(d.warning_count(), 1, "{:?}", d.items);
    assert!(toks.iter().any(|t| t.kind.describe().contains("a__b")));
}
