//! NerdBot — a minimal, self-hosted AI agent runtime.
//!
//! Run as a single binary, Docker-friendly.
//! Supports Telegram as the user-facing channel with iterative tool use
//! driven by multiple LLM providers via the `genai` crate.

use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use nerdbot::config::AppConfig;
use nerdbot::context::budget::ContextBudget;
use nerdbot::context::compaction_service::CompactionService;
use nerdbot::context::compaction_worker::CompactionWorker;
use nerdbot::error::AgentError;
use nerdbot::llm::LlmClient;
use nerdbot::scheduler::service::SchedulerService;
use nerdbot::storage::Database;
use nerdbot::telegram::bot::TelegramBot;
use nerdbot::telegram::handler::MessageHandler;
use nerdbot::telegram::service::TelegramService;
use nerdbot::tools::calculator::CalculatorTool;
use nerdbot::tools::echo::EchoTool;
use nerdbot::tools::files::{AppendFile, FileConfig, ListDirectory, ReadFile, WriteFile};
use nerdbot::tools::registry::ToolRegistry;
use nerdbot::tools::schedule::{DeleteJob, ListJobs, RunJobNow, ScheduleJob};
use nerdbot::tools::shell::{ShellConfig, ShellExecute};
use nerdbot::tools::telegram::SendTelegramMessage;
use nerdbot::tools::web::{WebFetch, WebSearch};
use tracing::{error, info, warn};

/// NerdBot — a minimal, self-hosted AI agent runtime.
#[derive(Parser, Debug)]
#[command(version, about)]
struct Cli {
    /// Path to TOML configuration file.
    #[arg(short, long, default_value = "config.toml")]
    config: PathBuf,
}

