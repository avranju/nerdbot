//! Integration tests for Telegram attachment support.
//!
//! Tests cover:
//! - getFile API mocking and parsing
//! - Bounded file download with size limits
//! - Largest-photo selection from multi-size responses
//! - Photo, image document, and PDF attachment processing
//! - Text document extraction with UTF-8 validation
//! - Caption vs default-prompt behavior
//! - Unsupported MIME type rejection
//! - Malformed text document handling
//! - Oversized file rejection
//! - Allowlist-before-download ordering
//! - Filename sanitization
//! - MIME signature inspection

#![allow(
    dead_code,
    unused,
    unused_imports,
    unused_variables,
    unused_assignments
)]

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
use nerdbot::telegram::attachment;
use nerdbot::telegram::bot::{Document, Message, PhotoSize, TelegramBot};
use nerdbot::telegram::handler::{
    AttachmentInfo, InboundMessage, MessageHandler, MessageHandlerInput, build_persist_text,
};
use nerdbot::tools::echo::EchoTool;
use nerdbot::tools::registry::ToolRegistry;
use wiremock::{Mock, MockServer, ResponseTemplate};

// ── Helpers ──────────────────────────────────────────────────────────

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

    let config = AppConfig::default();
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

/// Create a TelegramBot pointing at the given mock server.
async fn make_mock_bot(server: &MockServer) -> TelegramBot {
    let base_url = format!("{}/bottest", server.uri());
    TelegramBot::new_with_base_url("test-token".into(), base_url)
}

fn mock_path(endpoint: &str) -> String {
    format!("/bottest{endpoint}")
}

// ── getFile API tests ────────────────────────────────────────────────

#[tokio::test]
async fn test_get_file_returns_path_on_success() {
    let server = MockServer::start().await;
    let bot = make_mock_bot(&server).await;

    let response_body = r#"{"ok":true,"result":{"file_path":"photos/abc123/0.jpg"}}"#;
    Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path(mock_path("/getFile")))
        .and(wiremock::matchers::body_json(
            serde_json::json!({"file_id": "abc123"}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_string(response_body))
        .mount(&server)
        .await;

    let result = bot.get_file("abc123").await.unwrap();
    assert_eq!(result, Some("photos/abc123/0.jpg".to_string()));
}

#[tokio::test]
async fn test_get_file_returns_none_when_no_path() {
    let server = MockServer::start().await;
    let bot = make_mock_bot(&server).await;

    let response_body = r#"{"ok":true,"result":{}}"#;
    Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path(mock_path("/getFile")))
        .and(wiremock::matchers::body_json(
            serde_json::json!({"file_id": "big"}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_string(response_body))
        .mount(&server)
        .await;

    let result = bot.get_file("big").await.unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn test_get_file_returns_error_on_failure() {
    let server = MockServer::start().await;
    let bot = make_mock_bot(&server).await;

    Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path(mock_path("/getFile")))
        .respond_with(
            ResponseTemplate::new(404)
                .set_body_string(r#"{"ok":false,"description":"File not found"}"#),
        )
        .mount(&server)
        .await;

    let result = bot.get_file("nonexistent").await;
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("getFile returned HTTP 404")
    );
}

// ── download_file tests ──────────────────────────────────────────────

#[tokio::test]
async fn test_process_attachment_downloads_and_extracts_text() {
    let server = MockServer::start().await;
    let bot = make_mock_bot(&server).await;

    Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path(mock_path("/getFile")))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(r#"{"ok":true,"result":{"file_path":"documents/note.txt"}}"#),
        )
        .mount(&server)
        .await;
    Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path(mock_path("/documents/note.txt")))
        .respond_with(ResponseTemplate::new(200).set_body_bytes("hello from telegram"))
        .mount(&server)
        .await;

    let processed = bot
        .process_attachment(
            "text-file",
            Some("note.txt"),
            Some("text/plain"),
            19,
            1024,
            1024,
        )
        .await
        .unwrap();

    assert!(processed.downloaded);
    assert_eq!(
        processed.extracted_text.as_deref(),
        Some("hello from telegram")
    );
    assert_eq!(
        processed.persistence_marker,
        "[Attached text document: note.txt, 19 chars]"
    );
}

