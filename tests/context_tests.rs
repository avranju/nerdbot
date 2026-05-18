#![allow(
    dead_code,
    unused,
    unused_imports,
    unused_variables,
    unused_assignments
)]
//! Tests for context management: budgeting, summaries, compaction state.

use std::collections::HashMap;

use nerdbot::context::budget::ContextBudget;
use nerdbot::context::compaction_service::CompactionState;
use nerdbot::context::summaries::ContextSummary;

// ── ContextBudget ────────────────────────────────────────────────────────

#[test]
fn test_default_budget() {
    let budget = ContextBudget::default();
    assert_eq!(budget.context_window_tokens, 128_000);
    assert_eq!(budget.reserved_output_tokens, 4_096);
    assert_eq!(budget.reserved_tool_loop_tokens, 8_192);
    assert_eq!(budget.soft_compaction_threshold, 0.60);
    assert_eq!(budget.hard_context_threshold, 0.85);
}

#[test]
fn test_usable_input_budget() {
    let budget = ContextBudget {
        context_window_tokens: 100_000,
        reserved_output_tokens: 2_000,
        reserved_tool_loop_tokens: 3_000,
        soft_compaction_threshold: 0.5,
        hard_context_threshold: 0.8,
    };
    assert_eq!(budget.usable_input_budget(), 95_000);
}

#[test]
fn test_usable_budget_with_zero_reservations() {
    let budget = ContextBudget {
        context_window_tokens: 50_000,
        reserved_output_tokens: 0,
        reserved_tool_loop_tokens: 0,
        soft_compaction_threshold: 0.5,
        hard_context_threshold: 0.8,
    };
    assert_eq!(budget.usable_input_budget(), 50_000);
}

#[test]
fn test_soft_threshold_tokens() {
    let budget = ContextBudget {
        context_window_tokens: 100_000,
        reserved_output_tokens: 0,
        reserved_tool_loop_tokens: 0,
        soft_compaction_threshold: 0.5,
        hard_context_threshold: 0.8,
    };
    assert_eq!(budget.soft_threshold_tokens(), 50_000);
}

#[test]
fn test_hard_threshold_tokens() {
    let budget = ContextBudget {
        context_window_tokens: 100_000,
        reserved_output_tokens: 0,
        reserved_tool_loop_tokens: 0,
        soft_compaction_threshold: 0.5,
        hard_context_threshold: 0.8,
    };
    assert_eq!(budget.hard_threshold_tokens(), 80_000);
}

#[test]
fn test_thresholds_respect_reservations() {
    let budget = ContextBudget {
        context_window_tokens: 100_000,
        reserved_output_tokens: 10_000,
        reserved_tool_loop_tokens: 5_000,
        soft_compaction_threshold: 0.5,
        hard_context_threshold: 0.75,
    };
    // usable = 100_000 - 10_000 - 5_000 = 85_000
    // soft = 85_000 * 0.5 = 42_500
    assert_eq!(budget.soft_threshold_tokens(), 42_500);
    // hard = 85_000 * 0.75 = 63_750
    assert_eq!(budget.hard_threshold_tokens(), 63_750);
}

#[test]
fn test_thresholds_ordering() {
    let budget = ContextBudget::default();
    let soft = budget.soft_threshold_tokens();
    let hard = budget.hard_threshold_tokens();
    // soft threshold should always be less than hard threshold
    assert!(
        soft < hard,
        "soft threshold ({soft}) must be less than hard threshold ({hard})"
    );
}

#[test]
fn test_budget_custom_values() {
    let budget = ContextBudget {
        context_window_tokens: 8_192,
        reserved_output_tokens: 512,
        reserved_tool_loop_tokens: 1_024,
        soft_compaction_threshold: 0.6,
        hard_context_threshold: 0.8,
    };
    assert_eq!(budget.usable_input_budget(), 6_656);
    assert_eq!(budget.soft_threshold_tokens(), 3_993);
    assert_eq!(budget.hard_threshold_tokens(), 5_324);
}

#[test]
fn test_budget_clone() {
    let budget = ContextBudget::default();
    let cloned = budget.clone();
    assert_eq!(budget.context_window_tokens, cloned.context_window_tokens);
    assert_eq!(budget.reserved_output_tokens, cloned.reserved_output_tokens);
    assert_eq!(
        budget.soft_compaction_threshold,
        cloned.soft_compaction_threshold
    );
}

// ── ContextSummary ───────────────────────────────────────────────────────

