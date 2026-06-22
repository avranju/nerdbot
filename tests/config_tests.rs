#![allow(
    dead_code,
    unused,
    unused_imports,
    unused_variables,
    unused_assignments
)]
//! Tests for configuration loading in `src/config.rs`.

use std::fs;

use nerdbot::config::{AppConfig, TelegramIngress};

// ── Default values ───────────────────────────────────────────────────────

#[test]
fn test_default_config_agent_name() {
    let config = AppConfig::default();
    assert_eq!(config.agent.name, "nerdbot");
}

#[test]
fn test_default_config_personality_file() {
    let config = AppConfig::default();
    assert_eq!(
        config.agent.personality_file,
        std::path::PathBuf::from("/config/personality.md")
    );
}

#[test]
fn test_default_config_max_tool_iterations() {
    let config = AppConfig::default();
    assert_eq!(config.agent.max_tool_iterations, 10);
}

#[test]
fn test_default_config_timezone() {
    let config = AppConfig::default();
    assert_eq!(config.agent.default_timezone, "UTC");
}

#[test]
fn test_default_config_telegram_token_env() {
    let config = AppConfig::default();
    assert_eq!(config.channels.telegram.bot_token_env, "TELEGRAM_BOT_TOKEN");
}

#[test]
fn test_default_config_telegram_ingress() {
    let config = AppConfig::default();
    assert_eq!(config.channels.telegram.ingress, TelegramIngress::Poll);
    assert_eq!(config.channels.telegram.web_hook_url, None);
    assert_eq!(config.channels.telegram.poll_interval_secs, 5);
    assert_eq!(config.webhook.host, "127.0.0.1");
    assert_eq!(config.webhook.port, 24_682);
}

#[test]
fn test_default_config_zulip_presence() {
    let config = AppConfig::default();
    assert!(!config.channels.zulip.presence_enabled);
    assert_eq!(config.channels.zulip.presence_ping_interval_secs, 60);
}

#[test]
fn test_default_config_telegram_no_allowed_ids() {
    let config = AppConfig::default();
    assert!(config.channels.telegram.allowed_conversations.is_empty());
    assert!(config.channels.telegram.allowed_senders.is_empty());
}

#[test]
fn test_default_config_sqlite_path() {
    let config = AppConfig::default();
    assert_eq!(
        config.storage.sqlite_path,
        std::path::PathBuf::from("/data/agent.db")
    );
}

#[test]
fn test_default_config_workspace_root() {
    let config = AppConfig::default();
    assert_eq!(
        config.workspace.root,
        std::path::PathBuf::from("/workspace")
    );
}

#[test]
fn test_default_config_max_read_bytes() {
    let config = AppConfig::default();
    assert_eq!(config.workspace.max_read_bytes, 262_144);
}

#[test]
fn test_default_config_max_write_bytes() {
    let config = AppConfig::default();
    assert_eq!(config.workspace.max_write_bytes, 262_144);
}

#[test]
fn test_default_config_llm_model() {
    let config = AppConfig::default();
    assert_eq!(config.llm.model, "");
    assert!(config.llm.endpoint.is_none());
    assert!(config.llm.api_key_env.is_none());
}

#[test]
fn test_default_config_llm_temperature() {
    let config = AppConfig::default();
    assert_eq!(config.llm.temperature, 0.2);
}

#[test]
fn test_default_config_llm_max_output_tokens() {
    let config = AppConfig::default();
    assert_eq!(config.llm.max_output_tokens, 4096);
}

#[test]
fn test_default_config_llm_context_window_tokens() {
    let config = AppConfig::default();
    assert_eq!(config.llm.context_window_tokens, 128_000);
}

#[test]
fn test_default_config_llm_retries() {
    let config = AppConfig::default();
    assert_eq!(config.llm.max_retries, 4);
    assert_eq!(config.llm.retry_interval_secs, 10);
}

