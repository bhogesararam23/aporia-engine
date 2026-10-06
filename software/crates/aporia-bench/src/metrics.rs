//! The metrics the research question is answered with, and nothing else.
//!
//! Every number here is defined by a computation in this file, and the definitions are written next
//! to the code that performs them, because a metric whose meaning is decided after the run is not a
//! measurement. The important distinction is between what the *search* did and what is *true*: the
//! truth side comes from the corpus declaration and direct evaluation, never from the pipeline being
//! scored.
//!
//! - `first_true_failure` — the index of the first evaluation that landed inside a declared region.
//!   It says something about the sampler's luck and nothing about the method, which is why it is
//!   reported separately from localisation.
//! - `detected` — some SUSPICIOUS leaf intersects the region. Loose on purpose: a coarse cell that
//!   contains a narrow fault is a hit for this metric and a miss for the next one.
//! - `localised` — a SUSPICIOUS leaf intersects the region *and* is at least half inside declared
//!   regions. A big lazy cell cannot pass it, so this is what the budget sweep searches for.
//! - `false_positive_fraction` — suspicious volume that is not really wrong, as a fraction of all
//!   suspicious volume. On a control entry every cubic unit of suspicion is a false positive, which
//!   is exactly why the corpus carries controls.
//! - `boundary_error` — distance from a declared boundary to the nearest band edge, in units of the
//!   axis width, plus whether it fell inside the declared tolerance.
//! - `counterexample` — dimensions and significant digits left after minimisation, which is the
//!   usefulness half of the same finding, and the oracle calls and model executions it took, which
//!   are two numbers because the two oracles do not cost the same per answer.

use crate::corpus::Entry;
use crate::truth::Boundary;
use aporia_boundary::Label;
use aporia_search::{Campaign, Finding, Strategy};
use aporia_store::Json;

/// One declared boundary and what the finished atlas had to say about it.
#[derive(Clone, Debug, PartialEq)]
pub struct BoundaryHit {
    pub axis: String,
    pub declared_at: f64,
    pub tolerance: f64,
    pub band: Option<[f64; 2]>,
    pub error: Option<f64>,
    pub within_tolerance: bool,
}

impl BoundaryHit {
    fn to_json(&self) -> Json {
        Json::object(vec![
            ("axis", Json::text(self.axis.clone())),
            ("declared_at", Json::number(self.declared_at)),
            ("tolerance", Json::number(self.tolerance)),
            (
                "band",
                match self.band {
                    Some([lo, hi]) => Json::Arr(vec![Json::number(lo), Json::number(hi)]),
                    None => Json::Null,
                },
            ),
            (
                "error",
                self.error
                    .map_or(Json::text("no band on this axis"), Json::number),
            ),
            ("within_tolerance", Json::Bool(self.within_tolerance)),
        ])
    }
}

/// The size of a minimised counterexample, and what making it smaller cost in both units.
#[derive(Clone, Debug, PartialEq)]
pub struct CaseSize {
    pub dimensions: u64,
    pub digits: u64,
    pub description: String,
    pub verified: bool,
    /// Which question this row answers: `"rule"` — a declared rule fails at these coordinates — or
    /// `"risk"` — the report's own frozen evidence model still puts them at or above the bar that
    /// flagged the cell. Never both, and never silently the second one dressed as the first.
    pub oracle: &'static str,
    /// Oracle calls made, summed over whichever attempts the row needed. This is the unit the
    /// minimisation budget is spent in; it is not a measure of work.
    pub queries: u64,
    /// Model executions those calls performed, on the same sum.
    ///
    /// Equal to [`Self::queries`] for a `"rule"` row, because asking whether a declared rule fails
    /// runs the model once. A `"risk"` row rebuilds the evidence the report used — the candidate, one
    /// perturbed point per axis the probe star could build, a reduced-precision re-run and a
    /// double-double reference evaluation — so its executions exceed its calls by that many, and the
    /// two columns are read together rather than one of them being called "the cost".
    pub executions: u64,
}

