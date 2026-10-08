//! What names a measurement, and what does not.
//!
//! A results file used to be named for the second it finished. That makes the identity of an
//! experiment a fact about the clock rather than about the experiment, so re-running one plan produces
//! a second artefact that cannot be recognised as the same question, and "have I already measured
//! this?" has no answer short of diffing two files by hand. These tests hold the replacement to its
//! promises: derived from the document's own schema, plan and entries, insensitive to the order the
//! flags were typed and to where the run happened, sensitive to every part of the question actually
//! changing — including what its columns mean.

use aporia_bench::corpus::Entry;
use aporia_bench::harness::{Plan, identity_of, results_identity, results_json};
use aporia_bench::truth::{Declared, Truth};
use aporia_dsl::lower::compile;
use aporia_ir::Model;
use aporia_search::Strategy;
use aporia_store::{Environment, Json};
use std::path::PathBuf;

const SOURCE: &str =
    "model i \"\" {\n input x in [-10, 10]\n let y = sqrt(x)\n require finite(y)\n}\n";

fn model() -> Model {
    let c = compile("i.ap", SOURCE);
    assert!(!c.diagnostics.has_errors(), "{}", c.diagnostics);
    c.model
}

/// A declared region, distinct per index, so a truth with two regions does not digest the same as one
/// with two copies of the same region.
fn declared(i: usize) -> Declared {
    Declared {
        predicate: None,
        reason: format!("x below {i}"),
        axes: vec![("x".to_string(), [-10.0, -1.0 - i as f64])],
    }
}

fn truth(fault: &str, regions: usize) -> Truth {
    Truth {
        schema: "aporia.truth/1".to_string(),
        matched_to: None,
        choice_claims: Vec::new(),
        method: "static".to_string(),
        fault: fault.to_string(),
        derivation: String::new(),
        regions: (0..regions).map(declared).collect(),
        boundaries: Vec::new(),
        control: regions == 0,
        static_expected: false,
        narrow: false,
        curved: false,
        degenerate: false,
        expects: String::new(),
    }
}

/// An entry with `name`, carrying a claim about `regions` declared regions.
fn entry(family: &str, name: &str, fault: &str, regions: usize) -> Entry {
    entry_claimed(family, name, fault, regions, "x in [0, 1]")
}

/// The same entry, with the numbers it declares spelled out — which is the part an editor changes
/// and the identity used not to notice.
fn entry_claimed(family: &str, name: &str, fault: &str, regions: usize, claim: &str) -> Entry {
    Entry {
        family: family.to_string(),
        name: name.to_string(),
        dir: PathBuf::from("benchmarks").join(family).join(name),
        source: SOURCE.to_string(),
        model: Some(model()),
        diagnostics: Vec::new(),
        // What the loader would have read off the file: the claim, digested. Two entries that claim
        // different things must not share a digest, or the identity would not notice an edit.
        truth_digest: aporia_store::digest::sha256_hex(
            format!("{fault}:{regions}:{claim}").as_bytes(),
        )[..16]
            .to_string(),
        truth: truth(fault, regions),
    }
}

fn document(plan: &Plan, entries: &[Entry]) -> Json {
    let mut environment = Environment::current();
    environment.notes.push((
        "corpus".to_string(),
        aporia_bench::corpus_root().display().to_string(),
    ));
    results_json(&[], entries, plan, &environment, None)
}

fn named(doc: &Json) -> String {
    doc.get("identity")
        .and_then(Json::as_str)
        .expect("a results document records its own identity")
        .to_string()
}

fn plan(budgets: &[u64], strategies: &[&str], seeds: &[u64]) -> Plan {
    Plan {
        budgets: budgets.to_vec(),
        // Parsed by the enum that owns the names, so this helper cannot drift into testing a vocabulary
        // the tool no longer has — which is the same reason the CLI refuses `halton`.
        strategies: strategies
            .iter()
            .map(|s| Strategy::parse(s).unwrap_or_else(|| panic!("not a strategy name: {s}")))
            .collect(),
        seeds: seeds.to_vec(),
        ..Plan::default()
    }
}

