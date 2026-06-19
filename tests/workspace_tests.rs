#![allow(
    dead_code,
    unused,
    unused_imports,
    unused_variables,
    unused_assignments
)]
//! Tests for workspace sandbox in `src/workspace/sandbox.rs`.

use std::fs;

use nerdbot::error::AgentError;
use nerdbot::workspace::sandbox::WorkspaceSandbox;

// ── Sandbox creation ─────────────────────────────────────────────────────

#[test]
fn test_sandbox_new() {
    let sandbox = WorkspaceSandbox::new("/workspace".into(), 1024, 2048);
    assert_eq!(sandbox.root(), std::path::Path::new("/workspace"));
}

// ── Valid path resolution ────────────────────────────────────────────────

#[test]
fn test_resolve_valid_relative_path() {
    let tmp = tempfile::tempdir().unwrap();
    let sandbox = WorkspaceSandbox::new(tmp.path().to_path_buf(), 1024, 2048);

    let resolved = sandbox.resolve(std::path::Path::new("file.txt"));
    assert!(resolved.is_ok());
    let path = resolved.unwrap();
    assert!(path.starts_with(tmp.path()));
    assert!(path.ends_with("file.txt"));
}

#[test]
fn test_resolve_nested_path() {
    let tmp = tempfile::tempdir().unwrap();
    let sandbox = WorkspaceSandbox::new(tmp.path().to_path_buf(), 1024, 2048);

    let resolved = sandbox.resolve(std::path::Path::new("sub/dir/file.txt"));
    assert!(resolved.is_ok());
    let path = resolved.unwrap();
    assert!(path.starts_with(tmp.path()));
}

#[test]
fn test_resolve_dot_file() {
    let tmp = tempfile::tempdir().unwrap();
    let sandbox = WorkspaceSandbox::new(tmp.path().to_path_buf(), 1024, 2048);

    let resolved = sandbox.resolve(std::path::Path::new(".hidden"));
    assert!(resolved.is_ok());
}

#[test]
fn test_resolve_current_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let sandbox = WorkspaceSandbox::new(tmp.path().to_path_buf(), 1024, 2048);

    let resolved = sandbox.resolve(std::path::Path::new("."));
    // "." resolves to the root itself, which is valid
    assert!(resolved.is_ok());
}

// ── Path traversal rejection ─────────────────────────────────────────────

#[test]
fn test_reject_parent_directory_traversal() {
    let tmp = tempfile::tempdir().unwrap();
    let sandbox = WorkspaceSandbox::new(tmp.path().to_path_buf(), 1024, 2048);

    let result = sandbox.resolve(std::path::Path::new("../etc/passwd"));
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), AgentError::SandboxViolation));
}

#[test]
fn test_reject_multiple_parent_traversals() {
    let tmp = tempfile::tempdir().unwrap();
    let sandbox = WorkspaceSandbox::new(tmp.path().to_path_buf(), 1024, 2048);

    let result = sandbox.resolve(std::path::Path::new("../../etc/shadow"));
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), AgentError::SandboxViolation));
}

#[test]
fn test_reject_traversal_in_middle_of_path() {
    let tmp = tempfile::tempdir().unwrap();
    let sandbox = WorkspaceSandbox::new(tmp.path().to_path_buf(), 1024, 2048);

    // Even if a subdirectory exists with a ".." in it, the resolved path must stay in root
    let result = sandbox.resolve(std::path::Path::new("dir/../../etc/passwd"));
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), AgentError::SandboxViolation));
}

#[test]
fn test_reject_traversal_after_nested_dirs() {
    let tmp = tempfile::tempdir().unwrap();
    let sandbox = WorkspaceSandbox::new(tmp.path().to_path_buf(), 1024, 2048);

    let result = sandbox.resolve(std::path::Path::new("a/b/../../../etc/passwd"));
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), AgentError::SandboxViolation));
}

