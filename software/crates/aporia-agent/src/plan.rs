//! What the model is allowed to propose, and what the host refuses.
//!
//! A plan is not prose. Every gap names the facts that support it, every task names the gap it
//! serves, and both are checked before a person is asked to approve anything. A finding names the
//! facts and results it rests on. The refusals are the product's honesty: a recommendation that
//! cannot point at inspected evidence never reaches the execution stage.

use crate::inspect::Inspection;
use crate::json;
use aporia_sandbox::Sandbox;
use aporia_store::Json;
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::path::Path;

pub const MAX_GAPS: usize = 16;
pub const MAX_TASKS: usize = 16;
pub const MAX_TEXT: usize = 4096;
pub const MAX_BUDGET: u64 = 100_000;
pub const MAX_EXEC_TIMEOUT_MS: u64 = 120_000;

/// The charter's four priority names, and nothing else.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    Critical,
    High,
    Recommended,
    Exploratory,
}

impl Priority {
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "critical" => Some(Self::Critical),
            "high" => Some(Self::High),
            "recommended" => Some(Self::Recommended),
            "exploratory" => Some(Self::Exploratory),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Critical => "critical",
            Self::High => "high",
            Self::Recommended => "recommended",
            Self::Exploratory => "exploratory",
        }
    }
}

/// One evidence-backed opportunity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Gap {
    pub id: String,
    pub title: String,
    pub priority: Priority,
    pub why: String,
    /// The condition that makes this necessary, so "standard practice" is not enough.
    pub condition: String,
    pub evidence: Vec<String>,
}

/// The two kinds of approved operation the first slice can execute.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskTool {
    InstrumentRun,
    Exec,
}

impl TaskTool {
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "instrument_run" => Some(Self::InstrumentRun),
            "exec" => Some(Self::Exec),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InstrumentRun => "instrument_run",
            Self::Exec => "exec",
        }
    }
}

/// One ordered investigation step.
#[derive(Clone, Debug, PartialEq)]
pub struct Task {
    pub id: String,
    pub title: String,
    pub question: String,
    pub gap_ids: Vec<String>,
    pub depends_on: Vec<String>,
    pub method: String,
    pub tool: TaskTool,
    pub arguments: Json,
    pub expected_evidence: String,
    pub success_criteria: String,
    pub estimated_cost: String,
    pub risk: String,
}

/// The model's proposal, exactly as parsed and before validation.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub gaps: Vec<Gap>,
    pub tasks: Vec<Task>,
}

impl Plan {
    #[must_use]
    pub fn to_json(&self) -> Json {
        Json::object(vec![
            (
                "gaps",
                Json::Arr(self.gaps.iter().map(Gap::to_json).collect()),
            ),
            (
                "tasks",
                Json::Arr(self.tasks.iter().map(Task::to_json).collect()),
            ),
        ])
    }
}

impl Gap {
    #[must_use]
    pub fn to_json(&self) -> Json {
        Json::object(vec![
            ("id", Json::text(&self.id)),
            ("title", Json::text(&self.title)),
            ("priority", Json::text(self.priority.as_str())),
            ("why", Json::text(&self.why)),
            ("condition", Json::text(&self.condition)),
            (
                "evidence",
                Json::Arr(self.evidence.iter().map(Json::text).collect()),
            ),
        ])
    }
}

impl Task {
    #[must_use]
    pub fn to_json(&self) -> Json {
        Json::object(vec![
            ("id", Json::text(&self.id)),
            ("title", Json::text(&self.title)),
            ("question", Json::text(&self.question)),
            (
                "gap_ids",
                Json::Arr(self.gap_ids.iter().map(Json::text).collect()),
            ),
            (
                "depends_on",
                Json::Arr(self.depends_on.iter().map(Json::text).collect()),
            ),
            ("method", Json::text(&self.method)),
            ("tool", Json::text(self.tool.as_str())),
            ("arguments", self.arguments.clone()),
            ("expected_evidence", Json::text(&self.expected_evidence)),
            ("success_criteria", Json::text(&self.success_criteria)),
            ("estimated_cost", Json::text(&self.estimated_cost)),
            ("risk", Json::text(&self.risk)),
        ])
    }
}

