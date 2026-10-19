//! An archive has to be able to say which measurement it is.
//!
//! `archive_name` encodes the axes a plan moves — entry, strategy, seed, mask, trust rule, scale rule —
//! and never the experiment, so two protocols whose arms share those six ask for one directory. E1.3b
//! found this while its own reference arm was being run: it shares an entry, a seed, a strategy, an empty
//! mask and the current rule with E1.x's committed `full` arm. The bytes at stake are the ones a published
//! number is later opened from, and a replacement archive is *self-consistent* — it digests its own
//! artefacts and replays its own records — so nothing else in the repository could have noticed.
//!
//! The guard that closed it compared configurations, which catches most of that and guesses at the rest.
//! These tests are for the exact version: every archive now records the measurement identity of the run
//! that wrote it, so a directory can be recognised without its document beside it, and the comparison is
//! a fact rather than an inference from which fields happen to differ.

use aporia_bench::corpus;
use aporia_bench::corpus_root;
use aporia_bench::harness::{Plan, archive_name, results_json, run_identity};
use aporia_boundary::Resolution;
use aporia_search::Strategy;
use aporia_store::{Environment, Json};

fn entry(id: &str) -> corpus::Entry {
    let entries = corpus::load(&corpus_root()).expect("the corpus loads");
    entries
        .iter()
        .find(|e| e.id() == id)
        .unwrap_or_else(|| panic!("{id} is not in the corpus"))
        .clone()
}

/// A plan that archives, with an identity the runner would have computed for it.
fn plan_with(archive: &std::path::Path, resolution: Resolution) -> Plan {
    let mut plan = Plan {
        budgets: vec![60, 120],
        strategies: vec![Strategy::Adaptive],
        seeds: vec![1],
        archive_dir: archive.to_path_buf(),
        ..Plan::default()
    };
    plan.resolution = resolution;
    plan
}

/// Write the manifest `aporia_store` would write for a given configuration and identity, keeping only the
/// two sections the guard reads. The configuration is produced by the real `Config::json`, so a change to
/// what an archive records shows up here rather than being agreed with itself.
fn plant(dir: &std::path::Path, plan: &Plan, identity: Option<&str>) -> std::path::PathBuf {
    let e = entry("granular/fit_band");
    let strategy = Strategy::Adaptive;
    let seed = plan.seeds[0];
    let budget = *plan.budgets.last().unwrap();
    let at = dir.join(archive_name(&e, plan, strategy, seed));
    std::fs::create_dir_all(&at).expect("create an archive directory");
    let mut notes = Vec::new();
    if let Some(identity) = identity {
        notes.push(("measurement_identity".to_string(), identity.to_string()));
    }
    // A manifest built by the type that writes them, rather than a shape somebody guessed at: the guard
    // now reads archives through `Manifest::from_json`, so a fixture that is not a manifest would test
    // the refusal for being unreadable instead of the refusal for belonging to another measurement.
    let manifest = aporia_store::Manifest {
        schema: aporia_store::manifest::SCHEMA.to_string(),
        tool_version: "0.1.0".to_string(),
        created_unix_ms: 0,
        model_name: e.name.clone(),
        model_sha256: "aa".repeat(32),
        air_sha256: "bb".repeat(32),
        config: plan.config(strategy, budget, seed).json(),
        counts: aporia_store::Counts {
            evaluations: budget,
            instruction_steps: budget * 6,
            params: e.model.as_ref().expect("the entry compiles").params.len() as u64,
            outputs: 1,
            constraints: 1,
            relations: 0,
            cells: 6,
            samples: budget,
            findings: 1,
        },
        exec_fp: "f64".to_string(),
        exec_max_steps: 100_000,
        environment: aporia_store::Environment {
            notes,
            ..aporia_store::Environment::current()
        },
        calibration: Vec::new(),
        correlation: Vec::new(),
        correlation_samples: 0,
        files: vec![("model.ap".to_string(), "cc".repeat(32))],
    };
    std::fs::write(at.join("manifest.json"), manifest.to_json().to_compact())
        .expect("write a manifest");
    at
}

fn environment() -> Environment {
    let mut environment = Environment::current();
    environment.notes.clear();
    environment
}

#[test]
fn the_identity_a_document_publishes_is_the_identity_an_archive_records() {
    // One definition, or the two proofs that a directory holds a published number disagree quietly.
    let e = entry("granular/fit_band");
    for resolution in [
        Resolution::Current,
        Resolution::Demand,
        Resolution::Withhold,
        Resolution::DemandAndWithhold,
    ] {
        let plan = plan_with(std::path::Path::new(""), resolution);
        let computed = run_identity(&plan, std::slice::from_ref(&e));
        let doc = results_json(&[], std::slice::from_ref(&e), &plan, &environment(), None);
        let published = doc
            .get("identity")
            .and_then(Json::as_str)
            .expect("a document names its measurement")
            .to_string();
        assert_eq!(computed, published, "{resolution:?}");
        assert_eq!(computed.len(), 12, "the identity is a digest prefix");
    }
}

