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

use nerdbot::agent::personality::Personality;
use nerdbot::config::AppConfig;
use nerdbot::context::budget::ContextBudget;
use nerdbot::context::compaction_service::CompactionService;
use nerdbot::context::compaction_worker::CompactionWorker;
use nerdbot::error::AgentError;
use nerdbot::llm::LlmExecutor;
use nerdbot::llm::fake::{FakeProvider, FakeResponse};
use nerdbot::storage;
use nerdbot::telegram::TelegramBot;
use nerdbot::telegram::bot::BotCommand;
use nerdbot::telegram::commands::{CommandHandler, TelegramCommand};
use nerdbot::telegram::handler::{MessageHandler, MessageHandlerInput};
use nerdbot::telegram::service::TelegramService;
use nerdbot::tools::echo::EchoTool;
use nerdbot::tools::registry::ToolRegistry;

fn addr(chat_id: i64) -> nerdbot::channel::ConversationAddress {
    nerdbot::channel::ConversationAddress::telegram_chat(chat_id)
}

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
fn test_parse_telegram_menu_safe_aliases() {
    assert_eq!(
        TelegramCommand::parse("/reset_context"),
        Some(TelegramCommand::ResetContext)
    );
    assert_eq!(
        TelegramCommand::parse("/new_topic"),
        Some(TelegramCommand::ResetContext)
    );
}

#[test]
fn test_parse_hyphenated_reset_commands_are_not_supported() {
    assert_eq!(TelegramCommand::parse("/reset-context"), None);
    assert_eq!(TelegramCommand::parse("/new-topic"), None);
}

