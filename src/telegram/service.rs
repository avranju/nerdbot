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
        // Check if the message needs to be split
        if text.chars().count() <= super::bot::TELEGRAM_MAX_MESSAGE_LENGTH {
            self.bot.send_message(chat_id, text, None).await?;
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
            self.bot.send_message(chat_id, &message, None).await?;
            debug!(chunk = i + 1, total = chunks.len(), "chunk sent");
        }

        Ok(())
    }
}
