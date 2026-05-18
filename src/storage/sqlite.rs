//! SQLite database connection, schema, and migration.

use std::path::PathBuf;

use sqlx::SqlitePool;
use tracing::info;

use crate::error::AgentError;

/// SQLite database with connection pool.
pub struct Database {
    pool: SqlitePool,
}

impl Database {
    /// Create a new database handle and connect.
    pub async fn new(path: PathBuf) -> Result<Self, AgentError> {
        let pool = SqlitePool::connect(format!("sqlite://{}", path.display()).as_str())
            .await
            .map_err(|e| AgentError::Storage(format!("Failed to connect to database: {e}")))?;

        info!(db_path = %path.display(), "connected to SQLite database");
        Ok(Self { pool })
    }

    /// Initialize the database, creating all tables if they don't exist.
    pub async fn init(&self) -> Result<(), AgentError> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS chat_sessions (
                id TEXT PRIMARY KEY,
                telegram_chat_id INTEGER NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| AgentError::Storage(format!("Failed to create chat_sessions table: {e}")))?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS messages (
                id TEXT PRIMARY KEY,
                chat_session_id TEXT NOT NULL REFERENCES chat_sessions(id),
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                structured_content_json TEXT,
                token_estimate INTEGER,
                created_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| AgentError::Storage(format!("Failed to create messages table: {e}")))?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS context_summaries (
                id TEXT PRIMARY KEY,
                chat_session_id TEXT NOT NULL REFERENCES chat_sessions(id),
                summary_text TEXT NOT NULL,
                covers_through_message_id TEXT NOT NULL,
                created_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| AgentError::Storage(format!("Failed to create context_summaries table: {e}")))?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS scheduled_jobs (
                id TEXT PRIMARY KEY,
                owner_chat_id INTEGER NOT NULL,
                name TEXT NOT NULL,
                prompt TEXT NOT NULL,
                schedule_type TEXT NOT NULL,
                cron_expression TEXT,
                run_at TEXT,
                timezone TEXT,
                notify_on_completion INTEGER NOT NULL DEFAULT 0,
                context_policy TEXT NOT NULL,
                creation_context_snapshot TEXT,
                enabled INTEGER NOT NULL DEFAULT 1,
                last_run_at TEXT,
                next_run_at TEXT,
                last_status TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| AgentError::Storage(format!("Failed to create scheduled_jobs table: {e}")))?;

        info!("database initialized with all tables");
        Ok(())
    }

    /// Get the underlying connection pool.
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}