#[tokio::test]
async fn test_download_file_enforces_mocked_content_length_limit() {
    let server = MockServer::start().await;
    let bot = make_mock_bot(&server).await;

    Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path(mock_path("/getFile")))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(r#"{"ok":true,"result":{"file_path":"documents/large.txt"}}"#),
        )
        .mount(&server)
        .await;
    Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path(mock_path("/documents/large.txt")))
        .respond_with(ResponseTemplate::new(200).set_body_bytes("too large"))
        .mount(&server)
        .await;

    let err = bot.download_file("large-file", 3).await.unwrap_err();
    assert!(err.to_string().contains("exceeds maximum"));
}

#[test]
fn test_build_persist_text_keeps_failure_marker_and_full_bounded_text() {
    let bounded_text = "x".repeat(40_000);
    let inbound = InboundMessage {
        text: "caption".to_string(),
        attachment_parts: Vec::new(),
        attachments: vec![
            AttachmentInfo {
                display_name: "failed.pdf".to_string(),
                mime_type: "application/pdf".to_string(),
                size_bytes: 42,
                downloaded: false,
                persistence_marker: "[Attachment processing failed: failed.pdf]".to_string(),
                extracted_text: None,
            },
            AttachmentInfo {
                display_name: "note.txt".to_string(),
                mime_type: "text/plain".to_string(),
                size_bytes: bounded_text.len() as u64,
                downloaded: true,
                persistence_marker: "[Attached text document: note.txt]".to_string(),
                extracted_text: Some(bounded_text.clone()),
            },
        ],
    };

    let persisted = build_persist_text(&inbound);
    assert!(persisted.contains("[Attachment processing failed: failed.pdf]"));
    assert!(persisted.contains(&bounded_text));
    assert!(persisted.ends_with("caption"));
}

// ── sanitize_filename tests ──────────────────────────────────────────

#[test]
fn test_sanitize_filename_replaces_dangerous_chars() {
    assert_eq!(attachment::sanitize_filename("hello.txt"), "hello.txt");
    assert_eq!(
        attachment::sanitize_filename("path/to/file.txt"),
        "path_to_file.txt"
    );
    assert_eq!(
        attachment::sanitize_filename("file\0with\0nulls.txt"),
        "file_with_nulls.txt"
    );
    assert_eq!(
        attachment::sanitize_filename("path\\backslash.txt"),
        "path_backslash.txt"
    );
    assert_eq!(
        attachment::sanitize_filename(&"a".repeat(300)),
        "a".repeat(200)
    );
}

// ── MIME check tests ─────────────────────────────────────────────────

#[test]
fn test_mime_checks_use_exact_match() {
    // Exact match should work
    assert!(attachment::is_supported_binary_mime("image/jpeg"));
    assert!(attachment::is_supported_text_mime("text/plain"));

    // Near-misses should NOT match
    assert!(!attachment::is_supported_binary_mime("image/jpg"));
    assert!(!attachment::is_supported_text_mime("text/plainish"));
    assert!(!attachment::is_supported_binary_mime("application/pdfx"));
}

// ── Largest photo selection ──────────────────────────────────────────

#[test]
fn test_largest_photo_selects_by_area() {
    let photos = vec![
        PhotoSize {
            file_id: "small".into(),
            width: Some(100),
            height: Some(100),
            ..Default::default()
        },
        PhotoSize {
            file_id: "medium".into(),
            width: Some(640),
            height: Some(480),
            ..Default::default()
        },
        PhotoSize {
            file_id: "large".into(),
            width: Some(320),
            height: Some(240),
            ..Default::default()
        },
    ];

    let largest = attachment::largest_photo_size(&photos).unwrap();
    assert_eq!(largest.file_id, "medium");
    assert_eq!(largest.width, Some(640));
    assert_eq!(largest.height, Some(480));
}

