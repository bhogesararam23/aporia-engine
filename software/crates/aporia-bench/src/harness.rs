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
use aporia_boundary::Trust;
use aporia_search::{Config, Strategy, run, run_with};
use aporia_store::Environment;
use aporia_store::Json;
use std::time::Instant;

/// What a run *was*, as distinct from what it measured: the pre-registration it came from, and the
/// identity its plan and corpus digest to.
///
/// This is carried on the plan because an archive has to be recognisable on its own. A directory of
/// findings that cannot say which measurement it belongs to is why two experiments could write into one
/// namespace until a pre-run guard was bolted on; the guard can only be exact if the bytes it compares
/// name the measurement rather than describing the machine. It is deliberately *not* part of the
/// measurement identity: `plan_json` writes none of these fields, so recording which protocol produced a
/// run cannot rename the run.
#[derive(Clone, Debug, Default)]
pub struct Provenance {
    /// The frozen file's `id`, when the run came from one.
    pub protocol: Option<String>,
    /// Which arm of it. An arm is a measurement, and a directory that cannot say which arm it holds
    /// cannot refuse a second arm of the same protocol into the same place.
    pub arm: Option<String>,
    /// The digest of the protocol file's bytes, so an edit to a freeze shows up in an archive written
    /// afterwards rather than in a reader's memory of what the file said.
    pub protocol_digest: Option<String>,
    /// Whether this document was produced by `--pilot` — the freeze's own reduced scope rather than the
    /// experiment. Written only when true, so a full run's bytes are unchanged by the existence of the
    /// option: a pilot document is a document about the plumbing, and the field exists for the reader
    /// who would otherwise mistake one for the measurement.
    pub pilot: bool,
    /// The identity a results document from this plan and this entry list would carry. Recorded per
    /// archive because it is the one value that says "these bytes are that measurement".
    pub measurement_identity: Option<String>,
}

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
    /// Evidence channels whose readings this plan drops, for an ablation arm.
    ///
    /// Empty by default, which is the full instrument. The list is part of `to_json`, so it is part of
    /// the measurement identity: an ablation arm cannot overwrite the arm it was compared against, and
    /// a results file says which channels its numbers were produced with. Recorded as names rather
    /// than as a mask for the same reason the campaign records them as names — an archive is read by a
    /// person too.
    pub ablate: Vec<aporia_evidence::Channel>,
    /// Which trust rule this plan labels under.
    ///
    /// A plan field rather than a CLI flag, because the rule decides what a row means: two runs of one
    /// corpus whose leaves were trusted on different evidence are two experiments, and a variant that
    /// only existed in the arguments a person typed would leave their results documents identical in
    /// every field that names a measurement. [`Trust::Current`] is the policy that produced every
    /// committed number here, and [`plan_json`] writes no field for it, which is the only way both
    /// halves of the requirement can hold at once: the default reproducing its own identity, and a
    /// variant naming itself.
    pub trust: Trust,
    /// Which rule this plan applies to a leaf's *scale* before certifying it. See
    /// [`aporia_boundary::Resolution`].
    ///
    /// A plan field for the same reason [`Plan::trust`] is one, and with the same obligation: the rule
    /// changes what a row means, so it travels into the recorded configuration, into the measurement
    /// identity whenever it is not the default, and into the archive directory name. E1.3b is the
    /// precedent — the one arm that reached zero certified-over-true volume got there by trusting
    /// nothing at all, and that would have been indistinguishable from an arm that reached zero by
    /// finding the violation, had the rule not been in the bytes that name the measurement.
    pub resolution: aporia_boundary::Resolution,
    /// The scale certification is withheld below, as a fraction of the root cell's volume. Read only
    /// when [`Plan::resolution`] is not `Current`, and part of the identity whenever it is: two arms
    /// that hold themselves to different resolutions are not the same experiment, and a bar that
    /// travelled nowhere could be moved after the run without any document noticing.
    pub certify_share: f64,
    /// Which quantity [`Plan::resolution`] measures a leaf's adequacy against: its scale or the density
    /// of evidence inside it. See [`aporia_boundary::Certify`].
    ///
    /// The same obligation as the two axes above it, and for the same reason: it changes what a row
    /// claims, so it is recorded in the configuration, in the identity whenever it is not the default,
    /// and in the archive directory. `Certify::Size` is the reading every committed row was labelled
    /// with, and an E1.5 arm that withholds on density is a different measurement from an E1.4 arm that
    /// withheld on size even when the two agree leaf for leaf.
    pub certify: aporia_boundary::Certify,
    /// The evaluations per domain-volume a leaf must hold to be certified under [`Plan::certify`] =
    /// [`aporia_boundary::Certify::Density`]. Part of the identity whenever the plan is not the
    /// committed one, exactly as [`Plan::certify_share`] is: a bar that travelled nowhere could be
    /// moved after the run without any document noticing.
    pub certify_density: f64,
    /// Where per-entry archives go. Left empty, the harness skips archiving: the archives are the
    /// expensive part of a run and their numbers do not change the search.
    pub archive_dir: std::path::PathBuf,
    /// Which pre-registration and which measurement this plan's archives belong to. Never written into
    /// the plan section, so it cannot move an identity: it is what a directory is recognised by, not
    /// what a row means.
    pub provenance: Provenance,
}

impl Default for Plan {
    fn default() -> Self {
        Self {
            budgets: vec![60, 120, 250, 500, 1000],
            // E1's four arms. Adding the level-set baseline changes what a default plan measures, and
            // therefore its measurement identity: a rerun of the ladder cannot overwrite the runs that
            // were measured before the competitor existed, and the name says so.
            strategies: vec![
                Strategy::Adaptive,
                Strategy::LevelSet,
                Strategy::Stratified,
                Strategy::Random,
            ],
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
            // The full instrument: an ablation arm names the channels it drops.
            ablate: Vec::new(),
            // Today's rule, the one every committed measurement in `benchmarks/results/` was labelled
            // under. A plan that wants another variant says so, and its identity says so with it.
            trust: Trust::Current,
            // The semantics every committed row in `benchmarks/results/` was labelled under, and the
            // bar it would have been measured against had any of them asked the question. Both are
            // inert here: the bar is read only by a rule that is switched off.
            resolution: aporia_boundary::Resolution::Current,
            certify_share: aporia_boundary::Policy::default().certify_share,
            certify: aporia_boundary::Certify::Size,
            certify_density: aporia_boundary::Policy::default().certify_density,
            archive_dir: std::path::PathBuf::new(),
            provenance: Provenance::default(),
        }
    }
}

impl Plan {
    /// The campaign configuration this plan runs: the one place a plan becomes the thing that measures.
    ///
    /// Public because a test that wants the real ladder otherwise hand-copies these rates into a
    /// `Config`, and a copy is a second definition of the experiment that silently drifts from the
    /// first. A caller that wants to know what a variant does to a campaign asks *this* function, so the
    /// answer is the one a run would have produced.
    pub fn config(&self, strategy: Strategy, budget: u64, seed: u64) -> Config {
        // The trust rule the plan names, materialised onto the default thresholds. Under
        // `Trust::Current` this is `Policy::default()` exactly, which is what makes the default variant
        // a reproduction rather than a re-implementation.
        let mut policy = self.trust.policy();
        // The second axis a plan may move, set in the same place for the same reason: this is where a
        // plan becomes the thing that measures, so a rule that reached the campaign by another route
        // would be a second definition of the experiment. Under [`aporia_boundary::Resolution::Current`]
        // both fields hold what `Policy::default()` already holds, which is why a default plan still
        // writes the bytes that named a committed measurement.
        policy.resolution = self.resolution;
        policy.certify_share = self.certify_share;
        policy.certify = self.certify;
        policy.certify_density = self.certify_density;
        Config {
            budget,
            strategy,
            seed,
            probe_every: self.probe_every,
            policy,
            calibrate_every: self.calibrate_every,
            numerical_every: self.numerical_every,
            symmetric_every: self.symmetric_every,
            differential_every: self.differential_every,
            refine_every: self.refine_every,
            max_steps_per_evaluation: 2_000_000,
            // The ablation arm, applied to the campaign's configuration and nowhere else: it silences
            // readings, never evaluations, so every arm of a comparison runs the same points at the
            // same cost. See `aporia_search::Config::silenced`.
            silenced: self.ablated(),
        }
    }

    /// The channel mask for this plan's ablation list.
    #[must_use]
    pub fn ablated(&self) -> u8 {
        let mut mask = 0u8;
        for channel in &self.ablate {
            mask |= 1u8 << channel.index();
        }
        mask
    }

