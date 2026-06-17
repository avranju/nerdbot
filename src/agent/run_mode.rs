//! Agent run modes — distinguishes interactive, scheduled, and internal runs.

use serde::{Deserialize, Serialize};

use crate::channel::{ConversationAddress, SenderIdentity};

/// Unique identifier for a scheduled job.
pub type JobId = String;

/// Which kind of agent run is being executed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentRunMode {
    /// User sent a message.
    InteractiveReply {
        address: ConversationAddress,
        sender: SenderIdentity,
    },
    /// A scheduled job fired.
    ScheduledJob {
        job_id: JobId,
        /// Default conversation to send notification to if job completes.
        default_address: ConversationAddress,
        /// Whether to send the final result if the model didn't already notify.
        notify_on_completion: bool,
    },
    /// Internal harness-initiated run (e.g. compaction, maintenance).
    Internal { reason: String },
}

impl AgentRunMode {
    /// Returns the primary conversation address for this run.
    pub fn address(&self) -> Option<&ConversationAddress> {
        match self {
            AgentRunMode::InteractiveReply { address, .. } => Some(address),
            AgentRunMode::ScheduledJob {
                default_address, ..
            } => Some(default_address),
            AgentRunMode::Internal { .. } => None,
        }
    }

    /// Returns the sender identity if this is an interactive run.
    pub fn sender(&self) -> Option<&SenderIdentity> {
        match self {
            AgentRunMode::InteractiveReply { sender, .. } => Some(sender),
            _ => None,
        }
    }
}