impl CaseSize {
    fn to_json(&self) -> Json {
        Json::object(vec![
            ("dimensions", Json::count(self.dimensions)),
            ("digits", Json::count(self.digits)),
            ("description", Json::text(self.description.clone())),
            ("verified", Json::Bool(self.verified)),
            ("oracle", Json::text(self.oracle.to_string())),
            ("minimisation_queries", Json::count(self.queries)),
            ("minimisation_executions", Json::count(self.executions)),
        ])
    }
}

/// One channel's record in one outcome: the four counts that make an ablation's nulls
/// interpretable, plus the mask bit that says who removed what.
///
/// `applied` is how many executions the channel's measurements ran on, `computed` how many items
/// it produced before this arm's mask dropped anything, `readings` how many the arm kept,
/// `strong` how many of those calibrate to the loudness bar, and `findings` how many findings
/// carry the channel. From these, and never from anything else, the five states a channel can be
/// in are read: structurally unavailable (`applied = 0`), available but clean (`applied > 0`,
/// `computed = 0`), silenced after computing (`silenced`, `computed > 0`, `readings = 0`), weak
/// versus strong readings, and evidence-producing but not finding-producing (`readings > 0`,
/// `findings = 0`).
#[derive(Clone, Debug, PartialEq)]
pub struct ChannelCensus {
    pub channel: aporia_evidence::Channel,
    pub applied: u64,
    pub computed: u64,
    pub readings: u64,
    /// Kept readings whose strength, under this campaign's own final calibrator, reaches the bar
    /// that lets a claim sign a finding (0.5 — the same number `signature` uses, so "strong" here
    /// means the same thing it means everywhere else in a report).
    pub strong: u64,
    pub findings: u64,
    pub silenced: bool,
}

impl ChannelCensus {
    fn to_json(&self) -> Json {
        Json::object(vec![
            ("channel", Json::text(self.channel.name())),
            ("applied", Json::count(self.applied)),
            ("computed", Json::count(self.computed)),
            ("readings", Json::count(self.readings)),
            ("strong", Json::count(self.strong)),
            ("findings", Json::count(self.findings)),
            ("silenced", Json::Bool(self.silenced)),
        ])
    }

    fn from_json(value: &Json) -> Option<Self> {
        let channel = aporia_evidence::Channel::parse(value.get("channel")?.as_str()?)?;
        let count = |k: &str| value.get(k).and_then(Json::as_u64);
        Some(Self {
            channel,
            applied: count("applied")?,
            computed: count("computed")?,
            readings: count("readings")?,
            strong: count("strong")?,
            findings: count("findings")?,
            silenced: value.get("silenced")?.as_bool()?,
        })
    }
}

/// What each of the five channels did on one campaign, in [`Channel::ALL`] order.
///
/// Derived from the campaign's own records — its evidence, its findings, its per-channel counters
/// and the mask it actually ran with — never from the plan that requested the run: the census
/// exists to say, per entry and per arm, whether a channel had anything to say, and that is a
/// fact about the execution rather than about the configuration.
#[derive(Clone, Debug, PartialEq)]
pub struct Census {
    pub channels: Vec<ChannelCensus>,
}

impl Census {
    #[must_use]
    pub fn of(campaign: &Campaign) -> Self {
        let channels = aporia_evidence::Channel::ALL
            .into_iter()
            .map(|c| {
                let readings: Vec<&aporia_evidence::Evidence> = campaign
                    .evidence
                    .iter()
                    .filter(|e| e.channel == c)
                    .collect();
                let strong = readings
                    .iter()
                    .filter(|e| {
                        // The final calibrator, applied to a copy, is what the report's own
                        // loudness bar was measured against — reusing it here is what keeps
                        // "strong" from meaning one thing in a finding and another in a census.
                        let mut scored = (**e).clone();
                        campaign.calibrator.apply(std::slice::from_mut(&mut scored));
                        scored.strength >= 0.5
                    })
                    .count() as u64;
                ChannelCensus {
                    channel: c,
                    applied: campaign.applied[c.index()],
                    computed: campaign.computed[c.index()],
                    readings: readings.len() as u64,
                    strong,
                    findings: campaign
                        .findings
                        .iter()
                        .filter(|f| f.evidence.iter().any(|e| e.channel == c))
                        .count() as u64,
                    silenced: campaign.config.is_silenced(c),
                }
            })
            .collect();
        Self { channels }
    }