#[test]
fn test_accept_absolute_path_inside_root() {
    let tmp = tempfile::tempdir().unwrap();
    let sandbox = WorkspaceSandbox::new(tmp.path().to_path_buf(), 1024, 2048);

    // Absolute paths that fall within the workspace root are accepted.
    // This allows the LLM to send absolute paths like "/workspace".
    let result = sandbox.resolve(tmp.path());
    assert!(result.is_ok());
    assert!(result.unwrap().starts_with(tmp.path()));
}

#[test]
fn test_reject_absolute_path_outside_root() {
    let tmp = tempfile::tempdir().unwrap();
    let sandbox = WorkspaceSandbox::new(tmp.path().to_path_buf(), 1024, 2048);

    // Absolute paths outside the workspace root are rejected.
    let result = sandbox.resolve(std::path::Path::new("/etc/passwd"));
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), AgentError::SandboxViolation));
}

// ── Size limits ──────────────────────────────────────────────────────────

#[test]
fn test_check_read_size_within_limit() {
    let sandbox = WorkspaceSandbox::new("/tmp".into(), 1024, 2048);
    assert!(sandbox.check_read_size(512).is_ok());
    assert!(sandbox.check_read_size(1024).is_ok());
}

#[test]
fn test_check_read_size_exceeds_limit() {
    let sandbox = WorkspaceSandbox::new("/tmp".into(), 1024, 2048);
    let result = sandbox.check_read_size(1025);
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), AgentError::FileIo(_)));
}

#[test]
fn test_check_read_size_zero() {
    let sandbox = WorkspaceSandbox::new("/tmp".into(), 1024, 2048);
    assert!(sandbox.check_read_size(0).is_ok());
}

#[test]
fn test_check_write_size_within_limit() {
    let sandbox = WorkspaceSandbox::new("/tmp".into(), 1024, 2048);
    assert!(sandbox.check_write_size(2048).is_ok());
    assert!(sandbox.check_write_size(1024).is_ok());
}

#[test]
fn test_check_write_size_exceeds_limit() {
    let sandbox = WorkspaceSandbox::new("/tmp".into(), 1024, 2048);
    let result = sandbox.check_write_size(2049);
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), AgentError::FileIo(_)));
}

#[test]
fn test_check_write_size_zero() {
    let sandbox = WorkspaceSandbox::new("/tmp".into(), 1024, 2048);
    assert!(sandbox.check_write_size(0).is_ok());
}

// ── Root accessor ────────────────────────────────────────────────────────

#[test]
fn test_root_accessor() {
    let root = std::path::PathBuf::from("/my/workspace");
    let sandbox = WorkspaceSandbox::new(root.clone(), 1024, 2048);
    assert_eq!(sandbox.root(), std::path::Path::new("/my/workspace"));
}

// ── Edge cases ───────────────────────────────────────────────────────────

#[test]
fn test_empty_relative_path() {
    let tmp = tempfile::tempdir().unwrap();
    let sandbox = WorkspaceSandbox::new(tmp.path().to_path_buf(), 1024, 2048);

    let resolved = sandbox.resolve(std::path::Path::new(""));
    assert!(resolved.is_ok());
    assert!(resolved.unwrap().starts_with(tmp.path()));
}

#[test]
fn test_path_with_spaces() {
    let tmp = tempfile::tempdir().unwrap();
    let sandbox = WorkspaceSandbox::new(tmp.path().to_path_buf(), 1024, 2048);

    let resolved = sandbox.resolve(std::path::Path::new("my file.txt"));
    assert!(resolved.is_ok());
}

#[test]
fn test_path_with_special_chars() {
    let tmp = tempfile::tempdir().unwrap();
    let sandbox = WorkspaceSandbox::new(tmp.path().to_path_buf(), 1024, 2048);

    let resolved = sandbox.resolve(std::path::Path::new("file-v1.2.3.txt"));
    assert!(resolved.is_ok());
}
