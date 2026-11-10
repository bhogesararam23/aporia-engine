//! Running a project program inside the workspace, under the approved scope.
//!
//! The model names a program and its arguments; the host decides whether that program may run,
//! where it may run, how long it may take, what it may see of the environment, and what counts
//! as its output. There is no shell: the argv is the command, so a separator in an argument is a
//! character in an argument. A nonzero exit is a failure the model can read and react to, with
//! the captured streams and their digests attached -- never a silent success.

use super::result::{ArtifactRef, ErrorClass, ToolFailure, ToolOutput, ToolResult};
use super::{Tool, ToolContext, ToolMeta};
use crate::json;
use crate::scope::{Risk, validate_program};
use aporia_sandbox::{ExecSpec, ExecStatus, exec_in_with_cancel};
use aporia_store::Json;
use aporia_store::digest::sha256_hex;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

const OUTPUT_LIMIT: usize = 32 * 1024;
const DEFAULT_TIMEOUT_MS: u64 = 60_000;
const MAX_TIMEOUT_MS: u64 = 300_000;
const DEFAULT_OUTPUT_LIMIT: u64 = 256 * 1024;
const MAX_OUTPUT_LIMIT: u64 = 1024 * 1024;
const MAX_ARGV: usize = 32;
const MAX_ENV: usize = 16;
const MAX_ARTIFACTS: usize = 16;
const EXCERPT: usize = 2000;

/// Register the execution tool.
pub fn register(registry: &mut super::Registry) -> Result<(), String> {
    registry.register(Box::new(RunCommand::new()))
}

/// The variables a project program may inherit. Anything else is absent unless the host adds it
/// explicitly, and credential-shaped names are refused even then.
fn minimal_environment() -> Vec<String> {
    [
        "PATH",
        "HOME",
        "LANG",
        "LC_ALL",
        "TMPDIR",
        "TEMP",
        "TMP",
        "SYSTEMROOT",
        "COMSPEC",
    ]
    .iter()
    .map(|name| (*name).to_string())
    .collect()
}

struct RunCommand {
    meta: ToolMeta,
}

impl RunCommand {
    fn new() -> Self {
        Self {
            meta: ToolMeta::new(
                "run_command",
                "Run a project program in the workspace (argv, no shell). The program must be approved. Returns exit status, captured output, digests, duration and any declared artefacts.",
                r#"{"type":"object","properties":{"argv":{"type":"array","items":{"type":"string"},"minItems":1,"maxItems":32},"cwd":{"type":"string","description":"directory inside the workspace; default the workspace root"},"timeout_ms":{"type":"integer","minimum":1,"maximum":300000},"output_limit":{"type":"integer","minimum":1,"maximum":1048576},"env":{"type":"object","description":"extra environment variables for this command"},"artifacts":{"type":"array","items":{"type":"string"},"maxItems":16,"description":"relative paths the command is expected to produce"}},"required":["argv"],"additionalProperties":false}"#,
                Risk::Execute,
                OUTPUT_LIMIT,
            ),
        }
    }
}

impl Tool for RunCommand {
    fn meta(&self) -> &ToolMeta {
        &self.meta
    }

