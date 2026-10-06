//! What "still a failure" means, kept at arm's length from the minimiser.
//!
//! Minimisation is only trustworthy if the thing being preserved is the same claim the report makes,
//! so the predicate is an input rather than something the algorithm assumes. Two implementations ship
//! here: a closure, which is what a test or a caller with its own definition uses, and
//! [`FailureOracle`], which asks the model itself. A caller whose question needs more than one
//! execution brings its own [`Oracle`] type — `aporia-bench` does, for the risk threshold — because
//! the cost is part of what an oracle knows.
//!
//! An oracle must be a function of the point alone. If it consults a calibrator fitted on a
//! population, minimising changes the population and the answer stops meaning what it meant before —
//! which is why [`crate::minimize`] takes the physical-channel form by default and why the risk-based
//! form is handed in by the caller, with its calibrator already fixed.
//!
//! The *execution path* is a separate argument for exactly that reason. An oracle stays `&self`, so
//! the type system keeps saying it is a rule and not a process; asking a point runs something, and
//! whatever runs it may be a live child process whose next answer depends on state it holds. A
//! minimiser therefore carries the same [`Executor`] the campaign carried, and a finding discovered
//! through a program is verified by that program rather than by an interpreter that never computed it.

use aporia_evidence::Channel;
use aporia_ir::Model;
use aporia_properties::{constraints, divergence};
use aporia_runtime::{ExecConfig, Executor, Observation};

/// What one answer cost.
///
/// A minimiser spends two different quantities and a published cost column has to say which one it
/// is: the number of *queries* it asked, and the number of *model executions* those queries performed.
/// They coincide for an oracle that runs the model once per point and diverge badly for one that
/// rebuilds a probe star around it, so the cost travels with the answer instead of being inferred
/// from the call count by whoever reads the number next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Verdict {
    pub violating: bool,
    /// Model executions actually performed to reach this answer. Zero is a real value: a predicate
    /// that only reads the coordinates it was handed executed nothing, and saying so is what keeps
    /// the column honest when such an oracle is used.
    pub executions: u64,
}

impl Verdict {
    #[must_use]
    pub fn new(violating: bool, executions: u64) -> Self {
        Self {
            violating,
            executions,
        }
    }
}

/// Decides whether a point still counts as a failure, and reports what deciding it cost.
///
/// `engine` is the path the finding was found on. Nothing here defaults it to the interpreter: an
/// oracle that could silently run a model by a different path than the campaign did would be able to
/// answer "yes, this smaller case still fails" about a computation nobody performed.
pub trait Oracle {
    fn query(&self, x: &[f64], engine: &mut dyn Executor) -> Verdict;
}

/// Closures are oracles, so a caller can minimise against its own definition without a wrapper type.
/// A closure states its own cost because the alternative — assuming one execution per call — would put
/// a number in the column that nothing measured. A predicate that reads only the coordinates takes the
/// path and does not use it, which is what its zero cost then means.
impl<F> Oracle for F
where
    F: Fn(&[f64], &mut dyn Executor) -> Verdict,
{
    fn query(&self, x: &[f64], engine: &mut dyn Executor) -> Verdict {
        self(x, engine)
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
    pub fn evidence_at(
        &self,
        x: &[f64],
        engine: &mut dyn Executor,
    ) -> Vec<aporia_evidence::Evidence> {
        let outcome = engine.execute(self.model, x, self.cfg);
        let o = Observation::new(0, x.to_vec(), &outcome);
        let mut out = constraints(self.model, &o);
        out.extend(divergence(self.model, &o));
        out
    }
}

impl Oracle for FailureOracle<'_> {
    fn query(&self, x: &[f64], engine: &mut dyn Executor) -> Verdict {
        // A point that cannot be evaluated at all — the budget ran out, the path refused the shape —
        // is not a preserved failure, it is missing information, and the difference matters
        // when the case is reported. Refusing on arity runs nothing, so it is charged nothing.
        if x.len() != self.model.params.len() {
            return Verdict::new(false, 0);
        }
        // Existence is the signal. Both analyses emit only when something actually broke, and a
        // comparison rule's strength is still zero until calibration runs, so testing strength here
        // would silently ignore every `require` violation that is not a NaN.
        let violating = self
            .evidence_at(x, engine)
            .iter()
            .any(|e| e.channel == Channel::Physical && e.magnitude > 0.0);
        // One answer, one execution of the path that answered it. The number is not claimed to be an
        // interpreter run: a program's answer is exactly as much work as the campaign's were.
        Verdict::new(violating, 1)
    }
}
