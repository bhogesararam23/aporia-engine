//! The command line, run as a process.
//!
//! These tests spawn the built `aporia` binary rather than calling into the library, because the
//! thing under test is the *command*: that a `.ap` file on disk reaches the campaign in
//! `aporia-search`, that the exit status distinguishes a clean model from a flagged one, and that a
//! refused run says so on stderr instead of printing a report that did not happen. A CLI that only
//! works when called from inside its own crate is not a front door.

use std::path::{Path, PathBuf};
use std::process::Command;

fn aporia() -> Command {
    Command::new(env!("CARGO_BIN_EXE_aporia"))
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

/// A model from the corpus, by absolute path: the point is that this is a real file another tool
/// measures, not a fixture written to make a test pass.
fn corpus_model(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../benchmarks")
        .join(rel)
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).to_string()
}

#[test]
fn a_real_ap_model_reaches_the_existing_campaign_and_is_flagged() {
    // `projectile_sign_mutant` is the corpus entry whose sign error the ladder localises at 40
    // evaluations. Through the CLI it has to produce the same kind of answer: a campaign that spent
    // its budget, an atlas with suspicious volume, and a finding whose loudest item is the model's
    // own violated rule.
    let out = aporia()
        .arg("run")
        .arg(corpus_model("aerospace/projectile_sign_mutant/model.ap"))
        .output()
        .expect("aporia runs");
    let body = text(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "stdout was:\n{body}");
    assert!(
        body.contains("model projectile_sign_mutant"),
        "the report does not name the model:\n{body}"
    );
    assert!(
        body.contains("campaign 640 evaluations"),
        "the campaign did not spend the default budget:\n{body}"
    );
    assert!(
        body.contains("loudest  physical") && body.contains("r >= 0 violated"),
        "the finding is not attributed to the model's own rule:\n{body}"
    );
    assert!(
        body.contains("note TRUSTED means") && body.contains("not mean proven correct"),
        "the report asserts trust without the caveat:\n{body}"
    );
    assert_eq!(
        text(&out.stderr),
        "",
        "a successful run must not write to stderr"
    );
}

#[test]
fn a_clean_model_exits_zero_and_claims_no_suspicion() {
    let out = aporia()
        .arg("run")
        .arg(fixture("clean.ap"))
        .arg("--budget")
        .arg("300")
        .output()
        .expect("aporia runs");
    let body = text(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "stdout was:\n{body}");
    assert!(body.contains("findings 0"), "{body}");
    assert!(
        body.contains("suspicious 0.0000"),
        "a clean model was reported with suspicious volume:\n{body}"
    );
    assert!(
        body.contains("trusted 1.0000"),
        "a clean model's domain was not fully trusted:\n{body}"
    );
}

#[test]
fn the_budget_flag_is_the_budget_the_campaign_spent() {
    // Proves `--budget` reaches the existing `Config` rather than being formatted into the header
    // twice: the evaluations line is counted by the campaign, not by the printer.
    let out = aporia()
        .arg("run")
        .arg(corpus_model("analytic/sqrt_domain/model.ap"))
        .arg("--budget")
        .arg("120")
        .output()
        .expect("aporia runs");
    let body = text(&out.stdout);
    assert!(body.contains("budget 120"), "{body}");
    assert!(body.contains("campaign 120 evaluations"), "{body}");
}

#[test]
fn a_non_numeric_budget_is_refused_before_anything_runs() {
    let out = aporia()
        .arg("run")
        .arg(fixture("clean.ap"))
        .arg("--budget")
        .arg("lots")
        .output()
        .expect("aporia runs");
    assert_eq!(out.status.code(), Some(2));
    assert!(text(&out.stderr).contains("--budget needs a number"));
    assert_eq!(text(&out.stdout), "", "a refused run printed a report");
}

#[test]
fn a_model_that_does_not_compile_says_which_line_and_why() {
    // The unit `banana` does not exist in the vocabulary. The front end's diagnostic has to reach the
    // reader with its position: a tool that answers "invalid" and nothing else is not inspectable,
    // and the same model text is what the corpus gate refuses on.
    let out = aporia()
        .arg("run")
        .arg(fixture("bad_unit.ap"))
        .output()
        .expect("aporia runs");
    assert_eq!(out.status.code(), Some(3));
    let err = text(&out.stderr);
    assert!(err.contains("did not compile"), "{err}");
    assert!(err.contains("is not a unit APORIA knows"), "{err}");
    assert!(err.contains("banana"), "{err}");
    assert_eq!(
        text(&out.stdout),
        "",
        "a run that never happened printed a report"
    );
}

#[test]
fn a_missing_file_is_refused_as_input_not_as_usage() {
    let out = aporia()
        .arg("run")
        .arg("no-such-model.ap")
        .output()
        .expect("aporia runs");
    assert_eq!(out.status.code(), Some(3));
    assert!(text(&out.stderr).contains("cannot read no-such-model.ap"));
}

#[test]
fn an_unknown_flag_is_refused_rather_than_ignored() {
    let out = aporia()
        .arg("run")
        .arg(fixture("clean.ap"))
        .arg("--precise")
        .output()
        .expect("aporia runs");
    assert_eq!(out.status.code(), Some(2));
    let err = text(&out.stderr);
    assert!(err.contains("unknown flag --precise"), "{err}");
    assert!(err.contains("usage:"), "{err}");
}

#[test]
fn a_missing_argument_names_the_command_instead_of_guessing() {
    let out = aporia().arg("run").output().expect("aporia runs");
    assert_eq!(out.status.code(), Some(2));
    assert!(text(&out.stderr).contains("run needs a model file"));
}

#[test]
fn the_report_never_points_at_an_archive_it_did_not_write() {
    // `.apx` findings carry a `Replay: aporia replay <archive dir>` line, which is true of an archived
    // run and false of this command. The report deliberately prints fewer lines rather than inviting
    // the reader to replay a file that was never written.
    let out = aporia()
        .arg("run")
        .arg(corpus_model("aerospace/projectile_sign_mutant/model.ap"))
        .output()
        .expect("aporia runs");
    let body = text(&out.stdout);
    assert!(body.contains("findings "), "{body}");
    assert!(
        !body.contains("Replay:"),
        "the report offers to replay an archive this run did not create:\n{body}"
    );
}
