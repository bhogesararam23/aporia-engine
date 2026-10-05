//! Reading the corpus, and checking that its ground truth is true.
//!
//! An entry is a directory holding `model.ap` and `truth.json`. Loading it compiles the model, so a
//! corpus file that the DSL rejects is caught here rather than inside a metric that would silently
//! skip it. [`verify`] goes further and tests each declared region against direct evaluation of the
//! model's own rules on a dense grid, which is what makes the numbers in `results/` measurements
//! against something instead of measurements of an assumption.

use crate::truth::{Truth, grid, index_point};
use aporia_dsl::lower::compile;
use aporia_ir::Model;
use aporia_properties::{constraints, divergence};
use aporia_runtime::{ExecConfig, Observation, interp};
use std::path::{Path, PathBuf};

/// One corpus entry.
#[derive(Clone, Debug)]
pub struct Entry {
    pub family: String,
    pub name: String,
    pub dir: PathBuf,
    pub source: String,
    /// `None` when the model does not compile. A `static` entry is *expected* to fail here, and that
    /// is the result the corpus is measuring.
    pub model: Option<Model>,
    /// Every diagnostic the frontend produced, at any severity, rendered with its line.
    pub diagnostics: Vec<String>,
    pub truth: Truth,
}

impl Entry {
    #[must_use]
    pub fn id(&self) -> String {
        format!("{}/{}", self.family, self.name)
    }
}

/// A disagreement between a declared region and what the model actually does.
#[derive(Clone, Debug, PartialEq)]
pub struct Problem {
    pub entry: String,
    pub detail: String,
}

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.entry, self.detail)
    }
}

/// Load every entry the registry names.
///
/// The registry, not the filesystem, decides what is a corpus entry. Walking directories instead
/// meant an experiment archive written under the corpus root was parsed as a model, and it would
/// also have accepted a stray directory a person left there. A file the registry names and the
/// filesystem lacks is an error, because a silently skipped benchmark is a number that went
/// missing from a comparison without anyone noticing.
pub fn load(root: &Path) -> Result<Vec<Entry>, String> {
    let registry_path = root.join("registry.json");
    let registry_text = std::fs::read_to_string(&registry_path)
        .map_err(|e| format!("{}: {e}", registry_path.display()))?;
    let registry = aporia_store::Json::parse(&registry_text)
        .map_err(|e| format!("{}: {e}", registry_path.display()))?;
    let families = registry
        .get("families")
        .and_then(aporia_store::Json::as_array)
        .ok_or_else(|| "registry.json has no families array".to_string())?;
    let mut out = Vec::new();
    for family in families {
        let name = family
            .get("name")
            .and_then(aporia_store::Json::as_str)
            .ok_or_else(|| "a family has no name".to_string())?;
        let entries = family
            .get("entries")
            .and_then(aporia_store::Json::as_array)
            .ok_or_else(|| format!("family {name} has no entries array"))?;
        for entry in entries {
            let entry_name = entry
                .as_str()
                .ok_or_else(|| format!("family {name} has a non-string entry"))?;
            let dir = root.join(name).join(entry_name);
            out.push(load_entry(name, entry_name, &dir)?);
        }
    }
    if out.is_empty() {
        return Err(format!(
            "registry.json under {} names no entries",
            root.display()
        ));
    }
    Ok(out)
}

fn load_entry(family: &str, name: &str, dir: &Path) -> Result<Entry, String> {
    let model_path = dir.join("model.ap");
    let truth_path = dir.join("truth.json");
    if !model_path.exists() || !truth_path.exists() {
        return Err(format!(
            "{} needs both model.ap and truth.json",
            dir.display()
        ));
    }
    let source = std::fs::read_to_string(&model_path)
        .map_err(|e| format!("{}: {e}", model_path.display()))?;
    let truth_text = std::fs::read_to_string(&truth_path)
        .map_err(|e| format!("{}: {e}", truth_path.display()))?;
    let truth_value = aporia_store::Json::parse(&truth_text)
        .map_err(|e| format!("{}: {e}", truth_path.display()))?;
    let truth = Truth::from_json(&truth_value).map_err(|e| format!("{name}: {e}"))?;
    let compiled = compile(&model_path.display().to_string(), &source);
    let located = aporia_dsl::span::Source::new(model_path.display().to_string(), source.clone());
    let reported = compiled
        .diagnostics
        .items
        .iter()
        .map(|d| d.render(&located).lines().next().unwrap_or("").to_string())
        .collect::<Vec<_>>();
    let errors = compiled
        .diagnostics
        .items
        .iter()
        .filter(|d| d.severity == aporia_dsl::span::Severity::Error)
        .count();
    let model = if errors == 0 {
        Some(compiled.model)
    } else {
        None
    };
    Ok(Entry {
        family: family.to_string(),
        name: name.to_string(),
        dir: dir.to_path_buf(),
        source,
        model,
        diagnostics: reported,
        truth,
    })
}

