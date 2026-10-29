//! `aporia investigate` — the agent workflow, with the instrument as a tool.
//!
//! The CLI owns words, permissions and exit statuses. It does not plan, execute or interpret; that
//! is `aporia-agent`. The provider is chosen here because a provider is configuration, not product
//! logic, and the instrument path is this executable because `instrument_run` is meant to be the
//! same binary a person would run.

use crate::run::Exit;
use aporia_agent::{
    AgentError, Approve, Outcome, RunConfig, Severity, provider::http::HttpProvider,
    provider::scripted::ScriptedProvider, run, verify_record,
};
use std::io::{IsTerminal, Write};
use std::path::PathBuf;

const DEFAULT_MAX_TURNS: usize = 16;
const DEFAULT_MAX_TOOL_CALLS: usize = 32;
const DEFAULT_TOTAL_TIMEOUT_MS: u64 = 300_000;
const DEFAULT_PROVIDER_TIMEOUT_MS: u64 = 120_000;
const DEFAULT_WORKDIR: &str = ".aporia/investigations";

fn usage() -> &'static str {
    "usage: aporia investigate <project-dir> --objective TEXT --provider PROVIDER\n\
     \x20   [--model NAME] [--workdir DIR] [--approve]\n\
     \x20   [--allow-exec PROGRAM]... [--max-turns N] [--max-tool-calls N] [--timeout MS]\n\
     \x20   inspect a project, propose an evidence-backed plan, and run the approved tasks in a\n\
     \x20   sandbox copy. PROVIDER is `scripted:<transcript.json>` or `http://host:port/v1`\n\
     \x20   (with --model). The plan is printed and refused unless --approve is given or stdin\n\
     \x20   is a terminal. --allow-exec names a program an exec task may run; with none, exec\n\
     \x20   tasks are rejected. The source is read; all execution happens in the copy.\n\
     \x20 investigate report <investigation-dir>\n\
     \x20   verify a finished record and print its report without executing anything\n\
     \x20 help | --help                  this text\n\
     exit: 0 completed without a critical/high finding, 1 completed with one,\n\
     \x20     2 usage, 3 project or sandbox unusable, 5 record integrity failure,\n\
     \x20     10 plan refused, 11 provider failure, 12 budget exhausted (record incomplete),\n\
     \x20     13 the record could not be written\n"
}

#[derive(Debug)]
enum ProviderArg {
    Scripted(PathBuf),
    Http(String),
}

struct Args {
    project: PathBuf,
    objective: String,
    provider: ProviderArg,
    model: Option<String>,
    workdir: PathBuf,
    approve: bool,
    allow_exec: Vec<String>,
    max_turns: usize,
    max_tool_calls: usize,
    timeout_ms: u64,
}

