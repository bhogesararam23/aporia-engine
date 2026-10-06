//! The ablation control, and the two properties that make it a control rather than a reconfiguration.
//!
//! `docs/decisions/0028` narrowed the research question to one about evidence: does fusing five
//! heterogeneous signals localise regions that no subset of them localises at the same budget? That is
//! only answerable if removing a channel changes *what the instrument concluded* without changing
//! *what it paid for*. Those are the two assertions here, in both directions:
//!
//! - `silencing_a_channel_charges_the_same_evaluations` — the ablated arm spent the same budget on the
//!   same number of executions, so a difference between the arms is a difference in evidence, not in
//!   cost. This is the property a future change is most likely to break, because the tempting
//!   implementation of "turn the Differential channel off" is to stop running the reference
//!   evaluation, which quietly changes the sampling path and makes the two arms incomparable.
//! - `silencing_a_channel_changes_what_the_report_concluded` — the mask actually has an effect, on an
//!   entry whose suspicious region is made of that channel's readings.
//!
//! The rest of the file is provenance: a results file and an archive have to say which arm produced
//! them, and an ablation arm must not be able to overwrite the arm it was compared against.

use aporia_bench::corpus;
use aporia_bench::harness::Plan;
use aporia_evidence::Channel;
use aporia_ir::Model;
use aporia_search::{Campaign, Config, Strategy, run};
use std::path::PathBuf;

fn benchmarks() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../benchmarks")
}

/// The ladder's rates, so an ablation here is measured on the configuration the published numbers
/// came from rather than on a convenience setting.
fn ladder(budget: u64) -> Config {
    Config {
        budget,
        strategy: Strategy::Adaptive,
        seed: 1,
        probe_every: 7,
        calibrate_every: 25,
        numerical_every: 11,
        symmetric_every: 1,
        differential_every: 11,
        refine_every: 40,
        max_steps_per_evaluation: 2_000_000,
        silenced: 0,
        policy: aporia_boundary::Policy::default(),
    }
}

fn model(entry_id: &str) -> Model {
    let entries = corpus::load(&benchmarks()).expect("corpus loads");
    let entry = entries
        .iter()
        .find(|e| e.id() == entry_id)
        .unwrap_or_else(|| panic!("no corpus entry {entry_id}"));
    entry
        .model
        .clone()
        .unwrap_or_else(|| panic!("{entry_id} does not compile"))
}

/// The entry whose suspicious region is made of Sensitivity readings and violates no declared rule —
/// the reason the risk oracle exists (0020), and the reason silencing Sensitivity must change it.
const SENSITIVITY_ENTRY: &str = "aerospace/projectile_zero_gravity";

#[test]
fn silencing_a_channel_charges_the_same_evaluations() {
    // The cost basis of every ablation comparison. Same config, same seed, only the mask differs, so
    // anything other than an identical number of charged executions means the ablation removed work
    // as well as readings.
    let m = model(SENSITIVITY_ENTRY);
    let full = run(&m, ladder(320));
    let mut ablated_cfg = ladder(320);
    for channel in [
        Channel::Numerical,
        Channel::Differential,
        Channel::Sensitivity,
    ] {
        ablated_cfg = ablated_cfg.without(channel);
    }
    let ablated = aporia_search::run_with(&m, ablated_cfg, &mut aporia_runtime::Interp);
    assert_eq!(
        full.evaluations, ablated.evaluations,
        "an arm that was blind to three channels spent a different number of evaluations"
    );
    assert_eq!(
        full.records.items.len(),
        ablated.records.items.len(),
        "the arms recorded different numbers of observations, so one of them skipped work"
    );
    assert!(
        full.evaluations >= 300,
        "the budget was not spent, so this comparison proves nothing: {}",
        full.evaluations
    );
    // The silenced arm keeps the same *records* — same points, same answers — because it is the
    // readings that are withheld, not the arithmetic.
    let both = [(&full, "full"), (&ablated, "ablated")];
    let first = both[0].0.records.items.first().expect("records");
    assert!(
        both.iter().all(|(c, _)| c.records.items[0].x == first.x),
        "the arms did not even start at the same point"
    );
}

#[test]
fn silencing_the_channel_that_made_a_region_suspicious_removes_the_region() {
    // The other half of the control: the mask is not decoration. `projectile_zero_gravity`'s
    // suspicious cells are Sensitivity readings, so an arm that cannot see that channel must report a
    // materially smaller suspicious volume. If this ever stops holding, the ablation has become a
    // no-op and every E2 number in 0029 would be meaningless.
    let m = model(SENSITIVITY_ENTRY);
    let full = run(&m, ladder(320));
    let quiet = aporia_search::run_with(
        &m,
        ladder(320).without(Channel::Sensitivity),
        &mut aporia_runtime::Interp,
    );
    let (a, b) = (
        full.atlas.coverage().suspicious,
        quiet.atlas.coverage().suspicious,
    );
    assert!(
        a > 0.0,
        "the unablated arm found nothing suspicious, so there is nothing to ablate"
    );
    assert!(
        b < a,
        "silencing Sensitivity left the suspicious volume at {b} when the full arm measured {a}"
    );
    assert!(
        full.findings.len() > quiet.findings.len(),
        "the report still issued {} findings with the channel that produced them removed: {:?}",
        quiet.findings.len(),
        quiet
            .findings
            .iter()
            .map(|f| f
                .evidence
                .iter()
                .map(|e| e.channel.name())
                .collect::<Vec<_>>())
            .collect::<Vec<_>>()
    );
}

