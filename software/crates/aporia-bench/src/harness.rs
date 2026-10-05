//! Running the corpus: the budget sweep, the comparison, and the numbers that go into `results/`.
//!
//! The research question is about *cost*, so the primary measurement is not a single run per
//! strategy but the smallest budget at which each strategy manages the thing the entry declares:
//! detect the fault, localise it to a cell that is mostly really wrong, or — for a control — stay
//! clean. That is a doubling search over budgets, and the budgets are the declared axis of the
//! experiment rather than a wall-clock limit, so the same sweep runs on any machine.
//!
//! Every strategy runs the same driver from `aporia-search` with a different sampling rule, which is
//! what makes the comparison about placement. Nothing in this file re-implements a search.

use crate::corpus::Entry;
use crate::metrics::Outcome;
use crate::truth::Truth;
use aporia_boundary::Policy;
use aporia_search::{Config, Strategy, run};
use aporia_store::Environment;
use aporia_store::Json;
use std::time::Instant;

/// What a run is allowed to cost, and how often it is repeated.
#[derive(Clone, Debug)]
pub struct Plan {
    pub budgets: Vec<u64>,
    pub strategies: Vec<Strategy>,
    pub seeds: Vec<u64>,
    /// Points per axis for the ground-truth verification grid.
    pub grid: usize,
    /// Oracle calls allowed per counterexample minimisation.
    pub minimise_budget: u64,
    pub probe_every: u64,
    pub calibrate_every: u64,
    pub refine_every: u64,
    pub numerical_every: u64,
    /// Every nth base point is re-run with each declared symmetric parameter pair swapped, so a
    /// `check symmetric(...)` is actually tested rather than declared and never checked. Costs one
    /// evaluation per declared pair per firing; a model declaring no symmetry pays nothing, which is
    /// why this is on by default.
    pub symmetric_every: u64,
    /// Every nth base point is also run through the independent double-double reference evaluator,
    /// which is what makes the Differential channel exist during a measurement. Costs one reference
    /// evaluation per firing — the reference path is several times slower per step than the runtime —
    /// so it is a rate rather than a flag, and the rate is recorded in the plan the results carry.
    pub differential_every: u64,
    /// Where per-entry archives go. Left empty, the harness skips archiving: the archives are the
    /// expensive part of a run and their numbers do not change the search.
    pub archive_dir: std::path::PathBuf,
}

impl Default for Plan {
    fn default() -> Self {
        Self {
            budgets: vec![60, 120, 250, 500, 1000],
            strategies: vec![Strategy::Adaptive, Strategy::Stratified, Strategy::Random],
            seeds: vec![1],
            grid: 33,
            minimise_budget: 4_000,
            probe_every: 7,
            calibrate_every: 25,
            refine_every: 40,
            numerical_every: 11,
            symmetric_every: 1,
            // Off in the ladder, on in `explain` and on demand via `--differential-every`.
            //
            // Measured by A/B on the two controls at budgets 160 and 640, seeds 1,2,3, identical in
            // every other respect: with the rate at 0, 3 of 9 spring sweeps and 7 of 9 projectile
            // sweeps are suspicion-free at some budget; with the rate at 11 that falls to 0 of 9 and
            // 2 of 9. Not because the channel's evidence is loud -- its items calibrate to strength
            // 0.000 on these models, and moving the channel's noise floor over four orders changed
            // nothing at all, two runs came out byte-identical. Because every reference evaluation
            // is charged to the budget, so the same seed walks a different sample path and different
            // cells end up holding the single loud numerical reading that the corroboration rule
            // already refuses to call suspicious on its own.
            //
            // The finding is methodological and it applies to any evidence channel that costs
            // evaluations: control cleanliness in this build is sensitive to sampling trajectory,
            // not only to evidence semantics. Keeping the rate off by default means the published
            // ladder stays comparable with the runs before the channel existed, and the flag makes
            // the trade measurable by anyone with one command.
            differential_every: 0,
            archive_dir: std::path::PathBuf::new(),
        }
    }
}

impl Plan {
    fn config(&self, strategy: Strategy, budget: u64, seed: u64) -> Config {
        Config {
            budget,
            strategy,
            seed,
            probe_every: self.probe_every,
            policy: Policy::default(),
            calibrate_every: self.calibrate_every,
            numerical_every: self.numerical_every,
            symmetric_every: self.symmetric_every,
            differential_every: self.differential_every,
            refine_every: self.refine_every,
            max_steps_per_evaluation: 2_000_000,
        }
    }
}

