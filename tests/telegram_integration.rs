//! Integration tests for Telegram components: command parsing, handler,
//! service, and bot client.
//!
//! Tests cover:
//! - Command parsing edge cases
//! - Command handler responses (jobs, run, delete with DB)
//! - Message handler routing (commands vs agent loop)
//! - Allowlist enforcement
//! - Service message splitting
//! - Bot message splitting logic

#![allow(
    dead_code,
    unused,
    unused_imports,
    unused_variables,
    unused_assignments
)]

use std::path::PathBuf;
use std::sync::Arc;

use nerdbot::config::AppConfig;
use nerdbot::error::AgentError;
use nerdbot::llm::fake::{FakeProvider, FakeResponse};
use nerdbot::llm::provider::LlmProvider;
use nerdbot::storage;
use nerdbot::telegram::TelegramBot;
use nerdbot::telegram::commands::{CommandHandler, TelegramCommand};
use nerdbot::telegram::handler::MessageHandler;
use nerdbot::telegram::service::TelegramService;
use nerdbot::tools::echo::EchoTool;
use nerdbot::tools::registry::ToolRegistry;

// ── Command parsing ──────────────────────────────────────────────────

#[test]
fn test_parse_commands_with_leading_whitespace() {
    assert_eq!(
        TelegramCommand::parse("  /help"),
        Some(TelegramCommand::Help)
    );
    assert_eq!(
        TelegramCommand::parse("\t/start"),
        Some(TelegramCommand::Start)
    );
}

#[test]
fn test_parse_commands_case_sensitive() {
    // Commands are case-sensitive, /HELP is not recognized
    assert_eq!(TelegramCommand::parse("/HELP"), None);
}

#[test]
fn test_parse_run_with_multiple_spaces() {
    assert_eq!(
        TelegramCommand::parse("/run   abc123  "),
        Some(TelegramCommand::Run("abc123".into()))
    );
}

#[test]
fn test_parse_delete_with_multiple_spaces() {
    assert_eq!(
        TelegramCommand::parse("/delete   xyz  "),
        Some(TelegramCommand::Delete("xyz".into()))
    );
}

#[test]
fn test_parse_run_with_bot_mention_no_arg_is_none() {
    assert_eq!(TelegramCommand::parse("/run@NerdBot"), None);
    assert_eq!(TelegramCommand::parse("/delete@NerdBot"), None);
}

#[test]
fn test_parse_run_with_bot_mention_empty_arg() {
    // /run@NerdBot followed by space but no text
    assert_eq!(
        TelegramCommand::parse("/run@NerdBot "),
        Some(TelegramCommand::Run("".into()))
    );
}

#[test]
fn test_parse_new_topic() {
    assert_eq!(
        TelegramCommand::parse("/new-topic"),
        Some(TelegramCommand::ResetContext)
    );
}

// ── Command handler ──────────────────────────────────────────────────

/// Set up an in-memory SQLite database for tests.
async fn setup_test_db() -> sqlx::SqlitePool {
    let pool = sqlx::SqlitePool::connect("sqlite::memory:")
        .await
        .expect("failed to create in-memory DB");

    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("failed to run migrations");

    pool
}

#[tokio::test]
async fn test_command_handler_start() {
    let pool = setup_test_db().await;
    let response = CommandHandler::handle(TelegramCommand::Start, 1, 1, &pool)
        .await
        .unwrap();
    assert!(response.contains("Welcome"));
    assert!(response.contains("/help"));
}

#[tokio::test]
async fn test_command_handler_help() {
    let pool = setup_test_db().await;
    let response = CommandHandler::handle(TelegramCommand::Help, 1, 1, &pool)
        .await
        .unwrap();
    assert!(response.contains("/start"));
    assert!(response.contains("/help"));
    assert!(response.contains("/jobs"));
    assert!(response.contains("/reset-context"));
}

#[tokio::test]
async fn test_command_handler_jobs_empty() {
    let pool = setup_test_db().await;
    let response = CommandHandler::handle(TelegramCommand::Jobs, 1, 1, &pool)
        .await
        .unwrap();
    assert!(response.contains("no scheduled jobs"));
}

#[tokio::test]
async fn test_command_handler_jobs_with_job() {
    let pool = setup_test_db().await;

    // Create a test job
    storage::jobs::create_job(
        &pool,
        1,
        "Test Job".into(),
        "test prompt".into(),
        nerdbot::scheduler::models::ScheduleType::OneShot,
        None,
    )
    .await
    .unwrap();

    let response = CommandHandler::handle(TelegramCommand::Jobs, 1, 1, &pool)
        .await
        .unwrap();
    assert!(response.contains("Test Job"));
    assert!(!response.contains("no scheduled jobs"));
}

