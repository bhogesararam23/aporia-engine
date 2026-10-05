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
//! A scalar and a batched run of the same model on the same inputs must agree, and that requirement
//! is a test rather than a hope — semantics living in exactly one place is what makes the agreement
//! worth testing.
//!
//! Being precise about which pair the Differential channel actually compares during a campaign: it is
//! the scalar runtime against `aporia_numerics::reference`, the independent double-double evaluator,
//! because that comparison exists at the point where it costs an evaluation. Scalar-against-batch is
//! an invariant this crate tests; the batched path has no caller in the campaign, so it is not
//! currently a source of evidence, and a channel documented as measuring it would be reporting a
//! comparison nothing runs.
pub mod batch;
pub mod exec;
pub mod interp;
pub mod observe;
pub mod ops;
pub mod value;

pub use batch::{BatchOutcome, run_batch};
pub use exec::{Executor, Interp, needs_adapter};
pub use interp::{Outcome, run};
pub use observe::{Observation, Records};
pub use value::{ExecConfig, Flags, FpMode, Value};