#[test]
fn test_parse_run_with_bot_mention_empty_arg() {
    // /run@NerdBot followed by space but no text
    assert_eq!(
        TelegramCommand::parse("/run@NerdBot "),
        Some(TelegramCommand::Run("".into()))
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
    let response = CommandHandler::handle(TelegramCommand::Start, &addr(1), &pool, None)
        .await
        .unwrap();
    assert!(response.contains("Welcome"));
    assert!(response.contains("/help"));
}

#[tokio::test]
async fn test_command_handler_help() {
    let pool = setup_test_db().await;
    let response = CommandHandler::handle(TelegramCommand::Help, &addr(1), &pool, None)
        .await
        .unwrap();
    assert!(response.contains("/start"));
    assert!(response.contains("/help"));
    assert!(response.contains("/jobs"));
    assert!(response.contains("/reset_context"));
}

#[tokio::test]
async fn test_command_handler_jobs_empty() {
    let pool = setup_test_db().await;
    let response = CommandHandler::handle(TelegramCommand::Jobs, &addr(1), &pool, None)
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

    let response = CommandHandler::handle(TelegramCommand::Jobs, &addr(1), &pool, None)
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
    let response = CommandHandler::handle(TelegramCommand::Jobs, &addr(2), &pool, None)
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

    let response = CommandHandler::handle(
        TelegramCommand::Delete(job.id.clone()),
        &addr(1),
        &pool,
        None,
    )
    .await
    .unwrap();
    assert!(response.contains("deleted"));

    // Verify job is disabled
    let job_after = storage::jobs::get_job(&pool, &job.id).await.unwrap();
    assert!(!job_after.unwrap().enabled);
}

#[tokio::test]
async fn test_command_handler_jobs_hides_deleted_jobs() {
    let pool = setup_test_db().await;

    let deleted_job = storage::jobs::create_job(
        &pool,
        1,
        "Deleted Job".into(),
        "prompt".into(),
        nerdbot::scheduler::models::ScheduleType::OneShot,
        None,
    )
    .await
    .unwrap();
    storage::jobs::create_job(
        &pool,
        1,
        "Active Job".into(),
        "prompt".into(),
        nerdbot::scheduler::models::ScheduleType::OneShot,
        None,
    )
    .await
    .unwrap();

    CommandHandler::handle(
        TelegramCommand::Delete(deleted_job.id.clone()),
        &addr(1),
        &pool,
        None,
    )
    .await
    .unwrap();

    let response = CommandHandler::handle(TelegramCommand::Jobs, &addr(1), &pool, None)
        .await
        .unwrap();
    assert!(response.contains("Active Job"));
    assert!(!response.contains("Deleted Job"));
}

#[tokio::test]
async fn test_command_handler_delete_nonexistent_job() {
    let pool = setup_test_db().await;

    let response = CommandHandler::handle(
        TelegramCommand::Delete("nonexistent".into()),
        &addr(1),
        &pool,
        None,
    )
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
    let response = CommandHandler::handle(
        TelegramCommand::Delete(job.id.clone()),
        &addr(2),
        &pool,
        None,
    )
    .await
    .unwrap();
    assert!(response.contains("belongs to a different conversation"));
}

#[tokio::test]
async fn test_command_handler_run_nonexistent_job() {
    let pool = setup_test_db().await;

    let response = CommandHandler::handle(
        TelegramCommand::Run("nonexistent".into()),
        &addr(1),
        &pool,
        None,
    )
    .await
    .unwrap();
    assert!(response.contains("No job found"));
}

#[tokio::test]
async fn test_command_handler_reset_context() {
    let pool = setup_test_db().await;
    let response = CommandHandler::handle(TelegramCommand::ResetContext, &addr(1), &pool, None)
        .await
        .unwrap();
    assert!(response.contains("fresh conversation"));
}

// ── Message handler ──────────────────────────────────────────────────

fn make_test_config() -> AppConfig {
    AppConfig::default()
}

fn make_compaction_service(pool: sqlx::SqlitePool) -> Arc<CompactionService> {
    let llm = Arc::new(FakeProvider::new(vec![FakeResponse::final_text(
        "# Summary",
    )]));
    Arc::new(CompactionService::new(
        pool,
        Arc::new(CompactionWorker::new(llm, "fake-model".into(), 0.0)),
        ContextBudget::default(),
        30,
    ))
}

fn make_handler(pool: sqlx::SqlitePool) -> MessageHandler {
    let mut registry = ToolRegistry::new();
    registry.register(EchoTool);

    let provider: Arc<dyn LlmExecutor> =
        Arc::new(FakeProvider::new(vec![FakeResponse::final_text(
            "Hello from the agent loop!",
        )]));
    let compaction_service = make_compaction_service(pool.clone());

    let config = make_test_config();
    MessageHandler::new(MessageHandlerInput {
        pool,
        llm: provider,
        registry: Arc::new(registry),
        config: config.clone(),
        personality: Personality::from_config(&config),
        scheduler_notifier: None,
        telegram_service: None,
        compaction_service,
    })
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
async fn test_message_handler_active_reset_context() {
    let pool = setup_test_db().await;
    let handler = make_handler(pool.clone());

    // 1. Send first message to initialize session
    let _ = handler.handle_message(42, 100, "hello").await.unwrap();

    let session1 = storage::sessions::get_session_for_chat(&pool, 42)
        .await
        .unwrap()
        .unwrap();

    // 2. Call reset context
    let _ = handler
        .handle_message(42, 100, "/reset_context")
        .await
        .unwrap();

    // 3. Verify a new session has been created for the chat
    let session2 = storage::sessions::get_session_for_chat(&pool, 42)
        .await
        .unwrap()
        .unwrap();

    assert_ne!(
        session1.id, session2.id,
        "Expected a new session ID to be generated"
    );

    // 4. Verify that subsequent message goes to the new session
    let _ = handler
        .handle_message(42, 100, "new conversation starting")
        .await
        .unwrap();

    let messages1 = storage::messages::list_messages(&pool, &session1.id, None)
        .await
        .unwrap();
    let messages2 = storage::messages::list_messages(&pool, &session2.id, None)
        .await
        .unwrap();

    assert!(
        messages2
            .iter()
            .any(|m| m.content == "new conversation starting"),
        "Expected subsequent message to be persisted under the new session"
    );
    assert!(
        !messages1
            .iter()
            .any(|m| m.content == "new conversation starting"),
        "Subsequent message should not be in the old session"
    );
}

#[tokio::test]
async fn test_message_handler_allowlist_blocks_chat() {
    let pool = setup_test_db().await;

    let mut config = make_test_config();
    config.channels.telegram.allowed_conversations = vec!["100".to_string()]; // Only chat 100 allowed

    let mut registry = ToolRegistry::new();
    registry.register(EchoTool);

    let provider: Arc<dyn LlmExecutor> =
        Arc::new(FakeProvider::new(vec![FakeResponse::final_text(
            "should not be reached",
        )]));

    let compaction_service = make_compaction_service(pool.clone());
    let handler = MessageHandler::new(MessageHandlerInput {
        pool,
        llm: provider,
        registry: Arc::new(registry),
        config: config.clone(),
        personality: Personality::from_config(&config),
        scheduler_notifier: None,
        telegram_service: None,
        compaction_service,
    });

    // Chat 200 is not in the allowlist
    let result = handler.handle_message(200, 200, "blocked").await;
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), AgentError::PermissionDenied));
}

