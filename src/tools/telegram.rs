//! Telegram messaging tool — send_telegram_message.

use serde_json::json;

use crate::error::AgentError;
use crate::tools::traits::{Tool, ToolContext, ToolOutput};

pub struct SendTelegramMessage;

impl Tool for SendTelegramMessage {
    fn name(&self) -> &'static str { "send_telegram_message" }
    fn description(&self) -> &'static str { "Send a message to a Telegram chat." }
    fn input_schema(&self) -> serde_json::Value { json!({}) }
    fn execute(&self, _args: serde_json::Value, _ctx: ToolContext) -> Result<ToolOutput, AgentError> {
        Err(AgentError::Generic("send_telegram_message not yet implemented".into()))
    }
}
