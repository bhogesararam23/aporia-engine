//! Turning executions into evidence.
//!
//! Every function here has the same shape: it reads observations, measures one thing, and reports a
//! magnitude in a unit that is written down next to it. What the magnitude *means* relative to other
//! channels is not decided here — that is the calibrator's job — so an analysis never has to guess
//! how loudly to shout to be heard in the fusion.
//!
//! The five channels map onto these functions as: constraints and traces → physical, relations and
//! inferred patterns → behavioral, precision comparison → numerical, path comparison → differential,
//! probes → sensitivity.

use crate::pair::Probes;
use aporia_evidence::{Channel, Evidence, PatternKind, Subject};
use aporia_ir::{CmpOp, ConstraintKind, Model, NumType, Operand, RelationKind};
use aporia_runtime::observe::{Observation, Records};

/// A marker for a check that could not be evaluated at all.
///
/// It carries zero strength, so it never raises a risk score, but it is kept: "could not test this"
/// and "tested it and it held" are different answers, and a Trust Atlas that conflates them is
/// exactly the thing the specification warns against.
fn missing_data(
    channel: Channel,
    subject: Subject,
    records: &Records,
    probes: &Probes,
    detail: String,
) -> Evidence {
    Evidence::new(
        channel,
        subject,
        0.0,
        observation_ids(records, probes),
        detail,
    )
}

/// Relative residual used by every constraint, so a violated bound reports how far outside it the
/// model went rather than merely that it did.
#[must_use]
fn residual(value: f64, bound: f64) -> f64 {
    let scale = bound.abs().max(value.abs()).max(1.0);
    (value - bound).abs() / scale
}

fn observation_value(o: &Observation, operand: &Operand, model: &Model) -> Option<f64> {
    match operand {
        Operand::Lit(l) => l.as_f64(),
        Operand::Param(i) => o.x.get(*i as usize).copied(),
        Operand::Slot(_) | Operand::Node(_) => {
            // First the values an execution recorded for the rules it was given: a rule over a
            // computed quantity, like `abs(root - stable) < bound`, names no output at all, and
            // without this the rule would be skipped and the physical channel would be blind to
            // exactly the faults an author is most likely to write a rule about.
            if let Operand::Node(id) = operand
                && let Some((_, v)) = o.rule_values.iter().find(|(n, _)| *n == *id)
            {
                return Some(*v);
            }
            // Otherwise fall back to the output the value was named by, which is how a rule over a
            // top-level `let` resolves.
            model
                .outputs
                .iter()
                .position(|out| out.value == *operand)
                .and_then(|j| o.y.get(j).copied())
        }
    }
}

/// Declared `require` rules, evaluated at one execution. Channel: physical.
#[must_use]
pub fn constraints(model: &Model, o: &Observation) -> Vec<Evidence> {
    let mut out = Vec::new();
    for c in &model.constraints {
        let (magnitude, detail) = match &c.kind {
            ConstraintKind::Cmp {
                lhs,
                cmp,
                rhs,
                tolerance,
            } => {
                let (Some(l), Some(r)) = (
                    observation_value(o, lhs, model),
                    observation_value(o, rhs, model),
                ) else {
                    continue;
                };
                if !l.is_finite() || !r.is_finite() {
                    // A non-finite side is divergence's business, not a rule violation.
                    continue;
                }
                match cmp {
                    CmpOp::EqApprox => {
                        let tol = if *tolerance == 0.0 { 1e-9 } else { *tolerance };
                        let over = (l - r).abs() / (tol * r.abs().max(1.0));
                        (
                            (over - 1.0).max(0.0),
                            format!(
                                "{} ~ {r} needed |Δ|<{} found |Δ|={}",
                                l,
                                tol * r.abs().max(1.0),
                                (l - r).abs()
                            ),
                        )
                    }
                    other => {
                        if other.holds(l, r, *tolerance) {
                            continue;
                        }
                        (
                            // Whichever comparison failed, the distance to the declared bound is the
                            // same quantity: how far the value sits outside the rule.
                            residual(l, r),
                            format!("{} violated: {l} not {other:?} {r}", c.name),
                        )
                    }
                }
            }
            ConstraintKind::Finite { value } => {
                let Some(v) = observation_value(o, value, model) else {
                    continue;
                };
                if v.is_finite() {
                    continue;
                }
                (1.0, format!("{} left the real numbers: {v}", c.name))
            }
        };
        if magnitude > 0.0 {
            let e = Evidence::new(
                Channel::Physical,
                Subject::Constraint(c.id),
                magnitude,
                vec![o.id],
                detail,
            )
            // A rule that fired is a fact, not a measurement to be compared with other firings, so
            // it gets the same treatment a NaN already had. Calibrating it was a real loss: on an
            // explicit-Euler oscillator whose energy blows past its declared bound across two thirds
            // of the time-step range, the mild violations at the edge of that region were divided by
            // the median residual of the dramatic ones and came out near 0.34, under the bar, so the
            // atlas flagged 6% of a space whose declared failure region is 67%. "How far outside"
            // still travels as `magnitude`, which is what a report and the minimizer rank on; it no
            // longer decides whether the rule fired.
            .absolute(1.0);
            out.push(e);
        }
    }
    out
}

/// A baseline that APORIA always applies, whether or not the model declared anything: an output
/// that is NaN or infinite is a fact about the region, and reporting it under the physical channel
/// keeps it separate from a declared rule the analyst might disagree with.
#[must_use]
pub fn divergence(model: &Model, o: &Observation) -> Vec<Evidence> {
    let mut out = Vec::new();
    for (j, y) in o.y.iter().enumerate() {
        if y.is_nan() {
            out.push(
                Evidence::new(
                    Channel::Physical,
                    Subject::Divergence { output: j as u16 },
                    1.0,
                    vec![o.id],
                    format!("{} is NaN", model.outputs[j].name),
                )
                .absolute(1.0),
            );
        } else if y.is_infinite() {
            out.push(
                Evidence::new(
                    Channel::Physical,
                    Subject::Divergence { output: j as u16 },
                    1.0,
                    vec![o.id],
                    format!("{} is infinite", model.outputs[j].name),
                )
                .absolute(0.9),
            );
        }
    }
    out
}

