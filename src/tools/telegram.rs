//! User messaging tool — send_user_message.
//!
//! Sends messages to Telegram chats through the TelegramService abstraction.
//! Supports optional formatting (MarkdownV2) and notification suppression.
//! Enforces allowlist policies to prevent unauthorized messaging.

use serde_json::json;

use crate::error::AgentError;
use crate::tools::traits::{Tool, ToolContext, ToolOutput};

pub struct SendTelegramMessage;

#[async_trait::async_trait]
impl Tool for SendTelegramMessage {
    fn name(&self) -> &'static str {
        "send_user_message"
    }

    fn description(&self) -> &'static str {
        "Send a message to a Telegram chat. Use this for scheduled job notifications, \
         conditional alerts, intermediate updates, or when you need to send multiple \
         messages or messages to a different chat than the current conversation."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "chat_id": {
                    "type": "integer",
                    "description": "Target Telegram chat ID. If omitted, defaults to the current chat from the agent run context."
                },
                "text": {
                    "type": "string",
                    "description": "The message text to send."
                },
                "formatting": {
                    "type": "string",
                    "enum": ["plain_text", "markdown", "markdown_raw"],
                    "default": "plain_text",
                    "description": "Message formatting style. 'plain_text' for no formatting, 'markdown' for standard Markdown (automatically formatted and escaped), 'markdown_raw' for raw Telegram MarkdownV2 (requires manual escaping of dots, hyphens, and single-asterisk bold)."
                },
                "disable_notification": {
                    "type": "boolean",
                    "default": false,
                    "description": "If true, send the message without triggering notification sounds on the recipient's device."
                }
            },
            "required": ["text"]
        })
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        ctx: ToolContext,
    ) -> Result<ToolOutput, AgentError> {
        // Extract and validate arguments
        let text = args
            .get("text")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::Generic("text is required".into()))?;

        let chat_id = args
            .get("chat_id")
            .and_then(|v| v.as_i64())
            .or_else(|| ctx.run_mode.chat_id())
            .ok_or_else(|| {
                AgentError::Generic(
                    "chat_id is required: either provide it explicitly or run from a Telegram chat context".into(),
                )
            })?;

        let formatting = args
            .get("formatting")
            .and_then(|v| v.as_str())
            .unwrap_or("plain_text");

        let disable_notification = args
            .get("disable_notification")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // Enforce allowlist: the target chat must be in the allowed list
        if !ctx.allowed_chat_ids.is_empty() && !ctx.allowed_chat_ids.contains(&chat_id) {
            return Err(AgentError::PermissionDenied);
        }

        // Enforce user allowlist if applicable
        if let Some(user_id) = ctx.run_mode.user_id()
            && !ctx.allowed_user_ids.is_empty()
            && !ctx.allowed_user_ids.contains(&user_id)
        {
            return Err(AgentError::PermissionDenied);
        }

        // Determine parse mode
        let parse_mode = match formatting {
            "markdown" => Some("MarkdownV2"),
            "markdown_raw" => Some("MarkdownV2Raw"),
            _ => None,
        };

        // Send via TelegramService (handles long message splitting)
        let token = &ctx.telegram_token;
        if token.is_empty() {
            return Ok(ToolOutput {
                success: true,
                data: json!({
                    "tool_name": "send_user_message",
                    "chat_id": chat_id,
                    "sent": false,
                    "formatting": formatting,
                    "disable_notification": disable_notification,
                    "reason": "Telegram token not configured (mock mode)"
                }),
                summary: format!(
                    "Message to chat {chat_id} would have been sent (mock mode, no token configured)"
                ),
            });
        }

        let bot = crate::telegram::bot::TelegramBot::new(token.clone());
        let service = crate::telegram::service::TelegramService::new(std::sync::Arc::new(bot));

        service
            .send_message_with_options(chat_id, text, parse_mode, Some(disable_notification))
            .await?;

        Ok(ToolOutput {
            success: true,
            data: json!({
                "tool_name": "send_user_message",
                "chat_id": chat_id,
                "sent": true,
                "formatting": formatting,
                "disable_notification": disable_notification
            }),
            summary: format!("Message sent to chat {chat_id}"),
        })
    }
}
