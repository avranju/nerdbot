//! Configuration loading from TOML and environment variables.

use std::path::PathBuf;

use serde::Deserialize;
use url::Url;

use crate::channel::ChannelConfig;
use crate::error::AgentError;

pub const DEFAULT_TELEGRAM_POLL_INTERVAL_SECS: u64 = 5;
pub const DEFAULT_ZULIP_PRESENCE_PING_INTERVAL_SECS: u64 = 60;

/// Top-level configuration.
#[derive(Debug, Deserialize, Clone, Default)]
pub struct AppConfig {
    #[serde(default)]
    pub agent: AgentConfig,
    #[serde(default)]
    pub channels: ChannelConfig,
    #[serde(default)]
    pub webhook: WebhookConfig,
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
    #[serde(default)]
    pub mcp: McpConfig,
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

/// Telegram channel configuration.
#[derive(Debug, Deserialize, Clone)]
pub struct TelegramChannelConfig {
    /// Whether the Telegram channel should start.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Telegram ingress transport: long polling or webhook push.
    #[serde(default)]
    pub ingress: TelegramIngress,
    /// Environment variable name holding the bot token.
    #[serde(default = "default_telegram_token_env")]
    pub bot_token_env: String,
    /// Public HTTPS webhook URL registered with Telegram in push mode.
    #[serde(default)]
    pub web_hook_url: Option<String>,
    /// Sleep duration after an empty getUpdates response in poll mode.
    #[serde(default = "default_telegram_poll_interval_secs")]
    pub poll_interval_secs: u64,
    /// Allowed Telegram **conversation** IDs (private chats, groups, channels).
    /// Messages from chats not in this list are ignored. If empty, all chats
    /// are allowed (useful for local development).
    #[serde(default)]
    pub allowed_conversations: Vec<String>,
    /// Allowed Telegram **account** (user) IDs. Messages from users not in
    /// this list are ignored regardless of which chat they send from.
    /// If empty, all users are allowed (useful for local development).
    #[serde(default)]
    pub allowed_senders: Vec<String>,
    /// Maximum size in bytes for downloaded Telegram attachments (images, PDFs, etc.).
    /// Files larger than this limit are rejected before download begins.
    #[serde(default = "default_max_attachment_bytes")]
    pub max_attachment_bytes: usize,
    /// Maximum number of characters when extracting text from text documents.
    /// Text documents exceeding this limit are truncated with a notice.
    #[serde(default = "default_max_text_document_chars")]
    pub max_text_document_chars: usize,
}

impl Default for TelegramChannelConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            ingress: TelegramIngress::Poll,
            bot_token_env: default_telegram_token_env(),
            web_hook_url: None,
            poll_interval_secs: default_telegram_poll_interval_secs(),
            allowed_conversations: Vec::new(),
            allowed_senders: Vec::new(),
            max_attachment_bytes: default_max_attachment_bytes(),
            max_text_document_chars: default_max_text_document_chars(),
        }
    }
}

/// Telegram update ingress mode.
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum TelegramIngress {
    /// Receive updates through Telegram's getUpdates long-polling API.
    #[default]
    Poll,
    /// Receive updates through Telegram webhooks.
    Webhook,
}

/// Zulip update ingress mode.
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ZulipIngress {
    /// Receive updates through Zulip's event queue long-polling API.
    #[default]
    Poll,
    /// Receive updates through Zulip outgoing webhooks.
    Webhook,
}

