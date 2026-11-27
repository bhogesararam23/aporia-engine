//! The v0.2 engine's journal events, in one place.
//!
//! The journal itself ([`crate::journal`]) is a chain of `(kind, payload)` lines. This module is
//! the contract for the kinds the iterative engine writes: one variant per event, one stable kind
//! string, one payload shape. A reader reconstructing a session -- or a resume deciding what
//! already happened -- matches on these kinds, so they are defined here rather than scattered as
//! literals through the engine.
//!
//! Payloads that already have a contract elsewhere (a plan, a finding, an experiment item, a file
//! change, a task, a delivery) travel as their JSON form directly; this module does not restate
//! their schemas. Every variant here is written by the engine, the tools or the resume path --
//! an event kind nobody writes is not a contract, it is a wish.

use aporia_store::Json;

/// One event the engine can journal.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// A new investigation began: the objective, the source, the provider, and the user's
    /// configured ceilings. A resume reads its permissions and budgets from here.
    SessionStarted {
        objective: String,
        source: String,
        provider: String,
        allow_exec: Vec<String>,
        allow_write: bool,
        max_turns: u64,
        max_tool_calls: u64,
        timeout_ms: u64,
        facts: u64,
        started: String,
    },
    /// An existing investigation was reopened. `interrupted` lists invocation ids that were
    /// started but never completed before the previous process ended.
    SessionResumed {
        events: u64,
        interrupted: Vec<String>,
    },
    /// The user paused the session; a resume continues from the recorded state.
    SessionPaused,
    /// The session reached an end state.
    SessionFinished {
        status: String,
        failure: Option<String>,
        turns: u64,
        calls: u64,
    },
    /// The user said something, in or between turns. The words themselves: a resume reconstructs
    /// the conversation from the record.
    UserMessage { text: String },
    /// The host said something into the conversation (an approval outcome, a ceiling refusal).
    HostNote { text: String },
    /// What the model was asked. `tools` names the schemas offered on this turn.
    ModelRequest {
        turn: u64,
        messages: u64,
        tools: Vec<String>,
    },
    /// What the model answered. `calls` is the array of `{id, name, arguments}` objects, with
    /// the arguments whole -- a resume may still need to run a call that was never started.
    ModelResponse {
        content: Option<String>,
        calls: Json,
    },
    /// A call the model asked for, with its arguments whole.
    ToolRequested {
        call: String,
        name: String,
        arguments: String,
    },
    /// A permitted invocation began. A resume that finds this without a completion reports the
    /// invocation as interrupted; it never runs it again blindly.
    ToolStarted { call: String, name: String },
    /// The invocation ended. `ok` false is a result the model sees, not a host failure. `call`
    /// is the model's call id; `invocation` is the evidence id the finding may cite.
    ToolCompleted {
        call: String,
        invocation: String,
        name: String,
        ok: bool,
        class: Option<String>,
        text: String,
    },
    /// A repeated call id was answered from the ledger instead of running again. The text is
    /// what the model was shown, so a resume reconstructs the same conversation.
    ToolReplayed { call: String, text: String },
    /// A plan version was proposed. `plan` carries the plan's own JSON contract.
    PlanProposed { version: u64, plan: Json },
    /// A person decided a plan version. The version is absent only when a refused revision
    /// could not be named.
    PlanDecided {
        version: Option<u64>,
        decision: String,
    },
    /// The engine paused to ask for a decision.
    ApprovalRequested {
        kind: String,
        title: String,
        detail: Json,
    },
    /// The decision arrived and was recorded before anything it permits runs.
    ApprovalDecided { kind: String, decision: String },
    /// An approval named a program the session's configured ceiling does not allow.
    ApprovalOutsideCeiling { program: String },
    /// An approval named a plan version that was not pending.
    ApprovalVersionNotPending { version: u64 },
    /// The approved scope grew by one program.
    ScopeExpanded { program: String },
    /// A backlog item was created or updated; the payload is its full JSON form, so the latest
    /// event per id is the item's state.
    ExperimentChanged { item: Json },
    /// A file change was applied in the workspace; the payload carries before/after identities.
    ChangeApplied { change: Json },
    /// A planned task's status changed; the payload is the task's full JSON form.
    TaskChanged { task: Json },
    /// A grounded finding, in the finding's own JSON contract.
    FindingRecorded { finding: Json },
    /// The model's closing delivery.
    Delivery { delivery: Json },
    /// A tool produced an artefact, identified by its digest.
    ArtifactRegistered {
        kind: String,
        path: String,
        sha256: String,
    },
    /// A changeset was exported to an explicitly authorised destination.
    ChangesetExported { destination: String, files: u64 },
}

