//! Chat session persistence — CRUD operations via SQLx.

use chrono::DateTime;
use sqlx::SqlitePool;
use tracing::debug;

use crate::channel::ConversationAddress;
use crate::error::AgentError;

pub trait IntoConversationAddress {
    fn into_address(self) -> ConversationAddress;
}

impl IntoConversationAddress for ConversationAddress {
    fn into_address(self) -> ConversationAddress {
        self
    }
}

impl IntoConversationAddress for &ConversationAddress {
    fn into_address(self) -> ConversationAddress {
        self.clone()
    }
}

impl IntoConversationAddress for i64 {
    fn into_address(self) -> ConversationAddress {
        ConversationAddress::telegram_chat(self)
    }
}

/// A chat session in the database.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ChatSession {
    pub id: String,
    pub channel_id: String,
    pub conversation_id: String,
    pub thread_id: Option<String>,
    pub created_at: DateTime<chrono::Utc>,
    pub updated_at: DateTime<chrono::Utc>,
}

impl ChatSession {
    /// Create a new chat session record.
    pub fn new(address: impl IntoConversationAddress) -> Self {
        let address = address.into_address();
        let now = chrono::Utc::now();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            channel_id: address.channel_id,
            conversation_id: address.conversation_id,
            thread_id: address.thread_id,
            created_at: now,
            updated_at: now,
        }
    }

    pub fn address(&self) -> ConversationAddress {
        ConversationAddress {
            channel_id: self.channel_id.clone(),
            conversation_id: self.conversation_id.clone(),
            thread_id: self.thread_id.clone(),
        }
    }
}

/// Create a chat session and insert it into the database.
pub async fn create_session(
    pool: &SqlitePool,
    address: impl IntoConversationAddress,
) -> Result<ChatSession, AgentError> {
    let address = address.into_address();
    let session = ChatSession::new(address.clone());

    sqlx::query(
        r#"
        INSERT INTO chat_sessions (id, channel_id, conversation_id, thread_id, created_at, updated_at)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6)
        "#,
    )
    .bind(&session.id)
    .bind(&session.channel_id)
    .bind(&session.conversation_id)
    .bind(&session.thread_id)
    .bind(session.created_at.to_rfc3339())
    .bind(session.updated_at.to_rfc3339())
    .execute(pool)
    .await
    .map_err(|e| AgentError::Storage(format!("Failed to create session: {e}")))?;

    debug!(session_id = %session.id, address = ?address, "created chat session");
    Ok(session)
}

/// Find a chat session by its database ID.
pub async fn get_session(
    pool: &SqlitePool,
    session_id: &str,
) -> Result<Option<ChatSession>, AgentError> {
    let row = sqlx::query_as::<_, ChatSession>(
        "SELECT id, channel_id, conversation_id, thread_id, created_at, updated_at FROM chat_sessions WHERE id = ?1",
    )
    .bind(session_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| AgentError::Storage(format!("Failed to get session: {e}")))?;
    Ok(row)
}

pub async fn get_session_for_chat(
    pool: &SqlitePool,
    chat_id: i64,
) -> Result<Option<ChatSession>, AgentError> {
    get_session_for_address(pool, &ConversationAddress::telegram_chat(chat_id)).await
}

/// Find the latest chat session for a channel conversation.
///
/// Returns `None` if no session exists for this chat.
pub async fn get_session_for_address(
    pool: &SqlitePool,
    address: &ConversationAddress,
) -> Result<Option<ChatSession>, AgentError> {
    let row = sqlx::query_as::<_, ChatSession>(
        "SELECT id, channel_id, conversation_id, thread_id, created_at, updated_at FROM chat_sessions WHERE channel_id = ?1 AND conversation_id = ?2 AND thread_id IS ?3 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(&address.channel_id)
    .bind(&address.conversation_id)
    .bind(&address.thread_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| AgentError::Storage(format!("Failed to get session for chat: {e}")))?;
    Ok(row)
}

/// List all chat sessions, most recently updated first.
pub async fn list_sessions(pool: &SqlitePool) -> Result<Vec<ChatSession>, AgentError> {
    sqlx::query_as::<_, ChatSession>(
        "SELECT id, channel_id, conversation_id, thread_id, created_at, updated_at FROM chat_sessions ORDER BY updated_at DESC",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| AgentError::Storage(format!("Failed to list sessions: {e}")))
}

/// Update a session's updated_at timestamp.
pub async fn update_session(pool: &SqlitePool, session_id: &str) -> Result<(), AgentError> {
    sqlx::query(
        r#"
        UPDATE chat_sessions SET updated_at = ?1 WHERE id = ?2
        "#,
    )
    .bind(chrono::Utc::now().to_rfc3339())
    .bind(session_id)
    .execute(pool)
    .await
    .map_err(|e| AgentError::Storage(format!("Failed to update session: {e}")))?;

    Ok(())
}
