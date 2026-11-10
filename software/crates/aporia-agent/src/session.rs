//! The persistent investigation: identity, phase, status and the versioned checkpoint.
//!
//! The v0.2 engine keeps its state in a session directory whose journal is authoritative. The
//! checkpoint written here is a derived summary: it can be rebuilt from the journal, and a reader
//! who finds a checkpoint it cannot parse is told the version rather than handed a guess. Old
//! records stay readable because the schema version is an explicit field, never an inference.

use crate::json;
use aporia_store::Json;

/// The version of the `session.json` contract. Bump it when a field's meaning changes; a reader
/// refuses a version it does not know instead of misreading it.
pub const SESSION_SCHEMA: u64 = 1;

/// Where the investigation is in its lifecycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Read-only inspection and planning; nothing has been approved for execution.
    Inspecting,
    /// A plan and scope are approved; the agent may use the permitted tools.
    Working,
    /// The investigation reached an end state; no further work happens.
    Finished,
}

impl Phase {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Inspecting => "inspecting",
            Self::Working => "working",
            Self::Finished => "finished",
        }
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        match text {
            "inspecting" => Ok(Self::Inspecting),
            "working" => Ok(Self::Working),
            "finished" => Ok(Self::Finished),
            other => Err(format!("unknown session phase `{other}`")),
        }
    }
}

/// The session's disposition. Distinct from the phase: a paused session is still `Working`, and a
/// cancelled one is `Finished` with a status that says why.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionStatus {
    /// The session can continue.
    Active,
    /// The user paused it; resume continues from the recorded state.
    Paused,
    /// The model delivered a finished investigation.
    Completed,
    /// The model decided, before planning, that no investigation was warranted.
    NoPlan,
    /// The user refused the plan or a required approval.
    Refused,
    /// The host or provider failed; the record says what.
    Failed,
    /// A turn, call or wall-clock budget ran out; the record is incomplete.
    BudgetExhausted,
    /// The user cancelled. Nothing further runs; the record keeps what happened.
    Cancelled,
}

impl SessionStatus {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Completed => "completed",
            Self::NoPlan => "no_plan",
            Self::Refused => "refused",
            Self::Failed => "failed",
            Self::BudgetExhausted => "budget_exhausted",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        match text {
            "active" => Ok(Self::Active),
            "paused" => Ok(Self::Paused),
            "completed" => Ok(Self::Completed),
            "no_plan" => Ok(Self::NoPlan),
            "refused" => Ok(Self::Refused),
            "failed" => Ok(Self::Failed),
            "budget_exhausted" => Ok(Self::BudgetExhausted),
            "cancelled" => Ok(Self::Cancelled),
            other => Err(format!("unknown session status `{other}`")),
        }
    }

    /// Whether the session can be resumed. A finished session is history, not work in progress.
    #[must_use]
    pub fn is_open(self) -> bool {
        matches!(self, Self::Active | Self::Paused)
    }
}

/// The checkpoint summary of one investigation. Everything here is also derivable from the
/// journal; the checkpoint exists so `list` and `status` do not replay every event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionState {
    pub schema: u64,
    pub id: String,
    pub phase: Phase,
    pub status: SessionStatus,
    pub objective: String,
    pub source: String,
    pub provider: String,
    pub started: String,
    pub updated: String,
    pub turns: u64,
    pub calls: u64,
    /// The highest plan version the model proposed; 0 means none.
    pub plan_version: u64,
    /// The highest plan version a person approved; 0 means none.
    pub approved_version: u64,
}

impl SessionState {
    #[must_use]
    pub fn to_json(&self) -> Json {
        Json::object(vec![
            ("schema", Json::count(self.schema)),
            ("id", Json::text(&self.id)),
            ("phase", Json::text(self.phase.as_str())),
            ("status", Json::text(self.status.as_str())),
            ("objective", Json::text(&self.objective)),
            ("source", Json::text(&self.source)),
            ("provider", Json::text(&self.provider)),
            ("started", Json::text(&self.started)),
            ("updated", Json::text(&self.updated)),
            ("turns", Json::count(self.turns)),
            ("calls", Json::count(self.calls)),
            ("plan_version", Json::count(self.plan_version)),
            ("approved_version", Json::count(self.approved_version)),
        ])
    }