/// A declared relation, measured over the pairs that probe it.
///
/// Every arm names the evaluations its own measurement was taken from. That is not a reporting
/// nicety: the cell a finding is charged to is decided by which observations it names, and a
/// relation measured between two probes that says "every observation in the experiment" makes the
/// whole explored space responsible for one comparison. Measured with the atlas labelled from the
/// finished evidence, that bug put the entire domain of a correct projectile into SUSPICIOUS because
/// one power-law exponent came out 0.02 away from the declared 2.
#[expect(
    clippy::too_many_lines,
    reason = "one arm per relation kind, and each measurement is its own algorithm"
)]
#[must_use]
pub fn relations(model: &Model, records: &Records, probes: &Probes) -> Vec<Evidence> {
    let mut out = Vec::new();
    for r in &model.relations {
        // A conserved quantity is a statement about one execution's whole trajectory, so it is
        // measured once per execution and every drifting execution becomes a finding of its own. The
        // other relations compare probe pairs and are one statement over the experiment; this one
        // would otherwise fold a region-wide property into the single worst run, which measured as a
        // model whose energy drifts at every large time step coming out suspicious across 6% of its
        // space when 67% of it is the declared failure region.
        if let RelationKind::Conserved { trace, tolerance } = &r.kind {
            let mut measured_any = false;
            for o in &records.items {
                let Some(drift) = drift_of(o, *trace) else {
                    continue;
                };
                measured_any = true;
                if drift <= *tolerance {
                    continue;
                }
                out.push(Evidence::new(
                    Channel::Behavioral,
                    Subject::Relation(r.id),
                    drift / tolerance.max(1e-15),
                    vec![o.id],
                    format!("trace t{trace} drifted {drift:.3e}, allowed {tolerance:.3e}"),
                ));
            }
            if !measured_any {
                // Nothing was measured, which is not the same as nothing being wrong.
                out.push(missing_data(
                    Channel::Behavioral,
                    Subject::Relation(r.id),
                    records,
                    probes,
                    format!("conservation of t{trace} could not be evaluated: no trace values were recorded"),
                ));
            }
            continue;
        }
        // Symmetry is a comparison between two executions, and the campaign records one such pair per
        // sampled base point. Folding them into a single item -- which is what one measurement over
        // the whole probe set would do -- would report "this relation held at the first point we
        // swapped" as if it were the model's property, and would hide every other point where it did
        // not. So each swap contributes its own evidence, named to the two executions compared.
        if let RelationKind::Symmetric { out: o, pair } = &r.kind {
            for swap in probes.swaps.iter().filter(|s| &s.pair == pair) {
                let Some((d, ids)) = swap_distance(records, swap, *o) else {
                    continue;
                };
                if d <= 0.0 {
                    continue;
                }
                out.push(Evidence::new(
                    Channel::Behavioral,
                    Subject::Relation(r.id),
                    d,
                    ids.to_vec(),
                    format!(
                        "{o} changed by {d:.3e} when p{} and p{} were swapped",
                        pair[0], pair[1]
                    ),
                ));
            }
            continue;
        }
        let (magnitude, observations, detail) = match &r.kind {
            RelationKind::Monotone {
                out: o,
                param,
                direction,
            } => {
                let expect_increasing = *direction == aporia_ir::Direction::Increasing;
                let Some(m) = scan_monotone(records, probes, *o, *param, expect_increasing) else {
                    continue;
                };
                if m.violated == 0 {
                    continue;
                }
                let size = (m.violated as f64 / m.total as f64) * m.worst;
                (
                    size,
                    m.offenders,
                    format!(
                        "{o} failed to move as declared against p{param} in {} of {} probes (worst relative excursion {:.3})",
                        m.violated, m.total, m.worst
                    ),
                )
            }
            RelationKind::ScalesAs {
                out: o,
                param,
                power,
            } => {
                let Some((slope, deviation, ids)) =
                    exponent_evidence(records, probes, *o, *param, *power)
                else {
                    continue;
                };
                (
                    deviation,
                    ids,
                    format!("{o} scales like p{param}^{slope:.3}, declared {power}"),
                )
            }
            RelationKind::Lipschitz {
                out: o,
                param,
                bound,
            } => {
                let Some((slope, ids)) = slope_offenders(records, probes, *o, *param, *bound)
                else {
                    continue;
                };
                (
                    slope / bound,
                    ids,
                    format!("{o} changed at {slope:.3} per unit of p{param}, bound {bound}"),
                )
            }
            RelationKind::Symmetric { .. } | RelationKind::Conserved { .. } => {
                // Both handled above, one item per execution or per swap: neither is a single
                // measurement over the probe set, so neither belongs in this one-item shape.
                continue;
            }
        };
        if magnitude.is_finite() && magnitude > 0.0 {
            out.push(Evidence::new(
                Channel::Behavioral,
                Subject::Relation(r.id),
                magnitude,
                observations,
                detail,
            ));
        }
    }
    out
}

/// Behaviour APORIA looks for without being told. This is the piece that overlaps dynamic invariant
/// detection, and it is deliberately kept as one sensor rather than the answer: a violation of an
/// inferred pattern is evidence, not a verdict.
#[must_use]
pub fn inferred_patterns(model: &Model, records: &Records, probes: &Probes) -> Vec<Evidence> {
    let mut out = Vec::new();
    for param in 0..model.params.len() as u16 {
        if model.params[param as usize].ty.num == NumType::I64 {
            continue;
        }
        for o in 0..model.outputs.len() as u16 {
            // Only "keeps rising" is inferred. A decreasing relation is the same measurement with a
            // sign change, and inferring both directions from noisy data doubles the false-positive
            // rate for no information. `violated` therefore counts the pairs that fell.
            //
            // Bound rather than early-continued: the continuity measurement below is a separate
            // sensor, and it has to run whether or not this axis had anything to say.
            let scanned = scan_monotone(records, probes, o, param, true);
            if let Some(Monotony {
                violated,
                total,
                worst,
                severity,
                offenders,
            }) = scanned
            {
                // Two conditions, each of which kills one way this sensor used to manufacture
                // findings.
                //
                // The claim has to be one the data supports. "Rising with p0" fitted from samples
                // where a quarter of the pairs fall is not a hypothesis about the model, it is a
                // description of part of the space, and charging the rest of it with violating it
                // reports the model's own oscillation as a defect. Measured, before this condition
                // existed: a correct symplectic integrator had "o0 stopped rising with p0 in 40 of 84
                // probes" attached to cells across its domain at full strength, because half of an
                // oscillator's probes fall and the sensor had invented a claim that never held.
                //
                // And the reversal has to be out of family in *size* with the rest of that axis. A
                // smooth maximum turns over on the smallest step around it — that is what a maximum
                // is — so a reversal that moves exactly as far as an ordinary step says only "this
                // quantity has a turning point". A reversal that moves several times further, or that
                // stops moving where the model usually does, is the shape of a kink, a clamp or a
                // discontinuity, which is what this channel exists to find. Declared relations are
                // unaffected: an author who wrote `monotone_up` is owed an answer about direction,
                // and `relations` gives it one.
                if violated > 0 && violated * 4 < total && worst > 1e-6 && severity >= 2.0f64.ln() {
                    out.push(Evidence::new(
                        Channel::Behavioral,
                        Subject::Pattern {
                            output: o,
                            param,
                            kind: PatternKind::Increasing,
                        },
                        severity,
                        offenders,
                        format!(
                            "o{o} turned back against p{param} in {violated} of {total} probes, {severity:.2} log steps from an ordinary step along it"
                        ),
                    ));
                }
            }
            // There used to be a second sensor here: "the biggest step along this axis is far bigger
            // than the median step", which is a reasonable idea and a broken measurement. It sorted
            // every sample the experiment had by one parameter and diffed the neighbours, so
            // consecutive samples generally differed in *all* the other parameters as well. On a
            // smooth three-dimensional model that produced "r jumps by 13752.3x its typical step
            // along p0" — the projection of a surface onto one axis, not a discontinuity — and once
            // the atlas was labelled from the finished evidence, that artifact put a correct
            // projectile's whole domain in SUSPICIOUS.
            //
            // The event it was reaching for is measured properly elsewhere now: `sensitivity`
            // compares a probe's slope against the median slope of probes of the same output along
            // the same axis, and a probe moves one parameter by construction; a declared
            // `check lipschitz` tests steepness against a bound the author chose. Neither needs a
            // projected secant standing in for it, so the sensor is removed rather than kept and
            // damped with another threshold.
        }
    }
    out
}

