//! `aporia-bench` — the command that produces the numbers.
//!
//! ```text
//! aporia-bench list                    what the corpus holds, and what each entry claims
//! aporia-bench verify [--grid N]       check every declared region against direct evaluation
//! aporia-bench run [--budgets ..]      sweep the strategies and write results-<identity>.json
//! aporia-bench verdict <results.json>  read a results file back and print the comparison
//! aporia-bench scan <family/name>      measure where the model's own rule switches
//! aporia-bench explain <family/name>   one run opened up
//! ```
//!
//! The commands themselves live in `aporia_bench::cli`; this binary is one call to `cli::dispatch` plus
//! the process's exit status, so `aporia bench …` and `aporia-bench …` cannot drift into being two
//! different measurement tools. Arguments are parsed by hand in that module: a handful of flags and six
//! subcommands do not need a dependency, and a bad argument is refused with the usage printed, because a
//! benchmark run that silently ignores `--budget` is a run whose numbers mean nothing.

use aporia_bench::cli;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = args.first().map_or("help", String::as_str);
    match cli::dispatch("aporia-bench", command, &args[1..]) {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("aporia-bench: {e}");
            eprint!("{}", cli::usage("aporia-bench"));
            std::process::exit(2);
        }
    }
}
