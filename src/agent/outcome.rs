//! Agent run outcomes — what the agent loop returns to its caller.

/// The result of a completed agent run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentOutcome {
    /// Final text the agent produced as an answer.
    FinalText(String),
    /// The agent completed without producing text (e.g. silent job).
    Silent,
    /// The agent run was cancelled.
    Cancelled,
}

/// Token usage accumulated across all LLM calls in a run.
#[derive(Debug, Default, Clone)]
pub struct RunTokenUsage {
    /// Total input tokens consumed.
    pub input_tokens: usize,
    /// Total output tokens produced.
    pub output_tokens: usize,
    /// Total tokens (input + output).
    pub total_tokens: usize,
}

/// Metadata about a completed run.
#[derive(Debug, Default)]
pub struct RunMetadata {
    /// Number of tool-loop iterations performed.
    pub iterations: u32,
    /// Token usage across all LLM calls.
    pub token_usage: RunTokenUsage,
}

/// Full result from the agent loop.
#[derive(Debug)]
pub struct AgentResult {
    pub outcome: AgentOutcome,
    pub metadata: RunMetadata,
}
