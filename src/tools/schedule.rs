//! Scheduling tools — schedule_job, list_jobs, delete_job, run_job_now.

use serde_json::json;

use crate::error::AgentError;
use crate::tools::traits::{Tool, ToolContext, ToolOutput};

use crate::scheduler::cron::get_next_cron_run;

pub struct ScheduleJob;

#[async_trait::async_trait]
impl Tool for ScheduleJob {
    fn name(&self) -> &'static str {
        "schedule_job"
    }
    fn description(&self) -> &'static str {
        "Schedule a one-shot or recurring agent task."
    }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "The name of the scheduled job"
                },
                "prompt": {
                    "type": "string",
                    "description": "The prompt/instruction for the agent to run"
                },
                "schedule_type": {
                    "type": "string",
                    "enum": ["one_shot", "cron"],
                    "description": "Whether the job runs once or on a recurring cron schedule"
                },
                "cron_expression": {
                    "type": "string",
                    "description": "Standard 5-field cron expression (e.g. '*/5 * * * *') required if schedule_type is 'cron'"
                },
                "run_at": {
                    "type": "string",
                    "description": "ISO-8601 UTC date-time string (e.g. '2026-05-20T15:00:00Z') required if schedule_type is 'one_shot'"
                },
                "timezone": {
                    "type": "string",
                    "description": "Target timezone name (e.g. 'Europe/Paris'). If omitted, cron schedules use the host system timezone detected at runtime, falling back to UTC if detection fails."
                },
                "notify_on_completion": {
                    "type": "boolean",
                    "description": "Whether to notify the user via Telegram when the job completes. Defaults to true."
                },
                "context_policy": {
                    "type": "string",
                    "enum": ["isolated", "include_creation_snapshot", "include_chat_summary"],
                    "description": "Context building policy for the job run. Defaults to 'include_creation_snapshot'."
                }
            },
            "required": ["name", "prompt", "schedule_type"]
        })
    }
    async fn execute(
        &self,
        args: serde_json::Value,
        ctx: ToolContext,
    ) -> Result<ToolOutput, AgentError> {
        let chat_id = ctx.run_mode.chat_id().ok_or_else(|| {
            AgentError::Generic("Can only schedule jobs from a valid Telegram chat session".into())
        })?;

        let name = args.get("name").and_then(|v| v.as_str()).ok_or_else(|| AgentError::Generic("name is required".into()))?.to_string();
        let prompt = args.get("prompt").and_then(|v| v.as_str()).ok_or_else(|| AgentError::Generic("prompt is required".into()))?.to_string();
        let schedule_type_str = args.get("schedule_type").and_then(|v| v.as_str()).ok_or_else(|| AgentError::Generic("schedule_type is required".into()))?;
        let timezone = args.get("timezone").and_then(|v| v.as_str()).map(|s| s.to_string());
        let notify_on_completion = args.get("notify_on_completion").and_then(|v| v.as_bool()).unwrap_or(true);
        let context_policy_str = args.get("context_policy").and_then(|v| v.as_str()).unwrap_or("include_creation_snapshot");

        let schedule_type = match schedule_type_str {
            "one_shot" => crate::scheduler::models::ScheduleType::OneShot,
            "cron" => crate::scheduler::models::ScheduleType::Cron,
            _ => return Err(AgentError::Generic(format!("Invalid schedule_type '{}'", schedule_type_str))),
        };

        let context_policy = match context_policy_str {
            "isolated" => crate::scheduler::models::JobContextPolicy::Isolated,
            "include_creation_snapshot" => crate::scheduler::models::JobContextPolicy::IncludeCreationSnapshot,
            "include_chat_summary" => crate::scheduler::models::JobContextPolicy::IncludeChatSummary,
            _ => return Err(AgentError::Generic(format!("Invalid context_policy '{}'", context_policy_str))),
        };

        let pool = match ctx.pool {
            Some(p) => p,
            None => {
                return Ok(ToolOutput {
                    success: true,
                    data: json!({ "job_id": "mock-job-id" }),
                    summary: "Scheduler not configured in this context, returned mock job creation".to_string(),
                });
            }
        };

        // Calculate next run time and inputs
        let mut run_at = None;
        let mut cron_expression = None;
        let next_run_at;

        match schedule_type {
            crate::scheduler::models::ScheduleType::OneShot => {
                let run_at_str = args.get("run_at").and_then(|v| v.as_str()).ok_or_else(|| AgentError::Generic("run_at is required for one_shot job".into()))?;
                let run_at_parsed = chrono::DateTime::parse_from_rfc3339(run_at_str)
                    .map_err(|e| AgentError::Generic(format!("Invalid ISO-8601 date-time '{}': {e}", run_at_str)))?
                    .with_timezone(&chrono::Utc);
                run_at = Some(run_at_parsed);
                next_run_at = Some(run_at_parsed);
            }
            crate::scheduler::models::ScheduleType::Cron => {
                let cron_str = args.get("cron_expression").and_then(|v| v.as_str()).ok_or_else(|| AgentError::Generic("cron_expression is required for cron job".into()))?;
                next_run_at = Some(get_next_cron_run(cron_str, timezone.as_deref())?);
                cron_expression = Some(cron_str.to_string());
            }
        }

        // Assemble creation context snapshot if policy requests it
        let creation_context_snapshot = if context_policy == crate::scheduler::models::JobContextPolicy::IncludeCreationSnapshot {
            if let Some(session) = crate::storage::sessions::get_session_for_chat(&pool, chat_id).await? {
                let stored = crate::storage::messages::list_messages(&pool, &session.id, Some(30)).await?;
                let messages: Vec<genai::chat::ChatMessage> = stored
                    .into_iter()
                    .rev() // DESC to ASC
                    .filter_map(|sm| sm.to_message().ok())
                    .collect();
                Some(serde_json::to_string(&messages).unwrap_or_default())
            } else {
                None
            }
        } else {
            None
        };

        let job = crate::storage::jobs::create_job_full(
            &pool,
            crate::storage::jobs::CreateJobInput {
                owner_chat_id: chat_id,
                name: name.clone(),
                prompt,
                schedule_type,
                cron_expression,
                run_at,
                timezone,
                notify_on_completion,
                context_policy,
                creation_context_snapshot,
                next_run_at,
            },
        ).await?;

        // Immediately trigger wakeup of the scheduler service
        if let Some(notifier) = ctx.scheduler_notifier {
            notifier.notify_one();
        }

        Ok(ToolOutput {
            success: true,
            summary: format!("Successfully scheduled job \"{}\" with ID `{}`.", name, job.id),
            data: json!({ "job_id": job.id, "next_run_at": next_run_at }),
        })
    }
}

