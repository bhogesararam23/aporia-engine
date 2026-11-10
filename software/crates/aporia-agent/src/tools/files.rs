//! Reading and searching the workspace: bounded, contained, and honest about truncation.
//!
//! These are the agent's eyes after the initial scan. Every path resolves through the sandbox's
//! containment check, every walk skips the same ignored directories the copy skipped, and every
//! result says when it stopped early. Search is literal substring matching, not regex: the model
//! gets a capability the host can bound and explain, and the schema says exactly that.

use super::result::{ErrorClass, ToolFailure, ToolOutput, ToolResult};
use super::{Tool, ToolContext, ToolMeta};
use crate::json;
use crate::scope::Risk;
use aporia_sandbox::{IGNORED_DIRS, PathError};
use aporia_store::Json;
use aporia_store::digest::sha256_hex;
use std::fmt::Write as _;
use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};

const OUTPUT_LIMIT: usize = 32 * 1024;
const READ_DEFAULT: u64 = 32 * 1024;
pub(super) const SCAN_CAP: usize = 20_000;
const SEARCH_FILE_MAX: u64 = 1024 * 1024;
const LINE_DISPLAY: usize = 512;

/// Register the file tools.
pub fn register(registry: &mut super::Registry) -> Result<(), String> {
    registry.register(Box::new(ListFiles::new()))?;
    registry.register(Box::new(SearchFiles::new()))?;
    registry.register(Box::new(SearchText::new()))?;
    registry.register(Box::new(ReadFile::new()))?;
    registry.register(Box::new(ReadFileRange::new()))?;
    Ok(())
}

struct ListFiles {
    meta: ToolMeta,
}

impl ListFiles {
    fn new() -> Self {
        Self {
            meta: ToolMeta::new(
                "list_files",
                "List files under a workspace directory, skipping ignored directories. Bounded; says when truncated.",
                r#"{"type":"object","properties":{"path":{"type":"string","description":"directory relative to the workspace root; default the root"},"max_entries":{"type":"integer","minimum":1,"maximum":2000}},"additionalProperties":false}"#,
                Risk::ReadOnly,
                OUTPUT_LIMIT,
            ),
        }
    }
}

impl Tool for ListFiles {
    fn meta(&self) -> &ToolMeta {
        &self.meta
    }

    fn run(&self, context: &mut ToolContext, arguments: &Json) -> ToolResult {
        let fields = crate::json::fields(arguments).expect("validated object");
        let start = json::optional_string(fields, "path")?.unwrap_or(".");
        let max = json::optional_u64(fields, "max_entries")?.unwrap_or(200) as usize;
        let directory = resolve_directory(context, start)?;
        let (files, truncated) = walk_files(context.sandbox().root(), &directory, max);
        let mut text = files.join("\n");
        if truncated {
            let _ = write!(text, "\n[listing truncated at {max} files]");
        }
        if files.is_empty() && !truncated {
            text = format!("no files under `{start}`");
        }
        Ok(ToolOutput {
            text,
            data: Json::object(vec![
                ("path", Json::text(start)),
                ("files", Json::Arr(files.iter().map(Json::text).collect())),
                ("truncated", Json::Bool(truncated)),
            ]),
            artifacts: Vec::new(),
        })
    }
}

struct SearchFiles {
    meta: ToolMeta,
}

impl SearchFiles {
    fn new() -> Self {
        Self {
            meta: ToolMeta::new(
                "search_files",
                "Find files whose relative path contains a literal substring (case-insensitive). Bounded.",
                r#"{"type":"object","properties":{"pattern":{"type":"string","minLength":1,"maxLength":256},"path":{"type":"string"},"max_matches":{"type":"integer","minimum":1,"maximum":500}},"required":["pattern"],"additionalProperties":false}"#,
                Risk::ReadOnly,
                OUTPUT_LIMIT,
            ),
        }
    }
}

impl Tool for SearchFiles {
    fn meta(&self) -> &ToolMeta {
        &self.meta
    }

