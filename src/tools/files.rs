//! File I/O tools — read, write, append, list_directory.
//!
//! All file operations are sandboxed to the workspace root to prevent
//! path traversal attacks. Read and write size limits are enforced
//! via the workspace sandbox and are configurable.

use std::io::Write;

use serde_json::json;

use crate::config::FilesConfig;
use crate::error::AgentError;
use crate::tools::traits::{Tool, ToolContext, ToolOutput};
use crate::workspace::sandbox::WorkspaceSandbox;

/// Validate that every existing component in the resolved path tree
/// is within the workspace. Checks each symlink target and ensures
/// no component is a dangling symlink. Must be called BEFORE any
/// filesystem mutation (create_dir_all, write, etc.).
fn validate_path_tree(
    sandbox: &WorkspaceSandbox,
    resolved: &std::path::Path,
) -> Result<(), AgentError> {
    let relative = resolved
        .strip_prefix(sandbox.root())
        .ok()
        .unwrap_or(resolved);

    let components: Vec<_> = relative.components().collect();
    let total = components.len();

    let mut current: std::path::PathBuf = sandbox.root().to_path_buf();
    for (i, component) in components.iter().enumerate() {
        let component_str = component.as_os_str().to_string_lossy().to_string();
        let next = current.join(&component_str);

        // Check if this component exists using symlink_metadata (doesn't follow symlinks)
        match std::fs::symlink_metadata(&next) {
            Ok(meta) => {
                if meta.file_type().is_symlink() {
                    // Symlink: verify its target is within the workspace
                    match std::fs::canonicalize(&next) {
                        Ok(target) => {
                            if !target.starts_with(sandbox.root()) {
                                return Err(AgentError::SandboxViolation);
                            }
                        }
                        Err(_) => {
                            // Dangling symlink — reject
                            return Err(AgentError::SandboxViolation);
                        }
                    }
                } else if !meta.is_dir() {
                    // It's a regular file (or other non-directory).
                    // Only reject if this is an intermediate component —
                    // the final component is allowed to be a file (the target).
                    if i < total - 1 {
                        return Err(AgentError::SandboxViolation);
                    }
                }
                // If it's a regular directory, continue walking
            }
            Err(_) => {
                // Component doesn't exist yet — safe to create later
            }
        }

        current = next;
    }

    Ok(())
}

/// Shared configuration for all file I/O tools.
#[derive(Debug, Clone)]
pub struct FileConfig {
    /// Maximum size in bytes for read operations.
    pub max_read_bytes: usize,
    /// Maximum size in bytes for write operations.
    pub max_write_bytes: usize,
}

impl Default for FileConfig {
    fn default() -> Self {
        Self {
            max_read_bytes: 262_144,
            max_write_bytes: 262_144,
        }
    }
}

impl From<FilesConfig> for FileConfig {
    fn from(config: FilesConfig) -> Self {
        Self {
            max_read_bytes: config.max_read_bytes,
            max_write_bytes: config.max_write_bytes,
        }
    }
}

pub struct ReadFile {
    config: FileConfig,
}

impl ReadFile {
    pub fn new(config: FileConfig) -> Self {
        Self { config }
    }

    pub fn default_for_test() -> Self {
        Self {
            config: FileConfig::default(),
        }
    }
}

#[async_trait::async_trait]
impl Tool for ReadFile {
    fn name(&self) -> &'static str {
        "read_file"
    }

    fn description(&self) -> &'static str {
        "Read the contents of a text file from the workspace. The file path must be within the workspace root. Returns the file content as a string."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Relative path to the file within the workspace."
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        ctx: ToolContext,
    ) -> Result<ToolOutput, AgentError> {
        let path_str = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::InvalidToolArgs("missing or invalid 'path' field".into()))?;

        let sandbox = WorkspaceSandbox::new(
            ctx.workspace_root.clone(),
            self.config.max_read_bytes,
            self.config.max_write_bytes,
        );

        let resolved = sandbox.resolve(std::path::Path::new(path_str))?;
        sandbox.check_read_size(resolved.metadata().map(|m| m.len() as usize).unwrap_or(0))?;

        let content = std::fs::read_to_string(&resolved).map_err(|e| {
            AgentError::FileIo(format!("Failed to read file '{}': {e}", resolved.display()))
        })?;

        Ok(ToolOutput {
            success: true,
            data: json!({
                "path": path_str,
                "content": content,
                "bytes": content.len(),
            }),
            summary: format!("Read {} bytes from {}", content.len(), path_str),
        })
    }
}