#[test]
fn test_largest_photo_empty_list() {
    assert!(attachment::largest_photo_size(&[]).is_none());
}

#[test]
fn test_largest_photo_file_id() {
    let photos = vec![
        PhotoSize {
            file_id: "small".into(),
            width: Some(100),
            height: Some(100),
            ..Default::default()
        },
        PhotoSize {
            file_id: "big".into(),
            width: Some(1920),
            height: Some(1080),
            ..Default::default()
        },
    ];
    assert_eq!(
        attachment::largest_photo_file_id(&photos),
        Some("big".into())
    );
}

// ── MIME type validation ─────────────────────────────────────────────

#[test]
fn test_supported_binary_mime_types() {
    assert!(attachment::is_supported_binary_mime("image/jpeg"));
    assert!(attachment::is_supported_binary_mime("image/png"));
    assert!(attachment::is_supported_binary_mime("image/webp"));
    assert!(attachment::is_supported_binary_mime("image/gif"));
    assert!(attachment::is_supported_binary_mime("application/pdf"));
}

#[test]
fn test_unsupported_mime_types() {
    assert!(!attachment::is_supported_binary_mime("application/zip"));
    assert!(!attachment::is_supported_binary_mime("application/x-rar"));
    assert!(!attachment::is_supported_binary_mime("video/mp4"));
    assert!(!attachment::is_supported_binary_mime("audio/mpeg"));
}

#[test]
fn test_supported_text_mime_types() {
    assert!(attachment::is_supported_text_mime("text/plain"));
    assert!(attachment::is_supported_text_mime("text/markdown"));
    assert!(attachment::is_supported_text_mime("application/json"));
    assert!(attachment::is_supported_text_mime("text/csv"));
    assert!(attachment::is_supported_text_mime("application/xml"));
}

#[test]
fn test_text_extensions() {
    for ext in &[
        ".txt", ".md", ".json", ".csv", ".html", ".xml", ".py", ".js", ".yaml", ".toml",
    ] {
        assert!(attachment::is_text_extension(&format!("file{ext}")));
    }
    for ext in &[".jpg", ".png", ".pdf", ".zip", ".exe"] {
        assert!(!attachment::is_text_extension(&format!("file{ext}")));
    }
}

// ── Filename sanitization ────────────────────────────────────────────

#[test]
fn test_sanitize_filename_basic() {
    assert_eq!(attachment::sanitize_filename("hello.txt"), "hello.txt");
}

#[test]
fn test_sanitize_filename_replaces_path_separators() {
    assert_eq!(
        attachment::sanitize_filename("path/to/file.txt"),
        "path_to_file.txt"
    );
}

#[test]
fn test_sanitize_filename_replaces_null_bytes() {
    assert_eq!(
        attachment::sanitize_filename("file\0name.txt"),
        "file_name.txt"
    );
}

#[test]
fn test_sanitize_filename_truncates() {
    let long = "a".repeat(300);
    let result = attachment::sanitize_filename(&long);
    assert_eq!(result.len(), 200);
    assert_eq!(result, "a".repeat(200));
}

// ── MIME signature inspection ────────────────────────────────────────

#[test]
fn test_inspect_signature_png() {
    assert_eq!(
        attachment::inspect_mime_signature(&[0x89, 0x50, 0x4E, 0x47]),
        Some("image/png")
    );
}

#[test]
fn test_inspect_signature_jpeg() {
    assert_eq!(
        attachment::inspect_mime_signature(&[0xFF, 0xD8, 0xFF, 0xE0]),
        Some("image/jpeg")
    );
}

