//! Agent run outcomes — what the agent loop returns to its caller.

use crate::llm::types::TokenEstimate;

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

/// Metadata about a completed run.
#[derive(Debug, Default)]
pub struct RunMetadata {
    /// Number of tool-loop iterations performed.
    pub iterations: u32,
    /// Total estimated tokens consumed.
    pub token_estimate: Option<TokenEstimate>,
}

/// Full result from the agent loop.
#[derive(Debug)]
pub struct AgentResult {
    pub outcome: AgentOutcome,
    pub metadata: RunMetadata,
}