#[test]
fn test_default_config_exa() {
    let config = AppConfig::default();
    assert_eq!(config.exa.api_key_env, "EXA_API_KEY");
    assert_eq!(config.exa.max_results, 5);
    assert_eq!(config.exa.max_text_chars, 8000);
}

#[test]
fn test_default_config_compaction_soft_threshold() {
    let config = AppConfig::default();
    assert_eq!(config.context.soft_compaction_threshold, 0.60);
}

#[test]
fn test_default_config_compaction_hard_threshold() {
    let config = AppConfig::default();
    assert_eq!(config.context.hard_context_threshold, 0.85);
}

#[test]
fn test_default_config_recent_turns_to_preserve() {
    let config = AppConfig::default();
    assert_eq!(config.context.recent_turns_to_preserve, 30);
}

#[test]
fn test_default_config_reserved_tool_loop_tokens() {
    let config = AppConfig::default();
    assert_eq!(config.context.reserved_tool_loop_tokens, 8_192);
}

#[test]
fn test_default_config_scheduler_overdue_false() {
    let config = AppConfig::default();
    assert!(!config.scheduler.run_overdue_one_shots_on_startup);
}

#[test]
fn test_default_config_maintenance_disabled() {
    let config = AppConfig::default();
    assert!(!config.maintenance.enabled);
    assert_eq!(config.maintenance.reason, "");
}

#[test]
fn test_maintenance_message_without_reason() {
    let config = AppConfig::default();
    let msg = config.maintenance.message();
    assert!(msg.contains("maintenance mode"));
    assert!(!msg.contains("Reason:"));
}

#[test]
fn test_maintenance_message_with_reason() {
    let mut config = AppConfig::default();
    config.maintenance.enabled = true;
    config.maintenance.reason = "Database migration in progress".to_string();
    let msg = config.maintenance.message();
    assert!(msg.contains("maintenance mode"));
    assert!(msg.contains("Database migration in progress"));
    assert!(msg.contains("Reason:"));
}

#[test]
fn test_maintenance_message_with_whitespace_only_reason() {
    let mut config = AppConfig::default();
    config.maintenance.enabled = true;
    config.maintenance.reason = "   ".to_string();
    let msg = config.maintenance.message();
    assert!(msg.contains("maintenance mode"));
    assert!(!msg.contains("Reason:"));
}

// ── TOML parsing ─────────────────────────────────────────────────────────

const SAMPLE_CONFIG: &str = r#"
[agent]
name = "test-agent"
personality_file = "/etc/personality.md"
max_tool_iterations = 5
default_timezone = "America/New_York"

[channels.telegram]
ingress = "webhook"
bot_token_env = "MY_TELEGRAM_TOKEN"
web_hook_url = "https://example.test/telegram/webhook"
poll_interval_secs = 7
allowed_conversations = ["111111111", "222222222"]
allowed_senders = ["333333333"]

[webhook]
host = "0.0.0.0"
port = 24683

[storage]
sqlite_path = "/tmp/test.db"

[workspace]
root = "/tmp/workspace"
max_read_bytes = 131072
max_write_bytes = 65536

[llm]
model = "gpt-4o"
endpoint = "https://api.example.test/v1"
api_key_env = "OPENAI_KEY"
temperature = 0.7
max_output_tokens = 2048
context_window_tokens = 65536
max_retries = 6
retry_interval_secs = 5


[context]
soft_compaction_threshold = 0.50
hard_context_threshold = 0.80
recent_turns_to_preserve = 20
reserved_tool_loop_tokens = 4096

[scheduler]
run_overdue_one_shots_on_startup = true
"#;

