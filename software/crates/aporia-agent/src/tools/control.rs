//! The tools that act on the investigation rather than on the project.
//!
//! Proposing a plan, revising it, recording a finding, keeping the experiment backlog, marking
//! a task done and closing the delivery all go through the same registry as reading a file or
//! running a command: the model names one, the host validates it against what this investigation
//! actually recorded, and the decision is journalled. A plan becomes permission only when a
//! person approves it, and an approval that expands the scope says so explicitly.

use super::result::{ErrorClass, ToolFailure, ToolOutput, ToolResult};
use super::{Tool, ToolContext, ToolMeta};
use crate::investigation::{
    Experiment, ExperimentStatus, PlannedTask, Proposal, ScopeRequest, TaskStatus,
};
use crate::json;
use crate::plan::{Finding, Gap, Priority, parse_finding};
use crate::scope::{ApprovalKind, ApprovalRequest, Risk, Scope, validate_program};
use aporia_store::Json;
use std::collections::{BTreeSet, VecDeque};

const OUTPUT_LIMIT: usize = 16 * 1024;
const MAX_GAPS: usize = 16;
const MAX_TASKS: usize = 32;
const MAX_TEXT: usize = 4096;
const MAX_LIST: usize = 32;

/// Register the control tools.
pub fn register(registry: &mut super::Registry) -> Result<(), String> {
    registry.register(Box::new(ProposePlan::new()))?;
    registry.register(Box::new(RevisePlan::new()))?;
    registry.register(Box::new(RecordFinding::new()))?;
    registry.register(Box::new(ProposeExperiment::new()))?;
    registry.register(Box::new(UpdateExperiment::new()))?;
    registry.register(Box::new(UpdateTask::new()))?;
    registry.register(Box::new(Finish::new()))?;
    Ok(())
}

fn text(fields: &json::Fields, key: &str) -> Result<String, ToolFailure> {
    let value = json::string(fields, key)?;
    if value.trim().is_empty() {
        return Err(ToolFailure::new(
            ErrorClass::InvalidArguments,
            format!("field `{key}` must not be empty"),
        ));
    }
    if value.len() > MAX_TEXT {
        return Err(ToolFailure::new(
            ErrorClass::InvalidArguments,
            format!("field `{key}` is longer than {MAX_TEXT} characters"),
        ));
    }
    Ok(value.to_string())
}

fn list(fields: &json::Fields, key: &str) -> Result<Vec<String>, ToolFailure> {
    let values = json::string_array(fields, key)?;
    if values.len() > MAX_LIST {
        return Err(ToolFailure::new(
            ErrorClass::InvalidArguments,
            format!("field `{key}` has more than {MAX_LIST} items"),
        ));
    }
    for value in &values {
        if value.trim().is_empty() || value.len() > MAX_TEXT {
            return Err(ToolFailure::new(
                ErrorClass::InvalidArguments,
                format!("field `{key}` has an empty or too-long item"),
            ));
        }
    }
    Ok(values)
}

