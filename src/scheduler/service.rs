//! Scheduler service — manages job lifecycle.
//!
//! Implementations come in Phase 5.

use crate::error::AgentError;

/// Manages scheduled jobs.
///
/// Phase 1 stub.
pub struct SchedulerService;

impl SchedulerService {
    pub fn new() -> Self {
        Self
    }

    /// Start the scheduler, loading any persisted jobs.
    ///
    /// Phase 1 stub.
    pub async fn start(&self) -> Result<(), AgentError> {
        Err(AgentError::Scheduler(
            "Scheduler not yet implemented".into(),
        ))
    }

    /// Stop the scheduler gracefully.
    pub async fn stop(&self) -> Result<(), AgentError> {
        Ok(())
    }
}

impl Default for SchedulerService {
    fn default() -> Self {
        Self::new()
    }
}
