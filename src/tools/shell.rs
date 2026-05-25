//! Shell execution tool — run shell commands within the workspace sandbox.
//!
//! Provides a sandboxed environment for executing shell commands with:
//! - Command validation (allowlist / denylist)
//! - Execution timeout
//! - Output size limits
//! - Working directory isolation within the workspace root

use std::path::PathBuf;
use std::time::Duration;

use serde_json::json;

use crate::error::AgentError;
use crate::tools::traits::{Tool, ToolContext, ToolOutput};
use crate::workspace::sandbox::WorkspaceSandbox;

/// Configuration for shell command execution.
#[derive(Debug, Clone)]
pub struct ShellConfig {
    /// Allowed commands (empty means allow all).
    pub allowed_commands: Vec<String>,
    /// Denied commands (checked after allowlist).
    pub denied_commands: Vec<String>,
    /// Maximum output size in bytes.
    pub max_output_bytes: usize,
    /// Command execution timeout in seconds.
    pub timeout_secs: u64,
}

impl Default for ShellConfig {
    fn default() -> Self {
        Self {
            allowed_commands: vec![],
            denied_commands: vec![
                "rm".into(),
                "chmod".into(),
                "chown".into(),
                "mkfs".into(),
                "dd".into(),
                "wget".into(),
                "curl".into(),
            ],
            max_output_bytes: 1_048_576, // 1 MB
            timeout_secs: 30,
        }
    }
}

/// A tool that executes shell commands within a sandboxed workspace.
///
/// Commands are validated against allowlist/denylist and run with
/// output size limits and timeout enforcement.
pub struct ShellExecute {
    config: ShellConfig,
}

impl ShellExecute {
    pub fn new(config: ShellConfig) -> Self {
        Self { config }
    }

    /// Validate a command against the allowlist and denylist.
    ///
    /// Returns an error if the command is denied.
    fn validate_command(&self, cmd: &str) -> Result<(), AgentError> {
        // Parse the first word as the command name
        let command_name = cmd
            .split_whitespace()
            .next()
            .ok_or_else(|| AgentError::InvalidToolArgs("empty command".into()))?;

        // Check denylist first
        for denied in &self.config.denied_commands {
            if command_name == denied {
                return Err(AgentError::ToolExecution(format!(
                    "command denied: {command_name}"
                )));
            }
        }

        // If allowlist is non-empty, check it
        if !self.config.allowed_commands.is_empty() {
            let allowed = self
                .config
                .allowed_commands
                .iter()
                .any(|a| a == command_name);
            if !allowed {
                return Err(AgentError::ToolExecution(format!(
                    "command not in allowlist: {command_name}"
                )));
            }
        }

        Ok(())
    }

    /// Truncate output to the specified maximum size.
    fn truncate_output(&self, output: String, max_bytes: usize) -> (String, bool) {
        if output.len() <= max_bytes {
            return (output, false);
        }
        // Truncate to max bytes and add a notice
        let truncated = String::from_utf8_lossy(&output.as_bytes()[..max_bytes]).to_string();
        (
            format!(
                "{truncated}\n\n[Output truncated: exceeded {max_bytes} bytes]",
            ),
            true,
        )
    }
}

#[async_trait::async_trait]
impl Tool for ShellExecute {
    fn name(&self) -> &'static str {
        "shell_execute"
    }

    fn description(&self) -> &'static str {
        "Execute a shell command within the workspace sandbox. Supports command validation (allowlist/denylist), timeout, and output size limits. The command runs in the workspace root unless a working_directory is specified."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to execute (e.g., 'ls -la', 'grep pattern file.txt')."
                },
                "working_directory": {
                    "type": "string",
                    "description": "Optional subdirectory within the workspace to run the command in. Defaults to the workspace root."
                },
                "timeout_seconds": {
                    "type": "integer",
                    "description": "Optional execution timeout in seconds. Defaults to 30.",
                    "minimum": 1,
                    "maximum": 300
                },
                "max_output_bytes": {
                    "type": "integer",
                    "description": "Optional maximum output size in bytes. Defaults to 1048576 (1 MB).",
                    "minimum": 1024,
                    "maximum": 10485760
                }
            },
            "required": ["command"]
        })
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        ctx: ToolContext,
    ) -> Result<ToolOutput, AgentError> {
        // Parse command argument
        let command = args
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::InvalidToolArgs("missing or invalid 'command' field".into()))?;

        // Validate command against allowlist/denylist
        self.validate_command(command)?;

        // Parse optional arguments
        let working_dir: PathBuf = args
            .get("working_directory")
            .and_then(|v| v.as_str())
            .map(PathBuf::from)
            .unwrap_or_default();

        let timeout_secs: u64 = args
            .get("timeout_seconds")
            .and_then(|v| v.as_u64())
            .unwrap_or(self.config.timeout_secs)
            .max(1)
            .min(300);

        let max_output_bytes: usize = args
            .get("max_output_bytes")
            .and_then(|v| v.as_u64())
            .unwrap_or(self.config.max_output_bytes as u64)
            .max(1024)
            .min(10_485_760) as usize;

        // Resolve working directory within sandbox
        let sandbox = WorkspaceSandbox::new(
            ctx.workspace_root.clone(),
            max_output_bytes,
            max_output_bytes,
        );
        let resolved_dir = sandbox.resolve(&working_dir)?;

        // Build and execute the command
        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c")
            .arg(command)
            .current_dir(&resolved_dir)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);

        // Execute with timeout
        let output = tokio::time::timeout(
            Duration::from_secs(timeout_secs),
            cmd.output(),
        )
        .await;

        let (exit_code, stdout, stderr) = match output {
            Ok(Ok(result)) => {
                // status.code() is None when the process was killed by a signal
                // (e.g., timeout kill_on_drop). Treat as exit -1.
                let code = result.status.code().unwrap_or(-1);
                (code, result.stdout, result.stderr)
            }
            Ok(Err(e)) => {
                return Ok(ToolOutput {
                    success: false,
                    data: json!({ "error": format!("Command execution failed: {e}") }),
                    summary: format!("Command execution failed: {e}"),
                });
            }
            Err(_) => {
                return Ok(ToolOutput {
                    success: false,
                    data: json!({
                        "error": format!("Command timed out after {timeout_secs}s"),
                        "exit_code": -1i64,
                    }),
                    summary: format!("Command timed out after {timeout_secs}s"),
                });
            }
        };

        // Combine stdout and stderr
        let mut combined = stdout.clone();
        combined.extend_from_slice(&stderr);

        // Truncate if needed
        let output_str = String::from_utf8_lossy(&combined).to_string();
        let (output_str, truncated) = self.truncate_output(output_str, max_output_bytes);

        let success = exit_code == 0 && !truncated;

        Ok(ToolOutput {
            success,
            data: json!({
                "output": output_str,
                "exit_code": exit_code,
                "truncated": truncated,
                "working_directory": resolved_dir.to_string_lossy().to_string(),
                "timeout_seconds": timeout_secs,
            }),
            summary: if truncated {
                format!("Command completed with truncated output (exit {exit_code})")
            } else if exit_code == 0 {
                format!("Command completed successfully (exit {exit_code})")
            } else {
                format!("Command failed with exit code {exit_code}")
            },
        })
    }
}
