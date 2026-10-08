//! The benchmark commands, as a library.
//!
//! This used to be `main.rs`. The behaviour is the same; what changed is that the commands are
//! reachable without being the program, which is the only way `aporia bench` and `aporia-bench` can be
//! one implementation rather than two that have agreed so far. The instrument's front door forwards
//! here; it does not re-derive a plan, select a corpus entry, or format a sweep row.
//!
//! Two shapes are worth stating, because they are the reason a bad measurement is hard to produce here:
//!
//! - **Nothing is measured before the ground truth is checked.** [`run_run`] verifies the selected
//!   entries against direct evaluation of each model's own rules and refuses to sweep when a declared
//!   region does not hold, because a detection rate scored against a wrong claim is worse than no
//!   number.
//! - **A rejected argument is an error, not a default.** [`Args`] leaves unrecognised flags in place so
//!   the caller can refuse the whole command: a benchmark run that silently ignored `--budgets` would
//!   report numbers for an experiment nobody asked for.
//!
//! Output goes to the process's stdout, like any command's. The functions return the exit status the
//! command means, and a `String` error for the caller to print with its own program name -- so the same
//! refusal reads `aporia-bench: …` from the harness and `aporia: …` from the front door.

use crate::corpus;
use crate::harness;
use aporia_search::Strategy;
use aporia_store::Environment;
use std::path::PathBuf;

/// Run one benchmark command. `program` is how the caller is being addressed (`aporia-bench`, or
/// `aporia bench`), so a usage line or a hint never points at a command the reader did not type.
///
/// The returned `i32` is the exit status; the `Err` is a message for the caller to print, conventionally
/// followed by [`usage`].
pub fn dispatch(program: &str, command: &str, flags: &[String]) -> Result<i32, String> {
    match command {
        "list" => run_list(flags),
        "verify" => run_verify(flags),
        "audit" => run_audit(flags),
        "run" => run_run(program, flags),
        "verdict" => run_verdict(flags),
        "scan" => run_scan(flags),
        "explain" => run_explain(flags),
        "e2" => run_e2(flags),
        "help" | "--help" | "-h" => {
            print!("{}", usage(program));
            Ok(0)
        }
        other => Err(format!(
            "unknown command {other:?}; {} needs one of {}",
            program,
            COMMANDS.join(", ")
        )),
    }
}

/// The commands this module implements, in the order the usage text lists them.
pub const COMMANDS: [&str; 8] = [
    "list", "verify", "audit", "run", "verdict", "scan", "explain", "e2",
];

#[must_use]
pub fn usage(program: &str) -> String {
    // The arm names come from the enum that owns them, the same discipline that killed the `halton`
    // alias: help text that lists a vocabulary the parser has stopped accepting is a second name.
    let strategies = aporia_search::Strategy::names().join(",");
    format!(
        "usage: {program} <command> [flags]\n\
     \x20 list                      entries, faults and declared regions\n\
     \x20 verify [--grid N]         check the declarations against direct evaluation\n\
     \x20 audit  [--grid N]         check the corpus as a corpus: duplicates, difficulty rungs,\n\
     \x20                             unmapped discrete values, refusals that did not refuse\n\
     \x20 run [--budgets a,b,..] [--strategies {strategies}]\n\
     \x20     [--seeds n,..] [--grid N] [--out DIR] [--only family/name,..]\n\
     \x20     [--plan protocols/<id>.json --arm <name>]  run a frozen experiment definition;\n\
     \x20        with --plan every flag that shapes what is measured is refused, because an\n\
     \x20        override of a pre-registration is what a pre-registration forbids\n\
     \x20     [--archive DIR] [--differential-every N] [--numerical-every N]\n\
     \x20     [--ablate channel,..]  drop a channel's READINGS, not its evaluations:\n\
     \x20        every arm still runs the same points at the same cost, so the arms are\n\
     \x20        comparable on cost and differ only in what was concluded. Names:\n\
     \x20        behavioral, physical, numerical, differential, sensitivity\n\
     \x20 verdict <results.json>   print the comparison from a recorded run\n\
     \x20 scan <family/name> [--samples N]  measure where the model's own rule switches\n\
     \x20 explain <family/name> [--budget N] [--strategy S] [--seed N]\n\
     \x20     one run opened up: the atlas, the bands, the findings and where the risk landed\n\
     \x20 e2 <full.json> <arm.json> [arm.json...] [--json]\n\
     \x20     compare ablation arms against the full arm, from committed results files;\n\
     \x20     refuses to pair documents that were not one experiment with two masks\n"
    )
}

