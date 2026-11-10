//! Changing the workspace: create, modify, delete and put back.
//!
//! A change is a first-class record: the path, what it did, and the digests before and after.
//! The original project is never written -- the workspace is a copy, and putting a file back means
//! restoring the copy of the source's own bytes. Every write is atomic (a temporary file and a
//! rename), so an interrupted change leaves either the old file or the new one, never half of
//! each. Writing needs approved write permission; the risk gate refuses the call before the
//! path is even resolved.

use super::result::{ErrorClass, ToolFailure, ToolOutput, ToolResult};
use super::{Tool, ToolContext, ToolMeta};
use crate::investigation::ChangeKind;
use crate::json;
use crate::scope::Risk;
use aporia_store::Json;
use aporia_store::digest::sha256_hex;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

const OUTPUT_LIMIT: usize = 16 * 1024;
const MAX_CONTENT_BYTES: usize = 1024 * 1024;

/// Register the editing tools.
pub fn register(registry: &mut super::Registry) -> Result<(), String> {
    registry.register(Box::new(WriteFile::new()))?;
    registry.register(Box::new(DeleteFile::new()))?;
    registry.register(Box::new(RevertFile::new()))?;
    Ok(())
}

fn digest_of(path: &Path) -> Option<String> {
    fs::read(path).ok().map(|bytes| sha256_hex(&bytes))
}

/// Write the bytes to the workspace atomically: a temporary file beside the target, synced, then
/// renamed over it.
fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let temporary = path.with_extension("aporia-tmp");
    fs::write(&temporary, bytes).map_err(|error| error.to_string())?;
    if let Ok(file) = fs::File::open(&temporary) {
        let _ = file.sync_all();
    }
    fs::rename(&temporary, path).map_err(|error| {
        let _ = fs::remove_file(&temporary);
        error.to_string()
    })
}

struct WriteFile {
    meta: ToolMeta,
}

impl WriteFile {
    fn new() -> Self {
        Self {
            meta: ToolMeta::new(
                "write_file",
                "Create or replace a file in the workspace copy. The original project is never written. The change is recorded with the digests before and after.",
                r#"{"type":"object","properties":{"path":{"type":"string","minLength":1},"content":{"type":"string"},"append":{"type":"boolean"}},"required":["path","content"],"additionalProperties":false}"#,
                Risk::WorkspaceWrite,
                OUTPUT_LIMIT,
            ),
        }
    }
}

impl Tool for WriteFile {
    fn meta(&self) -> &ToolMeta {
        &self.meta
    }

    fn run(&self, context: &mut ToolContext, arguments: &Json) -> ToolResult {
        let fields = crate::json::fields(arguments).expect("validated object");
        let path = json::string(fields, "path")?;
        let content = json::string(fields, "content")?;
        let append = json::field(fields, "append")
            .and_then(Json::as_bool)
            .unwrap_or(false);
        if content.len() > MAX_CONTENT_BYTES {
            return Err(ToolFailure::new(
                ErrorClass::InvalidArguments,
                format!("the content is over {MAX_CONTENT_BYTES} bytes"),
            ));
        }
        let resolved = context
            .sandbox()
            .resolve(path)
            .map_err(|error| ToolFailure::new(ErrorClass::Denied, format!("{path}: {error}")))?;
        if resolved.is_dir() {
            return Err(ToolFailure::new(
                ErrorClass::InvalidArguments,
                format!("`{path}` is a directory"),
            ));
        }
        let before = digest_of(&resolved);
        let mut bytes = if append {
            fs::read(&resolved).unwrap_or_default()
        } else {
            Vec::new()
        };
        bytes.extend_from_slice(content.as_bytes());
        if let Some(parent) = resolved.parent() {
            let _ = fs::create_dir_all(parent);
        }
        write_atomically(&resolved, &bytes)
            .map_err(|error| ToolFailure::new(ErrorClass::Io, format!("{path}: {error}")))?;
        let after = digest_of(&resolved).unwrap_or_default();

        let change = crate::investigation::FileChange {
            path: path.to_string(),
            kind: if before.is_none() {
                ChangeKind::Created
            } else {
                ChangeKind::Modified
            },
            before,
            after: Some(after.clone()),
        };
        record_change(context, &change)?;
        let mut text = format!("wrote {path} ({} bytes, sha256 {after})", bytes.len());
        let _ = write!(text, "\n{}", describe_change(context, path));
        Ok(ToolOutput::text(text))
    }
}

struct DeleteFile {
    meta: ToolMeta,
}

impl DeleteFile {
    fn new() -> Self {
        Self {
            meta: ToolMeta::new(
                "delete_file",
                "Delete a file from the workspace copy. The original project is never written. The change is recorded.",
                r#"{"type":"object","properties":{"path":{"type":"string","minLength":1}},"required":["path"],"additionalProperties":false}"#,
                Risk::WorkspaceWrite,
                OUTPUT_LIMIT,
            ),
        }
    }
}

