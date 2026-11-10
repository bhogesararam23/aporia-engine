//! The iterative investigation loop.
//!
//! One stateful loop: ask the model, validate every call it asked for, decide what is permitted,
//! run what is allowed, return every result with its call id, and repeat until the model
//! finishes, a budget runs out, the user cancels, or a decision is needed. The model never runs
//! a command by writing one: the registry validates and dispatches, and a call outside the
//! approved scope becomes a question for the person rather than a silent expansion.
//!
//! The engine is free of any terminal rendering. It reports progress as events and pauses when
//! it needs a decision, so the same loop serves the batch command, the interactive workbench and
//! the tests.

use crate::events::Event;
use crate::investigation::Investigation;
use crate::journal::{Journal, RecordError, format_utc, now_seconds};
use crate::provider::{Message, ModelProvider, ModelResponse, ToolCall, ToolSchema};
use crate::record::RecordDir;
use crate::scope::{ApprovalKind, ApprovalRequest, Decision, Risk, Scope, ScopeLimits};
use crate::session::{Phase, SessionStatus};
use crate::tools::{
    ArtifactRef, Begin, InvocationTable, Registry, Session, ToolContext, ToolResult,
};
use aporia_sandbox::{CopyReport, Sandbox};
use aporia_store::Json;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const DETAIL_LIMIT: usize = 4096;

/// Why an investigation could not be started or recorded.
#[derive(Debug)]
pub enum EngineError {
    Input(String),
    Sandbox(String),
    Record(RecordError),
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Input(reason) => write!(f, "{reason}"),
            Self::Sandbox(reason) => write!(f, "the workspace could not be created: {reason}"),
            Self::Record(error) => {
                write!(f, "the investigation record could not be written: {error}")
            }
        }
    }
}

impl std::error::Error for EngineError {}

impl From<RecordError> for EngineError {
    fn from(value: RecordError) -> Self {
        Self::Record(value)
    }
}

/// How a person answers when no host asks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Approve {
    /// The flag is the approval: everything inside the ceilings is allowed.
    Flag,
    /// Refuse everything.
    Never,
}

/// What the engine needs from its host.
pub trait Controller {
    /// Decide an approval request. `None` means the host will ask the person later, and the
    /// engine pauses.
    fn decide(&mut self, request: &ApprovalRequest) -> Option<Decision>;

    /// Progress and outcome events, for a renderer that wants them.
    fn event(&mut self, _event: &EngineEvent) {}
}

/// Something the engine reports while it works.
#[derive(Clone, Debug, PartialEq)]
pub enum EngineEvent {
    Started {
        objective: String,
        source: String,
    },
    Turn {
        turn: u64,
        tools: Vec<String>,
    },
    Tool {
        invocation: String,
        name: String,
        ok: bool,
        detail: String,
    },
    ApprovalRequested {
        kind: String,
        title: String,
    },
    ApprovalDecided {
        kind: String,
        decision: String,
    },
    PlanProposed {
        version: u64,
    },
    Experiment {
        id: String,
        status: String,
    },
    Finished {
        outcome: String,
    },
}

/// Why the engine stopped asking the model.
#[derive(Clone, Debug, PartialEq)]
pub enum Wait {
    /// A decision is needed before anything else may happen.
    Approval(ApprovalRequest),
    /// The engine is finished; the outcome says how.
    Done(Outcome),
}

/// One step of the loop.
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    /// A model turn happened; more may follow.
    Turn,
    /// A decision is needed before anything else may happen.
    Approval(ApprovalRequest),
    /// The investigation ended.
    Finished(Outcome),
}

/// The end state of an investigation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Completed,
    NoPlan,
    Refused,
    Failed,
    BudgetExhausted,
    Cancelled,
}

impl Outcome {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::NoPlan => "no_plan",
            Self::Refused => "refused",
            Self::Failed => "failed",
            Self::BudgetExhausted => "budget_exhausted",
            Self::Cancelled => "cancelled",
        }
    }

    #[must_use]
    pub fn is_success(self) -> bool {
        matches!(self, Self::Completed | Self::NoPlan)
    }
}

