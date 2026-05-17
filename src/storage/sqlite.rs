//! SQLite database connection and schema.
//!
//! Implementations come in Phase 3.

use crate::error::AgentError;

/// SQLite database handle.
///
/// Phase 1 stub — full implementation with migrations in Phase 3.
pub struct Database {
    path: std::path::PathBuf,
}

impl Database {
    pub fn new(path: std::path::PathBuf) -> Self {
        Self { path }
    }

    /// Initialize the database, creating tables if needed.
    pub async fn init(&self) -> Result<(), AgentError> {
        // Phase 1 stub — actual SQL migrations in Phase 3
        tracing::info!(db_path = %self.path.display(), "database initialized (stub)");
        Ok(())
    }

    /// Get a connection to the database.
    pub async fn connection(&self) -> Result<(), AgentError> {
        Err(AgentError::Storage("Database not yet implemented".into()))
    }
}