fn parse_proposal(
    arguments: &Json,
    known: &BTreeSet<&str>,
    limits: &crate::scope::ScopeLimits,
) -> Result<Proposal, ToolFailure> {
    let fields = json::fields(arguments).ok_or_else(|| {
        ToolFailure::new(ErrorClass::InvalidArguments, "the plan must be an object")
    })?;
    json::only(
        fields,
        &["objective", "gaps", "tasks", "scope", "success_criteria"],
    )
    .map_err(ToolFailure::from)?;
    let objective = text(fields, "objective")?;
    let success_criteria = text(fields, "success_criteria")?;

    let gaps = json::array(fields, "gaps")
        .map_err(ToolFailure::from)?
        .iter()
        .enumerate()
        .map(|(index, value)| {
            parse_gap(value, known).map_err(|failure| {
                ToolFailure::new(
                    ErrorClass::InvalidArguments,
                    format!("gaps[{index}]: {}", failure.message),
                )
            })
        })
        .collect::<Result<Vec<Gap>, ToolFailure>>()?;
    if gaps.is_empty() || gaps.len() > MAX_GAPS {
        return Err(ToolFailure::new(
            ErrorClass::InvalidArguments,
            format!("a plan needs 1 to {MAX_GAPS} gaps"),
        ));
    }
    let gap_ids: BTreeSet<&str> = gaps.iter().map(|gap| gap.id.as_str()).collect();
    if gap_ids.len() != gaps.len() {
        return Err(ToolFailure::new(
            ErrorClass::InvalidArguments,
            "the gap ids are not unique",
        ));
    }

    let tasks = json::array(fields, "tasks")
        .map_err(ToolFailure::from)?
        .iter()
        .enumerate()
        .map(|(index, value)| {
            parse_task(value, &gap_ids).map_err(|failure| {
                ToolFailure::new(
                    ErrorClass::InvalidArguments,
                    format!("tasks[{index}]: {}", failure.message),
                )
            })
        })
        .collect::<Result<Vec<PlannedTask>, ToolFailure>>()?;
    if tasks.is_empty() || tasks.len() > MAX_TASKS {
        return Err(ToolFailure::new(
            ErrorClass::InvalidArguments,
            format!("a plan needs 1 to {MAX_TASKS} tasks"),
        ));
    }
    let task_ids: BTreeSet<&str> = tasks.iter().map(|task| task.id.as_str()).collect();
    if task_ids.len() != tasks.len() {
        return Err(ToolFailure::new(
            ErrorClass::InvalidArguments,
            "the task ids are not unique",
        ));
    }
    check_cycles(&tasks)?;

    let scope = parse_scope(fields)?;
    scope
        .as_scope()
        .within(limits)
        .map_err(|reason| ToolFailure::new(ErrorClass::Denied, reason))?;

    Ok(Proposal {
        objective,
        gaps,
        tasks,
        scope,
        success_criteria,
    })
}

fn parse_gap(value: &Json, known: &BTreeSet<&str>) -> Result<Gap, ToolFailure> {
    let fields = json::fields(value).ok_or_else(|| "a gap must be an object".to_string())?;
    json::only(
        fields,
        &["id", "title", "priority", "why", "condition", "evidence"],
    )?;
    let priority_text = json::string(fields, "priority")?;
    let priority = Priority::parse(priority_text).ok_or_else(|| {
        format!("priority `{priority_text}` is not one of critical, high, recommended, exploratory")
    })?;
    let evidence = json::string_array(fields, "evidence")?;
    if evidence.is_empty() {
        return Err(ToolFailure::new(
            ErrorClass::InvalidArguments,
            "a gap must cite at least one piece of evidence",
        ));
    }
    for reference in &evidence {
        if !known.contains(reference.as_str()) {
            return Err(ToolFailure::new(
                ErrorClass::InvalidArguments,
                format!("`{reference}` is not evidence this investigation recorded"),
            ));
        }
    }
    Ok(Gap {
        id: identifier(fields, "id")?,
        title: text(fields, "title")?,
        priority,
        why: text(fields, "why")?,
        condition: text(fields, "condition")?,
        evidence,
    })
}

fn parse_task(value: &Json, gap_ids: &BTreeSet<&str>) -> Result<PlannedTask, ToolFailure> {
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
            "expected_evidence",
            "success_criteria",
            "estimated_cost",
            "risk",
        ],
    )?;
    let gap_refs = json::string_array(fields, "gap_ids")?;
    if gap_refs.is_empty() {
        return Err(ToolFailure::new(
            ErrorClass::InvalidArguments,
            "a task must serve a gap",
        ));
    }
    for reference in &gap_refs {
        if !gap_ids.contains(reference.as_str()) {
            return Err(ToolFailure::new(
                ErrorClass::InvalidArguments,
                format!("`{reference}` is not a gap the plan defines"),
            ));
        }
    }
    let depends_on = json::string_array(fields, "depends_on")?;
    for reference in &depends_on {
        if reference == json::string(fields, "id")? {
            return Err(ToolFailure::new(
                ErrorClass::InvalidArguments,
                "a task cannot depend on itself",
            ));
        }
    }
    Ok(PlannedTask {
        id: identifier(fields, "id")?,
        title: text(fields, "title")?,
        question: text(fields, "question")?,
        gap_ids: gap_refs,
        depends_on,
        method: text(fields, "method")?,
        expected_evidence: text(fields, "expected_evidence")?,
        success_criteria: text(fields, "success_criteria")?,
        estimated_cost: text(fields, "estimated_cost")?,
        risk: text(fields, "risk")?,
        status: TaskStatus::Pending,
    })
}