/// The concrete operation a validated task will run.
#[derive(Clone, Debug, PartialEq)]
pub enum Operation {
    Instrument {
        model: String,
        budget: u64,
        archive: String,
    },
    Exec {
        argv: Vec<String>,
        timeout_ms: u64,
    },
}

/// A task plus the operation the host derived from its arguments.
#[derive(Clone, Debug, PartialEq)]
pub struct ValidatedTask {
    pub task: Task,
    pub operation: Operation,
}

/// A plan that passed every check, with a dependency order.
#[derive(Clone, Debug, PartialEq)]
pub struct ValidatedPlan {
    pub plan: Plan,
    pub order: Vec<usize>,
    pub tasks: Vec<ValidatedTask>,
}

impl ValidatedPlan {
    #[must_use]
    pub fn task_at(&self, index: usize) -> &ValidatedTask {
        &self.tasks[index]
    }
}

/// Parse the model's `propose_plan` arguments.
pub fn parse_plan(value: &Json) -> Result<Plan, String> {
    let fields = json::fields(value).ok_or_else(|| "the plan must be a JSON object".to_string())?;
    json::only(fields, &["gaps", "tasks"])?;
    let gaps = json::array(fields, "gaps")?
        .iter()
        .enumerate()
        .map(|(index, gap)| parse_gap(gap).map_err(|reason| format!("gaps[{index}]: {reason}")))
        .collect::<Result<Vec<_>, _>>()?;
    let tasks = json::array(fields, "tasks")?
        .iter()
        .enumerate()
        .map(|(index, task)| parse_task(task).map_err(|reason| format!("tasks[{index}]: {reason}")))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Plan { gaps, tasks })
}

fn parse_gap(value: &Json) -> Result<Gap, String> {
    let fields = json::fields(value).ok_or_else(|| "a gap must be an object".to_string())?;
    json::only(
        fields,
        &["id", "title", "priority", "why", "condition", "evidence"],
    )?;
    let priority_text = json::string(fields, "priority")?;
    let priority = Priority::parse(priority_text).ok_or_else(|| {
        format!("priority `{priority_text}` is not one of critical, high, recommended, exploratory")
    })?;
    Ok(Gap {
        id: identifier(fields, "id")?,
        title: text(fields, "title")?,
        priority,
        why: text(fields, "why")?,
        condition: text(fields, "condition")?,
        evidence: json::string_array(fields, "evidence")?,
    })
}

fn parse_task(value: &Json) -> Result<Task, String> {
    let fields = json::fields(value).ok_or_else(|| "a task must be an object".to_string())?;
    json::only(
        fields,
        &[
            "id",
            "title",
            "question",
            "gap_ids",
            "depends_on",
            "method",
            "tool",
            "arguments",
            "expected_evidence",
            "success_criteria",
            "estimated_cost",
            "risk",
        ],
    )?;
    let tool_text = json::string(fields, "tool")?;
    let tool = TaskTool::parse(tool_text)
        .ok_or_else(|| format!("tool `{tool_text}` is not one of instrument_run, exec"))?;
    Ok(Task {
        id: identifier(fields, "id")?,
        title: text(fields, "title")?,
        question: text(fields, "question")?,
        gap_ids: json::string_array(fields, "gap_ids")?,
        depends_on: json::string_array(fields, "depends_on")?,
        method: text(fields, "method")?,
        tool,
        arguments: json::required(fields, "arguments")?.clone(),
        expected_evidence: text(fields, "expected_evidence")?,
        success_criteria: text(fields, "success_criteria")?,
        estimated_cost: text(fields, "estimated_cost")?,
        risk: text(fields, "risk")?,
    })
}

