//! Scheduler runner — executes scheduled jobs.

use std::path::PathBuf;
use std::sync::Arc;
use tracing::info;

use genai::chat::{ChatMessage, ChatRole, ContentPart, MessageContent};

use crate::error::AgentError;
use crate::llm::LlmExecutor;
use crate::scheduler::models::JobContextPolicy;

/// Input arguments required to execute a scheduled job runner.
pub struct RunScheduledJobInput {
    pub pool: sqlx::SqlitePool,
    pub llm: Arc<dyn LlmExecutor>,
    pub registry: Arc<crate::tools::registry::ToolRegistry>,
    pub loop_config: crate::agent::agent_loop::AgentLoopConfig,
    pub personality: String,
    pub workspace_root: PathBuf,
    pub telegram_token: String,
    pub telegram_service: crate::telegram::service::TelegramService,
    pub allowed_chat_ids: Vec<i64>,
    pub allowed_user_ids: Vec<i64>,
    pub timezone: String,
}

/// Runs a scheduled job by assembling context, running the agent loop,
/// and notifying the user via Telegram on completion or error.
///
/// NOTE: The background job runner intentionally sets `scheduler_notifier` to `None` in the `AgentContext`
/// to prevent self-wake loops. For example, if a job's prompt instructs the agent to schedule
/// another job, the tool's wakeup call will be a no-op here since the runner is already executing.
pub async fn run_scheduled_job(
    input: RunScheduledJobInput,
    job_id: &str,
) -> Result<(), AgentError> {
    let RunScheduledJobInput {
        pool,
        llm,
        registry,
        loop_config,
        personality,
        workspace_root,
        telegram_token,
        telegram_service,
        allowed_chat_ids,
        allowed_user_ids,
        timezone,
    } = input;
    info!(job_id = %job_id, "retrieving job details for run");

    // 1. Get job details from SQLite
    let job = crate::storage::jobs::get_job(&pool, job_id)
        .await?
        .ok_or_else(|| AgentError::Scheduler(format!("Job {} not found in database", job_id)))?;

    // 2. Locate or create chat session
    let session =
        match crate::storage::sessions::get_session_for_chat(&pool, job.owner_chat_id).await? {
            Some(s) => s,
            None => crate::storage::sessions::create_session(&pool, job.owner_chat_id).await?,
        };

    // 3. Assemble message context according to policy
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

    // Save scheduled prompt to DB history and append to message context
    let user_msg = ChatMessage::user(MessageContent::from_text(&job.prompt));
    let _ = crate::storage::messages::create_message(&pool, &session.id, &user_msg, None).await?;
    messages.push(
        crate::context::manager::append_current_datetime_to_user_message(user_msg, &timezone),
    );

    // 4. Construct AgentContext
    let agent_ctx = crate::agent::agent_loop::AgentContext {
        run_mode: crate::agent::run_mode::AgentRunMode::ScheduledJob {
            job_id: job_id.to_string(),
            default_chat_id: job.owner_chat_id,
            notify_on_completion: job.notify_on_completion,
        },
        personality,
        messages,
        workspace_root,
        telegram_token: telegram_token.clone(),
        allowed_chat_ids,
        allowed_user_ids,
        pool: Some(pool.clone()),
        session_id: session.id.clone(),
        scheduler_notifier: None, // No immediate wakeup loop notifier inside background runner itself
    };

    // 5. Execute the agent loop
    let result = crate::agent::agent_loop::run_agent(
        &agent_ctx,
        llm.as_ref(),
        registry.as_ref(),
        &loop_config,
    )
    .await;

    match result {
        Ok(agent_result) => {
            if let crate::agent::outcome::AgentOutcome::FinalText(text) = agent_result.outcome {
                // Save assistant message to DB history
                let assistant_msg = ChatMessage::assistant(MessageContent::from_text(&text));
                let _ = crate::storage::messages::create_message(
                    &pool,
                    &session.id,
                    &assistant_msg,
                    None,
                )
                .await?;

                // Check whether the agent already sent a notification via send_user_message
                let agent_notified = agent_already_sent_notification(&pool, &session.id).await;

                if job.notify_on_completion && !agent_notified {
                    let notification = format!(
                        "🔔 **Job \"{}\" executed successfully**\n\n{}",
                        job.name, text
                    );
                    if let Err(e) = telegram_service
                        .send_message_with_options(
                            job.owner_chat_id,
                            &notification,
                            Some("MarkdownV2"),
                            None,
                        )
                        .await
                    {
                        tracing::error!(job_id = %job_id, error = %e, "Failed to send success notification");
                    }
                }
            } else if job.notify_on_completion {
                // Silent completion — check if agent already notified
                let agent_notified = agent_already_sent_notification(&pool, &session.id).await;
                if !agent_notified {
                    let notification =
                        format!("🔔 **Job \"{}\" completed with no output**", job.name);
                    let _ = telegram_service
                        .send_message_with_options(
                            job.owner_chat_id,
                            &notification,
                            Some("MarkdownV2"),
                            None,
                        )
                        .await;
                }
            }
            Ok(())
        }
        Err(e) => {
            // Notify Telegram of the failure (always, regardless of notify_on_completion)
            let err_notification = format!(
                "⚠️ **Job \"{}\" failed to execute**\n\nError: {}",
                job.name, e
            );
            if let Err(send_err) = telegram_service
                .send_message_with_options(
                    job.owner_chat_id,
                    &err_notification,
                    Some("MarkdownV2"),
                    None,
                )
                .await
            {
                tracing::error!(job_id = %job_id, error = %send_err, "Failed to send error notification");
            }
            Err(e)
        }
    }
}

/// Check whether the agent already sent a notification via `send_user_message` during this run.
///
/// Scans recent tool-result messages in the session for tool responses containing
/// `"send_user_message"` with `"sent": true` in the JSON content.
async fn agent_already_sent_notification(pool: &sqlx::SqlitePool, session_id: &str) -> bool {
    let stored = match crate::storage::messages::list_messages(pool, session_id, Some(50)).await {
        Ok(msgs) => msgs,
        Err(e) => {
            tracing::warn!(
                session_id = %session_id,
                error = %e,
                "failed to list messages for notification dedup, assuming not notified"
            );
            return false;
        }
    };

    for sm in stored {
        // We're looking for Tool-role messages
        if let Ok(role) = sm.role()
            && matches!(role, ChatRole::Tool)
        {
            // Try to parse as genai ContentPart format first
            if let Some(ref json) = sm.structured_content_json
                && let Ok(parts) = serde_json::from_value::<Vec<ContentPart>>(json.clone())
            {
                for part in parts {
                    if let ContentPart::ToolResponse(tr) = part
                        && let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&tr.content)
                    {
                        let is_send_user_message = parsed.get("tool_name").and_then(|v| v.as_str())
                            == Some("send_user_message");
                        let actually_sent =
                            parsed.get("sent").and_then(|v| v.as_bool()) == Some(true);
                        if is_send_user_message && actually_sent {
                            return true;
                        }
                    }
                }
            }
        }
    }

    false
}
