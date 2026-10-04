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
        }
    }
}

impl Policy {
    /// Classify one cell from its accumulated statistics.
    #[must_use]
    pub fn classify(&self, cell: &Cell) -> Label {
        if cell.risk_max >= self.suspicious_peak || cell.mean_risk() >= self.suspicious_mean {
            return Label::Suspicious;
        }
        if cell.samples < self.min_samples {
            return Label::Unknown;
        }
        if cell.measured.count_ones() < self.min_channels {
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
}

impl Atlas {
    /// Build the root cell from a model's declared domains. A parameter with a discrete choice set
    /// is treated as a unit interval over its indices, so every axis can be bisected.
    #[must_use]
    pub fn new(model: &Model, policy: Policy) -> Self {
        let bounds = model
            .params
            .iter()
            .map(|p| match &p.domain {
                Domain::Interval { lo, hi } => [*lo, *hi],
                Domain::Choices(v) if v.len() > 1 => [0.0, (v.len() - 1) as f64],
                Domain::Choices(_) => [0.0, 1.0],
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
            measured: 0,
            points: Vec::new(),
        };
        Self {
            cells: vec![root],
            leaves: vec![0],
            policy,
            observations: 0,
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
    #[must_use]
    pub fn locate(&self, x: &[f64]) -> u32 {
        self.leaves
            .iter()
            .copied()
            .find(|id| self.cells[*id as usize].contains(x))
            .unwrap_or(0)
    }

    /// Record one evaluation.
    ///
    /// The observation id is the atlas' own counter. It matches the campaign's record ids because a
    /// point is recorded once per evaluation, in evaluation order; the search test that compares
    /// `records` against `atlas` cell points is what keeps that promise honest.
    pub fn record(&mut self, x: &[f64], risk: f64, channels: u8) {
        // For a caller that only knows what spoke: whatever had something to say was, necessarily,
        // applied. The other direction does not hold, which is why a search that wants TRUSTED to be
        // reachable passes its own measured mask to [`Atlas::record_measured`].
        self.record_measured(x, risk, channels, channels);
    }

    /// Record one evaluation, separating the channels that produced evidence (`channels`) from the
    /// channels that were applied to it (`measured`).
    pub fn record_measured(&mut self, x: &[f64], risk: f64, channels: u8, measured: u8) {
        let observation = self.observations;
        self.observations += 1;
        let id = self.locate(x);
        let cell = &mut self.cells[id as usize];
        cell.points.push(Point {
            observation,
            x: x.to_vec(),
            risk,
            channels,
            measured,
        });
        cell.samples += 1;
        cell.risk_sum += risk;
        cell.risk_max = cell.risk_max.max(risk);
        cell.channels |= channels;
        cell.measured |= measured;
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
        for cell in self.cells.iter_mut() {
            cell.points.clear();
        }
        for p in points {
            let id = self.locate(&p.x) as usize;
            self.cells[id].points.push(p.clone());
        }
        for cell in self.cells.iter_mut() {
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
        }
    }

    /// Write the atlas as CSV, the machine-readable form a reviewer can open in anything.
    #[must_use]
    pub fn to_csv(&self) -> String {
        use std::fmt::Write as _;
        let root = self.cells[0].clone();
        let mut out = String::new();
        let _ = write!(
            out,
            "cell,depth,label,samples,mean_risk,max_risk,channels,measured,relative_size"
        );
        for i in 0..root.bounds.len() {
            let _ = write!(out, ",lo{i},hi{i}");
        }
        out.push('\n');
        for id in &self.leaves {
            let c = &self.cells[*id as usize];
            let _ = write!(
                out,
                "{},{},{},{},{:.6},{:.6},{},{},{:.6}",
                c.id,
                c.depth,
                c.label.name(),
                c.samples,
                c.mean_risk(),
                c.risk_max,
                c.channels,
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
    fn a_single_point_does_not_drill_the_atlas_to_the_depth_limit() {
        // Measured, before the sample requirement was applied to every branch of `refine`: one
        // suspicious leaf was split anyway, so a single NaN drilled to `max_depth`, the atlas of a
        // one-dimensional model grew to 881 leaves holding one evaluation each at a 640 budget, no
        // leaf ever reached the four samples a TRUSTED needs, nothing was trusted, no two labelled
        // cells disagreed, and a model with one known boundary reported no band at all.
        let mut a = Atlas::new(&model1d(0.0, 1.0), Policy::default());
        a.record(&[0.9], 1.0, 0b1);
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
    fn a_discrete_axis_still_bisects() {
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
            [0.0, 3.0],
            "index space, not value space"
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
            measured: 0,
            points: Vec::new(),
        }
    }
}
