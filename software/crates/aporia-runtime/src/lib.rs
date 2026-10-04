//! # aporia-runtime
//!
//! Execution of A-IR models. Everything upstream of a Trust Atlas begins as an observation produced
//! here, so the two things this crate cares about are: one definition of each operation, shared by
//! every path, and a record of what went wrong while evaluating.
//!
//! - [`value`] — runtime values, the flags an execution raises, and the floating-point modes
//! - [`ops`] — the arithmetic itself, called by every backend
//! - [`interp`] — the scalar interpreter, which is the reference for what a model means
//! - [`batch`] — many candidates through the same model, lane-major, the shape a vector or GPU
//!   backend copies directly
//!
//! A scalar and a batched run of the same model on the same inputs must agree, and that
//! requirement is a test rather than a hope. When they disagree, APORIA has found something:
//! that is the differential channel, and it only works because semantics live in exactly one place.
pub mod ops;
pub mod value;

pub use value::{ExecConfig, Flags, FpMode, Value};
