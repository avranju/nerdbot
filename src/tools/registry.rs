//! Tool registry — holds and dispatches tool calls.

use crate::error::AgentError;
use crate::llm::types::{ToolCall, ToolSpec};
use crate::tools::traits::{Tool, ToolContext, ToolOutput};

/// Registry that holds all available tools.
pub struct ToolRegistry {
    tools: Vec<std::sync::Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self { tools: Vec::new() }
    }

    /// Register a tool with the registry.
    pub fn register(&mut self, tool: impl Tool + 'static) {
        tracing::debug!(tool = tool.name(), "registering tool");
        self.tools.push(std::sync::Arc::new(tool));
    }

    /// Get all tools as specs (for sending to the LLM).
    pub fn specs(&self) -> Vec<ToolSpec> {
        self.tools
            .iter()
            .map(|t| ToolSpec::new(t.name(), t.description(), t.input_schema()))
            .collect()
    }

    /// Execute a tool call by name.
    pub async fn execute(
        &self,
        call: &ToolCall,
        ctx: ToolContext,
    ) -> Result<ToolOutput, AgentError> {
        let tool = self
            .tools
            .iter()
            .find(|t| t.name() == call.name)
            .ok_or_else(|| AgentError::ToolNotFound(call.name.clone()))?;

        tracing::debug!(
            tool = call.name,
            tool_id = call.id,
            args = ?call.arguments,
            "executing tool call"
        );

        tool.execute(call.arguments.clone(), ctx).await
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}
