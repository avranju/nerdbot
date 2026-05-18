//! Scheduled job persistence — CRUD operations via SQLx.

use chrono::DateTime;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::SqlitePool;
use tracing::debug;

use crate::error::AgentError;
use crate::scheduler::models::{JobContextPolicy, JobStatus, ScheduleType};

/// A stored job row from the database.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct StoredJob {
    pub id: String,
    pub owner_chat_id: i64,
    pub name: String,
    pub prompt: String,
    pub schedule_type: Value,
    pub cron_expression: Option<String>,
    pub run_at: Option<DateTime<chrono::Utc>>,
    pub timezone: Option<String>,
    pub notify_on_completion: bool,
    pub context_policy: Value,
    pub creation_context_snapshot: Option<String>,
    pub enabled: bool,
    pub last_run_at: Option<DateTime<chrono::Utc>>,
    pub next_run_at: Option<DateTime<chrono::Utc>>,
    pub last_status: Option<Value>,
    pub created_at: DateTime<chrono::Utc>,
    pub updated_at: DateTime<chrono::Utc>,
}

impl StoredJob {
    /// Convert stored fields back to their typed representations.
    pub fn schedule_type(&self) -> Result<ScheduleType, AgentError> {
        serde_json::from_value(self.schedule_type.clone())
            .map_err(|e| AgentError::Storage(format!("Failed to deserialize schedule_type: {e}")))
    }

    pub fn context_policy(&self) -> Result<JobContextPolicy, AgentError> {
        serde_json::from_value(self.context_policy.clone())
            .map_err(|e| AgentError::Storage(format!("Failed to deserialize context_policy: {e}")))
    }

    pub fn last_status(&self) -> Result<Option<JobStatus>, AgentError> {
        self.last_status
            .as_ref()
            .map(|v| {
                serde_json::from_value(v.clone()).map_err(|e| {
                    AgentError::Storage(format!("Failed to deserialize last_status: {e}"))
                })
            })
            .transpose()
    }
}

impl StoredJob {
    /// Create a new stored job (convenience constructor for tests).
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
            schedule_type: serde_json::to_value(&schedule_type).unwrap_or(Value::Null),
            cron_expression: None,
            run_at: None,
            timezone: None,
            notify_on_completion: false,
            context_policy: serde_json::to_value(JobContextPolicy::default())
                .unwrap_or(Value::Null),
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

/// Create a job and insert it into the database.
pub async fn create_job(
    pool: &SqlitePool,
    owner_chat_id: i64,
    name: String,
    prompt: String,
    schedule_type: ScheduleType,
    next_run_at: Option<DateTime<chrono::Utc>>,
) -> Result<StoredJob, AgentError> {
    let now = chrono::Utc::now();
    let id = uuid::Uuid::new_v4().to_string();

    sqlx::query(
        r#"
        INSERT INTO scheduled_jobs (id, owner_chat_id, name, prompt, schedule_type, cron_expression, run_at, timezone, notify_on_completion, context_policy, enabled, created_at, updated_at, next_run_at)
        VALUES (?1, ?2, ?3, ?4, ?5, NULL, NULL, NULL, 0, ?6, 1, ?7, ?8, ?9)
        "#,
    )
    .bind(&id)
    .bind(owner_chat_id)
    .bind(&name)
    .bind(&prompt)
    .bind(serde_json::to_value(&schedule_type).unwrap_or(Value::Null))
    .bind(serde_json::to_value(JobContextPolicy::default()).unwrap_or(Value::Null))
    .bind(now.to_rfc3339())
    .bind(now.to_rfc3339())
    .bind(next_run_at.map(|t| t.to_rfc3339()))
    .execute(pool)
    .await
    .map_err(|e| AgentError::Storage(format!("Failed to create job: {e}")))?;

    debug!(job_id = %id, "created scheduled job");
    Ok(StoredJob {
        id,
        owner_chat_id,
        name,
        prompt,
        schedule_type: serde_json::to_value(&schedule_type).unwrap_or(Value::Null),
        cron_expression: None,
        run_at: None,
        timezone: None,
        notify_on_completion: false,
        context_policy: serde_json::to_value(JobContextPolicy::default()).unwrap_or(Value::Null),
        creation_context_snapshot: None,
        enabled: true,
        last_run_at: None,
        next_run_at,
        last_status: None,
        created_at: now,
        updated_at: now,
    })
}

/// Get a job by ID.
pub async fn get_job(pool: &SqlitePool, job_id: &str) -> Result<Option<StoredJob>, AgentError> {
    let row = sqlx::query_as::<_, StoredJob>(
        "SELECT id, owner_chat_id, name, prompt, schedule_type, cron_expression, run_at, timezone, notify_on_completion, context_policy, creation_context_snapshot, enabled, last_run_at, next_run_at, last_status, created_at, updated_at FROM scheduled_jobs WHERE id = ?1",
    )
    .bind(job_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| AgentError::Storage(format!("Failed to get job: {e}")))?;
    Ok(row)
}

/// List jobs for a chat, ordered by next run.
pub async fn list_jobs(
    pool: &SqlitePool,
    owner_chat_id: i64,
    enabled_only: bool,
) -> Result<Vec<StoredJob>, AgentError> {
    let query = if enabled_only {
        "SELECT id, owner_chat_id, name, prompt, schedule_type, cron_expression, run_at, timezone, notify_on_completion, context_policy, creation_context_snapshot, enabled, last_run_at, next_run_at, last_status, created_at, updated_at FROM scheduled_jobs WHERE owner_chat_id = ?1 AND enabled = 1 ORDER BY next_run_at ASC"
    } else {
        "SELECT id, owner_chat_id, name, prompt, schedule_type, cron_expression, run_at, timezone, notify_on_completion, context_policy, creation_context_snapshot, enabled, last_run_at, next_run_at, last_status, created_at, updated_at FROM scheduled_jobs WHERE owner_chat_id = ?1 ORDER BY next_run_at ASC"
    };

    sqlx::query_as::<_, StoredJob>(query)
        .bind(owner_chat_id)
        .fetch_all(pool)
        .await
        .map_err(|e| AgentError::Storage(format!("Failed to list jobs: {e}")))
}

/// Disable a job by setting enabled = 0.
pub async fn disable_job(pool: &SqlitePool, job_id: &str) -> Result<(), AgentError> {
    sqlx::query("UPDATE scheduled_jobs SET enabled = 0, updated_at = ?1 WHERE id = ?2")
        .bind(chrono::Utc::now().to_rfc3339())
        .bind(job_id)
        .execute(pool)
        .await
        .map_err(|e| AgentError::Storage(format!("Failed to disable job: {e}")))?;

    Ok(())
}
