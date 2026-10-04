//! Ground truth: what the corpus says is really wrong, and whether that claim holds up.
//!
//! A truth declaration is only useful if it can be *checked*. Every region here is a union of
//! axis-aligned boxes in the model's own parameter names, which means a dense grid evaluation can
//! answer the one question that matters before any search result is trusted: does the model actually
//! fail inside the boxes it claims to fail inside. [`Truth::verify`] answers that, and
//! `aporia-bench verify` runs it over the whole corpus.
//!
//! The evaluation used for verification is the model's own declared rules, applied directly — not
//! the calibrated, fused, atlas-scored pipeline that the search uses. That distinction is the whole
//! point of a control group: if the same machinery produced both the answer and the marking, a
//! detection rate would be measuring agreement with itself.

use aporia_ir::Model;
use aporia_store::Json;

/// One axis interval of a declared region, in parameter-name space.
#[derive(Clone, Debug, PartialEq)]
pub struct Declared {
    pub reason: String,
    /// `(parameter name, [low, high])`, inclusive. A name that is not listed is unconstrained.
    pub axes: Vec<(String, [f64; 2])>,
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
}

impl Truth {
    /// Read a `truth.json`. Unknown schema is refused; unknown flag fields are ignored, so a newer
    /// corpus file can still be measured by an older harness.
    pub fn from_json(value: &Json) -> Result<Self, String> {
        if value.get("schema").and_then(Json::as_str) != Some("aporia.truth/1") {
            return Err(format!(
                "unrecognised truth schema {:?}",
                value
                    .get("schema")
                    .and_then(Json::as_str)
                    .unwrap_or("<missing>")
            ));
        }
        let flag = |k: &str| value.get(k).and_then(Json::as_bool).unwrap_or(false);
        let regions = value
            .get("regions")
            .and_then(Json::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        let axes = item.get("axes")?;
                        let Json::Obj(fields) = axes else {
                            return None;
                        };
                        let mut out = Vec::new();
                        for (name, span) in fields {
                            let pair = span.as_array()?;
                            let lo = pair.first()?.as_f64()?;
                            let hi = pair.get(1)?.as_f64()?;
                            out.push((name.clone(), [lo, hi]));
                        }
                        Some(Declared {
                            reason: item
                                .get("reason")
                                .and_then(Json::as_str)
                                .unwrap_or_default()
                                .to_string(),
                            axes: out,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
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
        })
    }

    /// Is this point inside a declared region?
    #[must_use]
    pub fn contains(&self, model: &Model, x: &[f64]) -> bool {
        self.regions.iter().any(|r| region_holds(model, r, x))
    }

    /// Distance from the nearest declared region, normalised by the domain width, used so the
    /// outside check can keep away from boundaries it knows are approximate.
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
        region.axes.iter().fold(1.0, |acc, (name, [lo, hi])| {
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
        self.regions.iter().any(|r| {
            r.axes.iter().all(|(name, [lo, hi])| {
                let Some(i) = axis_of(model, name) else {
                    return true;
                };
                let Some([clo, chi]) = cell.get(i) else {
                    return false;
                };
                chi >= lo && clo <= hi
            })
        })
    }

    /// Overlap volume between a cell and the declared regions, as a fraction of the cell. Used by
    /// the false-positive measure, where "suspicious volume that is not really wrong" is the thing
    /// being counted.
    #[must_use]
    pub fn overlap_fraction(&self, model: &Model, cell: &[[f64; 2]]) -> f64 {
        let mut best = 0.0f64;
        for r in &self.regions {
            let mut fraction = 1.0;
            let mut constrained = false;
            for (name, [lo, hi]) in &r.axes {
                let Some(i) = axis_of(model, name) else {
                    continue;
                };
                let Some([clo, chi]) = cell.get(i) else {
                    fraction = 0.0;
                    break;
                };
                constrained = true;
                let width = (chi - clo).max(1e-30);
                let isect = (hi.min(*chi) - lo.max(*clo)).max(0.0);
                fraction *= isect / width;
            }
            if !constrained {
                // An unconstrained region would be "everywhere", which the corpus never declares.
                continue;
            }
            best = best.max(fraction);
        }
        best
    }
}

fn region_holds(model: &Model, region: &Declared, x: &[f64]) -> bool {
    region.axes.iter().all(|(name, [lo, hi])| {
        let Some(i) = axis_of(model, name) else {
            return false;
        };
        let Some(v) = x.get(i).copied() else {
            return false;
        };
        v >= *lo && v <= *hi
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

fn axis_width(model: &Model, i: usize) -> f64 {
    use aporia_ir::Domain;
    match &model.params[i].domain {
        Domain::Interval { lo, hi } => hi - lo,
        Domain::Choices(v) => v.len().max(1) as f64,
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
        let bad = Json::parse(r#"{"schema":"aporia.truth/2"}"#).unwrap();
        assert!(Truth::from_json(&bad).is_err());
    }
}