/// Zulip channel configuration.
#[derive(Debug, Deserialize, Clone)]
pub struct ZulipChannelConfig {
    /// Whether the Zulip channel should start.
    #[serde(default = "default_false")]
    pub enabled: bool,
    /// Zulip ingress transport: long polling or webhook push.
    #[serde(default)]
    pub ingress: ZulipIngress,
    /// Environment variable name holding the bot's email address.
    #[serde(default = "default_zulip_bot_email_env")]
    pub bot_email_env: String,
    /// Environment variable name holding the bot's API key.
    #[serde(default = "default_zulip_api_key_env")]
    pub api_key_env: String,
    /// Base URL of the Zulip server (e.g., https://your-org.zulipchat.com).
    #[serde(default)]
    pub site_url: String,
    /// Environment variable name holding the webhook verification token.
    #[serde(default = "default_zulip_webhook_token_env")]
    pub web_hook_token_env: String,
    /// Public Zulip webhook URL registered with Zulip's outgoing webhook settings.
    #[serde(default)]
    pub web_hook_url: Option<String>,
    /// Sleep duration after an empty events response in poll mode.
    #[serde(default = "default_zulip_poll_interval_secs")]
    pub poll_interval_secs: u64,
    /// Whether to attempt active presence pings while NerdBot is running.
    ///
    /// Zulip rejects this endpoint for bot accounts on current servers, so the
    /// default is disabled.
    #[serde(default = "default_false")]
    pub presence_enabled: bool,
    /// Interval for active presence pings.
    #[serde(default = "default_zulip_presence_ping_interval_secs")]
    pub presence_ping_interval_secs: u64,
    /// Allowed Zulip conversations (stream names + optional topics).
    #[serde(default)]
    pub allowed_conversations: Vec<crate::channel::types::ConversationAddressPattern>,
    /// Allowed Zulip sender email addresses.
    #[serde(default)]
    pub allowed_senders: Vec<String>,
    /// Maximum size in bytes for downloaded Zulip attachments.
    #[serde(default = "default_max_attachment_bytes")]
    pub max_attachment_bytes: usize,
    /// Maximum number of characters when extracting text from text documents.
    #[serde(default = "default_max_text_document_chars")]
    pub max_text_document_chars: usize,
}

impl Default for ZulipChannelConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            ingress: ZulipIngress::Poll,
            bot_email_env: default_zulip_bot_email_env(),
            api_key_env: default_zulip_api_key_env(),
            site_url: String::new(),
            web_hook_token_env: default_zulip_webhook_token_env(),
            web_hook_url: None,
            poll_interval_secs: default_zulip_poll_interval_secs(),
            presence_enabled: false,
            presence_ping_interval_secs: default_zulip_presence_ping_interval_secs(),
            allowed_conversations: Vec::new(),
            allowed_senders: Vec::new(),
            max_attachment_bytes: default_max_attachment_bytes(),
            max_text_document_chars: default_max_text_document_chars(),
        }
    }
}

/// Shared webhook HTTP server configuration.
#[derive(Debug, Deserialize, Clone)]
pub struct WebhookConfig {
    /// Local interface used by the shared webhook HTTP server.
    #[serde(default = "default_webhook_host")]
    pub host: String,
    /// Local port used by the shared webhook HTTP server.
    #[serde(default = "default_webhook_port")]
    pub port: u16,
}

