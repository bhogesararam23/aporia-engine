//! The region decomposition: what the four labels cover, and what they must obey.
//!
//! `trusted_over_true` has always been one number about one label, and every experiment in this
//! repository has therefore had to infer where the volume went from where it came from. These tests
//! hold the four columns together instead:
//!
//! - the four **domain volumes** partition the declared domain, because the leaves of one atlas tile
//!   it and every leaf carries exactly one label;
//! - the four **region overlaps** partition the declared union for the same reason, so a fall in
//!   certified over-claim is readable as a rise in one of the other three rather than as safety;
//! - every overlap sits under its own label's volume, and their sum sits under the union the entry
//!   declares — bounds, not equalities, because a label may hold clean space;
//! - the union is never smaller than the best-single-region reading the historical column gives, and
//!   on a corpus that declares more than one region the two are not the same number.
//!
//! The corpus-wide checks run over the committed entries at a budget small enough to be quick and
//! large enough to sample something. A check that only ever ran on a hand-built fixture would not
//! notice a declared region the geometry cannot state.

use aporia_bench::corpus::Entry;
use aporia_bench::metrics::Outcome;
use aporia_bench::truth::Truth;
use aporia_search::{Config, Strategy, run};
use aporia_store::Json;

/// Every bound a row has to obey, read off the row itself rather than off a table of them.
///
/// Returns the list of violations, empty when the row holds. A `Vec` rather than assertions inside
/// the helper, because the corpus-wide test has to say *which* bound broke on *which* entry; one
/// `assert!` per bound would stop at the first and hide the rest.
///
/// `exact` is whether both sides of every comparison are geometry rather than sampling. On an entry
/// whose regions are all boxes the union is a sum of widths, the maximum is a clip, and the four
/// columns are a partition of the declared union to the last bit. On an entry that carves a `where`
/// both are lattice estimates taken on *different* lattices — the historical one samples inside the
/// cell's intersection with the envelope, the union samples the cell itself, and a leaf finer than
/// the domain lattice estimates a share the domain lattice never saw — so neither the additivity nor
/// the ordering is a theorem any more. Asserting them there would be asserting that two estimates
/// bracket a quantity neither of them has; what is checked instead is in
/// [`a_carved_entries_disagreement_is_the_lattices_own_and_is_reported`].
fn bounds_hold(outcome: &Outcome, exact: bool) -> Vec<String> {
    let mut notes = Vec::new();
    let partition = outcome.trusted_volume
        + outcome.unknown_volume
        + outcome.suspicious_volume
        + outcome.provisional_volume;
    // The leaves of one atlas tile the domain and nothing is left unlabelled, so the four domain
    // volumes are the domain. The tolerance is not slack: a sum of a few hundred products of axis
    // ratios is well inside it, and a leaf counted twice would blow straight past it.
    if (partition - 1.0).abs() > 1e-9 {
        notes.push(format!(
            "label volumes sum to {partition}, not to the domain"
        ));
    }
    let overlaps = [
        (
            "trusted",
            outcome.trusted_over_true_union,
            outcome.trusted_volume,
        ),
        ("unknown", outcome.unknown_over_true, outcome.unknown_volume),
        (
            "suspicious",
            outcome.suspicious_over_true,
            outcome.suspicious_volume,
        ),
        (
            "provisional",
            outcome.provisional_over_true,
            outcome.provisional_volume,
        ),
    ];
    let sum = match overlaps
        .iter()
        .map(|(_, value, _)| *value)
        .collect::<Option<Vec<_>>>()
    {
        None => {
            // A refusal is checked where it is produced. Four absences still obey every bound stated
            // here, and failing on them would hide the refusal this row is actually making.
            return notes;
        }
        Some(values) => values.iter().sum::<f64>(),
    };
    for (name, value, volume) in overlaps {
        let Some(value) = value else { continue };
        if value > volume + 1e-12 {
            notes.push(format!(
                "{name} overlaps {value} of the domain, more than its own {volume} of it"
            ));
        }
    }
    if exact {
        // Every leaf carries exactly one label and the leaves tile the domain, so on exact geometry
        // the four columns do not merely sit under the union the entry declares — they add up to it.
        // This is the strongest statement the decomposition makes, and it is only available where both
        // sides are arithmetic rather than sampling.
        if (sum - outcome.declared_union_volume).abs() > 1e-9 {
            notes.push(format!(
                "the four label overlaps sum to {sum} against the {} this entry declares",
                outcome.declared_union_volume
            ));
        }
    } else if sum > 1.0 + 1e-12 {
        notes.push(format!(
            "the four label overlaps sum to {sum}, more than the domain"
        ));
    }
    if let Some(union) = outcome.trusted_over_true_union {
        if exact && union + 1e-12 < outcome.trusted_over_true {
            notes.push(format!(
                "the union ({union}) came out under the best single region ({})",
                outcome.trusted_over_true
            ));
        }
    }
    notes
}

