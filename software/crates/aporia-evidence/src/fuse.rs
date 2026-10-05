//! Combining channels into one trust-risk score without double counting.
//!
//! This is the part of APORIA the research question actually rests on, so the rule is stated exactly
//! rather than left to a formula in a diagram.
//!
//! Two things make naive combination wrong.
//!
//! 1. **Several measurements in one channel are not several confirmations.** Six monotonicity
//!    violations do not mean six times the evidence of one; they mean the behavioral channel is
//!    shouting. Within a channel the representative is therefore the maximum, not the sum.
//! 2. **Different channels often describe one event.** A sensitivity spike and a physical-rule
//!    violation computed from the same pair of executions are one observation seen twice. Adding
//!    them as if they were independent manufactures confidence out of nothing, which is precisely
//!    how a tool ends up calling a working model suspicious.
//!
//! The rule that follows: order the channels by strength, then accept each one at a discount equal
//! to how much it overlaps with what has already been accepted. Overlap is measured two ways — from
//! recorded provenance (did these findings come from the same execution?) and from the empirical
//! correlation between the channels across the whole experiment. The larger of the two is used, so
//! a correlation the provenance cannot see still costs something, and a shared execution still costs
//! something even in an experiment too small to measure correlation from.
//!
//! The accepted strengths are then combined with a noisy-OR, `1 - Π(1 - a)`, which saturates at one
//! and never claims more confidence than the strongest single signal plus the *independent* part of
//! the others.
//!
//! A risk score is still not a probability. It is a ranking quantity with a documented meaning.

use crate::channel::{Channel, EvidenceSet};

/// How much a shared execution is worth as a discount, when provenance shows two findings came from
/// the same place.
///
/// Not one, because two channels describing the same execution can still disagree about it and that
/// disagreement is information; not zero, because then provenance would be decorative. Half, with
/// the empirical correlation free to raise it further.
pub const PROVENANCE_DISCOUNT: f64 = 0.5;

/// Population-level correlation between channels, estimated over an experiment.
#[derive(Clone, Debug, PartialEq)]
pub struct ChannelCorrelation {
    matrix: [[f64; 5]; 5],
    samples: usize,
}

impl Default for ChannelCorrelation {
    fn default() -> Self {
        Self::none()
    }
}

impl ChannelCorrelation {
    /// No correlation information at all: every pair is discounted only by provenance.
    #[must_use]
    pub fn none() -> Self {
        Self {
            matrix: [[0.0; 5]; 5],
            samples: 0,
        }
    }

    /// Estimate from the per-point channel strengths of a whole experiment.
    ///
    /// Each point contributes one vector of "strongest evidence per channel", and correlation is
    /// Pearson between those columns. This is deliberately crude and deliberately visible: a
    /// correlation estimated from a few hundred points is a rough number, which is why it is only
    /// ever used as a discount, never as a probability.
    #[must_use]
    pub fn estimate(sets: &[EvidenceSet]) -> Self {
        let mut columns: [Vec<f64>; 5] = Default::default();
        for set in sets {
            for (i, col) in columns.iter_mut().enumerate() {
                let c = Channel::from_index(i).unwrap_or(Channel::Behavioral);
                col.push(strongest(set, c));
            }
        }
        let n = sets.len();
        let mut m = [[0.0; 5]; 5];
        if n < 3 {
            // Three points cannot say anything about correlation; pretending otherwise would put a
            // fabricated number into the discount.
            return Self {
                matrix: m,
                samples: n,
            };
        }
        for i in 0..5 {
            for j in (i + 1)..5 {
                let r = pearson(&columns[i], &columns[j]);
                m[i][j] = r.clamp(0.0, 1.0);
                m[j][i] = m[i][j];
            }
        }
        Self {
            matrix: m,
            samples: n,
        }
    }

    /// Correlation between two channels, in `[0, 1]`.
    #[must_use]
    pub fn between(&self, a: Channel, b: Channel) -> f64 {
        if a == b {
            return 1.0;
        }
        self.matrix[a.index()][b.index()]
    }

    #[must_use]
    pub fn samples(&self) -> usize {
        self.samples
    }
}

/// The strongest evidence present in one channel, or zero when the channel is silent.
#[must_use]
pub fn strongest(set: &EvidenceSet, channel: Channel) -> f64 {
    set.items
        .iter()
        .filter(|e| e.channel == channel)
        .map(|e| e.strength)
        .fold(0.0, f64::max)
}

