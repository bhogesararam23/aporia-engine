//! The investigation's working state: proposals, tasks, evidence, findings and the backlog.
//!
//! Everything the model may cite lives here and is given an identity: an inspection fact (`F1`),
//! a tool invocation (`I1`), a planned task (`T1`), an experiment (`X1`). A citation that names
//! something this investigation never recorded is refused, which is what keeps a recommendation
//! attached to the work that produced it.
//!
//! The plan is a set of versions, not a single document: a revision is validated against the
//! evidence and the limits, and it only becomes permission when a person approves it.

use crate::orchestrator::Delivery;
use crate::plan::{Finding, Gap};
use crate::scope::{ApprovalRequest, Decision, Risk, Scope};
use aporia_store::Json;
use std::collections::BTreeSet;

/// How far a planned task has got.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Skipped,
}

impl TaskStatus {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
        }
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        match text {
            "pending" => Ok(Self::Pending),
            "running" => Ok(Self::Running),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "skipped" => Ok(Self::Skipped),
            other => Err(format!("unknown task status `{other}`")),
        }
    }
}

/// One step the model intends to take, with the gap it serves.
#[derive(Clone, Debug, PartialEq)]
pub struct PlannedTask {
    pub id: String,
    pub title: String,
    pub question: String,
    pub gap_ids: Vec<String>,
    pub depends_on: Vec<String>,
    pub method: String,
    pub expected_evidence: String,
    pub success_criteria: String,
    pub estimated_cost: String,
    pub risk: String,
    pub status: TaskStatus,
}

impl PlannedTask {
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
            ("expected_evidence", Json::text(&self.expected_evidence)),
            ("success_criteria", Json::text(&self.success_criteria)),
            ("estimated_cost", Json::text(&self.estimated_cost)),
            ("risk", Json::text(&self.risk)),
            ("status", Json::text(self.status.as_str())),
        ])
    }
}

/// What a plan asks to be permitted to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScopeRequest {
    pub allow_exec: Vec<String>,
    pub allow_write: bool,
    pub max_turns: u64,
    pub max_tool_calls: u64,
    pub timeout_ms: u64,
}

impl ScopeRequest {
    /// The scope this request would grant, if it were approved.
    #[must_use]
    pub fn as_scope(&self) -> Scope {
        Scope {
            allow_exec: self.allow_exec.clone(),
            allow_write: self.allow_write,
            max_turns: self.max_turns,
            max_tool_calls: self.max_tool_calls,
            timeout_ms: self.timeout_ms,
        }
    }

    #[must_use]
    pub fn to_json(&self) -> Json {
        self.as_scope().to_json()
    }
}

/// A plan the model proposed, before any human looked at it.
#[derive(Clone, Debug, PartialEq)]
pub struct Proposal {
    pub objective: String,
    pub gaps: Vec<Gap>,
    pub tasks: Vec<PlannedTask>,
    pub scope: ScopeRequest,
    pub success_criteria: String,
}

impl Proposal {
    #[must_use]
    pub fn to_json(&self) -> Json {
        Json::object(vec![
            ("objective", Json::text(&self.objective)),
            (
                "gaps",
                Json::Arr(self.gaps.iter().map(Gap::to_json).collect()),
            ),
            (
                "tasks",
                Json::Arr(self.tasks.iter().map(PlannedTask::to_json).collect()),
            ),
            ("scope", self.scope.to_json()),
            ("success_criteria", Json::text(&self.success_criteria)),
        ])
    }
}

/// One version of the plan and what was decided about it.
#[derive(Clone, Debug, PartialEq)]
pub struct PlanRevision {
    pub version: u64,
    pub proposal: Proposal,
    pub decision: Option<Decision>,
    /// The scope in force while this revision is approved.
    pub scope: Option<Scope>,
}

