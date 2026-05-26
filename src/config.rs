//! Configuration loading from TOML and environment variables.

use std::path::PathBuf;

use serde::Deserialize;

use crate::error::AgentError;

/// Top-level configuration.
#[derive(Debug, Deserialize, Clone, Default)]
pub struct AppConfig {
    #[serde(default)]
    pub agent: AgentConfig,
    #[serde(default)]
    pub telegram: TelegramConfig,
    #[serde(default)]
    pub storage: StorageConfig,
    #[serde(default)]
    pub workspace: WorkspaceConfig,
    #[serde(default)]
    pub llm: LlmConfig,
    #[serde(default)]
    pub context: ContextConfig,
    #[serde(default)]
    pub scheduler: SchedulerConfig,
    #[serde(default)]
    pub shell: ShellConfig,
    #[serde(default)]
    pub files: FilesConfig,
    #[serde(default)]
    pub exa: ExaConfig,
}

/// Agent-specific configuration.
#[derive(Debug, Deserialize, Clone)]
pub struct AgentConfig {
    #[serde(default = "default_agent_name")]
    pub name: String,
    #[serde(default = "default_personality_file")]
    pub personality_file: PathBuf,
    #[serde(default = "default_max_tool_iterations")]
    pub max_tool_iterations: u32,
    #[serde(default = "default_timezone")]
    pub default_timezone: String,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            name: default_agent_name(),
            personality_file: default_personality_file(),
            max_tool_iterations: default_max_tool_iterations(),
            default_timezone: default_timezone(),
        }
    }
}

/// Telegram-specific configuration.
#[derive(Debug, Deserialize, Clone)]
pub struct TelegramConfig {
    /// Environment variable name holding the bot token.
    #[serde(default = "default_telegram_token_env")]
    pub bot_token_env: String,
    /// Allowed Telegram **conversation** IDs (private chats, groups, channels).
    /// Messages from chats not in this list are ignored. If empty, all chats
    /// are allowed (useful for local development).
    #[serde(default)]
    pub allowed_chat_ids: Vec<i64>,
    /// Allowed Telegram **account** (user) IDs. Messages from users not in
    /// this list are ignored regardless of which chat they send from.
    /// If empty, all users are allowed (useful for local development).
    #[serde(default)]
    pub allowed_user_ids: Vec<i64>,
}

impl Default for TelegramConfig {
    fn default() -> Self {
        Self {
            bot_token_env: default_telegram_token_env(),
            allowed_chat_ids: Vec::new(),
            allowed_user_ids: Vec::new(),
        }
    }
}

/// Storage configuration.
#[derive(Debug, Deserialize, Clone)]
pub struct StorageConfig {
    #[serde(default = "default_sqlite_path")]
    pub sqlite_path: PathBuf,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            sqlite_path: default_sqlite_path(),
        }
    }
}

/// Workspace configuration.
#[derive(Debug, Deserialize, Clone)]
pub struct WorkspaceConfig {
    #[serde(default = "default_workspace_root")]
    pub root: PathBuf,
    #[serde(default = "default_max_read_bytes")]
    pub max_read_bytes: usize,
    #[serde(default = "default_max_write_bytes")]
    pub max_write_bytes: usize,
}

impl Default for WorkspaceConfig {
    fn default() -> Self {
        Self {
            root: default_workspace_root(),
            max_read_bytes: default_max_read_bytes(),
            max_write_bytes: default_max_write_bytes(),
        }
    }
}

