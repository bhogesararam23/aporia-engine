//! Reachability: what the atlas could enclose, decided before a run rather than explained after one.
//!
//! 0038's account of E1.3's failed condition turned on a geometric fact that was only visible after
//! 330 rows had been measured: on the edge entries no leaf the depth cap permits can be half inside the
//! declared region, so localisation was unreachable and the only route to a pass was for the region's
//! leaf to stay UNKNOWN. E1.3b's instrument therefore asks the same question of the same arithmetic at
//! load time — and the tests here are why the answer can be trusted to mean what the post-mortem said.
//!
//! The two properties that matter most are the two that a shortcut would break. A class must come from
//! the atlas's own partition rule and the truth language's own overlap measure, so it cannot disagree
//! with the run it precedes; and it must not rename a measurement, so a check that arrived after the
//! committed rows cannot retroactively re-file them.

use aporia_bench::cli;
use aporia_bench::corpus;
use aporia_bench::harness::{Plan, Protocol, RESULTS_SCHEMA, identity_of, results_json};
use aporia_bench::metrics::LOCALISATION_BAR;
use aporia_bench::reach::{Class, Reach, reach, reaches};
use aporia_boundary::Trust;
use aporia_store::{Environment, Json};
use std::path::PathBuf;

fn policy() -> aporia_boundary::Policy {
    Plan::default().trust.policy()
}

fn entries() -> Vec<corpus::Entry> {
    corpus::load(&aporia_bench::corpus_root()).expect("the corpus loads")
}

fn one(id: &str) -> corpus::Entry {
    entries()
        .iter()
        .find(|e| e.id() == id)
        .unwrap_or_else(|| panic!("{id} is not in the corpus"))
        .clone()
}

#[test]
fn the_entries_0038_called_unreachable_are_still_called_unreachable() {
    // This is the regression pin on the post-mortem's central claim, re-read from the committed corpus
    // rather than restated in prose. E1.3's two edge entries are sub-granular: their envelopes are
    // narrower than any permitted leaf can be, so *no* trust rule E1.3b can write would localise them,
    // and a row that stays UNKNOWN on them measures nothing about trust.
    for (id, class) in [
        ("edge/overflow_tail", Class::SubGranular),
        ("edge/pole_at_edge", Class::SubGranular),
    ] {
        let record = reach(&one(id), &policy());
        assert_eq!(record.class, class, "{id}: {}", record.row());
        let region = record
            .regions
            .first()
            .unwrap_or_else(|| panic!("{id} declares no region"));
        assert!(
            region.best_envelope_share < LOCALISATION_BAR,
            "{id} reaches the bar on its envelope after all: {}",
            region.best_envelope_share
        );
        assert_eq!(region.candidates, 0, "{id} found a candidate leaf");
        assert!(!record.class.resolvable(), "{id} is not resolvable");
    }
    // `geometry/narrow_oblique` is the entry 0038 grouped with those two, and this instrument draws the
    // distinction the prose could not: its *envelope* is the whole domain, so leaves reach the bar
    // easily; it is the `where` carve — a band thinner than a leaf — that no leaf can be half inside.
    // Same consequence for E1.3, different arithmetic, and E1.3b's resolvable set has to be able to say
    // which one it is refusing.
    let oblique = reach(&one("geometry/narrow_oblique"), &policy());
    assert_eq!(oblique.class, Class::CarvedAway, "{}", oblique.row());
    let region = oblique
        .regions
        .first()
        .expect("narrow_oblique declares one region");
    assert_eq!(
        region.candidates, oblique.permitted_cells,
        "every leaf is inside this envelope, which is the point of the entry"
    );
    assert!(
        region.best_declared_share < LOCALISATION_BAR,
        "the carve reaches the bar after all: {}",
        region.best_declared_share
    );
}

#[test]
fn a_measure_zero_declaration_is_unreachable_rather_than_safe() {
    // `projectile_zero_gravity` declares g = 0 exactly: a region with no volume for any leaf to be half
    // inside. The verification lattice still checks it at the named point, which is why the corpus
    // carries it, and `corpus::verify` passes it. What it must not do is read as resolvable, or as
    // anything else that could be scored as a safety outcome — an entry the instrument cannot enclose
    // contributes no evidence either way.
    let record = reach(&one("aerospace/projectile_zero_gravity"), &policy());
    assert_eq!(record.class, Class::Unreachable, "{}", record.row());
    assert!(!record.class.resolvable());
    // The class is not `SubGranular` either, because no amount of depth would help: the region is a
    // plane in one axis, and the two failures need different fixes.
    assert_ne!(record.class, Class::SubGranular);
}

