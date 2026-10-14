//! A frozen protocol is only a pre-registration if the runner reads it.

use aporia_bench::cli;
use aporia_bench::corpus_root;
use aporia_bench::harness::{Plan, Protocol, RESULTS_SCHEMA, identity_of, plan_json, results_json};
use aporia_boundary::Trust;
use aporia_evidence::Channel;
use aporia_search::Strategy;
use aporia_store::{Environment, Json};
use std::path::PathBuf;

fn protocol_path() -> PathBuf {
    corpus_root().join("protocols").join("e1-geometry.json")
}

fn protocol_text() -> String {
    std::fs::read_to_string(protocol_path()).expect("the frozen protocol is in the tree")
}

fn environment() -> Environment {
    let mut environment = Environment::current();
    environment.notes.clear();
    environment
}

/// A plan that is nothing like the defaults, so a reader that quietly fell back to a default would
/// show up as a difference rather than passing. The trust variant is unusual here for the same reason
/// as every other field, and `a_plan_reads_back_the_bytes_that_name_it` therefore round-trips it: a
/// plan section that lost its rule would otherwise be read as the current one.
fn unusual() -> Plan {
    Plan {
        budgets: vec![320, 640, 1280],
        strategies: vec![Strategy::Adaptive],
        seeds: vec![3, 1, 5, 2, 4],
        grid: 41,
        minimise_budget: 4_000,
        probe_every: 7,
        calibrate_every: 25,
        refine_every: 40,
        numerical_every: 11,
        symmetric_every: 1,
        differential_every: 11,
        ablate: vec![Channel::Physical, Channel::Behavioral],
        trust: Trust::NonFiniteBlocksTrust,
        archive_dir: PathBuf::from("where-bytes-land-is-not-part-of-a-measurement"),
    }
}

#[test]
fn a_plan_reads_back_the_bytes_that_name_it() {
    // `plan_json` is what the measurement identity digests, so "this file describes that measurement"
    // can only mean "it digests the same way".
    let plan = unusual();
    let doc = results_json(&[], &[], &plan, &environment(), None);
    let back = Plan::from_json(doc.get("plan").expect("a document carries its plan"))
        .expect("the plan section reads back");
    assert_eq!(back.budgets, plan.budgets);
    assert_eq!(back.strategies, plan.strategies);
    assert_eq!(back.seeds, plan.seeds);
    assert_eq!(back.grid, plan.grid);
    assert_eq!(back.minimise_budget, plan.minimise_budget);
    for (name, a, b) in [
        ("probe_every", back.probe_every, plan.probe_every),
        (
            "calibrate_every",
            back.calibrate_every,
            plan.calibrate_every,
        ),
        ("refine_every", back.refine_every, plan.refine_every),
        (
            "numerical_every",
            back.numerical_every,
            plan.numerical_every,
        ),
        (
            "symmetric_every",
            back.symmetric_every,
            plan.symmetric_every,
        ),
        (
            "differential_every",
            back.differential_every,
            plan.differential_every,
        ),
    ] {
        assert_eq!(a, b, "{name} did not survive the round trip");
    }
    assert_eq!(back.ablate, plan.ablate);
    assert_eq!(
        back.trust, plan.trust,
        "a plan section that lost its trust rule would be read as the current one"
    );
    assert!(
        back.archive_dir.as_os_str().is_empty(),
        "where a run writes is not part of what it measures, so it is not in the document"
    );
    assert_eq!(
        identity_of(RESULTS_SCHEMA, doc.get("plan").unwrap(), &Json::Arr(vec![])),
        doc.get("identity")
            .and_then(Json::as_str)
            .expect("a document records its identity"),
        "the identity is a digest of exactly the plan section a protocol file states"
    );
}