/// LLM configuration.
///
/// Uses `genai` for provider resolution. The `model` field determines
/// which provider is used (e.g., "gpt-4o" → OpenAI, "claude-sonnet-4-5" → Anthropic,
/// "gemini-2.5-flash" → Gemini, "open_router::openai/gpt-4.1" → OpenRouter).
///
/// For custom OpenAI-compatible endpoints, set `endpoint` and optionally
/// `api_key_env` to override the default auth.
#[derive(Debug, Deserialize, Clone)]
pub struct LlmConfig {
    /// Model name (e.g., "gpt-4o", "claude-sonnet-4-5", "gemini-2.5-flash").
    /// `genai` resolves the provider from the model name prefix.
    #[serde(default)]
    pub model: String,
    /// Optional custom endpoint URL (for OpenAI-compatible APIs).
    /// When set, overrides the default provider endpoint.
    #[serde(default)]
    pub endpoint: Option<String>,
    /// Optional environment variable name for the API key.
    /// When set, overrides the default auth for the resolved provider.
    #[serde(default)]
    pub api_key_env: Option<String>,
    /// Sampling temperature.
    #[serde(default = "default_temperature")]
    pub temperature: f32,
    /// Maximum output tokens.
    #[serde(default = "default_max_output_tokens")]
    pub max_output_tokens: u32,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            model: String::new(),
            endpoint: None,
            api_key_env: None,
            temperature: default_temperature(),
            max_output_tokens: default_max_output_tokens(),
        }
    }
}

/// Context management configuration.
#[derive(Debug, Deserialize, Clone)]
pub struct ContextConfig {
    #[serde(default = "default_soft_compaction_threshold")]
    pub soft_compaction_threshold: f32,
    #[serde(default = "default_hard_context_threshold")]
    pub hard_context_threshold: f32,
    #[serde(default = "default_recent_turns_to_preserve")]
    pub recent_turns_to_preserve: usize,
    #[serde(default)]
    pub compactor: CompactorConfig,
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            soft_compaction_threshold: default_soft_compaction_threshold(),
            hard_context_threshold: default_hard_context_threshold(),
            recent_turns_to_preserve: default_recent_turns_to_preserve(),
            compactor: CompactorConfig::default(),
        }
    }
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct CompactorConfig {
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub model: String,
}

/// Scheduler configuration.
#[derive(Debug, Deserialize, Clone, Default)]
pub struct SchedulerConfig {
    #[serde(default)]
    pub run_overdue_one_shots_on_startup: bool,
}

/// File I/O tool configuration.
#[derive(Debug, Deserialize, Clone)]
pub struct FilesConfig {
    /// Maximum size in bytes for file read operations.
    #[serde(default = "default_files_max_read_bytes")]
    pub max_read_bytes: usize,
    /// Maximum size in bytes for file write operations.
    #[serde(default = "default_files_max_write_bytes")]
    pub max_write_bytes: usize,
}

impl Default for FilesConfig {
    fn default() -> Self {
        Self {
            max_read_bytes: default_files_max_read_bytes(),
            max_write_bytes: default_files_max_write_bytes(),
        }
    }
}

/// Sandbox mode for shell command execution.
pub const SANDBOX_MODE_NONE: &str = "none";
pub const SANDBOX_MODE_BWRAP: &str = "bwrap";
pub const SANDBOX_MODE_BWRAP_STRICT: &str = "bwrap-strict";

/// Shell execution configuration.
#[derive(Debug, Deserialize, Clone)]
pub struct ShellConfig {
    /// Allowed commands (empty means allow all). Commands not in this list are blocked.
    #[serde(default = "default_shell_allowed_commands")]
    pub allowed_commands: Vec<String>,
    /// Denied commands that are always blocked, regardless of allowlist.
    #[serde(default = "default_shell_denied_commands")]
    pub denied_commands: Vec<String>,
    /// Maximum output size in bytes.
    #[serde(default = "default_shell_max_output_bytes")]
    pub max_output_bytes: usize,
    /// Command execution timeout in seconds.
    #[serde(default = "default_shell_timeout_secs")]
    pub timeout_secs: u64,
    /// Sandbox isolation mode.
    ///
    /// - `none`: Direct execution (current behavior, no namespace isolation).
    /// - `bwrap`: Bubblewrap namespace isolation — filesystem, PID, network,
    ///   IPC, and UTS namespaces. System files are read-only; workspace is
    ///   read-write. No network access.
    /// - `bwrap-strict`: Reserved for future resource limit enforcement
    ///   (--rlimit-nproc, --rlimit-as, --rlimit-core). Currently identical
    ///   to `bwrap`; the distinction is a hook for when bwrap supports these
    ///   flags.
    ///
    /// When `bwrap` or `bwrap-strict` is selected and bubblewrap is not
    /// installed, the tool returns an error explaining how to install it.
    #[serde(default = "default_shell_sandbox_mode")]
    pub sandbox_mode: String,
}

