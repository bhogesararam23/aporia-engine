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
    /// How many of those were errors. The audit needs the distinction: the unit checker refuses a
    /// scale mismatch at *warning* severity and the model still compiles, while an unbounded domain
    /// is an error the IR will not lower. Both are "a diagnostic", and E1.3 measures the second one.
    pub error_count: usize,
    pub truth: Truth,
    /// The first 16 hex digits of the SHA-256 of this entry's `truth.json` **bytes**, taken as they
    /// were read. It travels into the measurement identity so that editing a declared region
    /// produces a different measurement rather than a second file with the first file's name.
    ///
    /// It is the bytes rather than a canonical re-serialisation of the parsed claim, deliberately:
    /// a canonical form would have to enumerate every field it knows, and a field it forgets is an
    /// edit that does not change the identity — a silent hole in exactly the thing this is for. The
    /// cost is that reformatting a `truth.json` without changing its meaning renames the
    /// measurement, which is the honest direction to fail.
    pub truth_digest: String,
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
        error_count: errors,
        truth,
        truth_digest: aporia_store::digest::sha256_hex(truth_text.as_bytes())[..16].to_string(),
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

/// Why this entry cannot be measured at all, in one sentence, or `None` if it can.
///
/// Both reasons are about the *input*, not about the model's behaviour: an entry that does not compile
/// has no model to sample, and an entry whose values come from a program has no program in this
/// command. Checking them before the grid is what stops a missing input being reported as a finding — a
/// declared-external model sampled by the interpreter diverges everywhere, faithfully, and the atlas
/// would be a picture of the harness's own absent `--program`. Refusing at this boundary is the same
/// decision `aporia run` makes at its own; `aporia_search::run` does not check, because a driver is not
/// an input boundary and has no way to decline a budget it was handed.
///
/// This is the one predicate both for skipping an entry in a sweep and for recording in the results
/// document that it was skipped. E13's pre-registered metric is *that a refusal is reported rather
/// than that a label appears*, and a skip that left no trace in the file could not tell a reader
/// which entries the experiment was asked to measure and refused.
pub fn unmeasurable(e: &Entry) -> Option<String> {
    // Checked first, and not as part of the `model.is_none()` branch below: the unit checker reports
    // a scale mismatch at *warning* severity, so a declared-static entry still yields a model. It is
    // a hard refusal for `verify`, which wants the diagnostic, and a refusal for a sweep, which has
    // no search to run against an entry that should never be executed.
    if e.truth.static_expected {
        return Some(format!(
            "declared static, and the frontend rejects it as expected: {}",
            e.diagnostics.first().cloned().unwrap_or_default()
        ));
    }
    let model = e.model.as_ref();
    let Some(model) = model else {
        return Some(format!(
            "does not compile: {}",
            e.diagnostics.first().cloned().unwrap_or_default()
        ));
    };
    if aporia_runtime::needs_adapter(model) {
        return Some(
            "declares a value computed by a program; the benchmark harness has no --program to give \
             it, so this entry cannot be measured here"
                .to_string(),
        );
    }
    None
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
    if let Some(detail) = unmeasurable(e) {
        out.push(Problem {
            entry: e.id(),
            detail,
        });
        return out;
    }
    // `unmeasurable` refused the case where there is no model at all, so the rest of this function has
    // one to sample. Reaching `unwrap` would mean those two checks disagree about the same field.
    let model = e
        .model
        .as_ref()
        .unwrap_or_else(|| panic!("{} was measured without a model", e.id()));
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
    out.extend(regions_inside_the_domain(e, model));
    if let Some(problem) = predicate_answerable(e, model, per_axis) {
        out.push(problem);
        return out;
    }
    // Two directions of the same question — is every declared point really failing, and is every
    // failing point really declared — answered from one walk of the grid, so the two cannot
    // disagree about which points they looked at.
    let tally = region_tally(e, model, per_axis);
    let Tally {
        inside_total,
        inside_ok,
        outside_total,
        outside_ok,
        boundary_layer,
        first_bad,
    } = tally;
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
    // The outside check is reported per entry but tolerated for a curved entry, whose declared boxes
    // are an inner approximation on purpose. The tolerance is the same 90% either way — the `curved`
    // flag has never changed it, only the wording of this refusal, and the comment that claimed it
    // did is corrected here rather than worked around.
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
    if boundary_layer > 0 {
        println!(
            "  {}:{} boundary-layer points violate next to the declared region (within one grid \
             step): {}",
            e.id(),
            if e.truth.curved { "curved" } else { "region" },
            boundary_layer
        );
    }
    out
}