#[test]
fn test_parse_full_config() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("config.toml");
    fs::write(&path, SAMPLE_CONFIG).unwrap();

    let config = AppConfig::from_file(&path).unwrap();

    // Agent
    assert_eq!(config.agent.name, "test-agent");
    assert_eq!(
        config.agent.personality_file,
        std::path::PathBuf::from("/etc/personality.md")
    );
    assert_eq!(config.agent.max_tool_iterations, 5);
    assert_eq!(config.agent.default_timezone, "America/New_York");

    // Telegram
    assert_eq!(config.channels.telegram.ingress, TelegramIngress::Webhook);
    assert_eq!(config.channels.telegram.bot_token_env, "MY_TELEGRAM_TOKEN");
    assert_eq!(
        config.channels.telegram.web_hook_url.as_deref(),
        Some("https://example.test/telegram/webhook")
    );
    assert_eq!(config.webhook.host, "0.0.0.0");
    assert_eq!(config.webhook.port, 24_683);
    assert_eq!(config.channels.telegram.poll_interval_secs, 7);
    assert_eq!(
        config.channels.telegram.allowed_conversations,
        vec!["111111111".to_string(), "222222222".to_string()]
    );
    assert_eq!(
        config.channels.telegram.allowed_senders,
        vec!["333333333".to_string()]
    );

    // Storage
    assert_eq!(
        config.storage.sqlite_path,
        std::path::PathBuf::from("/tmp/test.db")
    );

    // Workspace
    assert_eq!(
        config.workspace.root,
        std::path::PathBuf::from("/tmp/workspace")
    );
    assert_eq!(config.workspace.max_read_bytes, 131_072);
    assert_eq!(config.workspace.max_write_bytes, 65_536);

    // LLM
    assert_eq!(config.llm.model, "gpt-4o");
    assert_eq!(
        config.llm.endpoint.as_deref(),
        Some("https://api.example.test/v1")
    );
    assert_eq!(config.llm.api_key_env.as_deref(), Some("OPENAI_KEY"));
    assert_eq!(config.llm.temperature, 0.7);
    assert_eq!(config.llm.max_output_tokens, 2048);
    assert_eq!(config.llm.context_window_tokens, 65_536);
    assert_eq!(config.llm.max_retries, 6);
    assert_eq!(config.llm.retry_interval_secs, 5);

    // Context
    assert_eq!(config.context.soft_compaction_threshold, 0.50);
    assert_eq!(config.context.hard_context_threshold, 0.80);
    assert_eq!(config.context.recent_turns_to_preserve, 20);
    assert_eq!(config.context.reserved_tool_loop_tokens, 4_096);

    // Scheduler
    assert!(config.scheduler.run_overdue_one_shots_on_startup);
}

#[test]
fn test_parse_minimal_config() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("config.toml");
    fs::write(&path, "").unwrap();

    let config = AppConfig::from_file(&path).unwrap();

    // All values should be defaults
    assert_eq!(config.agent.name, "nerdbot");
    assert_eq!(config.channels.telegram.bot_token_env, "TELEGRAM_BOT_TOKEN");
    assert_eq!(config.context.soft_compaction_threshold, 0.60);
}

#[test]
fn test_parse_missing_file() {
    let result = AppConfig::from_file(std::path::Path::new("/nonexistent/path.toml"));
    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err(),
        nerdbot::error::AgentError::Config(_)
    ));
}

#[test]
fn test_parse_invalid_toml() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("config.toml");
    fs::write(&path, "[agent\nname = broken").unwrap();

    let result = AppConfig::from_file(&path);
    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err(),
        nerdbot::error::AgentError::Config(_)
    ));
}

#[test]
fn test_webhook_ingress_requires_webhook_url() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("config.toml");
    fs::write(
        &path,
        r#"
[channels.telegram]
ingress = "webhook"
"#,
    )
    .unwrap();

    let result = AppConfig::from_file(&path);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("channels.telegram.web_hook_url is required"),
        "unexpected error: {err}"
    );
}

