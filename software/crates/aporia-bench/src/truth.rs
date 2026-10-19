//! Ground truth: what the corpus says is really wrong, and whether that claim holds up.
//!
//! A truth declaration is only useful if it can be *checked*. Under `aporia.truth/1` every region is
//! a union of axis-aligned boxes in the model's own parameter names, which means a dense grid
//! evaluation can answer the one question that matters before any search result is trusted: does the
//! model actually fail inside the region it claims to fail inside. [`crate::corpus::verify`] answers
//! that, and `aporia-bench verify` runs it over the whole corpus.
//!
//! `aporia.truth/2` adds one thing: a region may carry a `where` expression that carves its box down
//! to an exact set ([`crate::predicate`]), so a diagonal or curved region can be *declared* rather
//! than approximated by a staircase of boxes. The box stays required, as the envelope: it keeps every
//! measure finite and every sample bounded. A box-only region — all 22 committed entries — takes the
//! closed-form path and its numbers do not move.
//!
//! The evaluation used for verification is the model's own declared rules, applied directly — not the
//! calibrated, fused, atlas-scored pipeline that the search uses. That distinction is the whole point
//! of a control group: if the same machinery produced both the answer and the marking, a detection
//! rate would be measuring agreement with itself.

use crate::predicate::Predicate;
use aporia_ir::Model;
use aporia_store::Json;

/// Samples per axis when a `where` region's measure is estimated. Written into the metric definition
/// before any curved entry existed (0037 §3): a lattice this coarse is a real approximation, which is
/// why [`LATTICE_SENSITIVITY_PER_AXIS`] exists and why the boundary of a curved region is the audit's
/// problem rather than this constant's.
pub const LATTICE_PER_AXIS: usize = 32;
/// The doubled lattice an outcome is re-measured on to say whether its localisation decision was a
/// sampling artifact rather than a fact. See [`crate::corpus`] and the frozen protocol.
pub const LATTICE_SENSITIVITY_PER_AXIS: usize = 64;

/// The largest elementary-box decomposition [`Truth::union_overlap_at`] will enumerate before it
/// refuses. A million boxes is already more arithmetic than a row of a sweep should spend on one
/// cell's geometry, and the point of a bound is that the measure says *not measurable* rather than
/// quietly returning an approximation a reader cannot tell apart from the exact case.
pub const UNION_DECOMPOSITION_LIMIT: usize = 1 << 20;

/// The truth schemas a reader of this build understands.
///
/// `aporia.truth/3` adds the record an E1.3b entry has to carry: what the model computes, what each
/// declared variable *is* (its unit and its span, so the audit can compare them against the model
/// rather than against prose), the failure mechanism as a key the corpus can test for uniqueness, the
/// channels the entry expects to speak, an explicit resolvability claim the instrument checks against
/// its own arithmetic, and where the truth came from. None of it changes a measure: the regions, the
/// predicates and the lattice are `/2`'s, unchanged, which is why `/2` files stay readable and their
/// digests stay put.
pub const TRUTH_SCHEMAS: [&str; 3] = ["aporia.truth/1", "aporia.truth/2", "aporia.truth/3"];

/// One axis interval of a declared region, in parameter-name space.
#[derive(Clone, Debug, PartialEq)]
pub struct Declared {
    pub reason: String,
    /// `(parameter name, [low, high])`, inclusive. A name that is not listed is unconstrained.
    pub axes: Vec<(String, [f64; 2])>,
    /// A `where` expression that carves the box down to the exact set, in `aporia.truth/2`. Absent is
    /// the box itself, which is every entry written before the curved-shape experiment.
    pub predicate: Option<Predicate>,
}

impl Declared {
    /// Is this point inside the region? A predicate region answers false wherever its expression is
    /// undefined, which is [`Declared::undefined`]'s business, not a silent one.
    #[must_use]
    pub fn contains(&self, model: &Model, x: &[f64]) -> bool {
        if !self.envelope_holds(model, x) {
            return false;
        }
        match &self.predicate {
            None => true,
            Some(p) => self.values(model, x).is_some_and(|values| p.holds(&values)),
        }
    }

    /// Is the point inside the box, ignoring any predicate? This is the conservative test the search
    /// uses to decide whether a cell is even worth sampling.
    #[must_use]
    pub fn envelope_holds(&self, model: &Model, x: &[f64]) -> bool {
        self.axes.iter().all(|(name, [lo, hi])| {
            let Some(i) = axis_of(model, name) else {
                return false;
            };
            let Some(v) = x.get(i).copied() else {
                return false;
            };
            v >= *lo && v <= *hi
        })
    }

    /// Does this region's box intersect a cell? Unresolved by the predicate, so a cell that only
    /// touches the envelope outside the carved set still passes — [`Declared::overlap_fraction`] then
    /// reports the share that is really inside.
    #[must_use]
    pub fn overlaps(&self, model: &Model, cell: &[[f64; 2]]) -> bool {
        self.axes.iter().all(|(name, [lo, hi])| {
            let Some(i) = axis_of(model, name) else {
                return true;
            };
            let Some([clo, chi]) = cell.get(i) else {
                return false;
            };
            chi >= lo && clo <= hi
        })
    }

    /// The region's box as a fraction of the declared domain. A predicate region multiplies this by
    /// the share of its own lattice the expression accepts, in [`Declared::volume_fraction`].
    #[must_use]
    pub fn envelope_fraction(&self, model: &Model) -> f64 {
        self.axes.iter().fold(1.0, |acc, (name, [lo, hi])| {
            let Some(i) = axis_of(model, name) else {
                return acc;
            };
            let width = axis_width(model, i);
            if width <= 0.0 {
                return acc;
            }
            acc * ((hi - lo) / width).clamp(0.0, 1.0)
        })
    }

    /// The region's bound on the model's `axis`, when the declaration names that parameter.
    ///
    /// A name the model does not declare is reported as no bound rather than as a whole axis, which is
    /// the same reading [`Declared::clipped`] gives: an unconstrained region is *nowhere*, not
    /// everywhere, because a truth file whose names all failed to resolve would otherwise claim the
    /// entire domain. `corpus::verify` refuses such a file before a measurement runs; this is what the
    /// arithmetic says if it is ever bypassed.
    #[must_use]
    pub fn axis_span(&self, model: &Model, axis: usize) -> Option<[f64; 2]> {
        self.axes
            .iter()
            .find_map(|(name, span)| (axis_of(model, name) == Some(axis)).then_some(*span))
    }

    /// Does this region cover the whole box `query`? Exact for a box region: `query` is an atom of a
    /// decomposition cut at every region bound, so a region either contains all of it or none of it,
    /// and no comparison here needs a tolerance — the cut points are copies of these same bounds.
    #[must_use]
    pub fn covers(&self, model: &Model, query: &[[f64; 2]]) -> bool {
        let mut named = false;
        for (axis, [lo, hi]) in query.iter().enumerate() {
            let Some([rlo, rhi]) = self.axis_span(model, axis) else {
                continue;
            };
            named = true;
            if *lo < rlo || *hi > rhi {
                return false;
            }
        }
        named
    }

    /// Fraction of the *domain* the region covers. For a `where` region this is the envelope fraction
    /// times the accepted share of a fixed lattice inside the envelope, so it is an estimate whose
    /// resolution is a property of the metric, identical for every arm of every comparison.
    #[must_use]
    pub fn volume_fraction(&self, model: &Model, per_axis: usize) -> f64 {
        let envelope = self.envelope_fraction(model);
        if self.predicate.is_none() {
            return envelope;
        }
        let area: Vec<[f64; 2]> = self.axes.iter().map(|(_, span)| *span).collect();
        envelope * self.accepted_share(model, &area, per_axis)
    }

    /// Overlap between a cell and the region, as a fraction of the cell — the quantity `localised` is
    /// decided on ("a cell at least half inside a declared region"). For a `where` region the accepted
    /// share is sampled on the same fixed lattice over `cell ∩ envelope`, then scaled by how much of
    /// the cell that intersection covers, which keeps the meaning the box case has: how much of *this
    /// cell* is really wrong.
    #[must_use]
    pub fn overlap_fraction(&self, model: &Model, cell: &[[f64; 2]], per_axis: usize) -> f64 {
        let (box_fraction, intersection) = self.clipped(model, cell);
        if box_fraction <= 0.0 {
            return 0.0;
        }
        if self.predicate.is_none() {
            return box_fraction;
        }
        box_fraction * self.accepted_share(model, &intersection, per_axis)
    }

