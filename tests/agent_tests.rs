#![allow(
    dead_code,
    unused,
    unused_imports,
    unused_variables,
    unused_assignments
)]
//! Tests for agent module: run modes and outcomes.

use std::path::PathBuf;

use nerdbot::agent::outcome::{AgentOutcome, AgentResult, RunMetadata};
use nerdbot::agent::run_mode::{AgentRunMode, JobId, TelegramChatId, TelegramUserId};
use nerdbot::llm::types::TokenEstimate;

// ── AgentRunMode: InteractiveReply ───────────────────────────────────────

#[test]
fn test_interactive_reply_chat_id() {
    let mode = AgentRunMode::InteractiveReply {
        chat_id: 123_456_789,
        user_id: 987_654_321,
    };
    assert_eq!(mode.chat_id(), Some(123_456_789i64));
    assert_eq!(mode.user_id(), Some(987_654_321i64));
}

#[test]
fn test_interactive_reply_no_internal_chat() {
    let mode = AgentRunMode::InteractiveReply {
        chat_id: 0,
        user_id: 0,
    };
    assert_eq!(mode.chat_id(), Some(0i64));
    assert_eq!(mode.user_id(), Some(0i64));
}

// ── AgentRunMode: ScheduledJob ───────────────────────────────────────────

#[test]
fn test_scheduled_job_chat_id() {
    let mode = AgentRunMode::ScheduledJob {
        job_id: "job-123".into(),
        default_chat_id: 111_111_111,
        notify_on_completion: true,
    };
    assert_eq!(mode.chat_id(), Some(111_111_111i64));
    assert!(mode.user_id().is_none());
}

#[test]
fn test_scheduled_job_no_notify() {
    let mode = AgentRunMode::ScheduledJob {
        job_id: "job-456".into(),
        default_chat_id: 222_222_222,
        notify_on_completion: false,
    };
    assert_eq!(mode.chat_id(), Some(222_222_222i64));
    assert_eq!(mode.user_id(), None);
}

// ── AgentRunMode: Internal ───────────────────────────────────────────────

#[test]
fn test_internal_run_no_chat_id() {
    let mode = AgentRunMode::Internal {
        reason: "background compaction".into(),
    };
    assert_eq!(mode.chat_id(), None);
    assert_eq!(mode.user_id(), None);
}

#[test]
fn test_internal_run_with_reason() {
    let mode = AgentRunMode::Internal {
        reason: "maintenance".into(),
    };
    assert!(matches!(mode, AgentRunMode::Internal { .. }));
}

// ── AgentRunMode: Clone / Debug ──────────────────────────────────────────

#[test]
fn test_agent_run_mode_clone() {
    let mode = AgentRunMode::InteractiveReply {
        chat_id: 123,
        user_id: 456,
    };
    let cloned = mode.clone();
    assert_eq!(mode, cloned);
}

#[test]
fn test_agent_run_mode_debug() {
    let mode = AgentRunMode::ScheduledJob {
        job_id: "j1".into(),
        default_chat_id: 1,
        notify_on_completion: false,
    };
    let debug_str = format!("{mode:?}");
    assert!(debug_str.contains("ScheduledJob"));
}

// ── AgentRunMode: Serialize / Deserialize ────────────────────────────────

#[test]
fn test_agent_run_mode_serialize_interactive() {
    let mode = AgentRunMode::InteractiveReply {
        chat_id: 123,
        user_id: 456,
    };
    let json = serde_json::to_string(&mode).unwrap();
    assert!(json.contains("InteractiveReply"));
}

#[test]
fn test_agent_run_mode_serialize_scheduled() {
    let mode = AgentRunMode::ScheduledJob {
        job_id: "my-job".into(),
        default_chat_id: 789,
        notify_on_completion: true,
    };
    let json = serde_json::to_string(&mode).unwrap();
    assert!(json.contains("ScheduledJob"));
    assert!(json.contains("my-job"));
}

#[test]
fn test_agent_run_mode_roundtrip() {
    let mode = AgentRunMode::InteractiveReply {
        chat_id: 999,
        user_id: 888,
    };
    let json = serde_json::to_string(&mode).unwrap();
    let restored: AgentRunMode = serde_json::from_str(&json).unwrap();
    assert_eq!(mode, restored);
}

// ── AgentOutcome ─────────────────────────────────────────────────────────

#[test]
fn test_agent_outcome_final_text() {
    let outcome = AgentOutcome::FinalText("Hello, world!".into());
    assert!(matches!(outcome, AgentOutcome::FinalText(ref t) if t == "Hello, world!"));
}

#[test]
fn test_agent_outcome_silent() {
    let outcome = AgentOutcome::Silent;
    assert!(matches!(outcome, AgentOutcome::Silent));
}

#[test]
fn test_agent_outcome_cancelled() {
    let outcome = AgentOutcome::Cancelled;
    assert!(matches!(outcome, AgentOutcome::Cancelled));
}

#[test]
fn test_agent_outcome_clone() {
    let outcome = AgentOutcome::FinalText("test".into());
    let cloned = outcome.clone();
    assert_eq!(outcome, cloned);
}

// ── RunMetadata ──────────────────────────────────────────────────────────

#[test]
fn test_run_metadata_defaults() {
    let metadata = RunMetadata::default();
    assert_eq!(metadata.iterations, 0);
    assert!(metadata.token_estimate.is_none());
}

#[test]
fn test_run_metadata_with_values() {
    let metadata = RunMetadata {
        iterations: 3,
        token_estimate: Some(TokenEstimate::new(100, 50)),
    };
    assert_eq!(metadata.iterations, 3);
    assert!(metadata.token_estimate.is_some());
    let est = metadata.token_estimate.unwrap();
    assert_eq!(est.input_tokens, 100);
    assert_eq!(est.output_tokens, 50);
}

// ── AgentResult ──────────────────────────────────────────────────────────

#[test]
fn test_agent_result_final_text() {
    let result = AgentResult {
        outcome: AgentOutcome::FinalText("done".into()),
        metadata: RunMetadata {
            iterations: 2,
            token_estimate: None,
        },
    };
    assert!(matches!(result.outcome, AgentOutcome::FinalText(ref t) if t == "done"));
    assert_eq!(result.metadata.iterations, 2);
}

#[test]
fn test_agent_result_silent() {
    let result = AgentResult {
        outcome: AgentOutcome::Silent,
        metadata: RunMetadata::default(),
    };
    assert!(matches!(result.outcome, AgentOutcome::Silent));
}

#[test]
fn test_agent_result_cancelled() {
    let result = AgentResult {
        outcome: AgentOutcome::Cancelled,
        metadata: RunMetadata::default(),
    };
    assert!(matches!(result.outcome, AgentOutcome::Cancelled));
}

// ── Type aliases ─────────────────────────────────────────────────────────

#[test]
fn test_type_aliases() {
    let chat_id: TelegramChatId = 123;
    assert_eq!(chat_id, 123i64);

    let user_id: TelegramUserId = 456;
    assert_eq!(user_id, 456i64);

    let job_id: JobId = "my-job".into();
    assert_eq!(job_id, "my-job");
}