pub struct ListJobs;

#[async_trait::async_trait]
impl Tool for ListJobs {
    fn name(&self) -> &'static str {
        "list_jobs"
    }
    fn description(&self) -> &'static str {
        "List scheduled jobs."
    }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {}
        })
    }
    async fn execute(
        &self,
        _args: serde_json::Value,
        ctx: ToolContext,
    ) -> Result<ToolOutput, AgentError> {
        let chat_id = ctx.run_mode.chat_id().ok_or_else(|| {
            AgentError::Generic("Can only list jobs from a valid Telegram chat session".into())
        })?;

        let pool = match ctx.pool {
            Some(p) => p,
            None => {
                return Ok(ToolOutput {
                    success: true,
                    data: json!([]),
                    summary: "Scheduler not configured in this context, returned empty list".to_string(),
                });
            }
        };

        let jobs = crate::storage::jobs::list_jobs(&pool, chat_id, false).await?;
        let summary = if jobs.is_empty() {
            "No scheduled jobs found.".to_string()
        } else {
            let mut s = format!("Found {} scheduled jobs:\n", jobs.len());
            for job in &jobs {
                let status_str = if job.enabled { "active" } else { "disabled" };
                let next_str = job.next_run_at
                    .map(|t| t.format("%Y-%m-%d %H:%M UTC").to_string())
                    .unwrap_or_else(|| "none".to_string());
                s.push_str(&format!("- \"{}\" (id: `{}`, status: {}, next run: {})\n", job.name, job.id, status_str, next_str));
            }
            s
        };

        Ok(ToolOutput {
            success: true,
            summary,
            data: json!(jobs),
        })
    }
}

pub struct DeleteJob;

