//! Crash-safe resume: rebuild an investigation from its journal and continue it.
//!
//! The journal is authoritative. The checkpoint is a derived summary and is never trusted over
//! the record: a resume replays every event, reconstructs the conversation, the investigation
//! state and the invocation ledger, and only then appends. A call recorded as started but never
//! completed is reported to the model as interrupted, never re-executed; a call requested but
//! never started is run once, because it never ran. A finished record is history, not work to
//! resume, and a broken chain is a named refusal.

use crate::engine::{Controller, Engine, EngineConfig, EngineError, INTERRUPTED_TEXT};
use crate::inspect::Inspection;
use crate::investigation::{Experiment, FileChange, Investigation, PlanRevision, Proposal};
use crate::json;
use crate::orchestrator::Delivery;
use crate::plan::Finding;
use crate::provider::{Message, ModelProvider, Role, ToolCall};
use crate::record::RecordDir;
use crate::scope::{ApprovalKind, ApprovalRequest, Decision, Scope};
use crate::session::{Phase, SessionStatus};
use crate::tools::result::{ErrorClass, ToolFailure, ToolOutput};
use crate::tools::{ArtifactRef, InvocationTable, Registry, ToolResult};
use aporia_sandbox::{CopyReport, FileRecord, Sandbox};
use aporia_store::Json;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

/// What a resume needs: the record directory, a provider to continue with, and the instrument.
pub struct ResumeConfig {
    pub record: PathBuf,
    pub provider: Box<dyn ModelProvider>,
    pub instrument: PathBuf,
}

impl std::fmt::Debug for ResumeConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResumeConfig")
            .field("record", &self.record)
            .field("provider", &self.provider.describe())
            .field("instrument", &self.instrument)
            .finish()
    }
}

/// The ceilings and identity the first event recorded.
struct Started {
    objective: String,
    source: String,
    allow_exec: Vec<String>,
    allow_write: bool,
    max_turns: u64,
    max_tool_calls: u64,
    timeout_ms: u64,
    opened: String,
}

fn input(reason: String) -> EngineError {
    EngineError::Input(reason)
}

fn parse_started(payload: &Json) -> Result<Started, EngineError> {
    let fields = json::fields(payload)
        .ok_or_else(|| input("session.started is not an object".to_string()))?;
    let read = |key: &str| -> Result<u64, EngineError> {
        json::u64_value(fields, key).map_err(|reason| {
            input(format!(
                "this record predates resumable budgets ({reason}); it cannot be resumed"
            ))
        })
    };
    Ok(Started {
        objective: json::string(fields, "objective")
            .map_err(input)?
            .to_string(),
        source: json::string(fields, "source").map_err(input)?.to_string(),
        allow_exec: json::string_array(fields, "allow_exec").map_err(input)?,
        allow_write: json::required(fields, "allow_write")
            .ok()
            .and_then(Json::as_bool)
            .ok_or_else(|| input("allow_write is not a boolean".to_string()))?,
        max_turns: read("max_turns")?,
        max_tool_calls: read("max_tool_calls")?,
        timeout_ms: read("timeout_ms")?,
        opened: json::string(fields, "started").map_err(input)?.to_string(),
    })
}

fn parse_kind(text: &str) -> Result<ApprovalKind, EngineError> {
    match text {
        "plan" => Ok(ApprovalKind::Plan),
        "scope_expansion" => Ok(ApprovalKind::ScopeExpansion),
        "changeset_export" => Ok(ApprovalKind::ChangesetExport),
        other => Err(input(format!("unknown approval kind `{other}`"))),
    }
}

fn parse_decision(text: &str) -> Result<Decision, EngineError> {
    match text {
        "approve" => Ok(Decision::Approve),
        "refuse" => Ok(Decision::Refuse),
        other => Err(input(format!("unknown decision `{other}`"))),
    }
}

/// Rebuild the result a `tool.completed` event recorded.
fn recorded_result(ok: bool, class: Option<&str>, text: &str) -> Result<ToolResult, EngineError> {
    if ok {
        return Ok(Ok(ToolOutput {
            text: text.to_string(),
            data: Json::Null,
            artifacts: Vec::new(),
        }));
    }
    let class = class.ok_or_else(|| input("a failed completion has no class".to_string()))?;
    let class = ErrorClass::parse(class).map_err(input)?;
    let class_name = class.as_str();
    let prefix = format!("error[{class_name}]: ");
    let message = text.strip_prefix(&prefix).unwrap_or(text);
    Ok(Err(ToolFailure::new(class, message)))
}