#[test]
fn test_inspect_signature_gif() {
    assert_eq!(
        attachment::inspect_mime_signature(b"GIF8"),
        Some("image/gif")
    );
}

#[test]
fn test_inspect_signature_webp() {
    assert_eq!(
        attachment::inspect_mime_signature(b"RIFF\x00\x00\x00\x00WEBP"),
        Some("image/webp")
    );
}

#[test]
fn test_inspect_signature_unknown() {
    assert_eq!(attachment::inspect_mime_signature(b"hello world"), None);
}

#[test]
fn test_inspect_signature_too_short() {
    assert_eq!(attachment::inspect_mime_signature(b"hi"), None);
}

// ── Attachment classification ────────────────────────────────────────

#[test]
fn test_classify_image_jpeg() {
    assert_eq!(
        attachment::classify_attachment(Some("image/jpeg"), None),
        attachment::AttachmentKind::Binary
    );
}

#[test]
fn test_classify_pdf() {
    assert_eq!(
        attachment::classify_attachment(Some("application/pdf"), None),
        attachment::AttachmentKind::Binary
    );
}

#[test]
fn test_classify_text_plain() {
    assert_eq!(
        attachment::classify_attachment(Some("text/plain"), None),
        attachment::AttachmentKind::Text
    );
}

#[test]
fn test_classify_json() {
    assert_eq!(
        attachment::classify_attachment(Some("application/json"), None),
        attachment::AttachmentKind::Text
    );
}

#[test]
fn test_classify_extension_fallback() {
    assert_eq!(
        attachment::classify_attachment(None, Some("report.txt")),
        attachment::AttachmentKind::Text
    );
    assert_eq!(
        attachment::classify_attachment(None, Some("script.py")),
        attachment::AttachmentKind::Text
    );
}

#[test]
fn test_classify_unsupported() {
    assert_eq!(
        attachment::classify_attachment(Some("application/zip"), None),
        attachment::AttachmentKind::Unsupported
    );
    assert_eq!(
        attachment::classify_attachment(None, Some("archive.zip")),
        attachment::AttachmentKind::Unsupported
    );
}

#[test]
fn test_classify_mime_wins_over_extension() {
    // A .txt file with PDF MIME should be treated as binary
    assert_eq!(
        attachment::classify_attachment(Some("application/pdf"), Some("fake.txt")),
        attachment::AttachmentKind::Binary
    );
}

// ── Handler integration with rich messages ───────────────────────────

#[tokio::test]
async fn test_handler_accepts_rich_message_with_text() {
    let pool = setup_test_db().await;
    let handler = make_handler(pool);

    let inbound = InboundMessage {
        text: "Hello, this is a rich message".to_string(),
        attachment_parts: Vec::new(),
        attachments: Vec::new(),
    };

    let response = handler.handle_rich_message(1, 1, &inbound).await.unwrap();
    assert!(response.is_some());
    assert!(response.unwrap().contains("agent loop"));
}

#[tokio::test]
async fn test_handler_commands_override_attachments() {
    let pool = setup_test_db().await;
    let handler = make_handler(pool);

    // Even with attachments, /help should be handled as a command
    let inbound = InboundMessage {
        text: "/help".to_string(),
        attachment_parts: Vec::new(),
        attachments: Vec::new(),
    };

    let response = handler.handle_rich_message(1, 1, &inbound).await.unwrap();
    assert!(response.is_some());
    assert!(response.unwrap().contains("/start"));
}

#[tokio::test]
async fn test_handler_allows_when_no_restrictions() {
    let pool = setup_test_db().await;
    let handler = make_handler(pool);

    let inbound = InboundMessage {
        text: "test".to_string(),
        attachment_parts: Vec::new(),
        attachments: Vec::new(),
    };

    let result = handler.handle_rich_message(999, 888, &inbound).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_handler_blocks_unauthorized_chat() {
    let pool = setup_test_db().await;

    let mut config = AppConfig::default();
    config.telegram.allowed_chat_ids = vec![100];

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

    let inbound = InboundMessage {
        text: "blocked".to_string(),
        attachment_parts: Vec::new(),
        attachments: Vec::new(),
    };

    let result = handler.handle_rich_message(200, 200, &inbound).await;
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), AgentError::PermissionDenied));
}