/// Reads `--name value` pairs out of a mutable argument list, so an unknown flag survives to be
/// rejected by the caller rather than being silently consumed.
struct Args {
    items: Vec<String>,
}

impl Args {
    fn new(flags: &[String]) -> Self {
        Self {
            items: flags.to_vec(),
        }
    }

    fn value(&mut self, name: &str) -> Result<Option<String>, String> {
        let Some(at) = self.items.iter().position(|a| a == name) else {
            return Ok(None);
        };
        let Some(v) = self.items.get(at + 1) else {
            return Err(format!("{name} needs a value"));
        };
        if v.starts_with("--") {
            return Err(format!("{name} needs a value, not another flag"));
        }
        let value = v.clone();
        self.items.remove(at + 1);
        self.items.remove(at);
        Ok(Some(value))
    }

    fn list(&mut self, name: &str) -> Result<Vec<String>, String> {
        match self.value(name)? {
            Some(v) => Ok(v
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()),
            None => Ok(Vec::new()),
        }
    }

    /// A flag that stands alone, with no value. Its presence is the whole question, so it is
    /// consumed and reported rather than reaching for a value it never takes.
    fn flag(&mut self, name: &str) -> bool {
        if let Some(at) = self.items.iter().position(|a| a == name) {
            self.items.remove(at);
            true
        } else {
            false
        }
    }

    /// Anything left that looks like a flag was not understood.
    fn reject_unknown(&self) -> Result<(), String> {
        match self.items.iter().find(|a| a.starts_with("--")) {
            Some(flag) => Err(format!("unknown flag {flag}")),
            None => Ok(()),
        }
    }
}

fn load_corpus() -> Result<Vec<corpus::Entry>, String> {
    let root = crate::corpus_root();
    if !root.exists() {
        return Err(format!(
            "no corpus at {}; set APORIA_BENCHMARKS or run from the software directory",
            root.display()
        ));
    }
    corpus::load(&root)
}

pub fn run_list(_flags: &[String]) -> Result<i32, String> {
    let entries = load_corpus()?;
    println!(
        "{:<34} {:<24} {:<10} {:>7} {:>7}",
        "entry", "fault", "method", "regions", "truth %"
    );
    for e in &entries {
        let percent = match e.model.as_ref() {
            Some(m) => e.truth.total_fraction(m) * 100.0,
            None => 0.0,
        };
        println!(
            "{:<34} {:<24} {:<10} {:>7} {:>6.3}%",
            e.id(),
            e.truth.fault,
            e.truth.method,
            e.truth.regions.len(),
            percent
        );
        if e.model.is_none() {
            println!(
                "    does not compile: {}",
                e.diagnostics.first().cloned().unwrap_or_default()
            );
        }
    }
    println!("\n{} entries", entries.len());
    Ok(0)
}

pub fn run_verify(flags: &[String]) -> Result<i32, String> {
    let mut args = Args::new(flags);
    let grid = parse_grid(args.value("--grid")?)?;
    args.reject_unknown()?;
    let entries = load_corpus()?;
    let problems = corpus::verify(&entries, grid);
    for e in &entries {
        let status = if e.truth.static_expected {
            match e.diagnostics.first() {
                Some(d) => format!("caught before execution: {d}"),
                None => "compiled quietly, which a static entry must not do".to_string(),
            }
        } else if e.truth.control {
            "control, no declared region".to_string()
        } else {
            format!("{} declared region(s)", e.truth.regions.len())
        };
        println!("{:<34} {status}", e.id());
    }
    if problems.is_empty() {
        println!("\nall declarations hold on a {grid}-point-per-axis grid");
        return Ok(0);
    }
    println!("\n{} declaration problem(s):", problems.len());
    for p in &problems {
        println!("  {p}");
    }
    Ok(1)
}

