//! A pilot is part of the pre-registration, so the freeze has to be able to state it.
//!
//! E1.4's stopping rule said the pilot was "one entry, two seeds and the two smallest budgets". Its
//! runner refuses every flag that shapes what is measured beside `--plan`, so there was no way for the
//! pilot to be smaller than the experiment: it ran the full 100-cell plan, and the deviation went into
//! the analysis as a defect of the freeze. `aporia.protocol/4` closes that by putting the scope in the
//! file, and these tests are the shape of the obligation — a pilot may be *smaller* than its experiment
//! in exactly the four dimensions the plan already declares, and nothing else.

use aporia_bench::corpus_root;
use aporia_bench::harness::{Plan, Protocol, Provenance, results_json};
use aporia_store::{Environment, Json};

fn environment() -> Environment {
    let mut environment = Environment::current();
    environment.notes.clear();
    environment
}

/// The committed E1.4 freeze, read as JSON.
fn e14() -> Json {
    let text = std::fs::read_to_string(
        corpus_root()
            .join("protocols")
            .join("e1-4-resolution-demand.json"),
    )
    .expect("the E1.4 freeze is committed");
    Json::parse(&text).expect("the freeze is JSON")
}

fn put(value: &Json, key: &str, field: Json) -> Json {
    let Json::Obj(fields) = value else {
        panic!("the fixture is an object");
    };
    let mut out = fields.clone();
    out.push((key.to_string(), field));
    Json::Obj(out)
}

/// Replace one field's value, leaving the rest of the object and its order alone.
fn with_value(value: &Json, key: &str, text: &str) -> Json {
    let Json::Obj(fields) = value else {
        panic!("the fixture is an object");
    };
    Json::Obj(
        fields
            .iter()
            .map(|(k, v)| {
                (
                    k.clone(),
                    if k == key {
                        Json::text(text)
                    } else {
                        v.clone()
                    },
                )
            })
            .collect(),
    )
}

/// The `/3` bytes carried into whichever schema and obligations the test wants: the schema renamed, the
/// plan and arms given whatever the new schema obliges, and the pilot section added or left out.
///
/// Promoting an E1.4 file is a bundle of edits rather than one, and the bundle is the point: a file
/// cannot gain `pilot` alone and keep the obligations of the schema it was written under, so the
/// fixture has to promote all of it to be a fair test of any one piece.
fn shaped(pilot: Option<&str>, schema: &str) -> Result<Protocol, String> {
    let value = with_value(&e14(), "schema", schema);
    let with_pilot = match pilot {
        None => value,
        Some(text) => put(
            &value,
            "pilot",
            Json::parse(text).expect("the pilot fixture is JSON"),
        ),
    };
    if schema == Protocol::SCHEMA_V3 {
        return Protocol::from_json(&with_pilot);
    }
    let Json::Obj(fields) = &with_pilot else {
        panic!("the fixture is an object");
    };
    let fields = fields
        .iter()
        .map(|(key, value)| match key.as_str() {
            "plan" => (
                key.clone(),
                put(value, "certify_density", Json::number(1_024.0)),
            ),
            "arms" => {
                let Json::Arr(arms) = value else {
                    panic!("arms is a list");
                };
                (
                    key.clone(),
                    Json::Arr(
                        arms.iter()
                            .map(|arm| put(arm, "certify", Json::text("size")))
                            .collect(),
                    ),
                )
            }
            other => (other.to_string(), value.clone()),
        })
        .collect();
    Protocol::from_json(&Json::Obj(fields))
}

fn promoted(pilot: &str) -> Result<Protocol, String> {
    shaped(Some(pilot), Protocol::SCHEMA_V4)
}

const A_PILOT: &str = r#"{"arms": ["A-current", "C-withhold"], "entries": ["control/symplectic_spring"], "budgets": [320, 640], "seeds": [1, 2], "checks": ["determinism", "namespace separation", "equal charged cost", "replay"]}"#;

fn control_entry() -> aporia_bench::corpus::Entry {
    aporia_bench::corpus::load(&corpus_root())
        .expect("the corpus loads")
        .into_iter()
        .find(|entry| entry.id() == "control/symplectic_spring")
        .expect("the registered control is in the corpus")
}

#[test]
fn a_frozen_pilot_narrows_the_plan_in_the_four_dimensions_and_in_no_other() {
    let protocol = promoted(A_PILOT).expect("a /4 file with a pilot reads");
    assert_eq!(protocol.schema, Protocol::SCHEMA_V4);
    let (plan, entries, frozen) = protocol
        .pilot("A-current")
        .expect("the arm is in the pilot");
    assert!(frozen.pilot, "and the run knows it is a pilot");
    assert_eq!(plan.budgets, vec![320, 640]);
    assert_eq!(plan.seeds, vec![1, 2]);
    assert_eq!(entries, vec!["control/symplectic_spring".to_string()]);

    // The narrowing is real in every dimension the freeze named, and the experiment's own plan stays
    // the wider one.
    let (full, full_frozen) = protocol.arm("A-current").expect("the full arm reads");
    assert!(
        !full_frozen.pilot,
        "arm() is the experiment: only --pilot, which calls pilot(), produces a pilot"
    );
    assert!(full.budgets.len() > plan.budgets.len());
    assert!(full.seeds.len() > plan.seeds.len());
    assert!(protocol.entries.len() > entries.len());

    // What a pilot may *not* move: the arm's own axes. A pilot arm that silently ran a different mask,
    // trust rule or scale rule would be checking the plumbing of some other experiment.
    assert_eq!(plan.resolution, full.resolution);
    assert_eq!(plan.trust, full.trust);
    assert_eq!(plan.ablate, full.ablate);
    assert_eq!(plan.certify_share, full.certify_share);
    assert_eq!(plan.grid, full.grid);
    assert_eq!(plan.probe_every, full.probe_every);
    assert_eq!(plan.refine_every, full.refine_every);

    // And the reduced scope is a different measurement, which is what keeps a pilot's archives out of
    // the experiment's namespace rather than sitting on top of them.
    let entries = [control_entry()];
    assert_ne!(
        aporia_bench::harness::run_identity(&plan, &entries),
        aporia_bench::harness::run_identity(&full, &entries),
        "a pilot that shared the full run's identity would be refused by the namespace guard as \
         already-measured rather than reported as itself"
    );
}