/// Everything one investigation needs.
pub struct EngineConfig {
    pub source: PathBuf,
    pub objective: String,
    pub provider: Box<dyn ModelProvider>,
    /// The ceiling: the programs a plan may ask for.
    pub allow_exec: Vec<String>,
    /// The ceiling: whether a plan may ask to edit the workspace.
    pub allow_write: bool,
    pub max_turns: usize,
    pub max_tool_calls: usize,
    pub total_timeout_ms: u64,
    pub workdir: PathBuf,
    pub instrument: PathBuf,
}

impl std::fmt::Debug for EngineConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineConfig")
            .field("source", &self.source)
            .field("objective", &self.objective)
            .field("provider", &self.provider.describe())
            .field("allow_exec", &self.allow_exec)
            .field("allow_write", &self.allow_write)
            .field("max_turns", &self.max_turns)
            .field("max_tool_calls", &self.max_tool_calls)
            .field("total_timeout_ms", &self.total_timeout_ms)
            .field("workdir", &self.workdir)
            .field("instrument", &self.instrument)
            .finish()
    }
}

/// The controller that approves everything the ceilings allow.
#[derive(Debug)]
pub struct AutoApprove;

impl Controller for AutoApprove {
    fn decide(&mut self, _request: &ApprovalRequest) -> Option<Decision> {
        Some(Decision::Approve)
    }
}

/// The controller that refuses everything.
#[derive(Debug)]
pub struct DenyAll;

impl Controller for DenyAll {
    fn decide(&mut self, _request: &ApprovalRequest) -> Option<Decision> {
        Some(Decision::Refuse)
    }
}

/// The controller that answers from a prepared list, for tests.
#[derive(Debug)]
pub struct ScriptedDecisions {
    answers: std::collections::VecDeque<Decision>,
}

impl ScriptedDecisions {
    #[must_use]
    pub fn new(answers: Vec<Decision>) -> Self {
        Self {
            answers: answers.into(),
        }
    }
}

impl Controller for ScriptedDecisions {
    fn decide(&mut self, _request: &ApprovalRequest) -> Option<Decision> {
        self.answers.pop_front()
    }
}

/// The engine.
pub struct Engine {
    config: EngineConfig,
    record: RecordDir,
    sandbox: Sandbox,
    copy: CopyReport,
    registry: Registry,
    controller: Box<dyn Controller>,
    messages: Vec<Message>,
    invocations: InvocationTable,
    investigation: Investigation,
    scope: Option<Scope>,
    approvals: Vec<(ApprovalKind, Decision)>,
    artifacts: Vec<ArtifactRef>,
    pending: Option<ApprovalRequest>,
    pending_call: Option<ToolCall>,
    turns: usize,
    calls: usize,
    phase: Phase,
    status: SessionStatus,
    failure: Option<String>,
    deadline: Instant,
    cancel: Arc<AtomicBool>,
    started: String,
    provider_name: String,
    queued_messages: Vec<String>,
}

// The engine's own state is in the investigation record; the debug form is for a person reading a
// log line, so it names what matters and leaves the rest out.
#[allow(clippy::missing_fields_in_debug)]
impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Engine")
            .field("objective", &self.investigation.objective)
            .field("status", &self.status.as_str())
            .field("phase", &self.phase.as_str())
            .field("turns", &self.turns)
            .field("calls", &self.calls)
            .field("scope", &self.scope)
            .field("pending", &self.pending.is_some())
            .field("findings", &self.investigation.findings.len())
            .field("experiments", &self.investigation.experiments.len())
            .finish()
    }
}