fn identifier(fields: &json::Fields, key: &str) -> Result<String, ToolFailure> {
    let value = json::string(fields, key)?;
    if value.is_empty() || value.len() > 64 {
        return Err(ToolFailure::new(
            ErrorClass::InvalidArguments,
            format!("field `{key}` must be 1 to 64 characters"),
        ));
    }
    if !value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(ToolFailure::new(
            ErrorClass::InvalidArguments,
            format!("field `{key}` may contain only letters, digits, `_` and `-`"),
        ));
    }
    Ok(value.to_string())
}

fn check_cycles(tasks: &[PlannedTask]) -> Result<(), ToolFailure> {
    let ids: BTreeSet<&str> = tasks.iter().map(|task| task.id.as_str()).collect();
    let mut remaining: VecDeque<&PlannedTask> = tasks.iter().collect();
    let mut resolved: BTreeSet<&str> = BTreeSet::new();
    while !remaining.is_empty() {
        let mut progressed = false;
        let mut next = VecDeque::new();
        for task in remaining {
            let ready = task
                .depends_on
                .iter()
                .all(|dependency| resolved.contains(dependency.as_str()));
            if ready {
                resolved.insert(task.id.as_str());
                progressed = true;
            } else {
                next.push_back(task);
            }
        }
        remaining = next;
        if !progressed {
            let names: Vec<&str> = remaining.iter().map(|task| task.id.as_str()).collect();
            return Err(ToolFailure::new(
                ErrorClass::InvalidArguments,
                format!(
                    "the tasks have a dependency cycle through {}",
                    names.join(", ")
                ),
            ));
        }
    }
    for task in tasks {
        for dependency in &task.depends_on {
            if !ids.contains(dependency.as_str()) {
                return Err(ToolFailure::new(
                    ErrorClass::InvalidArguments,
                    format!("task `{dependency}` is not defined by the plan"),
                ));
            }
        }
    }
    Ok(())
}

fn parse_scope(fields: &json::Fields) -> Result<ScopeRequest, ToolFailure> {
    let value = json::required(fields, "scope").map_err(ToolFailure::from)?;
    let scope = json::fields(value)
        .ok_or_else(|| ToolFailure::new(ErrorClass::InvalidArguments, "scope must be an object"))?;
    json::only(
        scope,
        &[
            "allow_exec",
            "allow_write",
            "max_turns",
            "max_tool_calls",
            "timeout_ms",
        ],
    )
    .map_err(ToolFailure::from)?;
    let allow_exec = json::string_array(scope, "allow_exec")?;
    for program in &allow_exec {
        validate_program(program).map_err(ToolFailure::from)?;
    }
    let allow_write = json::required(scope, "allow_write")
        .map_err(ToolFailure::from)?
        .as_bool()
        .ok_or_else(|| {
            ToolFailure::new(
                ErrorClass::InvalidArguments,
                "field `allow_write` must be a boolean",
            )
        })?;
    Ok(ScopeRequest {
        allow_exec,
        allow_write,
        max_turns: json::u64_value(scope, "max_turns").map_err(ToolFailure::from)?,
        max_tool_calls: json::u64_value(scope, "max_tool_calls").map_err(ToolFailure::from)?,
        timeout_ms: json::u64_value(scope, "timeout_ms").map_err(ToolFailure::from)?,
    })
}

struct ProposePlan {
    meta: ToolMeta,
}

impl ProposePlan {
    fn new() -> Self {
        Self {
            meta: ToolMeta::new(
                "propose_plan",
                "Propose the investigation plan: the objective, evidence-backed gaps, ordered tasks, and the scope it needs. The host validates every citation and permission before a person sees it.",
                r#"{"type":"object","properties":{"objective":{"type":"string"},"gaps":{"type":"array","items":{"type":"object"}},"tasks":{"type":"array","items":{"type":"object"}},"scope":{"type":"object"},"success_criteria":{"type":"string"}},"required":["objective","gaps","tasks","scope","success_criteria"],"additionalProperties":false}"#,
                Risk::ReadOnly,
                OUTPUT_LIMIT,
            ),
        }
    }
}

