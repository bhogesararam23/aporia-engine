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
