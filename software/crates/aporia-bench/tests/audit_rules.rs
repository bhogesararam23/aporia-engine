//! The corpus audit, tested by making it refuse things.
//!
//! A gate that has only ever printed "clean" is unmeasured: it might be checking nothing. Every rule
//! here is fired on purpose against a corpus written into a temporary directory, and the committed
//! corpus is asserted to pass the same set. Fixtures are raw strings, because a JSON literal written
//! with escaped quotes inside an escaped string is a fixture nobody can review.

use aporia_bench::audit;
use aporia_bench::corpus;
use std::path::{Path, PathBuf};

/// A corpus of exactly these entries, read through the real loader so a fixture is parsed the same way
/// a published entry is.
fn load(corpus_dir: &Path, entries: &[(&str, &str, &str)]) -> Vec<corpus::Entry> {
    let mut by_family: Vec<(String, Vec<String>)> = Vec::new();
    for (id, _, _) in entries {
        let (family, name) = id.split_once('/').expect("fixture ids are family/name");
        if let Some((_, names)) = by_family.iter_mut().find(|(f, _)| f == family) {
            names.push(name.to_string());
        } else {
            by_family.push((family.to_string(), vec![name.to_string()]));
        }
    }
    let families = by_family
        .iter()
        .map(|(family, names)| {
            let list = names
                .iter()
                .map(|n| format!("\"{n}\""))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{{ \"name\": \"{family}\", \"purpose\": \"fixture\", \"entries\": [{list}] }}")
        })
        .collect::<Vec<_>>()
        .join(",\n");
    std::fs::write(
        corpus_dir.join("registry.json"),
        format!("{{ \"schema\": \"aporia.corpus/1\", \"families\": [\n{families}\n] }}\n"),
    )
    .expect("write the registry");
    for (id, model, truth) in entries {
        let (family, name) = id.split_once('/').expect("fixture ids are family/name");
        let dir = corpus_dir.join(family).join(name);
        std::fs::create_dir_all(&dir).expect("create the entry directory");
        std::fs::write(dir.join("model.ap"), model).expect("write model.ap");
        std::fs::write(dir.join("truth.json"), truth).expect("write truth.json");
    }
    corpus::load(corpus_dir).unwrap_or_else(|e| panic!("the fixture corpus did not load: {e}"))
}

/// A fresh fixture root. Files left by a previous run would be read as part of the corpus, so the
/// directory is emptied rather than reused.
fn workspace(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("aporia-audit-{tag}-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).expect("create the fixture root");
    dir
}

#[must_use]
fn region(reason: &str, axes: &str, tail: &str) -> String {
    format!(r#"{{"reason":"{reason}","axes":{axes}{tail}}}"#)
}

/// A complete `truth.json` for a fixture, one field per line so the claim under test is readable.
#[must_use]
fn truth(regions: &str, tail: &str) -> String {
    format!(
        r#"{{"schema":"aporia.truth/2","method":"analytic","fault":"boundary_condition",
 "derivation":"worked out by hand by the author","regions":[{regions}],"boundaries":[]{tail}}}"#
    )
}

/// The rung fixtures match to: half of the `p` axis, so a quarter of its domain.
const RUNG_SOURCE: &str = r#"model rung "" {
  input p in [0, 4]
  input q in [0, 4]
  let total = p + q
  require total <= 4
}
"#;

fn rung() -> String {
    truth(&region("a quarter", r#"{"p":[2,4],"q":[0,2]}"#, ""), "")
}

/// Fails where 2p + q > 8: a quarter of its domain, the same share as `rung`.
const COPY_SOURCE: &str = r#"model copy "" {
  input p in [0, 4]
  input q in [0, 4]
  let total = 2 * p + q
  require total <= 8
}
"#;

/// Fails nowhere, so a region naming the whole domain is the whole domain.
const EVERYWHERE_SOURCE: &str = r#"model everywhere "" {
  input p in [0, 4]
  let y = p
  require y >= 0
}
"#;

/// Fails where p > 3.
const THRESHOLD_SOURCE: &str = r#"model threshold "" {
  input p in [0, 4]
  let y = p - 3
  require y <= 0
}
"#;

#[test]
fn the_committed_corpus_passes_its_own_audit() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../benchmarks");
    let entries = corpus::load(&root).expect("the committed corpus loads");
    let problems = audit::audit(&entries, 41);
    let report: Vec<String> = problems.iter().map(ToString::to_string).collect();
    assert!(
        problems.is_empty(),
        "the corpus fails its audit: {report:#?}"
    );
    assert!(
        entries.len() > 30,
        "the geometry families have to be present: {}",
        entries.len()
    );
}