#[test]
fn a_fourth_arm_is_a_new_experiment_and_cannot_overwrite_the_old_one() {
    let entries = vec![entry(
        "analytic",
        "sqrt_domain",
        "sqrt of a negative input",
        1,
    )];
    let three = plan(&[40, 80], &["adaptive", "stratified", "random"], &[1]);
    let four = plan(
        &[40, 80],
        &["adaptive", "levelset", "stratified", "random"],
        &[1],
    );
    let (old, new) = (
        named(&document(&three, &entries)),
        named(&document(&four, &entries)),
    );
    assert_ne!(
        old, new,
        "adding the level-set baseline changes what is measured, so it must change the name"
    );
    assert_eq!(
        new,
        named(&document(
            &plan(
                &[80, 40],
                &["random", "adaptive", "levelset", "stratified"],
                &[1]
            ),
            &entries
        )),
        "membership names the experiment, the order the flags were typed in does not"
    );
    // The default plan is the E1 plan: four arms, and a reader of `explain` or `verdict` gets the arm
    // they asked for rather than one of three.
    assert_eq!(
        Plan::default().strategies,
        vec![
            Strategy::Adaptive,
            Strategy::LevelSet,
            Strategy::Stratified,
            Strategy::Random
        ]
    );
}

#[test]
fn one_plan_over_one_corpus_entry_names_itself_the_same_every_time() {
    let p = plan(&[40, 80], &["adaptive", "random"], &[1, 2]);
    let entries = vec![
        entry("analytic", "sqrt_domain", "sqrt of a negative input", 1),
        entry("control", "symplectic_spring", "no fault declared", 0),
    ];
    let first = named(&document(&p, &entries));
    let second = named(&document(&p, &entries));
    assert_eq!(first, second, "one measurement must have one name");
    assert_eq!(first.len(), 12, "{first}");
    assert_eq!(
        results_identity(&document(&p, &entries)).as_deref(),
        Some(first.as_str()),
        "reading the identity back must agree with the field the writer put in"
    );
}

#[test]
fn the_order_the_flags_were_typed_in_does_not_make_a_new_experiment() {
    let entries = vec![entry(
        "analytic",
        "sqrt_domain",
        "sqrt of a negative input",
        1,
    )];
    let forward = plan(&[40, 80], &["adaptive", "random"], &[1, 2]);
    let reversed = plan(&[80, 40], &["random", "adaptive"], &[2, 1]);
    assert_eq!(
        named(&document(&forward, &entries)),
        named(&document(&reversed, &entries)),
        "the same budgets, strategies and seeds listed in another order ask one question"
    );
}

#[test]
fn a_different_plan_is_a_different_measurement() {
    let entries = vec![entry(
        "analytic",
        "sqrt_domain",
        "sqrt of a negative input",
        1,
    )];
    let base = named(&document(&plan(&[40], &["adaptive"], &[1]), &entries));
    let changed = [
        plan(&[80], &["adaptive"], &[1]),
        plan(&[40], &["random"], &[1]),
        plan(&[40], &["adaptive"], &[2]),
    ];
    for p in changed {
        assert_ne!(
            base,
            named(&document(&p, &entries)),
            "a plan that differs in budget, strategy or seed must not share a name: {p:?}"
        );
    }
}

