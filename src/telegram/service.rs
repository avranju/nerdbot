//! Telegram service — outbound messaging with long-message splitting.
//!
//! Wraps TelegramBot to provide a clean interface for sending messages,
//! automatically splitting them if they exceed Telegram's 4096-char limit.

use std::sync::Arc;
use std::time::Duration;

use crate::error::AgentError;
use tracing::{debug, info, warn};

use super::bot::TelegramBot;
use crate::channel::{
    ChannelService, ChannelTypingIndicator, ConversationAddress, MessageFormat, OutboundMessage,
};

/// High-level service for sending messages through Telegram.
///
/// Handles message splitting and provides a simpler interface than
/// the raw bot client.
#[derive(Debug, Clone)]
pub struct TelegramService {
    bot: Arc<TelegramBot>,
}

/// Keeps Telegram's short-lived typing action refreshed while an agent turn runs.
pub struct TypingIndicator {
    refresh_task: tokio::task::JoinHandle<()>,
}

impl Drop for TypingIndicator {
    fn drop(&mut self) {
        self.refresh_task.abort();
    }
}

impl TelegramService {
    /// Create a new service wrapping a bot client.
    pub fn new(bot: Arc<TelegramBot>) -> Self {
        Self { bot }
    }

    /// Get a reference to the underlying bot.
    pub fn bot(&self) -> &Arc<TelegramBot> {
        &self.bot
    }

    /// Start refreshing Telegram's typing action until the returned guard is dropped.
    pub fn start_typing(&self, chat_id: i64) -> TypingIndicator {
        let service = self.clone();
        let refresh_task = tokio::spawn(async move {
            loop {
                if let Err(e) = service.bot.send_typing_action(chat_id).await {
                    warn!(chat_id, error = %e, "failed to send Telegram typing action");
                }
                tokio::time::sleep(Duration::from_secs(4)).await;
            }
        });

        TypingIndicator { refresh_task }
    }

    /// Send a text message to a chat.
    ///
    /// If the message exceeds Telegram's 4096-char limit, it is split into
    /// multiple messages and each is sent separately.
    pub async fn send_message(&self, chat_id: i64, text: &str) -> Result<(), AgentError> {
        self.send_message_with_options(chat_id, text, None, None)
            .await
    }

    /// Send a text message to a chat with optional formatting and notification settings.
    ///
    /// If the message exceeds Telegram's 4096-char limit, it is split into
    /// multiple messages and each is sent separately.
    ///
    /// # Arguments
    /// * `chat_id` - Target Telegram chat ID
    /// * `text` - Message text
    /// * `parse_mode` - Optional formatting: `"MarkdownV2"` for standard markdown conversion,
    ///   `"MarkdownV2Raw"` for pre-escaped Telegram markdown, or `None` for plain text
    /// * `disable_notification` - If true, send without triggering notification sounds
    pub async fn send_message_with_options(
        &self,
        chat_id: i64,
        text: &str,
        parse_mode: Option<&str>,
        disable_notification: Option<bool>,
    ) -> Result<(), AgentError> {
        match parse_mode {
            Some("MarkdownV2") => {
                let formatted = super::markdown::parse_markdown_to_v2(text);
                if formatted.chars().count() <= super::bot::TELEGRAM_MAX_MESSAGE_LENGTH {
                    return self
                        .send_formatted_with_parse_fallback(
                            chat_id,
                            &formatted,
                            text,
                            disable_notification,
                        )
                        .await;
                }
                warn!(
                    chat_id,
                    formatted_len = formatted.chars().count(),
                    "formatted MarkdownV2 response exceeds Telegram limit, sending split plain text"
                );
            }
            Some("MarkdownV2Raw") => {
                if text.chars().count() <= super::bot::TELEGRAM_MAX_MESSAGE_LENGTH {
                    return self
                        .send_formatted_with_parse_fallback(
                            chat_id,
                            text,
                            text,
                            disable_notification,
                        )
                        .await;
                }
                warn!(
                    chat_id,
                    text_len = text.chars().count(),
                    "raw MarkdownV2 response exceeds Telegram limit, sending split plain text"
                );
            }
            _ => {
                return self
                    .send_split_message(chat_id, text, parse_mode, disable_notification)
                    .await;
            }
        }

        self.send_split_message(chat_id, text, None, disable_notification)
            .await
    }