/// The result of one (entry, strategy, seed) sweep.
#[derive(Clone, Debug)]
pub struct Sweep {
    pub entry: String,
    pub strategy: &'static str,
    pub seed: u64,
    pub outcomes: Vec<Outcome>,
    /// Smallest budget at which some suspicious cell touched a declared region.
    pub detected_at: Option<u64>,
    /// Smallest budget at which every declared region was inside a mostly-justified suspicious cell.
    pub localised_at: Option<u64>,
    /// For a control: the smallest budget at which it stayed entirely free of suspicion.
    pub clean_at: Option<u64>,
    /// For a control: total suspicious volume at the largest budget, which is the false positive.
    pub control_suspicion: Option<f64>,
    /// `(reproduced, matched, total)` from writing this run to an archive and replaying it. Only
    /// the largest budget is archived, because that is the run a report would quote.
    pub replay: Option<(bool, u64, u64)>,
    pub replay_error: Option<String>,
}

/// Run one entry under every strategy and budget in the plan.
#[must_use]
pub fn sweep(entry: &Entry, plan: &Plan, seed: u64, strategy: Strategy) -> Option<Sweep> {
    let model = entry.model.as_ref()?;
    let mut outcomes = Vec::new();
    let mut detected_at = None;
    let mut localised_at = None;
    let mut clean_at = None;
    let mut control_suspicion = None;
    for &budget in &plan.budgets {
        let cfg = plan.config(strategy, budget, seed);
        let started = Instant::now();
        let campaign = run(model, cfg);
        let wall_ms = started.elapsed().as_millis() as u64;
        let mut outcome = Outcome::measure(entry, &campaign, wall_ms, strategy, budget);
        outcome.seed = seed;
        if !entry.truth.control && !entry.truth.regions.is_empty() {
            if detected_at.is_none() && outcome.detected_regions > 0 {
                detected_at = Some(budget);
            }
            if localised_at.is_none() && outcome.localised_regions == outcome.declared_regions {
                localised_at = Some(budget);
            }
        } else if outcome.suspicious_volume > 0.0 {
            control_suspicion = Some(outcome.suspicious_volume);
            clean_at = None;
        } else if clean_at.is_none() {
            clean_at = Some(budget);
        }
        let archiving = Some(budget) == plan.budgets.last().copied()
            && Some(seed) == plan.seeds.first().copied()
            && !plan.archive_dir.as_os_str().is_empty();
        if archiving {
            // Named per entry, not per family: two entries in one family otherwise overwrite each
            // other's archive, and the point of keeping them is that a reported number can be
            // opened later.
            let dir = plan.archive_dir.join(format!(
                "{}-{}-{strategy}-seed{seed}",
                entry.family,
                entry.name,
                strategy = strategy_name(strategy)
            ));
            match archive_and_replay(entry, &campaign, plan, strategy, seed, &dir) {
                Ok(r) => {
                    outcome.replay = Some(r);
                }
                Err(e) => {
                    outcome.replay_error = Some(e);
                }
            }
        }
        // Minimising a counterexample costs evaluations of its own, so it runs only at the largest
        // budget: the description of the fault does not change with the search's sample count, but
        // paying for it five times would.
        if budget == *plan.budgets.last().unwrap_or(&0) {
            outcome.counterexamples =
                crate::metrics::counterexamples(entry, &campaign, plan.minimise_budget);
        } else {
            outcome.counterexamples.clear();
        }
        outcomes.push(outcome);
    }
    // The archive is written for the largest budget only, so the sweep carries at most one.
    let replay = outcomes.iter().find_map(|o| o.replay);
    let replay_error = outcomes.iter().find_map(|o| o.replay_error.clone());
    Some(Sweep {
        entry: entry.id(),
        strategy: strategy_name(strategy),
        seed,
        outcomes,
        detected_at,
        localised_at,
        clean_at,
        control_suspicion,
        replay,
        replay_error,
    })
}

fn strategy_name(s: Strategy) -> &'static str {
    match s {
        Strategy::Random => "random",
        Strategy::Stratified => "stratified",
        Strategy::Adaptive => "adaptive",
    }
}