/// How much further an output moved than it usually does for a deliberately small move in one
/// parameter.
///
/// The magnitude is a ratio of *slopes*: the probe's `|Δy| / Δx` divided by the median of that same
/// quantity over every probe of that output along that axis. One is therefore "this probe moved the
/// way this model moves", the reported number is the excess `ratio - 1`, and a probe has to reach
/// 1.5 before this channel says anything at all. The units of the output, of the parameter and of
/// the probe step all cancel, so the channel says the same thing for a millimetre as for a
/// light-year: this place changes faster than the rest of the axis does.
///
/// The measure this replaced was an elasticity — relative output change over relative input change.
/// That is a better description of a power law and a worse one of a physical model, and the reason
/// is a singularity rather than a subtlety: dividing by the *instantaneous* value of the output puts
/// every root of that output inside the measurement. The range of a projectile at a vertical launch
/// is zero, so a move there is an infinite multiple of nothing, and the reading only decays as one
/// over the distance from the root — no threshold removes it, because it is not a band, it is the
/// shape of the division. Correct models were going suspicious wherever their output crossed zero,
/// which in physics is everywhere. A spring at the equilibrium point, a projectile at ninety
/// degrees, a velocity at a turnaround: none of them is a defect, and none of them is a finding here.
#[must_use]
pub fn sensitivity(records: &Records, probes: &Probes) -> Vec<Evidence> {
    let (sightings, reference) = probe_slopes(records, probes);
    slope_items(&sightings, &reference)
}

/// What "usual" means for the sensitivity channel: the median slope of every probe the campaign
/// actually took, per (axis, output).
///
/// Separated from [`sensitivity`] because a caller that wants to ask about one *new* point cannot
/// recompute this from that point's own neighbourhood. A three-point star is typical of itself: the
/// ratio would be 1 by construction, the channel would fall silent, and the answer would be a
/// property of the question rather than of the model. So a campaign-wide reference is measured once,
/// frozen, and every candidate is judged against the same ordinary value the report used.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SlopeReference {
    typical: Vec<f64>,
    width: usize,
}

impl SlopeReference {
    /// The ordinary slope for one output along one axis, or 0.0 when nothing was ever probed there.
    /// Zero means "no ordinary amount to be unusual against", which [`slope_items`] reports as
    /// nothing rather than as an infinite multiple.
    #[must_use]
    pub fn slope(&self, axis: u16, output: usize) -> f64 {
        self.typical
            .get(axis as usize * self.width + output)
            .copied()
            .unwrap_or(0.0)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.typical.is_empty()
    }
}

/// One probe's slope, kept with the two executions that produced it.
type Sighting = (u16, usize, f64, u64, u64);

/// Every probe slope in the record set, bucketed by (axis, output), plus the frozen median of each
/// bucket. This is the only place the slope of a probe is defined, so the campaign-wide pass and a
/// single-point query cannot drift apart in arithmetic.
fn probe_slopes(records: &Records, probes: &Probes) -> (Vec<Sighting>, SlopeReference) {
    let Some(first) = records.items.first() else {
        return (Vec::new(), SlopeReference::default());
    };
    let axes = first.x.len();
    let width = first.y.len();
    // Slopes are gathered before anything is judged, because the comparison is against this output's
    // own typical slope along this axis: a pass over the pairs has to finish before a pass over them
    // can mean anything.
    let mut slopes = vec![Vec::new(); axes * width];
    let mut seen: Vec<Sighting> = Vec::new();
    for pair in &probes.pairs {
        let (Some(base), Some(moved)) = (records.by_id(pair.base), records.by_id(pair.perturbed))
        else {
            continue;
        };
        let (Some(x0), Some(x1)) = (
            base.x.get(pair.axis as usize).copied(),
            moved.x.get(pair.axis as usize).copied(),
        ) else {
            continue;
        };
        let dx = (x1 - x0).abs();
        if dx <= 0.0 || !dx.is_finite() {
            continue;
        }
        for j in 0..width {
            let (Some(y0), Some(y1)) = (base.y.get(j).copied(), moved.y.get(j).copied()) else {
                continue;
            };
            if !y0.is_finite() || !y1.is_finite() {
                continue;
            }
            let slope = (y1 - y0).abs() / dx;
            slopes[pair.axis as usize * width + j].push(slope);
            seen.push((pair.axis, j, slope, base.id, moved.id));
        }
    }
    let typical: Vec<f64> = slopes
        .iter()
        .map(|v| aporia_numerics::median_of(v))
        .collect();
    (seen, SlopeReference { typical, width })
}

/// The channel's output from measured slopes and a reference: `ratio - 1` for every probe that moved
/// more than half again as far as the ordinary one, and nothing for the rest.
fn slope_items(sightings: &[Sighting], reference: &SlopeReference) -> Vec<Evidence> {
    let mut out = Vec::new();
    for (axis, j, slope, base_id, moved_id) in sightings {
        let typical = reference.slope(*axis, *j);
        if typical <= 0.0 || !typical.is_finite() {
            // Every probe along this axis moved the output by nothing, so there is no ordinary
            // amount to be unusual against. Saying nothing is the honest answer.
            continue;
        }
        let ratio = slope / typical;
        // Half again as far as the ordinary step, or nothing said. `ratio > 1` is not a usable bar:
        // two probes that moved the *same* distance differ in their last ulp, one of them then
        // exceeds the median of the pair, and the channel fills with findings whose whole content is
        // floating-point noise. Below this bar a probe is inside the model's own variation.
        if ratio > 1.5 {
            out.push(Evidence::new(
                Channel::Sensitivity,
                Subject::LocalSlope {
                    output: *j as u16,
                    axis: *axis,
                },
                ratio - 1.0,
                vec![*base_id, *moved_id],
                format!("o{j} moved {ratio:.1}x further than usual along p{axis}"),
            ));
        }
    }
    out
}

/// Sensitivity evidence for one base execution and the probes taken from it, judged against a frozen
/// [`SlopeReference`].
///
/// Same slope, same reference statistic, same bar and same sentence as [`sensitivity`]; the only
/// difference is that the caller supplies the ordinary value instead of it being measured from the
/// same handful of points. That is what makes the channel answerable about a point the campaign never
/// sampled, which is what a counterexample minimiser needs: it proposes coordinates, and something
/// has to say whether they are still as unusual as the ones the report flagged.
#[must_use]
pub fn sensitivity_at(
    base: &Observation,
    moved: &[(u16, &Observation)],
    reference: &SlopeReference,
) -> Vec<Evidence> {
    let mut sightings: Vec<Sighting> = Vec::new();
    for (axis, other) in moved {
        let Some(x0) = base.x.get(*axis as usize).copied() else {
            continue;
        };
        let Some(x1) = other.x.get(*axis as usize).copied() else {
            continue;
        };
        let dx = (x1 - x0).abs();
        if dx <= 0.0 || !dx.is_finite() {
            continue;
        }
        for j in 0..base.y.len() {
            let (Some(y0), Some(y1)) = (base.y.get(j).copied(), other.y.get(j).copied()) else {
                continue;
            };
            if !y0.is_finite() || !y1.is_finite() {
                continue;
            }
            sightings.push((*axis, j, (y1 - y0).abs() / dx, base.id, other.id));
        }
    }
    slope_items(&sightings, reference)
}

