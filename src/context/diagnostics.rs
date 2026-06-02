//! Shared context and compaction-pressure diagnostics.

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::context::budget::ContextBudget;
use crate::error::AgentError;
use crate::storage::messages::StoredMessage;

/// Metadata about the latest persisted context summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SummaryDiagnostics {
    pub id: String,
    pub covers_through_message_id: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Estimated context and compaction pressure for one chat session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextDiagnosticsSnapshot {
    pub session_id: String,
    pub raw_message_count: usize,
    pub preserved_raw_message_count: usize,
    pub estimated_uncompacted_tokens: usize,
    pub usable_input_budget: usize,
    pub soft_threshold_tokens: usize,
    pub hard_threshold_tokens: usize,
    pub remaining_before_compaction_tokens: usize,
    pub remaining_before_hard_bound_tokens: usize,
    pub pressure: f32,
    pub values_are_estimated: bool,
    pub latest_summary: Option<SummaryDiagnostics>,
}

impl ContextDiagnosticsSnapshot {
    /// Calculate a snapshot from persisted session history.
    pub async fn calculate(
        pool: &SqlitePool,
        session_id: &str,
        budget: &ContextBudget,
        recent_turns_to_preserve: usize,
    ) -> Result<Self, AgentError> {
        let latest_summary =
            crate::storage::summaries::get_latest_summary(pool, session_id).await?;
        let all_messages = crate::storage::messages::list_messages(pool, session_id, None).await?;
        let raw_messages = messages_after_summary(&all_messages, latest_summary.as_ref());
        let estimated_uncompacted_tokens = raw_messages
            .iter()
            .map(|message| estimate_stored_message_tokens(message))
            .sum();
        let usable_input_budget = budget.usable_input_budget();
        let soft_threshold_tokens = budget.soft_threshold_tokens();
        let hard_threshold_tokens = budget.hard_threshold_tokens();
        let pressure = if usable_input_budget == 0 {
            0.0
        } else {
            estimated_uncompacted_tokens as f32 / usable_input_budget as f32
        };

        Ok(Self {
            session_id: session_id.to_string(),
            raw_message_count: raw_messages.len(),
            preserved_raw_message_count: raw_messages.len().min(recent_turns_to_preserve),
            estimated_uncompacted_tokens,
            usable_input_budget,
            soft_threshold_tokens,
            hard_threshold_tokens,
            remaining_before_compaction_tokens: soft_threshold_tokens
                .saturating_sub(estimated_uncompacted_tokens),
            remaining_before_hard_bound_tokens: hard_threshold_tokens
                .saturating_sub(estimated_uncompacted_tokens),
            pressure,
            values_are_estimated: true,
            latest_summary: latest_summary.map(|summary| SummaryDiagnostics {
                id: summary.id,
                covers_through_message_id: summary.covers_through_message_id,
                created_at: summary.created_at,
            }),
        })
    }

    /// Whether uncompacted raw history has reached the soft compaction threshold.
    pub fn should_compact(&self) -> bool {
        self.estimated_uncompacted_tokens >= self.soft_threshold_tokens
    }
}

fn messages_after_summary<'a>(
    messages: &'a [StoredMessage],
    summary: Option<&crate::storage::summaries::StoredSummary>,
) -> Vec<&'a StoredMessage> {
    let boundary_created_at = summary.and_then(|summary| {
        messages
            .iter()
            .find(|message| message.id == summary.covers_through_message_id)
            .map(|message| message.created_at)
    });

    match boundary_created_at {
        Some(boundary) => messages
            .iter()
            .filter(|message| message.created_at > boundary)
            .collect(),
        None => messages.iter().collect(),
    }
}

fn estimate_stored_message_tokens(message: &StoredMessage) -> usize {
    message
        .token_estimate
        .map(|tokens| tokens as usize)
        .or_else(|| {
            message
                .to_message()
                .ok()
                .map(|message| super::manager::estimate_tokens_for_message(&message))
        })
        .unwrap_or(0)
}
