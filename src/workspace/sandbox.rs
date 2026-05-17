//! Workspace sandbox — validates and restricts file access.
//!
//! Implementations come in Phase 8.

use std::path::{Path, PathBuf};

use crate::error::AgentError;

/// A sandboxed workspace root.
pub struct WorkspaceSandbox {
    root: PathBuf,
    max_read_bytes: usize,
    max_write_bytes: usize,
}

impl WorkspaceSandbox {
    pub fn new(root: PathBuf, max_read_bytes: usize, max_write_bytes: usize) -> Self {
        Self {
            root,
            max_read_bytes,
            max_write_bytes,
        }
    }

    /// Resolve a relative path within the sandbox.
    ///
    /// Rejects path traversal attempts by checking for `..` components.
    pub fn resolve(&self, relative: &Path) -> Result<PathBuf, AgentError> {
        // Reject any path that contains `..` components
        for component in relative.components() {
            match component {
                std::path::Component::Normal(_) => {}
                std::path::Component::ParentDir => {
                    return Err(AgentError::SandboxViolation);
                }
                std::path::Component::RootDir => {
                    return Err(AgentError::SandboxViolation);
                }
                _ => {}
            }
        }

        let resolved = self.root.join(relative);
        // Try to canonicalize; if the path doesn't exist, still check prefix
        let canonical = resolved.canonicalize().unwrap_or(resolved);

        if !canonical.starts_with(&self.root) {
            return Err(AgentError::SandboxViolation);
        }

        Ok(canonical)
    }

    /// Check that the proposed read size is within limits.
    pub fn check_read_size(&self, size: usize) -> Result<(), AgentError> {
        if size > self.max_read_bytes {
            return Err(AgentError::FileIo(format!(
                "Read size {} exceeds maximum {}",
                size, self.max_read_bytes
            )));
        }
        Ok(())
    }

    /// Check that the proposed write size is within limits.
    pub fn check_write_size(&self, size: usize) -> Result<(), AgentError> {
        if size > self.max_write_bytes {
            return Err(AgentError::FileIo(format!(
                "Write size {} exceeds maximum {}",
                size, self.max_write_bytes
            )));
        }
        Ok(())
    }

    /// Get the root path (for reference).
    pub fn root(&self) -> &Path {
        &self.root
    }
}
