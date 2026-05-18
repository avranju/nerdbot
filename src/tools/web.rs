//! Web tools — web_search, web_fetch.

use serde_json::json;

use crate::error::AgentError;
use crate::tools::traits::{Tool, ToolContext, ToolOutput};

pub struct WebSearch;

#[async_trait::async_trait]
impl Tool for WebSearch {
    fn name(&self) -> &'static str { "web_search" }
    fn description(&self) -> &'static str { "Search the web." }
    fn input_schema(&self) -> serde_json::Value { json!({}) }
    async fn execute(&self, _args: serde_json::Value, _ctx: ToolContext) -> Result<ToolOutput, AgentError> {
        Err(AgentError::Generic("web_search not yet implemented".into()))
    }
}

pub struct WebFetch;

#[async_trait::async_trait]
impl Tool for WebFetch {
    fn name(&self) -> &'static str { "web_fetch" }
    fn description(&self) -> &'static str { "Fetch a URL and extract readable content." }
    fn input_schema(&self) -> serde_json::Value { json!({}) }
    async fn execute(&self, _args: serde_json::Value, _ctx: ToolContext) -> Result<ToolOutput, AgentError> {
        Err(AgentError::Generic("web_fetch not yet implemented".into()))
    }
}