#[test]
fn a_missing_or_unrecognised_plan_field_is_refused_not_defaulted() {
    let doc = results_json(&[], &[], &unusual(), &environment(), None);
    let Json::Obj(fields) = doc.get("plan").unwrap() else {
        panic!("the plan section is an object");
    };
    for drop in [
        "budgets",
        "seeds",
        "grid_per_axis",
        "calibrate_every",
        "ablate",
    ] {
        let kept: Vec<(String, Json)> = fields
            .iter()
            .filter(|(name, _)| name != drop)
            .cloned()
            .collect();
        let error = Plan::from_json(&Json::Obj(kept))
            .expect_err("a plan missing a field cannot be measured");
        assert!(
            error.contains(drop),
            "the refusal must name {drop}: {error}"
        );
    }
    // A typo is refused too: a reader that ignored it would quietly measure the default instead, and
    // a frozen protocol whose `"budget"` was read as nothing is a different experiment.
    let mut with_typo = fields.clone();
    with_typo.push(("budget".to_string(), Json::Arr(vec![Json::count(64)])));
    let error = Plan::from_json(&Json::Obj(with_typo)).expect_err("an unknown field is refused");
    assert!(error.contains("budget"), "{error}");
    // And a zero budget or a grid that cannot cover the domain edges is refused by meaning, not name.
    for (key, value, wanted) in [
        ("budgets", Json::Arr(vec![Json::count(0)]), "budget"),
        ("budgets", Json::Arr(vec![]), "budgets"),
        ("seeds", Json::Arr(vec![]), "seeds"),
        ("grid_per_axis", Json::count(2), "grid_per_axis"),
    ] {
        let edited: Vec<(String, Json)> = fields
            .iter()
            .map(|(name, v)| {
                (
                    name.clone(),
                    if name == key {
                        value.clone()
                    } else {
                        v.clone()
                    },
                )
            })
            .collect();
        let error = Plan::from_json(&Json::Obj(edited))
            .expect_err("{key} edited to an unusable value must stop the run");
        assert!(error.contains(wanted), "{error}");
    }
}

#[test]
fn the_committed_protocol_is_a_protocol_the_runner_can_read() {
    let protocol = Protocol::load(&protocol_path()).expect("the frozen protocol parses");
    assert_eq!(protocol.id, "e1-geometry");
    assert!(!protocol.question.is_empty());
    assert_eq!(protocol.plan.strategies, vec![Strategy::Adaptive]);
    assert_eq!(protocol.plan.budgets, vec![320, 640, 1280]);
    assert_eq!(protocol.plan.seeds, vec![1, 2, 3, 4, 5]);
    assert_eq!(protocol.plan.grid, 41);
    assert_eq!(protocol.plan.probe_every, 7);
    assert_eq!(protocol.plan.numerical_every, 11);
    assert_eq!(protocol.plan.differential_every, 11);
    assert_eq!(protocol.plan.symmetric_every, 1);
    assert_eq!(protocol.plan.refine_every, 40);
    assert_eq!(protocol.plan.calibrate_every, 25);
    assert!(
        protocol.plan.ablate.is_empty(),
        "the plan section states no mask; each arm carries its own"
    );
    for name in protocol.arm_names() {
        let (plan, frozen) = protocol.arm(name).expect("an arm names itself");
        assert_eq!(frozen.id, protocol.id);
        assert_eq!(frozen.arm, name);
        assert_eq!(frozen.question, protocol.question);
        assert_eq!(
            plan.budgets, protocol.plan.budgets,
            "{name} moved the ladder"
        );
        assert_eq!(plan.grid, protocol.plan.grid, "{name} moved the grid");
    }
    // The arm vocabulary is E2's, so every arm here has an E2 counterpart to be read against: the
    // full instrument, one removal per channel, and one channel kept.
    assert_eq!(
        protocol.arms.len(),
        1 + Channel::ALL.len() * 2,
        "E2's eleven arms are the comparison this experiment claims to reproduce"
    );
    assert!(
        protocol
            .arms
            .iter()
            .any(|a| a.ablate.contains(&Channel::Physical) && a.ablate.len() == 1)
    );
    assert!(protocol.arms.iter().any(
        |a| a.ablate.len() == Channel::ALL.len() - 1 && !a.ablate.contains(&Channel::Physical)
    ));
}

