//! Message persistence — CRUD operations via SQLx.

use chrono::DateTime;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::SqlitePool;
use tracing::debug;

use crate::error::AgentError;
use crate::llm::types::{Message, MessageContent, Role};

/// A stored message row from the database.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct StoredMessage {
    pub id: String,
    pub chat_session_id: String,
    pub role: Value,
    pub content: String,
    pub structured_content_json: Option<Value>,
    pub token_estimate: Option<i64>,
    pub created_at: DateTime<chrono::Utc>,
}

impl StoredMessage {
    /// Create a new stored message record.
    pub fn new(
        chat_session_id: String,
        role: Role,
        content: String,
        token_estimate: Option<usize>,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            chat_session_id,
            role: serde_json::to_value(&role).unwrap_or(Value::Null),
            content,
            structured_content_json: None,
            token_estimate: token_estimate.map(|t| t as i64),
            created_at: chrono::Utc::now(),
        }
    }
}

/// Convert a stored message back into a typed `Message`.
impl StoredMessage {
    /// Deserialize the role from JSON.
    pub fn role(&self) -> Result<Role, AgentError> {
        serde_json::from_value(self.role.clone()).map_err(|e| AgentError::Storage(format!("Failed to deserialize role: {e}")))
    }

    /// Convert to a full `Message` with role.
    pub fn to_message(&self) -> Result<Message, AgentError> {
        let role = self.role()?;
        Ok(Message::new(role, MessageContent::Text(self.content.clone())))
    }
}

/// Insert a message into the database.
pub async fn create_message(
    pool: &SqlitePool,
    session_id: &str,
    message: &Message,
    token_estimate: Option<usize>,
) -> Result<StoredMessage, AgentError> {
    let (content, structured_json) = match &message.content {
        MessageContent::Text(t) => (t.clone(), None),
        MessageContent::Parts(parts) => {
            let json = serde_json::to_value(parts).unwrap_or(Value::Null);
            (String::new(), Some(json))
        }
    };

    let stored = StoredMessage::new(
        session_id.to_string(),
        message.role.clone(),
        content.clone(),
        token_estimate,
    );

    sqlx::query(
        r#"
        INSERT INTO messages (id, chat_session_id, role, content, structured_content_json, token_estimate, created_at)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
        "#,
    )
    .bind(&stored.id)
    .bind(session_id)
    .bind(&stored.role)
    .bind(content)
    .bind(structured_json)
    .bind(stored.token_estimate)
    .bind(stored.created_at.to_rfc3339())
    .execute(pool)
    .await
    .map_err(|e| AgentError::Storage(format!("Failed to create message: {e}")))?;

    debug!(message_id = %stored.id, session_id, "created message");
    Ok(stored)
}

/// List messages for a session, ordered by creation time.
pub async fn list_messages(
    pool: &SqlitePool,
    session_id: &str,
    limit: Option<usize>,
) -> Result<Vec<StoredMessage>, AgentError> {
    let query = if let Some(lim) = limit {
        format!(
            "SELECT id, chat_session_id, role, content, structured_content_json, token_estimate, created_at FROM messages WHERE chat_session_id = ?1 ORDER BY created_at DESC LIMIT {}",
            lim
        )
    } else {
        "SELECT id, chat_session_id, role, content, structured_content_json, token_estimate, created_at FROM messages WHERE chat_session_id = ?1 ORDER BY created_at DESC".to_string()
    };

    sqlx::query_as::<_, StoredMessage>(&query)
        .bind(session_id)
        .fetch_all(pool)
        .await
        .map_err(|e| AgentError::Storage(format!("Failed to list messages: {e}")))
}