    /// The record for one channel, when present.
    #[must_use]
    pub fn of_channel(&self, channel: aporia_evidence::Channel) -> Option<&ChannelCensus> {
        self.channels.iter().find(|c| c.channel == channel)
    }

    fn to_json(&self) -> Json {
        Json::Arr(self.channels.iter().map(ChannelCensus::to_json).collect())
    }

    /// Read a census back out of the JSON a results file carries, in the order it was written.
    /// Absent when the file predates the field, which is absence rather than a zero.
    #[must_use]
    pub fn from_json(value: &Json) -> Option<Self> {
        Some(Self {
            channels: value
                .as_array()?
                .iter()
                .map(ChannelCensus::from_json)
                .collect::<Option<Vec<_>>>()?,
        })
    }
}

/// One (entry, strategy, budget, seed) measurement.
#[derive(Clone, Debug)]
pub struct Outcome {
    pub entry: String,
    pub family: String,
    pub fault: String,
    pub control: bool,
    pub strategy: &'static str,
    pub seed: u64,
    pub budget: u64,
    pub evaluations: u64,
    pub instruction_steps: u64,
    pub wall_ms: u64,
    pub arity: usize,
    pub declared_regions: u64,
    pub detected_regions: u64,
    pub localised_regions: u64,
    pub first_true_failure: Option<u64>,
    pub false_positive_fraction: Option<f64>,
    pub suspicious_volume: f64,
    pub trusted_volume: f64,
    pub unknown_volume: f64,
    pub findings: u64,
    pub duplicates: Option<f64>,
    pub boundaries: Vec<BoundaryHit>,
    pub counterexamples: Vec<CaseSize>,
    /// What each channel did on this outcome. See [`Census`].
    pub census: Census,
    /// `(reproduced, matched, total)` when this run was archived and replayed.
    pub replay: Option<(bool, u64, u64)>,
    pub replay_error: Option<String>,
}

