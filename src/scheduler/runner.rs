//! Scheduler runner — executes scheduled jobs.

use std::path::PathBuf;
use std::sync::Arc;
use tracing::info;

use genai::chat::{ChatMessage, MessageContent};

use crate::channel::{ChannelRegistry, OutboundMessage};
use crate::error::AgentError;
use crate::llm::LlmExecutor;
use crate::scheduler::models::JobContextPolicy;

pub struct RunScheduledJobInput {
    pub pool: sqlx::SqlitePool,
    pub llm: Arc<dyn LlmExecutor>,
    pub registry: Arc<crate::tools::registry::ToolRegistry>,
    pub channel_registry: Arc<ChannelRegistry>,
    pub loop_config: crate::agent::agent_loop::AgentLoopConfig,
    pub personality: String,
    pub workspace_root: PathBuf,
    pub access_policy: crate::channel::ChannelAccessPolicy,
    pub timezone: String,
}

pub async fn run_scheduled_job(
    input: RunScheduledJobInput,
    job_id: &str,
) -> Result<(), AgentError> {
    let RunScheduledJobInput {
        pool,
        llm,
        registry,
        channel_registry,
        loop_config,
        personality,
        workspace_root,
        access_policy,
        timezone,
    } = input;
    info!(job_id = %job_id, "retrieving job details for run");

    let job = crate::storage::jobs::get_job(&pool, job_id)
        .await?
        .ok_or_else(|| AgentError::Scheduler(format!("Job {} not found in database", job_id)))?;
    let owner_address = job.owner_address();

    let session =
        match crate::storage::sessions::get_session_for_address(&pool, &owner_address).await? {
            Some(s) => s,
            None => crate::storage::sessions::create_session(&pool, &owner_address).await?,
        };

    let policy = job.context_policy()?;
    let mut messages = match policy {
        JobContextPolicy::Isolated => vec![],
        JobContextPolicy::IncludeCreationSnapshot => {
            if let Some(ref snapshot) = job.creation_context_snapshot {
                serde_json::from_str::<Vec<ChatMessage>>(snapshot).map_err(|e| {
                    AgentError::Scheduler(format!(
                        "Corrupted context snapshot for job {job_id}: {e}"
                    ))
                })?
            } else {
                vec![]
            }
        }
        JobContextPolicy::IncludeChatSummary => {
            let mut msgs = vec![];
            if let Some(summary) =
                crate::storage::summaries::get_latest_summary(&pool, &session.id).await?
            {
                msgs.push(ChatMessage::system(MessageContent::from_text(format!(
                    "System Conversation Summary (covers older context):\n{}",
                    summary.summary_text
                ))));
            }
            msgs
        }
    };

    let user_msg = ChatMessage::user(MessageContent::from_text(&job.prompt));
    let _ = crate::storage::messages::create_message(&pool, &session.id, &user_msg, None).await?;
    messages.push(
        crate::context::manager::append_current_datetime_to_user_message(user_msg, &timezone),
    );

    let agent_ctx = crate::agent::agent_loop::AgentContext {
        run_mode: crate::agent::run_mode::AgentRunMode::ScheduledJob {
            job_id: job_id.to_string(),
            default_address: owner_address.clone(),
            notify_on_completion: job.notify_on_completion,
        },
        personality,
        messages,
        workspace_root,
        access_policy,
        channel_registry: Some(channel_registry.clone()),
        pool: Some(pool.clone()),
        session_id: session.id.clone(),
        scheduler_notifier: None,
    };

    let result = crate::agent::agent_loop::run_agent(
        &agent_ctx,
        llm.as_ref(),
        registry.as_ref(),
        &loop_config,
    )
    .await;

    match result {
        Ok(agent_result) => {
            let should_send_fallback =
                job.notify_on_completion && !agent_result.metadata.sent_user_message;

            match agent_result.outcome {
                crate::agent::outcome::AgentOutcome::FinalText(text) => {
                    let assistant_msg = ChatMessage::assistant(MessageContent::from_text(&text));
                    let _ = crate::storage::messages::create_message(
                        &pool,
                        &session.id,
                        &assistant_msg,
                        None,
                    )
                    .await?;

                    if should_send_fallback {
                        let notification = format!(
                            "🔔 **Job \"{}\" executed successfully**\n\n{}",
                            job.name, text
                        );
                        if let Err(e) = channel_registry
                            .send_message(&owner_address, OutboundMessage::markdown(notification))
                            .await
                        {
                            tracing::error!(job_id = %job_id, error = %e, "Failed to send success notification");
                        }
                    }
                }
                crate::agent::outcome::AgentOutcome::Silent => {
                    if should_send_fallback {
                        let notification =
                            format!("🔔 **Job \"{}\" completed with no output**", job.name);
                        let _ = channel_registry
                            .send_message(&owner_address, OutboundMessage::markdown(notification))
                            .await;
                    }
                }
            }
            Ok(())
        }
        Err(e) => {
            let err_notification = format!(
                "⚠️ **Job \"{}\" failed to execute**\n\nError: {}",
                job.name, e
            );
            if let Err(send_err) = channel_registry
                .send_message(&owner_address, OutboundMessage::markdown(err_notification))
                .await
            {
                tracing::error!(job_id = %job_id, error = %send_err, "Failed to send error notification");
            }
            Err(e)
        }
    }
}
