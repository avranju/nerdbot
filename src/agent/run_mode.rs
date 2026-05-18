//! Agent run modes — distinguishes interactive, scheduled, and internal runs.

use serde::{Deserialize, Serialize};

/// Identifier for a Telegram chat.
pub type TelegramChatId = i64;

/// Identifier for a Telegram user.
pub type TelegramUserId = i64;

/// Unique identifier for a scheduled job.
pub type JobId = String;

/// Which kind of agent run is being executed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentRunMode {
    /// User sent a message in a Telegram chat.
    InteractiveReply {
        chat_id: TelegramChatId,
        user_id: TelegramUserId,
    },
    /// A scheduled job fired.
    ScheduledJob {
        job_id: JobId,
        /// Default chat to send notification to if job completes.
        default_chat_id: TelegramChatId,
        /// Whether to send the final result if the model didn't already notify.
        notify_on_completion: bool,
    },
    /// Internal harness-initiated run (e.g. compaction, maintenance).
    Internal { reason: String },
}

impl AgentRunMode {
    /// Returns the primary chat ID for this run (used for messaging).
    pub fn chat_id(&self) -> Option<TelegramChatId> {
        match self {
            AgentRunMode::InteractiveReply { chat_id, .. } => Some(*chat_id),
            AgentRunMode::ScheduledJob {
                default_chat_id, ..
            } => Some(*default_chat_id),
            AgentRunMode::Internal { .. } => None,
        }
    }

    /// Returns the user ID if this is an interactive run.
    pub fn user_id(&self) -> Option<TelegramUserId> {
        match self {
            AgentRunMode::InteractiveReply { user_id, .. } => Some(*user_id),
            _ => None,
        }
    }
}
