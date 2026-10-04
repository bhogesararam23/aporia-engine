//! # aporia-runtime
//!
//! Execution of A-IR models. Everything upstream of a Trust Atlas begins as an observation produced
//! here, so the two things this crate cares about are: one definition of each operation, shared by
//! every path, and a record of what went wrong while evaluating.
//!
//! - [`value`] — runtime values, the flags an execution raises, and the floating-point modes
//! - [`ops`] — the arithmetic itself, called by every backend
//! - [`interp`] — the scalar interpreter, which is the reference for what a model means
//! - [`exec`] — the execution boundary a campaign is actually given, with the interpreter as one
//!   implementation of it
//! - [`batch`] — many candidates through the same model, lane-major, the shape a vector or GPU
//!   backend copies directly
//!
//! A scalar and a batched run of the same model on the same inputs must agree, and that
//! requirement is a test rather than a hope. When they disagree, APORIA has found something:
//! that is the differential channel, and it only works because semantics live in exactly one place.
pub mod batch;
pub mod exec;
pub mod interp;
pub mod observe;
pub mod ops;
pub mod value;

pub use batch::{BatchOutcome, run_batch};
pub use exec::{Executor, Interp};
pub use interp::{Outcome, run};
pub use observe::{Observation, Records};
pub use value::{ExecConfig, Flags, FpMode, Value};
