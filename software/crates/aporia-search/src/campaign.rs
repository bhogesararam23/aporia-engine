//! The campaign: budgeted, evidence-driven exploration of a model's parameter space.
//!
//! Two stages, deliberately.
//!
//! **Online.** Each round places one point by the acquisition policy, evaluates it and — every
//! `probe_every` rounds — the axis perturbations needed to measure sensitivity and local
//! behaviour. That evidence produces the risk score steering the next round, so the adaptivity is
//! driven by several channels at once rather than by one violation flag.
//!
//! **Retrospective.** When the budget is spent, the whole record set is analysed again: declared
//! relations over all probe pairs, inferred patterns along every axis, and the recorded numerical
//! comparisons. Risks are recomputed from that.
//!
//! The split exists because the stages answer different questions. Steering needs a number now;
//! reporting needs the most defensible number available. Running only the second would leave the
//! search blind; running only the first would overstate what a mid-campaign estimate supports. Both
//! risks are kept per point, so a reader can see how far refinement moved the answer.
//!
//! Every evaluation — base points and probes alike — is charged against the budget, and one driver
//! runs all three strategies, so a difference between strategies is a difference in where points were
//! placed and nothing else. Atlas labels come from the online risk the search actually acted on;
//! findings are ranked by the retrospective risk. That asymmetry is intentional and is stated in the
//! report rather than hidden in a merged number.

use crate::plan::{Acquisition, Family, Strategy, to_parameters};
use aporia_boundary::{Atlas, Label, Policy};
use aporia_evidence::{Calibrator, ChannelCorrelation, Evidence, EvidenceSet, fuse};
use aporia_ir::Model;
use aporia_numerics::Rng;
use aporia_properties::{Pair, Probes, constraints, divergence, numerical, sensitivity};
use aporia_runtime::interp::run as evaluate;
use aporia_runtime::observe::{Observation, Records};
use aporia_runtime::value::{ExecConfig, FpMode};

/// Everything a campaign needs to be told before it starts.
#[derive(Clone, Debug)]
pub struct Config {
    /// Model evaluations allowed, probes included.
    pub budget: u64,
    pub strategy: Strategy,
    pub seed: u64,
    /// Evaluate the full axis-probe star every nth base point. Probes are what make the sensitivity
    /// and behavioural channels available during the search, and they cost evaluations, so the rate
    /// is a declared trade rather than a detail. Zero disables probing entirely.
    pub probe_every: u64,
    pub policy: Policy,
    /// Refit the calibrator and the channel correlation every nth evaluation.
    pub calibrate_every: u64,
    /// Re-run the model in f32 every nth base point and record the comparison. Costs one evaluation
    /// per run, so it is off by default.
    pub numerical_every: u64,
    /// Relabel and refine the atlas every nth evaluation.
    pub refine_every: u64,
    pub max_steps_per_evaluation: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            budget: 4000,
            strategy: Strategy::Adaptive,
            seed: 0x6170_6f72_6961_0001,
            probe_every: 4,
            policy: Policy::default(),
            calibrate_every: 250,
            numerical_every: 0,
            refine_every: 64,
            max_steps_per_evaluation: 20_000_000,
        }
    }
}

impl Config {
    #[must_use]
    pub fn with_strategy(mut self, s: Strategy) -> Self {
        self.strategy = s;
        self
    }

    #[must_use]
    pub fn with_budget(mut self, b: u64) -> Self {
        self.budget = b;
        self
    }

    #[must_use]
    pub fn with_seed(mut self, s: u64) -> Self {
        self.seed = s;
        self
    }

    /// The configuration as an experiment manifest stores it, so a replay can name what it replays.
    #[must_use]
    pub fn to_json(&self) -> String {
        format!(
            concat!(
                "{{\"budget\":{},\"strategy\":\"{}\",\"seed\":{},\"probe_every\":{},",
                "\"calibrate_every\":{},\"numerical_every\":{},\"refine_every\":{},",
                "\"atlas\":{{\"suspicious_mean\":{},\"suspicious_peak\":{},\"min_samples\":{},",
                "\"min_channels\":{},\"max_depth\":{}}}}}"
            ),
            self.budget,
            self.strategy.name(),
            self.seed,
            self.probe_every,
            self.calibrate_every,
            self.numerical_every,
            self.refine_every,
            self.policy.suspicious_mean,
            self.policy.suspicious_peak,
            self.policy.min_samples,
            self.policy.min_channels,
            self.policy.max_depth,
        )
    }
}

