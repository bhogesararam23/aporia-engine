//! What every tool declares about itself.
//!
//! The metadata is the single source the provider schemas are generated from, so the model cannot
//! be offered a shape the host does not validate. Schemas are written in the JSON-Schema subset
//! [`super::schema`] understands; the built-in literals are checked by the registry's contract
//! tests, not trusted because they are literals.

use crate::provider::ToolSchema;
use crate::scope::Risk;
use aporia_store::Json;

/// A tool's stable identity, contract and limits.
#[derive(Clone, Debug)]
pub struct ToolMeta {
    pub name: &'static str,
    pub description: &'static str,
    /// The argument schema, in the validated subset.
    pub parameters: Json,
    pub risk: Risk,
    /// The most text this tool may return to the model in one call.
    pub max_output_bytes: usize,
}

impl ToolMeta {
    /// Build metadata from a schema literal. The literal is parsed once, here; a malformed
    /// built-in schema is a programming error the contract tests catch before any session runs.
    #[must_use]
    pub fn new(
        name: &'static str,
        description: &'static str,
        parameters: &str,
        risk: Risk,
        max_output_bytes: usize,
    ) -> Self {
        Self {
            name,
            description,
            parameters: Json::parse(parameters).expect("a built-in tool schema must be valid JSON"),
            risk,
            max_output_bytes,
        }
    }

    /// The schema the provider boundary carries, generated from this metadata.
    #[must_use]
    pub fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: self.name.to_string(),
            description: self.description.to_string(),
            parameters: self.parameters.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_provider_schema_is_generated_from_the_metadata() {
        let meta = ToolMeta::new(
            "example",
            "an example tool",
            r#"{"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false}"#,
            Risk::ReadOnly,
            4096,
        );
        let schema = meta.schema();
        assert_eq!(schema.name, "example");
        assert_eq!(schema.description, "an example tool");
        assert_eq!(schema.parameters, meta.parameters);
    }
}