#[test]
fn the_frozen_ladder_is_e2s_ladder_exactly() {
    // The protocol's central claim is that nothing but the corpus shape changes. That is checkable
    // only against a measurement, so it is checked against E2's committed full arm, read by the same
    // reader this run uses.
    let e2_path = aporia_bench::results_dir().join("results-a5143462383c.json");
    let text =
        std::fs::read_to_string(&e2_path).unwrap_or_else(|e| panic!("{}: {e}", e2_path.display()));
    let doc = Json::parse(&text).expect("E2's full arm is a results document");
    let e2 = Plan::from_json(doc.get("plan").expect("and carries its plan"))
        .expect("E2's recorded plan reads back");
    let protocol = Protocol::load(&protocol_path()).expect("the frozen protocol parses");
    let (full, _frozen) = protocol.arm("full").expect("the full arm");
    assert_eq!(full.budgets, e2.budgets, "the budget ladder moved");
    assert_eq!(full.seeds, e2.seeds, "the seed list moved");
    assert_eq!(full.strategies, e2.strategies, "the strategy moved");
    assert_eq!(full.grid, e2.grid, "the verification grid moved");
    assert_eq!(full.minimise_budget, e2.minimise_budget);
    for (name, a, b) in [
        ("probe_every", full.probe_every, e2.probe_every),
        ("numerical_every", full.numerical_every, e2.numerical_every),
        (
            "differential_every",
            full.differential_every,
            e2.differential_every,
        ),
        ("symmetric_every", full.symmetric_every, e2.symmetric_every),
        ("refine_every", full.refine_every, e2.refine_every),
        ("calibrate_every", full.calibrate_every, e2.calibrate_every),
    ] {
        assert_eq!(a, b, "{name} differs from the ladder E2 measured on");
    }
}

#[test]
fn a_file_that_states_no_deciding_metric_is_not_an_experiment() {
    let Json::Obj(fields) = Json::parse(&protocol_text()).unwrap() else {
        panic!("the protocol is an object");
    };
    let without_metric: Vec<(String, Json)> = fields
        .iter()
        .filter(|(name, _)| name != "primary_metric")
        .cloned()
        .collect();
    let error =
        Protocol::from_json(&Json::Obj(without_metric)).expect_err("a protocol needs a decider");
    assert!(error.contains("primary_metric"), "{error}");

    let metric = fields
        .iter()
        .find(|(name, _)| name == "primary_metric")
        .expect("the file has one")
        .1
        .clone();
    for key in ["name", "universe", "definition", "decision_rule"] {
        let Json::Obj(mut parts) = metric.clone() else {
            unreachable!()
        };
        parts.retain(|(name, _)| name != key);
        let edited: Vec<(String, Json)> = fields
            .iter()
            .map(|(name, value)| {
                (
                    name.clone(),
                    if name == "primary_metric" {
                        Json::Obj(parts.clone())
                    } else {
                        value.clone()
                    },
                )
            })
            .collect();
        let error = Protocol::from_json(&Json::Obj(edited))
            .expect_err("a metric missing its {key} decides nothing");
        assert!(error.contains(key), "the refusal must name {key}: {error}");
    }
}

#[test]
fn an_arm_is_never_guessed_and_a_plan_section_never_carries_a_mask() {
    let protocol = Protocol::load(&protocol_path()).expect("the frozen protocol parses");
    let error = protocol
        .arm("no-such-arm")
        .expect_err("a protocol with more than one arm cannot answer for an unnamed one");
    assert!(error.contains("no-such-arm"), "{error}");
    assert!(
        protocol.arm_names().iter().all(|name| error.contains(name)),
        "the refusal lists the arms that do exist: {error}"
    );

    let Json::Obj(fields) = Json::parse(&protocol_text()).unwrap() else {
        unreachable!()
    };
    let Json::Obj(plan_fields) = &fields
        .iter()
        .find(|(name, _)| name == "plan")
        .expect("a plan section")
        .1
    else {
        unreachable!()
    };
    let mut with_mask: Vec<(String, Json)> = plan_fields.clone();
    with_mask.push((
        "ablate".to_string(),
        Json::Arr(vec![Json::text("physical")]),
    ));
    let edited: Vec<(String, Json)> = fields
        .iter()
        .map(|(name, value)| {
            (
                name.clone(),
                if name == "plan" {
                    Json::Obj(with_mask.clone())
                } else {
                    value.clone()
                },
            )
        })
        .collect();
    let error =
        Protocol::from_json(&Json::Obj(edited)).expect_err("one file must not state two masks");
    assert!(error.contains("ablate"), "{error}");
}