/// One recorded decision. "Search decisions must be inspectable" is a design principle of this
/// project, and without this log a reviewer cannot tell an adaptive search from a story told about
/// one afterwards.
#[derive(Clone, Debug)]
pub struct Decision {
    pub evaluation: u64,
    pub family: Family,
    pub cell: u32,
    pub risk: f64,
    /// Information credited to the family at the next refinement: new suspicious cells, tightened
    /// bands and newly resolved volume, in the units `Acquisition::credit` consumes.
    pub gain: f64,
    pub x: Vec<f64>,
}

/// A region the atlas called suspicious.
#[derive(Clone, Debug)]
pub struct Finding {
    pub cell: u32,
    pub bounds: Vec<[f64; 2]>,
    /// The worst evaluation actually inside the cell. A finding has to be reproducible from
    /// something that ran, not from the cell's geometry.
    pub representative: Vec<f64>,
    pub observation: u64,
    pub online_risk: f64,
    pub final_risk: f64,
    pub samples: u32,
    pub evidence: Vec<Evidence>,
}

/// The result of a campaign.
#[derive(Debug)]
pub struct Campaign {
    pub model_name: String,
    pub config: Config,
    pub records: Records,
    pub probes: Probes,
    pub atlas: Atlas,
    /// Risk per record as the search saw it, in evaluation order.
    pub online_risk: Vec<f64>,
    /// Risk per record after the retrospective pass.
    pub final_risk: Vec<f64>,
    pub decisions: Vec<Decision>,
    pub evaluations: u64,
    pub instruction_steps: u64,
    pub calibrator: Calibrator,
    pub correlation: ChannelCorrelation,
    pub findings: Vec<Finding>,
    /// Record index of the first evaluation whose retrospective risk crossed the suspicious bar.
    ///
    /// Named for what it is: *flagged*, not *failed*. It says where the instrument first stopped
    /// being comfortable, which is a cost measurement — how long until the search has something to
    /// chase — and not a claim that the model is wrong at that point. The `sqrt` domain model is the
    /// case that made the distinction necessary: its output has an infinite slope exactly where it
    /// first becomes legal, so the loudest sensitivity reading in the whole experiment sits on the
    /// safe side of the boundary, and a field called `first_failure` would have reported that as a
    /// found fault. The benchmark's `first_true_failure`, measured against ground truth in
    /// `aporia-bench`, is the quantity the research question asks for.
    pub first_flagged: Option<usize>,
    pub evidence: Vec<Evidence>,
}