#[tokio::test]
async fn test_command_handler_jobs_only_own_chat() {
    let pool = setup_test_db().await;

    // Create a job for chat 1
    storage::jobs::create_job(
        &pool,
        1,
        "Chat 1 Job".into(),
        "prompt".into(),
        nerdbot::scheduler::models::ScheduleType::OneShot,
        None,
    )
    .await
    .unwrap();

    // Chat 2 sees no jobs
    let response = CommandHandler::handle(TelegramCommand::Jobs, 2, 2, &pool)
        .await
        .unwrap();
    assert!(response.contains("no scheduled jobs"));
}

#[tokio::test]
async fn test_command_handler_delete_job() {
    let pool = setup_test_db().await;

    let job = storage::jobs::create_job(
        &pool,
        1,
        "Delete Me".into(),
        "prompt".into(),
        nerdbot::scheduler::models::ScheduleType::OneShot,
        None,
    )
    .await
    .unwrap();

    let response = CommandHandler::handle(TelegramCommand::Delete(job.id.clone()), 1, 1, &pool)
        .await
        .unwrap();
    assert!(response.contains("deleted"));

    // Verify job is disabled
    let job_after = storage::jobs::get_job(&pool, &job.id).await.unwrap();
    assert!(!job_after.unwrap().enabled);
}

#[tokio::test]
async fn test_command_handler_delete_nonexistent_job() {
    let pool = setup_test_db().await;

    let response =
        CommandHandler::handle(TelegramCommand::Delete("nonexistent".into()), 1, 1, &pool)
            .await
            .unwrap();
    assert!(response.contains("No job found"));
}

#[tokio::test]
async fn test_command_handler_delete_wrong_chat() {
    let pool = setup_test_db().await;

    let job = storage::jobs::create_job(
        &pool,
        1, // Owned by chat 1
        "Chat 1 Job".into(),
        "prompt".into(),
        nerdbot::scheduler::models::ScheduleType::OneShot,
        None,
    )
    .await
    .unwrap();

    // Chat 2 tries to delete it
    let response = CommandHandler::handle(TelegramCommand::Delete(job.id.clone()), 2, 2, &pool)
        .await
        .unwrap();
    assert!(response.contains("belongs to a different chat"));
}

#[tokio::test]
async fn test_command_handler_run_nonexistent_job() {
    let pool = setup_test_db().await;

    let response = CommandHandler::handle(TelegramCommand::Run("nonexistent".into()), 1, 1, &pool)
        .await
        .unwrap();
    assert!(response.contains("No job found"));
}

#[tokio::test]
async fn test_command_handler_reset_context() {
    let pool = setup_test_db().await;
    let response = CommandHandler::handle(TelegramCommand::ResetContext, 1, 1, &pool)
        .await
        .unwrap();
    assert!(response.contains("fresh conversation"));
}

// ── Message handler ──────────────────────────────────────────────────

fn make_test_config() -> AppConfig {
    AppConfig::default()
}

fn make_handler(pool: sqlx::SqlitePool) -> MessageHandler {
    let mut registry = ToolRegistry::new();
    registry.register(EchoTool);

    let provider: Arc<dyn LlmProvider> =
        Arc::new(FakeProvider::new(vec![FakeResponse::final_text(
            "Hello from the agent loop!",
        )]));

    MessageHandler::new(pool, provider, Arc::new(registry), make_test_config())
}

#[tokio::test]
async fn test_message_handler_routes_commands() {
    let pool = setup_test_db().await;
    let handler = make_handler(pool);

    // Commands should be handled without running the agent loop
    let response = handler.handle_message(1, 1, "/help").await.unwrap();
    assert!(response.is_some());
    assert!(response.unwrap().contains("/start"));

    let response = handler.handle_message(1, 1, "/start").await.unwrap();
    assert!(response.is_some());
    assert!(response.unwrap().contains("Welcome"));
}

#[tokio::test]
async fn test_message_handler_runs_agent_for_normal_text() {
    let pool = setup_test_db().await;
    let handler = make_handler(pool);

    // Regular text should go through the agent loop
    let response = handler
        .handle_message(1, 1, "Hello, how are you?")
        .await
        .unwrap();
    assert!(response.is_some());
    assert!(response.unwrap().contains("agent loop"));
}

