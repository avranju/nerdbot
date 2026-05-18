//! Agent loop — the harness-controlled iterative execution engine.
//!
//! The loop continues until:
//! - The model returns final text (no tool calls)
//! - Max tool iterations are reached
//! - An unrecoverable error occurs

use tracing::{debug, error, info, instrument, warn};

use crate::agent::outcome::{AgentOutcome, AgentResult, RunMetadata};
use crate::agent::run_mode::AgentRunMode;
use crate::error::AgentError;
use crate::llm::provider::LlmProvider;
use crate::llm::types::ModelRequest;
use crate::tools::registry::ToolRegistry;
use crate::tools::traits::ToolContext;

/// Configuration for the agent loop.
#[derive(Debug, Clone)]
pub struct AgentLoopConfig {
    /// Maximum number of tool-loop iterations.
    pub max_tool_iterations: u32,
}

impl Default for AgentLoopConfig {
    fn default() -> Self {
        Self {
            max_tool_iterations: 10,
        }
    }
}

/// Run the agent loop for a single turn.
///
/// 1. Build the initial model request from context.
/// 2. Loop: send request to provider, execute tool calls, repeat.
/// 3. Return final text or an error.
///
/// Tool results are appended to the message history after each iteration,
/// allowing the provider to see tool outputs and respond accordingly.
#[instrument(skip(ctx, provider, registry, config), fields(run_mode = ?ctx.run_mode))]
pub async fn run_agent(
    ctx: &AgentContext,
    provider: &dyn LlmProvider,
    registry: &ToolRegistry,
    config: &AgentLoopConfig,
) -> Result<AgentResult, AgentError> {
    // Build the initial message set: personality as system message + user messages.
    let mut working_messages = ctx.messages.clone();
    if !ctx.personality.is_empty()
        && !working_messages
            .iter()
            .any(|m| matches!(m.role, crate::llm::types::Role::System))
    {
        working_messages.insert(0, crate::llm::types::Message::system(&ctx.personality));
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

        // Build the request for this iteration from working messages
        let request = ModelRequest::default()
            .with_messages(working_messages.clone())
            .with_tools(registry.specs());

        // Call the LLM
        let response = provider.complete(request).await?;

        // Track token usage
        if let Some(meta) = &response.provider_metadata
            && let Some(token_usage) = meta.get("usage") {
                if let Some(input_tokens) = token_usage.get("input_tokens").and_then(|t| t.as_u64())
                {
                    total_input_tokens += input_tokens as usize;
                }
                if let Some(output_tokens) =
                    token_usage.get("output_tokens").and_then(|t| t.as_u64())
                {
                    total_output_tokens += output_tokens as usize;
                }
            }

        debug!(
            step = step + 1,
            has_tool_calls = response.has_tool_calls(),
            assistant_text_present = response.assistant_text.is_some(),
            "LLM response"
        );

        // Check if the model returned final text (no tool calls)
        if !response.has_tool_calls() {
            let text = response.assistant_text.unwrap_or_default();
            info!(
                iterations = iterations,
                final_text_length = text.len(),
                "agent completed with final text"
            );

            return Ok(AgentResult {
                outcome: AgentOutcome::FinalText(text),
                metadata: RunMetadata {
                    iterations,
                    token_estimate: Some(crate::llm::types::TokenEstimate::new(
                        total_input_tokens,
                        total_output_tokens,
                    )),
                },
            });
        }

        // Check access before executing tool calls
        let chat_id = ctx.run_mode.chat_id();
        let user_id = ctx.run_mode.user_id();
        if let Some(cid) = chat_id
            && !ctx.allowed_chat_ids.is_empty() && !ctx.allowed_chat_ids.contains(&cid) {
                return Err(AgentError::PermissionDenied);
            }
        if let Some(uid) = user_id
            && !ctx.allowed_user_ids.is_empty() && !ctx.allowed_user_ids.contains(&uid) {
                return Err(AgentError::PermissionDenied);
            }

        // Execute tool calls and collect results
        let tool_ctx = ToolContext {
            run_mode: ctx.run_mode.clone(),
            workspace_root: ctx.workspace_root.clone(),
            telegram_token: ctx.telegram_token.clone(),
            allowed_chat_ids: ctx.allowed_chat_ids.clone(),
            allowed_user_ids: ctx.allowed_user_ids.clone(),
        };

        let mut tool_results = Vec::new();

        for tool_call in &response.tool_calls {
            match registry.execute(tool_call, tool_ctx.clone()).await {
                Ok(output) => {
                    debug!(
                        tool = tool_call.name,
                        tool_id = tool_call.id,
                        success = output.success,
                        summary = output.summary,
                        "tool call completed"
                    );
                    // Create a ToolResult for the message history
                    let status = if output.success {
                        crate::llm::types::ToolExecutionStatus::Success
                    } else {
                        crate::llm::types::ToolExecutionStatus::Error {
                            error: output.summary.clone(),
                        }
                    };
                    tool_results.push(crate::llm::types::ToolResult {
                        tool_call_id: tool_call.id.clone(),
                        status,
                        content: output.data,
                    });
                }
                Err(e) => {
                    warn!(
                        tool = tool_call.name,
                        tool_id = tool_call.id,
                        error = %e,
                        "tool call failed"
                    );
                    // Record the error as a tool result so the provider sees it
                    tool_results.push(crate::llm::types::ToolResult {
                        tool_call_id: tool_call.id.clone(),
                        status: crate::llm::types::ToolExecutionStatus::Error {
                            error: e.to_string(),
                        },
                        content: serde_json::json!({ "error": e.to_string() }),
                    });
                }
            }
        }

        // Append tool call messages and results to working messages
        // so the provider can see them on the next iteration.
        working_messages.push(crate::llm::types::Message::assistant_tool_calls(
            response.tool_calls.clone(),
        ));

        let results_count = tool_results.len();
        if !tool_results.is_empty() {
            working_messages.push(crate::llm::types::Message::with_tool_results(tool_results));
        }

        debug!(
            step = step + 1,
            tool_calls_count = response.tool_calls.len(),
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
    pub messages: Vec<crate::llm::types::Message>,
    /// Workspace root path.
    pub workspace_root: std::path::PathBuf,
    /// Telegram bot token.
    pub telegram_token: String,
    /// Allowed Telegram conversation IDs (chats, groups, channels).
    pub allowed_chat_ids: Vec<i64>,
    /// Allowed Telegram account IDs (individual users).
    pub allowed_user_ids: Vec<i64>,
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
        }
    }
}
