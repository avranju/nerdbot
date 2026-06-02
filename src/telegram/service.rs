//! Telegram service — outbound messaging with long-message splitting.
//!
//! Wraps TelegramBot to provide a clean interface for sending messages,
//! automatically splitting them if they exceed Telegram's 4096-char limit.

use std::sync::Arc;
use std::time::Duration;

use crate::error::AgentError;
use tracing::{debug, info, warn};

use super::bot::TelegramBot;

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
    /// * `parse_mode` - Optional formatting: `"MarkdownV2"` for markdown, or `None` for plain text
    /// * `disable_notification` - If true, send without triggering notification sounds
    pub async fn send_message_with_options(
        &self,
        chat_id: i64,
        text: &str,
        parse_mode: Option<&str>,
        disable_notification: Option<bool>,
    ) -> Result<(), AgentError> {
        // Check if the message needs to be split
        if text.chars().count() <= super::bot::TELEGRAM_MAX_MESSAGE_LENGTH {
            if parse_mode == Some("MarkdownV2") {
                let formatted = super::markdown::parse_markdown_to_v2(text);
                match self
                    .bot
                    .send_message(chat_id, &formatted, parse_mode, disable_notification)
                    .await
                {
                    Ok(_) => return Ok(()),
                    Err(e) => {
                        warn!(chat_id, error = %e, "MarkdownV2 delivery failed, retrying as plain text");
                        self.bot
                            .send_message(chat_id, text, None, disable_notification)
                            .await?;
                        return Ok(());
                    }
                }
            } else {
                self.bot
                    .send_message(chat_id, text, parse_mode, disable_notification)
                    .await?;
                return Ok(());
            }
        }

        // Split into chunks
        let chunks = TelegramBot::split_long_message(text);
        info!(
            chat_id,
            total_chunks = chunks.len(),
            original_len = text.chars().count(),
            "splitting long message into chunks"
        );

        for (i, chunk) in chunks.iter().enumerate() {
            let prefix = if chunks.len() > 1 {
                format!("[{}/{}] ", i + 1, chunks.len())
            } else {
                String::new()
            };

            let message = format!("{prefix}{chunk}");
            if parse_mode == Some("MarkdownV2") {
                let formatted = super::markdown::parse_markdown_to_v2(&message);
                match self
                    .bot
                    .send_message(chat_id, &formatted, parse_mode, disable_notification)
                    .await
                {
                    Ok(_) => {}
                    Err(e) => {
                        warn!(chat_id, error = %e, "MarkdownV2 delivery failed for chunk, retrying as plain text");
                        self.bot
                            .send_message(chat_id, &message, None, disable_notification)
                            .await?;
                    }
                }
            } else {
                self.bot
                    .send_message(chat_id, &message, parse_mode, disable_notification)
                    .await?;
            }
            debug!(chunk = i + 1, total = chunks.len(), "chunk sent");
        }

        Ok(())
    }
}