#[test]
fn a_renamed_copy_of_an_existing_model_is_refused() {
    // Same computation, different name and prose: it would add a row to every summary and no question.
    let copy = r#"model rung "a shorter note" {
  input p in [0, 4]
  input q in [0, 4]
  let total = p + q
  require total <= 4
}
"#;
    let entries = load(
        &workspace("dupe"),
        &[
            ("synthetic/rung", RUNG_SOURCE, &rung()),
            (
                "geometry/copy",
                copy,
                &truth(
                    &region("r", r#"{"p":[3,4],"q":[0,4]}"#, ""),
                    r#", "matched_to": "synthetic/rung""#,
                ),
            ),
        ],
    );
    let problems = audit::audit(&entries, 41);
    let text = problems
        .iter()
        .map(|p| format!("{}: {}", p.entry, p.detail))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        text.contains("same computation as")
            && text.contains("geometry/copy")
            && text.contains("synthetic/rung"),
        "the duplicate slipped through: {text}"
    );
}

#[test]
fn the_same_declared_region_in_two_entries_is_refused() {
    let shared = region("r", r#"{"p":[3,4],"q":[0,4]}"#, "");
    let entries = load(
        &workspace("dupe-region"),
        &[
            (
                "geometry/first",
                COPY_SOURCE,
                &truth(&shared, r#", "matched_to": "geometry/second""#),
            ),
            (
                "geometry/second",
                THRESHOLD_SOURCE,
                &truth(&shared, r#", "matched_to": "geometry/first""#),
            ),
        ],
    );
    let problems = audit::audit(&entries, 41);
    assert!(
        problems.iter().any(|p| p.detail.contains("counts one")),
        "one region declared twice was accepted: {problems:?}"
    );
}

#[test]
fn a_new_entry_without_a_rung_is_refused_and_so_is_a_wrong_one() {
    let no_rung = truth(&region("r", r#"{"p":[0,4],"q":[0,4]}"#, ""), "");
    let wrong_rung = truth(
        &region("all of p", r#"{"p":[0,4]}"#, ""),
        r#", "matched_to": "synthetic/rung""#,
    );
    let entries = load(
        &workspace("rung"),
        &[
            ("synthetic/rung", RUNG_SOURCE, &rung()),
            ("geometry/no_rung", COPY_SOURCE, &no_rung),
            ("geometry/wrong_rung", EVERYWHERE_SOURCE, &wrong_rung),
        ],
    );
    let problems = audit::audit(&entries, 41);
    let text = problems
        .iter()
        .map(|p| format!("{}: {}", p.entry, p.detail))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        text.contains("geometry/no_rung") && text.contains("no `matched_to`"),
        "the missing rung was accepted: {text}"
    );
    assert!(
        text.contains("geometry/wrong_rung") && text.contains("outside the factor of two"),
        "the wrong rung was accepted: {text}"
    );
}

#[test]
fn a_region_covering_the_whole_domain_is_refused() {
    let entries = load(
        &workspace("whole"),
        &[(
            "geometry/whole",
            EVERYWHERE_SOURCE,
            &truth(&region("everything", r#"{"p":[0,4]}"#, ""), ""),
        )],
    );
    let problems = audit::audit(&entries, 41);
    assert!(
        problems
            .iter()
            .any(|p| p.detail.contains("covers all of it")),
        "any cell at all is a hit against a whole-domain region: {problems:?}"
    );
}

#[test]
fn a_new_entry_whose_region_was_measured_rather_than_derived_is_refused() {
    // The instrument's own output, used as its answer key.
    let scanned = format!(
        r#"{{"schema":"aporia.truth/2","method":"measured","fault":"boundary_condition",
 "derivation":"read off scan_axis","regions":[{}],"boundaries":[]}}"#,
        region("r", r#"{"p":[3,4]}"#, "")
    );
    let entries = load(
        &workspace("method"),
        &[("geometry/scanned", THRESHOLD_SOURCE, &scanned)],
    );
    let problems = audit::audit(&entries, 41);
    assert!(
        problems.iter().any(|p| p.detail.contains("closed form")),
        "{problems:?}"
    );
}

#[test]
fn a_new_entry_with_no_derivation_is_refused() {
    let bare = format!(
        r#"{{"schema":"aporia.truth/2","method":"analytic","fault":"boundary_condition",
 "derivation":"","regions":[{}],"boundaries":[]}}"#,
        region("r", r#"{"p":[3,4]}"#, "")
    );
    let entries = load(
        &workspace("deriv"),
        &[("geometry/silent", THRESHOLD_SOURCE, &bare)],
    );
    let problems = audit::audit(&entries, 41);
    assert!(
        problems.iter().any(|p| p.detail.contains("no derivation")),
        "{problems:?}"
    );
}

/// A branch table: scheme 1 fails above dt = 2, scheme 2 above dt = 1.
const BRANCH_SOURCE: &str = r#"model branch "" {
  input scheme in {1, 2}
  input dt in [0.01, 3]
  let factor = 1 - scheme * dt
  require factor > -1
}
"#;

fn branch(claims: &str) -> String {
    format!(
        r#"{{"schema":"aporia.truth/2","method":"analytic","fault":"time_step_sensitivity",
 "derivation":"scheme scales the damping, so the threshold is dt = 2/scheme",
 "regions":[{},{}],"boundaries":[],
 "choice_claims":{claims},"matched_to":"synthetic/rung"}}"#,
        region("scheme 1", r#"{"scheme":[1,1.5],"dt":[2,3]}"#, ""),
        region("scheme 2", r#"{"scheme":[1.5,2],"dt":[1,3]}"#, ""),
    )
}

#[test]
fn every_discrete_value_has_to_be_claimed_and_the_claim_has_to_hold() {
    let one_only = branch(r#"[{"axis":"scheme","value":1,"expect":"fails"}]"#);
    let contradicted = branch(
        r#"[{"axis":"scheme","value":1,"expect":"fails"},
            {"axis":"scheme","value":2,"expect":"clean"}]"#,
    );
    let entries = load(
        &workspace("discrete"),
        &[
            ("synthetic/rung", RUNG_SOURCE, &rung()),
            ("discrete/unclaimed", BRANCH_SOURCE, &one_only),
            ("discrete/contradicted", BRANCH_SOURCE, &contradicted),
        ],
    );
    let problems = audit::audit(&entries, 41);
    let text = problems
        .iter()
        .map(|p| format!("{}: {}", p.entry, p.detail))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        text.contains("scheme = 2 has no claim"),
        "an unclaimed branch slipped through: {text}"
    );
    assert!(
        text.contains("claims scheme = 2 is clean"),
        "a branch labelled clean while it fails slipped through: {text}"
    );
}

#[test]
fn a_discrete_axis_where_nothing_differs_is_decoration() {
    // `gain` selects nothing: the rule is the same on every branch, so calling the axis discrete would
    // describe the file rather than the model.
    let model = r#"model alike "" {
  input gain in {1, 2, 3}
  input dt in [0.01, 3]
  let factor = 1 - dt
  require factor > -1
}
"#;
    let truth_text = format!(
        r#"{{"schema":"aporia.truth/2","method":"analytic","fault":"time_step_sensitivity",
 "derivation":"dt alone decides","regions":[{}],"boundaries":[],
 "choice_claims":[{{"axis":"gain","value":1,"expect":"fails"}},
                  {{"axis":"gain","value":2,"expect":"fails"}},
                  {{"axis":"gain","value":3,"expect":"fails"}}],
 "matched_to":"synthetic/rung"}}"#,
        region("above two", r#"{"gain":[1,3],"dt":[2,3]}"#, "")
    );
    let entries = load(
        &workspace("alike"),
        &[
            ("synthetic/rung", RUNG_SOURCE, &rung()),
            ("discrete/decoration", model, &truth_text),
        ],
    );
    let problems = audit::audit(&entries, 41);
    assert!(
        problems
            .iter()
            .any(|p| p.detail.contains("carries no information")),
        "a discrete axis that changes nothing was accepted: {problems:?}"
    );
}

#[test]
fn a_refusal_that_did_not_refuse_is_refused() {
    // Declared as an IR refusal, and it compiles: what E1.3 measures would not have happened.
    let lies = r#"model lies "" {
  input x in [0, 4]
  let y = x / 2
  require y <= 2
}
"#;
    let refusal = r#"{"schema":"aporia.truth/2","method":"ir-refusal","fault":"domain_assumption",
 "derivation":"the domain is the claim","regions":[],"boundaries":[],"static":true}"#;
    // A second refusal fixture, and the assertion runs the other way: an entry that really is
    // refused with an error must not be flagged, or the rule would fire on every honest E1.3 entry.
    let warns = r#"model warns "" {
  input x in [5, 1]
  let y = x / 2
  require y <= 2
}
"#;
    let entries = load(
        &workspace("refusal"),
        &[
            ("domain/lies", lies, refusal),
            ("domain/warns", warns, refusal),
        ],
    );
    let problems = audit::audit(&entries, 41);
    let text = problems
        .iter()
        .map(|p| format!("{}: {}", p.entry, p.detail))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        text.contains("domain/lies") && text.contains("still compiled"),
        "a refusal that compiled was accepted: {text}"
    );
    assert!(
        !text.contains("domain/warns"),
        "a genuine IR refusal was flagged as not refusing: {text}"
    );
}

