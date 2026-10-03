//! Compaction worker — produces structured summaries from old history.
//!
//! Phase 9 implementation.

use genai::chat::{ChatMessage, ChatRequest, MessageContent};
use sqlx::SqlitePool;
use tracing::{debug, warn};

use crate::context::budget::ContextBudget;
use crate::error::AgentError;
use crate::llm::LlmExecutor;

/// Worker that performs actual compaction via an LLM call.
///
/// Loads eligible history (older messages not covered by the latest summary),
/// combines it with the existing summary, calls the LLM to produce a new
/// structured summary, and persists it.
///
/// The `recent_turns_to_preserve` count (default 30) is respected: the N
/// most recent messages are excluded from compaction so they are never lost
/// when the summary boundary advances.
pub struct CompactionWorker {
    llm: std::sync::Arc<dyn LlmExecutor>,
    model: String,
    temperature: f32,
    recent_turns_to_preserve: usize,
}

impl CompactionWorker {
    /// Create a new CompactionWorker with an LLM executor.
    pub fn new(llm: std::sync::Arc<dyn LlmExecutor>, model: String, temperature: f32) -> Self {
        Self::new_with_preserve(llm, model, temperature, 30)
    }

    /// Create a new CompactionWorker with a custom `recent_turns_to_preserve` count.
    pub fn new_with_preserve(
        llm: std::sync::Arc<dyn LlmExecutor>,
        model: String,
        temperature: f32,
        recent_turns_to_preserve: usize,
    ) -> Self {
        Self {
            llm,
            model,
            temperature,
            recent_turns_to_preserve,
        }
    }

    /// Run compaction on a session, producing a structured summary.
    ///
    /// 1. Load the latest summary (if any)
    /// 2. Load all messages for the session
    /// 3. Identify messages older than the summary boundary
    /// 4. Build a compaction prompt with the existing summary + old messages
    /// 5. Call the LLM to produce a new structured summary
    /// 6. Persist the new summary
    pub async fn compact(
        &self,
        pool: &SqlitePool,
        session_id: &str,
        budget: &ContextBudget,
    ) -> Result<String, AgentError> {
        // Load the latest summary (if any)
        let latest_summary =
            crate::storage::summaries::get_latest_summary(pool, session_id).await?;

        // Load all messages for the session
        let all_messages = crate::storage::messages::list_messages(pool, session_id, None).await?;

        if all_messages.is_empty() {
            return Err(AgentError::Compaction("No messages to compact".into()));
        }

        let messages_to_compact = self.select_compactable(&all_messages, &latest_summary)?;

        if messages_to_compact.is_empty() {
            // Nothing to compact — all messages are within the recent window
            debug!(session_id = %session_id, "no messages eligible for compaction");
            return latest_summary
                .map(|s| s.summary_text)
                .ok_or_else(|| AgentError::Compaction("No summary exists to compact".into()));
        }

        // Build the compaction prompt
        let (prompt, count) =
            self.bound_compaction_prompt(&latest_summary, &messages_to_compact, budget)?;
        if count == 0 {
            return Err(AgentError::Compaction(
                "No complete history unit fits within the usable compaction input budget".into(),
            ));
        }
        let messages_to_compact = &messages_to_compact[..count];

        // Call the LLM to produce a new summary
        let new_summary_text = self
            .call_compaction_model(&*self.llm, &self.model, self.temperature, &prompt)
            .await?;

        // Determine the new boundary message ID (latest message being compacted)
        let new_boundary_id = messages_to_compact
            .last()
            .map(|m| m.id.clone())
            .unwrap_or_default();

        // Store the new summary first so a cleanup failure cannot leave the
        // session without any summary.
        let new_summary = crate::context::summaries::ContextSummary::new(
            session_id.to_string(),
            new_summary_text.clone(),
            new_boundary_id.clone(),
        );

        crate::storage::summaries::create_summary(pool, &new_summary).await?;

        // Then delete old summary if one existed. If cleanup fails, keep the
        // newly-created summary and rely on latest-summary ordering.
        if let Some(ref old_summary) = latest_summary
            && let Err(e) = self
                .delete_old_summary(pool, session_id, &old_summary.id)
                .await
        {
            warn!(
                old_summary_id = %old_summary.id,
                error = %e,
                "failed to clean up old summary after successful compaction"
            );
        }

        debug!(
            session_id = %session_id,
            new_boundary_id = %new_boundary_id,
            summary_length = new_summary_text.len(),
            "compaction completed and summary persisted"
        );

        Ok(new_summary_text)
    }

