//! Token budgeting — manages context window allocation.
//!
//! Implementations come in Phase 9.

use serde::{Deserialize, Serialize};

/// Configuration for context budgeting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextBudget {
    /// Total context window size in tokens.
    pub context_window_tokens: usize,
    /// Tokens reserved for model output.
    pub reserved_output_tokens: usize,
    /// Tokens reserved for tool-loop headroom.
    pub reserved_tool_loop_tokens: usize,
    /// Soft compaction threshold (fraction of usable budget).
    pub soft_compaction_threshold: f32,
    /// Hard context threshold (fraction of usable budget).
    pub hard_context_threshold: f32,
}

impl ContextBudget {
    /// Calculate the usable input budget.
    pub fn usable_input_budget(&self) -> usize {
        self.context_window_tokens
            - self.reserved_output_tokens
            - self.reserved_tool_loop_tokens
    }

    /// Get the soft compaction threshold in tokens.
    pub fn soft_threshold_tokens(&self) -> usize {
        (self.usable_input_budget() as f32 * self.soft_compaction_threshold) as usize
    }

    /// Get the hard threshold in tokens.
    pub fn hard_threshold_tokens(&self) -> usize {
        (self.usable_input_budget() as f32 * self.hard_context_threshold) as usize
    }
}

impl Default for ContextBudget {
    fn default() -> Self {
        Self {
            context_window_tokens: 128_000,
            reserved_output_tokens: 4_096,
            reserved_tool_loop_tokens: 8_192,
            soft_compaction_threshold: 0.60,
            hard_context_threshold: 0.85,
        }
    }
}
