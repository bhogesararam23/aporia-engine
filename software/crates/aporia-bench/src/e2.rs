//! The E2 comparison: ablation arms against the full instrument, read from committed results files.
//!
//! The strategy comparison in [`crate::metrics::compare`] asks one question — which sampler got
//! there first — and its unit is a strategy name. E2 asks a different question: what did the
//! evidence model contribute? Its unit is an *arm*, a plan identical to the full instrument's in
//! every field except the channels it silences, and its result is a delta against the full arm
//! paired on entry, seed and budget.
//!
//! Everything here reads results documents — the files the harness wrote — rather than in-memory
//! runs, because a comparison that exists only in a console transcript is a comparison nobody can
//! check later. The first document must be the full arm; every other document is paired against
//! it, and a document whose plan differs from the full arm's in anything except the ablation mask
//! is refused rather than paired: two experiments that were not the same experiment do not become
//! comparable because somebody typed their file names on one line.
//!
//! Every compared quantity is one of four states — `same`, `changed`, `not measurable`,
//! `not applicable` — and missing data is never converted into a zero. A boundary one arm could
//! not see is *not measurable*, not an error of zero; a control has no regions to localise, which
//! is *not applicable*, not a tie. The one number that must match exactly is the charged
//! evaluations: arms that spent different budgets are not arms of one experiment, and the pairing
//! refuses them instead of dividing by a cost difference nobody declared.

use crate::metrics::{Census, ChannelCensus};
use aporia_evidence::Channel;
use aporia_store::Json;

/// One sweep as the comparison reads it back from a results document.
#[derive(Debug)]
struct ArmSweep {
    entry: String,
    strategy: String,
    seed: u64,
    /// Zero on a control or a region-less entry, which is what makes localisation comparisons
    /// there *not applicable* rather than tied.
    declared_regions: u64,
    control: bool,
    detected_at: Option<u64>,
    localised_at: Option<u64>,
    outcomes: Vec<ArmOutcome>,
}

/// One budget's row of a sweep, in the shape the results file carries.
#[derive(Debug)]
struct ArmOutcome {
    budget: u64,
    evaluations: u64,
    instruction_steps: u64,
    detected: u64,
    localised: u64,
    findings: u64,
    suspicious: f64,
    trusted: f64,
    unknown: f64,
    boundaries: Vec<ArmBoundary>,
    census: Census,
}

#[derive(Debug)]
struct ArmBoundary {
    axis: String,
    error: Option<f64>,
    within: bool,
}

/// One arm of E2: a results document opened for comparison.
#[derive(Debug)]
pub struct Arm {
    pub path: String,
    pub identity: Option<String>,
    /// The channels this arm silences, from the plan it recorded.
    pub ablate: Vec<Channel>,
    /// The plan as `(field, value)` pairs in the order the document records them, *without* the
    /// `ablate` field — the fingerprint two documents must share to be the same experiment.
    plan: Vec<(String, String)>,
    sweeps: Vec<ArmSweep>,
}

