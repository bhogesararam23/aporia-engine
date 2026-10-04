//! # aporia-minimize
//!
//! Counterexample minimisation: take a failure and remove everything the failure does not need.
//!
//! - [`case`] — what a minimised failure *is*: a description with three states per parameter
//!   (pinned, interval, dropped), plus the sample points that make each state's claim
//! - [`oracle`] — the predicate being preserved, kept outside the algorithm on purpose
//! - [`minimize`] — ddmin over parameters, interval bisection per surviving axis, significant-digit
//!   reduction, run in that order and re-verified at every accepted step
//!
//! The spec's version of this (§11) is an eleven-parameter failure reduced to three parameters and
//! one runtime condition. The reduction is only worth reporting if the smaller case still fails, so
//! the unit of work here is verification rather than search: every candidate is checked at its
//! representative point and at the samples its claims depend on, and the cost is counted and
//! returned.
//!
//! Two limits, stated because they are load-bearing. Dropping a parameter means the failure survived
//! *the sampled points across its domain*, not that it is mathematically independent of it. And an
//! interval is the widest one the bisection could verify under an assumed-monotone predicate, which
//! is a boundary in the samples, not a proof about the shape between them.

pub mod case;
pub mod minimize;
pub mod oracle;

pub use case::{Axis, AxisState, Case, Span, Verify, round_sig};
pub use minimize::{Config, Minimal, minimize, reason};
pub use oracle::{FailureOracle, Oracle};