fn identifier(fields: &json::Fields, key: &str) -> Result<String, String> {
    let value = json::string(fields, key)?;
    if value.is_empty() || value.len() > 64 {
        return Err(format!("field `{key}` must be 1 to 64 characters"));
    }
    if !value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(format!(
            "field `{key}` may contain only letters, digits, `_` and `-`"
        ));
    }
    Ok(value.to_string())
}

fn text(fields: &json::Fields, key: &str) -> Result<String, String> {
    let value = json::string(fields, key)?;
    if value.trim().is_empty() {
        return Err(format!("field `{key}` must not be empty"));
    }
    if value.len() > MAX_TEXT {
        return Err(format!(
            "field `{key}` is longer than {MAX_TEXT} characters"
        ));
    }
    Ok(value.to_string())
}

/// Check a parsed plan against the inspection, the permissions and the workspace.
pub fn validate_plan(
    plan: &Plan,
    inspection: &Inspection,
    allow_exec: &[String],
    sandbox: &Sandbox,
) -> Result<ValidatedPlan, String> {
    if plan.gaps.is_empty() || plan.gaps.len() > MAX_GAPS {
        return Err(format!("a plan needs 1 to {MAX_GAPS} gaps"));
    }
    if plan.tasks.is_empty() || plan.tasks.len() > MAX_TASKS {
        return Err(format!("a plan needs 1 to {MAX_TASKS} tasks"));
    }

    let fact_ids: BTreeSet<&str> = inspection
        .facts
        .iter()
        .map(|fact| fact.id.as_str())
        .collect();
    let mut gap_ids = BTreeSet::new();
    for gap in &plan.gaps {
        if !gap_ids.insert(gap.id.as_str()) {
            return Err(format!("duplicate gap id `{}`", gap.id));
        }
        if gap.evidence.is_empty() {
            return Err(format!("gap `{}` cites no evidence", gap.id));
        }
        for evidence in &gap.evidence {
            if !fact_ids.contains(evidence.as_str()) {
                return Err(format!(
                    "gap `{}` cites `{evidence}`, which is not an inspected fact",
                    gap.id
                ));
            }
        }
    }

    let task_ids: BTreeSet<&str> = plan.tasks.iter().map(|task| task.id.as_str()).collect();
    if task_ids.len() != plan.tasks.len() {
        return Err("task ids are not unique".to_string());
    }
    for task in &plan.tasks {
        if task.gap_ids.is_empty() {
            return Err(format!("task `{}` serves no gap", task.id));
        }
        for gap_id in &task.gap_ids {
            if !gap_ids.contains(gap_id.as_str()) {
                return Err(format!(
                    "task `{}` names gap `{gap_id}`, which the plan does not define",
                    task.id
                ));
            }
        }
        for dependency in &task.depends_on {
            if dependency == &task.id {
                return Err(format!("task `{}` depends on itself", task.id));
            }
            if !task_ids.contains(dependency.as_str()) {
                return Err(format!(
                    "task `{}` depends on `{dependency}`, which the plan does not define",
                    task.id
                ));
            }
        }
    }

    let order = topological_order(plan)?;
    let mut tasks = Vec::with_capacity(plan.tasks.len());
    for task in &plan.tasks {
        let operation = validate_operation(task, allow_exec, sandbox)?;
        tasks.push(ValidatedTask {
            task: task.clone(),
            operation,
        });
    }
    Ok(ValidatedPlan {
        plan: plan.clone(),
        order,
        tasks,
    })
}

