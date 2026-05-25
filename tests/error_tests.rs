#![allow(
    dead_code,
    unused,
    unused_imports,
    unused_variables,
    unused_assignments
)]
//! Tests for error types in `src/error.rs`.

use nerdbot::error::{
    AgentError, CompactionError, TokenEstimationError, ToolError, WebFetchError, WebSearchError,
};

// ── AgentError display ───────────────────────────────────────────────────

#[test]
fn test_agent_error_display_llm_provider() {
    let err = AgentError::LlmProvider("rate limited".into());
    assert_eq!(err.to_string(), "LLM provider error: rate limited");
}

#[test]
fn test_agent_error_display_tool_execution() {
    let err = AgentError::ToolExecution("timeout".into());
    assert_eq!(err.to_string(), "Tool execution failed: timeout");
}

#[test]
fn test_agent_error_display_tool_not_found() {
    let err = AgentError::ToolNotFound("read_file".into());
    assert_eq!(err.to_string(), "Tool not found: read_file");
}

#[test]
fn test_agent_error_display_invalid_tool_args() {
    let err = AgentError::InvalidToolArgs("missing 'path' field".into());
    assert_eq!(
        err.to_string(),
        "Invalid tool arguments: missing 'path' field"
    );
}

#[test]
fn test_agent_error_display_telegram() {
    let err = AgentError::Telegram("bot not found".into());
    assert_eq!(err.to_string(), "Telegram error: bot not found");
}

#[test]
fn test_agent_error_display_storage() {
    let err = AgentError::Storage("table not found".into());
    assert_eq!(err.to_string(), "Storage error: table not found");
}

#[test]
fn test_agent_error_display_config() {
    let err = AgentError::Config("missing 'provider' field".into());
    assert_eq!(
        err.to_string(),
        "Configuration error: missing 'provider' field"
    );
}

#[test]
fn test_agent_error_display_max_tool_iterations() {
    let err = AgentError::MaxToolIterationsExceeded(10);
    assert_eq!(err.to_string(), "Max tool iterations (10) exceeded");
}

#[test]
fn test_agent_error_display_context() {
    let err = AgentError::Context("budget exceeded".into());
    assert_eq!(err.to_string(), "Context management error: budget exceeded");
}

#[test]
fn test_agent_error_display_scheduler() {
    let err = AgentError::Scheduler("job not found".into());
    assert_eq!(err.to_string(), "Scheduler error: job not found");
}

#[test]
fn test_agent_error_display_web_search() {
    let err = AgentError::WebSearch("backend error".into());
    assert_eq!(err.to_string(), "Web search error: backend error");
}

#[test]
fn test_agent_error_display_web_fetch() {
    let err = AgentError::WebFetch("bad gateway".into());
    assert_eq!(err.to_string(), "Web fetch error: bad gateway");
}

#[test]
fn test_agent_error_display_file_io() {
    let err = AgentError::FileIo("permission denied".into());
    assert_eq!(err.to_string(), "File I/O error: permission denied");
}

#[test]
fn test_agent_error_display_sandbox_violation() {
    let err = AgentError::SandboxViolation;
    assert_eq!(
        err.to_string(),
        "Workspace sandbox violation: attempted access outside sandbox root"
    );
}

#[test]
fn test_agent_error_display_token_estimation() {
    let err = AgentError::TokenEstimation("model not found".into());
    assert_eq!(err.to_string(), "Token estimation error: model not found");
}

#[test]
fn test_agent_error_display_compaction() {
    let err = AgentError::Compaction("model call failed".into());
    assert_eq!(err.to_string(), "Compaction error: model call failed");
}

#[test]
fn test_agent_error_display_permission_denied() {
    let err = AgentError::PermissionDenied;
    assert_eq!(err.to_string(), "Permission denied: unauthorized sender");
}

#[test]
fn test_agent_error_display_timeout() {
    let err = AgentError::Timeout("request timed out".into());
    assert_eq!(err.to_string(), "Timeout: request timed out");
}

#[test]
fn test_agent_error_display_generic() {
    let err = AgentError::Generic("something went wrong".into());
    assert_eq!(err.to_string(), "Generic error: something went wrong");
}

// ── Debug trait ──────────────────────────────────────────────────────────

