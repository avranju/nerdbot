//! Context summaries — structured conversation summaries.
//!
//! Implementations come in Phase 9.

use serde::{Deserialize, Serialize};

/// A structured rolling summary of conversation history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextSummary {
    /// Session ID this summary belongs to.
    pub chat_session_id: String,
    /// The summary text.
    pub summary_text: String,
    /// The summary covers all messages up to and including this message ID.
    pub covers_through_message_id: String,
    /// When the summary was created.
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl ContextSummary {
    pub fn new(
        chat_session_id: String,
        summary_text: String,
        covers_through_message_id: String,
    ) -> Self {
        Self {
            chat_session_id,
            summary_text,
            covers_through_message_id,
            created_at: chrono::Utc::now(),
        }
    }
}