impl Engine {
    /// Start an investigation: inspect, copy the project, open the record, say hello to the model.
    pub fn start(
        config: EngineConfig,
        controller: Box<dyn Controller>,
        registry: Registry,
    ) -> Result<Self, EngineError> {
        if config.objective.trim().is_empty() {
            return Err(EngineError::Input("the objective is empty".to_string()));
        }
        let inspection = crate::inspect::inspect(&config.source)
            .map_err(|error| EngineError::Input(error.to_string()))?;
        let started = format_utc(now_seconds());
        let provider_name = config.provider.describe();
        let mut record = RecordDir::create(&config.workdir).map_err(EngineError::Record)?;
        let (sandbox, copy) = Sandbox::create(&config.source, record.workspace())
            .map_err(|error| EngineError::Sandbox(error.to_string()))?;
        record
            .write_json("sandbox.json", &copy_json(&copy))
            .map_err(EngineError::Record)?;
        record
            .write_json("inspection.json", &inspection.to_json())
            .map_err(EngineError::Record)?;

        let mut investigation = Investigation {
            objective: config.objective.clone(),
            ..Investigation::default()
        };
        for fact in &inspection.facts {
            investigation.add_evidence(
                "fact",
                format!("{} {}: {}", fact.id, fact.path, fact.detail),
            );
        }

        let messages = vec![
            Message::system(crate::prompt::system_v2(
                &config.allow_exec,
                config.allow_write,
            )),
            Message::user(crate::prompt::objective_v2(
                &config.objective,
                &config.source,
                &inspection,
                !config.allow_exec.is_empty(),
            )),
        ];

        record
            .journal_mut()
            .append(
                "session.started",
                Json::object(vec![
                    ("objective", Json::text(&config.objective)),
                    ("source", Json::text(config.source.display().to_string())),
                    ("provider", Json::text(&provider_name)),
                    (
                        "allow_exec",
                        Json::Arr(config.allow_exec.iter().map(Json::text).collect()),
                    ),
                    ("allow_write", Json::Bool(config.allow_write)),
                    ("facts", Json::count(inspection.facts.len() as u64)),
                    ("started", Json::text(&started)),
                ]),
            )
            .map_err(EngineError::Record)?;

        let deadline = Instant::now() + Duration::from_millis(config.total_timeout_ms.max(1));
        Ok(Self {
            config,
            record,
            sandbox,
            copy,
            registry,
            controller,
            messages,
            invocations: InvocationTable::new(),
            investigation,
            scope: None,
            approvals: Vec::new(),
            artifacts: Vec::new(),
            pending: None,
            pending_call: None,
            turns: 0,
            calls: 0,
            phase: Phase::Inspecting,
            status: SessionStatus::Active,
            failure: None,
            deadline,
            cancel: Arc::new(AtomicBool::new(false)),
            started,
            provider_name,
            queued_messages: Vec::new(),
        })
    }

    /// Queue a message from the user for the next turn.
    pub fn say(&mut self, text: impl Into<String>) {
        self.queued_messages.push(text.into());
    }

