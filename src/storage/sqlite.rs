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

    /// Initialize the database by running all pending migrations.
    ///
    /// Migrations are idempotent — already-applied migrations are skipped.
    pub async fn init(&self) -> Result<(), AgentError> {
        sqlx::migrate!("./migrations")
            .run(&self.pool)
            .await
            .map_err(|e| AgentError::Storage(format!("Failed to run migrations: {e}")))?;

        info!("database initialized via migrations");
        Ok(())
    }

    /// Get the underlying connection pool.
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}
