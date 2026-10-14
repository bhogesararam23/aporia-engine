//! The trust variant as a dimension of a measurement, not a detail of a command line.
//!
//! E1.3b compares the rule that decides when a leaf may be called trusted. That makes the rule part of
//! what a row *means*, and the instrument has to be able to say which rule produced any row, refuse to
//! pair rows whose rules were never stated, and — the hard half — reproduce every committed measurement
//! exactly as it was, because the reference arm of the new experiment is the policy that wrote those
//! files.
//!
//! Each test here is one half of that requirement:
//!
//! - a default plan writes the bytes it wrote before the variant existed, and a published document's
//!   identity is still recomputable from them;
//! - a non-default variant moves the identity, so a variant row cannot overwrite the row it was
//!   measured against;
//! - a reader refuses a variant it cannot name, and a frozen arm of the new schema is refused when it
//!   names none;
//! - archives of two variants cannot land in one directory;
//! - `compare` pairs a variant against the current rule and refuses a comparison whose reference is
//!   itself a variant;
//! - a row counts the non-real readings its own atlas holds, which is what makes the non-finite
//!   variant's effect attributable rather than assumed.

use aporia_bench::cli;
use aporia_bench::corpus;
use aporia_bench::e2::{Arm, compare};
use aporia_bench::harness::{
    Plan, Protocol, RESULTS_SCHEMA, archive_name, identity_of, plan_json, results_json, sweep,
};
use aporia_boundary::Trust;
use aporia_evidence::Channel;
use aporia_search::Strategy;
use aporia_store::{Environment, Json};
use std::path::PathBuf;

fn environment() -> Environment {
    let mut environment = Environment::current();
    environment.notes.clear();
    environment
}

fn named(plan: &Plan) -> String {
    let doc = results_json(&[], &[], plan, &environment(), None);
    doc.get("identity")
        .and_then(Json::as_str)
        .expect("a document records its identity")
        .to_string()
}

/// The fields of any object this module builds or reads.
fn fields_of(value: &Json) -> Vec<(String, Json)> {
    let Json::Obj(fields) = value else {
        unreachable!("expected an object, got {}", value.to_compact());
    };
    fields.clone()
}

/// The same document with one top-level field replaced.
fn with_field(doc: &Json, key: &str, value: &Json) -> Json {
    Json::Obj(
        fields_of(doc)
            .into_iter()
            .map(|(name, held)| (name.clone(), if name == key { value.clone() } else { held }))
            .collect(),
    )
}

/// The same protocol with one field of one arm replaced or added.
fn with_arm_field(doc: &Json, at: usize, key: &str, value: &Json) -> Json {
    let mut fields = fields_of(doc);
    let (_, arms) = fields
        .iter_mut()
        .find(|(name, _)| name == "arms")
        .expect("a protocol states its arms");
    let Json::Arr(list) = arms else {
        unreachable!("arms is a list");
    };
    let arm = list
        .get_mut(at)
        .unwrap_or_else(|| panic!("the protocol has no arm at {at}"));
    let mut parts = fields_of(arm);
    match parts.iter_mut().find(|(n, _)| n == key) {
        Some((_, held)) => *held = value.clone(),
        None => parts.push((key.to_string(), value.clone())),
    }
    *arm = Json::Obj(parts);
    Json::Obj(fields)
}

fn append_arm(doc: &Json, arm: Json) -> Json {
    let mut fields = fields_of(doc);
    let (_, value) = fields
        .iter_mut()
        .find(|(name, _)| name == "arms")
        .expect("a protocol states its arms");
    let Json::Arr(list) = value else {
        unreachable!("arms is a list");
    };
    list.push(arm);
    Json::Obj(fields)
}

/// The same document with one top-level field removed.
fn without_field(doc: &Json, key: &str) -> Json {
    Json::Obj(
        fields_of(doc)
            .into_iter()
            .filter(|(name, _)| name != key)
            .collect(),
    )
}

/// The same document with one field of its plan section replaced, or added when it lacks one.
fn with_plan_field(doc: &Json, key: &str, value: Json) -> Json {
    let mut plan = fields_of(doc.get("plan").expect("a document carries its plan"));
    match plan.iter_mut().find(|(name, _)| name == key) {
        Some((_, held)) => *held = value,
        None => plan.push((key.to_string(), value)),
    }
    with_field(doc, "plan", &Json::Obj(plan))
}