/// The corpus-quality gate: what has to be true of the declarations *as a set*, which `verify`
/// cannot see because it reads each entry on its own.
fn run_audit(flags: &[String]) -> Result<i32, String> {
    let mut args = Args::new(flags);
    let grid = parse_grid(args.value("--grid")?)?;
    args.reject_unknown()?;
    let entries = load_corpus()?;
    let problems = crate::audit::audit(&entries, grid);
    for e in &entries {
        if e.truth.static_expected {
            println!("{:<34} refused before execution, as declared", e.id());
            continue;
        }
        if let Some(model) = e.model.as_ref() {
            // A control claims nothing, and the arithmetic returns `-0.0` for it, which prints as a
            // negative share of a domain and reads like a bug in the measure.
            let share = e.truth.total_fraction(model).abs();
            println!(
                "{:<34} {:<12} {:>9.4}% of its domain, {} region(s), {}",
                e.id(),
                e.truth.method,
                share * 100.0,
                e.truth.regions.len(),
                crate::audit::family_role(e)
            );
        }
    }
    if problems.is_empty() {
        println!(
            "\n{} entries pass the corpus audit on a {grid}-point-per-axis grid",
            entries.len()
        );
        return Ok(0);
    }
    println!("\n{} audit problem(s):", problems.len());
    for p in &problems {
        println!("  {p}");
    }
    Ok(1)
}

fn parse_grid(value: Option<String>) -> Result<usize, String> {
    match value {
        None => Ok(harness::Plan::default().grid),
        Some(text) => {
            let n = text
                .parse::<usize>()
                .map_err(|_| format!("--grid needs a number, got {text}"))?;
            if n < 3 {
                return Err("--grid must be at least 3 so the domain edges are covered".to_string());
            }
            Ok(n)
        }
    }
}

/// The channel names `--ablate` accepts, refused by name rather than defaulted.
///
/// A typo in an ablation list is not a harmless mistake: `--ablate behaviour` accepted as "nothing"
/// would report a full-instrument arm under an ablated arm's identity. So the vocabulary is
/// `Channel::parse`, the type that owns it, and a refusal lists it.
fn parse_channels(texts: &[String]) -> Result<Vec<aporia_evidence::Channel>, String> {
    let mut out = Vec::new();
    for text in texts {
        let Some(channel) = aporia_evidence::Channel::parse(text) else {
            return Err(format!(
                "--ablate does not know the channel {text:?}; the names are {}",
                aporia_evidence::Channel::names().join(", ")
            ));
        };
        if !out.contains(&channel) {
            out.push(channel);
        }
    }
    // A repeated name is a no-op on the mask, so saying so is better than silently accepting an
    // argument list that looks like it is doing more than it is.
    if out.len() == aporia_evidence::Channel::ALL.len() {
        return Err(
            "--ablate cannot silence every channel: an instrument with no evidence has nothing to \
             search on, and the campaign would report a domain of TRUSTED cells built on nothing"
                .to_string(),
        );
    }
    Ok(out)
}

