//! Deterministic reading of a project, before any model is asked anything.
//!
//! Every fact has an id, a kind, a project-relative path and a detail. The ids are what a plan or a
//! finding cites, so they are assigned in path order and never change for the same tree. The scan is
//! bounded: a directory deeper than four levels, more than five thousand entries, or a file larger
//! than the read limit produces a truncated summary rather than a claim of completeness.

use crate::json;
use aporia_sandbox::IGNORED_DIRS;
use aporia_store::Json;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};

pub const DEFAULT_MAX_ENTRIES: usize = 5_000;
pub const DEFAULT_MAX_DEPTH: usize = 4;
pub const DEFAULT_READ_LIMIT: u64 = 4 * 1024;

/// The kinds of fact the inspection can produce.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum FactKind {
    Tree,
    Extension,
    Git,
    Manifest,
    Readme,
    Test,
    Model,
    Unreadable,
}

impl FactKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tree => "tree",
            Self::Extension => "extension",
            Self::Git => "git",
            Self::Manifest => "manifest",
            Self::Readme => "readme",
            Self::Test => "test",
            Self::Model => "model",
            Self::Unreadable => "unreadable",
        }
    }
}

/// One inspected fact, identifiable so a plan can cite it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fact {
    pub id: String,
    pub kind: FactKind,
    pub path: String,
    pub detail: String,
}

/// What the scan covered, and what it did not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InspectionSummary {
    pub entries: usize,
    pub files: usize,
    pub bytes: u64,
    pub truncated: bool,
    /// Ignored directories, reparse points, and directories beyond the depth limit.
    pub skipped: Vec<String>,
    pub git_head: Option<String>,
}

/// The facts and their limits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Inspection {
    pub facts: Vec<Fact>,
    pub summary: InspectionSummary,
}

impl Inspection {
    #[must_use]
    pub fn to_json(&self) -> Json {
        let facts = self
            .facts
            .iter()
            .map(|fact| {
                Json::object(vec![
                    ("id", Json::text(&fact.id)),
                    ("kind", Json::text(fact.kind.as_str())),
                    ("path", Json::text(&fact.path)),
                    ("detail", Json::text(&fact.detail)),
                ])
            })
            .collect();
        let skipped = self.summary.skipped.iter().map(Json::text).collect();
        Json::object(vec![
            ("facts", Json::Arr(facts)),
            (
                "summary",
                Json::object(vec![
                    ("entries", Json::count(self.summary.entries as u64)),
                    ("files", Json::count(self.summary.files as u64)),
                    ("bytes", Json::count(self.summary.bytes)),
                    ("truncated", Json::Bool(self.summary.truncated)),
                    ("skipped", Json::Arr(skipped)),
                    (
                        "git_head",
                        self.summary
                            .git_head
                            .as_ref()
                            .map_or(Json::Null, Json::text),
                    ),
                ]),
            ),
        ])
    }

    /// A bounded rendering for a model prompt. The ids are the ones validation accepts.
    #[must_use]
    pub fn rendered(&self, max_bytes: usize) -> String {
        let mut out = String::new();
        let mut omitted = 0;
        for fact in &self.facts {
            let line = format!(
                "{} [{}] {}: {}\n",
                fact.id,
                fact.kind.as_str(),
                fact.path,
                fact.detail
            );
            if out.len() + line.len() > max_bytes {
                omitted += 1;
                continue;
            }
            out.push_str(&line);
        }
        if omitted > 0 {
            let _ = writeln!(out, "[{omitted} facts omitted by the prompt limit]");
        }
        out
    }
}

/// Why the project could not be read.
#[derive(Debug)]
pub enum InspectError {
    NotADirectory(PathBuf),
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl std::fmt::Display for InspectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotADirectory(path) => {
                write!(f, "{} is not a directory", path.display())
            }
            Self::Io { path, source } => write!(f, "cannot read {}: {source}", path.display()),
        }
    }
}

impl std::error::Error for InspectError {}

/// Limits, exposed so a test can exercise truncation without creating five thousand files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InspectLimits {
    pub max_entries: usize,
    pub max_depth: usize,
    pub read_limit: u64,
}

impl Default for InspectLimits {
    fn default() -> Self {
        Self {
            max_entries: DEFAULT_MAX_ENTRIES,
            max_depth: DEFAULT_MAX_DEPTH,
            read_limit: DEFAULT_READ_LIMIT,
        }
    }
}

/// Inspect a project directory.
pub fn inspect(root: &Path) -> Result<Inspection, InspectError> {
    inspect_with_limits(root, InspectLimits::default())
}