/// Write the archive for one campaign and replay it, returning `(reproduced, matched, total)`.
///
/// This is where the store earns its place in the measurement: a finding is only reproducible if
/// re-running the archive's own A-IR over its own recorded inputs gives the same bits, so the
/// benchmark reports the replay result beside its detection numbers instead of assuming it.
pub fn archive_and_replay(
    entry: &Entry,
    campaign: &aporia_search::Campaign,
    plan: &Plan,
    strategy: Strategy,
    seed: u64,
    dir: &std::path::Path,
) -> Result<(bool, u64, u64), String> {
    use aporia_store::{Environment, Json, Store};

    let model = entry.model.as_ref().ok_or("entry has no compiled model")?;
    let air = aporia_ir::to_text(model);
    // The campaign owns the mapping from a finding to its stored record; the only thing this harness
    // decides for itself is `case`, and it decides it by running the minimiser and keeping the
    // description only when the reduced case still violates the model.
    let findings = campaign.stored_findings(|f| {
        let minimal = aporia_minimize::minimize(
            &aporia_minimize::FailureOracle::new(model),
            model,
            &f.representative,
            aporia_minimize::Config {
                budget: plan.minimise_budget,
                ..aporia_minimize::Config::default()
            },
        );
        minimal.verified.then(|| minimal.case.describe())
    });
    let decisions: Vec<Json> = campaign
        .decisions
        .iter()
        .map(|d| {
            Json::object(vec![
                ("evaluation", Json::count(d.evaluation)),
                ("family", Json::text(format!("{:?}", d.family))),
                ("cell", Json::count(u64::from(d.cell))),
                ("risk", Json::number(d.risk)),
                ("gain", Json::number(d.gain)),
                (
                    "x",
                    Json::Arr(d.x.iter().map(|v| Json::number(*v)).collect()),
                ),
            ])
        })
        .collect();
    let bands = campaign.atlas.bands();
    let mut environment = Environment::current();
    environment.notes.push((
        "strategy".to_string(),
        crate::metrics::strategy_name(strategy).to_string(),
    ));
    environment
        .notes
        .push(("seed".to_string(), seed.to_string()));
    environment.notes.push(("entry".to_string(), entry.id()));
    let run = aporia_store::Run {
        model,
        model_text: &entry.source,
        air_text: &air,
        records: &campaign.records,
        config: Json::parse(&campaign_config_json(plan, strategy, seed)).unwrap_or(Json::Null),
        coverage: campaign.atlas.coverage(),
        atlas_csv: &campaign.atlas.to_csv(),
        bands: &bands,
        findings: &findings,
        decisions: &decisions,
        calibrator: &campaign.calibrator,
        correlation: &campaign.correlation,
        exec: aporia_runtime::ExecConfig::default(),
        evaluations: campaign.evaluations,
        instruction_steps: campaign.instruction_steps,
        environment,
        created_unix_ms: 0,
    };
    let _ = std::fs::remove_dir_all(dir);
    let mut store = Store::create(dir).map_err(|e| e.to_string())?;
    store.write(&run).map_err(|e| e.to_string())?;
    let replay = aporia_store::replay_dir(dir).map_err(|e| e.to_string())?;
    Ok((replay.reproduced, replay.matched, replay.total))
}

fn campaign_config_json(plan: &Plan, strategy: Strategy, seed: u64) -> String {
    format!(
        "{{\"budget\":{},\"strategy\":\"{}\",\"seed\":{},\"probe_every\":{},\"calibrate_every\":{},\"refine_every\":{},\"numerical_every\":{},\"differential_every\":{},\"symmetric_every\":{}}}",
        plan.budgets.last().copied().unwrap_or(0),
        crate::metrics::strategy_name(strategy),
        seed,
        plan.probe_every,
        plan.calibrate_every,
        plan.refine_every,
        plan.numerical_every,
        plan.differential_every,
        plan.symmetric_every,
    )
}

/// Run the whole corpus. Entries that do not compile are reported as skipped rather than measured.
#[must_use]
pub fn run_corpus(entries: &[Entry], plan: &Plan) -> Vec<Sweep> {
    let mut out = Vec::new();
    for entry in entries {
        if entry.truth.static_expected || entry.model.is_none() {
            continue;
        }
        for &seed in &plan.seeds {
            for &strategy in &plan.strategies {
                if let Some(sweep) = sweep(entry, plan, seed, strategy) {
                    out.push(sweep);
                }
            }
        }
    }
    out
}

