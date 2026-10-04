//! Tokens produced by the lexer.

use crate::span::Span;
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keyword {
    Model,
    Input,
    State,
    Let,
    Advance,
    Loop,
    Watch,
    Require,
    Check,
    /// `input x : m in [0, 1]`
    In,
    /// `check monotone_up(range wrt velocity)`
    Wrt,
    And,
    Or,
    Not,
    True,
    False,
}

impl Keyword {
    #[must_use]
    pub fn from_word(word: &str) -> Option<Self> {
        Some(match word {
            "model" => Self::Model,
            "input" => Self::Input,
            "state" => Self::State,
            "let" => Self::Let,
            "advance" => Self::Advance,
            "loop" => Self::Loop,
            "watch" => Self::Watch,
            "require" => Self::Require,
            "check" => Self::Check,
            "in" => Self::In,
            "wrt" => Self::Wrt,
            "and" => Self::And,
            "or" => Self::Or,
            "not" => Self::Not,
            "true" => Self::True,
            "false" => Self::False,
            _ => return None,
        })
    }

    #[must_use]
    pub fn text(self) -> &'static str {
        match self {
            Self::Model => "model",
            Self::Input => "input",
            Self::State => "state",
            Self::Let => "let",
            Self::Advance => "advance",
            Self::Loop => "loop",
            Self::Watch => "watch",
            Self::Require => "require",
            Self::Check => "check",
            Self::In => "in",
            Self::Wrt => "wrt",
            Self::And => "and",
            Self::Or => "or",
            Self::Not => "not",
            Self::True => "true",
            Self::False => "false",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Punct {
    LBrace,
    RBrace,
    LParen,
    RParen,
    LBracket,
    RBracket,
    Comma,
    Colon,
    Semi,
    Equals,
    Plus,
    Minus,
    Star,
    Slash,
    Caret,
    Percent,
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
    /// Approximate equality, used by `require`.
    Approx,
}

impl Punct {
    #[must_use]
    pub fn text(self) -> &'static str {
        match self {
            Self::LBrace => "{",
            Self::RBrace => "}",
            Self::LParen => "(",
            Self::RParen => ")",
            Self::LBracket => "[",
            Self::RBracket => "]",
            Self::Comma => ",",
            Self::Colon => ":",
            Self::Semi => ";",
            Self::Equals => "=",
            Self::Plus => "+",
            Self::Minus => "-",
            Self::Star => "*",
            Self::Slash => "/",
            Self::Caret => "^",
            Self::Percent => "%",
            Self::Lt => "<",
            Self::Le => "<=",
            Self::Gt => ">",
            Self::Ge => ">=",
            Self::Eq => "==",
            Self::Ne => "!=",
            Self::Approx => "~",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum TokenKind {
    Ident(String),
    Keyword(Keyword),
    /// `int` records whether the literal was written without a point or exponent, which is what
    /// decides whether it may be used as a loop trip count.
    Number {
        value: f64,
        int: bool,
    },
    Str(String),
    Punct(Punct),
    /// One logical line break. Statements are terminated by newlines, not semicolons.
    Newline,
    Eof,
}

impl TokenKind {
    /// How to refer to this token in a parse error.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Ident(name) => format!("identifier `{name}`"),
            Self::Keyword(k) => format!("keyword `{}`", k.text()),
            Self::Number { value, int } => {
                if *int {
                    format!("integer {value}")
                } else {
                    format!("number {value}")
                }
            }
            Self::Str(_) => "string".to_string(),
            Self::Punct(p) => format!("`{}`", p.text()),
            Self::Newline => "end of line".to_string(),
            Self::Eof => "end of file".to_string(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

impl fmt::Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.kind.describe())
    }
}