impl PlanRevision {
    #[must_use]
    pub fn to_json(&self) -> Json {
        Json::object(vec![
            ("version", Json::count(self.version)),
            ("proposal", self.proposal.to_json()),
            (
                "decision",
                self.decision
                    .map_or(Json::Null, |decision| Json::text(decision.as_str())),
            ),
            (
                "scope",
                self.scope.as_ref().map_or(Json::Null, Scope::to_json),
            ),
        ])
    }
}

/// Where an experiment stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExperimentStatus {
    Proposed,
    Approved,
    Ready,
    Running,
    Completed,
    Failed,
    Inconclusive,
    Blocked,
    Cancelled,
}

impl ExperimentStatus {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Proposed => "proposed",
            Self::Approved => "approved",
            Self::Ready => "ready",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Inconclusive => "inconclusive",
            Self::Blocked => "blocked",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        match text {
            "proposed" => Ok(Self::Proposed),
            "approved" => Ok(Self::Approved),
            "ready" => Ok(Self::Ready),
            "running" => Ok(Self::Running),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "inconclusive" => Ok(Self::Inconclusive),
            "blocked" => Ok(Self::Blocked),
            "cancelled" => Ok(Self::Cancelled),
            other => Err(format!("unknown experiment status `{other}`")),
        }
    }

    /// Whether the experiment ended. A failed experiment stays in the history.
    #[must_use]
    pub fn is_finished(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Inconclusive | Self::Cancelled
        )
    }
}

/// One item of the improvement backlog: work that was identified, whether or not it ran.
#[derive(Clone, Debug, PartialEq)]
pub struct Experiment {
    pub id: String,
    pub title: String,
    pub question: String,
    pub reason: String,
    pub evidence: Vec<String>,
    pub priority: String,
    pub methodology: String,
    pub variables: Vec<String>,
    pub dependencies: Vec<String>,
    pub required_data: Vec<String>,
    pub expected_outcomes: String,
    pub metrics: Vec<String>,
    pub estimated_cost: String,
    pub success_criteria: String,
    pub risks: String,
    pub status: ExperimentStatus,
    /// What actually happened, once it ran.
    pub result: Option<String>,
    pub follow_up: Option<String>,
}

impl Experiment {
    #[must_use]
    pub fn to_json(&self) -> Json {
        Json::object(vec![
            ("id", Json::text(&self.id)),
            ("title", Json::text(&self.title)),
            ("question", Json::text(&self.question)),
            ("reason", Json::text(&self.reason)),
            (
                "evidence",
                Json::Arr(self.evidence.iter().map(Json::text).collect()),
            ),
            ("priority", Json::text(&self.priority)),
            ("methodology", Json::text(&self.methodology)),
            (
                "variables",
                Json::Arr(self.variables.iter().map(Json::text).collect()),
            ),
            (
                "dependencies",
                Json::Arr(self.dependencies.iter().map(Json::text).collect()),
            ),
            (
                "required_data",
                Json::Arr(self.required_data.iter().map(Json::text).collect()),
            ),
            ("expected_outcomes", Json::text(&self.expected_outcomes)),
            (
                "metrics",
                Json::Arr(self.metrics.iter().map(Json::text).collect()),
            ),
            ("estimated_cost", Json::text(&self.estimated_cost)),
            ("success_criteria", Json::text(&self.success_criteria)),
            ("risks", Json::text(&self.risks)),
            ("status", Json::text(self.status.as_str())),
            (
                "result",
                self.result.as_ref().map_or(Json::Null, Json::text),
            ),
            (
                "follow_up",
                self.follow_up.as_ref().map_or(Json::Null, Json::text),
            ),
        ])
    }
}

/// One recorded piece of evidence the model may cite.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Evidence {
    pub id: String,
    pub kind: String,
    pub detail: String,
}

impl Evidence {
    #[must_use]
    pub fn to_json(&self) -> Json {
        Json::object(vec![
            ("id", Json::text(&self.id)),
            ("kind", Json::text(&self.kind)),
            ("detail", Json::text(&self.detail)),
        ])
    }
}

