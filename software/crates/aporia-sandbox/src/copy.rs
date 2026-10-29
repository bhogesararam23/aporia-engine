//! One complete copy of the project, or a named refusal.
//!
//! The copy is the isolation boundary this slice can actually guarantee: the source is read and
//! never written, and every regular file that is project content arrives with a digest. Caches,
//! version-control data and build output are omitted by name; anything else that cannot be copied
//! faithfully -- a link, a socket, a file past the limit -- stops the copy instead of quietly
//! producing a project that is not the one the user named.

use crate::path::{PathError, canonical_ish, resolve};
use aporia_store::digest::sha256_hex;
use std::fs;
use std::path::{Path, PathBuf};

/// Directory names that are never project content, at any depth.
///
/// The inspection stage uses the same list, so the facts a model sees describe the tree it can
/// actually read and run.
pub const IGNORED_DIRS: &[&str] = &[
    ".git",
    "target",
    "node_modules",
    ".venv",
    "venv",
    "__pycache__",
    ".mypy_cache",
    ".pytest_cache",
    ".ruff_cache",
    ".aporia",
    ".aporia-investigations",
    ".scratch",
    ".bootstrap",
];

/// How much may be copied. The defaults are the slice's limits, not a claim that larger projects
/// are unsupported; a refusal names the number so the user can decide what to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CopyLimits {
    pub max_files: usize,
    pub max_bytes: u64,
    pub max_file_bytes: u64,
}

impl Default for CopyLimits {
    fn default() -> Self {
        Self {
            max_files: 20_000,
            max_bytes: 512 * 1024 * 1024,
            max_file_bytes: 64 * 1024 * 1024,
        }
    }
}

/// One regular file in the copy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileRecord {
    /// Relative to the workspace, with `/` separators on every platform.
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
}

/// What the copy contains and what it deliberately did not copy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CopyReport {
    pub files: Vec<FileRecord>,
    pub ignored: Vec<String>,
    pub total_bytes: u64,
}

/// Why the source could not become a workspace.
#[derive(Debug)]
pub enum SandboxError {
    SourceMissing(PathBuf),
    NotADirectory(PathBuf),
    WorkspaceExists(PathBuf),
    Overlap {
        source: PathBuf,
        workspace: PathBuf,
    },
    TooManyFiles {
        found: usize,
        limit: usize,
    },
    TooManyBytes {
        found: u64,
        limit: u64,
    },
    FileTooLarge {
        path: String,
        bytes: u64,
        limit: u64,
    },
    /// A link or junction. Following one could copy or later run content outside the source.
    ReparsePoint(String),
    UnsupportedFileType(String),
    Io {
        path: PathBuf,
        action: &'static str,
        source: std::io::Error,
    },
}

impl std::fmt::Display for SandboxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SourceMissing(path) => write!(f, "the source {} does not exist", path.display()),
            Self::NotADirectory(path) => {
                write!(f, "the source {} is not a directory", path.display())
            }
            Self::WorkspaceExists(path) => write!(
                f,
                "the workspace {} already exists; an investigation never overwrites one",
                path.display()
            ),
            Self::Overlap { source, workspace } => write!(
                f,
                "the workspace {} and the source {} overlap",
                workspace.display(),
                source.display()
            ),
            Self::TooManyFiles { found, limit } => write!(
                f,
                "the source has at least {found} files; the copy limit is {limit}"
            ),
            Self::TooManyBytes { found, limit } => write!(
                f,
                "the source needs at least {found} bytes; the copy limit is {limit}"
            ),
            Self::FileTooLarge { path, bytes, limit } => write!(
                f,
                "{path} is {bytes} bytes; one copied file may be at most {limit}"
            ),
            Self::ReparsePoint(path) => write!(
                f,
                "{path} is a link or junction; the copy refuses to follow content out of the source"
            ),
            Self::UnsupportedFileType(path) => {
                write!(f, "{path} is not a regular file or a directory")
            }
            Self::Io {
                path,
                action,
                source,
            } => write!(f, "cannot {action} {}: {source}", path.display()),
        }
    }
}

impl std::error::Error for SandboxError {}

