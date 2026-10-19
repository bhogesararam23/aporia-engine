//! The Trust Atlas as a data structure: an adaptive partition of the parameter space.
//!
//! The specification asks what a Trust Atlas should be in dimensions above three, and the answer
//! here is a recursive bisection of the parameter box. Three properties made it the right choice.
//!
//! It is labelable and printable in any dimension, because a cell is a box plus a small summary and
//! never a surface to draw. It refinement-locally: a cell that straddles a boundary is split and
//! its children are measured again, so resolution follows evidence instead of being fixed by a grid.
//! And it degenerates correctly: in one dimension it is an interval tree that reproduces a bisection
//! search, which is what lets the boundary-precision metric be checked against a known threshold.
//!
//! What it does not do is represent a curved boundary cheaply — a diagonal transition in 2D needs
//! many small cells. That is the cost of a representation that can be described in a CSV, and the
//! atlas records cell counts so the reader can see the cost rather than infer it.

use aporia_ir::{Domain, Model};

/// One measurement inside a cell: the record it came from, where it was, what it was worth and
/// which channels spoke. Keeping the point itself is what lets a split put each measurement in the
/// right child instead of discarding it.
#[derive(Clone, Debug, PartialEq)]
pub struct Point {
    pub observation: u64,
    pub x: Vec<f64>,
    pub risk: f64,
    /// Which channels produced evidence about this evaluation.
    pub channels: u8,
    /// Whether one of those channels stated a fact rather than a measurement: a rule that fired, an
    /// output that left the real numbers. See [`Policy::suspicious_channels`].
    pub fact: bool,
    /// Which channels were *applied* to this evaluation, whether or not they had anything to say.
    ///
    /// The distinction is what makes TRUSTED reachable. A cell that was checked against the model's
    /// own rules and its own local response and produced no evidence has `channels == 0` by
    /// construction, so a policy that demands "at least one channel must have spoken" can only ever
    /// be satisfied by a cell that has a finding in it — which means no clean region of any model can
    /// be called trusted, and measured exactly that: an atlas of a one-dimensional model with one
    /// known boundary reported `trusted 0.0000, unknown 0.5347` at a 1200 budget and produced no band
    /// at all, because a band needs two labelled cells that disagree.
    pub measured: u8,
}

/// An axis-aligned box in parameter space, stored as half-open-ish inclusive bounds per axis.
#[derive(Clone, Debug, PartialEq)]
pub struct Cell {
    /// `[axis][0 = low, 1 = high]`, in the units the parameters were declared in.
    pub bounds: Vec<[f64; 2]>,
    pub id: u32,
    pub depth: u8,
    /// How many executions have landed inside this cell.
    pub samples: u32,
    /// Sum of risk scores. `points` already holds every value, but keeping the sum means the
    /// policy can classify a cell without walking them.
    pub risk_sum: f64,
    /// Highest risk seen in the cell. A mean hides a narrow failure; a maximum does not.
    pub risk_max: f64,
    pub label: Label,
    /// Which channels have spoken inside this cell. A cell that only ever heard one channel is
    /// reported as less well understood than one that heard four.
    pub channels: u8,
    /// How many evaluations inside this cell carried an absolute fact. Zero does not mean the cell is
    /// clean; it means whatever is wrong here is wrong by measurement rather than by the model's own
    /// declaration. See [`Policy::suspicious_channels`].
    pub facts: u32,
    /// Which channels have been applied inside this cell. See [`Point::measured`]: this is the mask
    /// an earned TRUSTED is judged against, and `channels` is not.
    pub measured: u8,
    /// Every evaluation that landed here, with the record id, risk and channel mask that came with
    /// it. Kept because the alternative is worse: a split that dropped the parent's measurements
    /// left children with no evidence, so an atlas could become entirely UNKNOWN by refining
    /// itself. With the points present, a split moves each measurement into whichever child now
    /// contains it, labels survive refinement, and a band can only shrink because the evidence
    /// says so.
    pub points: Vec<Point>,
}

impl Cell {
    /// Recompute the aggregates from `points`, after a split moved them between cells.
    pub fn recompute(&mut self) {
        self.samples = self.points.len() as u32;
        self.risk_sum = self.points.iter().map(|p| p.risk).sum();
        self.risk_max = self.points.iter().fold(0.0f64, |a, p| a.max(p.risk));
        self.channels = self.points.iter().fold(0u8, |a, p| a | p.channels);
        self.measured = self.points.iter().fold(0u8, |a, p| a | p.measured);
        self.facts = self.points.iter().filter(|p| p.fact).count() as u32;
        self.label = Label::Unknown;
    }

    /// Every evaluation this cell has seen, oldest first.
    pub fn risks(&self) -> impl Iterator<Item = f64> + '_ {
        self.points.iter().map(|p| p.risk)
    }
}

impl Cell {
    #[must_use]
    pub fn width(&self, axis: usize) -> f64 {
        self.bounds[axis][1] - self.bounds[axis][0]
    }

    #[must_use]
    pub fn center(&self, axis: usize) -> f64 {
        let [lo, hi] = self.bounds[axis];
        lo + (hi - lo) / 2.0
    }

    #[must_use]
    pub fn contains(&self, x: &[f64]) -> bool {
        x.len() == self.bounds.len()
            && self
                .bounds
                .iter()
                .zip(x)
                .all(|([lo, hi], v)| *v >= *lo && *v <= *hi)
    }

    #[must_use]
    pub fn mean_risk(&self) -> f64 {
        if self.samples == 0 {
            0.0
        } else {
            self.risk_sum / self.samples as f64
        }
    }

    /// Volume relative to the root cell, which is how "how much of the space does this cover" is
    /// answered without pretending a 12-dimensional volume is meaningful to a reader.
    #[must_use]
    pub fn relative_size(&self, root: &Cell) -> f64 {
        let mut product = 1.0;
        for axis in 0..self.bounds.len() {
            let total = root.width(axis);
            if total > 0.0 {
                product *= self.width(axis) / total;
            }
        }
        product
    }

    #[must_use]
    pub fn touches(&self, other: &Cell) -> bool {
        self.bounds
            .iter()
            .zip(&other.bounds)
            .all(|([a0, a1], [b0, b1])| {
                // Touching counts: two cells that meet at a face are neighbours, and a boundary between
                // them is real even though they share no volume.
                a1 >= b0 && b1 >= a0
            })
    }
}

/// What the atlas says about a cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Label {
    /// Not enough evidence to classify. The default, and the honest answer more often than a reader
    /// will expect.
    Unknown,
    /// Sampled, nothing unusual seen. Never "proven correct".
    Trusted,
    /// Sampled and quiet, and coarser than the resolution this instrument certifies at.
    ///
    /// The state exists because [`Label::Trusted`] was being handed out at whatever scale the search
    /// happened to reach: [`Atlas::refine`] cuts a leaf that is suspicious, that carries risk, or that
    /// has a disagreeing neighbour, and never merely because it is coarse — so a leaf that no
    /// evaluation had troubled was certified as though it had been examined. `PROVISIONAL` says
    /// something narrower than `TRUSTED` and something more than `UNKNOWN`: nothing was found at this
    /// scale, and this scale is not the one a claim is made at.
    ///
    /// It is not a softer `UNKNOWN`, and the difference is a measurement rather than a definition.
    /// `UNKNOWN` is reached by a leaf with too few samples, no applied channel, or a reading that is
    /// not a number; those do not become certifiable by bisecting the cell. A `PROVISIONAL` leaf holds
    /// the samples and the channels already, so refinement can turn it into either of the definite
    /// labels without buying evidence — which is why E1.4 pairs this state with an arm that demands the
    /// refinement and measures whether the two states separate.
    ///
    /// Reached only under a [`Resolution`] that withholds certification. Under
    /// [`Resolution::Current`], which every committed measurement in this repository used, no leaf is
    /// ever labelled `PROVISIONAL`.
    Provisional,
    /// Evidence points at a problem somewhere in here.
    Suspicious,
}

impl Label {
    #[must_use]
    pub fn code(self) -> char {
        match self {
            Self::Unknown => '?',
            Self::Trusted => 'T',
            Self::Provisional => 'P',
            Self::Suspicious => 'S',
        }
    }

    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Unknown => "UNKNOWN",
            Self::Trusted => "TRUSTED",
            Self::Provisional => "PROVISIONAL",
            Self::Suspicious => "SUSPICIOUS",
        }
    }

    /// Which cells count as disagreeing for the purposes of refinement and boundary bands.
    ///
    /// `PROVISIONAL` shares a class with `TRUSTED` deliberately, and it is the reason this method
    /// exists instead of a bare `!=` on the label: an arm that withholds certification must not also
    /// move the partition, or the difference between it and the control would be a difference in where
    /// the search went rather than in what the atlas claims. With this class, a partition built under
    /// [`Resolution::Current`] and one built under [`Resolution::Withhold`] are the same partition,
    /// cell for cell, and the only thing that differs is the word on it.
    #[must_use]
    pub fn adjacency(self) -> u8 {
        match self {
            Self::Unknown => 0,
            Self::Suspicious => 1,
            Self::Trusted | Self::Provisional => 2,
        }
    }
}

/// Which truth-blind extra requirement a quiet leaf must satisfy before it may be called `Trusted`.
///
/// A named variant rather than a knob, and the reason is the one E1.3 left behind. `min_channels` and
/// a non-finite rule are both reachable already from a caller that builds a [`Policy`] by hand, so
/// what a measurement means cannot be "whatever numbers the caller happened to set": it has to be the
/// rule the report names. `Current` is not a neutral default sitting beside three interesting
/// options — it *is* the semantics that produced every committed number in this repository, and an
/// experiment that names it is obliged to produce those numbers again unchanged. The other three are
/// the two live mechanisms 0038's post-mortem found, plus their conjunction: a leaf certified on one
/// sensor's silence, a leaf certified although it holds a reading that is not a real number, and both
/// at once — because two rules that each remove some over-claim are not one rule that removes it
/// twice, and that has to be measured rather than assumed.
///
/// None of them reads a declared region. That is a requirement and not a style preference: a trust
/// rule that consulted truth would make the instrument mark its own homework, which is what 0037's
/// correction C8 was written to keep out of the corpus. Every variant here is a function of
/// statistics the atlas already holds for its own reasons.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Trust {
    /// Today's rule and every committed measurement's: `min_samples` evaluations, at least
    /// [`Policy::min_channels`] channels applied, nothing flagged. Adding a variant must leave the
    /// numbers this one produces exactly where they are.
    Current,
    /// At least two distinct channels must have been applied inside the leaf before it may be
    /// trusted, whatever the policy's own floor says. One sensor looking at a quiet region and
    /// finding nothing is a thinner claim than two doing it.
    MinChannelsTwo,
    /// A leaf holding a reading that is not a real number may be suspicious and may be unknown, but
    /// it may not be trusted.
    ///
    /// This is wider than it sounds. `classify` already refuses NaN, because every comparison against
    /// NaN is false and a poisoned cell used to fall out of the bottom of that function labelled
    /// `Trusted`. What it does *not* do is look at the readings: an aggregate can hide one.
    /// [`Cell::recompute`] folds `risk_max` starting at `0.0`, so a leaf whose only unusual reading is
    /// `-inf` keeps a finite maximum and a mean below the flag bar, and under [`Trust::Current`] a
    /// leaf like that is a trusted one.
    NonFiniteBlocksTrust,
    /// Both of the above, because their interaction is exactly the kind of thing an experiment runs to
    /// find out.
    BothRules,
}

impl Trust {
    /// Every variant, in the order a report lists them, [`Trust::Current`] first because it is the one
    /// every other is measured against.
    pub const ALL: [Trust; 4] = [
        Trust::Current,
        Trust::MinChannelsTwo,
        Trust::NonFiniteBlocksTrust,
        Trust::BothRules,
    ];

    /// The names a plan, a protocol file and an archive use for a variant. One name each, in both
    /// directions — the discipline that killed the `halton` alias in `aporia_search::Strategy`.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::MinChannelsTwo => "min-channels-2",
            Self::NonFiniteBlocksTrust => "non-finite-blocks-trust",
            Self::BothRules => "both-trust-rules",
        }
    }

    /// Exactly the names [`Trust::name`] produces, so a refusal can never list a vocabulary the parser
    /// has stopped accepting.
    #[must_use]
    pub fn names() -> [&'static str; 4] {
        [
            Self::Current.name(),
            Self::MinChannelsTwo.name(),
            Self::NonFiniteBlocksTrust.name(),
            Self::BothRules.name(),
        ]
    }

    /// Parse a variant name. An unknown name is `None` and every caller of this refuses it: a trust
    /// rule that fell back to a default would silently re-measure a published experiment.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "current" => Self::Current,
            "min-channels-2" => Self::MinChannelsTwo,
            "non-finite-blocks-trust" => Self::NonFiniteBlocksTrust,
            "both-trust-rules" => Self::BothRules,
            _ => return None,
        })
    }

    /// The channel floor this variant demands on top of the policy's own, or zero when it demands
    /// none. Zero rather than one so that [`Policy::min_channels_required`] is the *stricter* of the
    /// two numbers and never a third opinion.
    #[must_use]
    pub fn min_channels_floor(self) -> u32 {
        match self {
            Self::Current | Self::NonFiniteBlocksTrust => 0,
            Self::MinChannelsTwo | Self::BothRules => 2,
        }
    }

    /// Whether a non-real reading inside a leaf blocks `Trusted` under this variant.
    #[must_use]
    pub fn blocks_non_finite(self) -> bool {
        matches!(self, Self::NonFiniteBlocksTrust | Self::BothRules)
    }

    /// The policy a variant names, over the default thresholds. This is the one place a name becomes a
    /// [`Policy`]; a plan that carries a variant uses it here rather than keeping a second copy of the
    /// numbers.
    #[must_use]
    pub fn policy(self) -> Policy {
        Policy {
            trust: self,
            ..Policy::default()
        }
    }
}