pub fn run_run(program: &str, flags: &[String]) -> Result<i32, String> {
    let mut args = Args::new(flags);
    let plan_path = args.value("--plan")?;
    let arm_name = args.value("--arm")?;
    let budgets = parse_budgets(&args.list("--budgets")?)?;
    let strategies = parse_strategies(&args.list("--strategies")?)?;
    let seeds = parse_seeds(&args.list("--seeds")?)?;
    let only = args.list("--only")?;
    let grid = parse_grid(args.value("--grid")?)?;
    let out_dir = args.value("--out")?.map(PathBuf::from);
    let archive_dir = args
        .value("--archive")?
        .map_or_else(crate::archives_dir, PathBuf::from);
    // The evidence-sampling rates cost evaluations, so they are part of a measurement's definition
    // rather than a constant: being able to set them to zero is what separates "this channel changed
    // the result" from "the budget moved".
    let mut rate = |flag: &str, fallback: u64| -> Result<u64, String> {
        match args.value(flag)? {
            None => Ok(fallback),
            Some(t) => t
                .parse()
                .map_err(|_| format!("{flag} needs a number of evaluations, got {t}")),
        }
    };
    let defaults = harness::Plan::default();
    let differential_every = rate("--differential-every", defaults.differential_every)?;
    let numerical_every = rate("--numerical-every", defaults.numerical_every)?;
    let ablated = args.list("--ablate")?;
    let ablate = parse_channels(&ablated)?;
    args.reject_unknown()?;
    let entries = load_corpus()?;

    // A frozen protocol is a different kind of run. Its file states the arms, the entry list, the
    // ladder and the rates *before* the corpus it names exists, so the only way the measurement can
    // be trusted to match the pre-registration is for the runner to read the file rather than a
    // command line that could have drifted from it. Every flag that shapes what is measured is
    // therefore refused beside `--plan`; `--out` and `--archive`, which only say where bytes land,
    // are not.
    let (selected, plan, frozen) = if let Some(path) = plan_path {
        let frozen_run = frozen_run(&path, arm_name, archive_dir, flags, &entries)?;
        println!(
            "protocol {} arm {} — {} entries, {} arm(s) frozen, budgets {:?}, seeds {:?}",
            frozen_run.frozen.id,
            frozen_run.frozen.arm,
            frozen_run.entries.len(),
            frozen_run.arms,
            frozen_run.plan.budgets,
            frozen_run.plan.seeds
        );
        (frozen_run.entries, frozen_run.plan, Some(frozen_run.frozen))
    } else {
        if arm_name.is_some() {
            return Err("--arm selects an arm of a --plan, and no --plan was given".to_string());
        }
        let defaults = harness::Plan::default();
        (
            select(&only, &entries)?,
            harness::Plan {
                budgets: if budgets.is_empty() {
                    defaults.budgets
                } else {
                    budgets
                },
                strategies,
                seeds,
                grid,
                numerical_every,
                differential_every,
                ablate,
                archive_dir,
                ..harness::Plan::default()
            },
            None,
        )
    };
    let grid = plan.grid;

    // A corpus that has not been verified cannot produce a measurement, so the check runs first and
    // a failure stops the run rather than being noted afterwards.
    let problems = corpus::verify(&selected, grid);
    if !problems.is_empty() {
        eprintln!("ground truth does not hold, refusing to measure against it:");
        for p in &problems {
            eprintln!("  {p}");
        }
        return Ok(1);
    }
    // The corpus-quality gate is enforced rather than advisory. An entry that duplicates a mechanism,
    // sits at an unclaimed difficulty, or leaves a discrete value unclaimed produces rows that look
    // like data and are not — and a sweep is the expensive place to find that out.
    //
    // It runs over the whole corpus and not over the selection, because the properties it checks are
    // corpus-wide: an entry's difficulty rung names another entry, and a duplicate is a duplicate of
    // something. Auditing only the selection would refuse a `--only` run because the entry that pins
    // its difficulty was left out of it — which is the pilot's first lesson, and the wrong reason.
    // The stricter reading is the honest one: a subset of a corpus that is not sound is not sound
    // either, and the sweep inherits whatever the corpus got wrong.
    let unfit = crate::audit::audit(&entries, grid);
    if !unfit.is_empty() {
        eprintln!("the corpus fails its own audit, refusing to measure against it:");
        for p in &unfit {
            eprintln!("  {p}");
        }
        return Ok(1);
    }

    let sweeps = harness::run_corpus(&selected, &plan);
    report_run(program, &plan, frozen.as_ref(), &sweeps, &selected);

    write_results(&sweeps, &selected, &plan, out_dir, frozen.as_ref())
}