/// Every point of the verification lattice, with whether it is inside a declared region and whether
/// the model's own rules fail there. Sampled once and read twice — by the region tally and by the
/// adjacency rule — because the two directions have to disagree about the same points or the check
/// is measuring its own sampling.
#[must_use]
fn grid_states(e: &Entry, model: &Model, per_axis: usize) -> Vec<(Vec<f64>, bool, bool)> {
    let total = per_axis.pow(model.params.len() as u32);
    let mut out = Vec::with_capacity(total.min(1 << 20));
    for index in 0..total {
        let x = index_point(model, index, per_axis);
        let inside = e.truth.contains(model, &x);
        let violated = violates(model, &x);
        out.push((x, inside, violated));
    }
    out
}

/// What the verification lattice and one entry's declarations said about each other.
#[derive(Default)]
struct Tally {
    inside_total: u64,
    inside_ok: u64,
    outside_total: u64,
    outside_ok: u64,
    /// Points that violate next to a `where` region — within one grid step — which is the boundary
    /// layer where the model's arithmetic and the authored expression round differently.
    boundary_layer: u64,
    first_bad: Option<String>,
}

/// Walk the grid once and classify every point against the declaration, in both directions.
///
/// A box-only entry uses the margin rule 0029 has always used. A `where` entry cannot: `margin`
/// measures distance from the envelope *box*, so a point the predicate carved away has margin zero
/// and would never be counted as outside no matter what tolerance was applied. Its rule is adjacency
/// instead — a violating point must sit next to the region on this very lattice or the expression
/// does not describe the model.
fn region_tally(e: &Entry, model: &Model, per_axis: usize) -> Tally {
    let mut out = Tally::default();
    // A degenerate box has zero width, so a grid can never land inside it: those regions contribute
    // their exact point to the same tally.
    let (degenerate_total, degenerate_ok, degenerate_bad) = check_degenerate_boxes(model, e);
    out.inside_total = degenerate_total;
    out.inside_ok = degenerate_ok;
    out.first_bad = degenerate_bad;
    let axes = model.params.len();
    let states = grid_states(e, model, per_axis);
    let inside_index: Vec<bool> = states.iter().map(|(_, inside, _)| *inside).collect();
    let carved = e.truth.has_predicates();
    for (index, (x, inside, violated)) in states.iter().enumerate() {
        if *inside {
            out.inside_total += 1;
            if *violated {
                out.inside_ok += 1;
            } else if out.first_bad.is_none() {
                out.first_bad = Some(format!(
                    "declared region contains {x:?}, which does not violate"
                ));
            }
            continue;
        }
        if !*violated {
            if !carved && e.truth.margin(model, x) > 0.02 {
                out.outside_total += 1;
                out.outside_ok += 1;
            }
            continue;
        }
        if carved {
            if neighbour_inside(index, &inside_index, per_axis, axes) {
                out.boundary_layer += 1;
            } else if out.first_bad.is_none() {
                out.first_bad = Some(format!(
                    "{x:?} violates and is not in the declared region, but is further than one grid \
                     step from it — the `where` expression does not describe the model's failure set"
                ));
            }
        } else if e.truth.margin(model, x) > 0.02 {
            out.outside_total += 1;
            if out.first_bad.is_none() {
                out.first_bad = Some(format!(
                    "{x:?} is outside every declared region by a margin and still violates"
                ));
            }
        }
    }
    out
}

/// Does any grid point leave a `where` expression undefined? A NaN answers every comparison false,
/// so an undeclarable region would look like an empty one — and an empty-looking region is the one
/// thing a verification run must never report as a pass.
fn predicate_answerable(e: &Entry, model: &Model, per_axis: usize) -> Option<Problem> {
    if !e.truth.has_predicates() {
        return None;
    }
    for x in grid(model, per_axis) {
        if e.truth.predicate_undefined_at(model, &x) {
            return Some(Problem {
                entry: e.id(),
                detail: format!(
                    "{x:?} leaves a region's `where` expression undefined, so the region does not \
                     say what it claims"
                ),
            });
        }
    }
    None
}