    /// Read a plan back from the JSON [`plan_json`] writes.
    ///
    /// Round-tripping matters because the frozen protocol files in `benchmarks/protocols/` are the
    /// definition an experiment runs from, and a reader that accepts a partial object would let a
    /// typo (`"budget"`, `"seed"`, `"caliberate_every"`) silently fall back to a default — which is
    /// a measurement of an experiment nobody specified. So every field is required by name and every
    /// unknown field is refused. `archive_dir` is not in the document, because where a run writes is
    /// not part of what it measures.
    #[expect(
        clippy::too_many_lines,
        reason = "one strict reader for one object: the closures that read a count, a list and a name \
                  are the refusal machinery, and lifting the field list out of this function would put \
                  the requirement that every field is named somewhere other than where the fields are \
                  read"
    )]
    pub fn from_json(value: &Json) -> Result<Self, String> {
        let Json::Obj(fields) = value else {
            return Err("a plan must be an object".to_string());
        };
        let known = [
            "budgets",
            "strategies",
            "seeds",
            "grid_per_axis",
            "minimise_budget",
            "ablate",
            "probe_every",
            "numerical_every",
            "differential_every",
            "symmetric_every",
            "refine_every",
            "calibrate_every",
            "trust",
            "resolution",
            "certify_share",
            "certify",
            "certify_density",
        ];
        for (name, _) in fields {
            if !known.contains(&name.as_str()) {
                return Err(format!("plan: unknown field {name:?}"));
            }
        }
        let at = |key: &str| -> Result<&Json, String> {
            fields
                .iter()
                .find(|(name, _)| name.as_str() == key)
                .map(|(_, value)| value)
                .ok_or_else(|| format!("plan: {key} is missing"))
        };
        let count = |key: &str| -> Result<u64, String> {
            at(key)?
                .as_u64()
                .ok_or_else(|| format!("plan: {key} needs a whole number"))
        };
        let items = |key: &str| -> Result<&[Json], String> {
            at(key)?
                .as_array()
                .ok_or_else(|| format!("plan: {key} needs a list"))
        };
        let list = |key: &str| -> Result<Vec<u64>, String> {
            items(key)?
                .iter()
                .map(|item| {
                    item.as_u64()
                        .ok_or_else(|| format!("plan: {key} holds {}", item.to_compact()))
                })
                .collect()
        };
        let names = |key: &str| -> Result<Vec<String>, String> {
            items(key)?
                .iter()
                .map(|item| {
                    item.as_str()
                        .map(str::to_string)
                        .ok_or_else(|| format!("plan: {key} holds {}", item.to_compact()))
                })
                .collect()
        };
        let budgets = list("budgets")?;
        if budgets.is_empty() || budgets.contains(&0) {
            return Err("plan: budgets must be non-empty and every budget above zero".to_string());
        }
        let strategies = plan_strategies(&names("strategies")?)?;
        let seeds = list("seeds")?;
        if seeds.is_empty() {
            return Err("plan: seeds must be non-empty".to_string());
        }
        let grid = count("grid_per_axis")?;
        if grid < 3 {
            return Err(
                "plan: grid_per_axis must be at least 3 so the domain edges are covered"
                    .to_string(),
            );
        }
        let ablate = plan_channels(&names("ablate")?, "plan")?;
        // The one field this reader accepts in its absence, and the reason is what the reader reads:
        // published documents as well as protocol files. Every results file in `benchmarks/results/`
        // predates named variants, and none of them could have been labelled under any rule but the
        // current one, so absence there is a fact about history rather than a value to be guessed. A
        // name it does not know is refused, because a document whose rule is a typo is a document whose
        // measurement is unknown. Where a file *is* obliged to state a rule is a frozen
        // [`Protocol::SCHEMA_V2`] arm, in [`Protocol::from_json`] — a pre-registration has no history to
        // fall back on.
        let trust = match at("trust") {
            Err(_) => Trust::Current,
            Ok(value) => {
                let text = value
                    .as_str()
                    .ok_or_else(|| "plan: trust needs a variant name".to_string())?;
                Trust::parse(text).ok_or_else(|| {
                    format!(
                        "plan: unknown trust variant {text:?}; this tool knows {}",
                        Trust::names().join(", ")
                    )
                })?
            }
        };
        // The second rule a plan may name, read the same way and for the same reason: absence means the
        // semantics that produced every committed document in this repository, presence means a name
        // this tool has to know. A pre-registration is obliged to state it, in
        // [`Protocol::SCHEMA_V3`].
        let resolution = match at("resolution") {
            Err(_) => aporia_boundary::Resolution::Current,
            Ok(value) => {
                let text = value
                    .as_str()
                    .ok_or_else(|| "plan: resolution needs a variant name".to_string())?;
                aporia_boundary::Resolution::parse(text).ok_or_else(|| {
                    format!(
                        "plan: unknown resolution variant {text:?}; this tool knows {}",
                        aporia_boundary::Resolution::names().join(", ")
                    )
                })?
            }
        };
        // A fraction rather than a name, and read even when the rule above leaves it inert: a plan that
        // records the scale it was measured against says nothing different when the rule is off, and
        // cannot be re-read afterwards as a different experiment when it is not.
        let certify_share = match at("certify_share") {
            Err(_) => aporia_boundary::Policy::default().certify_share,
            Ok(value) => value
                .as_f64()
                .filter(|share| *share > 0.0 && *share <= 1.0)
                .ok_or_else(|| {
                    "plan: certify_share needs a fraction of the root cell, in (0, 1]".to_string()
                })?,
        };
        // The criterion the scale rule measures adequacy against. Absent means `Size`, which is the
        // reading every committed row was labelled with, and an unknown name is a refusal rather than
        // a fallback for the reason given above.
        let certify = match at("certify") {
            Err(_) => aporia_boundary::Certify::Size,
            Ok(value) => {
                let text = value
                    .as_str()
                    .ok_or_else(|| "plan: certify needs a criterion name".to_string())?;
                aporia_boundary::Certify::parse(text).ok_or_else(|| {
                    format!(
                        "plan: unknown certify criterion {text:?}; this tool knows {}",
                        aporia_boundary::Certify::names().join(", ")
                    )
                })?
            }
        };
        // Read even when the criterion above is the inert one, for the same reason the share is: a
        // plan that records the bar it was measured against cannot be re-read afterwards as a
        // different experiment.
        let certify_density = match at("certify_density") {
            Err(_) => aporia_boundary::Policy::default().certify_density,
            Ok(value) => value
                .as_f64()
                .filter(|density| *density > 0.0 && density.is_finite())
                .ok_or_else(|| {
                    "plan: certify_density needs evaluations per domain volume, a finite number \
                     above zero"
                        .to_string()
                })?,
        };
        Ok(Self {
            budgets,
            strategies,
            seeds,
            grid: grid as usize,
            minimise_budget: count("minimise_budget")?,
            probe_every: count("probe_every")?,
            calibrate_every: count("calibrate_every")?,
            refine_every: count("refine_every")?,
            numerical_every: count("numerical_every")?,
            symmetric_every: count("symmetric_every")?,
            differential_every: count("differential_every")?,
            ablate,
            trust,
            resolution,
            certify_share,
            certify,
            certify_density,
            archive_dir: std::path::PathBuf::new(),
            // Provenance is not readable from a plan section and must not be defaulted from one: a
            // document that claimed a protocol it was never run under would then be believed.
            provenance: Provenance::default(),
        })
    }
}

