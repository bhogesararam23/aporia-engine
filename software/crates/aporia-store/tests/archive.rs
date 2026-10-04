//! The archive, end to end: write an experiment, open it, replay it.
//!
//! This is where APORIA's reproducibility claim is either true or not. A stored run has to come back
//! with the same inputs, the same outputs including the NaNs, the same raised flags, the same
//! instruction-step counts and the same traces — and a directory whose files no longer match their
//! recorded digests has to refuse to pretend.

use aporia_boundary::{Atlas, Policy};
use aporia_dsl::lower::compile;
use aporia_evidence::{Calibrator, Channel, ChannelCorrelation, Evidence, Subject};
use aporia_ir::{Model, to_text};
use aporia_runtime::{ExecConfig, FpMode, Observation, Records, interp};
use aporia_store::{Environment, Json, Loaded, Run, Store, StoredFinding, replay, replay_dir};
use std::path::{Path, PathBuf};

const SOURCE: &str = "model stored \"\" {\n input x in [-10, 10]\n input dt : s in [0, 1]\n state e = 1.0\n loop 3 {\n advance e = e * 0.5\n watch e\n }\n let y = x * x\n let z = y / dt\n let w = dt / dt\n require dt > 0\n}\n";

fn scratch(name: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    dir.push(format!("aporia-store-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn executed(model: &Model, cfg: ExecConfig) -> Records {
    let mut r = Records::new();
    for i in 0..24 {
        let x = vec![
            -10.0 + i as f64 * 0.8,
            if i % 6 == 0 { 0.0 } else { 0.01 * i as f64 },
        ];
        let outcome = interp::run(model, &x, cfg);
        r.push(Observation::new(i, x, &outcome));
    }
    r
}

/// Write one complete experiment into a fresh directory.
///
/// Everything the `Run` borrows is built here so the lifetimes stay honest: the archive takes
/// references rather than copies, because a campaign's records are already in memory and an
/// experiment should not have to duplicate them to be written down.
fn write_dir(name: &str, cfg: ExecConfig) -> (PathBuf, Model, Records) {
    let compiled = compile("stored.ap", SOURCE);
    assert!(
        !compiled.diagnostics.has_errors(),
        "{}",
        compiled.diagnostics
    );
    let model = compiled.model;
    let records = executed(&model, cfg);
    let mut atlas = Atlas::new(&model, Policy::default());
    for o in &records.items {
        let risk = if o.y.iter().any(|v| v.is_nan()) {
            0.9
        } else {
            0.1
        };
        atlas.record(&o.x, risk, 0b10);
    }
    atlas.relabel();
    atlas.refine();
    atlas.relabel();

    let air = to_text(&model);
    let coverage = atlas.coverage();
    let atlas_csv = atlas.to_csv();
    let bands = atlas.bands();
    let evidence = vec![
        Evidence::new(
            Channel::Physical,
            Subject::Constraint(0),
            1.0,
            vec![0, 6],
            "dt > 0 violated: 0 not > 0".to_string(),
        )
        .absolute(1.0),
    ];
    // A calibration fitted on synthetic sensitivity magnitudes, because the manifest has to record
    // the scale the scores were produced with even when a test supplies them by hand.
    let calibration_source: Vec<Evidence> = (0..12)
        .map(|i| {
            Evidence::new(
                Channel::Sensitivity,
                Subject::LocalSlope {
                    output: 0,
                    axis: 0,
                },
                1.0 + i as f64 * 0.5,
                vec![i],
                String::new(),
            )
        })
        .collect();
    let calibrator = Calibrator::fit(calibration_source.iter());
    let correlation = ChannelCorrelation::none();
    let findings = vec![StoredFinding {
        index: 27,
        cell: 3,
        bounds: vec![[-10.0, 0.0], [0.0, 0.02]],
        representative: vec![-4.0, 0.0],
        observation: 0,
        online_risk: 0.81,
        final_risk: 0.93,
        samples: 9,
        label: "SUSPICIOUS".to_string(),
        case: Some("x = -4\ndt in [0, 0.02]".to_string()),
        evidence,
        raw_evidence: Vec::new(),
    }];
    let decisions = vec![
        Json::object(vec![
            ("evaluation", Json::count(0)),
            ("family", Json::text("novelty")),
        ]),
        Json::object(vec![
            ("evaluation", Json::count(1)),
            ("family", Json::text("boundary")),
        ]),
    ];
    let run = Run {
        model: &model,
        model_text: SOURCE,
        air_text: &air,
        records: &records,
        config: Json::object(vec![
            ("budget", Json::count(400)),
            ("strategy", Json::text("adaptive")),
        ]),
        coverage,
        atlas_csv: &atlas_csv,
        bands: &bands,
        findings: &findings,
        decisions: &decisions,
        calibrator: &calibrator,
        correlation: &correlation,
        exec: cfg,
        evaluations: records.items.len() as u64,
        instruction_steps: records.total_steps(),
        environment: Environment::current(),
        created_unix_ms: 1_760_000_000_000,
    };
    let root = scratch(name);
    let mut store = Store::create(&root).expect("create");
    let receipt = store.write(&run).expect("write");
    assert!(receipt.total_bytes() > 0);
    (root, model, records)
}

#[test]
fn a_written_experiment_reads_back_and_replays_bit_for_bit() {
    let cfg = ExecConfig::default();
    let (root, model, records) = write_dir("replay", cfg);
    let loaded = Loaded::open(&root).expect("open");
    assert_eq!(loaded.manifest.model_name, "stored");
    assert_eq!(loaded.records.items.len(), records.items.len());
    for (found, expected) in loaded.records.items.iter().zip(&records.items) {
        assert_eq!(found.id, expected.id);
        assert_eq!(found.steps, expected.steps);
        assert_eq!(found.flags.to_bits(), expected.flags.to_bits());
        for (a, b) in found.x.iter().zip(&expected.x) {
            assert_eq!(a.to_bits(), b.to_bits());
        }
        // The stored run contains divisions by zero; NaN has to come back as the same bits, and
        // NaN != NaN is exactly why this comparison is on bit patterns.
        for (a, b) in found.y.iter().zip(&expected.y) {
            assert_eq!(a.to_bits(), b.to_bits());
        }
        assert_eq!(found.traces, expected.traces);
    }

    let report = replay(&model, &loaded.records, cfg);
    assert!(report.reproduced, "{}", report.summary());
    assert_eq!(report.matched, report.total);
    assert!(report.mismatches.is_empty());
    assert!(report.integrity.is_empty());
    assert!(report.summary().contains("identically"));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_nan_that_came_from_division_by_zero_replays_as_the_same_number() {
    let cfg = ExecConfig::default();
    let (root, model, _) = write_dir("nan", cfg);
    let loaded = Loaded::open(&root).unwrap();
    let nans = loaded
        .records
        .items
        .iter()
        .filter(|o| o.y.iter().any(|v| v.is_nan()))
        .count();
    assert!(nans > 0, "the corpus of this test must contain a NaN");
    let report = replay(&model, &loaded.records, cfg);
    assert_eq!(report.matched, report.total, "{:?}", report.mismatches);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_directory_is_never_overwritten() {
    let cfg = ExecConfig::default();
    let (root, _, _) = write_dir("twice", cfg);
    let again = Store::create(&root);
    assert!(
        matches!(again, Err(aporia_store::StoreError::Exists(_))),
        "a rerun must not replace an experiment someone is reading"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn editing_the_model_after_the_run_is_detected_instead_of_replayed_around() {
    let cfg = ExecConfig::default();
    let (root, _, _) = write_dir("tampered", cfg);
    // The most common real case: somebody changes the model and re-reads the old directory.
    std::fs::write(
        root.join("model.ap"),
        SOURCE.replace("let y = x * x", "let y = x * x * x"),
    )
    .unwrap();
    let problems = Loaded::open(&root).unwrap().integrity_problems();
    assert!(
        problems.iter().any(|p| p.contains("model.ap")),
        "{problems:?}"
    );
    let report = replay_dir(&root).unwrap();
    assert!(!report.reproduced);
    assert!(!report.integrity.is_empty());
    assert!(report.summary().starts_with("REPLAY BLOCKED"));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_truncated_observation_file_is_refused_when_the_directory_is_opened() {
    let cfg = ExecConfig::default();
    let (root, _, _) = write_dir("truncated", cfg);
    let path = root.join("observations.bin");
    let mut bytes = std::fs::read(&path).unwrap();
    bytes.truncate(bytes.len() - 12);
    std::fs::write(&path, &bytes).unwrap();
    let e = Loaded::open(&root);
    assert!(
        e.is_err(),
        "a file that ends mid-record must not read as a shorter experiment"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_manifest_records_the_calibration_and_every_artefact_digest() {
    let cfg = ExecConfig::default();
    let (root, _, _) = write_dir("manifest", cfg);
    let loaded = Loaded::open(&root).unwrap();
    let m = &loaded.manifest;
    assert_eq!(m.schema, "aporia.experiment/1");
    assert_eq!(m.calibration.len(), 5, "one entry per channel");
    assert!(
        m.calibration
            .iter()
            .any(|(c, s)| c == "sensitivity" && *s > 0.0)
    );
    assert_eq!(m.exec_fp, "f64");
    assert_eq!(m.exec_max_steps, ExecConfig::default().max_steps);
    assert_eq!(m.counts.evaluations, 24);
    assert_eq!(m.counts.params, 2);
    assert_eq!(m.counts.findings, 1);
    for name in ["model.ap", "model.air", "observations.bin", "atlas.csv"] {
        assert!(
            m.files.iter().any(|(p, _)| p == name),
            "{name} has no digest in the manifest"
        );
    }
    assert_eq!(m.model_sha256.len(), 64);
    assert!(
        loaded.integrity_problems().is_empty(),
        "{:?}",
        loaded.integrity_problems()
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_finding_reads_like_the_report_the_spec_asks_for() {
    let cfg = ExecConfig::default();
    let (root, _, _) = write_dir("finding", cfg);
    let loaded = Loaded::open(&root).unwrap();
    assert_eq!(loaded.findings.len(), 1);
    let finding = &loaded.findings[0];
    assert_eq!(finding.index, 27);
    assert_eq!(finding.label, "SUSPICIOUS");
    assert_eq!(finding.final_risk, 0.93);
    // The evidence is not rehydrated into typed form, but nothing is lost: the raw objects come back.
    assert_eq!(finding.raw_evidence.len(), 1);
    assert_eq!(
        finding.raw_evidence[0]
            .get("channel")
            .and_then(Json::as_str),
        Some("physical")
    );
    // The rendered form is what a person sees, so the replay command has to name a real file.
    let text = finding.render();
    for needle in [
        "APORIA FINDING #000027",
        "SUSPICIOUS",
        "Minimal reproducible case",
        "dt > 0 violated",
        "findings/000027.apx",
    ] {
        assert!(text.contains(needle), "missing {needle:?} in:\n{text}");
    }
    let file = root.join("findings").join("000027.apx");
    assert!(file.exists(), "the replay command must point at something");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn decisions_survive_as_ordered_jsonl() {
    let cfg = ExecConfig::default();
    let (root, _, _) = write_dir("decisions", cfg);
    let loaded = Loaded::open(&root).unwrap();
    assert_eq!(loaded.decisions.len(), 2);
    assert_eq!(
        loaded.decisions[1].get("family").and_then(Json::as_str),
        Some("boundary")
    );
    let lines = std::fs::read_to_string(root.join("decisions.jsonl")).unwrap();
    assert_eq!(lines.lines().count(), 2, "one decision per line");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_run_replayed_in_f32_reproduces_under_the_recorded_configuration() {
    let cfg = ExecConfig {
        fp: FpMode::F32,
        max_steps: 1_000,
    };
    let (root, _, _) = write_dir("f32", cfg);
    let loaded = Loaded::open(&root).unwrap();
    assert_eq!(loaded.manifest.exec_fp, "f32");
    assert_eq!(loaded.manifest.exec_max_steps, 1_000);
    let model = aporia_ir::from_text(&loaded.air_text).unwrap();
    let report = replay(&model, &loaded.records, cfg);
    assert!(report.reproduced, "{:?}", report.mismatches);
    // Replaying the f32 archive with f64 arithmetic must *not* claim to reproduce it.
    let wrong = replay(&model, &loaded.records, ExecConfig::default());
    assert!(
        !wrong.reproduced || wrong.matched < wrong.total,
        "a replay in another precision matched anyway, which means the comparison is too weak"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_summary_and_csv_files_are_readable_text() {
    let cfg = ExecConfig::default();
    let (root, _, _) = write_dir("summary", cfg);
    let summary = std::fs::read_to_string(root.join("summary.json")).unwrap();
    let value = Json::parse(&summary).unwrap();
    assert_eq!(
        value
            .get("counts")
            .and_then(|c| c.get("evaluations"))
            .and_then(Json::as_u64),
        Some(24)
    );
    assert!(value.get("coverage").is_some());
    let bands = std::fs::read_to_string(root.join("bands.csv")).unwrap();
    assert!(bands.starts_with("axis,axis_name,lo,hi"));
    let atlas = std::fs::read_to_string(root.join("atlas.csv")).unwrap();
    assert!(atlas.contains("label"), "{atlas}");
    assert!(Path::new(&root).exists());
    let _ = std::fs::remove_dir_all(&root);
}
