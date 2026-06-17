//! Shared HTTP webhook server for channel push ingress.
//!
//! The server owns the Axum listener and routes provider-specific webhook
//! requests into per-channel mpsc queues. Channel ingress implementations stay
//! focused on receiving queued updates.

use std::net::SocketAddr;
use std::time::Instant;

use axum::body::Bytes;
use axum::extract::DefaultBodyLimit;
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use constant_time_eq::constant_time_eq;
use humantime::format_duration;
use serde::Serialize;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

use crate::error::AgentError;
use crate::telegram::bot::Update;
use crate::zulip::update::hook::ZulipWebhookPayload;

const MAX_WEBHOOK_BODY_BYTES: usize = 1_048_576;

#[derive(Serialize)]
struct ZulipWebhookResponse {
    response_not_required: bool,
}

#[derive(Clone)]
pub struct TelegramWebhookRoute {
    pub path: String,
    pub secret_token: String,
    pub sender: mpsc::Sender<Update>,
}

#[derive(Clone)]
pub struct ZulipWebhookRoute {
    pub path: String,
    pub token: String,
    pub sender: mpsc::Sender<ZulipWebhookPayload>,
}

pub struct WebhookServerConfig {
    pub host: String,
    pub port: u16,
    pub telegram: Option<TelegramWebhookRoute>,
    pub zulip: Option<ZulipWebhookRoute>,
}

pub struct WebhookServer {
    shutdown_tx: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
    local_addr: SocketAddr,
}

impl WebhookServer {
    pub async fn start(config: WebhookServerConfig) -> Result<Self, AgentError> {
        if config.telegram.is_none() && config.zulip.is_none() {
            return Err(AgentError::Config(
                "cannot start webhook server without routes".into(),
            ));
        }
        validate_route_paths(&config)?;

        let start_time = Instant::now();
        let mut app = Router::new().route(
            "/health",
            get(move || async move { handle_health(start_time).await }),
        );

        if let Some(route) = config.telegram {
            let path = route.path.clone();
            app = app.route(
                &path,
                post(move |headers: HeaderMap, body: Bytes| {
                    let route = route.clone();
                    async move { handle_telegram_webhook(route, headers, body).await }
                }),
            );
        }

        if let Some(route) = config.zulip {
            let path = route.path.clone();
            app = app.route(
                &path,
                post(move |payload: Json<ZulipWebhookPayload>| {
                    let route = route.clone();
                    async move { handle_zulip_webhook(route, payload).await }
                }),
            );
        }

        let app = app.layer(DefaultBodyLimit::max(MAX_WEBHOOK_BODY_BYTES));
        let bind_target = format!("{}:{}", config.host, config.port);
        let listener = tokio::net::TcpListener::bind(&bind_target)
            .await
            .map_err(|e| {
                AgentError::Generic(format!(
                    "failed to bind webhook server at {bind_target}: {e}"
                ))
            })?;
        let local_addr = listener.local_addr().map_err(|e| {
            AgentError::Generic(format!("failed to read webhook bind address: {e}"))
        })?;

        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let task = tokio::spawn(async move {
            info!(bind_addr = %local_addr, "shared webhook server listening");
            if let Err(e) = axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await
            {
                error!(error = %e, "shared webhook server failed");
            }
        });

        Ok(Self {
            shutdown_tx: Some(shutdown_tx),
            task,
            local_addr,
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub async fn stop(&mut self) {
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }
        if let Err(e) = (&mut self.task).await {
            error!(error = %e, "shared webhook server task failed during shutdown");
        }
    }
}

impl Drop for WebhookServer {
    fn drop(&mut self) {
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
            debug!("sent shared webhook server shutdown signal");
        }
    }
}

fn validate_route_paths(config: &WebhookServerConfig) -> Result<(), AgentError> {
    let reserved_health_path = "/health";

    if let Some(telegram) = &config.telegram
        && telegram.path == reserved_health_path
    {
        return Err(AgentError::Config(
            "Telegram webhook path must not be /health".into(),
        ));
    }

    if let Some(zulip) = &config.zulip {
        if zulip.path == reserved_health_path {
            return Err(AgentError::Config(
                "Zulip webhook path must not be /health".into(),
            ));
        }

        if let Some(telegram) = &config.telegram
            && telegram.path == zulip.path
        {
            return Err(AgentError::Config(format!(
                "Telegram webhook path {} conflicts with Zulip webhook route",
                telegram.path
            )));
        }
    }

    Ok(())
}

async fn handle_health(start_time: Instant) -> (StatusCode, HeaderMap, String) {
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
    let uptime = start_time.elapsed();
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
    route: TelegramWebhookRoute,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    const SECRET_HEADER: &str = "x-telegram-bot-api-secret-token";

    let valid_secret = headers
        .get(SECRET_HEADER)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|secret| secret == route.secret_token);
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

    if let Err(e) = route.sender.send(update).await {
        error!(error = %e, "failed to enqueue Telegram webhook update");
        return StatusCode::SERVICE_UNAVAILABLE;
    }

    StatusCode::OK
}

async fn handle_zulip_webhook(
    route: ZulipWebhookRoute,
    payload: Json<ZulipWebhookPayload>,
) -> axum::response::Response {
    let payload_token = payload.token.as_bytes();
    let expected_token = route.token.as_bytes();
    if !constant_time_eq(payload_token, expected_token) {
        warn!("Zulip webhook received with invalid token");
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let message_id = payload.message.id;
    if route.sender.send(payload.0).await.is_err() {
        warn!("Zulip webhook event receiver dropped");
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }

    debug!(message_id, "Zulip webhook payload received");
    (
        StatusCode::OK,
        Json(ZulipWebhookResponse {
            response_not_required: true,
        }),
    )
        .into_response()
}
