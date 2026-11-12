use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Mutex, watch};
use tokio::task::JoinHandle;

use crate::config::{AppConfig, McpServerConfig, McpTransportConfig};
use crate::error::AgentError;
use crate::tools::registry::ToolRegistry;
use crate::tools::traits::Tool;

use super::client::McpClientHandle;
use super::tool_proxy::{McpToolProxy, validate_exposed_tool_name};

/// Equal-jitter exponential backoff: sleep in [cap/2, cap], doubling the cap
/// after each failure. Public timing injection keeps recovery tests fast.
#[derive(Clone, Copy)]
pub struct McpRetryPolicy {
    pub initial_delay: Duration,
    pub max_delay: Duration,
}

impl Default for McpRetryPolicy {
    fn default() -> Self {
        Self {
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(60),
        }
    }
}

impl McpRetryPolicy {
    fn delay(cap: Duration) -> Duration {
        // UUID v4 supplies independent randomness across servers and processes,
        // without a new RNG dependency. No sensitive state is used as a seed.
        let fraction = (uuid::Uuid::new_v4().as_u128() % 1_000_001) as f64 / 1_000_000.0;
        cap.mul_f64(0.5 + fraction * 0.5)
    }
}

/// Owns one supervisor per enabled server, including servers not yet available.
/// A supervisor retries only establishment/discovery, never tools/call.
pub struct McpManager {
    shutdown: watch::Sender<bool>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
}

impl McpManager {
    pub fn new() -> Self {
        Self {
            shutdown: watch::channel(false).0,
            tasks: Mutex::new(Vec::new()),
        }
    }

    pub async fn initialize(
        config: &AppConfig,
        registry: &ToolRegistry,
    ) -> Result<Self, AgentError> {
        Self::initialize_with_retry_policy(config, registry, McpRetryPolicy::default()).await
    }

    pub async fn initialize_with_retry_policy(
        config: &AppConfig,
        registry: &ToolRegistry,
        retry: McpRetryPolicy,
    ) -> Result<Self, AgentError> {
        if retry.initial_delay.is_zero() || retry.max_delay < retry.initial_delay {
            return Err(AgentError::Config("invalid MCP retry delays".into()));
        }
        let manager = Self::new();
        let mut states = Vec::new();
        // Preserve startup ordering and required-server fail-fast semantics. Do not
        // let retries race with the initial pass for collision ownership.
        for config in config.mcp.servers.iter().filter(|server| server.enabled) {
            let mut client = None;
            let pending = match Self::initialize_server(config, registry, &mut client).await {
                Ok(()) => false,
                Err(error) => {
                    let detail = Self::failure_detail(&error, &client);
                    Self::close(&mut client).await;
                    if config.required {
                        for (_, mut client, _) in states {
                            Self::close(&mut client).await;
                        }
                        return Err(error);
                    }
                    tracing::warn!(server = %config.name, transport = "streamable_http",
                        error = %detail, "optional MCP server unavailable; background retry scheduled");
                    true
                }
            };
            states.push((config.clone(), client, pending));
        }
        for (config, client, pending) in states {
            let registry = registry.clone();
            let shutdown = manager.shutdown.subscribe();
            manager
                .tasks
                .lock()
                .await
                .push(tokio::spawn(Self::supervise(
                    config, registry, client, pending, retry, shutdown,
                )));
        }
        Ok(manager)
    }

    async fn supervise(
        config: McpServerConfig,
        registry: ToolRegistry,
        mut client: Option<Arc<McpClientHandle>>,
        mut pending: bool,
        retry: McpRetryPolicy,
        mut shutdown: watch::Receiver<bool>,
    ) {
        let mut cap = retry.initial_delay;
        while pending {
            let delay = McpRetryPolicy::delay(cap);
            tracing::debug!(server = %config.name, delay_ms = delay.as_millis(),
                "waiting to retry optional MCP server");
            tokio::select! {
                biased;
                _ = shutdown.wait_for(|closed| *closed) => break,
                _ = tokio::time::sleep(delay) => {}
            }
            let result = tokio::select! {
                biased;
                _ = shutdown.wait_for(|closed| *closed) => break,
                result = Self::initialize_server(&config, &registry, &mut client) => result,
            };
            match result {
                Ok(()) => {
                    pending = false;
                    tracing::info!(server = %config.name, "optional MCP server recovered; tools available to new agent runs");
                }
                Err(error) => {
                    let detail = Self::failure_detail(&error, &client);
                    tracing::warn!(server = %config.name, error = %detail,
                        "optional MCP server retry failed");
                    Self::close(&mut client).await;
                    cap = cap.saturating_mul(2).min(retry.max_delay);
                }
            }
        }
        if !pending {
            let _ = shutdown.wait_for(|closed| *closed).await;
        }
        Self::close(&mut client).await;
    }