impl Sweep {
    #[must_use]
    pub fn to_json(&self) -> Json {
        Json::object(vec![
            ("entry", Json::text(self.entry.clone())),
            ("strategy", Json::text(self.strategy)),
            ("seed", Json::count(self.seed)),
            (
                "detected_at_budget",
                self.detected_at.map_or(Json::Null, Json::count),
            ),
            (
                "localised_at_budget",
                self.localised_at.map_or(Json::Null, Json::count),
            ),
            (
                "clean_at_budget",
                self.clean_at.map_or(Json::Null, Json::count),
            ),
            (
                "control_suspicious_volume",
                self.control_suspicion.map_or(Json::Null, Json::number),
            ),
            (
                "replay",
                self.replay
                    .map_or(Json::Null, |(reproduced, matched, total)| {
                        Json::object(vec![
                            ("reproduced", Json::Bool(reproduced)),
                            ("matched", Json::count(matched)),
                            ("total", Json::count(total)),
                            (
                                "error",
                                self.replay_error.clone().map_or(Json::Null, Json::text),
                            ),
                        ])
                    }),
            ),
            (
                "outcomes",
                Json::Arr(self.outcomes.iter().map(Outcome::to_json).collect()),
            ),
        ])
    }

    /// One line per strategy for the console, in the order the plan lists budgets.
    #[must_use]
    pub fn row(&self) -> String {
        let fmt = |v: Option<u64>| v.map_or_else(|| "-".to_string(), |b| b.to_string());
        format!(
            "{:<34} {:<11} detect {:>6}  localise {:>6}  clean {:>6}  evals {:>6}  ms {:>7}",
            self.entry,
            self.strategy,
            fmt(self.detected_at),
            fmt(self.localised_at),
            fmt(self.clean_at),
            self.outcomes.last().map_or(0, |o| o.evaluations),
            self.outcomes.iter().map(|o| o.wall_ms).sum::<u64>(),
        )
    }
}