    /// How much of `cell` lies inside the region's **envelope** — the box the declaration names, before
    /// any `where` carves it. A predicate region is never larger than its envelope, so this is an upper
    /// bound on [`Declared::overlap_fraction`], and the reachability check uses it as one: a cell that
    /// does not reach the bar here cannot reach it after the carve either.
    ///
    /// It is a method here rather than arithmetic in the check because the clip is the definition: two
    /// copies of `(hi.min(chi) - lo.max(clo)) / width` is two numbers that can drift, and the drift
    /// would decide which entries an experiment is allowed to score.
    #[must_use]
    pub fn envelope_share(&self, model: &Model, cell: &[[f64; 2]]) -> f64 {
        self.clipped(model, cell).0
    }

    /// The cell clipped to this region's box, as (share of the cell, the intersection per axis).
    #[must_use]
    fn clipped(&self, model: &Model, cell: &[[f64; 2]]) -> (f64, Vec<[f64; 2]>) {
        let mut box_fraction = 1.0;
        let mut intersection: Vec<[f64; 2]> = Vec::with_capacity(cell.len());
        let mut constrained = false;
        for (name, [lo, hi]) in &self.axes {
            let Some(i) = axis_of(model, name) else {
                continue;
            };
            let Some([clo, chi]) = cell.get(i) else {
                return (0.0, intersection);
            };
            constrained = true;
            let width = (chi - clo).max(1e-30);
            let isect = (hi.min(*chi) - lo.max(*clo)).max(0.0);
            box_fraction *= isect / width;
            intersection.push([lo.max(*clo), hi.min(*chi)]);
        }
        if !constrained {
            // An unconstrained region would be "everywhere", which the corpus never declares.
            return (0.0, intersection);
        }
        (box_fraction, intersection)
    }
    /// Would this region's predicate be undefined anywhere in the box `area`? A NaN makes every
    /// comparison false, which would look exactly like an empty region — so the audit asks this before
    /// anything is measured, and a region that cannot say is refused rather than scored.
    #[must_use]
    pub fn undefined_in(&self, model: &Model, area: &[[f64; 2]], per_axis: usize) -> bool {
        let Some(predicate) = &self.predicate else {
            return false;
        };
        self.for_each_point(model, area, per_axis, &mut |x| predicate.undefined(x))
    }

    /// The share of `area`'s lattice that the predicate accepts. A box region is 1.0 by construction.
    fn accepted_share(&self, model: &Model, area: &[[f64; 2]], per_axis: usize) -> f64 {
        let Some(predicate) = &self.predicate else {
            return 1.0;
        };
        let mut accepted = 0u64;
        let mut total = 0u64;
        self.for_each_point(model, area, per_axis, &mut |x| {
            total += 1;
            if predicate.holds(x) {
                accepted += 1;
            }
            false
        });
        if total == 0 {
            return 0.0;
        }
        accepted as f64 / total as f64
    }

    /// The values of this region's predicate variables at `x`, in first-seen name order. `None` when a
    /// name is not a parameter of this model — which `corpus::verify` refuses before a measurement, and
    /// which here answers "not inside" rather than guessing.
    fn values(&self, model: &Model, x: &[f64]) -> Option<Vec<f64>> {
        let predicate = self.predicate.as_ref()?;
        predicate
            .vars()
            .iter()
            .map(|name| {
                axis_of(model, name)
                    .and_then(|i| x.get(i).copied())
                    .ok_or(())
            })
            .collect::<Result<Vec<_>, _>>()
            .ok()
    }

    /// Walk the midpoint lattice of `area`, calling `visit` with one point's values. The lattice is
    /// `per_axis` samples per axis at `lo + (i + 0.5) · width / per_axis`, so it never lands on a
    /// domain edge and never depends on a caller's iteration order.
    fn for_each_point(
        &self,
        model: &Model,
        area: &[[f64; 2]],
        per_axis: usize,
        visit: &mut dyn FnMut(&mut Vec<f64>) -> bool,
    ) -> bool {
        let Some(predicate) = &self.predicate else {
            return false;
        };
        let axes: Vec<usize> = predicate
            .vars()
            .iter()
            .filter_map(|name| axis_of(model, name))
            .collect();
        if axes.is_empty() || area.is_empty() {
            return false;
        }
        let mut values = vec![0.0; axes.len()];
        let total = per_axis.pow(axes.len() as u32);
        for index in 0..total {
            for (slot, axis) in axes.iter().enumerate() {
                let [lo, hi] = area.get(*axis).copied().unwrap_or([0.0, 0.0]);
                let width = (hi - lo).max(0.0);
                let digit = (index / per_axis.pow(slot as u32)) % per_axis;
                values[slot] = lo + width * (digit as f64 + 0.5) / per_axis as f64;
            }
            if visit(&mut values) {
                return true;
            }
        }
        false
    }
}

/// One declared value of a discrete axis and what the entry says happens there.
#[derive(Clone, Debug, PartialEq)]
pub struct ChoiceClaim {
    pub axis: String,
    pub value: f64,
    /// `"fails"` or `"clean"`. Anything else is refused by the audit, because a per-choice claim that
    /// says neither is the decoration this check exists to catch.
    pub expect: String,
}

/// One declared parameter of the model, as the entry describes it.
///
/// The `unit` is in the IR's own canonical form — `m`, `m/s^2`, `1` for a dimensionless quantity — so
/// the audit can compare it against what the model actually declares with the parser that produced it
/// ([`aporia_ir::Dimension::parse_canonical`]) instead of against a second vocabulary of units invented
/// here. `lo` and `hi` are the declared span, checked the same way against the model's domain: a truth
/// file that describes a different model than the one beside it is a claim about nothing.
#[derive(Clone, Debug, PartialEq)]
pub struct Variable {
    pub name: String,
    pub unit: String,
    pub role: String,
    pub lo: f64,
    pub hi: f64,
}

/// A boundary the entry claims, with the tolerance the metric is allowed.
#[derive(Clone, Debug, PartialEq)]
pub struct Boundary {
    pub axis: String,
    pub at: f64,
    pub tolerance: f64,
}

/// A truth declaration as read from `truth.json`.
#[derive(Clone, Debug, PartialEq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "these are the corpus's own flags, read straight from truth.json"
)]
pub struct Truth {
    pub method: String,
    pub fault: String,
    pub derivation: String,
    pub regions: Vec<Declared>,
    pub boundaries: Vec<Boundary>,
    /// A control has no region, so everything suspicious in it is a false positive by definition.
    pub control: bool,
    /// The entry is expected to be rejected by the compiler rather than found by a search.
    pub static_expected: bool,
    pub narrow: bool,
    pub curved: bool,
    pub degenerate: bool,
    pub expects: String,
    /// The existing corpus entry this one is difficulty-matched to, and the reason the audit can
    /// check that a new entry is not quietly easier or harder than the rung it claims to extend.
    /// Only `aporia.truth/2` may state it, for the same reason it may state a `where`: an older
    /// reader would drop the claim rather than honour it.
    pub matched_to: Option<String>,
    /// Per-value claims about a discrete axis: which choices fail and which are clean. 0029's E12
    /// requires a *consistent mapping of every discrete value*, and a truth file that only lists the
    /// failing boxes leaves the clean ones unstated — an absence an audit cannot tell apart from an
    /// oversight. `aporia.truth/2` only.
    pub choice_claims: Vec<ChoiceClaim>,
    /// Which schema the file declared. `truth/2` is the only one allowed to carry a `where`, and the
    /// reader refuses the other way round, so a curved region can never be silently measured as the
    /// box around it — which would widen the region and flatter every metric that reads it.
    pub schema: String,
    /// What the model computes, in the entry's own words. `aporia.truth/3` only.
    pub meaning: String,
    /// Every declared parameter, with the unit and span the entry says it has. The audit insists that
    /// this list is exactly the model's own parameter list, which is what makes it a check rather than
    /// a caption: an entry that describes a different model than the one it points at measures nothing.
    /// `aporia.truth/3` only.
    pub variables: Vec<Variable>,
    /// The failure mechanism as a key. Two entries with one mechanism are one case counted twice, and
    /// the audit compares this string across the corpus — which is why it is a field and not a phrase
    /// buried in `derivation`, where no comparison could reach it. `aporia.truth/3` only.
    pub mechanism: String,
    /// Which evidence channels the entry expects to have something to say about its region. Recorded
    /// before the run, so an arm that goes quiet on a channel this entry named is a finding and not a
    /// surprise. `aporia.truth/3` only.
    pub channels: Vec<String>,
    /// The author's claim about whether the atlas can enclose this entry's regions at all. Checked
    /// against the instrument's own reachability arithmetic, not against the prose beside it: an entry
    /// that claims to be resolvable and is not is refused before it is run, and an entry that states no
    /// claim cannot be in the resolvable set. `aporia.truth/3` only.
    pub resolvable: Option<bool>,
    /// The resolvability calculation, in numbers a reader can reproduce: the leaf width the depth cap
    /// permits, the region's own width, and how many leaves fit across it. `aporia.truth/3` only.
    pub resolvability: String,
    /// Where the truth came from that is not this instrument. A closed form, a standard, an identity —
    /// never a run's output, which is the defect 0037's correction C8 was written to keep out of the
    /// corpus and which `/3` makes a field the audit can actually look at. `aporia.truth/3` only.
    pub source: String,
    /// Why this entry belongs to E1.3b rather than to the experiment that ran before it. `aporia.truth/3`
    /// only.
    pub why: String,
}

