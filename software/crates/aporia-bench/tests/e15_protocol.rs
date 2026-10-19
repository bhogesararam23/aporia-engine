//! Read the E1.5 freeze the way its own decision rule will be read, and check it can be read.
//!
//! A pre-registration fails in two ways. It can name a quantity the instrument does not record, in
//! which case the analysis invents it or the run stops; or it can leave a combination of readings with
//! no outcome, which is exactly what E1.4 did — two arms cut the primary metric to zero, held every
//! other guard, gained nothing and breached one ceiling, and the freeze had written no outcome for
//! that shape, so the decision came back `fired: null` and the gap went into the defect list. These
//! tests check both properties mechanically, against the recorded field names this build actually
//! writes rather than against a list of them kept up by hand.

use aporia_bench::corpus_root;
use aporia_bench::harness::{Protocol, plan_json};
use aporia_store::Json;
use std::path::PathBuf;

const ID: &str = "e1-5-spatial-uncertainty-cost";

fn protocol() -> Protocol {
    Protocol::load(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../benchmarks/protocols")
            .join(format!("{ID}.json")),
    )
    .expect("the E1.5 freeze reads")
}

fn raw() -> Json {
    let text = std::fs::read_to_string(corpus_root().join("protocols").join(format!("{ID}.json")))
        .expect("the E1.5 freeze is committed");
    Json::parse(&text).expect("the freeze is JSON")
}

/// Every field name this build can write into a results document: row columns, sweep summaries and
/// top-level sections.
///
/// The row keys come from a row this build measured, because a list of them maintained by hand is a
/// second definition of the schema and drifts from the first in exactly the way this audit exists to
/// catch. The sweep and document keys come from a committed document, so a rule that names a
/// per-seed figure rather than a per-row one is still checked against bytes a reader can open.
fn recorded_fields() -> Vec<String> {
    let entries = aporia_bench::corpus::load(&corpus_root()).expect("the corpus loads");
    let entry = entries
        .iter()
        .find(|e| e.id() == "control/symplectic_spring")
        .expect("the registered control is in the corpus");
    let campaign = aporia_search::run(
        entry.model.as_ref().expect("the entry compiles"),
        aporia_search::Config {
            budget: 320,
            ..aporia_search::Config::default()
        },
    );
    let outcome = aporia_bench::metrics::Outcome::measure(
        entry,
        &campaign,
        0,
        aporia_search::Strategy::Adaptive,
        320,
    );
    let mut fields = obj_keys(&outcome.to_json());
    // A committed document, so the sweep-level and document-level names a reader can open are checked
    // against the bytes that carry them rather than against this build's idea of them.
    let document = {
        let path = corpus_root()
            .join("results")
            .join("results-bf3964c8d2d9.json");
        let text = std::fs::read_to_string(path).expect("the E1.4 reference document is committed");
        Json::parse(&text).expect("it is JSON")
    };
    fields.extend(obj_keys(&document));
    for sweep in document
        .get("sweeps")
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
    {
        for key in obj_keys(sweep) {
            if key != "outcomes" {
                fields.push(key);
            }
        }
    }
    fields.sort();
    fields.dedup();
    fields
}

fn obj_keys(value: &Json) -> Vec<String> {
    match value {
        Json::Obj(fields) => fields.iter().map(|(key, _)| key.clone()).collect(),
        _ => Vec::new(),
    }
}

/// The names the freeze itself defines, which are quantities rather than columns: the primary metric,
/// the cost ratios, and the derived volumes its own text introduces.
const DECLARED_QUANTITIES: [&str; 8] = [
    "certified_over_region_share",
    "blindness_per_overclaim_removed",
    "clean_certification_withdrawal",
    "clean_certified",
    "clean_certified_volume",
    "silence_floor",
    "false_suspicion",
    "reduction",
];

/// The words a protocol file backticks that are not measurements at all: schemas, flags, states and
/// the names of the quantities the freeze defines in its own text.
const NOT_FIELDS: [&str; 13] = [
    "aporia.protocol/3",
    "aporia.protocol/4",
    "reach",
    "risk_max",
    "refine",
    "where",
    "not_applicable",
    "not_measurable",
    "no_benefit",
    "capped",
    "retention",
    "zero_benefit",
    "what_was_already_measured",
];