impl Outcome {
    /// Measure a finished campaign against the entry's declaration.
    #[must_use]
    #[expect(
        clippy::too_many_lines,
        reason = "one measurement per block, kept next to the definition it computes"
    )]
    pub fn measure(
        entry: &Entry,
        campaign: &Campaign,
        wall_ms: u64,
        strategy: Strategy,
        budget: u64,
    ) -> Self {
        let model = entry
            .model
            .as_ref()
            .expect("the corpus loader refuses an entry that does not compile");
        let truth = &entry.truth;
        let arity = model.params.len();

        let leaves: Vec<_> = campaign
            .atlas
            .leaf_ids()
            .iter()
            .filter_map(|id| campaign.atlas.cell(*id))
            .cloned()
            .collect();
        let suspicious: Vec<_> = leaves
            .iter()
            .filter(|c| c.label == Label::Suspicious)
            .cloned()
            .collect();

        let detected = truth
            .regions
            .iter()
            .filter(|r| {
                suspicious.iter().any(|c| {
                    region_intersects(model, &c.bounds, r) && overlap_of(model, &c.bounds, r) > 0.0
                })
            })
            .count() as u64;
        let localised = truth
            .regions
            .iter()
            .filter(|r| {
                suspicious.iter().any(|c| {
                    region_intersects(model, &c.bounds, r) && overlap_of(model, &c.bounds, r) >= 0.5
                })
            })
            .count() as u64;

        let first_true_failure = campaign
            .records
            .items
            .iter()
            .position(|o| truth.contains(model, &o.x))
            .map(|i| i as u64);

        // Cell sizes are counted as fractions of the declared domain, so a report can add them up
        // across models with different units on different axes.
        let volume_of = |c: &aporia_boundary::Cell| cell_fraction(model, &c.bounds);
        let suspicious_volume: f64 = suspicious.iter().map(volume_of).sum();
        let trusted_volume: f64 = leaves
            .iter()
            .filter(|c| c.label == Label::Trusted)
            .map(volume_of)
            .sum();
        let unknown_volume: f64 = leaves
            .iter()
            .filter(|c| c.label == Label::Unknown)
            .map(volume_of)
            .sum();

        // A suspicious cell's justified share is its overlap with the union of declared regions.
        let false_positive_fraction = if suspicious_volume > 0.0 {
            let justified: f64 = suspicious
                .iter()
                .map(|c| {
                    let share = best_overlap(model, &c.bounds, truth);
                    volume_of(c) * share
                })
                .sum();
            Some((1.0 - justified / suspicious_volume).clamp(0.0, 1.0))
        } else if truth.control {
            // Nothing suspicious anywhere: a perfect control result.
            Some(0.0)
        } else {
            None
        };

        let duplicates = duplicate_rate(&campaign.findings, truth, model);

        let boundaries = truth
            .boundaries
            .iter()
            .map(|b| boundary_hit(model, campaign, b))
            .collect();

        Self {
            entry: entry.id(),
            family: entry.family.clone(),
            fault: truth.fault.clone(),
            control: truth.control,
            strategy: strategy.name(),
            seed: 0,
            budget,
            evaluations: campaign.evaluations,
            instruction_steps: campaign.instruction_steps,
            wall_ms,
            arity,
            declared_regions: truth.regions.len() as u64,
            detected_regions: detected,
            localised_regions: localised,
            first_true_failure,
            false_positive_fraction,
            suspicious_volume,
            trusted_volume,
            unknown_volume,
            findings: campaign.findings.len() as u64,
            duplicates,
            boundaries,
            census: Census::of(campaign),
            replay: None,
            replay_error: None,
            // Filled in by the harness at the largest budget only: minimisation costs evaluations
            // of its own, and charging it at every budget would bill the same description five
            // times while inflating the cost column of the very table being compared.
            counterexamples: Vec::new(),
        }
    }

    #[must_use]
    pub fn to_json(&self) -> Json {
        Json::object(vec![
            ("entry", Json::text(self.entry.clone())),
            ("family", Json::text(self.family.clone())),
            ("fault", Json::text(self.fault.clone())),
            ("control", Json::Bool(self.control)),
            ("strategy", Json::text(self.strategy)),
            ("seed", Json::count(self.seed)),
            ("budget", Json::count(self.budget)),
            ("evaluations", Json::count(self.evaluations)),
            ("instruction_steps", Json::count(self.instruction_steps)),
            ("wall_ms", Json::count(self.wall_ms)),
            ("arity", Json::count(self.arity as u64)),
            ("declared_regions", Json::count(self.declared_regions)),
            ("detected_regions", Json::count(self.detected_regions)),
            ("localised_regions", Json::count(self.localised_regions)),
            (
                "first_true_failure",
                self.first_true_failure.map_or(Json::Null, Json::count),
            ),
            (
                "false_positive_fraction",
                self.false_positive_fraction
                    .map_or(Json::Null, Json::number),
            ),
            ("suspicious_volume", Json::number(self.suspicious_volume)),
            ("trusted_volume", Json::number(self.trusted_volume)),
            ("unknown_volume", Json::number(self.unknown_volume)),
            ("findings", Json::count(self.findings)),
            (
                "duplicate_discovery_rate",
                self.duplicates.map_or(Json::Null, Json::number),
            ),
            (
                "boundaries",
                Json::Arr(self.boundaries.iter().map(BoundaryHit::to_json).collect()),
            ),
            (
                "counterexamples",
                Json::Arr(self.counterexamples.iter().map(CaseSize::to_json).collect()),
            ),
            ("census", self.census.to_json()),
            (
                "replayed",
                self.replay
                    .map_or(Json::Null, |(r, m, t)| Json::Bool(r && m == t && t > 0)),
            ),
        ])
    }
}