    /// Ask the investigation to stop at the next opportunity.
    pub fn cancel(&mut self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    /// Whether the investigation is still open.
    #[must_use]
    pub fn status(&self) -> SessionStatus {
        self.status
    }

    /// The current phase.
    #[must_use]
    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// The state of the investigation, for a renderer.
    #[must_use]
    pub fn investigation(&self) -> &Investigation {
        &self.investigation
    }

    /// The approved scope, if any.
    #[must_use]
    pub fn scope(&self) -> Option<&Scope> {
        self.scope.as_ref()
    }

    /// The tools this session currently offers the model.
    #[must_use]
    pub fn tool_names(&self) -> Vec<String> {
        self.allowed_tools()
            .into_iter()
            .map(|tool| tool.name)
            .collect()
    }

    /// Run until the engine needs a decision, needs input, or ends.
    pub fn resume(&mut self) -> Result<Wait, EngineError> {
        loop {
            match self.step()? {
                Step::Turn => {}
                Step::Approval(request) => return Ok(Wait::Approval(request)),
                Step::Finished(outcome) => return Ok(Wait::Done(outcome)),
            }
        }
    }

    /// One step of the loop: a model turn, a decision to make, or the end.
    ///
    /// The interactive workbench drives this step by step, so the person can speak between turns
    /// and answer approvals as they arrive; a batch run simply loops over it.
    pub fn step(&mut self) -> Result<Step, EngineError> {
        if self.cancel.load(Ordering::SeqCst) {
            return Ok(Step::Finished(self.finish(SessionStatus::Cancelled, None)));
        }
        if !self.budget_available() {
            // A refusal or a failure already decided the outcome; the budget check only
            // decides the outcome when nothing else has.
            let status = match self.status {
                SessionStatus::Active => SessionStatus::BudgetExhausted,
                other => other,
            };
            let reason = self
                .failure
                .clone()
                .or_else(|| Some("a budget ran out before the investigation finished".to_string()));
            return Ok(Step::Finished(self.finish(status, reason)));
        }
        if let Some(request) = self.pending.clone() {
            match self.controller.decide(&request) {
                Some(decision) => {
                    self.decide(decision)?;
                    if self.status != SessionStatus::Active {
                        // A refusal ends the investigation; nothing else runs.
                        let status = self.status;
                        let reason = self.failure.clone();
                        return Ok(Step::Finished(self.finish(status, reason)));
                    }
                }
                None => return Ok(Step::Approval(request)),
            }
        }
        if !self.queued_messages.is_empty() {
            let message = self.queued_messages.remove(0);
            self.messages.push(Message::user(message));
            self.journal(
                "user.message",
                Json::object(vec![("text", Json::text("a message from the user"))]),
            )?;
        }
        let Some(response) = self.ask()? else {
            return Ok(Step::Finished(self.finish(
                SessionStatus::Failed,
                Some("the provider did not answer".to_string()),
            )));
        };
        let calls = response.tool_calls.clone();
        self.messages.push(Message::assistant(
            response.content.unwrap_or_default(),
            calls.clone(),
        ));
        if calls.is_empty() {
            self.messages.push(Message::user(
                "Your message contained no tool call. Use the tools you were given, or finish.",
            ));
            return Ok(Step::Turn);
        }
        for call in calls {
            match self.run_call(&call)? {
                CallOutcome::Continue => {}
                CallOutcome::Stop => {
                    return Ok(Step::Finished(self.finish(SessionStatus::Completed, None)));
                }
                CallOutcome::NeedsApproval => {
                    return Ok(Step::Approval(
                        self.pending.clone().expect("a pending approval"),
                    ));
                }
            }
        }
        Ok(Step::Turn)
    }

    /// Answer the pending approval and continue.
    pub fn decide(&mut self, decision: Decision) -> Result<(), EngineError> {
        let Some(request) = self.pending.take() else {
            return Ok(());
        };
        self.pending_call = None;
        self.journal(
            "approval.decided",
            Json::object(vec![
                ("kind", Json::text(request.kind.as_str())),
                ("decision", Json::text(decision.as_str())),
            ]),
        )?;
        self.controller.event(&EngineEvent::ApprovalDecided {
            kind: request.kind.as_str().to_string(),
            decision: decision.as_str().to_string(),
        });
        self.approvals.push((request.kind, decision));
        match request.kind {
            ApprovalKind::Plan | ApprovalKind::ScopeExpansion => {
                if decision == Decision::Approve {
                    self.apply_approval(&request)?;
                } else {
                    self.status = SessionStatus::Refused;
                    self.failure = Some("the plan was refused".to_string());
                }
            }
            ApprovalKind::ChangesetExport => {}
        }
        Ok(())
    }

    /// The pending approval, if the engine is waiting for one.
    #[must_use]
    pub fn pending_approval(&self) -> Option<&ApprovalRequest> {
        self.pending.as_ref()
    }

    /// Write the final report and close the record.
    pub fn finish(&mut self, status: SessionStatus, failure: Option<String>) -> Outcome {
        let outcome = match status {
            SessionStatus::Completed => Outcome::Completed,
            SessionStatus::NoPlan => Outcome::NoPlan,
            SessionStatus::Refused => Outcome::Refused,
            SessionStatus::Cancelled => Outcome::Cancelled,
            SessionStatus::BudgetExhausted => Outcome::BudgetExhausted,
            SessionStatus::Failed | SessionStatus::Paused | SessionStatus::Active => {
                Outcome::Failed
            }
        };
        self.status = status;
        self.phase = Phase::Finished;
        if failure.is_some() {
            self.failure = failure;
        }
        let finished = format_utc(now_seconds());
        let event = Event::SessionFinished {
            status: status.as_str().to_string(),
            failure: self.failure.clone(),
            turns: self.turns as u64,
            calls: self.calls as u64,
        };
        let _ = self.journal(event.kind(), event.payload());
        let summary = crate::report::render_v2_report(&self.summary());
        let _ = self.record.write_text("report.md", &summary);
        let _ = self
            .record
            .write_json("investigation.json", &self.summary_json());
        let manifest = self.manifest_json(&finished);
        let _ = self.record.finish(&manifest);
        self.controller.event(&EngineEvent::Finished {
            outcome: outcome.as_str().to_string(),
        });
        outcome
    }

    /// A machine-readable summary of the investigation.
    #[must_use]
    pub fn summary_json(&self) -> Json {
        Json::object(vec![
            ("schema", Json::count(2)),
            ("objective", Json::text(&self.investigation.objective)),
            ("status", Json::text(self.status.as_str())),
            ("phase", Json::text(self.phase.as_str())),
            ("provider", Json::text(&self.provider_name)),
            ("started", Json::text(&self.started)),
            ("finished", Json::text(format_utc(now_seconds()))),
            ("turns", Json::count(self.turns as u64)),
            ("calls", Json::count(self.calls as u64)),
            ("plan", self.investigation.to_json()),
            (
                "artifacts",
                Json::Arr(self.artifacts.iter().map(ArtifactRef::to_json).collect()),
            ),
            (
                "approvals",
                Json::Arr(
                    self.approvals
                        .iter()
                        .map(|(kind, decision)| {
                            Json::object(vec![
                                ("kind", Json::text(kind.as_str())),
                                ("decision", Json::text(decision.as_str())),
                            ])
                        })
                        .collect(),
                ),
            ),
            (
                "failure",
                self.failure.as_ref().map_or(Json::Null, Json::text),
            ),
            ("workspace", Json::text("workspace")),
        ])
    }

    fn summary(&self) -> crate::report::V2Summary<'_> {
        let experiments = self.investigation.ranked_experiments();
        crate::report::V2Summary {
            objective: &self.investigation.objective,
            status: self.status.as_str(),
            provider: &self.provider_name,
            turns: self.turns,
            calls: self.calls,
            findings: &self.investigation.findings,
            experiments,
            changes: &[],
            failure: self.failure.as_deref(),
            root: self.record.root(),
        }
    }