fn measured(entry: &Entry, budget: u64) -> Outcome {
    let campaign = run(
        entry.model.as_ref().expect("the fixture compiles"),
        Config {
            budget,
            ..Config::default()
        },
    );
    Outcome::measure(entry, &campaign, 0, Strategy::Adaptive, budget)
}

/// An entry assembled in memory: a model and its declaration, nothing on disk.
fn fixture(name: &str, model_text: &str, truth_text: &str) -> Entry {
    let compiled = aporia_dsl::lower::compile("t.ap", model_text);
    assert!(
        !compiled.diagnostics.has_errors(),
        "{}\n{model_text}",
        compiled.diagnostics
    );
    Entry {
        family: "synthetic".into(),
        name: name.into(),
        dir: std::path::PathBuf::from("."),
        source: model_text.into(),
        model: Some(compiled.model),
        diagnostics: Vec::new(),
        error_count: 0,
        truth: Truth::from_json(&Json::parse(truth_text).expect("the fixture truth parses"))
            .expect("the fixture truth reads"),
        truth_digest: "0000000000000000".into(),
    }
}

/// Two boxes that tile the axis side by side, so a leaf covering both is untrustworthy through and
/// through while the best-single-region reading of it says half.
const TILING_TRUTH: &str = r#"{"schema":"aporia.truth/1","method":"analytic","fault":"declared_well","regions":[{"reason":"lower","axes":{"x":[0,5]}},{"reason":"upper","axes":{"x":[5,10]}}],"boundaries":[]}"#;
const TILING_MODEL: &str =
    "model tiling \"\" {\n input x in [0, 10]\n let y = x / 10.0\n require y < 5.0\n}\n";

#[test]
fn the_four_labels_partition_the_domain_of_every_committed_entry() {
    let root = aporia_bench::corpus_root();
    let entries = aporia_bench::corpus::load(&root).expect("the committed corpus loads");
    let mut checked = 0usize;
    let mut exact_checked = 0usize;
    for entry in entries.iter().filter(|e| e.model.is_some()) {
        let outcome = measured(entry, 640);
        let exact = !entry.truth.has_predicates();
        let notes = bounds_hold(&outcome, exact);
        assert!(
            notes.is_empty(),
            "{} at budget 640:\n{}",
            entry.id(),
            notes.join("\n")
        );
        assert_eq!(
            outcome.region_overlap_refusals,
            0,
            "{} refuses to state a region overlap, so the bounds above were checked on a row that \
             states nothing",
            entry.id()
        );
        checked += 1;
        exact_checked += usize::from(exact);
    }
    assert!(
        checked > 30,
        "the whole measurable corpus has to be walked for this to mean anything; got {checked}"
    );
    assert!(
        exact_checked > 25,
        "the exact union-versus-maximum bound is only stated where both sides are geometry, and it \
         has to hold of most of the corpus for that qualification to mean anything; got \
         {exact_checked} of {checked}"
    );
}

#[test]
fn a_carved_disagreement_is_the_lattices_own_and_is_measured_not_assumed_away() {
    // The residual between "what the four labels say the region volume is" and "what the entry's own
    // lattice says it is", on the entries where the two are two estimates rather than one arithmetic.
    // Reported as a number, with a ceiling loose enough to be a sanity bound and tight enough to catch
    // a real double count: an actual duplicated leaf would show up many times over. Measured at HEAD,
    // the worst disagreement across the carved entries is 0.0180816650390625 on
    // `geometry/narrow_oblique` — the band four tenths of one per cent of its domain wide, whose
    // measure is a matter of which lattice points land in it, which is the same reason 0038 says it
    // cannot be localised at all.
    let root = aporia_bench::corpus_root();
    let entries = aporia_bench::corpus::load(&root).expect("the committed corpus loads");
    let mut worst = (0.0f64, String::new());
    let mut carved = 0usize;
    for entry in entries.iter().filter(|e| e.model.is_some()) {
        if !entry.truth.has_predicates() {
            continue;
        }
        carved += 1;
        let outcome = measured(entry, 640);
        let sum = [
            outcome.trusted_over_true_union,
            outcome.unknown_over_true,
            outcome.suspicious_over_true,
            outcome.provisional_over_true,
        ]
        .into_iter()
        .map(|value| value.unwrap_or(0.0))
        .sum::<f64>();
        let residual = (sum - outcome.declared_union_volume).abs();
        if residual > worst.0 {
            worst = (residual, entry.id());
        }
    }
    assert!(
        carved > 0,
        "the corpus has no carved entry, so this test is checking nothing"
    );
    assert!(
        worst.0 < 0.05,
        "the carved entries disagree by up to {} on {}, which is no longer a sampling difference",
        worst.0,
        worst.1
    );
}