#[tokio::test]
async fn test_message_handler_allowlist_blocks_user() {
    let pool = setup_test_db().await;

    let mut config = make_test_config();
    config.channels.telegram.allowed_senders = vec!["100".to_string()]; // Only user 100 allowed

    let mut registry = ToolRegistry::new();
    registry.register(EchoTool);

    let provider: Arc<dyn LlmExecutor> =
        Arc::new(FakeProvider::new(vec![FakeResponse::final_text(
            "should not be reached",
        )]));

    let compaction_service = make_compaction_service(pool.clone());
    let handler = MessageHandler::new(MessageHandlerInput {
        pool,
        llm: provider,
        registry: Arc::new(registry),
        config: config.clone(),
        personality: Personality::from_config(&config),
        scheduler_notifier: None,
        telegram_service: None,
        compaction_service,
    });

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

// ── Service: send_message_with_options ─────────────────────────────────

/// Create a TelegramService pointing at the given mock server.
async fn make_mock_service(server: &MockServer) -> TelegramService {
    let base_url = format!("{}/bottest", server.uri());
    let bot = TelegramBot::new_with_base_url("test-token".into(), base_url);
    TelegramService::new(std::sync::Arc::new(bot))
}

#[tokio::test]
async fn test_send_message_with_options_sends_markdown() {
    let server = MockServer::start().await;
    let service = make_mock_service(&server).await;

    let response_body = r#"{"ok":true,"result":{"message_id":1,"chat":{"id":123,"type":"private"},"text":"hello world"}}"#;
    Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path(mock_path("/sendMessage")))
        .and(wiremock::matchers::body_json(serde_json::json!({
            "chat_id": 123,
            "text": "hello world",
            "parse_mode": "MarkdownV2",
            "disable_notification": false
        })))
        .respond_with(ResponseTemplate::new(200).set_body_string(response_body))
        .mount(&server)
        .await;

    let result = service
        .send_message_with_options(123, "hello world", Some("MarkdownV2"), Some(false))
        .await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_send_message_with_options_disable_notification() {
    let server = MockServer::start().await;
    let service = make_mock_service(&server).await;

    let response_body = r#"{"ok":true,"result":{"message_id":1,"chat":{"id":123,"type":"private"},"text":"silent message"}}"#;
    Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path(mock_path("/sendMessage")))
        .and(wiremock::matchers::body_json(serde_json::json!({
            "chat_id": 123,
            "text": "silent message",
            "disable_notification": true
        })))
        .respond_with(ResponseTemplate::new(200).set_body_string(response_body))
        .mount(&server)
        .await;

    let result = service
        .send_message_with_options(123, "silent message", None, Some(true))
        .await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_send_typing_action() {
    let server = MockServer::start().await;
    let service = make_mock_service(&server).await;

    Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path(mock_path("/sendChatAction")))
        .and(wiremock::matchers::body_json(serde_json::json!({
            "chat_id": 123,
            "action": "typing"
        })))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"ok":true,"result":true}"#))
        .mount(&server)
        .await;

    let result = service.bot().send_typing_action(123).await;
    assert!(result.is_ok());
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
        .and(wiremock::matchers::path(mock_path("/getMe")))
        .respond_with(
            ResponseTemplate::new(404).set_body_string(r#"{"ok":false,"description":"Not Found"}"#),
        )
        .mount(&server)
        .await;

    let result = bot.get_me().await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    let err_str = err.to_string();
    assert!(
        err_str.contains("getMe returned HTTP 404"),
        "unexpected error: {err_str}"
    );
}