/// The strategy names a plan section may carry, refused by name rather than defaulted.
///
/// This is deliberately a second, stricter reader beside `cli::parse_strategies`: a command line can
/// accept `--strategies adaptive,adaptive` as a no-op, but a frozen protocol file that repeats an arm
/// is a file that does not say what it measures, so the plan reader checks emptiness here rather than
/// leaving it to the caller.
fn plan_strategies(names: &[String]) -> Result<Vec<Strategy>, String> {
    let out = names
        .iter()
        .map(|text| {
            Strategy::parse(text).ok_or_else(|| {
                format!(
                    "plan: unknown strategy {text:?}; this tool knows {}",
                    Strategy::names().join(", ")
                )
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    if out.is_empty() {
        return Err("plan: strategies must name at least one strategy".to_string());
    }
    if out.len() != names.len() {
        return Err("plan: strategies repeats a strategy".to_string());
    }
    Ok(out)
}

/// The channel names an arm may silence, by the vocabulary of the type that owns them.
///
/// The rules a frozen file has to satisfy are stricter than the command line's: a repeated name is a
/// defect in the file rather than a no-op, and an arm that silences all five channels would report a
/// domain of TRUSTED cells built on nothing, which is why 0034 refused it at the CLI and why the
/// reader refuses it here too.
fn plan_channels(names: &[String], site: &str) -> Result<Vec<aporia_evidence::Channel>, String> {
    let mut out = Vec::new();
    for text in names {
        let Some(channel) = aporia_evidence::Channel::parse(text) else {
            return Err(format!(
                "{site} does not know the channel {text:?}; the names are {}",
                aporia_evidence::Channel::names().join(", ")
            ));
        };
        if out.contains(&channel) {
            return Err(format!("{site} silences the same channel twice"));
        }
        out.push(channel);
    }
    if out.len() == aporia_evidence::Channel::ALL.len() {
        return Err(format!(
            "{site} silences every channel, and an instrument with no evidence has nothing to \
             search on"
        ));
    }
    Ok(out)
}

/// One arm of a [`Protocol`]: a name, the channels that arm was blind to, and the trust rule that
/// labelled its leaves.
#[derive(Clone, Debug)]
pub struct ProtocolArm {
    pub name: String,
    pub ablate: Vec<aporia_evidence::Channel>,
    /// Which rule decided what this arm was allowed to call trusted.
    ///
    /// Carried per arm and not per plan under [`Protocol::SCHEMA_V2`], exactly as the mask is, because
    /// the arms of this protocol are a grid of two declared axes: an arm that did not state its trust
    /// rule would be a different measurement from the arm beside it and would say so nowhere in the
    /// file that froze it.
    pub trust: Trust,
    /// Which scale rule this arm certified under, carried per arm under [`Protocol::SCHEMA_V3`] for the
    /// same reason the mask and the trust rule are: the arms of this protocol are a grid of declared
    /// axes, and an arm that did not state its resolution rule would be a different measurement from
    /// the one beside it and would say so nowhere in the file that froze it.
    ///
    /// `Current` under both earlier schemas, which is what makes those files keep meaning what they
    /// said.
    pub resolution: aporia_boundary::Resolution,
    /// Which quantity this arm's scale rule measures adequacy against, carried per arm under
    /// [`Protocol::SCHEMA_V4`] for the same reason the two axes above it are.
    ///
    /// [`aporia_boundary::Certify::Size`] under every earlier schema, which is what makes those files
    /// keep meaning what they said: E1.4's four arms withheld and demanded on *scale*, and no later
    /// reader can re-label those bytes as a density experiment.
    pub certify: aporia_boundary::Certify,
}

/// The scope the infrastructure pilot may run, as the freeze that owns it wrote it.
///
/// Every field is a *subset* check against the frozen plan and the frozen entry lists, which is what
/// makes `--pilot` safe next to a protocol that refuses every other flag shaping a measurement: a pilot
/// can be smaller than its experiment in exactly the four dimensions the experiment already declared,
/// and in nothing else. `checks` names what the pilot establishes — determinism, namespace separation,
/// equal charged cost, replay — so that a run which measured something else cannot be presented as the
/// pilot this file asked for.
#[derive(Clone, Debug)]
pub struct Pilot {
    pub arms: Vec<String>,
    pub entries: Vec<String>,
    pub budgets: Vec<u64>,
    pub seeds: Vec<u64>,
    pub checks: Vec<String>,
}

/// Who produced a measurement, when it came from a frozen protocol file rather than from a command
/// line. Recorded in the results document and deliberately *not* in the measurement identity: the
/// identity is the plan and the corpus, and two protocol files that state the same plan measure the
/// same experiment.
#[derive(Clone, Debug)]
pub struct Frozen {
    pub id: String,
    pub arm: String,
    pub question: String,
    /// The digest of the protocol *file* this arm came from, or `None` when the protocol was
    /// assembled from a value in memory and has no bytes to point at. Provenance rather than
    /// identity: `id` and `arm` say which pre-registration was followed, and this says which
    /// **version** of it — the deciding fields of a pre-registration live in prose the identity
    /// deliberately does not hash, so without a digest a metric edited after the run would be
    /// indistinguishable from a metric that was frozen.
    pub protocol_digest: Option<String>,
    /// Whether the run was the freeze's own pilot rather than the experiment. `--pilot` sets it; a
    /// protocol's `arm()` never does, so a document can only claim to be a pilot if the command that
    /// wrote it asked for the reduced scope by name.
    pub pilot: bool,
}

/// An experiment specified in full before its corpus or its arms existed, as
/// `benchmarks/protocols/<id>.json` holds it and as the runner reads it.
///
/// The point of making the protocol a file the runner *must* read is that a pre-registration nobody
/// executes is decoration: with `--plan`, the frozen budgets, seeds, rates, entry list and arm masks
/// cannot drift from the run that cites them, and a results document says which protocol and which
/// arm wrote it.
///
/// Strictness is asymmetric on purpose. Inside `plan` every field is required and unknown fields are
/// refused, because a silent default there changes what is measured. At the top level the reader
/// requires the sections that decide a result — the arms, the entries, and the primary metric with
/// its decision rule — and ignores the prose that explains them.
#[derive(Clone, Debug)]
pub struct Protocol {
    /// Which schema this file declared, kept because what a file is *obliged* to state is a property of
    /// its version and a reader — or a test — has to be able to ask which obligations this one carries.
    /// It is not in the measurement identity: two protocol files that state the same plan measure the
    /// same experiment whatever they call themselves.
    pub schema: String,
    pub id: String,
    pub question: String,
    pub plan: Plan,
    pub arms: Vec<ProtocolArm>,
    pub entries: Vec<String>,
    /// Which of them the file declares resolvable, and which sub-granular, as an
    /// [`Protocol::SCHEMA_V2`] file must say. Empty for a schema-1 file, whose meaning predates the
    /// distinction and whose committed rows must not be re-sorted by a rule invented afterwards.
    ///
    /// The instrument checks the two lists against its own reachability arithmetic at load time, in one
    /// direction: an entry named resolvable that the atlas cannot enclose is refused, because that is
    /// the mistake 0038 found — a condition scored on rows the partition could never have resolved. The
    /// other direction is *not* refused. An entry the file expects to be unresolvable and the instrument
    /// finds enclosed is a finding about 0038's account, and refusing the run for it would be the
    /// experiment editing its own control set to keep the story.
    pub resolvable: Vec<String>,
    pub sub_granular: Vec<String>,
    /// Which entries the file declares controls, as a [`Protocol::SCHEMA_V3`] file must say.
    ///
    /// Empty under the earlier schemas. The set exists because E1.3b froze a fourth quantity — control
    /// cleanliness, suspicious volume on an entry that declares no region — and then ran a list of
    /// entries none of which was marked as a control, so the metric had no rows and could only be
    /// reported as unmeasurable. A `/3` file is refused if it cannot name one, because a quantity that
    /// its own run list cannot produce was never a quantity to freeze in the first place.
    ///
    /// Checked against the corpus as well as against the run list: an entry in this set must carry
    /// `control: true` and no declared region, or it is not a control in the sense the metric means.
    pub control: Vec<String>,
    /// The reduced scope the infrastructure pilot is permitted to run, `Some` only under
    /// [`Protocol::SCHEMA_V4`]. See [`Pilot`].
    pub pilot: Option<Pilot>,
    /// The first 16 hex digits of the SHA-256 of the file's bytes, set by `load` and absent for a
    /// protocol built by `from_json`, which has no file. It travels into every results document that
    /// arm writes, so an edit to a frozen pre-registration is visible as a different digest rather
    /// than as a silent change of meaning.
    pub digest: Option<String>,
}

impl Protocol {
    pub const SCHEMA: &'static str = "aporia.protocol/1";
    /// A protocol whose arms state a trust rule beside a channel mask.
    ///
    /// A second version rather than a looser reader, because the difference is in what a file is
    /// *obliged* to say: an `/1` arm states a mask and inherits the rule every committed measurement
    /// used, and the frozen files of that shape must keep meaning what they said. A `/2` arm states
    /// both, and an arm that omits its rule is refused by name. Promoting a file from one to the other
    /// is an edit, and an edit changes the file's digest, so a pre-registration cannot acquire an axis
    /// afterwards without that being visible in every document that cites it.
    pub const SCHEMA_V2: &'static str = "aporia.protocol/2";

    /// A protocol whose arms state a resolution rule beside their mask and trust rule, and whose
    /// `entries` section names a control set.
    ///
    /// `/2` taught this by absence: it froze a control-cleanliness metric and then measured eight
    /// entries, none of which was marked control, so the quantity had no rows and could only be
    /// reported as unmeasurable. A `/3` file cannot be written in that shape — `entries.control` is
    /// required and non-empty — and an arm that does not say which scale rule it certifies under is
    /// refused, for the same reason an arm that did not say which channels it was blind to was refused.
    pub const SCHEMA_V3: &'static str = "aporia.protocol/3";

    /// A protocol that freezes its own pilot: the reduced scope the infrastructure check is permitted
    /// to run, written into the pre-registration rather than left to a command line.
    ///
    /// `/3` learned this the expensive way. Its stopping rule said the pilot was "one entry, two seeds
    /// and the two smallest budgets", and its runner refuses every flag that shapes what is measured
    /// beside `--plan` — so the pilot had no way to be smaller than the experiment, ran the full plan,
    /// and the deviation was recorded in the analysis as a defect of the freeze rather than of the run.
    /// A `/4` file states the scope, and `--pilot` reads it from the file: the only freedom a pilot
    /// gets is to be smaller, and it is bounded by what this schema has already frozen.
    pub const SCHEMA_V4: &'static str = "aporia.protocol/4";

    /// The schemas this reader accepts, in the order a refusal names them.
    pub const SCHEMAS: [&'static str; 4] = [
        Self::SCHEMA,
        Self::SCHEMA_V2,
        Self::SCHEMA_V3,
        Self::SCHEMA_V4,
    ];

    /// Read a protocol from a file, naming the path in every refusal.
    pub fn load(path: &std::path::Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let value =
            aporia_store::Json::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        let digest = aporia_store::digest::sha256_hex(text.as_bytes())[..16].to_string();
        Self::from_json(&value)
            .map(|protocol| Protocol {
                digest: Some(digest),
                ..protocol
            })
            .map_err(|e| format!("{}: {e}", path.display()))
    }

    #[must_use]
    pub fn arm_names(&self) -> Vec<&str> {
        self.arms.iter().map(|a| a.name.as_str()).collect()
    }

    /// The plan for one named arm. An arm is chosen by name and never defaulted: a run that silently
    /// used the full instrument when the protocol lists seven arms is the defect this refuses.
    pub fn arm(&self, name: &str) -> Result<(Plan, Frozen), String> {
        let arm = self.arms.iter().find(|a| a.name == name).ok_or_else(|| {
            format!(
                "protocol {:?} has no arm {name:?}; it has {}",
                self.id,
                self.arm_names().join(", ")
            )
        })?;
        let mut plan = self.plan.clone();
        plan.ablate.clone_from(&arm.ablate);
        // The second axis an arm may move, set in the same place and for the same reason as the first.
        plan.trust = arm.trust;
        // The third, and the one E1.4 varies: the bar itself stays the plan's, so four arms certify
        // against one scale and a difference between them cannot be a difference in what "coarse" means.
        plan.resolution = arm.resolution;
        // The fourth axis an arm may move, with both bars still the plan's: the arms hold themselves to
        // one scale and one density, so a difference between them cannot be a difference in what the
        // bar was.
        plan.certify = arm.certify;
        Ok((
            plan,
            Frozen {
                id: self.id.clone(),
                arm: arm.name.clone(),
                question: self.question.clone(),
                protocol_digest: self.digest.clone(),
                pilot: false,
            },
        ))
    }

    /// The same arm at the scope this file froze for its pilot.
    ///
    /// A method on the protocol rather than arithmetic in the CLI, because "smaller, and only in the
    /// four dimensions the freeze already declared" is part of what the pre-registration means and has
    /// to be checkable without running something. Returns the reduced plan and the entry list to run,
    /// with [`Frozen::pilot`] set so the document says what it is.
    pub fn pilot(&self, arm_name: &str) -> Result<(Plan, Vec<String>, Frozen), String> {
        let pilot = self.pilot.as_ref().ok_or_else(|| {
            format!(
                "protocol {:?} freezes no pilot scope, so there is nothing for --pilot to run",
                self.id
            )
        })?;
        let (mut plan, mut frozen) = self.arm(arm_name)?;
        if !pilot.arms.iter().any(|name| name == arm_name) {
            return Err(format!(
                "protocol {:?} does not include arm {arm_name:?} in its pilot, which names {}",
                self.id,
                pilot.arms.join(", ")
            ));
        }
        plan.budgets.clone_from(&pilot.budgets);
        plan.seeds.clone_from(&pilot.seeds);
        frozen.pilot = true;
        Ok((plan, pilot.entries.clone(), frozen))
    }

    #[expect(
        clippy::too_many_lines,
        reason = "a pre-registration is one object read in one place: the schema gate, the plan \
                  section, the arms with their two axes, the entry lists and the deciding metric have \
                  to be checked against each other here, because a refusal that lives in another \
                  function is a rule a later edit can drop without noticing"
    )]
    pub fn from_json(value: &Json) -> Result<Self, String> {
        let schema = value
            .get("schema")
            .and_then(Json::as_str)
            .unwrap_or("<missing>");
        if !Self::SCHEMAS.contains(&schema) {
            return Err(format!(
                "unrecognised protocol schema {schema:?}; this tool reads {}",
                Self::SCHEMAS.join(" and ")
            ));
        }
        let v2 = schema == Self::SCHEMA_V2;
        let v3 = schema == Self::SCHEMA_V3;
        let v4 = schema == Self::SCHEMA_V4;
        // The two schemas whose arms each state their own trust rule. `/3` and `/4` inherit the
        // obligation rather than relaxing it: a file that adds an axis is not allowed to forget one.
        let rules_per_arm = v2 || v3 || v4;
        // The scale axis and the control set are obliged of every schema from `/3` up, so `/4` reads
        // them through the same checks rather than through a second copy that could drift.
        let scale_rules = v3 || v4;
        let text = |key: &str| -> Result<String, String> {
            value
                .get(key)
                .and_then(Json::as_str)
                .map(str::to_string)
                .ok_or_else(|| format!("protocol: {key} is missing"))
        };
        let plan_value = value
            .get("plan")
            .ok_or_else(|| "protocol: plan is missing".to_string())?;
        if plan_value.get("ablate").is_some() {
            return Err(
                "protocol: the plan section does not carry ablate — each arm states its own mask"
                    .to_string(),
            );
        }
        if rules_per_arm && plan_value.get("trust").is_some() {
            return Err(format!(
                "protocol: a {} plan section does not carry trust — each arm states its own trust \
                 rule, beside its mask",
                Self::SCHEMA_V2
            ));
        }
        if scale_rules && plan_value.get("resolution").is_some() {
            return Err(format!(
                "protocol: a {} plan section does not carry resolution — each arm states its own \
                 scale rule, beside its mask and its trust rule",
                Self::SCHEMA_V3
            ));
        }
        if v4 && plan_value.get("certify").is_some() {
            return Err(format!(
 "protocol: a {} plan section does not carry certify — each arm states its own adequacy criterion, beside its mask, its trust rule and its scale rule",
                Self::SCHEMA_V4
            ));
        }
        if v4 && plan_value.get("certify_density").is_none() {
            return Err(format!(
 "protocol: a {} plan must state certify_density, because the density its arms hold themselves to is one number for the whole experiment, and a bar that travelled nowhere could be moved after the run without any document noticing",
                Self::SCHEMA_V4
            ));
        }
        if scale_rules && plan_value.get("certify_share").is_none() {
            // The bar is the single number the scale rules act on, and every arm shares it on purpose:
            // an experiment whose arms each chose a resolution would be several experiments. So the
            // plan states it once, and a file that left it to this reader's default is refused rather
            // than measured against a scale nobody froze.
            return Err(format!(
                "protocol: a {} plan must state certify_share, because the scale its arms hold \
                 themselves to is part of what the file measures",
                Self::SCHEMA_V3
            ));
        }
        // `Plan::from_json` requires every field it reads, including the mask, because a results
        // document that lost its `ablate` would otherwise be read as the full instrument. A protocol
        // states no mask in its plan on purpose, so the empty mask is written in here rather than
        // being allowed to become a default inside the plan reader.
        //
        // The trust rule needs no such injection: a plan section that names none *is* `current`, and
        // not as a silent default — see [`Plan::from_json`] for why that one field is read from a
        // document's history rather than required of it.
        let Json::Obj(plan_fields) = plan_value else {
            return Err("protocol: plan must be an object".to_string());
        };
        let mut with_mask = plan_fields.clone();
        with_mask.push(("ablate".to_string(), Json::Arr(Vec::new())));
        let plan = Plan::from_json(&Json::Obj(with_mask))?;
        let arms_value = value
            .get("arms")
            .and_then(Json::as_array)
            .ok_or_else(|| "protocol: arms is missing or is not a list".to_string())?;
        if arms_value.is_empty() {
            return Err("protocol: arms is empty, so there is nothing to compare".to_string());
        }
        let mut arms = Vec::new();
        for item in arms_value {
            let name = item
                .get("name")
                .and_then(Json::as_str)
                .ok_or_else(|| "protocol: an arm has no name".to_string())?
                .to_string();
            if arms.iter().any(|a: &ProtocolArm| a.name == name) {
                return Err(format!("protocol: arm {name:?} is named twice"));
            }
            let masks = item
                .get("ablate")
                .and_then(Json::as_array)
                .ok_or_else(|| format!("protocol: arm {name:?} has no ablate list"))?;
            let silenced: Vec<String> = masks
                .iter()
                .map(|mask| mask.as_str().unwrap_or_default().to_string())
                .collect();
            let ablate = plan_channels(&silenced, &format!("protocol: arm {name:?}"))?;
            // The arm's trust rule, required in one schema version and refused in the other. Both
            // refusals are about the same thing: a file that does not say which rule its arms ran
            // under cannot be the record of what it measured, and a file that says it in a place the
            // schema does not define would be read differently by two builds.
            let trust = match item.get("trust") {
                Some(_) if !rules_per_arm => {
                    return Err(format!(
                        "protocol: arm {name:?} states a trust variant, and {schema} arms state a \
                         mask only. Promote the file to {} in the commit that means to change what \
                         it measures.",
                        Self::SCHEMA_V2
                    ));
                }
                Some(value) => {
                    let text = value.as_str().ok_or_else(|| {
                        format!("protocol: arm {name:?} has a trust variant that is not a name")
                    })?;
                    Trust::parse(text).ok_or_else(|| {
                        format!(
                            "protocol: arm {name:?} names the unknown trust variant {text:?}; this \
                             tool knows {}",
                            Trust::names().join(", ")
                        )
                    })?
                }
                None if rules_per_arm => {
                    return Err(format!(
                        "protocol: arm {name:?} states no trust variant, and a {schema} arm is its \
                         mask and its rule together",
                    ));
                }
                None => Trust::Current,
            };
            // The scale axis: obliged of a `/3` arm, refused on an earlier one, exactly as the trust
            // rule above is. A file that could state a resolution rule in its plan or its arm would be
            // read two ways by two builds, which is the reason these refusals are per schema rather
            // than per field.
            let resolution = match item.get("resolution") {
                Some(_) if !scale_rules => {
                    return Err(format!(
                        "protocol: arm {name:?} states a resolution rule, and {schema} arms state a \
                         mask{} only. Promote the file to {} in the commit that means to change what \
                         it measures.",
                        if v2 { " and its trust rule" } else { "" },
                        Self::SCHEMA_V3
                    ));
                }
                Some(value) => {
                    let text = value.as_str().ok_or_else(|| {
                        format!("protocol: arm {name:?} has a resolution rule that is not a name")
                    })?;
                    aporia_boundary::Resolution::parse(text).ok_or_else(|| {
                        format!(
                            "protocol: arm {name:?} names the unknown resolution variant {text:?}; \
                             this tool knows {}",
                            aporia_boundary::Resolution::names().join(", ")
                        )
                    })?
                }
                None if scale_rules => {
                    return Err(format!(
                        "protocol: arm {name:?} states no resolution rule, and a {} arm is its mask, \
                         its trust rule and its scale rule together",
                        Self::SCHEMA_V3
                    ));
                }
                None => aporia_boundary::Resolution::Current,
            };
            // The criterion axis: obliged of a `/4` arm and refused on an earlier one, exactly as the
            // scale rule above it is, because an arm that did not say what it measured adequacy
            // against would be a different experiment wearing the same name.
            let certify = match item.get("certify") {
                Some(_) if !v4 => {
                    return Err(format!(
 "protocol: arm {name:?} states a certify criterion, and {schema} arms state a mask, a trust rule and a scale rule. Promote the file to {} in the commit that means to change what it measures.",
                        Self::SCHEMA_V4
                    ));
                }
                Some(value) => {
                    let text = value.as_str().ok_or_else(|| {
                        format!("protocol: arm {name:?} has a certify criterion that is not a name")
                    })?;
                    aporia_boundary::Certify::parse(text).ok_or_else(|| {
                        format!(
 "protocol: arm {name:?} names the unknown certify criterion {text:?}; this tool knows {}",
                            aporia_boundary::Certify::names().join(", ")
                        )
                    })?
                }
                None if v4 => {
                    return Err(format!(
 "protocol: arm {name:?} states no certify criterion, and a {} arm is its mask, its trust rule, its scale rule and its adequacy criterion together",
                        Self::SCHEMA_V4
                    ));
                }
                None => aporia_boundary::Certify::Size,
            };
            arms.push(ProtocolArm {
                name,
                ablate,
                trust,
                resolution,
                certify,
            });
        }
        // Two arms the instrument cannot tell apart are one measurement under two names, and a
        // comparison that reports both would count one experiment twice. Under `/1` the masks alone
        // decided an arm and this reduced to the existing name-uniqueness rule; under `/2` the two
        // axes are one definition, so they are checked together.
        for (at, arm) in arms.iter().enumerate() {
            for other in arms.iter().skip(at + 1) {
                let same_mask = arm.ablate.len() == other.ablate.len()
                    && arm.ablate.iter().all(|c| other.ablate.contains(c));
                if same_mask
                    && arm.trust == other.trust
                    && arm.resolution == other.resolution
                    && arm.certify == other.certify
                {
                    return Err(format!(
                        "protocol: arms {:?} and {:?} state the same ablate mask, the same trust rule and the same \
                         resolution rule, so they are one measurement under two \
                         names",
                        arm.name, other.name
                    ));
                }
                // A `/3` file that differs only in its bar is a different matter, and is refused by the
                // plan check above rather than here: under this schema the bar is not an arm's to move.
            }
        }
        let string_list = |key: &str| -> Result<Vec<String>, String> {
            let items = value
                .get("entries")
                .and_then(|e| e.get(key))
                .and_then(Json::as_array)
                .ok_or_else(|| format!("protocol: entries.{key} is missing or is not a list"))?;
            let mut out = Vec::new();
            for item in items {
                let name = item
                    .as_str()
                    .ok_or_else(|| format!("protocol: entries.{key} holds a non-string"))?;
                if out.contains(&name.to_string()) {
                    return Err(format!("protocol: entries.{key} names {name:?} twice"));
                }
                out.push(name.to_string());
            }
            Ok(out)
        };
        let names = string_list("run")?;
        if names.is_empty() {
            return Err("protocol: entries.run is empty".to_string());
        }
        // A schema-2 experiment is defined by which rows can decide it, so the file has to say which
        // entries it means by the resolvable set rather than leaving that to prose beside the numbers.
        // The two lists must cover `run` exactly: an entry in neither is a row whose universe would be
        // chosen by whoever read the file afterwards.
        let (resolvable, sub_granular, control) = if rules_per_arm {
            let resolvable = string_list("resolvable")?;
            let sub_granular = string_list("sub_granular")?;
            // The control set is obliged of a `/3` file and absent under `/2`, whose committed freeze
            // could not acquire one without an edit after its own run.
            let control = if scale_rules {
                string_list("control")?
            } else {
                Vec::new()
            };
            if resolvable.is_empty() {
                return Err(
                    "protocol: entries.resolvable names no entry, so the deciding set of this \
                     experiment is empty before it is measured"
                        .to_string(),
                );
            }
            if control.is_empty() && scale_rules {
                return Err(format!(
                    "protocol: a {} file whose entries.control names no entry has no rows on which \
                     false suspicion or unnecessary uncertainty could be measured. E1.3b froze a \
                     control-cleanliness metric and then ran eight entries, none of them marked \
                     control, and could only report the quantity as unmeasurable.",
                    Self::SCHEMA_V3
                ));
            }
            for (first_name, first, second_name, second) in [
                (
                    "entries.resolvable",
                    &resolvable,
                    "entries.sub_granular",
                    &sub_granular,
                ),
                (
                    "entries.resolvable",
                    &resolvable,
                    "entries.control",
                    &control,
                ),
                (
                    "entries.sub_granular",
                    &sub_granular,
                    "entries.control",
                    &control,
                ),
            ] {
                let shared: Vec<&str> = first
                    .iter()
                    .filter(|name| second.contains(name))
                    .map(String::as_str)
                    .collect();
                if !shared.is_empty() {
                    // Named by section, because a reader fixing a freeze needs to know which of the
                    // three lists to edit and a message that says "two of the sets" makes them go and
                    // look at all of them.
                    return Err(format!(
                        "protocol: {first_name} and {second_name} both name {}; an entry cannot be \
                         in one set and in another beside it",
                        shared.join(", ")
                    ));
                }
            }
            let covered: Vec<&String> = resolvable
                .iter()
                .chain(sub_granular.iter())
                .chain(control.iter())
                .collect();
            if covered.len() != names.len() || !names.iter().all(|n| covered.contains(&n)) {
                return Err(format!(
                    "protocol: entries.resolvable, entries.sub_granular and entries.control must \
                     cover entries.run exactly: run holds {} entry/entries and the sets hold {}",
                    names.len(),
                    covered.len()
                ));
            }
            (resolvable, sub_granular, control)
        } else {
            (Vec::new(), Vec::new(), Vec::new())
        };
        // An experiment is only pre-registered if the thing that decides it is written down. A
        // protocol file without a primary metric is a plan, not a protocol, and the runner refuses it
        // rather than letting a measurement be produced whose decider would be chosen later.
        let metric = value
            .get("primary_metric")
            .ok_or_else(|| "protocol: primary_metric is missing".to_string())?;
        // What a file is obliged to state about its decider grows with the schema rather than moving
        // between them. `/1` and `/2` name a universe and a definition; `/3` states its denominator,
        // how rows aggregate into an arm figure and that comparisons are paired — because a quantity
        // like this one can be driven to zero by an arm that certifies nothing, and only a stated
        // denominator and aggregation tell a reader which of the two happened.
        let required: &[&str] = if scale_rules {
            &[
                "name",
                "definition",
                "denominator",
                "aggregation",
                "paired_comparison",
            ]
        } else {
            &["name", "universe", "definition"]
        };
        for key in required {
            if metric.get(key).and_then(Json::as_str).is_none() {
                return Err(format!("protocol: primary_metric.{key} is missing"));
            }
        }
        let rule = metric
            .get("decision_rule")
            .and_then(Json::as_array)
            .ok_or_else(|| "protocol: primary_metric.decision_rule is missing".to_string())?;
        if rule.is_empty() {
            return Err("protocol: primary_metric.decision_rule states no outcome".to_string());
        }
        // The pilot is either the frozen scope `--pilot` reads, or nothing. A `/3` file that grew one
        // would be a freeze edited after its own pilot ran, which is the class of change the digest is
        // here to make visible rather than silent, so the key is refused rather than ignored.
        let arm_names: Vec<String> = arms.iter().map(|arm| arm.name.clone()).collect();
        let pilot = match value.get("pilot") {
            None if v4 => {
                return Err(format!(
                    "protocol: a {schema} file states no pilot section, so the infrastructure check \
                     that must precede this experiment has no frozen scope to run inside and would \
                     have to be given one on a command line — which is how {}'s pilot came to run the \
                     full plan while its own stopping rule said it should not",
                    Self::SCHEMA_V3
                ));
            }
            None => None,
            Some(_) if !v4 => {
                return Err(format!(
                    "protocol: a `pilot` section belongs to {}, and {schema} files do not have one. \
                     Adding a scope to a freeze that already ran would be editing the pre-registration",
                    Self::SCHEMA_V4
                ));
            }
            Some(item) => {
                let list = |key: &str| -> Result<Vec<String>, String> {
                    let items = item.get(key).and_then(Json::as_array).ok_or_else(|| {
                        format!("protocol: pilot.{key} is missing or is not a list")
                    })?;
                    let mut out = Vec::new();
                    for entry in items {
                        let name = entry
                            .as_str()
                            .ok_or_else(|| format!("protocol: pilot.{key} holds a non-string"))?
                            .to_string();
                        if out.contains(&name) {
                            return Err(format!("protocol: pilot.{key} names {name:?} twice"));
                        }
                        out.push(name);
                    }
                    if out.is_empty() {
                        return Err(format!(
                            "protocol: pilot.{key} is empty, so the pilot would run nothing and \
                             prove nothing about the run it precedes"
                        ));
                    }
                    Ok(out)
                };
                let numbers = |key: &str, frozen: &[u64]| -> Result<Vec<u64>, String> {
                    let items = item.get(key).and_then(Json::as_array).ok_or_else(|| {
                        format!("protocol: pilot.{key} is missing or is not a list")
                    })?;
                    let mut out = Vec::new();
                    for entry in items {
                        let at = entry
                            .as_u64()
                            .ok_or_else(|| format!("protocol: pilot.{key} holds a non-number"))?;
                        if !frozen.contains(&at) {
                            return Err(format!(
                                "protocol: pilot.{key} names {at}, which this file's plan does not \
                                 measure — a pilot may be smaller than its experiment and nothing else"
                            ));
                        }
                        if out.contains(&at) {
                            return Err(format!(
                                "protocol: pilot.{key} names {at} twice, so the number of rows the \
                                 pilot charges itself would depend on how the list was typed"
                            ));
                        }
                        out.push(at);
                    }
                    if out.is_empty() {
                        return Err(format!("protocol: pilot.{key} is empty"));
                    }
                    Ok(out)
                };
                let arms = list("arms")?;
                for name in &arms {
                    if !arm_names.contains(name) {
                        return Err(format!(
                            "protocol: pilot arms names {name:?}, which is not one of this file's arms"
                        ));
                    }
                }
                let entries = list("entries")?;
                for name in &entries {
                    if !names.contains(name) {
                        return Err(format!(
                            "protocol: pilot entries names {name:?}, which is not in entries.run"
                        ));
                    }
                }
                let checks = list("checks")?;
                Some(Pilot {
                    arms,
                    entries,
                    budgets: numbers("budgets", &plan.budgets)?,
                    seeds: numbers("seeds", &plan.seeds)?,
                    checks,
                })
            }
        };
        Ok(Self {
            id: text("id")?,
            schema: schema.to_string(),
            question: text("question")?,
            plan,
            arms,
            entries: names,
            resolvable,
            sub_granular,
            control,
            pilot,
            digest: None,
        })
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
    // The path this sweep executes the model on, named in one place because three consumers have to
    // agree on it: the campaign that finds the region, the minimiser that shrinks the finding it
    // reports, and the archive that says afterwards which arithmetic produced which number. Passing
    // it down is what makes "the same computation was re-run" a fact of the code rather than of habit.
    let mut engine = aporia_runtime::Interp;
    let mut outcomes = Vec::new();
    let mut detected_at = None;
    let mut localised_at = None;
    let mut clean_at = None;
    let mut control_suspicion = None;
    for &budget in &plan.budgets {
        let cfg = plan.config(strategy, budget, seed);
        let started = Instant::now();
        let campaign = run_with(model, cfg, &mut engine);
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
            // opened later. The ablated arm suffix exists for the same reason: E2's eleven arms
            // share entry, strategy and seed, and without it every arm would overwrite the
            // previous arm's archive — the last one standing would be the only one a reported
            // number could later be opened from. Canonical order (the enum's, not the flags'),
            // so `--ablate physical,behavioral` and `--ablate behavioral,physical` are the same
            // arm with the same directory.
            let dir = plan
                .archive_dir
                .join(archive_name(entry, plan, strategy, seed));
            // An archive is the bytes a reported number is later opened from, so replacing one is
            // worth a line. This is not a refusal because re-running a plan into the same directory
            // is a normal thing to do; it is a warning because it is also how a pilot's archive comes
            // to be gone by the time the full run needs it, and the two are told apart by which
            // directory each was asked to write.
            if dir.exists() {
                eprintln!("warning: replacing the existing archive {}", dir.display());
            }
            match archive_and_replay(entry, &campaign, plan, strategy, seed, &dir, &mut engine) {
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
            outcome.counterexamples = crate::metrics::counterexamples(
                entry,
                &campaign,
                plan.minimise_budget,
                &mut engine,
            );
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
        strategy: strategy.name(),
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

/// The directory one campaign's archive is written to, naming every axis a measurement can vary on.
///
/// Every one of them has to appear, because two arms that share a directory is one arm silently
/// replacing the other's evidence, and the point of keeping archives is that a reported number can be
/// opened later. Named per entry, not per family, for that reason: two entries in one family otherwise
/// overwrite each other. The ablation suffix exists because E2's eleven arms share entry, strategy and
/// seed. The trust suffix exists because E1.3b's arms share the entry, the seed, the budget, the
/// strategy and the mask, and differ only in the rule that decided what they were allowed to call
/// trusted. Both suffixes use canonical order — the enum's, not the caller's — so
/// `--ablate physical,behavioral` and `--ablate behavioral,physical` are one arm with one directory.
///
/// `Trust::Current` takes no suffix, which is the same compatibility decision as
/// [`plan_json`]'s omitted field and stated here rather than there because the two must not be able to
/// disagree: an archive written before variants existed already records its rule inside its own
/// manifest, and renaming its directory would make a committed archive unreachable by path.
#[must_use]
pub fn archive_name(entry: &Entry, plan: &Plan, strategy: Strategy, seed: u64) -> String {
    let silenced: Vec<&str> = aporia_evidence::Channel::ALL
        .iter()
        .filter(|c| plan.ablate.contains(c))
        .map(|c| c.name())
        .collect();
    let arm = if silenced.is_empty() {
        String::new()
    } else {
        format!("-ablated-{}", silenced.join("+"))
    };
    let trust = if plan.trust == Trust::Current {
        String::new()
    } else {
        format!("-trust-{}", plan.trust.name())
    };
    // The third axis an arm can move, named the same way and for the same reason: two arms that share
    // an entry, a seed and a mask but disagree about whether a coarse quiet leaf may be certified write
    // archives that are not the same measurement, and an archive is the bytes a reported number is
    // later opened from. A bar that differs between two arms of the same name is caught beside this, in
    // the manifest's recorded configuration, which the pre-run namespace guard compares.
    // The criterion enters the directory name beside the rule that reads it, so an arm that withheld
    // on evidence density cannot land in the directory of the arm that withheld on scale even when the
    // two partitions happen to agree.
    let certify = if plan.certify == aporia_boundary::Certify::Size
        || plan.resolution == aporia_boundary::Resolution::Current
    {
        String::new()
    } else {
        format!("-certify-{}", plan.certify.name())
    };
    let resolution = if plan.resolution == aporia_boundary::Resolution::Current {
        String::new()
    } else {
        format!("-res-{}", plan.resolution.name())
    };
    format!(
        "{}-{}-{}-seed{seed}{arm}{trust}{resolution}{certify}",
        entry.family,
        entry.name,
        strategy.name()
    )
}

/// Write the archive for one campaign and replay it, returning `(reproduced, matched, total)`.
///
/// This is where the store earns its place in the measurement: a finding is only reproducible if
/// re-running the archive's own A-IR over its own recorded inputs gives the same bits, so the
/// benchmark reports the replay result beside its detection numbers instead of assuming it.
///
/// `engine` is the sweep's own execution path, handed to the minimiser that decides each finding's
/// `case` field. Replay is deliberately *not* threaded through it: replay re-executes the archived
/// A-IR, which is the point of the check — see `aporia_store::replay`, and the archived-program case
/// that reports itself as not replayed rather than pretending.
pub fn archive_and_replay(
    entry: &Entry,
    campaign: &aporia_search::Campaign,
    plan: &Plan,
    strategy: Strategy,
    seed: u64,
    dir: &std::path::Path,
    engine: &mut dyn aporia_runtime::Executor,
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
            engine,
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
    let mut environment = Environment::current()
        .with_toolchain()
        .with_source_version();
    environment
        .notes
        .push(("strategy".to_string(), strategy.name().to_string()));
    environment
        .notes
        .push(("seed".to_string(), seed.to_string()));
    environment.notes.push(("entry".to_string(), entry.id()));
    // Which measurement these bytes belong to, stated in the archive itself. The protocol fields are
    // written only when the run came from a frozen file; the identity is written whenever the caller
    // knew it, because the guard that keeps one experiment from replacing another is exact only when it
    // has something exact to compare. An archive written by a caller that did not know its identity is
    // what the guard's "predates" refusal reads, and it is named rather than guessed at.
    if let Some(identity) = plan.provenance.measurement_identity.as_deref() {
        environment
            .notes
            .push(("measurement_identity".to_string(), identity.to_string()));
    }
    if let Some(protocol) = plan.provenance.protocol.as_deref() {
        environment
            .notes
            .push(("protocol".to_string(), protocol.to_string()));
    }
    if let Some(arm) = plan.provenance.arm.as_deref() {
        environment.notes.push(("arm".to_string(), arm.to_string()));
    }
    if let Some(digest) = plan.provenance.protocol_digest.as_deref() {
        environment
            .notes
            .push(("protocol_digest".to_string(), digest.to_string()));
    }
    if plan.provenance.pilot {
        // An archive of a pilot is evidence about the plumbing, and the note is what says so in the
        // bytes rather than only in the directory somebody chose to pass.
        environment
            .notes
            .push(("pilot".to_string(), "true".to_string()));
    }
    let run = aporia_store::Run {
        model,
        model_text: &entry.source,
        air_text: &air,
        records: &campaign.records,
        config: campaign.config.json(),
        coverage: campaign.atlas.coverage(),
        atlas_csv: &campaign.atlas.to_csv(),
        bands: &bands,
        findings: &findings,
        decisions: &decisions,
        calibrator: &campaign.calibrator,
        correlation: &campaign.correlation,
        exec: aporia_runtime::ExecConfig {
            fp: aporia_runtime::FpMode::F64,
            // The guard the campaign actually ran under, not the type's default. `replay_loaded`
            // replays with this value, so archiving 50,000,000 for a run that was cut off at
            // 2,000,000 means the replay is allowed to finish where the experiment aborted -- and the
            // disagreement that follows would be reported as a reproduction failure of the build
            // rather than as the archive's own bookkeeping error.
            max_steps: campaign.config.max_steps_per_evaluation,
        },
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

/// Run the whole corpus. Entries that do not compile are reported as skipped rather than measured.
#[must_use]
pub fn run_corpus(entries: &[Entry], plan: &Plan) -> Vec<Sweep> {
    let mut out = Vec::new();
    for entry in entries {
        if crate::corpus::unmeasurable(entry).is_some() {
            // The same predicate `results_json` reports from, so an entry skipped here is an entry the
            // document says something about.
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

/// What a results document from this build means, and when that meaning changed.
///
/// Bumped when a field's *definition* changes, not when a field is added: `/2` splits a
/// counterexample's cost into oracle calls and model executions, because a row from `/1` counted only
/// calls and called them evaluations. Two documents that answer different questions must not share a
/// measurement identity, so the schema is part of the digest — see [`identity_of`].
pub const RESULTS_SCHEMA: &str = "aporia.results/2";

/// Everything a report needs, in one document, with the environment it was measured on.
#[must_use]
pub fn results_json(
    sweeps: &[Sweep],
    entries: &[Entry],
    plan: &Plan,
    environment: &Environment,
    frozen: Option<&Frozen>,
) -> Json {
    let plan_value = plan_json(plan);
    let entries_value = entries_json(entries);
    // A results document says which question its plan can answer, so an ablation arm carries H1's
    // question rather than the strategy question its single arm cannot address. A run that came from
    // a frozen protocol quotes that file instead, because a protocol's question is decided before the
    // arms exist — and an experiment about whether one channel's result transfers is neither of the
    // two questions a plan shape can infer. The question is not part of the identity; the plan is.
    let question = match frozen {
        Some(frozen) => frozen.question.as_str(),
        None if plan.ablate.is_empty() => {
            "can a computation-aware multi-evidence search discover and localise regions of distrust \
             using fewer evaluations than simpler exploration strategies"
        }
        None => {
            "does the five-channel evidence model localise regions that no strict subset of the \
             channels localises at the same charged cost"
        }
    };
    let mut fields = vec![
        ("schema", Json::text(RESULTS_SCHEMA)),
        ("identity", Json::text(run_identity(plan, entries))),
        (
            "protocol",
            frozen.map_or(Json::Null, |frozen| {
                Json::object(vec![
                    ("id", Json::text(frozen.id.clone())),
                    ("arm", Json::text(frozen.arm.clone())),
                    (
                        "digest",
                        frozen
                            .protocol_digest
                            .as_ref()
                            .map_or(Json::Null, |d| Json::text(d.clone())),
                    ),
                ])
            }),
        ),
        ("question", Json::text(question)),
        // Which trust rule labelled these rows, named in every document whether or not the plan
        // section had to carry it. This is provenance beside the identity rather than part of it: a
        // reader opening a `Current` run has to be able to see that it says `current`, and a reader
        // comparing two documents must not have to infer the rule from which optional field is
        // present. The machine that enforces "different rule, different measurement" is the digest,
        // and [`plan_json`] carries it.
        ("trust", Json::text(plan.trust.name())),
        // The scale axis, in every document for the same reason and with the same status: the identity
        // carries it only when it moved, and a reader comparing two documents should not have to work
        // out which optional field is the absent one. The bar travels with the rule because a rule
        // without the scale it acted on is a word, not a measurement.
        ("resolution", Json::text(plan.resolution.name())),
        ("certify_share", Json::number(plan.certify_share)),
        // The criterion and its density bar, in every document beside the scale bar. E1.5's arms differ
        // by exactly this field, and a header that states which scale was withheld at but not *what
        // withholding was measured against* leaves a reader to infer the criterion from which optional
        // part of the plan section is present — the inference `plan_json` deliberately avoids by
        // carrying the bars beside the rule that reads them.
        ("certify", Json::text(plan.certify.name())),
        ("certify_density", Json::number(plan.certify_density)),
        ("plan", plan_value),
        ("entries", entries_value),
        // Could the atlas have enclosed what each entry declares, at the granularity this policy
        // permits? Computed here, at load time and from the atlas's own partition rule, because 0038's
        // account of E1.3's failed condition turned on a limit that was only visible after the run:
        // a region thinner than any leaf the depth cap allows cannot be localised by any trust rule,
        // and a reader of these rows is entitled to know which ones that was before scoring them.
        //
        // Outside the identity, for the reason the paragraph above gives for `trust` in reverse: this
        // says nothing about what was measured, only about what could be, and folding it into the
        // hashed sections would have renamed every committed measurement the moment the check existed.
        // It travels with each entry's `truth_digest`, so an edited declaration shows up as a different
        // record rather than as an old class beside a new claim.
        (
            "reachability",
            crate::reach::section(entries, &plan.trust.policy()),
        ),
        ("environment", environment_json(environment)),
        (
            "sweeps",
            Json::Arr(sweeps.iter().map(Sweep::to_json).collect()),
        ),
        // Which entries the plan covered and could not be measured, with the reason the loader gave.
        // A silently skipped entry is a number that went missing from a comparison without anyone
        // noticing — the reason the corpus loader already refuses a directory the registry names and
        // the filesystem lacks. E13's metric is that a refusal is *reported*, so the refusal belongs
        // in the document rather than in nobody's.
        (
            "refused",
            Json::Arr(
                entries
                    .iter()
                    .filter_map(|e| {
                        crate::corpus::unmeasurable(e).map(|reason| {
                            Json::object(vec![
                                ("entry", Json::text(e.id())),
                                ("reason", Json::text(reason)),
                            ])
                        })
                    })
                    .collect(),
            ),
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
    ];
    // Written only for a pilot run, so the bytes of every full measurement are exactly what they were
    // before the option existed — and a reader who opens a document about the plumbing can tell that
    // from a document about the experiment without knowing how it was invoked.
    if frozen.is_some_and(|frozen| frozen.pilot) {
        fields.push(("pilot", Json::Bool(true)));
    }
    Json::object(fields)
}

/// What the run was asked to do, as the document records it. Its own function because the identity of
/// a measurement is a digest of exactly this, and two definitions of "the plan" must not be able to
/// drift apart.
///
/// The trust variant is written **only** when it is not [`Trust::Current`], and that omission is a
/// measured decision rather than a shortcut. The identity of a measurement is a digest of this object,
/// and every committed file in `benchmarks/results/` was labelled under `Current`, so writing the field
/// unconditionally would have renamed 330 published rows the moment the instrument learned to name a
/// rule it had not changed — and a rerun of a frozen protocol could then no longer be recognised as the
/// measurement it reproduces. A run under any other variant *does* write it, so a variant row cannot
/// overwrite the row measured under another rule, which is the other half of the requirement. The name
/// is also recorded unconditionally at the top level of every results document, where it is visible to
/// a reader and outside the digest: see [`results_json`].
/// measurement identity. A test of the compatibility rule below has to be able to ask what this
/// function writes for a plan that names no variant, and a reader of a published file can recompute
/// its own identity from it, so it is the module's public shape rather than a private detail.
#[must_use]
pub fn plan_json(plan: &Plan) -> Json {
    let mut fields: Vec<(String, Json)> = vec![
        (
            "budgets".to_string(),
            Json::Arr(plan.budgets.iter().map(|b| Json::count(*b)).collect()),
        ),
        (
            "strategies".to_string(),
            Json::Arr(
                plan.strategies
                    .iter()
                    .map(|s| Json::text(s.name()))
                    .collect(),
            ),
        ),
        (
            "seeds".to_string(),
            Json::Arr(plan.seeds.iter().map(|s| Json::count(*s)).collect()),
        ),
        ("grid_per_axis".to_string(), Json::count(plan.grid as u64)),
        (
            "minimise_budget".to_string(),
            Json::count(plan.minimise_budget),
        ),
        // Which channels this arm was blind to, by name. In the identity, so an ablation arm cannot
        // overwrite the arm it is measured against; in the file, so a reader can tell an arm that
        // lacked a channel from one whose channel found nothing.
        (
            "ablate".to_string(),
            Json::Arr(
                plan.ablate
                    .iter()
                    .map(|c| Json::text(c.name()))
                    .collect::<Vec<_>>(),
            ),
        ),
        // The rates that cost evaluations are part of the definition of the measurement, so
        // they travel with its results: without them a reader cannot tell a channel that
        // changed nothing from a channel that was never sampled.
        ("probe_every".to_string(), Json::count(plan.probe_every)),
        (
            "numerical_every".to_string(),
            Json::count(plan.numerical_every),
        ),
        (
            "differential_every".to_string(),
            Json::count(plan.differential_every),
        ),
        (
            "symmetric_every".to_string(),
            Json::count(plan.symmetric_every),
        ),
        ("refine_every".to_string(), Json::count(plan.refine_every)),
        (
            "calibrate_every".to_string(),
            Json::count(plan.calibrate_every),
        ),
    ];
    if plan.trust != Trust::Current {
        // Beside `ablate`, because that is what it is: a second declared axis of the comparison
        // rather than a rate that happens to differ.
        let at = fields
            .iter()
            .position(|(name, _)| name == "ablate")
            .map_or(fields.len(), |at| at + 1);
        fields.insert(at, ("trust".to_string(), Json::text(plan.trust.name())));
    }
    if plan.resolution != aporia_boundary::Resolution::Current {
        // The third declared axis, beside the other two and under the same discipline: written only
        // when it is not the default, so a default plan still serialises to the bytes that named the
        // committed measurements, and written *with its bar*, because a plan that said it demanded a
        // resolution and not which one could be re-run against a different scale without moving its
        // identity. That is the one thing an axis carried in the plan exists to prevent.
        let at = fields
            .iter()
            .position(|(name, _)| name == "ablate")
            .map_or(fields.len(), |at| at + 1);
        fields.insert(
            at,
            ("resolution".to_string(), Json::text(plan.resolution.name())),
        );
        fields.insert(
            at + 1,
            (
                "certify_share".to_string(),
                Json::number(plan.certify_share),
            ),
        );
        // The criterion and its bar travel with the rule that reads them, for the same reason the
        // share does — and only where a rule reads them. A `size` plan never asks how many
        // evaluations a leaf holds, so writing its density bar would rename measurements that the
        // bar cannot have influenced: the reproduction gate caught exactly that, with E1.4's three
        // withholding arms coming back under a different identity than the ones they published.
        // Under `Resolution::Current` nothing is written for them either, because a criterion that
        // acts on no label is not a different measurement: two plans that differ only there must
        // digest alike, or the runner would refuse the second as a duplicate of a measurement that
        // is in fact identical to it.
        if plan.certify != aporia_boundary::Certify::Size {
            fields.insert(
                at + 2,
                ("certify".to_string(), Json::text(plan.certify.name())),
            );
            fields.insert(
                at + 3,
                (
                    "certify_density".to_string(),
                    Json::number(plan.certify_density),
                ),
            );
        }
    }
    Json::Obj(fields)
}

/// Which corpus entries were covered, and what each one claimed. The claim travels with the name —
/// and with the digest of the file it was read from — because measuring against an edited
/// `truth.json` is a different measurement. Before the digest, editing a region's numbers left the
/// identity untouched, so the corrected run could overwrite the published one's name.
/// The identity a results document carries for this plan and this entry list: the digest of the two
/// sections that decide what was measured.
///
/// One definition, because three things have to agree on it — the document's `identity` field, the
/// filename the writer refuses to overwrite, and the manifest each archive records so that a directory
/// can be recognised without its document. A second spelling is where those three would drift.
#[must_use]
pub fn run_identity(plan: &Plan, entries: &[Entry]) -> String {
    identity_of(RESULTS_SCHEMA, &plan_json(plan), &entries_json(entries))
}

fn entries_json(entries: &[Entry]) -> Json {
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
                    // The claim itself, not just its shape. An edit to a region's numbers changes
                    // this, and therefore changes the measurement's name.
                    ("truth_digest", Json::text(e.truth_digest.clone())),
                ])
            })
            .collect(),
    )
}

/// The identity of one measurement: a short digest of what its fields mean, the plan it ran, and the
/// entries it covered.
///
/// This exists because a measurement needs an address. Under the old name -- the second the run
/// finished -- two runs of one plan were two unnameable experiments, and "is this the same measurement
/// I published last month, or a different one?" had no answer that did not involve reading both files
/// and diffing them by hand. Derived from the document's own `schema`, `plan` and `entries` sections,
/// so the bytes of a results file decide its name and a reader can recompute it.
///
/// The schema is in the digest for the same reason the sampling rates are: a run whose cost column
/// counts a different quantity is a different experiment, even when its plan and its corpus are
/// byte-identical. Without it, re-measuring after a definition change would either collide with the
/// published file's name or need a hand-written second name.
///
/// Two things are deliberately *not* in it. The clock, because when a machine ran is not part of the
/// question being asked. And the environment, because the same experiment run on another OS, or read
/// from another corpus directory, is the same experiment -- `environment` still travels in the
/// document as provenance for the numbers; it just does not name them.
///
/// List-valued plan fields and the entry list are sorted before digesting: `--strategies adaptive,random`
/// and `--strategies random,adaptive` measure one comparison, so the name must not depend on the order
/// the flags were typed in.
#[must_use]
pub fn identity_of(schema: &str, plan: &Json, entries: &Json) -> String {
    let mut key = String::new();
    key.push_str("schema=");
    key.push_str(schema);
    key.push('\n');
    if let Json::Obj(fields) = plan {
        for (name, value) in fields {
            let text = match value {
                Json::Arr(items) => {
                    let mut parts: Vec<String> = items.iter().map(Json::to_compact).collect();
                    parts.sort();
                    parts.join(",")
                }
                other => other.to_compact(),
            };
            key.push_str(name);
            key.push('=');
            key.push_str(&text);
            key.push('\n');
        }
    }
    let mut covered: Vec<String> = match entries {
        Json::Arr(items) => items.iter().map(Json::to_compact).collect(),
        _ => Vec::new(),
    };
    covered.sort();
    key.push_str("entries=");
    key.push_str(&covered.join(","));
    // A prefix, not the whole digest: this is a name a human types and greps, not a checksum of the
    // file's bytes. 48 bits is far past the number of measurements this corpus will ever record.
    aporia_store::digest::sha256_hex(key.as_bytes())[..12].to_string()
}

/// The identity a results document carries: the recorded one when the writer computed it, and the one
/// the document's own schema, plan and entries imply when it predates the field. Both paths read the
/// file, so a published measurement can be named without knowing what build wrote it.
#[must_use]
pub fn results_identity(results: &Json) -> Option<String> {
    if let Some(recorded) = results.get("identity").and_then(Json::as_str) {
        return Some(recorded.to_string());
    }
    let schema = results.get("schema").and_then(Json::as_str)?;
    let plan = results.get("plan")?;
    let entries = results.get("entries")?;
    Some(identity_of(schema, plan, entries))
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
    let silenced_mask = cfg.silenced;
    let trust = cfg.trust();
    let campaign = run(model, cfg);
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{}  strategy {strategy:?}  budget {budget}  seed {seed}",
        entry.id()
    );
    // Which rule decided what this run was allowed to call trusted, named even when it is the one that
    // has always been in force: `explain` exists to answer why a label came out the way it did, and
    // the rule is the first thing a reader would have to assume without it.
    let _ = writeln!(out, "trust rule: {}", trust.name());
    // The scale axis, named together with the number it acts on for the same reason: a quiet coarse
    // leaf may have been left alone by a rule that is off, or reached and withheld by a rule that is
    // on, and without the bar in front of the reader those two runs explain identically. E1.5 adds the
    // criterion to that sentence because a leaf can be withheld for two different reasons — being
    // large, or being thinly measured — and an arm is defined by which one it asked. Naming only the
    // size bar would explain a density arm as if it had been a size arm.
    let acts_on = match plan.certify {
        aporia_boundary::Certify::Size => format!(
            "{} on leaf size: nothing coarser than {:.8} of the domain",
            plan.certify.name(),
            plan.certify_share
        ),
        aporia_boundary::Certify::Density => format!(
            "{} on evidence density: nothing holding fewer than {:.0} evaluations per domain volume",
            plan.certify.name(),
            plan.certify_density
        ),
    };
    let _ = writeln!(
        out,
        "resolution rule: {}, certifying under {}",
        plan.resolution.name(),
        acts_on,
    );
    let coverage = campaign.atlas.coverage();
    let policy = campaign.atlas.policy();
    let coarse_certified: f64 = campaign
        .atlas
        .leaf_ids()
        .iter()
        .filter_map(|id| campaign.atlas.cell(*id))
        .filter(|c| c.label == aporia_boundary::Label::Trusted && policy.coarse(c))
        .map(|c| c.relative_size(campaign.atlas.root()))
        .sum();
    let _ = writeln!(
        out,
        "cells {} ({} leaves), suspicious {:.4} of the domain, trusted {:.4}, unknown {:.4}, \
         provisional {:.4}, trusted-but-coarser {:.4}, trusted-but-thinly-measured {:.4}",
        coverage.cells,
        campaign.atlas.leaf_ids().len(),
        coverage.suspicious,
        coverage.trusted,
        coverage.unknown,
        coverage.provisional,
        coarse_certified,
        campaign
            .atlas
            .leaf_ids()
            .iter()
            .filter_map(|id| campaign.atlas.cell(*id))
            .filter(|c| c.label == aporia_boundary::Label::Trusted && policy.thinly_measured(c))
            .map(|c| c.relative_size(campaign.atlas.root()))
            .sum::<f64>()
    );
    let _ = writeln!(
        out,
        "non-finite readings in leaves {} — what {} has to work on, and the reason a label that moved \
         under a non-finite rule was moved by it or by something else",
        campaign.atlas.non_finite_readings(),
        trust.name()
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
    // What each channel did on this run, and — when this is an ablation arm — what was withheld.
    // The census is the question an ablation exists to answer, so an explain without it would
    // print every other number and omit the one the arm was built to vary.
    let silenced: Vec<&str> = aporia_evidence::Channel::ALL
        .into_iter()
        .filter(|c| silenced_mask & (1 << c.index()) != 0)
        .map(aporia_evidence::Channel::name)
        .collect();
    if !silenced.is_empty() {
        let _ = writeln!(out, "ablation: silenced {}", silenced.join(", "));
    }
    let census = crate::metrics::Census::of(&campaign);
    for c in &census.channels {
        let _ = writeln!(
            out,
            "channel {:<12} applied {:>5}  computed {:>5}  readings {:>5}  strong {:>5}  \
             findings {:>3}  {}",
            c.channel.name(),
            c.applied,
            c.computed,
            c.readings,
            c.strong,
            c.findings,
            if c.silenced { "silenced" } else { "" }
        );
    }
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
    // The decomposition, measured by the same function that writes it into a results row. Printed
    // here rather than recomputed, because an `explain` that derived its own copy of a metric would
    // be a second definition of it, and the whole point of the four columns is that there is one.
    let outcome = crate::metrics::Outcome::measure(entry, &campaign, 0, strategy, budget);
    let _ = writeln!(
        out,
        "region decomposition: the declared regions cover {:.6} of the space, of which the partition \
         leaves certified {:.6}, unknown {:.6}, suspicious {:.6}, provisional {:.6} — certified reads \
         {:.6} to the best single region it lies in",
        outcome.declared_union_volume,
        outcome.trusted_over_true_union.unwrap_or(f64::NAN),
        outcome.unknown_over_true.unwrap_or(f64::NAN),
        outcome.suspicious_over_true.unwrap_or(f64::NAN),
        outcome.provisional_over_true.unwrap_or(f64::NAN),
        outcome.trusted_over_true,
    );
    if outcome.region_overlap_refusals > 0 {
        let _ = writeln!(
            out,
            "{} leaves refused a region overlap, so the four columns above state nothing",
            outcome.region_overlap_refusals
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

/// Does this band straddle the declared span on any axis the region constrains?
///
/// Overlap, not containment: a band is `[lo, hi]` along one axis while a region names a span on
/// another, and the claim being made is "the atlas saw a transition where the fault is", which is true
/// when the two intervals meet. `aporia_boundary::Band::contains` answers a different question — is
/// this *value* inside this band — and is used by that crate's own tests, not by this metric.
fn band_covers(band: &aporia_boundary::Band, region: &crate::truth::Declared) -> bool {
    region
        .axes
        .iter()
        .any(|(_, [lo, hi])| band.facing[1] >= *lo && band.facing[0] <= *hi)
}

/// What the measurements say, as text a person can read in a terminal.
///
/// The subject of the sentence is the adaptive arm and everything else in the plan is a baseline,
/// which is what E1 asks. Arms are discovered from the sweeps rather than listed here, because a
/// comparison whose participant list is written by the printer rather than by the plan is a
/// comparison that quietly changes when the plan grows.
#[must_use]
pub fn verdict(sweeps: &[Sweep], truth_of: &dyn Fn(&str) -> Option<Truth>) -> String {
    use std::fmt::Write as _;
    const SUBJECT: &str = "adaptive";
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
        let (adaptive, a_ok, a_n) = best(SUBJECT);
        // Every arm this plan actually measured, named by the sweeps rather than by a list written
        // here. The three names this function used to hard-code are how a fourth arm would have been
        // dropped from the sentence the README quotes — quietly, with the tally still reading right.
        let mut arms: Vec<&'static str> = sweeps
            .iter()
            .filter(|s| s.entry == entry)
            .map(|s| s.strategy)
            .collect();
        arms.sort_unstable();
        arms.dedup();
        if a_n == 0 {
            let _ = writeln!(
                out,
                "{entry}: no {SUBJECT} arm in this plan; measured {}",
                arms.join(", ")
            );
            continue;
        }
        let others: Vec<&'static str> = arms.into_iter().filter(|a| *a != SUBJECT).collect();
        let baseline = others.iter().filter_map(|a| best(a).0).min();
        let detail = others
            .iter()
            .map(|a| {
                let (at, ok, n) = best(a);
                format!("{a} {at:?} ({ok}/{n})")
            })
            .collect::<Vec<_>>()
            .join(" | ");
        let _ = writeln!(
            out,
            "{entry}: {SUBJECT} {adaptive:?} ({a_ok}/{a_n}) | baseline {baseline:?} | {detail}"
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
