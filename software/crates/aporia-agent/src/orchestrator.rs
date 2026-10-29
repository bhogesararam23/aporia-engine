//! The investigation state machine.
//!
//! Inspect, plan, approve, execute, interpret, record. The model proposes and interprets; the host
//! decides what is permitted, runs the approved tasks, and writes down what actually happened. The
//! two loops are deliberately separate: before approval the model has no execution tools, and after
//! approval it cannot change the plan, only explain the recorded results.

use crate::execute::{TaskResult, execute};
use crate::inspect::{Inspection, inspect};
use crate::journal::{RecordError, format_utc, now_seconds};
use crate::json;
use crate::plan::{Finding, Plan, ValidatedPlan, parse_finding, parse_plan, validate_plan};
use crate::prompt;
use crate::provider::{Message, ModelProvider, ModelRequest, ModelResponse, ToolCall, ToolSchema};
use crate::record::RecordDir;
use crate::report;
use aporia_sandbox::{CopyReport, Sandbox};
use aporia_store::Json;
use aporia_store::digest::sha256_hex;
use std::collections::BTreeSet;
use std::io::{Read as _, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

const READ_FILE_LIMIT: usize = 32 * 1024;
const TOOL_DETAIL_LIMIT: usize = 4096;
const MODEL_TEXT_LIMIT: usize = 16 * 1024;

/// How the plan decision is made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Approve {
    /// `--approve`: the flag is the approval.
    Flag,
    /// Ask on standard input.
    Interactive,
    /// No approval is possible. Tests use this; the CLI uses it when stdin is not a terminal and
    /// `--approve` was not passed.
    Never,
}

/// The end state of an investigation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Completed,
    /// The model decided, before planning, that no investigation was warranted.
    NoPlan,
    Refused,
    Failed,
    BudgetExhausted,
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
        }
    }

    #[must_use]
    pub fn is_success(self) -> bool {
        matches!(self, Self::Completed | Self::NoPlan)
    }
}

/// What `finish` delivered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Delivery {
    pub summary: String,
    pub next_steps: Vec<String>,
    pub unresolved: Vec<String>,
}

impl Delivery {
    #[must_use]
    pub fn to_json(&self) -> Json {
        Json::object(vec![
            ("summary", Json::text(&self.summary)),
            (
                "next_steps",
                Json::Arr(self.next_steps.iter().map(Json::text).collect()),
            ),
            (
                "unresolved",
                Json::Arr(self.unresolved.iter().map(Json::text).collect()),
            ),
        ])
    }
}

/// A follow-up the model suggested but did not run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suggestion {
    pub question: String,
    pub why: String,
    pub method: String,
    pub estimated_cost: String,
}

impl Suggestion {
    #[must_use]
    pub fn to_json(&self) -> Json {
        Json::object(vec![
            ("question", Json::text(&self.question)),
            ("why", Json::text(&self.why)),
            ("method", Json::text(&self.method)),
            ("estimated_cost", Json::text(&self.estimated_cost)),
        ])
    }
}

/// Everything the caller needs to render or inspect a finished investigation.
#[derive(Clone, Debug)]
pub struct RunReport {
    pub id: String,
    pub root: PathBuf,
    pub outcome: Outcome,
    pub objective: String,
    pub source: PathBuf,
    pub provider: String,
    pub started: String,
    pub finished: String,
    pub inspection: Inspection,
    pub copy: CopyReport,
    pub plan: Option<Plan>,
    pub validated: Option<ValidatedPlan>,
    pub approved: Option<bool>,
    pub results: Vec<TaskResult>,
    pub findings: Vec<Finding>,
    pub suggestions: Vec<Suggestion>,
    pub delivery: Option<Delivery>,
    pub failure: Option<String>,
    pub turns: usize,
    pub calls: usize,
    pub journal_head: String,
    pub journal_events: u64,
    pub sandbox_sha256: String,
}

/// Why the investigation could not be started or recorded.
#[derive(Debug)]
pub enum AgentError {
    Input(String),
    Sandbox(String),
    Record(RecordError),
}

