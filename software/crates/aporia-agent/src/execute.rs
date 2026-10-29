//! Running the tasks a person approved, and recording what actually happened.
//!
//! The model cannot call these operations; it can only name them in a plan. By the time anything
//! runs, the arguments have been validated and approved, the working directory is the sandbox copy,
//! and the outcome is a captured observation -- including a nonzero exit, which is a result rather
//! than a host failure.

use crate::plan::{Operation, ValidatedTask};
use aporia_sandbox::{Captured, ExecSpec, ExecStatus, Sandbox};
use aporia_store::Json;
use aporia_store::digest::sha256_hex;
use std::path::Path;

pub const INSTRUMENT_TIMEOUT_MS: u64 = 300_000;
const MAX_ARTEFACT_FILES: u64 = 10_000;

/// A file the task produced that a reader can go back to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Artifact {
    pub kind: String,
    /// Relative to the investigation root, e.g. `workspace/archives/T1`.
    pub path: String,
    pub sha256: String,
    pub files: u64,
}

/// One executed (or skipped) task.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskResult {
    pub id: String,
    pub task_id: String,
    pub tool: String,
    pub status: String,
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    pub stdout: Captured,
    pub stderr: Captured,
    pub artifacts: Vec<Artifact>,
    pub note: String,
}

impl TaskResult {
    #[must_use]
    pub fn skipped(id: &str, task: &ValidatedTask, reason: &str) -> Self {
        Self {
            id: id.to_string(),
            task_id: task.task.id.clone(),
            tool: task.task.tool.as_str().to_string(),
            status: "skipped".to_string(),
            exit_code: None,
            duration_ms: 0,
            stdout: Captured::empty(),
            stderr: Captured::empty(),
            artifacts: Vec::new(),
            note: reason.to_string(),
        }
    }

    #[must_use]
    pub fn to_json(&self) -> Json {
        Json::object(vec![
            ("id", Json::text(&self.id)),
            ("task_id", Json::text(&self.task_id)),
            ("tool", Json::text(&self.tool)),
            ("status", Json::text(&self.status)),
            (
                "exit_code",
                self.exit_code.map_or(Json::Null, |code| {
                    Json::count(u64::try_from(code).unwrap_or(0))
                }),
            ),
            ("duration_ms", Json::count(self.duration_ms)),
            ("stdout", captured_json(&self.stdout)),
            ("stderr", captured_json(&self.stderr)),
            (
                "artifacts",
                Json::Arr(self.artifacts.iter().map(Artifact::to_json).collect()),
            ),
            ("note", Json::text(&self.note)),
        ])
    }
}

impl Artifact {
    #[must_use]
    pub fn to_json(&self) -> Json {
        Json::object(vec![
            ("kind", Json::text(&self.kind)),
            ("path", Json::text(&self.path)),
            ("sha256", Json::text(&self.sha256)),
            ("files", Json::count(self.files)),
        ])
    }
}

fn captured_json(captured: &Captured) -> Json {
    Json::object(vec![
        ("text", Json::text(&captured.text)),
        ("bytes", Json::count(captured.bytes)),
        ("sha256", Json::text(&captured.sha256)),
        ("truncated", Json::Bool(captured.truncated)),
    ])
}

/// Run one approved task.
pub fn execute(
    id: &str,
    task: &ValidatedTask,
    sandbox: &Sandbox,
    instrument: &Path,
    artifact_prefix: &str,
) -> TaskResult {
    let (spec, archive) = match &task.operation {
        Operation::Instrument {
            model,
            budget,
            archive,
        } => {
            let spec = ExecSpec::new(vec![
                instrument.to_string_lossy().to_string(),
                "run".to_string(),
                model.clone(),
                "--budget".to_string(),
                budget.to_string(),
                "--archive".to_string(),
                archive.clone(),
            ])
            .with_timeout(INSTRUMENT_TIMEOUT_MS);
            (spec, Some(archive.clone()))
        }
        Operation::Exec { argv, timeout_ms } => {
            (ExecSpec::new(argv.clone()).with_timeout(*timeout_ms), None)
        }
    };
    let outcome = sandbox.exec(&spec);
    let mut note = String::new();
    let mut artifacts = Vec::new();
    if let ExecStatus::LaunchFailed(reason) = &outcome.status {
        note.clone_from(reason);
    }
    if let Some(archive) = archive
        && matches!(outcome.status, ExecStatus::Completed)
    {
        match record_archive(sandbox, &archive, artifact_prefix) {
            Ok(Some(artifact)) => artifacts.push(artifact),
            Ok(None) => {
                note = "the instrument exited but wrote no archive manifest".to_string();
            }
            Err(reason) => note = reason,
        }
    }
    TaskResult {
        id: id.to_string(),
        task_id: task.task.id.clone(),
        tool: task.task.tool.as_str().to_string(),
        status: status_text(&outcome.status),
        exit_code: outcome.exit_code,
        duration_ms: outcome.duration_ms,
        stdout: outcome.stdout,
        stderr: outcome.stderr,
        artifacts,
        note,
    }
}

fn status_text(status: &ExecStatus) -> String {
    match status {
        ExecStatus::Completed => "completed",
        ExecStatus::LaunchFailed(_) => "launch_failed",
        ExecStatus::Timeout => "timeout",
    }
    .to_string()
}

fn record_archive(
    sandbox: &Sandbox,
    archive: &str,
    artifact_prefix: &str,
) -> Result<Option<Artifact>, String> {
    let root = sandbox
        .resolve(archive)
        .map_err(|error| format!("the archive path is not usable: {error}"))?;
    let manifest = root.join("manifest.json");
    if !manifest.is_file() {
        return Ok(None);
    }
    let bytes = std::fs::read(&manifest)
        .map_err(|error| format!("the archive manifest cannot be read: {error}"))?;
    let files = count_files(&root)?;
    Ok(Some(Artifact {
        kind: "instrument_archive".to_string(),
        path: format!("{artifact_prefix}/{}", archive.replace('\\', "/")),
        sha256: sha256_hex(&bytes),
        files,
    }))
}

fn count_files(root: &Path) -> Result<u64, String> {
    let mut count = 0u64;
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let entries = std::fs::read_dir(&directory)
            .map_err(|error| format!("cannot count the archive: {error}"))?;
        for entry in entries {
            let entry = entry.map_err(|error| format!("cannot count the archive: {error}"))?;
            let kind = entry
                .file_type()
                .map_err(|error| format!("cannot count the archive: {error}"))?;
            if kind.is_dir() {
                stack.push(entry.path());
                continue;
            }
            if kind.is_file() {
                count += 1;
                if count >= MAX_ARTEFACT_FILES {
                    return Ok(count);
                }
            }
        }
    }
    Ok(count)
}