#[test]
fn an_enclosed_region_comes_with_a_witness_leaf_a_reader_can_check() {
    let record = reach(&one("analytic/sqrt_domain"), &policy());
    assert_eq!(record.class, Class::Enclosed, "{}", record.row());
    let region = record
        .regions
        .first()
        .expect("sqrt_domain declares one region");
    let depth = region
        .witness_depth
        .expect("an enclosed class without a witness depth is a class without a witness");
    assert!(
        usize::from(depth) <= usize::from(policy().max_depth),
        "the witness sits past the depth cap: {depth}"
    );
    assert!(
        region.best_declared_share >= LOCALISATION_BAR,
        "the witness does not reach the bar: {}",
        region.best_declared_share
    );
    assert_eq!(
        region.witness_bounds.len(),
        record.axes,
        "a witness bounds every axis of the model"
    );
    // The witness is a leaf the atlas could actually build, checked against the partition it was drawn
    // from rather than against a description of it: the bounds have to be one of the boxes
    // `permitted_partition` enumerates, byte for byte.
    let model = one("analytic/sqrt_domain")
        .model
        .expect("sqrt_domain compiles");
    let cells = aporia_boundary::Atlas::permitted_partition(&model, policy())
        .expect("the frozen depth enumerates");
    assert!(
        cells.iter().any(|c| c.bounds == region.witness_bounds),
        "the witness is not a box this partition holds: {:?}",
        region.witness_bounds
    );
}

#[test]
fn every_entry_is_classified_and_the_answer_does_not_move() {
    let corpus = entries();
    let first = reaches(&corpus, &policy());
    assert_eq!(
        first.len(),
        corpus.len(),
        "an entry the check skipped is an entry that quietly left the experiment"
    );
    for (a, b) in first.iter().zip(reaches(&corpus, &policy()).iter()) {
        assert_eq!(
            a.json().to_compact(),
            b.json().to_compact(),
            "{} was classified twice and got two answers",
            a.entry
        );
    }
    // Every entry lands in a named class, and the two classes that mean "this cannot be scored" are
    // counted rather than absorbed into the resolvable set.
    let mut resolvable = 0usize;
    let mut unmeasurable = 0usize;
    for record in &first {
        if record.class.resolvable() {
            resolvable += 1;
        }
        if matches!(
            record.class,
            Class::NoModel | Class::Uncappable | Class::NoRegion
        ) {
            unmeasurable += 1;
        }
        assert!(
            Class::ALL.contains(&record.class),
            "{} produced a class outside the vocabulary",
            record.entry
        );
    }
    assert!(resolvable > 0, "the corpus has no reachable region at all");
    assert!(
        unmeasurable >= 8,
        "controls and refused domains vanished from the classification: {unmeasurable}"
    );
}

#[test]
fn a_class_recorded_in_a_document_does_not_rename_the_measurement_it_describes() {
    // The compatibility rule that makes the reproduction gate possible, in the other direction: the
    // reachability section had to be added somewhere, and if it had gone into the hashed `entries`
    // section every committed measurement would have changed its name when the check was written.
    let corpus = vec![one("analytic/sqrt_domain"), one("synthetic/wide_1d")];
    let plan = Plan {
        budgets: vec![320],
        strategies: vec![aporia_search::Strategy::Adaptive],
        seeds: vec![1],
        ..Plan::default()
    };
    let mut environment = Environment::current();
    environment.notes.clear();
    let doc = results_json(&[], &corpus, &plan, &environment, None);
    assert!(
        doc.get("reachability").is_some(),
        "the document records what the atlas could enclose"
    );
    let without: Vec<(String, Json)> = match &doc {
        Json::Obj(fields) => fields
            .iter()
            .filter(|(name, _)| name != "reachability")
            .cloned()
            .collect(),
        _ => panic!("a results document is an object"),
    };
    let record = Json::Obj(without);
    assert_eq!(
        record
            .get("identity")
            .and_then(Json::as_str)
            .expect("identity"),
        identity_of(
            RESULTS_SCHEMA,
            record.get("plan").expect("plan"),
            record.get("entries").expect("entries"),
        ),
        "the identity is still a digest of the sections it claims to read"
    );
    // And a variant run carries the same section: reachability is arithmetic on the partition, and the
    // trust variants share a depth cap, so the universe of resolvable rows is one list for the whole
    // comparison rather than one per arm — which is what makes the arms comparable at all.
    for variant in Trust::ALL {
        let variant_plan = Plan {
            trust: variant,
            ..plan.clone()
        };
        let variant_doc = results_json(&[], &corpus, &variant_plan, &environment, None);
        assert_eq!(
            variant_doc.get("reachability"),
            doc.get("reachability"),
            "{variant:?} changed which entries can be resolved"
        );
    }
}

