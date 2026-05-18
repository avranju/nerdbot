//! Configuration loading from TOML and environment variables.

use std::path::PathBuf;

use serde::Deserialize;

use crate::error::AgentError;

/// Top-level configuration.
#[derive(Debug, Deserialize, Default)]
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
    pub providers: ProvidersConfig,
    #[serde(default)]
    pub context: ContextConfig,
    #[serde(default)]
    pub scheduler: SchedulerConfig,
}

/// Agent-specific configuration.
#[derive(Debug, Deserialize)]
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
#[derive(Debug, Deserialize)]
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
#[derive(Debug, Deserialize)]
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
#[derive(Debug, Deserialize)]
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

/// LLM default configuration.
#[derive(Debug, Deserialize)]
pub struct LlmConfig {
    #[serde(default = "default_llm_provider")]
    pub provider: String,
    #[serde(default)]
    pub model: String,
    #[serde(default = "default_temperature")]
    pub temperature: f32,
    #[serde(default = "default_max_output_tokens")]
    pub max_output_tokens: u32,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            provider: default_llm_provider(),
            model: String::new(),
            temperature: default_temperature(),
            max_output_tokens: default_max_output_tokens(),
        }
    }
}

/// Per-provider configuration.
#[derive(Debug, Deserialize, Default)]
pub struct ProvidersConfig {
    #[serde(default)]
    pub anthropic: AnthropicProviderConfig,
    #[serde(default)]
    pub openai: OpenaiProviderConfig,
    #[serde(default)]
    pub gemini: GeminiProviderConfig,
    #[serde(default)]
    pub openrouter: OpenrouterProviderConfig,
}

#[derive(Debug, Deserialize)]
pub struct AnthropicProviderConfig {
    #[serde(default = "default_api_key_env")]
    pub api_key_env: String,
}

impl Default for AnthropicProviderConfig {
    fn default() -> Self {
        Self {
            api_key_env: default_api_key_env(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct OpenaiProviderConfig {
    #[serde(default = "default_api_key_env")]
    pub api_key_env: String,
}

impl Default for OpenaiProviderConfig {
    fn default() -> Self {
        Self {
            api_key_env: default_api_key_env(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct GeminiProviderConfig {
    #[serde(default = "default_api_key_env")]
    pub api_key_env: String,
}

impl Default for GeminiProviderConfig {
    fn default() -> Self {
        Self {
            api_key_env: default_api_key_env(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct OpenrouterProviderConfig {
    #[serde(default = "default_api_key_env")]
    pub api_key_env: String,
    #[serde(default = "default_openrouter_base_url")]
    pub base_url: String,
}

impl Default for OpenrouterProviderConfig {
    fn default() -> Self {
        Self {
            api_key_env: default_api_key_env(),
            base_url: default_openrouter_base_url(),
        }
    }
}

/// Context management configuration.
#[derive(Debug, Deserialize)]
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

#[derive(Debug, Deserialize, Default)]
pub struct CompactorConfig {
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub model: String,
}

/// Scheduler configuration.
#[derive(Debug, Deserialize, Default)]
pub struct SchedulerConfig {
    #[serde(default)]
    pub run_overdue_one_shots_on_startup: bool,
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
fn default_api_key_env() -> String {
    "API_KEY".to_string()
}
fn default_openrouter_base_url() -> String {
    "https://openrouter.ai/api/v1".to_string()
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
fn default_llm_provider() -> String {
    "anthropic".to_string()
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
