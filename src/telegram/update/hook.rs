use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::routing::{get, post};
use humantime::format_duration;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::config::TelegramChannelConfig;
use crate::error::AgentError;
use crate::telegram::bot::{TelegramBot, Update};

use super::TelegramUpdate;

const MAX_WEBHOOK_BODY_BYTES: usize = 1_048_576;

/// Telegram update ingress via webhook push.
pub struct TelegramHook {
    bot: Arc<TelegramBot>,
    config: TelegramChannelConfig,
    secret_token: String,
    sender: mpsc::Sender<Update>,
    receiver: mpsc::Receiver<Update>,
    server: Option<WebhookServer>,
}

impl TelegramHook {
    pub fn new(bot: Arc<TelegramBot>, config: TelegramChannelConfig) -> Self {
        let (sender, receiver) = mpsc::channel(100);
        Self {
            bot,
            config,
            secret_token: Uuid::new_v4().simple().to_string(),
            sender,
            receiver,
            server: None,
        }
    }
}

#[async_trait]
impl TelegramUpdate for TelegramHook {
    async fn init(&mut self) -> Result<(), AgentError> {
        let webhook_url = self.config.web_hook_url.clone().ok_or_else(|| {
            AgentError::Config(
                "channels.telegram.web_hook_url is required when channels.telegram.ingress is \"webhook\"".into(),
            )
        })?;

        let mut server = WebhookServer::start(
            self.config.clone(),
            self.secret_token.clone(),
            self.sender.clone(),
        )
        .await?;

        if let Err(e) = self.bot.set_webhook(&webhook_url, &self.secret_token).await {
            server.stop().await;
            return Err(e);
        }

        info!("Telegram webhook ingress initialized");
        self.server = Some(server);
        Ok(())
    }

    async fn poll(&mut self) -> Result<Option<Update>, AgentError> {
        self.receiver
            .recv()
            .await
            .map(Some)
            .ok_or_else(|| AgentError::Telegram("Telegram webhook update channel closed".into()))
    }
}

#[derive(Clone)]
struct WebhookState {
    secret_token: String,
    sender: mpsc::Sender<Update>,
    start_time: Instant,
}

struct WebhookServer {
    shutdown_tx: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
}

impl WebhookServer {
    async fn start(
        config: TelegramChannelConfig,
        secret_token: String,
        sender: mpsc::Sender<Update>,
    ) -> Result<Self, AgentError> {
        let route_path = config
            .web_hook_url
            .as_deref()
            .and_then(|url| url::Url::parse(url).ok())
            .map(|url| {
                let path = url.path();
                if path.is_empty() {
                    "/".to_string()
                } else {
                    path.to_string()
                }
            })
            .ok_or_else(|| {
                AgentError::Config("channels.telegram.web_hook_url is not a valid URL".into())
            })?;

        let start_time = Instant::now();
        let state = WebhookState {
            secret_token,
            sender,
            start_time,
        };
        let app = Router::new()
            .route(&route_path, post(handle_telegram_webhook))
            .route("/health", get(handle_health))
            .layer(DefaultBodyLimit::max(MAX_WEBHOOK_BODY_BYTES))
            .with_state(state);

        let bind_target = format!("{}:{}", config.host, config.port);
        let listener = tokio::net::TcpListener::bind(&bind_target)
            .await
            .map_err(|e| {
                AgentError::Telegram(format!(
                    "failed to bind Telegram webhook server at {bind_target}: {e}"
                ))
            })?;
        let local_addr = listener.local_addr().map_err(|e| {
            AgentError::Telegram(format!("failed to read webhook bind address: {e}"))
        })?;

        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let task = tokio::spawn(async move {
            info!(
                bind_addr = %local_addr,
                route_path,
                "Telegram webhook server listening"
            );
            if let Err(e) = axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await
            {
                error!(error = %e, "Telegram webhook server failed");
            }
        });

        Ok(Self {
            shutdown_tx: Some(shutdown_tx),
            task,
        })
    }

    async fn stop(&mut self) {
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }
        if let Err(e) = (&mut self.task).await {
            error!(error = %e, "Telegram webhook server task failed during shutdown");
        }
    }
}

impl Drop for WebhookServer {
    fn drop(&mut self) {
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
            debug!("sent Telegram webhook server shutdown signal");
        }
    }
}

async fn handle_health(State(state): State<WebhookState>) -> (StatusCode, HeaderMap, String) {
    let mut headers = HeaderMap::new();
    headers.insert(
        HeaderName::from_static("cache-control"),
        HeaderValue::from_static("no-store, no-cache, must-revalidate"),
    );
    headers.insert(
        HeaderName::from_static("pragma"),
        HeaderValue::from_static("no-cache"),
    );
    headers.insert(
        HeaderName::from_static("expires"),
        HeaderValue::from_static("0"),
    );
    let uptime = state.start_time.elapsed();
    (
        StatusCode::OK,
        headers,
        format!(
            "NerdBot is healthy and running for {}\n",
            format_duration(uptime)
        ),
    )
}

async fn handle_telegram_webhook(
    State(state): State<WebhookState>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    const SECRET_HEADER: &str = "x-telegram-bot-api-secret-token";

    let valid_secret = headers
        .get(SECRET_HEADER)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|secret| secret == state.secret_token);
    if !valid_secret {
        warn!("rejected Telegram webhook request: invalid secret token");
        return StatusCode::UNAUTHORIZED;
    }

    let update: Update = match serde_json::from_slice(&body) {
        Ok(update) => update,
        Err(e) => {
            warn!(error = %e, "rejected Telegram webhook request: invalid update JSON");
            return StatusCode::BAD_REQUEST;
        }
    };

    if let Err(e) = state.sender.send(update).await {
        error!(error = %e, "failed to enqueue Telegram webhook update");
        return StatusCode::SERVICE_UNAVAILABLE;
    }

    StatusCode::OK
}