#[test]
fn run_refuses_to_override_a_frozen_definition() {
    let path = protocol_path().display().to_string();
    for (extra, named) in [
        (vec!["--budgets", "64"], "--budgets"),
        (vec!["--seeds", "9"], "--seeds"),
        (vec!["--grid", "13"], "--grid"),
        (vec!["--only", "analytic/sqrt_domain"], "--only"),
        (vec!["--ablate", "physical"], "--ablate"),
        (vec!["--differential-every", "0"], "--differential-every"),
    ] {
        let mut flags = vec![
            "--plan".to_string(),
            path.clone(),
            "--arm".to_string(),
            "full".to_string(),
        ];
        flags.extend(extra.iter().map(|t| (*t).to_string()));
        let error = cli::run_run("aporia-bench", &flags)
            .expect_err("an override of a frozen plan is refused, not applied");
        assert!(error.contains(named), "{error}");
        assert!(error.contains("--plan"), "{error}");
    }
    // An arm name is required: a protocol with eleven arms does not say which measurement was wanted.
    let error = cli::run_run("aporia-bench", &["--plan".into(), path.clone()])
        .expect_err("the arm must be named");
    assert!(error.contains("--arm"), "{error}");
    // `--arm` alone selects an arm of nothing.
    let error = cli::run_run("aporia-bench", &["--arm".into(), "full".into()])
        .expect_err("--arm needs a plan file to select from");
    assert!(error.contains("--plan"), "{error}");
}

#[test]
fn a_protocol_run_says_which_protocol_and_arm_wrote_it() {
    let protocol = Protocol::load(&protocol_path()).expect("the frozen protocol parses");
    let (plan, frozen) = protocol
        .arm("only-physical")
        .expect("the only-physical arm exists");
    let entries = aporia_bench::corpus::load(&corpus_root()).expect("the corpus loads");
    let named: Vec<_> = entries
        .iter()
        .filter(|e| e.id() == "analytic/sqrt_domain")
        .cloned()
        .collect();
    assert_eq!(named.len(), 1, "the anchor this test measures against");
    let doc = results_json(&[], &named, &plan, &environment(), Some(&frozen));
    let record = doc
        .get("protocol")
        .expect("the document names its protocol");
    assert_eq!(record.get("id").and_then(Json::as_str), Some("e1-geometry"));
    assert_eq!(
        record.get("arm").and_then(Json::as_str),
        Some("only-physical")
    );
    assert_eq!(
        doc.get("question").and_then(Json::as_str),
        Some(protocol.question.as_str()),
        "a protocol run quotes the question frozen before its corpus existed"
    );
    // Provenance, not identity: the same plan over the same corpus measures the same experiment
    // whether or not the command that produced it cited a protocol file.
    let bare = results_json(&[], &named, &plan, &environment(), None);
    assert_eq!(
        bare.get("identity").and_then(Json::as_str),
        doc.get("identity").and_then(Json::as_str),
        "naming a protocol file must not rename the measurement"
    );
    assert_ne!(
        bare.get("question").and_then(Json::as_str),
        doc.get("question").and_then(Json::as_str),
        "without the protocol the question is inferred from the plan shape, which is a different claim"
    );
    // ... and it pins the bytes that claim was read from. `plan` and `entries` are inside the
    // identity; the metric, the decision rule and the wording tiers are not, so without this a
    // pre-registration edited after the run would leave no trace in the file that cites it.
    let file = protocol_text();
    let digest = aporia_store::digest::sha256_hex(file.as_bytes());
    let expected = &digest[..16];
    assert_eq!(
        record.get("digest").and_then(Json::as_str),
        Some(expected),
        "the document digests the protocol file it was read from"
    );
    assert_eq!(
        bare.get("protocol"),
        Some(&Json::Null),
        "a command-line run cites no protocol, so it digests none"
    );
}