/// The gate `bench run` applies before it spends a sweep: the audit reads the whole corpus, however
/// few entries the command selected. The pilot learned this the hard way — eight entries chosen for a
/// plumbing check, and every arm refused because the rungs they were measured against had been left
/// out of the selection.
#[must_use]
fn run_gate(corpus_dir: &Path) -> Vec<String> {
    let entries = corpus::load(corpus_dir).expect("the fixture corpus loads");
    audit::audit(&entries, 41)
        .iter()
        .map(|p| p.detail.clone())
        .collect()
}

#[test]
fn a_subset_selection_is_gated_on_the_whole_corpus_not_on_itself() {
    let dir = workspace("subset");
    let paired = region("r", r#"{"p":[3,4]}"#, "");
    let entries = load(
        &dir,
        &[
            (
                "geometry/pair_a",
                THRESHOLD_SOURCE,
                &truth(&paired, r#", "matched_to": "geometry/pair_b""#),
            ),
            (
                "geometry/pair_b",
                COPY_SOURCE,
                &truth(&paired, r#", "matched_to": "geometry/pair_a""#),
            ),
        ],
    );
    // The refusal is about the pair, and a selection of one of them does not cause it.
    let all = audit::audit(&entries, 41);
    assert!(
        all.iter().any(|p| p.detail.contains("counts one")),
        "the fixture must fail the corpus-wide audit: {all:?}"
    );
    let one: Vec<_> = entries
        .iter()
        .filter(|e| e.id() == "geometry/pair_a")
        .cloned()
        .collect();
    let alone = audit::audit(&one, 41);
    assert!(
        alone.iter().any(|p| p.detail.contains("pair_b")),
        "auditing the selection alone hides the rung it names, which is the reason the run gate \
         reads the whole corpus: {alone:?}"
    );
    assert!(
        run_gate(&dir).iter().any(|d| d.contains("counts one")),
        "the run gate must still refuse this corpus, rung partner or not"
    );
}

#[test]
fn a_boundary_with_no_tolerance_or_outside_the_domain_is_refused() {
    let truth_text = format!(
        r#"{{"schema":"aporia.truth/2","method":"analytic","fault":"boundary_condition",
 "derivation":"d","regions":[{}],
 "boundaries":[{{"axis":"p","at":3,"tolerance":0}},
                {{"axis":"p","at":99,"tolerance":0.1}}]}}"#,
        region("r", r#"{"p":[3,4]}"#, "")
    );
    let entries = load(
        &workspace("boundary"),
        &[("geometry/bounds", THRESHOLD_SOURCE, &truth_text)],
    );
    let problems = audit::audit(&entries, 41);
    let text: String = problems
        .iter()
        .map(|p| p.detail.clone())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        text.contains("asks for an exact hit"),
        "a zero tolerance measures nothing: {text}"
    );
    assert!(text.contains("outside the declared"), "{text}");
}

#[test]
fn a_zero_width_region_is_not_treated_as_an_empty_one() {
    // The rule that would otherwise fire here refused a committed entry on its first run, and the
    // entry was right and the rule was wrong: `verify` checks a degenerate box at its exact point.
    let model = r#"model stop "" {
  input g in [0, 10]
  input v in [0, 10]
  let drop = 0.5 * g * v * v / 100
  require drop <= 0
}
"#;
    let entries = load(
        &workspace("degenerate"),
        &[(
            "geometry/point",
            model,
            &truth(
                &region("exactly zero gravity", r#"{"g":[0,0],"v":[0,10]}"#, ""),
                "",
            ),
        )],
    );
    let problems = audit::audit(&entries, 41);
    assert!(
        !problems
            .iter()
            .any(|p| p.detail.contains("covers none of the domain")),
        "a degenerate region was refused as nothing: {problems:?}"
    );
}

/// A peaked law that fails between p = 1 and p = 3: a quarter of its domain, which is the share the
/// rung fixture declares, and a region the permitted partition can enclose — which is what an E1.3b
/// entry has to be.
const GRANULAR_SOURCE: &str = r#"model wide "" {
  input p in [0, 8]
  let y = 4 * p - p * p
  require y <= 3
}
"#;