#[tokio::main]
async fn main() {
    // Initialize structured logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cli = Cli::parse();

    info!(config_path = %cli.config.display(), "starting nerdbot");

    // Load and validate configuration
    let config = match AppConfig::from_file(&cli.config) {
        Ok(c) => {
            info!("configuration loaded");
            c
        }
        Err(e) => {
            tracing::warn!(error = %e, "no config file found or invalid, using defaults");
            AppConfig::default()
        }
    };

    info!(
        agent_name = config.agent.name,
        model = config.llm.model,
        "configuration loaded"
    );

    // Initialize storage
    let db = match Database::new(config.storage.sqlite_path.clone()).await {
        Ok(db) => {
            if let Err(e) = db.init().await {
                error!(error = %e, "database migration failed");
                return;
            }
            Arc::new(db)
        }
        Err(e) => {
            error!(error = %e, "database connection failed");
            return;
        }
    };

    // Initialize Telegram bot
    let bot_token = match std::env::var(&config.telegram.bot_token_env) {
        Ok(token) if !token.is_empty() => token,
        Ok(_) => {
            error!(
                env_var = config.telegram.bot_token_env,
                "Telegram bot token is empty"
            );
            return;
        }
        Err(_) => {
            error!(
                env_var = config.telegram.bot_token_env,
                "Telegram bot token environment variable not set"
            );
            return;
        }
    };

    let bot = TelegramBot::new(bot_token);

    // Verify the bot token
    match bot.get_me().await {
        Ok(user) => {
            info!(
                bot_username = ?user.username,
                bot_name = user.first_name,
                "Telegram bot authenticated"
            );
        }
        Err(e) => {
            error!(error = %e, "Telegram bot authentication failed");
            return;
        }
    }

    // Clear any existing webhook so long polling works
    if let Err(e) = bot.delete_webhook().await {
        error!(error = %e, "failed to delete webhook, long polling may not work");
    }

    // Set up tool registry
    let mut registry = ToolRegistry::new();
    registry.register(EchoTool);
    registry.register(CalculatorTool);
    registry.register(ScheduleJob);
    registry.register(ListJobs);
    registry.register(DeleteJob);
    registry.register(RunJobNow);
    registry.register(SendTelegramMessage);
    let file_config = FileConfig::from(config.files.clone());
    registry.register(ReadFile::new(file_config.clone()));
    registry.register(WriteFile::new(file_config.clone()));
    registry.register(AppendFile::new(file_config.clone()));
    registry.register(ListDirectory::new(file_config));

    // Web tools — Exa-powered
    let exa_api_key = std::env::var(&config.exa.api_key_env).unwrap_or_default();
    if exa_api_key.is_empty() {
        warn!(
            env_var = config.exa.api_key_env,
            "Exa API key not set — web_search and web_fetch will return errors"
        );
    }
    registry.register(WebSearch::new(exa_api_key.clone(), config.exa.max_results));
    registry.register(WebFetch::new(
        exa_api_key.clone(),
        config.exa.max_text_chars,
    ));

    registry.register(ShellExecute::new(ShellConfig {
        allowed_commands: config.shell.allowed_commands.clone(),
        denied_commands: config.shell.denied_commands.clone(),
        max_output_bytes: config.shell.max_output_bytes,
        timeout_secs: config.shell.timeout_secs,
        sandbox_mode: config.shell.sandbox_mode.clone(),
    }));
    let registry = Arc::new(registry);

    // Create the LLM client via genai
    let llm = match LlmClient::from_config(&config) {
        Ok(client) => {
            info!(model = config.llm.model, "LLM client initialized via genai");
            Arc::new(client)
        }
        Err(e) => {
            error!(error = %e, "Failed to create LLM client");
            return;
        }
    };

    // Validate LLM model is configured (required for both agent and compaction).
    if config.llm.model.is_empty() {
        error!(
            "LLM model not configured. Set llm.model in config.toml (e.g. gpt-4o, claude-sonnet-4-5)."
        );
        return;
    }

    // Initialize compaction service using the same LLM.
    let compaction_budget = ContextBudget::from_llm_and_context(&config.llm, &config.context);
    let compaction_worker = CompactionWorker::new(
        llm.clone(),
        config.llm.model.clone(),
        config.llm.temperature,
    );
    let compaction_service = Arc::new(CompactionService::new(
        db.pool().clone(),
        Arc::new(compaction_worker),
        compaction_budget,
    ));

    let bot = Arc::new(bot);
    let service = TelegramService::new(bot.clone());

    // Initialize the scheduler service
    let scheduler = Arc::new(SchedulerService::new(
        db.pool().clone(),
        llm.clone(),
        registry.clone(),
        config.clone(),
        service.clone(),
    ));

    // Create a drop guard to guarantee scheduler shutdown
    let _scheduler_guard = SchedulerGuard {
        scheduler: scheduler.clone(),
    };

    // Create the message handler
    let handler = Arc::new(MessageHandler::new(
        db.pool().clone(),
        llm.clone(),
        registry,
        config.clone(),
        Some(scheduler.notifier()),
        compaction_service,
    ));

    // Start scheduler
    if let Err(e) = scheduler.start().await {
        error!(error = %e, "Failed to start scheduler");
        return;
    }

    info!("starting Telegram long polling...");

    // ── Long polling loop ──────────────────────────────────────────
    let mut offset: Option<i64> = None;
    let timeout_secs: u32 = 30;

    loop {
        tokio::select! {
            res = bot.get_updates(offset, timeout_secs) => {
                match res {
                    Ok(updates) => {
                        for update in updates {
                            let new_offset = update.update_id + 1;
                            if offset.is_none_or(|o| new_offset > o) {
                                offset = Some(new_offset);
                            }

                            let msg = match update.message {
                                Some(ref m) => m,
                                None => continue,
                            };

                            let chat_id = msg.chat.id;
                            let text = match &msg.text {
                                Some(t) => t.clone(),
                                None => continue,
                            };

                            let user_id = msg.from.as_ref().map(|u| u.id).unwrap_or(0);

                            let handler = handler.clone();
                            let service = service.clone();

                            tokio::spawn(async move {
                                match handler.handle_message(chat_id, user_id, &text).await {
                                    Ok(Some(response)) => {
                                        if let Err(e) = service.send_message(chat_id, &response).await {
                                            error!(chat_id, error = %e, "failed to send reply");
                                        }
                                    }
                                    Ok(None) => {}
                                    Err(AgentError::PermissionDenied) => {}
                                    Err(e) => {
                                        error!(chat_id, error = %e, "message handler error");
                                        let err_msg = format!("❌ Internal error: {e}");
                                        let _ = service.send_message(chat_id, &err_msg).await;
                                    }
                                }
                            });
                        }
                    }
                    Err(e) => {
                        error!(error = %e, "getUpdates failed, retrying in 5s");
                        tokio::select! {
                            _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {}
                            _ = tokio::signal::ctrl_c() => {
                                info!("received Ctrl-C during retry sleep, shutting down...");
                                break;
                            }
                        }
                    }
                }
            }
            _ = tokio::signal::ctrl_c() => {
                info!("received Ctrl-C, shutting down...");
                break;
            }
        }
    }

    info!("Gracefully shutting down services...");
    if let Err(e) = scheduler.stop().await {
        error!(error = %e, "Failed to stop scheduler gracefully");
    }
    info!("Shutdown complete.");
}

/// Drop guard to guarantee scheduler shutdown when the main execution exits or panics.
struct SchedulerGuard {
    scheduler: Arc<SchedulerService>,
}

impl Drop for SchedulerGuard {
    fn drop(&mut self) {
        let scheduler = self.scheduler.clone();
        tokio::spawn(async move {
            info!("SchedulerGuard: stopping scheduler background loop...");
            if let Err(e) = scheduler.stop().await {
                error!(error = %e, "SchedulerGuard: failed to stop scheduler gracefully");
            } else {
                info!("SchedulerGuard: scheduler background loop stopped successfully");
            }
        });
    }
}
