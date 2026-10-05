//! Comparing two archives, one field at a time.
//!
//! These tests hold two manifests in memory rather than two directories on disk, because what is
//! being checked is the comparison's own rules: that a field both sides have is `Same` or `Changed`,
//! that a field only one side has is `OnlyA`/`OnlyB` and never a change, that both values travel with
//! the verdict, and that the field order repeats. The archive-level end of the same code — findings,
//! the atlas, artefact digests — is tested against real directories below.

use aporia_store::compare::{Section, calibration, configuration, identity, size};
use aporia_store::manifest::{Counts, Environment, SCHEMA};
use aporia_store::{Change, Json, Manifest};

fn one() -> Manifest {
    Manifest {
        schema: SCHEMA.to_string(),
        tool_version: "0.1.0".to_string(),
        created_unix_ms: 0,
        model_name: "sqrt_domain".to_string(),
        model_sha256: "aa".repeat(32),
        air_sha256: "bb".repeat(32),
        config: Json::object(vec![
            ("budget", Json::count(40)),
            ("strategy", Json::text("adaptive")),
            ("seed", Json::count(1)),
        ]),
        counts: Counts {
            evaluations: 40,
            instruction_steps: 900,
            params: 1,
            outputs: 1,
            constraints: 1,
            relations: 0,
            cells: 6,
            samples: 40,
            findings: 1,
        },
        exec_fp: "f64".to_string(),
        exec_max_steps: 100_000,
        environment: Environment {
            os: "windows".to_string(),
            arch: "x86_64".to_string(),
            pointer_width: 64,
            rust_channel: "1.99.0".to_string(),
            cpu_features: Vec::new(),
            notes: vec![("execution".to_string(), "interpreter".to_string())],
        },
        calibration: vec![
            ("behavioral".to_string(), 1.0),
            ("physical".to_string(), 2.5),
        ],
        correlation: vec![("behavioral".to_string(), "physical".to_string(), 0.4)],
        correlation_samples: 33,
        files: vec![("model.ap".to_string(), "cc".repeat(32))],
    }
}

fn edited(f: impl FnOnce(&mut Manifest)) -> Manifest {
    let mut m = one();
    f(&mut m);
    m
}

#[test]
fn one_manifest_compared_with_itself_reports_nothing_changed() {
    let a = one();
    let b = one();
    let fields = [
        identity(&a, &b),
        configuration(&a, &b),
        size(&a, &b),
        calibration(&a, &b),
    ]
    .concat();
    assert!(!fields.is_empty(), "the comparison read nothing");
    assert!(
        fields.iter().all(|f| f.change == Change::Same),
        "{:?}",
        fields
            .iter()
            .filter(|f| f.change != Change::Same)
            .map(|f| (&f.path, &f.a, &f.b))
            .collect::<Vec<_>>()
    );
}

#[test]
fn a_different_budget_is_the_only_field_that_moves() {
    let a = one();
    let b = edited(|m| {
        m.config = Json::object(vec![
            ("budget", Json::count(640)),
            ("strategy", Json::text("adaptive")),
            ("seed", Json::count(1)),
        ]);
    });
    let fields = configuration(&a, &b);
    let budget = fields
        .iter()
        .find(|f| f.path == "config.budget")
        .expect("the config's budget is compared");
    assert_eq!(budget.change, Change::Changed);
    // Both values travel with the verdict: "changed" on its own is not an actionable answer.
    assert_eq!(budget.a.as_deref(), Some("40"));
    assert_eq!(budget.b.as_deref(), Some("640"));
    assert_eq!(
        fields
            .iter()
            .filter(|f| f.change != Change::Same)
            .map(|f| f.path.as_str())
            .collect::<Vec<_>>(),
        vec!["config.budget"]
    );
}

#[test]
fn a_field_one_archive_lacks_is_that_side_and_not_a_change() {
    let a = one();
    let b = edited(|m| {
        m.config = Json::object(vec![("budget", Json::count(40)), ("seed", Json::count(1))]);
    });
    let strategy = configuration(&a, &b)
        .into_iter()
        .find(|f| f.path == "config.strategy")
        .expect("a key only the first archive has is still reported");
    assert_eq!(strategy.change, Change::OnlyA);
    assert_eq!(strategy.a.as_deref(), Some("adaptive"));
    assert_eq!(strategy.b, None, "absence must not be rendered as a value");
    let back = configuration(&b, &a)
        .into_iter()
        .find(|f| f.path == "config.strategy")
        .unwrap();
    assert_eq!(back.change, Change::OnlyB);
}

