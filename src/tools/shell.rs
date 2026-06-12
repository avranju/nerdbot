//! Shell execution tool — run shell commands within a sandboxed workspace.
//!
//! Provides a sandboxed environment for executing shell commands with:
//! - Command validation (allowlist / denylist)
//! - Execution timeout
//! - Output size limits
//! - Working directory isolation within the workspace root
//! - Optional Bubblewrap namespace isolation (filesystem, PID, network, IPC, UTS)
//!
//! Sandbox modes (configured via `[shell].sandbox_mode`):
//! - `none` — Direct execution (current behaviour, no namespace isolation).
//!   Allow/deny lists only inspect the initial command word and are not a
//!   security boundary against shell features or interpreter subcommands.
//! - `bwrap` — Bubblewrap namespace isolation with read-only system files,
//!   read-write workspace, clean environment, and configurable network access.
//! - `bwrap-strict` — Reserved for future resource limit enforcement.
//!   Currently identical to `bwrap`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::json;

use crate::config::{
    SANDBOX_MODE_BWRAP, SANDBOX_MODE_BWRAP_STRICT, SANDBOX_MODE_NONE,
    SHELL_NETWORK_ACCESS_DISABLED, SHELL_NETWORK_ACCESS_HOST,
};
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
    /// Sandbox isolation mode: "none", "bwrap", or "bwrap-strict".
    pub sandbox_mode: String,
    /// Bubblewrap network access policy: "disabled" or "host".
    pub network_access: String,
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
            sandbox_mode: SANDBOX_MODE_NONE.into(),
            network_access: SHELL_NETWORK_ACCESS_DISABLED.into(),
        }
    }
}

/// Filesystem view policy for bubblewrap sandboxing.
///
/// Defines which host paths are mounted read-only and which are
/// mounted read-write inside the sandbox.
#[derive(Debug, Clone)]
pub struct BwrapPolicy {
    /// Paths mounted read-write (workspace, user directories).
    pub writable_roots: Vec<PathBuf>,
}

impl BwrapPolicy {
    /// Build a policy for the given workspace root.
    pub fn for_workspace(workspace_root: &Path) -> Self {
        Self {
            writable_roots: vec![workspace_root.to_path_buf()],
        }
    }
}

struct ShellOutputMetadata<'a> {
    sandbox_mode: &'a str,
    network_access: &'a str,
    timeout_secs: u64,
    working_directory: Option<String>,
}