/// The evidence detail a completion produced: the same derivation the engine used live.
fn evidence_detail(ok: bool, class: Option<&str>, text: &str) -> String {
    let source = if ok {
        text.to_string()
    } else {
        let prefix = class.map_or_else(String::new, |class| format!("error[{class}]: "));
        text.strip_prefix(&prefix).unwrap_or(text).to_string()
    };
    source.chars().take(400).collect()
}

fn parse_copy_report(value: &Json) -> Result<CopyReport, EngineError> {
    let fields =
        json::fields(value).ok_or_else(|| input("sandbox.json is not an object".into()))?;
    let files = json::array(fields, "files")
        .map_err(input)?
        .iter()
        .map(|file| {
            let fields = json::fields(file).ok_or_else(|| input("a file record".to_string()))?;
            Ok(FileRecord {
                path: json::string(fields, "path").map_err(input)?.to_string(),
                bytes: json::u64_value(fields, "bytes").map_err(input)?,
                sha256: json::string(fields, "sha256").map_err(input)?.to_string(),
            })
        })
        .collect::<Result<Vec<_>, EngineError>>()?;
    Ok(CopyReport {
        files,
        ignored: json::string_array(fields, "ignored").map_err(input)?,
        total_bytes: json::u64_value(fields, "total_bytes").map_err(input)?,
    })
}

/// Everything the event replay reconstructs, before the trailing calls are answered.
struct Replay {
    started: Option<Started>,
    messages: Vec<Message>,
    invocations: InvocationTable,
    investigation: Investigation,
    approvals: Vec<(ApprovalKind, Decision)>,
    artifacts: Vec<ArtifactRef>,
    pending: Option<ApprovalRequest>,
    requested: BTreeMap<String, ToolCall>,
    failed_signatures: BTreeMap<String, u32>,
    turns: usize,
    calls: usize,
    phase: Phase,
}