/// The fields only `aporia.truth/3` defines. Named once so the refusal and the reader cannot disagree
/// about which fields belong to which schema.
const TRUTH3_FIELDS: [&str; 8] = [
    "meaning",
    "variables",
    "mechanism",
    "channels",
    "resolvable",
    "resolvability",
    "source",
    "why",
];

/// Refuse a file that states the `/3` record under an older schema.
///
/// The reason is the same one that makes a `where` a refused field under `/1`: a reader that drops a
/// claim silently is a reader that measured a different entry than the file describes. Here the dropped
/// claim would be the entry's own resolvability, which is the field E1.3b's deciding universe is built
/// from, so an ignored `resolvable` would put an entry in a set it never claimed to be in.
fn refuse_outside_schema(value: &Json, schema: &str, v3: bool) -> Result<(), String> {
    if v3 {
        return Ok(());
    }
    for key in TRUTH3_FIELDS {
        if value.get(key).is_some() {
            return Err(format!(
                "{schema} files may not state {key:?}: only aporia.truth/3 defines what it means,                  and a reader that ignored it would measure an entry with no resolvability claim"
            ));
        }
    }
    Ok(())
}

/// The boundaries a file claims, with the tolerance the metric is allowed.
///
/// A malformed boundary row is dropped rather than refused, as it always has been: `at` and `tolerance`
/// are checked by name by the corpus audit, which is where an unusable boundary becomes a refusal with a
/// message a reader can act on.
#[must_use]
fn read_boundaries(value: &Json) -> Vec<Boundary> {
    value
        .get("boundaries")
        .and_then(Json::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    Some(Boundary {
                        axis: item.get("axis")?.as_str()?.to_string(),
                        at: item.get("at")?.as_f64()?,
                        tolerance: item.get("tolerance")?.as_f64()?,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Per-value claims about discrete axes, exactly as `/2` introduced them.
#[must_use]
fn read_choice_claims(value: &Json) -> Vec<ChoiceClaim> {
    value
        .get("choice_claims")
        .and_then(Json::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let axis = item.get("axis")?.as_str()?.to_string();
                    let value_at = item.get("value")?.as_f64()?;
                    let expect = item.get("expect")?.as_str()?.to_string();
                    Some(ChoiceClaim {
                        axis,
                        value: value_at,
                        expect,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Every declared variable, read strictly: a name, a unit in the IR's canonical form, a role, and a
/// finite span. A half-stated variable is refused here rather than compared against the model later,
/// because a missing field would be found "equal" to whatever the comparison defaulted to.
fn read_variables(value: &Json) -> Result<Vec<Variable>, String> {
    let items = value
        .get("variables")
        .and_then(Json::as_array)
        .ok_or_else(|| "truth: variables is missing".to_string())?;
    let mut out = Vec::new();
    for (at, item) in items.iter().enumerate() {
        let number = |k: &str| -> Result<f64, String> {
            item.get(k)
                .and_then(Json::as_f64)
                .ok_or_else(|| format!("truth: variable {at} has no {k}"))
        };
        let name = item
            .get("name")
            .and_then(Json::as_str)
            .ok_or_else(|| format!("truth: variable {at} has no name"))?
            .to_string();
        let lo = number("lo")?;
        let hi = number("hi")?;
        if !lo.is_finite() || !hi.is_finite() {
            return Err(format!(
                "truth: variable {name:?} declares a non-finite span [{lo}, {hi}]"
            ));
        }
        let unit = item
            .get("unit")
            .and_then(Json::as_str)
            .ok_or_else(|| format!("truth: variable {name:?} declares no unit in canonical form"))?
            .to_string();
        let role = item
            .get("role")
            .and_then(Json::as_str)
            .ok_or_else(|| format!("truth: variable {name:?} declares no role"))?
            .to_string();
        out.push(Variable {
            name,
            unit,
            role,
            lo,
            hi,
        });
    }
    Ok(out)
}

impl Truth {
    /// Read a `truth.json`. An unknown schema is refused; unknown flag fields are ignored, so a newer
    /// corpus file can still be measured by an older harness — but a file that uses a `where` cannot,
    /// because an older reader would drop the predicate and measure its envelope instead.
    #[expect(
        clippy::too_many_lines,
        reason = "one document read in one function: the fields of a truth file are the shape of the \
                  corpus's claim, and splitting the rest of them into helpers that each return a piece \
                  would put the requirement that a schema permits a field somewhere other than where \
                  the field is read"
    )]
    pub fn from_json(value: &Json) -> Result<Self, String> {
        let schema = value
            .get("schema")
            .and_then(Json::as_str)
            .unwrap_or("<missing>")
            .to_string();
        if !TRUTH_SCHEMAS.contains(&schema.as_str()) {
            return Err(format!("unrecognised truth schema {schema:?}"));
        }
        // `/3` is `/2` plus the record an E1.3b entry carries, so every `/2` feature is available to it
        // and nothing about a region's shape or measure changed on the way up.
        let v2 = schema != TRUTH_SCHEMAS[0];
        let v3 = schema == TRUTH_SCHEMAS[2];
        // The `/3` fields are refused under an older schema rather than ignored, and read in full under
        // it: see [`refuse_outside_schema`] for why dropping one silently would be the worse failure.
        refuse_outside_schema(value, &schema, v3)?;
        let text = |k: &str| -> String {
            if v3 {
                value
                    .get(k)
                    .and_then(Json::as_str)
                    .unwrap_or_default()
                    .to_string()
            } else {
                String::new()
            }
        };
        let variables = if v3 {
            read_variables(value)?
        } else {
            Vec::new()
        };
        let flag = |k: &str| value.get(k).and_then(Json::as_bool).unwrap_or(false);
        let mut regions = Vec::new();
        if let Some(items) = value.get("regions").and_then(Json::as_array) {
            for (at, item) in items.iter().enumerate() {
                regions.push(read_region(at, item, v2)?);
            }
        }
        let boundaries = read_boundaries(value);
        Ok(Self {
            method: value
                .get("method")
                .and_then(Json::as_str)
                .unwrap_or("analytic")
                .to_string(),
            fault: value
                .get("fault")
                .and_then(Json::as_str)
                .unwrap_or("unknown")
                .to_string(),
            derivation: value
                .get("derivation")
                .and_then(Json::as_str)
                .unwrap_or_default()
                .to_string(),
            regions,
            boundaries,
            control: flag("control"),
            static_expected: flag("static"),
            narrow: flag("narrow"),
            curved: flag("curved"),
            degenerate: flag("degenerate"),
            expects: value
                .get("expect")
                .and_then(Json::as_str)
                .unwrap_or_default()
                .to_string(),
            matched_to: if v2 {
                value
                    .get("matched_to")
                    .and_then(Json::as_str)
                    .map(str::to_string)
            } else {
                None
            },
            choice_claims: if v2 {
                read_choice_claims(value)
            } else {
                Vec::new()
            },
            meaning: text("meaning"),
            variables,
            mechanism: text("mechanism"),
            channels: if v3 {
                value
                    .get("channels")
                    .and_then(Json::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(Json::as_str)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default()
            } else {
                Vec::new()
            },
            resolvable: if v3 {
                value.get("resolvable").and_then(Json::as_bool)
            } else {
                None
            },
            resolvability: text("resolvability"),
            source: text("source"),
            why: text("why"),
            schema,
        })
    }

    /// Is this point inside a declared region?
    #[must_use]
    pub fn contains(&self, model: &Model, x: &[f64]) -> bool {
        self.regions.iter().any(|r| r.contains(model, x))
    }

    /// Distance from the nearest declared region, normalised by the domain width, used so the
    /// outside check can keep away from boundaries it knows are approximate.
    ///
    /// This is a *box* distance and stays one: for a `where` region it under-reports the distance to
    /// the carved set, which is why `corpus::verify` does not use a tolerance on those regions and
    /// tests adjacency on the verification lattice instead.
    #[must_use]
    pub fn margin(&self, model: &Model, x: &[f64]) -> f64 {
        if self.regions.is_empty() {
            return 1.0;
        }
        self.regions
            .iter()
            .map(|r| region_margin(model, r, x))
            .fold(f64::INFINITY, f64::min)
    }

    /// Fraction of the parameter space the region occupies, over the axes it names.
    #[must_use]
    pub fn volume_fraction(&self, model: &Model, region: &Declared) -> f64 {
        region.volume_fraction(model, LATTICE_PER_AXIS)
    }

    /// Total declared untrustworthy fraction, bounded by inclusion-exclusion-free addition because
    /// the corpus keeps its boxes disjoint by construction.
    #[must_use]
    pub fn total_fraction(&self, model: &Model) -> f64 {
        self.regions
            .iter()
            .map(|r| self.volume_fraction(model, r))
            .sum::<f64>()
            .clamp(0.0, 1.0)
    }

    /// Does a cell intersect a declared region?
    #[must_use]
    pub fn overlaps(&self, model: &Model, cell: &[[f64; 2]]) -> bool {
        self.regions.iter().any(|r| r.overlaps(model, cell))
    }

    /// Overlap volume between a cell and the declared regions, as a fraction of the cell. Used by
    /// the false-positive measure, where "suspicious volume that is not really wrong" is the thing
    /// being counted, and by `localised`, which is decided at 0.5 of it.
    #[must_use]
    pub fn overlap_fraction(&self, model: &Model, cell: &[[f64; 2]]) -> f64 {
        self.overlap_at(model, cell, LATTICE_PER_AXIS)
    }

    /// The same measurement on a named lattice, so an outcome can be checked for whether its
    /// localisation decision moved when the lattice doubled.
    #[must_use]
    pub fn overlap_at(&self, model: &Model, cell: &[[f64; 2]], per_axis: usize) -> f64 {
        let mut best = 0.0f64;
        for r in &self.regions {
            best = best.max(r.overlap_fraction(model, cell, per_axis));
        }
        best
    }

    /// Fraction of `cell` the declared regions cover *between them*, counting an overlap once.
    ///
    /// [`Truth::overlap_fraction`] answers a different question — how much of this cell lies in the
    /// single region that best explains it — and that is what `localised` and `false_positive_fraction`
    /// have always been decided on, so their bytes stay where they are. A union is what a *volume
    /// decomposition* needs: a certified leaf that two declared boxes tile side by side is untrustworthy
    /// through and through, and a maximum reports one tile of it. The two are equal on a corpus whose
    /// regions are disjoint, and `ode/euler_decay_2d`, which declares twenty-four of them, is the
    /// committed counter-example the difference is pinned on.
    ///
    /// Exact while every region is a box: the cell is cut at each region bound that falls inside it, and
    /// every resulting elementary box is wholly covered or wholly not, so the share is a sum of widths
    /// rather than an estimate. As soon as one region carries a `where` there is no bound to cut at, and
    /// the share is sampled on the same fixed midpoint lattice [`Declared::accepted_share`] uses, which
    /// makes it an approximation with a declared resolution rather than a fact of geometry.
    ///
    /// `None` is a refusal, not a zero: it says this build will not state a share for this cell because
    /// the decomposition would exceed [`UNION_DECOMPOSITION_LIMIT`] elementary boxes. A caller reads
    /// that as not-measurable and reports it as such.
    #[must_use]
    pub fn union_overlap_at(
        &self,
        model: &Model,
        cell: &[[f64; 2]],
        per_axis: usize,
    ) -> Option<f64> {
        if self.regions.is_empty() || cell.is_empty() {
            return Some(0.0);
        }
        let volume = cell
            .iter()
            .fold(1.0f64, |acc, [lo, hi]| acc * (hi - lo).max(0.0));
        if volume <= 0.0 || !volume.is_finite() {
            // A leaf with no extent overlaps nothing, which is the convention `Declared::clipped`
            // already keeps by dividing a zero intersection by a floored width. A non-finite extent is
            // refused alongside it rather than measured, because a share of an infinite cell is not a
            // number any decomposition could add up.
            return Some(0.0);
        }
        if self.has_predicates() {
            return self.sampled_union_share(model, cell, per_axis);
        }
        self.boxed_union_share(model, cell, volume)
    }

    /// [`Truth::union_overlap_at`] on the lattice this build's measures use.
    #[must_use]
    pub fn union_overlap(&self, model: &Model, cell: &[[f64; 2]]) -> Option<f64> {
        self.union_overlap_at(model, cell, LATTICE_PER_AXIS)
    }

    /// The share of the whole declared domain covered by the union of the regions — the denominator a
    /// regional-overclaim rate is stated against, and the ceiling every per-label overlap sums to.
    #[must_use]
    pub fn union_domain_share(&self, model: &Model) -> Option<f64> {
        self.union_overlap(model, &domain_bounds(model))
    }

    /// Exact union share of a box-only declaration, by cutting `cell` at the bounds that reach it.
    fn boxed_union_share(&self, model: &Model, cell: &[[f64; 2]], volume: f64) -> Option<f64> {
        // Only the regions that actually reach the cell contribute cut points, so a leaf far from every
        // declaration stays one box rather than paying for the corpus's whole region list.
        let reached: Vec<&Declared> = self
            .regions
            .iter()
            .filter(|r| r.overlaps(model, cell))
            .collect();
        if reached.is_empty() {
            return Some(0.0);
        }
        let mut cuts: Vec<Vec<f64>> = Vec::with_capacity(cell.len());
        for (axis, [clo, chi]) in cell.iter().enumerate() {
            let mut points = vec![*clo, *chi];
            for region in &reached {
                if let Some([lo, hi]) = region.axis_span(model, axis) {
                    // Both ends are pulled inside the cell before they cut it: a region that stops
                    // short of this leaf would otherwise contribute an interval *outside* the cell,
                    // whose extent would then be added to a numerator the denominator never counted.
                    points.push(lo.min(*chi).max(*clo));
                    points.push(hi.min(*chi).max(*clo));
                }
            }
            points.sort_by(f64::total_cmp);
            points.dedup();
            cuts.push(points);
        }
        let mut atoms = 1usize;
        for points in &cuts {
            atoms = atoms.checked_mul(points.len().saturating_sub(1))?;
            if atoms > UNION_DECOMPOSITION_LIMIT {
                return None;
            }
        }
        let mut covered = 0.0f64;
        let mut slot = vec![0usize; cuts.len()];
        let mut query: Vec<[f64; 2]> = vec![[0.0, 0.0]; cuts.len()];
        loop {
            let mut extent = 1.0f64;
            let mut whole = true;
            for (axis, points) in cuts.iter().enumerate() {
                let (lo, hi) = (points[slot[axis]], points[slot[axis] + 1]);
                if hi <= lo {
                    whole = false;
                    break;
                }
                query[axis] = [lo, hi];
                extent *= hi - lo;
            }
            if whole && extent > 0.0 && reached.iter().any(|r| r.covers(model, &query)) {
                covered += extent;
            }
            let mut axis = cuts.len();
            loop {
                if axis == 0 {
                    return Some((covered / volume).clamp(0.0, 1.0));
                }
                axis -= 1;
                slot[axis] += 1;
                if slot[axis] < cuts[axis].len() - 1 {
                    break;
                }
                slot[axis] = 0;
            }
        }
    }

    /// Union share on the midpoint lattice of `cell`, which is all a carved set can be asked.
    fn sampled_union_share(
        &self,
        model: &Model,
        cell: &[[f64; 2]],
        per_axis: usize,
    ) -> Option<f64> {
        let arity = cell.len();
        if arity == 0 || per_axis == 0 {
            return None;
        }
        let total = per_axis.checked_pow(arity as u32)?;
        if total > UNION_DECOMPOSITION_LIMIT {
            return None;
        }
        let mut point = vec![0.0f64; arity];
        let mut accepted = 0u64;
        for index in 0..total {
            for (axis, [lo, hi]) in cell.iter().enumerate() {
                let digit = (index / per_axis.pow(axis as u32)) % per_axis;
                point[axis] = *lo + (hi - lo) * (digit as f64 + 0.5) / per_axis as f64;
            }
            if self.contains(model, &point) {
                accepted += 1;
            }
        }
        Some(accepted as f64 / total as f64)
    }

    /// Is some region's predicate unable to answer at this point? The verification grid hits domain
    /// and envelope edges exactly, which a midpoint lattice never does, so a pole sitting on the edge
    /// of a region shows up here rather than in [`Declared::undefined_in`] — and both are needed.
    #[must_use]
    pub fn predicate_undefined_at(&self, model: &Model, x: &[f64]) -> bool {
        self.regions.iter().any(|r| match &r.predicate {
            None => false,
            Some(p) => r.values(model, x).is_none_or(|values| p.undefined(&values)),
        })
    }

    /// Does any region carry a `where` predicate? The lattice-dependent work is only paid for on
    /// entries that need it, which is what keeps the 22 committed box entries measuring exactly as
    /// they did before this form existed.
    #[must_use]
    pub fn has_predicates(&self) -> bool {
        self.regions.iter().any(|r| r.predicate.is_some())
    }

    /// Every parameter name any region's predicate uses, for the audit that refuses a name the model
    /// does not declare.
    #[must_use]
    pub fn predicate_vars(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for r in &self.regions {
            if let Some(p) = &r.predicate {
                for name in p.vars() {
                    if !out.contains(name) {
                        out.push(name.clone());
                    }
                }
            }
        }
        out
    }
}

/// One declared region, read strictly. Every refusal names the region by position, because a truth
/// file that lost a bound or mis-pelled an expression would otherwise be measured as a different —
/// usually larger — region than its author wrote.
fn read_region(at: usize, item: &Json, v2: bool) -> Result<Declared, String> {
    let axes = item
        .get("axes")
        .ok_or_else(|| format!("region {at}: a declared region needs an axis envelope"))?;
    let Json::Obj(fields) = axes else {
        return Err(format!("region {at}: axes is not an object"));
    };
    let mut spans = Vec::new();
    for (name, span) in fields {
        let pair = span
            .as_array()
            .ok_or_else(|| format!("region {at}: axis {name:?} has no [low, high] pair"))?;
        let lo = pair
            .first()
            .and_then(Json::as_f64)
            .ok_or_else(|| format!("region {at}: {name:?} has no low bound"))?;
        let hi = pair
            .get(1)
            .and_then(Json::as_f64)
            .ok_or_else(|| format!("region {at}: {name:?} has no high bound"))?;
        if !lo.is_finite() || !hi.is_finite() {
            return Err(format!(
                "region {at}: {name:?} is bounded by [{lo}, {hi}], which is not finite: an open \
                 edge claims an extent the lattice cannot measure, and `volume_fraction` would \
                 report it as the whole axis"
            ));
        }
        spans.push((name.clone(), [lo, hi]));
    }
    let where_text = item.get("where").and_then(Json::as_str);
    if where_text.is_some() && !v2 {
        return Err(format!(
            "region {at}: a `where` region needs schema {}, which says the box is an envelope rather than the whole claim",
            TRUTH_SCHEMAS[1]
        ));
    }
    let predicate = match where_text {
        None => None,
        Some(text) => {
            if spans.is_empty() {
                return Err(format!(
                    "region {at}: a `where` region needs a non-empty envelope to carve, so its measure stays bounded"
                ));
            }
            Some(Predicate::parse(text).map_err(|e| format!("region {at}: {e}"))?)
        }
    };
    Ok(Declared {
        reason: {
            let text = item
                .get("reason")
                .and_then(Json::as_str)
                .unwrap_or_default()
                .trim()
                .to_string();
            if text.is_empty() {
                return Err(format!(
                    "region {at}: a declared region states no `reason`, so nothing can be asked \
                     about why this box rather than none"
                ));
            }
            text
        },
        axes: spans,
        predicate,
    })
}

fn region_margin(model: &Model, region: &Declared, x: &[f64]) -> f64 {
    region.axes.iter().fold(0.0f64, |acc, (name, [lo, hi])| {
        let Some(i) = axis_of(model, name) else {
            return acc;
        };
        let Some(v) = x.get(i).copied() else {
            return acc;
        };
        let (lo, hi) = (*lo, *hi);
        let outside = if v < lo {
            lo - v
        } else if v > hi {
            v - hi
        } else {
            0.0
        };
        acc + outside / axis_width(model, i).max(1e-30)
    })
}

/// Resolve a parameter name to its index. A truth file that names something the model does not
/// declare is a corpus bug, and treating an unknown name as "no constraint" would hide it, so
/// callers that care check the result.
fn axis_of(model: &Model, name: &str) -> Option<usize> {
    model.param(name).map(|p| p as usize)
}

/// The width of one axis of the declared domain. A `Choices` axis is bounded by the values the model
/// enumerates, so a cell on that axis lives in the same space as a region bound does; counting the
/// choices instead would mix a number of items with a length and make every fraction on a discrete
/// axis meaningless.
#[must_use]
pub fn axis_width(model: &Model, i: usize) -> f64 {
    use aporia_ir::Domain;
    match &model.params[i].domain {
        Domain::Interval { lo, hi } => hi - lo,
        Domain::Choices(v) => {
            let (lo, hi) = (
                v.first().copied().unwrap_or(0.0),
                v.last().copied().unwrap_or(0.0),
            );
            (hi - lo).max(1e-30)
        }
    }
}

/// Every axis of the declared domain, as the interval a leaf is cut out of.
///
/// A `Choices` axis reports the span its enumerated values live on, which is the same span
/// [`axis_width`] divides by and the same span the atlas partitions, so a share computed against these
/// bounds is in the units every other volume column of a row is in.
#[must_use]
pub fn domain_bounds(model: &Model) -> Vec<[f64; 2]> {
    use aporia_ir::Domain;
    model
        .params
        .iter()
        .map(|p| match &p.domain {
            Domain::Interval { lo, hi } => [*lo, *hi],
            Domain::Choices(v) => [
                v.first().copied().unwrap_or(0.0),
                v.last().copied().unwrap_or(0.0),
            ],
        })
        .collect()
}

/// Every grid point, in a deterministic order, over the model's declared domain.
#[must_use]
pub fn grid(model: &Model, per_axis: usize) -> Vec<Vec<f64>> {
    let n = model.params.len();
    if n == 0 {
        return vec![Vec::new()];
    }
    let total = per_axis.pow(n as u32);
    let mut out = Vec::with_capacity(total.min(1 << 20));
    for index in 0..total {
        out.push(index_point(model, index, per_axis));
    }
    out
}

/// The `index`th point of a grid whose **first** axis moves fastest, so a verification run can name
/// the exact point that disagreed with a declaration and a reader can work out where it sits.
#[must_use]
pub fn index_point(model: &Model, mut index: usize, per_axis: usize) -> Vec<f64> {
    use aporia_ir::Domain;
    let mut x = Vec::with_capacity(model.params.len());
    for p in &model.params {
        let digit = index % per_axis;
        index /= per_axis;
        match &p.domain {
            Domain::Interval { lo, hi } => {
                let step = (hi - lo) / (per_axis - 1).max(1) as f64;
                x.push(*lo + step * digit as f64);
            }
            Domain::Choices(v) if v.is_empty() => x.push(0.0),
            Domain::Choices(v) => {
                let at = digit % v.len();
                x.push(v[at]);
            }
        }
    }
    x
}

#[cfg(test)]
mod tests {
    use super::*;
    use aporia_dsl::lower::compile;

    fn model(src: &str) -> Model {
        let c = compile("t.ap", src);
        assert!(!c.diagnostics.has_errors(), "{}\n{src}", c.diagnostics);
        c.model
    }

    const SRC: &str = "model t \"\" {\n input x in [0, 10]\n input y in [0, 1]\n let z = x * y\n require z < 5\n}\n";

    fn truth(regions: &str) -> Truth {
        let text = format!(
            r#"{{"schema":"aporia.truth/1","method":"analytic","fault":"x","regions":{regions},"boundaries":[]}}"#
        );
        Truth::from_json(&Json::parse(&text).unwrap()).unwrap()
    }

    /// The same reader entry point with a region that carves its envelope — the form E1.1 needs.
    fn truth2(regions: &str) -> Truth {
        let text = format!(
            r#"{{"schema":"aporia.truth/2","method":"analytic","fault":"x","regions":{regions},"boundaries":[]}}"#
        );
        Truth::from_json(&Json::parse(&text).unwrap()).unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn a_declared_box_contains_exactly_its_own_points() {
        let m = model(SRC);
        let t = truth(r#"[{"reason":"r","axes":{"x":[6,10]}}]"#);
        assert!(t.contains(&m, &[7.0, 0.9]));
        assert!(!t.contains(&m, &[5.0, 0.9]));
        // An axis the region does not name is unconstrained, so y does not affect containment.
        assert!(t.contains(&m, &[7.0, 0.0]));
    }

    #[test]
    fn volumes_are_measured_against_the_declared_domain() {
        let m = model(SRC);
        let t = truth(r#"[{"reason":"r","axes":{"x":[0,5]}}]"#);
        assert!((t.volume_fraction(&m, &t.regions[0]) - 0.5).abs() < 1e-12);
        let both =
            truth(r#"[{"reason":"a","axes":{"x":[0,2.5]}},{"reason":"b","axes":{"x":[5,7.5]}}]"#);
        assert!((both.total_fraction(&m) - 0.5).abs() < 1e-12);
    }

    #[test]
    fn overlap_distinguishes_a_cell_inside_from_a_cell_straddling() {
        let m = model(SRC);
        let t = truth(r#"[{"reason":"r","axes":{"x":[4,6]}}]"#);
        assert!(t.overlaps(&m, &[[0.0, 10.0], [0.0, 1.0]]));
        assert_eq!(t.overlap_fraction(&m, &[[0.0, 10.0]]), 0.2);
        assert_eq!(t.overlap_fraction(&m, &[[5.0, 10.0]]), 0.2);
        assert_eq!(t.overlap_fraction(&m, &[[0.0, 5.0]]), 0.2);
        assert_eq!(t.overlap_fraction(&m, &[[6.0, 10.0]]), 0.0);
        assert_eq!(t.overlap_fraction(&m, &[[4.0, 6.0]]), 1.0);
    }

    #[test]
    fn margin_grows_with_distance_from_the_nearest_region() {
        let m = model(SRC);
        let t = truth(r#"[{"reason":"r","axes":{"x":[4,6]}}]"#);
        assert_eq!(t.margin(&m, &[5.0, 0.5]), 0.0);
        assert!((t.margin(&m, &[2.0, 0.5]) - 0.2).abs() < 1e-12);
    }

    #[test]
    fn a_grid_hits_the_boundaries_of_the_domain() {
        let m = model(SRC);
        let g = grid(&m, 5);
        assert_eq!(g.len(), 25);
        assert_eq!(g[0], vec![0.0, 0.0]);
        assert_eq!(g[24], vec![10.0, 1.0]);
        // The first axis moves fastest, so index 5 is the second step of the second axis with the
        // first back at its low edge.
        assert_eq!(g[5], vec![0.0, 0.25]);
    }

    #[test]
    fn flags_are_read_and_an_unknown_schema_is_refused() {
        let t = truth(r"[]");
        assert!(!t.control);
        let with_flags = Json::parse(
            r#"{"schema":"aporia.truth/1","method":"static","fault":"unit","control":false,"static":true,"regions":[],"boundaries":[]}"#,
        )
        .unwrap();
        let t = Truth::from_json(&with_flags).unwrap();
        assert!(t.static_expected);
        assert_eq!(t.method, "static");
        let bad = Json::parse(r#"{"schema":"aporia.truth/3"}"#).unwrap();
        assert!(Truth::from_json(&bad).is_err());
    }

    #[test]
    fn a_predicate_region_carves_its_envelope() {
        let m = model(SRC);
        // `x + y > 10` inside the full domain: a diagonal that no box on this model describes.
        let t =
            truth2(r#"[{"reason":"diagonal","axes":{"x":[0,10],"y":[0,1]},"where":"x + y > 10"}]"#);
        assert!(t.has_predicates());
        // Above the line, inside the envelope.
        assert!(t.contains(&m, &[9.5, 0.9]));
        // Below it — a point a box of the same envelope would have claimed.
        assert!(!t.contains(&m, &[1.0, 0.1]));
        // Outside the envelope, the predicate never gets to say yes.
        assert!(!t.contains(&m, &[20.0, 0.9]));
        assert_eq!(t.predicate_vars(), ["x".to_string(), "y".to_string()]);
    }

    #[test]
    fn a_predicate_volume_is_smaller_than_its_envelope_and_stable() {
        let m = model(SRC);
        let t =
            truth2(r#"[{"reason":"diagonal","axes":{"x":[0,10],"y":[0,1]},"where":"x + y > 10"}]"#);
        let region = &t.regions[0];
        let box_only = truth(r#"[{"reason":"diagonal","axes":{"x":[0,10],"y":[0,1]}}]"#);
        let envelope = box_only.volume_fraction(&m, &box_only.regions[0]);
        let carved = t.volume_fraction(&m, region);
        assert!(
            (envelope - 1.0).abs() < 1e-12,
            "the envelope is the whole domain"
        );
        assert!(carved > 0.0 && carved < envelope, "{carved} of {envelope}");
        // Deterministic: the same lattice answers the same on every call, so no arm of a comparison
        // can be helped or hurt by when the measure was taken.
        assert_eq!(carved, t.volume_fraction(&m, region));
    }

    #[test]
    fn a_predicate_overlap_keeps_the_boxes_meaning_of_a_fraction_of_the_cell() {
        let m = model(SRC);
        let t = truth2(r#"[{"reason":"half","axes":{"x":[0,10],"y":[0,1]},"where":"x + y > 10"}]"#);
        // The whole domain: the share inside the diagonal set, not the share of the envelope.
        let all = t.overlap_fraction(&m, &[[0.0, 10.0], [0.0, 1.0]]);
        assert!(all > 0.0 && all < 1.0, "{all}");
        // A cell entirely under the line overlaps nothing, even though its box touches the envelope.
        let under = t.overlap_fraction(&m, &[[0.0, 1.0], [0.0, 0.1]]);
        assert!(under < 0.05, "a cell below the diagonal reported {under}");
        // A cell the line cuts diagonally: `x + y > 10` over x in [9,10], y in [0.5,1] is true for
        // all of x > 9.5 and for a triangle of the rest, which is 3/4 of the cell. Worked out by hand,
        // because the claim worth pinning is that the sampled answer follows the shape and not the box.
        let cut = t.overlap_fraction(&m, &[[9.0, 10.0], [0.5, 1.0]]);
        assert!(
            (cut - 0.75).abs() < 0.03,
            "a cell straddling the diagonal reported {cut}, expected 0.75"
        );
        assert!(
            t.overlaps(&m, &[[0.0, 1.0], [0.0, 0.1]]),
            "the envelope test stays conservative"
        );
    }

    #[test]
    fn a_predicate_that_cannot_say_is_refused_before_it_is_measured() {
        let m = model(SRC);
        // Undefined everywhere: a region that never answers must not be scored as an empty one.
        let nowhere =
            truth2(r#"[{"reason":"nan","axes":{"x":[0,10],"y":[0,1]},"where":"0 / (x - x) > 1"}]"#);
        assert!(nowhere.regions[0].undefined_in(&m, &[[0.0, 10.0], [0.0, 1.0]], 32));
        // A `0/0` sitting exactly on the envelope edge is *not* on the midpoint lattice — the sampling
        // deliberately never lands on an edge — which is why the verification grid asks the same
        // question at the points it does hit. (`1/0` would be infinity, which is a real answer.)
        let edge = truth2(
            r#"[{"reason":"pole","axes":{"x":[0,10],"y":[0,1]},"where":"0 / (y - 1.0) > 1"}]"#,
        );
        assert!(
            !edge.regions[0].undefined_in(&m, &[[0.0, 10.0], [0.0, 1.0]], 32),
            "the measure lattice samples midpoints and must not pretend to have seen the edge"
        );
        assert!(
            edge.predicate_undefined_at(&m, &[0.0, 1.0]),
            "the grid, which does hit the edge, has to report the pole"
        );
        // A region that never divides by zero answers the same question honestly.
        let safe = truth2(
            r#"[{"reason":"pole","axes":{"x":[0,10],"y":[0,1]},"where":"x / (y + 1.0) > 1"}]"#,
        );
        assert!(!safe.regions[0].undefined_in(&m, &[[0.0, 10.0], [0.0, 1.0]], 32));
        assert!(!safe.predicate_undefined_at(&m, &[0.0, 1.0]));
    }

    #[test]
    fn a_where_under_the_box_schema_is_refused_not_read_as_its_envelope() {
        // The silent version of this bug would widen every curved region to its box and flatter all
        // nine metrics that read one, so the refusal has to be the reader's, not the audit's.
        let text = r#"{"schema":"aporia.truth/1","method":"analytic","fault":"x","regions":[{"reason":"r","axes":{"x":[0,10]},"where":"x > 5"}],"boundaries":[]}"#;
        let error = Truth::from_json(&Json::parse(text).unwrap())
            .expect_err("a truth/1 file cannot declare a predicate");
        assert!(error.contains("aporia.truth/2"), "{error}");
    }

    #[test]
    fn a_predicate_region_without_an_envelope_is_refused() {
        // Without a box the measure would have no bounded lattice to sample.
        let text = r#"{"schema":"aporia.truth/2","method":"analytic","fault":"x","regions":[{"reason":"r","axes":{},"where":"x > 5"}],"boundaries":[]}"#;
        let error = Truth::from_json(&Json::parse(text).unwrap())
            .expect_err("an unbounded region is not declarable");
        assert!(error.contains("envelope"), "{error}");
    }

    #[test]
    fn a_malformed_predicate_is_refused_with_the_region_named() {
        let text = r#"{"schema":"aporia.truth/2","method":"analytic","fault":"x","regions":[{"reason":"r","axes":{"x":[0,10]},"where":"sqrt(x) > 5"}],"boundaries":[]}"#;
        let error =
            Truth::from_json(&Json::parse(text).unwrap()).expect_err("no calls in this language");
        assert!(error.contains("region 0"), "{error}");
    }

    #[test]
    fn a_missing_axis_envelope_is_refused_rather_than_defaulted() {
        // The old reader dropped a region whose `axes` was absent, silently shrinking the claim; the
        // count of declared regions is a metric input, so a dropped region is a changed experiment.
        let text = r#"{"schema":"aporia.truth/1","method":"analytic","fault":"x","regions":[{"reason":"r","boundaries":[]}],"boundaries":[]}"#;
        let error =
            Truth::from_json(&Json::parse(text).unwrap()).expect_err("a region needs its axes");
        assert!(error.contains("region 0"), "{error}");
    }

    #[test]
    fn a_region_without_a_reason_is_refused() {
        // 0037's audit gate promised this one. A region whose author states no claim is a box that
        // will be scored as if it were an answer, so the reader refuses rather than defaulting it to
        // an empty string the report would print as nothing.
        let text = r#"{"schema":"aporia.truth/1","method":"analytic","fault":"x","regions":[{"axes":{"x":[0,10]}}],"boundaries":[]}"#;
        let error = Truth::from_json(&Json::parse(text).unwrap())
            .expect_err("a region without a reason declares nothing");
        assert!(error.contains("reason"), "{error}");
    }

    #[test]
    fn a_non_finite_region_bound_is_refused() {
        // An open edge is not a bounded region: the lattice would have no span to divide, and the
        // unbounded domain's own refusal is a static entry's business, not a swept region's.
        for bound in ["\"Infinity\"", "\"-Infinity\"", "\"NaN\"", "1e999"] {
            let text = format!(
                r#"{{"schema":"aporia.truth/1","method":"analytic","fault":"x","regions":[{{"reason":"r","axes":{{"x":[0,{bound}]}}}}],"boundaries":[]}}"#
            );
            let error = Truth::from_json(&Json::parse(&text).unwrap())
                .err()
                .unwrap_or_else(|| panic!("{bound} was read as a region bound"));
            assert!(error.contains("not finite"), "{error}");
        }
    }

    #[test]
    fn a_union_counts_two_regions_that_tile_a_cell_once_each() {
        let m = model(SRC);
        let tiled =
            truth(r#"[{"reason":"a","axes":{"x":[0,5]}},{"reason":"b","axes":{"x":[5,10]}}]"#);
        let domain = &[[0.0, 10.0], [0.0, 1.0]];
        // The whole cell is untrustworthy, because both halves are declared. The per-region maximum
        // that `localised` is decided on reports one tile of it, and keeps doing so: this is the
        // quantity a volume decomposition needs and that history does not use.
        assert_eq!(tiled.union_overlap(&m, domain), Some(1.0));
        assert_eq!(tiled.overlap_fraction(&m, domain), 0.5);
        // A leaf sitting wholly inside one of the tiles is fully untrustworthy too: the share is of the
        // cell, which is what makes the four label columns add up to the region instead of to a mean.
        assert_eq!(
            tiled.union_overlap(&m, &[[0.0, 5.0], [0.0, 1.0]]),
            Some(1.0)
        );
        assert_eq!(
            tiled.union_overlap(&m, &[[2.0, 7.0], [0.0, 1.0]]),
            Some(1.0)
        );
        assert_eq!(
            tiled.union_overlap(&m, &[[6.0, 8.0], [0.0, 1.0]]),
            Some(1.0)
        );
    }

    #[test]
    fn overlapping_regions_are_counted_once_and_a_nested_one_adds_nothing() {
        let m = model(SRC);
        let domain = &[[0.0, 10.0], [0.0, 1.0]];
        let crossing =
            truth(r#"[{"reason":"a","axes":{"x":[2,8]}},{"reason":"b","axes":{"x":[4,10]}}]"#);
        // 2..8 union 4..10 is 2..10: eight tenths of the axis, not twelve tenths clamped to ten and not
        // the six tenths a maximum gives a cell that straddles the middle.
        assert_eq!(crossing.union_overlap(&m, domain), Some(0.8));
        assert_eq!(crossing.overlap_fraction(&m, domain), 0.6);
        let nested =
            truth(r#"[{"reason":"a","axes":{"x":[2,8]}},{"reason":"b","axes":{"x":[3,4]}}]"#);
        assert_eq!(nested.union_overlap(&m, domain), Some(0.6));
        // The order the declarations sit in cannot move the answer, so no entry's score depends on how
        // its truth file was typed.
        let flipped =
            truth(r#"[{"reason":"b","axes":{"x":[4,10]}},{"reason":"a","axes":{"x":[2,8]}}]"#);
        assert_eq!(
            flipped.union_overlap(&m, domain),
            crossing.union_overlap(&m, domain)
        );
    }

    #[test]
    fn a_region_touching_a_cell_at_its_face_covers_none_of_it() {
        let m = model(SRC);
        let left = truth(r#"[{"reason":"a","axes":{"x":[0,5]}}]"#);
        // The envelope test is deliberately conservative — it says these two meet — and the measure
        // that follows it has to say the meeting has no extent.
        assert!(left.overlaps(&m, &[[5.0, 10.0], [0.0, 1.0]]));
        assert_eq!(
            left.union_overlap(&m, &[[5.0, 10.0], [0.0, 1.0]]),
            Some(0.0)
        );
        // A leaf with no extent of its own overlaps nothing, which is the convention the per-region
        // clip already keeps rather than a division that happens to land on zero.
        assert_eq!(left.union_overlap(&m, &[[5.0, 5.0], [0.0, 1.0]]), Some(0.0));
    }

    #[test]
    fn a_union_of_one_box_is_the_box_it_was_always_measured_against() {
        let m = model(SRC);
        let one = truth(r#"[{"reason":"a","axes":{"x":[4,6]}}]"#);
        for cell in [
            vec![[0.0, 10.0], [0.0, 1.0]],
            vec![[4.0, 6.0], [0.0, 1.0]],
            vec![[5.0, 10.0], [0.0, 1.0]],
            vec![[6.0, 10.0], [0.0, 1.0]],
            vec![[3.0, 4.5], [0.0, 1.0]],
        ] {
            assert_eq!(
                one.union_overlap(&m, &cell),
                Some(one.overlap_fraction(&m, &cell)),
                "{cell:?}"
            );
        }
    }

    #[test]
    fn a_region_naming_nothing_the_model_declares_covers_nothing() {
        let m = model(SRC);
        let nowhere = truth(r#"[{"reason":"a","axes":{"z":[0,10]}}]"#);
        // `clipped` treats an unresolved name as no constraint on that axis and refuses a region that
        // resolves nowhere, so the union has to agree: an unconstrained region would otherwise claim
        // the whole domain through a typo.
        assert_eq!(
            nowhere.union_overlap(&m, &[[0.0, 10.0], [0.0, 1.0]]),
            Some(0.0)
        );
    }

    #[test]
    fn a_carved_union_is_sampled_and_answers_the_same_twice() {
        let m = model(SRC);
        let domain = &[[0.0, 10.0], [0.0, 1.0]];
        let diagonal =
            truth2(r#"[{"reason":"d","axes":{"x":[0,10],"y":[0,1]},"where":"x + y > 10"}]"#);
        let boxed = truth2(
            r#"[{"reason":"d","axes":{"x":[0,10],"y":[0,1]},"where":"x + y > 10"},
               {"reason":"b","axes":{"x":[0,2]}}]"#,
        );
        let carved = diagonal
            .union_overlap(&m, domain)
            .expect("a lattice this size is measured");
        assert!(carved > 0.0 && carved < 1.0, "{carved}");
        assert_eq!(
            Some(carved),
            diagonal.union_overlap(&m, domain),
            "not deterministic"
        );
        let with_box = boxed.union_overlap(&m, domain).expect("same lattice");
        // The box x in [0,2] lies under the diagonal everywhere, so the two are disjoint and the union
        // is their sum. The sum is not `carved + 0.2`: on the midpoint lattice a column counts only when
        // its centre is inside the bound, so six of thirty-two columns count, which is 0.1875. Stating
        // the lattice's own arithmetic here is the point — a carved region's union share is an estimate
        // with a declared resolution, and a test that asserted 0.2 would be asserting otherwise.
        let columns = 6.0f64 / LATTICE_PER_AXIS as f64;
        assert!(
            (with_box - (carved + columns)).abs() < 1e-12,
            "{with_box} against {carved} plus {columns}"
        );
        assert!(with_box > carved);
    }

    #[test]
    fn a_decomposition_too_large_to_enumerate_is_refused_not_estimated() {
        // Two regions that each name all ten axes put four interior cuts on every one of them, so the
        // domain decomposes into five intervals to a power of ten — nine and a half million elementary
        // boxes, past the bound this build will spend on one cell's geometry.
        let m = model(
            "model wide \"\" {\n  input a in [0, 1]\n  input b in [0, 1]\n  input c in [0, 1]\n  \
             input d in [0, 1]\n  input e in [0, 1]\n  input f in [0, 1]\n  input g in [0, 1]\n  \
             input h in [0, 1]\n  input i in [0, 1]\n  input j in [0, 1]\n  let z = a + b + c + d + e \
             + f + g + h + i + j\n  require z < 5\n}\n",
        );
        let t = Truth::from_json(
            &Json::parse(&declared_boxes(m.params.len(), [0.1, 0.2, 0.3, 0.4])).unwrap(),
        )
        .unwrap();
        let cell: Vec<[f64; 2]> = (0..m.params.len()).map(|_| [0.0, 1.0]).collect();
        assert_eq!(t.union_overlap(&m, &cell), None, "the bound has to refuse");
        assert_eq!(
            t.union_overlap_at(&m, &cell, LATTICE_PER_AXIS),
            None,
            "and it refuses the same way whatever lattice is named, because a box union never samples"
        );
    }

    #[test]
    fn a_four_dimensional_union_is_exact_rather_than_sampled() {
        // The same declaration on four axes is 625 elementary boxes, and two disjoint unit-cornered
        // boxes of side 0.1 cover exactly 2e-4 of the domain. Stating the number, rather than
        // "nonzero", is what says the decomposition sums extents instead of counting lattice points.
        let m = model(
            "model four \"\" {\n  input a in [0, 1]\n  input b in [0, 1]\n  input c in [0, 1]\n  \
             input d in [0, 1]\n  let z = a + b + c + d\n  require z < 2\n}\n",
        );
        let t = Truth::from_json(
            &Json::parse(&declared_boxes(m.params.len(), [0.1, 0.2, 0.3, 0.4])).unwrap(),
        )
        .unwrap();
        let cell: Vec<[f64; 2]> = (0..m.params.len()).map(|_| [0.0, 1.0]).collect();
        let share = t.union_overlap(&m, &cell).expect("measured");
        assert!((share - 0.0002).abs() < 1e-12, "{share}");
        // A leaf inside neither box is zero and not a refusal, and one inside both boxes' band is
        // exactly the part of them it holds.
        let outside: Vec<[f64; 2]> = (0..m.params.len()).map(|_| [0.5, 0.6]).collect();
        assert_eq!(t.union_overlap(&m, &outside), Some(0.0));
        let across: Vec<[f64; 2]> = (0..m.params.len()).map(|_| [0.15, 0.35]).collect();
        // 0.15..0.2 from the first box and 0.3..0.35 from the second: a side of 0.05 on every axis,
        // over a leaf of side 0.2.
        let share = t.union_overlap(&m, &across).expect("measured");
        let expected = 2.0 * (0.05f64).powi(4) / (0.2f64).powi(4);
        assert!(
            (share - expected).abs() < 1e-15,
            "{share} against {expected}"
        );
    }

    #[test]
    fn a_domain_is_measured_in_the_bounds_the_atlas_partitions() {
        let m = model(SRC);
        assert_eq!(domain_bounds(&m), vec![[0.0, 10.0], [0.0, 1.0]]);
        let discrete = model(
            "model t \"\" {\n  input scheme in {1, 2, 4, 8}\n  let z = scheme\n  require z < 3\n}\n",
        );
        assert_eq!(domain_bounds(&discrete), vec![[1.0, 8.0]]);
        assert!((axis_width(&discrete, 0) - 7.0).abs() < 1e-12);
        let t = truth(r#"[{"reason":"r","axes":{"scheme":[4,8]}}]"#);
        // The same 4/7 the volume fraction gives, read off the union path instead of the envelope.
        let share = t
            .union_overlap(&discrete, &domain_bounds(&discrete))
            .expect("one axis, two cut points");
        assert!((share - 4.0 / 7.0).abs() < 1e-12, "{share}");
    }

    /// Two regions that each name the first `arity` axes (`a`, `b`, …), the first spanning
    /// `bounds[0]..bounds[1]` and the second `bounds[2]..bounds[3]`, as a `/1` truth document.
    fn declared_boxes(arity: usize, bounds: [f64; 4]) -> String {
        let names: Vec<String> = (0..arity)
            .map(|i| ((b'a' + i as u8) as char).to_string())
            .collect();
        let span = |lo: f64, hi: f64| {
            let parts: Vec<String> = names
                .iter()
                .map(|name| format!("\"{name}\":[{lo},{hi}]"))
                .collect();
            format!("{{{}}}", parts.join(","))
        };
        format!(
            r#"{{"schema":"aporia.truth/1","method":"analytic","fault":"x","regions":[{{"reason":"r1","axes":{}}},{{"reason":"r2","axes":{}}}]}}"#,
            span(bounds[0], bounds[1]),
            span(bounds[2], bounds[3])
        )
    }

    #[test]
    fn a_discrete_axis_is_measured_in_the_space_the_atlas_partitions() {
        // The atlas bounds a Choices axis by its declared values and splits that span, so a region
        // bound and a cell bound have to be compared in the same space. Counting choices instead of
        // spanning them made every fraction on a discrete axis a ratio of a length to a number.
        let m = model(
            "model t \"\" {\n  input scheme in {1, 2, 4, 8}\n  input dt in [0, 1]\n  \
                       let z = scheme * dt\n  require z < 1\n}\n",
        );
        assert!(
            (axis_width(&m, 0) - 7.0).abs() < 1e-12,
            "1 through 8 spans 7"
        );
        let t = truth(r#"[{"reason":"r","axes":{"scheme":[4,8]}}]"#);
        // 4..8 over 1..8 is 4/7 of the axis, not 2 of 4 choices.
        assert!(
            (t.volume_fraction(&m, &t.regions[0]) - 4.0 / 7.0).abs() < 1e-12,
            "{}",
            t.volume_fraction(&m, &t.regions[0])
        );
    }
}
