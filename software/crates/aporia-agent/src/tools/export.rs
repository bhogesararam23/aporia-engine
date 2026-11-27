//! Exporting the workspace changeset to a user-controlled location, only with authorisation.
//!
//! The tool prepares the patch inside the investigation record and asks; it never writes to the
//! destination itself. The engine performs the write only after a recorded `ChangesetExport`
//! approval, and the journal carries the destination and the patch's digest. A refusal leaves
//! the patch in the record and the investigation alive. The original project is never a valid
//! destination: export is how a person takes the change home, not how the agent writes the source.

use super::result::{ErrorClass, ToolFailure, ToolOutput, ToolResult};
use super::{Tool, ToolContext, ToolMeta};
use crate::diff::changeset;
use crate::json;
use crate::scope::{ApprovalKind, ApprovalRequest, Risk};
use aporia_store::Json;
use aporia_store::digest::sha256_hex;
use std::path::{Component, Path, PathBuf};

const OUTPUT_LIMIT: usize = 16 * 1024;

/// Register the export tool.
pub fn register(registry: &mut super::Registry) -> Result<(), String> {
    registry.register(Box::new(ExportChangeset::new()))
}

/// Where the prepared patch lives inside the record, before and after any decision.
pub const PATCH_ARTEFACT: &str = "changeset.patch";

/// The changeset a record's workspace holds against its source, read from the journal alone,
/// with the source path the record was opened against. The chain is verified first: an export
/// never stands on bytes that are not the ones written.
pub fn changeset_from_record(root: &Path) -> Result<(crate::diff::Changeset, PathBuf), String> {
    let journal_path = root.join("journal.jsonl");
    crate::journal::verify_journal(&journal_path).map_err(|error| error.to_string())?;
    let text = std::fs::read_to_string(&journal_path).map_err(|error| error.to_string())?;
    let mut source: Option<PathBuf> = None;
    let mut paths: Vec<String> = Vec::new();
    for line in text.lines() {
        if line.is_empty() {
            continue;
        }
        let value = Json::parse(line).map_err(|error| error.to_string())?;
        let fields = json::fields(&value).ok_or_else(|| "a journal line".to_string())?;
        let kind = json::string(fields, "kind")?;
        let payload = json::required(fields, "payload")?;
        match kind {
            "session.started" => {
                let pfields = json::fields(payload).ok_or_else(|| "session.started".to_string())?;
                source = Some(PathBuf::from(json::string(pfields, "source")?));
            }
            "change.applied" => {
                let change = crate::investigation::FileChange::from_json(payload)?;
                paths.push(change.path);
            }
            _ => {}
        }
    }
    let source = source.ok_or_else(|| "the record has no session.started event".to_string())?;
    let set =
        changeset(&source, &root.join("workspace"), &paths).map_err(|error| error.to_string())?;
    Ok((set, source))
}

struct ExportChangeset {
    meta: ToolMeta,
}

impl ExportChangeset {
    fn new() -> Self {
        Self {
            meta: ToolMeta::new(
                "export_changeset",
                "Prepare the workspace's net changes as a unified diff and ask permission to write it to a destination outside the sandbox. The write happens only after the person approves; the original project is never a valid destination.",
                r#"{"type":"object","properties":{"destination":{"type":"string","minLength":1,"description":"file path to write the patch to, outside the source project and the workspace"}},"required":["destination"],"additionalProperties":false}"#,
                Risk::WorkspaceWrite,
                OUTPUT_LIMIT,
            ),
        }
    }
}

impl Tool for ExportChangeset {
    fn meta(&self) -> &ToolMeta {
        &self.meta
    }

    fn run(&self, context: &mut ToolContext, arguments: &Json) -> ToolResult {
        let fields = crate::json::fields(arguments).expect("validated object");
        let destination = json::string(fields, "destination")?;
        let resolved = validate_destination(destination, context)?;
        let source = context.source().to_path_buf();
        let workspace = context.sandbox().root().to_path_buf();

        let session = context.session_mut().ok_or_else(|| {
            ToolFailure::new(
                ErrorClass::Internal,
                "export_changeset needs an investigation session",
            )
        })?;
        let paths: Vec<String> = session
            .investigation()
            .changes
            .iter()
            .map(|change| change.path.clone())
            .collect();
        if paths.is_empty() {
            return Err(ToolFailure::new(
                ErrorClass::NotFound,
                "no changes have been recorded in this workspace; there is nothing to export",
            ));
        }
        let set = changeset(&source, &workspace, &paths)
            .map_err(|error| ToolFailure::new(ErrorClass::Io, error.to_string()))?;
        if set.is_empty() {
            return Err(ToolFailure::new(
                ErrorClass::NotFound,
                "every recorded change was reverted; the workspace matches the original",
            ));
        }
        let digest = sha256_hex(set.patch.as_bytes());
        session
            .write_record_text(PATCH_ARTEFACT, &set.patch)
            .map_err(|reason| ToolFailure::new(ErrorClass::Internal, reason))?;
        session.request_approval(ApprovalRequest {
            kind: ApprovalKind::ChangesetExport,
            title: format!(
                "export {} changed file(s) to {}?",
                set.files.len(),
                resolved.display()
            ),
            detail: Json::object(vec![
                ("destination", Json::text(resolved.display().to_string())),
                ("files", Json::count(set.files.len() as u64)),
                ("sha256", Json::text(&digest)),
                (
                    "binary",
                    Json::Arr(set.binary.iter().map(Json::text).collect()),
                ),
            ]),
        });
        let mut text = format!(
            "changeset prepared: {} file(s), sha256 {digest}; it is in the record as {PATCH_ARTEFACT} and waits for approval to write to {}",
            set.files.len(),
            resolved.display()
        );
        if !set.binary.is_empty() {
            use std::fmt::Write as _;
            let _ = writeln!(
                text,
                "not representable as text and NOT in the patch: {}",
                set.binary.join(", ")
            );
        }
        Ok(ToolOutput::text(text))
    }
}

/// A destination must be a plain file path outside the source project and the workspace.
fn validate_destination(destination: &str, context: &ToolContext) -> Result<PathBuf, ToolFailure> {
    let path = Path::new(destination);
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(ToolFailure::new(
            ErrorClass::InvalidArguments,
            "the destination may not contain `..`",
        ));
    }
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| {
                ToolFailure::new(
                    ErrorClass::Io,
                    format!("cannot resolve the destination: {error}"),
                )
            })?
            .join(path)
    };
    for (name, root) in [
        ("the source project", context.source()),
        ("the workspace", context.sandbox().root()),
    ] {
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let near = canonical_parent(&absolute);
        if near.starts_with(&root) {
            return Err(ToolFailure::new(
                ErrorClass::Denied,
                format!(
                    "the destination is inside {name}; export writes outside it, never into it"
                ),
            ));
        }
    }
    Ok(absolute)
}

/// Canonicalise the deepest existing ancestor so a not-yet-created file still compares honestly.
fn canonical_parent(path: &Path) -> PathBuf {
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    let mut current = path.to_path_buf();
    loop {
        if let Ok(mut base) = current.canonicalize() {
            for part in tail.iter().rev() {
                base.push(part);
            }
            return base;
        }
        let Some(name) = current.file_name() else {
            return path.to_path_buf();
        };
        tail.push(name.to_os_string());
        let Some(parent) = current.parent() else {
            return path.to_path_buf();
        };
        current = parent.to_path_buf();
    }
}