/// Replay every journal event into the state the live engine would have held. Unknown kinds are
/// ignored so a newer record can still be read by an older build; the chain check already proved
/// the bytes are the ones that were written.
#[allow(clippy::too_many_lines)]
// One flat table from kind to effect: splitting it would scatter the replay contract it keeps.
fn replay_events(text: &str) -> Result<Replay, EngineError> {
    let mut started: Option<Started> = None;
    let mut messages: Vec<Message> = Vec::new();
    let mut invocations = InvocationTable::new();
    let mut investigation = Investigation::default();
    let mut approvals: Vec<(ApprovalKind, Decision)> = Vec::new();
    let mut artifacts: Vec<ArtifactRef> = Vec::new();
    let mut pending: Option<ApprovalRequest> = None;
    let mut requested: BTreeMap<String, ToolCall> = BTreeMap::new();
    let mut failed_signatures: BTreeMap<String, u32> = BTreeMap::new();
    let mut turns = 0usize;
    let mut calls = 0usize;
    let mut phase = Phase::Inspecting;

    for line in text.lines() {
        if line.is_empty() {
            continue;
        }
        let value = Json::parse(line).map_err(|error| input(format!("journal line: {error}")))?;
        let fields =
            json::fields(&value).ok_or_else(|| input("a journal line is not an object".into()))?;
        let kind = json::string(fields, "kind").map_err(input)?;
        let payload = json::required(fields, "payload").map_err(input)?;
        let pfields = json::fields(payload);
        let pstr = |key: &str| -> Result<String, EngineError> {
            pfields
                .and_then(|fields| json::string(fields, key).ok())
                .map(str::to_string)
                .ok_or_else(|| input(format!("{kind}: missing string `{key}`")))
        };
        match kind {
            "session.started" => {
                if started.is_some() {
                    return Err(input(
                        "the record has two session.started events".to_string(),
                    ));
                }
                started = Some(parse_started(payload)?);
            }
            "user.message" | "host.note" => {
                messages.push(Message::user(pstr("text")?));
            }
            "model.request" => {
                turns += 1;
            }
            "model.response" => {
                let content = pfields
                    .and_then(|fields| json::optional_string(fields, "content").ok())
                    .flatten()
                    .map(str::to_string);
                let mut tool_calls = Vec::new();
                if let Some(calls) = pfields
                    .and_then(|fields| json::field(fields, "tool_calls"))
                    .and_then(Json::as_array)
                {
                    for call in calls {
                        let cfields = json::fields(call)
                            .ok_or_else(|| input("a tool call is not an object".into()))?;
                        tool_calls.push(ToolCall {
                            id: json::string(cfields, "id").map_err(input)?.to_string(),
                            name: json::string(cfields, "name").map_err(input)?.to_string(),
                            arguments: json::string(cfields, "arguments")
                                .map_err(input)?
                                .to_string(),
                        });
                    }
                }
                messages.push(Message::assistant(
                    content.unwrap_or_default(),
                    tool_calls.clone(),
                ));
                if tool_calls.is_empty() {
                    messages.push(Message::user(
                        "Your message contained no tool call. Use the tools you were given, or finish.",
                    ));
                }
            }
            "tool.requested" => {
                let call = pstr("call")?;
                requested.insert(
                    call.clone(),
                    ToolCall {
                        id: call,
                        name: pstr("name")?,
                        arguments: pstr("arguments")?,
                    },
                );
            }
            "tool.started" => {
                calls += 1;
                invocations.replay_started(&pstr("call")?);
            }
            "tool.completed" => {
                let call = pstr("call")?;
                let invocation = pstr("invocation")?;
                let name = pstr("name")?;
                let ok = pfields
                    .and_then(|fields| json::field(fields, "ok"))
                    .and_then(Json::as_bool)
                    .ok_or_else(|| input("tool.completed: ok is not a boolean".to_string()))?;
                let class = pfields
                    .and_then(|fields| json::optional_string(fields, "class").ok())
                    .flatten();
                let completion_text = pstr("text")?;
                let result = recorded_result(ok, class, &completion_text)?;
                if !ok && let Some(requested_call) = requested.get(&call) {
                    let signature = format!("{}:{}", requested_call.name, requested_call.arguments);
                    *failed_signatures.entry(signature).or_insert(0) += 1;
                }
                invocations.replay_completed(&call, result);
                let detail = evidence_detail(ok, class, &completion_text);
                let evidence_id =
                    investigation.add_evidence("invocation", format!("{name}: {detail}"));
                if evidence_id != invocation {
                    return Err(input(format!(
                        "the record's evidence ids do not match its journal ({evidence_id} vs {invocation})"
                    )));
                }
                messages.push(Message::tool(
                    &call,
                    format!("[{invocation}] {completion_text}"),
                ));
            }
            "tool.replayed" => {
                messages.push(Message::tool(pstr("call")?, pstr("text")?));
            }
            "plan.proposed" => {
                let version = pfields
                    .and_then(|fields| json::u64_value(fields, "version").ok())
                    .ok_or_else(|| input("plan.proposed: no version".to_string()))?;
                let plan = pfields
                    .and_then(|fields| json::field(fields, "plan"))
                    .ok_or_else(|| input("plan.proposed: no plan".to_string()))?;
                investigation.revisions.push(PlanRevision {
                    version,
                    proposal: Proposal::from_json(plan).map_err(input)?,
                    decision: None,
                    scope: None,
                });
            }
            "plan.decided" => {
                let decision = parse_decision(&pstr("decision")?)?;
                let version = pfields
                    .and_then(|fields| json::field(fields, "version"))
                    .and_then(Json::as_u64);
                if let Some(version) = version
                    && let Some(revision) = investigation
                        .revisions
                        .iter_mut()
                        .find(|revision| revision.version == version)
                {
                    revision.decision = Some(decision);
                    if decision == Decision::Approve {
                        let scope = revision.proposal.scope.as_scope();
                        revision.scope = Some(scope.clone());
                        investigation.scope = Some(scope);
                        investigation.approved_version = Some(version);
                        phase = Phase::Working;
                    }
                }
            }
            "approval.requested" => {
                pending = Some(ApprovalRequest {
                    kind: parse_kind(&pstr("kind")?)?,
                    title: pstr("title")?,
                    detail: pfields
                        .and_then(|fields| json::field(fields, "detail"))
                        .cloned()
                        .unwrap_or(Json::Null),
                });
            }
            "approval.decided" => {
                approvals.push((
                    parse_kind(&pstr("kind")?)?,
                    parse_decision(&pstr("decision")?)?,
                ));
                pending = None;
            }
            "scope.expanded" => {
                let program = pstr("program")?;
                let limits = started
                    .as_ref()
                    .ok_or_else(|| input("scope.expanded before session.started".to_string()))?;
                let mut scope = investigation.scope.clone().unwrap_or_else(|| Scope {
                    allow_exec: Vec::new(),
                    allow_write: false,
                    max_turns: limits.max_turns,
                    max_tool_calls: limits.max_tool_calls,
                    timeout_ms: limits.timeout_ms,
                });
                if !scope.permits_exec(&program) {
                    scope.allow_exec.push(program);
                }
                investigation.scope = Some(scope);
            }
            "experiment.changed" => {
                investigation.upsert_experiment(Experiment::from_json(payload).map_err(input)?);
            }
            "task.changed" => {
                let id = pstr("id")?;
                let status =
                    crate::investigation::TaskStatus::parse(&pstr("status")?).map_err(input)?;
                if let Some(version) = investigation.approved_version
                    && let Some(revision) = investigation
                        .revisions
                        .iter_mut()
                        .find(|revision| revision.version == version)
                    && let Some(task) = revision
                        .proposal
                        .tasks
                        .iter_mut()
                        .find(|task| task.id == id)
                {
                    task.status = status;
                }
            }
            "change.applied" => {
                investigation
                    .changes
                    .push(FileChange::from_json(payload).map_err(input)?);
            }
            "finding.recorded" => {
                investigation
                    .findings
                    .push(Finding::from_json(payload).map_err(input)?);
            }
            "delivery" => {
                investigation.delivery = Some(Delivery::from_json(payload).map_err(input)?);
            }
            "artifact.registered" => {
                artifacts.push(ArtifactRef {
                    kind: pstr("kind")?,
                    path: pstr("path")?,
                    sha256: pstr("sha256")?,
                    bytes: pfields
                        .and_then(|fields| json::u64_value(fields, "bytes").ok())
                        .unwrap_or(0),
                });
            }
            "session.finished" => {
                return Err(input(
                    "the investigation is finished; its record is history, not work to resume"
                        .to_string(),
                ));
            }
            // session.paused, session.resumed and the approval bookkeeping kinds carry no state
            // the replay needs beyond what the events above already gave.
            _ => {}
        }
    }
    Ok(Replay {
        started,
        messages,
        invocations,
        investigation,
        approvals,
        artifacts,
        pending,
        requested,
        failed_signatures,
        turns,
        calls,
        phase,
    })
}