#[test]
fn a_leaf_two_regions_tile_reads_half_to_the_history_and_whole_to_the_union() {
    let entry = fixture("tiling", TILING_MODEL, TILING_TRUTH);
    let outcome = measured(&entry, 640);
    assert_eq!(
        outcome.declared_union_volume, 1.0,
        "the two boxes tile the axis"
    );
    // The historical column on the same certified cells: the best single region any of them lies in.
    let best = entry
        .truth
        .overlap_fraction(entry.model.as_ref().unwrap(), &domain(&entry));
    assert!((best - 0.5).abs() < 1e-12, "{best}");
    let union = outcome.trusted_over_true_union.unwrap_or(0.0)
        + outcome.unknown_over_true.unwrap_or(0.0)
        + outcome.suspicious_over_true.unwrap_or(0.0)
        + outcome.provisional_over_true.unwrap_or(0.0);
    assert!(
        (union - 1.0).abs() < 1e-9,
        "every leaf of this partition is inside a declared region, so the four overlaps have to add \
         to the whole domain and not to the {union} they do"
    );
    assert!(
        outcome.trusted_over_true < outcome.trusted_over_true_union.unwrap_or(0.0),
        "the certified half of this row reads {:?} to the maximum and {:?} to the union",
        outcome.trusted_over_true,
        outcome.trusted_over_true_union
    );
}

#[test]
fn an_entry_that_declares_nothing_overlaps_nothing_and_says_zero_rather_than_absence() {
    let entry = fixture(
        "clean",
        "model clean \"\" {\n input x in [0, 1]\n let y = x * x\n require y < 1.0\n}\n",
        r#"{"schema":"aporia.truth/1","method":"analytic","fault":"none","control":true,"regions":[],"boundaries":[]}"#,
    );
    let outcome = measured(&entry, 320);
    assert_eq!(outcome.declared_union_volume, 0.0);
    for value in [
        outcome.trusted_over_true_union,
        outcome.unknown_over_true,
        outcome.suspicious_over_true,
        outcome.provisional_over_true,
    ] {
        assert_eq!(
            value,
            Some(0.0),
            "a control declares a region of measure zero, which is an answer and not an absence"
        );
    }
}

#[test]
fn a_decomposition_the_geometry_refuses_is_four_absences_and_a_count_of_why() {
    // Ten axes with two regions naming every one of them puts the union past what this build will
    // enumerate, so the honest row states nothing about overlaps and says how many cells it could not
    // state. A zero here would be the invention the metric definition refuses.
    let names: Vec<&str> = vec!["a", "b", "c", "d", "e", "f", "g", "h", "i", "j"];
    let model = format!(
        "model wide \"\" {{\n{}\n let z = {}\n require z < 5\n}}\n",
        names
            .iter()
            .map(|n| format!(" input {n} in [0, 1]"))
            .collect::<Vec<_>>()
            .join("\n"),
        names.join("+")
    );
    let spans = |lo: f64, hi: f64| {
        format!(
            "{{{}}}",
            names
                .iter()
                .map(|n| format!("\"{n}\":[{lo},{hi}]"))
                .collect::<Vec<_>>()
                .join(",")
        )
    };
    let truth = format!(
        r#"{{"schema":"aporia.truth/1","method":"analytic","fault":"x","regions":[{{"reason":"r1","axes":{}}},{{"reason":"r2","axes":{}}}]}}"#,
        spans(0.1, 0.2),
        spans(0.3, 0.4)
    );
    let entry = fixture("wide", &model, &truth);
    let outcome = measured(&entry, 1);
    assert!(
        outcome.region_overlap_refusals > 0,
        "the refusal has to be counted, not swallowed"
    );
    assert_eq!(
        outcome.declared_union_volume, 0.0,
        "and the denominator states the refusal as its own absence"
    );
    for value in [
        outcome.trusted_over_true_union,
        outcome.unknown_over_true,
        outcome.suspicious_over_true,
        outcome.provisional_over_true,
    ] {
        assert_eq!(value, None, "a refused decomposition is absent, not zero");
    }
    // The domain volumes still partition the space, refusal or no refusal: what the atlas did is a
    // separate fact from what the geometry could state about it.
    let notes = bounds_hold(&outcome, true);
    assert!(notes.is_empty(), "{notes:?}");
}

