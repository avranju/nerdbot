//! Application-wide error types.

/// All errors the agent runtime can produce.
#[derive(thiserror::Error, Debug)]
pub enum AgentError {
    #[error("LLM provider error: {0}")]
    LlmProvider(String),

    #[error("Tool execution failed: {0}")]
    ToolExecution(String),

    #[error("Tool not found: {0}")]
    ToolNotFound(String),

    #[error("MCP error: {0}")]
    Mcp(String),

    #[error(
        "Tool name collision for {name:?}: existing provider {existing_provider}, incoming provider {incoming_provider}"
    )]
    ToolNameCollision {
        name: String,
        existing_provider: String,
        incoming_provider: String,
    },

    #[error("Invalid tool arguments: {0}")]
    InvalidToolArgs(String),

    #[error("Telegram error: {0}")]
    Telegram(String),

    #[error("Zulip error: {0}")]
    Zulip(String),

    #[error("Storage error: {0}")]
    Storage(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Max tool iterations ({0}) exceeded")]
    MaxToolIterationsExceeded(u32),

    #[error("Context management error: {0}")]
    Context(String),

    #[error("Scheduler error: {0}")]
    Scheduler(String),

    #[error("Web search error: {0}")]
    WebSearch(String),

    #[error("Web fetch error: {0}")]
    WebFetch(String),

    #[error("File I/O error: {0}")]
    FileIo(String),

    #[error("Workspace sandbox violation: attempted access outside sandbox root")]
    SandboxViolation,

    #[error("Token estimation error: {0}")]
    TokenEstimation(String),

    #[error("Compaction error: {0}")]
    Compaction(String),

    #[error("Diagnostics error: {0}")]
    Diagnostics(String),

    #[error("Permission denied: unauthorized sender")]
    PermissionDenied,

    #[error("Timeout: {0}")]
    Timeout(String),

    #[error("Generic error: {0}")]
    Generic(String),
}

/// Errors specific to tool execution.
#[derive(thiserror::Error, Debug)]
pub enum ToolError {
    #[error("Invalid input: {0}")]
    InvalidInput(String),

    #[error("Execution failed: {0}")]
    Execution(String),

    #[error("Rate limited")]
    RateLimited,
}

/// Errors specific to web search.
#[derive(thiserror::Error, Debug)]
pub enum WebSearchError {
    #[error("Search backend error: {0}")]
    Backend(String),

    #[error("Invalid query")]
    InvalidQuery,

    #[error("Rate limited")]
    RateLimited,
}

/// Errors specific to web fetching.
#[derive(thiserror::Error, Debug)]
pub enum WebFetchError {
    #[error("HTTP error: {0}")]
    Http(String),

    #[error("Invalid URL: {0}")]
    InvalidUrl(String),

    #[error("SSRF protection blocked request to private/localhost address")]
    SsrfBlocked,

    #[error("Response exceeded maximum size")]
    ResponseTooLarge,

    #[error("Too many redirects")]
    TooManyRedirects,

    #[error("Timeout")]
    Timeout,

    #[error("Rate limited")]
    RateLimited,
}

/// Errors specific to token estimation.
#[derive(thiserror::Error, Debug)]
pub enum TokenEstimationError {
    #[error("Provider does not support token estimation: {0}")]
    NotSupported(String),

    #[error("Estimation failed: {0}")]
    Failed(String),
}

/// Errors specific to compaction.
#[derive(thiserror::Error, Debug)]
pub enum CompactionError {
    #[error("Compaction model call failed: {0}")]
    ModelCall(String),

    #[error("No eligible history to compact")]
    NoHistory,

    #[error("Compaction already running for session")]
    AlreadyRunning,

    #[error("Storage error during compaction: {0}")]
    Storage(String),
}
