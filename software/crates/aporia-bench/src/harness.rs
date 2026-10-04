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
    Some(Sweep {
        entry: entry.id(),
        strategy: strategy_name(strategy),
        seed,
        outcomes,
        detected_at,
        localised_at,
        clean_at,
        control_suspicion,
    })
}

fn strategy_name(s: Strategy) -> &'static str {
    match s {
        Strategy::Random => "random",
        Strategy::Stratified => "stratified",
        Strategy::Adaptive => "adaptive",
    }
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
        let best = |name: &str| {
            sweeps
                .iter()
                .filter(|s| s.entry == entry && s.strategy == name)
                .filter_map(|s| s.localised_at)
                .min()
        };
        let (Some(adaptive), Some(baseline)) =
            (best("adaptive"), best("stratified").min(best("random")))
        else {
            let _ = writeln!(
                out,
                "{entry}: no strategy localised every declared region within the plan's budgets"
            );
            ties += 1;
            continue;
        };
        let _ = writeln!(
            out,
            "{entry}: adaptive localised at {adaptive}, best baseline at {baseline}"
        );
        match adaptive.cmp(&baseline) {
            std::cmp::Ordering::Less => wins += 1,
            std::cmp::Ordering::Greater => losses += 1,
            std::cmp::Ordering::Equal => ties += 1,
        }
    }
    let _ = writeln!(
        out,
        "adaptive better on {wins} entries, worse on {losses}, equal or unresolved on {ties}"
    );
    out
}
