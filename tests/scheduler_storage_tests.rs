#![allow(
    dead_code,
    unused,
    unused_imports,
    unused_variables,
    unused_assignments
)]
//! Tests for scheduler models, storage models, and Telegram commands.

use chrono::Utc;

use genai::chat::ChatRole;
use nerdbot::scheduler::models::{JobContextPolicy, JobStatus, ScheduleType};
use nerdbot::storage::jobs::StoredJob;
use nerdbot::storage::messages::StoredMessage;
use nerdbot::storage::sessions::ChatSession;
use nerdbot::storage::summaries::StoredSummary;
use nerdbot::telegram::commands::TelegramCommand;

fn stored_message(
    session_id: String,
    role: ChatRole,
    content: String,
    token_estimate: Option<i64>,
) -> StoredMessage {
    let role_str = match role {
        ChatRole::System => "system",
        ChatRole::User => "user",
        ChatRole::Assistant => "assistant",
        ChatRole::Tool => "tool",
    };
    StoredMessage {
        id: uuid::Uuid::new_v4().to_string(),
        chat_session_id: session_id.into(),
        role: serde_json::Value::String(role_str.to_string()),
        content: content.into(),
        structured_content_json: None,
        token_estimate,
        created_at: Utc::now(),
    }
}

// ── JobContextPolicy ─────────────────────────────────────────────────────

#[test]
fn test_job_context_policy_isolated() {
    let policy = JobContextPolicy::Isolated;
    assert!(matches!(policy, JobContextPolicy::Isolated));
}

#[test]
fn test_job_context_policy_include_creation_snapshot() {
    let policy = JobContextPolicy::IncludeCreationSnapshot;
    assert!(matches!(policy, JobContextPolicy::IncludeCreationSnapshot));
}

#[test]
fn test_job_context_policy_include_chat_summary() {
    let policy = JobContextPolicy::IncludeChatSummary;
    assert!(matches!(policy, JobContextPolicy::IncludeChatSummary));
}

#[test]
fn test_job_context_policy_default() {
    let policy = JobContextPolicy::default();
    assert!(matches!(policy, JobContextPolicy::IncludeCreationSnapshot));
}

#[test]
fn test_job_context_policy_serialization() {
    assert_eq!(
        serde_json::to_string(&JobContextPolicy::Isolated).unwrap(),
        "\"isolated\""
    );
    assert_eq!(
        serde_json::to_string(&JobContextPolicy::IncludeCreationSnapshot).unwrap(),
        "\"include_creation_snapshot\""
    );
    assert_eq!(
        serde_json::to_string(&JobContextPolicy::IncludeChatSummary).unwrap(),
        "\"include_chat_summary\""
    );
}

#[test]
fn test_job_context_policy_roundtrip() {
    for policy in [
        JobContextPolicy::Isolated,
        JobContextPolicy::IncludeCreationSnapshot,
        JobContextPolicy::IncludeChatSummary,
    ] {
        let json = serde_json::to_string(&policy).unwrap();
        let restored: JobContextPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(policy, restored);
    }
}

#[test]
fn test_job_context_policy_clone() {
    let policy = JobContextPolicy::IncludeChatSummary;
    let cloned = policy.clone();
    assert_eq!(policy, cloned);
}

// ── ScheduleType ─────────────────────────────────────────────────────────

#[test]
fn test_schedule_type_one_shot() {
    assert_eq!(
        serde_json::to_string(&ScheduleType::OneShot).unwrap(),
        "\"one_shot\""
    );
}

#[test]
fn test_schedule_type_cron() {
    assert_eq!(
        serde_json::to_string(&ScheduleType::Cron).unwrap(),
        "\"cron\""
    );
}

#[test]
fn test_schedule_type_roundtrip() {
    let st = ScheduleType::Cron;
    let json = serde_json::to_string(&st).unwrap();
    let restored: ScheduleType = serde_json::from_str(&json).unwrap();
    assert_eq!(st, restored);
}

// ── JobStatus ────────────────────────────────────────────────────────────

#[test]
fn test_job_status_serialization() {
    assert_eq!(
        serde_json::to_string(&JobStatus::Pending).unwrap(),
        "\"pending\""
    );
    assert_eq!(
        serde_json::to_string(&JobStatus::Running).unwrap(),
        "\"running\""
    );
    assert_eq!(
        serde_json::to_string(&JobStatus::Success).unwrap(),
        "\"success\""
    );
    assert_eq!(
        serde_json::to_string(&JobStatus::Failed).unwrap(),
        "\"failed\""
    );
    assert_eq!(
        serde_json::to_string(&JobStatus::Missed).unwrap(),
        "\"missed\""
    );
}

// ── ChatSession ──────────────────────────────────────────────────────────

#[test]
fn test_chat_session_new() {
    let session = ChatSession::new(123_456_789i64);
    assert!(!session.id.is_empty());
    assert_eq!(session.telegram_chat_id, 123_456_789i64);
    assert!(session.created_at <= Utc::now());
    assert_eq!(session.created_at, session.updated_at);
}

