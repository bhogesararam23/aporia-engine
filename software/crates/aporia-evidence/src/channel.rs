//! Evidence as data.
//!
//! The specification's first design principle is that evidence is a first-class object rather than
//! console output, and the reason is not cosmetic: to fuse signals honestly, each one has to carry
//! where it came from. Two findings that were computed from the same execution are not two findings,
//! and the only way to know that later is to have written the provenance down at the time.

use std::fmt;

/// The five channels the specification names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Channel {
    /// How outputs move when inputs move: monotonicity, scaling, symmetry, regime change.
    Behavioral,
    /// Whether a declared rule held: positivity, conservation, bounds, domain rules.
    Physical,
    /// Whether the answer depends on numerical choices: precision, evaluation order.
    Numerical,
    /// Whether independent execution paths agree: scalar against batch, CPU against GPU.
    Differential,
    /// Whether a tiny input change produced an implausibly large output change.
    Sensitivity,
}

impl Channel {
    /// The magnitude below which this channel's measurement says nothing.
    ///
    /// A relative calibration divides by the experiment's own typical value, so a channel whose
    /// typical value is round-off noise turns ordinary noise into maximum strength: comparing an
    /// f32 and an f64 path of a smooth model disagrees at about 1e-7 everywhere, and twice that
    /// tiny difference is not evidence of anything. The floor is what the channel *means*:
    ///
    /// - numerical compares two precisions, and f32 carries about seven digits, so anything under
    ///   1e-6 relative is the precision gap itself rather than a sensitivity to it.
    /// - differential compares paths that should agree exactly — scalar and batched, same
    ///   arithmetic, same inputs — so a nonzero difference is already evidence and a floor would
    ///   hide the bug the channel exists to find.
    /// - physical reports facts: a violated rule or a NaN has no noise band.
    /// - behavioural and sensitivity are ratios of quantities the model itself produced, whose
    ///   natural scale is set by the experiment, so they keep no absolute floor.
    #[must_use]
    pub const fn noise_floor(self) -> f64 {
        match self {
            // Only the numerical channel has a noise band: the others measure something whose
            // smallest meaningful value is set by the model, not by the representation.
            Self::Numerical => 1e-6,
            Self::Behavioral | Self::Physical | Self::Differential | Self::Sensitivity => 0.0,
        }
    }

    pub const ALL: [Channel; 5] = [
        Channel::Behavioral,
        Channel::Physical,
        Channel::Numerical,
        Channel::Differential,
        Channel::Sensitivity,
    ];

    #[must_use]
    pub fn index(self) -> usize {
        match self {
            Self::Behavioral => 0,
            Self::Physical => 1,
            Self::Numerical => 2,
            Self::Differential => 3,
            Self::Sensitivity => 4,
        }
    }

    #[must_use]
    pub fn from_index(i: usize) -> Option<Self> {
        Self::ALL.get(i).copied()
    }

    #[must_use]
    pub fn code(self) -> char {
        match self {
            Self::Behavioral => 'B',
            Self::Physical => 'P',
            Self::Numerical => 'N',
            Self::Differential => 'D',
            Self::Sensitivity => 'S',
        }
    }

    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Behavioral => "behavioral",
            Self::Physical => "physical",
            Self::Numerical => "numerical",
            Self::Differential => "differential",
            Self::Sensitivity => "sensitivity",
        }
    }
}

impl fmt::Display for Channel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// What a piece of evidence is about.
#[derive(Clone, Debug, PartialEq)]
pub enum Subject {
    /// A `require` rule, by A-IR constraint id.
    Constraint(u16),
    /// A `check` relation, by A-IR relation id.
    Relation(u16),
    /// A named behaviour APORIA discovered on its own, with the output and parameter involved.
    Pattern {
        output: u16,
        param: u16,
        kind: PatternKind,
    },
    /// An output whose value is not a number or is infinite.
    Divergence { output: u16 },
    /// The gap between two execution paths of the same model at the same point.
    PathDisagreement { output: u16 },
    /// How much one output moved for a known move in parameter `axis`.
    ///
    /// The output is part of the subject because two outputs of one model have different gains by
    /// nature: a position and its conserved energy are not the same claim about the same probe, and
    /// measuring one against the other's typical value turns a stiff quantity into a permanent
    /// alarm and a slack one into a permanent silence.
    LocalSlope { output: u16, axis: u16 },
}

