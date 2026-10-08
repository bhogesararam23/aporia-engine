//! The corpus audit: what has to be true of the *declarations* before any of them is measured.
//!
//! `corpus::verify` answers one question — does the model really fail where its entry says it does —
//! and answers it well. It cannot see the things that only show up across a corpus, and the geometry
//! experiment is about to produce numbers from entries nobody has read yet: a duplicate of an existing
//! mechanism dressed in new names, a discrete axis whose clean branch is an absence rather than a
//! claim, an entry made easy by putting its region where the atlas already divides, a `where` whose
//! bounds leave the domain and get clamped into claiming everything. Each of those would survive
//! `verify` and corrupt a result.
//!
//! So this module checks the corpus the way a reviewer would, and the experiment refuses to run
//! until it is clean. Every refusal names its entry and says what would have gone wrong, because the
//! point is not a green tick — it is that a person reading these entries in six months can see what
//! was policed when they were written.

use crate::corpus::{Entry, Problem};
use aporia_ir::{Domain, Model};

/// The families the corpus-geometry experiment added. The rules that are about *new* authored
/// entries — closed-form derivation, a stated difficulty rung — apply here and not to entries that
/// were measured before those rules existed, which is the difference between a gate and a rewrite of
/// history.
pub const NEW_FAMILIES: [&str; 4] = ["geometry", "discrete", "edge", "domain"];

/// One line of what the audit knows about an entry, for the command's own report.
#[must_use]
pub fn family_role(e: &Entry) -> String {
    let mut parts: Vec<String> = Vec::new();
    if e.truth.control {
        parts.push("control".to_string());
    }
    if e.truth.curved {
        parts.push("curved".to_string());
    }
    if e.truth.narrow {
        parts.push("narrow".to_string());
    }
    if !e.truth.choice_claims.is_empty() {
        parts.push(format!("{} choice claim(s)", e.truth.choice_claims.len()));
    }
    if let Some(rung) = &e.truth.matched_to {
        parts.push(format!("rung {rung}"));
    }
    if parts.is_empty() {
        parts.push("declared region".to_string());
    }
    parts.join(", ")
}

/// Every rule the corpus has to satisfy. Empty means the corpus is ready to be measured.
#[must_use]
pub fn audit(entries: &[Entry], per_axis: usize) -> Vec<Problem> {
    let mut out = Vec::new();
    out.extend(no_duplicate_models(entries));
    out.extend(no_duplicate_regions(entries));
    out.extend(new_entries_derive_in_closed_form(entries));
    out.extend(new_entries_state_a_rung(entries));
    out.extend(regions_are_bounded_and_reachable(entries));
    out.extend(boundaries_are_usable(entries));
    out.extend(discrete_axes_are_mapped(entries, per_axis));
    out.extend(refusals_are_refused(entries));
    out
}

/// Two entries whose models differ only in their name and their doc string are one entry.
///
/// The geometry families could have been filled with renamed copies of what E2 already measured, and
/// every summary number would have moved without any new question being asked.
fn no_duplicate_models(entries: &[Entry]) -> Vec<Problem> {
    let mut out = Vec::new();
    let prints: Vec<(String, String)> = entries
        .iter()
        .map(|e| (e.id(), fingerprint(&e.source)))
        .collect();
    for (at, (id, print)) in prints.iter().enumerate() {
        for (other_id, other) in prints.iter().skip(at + 1) {
            if print == other {
                out.push(Problem {
                    entry: id.clone(),
                    detail: format!(
                        "is the same computation as {other_id} apart from its name, so it adds a row \
                         and not a case"
                    ),
                });
            }
        }
    }
    out
}

/// The source with its name and its prose removed, whitespace collapsed.
#[must_use]
fn fingerprint(source: &str) -> String {
    let mut out = String::new();
    for line in source.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with("model ") {
            out.push_str("model\n");
            continue;
        }
        for word in trimmed.split_whitespace() {
            out.push_str(word);
            out.push(' ');
        }
        out.push('\n');
    }
    out
}