    fn manifest_json(&mut self, finished: &str) -> Json {
        Json::object(vec![
            ("schema", Json::count(2)),
            (
                "tool",
                Json::text(format!("aporia agent {}", env!("CARGO_PKG_VERSION"))),
            ),
            ("started", Json::text(&self.started)),
            ("finished", Json::text(finished)),
            ("outcome", Json::text(self.status.as_str())),
            ("objective", Json::text(&self.investigation.objective)),
            ("provider", Json::text(&self.provider_name)),
            (
                "budgets",
                Json::object(vec![
                    (
                        "allow_exec",
                        Json::Arr(self.config.allow_exec.iter().map(Json::text).collect()),
                    ),
                    ("allow_write", Json::Bool(self.config.allow_write)),
                    ("max_turns", Json::count(self.config.max_turns as u64)),
                    (
                        "max_tool_calls",
                        Json::count(self.config.max_tool_calls as u64),
                    ),
                    (
                        "total_timeout_ms",
                        Json::count(self.config.total_timeout_ms),
                    ),
                ]),
            ),
            (
                "counts",
                Json::object(vec![
                    ("workspace_files", Json::count(self.copy.files.len() as u64)),
                    ("turns", Json::count(self.turns as u64)),
                    ("tool_calls", Json::count(self.calls as u64)),
                    (
                        "findings",
                        Json::count(self.investigation.findings.len() as u64),
                    ),
                    (
                        "experiments",
                        Json::count(self.investigation.experiments.len() as u64),
                    ),
                    ("artifacts", Json::count(self.artifacts.len() as u64)),
                ]),
            ),
            (
                "journal",
                Json::object(vec![
                    ("events", Json::count(self.record.journal_mut().events())),
                    ("head", Json::text(self.record.journal_mut().head())),
                ]),
            ),
            (
                "failure",
                self.failure.as_ref().map_or(Json::Null, Json::text),
            ),
            ("workspace", Json::text("workspace")),
        ])
    }

    /// The ceiling this session was configured with.
    fn limits(&self) -> ScopeLimits {
        ScopeLimits {
            allow_exec: self.config.allow_exec.clone(),
            allow_write: self.config.allow_write,
            max_turns: self.config.max_turns as u64,
            max_tool_calls: self.config.max_tool_calls as u64,
            timeout_ms: self.config.total_timeout_ms,
        }
    }

    /// The schemas the model may see on this turn.
    fn allowed_tools(&self) -> Vec<ToolSchema> {
        self.registry
            .schemas()
            .into_iter()
            .filter(|schema| self.tool_permitted(&schema.name))
            .collect()
    }