impl Default for WebhookConfig {
    fn default() -> Self {
        Self {
            host: default_webhook_host(),
            port: default_webhook_port(),
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
    /// Total context window size in tokens (model-specific).
    #[serde(default = "default_context_window_tokens")]
    pub context_window_tokens: usize,
    /// Number of retry attempts after a transient LLM network failure.
    #[serde(default = "default_llm_max_retries")]
    pub max_retries: u32,
    /// Delay between transient LLM network retry attempts, in seconds.
    #[serde(default = "default_llm_retry_interval_secs")]
    pub retry_interval_secs: u64,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            model: String::new(),
            endpoint: None,
            api_key_env: None,
            temperature: default_temperature(),
            max_output_tokens: default_max_output_tokens(),
            context_window_tokens: default_context_window_tokens(),
            max_retries: default_llm_max_retries(),
            retry_interval_secs: default_llm_retry_interval_secs(),
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
    /// Tokens reserved for tool-loop headroom (iterative tool calls).
    #[serde(default = "default_reserved_tool_loop_tokens")]
    pub reserved_tool_loop_tokens: usize,
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            soft_compaction_threshold: default_soft_compaction_threshold(),
            hard_context_threshold: default_hard_context_threshold(),
            recent_turns_to_preserve: default_recent_turns_to_preserve(),
            reserved_tool_loop_tokens: default_reserved_tool_loop_tokens(),
        }
    }
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
pub const SHELL_NETWORK_ACCESS_DISABLED: &str = "disabled";
pub const SHELL_NETWORK_ACCESS_HOST: &str = "host";

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
    ///   Allow/deny lists only inspect the initial command word and are not a
    ///   security boundary against shell features or interpreter subcommands.
    /// - `bwrap`: Bubblewrap namespace isolation — filesystem, PID, network,
    ///   IPC, and UTS namespaces. System files are read-only; workspace is
    ///   read-write. Network access is controlled by `network_access`.
    /// - `bwrap-strict`: Reserved for future resource limit enforcement
    ///   (--rlimit-nproc, --rlimit-as, --rlimit-core). Currently identical
    ///   to `bwrap`; the distinction is a hook for when bwrap supports these
    ///   flags.
    ///
    /// When `bwrap` or `bwrap-strict` is selected and bubblewrap is not
    /// installed, the tool returns an error explaining how to install it.
    #[serde(default = "default_shell_sandbox_mode")]
    pub sandbox_mode: String,
    /// Network access policy for bubblewrap sandboxing.
    ///
    /// - `disabled`: Unshare the network namespace; loopback-only, no external access.
    /// - `host`: Share the host network namespace; commands can make outbound connections.
    ///
    /// This setting only affects `bwrap` and `bwrap-strict` sandbox modes.
    #[serde(default = "default_shell_network_access")]
    pub network_access: String,
}