/// The same declared region in two entries is one case counted twice.
fn no_duplicate_regions(entries: &[Entry]) -> Vec<Problem> {
    let mut out = Vec::new();
    let seen: Vec<(String, String)> = entries
        .iter()
        .flat_map(|e| {
            e.truth
                .regions
                .iter()
                .map(move |r| (e.id(), declared_shape(r)))
        })
        .collect();
    for (at, (id, shape)) in seen.iter().enumerate() {
        for (other_id, other) in seen.iter().skip(at + 1) {
            if shape == other {
                out.push(Problem {
                    entry: id.clone(),
                    detail: format!(
                        "declares exactly the region {other_id} declares ({shape}), so the pair \
                         counts one reachability question twice"
                    ),
                });
            }
        }
    }
    out
}

/// A region as written: its envelope bounds in name order, and its predicate if it has one.
#[must_use]
fn declared_shape(region: &crate::truth::Declared) -> String {
    let mut axes: Vec<String> = region
        .axes
        .iter()
        .map(|(name, [lo, hi])| format!("{name}=[{lo},{hi}]"))
        .collect();
    axes.sort();
    match &region.predicate {
        None => axes.join("&"),
        Some(p) => format!("{} where {}", axes.join("&"), p.source()),
    }
}

/// A new entry's region must come from a closed form the author worked out, not from a run.
///
/// `scan_axis` exists and is honest for what it is — measuring where a rule switches. An entry whose
/// region was copied from that measurement would make the instrument mark its own homework, and the
/// geometry families are exactly where the temptation bites, because a staircase fitted to a curve is
/// easier to generate than to derive.
fn new_entries_derive_in_closed_form(entries: &[Entry]) -> Vec<Problem> {
    let mut out = Vec::new();
    for e in entries {
        if !NEW_FAMILIES.contains(&e.family.as_str()) || e.truth.static_expected {
            continue;
        }
        if !matches!(e.truth.method.as_str(), "analytic" | "ir-refusal") {
            out.push(Problem {
                entry: e.id(),
                detail: format!(
                    "declares method {:?}, but a new entry's region has to come from a closed form: \
                     a `measured` region is the instrument's own output used as its answer key",
                    e.truth.method
                ),
            });
        }
        if e.truth.derivation.trim().is_empty() {
            out.push(Problem {
                entry: e.id(),
                detail: "states no derivation, so nothing can be checked about how its region was \
                         obtained"
                    .to_string(),
            });
        }
    }
    out
}

/// Every new region-bearing entry names an existing rung, and sits within a factor of two of it.
///
/// Without this, "curved but hard" and "curved and trivial" are the same file, and a comparison that
/// reads well can be produced by choosing easy shapes. The rule was written before the entries
/// existed (0037 correction C8) and the first thing it did was refuse two of them.
fn new_entries_state_a_rung(entries: &[Entry]) -> Vec<Problem> {
    let mut out = Vec::new();
    for e in entries {
        if !NEW_FAMILIES.contains(&e.family.as_str()) || e.truth.static_expected {
            continue;
        }
        if e.truth.control {
            if e.truth.matched_to.is_some() && e.truth.regions.is_empty() {
                // A control's rung is about what it is shaped like, not about how much of it fails.
                continue;
            }
            continue;
        }
        if e.truth.regions.is_empty() {
            out.push(Problem {
                entry: e.id(),
                detail: "is not marked as a control and declares no region, so it can neither be \
                         localised nor be a false positive"
                    .to_string(),
            });
            continue;
        }
        let Some(target) = e.truth.matched_to.as_deref() else {
            out.push(Problem {
                entry: e.id(),
                detail: "declares no `matched_to`, so nothing pins how hard it is meant to be"
                    .to_string(),
            });
            continue;
        };
        let Some(other) = entries.iter().find(|o| o.id() == target) else {
            out.push(Problem {
                entry: e.id(),
                detail: format!("names {target:?} as its rung and no entry by that name exists"),
            });
            continue;
        };
        let (Some(model), Some(target_model)) = (e.model.as_ref(), other.model.as_ref()) else {
            out.push(Problem {
                entry: e.id(),
                detail: format!(
                    "cannot be compared to its rung {target:?}: one of them has no model"
                ),
            });
            continue;
        };
        let mine = share(e, model);
        let theirs = share(other, target_model);
        if theirs <= 0.0 {
            out.push(Problem {
                entry: e.id(),
                detail: format!("its rung {target:?} declares no measurable share"),
            });
            continue;
        }
        let ratio = mine / theirs;
        if ratio < 0.5 || ratio > 2.0 {
            out.push(Problem {
                entry: e.id(),
                detail: format!(
                    "covers {mine:.4} of its domain against {target:?}'s {theirs:.4}, a ratio of \
                     {ratio:.2}: outside the factor of two that makes the pair comparable. Move the \
                     entry, or name the rung it actually sits beside."
                ),
            });
        }
    }
    out
}