/// The same band one domain-width further along, arrived at by a different law, so a fixture can be a
/// mechanism twin without also being a duplicate model or a duplicate region.
const TWIN_SOURCE: &str = r#"model twin "" {
  input p in [0, 8]
  let y = (p - 5) * (7 - p)
  require y <= 0
}
"#;

/// The variables a `/3` fixture declares: exactly the parameters `GRANULAR_SOURCE` and `TWIN_SOURCE`
/// have, with the unit and span those models declare.
const GRANULAR_VARIABLES: &str =
    r#"[{"name":"p","unit":"1","role":"the axis the peak runs along","lo":0.0,"hi":8.0}]"#;

/// The rung the fixtures are pinned against. `synthetic` is not a new family, so this entry needs no
/// rung of its own, and its declared box is a quarter of its domain — the same share as `wide`'s band.
const GRANULAR_RUNG: &str = "synthetic/rung";

/// One part of the record, as text, so a test can break one part and leave the rest of a passing
/// fixture alone. An emptied part stays a stated part: a key dropped would be read as "no `/3` claim",
/// which is a different case and has its own test below.
struct Record {
    derivation: &'static str,
    meaning: &'static str,
    mechanism: &'static str,
    source: &'static str,
    why: &'static str,
    resolvability: &'static str,
    channels: &'static str,
    variables: &'static str,
    resolvable: &'static str,
}