/// Exa web search configuration.
#[derive(Debug, Deserialize, Clone)]
pub struct ExaConfig {
    /// Environment variable name holding the Exa API key.
    #[serde(default = "default_exa_api_key_env")]
    pub api_key_env: String,
    /// Maximum number of search results to return.
    #[serde(default = "default_exa_num_results")]
    pub max_results: usize,
    /// Maximum characters per page text when fetching content.
    #[serde(default = "default_exa_max_text_chars")]
    pub max_text_chars: usize,
}

impl Default for ExaConfig {
    fn default() -> Self {
        Self {
            api_key_env: default_exa_api_key_env(),
            max_results: default_exa_num_results(),
            max_text_chars: default_exa_max_text_chars(),
        }
    }
}

impl Default for ShellConfig {
    fn default() -> Self {
        Self {
            allowed_commands: default_shell_allowed_commands(),
            denied_commands: default_shell_denied_commands(),
            max_output_bytes: default_shell_max_output_bytes(),
            timeout_secs: default_shell_timeout_secs(),
            sandbox_mode: default_shell_sandbox_mode(),
        }
    }
}

// Default functions for serde defaults

fn default_agent_name() -> String {
    "nerdbot".to_string()
}
fn default_personality_file() -> PathBuf {
    PathBuf::from("/config/personality.md")
}
fn default_max_tool_iterations() -> u32 {
    10
}
fn default_timezone() -> String {
    "UTC".to_string()
}
fn default_telegram_token_env() -> String {
    "TELEGRAM_BOT_TOKEN".to_string()
}
fn default_sqlite_path() -> PathBuf {
    PathBuf::from("/data/agent.db")
}
fn default_workspace_root() -> PathBuf {
    PathBuf::from("/workspace")
}
fn default_max_read_bytes() -> usize {
    262_144
}
fn default_max_write_bytes() -> usize {
    262_144
}
fn default_temperature() -> f32 {
    0.2
}
fn default_max_output_tokens() -> u32 {
    4096
}
fn default_soft_compaction_threshold() -> f32 {
    0.60
}
fn default_hard_context_threshold() -> f32 {
    0.85
}
fn default_recent_turns_to_preserve() -> usize {
    30
}
fn default_files_max_read_bytes() -> usize {
    262_144
}
fn default_files_max_write_bytes() -> usize {
    262_144
}
fn default_shell_allowed_commands() -> Vec<String> {
    Vec::new()
}
fn default_shell_denied_commands() -> Vec<String> {
    vec![
        "rm".into(),
        "chmod".into(),
        "chown".into(),
        "mkfs".into(),
        "dd".into(),
        "wget".into(),
        "curl".into(),
    ]
}
fn default_shell_max_output_bytes() -> usize {
    1_048_576 // 1 MB
}
fn default_shell_timeout_secs() -> u64 {
    30
}
fn default_shell_sandbox_mode() -> String {
    SANDBOX_MODE_NONE.into()
}
fn default_exa_api_key_env() -> String {
    "EXA_API_KEY".to_string()
}
fn default_exa_num_results() -> usize {
    5
}
fn default_exa_max_text_chars() -> usize {
    8000
}
impl AppConfig {
    /// Load configuration from a TOML file.
    pub fn from_file(path: &std::path::Path) -> Result<Self, AgentError> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| AgentError::Config(format!("Failed to read config file: {}", e)))?;

        let config: AppConfig = toml::from_str(&content)
            .map_err(|e| AgentError::Config(format!("Failed to parse config: {}", e)))?;

        tracing::info!(config_path = %path.display(), "loaded configuration");
        Ok(config)
    }
}
