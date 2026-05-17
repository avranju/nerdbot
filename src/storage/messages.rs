//! Message persistence.
//!
//! Implementations come in Phase 3.

use crate::llm::types;
use serde::{Deserialize, Serialize};

/// A stored message in a chat session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredMessage {
    pub id: String,
    pub chat_session_id: String,
    pub role: types::Role,
    pub content: String,
    pub structured_content_json: Option<String>,
    pub token_estimate: Option<usize>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl StoredMessage {
    pub fn new(
        chat_session_id: String,
        role: types::Role,
        content: String,
        token_estimate: Option<usize>,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            chat_session_id,
            role,
            content,
            structured_content_json: None,
            token_estimate,
            created_at: chrono::Utc::now(),
        }
    }
}
