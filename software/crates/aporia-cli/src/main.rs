//! `aporia` — the instrument, runnable without a benchmark.
//!
//! ```text
//! aporia run <model.ap> [--budget N]            analyse one model with the scalar interpreter
//! aporia run <model.ap> --program "<prog args>" analyse a model whose outputs come from a program
//! aporia replay <archive-dir>                   check an archive, then reproduce its run
//! aporia report <archive-dir>                   print a stored run without executing anything
//! aporia compare <dir-a> <dir-b>                say what differs between two stored runs
//! aporia bench <command> [flags]                the measurement harness, forwarded to aporia-bench
//! aporia help | --version                       this text, or which build this is
//! ```
//!
//! Arguments are parsed by hand, the way `aporia-bench` does it: two optional flags are not a reason
//! to take a dependency, and the usage text has to stay short enough to keep accurate. An unknown flag
//! is refused rather than ignored, because a run that silently dropped `--budget` would report an
//! analysis the caller did not ask for — and one that silently dropped `--program` would report a
//! map made by the wrong arithmetic entirely.
//!
//! Exit status, the same list `aporia help` prints: `0` ran and found nothing suspicious, `1` ran and
//! reported SUSPICIOUS regions, `2` bad usage, `3` the model could not be read, compiled or verified,
//! `4` the program stopped answering and the map is incomplete, `5` archive integrity failure, `6`
//! archive intact but the run did not reproduce, `7` the run finished and the archive could not be
//! written, `8` the two archives differ, `9` the two archives agree on everything comparable.

use aporia_cli::run::Exit;
use aporia_cli::{bench, compare, replay, report, run};
use aporia_search::Config;
use std::path::PathBuf;

/// The library default is a ladder-sized budget; a command line run should be answerable in the time
/// it takes to read one screen, and `--budget` is there for anyone who wants the other.
const DEFAULT_BUDGET: u64 = 640;

/// How long to wait for one answer from a program. A hung solver is a failed run, not a hang, and a
/// default that waits forever is a CLI that appears to be working.
const DEFAULT_TIMEOUT_MS: u64 = 5_000;