/// What a change did to a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeKind {
    Created,
    Modified,
    Deleted,
}

impl ChangeKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Modified => "modified",
            Self::Deleted => "deleted",
        }
    }
}

/// One change the agent made inside the workspace, with the identities before and after.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileChange {
    pub path: String,
    pub kind: ChangeKind,
    /// The digest before the change, when there was a file.
    pub before: Option<String>,
    /// The digest after the change, when there is one.
    pub after: Option<String>,
}

impl FileChange {
    #[must_use]
    pub fn to_json(&self) -> Json {
        Json::object(vec![
            ("path", Json::text(&self.path)),
            ("kind", Json::text(self.kind.as_str())),
            (
                "before",
                self.before.as_ref().map_or(Json::Null, Json::text),
            ),
            ("after", self.after.as_ref().map_or(Json::Null, Json::text)),
        ])
    }
}

/// The mutable state of one investigation.
#[derive(Clone, Debug, Default)]
pub struct Investigation {
    pub objective: String,
    pub revisions: Vec<PlanRevision>,
    pub approved_version: Option<u64>,
    pub scope: Option<Scope>,
    pub findings: Vec<Finding>,
    pub experiments: Vec<Experiment>,
    pub evidence: Vec<Evidence>,
    /// The decision the engine is waiting for, if any.
    pub pending_approval: Option<ApprovalRequest>,
    /// The model's closing delivery.
    pub delivery: Option<Delivery>,
    /// Every change the agent made in the workspace, with the digests before and after.
    pub changes: Vec<FileChange>,
    pub turns: u64,
    pub calls: u64,
}

impl Investigation {
    /// Record an evidence item and return its id. Ids are `F<n>` for inspection facts, `I<n>`
    /// for tool invocations, `E<n>` for experiment outcomes.
    pub fn add_evidence(&mut self, kind: &str, detail: impl Into<String>) -> String {
        let prefix = match kind {
            "fact" => "F",
            "invocation" => "I",
            "experiment" => "E",
            _ => "N",
        };
        let count = self
            .evidence
            .iter()
            .filter(|item| item.id.starts_with(prefix))
            .count();
        let id = format!("{prefix}{}", count + 1);
        self.evidence.push(Evidence {
            id: id.clone(),
            kind: kind.to_string(),
            detail: detail.into(),
        });
        id
    }

    #[must_use]
    pub fn has_evidence(&self, id: &str) -> bool {
        self.evidence.iter().any(|item| item.id == id)
    }

    #[must_use]
    pub fn evidence_ids(&self) -> BTreeSet<&str> {
        self.evidence.iter().map(|item| item.id.as_str()).collect()
    }

    /// The tasks of the approved plan, with their current status.
    #[must_use]
    pub fn tasks(&self) -> Vec<&PlannedTask> {
        match self.approved_revision() {
            Some(revision) => revision.proposal.tasks.iter().collect(),
            None => Vec::new(),
        }
    }

    #[must_use]
    pub fn approved_revision(&self) -> Option<&PlanRevision> {
        let version = self.approved_version?;
        self.revisions
            .iter()
            .find(|revision| revision.version == version)
    }

    /// The next experiment id.
    #[must_use]
    pub fn next_experiment_id(&self) -> String {
        let count = self.experiments.len();
        format!("X{}", count + 1)
    }

    /// Add or replace a backlog item, keeping the history of finished work intact.
    pub fn upsert_experiment(&mut self, experiment: Experiment) {
        match self
            .experiments
            .iter_mut()
            .find(|item| item.id == experiment.id)
        {
            Some(existing) => {
                // A finished experiment keeps its outcome when the model only updates its text.
                if experiment.status == ExperimentStatus::Proposed && existing.status.is_finished()
                {
                    let result = existing.result.clone();
                    let follow_up = existing.follow_up.clone();
                    let status = existing.status;
                    *existing = experiment;
                    existing.result = result;
                    existing.follow_up = follow_up;
                    existing.status = status;
                    return;
                }
                *existing = experiment;
            }
            None => self.experiments.push(experiment),
        }
    }

