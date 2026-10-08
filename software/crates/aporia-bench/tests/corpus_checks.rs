//! What the corpus has to satisfy before a number is produced from it.
//!
//! `aporia-bench run` verifies every declaration and refuses to measure against one that does not
//! hold, because a detection rate scored against a wrong ground truth is worse than no number. These
//! tests are about the checks that are *not* about whether a region is where it claims to be: a
//! declaration that names a parameter the model does not have cannot be checked at all, and used to be
//! scored as a boundary that missed.

use aporia_bench::corpus;

fn entry(id: &str) -> corpus::Entry {
    let entries = corpus::load(&corpus_root()).expect("the corpus loads");
    entries
        .iter()
        .find(|e| e.id() == id)
        .unwrap_or_else(|| panic!("no corpus entry {id}"))
        .clone()
}

fn corpus_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../benchmarks")
}

#[test]
fn the_committed_corpus_declares_only_axes_its_models_have() {
    // The positive half, and the one that has to be true for every published number: no entry names a
    // parameter that its own model does not declare.
    let entries = corpus::load(&corpus_root()).expect("the corpus loads");
    assert!(entries.len() > 10, "only {} entries found", entries.len());
    for e in &entries {
        for b in &e.truth.boundaries {
            if e.truth.static_expected {
                continue;
            }
            let Some(model) = &e.model else { continue };
            assert!(
                model.param(&b.axis).is_some(),
                "{} declares a boundary on {b:?}, which its model does not have",
                e.id()
            );
        }
    }
    let problems = corpus::verify(&entries, 12);
    assert!(
        problems.is_empty(),
        "the corpus no longer verifies: {:?}",
        problems
            .iter()
            .map(|p| format!("{}: {}", p.entry, p.detail))
            .collect::<Vec<_>>()
    );
}

#[test]
fn a_typo_in_a_declared_axis_stops_the_measurement_instead_of_becoming_a_missed_boundary() {
    let mut e = entry("ode/euler_decay");
    assert!(
        !e.truth.boundaries.is_empty(),
        "this entry is chosen because it declares a boundary"
    );
    assert!(
        corpus::verify(&[e.clone()], 12).is_empty(),
        "the untouched entry should verify"
    );
    e.truth.boundaries[0].axis = "dtee".to_string();

    let problems = corpus::verify(&[e.clone()], 12);
    let text: Vec<String> = problems
        .iter()
        .map(|p| format!("{}: {}", p.entry, p.detail))
        .collect();
    assert!(
        text.iter()
            .any(|t| t.contains("dtee") && t.contains("no parameter named")),
        "a boundary on a nonexistent axis was not refused: {text:?}"
    );
}

#[test]
fn a_region_naming_an_axis_the_model_does_not_have_is_refused_too() {
    let mut e = entry("analytic/sqrt_domain");
    assert!(
        !e.truth.regions.is_empty(),
        "this entry is chosen because it declares a region"
    );
    e.truth.regions[0].axes = vec![("z".to_string(), [-10.0, 0.0])];
    let text: Vec<String> = corpus::verify(&[e], 12)
        .iter()
        .map(|p| p.detail.clone())
        .collect();
    assert!(
        text.iter().any(|t| t.contains("no parameter named \"z\"")),
        "a region on a nonexistent axis was not refused: {text:?}"
    );
}

#[test]
fn no_published_measurement_came_from_a_model_this_command_cannot_execute() {
    // The corpus is the set the published numbers were produced from. If any entry declared a value
    // that only a program can supply, the harness would have sampled it to death for NaN and reported
    // the resulting divergence as a finding — so the claim worth testing on the committed corpus is
    // that every entry is interpretable.
    let entries = corpus::load(&corpus_root()).expect("the corpus loads");
    let external: Vec<String> = entries
        .iter()
        .filter(|e| e.model.as_ref().is_some_and(aporia_runtime::needs_adapter))
        .map(corpus::Entry::id)
        .collect();
    assert!(
        external.is_empty(),
        "the harness has no --program, so these entries cannot be measured here: {external:?}"
    );
}

#[test]
fn a_declared_external_entry_is_refused_before_any_budget_is_spent() {
    let mut e = entry("analytic/sqrt_domain");
    e.model = Some(
        aporia_dsl::lower::compile(
            "beam.ap",
            "model beam \"\" {\n input load : N in [0, 100]\n output deflection : mm\n \
             require deflection >= 0\n}\n",
        )
        .model,
    );
    e.truth.regions = vec![aporia_bench::truth::Declared {
        predicate: None,
        reason: "the program goes negative".to_string(),
        axes: vec![("load".to_string(), [60.0, 100.0])],
    }];
    let text: Vec<String> = corpus::verify(&[e], 12)
        .iter()
        .map(|p| p.detail.clone())
        .collect();
    assert!(
        text.iter().any(|t| t.contains("no --program")),
        "an external model was accepted for measurement: {text:?}"
    );
}