fn usage() -> &'static str {
    "usage: aporia <command> [flags]\n\
     \x20 run <model.ap> [--budget N] [--program \"<program> [args]\"] [--timeout MS]\n\
     \x20     [--archive DIR]\n\
     \x20     compile the model and run one campaign on it. A model that declares `output`\n\
     \x20     values is executed by the named program, one JSON request and response per line.\n\
     \x20 replay <archive-dir>        check an archive's integrity and reproduce its run\n\
     \x20 report <archive-dir>        print a stored run without executing anything\n\
     \x20 compare <dir-a> <dir-b>     say what differs between two stored runs\n\
     \x20     [--json] for the machine-readable form: every section, whether it could be\n\
     \x20     compared, and the counts — the text form prints only what moved\n\
     \x20 bench <command> [flags]    the measurement harness: list, verify, run, verdict, scan,\n\
     \x20                            explain (bare `bench` shows its own usage)\n\
     \x20 help                        show this text\n\
     \x20 version                     which build of aporia this is\n\
     exit: 0 clean, 1 suspicious regions reported, 2 usage, 3 model not usable,\n\
     \x20     4 the program stopped answering (the map above is incomplete),\n\
     \x20     5 archive integrity failure, 6 archive intact but the run did not reproduce,\n\
     \x20     7 the run finished and the archive could not be written,\n\
     \x20     8 the two archives differ, 9 the two archives agree on what could be compared\n"
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let exit = match args.first().map(String::as_str) {
        None => {
            eprint!("{}", usage());
            Exit::Usage
        }
        Some("run") => run_command(&args[1..]),
        Some("replay") => one_directory("replay", &args[1..], replay::command),
        Some("report") => one_directory("report", &args[1..], report::command),
        Some("compare") => {
            // `--json` selects the rendering, not the comparison: one `Comparison` object, two
            // documents. It is stripped here rather than passed to the reader because the arity of a
            // comparison is its meaning, and a flag must not be counted as a third archive.
            let json = args[1..].iter().any(|a| a == "--json");
            let positional: Vec<String> = args[1..]
                .iter()
                .filter(|a| *a != "--json")
                .cloned()
                .collect();
            let read_it = if json {
                compare::command_json
            } else {
                compare::command
            };
            two_directories("compare", &positional, read_it)
        }
        Some("bench") => {
            let stdout = std::io::stdout();
            let mut out = stdout.lock();
            bench::command(&args[1..], &mut out)
        }
        Some("help" | "--help" | "-h") => {
            print!("{}", usage());
            Exit::Clean
        }
        // The build a number came from is part of what makes it reproducible, and an archive's
        // environment block records the toolchain that wrote it. A reader holding a log line that
        // says only `aporia` needs the version beside it, so this costs one macro and no dependency.
        Some("version" | "--version" | "-V") => {
            println!("aporia {}", env!("CARGO_PKG_VERSION"));
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

/// True when the arguments ask what the command does rather than asking it to do something.
///
/// Every command answers this, because before it did, `aporia replay --help` read `--help` as the
/// archive directory and reported that no such archive existed — a usage question answered as a
/// missing file, with a status that meant "input not usable". Help is an argument the tool already
/// has, so it is checked before any argument is turned into a path.
fn help_requested(flags: &[String]) -> bool {
    flags
        .iter()
        .any(|a| a == "--help" || a == "-h" || a == "help")
}

/// Print the usage text for a command that was asked for it, and succeed.
fn show_help() -> Exit {
    print!("{}", usage());
    Exit::Clean
}

/// What `aporia run` was asked to do, once the words have been turned into values.
struct Args {
    path: PathBuf,
    budget: u64,
    /// Where to write the run's archive, if anywhere.
    archive: Option<PathBuf>,
    /// A program to execute the model with. `None` means the scalar interpreter, which is the
    /// default the way it should be: invisible, not assumed by the analysis.
    program: Option<String>,
    timeout_ms: u64,
}

/// Read the argument list. Parsing lives apart from running so that "you asked for `--timeout 0`" and
/// "the program stopped answering" cannot be confused by a reader, an exit status, or a future
/// maintainer: the first is a conversation about words, the second about a process.
fn parse_args(flags: &[String]) -> Result<Args, Exit> {
    let mut budget: Option<u64> = None;
    let mut program: Option<String> = None;
    let mut archive: Option<PathBuf> = None;
    let mut timeout_ms = DEFAULT_TIMEOUT_MS;
    let mut path: Option<PathBuf> = None;
    let mut i = 0;
    while i < flags.len() {
        let arg = flags[i].as_str();
        let value_of = match arg {
            "--budget" => Some("a number"),
            "--program" => Some("a command line"),
            "--timeout" => Some("a number of milliseconds"),
            "--archive" => Some("a directory"),
            _ => None,
        };
        if let Some(wants) = value_of {
            let Some(value) = flags.get(i + 1) else {
                eprintln!("aporia: {arg} needs {wants}");
                return Err(Exit::Usage);
            };
            match arg {
                "--budget" => {
                    let Ok(n) = value.parse::<u64>() else {
                        eprintln!("aporia: --budget needs a number, got {value}");
                        return Err(Exit::Usage);
                    };
                    budget = Some(n);
                }
                "--timeout" => {
                    let Ok(n) = value.parse::<u64>() else {
                        eprintln!("aporia: --timeout needs a number of milliseconds, got {value}");
                        return Err(Exit::Usage);
                    };
                    if n == 0 {
                        eprintln!(
                            "aporia: --timeout needs a positive number of milliseconds; 0 would mean \
                             'never wait', and a run that answers nothing is not a faster run"
                        );
                        return Err(Exit::Usage);
                    }
                    timeout_ms = n;
                }
                "--archive" => archive = Some(PathBuf::from(value)),
                _ => program = Some(value.clone()),
            }
            i += 2;
            continue;
        }
        if arg.starts_with('-') {
            eprintln!("aporia: unknown flag {arg}");
            eprint!("{}", usage());
            return Err(Exit::Usage);
        }
        if path.is_some() {
            eprintln!("aporia: run takes one model file");
            return Err(Exit::Usage);
        }
        path = Some(PathBuf::from(arg));
        i += 1;
    }
    let Some(path) = path else {
        eprintln!("aporia: run needs a model file, e.g. aporia run model.ap");
        eprint!("{}", usage());
        return Err(Exit::Usage);
    };
    Ok(Args {
        path,
        budget: budget.unwrap_or(DEFAULT_BUDGET),
        program,
        archive,
        timeout_ms,
    })
}

/// The shared one-argument shape of the two archive readers: `aporia replay <dir>` and
/// `aporia report <dir>`. Kept in one place so their usage messages cannot drift apart while doing
/// the same thing with the same argument.
fn one_directory<F>(name: &str, flags: &[String], read_it: F) -> Exit
where
    F: Fn(&std::path::Path, &mut dyn std::io::Write) -> Exit,
{
    if help_requested(flags) {
        return show_help();
    }
    let Some(first) = flags.first() else {
        eprintln!("aporia: {name} needs an archive directory, e.g. aporia {name} run-0001");
        eprint!("{}", usage());
        return Exit::Usage;
    };
    if flags.len() > 1 {
        eprintln!("aporia: {name} takes one archive directory");
        return Exit::Usage;
    }
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    read_it(&std::path::PathBuf::from(first), &mut out)
}

/// The two-archive shape of `aporia compare <a> <b>`. Separate from [`one_directory`] because the
/// arity is the command's meaning: a comparison of one archive with nothing is not a comparison.
fn two_directories<F>(name: &str, flags: &[String], read_it: F) -> Exit
where
    F: Fn(&std::path::Path, &std::path::Path, &mut dyn std::io::Write) -> Result<Exit, String>,
{
    if help_requested(flags) {
        return show_help();
    }
    // `--json` has already been taken out by the caller, so anything still shaped like a flag was not
    // understood. Refusing beats reading it as a directory name.
    if let Some(flag) = flags.iter().find(|a| a.starts_with('-')) {
        eprintln!("aporia: unknown flag {flag}");
        eprint!("{}", usage());
        return Exit::Usage;
    }
    let [first, second] = flags else {
        eprintln!(
            "aporia: {name} needs exactly two archive directories, e.g. aporia {name} run-0001 \
             run-0002"
        );
        eprint!("{}", usage());
        return Exit::Usage;
    };
    let a = std::path::PathBuf::from(first);
    let b = std::path::PathBuf::from(second);
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    match read_it(&a, &b, &mut out) {
        Ok(exit) => exit,
        Err(message) => {
            eprintln!("aporia: {message}");
            Exit::Input
        }
    }
}

fn run_command(flags: &[String]) -> Exit {
    if help_requested(flags) {
        return show_help();
    }
    let Ok(args) = parse_args(flags) else {
        return Exit::Usage;
    };
    let loaded = match run::load_model(&args.path) {
        Ok(loaded) => loaded,
        Err(message) => {
            eprintln!("aporia: {message}");
            return Exit::Input;
        }
    };
    let config = Config {
        budget: args.budget,
        ..Config::default()
    };
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let Some(command) = args.program.clone() else {
        if run::needs_adapter(&loaded.model) {
            eprintln!(
                "aporia: `{}` declares outputs the model does not compute; name the program that \
                 computes them with --program \"<program> [args]\"",
                args.path.display()
            );
            return Exit::Usage;
        }
        return run::run_and_report(
            &loaded,
            config,
            &mut aporia_runtime::Interp,
            "scalar interpreter (the model's own A-IR instructions)",
            args.archive.as_deref(),
            &mut out,
        );
    };
    // A program named for a model that computes its own values would be ignored: the execution path
    // answers for a model as a whole, so there is no point at which both could be asked. Refusing is
    // the only honest answer, and the same rule is why the DSL rejects a model that mixes `output`
    // with equations.
    if !run::needs_adapter(&loaded.model) {
        eprintln!(
            "aporia: `{}` computes its own outputs, so --program would be ignored; drop the flag or \
             declare the outputs with `output`",
            args.path.display()
        );
        return Exit::Usage;
    }
    let Some(spec) = aporia_adapter::ProgramSpec::parse(&command, args.timeout_ms) else {
        eprintln!("aporia: --program needs a command line, e.g. --program \"./solver\"");
        return Exit::Usage;
    };
    let display = spec.display();
    let mut engine = aporia_adapter::Program::new(spec);
    if let Err(e) = engine.start(loaded.model.outputs.len()) {
        eprintln!("aporia: {e}");
        return Exit::Program;
    }
    let exit = run::run_and_report(
        &loaded,
        config,
        &mut engine,
        &format!("program `{display}`, one answer per evaluation"),
        args.archive.as_deref(),
        &mut out,
    );
    // The report has already been written, because where a run stopped is worth seeing. What it is
    // not is a result: every point after the failure is a non-answer, and the exit status says so.
    match engine.failure() {
        Some(failure) => {
            eprintln!("aporia: {failure}");
            eprintln!("aporia: the map above is incomplete and must not be read as a result");
            Exit::Program
        }
        None => exit,
    }
}