#[tokio::test]
async fn test_message_handler_reset_context_via_text() {
    let pool = setup_test_db().await;
    let handler = make_handler(pool);

    // "reset context" (without slash) should be treated as a command
    let response = handler.handle_message(1, 1, "reset context").await.unwrap();
    assert!(response.is_some());
    assert!(response.unwrap().contains("fresh conversation"));
}

#[tokio::test]
async fn test_message_handler_creates_session() {
    let pool = setup_test_db().await;
    let handler = make_handler(pool.clone());

    let _ = handler
        .handle_message(42, 100, "first message")
        .await
        .unwrap();

    // Verify a session was created
    let session = storage::sessions::get_session_for_chat(&pool, 42)
        .await
        .unwrap();
    assert!(session.is_some());
}

#[tokio::test]
async fn test_message_handler_persists_messages() {
    let pool = setup_test_db().await;
    let handler = make_handler(pool.clone());

    let _ = handler
        .handle_message(99, 200, "persist this")
        .await
        .unwrap();

    let session = storage::sessions::get_session_for_chat(&pool, 99)
        .await
        .unwrap()
        .unwrap();

    let messages = storage::messages::list_messages(&pool, &session.id, None)
        .await
        .unwrap();
    assert!(!messages.is_empty());
}

#[tokio::test]
async fn test_message_handler_allowlist_blocks_chat() {
    let pool = setup_test_db().await;

    let mut config = make_test_config();
    config.telegram.allowed_chat_ids = vec![100]; // Only chat 100 allowed

    let mut registry = ToolRegistry::new();
    registry.register(EchoTool);

    let provider: Arc<dyn LlmProvider> =
        Arc::new(FakeProvider::new(vec![FakeResponse::final_text(
            "should not be reached",
        )]));

    let handler = MessageHandler::new(pool, provider, Arc::new(registry), config);

    // Chat 200 is not in the allowlist
    let result = handler.handle_message(200, 200, "blocked").await;
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), AgentError::PermissionDenied));
}

#[tokio::test]
async fn test_message_handler_allowlist_blocks_user() {
    let pool = setup_test_db().await;

    let mut config = make_test_config();
    config.telegram.allowed_user_ids = vec![100]; // Only user 100 allowed

    let mut registry = ToolRegistry::new();
    registry.register(EchoTool);

    let provider: Arc<dyn LlmProvider> =
        Arc::new(FakeProvider::new(vec![FakeResponse::final_text(
            "should not be reached",
        )]));

    let handler = MessageHandler::new(pool, provider, Arc::new(registry), config);

    // User 200 is not in the allowlist
    let result = handler.handle_message(1, 200, "blocked").await;
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), AgentError::PermissionDenied));
}

#[tokio::test]
async fn test_message_handler_allows_when_no_restrictions() {
    let pool = setup_test_db().await;
    let handler = make_handler(pool);

    // Empty allowlists mean all users/chats are allowed
    let response = handler.handle_message(999, 888, "hello").await;
    assert!(response.is_ok());
}

// ── Service: message splitting ───────────────────────────────────────

#[test]
fn test_service_does_not_split_short_messages() {
    let short = "hello world";
    let chunks = TelegramBot::split_long_message(short);
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0], short);
}

#[test]
fn test_service_splits_long_messages() {
    let long = "x".repeat(5000);
    let chunks = TelegramBot::split_long_message(&long);
    assert!(chunks.len() > 1);
    // Total chars across all chunks should equal original
    let total: usize = chunks.iter().map(|c| c.chars().count()).sum();
    assert_eq!(total, 5000);
}

#[test]
fn test_service_splits_on_space_preference() {
    // Create a string with a space right at the boundary
    let prefix = "x".repeat(3900);
    let suffix = "y".repeat(500);
    let text = format!("{prefix} {suffix}");
    let chunks = TelegramBot::split_long_message(&text);
    assert!(chunks.len() == 2);
    // First chunk should end with the space
    assert!(chunks[0].ends_with(' '));
}

// ── TelegramBot helpers ──────────────────────────────────────────────

#[test]
fn test_bot_new_sets_token() {
    let bot = TelegramBot::new("my-test-token".into());
    assert_eq!(bot.token(), "my-test-token");
}

// ── TelegramBot HTTP status checks ───────────────────────────────────

use wiremock::{Mock, MockServer, ResponseTemplate};

/// Create a TelegramBot pointing at the given mock server.
async fn make_mock_bot(server: &MockServer) -> TelegramBot {
    let base_url = format!("{}/bottest", server.uri());
    TelegramBot::new_with_base_url("test-token".into(), base_url)
}