#[tokio::test]
async fn test_get_me_returns_error_on_401() {
    let server = MockServer::start().await;
    let bot = make_mock_bot(&server).await;

    Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path(mock_path("/getMe")))
        .respond_with(
            ResponseTemplate::new(401)
                .set_body_string(r#"{"ok":false,"description":"Unauthorized"}"#),
        )
        .mount(&server)
        .await;

    let result = bot.get_me().await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    let err_str = err.to_string();
    assert!(
        err_str.contains("getMe returned HTTP 401"),
        "unexpected error: {err_str}"
    );
}

#[tokio::test]
async fn test_get_me_succeeds_on_200() {
    let server = MockServer::start().await;
    let bot = make_mock_bot(&server).await;

    let response_body =
        r#"{"ok":true,"result":{"id":12345,"first_name":"TestBot","username":"@testbot"}}"#;
    Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path(mock_path("/getMe")))
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
        .and(wiremock::matchers::path(mock_path("/deleteWebhook")))
        .respond_with(
            ResponseTemplate::new(500)
                .set_body_string(r#"{"ok":false,"description":"Internal Server Error"}"#),
        )
        .mount(&server)
        .await;

    let result = bot.delete_webhook().await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    let err_str = err.to_string();
    assert!(
        err_str.contains("deleteWebhook returned HTTP 500"),
        "unexpected error: {err_str}"
    );
}

#[tokio::test]
async fn test_delete_webhook_returns_error_on_404() {
    let server = MockServer::start().await;
    let bot = make_mock_bot(&server).await;

    Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path(mock_path("/deleteWebhook")))
        .respond_with(
            ResponseTemplate::new(404).set_body_string(r#"{"ok":false,"description":"Not Found"}"#),
        )
        .mount(&server)
        .await;

    let result = bot.delete_webhook().await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    let err_str = err.to_string();
    assert!(
        err_str.contains("deleteWebhook returned HTTP 404"),
        "unexpected error: {err_str}"
    );
}

#[tokio::test]
async fn test_set_my_commands_succeeds_on_200() {
    let server = MockServer::start().await;
    let bot = make_mock_bot(&server).await;

    Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path(mock_path("/setMyCommands")))
        .and(wiremock::matchers::body_json(serde_json::json!({
            "commands": [
                {
                    "command": "help",
                    "description": "Show available commands"
                },
                {
                    "command": "reset_context",
                    "description": "Start a fresh conversation"
                }
            ]
        })))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"ok":true,"result":true}"#))
        .mount(&server)
        .await;

    let result = bot
        .set_my_commands(&[
            BotCommand::new("help", "Show available commands"),
            BotCommand::new("reset_context", "Start a fresh conversation"),
        ])
        .await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn test_set_my_commands_returns_error_on_not_ok() {
    let server = MockServer::start().await;
    let bot = make_mock_bot(&server).await;

    Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path(mock_path("/setMyCommands")))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(r#"{"ok":false,"description":"Bad Request"}"#),
        )
        .mount(&server)
        .await;

    let result = bot
        .set_my_commands(&[BotCommand::new("bad-command", "Invalid")])
        .await;

    assert!(result.is_err());
    let err_str = result.unwrap_err().to_string();
    assert!(
        err_str.contains("Telegram setMyCommands failed: Bad Request"),
        "unexpected error: {err_str}"
    );
}

