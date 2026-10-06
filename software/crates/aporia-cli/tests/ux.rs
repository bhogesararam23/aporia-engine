//! What the command says when it is asked what it says.
//!
//! Part of a front door is being answerable: a person typing `--help` at a command they have never
//! used, or trying to record which build produced a number they are quoting. Before this file, no
//! test in the repository exercised `--help` on any command — and the behaviour it now pins was a
//! trap: `aporia replay --help` read `--help` as the archive directory and answered that no such
//! archive existed, with the exit status that means "the input is not usable". A question about words
//! answered as a missing file is the kind of confusion that makes a tool feel broken when it is only
//! unwelcoming.

use std::path::Path;
use std::process::Command;

fn aporia() -> Command {
    Command::new(env!("CARGO_BIN_EXE_aporia"))
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).to_string()
}

fn fixture(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

#[test]
fn every_command_answers_a_request_for_help_and_exits_clean() {
    for command in ["run", "replay", "report", "compare", "bench", "help"] {
        for flag in ["--help", "-h"] {
            let argv = if command == "help" {
                vec![flag.to_string()]
            } else {
                vec![command.to_string(), flag.to_string()]
            };
            let out = aporia().args(&argv).output().expect("aporia runs");
            let body = text(&out.stdout);
            assert_eq!(
                out.status.code(),
                Some(0),
                "`aporia {}` should answer, not refuse: stdout {body:?} stderr {:?}",
                argv.join(" "),
                text(&out.stderr)
            );
            assert!(
                body.contains("usage: aporia") || body.contains("usage: aporia bench"),
                "`aporia {}` printed no usage: {body:?}",
                argv.join(" ")
            );
            assert!(
                out.stderr.is_empty(),
                "help went to stderr for `{}`: {:?}",
                argv.join(" "),
                text(&out.stderr)
            );
        }
    }
}

#[test]
fn a_help_flag_is_never_mistaken_for_an_archive_directory() {
    // The specific old behaviour: `replay --help` looked for a directory named `--help` and reported
    // an unusable input. A flag is not a path, and answering it must not depend on the filesystem.
    for command in ["replay", "report"] {
        let out = aporia()
            .arg(command)
            .arg("--help")
            .output()
            .expect("aporia runs");
        let body = text(&out.stdout);
        assert_eq!(
            out.status.code(),
            Some(0),
            "`aporia {command} --help` exited with {:?}",
            out.status.code()
        );
        assert!(
            !body.contains("--help") || body.contains("usage:"),
            "`aporia {command} --help` talked about the flag as a file: {body}"
        );
    }
    // And `compare`, whose arity check is where a stray argument is most easily read as a path.
    let out = aporia()
        .arg("compare")
        .arg(fixture("archive-sqrt_domain"))
        .arg("--help")
        .output()
        .expect("aporia runs");
    assert_eq!(out.status.code(), Some(0), "compare --help refused");
}

#[test]
fn a_missing_argument_still_refuses_with_status_two() {
    // Answering help must not have swallowed the refusals that were already right.
    for (argv, needle) in [
        (vec!["run"], "run needs a model file"),
        (vec!["replay"], "replay needs an archive directory"),
        (vec!["report"], "report needs an archive directory"),
        (
            vec!["compare", "one-only"],
            "compare needs exactly two archive directories",
        ),
    ] {
        let asked = argv.join(" ");
        let out = aporia().args(argv).output().expect("aporia runs");
        assert_eq!(out.status.code(), Some(2), "`aporia {asked}` -> {out:?}");
        let err = text(&out.stderr);
        assert!(err.contains(needle), "`aporia {asked}` said: {err}");
    }
}

#[test]
fn which_build_this_is_can_be_asked() {
    // The version is the workspace's own, read at compile time rather than from a dependency, and it
    // is the same value an archive records in its manifest's `tool_version` field. A reader quoting a
    // number needs to be able to say which program made it.
    let out = aporia().arg("version").output().expect("aporia runs");
    assert_eq!(out.status.code(), Some(0));
    let body = text(&out.stdout);
    assert!(body.starts_with("aporia 0."), "not a version line: {body}");
    for flag in ["--version", "-V"] {
        let same = aporia().arg(flag).output().expect("aporia runs");
        assert_eq!(same.status.code(), Some(0), "{flag} refused");
        assert_eq!(text(&same.stdout), body, "{flag} disagrees with `version`");
    }
}

#[test]
fn a_zero_timeout_is_refused_in_one_readable_sentence() {
    // This message used to contain a run of literal spaces from a line continuation that was never
    // read as prose: "0 would mean                              'never wait'". The refusal is the
    // same one; the sentence is now the one that was meant to be written.
    let out = aporia()
        .arg("run")
        .arg(fixture("clean.ap"))
        .args(["--timeout", "0"])
        .output()
        .expect("aporia runs");
    assert_eq!(out.status.code(), Some(2));
    let err = text(&out.stderr);
    assert!(
        err.contains("0 would mean 'never wait'"),
        "the refusal is not one sentence: {err}"
    );
    assert!(
        !err.contains("  "),
        "the refusal still carries the run of spaces: {err:?}"
    );
}