#[test]
fn an_environment_note_present_in_one_run_is_named_by_key() {
    // The benchmark writes `entry`, `strategy` and `seed` notes; the command line writes `execution`.
    // Notes held by only one side are the ordinary case when comparing the two.
    let a = one();
    let b = edited(|m| {
        m.environment
            .notes
            .push(("entry".to_string(), "analytic/sqrt_domain".to_string()));
    });
    let entry = configuration(&a, &b)
        .into_iter()
        .find(|f| f.path == "note.entry")
        .expect("the second archive's note is reported");
    assert_eq!(entry.change, Change::OnlyB);
    assert_eq!(entry.b.as_deref(), Some("analytic/sqrt_domain"));
}

#[test]
fn calibration_is_compared_per_channel_because_scores_are_not_otherwise_comparable() {
    let a = one();
    let b = edited(|m| {
        m.calibration = vec![
            ("behavioral".to_string(), 1.0),
            ("physical".to_string(), 9.0),
        ];
    });
    let fields = calibration(&a, &b);
    let check = |path: &str, want: Change| {
        assert_eq!(
            fields
                .iter()
                .find(|f| f.path == path)
                .unwrap_or_else(|| panic!("no field {path}"))
                .change,
            want,
            "{:?}",
            fields.iter().map(|f| &f.path).collect::<Vec<_>>()
        );
    };
    check("scale.physical", Change::Changed);
    check("scale.behavioral", Change::Same);
    check("correlation.behavioral/physical", Change::Same);
    check("correlation_samples", Change::Same);
}

#[test]
fn a_channel_calibrated_in_one_run_only_is_reported_as_missing_in_the_other() {
    let a = one();
    let b = edited(|m| {
        m.calibration = vec![("behavioral".to_string(), 1.0)];
    });
    let fields = calibration(&a, &b);
    let physical = fields
        .iter()
        .find(|f| f.path == "scale.physical")
        .expect("a channel present in only one calibration is reported");
    assert_eq!(physical.change, Change::OnlyA);
    assert_eq!(physical.a.as_deref(), Some("2.5"));
}

#[test]
fn counts_that_move_are_named_with_their_two_numbers() {
    let a = one();
    let b = edited(|m| {
        m.counts.instruction_steps = 1_200;
        m.counts.cells = 7;
    });
    let fields = size(&a, &b);
    let steps = fields
        .iter()
        .find(|f| f.path == "instruction_steps")
        .expect("instruction steps are compared");
    assert_eq!(steps.change, Change::Changed);
    assert_eq!(steps.a.as_deref(), Some("900"));
    assert_eq!(steps.b.as_deref(), Some("1200"));
    assert_eq!(
        fields
            .iter()
            .filter(|f| f.change == Change::Changed)
            .map(|f| f.path.as_str())
            .collect::<Vec<_>>(),
        vec!["instruction_steps", "cells"],
        "an unrelated change must not be buried"
    );
}

#[test]
fn model_identity_is_compared_by_digest_as_well_as_by_name() {
    // Two runs can share a model *name* and hold different arithmetic, which is exactly the case a
    // reader of a diff must not be allowed to miss.
    let a = one();
    let b = edited(|m| m.air_sha256 = "dd".repeat(32));
    let fields = identity(&a, &b);
    assert_eq!(
        fields
            .iter()
            .find(|f| f.path == "model")
            .expect("the model name is compared")
            .change,
        Change::Same
    );
    assert_eq!(
        fields
            .iter()
            .find(|f| f.path == "air_sha256")
            .expect("the A-IR digest is compared")
            .change,
        Change::Changed
    );
}

#[test]
fn field_order_follows_the_archives_and_repeats() {
    let a = one();
    let b = edited(|m| {
        m.config = Json::object(vec![
            ("seed", Json::count(2)),
            ("budget", Json::count(40)),
            ("strategy", Json::text("adaptive")),
        ]);
    });
    let first = configuration(&a, &b);
    assert_eq!(first, configuration(&a, &b), "a comparison must repeat");
    let paths = first
        .iter()
        .map(|f| f.path.clone())
        .collect::<Vec<_>>()
        .join(" ");
    // The first archive's keys in the order it wrote them, then any the second added: not sorted, so
    // the report reads in the order the run produced its configuration.
    assert!(
        paths.starts_with("config.budget config.strategy config.seed exec.fp "),
        "{paths}"
    );
}

#[test]
fn every_field_carries_the_section_it_belongs_to() {
    let a = one();
    let b = one();
    for (section, fields) in [
        (Section::Identity, identity(&a, &b)),
        (Section::Run, configuration(&a, &b)),
        (Section::Size, size(&a, &b)),
        (Section::Calibration, calibration(&a, &b)),
    ] {
        assert!(
            fields.iter().all(|f| f.section == section),
            "{} put fields in the wrong section",
            section.name()
        );
    }
}
