//! Scheduled job persistence.
//!
//! Implementations come in Phase 3.

use crate::scheduler::models::{JobContextPolicy, JobStatus, ScheduleType};
use serde::{Deserialize, Serialize};

/// A stored scheduled job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredJob {
    pub id: String,
    pub owner_chat_id: i64,
    pub name: String,
    pub prompt: String,
    pub schedule_type: ScheduleType,
    pub cron_expression: Option<String>,
    pub run_at: Option<chrono::DateTime<chrono::Utc>>,
    pub timezone: Option<String>,
    pub notify_on_completion: bool,
    pub context_policy: JobContextPolicy,
    pub creation_context_snapshot: Option<String>,
    pub enabled: bool,
    pub last_run_at: Option<chrono::DateTime<chrono::Utc>>,
    pub next_run_at: Option<chrono::DateTime<chrono::Utc>>,
    pub last_status: Option<JobStatus>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl StoredJob {
    pub fn new(
        owner_chat_id: i64,
        name: String,
        prompt: String,
        schedule_type: ScheduleType,
    ) -> Self {
        let now = chrono::Utc::now();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            owner_chat_id,
            name,
            prompt,
            schedule_type,
            cron_expression: None,
            run_at: None,
            timezone: None,
            notify_on_completion: false,
            context_policy: JobContextPolicy::default(),
            creation_context_snapshot: None,
            enabled: true,
            last_run_at: None,
            next_run_at: None,
            last_status: None,
            created_at: now,
            updated_at: now,
        }
    }
}
