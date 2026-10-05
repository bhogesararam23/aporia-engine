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
use aporia_boundary::{Atlas, FACE_EPS, Label, Policy};
use aporia_evidence::{Calibrator, ChannelCorrelation, Evidence, EvidenceSet, fuse};
use aporia_ir::{Model, RelationKind};
use aporia_numerics::Rng;
use aporia_properties::{Pair, Probes, constraints, divergence, numerical, sensitivity};
use aporia_runtime::observe::{Observation, Records};
use aporia_runtime::value::{ExecConfig, FpMode};
use aporia_runtime::{Executor, Interp};
use aporia_store::{StoredFinding, label_text};

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
    /// Re-run the model through the independent double-double reference path every nth base point,
    /// and record where the two implementations disagree.
    ///
    /// This is the Differential channel: a different program, in a different crate, with deliberately
    /// duplicated arithmetic, computing the same answer (`aporia_numerics::reference`). The f32
    /// comparison above asks "does the answer depend on the precision I chose"; this one asks "do two
    /// implementations agree", which is a different question and is the evidence the spec's fifth
    /// channel names.
    ///
    /// It costs one reference evaluation per firing, and the reference path is several times slower
    /// than the runtime, so the rate is a declared cost. Off by default for library callers; the
    /// benchmark sets a rate, because a channel that never fires is not a channel.
    pub differential_every: u64,
    /// Re-run a base point with each declared symmetric parameter pair swapped, every nth base
    /// point. Costs one evaluation per declared pair per firing, and is charged.
    ///
    /// Zero for a model that declares no symmetry costs nothing, because there is nothing to swap:
    /// the rate is only paid by models that ask for the check.
    pub symmetric_every: u64,
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
            differential_every: 0,
            symmetric_every: 1,
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

    /// The configuration as data, for whatever records the run: an archive's manifest, a results
    /// document, a report.
    ///
    /// This used to return a `String` that every caller immediately parsed back into a value, which
    /// meant a config could only be stored by way of a format that might fail -- and both callers wrote
    /// `unwrap_or(Json::Null)` on that parse, so a failure would have archived an empty configuration
    /// about a run that had all of one. Building the value directly removes the fallible step, and with
    /// it the invented default.
    ///
    /// `max_steps_per_evaluation` is in here because it is part of what the run was: a campaign cut
    /// off at two million steps per point is a different experiment from one allowed twenty. It
    /// matches the archived execution guard (`aporia_store`'s `exec` block), which is the same number
    /// seen from the other end.
    #[must_use]
    pub fn json(&self) -> aporia_store::Json {
        use aporia_store::Json;
        Json::object(vec![
            ("budget", Json::count(self.budget)),
            ("strategy", Json::text(self.strategy.name())),
            ("seed", Json::count(self.seed)),
            ("probe_every", Json::count(self.probe_every)),
            ("calibrate_every", Json::count(self.calibrate_every)),
            ("numerical_every", Json::count(self.numerical_every)),
            ("differential_every", Json::count(self.differential_every)),
            ("symmetric_every", Json::count(self.symmetric_every)),
            ("refine_every", Json::count(self.refine_every)),
            (
                "max_steps_per_evaluation",
                Json::count(self.max_steps_per_evaluation),
            ),
            (
                "atlas",
                Json::object(vec![
                    ("suspicious_mean", Json::number(self.policy.suspicious_mean)),
                    ("suspicious_peak", Json::number(self.policy.suspicious_peak)),
                    (
                        "min_samples",
                        Json::count(u64::from(self.policy.min_samples)),
                    ),
                    (
                        "min_channels",
                        Json::count(u64::from(self.policy.min_channels)),
                    ),
                    (
                        "suspicious_channels",
                        Json::count(u64::from(self.policy.suspicious_channels)),
                    ),
                    ("max_depth", Json::count(u64::from(self.policy.max_depth))),
                ]),
            ),
        ])
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
    /// The worst constituent cell, which is what the risk and the representative belong to.
    pub cell: u32,
    /// Every leaf folded into this finding. A merged claim still names the cells it was built from,
    /// so it can be checked against the evaluations that produced them.
    pub cells: Vec<u32>,
    /// The loud claims of this finding, used as the merge key (see `merge_findings`). Empty in a
    /// finished report: it is a bookkeeping field, not something a reader should mistake for evidence.
    pub signature: Vec<String>,
    pub bounds: Vec<[f64; 2]>,
    /// The worst evaluation actually inside the cell. A finding has to be reproducible from
    /// something that ran, not from the cell's geometry.
    pub representative: Vec<f64>,
    pub observation: u64,
    pub online_risk: f64,
    pub final_risk: f64,
    /// Evaluations across every constituent cell.
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

impl Campaign {
    /// The campaign's findings in the shape `aporia-store` writes to an archive.
    ///
    /// This is the one place that knows how a `Finding` becomes a stored record, because two callers
    /// used to assemble the same eleven fields themselves and only the benchmark's version could be
    /// trusted to agree with the command line's. Everything except `case` is a fact the campaign
    /// already holds: the label is the atlas's own verdict on the worst constituent cell, and a cell
    /// the atlas no longer has is reported as `UNKNOWN` rather than dropped, because a finding that
    /// cannot say what region it came from is not replayable.
    ///
    /// `case` is a closure because that field is the one thing a caller has to decide for itself. A
    /// minimised counterexample is a separate claim with its own oracle question (decisions 0012 and
    /// 0020), so a command that merely ran a campaign answers `None`, and a harness that verified a
    /// reduced case passes the description it verified.
    #[must_use]
    pub fn stored_findings(
        &self,
        mut case: impl FnMut(&Finding) -> Option<String>,
    ) -> Vec<StoredFinding> {
        self.findings
            .iter()
            .enumerate()
            .map(|(i, f)| StoredFinding {
                index: i as u64,
                cell: f.cell,
                bounds: f.bounds.clone(),
                representative: f.representative.clone(),
                observation: f.observation,
                online_risk: f.online_risk,
                final_risk: f.final_risk,
                samples: f.samples as u64,
                label: self
                    .atlas
                    .cell(f.cell)
                    .map_or("UNKNOWN", |c| label_text(c.label))
                    .to_string(),
                case: case(f),
                evidence: f.evidence.clone(),
                raw_evidence: Vec::new(),
            })
            .collect()
    }
}

/// Run a campaign to the configured budget, evaluating the model with the scalar interpreter.
///
/// This is [`run_with`] with the default execution path. A campaign over a program APORIA did not
/// parse calls `run_with` with its own [`Executor`] instead; nothing else about the driver differs,
/// which is the point — an external program has to earn the same sampling, the same evidence
/// channels and the same budget accounting or its results are not comparable with anyone else's.
#[must_use]
pub fn run(model: &Model, config: Config) -> Campaign {
    run_with(model, config, &mut Interp)
}

/// Run a campaign to the configured budget against an arbitrary execution path.
#[expect(
    clippy::too_many_lines,
    reason = "the driver is one straight pipeline: sample, evaluate, gather evidence, calibrate, \
              fuse, record, refine. Splitting it would spread one loop's state over six functions"
)]
#[must_use]
pub fn run_with(model: &Model, config: Config, engine: &mut dyn Executor) -> Campaign {
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
    let at_f64 = ExecConfig {
        fp: FpMode::F64,
        max_steps: config.max_steps_per_evaluation,
    };
    let at_f32 = ExecConfig {
        fp: FpMode::F32,
        max_steps: config.max_steps_per_evaluation,
    };
    let mut last_credit = 0usize;
    // The parameter pairs the author declared symmetric, collected once. A model that declares none
    // pays nothing for this machinery, which is why the rate below is on by default.
    let symmetric_pairs: Vec<[u16; 2]> = model
        .relations
        .iter()
        .filter_map(|r| match &r.kind {
            RelationKind::Symmetric { pair, .. } => Some(*pair),
            _ => None,
        })
        .collect();

    while evaluations < config.budget {
        let round = evaluations;
        let family = acquisition.choose(&allowed, &mut rng);
        let x = choose_point(model, &atlas, family, &mut acquisition, &mut rng);
        let (obs, cost) = eval(engine, model, &x, at_f64, evaluations);
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
                let (o, c) = eval(engine, model, &moved, at_f64, evaluations);
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

        // Declared symmetry is only testable by running the swapped point, so it is done deliberately
        // rather than left to two samples happening to land mirrored.
        if config.symmetric_every > 0
            && !symmetric_pairs.is_empty()
            && round.is_multiple_of(config.symmetric_every)
        {
            for &[a, b] in &symmetric_pairs {
                if evaluations >= config.budget {
                    break;
                }
                let mut swapped = x.clone();
                let (va, vb) = (swapped[a as usize], swapped[b as usize]);
                let (Some(da), Some(db)) = (domain_of(model, a), domain_of(model, b)) else {
                    continue;
                };
                // Interchangeable values need the same interval. The lowering has already rejected a
                // swap between different kinds of quantity, but the same kind is not the same box.
                if (da.0 - db.0).abs() > FACE_EPS || (da.1 - db.1).abs() > FACE_EPS {
                    continue;
                }
                // And each value has to land inside the other's declared domain, so the swapped point
                // is a legitimate execution of this model rather than a point nobody declared.
                if !(va >= db.0 && va <= db.1 && vb >= da.0 && vb <= da.1) {
                    continue;
                }
                swapped[a as usize] = vb;
                swapped[b as usize] = va;
                let (o, c) = eval(engine, model, &swapped, at_f64, evaluations);
                evaluations += 1;
                steps += c;
                let sid = records.push(o);
                online_risk.push(0.0);
                probes.swaps.push(aporia_properties::SwapProbe {
                    base: id,
                    swapped: sid,
                    pair: [a, b],
                });
            }
        }

        // Only worth buying if a second precision is a second path. An executor that ignores the
        // mode would return the same numbers, and recording that as agreement would be reporting the
        // repeatability of one implementation under the name of a conditioning signal.
        let probed_precision = config.numerical_every > 0
            && engine.varies_with_precision()
            && round.is_multiple_of(config.numerical_every);
        if probed_precision && evaluations < config.budget {
            let (o, c) = eval(engine, model, &x, at_f32, evaluations);
            evaluations += 1;
            steps += c;
            numerical_pairs.push((id, o.y.clone()));
        }

        // Online evidence: everything knowable from this point and its probes.
        let mut fresh: Vec<Evidence> = Vec::new();
        if let Some(o) = records.by_id(id) {
            fresh.extend(constraints(model, o));
            fresh.extend(divergence(model, o));
            // Differential channel: the same point through an independent implementation. The
            // reference evaluator lives in another crate, repeats the arithmetic on purpose, and
            // works in double-double, so agreement between it and the runtime is evidence about the
            // model rather than evidence about shared code. Charged as the evaluation it costs: a
            // channel that gets something for free makes every later cost comparison dishonest.
            if config.differential_every > 0
                && engine.has_reference_path()
                && round.is_multiple_of(config.differential_every)
                && evaluations < config.budget
            {
                let reference = aporia_numerics::reference::evaluate(
                    model,
                    &x,
                    config.max_steps_per_evaluation,
                );
                evaluations += 1;
                steps += reference.steps;
                fresh.extend(aporia_properties::against_reference(
                    model,
                    &[id],
                    &o.y,
                    &reference.values(),
                ));
            }
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
        atlas.record_point(aporia_boundary::Point {
            observation: 0,
            x: x.clone(),
            risk,
            channels: channels_mask(&fresh),
            measured,
            fact: fresh.iter().any(aporia_evidence::Evidence::is_fixed),
        });
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
    let precise: std::collections::HashSet<u64> =
        numerical_pairs.iter().map(|(id, _)| *id).collect();
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
        let risk = fuse(&EvidenceSet { items: scored }, &correlation).score;
        final_risk.push(risk);
        points.push(aporia_boundary::Point {
            observation: o.id,
            x: o.x.clone(),
            risk,
            channels: channels_mask(&items),
            measured,
            fact: items.iter().any(aporia_evidence::Evidence::is_fixed),
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

fn eval(
    engine: &mut dyn Executor,
    model: &Model,
    x: &[f64],
    cfg: ExecConfig,
    id: u64,
) -> (Observation, u64) {
    let outcome = engine.execute(model, x, cfg);
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

/// The interval a parameter is declared over, if it has one. A discrete choice set has no
/// interchangeable coordinates, so a symmetry across it is not testable by swapping values.
fn domain_of(model: &Model, param: u16) -> Option<(f64, f64)> {
    use aporia_ir::Domain;
    match &model.params.get(param as usize)?.domain {
        Domain::Interval { lo, hi } => Some((*lo, *hi)),
        Domain::Choices(_) => None,
    }
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

/// The subject keys that say what a finding is *about*: the claims loud enough to have made the cell
/// suspicious. Two findings that agree on the evaluation and disagree on this are different faults.
fn signature(items: &[Evidence]) -> Vec<String> {
    let mut keys: Vec<String> = items
        .iter()
        .filter(|e| e.strength >= 0.5)
        .map(|e| format!("{}|{}", e.channel.code(), e.subject.key()))
        .collect();
    keys.sort();
    keys.dedup();
    keys
}

/// Merge findings that describe one region rather than one cell.
///
/// Leaves of a bisection tile the space, so a single fault covering a quarter of the domain produces
/// hundreds of suspicious cells and — measured — 0.677 of all findings on a run re-described a region
/// the report had already given. That is not a discovery rate, it is a tiling artifact, and it made
/// the count of findings a cost rather than a conclusion.
///
/// Two findings merge when they carry the same signature (the same loud claims) and their boxes
/// abut along exactly one axis while agreeing on every other, which is precisely the condition for
/// the union to still be a box. A finding that covers an L-shape would be a bounds list describing a
/// region nothing measured, so it is left as two findings.
///
/// The constituent cells are kept, because a merged report must still be checkable against the
/// evaluations it came from: `cells` names every leaf folded in, `cell` and the risk fields stay the
/// worst constituent's, and `samples` is the sum, so a reader can see how many executions a merged
/// claim rests on.
fn merge_findings(mut findings: Vec<Finding>) -> Vec<Finding> {
    loop {
        let mut merge = None;
        'outer: for i in 0..findings.len() {
            for j in (i + 1)..findings.len() {
                if findings[i].signature != findings[j].signature {
                    continue;
                }
                if let Some(union) = abutting_union(&findings[i].bounds, &findings[j].bounds) {
                    merge = Some((i, j, union));
                    break 'outer;
                }
            }
        }
        let Some((i, j, union)) = merge else { break };
        let (a, b) = (findings.remove(i), findings.remove(j - 1));
        let worst_first = a.final_risk >= b.final_risk;
        let (mut keep, other) = if worst_first { (a, b) } else { (b, a) };
        keep.bounds = union;
        keep.samples += other.samples;
        keep.cells.extend(other.cells);
        keep.cells.push(other.cell);
        keep.cells.sort_unstable();
        keep.cells.dedup();
        findings.push(keep);
    }
    findings
}

/// The box two cells form when they abut on one axis and coincide on all the others, if they do.
fn abutting_union(a: &[[f64; 2]], b: &[[f64; 2]]) -> Option<Vec<[f64; 2]>> {
    if a.len() != b.len() {
        return None;
    }
    let mut touches = 0;
    let mut out = Vec::with_capacity(a.len());
    for k in 0..a.len() {
        let ([lo0, hi0], [lo1, hi1]) = (a[k], b[k]);
        let same = (lo0 - lo1).abs() <= FACE_EPS && (hi0 - hi1).abs() <= FACE_EPS;
        let abuts = (hi0 - lo1).abs() <= FACE_EPS || (hi1 - lo0).abs() <= FACE_EPS;
        if same {
            out.push([lo0, hi0]);
        } else if abuts {
            out.push([lo0.min(lo1), hi0.max(hi1)]);
            touches += 1;
        } else {
            return None;
        }
    }
    (touches == 1).then_some(out)
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
            cells: vec![*id],
            signature: signature(&items),
            bounds: cell.bounds.clone(),
            representative: obs.x.clone(),
            observation: obs.id,
            online_risk: online,
            final_risk: risk,
            samples: cell.samples,
            evidence: items,
        });
    }
    let mut merged = merge_findings(out);
    merged.sort_by(|a, b| {
        b.final_risk
            .partial_cmp(&a.final_risk)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use aporia_dsl::lower::compile;
    use aporia_evidence::Channel;
    use aporia_runtime::interp::Outcome;
    use aporia_store::Json;

    fn model(text: &str) -> Model {
        let c = compile("t.ap", text);
        assert!(!c.diagnostics.has_errors(), "{}\n{text}", c.diagnostics);
        c.model
    }

    fn finding(cell: u32, bounds: &[[f64; 2]], keys: &[&str], risk: f64) -> Finding {
        Finding {
            cell,
            cells: vec![cell],
            signature: keys.iter().map(|k| (*k).to_string()).collect(),
            bounds: bounds.to_vec(),
            representative: vec![bounds[0][0]],
            observation: cell as u64,
            online_risk: risk,
            final_risk: risk,
            samples: 1,
            evidence: Vec::new(),
        }
    }

    #[test]
    fn abutting_findings_about_the_same_claims_become_one_region() {
        let four = vec![
            finding(1, &[[0.0, 0.25]], &["P|require0"], 0.9),
            finding(2, &[[0.25, 0.5]], &["P|require0"], 0.8),
            finding(3, &[[0.5, 0.75]], &["P|require0"], 0.7),
            // A different claim is a different fault, even next door.
            finding(4, &[[0.75, 1.0]], &["S|o0~p0:slope"], 0.6),
        ];
        let merged = merge_findings(four);
        assert_eq!(
            merged.len(),
            2,
            "{:?}",
            merged.iter().map(|f| &f.bounds).collect::<Vec<_>>()
        );
        let wide = merged
            .iter()
            .find(|f| f.bounds[0] == [0.0, 0.75])
            .expect("the three halves of one claim");
        assert_eq!(
            wide.cells,
            vec![1, 2, 3],
            "every constituent is still named"
        );
        assert_eq!(
            wide.samples, 3,
            "the merged claim rests on three evaluations"
        );
        assert_eq!(wide.cell, 1, "the worst constituent keeps its identity");
        assert_eq!(wide.final_risk, 0.9);
    }

    #[test]
    fn an_l_shape_is_not_reported_as_one_box() {
        // Two cells that touch on different axes in different places union to an L. Reporting that
        // as a bounds list would describe a region nothing was measured in, so it stays two.
        let two = vec![
            finding(1, &[[0.0, 0.5], [0.0, 1.0]], &["P|require0"], 0.9),
            finding(2, &[[0.5, 1.0], [1.0, 2.0]], &["P|require0"], 0.8),
        ];
        assert_eq!(merge_findings(two).len(), 2);
    }

    #[test]
    fn a_declared_symmetry_is_swapped_executed_and_leaves_no_evidence() {
        // The whole point of this test is the wiring, not the arithmetic: `check symmetric` existed
        // as a relation kind, as a lowering, and as a measurement function, and no campaign ever
        // produced a `SwapProbe`, so the claim could be declared and silently never tested.
        let m = model(
            "model s \"\" {\n input a in [0, 10]\n input b in [0, 10]\n let y = a + b\n check symmetric(y wrt (a, b))\n}\n",
        );
        let c = run(
            &m,
            Config {
                budget: 120,
                ..Config::default()
            },
        );
        assert!(
            !c.probes.swaps.is_empty(),
            "the campaign recorded no swap probes at all"
        );
        for s in &c.probes.swaps {
            assert_eq!(s.pair, [0, 1]);
            let (Some(base), Some(sw)) = (c.records.by_id(s.base), c.records.by_id(s.swapped))
            else {
                panic!("a swap names executions that were never recorded: {s:?}");
            };
            // The swapped point is a real execution of real coordinates, mirrored on both axes.
            assert_eq!(base.x[0], sw.x[1], "the values were not exchanged");
            assert_eq!(base.x[1], sw.x[0]);
            for v in &sw.x {
                assert!(
                    (0.0..=10.0).contains(v),
                    "swapped point left the domain: {v}"
                );
            }
        }
        // a + b is symmetric, so testing it must produce nothing rather than something plausible.
        assert!(
            !c.evidence.iter().any(|e| e.detail.contains("were swapped")),
            "{:?}",
            c.evidence
                .iter()
                .filter(|e| e.detail.contains("swapped"))
                .map(|e| &e.detail)
                .collect::<Vec<_>>()
        );
        // Not even a small one: a `B` scale that is not the default is the calibration readout's way
        // of saying the Behavioral channel saw magnitude on a model whose declared symmetry holds.
        let behavioural: Vec<&Evidence> = c
            .evidence
            .iter()
            .filter(|e| e.channel == Channel::Behavioral)
            .collect();
        assert!(
            behavioural.is_empty(),
            "{:?}",
            behavioural
                .iter()
                .map(|e| (e.subject.key(), e.magnitude, e.detail.clone()))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn round_off_in_a_true_symmetry_is_measured_and_stays_round_off() {
        // `check symmetric` is a statement about the mathematics, but a swap compares two
        // *executions*. `0.3 * turns_a * turns_b` is left-associated, so the base runs (0.3*a)*b and
        // the swap runs (0.3*b)*a, and those round differently at isolated points: measured here at a
        // budget of 640, 159 of the 160 swaps came back bit-identical and one came back 1.1677e-16
        // apart. The property that matters is that a last-ulp artifact is reported as a last-ulp
        // artifact rather than becoming suspicion, so this test pins both halves of that.
        let m = model(
            "model coupled_coils \"\" {\n  input turns_a : count in [1, 40]\n  input turns_b : count in [1, 40]\n  let self_term = 0.5 * (turns_a * turns_a + turns_b * turns_b)\n  let mutual = 0.3 * turns_a * turns_b\n  let reactance = self_term + mutual\n  require reactance > 0\n  check symmetric(reactance wrt (turns_a, turns_b))\n}\n",
        );
        let c = run(
            &m,
            Config {
                budget: 640,
                ..Config::default()
            },
        );
        assert!(
            !c.probes.swaps.is_empty(),
            "the control was never actually swapped"
        );
        let behavioural: Vec<&Evidence> = c
            .evidence
            .iter()
            .filter(|e| e.channel == Channel::Behavioral)
            .collect();
        for e in &behavioural {
            assert!(
                e.magnitude < 1e-9,
                "a symmetry artifact was reported at {:e}, which is not round-off",
                e.magnitude
            );
        }
        assert!(
            c.findings.is_empty(),
            "{:?}",
            c.findings.first().map(|f| &f.evidence)
        );
        assert_eq!(
            c.atlas
                .leaf_ids()
                .iter()
                .filter(|id| c
                    .atlas
                    .cell(**id)
                    .is_some_and(|x| x.label == Label::Suspicious))
                .count(),
            0,
            "round-off was promoted to suspicion"
        );
    }

    #[test]
    fn an_asymmetric_model_is_caught_by_the_swapped_execution() {
        let m = model(
            "model t \"\" {\n input a in [0, 10]\n input b in [0, 10]\n let y = 2 * a + b\n check symmetric(y wrt (a, b))\n}\n",
        );
        let c = run(
            &m,
            Config {
                budget: 120,
                ..Config::default()
            },
        );
        let hits: Vec<&Evidence> = c
            .evidence
            .iter()
            .filter(|e| e.detail.contains("were swapped"))
            .collect();
        assert!(!hits.is_empty(), "the asymmetry was never measured");
        for h in &hits {
            assert_eq!(h.channel, aporia_evidence::Channel::Behavioral);
            assert_eq!(h.observations.len(), 2, "a swap compares two executions");
            assert!(
                c.records.by_id(h.observations[0]).is_some()
                    && c.records.by_id(h.observations[1]).is_some(),
                "the evidence names executions that never ran"
            );
        }
        // One item per swapped base point, not one for the whole experiment: the first commit of
        // this path produced a single item and hid every other point where the relation held.
        assert_eq!(hits.len(), c.probes.swaps.len());
    }

    #[test]
    fn swapped_executions_are_paid_for_out_of_the_budget() {
        let m = model(
            "model u \"\" {\n input a in [0, 10]\n input b in [0, 10]\n let y = a * b\n check symmetric(y wrt (a, b))\n}\n",
        );
        let off = run(
            &m,
            Config {
                budget: 120,
                symmetric_every: 0,
                probe_every: 0,
                calibrate_every: 1_000,
                refine_every: 1_000,
                ..Config::default()
            },
        );
        let on = run(
            &m,
            Config {
                budget: 120,
                symmetric_every: 1,
                probe_every: 0,
                calibrate_every: 1_000,
                refine_every: 1_000,
                ..Config::default()
            },
        );
        assert!(
            off.probes.swaps.is_empty(),
            "a rate of 0 must not swap anything"
        );
        // The first version of this assertion checked `on.evaluations > off.evaluations`, which told
        // me nothing about charging: the budget is a cap, so both runs stop at 120 and the extra
        // evaluations are paid for by *displacing* other work rather than by exceeding the total.
        // The observable consequence of that is the decision log -- one entry per base point -- and
        // a swapped base point leaves less budget for the next one.
        assert_eq!(on.evaluations, off.evaluations, "both runs fill the cap");
        assert!(
            on.decisions.len() < off.decisions.len(),
            "swaps cost nothing: {} base points with them, {} without",
            on.decisions.len(),
            off.decisions.len()
        );
        assert_eq!(
            on.records.len() as u64,
            on.evaluations,
            "every charged evaluation has to be a recorded execution"
        );
        assert!(
            on.evaluations <= 120,
            "the budget was overspent: {}",
            on.evaluations
        );
    }

    #[test]
    fn a_model_that_declares_no_symmetry_pays_for_none() {
        let m = model(
            "model v \"\" {\n input a in [0, 10]\n input b in [0, 10]\n let y = a + b\n require y >= 0\n}\n",
        );
        let c = run(
            &m,
            Config {
                budget: 120,
                probe_every: 0,
                ..Config::default()
            },
        );
        assert!(c.probes.swaps.is_empty(), "nothing was declared");
        // 120 evaluations, one record each: the symmetry machinery is not charged to a model that
        // does not ask for it.
        assert_eq!(c.records.len() as u64, c.evaluations);
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
        let reported = c
            .atlas
            .cells
            .iter()
            .fold(0.0f64, |a, cell| a.max(cell.risk_max));
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
        let config = Config::default().json();
        assert_eq!(
            config.get("strategy").and_then(Json::as_str),
            Some("adaptive"),
            "{config:?}"
        );
        assert!(
            config
                .get("atlas")
                .is_some_and(|a| a.get("suspicious_mean").is_some()),
            "the labelling thresholds are part of what the run was: {config:?}"
        );
        assert_eq!(
            config.get("budget").and_then(Json::as_u64),
            Some(4000),
            "{config:?}"
        );
        // The step guard is the field this record existed without, and the one an archive's `exec`
        // block repeats: a run cut off at two million steps is not the run allowed twenty.
        assert_eq!(
            config
                .get("max_steps_per_evaluation")
                .and_then(Json::as_u64),
            Some(20_000_000),
            "{config:?}"
        );
        // Every rate that costs evaluations is named, or a reader cannot tell a channel that found
        // nothing from a channel that was never asked.
        for rate in [
            "probe_every",
            "calibrate_every",
            "numerical_every",
            "differential_every",
            "symmetric_every",
            "refine_every",
        ] {
            assert!(
                config.get(rate).is_some(),
                "{rate} is missing from the recorded configuration"
            );
        }
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

    /// A campaign that flags something, at a budget small enough for three tests to share.
    fn flagged() -> Campaign {
        let m = model(
            "model s \"\" {\n input x in [0, 1]\n let y = sqrt(x - 0.5)\n require finite(y)\n}\n",
        );
        let c = run(
            &m,
            Config {
                budget: 400,
                strategy: Strategy::Stratified,
                ..Config::default()
            },
        );
        assert!(
            !c.findings.is_empty(),
            "the model the storage tests rely on stopped producing findings"
        );
        c
    }

    #[test]
    fn stored_findings_carry_what_the_campaign_knows_and_nothing_else() {
        let c = flagged();
        let stored = c.stored_findings(|_| None);
        assert_eq!(stored.len(), c.findings.len());
        for (i, (s, f)) in stored.iter().zip(&c.findings).enumerate() {
            assert_eq!(s.index, i as u64, "the index is the report position");
            assert_eq!(s.cell, f.cell);
            assert_eq!(s.bounds, f.bounds);
            assert_eq!(s.representative, f.representative);
            assert_eq!(s.observation, f.observation);
            assert_eq!(s.online_risk, f.online_risk);
            assert_eq!(s.final_risk, f.final_risk);
            assert_eq!(s.samples, f.samples as u64);
            assert!(
                s.case.is_none(),
                "a caller that ran no minimiser stores no case"
            );
            // The label is read back out of the atlas rather than re-derived, which is what keeps the
            // archive's claim about a region in step with the map that region was drawn on.
            let label = c
                .atlas
                .cell(f.cell)
                .map_or("UNKNOWN", |cell| label_text(cell.label));
            assert_eq!(s.label, label);
        }
    }

    #[test]
    fn a_finding_whose_cell_the_atlas_lost_is_stored_as_unknown_not_dropped() {
        let mut c = flagged();
        c.findings
            .push(finding(u32::MAX, &[[0.0, 1.0]], &["phantom"], 0.8));
        let stored = c.stored_findings(|_| None);
        let last = stored.last().expect("the fabricated finding is stored");
        assert_eq!(last.label, "UNKNOWN");
        assert_eq!(last.cell, u32::MAX);
    }

    #[test]
    fn two_callers_of_the_construction_differ_only_in_the_case_they_verified() {
        // The shape of this test is the point: a report and a benchmark harness used to write these
        // fields separately, and the only difference that is supposed to exist between them is the
        // counterexample each was entitled to claim.
        let c = flagged();
        let plain = c.stored_findings(|_| None);
        let reduced = c.stored_findings(|f| Some(format!("x = {}", f.representative[0])));
        assert_eq!(plain.len(), reduced.len());
        for (a, b) in plain.iter().zip(&reduced) {
            assert_eq!(a.case, None);
            assert!(b.case.is_some(), "the caller asked for a case per finding");
            let mut agreed = b.clone();
            agreed.case = None;
            assert_eq!(
                *a, agreed,
                "one campaign produced two different stored findings for cell c{}",
                a.cell
            );
        }
    }

    /// An execution path that answers a different question than the model's A-IR does. `y = x - 7`
    /// is what this "program" computes; the model text says `x * x`, which is why the pair of tests
    /// below is informative rather than decorative.
    struct Linear {
        calls: usize,
        varies: bool,
        reference: bool,
    }

    impl Executor for Linear {
        fn execute(&mut self, model: &Model, x: &[f64], _cfg: ExecConfig) -> Outcome {
            self.calls += 1;
            Outcome {
                outputs: x
                    .iter()
                    .map(|v| v - 7.0)
                    .take(model.outputs.len())
                    .collect::<Vec<_>>(),
                traces: vec![Vec::new(); model.traces.len()],
                flags: aporia_runtime::value::Flags::default(),
                steps: 0,
                rule_values: Vec::new(),
            }
        }

        fn varies_with_precision(&self) -> bool {
            self.varies
        }

        fn has_reference_path(&self) -> bool {
            self.reference
        }
    }

    fn square_model() -> Model {
        model("model s \"\" {\n input x in [0, 10]\n let y = x * x\n require y >= 0\n}\n")
    }

    #[test]
    fn the_default_path_is_bit_for_bit_the_interpreter() {
        // Every published number was produced by `run`. If delegating to `run_with` changed the
        // sampling or the accounting by one evaluation, the ladder would silently describe a
        // different driver, so this is checked as equality of the report's own quantities.
        let m = square_model();
        let cfg = Config {
            budget: 240,
            numerical_every: 7,
            differential_every: 11,
            ..Config::default()
        };
        let a = run(&m, cfg.clone());
        let b = run_with(&m, cfg, &mut Interp);
        assert_eq!(a.evaluations, b.evaluations);
        assert_eq!(a.instruction_steps, b.instruction_steps);
        assert_eq!(a.records.len(), b.records.len());
        assert_eq!(a.decisions.len(), b.decisions.len());
        assert_eq!(a.final_risk, b.final_risk);
        assert_eq!(a.findings.len(), b.findings.len());
    }

    #[test]
    fn a_foreign_execution_path_changes_what_the_atlas_says() {
        // The seam is real only if the driver follows it. The same model text, the same seed and the
        // same budget: interpreted, `y = x*x` is never negative and the campaign reports nothing;
        // executed by a program that computes `x - 7`, seven tenths of the domain violates the
        // author's own rule and the campaign has to say so. If the driver had slipped back to the
        // interpreter, this test would fail by reporting a clean map.
        let m = square_model();
        let interpreted = run(
            &m,
            Config {
                budget: 240,
                ..Config::default()
            },
        );
        assert!(
            interpreted.findings.is_empty(),
            "the interpreter path changed"
        );
        let mut engine = Linear {
            calls: 0,
            varies: false,
            reference: false,
        };
        let adapted = run_with(
            &m,
            Config {
                budget: 240,
                ..Config::default()
            },
            &mut engine,
        );
        assert!(
            !adapted.findings.is_empty(),
            "the campaign ignored the executor it was given"
        );
        assert!(
            adapted
                .evidence
                .iter()
                .any(|e| e.channel == Channel::Physical && e.detail.contains("violated")),
            "{:?}",
            adapted
                .evidence
                .iter()
                .map(|e| &e.detail)
                .collect::<Vec<_>>()
        );
        // Every charged evaluation went through the adapter, and only through it: the interpreter was
        // never asked, which is what makes the reported cost honest.
        assert_eq!(engine.calls as u64, adapted.evaluations);
        assert_eq!(adapted.records.len() as u64, adapted.evaluations);
        // A program that counts its work in APORIA's units is the only source of `steps`, and this
        // one does not have any, so the campaign must report 0 rather than borrow the interpreter's.
        assert_eq!(adapted.instruction_steps, 0);
    }

    #[test]
    fn probes_that_cannot_answer_are_not_bought() {
        // Numerical and Differential cost evaluations, and an executor that cannot vary its precision
        // or produce an independent implementation has nothing to say through them. With the rates
        // turned up, an A-IR path spends budget on the extra runs and a foreign path does not: the
        // difference is exactly the probes, and the campaign's decision log shows it.
        let m = square_model();
        let mut quiet = Linear {
            calls: 0,
            varies: false,
            reference: false,
        };
        let foreign = run_with(
            &m,
            Config {
                budget: 200,
                numerical_every: 1,
                differential_every: 1,
                probe_every: 0,
                symmetric_every: 0,
                ..Config::default()
            },
            &mut quiet,
        );
        let interpreted = run(
            &m,
            Config {
                budget: 200,
                numerical_every: 1,
                differential_every: 1,
                probe_every: 0,
                symmetric_every: 0,
                ..Config::default()
            },
        );
        // The foreign path used its whole budget on points of the model, so nothing was spent
        // re-running the same path at another precision.
        assert_eq!(foreign.records.len() as u64, foreign.evaluations);
        assert_eq!(foreign.evaluations, 200);
        assert!(
            !foreign
                .evidence
                .iter()
                .any(|e| e.channel == Channel::Numerical || e.channel == Channel::Differential),
            "a channel that cannot answer was still reported"
        );
        // The interpreted path, with the same budget and rates, spends part of it on probes: fewer
        // placed points, and both channels present.
        assert!(
            interpreted.evaluations == 200,
            "the budget is a cap on both paths"
        );
        assert!(
            (interpreted.records.len() as u64) < interpreted.evaluations,
            "the interpreter should have paid for precision and reference runs"
        );
        assert!(
            interpreted
                .evidence
                .iter()
                .any(|e| e.channel == Channel::Numerical),
            "the capability that exists should still be used"
        );
    }

    #[test]
    fn the_same_seed_and_executor_answer_the_same_twice() {
        let m = square_model();
        let mut first = Linear {
            calls: 0,
            varies: false,
            reference: false,
        };
        let mut second = Linear {
            calls: 0,
            varies: false,
            reference: false,
        };
        let cfg = || Config {
            budget: 160,
            ..Config::default()
        };
        let a = run_with(&m, cfg(), &mut first);
        let b = run_with(&m, cfg(), &mut second);
        assert_eq!(a.final_risk, b.final_risk);
        assert_eq!(a.decisions.len(), b.decisions.len());
        assert_eq!(a.findings.len(), b.findings.len());
        assert_eq!(first.calls, second.calls);
    }

    /// An in-process stand-in for a program: a beam that deflects backwards beyond 60 N. Declared at
    /// module level because a type defined in the middle of a test body reads as an accident.
    struct Solver;

    impl Executor for Solver {
        fn execute(&mut self, _model: &Model, x: &[f64], _cfg: ExecConfig) -> Outcome {
            let load = x.first().copied().unwrap_or(0.0);
            let deflection = if load > 60.0 { -1.4 } else { 2.1 };
            Outcome {
                outputs: vec![deflection],
                traces: Vec::new(),
                flags: aporia_runtime::value::Flags::default(),
                steps: 0,
                rule_values: Vec::new(),
            }
        }

        fn varies_with_precision(&self) -> bool {
            false
        }

        fn has_reference_path(&self) -> bool {
            false
        }
    }

    #[test]
    fn a_model_declared_external_is_analysed_through_the_program_it_names() {
        // The claim of the whole input boundary in one test: a `.ap` file that declares `output`
        // instead of equations, lowered by the real front end, sampled by the real campaign driver and
        // executed by an in-process program. Nothing about the analysis is special-cased for foreign
        // models -- same acquisition, same rules, same budget accounting -- because if the adapter
        // path got its own weaker analysis, the instrument would stop being one instrument.
        //
        // The program is a beam whose deflection goes negative beyond 60 N of load. The model's own
        // rule says that is not allowed, so the atlas has to find the region, and the region is the
        // program's behaviour, not anything the A-IR computes.
        let m = model(
            "model beam \"\" {\n input load : N in [0, 100]\n output deflection : mm\n require deflection >= 0\n}\n",
        );
        assert!(
            aporia_runtime::needs_adapter(&m),
            "a declared-external model must not be interpretable"
        );

        let adapted = run_with(
            &m,
            Config {
                budget: 240,
                probe_every: 0,
                symmetric_every: 0,
                ..Config::default()
            },
            &mut Solver,
        );
        assert!(
            !adapted.findings.is_empty(),
            "the campaign found nothing wrong with a beam that deflects backwards"
        );
        assert!(
            adapted
                .evidence
                .iter()
                .any(|e| e.channel == Channel::Physical && e.detail.contains("deflection")),
            "{:?}",
            adapted
                .evidence
                .iter()
                .map(|e| e.detail.clone())
                .collect::<Vec<_>>()
        );
        // Part of the space is flagged and part is not: an external model is analysed, not simply
        // refused. The suspicious share should sit near the share of the domain past 60 N.
        let suspicious = adapted.atlas.coverage().suspicious;
        assert!(
            suspicious > 0.0 && suspicious < 0.95,
            "suspicious volume {suspicious} does not look like a region"
        );
        assert!(
            adapted.atlas.coverage().trusted > 0.0,
            "nothing was trusted, so the map says only 'external program'"
        );
        // And the rule oracle still works on it, because a violated rule is violated wherever it
        // fires -- which is what the counterexample minimiser needs from an adapter run.
        let violated = adapted
            .findings
            .iter()
            .any(|f| f.evidence.iter().any(|e| e.channel == Channel::Physical));
        assert!(violated, "no finding was attributed to the model's rule");
    }
}
