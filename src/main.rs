//! NerdBot — a minimal, self-hosted AI agent runtime.
//!
//! Run as a single binary, Docker-friendly.
//! Supports Telegram as the user-facing channel with iterative tool use
//! driven by multiple LLM providers via the `genai` crate.

use std::path::PathBuf;
use std::sync::Arc;

use clap::{Args, Parser, Subcommand};
use nerdbot::config::AppConfig;
use nerdbot::context::budget::ContextBudget;
use nerdbot::context::compaction_service::CompactionService;
use nerdbot::context::compaction_worker::CompactionWorker;
use nerdbot::error::AgentError;
use nerdbot::llm::LlmClient;
use nerdbot::scheduler::service::SchedulerService;
use nerdbot::storage::Database;
use nerdbot::telegram::attachment::{self, AttachmentKind};
use nerdbot::telegram::bot::TelegramBot;
use nerdbot::telegram::handler::{AttachmentInfo, InboundMessage, MessageHandler};
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
    /// Enable the local diagnostics service on this Unix socket path.
    #[arg(short, long)]
    diagnostics_socket: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Create a config.toml file through an interactive first-run flow.
    Onboard,
    /// Query a running NerdBot instance through its diagnostics Unix socket.
    Diagnostics(DiagnosticsArgs),
}

#[derive(Debug, Args)]
struct DiagnosticsArgs {
    /// Emit machine-readable JSON instead of human-readable output.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: DiagnosticsCommand,
}

