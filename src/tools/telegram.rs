//! User messaging tool — send_user_message.
//!
//! Currently supports Telegram; other channels (Slack, etc.) can be added later.

use serde_json::json;

use crate::error::AgentError;
use crate::tools::traits::{Tool, ToolContext, ToolOutput};

pub struct SendTelegramMessage;

#[async_trait::async_trait]
impl Tool for SendTelegramMessage {
    fn name(&self) -> &'static str { "send_user_message" }
    fn description(&self) -> &'static str { "Send a message to a Telegram chat." }
    fn input_schema(&self) -> serde_json::Value { json!({}) }
    async fn execute(&self, _args: serde_json::Value, _ctx: ToolContext) -> Result<ToolOutput, AgentError> {
        Err(AgentError::Generic("send_user_message not yet implemented".into()))
    }
}