/// Minimise up to three of the campaign's findings. Separate from `Outcome::measure` so the harness
/// decides when the cost is worth paying.
///
/// Two oracles, in that order, and the second only when the first cannot answer. `FailureOracle` asks
/// whether a declared rule fails, which is the strongest thing a counterexample can be verified
/// against and costs nothing extra; when a finding came from the measurement channels instead, a rule
/// never fails anywhere and the reduction has no criterion to preserve. Then [`RiskScorer`] asks the
/// question the report itself asked — is this point still at or above the bar that made the cell
/// SUSPICIOUS — against the campaign's own frozen calibrator, correlation and ordinary slopes.
///
/// Which oracle produced a row is recorded, because the two answers are not the same claim: a rule
/// failure says the model is wrong at these coordinates, a risk hit says the instrument would still
/// flag them. On a control that distinction is the entire content of the measurement.
///
/// Cost is the two attempts summed, in both units — calls and model executions — so a row's total is
/// what the finding actually cost even when only the second oracle could answer it. The split between
/// the attempts is not recorded, because no reader asked for it; what the pair does give is the
/// surplus, since a rule call costs exactly one execution and the difference between the columns is
/// therefore the probe star the risk calls paid for.
///
/// `engine` is the path the campaign executed the model on, and it is the path both oracles ask. A
/// corpus entry whose arithmetic came from an external program has its counterexample re-checked by
/// that program, or not at all: a reduction confirmed by arithmetic the model never ran is not a
/// smaller version of the finding, it is a different claim.
#[must_use]
pub fn counterexamples(
    entry: &Entry,
    campaign: &Campaign,
    budget: u64,
    engine: &mut dyn aporia_runtime::Executor,
) -> Vec<CaseSize> {
    let Some(model) = entry.model.as_ref() else {
        return Vec::new();
    };
    campaign
        .findings
        .iter()
        .take(3)
        .map(|f| {
            let config = aporia_minimize::Config {
                budget,
                ..aporia_minimize::Config::default()
            };
            let rule = aporia_minimize::minimize(
                &aporia_minimize::FailureOracle::new(model),
                engine,
                model,
                &f.representative,
                config,
            );
            let rule_executions = rule.executions;
            let rule_queries = rule.queries;
            // Built per finding, and only trusted if it reproduces the finding first: the scorer is
            // the campaign's frozen evidence model, and the agreement check is what stops a reduction
            // from being graded by a question the report never asked.
            let scorer = crate::risk::RiskScorer::for_finding(campaign, f, &*engine);
            let (minimal, oracle, queries, executions) =
                if rule.verified || !scorer.agrees_with(model, f, engine) {
                    (rule, "rule", rule_queries, rule_executions)
                } else {
                    let risk_oracle = crate::risk::RiskOracle::new(model, &scorer);
                    // The budget is the shared one: the fallback is not a second helping of search, it
                    // is the same finding asked of a different oracle, and the cost of both attempts is
                    // reported together so a reader can see what the second question added.
                    let risk = aporia_minimize::minimize(
                        &risk_oracle,
                        engine,
                        model,
                        &f.representative,
                        config,
                    );
                    let pair = (
                        rule_queries + risk.queries,
                        rule_executions + risk.executions,
                    );
                    if risk.verified {
                        (risk, "risk", pair.0, pair.1)
                    } else {
                        // Neither verified: report the rule attempt, which is the claim the reader
                        // would have expected, and let `verified: false` say that nothing was
                        // established.
                        (rule, "rule", pair.0, pair.1)
                    }
                };
            CaseSize {
                dimensions: minimal.case.dimensions() as u64,
                digits: minimal.case.digits() as u64,
                description: minimal.case.describe(),
                verified: minimal.verified,
                oracle,
                queries,
                executions,
            }
        })
        .collect()
}

