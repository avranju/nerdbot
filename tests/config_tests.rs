//! Tests for configuration loading in `src/config.rs`.

use std::fs;

use nerdbot::config::AppConfig;

// ── Default values ───────────────────────────────────────────────────────

#[test]
fn test_default_config_agent_name() {
    let config = AppConfig::default();
    assert_eq!(config.agent.name, "nerdbot");
}

#[test]
fn test_default_config_personality_file() {
    let config = AppConfig::default();
    assert_eq!(config.agent.personality_file, std::path::PathBuf::from("/config/personality.md"));
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
    assert_eq!(config.telegram.bot_token_env, "TELEGRAM_BOT_TOKEN");
}

#[test]
fn test_default_config_telegram_no_allowed_ids() {
    let config = AppConfig::default();
    assert!(config.telegram.allowed_chat_ids.is_empty());
    assert!(config.telegram.allowed_user_ids.is_empty());
}

#[test]
fn test_default_config_sqlite_path() {
    let config = AppConfig::default();
    assert_eq!(config.storage.sqlite_path, std::path::PathBuf::from("/data/agent.db"));
}

#[test]
fn test_default_config_workspace_root() {
    let config = AppConfig::default();
    assert_eq!(config.workspace.root, std::path::PathBuf::from("/workspace"));
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
fn test_default_config_llm_provider() {
    let config = AppConfig::default();
    assert_eq!(config.llm.provider, "anthropic");
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
fn test_default_config_scheduler_overdue_false() {
    let config = AppConfig::default();
    assert!(!config.scheduler.run_overdue_one_shots_on_startup);
}

// ── TOML parsing ─────────────────────────────────────────────────────────

const SAMPLE_CONFIG: &str = r#"
[agent]
name = "test-agent"
personality_file = "/etc/personality.md"
max_tool_iterations = 5
default_timezone = "America/New_York"

[telegram]
bot_token_env = "MY_TELEGRAM_TOKEN"
allowed_chat_ids = [111111111, 222222222]
allowed_user_ids = [333333333]

[storage]
sqlite_path = "/tmp/test.db"

[workspace]
root = "/tmp/workspace"
max_read_bytes = 131072
max_write_bytes = 65536

[llm]
provider = "openai"
model = "gpt-4o"
temperature = 0.7
max_output_tokens = 2048

[providers.anthropic]
api_key_env = "ANTHROPIC_KEY"

[providers.openai]
api_key_env = "OPENAI_KEY"

[providers.gemini]
api_key_env = "GEMINI_KEY"

[providers.openrouter]
api_key_env = "OPENROUTER_KEY"
base_url = "https://openrouter.ai/api/v1"

[context]
soft_compaction_threshold = 0.50
hard_context_threshold = 0.80
recent_turns_to_preserve = 20

[context.compactor]
provider = "anthropic"
model = "claude-haiku"

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
    assert_eq!(config.telegram.bot_token_env, "MY_TELEGRAM_TOKEN");
    assert_eq!(config.telegram.allowed_chat_ids, vec![111_111_111i64, 222_222_222]);
    assert_eq!(config.telegram.allowed_user_ids, vec![333_333_333i64]);

    // Storage
    assert_eq!(config.storage.sqlite_path, std::path::PathBuf::from("/tmp/test.db"));

    // Workspace
    assert_eq!(config.workspace.root, std::path::PathBuf::from("/tmp/workspace"));
    assert_eq!(config.workspace.max_read_bytes, 131_072);
    assert_eq!(config.workspace.max_write_bytes, 65_536);

    // LLM
    assert_eq!(config.llm.provider, "openai");
    assert_eq!(config.llm.model, "gpt-4o");
    assert_eq!(config.llm.temperature, 0.7);
    assert_eq!(config.llm.max_output_tokens, 2048);

    // Providers
    assert_eq!(config.providers.anthropic.api_key_env, "ANTHROPIC_KEY");
    assert_eq!(config.providers.openai.api_key_env, "OPENAI_KEY");
    assert_eq!(config.providers.gemini.api_key_env, "GEMINI_KEY");
    assert_eq!(config.providers.openrouter.api_key_env, "OPENROUTER_KEY");
    assert_eq!(
        config.providers.openrouter.base_url,
        "https://openrouter.ai/api/v1"
    );

    // Context
    assert_eq!(config.context.soft_compaction_threshold, 0.50);
    assert_eq!(config.context.hard_context_threshold, 0.80);
    assert_eq!(config.context.recent_turns_to_preserve, 20);
    assert_eq!(config.context.compactor.provider, "anthropic");
    assert_eq!(config.context.compactor.model, "claude-haiku");

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
    assert_eq!(config.telegram.bot_token_env, "TELEGRAM_BOT_TOKEN");
    assert_eq!(config.context.soft_compaction_threshold, 0.60);
}

#[test]
fn test_parse_missing_file() {
    let result = AppConfig::from_file(std::path::Path::new("/nonexistent/path.toml"));
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), nerdbot::error::AgentError::Config(_)));
}

#[test]
fn test_parse_invalid_toml() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("config.toml");
    fs::write(&path, "[agent\nname = broken").unwrap();

    let result = AppConfig::from_file(&path);
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), nerdbot::error::AgentError::Config(_)));
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
provider = "openai"
"#,
    )
    .unwrap();

    let config = AppConfig::from_file(&path).unwrap();

    assert_eq!(config.agent.name, "partial");
    assert_eq!(config.llm.provider, "openai");

    // Unspecified fields should be defaults
    assert_eq!(config.telegram.bot_token_env, "TELEGRAM_BOT_TOKEN");
    assert_eq!(config.context.hard_context_threshold, 0.85);
}
