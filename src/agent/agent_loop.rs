//! Agent loop — the harness-controlled iterative execution engine.
//!
//! The loop continues until:
//! - The model returns final text (no tool calls)
//! - Max tool iterations are reached
//! - An unrecoverable error occurs

use genai::chat::{ChatMessage, ChatRequest, ChatRole, ContentPart, MessageContent, ToolResponse};
use tracing::{debug, error, info, instrument, warn};

use crate::agent::outcome::{AgentOutcome, AgentResult, RunMetadata, RunTokenUsage};
use crate::agent::run_mode::AgentRunMode;
use crate::error::AgentError;
use crate::llm::LlmExecutor;
use crate::tools::registry::ToolRegistry;
use crate::tools::traits::ToolContext;

/// Configuration for the agent loop.
#[derive(Debug, Clone)]
pub struct AgentLoopConfig {
    /// Maximum number of tool-loop iterations.
    pub max_tool_iterations: u32,
    /// LLM model name to use for requests.
    pub llm_model: String,
    /// LLM temperature to use for requests.
    pub llm_temperature: f32,
    /// LLM max output tokens to use for requests.
    pub llm_max_output_tokens: u32,
}

impl Default for AgentLoopConfig {
    fn default() -> Self {
        Self {
            max_tool_iterations: 10,
            llm_model: String::new(),
            llm_temperature: 0.0,
            llm_max_output_tokens: 0,
        }
    }
}

/// Run the agent loop for a single turn.
///
/// The messages parameter should already be assembled by [`crate::context::manager::ContextManager`]
/// with bounded context (summary + recent messages). This function appends
/// personality as system message (if not already present) and tool specs.
///
/// 1. Loop: send request to provider, execute tool calls, repeat.
/// 2. Return final text or an error.
#[instrument(skip(ctx, executor, registry, config), fields(run_mode = ?ctx.run_mode))]
pub async fn run_agent(
    ctx: &AgentContext,
    executor: &dyn LlmExecutor,
    registry: &ToolRegistry,
    config: &AgentLoopConfig,
) -> Result<AgentResult, AgentError> {
    let mut working_messages = ctx.messages.clone();

    // Append personality as system message if not already present.
    // ContextManager may have already included a summary as system message;
    // personality is prepended before that.
    if !ctx.personality.is_empty()
        && !working_messages
            .iter()
            .any(|m| matches!(m.role, ChatRole::System))
    {
        working_messages.insert(
            0,
            ChatMessage::system(MessageContent::from_text(&ctx.personality)),
        );
    }

    let mut total_input_tokens: usize = 0;
    let mut total_output_tokens: usize = 0;

    debug!(
        iterations_limit = config.max_tool_iterations,
        tools_count = registry.len(),
        message_count = working_messages.len(),
        "starting agent loop"
    );

    for step in 0..config.max_tool_iterations {
        let iterations = step + 1;

        // Build the ChatRequest for this iteration
        let mut request = ChatRequest::new(working_messages.clone());

        if !registry.is_empty() {
            request = request.with_tools(registry.specs());
        }

        // Build chat options
        let chat_options = genai::chat::ChatOptions::default()
            .with_temperature(config.llm_temperature as f64)
            .with_max_tokens(config.llm_max_output_tokens);

        // Call the LLM
        let response = executor
            .complete(&config.llm_model, request, chat_options)
            .await?;

        // Track token usage from genai response
        if let Some(input) = response.usage.prompt_tokens {
            total_input_tokens += input.max(0) as usize;
        }
        if let Some(output) = response.usage.completion_tokens {
            total_output_tokens += output.max(0) as usize;
        }

        let tool_calls: Vec<genai::chat::ToolCall> =
            response.tool_calls().into_iter().cloned().collect();
        let final_text = response.content.joined_texts();

        debug!(
            step = step + 1,
            has_tool_calls = !tool_calls.is_empty(),
            assistant_text_present = final_text
                .as_deref()
                .map(|s| !s.is_empty())
                .unwrap_or(false),
            "LLM response"
        );

        // Check if the model returned final text (no tool calls)
        if tool_calls.is_empty() {
            match &final_text {
                Some(text) if !text.is_empty() => {
                    info!(
                        iterations = iterations,
                        final_text_length = text.len(),
                        "agent completed with final text"
                    );

                    return Ok(AgentResult {
                        outcome: AgentOutcome::FinalText(text.clone()),
                        metadata: RunMetadata {
                            iterations,
                            token_usage: RunTokenUsage {
                                input_tokens: total_input_tokens,
                                output_tokens: total_output_tokens,
                                total_tokens: total_input_tokens + total_output_tokens,
                            },
                        },
                    });
                }
                _ => {
                    // No tool calls and no text (or empty text) — silent completion
                    info!(
                        iterations = iterations,
                        "agent completed silently (no tool calls, no final text)"
                    );

                    return Ok(AgentResult {
                        outcome: AgentOutcome::Silent,
                        metadata: RunMetadata {
                            iterations,
                            token_usage: RunTokenUsage {
                                input_tokens: total_input_tokens,
                                output_tokens: total_output_tokens,
                                total_tokens: total_input_tokens + total_output_tokens,
                            },
                        },
                    });
                }
            }
        }

        // Check access before executing tool calls
        let chat_id = ctx.run_mode.chat_id();
        let user_id = ctx.run_mode.user_id();
        if let Some(cid) = chat_id
            && !ctx.allowed_chat_ids.is_empty()
            && !ctx.allowed_chat_ids.contains(&cid)
        {
            return Err(AgentError::PermissionDenied);
        }
        if let Some(uid) = user_id
            && !ctx.allowed_user_ids.is_empty()
            && !ctx.allowed_user_ids.contains(&uid)
        {
            return Err(AgentError::PermissionDenied);
        }

        // Execute tool calls and collect results
        let tool_ctx = ToolContext {
            run_mode: ctx.run_mode.clone(),
            workspace_root: ctx.workspace_root.clone(),
            telegram_token: ctx.telegram_token.clone(),
            allowed_chat_ids: ctx.allowed_chat_ids.clone(),
            allowed_user_ids: ctx.allowed_user_ids.clone(),
            pool: ctx.pool.clone(),
            scheduler_notifier: ctx.scheduler_notifier.clone(),
        };

        let mut tool_responses = Vec::new();

        for tool_call in &tool_calls {
            match registry.execute(tool_call, tool_ctx.clone()).await {
                Ok(output) => {
                    debug!(
                        tool = tool_call.fn_name,
                        tool_id = tool_call.call_id,
                        success = output.success,
                        summary = output.summary,
                        "tool call completed"
                    );
                    // Serialize the tool output as a JSON string for the ToolResponse
                    let content = serde_json::to_string(&serde_json::json!({
                        "tool_name": tool_call.fn_name,
                        "success": output.success,
                        "summary": output.summary,
                        "data": output.data,
                        "sent": output.data.get("sent").and_then(|v| v.as_bool()),
                    }))
                    .unwrap_or_else(|_| "{\"error\":\"failed to serialize output\"}".to_string());

                    tool_responses.push(ToolResponse::from_tool_call(tool_call, content));
                }
                Err(e) => {
                    warn!(
                        tool = tool_call.fn_name,
                        tool_id = tool_call.call_id,
                        error = %e,
                        "tool call failed"
                    );
                    let content = serde_json::to_string(&serde_json::json!({
                        "tool_name": tool_call.fn_name,
                        "success": false,
                        "summary": e.to_string(),
                        "error": e.to_string(),
                    }))
                    .unwrap_or_else(|_| format!("{{\"error\":\"{e}\"}}"));

                    tool_responses.push(ToolResponse::from_tool_call(tool_call, content));
                }
            }
        }

        // Append tool call messages and results to working messages.
        // genai provides ChatMessage::from(Vec<ToolCall>) for assistant tool-use messages.
        let tool_calls_count = tool_calls.len();
        working_messages.push(ChatMessage::from(tool_calls));

        let results_count = tool_responses.len();
        if !tool_responses.is_empty() {
            // Convert tool responses into a Tool-role message
            let tool_message = ChatMessage::from(tool_responses.clone());
            working_messages.push(tool_message.clone());

            // Persist tool results to the database so callers (e.g. scheduler)
            // can inspect them for notification deduplication.
            if !ctx.session_id.is_empty()
                && let Some(ref pool) = ctx.pool
                && let Err(e) = crate::storage::messages::create_message(
                    pool,
                    &ctx.session_id,
                    &tool_message,
                    None,
                )
                .await
            {
                warn!(
                    step = step + 1,
                    error = %e,
                    "failed to persist tool result messages"
                );
            }
        }

        debug!(
            step = step + 1,
            tool_calls_count,
            results_count,
            message_count = working_messages.len(),
            "tool calls executed, continuing loop"
        );
    }

    // Max iterations exceeded
    error!(
        max_iterations = config.max_tool_iterations,
        "agent loop exceeded max tool iterations"
    );

    Err(AgentError::MaxToolIterationsExceeded(
        config.max_tool_iterations,
    ))
}