/// Run a campaign to the configured budget.
#[expect(
    clippy::too_many_lines,
    reason = "the driver is one straight pipeline: sample, evaluate, gather evidence, calibrate, \
              fuse, record, refine. Splitting it would spread one loop's state over six functions"
)]
#[must_use]
pub fn run(model: &Model, config: Config) -> Campaign {
    let mut rng = Rng::seeded(config.seed);
    let mut acquisition = Acquisition::default();
    let mut atlas = Atlas::new(model, config.policy);
    let mut records = Records::new();
    let mut probes = Probes::new();
    let mut decisions: Vec<Decision> = Vec::new();
    let mut online_risk: Vec<f64> = Vec::new();
    let mut log: Vec<EvidenceSet> = Vec::new();
    let mut numerical_pairs: Vec<(u64, Vec<f64>)> = Vec::new();
    let mut calibrator = Calibrator::new();
    let mut correlation = ChannelCorrelation::none();
    let mut evaluations = 0u64;
    let mut steps = 0u64;

    let allowed: Vec<Family> = match config.strategy {
        Strategy::Random => vec![Family::Random],
        Strategy::Stratified => vec![Family::Coverage],
        Strategy::Adaptive => Family::ADAPTIVE.to_vec(),
    };
    let exec = ExecConfig {
        fp: FpMode::F64,
        max_steps: config.max_steps_per_evaluation,
    };
    let exec_reduced = ExecConfig {
        fp: FpMode::F32,
        max_steps: config.max_steps_per_evaluation,
    };
    let mut last_credit = 0usize;

    while evaluations < config.budget {
        let round = evaluations;
        let family = acquisition.choose(&allowed, &mut rng);
        let x = choose_point(model, &atlas, family, &mut acquisition, &mut rng);
        let (obs, cost) = eval(model, &x, exec, evaluations);
        evaluations += 1;
        steps += cost;
        let id = records.push(obs);
        online_risk.push(0.0);
        let mut group = vec![id];

        if config.probe_every > 0 && round.is_multiple_of(config.probe_every) {
            for axis in 0..model.params.len() {
                if evaluations >= config.budget {
                    break;
                }
                let Some((moved, step)) = perturb(model, &x, axis) else {
                    continue;
                };
                let (o, c) = eval(model, &moved, exec, evaluations);
                evaluations += 1;
                steps += c;
                let point_risk = {
                    let mut fresh: Vec<Evidence> = Vec::new();
                    fresh.extend(constraints(model, &o));
                    fresh.extend(divergence(model, &o));
                    calibrator.apply(&mut fresh);
                    fuse(&EvidenceSet { items: fresh }, &correlation).score
                };
                let pid = records.push(o);
                online_risk.push(point_risk);
                probes.pairs.push(Pair {
                    base: id,
                    perturbed: pid,
                    axis: axis as u16,
                });
                probes.kinds.push(aporia_properties::ProbeKind::Targeted);
                if probes.axis_step.len() <= axis {
                    probes.axis_step.resize(axis + 1, 0.0);
                }
                probes.axis_step[axis] = step;
                group.push(pid);
            }
        }

        let probed_precision = config.numerical_every > 0 && round.is_multiple_of(config.numerical_every);
        if probed_precision && evaluations < config.budget {
            let (o, c) = eval(model, &x, exec_reduced, evaluations);
            evaluations += 1;
            steps += c;
            numerical_pairs.push((id, o.y.clone()));
        }

        // Online evidence: everything knowable from this point and its probes.
        let mut fresh: Vec<Evidence> = Vec::new();
        if let Some(o) = records.by_id(id) {
            fresh.extend(constraints(model, o));
            fresh.extend(divergence(model, o));
        }
        let subset = subset_records(&records, &group);
        let subset_probes = subset_probes(&probes, &group);
        fresh.extend(sensitivity(&subset, &subset_probes));
        calibrator.apply(&mut fresh);
        let risk = fuse(
            &EvidenceSet {
                items: fresh.clone(),
            },
            &correlation,
        )
        .score;
        // What the label of this cell will be judged against: which sensors were actually applied
        // here, as distinct from which ones had something to say. A clean point has nothing to say,
        // and a policy that demands a channel have spoken before calling a cell trusted can then
        // never call anything trusted.
        let mut measured = 1u8 << aporia_evidence::Channel::Physical.index();
        if group.len() > 1 {
            measured |= 1 << aporia_evidence::Channel::Sensitivity.index();
        }
        if probed_precision {
            measured |= 1 << aporia_evidence::Channel::Numerical.index();
        }
        atlas.record_measured(&x, risk, channels_mask(&fresh), measured);
        // The base point carries the group's risk, which is the number the search actually acted
        // on. Probes sit next to it with their own pointwise risk, so `online_risk[i]` always
        // describes `records.items[i]`.
        let slot = records
            .items
            .iter()
            .position(|o| o.id == id)
            .unwrap_or(online_risk.len());
        if slot < online_risk.len() {
            online_risk[slot] = risk;
        } else {
            online_risk.push(risk);
        }
        if !fresh.is_empty() {
            log.push(EvidenceSet { items: fresh });
        }

        decisions.push(Decision {
            evaluation: round,
            family,
            cell: atlas.locate(&x),
            risk,
            gain: 0.0,
            x,
        });

        if config.calibrate_every > 0 && evaluations.is_multiple_of(config.calibrate_every) {
            calibrator = Calibrator::fit_from(&log);
            correlation = ChannelCorrelation::estimate(&log);
        }
        if config.refine_every > 0 && evaluations.is_multiple_of(config.refine_every) {
            atlas.relabel();
            atlas.refine();
            atlas.relabel();
            credit_rounds(
                &mut acquisition,
                &mut decisions,
                &atlas,
                model,
                &mut last_credit,
            );
        }
    }

    atlas.relabel();
    atlas.refine();
    atlas.relabel();

    // ----------------------------------------------------------- retrospective pass
    let mut final_log = retrospective(model, &records, &probes, &numerical_pairs);
    if !log.is_empty() {
        final_log.extend(std::mem::take(&mut log));
    }
    let calibrator = Calibrator::fit_from(&final_log);
    let correlation = ChannelCorrelation::estimate(&final_log);
    // Risk per record after the whole record set is available, together with the mask of channels
    // that were applied to it, so the atlas can be labelled from the report's evidence rather than
    // from the search's.
    let probed: std::collections::HashSet<u64> = probes
        .pairs
        .iter()
        .flat_map(|p| [p.base, p.perturbed])
        .collect();
    let precise: std::collections::HashSet<u64> = numerical_pairs.iter().map(|(id, _)| *id).collect();
    let mut final_risk: Vec<f64> = Vec::with_capacity(records.items.len());
    let mut points: Vec<aporia_boundary::Point> = Vec::with_capacity(records.items.len());
    for o in &records.items {
        let items = own_evidence(&final_log, o.id);
        let mut measured = 1u8 << aporia_evidence::Channel::Physical.index();
        if probed.contains(&o.id) {
            measured |= 1 << aporia_evidence::Channel::Behavioral.index();
            measured |= 1 << aporia_evidence::Channel::Sensitivity.index();
        }
        if precise.contains(&o.id) {
            measured |= 1 << aporia_evidence::Channel::Numerical.index();
        }
        let mut scored = items.clone();
        calibrator.apply(&mut scored);
        let risk = fuse(
            &EvidenceSet { items: scored },
            &correlation,
        )
        .score;
        final_risk.push(risk);
        points.push(aporia_boundary::Point {
            observation: o.id,
            x: o.x.clone(),
            risk,
            channels: channels_mask(&items),
            measured,
        });
    }

    // Label the finished atlas with the finished evidence, then let it settle: a split after this
    // point re-arranges measurements that were already paid for, so resolution really does follow
    // evidence rather than following the budget. `refine` now requires four samples in a cell before
    // it will cut it, so this terminates on its own — each split halves the samples available to the
    // children, and the loop is bounded regardless.
    atlas.remeasure(&points);
    for _ in 0..8 {
        if atlas.refine() == 0 {
            break;
        }
    }
    atlas.relabel();

    let first_flagged = final_risk
        .iter()
        .position(|r| *r >= config.policy.suspicious_mean);
    let findings = collect_findings(
        &atlas,
        &records,
        &final_log,
        &final_risk,
        &online_risk,
        &calibrator,
    );

    Campaign {
        model_name: model.name.clone(),
        config,
        records,
        probes,
        atlas,
        online_risk,
        final_risk,
        decisions,
        evaluations,
        instruction_steps: steps,
        calibrator,
        correlation,
        findings,
        first_flagged,
        evidence: final_log.into_iter().flat_map(|s| s.items).collect(),
    }
}