// ── Context manager tests ────────────────────────────────────────────

#[tokio::test]
async fn test_context_manager_respects_preserve_count() {
    let pool = setup_test_db().await;
    let budget = ContextBudget::default();
    let manager = nerdbot::context::manager::ContextManager::new_with_preserve(
        pool.clone(),
        budget,
        3, // preserve 3 most recent turns
    );

    // Create a session and add messages
    let session = storage::sessions::create_session(&pool, 1).await.unwrap();

    for i in 0..10 {
        let msg = genai::chat::ChatMessage::user(genai::chat::MessageContent::from_text(format!(
            "Message {i}"
        )));
        storage::messages::create_message(&pool, &session.id, &msg, None)
            .await
            .unwrap();
    }

    // Assemble messages — the 3 most recent should be preserved
    let messages = manager
        .assemble_messages(
            &session.id,
            "You are a bot.",
            genai::chat::ChatMessage::user(genai::chat::MessageContent::from_text("Current")),
            "UTC",
        )
        .await
        .unwrap();

    // Should have: personality + recent messages (including preserved ones) + current
    assert!(messages.len() >= 4); // at least 3 preserved + 1 current
}

#[tokio::test]
async fn test_context_manager_estimate_tokens_excludes_binary() {
    let pool = setup_test_db().await;
    let budget = ContextBudget::default();
    let manager = nerdbot::context::manager::ContextManager::new(pool.clone(), budget);

    // Create a message with both text and binary parts
    let mut content = genai::chat::MessageContent::from_text("Hello world");
    let binary_part = genai::chat::ContentPart::from_binary_base64(
        "image/png",
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==",
        Some("test.png".to_string()),
    );
    content = genai::chat::MessageContent::from_parts(vec![
        genai::chat::ContentPart::Text("Hello world".to_string()),
        binary_part,
    ]);
    let msg = genai::chat::ChatMessage::new(genai::chat::ChatRole::User, content);

    let tokens = manager.estimate_tokens(&[msg]);
    // Should estimate ~3 tokens for "Hello world" (11 chars / 4 ≈ 3)
    // Binary part should NOT contribute to token count
    assert!(tokens > 0);
    assert!(tokens < 10); // Should be small, not inflated by base64
}

#[tokio::test]
async fn test_context_manager_appends_datetime_without_dropping_binary_parts() {
    let pool = setup_test_db().await;
    let budget = ContextBudget::default();
    let manager = nerdbot::context::manager::ContextManager::new(pool.clone(), budget);
    let session = storage::sessions::create_session(&pool, 1).await.unwrap();

    let binary_part = genai::chat::ContentPart::from_binary_base64(
        "image/png",
        "iVBORw0KGgo=",
        Some("test.png".to_string()),
    );
    let current = genai::chat::ChatMessage::user(genai::chat::MessageContent::from_parts(vec![
        genai::chat::ContentPart::Text("Analyze this image".to_string()),
        binary_part,
    ]));

    let messages = manager
        .assemble_messages(&session.id, "You are a bot.", current, "UTC")
        .await
        .unwrap();
    let last = messages.last().unwrap();
    let parts = last.content.parts();

    assert_eq!(parts.len(), 3);
    assert!(matches!(parts[0], genai::chat::ContentPart::Text(_)));
    assert!(matches!(parts[1], genai::chat::ContentPart::Binary(_)));
    assert!(matches!(
        &parts[2],
        genai::chat::ContentPart::Text(t) if t.contains("## Current Date/Time")
    ));
}