impl Tool for ProposePlan {
    fn meta(&self) -> &ToolMeta {
        &self.meta
    }

    fn run(&self, context: &mut ToolContext, arguments: &Json) -> ToolResult {
        let session = context.session_mut().ok_or_else(|| {
            ToolFailure::new(
                ErrorClass::Internal,
                "propose_plan needs an investigation session",
            )
        })?;
        let limits = session.limits();
        let known = session.investigation().evidence_ids();
        let proposal = parse_proposal(arguments, &known, &limits)?;
        let version = session.investigation().revisions.len() as u64 + 1;
        session
            .investigation()
            .revisions
            .push(crate::investigation::PlanRevision {
                version,
                proposal: proposal.clone(),
                decision: None,
                scope: None,
            });
        session.journal(
            "plan.proposed",
            Json::object(vec![
                ("version", Json::count(version)),
                ("plan", proposal.to_json()),
            ]),
        )?;
        let request = ApprovalRequest {
            kind: ApprovalKind::Plan,
            title: format!("approve plan version {version}"),
            detail: proposal.to_json(),
        };
        session.request_approval(request);
        Ok(ToolOutput::text(format!(
            "plan version {version} was recorded with {} gaps and {} tasks; it is waiting for approval",
            proposal.gaps.len(),
            proposal.tasks.len()
        )))
    }
}

struct RevisePlan {
    meta: ToolMeta,
}

impl RevisePlan {
    fn new() -> Self {
        Self {
            meta: ToolMeta::new(
                "revise_plan",
                "Propose a new version of the plan. If it asks for more permission than the approved version, the person is asked again.",
                r#"{"type":"object","properties":{"objective":{"type":"string"},"gaps":{"type":"array","items":{"type":"object"}},"tasks":{"type":"array","items":{"type":"object"}},"scope":{"type":"object"},"success_criteria":{"type":"string"}},"required":["objective","gaps","tasks","scope","success_criteria"],"additionalProperties":false}"#,
                Risk::ReadOnly,
                OUTPUT_LIMIT,
            ),
        }
    }
}

impl Tool for RevisePlan {
    fn meta(&self) -> &ToolMeta {
        &self.meta
    }

    fn run(&self, context: &mut ToolContext, arguments: &Json) -> ToolResult {
        let session = context.session_mut().ok_or_else(|| {
            ToolFailure::new(
                ErrorClass::Internal,
                "revise_plan needs an investigation session",
            )
        })?;
        let limits = session.limits();
        let (known, approved_scope) = {
            let investigation = session.investigation();
            (investigation.evidence_ids(), investigation.scope.clone())
        };
        let proposal = parse_proposal(arguments, &known, &limits)?;
        let kind = match &approved_scope {
            Some(scope) => {
                let asked = crate::scope::expansions(scope, &proposal.scope.as_scope());
                if asked.is_empty() {
                    ApprovalKind::Plan
                } else {
                    ApprovalKind::ScopeExpansion
                }
            }
            None => ApprovalKind::Plan,
        };
        let version = session.investigation().revisions.len() as u64 + 1;
        session
            .investigation()
            .revisions
            .push(crate::investigation::PlanRevision {
                version,
                proposal: proposal.clone(),
                decision: None,
                scope: None,
            });
        session.journal(
            "plan.proposed",
            Json::object(vec![
                ("version", Json::count(version)),
                ("plan", proposal.to_json()),
            ]),
        )?;
        let title = match kind {
            ApprovalKind::ScopeExpansion => {
                format!("approve plan version {version}, which asks for more permission")
            }
            _ => format!("approve plan version {version}"),
        };
        session.request_approval(ApprovalRequest {
            kind,
            title,
            detail: proposal.to_json(),
        });
        Ok(ToolOutput::text(format!(
            "plan version {version} was recorded; it is waiting for approval"
        )))
    }
}

struct RecordFinding {
    meta: ToolMeta,
}

