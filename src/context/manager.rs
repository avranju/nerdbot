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
        self.assemble_prepared_messages(
            session_id,
            personality,
            append_current_datetime_to_user_message(current_user_message, timezone),
        )
        .await
    }

    /// Assemble context with a current message whose datetime is already prepared.
    ///
    /// Callers that persist a safe representation must enrich both messages with
    /// the same datetime before calling this method. History is replayed unchanged,
    /// and budgeting includes the prepared current message's datetime text.
    pub async fn assemble_prepared_messages(
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

        // Reserve fixed prompt costs before selecting history. Rich current
        // content is appended once; oversized fixed prompts fail before the LLM.
        let fixed_tokens = messages
            .iter()
            .map(estimate_tokens_for_message)
            .sum::<usize>()
            + estimate_tokens_for_message(&current_user_message);
        let remaining = self
            .usable_input_budget()
            .checked_sub(fixed_tokens)
            .ok_or_else(|| {
                crate::error::AgentError::Context(
                    "Current prompt and system context exceed the usable input budget".into(),
                )
            })?;
        let recent = self.load_recent_messages(session_id, &summary_opt).await?;
        messages.extend(self.bound_messages(recent, remaining)?);
        messages.push(current_user_message);

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
        messages.iter().map(estimate_tokens_for_message).sum()
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

        // Use row order and the boundary ID, rather than timestamps, which
        // can tie. An old boundary inside an exchange is safe after normalization.
        let boundary = summary.as_ref().and_then(|summary| {
            stored
                .iter()
                .position(|message| message.id == summary.covers_through_message_id)
        });
        let messages = match boundary {
            Some(index) => stored.into_iter().take(index).collect(),
            None => stored,
        };

        Ok(messages)
    }

    /// Select recent coherent units, never slicing an assistant call batch.
    /// Recent rows are preferred regardless of the preserve-window cutoff; a
    /// batch crossing that cutoff is still considered as one indivisible unit.
    fn bound_messages(
        &self,
        messages: Vec<StoredMessage>,
        budget: usize,
    ) -> Result<Vec<ChatMessage>, crate::error::AgentError> {
        let chronological: Vec<_> = messages
            .iter()
            .rev()
            .map(|message| message.to_message())
            .collect::<Result<Vec<_>, _>>()?;
        let units = crate::context::history::history_units(&chronological);
        let mut tokens = 0;
        let mut retained = Vec::new();
        for unit in units.into_iter().rev() {
            // Re-estimate normalized content: old stored estimates may omit
            // tool payloads or the new historical labels.
            let cost = unit
                .messages
                .iter()
                .zip(unit.source.clone())
                .map(|(message, index)| {
                    let estimate = messages[messages.len() - 1 - index]
                        .token_estimate
                        .and_then(|tokens| usize::try_from(tokens).ok())
                        .unwrap_or(0);
                    estimate.max(estimate_tokens_for_message(message))
                })
                .sum::<usize>();
            if tokens + cost <= budget {
                tokens += cost;
                retained.push(unit.messages);
            }
        }
        let messages = retained.into_iter().rev().flatten().collect::<Vec<_>>();
        debug!(
            message_count = messages.len(),
            tokens,
            preserve_window = self.recent_turns_to_preserve,
            "bounded coherent history units"
        );
        Ok(messages)
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
    append_datetime_to_user_message(current_user_message, &datetime_text)
}

/// Append a supplied datetime value, allowing rich and safe messages to share
/// the exact same enrichment and tests to supply a deterministic timestamp.
pub fn append_datetime_to_user_message(
    current_user_message: ChatMessage,
    datetime_text: &str,
) -> ChatMessage {
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
            ContentPart::ToolCall(tc) => Some(
                tc.call_id.len()
                    + tc.fn_name.len()
                    + tc.fn_arguments.to_string().len()
                    + tc.thought_signatures
                        .as_ref()
                        .map(|s| s.iter().map(String::len).sum::<usize>())
                        .unwrap_or(0),
            ),
            ContentPart::ToolResponse(tr) => Some(tr.call_id.len() + tr.content.len()),
            // Binary payloads excluded — they're base64-encoded and don't
            // map linearly to tokens.
            ContentPart::Binary(_) => None,
            ContentPart::ThoughtSignature(text) | ContentPart::ReasoningContent(text) => {
                Some(text.len())
            }
            ContentPart::Custom(value) => {
                Some(serde_json::to_string(value).map(|v| v.len()).unwrap_or(0))
            }
        })
        .sum();

    (total_chars / 4).max(1)
}