/// Everything a report needs, in one document, with the environment it was measured on.
#[must_use]
pub fn results_json(
    sweeps: &[Sweep],
    entries: &[Entry],
    plan: &Plan,
    environment: &Environment,
) -> Json {
    Json::object(vec![
        ("schema", Json::text("aporia.results/1")),
        (
            "question",
            Json::text(
                "can a computation-aware multi-evidence search discover and localise regions of \
                 distrust using fewer evaluations than simpler exploration strategies",
            ),
        ),
        (
            "plan",
            Json::object(vec![
                (
                    "budgets",
                    Json::Arr(plan.budgets.iter().map(|b| Json::count(*b)).collect()),
                ),
                (
                    "strategies",
                    Json::Arr(
                        plan.strategies
                            .iter()
                            .map(|s| Json::text(strategy_name(*s)))
                            .collect(),
                    ),
                ),
                (
                    "seeds",
                    Json::Arr(plan.seeds.iter().map(|s| Json::count(*s)).collect()),
                ),
                ("grid_per_axis", Json::count(plan.grid as u64)),
                ("minimise_budget", Json::count(plan.minimise_budget)),
                // The rates that cost evaluations are part of the definition of the measurement, so
                // they travel with its results: without them a reader cannot tell a channel that
                // changed nothing from a channel that was never sampled.
                ("probe_every", Json::count(plan.probe_every)),
                ("numerical_every", Json::count(plan.numerical_every)),
                ("differential_every", Json::count(plan.differential_every)),
                ("symmetric_every", Json::count(plan.symmetric_every)),
                ("refine_every", Json::count(plan.refine_every)),
                ("calibrate_every", Json::count(plan.calibrate_every)),
            ]),
        ),
        (
            "entries",
            Json::Arr(
                entries
                    .iter()
                    .map(|e| {
                        Json::object(vec![
                            ("entry", Json::text(e.id())),
                            ("fault", Json::text(e.truth.fault.clone())),
                            ("method", Json::text(e.truth.method.clone())),
                            ("control", Json::Bool(e.truth.control)),
                            (
                                "declared_regions",
                                Json::count(e.truth.regions.len() as u64),
                            ),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("environment", environment_json(environment)),
        (
            "sweeps",
            Json::Arr(sweeps.iter().map(Sweep::to_json).collect()),
        ),
        (
            "comparison",
            Json::Arr(crate::metrics::compare(
                &sweeps
                    .iter()
                    .flat_map(|s| s.outcomes.clone())
                    .collect::<Vec<_>>(),
            )),
        ),
    ])
}

fn environment_json(environment: &Environment) -> Json {
    Json::object(vec![
        ("os", Json::text(environment.os.clone())),
        ("arch", Json::text(environment.arch.clone())),
        (
            "cpu_features",
            Json::Arr(
                environment
                    .cpu_features
                    .iter()
                    .map(|f| Json::text(f.clone()))
                    .collect(),
            ),
        ),
        ("rust_channel", Json::text(environment.rust_channel.clone())),
        (
            "notes",
            Json::object(
                environment
                    .notes
                    .iter()
                    .map(|(k, v)| (k.as_str(), Json::text(v.clone())))
                    .collect(),
            ),
        ),
    ])
}

/// One run, opened up: what the atlas decided, what the bands said, and where the risk landed.
///
/// The summary table is the public artefact; this is the tool that finds out *why* a strategy did
/// or did not localise something, which is the difference between reporting a result and
/// understanding it.
#[must_use]
#[expect(
    clippy::too_many_lines,
    reason = "a report is a sequence of sections printed in order"
)]
pub fn explain(entry: &Entry, plan: &Plan, strategy: Strategy, seed: u64) -> Option<String> {
    use std::fmt::Write as _;
    let model = entry.model.as_ref()?;
    let budget = *plan.budgets.last().unwrap_or(&200);
    let cfg = plan.config(strategy, budget, seed);
    let campaign = run(model, cfg);
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{}  strategy {strategy:?}  budget {budget}  seed {seed}",
        entry.id()
    );
    let coverage = campaign.atlas.coverage();
    let _ = writeln!(
        out,
        "cells {} ({} leaves), suspicious {:.4} of the domain, trusted {:.4}, unknown {:.4}",
        coverage.cells,
        campaign.atlas.leaf_ids().len(),
        coverage.suspicious,
        coverage.trusted,
        coverage.unknown
    );
    let _ = writeln!(
        out,
        "evaluations {}  instruction steps {}",
        campaign.evaluations, campaign.instruction_steps
    );
    // The extra executions a campaign paid for have to be visible or the config lines are just
    // intentions: `symmetric_every 1` says the rate was set, the swap count says the swaps ran.
    let _ = writeln!(
        out,
        "probes: {} perturbation pairs, {} symmetry swaps",
        campaign.probes.pairs.len(),
        campaign.probes.swaps.len()
    );
    let _ = writeln!(
        out,
        "calibration fitted: {}",
        campaign.calibrator.describe()
    );
    let bands = campaign.atlas.bands();
    let _ = writeln!(out, "bands: {}", bands.len());
    for b in &bands {
        let _ = writeln!(
            out,
            "  axis {} span [{}, {}] facing [{}, {}] width {:.6}",
            b.axis,
            b.lo,
            b.hi,
            b.facing[0],
            b.facing[1],
            b.hi - b.lo
        );
    }
    for region in &entry.truth.regions {
        let fraction = entry.truth.volume_fraction(model, region);
        let detected = if let Some(at) = declared_axis(model, region) {
            bands
                .iter()
                .any(|b| b.axis as usize == at && band_covers(b, region))
        } else {
            false
        };
        let _ = writeln!(
            out,
            "declared region {:?} is {:.6} of the space, band on its boundary: {}",
            region.axes,
            fraction,
            if detected { "yes" } else { "no" }
        );
    }
    let _ = writeln!(out, "findings: {}", campaign.findings.len());
    for f in campaign.findings.iter().take(6) {
        let _ = writeln!(
            out,
            "  cell {} bounds {:?} online {:.3} final {:.3} samples {}",
            f.cell, f.bounds, f.online_risk, f.final_risk, f.samples
        );
        // The channels behind the first few findings, because "why is this suspicious" is the only
        // question that turns a bad result into a fixed one.
        for e in f.evidence.iter().take(6) {
            let _ = writeln!(
                out,
                "    {:<12} {:<18} magnitude {:>10.4} strength {:.3}  {}",
                e.channel.name(),
                e.subject.key(),
                e.magnitude,
                e.strength,
                e.detail
            );
        }
    }
    let mut buckets = [0u32; 10];
    for r in &campaign.final_risk {
        let i = (r.clamp(0.0, 0.999) * 10.0) as usize;
        buckets[i] += 1;
    }
    let _ = writeln!(out, "final risk histogram: {buckets:?}");
    let mut online = [0u32; 10];
    for r in &campaign.online_risk {
        let i = (r.clamp(0.0, 0.999) * 10.0) as usize;
        online[i] += 1;
    }
    let _ = writeln!(out, "online risk histogram: {online:?}");
    let _ = writeln!(out, "decisions by family:");
    let mut counts: std::collections::BTreeMap<String, u32> = std::collections::BTreeMap::new();
    for d in &campaign.decisions {
        *counts.entry(format!("{:?}", d.family)).or_insert(0) += 1;
    }
    for (family, n) in counts {
        let _ = writeln!(out, "  {family:<14} {n}");
    }
    Some(out)
}

fn declared_axis(model: &aporia_ir::Model, region: &crate::truth::Declared) -> Option<usize> {
    let (name, _) = region.axes.first()?;
    model.param(name).map(|p| p as usize)
}

fn band_covers(band: &aporia_boundary::Band, region: &crate::truth::Declared) -> bool {
    let name = band.axis.to_string();
    let _ = name;
    // A band is per-axis; a region constrains named axes by index, so the caller compares the
    // numeric axis against each declared span it cares about.
    region
        .axes
        .iter()
        .any(|(_, [lo, hi])| band.facing[1] >= *lo && band.facing[0] <= *hi)
}

/// What the measurements say, as text a person can read in a terminal.
#[must_use]
pub fn verdict(sweeps: &[Sweep], truth_of: &dyn Fn(&str) -> Option<Truth>) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let mut wins = 0u32;
    let mut losses = 0u32;
    let mut ties = 0u32;
    for entry in sweeps
        .iter()
        .map(|s| s.entry.clone())
        .collect::<std::collections::HashSet<_>>()
    {
        let Some(truth) = truth_of(&entry) else {
            continue;
        };
        if truth.control || truth.regions.is_empty() {
            continue;
        }
        // Minimum over the seeds that resolved, not over `Option` values: `Some(320).min(None)` is
        // `None`, so a strategy that worked on two of three seeds was being reported as unresolved
        // beside a baseline that never worked at all. The seed counts are printed because "1 of 3
        // at 320" and "3 of 3 at 320" are not the same evidence.
        let best = |name: &str| {
            let group: Vec<&Sweep> = sweeps
                .iter()
                .filter(|s| s.entry == entry && s.strategy == name)
                .collect();
            let resolved = group.iter().filter(|s| s.localised_at.is_some()).count();
            let at = group.iter().filter_map(|s| s.localised_at).min();
            (at, resolved, group.len())
        };
        let (adaptive, a_ok, a_n) = best("adaptive");
        let (stratified, s_ok, s_n) = best("stratified");
        let (random, r_ok, _) = best("random");
        let baseline = [stratified, random].into_iter().flatten().min();
        let _ = writeln!(
            out,
            "{entry}: adaptive {adaptive:?} ({a_ok}/{a_n}) | baseline {baseline:?}              | stratified {stratified:?} ({s_ok}/{s_n}) | random {random:?} ({r_ok})"
        );
        match (adaptive, baseline) {
            (None, None) => {
                let _ = writeln!(
                    out,
                    "    nothing localised every declared region in the ladder"
                );
                ties += 1;
            }
            (Some(_), None) => {
                let _ = writeln!(
                    out,
                    "    adaptive resolved within the ladder, neither baseline did"
                );
                wins += 1;
            }
            (None, Some(_)) => {
                let _ = writeln!(out, "    a baseline resolved and adaptive did not");
                losses += 1;
            }
            (Some(a), Some(b)) => match a.cmp(&b) {
                std::cmp::Ordering::Less => wins += 1,
                std::cmp::Ordering::Greater => losses += 1,
                std::cmp::Ordering::Equal => ties += 1,
            },
        }
    }
    let _ = writeln!(
        out,
        "adaptive better on {wins} entries, worse on {losses}, equal or unresolved on {ties}"
    );
    out
}