/// A schema-2 protocol file that states a resolvable set, written where a test may write.
fn draft_protocol(name: &str, run: &[&str], resolvable: &[&str], sub_granular: &[&str]) -> PathBuf {
    let plan = Json::object(vec![
        ("budgets", Json::Arr(vec![Json::count(8)])),
        (
            "strategies",
            Json::Arr(vec![Json::text("adaptive".to_string())]),
        ),
        ("seeds", Json::Arr(vec![Json::count(1)])),
        ("grid_per_axis", Json::count(41)),
        ("minimise_budget", Json::count(20)),
        ("probe_every", Json::count(7)),
        ("numerical_every", Json::count(0)),
        ("differential_every", Json::count(0)),
        ("symmetric_every", Json::count(1)),
        ("refine_every", Json::count(40)),
        ("calibrate_every", Json::count(25)),
    ]);
    let value = Json::object(vec![
        ("schema", Json::text(Protocol::SCHEMA_V2)),
        ("id", Json::text("reachability-test")),
        (
            "question",
            Json::text("does the load-time refusal fire on the entry it names"),
        ),
        ("plan", plan),
        (
            "arms",
            Json::Arr(vec![Json::object(vec![
                ("name", Json::text("full-current-trust")),
                ("ablate", Json::Arr(Vec::new())),
                ("trust", Json::text(Trust::Current.name())),
            ])]),
        ),
        (
            "entries",
            Json::object(vec![
                (
                    "run",
                    Json::Arr(run.iter().map(|n| Json::text(*n)).collect()),
                ),
                (
                    "resolvable",
                    Json::Arr(resolvable.iter().map(|n| Json::text(*n)).collect()),
                ),
                (
                    "sub_granular",
                    Json::Arr(sub_granular.iter().map(|n| Json::text(*n)).collect()),
                ),
            ]),
        ),
        (
            "primary_metric",
            Json::object(vec![
                ("name", Json::text("over-claim among the resolvable")),
                ("universe", Json::text("the resolvable rows")),
                (
                    "definition",
                    Json::text("trusted volume that overlaps truth"),
                ),
                (
                    "decision_rule",
                    Json::Arr(vec![Json::text("1. if the set is empty, stop")]),
                ),
            ]),
        ),
    ]);
    let dir = std::env::temp_dir().join(format!("aporia-reach-{}-{name}.d", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a temporary protocol directory");
    let path = dir.join("protocol.json");
    std::fs::write(&path, value.to_pretty()).expect("the protocol is written");
    path
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("aporia-reach-{}-{name}.d", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a temporary directory");
    dir
}

/// What the class each entry was given must say about the leaves it found, region by region.
///
/// The two halves are mirrors, and both are needed: an enclosed entry that produced no witness is a
/// claim with nothing behind it, and an entry that disclaimed resolvability and then found a witness
/// has been re-filed by a change to the cap rather than by its own geometry.
fn witnesses_agree_with_the_class(record: &Reach) {
    for region in &record.regions {
        if record.class == Class::Enclosed {
            assert!(
                region.witness_depth.is_some() && !region.witness_bounds.is_empty(),
                "{} region {} is enclosed without a witness to check",
                record.entry,
                region.index
            );
            assert!(
                region.best_declared_share >= aporia_bench::metrics::LOCALISATION_BAR,
                "{} region {} does not reach the bar it was called enclosed on",
                record.entry,
                region.index
            );
        } else {
            // The disclaiming entry has to fail the way 0038's edge entries failed: no witness at all,
            // so there is no leaf for a resolution rule to have recovered.
            assert!(
                region.witness_depth.is_none() && region.witness_bounds.is_empty(),
                "{} region {} found a witness and did not change class",
                record.entry,
                region.index
            );
            assert!(
                region.best_declared_share < LOCALISATION_BAR,
                "{} region {} reaches the bar on its declared box after all: {}",
                record.entry,
                region.index,
                region.best_declared_share
            );
        }
        assert!(
            !region.reason.is_empty(),
            "{} region {} gives no reason",
            record.entry,
            region.index
        );
    }
}

/// The reasoning each entry in this family was authored to carry: no entry may reach the set by
/// leaving its own account out, because the set is a claim about the instrument and not about a run.
fn record_is_stated(entry: &corpus::Entry) {
    let id = entry.id();
    assert!(
        !entry.truth.mechanism.is_empty(),
        "{id} states no mechanism"
    );
    assert!(
        !entry.truth.resolvability.is_empty(),
        "{id} states no calculation"
    );
    assert!(
        !entry.truth.source.is_empty(),
        "{id} states no truth source"
    );
    assert!(!entry.truth.meaning.is_empty(), "{id} states no meaning");
    assert!(
        !entry.truth.why.is_empty(),
        "{id} states no reason to be here"
    );
    assert!(
        !entry.truth.channels.is_empty() && !entry.truth.variables.is_empty(),
        "{id} declares no expected channels or no variables"
    );
}

#[test]
fn the_granular_family_is_enclosed_except_for_the_one_entry_that_declares_otherwise() {
    // The corpus this experiment will measure, pinned against the instrument rather than against
    // anybody's reading of it. Two things are checked at once: that each entry's class comes from the
    // atlas's own partition rule and the truth language's own overlap measure, and that the entry's own
    // `resolvable` claim agrees. The agreement has to hold in both directions, because both directions
    // are a way for the corpus to lie: an entry that claimed resolvability the arithmetic denies is
    // refused from the set rather than reclassified, and an entry that disclaims it may not be quietly
    // re-filed as reachable by a later change to the depth cap.
    let corpus = entries();
    let policy = policy();
    let family: Vec<&corpus::Entry> = corpus.iter().filter(|e| e.family == "granular").collect();
    let mut enclosed = 0usize;
    let mut beyond = 0usize;
    for entry in &family {
        let record = reach(entry, &policy);
        assert_eq!(
            entry.truth.resolvable == Some(true),
            record.class.resolvable(),
            "{} claims {:?} and the instrument found {}",
            record.entry,
            entry.truth.resolvable,
            record.row()
        );
        assert_eq!(
            record.regions.len(),
            entry.truth.regions.len(),
            "{} classified a different number of regions than it declares",
            record.entry
        );
        match record.class {
            Class::Enclosed => enclosed += 1,
            Class::SubGranular => beyond += 1,
            other => panic!(
                "{} is {} rather than enclosed or beyond reach: {}",
                record.entry,
                other.name(),
                record.row()
            ),
        }
        witnesses_agree_with_the_class(&record);
        record_is_stated(entry);
    }
    // Five entries when E1.3b froze its resolvable set, six since E1.4 authored `granular/fit_band` into
    // the same family, ten since E1.5 authored four more. The count is the family's, not one freeze's,
    // and the split is the new part: an entry can now be *filed* as beyond reach on purpose, so the
    // check has to say which classes it expected rather than assume the family is all of one class.
    assert_eq!(
        family.len(),
        10,
        "the granular family is E1.3b's five, E1.4's band and E1.5's four"
    );
    assert_eq!(enclosed, 9, "nine of them are enclosed by a permitted leaf");
    assert_eq!(
        beyond, 1,
        "exactly one granular entry declares itself beyond reach, and it is the entry E1.5 authored to \
         price the depth cap: granular/thin_reach"
    );
}

#[test]
fn the_control_half_of_the_corpus_stays_unresolvable() {
    // The sub-granular set E1.3b froze as its control: the entries whose over-claim 0038 explained by
    // granularity. If one of them became enclosed, the account being tested would already be wrong, and
    // this is where that would show up — before the run rather than in its interpretation.
    for (id, class) in [
        ("edge/overflow_tail", Class::SubGranular),
        ("edge/pole_at_edge", Class::SubGranular),
        ("geometry/narrow_oblique", Class::CarvedAway),
    ] {
        let record = reach(&one(id), &policy());
        assert_eq!(record.class, class, "{}: {}", record.entry, record.row());
        assert!(!record.class.resolvable());
        assert_eq!(
            record.regions.len(),
            one(id).truth.regions.len(),
            "{id} was not classified region by region"
        );
        for region in &record.regions {
            assert!(
                region.witness_bounds.is_empty(),
                "{id} region {} found a witness and did not change class",
                region.index
            );
        }
    }
}

#[test]
fn a_resolvable_claim_the_atlas_cannot_keep_is_refused_before_anything_runs() {
    // The refusal E1.3's condition needed. `edge/pole_at_edge` is sub-granular, so a protocol that
    // counts it among the resolvable rows is asking a trust rule to fix a partition limit, and the run
    // has to stop rather than produce a row that would read as a safety result either way.
    let protocol = draft_protocol(
        "refused",
        &["edge/pole_at_edge", "analytic/sqrt_domain"],
        &["edge/pole_at_edge", "analytic/sqrt_domain"],
        &[],
    );
    let out = scratch("refused-out");
    let error = cli::run_run(
        "aporia-bench",
        &[
            "--plan".to_string(),
            protocol.display().to_string(),
            "--arm".to_string(),
            "full-current-trust".to_string(),
            "--out".to_string(),
            out.display().to_string(),
            "--archive".to_string(),
            scratch("refused-archives").display().to_string(),
        ],
    )
    .expect_err("a claimed-resolvable entry the atlas cannot enclose is not a runnable plan");
    assert!(
        error.contains("edge/pole_at_edge"),
        "the refusal names the entry: {error}"
    );
    assert!(
        error.contains("sub-granular"),
        "and the class it computed: {error}"
    );
    assert!(
        error.contains("0.5") || error.contains("bar"),
        "and the bar it was measured against: {error}"
    );
    assert!(
        !std::fs::read_dir(&out).is_ok_and(|mut d| d.next().is_some()),
        "a refused run wrote a results file anyway"
    );
    let _ = std::fs::remove_dir_all(&out);
    let _ = std::fs::remove_file(&protocol);
    // The protocol's own directory, and nothing else: sibling tests are still running in it, and a
    // removal that succeeds only when empty is the one that cannot destroy their files.
    let _ = protocol.parent().map(std::fs::remove_dir);
}

#[test]
fn a_claim_the_instrument_disagrees_with_is_recorded_and_the_run_goes_on() {
    // The other direction is not a refusal. A protocol that expects `analytic/sqrt_domain` to be
    // unresolvable and the atlas encloses it has been contradicted about its own control set, which is
    // news about the expectation — and refusing the run would let the expectation decide what gets
    // measured. The disagreement therefore lands in the document, where a reader of the rows finds it.
    let protocol = draft_protocol(
        "noted",
        &["analytic/sqrt_domain", "synthetic/wide_1d"],
        &["synthetic/wide_1d"],
        &["analytic/sqrt_domain"],
    );
    let out = scratch("noted-out");
    let archives = scratch("noted-archives");
    let status = cli::run_run(
        "aporia-bench",
        &[
            "--plan".to_string(),
            protocol.display().to_string(),
            "--arm".to_string(),
            "full-current-trust".to_string(),
            "--out".to_string(),
            out.display().to_string(),
            "--archive".to_string(),
            archives.display().to_string(),
        ],
    )
    .expect("a disagreeing expectation is reported, not refused");
    assert_eq!(status, 0);
    let written: Vec<_> = std::fs::read_dir(&out)
        .expect("the results directory")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    assert_eq!(written.len(), 1, "one run, one document");
    let text = std::fs::read_to_string(&written[0]).expect("the document is readable");
    let doc = Json::parse(&text).expect("and parses");
    let section = doc
        .get("reachability")
        .and_then(Json::as_array)
        .expect("the document records reachability");
    assert_eq!(section.len(), 2, "one record per covered entry");
    let noted = section
        .iter()
        .find(|r| r.get("entry").and_then(Json::as_str) == Some("analytic/sqrt_domain"))
        .expect("the entry the file expected unresolvable");
    assert_eq!(
        noted.get("class").and_then(Json::as_str),
        Some(Class::Enclosed.name()),
        "the document does not record what the instrument really found"
    );
    // The protocol identity travels with the rows, so this run cites the file whose expectation it
    // contradicted rather than leaving a reader to guess which definition produced them.
    assert_eq!(
        doc.get("protocol")
            .and_then(|p| p.get("id"))
            .and_then(Json::as_str),
        Some("reachability-test")
    );
    for path in [&written[0], &protocol] {
        let _ = std::fs::remove_file(path);
    }
    for dir in [&out, &archives] {
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[test]
fn a_protocol_that_splits_its_corpus_incompletely_is_refused_by_the_reader() {
    // The lists are the universe of the deciding metric, so they have to cover the run set exactly: an
    // entry in neither list is a row whose side of the comparison would be chosen by whoever read the
    // file after the freeze.
    let mut caught = 0usize;
    for (name, run, resolvable, sub_granular) in [
        // An entry named in `run` and in neither set.
        (
            "uncovered",
            vec!["analytic/sqrt_domain", "synthetic/wide_1d"],
            vec!["analytic/sqrt_domain"],
            vec![],
        ),
        // An entry in both sets, so it would be counted in the deciding universe and in its control.
        (
            "overlap",
            vec!["analytic/sqrt_domain"],
            vec!["analytic/sqrt_domain"],
            vec!["analytic/sqrt_domain"],
        ),
        // A resolvable set with nothing in it, which is decision rule 1 arriving as a plan.
        (
            "empty",
            vec!["analytic/sqrt_domain"],
            vec![],
            vec!["analytic/sqrt_domain"],
        ),
    ] {
        let path = draft_protocol(name, &run, &resolvable, &sub_granular);
        let error =
            Protocol::load(&path).expect_err("an incomplete split of the corpus is not a plan");
        assert!(
            error.contains("entries."),
            "{name}: the refusal must name a section: {error}"
        );
        caught += 1;
        let _ = std::fs::remove_file(&path);
        let _ = path.parent().map(std::fs::remove_dir);
    }
    assert_eq!(caught, 3, "each incomplete split was caught");
}

#[test]
fn a_thin_carve_is_judged_by_the_leaves_that_could_hold_it_not_by_the_first_ones_indexed() {
    // A capped search ranks, and the ranking is the class. Until the defect this test pins was fixed the
    // ranking was the region's envelope share alone, and for any entry that declares its box as the whole
    // domain that ties every leaf in the partition together: the 32 leaves actually asked were the 32 the
    // tree created first, so a verdict about "could a leaf be half inside" was read off a cell four
    // bisections from the root. Measured on the first E1.4 entry authored against that classifier — a
    // diagonal band 0.0075 mm wide, inside leaves 0.00625 mm across, so a leaf at the depth cap sits
    // wholly inside it — the entry came out `carved-away` with best declared share 0.150391.
    let band = reach(&one("granular/fit_band"), &policy());
    assert_eq!(band.class, Class::Enclosed, "{}", band.row());
    let region = band
        .regions
        .first()
        .expect("the fit band declares one region");
    assert!(
        region.best_declared_share >= LOCALISATION_BAR,
        "the witness has to be a leaf at least half inside the band: {}",
        region.best_declared_share
    );
    assert_eq!(
        region.witness_depth,
        Some(12),
        "the leaf that decides this is at the depth cap, which is the whole point of ranking finest \
         first; a shallow witness would be the old behaviour wearing a new number"
    );
    // The fix must re-file nothing that has already been measured. These are the classes the committed
    // protocols and rows cite: the two sub-granular edge entries, the carved-away oblique band, and the
    // three resolvable entries whose witnesses are coarse cells a finest-first ranking could have lost.
    for (id, class) in [
        ("geometry/narrow_oblique", Class::CarvedAway),
        ("edge/overflow_tail", Class::SubGranular),
        ("edge/pole_at_edge", Class::SubGranular),
        ("granular/tolerance_stack", Class::Enclosed),
        ("granular/rail_ceiling", Class::Enclosed),
        ("geometry/curved_product", Class::Enclosed),
    ] {
        let record = reach(&one(id), &policy());
        assert_eq!(
            record.class,
            class,
            "{id} was re-filed by the ranking fix: {}",
            record.row()
        );
    }
}