    /// Build the compaction prompt that would be generated for the session currently.
    pub async fn get_compaction_prompt(
        &self,
        pool: &SqlitePool,
        session_id: &str,
        budget: &ContextBudget,
    ) -> Result<String, AgentError> {
        let latest_summary =
            crate::storage::summaries::get_latest_summary(pool, session_id).await?;

        let all_messages = crate::storage::messages::list_messages(pool, session_id, None).await?;

        let compactable = self.select_compactable(&all_messages, &latest_summary)?;
        self.bound_compaction_prompt(&latest_summary, &compactable, budget)
            .map(|(prompt, _)| prompt)
    }

    fn select_compactable<'a>(
        &self,
        all_messages: &'a [crate::storage::messages::StoredMessage],
        summary: &Option<crate::storage::summaries::StoredSummary>,
    ) -> Result<Vec<&'a crate::storage::messages::StoredMessage>, AgentError> {
        let boundary = summary.as_ref().and_then(|summary| {
            all_messages
                .iter()
                .position(|m| m.id == summary.covers_through_message_id)
        });
        let recent = &all_messages[..boundary.unwrap_or(all_messages.len())];
        let chronological: Vec<_> = recent.iter().rev().collect();
        // Propagate decoding errors rather than shifting source-row indices or
        // advancing the boundary past history that was not summarized.
        let messages = chronological
            .iter()
            .map(|m| m.to_message())
            .collect::<Result<Vec<_>, _>>()?;
        let cutoff = crate::context::history::compaction_prefix_len(
            &messages,
            self.recent_turns_to_preserve,
        );
        Ok(chronological.into_iter().take(cutoff).collect())
    }

    /// Fit an oldest prefix of whole units. Never advance a summary boundary
    /// past omitted rows. Binary search avoids rebuilding a growing prompt for
    /// every row; existing per-message text truncation still applies.
    fn bound_compaction_prompt(
        &self,
        summary: &Option<crate::storage::summaries::StoredSummary>,
        candidates: &[&crate::storage::messages::StoredMessage],
        budget: &ContextBudget,
    ) -> Result<(String, usize), AgentError> {
        let base = self.build_compaction_prompt(summary, &[]);
        let fits = |prompt: &str| (prompt.len() / 4).max(1) <= budget.usable_input_budget();
        if !fits(&base) {
            return Err(AgentError::Compaction(
                "Summary and instructions exceed the usable compaction input budget".into(),
            ));
        }
        let decoded = candidates
            .iter()
            .map(|m| m.to_message())
            .collect::<Result<Vec<_>, _>>()?;
        let units = crate::context::history::history_units(&decoded);
        let mut low = 0;
        let mut high = units.len();
        while low < high {
            let middle = (low + high).div_ceil(2);
            let end = units[middle - 1].source.end;
            if fits(&self.build_compaction_prompt(summary, &candidates[..end])) {
                low = middle;
            } else {
                high = middle - 1;
            }
        }
        let count = if low == 0 {
            0
        } else {
            units[low - 1].source.end
        };
        Ok((
            self.build_compaction_prompt(summary, &candidates[..count]),
            count,
        ))
    }

    /// Build a compaction prompt for the LLM.
    pub fn build_compaction_prompt(
        &self,
        existing_summary: &Option<crate::storage::summaries::StoredSummary>,
        messages: &[&crate::storage::messages::StoredMessage],
    ) -> String {
        let mut prompt = String::from(
            "You are a conversation summarizer. Your job is to produce a structured \
            working summary of a conversation that will be used as context for future \
            AI assistant interactions.\n\n\
            The summary must preserve:\n\
            - User preferences and standing instructions\n\
            - Ongoing projects or threads\n\
            - Decisions already made\n\
            - Important facts introduced by the user\n\
            - Open loops (tasks or questions not yet resolved)\n\
            - Relevant tool outcomes or state changes\n\n\
            The summary should be concise but complete enough that a future AI assistant \
            can pick up the conversation without losing context.\n\n",
        );

        // Include existing summary as a starting point
        if let Some(summary) = existing_summary {
            prompt.push_str(&format!(
                "Existing summary (from older context):\n{}\n\n",
                summary.summary_text
            ));
        } else {
            prompt.push_str("No existing summary — produce a fresh summary from scratch.\n\n");
        }

        // Include the messages to compact
        prompt.push_str("Messages to incorporate into the new summary:\n\n");
        // Summarization always uses ordinary text, including useful tool
        // arguments/outcomes from valid and malformed legacy exchanges.
        let decoded: Vec<_> = messages
            .iter()
            .filter_map(|m| m.to_message().ok())
            .collect();
        for msg in crate::context::history::normalize_history(&decoded) {
            let role_str = match msg.role {
                genai::chat::ChatRole::System => "[SYSTEM]",
                genai::chat::ChatRole::User => "[USER]",
                genai::chat::ChatRole::Assistant => "[ASSISTANT]",
                genai::chat::ChatRole::Tool => "[TOOL RESULT]",
            };
            let content = crate::context::history::summary_text(&msg);
            // Truncate very long messages at character boundaries.
            let truncated = if content.len() > 2000 {
                format!("{}...", truncate_str(&content, 2000))
            } else {
                content
            };
            prompt.push_str(&format!("{}: {}\n\n", role_str, truncated));
        }

        prompt.push_str(
            "Produce a new structured summary following the format below:\n\n\
            # Conversation Working Summary\n\n\
            ## User preferences and standing instructions\n\
            - ...\n\n\
            ## Ongoing projects or threads\n\
            - ...\n\n\
            ## Decisions already made\n\
            - ...\n\n\
            ## Important facts introduced by the user\n\
            - ...\n\n\
            ## Open loops\n\
            - ...\n\n\
            ## Relevant tool outcomes or state changes\n\
            - ...\n\n\
            Write the actual summary content after each heading.",
        );

        prompt
    }

    /// Call the compaction model to produce a structured summary.
    async fn call_compaction_model(
        &self,
        llm: &dyn LlmExecutor,
        model: &str,
        temperature: f32,
        prompt: &str,
    ) -> Result<String, AgentError> {
        let messages = vec![ChatMessage::user(MessageContent::from_text(prompt))];
        let request = ChatRequest::new(messages);
        let options = genai::chat::ChatOptions::default().with_temperature(temperature as f64);

        let response = llm
            .complete(model, request, options)
            .await
            .map_err(|e| AgentError::Compaction(format!("Compaction model call failed: {e}")))?;

        let text = response
            .content
            .joined_texts()
            .ok_or_else(|| AgentError::Compaction("Compaction model returned no text".into()))?;

        if text.is_empty() {
            return Err(AgentError::Compaction(
                "Compaction model returned empty text".into(),
            ));
        }

        debug!(
            model = %model,
            text_length = text.len(),
            "compaction model produced summary"
        );

        Ok(text)
    }

    /// Delete an old summary from the database.
    async fn delete_old_summary(
        &self,
        pool: &SqlitePool,
        session_id: &str,
        summary_id: &str,
    ) -> Result<(), AgentError> {
        sqlx::query("DELETE FROM context_summaries WHERE id = ?1 AND chat_session_id = ?2")
            .bind(summary_id)
            .bind(session_id)
            .execute(pool)
            .await
            .map_err(|e| {
                warn!(
                    summary_id = %summary_id,
                    error = %e,
                    "failed to delete old summary"
                );
                AgentError::Compaction(format!("Failed to delete old summary: {e}"))
            })?;

        Ok(())
    }
}

/// Safely truncate a string to a maximum byte length, cutting at a character boundary.
/// The result is guaranteed to be <= max_bytes.
fn truncate_str(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    // Find the last character that starts entirely before max_bytes.
    s.char_indices()
        .take_while(|(i, _)| *i < max_bytes)
        .last()
        .map_or(&s[..0], |(i, _)| &s[..i])
}
