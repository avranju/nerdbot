//! Context manager — assembles bounded model input from summaries and recent messages.
//!
//! Phase 9 implementation.
//!
//! Also enforces:
//! - `recent_turns_to_preserve` — the N most recent turns are preferred during bounding
//! - Binary payload awareness — binary ContentParts are excluded from token estimation
//!   to prevent them from silently bypassing bounds

use genai::chat::{ChatMessage, ContentPart, MessageContent};
use sqlx::SqlitePool;
use tracing::debug;

use crate::agent::system_prompt::current_datetime_in_timezone;
use crate::config::ContextConfig;
use crate::context::budget::ContextBudget;
use crate::storage::messages::StoredMessage;
use crate::storage::summaries::StoredSummary;

/// Manages context assembly for model requests.
///
/// Assembles a bounded set of messages from:
/// 1. Personality / system prompt
/// 2. Latest rolling summary of older context
/// 3. Recent raw messages after the summary boundary (bounded by budget)
/// 4. Current user message (with optional attachment content parts,
///    and current date/time appended as trailing text context)
///
/// Also enforces `recent_turns_to_preserve`: the N most recent turns are
/// preferred during bounding, ensuring the latest conversation context
/// is retained when it fits within the model input budget.
pub struct ContextManager {
    pool: SqlitePool,
    budget: ContextBudget,
    recent_turns_to_preserve: usize,
}

impl ContextManager {
    /// Create a new ContextManager.
    pub fn new(pool: SqlitePool, budget: ContextBudget) -> Self {
        Self::new_with_preserve(pool, budget, 30)
    }

    /// Create a new ContextManager with a custom `recent_turns_to_preserve` count.
    pub fn new_with_preserve(
        pool: SqlitePool,
        budget: ContextBudget,
        recent_turns_to_preserve: usize,
    ) -> Self {
        Self {
            pool,
            budget,
            recent_turns_to_preserve,
        }
    }

    /// Create a ContextManager from config.
    pub fn from_config(pool: SqlitePool, budget: ContextBudget, config: &ContextConfig) -> Self {
        Self::new_with_preserve(pool, budget, config.recent_turns_to_preserve)
    }

