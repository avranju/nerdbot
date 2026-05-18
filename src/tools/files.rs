//! File I/O tools — read, write, append, list_directory.

use serde_json::json;

use crate::error::AgentError;
use crate::tools::traits::{Tool, ToolContext, ToolOutput};

pub struct ReadFile;

#[async_trait::async_trait]
impl Tool for ReadFile {
    fn name(&self) -> &'static str {
        "read_file"
    }
    fn description(&self) -> &'static str {
        "Read a text file from the workspace."
    }
    fn input_schema(&self) -> serde_json::Value {
        json!({})
    }
    async fn execute(
        &self,
        _args: serde_json::Value,
        _ctx: ToolContext,
    ) -> Result<ToolOutput, AgentError> {
        Err(AgentError::Generic("read_file not yet implemented".into()))
    }
}

pub struct WriteFile;

#[async_trait::async_trait]
impl Tool for WriteFile {
    fn name(&self) -> &'static str {
        "write_file"
    }
    fn description(&self) -> &'static str {
        "Write text to a file in the workspace."
    }
    fn input_schema(&self) -> serde_json::Value {
        json!({})
    }
    async fn execute(
        &self,
        _args: serde_json::Value,
        _ctx: ToolContext,
    ) -> Result<ToolOutput, AgentError> {
        Err(AgentError::Generic("write_file not yet implemented".into()))
    }
}

pub struct AppendFile;

#[async_trait::async_trait]
impl Tool for AppendFile {
    fn name(&self) -> &'static str {
        "append_file"
    }
    fn description(&self) -> &'static str {
        "Append text to a file in the workspace."
    }
    fn input_schema(&self) -> serde_json::Value {
        json!({})
    }
    async fn execute(
        &self,
        _args: serde_json::Value,
        _ctx: ToolContext,
    ) -> Result<ToolOutput, AgentError> {
        Err(AgentError::Generic("append_file not yet implemented".into()))
    }
}

pub struct ListDirectory;

#[async_trait::async_trait]
impl Tool for ListDirectory {
    fn name(&self) -> &'static str {
        "list_directory"
    }
    fn description(&self) -> &'static str {
        "List files and directories in the workspace."
    }
    fn input_schema(&self) -> serde_json::Value {
        json!({})
    }
    async fn execute(
        &self,
        _args: serde_json::Value,
        _ctx: ToolContext,
    ) -> Result<ToolOutput, AgentError> {
        Err(AgentError::Generic("list_directory not yet implemented".into()))
    }
}
