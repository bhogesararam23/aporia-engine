//! Two archives on disk, compared.
//!
//! The manifest-level rules are tested in `compare.rs`; this file checks the part of the comparison
//! that only shows up when the data comes back out of files. A finding written by a campaign and a
//! finding read from a `.apx` hold their evidence differently, the atlas numbers its cells in the order
//! one particular run refined them, and the report position of a region depends on how many other
//! regions that run found. Those are exactly the things a naive diff would turn into false
//! differences between two runs that agreed.

use aporia_boundary::{Atlas, Policy};
use aporia_dsl::lower::compile;
use aporia_evidence::{Calibrator, Channel, ChannelCorrelation, Evidence, Subject};
use aporia_ir::to_text;
use aporia_runtime::{ExecConfig, Observation, Records, interp};
use aporia_store::compare::{Change, Comparison, Field, Section};
use aporia_store::{Environment, Json, Loaded, Run, Store, StoredFinding};
use std::path::{Path, PathBuf};

/// A model with one free parameter, and one with two. Enough of a difference that their regions live
/// in different spaces, which is the case the comparison has to refuse rather than diff.
const ONE: &str =
    "model cmp \"\" {\n input x in [-10, 10]\n let y = sqrt(x)\n require finite(y)\n}\n";
const TWO: &str = "model cmp \"\" {\n input x in [-10, 10]\n input dt in [0, 1]\n let y = sqrt(x)\n require finite(y)\n}\n";