fn snake_case_in(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    // Skip, *then* step: the quoted spans are at indices 1, 3, 5 of a backtick split, and the other
    // order walks the prose between them — which would let this audit pass while reading the halves
    // of sentences nobody quoted. The parity is asserted by the caller for the same reason.
    for span in text
        .split('`')
        .collect::<Vec<_>>()
        .iter()
        .skip(1)
        .step_by(2)
        .flat_map(|quoted| quoted.split_whitespace().collect::<Vec<_>>())
    {
        let word: String = span
            .chars()
            .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '_')
            .collect();
        let looks_like_a_field = word.contains('_')
            && word.len() > 4
            && span.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if looks_like_a_field && !out.contains(&word) {
            out.push(word);
        }
    }
    out
}

#[test]
fn the_freeze_reads_under_the_schema_it_declares_and_names_its_own_bytes() {
    let protocol = protocol();
    assert_eq!(protocol.schema, Protocol::SCHEMA_V4);
    assert_eq!(protocol.id, ID);
    let digest = protocol.digest.clone().expect("a loaded file has a digest");
    // The digest the four arm documents will cite. Pinned as a value because the point of the field is
    // that an edit after the run is visible: a test that only re-derived it from the same bytes could
    // never notice the file having changed under them. The pinned value has moved once, when the corpus
    // this file named contradicted three of its sentences; `amended_before_running` is the record of
    // that, every change was prose, and the assertions below hold the bars and the arm set still.
    assert_eq!(digest, "4bfa6855279d165f", "the freeze's bytes changed");
    assert_eq!(protocol.arms.len(), 4);
    assert_eq!(protocol.plan.certify_density, 1_024.0);
    assert_eq!(protocol.plan.certify_share, 0.003_906_25);
    // A `/4` file whose arms moved the bars would be several experiments; the plan states them once.
    for arm in &protocol.arms {
        let (plan, _) = protocol.arm(&arm.name).expect("an arm names itself");
        assert_eq!(plan.certify_share, protocol.plan.certify_share);
        assert_eq!(plan.certify_density, protocol.plan.certify_density);
        assert_eq!(plan.trust, aporia_boundary::Trust::Current, "{}", arm.name);
        assert!(plan.ablate.is_empty(), "{} ablates", arm.name);
    }
    // Four arms, four measurements: the plan sections have to differ, or two of the names are one
    // measurement written twice and the runner's duplicate refusal is the only thing saying so.
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
        "the four arms do not name four measurements"
    );
    // A-current is the committed semantics: its plan section carries no axis at all, which is what lets
    // it reproduce the published identities rather than claiming a fifth experiment.
    assert!(
        !sections[0].contains("resolution")
            && !sections[0].contains("certify")
            && !sections[0].contains("trust"),
        "the control arm wrote an axis into its own plan section: {}",
        sections[0]
    );
}

#[test]
fn every_quantity_the_decision_rule_reads_is_a_field_this_build_records() {
    // The audit that E1.4's uncomputable falsifier was asking for. Every backticked snake_case name in
    // the deciding text has to be a column this binary writes, a plan field the document records, a
    // quantity the freeze defines in its own text, or a word that is not a measurement. Anything else
    // is a rule that cannot be evaluated from the rows, which is the defect this phase was written to
    // close and must not be written into the next freeze.
    let recorded = recorded_fields();
    let document = raw();
    let mut plan_fields: Vec<String> = Vec::new();
    if let Some(Json::Obj(fields)) = document.get("plan") {
        plan_fields = fields.iter().map(|(key, _)| key.clone()).collect();
    }
    let metric = document
        .get("primary_metric")
        .expect("the freeze states a primary metric");
    let mut text = String::new();
    for key in [
        "definition",
        "denominator",
        "zero_denominator",
        "aggregation",
        "exactness",
    ] {
        text.push_str(metric.get(key).and_then(Json::as_str).unwrap_or_default());
        text.push(' ');
    }
    if let Some(Json::Obj(rules)) = metric.get("cost_rules") {
        for (_, value) in rules {
            text.push_str(value.as_str().unwrap_or_default());
            text.push(' ');
        }
    }
    for outcome in metric
        .get("decision_rule")
        .and_then(Json::as_array)
        .expect("the freeze enumerates outcomes")
    {
        text.push_str(outcome.as_str().unwrap_or_default());
        text.push(' ');
    }
    assert_eq!(
        text.split('`').count() % 2,
        1,
        "the deciding text has an odd number of backticks, so which spans are quoted at all is ambiguous and this audit would read the wrong halves of the sentences"
    );
    let mut unexplained: Vec<String> = Vec::new();
    for name in snake_case_in(&text) {
        if recorded.contains(&name)
            || plan_fields.contains(&name)
            || DECLARED_QUANTITIES.contains(&name.as_str())
            || NOT_FIELDS.contains(&name.as_str())
        {
            continue;
        }
        unexplained.push(name);
    }
    assert!(
        unexplained.is_empty(),
        "the deciding text reads quantities no row records: {unexplained:?}\nrecorded: {}",
        recorded.join(", ")
    );
}