impl std::fmt::Display for AgentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Input(reason) => write!(f, "{reason}"),
            Self::Sandbox(reason) => write!(f, "the sandbox could not be created: {reason}"),
            Self::Record(error) => {
                write!(f, "the investigation record could not be written: {error}")
            }
        }
    }
}

impl std::error::Error for AgentError {}

impl From<RecordError> for AgentError {
    fn from(value: RecordError) -> Self {
        Self::Record(value)
    }
}

/// Everything a run needs. The provider is boxed so the orchestrator does not depend on one.
pub struct RunConfig {
    pub source: PathBuf,
    pub objective: String,
    pub provider: Box<dyn ModelProvider>,
    pub approve: Approve,
    pub allow_exec: Vec<String>,
    pub max_turns: usize,
    pub max_tool_calls: usize,
    pub total_timeout_ms: u64,
    pub workdir: PathBuf,
    pub instrument: PathBuf,
}

impl std::fmt::Debug for RunConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunConfig")
            .field("source", &self.source)
            .field("objective", &self.objective)
            .field("provider", &self.provider.describe())
            .field("approve", &self.approve)
            .field("allow_exec", &self.allow_exec)
            .field("max_turns", &self.max_turns)
            .field("max_tool_calls", &self.max_tool_calls)
            .field("total_timeout_ms", &self.total_timeout_ms)
            .field("workdir", &self.workdir)
            .field("instrument", &self.instrument)
            .finish()
    }
}

/// Run one investigation.
pub fn run(config: RunConfig, out: &mut dyn Write) -> Result<RunReport, AgentError> {
    let mut session = Session::start(config, out)?;
    session.planning()?;
    if session.outcome.is_none()
        && let Some(plan) = session.plan.clone()
        && session.approve(&plan)?
    {
        session.execution()?;
        if session.outcome.is_none() {
            session.interpretation()?;
        }
    }
    session.finish()
}

struct Session<'a> {
    config: RunConfig,
    record: RecordDir,
    inspection: Inspection,
    sandbox: Sandbox,
    copy: CopyReport,
    sandbox_sha256: String,
    messages: Vec<Message>,
    tools: Vec<ToolSchema>,
    out: &'a mut dyn Write,
    plan: Option<Plan>,
    validated: Option<ValidatedPlan>,
    approved: Option<bool>,
    results: Vec<TaskResult>,
    findings: Vec<Finding>,
    suggestions: Vec<Suggestion>,
    delivery: Option<Delivery>,
    outcome: Option<Outcome>,
    failure: Option<String>,
    turns: usize,
    calls: usize,
    invalid_plans: usize,
    deadline: Instant,
    started: String,
    known: BTreeSet<String>,
}

impl<'a> Session<'a> {
    fn start(config: RunConfig, out: &'a mut dyn Write) -> Result<Self, AgentError> {
        if config.objective.trim().is_empty() {
            return Err(AgentError::Input("the objective is empty".to_string()));
        }
        let inspection =
            inspect(&config.source).map_err(|error| AgentError::Input(error.to_string()))?;
        let started = format_utc(now_seconds());
        let provider = config.provider.describe();
        let mut record = begin_record(&config, &inspection, &started, &provider)?;
        let (sandbox, copy, sandbox_sha256) =
            create_sandbox(&config, &inspection, &started, &provider, &mut record)?;

        let messages = vec![
            Message::system(prompt::system(&config.allow_exec)),
            Message::user(prompt::initial_user(
                &config.objective,
                &config.source,
                &inspection,
            )),
        ];
        let tools = prompt::tools();
        let known = inspection
            .facts
            .iter()
            .map(|fact| fact.id.clone())
            .collect();
        let deadline = Instant::now() + Duration::from_millis(config.total_timeout_ms.max(1));
        Ok(Self {
            config,
            record,
            inspection,
            sandbox,
            copy,
            sandbox_sha256,
            messages,
            tools,
            out,
            plan: None,
            validated: None,
            approved: None,
            results: Vec::new(),
            findings: Vec::new(),
            suggestions: Vec::new(),
            delivery: None,
            outcome: None,
            failure: None,
            turns: 0,
            calls: 0,
            invalid_plans: 0,
            deadline,
            started,
            known,
        })
    }

