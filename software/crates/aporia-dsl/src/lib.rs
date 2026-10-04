//! # aporia-dsl
//!
//! The Aporia language: a small, readable way to state a scientific computation, the space it is
//! defined over, and what should be true about it.
//!
//! The pipeline is `lex -> parse -> check -> lower`, and each stage is a module that can be tested
//! on its own:
//!
//! - [`span`] — byte ranges, the source, and rendered diagnostics
//! - [`token`] / [`lexer`] — the character stream and its vocabulary
//!
//! Later stages add [`ast`], [`parser`], [`units`], [`check`] and [`lower`], which together turn a
//! `.ap` file into a verified A-IR `Model`.

pub mod ast;
pub mod builtins;
pub mod lexer;
pub mod parser;
pub mod span;
pub mod token;
pub mod units;