/// The same protocol as a schema-2 file: the two entry lists that schema requires, written from its own
/// `entries.run`. Which of them the atlas can actually enclose is not what this test is about — that is
/// decided at load time by `frozen_run` and covered in `tests/reachability.rs` — so the split here is
/// the trivial one, every named entry in the resolvable set, none in the control.
fn as_schema_two(value: &Json) -> Json {
    let with_schema = with_field(value, "schema", &Json::text(Protocol::SCHEMA_V2));
    let entries_value = with_schema
        .get("entries")
        .expect("a protocol states its entries");
    let run = entries_value
        .get("run")
        .and_then(Json::as_array)
        .expect("entries.run is a list")
        .to_vec();
    let mut entries = fields_of(entries_value);
    for (name, held) in &mut entries {
        if name == "resolvable" {
            *held = Json::Arr(run.clone());
        }
    }
    if !entries.iter().any(|(name, _)| name == "resolvable") {
        entries.push(("resolvable".to_string(), Json::Arr(run)));
    }
    if !entries.iter().any(|(name, _)| name == "sub_granular") {
        entries.push(("sub_granular".to_string(), Json::Arr(Vec::new())));
    }
    with_field(&with_schema, "entries", &Json::Obj(entries))
}

/// One committed E1.x document, found by the arm name it records.
fn committed(arm: &str) -> (Json, PathBuf) {
    let mut found: Option<(Json, PathBuf)> = None;
    for (value, path) in documents() {
        if value
            .get("protocol")
            .and_then(|p| p.get("arm"))
            .and_then(Json::as_str)
            == Some(arm)
        {
            if let Some((_, earlier)) = &found {
                panic!(
                    "{} and {} both name the {arm:?} arm, so one of them is not the committed one",
                    earlier.display(),
                    path.display()
                );
            }
            found = Some((value, path));
        }
    }
    found.unwrap_or_else(|| panic!("no committed document names the {arm:?} arm"))
}

/// Every results document in the published directory, parsed. Files that are not documents — the
/// README that sits beside them — are skipped here and named by the test that counts them.
fn documents() -> Vec<(Json, PathBuf)> {
    let dir = aporia_bench::results_dir();
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
        let path = entry.expect("a readable directory entry").path();
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        if let Ok(value) = Json::parse(&text) {
            out.push((value, path));
        }
    }
    assert!(!out.is_empty(), "{} holds no documents", dir.display());
    out
}