    fn planning(&mut self) -> Result<(), AgentError> {
        loop {
            if !self.budget_available() {
                return Ok(());
            }
            let Some(response) = self.ask()? else {
                return Ok(());
            };
            let calls = response.tool_calls.clone();
            self.messages.push(Message::assistant(
                response.content.unwrap_or_default(),
                calls.clone(),
            ));
            if calls.is_empty() {
                self.messages.push(Message::user(
                    "Your message contained no tool call. Call read_file, propose_plan or finish.",
                ));
                continue;
            }
            for call in calls {
                if !self.count_call() {
                    return Ok(());
                }
                let stop = match call.name.as_str() {
                    "read_file" => {
                        self.read_file(&call)?;
                        false
                    }
                    "propose_plan" => self.propose_plan(&call)?,
                    "finish" => self.finish_delivery(&call, true)?,
                    other => {
                        self.tool_error(&call, &format!("unknown tool `{other}`"))?;
                        false
                    }
                };
                if stop {
                    return Ok(());
                }
            }
        }
    }

    fn propose_plan(&mut self, call: &ToolCall) -> Result<bool, AgentError> {
        let value = match Json::parse(&call.arguments) {
            Ok(value) => value,
            Err(error) => {
                self.tool_error(call, &format!("the arguments are not JSON: {error}"))?;
                self.plan_rejected();
                return Ok(self.outcome.is_some());
            }
        };
        let result = parse_plan(&value).and_then(|plan| {
            validate_plan(
                &plan,
                &self.inspection,
                &self.config.allow_exec,
                &self.sandbox,
            )
            .map(|validated| (plan, validated))
        });
        match result {
            Ok((plan, validated)) => {
                self.invalid_plans = 0;
                self.journal("plan.proposed", plan.to_json())?;
                self.plan = Some(plan);
                self.validated = Some(validated);
                self.tool_ok(
                    call,
                    "The plan was accepted and is being shown to the user for approval."
                        .to_string(),
                )?;
                Ok(true)
            }
            Err(reason) => {
                self.tool_error(call, &format!("the plan was rejected: {reason}"))?;
                self.plan_rejected();
                Ok(self.outcome.is_some())
            }
        }
    }

    fn plan_rejected(&mut self) {
        self.invalid_plans += 1;
        if self.invalid_plans >= 3 {
            self.fail(
                "three plans in a row were rejected; the model did not produce a valid plan"
                    .to_string(),
            );
        }
    }

    fn approve(&mut self, plan: &Plan) -> Result<bool, AgentError> {
        let _ = writeln!(self.out, "{}", report::render_plan(plan));
        let approved = match self.config.approve {
            Approve::Flag => true,
            Approve::Never => false,
            Approve::Interactive => {
                let _ = write!(self.out, "approve? [y/N] ");
                let _ = self.out.flush();
                let mut line = String::new();
                std::io::stdin().read_line(&mut line).is_ok()
                    && matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
            }
        };
        self.approved = Some(approved);
        self.journal(
            "plan.decided",
            Json::object(vec![("approved", Json::Bool(approved))]),
        )?;
        let _ = writeln!(
            self.out,
            "{}",
            if approved {
                "approved"
            } else {
                "refused; nothing will run"
            }
        );
        if !approved {
            self.outcome = Some(Outcome::Refused);
        }
        Ok(approved)
    }

    fn execution(&mut self) -> Result<(), AgentError> {
        let Some(validated) = self.validated.clone() else {
            return Ok(());
        };
        let total = validated.order.len();
        for (position, &index) in validated.order.iter().enumerate() {
            if !self.budget_available() || !self.count_call() {
                return Ok(());
            }
            let task = &validated.tasks[index];
            let id = format!("R{}", self.results.len() + 1);
            let skip_reason = self.dependency_skip(task);
            let result = if let Some(reason) = skip_reason {
                TaskResult::skipped(&id, task, &reason)
            } else {
                self.journal(
                    "task.started",
                    Json::object(vec![
                        ("result_id", Json::text(&id)),
                        ("task_id", Json::text(&task.task.id)),
                        ("tool", Json::text(task.task.tool.as_str())),
                    ]),
                )?;
                execute(
                    &id,
                    task,
                    &self.sandbox,
                    &self.config.instrument,
                    "workspace",
                )
            };
            self.journal("task.completed", result.to_json())?;
            let _ = writeln!(
                self.out,
                "[{}/{}] {} {}: {} in {} ms",
                position + 1,
                total,
                result.task_id,
                result.tool,
                result.status,
                result.duration_ms
            );
            self.known.insert(result.id.clone());
            self.results.push(result);
        }
        Ok(())
    }

