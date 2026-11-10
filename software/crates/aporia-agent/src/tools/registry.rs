//! The host-owned collection of tools.
//!
//! Registration is the only way a capability becomes callable: the registry rejects duplicate
//! names, generates the provider schemas from the registered metadata, and is what dispatch
//! consults. Nothing elsewhere in the engine matches on tool names to decide what exists.

use super::Tool;
use crate::provider::ToolSchema;

/// The tools one session offers.
pub struct Registry {
    tools: Vec<Box<dyn Tool>>,
}

impl std::fmt::Debug for Registry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Registry")
            .field("tools", &self.names())
            .finish()
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

impl Registry {
    #[must_use]
    pub fn new() -> Self {
        Self { tools: Vec::new() }
    }

    /// Register a tool. A duplicate name is a refusal: two tools under one name would make the
    /// schema offered to the model a lie about what runs.
    pub fn register(&mut self, tool: Box<dyn Tool>) -> Result<(), String> {
        let name = tool.meta().name;
        if self.get(name).is_some() {
            return Err(format!("a tool named `{name}` is already registered"));
        }
        self.tools.push(tool);
        Ok(())
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools
            .iter()
            .find(|tool| tool.meta().name == name)
            .map(Box::as_ref)
    }

    /// The schemas the model receives, generated from the registered metadata in registration
    /// order (a stable order keeps transcripts and records comparable).
    #[must_use]
    pub fn schemas(&self) -> Vec<ToolSchema> {
        self.tools.iter().map(|tool| tool.meta().schema()).collect()
    }

    #[must_use]
    pub fn names(&self) -> Vec<&'static str> {
        self.tools.iter().map(|tool| tool.meta().name).collect()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}