#[test]
fn the_decision_rule_numbers_its_outcomes_in_order_and_covers_every_shape() {
    let document = raw();
    let outcomes = document
        .get("primary_metric")
        .and_then(|m| m.get("decision_rule"))
        .and_then(Json::as_array)
        .expect("the freeze enumerates outcomes");
    let mut numbers = Vec::new();
    for outcome in outcomes {
        let text = outcome.as_str().expect("an outcome is a sentence");
        let number: String = text.chars().take_while(char::is_ascii_digit).collect();
        assert!(
            !number.is_empty(),
            "an outcome has to be addressable by number so a report can name which one fired: {text}"
        );
        numbers.push(
            number
                .parse::<u32>()
                .expect("the outcome number parses as an integer"),
        );
    }
    let mut ordered = numbers.clone();
    ordered.sort_unstable();
    assert_eq!(
        ordered, numbers,
        "the outcomes are not in the order they are evaluated in"
    );
    assert_eq!(
        numbers.first(),
        Some(&1),
        "the rule starts at 1 so a report can say which outcome fired without a legend"
    );
    // The shapes a deciding comparison can produce, and the outcome each lands on. This is the check
    // E1.4's list failed: {metric falls, blindness rises, certification falls, nothing changes} has to
    // be exhausted, and the arm E1.4 produced - metric to zero, other guards held, no localisation
    // gain, ceiling breached - must not be homeless here.
    let shapes = [
        "no certified over-claim anywhere",
        "too few measurable rows",
        "certifies almost nothing",
        "the metric does not fall",
        "the metric falls and the cost exceeds the benefit",
        "the metric falls at an affordable cost with detection gained",
        "the metric falls at an affordable cost with nothing gained",
        "every paired difference is zero",
    ];
    let names = ["1", "2", "3", "4", "5", "6", "7", "8"];
    for (shape, name) in shapes.iter().zip(names.iter()) {
        let found = outcomes
            .iter()
            .filter_map(Json::as_str)
            .any(|text| text.starts_with(name));
        assert!(found, "no outcome is numbered {name}, which {shape} needs");
    }
    // 9 and 10 are the hypothesis tests, evaluated whatever fired, and both are named so a report
    // cannot print an improvement without them.
    for name in ["9", "10"] {
        assert!(
            outcomes
                .iter()
                .filter_map(Json::as_str)
                .any(|text| text.starts_with(name)),
            "outcome {name} is missing, so a hypothesis this freeze promised to test would go untested"
        );
    }
}