fn topological_order(plan: &Plan) -> Result<Vec<usize>, String> {
    let mut index_by_id = HashMap::new();
    for (index, task) in plan.tasks.iter().enumerate() {
        index_by_id.insert(task.id.as_str(), index);
    }
    let mut indegree = vec![0usize; plan.tasks.len()];
    let mut edges: Vec<Vec<usize>> = vec![Vec::new(); plan.tasks.len()];
    for (index, task) in plan.tasks.iter().enumerate() {
        for dependency in &task.depends_on {
            let Some(&from) = index_by_id.get(dependency.as_str()) else {
                return Err(format!(
                    "task `{}` depends on `{dependency}`, which the plan does not define",
                    task.id
                ));
            };
            edges[from].push(index);
            indegree[index] += 1;
        }
    }
    let mut ready: VecDeque<usize> = indegree
        .iter()
        .enumerate()
        .filter(|(_, degree)| **degree == 0)
        .map(|(index, _)| index)
        .collect();
    let mut order = Vec::with_capacity(plan.tasks.len());
    while let Some(index) = ready.pop_front() {
        order.push(index);
        for &next in &edges[index] {
            indegree[next] -= 1;
            if indegree[next] == 0 {
                ready.push_back(next);
            }
        }
    }
    if order.len() != plan.tasks.len() {
        let cyclic: Vec<&str> = plan
            .tasks
            .iter()
            .zip(indegree.iter())
            .filter(|(_, degree)| **degree > 0)
            .map(|(task, _)| task.id.as_str())
            .collect();
        return Err(format!(
            "the tasks have a dependency cycle through {}",
            cyclic.join(", ")
        ));
    }
    Ok(order)
}

fn validate_operation(
    task: &Task,
    allow_exec: &[String],
    sandbox: &Sandbox,
) -> Result<Operation, String> {
    let arguments = json::fields(&task.arguments)
        .ok_or_else(|| format!("task `{}` arguments must be an object", task.id))?;
    match task.tool {
        TaskTool::InstrumentRun => {
            json::only(arguments, &["model", "budget", "archive"])?;
            let model = json::string(arguments, "model")?.to_string();
            if !Path::new(&model)
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("ap"))
            {
                return Err(format!("task `{}` model must be a .ap file", task.id));
            }
            let resolved = sandbox
                .resolve(&model)
                .map_err(|error| format!("task `{}` model: {error}", task.id))?;
            if !resolved.is_file() {
                return Err(format!(
                    "task `{}` names `{model}`, which is not a file in the workspace",
                    task.id
                ));
            }
            let budget = json::u64_value(arguments, "budget")?;
            if budget == 0 || budget > MAX_BUDGET {
                return Err(format!(
                    "task `{}` budget must be between 1 and {MAX_BUDGET}",
                    task.id
                ));
            }
            let archive = json::optional_string(arguments, "archive")?
                .map_or_else(|| format!("archives/{}", task.id), str::to_string);
            let resolved_archive = sandbox
                .resolve(&archive)
                .map_err(|error| format!("task `{}` archive: {error}", task.id))?;
            if resolved_archive.exists() {
                return Err(format!(
                    "task `{}` archive `{archive}` already exists; an archive is never replaced",
                    task.id
                ));
            }
            Ok(Operation::Instrument {
                model,
                budget,
                archive,
            })
        }
        TaskTool::Exec => {
            json::only(arguments, &["argv", "timeout_ms"])?;
            let argv = json::string_array(arguments, "argv")?;
            if argv.is_empty() || argv.len() > 32 {
                return Err(format!("task `{}` argv must have 1 to 32 items", task.id));
            }
            for argument in &argv {
                if argument.is_empty() || argument.len() > 512 || argument.contains('\0') {
                    return Err(format!(
                        "task `{}` has an empty, too long, or NUL-containing argument",
                        task.id
                    ));
                }
            }
            let program = Path::new(&argv[0]);
            let mut components = program.components();
            let Some(std::path::Component::Normal(name)) = components.next() else {
                return Err(format!(
                    "task `{}` program must be a bare program name, not a path",
                    task.id
                ));
            };
            if components.next().is_some() {
                return Err(format!(
                    "task `{}` program must be a bare program name, not a path",
                    task.id
                ));
            }
            let name = name.to_string_lossy();
            let allowed = allow_exec.iter().any(|allowed| {
                if cfg!(windows) {
                    allowed.eq_ignore_ascii_case(&name)
                } else {
                    allowed == name.as_ref()
                }
            });
            if !allowed {
                return Err(format!(
                    "task `{}` runs `{name}`, which this investigation was not allowed to run; pass --allow-exec {name} to permit it",
                    task.id
                ));
            }
            let timeout_ms =
                json::optional_u64(arguments, "timeout_ms")?.unwrap_or(MAX_EXEC_TIMEOUT_MS);
            if timeout_ms == 0 || timeout_ms > MAX_EXEC_TIMEOUT_MS {
                return Err(format!(
                    "task `{}` timeout_ms must be between 1 and {MAX_EXEC_TIMEOUT_MS}",
                    task.id
                ));
            }
            Ok(Operation::Exec { argv, timeout_ms })
        }
    }
}