pub struct WriteFile {
    config: FileConfig,
}

impl WriteFile {
    pub fn new(config: FileConfig) -> Self {
        Self { config }
    }

    pub fn default_for_test() -> Self {
        Self {
            config: FileConfig::default(),
        }
    }
}

#[async_trait::async_trait]
impl Tool for WriteFile {
    fn name(&self) -> &'static str {
        "write_file"
    }

    fn description(&self) -> &'static str {
        "Write text content to a file in the workspace, overwriting any existing content. Creates parent directories if they do not exist. The file path must be within the workspace root."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Relative path to the file within the workspace."
                },
                "content": {
                    "type": "string",
                    "description": "Text content to write to the file."
                }
            },
            "required": ["path", "content"]
        })
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        ctx: ToolContext,
    ) -> Result<ToolOutput, AgentError> {
        let path_str = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::InvalidToolArgs("missing or invalid 'path' field".into()))?;

        let content = args
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                AgentError::InvalidToolArgs("missing or invalid 'content' field".into())
            })?;

        let sandbox = WorkspaceSandbox::new(
            ctx.workspace_root.clone(),
            self.config.max_read_bytes,
            self.config.max_write_bytes,
        );

        sandbox.check_write_size(content.len())?;

        let resolved = sandbox.resolve(std::path::Path::new(path_str))?;

        // Validate the entire path tree BEFORE creating any directories.
        // This prevents symlink-escape attacks where create_dir_all would
        // follow a symlink to create directories outside the workspace.
        validate_path_tree(&sandbox, &resolved)?;

        // Create parent directories if they don't exist (now safe, tree validated)
        if let Some(parent) = resolved.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                AgentError::FileIo(format!(
                    "Failed to create parent directory '{}': {e}",
                    parent.display()
                ))
            })?;
        }

        std::fs::write(&resolved, content).map_err(|e| {
            AgentError::FileIo(format!(
                "Failed to write file '{}': {e}",
                resolved.display()
            ))
        })?;

        Ok(ToolOutput {
            success: true,
            data: json!({
                "path": path_str,
                "bytes_written": content.len(),
            }),
            summary: format!("Wrote {} bytes to {}", content.len(), path_str),
        })
    }
}

pub struct AppendFile {
    config: FileConfig,
}

impl AppendFile {
    pub fn new(config: FileConfig) -> Self {
        Self { config }
    }

    pub fn default_for_test() -> Self {
        Self {
            config: FileConfig::default(),
        }
    }
}