/// What a finished run says about itself, in the one sentence its plan can support.
///
/// A protocol arm and an ad-hoc ablation arm both have a single arm, so the strategy verdict would be
/// a sentence about a comparison that did not happen; and the strategy verdict itself is only correct
/// for a plan that measured more than one strategy.
fn report_run(
    program: &str,
    plan: &harness::Plan,
    frozen: Option<&harness::Frozen>,
    sweeps: &[harness::Sweep],
    selected: &[corpus::Entry],
) {
    for s in sweeps {
        println!("{}", s.row());
    }
    println!();
    match (frozen, plan.ablate.is_empty()) {
        (Some(frozen), _) => println!(
            "protocol {} arm {} — this file is one arm of a frozen comparison, not a strategy \
             verdict. Compare it against the protocol's full arm with `{program} e2 \
             <full-results.json> <this file>`.",
            frozen.id, frozen.arm
        ),
        (None, true) => print!(
            "{}",
            harness::verdict(sweeps, &|id: &str| {
                selected
                    .iter()
                    .find(|e| e.id() == id)
                    .map(|e| e.truth.clone())
            })
        ),
        (None, false) => {
            // The strategy verdict is the wrong sentence for an arm whose only difference from the run
            // beside it is the evidence it was allowed to keep: it would read "adaptive resolved,
            // neither baseline did" about an experiment with one arm. The ablation-aware line says
            // what this file is and how to compare it, which is the question the caller actually has.
            let silenced = plan
                .ablate
                .iter()
                .copied()
                .map(aporia_evidence::Channel::name)
                .collect::<Vec<_>>()
                .join(",");
            println!(
                "ablation arm: silenced {silenced}. This file is one arm of an ablation comparison, \
                 not a strategy comparison — compare it against the full arm's results with \
                 `{program} e2 <full-results.json> <this file>`"
            );
        }
    }
}

/// The flags that decide *what* a run measures, as opposed to where its bytes land. Beside `--plan`
/// they are refused rather than honoured: a pre-registration that can be overruled from the command
/// line is not a pre-registration.
const FROZEN_OVERRIDES: [&str; 8] = [
    "--budgets",
    "--strategies",
    "--seeds",
    "--grid",
    "--only",
    "--ablate",
    "--numerical-every",
    "--differential-every",
];

/// The corpus rows one command will measure.
fn select(only: &[String], entries: &[corpus::Entry]) -> Result<Vec<corpus::Entry>, String> {
    let selected: Vec<_> = entries
        .iter()
        .filter(|e| only.is_empty() || only.contains(&e.id()))
        .cloned()
        .collect();
    if selected.is_empty() {
        return Err("no corpus entry matched the selection".to_string());
    }
    Ok(selected)
}

/// What a `--plan` command resolves to.
struct FrozenRun {
    entries: Vec<corpus::Entry>,
    plan: harness::Plan,
    frozen: harness::Frozen,
    arms: usize,
}

