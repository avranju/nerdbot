//! NerdBot — a minimal, self-hosted AI agent runtime.
//!
//! Run as a single binary, Docker-friendly.
//! Supports Telegram as the user-facing channel with iterative tool use
//! driven by multiple LLM providers.

// TODO: Remove this allow once all modules are fully implemented (target Phase 5+).
#![allow(
    dead_code,
    unused,
    unused_imports,
    unused_variables,
    unused_assignments
)]

use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use error::AgentError;
use tracing::{error, info};

mod agent;
mod config;
mod context;
mod error;
mod llm;
mod scheduler;
mod storage;
mod telegram;
mod tools;
mod web;
mod workspace;

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

    // Phase 1: load and validate configuration
    let config = match config::AppConfig::from_file(&cli.config) {
        Ok(c) => {
            info!("configuration loaded");
            c
        }
        Err(e) => {
            tracing::warn!(error = %e, "no config file found or invalid, using defaults");
            config::AppConfig::default()
        }
    };

    info!(
        agent_name = config.agent.name,
        provider = config.llm.provider,
        model = config.llm.model,
        "configuration loaded"
    );

    // Phase 3: initialize storage
    let db = match storage::Database::new(config.storage.sqlite_path.clone()).await {
        Ok(db) => {
            // Run migrations
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

    // Phase 4: initialize Telegram bot
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

    let bot = telegram::TelegramBot::new(bot_token);

    // Verify the bot token by calling getMe
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

    // Phase 4: set up tool registry with toy tools
    let mut registry = tools::registry::ToolRegistry::new();
    registry.register(tools::echo::EchoTool);
    registry.register(tools::calculator::CalculatorTool);
    // Phase 5: register scheduling tools
    registry.register(tools::schedule::ScheduleJob);
    registry.register(tools::schedule::ListJobs);
    registry.register(tools::schedule::DeleteJob);
    registry.register(tools::schedule::RunJobNow);
    let registry = Arc::new(registry);

    // Phase 4: use fake provider (real providers come in Phase 7)
    let provider: Arc<dyn llm::provider::LlmProvider> = Arc::new(llm::fake::FakeProvider::new(
        vec![llm::fake::FakeResponse::final_text(
            "Hello! I'm NerdBot, running on a fake provider. Real LLM integration is coming in Phase 7. How can I help you today?",
        )],
    ));

    let bot = Arc::new(bot);
    let service = telegram::TelegramService::new(bot.clone());

    // Phase 5: initialize the scheduler service
    let scheduler = Arc::new(scheduler::service::SchedulerService::new(
        db.pool().clone(),
        provider.clone(),
        registry.clone(),
        config.clone(),
        service.clone(),
    ));

    // Create a drop guard to guarantee scheduler shutdown when the main execution exits or panics
    let _scheduler_guard = SchedulerGuard {
        scheduler: scheduler.clone(),
    };

    // Phase 4: create the message handler
    let handler = Arc::new(telegram::MessageHandler::new(
        db.pool().clone(),
        provider,
        registry,
        config.clone(),
        Some(scheduler.notifier()),
    ));

    // Phase 5: start scheduler
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
                            // Track the latest update_id to acknowledge it
                            let new_offset = update.update_id + 1;
                            if offset.is_none_or(|o| new_offset > o) {
                                offset = Some(new_offset);
                            }

                            // Extract message data
                            let msg = match update.message {
                                Some(ref m) => m,
                                None => continue, // Skip non-message updates for now
                            };

                            let chat_id = msg.chat.id;
                            let text = match &msg.text {
                                Some(t) => t.clone(),
                                None => continue, // Skip non-text messages
                            };

                            let user_id = msg.from.as_ref().map(|u| u.id).unwrap_or(0);

                            // Dispatch to handler
                            let handler = handler.clone();
                            let service = service.clone();

                            tokio::spawn(async move {
                                match handler.handle_message(chat_id, user_id, &text).await {
                                    Ok(Some(response)) => {
                                        if let Err(e) = service.send_message(chat_id, &response).await {
                                            error!(chat_id, error = %e, "failed to send reply");
                                        }
                                    }
                                    Ok(None) => {
                                        // No reply needed (e.g., silent command)
                                    }
                                    Err(AgentError::PermissionDenied) => {
                                        // Silently ignore unauthorized messages
                                    }
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
    scheduler: Arc<scheduler::service::SchedulerService>,
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
