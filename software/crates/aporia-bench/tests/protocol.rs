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
        // Unusual for the same reason as every other field here: a plan section that quietly dropped
        // the scale axis would be read back as the committed semantics, and this fixture would pass.
        // The bar is not a power of two on purpose, so a round-trip through text is exercised.
        resolution: aporia_boundary::Resolution::Demand,
        certify_share: 0.05,
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
fn a_promoted_freeze_reads_and_is_pinned_to_the_sets_it_froze() {
    // `e1-3b-trust-resolution.json` was a pre-registration written before its corpus and before the two
    // instrument changes it depends on. Both exist now and the corpus is authored, so the protection
    // moved: it is no longer that the runner cannot read the file, it is that the file is *readable and
    // pinned* — the deciding text, the entry sets and the arm rules are checked as fields, so a later
    // edit that quietly widens a set or reword a rule fails here rather than changing what was measured.
    let path = corpus_root()
        .join("protocols")
        .join("e1-3b-trust-resolution.json");
    let text = std::fs::read_to_string(&path).expect("the follow-up protocol is in the tree");
    let protocol = Protocol::load(&path).expect("a promoted freeze is a protocol the runner reads");
    assert_eq!(protocol.id, "e1-3b-trust-resolution");
    // The digest is of the file's bytes, so the promotion moved it and every document written from the
    // promoted file cites the promoted version rather than the draft.
    let digest = aporia_store::digest::sha256_hex(text.as_bytes());
    assert_eq!(
        protocol.digest.as_deref(),
        Some(&digest[..16]),
        "a document has to cite the bytes it was run from"
    );

    // The three lists, as the file states them.
    let as_text =
        |names: &[&str]| -> Vec<String> { names.iter().map(|s| (*s).to_string()).collect() };
    assert_eq!(
        protocol.entries,
        as_text(&FROZEN_RUN),
        "entries.run is not what was frozen"
    );
    assert_eq!(protocol.resolvable, as_text(&FROZEN_RESOLVABLE));
    assert_eq!(protocol.sub_granular, as_text(&FROZEN_CONTROL));
    // The two sets are the whole run list, which is what the reader enforces and what makes the split
    // the file's claim rather than the analysis's choice.
    let mut both = protocol.resolvable.clone();
    both.extend(protocol.sub_granular.iter().cloned());
    assert_eq!(both.len(), protocol.entries.len());
    // The frozen plan, field by field: the ladder is E1.x's plus the one budget the freeze explains,
    // and neither a mask nor a rule is stated where the schema says each arm states its own.
    assert_eq!(protocol.plan.budgets, vec![320, 640, 1280, 2560]);
    assert_eq!(protocol.plan.grid, 41);
    assert_eq!(protocol.plan.differential_every, 11);
    assert_eq!(protocol.arm_names().len(), 8);
    assert!(
        protocol.plan.ablate.is_empty(),
        "the plan section states no mask: each arm names its own, or the file is two experiments"
    );
    assert_eq!(
        protocol.plan.trust,
        Trust::Current,
        "and it states no rule either, for the same reason"
    );
    // E1.x measured seventeen entries and E1.3b measures eight. Only the three controls cross over, and
    // they cross as the control set the freeze named — not as evidence. If a future edit added a
    // geometry or discrete entry to this run list, E1.x's rows would be re-labelled under a trust rule
    // without E1.x being re-run, which is the one thing `supersedes_nothing` forbids.
    let geometry: Json = Json::parse(&protocol_text()).expect("E1.x's protocol is JSON");
    let measured: Vec<&str> = geometry
        .get("entries")
        .and_then(|e| e.get("run"))
        .and_then(Json::as_array)
        .expect("E1.x states its entries")
        .iter()
        .filter_map(Json::as_str)
        .collect();
    let crossed: Vec<String> = protocol
        .entries
        .iter()
        .filter(|name| measured.contains(&name.as_str()))
        .cloned()
        .collect();
    assert_eq!(
        crossed,
        as_text(&FROZEN_CONTROL),
        "only the three controls may be entries E1.x already measured"
    );

    // The claim the lists make is the one the instrument's own arithmetic makes. `cli`'s load-time gate
    // refuses a resolvable entry the partition denies, so a freeze whose list the gate would refuse is a
    // freeze that cannot be run — and the check belongs here, before any arm has measured anything,
    // rather than as a surprise in the middle of a campaign.
    let corpus = aporia_bench::corpus::load(&corpus_root()).expect("the committed corpus loads");
    let policy = protocol.plan.trust.policy();
    for name in &protocol.resolvable {
        let entry = corpus
            .iter()
            .find(|e| &e.id() == name)
            .unwrap_or_else(|| panic!("{name} is in the freeze and not in the corpus"));
        let record = aporia_bench::reach::reach(entry, &policy);
        assert_eq!(
            record.class,
            aporia_bench::reach::Class::Enclosed,
            "{name} is claimed resolvable and the partition says {}",
            record.class.name()
        );
    }
    for name in &protocol.sub_granular {
        let entry = corpus
            .iter()
            .find(|e| &e.id() == name)
            .expect("the control set names a corpus entry");
        let record = aporia_bench::reach::reach(entry, &policy);
        assert!(
            !record.class.resolvable(),
            "{name} is the control for being unresolvable and the partition calls it {}",
            record.class.name()
        );
    }
}