/// Check if bubblewrap is available on the system.
pub fn bwrap_available() -> bool {
    std::process::Command::new("bwrap")
        .arg("--version")
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

/// A tool that executes shell commands within a sandboxed workspace.
///
/// Commands are validated against allowlist/denylist and run with
/// output size limits and timeout enforcement.
///
/// When `sandbox_mode` is `"bwrap"` or `"bwrap-strict"`, commands are
/// executed inside a bubblewrap sandbox providing namespace isolation:
/// - Filesystem: read-only system files, read-write workspace
/// - Process: PID namespace isolation
/// - Network: configurable as loopback-only or shared host networking
/// - Environment: clean environment (no host secrets leaked)
/// - IPC/UTS: isolated IPC and hostname
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
    fn truncate_output(output: String, max_bytes: usize) -> (String, bool) {
        if output.len() <= max_bytes {
            return (output, false);
        }
        // Truncate to max bytes and add a notice
        let truncated = String::from_utf8_lossy(&output.as_bytes()[..max_bytes]).to_string();
        (
            format!("{truncated}\n\n[Output truncated: exceeded {max_bytes} bytes]",),
            true,
        )
    }

    /// Build a bubblewrap command for sandboxed execution.
    fn build_bwrap_command(
        &self,
        command: &str,
        policy: &BwrapPolicy,
        workspace_root: &Path,
        working_dir: &Path,
        strict: bool,
    ) -> tokio::process::Command {
        let mut cmd = tokio::process::Command::new("bwrap");

        // Session and namespace isolation
        cmd.args([
            "--new-session",
            "--die-with-parent",
            "--unshare-user",
            "--unshare-pid",
            "--unshare-ipc",
            "--unshare-uts",
        ]);
        if self.config.network_access == SHELL_NETWORK_ACCESS_DISABLED {
            cmd.arg("--unshare-net");
        }

        // Clean environment — no host secrets leaked
        cmd.arg("--clearenv");

        // Set minimal environment variables
        cmd.args(["--setenv", "PATH", "/usr/bin:/bin"])
            .args(["--setenv", "HOME", "/home"])
            .args([
                "--setenv",
                "WORKSPACE",
                workspace_root.to_string_lossy().as_ref(),
            ]);

        // Bind the entire host root read-only as the base filesystem view.
        // This ensures all paths (/bin, /lib, /usr, /etc, etc.) are
        // accessible inside the sandbox. The workspace is then mounted
        // as read-write on top of this base.
        cmd.args(["--ro-bind", "/", "/"]);

        // Read-write workspace
        for path in &policy.writable_roots {
            cmd.args([
                "--bind",
                path.to_string_lossy().as_ref(),
                path.to_string_lossy().as_ref(),
            ]);
        }

        // Virtual filesystems (tmpfs for temp dirs)
        cmd.args(["--tmpfs", "/tmp"])
            .args(["--tmpfs", "/var"])
            .args(["--proc", "/proc"])
            .args(["--dev", "/dev"]);

        // Strict mode — reserved for future resource limit flags
        // (bwrap --rlimit-nproc, --rlimit-as, --rlimit-core).
        // Currently strict mode is identical to bwrap but provides
        // a hook for adding RLIMITs when bwrap supports them.
        let _ = strict;

        // Working directory (relative to workspace root)
        let resolved_dir = if working_dir == Path::new("") || working_dir == Path::new(".") {
            workspace_root.to_path_buf()
        } else {
            workspace_root.join(working_dir)
        };
        cmd.args(["--chdir", resolved_dir.to_string_lossy().as_ref()]);

        // Command to execute (must be LAST)
        cmd.arg("--");
        cmd.args(["/bin/sh", "-c", command]);

        cmd
    }

    /// Build a ToolOutput from command execution results.
    fn build_tool_output(
        exit_code: i32,
        stdout: &[u8],
        stderr: &[u8],
        max_output_bytes: usize,
        metadata: ShellOutputMetadata<'_>,
    ) -> ToolOutput {
        let mut combined = stdout.to_vec();
        combined.extend_from_slice(stderr);
        let output_str = String::from_utf8_lossy(&combined).to_string();
        let (output_str, truncated) = Self::truncate_output(output_str, max_output_bytes);
        let success = exit_code == 0 && !truncated;

        let mut data = json!({
            "output": output_str,
            "exit_code": exit_code,
            "truncated": truncated,
            "sandbox_mode": metadata.sandbox_mode,
            "network_access": metadata.network_access,
            "timeout_seconds": metadata.timeout_secs,
        });
        if let Some(wd) = metadata.working_directory {
            data["working_directory"] = json!(wd);
        }

        ToolOutput {
            success,
            data,
            summary: if truncated {
                format!("Command completed with truncated output (exit {exit_code})")
            } else if exit_code == 0 {
                format!("Command completed successfully (exit {exit_code})")
            } else {
                format!("Command failed with exit code {exit_code}")
            },
        }
    }

    /// Execute a command inside a bubblewrap sandbox.
    async fn execute_with_bwrap(
        &self,
        command: &str,
        policy: &BwrapPolicy,
        workspace_root: &Path,
        working_dir: &Path,
        timeout_secs: u64,
        max_output_bytes: usize,
    ) -> Result<ToolOutput, AgentError> {
        let mut cmd = self.build_bwrap_command(command, policy, workspace_root, working_dir, false);

        // Ensure workspace exists (bwrap --bind fails if the target doesn't exist)
        if !workspace_root.exists()
            && let Err(e) = std::fs::create_dir_all(workspace_root)
        {
            return Ok(ToolOutput {
                success: false,
                data: json!({
                    "error": format!("Failed to create workspace: {e}"),
                    "sandbox_mode": "bwrap",
                }),
                summary: format!("Bubblewrap execution failed: {e}"),
            });
        }

        cmd.stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);

        // Execute with timeout
        let output = tokio::time::timeout(Duration::from_secs(timeout_secs), cmd.output()).await;

        let (exit_code, stdout, stderr) = match output {
            Ok(Ok(result)) => {
                let code = result.status.code().unwrap_or(-1);
                (code, result.stdout, result.stderr)
            }
            Ok(Err(e)) => {
                return Ok(ToolOutput {
                    success: false,
                    data: json!({
                        "error": format!("Bubblewrap execution failed: {e}"),
                        "sandbox_mode": "bwrap",
                    }),
                    summary: format!("Bubblewrap execution failed: {e}"),
                });
            }
            Err(_) => {
                return Ok(ToolOutput {
                    success: false,
                    data: json!({
                        "error": format!("Command timed out after {timeout_secs}s"),
                        "exit_code": -1i64,
                        "sandbox_mode": "bwrap",
                    }),
                    summary: format!("Command timed out after {timeout_secs}s"),
                });
            }
        };

        Ok(Self::build_tool_output(
            exit_code,
            &stdout,
            &stderr,
            max_output_bytes,
            ShellOutputMetadata {
                sandbox_mode: "bwrap",
                network_access: &self.config.network_access,
                timeout_secs,
                working_directory: None,
            },
        ))
    }

    /// Execute a command with bubblewrap-strict (adds resource limits).
    async fn execute_with_bwrap_strict(
        &self,
        command: &str,
        policy: &BwrapPolicy,
        workspace_root: &Path,
        working_dir: &Path,
        timeout_secs: u64,
        max_output_bytes: usize,
    ) -> Result<ToolOutput, AgentError> {
        let mut cmd = self.build_bwrap_command(command, policy, workspace_root, working_dir, true);

        // Ensure workspace exists
        if !workspace_root.exists()
            && let Err(e) = std::fs::create_dir_all(workspace_root)
        {
            return Ok(ToolOutput {
                success: false,
                data: json!({
                    "error": format!("Failed to create workspace: {e}"),
                    "sandbox_mode": "bwrap-strict",
                }),
                summary: format!("Bubblewrap execution failed: {e}"),
            });
        }

        cmd.stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);

        // Execute with timeout
        let output = tokio::time::timeout(Duration::from_secs(timeout_secs), cmd.output()).await;

        let (exit_code, stdout, stderr) = match output {
            Ok(Ok(result)) => {
                let code = result.status.code().unwrap_or(-1);
                (code, result.stdout, result.stderr)
            }
            Ok(Err(e)) => {
                return Ok(ToolOutput {
                    success: false,
                    data: json!({
                        "error": format!("Bubblewrap execution failed: {e}"),
                        "sandbox_mode": "bwrap-strict",
                    }),
                    summary: format!("Bubblewrap execution failed: {e}"),
                });
            }
            Err(_) => {
                return Ok(ToolOutput {
                    success: false,
                    data: json!({
                        "error": format!("Command timed out after {timeout_secs}s"),
                        "exit_code": -1i64,
                        "sandbox_mode": "bwrap-strict",
                    }),
                    summary: format!("Command timed out after {timeout_secs}s"),
                });
            }
        };

        Ok(Self::build_tool_output(
            exit_code,
            &stdout,
            &stderr,
            max_output_bytes,
            ShellOutputMetadata {
                sandbox_mode: "bwrap-strict",
                network_access: &self.config.network_access,
                timeout_secs,
                working_directory: None,
            },
        ))
    }
}

