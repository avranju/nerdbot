//! Compaction worker — produces structured summaries from old history.
//!
//! Phase 9 implementation.

use genai::chat::{ChatMessage, ChatRequest, MessageContent};
use sqlx::SqlitePool;
use tracing::{debug, info, warn};

use crate::context::budget::ContextBudget;
use crate::error::AgentError;
use crate::llm::LlmExecutor;

/// Worker that performs actual compaction via an LLM call.
///
/// Loads eligible history (older messages not covered by the latest summary),
/// combines it with the existing summary, calls the LLM to produce a new
/// structured summary, and persists it.
pub struct CompactionWorker {
    llm: Option<std::sync::Arc<dyn LlmExecutor>>,
    model: String,
    temperature: f32,
}

impl CompactionWorker {
    /// Create a new CompactionWorker.
    pub fn new() -> Self {
        Self {
            llm: None,
            model: String::new(),
            temperature: 0.0,
        }
    }

    /// Create a new CompactionWorker with an LLM executor.
    pub fn with_llm(llm: std::sync::Arc<dyn LlmExecutor>, model: String, temperature: f32) -> Self {
        Self {
            llm: Some(llm),
            model,
            temperature,
        }
    }

    /// Get the LLM executor if available.
    fn llm_executor(&self) -> Option<&dyn LlmExecutor> {
        self.llm.as_deref()
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

        // Identify messages to compact. When a summary already exists, only
        // incorporate messages that arrived after the message covered by the
        // latest summary.
        let messages_to_compact: Vec<_> = if let Some(ref summary) = latest_summary {
            let boundary_created_at = all_messages
                .iter()
                .find(|m| m.id == summary.covers_through_message_id)
                .map(|m| m.created_at)
                .unwrap_or(summary.created_at);

            all_messages
                .iter()
                .filter(|m| m.created_at > boundary_created_at)
                .collect()
        } else {
            all_messages.iter().collect()
        };

        if messages_to_compact.is_empty() {
            // Nothing to compact — all messages are within the recent window
            debug!(session_id = %session_id, "no messages eligible for compaction");
            return latest_summary
                .map(|s| s.summary_text)
                .ok_or_else(|| AgentError::Compaction("No summary exists to compact".into()));
        }

        // Build the compaction prompt
        let prompt = self.build_compaction_prompt(&latest_summary, &messages_to_compact);

        // Call the LLM to produce a new summary
        let new_summary_text = if let Some(llm_ref) = self.llm_executor() {
            self.call_compaction_model(llm_ref, &self.model, self.temperature, &prompt)
                .await?
        } else {
            // Fallback: produce a basic summary without LLM
            self.fallback_compaction(&messages_to_compact)?
        };

        // Determine the new boundary message ID (latest message being compacted)
        let new_boundary_id = messages_to_compact
            .iter()
            .max_by_key(|m| m.created_at)
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

        info!(
            session_id = %session_id,
            new_boundary_id = %new_boundary_id,
            summary_length = new_summary_text.len(),
            "compaction completed and summary persisted"
        );

        Ok(new_summary_text)
    }

    /// Build a compaction prompt for the LLM.
    fn build_compaction_prompt(
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
        for msg in messages {
            let role_str = match msg.role.as_str() {
                Some("system") => "[SYSTEM]",
                Some("user") => "[USER]",
                Some("assistant") => "[ASSISTANT]",
                Some("tool") => "[TOOL RESULT]",
                _ => "[UNKNOWN]",
            };
            let content = if msg.content.is_empty() {
                msg.structured_content_json
                    .as_ref()
                    .map(|v| v.to_string())
                    .unwrap_or_default()
            } else {
                msg.content.clone()
            };
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

    /// Fallback compaction when no LLM is available.
    ///
    /// Produces a basic summary by concatenating key information from messages.
    fn fallback_compaction(
        &self,
        messages: &[&crate::storage::messages::StoredMessage],
    ) -> Result<String, AgentError> {
        let mut summary = String::from("# Conversation Working Summary\n\n");

        // Collect user messages and assistant responses
        let mut user_msgs = Vec::new();
        let mut assistant_msgs = Vec::new();

        for msg in messages {
            match msg.role.as_str() {
                Some("user") => {
                    let content = if msg.content.is_empty() {
                        msg.structured_content_json
                            .as_ref()
                            .map(|v| v.to_string())
                            .unwrap_or_default()
                    } else {
                        msg.content.clone()
                    };
                    user_msgs.push(content);
                }
                Some("assistant") => {
                    let content = if msg.content.is_empty() {
                        msg.structured_content_json
                            .as_ref()
                            .map(|v| v.to_string())
                            .unwrap_or_default()
                    } else {
                        msg.content.clone()
                    };
                    assistant_msgs.push(content);
                }
                _ => {}
            }
        }

        if user_msgs.is_empty() && assistant_msgs.is_empty() {
            return Err(AgentError::Compaction(
                "No user or assistant messages to summarize".into(),
            ));
        }

        summary.push_str("## User preferences and standing instructions\n");
        summary.push_str("- No specific preferences identified in compacted history.\n\n");

        summary.push_str("## Ongoing projects or threads\n");
        if user_msgs.len() <= 5 {
            for (i, msg) in user_msgs.iter().enumerate() {
                let truncated = if msg.len() > 200 {
                    format!("{}...", truncate_str(msg, 200))
                } else {
                    msg.clone()
                };
                summary.push_str(&format!("- Message {}: {}\n", i + 1, truncated));
            }
        } else {
            summary.push_str(&format!(
                "- {} user messages were compacted (too many to list individually).\n",
                user_msgs.len()
            ));
        }
        summary.push('\n');

        summary.push_str("## Decisions already made\n");
        summary.push_str("- No explicit decisions identified.\n\n");

        summary.push_str("## Important facts introduced by the user\n");
        summary.push_str("- Information from compacted conversation turns.\n\n");

        summary.push_str("## Open loops\n");
        summary.push_str("- No explicit open loops identified.\n\n");

        summary.push_str("## Relevant tool outcomes or state changes\n");
        summary.push_str("- Tool call results from compacted conversation turns.\n");

        info!(
            summary_length = summary.len(),
            message_count = messages.len(),
            "fallback compaction produced basic summary"
        );

        Ok(summary)
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

impl Default for CompactionWorker {
    fn default() -> Self {
        Self::new()
    }
}