/// Build the expected path for a Telegram API endpoint.
/// The mock bot uses base URL `http://host:port/bottest`, so
/// the code appends `/getMe` → full path `/bottest/getMe`.
fn mock_path(endpoint: &str) -> String {
    format!("/bottest{endpoint}")
}

#[tokio::test]
async fn test_get_me_returns_error_on_404() {
    let server = MockServer::start().await;
    let bot = make_mock_bot(&server).await;

    Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path(&mock_path("/getMe")))
        .respond_with(ResponseTemplate::new(404).set_body_string(r#"{"ok":false,"description":"Not Found"}"#))
        .mount(&server)
        .await;

    let result = bot.get_me().await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    let err_str = err.to_string();
    assert!(err_str.contains("getMe returned HTTP 404"), "unexpected error: {err_str}");
}

#[tokio::test]
async fn test_get_me_returns_error_on_401() {
    let server = MockServer::start().await;
    let bot = make_mock_bot(&server).await;

    Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path(&mock_path("/getMe")))
        .respond_with(ResponseTemplate::new(401).set_body_string(r#"{"ok":false,"description":"Unauthorized"}"#))
        .mount(&server)
        .await;

    let result = bot.get_me().await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    let err_str = err.to_string();
    assert!(err_str.contains("getMe returned HTTP 401"), "unexpected error: {err_str}");
}

#[tokio::test]
async fn test_get_me_succeeds_on_200() {
    let server = MockServer::start().await;
    let bot = make_mock_bot(&server).await;

    let response_body = r#"{"ok":true,"result":{"id":12345,"first_name":"TestBot","username":"@testbot"}}"#;
    Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path(&mock_path("/getMe")))
        .respond_with(ResponseTemplate::new(200).set_body_string(response_body))
        .mount(&server)
        .await;

    let result = bot.get_me().await.unwrap();
    assert_eq!(result.id, 12345);
    assert_eq!(result.first_name, "TestBot");
    assert_eq!(result.username, Some("@testbot".into()));
}

#[tokio::test]
async fn test_delete_webhook_returns_error_on_500() {
    let server = MockServer::start().await;
    let bot = make_mock_bot(&server).await;

    Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path(&mock_path("/deleteWebhook")))
        .respond_with(ResponseTemplate::new(500).set_body_string(r#"{"ok":false,"description":"Internal Server Error"}"#))
        .mount(&server)
        .await;

    let result = bot.delete_webhook().await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    let err_str = err.to_string();
    assert!(err_str.contains("deleteWebhook returned HTTP 500"), "unexpected error: {err_str}");
}

#[tokio::test]
async fn test_delete_webhook_returns_error_on_404() {
    let server = MockServer::start().await;
    let bot = make_mock_bot(&server).await;

    Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path(&mock_path("/deleteWebhook")))
        .respond_with(ResponseTemplate::new(404).set_body_string(r#"{"ok":false,"description":"Not Found"}"#))
        .mount(&server)
        .await;

    let result = bot.delete_webhook().await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    let err_str = err.to_string();
    assert!(err_str.contains("deleteWebhook returned HTTP 404"), "unexpected error: {err_str}");
}

#[tokio::test]
async fn test_get_updates_returns_error_on_503() {
    let server = MockServer::start().await;
    let bot = make_mock_bot(&server).await;

    Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path(&mock_path("/getUpdates")))
        .respond_with(ResponseTemplate::new(503).set_body_string(r#"{"ok":false,"description":"Service Unavailable"}"#))
        .mount(&server)
        .await;

    let result = bot.get_updates(None, 0).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    let err_str = err.to_string();
    assert!(err_str.contains("getUpdates returned HTTP 503"), "unexpected error: {err_str}");
}

// ── Tool context ──────────────────────────────────────────────────────

#[test]
fn test_tool_context_carries_telegram_config() {
    let ctx = nerdbot::tools::traits::ToolContext {
        run_mode: nerdbot::agent::run_mode::AgentRunMode::InteractiveReply {
            chat_id: 1,
            user_id: 2,
        },
        workspace_root: PathBuf::from("/tmp"),
        telegram_token: "bot123".into(),
        allowed_chat_ids: vec![1, 2],
        allowed_user_ids: vec![10, 20],
    };

    assert_eq!(ctx.telegram_token, "bot123");
    assert_eq!(ctx.allowed_chat_ids, vec![1, 2]);
    assert_eq!(ctx.allowed_user_ids, vec![10, 20]);
}
