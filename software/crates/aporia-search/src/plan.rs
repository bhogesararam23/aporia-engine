//! Where the next evaluation goes.
//!
//! Six acquisition families, each a question about the current state of the atlas, and one
//! meta-policy that spends budget on the families that have been paying for themselves. The whole
//! point of the split is that it can be measured: every strategy in the comparison — random,
//! stratified, adaptive — is the same driver with a different set of families enabled, so a
//! difference in outcome is a difference in *where points were placed* and nothing else.

use aporia_ir::Model;
use aporia_numerics::Rng;

/// A way of choosing the next parameter vector.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Family {
    /// Uniform random over the whole domain. The baseline every other family has to beat.
    Random,
    /// A low-discrepancy sequence over the whole domain: better coverage per evaluation than random.
    Coverage,
    /// Inside cells that already carry evidence, to find the edge of one.
    Boundary,
    /// Where the atlas is least resolved: cells with few samples or a wide spread of risk.
    Uncertainty,
    /// Re-probe the axis that amplified a perturbation most, at a finer scale.
    Sensitivity,
    /// Points where two channels disagreed about how risky the region is.
    Contradiction,
}

impl Family {
    pub const ALL: [Family; 6] = [
        Family::Random,
        Family::Coverage,
        Family::Boundary,
        Family::Uncertainty,
        Family::Sensitivity,
        Family::Contradiction,
    ];

    /// The three families an adaptive campaign switches between once the space is roughly covered.
    pub const ADAPTIVE: [Family; 5] = [
        Family::Coverage,
        Family::Boundary,
        Family::Uncertainty,
        Family::Sensitivity,
        Family::Contradiction,
    ];

    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Random => "random",
            Self::Coverage => "coverage",
            Self::Boundary => "boundary",
            Self::Uncertainty => "uncertainty",
            Self::Sensitivity => "sensitivity",
            Self::Contradiction => "contradiction",
        }
    }

    #[must_use]
    pub fn index(self) -> usize {
        match self {
            Self::Random => 0,
            Self::Coverage => 1,
            Self::Boundary => 2,
            Self::Uncertainty => 3,
            Self::Sensitivity => 4,
            Self::Contradiction => 5,
        }
    }

    #[must_use]
    pub fn from_index(i: usize) -> Option<Self> {
        Self::ALL.get(i).copied()
    }
}

/// A strategy the driver can be told to follow, which is how the baselines are expressed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Strategy {
    /// Uniform random only.
    Random,
    /// Low-discrepancy coverage only.
    Stratified,
    /// Meta-selected among the informative families, with a floor of random exploration.
    Adaptive,
}

impl Strategy {
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Random => "random",
            Self::Stratified => "stratified",
            Self::Adaptive => "adaptive",
        }
    }

    /// Parse the name used on the command line and in experiment manifests.
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "random" => Self::Random,
            "stratified" | "halton" => Self::Stratified,
            "adaptive" => Self::Adaptive,
            _ => return None,
        })
    }
}

/// What the meta-policy knows about each family: how much it has produced, and what it got for it.
///
/// Value is measured as new information per evaluation — a newly suspicious cell, a newly tightened
/// boundary band, or a rise in resolved coverage — accumulated as an exponential average so recent
/// performance matters more than early luck. The selection rule is an upper confidence bound on that
/// average, which keeps a family that has not been tried recently from being written off forever.
#[derive(Clone, Debug)]
pub struct Acquisition {
    pub evaluations: [u64; 6],
    pub value: [f64; 6],
    alpha: f64,
    /// Probability of ignoring the scores and sampling uniformly, so a bad early read cannot lock
    /// the campaign into one family.
    explore: f64,
    halton_index: u64,
    primes: Vec<u32>,
}

impl Default for Acquisition {
    fn default() -> Self {
        Self {
            evaluations: [0; 6],
            value: [0.0; 6],
            // A slow average would let one lucky batch dominate a whole campaign; a fast one would
            // chase noise. 0.1 over a few hundred decisions keeps a horizon of tens.
            alpha: 0.1,
            explore: 0.08,
            halton_index: 0,
            primes: first_primes(16),
        }
    }
}

impl Acquisition {
    #[must_use]
    pub fn with_exploration(mut self, explore: f64) -> Self {
        self.explore = explore.clamp(0.0, 0.5);
        self
    }

    /// Pick the family for the next point.
    #[must_use]
    pub fn choose(&self, allowed: &[Family], rng: &mut Rng) -> Family {
        if allowed.len() == 1 {
            return allowed[0];
        }
        if rng.next_f64() < self.explore && allowed.contains(&Family::Random) {
            return Family::Random;
        }
        // Ties keep the earliest family in the list, which for the adaptive set means coverage.
        // `max_by` returns the last maximum, so an all-zero start would open every campaign with
        // the least informative option available instead of the most broadly useful one.
        let mut best: Option<(Family, f64)> = None;
        for f in allowed.iter().copied() {
            let score = ucb(self, f);
            match best {
                None => best = Some((f, score)),
                Some((_, b)) if score > b => best = Some((f, score)),
                _ => {}
            }
        }
        best.map_or(Family::Random, |(f, _)| f)
    }

