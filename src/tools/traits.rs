//! Tool trait — all tools must implement this interface.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::AgentError;

/// Shared context available to all tool executions.
#[derive(Debug, Clone)]
pub struct ToolContext {
    /// The run mode determining how this tool is being called.
    pub run_mode: crate::agent::run_mode::AgentRunMode,
    /// Workspace root for file-based tools.
    pub workspace_root: std::path::PathBuf,
    /// Telegram bot token (for messaging tools).
    pub telegram_token: String,
    /// Allowed conversation IDs (private chats, groups, channels).
    pub allowed_chat_ids: Vec<i64>,
    /// Allowed account IDs (individual Telegram users).
    pub allowed_user_ids: Vec<i64>,
    /// Database pool for tools needing access to storage (like scheduling)
    pub pool: Option<sqlx::SqlitePool>,
    /// Notifier to wake up the scheduler service loop instantly
    pub scheduler_notifier: Option<std::sync::Arc<tokio::sync::Notify>>,
}

/// Output from a tool execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolOutput {
    /// Success indicator.
    pub success: bool,
    /// Structured result data.
    pub data: Value,
    /// Human-readable summary.
    pub summary: String,
}

/// All tools must implement this trait.
#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    /// Unique name of the tool (must match what the LLM will call).
    fn name(&self) -> &'static str;

    /// Human-readable description of what the tool does.
    fn description(&self) -> &'static str;

    /// JSON Schema describing the tool's expected input arguments.
    fn input_schema(&self) -> Value;

    /// Execute the tool with the given arguments.
    async fn execute(&self, args: Value, ctx: ToolContext) -> Result<ToolOutput, AgentError>;
}

/// A boxed tool, stored as Arc for cheap cloning.
pub type BoxedTool = Arc<dyn Tool>;