/// A workspace that tools may read and write, and nothing outside it.
#[derive(Clone, Debug)]
pub struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    /// Copy `source` into a new `workspace`. The source is only read.
    pub fn create(source: &Path, workspace: &Path) -> Result<(Self, CopyReport), SandboxError> {
        Self::create_with_limits(source, workspace, CopyLimits::default())
    }

    /// The same copy with explicit limits, for tests and for a caller with a different budget.
    pub fn create_with_limits(
        source: &Path,
        workspace: &Path,
        limits: CopyLimits,
    ) -> Result<(Self, CopyReport), SandboxError> {
        let source_canonical = source.canonicalize().map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                SandboxError::SourceMissing(source.to_path_buf())
            } else {
                SandboxError::Io {
                    path: source.to_path_buf(),
                    action: "resolve the source",
                    source: error,
                }
            }
        })?;
        if !source_canonical.is_dir() {
            return Err(SandboxError::NotADirectory(source.to_path_buf()));
        }

        let workspace_near = canonical_ish(workspace).map_err(|error| SandboxError::Io {
            path: workspace.to_path_buf(),
            action: "resolve the workspace",
            source: std::io::Error::other(error.to_string()),
        })?;
        // A workspace inside the source is usually a recursive copy waiting to happen. The one
        // exception is a workspace under an ignored directory (the default `.aporia/investigations`):
        // the copy skips that directory by name, so the workspace cannot contain itself.
        let inside_ignored = workspace_near
            .strip_prefix(&source_canonical)
            .ok()
            .and_then(|relative| relative.components().next())
            .is_some_and(|component| match component {
                std::path::Component::Normal(name) => {
                    IGNORED_DIRS.contains(&name.to_string_lossy().as_ref())
                }
                _ => false,
            });
        if source_canonical.starts_with(&workspace_near)
            || (workspace_near.starts_with(&source_canonical) && !inside_ignored)
        {
            return Err(SandboxError::Overlap {
                source: source_canonical,
                workspace: workspace_near,
            });
        }
        if workspace.exists() {
            return Err(SandboxError::WorkspaceExists(workspace.to_path_buf()));
        }

        fs::create_dir_all(workspace).map_err(|error| SandboxError::Io {
            path: workspace.to_path_buf(),
            action: "create the workspace",
            source: error,
        })?;
        let canonical = workspace.canonicalize().map_err(|error| SandboxError::Io {
            path: workspace.to_path_buf(),
            action: "resolve the created workspace",
            source: error,
        })?;

        let mut copier = Copier {
            limits,
            report: CopyReport {
                files: Vec::new(),
                ignored: Vec::new(),
                total_bytes: 0,
            },
        };
        if let Err(error) = copier.copy_dir(&source_canonical, &canonical, Path::new("")) {
            // A failed copy must not leave a directory that looks like a workspace.
            let _ = fs::remove_dir_all(&canonical);
            return Err(error);
        }
        Ok((Self { root: canonical }, copier.report))
    }

    /// The canonical workspace root. Commands run here; tools resolve here.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Resolve a name inside the workspace, refusing anything that leaves it.
    pub fn resolve(&self, relative: &str) -> Result<PathBuf, PathError> {
        resolve(&self.root, relative)
    }
}

struct Copier {
    limits: CopyLimits,
    report: CopyReport,
}

impl Copier {
    fn copy_dir(
        &mut self,
        source: &Path,
        destination: &Path,
        relative: &Path,
    ) -> Result<(), SandboxError> {
        let entries = fs::read_dir(source).map_err(|error| SandboxError::Io {
            path: source.to_path_buf(),
            action: "read a directory",
            source: error,
        })?;
        let mut entries: Vec<fs::DirEntry> =
            entries
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| SandboxError::Io {
                    path: source.to_path_buf(),
                    action: "read a directory entry",
                    source: error,
                })?;
        entries.sort_by_key(fs::DirEntry::file_name);

        for entry in entries {
            let name = entry.file_name();
            let child_relative = relative.join(&name);
            let child_source = entry.path();
            let child_destination = destination.join(&name);
            let display = display_path(&child_relative);
            let metadata =
                fs::symlink_metadata(&child_source).map_err(|error| SandboxError::Io {
                    path: child_source.clone(),
                    action: "read metadata",
                    source: error,
                })?;
            if is_reparse_point(&metadata) {
                return Err(SandboxError::ReparsePoint(display));
            }
            let file_type = metadata.file_type();
            if file_type.is_symlink() {
                return Err(SandboxError::ReparsePoint(display));
            }
            if file_type.is_dir() {
                let name_text: &str = &name.to_string_lossy();
                if IGNORED_DIRS.contains(&name_text) {
                    self.report.ignored.push(display);
                    continue;
                }
                fs::create_dir_all(&child_destination).map_err(|error| SandboxError::Io {
                    path: child_destination.clone(),
                    action: "create a directory",
                    source: error,
                })?;
                self.copy_dir(&child_source, &child_destination, &child_relative)?;
                continue;
            }
            if !file_type.is_file() {
                return Err(SandboxError::UnsupportedFileType(display));
            }

            self.copy_file(&child_source, &child_destination, display, &metadata)?;
        }
        Ok(())
    }

    fn copy_file(
        &mut self,
        source: &Path,
        destination: &Path,
        display: String,
        metadata: &fs::Metadata,
    ) -> Result<(), SandboxError> {
        let bytes = metadata.len();
        if bytes > self.limits.max_file_bytes {
            return Err(SandboxError::FileTooLarge {
                path: display,
                bytes,
                limit: self.limits.max_file_bytes,
            });
        }
        let found = self.report.files.len() + 1;
        if found > self.limits.max_files {
            return Err(SandboxError::TooManyFiles {
                found,
                limit: self.limits.max_files,
            });
        }
        let total = self.report.total_bytes.saturating_add(bytes);
        if total > self.limits.max_bytes {
            return Err(SandboxError::TooManyBytes {
                found: total,
                limit: self.limits.max_bytes,
            });
        }

        let contents = fs::read(source).map_err(|error| SandboxError::Io {
            path: source.to_path_buf(),
            action: "read a file",
            source: error,
        })?;
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|error| SandboxError::Io {
                path: parent.to_path_buf(),
                action: "create a parent directory",
                source: error,
            })?;
        }
        fs::write(destination, &contents).map_err(|error| SandboxError::Io {
            path: destination.to_path_buf(),
            action: "write a file",
            source: error,
        })?;
        fs::set_permissions(destination, metadata.permissions()).map_err(|error| {
            SandboxError::Io {
                path: destination.to_path_buf(),
                action: "copy file permissions",
                source: error,
            }
        })?;
        self.report.total_bytes += contents.len() as u64;
        self.report.files.push(FileRecord {
            path: display,
            bytes: contents.len() as u64,
            sha256: sha256_hex(&contents),
        });
        Ok(())
    }
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