    fn tool_permitted(&self, name: &str) -> bool {
        match self.registry.get(name).map(|tool| tool.meta().risk) {
            Some(Risk::ReadOnly) => true,
            Some(Risk::WorkspaceWrite) => {
                self.scope.as_ref().is_some_and(|scope| scope.allow_write)
            }
            Some(Risk::Execute) => self
                .scope
                .as_ref()
                .is_some_and(|scope| !scope.allow_exec.is_empty()),
            None => false,
        }
    }

    fn ask(&mut self) -> Result<Option<ModelResponse>, EngineError> {
        self.turns += 1;
        let tools = self.allowed_tools();
        let names: Vec<String> = tools.iter().map(|tool| tool.name.clone()).collect();
        let event = Event::ModelRequest {
            turn: self.turns as u64,
            messages: self.messages.len() as u64,
            tools: names.clone(),
        };
        self.journal(event.kind(), event.payload())?;
        self.controller.event(&EngineEvent::Turn {
            turn: self.turns as u64,
            tools: names,
        });
        let request = crate::provider::ModelRequest {
            messages: self.messages.clone(),
            tools,
        };
        match self.config.provider.complete(&request) {
            Ok(response) => {
                let event = Event::ModelResponse {
                    content: response.content.clone(),
                    calls: calls_json(&response.tool_calls),
                };
                self.journal(event.kind(), event.payload())?;
                Ok(Some(response))
            }
            Err(error) => {
                self.failure = Some(error.to_string());
                Ok(None)
            }
        }
    }

    /// Run one model call: idempotency first, then execution.
    fn run_call(&mut self, call: &ToolCall) -> Result<CallOutcome, EngineError> {
        match self.invocations.begin(&call.id) {
            Begin::Completed(result) => {
                let text = match &result {
                    Ok(output) => output.text.clone(),
                    Err(failure) => failure.model_text(),
                };
                self.messages.push(Message::tool(&call.id, text));
                return Ok(CallOutcome::Continue);
            }
            Begin::Interrupted => {
                let text = "error[interrupted]: this call was started before an interruption and \
                            never completed; it was not run again"
                    .to_string();
                self.messages.push(Message::tool(&call.id, text));
                return Ok(CallOutcome::Continue);
            }
            Begin::Fresh => {}
        }
        if self.calls >= self.config.max_tool_calls {
            self.status = SessionStatus::BudgetExhausted;
            self.failure = Some("the tool-call budget ran out".to_string());
            return Ok(CallOutcome::Stop);
        }
        self.calls += 1;
        self.execute_call(call)
    }

    /// Execute one call: journal it, dispatch it, record what happened.
    fn execute_call(&mut self, call: &ToolCall) -> Result<CallOutcome, EngineError> {
        let name = call.name.clone();
        self.journal(
            "tool.requested",
            Json::object(vec![
                ("call", Json::text(&call.id)),
                ("name", Json::text(&name)),
                ("arguments", Json::text(&call.arguments)),
            ]),
        )?;
        self.journal(
            "tool.started",
            Json::object(vec![
                ("call", Json::text(&call.id)),
                ("name", Json::text(&name)),
            ]),
        )?;

        let (outcome, delivered, requested) = self.dispatch(call);
        if let Some(request) = requested {
            self.journal(
                "approval.requested",
                Json::object(vec![
                    ("kind", Json::text(request.kind.as_str())),
                    ("title", Json::text(&request.title)),
                    ("detail", request.detail.clone()),
                ]),
            )?;
            self.controller.event(&EngineEvent::ApprovalRequested {
                kind: request.kind.as_str().to_string(),
                title: request.title.clone(),
            });
            self.pending = Some(request);
        }
        let (ok, text, artifacts, class) = match &outcome {
            Ok(output) => (
                true,
                output.text.clone(),
                output.artifacts.clone(),
                Option::None,
            ),
            Err(failure) => (
                false,
                failure.model_text(),
                Vec::new(),
                Some(failure.class.as_str().to_string()),
            ),
        };
        self.invocations.complete(&call.id, outcome.clone());
        let detail = match &outcome {
            Ok(output) => output.text.chars().take(400).collect::<String>(),
            Err(failure) => failure.message.chars().take(400).collect::<String>(),
        };
        let evidence_id = self
            .investigation
            .add_evidence("invocation", format!("{name}: {detail}"));
        self.journal(
            "tool.completed",
            Json::object(vec![
                ("invocation", Json::text(&evidence_id)),
                ("name", Json::text(&name)),
                ("ok", Json::Bool(ok)),
                ("class", class.map_or(Json::Null, Json::text)),
                ("text", Json::text(&text)),
            ]),
        )?;
        for artifact in &artifacts {
            self.artifacts.push(artifact.clone());
            self.journal(
                "artifact.registered",
                Json::object(vec![
                    ("kind", Json::text(&artifact.kind)),
                    ("path", Json::text(&artifact.path)),
                    ("sha256", Json::text(&artifact.sha256)),
                ]),
            )?;
        }
        self.controller.event(&EngineEvent::Tool {
            invocation: evidence_id.clone(),
            name: name.clone(),
            ok,
            detail: text.chars().take(200).collect(),
        });

        // A denial that names a permission gap becomes a question, not a dead end.
        if let Err(failure) = &outcome
            && failure.class == crate::tools::result::ErrorClass::Denied
            && self.pending.is_none()
            && let Some(request) = Self::approval_for(call, failure)
        {
            self.pending = Some(request);
            self.pending_call = Some(call.clone());
            return Ok(CallOutcome::NeedsApproval);
        }

        // The invocation id goes into the message the model reads, so a finding can cite the
        // call that produced the evidence.
        self.messages
            .push(Message::tool(&call.id, format!("[{evidence_id}] {text}")));
        if delivered {
            return Ok(CallOutcome::Stop);
        }
        Ok(CallOutcome::Continue)
    }