#[test]
fn the_rates_that_cost_evaluations_are_part_of_the_identity() {
    // A measurement made with the Differential channel sampled is not the same measurement made
    // without it, and the results README says so -- so the name has to say so too, or two documents
    // that answer different questions would collide.
    let entries = vec![entry(
        "analytic",
        "sqrt_domain",
        "sqrt of a negative input",
        1,
    )];
    let mut with = plan(&[40], &["adaptive"], &[1]);
    with.differential_every = 11;
    let mut without = plan(&[40], &["adaptive"], &[1]);
    without.differential_every = 0;
    assert_ne!(
        named(&document(&with, &entries)),
        named(&document(&without, &entries))
    );

    let mut probe_on = plan(&[40], &["adaptive"], &[1]);
    probe_on.probe_every = 3;
    let mut probe_off = plan(&[40], &["adaptive"], &[1]);
    probe_off.probe_every = 0;
    assert_ne!(
        named(&document(&probe_on, &entries)),
        named(&document(&probe_off, &entries)),
        "turning the star probe off changes what was measured"
    );
}

#[test]
fn the_entries_covered_are_part_of_the_identity() {
    let one = vec![entry(
        "analytic",
        "sqrt_domain",
        "sqrt of a negative input",
        1,
    )];
    let two = vec![
        entry("analytic", "sqrt_domain", "sqrt of a negative input", 1),
        entry("analytic", "log_positive", "log of a non-positive input", 1),
    ];
    let p = plan(&[40], &["adaptive"], &[1]);
    assert_ne!(
        named(&document(&p, &one)),
        named(&document(&p, &two)),
        "measuring one more entry is not the same experiment"
    );
    // The order the selection was assembled in is not part of the question either.
    let swapped = vec![two[1].clone(), two[0].clone()];
    assert_eq!(
        named(&document(&p, &two)),
        named(&document(&p, &swapped)),
        "the same two entries listed the other way round ask the same question"
    );
}

#[test]
fn an_edited_ground_truth_is_a_different_measurement() {
    // The claim a detection rate is scored against lives in `truth.json`. If it moves, a number
    // carrying the old name would be a different number wearing the same label.
    let before = vec![entry(
        "analytic",
        "sqrt_domain",
        "sqrt of a negative input",
        1,
    )];
    let after = vec![entry(
        "analytic",
        "sqrt_domain",
        "sqrt of a negative input, now claimed over two regions",
        2,
    )];
    let p = plan(&[40], &["adaptive"], &[1]);
    assert_ne!(
        named(&document(&p, &before)),
        named(&document(&p, &after)),
        "the fault text and the region count both travel in the identity"
    );
}

#[test]
fn editing_a_regions_numbers_renames_the_measurement() {
    // The doc comment on `entries_json` already claimed that "measuring against an edited
    // `truth.json` is a different measurement", and the digest did not cover the edit: the entry
    // section carried the name, the fault, the method, the control flag and the *count* of regions.
    // Moving a declared bound left every one of those unchanged, so a corrected run could be written
    // under a published run's name. The claim's bytes are in the identity now.
    let before = vec![entry_claimed(
        "analytic",
        "sqrt_domain",
        "sqrt of a negative input",
        1,
        "x in [-10, -0.000000001]",
    )];
    let after = vec![entry_claimed(
        "analytic",
        "sqrt_domain",
        "sqrt of a negative input",
        1,
        "x in [-10, 0]",
    )];
    let p = plan(&[40], &["adaptive"], &[1]);
    let (a, b) = (named(&document(&p, &before)), named(&document(&p, &after)));
    assert_ne!(
        a, b,
        "an entry that claims a different region is a different measurement, whatever it is called"
    );
}

#[test]
fn where_the_run_happened_does_not_name_it() {
    // Provenance, not definition: the same plan measured on another machine, or read from another
    // corpus directory, is the same experiment, and its numbers are meant to be comparable. The
    // environment still travels inside the document.
    let entries = vec![entry(
        "analytic",
        "sqrt_domain",
        "sqrt of a negative input",
        1,
    )];
    let p = plan(&[40], &["adaptive"], &[1]);
    let mut here = Environment::current();
    here.notes.push(("corpus".to_string(), "/a".to_string()));
    let mut elsewhere = Environment::current();
    elsewhere.os = "linux".to_string();
    elsewhere.arch = "aarch64".to_string();
    elsewhere.rust_channel = "1.99.0".to_string();
    elsewhere
        .notes
        .push(("corpus".to_string(), "/b".to_string()));
    let a = results_json(&[], &entries, &p, &here, None);
    let b = results_json(&[], &entries, &p, &elsewhere, None);
    assert_eq!(named(&a), named(&b));
    assert_ne!(
        a.get("environment"),
        b.get("environment"),
        "and yet the environment is recorded, so the provenance is not lost"
    );
}