/// What a `--plan` command means: the file's plan, one arm chosen by name, the entries the file
/// names, and the provenance that says which protocol and arm produced the numbers.
///
/// An entry the protocol names and the corpus lacks is an error rather than a smaller run: a
/// measurement taken over nine of seventeen entries is a different experiment, and the file that
/// froze it says seventeen.
fn frozen_run(
    path: &str,
    arm: Option<String>,
    archive_dir: PathBuf,
    flags: &[String],
    entries: &[corpus::Entry],
) -> Result<FrozenRun, String> {
    if let Some(flag) = FROZEN_OVERRIDES
        .iter()
        .find(|flag| flags.iter().any(|a| a == *flag))
    {
        return Err(format!(
            "{flag} cannot be used with --plan. The protocol file is the definition that was frozen \
             before this corpus existed, and overriding a piece of it from the command line is what \
             --plan exists to make impossible. Drop --plan to run an ad-hoc plan."
        ));
    }
    let protocol = harness::Protocol::load(&PathBuf::from(path))?;
    // The arm is chosen by name or the run does not happen: a file with eleven arms does not say
    // which measurement was wanted, and defaulting to the full instrument would let a run claim to
    // be an ablation arm it was not. `Protocol::arm` names the arms that do exist when the one
    // asked for is not among them.
    let (mut plan, frozen) = match arm {
        Some(text) => protocol.arm(&text)?,
        None => {
            return Err(format!(
                "--plan needs --arm, because a protocol with more than one arm does not say which \
                 measurement is wanted. {} has: {}",
                protocol.id,
                protocol.arm_names().join(", ")
            ));
        }
    };
    plan.archive_dir = archive_dir;
    let missing: Vec<&String> = protocol
        .entries
        .iter()
        .filter(|name| !entries.iter().any(|e| e.id() == **name))
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "protocol {} names entries the corpus does not have: {}",
            protocol.id,
            missing
                .iter()
                .map(|name| (*name).as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let selected: Vec<_> = entries
        .iter()
        .filter(|e| protocol.entries.iter().any(|name| *name == e.id()))
        .cloned()
        .collect();
    Ok(FrozenRun {
        entries: selected,
        plan,
        frozen,
        arms: protocol.arms.len(),
    })
}

/// What this build recorded about where the measurement happened. Provenance for the numbers, and
/// deliberately not part of their identity: the same plan measured elsewhere is the same experiment.
fn measurement_environment() -> Environment {
    let mut environment = Environment::current()
        .with_toolchain()
        .with_source_version();
    environment.notes.push((
        "gpu".to_string(),
        "no NVIDIA device on this machine, so no CUDA path was measured".to_string(),
    ));
    environment.notes.push((
        "corpus".to_string(),
        crate::corpus_root().display().to_string(),
    ));
    environment
}

/// Write the results document, under the name the measurement itself determines.
///
/// A rerun of a plan that was already measured is refused rather than written twice, because two files
/// with the same numbers and different names invite a reader to compare them, and silently replacing
/// one published number with another is how a measurement stops being evidence. `--out` to a fresh
/// directory is the way to keep both.
fn write_results(
    sweeps: &[harness::Sweep],
    selected: &[corpus::Entry],
    plan: &harness::Plan,
    out_dir: Option<PathBuf>,
    frozen: Option<&harness::Frozen>,
) -> Result<i32, String> {
    let results = harness::results_json(sweeps, selected, plan, &measurement_environment(), frozen);
    let dir = out_dir.unwrap_or_else(crate::results_dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let identity = harness::results_identity(&results)
        .ok_or("the results document carries no plan or entries, so it cannot be named")?;
    let path = dir.join(format!("results-{identity}.json"));
    if path.exists() {
        return Err(format!(
            "{} already holds this measurement (identity {identity}). The plan, the strategies, the \
             seeds and the ground truth are identical, so a second run could only differ in wall_ms; \
             pass --out DIR to keep both.",
            path.display()
        ));
    }
    std::fs::write(&path, results.to_pretty()).map_err(|e| format!("{}: {e}", path.display()))?;
    println!(
        "\nresults written to {}  identity {identity}",
        path.display()
    );
    Ok(0)
}

fn parse_budgets(items: &[String]) -> Result<Vec<u64>, String> {
    if items.is_empty() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for text in items {
        let v = text
            .parse::<u64>()
            .map_err(|_| format!("--budgets needs numbers, got {text}"))?;
        if v == 0 {
            return Err("a budget of zero evaluates nothing".to_string());
        }
        out.push(v);
    }
    out.sort_unstable();
    Ok(out)
}

fn parse_strategies(items: &[String]) -> Result<Vec<Strategy>, String> {
    if items.is_empty() {
        return Ok(Strategy::ALL.to_vec());
    }
    items
        .iter()
        // The enum parses its own names, so the harness and `aporia bench` cannot drift into two
        // vocabularies for one strategy — which is what `halton` was, accepted here in one reader of
        // `Strategy` and refused in the other.
        .map(|text| {
            Strategy::parse(text).ok_or_else(|| {
                format!(
                    "unknown strategy {text:?}; this tool knows {}",
                    Strategy::names().join(", ")
                )
            })
        })
        .collect()
}

fn parse_seeds(items: &[String]) -> Result<Vec<u64>, String> {
    if items.is_empty() {
        return Ok(vec![1]);
    }
    items
        .iter()
        .map(|text| {
            text.parse::<u64>()
                .map_err(|_| format!("--seeds needs numbers, got {text}"))
        })
        .collect()
}

pub fn run_verdict(flags: &[String]) -> Result<i32, String> {
    // The path is a positional argument here, unlike every other flag in this tool, because
    // `verdict <file>` is what a person types when they want to read a run back.
    let Some(path) = flags.first().cloned() else {
        return Err("verdict needs a results file".to_string());
    };
    if flags.len() > 1 {
        return Err(format!(
            "verdict takes one argument, got {}",
            flags.join(" ")
        ));
    }
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?;
    let value = aporia_store::Json::parse(&text).map_err(|e| format!("{path}: {e}"))?;
    let Some(sweeps) = value.get("sweeps").and_then(aporia_store::Json::as_array) else {
        return Err(format!("{path} has no sweeps section"));
    };
    // The file's own name is the measurement's identity, so a results file written before the field
    // existed can still be asked "which experiment are you" -- the answer comes from the schema, plan
    // and entries recorded inside it, not from the filename it happens to have.
    if let Some(identity) = harness::results_identity(&value) {
        let recorded = value.get("identity").is_some();
        println!(
            "identity  {identity}  {}",
            if recorded {
                "(as written)"
            } else {
                "(derived from the schema, plan and entries it records)"
            }
        );
    }
    for s in sweeps {
        let entry = s
            .get("entry")
            .and_then(aporia_store::Json::as_str)
            .unwrap_or("?");
        let strategy = s
            .get("strategy")
            .and_then(aporia_store::Json::as_str)
            .unwrap_or("?");
        let budget = |k: &str| s.get(k).and_then(aporia_store::Json::as_u64);
        println!(
            "{entry:<34} {strategy:<11} detect {:>6}  localise {:>6}  clean {:>6}",
            text_of(budget("detected_at_budget")),
            text_of(budget("localised_at_budget")),
            text_of(budget("clean_at_budget")),
        );
    }
    Ok(0)
}

/// Measure where each parameter's own rule flips. The numbers this prints are what a `truth.json`
/// boundary should quote, so a corpus entry's ground truth comes out of the model rather than from
/// somebody's algebra on a napkin.
pub fn run_scan(flags: &[String]) -> Result<i32, String> {
    let mut args = Args::new(flags);
    let samples = match args.value("--samples")? {
        Some(text) => text
            .parse::<usize>()
            .map_err(|_| format!("--samples needs a number, got {text}"))?,
        None => 2000,
    };
    args.reject_unknown()?;
    let Some(selector) = flags.first().cloned() else {
        return Err("scan needs an entry, e.g. scan ode/euler_decay".to_string());
    };
    let entries = load_corpus()?;
    let Some(entry) = entries.iter().find(|e| e.id() == selector) else {
        return Err(format!("no corpus entry {selector}"));
    };
    let Some(model) = entry.model.as_ref() else {
        return Err(format!("{} does not compile", entry.id()));
    };
    println!(
        "{}: fault {}, {} declared region(s)",
        entry.id(),
        entry.truth.fault,
        entry.truth.regions.len()
    );
    for (axis, p) in model.params.iter().enumerate() {
        let crossings = corpus::scan_axis(model, axis, samples, 1e-6);
        // Only the outermost crossings are printed: a cancellation boundary is quantised by
        // floating-point granularity, so it flickers over a wide band, and the list is thousands
        // long. The first and last are the ones a declaration needs.
        let shown: Vec<String> = match crossings.len() {
            0 => Vec::new(),
            n if n <= 6 => crossings.iter().map(|c| format!("{c}")).collect(),
            n => {
                let mut v = vec![format!("{}", crossings[0]), format!(".. {n} crossings ..")];
                v.push(format!("{}", crossings[n - 1]));
                if n > 6 {
                    v.push(format!("last three: {:?}", &crossings[n - 3..]));
                }
                v
            }
        };
        println!("  {:<8} crossings {shown:?}", p.name);
        for (i, declared) in entry.truth.boundaries.iter().enumerate() {
            if declared.axis != p.name {
                continue;
            }
            let nearest = crossings
                .iter()
                .map(|c| (c - declared.at).abs())
                .fold(f64::INFINITY, f64::min);
            println!(
                "      boundary {i} declared at {} with tolerance {}: nearest measured crossing is {nearest:.6} away",
                declared.at, declared.tolerance
            );
        }
    }
    Ok(0)
}

/// One run, printed in full: the atlas, the bands, the findings and where the risk landed.
pub fn run_explain(flags: &[String]) -> Result<i32, String> {
    let mut args = Args::new(flags);
    let budget = match args.value("--budget")? {
        Some(t) => t
            .parse::<u64>()
            .map_err(|_| format!("--budget needs a number, got {t}"))?,
        None => 640,
    };
    let strategy = match args.value("--strategy")? {
        Some(t) => parse_strategies(&[t])?,
        None => vec![aporia_search::Strategy::Adaptive],
    };
    let seed = match args.value("--seed")? {
        Some(t) => t
            .parse::<u64>()
            .map_err(|_| format!("--seed needs a number, got {t}"))?,
        None => 1,
    };
    args.reject_unknown()?;
    let Some(selector) = flags.first().cloned() else {
        return Err(
            "explain needs an entry, e.g. explain electromagnetics/rlc_resonance".to_string(),
        );
    };
    let entries = load_corpus()?;
    let Some(entry) = entries.iter().find(|e| e.id() == selector) else {
        return Err(format!("no corpus entry {selector}"));
    };
    let plan = harness::Plan {
        budgets: vec![budget],
        ..Default::default()
    };
    match harness::explain(entry, &plan, strategy[0], seed) {
        Some(text) => {
            print!("{text}");
            Ok(0)
        }
        None => Err(format!("{} cannot be run", entry.id())),
    }
}

fn text_of(value: Option<u64>) -> String {
    value.map_or_else(|| "-".to_string(), |v| v.to_string())
}

/// Compare ablation arms against the full arm, from committed results files.
///
/// The files are the source of truth rather than the console: every number printed here was read
/// back out of a document the harness wrote, so the comparison a researcher quotes months later is
/// the one the files support. The first path is the full arm; every later path is paired against
/// it, and a document that was not the same experiment with a different mask is refused.
pub fn run_e2(flags: &[String]) -> Result<i32, String> {
    let mut args = Args::new(flags);
    let machine = args.flag("--json");
    args.reject_unknown()?;
    let paths = args.items.clone();
    if paths.len() < 2 {
        return Err(
            "e2 needs the full arm's results file and at least one arm to compare with it"
                .to_string(),
        );
    }
    let mut arms = Vec::new();
    for path in &paths {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
        let value = aporia_store::Json::parse(&text).map_err(|e| format!("{path}: {e}"))?;
        arms.push(crate::e2::Arm::from_document(path, &value)?);
    }
    let (full, rest) = arms
        .split_first()
        .expect("at least two paths were checked above");
    let mut comparisons = Vec::new();
    for arm in rest {
        comparisons.push(crate::e2::compare(full, arm)?);
    }
    if machine {
        println!("{}", crate::e2::to_json(&comparisons).to_pretty());
    } else {
        print!("{}", crate::e2::report(full, &comparisons));
    }
    Ok(0)
}