impl RecordFinding {
    fn new() -> Self {
        Self {
            meta: ToolMeta::new(
                "record_finding",
                "Record one grounded statement. The evidence must be ids this investigation recorded (F for facts, I for tool invocations, E for experiment outcomes).",
                r#"{"type":"object","properties":{"kind":{"type":"string","enum":["observation","interpretation","hypothesis"]},"severity":{"type":"string","enum":["critical","high","moderate","info"]},"text":{"type":"string"},"evidence":{"type":"array","items":{"type":"string"}},"uncertainty":{"type":"string"}},"required":["kind","severity","text","evidence","uncertainty"],"additionalProperties":false}"#,
                Risk::ReadOnly,
                OUTPUT_LIMIT,
            ),
        }
    }
}

impl Tool for RecordFinding {
    fn meta(&self) -> &ToolMeta {
        &self.meta
    }

    fn run(&self, context: &mut ToolContext, arguments: &Json) -> ToolResult {
        let session = context.session_mut().ok_or_else(|| {
            ToolFailure::new(
                ErrorClass::Internal,
                "record_finding needs an investigation session",
            )
        })?;
        let known: BTreeSet<String> = session
            .investigation()
            .evidence_ids()
            .into_iter()
            .map(str::to_string)
            .collect();
        let finding: Finding = parse_finding(arguments, &known)
            .map_err(|reason| ToolFailure::new(ErrorClass::InvalidArguments, reason))?;
        let number = session.investigation().findings.len() + 1;
        session.journal("finding.recorded", finding.to_json())?;
        session.investigation().findings.push(finding);
        Ok(ToolOutput::text(format!("finding {number} was recorded")))
    }
}

struct ProposeExperiment {
    meta: ToolMeta,
}

impl ProposeExperiment {
    fn new() -> Self {
        Self {
            meta: ToolMeta::new(
                "propose_experiment",
                "Add an item to the improvement backlog: the question, why it matters, the method, the variables, the data it needs, what would count as success, and the risk. Without a runner it stays a proposal, never a completed experiment.",
                r#"{"type":"object","properties":{"title":{"type":"string"},"question":{"type":"string"},"reason":{"type":"string"},"evidence":{"type":"array","items":{"type":"string"}},"priority":{"type":"string","enum":["critical","high","recommended","exploratory"]},"methodology":{"type":"string"},"variables":{"type":"array","items":{"type":"string"}},"dependencies":{"type":"array","items":{"type":"string"}},"required_data":{"type":"array","items":{"type":"string"}},"expected_outcomes":{"type":"string"},"metrics":{"type":"array","items":{"type":"string"}},"estimated_cost":{"type":"string"},"success_criteria":{"type":"string"},"risks":{"type":"string"}},"required":["title","question","reason","evidence","priority","methodology","variables","dependencies","required_data","expected_outcomes","metrics","estimated_cost","success_criteria","risks"],"additionalProperties":false}"#,
                Risk::ReadOnly,
                OUTPUT_LIMIT,
            ),
        }
    }
}

impl Tool for ProposeExperiment {
    fn meta(&self) -> &ToolMeta {
        &self.meta
    }

    fn run(&self, context: &mut ToolContext, arguments: &Json) -> ToolResult {
        let session = context.session_mut().ok_or_else(|| {
            ToolFailure::new(
                ErrorClass::Internal,
                "propose_experiment needs an investigation session",
            )
        })?;
        let fields = json::fields(arguments).ok_or_else(|| {
            ToolFailure::new(
                ErrorClass::InvalidArguments,
                "an experiment must be an object",
            )
        })?;
        let known = session.investigation().evidence_ids();
        let evidence = list(fields, "evidence")?;
        for reference in &evidence {
            if !known.contains(reference.as_str()) {
                return Err(ToolFailure::new(
                    ErrorClass::InvalidArguments,
                    format!("`{reference}` is not evidence this investigation recorded"),
                ));
            }
        }
        let experiment = Experiment {
            id: session.investigation().next_experiment_id(),
            title: text(fields, "title")?,
            question: text(fields, "question")?,
            reason: text(fields, "reason")?,
            evidence,
            priority: text(fields, "priority")?,
            methodology: text(fields, "methodology")?,
            variables: list(fields, "variables")?,
            dependencies: list(fields, "dependencies")?,
            required_data: list(fields, "required_data")?,
            expected_outcomes: text(fields, "expected_outcomes")?,
            metrics: list(fields, "metrics")?,
            estimated_cost: text(fields, "estimated_cost")?,
            success_criteria: text(fields, "success_criteria")?,
            risks: text(fields, "risks")?,
            status: ExperimentStatus::Proposed,
            result: None,
            follow_up: None,
        };
        let id = experiment.id.clone();
        session
            .journal("experiment.changed", experiment.to_json())
            .map_err(|reason| ToolFailure::new(ErrorClass::Internal, reason))?;
        session.investigation().upsert_experiment(experiment);
        Ok(ToolOutput::text(format!(
            "experiment {id} was added to the backlog as a proposal"
        )))
    }
}

