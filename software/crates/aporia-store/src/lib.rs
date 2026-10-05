//! # aporia-store
//!
//! The experiment archive: what ran, in what configuration, on what model, with what evidence, and
//! whether it happens the same way twice.
//!
//! - [`manifest`] — what the experiment was, including the calibration and channel correlation the
//!   scores were produced with, and a digest of every artefact
//! - [`json`] — the JSON writer and reader this crate and the adapter protocol use, with float and
//!   integer handling controlled here rather than by a dependency
//! - [`records`] — the fixed-record binary form of every observation, stored as bit patterns
//! - [`store`] — the directory layout, the writing, the reading, and the finding archive
//! - [`replay`] — re-execution of a stored experiment against its own A-IR, compared bit for bit
//! - [`compare`] — two archives read against each other, field by field, with nothing re-executed
//! - [`compare`] — two archives read against each other, field by field, without executing anything
//!
//! The point of the whole crate is the last item. Spec §23 requires provenance and replay, and §21's
//! example finding ends with `aporia replay findings/27.apx`. A claim about a trust boundary is only
//! evidence if someone else, later, on this machine or another, can make the same numbers appear.
//! That is testable, so it is tested: write a run, read it back, replay it, and check the outputs,
//! the traces, the raised flags and the instruction-step counts are identical.
//!
//! A directory is never overwritten, and a manifest is written last, so the presence of a manifest
//! means the archive is complete.

pub mod compare;
pub mod digest;
pub mod json;
pub mod manifest;
pub mod records;
pub mod replay;
pub mod store;

pub use compare::{Change, Field, Section, calibration, configuration, identity, size};
pub use json::{Json, JsonError};
pub use manifest::{Counts, Environment, Manifest};
pub use replay::{Mismatch, Replay, replay, replay_dir, replay_loaded};
pub use store::{Loaded, Receipt, Run, Store, StoreError, StoredFinding, bands_csv, label_text};