impl Record {
    fn base() -> Self {
        Self {
            derivation: "(p-1)(p-3) < 0 solved by hand",
            meaning: "a response that peaks past its limit between one and three",
            mechanism: "a peaked response exceeding its ceiling in one interior band",
            source: "the factorisation of the declared inequality worked out on paper",
            why: "an interior band the atlas can enclose which E1.3 could not",
            resolvability: "finest leaf 8/4096 = 0.00195 against a band 2 wide",
            channels: r#"["physical"]"#,
            variables: GRANULAR_VARIABLES,
            resolvable: "true",
        }
    }

    /// The same record with one named field replaced, which is every mutation these tests need.
    fn changed(&self, field: &str, value: &'static str) -> Self {
        let mut out = Self {
            derivation: self.derivation,
            meaning: self.meaning,
            mechanism: self.mechanism,
            source: self.source,
            why: self.why,
            resolvability: self.resolvability,
            channels: self.channels,
            variables: self.variables,
            resolvable: self.resolvable,
        };
        match field {
            "derivation" => out.derivation = value,
            "meaning" => out.meaning = value,
            "mechanism" => out.mechanism = value,
            "source" => out.source = value,
            "why" => out.why = value,
            "resolvability" => out.resolvability = value,
            "channels" => out.channels = value,
            "variables" => out.variables = value,
            "resolvable" => out.resolvable = value,
            other => panic!("the fixture states no field {other:?}"),
        }
        out
    }
}

/// A complete `aporia.truth/3` fixture, one field per line.
///
/// It names a rung as well as a record, because an entry in a new family has to be pinned against
/// something of comparable difficulty before anything about it can be measured.
#[must_use]
fn truth3(record: &Record, regions: &str, tail: &str) -> String {
    format!(
        r#"{{"schema":"aporia.truth/3","method":"analytic","fault":"boundary_condition",
 "derivation":"{derivation}","meaning":"{meaning}","variables":{variables},
 "mechanism":"{mechanism}","channels":{channels},"resolvable":{resolvable},
 "resolvability":"{resolvability}","source":"{source}","why":"{why}",
 "matched_to":"{rung}","regions":[{regions}],"boundaries":[]{tail}}}"#,
        derivation = record.derivation,
        meaning = record.meaning,
        variables = record.variables,
        mechanism = record.mechanism,
        channels = record.channels,
        resolvable = record.resolvable,
        resolvability = record.resolvability,
        source = record.source,
        why = record.why,
        rung = GRANULAR_RUNG,
    )
}

