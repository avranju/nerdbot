//! Runtime MCP tool providers.

mod client;
mod manager;
mod tool_proxy;

pub use client::McpClientHandle;
pub use manager::McpManager;
pub use tool_proxy::{McpToolProxy, map_call_tool_result, validate_exposed_tool_name};
