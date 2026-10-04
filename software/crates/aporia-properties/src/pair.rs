//! Paired observations: the input shape that behavioural and sensitivity evidence needs.
//!
//! A single observation can only violate a pointwise rule. Everything else APORIA claims — that a
//! quantity stops increasing, that it scales like a power, that a small push moves it too much —
//! is a statement about *two* executions and the axis between them. So the pairing is recorded
//! rather than rediscovered, and a pair carries the parameter index that was moved.

/// A base execution and a perturbed one, differing along one parameter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pair {
    pub base: u64,
    pub perturbed: u64,
    /// Index of the parameter that was moved.
    pub axis: u16,
}

/// How a pair came to exist, which decides how its evidence is weighed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProbeKind {
    /// The search deliberately moved this axis by this much to test a local property.
    Targeted,
    /// Two independent samples that happen to lie close together, paired afterwards.
    Incidental,
}

/// A pair of executions that differ by swapping two parameter values. Symmetry is only testable
/// this way, and guessing it from the sample would depend on luck rather than design.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SwapProbe {
    pub base: u64,
    pub swapped: u64,
    pub pair: [u16; 2],
}

/// The set of pairs produced by one round of probing.
#[derive(Clone, Debug, Default)]
pub struct Probes {
    pub pairs: Vec<Pair>,
    /// Kind per pair, kept in the same order so a caller never has to zip two vectors by faith.
    pub kinds: Vec<ProbeKind>,
    /// The step used for targeted probes, per axis, in units of that axis' domain width. It is
    /// needed to turn an output difference into a slope, so it is recorded rather than assumed.
    pub axis_step: Vec<f64>,
    /// Deliberate swap probes, one per parameter pair the analyst declared symmetric.
    pub swaps: Vec<SwapProbe>,
}

impl Probes {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.pairs.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pairs.is_empty()
    }

    /// Pairs along one axis.
    #[must_use]
    pub fn on_axis(&self, axis: u16) -> impl Iterator<Item = (&Pair, ProbeKind)> {
        self.pairs
            .iter()
            .zip(self.kinds.iter())
            .filter(move |(p, _)| p.axis == axis)
            .map(|(p, k)| (p, *k))
    }

    #[must_use]
    pub fn step_of(&self, axis: u16) -> f64 {
        self.axis_step.get(axis as usize).copied().unwrap_or(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probes_keep_pair_and_kind_together() {
        let mut p = Probes::new();
        p.pairs.push(Pair {
            base: 0,
            perturbed: 1,
            axis: 2,
        });
        p.kinds.push(ProbeKind::Targeted);
        p.axis_step = vec![0.5, 0.2, 0.01];
        assert_eq!(p.len(), 1);
        assert_eq!(p.step_of(2), 0.01);
        assert_eq!(p.step_of(9), 0.0, "an unknown axis has no step");
        assert_eq!(p.on_axis(2).count(), 1);
        assert_eq!(p.on_axis(1).count(), 0);
    }

    #[test]
    fn an_empty_probe_set_answers_emptily() {
        let p = Probes::new();
        assert!(p.is_empty());
        assert_eq!(p.on_axis(0).count(), 0);
    }
}