/// What kind of statement a finding is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FindingKind {
    Observation,
    Interpretation,
    Hypothesis,
}

impl FindingKind {
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "observation" => Some(Self::Observation),
            "interpretation" => Some(Self::Interpretation),
            "hypothesis" => Some(Self::Hypothesis),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Observation => "observation",
            Self::Interpretation => "interpretation",
            Self::Hypothesis => "hypothesis",
        }
    }
}

/// How loud a finding is. Only the model's own assessment; the report labels it as such.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Critical,
    High,
    Moderate,
    Info,
}

impl Severity {
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "critical" => Some(Self::Critical),
            "high" => Some(Self::High),
            "moderate" => Some(Self::Moderate),
            "info" => Some(Self::Info),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Critical => "critical",
            Self::High => "high",
            Self::Moderate => "moderate",
            Self::Info => "info",
        }
    }
}

/// A grounded statement about what the investigation found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub kind: FindingKind,
    pub severity: Severity,
    pub text: String,
    pub evidence: Vec<String>,
    pub uncertainty: String,
}

impl Finding {
    #[must_use]
    pub fn to_json(&self) -> Json {
        Json::object(vec![
            ("kind", Json::text(self.kind.as_str())),
            ("severity", Json::text(self.severity.as_str())),
            ("text", Json::text(&self.text)),
            (
                "evidence",
                Json::Arr(self.evidence.iter().map(Json::text).collect()),
            ),
            ("uncertainty", Json::text(&self.uncertainty)),
        ])
    }
}

/// Parse and ground a `record_finding` call.
pub fn parse_finding(value: &Json, known: &BTreeSet<String>) -> Result<Finding, String> {
    let fields = json::fields(value).ok_or_else(|| "a finding must be an object".to_string())?;
    json::only(
        fields,
        &["kind", "severity", "text", "evidence", "uncertainty"],
    )?;
    let kind_text = json::string(fields, "kind")?;
    let kind = FindingKind::parse(kind_text).ok_or_else(|| {
        format!("kind `{kind_text}` is not one of observation, interpretation, hypothesis")
    })?;
    let severity_text = json::string(fields, "severity")?;
    let severity = Severity::parse(severity_text).ok_or_else(|| {
        format!("severity `{severity_text}` is not one of critical, high, moderate, info")
    })?;
    let finding_text = text(fields, "text")?;
    let uncertainty = text(fields, "uncertainty")?;
    let evidence = json::string_array(fields, "evidence")?;
    if evidence.is_empty() {
        return Err("a finding must cite at least one fact or result".to_string());
    }
    for reference in &evidence {
        if !known.contains(reference) {
            return Err(format!(
                "the finding cites `{reference}`, which this investigation did not record"
            ));
        }
    }
    Ok(Finding {
        kind,
        severity,
        text: finding_text,
        evidence,
        uncertainty,
    })
}
