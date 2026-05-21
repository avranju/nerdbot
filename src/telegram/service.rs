//! Telegram service — outbound messaging with long-message splitting.
//!
//! Wraps TelegramBot to provide a clean interface for sending messages,
//! automatically splitting them if they exceed Telegram's 4096-char limit.

use std::sync::Arc;

use crate::error::AgentError;
use tracing::{debug, info};

use super::bot::TelegramBot;

/// High-level service for sending messages through Telegram.
///
/// Handles message splitting and provides a simpler interface than
/// the raw bot client.
#[derive(Debug, Clone)]
pub struct TelegramService {
    bot: Arc<TelegramBot>,
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

    /// Send a text message to a chat.
    ///
    /// If the message exceeds Telegram's 4096-char limit, it is split into
    /// multiple messages and each is sent separately.
    pub async fn send_message(&self, chat_id: i64, text: &str) -> Result<(), AgentError> {
        self.send_message_with_options(chat_id, text, None, None).await
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
            self.bot
                .send_message(chat_id, text, parse_mode, disable_notification)
                .await?;
            return Ok(());
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
            self.bot
                .send_message(chat_id, &message, parse_mode, disable_notification)
                .await?;
            debug!(chunk = i + 1, total = chunks.len(), "chunk sent");
        }

        Ok(())
    }
}
