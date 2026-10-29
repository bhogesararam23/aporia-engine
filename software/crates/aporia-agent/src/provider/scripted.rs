//! A transcript, consumed one response per turn.
//!
//! This is a test double and the offline demo's provider. It is not a model and the record says so:
//! `describe` carries the transcript's digest so a reader can see exactly which script produced the
//! conversation. Running out is an error, never a plausible-sounding answer.

use super::{ModelProvider, ModelRequest, ModelResponse, ProviderError, ToolCall};
use crate::json;
use aporia_store::Json;
use aporia_store::digest::sha256_hex;
use std::path::Path;

/// A provider backed by a committed sequence of responses.
#[derive(Debug)]
pub struct ScriptedProvider {
    responses: Vec<ModelResponse>,
    next: usize,
    label: String,
}

impl ScriptedProvider {
    /// Read a transcript:
    /// `{"note": "...", "responses": [{"content": "...", "tool_calls": [...]}]}`.
    pub fn load(path: &Path) -> Result<Self, ProviderError> {
        let text = std::fs::read_to_string(path).map_err(|error| {
            ProviderError::Malformed(format!("cannot read {}: {error}", path.display()))
        })?;
        let document = Json::parse(&text)
            .map_err(|error| ProviderError::Malformed(format!("{}: {error}", path.display())))?;
        let fields = json::fields(&document).ok_or_else(|| {
            ProviderError::Malformed("the transcript must be a JSON object".to_string())
        })?;
        let responses = json::array(fields, "responses")
            .map_err(ProviderError::Malformed)?
            .iter()
            .enumerate()
            .map(|(index, value)| {
                parse_response(value).map_err(|reason| {
                    ProviderError::Malformed(format!("response {index}: {reason}"))
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            responses,
            next: 0,
            label: format!(
                "scripted transcript {} (sha256 {})",
                path.display(),
                sha256_hex(text.as_bytes())
            ),
        })
    }

    /// Build one in memory, for tests that do not need a file.
    #[must_use]
    pub fn from_responses(responses: Vec<ModelResponse>) -> Self {
        Self {
            responses,
            next: 0,
            label: "scripted transcript (in memory)".to_string(),
        }
    }
}

impl ModelProvider for ScriptedProvider {
    fn complete(&mut self, _request: &ModelRequest) -> Result<ModelResponse, ProviderError> {
        let response = self
            .responses
            .get(self.next)
            .cloned()
            .ok_or(ProviderError::Exhausted)?;
        self.next += 1;
        Ok(response)
    }

    fn describe(&self) -> String {
        self.label.clone()
    }
}

fn parse_response(value: &Json) -> Result<ModelResponse, String> {
    let fields = json::fields(value).ok_or_else(|| "a response must be an object".to_string())?;
    json::only(fields, &["content", "tool_calls"])?;
    let content = json::optional_string(fields, "content")?.map(str::to_string);
    let tool_calls = match json::field(fields, "tool_calls") {
        None => Vec::new(),
        Some(value) => {
            let array = value
                .as_array()
                .ok_or_else(|| "field `tool_calls` must be an array".to_string())?;
            array
                .iter()
                .enumerate()
                .map(|(index, call)| {
                    parse_call(call).map_err(|reason| format!("tool_calls[{index}]: {reason}"))
                })
                .collect::<Result<Vec<_>, _>>()?
        }
    };
    if content.as_deref().is_none_or(str::is_empty) && tool_calls.is_empty() {
        return Err("a response must carry text or at least one tool call".to_string());
    }
    Ok(ModelResponse {
        content,
        tool_calls,
    })
}

fn parse_call(value: &Json) -> Result<ToolCall, String> {
    let fields = json::fields(value).ok_or_else(|| "a tool call must be an object".to_string())?;
    json::only(fields, &["id", "name", "arguments"])?;
    let id = json::string(fields, "id")?.to_string();
    let name = json::string(fields, "name")?.to_string();
    let arguments = match json::required(fields, "arguments")? {
        Json::Str(text) => text.clone(),
        value if json::fields(value).is_some() => value.to_compact(),
        _ => return Err("field `arguments` must be a JSON object or a JSON string".to_string()),
    };
    Ok(ToolCall {
        id,
        name,
        arguments,
    })
}