#[test]
fn test_agent_error_debug() {
    let err = AgentError::LlmProvider("test".into());
    let debug_str = format!("{err:?}");
    assert!(debug_str.contains("LlmProvider"));
    assert!(debug_str.contains("test"));
}

#[test]
fn test_agent_error_debug_sandbox() {
    let err = AgentError::SandboxViolation;
    let debug_str = format!("{err:?}");
    assert!(debug_str.contains("SandboxViolation"));
}

// ── ToolError ────────────────────────────────────────────────────────────

#[test]
fn test_tool_error_display_invalid_input() {
    let err = ToolError::InvalidInput("missing 'path'".into());
    assert_eq!(err.to_string(), "Invalid input: missing 'path'");
}

#[test]
fn test_tool_error_display_execution() {
    let err = ToolError::Execution("disk full".into());
    assert_eq!(err.to_string(), "Execution failed: disk full");
}

#[test]
fn test_tool_error_display_rate_limited() {
    let err = ToolError::RateLimited;
    assert_eq!(err.to_string(), "Rate limited");
}

// ── WebSearchError ───────────────────────────────────────────────────────

#[test]
fn test_web_search_error_display_backend() {
    let err = WebSearchError::Backend("connection refused".into());
    assert_eq!(err.to_string(), "Search backend error: connection refused");
}

#[test]
fn test_web_search_error_display_invalid_query() {
    let err = WebSearchError::InvalidQuery;
    assert_eq!(err.to_string(), "Invalid query");
}

#[test]
fn test_web_search_error_display_rate_limited() {
    let err = WebSearchError::RateLimited;
    assert_eq!(err.to_string(), "Rate limited");
}

// ── WebFetchError ────────────────────────────────────────────────────────

#[test]
fn test_web_fetch_error_display_http() {
    let err = WebFetchError::Http("404 Not Found".into());
    assert_eq!(err.to_string(), "HTTP error: 404 Not Found");
}

#[test]
fn test_web_fetch_error_display_invalid_url() {
    let err = WebFetchError::InvalidUrl("not a url".into());
    assert_eq!(err.to_string(), "Invalid URL: not a url");
}

#[test]
fn test_web_fetch_error_display_ssrf_blocked() {
    let err = WebFetchError::SsrfBlocked;
    assert_eq!(
        err.to_string(),
        "SSRF protection blocked request to private/localhost address"
    );
}

#[test]
fn test_web_fetch_error_display_response_too_large() {
    let err = WebFetchError::ResponseTooLarge;
    assert_eq!(err.to_string(), "Response exceeded maximum size");
}

#[test]
fn test_web_fetch_error_display_too_many_redirects() {
    let err = WebFetchError::TooManyRedirects;
    assert_eq!(err.to_string(), "Too many redirects");
}

#[test]
fn test_web_fetch_error_display_rate_limited() {
    let err = WebFetchError::RateLimited;
    assert_eq!(err.to_string(), "Rate limited");
}

#[test]
fn test_web_fetch_error_display_timeout() {
    let err = WebFetchError::Timeout;
    assert_eq!(err.to_string(), "Timeout");
}

// ── TokenEstimationError ─────────────────────────────────────────────────

#[test]
fn test_token_estimation_error_display_not_supported() {
    let err = TokenEstimationError::NotSupported("model".into());
    assert_eq!(
        err.to_string(),
        "Provider does not support token estimation: model"
    );
}

#[test]
fn test_token_estimation_error_display_failed() {
    let err = TokenEstimationError::Failed("bad response".into());
    assert_eq!(err.to_string(), "Estimation failed: bad response");
}

// ── CompactionError ──────────────────────────────────────────────────────

#[test]
fn test_compaction_error_display_model_call() {
    let err = CompactionError::ModelCall("context overflow".into());
    assert_eq!(
        err.to_string(),
        "Compaction model call failed: context overflow"
    );
}

#[test]
fn test_compaction_error_display_no_history() {
    let err = CompactionError::NoHistory;
    assert_eq!(err.to_string(), "No eligible history to compact");
}

#[test]
fn test_compaction_error_display_already_running() {
    let err = CompactionError::AlreadyRunning;
    assert_eq!(err.to_string(), "Compaction already running for session");
}

#[test]
fn test_compaction_error_display_storage() {
    let err = CompactionError::Storage("write failed".into());
    assert_eq!(
        err.to_string(),
        "Storage error during compaction: write failed"
    );
}