#[test]
fn a_document_written_before_the_field_existed_is_still_nameable() {
    // The published measurements predate `identity`, and a reader of one of them is entitled to ask
    // which experiment it holds. So the identity is recomputed from the plan and entries the file
    // itself records, rather than being read out of a filename that is a timestamp.
    let entries = vec![entry(
        "analytic",
        "sqrt_domain",
        "sqrt of a negative input",
        1,
    )];
    let p = plan(&[40], &["adaptive"], &[1]);
    let fresh = document(&p, &entries);
    let derived = match &fresh {
        Json::Obj(fields) => Json::object(
            fields
                .iter()
                .filter(|(k, _)| k != "identity")
                .map(|(k, v)| (k.as_str(), v.clone()))
                .collect(),
        ),
        _ => panic!("the document is an object"),
    };
    assert!(
        derived.get("identity").is_none(),
        "the copy must really lack the field"
    );
    assert_eq!(
        results_identity(&derived).as_deref(),
        Some(named(&fresh).as_str()),
        "deriving from the schema, plan and entries must reproduce what the writer recorded"
    );
    // And a document that is not a measurement at all yields no name rather than inventing one.
    assert_eq!(results_identity(&Json::object(vec![])), None);
}

#[test]
fn the_identity_is_a_digest_of_the_sections_it_claims_to_read() {
    let entries = vec![entry(
        "analytic",
        "sqrt_domain",
        "sqrt of a negative input",
        1,
    )];
    let doc = document(&plan(&[40], &["adaptive"], &[1]), &entries);
    assert_eq!(
        doc.get("identity").and_then(Json::as_str),
        Some(
            identity_of(
                doc.get("schema").and_then(Json::as_str).unwrap(),
                doc.get("plan").unwrap(),
                doc.get("entries").unwrap(),
            )
            .as_str()
        ),
        "the recorded identity is the digest of the schema, plan and entries in the same file"
    );
}

#[test]
fn a_field_that_changed_what_it_means_is_a_different_measurement() {
    // The reason the schema is in the digest. `aporia.results/1` recorded one number per
    // counterexample row and called it evaluations while it was a call count; `/2` records calls and
    // executions. Re-measuring the same plan over the same corpus after that change has to produce a
    // document with its own name, or the refusal that protects a published file would refuse the
    // corrected run as "already measured" — and the old file keeps its own identity either way.
    let entries = vec![entry(
        "analytic",
        "sqrt_domain",
        "sqrt of a negative input",
        1,
    )];
    let p = plan(&[40], &["adaptive"], &[1]);
    let current = document(&p, &entries);
    let historical = match &current {
        Json::Obj(fields) => Json::object(
            fields
                .iter()
                .filter(|(k, _)| k != "identity")
                .map(|(k, v)| {
                    (
                        k.as_str(),
                        if k == "schema" {
                            Json::text("aporia.results/1")
                        } else {
                            v.clone()
                        },
                    )
                })
                .collect(),
        ),
        _ => panic!("the document is an object"),
    };
    assert_eq!(aporia_bench::harness::RESULTS_SCHEMA, "aporia.results/2");
    assert_ne!(
        results_identity(&historical),
        Some(named(&current)),
        "a run of the corrected definition must not answer to the old measurement's name"
    );
    assert!(
        results_identity(&historical).is_some(),
        "a published file must still be nameable from what it records"
    );
}
