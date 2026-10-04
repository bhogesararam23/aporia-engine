//! `aporia` — the instrument, runnable without a benchmark.
//!
//! ```text
//! aporia run <model.ap> [--budget N]   analyse one model and print the atlas summary
//! aporia help                          this text
//! ```
//!
//! Arguments are parsed by hand, the way `aporia-bench` does it: one optional flag is not a reason to
//! take a dependency, and the usage text has to stay short enough to keep accurate. An unknown flag is
//! refused rather than ignored, because a run that silently dropped `--budget` would report an
//! analysis the caller did not ask for.
//!
//! Exit status: `0` ran and found nothing suspicious, `1` ran and reported SUSPICIOUS regions, `2`
//! bad usage, `3` the model could not be read, compiled or verified.

use aporia_cli::run::{self, Exit};
use aporia_search::Config;
use std::path::PathBuf;

/// The library default is a ladder-sized budget; a command line run should be answerable in the time
/// it takes to read one screen, and `--budget` is there for anyone who wants the other.
const DEFAULT_BUDGET: u64 = 640;

fn usage() -> &'static str {
    "usage: aporia <command> [flags]\n\
     \x20 run <model.ap> [--budget N]   compile the model and run one campaign on it\n\
     \x20 help                        show this text\n\
     exit: 0 clean, 1 suspicious regions reported, 2 usage, 3 model not usable\n"
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let exit = match args.first().map(String::as_str) {
        None => {
            eprint!("{}", usage());
            Exit::Usage
        }
        Some("run") => run_command(&args[1..]),
        Some("help" | "--help" | "-h") => {
            print!("{}", usage());
            Exit::Clean
        }
        Some(other) => {
            eprintln!("aporia: unknown command {other:?}");
            eprint!("{}", usage());
            Exit::Usage
        }
    };
    std::process::exit(exit.code());
}

fn run_command(flags: &[String]) -> Exit {
    let mut budget: Option<u64> = None;
    let mut path: Option<PathBuf> = None;
    let mut i = 0;
    while i < flags.len() {
        let arg = flags[i].as_str();
        if arg == "--budget" {
            let Some(value) = flags.get(i + 1) else {
                eprintln!("aporia: --budget needs a number");
                return Exit::Usage;
            };
            let Ok(n) = value.parse::<u64>() else {
                eprintln!("aporia: --budget needs a number, got {value}");
                return Exit::Usage;
            };
            budget = Some(n);
            i += 2;
            continue;
        }
        if arg.starts_with('-') {
            eprintln!("aporia: unknown flag {arg}");
            eprint!("{}", usage());
            return Exit::Usage;
        }
        if path.is_some() {
            eprintln!("aporia: run takes one model file");
            return Exit::Usage;
        }
        path = Some(PathBuf::from(arg));
        i += 1;
    }
    let Some(path) = path else {
        eprintln!("aporia: run needs a model file, e.g. aporia run model.ap");
        eprint!("{}", usage());
        return Exit::Usage;
    };
    let loaded = match run::load_model(&path) {
        Ok(loaded) => loaded,
        Err(message) => {
            eprintln!("aporia: {message}");
            return Exit::Input;
        }
    };
    let config = Config {
        budget: budget.unwrap_or(DEFAULT_BUDGET),
        ..Config::default()
    };
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    run::analyse(&loaded, config, &mut out)
}
