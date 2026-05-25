//! Compaction service — monitors session pressure and triggers async compaction.
//!
//! Phase 9 implementation.

use std::collections::HashMap;
use std::sync::Arc;

use sqlx::SqlitePool;
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};

use crate::context::budget::ContextBudget;
use crate::context::compaction_worker::CompactionWorker;
use crate::error::AgentError;

/// State of compaction for a session.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    state: Arc<Mutex<HashMap<String, CompactionState>>>,
}

impl CompactionService {
    /// Create a new CompactionService.
    pub fn new(pool: SqlitePool, worker: Arc<CompactionWorker>, budget: ContextBudget) -> Self {
        Self {
            pool,
            worker,
            budget,
            state: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Check if a session needs compaction and enqueue if so.
    ///
    /// Compares the current message count against the soft threshold.
    /// If compaction is already running for the session, marks it dirty
    /// so a follow-up compaction will be triggered after the current one.
    pub async fn check_session(&self, session_id: &str) -> Result<(), AgentError> {
        // Count messages in the session
        let message_count = self.count_messages(session_id).await?;

        // Load latest summary to understand coverage
        let latest_summary =
            crate::storage::summaries::get_latest_summary(&self.pool, session_id).await?;

        // Calculate pressure: fraction of usable budget consumed
        let usable_budget = self.budget.usable_input_budget();
        // Rough estimate: average message ~100 tokens
        let estimated_tokens = message_count.saturating_mul(100);
        let pressure = if usable_budget > 0 {
            estimated_tokens as f32 / usable_budget as f32
        } else {
            0.0
        };

        debug!(
            session_id = %session_id,
            message_count,
            estimated_tokens,
            usable_budget,
            pressure,
            soft_threshold = self.budget.soft_compaction_threshold,
            "compaction pressure check"
        );

        if pressure >= self.budget.soft_compaction_threshold {
            info!(
                session_id = %session_id,
                pressure,
                "session pressure exceeds soft threshold, triggering compaction"
            );
            self.trigger_compaction(session_id, &latest_summary).await;
        } else {
            debug!(
                session_id = %session_id,
                pressure,
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

    /// Count messages in a session.
    async fn count_messages(&self, session_id: &str) -> Result<usize, AgentError> {
        let messages =
            crate::storage::messages::list_messages(&self.pool, session_id, None).await?;
        Ok(messages.len())
    }
}

impl Default for CompactionService {
    fn default() -> Self {
        // Use a minimal in-memory pool for the default case.
        // This is only used when CompactionService::default() is called
        // (e.g. in tests or when not explicitly constructed).
        let pool = match tokio::runtime::Handle::current()
            .block_on(sqlx::SqlitePool::connect(":memory:"))
        {
            Ok(p) => p,
            Err(_) => {
                // If we can't create a pool, use a placeholder
                // This shouldn't happen in practice
                panic!("Failed to create in-memory SQLite pool for default CompactionService");
            }
        };
        Self::new(
            pool,
            Arc::new(CompactionWorker::new()),
            ContextBudget::default(),
        )
    }
}