/// Inspect with explicit limits.
pub fn inspect_with_limits(root: &Path, limits: InspectLimits) -> Result<Inspection, InspectError> {
    if !root.is_dir() {
        return Err(InspectError::NotADirectory(root.to_path_buf()));
    }
    let git_head = read_git_head(root);
    let mut walker = Walker {
        limits,
        raw: Vec::new(),
        extensions: BTreeMap::new(),
        skipped: Vec::new(),
        entries: 0,
        files: 0,
        bytes: 0,
        truncated: false,
    };
    walker.walk(root, Path::new(""), 0)?;

    if !walker.extensions.is_empty() {
        walker.raw.push(RawFact {
            kind: FactKind::Extension,
            path: ".".to_string(),
            detail: extension_summary(&walker.extensions),
        });
    }
    walker.raw.push(RawFact {
        kind: FactKind::Tree,
        path: ".".to_string(),
        detail: format!(
            "{} entries, {} files, {} bytes{}, depth limit {}",
            walker.entries,
            walker.files,
            walker.bytes,
            if walker.truncated { ", truncated" } else { "" },
            limits.max_depth
        ),
    });
    if let Some(head) = &git_head {
        walker.raw.push(RawFact {
            kind: FactKind::Git,
            path: ".git/HEAD".to_string(),
            detail: head.clone(),
        });
    }

    walker
        .raw
        .sort_by(|left, right| left.path.cmp(&right.path).then(left.kind.cmp(&right.kind)));
    let facts = walker
        .raw
        .into_iter()
        .enumerate()
        .map(|(index, raw)| Fact {
            id: format!("F{}", index + 1),
            kind: raw.kind,
            path: raw.path,
            detail: raw.detail,
        })
        .collect();

    let mut skipped = walker.skipped;
    skipped.sort();
    skipped.dedup();
    Ok(Inspection {
        facts,
        summary: InspectionSummary {
            entries: walker.entries,
            files: walker.files,
            bytes: walker.bytes,
            truncated: walker.truncated,
            skipped,
            git_head,
        },
    })
}

struct RawFact {
    kind: FactKind,
    path: String,
    detail: String,
}

struct Walker {
    limits: InspectLimits,
    raw: Vec<RawFact>,
    extensions: BTreeMap<String, usize>,
    skipped: Vec<String>,
    entries: usize,
    files: usize,
    bytes: u64,
    truncated: bool,
}

impl Walker {
    fn walk(&mut self, root: &Path, relative: &Path, depth: usize) -> Result<(), InspectError> {
        let directory = root.join(relative);
        let entries = fs::read_dir(&directory).map_err(|source| InspectError::Io {
            path: directory.clone(),
            source,
        })?;
        let mut entries: Vec<fs::DirEntry> =
            entries
                .collect::<Result<Vec<_>, _>>()
                .map_err(|source| InspectError::Io {
                    path: directory.clone(),
                    source,
                })?;
        entries.sort_by_key(fs::DirEntry::file_name);

        for entry in entries {
            if self.entries >= self.limits.max_entries {
                self.truncated = true;
                break;
            }
            self.entries += 1;
            let name = entry.file_name();
            let child_relative = relative.join(&name);
            let child = entry.path();
            let display = display_path(&child_relative);
            let metadata = fs::symlink_metadata(&child).map_err(|source| InspectError::Io {
                path: child.clone(),
                source,
            })?;
            if metadata.file_type().is_symlink() || is_reparse_point(&metadata) {
                self.skipped.push(display);
                continue;
            }
            if metadata.is_dir() {
                let name_text: &str = &name.to_string_lossy();
                if IGNORED_DIRS.contains(&name_text) {
                    self.skipped.push(display);
                    continue;
                }
                if depth >= self.limits.max_depth {
                    self.skipped.push(display);
                    continue;
                }
                self.walk(root, &child_relative, depth + 1)?;
                continue;
            }
            if !metadata.is_file() {
                continue;
            }
            self.files += 1;
            self.bytes = self.bytes.saturating_add(metadata.len());
            if let Some(extension) = child.extension().and_then(|value| value.to_str()) {
                *self
                    .extensions
                    .entry(extension.to_ascii_lowercase())
                    .or_insert(0) += 1;
            }
            self.classify(&child, &child_relative, metadata.len());
        }
        Ok(())
    }

    fn classify(&mut self, path: &Path, relative: &Path, bytes: u64) {
        let display = display_path(relative);
        let name = path
            .file_name()
            .map_or_else(String::new, |value| value.to_string_lossy().to_string());
        let lower = name.to_ascii_lowercase();

        if path.extension().and_then(|value| value.to_str()) == Some("ap") {
            let detail = match read_prefix(path, self.limits.read_limit) {
                Ok(text) => {
                    let first = text.lines().find(|line| !line.trim().is_empty());
                    match first {
                        Some(line) => format!("{bytes} bytes; {}", line.trim()),
                        None => format!("{bytes} bytes; empty model file"),
                    }
                }
                Err(error) => {
                    self.unreadable(display, format!("{bytes} bytes; {error}"));
                    return;
                }
            };
            self.raw.push(RawFact {
                kind: FactKind::Model,
                path: display,
                detail,
            });
            return;
        }

        if is_manifest(&lower) {
            let detail = match read_prefix(path, self.limits.read_limit) {
                Ok(text) => {
                    let declared = manifest_name(&text);
                    match declared {
                        Some(declared) => format!("{}: {declared}", manifest_kind(&lower)),
                        None => manifest_kind(&lower).to_string(),
                    }
                }
                Err(error) => {
                    self.unreadable(display, error.to_string());
                    return;
                }
            };
            self.raw.push(RawFact {
                kind: FactKind::Manifest,
                path: display,
                detail,
            });
            return;
        }

        if lower.starts_with("readme.") || lower == "readme" {
            let detail = match read_prefix(path, self.limits.read_limit.min(2048)) {
                Ok(text) => text,
                Err(error) => {
                    self.unreadable(display, error.to_string());
                    return;
                }
            };
            self.raw.push(RawFact {
                kind: FactKind::Readme,
                path: display,
                detail,
            });
            return;
        }

        if looks_like_test(relative) {
            self.raw.push(RawFact {
                kind: FactKind::Test,
                path: display,
                detail: format!("{bytes} bytes"),
            });
        }
    }