/// Behavioural patterns APORIA looks for without being told.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PatternKind {
    Increasing,
    Decreasing,
    Scaling,
    Symmetry,
    Continuity,
}

impl PatternKind {
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Increasing => "monotone_up",
            Self::Decreasing => "monotone_down",
            Self::Scaling => "scales_as",
            Self::Symmetry => "symmetric",
            Self::Continuity => "continuous",
        }
    }
}

impl Subject {
    /// A short identifier used in reports and as a grouping key.
    #[must_use]
    pub fn key(&self) -> String {
        match self {
            Self::Constraint(id) => format!("require{id}"),
            Self::Relation(id) => format!("check{id}"),
            Self::Pattern {
                output,
                param,
                kind,
            } => {
                format!("o{output}~p{param}:{}", kind.name())
            }
            Self::Divergence { output } => format!("o{output}:divergent"),
            Self::PathDisagreement { output } => format!("o{output}:paths"),
            Self::LocalSlope { output, axis } => format!("o{output}~p{axis}:slope"),
        }
    }
}

/// One finding about one execution.
///
/// `magnitude` is the raw measurement in whatever unit the channel uses. `strength` is that
/// measurement after calibration into `[0, 1]`, which is what makes channels comparable at all.
/// `observations` is the provenance the fusion rule depends on.
#[derive(Clone, Debug, PartialEq)]
pub struct Evidence {
    pub channel: Channel,
    pub subject: Subject,
    pub magnitude: f64,
    pub strength: f64,
    pub observations: Vec<u64>,
    pub detail: String,
    /// See [`Evidence::absolute`].
    pub fixed: bool,
}

impl Evidence {
    #[must_use]
    pub fn new(
        channel: Channel,
        subject: Subject,
        magnitude: f64,
        observations: Vec<u64>,
        detail: String,
    ) -> Self {
        Self {
            channel,
            subject,
            magnitude,
            strength: 0.0,
            observations,
            detail,
            fixed: false,
        }
    }

    #[must_use]
    pub fn with_strength(mut self, strength: f64) -> Self {
        self.strength = strength.clamp(0.0, 1.0);
        self
    }

    /// True when this finding and `other` were computed from at least one execution in common.
    ///
    /// Correlation between channels is mostly a consequence of this: a sensitivity spike and a
    /// monotonicity violation found on the same pair of observations are one event seen twice, not
    /// two events.
    #[must_use]
    pub fn shares_observation_with(&self, other: &Self) -> bool {
        if self.observations.is_empty() || other.observations.is_empty() {
            return false;
        }
        self.observations
            .iter()
            .any(|a| other.observations.contains(a))
    }

    /// Evidence whose strength is a fact rather than a measurement.
    ///
    /// A rule that fired is fired; it must not be diluted because every other violation in the
    /// experiment fired by the same amount. Calibration answers "how unusual is this number", and
    /// "unusual" has no meaning for a boolean outcome, so this flag lets the calibrator leave such
    /// an item alone. Set by the divergence and hard-failure analyses.
    #[must_use]
    pub fn absolute(mut self, strength: f64) -> Self {
        self.strength = strength.clamp(0.0, 1.0);
        self.fixed = true;
        self
    }

    #[must_use]
    pub fn is_fixed(&self) -> bool {
        self.fixed
    }

    /// The report's confidence word. `strength` is continuous; people need a discrete one.
    #[must_use]
    pub fn level(&self) -> Confidence {
        if self.strength >= 0.75 {
            Confidence::High
        } else if self.strength >= 0.45 {
            Confidence::Medium
        } else {
            Confidence::Low
        }
    }
}

/// The discrete confidence a finding is reported with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Confidence {
    Low,
    Medium,
    High,
}

impl Confidence {
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Low => "LOW",
            Self::Medium => "MEDIUM",
            Self::High => "HIGH",
        }
    }
}

/// Evidence collected about one evaluated point.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EvidenceSet {
    pub items: Vec<Evidence>,
}