/// What the instrument requires of a cell's *scale* before it certifies it, and what it does when the
/// scale is not there.
///
/// The other axis on [`Policy`] moves what a trusted leaf must have *in* it — evaluations, applied
/// channels, real numbers — and [`Trust`] records the measured result of tightening those: on a corpus
/// chosen to be resolvable, no truth-blind requirement reduced certified-over-true volume, and the one
/// arm that reached zero did it by trusting nothing at all. This axis is about the geometry of the leaf
/// rather than its evidence, and it asks whether a leaf that is quiet because nothing was ever looked
/// for inside it is a different claim from a leaf that is quiet because looking did not find anything.
///
/// Two things can be demanded, independently, which is why this is a four-variant grid rather than a
/// list of rules:
///
/// * *demanding resolution* changes the partition — [`Atlas::refine`] bisects a quiet cell that is
///   coarser than [`Policy::certify_share`] even though nothing about its evidence says to;
/// * *withholding certification* changes only what the partition claims — a quiet coarse cell becomes
///   [`Label::Provisional`] and is counted as neither certified nor unmeasurable.
///
/// `Withhold` alone leaves the cells where they are, by [`Label::adjacency`], so it can be compared
/// with `Current` as a difference in claims and nothing else. `Demand` alone leaves the labels where
/// they are, so a fall in certified volume there is a fall caused by the partition actually going
/// finer — and, since a split shares the parent's samples out, by some children then failing
/// [`Policy::min_samples`] and becoming `UNKNOWN`. Whether that trade is worth anything is what E1.4
/// measures; nothing in this type pretends it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resolution {
    /// The semantics every committed measurement in this repository was made under: a leaf that is
    /// sampled, measured and unflagged is trusted whatever its size. A new variant must leave the
    /// labels and the partition this one produces exactly where they are.
    Current,
    /// Bisect a quiet leaf while it is coarser than [`Policy::certify_share`], subject to the same
    /// sample floor and depth cap the committed rule already obeys.
    Demand,
    /// Do not certify a quiet leaf that is coarser than [`Policy::certify_share`]: label it
    /// [`Label::Provisional`] instead.
    Withhold,
    /// Both, because the combination is the one a reader will actually want — look closer, and hold
    /// back the claim on whatever still cannot be resolved — and because it is the only arm that can
    /// show whether the two axes are independent or interfere.
    DemandAndWithhold,
}

impl Resolution {
    /// Every variant, in the order a report lists them, [`Resolution::Current`] first because it is the
    /// one every other is measured against.
    pub const ALL: [Resolution; 4] = [
        Resolution::Current,
        Resolution::Demand,
        Resolution::Withhold,
        Resolution::DemandAndWithhold,
    ];

    /// The names a plan, a protocol file and an archive use for a variant. One name each, in both
    /// directions, for the reason that killed the `halton` alias in `aporia_search::Strategy`.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Demand => "demand",
            Self::Withhold => "withhold",
            Self::DemandAndWithhold => "demand-and-withhold",
        }
    }

    /// Exactly the names [`Resolution::name`] produces, so a refusal can never list a vocabulary the
    /// parser has stopped accepting.
    #[must_use]
    pub fn names() -> [&'static str; 4] {
        [
            Self::Current.name(),
            Self::Demand.name(),
            Self::Withhold.name(),
            Self::DemandAndWithhold.name(),
        ]
    }

    /// Parse a variant name. An unknown name is `None` and every caller refuses it: a resolution rule
    /// that fell back to a default would re-measure a published experiment without saying so.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "current" => Self::Current,
            "demand" => Self::Demand,
            "withhold" => Self::Withhold,
            "demand-and-withhold" => Self::DemandAndWithhold,
            _ => return None,
        })
    }

    /// Whether this variant bisects quiet cells that are too coarse to certify.
    #[must_use]
    pub fn demands(self) -> bool {
        matches!(self, Self::Demand | Self::DemandAndWithhold)
    }

    /// Whether this withholds certification from a quiet cell that is too coarse to certify.
    #[must_use]
    pub fn withholds(self) -> bool {
        matches!(self, Self::Withhold | Self::DemandAndWithhold)
    }
}

/// What the instrument measures a leaf's *adequacy* against, when a [`Resolution`] demands one.
///
/// The two candidates are the two things a quiet coarse leaf can be quiet for, and E1.5's whole design
/// turns on them coming apart. [`Resolution`] asks a question about geometry — how big is the cell you
/// are signing off — and this axis asks a question about evidence — how many of the run's evaluations
/// actually landed inside it. A leaf bisected to the frozen bar and holding exactly the sample floor
/// passes both rules; a leaf that holds forty evaluations in half the domain passes the size rule and
/// fails nothing, while a leaf that holds four in a quarter of it fails the density rule and looks
/// resolved by size. Which of the two the instrument should condition certification on is a measurement,
/// not a preference, and an arm exists for each.
///
/// [`Certify::Size`] is the default and the committed reading: it is exactly the predicate every
/// published row was labelled with, and a new variant must leave its answers where they are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Certify {
    /// A leaf is uncertifiable while it is coarser than [`Policy::certify_share`] of its root — the
    /// reading [`Policy::coarse`] implements and every committed measurement used.
    Size,
    /// A leaf is uncertifiable while its evaluations-per-domain-volume is below
    /// [`Policy::certify_density`]: the space was measured, but not densely enough to say anything
    /// about a region of the size this instrument claims at.
    Density,
}

impl Certify {
    /// Every variant, with the committed reading first for the reason [`Resolution::ALL`] gives.
    pub const ALL: [Certify; 2] = [Certify::Size, Certify::Density];

    /// The names a plan, a protocol file and an archive use for a variant. One name each, in both
    /// directions.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Size => "size",
            Self::Density => "density",
        }
    }

    #[must_use]
    pub fn names() -> [&'static str; 2] {
        [Self::Size.name(), Self::Density.name()]
    }

    /// Parse a variant name; an unknown name is `None` and every caller refuses it.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "size" => Self::Size,
            "density" => Self::Density,
            _ => return None,
        })
    }
}

/// The thresholds that decide a label, and nothing else. Held apart from the partition so an
/// experiment can record exactly what it meant by "suspicious".
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Policy {
    /// Mean risk at or above which a cell is suspicious.
    pub suspicious_mean: f64,
    /// Peak risk at or above which a cell is suspicious, whatever its mean, because a narrow failure
    /// inside a large cell is still a failure.
    pub suspicious_peak: f64,
    /// Below this many samples a cell cannot be called trusted, only unknown.
    pub min_samples: u32,
    /// How many distinct channels must have been *applied* inside a cell for a "trusted" to be
    /// earned: one sensor looking at a point and finding nothing is a thinner claim than four looking
    /// at it and finding nothing.
    ///
    /// Judged against [`Cell::measured`], not [`Cell::channels`]. The mask of channels that *spoke* is
    /// empty exactly where a cell is clean, so requiring a channel to have spoken would make TRUSTED
    /// unreachable — which is what it was, and what made every atlas report nothing but UNKNOWN and
    /// SUSPICIOUS and therefore no bands at all.
    pub min_channels: u32,
    /// Depth at which a cell stops being split, so the atlas cannot grow without bound.
    pub max_depth: u8,
    /// Mean risk below which a cell is worth splitting again looking for the transition.
    pub refine_below: f64,
    /// How many channels must have spoken about the *same evaluation* for that evaluation to make a
    /// cell suspicious on measurement alone.
    ///
    /// One channel's opinion, however loud, is a lead: it says this quantity behaves unusually here,
    /// which a correct model does all the time. A square-root domain has an infinite slope at the
    /// edge of its own legal region; a symplectic integrator over a long horizon genuinely swings
    /// 700x harder in one part of its step-size range than in another. Those are true statements
    /// about the region and not defects in the model, and after every measurement fix in `0014` and
    /// `0015` they are what remained: no control sweep in the ladder was clean at every budget, and
    /// the flags left were single-channel sensitivity.
    ///
    /// An evaluation carrying an absolute item — a rule that fired, an output that left the real
    /// numbers — clears the bar on its own, because that is not an opinion about the model's
    /// behaviour but a fact against the model's own declaration.
    pub suspicious_channels: u32,
    /// Which extra truth-blind requirement [`Policy::classify`] applies before it hands out `Trusted`.
    ///
    /// [`Trust::Current`] is the semantics every committed measurement in this repository was made
    /// under, and it is not decoration around the numbers above: the variant is part of what a run
    /// means, so it travels into the recorded configuration, into the experiment plan and its
    /// measurement identity whenever it is not `Current`, and into the archive directory name. See
    /// [`Trust`].
    pub trust: Trust,
    /// The scale below which this instrument will not certify a leaf, as a fraction of the root cell's
    /// volume. Read only when [`Policy::resolution`] is not [`Resolution::Current`], and inert at that
    /// point rather than decorative: it is the single number the two resolution rules act on, so an arm
    /// that demands more resolution and an arm that withholds certification can be compared at all.
    ///
    /// Expressed as a fraction rather than a depth because a depth would be a statement about this
    /// partition rule's bisection and a fraction is a statement about the space. The value the
    /// experiments freeze is 2^-8: four bisections above the finest leaf the default depth cap permits.
    pub certify_share: f64,
    /// Which requirement, if any, this policy places on a leaf's *scale* before it certifies it, and
    /// what it does when the scale is not there. See [`Resolution`].
    pub resolution: Resolution,
    /// What a leaf's adequacy is measured against when [`Policy::resolution`] is not
    /// [`Resolution::Current`]: its geometric scale or its evidence density. See [`Certify`].
    ///
    /// [`Certify::Size`] reproduces the committed predicate exactly, so this field is inert at its
    /// default and a run that does not name it is the run every published row came from.
    pub certify: Certify,
    /// The evaluations per domain-volume a leaf must hold before this policy will certify it, read only
    /// under [`Certify::Density`].
    ///
    /// Its default is not a second tuned constant: it is what the size bar and the sample floor jointly
    /// imply, [`Policy::min_samples`] evaluations inside a leaf of [`Policy::certify_share`] of the
    /// domain — `4 / 2⁻⁸ = 1024`. The two rules therefore disagree only where the arithmetic says they
    /// must, on a leaf that is coarse and densely measured or fine and thinly measured, and a
    /// difference between an arm under one and an arm under the other is a difference about which of
    /// the two quantities certification was resting on.
    pub certify_density: f64,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            suspicious_mean: 0.45,
            suspicious_peak: 0.7,
            min_samples: 4,
            min_channels: 1,
            max_depth: 12,
            refine_below: 0.12,
            suspicious_channels: 2,
            trust: Trust::Current,
            // 2^-8, four bisections above the finest leaf `max_depth` above permits. Inert under the
            // `resolution` default, and stated rather than left at zero so that a policy built by
            // hand and switched on measures something a reader can name.
            certify_share: 0.003_906_25,
            resolution: Resolution::Current,
            certify: Certify::Size,
            // min_samples over certify_share: four evaluations inside a leaf of 2⁻⁸ of the domain is
            // one per 2⁻¹⁰ of it. Derived from the two constants above rather than chosen beside them,
            // so the two criteria start at the same operating point.
            certify_density: 1_024.0,
        }
    }
}

impl Policy {
    /// How many distinct channels must have been applied inside a leaf for this policy to trust it:
    /// the policy's own floor, or the trust variant's, whichever is *stricter*.
    ///
    /// Deliberately a maximum rather than an override. Two places could otherwise state the bar — the
    /// field and the variant — and a caller that set `min_channels: 1` under `Trust::MinChannelsTwo`
    /// would be measured under whichever number the code happened to read last. Under
    /// [`Trust::Current`] the floor is zero and this returns [`Policy::min_channels`] exactly, which is
    /// what keeps the committed measurements' bar where it was.
    #[must_use]
    pub fn min_channels_required(&self) -> u32 {
        self.min_channels.max(self.trust.min_channels_floor())
    }

    /// Whether a leaf holding a reading that is not a real number may be trusted under this policy.
    #[must_use]
    pub fn blocks_non_finite(&self) -> bool {
        self.trust.blocks_non_finite()
    }

    /// Whether a leaf is coarser than the scale this policy certifies at.
    ///
    /// A split bisects one axis of one cell, so a leaf at depth `d` holds exactly 2^-`d` of its root's
    /// volume whichever axis was chosen — which is what lets this predicate read the depth the cell
    /// already carries instead of reconstructing a volume from the root, and what makes it agree with
    /// [`Cell::relative_size`]. A test pins that agreement, because the metric reports volume and this
    /// hands out labels, and the two have to mean the same cell by "too coarse".
    ///
    /// Deliberately independent of [`Policy::resolution`]: whether a cell was certified at a scale the
    /// experiment calls too coarse has to be answerable of an arm that never asked the question, or the
    /// control cannot be scored on the yardstick the variants are scored on.
    #[must_use]
    pub fn coarse(&self, cell: &Cell) -> bool {
        0.5f64.powi(i32::from(cell.depth)) > self.certify_share
    }

    /// Whether the leaf holds fewer evaluations per domain-volume than [`Policy::certify_density`]
    /// demands — the density predicate on its own, so a report can state what a density rule would
    /// have withheld from an arm that was not running one.
    ///
    /// `2^-depth` is the leaf's volume for this arithmetic by the same argument [`Policy::coarse`]
    /// gives, and the quotient is finite because a depth is a bounded integer and a leaf of depth 0 has
    /// the volume of its own root.
    #[must_use]
    pub fn thinly_measured(&self, cell: &Cell) -> bool {
        let volume = 0.5f64.powi(i32::from(cell.depth));
        f64::from(cell.samples) / volume < self.certify_density
    }

    /// Whether this policy will certify the leaf at all, under whichever criterion [`Policy::certify`]
    /// names. `true` means it will not: the leaf is short of the scale the policy claims at, or short
    /// of the evidence its claim would need.
    ///
    /// Under [`Certify::Size`] this is exactly [`Policy::coarse`], which is what makes the density axis
    /// an addition rather than a revision: every committed label, every `coarse_certified_volume` and
    /// every reproduction of a published row goes through this function and gets the answer it got
    /// before.
    ///
    /// A leaf with no samples is uncertifiable under the density rule by construction — zero
    /// evaluations of any volume is zero — and cannot be under the size rule, because [`Policy::classify`]
    /// sends an under-sampled leaf to `UNKNOWN` long before a scale question is asked.
    #[must_use]
    pub fn unsupported(&self, cell: &Cell) -> bool {
        match self.certify {
            Certify::Size => self.coarse(cell),
            Certify::Density => self.thinly_measured(cell),
        }
    }

    /// Whether a leaf's label is a claim of quietness rather than of trouble or of no evidence: the
    /// class that certification is granted in, and the class a resolution rule is allowed to act on.
    /// [`Label::Provisional`] is in it because it is quiet, and out of it as a *claim* because
    /// withholding is what puts it there.
    #[must_use]
    pub fn quiet(&self, cell: &Cell) -> bool {
        cell.label.adjacency() == Label::Trusted.adjacency()
    }