#[test]
fn test_chat_session_unique_ids() {
    let s1 = ChatSession::new(111);
    let s2 = ChatSession::new(111);
    assert_ne!(s1.id, s2.id);
}

#[test]
fn test_chat_session_clone() {
    let session = ChatSession::new(42);
    let cloned = session.clone();
    assert_eq!(session.telegram_chat_id, cloned.telegram_chat_id);
    assert_eq!(session.id, cloned.id);
}

// ── StoredMessage ────────────────────────────────────────────────────────

#[test]
fn test_stored_message_new_text() {
    let msg = stored_message(
        "session-1".into(),
        ChatRole::User,
        "Hello, agent!".into(),
        Some(25),
    );
    assert!(!msg.id.is_empty());
    assert_eq!(msg.chat_session_id, "session-1");
    assert!(matches!(msg.role(), Ok(ChatRole::User)));
    assert_eq!(msg.content, "Hello, agent!");
    assert_eq!(msg.token_estimate, Some(25));
    assert!(msg.structured_content_json.is_none());
}

#[test]
fn test_stored_message_new_no_estimate() {
    let msg = stored_message(
        "session-1".into(),
        ChatRole::Assistant,
        "I can help with that.".into(),
        None,
    );
    assert_eq!(msg.token_estimate, None);
}

#[test]
fn test_stored_message_structured_content() {
    let mut msg = stored_message(
        "session-1".into(),
        ChatRole::Assistant,
        "tool call".into(),
        Some(50),
    );
    msg.structured_content_json = Some(serde_json::json!({"tool":"read_file"}));
    assert_eq!(
        msg.structured_content_json,
        Some(serde_json::json!({"tool":"read_file"}))
    );
}

#[test]
fn test_stored_message_clone() {
    let msg = stored_message("s1".into(), ChatRole::User, "test".into(), Some(10));
    let cloned = msg.clone();
    assert_eq!(msg.content, cloned.content);
    assert_eq!(msg.chat_session_id, cloned.chat_session_id);
    assert!(matches!(
        (msg.role(), cloned.role()),
        (Ok(ChatRole::User), Ok(ChatRole::User))
    ));
}

#[test]
fn test_stored_message_serialization() {
    let msg = stored_message("s1".into(), ChatRole::User, "hi".into(), Some(5));
    let json = serde_json::to_string(&msg).unwrap();
    assert!(json.contains("s1"));
    assert!(json.contains("hi"));
    assert!(json.contains("user"));
}

#[test]
fn test_stored_message_role_deserialization() {
    let msg = stored_message("s1".into(), ChatRole::Assistant, "test".into(), Some(5));
    assert!(matches!(msg.role(), Ok(ChatRole::Assistant)));
}

// ── StoredJob ────────────────────────────────────────────────────────────

#[test]
fn test_stored_job_new_oneshot() {
    let job = StoredJob::new(
        123_456_789i64,
        "morning report".into(),
        "Summarize today's news.".into(),
        ScheduleType::OneShot,
    );
    assert!(!job.id.is_empty());
    assert_eq!(job.owner_chat_id, 123_456_789i64);
    assert_eq!(job.name, "morning report");
    assert!(matches!(job.schedule_type(), Ok(ScheduleType::OneShot)));
    assert!(job.enabled);
    assert!(!job.notify_on_completion);
    assert!(job.last_run_at.is_none());
    assert!(job.next_run_at.is_none());
    assert!(job.last_status.is_none());
}

#[test]
fn test_stored_job_unique_ids() {
    let j1 = StoredJob::new(1, "a".into(), "b".into(), ScheduleType::OneShot);
    let j2 = StoredJob::new(1, "a".into(), "b".into(), ScheduleType::OneShot);
    assert_ne!(j1.id, j2.id);
}

#[test]
fn test_stored_job_schedule_type_roundtrip() {
    let job = StoredJob::new(1, "j".into(), "p".into(), ScheduleType::Cron);
    assert!(matches!(job.schedule_type(), Ok(ScheduleType::Cron)));
}

#[test]
fn test_stored_job_created_updated_times() {
    let job = StoredJob::new(1, "job".into(), "prompt".into(), ScheduleType::OneShot);
    assert!(job.created_at <= Utc::now());
    assert_eq!(job.created_at, job.updated_at);
}

#[test]
fn test_stored_job_clone() {
    let job = StoredJob::new(1, "j".into(), "p".into(), ScheduleType::Cron);
    let cloned = job.clone();
    assert_eq!(job.name, cloned.name);
    assert!(matches!(
        (job.schedule_type(), cloned.schedule_type()),
        (Ok(ScheduleType::Cron), Ok(ScheduleType::Cron))
    ));
    assert_eq!(job.owner_chat_id, cloned.owner_chat_id);
}

#[test]
fn test_stored_job_serialization() {
    let job = StoredJob::new(42, "report".into(), "summarize".into(), ScheduleType::Cron);
    let json = serde_json::to_string(&job).unwrap();
    assert!(json.contains("report"));
    assert!(json.contains("summarize"));
    assert!(json.contains("cron"));
}

