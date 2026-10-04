//! # aporia-evidence
//!
//! The five evidence channels the specification names, as data rather than as log lines.
//!
//! - [`channel`] — [`Channel`], [`Evidence`], [`Subject`]: what was found, how strong it is, and
//!   which executions it came from
//! - [`calibrate`] — turning a channel's own units into comparable strengths, using the median of
//!   the experiment's own magnitudes so one channel cannot dominate by arithmetic accident
//! - [`fuse`] — combining channels into a risk score while discounting evidence that describes the
//!   same event twice, and Pareto ranking when a single number would hide a disagreement
//!
//! A risk score here is a ranking quantity with a documented meaning. It is not a probability, and
//! nothing downstream should treat a low score as proof that a model is correct.
pub mod calibrate;
pub mod channel;
pub mod fuse;

pub use calibrate::Calibrator;
pub use channel::{Channel, Confidence, Evidence, EvidenceSet, PatternKind, Subject};
pub use fuse::{ChannelCorrelation, Contribution, Risk, fuse, strongest};