    fn dependency_skip(&self, task: &crate::plan::ValidatedTask) -> Option<String> {
        for dependency in &task.task.depends_on {
            match self
                .results
                .iter()
                .find(|result| &result.task_id == dependency)
            {
                Some(result) if result.status == "completed" => {}
                Some(result) => {
                    return Some(format!(
                        "dependency `{dependency}` ended as {}",
                        result.status
                    ));
                }
                None => return Some(format!("dependency `{dependency}` has no recorded result")),
            }
        }
        None
    }

    fn interpretation(&mut self) -> Result<(), AgentError> {
        self.messages
            .push(Message::user(prompt::results(&self.results)));
        self.journal(
            "interpretation.started",
            Json::object(vec![("results", Json::count(self.results.len() as u64))]),
        )?;
        loop {
            if !self.budget_available() {
                return Ok(());
            }
            let Some(response) = self.ask()? else {
                return Ok(());
            };
            let calls = response.tool_calls.clone();
            self.messages.push(Message::assistant(
                response.content.unwrap_or_default(),
                calls.clone(),
            ));
            if calls.is_empty() {
                self.messages.push(Message::user(
                    "Your message contained no tool call. Read a recorded result, record_finding, \
                     suggest_experiment or finish.",
                ));
                continue;
            }
            for call in calls {
                if !self.count_call() {
                    return Ok(());
                }
                let stop = match call.name.as_str() {
                    "read_file" => {
                        self.read_file(&call)?;
                        false
                    }
                    "record_finding" => {
                        self.record_finding(&call)?;
                        false
                    }
                    "suggest_experiment" => {
                        self.suggest_experiment(&call)?;
                        false
                    }
                    "finish" => self.finish_delivery(&call, false)?,
                    other => {
                        self.tool_error(&call, &format!("unknown tool `{other}`"))?;
                        false
                    }
                };
                if stop {
                    return Ok(());
                }
            }
        }
    }

    fn record_finding(&mut self, call: &ToolCall) -> Result<(), AgentError> {
        let value = match Json::parse(&call.arguments) {
            Ok(value) => value,
            Err(error) => {
                return self.tool_error(call, &format!("the arguments are not JSON: {error}"));
            }
        };
        match parse_finding(&value, &self.known) {
            Ok(finding) => {
                let number = self.findings.len() + 1;
                self.journal("finding.recorded", finding.to_json())?;
                self.findings.push(finding);
                self.tool_ok(
                    call,
                    format!("Recorded finding {number}. Continue or call finish."),
                )
            }
            Err(reason) => self.tool_error(call, &format!("the finding was rejected: {reason}")),
        }
    }

    fn suggest_experiment(&mut self, call: &ToolCall) -> Result<(), AgentError> {
        let value = match Json::parse(&call.arguments) {
            Ok(value) => value,
            Err(error) => {
                return self.tool_error(call, &format!("the arguments are not JSON: {error}"));
            }
        };
        match parse_suggestion(&value) {
            Ok(suggestion) => {
                self.journal("experiment.suggested", suggestion.to_json())?;
                self.suggestions.push(suggestion);
                self.tool_ok(
                    call,
                    "The suggestion was recorded. It was not run; the report lists it as a next step."
                        .to_string(),
                )
            }
            Err(reason) => self.tool_error(call, &format!("the suggestion was rejected: {reason}")),
        }
    }

    fn finish_delivery(&mut self, call: &ToolCall, planning: bool) -> Result<bool, AgentError> {
        let value = match Json::parse(&call.arguments) {
            Ok(value) => value,
            Err(error) => {
                self.tool_error(call, &format!("the arguments are not JSON: {error}"))?;
                return Ok(false);
            }
        };
        match parse_delivery(&value) {
            Ok(delivery) => {
                self.journal("delivery", delivery.to_json())?;
                self.delivery = Some(delivery);
                self.outcome = Some(if planning {
                    Outcome::NoPlan
                } else {
                    Outcome::Completed
                });
                self.tool_ok(call, "The investigation is closed.".to_string())?;
                Ok(true)
            }
            Err(reason) => {
                self.tool_error(call, &format!("finish was refused: {reason}"))?;
                Ok(false)
            }
        }
    }