impl Tool for DeleteFile {
    fn meta(&self) -> &ToolMeta {
        &self.meta
    }

    fn run(&self, context: &mut ToolContext, arguments: &Json) -> ToolResult {
        let fields = crate::json::fields(arguments).expect("validated object");
        let path = json::string(fields, "path")?;
        let resolved = context
            .sandbox()
            .resolve(path)
            .map_err(|error| ToolFailure::new(ErrorClass::Denied, format!("{path}: {error}")))?;
        if !resolved.is_file() {
            return Err(ToolFailure::new(
                ErrorClass::NotFound,
                format!("`{path}` is not a file in the workspace"),
            ));
        }
        let before = digest_of(&resolved);
        fs::remove_file(&resolved).map_err(|error| {
            ToolFailure::new(ErrorClass::Io, format!("cannot delete `{path}`: {error}"))
        })?;
        let change = crate::investigation::FileChange {
            path: path.to_string(),
            kind: ChangeKind::Deleted,
            before,
            after: None,
        };
        record_change(context, &change)?;
        Ok(ToolOutput::text(format!("deleted {path}")))
    }
}

struct RevertFile {
    meta: ToolMeta,
}

impl RevertFile {
    fn new() -> Self {
        Self {
            meta: ToolMeta::new(
                "revert_file",
                "Put a workspace file back to the original project's bytes, or remove it if the original had no such file.",
                r#"{"type":"object","properties":{"path":{"type":"string","minLength":1}},"required":["path"],"additionalProperties":false}"#,
                Risk::WorkspaceWrite,
                OUTPUT_LIMIT,
            ),
        }
    }
}

impl Tool for RevertFile {
    fn meta(&self) -> &ToolMeta {
        &self.meta
    }

    fn run(&self, context: &mut ToolContext, arguments: &Json) -> ToolResult {
        let fields = crate::json::fields(arguments).expect("validated object");
        let path = json::string(fields, "path")?;
        let resolved = context
            .sandbox()
            .resolve(path)
            .map_err(|error| ToolFailure::new(ErrorClass::Denied, format!("{path}: {error}")))?;
        // The original bytes live in the source project, which the copy was made from and which
        // the agent never writes.
        let original = context.source().join(path);
        let change = if original.is_file() {
            let bytes = fs::read(&original).map_err(|error| {
                ToolFailure::new(ErrorClass::Io, format!("cannot read the original: {error}"))
            })?;
            write_atomically(&resolved, &bytes)
                .map_err(|error| ToolFailure::new(ErrorClass::Io, format!("{path}: {error}")))?;
            crate::investigation::FileChange {
                path: path.to_string(),
                kind: ChangeKind::Modified,
                before: digest_of(&resolved),
                after: Some(sha256_hex(&bytes)),
            }
        } else {
            let _ = fs::remove_file(&resolved);
            crate::investigation::FileChange {
                path: path.to_string(),
                kind: ChangeKind::Deleted,
                before: digest_of(&resolved),
                after: None,
            }
        };
        record_change(context, &change)?;
        Ok(ToolOutput::text(format!("reverted {path}")))
    }
}

/// Record the change in the investigation, when the tool has a session.
fn record_change(
    context: &mut ToolContext,
    change: &crate::investigation::FileChange,
) -> Result<(), ToolFailure> {
    let Some(session) = context.session_mut() else {
        return Ok(());
    };
    let investigation = session.investigation();
    investigation.changes.push(change.clone());
    session
        .journal("change.applied", change.to_json())
        .map_err(|reason| ToolFailure::new(ErrorClass::Internal, reason))
}

/// A short account of what changed in a file, for the model to read.
fn describe_change(context: &ToolContext, path: &str) -> String {
    let original = context.source().join(path);
    let Ok(current) = context.sandbox().resolve(path) else {
        return "the file is gone".to_string();
    };
    let before = fs::read_to_string(&original).unwrap_or_default();
    let after = fs::read_to_string(&current).unwrap_or_default();
    if before == after {
        return "the content is the same as the original".to_string();
    }
    let before_lines: Vec<&str> = before.lines().collect();
    let after_lines: Vec<&str> = after.lines().collect();
    let mut out = String::new();
    let _ = writeln!(
        out,
        "the original had {} lines, the workspace has {}",
        before_lines.len(),
        after_lines.len()
    );
    for (index, line) in after_lines.iter().enumerate() {
        match before_lines.get(index) {
            Some(original_line) if original_line == line => {}
            Some(original_line) => {
                let _ = writeln!(out, "line {}: - {original_line}", index + 1);
                let _ = writeln!(out, "line {}: + {line}", index + 1);
            }
            None => {
                let _ = writeln!(out, "line {}: + {line}", index + 1);
            }
        }
    }
    for (index, line) in before_lines.iter().enumerate().skip(after_lines.len()) {
        let _ = writeln!(out, "line {}: - {line}", index + 1);
    }
    out
}
