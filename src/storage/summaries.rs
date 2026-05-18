//! Context summary persistence — CRUD operations via SQLx.

use chrono::DateTime;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use tracing::debug;

use crate::context::summaries::ContextSummary;
use crate::error::AgentError;

/// A stored context summary row from the database.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct StoredSummary {
    pub id: String,
    pub chat_session_id: String,
    pub summary_text: String,
    pub covers_through_message_id: String,
    pub created_at: DateTime<chrono::Utc>,
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

/// Insert a context summary into the database.
pub async fn create_summary(
    pool: &SqlitePool,
    summary: &ContextSummary,
) -> Result<StoredSummary, AgentError> {
    let stored = StoredSummary::from(summary.clone());

    sqlx::query(
        r#"
        INSERT INTO context_summaries (id, chat_session_id, summary_text, covers_through_message_id, created_at)
        VALUES (?1, ?2, ?3, ?4, ?5)
        "#,
    )
    .bind(&stored.id)
    .bind(&stored.chat_session_id)
    .bind(&stored.summary_text)
    .bind(&stored.covers_through_message_id)
    .bind(stored.created_at.to_rfc3339())
    .execute(pool)
    .await
    .map_err(|e| AgentError::Storage(format!("Failed to create summary: {e}")))?;

    debug!(
        summary_id = %stored.id,
        session_id = %stored.chat_session_id,
        "created context summary"
    );
    Ok(stored)
}

/// Get the latest (most recent) context summary for a session.
pub async fn get_latest_summary(
    pool: &SqlitePool,
    session_id: &str,
) -> Result<Option<StoredSummary>, AgentError> {
    let row = sqlx::query_as::<_, StoredSummary>(
        "SELECT id, chat_session_id, summary_text, covers_through_message_id, created_at FROM context_summaries WHERE chat_session_id = ?1 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(session_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| AgentError::Storage(format!("Failed to get latest summary: {e}")))?;
    Ok(row)
}