/// Answer the trailing assistant turn's unanswered calls. A completed call is answered from the
/// ledger, a started-but-unfinished one is reported interrupted, and a never-started one is
/// queued to run once. Returns the queue and the answers that must be journalled as replays.
fn answer_trailing_calls(
    messages: &mut Vec<Message>,
    invocations: &InvocationTable,
    requested: &BTreeMap<String, ToolCall>,
) -> (Vec<ToolCall>, Vec<(String, String)>) {
    let mut resume_calls: Vec<ToolCall> = Vec::new();
    let mut synthesized: Vec<(String, String)> = Vec::new();
    let Some(last) = messages
        .iter()
        .rposition(|message| message.role == Role::Assistant)
    else {
        return (resume_calls, synthesized);
    };
    let trailing = messages[last].tool_calls.clone();
    for call in trailing {
        let answered = messages[last + 1..].iter().any(|message| {
            message.role == Role::Tool && message.tool_call_id.as_deref() == Some(call.id.as_str())
        });
        if answered {
            continue;
        }
        if let Some(result) = invocations.recorded(&call.id) {
            let text = match result {
                Ok(output) => output.text.clone(),
                Err(failure) => failure.model_text(),
            };
            messages.push(Message::tool(&call.id, text.clone()));
            synthesized.push((call.id.clone(), text));
        } else if invocations.was_started(&call.id) {
            let text = INTERRUPTED_TEXT.to_string();
            messages.push(Message::tool(&call.id, text.clone()));
            synthesized.push((call.id.clone(), text));
        } else {
            // Requested or not, it never started: running it now is not a re-execution.
            let call = requested.get(&call.id).cloned().unwrap_or(call);
            resume_calls.push(call);
        }
    }
    (resume_calls, synthesized)
}