/// The reference the campaign's own probes measured, frozen for reuse by a single-point query.
#[must_use]
pub fn slope_reference(records: &Records, probes: &Probes) -> SlopeReference {
    probe_slopes(records, probes).1
}

/// Two paths that should give the same number: precision modes for numerical evidence, different
/// implementations or backends for differential evidence.
#[must_use]
pub fn disagreement(
    model: &Model,
    ids: &[u64],
    a: &[f64],
    b: &[f64],
    channel: Channel,
) -> Vec<Evidence> {
    let mut out = Vec::new();
    for (j, (x, y)) in a.iter().zip(b.iter()).enumerate() {
        let rel = aporia_numerics::relative(*x, *y);
        let ulps = aporia_numerics::ulps(*x, *y);
        // Both sides non-finite and of the same kind is agreement; anything else where one side is
        // non-finite is a divergence, reported by the physical channel already.
        if !(x.is_finite() && y.is_finite()) {
            continue;
        }
        if rel > 0.0 && ulps > 1 {
            out.push(Evidence::new(
                channel,
                if channel == Channel::Numerical {
                    Subject::Pattern {
                        output: j as u16,
                        param: u16::MAX,
                        kind: PatternKind::Continuity,
                    }
                } else {
                    Subject::PathDisagreement { output: j as u16 }
                },
                rel,
                ids.to_vec(),
                format!(
                    "{} differs between paths: rel={rel:.3e} ulps={ulps}",
                    model.outputs.get(j).map_or("?", |o| o.name.as_str())
                ),
            ));
        }
    }
    out
}

/// Numerical channel: the same model at two floating-point configurations.
#[must_use]
pub fn numerical(model: &Model, ids: &[u64], f64_out: &[f64], f32_out: &[f64]) -> Vec<Evidence> {
    disagreement(model, ids, f64_out, f32_out, Channel::Numerical)
}

/// Differential channel: the runtime against an independent implementation of the same model.
///
/// The distinction from `numerical` is the one §7 of the specification draws. f64-versus-f32 asks
/// "does the answer depend strongly on the numerical choices I made inside this one program?" — one
/// implementation, two configurations. Against the reference path asks "do two programs agree?" —
/// `aporia_numerics::reference` is a separate evaluator that repeats the arithmetic on purpose,
/// because if it shared the runtime's operations then agreement would prove something about the code
/// and nothing about the model. A disagreement between configurations is a conditioning signal; a
/// disagreement between implementations is a claim that one of them is wrong.
///
/// This function had no caller anywhere in the project until the campaign wired it, which is how a
/// five-channel instrument shipped with four channels for a whole measurement campaign.
#[must_use]
pub fn against_reference(
    model: &Model,
    ids: &[u64],
    fast: &[f64],
    reference: &[f64],
) -> Vec<Evidence> {
    let out = disagreement(model, ids, fast, reference, Channel::Differential);
    // No magnitude fudge here. An earlier version multiplied this channel's distances by 1e9 "because
    // the reference is the more trustworthy signal", which was a way of making one channel outrank
    // another by decree; it predates per-claim calibration, and with 0014 in place it is both
    // unnecessary and harmful — it put the channel's own typical value at 1e9 times the measured
    // distance, so genuine 0.2% disagreements between two implementations calibrated to zero.
    // "How unusual is this for this claim" is now answered by the calibrator, not baked into the
    // measurement.
    out
}

/// Differential channel: two execution paths that are supposed to compute the same thing.
#[must_use]
pub fn differential(model: &Model, ids: &[u64], a: &[f64], b: &[f64]) -> Vec<Evidence> {
    disagreement(model, ids, a, b, Channel::Differential)
}

// ------------------------------------------------------------------ measurements

/// Count and severity of monotonicity failures over the pairs on one axis.
///
/// Two numbers come out, because they answer different questions. `worst` is how far the model moved
/// against the claim at its most extreme, which is what an author who wrote `check monotone_up`
/// needs: the contract was broken, and by how much. `severity` is the size of a typical *failing*
/// step measured against the typical step of *every* pair on that axis, in log units, which is what
/// an inferred claim needs: a reversal that moves exactly as far as the model usually moves is the
/// model moving normally in the other direction, and only a reversal whose size is out of family
/// with the rest of the axis is a qualitative event.
struct Monotony {
    violated: usize,
    total: usize,
    worst: f64,
    severity: f64,
    offenders: Vec<u64>,
}

fn scan_monotone(
    records: &Records,
    probes: &Probes,
    output: u16,
    param: u16,
    expect_increasing: bool,
) -> Option<Monotony> {
    let mut violated = 0;
    let mut total = 0;
    let mut worst = 0.0f64;
    let mut offenders: Vec<u64> = Vec::new();
    let mut every_step: Vec<f64> = Vec::new();
    let mut wrong_steps: Vec<f64> = Vec::new();
    for pair in probes.pairs.iter().filter(|p| p.axis == param) {
        let (Some(base), Some(moved)) = (records.by_id(pair.base), records.by_id(pair.perturbed))
        else {
            continue;
        };
        let (Some(y0), Some(y1)) = (base.y.get(output as usize), moved.y.get(output as usize))
        else {
            continue;
        };
        if !y0.is_finite() || !y1.is_finite() {
            continue;
        }
        total += 1;
        let scale = y0.abs().max(y1.abs()).max(1.0);
        let rel = (y1 - y0).abs() / scale;
        every_step.push(rel);
        let rose = y1 > y0;
        if rose != expect_increasing && y1 != y0 {
            violated += 1;
            // The pairs that broke the pattern, not every pair that was examined: a relation that
            // fails in one corner should make that corner suspicious, not the whole space.
            offenders.extend([pair.base, pair.perturbed]);
            wrong_steps.push(rel);
            worst = worst.max(rel);
        }
    }
    if total == 0 {
        return None;
    }
    offenders.sort_unstable();
    offenders.dedup();
    // Log distance of a typical failing step from a typical step at all: symmetric, so a reversal
    // half the usual size (a stall, a clamp) is the same strength of event as one twice the usual
    // size (a kink, a discontinuity). A model that never moves has no ordinary step to be measured
    // against, and severity zero is the honest answer where a ratio would be a division by zero.
    let typical = aporia_numerics::median_of(&every_step);
    let severity = if typical > 0.0 && !wrong_steps.is_empty() {
        let ratio = aporia_numerics::median_of(&wrong_steps) / typical;
        ratio.max(f64::MIN_POSITIVE).ln().abs()
    } else {
        0.0
    };
    Some(Monotony {
        violated,
        total,
        worst,
        severity,
        offenders,
    })
}

