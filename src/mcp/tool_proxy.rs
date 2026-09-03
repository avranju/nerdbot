use rmcp::model::{CallToolRequestParams, CallToolResult, ContentBlock, Tool as RemoteTool};
use serde_json::Value;
use std::sync::Arc;

use crate::config::{MCP_TOOL_NAME_MAX_LEN, is_valid_tool_name_byte};
use crate::error::AgentError;
use crate::tools::traits::{Tool, ToolContext, ToolOutput};

/// One discovered MCP tool exposed through NerdBot's ordinary tool interface.
pub struct McpToolProxy {
    server: Arc<super::client::McpClientHandle>,
    exposed_name: String,
    remote_name: String,
    description: String,
    input_schema: Value,
}

impl McpToolProxy {
    pub fn new(
        server: Arc<super::client::McpClientHandle>,
        remote: RemoteTool,
        exposed_name: String,
    ) -> Result<Self, AgentError> {
        let input_schema = serde_json::to_value(remote.input_schema)
            .map_err(|e| AgentError::Mcp(format!("could not serialize discovered schema: {e}")))?;
        Ok(Self {
            server,
            exposed_name,
            remote_name: remote.name.into_owned(),
            description: remote
                .description
                .map(|d| d.into_owned())
                .unwrap_or_default(),
            input_schema,
        })
    }
}

#[async_trait::async_trait]
impl Tool for McpToolProxy {
    fn name(&self) -> &str {
        &self.exposed_name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn input_schema(&self) -> Value {
        self.input_schema.clone()
    }

    async fn execute(&self, args: Value, _ctx: ToolContext) -> Result<ToolOutput, AgentError> {
        let Value::Object(arguments) = args else {
            return Err(AgentError::InvalidToolArgs(format!(
                "MCP tool {} requires a JSON object",
                self.exposed_name
            )));
        };
        let result = self
            .server
            .call_tool(
                CallToolRequestParams::new(self.remote_name.clone()).with_arguments(arguments),
            )
            .await?;
        map_call_tool_result(&self.remote_name, result)
    }
}

pub fn validate_exposed_tool_name(name: &str) -> Result<(), AgentError> {
    if name.is_empty()
        || name.len() > MCP_TOOL_NAME_MAX_LEN
        || !name.bytes().all(is_valid_tool_name_byte)
    {
        return Err(AgentError::Mcp(format!(
            "invalid exposed MCP tool name {:?}; names must be 1-{} ASCII letters, digits, _ or -",
            name, MCP_TOOL_NAME_MAX_LEN
        )));
    }
    Ok(())
}

pub fn map_call_tool_result(
    _remote_name: &str,
    result: CallToolResult,
) -> Result<ToolOutput, AgentError> {
    let summary = bounded_summary(&result.content);
    let data = match result.structured_content {
        Some(value) => value,
        None => serde_json::to_value(&result.content)
            .map_err(|e| AgentError::Mcp(format!("could not serialize MCP tool result: {e}")))?,
    };
    Ok(ToolOutput {
        success: !result.is_error.unwrap_or(false),
        data,
        summary,
    })
}

fn bounded_summary(content: &[ContentBlock]) -> String {
    let mut summary = String::new();
    for block in content {
        let text = match block {
            ContentBlock::Text(text) => text.text.clone(),
            ContentBlock::Resource(resource) => resource.get_text(),
            _ => String::new(),
        };
        if !text.is_empty() {
            if !summary.is_empty() {
                summary.push('\n');
            }
            summary.push_str(&text);
        }
    }
    summary.chars().take(4096).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structured_result_is_preferred_and_errors_are_preserved() {
        let output = map_call_tool_result(
            "remote",
            rmcp::model::CallToolResult::structured(serde_json::json!({"answer": 42})),
        )
        .unwrap();
        assert!(output.success);
        assert_eq!(output.data["answer"], 42);

        let output = map_call_tool_result(
            "remote",
            rmcp::model::CallToolResult::error(vec![ContentBlock::text("failed")]),
        )
        .unwrap();
        assert!(!output.success);
        assert_eq!(output.summary, "failed");
    }

    #[test]
    fn exposed_names_use_the_provider_safe_policy() {
        assert!(validate_exposed_tool_name("mail_search").is_ok());
        assert!(validate_exposed_tool_name("bad name").is_err());
        assert!(validate_exposed_tool_name(&"x".repeat(65)).is_err());
    }
}