    async fn send_formatted_with_parse_fallback(
        &self,
        chat_id: i64,
        formatted_text: &str,
        plain_text: &str,
        disable_notification: Option<bool>,
    ) -> Result<(), AgentError> {
        match self
            .bot
            .send_message(
                chat_id,
                formatted_text,
                Some("MarkdownV2"),
                disable_notification,
            )
            .await
        {
            Ok(_) => Ok(()),
            Err(e) if is_telegram_parse_error(&e) => {
                warn!(chat_id, error = %e, "MarkdownV2 entity parse failed, retrying as plain text");
                self.send_split_message(chat_id, plain_text, None, disable_notification)
                    .await
            }
            Err(e) => Err(e),
        }
    }

    async fn send_split_message(
        &self,
        chat_id: i64,
        text: &str,
        parse_mode: Option<&str>,
        disable_notification: Option<bool>,
    ) -> Result<(), AgentError> {
        if text.chars().count() <= super::bot::TELEGRAM_MAX_MESSAGE_LENGTH {
            self.bot
                .send_message(chat_id, text, parse_mode, disable_notification)
                .await?;
            return Ok(());
        }

        let chunks = TelegramBot::split_long_message(text);
        info!(
            chat_id,
            total_chunks = chunks.len(),
            original_len = text.chars().count(),
            "splitting long Telegram message into chunks"
        );

        for (i, chunk) in chunks.iter().enumerate() {
            let prefix = if chunks.len() > 1 {
                format!("[{}/{}] ", i + 1, chunks.len())
            } else {
                String::new()
            };

            let message = format!("{prefix}{chunk}");
            self.bot
                .send_message(chat_id, &message, parse_mode, disable_notification)
                .await?;
            debug!(chunk = i + 1, total = chunks.len(), "chunk sent");
        }

        Ok(())
    }
}

fn is_telegram_parse_error(err: &AgentError) -> bool {
    if let AgentError::Telegram(msg) = err {
        msg.contains("can't parse entities") || msg.contains("can't parse message")
    } else {
        false
    }
}

#[async_trait::async_trait]
impl ChannelService for TelegramService {
    fn channel_id(&self) -> &str {
        "telegram"
    }

    async fn send_message(
        &self,
        address: &ConversationAddress,
        message: OutboundMessage,
    ) -> Result<(), AgentError> {
        let chat_id = address.conversation_id.parse::<i64>().map_err(|e| {
            AgentError::Telegram(format!(
                "Telegram conversation_id must be an integer chat ID, got {:?}: {e}",
                address.conversation_id
            ))
        })?;
        let parse_mode = match message.format {
            MessageFormat::PlainText => None,
            MessageFormat::Markdown => Some("MarkdownV2"),
            MessageFormat::MarkdownRaw => Some("MarkdownV2Raw"),
        };
        self.send_message_with_options(
            chat_id,
            &message.text,
            parse_mode,
            message.disable_notification,
        )
        .await
    }

    fn start_typing(&self, address: &ConversationAddress) -> Option<ChannelTypingIndicator> {
        let chat_id = address.conversation_id.parse::<i64>().ok()?;
        let service = self.clone();
        let refresh_task = tokio::spawn(async move {
            loop {
                if let Err(e) = service.bot.send_typing_action(chat_id).await {
                    warn!(chat_id, error = %e, "failed to send Telegram typing action");
                }
                tokio::time::sleep(Duration::from_secs(4)).await;
            }
        });
        Some(ChannelTypingIndicator::new(refresh_task))
    }
}