/// The failing set of [`GRANULAR_SOURCE`], declared as the closed form says it is.
fn wide_region() -> String {
    region("past the limit", r#"{"p":[1,3]}"#, "")
}

/// The rung fixture plus the `/3` entry under test, read through the real loader.
fn granular_corpus(tag: &str, truth: &str) -> Vec<corpus::Entry> {
    let rung_truth = rung();
    load(
        &workspace(tag),
        &[
            (GRANULAR_RUNG, RUNG_SOURCE, rung_truth.as_str()),
            ("granular/wide", GRANULAR_SOURCE, truth),
        ],
    )
}

/// What the gate says about a fixture, as one block of text — the shape a person reading the refusal
/// sees, which is what these tests assert against rather than the count of problems.
#[must_use]
fn said(entries: &[corpus::Entry]) -> String {
    audit::audit(entries, 41)
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

/// A fixture that states its record in full has to pass before another test can refuse it for one
/// emptied part — otherwise the refusal might be about the fixture rather than the rule.
#[test]
fn a_resolvable_granular_entry_passes_the_new_rules() {
    let text = said(&granular_corpus(
        "g-ok",
        &truth3(&Record::base(), &wide_region(), ""),
    ));
    assert!(text.is_empty(), "the clean fixture was refused: {text}");
}

#[test]
fn an_entry_that_claims_resolvability_the_partition_denies_is_refused_with_the_numbers() {
    // The rule E1.3's own corpus needed: a region thinner than any leaf the atlas may build cannot be
    // localised at any budget, so a trust rule scored on it is measured on nothing at all.
    let thin = region(
        "a sliver",
        r#"{"p":[1.99992,2.00008]}"#,
        r#","where":"4 * p - p * p >= 3.99999""#,
    );
    let text = said(&granular_corpus(
        "g-thin",
        &truth3(&Record::base(), &thin, ""),
    ));
    assert!(
        text.contains("claims resolvable = true") && text.contains("sub-granular"),
        "the refusal must name the claim and the class that contradicts it: {text}"
    );
    assert!(
        text.contains("localisation bar"),
        "and the bar the region was measured against: {text}"
    );
}

#[test]
fn an_empty_part_of_the_record_is_named_rather_than_tolerated() {
    for field in [
        "derivation",
        "meaning",
        "mechanism",
        "source",
        "why",
        "resolvability",
        "channels",
        "variables",
    ] {
        let value = if field == "channels" || field == "variables" {
            "[]"
        } else {
            ""
        };
        let record = Record::base().changed(field, value);
        let text = said(&granular_corpus(
            &format!("g-empty-{field}"),
            &truth3(&record, &wide_region(), ""),
        ));
        assert!(
            text.contains(&format!("states no {field}")),
            "{field}: the refusal must name the missing part: {text}"
        );
        assert!(
            text.contains("aporia.truth/3"),
            "{field}: the refusal must name the schema that requires it: {text}"
        );
    }
    // The claim itself: an entry that states nothing to check is refused for that, not skipped.
    let record = Record::base().changed("resolvable", "null");
    let text = said(&granular_corpus(
        "g-claim",
        &truth3(&record, &wide_region(), ""),
    ));
    assert!(
        text.contains("`resolvable` claim"),
        "the refusal must name the claim that is missing: {text}"
    );
}

#[test]
fn a_unit_or_span_the_model_does_not_declare_is_refused() {
    // A truth file describing a different model than the one beside it measures nothing, so the variable
    // list is compared against the model's own parameters: their names, their canonical dimensions and
    // their declared spans.
    for (tag, variables, wanted) in [
        (
            "dimension",
            r#"[{"name":"p","unit":"kg","role":"the axis","lo":0.0,"hi":8.0}]"#,
            "declares it as",
        ),
        (
            "unparsable",
            r#"[{"name":"p","unit":"furlongs","role":"the axis","lo":0.0,"hi":8.0}]"#,
            "canonical form",
        ),
        (
            "span",
            r#"[{"name":"p","unit":"1","role":"the axis","lo":0.0,"hi":40.0}]"#,
            "domain is",
        ),
        (
            "absent",
            r#"[{"name":"q","unit":"1","role":"an axis that is not here","lo":0.0,"hi":8.0}]"#,
            "no parameter for",
        ),
        ("nothing", "[]", "says nothing about"),
    ] {
        let record = Record::base().changed("variables", variables);
        let text = said(&granular_corpus(
            &format!("g-unit-{tag}"),
            &truth3(&record, &wide_region(), ""),
        ));
        assert!(
            text.contains(wanted),
            "{tag}: the refusal must say {wanted:?}: {text}"
        );
    }
}

#[test]
fn one_mechanism_written_twice_is_one_case_counted_twice() {
    // Two entries whose models, regions and records differ in every field except the mechanism string:
    // the duplicate that `no_duplicate_models` and `no_duplicate_regions` cannot see, and that a summary
    // counting rows would read as two cases.
    let base = Record::base();
    let twin = base
        .changed("why", "the same mechanism under a second name")
        .changed(
            "meaning",
            "a second band that runs past its floor between five and seven",
        )
        .changed("derivation", "(p-5)(p-7) < 0 solved by hand");
    let rung_truth = rung();
    let first = truth3(&base, &wide_region(), "");
    let second = truth3(&twin, &region("the second band", r#"{"p":[5,7]}"#, ""), "");
    let entries = load(
        &workspace("g-mech"),
        &[
            (GRANULAR_RUNG, RUNG_SOURCE, rung_truth.as_str()),
            ("granular/wide", GRANULAR_SOURCE, first.as_str()),
            ("granular/again", TWIN_SOURCE, second.as_str()),
        ],
    );
    let text = said(&entries);
    assert!(
        text.contains("same mechanism as"),
        "the refusal must name the duplicate: {text}"
    );
}

#[test]
fn an_answer_key_taken_from_the_instrument_is_refused() {
    // 0037's correction C8 was written to keep this out of the corpus; the field check makes it a
    // refusal instead of a reading. The answer key has to come from outside the thing being marked.
    for (tag, source) in [
        ("search", "measured by the search at the largest budget"),
        ("atlas", "read out of the atlas the run built"),
        ("results", "results-0704da190c01.json"),
    ] {
        let record = Record::base().changed("source", source);
        let text = said(&granular_corpus(
            &format!("g-source-{tag}"),
            &truth3(&record, &wide_region(), ""),
        ));
        assert!(
            text.contains("answer key has to come from outside"),
            "{source:?} was accepted as a truth source: {text}"
        );
    }
}

#[test]
fn an_endpoint_region_in_a_resolvable_entry_is_refused() {
    // `projectile_zero_gravity` declares g = 0 exactly and is a real case, checked at the point by
    // `verify` and classified unreachable by the instrument. Claiming that shape as *resolvable* is a
    // mistake rather than a case: it can only ever be missed.
    let point = region("exactly the vertex", r#"{"p":[2.0,2.0]}"#, "");
    let text = said(&granular_corpus(
        "g-point",
        &truth3(&Record::base(), &point, ""),
    ));
    assert!(
        text.contains("zero width on \"p\""),
        "the refusal must name the flat axis: {text}"
    );
    assert!(
        text.contains("endpoint"),
        "and say why an endpoint is not a region: {text}"
    );
}

#[test]
fn a_channel_the_instrument_does_not_have_is_refused_by_name() {
    let record = Record::base().changed("channels", r#"["magnetic"]"#);
    let text = said(&granular_corpus(
        "g-chan",
        &truth3(&record, &wide_region(), ""),
    ));
    assert!(
        text.contains("does not have") && text.contains("behavioral"),
        "the refusal must name the channel and the vocabulary it expected: {text}"
    );
}

#[test]
fn an_entry_marked_control_that_declares_a_region_is_refused() {
    // A `/3` freeze has to name its control entries, and a control is only worth naming because nothing
    // is to be found on it: its suspicious volume measures false suspicion and its withheld volume
    // measures cost. An entry claiming both is a different measurement wearing the cleanliness name, so
    // the audit refuses it. The fixture starts from the record that already passes every other rule, so
    // the refusal can only be about `control`.
    let text = said(&granular_corpus(
        "g-ctrl",
        &truth3(&Record::base(), &wide_region(), r#","control":true"#),
    ));
    assert!(
        text.contains("marked `control`") && text.contains("nothing is to be found"),
        "the refusal has to name the contradiction rather than reject the entry: {text}"
    );

    // And the rule is not vacuous on the committed corpus: five entries are marked control, every one of
    // them declares nothing to find, and the audit therefore says nothing about any of them. Without the
    // count the absence would prove nothing -- an empty control set passes by saying nothing, which is
    // exactly the defect E1.3b reported as unmeasurable.
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../benchmarks");
    let entries = corpus::load(&root).expect("the committed corpus loads");
    let controls: Vec<&corpus::Entry> = entries.iter().filter(|e| e.truth.control).collect();
    assert_eq!(
        controls.len(),
        5,
        "five committed entries are marked control, one of them the entry E1.4 registers in its run list"
    );
    assert!(
        controls
            .iter()
            .any(|e| e.id() == "control/symplectic_spring"),
        "E1.4's registered control has to be a control in the corpus as well as in the freeze"
    );
    for e in &controls {
        assert!(
            e.truth.regions.is_empty() && e.truth.boundaries.is_empty(),
            "{} is marked control and declares something to find",
            e.id()
        );
    }
    assert!(
        !audit::audit(&entries, 41)
            .iter()
            .any(|p| p.to_string().contains("marked `control`")),
        "the committed corpus trips its own control rule"
    );
}

#[test]
fn a_stray_artefact_beside_an_entry_is_refused_rather_than_ignored() {
    // The loader reads the registry, so a stray file cannot become an entry — but it can be read back as
    // one later, which is exactly what an experiment archive written under the corpus root once did.
    let dir = workspace("g-stray");
    let rung_truth = rung();
    let truth = truth3(&Record::base(), &wide_region(), "");
    let entries = load(
        &dir,
        &[
            (GRANULAR_RUNG, RUNG_SOURCE, rung_truth.as_str()),
            ("granular/wide", GRANULAR_SOURCE, truth.as_str()),
        ],
    );
    assert!(
        audit::audit(&entries, 41).is_empty(),
        "the fixture is clean"
    );
    std::fs::write(
        dir.join("granular").join("wide").join("atlas.csv"),
        "cell,depth,label\n0,0,TRUSTED\n",
    )
    .expect("write a stray artefact");
    let reread = corpus::load(&dir).expect("the corpus still loads");
    let text = said(&reread);
    assert!(
        text.contains("atlas.csv") && text.contains("stale or in the wrong place"),
        "the refusal must name the file: {text}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_committed_granular_entries_are_the_record_they_claim() {
    // Every entry the `granular` family holds, through the same rules the fixtures exercise, and with
    // their own text. The family was E1.3b's five, became six when E1.4 authored `granular/fit_band`
    // into it, and became ten when E1.5 authored its four: the two that give the resolution question
    // something to act on, the one whose region is narrower than any leaf the certification bar allows,
    // and the one that is wide, sampled and therefore only able to show what a policy costs. This counts
    // the directory rather than one freeze's list — an entry that failed to claim what the others claim
    // has to show up here, not be filtered out by a number written before it existed.
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../benchmarks");
    let entries = corpus::load(&root).expect("the committed corpus loads");
    let granular: Vec<corpus::Entry> = entries
        .iter()
        .filter(|e| e.family == "granular")
        .cloned()
        .collect();
    assert_eq!(
        granular.len(),
        10,
        "the granular family is E1.3b's five, E1.4's band and E1.5's four"
    );
    for e in &granular {
        // Exactly one entry in this family claims unresolvability, and it is the entry whose whole
        // purpose is that claim: a family where the flag could drift either way would let a future
        // entry be filed as unreachable because nobody re-read the list.
        let expected = if e.id() == "granular/thin_reach" {
            Some(false)
        } else {
            Some(true)
        };
        assert_eq!(e.truth.resolvable, expected, "{}", e.id());
        assert!(
            !e.truth.mechanism.is_empty()
                && !e.truth.resolvability.is_empty()
                && !e.truth.source.is_empty()
                && !e.truth.why.is_empty()
                && !e.truth.meaning.is_empty()
                && !e.truth.derivation.is_empty(),
            "{} leaves part of its record empty",
            e.id()
        );
        assert_eq!(
            e.truth.variables.len(),
            e.model
                .as_ref()
                .expect("the resolvable set is measurable")
                .params
                .len(),
            "{} declares a different number of variables than it has parameters",
            e.id()
        );
        assert!(
            !e.truth.curved,
            "{}: none of these boundaries is a curve, and claiming one would misdescribe the region",
            e.id()
        );
    }
    // E1.5's deciding entries are box-declared, because the region decomposition is only exact where
    // the union is a sum of widths and a lattice estimate is not a denominator.
    for e in &granular {
        let e15 = [
            "granular/quiet_band",
            "granular/mixed_cell",
            "granular/thin_reach",
            "granular/semantic_step",
        ]
        .contains(&e.id().as_str());
        if !e15 {
            continue;
        }
        for (at, region) in e.truth.regions.iter().enumerate() {
            assert!(
                region.predicate.is_none(),
                "{} region {at} carves a `where`, so its union share is a lattice estimate and cannot \
                 carry the frozen metric",
                e.id()
            );
        }
    }
    let mut mechanisms: Vec<&str> = granular
        .iter()
        .map(|e| e.truth.mechanism.as_str())
        .collect();
    let before = mechanisms.len();
    mechanisms.sort_unstable();
    mechanisms.dedup();
    assert_eq!(before, mechanisms.len(), "two entries share a mechanism");
    let text = said(&entries);
    assert!(text.is_empty(), "the corpus fails its own audit: {text}");
}
