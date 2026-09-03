use std::sync::Arc;
use std::time::Duration;

use rmcp::model::{CallToolRequestParams, CallToolResult, Tool};
use rmcp::service::{RoleClient, RunningService, ServiceError, ServiceExt};
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use tokio::sync::{Mutex, MutexGuard};

use crate::error::AgentError;

/// A long-lived Streamable HTTP MCP client session.
pub struct McpClientHandle {
    pub(crate) server_name: String,
    url: String,
    bearer_token: Option<String>,
    connect_timeout: Duration,
    call_timeout: Duration,
    connection: Mutex<Option<McpConnection>>,
    reconnect_gate: Mutex<()>,
}

impl McpClientHandle {
    pub async fn connect(
        server_name: impl Into<String>,
        url: impl Into<String>,
        bearer_token: Option<String>,
        connect_timeout: Duration,
        call_timeout: Duration,
    ) -> Result<Arc<Self>, AgentError> {
        let handle = Arc::new(Self {
            server_name: server_name.into(),
            url: url.into(),
            bearer_token,
            connect_timeout,
            call_timeout,
            connection: Mutex::new(None),
            reconnect_gate: Mutex::new(()),
        });
        let service = tokio::time::timeout(connect_timeout, handle.open())
            .await
            .map_err(|_| {
                AgentError::Mcp(format!(
                    "server {} connection timed out",
                    handle.server_name
                ))
            })??;
        *handle.connection.lock().await = Some(McpConnection {
            generation: 0,
            service,
        });
        tracing::info!(
            server = %handle.server_name,
            implementation = ?handle.peer_implementation(),
            "MCP connection established"
        );
        Ok(handle)
    }

    async fn open(&self) -> Result<RunningService<RoleClient, ()>, AgentError> {
        let mut config = StreamableHttpClientTransportConfig::with_uri(self.url.clone())
            .reinit_on_expired_session(true);
        if let Some(token) = &self.bearer_token {
            config = config.auth_header(token.clone());
        }
        ().serve(StreamableHttpClientTransport::from_config(config))
            .await
            .map_err(|e| {
                AgentError::Mcp(format!("server {} handshake failed: {e}", self.server_name))
            })
    }

    async fn connection(&self) -> Result<MutexGuard<'_, Option<McpConnection>>, AgentError> {
        let guard = self.connection.lock().await;
        if guard.is_none() {
            return Err(AgentError::Mcp(format!(
                "server {} is disconnected",
                self.server_name
            )));
        }
        Ok(guard)
    }

    pub async fn list_tools(&self, timeout: Duration) -> Result<Vec<Tool>, AgentError> {
        let guard = self.connection().await?;
        tokio::time::timeout(timeout, guard.as_ref().unwrap().service.list_all_tools())
            .await
            .map_err(|_| {
                AgentError::Mcp(format!("server {} tools/list timed out", self.server_name))
            })?
            .map_err(|e| {
                AgentError::Mcp(format!(
                    "server {} tools/list failed: {e}",
                    self.server_name
                ))
            })
    }

    pub async fn call_tool(
        &self,
        params: CallToolRequestParams,
    ) -> Result<CallToolResult, AgentError> {
        let first = {
            let guard = self.connection().await?;
            let generation = guard.as_ref().unwrap().generation;
            let result = tokio::time::timeout(
                self.call_timeout,
                guard.as_ref().unwrap().service.call_tool(params.clone()),
            )
            .await;
            (generation, result)
        };
        match first {
            (_, Ok(Ok(result))) => Ok(result),
            (
                generation,
                Ok(Err(error @ (ServiceError::TransportSend(_) | ServiceError::TransportClosed))),
            ) => {
                // A single reconnect-and-retry path is intentionally bounded and only applies to
                // transport failures, never MCP-declared or protocol-level failures.
                self.reconnect(generation).await?;
                let guard = self.connection().await?;
                tokio::time::timeout(
                    self.call_timeout,
                    guard.as_ref().unwrap().service.call_tool(params),
                )
                .await
                .map_err(|_| {
                    AgentError::Mcp(format!("server {} tools/call timed out", self.server_name))
                })?
                .map_err(|e| {
                    AgentError::Mcp(format!(
                        "server {} tools/call failed after reconnect: {e} (initial: {error})",
                        self.server_name
                    ))
                })
            }
            (_, Ok(Err(error))) => Err(AgentError::Mcp(format!(
                "server {} tools/call failed: {error}",
                self.server_name
            ))),
            (_, Err(_)) => Err(AgentError::Mcp(format!(
                "server {} tools/call timed out",
                self.server_name
            ))),
        }
    }

    async fn reconnect(&self, failed_generation: u64) -> Result<(), AgentError> {
        let _gate = self.reconnect_gate.lock().await;
        {
            let connection = self.connection.lock().await;
            if connection
                .as_ref()
                .is_some_and(|current| current.generation != failed_generation)
            {
                // Another caller already replaced the connection for this failed generation.
                return Ok(());
            }
        }

        tracing::warn!(
            server = %self.server_name,
            generation = failed_generation,
            "MCP connection lost; reconnecting"
        );

        // Keep the old connection in place until the replacement handshake succeeds. This
        // ensures a failed reconnect does not strand the handle in a disconnected state, and
        // callers arriving while the handshake is in progress can wait for the gate and retry
        // the recovery themselves if necessary.
        let service = tokio::time::timeout(self.connect_timeout, self.open())
            .await
            .map_err(|_| {
                AgentError::Mcp(format!("server {} reconnect timed out", self.server_name))
            })??;
        let next_generation = failed_generation.saturating_add(1);
        let old = {
            let mut connection = self.connection.lock().await;
            if connection
                .as_ref()
                .is_some_and(|current| current.generation != failed_generation)
            {
                drop(connection);
                let mut service = service;
                let _ = service.close_with_timeout(Duration::from_secs(1)).await;
                return Ok(());
            }
            connection.replace(McpConnection {
                generation: next_generation,
                service,
            })
        };
        if let Some(mut old) = old {
            let _ = old.service.close_with_timeout(Duration::from_secs(1)).await;
        }
        tracing::info!(
            server = %self.server_name,
            generation = next_generation,
            implementation = ?self.peer_implementation(),
            "MCP connection re-established"
        );
        Ok(())
    }

    pub async fn shutdown(&self) {
        if let Some(mut connection) = self.connection.lock().await.take() {
            tracing::info!(server = %self.server_name, "closing MCP connection");
            let _ = connection
                .service
                .close_with_timeout(Duration::from_secs(5))
                .await;
        }
    }

    /// Return the sanitized implementation identity reported by the handshake.
    pub fn peer_implementation(&self) -> Option<(String, String)> {
        self.connection
            .try_lock()
            .ok()
            .and_then(|connection| connection.as_ref()?.service.peer_info())
            .and_then(|info| {
                info.server_info.as_ref().map(|implementation| {
                    (
                        sanitize_peer_field(&implementation.name),
                        sanitize_peer_field(&implementation.version),
                    )
                })
            })
    }

    pub fn server_name(&self) -> &str {
        &self.server_name
    }
}

struct McpConnection {
    generation: u64,
    service: RunningService<RoleClient, ()>,
}

fn sanitize_peer_field(value: &str) -> String {
    value
        .chars()
        .filter(|character| !character.is_control())
        .take(128)
        .collect()
}
