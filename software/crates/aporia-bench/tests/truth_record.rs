//! `aporia.truth/3` is a schema gate, not a caption.
//!
//! The record an E1.3b entry carries — meaning, variables with units and spans, a mechanism key, the
//! channels it expects, its own resolvability claim, and where its truth came from — is only worth
//! having if a file cannot state it in a form a reader would drop. That is the same discipline that
//! makes a `where` expression a `/2`-only field: an older reader measures the envelope around a curved
//! region and calls it the region, and a reader that ignored `resolvable` would measure an entry whose
//! resolvability nobody claimed.

use aporia_bench::corpus;
use aporia_bench::truth::{TRUTH_SCHEMAS, Truth};
use aporia_store::Json;

fn corpus_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../benchmarks")
}

/// One committed truth document, read as a value so a test can edit it.
fn truth_file(entry: &str) -> Json {
    let (family, name) = entry.split_once('/').expect("family/name");
    let path = corpus_root().join(family).join(name).join("truth.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    Json::parse(&text).expect("a committed truth file is JSON")
}

/// The same object with one field replaced, or added when it is missing.
fn with_field(doc: &Json, key: &str, value: Json) -> Json {
    let Json::Obj(fields) = doc else {
        unreachable!("a truth file is an object");
    };
    let mut out = fields.clone();
    match out.iter_mut().find(|(name, _)| name == key) {
        Some((_, held)) => *held = value,
        None => out.push((key.to_string(), value)),
    }
    Json::Obj(out)
}

/// A document declared under `schema`, keeping everything else the committed entry stated.
fn redeclared(entry: &str, schema: &str) -> Json {
    let doc = truth_file(entry);
    let Json::Obj(fields) = &doc else {
        unreachable!();
    };
    let rewritten: Vec<(String, Json)> = fields
        .iter()
        .map(|(name, value)| {
            (
                name.clone(),
                if name == "schema" {
                    Json::text(schema)
                } else {
                    value.clone()
                },
            )
        })
        .collect();
    Json::Obj(rewritten)
}

/// The full `/3` record, for a model whose one parameter is named `x` and spans `[0, 1]`.
fn record() -> Vec<(&'static str, Json)> {
    vec![
        (
            "meaning",
            Json::text("a readout that saturates against its ceiling"),
        ),
        (
            "variables",
            Json::Arr(vec![Json::object(vec![
                ("name", Json::text("x")),
                ("unit", Json::text("1")),
                ("role", Json::text("the normalised setpoint")),
                ("lo", Json::number(0.0)),
                ("hi", Json::number(1.0)),
            ])]),
        ),
        (
            "mechanism",
            Json::text("saturation against a declared ceiling"),
        ),
        (
            "channels",
            Json::Arr(vec![Json::text("physical"), Json::text("behavioral")]),
        ),
        ("resolvable", Json::Bool(true)),
        (
            "resolvability",
            Json::text(
                "leaf width 1/64 of the span; the region is 1/2 of it, so 32 leaves fit across",
            ),
        ),
        (
            "source",
            Json::text("the closed form of the declared rule, solved by hand"),
        ),
        (
            "why",
            Json::text("it is the case E1.3 could not reach, widened until the atlas can"),
        ),
    ]
}

/// A `/3` document carrying the record, minus one field named by `drop`.
fn with_record(drop: Option<&str>) -> Json {
    let mut doc = redeclared("edge/overflow_tail", "aporia.truth/3");
    for (key, value) in record() {
        if drop == Some(key) {
            continue;
        }
        doc = with_field(&doc, key, value);
    }
    doc
}

#[test]
fn a_record_stated_under_an_older_schema_is_refused_by_name() {
    // The discipline the `where` gate already enforces, applied to the new fields: a reader that
    // silently dropped `resolvable` would be measuring an entry whose resolvability nobody claimed, and
    // E1.3b's whole universe is chosen from that claim.
    let base = truth_file("edge/overflow_tail");
    assert_eq!(
        base.get("schema").and_then(Json::as_str),
        Some("aporia.truth/2"),
        "this test is about a `/2` file, and it must really be one"
    );
    for (key, value) in record() {
        let edited = with_field(&base, key, value.clone());
        let error = Truth::from_json(&edited).unwrap_err();
        assert!(
            error.contains(key) && error.contains("aporia.truth/3"),
            "the refusal must name the field and the schema that defines it: {error}"
        );
    }
    // And a `/1` file is refused the same way, so the gate is about the schema and not about one version
    // bump.
    let v1 = redeclared("geometry/diagonal_sum", "aporia.truth/1");
    let error = Truth::from_json(&with_field(&v1, "mechanism", Json::text("anything")))
        .expect_err("a /1 file may not state a /3 field either");
    assert!(
        error.contains("mechanism") && error.contains("aporia.truth/1"),
        "the refusal names the field and the schema the file declared: {error}"
    );
}

#[test]
fn a_schema_3_file_keeps_every_part_of_the_record_it_declares() {
    let truth = Truth::from_json(&with_record(None)).expect("a /3 file reads");
    assert_eq!(truth.schema, "aporia.truth/3");
    assert!(!truth.meaning.is_empty());
    assert_eq!(truth.mechanism, "saturation against a declared ceiling");
    assert_eq!(truth.channels, vec!["physical", "behavioral"]);
    assert_eq!(truth.resolvable, Some(true));
    assert!(!truth.resolvability.is_empty());
    assert!(!truth.source.is_empty());
    assert!(!truth.why.is_empty());
    let variable = truth.variables.first().expect("one declared variable");
    assert_eq!(variable.name, "x");
    assert_eq!(variable.unit, "1");
    assert_eq!((variable.lo, variable.hi), (0.0, 1.0));
    // A `/3` file is still a `/2` file in every way that decides a measure: the predicate this entry
    // carries survived the version bump, so the shape of a region did not change when the record did.
    assert!(
        truth
            .regions
            .first()
            .and_then(|r| r.predicate.as_ref())
            .is_some(),
        "the `where` a /2 entry declares must still be read by /3"
    );
    assert!(
        truth.matched_to.is_some(),
        "and so must its difficulty rung"
    );
}

#[test]
fn a_variable_is_read_in_full_or_not_at_all() {
    // An empty list is *read* — the reader is not the audit, and refusing an empty claim here would
    // hide the fact that the entry states one.
    let empty = with_field(
        &with_record(Some("variables")),
        "variables",
        Json::Arr(Vec::new()),
    );
    let truth = Truth::from_json(&empty).expect("an empty list still parses");
    assert!(truth.variables.is_empty());
    // What the reader refuses is a variable missing part of its own record, because a half-read
    // variable would be compared against the model and found equal by accident.
    for key in ["name", "unit", "role", "lo", "hi"] {
        let mut fields = vec![
            ("name".to_string(), Json::text("x")),
            ("unit".to_string(), Json::text("1")),
            ("role".to_string(), Json::text("setpoint")),
            ("lo".to_string(), Json::number(0.0)),
            ("hi".to_string(), Json::number(1.0)),
        ];
        fields.retain(|(name, _)| name != key);
        let doc = with_field(
            &with_record(Some("variables")),
            "variables",
            Json::Arr(vec![Json::Obj(fields)]),
        );
        let error = Truth::from_json(&doc).unwrap_err();
        assert!(
            error.contains(key),
            "the refusal must say which part is missing: {error}"
        );
    }
    // A non-finite or absent bound is refused where it is read, because every measure that would use it
    // is a product of widths.
    let doc = with_field(
        &with_record(Some("variables")),
        "variables",
        Json::Arr(vec![Json::object(vec![
            ("name", Json::text("x")),
            ("unit", Json::text("1")),
            ("role", Json::text("setpoint")),
            ("lo", Json::Null),
            ("hi", Json::number(1.0)),
        ])]),
    );
    assert!(
        Truth::from_json(&doc).is_err(),
        "a variable whose span is not a number is refused"
    );
    let doc = with_field(
        &with_record(Some("variables")),
        "variables",
        Json::Arr(vec![Json::object(vec![
            ("name", Json::text("x")),
            ("unit", Json::text("1")),
            ("role", Json::text("setpoint")),
            ("lo", Json::number(0.0)),
            ("hi", Json::text("one")),
        ])]),
    );
    assert!(
        Truth::from_json(&doc).is_err(),
        "a span written as prose is refused rather than defaulted"
    );
}

#[test]
fn the_three_schemas_are_the_whole_vocabulary_and_the_old_ones_still_read() {
    assert_eq!(TRUTH_SCHEMAS.len(), 3);
    assert_eq!(TRUTH_SCHEMAS[0], "aporia.truth/1");
    assert_eq!(TRUTH_SCHEMAS[1], "aporia.truth/2");
    assert_eq!(TRUTH_SCHEMAS[2], "aporia.truth/3");
    // Nothing about a committed file's meaning moved: the whole corpus still loads, every entry still
    // reports a schema it wrote, and none of them states a `/3` field.
    let entries = corpus::load(&corpus_root()).expect("the corpus loads");
    assert!(entries.len() > 20, "the committed corpus is still here");
    for entry in &entries {
        let doc = truth_file(&entry.id());
        assert!(
            doc.get("meaning").is_none() && doc.get("resolvable").is_none(),
            "{} is a committed entry and must not state the new record",
            entry.id()
        );
        assert!(
            doc.get("schema").and_then(Json::as_str) != Some("aporia.truth/3"),
            "{} was re-declared as /3 without this test knowing",
            entry.id()
        );
        assert!(
            TRUTH_SCHEMAS.contains(&entry.truth.schema.as_str()),
            "{} declares a schema this reader does not know",
            entry.id()
        );
    }
}