/// How much of the domain the entry claims is untrustworthy, on the same measure the report uses.
fn share(e: &Entry, model: &Model) -> f64 {
    e.truth.total_fraction(model)
}

/// A region with no measure cannot be scored, and a region with the whole domain cannot be missed;
/// either one makes the entry's rows uninformative while still looking like data.
///
/// This is *not* a reachability check on the verification lattice. Two committed entries failed that
/// rule when it was first written, and they were right and the rule was wrong: a zero-width box is
/// checked at its exact point, and `rlc_resonance`'s band is found by cells that *overlap* it, which
/// is what `localised` measures — no grid point need ever land inside a region for the atlas to be
/// able to report it.
fn regions_are_bounded_and_reachable(entries: &[Entry]) -> Vec<Problem> {
    let mut out = Vec::new();
    for e in entries {
        let Some(model) = e.model.as_ref() else {
            continue;
        };
        if e.truth.static_expected || e.truth.control {
            continue;
        }
        for (at, region) in e.truth.regions.iter().enumerate() {
            let fraction = region.volume_fraction(model, crate::truth::LATTICE_PER_AXIS);
            // A zero-width axis is not a zero region: `corpus::verify` checks such a box at the exact
            // point it names, and `projectile_zero_gravity`'s g = 0 is a real case that any grid misses
            // by construction. Only a region with a proper extent that still measures nothing is the
            // nothing-this-entry-declares-something case.
            let degenerate = region.axes.iter().any(|(_, [lo, hi])| lo == hi);
            if fraction <= 0.0 && !degenerate {
                out.push(Problem {
                    entry: e.id(),
                    detail: format!(
                        "region {at} covers none of the domain, so no point is ever inside it and no \
                         arm can be scored against it"
                    ),
                });
            }
            if fraction >= 0.999 {
                out.push(Problem {
                    entry: e.id(),
                    detail: format!(
                        "region {at} covers all of it ({fraction:.3}), so any cell at all is a hit"
                    ),
                });
            }
        }
    }
    out
}

/// A boundary with no tolerance asserts nothing, and a boundary on an axis the model lacks is
/// refused by `verify`; this catches the arithmetic problems left.
fn boundaries_are_usable(entries: &[Entry]) -> Vec<Problem> {
    let mut out = Vec::new();
    for e in entries {
        for b in &e.truth.boundaries {
            if b.tolerance.is_nan() || b.tolerance <= 0.0 {
                out.push(Problem {
                    entry: e.id(),
                    detail: format!(
                        "boundary on {:?} has tolerance {:?}, which asks for an exact hit the atlas \
                         can never offer",
                        b.axis, b.tolerance
                    ),
                });
            }
            let Some(model) = e.model.as_ref() else {
                continue;
            };
            if let Some([lo, hi]) = axis_span(model, &b.axis)
                && (b.at < lo || b.at > hi)
            {
                out.push(Problem {
                    entry: e.id(),
                    detail: format!(
                        "boundary on {:?} sits at {}, outside the declared [{lo}, {hi}]",
                        b.axis, b.at
                    ),
                });
            }
        }
    }
    out
}

