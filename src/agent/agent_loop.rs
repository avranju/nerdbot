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
use crate::tools::traits::ToolContext;
use crate::tools::registry::ToolRegistry;

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
#[instrument(skip(ctx, provider, registry, config), fields(run_mode = ?ctx.run_mode))]
pub async fn run_agent(
    ctx: &AgentContext,
    provider: &dyn LlmProvider,
    registry: &ToolRegistry,
    config: &AgentLoopConfig,
) -> Result<AgentResult, AgentError> {
    // Build the initial model request with personality, context, and tools
    let request = build_model_request(ctx, registry).await?;

    let mut total_input_tokens: usize = 0;
    let mut total_output_tokens: usize = 0;

    debug!(
        iterations_limit = config.max_tool_iterations,
        tools_count = registry.len(),
        "starting agent loop"
    );

    for step in 0..config.max_tool_iterations {
        let iterations = step + 1;

        // Call the LLM
        let response = provider.complete(request.clone()).await?;

        // Track token usage
        if let Some(meta) = &response.provider_metadata {
            if let Some(token_usage) = meta.get("usage") {
                if let Some(input_tokens) = token_usage.get("input_tokens").and_then(|t| t.as_u64()) {
                    total_input_tokens += input_tokens as usize;
                }
                if let Some(output_tokens) = token_usage.get("output_tokens").and_then(|t| t.as_u64()) {
                    total_output_tokens += output_tokens as usize;
                }
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
        if let Some(cid) = chat_id {
            if !ctx.allowed_chat_ids.is_empty() && !ctx.allowed_chat_ids.contains(&cid) {
                return Err(AgentError::PermissionDenied);
            }
        }
        if let Some(uid) = user_id {
            if !ctx.allowed_user_ids.is_empty() && !ctx.allowed_user_ids.contains(&uid) {
                return Err(AgentError::PermissionDenied);
            }
        }

        // Execute tool calls
        let tool_ctx = ToolContext {
            run_mode: ctx.run_mode.clone(),
            workspace_root: ctx.workspace_root.clone(),
            telegram_token: ctx.telegram_token.clone(),
            allowed_chat_ids: ctx.allowed_chat_ids.clone(),
            allowed_user_ids: ctx.allowed_user_ids.clone(),
        };

        for tool_call in &response.tool_calls {
            match registry.execute(tool_call, tool_ctx.clone()) {
                Ok(output) => {
                    debug!(
                        tool = tool_call.name,
                        tool_id = tool_call.id,
                        success = output.success,
                        "tool call completed"
                    );
                }
                Err(e) => {
                    warn!(
                        tool = tool_call.name,
                        tool_id = tool_call.id,
                        error = %e,
                        "tool call failed"
                    );
                }
            }
        }

        // Prepare request for next iteration with tool results
        // In a full implementation, this would append tool results to the message history
        // For Phase 1 skeleton, we track that tool calls were made
        debug!(
            step = step + 1,
            tool_calls_count = response.tool_calls.len(),
            "tool calls executed, continuing loop"
        );
    }

    // Max iterations exceeded
    error!(
        max_iterations = config.max_tool_iterations,
        "agent loop exceeded max tool iterations"
    );

    Err(AgentError::MaxToolIterationsExceeded(config.max_tool_iterations))
}

/// Build a model request from the current agent context.
///
/// Phase 1 skeleton — full implementation will include:
/// - Personality prompt injection
/// - Context assembly (summary + recent turns)
/// - Tool specs from the registry
async fn build_model_request(
    _ctx: &AgentContext,
    registry: &ToolRegistry,
) -> Result<ModelRequest, AgentError> {
    let tools = registry.specs();

    // Phase 1: minimal request with tools
    // Full implementation will inject personality, conversation history, etc.
    let request = ModelRequest::default()
        .with_tools(tools)
        .with_messages(vec![]); // Will be filled in later phases

    Ok(request)
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