#[tokio::test]
async fn test_set_webhook_succeeds_on_200() {
    let server = MockServer::start().await;
    let bot = make_mock_bot(&server).await;

    Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path(mock_path("/setWebhook")))
        .and(wiremock::matchers::body_json(serde_json::json!({
            "url": "https://example.test/telegram/webhook",
            "secret_token": "test-secret",
            "allowed_updates": ["message", "edited_message"],
            "drop_pending_updates": false
        })))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"ok":true,"result":true}"#))
        .mount(&server)
        .await;

    let result = bot
        .set_webhook("https://example.test/telegram/webhook", "test-secret")
        .await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn test_set_webhook_returns_error_on_not_ok() {
    let server = MockServer::start().await;
    let bot = make_mock_bot(&server).await;

    Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path(mock_path("/setWebhook")))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(r#"{"ok":false,"description":"Bad Request"}"#),
        )
        .mount(&server)
        .await;

    let result = bot
        .set_webhook("https://example.test/telegram/webhook", "test-secret")
        .await;

    assert!(result.is_err());
    let err_str = result.unwrap_err().to_string();
    assert!(
        err_str.contains("Telegram setWebhook failed: Bad Request"),
        "unexpected error: {err_str}"
    );
}

#[tokio::test]
async fn test_get_updates_returns_error_on_503() {
    let server = MockServer::start().await;
    let bot = make_mock_bot(&server).await;

    Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path(mock_path("/getUpdates")))
        .respond_with(
            ResponseTemplate::new(503)
                .set_body_string(r#"{"ok":false,"description":"Service Unavailable"}"#),
        )
        .mount(&server)
        .await;

    let result = bot.get_updates(None, 0).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    let err_str = err.to_string();
    assert!(
        err_str.contains("getUpdates returned HTTP 503"),
        "unexpected error: {err_str}"
    );
}

#[tokio::test]
#[ignore = "requires a production Telegram bot token and network access"]
async fn test_get_me() {
    let token = std::env::var("TELEGRAM_BOT_TOKEN").expect("TELEGRAM_BOT_TOKEN must be set");
    let bot = TelegramBot::new(token);
    let user = bot.get_me().await.unwrap();
    println!(
        "Bot info: id={}, name={}, username={:?}",
        user.id, user.first_name, user.username
    );
}

// ── Tool context ──────────────────────────────────────────────────────

#[test]
fn test_tool_context_carries_telegram_config() {
    let ctx = nerdbot::tools::traits::ToolContext {
        run_mode: nerdbot::agent::run_mode::AgentRunMode::InteractiveReply {
            address: nerdbot::channel::ConversationAddress::telegram_chat(1),
            sender: nerdbot::channel::SenderIdentity::new((2).to_string(), None),
        },
        workspace_root: PathBuf::from("/tmp"),
        access_policy: nerdbot::channel::ChannelAccessPolicy {
            allowed_conversations: vec![1, 2]
                .into_iter()
                .map(|id| nerdbot::channel::ConversationAddressPattern {
                    channel_id: "telegram".to_string(),
                    conversation_id: id.to_string(),
                    thread_id: None,
                })
                .collect(),
            allowed_senders: vec![10, 20].into_iter().map(|id| id.to_string()).collect(),
        },
        channel_registry: None,
        pool: None,
        scheduler_notifier: None,
    };

    assert_eq!(ctx.access_policy.allowed_conversations.len(), 2);
    assert_eq!(ctx.access_policy.allowed_senders, vec!["10", "20"]);
}

// ── Markdown V2 Integration Tests ─────────────────────────────────────

