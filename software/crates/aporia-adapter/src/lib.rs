//! # aporia-adapter
//!
//! The external-computation boundary: a program APORIA did not parse, run as a child process, whose
//! answers go to the same campaign driver as an interpreted model's.
//!
//! This crate is the layer the audit named "adapter" and it deliberately contains no analysis. It
//! implements `aporia_runtime::Executor`, so the search, the evidence channels, the calibration and
//! the atlas all stay in the crates that already own them; what lives here is the process, the pipe,
//! the wire format and the accounting of what the program cost.
//!
//! - [`protocol`] — one JSON object per line each way, shared by both sides. The example program
//!   decodes requests with the same functions APORIA uses to decode responses, so the format cannot
//!   quietly mean two things.
//! - [`program`] — the child process: launch, ask, answer, and the explicit failure modes
//!   (cannot launch, died mid-run, refused to answer, malformed line, timeout).
//!
//! ## Why the JSON code is borrowed
//!
//! `aporia_store::Json` does the number encoding. It is not a storage-specific type — it is this
//! workspace's hand-written JSON value, parser and writer, already tested for the property that
//! matters most here: a float written and read back is the same 64 bits, with the non-finite cases
//! written as the strings the archives already use. Writing a second JSON reader for the adapter
//! would have produced two encodings of the same number and a replay comparison that could disagree
//! with itself, which is precisely the class of bug this project has been hunting.
//!
//! ## What the boundary does not do
//!
//! It does not run cloud services, and it needs no external API or interpreter: the child is a program
//! on this machine, launched by `std::process`. It does not accept a model that mixes external outputs
//! with the model's own equations — the DSL refuses that, and the reason is recorded there.

pub mod example;
pub mod program;
pub mod protocol;

pub use program::{AdapterError, Program, ProgramSpec};
pub use protocol::{
    Answer, ProtocolError, decode_request, decode_response, describe, encode_refusal,
    encode_request, encode_response,
};