    #[must_use]
    pub fn experiment(&self, id: &str) -> Option<&Experiment> {
        self.experiments.iter().find(|item| item.id == id)
    }

    pub fn experiment_mut(&mut self, id: &str) -> Option<&mut Experiment> {
        self.experiments.iter_mut().find(|item| item.id == id)
    }

    /// The backlog, most valuable first: unfinished before finished, priority before recency.
    #[must_use]
    pub fn ranked_experiments(&self) -> Vec<&Experiment> {
        let mut items: Vec<&Experiment> = self.experiments.iter().collect();
        items.sort_by(|left, right| {
            let finished = left.status.is_finished().cmp(&right.status.is_finished());
            finished
                .then_with(|| priority_rank(&left.priority).cmp(&priority_rank(&right.priority)))
                .then_with(|| left.id.cmp(&right.id))
        });
        items
    }

    #[must_use]
    pub fn to_json(&self) -> Json {
        Json::object(vec![
            ("objective", Json::text(&self.objective)),
            (
                "revisions",
                Json::Arr(self.revisions.iter().map(PlanRevision::to_json).collect()),
            ),
            (
                "approved_version",
                self.approved_version.map_or(Json::Null, Json::count),
            ),
            (
                "scope",
                self.scope.as_ref().map_or(Json::Null, Scope::to_json),
            ),
            (
                "findings",
                Json::Arr(self.findings.iter().map(Finding::to_json).collect()),
            ),
            (
                "experiments",
                Json::Arr(self.experiments.iter().map(Experiment::to_json).collect()),
            ),
            (
                "evidence",
                Json::Arr(self.evidence.iter().map(Evidence::to_json).collect()),
            ),
            (
                "delivery",
                self.delivery.as_ref().map_or(Json::Null, Delivery::to_json),
            ),
            (
                "changes",
                Json::Arr(self.changes.iter().map(FileChange::to_json).collect()),
            ),
            ("turns", Json::count(self.turns)),
            ("calls", Json::count(self.calls)),
        ])
    }
}

fn priority_rank(priority: &str) -> u8 {
    match priority {
        "critical" => 0,
        "high" => 1,
        "recommended" => 2,
        "exploratory" => 3,
        _ => 4,
    }
}

