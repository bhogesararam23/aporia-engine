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

/// The truth schemas a reader of this build understands.
pub const TRUTH_SCHEMAS: [&str; 2] = ["aporia.truth/1", "aporia.truth/2"];

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
        let mut box_fraction = 1.0;
        let mut intersection: Vec<[f64; 2]> = Vec::with_capacity(cell.len());
        let mut constrained = false;
        for (name, [lo, hi]) in &self.axes {
            let Some(i) = axis_of(model, name) else {
                continue;
            };
            let Some([clo, chi]) = cell.get(i) else {
                return 0.0;
            };
            constrained = true;
            let width = (chi - clo).max(1e-30);
            let isect = (hi.min(*chi) - lo.max(*clo)).max(0.0);
            box_fraction *= isect / width;
            intersection.push([lo.max(*clo), hi.min(*chi)]);
        }
        if !constrained {
            // An unconstrained region would be "everywhere", which the corpus never declares.
            return 0.0;
        }
        if self.predicate.is_none() {
            return box_fraction;
        }
        box_fraction * self.accepted_share(model, &intersection, per_axis)
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
}

impl Truth {
    /// Read a `truth.json`. An unknown schema is refused; unknown flag fields are ignored, so a newer
    /// corpus file can still be measured by an older harness — but a file that uses a `where` cannot,
    /// because an older reader would drop the predicate and measure its envelope instead.
    pub fn from_json(value: &Json) -> Result<Self, String> {
        let schema = value
            .get("schema")
            .and_then(Json::as_str)
            .unwrap_or("<missing>")
            .to_string();
        if !TRUTH_SCHEMAS.contains(&schema.as_str()) {
            return Err(format!("unrecognised truth schema {schema:?}"));
        }
        let v2 = schema == TRUTH_SCHEMAS[1];
        let flag = |k: &str| value.get(k).and_then(Json::as_bool).unwrap_or(false);
        let mut regions = Vec::new();
        if let Some(items) = value.get("regions").and_then(Json::as_array) {
            for (at, item) in items.iter().enumerate() {
                regions.push(read_region(at, item, v2)?);
            }
        }
        let boundaries = value
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
            .unwrap_or_default();
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
            } else {
                Vec::new()
            },
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