    fn read_file(&mut self, call: &ToolCall) -> Result<(), AgentError> {
        let value = match Json::parse(&call.arguments) {
            Ok(value) => value,
            Err(error) => {
                return self.tool_error(call, &format!("the arguments are not JSON: {error}"));
            }
        };
        let Some(fields) = json::fields(&value) else {
            return self.tool_error(call, "the arguments must be an object");
        };
        if let Err(reason) = json::only(fields, &["path"]) {
            return self.tool_error(call, &reason);
        }
        let path = match json::string(fields, "path") {
            Ok(path) => path,
            Err(reason) => return self.tool_error(call, &reason),
        };
        let resolved = match self.sandbox.resolve(path) {
            Ok(path) => path,
            Err(error) => return self.tool_error(call, &format!("{error}")),
        };
        let file = match std::fs::File::open(&resolved) {
            Ok(file) => file,
            Err(error) => {
                return self.tool_error(call, &format!("cannot read `{path}`: {error}"));
            }
        };
        let mut bytes = Vec::new();
        if let Err(error) =
            std::io::Read::take(file, READ_FILE_LIMIT as u64).read_to_end(&mut bytes)
        {
            return self.tool_error(call, &format!("cannot read `{path}`: {error}"));
        }
        let text = String::from_utf8_lossy(&bytes).to_string();
        let suffix = if bytes.len() >= READ_FILE_LIMIT {
            "\n[read stopped at 32 KiB]"
        } else {
            ""
        };
        self.tool_ok(call, format!("{path}:\n{text}{suffix}"))
    }

    fn tool_ok(&mut self, call: &ToolCall, content: String) -> Result<(), AgentError> {
        self.tool_result(call, true, &content)?;
        self.messages.push(Message::tool(&call.id, content));
        Ok(())
    }

    fn tool_error(&mut self, call: &ToolCall, reason: &str) -> Result<(), AgentError> {
        let content = format!("error: {reason}");
        self.tool_result(call, false, reason)?;
        self.messages.push(Message::tool(&call.id, content));
        Ok(())
    }

    fn tool_result(&mut self, call: &ToolCall, ok: bool, detail: &str) -> Result<(), AgentError> {
        self.journal(
            "tool.result",
            Json::object(vec![
                ("call", Json::text(&call.id)),
                ("name", Json::text(&call.name)),
                ("ok", Json::Bool(ok)),
                ("detail", Json::text(cap(detail, TOOL_DETAIL_LIMIT))),
            ]),
        )
    }

    fn ask(&mut self) -> Result<Option<ModelResponse>, AgentError> {
        self.turns += 1;
        self.journal(
            "model.request",
            Json::object(vec![
                ("turn", Json::count(self.turns as u64)),
                ("messages", Json::count(self.messages.len() as u64)),
                (
                    "tools",
                    Json::Arr(
                        self.tools
                            .iter()
                            .map(|tool| Json::text(&tool.name))
                            .collect(),
                    ),
                ),
            ]),
        )?;
        let request = ModelRequest {
            messages: self.messages.clone(),
            tools: self.tools.clone(),
        };
        match self.config.provider.complete(&request) {
            Ok(response) => {
                self.journal_response(&response)?;
                Ok(Some(response))
            }
            Err(error) => {
                self.fail(error.to_string());
                Ok(None)
            }
        }
    }

    fn journal_response(&mut self, response: &ModelResponse) -> Result<(), AgentError> {
        let calls = response
            .tool_calls
            .iter()
            .map(|call| {
                Json::object(vec![
                    ("id", Json::text(&call.id)),
                    ("name", Json::text(&call.name)),
                    (
                        "arguments",
                        Json::text(cap(&call.arguments, TOOL_DETAIL_LIMIT)),
                    ),
                ])
            })
            .collect();
        self.journal(
            "model.response",
            Json::object(vec![
                (
                    "content",
                    response
                        .content
                        .as_ref()
                        .map_or(Json::Null, |text| Json::text(cap(text, MODEL_TEXT_LIMIT))),
                ),
                ("tool_calls", Json::Arr(calls)),
            ]),
        )
    }

