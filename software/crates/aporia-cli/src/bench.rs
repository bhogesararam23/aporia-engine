//! The benchmark, reached from the instrument's front door.
//!
//! This module owns exactly one decision -- what exit status a benchmark command's outcome becomes --
//! and forwards everything else to `aporia_bench::cli`. It is not a second measurement tool: there is
//! no argument parsing here, no corpus selection, no sweep and no results writer. That is the point. A
//! `bench` verb that re-derived the harness's rules would be free to diverge from them, and the
//! divergence would show up as two different numbers for one plan rather than as a failing test.
//!
//! ## Exit statuses
//!
//! The harness's own numbers are kept, so `aporia bench run …` and `aporia-bench run …` are
//! interchangeable for a caller that checks them:
//!
//! - `0` the command did what was asked. For `run`, a measurement was written; for `verify`, every
//!   declared region held against direct evaluation of the model's own rules.
//! - `1` nothing was measured because the corpus would not support a measurement -- a declared region
//!   did not hold. This is the harness's `Exit` meaning for `run`/`verify`, not `aporia run`'s "a
//!   SUSPICIOUS region was reported": for the benchmark, a wrong ground truth is the failure a caller
//!   has to see.
//! - `2` the command was refused: bad usage, an unreadable file, a missing corpus, or a real error.
//!
//! A sweep is expensive and writes into `software/benchmarks`, so nothing here defaults to one: `run`
//! with no `--out` writes to the published results directory and refuses to overwrite a measurement it
//! already made, which is the harness's rule rather than a courtesy of this module.

use aporia_bench::cli;

use crate::run::Exit;

/// `aporia bench <command> [flags]`.
pub fn command(flags: &[String], out: &mut dyn std::io::Write) -> Exit {
    let Some(sub) = flags.first() else {
        // The top-level `aporia` refuses a bare invocation the same way, so `bench` must not look like
        // the one command that accepts having nothing asked of it.
        let _ = write!(out, "{}", cli::usage("aporia bench"));
        return Exit::Usage;
    };
    match cli::dispatch("aporia bench", sub, &flags[1..]) {
        Ok(0) => Exit::Clean,
        // The harness's refusal to measure against a ground truth that does not hold. See the module
        // docs: this is not `aporia run`'s status 1.
        Ok(1) => Exit::Suspicious,
        // `dispatch` only returns 0 or 1 itself; anything else means a future command invented a
        // status this mapping has not been told about, and guessing would be worse than saying so.
        Ok(other) => {
            eprintln!(
                "aporia: the benchmark command returned status {other}, which `aporia bench` does not know how to report"
            );
            Exit::Usage
        }
        Err(message) => {
            eprintln!("aporia: {message}");
            eprint!("{}", cli::usage("aporia bench"));
            Exit::Usage
        }
    }
}
