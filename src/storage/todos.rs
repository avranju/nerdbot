//! Instance-wide todo persistence with stable, non-reused integer IDs.

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::error::AgentError;

/// A todo in the shared instance-wide list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub struct StoredTodo {
    pub id: i64,
    pub text: String,
    pub completed: bool,
}

/// Add a pending todo. SQLite allocates the ID atomically; never derive it from a list position.
pub async fn create_todo(pool: &SqlitePool, text: &str) -> Result<StoredTodo, AgentError> {
    sqlx::query_as::<_, StoredTodo>(
        "INSERT INTO todos (text) VALUES (?1) RETURNING id, text, completed",
    )
    .bind(text)
    .fetch_one(pool)
    .await
    .map_err(|e| AgentError::Storage(format!("Failed to create todo: {e}")))
}

/// List todos in ascending ID order, optionally excluding completed items.
pub async fn list_todos(
    pool: &SqlitePool,
    pending_only: bool,
) -> Result<Vec<StoredTodo>, AgentError> {
    sqlx::query_as::<_, StoredTodo>(
        "SELECT id, text, completed FROM todos WHERE (?1 = 0 OR completed = 0) ORDER BY id",
    )
    .bind(pending_only)
    .fetch_all(pool)
    .await
    .map_err(|e| AgentError::Storage(format!("Failed to list todos: {e}")))
}

/// Mark a todo done. Repeating completion succeeds; a missing ID returns None.
pub async fn complete_todo(pool: &SqlitePool, id: i64) -> Result<Option<StoredTodo>, AgentError> {
    sqlx::query_as::<_, StoredTodo>(
        "UPDATE todos SET completed = 1 WHERE id = ?1 RETURNING id, text, completed",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(|e| AgentError::Storage(format!("Failed to complete todo: {e}")))
}

/// Change only the text, preserving the ID and completion state.
pub async fn update_todo(
    pool: &SqlitePool,
    id: i64,
    text: &str,
) -> Result<Option<StoredTodo>, AgentError> {
    sqlx::query_as::<_, StoredTodo>(
        "UPDATE todos SET text = ?2 WHERE id = ?1 RETURNING id, text, completed",
    )
    .bind(id)
    .bind(text)
    .fetch_optional(pool)
    .await
    .map_err(|e| AgentError::Storage(format!("Failed to update todo: {e}")))
}

/// Permanently delete a todo, returning whether it existed. Its ID will not be reused.
pub async fn delete_todo(pool: &SqlitePool, id: i64) -> Result<bool, AgentError> {
    let result = sqlx::query("DELETE FROM todos WHERE id = ?1")
        .bind(id)
        .execute(pool)
        .await
        .map_err(|e| AgentError::Storage(format!("Failed to delete todo: {e}")))?;
    Ok(result.rows_affected() > 0)
}