#[test]
fn a_pilot_that_is_not_smaller_than_its_experiment_is_refused_by_name() {
    let cases = [
        (
            "a budget the plan does not measure",
            r#"{"arms": ["A-current"], "entries": ["control/symplectic_spring"], "budgets": [9999], "seeds": [1], "checks": ["determinism"]}"#,
        ),
        (
            "a seed the plan does not measure",
            r#"{"arms": ["A-current"], "entries": ["control/symplectic_spring"], "budgets": [320], "seeds": [77], "checks": ["determinism"]}"#,
        ),
        (
            "an entry outside entries.run",
            r#"{"arms": ["A-current"], "entries": ["ode/euler_decay"], "budgets": [320], "seeds": [1], "checks": ["determinism"]}"#,
        ),
        (
            "an arm this file does not have",
            r#"{"arms": ["E-not-an-arm"], "entries": ["control/symplectic_spring"], "budgets": [320], "seeds": [1], "checks": ["determinism"]}"#,
        ),
        (
            "an empty entry list",
            r#"{"arms": ["A-current"], "entries": [], "budgets": [320], "seeds": [1], "checks": ["determinism"]}"#,
        ),
        (
            "an empty check list",
            r#"{"arms": ["A-current"], "entries": ["control/symplectic_spring"], "budgets": [320], "seeds": [1], "checks": []}"#,
        ),
        (
            "a repeated budget, which would make the pilot's own row count ambiguous",
            r#"{"arms": ["A-current"], "entries": ["control/symplectic_spring"], "budgets": [320, 320], "seeds": [1], "checks": ["determinism"]}"#,
        ),
    ];
    for (why, pilot) in cases {
        let error = promoted(pilot)
            .err()
            .unwrap_or_else(|| panic!("a pilot with {why} read successfully"));
        assert!(
            error.contains("pilot"),
            "the refusal for {why} has to name the section: {error}"
        );
    }
}

#[test]
fn a_freeze_cannot_acquire_a_pilot_after_its_own_run() {
    // `/3` has no pilot, so a file that keeps the `/3` name and grows the section has been edited after
    // the fact. The digest would change, which is the point — and the reader refuses it first.
    // `shaped` with the `/3` name produces exactly that document: the committed bytes, same schema,
    // with a section added that only `/4` is allowed to carry.
    let error = shaped(Some(A_PILOT), Protocol::SCHEMA_V3)
        .expect_err("a /3 file that grew a pilot section is refused");
    assert!(
        error.contains(Protocol::SCHEMA_V4) && error.contains("editing the pre-registration"),
        "the refusal has to say which schema owns the section and why it cannot appear here: {error}"
    );

    // The other direction: `/4` without the section is refused, because a `/4` file that could be
    // written without one is a `/3` file with a new name and no new obligation.
    let error = shaped(None, Protocol::SCHEMA_V4).expect_err("a /4 file with no pilot is refused");
    assert!(
        error.contains("states no pilot section") && error.contains("full plan"),
        "the refusal has to name the defect this schema exists to close: {error}"
    );
}

#[test]
fn only_a_pilot_run_marks_its_document() {
    let plan = Plan::default();
    let full = results_json(&[], &[], &plan, &environment(), None);
    assert!(
        full.get("pilot").is_none(),
        "a run nobody asked to be a pilot does not claim to be one, and its bytes are unchanged by \
         the option existing"
    );
    let frozen = aporia_bench::harness::Frozen {
        id: "e1-5-example".to_string(),
        arm: "A-current".to_string(),
        question: "does the plumbing hold".to_string(),
        protocol_digest: Some("0123456789abcdef".to_string()),
        pilot: true,
    };
    let marked = results_json(&[], &[], &plan, &environment(), Some(&frozen));
    assert_eq!(marked.get("pilot").and_then(Json::as_bool), Some(true));

    // The archive provenance carries the same fact, so the bytes a pilot leaves behind say so in their
    // own manifest rather than only in whichever directory somebody passed.
    let mut with_provenance = plan;
    with_provenance.provenance = Provenance {
        protocol: Some("e1-5-example".to_string()),
        arm: Some("A-current".to_string()),
        protocol_digest: Some("0123456789abcdef".to_string()),
        pilot: true,
        measurement_identity: Some("0000000000000000".to_string()),
    };
    let document = results_json(&[], &[], &with_provenance, &environment(), Some(&frozen));
    assert_eq!(
        document.get("pilot").and_then(Json::as_bool),
        Some(true),
        "the document and the archive have to tell the same story about what ran"
    );
}

#[test]
fn asking_for_an_arm_the_pilot_excluded_is_refused_rather_than_run_anyway() {
    let protocol = promoted(A_PILOT).expect("the file reads");
    let error = protocol
        .pilot("B-demand")
        .expect_err("B-demand is not in the frozen pilot scope");
    assert!(
        error.contains("does not include arm") && error.contains("A-current"),
        "the refusal has to name the arms the pilot does run: {error}"
    );
    // The same arm runs in full, and that is the distinction the refusal is making.
    assert!(protocol.arm("B-demand").is_ok());
}