fn geometry_text() -> String {
    let path = aporia_bench::corpus_root()
        .join("protocols")
        .join("e1-geometry.json");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn geometry() -> Json {
    Json::parse(&geometry_text()).expect("the frozen protocol is JSON")
}

fn anchor() -> corpus::Entry {
    let entries = corpus::load(&aporia_bench::corpus_root()).expect("the corpus loads");
    entries
        .iter()
        .find(|e| e.id() == "analytic/sqrt_domain")
        .expect("the anchor entry is in the corpus")
        .clone()
}

#[test]
fn the_default_variant_writes_the_bytes_that_named_a_committed_measurement() {
    // The reproduction gate's precondition, checked without running anything: a plan read back from a
    // committed E1.x document, which holds the *default* variant, serialises to the same plan section
    // and digests to the same identity as the file it came from. Had the variant been written into
    // `plan_json` unconditionally, this is the assertion that would have failed — and with it the claim
    // that the `full-current-trust` arm is the same experiment as the committed `full` arm rather than a
    // new one wearing its numbers.
    let (doc, path) = committed("full");
    let recorded = doc
        .get("plan")
        .expect("a committed document records its plan");
    let plan = Plan::from_json(recorded).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    assert_eq!(
        plan.trust,
        Trust::Current,
        "a document that names no variant was labelled under the only rule that existed"
    );
    assert_eq!(
        plan_json(&plan).to_compact(),
        recorded.to_compact(),
        "the default variant changes what a plan writes: {}",
        path.display()
    );
    let recomputed = identity_of(
        RESULTS_SCHEMA,
        recorded,
        doc.get("entries").expect("and its entries"),
    );
    assert_eq!(
        doc.get("identity").and_then(Json::as_str),
        Some(recomputed.as_str()),
        "so the published identity is still a digest of exactly these sections"
    );
    // The identity is also the file's name, which is how a rerun is refused as a duplicate of the
    // measurement it reproduces rather than written beside it — the behaviour the gate depends on.
    let stem = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    assert_eq!(
        stem,
        format!("results-{recomputed}.json"),
        "{} is not named after the measurement it holds",
        path.display()
    );
}

#[test]
fn every_committed_e1x_arm_still_names_itself_under_the_default_variant() {
    // The same claim as above over all eleven committed arms, because the gate compares against all of
    // them and one compatible file is not eleven. Each still reads, still means `current`, still
    // round-trips its plan section, and still digests to the identity in its own filename.
    let mut checked = 0usize;
    for (doc, path) in documents() {
        if doc
            .get("protocol")
            .and_then(|p| p.get("id"))
            .and_then(Json::as_str)
            != Some("e1-geometry")
        {
            continue;
        }
        let plan_value = doc
            .get("plan")
            .unwrap_or_else(|| panic!("{} records no plan", path.display()));
        let plan =
            Plan::from_json(plan_value).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert_eq!(plan.trust, Trust::Current, "{}", path.display());
        assert_eq!(
            plan_json(&plan).to_compact(),
            plan_value.to_compact(),
            "a committed plan section no longer round-trips: {}",
            path.display()
        );
        let recomputed = identity_of(
            RESULTS_SCHEMA,
            plan_value,
            doc.get("entries").expect("a document records its entries"),
        );
        assert_eq!(
            doc.get("identity").and_then(Json::as_str),
            Some(recomputed.as_str()),
            "{}",
            path.display()
        );
        checked += 1;
    }
    assert_eq!(checked, 11, "E1.x committed eleven arms");
}

#[test]
fn a_variant_moves_the_identity_and_the_default_does_not() {
    let base = Plan {
        budgets: vec![320],
        strategies: vec![Strategy::Adaptive],
        seeds: vec![1],
        ..Plan::default()
    };
    let current = named(&base);
    // Naming the current rule explicitly is the same measurement as not naming it: a plan cannot become
    // a different experiment because someone wrote down what it already was.
    let explicit = Plan {
        trust: Trust::Current,
        ..base.clone()
    };
    assert_eq!(named(&explicit), current);
    let mut identities = vec![current.clone()];
    for variant in Trust::ALL {
        let plan = Plan {
            trust: variant,
            ..base.clone()
        };
        let written = plan_json(&plan);
        if variant == Trust::Current {
            assert!(
                written.get("trust").is_none(),
                "the default must not write the field that would rename every committed row"
            );
            continue;
        }
        assert_eq!(
            written.get("trust").and_then(Json::as_str),
            Some(variant.name()),
            "{variant:?} must name itself in the object the identity digests"
        );
        let id = named(&plan);
        assert_ne!(
            id, current,
            "{variant:?} is the same measurement as the default"
        );
        assert!(
            !identities.contains(&id),
            "{variant:?} shares an identity with another variant"
        );
        identities.push(id);
    }
    assert_eq!(identities.len(), 4, "four variants, four measurements");
    // The mask and the rule are two axes of one arm definition, so an arm that differs on both is a
    // third measurement and not a re-reading of either.
    let both = Plan {
        ablate: vec![Channel::Physical],
        trust: Trust::BothRules,
        ..base.clone()
    };
    let mask_only = Plan {
        ablate: vec![Channel::Physical],
        ..base.clone()
    };
    let rule_only = Plan {
        trust: Trust::BothRules,
        ..base.clone()
    };
    let ids = [named(&both), named(&mask_only), named(&rule_only)];
    for (at, id) in ids.iter().enumerate() {
        for other in ids.iter().skip(at + 1) {
            assert_ne!(
                id, other,
                "two arms that differ on different axes share a name"
            );
        }
    }
}

#[test]
fn a_variant_name_is_refused_and_an_absent_one_is_read_as_history() {
    let base = Plan {
        budgets: vec![320],
        strategies: vec![Strategy::Adaptive],
        seeds: vec![1],
        // Non-default, so the plan section really does write the field and the edits below have one to
        // corrupt.
        trust: Trust::MinChannelsTwo,
        ..Plan::default()
    };
    let fields = fields_of(&plan_json(&base));
    let read = |edited: Vec<(String, Json)>| Plan::from_json(&Json::Obj(edited));
    for (value, wanted) in [
        (Json::text("min-channels-3"), "min-channels-3"),
        (Json::text("Current"), "Current"),
        (Json::Null, "trust"),
        (Json::count(2), "trust"),
    ] {
        let edited: Vec<(String, Json)> = fields
            .iter()
            .map(|(name, v)| {
                (
                    name.clone(),
                    if name == "trust" {
                        value.clone()
                    } else {
                        v.clone()
                    },
                )
            })
            .collect();
        let error = read(edited).expect_err("an unusable trust field stops the run");
        assert!(
            error.contains(wanted),
            "the refusal must name {wanted}: {error}"
        );
        assert!(
            error.contains("current") || wanted == "trust",
            "a refusal about a variant names the vocabulary it expected: {error}"
        );
    }
    // Absent is the one readable case, read as `current` because a plan written before the variant
    // existed could not have meant anything else. Where a file *is* obliged to state a rule is a frozen
    // `aporia.protocol/2` arm, which the next test checks.
    let without: Vec<(String, Json)> = fields
        .iter()
        .filter(|(name, _)| name != "trust")
        .cloned()
        .collect();
    let back = read(without).expect("a plan with no variant field is a historical one");
    assert_eq!(back.trust, Trust::Current);
    // A near-miss name is refused as an unknown field, not ignored and defaulted.
    let mut with_typo = fields.clone();
    with_typo.push(("trusts".to_string(), Json::text("current")));
    assert!(
        read(with_typo).is_err(),
        "an unknown plan field is refused, not defaulted"
    );
}

#[test]
fn a_frozen_arm_of_the_new_schema_must_state_its_rule_and_the_old_one_may_not() {
    let protocol = geometry();
    // A schema-1 arm states a mask and nothing else, so a file that acquired a trust rule afterwards is
    // refused rather than honoured: promoting a pre-registration is an edit, and the edit has to move
    // the digest every later document cites.
    let stated_early = with_arm_field(
        &protocol,
        0,
        "trust",
        &Json::text(Trust::MinChannelsTwo.name()),
    );
    let error = Protocol::from_json(&stated_early)
        .expect_err("a schema-1 arm cannot state a trust variant");
    assert!(
        error.contains("trust") && error.contains(Protocol::SCHEMA_V2),
        "the refusal must name the axis and the schema that carries it: {error}"
    );
    // The committed file reads, and every arm it states is under the current rule.
    let read = Protocol::from_json(&protocol).expect("the frozen protocol still parses");
    assert_eq!(read.arms.len(), 11);
    assert!(
        read.arms.iter().all(|a| a.trust == Trust::Current),
        "a schema-1 file has no other rule to mean"
    );
    // Promote the same bytes to schema 2: an arm that names no rule is now not a definition of an arm.
    let promoted = as_schema_two(&protocol);
    let error =
        Protocol::from_json(&promoted).expect_err("a schema-2 arm that states no rule is refused");
    assert!(
        error.contains("trust") && error.contains("full"),
        "the refusal must name the field and the arm: {error}"
    );
    // With a rule stated on every arm the file reads, and each arm carries its rule into the plan
    // `arm()` returns — which is the whole mechanism: the protocol says it, the plan holds it, the
    // identity digests it. One arm alone is not enough, and the refusal names the next arm that still
    // states nothing, which is the proof that the obligation is per arm rather than per file.
    let stated = with_arm_field(
        &promoted,
        0,
        "trust",
        &Json::text(Trust::MinChannelsTwo.name()),
    );
    let error = Protocol::from_json(&stated).expect_err("one stated arm of eleven states nothing");
    assert!(
        error.contains("no-behavioral") && error.contains("trust"),
        "and it names the arm it is complaining about: {error}"
    );
    let all_stated = {
        let mut value = stated.clone();
        for at in 1..11 {
            value = with_arm_field(&value, at, "trust", &Json::text(Trust::Current.name()));
        }
        value
    };
    let protocol2 =
        Protocol::from_json(&all_stated).expect("a file whose arms all state a rule reads");
    assert_eq!(protocol2.arms[0].trust, Trust::MinChannelsTwo);
    for arm in protocol2.arms.iter().skip(1) {
        assert_eq!(
            arm.trust,
            Trust::Current,
            "{} was given the wrong rule by this edit",
            arm.name
        );
    }
    let name = protocol2.arms[0].name.clone();
    let (plan, frozen) = protocol2.arm(&name).expect("the stated arm selects");
    assert_eq!(plan.trust, Trust::MinChannelsTwo);
    assert_eq!(
        plan_json(&plan).get("trust").and_then(Json::as_str),
        Some(Trust::MinChannelsTwo.name()),
        "the arm's rule reaches the object the identity digests, and the arm names it too: {:?}",
        frozen.arm
    );
    // An unknown variant in an arm is refused with the vocabulary, not read as the familiar one.
    let bad = with_arm_field(&promoted, 0, "trust", &Json::text("min-channels-9"));
    let error = Protocol::from_json(&bad).expect_err("a misspelled variant is not a variant");
    assert!(
        error.contains("min-channels-9") && error.contains("current"),
        "the refusal names the word and the vocabulary: {error}"
    );
    // An arm that states the same mask and the same rule as another arm is one measurement under two
    // names, and this refusal is what makes the eight E1.3b arms a grid rather than a list of aliases.
    let twin = append_arm(
        &all_stated,
        Json::object(vec![
            ("name", Json::text("full-under-another-name")),
            ("ablate", Json::Arr(Vec::new())),
            ("trust", Json::text(Trust::MinChannelsTwo.name())),
        ]),
    );
    let error = Protocol::from_json(&twin)
        .expect_err("two arms with one mask and one rule are one experiment counted twice");
    assert!(error.contains("one measurement under two names"), "{error}");
    // The plan section is not a place a schema-2 file may state a rule either: it belongs to the arms,
    // beside the mask, or two parts of one file name two different measurements.
    let both = geometry_text().replacen(
        "\"plan\": {\n",
        "\"plan\": {\n    \"trust\": \"both-trust-rules\",\n",
        1,
    );
    assert_ne!(both, geometry_text(), "the edit found the plan section");
    let both = both.replacen(Protocol::SCHEMA, Protocol::SCHEMA_V2, 1);
    let error = Protocol::from_json(&Json::parse(&both).expect("still JSON"))
        .expect_err("a plan-level rule beside arm-level rules is two rules");
    assert!(error.contains("plan section"), "{error}");
}

#[test]
fn two_variants_cannot_be_given_the_same_archive_directory() {
    let entry = anchor();
    let base = Plan {
        archive_dir: PathBuf::from("archives"),
        ..Plan::default()
    };
    // The name a run before variants wrote, byte for byte: renaming it would make a committed archive
    // unreachable by the path a published number quotes.
    let before = archive_name(&entry, &base, Strategy::Adaptive, 1);
    assert_eq!(before, "analytic-sqrt_domain-adaptive-seed1");
    let mut seen = vec![before.clone()];
    for variant in Trust::ALL {
        let plan = Plan {
            trust: variant,
            ..base.clone()
        };
        let name = archive_name(&entry, &plan, Strategy::Adaptive, 1);
        if variant == Trust::Current {
            assert_eq!(
                name, before,
                "the default variant renames an existing namespace"
            );
        } else {
            assert_eq!(
                name,
                format!("{before}-trust-{}", variant.name()),
                "a variant archive must be findable by the rule that wrote it"
            );
            assert!(
                !seen.contains(&name),
                "{variant:?} shares an archive directory"
            );
        }
        seen.push(name);
    }
    // Both axes, one name: E2's arms and E1.3b's arms share a corpus root and an
    // entry-strategy-seed prefix, so the two suffixes have to compose rather than collide.
    let ablated = Plan {
        ablate: vec![Channel::Physical],
        trust: Trust::BothRules,
        ..base.clone()
    };
    assert_eq!(
        archive_name(&entry, &ablated, Strategy::Adaptive, 1),
        "analytic-sqrt_domain-adaptive-seed1-ablated-physical-trust-both-trust-rules"
    );
}

#[test]
fn a_variant_pairs_with_the_current_rule_and_not_with_another_variant() {
    let entry = anchor();
    let base = Plan {
        budgets: vec![80],
        strategies: vec![Strategy::Adaptive],
        seeds: vec![1],
        archive_dir: PathBuf::new(),
        ..Plan::default()
    };
    let arm = |plan: &Plan| {
        let sweeps = [sweep(&entry, plan, 1, Strategy::Adaptive).expect("the arm runs")];
        results_json(
            &sweeps,
            std::slice::from_ref(&entry),
            plan,
            &environment(),
            None,
        )
    };
    let full = arm(&base);
    let variant = arm(&Plan {
        trust: Trust::MinChannelsTwo,
        ..base.clone()
    });
    let other = arm(&Plan {
        trust: Trust::BothRules,
        ..base.clone()
    });
    // Every document names its rule at its top level, the reference included: opening a file should not
    // require knowing that an absent optional plan field is a statement about history.
    for (doc, want) in [
        (&full, Trust::Current),
        (&variant, Trust::MinChannelsTwo),
        (&other, Trust::BothRules),
    ] {
        assert_eq!(doc.get("trust").and_then(Json::as_str), Some(want.name()));
    }
    let full_arm = Arm::from_document("full.json", &full).expect("the reference reads");
    let variant_arm = Arm::from_document("variant.json", &variant).expect("the variant reads");
    let other_arm = Arm::from_document("other.json", &other).expect("the other variant reads");
    assert_eq!(full_arm.trust, Trust::Current);
    assert_eq!(variant_arm.trust, Trust::MinChannelsTwo);
    let comparison = compare(&full_arm, &variant_arm)
        .expect("one experiment with two trust rules is comparable");
    assert_eq!(comparison.trust, Trust::MinChannelsTwo);
    assert!(
        comparison.label.contains("min-channels-2"),
        "the label says which rule the arm ran under: {}",
        comparison.label
    );
    // The reference is the instrument as it has always been labelled. Pairing two variants and printing
    // the result against a document called `full` would be a delta between two experiments wearing the
    // reference arm's name.
    let error = compare(&variant_arm, &other_arm).expect_err("a variant is not a reference arm");
    assert!(error.contains("reference"), "{error}");
    assert!(error.contains("min-channels-2"), "{error}");
    // A document whose rule cannot be named is not an arm at all. The top-level name is removed first,
    // so the refusal is about the word and not about the two places disagreeing with each other — which
    // is the next assertion down.
    let broken = without_field(
        &with_plan_field(&variant, "trust", Json::text("min-channels-7")),
        "trust",
    );
    let error = Arm::from_document("broken.json", &broken)
        .expect_err("an unknown trust variant is refused, not read as the familiar one");
    assert!(
        error.contains("min-channels-7") && error.contains("current"),
        "the refusal names the word and the vocabulary: {error}"
    );
    // A document that states its rule twice, differently, is refused rather than resolved by whichever
    // field the reader happened to consult first.
    let contradictory = with_field(
        &variant,
        "plan",
        &plan_json(&Plan {
            trust: Trust::NonFiniteBlocksTrust,
            ..base.clone()
        }),
    );
    let error = Arm::from_document("two-rules.json", &contradictory)
        .expect_err("one document may not name two rules");
    assert!(error.contains("two rules"), "{error}");
    // Two documents that differ in a rate rather than in a declared axis are not one experiment.
    let moved = arm(&Plan {
        trust: Trust::MinChannelsTwo,
        calibrate_every: 11,
        ..base.clone()
    });
    let moved_arm = Arm::from_document("moved.json", &moved).expect("it reads");
    let error = compare(&full_arm, &moved_arm).expect_err("a moved rate is a different plan");
    assert!(error.contains("calibrate_every"), "{error}");
}

/// The writer's half of the proof: a run under a non-default variant puts the rule in the bytes, and
/// reading those bytes back gives the same measurement and the same identity — so it is not only the
/// reader that carries the variant, and not only the reader that can check it.
#[test]
fn a_variant_run_writes_the_rule_into_the_bytes_it_names_itself_with() {
    let entry = anchor();
    let path =
        std::env::temp_dir().join(format!("aporia-trust-variant-{}.json", std::process::id()));
    for variant in Trust::ALL {
        let plan = Plan {
            budgets: vec![80],
            strategies: vec![Strategy::Adaptive],
            seeds: vec![1],
            trust: variant,
            archive_dir: PathBuf::new(),
            ..Plan::default()
        };
        let sweeps = [sweep(&entry, &plan, 1, Strategy::Adaptive).expect("the arm runs")];
        let doc = results_json(
            &sweeps,
            std::slice::from_ref(&entry),
            &plan,
            &environment(),
            None,
        );
        std::fs::write(&path, doc.to_pretty()).expect("the document is written");
        let text = std::fs::read_to_string(&path).expect("and read back");
        let from_file = Json::parse(&text).expect("and parsed");
        let read = Plan::from_json(from_file.get("plan").expect("a plan section"))
            .expect("the written plan section reads back");
        assert_eq!(read.trust, variant, "{variant:?} did not survive the file");
        // The file's recorded identity is a digest of the plan and entries the file itself carries, so
        // the variant is inside the name rather than only in a prose field beside it.
        let recomputed = identity_of(
            RESULTS_SCHEMA,
            from_file.get("plan").expect("a plan section"),
            from_file.get("entries").expect("an entries section"),
        );
        assert_eq!(
            from_file.get("identity").and_then(Json::as_str),
            Some(recomputed.as_str()),
            "{variant:?} was written under an identity its own sections do not imply"
        );
        // And the sweep rows are not in that name: the same plan over the same corpus is one
        // measurement, which is why a rerun is refused as a duplicate rather than written twice.
        let in_memory = results_json(
            &[],
            std::slice::from_ref(&entry),
            &plan,
            &environment(),
            None,
        );
        assert_eq!(
            recomputed,
            identity_of(
                RESULTS_SCHEMA,
                in_memory.get("plan").expect("a plan section"),
                in_memory.get("entries").expect("an entries section"),
            ),
            "{variant:?} names itself differently in memory than on disk"
        );
        // And the archive directory the run would have written under is named for the rule too, so the
        // bytes behind a row cannot be shared between two variants.
        let archive = archive_name(&entry, &plan, Strategy::Adaptive, 1);
        if variant == Trust::Current {
            assert!(
                !archive.contains("trust-"),
                "the default renames a namespace"
            );
        } else {
            assert!(
                archive.ends_with(&format!("-trust-{}", variant.name())),
                "{archive}"
            );
        }
    }
    let _ = std::fs::remove_file(&path);
}

/// A measured row counts the readings its trust rule would act on, taken from the atlas that produced
/// the row rather than from a constant a caller remembered to pass.
#[test]
fn a_row_counts_the_readings_its_trust_rule_acts_on() {
    // E1.3b's third secondary metric is the number of non-real readings per row. It exists because
    // `non-finite-blocks-trust` can only have moved a label in a leaf that holds one, and because
    // `aporia_evidence::fuse` clamps its score into `[0, 1]` — so whether the rule has anything to work
    // on is a measurement this instrument does not get to assume either way. A row that disagreed with
    // its own partition would make the variant unattributable, which is what this checks.
    let entry = anchor();
    let model = entry
        .model
        .as_ref()
        .expect("the anchor entry compiles to a model");
    let plan = Plan {
        trust: Trust::NonFiniteBlocksTrust,
        budgets: vec![320],
        ..Plan::default()
    };
    let mut campaign = aporia_search::run(model, plan.config(Strategy::Adaptive, 320, 1));
    let row =
        aporia_bench::metrics::Outcome::measure(&entry, &campaign, 0, Strategy::Adaptive, 320);
    assert_eq!(
        row.non_finite_readings,
        campaign.atlas.non_finite_readings(),
        "the row reports a number that is not its own atlas's"
    );
    let json = row.to_json();
    assert_eq!(
        json.get("non_finite_readings").and_then(Json::as_u64),
        Some(row.non_finite_readings),
        "the analysis reads the count out of the row, so the row has to carry it"
    );
    // Poison one reading in one leaf and the next row says one more. This is the half that a field
    // hard-wired to zero would pass, which is why the count is taken from the partition instead.
    let leaf = campaign
        .atlas
        .leaf_ids()
        .iter()
        .copied()
        .find(|id| {
            campaign
                .atlas
                .cell(*id)
                .is_some_and(|c| !c.points.is_empty())
        })
        .expect("a finished campaign leaves evidence in some leaf");
    campaign.atlas.cells[leaf as usize].points[0].risk = f64::NEG_INFINITY;
    let after =
        aporia_bench::metrics::Outcome::measure(&entry, &campaign, 0, Strategy::Adaptive, 320);
    assert_eq!(
        after.non_finite_readings,
        row.non_finite_readings + 1,
        "one reading left the real numbers and the row did not notice"
    );
}

/// Two experiments' arms cannot be written into one archive directory.
#[test]
fn a_run_refuses_to_archive_over_another_experiments_measurement() {
    // `archive_name` encodes the entry, the strategy, the seed, the mask and the rule — the axes a plan
    // moves — and not the protocol. E1.3b's `full-current-trust` arm and E1.x's committed `full` arm
    // therefore ask for the same directory at `edge/pole_at_edge` seed 1, at different largest budgets,
    // and the runner that got there second would replace the bytes the first one's published number is
    // opened from. The replacement is invisible to every other check in the repository, because an
    // archive digests its own artefacts and replays its own records: so this is the one place it can be
    // caught, and it is caught before a single campaign runs.
    let entries = corpus::load(&aporia_bench::corpus_root()).expect("the corpus loads");
    let path = aporia_bench::corpus_root()
        .join("protocols")
        .join("e1-3b-trust-resolution.json");
    let protocol = Protocol::load(&path).expect("the promoted freeze reads");
    let (mut plan, _frozen) = protocol
        .arm("full-current-trust")
        .expect("the reference arm selects");
    let selected: Vec<corpus::Entry> = entries
        .iter()
        .filter(|e| protocol.entries.contains(&e.id()))
        .cloned()
        .collect();
    let dir = aporia_bench::corpus_root().join("../.scratch/archive-guard");
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).expect("the temporary namespace exists");
    plan.archive_dir = dir.clone();
    let strategy = plan.strategies[0];
    let budget = *plan.budgets.last().expect("the freeze states a ladder");
    let seed = plan.seeds[0];

    // Nothing written yet, so nothing is being replaced and the run may go ahead.
    cli::archive_namespace_guard(&plan, &selected).expect("an empty namespace cannot collide");

    let entry = selected
        .iter()
        .find(|e| e.id() == "edge/pole_at_edge")
        .expect("the control entry is in the freeze's corpus");
    let name = archive_name(entry, &plan, strategy, seed);
    let archive = dir.join(&name);
    std::fs::create_dir_all(&archive).expect("the archive directory is created");
    let manifest = archive.join("manifest.json");
    let mine = plan.config(strategy, budget, seed).json();
    // E1.x's archive of the same entry, seed, strategy, mask and rule: one configuration, whose largest
    // budget is the ladder it ran, which is not this run's.
    let written = with_field(&mine, "budget", &Json::count(1280));
    std::fs::write(
        &manifest,
        Json::object(vec![("config", written)]).to_compact(),
    )
    .expect("the manifest is written");
    let error = cli::archive_namespace_guard(&plan, &selected)
        .expect_err("a differently-configured archive in this run's path must stop it");
    assert!(
        error.contains(&name) && error.contains("config.budget"),
        "the refusal must name the directory and the field that differs: {error}"
    );
    assert!(
        error.contains("--archive"),
        "and say how to resolve it, rather than leaving the operator to guess: {error}"
    );

    // The same configuration, written by the same plan a minute earlier, is a rerun and not a collision.
    std::fs::write(
        &manifest,
        Json::object(vec![("config", mine.clone())]).to_compact(),
    )
    .expect("the manifest is rewritten");
    cli::archive_namespace_guard(&plan, &selected)
        .expect("a rerun of the same measurement is allowed");

    // And a manifest written before this build started recording the trust rule under `atlas` states
    // less, not differently: the history rule that reads an omitted field as the one that was in force
    // has to apply here too, or the guard would refuse to re-run the very archives it protects.
    let older = without_atlas_fields(
        &mine,
        &["trust", "min_channels_required", "blocks_non_finite"],
    );
    std::fs::write(
        &manifest,
        Json::object(vec![("config", older)]).to_compact(),
    )
    .expect("the older manifest is written");
    cli::archive_namespace_guard(&plan, &selected)
        .expect("a manifest that states less is not one that states something else");
    std::fs::remove_dir_all(&dir).ok();
}

/// One field of the recorded atlas block dropped, which is what an older manifest looks like.
fn without_atlas_fields(value: &Json, drop: &[&str]) -> Json {
    let Json::Obj(fields) = value else {
        return value.clone();
    };
    Json::Obj(
        fields
            .iter()
            .map(|(key, held)| {
                if key != "atlas" {
                    return (key.clone(), held.clone());
                }
                let Json::Obj(atlas) = held else {
                    return (key.clone(), held.clone());
                };
                (
                    key.clone(),
                    Json::Obj(
                        atlas
                            .iter()
                            .filter(|(name, _)| !drop.contains(&name.as_str()))
                            .cloned()
                            .collect(),
                    ),
                )
            })
            .collect(),
    )
}