#[test]
fn a_zero_volume_overlap_prints_as_zero_and_not_as_its_own_sign() {
    // The results README records `-0.0` in `trusted_over_true`'s committed bytes and does not fix it,
    // because normalising a published row to change how a zero displays is not a correction. The
    // columns written from now on are born clean, so a reader is not asked to hold a sign of a
    // quantity that has no sign.
    let entry = fixture(
        "clean",
        "model clean \"\" {\n input x in [0, 1]\n let y = x * x\n require y < 1.0\n}\n",
        r#"{"schema":"aporia.truth/1","method":"analytic","fault":"none","control":true,"regions":[],"boundaries":[]}"#,
    );
    let text = measured(&entry, 320).to_json().to_compact();
    for field in [
        "declared_union_volume",
        "trusted_over_true_union",
        "unknown_over_true",
        "suspicious_over_true",
        "provisional_over_true",
    ] {
        let needle = format!("\"{field}\":-0.0");
        assert!(!text.contains(&needle), "{needle} in a published row");
    }
}

#[test]
fn measuring_the_same_cell_twice_writes_the_same_numbers() {
    let entry = fixture("tiling", TILING_MODEL, TILING_TRUTH);
    let first = measured(&entry, 640);
    let second = measured(&entry, 640);
    assert_eq!(
        first.to_json().to_compact(),
        second.to_json().to_compact(),
        "the decomposition has to be a property of the run, not of when it was read"
    );
}

#[test]
fn the_union_and_the_best_single_region_are_not_the_same_number_on_the_committed_corpus() {
    // If this ever passes vacuously, the new columns are decoration and the honest report is that no
    // entry in this corpus has a cell two regions tile.
    let root = aporia_bench::corpus_root();
    let entries = aporia_bench::corpus::load(&root).expect("the committed corpus loads");
    let mut diverging: Vec<String> = Vec::new();
    for entry in entries.iter().filter(|e| e.model.is_some()) {
        let model = entry.model.as_ref().expect("filtered above");
        let cell = aporia_bench::truth::domain_bounds(model);
        let best = entry.truth.overlap_fraction(model, &cell);
        let union = entry
            .truth
            .union_overlap(model, &cell)
            .expect("the committed corpus is stateable");
        if (union - best).abs() > 1e-12 {
            diverging.push(format!("{}: union {union} against best {best}", entry.id()));
        }
    }
    assert!(
        !diverging.is_empty(),
        "no committed entry distinguishes the two geometries on its own domain"
    );
    // `ode/euler_decay_2d` is the twenty-four-region staircase this was written for, so it belongs in
    // the list rather than only in the comment above.
    assert!(
        diverging
            .iter()
            .any(|line| line.starts_with("ode/euler_decay_2d")),
        "the multi-region staircase does not distinguish the geometries: {diverging:?}"
    );
}

#[test]
fn explain_prints_the_decomposition_from_the_function_a_row_uses() {
    // The readable output is where a metric either becomes part of how the instrument is understood or
    // stays a column nobody opens. It is printed from `Outcome::measure` rather than recomputed here,
    // so what a reader sees and what a results document carries cannot drift into two definitions.
    let root = aporia_bench::corpus_root();
    let entries = aporia_bench::corpus::load(&root).expect("the committed corpus loads");
    let entry = entries
        .iter()
        .find(|e| e.id() == "granular/stroke_reference")
        .expect("the entry E1.4 crossed over is in the corpus");
    let plan = aporia_bench::harness::Plan::default();
    let text = aporia_bench::harness::explain(entry, &plan, Strategy::Adaptive, 1)
        .expect("the entry compiles");
    assert!(
        text.contains("region decomposition:"),
        "explain has to state the decomposition: {text}"
    );
    for needle in [
        "declared regions cover",
        "certified",
        "unknown",
        "suspicious",
        "provisional",
        "best single region",
    ] {
        assert!(text.contains(needle), "the line omits {needle}:\n{text}");
    }
    assert!(
        !text.contains("refused a region overlap"),
        "this entry is stateable, so the refusal sentence must not print:\n{text}"
    );
}

/// The declared domain of a fixture, in the bounds the atlas partitions.
fn domain(entry: &Entry) -> Vec<[f64; 2]> {
    aporia_bench::truth::domain_bounds(entry.model.as_ref().expect("the fixture compiles"))
}