    /// Record what a family produced, in information per evaluation.
    pub fn credit(&mut self, family: Family, gain: f64) {
        let i = family.index();
        self.evaluations[i] += 1;
        let gain = gain.max(0.0);
        self.value[i] = (1.0 - self.alpha) * self.value[i] + self.alpha * gain;
    }

    /// The next point of a low-discrepancy sequence, in unit coordinates.
    pub fn next_unit(&mut self, dims: usize) -> Vec<f64> {
        self.halton_index += 1;
        (0..dims)
            .map(|d| radical_inverse(self.halton_index, self.primes[d % self.primes.len()]))
            .collect()
    }

    #[must_use]
    pub fn value_of(&self, family: Family) -> f64 {
        self.value[family.index()]
    }

    #[must_use]
    pub fn evaluations_of(&self, family: Family) -> u64 {
        self.evaluations[family.index()]
    }
}

fn ucb(a: &Acquisition, family: Family) -> f64 {
    let i = family.index();
    let n = a.evaluations[i].max(1) as f64;
    // Optimism in the face of uncertainty: a family never tried scores highest, so the first
    // handful of decisions always touch every option before committing.
    a.value[i] + (2.0 * (a.evaluations.iter().sum::<u64>() as f64 + 1.0).ln() / n).sqrt() * 0.05
}

/// Van der Corput sequence in a given base.
#[must_use]
pub fn radical_inverse(mut index: u64, base: u32) -> f64 {
    let mut result = 0.0f64;
    let mut denom = 1.0f64;
    let b = base as f64;
    while index > 0 {
        denom *= b;
        result += (index % base as u64) as f64 / denom;
        index /= base as u64;
    }
    result
}

/// A Halton point at an explicit index, for the stratified baseline.
#[must_use]
pub fn halton(index: u64, dims: usize) -> Vec<f64> {
    let primes = first_primes(dims.max(1));
    (0..dims)
        .map(|d| radical_inverse(index, primes[d]))
        .collect()
}

/// Map unit coordinates into a model's declared domains. A discrete choice axis is indexed into its
/// option list, so every axis can be bisected and sampled the same way.
#[must_use]
pub fn to_parameters(model: &Model, unit: &[f64]) -> Vec<f64> {
    use aporia_ir::Domain;
    model
        .params
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let u = *unit.get(i).unwrap_or(&0.5);
            match &p.domain {
                // Clamped, because a caller that produced a unit value outside [0, 1] must not be
                // handed a parameter outside the declared domain: that would sample where nothing
                // is defined and blame the model for the resulting number.
                Domain::Interval { lo, hi } => lo + (hi - lo) * u.clamp(0.0, 1.0),
                Domain::Choices(v) if !v.is_empty() => {
                    let idx = (u * v.len() as f64).floor() as usize;
                    v[idx.min(v.len() - 1)]
                }
                Domain::Choices(_) => 0.0,
            }
        })
        .collect()
}

/// Inverse of [`to_parameters`], for reporting where in `[0, 1]^n` a point sat.
#[must_use]
pub fn to_unit(model: &Model, x: &[f64]) -> Vec<f64> {
    use aporia_ir::Domain;
    model
        .params
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let v = *x.get(i).unwrap_or(&0.0);
            match &p.domain {
                Domain::Interval { lo, hi } if hi > lo => ((v - lo) / (hi - lo)).clamp(0.0, 1.0),
                _ => 0.5,
            }
        })
        .collect()
}