#[tokio::test]
async fn test_send_message_markdown_headings_lists_links() {
    let server = MockServer::start().await;
    let service = make_mock_service(&server).await;

    let response_body = r#"{"ok":true,"result":{"message_id":1,"chat":{"id":123,"type":"private"},"text":"formatted"}}"#;
    Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path(mock_path("/sendMessage")))
        .and(wiremock::matchers::body_json(serde_json::json!({
            "chat_id": 123,
            "text": "*My Heading*\n\\- bullet item\n1\\. first item\n[google](https://google.com)",
            "parse_mode": "MarkdownV2",
            "disable_notification": false
        })))
        .respond_with(ResponseTemplate::new(200).set_body_string(response_body))
        .mount(&server)
        .await;

    let text = "# My Heading\n- bullet item\n1. first item\n[google](https://google.com)";
    let result = service
        .send_message_with_options(123, text, Some("MarkdownV2"), Some(false))
        .await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_send_message_markdown_code_blocks() {
    let server = MockServer::start().await;
    let service = make_mock_service(&server).await;

    let response_body = r#"{"ok":true,"result":{"message_id":1,"chat":{"id":123,"type":"private"},"text":"formatted"}}"#;
    Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path(mock_path("/sendMessage")))
        .and(wiremock::matchers::body_json(serde_json::json!({
            "chat_id": 123,
            "text": "```rust\nlet x = 1.0;\n```",
            "parse_mode": "MarkdownV2",
            "disable_notification": false
        })))
        .respond_with(ResponseTemplate::new(200).set_body_string(response_body))
        .mount(&server)
        .await;

    let text = "```rust\nlet x = 1.0;\n```";
    let result = service
        .send_message_with_options(123, text, Some("MarkdownV2"), Some(false))
        .await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_send_message_markdown_escaped_characters() {
    let server = MockServer::start().await;
    let service = make_mock_service(&server).await;

    let response_body = r#"{"ok":true,"result":{"message_id":1,"chat":{"id":123,"type":"private"},"text":"formatted"}}"#;
    Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path(mock_path("/sendMessage")))
        .and(wiremock::matchers::body_json(serde_json::json!({
            "chat_id": 123,
            "text": "hello\\. world\\! A \\-\\> B",
            "parse_mode": "MarkdownV2",
            "disable_notification": false
        })))
        .respond_with(ResponseTemplate::new(200).set_body_string(response_body))
        .mount(&server)
        .await;

    let text = "hello. world! A -> B";
    let result = service
        .send_message_with_options(123, text, Some("MarkdownV2"), Some(false))
        .await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_send_message_markdown_split_messages() {
    let server = MockServer::start().await;
    let service = make_mock_service(&server).await;

    let response_body = r#"{"ok":true,"result":{"message_id":1,"chat":{"id":123,"type":"private"},"text":"formatted"}}"#;

    let part1 = "x".repeat(3000);
    let part2 = "y".repeat(2000);
    let text = format!("{}\n{}", part1, part2);

    Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path(mock_path("/sendMessage")))
        .respond_with(ResponseTemplate::new(200).set_body_string(response_body))
        .mount(&server)
        .await;

    let result = service
        .send_message_with_options(123, &text, Some("MarkdownV2"), Some(false))
        .await;

    assert!(result.is_ok(), "result was Err: {:?}", result.err());

    let reqs = server.received_requests().await.unwrap_or_default();
    assert_eq!(reqs.len(), 2);

    let body1: serde_json::Value = serde_json::from_slice(&reqs[0].body).unwrap();
    let body2: serde_json::Value = serde_json::from_slice(&reqs[1].body).unwrap();

    let expected_chunk1 = format!("[1/2] {}\n", part1);
    let expected_chunk2 = format!("[2/2] {}", part2);

    assert_eq!(body1["chat_id"], 123);
    assert!(body1.get("parse_mode").is_none());
    assert_eq!(body1["disable_notification"], false);
    assert_eq!(body1["text"], expected_chunk1);

    assert_eq!(body2["chat_id"], 123);
    assert!(body2.get("parse_mode").is_none());
    assert_eq!(body2["disable_notification"], false);
    assert_eq!(body2["text"], expected_chunk2);
}

