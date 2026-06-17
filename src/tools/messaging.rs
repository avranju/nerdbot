//! User messaging tool — send_user_message.

use serde_json::json;

use crate::channel::{ConversationAddress, MessageFormat, OutboundMessage};
use crate::error::AgentError;
use crate::tools::traits::{Tool, ToolContext, ToolOutput};

pub struct SendUserMessage;

#[async_trait::async_trait]
impl Tool for SendUserMessage {
    fn name(&self) -> &'static str {
        "send_user_message"
    }

    fn description(&self) -> &'static str {
        "Send a message to a configured communication channel conversation. Use this for scheduled job notifications, \
         conditional alerts, intermediate updates, or when you need to send multiple \
         messages or messages to a different conversation than the current one."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "conversation": {
                    "type": "object",
                    "description": "Target conversation. If omitted, defaults to the current conversation from the agent run context.",
                    "properties": {
                        "channel_id": { "type": "string", "description": "Channel identifier, e.g. 'telegram'." },
                        "conversation_id": { "type": "string", "description": "Channel-specific conversation ID." },
                        "thread_id": { "type": ["string", "null"], "description": "Optional thread/topic ID." }
                    },
                    "required": ["channel_id", "conversation_id"]
                },
                "text": {
                    "type": "string",
                    "description": "The message text to send."
                },
                "formatting": {
                    "type": "string",
                    "enum": ["plain_text", "markdown", "markdown_raw"],
                    "default": "plain_text",
                    "description": "Message formatting style. 'plain_text' for no formatting, 'markdown' for channel-safe Markdown, 'markdown_raw' for pre-escaped channel-specific Markdown."
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

        let address = if let Some(conversation) = args.get("conversation") {
            ConversationAddress {
                channel_id: conversation
                    .get("channel_id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        AgentError::Generic("conversation.channel_id is required".into())
                    })?
                    .to_string(),
                conversation_id: conversation
                    .get("conversation_id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        AgentError::Generic("conversation.conversation_id is required".into())
                    })?
                    .to_string(),
                thread_id: conversation
                    .get("thread_id")
                    .and_then(|v| v.as_str())
                    .map(ToString::to_string),
            }
        } else {
            ctx.run_mode.address().cloned().ok_or_else(|| {
                AgentError::Generic(
                    "conversation is required: either provide it explicitly or run from a channel conversation context".into(),
                )
            })?
        };

        let formatting = args
            .get("formatting")
            .and_then(|v| v.as_str())
            .unwrap_or("plain_text");

        let disable_notification = args
            .get("disable_notification")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        if !ctx.access_policy.is_allowed(
            &address,
            &ctx.run_mode
                .sender()
                .cloned()
                .unwrap_or_else(|| crate::channel::SenderIdentity::new("unknown", None)),
        ) {
            return Err(AgentError::PermissionDenied);
        }

        let format = match formatting {
            "markdown" => MessageFormat::Markdown,
            "markdown_raw" => MessageFormat::MarkdownRaw,
            _ => MessageFormat::PlainText,
        };

        let Some(registry) = ctx.channel_registry else {
            return Ok(ToolOutput {
                success: true,
                data: json!({
                    "tool_name": "send_user_message",
                    "conversation": address,
                    "sent": false,
                    "formatting": formatting,
                    "disable_notification": disable_notification,
                    "reason": "No channel registry configured (mock mode)"
                }),
                summary: format!("Message to {:?} would have been sent (mock mode)", address),
            });
        };

        registry
            .send_message(
                &address,
                OutboundMessage {
                    text: text.to_string(),
                    format,
                    disable_notification: Some(disable_notification),
                },
            )
            .await?;

        Ok(ToolOutput {
            success: true,
            data: json!({
                "tool_name": "send_user_message",
                "conversation": address,
                "sent": true,
                "formatting": formatting,
                "disable_notification": disable_notification
            }),
            summary: "Message sent".to_string(),
        })
    }
}
