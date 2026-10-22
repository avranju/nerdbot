//! Agent run outcomes — what the agent loop returns to its caller.

use genai::chat::Usage;

/// The result of a completed agent run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentOutcome {
    /// Final text the agent produced as an answer.
    FinalText(String),
    /// The agent completed without producing text (e.g. silent job).
    Silent,
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
    /// Known cache-read subtotal, or None if no call reported a valid count.
    /// Some(0) means an explicit zero survived provider normalization.
    /// This may cover only part of the run; see cached_input_reported_calls.
    pub cached_input_tokens: Option<usize>,
    /// Calls reporting nonnegative cached-input counts, out of RunMetadata::iterations.
    pub cached_input_reported_calls: u32,
    /// Known cache-write subtotal, tracked independently from cache reads.
    /// None means unavailable; partial coverage is possible.
    pub cache_creation_input_tokens: Option<usize>,
    /// Calls reporting nonnegative cache-write counts, out of RunMetadata::iterations.
    pub cache_creation_reported_calls: u32,
}

impl RunTokenUsage {
    pub(crate) fn from_call(usage: &Usage) -> Self {
        let input_tokens = usage.prompt_tokens.unwrap_or(0).max(0) as usize;
        let output_tokens = usage.completion_tokens.unwrap_or(0).max(0) as usize;
        let details = usage.prompt_tokens_details.as_ref();
        // genai can normalize explicit zeros to None. Do not infer cache misses
        // from missing values, empty detail objects, or malformed negative counts.
        let cached_input_tokens = details
            .and_then(|d| d.cached_tokens)
            .and_then(|n| usize::try_from(n).ok());
        let cache_creation_input_tokens = details
            .and_then(|d| d.cache_creation_tokens)
            .and_then(|n| usize::try_from(n).ok());

        Self {
            input_tokens,
            output_tokens,
            total_tokens: input_tokens + output_tokens,
            cached_input_tokens,
            cached_input_reported_calls: u32::from(cached_input_tokens.is_some()),
            cache_creation_input_tokens,
            cache_creation_reported_calls: u32::from(cache_creation_input_tokens.is_some()),
        }
    }

    pub(crate) fn accumulate(&mut self, call: &Self) {
        self.input_tokens += call.input_tokens;
        self.output_tokens += call.output_tokens;
        self.total_tokens += call.total_tokens;
        if let Some(tokens) = call.cached_input_tokens {
            self.cached_input_tokens = Some(self.cached_input_tokens.unwrap_or(0) + tokens);
        }
        self.cached_input_reported_calls += call.cached_input_reported_calls;
        if let Some(tokens) = call.cache_creation_input_tokens {
            self.cache_creation_input_tokens =
                Some(self.cache_creation_input_tokens.unwrap_or(0) + tokens);
        }
        self.cache_creation_reported_calls += call.cache_creation_reported_calls;
    }
}

/// Metadata about a completed run.
#[derive(Debug, Default)]
pub struct RunMetadata {
    /// Number of tool-loop iterations performed.
    pub iterations: u32,
    /// Token usage across all LLM calls.
    pub token_usage: RunTokenUsage,
    /// Whether this run successfully sent at least one Telegram message via send_user_message.
    pub sent_user_message: bool,
}

/// Full result from the agent loop.
#[derive(Debug)]
pub struct AgentResult {
    pub outcome: AgentOutcome,
    pub metadata: RunMetadata,
}
