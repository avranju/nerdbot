//! Chat session persistence — CRUD operations via SQLx.

use chrono::DateTime;
use sqlx::SqlitePool;
use tracing::debug;

use crate::error::AgentError;

/// A chat session in the database.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ChatSession {
    pub id: String,
    pub telegram_chat_id: i64,
    pub created_at: DateTime<chrono::Utc>,
    pub updated_at: DateTime<chrono::Utc>,
}

impl ChatSession {
    /// Create a new chat session record.
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

/// Create a chat session and insert it into the database.
pub async fn create_session(pool: &SqlitePool, chat_id: i64) -> Result<ChatSession, AgentError> {
    let session = ChatSession::new(chat_id);

    sqlx::query(
        r#"
        INSERT INTO chat_sessions (id, telegram_chat_id, created_at, updated_at)
        VALUES (?1, ?2, ?3, ?4)
        "#,
    )
    .bind(&session.id)
    .bind(session.telegram_chat_id)
    .bind(session.created_at.to_rfc3339())
    .bind(session.updated_at.to_rfc3339())
    .execute(pool)
    .await
    .map_err(|e| AgentError::Storage(format!("Failed to create session: {e}")))?;

    debug!(session_id = %session.id, chat_id = chat_id, "created chat session");
    Ok(session)
}

/// Find a chat session by its database ID.
pub async fn get_session(
    pool: &SqlitePool,
    session_id: &str,
) -> Result<Option<ChatSession>, AgentError> {
    let row = sqlx::query_as::<_, ChatSession>(
        "SELECT id, telegram_chat_id, created_at, updated_at FROM chat_sessions WHERE id = ?1",
    )
    .bind(session_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| AgentError::Storage(format!("Failed to get session: {e}")))?;
    Ok(row)
}

/// Find the latest chat session for a Telegram chat ID.
///
/// Returns `None` if no session exists for this chat.
pub async fn get_session_for_chat(
    pool: &SqlitePool,
    telegram_chat_id: i64,
) -> Result<Option<ChatSession>, AgentError> {
    let row = sqlx::query_as::<_, ChatSession>(
        "SELECT id, telegram_chat_id, created_at, updated_at FROM chat_sessions WHERE telegram_chat_id = ?1 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(telegram_chat_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| AgentError::Storage(format!("Failed to get session for chat: {e}")))?;
    Ok(row)
}

/// List all chat sessions, most recently updated first.
pub async fn list_sessions(pool: &SqlitePool) -> Result<Vec<ChatSession>, AgentError> {
    sqlx::query_as::<_, ChatSession>(
        "SELECT id, telegram_chat_id, created_at, updated_at FROM chat_sessions ORDER BY updated_at DESC",
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
