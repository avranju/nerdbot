use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use crate::config::{AppConfig, McpTransportConfig};
use crate::error::AgentError;
use crate::tools::registry::ToolRegistry;
use crate::tools::traits::Tool;

use super::client::McpClientHandle;
use super::tool_proxy::{McpToolProxy, validate_exposed_tool_name};

/// Owns all successfully initialized MCP sessions for the runtime lifetime.
pub struct McpManager {
    servers: BTreeMap<String, Arc<McpClientHandle>>,
}

impl McpManager {
    pub fn new() -> Self {
        Self {
            servers: BTreeMap::new(),
        }
    }

    pub async fn initialize(
        config: &AppConfig,
        registry: &mut ToolRegistry,
    ) -> Result<Self, AgentError> {
        let mut manager = Self::new();
        for server_config in config.mcp.servers.iter().filter(|server| server.enabled) {
            let result = manager.initialize_server(server_config, registry).await;
            if let Err(error) = result {
                if server_config.required {
                    return Err(error);
                }
                tracing::warn!(
                    server = %server_config.name,
                    transport = "streamable_http",
                    error = %error,
                    "optional MCP server skipped"
                );
            }
        }
        Ok(manager)
    }

    async fn initialize_server(
        &mut self,
        server_config: &crate::config::McpServerConfig,
        registry: &mut ToolRegistry,
    ) -> Result<(), AgentError> {
        let (url, bearer_token_env) = match &server_config.transport {
            McpTransportConfig::StreamableHttp {
                url,
                bearer_token_env,
            } => (url.clone(), bearer_token_env.clone()),
        };
        let bearer_token = match bearer_token_env {
            Some(env) => match std::env::var(&env) {
                Ok(value) if !value.is_empty() => Some(value),
                Ok(_) | Err(_) => {
                    return Err(AgentError::Mcp(format!(
                        "server {} bearer token environment variable is missing or empty",
                        server_config.name
                    )));
                }
            },
            None => None,
        };

        let server = McpClientHandle::connect(
            server_config.name.clone(),
            url,
            bearer_token,
            Duration::from_secs(server_config.connect_timeout_secs),
            Duration::from_secs(server_config.call_timeout_secs),
        )
        .await?;
        let tools = server
            .list_tools(Duration::from_secs(server_config.connect_timeout_secs))
            .await?;
        let discovered_count = tools.len();
        let include = &server_config.include_tools;
        let exclude = &server_config.exclude_tools;
        let prefix = server_config.tool_prefix.as_deref().unwrap_or("");
        let mut proxies: Vec<Arc<dyn Tool>> = Vec::new();

        for remote in tools {
            let remote_name = remote.name.as_ref();
            if !include.is_empty() && !include.iter().any(|name| name == remote_name) {
                continue;
            }
            if exclude.iter().any(|name| name == remote_name) {
                continue;
            }
            let exposed_name = format!("{prefix}{remote_name}");
            validate_exposed_tool_name(&exposed_name).map_err(|error| {
                AgentError::Mcp(format!("server {}: {error}", server_config.name))
            })?;
            proxies.push(Arc::new(McpToolProxy::new(
                server.clone(),
                remote,
                exposed_name,
            )?));
        }
        proxies.sort_by(|left, right| left.name().cmp(right.name()));
        let exposed_count = proxies.len();
        let exposed_names: Vec<String> =
            proxies.iter().map(|tool| tool.name().to_string()).collect();
        registry.register_batch_from(proxies, format!("mcp:{}", server_config.name))?;
        tracing::info!(
            server = %server_config.name,
            transport = "streamable_http",
            implementation = ?server.peer_implementation(),
            discovered = discovered_count,
            exposed = exposed_count,
            "MCP server connected"
        );
        tracing::debug!(server = %server_config.name, tool_names = ?exposed_names, "MCP tools exposed");
        self.servers.insert(server_config.name.clone(), server);
        Ok(())
    }

    pub async fn shutdown(&self) {
        for server in self.servers.values() {
            server.shutdown().await;
        }
    }
}

impl Default for McpManager {
    fn default() -> Self {
        Self::new()
    }
}
