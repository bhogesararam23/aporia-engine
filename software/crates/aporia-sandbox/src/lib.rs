//! # aporia-sandbox
//!
//! The investigation workspace: one complete copy of a project, path containment for every tool
//! that touches it, and argv-only execution with time, output and environment limits.
//!
//! What this crate guarantees is narrow and testable: the source is never written, the copy is
//! complete for regular project content or the copy fails, a path a tool names resolves inside the
//! workspace, and a child process is given no shell, a fixed working directory, a bounded lifetime
//! and a bounded output. What it does not guarantee is written down in
//! `documentation/limitations.md`: this is copy isolation, not an operating-system jail.
//!
//! The digest comes from `aporia-store` rather than a second implementation here. A sandbox that
//! hashed output differently from the archive would produce a record that cannot be checked against
//! the artefact it describes.

pub mod copy;
pub mod exec;
pub mod path;

pub use copy::{CopyLimits, CopyReport, FileRecord, IGNORED_DIRS, Sandbox, SandboxError};
pub use exec::{Captured, ExecOutcome, ExecSpec, ExecStatus, is_sensitive_env};
pub use path::PathError;