/// The risk a task carries, used to decide whether a revision needs a new approval.
#[must_use]
pub fn proposal_risk(proposal: &Proposal) -> Risk {
    if proposal.scope.allow_exec.is_empty() && !proposal.scope.allow_write {
        Risk::ReadOnly
    } else if proposal.scope.allow_exec.is_empty() {
        Risk::WorkspaceWrite
    } else {
        Risk::Execute
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scope::ScopeLimits;

    fn proposal() -> Proposal {
        Proposal {
            objective: "improve the integration rule".to_string(),
            gaps: vec![Gap {
                id: "G1".to_string(),
                title: "no convergence study".to_string(),
                priority: crate::plan::Priority::High,
                why: "the bias is unmeasured".to_string(),
                condition: "an error estimate exists but no study".to_string(),
                evidence: vec!["F1".to_string()],
            }],
            tasks: vec![PlannedTask {
                id: "T1".to_string(),
                title: "measure the error".to_string(),
                question: "does the error shrink as expected?".to_string(),
                gap_ids: vec!["G1".to_string()],
                depends_on: Vec::new(),
                method: "run the sweep".to_string(),
                expected_evidence: "a table of errors".to_string(),
                success_criteria: "the error falls below 1e-6".to_string(),
                estimated_cost: "one minute".to_string(),
                risk: "low".to_string(),
                status: TaskStatus::Pending,
            }],
            scope: ScopeRequest {
                allow_exec: vec!["python".to_string()],
                allow_write: false,
                max_turns: 8,
                max_tool_calls: 16,
                timeout_ms: 60_000,
            },
            success_criteria: "the error is characterised".to_string(),
        }
    }

    #[test]
    fn evidence_ids_are_unique_per_kind() {
        let mut investigation = Investigation::default();
        assert_eq!(investigation.add_evidence("fact", "a file"), "F1");
        assert_eq!(investigation.add_evidence("fact", "another file"), "F2");
        assert_eq!(investigation.add_evidence("invocation", "a call"), "I1");
        assert!(investigation.has_evidence("F2"));
        assert!(!investigation.has_evidence("F3"));
    }

    #[test]
    fn the_scope_a_request_would_grant_is_checked_against_the_ceilings() {
        let request = proposal().scope;
        let limits = ScopeLimits {
            allow_exec: vec!["python".to_string()],
            allow_write: false,
            max_turns: 8,
            max_tool_calls: 16,
            timeout_ms: 60_000,
        };
        assert!(request.as_scope().within(&limits).is_ok());
        let closed = ScopeLimits {
            allow_exec: Vec::new(),
            ..limits
        };
        assert!(request.as_scope().within(&closed).is_err());
    }

    #[test]
    fn a_finished_experiment_keeps_its_outcome_when_only_its_text_is_updated() {
        let mut investigation = Investigation::default();
        let mut experiment = Experiment {
            id: "X1".to_string(),
            title: "a sweep".to_string(),
            question: "does it converge?".to_string(),
            reason: "never measured".to_string(),
            evidence: vec!["F1".to_string()],
            priority: "high".to_string(),
            methodology: "run it".to_string(),
            variables: Vec::new(),
            dependencies: Vec::new(),
            required_data: Vec::new(),
            expected_outcomes: "smaller error".to_string(),
            metrics: Vec::new(),
            estimated_cost: "a minute".to_string(),
            success_criteria: "error < 1e-6".to_string(),
            risks: "none".to_string(),
            status: ExperimentStatus::Completed,
            result: Some("the error fell to 3e-7".to_string()),
            follow_up: None,
        };
        investigation.upsert_experiment(experiment.clone());
        experiment.status = ExperimentStatus::Proposed;
        experiment.title = "a better worded sweep".to_string();
        experiment.result = None;
        investigation.upsert_experiment(experiment);
        let stored = investigation.experiment("X1").expect("X1");
        assert_eq!(stored.title, "a better worded sweep");
        assert_eq!(
            stored.result.as_deref(),
            Some("the error fell to 3e-7"),
            "a finished experiment lost its outcome"
        );
        assert_eq!(stored.status, ExperimentStatus::Completed);
    }

    #[test]
    fn the_backlog_ranks_unfinished_work_first() {
        let mut investigation = Investigation::default();
        for (id, priority, status) in [
            ("X1", "exploratory", ExperimentStatus::Completed),
            ("X2", "critical", ExperimentStatus::Proposed),
            ("X3", "high", ExperimentStatus::Failed),
        ] {
            investigation.upsert_experiment(Experiment {
                id: id.to_string(),
                title: id.to_string(),
                question: "q".to_string(),
                reason: "r".to_string(),
                evidence: Vec::new(),
                priority: priority.to_string(),
                methodology: "m".to_string(),
                variables: Vec::new(),
                dependencies: Vec::new(),
                required_data: Vec::new(),
                expected_outcomes: "e".to_string(),
                metrics: Vec::new(),
                estimated_cost: "c".to_string(),
                success_criteria: "s".to_string(),
                risks: "none".to_string(),
                status,
                result: None,
                follow_up: None,
            });
        }
        let ranked: Vec<&str> = investigation
            .ranked_experiments()
            .iter()
            .map(|item| item.id.as_str())
            .collect();
        assert_eq!(ranked, vec!["X2", "X3", "X1"]);
    }
}
