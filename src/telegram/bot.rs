//! Telegram bot client — long polling implementation.
//!
//! Implementations come in Phase 4.

/// Skeleton stub for the Telegram bot client.
pub struct TelegramBot {
    pub token: String,
}

impl TelegramBot {
    pub fn new(token: String) -> Self {
        Self { token }
    }
}