#[async_trait::async_trait]
impl Tool for DeleteJob {
    fn name(&self) -> &'static str {
        "delete_job"
    }
    fn description(&self) -> &'static str {
        "Delete a scheduled job."
    }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "job_id": {
                    "type": "string",
                    "description": "The unique ID of the scheduled job to delete"
                }
            },
            "required": ["job_id"]
        })
    }
    async fn execute(
        &self,
        args: serde_json::Value,
        ctx: ToolContext,
    ) -> Result<ToolOutput, AgentError> {
        let chat_id = ctx.run_mode.chat_id().ok_or_else(|| {
            AgentError::Generic("Can only delete jobs from a valid Telegram chat session".into())
        })?;

        let job_id = args.get("job_id").and_then(|v| v.as_str()).ok_or_else(|| {
            AgentError::Generic("job_id is required".into())
        })?;

        let pool = match ctx.pool {
            Some(p) => p,
            None => {
                return Ok(ToolOutput {
                    success: true,
                    data: json!({ "job_id": job_id }),
                    summary: format!("Scheduler not configured in this context, mock deleted job `{}`", job_id),
                });
            }
        };

        let job = crate::storage::jobs::get_job(&pool, job_id).await?;
        match job {
            None => Err(AgentError::Generic(format!("No job found with ID `{job_id}`."))),
            Some(job) if job.owner_chat_id != chat_id => {
                Err(AgentError::Generic("That job belongs to a different chat session.".into()))
            }
            Some(_) => {
                crate::storage::jobs::disable_job(&pool, job_id).await?;
                
                // Immediately trigger wakeup of the scheduler service to adjust sleep timer
                if let Some(notifier) = ctx.scheduler_notifier {
                    notifier.notify_one();
                }

                Ok(ToolOutput {
                    success: true,
                    summary: format!("Job `{}` successfully deleted.", job_id),
                    data: json!({ "job_id": job_id }),
                })
            }
        }
    }
}

pub struct RunJobNow;

#[async_trait::async_trait]
impl Tool for RunJobNow {
    fn name(&self) -> &'static str {
        "run_job_now"
    }
    fn description(&self) -> &'static str {
        "Immediately trigger a scheduled job once."
    }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "job_id": {
                    "type": "string",
                    "description": "The unique ID of the scheduled job to run immediately"
                }
            },
            "required": ["job_id"]
        })
    }
    async fn execute(
        &self,
        args: serde_json::Value,
        ctx: ToolContext,
    ) -> Result<ToolOutput, AgentError> {
        let chat_id = ctx.run_mode.chat_id().ok_or_else(|| {
            AgentError::Generic("Can only trigger jobs from a valid Telegram chat session".into())
        })?;

        let job_id = args.get("job_id").and_then(|v| v.as_str()).ok_or_else(|| {
            AgentError::Generic("job_id is required".into())
        })?;

        let pool = match ctx.pool {
            Some(p) => p,
            None => {
                return Ok(ToolOutput {
                    success: true,
                    data: json!({ "job_id": job_id }),
                    summary: format!("Scheduler not configured in this context, mock triggered job `{}`", job_id),
                });
            }
        };

        let job = crate::storage::jobs::get_job(&pool, job_id).await?;
        match job {
            None => Err(AgentError::Generic(format!("No job found with ID `{job_id}`."))),
            Some(job) if job.owner_chat_id != chat_id => {
                Err(AgentError::Generic("That job belongs to a different chat session.".into()))
            }
            Some(job) if !job.enabled => {
                Err(AgentError::Generic(format!("Job `{job_id}` is disabled or deleted and cannot be run.")))
            }
            Some(_) => {
                // Update next_run_at to now and make sure it is enabled
                crate::storage::jobs::update_job_next_run(&pool, job_id, Some(chrono::Utc::now()), true).await?;

                // Immediately trigger wakeup of the scheduler service to execute it instantly
                if let Some(notifier) = ctx.scheduler_notifier {
                    notifier.notify_one();
                }

                Ok(ToolOutput {
                    success: true,
                    summary: format!("Job `{}` successfully scheduled to run immediately.", job_id),
                    data: json!({ "job_id": job_id }),
                })
            }
        }
    }
}
