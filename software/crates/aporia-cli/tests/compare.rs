//! `aporia compare <dir-a> <dir-b>`: two stored runs, put next to each other.
//!
//! The interesting property is what the command must *not* do. It must not execute either model, must
//! not re-fit a calibration, and must not present a difference between a corrupt archive and an intact
//! one as a finding. So most of what these tests check is a refusal: exit statuses that keep "the
//! archives differ", "the archives could not be fully compared", "one of these is not the archive that
//! was written" and "I was not given two directories" apart from each other, because a caller that has
//! to grep stdout to tell those apart will get it wrong eventually.

use std::path::{Path, PathBuf};
use std::process::Command;

const PROGRAM: &str = env!("CARGO_BIN_EXE_aporia-test-program");

fn aporia() -> Command {
    Command::new(env!("CARGO_BIN_EXE_aporia"))
}

fn fixtures(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

/// A model from the corpus, so the comparison is run on the same arithmetic the published
/// measurements were made with rather than on a fixture written for this test.
fn corpus_model(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../benchmarks")
        .join(rel)
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).to_string()
}

fn scratch(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("aporia-compare-{}-{}", label, std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    dir
}

#[test]
fn two_archives_of_one_run_compare_with_nothing_to_report() {
    // The same directory on both sides is the cheapest case that still exercises the whole path, and
    // the case that must be exactly zero: a comparison that reports a difference between an archive
    // and itself has invented one.
    let dir = fixtures("archive-sqrt_domain");
    let out = aporia()
        .arg("compare")
        .arg(&dir)
        .arg(&dir)
        .output()
        .expect("aporia runs");
    let body = text(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "stdout was:\n{body}");
    assert!(body.contains("verdict  IDENTICAL"), "{body}");
    assert!(!body.contains("DIFFERENT"), "{body}");
    assert!(body.contains("identity  same"), "{body}");
}

#[test]
fn the_committed_pair_reports_the_budget_and_everything_it_moved() {
    // Two real archives of one corpus model, 40 evaluations against 80, both committed. What differs
    // between them is a fact about the runs, not about this test: the second spent twice the budget,
    // refined the map further, and localised the region more tightly.
    let a = fixtures("archive-sqrt_domain");
    let b = fixtures("archive-sqrt_domain-b80");
    let out = aporia()
        .arg("compare")
        .arg(&a)
        .arg(&b)
        .output()
        .expect("aporia runs");
    let body = text(&out.stdout);
    assert_eq!(out.status.code(), Some(8), "stdout was:\n{body}");
    assert!(body.contains("verdict  DIFFERENT:"), "{body}");
    assert!(body.contains("config.budget  40 -> 80"), "{body}");
    assert!(body.contains("evaluations  40 -> 80"), "{body}");
    // The region moved: the small run could only say `x in [-10, 0]`, the larger one narrowed it.
    // Each is a region the other archive does not hold, which is what `only in A` / `only in B` means.
    assert!(body.contains("region [-10, 0]"), "{body}");
    assert!(body.contains("(only in A)"), "{body}");
    assert!(body.contains("(only in B)"), "{body}");
    // Model identity agreed, so this is a comparison of two runs of one model rather than of two
    // models that happen to share a name.
    assert!(body.contains("identity  same"), "{body}");
    assert!(
        body.contains("neither model was executed"),
        "the report should say where its numbers came from:\n{body}"
    );
}

#[test]
fn a_corrupt_archive_stops_the_comparison_before_it_starts() {
    // Editing a byte makes one side's fields unreliable. Diffing them anyway would turn a filesystem
    // accident into a statement about two runs, and the status a CI job sees would be "these experiments
    // differ".
    let intact = fixtures("archive-sqrt_domain");
    let tampered = scratch("tampered");
    copy_dir(&intact, &tampered);
    let records = tampered.join("observations.bin");
    let mut bytes = std::fs::read(&records).expect("the archive's records");
    let last = bytes.len() - 1;
    bytes[last] ^= 0x01;
    std::fs::write(&records, &bytes).expect("write back");

    let out = aporia()
        .arg("compare")
        .arg(&intact)
        .arg(&tampered)
        .output()
        .expect("aporia runs");
    let body = text(&out.stdout);
    assert_eq!(out.status.code(), Some(5), "stdout was:\n{body}");
    assert!(body.contains("integrity  1 problem(s)"), "{body}");
    assert!(body.contains("observations.bin"), "{body}");
    assert!(body.contains("verdict  NOT COMPARED"), "{body}");
    // Nothing was diffed, so no field-level verdict may appear.
    assert!(!body.contains("verdict  DIFFERENT"), "{body}");
    assert!(!body.contains("config.budget"), "{body}");
    let _ = std::fs::remove_dir_all(&tampered);
}

#[test]
fn comparing_a_program_executed_run_never_touches_the_program() {
    // The archive of an external run records points and answers, not a recipe for recomputing them.
    // `replay` therefore refuses to reproduce it. `compare` has a different contract: it reads, so two
    // such archives must be comparable with no program present at all -- and the command must not
    // quietly decide to re-run one to fill a gap.
    let model = fixtures("beam.ap");
    let a = scratch("program-a");
    let b = scratch("program-b");
    for dir in [&a, &b] {
        let out = aporia()
            .arg("run")
            .arg(&model)
            .arg("--program")
            .arg(PROGRAM)
            .arg("--budget")
            .arg("30")
            .arg("--archive")
            .arg(dir)
            .output()
            .expect("aporia runs");
        assert!(
            out.status.code() == Some(0) || out.status.code() == Some(1),
            "the run failed: {}",
            text(&out.stderr)
        );
    }
    // Compare them with the program path gone from the argument list: nothing here can execute the
    // model, and the command is not allowed to need it.
    let out = aporia()
        .arg("compare")
        .arg(&a)
        .arg(&b)
        .output()
        .expect("aporia runs");
    let body = text(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "stdout was:\n{body}");
    assert!(body.contains("verdict  IDENTICAL"), "{body}");
    assert!(!body.contains("not attempted"), "{body}");
    let _ = std::fs::remove_dir_all(&a);
    let _ = std::fs::remove_dir_all(&b);
}

#[test]
fn two_models_of_different_size_are_not_paired_region_by_region() {
    // One parameter against two: the regions are coordinates in different spaces, so the comparison
    // refuses those sections and says which. The status is not `0`, because nothing was agreed about
    // the parts that could not be asked.
    let small = fixtures("archive-sqrt_domain");
    let big = scratch("two-params");
    let ran = aporia()
        .arg("run")
        .arg(corpus_model("ode/euler_decay_2d/model.ap"))
        .arg("--budget")
        .arg("40")
        .arg("--archive")
        .arg(&big)
        .output()
        .expect("aporia runs");
    assert!(
        ran.status.code() == Some(0) || ran.status.code() == Some(1),
        "the run failed: {}",
        text(&ran.stderr)
    );
    let out = aporia()
        .arg("compare")
        .arg(&small)
        .arg(&big)
        .output()
        .expect("aporia runs");
    let body = text(&out.stdout);
    assert_eq!(out.status.code(), Some(8), "stdout was:\n{body}");
    assert!(body.contains("findings  not compared"), "{body}");
    assert!(body.contains("atlas  not compared"), "{body}");
    assert!(
        body.contains("skipped  findings: 1 parameters versus 2"),
        "{body}"
    );
    assert!(body.contains("params  1 -> 2"), "{body}");
    // What is not inside the parameter space is still asked: the two runs' record bytes and their
    // finding files have nothing to do with how many inputs the model had, and both are reported.
    // (Their *counts* happen to agree here -- 40 evaluations each -- and an agreed field prints
    // nothing, which is the other half of this test.)
    assert!(body.contains("digest[observations.bin]"), "{body}");
    assert!(!body.contains("records[observations.bin]"), "{body}");
    assert!(body.contains("findings[*.apx]"), "{body}");
    let _ = std::fs::remove_dir_all(&big);
}

#[test]
fn a_benchmark_archive_and_a_command_line_archive_of_one_model_agree_on_the_model() {
    // One corpus model, archived by both callers: `aporia-bench` at 160 evaluations and `aporia run`
    // at 40. Those directories were written through code paths that each assembled their own stored
    // finding records until they were made to share one construction, so their bytes agreeing on model
    // identity is the result worth pinning down. The harness's archive is committed here as a fixture
    // rather than read from the local-only `software/benchmarks/archives` tree, so a fresh clone runs
    // this against the same bytes.
    //
    // What legitimately differs is reported as what it is: the campaign settings, how many regions the
    // longer run found, and the environment notes each caller writes about itself, which appear on one
    // side only rather than as a disagreement.
    let from_bench = fixtures("archive-bench-sqrt_domain");
    let from_cli = fixtures("archive-sqrt_domain");
    let out = aporia()
        .arg("compare")
        .arg(&from_bench)
        .arg(&from_cli)
        .output()
        .expect("aporia runs");
    let body = text(&out.stdout);
    assert_eq!(out.status.code(), Some(8), "stdout was:\n{body}");
    assert!(body.contains("identity  same"), "{body}");
    let line = |name: &str| {
        body.lines()
            .find(|l| l.trim_start().starts_with(name))
            .map_or_else(|| panic!("no line for {name} in:\n{body}"), str::trim)
            .to_string()
    };
    assert!(
        line("note.entry").contains("only in A"),
        "the harness names its corpus entry and the command line does not: {}",
        line("note.entry")
    );
    assert!(
        line("note.execution").contains("only in B"),
        "the command line names who computed the values and the harness does not: {}",
        line("note.execution")
    );
    assert!(
        line("config.budget").contains("160 -> 40"),
        "{}",
        line("config.budget")
    );
    // The findings section reports its own count first, so a pair of runs that both found nothing
    // still shows a comparison that took place. The harness spent four times the budget and named two
    // more regions; both are stored facts about the two runs, not interpretations.
    assert!(
        line("count").contains("3 -> 1"),
        "the region counts the two callers stored: {}",
        line("count")
    );
    // Both runs were adaptive. The text carries only the fields there is something to say about, so
    // agreement prints no line at all -- which is the property worth checking, rather than hoping a
    // printed `adaptive -> adaptive` never appears.
    assert!(
        !body.contains("config.strategy"),
        "an agreed field was printed as a difference:\n{body}"
    );
}

#[test]
fn the_arity_is_enforced_because_one_directory_is_not_a_comparison() {
    for (argv, message) in [
        (vec![], "needs exactly two archive directories"),
        (
            vec!["archive-sqrt_domain"],
            "needs exactly two archive directories",
        ),
        (
            vec![
                "archive-sqrt_domain",
                "archive-sqrt_domain",
                "archive-sqrt_domain-b80",
            ],
            "needs exactly two archive directories",
        ),
    ] {
        let mut cmd = aporia();
        cmd.arg("compare");
        for arg in &argv {
            cmd.arg(fixtures(arg));
        }
        let out = cmd.output().expect("aporia runs");
        let err = text(&out.stderr);
        assert_eq!(out.status.code(), Some(2), "{argv:?} -> {err}");
        assert!(err.contains(message), "{argv:?} -> {err}");
    }
}

#[test]
fn a_missing_directory_is_an_input_error_not_a_difference() {
    let out = aporia()
        .arg("compare")
        .arg(fixtures("archive-sqrt_domain"))
        .arg("no-such-archive-here")
        .output()
        .expect("aporia runs");
    assert_eq!(out.status.code(), Some(3));
    let err = text(&out.stderr);
    assert!(err.contains("cannot read the archive"), "{err}");
    assert!(err.contains("no-such-archive-here"), "{err}");
}

#[test]
fn compare_without_arguments_shows_the_usage_it_was_missing() {
    let out = aporia().arg("compare").output().expect("aporia runs");
    assert_eq!(out.status.code(), Some(2));
    let err = text(&out.stderr);
    assert!(err.contains("compare <dir-a> <dir-b>"), "{err}");
}

/// Copy an archive directory, so a test can tamper with a copy of a committed example without
/// touching the example itself.
fn copy_dir(from: &Path, to: &Path) {
    for entry in std::fs::read_dir(from).expect("read the archive") {
        let entry = entry.expect("entry");
        let path = entry.path();
        let target = to.join(entry.file_name());
        if path.is_dir() {
            std::fs::create_dir_all(&target).expect("create the copy's directory");
            copy_dir(&path, &target);
        } else {
            std::fs::copy(&path, &target).expect("copy the artefact");
        }
    }
}
