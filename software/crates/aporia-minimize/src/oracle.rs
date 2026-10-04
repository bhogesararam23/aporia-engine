//! What "still a failure" means, kept at arm's length from the minimiser.
//!
//! Minimisation is only trustworthy if the thing being preserved is the same claim the report makes,
//! so the predicate is an input rather than something the algorithm assumes. Two implementations ship
//! here: a closure, which is what the benchmark and the tests use, and [`FailureOracle`], which asks
//! the model itself.
//!
//! An oracle must be a function of the point alone. If it consults a calibrator fitted on a
//! population, minimising changes the population and the answer stops meaning what it meant before —
//! which is why [`crate::minimize`] takes the physical-channel form by default and why the risk-based
//! form is handed in by the caller, with its calibrator already fixed.

use aporia_evidence::Channel;
use aporia_ir::Model;
use aporia_properties::{constraints, divergence};
use aporia_runtime::{ExecConfig, Observation, interp};

/// Decides whether a point still counts as a failure.
pub trait Oracle {
    fn violating(&self, x: &[f64]) -> bool;
}

/// Closures are oracles, so a caller can minimise against its own definition without a wrapper type.
impl<F> Oracle for F
where
    F: Fn(&[f64]) -> bool,
{
    fn violating(&self, x: &[f64]) -> bool {
        self(x)
    }
}

/// A failure is a point where the physical channel speaks: a declared rule is violated, or an output
/// left the real numbers.
///
/// This is the right default for a counterexample, because both halves of it are absolute evidence —
/// strength 1.0 by construction, not calibrated against anything. A minimised case therefore preserves
/// a fact about the model at that point, not a score that depends on which other points happened to
/// be sampled. That distinction is the difference between "the smallest thing I could reproduce" and
/// "the smallest thing my search happened to like".
#[derive(Debug)]
pub struct FailureOracle<'a> {
    pub model: &'a Model,
    pub cfg: ExecConfig,
}

impl<'a> FailureOracle<'a> {
    #[must_use]
    pub fn new(model: &'a Model) -> Self {
        Self {
            model,
            cfg: ExecConfig::default(),
        }
    }

    /// The physical evidence at one point, with nothing filtered. Exposed because a report should
    /// print *why* the minimal case is a failure, not only that it is.
    #[must_use]
    pub fn evidence_at(&self, x: &[f64]) -> Vec<aporia_evidence::Evidence> {
        let outcome = interp::run(self.model, x, self.cfg);
        let o = Observation::new(0, x.to_vec(), &outcome);
        let mut out = constraints(self.model, &o);
        out.extend(divergence(self.model, &o));
        out
    }
}

impl Oracle for FailureOracle<'_> {
    fn violating(&self, x: &[f64]) -> bool {
        // A point that cannot be evaluated at all — the budget ran out, the interpreter refused the
        // shape — is not a preserved failure, it is missing information, and the difference matters
        // when the case is reported.
        if x.len() != self.model.params.len() {
            return false;
        }
        // Existence is the signal. Both analyses emit only when something actually broke, and a
        // comparison rule's strength is still zero until calibration runs, so testing strength here
        // would silently ignore every `require` violation that is not a NaN.
        self.evidence_at(x)
            .iter()
            .any(|e| e.channel == Channel::Physical && e.magnitude > 0.0)
    }
}