/// Every declared value of every discrete axis is claimed, and the claim is checked.
///
/// "Explicit truth per choice" is the difference between a discrete entry that tests something and a
/// continuous entry with an extra axis. An axis where every value behaves alike is decoration, so that
/// is refused too; and a `fails` claim is verified against the model, not taken on trust, because a
/// clean branch labelled as failing would give the search a region it can never be blamed for
/// missing.
fn discrete_axes_are_mapped(entries: &[Entry], per_axis: usize) -> Vec<Problem> {
    let mut out = Vec::new();
    for e in entries {
        let Some(model) = e.model.as_ref() else {
            continue;
        };
        let discrete: Vec<(usize, Vec<f64>)> = model
            .params
            .iter()
            .enumerate()
            .filter_map(|(at, p)| match &p.domain {
                Domain::Choices(v) => Some((at, v.clone())),
                Domain::Interval { .. } => None,
            })
            .collect();
        if discrete.is_empty() {
            continue;
        }
        let axes: Vec<(usize, Vec<f64>)> = discrete;
        for (index, values) in &axes {
            let name = &model.params[*index].name;
            for value in values {
                let claims: Vec<&crate::truth::ChoiceClaim> = e
                    .truth
                    .choice_claims
                    .iter()
                    .filter(|c| &c.axis == name && c.value == *value)
                    .collect();
                match claims.len() {
                    0 => out.push(Problem {
                        entry: e.id(),
                        detail: format!(
                            "{name} = {value} has no claim, so the entry states truth for some \
                             branches and silence for this one"
                        ),
                    }),
                    1 => {}
                    n => out.push(Problem {
                        entry: e.id(),
                        detail: format!("{name} = {value} is claimed {n} times"),
                    }),
                }
            }
        }
        for claim in &e.truth.choice_claims {
            let Some((index, values)) = axes
                .iter()
                .find(|(i, _)| model.params[*i].name == claim.axis)
            else {
                out.push(Problem {
                    entry: e.id(),
                    detail: format!(
                        "a choice claim names {:?}, which is not a discrete axis of this model",
                        claim.axis
                    ),
                });
                continue;
            };
            if !values.contains(&claim.value) {
                out.push(Problem {
                    entry: e.id(),
                    detail: format!(
                        "claims {name} = {} and this axis does not offer that value: {values:?}",
                        claim.value,
                        name = claim.axis
                    ),
                });
                continue;
            }
            if !matches!(claim.expect.as_str(), "fails" | "clean") {
                out.push(Problem {
                    entry: e.id(),
                    detail: format!(
                        "claim for {} = {} says {:?}; the only readings are \"fails\" and \"clean\"",
                        claim.axis, claim.value, claim.expect
                    ),
                });
                continue;
            }
            check_claim_against_the_model(e, model, *index, claim, per_axis, &mut out);
        }
        discrete_axes_carry_information(e, model, &axes, per_axis, &mut out);
    }
    out
}

/// The span one axis is declared over: its interval, or the extent of its choice set.
#[must_use]
fn axis_span(model: &Model, name: &str) -> Option<[f64; 2]> {
    let param = model.param(name).map(|p| &model.params[p as usize])?;
    Some(match &param.domain {
        Domain::Interval { lo, hi } => [*lo, *hi],
        Domain::Choices(v) => [
            v.first().copied().unwrap_or(0.0),
            v.last().copied().unwrap_or(0.0),
        ],
    })
}

