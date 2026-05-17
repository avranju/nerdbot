//! Telegram command handling.
//!
//! Implementations come in Phase 4.

/// Supported Telegram commands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TelegramCommand {
    Start,
    Help,
    Jobs,
    Run(String),
    Delete(String),
    ResetContext,
}

impl TelegramCommand {
    /// Parse a command string (e.g. "/jobs" -> Jobs).
    pub fn parse(command: &str) -> Option<Self> {
        match command.trim() {
            "/start" => Some(Self::Start),
            "/help" => Some(Self::Help),
            "/jobs" => Some(Self::Jobs),
            _ if command.starts_with("/run ") => Some(Self::Run(command.trim_start_matches("/run ").to_string())),
            _ if command.starts_with("/delete ") => Some(Self::Delete(command.trim_start_matches("/delete ").to_string())),
            "/reset-context" => Some(Self::ResetContext),
            _ => None,
        }
    }
}