/// Fraction of a cell that lies inside one declared region.
fn overlap_of(
    model: &aporia_ir::Model,
    bounds: &[[f64; 2]],
    region: &crate::truth::Declared,
) -> f64 {
    let mut fraction = 1.0;
    for (name, [lo, hi]) in &region.axes {
        let Some(i) = model.param(name).map(|p| p as usize) else {
            continue;
        };
        let Some([clo, chi]) = bounds.get(i) else {
            return 0.0;
        };
        let width = (chi - clo).max(1e-30);
        fraction *= (hi.min(*chi) - lo.max(*clo)).max(0.0) / width;
    }
    fraction
}

fn region_intersects(
    model: &aporia_ir::Model,
    bounds: &[[f64; 2]],
    region: &crate::truth::Declared,
) -> bool {
    region.axes.iter().all(|(name, [lo, hi])| {
        let Some(i) = model.param(name).map(|p| p as usize) else {
            return false;
        };
        let Some([clo, chi]) = bounds.get(i) else {
            return false;
        };
        chi >= lo && clo <= hi
    })
}

/// Overlap with whichever declared region explains the cell best.
fn best_overlap(model: &aporia_ir::Model, bounds: &[[f64; 2]], truth: &crate::truth::Truth) -> f64 {
    truth
        .regions
        .iter()
        .map(|r| overlap_of(model, bounds, r))
        .fold(0.0, f64::max)
}

fn cell_fraction(model: &aporia_ir::Model, bounds: &[[f64; 2]]) -> f64 {
    bounds.iter().enumerate().fold(1.0, |acc, (i, [lo, hi])| {
        let span = declared_width(model, i);
        acc * ((hi - lo).min(span).max(0.0) / span.max(1e-30))
    })
}

fn declared_width(model: &aporia_ir::Model, i: usize) -> f64 {
    use aporia_ir::Domain;
    match &model.params[i].domain {
        Domain::Interval { lo, hi } => hi - lo,
        Domain::Choices(v) => v.len().max(1) as f64,
    }
}

/// How many findings describe a fault the report has already described.
///
/// Leaves of one atlas never overlap, so suspicious *volume* cannot measure duplication. The honest
/// version counts findings against declared regions: a finding is assigned to the region it overlaps
/// best, and every further finding assigned to the same region is a duplicate. Zero findings, or an
/// entry with no declared regions, has no answer to give, which is reported as None rather than as a
/// flattering zero.
fn duplicate_rate(
    findings: &[Finding],
    truth: &crate::truth::Truth,
    model: &aporia_ir::Model,
) -> Option<f64> {
    if truth.regions.is_empty() || findings.is_empty() {
        return None;
    }
    let mut claimed: Vec<u32> = Vec::new();
    let mut duplicates = 0u64;
    for f in findings {
        let best = truth
            .regions
            .iter()
            .enumerate()
            .map(|(i, r)| (i, overlap_of(model, &f.bounds, r)))
            .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let Some((index, share)) = best else { continue };
        if share <= 0.0 {
            // A finding that overlaps nothing declared is a false positive, not a duplicate.
            continue;
        }
        if claimed.contains(&(index as u32)) {
            duplicates += 1;
        } else {
            claimed.push(index as u32);
        }
    }
    Some((duplicates as f64 / findings.len() as f64).clamp(0.0, 1.0))
}