/// One fused risk score for one evaluated point.
#[derive(Clone, Debug, PartialEq)]
pub struct Risk {
    /// Calibrated strength per channel, before any combination. Kept visible because the report and
    /// the search both need the vector, not just the scalar.
    pub per_channel: [f64; 5],
    /// The single number the boundary search ranks on.
    pub score: f64,
    /// What each channel contributed after its discount, with the discount that produced it.
    pub contributions: Vec<Contribution>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Contribution {
    pub channel: Channel,
    /// The channel's own calibrated strength.
    pub strength: f64,
    /// How much of it was treated as already counted elsewhere.
    pub discount: f64,
    /// `strength * (1 - discount)`, the part that actually entered the score.
    pub accepted: f64,
}

impl Default for Risk {
    fn default() -> Self {
        Self {
            per_channel: [0.0; 5],
            score: 0.0,
            contributions: Vec::new(),
        }
    }
}

impl Risk {
    /// The channel that contributed most after discounting.
    #[must_use]
    pub fn dominant(&self) -> Option<Channel> {
        self.contributions
            .iter()
            .max_by(|a, b| {
                a.accepted
                    .partial_cmp(&b.accepted)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .filter(|c| c.accepted > 0.0)
            .map(|c| c.channel)
    }

    #[must_use]
    pub fn channels_present(&self) -> usize {
        self.per_channel.iter().filter(|s| **s > 0.0).count()
    }

    /// Does this point dominate `other` on every channel and beat it on at least one?
    ///
    /// Implemented, tested and *not currently what the search ranks with*: `aporia-search` compares
    /// candidates by the single fused score, so dominance is available to a caller that wants to keep
    /// a channel disagreement from being averaged away but no caller does yet. Stated here because the
    /// alternative — a doc comment implying the acquisition policy is Pareto-aware — describes an
    /// instrument this repository does not run.
    #[must_use]
    pub fn dominates(&self, other: &Risk) -> bool {
        let at_least = self
            .per_channel
            .iter()
            .zip(other.per_channel.iter())
            .all(|(a, b)| *a >= *b);
        let strictly = self
            .per_channel
            .iter()
            .zip(other.per_channel.iter())
            .any(|(a, b)| *a > *b);
        at_least && strictly
    }

    /// The Pareto front of a set of risks, as indices into the input, strongest first.
    ///
    /// O(n²) by design. The fronts the search needs are small, and a cleverer algorithm would trade
    /// a proof of correctness for a data structure nobody reading this file can check.
    #[must_use]
    pub fn pareto_front(risks: &[Risk]) -> Vec<usize> {
        let mut front = Vec::new();
        for (i, a) in risks.iter().enumerate() {
            let dominated = risks
                .iter()
                .enumerate()
                .any(|(j, b)| i != j && b.dominates(a));
            if !dominated {
                front.push(i);
            }
        }
        front.sort_by(|&x, &y| {
            risks[y]
                .score
                .partial_cmp(&risks[x].score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        front
    }
}

/// Fuse one point's evidence into a risk.
#[must_use]
pub fn fuse(set: &EvidenceSet, correlation: &ChannelCorrelation) -> Risk {
    let per_channel: Vec<f64> = Channel::ALL.iter().map(|c| strongest(set, *c)).collect();
    let mut ordered: Vec<(Channel, f64)> = Channel::ALL
        .iter()
        .copied()
        .zip(per_channel.iter().copied())
        .filter(|(_, s)| *s > 0.0)
        .collect();
    // Strongest first: the discount is "how much of this is already counted", so what has already
    // been counted has to be decided before the thing being discounted.
    ordered.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    let mut contributions = Vec::new();
    let mut survival = 1.0f64;
    let mut accepted_channels: Vec<Channel> = Vec::new();

    for (channel, strength) in ordered {
        let empirical = accepted_channels
            .iter()
            .map(|a| correlation.between(*a, channel))
            .fold(0.0f64, f64::max);
        let provenance = if accepted_channels.is_empty() {
            0.0
        } else {
            provenance_overlap(set, channel, &accepted_channels)
        };
        let discount = empirical.max(provenance).min(0.95);
        let accepted = strength * (1.0 - discount);
        contributions.push(Contribution {
            channel,
            strength,
            discount,
            accepted,
        });
        survival *= 1.0 - accepted;
        accepted_channels.push(channel);
    }

    Risk {
        per_channel: [
            per_channel[0],
            per_channel[1],
            per_channel[2],
            per_channel[3],
            per_channel[4],
        ],
        score: (1.0 - survival).clamp(0.0, 1.0),
        contributions,
    }
}

/// Discount derived from shared executions: 0, [`PROVENANCE_DISCOUNT`], or 0.95 when the two
/// channels are built from entirely the same set of observations.
fn provenance_overlap(set: &EvidenceSet, channel: Channel, already: &[Channel]) -> f64 {
    let mine: Vec<&u64> = set
        .items
        .iter()
        .filter(|e| e.channel == channel)
        .flat_map(|e| e.observations.iter())
        .collect();
    if mine.is_empty() {
        return 0.0;
    }
    let theirs: Vec<&u64> = set
        .items
        .iter()
        .filter(|e| already.contains(&e.channel))
        .flat_map(|e| e.observations.iter())
        .collect();
    if theirs.is_empty() {
        return 0.0;
    }
    let shared = mine.iter().filter(|m| theirs.contains(m)).count();
    if shared == 0 {
        return 0.0;
    }
    // Jaccard over observation ids: one shared execution out of many is a mild discount, and two
    // channels computed from exactly the same executions are nearly the same finding.
    let union = {
        let mut all: Vec<u64> = mine.iter().chain(theirs.iter()).copied().copied().collect();
        all.sort_unstable();
        all.dedup();
        all.len()
    };
    let jaccard = shared as f64 / union as f64;
    if jaccard >= 0.999 {
        0.95
    } else {
        PROVENANCE_DISCOUNT * jaccard
    }
}

/// Pearson correlation of two equal-length columns, or 0 when either is constant.
#[must_use]
fn pearson(a: &[f64], b: &[f64]) -> f64 {
    if a.len() != b.len() || a.len() < 2 {
        return 0.0;
    }
    let n = a.len() as f64;
    let ma = a.iter().sum::<f64>() / n;
    let mb = b.iter().sum::<f64>() / n;
    let mut cov = 0.0;
    let mut va = 0.0;
    let mut vb = 0.0;
    for i in 0..a.len() {
        let da = a[i] - ma;
        let db = b[i] - mb;
        cov += da * db;
        va += da * da;
        vb += db * db;
    }
    if va <= 0.0 || vb <= 0.0 {
        return 0.0;
    }
    cov / (va.sqrt() * vb.sqrt())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel::{Evidence, Subject};

    fn ev(channel: Channel, strength: f64, obs: &[u64]) -> Evidence {
        Evidence::new(
            channel,
            Subject::Constraint(0),
            strength,
            obs.to_vec(),
            String::new(),
        )
        .with_strength(strength)
    }

    fn set(items: Vec<Evidence>) -> EvidenceSet {
        let mut s = EvidenceSet::new();
        s.items = items;
        s
    }

    #[test]
    fn no_evidence_is_no_risk() {
        let r = fuse(&EvidenceSet::new(), &ChannelCorrelation::none());
        assert_eq!(r.score, 0.0);
        assert_eq!(r.contributions, []);
        assert_eq!(r.dominant(), None);
    }

    #[test]
    fn one_channel_passes_straight_through() {
        let r = fuse(
            &set(vec![ev(Channel::Physical, 0.6, &[1])]),
            &ChannelCorrelation::none(),
        );
        assert!((r.score - 0.6).abs() < 1e-12, "{}", r.score);
        assert_eq!(r.dominant(), Some(Channel::Physical));
    }

    #[test]
    fn several_findings_in_one_channel_do_not_add_up() {
        let one = fuse(
            &set(vec![ev(Channel::Behavioral, 0.7, &[1])]),
            &ChannelCorrelation::none(),
        );
        let many = fuse(
            &set(vec![
                ev(Channel::Behavioral, 0.7, &[1]),
                ev(Channel::Behavioral, 0.65, &[2]),
                ev(Channel::Behavioral, 0.5, &[3]),
            ]),
            &ChannelCorrelation::none(),
        );
        assert_eq!(
            one.score, many.score,
            "a channel reports its maximum, not its sum"
        );
    }

    #[test]
    fn independent_channels_combine_with_a_noisy_or() {
        let r = fuse(
            &set(vec![
                ev(Channel::Physical, 0.5, &[1]),
                ev(Channel::Numerical, 0.5, &[2, 3]),
            ]),
            &ChannelCorrelation::none(),
        );
        assert!((r.score - 0.75).abs() < 1e-12, "{}", r.score);
    }

    #[test]
    fn findings_from_the_same_execution_are_worth_less_than_two_events() {
        let shared = fuse(
            &set(vec![
                ev(Channel::Physical, 0.5, &[1, 2]),
                ev(Channel::Sensitivity, 0.5, &[1, 2]),
            ]),
            &ChannelCorrelation::none(),
        );
        let separate = fuse(
            &set(vec![
                ev(Channel::Physical, 0.5, &[1, 2]),
                ev(Channel::Sensitivity, 0.5, &[3, 4]),
            ]),
            &ChannelCorrelation::none(),
        );
        assert!(
            shared.score < separate.score,
            "double counting: shared {} vs separate {}",
            shared.score,
            separate.score
        );
        // Exactly the same provenance is almost the same finding.
        let identical = fuse(
            &set(vec![
                ev(Channel::Physical, 0.5, &[7]),
                ev(Channel::Sensitivity, 0.5, &[7]),
            ]),
            &ChannelCorrelation::none(),
        );
        assert!(
            identical.score < 0.6,
            "identical provenance gave {}",
            identical.score
        );
    }

    #[test]
    fn an_estimated_channel_correlation_discounts_even_without_shared_provenance() {
        // Build an experiment where two channels rise and fall together across many points.
        let mut sets = Vec::new();
        for i in 0..40 {
            let s = (i as f64) / 40.0;
            sets.push(set(vec![
                ev(Channel::Behavioral, s, &[i as u64]),
                ev(Channel::Numerical, s * 0.9 + 0.01, &[1000 + i as u64]),
            ]));
        }
        let corr = ChannelCorrelation::estimate(&sets);
        assert!(
            corr.between(Channel::Behavioral, Channel::Numerical) > 0.99,
            "estimate failed"
        );
        let fused = fuse(&sets[20], &corr);
        let naive = fuse(&sets[20], &ChannelCorrelation::none());
        assert!(fused.score < naive.score, "correlation was ignored");
        assert_eq!(corr.samples(), 40);
    }

    #[test]
    fn three_points_are_not_enough_to_claim_a_correlation() {
        let sets = vec![
            set(vec![ev(Channel::Behavioral, 0.1, &[0])]),
            set(vec![ev(Channel::Behavioral, 0.2, &[1])]),
        ];
        let corr = ChannelCorrelation::estimate(&sets);
        assert_eq!(corr.between(Channel::Behavioral, Channel::Physical), 0.0);
    }

    #[test]
    fn a_constant_channel_has_no_correlation_with_anything() {
        let sets: Vec<EvidenceSet> = (0..10)
            .map(|i| {
                set(vec![
                    ev(Channel::Physical, 0.5, &[i]),
                    ev(Channel::Numerical, (i as f64) / 10.0, &[100 + i]),
                ])
            })
            .collect();
        let corr = ChannelCorrelation::estimate(&sets);
        assert_eq!(corr.between(Channel::Physical, Channel::Numerical), 0.0);
    }

    #[test]
    fn risk_is_monotone_in_each_channel() {
        let base = set(vec![ev(Channel::Physical, 0.4, &[1])]);
        let low = fuse(&base, &ChannelCorrelation::none());
        let high = fuse(
            &set(vec![ev(Channel::Physical, 0.9, &[1])]),
            &ChannelCorrelation::none(),
        );
        assert!(high.score > low.score);
    }

    #[test]
    fn pareto_front_keeps_the_undominated_and_only_those() {
        let a = Risk {
            per_channel: [0.5, 0.0, 0.0, 0.0, 0.0],
            score: 0.5,
            contributions: Vec::new(),
        };
        let b = Risk {
            per_channel: [0.4, 0.0, 0.0, 0.0, 0.0],
            score: 0.4,
            contributions: Vec::new(),
        };
        let c = Risk {
            per_channel: [0.1, 0.9, 0.0, 0.0, 0.0],
            score: 0.9,
            contributions: Vec::new(),
        };
        assert!(a.dominates(&b));
        assert!(!a.dominates(&c), "a trades against c, so neither dominates");
        assert!(!b.dominates(&a));
        let front = Risk::pareto_front(&[a, b, c]);
        assert_eq!(front.len(), 2, "{front:?}");
        assert!(!front.contains(&1), "b is dominated by a");
        assert_eq!(front[0], 2, "the higher score leads the front");
    }

    #[test]
    fn a_channel_identical_to_itself_is_fully_discounted() {
        let corr = ChannelCorrelation::none();
        assert_eq!(
            corr.between(Channel::Sensitivity, Channel::Sensitivity),
            1.0
        );
    }

    #[test]
    fn channels_present_counts_only_channels_that_spoke() {
        let r = fuse(
            &set(vec![
                ev(Channel::Physical, 0.3, &[1]),
                ev(Channel::Differential, 0.1, &[2]),
                ev(Channel::Behavioral, 0.0, &[3]),
            ]),
            &ChannelCorrelation::none(),
        );
        assert_eq!(r.channels_present(), 2);
    }
}