/// What the freeze commits to, checked as fields so a promotion cannot quietly rewrite the decider, and
/// the two entry lists checked as load-bearing rather than decorative: each of the three ways a schema-2
/// file can mis-state its universe is refused by name.
#[test]
fn the_promoted_freeze_still_states_the_decider_it_froze() {
    let value = follow_up();
    let field = |key: &str| -> Json {
        value
            .get(key)
            .unwrap_or_else(|| panic!("the freeze states no {key}"))
            .clone()
    };
    assert_eq!(field("frozen").as_str(), Some("2026-10-09"));
    assert_eq!(
        field("schema").as_str(),
        Some(Protocol::SCHEMA_V2),
        "the promoted file is the schema whose arms state their rule"
    );
    assert!(
        field("status")
            .as_str()
            .unwrap_or_default()
            .contains("NOT RUN"),
        "and it still says so: promotion makes a freeze runnable, not measured"
    );
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

    // The lists are load-bearing rather than decorative.
    for (what, entries) in [
        (
            "an entry in neither set",
            with_lists(
                &value,
                &FROZEN_RUN,
                &FROZEN_RESOLVABLE,
                &FROZEN_CONTROL[..2],
            ),
        ),
        (
            "an entry in both sets",
            with_lists(
                &value,
                &FROZEN_RUN,
                &FROZEN_RESOLVABLE,
                &[FROZEN_CONTROL[0], FROZEN_RESOLVABLE[0]],
            ),
        ),
        (
            "an empty deciding set",
            with_lists(&value, &FROZEN_RUN, &[], &FROZEN_CONTROL),
        ),
    ] {
        let error = Protocol::from_json(&entries)
            .err()
            .unwrap_or_else(|| panic!("{what} was accepted as a protocol"));
        assert!(
            error.contains("resolvable") || error.contains("cover entries.run"),
            "{what}: the refusal must name the set it is about: {error}"
        );
    }
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

/// The five entries E1.3b authored for its deciding set, in the order the file names them.
const FROZEN_RESOLVABLE: [&str; 5] = [
    "granular/rail_ceiling",
    "granular/stroke_reference",
    "granular/match_return",
    "granular/diagonal_count",
    "granular/tolerance_stack",
];

/// The three entries that stay the control set: 0038's corpus, unchanged, expected to over-claim under
/// every variant and never scored on.
const FROZEN_CONTROL: [&str; 3] = [
    "edge/overflow_tail",
    "edge/pole_at_edge",
    "geometry/narrow_oblique",
];

/// `entries.run`, which the schema-2 reader requires the two sets above to cover exactly.
const FROZEN_RUN: [&str; 8] = [
    "granular/rail_ceiling",
    "granular/stroke_reference",
    "granular/match_return",
    "granular/diagonal_count",
    "granular/tolerance_stack",
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

/// One field removed from an object, which is how a test writes the shape an older schema expects.
fn without(value: &Json, key: &str) -> Json {
    let Json::Obj(fields) = value else {
        return value.clone();
    };
    Json::Obj(
        fields
            .iter()
            .filter(|(name, _)| name != key)
            .cloned()
            .collect(),
    )
}

/// The promoted freeze read as a mask-only file: the older schema, with every arm's rule taken back out
/// of it. This is the shape the freeze had before the promotion, and the shape whose duplicate arms the
/// reader now has to catch.
fn as_mask_only(value: &Json) -> Json {
    let with_schema = edited(value, "schema", |_| Json::text(Protocol::SCHEMA));
    edited(&with_schema, "arms", |arms| {
        let Json::Arr(list) = arms else {
            unreachable!("the freeze states its arms as a list");
        };
        Json::Arr(list.iter().map(|arm| without(arm, "trust")).collect())
    })
}

/// The same protocol with all three entry lists rewritten. The promoted file states them as lists, so a
/// test that wants the reader's refusals about the universe has to mis-state one of them on purpose.
fn with_lists(value: &Json, run: &[&str], resolvable: &[&str], sub_granular: &[&str]) -> Json {
    let list = |names: &[&str]| Json::Arr(names.iter().map(|n| Json::text(*n)).collect());
    edited(value, "entries", |entries| {
        let with_run = edited(entries, "run", |_| list(run));
        let with_resolvable = edited(&with_run, "resolvable", |_| list(resolvable));
        edited(&with_resolvable, "sub_granular", |_| list(sub_granular))
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

/// The eight arms are four trust variants across three channel masks. The masks alone do not separate
/// them — eight names, three measurements — so the promotion had to put the rule in a field an arm
/// states, which is what the freeze's first prerequisite asked the instrument for.
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
    // The dimension the experiment is about is a field on every arm, not a word inside a label — which
    // is the promotion contract, and what makes the eight arms above eight measurements rather than
    // three names written twice.
    for (name, rule) in FROZEN_RULES {
        let arm = arms
            .iter()
            .find(|a| a.get("name").and_then(Json::as_str) == Some(name))
            .unwrap_or_else(|| panic!("the file lost {name}"));
        assert_eq!(
            arm.get("trust").and_then(Json::as_str),
            Some(rule.name()),
            "{name} does not state the rule its label claims"
        );
    }
}

/// Under the schema that only carries masks the freeze is a refused file, and under the schema whose
/// arms state their rule the same eight arms are eight measurements with eight identities. The first
/// refusal is the proof that the collision is policed rather than silently written twice; the second
/// half is the freeze's first prerequisite, met.
#[test]
fn the_frozen_arms_are_eight_measurements_once_the_rule_reaches_the_plan() {
    let value = follow_up();
    // The same eight arms read as a mask-only file, with their rules taken back out, are three
    // measurements under eight names — and the reader says so instead of writing two files that mean
    // the same thing.
    let as_schema_one = as_mask_only(&value);
    let error = Protocol::from_json(&as_schema_one).expect_err(
        "eight arms that differ only in a word inside their names are three measurements",
    );
    assert!(error.contains("one measurement under two names"), "{error}");
    assert!(
        error.contains("full-current-trust") && error.contains("full-min-channels-2"),
        "and it names the two arms it caught: {error}"
    );

    // As committed, the eight arms are eight plans and eight identities.
    let protocol = Protocol::from_json(&value).expect("the promoted freeze reads");
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

/// The E1.4 freeze, as the tree holds it before any arm has run.
fn e14_value() -> Json {
    let path = e14_path();
    let text = std::fs::read_to_string(&path).expect("the E1.4 freeze is in the tree");
    Json::parse(&text).expect("it is JSON")
}

fn e14_path() -> PathBuf {
    corpus_root()
        .join("protocols")
        .join("e1-4-resolution-demand.json")
}

/// Documents that cite a protocol id, so a freeze can be checked against whether it has been run.
fn documents_citing(id: &str) -> Vec<PathBuf> {
    let dir = corpus_root().join("results");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if !name.starts_with("results-")
            || path.extension().and_then(|e| e.to_str()) != Some("json")
        {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(value) = Json::parse(&text) else {
            continue;
        };
        if value
            .get("protocol")
            .and_then(|p| p.get("id"))
            .and_then(Json::as_str)
            == Some(id)
        {
            found.push(path);
        }
    }
    found
}

/// Replace a top-level field of the E1.4 freeze, in memory, for the refusals below.
fn e14_with(value: &Json, key: &str, replacement: Json) -> Json {
    let Json::Obj(fields) = value else {
        panic!("the freeze is an object");
    };
    let mut out = fields.clone();
    for (name, held) in &mut out {
        if name == key {
            *held = replacement.clone();
        }
    }
    Json::Obj(out)
}

/// Drop one field from every object of a list, so a test can ask what a `/3` file that forgets to
/// state an axis is told.
fn e14_strip_items(value: &Json, list: &str, field: &str) -> Json {
    let Json::Obj(fields) = value else {
        panic!("the freeze is an object");
    };
    let mut out = fields.clone();
    for (name, held) in &mut out {
        if name != list {
            continue;
        }
        let Json::Arr(items) = held else {
            panic!("{list} is not a list");
        };
        let stripped = items
            .iter()
            .map(|item| {
                let Json::Obj(pairs) = item else {
                    panic!("a {list} entry is not an object");
                };
                Json::Obj(
                    pairs
                        .iter()
                        .filter(|(key, _)| key != field)
                        .cloned()
                        .collect(),
                )
            })
            .collect();
        *held = Json::Arr(stripped);
    }
    Json::Obj(out)
}

#[test]
fn the_e1_4_freeze_reads_under_schema_3_and_its_obligations_are_enforced() {
    // Promotion is the point of a schema version: an obligation a runner cannot check is a sentence in
    // a file rather than a constraint on a measurement. `/3` says every arm states its mask, its trust
    // rule and its scale rule, that the plan states none of the three but does state the bar, and that
    // the run list is partitioned into a deciding set, a control-of-the-partition set and a set of
    // entries marked control. This reads the frozen file and then refuses six edits to it.
    let protocol = Protocol::load(&e14_path()).expect("a promoted schema is one the runner reads");
    assert_eq!(protocol.id, "e1-4-resolution-demand");
    let text = std::fs::read_to_string(e14_path()).expect("the freeze is in the tree");
    let digest = aporia_store::digest::sha256_hex(text.as_bytes());
    assert_eq!(
        protocol.digest.as_deref(),
        Some(&digest[..16]),
        "a document has to cite the bytes it was run from"
    );

    // The arms: four cells of two declared axes, on one mask and one trust rule.
    assert_eq!(protocol.arms.len(), 4, "two axes, four arms, no padding");
    for arm in &protocol.arms {
        assert!(
            arm.ablate.is_empty(),
            "{} silences a channel; this experiment varies the scale rule, not the evidence",
            arm.name
        );
        assert_eq!(arm.trust, Trust::Current, "{}", arm.name);
    }
    let rules: Vec<&str> = protocol.arms.iter().map(|a| a.resolution.name()).collect();
    assert_eq!(
        rules,
        vec!["current", "demand", "withhold", "demand-and-withhold"],
        "and the control arm is the first row of the table"
    );

    // The plan holds the bar and none of the axes the arms hold.
    assert_eq!(
        protocol.plan.certify_share, 0.003_906_25,
        "the bar the freeze states is the bar the plan carries"
    );
    assert_eq!(protocol.plan.certify_share, 2f64.powi(-8));
    assert!(protocol.plan.ablate.is_empty(), "the plan states no mask");
    assert_eq!(
        protocol.plan.trust,
        Trust::Current,
        "the plan states no rule"
    );
    assert_eq!(
        protocol.plan.resolution,
        aporia_boundary::Resolution::Current,
        "the plan states no scale rule either, for the same reason"
    );

    // The entry sets, including the one E1.3b froze without.
    assert_eq!(
        protocol.control,
        vec!["control/symplectic_spring".to_string()]
    );
    assert_eq!(
        protocol.sub_granular,
        vec!["geometry/narrow_oblique".to_string()]
    );
    assert_eq!(
        protocol.resolvable,
        vec![
            "granular/stroke_reference".to_string(),
            "granular/tolerance_stack".to_string(),
            "granular/fit_band".to_string()
        ]
    );

    // An arm is a plan, not a label: choosing it has to move the axis that reaches the identity, the
    // archive directory and the campaign's policy, or four arms measure one measurement four times.
    use aporia_boundary::Resolution;
    for (name, expected) in [
        ("A-current", Resolution::Current),
        ("B-demand", Resolution::Demand),
        ("C-withhold", Resolution::Withhold),
        ("D-demand-and-withhold", Resolution::DemandAndWithhold),
    ] {
        let (plan, frozen) = protocol
            .arm(name)
            .unwrap_or_else(|e| panic!("{}: {e}", protocol.id));
        assert_eq!(plan.resolution, expected, "{name} did not reach the plan");
        assert_eq!(
            plan.certify_share, protocol.plan.certify_share,
            "{name} moved the bar, which is the plan's to state"
        );
        assert_eq!(frozen.arm, name);
    }
    // The plan sections must differ, which is what puts the axis inside the measurement identity.
    let sections: Vec<String> = protocol
        .arms
        .iter()
        .map(|arm| {
            let (plan, _) = protocol.arm(&arm.name).expect("an arm names itself");
            plan_json(&plan).to_compact()
        })
        .collect();
    let mut distinct = sections.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        4,
        "four arms whose plan sections are identical are one measurement under four names"
    );
    // A-current is the committed semantics: its plan section carries neither rule, neither mask, and
    // neither scale axis, so it digests to the bytes that named the measurements already published.
    assert!(
        !sections[0].contains("resolution") && !sections[0].contains("certify_share"),
        "the control arm wrote an axis into its own plan section: {}",
        sections[0]
    );

    let value = e14_value();
    let list =
        |names: &[&str]| -> Json { Json::Arr(names.iter().map(|n| Json::text(*n)).collect()) };
    for (what, edited) in [
        (
            "an arm that states no scale rule",
            e14_strip_items(&value, "arms", "resolution"),
        ),
        (
            "two arms that differ in nothing the instrument can see",
            e14_with(
                &value,
                "arms",
                Json::Arr(vec![
                    Json::object(vec![
                        ("name", Json::text("A-current")),
                        ("ablate", Json::Arr(Vec::new())),
                        ("trust", Json::text("current")),
                        ("resolution", Json::text("current")),
                    ]),
                    Json::object(vec![
                        ("name", Json::text("B-demand")),
                        ("ablate", Json::Arr(Vec::new())),
                        ("trust", Json::text("current")),
                        ("resolution", Json::text("demand")),
                    ]),
                    Json::object(vec![
                        ("name", Json::text("E-clone-of-A")),
                        ("ablate", Json::Arr(Vec::new())),
                        ("trust", Json::text("current")),
                        ("resolution", Json::text("current")),
                    ]),
                ]),
            ),
        ),
        (
            "no control set, which is the defect E1.3b reported",
            e14_with(&value, "entries", {
                let Json::Obj(fields) = value.get("entries").expect("entries is an object") else {
                    panic!("entries is not an object");
                };
                Json::Obj(
                    fields
                        .iter()
                        .filter(|(key, _)| key != "control")
                        .cloned()
                        .chain(std::iter::once((
                            "control".to_string(),
                            Json::Arr(Vec::new()),
                        )))
                        .collect(),
                )
            }),
        ),
        (
            "an entry in the resolvable set and in the control set",
            e14_with(&value, "entries", {
                let Json::Obj(fields) = value.get("entries").expect("entries is an object") else {
                    panic!("entries is not an object");
                };
                Json::Obj(
                    fields
                        .iter()
                        .map(|(key, held)| {
                            if key == "control" {
                                (key.clone(), list(&["granular/fit_band"]))
                            } else {
                                (key.clone(), held.clone())
                            }
                        })
                        .collect(),
                )
            }),
        ),
        (
            "a plan that states the scale rule itself",
            e14_with(&value, "plan", {
                let Json::Obj(fields) = value.get("plan").expect("plan is an object") else {
                    panic!("plan is not an object");
                };
                let mut with_rule = fields.clone();
                with_rule.push(("resolution".to_string(), Json::text("demand")));
                Json::Obj(with_rule)
            }),
        ),
        (
            "a plan that leaves the bar to a default",
            e14_with(&value, "plan", {
                let Json::Obj(fields) = value.get("plan").expect("plan is an object") else {
                    panic!("plan is not an object");
                };
                Json::Obj(
                    fields
                        .iter()
                        .filter(|(key, _)| key != "certify_share")
                        .cloned()
                        .collect(),
                )
            }),
        ),
    ] {
        let error = Protocol::from_json(&edited)
            .err()
            .unwrap_or_else(|| panic!("{what} was accepted as a protocol"));
        assert!(
            error.contains("resolution")
                || error.contains("control")
                || error.contains("certify_share")
                || error.contains("two names"),
            "{what}: the refusal has to name what it is refusing: {error}"
        );
    }

    // And the older schemas keep refusing an axis they do not define: a `/2` file that gained a
    // resolution field in one of its arms would otherwise be read two ways by two builds, which is the
    // reason these obligations are per schema version rather than per field.
    let as_v2 = e14_with(&value, "schema", Json::text(Protocol::SCHEMA_V2));
    let error = Protocol::from_json(&as_v2)
        .err()
        .expect("a schema-2 arm may not state a scale rule");
    assert!(
        error.contains("resolution") && error.contains(Protocol::SCHEMA_V3),
        "the refusal should name the axis and the schema that owns it: {error}"
    );
}

/// A top-level field of the E1.4 freeze, named rather than defaulted when it is absent: the
/// deciding text of a pre-registration has to go missing loudly.
fn e14_field(key: &str) -> Json {
    let value = e14_value();
    value
        .get(key)
        .unwrap_or_else(|| panic!("the freeze states no {key}"))
        .clone()
}

#[test]
fn the_e1_4_freeze_states_its_metric_and_the_four_arms_it_moves() {
    // The deciding text, pinned as fields while it is still only a promise. This is the part of a
    // pre-registration that must not be editable after the run, and a reader that cannot yet enforce
    // it can still make an edit show up here rather than in a table nobody questions.
    let field = e14_field;
    assert_eq!(field("frozen").as_str(), Some("2026-10-19"));

    // The primary metric, its denominator, and the bar it is measured against.
    let metric = field("primary_metric");
    assert_eq!(
        metric.get("name").and_then(Json::as_str),
        Some("coarse_certified_volume")
    );
    assert!(
        metric
            .get("denominator")
            .and_then(Json::as_str)
            .unwrap_or_default()
            .contains("declared domain"),
        "the denominator is stated, because a share without one is a preference"
    );
    let share = field("plan")
        .get("certify_share")
        .and_then(Json::as_f64)
        .expect("the bar is a plan field and not prose");
    assert_eq!(
        share, 0.003_906_25,
        "2^-8 exactly, so no leaf sits on the bar by rounding"
    );
    assert_eq!(share, 2f64.powi(-8));

    // Four arms, which is the whole grid of the two axes moved, on one mask and one trust rule.
    let arms = field("arms")
        .as_array()
        .expect("the freeze states its arms")
        .to_vec();
    assert_eq!(
        arms.len(),
        4,
        "two axes, four cells, no fifth arm for company"
    );
    for arm in &arms {
        assert!(
            arm.get("ablate")
                .and_then(Json::as_array)
                .is_some_and(<[Json]>::is_empty),
            "every arm runs the full instrument: the channels are not what this freeze is testing"
        );
        assert_eq!(arm.get("trust").and_then(Json::as_str), Some("current"));
        assert!(
            arm.get("resolution").and_then(Json::as_str).is_some(),
            "a /3 arm that omits its resolution rule is a different measurement saying so nowhere"
        );
    }
    let rules: Vec<&str> = arms
        .iter()
        .filter_map(|a| a.get("resolution").and_then(Json::as_str))
        .collect();
    assert_eq!(
        rules,
        vec!["current", "demand", "withhold", "demand-and-withhold"],
        "and in that order, so the control is the first row of every table"
    );
    // The plan states none of the three axes. An arm that does not name its own mask and rule would
    // make this file two experiments wearing one identity, which is the refusal `/2` was built for.
    let plan = field("plan");
    for axis in ["ablate", "trust", "resolution"] {
        assert!(
            plan.get(axis).is_none(),
            "the plan section states {axis}, which is the arms to state"
        );
    }
}

#[test]
fn the_e1_4_freeze_names_its_entry_sets_and_its_decision_rule() {
    // The second half of the same promise, in its own test because the two halves are decided by
    // different arguments: what a row means is fixed by the metric, and which rows exist is fixed by
    // the corpus. Someone editing a freeze after the fact changes one and not the other.
    let field = e14_field;

    // The entry sets, including the set E1.3b froze without.
    let entries = field("entries");
    let list = |key: &str| -> Vec<String> {
        entries
            .get(key)
            .and_then(Json::as_array)
            .unwrap_or_else(|| panic!("the freeze states no entries.{key}"))
            .iter()
            .filter_map(Json::as_str)
            .map(str::to_string)
            .collect()
    };
    let run = list("run");
    let resolvable = list("resolvable");
    let sub_granular = list("sub_granular");
    let control = list("control");
    assert!(
        !control.is_empty(),
        "a /3 freeze with no control set is the defect E1.3b reported as unmeasurable"
    );
    let mut covered = resolvable.clone();
    covered.extend(sub_granular.iter().cloned());
    covered.extend(control.iter().cloned());
    covered.sort();
    let mut sorted_run = run.clone();
    sorted_run.sort();
    assert_eq!(
        covered, sorted_run,
        "the three sets are the run list, exactly once each"
    );
    assert_eq!(
        run.len(),
        5,
        "five entries: four declared regions and the clean control"
    );
    assert!(
        run.contains(&"granular/fit_band".to_string()),
        "the resolvable entry built to over-claim under the full instrument is why this freeze exists"
    );

    // Seven outcomes, the null one first, with the two ceilings that decide them inside the rule.
    let rule = field("primary_metric")
        .get("decision_rule")
        .and_then(Json::as_array)
        .expect("the decision rule travels with the metric it decides")
        .to_vec();
    assert_eq!(rule.len(), 7);
    assert!(
        rule[0]
            .as_str()
            .unwrap_or_default()
            .contains("cannot state the question"),
        "the first outcome is the one where the corpus cannot ask"
    );
    assert!(
        rule[1]
            .as_str()
            .unwrap_or_default()
            .contains("certifying almost nothing"),
        "and the second refuses silence as a result, which is what E1.3b rule 3 had to learn"
    );
    let text = rule
        .iter()
        .filter_map(Json::as_str)
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        text.contains("0.10"),
        "the UNKNOWN ceiling is stated in the rule, not in prose beside it"
    );
    assert!(
        text.contains("one quarter"),
        "and so is the floor under which an arm certifies nothing"
    );

    // A freeze records what it supersedes by not touching it.
    let supersedes = field("supersedes_nothing")
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        supersedes.contains("stays failed"),
        "the condition E1.3 failed stays failed, as in every freeze since"
    );
    assert!(
        supersedes.contains("E1.3b is measured"),
        "and E1.3b's decision is cited as a measurement, not as a defect to repair"
    );

    // The status text and the tree have to agree, in whichever direction the run goes. Written as a
    // two-way check so this assertion cannot be weakened later by editing the freeze to match a run:
    // either the freeze still says NOT RUN and no document cites it, or documents exist and the
    // freeze no longer claims it has not been measured.
    let status = field("status").as_str().unwrap_or_default().to_string();
    let measured = documents_citing("e1-4-resolution-demand");
    if measured.is_empty() {
        assert!(
            status.contains("FROZEN, NOT RUN"),
            "no document cites this freeze, so its status must still say it has not been run"
        );
    } else {
        assert!(
            !status.contains("NOT RUN"),
            "{} results document(s) cite this freeze while its status still says NOT RUN",
            measured.len()
        );
    }
}