impl Event {
    /// The stable kind string written to the journal.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::SessionStarted { .. } => "session.started",
            Self::SessionResumed { .. } => "session.resumed",
            Self::SessionPaused => "session.paused",
            Self::SessionFinished { .. } => "session.finished",
            Self::UserMessage { .. } => "user.message",
            Self::HostNote { .. } => "host.note",
            Self::ModelRequest { .. } => "model.request",
            Self::ModelResponse { .. } => "model.response",
            Self::ToolRequested { .. } => "tool.requested",
            Self::ToolStarted { .. } => "tool.started",
            Self::ToolCompleted { .. } => "tool.completed",
            Self::ToolReplayed { .. } => "tool.replayed",
            Self::PlanProposed { .. } => "plan.proposed",
            Self::PlanDecided { .. } => "plan.decided",
            Self::ApprovalRequested { .. } => "approval.requested",
            Self::ApprovalDecided { .. } => "approval.decided",
            Self::ApprovalOutsideCeiling { .. } => "approval.outside_the_ceiling",
            Self::ApprovalVersionNotPending { .. } => "approval.version_not_pending",
            Self::ScopeExpanded { .. } => "scope.expanded",
            Self::ExperimentChanged { .. } => "experiment.changed",
            Self::ChangeApplied { .. } => "change.applied",
            Self::TaskChanged { .. } => "task.changed",
            Self::FindingRecorded { .. } => "finding.recorded",
            Self::Delivery { .. } => "delivery",
            Self::ArtifactRegistered { .. } => "artifact.registered",
            Self::ChangesetExported { .. } => "changeset.exported",
        }
    }

    /// The payload written under the kind.
    #[must_use]
    #[allow(clippy::too_many_lines)]
    // One flat table from variant to payload: splitting it would scatter the contract it exists
    // to keep in one place.
    pub fn payload(&self) -> Json {
        match self {
            Self::SessionStarted {
                objective,
                source,
                provider,
                allow_exec,
                allow_write,
                max_turns,
                max_tool_calls,
                timeout_ms,
                facts,
                started,
            } => Json::object(vec![
                ("objective", Json::text(objective)),
                ("source", Json::text(source)),
                ("provider", Json::text(provider)),
                (
                    "allow_exec",
                    Json::Arr(allow_exec.iter().map(Json::text).collect()),
                ),
                ("allow_write", Json::Bool(*allow_write)),
                ("max_turns", Json::count(*max_turns)),
                ("max_tool_calls", Json::count(*max_tool_calls)),
                ("timeout_ms", Json::count(*timeout_ms)),
                ("facts", Json::count(*facts)),
                ("started", Json::text(started)),
            ]),
            Self::SessionResumed {
                events,
                interrupted,
            } => Json::object(vec![
                ("events", Json::count(*events)),
                (
                    "interrupted",
                    Json::Arr(interrupted.iter().map(Json::text).collect()),
                ),
            ]),
            Self::SessionPaused => Json::object(Vec::new()),
            Self::SessionFinished {
                status,
                failure,
                turns,
                calls,
            } => Json::object(vec![
                ("status", Json::text(status)),
                ("failure", failure.as_ref().map_or(Json::Null, Json::text)),
                ("turns", Json::count(*turns)),
                ("calls", Json::count(*calls)),
            ]),
            Self::UserMessage { text } | Self::HostNote { text } => {
                Json::object(vec![("text", Json::text(text))])
            }
            Self::ModelRequest {
                turn,
                messages,
                tools,
            } => Json::object(vec![
                ("turn", Json::count(*turn)),
                ("messages", Json::count(*messages)),
                ("tools", Json::Arr(tools.iter().map(Json::text).collect())),
            ]),
            Self::ModelResponse { content, calls } => Json::object(vec![
                ("content", content.as_ref().map_or(Json::Null, Json::text)),
                ("tool_calls", calls.clone()),
            ]),
            Self::ToolRequested {
                call,
                name,
                arguments,
            } => Json::object(vec![
                ("call", Json::text(call)),
                ("name", Json::text(name)),
                ("arguments", Json::text(arguments)),
            ]),
            Self::ToolStarted { call, name } => {
                Json::object(vec![("call", Json::text(call)), ("name", Json::text(name))])
            }
            Self::ToolCompleted {
                call,
                invocation,
                name,
                ok,
                class,
                text,
            } => Json::object(vec![
                ("call", Json::text(call)),
                ("invocation", Json::text(invocation)),
                ("name", Json::text(name)),
                ("ok", Json::Bool(*ok)),
                ("class", class.as_ref().map_or(Json::Null, Json::text)),
                ("text", Json::text(text)),
            ]),
            Self::ToolReplayed { call, text } => {
                Json::object(vec![("call", Json::text(call)), ("text", Json::text(text))])
            }
            Self::PlanProposed { version, plan } => Json::object(vec![
                ("version", Json::count(*version)),
                ("plan", plan.clone()),
            ]),
            Self::PlanDecided { version, decision } => Json::object(vec![
                ("version", version.map_or(Json::Null, Json::count)),
                ("decision", Json::text(decision)),
            ]),
            Self::ApprovalRequested {
                kind,
                title,
                detail,
            } => Json::object(vec![
                ("kind", Json::text(kind)),
                ("title", Json::text(title)),
                ("detail", detail.clone()),
            ]),
            Self::ApprovalDecided { kind, decision } => Json::object(vec![
                ("kind", Json::text(kind)),
                ("decision", Json::text(decision)),
            ]),
            Self::ApprovalOutsideCeiling { program } | Self::ScopeExpanded { program } => {
                Json::object(vec![("program", Json::text(program))])
            }
            Self::ApprovalVersionNotPending { version } => {
                Json::object(vec![("version", Json::count(*version))])
            }
            Self::ExperimentChanged { item } => item.clone(),
            Self::ChangeApplied { change } => change.clone(),
            Self::TaskChanged { task } => task.clone(),
            Self::FindingRecorded { finding } => finding.clone(),
            Self::Delivery { delivery } => delivery.clone(),
            Self::ArtifactRegistered { kind, path, sha256 } => Json::object(vec![
                ("kind", Json::text(kind)),
                ("path", Json::text(path)),
                ("sha256", Json::text(sha256)),
            ]),
            Self::ChangesetExported { destination, files } => Json::object(vec![
                ("destination", Json::text(destination)),
                ("files", Json::count(*files)),
            ]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[allow(clippy::too_many_lines)]
    // The fixture's purpose is to enumerate every variant; a shorter list would test less.
    fn samples() -> Vec<Event> {
        vec![
            Event::SessionStarted {
                objective: "improve the integration rule".to_string(),
                source: "example".to_string(),
                provider: "scripted".to_string(),
                allow_exec: vec!["python".to_string()],
                allow_write: true,
                max_turns: 32,
                max_tool_calls: 64,
                timeout_ms: 600_000,
                facts: 7,
                started: "2026-11-27T00:00:00Z".to_string(),
            },
            Event::SessionResumed {
                events: 12,
                interrupted: vec!["I3".to_string()],
            },
            Event::SessionPaused,
            Event::SessionFinished {
                status: "completed".to_string(),
                failure: None,
                turns: 4,
                calls: 9,
            },
            Event::UserMessage {
                text: "also check the boundary case".to_string(),
            },
            Event::HostNote {
                text: "The scope now permits `python`.".to_string(),
            },
            Event::ModelRequest {
                turn: 1,
                messages: 2,
                tools: vec!["list_files".to_string()],
            },
            Event::ModelResponse {
                content: None,
                calls: Json::Arr(Vec::new()),
            },
            Event::ToolRequested {
                call: "call-1".to_string(),
                name: "read_file".to_string(),
                arguments: "{}".to_string(),
            },
            Event::ToolStarted {
                call: "call-1".to_string(),
                name: "read_file".to_string(),
            },
            Event::ToolCompleted {
                call: "call-1".to_string(),
                invocation: "I1".to_string(),
                name: "read_file".to_string(),
                ok: true,
                class: None,
                text: "contents".to_string(),
            },
            Event::ToolReplayed {
                call: "call-1".to_string(),
                text: "contents".to_string(),
            },
            Event::PlanProposed {
                version: 1,
                plan: Json::object(Vec::new()),
            },
            Event::PlanDecided {
                version: Some(1),
                decision: "approve".to_string(),
            },
            Event::ApprovalRequested {
                kind: "plan".to_string(),
                title: "approve the plan".to_string(),
                detail: Json::Null,
            },
            Event::ApprovalDecided {
                kind: "plan".to_string(),
                decision: "approve".to_string(),
            },
            Event::ApprovalOutsideCeiling {
                program: "curl".to_string(),
            },
            Event::ApprovalVersionNotPending { version: 3 },
            Event::ScopeExpanded {
                program: "python".to_string(),
            },
            Event::ExperimentChanged {
                item: Json::object(vec![("id", Json::text("X1"))]),
            },
            Event::ChangeApplied {
                change: Json::object(Vec::new()),
            },
            Event::TaskChanged {
                task: Json::object(vec![("id", Json::text("T1"))]),
            },
            Event::FindingRecorded {
                finding: Json::object(Vec::new()),
            },
            Event::Delivery {
                delivery: Json::object(Vec::new()),
            },
            Event::ArtifactRegistered {
                kind: "file".to_string(),
                path: "workspace/out.json".to_string(),
                sha256: "ab".to_string(),
            },
            Event::ChangesetExported {
                destination: "out/patch".to_string(),
                files: 1,
            },
        ]
    }

    #[test]
    fn every_event_has_a_unique_stable_kind() {
        let mut kinds = BTreeSet::new();
        for event in samples() {
            assert!(
                kinds.insert(event.kind()),
                "duplicate kind {}",
                event.kind()
            );
            assert!(
                event.kind().contains('.') || event.kind() == "delivery",
                "kind {} does not follow the namespace convention",
                event.kind()
            );
        }
        assert_eq!(kinds.len(), 26);
    }

    #[test]
    fn every_payload_is_an_object_that_survives_a_json_round_trip() {
        for event in samples() {
            let payload = event.payload();
            assert!(
                crate::json::fields(&payload).is_some(),
                "{} payload is not an object",
                event.kind()
            );
            let text = payload.to_compact();
            let parsed = Json::parse(&text).expect("json");
            assert_eq!(parsed, payload, "{} changed in transit", event.kind());
        }
    }

    #[test]
    fn the_resume_event_carries_the_interrupted_invocations() {
        let event = Event::SessionResumed {
            events: 30,
            interrupted: vec!["I4".to_string(), "I7".to_string()],
        };
        let payload = event.payload();
        let fields = crate::json::fields(&payload).expect("object");
        let interrupted = crate::json::string_array(fields, "interrupted").expect("array");
        assert_eq!(interrupted, vec!["I4".to_string(), "I7".to_string()]);
    }
}
