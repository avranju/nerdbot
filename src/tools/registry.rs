//! Tool registry — holds and dispatches tool calls.

use std::sync::{Arc, RwLock};

use genai::chat::Tool;

use crate::error::AgentError;
use crate::tools::traits::{Tool as ToolTrait, ToolContext, ToolOutput};

#[derive(Clone)]
struct RegisteredTool {
    tool: Arc<dyn ToolTrait>,
    provider: String,
}

/// Shared, append-only registry. Clones share atomic registrations; snapshots do not.
#[derive(Clone)]
pub struct ToolRegistry {
    tools: Arc<RwLock<Vec<RegisteredTool>>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Freeze the available tools for one agent run (including execution dispatch).
    pub fn snapshot(&self) -> Self {
        Self {
            tools: Arc::new(RwLock::new(self.tools.read().unwrap().clone())),
        }
    }

    /// Register a built-in tool with the registry.
    pub fn register(&mut self, tool: impl ToolTrait + 'static) -> Result<(), AgentError> {
        self.register_boxed_from(Arc::new(tool), "builtin")
    }

    /// Register an already boxed tool as a built-in tool.
    pub fn register_boxed(&mut self, tool: Arc<dyn ToolTrait>) -> Result<(), AgentError> {
        self.register_boxed_from(tool, "builtin")
    }

    /// Register a runtime-provided tool with a provider label.
    pub fn register_boxed_from(
        &mut self,
        tool: Arc<dyn ToolTrait>,
        provider: impl Into<String>,
    ) -> Result<(), AgentError> {
        self.register_batch_from(vec![tool], provider)
    }

    /// Register a batch atomically. No tools are appended unless every name is unique.
    pub fn register_batch_from(
        &self,
        tools: Vec<Arc<dyn ToolTrait>>,
        provider: impl Into<String>,
    ) -> Result<(), AgentError> {
        let provider = provider.into();
        let mut registered = self.tools.write().unwrap();
        let mut names = std::collections::HashSet::new();
        for tool in &tools {
            let name = tool.name();
            if let Some(existing) = registered.iter().find(|entry| entry.tool.name() == name) {
                return Err(AgentError::ToolNameCollision {
                    name: name.to_string(),
                    existing_provider: existing.provider.clone(),
                    incoming_provider: provider.clone(),
                });
            }
            if !names.insert(name.to_string()) {
                return Err(AgentError::ToolNameCollision {
                    name: name.to_string(),
                    existing_provider: provider.clone(),
                    incoming_provider: provider.clone(),
                });
            }
        }

        for tool in tools {
            tracing::debug!(tool = tool.name(), provider = %provider, "registering tool");
            registered.push(RegisteredTool {
                tool,
                provider: provider.clone(),
            });
        }
        Ok(())
    }

    /// Get all tools as `genai` specs (for sending to the LLM).
    pub fn specs(&self) -> Vec<Tool> {
        self.tools
            .read()
            .unwrap()
            .iter()
            .map(|entry| {
                Tool::new(entry.tool.name())
                    .with_description(entry.tool.description())
                    .with_schema(entry.tool.input_schema())
            })
            .collect()
    }

    /// Execute a tool call by name.
    pub async fn execute(
        &self,
        call: &genai::chat::ToolCall,
        ctx: ToolContext,
    ) -> Result<ToolOutput, AgentError> {
        // Clone the implementation under the lock; never hold a lock across tool I/O.
        let tool = self
            .tools
            .read()
            .unwrap()
            .iter()
            .find(|entry| entry.tool.name() == call.fn_name)
            .map(|entry| entry.tool.clone())
            .ok_or_else(|| AgentError::ToolNotFound(call.fn_name.clone()))?;

        let args_len = serde_json::to_string(&call.fn_arguments)
            .map(|args| args.len())
            .unwrap_or(0);
        tracing::debug!(
            tool = call.fn_name,
            tool_id = call.call_id,
            args_len,
            "executing tool call"
        );

        tool.execute(call.fn_arguments.clone(), ctx).await
    }

    pub fn len(&self) -> usize {
        self.tools.read().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.read().unwrap().is_empty()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}