#[test]
fn the_reference_path_finishes_wherever_the_corpus_measures_it() {
    // The differential channel now refuses to compare a reference that ran out of steps, and names a
    // reference that left the real numbers while the runtime did not. Both branches are inert on the
    // corpus as it stands, and this is where that is checked rather than asserted in a comment: the
    // claim "this change moves no published number" has to be re-derivable by a reader, not trusted
    // from a session log.
    //
    // The grid is the search's own unit square at 64 deterministic points per entry, and the step
    // guard is the one every committed ladder run used.
    let entries = corpus::load(&corpus_root()).expect("the corpus loads");
    let mut non_finite_readings = 0usize;
    for e in &entries {
        let Some(m) = &e.model else { continue };
        for k in 0..64u64 {
            let unit: Vec<f64> = (0..m.params.len())
                .map(|i| ((k.wrapping_mul(7 + i as u64) % 64) as f64) / 63.0)
                .collect();
            let x = aporia_search::to_parameters(m, &unit);
            let r = aporia_numerics::reference::evaluate(m, &x, 2_000_000);
            assert!(
                !r.budget_exceeded,
                "{} at {x:?}: the reference path exhausted its 2M-step budget after {} steps, so the \
                 differential channel would report an un-compared point rather than a comparison",
                e.id(),
                r.steps
            );
            let fast = aporia_runtime::interp::run(m, &x, aporia_runtime::ExecConfig::default());
            for (j, (f, rv)) in fast.outputs.iter().zip(r.values()).enumerate() {
                if !rv.is_finite() {
                    non_finite_readings += 1;
                    assert!(
                        !f.is_finite(),
                        "{} at {x:?} output {j}: the reference produced {rv} while the runtime \
                         produced {f}, which is a one-sided divergence the ladder has never been \
                         scored on — the channel must name it now, so this entry needs its own \
                         investigation rather than a silenced pair",
                        e.id()
                    );
                }
            }
        }
    }
    // Recorded, not asserted away: some corpus models genuinely leave the reals inside their declared
    // domains, and that is what the sqrt and reciprocal entries exist to test.
    assert!(
        non_finite_readings > 0,
        "no entry left the real numbers anywhere, which would mean the boundary entries stopped \
         being boundary entries"
    );
}

#[test]
fn an_entry_the_plan_covers_but_cannot_measure_is_named_with_its_reason() {
    // E13's pre-registered metric in 0029 is "refusal at the IR gate, not a label": the experiment is
    // that the refusal is *reported*. `run_corpus` had been skipping unmeasurable entries in silence
    // while its own doc comment said they were reported, so a reader of a results file could not tell
    // an entry the plan refused from an entry the plan never had.
    use aporia_bench::harness::{Plan, results_json};
    use aporia_store::{Environment, Json};
    let entries = corpus::load(&corpus_root()).expect("the corpus loads");
    let mut environment = Environment::current();
    environment.notes.clear();
    let doc = results_json(&[], &entries, &Plan::default(), &environment, None);
    let refused = doc
        .get("refused")
        .and_then(Json::as_array)
        .expect("a results document says which entries it refused");
    let named: Vec<&str> = refused
        .iter()
        .map(|r| r.get("entry").and_then(Json::as_str).unwrap_or("?"))
        .collect();
    let expected: Vec<String> = entries
        .iter()
        .filter(|e| corpus::unmeasurable(e).is_some())
        .map(corpus::Entry::id)
        .collect();
    assert_eq!(
        named,
        expected.iter().map(String::as_str).collect::<Vec<_>>(),
        "the document names exactly the entries the runner skips, in corpus order"
    );
    assert!(
        named.contains(&"mutants/unit_mistake"),
        "the corpus's static entry is the one a sweep can never measure: {named:?}"
    );
    for record in refused {
        let id = record.get("entry").and_then(Json::as_str).unwrap_or("?");
        let reason = record
            .get("reason")
            .and_then(Json::as_str)
            .unwrap_or("<missing>");
        assert!(
            !reason.is_empty(),
            "{id} was refused without a reason being recorded"
        );
        if id == "mutants/unit_mistake" {
            assert!(
                reason.contains("static"),
                "a refusal of a declared-static entry must say so, got {reason:?}"
            );
        }
    }
}

#[test]
fn skipping_an_entry_and_reporting_it_are_the_same_decision() {
    // One predicate for both, or they drift: a sweep list and a refusal list that disagree about the
    // same entry is a document that describes an experiment nobody ran.
    use aporia_bench::harness::Plan;
    let all = corpus::load(&corpus_root()).expect("the corpus loads");
    let chosen: Vec<_> = all
        .iter()
        .filter(|e| {
            [
                "mutants/unit_mistake",
                "analytic/sqrt_domain",
                "synthetic/wide_1d",
            ]
            .contains(&e.id().as_str())
        })
        .cloned()
        .collect();
    assert_eq!(chosen.len(), 3, "the mixed selection this test needs");
    let plan = Plan {
        budgets: vec![40],
        strategies: vec![aporia_search::Strategy::Adaptive],
        seeds: vec![1],
        ..Plan::default()
    };
    let swept: Vec<String> = aporia_bench::harness::run_corpus(&chosen, &plan)
        .iter()
        .map(|s| s.entry.clone())
        .collect();
    let refused: Vec<String> = chosen
        .iter()
        .filter(|e| corpus::unmeasurable(e).is_some())
        .map(corpus::Entry::id)
        .collect();
    assert_eq!(swept.len(), 2, "{swept:?} / {refused:?}");
    assert_eq!(refused, vec!["mutants/unit_mistake".to_string()]);
    for id in &swept {
        assert!(!refused.contains(id), "{id} was both swept and refused");
    }
}