struct UpdateExperiment {
    meta: ToolMeta,
}

impl UpdateExperiment {
    fn new() -> Self {
        Self {
            meta: ToolMeta::new(
                "update_experiment",
                "Update a backlog item: its status, its result, what it should follow up with, or its priority. A failed experiment stays in the history.",
                r#"{"type":"object","properties":{"id":{"type":"string"},"status":{"type":"string","enum":["proposed","approved","ready","running","completed","failed","inconclusive","blocked","cancelled"]},"result":{"type":"string"},"follow_up":{"type":"string"},"priority":{"type":"string","enum":["critical","high","recommended","exploratory"]}},"required":["id"],"additionalProperties":false}"#,
                Risk::ReadOnly,
                OUTPUT_LIMIT,
            ),
        }
    }
}

impl Tool for UpdateExperiment {
    fn meta(&self) -> &ToolMeta {
        &self.meta
    }

    fn run(&self, context: &mut ToolContext, arguments: &Json) -> ToolResult {
        let session = context.session_mut().ok_or_else(|| {
            ToolFailure::new(
                ErrorClass::Internal,
                "update_experiment needs an investigation session",
            )
        })?;
        let fields = json::fields(arguments).ok_or_else(|| {
            ToolFailure::new(ErrorClass::InvalidArguments, "an update must be an object")
        })?;
        let id = text(fields, "id")?;
        let status = match json::optional_string(fields, "status")? {
            Some(text) => Some(ExperimentStatus::parse(text).map_err(ToolFailure::from)?),
            None => None,
        };
        let priority = match json::optional_string(fields, "priority")? {
            Some(text) => {
                if Priority::parse(text).is_none() {
                    return Err(ToolFailure::new(
                        ErrorClass::InvalidArguments,
                        format!("`{text}` is not one of critical, high, recommended, exploratory"),
                    ));
                }
                Some(text.to_string())
            }
            None => None,
        };
        let result = json::optional_string(fields, "result")?.map(str::to_string);
        let follow_up = json::optional_string(fields, "follow_up")?.map(str::to_string);

        let existing = session
            .investigation()
            .experiment(&id)
            .cloned()
            .ok_or_else(|| {
                ToolFailure::new(
                    ErrorClass::NotFound,
                    format!("there is no experiment `{id}` in the backlog"),
                )
            })?;
        let mut updated = existing.clone();
        if let Some(status) = status {
            updated.status = status;
        }
        if let Some(priority) = priority {
            updated.priority = priority;
        }
        if let Some(result) = result {
            updated.result = Some(result);
        }
        if let Some(follow_up) = follow_up {
            updated.follow_up = Some(follow_up);
        }
        session
            .journal("experiment.changed", updated.to_json())
            .map_err(|reason| ToolFailure::new(ErrorClass::Internal, reason))?;
        session.investigation().upsert_experiment(updated);
        Ok(ToolOutput::text(format!(
            "experiment {id} is now {}",
            existing.status.as_str()
        )))
    }
}

struct UpdateTask {
    meta: ToolMeta,
}

impl UpdateTask {
    fn new() -> Self {
        Self {
            meta: ToolMeta::new(
                "update_task",
                "Mark a planned task as running, completed, failed or skipped. The task must be one the approved plan defines.",
                r#"{"type":"object","properties":{"id":{"type":"string"},"status":{"type":"string","enum":["pending","running","completed","failed","skipped"]}},"required":["id","status"],"additionalProperties":false}"#,
                Risk::ReadOnly,
                OUTPUT_LIMIT,
            ),
        }
    }
}