// --------------------------------------------------------------------------- parts

fn eval(model: &Model, x: &[f64], cfg: ExecConfig, id: u64) -> (Observation, u64) {
    let outcome = evaluate(model, x, cfg);
    let cost = outcome.steps;
    (Observation::new(id, x.to_vec(), &outcome), cost)
}

/// Place one point, in the model's own units.
fn choose_point(
    model: &Model,
    atlas: &Atlas,
    family: Family,
    acquisition: &mut Acquisition,
    rng: &mut Rng,
) -> Vec<f64> {
    let dims = model.params.len();
    match family {
        Family::Random | Family::Sensitivity => to_parameters(
            model,
            &(0..dims).map(|_| rng.next_f64()).collect::<Vec<f64>>(),
        ),
        Family::Coverage => to_parameters(model, &acquisition.next_unit(dims)),
        other => {
            let Some(cell) = target_cell(atlas, other) else {
                // Nothing to target yet, so the first rounds are necessarily coarse.
                return to_parameters(
                    model,
                    &(0..dims).map(|_| rng.next_f64()).collect::<Vec<f64>>(),
                );
            };
            let root = atlas.root().clone();
            let unit: Vec<f64> = (0..dims)
                .map(|axis| {
                    let [lo, hi] = cell.bounds[axis];
                    let [rlo, rhi] = root.bounds[axis];
                    let span = rhi - rlo;
                    if span <= 0.0 {
                        return 0.5;
                    }
                    // Stay away from the cell faces so the point lands where it is meant to land and
                    // a repeat of the same coordinate is unlikely.
                    let inset = (hi - lo) * 0.15;
                    ((lo + inset) - rlo) / span
                        + ((hi - inset) - (lo + inset)) / span * rng.next_f64()
                })
                .collect();
            to_parameters(model, &unit)
        }
    }
}