    /// Classify one cell from its accumulated statistics.
    #[must_use]
    pub fn classify(&self, cell: &Cell) -> Label {
        // A reading that is not a number is not a clean bill either. Every comparison below is false
        // against NaN, so a poisoned cell used to fall through the bottom of this function labelled
        // `Trusted` — the one outcome this instrument must never hand out for want of information.
        // `fuse` keeps scores finite, so this guards a caller that did not.
        //
        // Only NaN is refused here. An infinite risk is an extreme *value*, which the ordinary
        // comparison reads as flagged, and calling it unmeasurable would be the opposite mistake.
        if cell.mean_risk().is_nan() || cell.risk_max.is_nan() {
            return Label::Unknown;
        }
        let flagged =
            cell.risk_max >= self.suspicious_peak || cell.mean_risk() >= self.suspicious_mean;
        if flagged {
            // The cell looks troubled in aggregate; say so only for an evaluation that either holds
            // a fact or was seen the same way by two independent channels.
            if cell.points.iter().any(|p| {
                p.risk >= self.suspicious_mean
                    && (p.fact || p.channels.count_ones() >= self.suspicious_channels)
            }) {
                return Label::Suspicious;
            }
            return Label::Unknown;
        }
        if cell.samples < self.min_samples {
            return Label::Unknown;
        }
        if self.blocks_non_finite() && cell.points.iter().any(|p| !p.risk.is_finite()) {
            // Reachable as a code path only under a variant: the branch above already catches a NaN
            // aggregate and everything at or above the peak bar, so what is left here is a leaf whose
            // *individual* reading left the real numbers without moving either aggregate. A `-inf`
            // risk is that case — [`Cell::recompute`] folds the maximum from `0.0`, so it does not
            // lower `risk_max`, and a mean below the bar keeps the cell unflagged.
            //
            // Whether any campaign ever produces such a reading is a measurement and not a fact of this
            // function: `aporia_evidence::fuse` clamps its score into `[0, 1]`, so a search built on
            // fused scores has no non-real risk to record. E1.3b counts non-finite readings per row for
            // exactly that reason — a variant that never fires is reported as never firing, not as a
            // rule that made the instrument safer.
            return Label::Unknown;
        }
        if cell.measured.count_ones() < self.min_channels_required() {
            return Label::Unknown;
        }
        if self.resolution.withholds() && self.unsupported(cell) {
            // Everything above says this leaf is sampled, measured and quiet, and this line asks the one
            // question [`Trust`] never asks: was it measured at the scale this policy claims at, which
            // [`Policy::certify`] decides between a leaf's size and its sample density. Reaching it is a
            // fact about the cell rather than about any declared region, so unlike the two committed
            // rules this one fires wherever the partition is short of the bar — which is why it is priced
            // on a clean entry as well as on a violated one, and why a reader gets to see the price.
            //
            // It says nothing about whether a violation is inside. The instrument is still truth-blind
            // here, and the claim withheld is certification, not knowledge: `PROVISIONAL` is the atlas
            // reporting that it did not look closely enough to promise anything, which is a smaller
            // claim than `TRUSTED` and a larger one than `UNKNOWN`.
            return Label::Provisional;
        }
        Label::Trusted
    }
}

/// The atlas itself: a forest of cells, index 0 being the root.
#[derive(Clone, Debug)]
pub struct Atlas {
    pub cells: Vec<Cell>,
    /// Ids of cells that are leaves: no other cell lists them as a parent.
    pub leaves: Vec<u32>,
    policy: Policy,
    observations: u64,
    /// Measurements that fell inside no leaf, and so live on the root rather than in the partition.
    /// Counted because a point that cannot be placed is a fact about the run a reader is entitled to,
    /// and silent because until it was counted it was exactly how a third of a model's evaluations
    /// vanished from its own atlas. See [`Atlas::leaf_of`].
    unplaced: u32,
}

impl Atlas {
    /// Build the root cell from a model's declared domains.
    ///
    /// Every axis is bounded in the coordinates the points actually use, because the atlas is a
    /// partition of the parameter space and those coordinates are what the model is evaluated at, what
    /// the records store, what a finding's bounds report, and what a `truth.json` region declares. A
    /// discrete choice set is bounded by the extent of its values for the same reason: the sampler
    /// hands back an option, not an index, and bisecting a numeric interval that happens to be sampled
    /// discretely is still meaningful.
    #[must_use]
    pub fn new(model: &Model, policy: Policy) -> Self {
        let bounds = model
            .params
            .iter()
            .map(|p| match &p.domain {
                Domain::Interval { lo, hi } => [*lo, *hi],
                Domain::Choices(v) => {
                    let first = v.first().copied().unwrap_or(0.0);
                    [
                        v.iter().fold(first, |acc, x| acc.min(*x)),
                        v.iter().fold(first, |acc, x| acc.max(*x)),
                    ]
                }
            })
            .collect();
        let root = Cell {
            bounds,
            id: 0,
            depth: 0,
            samples: 0,
            risk_sum: 0.0,
            risk_max: 0.0,
            label: Label::Unknown,
            channels: 0,
            facts: 0,
            measured: 0,
            points: Vec::new(),
        };
        Self {
            cells: vec![root],
            leaves: vec![0],
            policy,
            observations: 0,
            unplaced: 0,
        }
    }

    #[must_use]
    pub fn policy(&self) -> Policy {
        self.policy
    }

    #[must_use]
    pub fn root(&self) -> &Cell {
        &self.cells[0]
    }

    #[must_use]
    pub fn cell(&self, id: u32) -> Option<&Cell> {
        self.cells.get(id as usize)
    }

    #[must_use]
    pub fn leaf_ids(&self) -> &[u32] {
        &self.leaves
    }

    /// The leaf cell containing a point, creating the partition lazily is not needed: points are
    /// always assigned to whatever leaf exists.
    /// The leaf that contains `x`, or `None` when no leaf does.
    ///
    /// `None` is the honest answer, and the one that used to be silently swallowed: a point that
    /// belongs to no leaf was folded into cell 0, which stopped being a leaf at the first split, so its
    /// measurement left the labels, the coverage and the findings without saying so. [`Atlas::locate`]
    /// still needs a cell for callers that only route, so it keeps the root as its fallback and counts
    /// the event instead.
    #[must_use]
    pub fn leaf_of(&self, x: &[f64]) -> Option<u32> {
        self.leaves
            .iter()
            .copied()
            .find(|id| self.cells[*id as usize].contains(x))
    }

    /// The cell `x` falls in, defaulting to the root when it falls in nothing. Prefer
    /// [`Atlas::leaf_of`], which reports the failure rather than absorbing it; every use of this one
    /// is a place that has to name a cell anyway.
    #[must_use]
    pub fn locate(&self, x: &[f64]) -> u32 {
        self.leaf_of(x).unwrap_or_default()
    }

    /// The cell a measurement is filed under, counting the case where no leaf claims it. Used by the
    /// two places that attach a point to the partition; `locate` is for routing that does not record.
    fn place(&mut self, x: &[f64]) -> u32 {
        let leaf = self.leaf_of(x);
        if leaf.is_none() {
            self.unplaced += 1;
        }
        leaf.unwrap_or_default()
    }

    /// Record one evaluation.
    ///
    /// The observation id is the atlas' own counter, assigned in the order points were handed in — it
    /// is *not* the campaign's record index, because a campaign records probe and swap evaluations it
    /// does not offer the atlas. The ids that end up attached to a labelled cell are the ones
    /// [`Atlas::remeasure`] installs from the finished record set, which is the pass that produces the
    /// report; this counter only orders the online stream the search steers on.
    pub fn record(&mut self, x: &[f64], risk: f64, channels: u8) {
        // For a caller that only knows what spoke: whatever had something to say was, necessarily,
        // applied. The other direction does not hold, which is why a search that wants TRUSTED to be
        // reachable builds the whole point itself.
        self.record_point(Point {
            observation: 0,
            x: x.to_vec(),
            risk,
            channels,
            measured: channels,
            fact: false,
        });
    }

    /// Record one evaluation given in full, and return the observation id it was assigned.
    ///
    /// `observation` is filled in here, in evaluation order, so a caller that constructed the point
    /// with the record id it already holds gets the same number back.
    pub fn record_point(&mut self, mut p: Point) -> u64 {
        let observation = self.observations;
        self.observations += 1;
        p.observation = observation;
        let id = self.place(&p.x) as usize;
        let cell = &mut self.cells[id];
        cell.measured |= p.measured;
        cell.channels |= p.channels;
        cell.facts += u32::from(p.fact);
        cell.samples += 1;
        cell.risk_sum += p.risk;
        cell.risk_max = cell.risk_max.max(p.risk);
        cell.points.push(p);
        observation
    }

    /// Rebuild every cell's measurements from a finished set of evaluations, and label again.
    ///
    /// The partition is kept — it is what the search paid for — but the numbers inside it are the
    /// report's. Without this, a cell is labelled from whatever risk the search could see while it
    /// was looking, and any evidence that only exists after the whole record set is in — a declared
    /// relation tested across probes, an inferred pattern, a precision comparison, a sensitivity
    /// reading whose reference is the experiment's own distribution — can never make a cell
    /// suspicious. The map handed to a reader then disagrees with the evidence printed underneath
    /// it, silently.
    ///
    /// Every evaluation lands in exactly one cell, so this neither invents evidence nor loses it,
    /// and it costs no executions: refinement after this point can only re-arrange measurements
    /// that were already paid for.
    pub fn remeasure(&mut self, points: &[Point]) {
        for cell in &mut self.cells {
            cell.points.clear();
        }
        for p in points {
            let id = self.place(&p.x) as usize;
            self.cells[id].points.push(p.clone());
        }
        for cell in &mut self.cells {
            cell.recompute();
        }
        self.relabel();
    }

    /// Re-classify every leaf. Kept separate from `record` so a run can record a batch and then
    /// label once, which is cheaper and makes the label of a cell reproducible from its statistics.
    pub fn relabel(&mut self) {
        for id in self.leaves.clone() {
            let (policy, cell) = (self.policy, &mut self.cells[id as usize]);
            cell.label = policy.classify(cell);
        }
    }

    /// Split every leaf that looks like it straddles a transition: it has seen enough to have an
    /// opinion, it is not already at the depth limit, and either it is suspicious, or its mean risk
    /// says something is inside it, or it sits next to a cell with a different label — or, under a
    /// [`Resolution`] that demands it, it is quiet and coarser than [`Policy::certify_share`].
    ///
    /// That fourth reason is the whole of [`Resolution::Demand`], and it is the only way this partition
    /// ever looks at a quiet cell at all: nothing above fires on a leaf that has been sampled, is
    /// unflagged and has a neighbour with the same claim, so under the committed rule a coarse quiet
    /// leaf is certified at its own scale forever and no label can tell a reader that the scale was
    /// never tested. The reason is restricted to [`Policy::quiet`] cells rather than opened to any
    /// coarse leaf for the reason the paragraph below records: a leaf that is already `UNKNOWN` from
    /// too few samples gets *less* evidence from a split, never more.
    ///
    /// What it costs is measured, not assumed. A split shares the parent's samples out, so demanding
    /// resolution deep enough buys cells that fail [`Policy::min_samples`] and land in `UNKNOWN` — a
    /// fall in certified volume that is a withdrawal of claim rather than a discovery, which is why
    /// E1.4 reports UNKNOWN area and localisation beside the certified volume, and why its decision
    /// rule refuses an arm that stops certifying as anything other than safe.
    ///
    /// The sample requirement applies to *every* reason for splitting, including suspicion. It used
    /// to apply only to the mean-risk branch, and measured what that cost: at a 640 budget the atlas
    /// of a one-dimensional model grew to 881 leaves holding one evaluation each, because every point
    /// that carried absolute evidence — one NaN is a fact, and a fact does not need four samples to be
    /// a fact — drilled to `max_depth` on its own, and no cell ever accumulated the samples a
    /// `TRUSTED` needs. Nothing was trusted, so no two labelled cells disagreed, so no band was
    /// reported, on a model whose boundary is a single known square root. Resolution that follows
    /// evidence cannot be paid for by empty cells: a split has to be worth two measurements before it
    /// is worth making.
    ///
    /// Returns the number of cells created, which is the quantity the search's cost model cares
    /// about.
    pub fn refine(&mut self) -> usize {
        let before = self.cells.len();
        let leaves = self.leaves.clone();
        let mut keep = Vec::with_capacity(leaves.len());
        let mut added = Vec::new();
        for id in leaves {
            let should_split = {
                let cell = &self.cells[id as usize];
                let worth_splitting = cell.samples >= self.policy.min_samples
                    && (cell.label == Label::Suspicious
                        || cell.mean_risk() > self.policy.refine_below
                        || (self.policy.resolution.demands()
                            && self.policy.quiet(cell)
                            && self.policy.unsupported(cell))
                        || self.has_disagreeing_neighbour(id));
                cell.depth < self.policy.max_depth && worth_splitting
            };
            if !should_split {
                keep.push(id);
                continue;
            }
            let children = self.split(id);
            match children {
                Some(ids) => added.extend(ids),
                None => keep.push(id),
            }
        }
        self.leaves
            .retain(|id| keep.contains(id) || added.contains(id));
        // A parent that was split is no longer a leaf, so keep only the survivors and the new ones.
        // The split parents drop out and their children take their place.
        self.leaves = keep.into_iter().chain(added).collect();
        self.relabel();
        self.cells.len() - before
    }

