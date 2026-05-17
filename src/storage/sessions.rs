//! Chat session persistence.
//!
//! Implementations come in Phase 3.

use serde::{Deserialize, Serialize};

/// A chat session in the database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatSession {
    pub id: String,
    pub telegram_chat_id: i64,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl ChatSession {
    pub fn new(telegram_chat_id: i64) -> Self {
        let now = chrono::Utc::now();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            telegram_chat_id,
            created_at: now,
            updated_at: now,
        }
    }
}
