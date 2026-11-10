//! The v0.2 engine's journal events, in one place.
//!
//! The journal itself ([`crate::journal`]) is a chain of `(kind, payload)` lines. This module is
//! the contract for the kinds the iterative engine writes: one variant per event, one stable kind
//! string, one payload shape. A reader reconstructing a session -- or a resume deciding what
//! already happened -- matches on these kinds, so they are defined here rather than scattered as
//! literals through the engine.
//!
//! Payloads that already have a contract elsewhere (a plan, a finding, an experiment item, a file
//! change) travel as their JSON form; this module does not restate their schemas.

use aporia_store::Json;

/// One event the engine can journal.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// A new investigation began. `limits` is the user's configured ceiling, as JSON.
    SessionStarted {
        objective: String,
        source: String,
        provider: String,
        limits: Json,
    },
    /// An existing investigation was reopened. `interrupted` lists invocation ids that were
    /// started but never completed before the previous process ended.
    SessionResumed {
        events: u64,
        interrupted: Vec<String>,
    },
    /// The user paused the session.
    SessionPaused,
    /// The user cancelled; nothing further runs.
    SessionCancelled { reason: String },
    /// The session reached an end state.
    SessionFinished {
        status: String,
        failure: Option<String>,
        turns: u64,
        calls: u64,
    },
    /// The user said something, in or between turns.
    UserMessage { text: String },
    /// What the model was asked. `tools` names the schemas offered on this turn.
    ModelRequest {
        turn: u64,
        messages: u64,
        tools: Vec<String>,
    },
    /// What the model answered. `calls` is the array of `{id, name, arguments}` objects.
    ModelResponse {
        content: Option<String>,
        calls: Json,
    },
    /// A requested call passed validation and got an invocation identity.
    ToolRequested {
        invocation: String,
        call: String,
        name: String,
        arguments: String,
    },
    /// The host refused a call before running it: bad arguments, unknown tool, or no permission.
    ToolRejected {
        call: String,
        name: String,
        class: String,
        reason: String,
    },
    /// A permitted invocation began. A resume that finds this without a completion reports the
    /// invocation as interrupted; it never runs it again blindly.
    ToolStarted { invocation: String, name: String },
    /// The invocation ended. `ok` false is a result the model sees, not a host failure.
    ToolCompleted {
        invocation: String,
        name: String,
        ok: bool,
        text: String,
        data: Json,
        duration_ms: u64,
    },
    /// A plan version was proposed. `plan` carries the plan's own JSON contract.
    PlanProposed { version: u64, plan: Json },
    /// A person decided a plan version.
    PlanDecided { version: u64, decision: String },
    /// The engine paused to ask for a decision.
    ApprovalRequested {
        kind: String,
        title: String,
        detail: Json,
    },
    /// The decision arrived and was recorded before anything it permits runs.
    ApprovalDecided { kind: String, decision: String },
    /// A backlog item was created or updated; `item` is its full JSON form, so the latest event
    /// per id is the item's state.
    ExperimentChanged { item: Json },
    /// An approved experiment began running.
    ExperimentStarted { id: String },
    /// An experiment ended: completed, failed or inconclusive, with its result JSON.
    ExperimentFinished {
        id: String,
        status: String,
        result: Json,
    },
    /// A file change was applied in the workspace; `change` carries before/after identities.
    ChangeApplied { change: Json },
    /// A change was rolled back to its recorded before-state.
    ChangeReverted { path: String },
    /// A changeset was exported to an explicitly authorised destination.
    ChangesetExported { destination: String, files: u64 },
    /// A grounded finding, in the finding's own JSON contract.
    FindingRecorded { finding: Json },
    /// The model's closing delivery.
    Delivery { delivery: Json },
}

