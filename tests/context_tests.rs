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

    // Verify constructor derives values from config
    let llm = nerdbot::config::LlmConfig {
        model: "gpt-4o".into(),
        endpoint: None,
        api_key_env: None,
        temperature: 0.2,
        max_output_tokens: 4096,
        context_window_tokens: 128_000,
    };
    let ctx = nerdbot::config::ContextConfig {
        soft_compaction_threshold: 0.60,
        hard_context_threshold: 0.85,
        recent_turns_to_preserve: 30,
        reserved_tool_loop_tokens: 8_192,
    };
    let budget_from_config = ContextBudget::from_llm_and_context(&llm, &ctx);
    assert_eq!(budget_from_config.context_window_tokens, 128_000);
    assert_eq!(budget_from_config.reserved_output_tokens, 4_096);
    assert_eq!(budget_from_config.reserved_tool_loop_tokens, 8_192);
}

#[test]
fn test_usable_input_budget() {
    let budget = ContextBudget {
        context_window_tokens: 100_000,
        reserved_output_tokens: 2_000,
        soft_compaction_threshold: 0.5,
        hard_context_threshold: 0.8,
        reserved_tool_loop_tokens: 3_000,
    };
    assert_eq!(budget.usable_input_budget(), 95_000);
}

#[test]
fn test_usable_budget_with_zero_reservations() {
    let budget = ContextBudget {
        context_window_tokens: 50_000,
        reserved_output_tokens: 0,
        soft_compaction_threshold: 0.5,
        hard_context_threshold: 0.8,
        reserved_tool_loop_tokens: 0,
    };
    assert_eq!(budget.usable_input_budget(), 50_000);
}

#[test]
fn test_soft_threshold_tokens() {
    let budget = ContextBudget {
        context_window_tokens: 100_000,
        reserved_output_tokens: 0,
        soft_compaction_threshold: 0.5,
        hard_context_threshold: 0.8,
        reserved_tool_loop_tokens: 0,
    };
    assert_eq!(budget.soft_threshold_tokens(), 50_000);
}

#[test]
fn test_hard_threshold_tokens() {
    let budget = ContextBudget {
        context_window_tokens: 100_000,
        reserved_output_tokens: 0,
        soft_compaction_threshold: 0.5,
        hard_context_threshold: 0.8,
        reserved_tool_loop_tokens: 0,
    };
    assert_eq!(budget.hard_threshold_tokens(), 80_000);
}

#[test]
fn test_thresholds_respect_reservations() {
    let budget = ContextBudget {
        context_window_tokens: 100_000,
        reserved_output_tokens: 10_000,
        soft_compaction_threshold: 0.5,
        hard_context_threshold: 0.75,
        reserved_tool_loop_tokens: 5_000,
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
        soft_compaction_threshold: 0.6,
        hard_context_threshold: 0.8,
        reserved_tool_loop_tokens: 1_024,
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
        budget.reserved_tool_loop_tokens,
        cloned.reserved_tool_loop_tokens
    );
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

// ── Phase 9 behavior tests ──────────────────────────────────────────────

use async_trait::async_trait;
use genai::chat::{
    ChatMessage, ChatOptions, ChatRequest, ChatResponse, ChatRole, MessageContent, Usage,
};
use nerdbot::context::compaction_service::CompactionService;
use nerdbot::context::compaction_worker::CompactionWorker;
use nerdbot::context::manager::ContextManager;
use nerdbot::error::AgentError;
use nerdbot::llm::LlmExecutor;
use sqlx::sqlite::SqlitePoolOptions;
use std::sync::Arc;
use std::time::Duration;

async fn setup_context_pool() -> sqlx::SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("failed to create in-memory DB");

    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("failed to run migrations");

    pool
}

fn small_budget(context_window_tokens: usize, soft_compaction_threshold: f32) -> ContextBudget {
    ContextBudget {
        context_window_tokens,
        reserved_output_tokens: 0,
        soft_compaction_threshold,
        hard_context_threshold: 0.9,
        reserved_tool_loop_tokens: 0,
    }
}

fn msg_text(message: &ChatMessage) -> String {
    message.content.joined_texts().unwrap_or_default()
}