#[test]
fn a_directory_that_belongs_to_another_measurement_is_refused_by_both_identities() {
    // Two cases that the configuration compare alone would have handled differently, and the reason the
    // identity is checked first: an archive whose recorded measurement is not this one is another
    // experiment's evidence whatever its fields happen to agree about.
    let dir = std::env::temp_dir().join(format!("aporia-archive-identity-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let e = entry("granular/fit_band");
    let entries = [e.clone()];

    let mut ours = plan_with(&dir, Resolution::Current);
    ours.provenance = aporia_bench::harness::Provenance {
        protocol: Some("e1-4-resolution-demand".to_string()),
        arm: Some("A-current".to_string()),
        protocol_digest: Some("0123456789abcdef".to_string()),
        pilot: false,
        measurement_identity: Some(run_identity(&ours, &entries)),
    };
    let ours_identity = run_identity(&ours, &entries);

    // A withholding arm over a control arm's directory: same entry, same seed, same mask, same trust
    // rule, same rates -- and a different measurement, which is exactly what the directory name cannot
    // express and the identity can.
    // The case the directory name cannot express, taken from the collision E1.3b actually hit: a plan
    // that differs only in its ladder names the same directory, because the name carries the entry, the
    // strategy, the seed and the axes an arm states -- never the budget the sweep stops at.
    let mut foreign = plan_with(&dir, Resolution::Current);
    foreign.budgets = vec![60, 120, 250];
    let foreign_identity = run_identity(&foreign, &entries);
    assert_ne!(
        foreign_identity, ours_identity,
        "the two runs were supposed to be different measurements"
    );
    assert_eq!(
        archive_name(&e, &foreign, Strategy::Adaptive, foreign.seeds[0]),
        archive_name(&e, &ours, Strategy::Adaptive, ours.seeds[0]),
        "if these names differed there would be no collision to refuse, and the test would be about          nothing"
    );
    let at = plant(&dir, &foreign, Some(&foreign_identity));
    let error = aporia_bench::cli::archive_namespace_guard(&ours, &entries)
        .expect_err("arm A may be handed a directory arm C already fills");
    for want in [
        ours_identity.as_str(),
        foreign_identity.as_str(),
        "--archive",
    ] {
        assert!(
            error.contains(want),
            "the refusal has to name {want:?}: {error}"
        );
    }
    std::fs::remove_dir_all(&at).ok();

    // And the case the identity cannot see: two runs of one measurement whose archives were written with
    // a different threshold. Because `certify_share` is recorded unconditionally in a manifest while it
    // is inert in an identity, the configuration compare still refuses rather than calling it a rerun.
    let mut inert = plan_with(&dir, Resolution::Current);
    inert.certify_share = 0.0625;
    assert_eq!(
        run_identity(&inert, &entries),
        ours_identity,
        "a bar no rule reads cannot be a different measurement"
    );
    let at = plant(&dir, &inert, Some(&ours_identity));
    let error = aporia_bench::cli::archive_namespace_guard(&ours, &entries)
        .expect_err("a differently configured archive was accepted as this measurement's");
    assert!(
        error.contains("configured differently"),
        "the second check is the configuration, and it should say so: {error}"
    );
    std::fs::remove_dir_all(&at).ok();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_archive_that_cannot_say_what_it_measures_is_not_replaced_on_an_assumption() {
    // The backward-compatible half. Archives written before this field existed are still readable and
    // still verifiable — nothing about their bytes changed — but a run cannot prove it would reproduce
    // them, so it is refused with the reason rather than allowed to overwrite evidence.
    let dir = std::env::temp_dir().join(format!("aporia-archive-legacy-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let e = entry("granular/fit_band");
    let entries = [e.clone()];

    let legacy = plan_with(&dir, Resolution::Current);
    let at = plant(&dir, &legacy, None);

    let mut ours = plan_with(&dir, Resolution::Current);
    ours.provenance = aporia_bench::harness::Provenance {
        measurement_identity: Some(run_identity(&ours, &entries)),
        ..aporia_bench::harness::Provenance::default()
    };
    let error = aporia_bench::cli::archive_namespace_guard(&ours, &entries)
        .expect_err("a manifest that records no identity must not be silently overwritten");
    assert!(
        error.contains("no measurement identity") && error.contains("predates"),
        "the refusal has to say why an unrecognised archive is not the same as an empty directory: \
         {error}"
    );
    std::fs::remove_dir_all(&at).ok();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_same_measurement_measured_twice_is_a_rerun_and_not_a_replacement() {
    // The other side of the same rule: refusing a rerun would make reproduction expensive, and the whole
    // point of the identity is to tell these two cases apart.
    let dir = std::env::temp_dir().join(format!("aporia-archive-rerun-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let e = entry("granular/fit_band");
    let entries = [e.clone()];

    let mut first = plan_with(&dir, Resolution::Demand);
    first.certify_share = 0.0625;
    let identity = run_identity(&first, &entries);
    let at = plant(&dir, &first, Some(&identity));

    first.provenance = aporia_bench::harness::Provenance {
        measurement_identity: Some(identity.clone()),
        ..aporia_bench::harness::Provenance::default()
    };
    aporia_bench::cli::archive_namespace_guard(&first, &entries)
        .expect("re-running the same measurement into its own directory is allowed");

    // And the plan section that made it is unchanged by having provenance attached: the identity digests
    // the measurement, never the record of who ran it.
    let mut bare = plan_with(&dir, Resolution::Demand);
    bare.certify_share = 0.0625;
    assert_eq!(
        run_identity(&bare, &entries),
        identity,
        "provenance moved the identity it is supposed to name"
    );
    std::fs::remove_dir_all(&at).ok();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_scale_rule_the_plan_does_not_name_cannot_share_an_archive_namespace() {
    // The four arms of E1.4 differ only in the axis this commit carries, so if the directory name did not
    // say it, four measurements would ask for the same bytes.
    let e = entry("granular/fit_band");
    let names: Vec<String> = Resolution::ALL
        .iter()
        .map(|rule| {
            archive_name(
                &e,
                &plan_with(std::path::Path::new(""), *rule),
                Strategy::Adaptive,
                1,
            )
        })
        .collect();
    let mut distinct = names.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        names.len(),
        "arms that write into one directory cannot both be evidence: {names:?}"
    );
    assert_eq!(
        names[0], "granular-fit_band-adaptive-seed1",
        "the committed naming is unchanged for the default rule: {}",
        names[0]
    );
}

#[test]
fn an_archive_that_is_not_a_manifest_is_refused_rather_than_walked_past() {
    // The guard reads an archive through the type that owns its shape, which splits the two ways a
    // directory can fail to be evidence. A manifest this tool cannot read is not an empty directory:
    // treating it as one would let a run overwrite bytes nobody checked, and the whole reason the
    // identity exists is that a replacement archive is self-consistent enough to pass every later
    // check. Nothing about a committed archive's bytes moved — all 412 of them read under
    // `aporia.experiment/1`, which is the schema this refuses to guess past.
    let dir =
        std::env::temp_dir().join(format!("aporia-archive-unreadable-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let e = entry("granular/fit_band");
    let entries = [e.clone()];

    let planted = plan_with(&dir, Resolution::Current);
    let identity = run_identity(&planted, &entries);
    let at = plant(&dir, &planted, Some(&identity));
    let written = std::fs::read_to_string(at.join("manifest.json")).expect("the planted manifest");
    std::fs::write(
        at.join("manifest.json"),
        written.replace("aporia.experiment/1", "aporia.experiment/2"),
    )
    .expect("a manifest from a schema this reader does not know");

    let mut ours = plan_with(&dir, Resolution::Current);
    ours.provenance = aporia_bench::harness::Provenance {
        measurement_identity: Some(identity),
        ..aporia_bench::harness::Provenance::default()
    };
    let error = aporia_bench::cli::archive_namespace_guard(&ours, &entries)
        .expect_err("an unreadable archive must not be treated as an absent one");
    assert!(
        error.contains("not a readable manifest"),
        "the refusal has to name the archive as unreadable rather than report a measurement it never \
         stated: {error}"
    );
    std::fs::remove_dir_all(&at).ok();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn every_committed_archive_reads_and_none_of_them_names_its_measurement_yet() {
    // The compatibility claim, checked against the corpus rather than asserted in prose. Every published
    // number is opened from one of these directories, and the guard now reads them through
    // `Manifest::from_json`, so a drifted schema or a missing required field would stop E1.5 before it
    // ran — and would have been the moment a run overwrote bytes nobody checked.
    //
    // What the count of *absent* identities says is the more interesting half: not one archive in the
    // published corpus carries a measurement identity, including E1.4's, because they were written before
    // an archive could name its measurement. That is precisely the case the guard refuses by name rather
    // than reading charitably, so the refusal path is exercised on real bytes and not only on a fixture.
    // Re-running a historical arm to fill the note in would replace published evidence with
    // self-consistent bytes of a later build, which is the thing the immutability rule forbids; if the
    // second assertion below ever stops holding, that is what happened.
    let root = corpus_root().join("archives");
    let mut dirs: Vec<_> = std::fs::read_dir(&root)
        .expect("the committed archives are here")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.join("manifest.json").exists())
        .collect();
    dirs.sort();
    let mut read = 0usize;
    let mut named = 0usize;
    for dir in dirs {
        let path = dir.join("manifest.json");
        let text =
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let value = Json::parse(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let manifest = aporia_store::Manifest::from_json(&value)
            .unwrap_or_else(|e| panic!("{} reads as no manifest: {e}", path.display()));
        if let Some(identity) = manifest.measurement_identity() {
            assert_eq!(
                identity.len(),
                12,
                "{} stated a strange identity",
                path.display()
            );
            named += 1;
        }
        read += 1;
    }
    assert!(
        read > 400,
        "the published archives are here, and {read} read"
    );
    assert_eq!(
        named, 0,
        "{named} published archive(s) now name a measurement: either a historical arm was re-run for          its notes, which replaces bytes a reader already opened, or this experiment's own archives          have been committed and the assertion belongs to them rather than here"
    );
}