    // One linear sequence of checks and one execution; splitting it would hide the order in
    // which the permissions are consulted.
    #[allow(clippy::too_many_lines)]
    fn run(&self, context: &mut ToolContext, arguments: &Json) -> ToolResult {
        let fields = crate::json::fields(arguments).expect("validated object");
        let argv = json::string_array(fields, "argv")?;
        if argv.is_empty() || argv.len() > MAX_ARGV {
            return Err(ToolFailure::new(
                ErrorClass::InvalidArguments,
                format!("argv must have 1 to {MAX_ARGV} items"),
            ));
        }
        let program = argv[0].clone();
        validate_program(&program).map_err(ToolFailure::from)?;
        for argument in &argv[1..] {
            if argument.is_empty() || argument.len() > 4096 || argument.contains('\0') {
                return Err(ToolFailure::new(
                    ErrorClass::InvalidArguments,
                    "an argument is empty, too long, or contains NUL",
                ));
            }
        }

        let scope = context.scope().ok_or_else(|| {
            ToolFailure::new(
                ErrorClass::Denied,
                "no execution scope has been approved in this session",
            )
        })?;
        if !scope.permits_exec(&program) {
            return Err(ToolFailure::new(
                ErrorClass::Denied,
                format!(
                    "`{program}` is not in the approved scope; ask for it to be approved before running it"
                ),
            ));
        }

        let working_directory = match json::optional_string(fields, "cwd")? {
            Some(cwd) => {
                let resolved = context.sandbox().resolve(cwd).map_err(|error| {
                    ToolFailure::new(ErrorClass::Denied, format!("{cwd}: {error}"))
                })?;
                if !resolved.is_dir() {
                    return Err(ToolFailure::new(
                        ErrorClass::NotFound,
                        format!("`{cwd}` is not a directory in the workspace"),
                    ));
                }
                resolved
            }
            None => context.sandbox().root().to_path_buf(),
        };

        let timeout_ms = json::optional_u64(fields, "timeout_ms")?.unwrap_or(DEFAULT_TIMEOUT_MS);
        let output_limit =
            json::optional_u64(fields, "output_limit")?.unwrap_or(DEFAULT_OUTPUT_LIMIT);
        let mut spec = ExecSpec::new(argv.clone())
            .with_timeout(timeout_ms.min(MAX_TIMEOUT_MS))
            .with_output_limit(output_limit.min(MAX_OUTPUT_LIMIT))
            .with_env_allowlist(minimal_environment());
        for (name, value) in extra_environment(fields)? {
            spec = spec.with_env(&name, &value);
        }

        let outcome = exec_in_with_cancel(&working_directory, &spec, Some(context.cancel_flag()));
        let mut data = Json::object(vec![
            ("argv", Json::Arr(argv.iter().map(Json::text).collect())),
            (
                "cwd",
                Json::text(working_directory.to_string_lossy().to_string()),
            ),
            ("duration_ms", Json::count(outcome.duration_ms)),
            ("stdout_sha256", Json::text(outcome.stdout.sha256.clone())),
            ("stderr_sha256", Json::text(outcome.stderr.sha256.clone())),
            ("stdout_bytes", Json::count(outcome.stdout.bytes)),
            ("stderr_bytes", Json::count(outcome.stderr.bytes)),
        ]);

        match outcome.status {
            ExecStatus::Completed => {
                let code = outcome.exit_code.unwrap_or(-1);
                data = merge(
                    data,
                    Json::object(vec![("exit_code", Json::count(code.max(0) as u64))]),
                );
                if code == 0 {
                    let mut text = String::new();
                    let _ = writeln!(
                        text,
                        "`{}` exited 0 in {} ms",
                        argv.join(" "),
                        outcome.duration_ms
                    );
                    if !outcome.stdout.text.is_empty() {
                        let _ = writeln!(text, "stdout:\n{}", outcome.stdout.text.trim_end());
                    }
                    if !outcome.stderr.text.is_empty() {
                        let _ = writeln!(text, "stderr:\n{}", outcome.stderr.text.trim_end());
                    }
                    let artifacts = declared_artifacts(context, fields)?;
                    if !artifacts.is_empty() {
                        let _ = writeln!(text, "artefacts:");
                        for artifact in &artifacts {
                            let _ = writeln!(text, "- {} {}", artifact.path, artifact.sha256);
                        }
                    }
                    data = merge(
                        data,
                        Json::object(vec![(
                            "artifacts",
                            Json::Arr(artifacts.iter().map(ArtifactRef::to_json).collect()),
                        )]),
                    );
                    return Ok(ToolOutput {
                        text,
                        data,
                        artifacts,
                    });
                }
                let excerpt = excerpt(&outcome.stderr.text, &outcome.stdout.text);
                Err(ToolFailure::new(
                    ErrorClass::Execution,
                    format!("`{}` exited {code}: {excerpt}", argv.join(" ")),
                )
                .with_data(data))
            }
            ExecStatus::LaunchFailed(reason) => Err(ToolFailure::new(
                ErrorClass::Execution,
                format!("`{}` could not be started: {reason}", argv.join(" ")),
            )
            .with_data(data)),
            ExecStatus::Timeout => Err(ToolFailure::new(
                ErrorClass::Timeout,
                format!(
                    "`{}` did not finish within {timeout_ms} ms and was killed",
                    argv.join(" ")
                ),
            )
            .with_data(data)),
            ExecStatus::Cancelled => Err(ToolFailure::new(
                ErrorClass::Cancelled,
                format!("`{}` was cancelled", argv.join(" ")),
            )
            .with_data(data)),
        }
    }
}