#[test]
fn test_invalid_telegram_ingress_is_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("config.toml");
    fs::write(
        &path,
        r#"
[channels.telegram]
ingress = "push"
"#,
    )
    .unwrap();

    let result = AppConfig::from_file(&path);
    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err(),
        nerdbot::error::AgentError::Config(_)
    ));
}

#[test]
fn test_push_mode_requires_valid_webhook_url() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("config.toml");
    fs::write(
        &path,
        r#"
[channels.telegram]
ingress = "webhook"
web_hook_url = "not a url"
"#,
    )
    .unwrap();

    let result = AppConfig::from_file(&path);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("channels.telegram.web_hook_url is not a valid URL"),
        "unexpected error: {err}"
    );
}

#[test]
fn test_push_mode_requires_https_webhook_url() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("config.toml");
    fs::write(
        &path,
        r#"
[channels.telegram]
ingress = "webhook"
web_hook_url = "http://example.test/telegram/webhook"
"#,
    )
    .unwrap();

    let result = AppConfig::from_file(&path);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("channels.telegram.web_hook_url must use https"),
        "unexpected error: {err}"
    );
}

#[test]
fn test_webhook_ingress_requires_global_webhook_host() {
    let toml = r#"
[channels.telegram]
ingress = "webhook"
web_hook_url = "https://example.test/telegram/hook"

[webhook]
host = " "
port = 24682
"#;

    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("config.toml");
    fs::write(&path, toml).unwrap();

    let err = AppConfig::from_file(&path).unwrap_err().to_string();
    assert!(err.contains("webhook.host is required"));
}

#[test]
fn test_webhook_ingress_rejects_zero_global_webhook_port() {
    let toml = r#"
[channels.telegram]
ingress = "webhook"
web_hook_url = "https://example.test/telegram/hook"

[webhook]
host = "127.0.0.1"
port = 0
"#;

    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("config.toml");
    fs::write(&path, toml).unwrap();

    let err = AppConfig::from_file(&path).unwrap_err().to_string();
    assert!(err.contains("webhook.port must be greater than 0"));
}

#[test]
fn test_allowed_conversations_must_not_be_blank() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("config.toml");
    fs::write(
        &path,
        r#"
[channels.telegram]
allowed_conversations = ["room-1", "  "]
"#,
    )
    .unwrap();

    let result = AppConfig::from_file(&path);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("channels.telegram.allowed_conversations must not contain blank IDs"),
        "unexpected error: {err}"
    );
}

#[test]
fn test_allowed_senders_must_not_be_blank() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("config.toml");
    fs::write(
        &path,
        r#"
[channels.telegram]
allowed_senders = ["alice", ""]
"#,
    )
    .unwrap();

    let result = AppConfig::from_file(&path);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("channels.telegram.allowed_senders must not contain blank IDs"),
        "unexpected error: {err}"
    );
}

#[test]
fn test_parse_partial_config() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("config.toml");
    fs::write(
        &path,
        r#"
[agent]
name = "partial"

[llm]
model = "gpt-4o"
"#,
    )
    .unwrap();

    let config = AppConfig::from_file(&path).unwrap();

    assert_eq!(config.agent.name, "partial");
    assert_eq!(config.llm.model, "gpt-4o");

    // Unspecified fields should be defaults
    assert_eq!(config.channels.telegram.bot_token_env, "TELEGRAM_BOT_TOKEN");
    assert_eq!(config.context.hard_context_threshold, 0.85);
}

// ── Zulip webhook URL validation ─────────────────────────────────────────

#[test]
fn test_zulip_webhook_ingress_requires_webhook_url() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("config.toml");
    fs::write(
        &path,
        r#"
[channels.zulip]
enabled = true
site_url = "https://org.zulipchat.com"
ingress = "webhook"
web_hook_token_env = "ZULIP_WEBHOOK_TOKEN"

[llm]
model = "gpt-4o"
"#,
    )
    .unwrap();

    let result = AppConfig::from_file(&path);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("channels.zulip.web_hook_url is required"),
        "unexpected error: {err}"
    );
}

