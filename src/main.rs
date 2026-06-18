//! NerdBot — a minimal, self-hosted AI agent runtime.
//!
//! Run as a single binary, Docker-friendly.
//! Supports Telegram as the user-facing channel with iterative tool use
//! driven by multiple LLM providers via the `genai` crate.

use std::path::PathBuf;
use std::sync::Arc;

use clap::{Args, Parser, Subcommand};
use nerdbot::agent::personality::Personality;
use nerdbot::channel::{
    AttachmentInfo, ChannelInboundEvent, ChannelMessageHandler, ChannelMessageHandlerInput,
    ChannelRegistry, ChannelService, ConversationAddress, InboundMessage, SenderIdentity,
};
use nerdbot::config::{
    AppConfig, TelegramChannelConfig, TelegramIngress, ZulipChannelConfig, ZulipIngress,
};
use nerdbot::context::budget::ContextBudget;
use nerdbot::context::compaction_service::CompactionService;
use nerdbot::context::compaction_worker::CompactionWorker;
use nerdbot::diagnostics::server::DiagnosticsServer;
use nerdbot::error::AgentError;
use nerdbot::llm::{LlmClient, LlmExecutor};
use nerdbot::scheduler::service::SchedulerService;
use nerdbot::storage::Database;
use nerdbot::telegram::attachment::{self, AttachmentKind};
use nerdbot::telegram::bot::{TelegramBot, Update};
use nerdbot::telegram::commands::TelegramCommand;
use nerdbot::telegram::service::TelegramService;
use nerdbot::telegram::update::TelegramUpdate;
use nerdbot::telegram::{TelegramHook, TelegramPoll};
use nerdbot::tools::calculator::CalculatorTool;
use nerdbot::tools::echo::EchoTool;
use nerdbot::tools::files::{AppendFile, FileConfig, ListDirectory, ReadFile, WriteFile};
use nerdbot::tools::messaging::SendUserMessage;
use nerdbot::tools::registry::ToolRegistry;
use nerdbot::tools::schedule::{DeleteJob, ListJobs, RunJobNow, ScheduleJob};
use nerdbot::tools::shell::{ShellConfig, ShellExecute};
use nerdbot::tools::web::{WebFetch, WebSearch};
use nerdbot::webhook::{
    TelegramWebhookRoute, WebhookServer, WebhookServerConfig, ZulipWebhookRoute,
};
use nerdbot::zulip::update::ZulipUpdate;
use nerdbot::zulip::update::hook::ZulipWebhookPayload;
use nerdbot::zulip::{ZulipBot, ZulipHook, ZulipPoll, ZulipService};
use tokio::sync::mpsc;
use tracing::{error, info, warn};
use uuid::Uuid;

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
        #[arg(long, conflicts_with_all = ["channel_id", "conversation_id"])]
        session_id: Option<String>,
        /// Channel ID, e.g. telegram.
        #[arg(long, conflicts_with = "session_id", requires = "conversation_id")]
        channel_id: Option<String>,
        /// Channel-specific conversation ID.
        #[arg(long, conflicts_with = "session_id", requires = "channel_id")]
        conversation_id: Option<String>,
        /// Optional channel-specific thread/topic ID.
        #[arg(long, conflicts_with = "session_id")]
        thread_id: Option<String>,
        /// Include full personality and compaction prompt bodies.
        #[arg(long)]
        show_prompts: bool,
    },
}

#[tokio::main]
async fn main() {
    init_tracing();

    let cli = Cli::parse();

    if let Some(command) = cli.command {
        handle_cli_command(&cli.config, cli.diagnostics_socket.as_deref(), command).await;
        return;
    }

    let config = match load_config(&cli.config) {
        Ok(config) => config,
        Err(e) => {
            error!(error = %e, "invalid configuration");
            return;
        }
    };

    let runtime = match build_runtime(config, cli.diagnostics_socket).await {
        Ok(runtime) => runtime,
        Err(e) => {
            error!(error = %e, "failed to initialize runtime");
            return;
        }
    };

    run_runtime(runtime).await;
}

fn init_tracing() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
}