fn boundary_hit(model: &aporia_ir::Model, campaign: &Campaign, b: &Boundary) -> BoundaryHit {
    let Some(axis) = model.param(&b.axis).map(|p| p as usize) else {
        // Nothing was measured, so nothing is reported as measured. `error: Some(f64::NAN)` was here
        // instead, which the results writer rendered as the string "NaN" -- a fabricated reading of a
        // number that does not exist, in the same column as real errors, and `within_tolerance: false`
        // made the absence count as a missed boundary. `corpus::verify` now refuses such a declaration
        // before any measurement runs; this is the shape the answer takes if that check is ever
        // bypassed.
        return BoundaryHit {
            axis: b.axis.clone(),
            declared_at: b.at,
            tolerance: b.tolerance,
            band: None,
            error: None,
            within_tolerance: false,
        };
    };
    let width = declared_width(model, axis);
    // The bands come from the finished atlas, which is the object a user reads.
    let mut best: Option<f64> = None;
    let mut band = None;
    for found in campaign.atlas.bands() {
        if found.axis as usize != axis {
            continue;
        }
        let candidates = [
            found.facing[0],
            found.facing[1],
            f64::midpoint(found.lo, found.hi),
        ];
        for c in candidates {
            let error = (c - b.at).abs();
            if best.is_none_or(|worse| error < worse) {
                best = Some(error);
                band = Some([found.lo, found.hi]);
            }
        }
    }
    let error = best.map(|e| e / width.max(1e-30));
    let within = best.is_some_and(|e| e <= b.tolerance);
    BoundaryHit {
        axis: b.axis.clone(),
        declared_at: b.at,
        tolerance: b.tolerance,
        band,
        error,
        within_tolerance: within,
    }
}

