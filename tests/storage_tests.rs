//! Integration tests for the storage layer.
//!
//! Uses temporary SQLite files to test CRUD operations
//! for sessions, messages, summaries, and jobs.

#![allow(
    dead_code,
    unused,
    unused_imports,
    unused_variables,
    unused_assignments
)]

use std::path::PathBuf;
use std::sync::OnceLock;

use tempfile::NamedTempFile;
use tokio::sync::Mutex;

use genai::chat::{ChatMessage, ChatRole, ContentPart, MessageContent, ToolCall, ToolResponse};
use nerdbot::error::AgentError;
use nerdbot::scheduler::models::{JobContextPolicy, JobStatus, ScheduleType};
use nerdbot::storage::{ChatSession, Database, StoredJob, StoredMessage, StoredSummary};

/// A global lock to prevent parallel temp file cleanup races.
static TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

/// Get the test lock (creates it lazily).
fn test_lock() -> &'static Mutex<()> {
    TEST_LOCK.get_or_init(|| Mutex::new(()))
}

/// Create a database backed by a temporary file and initialize it.
async fn setup_db() -> Database {
    let _lock = test_lock().lock().await;
    let tmp = NamedTempFile::new().unwrap();
    let path = tmp.path().to_path_buf();
    let db = Database::new(path.clone()).await.unwrap();
    db.init().await.unwrap();
    // Leak the temp file so it doesn't get deleted while the DB is in use
    std::mem::forget(tmp);
    db
}

/// Get the pool from a database for use with direct function calls.
fn pool(db: &Database) -> &sqlx::SqlitePool {
    db.pool()
}

// ── Session CRUD ───────────────────────────────────────────────────────

#[tokio::test]
async fn test_create_and_get_session() {
    let db = setup_db().await;
    let pool = pool(&db);

    let session = nerdbot::storage::sessions::create_session(pool, 123_456_789)
        .await
        .unwrap();
    assert_eq!(session.conversation_id, "123456789");
    assert!(!session.id.is_empty());

    let found = nerdbot::storage::sessions::get_session(pool, &session.id)
        .await
        .unwrap();
    assert!(found.is_some());
    assert_eq!(found.unwrap().conversation_id, "123456789");
}

#[tokio::test]
async fn test_get_nonexistent_session() {
    let db = setup_db().await;
    let pool = pool(&db);

    let found = nerdbot::storage::sessions::get_session(pool, "nonexistent-id")
        .await
        .unwrap();
    assert!(found.is_none());
}

#[tokio::test]
async fn test_get_session_for_chat() {
    let db = setup_db().await;
    let pool = pool(&db);

    let session = nerdbot::storage::sessions::create_session(pool, 42)
        .await
        .unwrap();

    let found = nerdbot::storage::sessions::get_session_for_chat(pool, 42)
        .await
        .unwrap();
    assert!(found.is_some());
    assert_eq!(found.unwrap().id, session.id);
}

#[tokio::test]
async fn test_get_session_for_chat_no_match() {
    let db = setup_db().await;
    let pool = pool(&db);

    // Create a session for chat 1
    nerdbot::storage::sessions::create_session(pool, 1)
        .await
        .unwrap();

    // Query for chat 2 — should return None
    let found = nerdbot::storage::sessions::get_session_for_chat(pool, 999)
        .await
        .unwrap();
    assert!(found.is_none());
}

#[tokio::test]
async fn test_update_session() {
    let db = setup_db().await;
    let pool = pool(&db);

    let session = nerdbot::storage::sessions::create_session(pool, 1)
        .await
        .unwrap();

    // Update the session
    nerdbot::storage::sessions::update_session(pool, &session.id)
        .await
        .unwrap();

    // Verify it still exists with updated timestamp
    let found = nerdbot::storage::sessions::get_session(pool, &session.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.id, session.id);
    assert!(found.updated_at >= session.updated_at);
}