fn scratch(name: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    dir.push(format!("aporia-compare-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Everything two archives in these tests are allowed to differ in. The rest of the run is written
/// identically, so a difference the comparison reports is the difference the test put in.
#[derive(Clone, Debug)]
struct Variation {
    source: &'static str,
    budget: u64,
    samples: u64,
    risk: f64,
    findings: Vec<StoredFinding>,
}

impl Variation {
    fn new(budget: u64) -> Self {
        Self {
            source: ONE,
            budget,
            samples: 8,
            risk: 0.9,
            findings: Vec::new(),
        }
    }
}

fn finding(cell: u32, bounds: &[[f64; 2]], representative: f64, risk: f64) -> StoredFinding {
    StoredFinding {
        index: 0,
        cell,
        bounds: bounds.to_vec(),
        representative: vec![representative],
        observation: 0,
        online_risk: risk,
        final_risk: risk,
        samples: 4,
        label: "SUSPICIOUS".to_string(),
        case: None,
        evidence: vec![
            Evidence::new(
                Channel::Physical,
                Subject::Constraint(0),
                1.0,
                vec![0],
                "sqrt of a negative input".to_string(),
            )
            .absolute(1.0),
        ],
        raw_evidence: Vec::new(),
    }
}

/// Write one experiment directory from a variation, then open it the way any reader would.
fn archive(label: &str, v: &Variation) -> (PathBuf, Loaded) {
    let compiled = compile("cmp.ap", v.source);
    assert!(
        !compiled.diagnostics.has_errors(),
        "{}\n{}",
        compiled.diagnostics,
        v.source
    );
    let model = compiled.model;
    let air = to_text(&model);
    let cfg = ExecConfig::default();

    let mut records = Records::new();
    for i in 0..v.samples {
        let x = vec![-10.0 + i as f64 * 2.5, 0.25];
        let x: Vec<f64> = x.into_iter().take(model.params.len()).collect();
        let outcome = interp::run(&model, &x, cfg);
        records.push(Observation::new(i, x, &outcome));
    }
    let mut atlas = Atlas::new(&model, Policy::default());
    for o in &records.items {
        atlas.record(&o.x, v.risk, 0b10);
    }
    atlas.relabel();
    atlas.refine();
    atlas.relabel();
    let coverage = atlas.coverage();
    let atlas_csv = atlas.to_csv();
    let bands = atlas.bands();

    let calibration_source: Vec<Evidence> = records
        .items
        .iter()
        .map(|o| {
            Evidence::new(
                Channel::Sensitivity,
                Subject::LocalSlope { output: 0, axis: 0 },
                1.0 + o.x[0].abs(),
                vec![o.id],
                String::new(),
            )
        })
        .collect();
    let calibrator = Calibrator::fit(calibration_source.iter());
    let correlation = ChannelCorrelation::none();
    // Deliberately independent of the variation: a test that changes the budget must not also change
    // the decision log, or the difference it is checking for arrives twice, once as a field and once
    // as a digest.
    let decisions = vec![
        Json::object(vec![
            ("evaluation", Json::count(0)),
            ("family", Json::text("novelty")),
        ]),
        Json::object(vec![
            ("evaluation", Json::count(1)),
            ("family", Json::text("boundary")),
        ]),
    ];
    let mut environment = Environment::current();
    environment
        .notes
        .push(("execution".to_string(), "interpreter".to_string()));
    let config = Json::object(vec![
        ("budget", Json::count(v.budget)),
        ("strategy", Json::text("adaptive")),
    ]);
    let run = Run {
        model: &model,
        model_text: v.source,
        air_text: &air,
        records: &records,
        config,
        coverage,
        atlas_csv: &atlas_csv,
        bands: &bands,
        findings: &v.findings,
        decisions: &decisions,
        calibrator: &calibrator,
        correlation: &correlation,
        exec: cfg,
        evaluations: records.items.len() as u64,
        instruction_steps: records.total_steps(),
        environment,
        created_unix_ms: 0,
    };
    let root = scratch(label);
    Store::create(&root)
        .expect("create")
        .write(&run)
        .expect("write");
    let loaded = Loaded::open(&root).expect("open");
    (root, loaded)
}

/// The paths of every field the comparison had something to say about.
fn reported(fields: &[Field]) -> Vec<String> {
    fields
        .iter()
        .filter(|f| f.change != Change::Same)
        .map(|f| f.path.clone())
        .collect()
}

fn findings_of(c: &Comparison, want: Change) -> Vec<String> {
    c.fields
        .iter()
        .filter(|f| f.section == Section::Findings && f.change == want)
        .map(|f| f.path.clone())
        .collect()
}

#[test]
fn two_copies_of_one_archive_compare_with_nothing_to_report() {
    let v = Variation::new(40);
    let (a_root, a) = archive("same-a", &v);
    let (b_root, b) = archive("same-b", &v);
    let c = Comparison::new(&a, &b);
    assert!(c.identical(), "{:?}", reported(&c.fields));
    assert_eq!(c.tally(), (0, 0, 0));
    let _ = std::fs::remove_dir_all(a_root);
    let _ = std::fs::remove_dir_all(b_root);
}

#[test]
fn a_changed_budget_is_the_only_field_that_moves() {
    let (a_root, a) = archive("budget-a", &Variation::new(40));
    let (b_root, b) = archive("budget-b", &Variation::new(640));
    let c = Comparison::new(&a, &b);
    assert_eq!(reported(&c.fields), vec!["config.budget"]);
    let budget = c
        .fields
        .iter()
        .find(|f| f.path == "config.budget")
        .expect("the budget is compared");
    assert_eq!(budget.a.as_deref(), Some("40"));
    assert_eq!(budget.b.as_deref(), Some("640"));
    assert!(!c.identical());
    let _ = std::fs::remove_dir_all(a_root);
    let _ = std::fs::remove_dir_all(b_root);
}

#[test]
fn the_same_region_is_paired_even_when_the_two_atlas_runs_numbered_it_differently() {
    // Both archives describe `x in [-10, 0]`; one found it in cell 3 at report position 0, the other
    // in cell 9 at position 1, because it also found something else first. Pairing them is the whole
    // point, and the bookkeeping difference is reported as what it is: a different cell, not a
    // different region.
    let mut a = Variation::new(40);
    a.findings = vec![finding(3, &[[-10.0, 0.0]], -8.75, 0.71)];
    let mut b = Variation::new(40);
    b.findings = vec![
        finding(7, &[[-10.0, -5.0]], -8.75, 0.8),
        finding(9, &[[-10.0, 0.0]], -8.75, 0.93),
    ];
    let (a_root, la) = archive("pair-a", &a);
    let (b_root, lb) = archive("pair-b", &b);
    let c = Comparison::new(&la, &lb);
    let paired: Vec<&Field> = c
        .fields
        .iter()
        .filter(|f| f.section == Section::Findings && f.path.contains("[-10, 0]"))
        .collect();
    assert!(
        !paired.is_empty(),
        "the shared region was not paired: {:?}",
        reported(&c.fields)
    );
    assert!(
        paired
            .iter()
            .any(|f| f.path.contains(".final_risk") && f.change == Change::Changed),
        "the risk that moved is not reported: {:?}",
        paired.iter().map(|f| &f.path).collect::<Vec<_>>()
    );
    assert!(
        paired
            .iter()
            .any(|f| f.path.contains(".cell") && f.change == Change::Changed),
        "the different atlas cell is not reported"
    );
    assert!(
        !paired
            .iter()
            .any(|f| f.path.contains(".evidence") && f.change != Change::Same),
        "identical evidence was reported as a difference"
    );
    assert!(
        paired
            .iter()
            .any(|f| f.path.contains(".label") && f.change == Change::Same),
        "the label both runs agreed on should still be a compared field"
    );
    let _ = std::fs::remove_dir_all(a_root);
    let _ = std::fs::remove_dir_all(b_root);
}

#[test]
fn a_region_only_one_run_found_is_reported_as_only_there() {
    let mut a = Variation::new(40);
    a.findings = vec![
        finding(1, &[[-10.0, 0.0]], -8.75, 0.9),
        finding(2, &[[1.0, 5.0]], 2.5, 0.8),
    ];
    let mut b = Variation::new(40);
    b.findings = vec![finding(1, &[[-10.0, 0.0]], -8.75, 0.9)];
    let (a_root, la) = archive("only-a", &a);
    let (b_root, lb) = archive("only-b", &b);
    let c = Comparison::new(&la, &lb);
    let only_a = findings_of(&c, Change::OnlyA);
    assert_eq!(only_a.len(), 1, "{:?}", reported(&c.fields));
    assert!(only_a[0].contains("[1, 5]"), "{}", only_a[0]);
    let unpaired = c
        .fields
        .iter()
        .find(|f| f.change == Change::OnlyA)
        .expect("a region one run reported");
    assert!(unpaired.b.is_none(), "the side that lacks it has no value");
    assert!(
        unpaired
            .a
            .as_deref()
            .is_some_and(|v| v.contains("SUSPICIOUS"))
    );
    assert!(!c.identical());
    let _ = std::fs::remove_dir_all(a_root);
    let _ = std::fs::remove_dir_all(b_root);
}

#[test]
fn a_verified_case_on_one_side_is_absence_rather_than_a_different_case() {
    // `case` is the archive's claim that a smaller counterexample was verified. One run making that
    // claim and the other not is not two runs disagreeing about what the case is, and the fields that
    // do agree must not be swept up with it.
    let mut a = Variation::new(40);
    a.findings = vec![finding(1, &[[-10.0, 0.0]], -8.75, 0.9)];
    let mut b = Variation::new(40);
    let mut with_case = finding(1, &[[-10.0, 0.0]], -8.75, 0.9);
    with_case.case = Some("x in [-10, -0.5]".to_string());
    b.findings = vec![with_case];
    let (a_root, la) = archive("case-a", &a);
    let (b_root, lb) = archive("case-b", &b);
    let c = Comparison::new(&la, &lb);
    let case = c
        .fields
        .iter()
        .find(|f| f.path.contains(".case"))
        .expect("the case field is compared");
    assert_eq!(case.change, Change::OnlyB);
    assert_eq!(
        case.a, None,
        "no verified case is not the same as an empty one"
    );
    assert_eq!(case.b.as_deref(), Some("x in [-10, -0.5]"));
    // Within the findings, `case` is the only thing that moved: the region, its label, its risks and
    // its evidence all agree.
    let moved: Vec<String> = c
        .fields
        .iter()
        .filter(|f| f.section == Section::Findings && f.change != Change::Same)
        .map(|f| f.path.clone())
        .collect();
    assert_eq!(moved.len(), 1, "{moved:?}");
    // The bytes still disagree, and the comparison says so rather than implying the two archives are
    // interchangeable: an extra claim changes the finding file, and its digest with it.
    let digest = c
        .fields
        .iter()
        .find(|f| f.path == "digest[findings/000000.apx]")
        .expect("the finding file's digest is compared");
    assert_eq!(digest.change, Change::Changed);
    let _ = std::fs::remove_dir_all(a_root);
    let _ = std::fs::remove_dir_all(b_root);
}

#[test]
fn two_models_of_different_dimension_are_not_paired_region_by_region() {
    let mut a = Variation::new(40);
    a.findings = vec![finding(1, &[[-10.0, 0.0]], -8.75, 0.9)];
    let mut b = Variation::new(40);
    b.source = TWO;
    b.findings = vec![finding(1, &[[-10.0, 0.0], [0.0, 1.0]], -8.75, 0.9)];
    let (a_root, la) = archive("dim-a", &a);
    let (b_root, lb) = archive("dim-b", &b);
    let c = Comparison::new(&la, &lb);
    assert!(
        c.skipped.iter().any(|s| s.starts_with("findings:")),
        "the region comparison should have been refused: {:?}",
        c.skipped
    );
    assert!(
        !c.fields.iter().any(|f| f.section == Section::Findings),
        "regions from different spaces were compared anyway: {:?}",
        reported(&c.fields)
    );
    // Refusing part of a comparison is not a pass: an incomplete comparison is never `identical`.
    assert!(!c.identical());
    assert!(
        reported(&c.fields)
            .iter()
            .any(|p| p == "params" || p == "model_sha256" || p == "air_sha256"),
        "{:?}",
        reported(&c.fields)
    );
    let _ = std::fs::remove_dir_all(a_root);
    let _ = std::fs::remove_dir_all(b_root);
}

#[test]
fn a_denser_sweep_moves_the_map_and_the_comparison_says_so() {
    // Same model, same budget, more recorded points: the atlas divides differently, so the run's
    // counts differ and the regions it can name do too. Nothing here is invented -- each reported
    // field is a number the archive itself holds.
    let mut a = Variation::new(40);
    a.samples = 8;
    let mut b = Variation::new(40);
    b.samples = 12;
    let (a_root, la) = archive("dense-a", &a);
    let (b_root, lb) = archive("dense-b", &b);
    let c = Comparison::new(&la, &lb);
    let moved = reported(&c.fields);
    assert!(
        moved.iter().any(|p| p == "samples"),
        "the sweep size did not register: {moved:?}"
    );
    assert!(
        moved
            .iter()
            .any(|p| p == "digest[atlas.csv]" || p == "digest[observations.bin]"),
        "a different partition left the atlas table unexamined: {moved:?}"
    );
    assert!(!c.identical());
    let _ = std::fs::remove_dir_all(a_root);
    let _ = std::fs::remove_dir_all(b_root);
}

#[test]
fn the_map_two_runs_drew_is_compared_from_the_totals_themselves_recorded() {
    // Same model, same sweep, one run scoring every point harder. What the two archives actually
    // recorded is measured rather than assumed, and it is not what a first guess says: the harder run
    // resolves *less* of the space, because one channel's opinion holds a cell at UNKNOWN rather than
    // calling it suspicious -- suspicion needs corroboration -- and refining the map to chase it leaves
    // volume nobody has settled. The comparison reports those totals as written.
    let mut a = Variation::new(40);
    a.risk = 0.1;
    let mut b = Variation::new(40);
    b.risk = 0.9;
    let (a_root, la) = archive("map-a", &a);
    let (b_root, lb) = archive("map-b", &b);
    let c = Comparison::new(&la, &lb);
    let field = |path: &str| {
        c.fields
            .iter()
            .find(|f| f.path == path)
            .unwrap_or_else(|| panic!("no compared field {path}"))
    };
    assert_eq!(field("coverage.trusted_fraction").change, Change::Changed);
    assert_eq!(field("coverage.trusted_fraction").a.as_deref(), Some("1.0"));
    assert_eq!(field("coverage.trusted_fraction").b.as_deref(), Some("0.0"));
    assert_eq!(field("coverage.unknown_fraction").change, Change::Changed);
    assert_eq!(field("coverage.resolved_fraction").change, Change::Changed);
    assert_eq!(field("coverage.cells").change, Change::Changed);
    assert_eq!(
        field("coverage.suspicious_fraction").change,
        Change::Same,
        "neither run corroborated a region, so neither could call one suspicious"
    );
    assert_eq!(
        field("coverage.suspicious_fraction").a.as_deref(),
        Some("0.0")
    );
    assert_eq!(field("bands").change, Change::Same);
    let _ = std::fs::remove_dir_all(a_root);
    let _ = std::fs::remove_dir_all(b_root);
}

#[test]
fn a_file_one_archive_never_wrote_is_named_as_that_file() {
    // A run with no findings writes no finding files at all, so the two directories hold different
    // sets of artefacts. Reporting that as a changed field would be wrong: the second archive does
    // not have a value to compare, it has a file the first one does not.
    let a = Variation::new(40);
    let mut b = Variation::new(40);
    b.findings = vec![finding(1, &[[-10.0, 0.0]], -8.75, 0.9)];
    let (a_root, la) = archive("files-a", &a);
    let (b_root, lb) = archive("files-b", &b);
    let c = Comparison::new(&la, &lb);
    let digest = c
        .fields
        .iter()
        .find(|f| f.path == "digest[findings/000000.apx]")
        .expect("a file only the second archive holds");
    assert_eq!(digest.change, Change::OnlyB);
    assert_eq!(digest.a, None);
    let held = c
        .fields
        .iter()
        .find(|f| f.path == "findings[*.apx]")
        .expect("the number of finding files is compared");
    assert_eq!(held.a.as_deref(), Some("0"));
    assert_eq!(held.b.as_deref(), Some("1"));
    // The region it describes is reported once, as a region only one run found.
    assert_eq!(findings_of(&c, Change::OnlyB).len(), 1);
    let _ = std::fs::remove_dir_all(a_root);
    let _ = std::fs::remove_dir_all(b_root);
}

#[test]
fn comparing_the_same_two_archives_twice_gives_the_same_answer() {
    let (a_root, a) = archive("repeat-a", &Variation::new(40));
    let (b_root, b) = archive("repeat-b", &Variation::new(640));
    let first = Comparison::new(&a, &b);
    let second = Comparison::new(&a, &b);
    assert_eq!(first, second, "a comparison must repeat");
    let _ = std::fs::remove_dir_all(a_root);
    let _ = std::fs::remove_dir_all(b_root);
}

#[test]
fn a_directory_that_is_not_an_archive_does_not_compare() {
    let missing = Path::new(env!("CARGO_MANIFEST_DIR")).join("no-such-archive");
    assert!(
        Loaded::open(&missing).is_err(),
        "a reader must fail before it starts comparing"
    );
}
