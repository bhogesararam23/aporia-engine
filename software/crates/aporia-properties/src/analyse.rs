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
            );
            // A `finite` rule is a boolean outcome, not a magnitude to compare against other
            // measurements of the same channel.
            out.push(match &c.kind {
                ConstraintKind::Finite { .. } => e.absolute(1.0),
                ConstraintKind::Cmp { .. } => e,
            });
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
#[expect(
    clippy::too_many_lines,
    reason = "one arm per relation kind, and each measurement is its own algorithm"
)]
#[must_use]
pub fn relations(model: &Model, records: &Records, probes: &Probes) -> Vec<Evidence> {
    let mut out = Vec::new();
    for r in &model.relations {
        let mut named: Option<Vec<u64>> = None;
        let (magnitude, detail) = match &r.kind {
            RelationKind::Monotone {
                out: o,
                param,
                direction,
            } => {
                let expect_increasing = *direction == aporia_ir::Direction::Increasing;
                let Some((violated, total, worst, offenders)) =
                    scan_monotone(records, probes, *o, *param, expect_increasing)
                else {
                    continue;
                };
                // A failed monotonicity claim is about the probes that broke it, so this arm names
                // them; the other relations are statements over the whole probe set.
                named = Some(offenders);
                if violated == 0 {
                    continue;
                }
                let m = (violated as f64 / total as f64) * worst;
                (
                    m,
                    format!(
                        "{o} failed to move as declared against p{param} in {violated} of {total} probes (worst relative excursion {worst:.3})"
                    ),
                )
            }
            RelationKind::ScalesAs {
                out: o,
                param,
                power,
            } => {
                let Some(slope) = log_log_slope(records, probes, *o, *param) else {
                    continue;
                };
                let m = (slope - power).abs();
                (
                    m,
                    format!("{o} scales like p{param}^{slope:.3}, declared {power}"),
                )
            }
            RelationKind::Symmetric { out: o, pair } => {
                let Some(d) = swap_distance(records, probes, *o, *pair) else {
                    continue;
                };
                if d <= 0.0 {
                    continue;
                }
                (
                    d,
                    format!(
                        "{o} changed by {d:.3} when p{} and p{} were swapped",
                        pair[0], pair[1]
                    ),
                )
            }
            RelationKind::Conserved { trace, tolerance } => {
                let Some(drift) = trace_drift(records, *trace) else {
                    // Nothing was measured, which is not the same as nothing being wrong.
                    out.push(missing_data(
                        Channel::Behavioral,
                        Subject::Relation(r.id),
                        records,
                        probes,
                        format!("conservation of t{trace} could not be evaluated: no trace values were recorded"),
                    ));
                    continue;
                };
                if drift <= *tolerance {
                    continue;
                }
                (
                    drift / tolerance.max(1e-15),
                    format!("trace t{trace} drifted {drift:.3e}, allowed {tolerance:.3e}"),
                )
            }
            RelationKind::Lipschitz {
                out: o,
                param,
                bound,
            } => {
                let Some(slope) = max_slope(records, probes, *o, *param) else {
                    continue;
                };
                if slope <= *bound {
                    continue;
                }
                (
                    slope / bound,
                    format!("{o} changed at {slope:.3} per unit of p{param}, bound {bound}"),
                )
            }
        };
        if magnitude.is_finite() && magnitude > 0.0 {
            out.push(Evidence::new(
                Channel::Behavioral,
                Subject::Relation(r.id),
                magnitude,
                named.unwrap_or_else(|| observation_ids(records, probes)),
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
            if let Some((violated, total, worst, offenders)) =
                scan_monotone(records, probes, o, param, true)
                && violated * 4 >= total
                && violated > 0
                && worst > 1e-6
            {
                out.push(Evidence::new(
                    Channel::Behavioral,
                    Subject::Pattern {
                        output: o,
                        param,
                        kind: PatternKind::Increasing,
                    },
                    (violated as f64 / total as f64) * worst,
                    offenders,
                    format!("o{o} stopped rising with p{param} in {violated} of {total} probes"),
                ));
            }
            if let Some(jump) = largest_jump(model, records, o, param) {
                out.push(Evidence::new(
                    Channel::Behavioral,
                    Subject::Pattern {
                        output: o,
                        param,
                        kind: PatternKind::Continuity,
                    },
                    jump.ratio,
                    jump.observations,
                    format!(
                        "{} jumps by {:.1}x its typical step along p{param}",
                        model.outputs[o as usize].name, jump.ratio
                    ),
                ));
            }
        }
    }
    out
}

/// How much an output amplified a deliberately small move in one parameter.
///
/// The magnitude is a *gain*: relative output change divided by relative input change. A gain near
/// one is ordinary — the output moved as much as the input did. The number that matters is the
/// excess, so `gain - 1` is what is reported, and a probe whose parameters sit at zero (where a
/// relative measure is meaningless) is skipped instead of guessed at.
#[must_use]
pub fn sensitivity(records: &Records, probes: &Probes) -> Vec<Evidence> {
    let mut out = Vec::new();
    for (pair, _) in probes.pairs.iter().zip(probes.kinds.iter()) {
        let (Some(base), Some(moved)) = (records.by_id(pair.base), records.by_id(pair.perturbed))
        else {
            continue;
        };
        for j in 0..base.y.len() {
            let (Some(x0), Some(x1), Some(y0), Some(y1)) = (
                base.x.get(pair.axis as usize).copied(),
                moved.x.get(pair.axis as usize).copied(),
                base.y.get(j).copied(),
                moved.y.get(j).copied(),
            ) else {
                continue;
            };
            if !y0.is_finite() || !y1.is_finite() || x0 == 0.0 || y0 == 0.0 {
                continue;
            }
            let dx = (x1 - x0).abs() / x0.abs();
            let dy = (y1 - y0).abs() / y0.abs();
            if dx <= 0.0 || !dx.is_finite() {
                continue;
            }
            let gain = dy / dx;
            if gain > 1.0 {
                out.push(Evidence::new(
                    Channel::Sensitivity,
                    Subject::LocalSlope { axis: pair.axis },
                    gain - 1.0,
                    vec![base.id, moved.id],
                    format!("o{j} gain {gain:.1}x along p{}", pair.axis),
                ));
            }
        }
    }
    out
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

/// Numerical channel, stronger form: the model against its double-double reference.
#[must_use]
pub fn against_reference(
    model: &Model,
    ids: &[u64],
    fast: &[f64],
    reference: &[f64],
) -> Vec<Evidence> {
    let mut out = disagreement(model, ids, fast, reference, Channel::Numerical);
    for e in &mut out {
        // The reference comparison is the more trustworthy of the two numerical signals, and the
        // magnitude needs to reflect that it is measured against a better answer rather than a
        // worse one. The scaling is a factor of f64 epsilon relative to the observed distance.
        e.magnitude *= 1.0 / 1e-9;
    }
    out
}

/// Differential channel: two execution paths that are supposed to compute the same thing.
#[must_use]
pub fn differential(model: &Model, ids: &[u64], a: &[f64], b: &[f64]) -> Vec<Evidence> {
    disagreement(model, ids, a, b, Channel::Differential)
}

// ------------------------------------------------------------------ measurements

/// Count and severity of monotonicity failures over the pairs on one axis.
fn scan_monotone(
    records: &Records,
    probes: &Probes,
    output: u16,
    param: u16,
    expect_increasing: bool,
) -> Option<(usize, usize, f64, Vec<u64>)> {
    let mut violated = 0;
    let mut total = 0;
    let mut worst = 0.0f64;
    let mut offenders: Vec<u64> = Vec::new();
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
        let rose = y1 > y0;
        if rose != expect_increasing && y1 != y0 {
            violated += 1;
            // The pairs that broke the pattern, not every pair that was examined: a relation that
            // fails in one corner should make that corner suspicious, not the whole space.
            offenders.extend([pair.base, pair.perturbed]);
            let scale = y0.abs().max(y1.abs()).max(1.0);
            worst = worst.max((y1 - y0).abs() / scale);
        }
    }
    if total == 0 {
        None
    } else {
        offenders.sort_unstable();
        offenders.dedup();
        Some((violated, total, worst, offenders))
    }
}

/// Slope of `log|y|` against `log|x|`, which is the exponent in a power law.
fn log_log_slope(records: &Records, probes: &Probes, output: u16, param: u16) -> Option<f64> {
    // The exponent of a power law is the slope of log|y| against log|x|. Each probe pair is short,
    // so the estimator is the mean of the local slopes rather than a regression over absolute
    // positions: it is less sensitive to how far apart the pairs happen to be.
    let mut sum = 0.0;
    let mut n = 0;
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
        sum += (y1.ln() - y0.ln()) / dx;
        n += 1;
    }
    if n == 0 {
        return None;
    }
    Some(sum / n as f64)
}

/// Relative distance between an execution and the same execution with two parameters swapped.
fn swap_distance(records: &Records, probes: &Probes, output: u16, pair: [u16; 2]) -> Option<f64> {
    let swap = probes.swaps.iter().find(|s| s.pair == pair)?;
    let (Some(a), Some(b)) = (records.by_id(swap.base), records.by_id(swap.swapped)) else {
        return None;
    };
    let (Some(y0), Some(y1)) = (a.y.get(output as usize), b.y.get(output as usize)) else {
        return None;
    };
    if !y0.is_finite() || !y1.is_finite() {
        return None;
    }
    Some(aporia_numerics::relative(*y0, *y1))
}

/// Largest relative departure of a traced quantity from its first value.
fn trace_drift(records: &Records, trace: u16) -> Option<f64> {
    let mut worst: f64 = 0.0;
    let mut seen = false;
    for o in &records.items {
        let Some(series) = o.traces.get(trace as usize) else {
            continue;
        };
        let Some(&first) = series.first() else {
            continue;
        };
        if first == 0.0 || !first.is_finite() {
            continue;
        }
        seen = true;
        for v in series {
            if v.is_finite() {
                worst = worst.max((v - first).abs() / first.abs());
            }
        }
    }
    if seen { Some(worst) } else { None }
}

/// Steepest observed slope of an output against a parameter, in output units per parameter unit.
fn max_slope(records: &Records, probes: &Probes, output: u16, param: u16) -> Option<f64> {
    let mut best = None;
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
        best = Some(best.map_or(s, |b: f64| b.max(s)));
    }
    best
}