/// The extra variables a command asked for, checked before they are passed on.
fn extra_environment(fields: &json::Fields) -> Result<Vec<(String, String)>, ToolFailure> {
    let Some(value) = json::field(fields, "env") else {
        return Ok(Vec::new());
    };
    let entries = json::fields(value)
        .ok_or_else(|| ToolFailure::new(ErrorClass::InvalidArguments, "env must be an object"))?;
    if entries.len() > MAX_ENV {
        return Err(ToolFailure::new(
            ErrorClass::InvalidArguments,
            format!("env has more than {MAX_ENV} variables"),
        ));
    }
    let mut out = Vec::new();
    for (name, value) in entries {
        if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            || name.is_empty()
            || name.starts_with(|c: char| c.is_ascii_digit())
        {
            return Err(ToolFailure::new(
                ErrorClass::InvalidArguments,
                format!("`{name}` is not a usable environment variable name"),
            ));
        }
        if aporia_sandbox::is_sensitive_env(name) {
            return Err(ToolFailure::new(
                ErrorClass::Denied,
                format!("`{name}` is credential-shaped and is never passed to a program"),
            ));
        }
        let text = value.as_str().ok_or_else(|| {
            ToolFailure::new(
                ErrorClass::InvalidArguments,
                format!("the value of `{name}` must be a string"),
            )
        })?;
        if text.len() > 1024 || text.contains('\0') {
            return Err(ToolFailure::new(
                ErrorClass::InvalidArguments,
                format!("the value of `{name}` is too long or contains NUL"),
            ));
        }
        out.push((name.clone(), text.to_string()));
    }
    Ok(out)
}

/// The paths the command said it would produce. A declared path that does not exist is reported
/// as missing rather than quietly dropped.
fn declared_artifacts(
    context: &ToolContext,
    fields: &json::Fields,
) -> Result<Vec<ArtifactRef>, ToolFailure> {
    let Some(value) = json::field(fields, "artifacts") else {
        return Ok(Vec::new());
    };
    let paths = value.as_array().ok_or_else(|| {
        ToolFailure::new(ErrorClass::InvalidArguments, "artifacts must be an array")
    })?;
    if paths.len() > MAX_ARTIFACTS {
        return Err(ToolFailure::new(
            ErrorClass::InvalidArguments,
            format!("artifacts has more than {MAX_ARTIFACTS} paths"),
        ));
    }
    let mut out = Vec::new();
    for value in paths {
        let name = value.as_str().ok_or_else(|| {
            ToolFailure::new(
                ErrorClass::InvalidArguments,
                "each artefact path must be a string",
            )
        })?;
        let resolved = context
            .sandbox()
            .resolve(name)
            .map_err(|error| ToolFailure::new(ErrorClass::Denied, format!("{name}: {error}")))?;
        if !resolved.exists() {
            return Err(ToolFailure::new(
                ErrorClass::NotFound,
                format!("the declared artefact `{name}` was not produced"),
            ));
        }
        out.push(describe_artifact(&resolved, name));
    }
    Ok(out)
}

/// Identify a produced file or directory: a digest over the bytes, or over the sorted digests of
/// the files inside it.
fn describe_artifact(path: &Path, display: &str) -> ArtifactRef {
    let metadata = fs::metadata(path).ok();
    if metadata.as_ref().is_some_and(fs::Metadata::is_file) {
        let bytes = fs::read(path).unwrap_or_default();
        return ArtifactRef {
            kind: "file".to_string(),
            path: display.to_string(),
            sha256: sha256_hex(&bytes),
            bytes: bytes.len() as u64,
        };
    }
    let mut files: Vec<(String, String)> = Vec::new();
    let mut total = 0u64;
    let mut stack = vec![path.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let child = entry.path();
            let Ok(meta) = fs::symlink_metadata(&child) else {
                continue;
            };
            if meta.is_dir() {
                stack.push(child);
                continue;
            }
            if !meta.is_file() {
                continue;
            }
            let bytes = fs::read(&child).unwrap_or_default();
            total += bytes.len() as u64;
            let name = child.strip_prefix(path).map_or_else(
                |_| child.to_string_lossy().to_string(),
                |relative| relative.to_string_lossy().replace('\\', "/"),
            );
            files.push((name, sha256_hex(&bytes)));
        }
    }
    files.sort();
    let mut identity = String::new();
    for (name, digest) in &files {
        let _ = writeln!(identity, "{name} {digest}");
    }
    ArtifactRef {
        kind: "directory".to_string(),
        path: display.to_string(),
        sha256: sha256_hex(identity.as_bytes()),
        bytes: total,
    }
}

fn excerpt(stderr: &str, stdout: &str) -> String {
    let source = if stderr.trim().is_empty() {
        stdout
    } else {
        stderr
    };
    let trimmed = source.trim_end();
    if trimmed.len() <= EXCERPT {
        return trimmed.to_string();
    }
    let mut end = EXCERPT;
    while !trimmed.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n[excerpt truncated]", &trimmed[..end])
}

fn merge(base: Json, extra: Json) -> Json {
    match (base, extra) {
        (Json::Obj(mut fields), Json::Obj(others)) => {
            for (name, value) in others {
                fields.push((name, value));
            }
            Json::Obj(fields)
        }
        (base, _) => base,
    }
}