fn target_cell(atlas: &Atlas, family: Family) -> Option<aporia_boundary::Cell> {
    let known: Vec<u32> = atlas
        .leaf_ids()
        .iter()
        .copied()
        .filter(|id| atlas.cell(*id).is_some_and(|c| c.samples > 0))
        .collect();
    let better = |a: u32, b: u32| {
        rank(atlas, a)
            .partial_cmp(&rank(atlas, b))
            .unwrap_or(std::cmp::Ordering::Equal)
    };
    let pick = match family {
        Family::Boundary => known
            .iter()
            .copied()
            .filter(|id| atlas.cell(*id).is_some_and(|c| c.label != Label::Unknown))
            .max_by(|a, b| better(*a, *b)),
        Family::Uncertainty => known
            .iter()
            .copied()
            .min_by_key(|id| atlas.cell(*id).map_or(0, |c| c.samples)),
        Family::Contradiction => known
            .iter()
            .copied()
            .filter(|id| atlas.cell(*id).is_some_and(|c| c.channels.count_ones() > 1))
            .max_by(|a, b| better(*a, *b)),
        Family::Sensitivity => known.iter().copied().max_by(|a, b| {
            atlas
                .cell(*a)
                .map_or(0.0, |c| c.risk_max)
                .partial_cmp(&atlas.cell(*b).map_or(0.0, |c| c.risk_max))
                .unwrap_or(std::cmp::Ordering::Equal)
        }),
        _ => None,
    };
    pick.and_then(|id| atlas.cell(id).cloned())
}

fn rank(atlas: &Atlas, id: u32) -> f64 {
    atlas.cell(id).map_or(0.0, |c| c.risk_max + c.mean_risk())
}

/// A perturbed point along one axis, plus the relative step actually used. A slope without its
/// denominator is not a number anyone can check, so the step is returned rather than assumed.
#[must_use]
pub fn perturb(model: &Model, x: &[f64], axis: usize) -> Option<(Vec<f64>, f64)> {
    use aporia_ir::Domain;
    let p = model.params.get(axis)?;
    let mut y = x.to_vec();
    let (lo, hi) = match &p.domain {
        Domain::Interval { lo, hi } => (*lo, *hi),
        Domain::Choices(v) if v.len() > 1 => (v[0], v[v.len() - 1]),
        Domain::Choices(_) => return None,
    };
    let width = hi - lo;
    if !width.is_finite() || width <= 0.0 {
        return None;
    }
    let step = width * 1e-4;
    let base = y[axis];
    // Move away from the nearer bound so a probe never leaves the domain.
    let delta = if base - lo < step {
        step
    } else if hi - base < step {
        -step
    } else {
        step
    };
    y[axis] = base + delta;
    Some((y, (delta / width).abs()))
}

fn subset_records(records: &Records, ids: &[u64]) -> Records {
    let mut out = Records::new();
    for id in ids {
        if let Some(o) = records.by_id(*id) {
            out.push(o.clone());
        }
    }
    out
}

fn subset_probes(probes: &Probes, ids: &[u64]) -> Probes {
    let keep: Vec<usize> = probes
        .pairs
        .iter()
        .enumerate()
        .filter(|(_, p)| ids.contains(&p.base) || ids.contains(&p.perturbed))
        .map(|(i, _)| i)
        .collect();
    Probes {
        pairs: keep.iter().map(|i| probes.pairs[*i]).collect(),
        kinds: keep.iter().map(|i| probes.kinds[*i]).collect(),
        axis_step: probes.axis_step.clone(),
        swaps: probes.swaps.clone(),
    }
}

fn channels_mask(items: &[Evidence]) -> u8 {
    items
        .iter()
        .fold(0u8, |acc, e| acc | (1 << e.channel.index() as u8))
}

fn retrospective(
    model: &Model,
    records: &Records,
    probes: &Probes,
    numerical_pairs: &[(u64, Vec<f64>)],
) -> Vec<EvidenceSet> {
    let mut pointwise = EvidenceSet::new();
    for o in &records.items {
        pointwise.extend(constraints(model, o));
        pointwise.extend(divergence(model, o));
    }
    let mut relational = EvidenceSet::new();
    relational.extend(aporia_properties::relations(model, records, probes));
    let mut inferred = EvidenceSet::new();
    inferred.extend(aporia_properties::inferred_patterns(model, records, probes));
    let mut sens = EvidenceSet::new();
    sens.extend(sensitivity(records, probes));
    let mut num = EvidenceSet::new();
    for (id, reduced) in numerical_pairs {
        let Some(base) = records.by_id(*id) else {
            continue;
        };
        num.extend(numerical(
            model,
            &[*id, base.id + 1_000_000],
            &base.y,
            reduced,
        ));
    }
    [pointwise, relational, inferred, sens, num]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect()
}