/// The direct answer to "is this point a failure", from the model's own declared rules.
///
/// Deliberately not the search's pipeline: no calibration, no fusion, no atlas. If ground truth and
/// detection shared those, a detection rate would measure the pipeline agreeing with itself.
#[must_use]
pub fn violates(model: &Model, x: &[f64]) -> bool {
    let outcome = interp::run(model, x, ExecConfig::default());
    let o = Observation::new(0, x.to_vec(), &outcome);
    !constraints(model, &o).is_empty() || !divergence(model, &o).is_empty()
}

/// Check every entry's declared boxes against direct evaluation.
///
/// Two directions are checked, with different strength because the corpus contains both exact
/// regions and inner approximations of curved ones:
///
/// - **inside**: every grid point inside a declared region must violate. A counterexample here means
///   the declaration is wrong, full stop, so this is checked exhaustively.
/// - **outside**: a point outside every region *by a margin* must not violate. Points close to a
///   boundary of an approximated region may legitimately violate, which is why the margin exists —
///   and why a `curved` entry is held to a looser version of this than an exact one.
#[must_use]
pub fn verify(entries: &[Entry], per_axis: usize) -> Vec<Problem> {
    entries
        .iter()
        .flat_map(|e| verify_entry(e, per_axis))
        .collect()
}

/// One entry's declarations against one grid of the model's own behaviour.
fn verify_entry(e: &Entry, per_axis: usize) -> Vec<Problem> {
    let mut out = Vec::new();
    if e.truth.static_expected {
        // A static entry is *satisfied* by a diagnostic, at warning severity or above: a scale
        // mismatch is reported as a warning that names the factor, which is the honest strength
        // of the unit checker, and an entry that demanded an error would be claiming more than
        // the checker gives.
        if e.diagnostics.is_empty() {
            out.push(Problem {
                entry: e.id(),
                detail: "declared as a static fault, yet the frontend said nothing about it"
                    .to_string(),
            });
        }
        return out;
    }
    let Some(model) = &e.model else {
        out.push(Problem {
            entry: e.id(),
            detail: format!(
                "does not compile: {}",
                e.diagnostics.first().cloned().unwrap_or_default()
            ),
        });
        return out;
    };
    if e.truth.control && !e.truth.regions.is_empty() {
        out.push(Problem {
            entry: e.id(),
            detail: "a control declares regions, so it is not a control".to_string(),
        });
    }
    if e.truth.regions.is_empty() && !e.truth.control {
        out.push(Problem {
            entry: e.id(),
            detail: "no declared region and not marked as a control".to_string(),
        });
        return out;
    }
    out.extend(undeclared_axes(e, model));
    let mut inside_total = 0u64;
    let mut inside_ok = 0u64;
    let mut outside_total = 0u64;
    let mut outside_ok = 0u64;
    // A degenerate box has zero width, so a grid can never land inside it: those regions
    // contribute their exact point to the same tally.
    let (degenerate_total, degenerate_ok, degenerate_bad) = check_degenerate_boxes(model, e);
    inside_total += degenerate_total;
    inside_ok += degenerate_ok;
    let mut first_bad = degenerate_bad;
    for x in grid(model, per_axis) {
        let inside = e.truth.contains(model, &x);
        if inside {
            inside_total += 1;
            if violates(model, &x) {
                inside_ok += 1;
            } else if first_bad.is_none() {
                first_bad = Some(format!(
                    "declared region contains {x:?}, which does not violate"
                ));
            }
        } else if e.truth.margin(model, &x) > 0.02 {
            outside_total += 1;
            if !violates(model, &x) {
                outside_ok += 1;
            } else if first_bad.is_none() {
                first_bad = Some(format!(
                    "{x:?} is outside every declared region by a margin and still violates"
                ));
            }
        }
    }
    if let Some(detail) = first_bad {
        out.push(Problem {
            entry: e.id(),
            detail: format!("{detail} (inside {inside_ok}/{inside_total} clean)"),
        });
    }
    if inside_total > 0 && inside_ok * 100 < inside_total * 99 {
        out.push(Problem {
            entry: e.id(),
            detail: format!(
                "only {inside_ok} of {inside_total} declared-region grid points violate, so the \
                     boxes are not the region the entry claims"
            ),
        });
    }
    // The outside check is reported per entry but tolerated for a curved region, whose declared
    // boxes are an inner approximation on purpose.
    if outside_total > 0 && outside_ok * 100 < outside_total * 90 {
        out.push(Problem {
            entry: e.id(),
            detail: format!(
                "{}/{} clean points outside the declared regions still violate{}",
                outside_total - outside_ok,
                outside_total,
                if e.truth.curved {
                    ", and this entry is marked curved so the approximation is too coarse"
                } else {
                    ""
                }
            ),
        });
    }
    out
}

