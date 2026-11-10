//! What a tool returns, and how a failure is classified.
//!
//! A tool result has two faces: bounded text for the model, and structured data for the record
//! and the report. Artefacts a tool produced travel as references with digests, so the report can
//! point at bytes without carrying them. A failure carries a class, because "the arguments were
//! wrong", "the user did not permit this" and "the program failed" are different conversations.

use aporia_store::Json;

/// A file or directory a tool produced, identified well enough to find and check it later.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtifactRef {
    pub kind: String,
    /// Relative to the investigation root, with `/` separators.
    pub path: String,
    pub sha256: String,
    pub bytes: u64,
}

impl ArtifactRef {
    #[must_use]
    pub fn to_json(&self) -> Json {
        Json::object(vec![
            ("kind", Json::text(&self.kind)),
            ("path", Json::text(&self.path)),
            ("sha256", Json::text(&self.sha256)),
            ("bytes", Json::count(self.bytes)),
        ])
    }
}

/// A successful tool outcome.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolOutput {
    /// What the model is told, bounded by the tool's output limit.
    pub text: String,
    /// The structured form for the record; `Json::Null` when there is nothing structured.
    pub data: Json,
    pub artifacts: Vec<ArtifactRef>,
}

impl ToolOutput {
    /// Text only: the common case for inspection tools.
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            data: Json::Null,
            artifacts: Vec::new(),
        }
    }

    #[must_use]
    pub fn to_json(&self) -> Json {
        Json::object(vec![
            ("text", Json::text(&self.text)),
            ("data", self.data.clone()),
            (
                "artifacts",
                Json::Arr(self.artifacts.iter().map(ArtifactRef::to_json).collect()),
            ),
        ])
    }
}

/// What kind of thing went wrong. The class is recorded and shown to the model, so it can react
/// appropriately: fix its arguments, ask for permission, or treat the failure as an observation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorClass {
    /// The arguments did not parse or did not match the schema.
    InvalidArguments,
    /// No tool with that name is registered.
    UnknownTool,
    /// The session's approved scope does not permit this operation.
    Denied,
    /// A named file, path or item does not exist.
    NotFound,
    /// The filesystem refused an operation.
    Io,
    /// A program ran and failed, or could not start. The output is still an observation.
    Execution,
    /// A time limit ended the operation.
    Timeout,
    /// A turn, call, output or wall-clock budget ended the operation.
    Budget,
    /// The user cancelled.
    Cancelled,
    /// The operation is recognised but not supported here (a format, a platform capability).
    Unsupported,
    /// The host itself failed; this is not the model's fault and not an observation.
    Internal,
}

impl ErrorClass {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidArguments => "invalid_arguments",
            Self::UnknownTool => "unknown_tool",
            Self::Denied => "denied",
            Self::NotFound => "not_found",
            Self::Io => "io",
            Self::Execution => "execution",
            Self::Timeout => "timeout",
            Self::Budget => "budget",
            Self::Cancelled => "cancelled",
            Self::Unsupported => "unsupported",
            Self::Internal => "internal",
        }
    }
}

/// A classified tool failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolFailure {
    pub class: ErrorClass,
    pub message: String,
}

impl ToolFailure {
    #[must_use]
    pub fn new(class: ErrorClass, message: impl Into<String>) -> Self {
        Self {
            class,
            message: message.into(),
        }
    }

    /// The form the model sees: the class is part of the message, so a refusal is never mistaken
    /// for a result.
    #[must_use]
    pub fn model_text(&self) -> String {
        format!("error[{}]: {}", self.class.as_str(), self.message)
    }
}

impl std::fmt::Display for ToolFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.model_text())
    }
}

impl std::error::Error for ToolFailure {}

/// The JSON field helpers report argument problems as strings; in a tool those are invalid
/// arguments by definition.
impl From<String> for ToolFailure {
    fn from(message: String) -> Self {
        Self::new(ErrorClass::InvalidArguments, message)
    }
}

/// The result of one dispatch.
pub type ToolResult = Result<ToolOutput, ToolFailure>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failure_names_its_class_to_the_model() {
        let failure = ToolFailure::new(ErrorClass::Denied, "curl is not approved");
        assert_eq!(failure.model_text(), "error[denied]: curl is not approved");
    }

    #[test]
    fn an_output_serialises_text_data_and_artifacts() {
        let output = ToolOutput {
            text: "done".to_string(),
            data: Json::object(vec![("exit", Json::count(0))]),
            artifacts: vec![ArtifactRef {
                kind: "results".to_string(),
                path: "workspace/out.json".to_string(),
                sha256: "ab".to_string(),
                bytes: 12,
            }],
        };
        let value = output.to_json();
        let fields = crate::json::fields(&value).expect("object");
        assert_eq!(crate::json::string(fields, "text"), Ok("done"));
        let artifacts = crate::json::array(fields, "artifacts").expect("array");
        assert_eq!(artifacts.len(), 1);
    }
}
