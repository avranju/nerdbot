use std::sync::Arc;
use std::time::Duration;

use rmcp::model::{CallToolRequestParams, CallToolResult, Tool};
use rmcp::service::{RoleClient, RunningService, ServiceError, ServiceExt};
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use tokio::sync::{Mutex, MutexGuard, watch};
use tokio::time::Instant;

use crate::error::AgentError;

use super::http::{McpHttpClient, sanitize_error};

type Session = RunningService<RoleClient, ()>;

/// A long-lived Streamable HTTP MCP client session. Calls are never replayed:
/// a lost response does not tell us whether the server executed the tool.
pub struct McpClientHandle {
    pub(crate) server_name: String,
    url: String,
    bearer_token: Option<String>,
    connect_timeout: Duration,
    call_timeout: Duration,
    http: McpHttpClient,
    connection: Mutex<Option<Session>>,
    shutdown: watch::Sender<bool>,
}

impl McpClientHandle {
    pub async fn connect(
        server_name: impl Into<String>,
        url: impl Into<String>,
        bearer_token: Option<String>,
        connect_timeout: Duration,
        call_timeout: Duration,
    ) -> Result<Arc<Self>, AgentError> {
        let http = McpHttpClient::new(connect_timeout, call_timeout, bearer_token.clone())?;
        let handle = Arc::new(Self {
            server_name: server_name.into(),
            url: url.into().trim().to_owned(),
            bearer_token,
            connect_timeout,
            call_timeout,
            http,
            connection: Mutex::new(None),
            shutdown: watch::channel(false).0,
        });
        let service = handle.open(Instant::now() + connect_timeout).await?;
        *handle.connection.lock().await = Some(service);
        tracing::info!(
            server = %handle.server_name,
            implementation = ?handle.peer_implementation(),
            "MCP connection established"
        );
        Ok(handle)
    }

    fn error(&self, operation: &str, detail: &str) -> AgentError {
        AgentError::Mcp(sanitize_error(
            &format!("server {} {operation}: {detail}", self.server_name),
            self.bearer_token.as_deref(),
        ))
    }

    async fn open(&self, deadline: Instant) -> Result<Session, AgentError> {
        let mut config = StreamableHttpClientTransportConfig::with_uri(self.url.clone())
            // Session renewal also replays the outstanding tools/call. Recover on the next
            // independent call instead, regardless of tool annotations.
            .reinit_on_expired_session(false);
        if let Some(token) = &self.bearer_token {
            config = config.auth_header(token.clone());
        }
        let mut shutdown = self.shutdown.subscribe();
        let deadline = deadline.min(Instant::now() + self.connect_timeout);
        tokio::select! {
            biased;
            _ = shutdown.wait_for(|closed| *closed) => Err(self.error("connection", "shut down")),
            result = tokio::time::timeout_at(deadline, ().serve(
                StreamableHttpClientTransport::with_client(self.http.clone(), config),
            )) => match result {
                Ok(Ok(service)) => Ok(service),
                Ok(Err(error)) => Err(self.error("handshake failed", &error.to_string())),
                Err(_) => Err(self.error("connection", "timed out")),
            }
        }
    }

