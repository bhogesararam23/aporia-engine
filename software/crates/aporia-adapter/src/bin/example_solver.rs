//! A worked example of an external program, and the one the adapter tests drive.
//!
//! It is a real process, launched over a pipe, speaking the protocol with the same `aporia-adapter`
//! functions APORIA uses — so a test that passes against it has exercised the boundary rather than a
//! mock of it. Anyone writing a solver adapter can copy this file's shape: read a line, decode the
//! request, answer one value per declared output, exit when the pipe closes.
//!
//! The physics is a cantilever beam under a tip load that is deliberately trivial — deflection is a
//! positive number up to 60 N and negative beyond it, so a model declaring `require deflection >= 0`
//! has a region for APORIA to find. The point is not the beam. The point is that one side of the pipe
//! knows nothing about the analysis and the other knows nothing about the arithmetic.
//!
//! ## The misbehaviour switches
//!
//! `APORIA_PROGRAM_MODE` makes the failure paths testable instead of simulated. Each mode is a thing
//! a real program does: a solver that exits when its matrix is singular, a build that prints a banner
//! before the answers, a script that waits on input it never gets, a language that writes `nan` where
//! the protocol wants a number.
//!
//! - `answer` (default) — the beam, plus a work count.
//! - `exit` — answers once, then exits nonzero, as if the second load had broken the solver.
//! - `garbage` — answers with text that is not the protocol.
//! - `banner` — prints a non-protocol line first, which is the most likely real-world mistake.
//! - `arity` — answers with two values for a one-output model.
//! - `refuse` — answers `{"error": ...}`, a refusal rather than a value.
//! - `nan` — answers with the value NaN, which the protocol *does* carry, as a string.
//! - `hang` — stops answering, so the caller's timeout is what ends the run.

//!
//! The loop itself lives in `aporia_adapter::example`, so a binary that needs a program to spawn
//! is two lines and the protocol semantics are written once.

fn main() {
    aporia_adapter::example::serve();
}