/// How much the biggest step along an axis exceeds the median step: a cheap stand-in for a
/// discontinuity detector that does not need the function to be smooth anywhere.
/// A discontinuity candidate: the slope ratio and the two samples that produced it.
#[derive(Clone, Debug, PartialEq)]
struct Jump {
    ratio: f64,
    observations: Vec<u64>,
}

fn largest_jump(model: &Model, records: &Records, output: u16, param: u16) -> Option<Jump> {
    let DomainWidth(width) = width_of(model, param)?;
    let mut samples: Vec<(f64, f64, u64)> = records
        .items
        .iter()
        .filter_map(|o| {
            let x = *o.x.get(param as usize)?;
            let y = *o.y.get(output as usize)?;
            y.is_finite().then_some((x, y, o.id))
        })
        .collect();
    if samples.len() < 5 {
        return None;
    }
    samples.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    // The pair with the steepest secant as well as the ratio: a jump belongs *between* two
    // samples, and charging it to every observation in the experiment is what made a sharp feature
    // in one corner of the space look suspicious everywhere within reach of it.
    let mut steps: Vec<f64> = Vec::with_capacity(samples.len() - 1);
    let mut worst_pair: Option<(u64, u64)> = None;
    let mut best = 0.0f64;
    for w in samples.windows(2) {
        let dx = (w[1].0 - w[0].0) / width;
        let dy = (w[1].1 - w[0].1).abs();
        if dx > 1e-9 {
            let slope = dy / dx;
            steps.push(slope);
            if slope > best {
                best = slope;
                worst_pair = Some((w[0].2, w[1].2));
            }
        }
    }
    if steps.len() < 3 {
        return None;
    }
    let med = aporia_numerics::median_of(&steps);
    let max = steps.iter().fold(0.0f64, |a, b| a.max(*b));
    if med > 0.0 && max / med > 20.0 {
        let (base, moved) = worst_pair?;
        Some(Jump {
            ratio: max / med,
            observations: vec![base, moved],
        })
    } else {
        None
    }
}