    pub fn from_json(value: &Json) -> Result<Self, String> {
        let fields = json::fields(value)
            .ok_or_else(|| "the session checkpoint must be an object".to_string())?;
        json::only(
            fields,
            &[
                "schema",
                "id",
                "phase",
                "status",
                "objective",
                "source",
                "provider",
                "started",
                "updated",
                "turns",
                "calls",
                "plan_version",
                "approved_version",
            ],
        )?;
        let schema = json::u64_value(fields, "schema")?;
        if schema != SESSION_SCHEMA {
            return Err(format!(
                "the session checkpoint is schema {schema}, and this build reads schema {SESSION_SCHEMA}"
            ));
        }
        Ok(Self {
            schema,
            id: json::string(fields, "id")?.to_string(),
            phase: Phase::parse(json::string(fields, "phase")?)?,
            status: SessionStatus::parse(json::string(fields, "status")?)?,
            objective: json::string(fields, "objective")?.to_string(),
            source: json::string(fields, "source")?.to_string(),
            provider: json::string(fields, "provider")?.to_string(),
            started: json::string(fields, "started")?.to_string(),
            updated: json::string(fields, "updated")?.to_string(),
            turns: json::u64_value(fields, "turns")?,
            calls: json::u64_value(fields, "calls")?,
            plan_version: json::u64_value(fields, "plan_version")?,
            approved_version: json::u64_value(fields, "approved_version")?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> SessionState {
        SessionState {
            schema: SESSION_SCHEMA,
            id: "inv-1700000000".to_string(),
            phase: Phase::Working,
            status: SessionStatus::Active,
            objective: "find the missing convergence study".to_string(),
            source: "example/project".to_string(),
            provider: "scripted transcript (in memory)".to_string(),
            started: "2026-11-10T00:00:00Z".to_string(),
            updated: "2026-11-10T00:01:00Z".to_string(),
            turns: 3,
            calls: 7,
            plan_version: 2,
            approved_version: 1,
        }
    }

    #[test]
    fn a_checkpoint_survives_a_json_round_trip() {
        let original = state();
        let text = original.to_json().to_compact();
        let parsed = SessionState::from_json(&Json::parse(&text).expect("json")).expect("parse");
        assert_eq!(parsed, original);
    }

    #[test]
    fn a_future_schema_is_refused_by_version_not_misread() {
        let mut value = state().to_json();
        if let Json::Obj(fields) = &mut value {
            for (name, field) in fields.iter_mut() {
                if name == "schema" {
                    *field = Json::count(SESSION_SCHEMA + 1);
                }
            }
        }
        let error = SessionState::from_json(&value).expect_err("future schema");
        assert!(error.contains("schema"), "{error}");
    }

    #[test]
    fn an_unknown_field_is_a_refusal() {
        let mut value = state().to_json();
        if let Json::Obj(fields) = &mut value {
            fields.push(("surprise".to_string(), Json::Bool(true)));
        }
        assert!(SessionState::from_json(&value).is_err());
    }

    #[test]
    fn only_open_statuses_resume() {
        assert!(SessionStatus::Active.is_open());
        assert!(SessionStatus::Paused.is_open());
        for status in [
            SessionStatus::Completed,
            SessionStatus::NoPlan,
            SessionStatus::Refused,
            SessionStatus::Failed,
            SessionStatus::BudgetExhausted,
            SessionStatus::Cancelled,
        ] {
            assert!(!status.is_open(), "{} must not resume", status.as_str());
        }
    }

    #[test]
    fn every_phase_and_status_parses_back() {
        for phase in [Phase::Inspecting, Phase::Working, Phase::Finished] {
            assert_eq!(Phase::parse(phase.as_str()), Ok(phase));
        }
        for status in [
            SessionStatus::Active,
            SessionStatus::Paused,
            SessionStatus::Completed,
            SessionStatus::NoPlan,
            SessionStatus::Refused,
            SessionStatus::Failed,
            SessionStatus::BudgetExhausted,
            SessionStatus::Cancelled,
        ] {
            assert_eq!(SessionStatus::parse(status.as_str()), Ok(status));
        }
        assert!(Phase::parse("sleeping").is_err());
        assert!(SessionStatus::parse("sleeping").is_err());
    }
}