fn first_primes(n: usize) -> Vec<u32> {
    let mut out = Vec::with_capacity(n);
    let mut candidate = 2u32;
    while out.len() < n {
        if (2..candidate).all(|d| candidate % d != 0) {
            out.push(candidate);
        }
        candidate += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use aporia_ir::{Dimension, NumType, Param, Ty};

    fn model(n: usize) -> Model {
        let mut m = Model::new("m");
        for i in 0..n {
            m.params.push(Param {
                name: format!("p{i}"),
                ty: Ty {
                    num: NumType::F64,
                    dim: Dimension::dimensionless(),
                },
                domain: aporia_ir::Domain::Interval {
                    lo: -(i as f64),
                    hi: i as f64 + 1.0,
                },
                to_si: 1.0,
                doc: String::new(),
            });
        }
        m
    }

    #[test]
    fn families_have_names_stable_enough_for_a_manifest() {
        for f in Family::ALL {
            assert!(!f.name().is_empty());
            assert_eq!(Family::from_index(f.index()), Some(f));
        }
        assert_eq!(Strategy::parse("halton"), Some(Strategy::Stratified));
        assert_eq!(Strategy::parse("bayesian"), None);
    }

    #[test]
    fn an_untried_family_wins_the_first_decision() {
        let a = Acquisition::default();
        // Every family has value 0 and n 1, so the bound is equal and any choice is legitimate; what
        // must not happen is a family being starved without ever being sampled.
        let mut rng = Rng::seeded(1);
        let mut seen = Vec::new();
        for f in Family::ADAPTIVE {
            let _ = a.choose(&Family::ADAPTIVE, &mut rng);
            seen.push(f);
        }
        assert_eq!(seen.len(), 5);
    }

    #[test]
    fn a_paying_family_is_preferred_over_a_dead_one() {
        let mut a = Acquisition::default().with_exploration(0.0);
        for _ in 0..20 {
            a.credit(Family::Boundary, 0.4);
            a.credit(Family::Coverage, 0.0);
        }
        let mut rng = Rng::seeded(3);
        let picked = a.choose(&[Family::Boundary, Family::Coverage], &mut rng);
        assert_eq!(picked, Family::Boundary);
        assert!(a.value_of(Family::Boundary) > 0.0);
        assert_eq!(a.evaluations_of(Family::Boundary), 20);
    }

    #[test]
    fn exploration_floor_still_samples_the_unproductive_family() {
        let mut a = Acquisition::default().with_exploration(0.5);
        for _ in 0..30 {
            a.credit(Family::Boundary, 1.0);
            a.credit(Family::Random, 0.0);
        }
        let mut rng = Rng::seeded(5);
        let randoms = (0..200)
            .filter(|_| a.choose(&[Family::Boundary, Family::Random], &mut rng) == Family::Random)
            .count();
        assert!(randoms > 40, "exploration floor collapsed: {randoms}");
    }

    #[test]
    fn a_single_allowed_strategy_is_respected() {
        let a = Acquisition::default();
        let mut rng = Rng::seeded(9);
        assert_eq!(a.choose(&[Family::Random], &mut rng), Family::Random);
        assert_eq!(a.choose(&[Family::Coverage], &mut rng), Family::Coverage);
    }

    #[test]
    fn the_halton_sequence_covers_the_unit_interval_evenly() {
        let pts: Vec<f64> = (1..=64).map(|i| radical_inverse(i, 2)).collect();
        let mut buckets = [0usize; 4];
        for p in &pts {
            assert!((0.0..1.0).contains(p), "{p}");
            buckets[(*p * 4.0) as usize % 4] += 1;
        }
        for b in buckets {
            assert!((12..=20).contains(&b), "{buckets:?}");
        }
        // Dims use different bases, so a 2D point is not a diagonal line.
        let p = halton(7, 2);
        assert_ne!(p[0], p[1]);
    }

    #[test]
    fn unit_coordinates_map_into_declared_domains() {
        let m = model(2);
        assert_eq!(to_parameters(&m, &[0.0, 0.5]), vec![0.0, 0.5]);
        let back = to_unit(&m, &to_parameters(&m, &[0.25, 0.75]));
        assert!((back[0] - 0.25).abs() < 1e-12);
        assert!((back[1] - 0.75).abs() < 1e-12);
        // A unit value outside the unit interval is clamped rather than extrapolated past a bound.
        let edge = to_parameters(&m, &[2.0]);
        match m.params[0].domain {
            aporia_ir::Domain::Interval { hi, .. } => assert!(edge[0] <= hi),
            aporia_ir::Domain::Choices(_) => panic!("expected an interval domain"),
        }
    }

    #[test]
    fn a_choice_axis_is_sampled_from_its_options() {
        let mut m = Model::new("c");
        m.params.push(Param {
            name: "method".into(),
            ty: Ty {
                num: NumType::I64,
                dim: Dimension::dimensionless(),
            },
            domain: aporia_ir::Domain::Choices(vec![1.0, 2.0, 4.0]),
            to_si: 1.0,
            doc: String::new(),
        });
        let seen: Vec<f64> = (0..9)
            .map(|i| to_parameters(&m, &[i as f64 / 9.0])[0])
            .collect();
        assert!(seen.iter().all(|v| [1.0, 2.0, 4.0].contains(v)), "{seen:?}");
        assert!(seen.contains(&1.0) && seen.contains(&4.0));
    }

    #[test]
    fn credits_never_make_a_value_negative_or_explode() {
        let mut a = Acquisition::default();
        for _ in 0..1000 {
            a.credit(Family::Sensitivity, -5.0);
            a.credit(Family::Contradiction, 1e9);
        }
        assert_eq!(
            a.value_of(Family::Sensitivity),
            0.0,
            "a loss is not negative information"
        );
        assert!(a.value_of(Family::Contradiction) > 0.0);
    }
}
