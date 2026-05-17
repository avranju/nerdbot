//! Context summary persistence.
//!
//! Implementations come in Phase 3.

use crate::context::summaries::ContextSummary;
use serde::{Deserialize, Serialize};

/// A stored context summary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredSummary {
    pub id: String,
    pub chat_session_id: String,
    pub summary_text: String,
    pub covers_through_message_id: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl From<ContextSummary> for StoredSummary {
    fn from(summary: ContextSummary) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            chat_session_id: summary.chat_session_id,
            summary_text: summary.summary_text,
            covers_through_message_id: summary.covers_through_message_id,
            created_at: summary.created_at,
        }
    }
}
