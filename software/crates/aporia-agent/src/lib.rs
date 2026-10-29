//! # aporia-agent
//!
//! The application layer: one investigation, from a project on disk to a record someone else can
//! check.
//!
//! The pieces are deliberately separate. [`provider`] is the only place a language model is spoken
//! to; `inspect` is deterministic reading; `plan` is the schema and the refusals that keep a
//! recommendation attached to evidence; `orchestrator` is the state machine; `execute` runs the
//! approved tasks inside `aporia_sandbox`; `journal` and `record` are the byte contract; and
//! `report` renders what happened without asking a model again.
//!
//! The instrument is not linked into this crate. `instrument_run` starts the `aporia` binary in the
//! sandbox and reads the archive manifest it wrote. That keeps the archive the interface: a claim
//! this application produces can still be replayed with the instrument's own tools.

pub mod execute;
pub mod inspect;
pub mod journal;
pub mod json;
pub mod orchestrator;
pub mod plan;
pub mod prompt;
pub mod provider;
pub mod record;
pub mod report;

pub use execute::{Artifact, TaskResult, execute};
pub use inspect::{Fact, FactKind, InspectError, Inspection, InspectionSummary, inspect};
pub use journal::{Journal, RecordError, Verified, verify_journal};
pub use orchestrator::{
    AgentError, Approve, Delivery, Outcome, RunConfig, RunReport, Suggestion, run,
};
pub use plan::{
    Finding, FindingKind, Gap, Operation, Plan, Priority, Severity, Task, TaskTool, ValidatedPlan,
    ValidatedTask, parse_finding, parse_plan, validate_plan,
};
pub use provider::{
    Message, ModelProvider, ModelRequest, ModelResponse, ProviderError, Role, ToolCall, ToolSchema,
};
pub use record::{RecordDir, RecordSummary, verify_record};
pub use report::{machine_summary, render_plan, render_report};
