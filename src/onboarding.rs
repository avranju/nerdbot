//! Interactive first-run configuration generation.

use std::error::Error;
use std::fs;
use std::path::Path;

use cliclack::{input, intro, note, outro, select};
use toml::Value;

use crate::config::{
    AppConfig, SANDBOX_MODE_BWRAP, SANDBOX_MODE_BWRAP_STRICT, SANDBOX_MODE_NONE,
    SHELL_NETWORK_ACCESS_DISABLED, SHELL_NETWORK_ACCESS_HOST,
};

const DEFAULT_PERSONALITY_FILE: &str = "/config/personality.md";
const DEFAULT_SQLITE_PATH: &str = "/data/agent.db";
const DEFAULT_WORKSPACE_ROOT: &str = "/workspace";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LlmProvider {
    OpenAi,
    Anthropic,
    Gemini,
    OpenRouter,
    Custom,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OnboardingAnswers {
    agent_name: String,
    default_timezone: String,
    telegram_token_env: String,
    allowed_chat_ids: Vec<i64>,
    allowed_user_ids: Vec<i64>,
    model: String,
    endpoint: Option<String>,
    api_key_env: Option<String>,
    sandbox_mode: String,
    shell_network_access: String,
    exa_api_key_env: String,
}

/// Run the interactive onboarding flow and write a valid TOML configuration.
pub fn run(config_path: &Path) -> Result<(), Box<dyn Error>> {
    intro("NerdBot onboarding")?;

    let defaults = if config_path.exists() {
        AppConfig::from_file(config_path)?
    } else {
        AppConfig::default()
    };
    let default_chat_ids = format_id_list(&defaults.telegram.allowed_chat_ids);
    let default_user_ids = format_id_list(&defaults.telegram.allowed_user_ids);

    let agent_name: String = input("Bot name")
        .default_input(&defaults.agent.name)
        .validate(|value: &String| required(value))
        .interact()?;
    note(
        "Timezone",
        "Enter an IANA timezone name such as Asia/Kolkata or America/New_York. See: https://en.wikipedia.org/wiki/List_of_tz_database_time_zones",
    )?;
    let default_timezone: String = input("Timezone")
        .default_input(&defaults.agent.default_timezone)
        .validate(|value: &String| required(value))
        .interact()?;
    let telegram_token_env: String =
        input("Environment variable that holds your Telegram bot token")
            .default_input(&defaults.telegram.bot_token_env)
            .validate(|value: &String| required(value))
            .interact()?;

    note(
        "Finding a Telegram chat ID",
        "Send your bot a message in the target chat, then call the Telegram Bot API getUpdates method and read message.chat.id from the response.",
    )?;
    let allowed_chat_ids: String =
        input("Allowed Telegram chat IDs (comma-separated, blank allows all chats)")
            .default_input(&default_chat_ids)
            .validate(|value: &String| validate_id_list(value))
            .interact()?;
    let allowed_user_ids: String =
        input("Allowed Telegram user IDs (comma-separated, blank allows all users)")
            .default_input(&default_user_ids)
            .validate(|value: &String| validate_id_list(value))
            .interact()?;

    let provider = select("Choose your LLM provider")
        .item(LlmProvider::OpenAi, "OpenAI", "models such as gpt-4o")
        .item(
            LlmProvider::Anthropic,
            "Anthropic",
            "models such as claude-sonnet-4-5",
        )
        .item(
            LlmProvider::Gemini,
            "Google Gemini",
            "models such as gemini-2.5-flash",
        )
        .item(
            LlmProvider::OpenRouter,
            "OpenRouter",
            "models prefixed with open_router::",
        )
        .item(
            LlmProvider::Custom,
            "Custom endpoint",
            "OpenAI-compatible endpoint",
        )
        .initial_value(infer_provider(&defaults))
        .interact()?;

    let default_model = if defaults.llm.model.is_empty() {
        provider_default_model(provider)
    } else {
        &defaults.llm.model
    };
    let model: String = input("LLM model name")
        .default_input(default_model)
        .validate(|value: &String| required(value))
        .interact()?;

    let endpoint = if provider == LlmProvider::Custom {
        let endpoint: String = input("OpenAI-compatible endpoint URL")
            .default_input(defaults.llm.endpoint.as_deref().unwrap_or(""))
            .validate(|value: &String| required(value))
            .interact()?;
        Some(endpoint)
    } else {
        None
    };
    let api_key_env: String =
        input("Optional LLM API key environment variable ('-' clears the value)")
            .default_input(defaults.llm.api_key_env.as_deref().unwrap_or(""))
            .required(false)
            .interact()?;

    let sandbox_mode = select("Shell sandbox mode")
        .item(
            SANDBOX_MODE_NONE,
            "none",
            "direct execution without isolation",
        )
        .item(
            SANDBOX_MODE_BWRAP,
            "bwrap",
            "Bubblewrap namespace isolation",
        )
        .item(
            SANDBOX_MODE_BWRAP_STRICT,
            "bwrap-strict",
            "reserved for future resource limits",
        )
        .initial_value(&defaults.shell.sandbox_mode)
        .interact()?;
    let shell_network_access = select("Shell sandbox network access")
        .item(
            SHELL_NETWORK_ACCESS_DISABLED,
            "disabled",
            "loopback-only when using bwrap",
        )
        .item(
            SHELL_NETWORK_ACCESS_HOST,
            "host",
            "allow outbound network from bwrap",
        )
        .initial_value(&defaults.shell.network_access)
        .interact()?;
    let exa_api_key_env: String =
        input("Optional Exa API key environment variable ('-' clears the value)")
            .default_input(&defaults.exa.api_key_env)
            .required(false)
            .interact()?;

    let answers = OnboardingAnswers {
        agent_name,
        default_timezone,
        telegram_token_env,
        allowed_chat_ids: parse_id_list(&allowed_chat_ids)?,
        allowed_user_ids: parse_id_list(&allowed_user_ids)?,
        model,
        endpoint,
        api_key_env: optional_value(&api_key_env),
        sandbox_mode: sandbox_mode.into(),
        shell_network_access: shell_network_access.into(),
        exa_api_key_env: optional_value(&exa_api_key_env).unwrap_or_default(),
    };

    write_config(config_path, &answers)?;
    outro(format!(
        "Wrote {}. Export {} and your LLM provider API key before starting NerdBot.",
        config_path.display(),
        answers.telegram_token_env
    ))?;
    Ok(())
}

fn write_config(path: &Path, answers: &OnboardingAnswers) -> Result<(), Box<dyn Error>> {
    let mut config = if path.exists() {
        fs::read_to_string(path)?.parse::<Value>()?
    } else {
        Value::Table(toml::map::Map::new())
    };

    let root = config
        .as_table_mut()
        .ok_or("Configuration root must be a TOML table.")?;
    let agent = table_mut(root, "agent")?;
    agent.insert("name".into(), Value::String(answers.agent_name.clone()));
    agent.insert(
        "personality_file".into(),
        Value::String(DEFAULT_PERSONALITY_FILE.into()),
    );
    agent.insert(
        "default_timezone".into(),
        Value::String(answers.default_timezone.clone()),
    );

    let telegram = table_mut(root, "telegram")?;
    telegram.insert(
        "bot_token_env".into(),
        Value::String(answers.telegram_token_env.clone()),
    );
    telegram.insert(
        "allowed_chat_ids".into(),
        id_array(&answers.allowed_chat_ids),
    );
    telegram.insert(
        "allowed_user_ids".into(),
        id_array(&answers.allowed_user_ids),
    );

    table_mut(root, "workspace")?
        .insert("root".into(), Value::String(DEFAULT_WORKSPACE_ROOT.into()));
    table_mut(root, "storage")?.insert(
        "sqlite_path".into(),
        Value::String(DEFAULT_SQLITE_PATH.into()),
    );

    let llm = table_mut(root, "llm")?;
    llm.insert("model".into(), Value::String(answers.model.clone()));
    set_optional_string(llm, "endpoint", answers.endpoint.as_deref());
    set_optional_string(llm, "api_key_env", answers.api_key_env.as_deref());

    let shell = table_mut(root, "shell")?;
    shell.insert(
        "sandbox_mode".into(),
        Value::String(answers.sandbox_mode.clone()),
    );
    shell.insert(
        "network_access".into(),
        Value::String(answers.shell_network_access.clone()),
    );
    table_mut(root, "exa")?.insert(
        "api_key_env".into(),
        Value::String(answers.exa_api_key_env.clone()),
    );

    let toml = toml::to_string_pretty(&config)?;
    fs::write(path, toml)?;
    Ok(())
}

fn table_mut<'a>(
    root: &'a mut toml::map::Map<String, Value>,
    key: &str,
) -> Result<&'a mut toml::map::Map<String, Value>, Box<dyn Error>> {
    root.entry(key)
        .or_insert_with(|| Value::Table(toml::map::Map::new()))
        .as_table_mut()
        .ok_or_else(|| format!("Configuration section [{key}] must be a TOML table.").into())
}

