//! # aporia-search
//!
//! The engine the research question is about: can a computation-aware, multi-evidence search
//! strategy discover and localise regions of distrust using fewer evaluations than simpler
//! exploration?
//!
//! - [`plan`] — the acquisition families, the meta-policy that spends budget between them, and the
//!   deterministic sequences that place points
//! - [`campaign`] — the budgeted loop that evaluates, gathers evidence, updates the atlas, and
//!   records every decision it made
//!
//! The same driver runs all three strategies compared in the benchmark. Random and stratified are
//! not separate implementations: they are this loop with one family enabled, which is what makes the
//! comparison about placement rather than about code.

pub mod campaign;
pub mod plan;

pub use campaign::{Campaign, Config, Decision, Finding, run, run_with};
pub use plan::{Acquisition, Family, Strategy, halton, radical_inverse, to_parameters, to_unit};