/// How far one execution's trace drifted from its own first value, as a fraction of that value.
///
/// Measured per execution, not per experiment, because that is the unit the atlas can use: an
/// evaluation is a whole run, and the question a Trust Atlas answers is *where in the parameter
/// space* runs drift. Reducing it to the single worst run in the experiment — which is what this did
/// first — names one evaluation and leaves the atlas to call a model whose energy drifts everywhere
/// suspicious in 6% of its space.
fn drift_of(record: &Observation, trace: u16) -> Option<f64> {
    let series = record.traces.get(trace as usize)?;
    let first = *series.first()?;
    if first == 0.0 || !first.is_finite() {
        return None;
    }
    let mut worst = 0.0f64;
    let mut seen = false;
    for v in series {
        if v.is_finite() {
            worst = worst.max((v - first).abs() / first.abs());
            seen = true;
        }
    }
    seen.then_some(worst)
}

/// Every probe pair whose slope exceeded `bound`, with the steepest slope among them.
fn slope_offenders(
    records: &Records,
    probes: &Probes,
    output: u16,
    param: u16,
    bound: f64,
) -> Option<(f64, Vec<u64>)> {
    let mut ids: Vec<u64> = Vec::new();
    let mut steepest = 0.0f64;
    for pair in probes.pairs.iter().filter(|p| p.axis == param) {
        let (Some(base), Some(moved)) = (records.by_id(pair.base), records.by_id(pair.perturbed))
        else {
            continue;
        };
        let (Some(x0), Some(x1), Some(y0), Some(y1)) = (
            base.x.get(param as usize),
            moved.x.get(param as usize),
            base.y.get(output as usize),
            moved.y.get(output as usize),
        ) else {
            continue;
        };
        if !y0.is_finite() || !y1.is_finite() || x1 == x0 {
            continue;
        }
        let s = (y1 - y0).abs() / (x1 - x0).abs();
        if s > steepest {
            steepest = s;
        }
        if s > bound {
            ids.extend([base.id, moved.id]);
        }
    }
    if ids.is_empty() {
        return None;
    }
    ids.sort_unstable();
    ids.dedup();
    Some((steepest, ids))
}

/// The mean power-law exponent, declared exponent, and every probe that agreed with the verdict
/// rather than sitting below it.
fn exponent_evidence(
    records: &Records,
    probes: &Probes,
    output: u16,
    param: u16,
    power: f64,
) -> Option<(f64, f64, Vec<u64>)> {
    let local = local_exponents(records, probes, output, param);
    if local.is_empty() {
        return None;
    }
    let mean = local.iter().map(|(s, _)| *s).sum::<f64>() / local.len() as f64;
    let deviation = (mean - power).abs();
    // The witnesses are the probes that show the deviation, not the probes that happen to bracket
    // the mean: an exponent that comes out 1.94 where 2 was declared is wrong at every pair that
    // reads 1.9-ish, and naming only the furthest one would blame a single cell for a property of
    // the whole relation.
    let mut ids: Vec<u64> = local
        .iter()
        .filter(|(s, _)| (*s - power).abs() >= deviation * 0.5)
        .flat_map(|(_, pair)| pair.to_vec())
        .collect();
    ids.sort_unstable();
    ids.dedup();
    if ids.is_empty() {
        ids = local[0].1.to_vec();
    }
    Some((mean, deviation, ids))
}

/// Every local exponent estimate along one axis, each with the pair that produced it.
fn local_exponents(
    records: &Records,
    probes: &Probes,
    output: u16,
    param: u16,
) -> Vec<(f64, [u64; 2])> {
    let mut out = Vec::new();
    for pair in probes.pairs.iter().filter(|p| p.axis == param) {
        let (Some(base), Some(moved)) = (records.by_id(pair.base), records.by_id(pair.perturbed))
        else {
            continue;
        };
        let (Some(x0), Some(x1)) = (base.x.get(param as usize), moved.x.get(param as usize)) else {
            continue;
        };
        let (Some(y0), Some(y1)) = (base.y.get(output as usize), moved.y.get(output as usize))
        else {
            continue;
        };
        if *x0 <= 0.0 || *x1 <= 0.0 || *y0 <= 0.0 || *y1 <= 0.0 {
            // A power law through zero or a negative value has no logarithm. Skipping is honest;
            // substituting a sign trick would report an exponent the data does not support.
            continue;
        }
        let dx = x1.ln() - x0.ln();
        if dx.abs() < 1e-12 {
            continue;
        }
        out.push(((y1.ln() - y0.ln()) / dx, [base.id, moved.id]));
    }
    out
}

/// Relative distance between an execution and the same execution with two parameters swapped,
/// named to the two executions that were compared.
///
/// Takes the swap probe itself rather than searching `probes` for a matching pair. A campaign records
/// one swap per declared pair per sampled base point, and taking the first of them would turn a
/// relation claimed to hold everywhere into a statement about one lucky point.
fn swap_distance(
    records: &Records,
    swap: &crate::pair::SwapProbe,
    output: u16,
) -> Option<(f64, [u64; 2])> {
    let (Some(a), Some(b)) = (records.by_id(swap.base), records.by_id(swap.swapped)) else {
        return None;
    };
    let (Some(y0), Some(y1)) = (a.y.get(output as usize), b.y.get(output as usize)) else {
        return None;
    };
    if !y0.is_finite() || !y1.is_finite() {
        return None;
    }
    Some((aporia_numerics::relative(*y0, *y1), [a.id, b.id]))
}