    fn unreadable(&mut self, path: String, detail: String) {
        self.raw.push(RawFact {
            kind: FactKind::Unreadable,
            path,
            detail,
        });
    }
}

fn read_prefix(path: &Path, limit: u64) -> Result<String, std::io::Error> {
    let file = fs::File::open(path)?;
    let mut bytes = Vec::new();
    file.take(limit).read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).to_string())
}

fn manifest_kind(lower: &str) -> &'static str {
    match lower {
        "cargo.toml" => "Rust manifest",
        "pyproject.toml" | "setup.py" | "setup.cfg" | "requirements.txt" => "Python manifest",
        "package.json" => "Node manifest",
        "go.mod" => "Go manifest",
        "cmakelists.txt" => "CMake manifest",
        "makefile" => "Makefile",
        _ => "manifest",
    }
}

fn is_manifest(lower: &str) -> bool {
    manifest_kind(lower) != "manifest"
}

fn manifest_name(text: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("name") {
            let rest = rest.trim_start();
            if let Some(rest) = rest.strip_prefix('=') {
                let value = rest
                    .trim()
                    .trim_matches('"')
                    .trim_matches('\'')
                    .trim_end_matches(',');
                if !value.is_empty() {
                    return Some(value.to_string());
                }
            }
        }
        if line.starts_with("\"name\"")
            && let Some(value) = line.split(':').nth(1)
        {
            let value = value.trim().trim_matches('"').trim_end_matches(',');
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

fn looks_like_test(relative: &Path) -> bool {
    let path = relative
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase();
    path.starts_with("tests/")
        || path.contains("/tests/")
        || path.starts_with("test/")
        || path.contains("/test/")
        || path.rsplit('/').next().is_some_and(|name| {
            name.starts_with("test_") || name.ends_with("_test.py") || name.ends_with("_test.rs")
        })
}

fn extension_summary(extensions: &BTreeMap<String, usize>) -> String {
    let mut counts: Vec<(&String, &usize)> = extensions.iter().collect();
    counts.sort_by(|left, right| right.1.cmp(left.1).then(left.0.cmp(right.0)));
    counts
        .iter()
        .take(10)
        .map(|(extension, count)| format!(".{extension}: {count}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn read_git_head(root: &Path) -> Option<String> {
    let head_path = root.join(".git").join("HEAD");
    let text = fs::read_to_string(head_path).ok()?;
    let text = text.trim();
    if let Some(reference) = text.strip_prefix("ref: ") {
        let resolved = root.join(".git").join(reference.trim());
        let value = fs::read_to_string(resolved).ok()?;
        let value = value.trim();
        if value.len() == 40 && value.chars().all(|c| c.is_ascii_hexdigit()) {
            return Some(value.to_string());
        }
        return None;
    }
    if text.len() == 40 && text.chars().all(|c| c.is_ascii_hexdigit()) {
        return Some(text.to_string());
    }
    None
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
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

/// The facts as JSON for the record. Kept here so the writer and the reader cannot disagree.
pub fn inspection_from_json(value: &Json) -> Option<Vec<Fact>> {
    let fields = json::fields(value)?;
    let facts = json::field(fields, "facts")?.as_array()?;
    facts
        .iter()
        .map(|fact| {
            let fields = json::fields(fact)?;
            let kind = json::field(fields, "kind")?.as_str()?;
            let kind = match kind {
                "tree" => FactKind::Tree,
                "extension" => FactKind::Extension,
                "git" => FactKind::Git,
                "manifest" => FactKind::Manifest,
                "readme" => FactKind::Readme,
                "test" => FactKind::Test,
                "model" => FactKind::Model,
                "unreadable" => FactKind::Unreadable,
                _ => return None,
            };
            Some(Fact {
                id: json::field(fields, "id")?.as_str()?.to_string(),
                kind,
                path: json::field(fields, "path")?.as_str()?.to_string(),
                detail: json::field(fields, "detail")?.as_str()?.to_string(),
            })
        })
        .collect()
}