/// Context passed to the agent loop.
///
/// Contains everything the loop needs to build requests and execute tools.
pub struct AgentContext {
    /// The run mode (interactive, scheduled, internal).
    pub run_mode: AgentRunMode,
    /// Personality/system prompt content.
    pub personality: String,
    /// Current conversation messages (for context assembly).
    pub messages: Vec<ChatMessage>,
    /// Workspace root path.
    pub workspace_root: std::path::PathBuf,
    /// Telegram bot token.
    pub telegram_token: String,
    /// Allowed Telegram conversation IDs (chats, groups, channels).
    pub allowed_chat_ids: Vec<i64>,
    /// Allowed Telegram account IDs (individual users).
    pub allowed_user_ids: Vec<i64>,
    /// Database pool for tools needing access to storage
    pub pool: Option<sqlx::SqlitePool>,
    /// Chat session ID for persisting tool results.
    /// When non-empty, tool-result messages are persisted to the database.
    pub session_id: String,
    /// Notifier to wake up the scheduler service loop instantly
    pub scheduler_notifier: Option<std::sync::Arc<tokio::sync::Notify>>,
}

impl AgentContext {
    pub fn new(
        run_mode: AgentRunMode,
        personality: String,
        workspace_root: std::path::PathBuf,
        telegram_token: String,
        allowed_chat_ids: Vec<i64>,
        allowed_user_ids: Vec<i64>,
    ) -> Self {
        Self {
            run_mode,
            personality,
            messages: Vec::new(),
            workspace_root,
            telegram_token,
            allowed_chat_ids,
            allowed_user_ids,
            pool: None,
            session_id: String::new(),
            scheduler_notifier: None,
        }
    }
}
