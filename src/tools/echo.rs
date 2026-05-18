//! Echo tool — returns its input arguments as output.
//!
//! This is a toy tool used in Phase 2 integration tests to exercise
//! the agent loop's tool execution flow. It echoes back the JSON
//! arguments it receives.

use serde_json::json;

use crate::error::AgentError;
use crate::tools::traits::{Tool, ToolContext, ToolOutput};

/// An echo tool that returns its input arguments as output.
///
/// Useful for testing the agent loop: the LLM can call `echo` to
/// verify that tool arguments are correctly passed through the system.
pub struct EchoTool;

#[async_trait::async_trait]
impl Tool for EchoTool {
    fn name(&self) -> &'static str {
        "echo"
    }

    fn description(&self) -> &'static str {
        "Echo back the input provided by the user or agent. Returns the arguments as a JSON string."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "message": {
                    "type": "string",
                    "description": "The message to echo back."
                }
            },
            "required": ["message"]
        })
    }

    async fn execute(&self, args: serde_json::Value, _ctx: ToolContext) -> Result<ToolOutput, AgentError> {
        let message = args
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        Ok(ToolOutput {
            success: true,
            data: json!({ "echoed": message }),
            summary: format!("Echoed: {message}"),
        })
    }
}