fn id_array(ids: &[i64]) -> Value {
    Value::Array(ids.iter().copied().map(Value::Integer).collect())
}

fn set_optional_string(table: &mut toml::map::Map<String, Value>, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        table.insert(key.into(), Value::String(value.into()));
    } else {
        table.remove(key);
    }
}

fn optional_value(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || value == "-" {
        None
    } else {
        Some(value.into())
    }
}

fn format_id_list(ids: &[i64]) -> String {
    ids.iter().map(i64::to_string).collect::<Vec<_>>().join(",")
}

fn infer_provider(config: &AppConfig) -> LlmProvider {
    if config.llm.endpoint.is_some() {
        LlmProvider::Custom
    } else if config.llm.model.starts_with("open_router::") {
        LlmProvider::OpenRouter
    } else if config.llm.model.starts_with("claude-") {
        LlmProvider::Anthropic
    } else if config.llm.model.starts_with("gemini-") {
        LlmProvider::Gemini
    } else {
        LlmProvider::OpenAi
    }
}

fn provider_default_model(provider: LlmProvider) -> &'static str {
    match provider {
        LlmProvider::OpenAi => "gpt-4o",
        LlmProvider::Anthropic => "claude-sonnet-4-5",
        LlmProvider::Gemini => "gemini-2.5-flash",
        LlmProvider::OpenRouter => "open_router::openai/gpt-4.1",
        LlmProvider::Custom => "",
    }
}