    fn run(&self, context: &mut ToolContext, arguments: &Json) -> ToolResult {
        let fields = crate::json::fields(arguments).expect("validated object");
        let pattern = json::string(fields, "pattern")?.to_ascii_lowercase();
        let start = json::optional_string(fields, "path")?.unwrap_or(".");
        let max = json::optional_u64(fields, "max_matches")?.unwrap_or(100) as usize;
        let directory = resolve_directory(context, start)?;
        let (files, scan_truncated) = walk_files(context.sandbox().root(), &directory, SCAN_CAP);
        let matches: Vec<String> = files
            .into_iter()
            .filter(|path| path.to_ascii_lowercase().contains(&pattern))
            .take(max)
            .collect();
        let truncated = scan_truncated || matches.len() == max;
        let mut text = matches.join("\n");
        if truncated {
            text.push_str("\n[search stopped at a limit; more may exist]");
        }
        if matches.is_empty() && !truncated {
            text = format!("no file path contains `{pattern}`");
        }
        Ok(ToolOutput {
            text,
            data: Json::object(vec![
                (
                    "matches",
                    Json::Arr(matches.iter().map(Json::text).collect()),
                ),
                ("truncated", Json::Bool(truncated)),
            ]),
            artifacts: Vec::new(),
        })
    }
}

struct SearchText {
    meta: ToolMeta,
}

impl SearchText {
    fn new() -> Self {
        Self {
            meta: ToolMeta::new(
                "search_text",
                "Search file contents for a literal substring and return path:line: text. Skips files over 1 MiB. Bounded.",
                r#"{"type":"object","properties":{"pattern":{"type":"string","minLength":1,"maxLength":256},"path":{"type":"string"},"max_matches":{"type":"integer","minimum":1,"maximum":200},"case_sensitive":{"type":"boolean"}},"required":["pattern"],"additionalProperties":false}"#,
                Risk::ReadOnly,
                OUTPUT_LIMIT,
            ),
        }
    }
}

impl Tool for SearchText {
    fn meta(&self) -> &ToolMeta {
        &self.meta
    }

    fn run(&self, context: &mut ToolContext, arguments: &Json) -> ToolResult {
        let fields = crate::json::fields(arguments).expect("validated object");
        let raw_pattern = json::string(fields, "pattern")?;
        let case_sensitive = json::field(fields, "case_sensitive")
            .and_then(Json::as_bool)
            .unwrap_or(false);
        let pattern = if case_sensitive {
            raw_pattern.to_string()
        } else {
            raw_pattern.to_ascii_lowercase()
        };
        let start = json::optional_string(fields, "path")?.unwrap_or(".");
        let max = json::optional_u64(fields, "max_matches")?.unwrap_or(50) as usize;
        let directory = resolve_directory(context, start)?;
        let (files, scan_truncated) = walk_files(context.sandbox().root(), &directory, SCAN_CAP);

        let mut matches: Vec<Json> = Vec::new();
        let mut lines_out = String::new();
        let mut scanned = 0u64;
        let mut skipped_large = 0u64;
        let mut hit_limit = false;
        for relative in &files {
            if matches.len() >= max {
                hit_limit = true;
                break;
            }
            let Ok(path) = context.sandbox().resolve(relative) else {
                continue;
            };
            let Ok(metadata) = fs::metadata(&path) else {
                continue;
            };
            if metadata.len() > SEARCH_FILE_MAX {
                skipped_large += 1;
                continue;
            }
            let Ok(bytes) = fs::read(&path) else {
                continue;
            };
            scanned += 1;
            let text = String::from_utf8_lossy(&bytes);
            for (index, line) in text.lines().enumerate() {
                let haystack = if case_sensitive {
                    line.to_string()
                } else {
                    line.to_ascii_lowercase()
                };
                if haystack.contains(&pattern) {
                    let shown: String = line.chars().take(LINE_DISPLAY).collect();
                    let _ = writeln!(lines_out, "{relative}:{}: {shown}", index + 1);
                    matches.push(Json::object(vec![
                        ("path", Json::text(relative)),
                        ("line", Json::count((index + 1) as u64)),
                        ("text", Json::text(shown)),
                    ]));
                    if matches.len() >= max {
                        break;
                    }
                }
            }
        }
        let truncated = scan_truncated || hit_limit || matches.len() >= max;
        if truncated {
            lines_out.push_str("[search stopped at a limit; more may exist]\n");
        }
        if matches.is_empty() && !truncated {
            lines_out = format!("no match for `{raw_pattern}` in {scanned} scanned files\n");
        }
        let _ = writeln!(
            lines_out,
            "[scanned {scanned} files, skipped {skipped_large} over 1 MiB]"
        );
        Ok(ToolOutput {
            text: lines_out,
            data: Json::object(vec![
                ("matches", Json::Arr(matches)),
                ("files_scanned", Json::count(scanned)),
                ("files_skipped_large", Json::count(skipped_large)),
                ("truncated", Json::Bool(truncated)),
            ]),
            artifacts: Vec::new(),
        })
    }
}