#[tokio::test]
async fn test_send_message_markdown_preserves_paragraph_spacing() {
    let server = MockServer::start().await;
    let service = make_mock_service(&server).await;

    let response_body = r#"{"ok":true,"result":{"message_id":1,"chat":{"id":123,"type":"private"},"text":"formatted"}}"#;
    Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path(mock_path("/sendMessage")))
        .and(wiremock::matchers::body_json(serde_json::json!({
            "chat_id": 123,
            "text": "first\n\nsecond",
            "parse_mode": "MarkdownV2",
            "disable_notification": false
        })))
        .respond_with(ResponseTemplate::new(200).set_body_string(response_body))
        .mount(&server)
        .await;

    let result = service
        .send_message_with_options(123, "first\n\nsecond", Some("MarkdownV2"), Some(false))
        .await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_send_message_markdown_escape_expansion_falls_back_to_plain_text() {
    let server = MockServer::start().await;
    let service = make_mock_service(&server).await;

    let text = ".".repeat(3000);
    let response_body = r#"{"ok":true,"result":{"message_id":1,"chat":{"id":123,"type":"private"},"text":"plain"}}"#;
    Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path(mock_path("/sendMessage")))
        .and(wiremock::matchers::body_json(serde_json::json!({
            "chat_id": 123,
            "text": text,
            "disable_notification": false
        })))
        .respond_with(ResponseTemplate::new(200).set_body_string(response_body))
        .mount(&server)
        .await;

    let result = service
        .send_message_with_options(123, &text, Some("MarkdownV2"), Some(false))
        .await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_send_message_markdown_fallback_delivery() {
    let server = MockServer::start().await;
    let service = make_mock_service(&server).await;

    // First attempt with MarkdownV2 fails with 400 Bad Request
    let error_response = r#"{"ok":false,"description":"Bad Request: can't parse entities: Character '.' is reserved and must be escaped with the preceding '\\'"}"#;
    Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path(mock_path("/sendMessage")))
        .and(wiremock::matchers::body_json(serde_json::json!({
            "chat_id": 123,
            "text": "hello\\. world",
            "parse_mode": "MarkdownV2",
            "disable_notification": false
        })))
        .respond_with(ResponseTemplate::new(400).set_body_string(error_response))
        .up_to_n_times(1)
        .mount(&server)
        .await;

    // Second attempt fallback with None (plain text) succeeds
    let success_response = r#"{"ok":true,"result":{"message_id":2,"chat":{"id":123,"type":"private"},"text":"hello. world"}}"#;
    Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path(mock_path("/sendMessage")))
        .and(wiremock::matchers::body_json(serde_json::json!({
            "chat_id": 123,
            "text": "hello. world",
            "disable_notification": false
        })))
        .respond_with(ResponseTemplate::new(200).set_body_string(success_response))
        .mount(&server)
        .await;

    let result = service
        .send_message_with_options(123, "hello. world", Some("MarkdownV2"), Some(false))
        .await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_send_message_markdown_no_fallback_on_500_error() {
    let server = MockServer::start().await;
    let service = make_mock_service(&server).await;

    // Attempt with MarkdownV2 fails with 500 Internal Server Error
    Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path(mock_path("/sendMessage")))
        .respond_with(ResponseTemplate::new(500).set_body_string("Internal Error"))
        .up_to_n_times(1)
        .mount(&server)
        .await;

    // It should fail immediately and not send a fallback request!
    let result = service
        .send_message_with_options(123, "hello. world", Some("MarkdownV2"), Some(false))
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_send_message_markdown_raw_mode() {
    let server = MockServer::start().await;
    let service = make_mock_service(&server).await;

    let response_body =
        r#"{"ok":true,"result":{"message_id":1,"chat":{"id":123,"type":"private"},"text":"raw"}}"#;

    // In Raw mode, we pass standard-markdown unchanged without any escaping.
    // So *italic* remains *italic* (which Telegram interprets as bold).
    Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path(mock_path("/sendMessage")))
        .and(wiremock::matchers::body_json(serde_json::json!({
            "chat_id": 123,
            "text": "*bold*",
            "parse_mode": "MarkdownV2",
            "disable_notification": false
        })))
        .respond_with(ResponseTemplate::new(200).set_body_string(response_body))
        .mount(&server)
        .await;

    let result = service
        .send_message_with_options(123, "*bold*", Some("MarkdownV2Raw"), Some(false))
        .await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_send_message_long_markdown_raw_falls_back_to_plain_text() {
    let server = MockServer::start().await;
    let service = make_mock_service(&server).await;

    let part1 = "x".repeat(3000);
    let part2 = "y".repeat(2000);
    let text = format!("{}\n{}", part1, part2);
    let response_body = r#"{"ok":true,"result":{"message_id":1,"chat":{"id":123,"type":"private"},"text":"plain"}}"#;
    Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path(mock_path("/sendMessage")))
        .respond_with(ResponseTemplate::new(200).set_body_string(response_body))
        .mount(&server)
        .await;

    let result = service
        .send_message_with_options(123, &text, Some("MarkdownV2Raw"), Some(false))
        .await;
    assert!(result.is_ok());

    let reqs = server.received_requests().await.unwrap_or_default();
    assert_eq!(reqs.len(), 2);
    for req in reqs {
        let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
        assert!(body.get("parse_mode").is_none());
    }
}