/// Does the model actually behave the way this branch's claim says, and does the declaration cover
/// what fails? A `fails` branch whose violations lie outside every declared region would give the
/// search a region it can never be blamed for missing, and a `clean` branch that fails is a truth
/// file contradicted by the model it describes.
fn check_claim_against_the_model(
    e: &Entry,
    model: &Model,
    axis: usize,
    claim: &crate::truth::ChoiceClaim,
    per_axis: usize,
    out: &mut Vec<Problem>,
) {
    let mut saw_violation = false;
    let mut violation_inside = false;
    let mut region_without_violation = false;
    let total = per_axis.pow(model.params.len() as u32);
    for at in 0..total {
        let x = crate::truth::index_point(model, at, per_axis);
        if x.get(axis).copied() != Some(claim.value) {
            continue;
        }
        let violated = crate::corpus::violates(model, &x);
        saw_violation |= violated;
        if violated && e.truth.contains(model, &x) {
            violation_inside = true;
        }
        if !violated && e.truth.contains(model, &x) {
            region_without_violation = true;
        }
    }
    match claim.expect.as_str() {
        "fails" if !saw_violation => out.push(Problem {
            entry: e.id(),
            detail: format!(
                "claims {} = {} fails and nothing on that branch violates, so the search would be \
                 scored against a region that is not there",
                claim.axis, claim.value
            ),
        }),
        "fails" if !violation_inside => out.push(Problem {
            entry: e.id(),
            detail: format!(
                "claims {} = {} fails and its violations are outside every declared region",
                claim.axis, claim.value
            ),
        }),
        "clean" if saw_violation => out.push(Problem {
            entry: e.id(),
            detail: format!(
                "claims {} = {} is clean and a point on that branch violates",
                claim.axis, claim.value
            ),
        }),
        _ => {}
    }
    if region_without_violation {
        out.push(Problem {
            entry: e.id(),
            detail: format!(
                "a declared region on the {} = {} branch contains a point that does not violate",
                claim.axis, claim.value
            ),
        });
    }
}

/// A discrete axis where every value behaves alike carries no information, so calling the entry
/// discrete would describe the file rather than the model. A control is exempt: its whole point is
/// that nothing differs.
fn discrete_axes_carry_information(
    e: &Entry,
    model: &Model,
    axes: &[(usize, Vec<f64>)],
    per_axis: usize,
    out: &mut Vec<Problem>,
) {
    if e.truth.control {
        return;
    }
    let outcomes: Vec<bool> = axes
        .iter()
        .flat_map(|(axis, values)| {
            values.iter().map(|value| {
                let total = per_axis.pow(model.params.len() as u32);
                (0..total).any(|at| {
                    let x = crate::truth::index_point(model, at, per_axis);
                    x.get(*axis).copied() == Some(*value) && crate::corpus::violates(model, &x)
                })
            })
        })
        .collect();
    if outcomes.len() > 1 && outcomes.iter().all(|o| *o == outcomes[0]) {
        let alike: Vec<String> = axes
            .iter()
            .map(|(axis, values)| {
                format!(
                    "{} ({} values, all {})",
                    model.params[*axis].name,
                    values.len(),
                    if outcomes.first().copied().unwrap_or(false) {
                        "failing"
                    } else {
                        "clean"
                    }
                )
            })
            .collect();
        out.push(Problem {
            entry: e.id(),
            detail: format!(
                "every discrete value behaves alike: {} — the discrete axis carries no \
                 information, so the entry is a continuous case with an extra column",
                alike.join(", ")
            ),
        });
    }
}

/// E1.3's measurement is the refusal, so a refusal has to be an error and a real refusal.
///
/// `static: true` alone is the unit checker's path — a scale mismatch reported as a warning over a
/// model that still compiles. A domain the IR will not accept is different in kind, and if it were
/// allowed to look the same, an entry that merely *warned* would be counted as a refused domain.
fn refusals_are_refused(entries: &[Entry]) -> Vec<Problem> {
    let mut out = Vec::new();
    for e in entries {
        if e.truth.method == "ir-refusal" {
            if e.model.is_some() {
                out.push(Problem {
                    entry: e.id(),
                    detail: "declares itself refused by the IR and still compiled, so the refusal \
                             being measured is not happening"
                        .to_string(),
                });
            }
            if e.error_count == 0 {
                out.push(Problem {
                    entry: e.id(),
                    detail: "was expected to be refused and produced no error-severity diagnostic \
                             (a warning is the unit checker's answer, not the IR's)"
                        .to_string(),
                });
            }
        }
        if !NEW_FAMILIES.contains(&e.family.as_str()) && e.truth.method == "ir-refusal" {
            out.push(Problem {
                entry: e.id(),
                detail: format!(
                    "uses the {} method, which is only meaningful for the families this experiment \
                     added",
                    "ir-refusal"
                ),
            });
        }
    }
    out
}
