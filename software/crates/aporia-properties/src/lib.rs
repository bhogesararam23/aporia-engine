//! # aporia-properties
//!
//! The analyses that read observations and produce evidence. See [`analyse`] for the channels and
//! [`pair`] for the paired-execution input they need.
//!
//! Property families are of two kinds and the distinction is deliberate. **Declared** relations are
//! what the analyst asserted in the model: monotonicity, a scaling exponent, a symmetry, a
//! conservation rule, a slope bound. **Inferred** patterns are what APORIA looks for unaided, and
//! they are reported as one sensor among five rather than as findings — an inferred violation is
//! evidence that a region deserves more experiments, not evidence that the model is wrong.

mod analyse;
mod pair;

pub use analyse::{
    against_reference, constraints, differential, divergence, inferred_patterns, numerical,
    relations, sensitivity,
};
pub use pair::{Pair, ProbeKind, Probes, SwapProbe};
