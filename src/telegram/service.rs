//! Telegram service — outbound messaging.
//!
//! Implementations come in Phase 4.

use crate::error::AgentError;

/// Service for sending messages through Telegram.
pub struct TelegramService {
    bot: super::bot::TelegramBot,
}

impl TelegramService {
    pub fn new(token: String) -> Self {
        Self {
            bot: super::bot::TelegramBot::new(token),
        }
    }

    /// Send a text message to a chat.
    ///
    /// Phase 1 stub.
    pub async fn send_message(&self, _chat_id: i64, _text: &str) -> Result<(), AgentError> {
        Err(AgentError::Telegram(
            "Telegram service not yet implemented".into(),
        ))
    }
}
