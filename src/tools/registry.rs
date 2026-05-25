//! Tool registry — holds and dispatches tool calls.

use genai::chat::Tool;

use crate::error::AgentError;
use crate::tools::traits::{Tool as ToolTrait, ToolContext, ToolOutput};

/// Registry that holds all available tools.
pub struct ToolRegistry {
    tools: Vec<std::sync::Arc<dyn ToolTrait>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self { tools: Vec::new() }
    }

    /// Register a tool with the registry.
    pub fn register(&mut self, tool: impl ToolTrait + 'static) {
        tracing::debug!(tool = tool.name(), "registering tool");
        self.tools.push(std::sync::Arc::new(tool));
    }

    /// Get all tools as `genai` specs (for sending to the LLM).
    pub fn specs(&self) -> Vec<Tool> {
        self.tools
            .iter()
            .map(|t| {
                Tool::new(t.name())
                    .with_description(t.description())
                    .with_schema(t.input_schema())
            })
            .collect()
    }

    /// Execute a tool call by name.
    pub async fn execute(
        &self,
        call: &genai::chat::ToolCall,
        ctx: ToolContext,
    ) -> Result<ToolOutput, AgentError> {
        let tool = self
            .tools
            .iter()
            .find(|t| t.name() == call.fn_name)
            .ok_or_else(|| AgentError::ToolNotFound(call.fn_name.clone()))?;

        tracing::debug!(
            tool = call.fn_name,
            tool_id = call.call_id,
            args = ?call.fn_arguments,
            "executing tool call"
        );

        tool.execute(call.fn_arguments.clone(), ctx).await
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
