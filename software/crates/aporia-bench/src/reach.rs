//! Can this atlas, under this policy, enclose the region an entry declares?
//!
//! E1.3 asked whether a truth-blind trust rule could stop a leaf being called TRUSTED while a declared
//! region lay unsampled inside it, and 0038's post-mortem found that on two of its entries the question
//! could not have been answered by any trust rule: no leaf the policy permits can be half inside those
//! regions, so localisation was unreachable before any labelling decision was made. That is a property of
//! the partition and the depth cap, and it is computable before a run — which is the only reason it is
//! worth knowing, because a limit discovered afterwards contaminates the reading of the measurement.
//!
//! So this module asks the atlas what it could build ([`aporia_boundary::Atlas::permitted_partition`])
//! and asks the region's own declaration what a leaf would overlap ([`crate::truth::Declared`]). There is
//! no second geometry here and no second truth language: the boxes come from the code that splits, and
//! the share comes from the code that scores. A Python script or a hand-written table would have been a
//! third definition of "enclosed", and two definitions of a threshold decide which entries an experiment
//! may score.
//!
//! The classes are deliberately not two. `SubGranular` (no leaf reaches the bar on even the region's
//! box) and `CarvedAway` (leaves reach it on the box, and the `where` takes them under it again) are the
//! same *answer* to E1.3's question and different *explanations*, and 0038's account of why the frozen
//! condition failed turns on which one a row is. `Unreachable` is a third thing again: a region no
//! permitted leaf intersects at all, which is what an endpoint-only declaration comes to.
//!
//! None of this is a safety claim. An unresolvable entry is not a safe entry, and a class is not a
//! verdict about the model — it is a statement about the instrument, and the harness refuses to run a
//! protocol whose resolvable set contains an entry the instrument cannot resolve rather than quietly
//! dropping it or counting it as no detection.

use crate::corpus::Entry;
use crate::metrics::LOCALISATION_BAR;
use crate::truth::{Declared, LATTICE_PER_AXIS};
use aporia_boundary::{Atlas, Cell, Policy};
use aporia_ir::Model;
use aporia_store::Json;

/// How many candidate leaves one region's carved share is measured on before the check says it cannot
/// answer rather than answering expensively.
///
/// A `where` region's share is sampled on the metric's own lattice — 32 points per axis, per leaf — and
/// candidates are examined in descending order of envelope share, so the ones a cap cuts off are the
/// least able to reach the bar. Box regions cost nothing: their first candidate is its own witness.
/// Falling short of the cap is reported as [`Class::CarvedAway`] with both counts recorded, so a reader
/// can see how much was looked at and how much there was to look at.
pub const WITNESS_CAP: usize = 32;

/// What the instrument can do with one declared region, given the partition it is permitted to build.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    /// Some permitted leaf is at least [`LOCALISATION_BAR`] inside the region as declared. The entry's
    /// fault is reachable at the spatial granularity the metric scores.
    Enclosed,
    /// Leaves reach the bar inside the region's box, and none of the witnesses measured reaches it after
    /// the `where` carve. The region is the right size and the wrong shape for this partition.
    CarvedAway,
    /// No permitted leaf reaches the bar inside the region's box: the region is thinner than the finest
    /// leaf the depth cap allows. Nothing a trust rule does can localise it, because localisation is
    /// decided on a leaf's overlap and every leaf overlaps too much of the rest of the domain.
    SubGranular,
    /// No permitted leaf intersects the region at all — a zero-width declaration, or a region outside
    /// every box the partition holds.
    Unreachable,
    /// The entry declares no region to enclose: a control.
    NoRegion,
    /// The model does not compile, so there is no partition. E1.3's refused domains are this class, and
    /// it is reported as a refusal and never as a measurement.
    NoModel,
    /// The permitted partition is too large to enumerate under [`Atlas::PARTITION_CELL_CAP`], so the
    /// question has no answer from this instrument.
    Uncappable,
}