// ── Maintenance Mode Tests ─────────────────────────────────────────

fn make_maintenance_handler(pool: sqlx::SqlitePool, reason: &str) -> MessageHandler {
    let mut config = make_test_config();
    config.maintenance.enabled = true;
    config.maintenance.reason = reason.to_string();

    let mut registry = ToolRegistry::new();
    registry.register(EchoTool);

    let provider: Arc<dyn LlmExecutor> =
        Arc::new(FakeProvider::new(vec![FakeResponse::final_text(
            "should not be reached",
        )]));
    let compaction_service = make_compaction_service(pool.clone());

    MessageHandler::new(MessageHandlerInput {
        pool,
        llm: provider,
        registry: Arc::new(registry),
        config,
        personality: Personality::from_config(&make_test_config()),
        scheduler_notifier: None,
        telegram_service: None,
        compaction_service,
    })
}

#[tokio::test]
async fn test_maintenance_mode_returns_maintenance_text_for_normal_message() {
    let pool = setup_test_db().await;
    let handler = make_maintenance_handler(pool, "Database migration in progress");

    let response = handler
        .handle_message(1, 1, "Hello, how are you?")
        .await
        .unwrap();

    assert!(response.is_some());
    let text = response.unwrap();
    assert!(text.contains("maintenance mode"));
    assert!(text.contains("Database migration in progress"));
}

#[tokio::test]
async fn test_maintenance_mode_short_circuits_commands() {
    let pool = setup_test_db().await;
    let handler = make_maintenance_handler(pool, "Scheduled maintenance");

    let response = handler.handle_message(1, 1, "/help").await.unwrap();

    assert!(response.is_some());
    let text = response.unwrap();
    assert!(text.contains("maintenance mode"));
    assert!(!text.contains("/start"));
    assert!(!text.contains("/help"));
}

#[tokio::test]
async fn test_maintenance_mode_no_session_created() {
    let pool = setup_test_db().await;
    let handler = make_maintenance_handler(pool.clone(), "Reason");

    let _ = handler
        .handle_message(999, 888, "maintenance message")
        .await
        .unwrap();

    // Verify no session was created
    let session = storage::sessions::get_session_for_chat(&pool, 999)
        .await
        .unwrap();
    assert!(
        session.is_none(),
        "Expected no session to be created during maintenance mode"
    );
}

#[tokio::test]
async fn test_maintenance_mode_no_message_persistence() {
    let pool = setup_test_db().await;
    let handler = make_maintenance_handler(pool.clone(), "Reason");

    let _ = handler
        .handle_message(777, 666, "persistent message")
        .await
        .unwrap();

    // Verify no session was created, therefore no messages persisted
    let session = storage::sessions::get_session_for_chat(&pool, 777)
        .await
        .unwrap();
    assert!(session.is_none(), "No session should exist");
}

#[tokio::test]
async fn test_maintenance_mode_without_reason() {
    let pool = setup_test_db().await;
    let handler = make_maintenance_handler(pool, "");

    let response = handler.handle_message(1, 1, "test").await.unwrap();

    assert!(response.is_some());
    let text = response.unwrap();
    assert!(text.contains("maintenance mode"));
    assert!(!text.contains("Reason:"));
}

#[tokio::test]
async fn test_maintenance_mode_reason_trims_whitespace() {
    let pool = setup_test_db().await;
    let handler = make_maintenance_handler(pool, "   ");

    let response = handler.handle_message(1, 1, "test").await.unwrap();

    assert!(response.is_some());
    let text = response.unwrap();
    assert!(text.contains("maintenance mode"));
    assert!(!text.contains("Reason:"));
}