impl EvidenceSet {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, e: Evidence) {
        self.items.push(e);
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    #[must_use]
    pub fn channels_present(&self) -> Vec<Channel> {
        Channel::ALL
            .iter()
            .copied()
            .filter(|c| self.items.iter().any(|e| &e.channel == c))
            .collect()
    }

    /// Evidence for one channel, strongest first.
    #[must_use]
    pub fn for_channel(&self, channel: Channel) -> Vec<&Evidence> {
        let mut v: Vec<&Evidence> = self.items.iter().filter(|e| e.channel == channel).collect();
        v.sort_by(|a, b| {
            b.strength
                .partial_cmp(&a.strength)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        v
    }

    pub fn extend(&mut self, other: impl IntoIterator<Item = Evidence>) {
        self.items.extend(other);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Subject::*;

    fn ev(channel: Channel, subject: Subject, strength: f64, obs: &[u64]) -> Evidence {
        Evidence::new(channel, subject, strength, obs.to_vec(), String::new())
            .with_strength(strength)
    }

    #[test]
    fn channels_have_a_stable_index_and_code() {
        for (i, c) in Channel::ALL.iter().enumerate() {
            assert_eq!(Channel::from_index(c.index()), Some(*c));
            assert_eq!(c.index(), i);
            assert!(!c.code().is_lowercase());
        }
        assert_eq!(Channel::from_index(9), None);
    }

    #[test]
    fn subject_keys_are_unique_per_subject() {
        let a = Constraint(3).key();
        let b = Pattern {
            output: 1,
            param: 2,
            kind: PatternKind::Scaling,
        }
        .key();
        let c = Pattern {
            output: 1,
            param: 2,
            kind: PatternKind::Continuity,
        }
        .key();
        assert_eq!(a, "require3");
        assert_ne!(b, c, "the kind has to be part of the key");
        assert_ne!(Constraint(3).key(), Relation(3).key());
    }

    #[test]
    fn shared_provenance_is_detected_and_empty_never_counts() {
        let a = ev(
            Channel::Sensitivity,
            LocalSlope { output: 0, axis: 0 },
            0.6,
            &[7, 8],
        );
        let b = ev(Channel::Behavioral, Relation(1), 0.7, &[8, 9]);
        let c = ev(Channel::Physical, Constraint(0), 0.9, &[]);
        assert!(a.shares_observation_with(&b));
        assert!(
            !a.shares_observation_with(&c),
            "no provenance cannot overlap"
        );
    }

    #[test]
    fn strength_is_clamped_into_the_unit_interval() {
        assert_eq!(
            Evidence::new(
                Channel::Numerical,
                Divergence { output: 0 },
                5.0,
                vec![],
                String::new()
            )
            .with_strength(3.0)
            .strength,
            1.0
        );
        assert_eq!(
            ev(Channel::Numerical, Divergence { output: 0 }, -1.0, &[]).strength,
            0.0
        );
    }

    #[test]
    fn confidence_words_follow_the_strength() {
        assert_eq!(
            ev(Channel::Physical, Constraint(0), 0.9, &[]).level(),
            Confidence::High
        );
        assert_eq!(
            ev(Channel::Physical, Constraint(0), 0.5, &[]).level(),
            Confidence::Medium
        );
        assert_eq!(
            ev(Channel::Physical, Constraint(0), 0.1, &[]).level(),
            Confidence::Low
        );
    }

    #[test]
    fn a_set_answers_the_questions_a_report_asks() {
        let mut set = EvidenceSet::new();
        set.push(ev(Channel::Behavioral, Relation(0), 0.2, &[1]));
        set.push(ev(Channel::Behavioral, Relation(1), 0.8, &[2]));
        set.push(ev(
            Channel::Sensitivity,
            LocalSlope { output: 0, axis: 0 },
            0.4,
            &[1, 2],
        ));
        assert_eq!(set.len(), 3);
        assert_eq!(
            set.channels_present(),
            vec![Channel::Behavioral, Channel::Sensitivity]
        );
        let ordered = set.for_channel(Channel::Behavioral);
        assert_eq!(ordered[0].subject, Relation(1), "strongest first");
        assert!(
            !EvidenceSet::new()
                .channels_present()
                .contains(&Channel::Physical)
        );
    }
}