struct ReadFile {
    meta: ToolMeta,
}

impl ReadFile {
    fn new() -> Self {
        Self {
            meta: ToolMeta::new(
                "read_file",
                "Read one workspace file, bounded (default 32 KiB, max 64 KiB). Reports bytes, digest and truncation.",
                r#"{"type":"object","properties":{"path":{"type":"string","minLength":1},"max_bytes":{"type":"integer","minimum":1,"maximum":65536}},"required":["path"],"additionalProperties":false}"#,
                Risk::ReadOnly,
                OUTPUT_LIMIT,
            ),
        }
    }
}

impl Tool for ReadFile {
    fn meta(&self) -> &ToolMeta {
        &self.meta
    }

    fn run(&self, context: &mut ToolContext, arguments: &Json) -> ToolResult {
        let fields = crate::json::fields(arguments).expect("validated object");
        let path = json::string(fields, "path")?;
        let limit = json::optional_u64(fields, "max_bytes")?.unwrap_or(READ_DEFAULT);
        let resolved = resolve_file(context, path)?;
        let file = fs::File::open(&resolved).map_err(|error| {
            ToolFailure::new(ErrorClass::Io, format!("cannot read `{path}`: {error}"))
        })?;
        let mut bytes = Vec::new();
        file.take(limit + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| {
                ToolFailure::new(ErrorClass::Io, format!("cannot read `{path}`: {error}"))
            })?;
        let truncated = bytes.len() as u64 > limit;
        if truncated {
            bytes.truncate(limit as usize);
        }
        let digest = sha256_hex(&bytes);
        let text = String::from_utf8_lossy(&bytes).to_string();
        let mut shown = format!("{path} ({} bytes, sha256 {digest}):\n{text}", bytes.len());
        if truncated {
            let _ = write!(shown, "\n[read stopped at {limit} bytes]");
        }
        Ok(ToolOutput {
            text: shown,
            data: Json::object(vec![
                ("path", Json::text(path)),
                ("bytes", Json::count(bytes.len() as u64)),
                ("sha256", Json::text(digest)),
                ("truncated", Json::Bool(truncated)),
            ]),
            artifacts: Vec::new(),
        })
    }
}

struct ReadFileRange {
    meta: ToolMeta,
}

impl ReadFileRange {
    fn new() -> Self {
        Self {
            meta: ToolMeta::new(
                "read_file_range",
                "Read an inclusive line range (at most 500 lines) with line numbers. Files over 1 MiB are refused.",
                r#"{"type":"object","properties":{"path":{"type":"string","minLength":1},"start_line":{"type":"integer","minimum":1},"end_line":{"type":"integer","minimum":1}},"required":["path","start_line","end_line"],"additionalProperties":false}"#,
                Risk::ReadOnly,
                OUTPUT_LIMIT,
            ),
        }
    }
}

impl Tool for ReadFileRange {
    fn meta(&self) -> &ToolMeta {
        &self.meta
    }

    fn run(&self, context: &mut ToolContext, arguments: &Json) -> ToolResult {
        let fields = crate::json::fields(arguments).expect("validated object");
        let path = json::string(fields, "path")?;
        let start = json::u64_value(fields, "start_line")?;
        let end = json::u64_value(fields, "end_line")?;
        if end < start || end - start + 1 > 500 {
            return Err(ToolFailure::new(
                ErrorClass::InvalidArguments,
                "end_line must be at least start_line and the range at most 500 lines",
            ));
        }
        let resolved = resolve_file(context, path)?;
        let metadata = fs::metadata(&resolved).map_err(|error| {
            ToolFailure::new(ErrorClass::Io, format!("cannot stat `{path}`: {error}"))
        })?;
        if metadata.len() > SEARCH_FILE_MAX {
            return Err(ToolFailure::new(
                ErrorClass::Unsupported,
                format!("`{path}` is over 1 MiB; range reading supports files up to 1 MiB"),
            ));
        }
        let bytes = fs::read(&resolved).map_err(|error| {
            ToolFailure::new(ErrorClass::Io, format!("cannot read `{path}`: {error}"))
        })?;
        let text = String::from_utf8_lossy(&bytes);
        let total = text.lines().count() as u64;
        if start > total.max(1) {
            return Err(ToolFailure::new(
                ErrorClass::InvalidArguments,
                format!("start_line {start} is past the end of `{path}` ({total} lines)"),
            ));
        }
        let mut shown = String::new();
        for (index, line) in text.lines().enumerate() {
            let number = index as u64 + 1;
            if number >= start && number <= end {
                let _ = writeln!(shown, "{number}: {line}");
            }
            if number > end {
                break;
            }
        }
        Ok(ToolOutput {
            text: shown,
            data: Json::object(vec![
                ("path", Json::text(path)),
                ("start_line", Json::count(start)),
                ("end_line", Json::count(end.min(total))),
                ("total_lines", Json::count(total)),
            ]),
            artifacts: Vec::new(),
        })
    }
}