async fn handle_cli_command(
    config_path: &std::path::Path,
    diagnostics_socket: Option<&std::path::Path>,
    command: Command,
) {
    match command {
        Command::Onboard => {
            if let Err(e) = nerdbot::onboarding::run(config_path) {
                error!(error = %e, "onboarding failed");
            }
        }
        Command::Diagnostics(args) => {
            let Some(socket_path) = diagnostics_socket else {
                eprintln!("error: --diagnostics-socket <path> is required");
                std::process::exit(2);
            };
            if let Err(e) = run_diagnostics_command(socket_path, args).await {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
    }
}

fn load_config(config_path: &std::path::Path) -> Result<AppConfig, AgentError> {
    info!(config_path = %config_path.display(), "starting nerdbot");

    let config = match AppConfig::from_file(config_path) {
        Ok(config) => config,
        Err(e) if !config_path.exists() => {
            tracing::warn!(error = %e, "no config file found, using defaults");
            AppConfig::default()
        }
        Err(e) => return Err(e),
    };

    info!(
        agent_name = config.agent.name,
        model = config.llm.model,
        "configuration loaded"
    );
    Ok(config)
}

struct AppRuntime {
    scheduler: Arc<SchedulerService>,
    diagnostics_server: Option<DiagnosticsServer>,
    webhook_server: Option<WebhookServer>,
    handler: Arc<ChannelMessageHandler>,
    telegram: Option<TelegramRuntime>,
    zulip: Option<ZulipRuntime>,
}

struct TelegramRuntime {
    bot: Arc<TelegramBot>,
    config: TelegramChannelConfig,
    webhook: Option<TelegramWebhookRuntime>,
}

struct TelegramWebhookRuntime {
    secret_token: String,
    receiver: mpsc::Receiver<Update>,
}

struct ZulipRuntime {
    bot: Arc<ZulipBot>,
    config: ZulipChannelConfig,
    webhook: Option<ZulipWebhookRuntime>,
}

struct ZulipWebhookRuntime {
    receiver: mpsc::Receiver<ZulipWebhookPayload>,
}

async fn build_runtime(
    config: AppConfig,
    diagnostics_socket: Option<PathBuf>,
) -> Result<AppRuntime, AgentError> {
    ensure_channel_enabled(&config)?;

    let db = init_storage(&config).await?;
    let registry = build_tool_registry(&config);
    let llm = build_llm(&config)?;
    let compaction_service = build_compaction_service(&config, db.clone(), llm.clone());
    let personality = Personality::from_config(&config);
    let diagnostics_server = start_diagnostics_server(
        diagnostics_socket,
        db.clone(),
        compaction_service.clone(),
        config.clone(),
        personality.clone(),
        registry.clone(),
    )
    .await?;
    let telegram = build_telegram_runtime(&config.channels.telegram).await?;
    let zulip = build_zulip_runtime(&config.channels.zulip).await?;
    let mut telegram = telegram;
    let mut zulip = zulip;
    let webhook_server = start_webhook_server(&config, telegram.as_mut(), zulip.as_mut()).await?;
    let channel_registry = build_channel_registry(&telegram, &zulip);
    let scheduler = build_scheduler(
        db.clone(),
        llm.clone(),
        registry.clone(),
        channel_registry.clone(),
        config.clone(),
        personality.clone(),
    );
    let handler = build_message_handler(MessageHandlerDeps {
        db,
        llm,
        registry,
        channel_registry,
        config,
        personality,
        scheduler: scheduler.clone(),
        compaction_service,
    });

    Ok(AppRuntime {
        scheduler,
        diagnostics_server,
        webhook_server,
        handler,
        telegram,
        zulip,
    })
}

fn ensure_channel_enabled(config: &AppConfig) -> Result<(), AgentError> {
    if !config.channels.telegram.enabled && !config.channels.zulip.enabled {
        return Err(AgentError::Config(
            "No communication channels are enabled".to_string(),
        ));
    }
    Ok(())
}

async fn init_storage(config: &AppConfig) -> Result<Arc<Database>, AgentError> {
    let db = Database::new(config.storage.sqlite_path.clone()).await?;
    db.init().await?;
    Ok(Arc::new(db))
}

async fn build_telegram_runtime(
    config: &TelegramChannelConfig,
) -> Result<Option<TelegramRuntime>, AgentError> {
    if !config.enabled {
        return Ok(None);
    }

    let bot_token = required_env(&config.bot_token_env, "Telegram bot token")?;
    let bot = Arc::new(TelegramBot::new(bot_token));

    match bot.get_me().await {
        Ok(user) => {
            info!(
                bot_username = ?user.username,
                bot_name = user.first_name,
                "Telegram bot authenticated"
            );
        }
        Err(e) => return Err(e),
    }

    if let Err(e) = bot.set_my_commands(&TelegramCommand::menu_commands()).await {
        warn!(
            error = %e,
            "failed to configure Telegram command menu; slash commands still work when typed manually"
        );
    }

    Ok(Some(TelegramRuntime {
        bot,
        config: config.clone(),
        webhook: None,
    }))
}

fn build_tool_registry(config: &AppConfig) -> Arc<ToolRegistry> {
    let mut registry = ToolRegistry::new();
    registry.register(EchoTool);
    registry.register(CalculatorTool);
    registry.register(ScheduleJob);
    registry.register(ListJobs);
    registry.register(DeleteJob);
    registry.register(RunJobNow);
    registry.register(SendUserMessage);

    let file_config = FileConfig::from(config.files.clone());
    registry.register(ReadFile::new(file_config.clone()));
    registry.register(WriteFile::new(file_config.clone()));
    registry.register(AppendFile::new(file_config.clone()));
    registry.register(ListDirectory::new(file_config));

    let exa_api_key = std::env::var(&config.exa.api_key_env).unwrap_or_default();
    if exa_api_key.is_empty() {
        warn!(
            env_var = config.exa.api_key_env,
            "Exa API key not set — web_search and web_fetch will return errors"
        );
    }
    registry.register(WebSearch::new(exa_api_key.clone(), config.exa.max_results));
    registry.register(WebFetch::new(exa_api_key, config.exa.max_text_chars));

    registry.register(ShellExecute::new(ShellConfig {
        allowed_commands: config.shell.allowed_commands.clone(),
        denied_commands: config.shell.denied_commands.clone(),
        max_output_bytes: config.shell.max_output_bytes,
        timeout_secs: config.shell.timeout_secs,
        sandbox_mode: config.shell.sandbox_mode.clone(),
        network_access: config.shell.network_access.clone(),
    }));

    Arc::new(registry)
}

fn build_llm(config: &AppConfig) -> Result<Arc<dyn LlmExecutor>, AgentError> {
    if config.llm.model.is_empty() {
        return Err(AgentError::Config(
            "LLM model not configured. Set llm.model in config.toml (e.g. gpt-4o, claude-sonnet-4-5).".to_string(),
        ));
    }

    let client = LlmClient::from_config(config)?;
    info!(model = config.llm.model, "LLM client initialized via genai");
    Ok(Arc::new(client))
}

fn build_compaction_service(
    config: &AppConfig,
    db: Arc<Database>,
    llm: Arc<dyn LlmExecutor>,
) -> Arc<CompactionService> {
    let compaction_budget = ContextBudget::from_llm_and_context(&config.llm, &config.context);
    let compaction_worker = CompactionWorker::new_with_preserve(
        llm,
        config.llm.model.clone(),
        config.llm.temperature,
        config.context.recent_turns_to_preserve,
    );

    Arc::new(CompactionService::new(
        db.pool().clone(),
        Arc::new(compaction_worker),
        compaction_budget,
        config.context.recent_turns_to_preserve,
    ))
}

async fn start_diagnostics_server(
    socket_path: Option<PathBuf>,
    db: Arc<Database>,
    compaction_service: Arc<CompactionService>,
    config: AppConfig,
    personality: Personality,
    registry: Arc<ToolRegistry>,
) -> Result<Option<DiagnosticsServer>, AgentError> {
    let Some(socket_path) = socket_path else {
        return Ok(None);
    };

    DiagnosticsServer::start(
        socket_path,
        db.pool().clone(),
        compaction_service,
        config,
        personality,
        registry,
    )
    .await
    .map(Some)
}

async fn build_zulip_runtime(
    config: &ZulipChannelConfig,
) -> Result<Option<ZulipRuntime>, AgentError> {
    if !config.enabled {
        return Ok(None);
    }

    let bot_email = required_env(&config.bot_email_env, "Zulip bot email")?;
    let api_key = required_env(&config.api_key_env, "Zulip API key")?;
    let bot = Arc::new(ZulipBot::new(config.site_url.clone(), bot_email, api_key));

    info!(
        site_url = config.site_url,
        bot_email = bot.bot_email(),
        "Zulip bot initialized"
    );

    match bot.get_me().await {
        Ok(user_info) => {
            bot.set_bot_name(user_info.full_name);
            let bot_name = bot.bot_name();
            info!(
                bot_name = %bot_name,
                bot_id = user_info.user_id,
                "Zulip bot name resolved"
            );
        }
        Err(e) => {
            warn!(
                error = %e,
                "Failed to fetch Zulip bot name; mention stripping will use broad pattern"
            );
        }
    }

    Ok(Some(ZulipRuntime {
        bot,
        config: config.clone(),
        webhook: None,
    }))
}

async fn start_webhook_server(
    config: &AppConfig,
    telegram: Option<&mut TelegramRuntime>,
    zulip: Option<&mut ZulipRuntime>,
) -> Result<Option<WebhookServer>, AgentError> {
    let telegram_webhook_enabled = config.channels.telegram.enabled
        && config.channels.telegram.ingress == TelegramIngress::Webhook;
    let zulip_webhook_enabled =
        config.channels.zulip.enabled && config.channels.zulip.ingress == ZulipIngress::Webhook;

    if !telegram_webhook_enabled && !zulip_webhook_enabled {
        return Ok(None);
    }

    let mut telegram_route = None;
    let mut zulip_route = None;

    if telegram_webhook_enabled {
        let Some(telegram) = telegram else {
            return Err(AgentError::Config(
                "Telegram webhook ingress is enabled but Telegram runtime was not initialized"
                    .into(),
            ));
        };
        let route_path = telegram_webhook_path(&telegram.config)?;
        let secret_token = Uuid::new_v4().simple().to_string();
        let (sender, receiver) = mpsc::channel(100);
        telegram.webhook = Some(TelegramWebhookRuntime {
            secret_token: secret_token.clone(),
            receiver,
        });
        telegram_route = Some(TelegramWebhookRoute {
            path: route_path,
            secret_token,
            sender,
        });
    }

    if zulip_webhook_enabled {
        let Some(zulip) = zulip else {
            return Err(AgentError::Config(
                "Zulip webhook ingress is enabled but Zulip runtime was not initialized".into(),
            ));
        };
        let route_path = zulip_webhook_path(&zulip.config)?;
        let token = required_env(&zulip.config.web_hook_token_env, "Zulip webhook token")?;
        let (sender, receiver) = mpsc::channel(100);
        zulip.webhook = Some(ZulipWebhookRuntime { receiver });
        zulip_route = Some(ZulipWebhookRoute {
            path: route_path,
            token,
            sender,
        });
    }

    WebhookServer::start(WebhookServerConfig {
        host: config.webhook.host.clone(),
        port: config.webhook.port,
        telegram: telegram_route,
        zulip: zulip_route,
    })
    .await
    .map(Some)
}

fn telegram_webhook_path(config: &TelegramChannelConfig) -> Result<String, AgentError> {
    extract_webhook_path(config.web_hook_url.as_deref())
}

fn zulip_webhook_path(config: &ZulipChannelConfig) -> Result<String, AgentError> {
    extract_webhook_path(config.web_hook_url.as_deref())
}

fn extract_webhook_path(url: Option<&str>) -> Result<String, AgentError> {
    url.and_then(|url| url::Url::parse(url).ok())
        .map(|url| {
            let path = url.path();
            if path.is_empty() {
                "/".to_string()
            } else {
                path.to_string()
            }
        })
        .ok_or_else(|| AgentError::Config("web_hook_url is not a valid URL".into()))
}

fn build_channel_registry(
    telegram: &Option<TelegramRuntime>,
    zulip: &Option<ZulipRuntime>,
) -> Arc<ChannelRegistry> {
    let mut services: Vec<Arc<dyn ChannelService>> = Vec::new();
    if let Some(telegram) = telegram {
        services.push(Arc::new(TelegramService::new(telegram.bot.clone())));
    }
    if let Some(zulip) = zulip {
        services.push(Arc::new(ZulipService::new(zulip.bot.clone())));
    }
    Arc::new(ChannelRegistry::new(services))
}

fn build_scheduler(
    db: Arc<Database>,
    llm: Arc<dyn LlmExecutor>,
    registry: Arc<ToolRegistry>,
    channel_registry: Arc<ChannelRegistry>,
    config: AppConfig,
    personality: Personality,
) -> Arc<SchedulerService> {
    Arc::new(SchedulerService::new(
        db.pool().clone(),
        llm,
        registry,
        channel_registry,
        config,
        personality,
    ))
}

struct MessageHandlerDeps {
    db: Arc<Database>,
    llm: Arc<dyn LlmExecutor>,
    registry: Arc<ToolRegistry>,
    channel_registry: Arc<ChannelRegistry>,
    config: AppConfig,
    personality: Personality,
    scheduler: Arc<SchedulerService>,
    compaction_service: Arc<CompactionService>,
}

fn build_message_handler(deps: MessageHandlerDeps) -> Arc<ChannelMessageHandler> {
    Arc::new(ChannelMessageHandler::new(ChannelMessageHandlerInput {
        pool: deps.db.pool().clone(),
        llm: deps.llm,
        registry: deps.registry,
        channel_registry: deps.channel_registry,
        config: deps.config,
        personality: deps.personality,
        scheduler_notifier: Some(deps.scheduler.notifier()),
        compaction_service: deps.compaction_service,
    }))
}

async fn run_runtime(mut runtime: AppRuntime) {
    let scheduler = runtime.scheduler.clone();
    let _scheduler_guard = SchedulerGuard {
        scheduler: scheduler.clone(),
    };

    if let Err(e) = scheduler.start().await {
        error!(error = %e, "Failed to start scheduler");
        return;
    }

    run_channel_ingress_loops(runtime.telegram, runtime.zulip, runtime.handler.clone()).await;

    info!("Gracefully shutting down services...");
    if let Err(e) = scheduler.stop().await {
        error!(error = %e, "Failed to stop scheduler gracefully");
    }
    if let Some(server) = runtime.diagnostics_server.as_mut() {
        server.stop().await;
    }
    if let Some(server) = runtime.webhook_server.as_mut() {
        server.stop().await;
    }
    info!("Shutdown complete.");
}

async fn run_channel_ingress_loops(
    telegram: Option<TelegramRuntime>,
    zulip: Option<ZulipRuntime>,
    handler: Arc<ChannelMessageHandler>,
) {
    let telegram_handle = telegram.map(|telegram| {
        let handler = handler.clone();
        tokio::spawn(async move {
            run_telegram_ingress_loop(telegram, handler).await;
        })
    });

    let zulip_handle = zulip.map(|zulip| {
        let handler = handler.clone();
        tokio::spawn(async move {
            run_zulip_ingress_loop(zulip, handler).await;
        })
    });

    if let Some(handle) = telegram_handle {
        if let Some(zulip_handle) = zulip_handle {
            tokio::select! {
                _ = handle => {}
                _ = zulip_handle => {}
            }
        } else {
            let _ = handle.await;
        }
    } else if let Some(handle) = zulip_handle {
        let _ = handle.await;
    }
}

async fn run_telegram_ingress_loop(telegram: TelegramRuntime, handler: Arc<ChannelMessageHandler>) {
    match telegram.config.ingress {
        TelegramIngress::Poll => {
            let updates =
                TelegramPoll::new(telegram.bot.clone(), telegram.config.poll_interval_secs);
            if let Err(e) =
                run_telegram_update_loop(updates, telegram.bot, handler, telegram.config).await
            {
                error!(error = %e, "Telegram polling ingress failed");
            }
        }
        TelegramIngress::Webhook => {
            let Some(webhook) = telegram.webhook else {
                error!(
                    "Telegram webhook ingress is enabled but shared webhook receiver is missing"
                );
                return;
            };
            let updates = TelegramHook::new(
                telegram.bot.clone(),
                telegram.config.clone(),
                webhook.secret_token,
                webhook.receiver,
            );
            if let Err(e) =
                run_telegram_update_loop(updates, telegram.bot, handler, telegram.config).await
            {
                error!(error = %e, "Telegram webhook ingress failed");
            }
        }
    }
}

async fn run_zulip_ingress_loop(zulip: ZulipRuntime, handler: Arc<ChannelMessageHandler>) {
    let presence_heartbeat = start_zulip_presence_heartbeat(
        zulip.bot.clone(),
        zulip.config.presence_enabled,
        zulip.config.presence_ping_interval_secs,
    );

    match zulip.config.ingress {
        ZulipIngress::Poll => {
            let updates = ZulipPoll::new(zulip.bot.clone(), zulip.config.poll_interval_secs)
                .with_attachment_limits(
                    zulip.config.max_attachment_bytes,
                    zulip.config.max_text_document_chars,
                );
            if let Err(e) = run_zulip_update_loop(updates, zulip.bot, handler, zulip.config).await {
                error!(error = %e, "Zulip polling ingress failed");
            }
        }
        ZulipIngress::Webhook => {
            let Some(webhook) = zulip.webhook else {
                error!("Zulip webhook ingress is enabled but shared webhook receiver is missing");
                return;
            };
            let updates = ZulipHook::new(webhook.receiver);
            if let Err(e) = run_zulip_update_loop(updates, zulip.bot, handler, zulip.config).await {
                error!(error = %e, "Zulip webhook ingress failed");
            }
        }
    }

    if let Some(handle) = presence_heartbeat {
        handle.abort();
    }
}

fn start_zulip_presence_heartbeat(
    bot: Arc<ZulipBot>,
    enabled: bool,
    interval_secs: u64,
) -> Option<tokio::task::JoinHandle<()>> {
    if !enabled {
        info!("Zulip presence heartbeat disabled");
        return None;
    }

    Some(tokio::spawn(async move {
        let interval = std::time::Duration::from_secs(interval_secs);
        info!(interval_secs, "starting Zulip active presence heartbeat");

        loop {
            if let Err(e) = bot.update_presence("active", true).await {
                if zulip_presence_rejected_for_bot(&e) {
                    warn!(
                        error = %e,
                        "Zulip rejected presence updates for the bot account; disabling presence heartbeat"
                    );
                    return;
                }
                warn!(error = %e, "failed to update Zulip presence");
            }
            tokio::time::sleep(interval).await;
        }
    }))
}

fn zulip_presence_rejected_for_bot(error: &AgentError) -> bool {
    error
        .to_string()
        .contains("This endpoint does not accept bot requests.")
}

async fn run_zulip_update_loop<T>(
    mut updates: T,
    bot: Arc<ZulipBot>,
    handler: Arc<ChannelMessageHandler>,
    config: ZulipChannelConfig,
) -> Result<(), AgentError>
where
    T: ZulipUpdate,
{
    updates.init().await?;

    loop {
        tokio::select! {
            res = updates.poll() => {
                match res {
                    Ok(Some(msg)) => {
                        if let Err(e) = handler
                            .handle_zulip_message(
                                &msg,
                                &bot,
                                bot.bot_email(),
                                config.max_attachment_bytes,
                                config.max_text_document_chars,
                            )
                            .await
                        {
                            error!(error = %e, "Zulip handler error");
                        }
                    }
                    Ok(None) => continue,
                    Err(e) => {
                        error!(error = %e, "Zulip polling failed, retrying in 5s");
                        tokio::select! {
                            _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {}
                            _ = tokio::signal::ctrl_c() => {
                                info!("received Ctrl-C during Zulip retry sleep, shutting down...");
                                return Ok(());
                            }
                        }
                    }
                }
            }
            _ = tokio::signal::ctrl_c() => {
                info!("received Ctrl-C, shutting down Zulip...");
                return Ok(());
            }
        }
    }
}

fn required_env(env_var: &str, description: &str) -> Result<String, AgentError> {
    match std::env::var(env_var) {
        Ok(value) if !value.is_empty() => Ok(value),
        Ok(_) => Err(AgentError::Config(format!(
            "{description} environment variable {env_var} is empty"
        ))),
        Err(_) => Err(AgentError::Config(format!(
            "{description} environment variable {env_var} is not set"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_zulip_bot_presence_rejection() {
        let error = AgentError::Zulip(
            r#"Zulip presence update failed: HTTP 400 Bad Request: {"result":"error","msg":"This endpoint does not accept bot requests.","code":"BAD_REQUEST"}"#
                .to_string(),
        );

        assert!(zulip_presence_rejected_for_bot(&error));
    }
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
            channel_id,
            conversation_id,
            thread_id,
            show_prompts,
        } => DiagnosticsRequest::ShowSession {
            session_id,
            channel_id,
            conversation_id,
            thread_id,
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

async fn run_telegram_update_loop<T>(
    mut updates: T,
    bot: Arc<TelegramBot>,
    handler: Arc<ChannelMessageHandler>,
    telegram_config: TelegramChannelConfig,
) -> Result<(), AgentError>
where
    T: TelegramUpdate,
{
    updates.init().await?;

    loop {
        tokio::select! {
            res = updates.poll() => {
                match res {
                    Ok(Some(update)) => {
                        let bot = bot.clone();
                        let handler = handler.clone();
                        let telegram_config = telegram_config.clone();
                        tokio::spawn(async move {
                            dispatch_telegram_update(
                                update,
                                bot,
                                handler,
                                telegram_config,
                            )
                            .await;
                        });
                    }
                    Ok(None) => {
                        continue;
                    }
                    Err(e) => {
                        error!(error = %e, "Telegram update polling failed, retrying in 5s");
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

    Ok(())
}

async fn dispatch_telegram_update(
    update: Update,
    bot: Arc<TelegramBot>,
    handler: Arc<ChannelMessageHandler>,
    telegram_config: TelegramChannelConfig,
) {
    let msg = match update.message {
        Some(m) => m,
        None => return,
    };

    let chat_id = msg.chat.id;
    let user_id = msg.from.as_ref().map(|u| u.id).unwrap_or(0);
    let address = ConversationAddress::telegram_chat(chat_id);
    let sender = SenderIdentity::new(
        user_id.to_string(),
        msg.from
            .as_ref()
            .map(|u| u.username.clone().unwrap_or_else(|| u.first_name.clone())),
    );

    // Allowlist check BEFORE building/downloading attachments.
    // This prevents untrusted users from triggering expensive downloads.
    let allowed = (telegram_config.allowed_conversations.is_empty()
        || telegram_config
            .allowed_conversations
            .contains(&address.conversation_id))
        && (telegram_config.allowed_senders.is_empty()
            || telegram_config.allowed_senders.contains(&sender.sender_id));
    if !allowed {
        warn!(?address, ?sender, "skipping message: not in allowlist");
        return;
    }

    let (text, attachment_parts, attachment_infos) = build_inbound_message(
        &msg,
        bot.as_ref(),
        telegram_config.max_attachment_bytes,
        telegram_config.max_text_document_chars,
    )
    .await;

    let inbound = InboundMessage {
        text,
        attachment_parts,
        attachments: attachment_infos,
    };

    let event = ChannelInboundEvent {
        address,
        sender,
        message: inbound,
    };

    match handler.handle_event(event).await {
        Ok(()) => {}
        Err(AgentError::PermissionDenied) => {
            error!(chat_id, user_id, "permission denied");
        }
        Err(e) => {
            error!(chat_id, error = %e, "message handler error");
        }
    }
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
        // Best-effort shutdown for abnormal exits and unwinding. Job state is
        // persisted in SQLite, so recovery does not depend on this task running.
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