/// Runtime MCP extension configuration.
#[derive(Debug, Deserialize, Clone, Default)]
pub struct McpConfig {
    #[serde(default)]
    pub servers: Vec<McpServerConfig>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct McpServerConfig {
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub required: bool,
    pub transport: McpTransportConfig,
    #[serde(default)]
    pub tool_prefix: Option<String>,
    #[serde(default)]
    pub include_tools: Vec<String>,
    #[serde(default)]
    pub exclude_tools: Vec<String>,
    #[serde(default = "default_mcp_connect_timeout_secs")]
    pub connect_timeout_secs: u64,
    #[serde(default = "default_mcp_call_timeout_secs")]
    pub call_timeout_secs: u64,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum McpTransportConfig {
    StreamableHttp {
        url: String,
        #[serde(default)]
        bearer_token_env: Option<String>,
    },
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
            network_access: default_shell_network_access(),
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
fn default_true() -> bool {
    true
}
fn default_webhook_host() -> String {
    "127.0.0.1".to_string()
}
fn default_webhook_port() -> u16 {
    24_682
}
fn default_telegram_poll_interval_secs() -> u64 {
    DEFAULT_TELEGRAM_POLL_INTERVAL_SECS
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
fn default_context_window_tokens() -> usize {
    128_000
}
fn default_llm_max_retries() -> u32 {
    4
}
fn default_llm_retry_interval_secs() -> u64 {
    10
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
fn default_reserved_tool_loop_tokens() -> usize {
    8_192
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
fn default_shell_network_access() -> String {
    SHELL_NETWORK_ACCESS_DISABLED.into()
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
fn default_mcp_connect_timeout_secs() -> u64 {
    10
}
fn default_mcp_call_timeout_secs() -> u64 {
    60
}
fn default_max_attachment_bytes() -> usize {
    5_242_880 // 5 MB
}
fn default_max_text_document_chars() -> usize {
    32_768 // 32 KB
}

pub const MCP_TOOL_NAME_MAX_LEN: usize = 64;

pub fn is_valid_tool_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
}

fn default_false() -> bool {
    false
}
fn default_zulip_bot_email_env() -> String {
    "ZULIP_BOT_EMAIL".to_string()
}
fn default_zulip_api_key_env() -> String {
    "ZULIP_BOT_API_KEY".to_string()
}
fn default_zulip_webhook_token_env() -> String {
    "ZULIP_WEBHOOK_TOKEN".to_string()
}
fn default_zulip_poll_interval_secs() -> u64 {
    2
}
fn default_zulip_presence_ping_interval_secs() -> u64 {
    DEFAULT_ZULIP_PRESENCE_PING_INTERVAL_SECS
}
impl AppConfig {
    /// Load configuration from a TOML file.
    pub fn from_file(path: &std::path::Path) -> Result<Self, AgentError> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| AgentError::Config(format!("Failed to read config file: {}", e)))?;

        let config: AppConfig = toml::from_str(&content)
            .map_err(|e| AgentError::Config(format!("Failed to parse config: {}", e)))?;
        config.validate()?;

        tracing::info!(config_path = %path.display(), "loaded configuration");
        Ok(config)
    }

    /// Validate cross-field configuration constraints after TOML defaults are applied.
    pub fn validate(&self) -> Result<(), AgentError> {
        let telegram = &self.channels.telegram;
        let telegram_webhook_enabled =
            telegram.enabled && telegram.ingress == TelegramIngress::Webhook;

        if let Some(conversation_id) = telegram
            .allowed_conversations
            .iter()
            .find(|id| id.trim().is_empty())
        {
            return Err(AgentError::Config(format!(
                "channels.telegram.allowed_conversations must not contain blank IDs, got {conversation_id:?}"
            )));
        }

        if let Some(sender_id) = telegram
            .allowed_senders
            .iter()
            .find(|id| id.trim().is_empty())
        {
            return Err(AgentError::Config(format!(
                "channels.telegram.allowed_senders must not contain blank IDs, got {sender_id:?}"
            )));
        }

        if telegram_webhook_enabled {
            let Some(web_hook_url) = telegram
                .web_hook_url
                .as_deref()
                .map(str::trim)
                .filter(|url| !url.is_empty())
            else {
                return Err(AgentError::Config(
                    "channels.telegram.web_hook_url is required when channels.telegram.ingress is \"webhook\"".into(),
                ));
            };

            let parsed = Url::parse(web_hook_url).map_err(|e| {
                AgentError::Config(format!(
                    "channels.telegram.web_hook_url is not a valid URL: {e}"
                ))
            })?;
            if parsed.scheme() != "https" {
                return Err(AgentError::Config(format!(
                    "channels.telegram.web_hook_url must use https, got {:?}",
                    parsed.scheme()
                )));
            }
            if parsed.host_str().is_none() {
                return Err(AgentError::Config(
                    "channels.telegram.web_hook_url must include a host".into(),
                ));
            }
        }

        let zulip = &self.channels.zulip;
        let mut zulip_webhook_enabled = false;

        if zulip.enabled {
            let site_url = zulip.site_url.trim();
            if site_url.is_empty() {
                return Err(AgentError::Config(
                    "channels.zulip.site_url is required when channels.zulip.enabled is true"
                        .into(),
                ));
            }
            let parsed = Url::parse(site_url).map_err(|e| {
                AgentError::Config(format!("channels.zulip.site_url is not a valid URL: {e}"))
            })?;
            if parsed.host_str().is_none() {
                return Err(AgentError::Config(
                    "channels.zulip.site_url must include a host".into(),
                ));
            }

            if zulip.ingress == ZulipIngress::Webhook {
                zulip_webhook_enabled = true;
                let token_env = zulip.web_hook_token_env.trim();
                if token_env.is_empty() {
                    return Err(AgentError::Config(
                        "channels.zulip.web_hook_token_env is required when channels.zulip.ingress is \"webhook\"".into(),
                    ));
                }
                let Some(web_hook_url) = zulip
                    .web_hook_url
                    .as_deref()
                    .map(str::trim)
                    .filter(|url| !url.is_empty())
                else {
                    return Err(AgentError::Config(
                        "channels.zulip.web_hook_url is required when channels.zulip.ingress is \"webhook\"".into(),
                    ));
                };

                let parsed = Url::parse(web_hook_url).map_err(|e| {
                    AgentError::Config(format!(
                        "channels.zulip.web_hook_url is not a valid URL: {e}"
                    ))
                })?;
                if parsed.scheme() != "https" {
                    return Err(AgentError::Config(format!(
                        "channels.zulip.web_hook_url must use https, got {:?}",
                        parsed.scheme()
                    )));
                }
                if parsed.host_str().is_none() {
                    return Err(AgentError::Config(
                        "channels.zulip.web_hook_url must include a host".into(),
                    ));
                }
            }

            if zulip.presence_enabled && zulip.presence_ping_interval_secs == 0 {
                return Err(AgentError::Config(
                    "channels.zulip.presence_ping_interval_secs must be greater than 0 when channels.zulip.presence_enabled is true".into(),
                ));
            }
        }

        if telegram_webhook_enabled || zulip_webhook_enabled {
            if self.webhook.host.trim().is_empty() {
                return Err(AgentError::Config(
                    "webhook.host is required when any channel webhook ingress is enabled".into(),
                ));
            }
            if self.webhook.port == 0 {
                return Err(AgentError::Config(
                    "webhook.port must be greater than 0 when any channel webhook ingress is enabled"
                        .into(),
                ));
            }
        }

        let mut mcp_names = std::collections::HashSet::new();
        for server in &self.mcp.servers {
            if server.name.trim().is_empty() {
                return Err(AgentError::Config(
                    "mcp server names must not be blank".into(),
                ));
            }
            if !mcp_names.insert(server.name.clone()) {
                return Err(AgentError::Config(format!(
                    "duplicate mcp server name {:?}",
                    server.name
                )));
            }
            if server.connect_timeout_secs == 0 || server.call_timeout_secs == 0 {
                return Err(AgentError::Config(format!(
                    "mcp server {:?} timeouts must be greater than 0",
                    server.name
                )));
            }
            if let Some(prefix) = &server.tool_prefix
                && (prefix.len() >= MCP_TOOL_NAME_MAX_LEN
                    || !prefix.bytes().all(is_valid_tool_name_byte))
            {
                return Err(AgentError::Config(format!(
                    "mcp server {:?} has an invalid tool_prefix",
                    server.name
                )));
            }
            for (kind, names) in [
                ("include_tools", &server.include_tools),
                ("exclude_tools", &server.exclude_tools),
            ] {
                if names.iter().any(|name| name.trim().is_empty()) {
                    return Err(AgentError::Config(format!(
                        "mcp server {:?} {kind} must not contain blank names",
                        server.name
                    )));
                }
            }
            match &server.transport {
                McpTransportConfig::StreamableHttp {
                    url,
                    bearer_token_env,
                } => {
                    let parsed = Url::parse(url.trim()).map_err(|e| {
                        AgentError::Config(format!(
                            "mcp server {:?} URL is invalid: {e}",
                            server.name
                        ))
                    })?;
                    if !matches!(parsed.scheme(), "http" | "https") {
                        return Err(AgentError::Config(format!(
                            "mcp server {:?} URL must use http or https",
                            server.name
                        )));
                    }
                    if parsed.host_str().is_none() {
                        return Err(AgentError::Config(format!(
                            "mcp server {:?} URL must include a host",
                            server.name
                        )));
                    }
                    if bearer_token_env
                        .as_deref()
                        .is_some_and(|env| env.trim().is_empty())
                    {
                        return Err(AgentError::Config(format!(
                            "mcp server {:?} bearer_token_env must not be blank",
                            server.name
                        )));
                    }
                }
            }
        }

        Ok(())
    }
}
