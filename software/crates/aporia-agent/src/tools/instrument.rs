//! The Trust Atlas instrument, as one capability among the agent's tools.
//!
//! The instrument keeps its process boundary: the agent starts the `aporia` binary inside the
//! workspace and reads the archive the instrument wrote. The archive contract is the
//! instrument's, not the agent's -- a claim this application produces can still be replayed with
//! the instrument's own tools. The archive path never replaces an existing archive.

use super::result::{ArtifactRef, ErrorClass, ToolFailure, ToolOutput, ToolResult};
use super::{Tool, ToolContext, ToolMeta};
use crate::json;
use crate::scope::Risk;
use aporia_sandbox::{ExecSpec, ExecStatus, exec_in_with_cancel};
use aporia_store::Json;
use aporia_store::digest::sha256_hex;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

const OUTPUT_LIMIT: usize = 32 * 1024;
const INSTRUMENT_TIMEOUT_MS: u64 = 300_000;
const MAX_BUDGET: u64 = 100_000;
const MAX_ARCHIVE_FILES: u64 = 10_000;

/// Register the instrument tool.
pub fn register(registry: &mut super::Registry) -> Result<(), String> {
    registry.register(Box::new(InstrumentRun::new()))
}

struct InstrumentRun {
    meta: ToolMeta,
}

impl InstrumentRun {
    fn new() -> Self {
        Self {
            meta: ToolMeta::new(
                "instrument_run",
                "Run the Trust Atlas instrument on a .ap model inside the workspace and record the archive it writes. The instrument is a separate process; its archive contract is unchanged.",
                r#"{"type":"object","properties":{"model":{"type":"string","minLength":1},"budget":{"type":"integer","minimum":1,"maximum":100000},"archive":{"type":"string","minLength":1,"description":"archive directory relative to the workspace; defaults to archives/instrument"}},"required":["model","budget"],"additionalProperties":false}"#,
                Risk::Execute,
                OUTPUT_LIMIT,
            ),
        }
    }
}

impl Tool for InstrumentRun {
    fn meta(&self) -> &ToolMeta {
        &self.meta
    }

    // The instrument's contract is a sequence of checks followed by one execution and one
    // archive check; splitting it would scatter the order the checks happen in.
    #[allow(clippy::too_many_lines)]
    fn run(&self, context: &mut ToolContext, arguments: &Json) -> ToolResult {
        let fields = crate::json::fields(arguments).expect("validated object");
        let model = json::string(fields, "model")?.to_string();
        if !model.to_ascii_lowercase().ends_with(".ap") {
            return Err(ToolFailure::new(
                ErrorClass::InvalidArguments,
                format!("`{model}` is not a .ap model file"),
            ));
        }
        let budget = json::u64_value(fields, "budget")?;
        if budget == 0 || budget > MAX_BUDGET {
            return Err(ToolFailure::new(
                ErrorClass::InvalidArguments,
                format!("the budget must be between 1 and {MAX_BUDGET}"),
            ));
        }
        let model_path = context
            .sandbox()
            .resolve(&model)
            .map_err(|error| ToolFailure::new(ErrorClass::Denied, format!("{model}: {error}")))?;
        if !model_path.is_file() {
            return Err(ToolFailure::new(
                ErrorClass::NotFound,
                format!("`{model}` is not a file in the workspace"),
            ));
        }
        let archive = json::optional_string(fields, "archive")?
            .unwrap_or("archives/instrument")
            .to_string();
        let archive_path = context
            .sandbox()
            .resolve(&archive)
            .map_err(|error| ToolFailure::new(ErrorClass::Denied, format!("{archive}: {error}")))?;
        if archive_path.exists() {
            return Err(ToolFailure::new(
                ErrorClass::InvalidArguments,
                format!("the archive `{archive}` already exists; an archive is never replaced"),
            ));
        }

        let instrument = context.instrument().to_string_lossy().to_string();
        let spec = ExecSpec::new(vec![
            instrument,
            "run".to_string(),
            model.clone(),
            "--budget".to_string(),
            budget.to_string(),
            "--archive".to_string(),
            archive.clone(),
        ])
        .with_timeout(INSTRUMENT_TIMEOUT_MS);
        let outcome =
            exec_in_with_cancel(context.sandbox().root(), &spec, Some(context.cancel_flag()));
        let data = Json::object(vec![
            ("model", Json::text(&model)),
            ("budget", Json::count(budget)),
            ("archive", Json::text(&archive)),
            ("duration_ms", Json::count(outcome.duration_ms)),
            ("stdout_sha256", Json::text(outcome.stdout.sha256.clone())),
            ("stderr_sha256", Json::text(outcome.stderr.sha256.clone())),
        ]);
        match outcome.status {
            ExecStatus::Completed => {
                let code = outcome.exit_code.unwrap_or(-1);
                if code != 0 {
                    let excerpt = if outcome.stderr.text.trim().is_empty() {
                        outcome.stdout.text.trim().to_string()
                    } else {
                        outcome.stderr.text.trim().to_string()
                    };
                    return Err(ToolFailure::new(
                        ErrorClass::Execution,
                        format!("the instrument exited {code}: {excerpt}"),
                    )
                    .with_data(data));
                }
                let artifact = archive_artifact(&archive_path, &archive)?;
                let mut text = String::new();
                let _ = writeln!(
                    text,
                    "the instrument wrote the archive {archive} ({} files, manifest sha256 {})",
                    artifact.bytes, artifact.sha256
                );
                if !outcome.stdout.text.is_empty() {
                    let _ = writeln!(text, "stdout:\n{}", outcome.stdout.text.trim_end());
                }
                Ok(ToolOutput {
                    text,
                    data,
                    artifacts: vec![artifact],
                })
            }
            ExecStatus::LaunchFailed(reason) => Err(ToolFailure::new(
                ErrorClass::Execution,
                format!("the instrument could not be started: {reason}"),
            )
            .with_data(data)),
            ExecStatus::Timeout => Err(ToolFailure::new(
                ErrorClass::Timeout,
                format!("the instrument did not finish within {INSTRUMENT_TIMEOUT_MS} ms"),
            )
            .with_data(data)),
            ExecStatus::Cancelled => Err(ToolFailure::new(
                ErrorClass::Cancelled,
                "the instrument run was cancelled",
            )
            .with_data(data)),
        }
    }
}

/// Identify the archive the instrument wrote: the manifest's digest and the number of files.
fn archive_artifact(root: &Path, display: &str) -> Result<ArtifactRef, ToolFailure> {
    let manifest = root.join("manifest.json");
    if !manifest.is_file() {
        return Err(ToolFailure::new(
            ErrorClass::Execution,
            "the instrument exited successfully but wrote no archive manifest",
        ));
    }
    let bytes = fs::read(&manifest).map_err(|error| {
        ToolFailure::new(
            ErrorClass::Io,
            format!("the archive manifest cannot be read: {error}"),
        )
    })?;
    let files = count_files(root);
    Ok(ArtifactRef {
        kind: "instrument_archive".to_string(),
        path: format!("workspace/{}", display.replace('\\', "/")),
        sha256: sha256_hex(&bytes),
        bytes: files,
    })
}

fn count_files(root: &Path) -> u64 {
    let mut count = 0u64;
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                stack.push(entry.path());
                continue;
            }
            if kind.is_file() {
                count += 1;
                if count >= MAX_ARCHIVE_FILES {
                    return count;
                }
            }
        }
    }
    count
}