    fn failure_detail(error: &AgentError, client: &Option<Arc<McpClientHandle>>) -> String {
        let detail = error.to_string();
        match client {
            Some(client) => client.sanitize_detail(&detail),
            None => super::http::sanitize_error(&detail, None),
        }
    }

    async fn close(client: &mut Option<Arc<McpClientHandle>>) {
        if let Some(client) = client.take() {
            client.shutdown().await;
        }
    }

    async fn initialize_server(
        config: &McpServerConfig,
        registry: &ToolRegistry,
        client: &mut Option<Arc<McpClientHandle>>,
    ) -> Result<(), AgentError> {
        let McpTransportConfig::StreamableHttp {
            url,
            bearer_token_env,
        } = &config.transport;
        let bearer_token = match bearer_token_env {
            Some(env) => match std::env::var(env) {
                Ok(value) if !value.is_empty() => Some(value),
                Ok(_) | Err(_) => {
                    return Err(AgentError::Mcp(format!(
                        "server {} bearer token environment variable is missing or empty",
                        config.name
                    )));
                }
            },
            None => None,
        };
        // Publish ownership before awaiting anything: cancellation always closes
        // the rejected session, including during handshake or tools/list.
        let server = McpClientHandle::new(
            config.name.clone(),
            url.clone(),
            bearer_token,
            Duration::from_secs(config.connect_timeout_secs),
            Duration::from_secs(config.call_timeout_secs),
        )?;
        *client = Some(server.clone());
        // Keep separate handshake and discovery deadlines, as at startup before
        // background recovery existed. Each phase is independently bounded.
        server.establish().await?;
        let tools = server
            .list_tools(Duration::from_secs(config.connect_timeout_secs))
            .await?;
        let discovered_count = tools.len();
        let prefix = config.tool_prefix.as_deref().unwrap_or("");
        let mut proxies: Vec<Arc<dyn Tool>> = Vec::new();
        for remote in tools {
            let remote_name = remote.name.as_ref();
            if !config.include_tools.is_empty()
                && !config.include_tools.iter().any(|name| name == remote_name)
            {
                continue;
            }
            if config.exclude_tools.iter().any(|name| name == remote_name) {
                continue;
            }
            let exposed_name = format!("{prefix}{remote_name}");
            validate_exposed_tool_name(&exposed_name)
                .map_err(|error| AgentError::Mcp(format!("server {}: {error}", config.name)))?;
            proxies.push(Arc::new(McpToolProxy::new(
                server.clone(),
                remote,
                exposed_name,
            )?));
        }
        proxies.sort_by(|left, right| left.name().cmp(right.name()));
        let exposed_count = proxies.len();
        registry.register_batch_from(proxies, format!("mcp:{}", config.name))?;
        tracing::info!(server = %config.name, transport = "streamable_http",
            implementation = ?server.peer_implementation(), discovered = discovered_count,
            exposed = exposed_count, "MCP server connected");
        Ok(())
    }

    pub async fn shutdown(&self) {
        self.shutdown.send_replace(true);
        // Keep handles in the manager while joining: cancelling shutdown itself
        // must not lose ownership, and concurrent shutdowns wait for completion.
        let mut tasks = self.tasks.lock().await;
        while let Some(task) = tasks.last_mut() {
            let _ = task.await;
            tasks.pop();
        }
    }
}

impl Drop for McpManager {
    fn drop(&mut self) {
        // Also cover startup errors after manager construction. Supervisors own
        // their sessions and exit on this signal, even without explicit shutdown.
        self.shutdown.send_replace(true);
    }
}

impl Default for McpManager {
    fn default() -> Self {
        Self::new()
    }
}