fn map_path_error(error: &PathError, name: &str) -> ToolFailure {
    match error {
        PathError::Empty | PathError::Absolute => {
            ToolFailure::new(ErrorClass::InvalidArguments, format!("`{name}`: {error}"))
        }
        PathError::Escapes(_) => ToolFailure::new(
            ErrorClass::Denied,
            format!("`{name}` leaves the workspace; only contained paths are permitted"),
        ),
        PathError::Io(_) => ToolFailure::new(
            ErrorClass::NotFound,
            format!("`{name}` does not resolve to anything in the workspace"),
        ),
    }
}

fn resolve_directory(context: &ToolContext, name: &str) -> Result<PathBuf, ToolFailure> {
    let path = context
        .sandbox()
        .resolve(name)
        .map_err(|error| map_path_error(&error, name))?;
    if !path.is_dir() {
        return Err(ToolFailure::new(
            ErrorClass::NotFound,
            format!("`{name}` is not a directory in the workspace"),
        ));
    }
    Ok(path)
}

fn resolve_file(context: &ToolContext, name: &str) -> Result<PathBuf, ToolFailure> {
    let path = context
        .sandbox()
        .resolve(name)
        .map_err(|error| map_path_error(&error, name))?;
    if !path.is_file() {
        return Err(ToolFailure::new(
            ErrorClass::NotFound,
            format!("`{name}` is not a file in the workspace"),
        ));
    }
    Ok(path)
}

/// Walk regular files under `directory`, relative to `root`, in sorted order, skipping ignored
/// directories and any link (a program the agent ran could have made one). Bounded by `cap`.
pub(super) fn walk_files(root: &Path, directory: &Path, cap: usize) -> (Vec<String>, bool) {
    let mut files = Vec::new();
    let mut stack = vec![directory.to_path_buf()];
    let mut truncated = false;
    while let Some(current) = stack.pop() {
        let Ok(entries) = fs::read_dir(&current) else {
            continue;
        };
        let mut entries: Vec<fs::DirEntry> = entries.filter_map(Result::ok).collect();
        entries.sort_by_key(fs::DirEntry::file_name);
        // Reverse because the stack is LIFO; the result stays in sorted order.
        for entry in entries.into_iter().rev() {
            let Ok(metadata) = fs::symlink_metadata(entry.path()) else {
                continue;
            };
            if metadata.file_type().is_symlink() || is_reparse_point(&metadata) {
                continue;
            }
            if metadata.is_dir() {
                let name = entry.file_name();
                if IGNORED_DIRS.contains(&name.to_string_lossy().as_ref()) {
                    continue;
                }
                stack.push(entry.path());
                continue;
            }
            if !metadata.is_file() {
                continue;
            }
            if files.len() >= cap {
                truncated = true;
                break;
            }
            if let Ok(relative) = entry.path().strip_prefix(root) {
                files.push(relative.to_string_lossy().replace('\\', "/"));
            }
        }
        if truncated {
            break;
        }
    }
    files.sort();
    (files, truncated)
}

