//! The benchmark commands reached as a library.
//!
//! These are the calls `aporia bench` and `aporia-bench` both make, so what is checked here is the
//! contract between them: which words are refused before anything runs, what a refusal says, and that
//! the usage a reader is handed names the program they actually typed. The measurement itself is tested
//! in `identity.rs`, `risk.rs` and `semantics.rs`; nothing here sweeps a corpus, which would be the same
//! 945 campaigns the other files already spend.

use aporia_bench::cli;

/// The message a command refused with, panicking with the caller's name for the command if it accepted.
fn refused(command: &str, argv: &[String]) -> String {
    let Err(message) = cli::dispatch("aporia-bench", command, argv) else {
        panic!(
            "`aporia-bench {command} {}` should have been refused",
            argv.join(" ")
        );
    };
    message
}

#[test]
fn an_unknown_command_names_the_ones_that_exist() {
    let e = refused("frobnicate", &[]);
    assert!(e.contains("frobnicate"), "{e}");
    for command in cli::COMMANDS {
        assert!(
            e.contains(command),
            "the refusal should list {command}: {e}"
        );
    }
}

#[test]
fn the_usage_text_addresses_the_program_the_reader_typed() {
    // The same text serves both binaries, so a hint that says `aporia-bench run …` while the reader
    // typed `aporia bench run …` is a hint they cannot follow.
    let harness = cli::usage("aporia-bench");
    let front_door = cli::usage("aporia bench");
    assert!(
        harness.starts_with("usage: aporia-bench <command>"),
        "{harness}"
    );
    assert!(
        front_door.starts_with("usage: aporia bench <command>"),
        "{front_door}"
    );
    for command in cli::COMMANDS {
        assert!(
            harness.contains(command) && front_door.contains(command),
            "usage does not list {command}"
        );
    }
    assert_ne!(harness, front_door, "only the program name may differ");
}

#[test]
fn a_flag_that_was_not_understood_refuses_the_whole_command() {
    // `--budget` is not a `run` flag; silently falling back to the default budget would report numbers
    // for an experiment nobody asked for.
    assert_eq!(
        refused("run", &["--budget".to_string(), "40".to_string()]),
        "unknown flag --budget"
    );
}

#[test]
fn an_argument_that_cannot_be_a_value_is_refused_before_the_corpus_is_read() {
    for (argv, needle) in [
        (vec!["--budgets".to_string()], "--budgets needs a value"),
        (
            vec!["--seeds".to_string(), "--grid".to_string()],
            "--seeds needs a value, not another flag",
        ),
        (
            vec!["--budgets".to_string(), "0".to_string()],
            "a budget of zero evaluates nothing",
        ),
        (
            vec!["--strategies".to_string(), "genetic".to_string()],
            "unknown strategy \"genetic\"",
        ),
        (
            vec!["--seeds".to_string(), "one".to_string()],
            "--seeds needs numbers, got one",
        ),
        (
            vec!["--grid".to_string(), "2".to_string()],
            "--grid must be at least 3",
        ),
    ] {
        let e = refused("run", &argv);
        assert!(e.contains(needle), "{} -> {e}", argv.join(" "));
    }
}

#[test]
fn a_positional_command_without_its_argument_says_what_it_needs() {
    assert!(
        refused("verdict", &[]).contains("verdict needs a results file"),
        "verdict should name what it is missing"
    );
    assert!(
        refused("scan", &[]).contains("scan needs an entry"),
        "scan should name what it is missing"
    );
    assert!(
        refused("explain", &[]).contains("explain needs an entry"),
        "explain should name what it is missing"
    );
    // `verdict` takes exactly one path, and a second one is a mistake rather than a second run.
    assert!(
        refused("verdict", &["a.json".to_string(), "b.json".to_string()])
            .contains("verdict takes one argument"),
        "two files are not one verdict"
    );
}

#[test]
fn an_entry_that_is_not_in_the_corpus_is_named_as_absent() {
    let e = refused("explain", &["no_such/family".to_string()]);
    assert!(e.contains("no corpus entry no_such/family"), "{e}");
}

#[test]
fn listing_the_corpus_through_the_library_reaches_the_real_corpus() {
    // The one test here that does the thing rather than checking its arguments: `aporia bench list` and
    // `aporia-bench list` resolve the same registry, so the front door cannot show a different corpus
    // from the harness that produced the published numbers. Output goes to stdout, as any command's
    // would; the assertion is on the status it returned.
    assert_eq!(
        cli::dispatch("aporia-bench", "list", &[]).ok(),
        Some(0),
        "listing the corpus must succeed"
    );
}