    fn budget_available(&mut self) -> bool {
        if self.outcome.is_some() {
            return false;
        }
        if self.turns >= self.config.max_turns {
            self.budget("the model-turn budget ran out");
            return false;
        }
        if self.calls >= self.config.max_tool_calls {
            self.budget("the tool-call budget ran out");
            return false;
        }
        if Instant::now() >= self.deadline {
            self.budget("the wall-clock budget ran out");
            return false;
        }
        true
    }

    fn count_call(&mut self) -> bool {
        if self.calls >= self.config.max_tool_calls {
            self.budget("the tool-call budget ran out");
            return false;
        }
        self.calls += 1;
        true
    }

    fn budget(&mut self, reason: &str) {
        if self.outcome.is_none() {
            self.outcome = Some(Outcome::BudgetExhausted);
            self.failure = Some(format!(
                "{reason} before the investigation finished; the record is incomplete"
            ));
        }
    }

    fn fail(&mut self, reason: String) {
        if self.outcome.is_none() {
            self.outcome = Some(Outcome::Failed);
            self.failure = Some(reason);
        }
    }

    fn journal(&mut self, kind: &str, payload: Json) -> Result<(), AgentError> {
        self.record.journal_mut().append(kind, payload)?;
        Ok(())
    }

    fn finish(mut self) -> Result<RunReport, AgentError> {
        let outcome = self.outcome.unwrap_or_else(|| {
            self.failure = Some("the investigation ended without a delivery".to_string());
            Outcome::Failed
        });
        let finished = format_utc(now_seconds());
        let event = if outcome.is_success() {
            "investigation.completed"
        } else {
            "investigation.failed"
        };
        self.journal(
            event,
            Json::object(vec![
                ("outcome", Json::text(outcome.as_str())),
                (
                    "failure",
                    self.failure.as_ref().map_or(Json::Null, |reason| {
                        Json::text(cap(reason, TOOL_DETAIL_LIMIT))
                    }),
                ),
                ("findings", Json::count(self.findings.len() as u64)),
                ("turns", Json::count(self.turns as u64)),
                ("calls", Json::count(self.calls as u64)),
            ]),
        )?;
        let head = self.record.journal_mut().head().to_string();
        let events = self.record.journal_mut().events();
        let report = RunReport {
            id: self
                .record
                .root()
                .file_name()
                .map_or_else(String::new, |name| name.to_string_lossy().to_string()),
            root: self.record.root().to_path_buf(),
            outcome,
            objective: self.config.objective.clone(),
            source: self.config.source.clone(),
            provider: self.config.provider.describe(),
            started: self.started.clone(),
            finished: finished.clone(),
            inspection: self.inspection.clone(),
            copy: self.copy.clone(),
            plan: self.plan.clone(),
            validated: self.validated.clone(),
            approved: self.approved,
            results: self.results.clone(),
            findings: self.findings.clone(),
            suggestions: self.suggestions.clone(),
            delivery: self.delivery.clone(),
            failure: self.failure.clone(),
            turns: self.turns,
            calls: self.calls,
            journal_head: head.clone(),
            journal_events: events,
            sandbox_sha256: self.sandbox_sha256.clone(),
        };
        if let Some(plan) = &report.plan {
            self.record.write_json("plan.json", &plan.to_json())?;
        }
        self.record
            .write_text("report.md", &report::render_report(&report))?;
        self.record
            .write_json("investigation.json", &report::machine_summary(&report))?;
        let manifest = manifest_json(&report, &self.config, &finished);
        self.record.finish(&manifest)?;
        Ok(report)
    }
}

