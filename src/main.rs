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
    ChannelMessageHandler, ChannelMessageHandlerInput, ChannelRegistry, ChannelService,
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
use nerdbot::mcp::McpManager;
use nerdbot::scheduler::service::SchedulerService;
use nerdbot::storage::Database;
use nerdbot::telegram::runtime::{
    TelegramRuntime, TelegramWebhookRuntime, build_runtime as build_telegram_runtime,
    run_ingress_loop as run_telegram_ingress_loop,
};
use nerdbot::telegram::service::TelegramService;
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
use nerdbot::zulip::ZulipService;
use nerdbot::zulip::runtime::{
    ZulipRuntime, ZulipWebhookRuntime, build_runtime as build_zulip_runtime,
    run_ingress_loop as run_zulip_ingress_loop,
};
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
    mcp: Arc<McpManager>,
}

async fn build_runtime(
    config: AppConfig,
    diagnostics_socket: Option<PathBuf>,
) -> Result<AppRuntime, AgentError> {
    ensure_channel_enabled(&config)?;

    let db = init_storage(&config).await?;
    let (registry, mcp) = build_tool_registry(&config).await?;
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
        mcp,
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

async fn build_tool_registry(
    config: &AppConfig,
) -> Result<(Arc<ToolRegistry>, Arc<McpManager>), AgentError> {
    let mut registry = ToolRegistry::new();
    registry.register(EchoTool)?;
    registry.register(CalculatorTool)?;
    registry.register(ScheduleJob)?;
    registry.register(ListJobs)?;
    registry.register(DeleteJob)?;
    registry.register(RunJobNow)?;
    registry.register(SendUserMessage)?;

    let file_config = FileConfig::from(config.files.clone());
    registry.register(ReadFile::new(file_config.clone()))?;
    registry.register(WriteFile::new(file_config.clone()))?;
    registry.register(AppendFile::new(file_config.clone()))?;
    registry.register(ListDirectory::new(file_config))?;

    let exa_api_key = std::env::var(&config.exa.api_key_env).unwrap_or_default();
    if exa_api_key.is_empty() {
        warn!(
            env_var = config.exa.api_key_env,
            "Exa API key not set — web_search and web_fetch will return errors"
        );
    }
    registry.register(WebSearch::new(exa_api_key.clone(), config.exa.max_results))?;
    registry.register(WebFetch::new(exa_api_key, config.exa.max_text_chars))?;

    registry.register(ShellExecute::new(ShellConfig {
        allowed_commands: config.shell.allowed_commands.clone(),
        denied_commands: config.shell.denied_commands.clone(),
        max_output_bytes: config.shell.max_output_bytes,
        timeout_secs: config.shell.timeout_secs,
        sandbox_mode: config.shell.sandbox_mode.clone(),
        network_access: config.shell.network_access.clone(),
    }))?;

    let mcp = McpManager::initialize(config, &mut registry).await?;
    Ok((Arc::new(registry), Arc::new(mcp)))
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
    runtime.mcp.shutdown().await;
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