/// Aggregated view of one entry across strategies, which is the table the report prints.
#[must_use]
pub fn compare(outcomes: &[Outcome]) -> Vec<Json> {
    let mut by_entry: Vec<(String, Vec<&Outcome>)> = Vec::new();
    for o in outcomes {
        if let Some(slot) = by_entry.iter_mut().find(|(k, _)| *k == o.entry) {
            slot.1.push(o);
        } else {
            by_entry.push((o.entry.clone(), vec![o]));
        }
    }
    by_entry
        .into_iter()
        .map(|(entry, group)| {
            let row_for = |name: &str| {
                let Some(best) = group.iter().filter(|o| o.strategy == name).max_by_key(|o| {
                    // Rank on localised regions first, then on how early they were localised.
                    (o.localised_regions, o.evaluations.min(u32::MAX as u64))
                }) else {
                    return Json::Null;
                };
                Json::object(vec![
                    ("evaluations", Json::count(best.evaluations)),
                    ("localised", Json::count(best.localised_regions)),
                    ("detected", Json::count(best.detected_regions)),
                    (
                        "false_positive",
                        best.false_positive_fraction
                            .map_or(Json::Null, Json::number),
                    ),
                ])
            };
            // One row per arm this batch actually measured, in the order the enum reports them. The
            // three names this function used to write out by hand are how the comparison table would
            // have gone on claiming three arms after the plan started measuring four — the same class
            // of defect `verdict` had, and the reason the run that found it was worth doing.
            let arms: Vec<&'static str> = Strategy::ALL
                .iter()
                .map(|s| s.name())
                .filter(|name| group.iter().any(|o| o.strategy == *name))
                .collect();
            let mut row = vec![("entry", Json::text(entry))];
            for name in arms {
                row.push((name, row_for(name)));
            }
            Json::object(row)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use aporia_dsl::lower::compile;
    use aporia_evidence::Channel;
    use aporia_search::{Config, run};

    /// A model whose slope grows along its only axis, so the Sensitivity channel certainly has
    /// something to say wherever probes run.
    fn slope_model() -> aporia_ir::Model {
        let c = compile(
            "t.ap",
            "model census_slope \"\" {\n input x in [0, 1]\n let y = x * x\n}\n",
        );
        assert!(!c.diagnostics.has_errors());
        c.model
    }

    fn campaign_config() -> Config {
        Config {
            budget: 240,
            probe_every: 2,
            ..Config::default()
        }
    }

    #[test]
    fn the_census_reads_the_campaign_not_the_requested_mask() {
        // Three campaigns, one model. A census computed from the mask could only distinguish the
        // first two; only a census computed from what ran can say the third one silenced a channel
        // that had readings waiting, which is the fact E2's nulls are read against.
        let m = slope_model();

        // No mask and no probes: the channel never ran. Structurally unavailable.
        let unprobed = run(
            &m,
            Config {
                budget: 240,
                probe_every: 0,
                ..Config::default()
            },
        );
        let quiet = Census::of(&unprobed)
            .of_channel(Channel::Sensitivity)
            .expect("every channel has a census entry")
            .clone();
        assert_eq!((quiet.applied, quiet.computed, quiet.readings), (0, 0, 0));
        assert!(
            !quiet.silenced,
            "nothing was silenced; the channel never ran"
        );

        // No mask, probes on: the channel ran and spoke.
        let full = Census::of(&run(&m, campaign_config()))
            .of_channel(Channel::Sensitivity)
            .expect("every channel has a census entry")
            .clone();
        assert!(full.applied > 0, "the probe star was never applied");
        assert!(full.computed > 0, "y = x*x produced no atypical slopes");
        assert_eq!(full.computed, full.readings, "nothing dropped the readings");
        assert!(!full.silenced);

        // Mask on, probes on: the probe star still ran and still computed readings — the records
        // say so — and only the keeping was withheld. That is "silenced", not "silent", and it is
        // the distinction a mask-derived census cannot make.
        let arm = run(&m, campaign_config().without(Channel::Sensitivity));
        let silenced = Census::of(&arm)
            .of_channel(Channel::Sensitivity)
            .expect("every channel has a census entry")
            .clone();
        assert!(
            silenced.applied > 0,
            "silencing must not skip the probe star"
        );
        assert!(
            silenced.computed > 0,
            "the silenced arm recorded no computed readings, so the mask changed the work"
        );
        assert_eq!(silenced.readings, 0, "a silenced channel kept readings");
        assert_eq!(
            silenced.strong, 0,
            "a silenced channel kept strong readings"
        );
        assert!(silenced.silenced);
    }

    #[test]
    fn the_census_counts_strong_readings_with_the_campaigns_own_bar() {
        // Physical carries the census's loudest case: a rule that fires is an absolute fact, so
        // every Physical reading on this model is strong by the campaign's own calibration.
        let c = compile(
            "t.ap",
            "model census_rule \"\" {\n input x in [0, 1]\n let y = sqrt(x - 0.5)\n require finite(y)\n}\n",
        );
        assert!(!c.diagnostics.has_errors());
        let campaign = run(&c.model, Config::default());
        let physical = Census::of(&campaign)
            .of_channel(Channel::Physical)
            .expect("every channel has a census entry")
            .clone();
        assert!(physical.readings > 0, "the NaN region produced no readings");
        assert_eq!(physical.strong, physical.readings);
        assert!(physical.findings > 0, "a firing rule produced no finding");
    }

    #[test]
    fn the_census_survives_a_round_trip_through_the_results_file() {
        let campaign = run(&slope_model(), campaign_config());
        let census = Census::of(&campaign);
        let text = census.to_json().to_compact();
        let parsed = aporia_store::Json::parse(&text).expect("the census JSON parses");
        let back = Census::from_json(&parsed).expect("the census JSON reads back");
        assert_eq!(back.channels.len(), Channel::ALL.len());
        assert_eq!(back, census);
        // Every channel is present even when it did nothing, in the enum's order: an absent row
        // would read as "not measured on this path" and a reordered one as somebody else's census.
        assert_eq!(
            back.channels.iter().map(|c| c.channel).collect::<Vec<_>>(),
            Channel::ALL.to_vec()
        );
        // And a document without the field — every results file written before the census existed —
        // simply has no census, which is absence rather than a zero.
        assert!(Census::from_json(&Json::Null).is_none());
    }
}