#[cfg(windows)]
fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_reparse_point(_metadata: &fs::Metadata) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::ToolCall;
    use crate::tools::{Registry, dispatch};
    use aporia_sandbox::Sandbox;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct Temp(PathBuf);

    impl Temp {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "aporia-files-{tag}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).expect("temp");
            Self(path)
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn fixture() -> (Temp, Temp, Sandbox, PathBuf) {
        let source = Temp::new("src");
        fs::create_dir_all(source.0.join("src")).expect("dir");
        fs::create_dir_all(source.0.join("node_modules")).expect("dir");
        fs::write(
            source.0.join("src/main.py"),
            b"def solve():\n    return 42\n",
        )
        .expect("file");
        fs::write(source.0.join("src/util.py"), b"ANSWER = 42\n").expect("file");
        fs::write(source.0.join("README.md"), b"# example\n").expect("file");
        fs::write(source.0.join("node_modules/junk.js"), b"ignored").expect("file");
        let workspace = Temp::new("ws");
        let (sandbox, _) = Sandbox::create(&source.0, &workspace.0.join("copy")).expect("sandbox");
        let instrument = std::env::current_exe().expect("exe");
        (source, workspace, sandbox, instrument)
    }

    fn run_tool(
        sandbox: &Sandbox,
        source: &Path,
        instrument: &Path,
        name: &str,
        arguments: &str,
    ) -> ToolResult {
        let mut registry = Registry::new();
        register(&mut registry).expect("register");
        let cancel = AtomicBool::new(false);
        let mut context = ToolContext::new(sandbox, source, instrument, None, &cancel);
        let call = ToolCall {
            id: "call-1".to_string(),
            name: name.to_string(),
            arguments: arguments.to_string(),
        };
        dispatch(&registry, &mut context, &call)
    }

    #[test]
    fn listing_skips_ignored_directories_and_reports_truncation() {
        let (source, _ws, sandbox, instrument) = fixture();
        let output = run_tool(&sandbox, &source.0, &instrument, "list_files", "{}").expect("ok");
        assert!(output.text.contains("src/main.py"));
        assert!(!output.text.contains("node_modules"));
        let limited = run_tool(
            &sandbox,
            &source.0,
            &instrument,
            "list_files",
            r#"{"max_entries":2}"#,
        )
        .expect("ok");
        assert!(limited.text.contains("truncated at 2"));
    }

    #[test]
    fn reading_returns_content_and_identity_and_refuses_escapes() {
        let (source, _ws, sandbox, instrument) = fixture();
        let output = run_tool(
            &sandbox,
            &source.0,
            &instrument,
            "read_file",
            r#"{"path":"src/util.py"}"#,
        )
        .expect("ok");
        assert!(output.text.contains("ANSWER = 42"));
        let fields = crate::json::fields(&output.data).expect("data");
        assert_eq!(crate::json::u64_value(fields, "bytes"), Ok(12));

        let missing = run_tool(
            &sandbox,
            &source.0,
            &instrument,
            "read_file",
            r#"{"path":"nope.py"}"#,
        )
        .expect_err("missing");
        assert_eq!(missing.class, ErrorClass::NotFound);

        let escape = run_tool(
            &sandbox,
            &source.0,
            &instrument,
            "read_file",
            r#"{"path":"../outside"}"#,
        )
        .expect_err("escape");
        assert_eq!(escape.class, ErrorClass::Denied);
    }

    #[test]
    fn a_range_is_numbered_and_a_bad_range_is_refused() {
        let (source, _ws, sandbox, instrument) = fixture();
        let output = run_tool(
            &sandbox,
            &source.0,
            &instrument,
            "read_file_range",
            r#"{"path":"src/main.py","start_line":2,"end_line":2}"#,
        )
        .expect("ok");
        assert_eq!(output.text.trim(), "2:     return 42");
        let bad = run_tool(
            &sandbox,
            &source.0,
            &instrument,
            "read_file_range",
            r#"{"path":"src/main.py","start_line":9,"end_line":10}"#,
        )
        .expect_err("past end");
        assert_eq!(bad.class, ErrorClass::InvalidArguments);
    }

    #[test]
    fn text_search_reports_positions_and_scanned_counts() {
        let (source, _ws, sandbox, instrument) = fixture();
        let output = run_tool(
            &sandbox,
            &source.0,
            &instrument,
            "search_text",
            r#"{"pattern":"answer"}"#,
        )
        .expect("ok");
        assert!(output.text.contains("src/util.py:1:"));
        assert!(output.text.contains("scanned 3 files"));
        let sensitive = run_tool(
            &sandbox,
            &source.0,
            &instrument,
            "search_text",
            r#"{"pattern":"answer","case_sensitive":true}"#,
        )
        .expect("ok");
        assert!(sensitive.text.contains("no match"));
    }

    #[test]
    fn file_search_matches_paths() {
        let (source, _ws, sandbox, instrument) = fixture();
        let output = run_tool(
            &sandbox,
            &source.0,
            &instrument,
            "search_files",
            r#"{"pattern":"MAIN"}"#,
        )
        .expect("ok");
        assert!(output.text.contains("src/main.py"));
        assert!(!output.text.contains("util"));
    }
}
