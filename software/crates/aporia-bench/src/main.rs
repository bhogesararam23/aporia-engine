//! `aporia-bench` — the command that produces the numbers.
//!
//! ```text
//! aporia-bench list                    what the corpus holds, and what each entry claims
//! aporia-bench verify [--grid N]       check every declared region against direct evaluation
//! aporia-bench run [--budgets ..]      sweep the strategies and write results/run-<date>.json
//! aporia-bench verdict <results.json>  read a results file back and print the comparison
//! ```
//!
//! Arguments are parsed by hand: three flags and four subcommands do not need a dependency, and the
//! help text is short enough to keep accurate. A bad argument is refused with the usage printed,
//! because a benchmark run that silently ignores `--budget` is a run whose numbers mean nothing.

use aporia_bench::{corpus, harness, truth};
use aporia_search::Strategy;
use aporia_store::Environment;
use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = args.first().map_or("help", String::as_str);
    let flags = &args[1..];
    let out = match command {
        "list" => run_list(flags),
        "verify" => run_verify(flags),
        "run" => run_run(flags),
        "verdict" => run_verdict(flags),
        "scan" => run_scan(flags),
        "explain" => run_explain(flags),
        "help" | "--help" | "-h" => {
            print!("{}", usage());
            Ok(0)
        }
        other => fail(&format!("unknown command {other:?}")),
    };
    match out {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("aporia-bench: {e}");
            eprint!("{}", usage());
            std::process::exit(2);
        }
    }
}

fn usage() -> &'static str {
    "usage: aporia-bench <command> [flags]\n\
     \x20 list                      entries, faults and declared regions\n\
     \x20 verify [--grid N]         check the declarations against direct evaluation\n\
     \x20 run [--budgets a,b,..] [--strategies adaptive,random,stratified]\n\
     \x20     [--seeds n,..] [--grid N] [--out DIR] [--only family/name,..]\n\
     \x20 verdict <results.json>   print the comparison from a recorded run\n\
     \x20 scan <family/name> [--samples N]  measure where the model's own rule switches\n"
}

fn fail(message: &str) -> Result<i32, String> {
    Err(message.to_string())
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

    /// Anything left that looks like a flag was not understood.
    fn reject_unknown(&self) -> Result<(), String> {
        match self.items.iter().find(|a| a.starts_with("--")) {
            Some(flag) => Err(format!("unknown flag {flag}")),
            None => Ok(()),
        }
    }
}

fn load_corpus() -> Result<Vec<corpus::Entry>, String> {
    let root = aporia_bench::corpus_root();
    if !root.exists() {
        return Err(format!(
            "no corpus at {}; set APORIA_BENCHMARKS or run from the software directory",
            root.display()
        ));
    }
    corpus::load(&root)
}

fn run_list(_flags: &[String]) -> Result<i32, String> {
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

fn run_verify(flags: &[String]) -> Result<i32, String> {
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

fn run_run(flags: &[String]) -> Result<i32, String> {
    let mut args = Args::new(flags);
    let budgets = parse_budgets(&args.list("--budgets")?)?;
    let strategies = parse_strategies(&args.list("--strategies")?)?;
    let seeds = parse_seeds(&args.list("--seeds")?)?;
    let only = args.list("--only")?;
    let grid = parse_grid(args.value("--grid")?)?;
    let out_dir = args.value("--out")?.map(PathBuf::from);
    let archive_dir = args
        .value("--archive")?
        .map_or_else(aporia_bench::archives_dir, PathBuf::from);
    args.reject_unknown()?;

    let entries = load_corpus()?;
    let selected: Vec<_> = entries
        .iter()
        .filter(|e| only.is_empty() || only.contains(&e.id()))
        .cloned()
        .collect();
    if selected.is_empty() {
        return Err("no corpus entry matched the selection".to_string());
    }

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

    let mut plan = harness::Plan {
        budgets,
        strategies,
        seeds,
        grid,
        archive_dir,
        ..harness::Plan::default()
    };
    if plan.budgets.is_empty() {
        plan.budgets = harness::Plan::default().budgets;
    }
    let mut environment = Environment::current();
    environment.rust_channel = std::env::var("APORIA_TOOLCHAIN").unwrap_or_default();
    environment.notes.push((
        "gpu".to_string(),
        "no NVIDIA device on this machine, so no CUDA path was measured".to_string(),
    ));
    environment.notes.push((
        "corpus".to_string(),
        aporia_bench::corpus_root().display().to_string(),
    ));

    let sweeps = harness::run_corpus(&selected, &plan);
    for s in &sweeps {
        println!("{}", s.row());
    }
    println!();
    print!(
        "{}",
        harness::verdict(&sweeps, &|id: &str| {
            selected
                .iter()
                .find(|e| e.id() == id)
                .map(|e| truth::Truth {
                    method: e.truth.method.clone(),
                    fault: e.truth.fault.clone(),
                    derivation: e.truth.derivation.clone(),
                    regions: e.truth.regions.clone(),
                    boundaries: e.truth.boundaries.clone(),
                    control: e.truth.control,
                    static_expected: e.truth.static_expected,
                    narrow: e.truth.narrow,
                    curved: e.truth.curved,
                    degenerate: e.truth.degenerate,
                    expects: e.truth.expects.clone(),
                })
        })
    );

    let results = harness::results_json(&sweeps, &selected, &plan, &environment);
    let dir = out_dir.unwrap_or_else(aporia_bench::results_dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let path = dir.join(format!("results-{stamp}.json"));
    std::fs::write(&path, results.to_pretty()).map_err(|e| format!("{}: {e}", path.display()))?;
    println!("\nresults written to {}", path.display());
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
        return Ok(vec![
            Strategy::Adaptive,
            Strategy::Stratified,
            Strategy::Random,
        ]);
    }
    items
        .iter()
        .map(|text| match text.as_str() {
            "adaptive" => Ok(Strategy::Adaptive),
            "stratified" => Ok(Strategy::Stratified),
            "random" => Ok(Strategy::Random),
            other => Err(format!("unknown strategy {other:?}")),
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

fn run_verdict(flags: &[String]) -> Result<i32, String> {
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
fn run_scan(flags: &[String]) -> Result<i32, String> {
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
        let crossings = aporia_bench::corpus::scan_axis(model, axis, samples, 1e-6);
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
fn run_explain(flags: &[String]) -> Result<i32, String> {
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
    let plan = aporia_bench::harness::Plan {
        budgets: vec![budget],
        ..Default::default()
    };
    match aporia_bench::harness::explain(entry, &plan, strategy[0], seed) {
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