impl Class {
    /// Every class, in the order a report lists them: resolvable first, then the two ways a region can
    /// be too thin, then the ways an entry cannot be measured at all.
    pub const ALL: [Class; 7] = [
        Class::Enclosed,
        Class::CarvedAway,
        Class::SubGranular,
        Class::Unreachable,
        Class::NoRegion,
        Class::NoModel,
        Class::Uncappable,
    ];

    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Enclosed => "enclosed",
            Self::CarvedAway => "carved-away",
            Self::SubGranular => "sub-granular",
            Self::Unreachable => "unreachable",
            Self::NoRegion => "no-region",
            Self::NoModel => "no-model",
            Self::Uncappable => "uncappable",
        }
    }

    /// True only for the class that says a localisation verdict is reachable. Written as a method
    /// because the harness's refusal and an entry's own claim both have to ask this question, and two
    /// spellings of `== Enclosed` is where a third class would be silently excluded.
    #[must_use]
    pub fn resolvable(self) -> bool {
        self == Self::Enclosed
    }

    /// The order an entry's several regions are reported in: the most restrictive finding wins. This is
    /// a reporting order, not a severity claim — `CarvedAway` beside `SubGranular` says which arithmetic
    /// produced the answer, not which failure is worse.
    #[must_use]
    pub fn rank(self) -> u8 {
        match self {
            Self::Enclosed => 0,
            Self::CarvedAway => 1,
            Self::SubGranular => 2,
            Self::Unreachable => 3,
            Self::NoRegion => 4,
            Self::NoModel => 5,
            Self::Uncappable => 6,
        }
    }
}

/// One region's answer, with the arithmetic that produced it recorded beside it.
#[derive(Clone, Debug)]
pub struct RegionReach {
    pub index: usize,
    /// The declaration's own `reason`, so a reader can point at the region this row describes.
    pub reason: String,
    pub class: Class,
    /// The largest share of any permitted leaf that lies inside this region's box. An upper bound on
    /// [`Self::best_declared_share`], and equal to it when the region declares no `where`.
    pub best_envelope_share: f64,
    /// The largest carved share among the leaves this check measured.
    pub best_declared_share: f64,
    /// Depth of the best witness, when one reached the bar.
    pub witness_depth: Option<u8>,
    /// Bounds of the best witness, so a class can be checked by hand rather than taken on trust.
    pub witness_bounds: Vec<[f64; 2]>,
    /// Permitted leaves that reach the bar on the box, and how many of them were measured.
    pub candidates: usize,
    pub examined: usize,
    /// Width of the finest permitted leaf along each axis, as a fraction of the declared domain — the
    /// granularity a region has to beat, stated rather than implied.
    pub finest_leaf_share: Vec<f64>,
}

impl RegionReach {
    #[must_use]
    pub fn json(&self) -> Json {
        Json::object(vec![
            ("region", Json::count(self.index as u64)),
            ("reason", Json::text(self.reason.clone())),
            ("class", Json::text(self.class.name())),
            (
                "best_envelope_share",
                Json::number(self.best_envelope_share),
            ),
            (
                "best_declared_share",
                Json::number(self.best_declared_share),
            ),
            (
                "witness_depth",
                self.witness_depth
                    .map_or(Json::Null, |d| Json::count(u64::from(d))),
            ),
            (
                "witness_bounds",
                Json::Arr(
                    self.witness_bounds
                        .iter()
                        .map(|[lo, hi]| Json::Arr(vec![Json::number(*lo), Json::number(*hi)]))
                        .collect(),
                ),
            ),
            ("candidates", Json::count(self.candidates as u64)),
            ("examined", Json::count(self.examined as u64)),
            (
                "finest_leaf_share",
                Json::Arr(
                    self.finest_leaf_share
                        .iter()
                        .map(|w| Json::number(*w))
                        .collect(),
                ),
            ),
        ])
    }