    /// Assemble a bounded set of messages for the model request.
    ///
    /// Loads the latest summary and recent messages from storage for the
    /// given session, then builds a message list:
    /// - System message: personality (if provided)
    /// - System message: summary (if available, covering older context)
    /// - Recent raw messages (bounded by budget, preserving recent turns)
    /// - Current user message (with optional attachment content parts,
    ///   and current date/time appended as context)
    ///
    /// The `timezone` parameter is used for the datetime appended to the
    /// current user message. This avoids introducing a system message
    /// after non-system messages, which llama.cpp's Jinja templates reject.
    pub async fn assemble_messages(
        &self,
        session_id: &str,
        personality: &str,
        current_user_message: ChatMessage,
        timezone: &str,
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
        let (bounded, _preserve_tokens) = self.bound_messages(recent);

        // 4. Append recent messages.
        // The bounded function already enforces the budget for recent messages.
        messages.extend(bounded);

        // 5. Append the current user message with the current date/time
        // appended as context. This avoids introducing a system message
        // after non-system messages, which llama.cpp's Jinja templates reject.
        messages.push(append_current_datetime_to_user_message(
            current_user_message,
            timezone,
        ));

        let recent_count = messages.len();
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
    ///
    /// Binary ContentParts are excluded from token estimation since they
    /// are encoded as base64 and don't map linearly to tokens.
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
                            // Binary payloads are base64-encoded and don't map
                            // linearly to tokens; exclude from estimation.
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
    ///
    /// The N most recent turns (where N = `recent_turns_to_preserve`) are
    /// preferred, but still trimmed if they alone exceed the input budget.
    ///
    /// Returns `(bounded, preserve_tokens)` where `bounded` are the messages
    /// that fit within the budget and `preserve_tokens` is the total token
    /// count of preserved messages (for budget accounting in `assemble_messages`).
    fn bound_messages(&self, messages: Vec<StoredMessage>) -> (Vec<ChatMessage>, usize) {
        let budget = self.budget.usable_input_budget();
        let preserve = self.recent_turns_to_preserve;

        // list_messages returns DESC (newest first).
        // The first `preserve` entries are the most recent — preserve them.
        let preserve_count = messages.len().min(preserve);
        let (to_preserve, to_bound) = messages.split_at(preserve_count);

        // Keep as much of the preserve window as fits, preferring the newest
        // messages. A large preserve window must not defeat the hard bound.
        let mut preserve_tokens = 0usize;
        let mut preserve_msgs: Vec<ChatMessage> = to_preserve
            .iter()
            .filter_map(|m| {
                let msg = m.to_message().ok()?;
                let tokens = m
                    .token_estimate
                    .map(|t| t as usize)
                    .unwrap_or_else(|| estimate_tokens_for_message(&msg));
                if preserve_tokens + tokens > budget {
                    return None;
                }
                preserve_tokens += tokens;
                Some(msg)
            })
            .collect();
        preserve_msgs.reverse();

        // Bound the non-preserved messages using the remaining budget.
        // to_bound is older messages in DESC order (newest first from list_messages).
        // We want to keep the most recent ones within the remaining budget.
        let remaining_budget = budget.saturating_sub(preserve_tokens);
        let mut bounded: Vec<ChatMessage> = to_bound
            .iter()
            .rev() // reverse to get chronological (oldest first)
            .filter_map(|m| {
                let stored_tokens = m.token_estimate.map(|t| t as usize);
                let msg = m.to_message().ok()?;
                let msg_tokens = stored_tokens.unwrap_or_else(|| estimate_tokens_for_message(&msg));
                Some((msg, msg_tokens))
            })
            .rev() // reverse again to get newest-first
            .filter_map(|(msg, tokens)| {
                if tokens > remaining_budget {
                    return None;
                }
                Some((msg, tokens))
            })
            .scan(0usize, |accum, (msg, tokens)| {
                if *accum + tokens > remaining_budget {
                    None
                } else {
                    *accum += tokens;
                    Some(msg)
                }
            })
            .collect();

        // Reverse bounded to chronological order (oldest first).
        bounded.reverse();
        // Combine: bounded (older, chronological) + preserved (most recent, chronological).
        let mut result = bounded;
        result.extend(preserve_msgs);

        debug!(
            message_count = result.len(),
            preserve_tokens,
            preserve_window = preserve_count,
            "bounded messages with preserve window"
        );

        (result, preserve_tokens)
    }
}

/// Append the current date/time to a user message without flattening content parts.
///
/// Rich Telegram turns may include binary image/PDF parts or extracted document
/// text parts. Preserving the existing parts keeps those payloads available to
/// the LLM while still placing the volatile datetime at the tail of the prompt.
pub fn append_current_datetime_to_user_message(
    current_user_message: ChatMessage,
    timezone: &str,
) -> ChatMessage {
    let datetime_text = current_datetime_in_timezone(timezone);
    let mut parts = current_user_message.content.parts().clone();
    parts.push(ContentPart::Text(format!(
        "\n\n[Current date/time: {datetime_text}]"
    )));
    ChatMessage::user(MessageContent::from_parts(parts))
}

/// Estimate token count from a ChatMessage, accounting for binary parts.
pub(crate) fn estimate_tokens_for_message(msg: &ChatMessage) -> usize {
    let total_chars: usize = msg
        .content
        .parts()
        .iter()
        .filter_map(|p| match p {
            ContentPart::Text(t) => Some(t.len()),
            ContentPart::ToolCall(tc) => Some(tc.fn_name.len() + tc.fn_arguments.to_string().len()),
            ContentPart::ToolResponse(tr) => Some(tr.content.len()),
            // Binary payloads excluded — they're base64-encoded and don't
            // map linearly to tokens.
            ContentPart::Binary(_) => None,
            ContentPart::ThoughtSignature(_) => None,
            ContentPart::ReasoningContent(_) => None,
            ContentPart::Custom(_) => None,
        })
        .sum();

    (total_chars / 4).max(1)
}
