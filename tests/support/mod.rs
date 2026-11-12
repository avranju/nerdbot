//! Test-only tool fixture for exercising dispatch and historical tool replay.
//! This module is not part of the NerdBot library or runtime tool registry.

use nerdbot::error::AgentError;
use nerdbot::tools::traits::{Tool, ToolContext, ToolOutput};
use serde_json::{Value, json};

pub struct EchoTool;

#[async_trait::async_trait]
impl Tool for EchoTool {
    fn name(&self) -> &str {
        "echo"
    }

    fn description(&self) -> &str {
        "Test fixture that returns its arguments."
    }

    fn input_schema(&self) -> Value {
        json!({ "type": "object" })
    }

    async fn execute(&self, args: Value, _ctx: ToolContext) -> Result<ToolOutput, AgentError> {
        Ok(ToolOutput {
            success: true,
            data: args,
            summary: "Test tool executed".into(),
        })
    }
}
