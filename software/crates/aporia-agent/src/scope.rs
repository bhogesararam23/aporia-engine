//! What the agent may do, and who said so.
//!
//! Every tool carries a risk class; the approved [`Scope`] says which classes are permitted and
//! with which limits. Read-only inspection never asks. Execution needs a named program in the
//! approved scope; editing needs workspace write permission. A plan revision that expands the
//! scope is detected here, as a list the engine can show a person -- never applied silently.
//!
//! The user's configuration sets hard ceilings ([`ScopeLimits`]); an approval can grant less than
//! the ceiling, never more. That keeps a batch run's permissions exactly what the command line
//! said, and an interactive approval honest about what it can grant.

use crate::json;
use aporia_store::Json;

/// What a tool can do to the world, in increasing order of consequence.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Risk {
    /// Reads the workspace or the record. Always permitted once a session exists.
    ReadOnly,
    /// Creates, modifies or deletes files inside the sandbox copy.
    WorkspaceWrite,
    /// Starts a process. The program must be named in the approved scope.
    Execute,
}

impl Risk {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read_only",
            Self::WorkspaceWrite => "workspace_write",
            Self::Execute => "execute",
        }
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        match text {
            "read_only" => Ok(Self::ReadOnly),
            "workspace_write" => Ok(Self::WorkspaceWrite),
            "execute" => Ok(Self::Execute),
            other => Err(format!("unknown risk class `{other}`")),
        }
    }
}

/// The permissions a person approved for one plan version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scope {
    /// Bare program names an execution tool may start.
    pub allow_exec: Vec<String>,
    /// Whether patch tools may change the sandbox copy.
    pub allow_write: bool,
    pub max_turns: u64,
    pub max_tool_calls: u64,
    pub timeout_ms: u64,
}

impl Scope {
    #[must_use]
    pub fn to_json(&self) -> Json {
        Json::object(vec![
            (
                "allow_exec",
                Json::Arr(self.allow_exec.iter().map(Json::text).collect()),
            ),
            ("allow_write", Json::Bool(self.allow_write)),
            ("max_turns", Json::count(self.max_turns)),
            ("max_tool_calls", Json::count(self.max_tool_calls)),
            ("timeout_ms", Json::count(self.timeout_ms)),
        ])
    }

    pub fn from_json(value: &Json) -> Result<Self, String> {
        let fields =
            json::fields(value).ok_or_else(|| "the scope must be an object".to_string())?;
        json::only(
            fields,
            &[
                "allow_exec",
                "allow_write",
                "max_turns",
                "max_tool_calls",
                "timeout_ms",
            ],
        )?;
        let allow_exec = json::string_array(fields, "allow_exec")?;
        for program in &allow_exec {
            validate_program(program)?;
        }
        let allow_write = json::required(fields, "allow_write")?
            .as_bool()
            .ok_or_else(|| "field `allow_write` must be a boolean".to_string())?;
        Ok(Self {
            allow_exec,
            allow_write,
            max_turns: json::u64_value(fields, "max_turns")?,
            max_tool_calls: json::u64_value(fields, "max_tool_calls")?,
            timeout_ms: json::u64_value(fields, "timeout_ms")?,
        })
    }

    /// Whether this scope may run `program`. Windows names are case-insensitive, matching the
    /// platform's own resolution; elsewhere they are exact.
    #[must_use]
    pub fn permits_exec(&self, program: &str) -> bool {
        self.allow_exec
            .iter()
            .any(|allowed| program_eq(allowed, program))
    }

    /// Check the scope against the user's hard ceilings. An approval cannot grant more than the
    /// command line permitted.
    pub fn within(&self, limits: &ScopeLimits) -> Result<(), String> {
        for program in &self.allow_exec {
            if !limits
                .allow_exec
                .iter()
                .any(|allowed| program_eq(allowed, program))
            {
                return Err(format!(
                    "the plan asks to run `{program}`, which this session was not configured to allow; start with --allow-exec {program} to permit it"
                ));
            }
        }
        if self.allow_write && !limits.allow_write {
            return Err(
                "the plan asks to edit the workspace, but this session was started without --allow-write"
                    .to_string(),
            );
        }
        if self.max_turns > limits.max_turns {
            return Err(format!(
                "the plan asks for {} turns; the session ceiling is {}",
                self.max_turns, limits.max_turns
            ));
        }
        if self.max_tool_calls > limits.max_tool_calls {
            return Err(format!(
                "the plan asks for {} tool calls; the session ceiling is {}",
                self.max_tool_calls, limits.max_tool_calls
            ));
        }
        if self.timeout_ms > limits.timeout_ms {
            return Err(format!(
                "the plan asks for {} ms; the session ceiling is {} ms",
                self.timeout_ms, limits.timeout_ms
            ));
        }
        Ok(())
    }
}

/// The hard ceilings the user configured. A scope is valid only inside them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScopeLimits {
    pub allow_exec: Vec<String>,
    pub allow_write: bool,
    pub max_turns: u64,
    pub max_tool_calls: u64,
    pub timeout_ms: u64,
}

/// One way a new scope asks for more than the approved one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expansion {
    Executable(String),
    WriteAccess,
    Turns { from: u64, to: u64 },
    ToolCalls { from: u64, to: u64 },
    WallClock { from: u64, to: u64 },
}

impl Expansion {
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Executable(program) => format!("run a new program: {program}"),
            Self::WriteAccess => "edit files in the workspace".to_string(),
            Self::Turns { from, to } => format!("raise the turn budget from {from} to {to}"),
            Self::ToolCalls { from, to } => {
                format!("raise the tool-call budget from {from} to {to}")
            }
            Self::WallClock { from, to } => {
                format!("raise the wall-clock budget from {from} ms to {to} ms")
            }
        }
    }
}