struct DomainWidth(f64);

fn width_of(model: &Model, param: u16) -> Option<DomainWidth> {
    use aporia_ir::Domain;
    let p = model.params.get(param as usize)?;
    match &p.domain {
        Domain::Interval { lo, hi } if hi > lo => Some(DomainWidth(hi - lo)),
        _ => None,
    }
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
    fn sensitivity_reports_the_gain_above_one() {
        let m = model("model g \"\" {\n input x in [0.1, 10]\n let y = x * x\n}\n");
        let r = dataset(&m, &[&[1.0], &[1.01]]);
        let p = pairs(&[(0, 1, 0)]);
        let ev = sensitivity(&r, &p);
        // y doubles its relative change at about 2x gain near x = 1.
        assert!(!ev.is_empty());
        assert_eq!(ev[0].channel, Channel::Sensitivity);
        assert!(
            ev[0].magnitude > 0.5 && ev[0].magnitude < 2.0,
            "{}",
            ev[0].magnitude
        );
    }

    #[test]
    fn an_amplification_of_one_is_not_sensitivity() {
        let m = model("model u \"\" {\n input x in [0.1, 10]\n let y = x\n}\n");
        let r = dataset(&m, &[&[2.0], &[2.02]]);
        assert!(sensitivity(&r, &pairs(&[(0, 1, 0)])).is_empty());
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
    fn a_jump_is_attributed_to_the_two_samples_around_it() {
        // A sharp feature must not make the whole explored space suspicious. Charging one
        // discontinuity to every observation the experiment probed was what produced findings far
        // from the fault, so the continuity evidence names only the two samples either side of it.
        let m = model(
            "model p \"\" {
 input x in [0, 10]
 let y = 1 / (x - 5)
 require finite(y)
}
",
        );
        let xs: Vec<Vec<f64>> = [0.0, 1.0, 2.0, 3.0, 4.0, 4.9, 5.1, 6.0, 7.0, 8.0, 9.0, 10.0]
            .into_iter()
            .map(|x| vec![x])
            .collect();
        let r = dataset(
            &m,
            &xs.iter().map(std::vec::Vec::as_slice).collect::<Vec<_>>(),
        );
        let ev = inferred_patterns(&m, &r, &Probes::new());
        let jump = ev
            .iter()
            .find(|e| e.detail.contains("jumps"))
            .expect("the pole at x = 5 should read as a jump");
        assert!(
            jump.observations.len() <= 2,
            "one jump named {} observations: {:?}",
            jump.observations.len(),
            jump.observations
        );
        // And they are the samples either side of the pole, not the first two that were run.
        assert!(
            jump.observations
                .iter()
                .all(|id| (4.0..=6.0).contains(&r.items[*id as usize].x[0])),
            "named {:?}",
            jump.observations
        );
    }

    #[test]
    fn inferred_patterns_notice_a_loss_of_monotonicity_without_being_told() {
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
            ev.iter().any(|e| e.subject.key().contains("monotone_up")),
            "{:?}",
            ev.iter().map(|e| &e.detail).collect::<Vec<_>>()
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
