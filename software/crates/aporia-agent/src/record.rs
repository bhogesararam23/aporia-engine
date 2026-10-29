//! The investigation directory: a journal, the artefact files, and a manifest written last.
//!
//! The layout is the contract a reader can rely on: if `manifest.json` is present, the journal is
//! expected to be complete and the manifest's head and sandbox digest are expected to match the
//! bytes on disk. `verify_record` checks exactly that, and `aporia investigate report` refuses a
//! record that fails.

use crate::journal::{Journal, RecordError, now_seconds, verify_journal};
use crate::json;
use aporia_store::Json;
use aporia_store::digest::sha256_hex;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

/// A new investigation directory on disk.
#[derive(Debug)]
pub struct RecordDir {
    root: PathBuf,
    workspace: PathBuf,
    journal: Journal,
}

impl RecordDir {
    /// Create a new directory under `workdir`. The name carries the Unix second, with a numeric
    /// suffix if two investigations start in the same second; an existing directory is never reused.
    pub fn create(workdir: &Path) -> Result<Self, RecordError> {
        fs::create_dir_all(workdir).map_err(|source| RecordError::Io {
            path: workdir.to_path_buf(),
            action: "create the work directory",
            source,
        })?;
        let base = format!("inv-{}", now_seconds());
        let mut candidate = workdir.join(&base);
        let mut suffix = 1u32;
        loop {
            match fs::create_dir(&candidate) {
                Ok(()) => break,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    candidate = workdir.join(format!("{base}-{suffix}"));
                    suffix += 1;
                    if suffix > 10_000 {
                        return Err(RecordError::Io {
                            path: candidate,
                            action: "find a free investigation name",
                            source: error,
                        });
                    }
                }
                Err(source) => {
                    return Err(RecordError::Io {
                        path: candidate,
                        action: "create the investigation directory",
                        source,
                    });
                }
            }
        }
        let root = candidate.canonicalize().map_err(|source| RecordError::Io {
            path: candidate.clone(),
            action: "resolve the investigation directory",
            source,
        })?;
        let journal = Journal::create(&root.join("journal.jsonl"))?;
        let workspace = root.join("workspace");
        Ok(Self {
            root,
            workspace,
            journal,
        })
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    pub fn journal_mut(&mut self) -> &mut Journal {
        &mut self.journal
    }

    /// Write a JSON artefact. Names are simple: no separators, no traversal.
    pub fn write_json(&self, name: &str, value: &Json) -> Result<(), RecordError> {
        self.write_text(name, &value.to_pretty())
    }

    /// Write a text artefact.
    pub fn write_text(&self, name: &str, text: &str) -> Result<(), RecordError> {
        let path = self.path(name)?;
        let mut file = fs::File::create(&path).map_err(|source| RecordError::Io {
            path: path.clone(),
            action: "create an artefact",
            source,
        })?;
        file.write_all(text.as_bytes())
            .and_then(|()| file.sync_all())
            .map_err(|source| RecordError::Io {
                path,
                action: "write an artefact",
                source,
            })
    }

    /// Write `manifest.json` last. Its presence is what makes a record complete.
    pub fn finish(&self, manifest: &Json) -> Result<(), RecordError> {
        self.write_json("manifest.json", manifest)
    }

    fn path(&self, name: &str) -> Result<PathBuf, RecordError> {
        if name.is_empty()
            || name.contains('/')
            || name.contains('\\')
            || name.contains("..")
            || !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        {
            return Err(RecordError::Io {
                path: self.root.join(name),
                action: "use an artefact name",
                source: std::io::Error::other("the name must be one simple file name"),
            });
        }
        Ok(self.root.join(name))
    }
}

/// What `verify_record` learned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordSummary {
    pub outcome: String,
    pub provider: String,
    pub events: u64,
    pub head: String,
}

/// Verify a finished investigation: the journal chain, the manifest's head and event count, and
/// (when recorded) the digest of `sandbox.json`.
pub fn verify_record(root: &Path) -> Result<RecordSummary, RecordError> {
    let manifest_path = root.join("manifest.json");
    if !manifest_path.is_file() {
        return Err(RecordError::NotFinalised(root.to_path_buf()));
    }
    let text = fs::read_to_string(&manifest_path).map_err(|source| RecordError::Io {
        path: manifest_path.clone(),
        action: "read the manifest",
        source,
    })?;
    let value = Json::parse(&text).map_err(|error| RecordError::Manifest {
        path: manifest_path.clone(),
        reason: error.to_string(),
    })?;
    let fields = json::fields(&value).ok_or_else(|| RecordError::Manifest {
        path: manifest_path.clone(),
        reason: "the manifest must be an object".to_string(),
    })?;
    let journal_object = json::field(fields, "journal")
        .and_then(json::fields)
        .ok_or_else(|| RecordError::Manifest {
            path: manifest_path.clone(),
            reason: "missing journal object".to_string(),
        })?;
    let expected_head = json::string(journal_object, "head")
        .map_err(|reason| RecordError::Manifest {
            path: manifest_path.clone(),
            reason,
        })?
        .to_string();
    let expected_events =
        json::u64_value(journal_object, "events").map_err(|reason| RecordError::Manifest {
            path: manifest_path.clone(),
            reason,
        })?;

    let verified = verify_journal(&root.join("journal.jsonl"))?;
    if verified.head != expected_head {
        return Err(RecordError::Head {
            expected: expected_head,
            found: verified.head,
        });
    }
    if verified.events != expected_events {
        return Err(RecordError::Events {
            expected: expected_events,
            found: verified.events,
        });
    }

    if let Some(expected) = json::field(fields, "sandbox_sha256").and_then(Json::as_str) {
        let sandbox_path = root.join("sandbox.json");
        let bytes = fs::read(&sandbox_path).map_err(|source| RecordError::Io {
            path: sandbox_path,
            action: "read the sandbox record",
            source,
        })?;
        let found = sha256_hex(&bytes);
        if found != expected {
            return Err(RecordError::SandboxDigest {
                expected: expected.to_string(),
                found,
            });
        }
    }

    Ok(RecordSummary {
        outcome: json::field(fields, "outcome")
            .and_then(Json::as_str)
            .unwrap_or("unknown")
            .to_string(),
        provider: json::field(fields, "provider")
            .and_then(Json::as_str)
            .unwrap_or("unknown")
            .to_string(),
        events: verified.events,
        head: verified.head,
    })
}