#[test]
fn an_edited_protocol_reads_as_a_different_document_than_the_one_it_was_frozen_in() {
    let pristine = Protocol::load(&protocol_path()).expect("the frozen protocol parses");
    let dir = std::env::temp_dir().join(format!("aporia-protocol-digest-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a temporary directory");
    let path = dir.join("e1-geometry.json");
    // One string of the deciding metric's wording: the plan section is untouched, so the measurement
    // identity does not move and only the digest can show the file differs from the one committed.
    let text = protocol_text().replacen("localised_at", "localised_XY", 1);
    assert_ne!(
        text,
        protocol_text(),
        "the edit found the metric text it meant to edit"
    );
    std::fs::write(&path, text).expect("the edited copy is written");
    let changed = Protocol::load(&path).expect("the edited copy is still a protocol");
    std::fs::remove_dir_all(&dir).expect("the temporary directory is removed");

    assert_ne!(
        pristine.digest, changed.digest,
        "two files that state different deciding text must not share a digest"
    );
    let file = protocol_text();
    let digest = aporia_store::digest::sha256_hex(file.as_bytes());
    assert_eq!(
        pristine.digest.as_deref(),
        Some(&digest[..16]),
        "the digest is of the file's bytes, not of a re-rendered form of them"
    );
    let identity = |plan: &Plan| {
        results_json(&[], &[], plan, &environment(), None)
            .get("identity")
            .and_then(Json::as_str)
            .unwrap_or_default()
            .to_string()
    };
    assert_eq!(
        identity(&pristine.plan),
        identity(&changed.plan),
        "the difference is invisible to the identity, which is exactly why the digest exists"
    );
    // A protocol assembled from a value in memory has no bytes, and says so instead of inventing one.
    let in_memory = Protocol::from_json(&Json::parse(&protocol_text()).expect("the file is JSON"))
        .expect("the committed protocol reads");
    assert_eq!(in_memory.digest, None);
}
#[test]
fn a_frozen_follow_up_that_cannot_be_read_cannot_be_run_by_accident() {
    // `e1-3b-trust-resolution.json` is a pre-registration written before its corpus and before the
    // instrument changes it depends on. The protection has to be structural rather than a note in
    // prose: an unreadable schema cannot be measured against, so the file cannot quietly become a run.
    let path = corpus_root()
        .join("protocols")
        .join("e1-3b-trust-resolution.json");
    let text = std::fs::read_to_string(&path).expect("the follow-up protocol is in the tree");
    let value = Json::parse(&text).expect("it is JSON");
    let error =
        Protocol::from_json(&value).expect_err("a draft schema is not a protocol the runner reads");
    assert!(
        error.contains("aporia.protocol/draft-1"),
        "the refusal names the schema rather than a detail: {error}"
    );

    // A freeze that cannot be promoted is a note rather than a pre-registration, so the file is checked
    // as the definition it claims to be. Two things were missing from it as frozen: a schema whose arms
    // state a trust rule, and a corpus. The first is the instrument's job and it is now real, so the
    // promotion performed here in memory — schema `/2`, each arm given the rule its own name already
    // claims — leaves exactly one thing for the reader to refuse: the entries this experiment has not
    // authored yet.
    let promoted = promote(&Json::parse(&text).expect("it is JSON"));
    let error = Protocol::from_json(&promoted)
        .err()
        .unwrap_or_else(|| panic!("the freeze parsed with its entry list still empty"));
    assert!(
        error.contains("entries.run"),
        "the refusal must be about the corpus it has not authored, got: {error}"
    );
    // Hand it three existing entries and nothing else changes: it reads as a protocol, with its frozen
    // plan, its eight arms and its decider intact. That is the difference between a definition waiting
    // for a corpus and a document that was never machine-checkable in the first place.
    let filled = with_run(&promoted, &FROZEN_SAMPLE);
    let protocol = Protocol::from_json(&filled)
        .expect("the freeze is a readable protocol once its entries exist");
    assert_eq!(protocol.arm_names().len(), 8);
    assert_eq!(protocol.plan.budgets, vec![320, 640, 1280, 2560]);
    assert_eq!(protocol.plan.grid, 41);
    assert_eq!(protocol.plan.differential_every, 11);
    assert!(
        protocol.plan.ablate.is_empty(),
        "the plan section states no mask: each arm names its own, or the file is two experiments"
    );
    assert_eq!(
        protocol.plan.trust,
        Trust::Current,
        "and it states no rule either, for the same reason"
    );
    assert!(
        Protocol::load(&path).is_err(),
        "and as committed, the runner cannot read the draft at all"
    );
    // Committing the file with an empty list is the discipline; a stray edit that fills it silently
    // would turn the freeze into a runnable plan without a review.
    assert!(
        text.contains("\"run\": []"),
        "the committed freeze must still name no entry"
    );

    // What the freeze commits to, checked as fields so it cannot be quietly rewritten later.
    let field = |key: &str| -> Json {
        filled
            .get(key)
            .unwrap_or_else(|| panic!("the freeze states no {key}"))
            .clone()
    };
    assert_eq!(field("frozen").as_str(), Some("2026-10-09"));
    assert!(
        field("supersedes_nothing")
            .as_str()
            .unwrap_or_default()
            .contains("stays failed"),
        "a follow-up must not reopen the condition it follows"
    );
    let rule = field("primary_metric")
        .get("decision_rule")
        .and_then(Json::as_array)
        .expect("the freeze states a decision rule")
        .to_vec();
    assert_eq!(
        rule.len(),
        5,
        "five outcomes, including the one where the corpus cannot ask"
    );
    assert!(
        rule[0]
            .as_str()
            .unwrap_or_default()
            .contains("stop and report"),
        "the first outcome must be the null one, stated before the favourable ones"
    );
    let prerequisites = field("prerequisite_instrument_changes")
        .as_array()
        .expect("the freeze names what must be built first")
        .to_vec();
    assert!(prerequisites.len() >= 3);
    assert!(
        prerequisites.iter().any(|p| {
            p.as_str()
                .unwrap_or_default()
                .contains("declared region into the labelling path")
        }),
        "and one of them must be the prohibition that keeps the answer key out of the rule"
    );
}

/// Which trust rule each of the E1.3b freeze's eight arm names claims.
///
/// The reader deliberately does not parse an arm's label — a name is prose, a field is a measurement —
/// so this table is the author's promise rather than the instrument's check. It is a test because the
/// freeze makes the promotion depend on it: the commit that fills `entries.run` has to write these exact
/// values into `"trust"`, and an arm whose stated rule contradicts the name a report prints for it is a
/// frozen experiment nobody can read.
const FROZEN_RULES: [(&str, Trust); 8] = [
    ("full-current-trust", Trust::Current),
    ("full-min-channels-2", Trust::MinChannelsTwo),
    ("full-nonfinite-blocks-trust", Trust::NonFiniteBlocksTrust),
    ("full-both-trust-rules", Trust::BothRules),
    ("no-physical-current-trust", Trust::Current),
    ("no-physical-both-trust-rules", Trust::BothRules),
    ("only-physical-current-trust", Trust::Current),
    ("only-physical-both-trust-rules", Trust::BothRules),
];

/// Three entries that exist today, for the tests that need a non-empty `entries.run` to get as far as
/// the reader's other refusals. These are not the experiment's corpus: the freeze's own entry list stays
/// empty until the resolvable entries are authored.
const FROZEN_SAMPLE: [&str; 3] = [
    "edge/overflow_tail",
    "edge/pole_at_edge",
    "geometry/narrow_oblique",
];

/// One field of an object replaced, or appended when the object does not carry it.
fn edited(value: &Json, key: &str, make: impl Fn(&Json) -> Json) -> Json {
    let Json::Obj(fields) = value else {
        return value.clone();
    };
    let mut out = fields.clone();
    match out.iter_mut().find(|(name, _)| name == key) {
        Some((_, held)) => {
            let replacement = make(held);
            *held = replacement;
        }
        None => out.push((key.to_string(), make(&Json::Null))),
    }
    Json::Obj(out)
}

/// The freeze promoted the way its promotion commit must promote it: the schema whose arms state a
/// rule, and each arm given the rule its own name already claims.
fn promote(value: &Json) -> Json {
    let with_schema = edited(value, "schema", |_| Json::text(Protocol::SCHEMA_V2));
    edited(&with_schema, "arms", |arms| {
        let Json::Arr(list) = arms else {
            unreachable!("the freeze states its arms as a list");
        };
        Json::Arr(
            list.iter()
                .map(|arm| {
                    let name = arm
                        .get("name")
                        .and_then(Json::as_str)
                        .expect("every frozen arm names itself");
                    let (_, rule) = FROZEN_RULES
                        .iter()
                        .find(|(arm_name, _)| *arm_name == name)
                        .unwrap_or_else(|| panic!("{name:?} is not one of the freeze's arms"));
                    edited(arm, "trust", |_| Json::text(rule.name()))
                })
                .collect(),
        )
    })
}

/// The same protocol with `entries.run` replaced and the rest of its `entries` section untouched.
fn with_run(value: &Json, names: &[&str]) -> Json {
    edited(value, "entries", |entries| {
        edited(entries, "run", |_| {
            Json::Arr(names.iter().map(|n| Json::text(*n)).collect())
        })
    })
}

/// How many distinct values a list of names holds, without pulling in a set for one assertion.
fn distinct(values: &[String]) -> usize {
    let mut seen: Vec<&String> = Vec::new();
    for value in values {
        if !seen.contains(&value) {
            seen.push(value);
        }
    }
    seen.len()
}

/// The freeze's own arms, as the committed file states them.
fn follow_up() -> Json {
    let path = corpus_root()
        .join("protocols")
        .join("e1-3b-trust-resolution.json");
    Json::parse(&std::fs::read_to_string(path).expect("the follow-up protocol is in the tree"))
        .expect("it is JSON")
}

/// The eight arms are four trust variants across three channel masks, and the masks alone do not
/// separate them: eight names, three measurements, and no field anywhere that says which rule labelled a
/// row. This is what the freeze's first prerequisite asks the instrument to fix.
#[test]
fn the_frozen_arms_are_eight_names_over_three_masks() {
    let arms = follow_up()
        .get("arms")
        .and_then(Json::as_array)
        .expect("arms")
        .to_vec();
    assert_eq!(
        arms.len(),
        FROZEN_RULES.len(),
        "four trust variants across three channel masks"
    );
    assert!(
        arms.iter().any(|a| {
            a.get("name").and_then(Json::as_str) == Some("full-current-trust")
                && a.get("ablate")
                    .and_then(Json::as_array)
                    .is_some_and(<[Json]>::is_empty)
        }),
        "the default arm must be today's policy with an empty mask, because it is the one the \
         reproduction gate compares against committed rows"
    );
    let masks: Vec<String> = arms
        .iter()
        .map(|a| {
            a.get("ablate")
                .and_then(Json::as_array)
                .map(|m| {
                    m.iter()
                        .filter_map(Json::as_str)
                        .collect::<Vec<_>>()
                        .join("+")
                })
                .unwrap_or_default()
        })
        .collect();
    // Three masks across eight arms. The trust dimension is the other four, and it is the dimension the
    // experiment is about, so it has to be a field and not a word inside a label.
    assert_eq!(distinct(&masks), 3, "eight arms over three channel masks");
    let names: Vec<String> = arms
        .iter()
        .map(|a| {
            a.get("name")
                .and_then(Json::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert_eq!(
        names,
        FROZEN_RULES
            .iter()
            .map(|(name, _)| (*name).to_string())
            .collect::<Vec<_>>(),
        "the table has to be read in the file's own order, or it checks nothing"
    );
    assert!(
        arms.iter()
            .all(|a| a.get("trust").and_then(Json::as_str).is_none()),
        "as frozen, no arm states its rule: the promotion commit is what adds them"
    );
}

/// Under the schema that only carries masks the freeze is a refused file, and under the schema whose
/// arms state their rule the same eight arms are eight measurements with eight identities. The first
/// refusal is the proof that the collision is now policed rather than silently written twice; the second
/// half is the freeze's first prerequisite, met.
#[test]
fn the_frozen_arms_are_eight_measurements_once_the_rule_reaches_the_plan() {
    let value = follow_up();
    // Read as a mask-only file, with a corpus supplied so the reader gets past the empty list, the
    // duplicate arms are refused.
    let as_schema_one = with_run(
        &edited(&value, "schema", |_| Json::text(Protocol::SCHEMA)),
        &FROZEN_SAMPLE,
    );
    let error = Protocol::from_json(&as_schema_one).expect_err(
        "eight arms that differ only in a word inside their names are three measurements",
    );
    assert!(error.contains("one measurement under two names"), "{error}");
    assert!(
        error.contains("full-current-trust") && error.contains("full-min-channels-2"),
        "and it names the two arms it caught: {error}"
    );

    // Promoted, the same eight arms are eight plans and eight identities.
    let promoted = with_run(&promote(&value), &FROZEN_SAMPLE);
    let protocol = Protocol::from_json(&promoted).expect("a promoted freeze reads");
    assert_eq!(protocol.arms.len(), 8);
    for (name, rule) in FROZEN_RULES {
        let arm = protocol
            .arms
            .iter()
            .find(|a| a.name == name)
            .unwrap_or_else(|| panic!("the promotion lost {name}"));
        assert_eq!(
            arm.trust, rule,
            "{name} states a different rule than its name"
        );
    }
    let mut identities: Vec<String> = Vec::new();
    for arm in &protocol.arms {
        let (plan, _frozen) = protocol.arm(&arm.name).expect("the arm selects");
        assert_eq!(
            plan.trust, arm.trust,
            "the arm's rule has to reach the plan that runs it"
        );
        let id = identity_of(
            RESULTS_SCHEMA,
            &plan_identity_shape(&plan),
            &Json::Arr(vec![]),
        );
        assert!(
            !identities.contains(&id),
            "{} shares an identity with an arm beside it",
            arm.name
        );
        identities.push(id);
        // The reference arm is the plan a committed E1.x row was measured by, field for field beside
        // the ladder this protocol froze — which is what makes the reproduction gate a comparison at all.
        if arm.trust == Trust::Current {
            assert!(
                !plan_json(&plan).to_compact().contains("trust"),
                "the current rule must not rename a measurement it did not change"
            );
        } else {
            assert!(
                plan_json(&plan)
                    .get("trust")
                    .and_then(Json::as_str)
                    .is_some(),
                "{} is a variant that names itself nowhere",
                arm.name
            );
        }
    }
    assert_eq!(identities.len(), 8, "eight arms, eight measurements");
}

/// The plan section as the identity digests it. A helper rather than a call to `results_json` because
/// this test is about eight plans and not about a document: the same object the writer would have
/// produced, without an environment to invent.
fn plan_identity_shape(plan: &Plan) -> Json {
    let mut environment = Environment::current();
    environment.notes.clear();
    results_json(&[], &[], plan, &environment, None)
        .get("plan")
        .expect("a document carries its plan")
        .clone()
}
