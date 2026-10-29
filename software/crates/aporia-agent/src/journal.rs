//! The investigation journal: one JSON event per line, chained by SHA-256.
//!
//! Every line carries the digest of the line before it. Editing any line breaks the next line's
//! `prev`; editing the last line breaks the head recorded in the manifest. This is not a security
//! claim -- anyone can rewrite the whole chain -- it is the same claim the archives make: the
//! record can tell you that the bytes are not the ones that were written.

use aporia_store::Json;
use aporia_store::digest::sha256_hex;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const ZERO_HEAD: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// Why a record could not be written or read.
#[derive(Debug)]
pub enum RecordError {
    Io {
        path: PathBuf,
        action: &'static str,
        source: io::Error,
    },
    Json {
        path: PathBuf,
        line: usize,
        reason: String,
    },
    /// A line's `seq` or `prev` does not match the chain.
    Chain {
        line: usize,
        seq: u64,
        expected_prev: String,
        found_prev: String,
    },
    /// The manifest's head is not the digest of the last journal line.
    Head {
        expected: String,
        found: String,
    },
    /// The manifest's event count is not the number of journal lines.
    Events {
        expected: u64,
        found: u64,
    },
    /// The manifest's sandbox digest is not the digest of `sandbox.json`.
    SandboxDigest {
        expected: String,
        found: String,
    },
    NotFinalised(PathBuf),
    Manifest {
        path: PathBuf,
        reason: String,
    },
}

impl std::fmt::Display for RecordError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io {
                path,
                action,
                source,
            } => write!(f, "cannot {action} {}: {source}", path.display()),
            Self::Json { path, line, reason } => write!(
                f,
                "{} line {} is not an event: {reason}",
                path.display(),
                line + 1
            ),
            Self::Chain {
                line,
                seq,
                expected_prev,
                found_prev,
            } => write!(
                f,
                "the journal chain breaks at line {} (seq {seq}): expected prev {expected_prev}, found {found_prev}",
                line + 1
            ),
            Self::Head { expected, found } => write!(
                f,
                "the manifest's journal head is {expected}, but the journal hashes to {found}"
            ),
            Self::Events { expected, found } => write!(
                f,
                "the manifest records {expected} events, but the journal has {found}"
            ),
            Self::SandboxDigest { expected, found } => write!(
                f,
                "the manifest's sandbox digest is {expected}, but sandbox.json hashes to {found}"
            ),
            Self::NotFinalised(path) => write!(
                f,
                "{} has no manifest.json; the investigation did not finish writing its record",
                path.display()
            ),
            Self::Manifest { path, reason } => {
                write!(f, "{} is not a usable manifest: {reason}", path.display())
            }
        }
    }
}

impl std::error::Error for RecordError {}

/// The result of checking a journal on its own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verified {
    pub events: u64,
    pub head: String,
}

/// An open journal. Appending is durable: the line is flushed and synced before the call returns.
#[derive(Debug)]
pub struct Journal {
    path: PathBuf,
    file: File,
    seq: u64,
    head: String,
}

impl Journal {
    /// Create a new journal. The record directory is new, so an existing file is a bug, not a
    /// continuation.
    pub fn create(path: &Path) -> Result<Self, RecordError> {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|source| RecordError::Io {
                path: path.to_path_buf(),
                action: "create the journal",
                source,
            })?;
        Ok(Self {
            path: path.to_path_buf(),
            file,
            seq: 0,
            head: ZERO_HEAD.to_string(),
        })
    }

    /// Append one event and return its sequence number.
    pub fn append(&mut self, kind: &str, payload: Json) -> Result<u64, RecordError> {
        let seq = self.seq;
        let event = Json::object(vec![
            ("seq", Json::count(seq)),
            ("at", Json::text(format_utc(unix_now()))),
            ("prev", Json::text(&self.head)),
            ("kind", Json::text(kind)),
            ("payload", payload),
        ]);
        let line = event.to_compact();
        let head = sha256_hex(line.as_bytes());
        self.file
            .write_all(line.as_bytes())
            .and_then(|()| self.file.write_all(b"\n"))
            .and_then(|()| self.file.flush())
            .and_then(|()| self.file.sync_data())
            .map_err(|source| RecordError::Io {
                path: self.path.clone(),
                action: "append to the journal",
                source,
            })?;
        self.head = head;
        self.seq += 1;
        Ok(seq)
    }

    #[must_use]
    pub fn head(&self) -> &str {
        &self.head
    }

    #[must_use]
    pub fn events(&self) -> u64 {
        self.seq
    }
}

/// Verify a journal's chain and return its final head.
pub fn verify_journal(path: &Path) -> Result<Verified, RecordError> {
    let text = std::fs::read_to_string(path).map_err(|source| RecordError::Io {
        path: path.to_path_buf(),
        action: "read the journal",
        source,
    })?;
    let mut expected_prev = ZERO_HEAD.to_string();
    let mut events = 0u64;
    let mut lines = text.split('\n').peekable();
    while let Some(line) = lines.next() {
        if line.is_empty() {
            if lines.peek().is_none() {
                break;
            }
            return Err(RecordError::Json {
                path: path.to_path_buf(),
                line: events as usize,
                reason: "empty line".to_string(),
            });
        }
        let value = Json::parse(line).map_err(|error| RecordError::Json {
            path: path.to_path_buf(),
            line: events as usize,
            reason: error.to_string(),
        })?;
        let Json::Obj(fields) = value else {
            return Err(RecordError::Json {
                path: path.to_path_buf(),
                line: events as usize,
                reason: "an event must be an object".to_string(),
            });
        };
        let field = |name: &str| {
            fields
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value)
        };
        let seq = field("seq")
            .and_then(Json::as_u64)
            .ok_or_else(|| RecordError::Json {
                path: path.to_path_buf(),
                line: events as usize,
                reason: "missing or non-integer seq".to_string(),
            })?;
        let prev = field("prev")
            .and_then(Json::as_str)
            .ok_or_else(|| RecordError::Json {
                path: path.to_path_buf(),
                line: events as usize,
                reason: "missing or non-string prev".to_string(),
            })?
            .to_string();
        if seq != events || prev != expected_prev {
            return Err(RecordError::Chain {
                line: events as usize,
                seq,
                expected_prev,
                found_prev: prev,
            });
        }
        expected_prev = sha256_hex(line.as_bytes());
        events += 1;
    }
    Ok(Verified {
        events,
        head: expected_prev,
    })
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

/// The Unix second, exposed for the record directory's name.
#[must_use]
pub fn now_seconds() -> u64 {
    unix_now()
}

/// UTC without a dependency, from the Unix second.
#[must_use]
pub fn format_utc(seconds: u64) -> String {
    let days = i64::try_from(seconds / 86_400).unwrap_or(i64::MAX);
    let remainder = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        remainder / 3_600,
        (remainder % 3_600) / 60,
        remainder % 60
    )
}

// Howard Hinnant's days-to-civil algorithm; proleptic Gregorian, no leap-second fiction.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    (
        year,
        u32::try_from(month).unwrap_or(0),
        u32::try_from(day).unwrap_or(0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_utc_seconds_format_as_the_right_dates() {
        assert_eq!(format_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_utc(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(format_utc(1_700_000_000), "2023-11-14T22:13:20Z");
    }
}