    /// Bisect one cell along the axis with the largest normalised extent, so depth is spent where
    /// the box is fuzziest rather than cycling through axes that are already narrow.
    ///
    /// The parent's measurements are distributed to whichever child now contains them. A point
    /// lands in exactly one child, so refinement can neither invent evidence nor lose it, and a
    /// cell's label always traces back to evaluations that really ran inside it.
    fn split(&mut self, id: u32) -> Option<Vec<u32>> {
        let parent = self.cells.get(id as usize)?.clone();
        let root = self.cells[0].clone();
        let axis = (0..parent.bounds.len()).max_by(|&a, &b| {
            let ra = parent.width(a) / root.width(a).max(1e-30);
            let rb = parent.width(b) / root.width(b).max(1e-30);
            ra.partial_cmp(&rb).unwrap_or(std::cmp::Ordering::Equal)
        })?;
        let mid = parent.center(axis);
        if !(mid.is_finite() && mid > parent.bounds[axis][0] && mid < parent.bounds[axis][1]) {
            return None;
        }
        let base = self.cells.len() as u32;
        let halves: Vec<[f64; 2]> =
            vec![[parent.bounds[axis][0], mid], [mid, parent.bounds[axis][1]]];
        let mut children = Vec::with_capacity(2);
        for (side, bound) in halves.into_iter().enumerate() {
            let mut bounds = parent.bounds.clone();
            bounds[axis] = bound;
            let mut points: Vec<Point> = parent
                .points
                .iter()
                .filter(|p| {
                    let v = p.x.get(axis).copied().unwrap_or(f64::NAN);
                    if side == 0 {
                        v <= bound[1]
                    } else {
                        v > bound[0]
                    }
                })
                .cloned()
                .collect();
            points.sort_by_key(|p| p.observation);
            children.push(Cell {
                bounds,
                id: base + side as u32,
                depth: parent.depth + 1,
                samples: 0,
                risk_sum: 0.0,
                risk_max: 0.0,
                label: Label::Unknown,
                channels: 0,
                facts: 0,
                measured: 0,
                points,
            });
        }
        for child in &mut children {
            child.recompute();
        }
        let ids: Vec<u32> = children.iter().map(|c| c.id).collect();
        // The parent keeps its statistics but stops being a leaf, so it is never double-counted in
        // coverage; `refine` removes it from the leaf list.
        self.cells.extend(children);
        Some(ids)
    }

    /// Every box this model's partition rule is *permitted* to build, from the root down to
    /// [`Policy::max_depth`], produced by [`Atlas::split`] and by nothing else.
    ///
    /// This is the geometry behind a reachability question, and it is stated here rather than in the
    /// crate that asks the question because the answer is only as good as its agreement with the
    /// partition a run actually builds: a second implementation of "what boxes could this atlas have"
    /// is a second implementation that can drift, and the drift would show up as an experiment
    /// declaring a region resolvable that the atlas can never reach.
    ///
    /// It is the whole tree and not only its deepest level, because the quantity a caller wants is the
    /// best overlap any permitted leaf could hold. Every cell that ever exists in a real run is a node
    /// of this tree — refinement only ever splits cells, and the axis it picks depends on the cell's
    /// bounds and the root's, not on what evidence happened to arrive — so no run can produce a box
    /// that is missing here. The *order* in which `refine` reaches them depends on evidence; the set
    /// does not, which is what makes this computable before a run instead of after it.
    ///
    /// `None` when the tree would hold more cells than [`Atlas::PARTITION_CELL_CAP`]. A depth cap is a
    /// promise that the partition is finite, and an enumerator that walks it must not be the thing that
    /// runs out.
    #[must_use]
    pub fn permitted_partition(model: &Model, policy: Policy) -> Option<Vec<Cell>> {
        let mut atlas = Atlas::new(model, policy);
        let mut out: Vec<Cell> = vec![atlas.cells[0].clone()];
        let mut frontier: Vec<u32> = vec![0];
        while !frontier.is_empty() {
            let mut next = Vec::new();
            for id in frontier {
                if atlas.cells[id as usize].depth >= policy.max_depth {
                    continue;
                }
                // `split` is the atlas's own rule: the longest normalised axis, halved, and `None` when
                // the midpoint is not strictly inside the cell. A cell that cannot be split — a
                // zero-width axis, a domain that is a single point — ends its branch here exactly as it
                // would in a run.
                let Some(children) = atlas.split(id) else {
                    continue;
                };
                for child in children {
                    if out.len() >= Self::PARTITION_CELL_CAP {
                        return None;
                    }
                    out.push(atlas.cells[child as usize].clone());
                    next.push(child);
                }
            }
            frontier = next;
        }
        Some(out)
    }

    /// How many boxes [`Atlas::permitted_partition`] will enumerate before refusing. A full binary
    /// tree to depth `d` holds `2^(d+1) - 1` cells, so this cap admits a depth of fifteen — three past
    /// the frozen policy's twelve, whose tree is 8,191 cells, and far past any depth an atlas is asked
    /// to reach in this repository.
    pub const PARTITION_CELL_CAP: usize = 1 << 16;

    fn has_disagreeing_neighbour(&self, id: u32) -> bool {
        let me = &self.cells[id as usize];
        if me.label == Label::Unknown || me.samples == 0 {
            return false;
        }
        self.leaves
            .iter()
            .filter(|other| **other != id)
            .any(|other| {
                let o = &self.cells[*other as usize];
                // Compared by [`Label::adjacency`] and not by equality, so that adding
                // `PROVISIONAL` cannot move a partition: an arm that withholds certification has to be
                // the same atlas as the arm that does not, or the difference between their numbers
                // would be a difference in where the search went and not in what the leaf claims.
                o.samples > 0
                    && o.label != Label::Unknown
                    && o.label.adjacency() != me.label.adjacency()
                    && me.touches(o)
            })
    }

    /// Intervals along each axis that must contain at least one label transition.
    ///
    /// The band is deliberately *conservative*: it is the union of the two disagreeing cells'
    /// extent along the axis that separates them, because a trusted cell may contain the transition
    /// anywhere inside it and so may its neighbour. That is the honest claim, and it is the claim
    /// that shrinks as refinement proceeds, which is what makes "boundary precision versus
    /// evaluations" a measurement rather than an assertion.
    #[must_use]
    pub fn bands(&self) -> Vec<Band> {
        let mut raw: Vec<Band> = Vec::new();
        for (i, &a) in self.leaves.iter().enumerate() {
            for &b in self.leaves.iter().skip(i + 1) {
                let (ca, cb) = (&self.cells[a as usize], &self.cells[b as usize]);
                if ca.samples == 0 || cb.samples == 0 {
                    continue;
                }
                if ca.label == Label::Unknown || cb.label == Label::Unknown {
                    continue;
                }
                if ca.label.adjacency() == cb.label.adjacency() {
                    // Same class, so no transition is claimed between them: `TRUSTED` beside
                    // `PROVISIONAL` is one quiet neighbourhood reported at two scales, not a boundary.
                    continue;
                }
                for axis in 0..ca.bounds.len() {
                    let [a0, a1] = ca.bounds[axis];
                    let [b0, b1] = cb.bounds[axis];
                    // Separated (or merely touching) on this axis, and overlapping on every other
                    // axis, is what "adjacent across a face" means for boxes.
                    let separated = a1 <= b0 + FACE_EPS || b1 <= a0 + FACE_EPS;
                    if !separated {
                        continue;
                    }
                    if !(0..ca.bounds.len()).all(|other| {
                        other == axis
                            || (ca.bounds[other][1] >= cb.bounds[other][0] - FACE_EPS
                                && cb.bounds[other][1] >= ca.bounds[other][0] - FACE_EPS)
                    }) {
                        continue;
                    }
                    raw.push(Band {
                        axis: axis as u16,
                        lo: a0.min(b0),
                        hi: a1.max(b1),
                        facing: [a1.min(b1), a0.max(b0)],
                        transitions: 1,
                    });
                }
            }
        }
        merge_bands(raw)
    }

    /// A one-line summary that goes into the report: how much of the space has been resolved.
    #[must_use]
    pub fn coverage(&self) -> Coverage {
        let root = self.cells[0].clone();
        let (mut trusted, mut suspicious, mut unknown, mut provisional) = (0.0, 0.0, 0.0, 0.0);
        let mut samples = 0;
        for id in &self.leaves {
            let c = &self.cells[*id as usize];
            samples += c.samples;
            let size = c.relative_size(&root);
            match c.label {
                Label::Trusted => trusted += size,
                Label::Suspicious => suspicious += size,
                Label::Provisional => provisional += size,
                Label::Unknown => unknown += size,
            }
        }
        Coverage {
            trusted,
            suspicious,
            unknown,
            provisional,
            cells: self.leaves.len(),
            samples,
            unplaced: self.unplaced,
        }
    }

    /// How many of the readings the finished partition holds in its leaves are not real numbers.
    ///
    /// This is the quantity [`Trust::NonFiniteBlocksTrust`] acts on, and it is counted rather than
    /// assumed because whether any campaign produces such a reading is a measurement and not a fact of
    /// this code: the risk a point carries comes from `aporia_evidence::fuse`, which clamps its score
    /// into `[0, 1]`, so a search built on fused scores may have none at all. E1.3b's frozen secondary
    /// metric asks for the count per row for exactly that reason — a variant that never fires has to be
    /// reported as a variant that never fired, not as a rule that made the instrument safer.
    ///
    /// Leaves only, because a leaf is what [`Policy::classify`] sees, and because a split leaves its
    /// parent's statistics in place — the parent simply drops out of the leaf list — so walking the
    /// cells as well would count one measurement once per generation that held it.
    #[must_use]
    pub fn non_finite_readings(&self) -> u64 {
        self.leaves
            .iter()
            .filter_map(|id| self.cells.get(*id as usize))
            .map(|c| c.points.iter().filter(|p| !p.risk.is_finite()).count() as u64)
            .sum()
    }

    /// Write the atlas as CSV, the machine-readable form a reviewer can open in anything.
    #[must_use]
    pub fn to_csv(&self) -> String {
        use std::fmt::Write as _;
        let root = self.cells[0].clone();
        let mut out = String::new();
        let _ = write!(
            out,
            "cell,depth,label,samples,mean_risk,max_risk,channels,facts,measured,relative_size"
        );
        for i in 0..root.bounds.len() {
            let _ = write!(out, ",lo{i},hi{i}");
        }
        out.push('\n');
        for id in &self.leaves {
            let c = &self.cells[*id as usize];
            let _ = write!(
                out,
                "{},{},{},{},{:.6},{:.6},{},{},{},{:.6}",
                c.id,
                c.depth,
                c.label.name(),
                c.samples,
                c.mean_risk(),
                c.risk_max,
                c.channels,
                c.facts,
                c.measured,
                c.relative_size(&root)
            );
            for axis in 0..c.bounds.len() {
                let _ = write!(out, ",{},{}", c.bounds[axis][0], c.bounds[axis][1]);
            }
            out.push('\n');
        }
        out
    }
}

/// The interval along one axis across which at least one label change was observed.
/// Tolerance for treating two box faces as touching. Cells meet at a shared coordinate, and
/// bisecting in floating point does not always reproduce it bit for bit.
pub const FACE_EPS: f64 = 1e-12;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Band {
    pub axis: u16,
    /// The conservative interval that must contain the transition.
    pub lo: f64,
    pub hi: f64,
    /// The facing edges of the two cells: `[upper edge of the lower cell, lower edge of the upper
    /// cell]`, which for a well-refined atlas is where the transition actually is.
    pub facing: [f64; 2],
    pub transitions: u32,
}

impl Band {
    #[must_use]
    pub fn width(&self) -> f64 {
        self.hi - self.lo
    }

    /// Does this band contain the true transition? Only meaningful against a benchmark with a known
    /// answer, which is why the corpus carries one.
    #[must_use]
    pub fn contains(&self, value: f64) -> bool {
        self.lo - FACE_EPS <= value && value <= self.hi + FACE_EPS
    }
}