fn begin_record(
    config: &RunConfig,
    inspection: &Inspection,
    started: &str,
    provider: &str,
) -> Result<RecordDir, AgentError> {
    let mut record = RecordDir::create(&config.workdir)?;
    record.journal_mut().append(
        "investigation.started",
        Json::object(vec![
            ("objective", Json::text(&config.objective)),
            ("source", Json::text(config.source.display().to_string())),
            ("provider", Json::text(provider)),
            (
                "allow_exec",
                Json::Arr(config.allow_exec.iter().map(Json::text).collect()),
            ),
            ("max_turns", Json::count(config.max_turns as u64)),
            ("max_tool_calls", Json::count(config.max_tool_calls as u64)),
            ("total_timeout_ms", Json::count(config.total_timeout_ms)),
            ("started", Json::text(started)),
        ]),
    )?;
    record.write_json("inspection.json", &inspection.to_json())?;
    record.journal_mut().append(
        "inspection.completed",
        Json::object(vec![
            ("facts", Json::count(inspection.facts.len() as u64)),
            ("files", Json::count(inspection.summary.files as u64)),
            ("truncated", Json::Bool(inspection.summary.truncated)),
        ]),
    )?;
    Ok(record)
}

fn create_sandbox(
    config: &RunConfig,
    inspection: &Inspection,
    started: &str,
    provider: &str,
    record: &mut RecordDir,
) -> Result<(Sandbox, CopyReport, String), AgentError> {
    let (sandbox, copy) = match Sandbox::create(&config.source, record.workspace()) {
        Ok(pair) => pair,
        Err(error) => {
            let reason = error.to_string();
            let _ = record.journal_mut().append(
                "investigation.failed",
                Json::object(vec![("reason", Json::text(&reason))]),
            );
            let head = record.journal_mut().head().to_string();
            let events = record.journal_mut().events();
            let manifest = failure_manifest(
                config, inspection, provider, started, &head, events, &reason,
            );
            let _ = record.finish(&manifest);
            return Err(AgentError::Sandbox(reason));
        }
    };
    let copy_json = copy_to_json(&copy);
    let copy_text = copy_json.to_pretty();
    record.write_text("sandbox.json", &copy_text)?;
    let sandbox_sha256 = sha256_hex(copy_text.as_bytes());
    record.journal_mut().append(
        "sandbox.created",
        Json::object(vec![
            ("files", Json::count(copy.files.len() as u64)),
            ("bytes", Json::count(copy.total_bytes)),
            ("ignored", Json::count(copy.ignored.len() as u64)),
            ("sha256", Json::text(&sandbox_sha256)),
        ]),
    )?;
    Ok((sandbox, copy, sandbox_sha256))
}

fn parse_delivery(value: &Json) -> Result<Delivery, String> {
    let fields = json::fields(value).ok_or_else(|| "finish takes one JSON object".to_string())?;
    json::only(fields, &["summary", "next_steps", "unresolved"])?;
    let summary = bounded(fields, "summary", 4096)?;
    let next_steps = string_list(fields, "next_steps", 1, 32, 1024)?;
    let unresolved = string_list(fields, "unresolved", 0, 32, 1024)?;
    Ok(Delivery {
        summary,
        next_steps,
        unresolved,
    })
}

fn parse_suggestion(value: &Json) -> Result<Suggestion, String> {
    let fields =
        json::fields(value).ok_or_else(|| "a suggestion must be a JSON object".to_string())?;
    json::only(fields, &["question", "why", "method", "estimated_cost"])?;
    Ok(Suggestion {
        question: bounded(fields, "question", 2048)?,
        why: bounded(fields, "why", 2048)?,
        method: bounded(fields, "method", 2048)?,
        estimated_cost: bounded(fields, "estimated_cost", 2048)?,
    })
}

fn bounded(fields: &json::Fields, key: &str, limit: usize) -> Result<String, String> {
    let text = json::string(fields, key)?;
    if text.trim().is_empty() {
        return Err(format!("field `{key}` must not be empty"));
    }
    if text.len() > limit {
        return Err(format!("field `{key}` is longer than {limit} characters"));
    }
    Ok(text.to_string())
}

