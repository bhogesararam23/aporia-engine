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
    /// Evidence points at a problem somewhere in here.
    Suspicious,
}

impl Label {
    #[must_use]
    pub fn code(self) -> char {
        match self {
            Self::Unknown => '?',
            Self::Trusted => 'T',
            Self::Suspicious => 'S',
        }
    }

    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Unknown => "UNKNOWN",
            Self::Trusted => "TRUSTED",
            Self::Suspicious => "SUSPICIOUS",
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
    /// says something is inside it, or it sits next to a cell with a different label.
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
                o.samples > 0 && o.label != Label::Unknown && o.label != me.label && me.touches(o)
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
                if ca.label == cb.label {
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
        let (mut trusted, mut suspicious, mut unknown) = (0.0, 0.0, 0.0);
        let mut samples = 0;
        for id in &self.leaves {
            let c = &self.cells[*id as usize];
            samples += c.samples;
            let size = c.relative_size(&root);
            match c.label {
                Label::Trusted => trusted += size,
                Label::Suspicious => suspicious += size,
                Label::Unknown => unknown += size,
            }
        }
        Coverage {
            trusted,
            suspicious,
            unknown,
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
}