/// A degenerate box has zero width, so a grid can never land inside it: those regions are checked by
/// evaluating the exact boundary value the box names instead.
#[must_use]
/// Every axis a declaration names that the model does not have.
///
/// A boundary or region on an absent parameter cannot be checked against anything. It used to reach
/// the metrics anyway, which reported it as a band that missed -- an absence scored as a failure, in
/// the same column as real measurements. Refusing it here means a typo in a `truth.json` stops a
/// measurement instead of quietly becoming part of its published numbers.
fn undeclared_axes(e: &Entry, model: &Model) -> Vec<Problem> {
    let named = e
        .truth
        .boundaries
        .iter()
        .map(|b| {
            (
                format!("boundary on axis {:?}", b.axis),
                vec![b.axis.clone()],
            )
        })
        .chain(e.truth.regions.iter().enumerate().map(|(i, r)| {
            (
                format!("region {i} ({})", r.reason),
                r.axes.iter().map(|(n, _)| n.clone()).collect(),
            )
        }));
    let mut out = Vec::new();
    for (what, axes) in named {
        for axis in axes {
            if model.param(&axis).is_none() {
                out.push(Problem {
                    entry: e.id(),
                    detail: format!("{what}: the model has no parameter named {axis:?}"),
                });
            }
        }
    }
    out
}

fn check_degenerate_boxes(model: &Model, e: &Entry) -> (u64, u64, Option<String>) {
    let mut total = 0;
    let mut ok = 0;
    let mut bad = None;
    for region in &e.truth.regions {
        if !region.axes.iter().any(|(_, [lo, hi])| lo == hi) {
            continue;
        }
        let x = exact_point(model, region);
        total += 1;
        if violates(model, &x) {
            ok += 1;
        } else if bad.is_none() {
            bad = Some(format!("the degenerate point {x:?} does not violate"));
        }
    }
    (total, ok, bad)
}

/// A point that sits exactly on a degenerate (zero-width) box.
fn exact_point(model: &Model, region: &crate::truth::Declared) -> Vec<f64> {
    let mut x = index_point(model, 0, 2);
    for (i, p) in model.params.iter().enumerate() {
        if let Some((_, [lo, _])) = region.axes.iter().find(|(n, _)| n == &p.name) {
            x[i] = *lo;
        }
    }
    x
}

/// Where a declared rule actually switches on one axis, measured rather than derived.
///
/// The other axes are held at their domain centres, the axis is scanned, and every sign change of
/// [`violates`] is bisected to within `tolerance` of the axis width. This is how a corpus entry gets
/// a boundary value it can defend: the number comes out of the model, and the `verify` command then
/// checks the declared boxes against the same evaluation.
#[must_use]
pub fn scan_axis(model: &Model, axis: usize, samples: usize, tolerance: f64) -> Vec<f64> {
    use aporia_ir::Domain;
    let (lo, hi) = match &model.params[axis].domain {
        Domain::Interval { lo, hi } => (*lo, *hi),
        Domain::Choices(v) => (
            v.first().copied().unwrap_or(0.0),
            v.last().copied().unwrap_or(1.0),
        ),
    };
    let width = (hi - lo).max(1e-30);
    let mut base = Vec::with_capacity(model.params.len());
    for (i, p) in model.params.iter().enumerate() {
        base.push(if i == axis {
            lo
        } else {
            match &p.domain {
                Domain::Interval { lo, hi } => lo + (hi - lo) / 2.0,
                Domain::Choices(v) => v.first().copied().unwrap_or(0.0),
            }
        });
    }
    let at = |t: f64| {
        let mut x = base.clone();
        x[axis] = t;
        x
    };
    let mut crossings = Vec::new();
    let mut previous_state = violates(model, &at(lo));
    for k in 1..=samples {
        let t = lo + width * k as f64 / samples as f64;
        let state = violates(model, &at(t));
        if state != previous_state {
            let mut a = lo + width * (k - 1) as f64 / samples as f64;
            let mut b = t;
            while b - a > tolerance * width {
                let m = a + (b - a) / 2.0;
                if violates(model, &at(m)) == previous_state {
                    a = m;
                } else {
                    b = m;
                }
            }
            crossings.push(a + (b - a) / 2.0);
        }
        previous_state = state;
    }
    crossings
}

/// Which parameters are held at their centre while one axis is scanned, for the report line.
#[must_use]
pub fn centre_line(model: &Model) -> Vec<f64> {
    use aporia_ir::Domain;
    model
        .params
        .iter()
        .map(|p| match &p.domain {
            Domain::Interval { lo, hi } => lo + (hi - lo) / 2.0,
            Domain::Choices(v) => v.first().copied().unwrap_or(0.0),
        })
        .collect()
}
