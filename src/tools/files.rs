//! File I/O tools — read, write, append, list_directory.

use serde_json::json;

use crate::error::AgentError;
use crate::tools::traits::{Tool, ToolContext, ToolOutput};

macro_rules! stub_tool {
    ($name:expr, $desc:expr, $schema:expr) => {
        fn name(&self) -> &'static str { $name }
        fn description(&self) -> &'static str { $desc }
        fn input_schema(&self) -> serde_json::Value { $schema }
        fn execute(&self, _args: serde_json::Value, _ctx: ToolContext) -> Result<ToolOutput, AgentError> {
            Err(AgentError::Generic(concat!($name, " not yet implemented").into()))
        }
    };
}

pub struct ReadFile;
impl Tool for ReadFile {
    stub_tool!("read_file", "Read a text file from the workspace.", json!({}));
}

pub struct WriteFile;
impl Tool for WriteFile {
    stub_tool!("write_file", "Write text to a file in the workspace.", json!({}));
}

pub struct AppendFile;
impl Tool for AppendFile {
    stub_tool!("append_file", "Append text to a file in the workspace.", json!({}));
}

pub struct ListDirectory;
impl Tool for ListDirectory {
    stub_tool!("list_directory", "List files and directories in the workspace.", json!({}));
}
