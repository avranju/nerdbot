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
///
/// NOTE: `sqlx::FromRow` maps database columns by name to the struct fields.
/// The explicit column list in the query matches `StoredJob` fields precisely.
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
///
/// NOTE: `sqlx::FromRow` maps database columns by name to the struct fields.
/// The explicit column list in the query matches `StoredJob` fields precisely.
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

/// List all enabled jobs across all chats.
///
/// NOTE: `sqlx::FromRow` maps database columns by name to the struct fields.
/// The explicit column list in the query matches `StoredJob` fields precisely.
pub async fn list_all_enabled_jobs(pool: &SqlitePool) -> Result<Vec<StoredJob>, AgentError> {
    sqlx::query_as::<_, StoredJob>(
        "SELECT id, owner_chat_id, name, prompt, schedule_type, cron_expression, run_at, timezone, notify_on_completion, context_policy, creation_context_snapshot, enabled, last_run_at, next_run_at, last_status, created_at, updated_at FROM scheduled_jobs WHERE enabled = 1 ORDER BY next_run_at ASC"
    )
    .fetch_all(pool)
    .await
    .map_err(|e| AgentError::Storage(format!("Failed to list all enabled jobs: {e}")))
}

/// Retrieve the next upcoming enabled job.
///
/// NOTE: `sqlx::FromRow` maps database columns by name to the struct fields.
/// The explicit column list in the query matches `StoredJob` fields precisely.
pub async fn get_next_enabled_job(pool: &SqlitePool) -> Result<Option<StoredJob>, AgentError> {
    sqlx::query_as::<_, StoredJob>(
        "SELECT id, owner_chat_id, name, prompt, schedule_type, cron_expression, run_at, timezone, notify_on_completion, context_policy, creation_context_snapshot, enabled, last_run_at, next_run_at, last_status, created_at, updated_at FROM scheduled_jobs WHERE enabled = 1 ORDER BY next_run_at ASC LIMIT 1"
    )
    .fetch_optional(pool)
    .await
    .map_err(|e| AgentError::Storage(format!("Failed to get next enabled job: {e}")))
}

/// Inputs required to create a new scheduled job.
pub struct CreateJobInput {
    pub owner_chat_id: i64,
    pub name: String,
    pub prompt: String,
    pub schedule_type: ScheduleType,
    pub cron_expression: Option<String>,
    pub run_at: Option<DateTime<chrono::Utc>>,
    pub timezone: Option<String>,
    pub notify_on_completion: bool,
    pub context_policy: JobContextPolicy,
    pub creation_context_snapshot: Option<String>,
    pub next_run_at: Option<DateTime<chrono::Utc>>,
}

/// Create a fully parameterized job and insert it into the database.
pub async fn create_job_full(
    pool: &SqlitePool,
    input: CreateJobInput,
) -> Result<StoredJob, AgentError> {
    let now = chrono::Utc::now();
    let id = uuid::Uuid::new_v4().to_string();

    let schedule_type_val = serde_json::to_value(&input.schedule_type)
        .map_err(|e| AgentError::Storage(format!("Failed to serialize schedule_type: {e}")))?;
    let context_policy_val = serde_json::to_value(&input.context_policy)
        .map_err(|e| AgentError::Storage(format!("Failed to serialize context_policy: {e}")))?;

    sqlx::query(
        r#"
        INSERT INTO scheduled_jobs (
            id, owner_chat_id, name, prompt, schedule_type, cron_expression, run_at,
            timezone, notify_on_completion, context_policy, creation_context_snapshot,
            enabled, created_at, updated_at, next_run_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 1, ?12, ?13, ?14)
        "#,
    )
    .bind(&id)
    .bind(input.owner_chat_id)
    .bind(&input.name)
    .bind(&input.prompt)
    .bind(&schedule_type_val)
    .bind(&input.cron_expression)
    .bind(input.run_at.map(|t| t.to_rfc3339()))
    .bind(&input.timezone)
    .bind(input.notify_on_completion)
    .bind(&context_policy_val)
    .bind(&input.creation_context_snapshot)
    .bind(now.to_rfc3339())
    .bind(now.to_rfc3339())
    .bind(input.next_run_at.map(|t| t.to_rfc3339()))
    .execute(pool)
    .await
    .map_err(|e| AgentError::Storage(format!("Failed to create job: {e}")))?;

    debug!(job_id = %id, "created fully parameterized scheduled job");
    Ok(StoredJob {
        id,
        owner_chat_id: input.owner_chat_id,
        name: input.name,
        prompt: input.prompt,
        schedule_type: schedule_type_val,
        cron_expression: input.cron_expression,
        run_at: input.run_at,
        timezone: input.timezone,
        notify_on_completion: input.notify_on_completion,
        context_policy: context_policy_val,
        creation_context_snapshot: input.creation_context_snapshot,
        enabled: true,
        last_run_at: None,
        next_run_at: input.next_run_at,
        last_status: None,
        created_at: now,
        updated_at: now,
    })
}

/// Update a job's execution state after running it.
pub async fn update_job_run_state(
    pool: &SqlitePool,
    job_id: &str,
    status: JobStatus,
    last_run_at: DateTime<chrono::Utc>,
    next_run_at: Option<DateTime<chrono::Utc>>,
    enabled: bool,
) -> Result<(), AgentError> {
    sqlx::query(
        r#"
        UPDATE scheduled_jobs
        SET last_status = ?1, last_run_at = ?2, next_run_at = ?3, enabled = ?4, updated_at = ?5
        WHERE id = ?6
        "#,
    )
    .bind(serde_json::to_value(&status).unwrap_or(Value::Null))
    .bind(last_run_at.to_rfc3339())
    .bind(next_run_at.map(|t| t.to_rfc3339()))
    .bind(enabled)
    .bind(chrono::Utc::now().to_rfc3339())
    .bind(job_id)
    .execute(pool)
    .await
    .map_err(|e| AgentError::Storage(format!("Failed to update job run state: {e}")))?;

    Ok(())
}

/// Update next run time and enabled state of a job.
pub async fn update_job_next_run(
    pool: &SqlitePool,
    job_id: &str,
    next_run_at: Option<DateTime<chrono::Utc>>,
    enabled: bool,
) -> Result<(), AgentError> {
    sqlx::query(
        r#"
        UPDATE scheduled_jobs
        SET next_run_at = ?1, enabled = ?2, updated_at = ?3
        WHERE id = ?4
        "#,
    )
    .bind(next_run_at.map(|t| t.to_rfc3339()))
    .bind(enabled)
    .bind(chrono::Utc::now().to_rfc3339())
    .bind(job_id)
    .execute(pool)
    .await
    .map_err(|e| AgentError::Storage(format!("Failed to update job next run: {e}")))?;

    Ok(())
}