impl Event {
    /// The stable kind string written to the journal.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::SessionStarted { .. } => "session.started",
            Self::SessionResumed { .. } => "session.resumed",
            Self::SessionPaused => "session.paused",
            Self::SessionCancelled { .. } => "session.cancelled",
            Self::SessionFinished { .. } => "session.finished",
            Self::UserMessage { .. } => "user.message",
            Self::ModelRequest { .. } => "model.request",
            Self::ModelResponse { .. } => "model.response",
            Self::ToolRequested { .. } => "tool.requested",
            Self::ToolRejected { .. } => "tool.rejected",
            Self::ToolStarted { .. } => "tool.started",
            Self::ToolCompleted { .. } => "tool.completed",
            Self::PlanProposed { .. } => "plan.proposed",
            Self::PlanDecided { .. } => "plan.decided",
            Self::ApprovalRequested { .. } => "approval.requested",
            Self::ApprovalDecided { .. } => "approval.decided",
            Self::ExperimentChanged { .. } => "experiment.changed",
            Self::ExperimentStarted { .. } => "experiment.started",
            Self::ExperimentFinished { .. } => "experiment.finished",
            Self::ChangeApplied { .. } => "change.applied",
            Self::ChangeReverted { .. } => "change.reverted",
            Self::ChangesetExported { .. } => "changeset.exported",
            Self::FindingRecorded { .. } => "finding.recorded",
            Self::Delivery { .. } => "delivery",
        }
    }

    /// The payload written under the kind.
    #[must_use]
    pub fn payload(&self) -> Json {
        match self {
            Self::SessionStarted {
                objective,
                source,
                provider,
                limits,
            } => Json::object(vec![
                ("objective", Json::text(objective)),
                ("source", Json::text(source)),
                ("provider", Json::text(provider)),
                ("limits", limits.clone()),
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
            Self::SessionCancelled { reason } => Json::object(vec![("reason", Json::text(reason))]),
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
            Self::UserMessage { text } => Json::object(vec![("text", Json::text(text))]),
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
                invocation,
                call,
                name,
                arguments,
            } => Json::object(vec![
                ("invocation", Json::text(invocation)),
                ("call", Json::text(call)),
                ("name", Json::text(name)),
                ("arguments", Json::text(arguments)),
            ]),
            Self::ToolRejected {
                call,
                name,
                class,
                reason,
            } => Json::object(vec![
                ("call", Json::text(call)),
                ("name", Json::text(name)),
                ("class", Json::text(class)),
                ("reason", Json::text(reason)),
            ]),
            Self::ToolStarted { invocation, name } => Json::object(vec![
                ("invocation", Json::text(invocation)),
                ("name", Json::text(name)),
            ]),
            Self::ToolCompleted {
                invocation,
                name,
                ok,
                text,
                data,
                duration_ms,
            } => Json::object(vec![
                ("invocation", Json::text(invocation)),
                ("name", Json::text(name)),
                ("ok", Json::Bool(*ok)),
                ("text", Json::text(text)),
                ("data", data.clone()),
                ("duration_ms", Json::count(*duration_ms)),
            ]),
            Self::PlanProposed { version, plan } => Json::object(vec![
                ("version", Json::count(*version)),
                ("plan", plan.clone()),
            ]),
            Self::PlanDecided { version, decision } => Json::object(vec![
                ("version", Json::count(*version)),
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
            Self::ExperimentChanged { item } => Json::object(vec![("item", item.clone())]),
            Self::ExperimentStarted { id } => Json::object(vec![("id", Json::text(id))]),
            Self::ExperimentFinished { id, status, result } => Json::object(vec![
                ("id", Json::text(id)),
                ("status", Json::text(status)),
                ("result", result.clone()),
            ]),
            Self::ChangeApplied { change } => Json::object(vec![("change", change.clone())]),
            Self::ChangeReverted { path } => Json::object(vec![("path", Json::text(path))]),
            Self::ChangesetExported { destination, files } => Json::object(vec![
                ("destination", Json::text(destination)),
                ("files", Json::count(*files)),
            ]),
            Self::FindingRecorded { finding } => Json::object(vec![("finding", finding.clone())]),
            Self::Delivery { delivery } => Json::object(vec![("delivery", delivery.clone())]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn samples() -> Vec<Event> {
        vec![
            Event::SessionStarted {
                objective: "improve the integration rule".to_string(),
                source: "example".to_string(),
                provider: "scripted".to_string(),
                limits: Json::object(Vec::new()),
            },
            Event::SessionResumed {
                events: 12,
                interrupted: vec!["I3".to_string()],
            },
            Event::SessionPaused,
            Event::SessionCancelled {
                reason: "user asked".to_string(),
            },
            Event::SessionFinished {
                status: "completed".to_string(),
                failure: None,
                turns: 4,
                calls: 9,
            },
            Event::UserMessage {
                text: "also check the boundary case".to_string(),
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
                invocation: "I1".to_string(),
                call: "call-1".to_string(),
                name: "read_file".to_string(),
                arguments: "{}".to_string(),
            },
            Event::ToolRejected {
                call: "call-2".to_string(),
                name: "run_command".to_string(),
                class: "denied".to_string(),
                reason: "program not approved".to_string(),
            },
            Event::ToolStarted {
                invocation: "I1".to_string(),
                name: "read_file".to_string(),
            },
            Event::ToolCompleted {
                invocation: "I1".to_string(),
                name: "read_file".to_string(),
                ok: true,
                text: "contents".to_string(),
                data: Json::Null,
                duration_ms: 2,
            },
            Event::PlanProposed {
                version: 1,
                plan: Json::object(Vec::new()),
            },
            Event::PlanDecided {
                version: 1,
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
            Event::ExperimentChanged {
                item: Json::object(vec![("id", Json::text("X1"))]),
            },
            Event::ExperimentStarted {
                id: "X1".to_string(),
            },
            Event::ExperimentFinished {
                id: "X1".to_string(),
                status: "completed".to_string(),
                result: Json::Null,
            },
            Event::ChangeApplied {
                change: Json::object(Vec::new()),
            },
            Event::ChangeReverted {
                path: "src/rule.py".to_string(),
            },
            Event::ChangesetExported {
                destination: "out/patch".to_string(),
                files: 1,
            },
            Event::FindingRecorded {
                finding: Json::object(Vec::new()),
            },
            Event::Delivery {
                delivery: Json::object(Vec::new()),
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
        assert_eq!(kinds.len(), 24);
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