/// The command. `out` is for progress and reports, `err` for refusals.
pub fn command(flags: &[String], out: &mut dyn Write, err: &mut dyn Write) -> Exit {
    if flags.first().map(String::as_str) == Some("report") {
        return report_command(&flags[1..], out, err);
    }
    if flags
        .iter()
        .any(|arg| arg == "--help" || arg == "-h" || arg == "help")
    {
        let _ = write!(out, "{}", usage());
        return Exit::Clean;
    }
    let args = match parse(flags) {
        Ok(args) => args,
        Err(reason) => {
            let _ = writeln!(err, "aporia investigate: {reason}");
            let _ = write!(err, "{}", usage());
            return Exit::Usage;
        }
    };

    let provider: Box<dyn aporia_agent::ModelProvider> = match &args.provider {
        ProviderArg::Scripted(path) => match ScriptedProvider::load(path) {
            Ok(provider) => Box::new(provider),
            Err(error) => {
                let _ = writeln!(err, "aporia investigate: {error}");
                return Exit::Provider;
            }
        },
        ProviderArg::Http(base) => {
            let Some(model) = &args.model else {
                let _ = writeln!(
                    err,
                    "aporia investigate: an HTTP provider needs --model NAME"
                );
                return Exit::Usage;
            };
            match HttpProvider::new(base, model, DEFAULT_PROVIDER_TIMEOUT_MS) {
                Ok(provider) => Box::new(provider),
                Err(error) => {
                    let _ = writeln!(err, "aporia investigate: {error}");
                    return Exit::Provider;
                }
            }
        }
    };

    let approve = if args.approve {
        Approve::Flag
    } else if std::io::stdin().is_terminal() {
        Approve::Interactive
    } else {
        Approve::Never
    };
    let instrument = match std::env::current_exe() {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(
                err,
                "aporia investigate: cannot find this executable for the instrument: {error}"
            );
            return Exit::Input;
        }
    };
    let config = RunConfig {
        source: args.project.clone(),
        objective: args.objective.clone(),
        provider,
        approve,
        allow_exec: args.allow_exec.clone(),
        max_turns: args.max_turns,
        max_tool_calls: args.max_tool_calls,
        total_timeout_ms: args.timeout_ms,
        workdir: args.workdir.clone(),
        instrument,
    };
    match run(config, out) {
        Ok(report) => {
            let _ = writeln!(
                out,
                "\noutcome {}; report {}",
                report.outcome.as_str(),
                shown(&report.root.join("report.md"))
            );
            exit_for(report.outcome, &report.findings)
        }
        Err(error) => {
            let _ = writeln!(err, "aporia investigate: {error}");
            match error {
                AgentError::Input(_) | AgentError::Sandbox(_) => Exit::Input,
                AgentError::Record(_) => Exit::Record,
            }
        }
    }
}

fn exit_for(outcome: Outcome, findings: &[aporia_agent::Finding]) -> Exit {
    match outcome {
        Outcome::Completed => {
            if findings
                .iter()
                .any(|finding| matches!(finding.severity, Severity::Critical | Severity::High))
            {
                Exit::Suspicious
            } else {
                Exit::Clean
            }
        }
        Outcome::NoPlan => Exit::Clean,
        Outcome::Refused => Exit::Refused,
        Outcome::Failed => Exit::Provider,
        Outcome::BudgetExhausted => Exit::Budget,
    }
}

fn report_command(flags: &[String], out: &mut dyn Write, err: &mut dyn Write) -> Exit {
    if flags
        .iter()
        .any(|arg| arg == "--help" || arg == "-h" || arg == "help")
    {
        let _ = write!(out, "{}", usage());
        return Exit::Clean;
    }
    let positional: Vec<&String> = flags.iter().filter(|arg| !arg.starts_with("--")).collect();
    if positional.len() != 1 || flags.len() != positional.len() {
        let _ = writeln!(
            err,
            "aporia investigate report: expected exactly one directory"
        );
        return Exit::Usage;
    }
    match verify_record(std::path::Path::new(positional[0])) {
        Ok(summary) => {
            let _ = writeln!(out, "investigation {}", positional[0]);
            let _ = writeln!(out, "outcome {}", summary.outcome);
            let _ = writeln!(out, "provider {}", summary.provider);
            let _ = writeln!(
                out,
                "journal {} events, head {}",
                summary.events, summary.head
            );
            let report = std::path::Path::new(positional[0]).join("report.md");
            match std::fs::read_to_string(&report) {
                Ok(text) => {
                    let _ = writeln!(out, "\n{text}");
                    Exit::Clean
                }
                Err(error) => {
                    let _ = writeln!(
                        err,
                        "aporia investigate report: cannot read {}: {error}",
                        report.display()
                    );
                    Exit::Integrity
                }
            }
        }
        Err(error) => {
            let _ = writeln!(err, "aporia investigate report: {error}");
            Exit::Integrity
        }
    }
}