#[derive(Debug, Subcommand)]
enum DiagnosticsCommand {
    /// Verify that the diagnostics service is available.
    Ping,
    /// List persisted chat sessions.
    ListSessions,
    /// Show token usage and compaction state for one chat session.
    Show {
        /// Database session UUID.
        #[arg(long, conflicts_with = "chat_id", required_unless_present = "chat_id")]
        session_id: Option<String>,
        /// Telegram chat ID. Uses the latest session for that chat.
        #[arg(
            long,
            conflicts_with = "session_id",
            required_unless_present = "session_id"
        )]
        chat_id: Option<i64>,
        /// Include full personality and compaction prompt bodies.
        #[arg(long)]
        show_prompts: bool,
    },
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

    if let Some(command) = cli.command {
        match command {
            Command::Onboard => {
                if let Err(e) = nerdbot::onboarding::run(&cli.config) {
                    error!(error = %e, "onboarding failed");
                }
            }
            Command::Diagnostics(args) => {
                let Some(socket_path) = cli.diagnostics_socket.as_deref() else {
                    eprintln!("error: --diagnostics-socket <path> is required");
                    std::process::exit(2);
                };
                if let Err(e) = run_diagnostics_command(socket_path, args).await {
                    eprintln!("error: {e}");
                    std::process::exit(1);
                }
            }
        }
        return;
    }

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

    let bot = TelegramBot::new(bot_token.clone());

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
    let compaction_worker = CompactionWorker::new_with_preserve(
        llm.clone(),
        config.llm.model.clone(),
        config.llm.temperature,
        config.context.recent_turns_to_preserve,
    );
    let compaction_service = Arc::new(CompactionService::new(
        db.pool().clone(),
        Arc::new(compaction_worker),
        compaction_budget,
        config.context.recent_turns_to_preserve,
    ));
    let mut diagnostics_server = match cli.diagnostics_socket {
        Some(socket_path) => {
            match nerdbot::diagnostics::server::DiagnosticsServer::start(
                socket_path,
                db.pool().clone(),
                compaction_service.clone(),
                config.clone(),
                registry.clone(),
            )
            .await
            {
                Ok(server) => Some(server),
                Err(e) => {
                    error!(error = %e, "failed to start diagnostics service");
                    return;
                }
            }
        }
        None => None,
    };

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
        Some(service.clone()),
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
                            let user_id = msg.from.as_ref().map(|u| u.id).unwrap_or(0);
                            let msg = msg.clone();

                            // Allowlist check BEFORE building/downloading attachments.
                            // This prevents untrusted users from triggering expensive downloads.
                            let allowed_chat_ids = config.telegram.allowed_chat_ids.clone();
                            let allowed_user_ids = config.telegram.allowed_user_ids.clone();
                            let allowed = (allowed_chat_ids.is_empty() || allowed_chat_ids.contains(&chat_id))
                                && (allowed_user_ids.is_empty() || allowed_user_ids.contains(&user_id));
                            if !allowed {
                                warn!(chat_id, user_id, "skipping message: not in allowlist");
                                continue;
                            }

                            // Build a structured inbound message from the Telegram update.
                            // Extract the user prompt from text or caption (captions take priority
                            // for attachment-only messages). Use a default prompt when neither exists.
                            let handler = handler.clone();
                            let service = service.clone();
                            let bot = bot.clone();
                            let max_attachment_bytes = config.telegram.max_attachment_bytes;
                            let max_text_chars = config.telegram.max_text_document_chars;

                            tokio::spawn(async move {
                                // Build the inbound message
                                let (text, attachment_parts, attachment_infos) =
                                    build_inbound_message(&msg, bot.as_ref(), max_attachment_bytes, max_text_chars).await;

                                let inbound = InboundMessage {
                                    text,
                                    attachment_parts,
                                    attachments: attachment_infos,
                                };

                                match handler.handle_rich_message(chat_id, user_id, &inbound).await {
                                    Ok(Some(response)) => {
                                        if let Err(e) = service
                                            .send_message_with_options(chat_id, &response, Some("MarkdownV2"), None)
                                            .await
                                        {
                                            error!(chat_id, error = %e, "failed to send reply");
                                        }
                                    }
                                    Ok(None) => {}
                                    Err(AgentError::PermissionDenied) => {
                                        error!(chat_id, user_id, "permission denied");
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
    if let Some(server) = diagnostics_server.as_mut() {
        server.stop().await;
    }
    info!("Shutdown complete.");
}

async fn run_diagnostics_command(
    socket_path: &std::path::Path,
    args: DiagnosticsArgs,
) -> Result<(), AgentError> {
    use nerdbot::diagnostics::client::{render_human, render_json, send_request};
    use nerdbot::diagnostics::protocol::DiagnosticsRequest;

    let request = match args.command {
        DiagnosticsCommand::Ping => DiagnosticsRequest::Ping,
        DiagnosticsCommand::ListSessions => DiagnosticsRequest::ListSessions,
        DiagnosticsCommand::Show {
            session_id,
            chat_id,
            show_prompts,
        } => DiagnosticsRequest::ShowSession {
            session_id,
            chat_id,
            include_prompts: show_prompts,
        },
    };
    let response = send_request(socket_path, &request).await?;
    let output = if args.json {
        render_json(&response)?
    } else {
        render_human(&response)
    };
    println!("{output}");
    Ok(())
}

/// Build an `InboundMessage` from a Telegram update.
///
/// Processes attachments (photos, documents) by:
/// 1. Selecting the largest photo variant
/// 2. Downloading supported files (bounded by config limits) via `TelegramBot::process_attachment`
/// 3. Converting to LLM content parts (binary or text)
/// 4. Building a user prompt from caption or text
///
/// Returns (user_prompt, content_parts, attachment_infos).
async fn build_inbound_message(
    msg: &nerdbot::telegram::bot::Message,
    bot: &TelegramBot,
    max_attachment_bytes: usize,
    max_text_chars: usize,
) -> (String, Vec<genai::chat::ContentPart>, Vec<AttachmentInfo>) {
    let mut attachment_parts = Vec::new();
    let mut attachment_infos = Vec::new();
    let mut user_text = String::new();

    // Extract caption first (for photo/document messages)
    let caption = msg.caption.as_deref().unwrap_or("");

    // Process photos — select the largest variant
    if !msg.photo.is_empty()
        && let Some(largest) = attachment::largest_photo_size(&msg.photo)
    {
        let file_id = &largest.file_id;
        let size_bytes = largest.file_size.unwrap_or(0);

        // Download and process as binary via TelegramBot
        match bot
            .process_attachment(
                file_id,
                None,               // Photos don't have filenames
                Some("image/jpeg"), // Telegram photos are JPEG
                size_bytes,
                max_attachment_bytes,
                max_text_chars,
            )
            .await
        {
            Ok(processed) => {
                attachment_parts.push(processed.content_part);
                attachment_infos.push(AttachmentInfo {
                    display_name: format!(
                        "photo_{}x{}",
                        largest.width.unwrap_or(0),
                        largest.height.unwrap_or(0)
                    ),
                    mime_type: "image/jpeg".to_string(),
                    size_bytes,
                    downloaded: processed.downloaded,
                    persistence_marker: processed.persistence_marker,
                    extracted_text: processed.extracted_text,
                });
            }
            Err(e) => {
                warn!(chat_id = msg.chat.id, error = %e, "failed to process photo attachment");
                attachment_infos.push(AttachmentInfo {
                    display_name: "photo".to_string(),
                    mime_type: "image/jpeg".to_string(),
                    size_bytes,
                    downloaded: false,
                    persistence_marker: format!("[Attachment processing failed: photo, {e}]"),
                    extracted_text: None,
                });
                attachment_parts.push(genai::chat::ContentPart::Text(format!(
                    "⚠️ Failed to process photo: {e}"
                )));
            }
        }
    }

    // Track whether an unsupported warning was set (must not be overwritten).
    let mut unsupported_warn_set = false;

    // Process documents
    if let Some(doc) = &msg.document {
        let mime = doc.mime_type.as_deref();
        let filename = doc.file_name.as_deref();
        let size_bytes = doc.file_size.unwrap_or(0);

        // Validate before downloading
        let kind = attachment::classify_attachment(mime, filename);

        if kind == AttachmentKind::Unsupported {
            let display_name = attachment::sanitize_filename(filename.unwrap_or("document"));
            let mime_str = mime.unwrap_or("unknown");
            // For unsupported attachments, prepend the warning to the caption
            // so the LLM receives both the warning and any user text.
            let unsupported_msg = format!(
                "⚠️ Unsupported attachment type: {mime_str} ({display_name}).\n\
                 Supported: images (JPEG, PNG, WebP, GIF), PDFs, and text documents.",
            );
            if !caption.is_empty() {
                user_text = format!("{unsupported_msg}\n\n{caption}");
            } else if let Some(text) = &msg.text {
                user_text = format!("{unsupported_msg}\n\n{text}");
            } else {
                user_text = unsupported_msg;
            }
            attachment_infos.push(AttachmentInfo {
                display_name: display_name.clone(),
                mime_type: mime_str.to_string(),
                size_bytes,
                downloaded: false,
                persistence_marker: format!(
                    "[Unsupported attachment: {display_name}, type={mime_str}]"
                ),
                extracted_text: None,
            });
            unsupported_warn_set = true;
        } else {
            // Download and process via TelegramBot
            let file_id = &doc.file_id;
            match bot
                .process_attachment(
                    file_id,
                    filename,
                    mime,
                    size_bytes,
                    max_attachment_bytes,
                    max_text_chars,
                )
                .await
            {
                Ok(processed) => {
                    attachment_parts.push(processed.content_part);
                    let display_name =
                        attachment::sanitize_filename(filename.unwrap_or("document"));
                    attachment_infos.push(AttachmentInfo {
                        display_name,
                        mime_type: mime.unwrap_or("application/octet-stream").to_string(),
                        size_bytes,
                        downloaded: processed.downloaded,
                        persistence_marker: processed.persistence_marker,
                        extracted_text: processed.extracted_text,
                    });
                }
                Err(e) => {
                    warn!(chat_id = msg.chat.id, error = %e, "failed to process document attachment");
                    let display_name =
                        attachment::sanitize_filename(filename.unwrap_or("document"));
                    attachment_infos.push(AttachmentInfo {
                        display_name: display_name.clone(),
                        mime_type: mime.unwrap_or("application/octet-stream").to_string(),
                        size_bytes,
                        downloaded: false,
                        persistence_marker: format!(
                            "[Attachment processing failed: {display_name}, {e}]"
                        ),
                        extracted_text: None,
                    });
                    attachment_parts.push(genai::chat::ContentPart::Text(format!(
                        "⚠️ Failed to process document: {e}"
                    )));
                }
            }
        }
    }

    // Determine the user prompt text only if no unsupported warning was set.
    // Unsupported warnings already include the caption/text, so we must not
    // overwrite them.
    if !unsupported_warn_set {
        // Priority: caption > message text > default prompt
        if !caption.is_empty() {
            user_text = caption.to_string();
        } else if let Some(text) = &msg.text {
            user_text = text.clone();
        } else if !attachment_parts.is_empty() {
            // Attachment-only message: use default prompt
            user_text = "Please analyze the attached file(s).".to_string();
        }
    }

    (user_text, attachment_parts, attachment_infos)
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