fn observation_ids(_records: &Records, probes: &Probes) -> Vec<u64> {
    let mut ids: Vec<u64> = probes
        .pairs
        .iter()
        .flat_map(|p| [p.base, p.perturbed])
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ids
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pair::Pair;
    use aporia_dsl::lower::compile;
    use aporia_ir::Model;
    use aporia_runtime::interp::{Outcome, run};
    use aporia_runtime::value::ExecConfig;

    fn model(text: &str) -> Model {
        let c = compile("t.ap", text);
        assert!(!c.diagnostics.has_errors(), "{}\n{text}", c.diagnostics);
        c.model
    }

    fn record(model: &Model, id: u64, x: &[f64]) -> Observation {
        let o: Outcome = run(model, x, ExecConfig::default());
        Observation::new(id, x.to_vec(), &o)
    }

    fn dataset(model: &Model, xs: &[&[f64]]) -> Records {
        let mut r = Records::new();
        for (i, x) in xs.iter().enumerate() {
            r.push(record(model, i as u64, x));
        }
        r
    }

    fn pairs(items: &[(u64, u64, u16)]) -> Probes {
        Probes {
            pairs: items
                .iter()
                .map(|(b, p, a)| Pair {
                    base: *b,
                    perturbed: *p,
                    axis: *a,
                })
                .collect(),
            kinds: vec![crate::pair::ProbeKind::Targeted; items.len()],
            axis_step: vec![0.01],
            swaps: Vec::new(),
        }
    }

    #[test]
    fn a_satisfied_rule_produces_nothing() {
        let m = model(
            "model p \"\" {\n input g : m/s^2 in [9.7, 9.9]\n let y = 1.0 / g\n require g > 0\n}\n",
        );
        let o = record(&m, 0, &[9.81]);
        assert!(constraints(&m, &o).is_empty());
    }

    #[test]
    fn a_violated_rule_reports_how_far_outside_it_the_model_is() {
        let m = model("model n \"\" {\n input x in [-10, 10]\n let y = x\n require y >= 0\n}\n");
        let bad = record(&m, 0, &[-3.0]);
        let ev = constraints(&m, &bad);
        assert_eq!(ev.len(), 1);
        assert!(
            (ev[0].magnitude - 3.0 / 3.0).abs() < 1e-12,
            "{}",
            ev[0].magnitude
        );
        assert_eq!(ev[0].channel, Channel::Physical);
        assert_eq!(ev[0].observations, vec![0]);
    }

    #[test]
    fn a_rule_over_computed_values_is_evaluated_not_skipped() {
        // `require gap < bound` names two intermediate results and no output at all. Resolving rule
        // operands only through the output list made this rule invisible: nothing was reported, on
        // either side of the boundary, which is the worst kind of blindness because it looks like
        // agreement.
        let m = model(
            "model q \"\" {
 input b in [1, 10000]
 let s = b * b
 let gap = s - b * b
 let bound = 0.001
 require gap < bound
}
",
        );
        let nodes = m.rule_nodes();
        assert!(!nodes.is_empty(), "the rule should name computed operands");
        let good = record(&m, 0, &[2.0]);
        assert!(
            constraints(&m, &good).is_empty(),
            "gap is exactly zero, so the rule holds"
        );
        let bad = model(
            "model q2 \"\" {
 input b in [1, 10000]
 let gap = b - 100
 let bound = 5
 require gap < bound
}
",
        );
        let over = record(&bad, 1, &[9000.0]);
        let ev = constraints(&bad, &over);
        assert_eq!(ev.len(), 1, "a computed lhs must still be checked");
        assert_eq!(ev[0].channel, Channel::Physical);
    }

    #[test]
    fn a_finite_rule_fires_on_nan_and_divergence_also_notices() {
        let m =
            model("model d \"\" {\n input x in [-4, 4]\n let y = sqrt(x)\n require finite(y)\n}\n");
        let o = record(&m, 5, &[-1.0]);
        assert_eq!(constraints(&m, &o).len(), 1);
        assert_eq!(divergence(&m, &o).len(), 1);
    }

    #[test]
    fn divergence_distinguishes_nan_from_infinity() {
        let m =
            model("model d \"\" {\n input x in [0, 4]\n let a = ln(x)\n let b = 1 / (x - 2)\n}\n");
        let o = record(&m, 0, &[0.0]);
        let d = divergence(&m, &o);
        assert!(d.iter().any(|e| e.detail.contains("infinite")));
    }

    #[test]
    fn monotonicity_is_measured_over_pairs_not_points() {
        // y = x^2 is not increasing over [-5, 5]; the probe set straddles the turning point.
        let m = model(
            "model s \"\" {\n input x in [-5, 5]\n let y = x * x\n check monotone_up(y wrt x)\n}\n",
        );
        let r = dataset(
            &m,
            &[
                &[-4.0, 0.0, 0.0],
                &[-3.0, 0.0, 0.0],
                &[3.0, 0.0, 0.0],
                &[4.0, 0.0, 0.0],
            ],
        );
        let p = pairs(&[(0, 1, 0), (2, 3, 0)]);
        let ev = relations(&m, &r, &p);
        assert_eq!(ev.len(), 1, "{ev:?}");
        assert_eq!(ev[0].channel, Channel::Behavioral);
        assert!(ev[0].magnitude > 0.0);
    }

    #[test]
    fn a_satisfied_monotonicity_produces_nothing() {
        let m = model(
            "model l \"\" {\n input x in [0, 5]\n let y = 2 * x\n check monotone_up(y wrt x)\n}\n",
        );
        let r = dataset(&m, &[&[0.0], &[1.0], &[2.0], &[3.0]]);
        assert!(relations(&m, &r, &pairs(&[(0, 1, 0), (1, 2, 0), (2, 3, 0)])).is_empty());
    }

    #[test]
    fn a_scaling_relation_recovers_the_declared_exponent() {
        let m = model(
            "model q \"\" {\n input v : m/s in [1, 100]\n let y = v * v\n check scales_as(y wrt v, 2)\n}\n",
        );
        let r = dataset(&m, &[&[10.0], &[20.0], &[40.0]]);
        let ev = relations(&m, &r, &pairs(&[(0, 1, 0), (1, 2, 0)]));
        assert!(
            ev.is_empty(),
            "declared exponent matches: {:?}",
            ev.first().map(|e| &e.detail)
        );

        // The mutant case: the model really scales like v^3 while the declaration says 2.
        let m3 = model(
            "model q \"\" {\n input v : m/s in [1, 100]\n let y = v * v * v\n check scales_as(y wrt v, 2)\n}\n",
        );
        let r3 = dataset(&m3, &[&[10.0], &[20.0], &[40.0]]);
        let ev3 = relations(&m3, &r3, &pairs(&[(0, 1, 0), (1, 2, 0)]));
        assert_eq!(ev3.len(), 1);
        assert!(
            (ev3[0].magnitude - 1.0).abs() < 0.05,
            "{}",
            ev3[0].magnitude
        );
    }

    #[test]
    fn a_conserved_trace_reports_its_drift() {
        let m = model(
            "model h \"\" {\n input dt in [0.001, 0.2]\n input steps : count in [1, 200]\n state x = 1.0\n state v = 0.0\n state e = 1.0\n loop steps {\n advance v = v - x * dt\n advance x = x + v * dt\n advance e = x * x + v * v\n watch e\n }\n let y = x\n check conserved(e, 0.001)\n}\n",
        );
        let r = dataset(&m, &[&[0.15, 50.0]]);
        let ev = relations(&m, &r, &Probes::new());
        assert_eq!(
            ev.len(),
            1,
            "explicit Euler does not conserve energy: {ev:?}"
        );
        assert!(ev[0].magnitude > 1.0);
        assert!(ev[0].detail.contains("drifted"), "{}", ev[0].detail);
    }

    #[test]
    fn a_lipschitz_bound_is_checked_in_units_per_unit() {
        let m = model(
            "model k \"\" {\n input x in [0, 10]\n let y = 5 * x\n check lipschitz(y wrt x, 2)\n}\n",
        );
        let r = dataset(&m, &[&[1.0], &[1.02]]);
        let ev = relations(&m, &r, &pairs(&[(0, 1, 0)]));
        assert_eq!(ev.len(), 1);
        assert!((ev[0].magnitude - 2.5).abs() < 1e-9, "{}", ev[0].magnitude);
    }

    #[test]
    fn a_symmetry_needs_a_deliberate_swap_and_says_so_without_one() {
        let m = model(
            "model z \"\" {\n input a in [0, 10]\n input b in [0, 10]\n let y = a + b\n check symmetric(y wrt (a, b))\n}\n",
        );
        let r = dataset(&m, &[&[1.0, 2.0], &[2.0, 1.0]]);
        // No swap probes recorded: the relation cannot be tested, and guessing would be dishonest.
        assert!(relations(&m, &r, &Probes::new()).is_empty());
        let mut p = Probes {
            pairs: Vec::new(),
            kinds: Vec::new(),
            axis_step: Vec::new(),
            swaps: vec![crate::pair::SwapProbe {
                base: 0,
                swapped: 1,
                pair: [0, 1],
            }],
        };
        assert!(
            relations(&m, &r, &p).is_empty(),
            "a real symmetry is not evidence"
        );
        let m2 = model(
            "model z \"\" {\n input a in [0, 10]\n input b in [0, 10]\n let y = 2 * a + b\n check symmetric(y wrt (a, b))\n}\n",
        );
        let r2 = dataset(&m2, &[&[1.0, 2.0], &[2.0, 1.0]]);
        p.swaps = vec![crate::pair::SwapProbe {
            base: 0,
            swapped: 1,
            pair: [0, 1],
        }];
        let ev = relations(&m2, &r2, &p);
        assert_eq!(ev.len(), 1, "an asymmetric model must be caught");
    }

    #[test]
    fn sensitivity_compares_a_probe_with_the_model_s_own_ordinary_probe() {
        let m = model("model g \"\" {\n input x in [0.1, 10]\n let y = x * x\n}\n");
        // y moves about ten times faster near x = 10 than near x = 1, and the measurement is that
        // difference, not the size of the numbers involved.
        let r = dataset(&m, &[&[1.0], &[1.01], &[10.0], &[10.01]]);
        let ev = sensitivity(&r, &pairs(&[(0, 1, 0), (2, 3, 0)]));
        assert_eq!(
            ev.len(),
            1,
            "{:?}",
            ev.iter().map(|e| &e.detail).collect::<Vec<_>>()
        );
        // Only the steeper pair is above ordinary: slopes 2.01 and 20.01, their median 11.01, so the
        // ratio for the steep one is 1.82 and the shallow one reports nothing.
        assert_eq!(ev[0].channel, Channel::Sensitivity);
        assert!(
            ev[0].magnitude > 0.6 && ev[0].magnitude < 1.0,
            "{}",
            ev[0].magnitude
        );
        assert_eq!(
            ev[0].observations,
            vec![2, 3],
            "the finding belongs to the steep probe, not to every sample that was run"
        );
    }

    #[test]
    fn a_uniformly_amplifying_linear_model_is_not_sensitive() {
        let m = model("model u \"\" {\n input x in [0.1, 10]\n let y = 7 * x\n}\n");
        let r = dataset(&m, &[&[2.0], &[2.02], &[9.0], &[9.02]]);
        assert!(sensitivity(&r, &pairs(&[(0, 1, 0), (2, 3, 0)])).is_empty());
    }

    #[test]
    fn an_output_crossing_zero_at_its_ordinary_rate_is_not_an_amplification() {
        // y = x - 2 has one rate everywhere and passes through zero on the way. Measuring a move
        // against the *instantaneous* value of the output turned the neighbourhood of x = 2 into a
        // gain of hundreds, which is what made a correct projectile suspicious wherever its range
        // vanished. Measuring it against the model's own ordinary slope does not.
        let m = model("model z \"\" {\n input x in [0, 5]\n let y = x - 2\n}\n");
        let r = dataset(&m, &[&[2.0], &[2.001], &[0.1], &[0.2]]);
        let ev = sensitivity(&r, &pairs(&[(0, 1, 0), (2, 3, 0)]));
        assert!(
            ev.is_empty(),
            "{:?}",
            ev.iter().map(|e| &e.detail).collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_frozen_reference_answers_exactly_what_the_campaign_wide_pass_answered() {
        // The single-point query exists so a minimiser can ask about coordinates the search never
        // sampled. If it judged differently from the pass that produced the finding, "this point is
        // still suspicious" would be a different claim in the two places, and a verified
        // counterexample would not be the report's own counterexample. So: same data, same reference,
        // same item, field for field.
        let m = model("model g \"\" {\n input x in [0.1, 10]\n let y = x * x\n}\n");
        let r = dataset(&m, &[&[1.0], &[1.01], &[10.0], &[10.01]]);
        let p = pairs(&[(0, 1, 0), (2, 3, 0)]);
        let wide = sensitivity(&r, &p);
        let reference = slope_reference(&r, &p);
        let one = sensitivity_at(
            r.by_id(2).expect("the steep base was recorded"),
            &[(0, r.by_id(3).expect("the steep probe was recorded"))],
            &reference,
        );
        assert_eq!(
            one.len(),
            1,
            "{:?}",
            one.iter().map(|e| &e.detail).collect::<Vec<_>>()
        );
        assert_eq!(
            one[0], wide[0],
            "the two paths disagree about the same probe"
        );
        // And the shallow probe stays silent under both, so this is equality and not both of them
        // reporting the loudest thing in the set.
        assert!(
            sensitivity_at(
                r.by_id(0).expect("the shallow base was recorded"),
                &[(0, r.by_id(1).expect("the shallow probe was recorded"))],
                &reference
            )
            .is_empty()
        );
    }

    #[test]
    fn a_point_that_was_never_probed_can_still_be_asked_about() {
        // The whole reason for freezing the reference: the candidate is not in the record set, so
        // nothing in the campaign describes it. x = 20 on y = x*x was never sampled, and its slope is
        // far above the ordinary one measured at x = 1.
        let m = model("model g \"\" {\n input x in [0.1, 30]\n let y = x * x\n}\n");
        let r = dataset(&m, &[&[1.0], &[1.01]]);
        let reference = slope_reference(&r, &pairs(&[(0, 1, 0)]));
        let base = record(&m, 100, &[20.0]);
        let moved = record(&m, 101, &[20.02]);
        let ev = sensitivity_at(&base, &[(0, &moved)], &reference);
        assert_eq!(
            ev.len(),
            1,
            "{:?}",
            ev.iter().map(|e| &e.detail).collect::<Vec<_>>()
        );
        assert_eq!(ev[0].observations, vec![100, 101]);
        // Slopes 40.02 near x = 20 against an ordinary 2.01: nineteen times the usual move.
        assert!(
            ev[0].magnitude > 15.0 && ev[0].magnitude < 20.0,
            "{}",
            ev[0].magnitude
        );
    }

    #[test]
    fn a_point_with_no_reference_about_it_is_not_claimed_to_be_sensitive() {
        // Nothing was probed, so there is no ordinary slope, so the channel has no answer -- and the
        // honest answer is silence rather than "this point is fine". A minimiser reads the same
        // absence as "cannot reduce", never as "reduced".
        let m = model("model g \"\" {\n input x in [0.1, 10]\n let y = x * x\n}\n");
        let empty = slope_reference(&Records::new(), &pairs(&[]));
        assert!(empty.is_empty());
        let base = record(&m, 0, &[5.0]);
        let moved = record(&m, 1, &[5.01]);
        assert!(sensitivity_at(&base, &[(0, &moved)], &empty).is_empty());
    }

    #[test]
    fn disagreement_between_two_paths_becomes_evidence() {
        let m = model("model e \"\" {\n input x in [0, 10]\n let y = x\n}\n");
        let ev = differential(&m, &[0, 1], &[1.0, 2.0], &[1.0, 2.5]);
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].channel, Channel::Differential);
        assert!((ev[0].magnitude - 0.2).abs() < 1e-12);
        // Bit-level differences below a couple of ulps are not disagreement.
        assert!(differential(&m, &[0], &[1.0], &[f64::from_bits(1.0f64.to_bits() + 1)]).is_empty());
    }

    #[test]
    fn a_non_finite_pair_is_left_to_the_divergence_channel() {
        let m = model("model f \"\" {\n input x in [0, 10]\n let y = x\n}\n");
        let both_inf = disagreement(
            &m,
            &[0],
            &[f64::INFINITY],
            &[f64::INFINITY],
            Channel::Numerical,
        );
        assert!(both_inf.is_empty());
        let one_inf = disagreement(&m, &[0], &[1.0], &[f64::INFINITY], Channel::Numerical);
        assert!(one_inf.is_empty());
    }

    #[test]
    fn a_smooth_surface_is_not_read_as_a_discontinuity_through_one_axis() {
        // y = a * b * c is smooth everywhere and its projection onto any single axis is a family of
        // parabolas of wildly different heights: sorted by `a` alone, consecutive samples differ in
        // b and c too, and the "biggest step divided by the median step" sensor measured that
        // projection and called it a jump. The sensor is gone; the sharp features a real experiment
        // has are caught by probes, which move one parameter at a time, and by the declared bounds.
        let m = model(
            "model p \"\" {\n input a in [0, 10]\n input b in [0, 10]\n input c in [0, 10]\n let y = a * b * c\n}\n",
        );
        let mut rows: Vec<Vec<f64>> = Vec::new();
        for i in 0..11 {
            for j in 0..11 {
                rows.push(vec![i as f64, j as f64, 3.0]);
            }
        }
        let refs: Vec<&[f64]> = rows.iter().map(std::vec::Vec::as_slice).collect();
        let r = dataset(&m, &refs);
        let ev = inferred_patterns(&m, &r, &Probes::new());
        assert!(
            !ev.iter().any(|e| e.detail.contains("jumps")),
            "{:?}",
            ev.iter().map(|e| &e.detail).collect::<Vec<_>>()
        );
        assert!(
            ev.iter().all(|e| !e.subject.key().contains("continuous")),
            "{:?}",
            ev.iter().map(|e| e.subject.key()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_claim_the_data_rejects_is_not_invented_and_then_broken() {
        // y = x * x over a domain either side of zero: half the probes fall, which is what a
        // parabola does, and a sensor that first fits "rising" to this data and then charges the
        // other half with violating it is reporting its own bad fit as a defect in the model.
        let m = model("model w \"\" {\n input x in [-3, 3]\n let y = x * x\n}\n");
        let xs: Vec<f64> = vec![-3.0, -2.0, -1.0, 0.0, 1.0, 2.0, 3.0];
        let refs: Vec<&[f64]> = xs.iter().map(std::slice::from_ref).collect();
        let r = dataset(&m, &refs);
        let mut p = Probes::new();
        for i in 0..xs.len() - 1 {
            p.pairs.push(Pair {
                base: i as u64,
                perturbed: (i + 1) as u64,
                axis: 0,
            });
            p.kinds.push(crate::pair::ProbeKind::Targeted);
        }
        p.axis_step = vec![0.01];
        let ev = inferred_patterns(&m, &r, &p);
        assert!(
            !ev.iter().any(|e| e.subject.key().contains("monotone_up")),
            "{:?}",
            ev.iter().map(|e| &e.detail).collect::<Vec<_>>()
        );
    }

    #[test]
    fn an_abrupt_reversal_inside_a_supported_claim_is_evidence() {
        // Rising, rising, rising, rising, and then one pair that falls by three times an ordinary
        // step: the relation is supported by the data everywhere except there, which is the shape of
        // something wrong rather than the shape of the model.
        let m = model("model w \"\" {\n input x in [1, 1.4]\n let y = x * x\n}\n");
        let r = dataset(&m, &[&[1.0], &[1.1], &[1.2], &[1.3], &[1.4]]);
        let p = pairs(&[(0, 1, 0), (1, 2, 0), (2, 3, 0), (3, 4, 0), (4, 0, 0)]);
        let ev = inferred_patterns(&m, &r, &p);
        let found = ev
            .iter()
            .find(|e| e.subject.key().contains("monotone_up"))
            .unwrap_or_else(|| {
                panic!(
                    "no monotonicity evidence: {:?}",
                    ev.iter().map(|e| &e.detail).collect::<Vec<_>>()
                )
            });
        assert!(found.magnitude > 1.0, "{}", found.magnitude);
        assert_eq!(
            found.observations,
            vec![0, 4],
            "named to the probe that reversed"
        );
    }

    #[test]
    fn a_smooth_monotone_model_produces_no_inferred_violation() {
        let m = model("model s \"\" {\n input x in [0.1, 3]\n let y = ln(x)\n}\n");
        let xs: Vec<f64> = (0..7).map(|i| 0.2 + i as f64 * 0.4).collect();
        let refs: Vec<&[f64]> = xs.iter().map(std::slice::from_ref).collect();
        let r = dataset(&m, &refs);
        let mut p = Probes::new();
        for i in 0..xs.len() - 1 {
            p.pairs.push(Pair {
                base: i as u64,
                perturbed: (i + 1) as u64,
                axis: 0,
            });
            p.kinds.push(crate::pair::ProbeKind::Targeted);
        }
        let ev = inferred_patterns(&m, &r, &p);
        assert!(
            !ev.iter().any(|e| e.subject.key().contains("monotone_up")),
            "{:?}",
            ev.iter().map(|e| &e.detail).collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_nan_is_a_fact_and_not_a_relative_measurement() {
        // Even if every other finding in the experiment is a NaN, that is not evidence that NaN is
        // the expected outcome: the strength must stay at full.
        let m = model(
            "model n \"\" {
 input x in [-4, 4]
 let y = sqrt(x)
 require finite(y)
}
",
        );
        let ev = divergence(&m, &record(&m, 0, &[-1.0]));
        assert_eq!(ev.len(), 1);
        assert!(ev[0].is_fixed(), "{:?}", ev[0]);
        assert_eq!(ev[0].strength, 1.0);
    }

    #[test]
    fn an_approximate_rule_uses_its_tolerance_as_the_scale() {
        let m =
            model("model a \"\" {\n input x in [0, 2]\n let y = x\n require y ~ 1 within 0.1\n}\n");
        let off = record(&m, 0, &[1.5]);
        let ev = constraints(&m, &off);
        assert_eq!(ev.len(), 1);
        assert!((ev[0].magnitude - 4.0).abs() < 1e-9, "{}", ev[0].magnitude);
        let near = record(&m, 1, &[1.05]);
        assert!(constraints(&m, &near).is_empty());
    }

    #[test]
    fn an_integer_axis_is_not_probed_for_behavioural_patterns() {
        let m = model("model i \"\" {\n input n : count in [1, 10]\n let y = n * 2\n}\n");
        let r = dataset(&m, &[&[1.0], &[2.0], &[3.0]]);
        let p = pairs(&[(0, 1, 0), (1, 2, 0)]);
        assert!(
            !inferred_patterns(&m, &r, &p)
                .iter()
                .any(|e| matches!(e.subject, Subject::Pattern { param: 0, .. }))
        );
    }

    #[test]
    fn probe_ids_are_collected_without_duplicates() {
        let p = pairs(&[(0, 1, 0), (1, 2, 0)]);
        let r = Records::new();
        let ids = observation_ids(&r, &p);
        assert_eq!(ids, vec![0, 1, 2]);
    }
}