#[test]
fn the_pilot_scope_is_a_subset_of_the_experiment_and_the_sets_cover_the_run_list() {
    let protocol = protocol();
    let pilot = protocol.pilot.clone().expect("a /4 file freezes a pilot");
    for name in &pilot.arms {
        assert!(
            protocol.arms.iter().any(|arm| &arm.name == name),
            "the pilot names {name}, which the file does not arm"
        );
    }
    for name in &pilot.entries {
        assert!(
            protocol.entries.contains(name),
            "the pilot names {name}, which is not in entries.run"
        );
    }
    for budget in &pilot.budgets {
        assert!(
            protocol.plan.budgets.contains(budget),
            "pilot budget {budget}"
        );
    }
    for seed in &pilot.seeds {
        assert!(protocol.plan.seeds.contains(seed), "pilot seed {seed}");
    }
    assert!(
        pilot.budgets.len() < protocol.plan.budgets.len()
            && pilot.seeds.len() < protocol.plan.seeds.len()
            && pilot.entries.len() < protocol.entries.len(),
        "a pilot as wide as its experiment is the defect this schema was written to close"
    );
    // And the charge is stated where a reader can see it, because E1.4's pilot ran 100 cells while its
    // stopping rule described 16.
    let cells = pilot.arms.len() * pilot.entries.len() * pilot.budgets.len() * pilot.seeds.len();
    assert_eq!(cells, 16, "the frozen pilot scope is {cells} charged cells");

    let entries = raw().get("entries").expect("entries is a section").clone();
    let list = |key: &str| -> Vec<String> {
        entries
            .get(key)
            .and_then(Json::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Json::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    };
    let run = list("run");
    let covered: Vec<String> = [list("resolvable"), list("sub_granular"), list("control")].concat();
    assert_eq!(
        run.len(),
        covered.len(),
        "an entry is in none of the three sets, or in more than one"
    );
    for entry in &run {
        assert_eq!(
            covered.iter().filter(|name| *name == entry).count(),
            1,
            "{entry} is not in exactly one deciding set"
        );
    }
    assert!(
        !list("control").is_empty(),
        "a freeze with no control cannot price uncertainty"
    );
    assert!(
        !list("resolvable").is_empty(),
        "a freeze with no resolvable entry cannot test resolution"
    );
}

#[test]
fn the_freeze_names_its_own_bytes_and_the_corpus_answers_every_entry_it_named() {
    // The freeze was committed while all four of its new entries existed only as names in this file —
    // `tests/e15_protocol.rs` at that commit asserted exactly that. They are authored now, so the
    // durable check is the one that outlives the ordering: the run list the freeze names is the list
    // the loader reads, every entry it declares resolvable is enclosed by the atlas's own arithmetic,
    // and the one it declares sub-granular is the one it says is not.
    let protocol = protocol();
    let entries = aporia_bench::corpus::load(&corpus_root()).expect("the corpus loads");
    let missing: Vec<&String> = protocol
        .entries
        .iter()
        .filter(|name| !entries.iter().any(|e| e.id() == **name))
        .collect();
    assert!(
        missing.is_empty(),
        "the freeze names {missing:?}, which the corpus lacks"
    );
    let policy = aporia_boundary::Policy::default();
    for name in &protocol.resolvable {
        let entry = entries
            .iter()
            .find(|e| &e.id() == name)
            .unwrap_or_else(|| panic!("{name} is in the freeze's resolvable set"));
        let record = aporia_bench::reach::reach(entry, &policy);
        assert!(
            record.class.resolvable(),
            "{} is listed resolvable and the atlas says {}",
            name,
            record.class.name()
        );
    }
    for name in &protocol.sub_granular {
        let entry = entries
            .iter()
            .find(|e| &e.id() == name)
            .unwrap_or_else(|| panic!("{name} is in the freeze's sub-granular set"));
        let record = aporia_bench::reach::reach(entry, &policy);
        assert!(
            !record.class.resolvable(),
            "{} is listed sub-granular and the atlas encloses it: the entry's own claim has to agree with the arithmetic either way, and it says {}",
            name,
            record.class.name()
        );
    }
    // The control set is the one the corpus contract pins hardest: an entry named there declares no
    // region, which is what makes its withheld and suspicious volume a cost rather than a result.
    for name in &protocol.control {
        let entry = entries
            .iter()
            .find(|e| &e.id() == name)
            .expect("the freeze's control is in the corpus");
        assert!(
            entry.truth.control && entry.truth.regions.is_empty(),
            "{name} is named a control and does not declare nothing"
        );
    }
    // Every entry this experiment *authored* is box-declared, which is what the corrected exactness
    // clause says: the union has to be a sum of widths for the four label columns to add up to the
    // denominator the metric divides by. The carried-over entries are not all boxes, and the clause
    // names that in the freeze rather than the corpus being edited to fit it.
    let authored = [
        "granular/quiet_band",
        "granular/mixed_cell",
        "granular/thin_reach",
        "granular/semantic_step",
    ];
    let mut carved: Vec<String> = Vec::new();
    for name in &protocol.entries {
        let entry = entries
            .iter()
            .find(|e| &e.id() == name)
            .expect("the freeze's entries are in the corpus");
        if entry
            .truth
            .regions
            .iter()
            .any(|region| region.predicate.is_some())
        {
            carved.push(name.clone());
        }
        assert!(
            !(authored.contains(&name.as_str()) && carved.last() == Some(name)),
            "{name} was authored for E1.5 and carves a `where`, so its decomposition is a lattice \
             estimate and cannot carry the metric"
        );
    }
    assert_eq!(
        carved,
        vec!["granular/tolerance_stack".to_string()],
        "the freeze's exactness clause names the carved entries in `entries.run`, so the list is a \
         claim a test can check"
    );
}

/// Recompute the primary metric from the artifact's own rows: every row's recorded share must be the
/// quotient of the two columns the frozen definition divides, and the mean of those quotients is what
/// the summary and the amendment both quote. Returns the mean and how many rows carry it.
fn shares_of(rows: &[Json], smallest: u64) -> (f64, usize) {
    let mut sum = 0.0f64;
    let mut nonzero = 0usize;
    for row in rows {
        let number = |key: &str| {
            row.get(key)
                .and_then(Json::as_f64)
                .unwrap_or_else(|| panic!("a row states no {key}"))
        };
        let (numerator, denominator, refusals) = (
            number("trusted_over_true_union"),
            number("declared_union_volume"),
            row.get("region_overlap_refusals")
                .and_then(Json::as_u64)
                .expect("a row states its refusals"),
        );
        assert_eq!(
            refusals, 0,
            "a deciding row came back refused, so the metric's own `not_measurable` branch would be              averaging a set this artifact does not have"
        );
        assert!(
            denominator > 0.0,
            "the deciding entry declares no region, which its record contradicts"
        );
        let share = numerator / denominator;
        assert!(
            (share - number("share")).abs() <= 1e-12,
            "a row's recorded share is not the quotient of the two columns the frozen metric divides"
        );
        sum += share;
        if share > 0.0 {
            nonzero += 1;
            assert_eq!(
                row.get("budget").and_then(Json::as_u64),
                Some(smallest),
                "the amendment says every nonzero row sits at the smallest frozen budget, so a row                  that is nonzero anywhere else means the sentence and the measurement disagree"
            );
        }
    }
    (sum / rows.len() as f64, nonzero)
}

/// The pre-run check the freeze's amendment quotes, read back from the committed artifact.
///
/// A sentence in a pre-registration that says "and this was measured" is worth nothing unless the
/// measurement can be opened again. So the artifact is checked against the freeze rather than
/// beside it: the digest it cites is the freeze's current bytes, the plan it measured under is the
/// plan this file still carries, every row's share is recomputed from the two columns the frozen
/// metric divides, and the mean the prose quotes is the mean the rows produce.
#[test]
fn the_prerun_check_the_freeze_quotes_is_the_one_the_artifact_carries() {
    let path = corpus_root()
        .join("analysis")
        .join("e1-5-deciding-entry-prerun.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let artifact = Json::parse(&text).expect("the pre-run artifact is JSON");
    let protocol = protocol();
    let digest = protocol.digest.clone().expect("a loaded file has a digest");

    let cited = artifact
        .get("protocol")
        .and_then(|p| p.get("digest"))
        .and_then(Json::as_str)
        .expect("the artifact cites the freeze by digest");
    assert_eq!(
        cited, digest,
        "the artifact cites {cited} while the tree holds {digest}: either the check is stale or the \
         freeze moved after the measurement it quotes"
    );

    // The measurement was made under the frozen plan, not under a plan chosen for it.
    let plan = artifact.get("plan").expect("the artifact records its plan");
    assert_eq!(
        plan.get("grid_per_axis").and_then(Json::as_u64),
        Some(protocol.plan.grid as u64)
    );
    assert_eq!(
        plan.get("certify_share").and_then(Json::as_f64),
        Some(protocol.plan.certify_share)
    );
    assert_eq!(
        plan.get("certify_density").and_then(Json::as_f64),
        Some(protocol.plan.certify_density)
    );
    let budgets: Vec<u64> = plan
        .get("budgets")
        .and_then(Json::as_array)
        .expect("budgets")
        .iter()
        .map(|b| b.as_u64().expect("a budget is a number"))
        .collect();
    assert_eq!(budgets, protocol.plan.budgets);

    let rows = artifact.get("rows").and_then(Json::as_array).expect("rows");
    assert_eq!(
        rows.len(),
        protocol.plan.budgets.len() * protocol.plan.seeds.len(),
        "the artifact carries one row per (budget, seed) of the frozen plan"
    );
    let smallest = *protocol
        .plan
        .budgets
        .iter()
        .min()
        .expect("a plan has budgets");
    let (mean, nonzero) = shares_of(rows, smallest);
    let stated = artifact
        .get("prediction")
        .and_then(|p| p.get("measured_mean"))
        .and_then(Json::as_f64)
        .expect("the artifact states the mean it measured");
    assert!(
        (mean - stated).abs() <= 1e-12,
        "the artifact's summary is {stated} and its rows say {mean}"
    );
    assert_eq!(
        artifact
            .get("prediction")
            .and_then(|p| p.get("holds"))
            .and_then(Json::as_bool),
        Some(mean >= 0.05),
        "the entry's design claim is checked against the 0.05 the freeze wrote, not against a number \
         this test brought with it"
    );
    assert_eq!(
        artifact
            .get("summary")
            .and_then(|s| s.get("rows_with_nonzero_share"))
            .and_then(Json::as_u64),
        Some(nonzero as u64),
        "the tally of rows carrying the whole effect is stated twice and disagrees"
    );

    // And the amendment's sentence quotes that number rather than a rounded memory of it.
    let amendment = raw()
        .get("amended_before_running")
        .and_then(Json::as_str)
        .expect("the freeze records its amendments")
        .to_string();
    let quoted = format!("{mean}");
    assert!(
        amendment.contains(&quoted),
        "the freeze quotes a mean the rows do not produce: it says {quoted} where the artifact's \
         rows say {mean}"
    );
}