fn own_evidence(sets: &[EvidenceSet], id: u64) -> Vec<Evidence> {
    let mut out = Vec::new();
    for s in sets {
        for e in &s.items {
            if e.observations.contains(&id) {
                out.push(e.clone());
            }
        }
    }
    out
}

/// Credit every round since the last refinement with the information the atlas gained in between.
///
/// Crude on purpose: newly suspicious cells, tightened bands and newly resolved volume, all of which
/// a reader can recompute from the recorded atlas. A subtler credit would be harder to argue about
/// in a paper and easier to get quietly wrong.
fn credit_rounds(
    acquisition: &mut Acquisition,
    decisions: &mut [Decision],
    atlas: &Atlas,
    model: &Model,
    marker: &mut usize,
) {
    let suspicious = atlas
        .leaf_ids()
        .iter()
        .filter(|id| {
            atlas
                .cell(**id)
                .is_some_and(|c| c.label == Label::Suspicious)
        })
        .count() as f64;
    let tightness: f64 = atlas
        .bands()
        .iter()
        .map(|b| 1.0 / (1.0 + b.width() / axis_width(model, b.axis as usize)))
        .sum();
    let resolved = atlas.coverage().resolved();
    let gain = suspicious * 0.2 + tightness * 0.5 + resolved;
    let from = *marker;
    let window = decisions.len() - from;
    if window == 0 {
        return;
    }
    let per = gain / window as f64;
    for d in &mut decisions[from..] {
        d.gain = per;
        acquisition.credit(d.family, per);
    }
    *marker = decisions.len();
}

fn axis_width(model: &Model, axis: usize) -> f64 {
    use aporia_ir::Domain;
    model.params.get(axis).map_or(1.0, |p| match &p.domain {
        Domain::Interval { lo, hi } => (hi - lo).max(1e-12),
        Domain::Choices(v) if v.len() > 1 => v[v.len() - 1] - v[0],
        // A single choice, or none, has no extent to normalise against.
        Domain::Choices(_) => 1.0,
    })
}