/// Merge overlapping bands on the same axis, keeping the number of distinct transitions seen.
///
/// Without this, twenty cells straddling one transition produce twenty bands and a reader cannot
/// tell a wide boundary from many neighbouring ones.
#[must_use]
fn merge_bands(mut raw: Vec<Band>) -> Vec<Band> {
    let mut out: Vec<Band> = Vec::new();
    raw.sort_by(|a, b| {
        a.axis
            .cmp(&b.axis)
            .then(a.lo.partial_cmp(&b.lo).unwrap_or(std::cmp::Ordering::Equal))
    });
    for band in raw {
        match out.last_mut() {
            Some(prev) if prev.axis == band.axis && band.lo <= prev.hi => {
                prev.hi = prev.hi.max(band.hi);
                prev.facing = [
                    prev.facing[0].min(band.facing[0]),
                    prev.facing[1].max(band.facing[1]),
                ];
                prev.transitions += 1;
            }
            _ => out.push(band),
        }
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Coverage {
    pub trusted: f64,
    pub suspicious: f64,
    pub unknown: f64,
    /// Volume the atlas calls quiet and refuses to certify because it is coarser than
    /// [`Policy::certify_share`]. Kept out of both `trusted` and `unknown` on purpose: adding it to
    /// `unknown` would let a policy turn a claim into an uncertainty statistic by renaming it, and
    /// adding it to `trusted` would be the fault this state exists to stop. Under
    /// [`Resolution::Current`] it is always zero, which is what makes the committed measurements'
    /// coverage line up with the numbers published from them.
    pub provisional: f64,
    pub cells: usize,
    pub samples: u32,
    /// Measurements the partition could not place. See [`Atlas::leaf_of`].
    pub unplaced: u32,
}

impl Coverage {
    /// Fraction of the space that carries a definite label. `unknown` is what is left.
    #[must_use]
    pub fn resolved(&self) -> f64 {
        (self.trusted + self.suspicious).clamp(0.0, 1.0)
    }

    /// The figure a *search* credits itself with: everything that is not "we could not measure",
    /// including [`Label::Provisional`].
    ///
    /// Distinct from [`Coverage::resolved`] for one reason, and it is a load-bearing one.
    /// `PROVISIONAL` is `TRUSTED` with the claim withheld, so a progress signal built on `resolved()`
    /// would shrink the moment certification is withheld -- and the acquisition that spends the next
    /// evaluations on a shrinking signal places different points, which builds a different partition.
    /// The control arm and the withholding arm would then be two searches rather than one search making
    /// two claims, and the difference between their numbers would measure point placement instead of
    /// semantics. Measured on the E1.4 pilot before this existed: the two arms disagreed on cell counts
    /// on three of five entries (98 against 93, 303 against 253, 378 against 411) purely because the
    /// label reached the credit.
    #[must_use]
    pub fn credit(&self) -> f64 {
        (self.trusted + self.suspicious + self.provisional).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aporia_ir::Dimension;
    use aporia_ir::{NumType, Param, Ty};

    fn model1d(lo: f64, hi: f64) -> Model {
        let mut m = Model::new("one");
        m.params.push(Param {
            name: "x".into(),
            ty: Ty {
                num: NumType::F64,
                dim: Dimension::dimensionless(),
            },
            domain: Domain::Interval { lo, hi },
            to_si: 1.0,
            doc: String::new(),
        });
        m
    }

    fn modelnd(n: usize) -> Model {
        let mut m = Model::new("many");
        for i in 0..n {
            m.params.push(Param {
                name: format!("p{i}"),
                ty: Ty {
                    num: NumType::F64,
                    dim: Dimension::dimensionless(),
                },
                domain: Domain::Interval {
                    lo: -(i as f64 + 1.0),
                    hi: i as f64 + 1.0,
                },
                to_si: 1.0,
                doc: String::new(),
            });
        }
        m
    }

    #[test]
    fn a_new_atlas_is_one_unknown_cell() {
        let a = Atlas::new(&model1d(0.0, 1.0), Policy::default());
        assert_eq!(a.cells.len(), 1);
        assert_eq!(a.root().label, Label::Unknown);
        assert_eq!(a.coverage().unknown, 1.0);
        assert_eq!(a.coverage().resolved(), 0.0);
    }

    #[test]
    fn samples_accumulate_into_cell_statistics() {
        let mut a = Atlas::new(&model1d(0.0, 1.0), Policy::default());
        a.record(&[0.1], 0.0, 0b1);
        a.record(&[0.9], 0.8, 0b10);
        let c = a.cell(0).unwrap();
        assert_eq!(c.samples, 2);
        assert!((c.mean_risk() - 0.4).abs() < 1e-12);
        assert!((c.risk_max - 0.8).abs() < 1e-12);
        assert_eq!(c.channels, 0b11);
    }

    #[test]
    fn a_cell_needs_samples_before_it_can_be_trusted() {
        let p = Policy::default();
        let mut c = a_cell(vec![[0.0, 1.0]]);
        c.risk_max = 0.0;
        c.risk_sum = 0.0;
        c.samples = 1;
        assert_eq!(
            p.classify(&c),
            Label::Unknown,
            "one sample is not a verdict"
        );
        c.samples = 4;
        c.measured = 0b00010;
        assert_eq!(
            p.classify(&c),
            Label::Trusted,
            "a sensor looked, and found nothing: that is what TRUSTED means"
        );
        // The *evidence* mask earns nothing, and must not: a clean cell has no evidence by
        // definition, so judging trust on which channels spoke would leave every clean region of
        // every model UNKNOWN for the rest of the run.
        c.measured = 0;
        c.channels = 0b11111;
        assert_eq!(
            p.classify(&c),
            Label::Unknown,
            "a cell no sensor was applied to is not a trusted one, whatever it heard"
        );
    }

    #[test]
    fn a_peak_failure_makes_a_cell_suspicious_even_when_averaged_away() {
        let p = Policy::default();
        let mut c = a_cell(vec![[0.0, 1.0]]);
        c.samples = 100;
        c.risk_sum = 0.1;
        c.risk_max = 0.9;
        // The peak has to belong to an evaluation, and a single loud channel is only a lead (see
        // [`Policy::suspicious_channels`]), so this cell's narrow failure is one two channels saw at
        // the same point: what the aggregate `risk_max` is standing in for.
        c.points = vec![Point {
            observation: 7,
            x: vec![0.42],
            risk: 0.9,
            channels: 0b10010,
            measured: 0b10010,
            fact: false,
        }];
        assert!(c.mean_risk() < p.suspicious_mean, "the mean looks calm");
        assert_eq!(p.classify(&c), Label::Suspicious);
    }

    #[test]
    fn splitting_halves_the_longest_normalised_axis() {
        let mut a = Atlas::new(&model1d(0.0, 1.0), Policy::default());
        a.record(&[0.5], 0.9, 0b1);
        let children = a.split(0).expect("splits");
        assert_eq!(children.len(), 2);
        assert_eq!(a.cell(children[0]).unwrap().bounds[0], [0.0, 0.5]);
        assert_eq!(a.cell(children[1]).unwrap().bounds[0], [0.5, 1.0]);
        assert_eq!(a.cell(children[0]).unwrap().depth, 1);
        // An earlier contract ("a child starts with no evidence") is what broke the atlas: refining
        // then wiped every label, so a campaign could make itself entirely UNKNOWN by splitting.
        // The contract now is that measurements move to the child that contains them, which is why
        // the total across the two children has to equal what the parent saw.
        assert_eq!(a.cell(children[0]).unwrap().samples, 1);
        assert_eq!(
            children
                .iter()
                .map(|id| a.cell(*id).unwrap().samples)
                .sum::<u32>(),
            1,
            "a point must land in exactly one child"
        );
    }

    #[test]
    fn an_unmeasurable_reading_is_never_reported_as_trusted() {
        // The trap: every comparison against NaN is false, so a cell whose `risk_sum` was poisoned by
        // one non-finite score failed the `flagged` test, passed the sample and channel tests, and fell
        // out of the bottom of `classify` as `Trusted` — a clean bill of health issued for want of
        // information. `fuse` keeps scores finite, so this is the guard for a caller that did not.
        let p = Policy::default();
        let mut c = a_cell(vec![[0.0, 1.0]]);
        c.samples = 8;
        c.measured = 0b11111;
        c.channels = 0b11111;
        c.risk_sum = 8.0 * 0.01;
        c.risk_max = 0.01;
        assert_eq!(p.classify(&c), Label::Trusted, "the clean case first");
        c.risk_sum = f64::NAN;
        assert_eq!(
            p.classify(&c),
            Label::Unknown,
            "a cell that cannot state its mean was called clean"
        );
        // The same through the atlas itself, because the label a reader sees is assigned by `relabel`
        // walking real recorded points.
        let mut a = Atlas::new(&model1d(0.0, 1.0), Policy::default());
        for i in 0..8 {
            a.record(&[i as f64 / 7.0], f64::NAN, 0b11111);
        }
        a.relabel();
        assert_eq!(
            a.cell(0).map(|c| c.label),
            Some(Label::Unknown),
            "the atlas trusted a domain it could not score"
        );
        // An infinite risk is a different thing: an extreme value, read as flagged.
        let mut c = a_cell(vec![[0.0, 1.0]]);
        c.samples = 8;
        c.measured = 0b11111;
        c.channels = 0b11111;
        c.risk_sum = f64::INFINITY;
        c.risk_max = f64::INFINITY;
        c.points = vec![Point {
            observation: 0,
            x: vec![0.5],
            risk: f64::INFINITY,
            channels: 0b11111,
            measured: 0b11111,
            fact: true,
        }];
        assert_eq!(
            p.classify(&c),
            Label::Suspicious,
            "an infinite risk is a loud one, not a missing one"
        );
    }

    #[test]
    fn refinement_grows_the_atlas_only_around_interest() {
        let mut a = Atlas::new(
            &model1d(0.0, 1.0),
            Policy {
                min_samples: 1,
                ..Policy::default()
            },
        );
        for i in 0..8 {
            a.record(&[i as f64 / 7.0], if i > 5 { 0.9 } else { 0.0 }, 0b1);
        }
        a.relabel();
        let grown = a.refine();
        assert!(grown > 0, "the suspicious half must be split");
        assert!(a.leaves.len() > 1);
    }

    #[test]
    fn one_channels_opinion_holds_a_cell_at_unknown_and_a_fact_does_not() {
        // The remaining false positives after every measurement fix were single-channel sensitivity
        // readings of regions that really are ill-conditioned and contain no defect. A loud
        // measurement is a lead; SUSPICIOUS is something else.
        let p = Policy::default();
        let mut c = a_cell(vec![[0.0, 1.0]]);
        c.samples = 6;
        c.risk_sum = 6.0 * 0.95;
        c.risk_max = 0.95;
        c.points = vec![Point {
            observation: 1,
            x: vec![0.5],
            risk: 0.95,
            channels: 0b10000,
            measured: 0b10000,
            fact: false,
        }];
        assert_eq!(
            p.classify(&c),
            Label::Unknown,
            "one channel at full strength is not a finding"
        );
        // Two channels agreeing about the same evaluation is a finding.
        c.points[0].channels = 0b10010;
        assert_eq!(p.classify(&c), Label::Suspicious);
        // And a fact clears the bar alone: a rule that fired is not an opinion.
        c.points[0].channels = 0b00010;
        c.points[0].fact = true;
        assert_eq!(
            p.classify(&c),
            Label::Suspicious,
            "a fired rule needs no second voice"
        );
        // Two channels that spoke about *different* evaluations do not corroborate each other.
        c.points[0].fact = false;
        c.points[0].channels = 0b00010;
        c.points.push(Point {
            observation: 2,
            x: vec![0.7],
            risk: 0.95,
            channels: 0b10000,
            measured: 0b10000,
            fact: false,
        });
        c.samples = 12;
        c.risk_sum = 12.0 * 0.95;
        assert_eq!(
            p.classify(&c),
            Label::Unknown,
            "the cell heard two channels, but never about the same evaluation"
        );
    }

    #[test]
    fn a_single_point_does_not_drill_the_atlas_to_the_depth_limit() {
        // Measured, before the sample requirement was applied to every branch of `refine`: one
        // suspicious leaf was split anyway, so a single NaN drilled to `max_depth`, the atlas of a
        // one-dimensional model grew to 881 leaves holding one evaluation each at a 640 budget, no
        // leaf ever reached the four samples a TRUSTED needs, nothing was trusted, no two labelled
        // cells disagreed, and a model with one known boundary reported no band at all.
        let mut a = Atlas::new(&model1d(0.0, 1.0), Policy::default());
        // A NaN: an absolute fact, which is what makes it suspicious on one channel's word.
        a.record_point(Point {
            observation: 0,
            x: vec![0.9],
            risk: 1.0,
            channels: 0b00010,
            measured: 0b00010,
            fact: true,
        });
        a.relabel();
        let root = a.leaves[0];
        assert_eq!(
            a.cell(root).unwrap().label,
            Label::Suspicious,
            "one fact is still a fact: a label does not need four samples"
        );
        assert_eq!(
            a.refine(),
            0,
            "a cell measured once cannot be resolved by cutting it in half"
        );
        for i in 0..3 {
            a.record(&[0.85 + i as f64 * 0.01], 1.0, 0b1);
        }
        a.relabel();
        assert!(
            a.refine() > 0,
            "the same suspicious cell with enough samples is worth splitting"
        );
    }

    #[test]
    fn refinement_stops_at_the_depth_limit() {
        let mut a = Atlas::new(
            &model1d(0.0, 1.0),
            Policy {
                max_depth: 1,
                min_samples: 1,
                ..Policy::default()
            },
        );
        for _ in 0..4 {
            a.record(&[0.5], 0.9, 0b1);
        }
        a.relabel();
        let first = a.refine();
        assert!(first > 0);
        let second = a.refine();
        assert_eq!(second, 0, "a leaf at max_depth must not keep splitting");
    }

    #[test]
    fn a_boundary_between_two_labels_becomes_a_band() {
        let mut a = Atlas::new(
            &model1d(0.0, 1.0),
            Policy {
                min_samples: 1,
                // This test is about which pairs of cells make a band, not about how many channels a
                // label needs; the corroboration rule is exercised in
                // `one_channels_opinion_holds_a_cell_at_unknown_and_a_fact_does_not`.
                suspicious_channels: 1,
                ..Policy::default()
            },
        );
        for i in 0..4 {
            a.record(&[i as f64 / 3.0], 0.0, 0b1);
            a.record(&[0.6 + i as f64 / 10.0], 0.9, 0b1);
        }
        a.relabel();
        a.refine();
        // A split child begins with no evidence, so it has to be sampled again before it can
        // disagree with its neighbour. Refining without re-sampling gives empty cells, not a
        // boundary — which is the correct behaviour and easy to get wrong in a test.
        a.record(&[0.25], 0.0, 0b1);
        a.record(&[0.75], 0.9, 0b1);
        a.relabel();
        let bands = a.bands();
        assert!(
            !bands.is_empty(),
            "no band found, coverage {:?}",
            a.coverage()
        );
        assert_eq!(bands[0].axis, 0);
        assert!(bands[0].hi >= bands[0].lo);
        assert!(bands[0].contains(0.5), "band {:?}", bands[0]);
    }

    #[test]
    fn a_band_shrinks_as_the_atlas_refines() {
        let truth = 0.55;
        let mut a = Atlas::new(
            &model1d(0.0, 1.0),
            Policy {
                min_samples: 1,
                // About partition resolution, not about how many channels a label needs.
                suspicious_channels: 1,
                ..Policy::default()
            },
        );
        let feed = |a: &mut Atlas, n: usize| {
            for i in 0..n {
                let x = i as f64 / (n - 1) as f64;
                a.record(&[x], if x >= truth { 0.9 } else { 0.0 }, 0b1);
            }
        };
        feed(&mut a, 8);
        a.relabel();
        a.refine();
        feed(&mut a, 8);
        a.relabel();
        let coarse = a
            .bands()
            .into_iter()
            .find(|b| b.axis == 0)
            .expect("one band");
        assert!(coarse.contains(truth), "the band lost the real transition");
        let mut width = coarse.width();
        for _ in 0..5 {
            a.refine();
            feed(&mut a, 16);
            a.relabel();
            let band = a
                .bands()
                .into_iter()
                .find(|b| b.axis == 0)
                .expect("still one band");
            assert!(band.contains(truth), "refinement lost the transition");
            width = band.width();
        }
        assert!(
            width <= coarse.width(),
            "refinement widened the band: {} -> {}",
            coarse.width(),
            width
        );
        // The facing edges are the tight claim, and they should close in on the truth.
        let band = a.bands().remove(0);
        assert!(
            band.facing[0] <= truth + 1e-9 && band.facing[1] >= truth - 1e-9,
            "facing edges {:?} do not bracket {truth}",
            band.facing
        );
    }

    #[test]
    fn an_unexplored_axis_produces_no_boundary() {
        let mut a = Atlas::new(&model1d(0.0, 1.0), Policy::default());
        a.record(&[0.5], 0.0, 0b1);
        a.relabel();
        assert!(a.bands().is_empty());
    }

    #[test]
    fn the_atlas_works_in_twelve_dimensions() {
        let m = modelnd(12);
        let mut a = Atlas::new(
            &m,
            Policy {
                min_samples: 1,
                ..Policy::default()
            },
        );
        let inside: Vec<f64> = (0..12).map(|i| (i as f64 + 1.0) * 0.1).collect();
        a.record(&inside, 0.8, 0b1);
        a.relabel();
        a.refine();
        assert!(a.leaves.len() > 1, "a 12-dimensional cell must still split");
        let csv = a.to_csv();
        assert!(csv.contains("lo11,hi11"), "{}", &csv[..csv.len().min(200)]);
    }

    #[test]
    fn relative_size_partitions_the_space() {
        let mut a = Atlas::new(&model1d(0.0, 4.0), Policy::default());
        a.record(&[1.0], 0.0, 0b1);
        let kids = a.split(0).unwrap();
        let sum: f64 = kids
            .iter()
            .map(|id| a.cell(*id).unwrap().relative_size(a.root()))
            .sum();
        assert!((sum - 1.0).abs() < 1e-12, "{sum}");
    }

    #[test]
    fn a_measurement_outside_every_leaf_is_counted_rather_than_absorbed() {
        // The failure this guards against was silent: before the count existed, a point that no leaf
        // contained was filed under cell 0, which after the first split is not a leaf at all, so the
        // measurement disappeared from `coverage().samples`, from the labels and from the findings while
        // the run went on reporting an atlas as if it had used every evaluation it had paid for.
        let mut a = Atlas::new(
            &model1d(0.0, 1.0),
            Policy {
                min_samples: 1,
                ..Policy::default()
            },
        );
        for x in [0.1, 0.3, 0.7, 0.9] {
            a.record(&[x], 0.9, 0b1);
        }
        a.relabel();
        a.refine();
        a.relabel();
        assert!(
            a.leaves.len() > 1,
            "this test needs a partition with more than one leaf"
        );
        assert_eq!(a.coverage().unplaced, 0, "every point above is inside");

        // A point outside the declared domain, and a coordinate that is not a number at all: both fall
        // in no leaf, and `contains` is false for NaN by comparison rather than by special case.
        a.record(&[1.5], 0.9, 0b1);
        a.record(&[f64::NAN], 0.9, 0b1);
        assert_eq!(a.coverage().unplaced, 2);
        // Counted, and still not part of any leaf's evidence: that is the whole point of the number.
        let placed: u32 = a
            .leaf_ids()
            .iter()
            .map(|id| a.cell(*id).map_or(0, |c| c.samples))
            .sum();
        assert_eq!(a.coverage().samples, placed);
        assert_eq!(
            a.coverage().samples,
            4,
            "the two stray points are not in it"
        );
    }

    #[test]
    fn a_discrete_axis_is_bounded_in_the_values_its_points_carry() {
        // This asserted the opposite until the two spaces were found to disagree: the sampler hands
        // back an option value (`to_parameters` returns 8.0 for this domain), so a root bounded in
        // indices claimed `[0, 3]` while every point carried 1, 2, 4 or 8 -- and the points above 3
        // belonged to no leaf at all. Measured on a model with a four-way choice axis, 38 of 120
        // recorded evaluations were sitting in the root cell that had stopped being a leaf, so they
        // appeared in neither the labels nor the coverage nor a finding.
        let mut m = Model::new("choice");
        m.params.push(Param {
            name: "method".into(),
            ty: Ty {
                num: NumType::I64,
                dim: Dimension::dimensionless(),
            },
            domain: Domain::Choices(vec![1.0, 2.0, 4.0, 8.0]),
            to_si: 1.0,
            doc: String::new(),
        });
        let a = Atlas::new(&m, Policy::default());
        assert_eq!(
            a.root().bounds[0],
            [1.0, 8.0],
            "the axis is bounded where its points are"
        );
        // Every option is inside the partition, including the extremes, and a value between options --
        // what an axis probe produces -- is inside too rather than orphaned.
        for value in [1.0, 2.0, 4.0, 8.0, 3.5, 7.999_7] {
            assert!(
                a.root().contains(&[value]),
                "{value} falls outside the root cell"
            );
        }
        // And bisecting still terminates, which was the reason index space was chosen in the first
        // place. Value space keeps it: a numeric interval over discrete samples splits like any other.
        let mut a = Atlas::new(
            &m,
            Policy {
                min_samples: 1,
                ..Policy::default()
            },
        );
        for value in [1.0, 2.0, 4.0, 8.0] {
            a.record(&[value], if value > 3.0 { 0.9 } else { 0.05 }, 0b1);
        }
        a.relabel();
        a.refine();
        a.relabel();
        assert!(
            a.leaves.len() > 1,
            "refinement could not split a discrete axis"
        );
        let placed = a
            .leaf_ids()
            .iter()
            .filter(|id| a.cell(**id).is_some_and(|c| c.samples > 0))
            .count();
        assert!(placed > 0, "no leaf holds a measurement");
        assert_eq!(
            a.coverage().samples,
            4,
            "a recorded point fell out of the partition"
        );
    }

    #[test]
    fn csv_has_one_row_per_leaf_and_names_the_bounds() {
        let mut a = Atlas::new(
            &model1d(0.0, 1.0),
            Policy {
                min_samples: 1,
                ..Policy::default()
            },
        );
        a.record(&[0.2], 0.9, 0b1);
        a.relabel();
        a.refine();
        let csv = a.to_csv();
        let rows: Vec<&str> = csv.lines().collect();
        assert!(rows[0].starts_with("cell,depth,label"), "{}", rows[0]);
        assert_eq!(rows.len() - 1, a.leaves.len());
    }

    #[test]
    fn the_permitted_partition_is_the_tree_the_split_rule_builds() {
        // The claim a load-time reachability check rests on: this is the partition the atlas is
        // permitted to build, so a region none of these boxes can enclose is a region the atlas cannot
        // localise whatever the evidence says.
        let model = model1d(0.0, 1.0);
        let policy = Policy {
            max_depth: 3,
            ..Policy::default()
        };
        let cells = Atlas::permitted_partition(&model, policy).expect("depth 3 is inside the cap");
        // A full binary tree to depth d holds 2^(d+1) - 1 cells.
        assert_eq!(
            cells.len(),
            (1 << 4) - 1,
            "fifteen boxes, three levels of bisection"
        );
        assert!(
            cells.iter().all(|c| usize::from(c.depth) <= 3),
            "a permitted cell sits past the depth cap"
        );
        let root = &cells[0];
        assert_eq!(root.depth, 0);
        assert_eq!(root.bounds, vec![[0.0, 1.0]]);
        let deepest: Vec<&Cell> = cells.iter().filter(|c| c.depth == 3).collect();
        assert_eq!(deepest.len(), 8, "eight finest intervals");
        for cell in &deepest {
            assert!((cell.width(0) - 0.125).abs() < 1e-12, "{}", cell.width(0));
        }
        // And the finest level tiles the domain: no gap and no overlap, which is what "partition" means
        // and what a coverage fraction added over leaves relies on.
        let sum: f64 = deepest.iter().map(|c| c.relative_size(root)).sum::<f64>();
        assert!(
            (sum - 1.0).abs() < 1e-12,
            "the deepest cells cover {sum} of the domain"
        );
    }

    #[test]
    fn the_permitted_partition_halves_the_longest_normalised_axis_and_pin_the_tie() {
        // Normalised, not raw: a model whose second axis is declared ten times wider than its first is
        // still bisected on the *first*, because both axes start at full extent and the tie goes to the
        // last maximum of `max_by`. That tie-break is not decoration — the reachability class of a
        // region depends on which axis is cut first, so a reader who wants to recompute a class needs
        // this exact behaviour, and this test is where it is written down.
        let mut model = Model::new("wide");
        for (name, lo, hi) in [("a", 0.0f64, 1.0f64), ("b", 0.0f64, 1000.0f64)] {
            model.params.push(Param {
                name: name.into(),
                ty: Ty {
                    num: NumType::F64,
                    dim: Dimension::dimensionless(),
                },
                domain: Domain::Interval { lo, hi },
                to_si: 1.0,
                doc: String::new(),
            });
        }
        let policy = Policy {
            max_depth: 2,
            ..Policy::default()
        };
        let cells = Atlas::permitted_partition(&model, policy).expect("depth 2 is small");
        let level1: Vec<&Cell> = cells.iter().filter(|c| c.depth == 1).collect();
        assert_eq!(level1.len(), 2);
        // The root's tie resolved to the second axis, so the first bisection halves `b` and leaves `a`.
        for cell in &level1 {
            assert_eq!(
                cell.bounds[0],
                [0.0, 1.0],
                "the tie did not fall on the second axis"
            );
            assert_eq!(
                cell.width(1),
                500.0,
                "the first split did not halve the second axis"
            );
        }
        // Its children, whose second axis is now at half extent and first axis still full, must be cut
        // on the *first* axis even though it is the narrower one in raw units: the rule is about how
        // much of each declared domain a cell still spans, not about how large the numbers are.
        let level2: Vec<&Cell> = cells.iter().filter(|c| c.depth == 2).collect();
        assert_eq!(level2.len(), 4);
        for cell in &level2 {
            assert_eq!(
                cell.width(0),
                0.5,
                "the second split did not halve the first axis"
            );
            assert_eq!(
                cell.width(1),
                500.0,
                "the second split moved the wrong axis"
            );
        }
        // Deterministic: the same model and policy enumerate the same boxes in the same order, twice.
        let again = Atlas::permitted_partition(&model, policy).expect("and again");
        assert_eq!(again.len(), cells.len());
        for (a, b) in again.iter().zip(&cells) {
            assert_eq!(a.bounds, b.bounds, "the enumeration is not deterministic");
            assert_eq!(a.depth, b.depth);
        }
    }

    #[test]
    fn a_cell_that_cannot_be_bisected_ends_the_branch() {
        // A degenerate axis has no strictly interior midpoint, so `split` declines, and the permitted
        // partition is the root alone: exactly the atlas a run would be stuck with. A reachability
        // question about such a domain has to be answered from this, not from a lattice someone drew
        // somewhere else.
        let model = model1d(0.0, 0.0);
        let cells =
            Atlas::permitted_partition(&model, Policy::default()).expect("one cell is finite");
        assert_eq!(cells.len(), 1, "a point-sized domain cannot be subdivided");
        assert_eq!(cells[0].bounds, vec![[0.0, 0.0]]);
        // A choice axis holding one value is the same case in the other vocabulary.
        let mut discrete = Model::new("one-choice");
        discrete.params.push(Param {
            name: "method".into(),
            ty: Ty {
                num: NumType::I64,
                dim: Dimension::dimensionless(),
            },
            domain: Domain::Choices(vec![3.0, 3.0]),
            to_si: 1.0,
            doc: String::new(),
        });
        let cells =
            Atlas::permitted_partition(&discrete, Policy::default()).expect("one cell is finite");
        assert_eq!(
            cells.len(),
            1,
            "a choice set spanning nothing cannot be subdivided"
        );
    }

    #[test]
    fn a_partition_too_deep_to_enumerate_is_refused_rather_than_run_out() {
        // The cap is the difference between a check that answers and a check that hangs. Depth 20 in one
        // dimension is 2,097,151 boxes, past the admitted ceiling, so the caller gets a refusal and a
        // reachability question it cannot answer — which the harness reports as a named failure instead
        // of as a verdict about the region.
        let model = model1d(0.0, 1.0);
        let deep = Policy {
            max_depth: 20,
            ..Policy::default()
        };
        assert_eq!(
            Atlas::permitted_partition(&model, deep),
            None,
            "the enumerator walked a tree it was never going to finish"
        );
        // And the frozen depth is four orders inside the cap, so this refusal is a guard and not a limit
        // anyone working here will meet.
        let frozen = Policy::default();
        let cells =
            Atlas::permitted_partition(&model, frozen).expect("the frozen depth enumerates");
        assert_eq!(cells.len(), (1 << 13) - 1, "8,191 boxes at depth 12");
        assert!(cells.len() < Atlas::PARTITION_CELL_CAP);
    }

    fn a_cell(bounds: Vec<[f64; 2]>) -> Cell {
        Cell {
            bounds,
            id: 0,
            depth: 0,
            samples: 0,
            risk_sum: 0.0,
            risk_max: 0.0,
            label: Label::Unknown,
            channels: 0,
            facts: 0,
            measured: 0,
            points: Vec::new(),
        }
    }

    /// Cells whose label every variant has to be able to state: the clean case, the under-sampled
    /// case, the one-channel-silence case, the corroboration case, and the two ways a reading can
    /// leave the real numbers without tripping the flag bar.
    fn a_table() -> Vec<Cell> {
        let quiet = || {
            let mut c = a_cell(vec![[0.0, 1.0]]);
            c.samples = 8;
            c.measured = 0b11111;
            c.channels = 0;
            c.risk_sum = 8.0 * 0.01;
            c.risk_max = 0.01;
            c.points = (0..8)
                .map(|i| Point {
                    observation: i,
                    x: vec![0.1 * (f64::from(i as u32) + 1.0)],
                    risk: 0.01,
                    channels: 0b10,
                    measured: 0b11111,
                    fact: false,
                })
                .collect();
            c.recompute();
            c.label = Label::Unknown;
            c
        };
        let mut one_channel = quiet();
        for p in &mut one_channel.points {
            p.measured = 0b00010;
        }
        one_channel.recompute();
        let mut under_sampled = quiet();
        under_sampled.points.truncate(3);
        under_sampled.recompute();
        let mut negative_infinity = quiet();
        negative_infinity.points.push(Point {
            observation: 99,
            x: vec![0.5],
            risk: f64::NEG_INFINITY,
            channels: 0b10,
            measured: 0b11111,
            fact: false,
        });
        negative_infinity.recompute();
        let mut poisoned = quiet();
        poisoned.points.push(Point {
            observation: 98,
            x: vec![0.6],
            risk: f64::NAN,
            channels: 0b10,
            measured: 0b11111,
            fact: false,
        });
        poisoned.recompute();
        let mut loud = quiet();
        loud.points.push(Point {
            observation: 97,
            x: vec![0.7],
            risk: 0.95,
            channels: 0b10010,
            measured: 0b11111,
            fact: false,
        });
        loud.recompute();
        vec![
            quiet(),
            one_channel,
            under_sampled,
            negative_infinity,
            poisoned,
            loud,
        ]
    }

    #[test]
    fn the_current_variant_is_the_committed_rule_and_not_a_restatement_of_it() {
        // The requirement E1.3b puts on this enum: naming today's rule must reproduce today's
        // numbers. Every cell in the table is classified twice — once by the plain default policy, as
        // it has been since before the variant existed, and once by `Trust::Current` through the new
        // path — and a difference is a changed measurement wearing an old name.
        let before = Policy {
            trust: Trust::Current,
            ..Policy::default()
        };
        let committed = Policy::default();
        assert_eq!(before, committed, "a default policy is the current variant");
        for cell in a_table() {
            assert_eq!(
                committed.classify(&cell),
                before.classify(&cell),
                "the variant path moved a label under the default variant"
            );
        }
        // The effective bar is the stricter of the two numbers, and under `Current` the variant adds
        // nothing to it: `min_channels` still means exactly what it meant.
        assert_eq!(committed.min_channels_required(), committed.min_channels);
        assert!(!committed.blocks_non_finite());
    }

    #[test]
    fn a_variant_names_the_two_rules_that_can_stop_a_trusted_and_nothing_else() {
        let table = a_table();
        let clean = &table[0];
        assert_eq!(Policy::default().classify(clean), Label::Trusted);
        for variant in [
            Trust::MinChannelsTwo,
            Trust::NonFiniteBlocksTrust,
            Trust::BothRules,
        ] {
            let policy = variant.policy();
            // A leaf measured by five channels is unaffected by the channel floor, and a leaf whose
            // readings are all real is unaffected by the non-finite rule. Nothing here touches a
            // suspicious cell: the variants refuse trust, they do not manufacture evidence.
            assert_eq!(policy.classify(clean), Label::Trusted, "{variant:?}");
            assert_eq!(policy.classify(&table[5]), Label::Suspicious);
        }
        // The channel rule: one sensor's silence is enough for `Current` and not for the two variants
        // that ask for two.
        let one = &table[1];
        assert_eq!(one.measured.count_ones(), 1);
        assert_eq!(Policy::default().classify(one), Label::Trusted);
        for variant in [Trust::MinChannelsTwo, Trust::BothRules] {
            assert_eq!(
                variant.policy().classify(one),
                Label::Unknown,
                "{variant:?} should not trust a leaf one channel was applied to"
            );
        }
        for variant in [Trust::Current, Trust::NonFiniteBlocksTrust] {
            assert_eq!(variant.policy().classify(one), Label::Trusted);
        }
        // The non-finite rule, on the cell that is reachable rather than theoretical: a `-inf`
        // reading leaves `risk_max` at 0.01 because the fold starts there, keeps the mean below the
        // bar, and so was trusted — which is the mechanism 0029 named and 0038 found in the trace.
        let negative = &table[3];
        assert!(negative.points.iter().any(|p| !p.risk.is_finite()));
        assert!(
            negative.risk_max.is_finite(),
            "this test is about a leaf whose aggregates hide the reading"
        );
        assert_eq!(Policy::default().classify(negative), Label::Trusted);
        for variant in [Trust::NonFiniteBlocksTrust, Trust::BothRules] {
            assert_eq!(
                variant.policy().classify(negative),
                Label::Unknown,
                "{variant:?} should refuse to trust a leaf holding a non-real reading"
            );
        }
        // A NaN is already refused by the aggregate guard, so this variant adds no second route to the
        // same answer — stated because a reader would otherwise expect it to.
        assert_eq!(Policy::default().classify(&table[4]), Label::Unknown);
        assert_eq!(
            Trust::BothRules.policy().classify(&table[4]),
            Label::Unknown
        );
    }

    #[test]
    fn a_non_finite_reading_is_counted_in_the_one_leaf_that_holds_it() {
        // E1.3b's third secondary metric is the count of non-real readings per row, because a variant
        // that refused trust in a leaf holding nothing non-finite refused it for some other reason.
        // The number has to come from the same partition the labels come from, so this builds one
        // rather than handing `classify` a cell it never saw.
        let mut a = Atlas::new(&model1d(0.0, 1.0), Policy::default());
        for i in 1..=2 {
            a.record(&[0.1 * f64::from(i)], 0.01, 0b11111);
        }
        for i in 6..=9 {
            a.record(&[0.1 * f64::from(i)], 0.01, 0b11111);
        }
        assert_eq!(
            a.non_finite_readings(),
            0,
            "a quiet partition has nothing to block"
        );
        let children = a.split(0).expect("the root splits in two");
        // `refine` does this bookkeeping; `split` on its own leaves the parent in the leaf list, and a
        // test that forgot it would be measuring a partition no run ever has.
        a.leaves = children.clone();
        assert_eq!(
            a.non_finite_readings(),
            0,
            "splitting moves readings between leaves, it does not create them"
        );
        let at = children[1] as usize;
        a.cells[at].points[0].risk = f64::NEG_INFINITY;
        a.cells[at].recompute();
        assert_eq!(
            a.non_finite_readings(),
            1,
            "one reading left the real numbers"
        );
        // The leaf the count names is the leaf the variant acts on, and the rule in force since 0036
        // does not act on it: the fold starts at zero and the mean is below the bar, so the aggregates
        // hide the reading and the cell was trusted.
        assert_eq!(Policy::default().classify(&a.cells[at]), Label::Trusted);
        assert_eq!(
            Trust::NonFiniteBlocksTrust.policy().classify(&a.cells[at]),
            Label::Unknown
        );
        // Refining carries the reading into the child that contains it and leaves the parent's copy
        // behind, so walking the leaves counts the evidence once where walking the cells would count it
        // once per generation that held it.
        let grandchildren = a.split(children[1]).expect("the poisoned leaf splits");
        a.leaves = vec![children[0], grandchildren[0], grandchildren[1]];
        assert_eq!(a.non_finite_readings(), 1, "one reading, in one leaf");
    }

    #[test]
    fn a_trust_variant_has_one_name_and_refuses_a_fifth() {
        for variant in Trust::ALL {
            assert_eq!(Trust::parse(variant.name()), Some(variant));
        }
        // `names()` is derived from `name()` in the enum's own order rather than written out again
        // beside it, so a refusal can never list a vocabulary the parser has stopped accepting.
        assert_eq!(
            Trust::names().to_vec(),
            Trust::ALL.iter().map(|v| v.name()).collect::<Vec<_>>()
        );
        // One name per variant: two variants under one word would make an arm's identity a guess.
        let mut unique: Vec<&str> = Trust::names().to_vec();
        unique.sort_unstable();
        let count = unique.len();
        unique.dedup();
        assert_eq!(count, unique.len(), "two trust variants share a name");
        // The names a frozen protocol uses, spelled here once so a typo in a protocol file is a
        // refusal rather than a silently different experiment.
        assert_eq!(Trust::parse("current"), Some(Trust::Current));
        assert_eq!(Trust::parse("min-channels-2"), Some(Trust::MinChannelsTwo));
        assert_eq!(
            Trust::parse("non-finite-blocks-trust"),
            Some(Trust::NonFiniteBlocksTrust)
        );
        assert_eq!(Trust::parse("both-trust-rules"), Some(Trust::BothRules));
        for wrong in [
            "",
            "Current",
            "min_channels_2",
            "min-channels-3",
            "nonfinite-blocks-trust",
            "both",
            "trust-default",
        ] {
            assert_eq!(
                Trust::parse(wrong),
                None,
                "{wrong:?} is not in the vocabulary"
            );
        }
        // Four variants, and each is a different rule except where the conjunction is defined to
        // repeat one: two rules that each halve an over-claim are not one rule that halves it twice,
        // so `Both` is not derivable from the other two and stays its own name.
        assert_eq!(Trust::ALL.len(), 4);
        assert_eq!(
            Trust::BothRules.min_channels_floor(),
            Trust::MinChannelsTwo.min_channels_floor()
        );
        assert!(Trust::BothRules.blocks_non_finite());
    }

    #[test]
    fn a_variant_never_reads_a_region_into_the_label() {
        // The prohibition E1.3b states as an instrument requirement: `classify` decides from cell
        // statistics. This asserts the shape of the thing rather than its behaviour, because a
        // `Cell` that gained a truth reference would pass every other test in this file and make the
        // experiment measure the answer key.
        let p = Policy::default();
        let c = a_cell(vec![[0.0, 1.0]]);
        let label: Label = p.classify(&c);
        let _ = label;
        // Every field a variant can consult is one the atlas maintains for its own reasons.
        let mut cell = c.clone();
        cell.samples = 4;
        cell.measured = 0b1;
        cell.risk_sum = 0.0;
        cell.risk_max = 0.0;
        assert_eq!(
            p.classify(&cell),
            Label::Trusted,
            "4 samples, 1 channel, quiet"
        );
        cell.points = vec![Point {
            observation: 0,
            x: vec![0.5],
            risk: 0.0,
            channels: 0,
            measured: 0b1,
            fact: false,
        }];
        assert_eq!(
            p.classify(&cell),
            Label::Trusted,
            "a point with no evidence in it does not change the answer"
        );
    }

    // ---- the resolution axis: what a leaf's *scale* has to do with before it is certified ----

    /// Quiet evaluations spread over a model's space, so a partition can be built without any evidence
    /// of a fault in it. `hot` indexes carry a risk high enough to make their cell suspicious, which is
    /// how a test gets a partition that has actually split without resorting to the demand rule.
    fn spread(model: &Model, count: usize, hot: bool) -> Vec<Point> {
        let spans: Vec<[f64; 2]> = model
            .params
            .iter()
            .map(|p| match &p.domain {
                Domain::Interval { lo, hi } => [*lo, *hi],
                Domain::Choices(v) => {
                    let first = v.first().copied().unwrap_or(0.0);
                    [
                        v.iter().fold(first, |acc, x| acc.min(*x)),
                        v.iter().fold(first, |acc, x| acc.max(*x)),
                    ]
                }
            })
            .collect();
        (0..count)
            .map(|i| {
                // A van der Corput-ish walk: every axis gets a different irrational step, so the points
                // land in different cells rather than all on one line.
                let x: Vec<f64> = spans
                    .iter()
                    .enumerate()
                    .map(|(a, [lo, hi])| {
                        let t = (i as f64 * (0.618_033_988_7 + a as f64 * 0.137)).fract();
                        lo + t * (hi - lo)
                    })
                    .collect();
                let risk = if hot && i % 7 == 0 { 0.9 } else { 0.0 };
                Point {
                    observation: i as u64,
                    x,
                    risk,
                    channels: 0b1,
                    measured: 0b1,
                    fact: false,
                }
            })
            .collect()
    }

    fn filled(model: &Model, policy: Policy, points: &[Point]) -> Atlas {
        let mut atlas = Atlas::new(model, policy);
        for p in points {
            atlas.record(&p.x, p.risk, p.measured);
        }
        atlas.relabel();
        atlas
    }

    /// The settling loop `aporia_search` runs once a campaign is over: label from the finished evidence,
    /// then let the partition settle, then label again.
    fn settle(mut atlas: Atlas, points: &[Point], rounds: usize) -> Atlas {
        atlas.remeasure(points);
        for _ in 0..rounds {
            if atlas.refine() == 0 {
                break;
            }
        }
        atlas.relabel();
        atlas
    }

    #[test]
    fn a_quiet_leaf_too_coarse_to_certify_is_provisional_and_not_unknown() {
        let model = modelnd(2);
        let points = spread(&model, 20, false);

        // The committed semantics: sampled, measured, unflagged — and certified, whatever its size.
        let current = filled(&model, Policy::default(), &points);
        assert_eq!(current.root().label, Label::Trusted);

        // The withholding rule sees the same leaf, with the same statistics, and declines.
        let withheld = filled(
            &model,
            Policy {
                resolution: Resolution::Withhold,
                ..Policy::default()
            },
            &points,
        );
        assert_eq!(withheld.root().label, Label::Provisional);
        let coverage = withheld.coverage();
        assert_eq!(
            coverage.trusted, 0.0,
            "certification was withheld, not lost"
        );
        assert_eq!(coverage.unknown, 0.0, "and it did not become unmeasurable");
        assert_eq!(coverage.provisional, 1.0);
        // Neither resolved nor a gap in the evidence, which is the whole claim of the state.
        assert_eq!(coverage.resolved(), 0.0);
        assert_eq!(
            coverage.trusted + coverage.provisional + coverage.unknown + coverage.suspicious,
            1.0,
            "the four volumes are a partition of the domain"
        );
    }

    #[test]
    fn withholding_certification_moves_no_cell_and_splits_no_leaf() {
        // The freeze's control claim: arms A and C differ only in what the atlas says about the cells it
        // already has. Without this, a fall in certified volume could be a fall in where the search went,
        // and the comparison would be about trajectories rather than about semantics.
        let model = modelnd(2);
        // Evidence that does split the partition, so "the same cells" is a non-trivial claim: some leaves
        // here are suspicious, their neighbours are not, and bands exist between them.
        let points = spread(&model, 64, true);
        let current = settle(Atlas::new(&model, Policy::default()), &points, 6);
        let withheld = settle(
            Atlas::new(
                &model,
                Policy {
                    resolution: Resolution::Withhold,
                    ..Policy::default()
                },
            ),
            &points,
            6,
        );

        assert!(
            current.cells.len() > 2,
            "this fixture was supposed to build a partition, not one leaf"
        );
        assert_eq!(
            current.cells.len(),
            withheld.cells.len(),
            "withholding certification changed the number of cells"
        );
        for (a, b) in current.cells.iter().zip(&withheld.cells) {
            assert_eq!(a.depth, b.depth, "a cell moved in depth: {:?}", a.bounds);
            assert_eq!(a.bounds, b.bounds, "a cell moved in space");
            assert_eq!(a.samples, b.samples);
            if a.label == Label::Trusted {
                assert_eq!(
                    b.label,
                    Label::Provisional,
                    "a trusted cell did not become provisional: {:?}",
                    b.bounds
                );
            } else {
                assert_eq!(
                    a.label, b.label,
                    "only the trusted cells were allowed to change their label"
                );
            }
        }
        // Suspicion and the bands between labelled cells are unchanged, because `Label::adjacency` puts
        // PROVISIONAL in the same class as TRUSTED: `bands` reports the same boundaries for both.
        assert_eq!(current.bands().len(), withheld.bands().len());
    }

    #[test]
    fn demanding_resolution_buys_cells_and_pays_in_sample_starvation_at_low_budget() {
        // What the demand rule actually does to a quiet space, measured rather than argued: a split
        // shares the parent's samples out, so a leaf with four evaluations cuts itself into two leaves
        // that can no longer be trusted. Below roughly 4 x 2^8 evaluations a 2^-8 bar cannot be met at
        // all, and the certified volume becomes UNKNOWN instead of becoming finer.
        let model = modelnd(2);
        let coarse_policy = Policy {
            resolution: Resolution::Demand,
            ..Policy::default()
        };

        let thin = spread(&model, 128, false);
        let current_thin = settle(Atlas::new(&model, Policy::default()), &thin, 8);
        let demand_thin = settle(Atlas::new(&model, coarse_policy), &thin, 8);
        assert_eq!(
            current_thin.leaves.len(),
            1,
            "nothing asked the quiet root to split"
        );
        assert!(
            demand_thin.leaves.len() > 8,
            "the demand did not go looking: {} leaves",
            demand_thin.leaves.len()
        );
        let starved = demand_thin.coverage();
        assert_eq!(
            starved.trusted, 0.0,
            "every leaf it made was too thinly sampled"
        );
        assert_eq!(
            starved.unknown, 1.0,
            "and the whole domain became unmeasurable"
        );

        // The crossing the bar's arithmetic predicts: to certify at 2^-8 with a floor of four samples a
        // leaf, the space needs about 4 x 2^8 evaluations inside it. Same policy, same geometry, more
        // evidence, and the certified volume comes back at the resolution the bar asks for.
        let deep = spread(&model, 1024, false);
        let demand_deep = settle(Atlas::new(&model, coarse_policy), &deep, 8);
        let earned = demand_deep.coverage();
        // Not the whole domain: a leaf whose sample count is not a power of two splits into children
        // that fall under the floor together, so both go UNKNOWN and the recovery is partial. Measured
        // here at 0.6992 of the domain against 0.0 at 128 evaluations, which is the crossing this test
        // exists to pin -- its size is the fixture's arithmetic, not a number a report cites.
        assert!(
            earned.trusted > 0.5,
            "at 1024 evaluations the demand should certify most of the space, and trusted {} of it",
            earned.trusted
        );
        for id in demand_deep.leaf_ids() {
            let cell = demand_deep.cell(*id).expect("a leaf is a cell");
            if cell.label == Label::Trusted {
                assert!(
                    !demand_deep.policy().coarse(cell),
                    "a leaf trusted under the demand rule is still coarser than the bar: depth {}",
                    cell.depth
                );
            }
        }
    }

    #[test]
    fn the_coarse_test_and_the_root_relative_volume_agree_about_which_leaves_are_coarse() {
        // `Policy::coarse` reads a leaf's depth; the report reads its volume against the root. They have
        // to mean the same cell by "too coarse", because one hands out labels and the other prices them.
        // Evidence that does split the partition: a quiet atlas under the committed rule is one leaf,
        // and one leaf tests the agreement between two formulas for nothing.
        let model = modelnd(3);
        let mut atlas = Atlas::new(&model, Policy::default());
        for p in spread(&model, 64, true) {
            atlas.record(&p.x, p.risk, 0b1);
        }
        for _ in 0..6 {
            atlas.refine();
        }
        atlas.relabel();
        let policy = atlas.policy();
        let root = atlas.root().clone();
        assert!(
            atlas.leaves.len() > 8,
            "this fixture was supposed to make a deep partition and made {}",
            atlas.leaves.len()
        );
        for id in atlas.leaf_ids() {
            let cell = atlas.cell(*id).expect("a leaf is a cell");
            let by_depth = 0.5f64.powi(i32::from(cell.depth));
            let by_geometry = cell.relative_size(&root);
            assert!(
                (by_depth - by_geometry).abs() < 1e-12,
                "a leaf at depth {} holds {} of the domain by depth and {} by geometry",
                cell.depth,
                by_depth,
                by_geometry
            );
            assert_eq!(
                policy.coarse(cell),
                by_geometry > policy.certify_share,
                "the two definitions of coarse disagreed at depth {}",
                cell.depth
            );
        }
        // The bar is a power of two, so a leaf can sit on it exactly and the comparison is decidable
        // rather than a matter of how the division rounded.
        let on_the_bar = Policy::default().certify_share == 0.5f64.powi(8);
        assert!(on_the_bar, "the default certification bar is 2^-8 exactly");
    }

    #[test]
    fn the_current_rule_is_inert_whatever_the_bar_says() {
        // A plan that names `Current` and a bar of 2^-4 must reproduce the committed measurements: the
        // bar is read by the two resolution rules and by nothing else, so an inert axis with a moved bar
        // is still the semantics every published row was labelled under.
        let model = modelnd(2);
        let points = spread(&model, 64, true);
        let committed = settle(Atlas::new(&model, Policy::default()), &points, 6);
        let moved_bar = settle(
            Atlas::new(
                &model,
                Policy {
                    certify_share: 0.0625,
                    ..Policy::default()
                },
            ),
            &points,
            6,
        );
        assert_eq!(committed.cells.len(), moved_bar.cells.len());
        for (a, b) in committed.cells.iter().zip(&moved_bar.cells) {
            assert_eq!(
                a.label, b.label,
                "{:?} changed under an inert axis",
                b.bounds
            );
            assert_eq!(a.bounds, b.bounds);
        }
        assert_eq!(moved_bar.coverage().provisional, 0.0);
    }

    /// A leaf of the given depth holding the given number of samples, with no evidence in it.
    fn leaf(depth: u8, samples: u32) -> Cell {
        let mut cell = Cell {
            bounds: vec![[0.0, 1.0], [0.0, 1.0]],
            id: 1,
            depth,
            samples,
            risk_sum: 0.0,
            risk_max: 0.0,
            label: Label::Unknown,
            channels: 0,
            facts: 0,
            measured: 0b11111,
            points: (0..samples)
                .map(|at| Point {
                    observation: u64::from(at),
                    x: vec![0.5, 0.5],
                    risk: 0.0,
                    channels: 0,
                    fact: false,
                    measured: 0b11111,
                })
                .collect(),
        };
        cell.recompute();
        cell.label = Label::Unknown;
        cell
    }

    #[test]
    fn the_size_criterion_is_the_committed_predicate_under_every_depth_and_sample_count() {
        // The whole claim that `Certify::Size` is inert, checked against `coarse` rather than asserted
        // beside it: if these two ever disagree, every published `coarse_certified_volume` and every
        // committed label is describing a rule the instrument no longer runs.
        let policy = Policy {
            resolution: Resolution::Withhold,
            ..Policy::default()
        };
        for depth in 0..=12u8 {
            for samples in [0u32, 1, 4, 16, 64, 1_024] {
                let cell = leaf(depth, samples);
                assert_eq!(
                    policy.unsupported(&cell),
                    policy.coarse(&cell),
                    "size criterion disagreed with the committed predicate at depth {depth} with \
                     {samples} samples"
                );
            }
        }
    }

    #[test]
    fn the_density_criterion_acts_where_evidence_and_scale_disagree() {
        // The default bar is `min_samples / certify_share` = 4 / 2⁻⁸ = 1024 evaluations per domain, so
        // a leaf at the bar holding exactly the floor passes both rules. Away from that corner the two
        // criteria separate, and each separation is the hypothesis one arm exists to test.
        let density = Policy {
            resolution: Resolution::Withhold,
            certify: Certify::Density,
            ..Policy::default()
        };
        assert_eq!(density.certify_density, 1_024.0);
        // Coarse and thinly measured: uncertifiable either way.
        assert!(density.unsupported(&leaf(4, 4)));
        assert!(density.coarse(&leaf(4, 4)));
        // On the bar with the floor of samples: both satisfied, which is the operating point the
        // default derives from, and the reason a difference between the arms is not a difference in
        // strictness.
        assert!(!density.unsupported(&leaf(8, 4)));
        assert!(!density.coarse(&leaf(8, 4)));
        // Coarse but densely measured: the size rule withholds and the density rule does not. This is
        // the space a resolution demand would go and look at for no reason — the leaf already holds as
        // much evidence per unit of domain as the bar asks for.
        assert!(density.coarse(&leaf(4, 64)));
        assert!(!density.unsupported(&leaf(4, 64)));
        // And the arithmetic that follows from stating the bar as `min_samples / certify_share`: the
        // density rule withholds a leaf at depth d only below 2^(10-d) samples, so it never withholds
        // anything the size rule would certify. It is a relaxation of the same claim, not a rival one,
        // and an experiment that compares them has to say which of the two it is asking about.
        assert!(!density.unsupported(&leaf(10, 4)));
        assert!(!density.unsupported(&leaf(12, 4)));
        assert!(!density.coarse(&leaf(12, 4)));
        // Given the sample floor — which `classify` applies before any scale question — the density
        // rule only ever withholds space the size rule withholds too. Four samples at depth d is
        // 4·2^d evaluations per domain, which reaches the 1024 bar at d = 8, the depth the size bar
        // sits at. Below the floor a leaf is UNKNOWN either way, so the sweep is restricted to the
        // sample counts a certification could actually be made on.
        for depth in 0..=12u8 {
            for samples in [4u32, 16, 64, 256, 1_024, 4_096] {
                let cell = leaf(depth, samples);
                if density.unsupported(&cell) {
                    assert!(
                        density.coarse(&cell),
                        "density withheld a leaf at depth {depth} with {samples} samples that the \
                         size rule would have certified, which contradicts the bar's derivation"
                    );
                }
            }
        }
        // A leaf with no samples fails the density rule by arithmetic, and is unreachable as a
        // certification either way because `classify` sends it to UNKNOWN on the sample floor first.
        assert!(density.unsupported(&leaf(10, 0)));
        assert_eq!(
            density.classify(&leaf(10, 0)),
            Label::Unknown,
            "the sample floor decides before any scale question is asked"
        );
    }

    #[test]
    fn the_two_criteria_disagree_in_the_label_and_nowhere_else() {
        // Where the rules part: a coarse leaf that is well enough measured. Under the size rule it is
        // PROVISIONAL, under the density rule it is TRUSTED, and under `Current` both are TRUSTED — so
        // an arm that moved from one criterion to the other changed a claim and nothing else.
        let cell = leaf(4, 256);
        let size = Policy {
            resolution: Resolution::Withhold,
            certify: Certify::Size,
            ..Policy::default()
        };
        let density = Policy {
            resolution: Resolution::Withhold,
            certify: Certify::Density,
            ..Policy::default()
        };
        let inert = Policy::default();
        assert_eq!(size.classify(&cell), Label::Provisional);
        assert_eq!(density.classify(&cell), Label::Trusted);
        assert_eq!(inert.classify(&cell), Label::Trusted);
        // PROVISIONAL shares TRUSTED's adjacency, which is what keeps the partition — and therefore
        // where the sampler went — identical between the two arms.
        assert_eq!(
            Label::Provisional.adjacency(),
            Label::Trusted.adjacency(),
            "a withheld claim must not read as disagreement to the refinement rule"
        );
        // And a leaf the density rule does act on is one the size rule acts on too: the relaxation
        // goes one way, and the labels agree wherever both rules fire.
        let thin = leaf(4, 8);
        assert!(density.unsupported(&thin) && size.unsupported(&thin));
        assert_eq!(size.classify(&thin), density.classify(&thin));
    }

    #[test]
    fn an_inert_resolution_leaves_the_labels_alone_whatever_the_criterion() {
        // Both criteria under `Current` must produce exactly the committed labels: the criterion is
        // read by the withhold and demand rules, and neither runs. A plan that named the density axis
        // and left the rule off would otherwise be a second semantics nobody asked for.
        let model = modelnd(2);
        let points = spread(&model, 64, true);
        let committed = settle(Atlas::new(&model, Policy::default()), &points, 6);
        for certify in Certify::ALL {
            let atlas = settle(
                Atlas::new(
                    &model,
                    Policy {
                        certify,
                        certify_density: 3.0,
                        ..Policy::default()
                    },
                ),
                &points,
                6,
            );
            assert_eq!(atlas.cells.len(), committed.cells.len(), "{certify:?}");
            for (a, b) in committed.cells.iter().zip(&atlas.cells) {
                assert_eq!(a.label, b.label, "{certify:?} moved a label under Current");
                assert_eq!(a.bounds, b.bounds);
            }
            assert_eq!(atlas.coverage().provisional, 0.0, "{certify:?}");
        }
    }

    #[test]
    fn withholding_on_density_relables_without_rearranging_the_partition() {
        // The property E1.4 needed of its size rule — an arm whose only difference is what it claims —
        // has to hold of the density rule too, or the two arms would be measuring different searches
        // and the comparison would be about point placement. Quiet points, so the cells reach a claim
        // rather than sitting in UNKNOWN, and enough of them that the coarse leaf the size rule
        // withholds clears the density bar: one leaf over the whole domain holding 6,000 evaluations is
        // 6,000 per domain against a bar of 1,024.
        let model = modelnd(2);
        let points = spread(&model, 6_000, false);
        let build = |certify| {
            settle(
                Atlas::new(
                    &model,
                    Policy {
                        resolution: Resolution::Withhold,
                        certify,
                        ..Policy::default()
                    },
                ),
                &points,
                6,
            )
        };
        let size = build(Certify::Size);
        let density = build(Certify::Density);
        let by_bounds = |atlas: &Atlas| -> Vec<(Vec<[f64; 2]>, Label)> {
            atlas
                .leaf_ids()
                .iter()
                .map(|id| {
                    let cell = atlas.cell(*id).expect("a leaf is a cell");
                    (cell.bounds.clone(), cell.label)
                })
                .collect::<Vec<_>>()
        };
        let (a, b) = (by_bounds(&size), by_bounds(&density));
        assert_eq!(
            a.iter().map(|(bounds, _)| bounds).collect::<Vec<_>>(),
            b.iter().map(|(bounds, _)| bounds).collect::<Vec<_>>(),
            "two withholding arms that disagree about the criterion must still agree about the cells"
        );
        // The two arms have to disagree about a claim, and the direction is the one the arithmetic
        // predicts: the size rule withholds the coarse leaf and the density rule, satisfied by its
        // evaluations per domain, certifies it.
        assert!(
            a.iter().any(|(_, label)| *label == Label::Provisional),
            "the size arm made no withheld claim, so there is nothing for the density arm to \
             disagree about"
        );
        assert!(
            b.iter().any(|(_, label)| *label == Label::Trusted),
            "the density arm withheld everything the size arm withheld, so the two criteria are not \
             separable on this fixture"
        );
        // And they must actually disagree about something, or the second criterion measures nothing.
        assert_ne!(
            a.iter().map(|(_, label)| *label).collect::<Vec<_>>(),
            b.iter().map(|(_, label)| *label).collect::<Vec<_>>(),
            "no leaf changed its claim between the two criteria, so this fixture proves nothing"
        );
        // The volume the size rule withheld and the density rule did not is exactly the difference in
        // what the two arms claim, and it is not zero.
        assert!(size.coverage().trusted < density.coverage().trusted);
        assert!(size.coverage().provisional > density.coverage().provisional);
    }
}