async fn set_message_created_at(pool: &sqlx::SqlitePool, message_id: &str, seconds: i64) {
    let created_at = chrono::DateTime::<chrono::Utc>::from_timestamp(seconds, 0)
        .unwrap()
        .to_rfc3339();
    sqlx::query("UPDATE messages SET created_at = ?1 WHERE id = ?2")
        .bind(created_at)
        .bind(message_id)
        .execute(pool)
        .await
        .unwrap();
}

async fn create_user_message(
    pool: &sqlx::SqlitePool,
    session_id: &str,
    text: &str,
    token_estimate: Option<usize>,
) -> nerdbot::storage::StoredMessage {
    let message = ChatMessage::user(MessageContent::from_text(text));
    nerdbot::storage::messages::create_message(pool, session_id, &message, token_estimate)
        .await
        .unwrap()
}

async fn wait_for_summary(
    pool: &sqlx::SqlitePool,
    session_id: &str,
) -> Option<nerdbot::storage::StoredSummary> {
    for _ in 0..20 {
        let summary = nerdbot::storage::summaries::get_latest_summary(pool, session_id)
            .await
            .unwrap();
        if summary.is_some() {
            return summary;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    None
}

#[tokio::test]
async fn test_assemble_messages_no_summary() {
    let pool = setup_context_pool().await;
    let session = nerdbot::storage::sessions::create_session(&pool, 1)
        .await
        .unwrap();
    create_user_message(&pool, &session.id, "stored one", Some(1)).await;
    let assistant = ChatMessage::assistant(MessageContent::from_text("stored two"));
    nerdbot::storage::messages::create_message(&pool, &session.id, &assistant, Some(1))
        .await
        .unwrap();

    let manager = ContextManager::new(pool, ContextBudget::default());
    let messages = manager
        .assemble_messages(
            &session.id,
            "test personality",
            ChatMessage::user(MessageContent::from_text("current user")),
        )
        .await
        .unwrap();

    assert_eq!(messages.len(), 4);
    assert!(matches!(messages[0].role, ChatRole::System));
    assert_eq!(msg_text(&messages[0]), "test personality");
    assert_eq!(msg_text(&messages[1]), "stored one");
    assert_eq!(msg_text(&messages[2]), "stored two");
    assert_eq!(msg_text(&messages[3]), "current user");
}

#[tokio::test]
async fn test_assemble_messages_with_summary() {
    let pool = setup_context_pool().await;
    let session = nerdbot::storage::sessions::create_session(&pool, 1)
        .await
        .unwrap();
    let old = create_user_message(&pool, &session.id, "old covered message", Some(1)).await;
    let recent = create_user_message(&pool, &session.id, "recent raw message", Some(1)).await;
    set_message_created_at(&pool, &old.id, 1_700_000_000).await;
    set_message_created_at(&pool, &recent.id, 1_700_000_010).await;

    let summary = ContextSummary::new(
        session.id.clone(),
        "summary text".to_string(),
        old.id.clone(),
    );
    nerdbot::storage::summaries::create_summary(&pool, &summary)
        .await
        .unwrap();

    let manager = ContextManager::new(pool, ContextBudget::default());
    let messages = manager
        .assemble_messages(
            &session.id,
            "test personality",
            ChatMessage::user(MessageContent::from_text("current user")),
        )
        .await
        .unwrap();

    let texts: Vec<String> = messages.iter().map(msg_text).collect();
    assert!(texts.iter().any(|t| t == "test personality"));
    assert!(texts.iter().any(|t| t.contains("summary text")));
    assert!(texts.iter().any(|t| t == "recent raw message"));
    assert!(texts.iter().any(|t| t == "current user"));
    assert!(!texts.iter().any(|t| t == "old covered message"));
}

#[tokio::test]
async fn test_assemble_messages_empty_personality() {
    let pool = setup_context_pool().await;
    let session = nerdbot::storage::sessions::create_session(&pool, 1)
        .await
        .unwrap();

    let manager = ContextManager::new(pool, ContextBudget::default());
    let messages = manager
        .assemble_messages(
            &session.id,
            "",
            ChatMessage::user(MessageContent::from_text("current user")),
        )
        .await
        .unwrap();

    assert_eq!(messages.len(), 1);
    assert!(matches!(messages[0].role, ChatRole::User));
    assert_eq!(msg_text(&messages[0]), "current user");
}

#[tokio::test]
async fn test_assemble_messages_budget_exceeded() {
    let pool = setup_context_pool().await;
    let session = nerdbot::storage::sessions::create_session(&pool, 1)
        .await
        .unwrap();
    for i in 1..=4 {
        let msg = create_user_message(&pool, &session.id, &format!("m{i}"), Some(5)).await;
        set_message_created_at(&pool, &msg.id, 1_700_000_000 + i).await;
    }

    let manager = ContextManager::new(pool, small_budget(10, 0.5));
    let messages = manager
        .assemble_messages(
            &session.id,
            "personality",
            ChatMessage::user(MessageContent::from_text("current")),
        )
        .await
        .unwrap();
    let texts: Vec<String> = messages.iter().map(msg_text).collect();

    assert!(texts.iter().any(|t| t == "m3"));
    assert!(texts.iter().any(|t| t == "m4"));
    assert!(!texts.iter().any(|t| t == "m1"));
    assert!(!texts.iter().any(|t| t == "m2"));
    assert!(texts.iter().any(|t| t == "current"));
}

#[tokio::test]
async fn test_assemble_messages_budget_not_exceeded() {
    let pool = setup_context_pool().await;
    let session = nerdbot::storage::sessions::create_session(&pool, 1)
        .await
        .unwrap();
    for i in 1..=3 {
        create_user_message(&pool, &session.id, &format!("m{i}"), Some(5)).await;
    }

    let manager = ContextManager::new(pool, small_budget(100, 0.5));
    let messages = manager
        .assemble_messages(
            &session.id,
            "personality",
            ChatMessage::user(MessageContent::from_text("current")),
        )
        .await
        .unwrap();
    let texts: Vec<String> = messages.iter().map(msg_text).collect();

    for expected in ["m1", "m2", "m3", "current"] {
        assert!(texts.iter().any(|t| t == expected), "missing {expected}");
    }
}

#[tokio::test]
async fn test_check_session_below_threshold() {
    let pool = setup_context_pool().await;
    let session = nerdbot::storage::sessions::create_session(&pool, 1)
        .await
        .unwrap();
    create_user_message(&pool, &session.id, "hello", None).await;

    let llm =
        nerdbot::llm::fake::FakeProvider::new(vec![nerdbot::llm::fake::FakeResponse::final_text(
            "# Summary",
        )]);
    let service = CompactionService::new(
        pool.clone(),
        Arc::new(CompactionWorker::new(
            Arc::new(llm),
            "fake-model".into(),
            0.0,
        )),
        ContextBudget::default(),
    );
    service.check_session(&session.id).await.unwrap();

    assert_eq!(service.get_state(&session.id).await, CompactionState::Idle);
    assert!(
        nerdbot::storage::summaries::get_latest_summary(&pool, &session.id)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn test_check_session_above_threshold() {
    let pool = setup_context_pool().await;
    let session = nerdbot::storage::sessions::create_session(&pool, 1)
        .await
        .unwrap();
    create_user_message(&pool, &session.id, "hello", None).await;

    let service = CompactionService::new(
        pool.clone(),
        Arc::new(CompactionWorker::new(
            Arc::new(nerdbot::llm::fake::FakeProvider::new(vec![
                nerdbot::llm::fake::FakeResponse::final_text("# Summary"),
            ])),
            "fake-model".into(),
            0.0,
        )),
        small_budget(100, 0.1),
    );
    service.check_session(&session.id).await.unwrap();

    let summary = wait_for_summary(&pool, &session.id).await;
    assert!(
        summary.is_some(),
        "expected background compaction to create a summary"
    );
    assert_eq!(service.get_state(&session.id).await, CompactionState::Idle);
}

struct SlowProvider;

#[async_trait]
impl LlmExecutor for SlowProvider {
    async fn complete(
        &self,
        _model: &str,
        _request: ChatRequest,
        _options: ChatOptions,
    ) -> Result<ChatResponse, AgentError> {
        tokio::time::sleep(Duration::from_millis(150)).await;
        Ok(ChatResponse {
            content: MessageContent::from_text("# Conversation Working Summary\n\nSlow summary"),
            reasoning_content: None,
            model_iden: genai::ModelIden::new(genai::adapter::AdapterKind::OpenAI, "fake-model"),
            provider_model_iden: genai::ModelIden::new(
                genai::adapter::AdapterKind::OpenAI,
                "fake-model",
            ),
            stop_reason: Some(genai::chat::StopReason::Completed("stop".into())),
            usage: Usage::default(),
            captured_raw_body: None,
            response_id: None,
        })
    }
}

#[tokio::test]
async fn test_compaction_state_transitions() {
    let pool = setup_context_pool().await;
    let session = nerdbot::storage::sessions::create_session(&pool, 1)
        .await
        .unwrap();
    create_user_message(&pool, &session.id, "hello", None).await;

    let worker = CompactionWorker::new(Arc::new(SlowProvider), "fake-model".into(), 0.0);
    let service = CompactionService::new(pool, Arc::new(worker), small_budget(100, 0.1));

    assert_eq!(service.get_state(&session.id).await, CompactionState::Idle);
    service.check_session(&session.id).await.unwrap();
    assert!(matches!(
        service.get_state(&session.id).await,
        CompactionState::Running { .. }
    ));

    service.check_session(&session.id).await.unwrap();
    assert!(matches!(
        service.get_state(&session.id).await,
        CompactionState::RunningAndDirty { .. }
    ));

    for _ in 0..20 {
        if service.get_state(&session.id).await == CompactionState::Idle {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("compaction service did not return to Idle");
}

#[tokio::test]
async fn test_compact_no_messages() {
    let pool = setup_context_pool().await;
    let session = nerdbot::storage::sessions::create_session(&pool, 1)
        .await
        .unwrap();
    let llm =
        nerdbot::llm::fake::FakeProvider::new(vec![nerdbot::llm::fake::FakeResponse::final_text(
            "# Summary",
        )]);
    let worker = CompactionWorker::new(Arc::new(llm), "fake-model".into(), 0.0);

    let err = worker
        .compact(&pool, &session.id, &ContextBudget::default())
        .await
        .unwrap_err();
    assert!(matches!(err, AgentError::Compaction(_)));
}

#[tokio::test]
async fn test_compact_with_llm() {
    let pool = setup_context_pool().await;
    let session = nerdbot::storage::sessions::create_session(&pool, 1)
        .await
        .unwrap();
    let stored = create_user_message(&pool, &session.id, "remember this", None).await;

    let llm =
        nerdbot::llm::fake::FakeProvider::new(vec![nerdbot::llm::fake::FakeResponse::final_text(
            "# Conversation Working Summary\n\nTest summary content",
        )]);
    let worker = CompactionWorker::new(Arc::new(llm), "fake-model".into(), 0.0);
    let summary_text = worker
        .compact(&pool, &session.id, &ContextBudget::default())
        .await
        .unwrap();
    let summary = nerdbot::storage::summaries::get_latest_summary(&pool, &session.id)
        .await
        .unwrap()
        .unwrap();

    assert!(summary_text.contains("Conversation Working Summary"));
    assert_eq!(summary.covers_through_message_id, stored.id);
}

#[tokio::test]
async fn test_compact_replaces_old_summary() {
    let pool = setup_context_pool().await;
    let session = nerdbot::storage::sessions::create_session(&pool, 1)
        .await
        .unwrap();
    let old = create_user_message(&pool, &session.id, "old covered", None).await;
    let new = create_user_message(&pool, &session.id, "new uncovered", None).await;
    set_message_created_at(&pool, &old.id, 1_700_000_000).await;
    set_message_created_at(&pool, &new.id, 1_700_000_010).await;

    let old_summary = ContextSummary::new(
        session.id.clone(),
        "old summary".to_string(),
        old.id.clone(),
    );
    let stored_old_summary = nerdbot::storage::summaries::create_summary(&pool, &old_summary)
        .await
        .unwrap();

    let llm =
        nerdbot::llm::fake::FakeProvider::new(vec![nerdbot::llm::fake::FakeResponse::final_text(
            "# Summary",
        )]);
    let worker = CompactionWorker::new(Arc::new(llm), "fake-model".into(), 0.0);
    worker
        .compact(&pool, &session.id, &ContextBudget::default())
        .await
        .unwrap();

    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM context_summaries WHERE chat_session_id = ?1")
            .bind(&session.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    let latest = nerdbot::storage::summaries::get_latest_summary(&pool, &session.id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(count, 1);
    assert_ne!(latest.id, stored_old_summary.id);
    assert_eq!(latest.covers_through_message_id, new.id);
}
