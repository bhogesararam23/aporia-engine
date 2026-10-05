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