    async fn connection(&self, deadline: Instant) -> Result<SessionLease<'_>, AgentError> {
        let mut shutdown = self.shutdown.subscribe();
        let guard = tokio::select! {
            biased;
            _ = shutdown.wait_for(|closed| *closed) => return Err(self.error("connection", "shut down")),
            result = tokio::time::timeout_at(deadline, self.connection.lock()) => {
                result.map_err(|_| self.error("waiting for connection", "timed out"))?
            }
        };
        if Instant::now() >= deadline {
            return Err(self.error("waiting for connection", "timed out"));
        }
        let mut lease = SessionLease {
            guard,
            discard: false,
        };
        // This lock also serializes recovery: queued callers share one successful handshake.
        if lease.guard.is_none() {
            tracing::info!(server = %self.server_name, "reconnecting MCP session before a new call");
            *lease.guard = Some(self.open(deadline).await?);
        }
        // A shutdown can arrive as the handshake completes. Never publish a usable session
        // after closure, even when both select branches became ready at the same time.
        if *self.shutdown.borrow() {
            lease.discard = true;
            return Err(self.error("connection", "shut down"));
        }
        if Instant::now() >= deadline {
            return Err(self.error("connection", "timed out"));
        }
        Ok(lease)
    }

    pub async fn list_tools(&self, timeout: Duration) -> Result<Vec<Tool>, AgentError> {
        let deadline = Instant::now() + timeout;
        let mut lease = self.connection(deadline).await?;
        let mut shutdown = self.shutdown.subscribe();
        lease.discard = true;
        let result = tokio::select! {
            biased;
            _ = shutdown.wait_for(|closed| *closed) => return Err(self.error("tools/list", "shut down")),
            result = tokio::time::timeout_at(deadline, lease.guard.as_ref().unwrap().list_all_tools()) => result,
        };
        match result {
            Ok(result) => {
                lease.discard = result.as_ref().is_err_and(is_connection_error);
                result.map_err(|error| self.error("tools/list failed", &error.to_string()))
            }
            Err(_) => Err(self.error("tools/list", "timed out")),
        }
    }

    pub async fn call_tool(
        &self,
        params: CallToolRequestParams,
    ) -> Result<CallToolResult, AgentError> {
        let deadline = Instant::now() + self.call_timeout;
        let mut lease = self.connection(deadline).await?;
        let mut shutdown = self.shutdown.subscribe();
        // Discard on timeout, shutdown, or cancellation of the caller's future. That prevents
        // a still-pending HTTP request from blocking the next call on this transport.
        lease.discard = true;
        let result = tokio::select! {
            biased;
            _ = shutdown.wait_for(|closed| *closed) => return Err(self.error("tools/call", "shut down")),
            result = tokio::time::timeout_at(deadline, lease.guard.as_ref().unwrap().call_tool(params)) => result,
        };
        match result {
            Ok(result) => {
                lease.discard = result.as_ref().is_err_and(is_connection_error);
                result.map_err(|error| {
                    self.error(
                        "tools/call failed (not retried; execution may have completed)",
                        &error.to_string(),
                    )
                })
            }
            Err(_) => Err(self.error(
                "tools/call",
                "timed out; not retried, execution may have completed",
            )),
        }
    }

    pub async fn shutdown(&self) {
        // Signal before waiting for the session lock, so active calls and handshakes stop.
        // send_replace retains the terminal value even when no receiver is subscribed.
        self.shutdown.send_replace(true);
        let connection = self.connection.lock().await.take();
        if let Some(mut service) = connection {
            tracing::info!(server = %self.server_name, "closing MCP connection");
            let _ = service.close_with_timeout(Duration::from_secs(5)).await;
        }
    }

    /// Return the sanitized implementation identity reported by the handshake.
    pub fn peer_implementation(&self) -> Option<(String, String)> {
        self.connection.try_lock().ok().and_then(|connection| {
            let info = connection.as_ref()?.peer_info()?;
            let implementation = info.server_info.as_ref()?;
            Some((
                sanitize_peer_field(&implementation.name, self.bearer_token.as_deref()),
                sanitize_peer_field(&implementation.version, self.bearer_token.as_deref()),
            ))
        })
    }

    pub fn server_name(&self) -> &str {
        &self.server_name
    }
}

fn is_connection_error(error: &ServiceError) -> bool {
    matches!(
        error,
        ServiceError::TransportSend(_)
            | ServiceError::TransportClosed
            | ServiceError::Timeout { .. }
    )
}

/// Cancelling a request future must retire its transport as well as release the mutex.
struct SessionLease<'a> {
    guard: MutexGuard<'a, Option<Session>>,
    discard: bool,
}

impl Drop for SessionLease<'_> {
    fn drop(&mut self) {
        if self.discard
            && let Some(mut service) = self.guard.take()
        {
            tokio::spawn(async move {
                let _ = service.close_with_timeout(Duration::from_secs(5)).await;
            });
        }
    }
}

fn sanitize_peer_field(value: &str, token: Option<&str>) -> String {
    sanitize_error(value, token).chars().take(128).collect()
}
