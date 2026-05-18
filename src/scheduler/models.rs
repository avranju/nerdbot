//! Scheduler data models.
//!
//! Implementations come in Phase 5.

use serde::{Deserialize, Serialize};

/// How a scheduled job should be executed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum JobContextPolicy {
    /// Job runs in isolation (no chat context).
    Isolated,
    /// Job includes a snapshot of context from when it was created.
    #[default]
    IncludeCreationSnapshot,
    /// Job includes a summary of the current chat context.
    IncludeChatSummary,
}


/// Type of schedule for a job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleType {
    OneShot,
    Cron,
}

/// Status of a scheduled job.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Pending,
    Running,
    Success,
    Failed,
    Missed,
}
