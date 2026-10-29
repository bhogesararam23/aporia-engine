//! The model-provider boundary.
//!
//! Everything above this module speaks one small transcript type. Everything below it is a
//! particular way of getting a reply: a committed transcript for tests and the offline demo, or an
//! OpenAI-compatible HTTP endpoint for a model that is actually running. Replacing the model must
//! not require touching the orchestrator, the tools or the record.

pub mod http;
pub mod scripted;

use aporia_store::Json;
use std::fmt;

/// Who said it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

impl Role {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::Tool => "tool",
        }
    }
}

/// A tool the model may call, in the shape the provider boundary carries.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolSchema {
    pub name: String,
    pub description: String,
    pub parameters: Json,
}

/// One request from the model's side of the conversation.
#[derive(Clone, Debug, PartialEq)]
pub struct Message {
    pub role: Role,
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    /// Set on a tool result: the id of the call it answers.
    pub tool_call_id: Option<String>,
}

impl Message {
    #[must_use]
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    #[must_use]
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    #[must_use]
    pub fn assistant(content: impl Into<String>, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
            tool_calls,
            tool_call_id: None,
        }
    }

    #[must_use]
    pub fn tool(call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: Some(call_id.into()),
        }
    }
}

/// One call the model asked for. `arguments` is the raw JSON text, so a provider cannot quietly
/// re-type an argument on its way to the validator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

/// What the model was asked.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelRequest {
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSchema>,
}

/// What the model answered.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelResponse {
    pub content: Option<String>,
    pub tool_calls: Vec<ToolCall>,
}

impl ModelResponse {
    #[must_use]
    pub fn text(content: impl Into<String>) -> Self {
        Self {
            content: Some(content.into()),
            tool_calls: Vec::new(),
        }
    }

    #[must_use]
    pub fn calls(calls: Vec<ToolCall>) -> Self {
        Self {
            content: None,
            tool_calls: calls,
        }
    }
}

/// Why the boundary could not produce an answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderError {
    /// A scripted transcript ran out. Never turned into a fabricated reply.
    Exhausted,
    /// The provider's answer was not what the boundary promised.
    Malformed(String),
    /// The endpoint could not be reached at all.
    Unreachable(String),
    /// The endpoint answered with a non-success status.
    Status(u16, String),
    /// No answer within the provider's time limit.
    Timeout,
    /// An answer larger than the boundary will carry.
    Limit(String),
}

impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Exhausted => write!(f, "the scripted transcript has no further responses"),
            Self::Malformed(reason) => write!(f, "the provider's answer is not usable: {reason}"),
            Self::Unreachable(reason) => {
                write!(f, "the provider endpoint is unreachable: {reason}")
            }
            Self::Status(code, body) => {
                write!(f, "the provider answered {code}: {body}")
            }
            Self::Timeout => write!(f, "the provider did not answer in time"),
            Self::Limit(reason) => write!(f, "the provider's answer exceeds a limit: {reason}"),
        }
    }
}

impl std::error::Error for ProviderError {}

/// The one seam between the application and a language model.
pub trait ModelProvider {
    fn complete(&mut self, request: &ModelRequest) -> Result<ModelResponse, ProviderError>;

    /// A stable description for the record: which provider, and which transcript or endpoint.
    fn describe(&self) -> String;
}
