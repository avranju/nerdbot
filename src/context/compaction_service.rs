//! Compaction service — monitors session pressure and triggers async compaction.
//!
//! Phase 9 implementation.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};

use crate::context::budget::ContextBudget;
use crate::context::compaction_worker::CompactionWorker;
use crate::context::diagnostics::ContextDiagnosticsSnapshot;
use crate::error::AgentError;

/// State of compaction for a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CompactionState {
    /// No compaction is happening.
    Idle,
    /// Compaction is running, targeting a specific boundary.
    Running { target_through_message_id: String },
    /// Compaction is running but more messages arrived (needs another pass).
    RunningAndDirty { target_through_message_id: String },
}

/// Manages background compaction for all sessions.
///
/// Monitors session pressure and triggers asynchronous compaction when
/// the soft threshold is crossed. Prevents concurrent compaction jobs
/// for the same session.
pub struct CompactionService {
    pool: SqlitePool,
    worker: Arc<CompactionWorker>,
    budget: ContextBudget,
    recent_turns_to_preserve: usize,
    state: Arc<Mutex<HashMap<String, CompactionState>>>,
}

impl CompactionService {
    /// Create a new CompactionService.
    pub fn new(
        pool: SqlitePool,
        worker: Arc<CompactionWorker>,
        budget: ContextBudget,
        recent_turns_to_preserve: usize,
    ) -> Self {
        Self {
            pool,
            worker,
            budget,
            recent_turns_to_preserve,
            state: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Check if a session needs compaction and enqueue if so.
    ///
    /// Compares estimated uncompacted raw-history tokens against the soft threshold.
    /// If compaction is already running for the session, marks it dirty
    /// so a follow-up compaction will be triggered after the current one.
    pub async fn check_session(&self, session_id: &str) -> Result<(), AgentError> {
        // Load latest summary to understand coverage
        let latest_summary =
            crate::storage::summaries::get_latest_summary(&self.pool, session_id).await?;
        let snapshot = self.diagnostics_snapshot(session_id).await?;

        debug!(
            session_id = %session_id,
            raw_message_count = snapshot.raw_message_count,
            estimated_tokens = snapshot.estimated_uncompacted_tokens,
            usable_budget = snapshot.usable_input_budget,
            pressure = snapshot.pressure,
            soft_threshold_tokens = snapshot.soft_threshold_tokens,
            "compaction pressure check"
        );

        if snapshot.should_compact() {
            info!(
                session_id = %session_id,
                pressure = snapshot.pressure,
                "session pressure exceeds soft threshold, triggering compaction"
            );
            self.trigger_compaction(session_id, &latest_summary).await;
        } else {
            debug!(
                session_id = %session_id,
                pressure = snapshot.pressure,
                "session pressure below soft threshold"
            );
        }

        Ok(())
    }

    /// Get the compaction state for a session.
    pub async fn get_state(&self, session_id: &str) -> CompactionState {
        let state = self.state.lock().await;
        state
            .get(session_id)
            .cloned()
            .unwrap_or(CompactionState::Idle)
    }

    /// Get the compaction prompt for a session.
    pub async fn get_compaction_prompt(&self, session_id: &str) -> Result<String, AgentError> {
        self.worker
            .get_compaction_prompt(&self.pool, session_id)
            .await
    }

    /// Calculate persisted context and compaction pressure for a session.
    pub async fn diagnostics_snapshot(
        &self,
        session_id: &str,
    ) -> Result<ContextDiagnosticsSnapshot, AgentError> {
        ContextDiagnosticsSnapshot::calculate(
            &self.pool,
            session_id,
            &self.budget,
            self.recent_turns_to_preserve,
        )
        .await
    }

    /// Trigger compaction for a session, managing state transitions.
    async fn trigger_compaction(
        &self,
        session_id: &str,
        latest_summary: &Option<crate::storage::summaries::StoredSummary>,
    ) {
        let mut state = self.state.lock().await;
        let target_id = latest_summary
            .as_ref()
            .map(|s| s.covers_through_message_id.clone())
            .unwrap_or_default();

        match state.get(session_id) {
            Some(CompactionState::Running { .. })
            | Some(CompactionState::RunningAndDirty { .. }) => {
                // Compaction already running — mark dirty for follow-up
                state.insert(
                    session_id.to_string(),
                    CompactionState::RunningAndDirty {
                        target_through_message_id: target_id,
                    },
                );
                debug!(session_id = %session_id, "compaction already running, marked dirty");
            }
            _ => {
                // Start compaction
                state.insert(
                    session_id.to_string(),
                    CompactionState::Running {
                        target_through_message_id: target_id.clone(),
                    },
                );
                drop(state);

                let worker = self.worker.clone();
                let pool = self.pool.clone();
                let session_id = session_id.to_string();
                let budget = self.budget.clone();
                let state = self.state.clone();

                tokio::spawn(async move {
                    match worker.compact(&pool, &session_id, &budget).await {
                        Ok(new_summary) => {
                            info!(
                                session_id = %session_id,
                                summary_length = new_summary.len(),
                                "compaction completed successfully"
                            );
                        }
                        Err(e) => {
                            error!(
                                session_id = %session_id,
                                error = %e,
                                "compaction failed"
                            );
                        }
                    }

                    // Clear the running state and check for dirty flag
                    let mut s = state.lock().await;
                    match s.get(&session_id) {
                        Some(CompactionState::RunningAndDirty { .. }) => {
                            // Mark idle — the next check_session call will re-trigger
                            s.insert(session_id.clone(), CompactionState::Idle);
                            warn!(
                                session_id = %session_id,
                                "compaction dirty flag cleared; next check_session will re-trigger"
                            );
                        }
                        _ => {
                            s.insert(session_id.clone(), CompactionState::Idle);
                        }
                    }
                });
            }
        }
    }
}