fn string_list(
    fields: &json::Fields,
    key: &str,
    minimum: usize,
    maximum: usize,
    limit: usize,
) -> Result<Vec<String>, String> {
    let values = json::string_array(fields, key)?;
    if values.len() < minimum || values.len() > maximum {
        return Err(format!(
            "field `{key}` must have between {minimum} and {maximum} items"
        ));
    }
    for value in &values {
        if value.trim().is_empty() || value.len() > limit {
            return Err(format!("field `{key}` has an empty or too-long item"));
        }
    }
    Ok(values)
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

fn copy_to_json(copy: &CopyReport) -> Json {
    let files = copy
        .files
        .iter()
        .map(|file| {
            Json::object(vec![
                ("path", Json::text(&file.path)),
                ("bytes", Json::count(file.bytes)),
                ("sha256", Json::text(&file.sha256)),
            ])
        })
        .collect();
    let ignored = copy.ignored.iter().map(Json::text).collect();
    Json::object(vec![
        ("files", Json::Arr(files)),
        ("ignored", Json::Arr(ignored)),
        ("total_bytes", Json::count(copy.total_bytes)),
    ])
}

fn manifest_json(report: &RunReport, config: &RunConfig, finished: &str) -> Json {
    Json::object(vec![
        ("schema", Json::count(1)),
        (
            "tool",
            Json::text(format!("aporia investigate {}", env!("CARGO_PKG_VERSION"))),
        ),
        ("started", Json::text(&report.started)),
        ("finished", Json::text(finished)),
        ("outcome", Json::text(report.outcome.as_str())),
        ("objective", Json::text(&report.objective)),
        ("provider", Json::text(&report.provider)),
        (
            "source",
            Json::object(vec![
                ("path", Json::text(report.source.display().to_string())),
                (
                    "git_head",
                    report
                        .inspection
                        .summary
                        .git_head
                        .as_ref()
                        .map_or(Json::Null, Json::text),
                ),
            ]),
        ),
        (
            "budgets",
            Json::object(vec![
                (
                    "allow_exec",
                    Json::Arr(config.allow_exec.iter().map(Json::text).collect()),
                ),
                ("max_turns", Json::count(config.max_turns as u64)),
                ("max_tool_calls", Json::count(config.max_tool_calls as u64)),
                ("total_timeout_ms", Json::count(config.total_timeout_ms)),
            ]),
        ),
        (
            "counts",
            Json::object(vec![
                ("turns", Json::count(report.turns as u64)),
                ("tool_calls", Json::count(report.calls as u64)),
                ("executions", Json::count(report.results.len() as u64)),
                ("findings", Json::count(report.findings.len() as u64)),
                (
                    "gaps",
                    Json::count(
                        report
                            .plan
                            .as_ref()
                            .map_or(0, |plan| plan.gaps.len() as u64),
                    ),
                ),
            ]),
        ),
        ("sandbox_sha256", Json::text(&report.sandbox_sha256)),
        (
            "journal",
            Json::object(vec![
                ("events", Json::count(report.journal_events)),
                ("head", Json::text(&report.journal_head)),
            ]),
        ),
        (
            "failure",
            report.failure.as_ref().map_or(Json::Null, |reason| {
                Json::text(cap(reason, TOOL_DETAIL_LIMIT))
            }),
        ),
        ("workspace", Json::text("workspace")),
    ])
}

fn failure_manifest(
    config: &RunConfig,
    inspection: &Inspection,
    provider: &str,
    started: &str,
    head: &str,
    events: u64,
    reason: &str,
) -> Json {
    Json::object(vec![
        ("schema", Json::count(1)),
        (
            "tool",
            Json::text(format!("aporia investigate {}", env!("CARGO_PKG_VERSION"))),
        ),
        ("started", Json::text(started)),
        ("finished", Json::text(format_utc(now_seconds()))),
        ("outcome", Json::text("failed")),
        ("objective", Json::text(&config.objective)),
        ("provider", Json::text(provider)),
        (
            "source",
            Json::object(vec![
                ("path", Json::text(config.source.display().to_string())),
                (
                    "git_head",
                    inspection
                        .summary
                        .git_head
                        .as_ref()
                        .map_or(Json::Null, Json::text),
                ),
            ]),
        ),
        ("sandbox_sha256", Json::Null),
        (
            "journal",
            Json::object(vec![
                ("events", Json::count(events)),
                ("head", Json::text(head)),
            ]),
        ),
        ("failure", Json::text(reason)),
        ("workspace", Json::text("workspace")),
    ])
}
