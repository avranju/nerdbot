//! Compaction worker — produces structured summaries from old history.
//!
//! Implementations come in Phase 9.

use crate::error::AgentError;

/// Worker that performs actual compaction via an LLM call.
///
/// Phase 1 stub.
pub struct CompactionWorker;

impl CompactionWorker {
    pub fn new() -> Self {
        Self
    }

    /// Run compaction on a session, producing a structured summary.
    ///
    /// Phase 1 stub.
    pub async fn compact(&self, _session_id: &str) -> Result<String, AgentError> {
        Err(AgentError::Compaction(
            "Compaction worker not yet implemented".into(),
        ))
    }
}

impl Default for CompactionWorker {
    fn default() -> Self {
        Self::new()
    }
}