/// Resume an investigation from its record. See the module docs for the rules.
pub fn resume_session(
    config: ResumeConfig,
    controller: Box<dyn Controller>,
    registry: Registry,
) -> Result<Engine, EngineError> {
    let root = &config.record;
    if root.join("manifest.json").is_file() {
        return Err(input(
            "the investigation is finished; its record is history, not work to resume".to_string(),
        ));
    }
    let journal_path = root.join("journal.jsonl");
    let verified = crate::journal::verify_journal(&journal_path).map_err(EngineError::Record)?;
    let text = std::fs::read_to_string(&journal_path).map_err(|source| {
        EngineError::Record(crate::journal::RecordError::Io {
            path: journal_path.clone(),
            action: "read the journal",
            source,
        })
    })?;

    let mut replay = replay_events(&text)?;
    let started = replay
        .started
        .ok_or_else(|| input("the record has no session.started event".to_string()))?;
    let mut investigation = replay.investigation;
    investigation.objective.clone_from(&started.objective);

    // The facts the first scan recorded, re-presented exactly as they were. Evidence ids count
    // per kind, so re-adding the facts here keeps the invocation ids the replay assigned.
    let inspection_text = std::fs::read_to_string(root.join("inspection.json"))
        .map_err(|_| input("the record has no readable inspection.json".to_string()))?;
    let inspection = Inspection::from_json(
        &Json::parse(&inspection_text)
            .map_err(|error| input(format!("inspection.json: {error}")))?,
    )
    .map_err(input)?;
    for fact in &inspection.facts {
        investigation.add_evidence(
            "fact",
            format!("{} {}: {}", fact.id, fact.path, fact.detail),
        );
    }
    let copy_text = std::fs::read_to_string(root.join("sandbox.json"))
        .map_err(|_| input("the record has no readable sandbox.json".to_string()))?;
    let copy = parse_copy_report(
        &Json::parse(&copy_text).map_err(|error| input(format!("sandbox.json: {error}")))?,
    )?;

    let mut messages = opening_messages(&started, &inspection);
    messages.append(&mut replay.messages);
    let (resume_calls, synthesized) =
        answer_trailing_calls(&mut messages, &replay.invocations, &replay.requested);

    let record = RecordDir::open(root).map_err(EngineError::Record)?;
    let sandbox = Sandbox::open(record.workspace())
        .map_err(|error| EngineError::Sandbox(error.to_string()))?;
    let provider_name = config.provider.describe();
    let scope = investigation.scope.clone();
    let engine_config = EngineConfig {
        source: PathBuf::from(&started.source),
        objective: started.objective.clone(),
        provider: config.provider,
        allow_exec: started.allow_exec.clone(),
        allow_write: started.allow_write,
        max_turns: usize::try_from(started.max_turns).unwrap_or(usize::MAX),
        max_tool_calls: usize::try_from(started.max_tool_calls).unwrap_or(usize::MAX),
        total_timeout_ms: started.timeout_ms,
        workdir: root
            .parent()
            .map_or_else(|| PathBuf::from("."), PathBuf::from),
        instrument: config.instrument,
    };
    let deadline = Instant::now() + Duration::from_millis(started.timeout_ms.max(1));
    let mut engine = Engine {
        config: engine_config,
        record,
        sandbox,
        copy,
        registry,
        controller,
        messages,
        invocations: replay.invocations,
        investigation,
        scope,
        approvals: replay.approvals,
        artifacts: replay.artifacts,
        pending: replay.pending,
        resume_calls,
        failed_signatures: replay.failed_signatures,
        turns: replay.turns,
        calls: replay.calls,
        phase: replay.phase,
        status: SessionStatus::Active,
        failure: None,
        deadline,
        cancel: Arc::new(AtomicBool::new(false)),
        started: started.opened,
        provider_name,
    };
    announce_resume(&mut engine, verified.events, synthesized)?;
    engine.checkpoint()?;
    Ok(engine)
}

/// The system and objective messages, rebuilt from what the record says the session was opened
/// with -- not from a fresh scan of a source that may have changed since.
fn opening_messages(started: &Started, inspection: &Inspection) -> Vec<Message> {
    vec![
        Message::system(crate::prompt::system_v2(
            &started.allow_exec,
            started.allow_write,
        )),
        Message::user(crate::prompt::objective_v2(
            &started.objective,
            &PathBuf::from(&started.source),
            inspection,
            !started.allow_exec.is_empty(),
        )),
    ]
}

/// Record the resume itself, and the answers synthesized for trailing calls, so the next resume
/// reconstructs exactly this conversation.
fn announce_resume(
    engine: &mut Engine,
    events: u64,
    synthesized: Vec<(String, String)>,
) -> Result<(), EngineError> {
    let interrupted = engine.invocations.interrupted();
    engine.journal(
        "session.resumed",
        Json::object(vec![
            ("events", Json::count(events)),
            (
                "interrupted",
                Json::Arr(interrupted.iter().map(Json::text).collect()),
            ),
        ]),
    )?;
    for (call, text) in synthesized {
        engine.journal(
            "tool.replayed",
            Json::object(vec![
                ("call", Json::text(&call)),
                ("text", Json::text(&text)),
            ]),
        )?;
    }
    Ok(())
}