impl Tool for UpdateTask {
    fn meta(&self) -> &ToolMeta {
        &self.meta
    }

    fn run(&self, context: &mut ToolContext, arguments: &Json) -> ToolResult {
        let session = context.session_mut().ok_or_else(|| {
            ToolFailure::new(
                ErrorClass::Internal,
                "update_task needs an investigation session",
            )
        })?;
        let fields = json::fields(arguments).ok_or_else(|| {
            ToolFailure::new(ErrorClass::InvalidArguments, "an update must be an object")
        })?;
        let id = text(fields, "id")?;
        let status =
            TaskStatus::parse(json::string(fields, "status")?).map_err(ToolFailure::from)?;
        let revision = session.investigation().approved_revision().ok_or_else(|| {
            ToolFailure::new(
                ErrorClass::Denied,
                "no plan is approved, so there are no planned tasks to update",
            )
        })?;
        let task = revision
            .proposal
            .tasks
            .iter()
            .find(|task| task.id == id)
            .cloned()
            .ok_or_else(|| {
                ToolFailure::new(
                    ErrorClass::NotFound,
                    format!("the approved plan has no task `{id}`"),
                )
            })?;
        let mut updated = task.clone();
        updated.status = status;
        session
            .journal("task.changed", updated.to_json())
            .map_err(|reason| ToolFailure::new(ErrorClass::Internal, reason))?;
        // The task lives in the approved revision, so the change is written back there.
        let approved_version = session.investigation().approved_version.unwrap_or(0);
        if let Some(revision) = session
            .investigation()
            .revisions
            .iter_mut()
            .find(|revision| revision.version == approved_version)
            && let Some(slot) = revision
                .proposal
                .tasks
                .iter_mut()
                .find(|task| task.id == id)
        {
            slot.status = status;
        }
        Ok(ToolOutput::text(format!(
            "task {id} is now {}",
            status.as_str()
        )))
    }
}

struct Finish {
    meta: ToolMeta,
}

impl Finish {
    fn new() -> Self {
        Self {
            meta: ToolMeta::new(
                "finish",
                "Close the investigation with a summary, the next steps, and an explicit list of what is still unresolved. Use it only when the work is done or when nothing more is warranted.",
                r#"{"type":"object","properties":{"summary":{"type":"string"},"next_steps":{"type":"array","items":{"type":"string"}},"unresolved":{"type":"array","items":{"type":"string"}}},"required":["summary","next_steps","unresolved"],"additionalProperties":false}"#,
                Risk::ReadOnly,
                OUTPUT_LIMIT,
            ),
        }
    }
}

impl Tool for Finish {
    fn meta(&self) -> &ToolMeta {
        &self.meta
    }

    fn run(&self, context: &mut ToolContext, arguments: &Json) -> ToolResult {
        let session = context.session_mut().ok_or_else(|| {
            ToolFailure::new(
                ErrorClass::Internal,
                "finish needs an investigation session",
            )
        })?;
        let fields = json::fields(arguments).ok_or_else(|| {
            ToolFailure::new(ErrorClass::InvalidArguments, "finish takes one object")
        })?;
        json::only(fields, &["summary", "next_steps", "unresolved"]).map_err(ToolFailure::from)?;
        let summary = text(fields, "summary")?;
        let next_steps = list(fields, "next_steps")?;
        let unresolved = list(fields, "unresolved")?;
        let delivery = crate::orchestrator::Delivery {
            summary,
            next_steps,
            unresolved,
        };
        session
            .journal("delivery", delivery.to_json())
            .map_err(|reason| ToolFailure::new(ErrorClass::Internal, reason))?;
        session.deliver(delivery);
        Ok(ToolOutput::text("the investigation is closed"))
    }
}

/// The scope a plan would be judged against, exposed for the engine's own checks.
#[must_use]
pub fn requested_scope(proposal: &Proposal) -> Scope {
    proposal.scope.as_scope()
}