impl Arm {
    /// Read an arm out of a results document.
    ///
    /// Refuses a document with no sweeps or no readable plan, because neither can be paired with
    /// anything: the comparison is only as honest as the refusal that keeps unlike things apart.
    #[expect(
        clippy::too_many_lines,
        reason = "one document read in one place: the sweep, the outcome and the census fields are                   the shape of the file, and splitting the reader would spread that shape over                   three functions that must agree"
    )]
    pub fn from_document(path: &str, doc: &Json) -> Result<Self, String> {
        let plan = doc
            .get("plan")
            .ok_or_else(|| format!("{path} records no plan, so it is not a measurable arm"))?;
        let mut ablate = Vec::new();
        let mut fields = Vec::new();
        if let Json::Obj(items) = plan {
            for (name, value) in items {
                if name == "ablate" {
                    // Names, refused rather than defaulted: a channel word this build does not
                    // know is an arm nobody can describe, not an arm that silenced nothing.
                    if let Json::Arr(list) = value {
                        for item in list {
                            let text = item.as_str().ok_or_else(|| {
                                format!("{path}: the ablate list holds a non-name: {item:?}")
                            })?;
                            let channel = Channel::parse(text).ok_or_else(|| {
                                format!("{path} silences the unknown channel {text:?}")
                            })?;
                            ablate.push(channel);
                        }
                    }
                    continue;
                }
                fields.push((name.clone(), value.to_compact()));
            }
        }
        let sweeps_json = doc
            .get("sweeps")
            .and_then(Json::as_array)
            .ok_or_else(|| format!("{path} records no sweeps, so it measured nothing"))?;
        let mut sweeps = Vec::new();
        for s in sweeps_json {
            let outcomes = s
                .get("outcomes")
                .and_then(Json::as_array)
                .ok_or_else(|| format!("{path} has a sweep with no outcomes"))?;
            let mut parsed = Vec::new();
            for o in outcomes {
                // Every field below is refused by name when absent. A comparison is allowed to say
                // "not measurable" about a quantity the run could not produce; it is never allowed
                // to read a quantity the *file* does not contain as a zero, because that turns a
                // truncated or hand-edited document into a measurement of something that did not
                // happen. The writer emits all of these, so refusing costs nothing real.
                let count = |k: &str| -> Result<u64, String> {
                    o.get(k).and_then(Json::as_u64).ok_or_else(|| {
                        format!("{path} has an outcome with no {k}; a missing number is refused, not read as 0")
                    })
                };
                let volume = |k: &str| -> Result<f64, String> {
                    o.get(k).and_then(Json::as_f64).ok_or_else(|| {
                        format!("{path} has an outcome with no {k}; a missing volume is refused, not read as 0")
                    })
                };
                let boundaries = match o.get("boundaries") {
                    None => {
                        return Err(format!(
                            "{path} has an outcome with no boundaries section; an entry that declares \
                             no boundary writes an empty list, which is not the same as nothing"
                        ));
                    }
                    Some(list) => list
                        .as_array()
                        .ok_or_else(|| {
                            format!("{path} has a boundaries section that is not a list")
                        })?
                        .iter()
                        .map(|b| {
                            let axis = b
                                .get("axis")
                                .and_then(Json::as_str)
                                .ok_or_else(|| format!("{path} has a boundary row with no axis"))?
                                .to_string();
                            let within = b
                                .get("within_tolerance")
                                .and_then(Json::as_bool)
                                .ok_or_else(|| {
                                    format!("{path} has a boundary row with no within_tolerance")
                                })?;
                            Ok::<_, String>(ArmBoundary {
                                axis,
                                // `error` is legitimately absent: the writer puts text there when no
                                // band formed. That is the not-measurable state, read as such below.
                                error: b.get("error").and_then(Json::as_f64),
                                within,
                            })
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                };
                let census = o.get("census").and_then(Census::from_json).ok_or_else(|| {
                    format!(
                        "{path} has an outcome without a readable census; it predates the \
                             field or was written by another build"
                    )
                })?;
                parsed.push(ArmOutcome {
                    budget: count("budget")?,
                    evaluations: count("evaluations")?,
                    instruction_steps: count("instruction_steps")?,
                    detected: count("detected_regions")?,
                    localised: count("localised_regions")?,
                    findings: count("findings")?,
                    suspicious: volume("suspicious_volume")?,
                    trusted: volume("trusted_volume")?,
                    unknown: volume("unknown_volume")?,
                    boundaries,
                    census,
                });
            }
            let first = outcomes
                .first()
                .ok_or_else(|| format!("{path} has a sweep with no outcomes"))?;
            let sweep = ArmSweep {
                entry: s
                    .get("entry")
                    .and_then(Json::as_str)
                    .ok_or_else(|| format!("{path} has a sweep with no entry"))?
                    .to_string(),
                // The pairing key, all three of it: a sweep that does not say which arm and seed it
                // is cannot be matched against the full arm, and defaulting would match it to any
                // other sweep missing the same field.
                strategy: s
                    .get("strategy")
                    .and_then(Json::as_str)
                    .ok_or_else(|| format!("{path} has a sweep with no strategy"))?
                    .to_string(),
                seed: s
                    .get("seed")
                    .and_then(Json::as_u64)
                    .ok_or_else(|| format!("{path} has a sweep with no seed"))?,
                declared_regions: first
                    .get("declared_regions")
                    .and_then(Json::as_u64)
                    .ok_or_else(|| {
                        format!(
                            "{path} has a sweep whose outcomes carry no declared_regions, the field \
                             that decides whether localisation applies at all"
                        )
                    })?,
                control: first
                    .get("control")
                    .and_then(Json::as_bool)
                    .ok_or_else(|| {
                        format!(
                            "{path} has a sweep whose outcomes do not say whether it is a control"
                        )
                    })?,
                // Absent here means "no budget in the ladder reached it", which is the measured
                // null and distinct from a missing field; the writer emits null, not nothing.
                detected_at: s.get("detected_at_budget").and_then(Json::as_u64),
                localised_at: s.get("localised_at_budget").and_then(Json::as_u64),
                outcomes: parsed,
            };
            sweeps.push(sweep);
        }
        Ok(Self {
            path: path.to_string(),
            identity: doc
                .get("identity")
                .and_then(Json::as_str)
                .map(str::to_string),
            ablate,
            plan: fields,
            sweeps,
        })
    }

    /// What this arm is called in a report: the channels it silences, by name.
    fn label(&self) -> String {
        if self.ablate.is_empty() {
            "full".to_string()
        } else if self.ablate.len() == Channel::ALL.len() - 1 {
            let kept = Channel::ALL
                .into_iter()
                .find(|c| !self.ablate.contains(c))
                .expect("four of five silenced leaves one kept");
            format!("only-{}", kept.name())
        } else {
            format!(
                "-{}",
                self.ablate
                    .iter()
                    .map(|c| c.name())
                    .collect::<Vec<_>>()
                    .join("+")
            )
        }
    }
}

/// The localisation or detection events of one arm, over every region-bearing (entry, seed).
#[derive(Default, Debug)]
pub struct Tally {
    /// Full localised and the arm did not, with the (entry, seed) it happened on.
    pub full_only: Vec<(String, u64)>,
    /// The arm localised and full did not — removing evidence helped, which is a real result and
    /// is never folded into "changed nothing".
    pub arm_only: Vec<(String, u64)>,
    pub both_same: u64,
    /// Both localised and full needed the smaller budget.
    pub full_earlier: u64,
    pub arm_earlier: u64,
    pub neither: u64,
    /// Controls and region-less entries: the comparison has no meaning there, and the count says
    /// so rather than the pairs disappearing.
    pub not_applicable: u64,
}

impl Tally {
    fn json(&self) -> Json {
        let pair = |v: &[(String, u64)]| {
            Json::Arr(
                v.iter()
                    .map(|(e, s)| Json::Arr(vec![Json::text(e.clone()), Json::count(*s)]))
                    .collect(),
            )
        };
        Json::object(vec![
            ("full_only", pair(&self.full_only)),
            ("arm_only", pair(&self.arm_only)),
            ("both_same", Json::count(self.both_same)),
            ("full_earlier", Json::count(self.full_earlier)),
            ("arm_earlier", Json::count(self.arm_earlier)),
            ("neither", Json::count(self.neither)),
            ("not_applicable", Json::count(self.not_applicable)),
        ])
    }
}

/// Boundary tolerance flips, which are the discrete event inside the continuous error metric.
#[derive(Default, Debug)]
pub struct BoundaryTally {
    /// Full was within tolerance and the arm was not.
    pub lost_within: u64,
    /// The arm was within tolerance and full was not.
    pub gained_within: u64,
    pub both_within: u64,
    pub neither_within: u64,
    /// At least one arm had no band on the axis: the delta does not exist and is not zero.
    pub not_measurable: u64,
}

impl BoundaryTally {
    fn json(&self) -> Json {
        Json::object(vec![
            ("lost_within", Json::count(self.lost_within)),
            ("gained_within", Json::count(self.gained_within)),
            ("both_within", Json::count(self.both_within)),
            ("neither_within", Json::count(self.neither_within)),
            ("not_measurable", Json::count(self.not_measurable)),
        ])
    }
}

/// False-positive volume on one control entry, per arm, with the pairs that moved.
#[derive(Debug)]
pub struct ControlSuspicion {
    pub entry: String,
    /// How many (seed, budget) pairs the control contributed.
    pub pairs: u64,
    /// Mean signed change in suspicious volume, arm minus full.
    pub mean_delta: f64,
    /// The largest signed change, with where it happened.
    pub worst: (f64, u64, u64),
    /// Accumulator for the mean; not part of the reading.
    sum: f64,
}

/// A control pair where the ablated arm trusted *more* of the domain than the full arm.
///
/// The three volumes always sum to the whole domain, so a trust gain is arithmetically always paid
/// for by suspicion and ignorance falling — the informative split is **which** pool paid, and this
/// record carries both. Trust bought by freeing false suspicion is the silenced channel's cost on
/// a model that has nothing wrong with it; trust bought by promoting volume the full arm never had
/// enough measurement to label is the arm declaring a region trustworthy on less evidence, which
/// is the direction worth inspecting. The comparison names every pair and both sources; it does
/// not decide the reading for the reader.
#[derive(Debug)]
pub struct TrustIncrease {
    pub entry: String,
    pub seed: u64,
    pub budget: u64,
    pub gain: f64,
    /// Change in suspicious volume at the same pair, arm minus full. Negative means the arm called
    /// less of the domain suspicious, so suspicion paid for the trust.
    pub suspicion_from: f64,
    /// Change in UNKNOWN volume at the same pair. Negative means the arm resolved ignorance the
    /// full arm had kept, so unlabelled volume paid for the trust.
    pub unknown_from: f64,
}

/// One channel's census, both sides of the pair.
#[derive(Debug)]
pub struct CensusDelta {
    pub channel: Channel,
    pub full: ChannelCensus,
    pub arm: ChannelCensus,
}

/// One paired budget row: everything the arm did differently from the full arm at the same cost.
#[derive(Debug)]
pub struct Pair {
    pub entry: String,
    pub seed: u64,
    pub budget: u64,
    /// Detected regions, (full, arm).
    pub detected: (u64, u64),
    /// Localised regions, (full, arm).
    pub localised: (u64, u64),
    pub findings: (u64, u64),
    pub suspicious: (f64, f64),
    pub trusted: (f64, f64),
    pub unknown: (f64, f64),
    /// Instruction steps, arm minus full. Charged evaluations must match; steps may differ when
    /// the arms' trajectories diverged, which is the effect being measured, and the difference is
    /// shown rather than asserted away.
    pub steps: i64,
    pub boundaries: Vec<(String, Option<f64>, Option<f64>)>,
    pub census: Vec<CensusDelta>,
}

impl Pair {
    fn json(&self) -> Json {
        let counts = |(a, b): (u64, u64)| {
            Json::object(vec![("full", Json::count(a)), ("arm", Json::count(b))])
        };
        let volumes = |(a, b): (f64, f64)| {
            Json::object(vec![
                ("full", Json::number(a)),
                ("arm", Json::number(b)),
                ("delta", Json::number(b - a)),
            ])
        };
        Json::object(vec![
            ("entry", Json::text(self.entry.clone())),
            ("seed", Json::count(self.seed)),
            ("budget", Json::count(self.budget)),
            ("detected_regions", counts(self.detected)),
            ("localised_regions", counts(self.localised)),
            ("findings", counts(self.findings)),
            ("suspicious_volume", volumes(self.suspicious)),
            ("trusted_volume", volumes(self.trusted)),
            ("unknown_volume", volumes(self.unknown)),
            ("instruction_steps_delta", Json::number(self.steps as f64)),
            (
                "boundaries",
                Json::Arr(
                    self.boundaries
                        .iter()
                        .map(|(axis, full, arm)| {
                            Json::object(vec![
                                ("axis", Json::text(axis.clone())),
                                ("full_error", full.map_or(Json::Null, Json::number)),
                                ("arm_error", arm.map_or(Json::Null, Json::number)),
                            ])
                        })
                        .collect(),
                ),
            ),
            (
                "census",
                Json::Arr(
                    self.census
                        .iter()
                        .map(|d| {
                            Json::object(vec![
                                ("channel", Json::text(d.channel.name())),
                                ("full_computed", Json::count(d.full.computed)),
                                ("arm_computed", Json::count(d.arm.computed)),
                                ("full_readings", Json::count(d.full.readings)),
                                ("arm_readings", Json::count(d.arm.readings)),
                                ("arm_findings", Json::count(d.arm.findings)),
                                ("silenced", Json::Bool(d.arm.silenced)),
                            ])
                        })
                        .collect(),
                ),
            ),
        ])
    }
}

/// The whole comparison of one arm against the full arm.
#[derive(Debug)]
pub struct Comparison {
    pub label: String,
    pub identity: Option<String>,
    pub silenced: Vec<Channel>,
    pub pairs: Vec<Pair>,
    pub localisation: Tally,
    pub detection: Tally,
    pub boundaries: BoundaryTally,
    pub controls: Vec<ControlSuspicion>,
    pub trust_increases: Vec<TrustIncrease>,
    /// Entries where every channel this arm silences computed nothing on the full arm: the arm's
    /// nulls there are properties of the corpus, not of the channels, and are not findings.
    pub vacuous: Vec<String>,
}

impl Comparison {
    fn json(&self) -> Json {
        Json::object(vec![
            ("arm", Json::text(self.label.clone())),
            (
                "identity",
                self.identity.clone().map_or(Json::Null, Json::text),
            ),
            (
                "silenced",
                Json::Arr(self.silenced.iter().map(|c| Json::text(c.name())).collect()),
            ),
            ("localisation", self.localisation.json()),
            ("detection", self.detection.json()),
            ("boundaries", self.boundaries.json()),
            (
                "controls",
                Json::Arr(
                    self.controls
                        .iter()
                        .map(|c| {
                            Json::object(vec![
                                ("entry", Json::text(c.entry.clone())),
                                ("pairs", Json::count(c.pairs)),
                                ("mean_suspicion_delta", Json::number(c.mean_delta)),
                                ("worst_delta", Json::number(c.worst.0)),
                                ("worst_at_seed", Json::count(c.worst.1)),
                                ("worst_at_budget", Json::count(c.worst.2)),
                            ])
                        })
                        .collect(),
                ),
            ),
            (
                "trust_increases",
                Json::Arr(
                    self.trust_increases
                        .iter()
                        .map(|t| {
                            Json::object(vec![
                                ("entry", Json::text(t.entry.clone())),
                                ("seed", Json::count(t.seed)),
                                ("budget", Json::count(t.budget)),
                                ("trusted_volume_gain", Json::number(t.gain)),
                                ("paid_by_suspicion_change", Json::number(t.suspicion_from)),
                                ("paid_by_unknown_change", Json::number(t.unknown_from)),
                            ])
                        })
                        .collect(),
                ),
            ),
            (
                "vacuous_entries",
                Json::Arr(self.vacuous.iter().map(|e| Json::text(e.clone())).collect()),
            ),
            (
                "pairs",
                Json::Arr(self.pairs.iter().map(Pair::json).collect()),
            ),
        ])
    }
}

/// Pair one ablated arm against the full arm.
///
/// Refuses, with a message naming the offender, when the two documents are not one experiment
/// with two masks: differing plans, a sweep one side does not have, a budget one side did not
/// run, or a pair that charged a different number of evaluations.
#[expect(
    clippy::too_many_lines,
    reason = "the comparison is one pass over the paired sweeps, and every tally it fills is a               local of that pass; extracting halves would hand the invariants between them to the               caller instead of proving them here"
)]
pub fn compare(full: &Arm, arm: &Arm) -> Result<Comparison, String> {
    if !full.ablate.is_empty() {
        return Err(format!(
            "{} is not the full arm: it silences {}. The first document must be the full \
             instrument's own results",
            full.path,
            full.ablate
                .iter()
                .map(|c| c.name())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    // The plan comparison is field by field in the order the documents record them, so the message
    // names the field that differs rather than producing two opaque digests.
    for (name, full_value) in &full.plan {
        let arm_value = arm
            .plan
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.clone());
        if arm_value.as_deref() != Some(full_value.as_str()) {
            return Err(format!(
                "{} and {} differ in {name} ({full_value} vs {}): the arms of one experiment \
                 agree on everything except the ablate mask",
                full.path,
                arm.path,
                arm_value.unwrap_or_else(|| "absent".to_string())
            ));
        }
    }
    if full.plan.len() != arm.plan.len() {
        return Err(format!(
            "{} and {} record different plan fields, so they were written by different builds",
            full.path, arm.path
        ));
    }

    let mut pairs = Vec::new();
    let mut localisation = Tally::default();
    let mut detection = Tally::default();
    let mut boundary_tally = BoundaryTally::default();
    // Every control entry gets its slot before any pair is compared, so the per-entry aggregation
    // is by construction rather than by append-order.
    let mut controls: Vec<ControlSuspicion> = arm
        .sweeps
        .iter()
        .filter(|s| s.control)
        .map(|s| ControlSuspicion {
            entry: s.entry.clone(),
            pairs: 0,
            mean_delta: 0.0,
            worst: (0.0, 0, 0),
            sum: 0.0,
        })
        .fold(Vec::new(), |mut acc, slot| {
            if !acc.iter().any(|c| c.entry == slot.entry) {
                acc.push(slot);
            }
            acc
        });
    let mut trust_increases = Vec::new();
    // An entry is vacuous for this arm only when *every* silenced channel computed nothing on the
    // full arm, at the top budget, on every seed: if any seed saw material, the channel had
    // something to say on that entry and the arm's null there is a finding about the channel.
    let mut ever_computed: std::collections::BTreeMap<String, bool> =
        std::collections::BTreeMap::new();

    for arm_sweep in &arm.sweeps {
        let key = (&arm_sweep.entry, &arm_sweep.strategy, arm_sweep.seed);
        let full_sweep = full
            .sweeps
            .iter()
            .find(|s| (&s.entry, &s.strategy, s.seed) == key)
            .ok_or_else(|| {
                format!(
                    "{} has a sweep for {} seed {} that {} never ran",
                    arm.path, arm_sweep.entry, arm_sweep.seed, full.path
                )
            })?;
        for arm_outcome in &arm_sweep.outcomes {
            let full_outcome = full_sweep
                .outcomes
                .iter()
                .find(|o| o.budget == arm_outcome.budget)
                .ok_or_else(|| {
                    format!(
                        "{} ran {} at budget {} but {} did not",
                        arm.path, arm_sweep.entry, arm_outcome.budget, full.path
                    )
                })?;
            if full_outcome.evaluations != arm_outcome.evaluations {
                return Err(format!(
                    "{} and {} charge {} and {} evaluations on {} seed {} budget {}: arms of one \
                     experiment spend the same budget",
                    full.path,
                    arm.path,
                    full_outcome.evaluations,
                    arm_outcome.evaluations,
                    arm_sweep.entry,
                    arm_sweep.seed,
                    arm_outcome.budget
                ));
            }

            let mut boundaries = Vec::new();
            if full_outcome.boundaries.len() != arm_outcome.boundaries.len() {
                return Err(format!(
                    "{} and {} report different numbers of boundaries on {} seed {} budget {}: \
                     the entry's own declaration cannot have changed",
                    full.path, arm.path, arm_sweep.entry, arm_sweep.seed, arm_outcome.budget
                ));
            }
            for (full_b, arm_b) in full_outcome.boundaries.iter().zip(&arm_outcome.boundaries) {
                boundaries.push((full_b.axis.clone(), full_b.error, arm_b.error));
                match (full_b.error, arm_b.error) {
                    (Some(_), Some(_)) => match (full_b.within, arm_b.within) {
                        (true, true) => boundary_tally.both_within += 1,
                        (true, false) => boundary_tally.lost_within += 1,
                        (false, true) => boundary_tally.gained_within += 1,
                        (false, false) => boundary_tally.neither_within += 1,
                    },
                    // One side had no band on the axis: the error does not exist, and reporting a
                    // zero for it would be inventing a boundary.
                    _ => boundary_tally.not_measurable += 1,
                }
            }

            let mut census = Vec::new();
            for full_c in &full_outcome.census.channels {
                let arm_c = arm_outcome
                    .census
                    .channels
                    .iter()
                    .find(|a| a.channel == full_c.channel)
                    .ok_or_else(|| {
                        format!(
                            "{} has no {} census on {} seed {} budget {}: a census that lists \
                             some channels and not others cannot be paired",
                            arm.path,
                            full_c.channel.name(),
                            arm_sweep.entry,
                            arm_sweep.seed,
                            arm_outcome.budget
                        )
                    })?;
                census.push(CensusDelta {
                    channel: full_c.channel,
                    full: full_c.clone(),
                    arm: arm_c.clone(),
                });
            }

            pairs.push(Pair {
                entry: arm_sweep.entry.clone(),
                seed: arm_sweep.seed,
                budget: arm_outcome.budget,
                detected: (full_outcome.detected, arm_outcome.detected),
                localised: (full_outcome.localised, arm_outcome.localised),
                findings: (full_outcome.findings, arm_outcome.findings),
                suspicious: (full_outcome.suspicious, arm_outcome.suspicious),
                trusted: (full_outcome.trusted, arm_outcome.trusted),
                unknown: (full_outcome.unknown, arm_outcome.unknown),
                steps: arm_outcome.instruction_steps.cast_signed()
                    - full_outcome.instruction_steps.cast_signed(),
                boundaries,
                census,
            });

            // Controls: no regions to find, so the comparison is over suspicion and trust. Ablating
            // evidence must never buy trust, and every pair where it did is named.
            if arm_sweep.control {
                let slot = controls
                    .iter_mut()
                    .find(|c| c.entry == arm_sweep.entry)
                    .expect("every control entry gets its slot before its pairs");
                slot.pairs += 1;
                let delta = arm_outcome.suspicious - full_outcome.suspicious;
                slot.sum += delta;
                if delta.abs() > slot.worst.0.abs() {
                    slot.worst = (delta, arm_sweep.seed, arm_outcome.budget);
                }
                let gain = arm_outcome.trusted - full_outcome.trusted;
                if gain > 0.0 {
                    trust_increases.push(TrustIncrease {
                        entry: arm_sweep.entry.clone(),
                        seed: arm_sweep.seed,
                        budget: arm_outcome.budget,
                        gain,
                        suspicion_from: arm_outcome.suspicious - full_outcome.suspicious,
                        unknown_from: arm_outcome.unknown - full_outcome.unknown,
                    });
                }
            }
        }

        // The sweep-level events, only where regions exist to localise.
        if arm_sweep.declared_regions == 0 {
            localisation.not_applicable += 1;
            detection.not_applicable += 1;
        } else {
            tally_event(
                &mut localisation,
                full_sweep.localised_at,
                arm_sweep.localised_at,
                arm_sweep,
            );
            tally_event(
                &mut detection,
                full_sweep.detected_at,
                arm_sweep.detected_at,
                arm_sweep,
            );
            // Vacuity is judged on the full arm at the largest budget it ran: did every channel
            // this arm silences have nothing to compute on this entry, on this seed? Every
            // region-bearing entry is present in the map (defaulting to silent), and a channel
            // that spoke on any seed makes the entry non-vacuous for this arm.
            if !arm.ablate.is_empty()
                && let Some(top) = full_sweep.outcomes.last()
            {
                let spoke = arm
                    .ablate
                    .iter()
                    .any(|c| top.census.of_channel(*c).is_some_and(|cc| cc.computed > 0));
                let slot = ever_computed
                    .entry(arm_sweep.entry.clone())
                    .or_insert(false);
                if spoke {
                    *slot = true;
                }
            }
        }
    }
    let vacuous: Vec<String> = ever_computed
        .into_iter()
        .filter(|(_, spoke)| !spoke)
        .map(|(entry, _)| entry)
        .collect();
    let controls = controls
        .into_iter()
        .map(|mut c| {
            let mean = if c.pairs > 0 {
                c.sum / c.pairs as f64
            } else {
                0.0
            };
            c.mean_delta = mean;
            c
        })
        .collect();

    Ok(Comparison {
        label: arm.label(),
        identity: arm.identity.clone(),
        silenced: arm.ablate.clone(),
        pairs,
        localisation,
        detection,
        boundaries: boundary_tally,
        controls,
        trust_increases,
        vacuous,
    })
}

fn tally_event(tally: &mut Tally, full: Option<u64>, arm: Option<u64>, sweep: &ArmSweep) {
    let key = (sweep.entry.clone(), sweep.seed);
    match (full, arm) {
        (Some(_), None) => tally.full_only.push(key),
        (None, Some(_)) => tally.arm_only.push(key),
        (Some(a), Some(b)) => match a.cmp(&b) {
            std::cmp::Ordering::Less => tally.full_earlier += 1,
            std::cmp::Ordering::Greater => tally.arm_earlier += 1,
            std::cmp::Ordering::Equal => tally.both_same += 1,
        },
        (None, None) => tally.neither += 1,
    }
}

/// The whole E2 reading of a set of documents: the full arm plus its ablations, as text.
#[must_use]
#[expect(
    clippy::too_many_lines,
    reason = "a report is a sequence of sections printed in order"
)]
pub fn report(full: &Arm, comparisons: &[Comparison]) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let budgets = full.sweeps.first().map_or_else(
        || "?".to_string(),
        |s| {
            s.outcomes
                .iter()
                .map(|o| o.budget.to_string())
                .collect::<Vec<_>>()
                .join(",")
        },
    );
    let _ = writeln!(
        out,
        "full arm {} (identity {}) against {} arm(s); budgets {budgets}",
        full.path,
        full.identity.as_deref().unwrap_or("?"),
        comparisons.len()
    );
    for c in comparisons {
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "{}  identity {}",
            c.label,
            c.identity.as_deref().unwrap_or("?")
        );
        let l = &c.localisation;
        let _ = writeln!(
            out,
            "  localisation: full-only {} {:?}, arm-only {} {:?}, both {} (full earlier {}, arm \
             earlier {}), neither {}, not applicable {}",
            l.full_only.len(),
            l.full_only
                .iter()
                .map(|(e, s)| format!("{e}:{s}"))
                .collect::<Vec<_>>(),
            l.arm_only.len(),
            l.arm_only
                .iter()
                .map(|(e, s)| format!("{e}:{s}"))
                .collect::<Vec<_>>(),
            l.both_same,
            l.full_earlier,
            l.arm_earlier,
            l.neither,
            l.not_applicable
        );
        let d = &c.detection;
        let _ = writeln!(
            out,
            "  detection:    full-only {}, arm-only {}, both {} (full earlier {}, arm earlier {}), \
             neither {}, not applicable {}",
            d.full_only.len(),
            d.arm_only.len(),
            d.both_same,
            d.full_earlier,
            d.arm_earlier,
            d.neither,
            d.not_applicable
        );
        let b = &c.boundaries;
        let _ = writeln!(
            out,
            "  boundaries:   {} lost tolerance, {} gained it, {} both within, {} neither, {} not \
             measurable",
            b.lost_within, b.gained_within, b.both_within, b.neither_within, b.not_measurable
        );
        if !c.controls.is_empty() {
            let _ = write!(out, "  controls:     ");
            let lines: Vec<String> = c
                .controls
                .iter()
                .map(|ctl| {
                    format!(
                        "{} mean {:+.5}, worst {:+.5} (seed {} budget {})",
                        ctl.entry, ctl.mean_delta, ctl.worst.0, ctl.worst.1, ctl.worst.2
                    )
                })
                .collect();
            let _ = writeln!(out, "{}", lines.join("; "));
        }
        if c.trust_increases.is_empty() {
            let _ = writeln!(
                out,
                "  trust:        no control pair trusted more after ablation"
            );
        } else {
            // The three volumes sum to the domain, so trust gained is always suspicion plus
            // ignorance lost; naming which pool paid is the whole content of the line.
            let suspicion_paid = c
                .trust_increases
                .iter()
                .filter(|t| t.suspicion_from < 0.0)
                .count();
            let ignorance_paid = c.trust_increases.len() - suspicion_paid;
            let _ = writeln!(
                out,
                "  trust:        {} control pair(s) trusted MORE after ablation ({} paid for by \
                 freed suspicion, {} by resolved UNKNOWN volume):",
                c.trust_increases.len(),
                suspicion_paid,
                ignorance_paid
            );
            for t in &c.trust_increases {
                let _ = writeln!(
                    out,
                    "    {} seed {} budget {}: trusted {:+.5}, from suspicion {:+.5} and unknown \
                     {:+.5}",
                    t.entry, t.seed, t.budget, t.gain, t.suspicion_from, t.unknown_from
                );
            }
        }
        if !c.vacuous.is_empty() {
            let _ = writeln!(
                out,
                "  vacuous here: {} (every silenced channel computed nothing on the full arm, so \
                 these nulls are corpus facts)",
                c.vacuous.join(", ")
            );
        }
    }
    out
}

/// The machine-readable form: one JSON object per arm, deterministic in the documents' own order.
#[must_use]
pub fn to_json(comparisons: &[Comparison]) -> Json {
    Json::Arr(comparisons.iter().map(Comparison::json).collect())
}
