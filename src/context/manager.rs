//! Context manager — assembles bounded model input from summaries and recent messages.
//!
//! Phase 9 implementation.

use genai::chat::{ChatMessage, ContentPart, MessageContent};
use sqlx::SqlitePool;
use tracing::debug;

use crate::context::budget::ContextBudget;
use crate::storage::messages::StoredMessage;
use crate::storage::summaries::StoredSummary;

/// Manages context assembly for model requests.
///
/// Assembles a bounded set of messages from:
/// 1. Personality / system prompt
/// 2. Latest rolling summary of older context
/// 3. Recent raw messages after the summary boundary
/// 4. Current user message
pub struct ContextManager {
    pool: SqlitePool,
    budget: ContextBudget,
}

impl ContextManager {
    /// Create a new ContextManager.
    pub fn new(pool: SqlitePool, budget: ContextBudget) -> Self {
        Self { pool, budget }
    }

    /// Assemble a bounded set of messages for the model request.
    ///
    /// Loads the latest summary and recent messages from storage for the
    /// given session, then builds a message list:
    /// - System message: personality (if provided)
    /// - System message: summary (if available, covering older context)
    /// - Recent raw messages (bounded by budget)
    /// - Current user message
    pub async fn assemble_messages(
        &self,
        session_id: &str,
        personality: &str,
        current_user_message: ChatMessage,
    ) -> Result<Vec<ChatMessage>, crate::error::AgentError> {
        let mut messages = Vec::new();

        // 1. Personality as system message
        if !personality.is_empty() {
            messages.push(ChatMessage::system(MessageContent::from_text(personality)));
        }

        // 2. Load latest summary for this session
        let summary_opt = self.load_latest_summary(session_id).await?;
        if let Some(summary) = &summary_opt {
            messages.push(ChatMessage::system(MessageContent::from_text(format!(
                "Conversation Working Summary (covers older context):\n{}",
                summary.summary_text
            ))));
            debug!(
                summary_id = %summary.id,
                covers_through = %summary.covers_through_message_id,
                "loaded context summary"
            );
        }

        // 3. Load recent raw messages (after summary boundary)
        let recent = self.load_recent_messages(session_id, &summary_opt).await?;
        let bounded = self.bound_messages(recent);

        let recent_count = bounded.len();
        // 4. Append recent messages and current user message
        messages.extend(bounded);
        messages.push(current_user_message);

        debug!(
            message_count = messages.len(),
            has_summary = summary_opt.is_some(),
            recent_count,
            "assembled bounded context"
        );

        Ok(messages)
    }

    /// Estimate the total token count of the assembled messages list.
    ///
    /// Uses a character-based heuristic (4 chars ≈ 1 token) since we don't
    /// have per-message token counts stored. This is sufficient for threshold
    /// comparisons during compaction pressure checks.
    pub fn estimate_tokens(&self, messages: &[ChatMessage]) -> usize {
        let total_chars: usize = messages
            .iter()
            .map(|m| {
                // Join text parts for character counting
                m.content.joined_texts().map(|t| t.len()).unwrap_or(0)
                    + m.content
                        .parts()
                        .iter()
                        .filter_map(|p| match p {
                            ContentPart::Text(t) => Some(t.len()),
                            ContentPart::ToolCall(tc) => {
                                Some(tc.fn_name.len() + tc.fn_arguments.to_string().len())
                            }
                            ContentPart::ToolResponse(tr) => Some(tr.content.len()),
                            ContentPart::Binary(_) => None,
                            ContentPart::ThoughtSignature(_) => None,
                            ContentPart::ReasoningContent(_) => None,
                            ContentPart::Custom(_) => None,
                        })
                        .sum::<usize>()
            })
            .sum();

        // Heuristic: ~4 characters per token for English text
        (total_chars / 4).max(1)
    }

    /// Get the usable input budget in tokens.
    pub fn usable_input_budget(&self) -> usize {
        self.budget.usable_input_budget()
    }

    /// Get the soft threshold in tokens.
    pub fn soft_threshold_tokens(&self) -> usize {
        self.budget.soft_threshold_tokens()
    }

    /// Get the hard threshold in tokens.
    pub fn hard_threshold_tokens(&self) -> usize {
        self.budget.hard_threshold_tokens()
    }

    /// Load the latest context summary for a session.
    async fn load_latest_summary(
        &self,
        session_id: &str,
    ) -> Result<Option<StoredSummary>, crate::error::AgentError> {
        let summary = crate::storage::summaries::get_latest_summary(&self.pool, session_id).await?;
        Ok(summary)
    }

    /// Load recent messages after the summary boundary.
    ///
    /// If a summary exists, only loads messages created after the summary's
    /// covered boundary message. Otherwise loads the most recent messages.
    async fn load_recent_messages(
        &self,
        session_id: &str,
        summary: &Option<StoredSummary>,
    ) -> Result<Vec<StoredMessage>, crate::error::AgentError> {
        let stored = crate::storage::messages::list_messages(&self.pool, session_id, None).await?;

        let boundary_created_at = summary.as_ref().and_then(|s| {
            // Find the boundary message's created_at timestamp.
            stored
                .iter()
                .find(|m| m.id == s.covers_through_message_id)
                .map(|m| m.created_at)
        });

        let messages: Vec<StoredMessage> = match boundary_created_at {
            Some(boundary_ts) => stored
                .into_iter()
                .filter(|m| m.created_at > boundary_ts)
                .collect(),
            None => stored,
        };

        Ok(messages)
    }

    /// Bound a list of messages to fit within the usable input budget.
    ///
    /// `list_messages` returns newest-first; this keeps the most recent
    /// messages that fit within the budget and returns them chronological.
    fn bound_messages(&self, messages: Vec<StoredMessage>) -> Vec<ChatMessage> {
        let budget = self.budget.usable_input_budget();

        // list_messages returns DESC (newest first), reverse to get chronological.
        let chron_messages: Vec<StoredMessage> = messages.into_iter().rev().collect();

        // Take from the end (most recent) until the budget is exhausted.
        let mut result = Vec::new();
        let mut total_tokens: usize = 0;

        for msg in chron_messages.into_iter().rev() {
            let msg_tokens = msg.token_estimate.map(|t| t as usize).unwrap_or_else(|| {
                estimate_tokens_from_content(&msg.content, &msg.structured_content_json)
            });

            if total_tokens + msg_tokens > budget {
                debug!(
                    msg_id = %msg.id,
                    msg_tokens,
                    budget,
                    "message exceeds budget, stopping"
                );
                break;
            }

            total_tokens += msg_tokens;
            if let Ok(chat_msg) = msg.to_message() {
                result.push(chat_msg);
            }
        }

        // We selected newest-to-oldest, so reverse back to chronological order.
        result.reverse();
        debug!(
            message_count = result.len(),
            total_tokens, budget, "bounded messages"
        );

        result
    }
}

/// Estimate token count from message content using character heuristic.
fn estimate_tokens_from_content(content: &str, structured: &Option<serde_json::Value>) -> usize {
    let total_chars = content.len()
        + structured
            .as_ref()
            .map(|v| v.to_string().len())
            .unwrap_or(0);
    (total_chars / 4).max(1)
}