    /// The question a denial raises, when it is about permission.
    fn approval_for(
        call: &ToolCall,
        failure: &crate::tools::ToolFailure,
    ) -> Option<ApprovalRequest> {
        if call.name != "run_command" && call.name != "instrument_run" {
            return None;
        }
        if !failure.message.contains("approved scope") {
            return None;
        }
        let program = requested_program(call);
        Some(ApprovalRequest {
            kind: ApprovalKind::ScopeExpansion,
            title: format!("allow running `{program}` in this session?"),
            detail: Json::object(vec![
                ("program", Json::text(program)),
                ("reason", Json::text(&failure.message)),
            ]),
        })
    }

    /// Apply an approved plan or an approved expansion.
    fn apply_approval(&mut self, request: &ApprovalRequest) -> Result<(), EngineError> {
        if request.kind == ApprovalKind::ScopeExpansion {
            let program = crate::json::fields(&request.detail)
                .and_then(|fields| {
                    fields
                        .iter()
                        .find(|(name, _)| name == "program")
                        .and_then(|(_, value)| value.as_str())
                })
                .unwrap_or_default()
                .to_string();
            let permitted = self.config.allow_exec.contains(&program);
            if !permitted {
                self.journal(
                    "approval.outside_the_ceiling",
                    Json::object(vec![("program", Json::text(&program))]),
                )?;
                self.failure = Some(format!(
                    "`{program}` is outside this session's configured permissions; it cannot be approved here"
                ));
                return Ok(());
            }
            let mut scope = self.scope.clone().unwrap_or_else(|| Scope {
                allow_exec: Vec::new(),
                allow_write: false,
                max_turns: self.config.max_turns as u64,
                max_tool_calls: self.config.max_tool_calls as u64,
                timeout_ms: self.config.total_timeout_ms,
            });
            if !scope.permits_exec(&program) {
                scope.allow_exec.push(program.clone());
            }
            self.scope = Some(scope.clone());
            self.investigation.scope = Some(scope);
            self.journal(
                "scope.expanded",
                Json::object(vec![("program", Json::text(&program))]),
            )?;
            return Ok(());
        }
        let pending = self
            .investigation
            .revisions
            .iter()
            .find(|revision| revision.decision.is_none())
            .map(|revision| revision.version);
        if let Some(version) = pending {
            let scope = {
                let revision = self
                    .investigation
                    .revisions
                    .iter_mut()
                    .find(|revision| revision.version == version)
                    .expect("the revision is still there");
                revision.decision = Some(Decision::Approve);
                revision.scope = Some(revision.proposal.scope.as_scope());
                revision.scope.clone().expect("a scope was just set")
            };
            self.investigation.scope = Some(scope.clone());
            self.investigation.approved_version = Some(version);
            self.scope = Some(scope);
            self.phase = Phase::Working;
            self.journal(
                "plan.decided",
                Json::object(vec![
                    ("version", Json::count(version)),
                    ("decision", Json::text("approve")),
                ]),
            )?;
        }
        Ok(())
    }

