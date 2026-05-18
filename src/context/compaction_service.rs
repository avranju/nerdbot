//! Compaction service — monitors session pressure and triggers async compaction.
//!
//! Implementations come in Phase 9.

use crate::error::AgentError;

/// State of compaction for a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompactionState {
    /// No compaction is happening.
    Idle,
    /// Compaction is running, targeting a specific boundary.
    Running { target_through_message_id: String },
    /// Compaction is running but more messages arrived (needs another pass).
    RunningAndDirty { target_through_message_id: String },
}

/// Manages background compaction for all sessions.
///
/// Phase 1 stub.
pub struct CompactionService;

impl CompactionService {
    pub fn new() -> Self {
        Self
    }

    /// Check if a session needs compaction and enqueue if so.
    pub async fn check_session(&self, _session_id: &str) -> Result<(), AgentError> {
        Err(AgentError::Compaction(
            "Compaction not yet implemented".into(),
        ))
    }

    /// Get the compaction state for a session.
    pub fn get_state(&self, _session_id: &str) -> CompactionState {
        CompactionState::Idle
    }
}

impl Default for CompactionService {
    fn default() -> Self {
        Self::new()
    }
}