#[async_trait::async_trait]
impl Tool for AppendFile {
    fn name(&self) -> &'static str {
        "append_file"
    }

    fn description(&self) -> &'static str {
        "Append text content to the end of a file in the workspace. Creates the file if it does not exist. Creates parent directories if they do not exist. The file path must be within the workspace root."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Relative path to the file within the workspace."
                },
                "content": {
                    "type": "string",
                    "description": "Text content to append to the file."
                }
            },
            "required": ["path", "content"]
        })
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        ctx: ToolContext,
    ) -> Result<ToolOutput, AgentError> {
        let path_str = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::InvalidToolArgs("missing or invalid 'path' field".into()))?;

        let content = args
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                AgentError::InvalidToolArgs("missing or invalid 'content' field".into())
            })?;

        let sandbox = WorkspaceSandbox::new(
            ctx.workspace_root.clone(),
            self.config.max_read_bytes,
            self.config.max_write_bytes,
        );

        sandbox.check_write_size(content.len())?;

        let resolved = sandbox.resolve(std::path::Path::new(path_str))?;

        // Check existing file size and ensure total won't exceed the write limit.
        let existing_size = resolved
            .metadata()
            .ok()
            .map(|m| m.len() as usize)
            .unwrap_or(0);
        if existing_size + content.len() > self.config.max_write_bytes {
            return Err(AgentError::FileIo(format!(
                "Append would exceed write limit: {} + {} > {}",
                existing_size,
                content.len(),
                self.config.max_write_bytes
            )));
        }

        // Validate the entire path tree BEFORE creating any directories.
        validate_path_tree(&sandbox, &resolved)?;

        // Create parent directories if they don't exist (now safe, tree validated)
        if let Some(parent) = resolved.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                AgentError::FileIo(format!(
                    "Failed to create parent directory '{}': {e}",
                    parent.display()
                ))
            })?;
        }

        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&resolved)
            .map_err(|e| {
                AgentError::FileIo(format!(
                    "Failed to open file for appending '{}': {e}",
                    resolved.display()
                ))
            })?
            .write_all(content.as_bytes())
            .map_err(|e| {
                AgentError::FileIo(format!(
                    "Failed to append to file '{}': {e}",
                    resolved.display()
                ))
            })?;

        Ok(ToolOutput {
            success: true,
            data: json!({
                "path": path_str,
                "bytes_appended": content.len(),
            }),
            summary: format!("Appended {} bytes to {}", content.len(), path_str),
        })
    }
}

pub struct ListDirectory {
    config: FileConfig,
}

impl ListDirectory {
    pub fn new(config: FileConfig) -> Self {
        Self { config }
    }

    pub fn default_for_test() -> Self {
        Self {
            config: FileConfig::default(),
        }
    }
}

#[async_trait::async_trait]
impl Tool for ListDirectory {
    fn name(&self) -> &'static str {
        "list_directory"
    }

    fn description(&self) -> &'static str {
        "List files and directories within the workspace. Returns a list of entries with their names and types (file or directory). Defaults to the workspace root if no path is specified."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Relative path to the directory within the workspace. Defaults to the workspace root."
                }
            },
            "required": []
        })
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        ctx: ToolContext,
    ) -> Result<ToolOutput, AgentError> {
        let path_str = args.get("path").and_then(|v| v.as_str()).unwrap_or("");

        let sandbox = WorkspaceSandbox::new(
            ctx.workspace_root.clone(),
            self.config.max_read_bytes,
            self.config.max_write_bytes,
        );

        let resolved = sandbox.resolve(std::path::Path::new(path_str))?;

        let entries = std::fs::read_dir(&resolved).map_err(|e| {
            AgentError::FileIo(format!(
                "Failed to read directory '{}': {e}",
                resolved.display()
            ))
        })?;

        let mut items: Vec<serde_json::Value> = Vec::new();
        for entry in entries {
            let entry = entry
                .map_err(|e| AgentError::FileIo(format!("Error reading directory entry: {e}")))?;

            let file_name = entry.file_name().to_string_lossy().to_string();
            let metadata = entry.metadata().map_err(|e| {
                AgentError::FileIo(format!("Failed to read metadata for '{}': {e}", file_name))
            })?;

            let entry_type = if metadata.is_dir() {
                "directory"
            } else {
                "file"
            };

            items.push(json!({
                "name": file_name,
                "type": entry_type,
            }));
        }

        // Sort entries by name for deterministic output
        items.sort_by(|a, b| {
            a["name"]
                .as_str()
                .unwrap_or("")
                .cmp(b["name"].as_str().unwrap_or(""))
        });

        Ok(ToolOutput {
            success: true,
            data: json!({
                "path": path_str,
                "entries": items,
                "count": items.len(),
            }),
            summary: format!("Listed {} items in {}", items.len(), path_str),
        })
    }
}