/// Is any lattice point adjacent to `index` inside a declared region? Neighbourhood is one step in
/// each axis, which is the grid's own resolution — the audit's unit of "close to the boundary".
#[must_use]
fn neighbour_inside(index: usize, inside: &[bool], per_axis: usize, axes: usize) -> bool {
    let mut digits = Vec::with_capacity(axes);
    let mut rest = index;
    for _ in 0..axes {
        digits.push(rest % per_axis);
        rest /= per_axis;
    }
    // Each axis offers its own digit and its in-range neighbours, walked as a product with a digit
    // counter. Nothing goes negative and nothing is cast, so a lattice edge cannot wrap into a
    // neighbour that does not exist.
    let ranges: Vec<Vec<usize>> = digits
        .iter()
        .map(|d| {
            let mut r = Vec::with_capacity(3);
            if *d > 0 {
                r.push(*d - 1);
            }
            r.push(*d);
            if *d + 1 < per_axis {
                r.push(*d + 1);
            }
            r
        })
        .collect();
    let mut chosen = vec![0usize; axes];
    loop {
        let mut next = 0usize;
        let mut place = 1usize;
        for (slot, digit) in chosen.iter().enumerate() {
            next += ranges[slot][*digit] * place;
            place *= per_axis;
        }
        if next != index && inside.get(next).copied().unwrap_or(false) {
            return true;
        }
        let mut slot = 0;
        loop {
            if slot == axes {
                return false;
            }
            chosen[slot] += 1;
            if chosen[slot] < ranges[slot].len() {
                break;
            }
            chosen[slot] = 0;
            slot += 1;
        }
    }
}

/// Every declared region bound that leaves the model's declared domain, or that names a value a
/// `Choices` axis does not contain.
///
/// `Truth::volume_fraction` clamps a bound to the axis width, so a box declared as `[8, 1e9]` on a
/// domain of `[0, 10]` verified green while claiming the whole axis: the overhang was invisible to
/// the grid because the grid never leaves the domain. Refusing it is what makes E1.3's edge entries
/// say where their truth stops.
#[must_use]
fn regions_inside_the_domain(e: &Entry, model: &Model) -> Vec<Problem> {
    use aporia_ir::Domain;
    let mut out = Vec::new();
    for (at, region) in e.truth.regions.iter().enumerate() {
        for (name, [lo, hi]) in &region.axes {
            let Some(index) = model.param(name).map(|p| p as usize) else {
                continue;
            };
            let detail = match &model.params[index].domain {
                Domain::Interval { lo: dlo, hi: dhi } => {
                    if *lo < *dlo || *hi > *dhi {
                        Some(format!(
                            "region {at}: axis {name:?} is declared over [{lo}, {hi}] but the model's domain is [{dlo}, {dhi}]"
                        ))
                    } else {
                        None
                    }
                }
                Domain::Choices(values) => {
                    let known = |v: &f64| values.iter().any(|c| c == v);
                    if !known(lo)
                        && !known(hi)
                        && (*lo < *values.first().unwrap_or(lo)
                            || *hi > *values.last().unwrap_or(hi))
                    {
                        Some(format!(
                            "region {at}: axis {name:?} is discrete, so its bounds have to be values the model enumerates; [{lo}, {hi}] names neither of {}",
                            values
                                .iter()
                                .map(ToString::to_string)
                                .collect::<Vec<_>>()
                                .join(", ")
                        ))
                    } else {
                        None
                    }
                }
            };
            if let Some(detail) = detail {
                out.push(Problem {
                    entry: e.id(),
                    detail,
                });
            }
        }
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
///
/// A `where` expression's names are checked in the same place, because the predicate reader cannot
/// check them itself: a truth file is read before its model is compiled.
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
    for axis in e.truth.predicate_vars() {
        if model.param(&axis).is_none() {
            out.push(Problem {
                entry: e.id(),
                detail: format!(
 "a region's `where` expression names {axis:?}, which is not a parameter of this model"
                ),
            });
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
