//! `aporia run` with a program on the other end of the pipe.
//!
//! These spawn the real `aporia` binary and a real second process (`aporia-test-program`, which
//! speaks the protocol through the same code the adapter's tests use). What is under test is the
//! command's judgement, not the analysis: which combinations of model and program it accepts, what it
//! says when the program cannot be used or cannot be launched, and whether the report says honestly
//! where the numbers came from.

use std::path::{Path, PathBuf};
use std::process::Command;

const PROGRAM: &str = env!("CARGO_BIN_EXE_aporia-test-program");

fn aporia(mode: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_aporia"));
    if !mode.is_empty() {
        // The CLI inherits its environment and passes it to the program it launches, which is how a
        // solver picks up its own settings — so the mode reaches the grandchild unchanged.
        command.env("APORIA_PROGRAM_MODE", mode);
    }
    command
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).to_string()
}

#[test]
fn an_external_model_runs_from_the_command_line_through_a_real_program() {
    let out = aporia("answer")
        .arg("run")
        .arg(fixture("beam.ap"))
        .arg("--program")
        .arg(PROGRAM)
        .arg("--budget")
        .arg("160")
        .output()
        .expect("aporia runs");
    let body = text(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "stdout was:\n{body}");
    assert!(
        body.contains(&format!("execution  program `{PROGRAM}`")),
        "the report does not say where the values came from:\n{body}"
    );
    assert!(
        body.contains("deflection") && body.contains("violated"),
        "the model's own rule is missing from the report:\n{body}"
    );
    assert!(
        body.contains("campaign 160 evaluations"),
        "the budget did not reach the campaign:\n{body}"
    );
    // A region, not a wall of suspicion: the program misbehaves past 60 N and is fine below it.
    let suspicious = body
        .lines()
        .find(|l| l.starts_with("atlas"))
        .and_then(|l| l.split("suspicious ").nth(1))
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap_or_else(|| panic!("no atlas line:\n{body}"))
        .parse::<f64>()
        .expect("suspicious volume parses");
    assert!(
        suspicious > 0.0 && suspicious < 0.9,
        "suspicious volume {suspicious} does not describe a region"
    );
}

#[test]
fn the_interpreted_path_still_says_it_used_the_interpreter() {
    // The `.ap` workflow the previous commit shipped must be unchanged apart from the provenance
    // line, which is the one thing that had to be added when a second execution path became
    // possible: a reader of a Trust Atlas should never have to guess who did the arithmetic.
    let out = aporia("")
        .arg("run")
        .arg(fixture("clean.ap"))
        .arg("--budget")
        .arg("120")
        .output()
        .expect("aporia runs");
    let body = text(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "stdout was:\n{body}");
    assert!(body.contains("execution  scalar interpreter"), "{body}");
    assert_eq!(text(&out.stderr), "");
}

#[test]
fn an_external_model_without_a_program_says_which_flag_is_missing() {
    let out = aporia("")
        .arg("run")
        .arg(fixture("beam.ap"))
        .output()
        .expect("aporia runs");
    assert_eq!(out.status.code(), Some(2));
    let err = text(&out.stderr);
    assert!(err.contains("--program"), "{err}");
    assert!(err.contains("does not compute"), "{err}");
    assert_eq!(
        text(&out.stdout),
        "",
        "a run that never executed produced a report"
    );
}

#[test]
fn a_program_offered_for_a_model_that_computes_its_own_values_is_refused() {
    // Accepting it would be worse than ignoring it: the report would say "program" while the
    // interpreter produced the numbers. The CLI refuses, and the reason is the same one the DSL gives
    // for rejecting a model that mixes `output` with equations.
    let out = aporia("")
        .arg("run")
        .arg(fixture("clean.ap"))
        .arg("--program")
        .arg(PROGRAM)
        .output()
        .expect("aporia runs");
    assert_eq!(out.status.code(), Some(2));
    let err = text(&out.stderr);
    assert!(err.contains("would be ignored"), "{err}");
    assert_eq!(text(&out.stdout), "");
}

#[test]
fn a_program_that_dies_mid_run_exits_four_and_calls_the_map_incomplete() {
    let out = aporia("exit")
        .arg("run")
        .arg(fixture("beam.ap"))
        .arg("--program")
        .arg(PROGRAM)
        .arg("--budget")
        .arg("60")
        .output()
        .expect("aporia runs");
    assert_eq!(
        out.status.code(),
        Some(4),
        "stdout was:\n{}",
        text(&out.stdout)
    );
    let err = text(&out.stderr);
    assert!(err.contains("stopped answering"), "{err}");
    assert!(err.contains("must not be read as a result"), "{err}");
    // The report is still on stdout — where the run broke is information — but it is not a verdict,
    // and the exit status is the part a script reads.
    assert!(
        text(&out.stdout).contains("execution  program"),
        "the report should still name its program"
    );
}

#[test]
fn a_hanging_program_is_stopped_by_the_timeout_flag() {
    let started = std::time::Instant::now();
    let out = aporia("hang")
        .arg("run")
        .arg(fixture("beam.ap"))
        .arg("--program")
        .arg(PROGRAM)
        .arg("--timeout")
        .arg("200")
        .arg("--budget")
        .arg("60")
        .output()
        .expect("aporia runs");
    let elapsed = started.elapsed();
    assert_eq!(
        out.status.code(),
        Some(4),
        "stdout was:\n{}",
        text(&out.stdout)
    );
    assert!(
        text(&out.stderr).contains("did not answer within 200 ms"),
        "{}",
        text(&out.stderr)
    );
    assert!(
        elapsed.as_secs() < 15,
        "the timeout did not bound the run: {elapsed:?}"
    );
}

#[test]
fn a_program_that_cannot_be_started_is_status_four_with_its_name() {
    let out = aporia("")
        .arg("run")
        .arg(fixture("beam.ap"))
        .arg("--program")
        .arg("aporia-there-is-no-such-program")
        .output()
        .expect("aporia runs");
    assert_eq!(out.status.code(), Some(4));
    let err = text(&out.stderr);
    assert!(err.contains("cannot start"), "{err}");
    assert!(err.contains("aporia-there-is-no-such-program"), "{err}");
    assert_eq!(text(&out.stdout), "", "a launch failure printed a report");
}

#[test]
fn a_timeout_of_zero_is_refused_rather_than_trusted_to_mean_forever() {
    let out = aporia("")
        .arg("run")
        .arg(fixture("beam.ap"))
        .arg("--program")
        .arg(PROGRAM)
        .arg("--timeout")
        .arg("0")
        .output()
        .expect("aporia runs");
    assert_eq!(out.status.code(), Some(2));
    assert!(
        text(&out.stderr).contains("positive number of milliseconds"),
        "{}",
        text(&out.stderr)
    );
}

#[test]
fn the_executable_path_of_a_missing_model_is_still_input_error_three() {
    // Status 3 and status 4 answer different questions — "your model is unusable" versus "the
    // program stopped" — and a script that retries a solver should not be triggered by a typo in a
    // file name.
    let missing = Path::new("no-such-model.ap");
    let out = aporia("")
        .arg("run")
        .arg(missing)
        .arg("--program")
        .arg(PROGRAM)
        .output()
        .expect("aporia runs");
    assert_eq!(out.status.code(), Some(3));
    assert!(text(&out.stderr).contains("cannot read"));
}