#[test]
fn test_stored_job_context_policy_default() {
    let job = StoredJob::new(1, "j".into(), "p".into(), ScheduleType::OneShot);
    assert!(matches!(
        job.context_policy(),
        Ok(JobContextPolicy::IncludeCreationSnapshot)
    ));
}

// ── StoredSummary ────────────────────────────────────────────────────────

#[test]
fn test_stored_summary_from_context_summary() {
    let cs = nerdbot::context::summaries::ContextSummary::new(
        "sess-1".into(),
        "Old conversation was about Rust.".into(),
        "msg-50".into(),
    );
    let ss = StoredSummary::from(cs);
    assert_eq!(ss.chat_session_id, "sess-1");
    assert_eq!(ss.summary_text, "Old conversation was about Rust.");
    assert_eq!(ss.covers_through_message_id, "msg-50");
}

#[test]
fn test_stored_summary_serialization() {
    let cs = nerdbot::context::summaries::ContextSummary::new(
        "s1".into(),
        "Summary here.".into(),
        "m1".into(),
    );
    let ss = StoredSummary::from(cs);
    let json = serde_json::to_string(&ss).unwrap();
    assert!(json.contains("Summary here."));
    assert!(json.contains("s1"));
}

// ── TelegramCommand ──────────────────────────────────────────────────────

#[test]
fn test_telegram_command_parse_start() {
    assert_eq!(
        TelegramCommand::parse("/start"),
        Some(TelegramCommand::Start)
    );
}

#[test]
fn test_telegram_command_parse_help() {
    assert_eq!(TelegramCommand::parse("/help"), Some(TelegramCommand::Help));
}

#[test]
fn test_telegram_command_parse_jobs() {
    assert_eq!(TelegramCommand::parse("/jobs"), Some(TelegramCommand::Jobs));
}

#[test]
fn test_telegram_command_parse_run_with_id() {
    assert_eq!(
        TelegramCommand::parse("/run job-123"),
        Some(TelegramCommand::Run("job-123".into()))
    );
}

#[test]
fn test_telegram_command_parse_delete_with_id() {
    assert_eq!(
        TelegramCommand::parse("/delete job-456"),
        Some(TelegramCommand::Delete("job-456".into()))
    );
}

#[test]
fn test_telegram_command_parse_reset_context() {
    assert_eq!(
        TelegramCommand::parse("/reset-context"),
        Some(TelegramCommand::ResetContext)
    );
}

#[test]
fn test_telegram_command_parse_whitespace() {
    assert_eq!(
        TelegramCommand::parse("  /jobs  "),
        Some(TelegramCommand::Jobs)
    );
}

#[test]
fn test_telegram_command_parse_no_match() {
    assert_eq!(TelegramCommand::parse("/unknown"), None);
    assert_eq!(TelegramCommand::parse("hello"), None);
    assert_eq!(TelegramCommand::parse(""), None);
    assert_eq!(TelegramCommand::parse("/run"), None); // missing arg
    assert_eq!(TelegramCommand::parse("/delete"), None); // missing arg
}

#[test]
fn test_telegram_command_parse_run_empty_arg() {
    assert_eq!(
        TelegramCommand::parse("/run "),
        Some(TelegramCommand::Run("".into()))
    );
}

#[test]
fn test_telegram_command_parse_delete_empty_arg() {
    assert_eq!(
        TelegramCommand::parse("/delete "),
        Some(TelegramCommand::Delete("".into()))
    );
}

#[test]
fn test_telegram_command_parse_complex_id() {
    assert_eq!(
        TelegramCommand::parse("/run weekly-report-daily"),
        Some(TelegramCommand::Run("weekly-report-daily".into()))
    );
}

#[test]
fn test_telegram_command_equality() {
    assert_eq!(TelegramCommand::Start, TelegramCommand::Start);
    assert_eq!(TelegramCommand::Help, TelegramCommand::Help);
    assert_ne!(TelegramCommand::Start, TelegramCommand::Help);
}

#[test]
fn test_telegram_command_run_neq_delete() {
    let run = TelegramCommand::Run("j1".into());
    let delete = TelegramCommand::Delete("j1".into());
    assert_ne!(run, delete);
}

#[test]
fn test_telegram_command_run_different_ids() {
    let r1 = TelegramCommand::Run("a".into());
    let r2 = TelegramCommand::Run("b".into());
    assert_ne!(r1, r2);
}

// ── LLM Type Round-trips (storage context) ───────────────────────────────

#[test]
fn test_role_roundtrip() {
    for role in [ChatRole::System, ChatRole::User, ChatRole::Assistant, ChatRole::Tool] {
        let json = serde_json::to_string(&role).unwrap();
        let restored: ChatRole = serde_json::from_str(&json).unwrap();
        assert_eq!(role, restored);
    }
}

#[test]
fn test_tool_response_content_encodes_error_status() {
    let content = serde_json::json!({"success": false, "error": "fail"}).to_string();
    let response = genai::chat::ToolResponse::new("call-1", content.clone());
    assert_eq!(response.content, content);
}