/// Everything the new scope asks for beyond the old one. A reduction is not an expansion: giving
/// permissions back never needs asking.
#[must_use]
pub fn expansions(old: &Scope, new: &Scope) -> Vec<Expansion> {
    let mut out = Vec::new();
    for program in &new.allow_exec {
        if !old.permits_exec(program) {
            out.push(Expansion::Executable(program.clone()));
        }
    }
    if new.allow_write && !old.allow_write {
        out.push(Expansion::WriteAccess);
    }
    if new.max_turns > old.max_turns {
        out.push(Expansion::Turns {
            from: old.max_turns,
            to: new.max_turns,
        });
    }
    if new.max_tool_calls > old.max_tool_calls {
        out.push(Expansion::ToolCalls {
            from: old.max_tool_calls,
            to: new.max_tool_calls,
        });
    }
    if new.timeout_ms > old.timeout_ms {
        out.push(Expansion::WallClock {
            from: old.timeout_ms,
            to: new.timeout_ms,
        });
    }
    out
}

/// What kind of decision a person is being asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApprovalKind {
    /// The first plan, or a revision that does not expand the scope.
    Plan,
    /// A revision that materially expands the approved scope or changes the objective.
    ScopeExpansion,
    /// Writing a changeset or deliverable outside the investigation record.
    ChangesetExport,
}

impl ApprovalKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Plan => "plan",
            Self::ScopeExpansion => "scope_expansion",
            Self::ChangesetExport => "changeset_export",
        }
    }
}

/// One question put to the user, with enough detail to decide it.
#[derive(Clone, Debug, PartialEq)]
pub struct ApprovalRequest {
    pub kind: ApprovalKind,
    pub title: String,
    pub detail: Json,
}

/// The answer. There is no default: an unanswered request pauses the session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Approve,
    Refuse,
}

impl Decision {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Approve => "approve",
            Self::Refuse => "refuse",
        }
    }
}

fn program_eq(left: &str, right: &str) -> bool {
    if cfg!(windows) {
        left.eq_ignore_ascii_case(right)
    } else {
        left == right
    }
}

/// A bare program name: no path, no space, no NUL. The same rule the first slice's `--allow-exec`
/// uses, so a scope cannot smuggle a shell or a path into the executable slot.
pub fn validate_program(program: &str) -> Result<(), String> {
    if program.is_empty()
        || program.contains('/')
        || program.contains('\\')
        || program.contains(' ')
        || program.contains('\0')
    {
        return Err(format!(
            "`{program}` is not a bare program name; paths, spaces and NUL are refused"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope() -> Scope {
        Scope {
            allow_exec: vec!["python".to_string()],
            allow_write: false,
            max_turns: 16,
            max_tool_calls: 64,
            timeout_ms: 300_000,
        }
    }

    fn limits() -> ScopeLimits {
        ScopeLimits {
            allow_exec: vec!["python".to_string(), "python3".to_string()],
            allow_write: true,
            max_turns: 32,
            max_tool_calls: 128,
            timeout_ms: 600_000,
        }
    }

    #[test]
    fn a_scope_round_trips_through_json() {
        let original = scope();
        let parsed = Scope::from_json(&original.to_json()).expect("parse");
        assert_eq!(parsed, original);
    }

    #[test]
    fn a_scope_outside_the_ceilings_is_refused_with_the_reason() {
        let mut asked = scope();
        asked.allow_exec.push("curl".to_string());
        let error = asked.within(&limits()).expect_err("curl is not configured");
        assert!(error.contains("curl"), "{error}");

        let mut asked = scope();
        asked.allow_write = true;
        let mut closed = limits();
        closed.allow_write = false;
        assert!(asked.within(&closed).is_err());

        let mut asked = scope();
        asked.max_turns = 64;
        assert!(asked.within(&limits()).is_err());
        assert!(scope().within(&limits()).is_ok());
    }

    #[test]
    fn expansions_name_every_new_permission() {
        let old = scope();
        let mut new = scope();
        new.allow_exec.push("python3".to_string());
        new.allow_write = true;
        new.max_turns = 20;
        new.max_tool_calls = 100;
        new.timeout_ms = 400_000;
        let found = expansions(&old, &new);
        assert_eq!(found.len(), 5, "{found:?}");
        assert!(found.contains(&Expansion::Executable("python3".to_string())));
        assert!(found.contains(&Expansion::WriteAccess));
    }

    #[test]
    fn giving_permissions_back_is_not_an_expansion() {
        let mut old = scope();
        old.allow_write = true;
        old.max_turns = 20;
        let new = scope();
        assert!(expansions(&old, &new).is_empty());
        assert!(expansions(&new, &new).is_empty());
    }

    #[test]
    fn program_names_are_bare() {
        assert!(validate_program("python").is_ok());
        for bad in ["", "/bin/sh", "a b", "c\\d", "e\0f"] {
            assert!(validate_program(bad).is_err(), "{bad:?} was accepted");
        }
    }

    #[test]
    fn exec_permission_matches_how_the_platform_resolves_names() {
        let permitted = scope();
        if cfg!(windows) {
            assert!(permitted.permits_exec("PYTHON"));
        } else {
            assert!(!permitted.permits_exec("PYTHON"));
        }
        assert!(!permitted.permits_exec("curl"));
    }

    #[test]
    fn risk_classes_parse_back() {
        for risk in [Risk::ReadOnly, Risk::WorkspaceWrite, Risk::Execute] {
            assert_eq!(Risk::parse(risk.as_str()), Ok(risk));
        }
        assert!(Risk::parse("root").is_err());
        assert!(Risk::ReadOnly < Risk::WorkspaceWrite);
        assert!(Risk::WorkspaceWrite < Risk::Execute);
    }
}