    /// One line for the command's own report.
    #[must_use]
    pub fn row(&self) -> String {
        let bounds = if self.witness_bounds.is_empty() {
            String::new()
        } else {
            format!(
                "  witness {:?}",
                self.witness_bounds
                    .iter()
                    .map(|[lo, hi]| format!("[{lo}, {hi}]"))
                    .collect::<Vec<_>>()
            )
        };
        format!(
            "  region {} {class:<13} envelope {:.6} declared {:.6}  {} candidate(s), {} measured  \
             finest leaf {}  {}{reason})",
            self.index,
            self.best_envelope_share,
            self.best_declared_share,
            self.candidates,
            self.examined,
            self.finest_leaf_share
                .iter()
                .map(|w| format!("{w:.6}"))
                .collect::<Vec<_>>()
                .join(" x "),
            bounds,
            class = self.class.name(),
            reason = self.reason,
        )
    }
}

/// One entry's reachability, aggregated over everything it declares.
#[derive(Clone, Debug)]
pub struct Reach {
    pub entry: String,
    /// The digest of the `truth.json` these regions were read from: a class is a claim about a
    /// declaration, and an edited declaration has to show up as a different record.
    pub truth_digest: String,
    pub class: Class,
    pub max_depth: u8,
    pub axes: usize,
    /// How many permitted leaves the enumeration held, which is the size of the search this check made.
    pub permitted_cells: usize,
    pub regions: Vec<RegionReach>,
}

impl Reach {
    #[must_use]
    pub fn json(&self) -> Json {
        Json::object(vec![
            ("entry", Json::text(self.entry.clone())),
            ("truth_digest", Json::text(self.truth_digest.clone())),
            ("class", Json::text(self.class.name())),
            ("max_depth", Json::count(u64::from(self.max_depth))),
            ("axes", Json::count(self.axes as u64)),
            ("permitted_cells", Json::count(self.permitted_cells as u64)),
            (
                "regions",
                Json::Arr(self.regions.iter().map(RegionReach::json).collect()),
            ),
        ])
    }

    #[must_use]
    pub fn row(&self) -> String {
        let mut out = format!(
            "{:<34} {:<13} {} axis(s), depth cap {}, {} permitted leaf/leaves, {} region(s)",
            self.entry,
            self.class.name(),
            self.axes,
            self.max_depth,
            self.permitted_cells,
            self.regions.len(),
        );
        for region in &self.regions {
            out.push('\n');
            out.push_str(&region.row());
        }
        out
    }
}

/// What the atlas can do with one entry's declarations under `policy`.
///
/// Deterministic by construction: the partition enumeration has no evidence input, the candidate order
/// is by descending envelope share then enumeration order, and the lattice the carve is sampled on is
/// the metric's own. The same entry and policy give the same record on any machine, which is what lets
/// this run at load time in front of an experiment rather than as a post-hoc annotation of one.
#[must_use]
pub fn reach(entry: &Entry, policy: &Policy) -> Reach {
    let Some(model) = entry.model.as_ref() else {
        return Reach {
            entry: entry.id(),
            truth_digest: entry.truth_digest.clone(),
            class: Class::NoModel,
            max_depth: policy.max_depth,
            axes: 0,
            permitted_cells: 0,
            regions: Vec::new(),
        };
    };
    if entry.truth.regions.is_empty() {
        return Reach {
            entry: entry.id(),
            truth_digest: entry.truth_digest.clone(),
            class: Class::NoRegion,
            max_depth: policy.max_depth,
            axes: model.params.len(),
            permitted_cells: 0,
            regions: Vec::new(),
        };
    }
    let Some(cells) = Atlas::permitted_partition(model, *policy) else {
        return Reach {
            entry: entry.id(),
            truth_digest: entry.truth_digest.clone(),
            class: Class::Uncappable,
            max_depth: policy.max_depth,
            axes: model.params.len(),
            permitted_cells: 0,
            regions: Vec::new(),
        };
    };
    let regions: Vec<RegionReach> = entry
        .truth
        .regions
        .iter()
        .enumerate()
        .map(|(index, region)| classify(index, region, model, &cells))
        .collect();
    let class = regions
        .iter()
        .map(|r| r.class)
        .max_by_key(|c| c.rank())
        .unwrap_or(Class::NoRegion);
    Reach {
        entry: entry.id(),
        truth_digest: entry.truth_digest.clone(),
        class,
        max_depth: policy.max_depth,
        axes: model.params.len(),
        permitted_cells: cells.len(),
        regions,
    }
}

