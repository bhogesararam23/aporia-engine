//! `aporia bench` — the measurement harness behind the instrument's own front door.
//!
//! The property these tests exist to protect is that there is only one implementation. Every command is
//! checked against what `aporia-bench` would do with the same words: the same statuses, the same
//! refusals, the same corpus, and a usage line that names `aporia bench` rather than the sibling
//! binary. If this module ever grows a rule of its own, one of these assertions has to break.

use std::path::{Path, PathBuf};
use std::process::Command;

fn aporia() -> Command {
    Command::new(env!("CARGO_BIN_EXE_aporia"))
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).to_string()
}

fn scratch(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("aporia-bench-{}-{}", label, std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn results(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../benchmarks/results")
        .join(rel)
}

#[test]
fn a_bare_bench_shows_its_own_usage_and_calls_that_a_command_was_missing() {
    let out = aporia().arg("bench").output().expect("aporia runs");
    let body = text(&out.stdout);
    assert_eq!(out.status.code(), Some(2), "stdout was:\n{body}");
    assert!(body.contains("usage: aporia bench <command>"), "{body}");
    for command in ["list", "verify", "run", "verdict", "scan", "explain"] {
        assert!(
            body.contains(command),
            "the usage does not list {command}:\n{body}"
        );
    }
}

#[test]
fn listing_the_corpus_reaches_the_registry_the_published_numbers_came_from() {
    let out = aporia()
        .arg("bench")
        .arg("list")
        .output()
        .expect("aporia runs");
    let body = text(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "stdout was:\n{body}");
    assert!(body.contains("analytic/sqrt_domain"), "{body}");
    assert!(body.contains("electromagnetics/rlc_resonance"), "{body}");
    let counted = body
        .lines()
        .last()
        .unwrap_or_else(|| panic!("no trailing summary in:\n{body}"))
        .to_string();
    // The same registry `aporia-bench list` reads. Counted from the corpus's own definition of an
    // entry -- a directory with a `truth.json` -- rather than from a number copied into this test, so
    // the assertion says the front door and the harness agree, not that both match my arithmetic.
    let declared = count_corpus_entries();
    assert!(
        counted.contains(&format!("{declared} entries")),
        "the front door reported {counted:?} while the corpus holds {declared} entries"
    );
}

/// How many entries the corpus declares: every directory under `benchmarks` that carries a
/// `truth.json`, excluding the generated `archives` and `results` trees.
fn count_corpus_entries() -> usize {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../benchmarks");
    let mut seen = 0;
    count_entries(&root, &mut seen);
    seen
}

fn count_entries(dir: &Path, seen: &mut usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if path.join("truth.json").is_file() {
            *seen += 1;
        } else if !matches!(
            path.file_name().and_then(|n| n.to_str()),
            Some("archives" | "results")
        ) {
            count_entries(&path, seen);
        }
    }
}

#[test]
fn an_unknown_bench_command_is_refused_with_the_front_door_s_own_name() {
    let out = aporia()
        .arg("bench")
        .arg("frobnicate")
        .output()
        .expect("aporia runs");
    assert_eq!(out.status.code(), Some(2));
    let err = text(&out.stderr);
    assert!(err.contains("unknown command \"frobnicate\""), "{err}");
    // The hint has to name the command the reader typed, not the other binary.
    assert!(err.contains("usage: aporia bench"), "{err}");
    assert!(!err.contains("usage: aporia-bench"), "{err}");
}

#[test]
fn a_flag_that_is_not_part_of_a_bench_command_is_refused_not_ignored() {
    // `--budget` belongs to `aporia run`, not to `bench run`, whose knob is `--budgets`. Silently
    // sweeping the default ladder because one "s" was missing would report an experiment nobody asked
    // for, under a name that looks like the one they typed.
    let out = aporia()
        .arg("bench")
        .arg("run")
        .arg("--budget")
        .arg("40")
        .output()
        .expect("aporia runs");
    assert_eq!(out.status.code(), Some(2));
    assert!(text(&out.stderr).contains("unknown flag --budget"));
}

#[test]
fn a_measurement_can_be_made_through_the_front_door_and_is_named_for_its_plan() {
    // The whole flow, from `aporia bench run` to a results file: corpus selection, ground-truth
    // verification, the sweep, the archive, and the identity the measurement is named for. One entry at
    // one budget keeps this a test rather than a measurement campaign; `--out` and `--archive` point at
    // a scratch directory so nothing here touches the published results.
    let dir = scratch("run");
    let archives = scratch("archive");
    let rerun_archives = scratch("archive2");
    let first = aporia()
        .arg("bench")
        .arg("run")
        .arg("--only")
        .arg("analytic/sqrt_domain")
        .arg("--budgets")
        .arg("40")
        .arg("--seeds")
        .arg("1")
        .arg("--strategies")
        .arg("adaptive")
        .arg("--out")
        .arg(&dir)
        .arg("--archive")
        .arg(&archives)
        .output()
        .expect("aporia runs");
    let body = text(&first.stdout);
    assert_eq!(first.status.code(), Some(0), "stdout was:\n{body}");
    assert!(body.contains("results written to"), "{body}");
    let written: Vec<String> = std::fs::read_dir(&dir)
        .expect("the results directory")
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(written.len(), 1, "{written:?}");
    // The name is `results-<identity>.json`, where the identity is the plan digest the document
    // records, so the file's name and its content can be checked against each other.
    let identity = written[0]
        .strip_prefix("results-")
        .and_then(|rest| rest.strip_suffix(".json"))
        .unwrap_or_else(|| panic!("the results file is not named for its plan: {}", written[0]));
    assert!(
        identity.len() == 12 && identity.chars().all(|c| c.is_ascii_hexdigit()),
        "{identity} is not a measurement identity"
    );
    assert!(body.contains(&format!("identity {identity}")), "{body}");

    // The same plan through the same door a second time is the same measurement, and the harness's
    // refusal to write it twice is what makes the identity mean something.
    let second = aporia()
        .arg("bench")
        .arg("run")
        .arg("--only")
        .arg("analytic/sqrt_domain")
        .arg("--budgets")
        .arg("40")
        .arg("--seeds")
        .arg("1")
        .arg("--strategies")
        .arg("adaptive")
        .arg("--out")
        .arg(&dir)
        .arg("--archive")
        .arg(&rerun_archives)
        .output()
        .expect("aporia runs");
    assert_eq!(second.status.code(), Some(2));
    let err = text(&second.stderr);
    assert!(err.contains("already holds this measurement"), "{err}");
    assert!(err.contains("identity"), "{err}");
    // The refusal means the first file is still the one that was written, not a second copy of it.
    let after: Vec<String> = std::fs::read_dir(&dir)
        .expect("the results directory")
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(after.len(), 1, "{after:?}");
    for scratch_dir in [&dir, &archives, &rerun_archives] {
        let _ = std::fs::remove_dir_all(scratch_dir);
    }
}

#[test]
fn reading_a_published_measurement_back_names_the_experiment_it_was() {
    // A committed results file, written before measurement identities existed, read through the front
    // door. The identity it prints comes from the schema, plan and entries recorded inside it, which
    // is the only way an old number stays addressable — and the schema is in that digest, so a v1 file
    // names the v1 experiment rather than borrowing the name a v2 run of the same plan would get.
    let out = aporia()
        .arg("bench")
        .arg("verdict")
        .arg(results("results-1791153844.json"))
        .output()
        .expect("aporia runs");
    let body = text(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "stdout was:\n{body}");
    let line = body
        .lines()
        .find(|l| l.starts_with("identity  "))
        .unwrap_or_default();
    assert!(
        line.contains("derived from the schema, plan and entries it records"),
        "{body}"
    );
    let named = line.split_whitespace().nth(1).unwrap_or_default();
    assert!(
        named.len() == 12 && named.chars().all(|c| c.is_ascii_hexdigit()),
        "the derived name is not the digest this tool writes: {line}"
    );
    assert!(body.contains("analytic/sqrt_domain"), "{body}");
}

#[test]
fn a_measurement_that_recorded_its_own_identity_is_not_asked_to_guess_it() {
    // The other half of the same promise: when the writer put the field in, `verdict` says so instead
    // of printing a derived name that happens to agree. Written by the build that charges a
    // counterexample row in executions, so it is the first `aporia.results/2` file in the repository.
    let out = aporia()
        .arg("bench")
        .arg("verdict")
        .arg(results("results-bb424c168dd4.json"))
        .output()
        .expect("aporia runs");
    let body = text(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "stdout was:\n{body}");
    assert!(
        body.contains("identity  bb424c168dd4  (as written)"),
        "{body}"
    );
}

#[test]
fn a_missing_results_file_is_the_harness_s_refusal_not_a_blank_verdict() {
    let out = aporia()
        .arg("bench")
        .arg("verdict")
        .arg("no-such-results-file.json")
        .output()
        .expect("aporia runs");
    assert_eq!(out.status.code(), Some(2));
    let err = text(&out.stderr);
    assert!(err.contains("no-such-results-file.json"), "{err}");
    assert!(err.contains("usage: aporia bench"), "{err}");
}

#[test]
fn the_ground_truth_check_runs_before_any_number_is_produced() {
    // `verify` is the step that turns `benchmarks/` from a set of opinions into a measuring instrument,
    // and it has to be reachable from the same door that runs the sweep. Committed corpus, real grid.
    let out = aporia()
        .arg("bench")
        .arg("verify")
        .arg("--grid")
        .arg("20")
        .output()
        .expect("aporia runs");
    let body = text(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "stdout was:\n{body}");
    assert!(
        body.contains("all declarations hold"),
        "the corpus no longer verifies: {body}"
    );
    // And a grid too coarse to cover the domain edges is refused rather than silently used.
    let small = aporia()
        .arg("bench")
        .arg("verify")
        .arg("--grid")
        .arg("2")
        .output()
        .expect("aporia runs");
    assert_eq!(small.status.code(), Some(2));
    assert!(text(&small.stderr).contains("--grid must be at least 3"));
}