#[tokio::test]
async fn test_create_multiple_sessions_same_chat() {
    let db = setup_db().await;
    let pool = pool(&db);

    let s1 = nerdbot::storage::sessions::create_session(pool, 1)
        .await
        .unwrap();
    let s2 = nerdbot::storage::sessions::create_session(pool, 1)
        .await
        .unwrap();

    // get_session_for_chat returns the latest one
    let latest = nerdbot::storage::sessions::get_session_for_chat(pool, 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(latest.id, s2.id);
}

// ── Message CRUD ───────────────────────────────────────────────────────

#[tokio::test]
async fn test_create_and_list_messages() {
    let db = setup_db().await;
    let pool = pool(&db);

    // Create a session first
    let session = nerdbot::storage::sessions::create_session(pool, 1)
        .await
        .unwrap();

    // Create messages
    let msg1 = ChatMessage::user(MessageContent::from_text("Hello"));
    let msg2 = ChatMessage::assistant(MessageContent::from_text("Hi there!"));

    let stored1 = nerdbot::storage::messages::create_message(pool, &session.id, &msg1, Some(10))
        .await
        .unwrap();
    let stored2 = nerdbot::storage::messages::create_message(pool, &session.id, &msg2, Some(15))
        .await
        .unwrap();

    // List messages
    let messages = nerdbot::storage::messages::list_messages(pool, &session.id, None)
        .await
        .unwrap();
    assert_eq!(messages.len(), 2);
    let contents: Vec<_> = messages.iter().map(|m| m.content.as_str()).collect();
    assert!(contents.contains(&"Hello"));
    assert!(contents.contains(&"Hi there!"));
}

#[tokio::test]
async fn test_list_messages_with_limit() {
    let db = setup_db().await;
    let pool = pool(&db);

    let session = nerdbot::storage::sessions::create_session(pool, 1)
        .await
        .unwrap();

    for i in 0..5 {
        let msg = ChatMessage::user(MessageContent::from_text(format!("Message {i}")));
        nerdbot::storage::messages::create_message(pool, &session.id, &msg, None)
            .await
            .unwrap();
    }

    let messages = nerdbot::storage::messages::list_messages(pool, &session.id, Some(3))
        .await
        .unwrap();
    assert_eq!(messages.len(), 3);
}

#[tokio::test]
async fn test_list_messages_empty_session() {
    let db = setup_db().await;
    let pool = pool(&db);

    let session = nerdbot::storage::sessions::create_session(pool, 1)
        .await
        .unwrap();

    let messages = nerdbot::storage::messages::list_messages(pool, &session.id, None)
        .await
        .unwrap();
    assert!(messages.is_empty());
}

#[tokio::test]
async fn test_message_to_message_conversion() {
    let db = setup_db().await;
    let pool = pool(&db);

    let session = nerdbot::storage::sessions::create_session(pool, 1)
        .await
        .unwrap();

    let original = ChatMessage::user(MessageContent::from_text("test content"));
    let stored = nerdbot::storage::messages::create_message(pool, &session.id, &original, Some(5))
        .await
        .unwrap();

    let converted = stored.to_message().unwrap();
    assert_eq!(converted.role, ChatRole::User);
    assert_eq!(converted.content.first_text(), Some("test content"));
}

#[tokio::test]
async fn test_message_to_message_conversion_structured() {
    let db = setup_db().await;
    let pool = pool(&db);

    let session = nerdbot::storage::sessions::create_session(pool, 1)
        .await
        .unwrap();

    let tool_call = ToolCall {
        call_id: "call-123".into(),
        fn_name: "web_search".into(),
        fn_arguments: serde_json::json!({"query": "rust language"}),
        thought_signatures: None,
    };
    let original = ChatMessage::assistant(MessageContent::from_parts(vec![
        ContentPart::Text("Let me look that up.".into()),
        ContentPart::ToolCall(tool_call),
    ]));

    let stored = nerdbot::storage::messages::create_message(pool, &session.id, &original, Some(25))
        .await
        .unwrap();

    let converted = stored.to_message().unwrap();
    assert_eq!(converted.role, ChatRole::Assistant);
    let parts = converted.content.parts();
    assert_eq!(parts.len(), 2);
    match &parts[0] {
        ContentPart::Text(t) => assert_eq!(t, "Let me look that up."),
        _ => panic!("expected first part to be text"),
    }
    match &parts[1] {
        ContentPart::ToolCall(tc) => {
            assert_eq!(tc.call_id, "call-123");
            assert_eq!(tc.fn_name, "web_search");
            assert_eq!(tc.fn_arguments["query"], "rust language");
        }
        _ => panic!("expected second part to be a tool call"),
    }
}

// ── Summary CRUD ───────────────────────────────────────────────────────

#[tokio::test]
async fn test_create_and_get_summary() {
    let db = setup_db().await;
    let pool = pool(&db);

    let session = nerdbot::storage::sessions::create_session(pool, 1)
        .await
        .unwrap();

    let summary = nerdbot::context::summaries::ContextSummary::new(
        session.id.clone(),
        "Conversation was about Rust programming.".into(),
        "msg-10".into(),
    );

    let stored = nerdbot::storage::summaries::create_summary(pool, &summary)
        .await
        .unwrap();

    assert_eq!(stored.chat_session_id, session.id);
    assert_eq!(
        stored.summary_text,
        "Conversation was about Rust programming."
    );

    let found = nerdbot::storage::summaries::get_latest_summary(pool, &session.id)
        .await
        .unwrap();
    assert!(found.is_some());
    assert_eq!(found.unwrap().id, stored.id);
}

#[tokio::test]
async fn test_get_latest_summary_only() {
    let db = setup_db().await;
    let pool = pool(&db);

    let session = nerdbot::storage::sessions::create_session(pool, 1)
        .await
        .unwrap();

    let summary1 = nerdbot::context::summaries::ContextSummary::new(
        session.id.clone(),
        "Summary 1".into(),
        "msg-5".into(),
    );
    nerdbot::storage::summaries::create_summary(pool, &summary1)
        .await
        .unwrap();

    // Small delay to ensure different timestamps
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;

    let summary2 = nerdbot::context::summaries::ContextSummary::new(
        session.id.clone(),
        "Summary 2 (later)".into(),
        "msg-20".into(),
    );
    nerdbot::storage::summaries::create_summary(pool, &summary2)
        .await
        .unwrap();

    let latest = nerdbot::storage::summaries::get_latest_summary(pool, &session.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(latest.summary_text, "Summary 2 (later)");
}

#[tokio::test]
async fn test_get_summary_no_summary() {
    let db = setup_db().await;
    let pool = pool(&db);

    let session = nerdbot::storage::sessions::create_session(pool, 1)
        .await
        .unwrap();

    let found = nerdbot::storage::summaries::get_latest_summary(pool, &session.id)
        .await
        .unwrap();
    assert!(found.is_none());
}

// ── Job CRUD ───────────────────────────────────────────────────────────

#[tokio::test]
async fn test_create_and_get_job() {
    let db = setup_db().await;
    let pool = pool(&db);

    let stored = nerdbot::storage::jobs::create_job(
        pool,
        123,
        "Morning report".into(),
        "Summarize overnight events.".into(),
        ScheduleType::Cron,
        None,
    )
    .await
    .unwrap();

    assert_eq!(stored.owner_conversation_id, "123");
    assert_eq!(stored.name, "Morning report");
    assert!(stored.schedule_type().unwrap() == ScheduleType::Cron);

    let found = nerdbot::storage::jobs::get_job(pool, &stored.id)
        .await
        .unwrap();
    assert!(found.is_some());
    assert_eq!(found.unwrap().id, stored.id);
}

#[tokio::test]
async fn test_get_nonexistent_job() {
    let db = setup_db().await;
    let pool = pool(&db);

    let found = nerdbot::storage::jobs::get_job(pool, "nonexistent-id")
        .await
        .unwrap();
    assert!(found.is_none());
}

#[tokio::test]
async fn test_list_jobs_for_chat() {
    let db = setup_db().await;
    let pool = pool(&db);

    // Create jobs for chat 1
    nerdbot::storage::jobs::create_job(
        pool,
        1,
        "Job A".into(),
        "Prompt A".into(),
        ScheduleType::OneShot,
        None,
    )
    .await
    .unwrap();
    nerdbot::storage::jobs::create_job(
        pool,
        1,
        "Job B".into(),
        "Prompt B".into(),
        ScheduleType::Cron,
        None,
    )
    .await
    .unwrap();

    // Create a job for chat 2
    nerdbot::storage::jobs::create_job(
        pool,
        2,
        "Job C".into(),
        "Prompt C".into(),
        ScheduleType::OneShot,
        None,
    )
    .await
    .unwrap();

    let jobs1 = nerdbot::storage::jobs::list_jobs(pool, 1, true)
        .await
        .unwrap();
    assert_eq!(jobs1.len(), 2);
    let names: Vec<_> = jobs1.iter().map(|j| j.name.as_str()).collect();
    assert!(names.contains(&"Job A"));
    assert!(names.contains(&"Job B"));

    let jobs2 = nerdbot::storage::jobs::list_jobs(pool, 2, true)
        .await
        .unwrap();
    assert_eq!(jobs2.len(), 1);
    assert_eq!(jobs2[0].name, "Job C");
}

#[tokio::test]
async fn test_list_jobs_enabled_only() {
    let db = setup_db().await;
    let pool = pool(&db);

    let job = nerdbot::storage::jobs::create_job(
        pool,
        1,
        "Enabled".into(),
        "Prompt".into(),
        ScheduleType::OneShot,
        None,
    )
    .await
    .unwrap();

    // Disable the job
    nerdbot::storage::jobs::disable_job(pool, &job.id)
        .await
        .unwrap();

    let enabled_jobs = nerdbot::storage::jobs::list_jobs(pool, 1, true)
        .await
        .unwrap();
    assert!(enabled_jobs.is_empty());

    // List all jobs (not just enabled)
    let all_jobs = nerdbot::storage::jobs::list_jobs(pool, 1, false)
        .await
        .unwrap();
    assert_eq!(all_jobs.len(), 1);
    assert_eq!(all_jobs[0].name, "Enabled");
}

#[tokio::test]
async fn test_job_typed_accessors() {
    let db = setup_db().await;
    let pool = pool(&db);

    let job = nerdbot::storage::jobs::create_job(
        pool,
        42,
        "Test Job".into(),
        "Test Prompt".into(),
        ScheduleType::Cron,
        None,
    )
    .await
    .unwrap();

    assert!(matches!(job.schedule_type(), Ok(ScheduleType::Cron)));
    assert!(matches!(
        job.context_policy(),
        Ok(JobContextPolicy::IncludeCreationSnapshot)
    ));
    assert!(job.last_status().unwrap().is_none());
    assert!(job.enabled);
    assert!(!job.notify_on_completion);
}

// ── End-to-End: Session → Messages → Summary ──────────────────────────

#[tokio::test]
async fn test_session_message_summary_flow() {
    let db = setup_db().await;
    let pool = pool(&db);

    // Create a session
    let session = nerdbot::storage::sessions::create_session(pool, 999)
        .await
        .unwrap();

    // Add messages
    let user_msg = ChatMessage::user(MessageContent::from_text("What is Rust?"));
    let assistant_msg = ChatMessage::assistant(MessageContent::from_text(
        "Rust is a systems programming language.",
    ));

    let user_stored =
        nerdbot::storage::messages::create_message(pool, &session.id, &user_msg, Some(8))
            .await
            .unwrap();
    let assistant_stored =
        nerdbot::storage::messages::create_message(pool, &session.id, &assistant_msg, Some(12))
            .await
            .unwrap();

    // List messages
    let messages = nerdbot::storage::messages::list_messages(pool, &session.id, None)
        .await
        .unwrap();
    assert_eq!(messages.len(), 2);

    // Create a summary
    let summary = nerdbot::context::summaries::ContextSummary::new(
        session.id.clone(),
        "User asked about Rust. Bot explained it's a systems programming language.".into(),
        assistant_stored.id.clone(),
    );
    let stored_summary = nerdbot::storage::summaries::create_summary(pool, &summary)
        .await
        .unwrap();

    // Verify summary retrieval
    let latest = nerdbot::storage::summaries::get_latest_summary(pool, &session.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(latest.id, stored_summary.id);
    assert_eq!(latest.covers_through_message_id, assistant_stored.id);
}

// ── Error Handling ─────────────────────────────────────────────────────

#[tokio::test]
async fn test_session_create_error_handling() {
    // Should not error with valid data
    let db = setup_db().await;
    let pool = pool(&db);

    let result = nerdbot::storage::sessions::create_session(pool, 0).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_message_create_with_no_session_fails() {
    // SQLite now enforces foreign key constraints.
    // Creating a message for a nonexistent session must return a Storage error.
    let db = setup_db().await;
    let pool = pool(&db);

    let msg = ChatMessage::user(MessageContent::from_text("orphan"));
    let result =
        nerdbot::storage::messages::create_message(pool, "nonexistent-session", &msg, None).await;
    assert!(
        result.is_err(),
        "Expected an error because session does not exist"
    );
    match result {
        Err(AgentError::Storage(e)) => {
            assert!(
                e.contains("foreign key constraint failed")
                    || e.contains("FOREIGN KEY constraint failed"),
                "Expected foreign key violation error, got: {}",
                e
            );
        }
        other => panic!("Expected Storage error, got: {:?}", other),
    }
}

// ── Storage Model Serialization ───────────────────────────────────────

#[test]
fn test_stored_job_serialization_roundtrip() {
    let job = StoredJob::new(
        123,
        "Test Job".into(),
        "Test Prompt".into(),
        ScheduleType::Cron,
    );
    let json = serde_json::to_string(&job).unwrap();
    let restored: StoredJob = serde_json::from_str(&json).unwrap();
    assert_eq!(restored.id, job.id);
    assert_eq!(restored.owner_conversation_id, "123");
    assert_eq!(restored.name, "Test Job");
}

#[test]
fn test_stored_message_serialization_roundtrip() {
    let msg = StoredMessage {
        id: "m1".into(),
        chat_session_id: "session-1".into(),
        role: serde_json::Value::String("user".into()),
        content: "Hello".into(),
        structured_content_json: None,
        token_estimate: Some(5),
        created_at: chrono::Utc::now(),
    };
    let json = serde_json::to_string(&msg).unwrap();
    let restored: StoredMessage = serde_json::from_str(&json).unwrap();
    assert_eq!(restored.chat_session_id, "session-1");
    assert_eq!(restored.content, "Hello");
}

#[test]
fn test_stored_summary_serialization_roundtrip() {
    let summary = nerdbot::context::summaries::ContextSummary::new(
        "s1".into(),
        "Summary text".into(),
        "m1".into(),
    );
    let stored = StoredSummary::from(summary);
    let json = serde_json::to_string(&stored).unwrap();
    let restored: StoredSummary = serde_json::from_str(&json).unwrap();
    assert_eq!(restored.summary_text, "Summary text");
    assert_eq!(restored.chat_session_id, "s1");
}