fn parse_id_list(value: &str) -> Result<Vec<i64>, std::num::ParseIntError> {
    value
        .split(',')
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::parse)
        .collect()
}

fn validate_id_list(value: &str) -> Result<(), String> {
    parse_id_list(value)
        .map(|_| ())
        .map_err(|_| "Use comma-separated numeric Telegram IDs.".to_string())
}

fn required(value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        Err("A value is required.".to_string())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn parse_id_list_accepts_empty_and_comma_separated_values() {
        assert_eq!(parse_id_list("").unwrap(), Vec::<i64>::new());
        assert_eq!(parse_id_list(" 12, -34,56 ").unwrap(), vec![12, -34, 56]);
    }

    #[test]
    fn write_config_produces_loadable_toml() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        let answers = OnboardingAnswers {
            agent_name: "test-bot".into(),
            default_timezone: "Asia/Kolkata".into(),
            telegram_token_env: "BOT_TOKEN".into(),
            allowed_chat_ids: vec![123],
            allowed_user_ids: vec![456],
            model: "custom-model".into(),
            endpoint: Some("http://localhost:8080/v1".into()),
            api_key_env: Some("CUSTOM_API_KEY".into()),
            sandbox_mode: SANDBOX_MODE_BWRAP.into(),
            shell_network_access: SHELL_NETWORK_ACCESS_HOST.into(),
            exa_api_key_env: "EXA_TOKEN".into(),
        };

        write_config(&path, &answers).unwrap();
        let generated = fs::read_to_string(&path).unwrap();
        let config = AppConfig::from_file(&path).unwrap();

        assert!(!generated.contains("temperature"));
        assert_eq!(config.agent.name, "test-bot");
        assert_eq!(
            config.agent.personality_file,
            PathBuf::from(DEFAULT_PERSONALITY_FILE)
        );
        assert_eq!(config.agent.default_timezone, "Asia/Kolkata");
        assert_eq!(config.telegram.bot_token_env, "BOT_TOKEN");
        assert_eq!(config.telegram.allowed_chat_ids, vec![123]);
        assert_eq!(config.telegram.allowed_user_ids, vec![456]);
        assert_eq!(config.llm.model, "custom-model");
        assert_eq!(
            config.llm.endpoint.as_deref(),
            Some("http://localhost:8080/v1")
        );
        assert_eq!(config.llm.api_key_env.as_deref(), Some("CUSTOM_API_KEY"));
        assert_eq!(config.workspace.root, PathBuf::from(DEFAULT_WORKSPACE_ROOT));
        assert_eq!(
            config.storage.sqlite_path,
            PathBuf::from(DEFAULT_SQLITE_PATH)
        );
        assert_eq!(config.shell.sandbox_mode, SANDBOX_MODE_BWRAP);
        assert_eq!(config.shell.network_access, SHELL_NETWORK_ACCESS_HOST);
        assert_eq!(config.exa.api_key_env, "EXA_TOKEN");
    }

    #[test]
    fn write_config_preserves_settings_not_managed_by_onboarding() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        fs::write(
            &path,
            r#"
[agent]
name = "custom-name"

[llm]
model = "old-model"
temperature = 0.7
"#,
        )
        .unwrap();
        let answers = OnboardingAnswers {
            agent_name: "updated-name".into(),
            default_timezone: "UTC".into(),
            telegram_token_env: "BOT_TOKEN".into(),
            allowed_chat_ids: vec![],
            allowed_user_ids: vec![],
            model: "new-model".into(),
            endpoint: None,
            api_key_env: None,
            sandbox_mode: SANDBOX_MODE_NONE.into(),
            shell_network_access: SHELL_NETWORK_ACCESS_DISABLED.into(),
            exa_api_key_env: String::new(),
        };

        write_config(&path, &answers).unwrap();
        let config = AppConfig::from_file(&path).unwrap();

        assert_eq!(config.agent.name, "updated-name");
        assert_eq!(config.llm.model, "new-model");
        assert_eq!(config.llm.temperature, 0.7);
    }

    #[test]
    fn infer_provider_uses_existing_config() {
        let mut config = AppConfig::default();
        config.llm.model = "gemini-2.5-flash".into();
        assert_eq!(infer_provider(&config), LlmProvider::Gemini);

        config.llm.endpoint = Some("http://localhost:8080/v1".into());
        assert_eq!(infer_provider(&config), LlmProvider::Custom);
    }

    #[test]
    fn optional_value_accepts_blank_and_clear_marker() {
        assert_eq!(optional_value(""), None);
        assert_eq!(optional_value(" - "), None);
        assert_eq!(optional_value(" CUSTOM_KEY "), Some("CUSTOM_KEY".into()));
    }
}