/// One region against the whole permitted partition.
fn classify(index: usize, region: &Declared, model: &Model, cells: &[Cell]) -> RegionReach {
    let mut best_envelope = 0.0f64;
    let mut candidates: Vec<(f64, usize)> = Vec::new();
    for (at, cell) in cells.iter().enumerate() {
        let share = region.envelope_share(model, &cell.bounds);
        if share > best_envelope {
            best_envelope = share;
        }
        if share >= LOCALISATION_BAR {
            candidates.push((share, at));
        }
    }
    let finest_leaf_share: Vec<f64> = (0..model.params.len())
        .map(|axis| {
            cells
                .iter()
                .map(|c| c.width(axis) / crate::truth::axis_width(model, axis).max(1e-30))
                .fold(1.0f64, f64::min)
        })
        .collect();
    let empty = RegionReach {
        index,
        reason: region.reason.clone(),
        class: Class::Unreachable,
        best_envelope_share: best_envelope,
        best_declared_share: 0.0,
        witness_depth: None,
        witness_bounds: Vec::new(),
        candidates: candidates.len(),
        examined: 0,
        finest_leaf_share: finest_leaf_share.clone(),
    };
    // An envelope share of nothing is not a small share: a zero-width declaration has no volume for a
    // leaf to be half inside, however finely the atlas could cut. Named separately because the two need
    // different fixes — one is a depth limit, the other is a truth file that declares a point.
    if best_envelope <= 0.0 {
        return RegionReach {
            class: Class::Unreachable,
            ..empty
        };
    }
    if candidates.is_empty() {
        // Nothing reached the bar even before the carve, so the depth arithmetic has already answered:
        // no leaf of this partition can be half inside this region. Measured on the E1.3 entries, and
        // the reason 0038's account of the failed condition is about granularity and not about evidence.
        return RegionReach {
            class: Class::SubGranular,
            ..empty
        };
    }
    // Descending envelope share, then enumeration order, so the cap cuts the least able candidates and a
    // repeated run measures the same ones.
    candidates.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.1.cmp(&b.1))
    });
    let found = candidates.len();
    let examined = found.min(WITNESS_CAP);
    let mut best_declared = 0.0f64;
    let mut witness: Option<(u8, Vec<[f64; 2]>)> = None;
    for (share, at) in candidates.into_iter().take(examined) {
        let cell = &cells[at];
        // `share` is the envelope figure this candidate was selected on; a box region's carved share is
        // the same number, so the first candidate is its own witness and costs one evaluation.
        let declared = if region.predicate.is_none() {
            share
        } else {
            region.overlap_fraction(model, &cell.bounds, LATTICE_PER_AXIS)
        };
        if declared > best_declared {
            best_declared = declared;
        }
        if declared >= LOCALISATION_BAR {
            witness = Some((cell.depth, cell.bounds.clone()));
            break;
        }
    }
    RegionReach {
        index,
        reason: region.reason.clone(),
        class: if witness.is_some() {
            Class::Enclosed
        } else {
            Class::CarvedAway
        },
        best_envelope_share: best_envelope,
        best_declared_share: best_declared,
        witness_depth: witness.as_ref().map(|(d, _)| *d),
        witness_bounds: witness.map(|(_, b)| b).unwrap_or_default(),
        candidates: found,
        examined,
        finest_leaf_share,
    }
}

/// Every entry's reachability, in corpus order.
#[must_use]
pub fn reaches(entries: &[Entry], policy: &Policy) -> Vec<Reach> {
    entries.iter().map(|e| reach(e, policy)).collect()
}

/// The results-document section: one record per covered entry, beside the digest of the declaration
/// each one was computed from.
///
/// Deliberately outside the measurement identity. A class is a statement about the instrument and the
/// corpus, not about what a run measured, and folding it into the hashed `entries` section would have
/// renamed every committed measurement the moment the check existed — which is the trade the trust
/// variant documents at [`crate::harness::plan_json`], taken the same way here.
#[must_use]
pub fn section(entries: &[Entry], policy: &Policy) -> Json {
    Json::Arr(reaches(entries, policy).iter().map(Reach::json).collect())
}