fn collect_findings(
    atlas: &Atlas,
    records: &Records,
    sets: &[EvidenceSet],
    final_risk: &[f64],
    online_risk: &[f64],
    calibrator: &Calibrator,
) -> Vec<Finding> {
    let mut out = Vec::new();
    for id in atlas.leaf_ids() {
        let Some(cell) = atlas.cell(*id) else {
            continue;
        };
        if cell.label != Label::Suspicious {
            continue;
        }
        let mut best: Option<(usize, f64)> = None;
        for (i, o) in records.items.iter().enumerate() {
            if !cell.contains(&o.x) {
                continue;
            }
            let r = final_risk.get(i).copied().unwrap_or(0.0);
            if best.is_none_or(|(_, br)| r > br) {
                best = Some((i, r));
            }
        }
        let Some((index, risk)) = best else { continue };
        let obs = &records.items[index];
        let mut items = own_evidence(sets, obs.id);
        calibrator.apply(&mut items);
        let online = online_risk.get(index).copied().unwrap_or(0.0);
        out.push(Finding {
            cell: *id,
            bounds: cell.bounds.clone(),
            representative: obs.x.clone(),
            observation: obs.id,
            online_risk: online,
            final_risk: risk,
            samples: cell.samples,
            evidence: items,
        });
    }
    out.sort_by(|a, b| {
        b.final_risk
            .partial_cmp(&a.final_risk)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use aporia_dsl::lower::compile;
    use aporia_evidence::Channel;

    fn model(text: &str) -> Model {
        let c = compile("t.ap", text);
        assert!(!c.diagnostics.has_errors(), "{}\n{text}", c.diagnostics);
        c.model
    }

    #[test]
    fn a_clean_model_finds_nothing() {
        let m = model("model ok \"\" {\n input x in [0, 1]\n let y = x * x\n require y >= 0\n}\n");
        let c = run(
            &m,
            Config {
                budget: 300,
                ..Config::default()
            },
        );
        assert!(
            c.findings.is_empty(),
            "{:?}",
            c.findings.first().map(|f| &f.evidence)
        );
        assert!(c.evaluations >= 250, "spent only {}", c.evaluations);
        assert_eq!(c.first_flagged, None);
    }

    #[test]
    fn the_reported_atlas_carries_the_report_risk_not_the_search_risk() {
        // A declared `check` is tested over the whole record set, so it does not exist while the
        // search is running. It has to reach the map the reader is handed: before
        // `Atlas::remeasure`, cell labels came from online evidence only, so a model whose only
        // problem is a failed declared relation could finish with an entirely TRUSTED atlas sitting
        // next to an evidence list that said otherwise.
        let m = model(
            "model q \"\" {\n input x in [-1, 1]\n let y = x * x\n check monotone_up(y wrt x)\n}\n",
        );
        let c = run(
            &m,
            Config {
                budget: 240,
                ..Config::default()
            },
        );
        let reported = c.atlas.cells.iter().fold(0.0f64, |a, cell| a.max(cell.risk_max));
        let expected = c.final_risk.iter().fold(0.0f64, |a, r| a.max(*r));
        assert!(
            (reported - expected).abs() < 1e-12,
            "the atlas maxes at {reported} while the report maxes at {expected}"
        );
        assert!(
            c.evidence
                .iter()
                .any(|e| e.channel == aporia_evidence::Channel::Behavioral),
            "this example stopped exercising the retrospective pass"
        );
    }

    #[test]
    fn a_sharp_failure_is_found_and_the_band_contains_it() {
        let m = model(
            "model b \"\" {\n input x in [0.0, 1.0]\n let y = sqrt(x - 0.4)\n require finite(y)\n}\n",
        );
        let c = run(
            &m,
            Config {
                budget: 1200,
                ..Config::default()
            },
        );
        assert!(!c.findings.is_empty(), "coverage {:?}", c.atlas.coverage());
        let f = &c.findings[0];
        assert!(
            f.representative[0] < 0.4,
            "representative {:?}",
            f.representative
        );
        let band = c
            .atlas
            .bands()
            .into_iter()
            .find(|b| b.axis == 0)
            .expect("a band along the only axis");
        assert!(band.contains(0.4), "band {band:?} misses the transition");
    }

    #[test]
    fn every_strategy_spends_the_budget_it_is_given() {
        let m = model(
            "model c \"\" {\n input x in [0, 1]\n input y in [0, 1]\n let z = x / (y + 0.001)\n require finite(z)\n}\n",
        );
        for s in [Strategy::Random, Strategy::Stratified, Strategy::Adaptive] {
            let c = run(
                &m,
                Config {
                    budget: 240,
                    strategy: s,
                    ..Config::default()
                },
            );
            assert!(c.evaluations <= 240, "{s:?} overspent: {}", c.evaluations);
            assert!(c.evaluations >= 200, "{s:?} underspent: {}", c.evaluations);
            assert_eq!(c.records.len() as u64, c.evaluations);
            assert_eq!(c.online_risk.len(), c.records.len());
            assert_eq!(c.final_risk.len(), c.records.len());
            if s != Strategy::Random {
                assert!(!c.probes.pairs.is_empty(), "{s:?} never probed");
            }
        }
    }

    #[test]
    fn decisions_are_logged_and_name_the_family_that_acted() {
        let m =
            model("model d \"\" {\n input x in [0, 1]\n let y = 1 / x\n require finite(y)\n}\n");
        let c = run(
            &m,
            Config {
                budget: 160,
                probe_every: 0,
                ..Config::default()
            },
        );
        assert!(!c.decisions.is_empty());
        assert!(c.decisions.iter().all(|d| (0.0..=1.0).contains(&d.risk)));
        let families: Vec<Family> = c.decisions.iter().map(|d| d.family).collect();
        assert!(families.contains(&Family::Coverage), "{families:?}");
    }

    #[test]
    fn a_replay_of_a_seed_reproduces_the_whole_campaign() {
        let m = model(
            "model s \"\" {\n input x in [0, 1]\n input y in [0, 1]\n let z = sqrt(x - y)\n require finite(z)\n}\n",
        );
        let cfg = || Config {
            budget: 400,
            seed: 17,
            ..Config::default()
        };
        let a = run(&m, cfg());
        let b = run(&m, cfg());
        assert_eq!(a.evaluations, b.evaluations);
        assert_eq!(a.records.len(), b.records.len());
        let bits = |v: &[f64]| v.iter().map(|f| f.to_bits()).collect::<Vec<u64>>();
        for (x, y) in a.records.items.iter().zip(b.records.items.iter()) {
            assert_eq!(bits(&x.x), bits(&y.x));
            assert_eq!(bits(&x.y), bits(&y.y));
        }
        assert_eq!(bits(&a.online_risk), bits(&b.online_risk));
        assert_eq!(a.atlas.coverage().cells, b.atlas.coverage().cells);
    }

    #[test]
    fn probes_stay_inside_the_declared_domain() {
        let m = model("model p \"\" {\n input x in [0, 1]\n let y = x\n}\n");
        for x in [0.0, 1e-9, 0.5, 1.0] {
            let (moved, step) = perturb(&m, &[x], 0).expect("a probe on the only axis");
            assert!((0.0..=1.0).contains(&moved[0]), "{x} -> {moved:?}");
            assert!(step > 0.0 && step < 1.0, "{step}");
        }
    }

    #[test]
    fn turning_probing_off_costs_exactly_one_evaluation_per_round() {
        let m = model(
            "model n \"\" {\n input x in [0, 1]\n input y in [0, 1]\n let z = x + y\n require finite(z)\n}\n",
        );
        let c = run(
            &m,
            Config {
                budget: 100,
                probe_every: 0,
                numerical_every: 0,
                strategy: Strategy::Random,
                ..Config::default()
            },
        );
        assert_eq!(c.evaluations, 100);
        assert!(c.probes.pairs.is_empty());
    }

    #[test]
    fn the_numerical_channel_only_appears_when_it_was_paid_for() {
        let m = model("model q \"\" {\n input x in [0, 1]\n let y = x + 1e-8\n}\n");
        let off = run(
            &m,
            Config {
                budget: 120,
                numerical_every: 0,
                ..Config::default()
            },
        );
        assert!(!off.evidence.iter().any(|e| e.channel == Channel::Numerical));
        let on = run(
            &m,
            Config {
                budget: 120,
                numerical_every: 1,
                ..Config::default()
            },
        );
        assert!(
            on.evidence.iter().any(|e| e.channel == Channel::Numerical),
            "f32 vs f64 produced nothing"
        );
    }

    #[test]
    fn a_config_describes_itself_exactly_enough_to_replay() {
        let text = Config::default().to_json();
        assert!(text.contains("\"strategy\":\"adaptive\""), "{text}");
        assert!(text.contains("suspicious_mean"), "{text}");
        assert!(text.contains("\"budget\":4000"), "{text}");
    }

    #[test]
    fn first_flagged_is_an_index_and_the_unsafe_side_is_flagged_too() {
        let m = model(
            "model f \"\" {\n input x in [0, 1]\n let y = sqrt(x - 0.5)\n require finite(y)\n}\n",
        );
        let c = run(
            &m,
            Config {
                budget: 400,
                strategy: Strategy::Stratified,
                ..Config::default()
            },
        );
        let bar = c.config.policy.suspicious_mean;
        let i = c
            .first_flagged
            .expect("a deterministic sweep crosses the boundary");
        assert!(i < c.records.len(), "index {i} out of range");
        assert!(
            c.final_risk[i] >= bar,
            "flagged record {} has risk {}, below the bar",
            c.records.items[i].id,
            c.final_risk[i]
        );
        // The claim that matters is not which record was flagged first — an infinite slope at the
        // edge of a square root is the loudest signal in this model and it sits on the legal side —
        // but that the illegal side is flagged at all.
        assert!(
            c.final_risk
                .iter()
                .enumerate()
                .any(|(j, r)| *r >= bar && c.records.items[j].x[0] < 0.5),
            "nothing on the NaN side of the boundary was flagged"
        );
    }

    #[test]
    fn calibration_and_correlation_are_recorded_for_the_report() {
        let m = model(
            "model k \"\" {\n input x in [0, 1]\n let y = sqrt(x - 0.5)\n require finite(y)\n}\n",
        );
        let c = run(
            &m,
            Config {
                budget: 300,
                ..Config::default()
            },
        );
        let text = c.calibrator.describe();
        assert!(text.contains('B') && text.contains('P'), "{text}");
        assert!(c.correlation.samples() >= 3 || c.evidence.is_empty());
    }
}
