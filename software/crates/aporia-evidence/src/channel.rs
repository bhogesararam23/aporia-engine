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
    /// Whether independent execution paths agree. In a campaign this is the scalar runtime against
    /// `aporia-numerics`' double-double reference; the other pairs worth having — scalar against
    /// batched, one machine's float against another's — are the same question asked of a different
    /// pair, and are not all wired. What the channel means is: two ways of computing the same
    /// expression that should not disagree, disagreeing.
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
            // f64 against f32: the expected disagreement of a well-conditioned computation is about
            // 1e-7, so anything at that level is the representation talking, not the model.
            Self::Numerical => 1e-6,
            // f64 against the double-double reference. The reference is accurate to roughly 32
            // digits, so any disagreement this channel can observe at all is f64's own
            // representation error; one part in 10^9 sits three orders above f64 epsilon, which is
            // the same relation its floor has to Numerical's 1e-6 (about one order above f32
            // epsilon).
            //
            // The value chosen first was 1e-13 and it did nothing whatsoever: `Calibrator::fit`
            // clamps every scale to `MIN_SCALE` = 1e-12, so a floor below the guard cannot raise
            // anything. Two full ladder runs came out byte-identical and that is how it was caught;
            // hence the test in `calibrate.rs` that fails any declared floor sitting between zero
            // and the guard.
            //
            // What this floor is NOT responsible for, despite an earlier claim here: the control
            // regression that appeared when the channel was wired. Measured by A/B on the same seeds
            // with only the rate changing, control cleanliness moved 73/90 -> 49/90 zero-suspicion
            // campaigns because charged reference evaluations shift the sampled points, not because
            // round-off was being amplified — the differential items on those models sit near 1e-13,
            // below even the old guard, and calibrate to strength 0.000 whichever floor is used. See
            // `harness::Plan::differential_every` for the numbers.
            Self::Differential => 1e-9,
            Self::Behavioral | Self::Physical | Self::Sensitivity => 0.0,
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

    /// The channel a caller named, or nothing. Derived from [`Channel::name`] rather than written out
    /// beside it, so a refusal can never list a vocabulary this type has stopped producing — the same
    /// rule `Strategy::parse` was put under when the two readers of one enum disagreed.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.name() == text)
    }

    /// Every name a caller may use, in [`Channel::ALL`] order.
    #[must_use]
    pub fn names() -> Vec<&'static str> {
        Self::ALL.iter().map(|c| c.name()).collect()
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
    fn the_channel_vocabulary_round_trips_and_refuses_anything_else() {
        // The ablation flag in `aporia-bench` parses these words. A name that parses here and is
        // refused there, or the reverse, is the defect that `Strategy::parse` was made the single
        // vocabulary for: two readers of one enum disagreeing.
        for c in Channel::ALL {
            assert_eq!(Channel::parse(c.name()), Some(c), "{}", c.name());
        }
        assert_eq!(
            Channel::names(),
            Channel::ALL.iter().map(|c| c.name()).collect::<Vec<_>>()
        );
        // Refused rather than defaulted: `--ablate behaviour` would otherwise silence nothing while
        // reading to the caller as if it had silenced the Behavioral channel.
        for text in ["behaviour", "Behavioral", "behavioral ", "numerics", ""] {
            assert_eq!(Channel::parse(text), None, "{text:?} must not parse");
        }
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