#[test]
fn test_zulip_webhook_url_requires_https() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("config.toml");
    fs::write(
        &path,
        r#"
[channels.zulip]
enabled = true
site_url = "https://org.zulipchat.com"
ingress = "webhook"
web_hook_token_env = "ZULIP_WEBHOOK_TOKEN"
web_hook_url = "http://example.test/zulip/hook"

[llm]
model = "gpt-4o"
"#,
    )
    .unwrap();

    let result = AppConfig::from_file(&path);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("channels.zulip.web_hook_url must use https"),
        "unexpected error: {err}"
    );
}

#[test]
fn test_zulip_webhook_url_requires_valid_url() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("config.toml");
    fs::write(
        &path,
        r#"
[channels.zulip]
enabled = true
site_url = "https://org.zulipchat.com"
ingress = "webhook"
web_hook_token_env = "ZULIP_WEBHOOK_TOKEN"
web_hook_url = "not a url"

[llm]
model = "gpt-4o"
"#,
    )
    .unwrap();

    let result = AppConfig::from_file(&path);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("channels.zulip.web_hook_url is not a valid URL"),
        "unexpected error: {err}"
    );
}

#[test]
fn test_zulip_default_web_hook_url_is_none() {
    let config = AppConfig::default();
    assert_eq!(config.channels.zulip.web_hook_url, None);
}

#[test]
fn test_zulip_presence_interval_must_be_positive_when_enabled() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("config.toml");
    fs::write(
        &path,
        r#"
[channels.zulip]
enabled = true
site_url = "https://org.zulipchat.com"
presence_enabled = true
presence_ping_interval_secs = 0

[llm]
model = "gpt-4o"
"#,
    )
    .unwrap();

    let result = AppConfig::from_file(&path);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("channels.zulip.presence_ping_interval_secs must be greater than 0"),
        "unexpected error: {err}"
    );
}

#[test]
fn test_zulip_presence_interval_can_be_zero_when_disabled() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("config.toml");
    fs::write(
        &path,
        r#"
[channels.zulip]
enabled = true
site_url = "https://org.zulipchat.com"
presence_enabled = false
presence_ping_interval_secs = 0

[llm]
model = "gpt-4o"
"#,
    )
    .unwrap();

    let config = AppConfig::from_file(&path).unwrap();
    assert!(!config.channels.zulip.presence_enabled);
    assert_eq!(config.channels.zulip.presence_ping_interval_secs, 0);
}

#[test]
fn test_parse_maintenance_config() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("config.toml");
    fs::write(
        &path,
        r#"
[maintenance]
enabled = true
reason = "Database migration in progress"

[llm]
model = "gpt-4o"
"#,
    )
    .unwrap();

    let config = AppConfig::from_file(&path).unwrap();
    assert!(config.maintenance.enabled);
    assert_eq!(config.maintenance.reason, "Database migration in progress");
    assert!(config.maintenance.message().contains("Database migration"));
}

#[test]
fn test_parse_maintenance_config_default_disabled() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("config.toml");
    fs::write(
        &path,
        r#"
[llm]
model = "gpt-4o"
"#,
    )
    .unwrap();

    let config = AppConfig::from_file(&path).unwrap();
    assert!(!config.maintenance.enabled);
    assert_eq!(config.maintenance.reason, "");
}

#[test]
fn test_maintenance_reason_too_long_is_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("config.toml");
    fs::write(
        &path,
        r#"
[maintenance]
enabled = true
reason = "A"

[llm]
model = "gpt-4o"
"#,
    )
    .unwrap();

    let mut config = AppConfig::from_file(&path).unwrap();
    // Set reason to > 1000 chars
    config.maintenance.reason = "x".repeat(1001);
    let err = config.validate().unwrap_err().to_string();
    assert!(err.contains("maintenance.reason must not exceed 1000 characters"));
}