#[test]
fn test_context_summary_new() {
    let summary = ContextSummary::new(
        "session-1".into(),
        "User asked about Rust. Discussed memory safety.".into(),
        "msg-42".into(),
    );
    assert_eq!(summary.chat_session_id, "session-1");
    assert_eq!(
        summary.summary_text,
        "User asked about Rust. Discussed memory safety."
    );
    assert_eq!(summary.covers_through_message_id, "msg-42");
    assert!(summary.created_at <= chrono::Utc::now());
}

#[test]
fn test_context_summary_fields() {
    let summary = ContextSummary::new("s1".into(), "Summary text here.".into(), "msg-100".into());
    assert!(summary.created_at > chrono::DateTime::<chrono::Utc>::MIN_UTC);
}

#[test]
fn test_context_summary_clone() {
    let s1 = ContextSummary::new("s1".into(), "text".into(), "m1".into());
    let s2 = s1.clone();
    assert_eq!(s1.chat_session_id, s2.chat_session_id);
    assert_eq!(s1.summary_text, s2.summary_text);
    assert_eq!(s1.covers_through_message_id, s2.covers_through_message_id);
}

#[test]
fn test_context_summary_serialization() {
    let summary = ContextSummary::new(
        "test-session".into(),
        "Test summary content.".into(),
        "last-msg-id".into(),
    );
    let json = serde_json::to_string(&summary).unwrap();
    assert!(json.contains("test-session"));
    assert!(json.contains("Test summary content."));
    assert!(json.contains("last-msg-id"));
}

#[test]
fn test_context_summary_roundtrip() {
    let summary = ContextSummary::new(
        "roundtrip-sess".into(),
        "Round trip test.".into(),
        "msg-99".into(),
    );
    let json = serde_json::to_string(&summary).unwrap();
    let restored: ContextSummary = serde_json::from_str(&json).unwrap();
    assert_eq!(summary.chat_session_id, restored.chat_session_id);
    assert_eq!(summary.summary_text, restored.summary_text);
    assert_eq!(
        summary.covers_through_message_id,
        restored.covers_through_message_id
    );
}

// ── CompactionState ──────────────────────────────────────────────────────

#[test]
fn test_compaction_state_idle() {
    let state = CompactionState::Idle;
    assert!(matches!(state, CompactionState::Idle));
    assert!(!matches!(state, CompactionState::Running { .. }));
}

#[test]
fn test_compaction_state_running() {
    let state = CompactionState::Running {
        target_through_message_id: "msg-50".into(),
    };
    assert!(
        matches!(state, CompactionState::Running { target_through_message_id } if target_through_message_id == "msg-50")
    );
}

#[test]
fn test_compaction_state_running_and_dirty() {
    let state = CompactionState::RunningAndDirty {
        target_through_message_id: "msg-75".into(),
    };
    assert!(matches!(
        state,
        CompactionState::RunningAndDirty { target_through_message_id } if target_through_message_id == "msg-75"
    ));
}

#[test]
fn test_compaction_state_equality() {
    let idle1 = CompactionState::Idle;
    let idle2 = CompactionState::Idle;
    assert_eq!(idle1, idle2);
}

#[test]
fn test_compaction_state_inequality() {
    let idle = CompactionState::Idle;
    let running = CompactionState::Running {
        target_through_message_id: "msg-1".into(),
    };
    assert_ne!(idle, running);
}

#[test]
fn test_compaction_state_different_running_targets() {
    let s1 = CompactionState::Running {
        target_through_message_id: "msg-10".into(),
    };
    let s2 = CompactionState::Running {
        target_through_message_id: "msg-20".into(),
    };
    assert_ne!(s1, s2);
}

#[test]
fn test_compaction_state_running_vs_dirty() {
    let running = CompactionState::Running {
        target_through_message_id: "msg-5".into(),
    };
    let dirty = CompactionState::RunningAndDirty {
        target_through_message_id: "msg-5".into(),
    };
    // Running and RunningAndDirty with same target are different states
    assert_ne!(running, dirty);
}

#[test]
fn test_compaction_state_clone() {
    let state = CompactionState::Running {
        target_through_message_id: "msg-42".into(),
    };
    let cloned = state.clone();
    assert_eq!(state, cloned);
}

#[test]
fn test_compaction_state_running_and_dirty_with_different_targets() {
    let running_and_dirty = CompactionState::RunningAndDirty {
        target_through_message_id: "msg-100".into(),
    };
    let running = CompactionState::Running {
        target_through_message_id: "msg-100".into(),
    };
    assert_ne!(running_and_dirty, running);
}

#[test]
fn test_compaction_state_running_equality() {
    let s1 = CompactionState::Running {
        target_through_message_id: "msg-1".into(),
    };
    let s2 = CompactionState::Running {
        target_through_message_id: "msg-1".into(),
    };
    assert_eq!(s1, s2);
}