    /// Dispatch one call with the session attached.
    fn dispatch(&mut self, call: &ToolCall) -> (ToolResult, bool, Option<ApprovalRequest>) {
        let scope = self.scope.clone();
        let limits = self.limits();
        let mut pending: Option<ApprovalRequest> = None;
        let mut delivered = false;
        let outcome = {
            let Engine {
                sandbox,
                config,
                investigation,
                record,
                cancel,
                registry,
                ..
            } = self;
            let mut session = EngineSession {
                investigation,
                journal: record.journal_mut(),
                limits,
                pending: &mut pending,
                delivered: &mut delivered,
            };
            let mut context = ToolContext::new(
                sandbox,
                &config.source,
                &config.instrument,
                scope.as_ref(),
                cancel,
            )
            .with_session(&mut session);
            crate::tools::dispatch(registry, &mut context, call)
        };
        (outcome, delivered, pending)
    }

    fn journal(&mut self, kind: &str, payload: Json) -> Result<(), EngineError> {
        self.record
            .journal_mut()
            .append(kind, payload)
            .map(|_| ())
            .map_err(EngineError::Record)
    }

    fn budget_available(&mut self) -> bool {
        if self.status != SessionStatus::Active {
            return false;
        }
        if self.turns >= self.config.max_turns {
            self.failure = Some("the model-turn budget ran out".to_string());
            return false;
        }
        if self.calls >= self.config.max_tool_calls {
            self.failure = Some("the tool-call budget ran out".to_string());
            return false;
        }
        if Instant::now() >= self.deadline {
            self.failure = Some("the wall-clock budget ran out".to_string());
            return false;
        }
        true
    }
}

/// The session the control tools act on.
struct EngineSession<'a> {
    investigation: &'a mut Investigation,
    journal: &'a mut Journal,
    limits: ScopeLimits,
    pending: &'a mut Option<ApprovalRequest>,
    delivered: &'a mut bool,
}

impl Session for EngineSession<'_> {
    fn investigation(&mut self) -> &mut Investigation {
        self.investigation
    }

    fn journal(&mut self, kind: &str, payload: Json) -> Result<(), String> {
        self.journal
            .append(kind, payload)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    fn request_approval(&mut self, request: ApprovalRequest) {
        *self.pending = Some(request);
    }

    fn limits(&self) -> ScopeLimits {
        self.limits.clone()
    }

    fn deliver(&mut self, delivery: crate::orchestrator::Delivery) -> bool {
        self.investigation.delivery = Some(delivery);
        *self.delivered = true;
        true
    }
}

/// What one call did to the loop.
enum CallOutcome {
    Continue,
    Stop,
    NeedsApproval,
}

/// The program a call asked for, if it named one.
fn requested_program(call: &ToolCall) -> String {
    let Ok(value) = Json::parse(&call.arguments) else {
        return String::new();
    };
    let Some(fields) = crate::json::fields(&value) else {
        return String::new();
    };
    match call.name.as_str() {
        "run_command" => crate::json::string_array(fields, "argv")
            .ok()
            .and_then(|argv| argv.first().cloned())
            .unwrap_or_default(),
        _ => String::new(),
    }
}

fn calls_json(calls: &[ToolCall]) -> Json {
    Json::Arr(
        calls
            .iter()
            .map(|call| {
                Json::object(vec![
                    ("id", Json::text(&call.id)),
                    ("name", Json::text(&call.name)),
                    ("arguments", Json::text(cap(&call.arguments, DETAIL_LIMIT))),
                ])
            })
            .collect(),
    )
}

fn cap(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_string();
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n[truncated]", &text[..end])
}

fn copy_json(copy: &CopyReport) -> Json {
    Json::object(vec![
        (
            "files",
            Json::Arr(
                copy.files
                    .iter()
                    .map(|file| {
                        Json::object(vec![
                            ("path", Json::text(&file.path)),
                            ("bytes", Json::count(file.bytes)),
                            ("sha256", Json::text(&file.sha256)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "ignored",
            Json::Arr(copy.ignored.iter().map(Json::text).collect()),
        ),
        ("total_bytes", Json::count(copy.total_bytes)),
    ])
}