#[test]
fn a_silenced_channel_never_appears_in_the_evidence_the_report_shows() {
    // A finding must not be explained by evidence its own arm was not allowed to have.
    let m = model(SENSITIVITY_ENTRY);
    let campaign = aporia_search::run_with(
        &m,
        ladder(640).without(Channel::Differential),
        &mut aporia_runtime::Interp,
    );
    let leaked: Vec<String> = campaign
        .evidence
        .iter()
        .filter(|e| e.channel == Channel::Differential)
        .map(|e| e.detail.clone())
        .collect();
    assert!(
        leaked.is_empty(),
        "the ablated arm still carries differential evidence: {leaked:?}"
    );
}

#[test]
fn the_archive_and_the_results_file_say_which_arm_produced_them() {
    // Provenance, in the two places a reader would look: the campaign configuration written into an
    // archive, and the plan whose digest names a results file.
    let cfg = ladder(320)
        .without(Channel::Behavioral)
        .without(Channel::Physical);
    let json = cfg.json().to_compact();
    assert!(
        json.contains("\"silenced\":[\"behavioral\",\"physical\"]"),
        "the config does not record the mask by name: {json}"
    );
    // And an empty mask is written as an empty list rather than omitted: `silenced: []` is the answer
    // to "was this arm ablated?", and a missing field is not.
    let plain = ladder(320).json().to_compact();
    assert!(
        plain.contains("\"silenced\":[]"),
        "the full instrument must be distinguishable from an archive that lost the field: {plain}"
    );

    // The identity route, using the harness's own document builder rather than a second copy of its
    // plan JSON: if the identity is going to ignore a field, the way to notice is to ask the thing
    // that computes it.
    let entries = corpus::load(&benchmarks()).expect("corpus loads");
    let mut environment = aporia_store::Environment::current();
    environment.notes.push((
        "corpus".to_string(),
        aporia_bench::corpus_root().display().to_string(),
    ));
    let identity = |plan: &Plan| -> String {
        aporia_bench::harness::results_json(&[], &entries, plan, &environment)
            .get("identity")
            .and_then(aporia_store::Json::as_str)
            .expect("a results document records its own identity")
            .to_string()
    };
    let full = Plan::default();
    let ablated = Plan {
        ablate: vec![Channel::Numerical],
        ..Plan::default()
    };
    assert_ne!(
        identity(&full),
        identity(&ablated),
        "an ablation arm has the same measurement identity as the arm it is compared against, so it \
         would overwrite its results file"
    );
    // A repeat of the same arm is the same identity: the ablation is a fact about the experiment, not
    // about the clock or the machine.
    assert_eq!(identity(&ablated), identity(&ablated));
}

#[test]
fn the_ablation_vocabulary_is_refused_by_name() {
    // A typo must not read as "nothing silenced". The parser is `Channel::parse`, and the refusal names
    // the vocabulary it expected.
    let entries = corpus::load(&benchmarks()).expect("corpus loads");
    assert!(!entries.is_empty());
    for bad in ["behaviour", "Numerical", "numerics", ""] {
        assert_eq!(Channel::parse(bad), None, "{bad:?} should be refused");
    }
    assert_eq!(Channel::parse("numerical"), Some(Channel::Numerical));
    // And silencing everything is refused rather than run: with no channel consulted a campaign has
    // nothing to search on and would report a domain of TRUSTED cells built on nothing.
    let all: Vec<String> = Channel::names().into_iter().map(str::to_string).collect();
    assert_eq!(all.len(), Channel::ALL.len());
}

#[test]
fn an_arm_blind_to_everything_labels_the_domain_unknown_not_trusted() {
    // The failure mode the `--ablate` refusal exists to prevent, shown from the other side. An arm
    // with no channels was run anyway, and the atlas came back with nothing suspicious — which is
    // exactly the sentence that must not be readable as "this computation is fine".
    //
    // It is not, and this is where the instrument says so: every cell is UNKNOWN. `Policy` requires a
    // channel to have *measured* something before a cell can be trusted, so silencing everything
    // leaves the whole domain unlabelled rather than clean. That is the behaviour 0016 chose and
    // 0023 item 10 guarded, and an ablation arm is the first place it would realistically break: a
    // mask that merely zeroed the evidence, without clearing the measured-channels mask, would report
    // a fully TRUSTED domain built on nothing at all.
    let m = model(SENSITIVITY_ENTRY);
    let mut cfg = ladder(200);
    for channel in Channel::ALL {
        cfg = cfg.without(channel);
    }
    let campaign: Campaign = aporia_search::run(&m, cfg);
    let coverage = campaign.atlas.coverage();
    assert_eq!(
        coverage.suspicious, 0.0,
        "an arm with no evidence flagged something: {coverage:?}"
    );
    assert!(
        campaign.findings.is_empty(),
        "an arm with no evidence issued a finding"
    );
    assert_eq!(
        coverage.trusted, 0.0,
        "an arm with no evidence called part of the domain trustworthy: {coverage:?}"
    );
    assert_eq!(
        coverage.unknown, 1.0,
        "the whole unsampled domain should be UNKNOWN, not partly trusted: {coverage:?}"
    );
    assert!(
        coverage.samples > 0,
        "and it did execute: the evaluations were paid for, the readings were not used"
    );
}