fn shown(path: &std::path::Path) -> String {
    let text = path.display().to_string();
    text.strip_prefix(r"\\?\")
        .map_or(text.clone(), str::to_string)
}

fn parse(flags: &[String]) -> Result<Args, String> {
    let mut project = None;
    let mut objective = None;
    let mut provider = None;
    let mut model = None;
    let mut workdir = PathBuf::from(DEFAULT_WORKDIR);
    let mut approve = false;
    let mut allow_exec = Vec::new();
    let mut max_turns = DEFAULT_MAX_TURNS;
    let mut max_tool_calls = DEFAULT_MAX_TOOL_CALLS;
    let mut timeout_ms = DEFAULT_TOTAL_TIMEOUT_MS;
    let mut index = 0;
    while index < flags.len() {
        let flag = flags[index].as_str();
        let value = |index: &mut usize, name: &str| -> Result<String, String> {
            *index += 1;
            flags
                .get(*index)
                .cloned()
                .ok_or_else(|| format!("{name} needs a value"))
        };
        match flag {
            "--objective" => objective = Some(value(&mut index, "--objective")?),
            "--provider" => {
                let text = value(&mut index, "--provider")?;
                if let Some(path) = text.strip_prefix("scripted:") {
                    if path.is_empty() {
                        return Err("--provider scripted: needs a transcript path".to_string());
                    }
                    provider = Some(ProviderArg::Scripted(PathBuf::from(path)));
                } else if text.starts_with("http://") {
                    provider = Some(ProviderArg::Http(text));
                } else {
                    return Err(
                        "--provider must be `scripted:<path>` or `http://host:port/v1`".to_string(),
                    );
                }
            }
            "--model" => model = Some(value(&mut index, "--model")?),
            "--workdir" => workdir = PathBuf::from(value(&mut index, "--workdir")?),
            "--approve" => approve = true,
            "--allow-exec" => allow_exec.push(value(&mut index, "--allow-exec")?),
            "--max-turns" => max_turns = number(&value(&mut index, "--max-turns")?, "--max-turns")?,
            "--max-tool-calls" => {
                max_tool_calls =
                    number(&value(&mut index, "--max-tool-calls")?, "--max-tool-calls")?;
            }
            "--timeout" => timeout_ms = number(&value(&mut index, "--timeout")?, "--timeout")?,
            other if other.starts_with("--") => {
                return Err(format!("unknown flag `{other}`"));
            }
            other => {
                if project.is_some() {
                    return Err(format!("unexpected second project path `{other}`"));
                }
                project = Some(PathBuf::from(other));
            }
        }
        index += 1;
    }
    let project = project.ok_or_else(|| "a project directory is required".to_string())?;
    let objective = objective.ok_or_else(|| "--objective is required".to_string())?;
    if objective.trim().is_empty() || objective.len() > 4096 {
        return Err("--objective must be 1 to 4096 characters".to_string());
    }
    let provider = provider.ok_or_else(|| "--provider is required".to_string())?;
    if matches!(provider, ProviderArg::Scripted(_)) && model.is_some() {
        return Err("--model is only used by an HTTP provider".to_string());
    }
    if max_turns == 0 || max_turns > 1_000 {
        return Err("--max-turns must be between 1 and 1000".to_string());
    }
    if max_tool_calls == 0 || max_tool_calls > 10_000 {
        return Err("--max-tool-calls must be between 1 and 10000".to_string());
    }
    if timeout_ms == 0 || timeout_ms > 86_400_000 {
        return Err("--timeout must be between 1 and 86400000 milliseconds".to_string());
    }
    if allow_exec.iter().any(|program| {
        program.is_empty()
            || program.contains('/')
            || program.contains('\\')
            || program.contains(' ')
    }) {
        return Err("--allow-exec takes a bare program name".to_string());
    }
    Ok(Args {
        project,
        objective,
        provider,
        model,
        workdir,
        approve,
        allow_exec,
        max_turns,
        max_tool_calls,
        timeout_ms,
    })
}

fn number<T>(text: &str, flag: &str) -> Result<T, String>
where
    T: std::str::FromStr,
{
    text.parse().map_err(|_| format!("{flag} needs a number"))
}
