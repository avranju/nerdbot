//! Scheduler runner — executes scheduled jobs.
//!
//! Implementations come in Phase 5.

use crate::error::AgentError;

/// Runs a scheduled job.
///
/// Phase 1 stub.
pub async fn run_scheduled_job(
    _job_id: &str,
    _prompt: &str,
) -> Result<(), AgentError> {
    Err(AgentError::Scheduler("Scheduler not yet implemented".into()))
}