#[async_trait::async_trait]
impl Tool for ShellExecute {
    fn name(&self) -> &'static str {
        "shell_execute"
    }

    fn description(&self) -> &'static str {
        "Execute a shell command within a sandboxed workspace. Supports command validation (allowlist/denylist), timeout, output size limits, and optional Bubblewrap namespace isolation (filesystem, PID, network, IPC, UTS). When sandbox_mode is set, system files are read-only, the workspace is read-write, and network access follows shell.network_access."
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
            .ok_or_else(|| {
                AgentError::InvalidToolArgs("missing or invalid 'command' field".into())
            })?;

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
            .clamp(1, 300);

        let max_output_bytes: usize = args
            .get("max_output_bytes")
            .and_then(|v| v.as_u64())
            .unwrap_or(self.config.max_output_bytes as u64)
            .clamp(1024, 10_485_760) as usize;

        // Resolve working directory within sandbox
        let sandbox = WorkspaceSandbox::new(
            ctx.workspace_root.clone(),
            max_output_bytes,
            max_output_bytes,
        );
        let resolved_dir = sandbox.resolve(&working_dir)?;

        // Build the sandbox policy
        let policy = BwrapPolicy::for_workspace(&ctx.workspace_root);
        if !matches!(
            self.config.network_access.as_str(),
            SHELL_NETWORK_ACCESS_DISABLED | SHELL_NETWORK_ACCESS_HOST
        ) {
            return Err(AgentError::Config(format!(
                "invalid shell network access: '{}' (must be 'disabled' or 'host')",
                self.config.network_access
            )));
        }

        // Execute based on sandbox mode
        match self.config.sandbox_mode.as_str() {
            SANDBOX_MODE_NONE => {
                // Direct execution (current behavior)
                let mut cmd = tokio::process::Command::new("sh");
                cmd.arg("-c")
                    .arg(command)
                    .current_dir(&resolved_dir)
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .kill_on_drop(true);

                let output =
                    tokio::time::timeout(Duration::from_secs(timeout_secs), cmd.output()).await;

                let (exit_code, stdout, stderr) = match output {
                    Ok(Ok(result)) => {
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

                Ok(Self::build_tool_output(
                    exit_code,
                    &stdout,
                    &stderr,
                    max_output_bytes,
                    ShellOutputMetadata {
                        sandbox_mode: "none",
                        network_access: &self.config.network_access,
                        timeout_secs,
                        working_directory: Some(resolved_dir.to_string_lossy().to_string()),
                    },
                ))
            }
            SANDBOX_MODE_BWRAP => {
                // Check if bubblewrap is available
                if !bwrap_available() {
                    return Err(AgentError::ToolExecution(
                        "bubblewrap is not installed — install it with 'apt install bubblewrap' \
                         (Debian/Ubuntu), 'yum install bubblewrap' (RHEL/Fedora), \
                         or 'pacman -S bubblewrap' (Arch). \
                         Alternatively, set sandbox_mode to 'none' to disable sandboxing."
                            .into(),
                    ));
                }

                self.execute_with_bwrap(
                    command,
                    &policy,
                    &ctx.workspace_root,
                    &resolved_dir,
                    timeout_secs,
                    max_output_bytes,
                )
                .await
            }
            SANDBOX_MODE_BWRAP_STRICT => {
                // Check if bubblewrap is available
                if !bwrap_available() {
                    return Err(AgentError::ToolExecution(
                        "bubblewrap is not installed — install it with 'apt install bubblewrap' \
                         (Debian/Ubuntu), 'yum install bubblewrap' (RHEL/Fedora), \
                         or 'pacman -S bubblewrap' (Arch). \
                         Alternatively, set sandbox_mode to 'none' to disable sandboxing."
                            .into(),
                    ));
                }

                self.execute_with_bwrap_strict(
                    command,
                    &policy,
                    &ctx.workspace_root,
                    &resolved_dir,
                    timeout_secs,
                    max_output_bytes,
                )
                .await
            }
            other => Err(AgentError::Config(format!(
                "invalid sandbox mode: '{other}' (must be 'none', 'bwrap', or 'bwrap-strict')"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bwrap_available_check() {
        // This test checks the function is callable; the actual result
        // depends on whether bwrap is installed in the test environment.
        let _available = bwrap_available();
    }

    #[test]
    fn test_sandbox_mode_none_is_default() {
        let config = ShellConfig::default();
        assert_eq!(config.sandbox_mode, SANDBOX_MODE_NONE);
        assert_eq!(config.network_access, SHELL_NETWORK_ACCESS_DISABLED);
    }

    #[test]
    fn test_policy_for_workspace() {
        let policy = BwrapPolicy::for_workspace(Path::new("/workspace"));
        assert!(policy.writable_roots.iter().any(|p| p == "/workspace"));
    }

    #[test]
    fn test_bwrap_disabled_network_adds_unshare_net() {
        let tool = ShellExecute::new(ShellConfig {
            sandbox_mode: SANDBOX_MODE_BWRAP.into(),
            network_access: SHELL_NETWORK_ACCESS_DISABLED.into(),
            ..ShellConfig::default()
        });
        let policy = BwrapPolicy::for_workspace(Path::new("/workspace"));

        let cmd = tool.build_bwrap_command(
            "echo hello",
            &policy,
            Path::new("/workspace"),
            Path::new("/workspace"),
            false,
        );
        let args: Vec<_> = cmd.as_std().get_args().collect();

        assert!(args.iter().any(|arg| *arg == "--unshare-net"));
    }

    #[test]
    fn test_bwrap_host_network_omits_unshare_net() {
        let tool = ShellExecute::new(ShellConfig {
            sandbox_mode: SANDBOX_MODE_BWRAP.into(),
            network_access: SHELL_NETWORK_ACCESS_HOST.into(),
            ..ShellConfig::default()
        });
        let policy = BwrapPolicy::for_workspace(Path::new("/workspace"));

        let cmd = tool.build_bwrap_command(
            "echo hello",
            &policy,
            Path::new("/workspace"),
            Path::new("/workspace"),
            false,
        );
        let args: Vec<_> = cmd.as_std().get_args().collect();

        assert!(!args.iter().any(|arg| *arg == "--unshare-net"));
    }
}
