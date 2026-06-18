//! Tests for Zulip channel integration.
//!
//! Covers URL construction, attachment processing, address resolution,
//! mention stripping, and bot info fetching.

use nerdbot::channel::{ChannelService, ConversationAddress};
use nerdbot::zulip::attachment::process_inbound_attachments;
use nerdbot::zulip::bot::{
    ZulipBot, ZulipDisplayRecipient, ZulipMessage, ZulipPrivateRecipient, ZulipRecipient,
};
use nerdbot::zulip::service::ZulipService;
use nerdbot::zulip::update::{
    resolve_zulip_address, resolve_zulip_address_for_user, resolve_zulip_private_recipient_ids,
    resolve_zulip_private_recipient_ids_for_user, strip_bot_mention,
};
use std::sync::Arc;
use std::time::Duration;
use wiremock::matchers::{body_string_contains, method, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

// Helper to get the mock server base URL (without trailing slash).
// ZulipBot adds the trailing slash internally.
fn mock_base_url(server: &MockServer) -> String {
    server.uri()
}

// ── URL Construction Tests ────────────────────────────────────────

#[tokio::test]
async fn test_send_message_url_construction() {
    let mock_server = MockServer::start().await;

    // The ZulipBot constructs: {site_url}/api/v1/messages
    // mock_server.uri() returns e.g. "http://127.0.0.1:12345"
    // ZulipBot adds trailing slash, so URL becomes: http://127.0.0.1:12345/api/v1/messages
    Mock::given(method("POST"))
        .and(path_regex("/api/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(r#"{"id": 1, "result": "success", "msg": ""}"#),
        )
        .mount(&mock_server)
        .await;

    let bot = ZulipBot::new(
        mock_base_url(&mock_server),
        "test-bot@test.com".into(),
        "test-api-key".into(),
    );

    let recipient = ZulipRecipient::Stream {
        stream_name: "general".into(),
        topic: "test".into(),
    };
    let result = bot.send_message(&recipient, "Hello").await;
    assert!(
        result.is_ok(),
        "send_message should succeed with correct URL: {:?}",
        result
    );
}

#[tokio::test]
async fn test_send_typing_notification_url_construction() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path_regex("/api/v1/typing"))
        .and(body_string_contains("to=%5B100%5D"))
        .and(body_string_contains("type=direct"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(r#"{"result": "success", "msg": ""}"#),
        )
        .mount(&mock_server)
        .await;

    let bot = ZulipBot::new(
        mock_base_url(&mock_server),
        "test-bot@test.com".into(),
        "test-api-key".into(),
    );

    let result = bot.send_typing_notification(&[100], "start").await;
    assert!(
        result.is_ok(),
        "send_typing_notification should succeed: {:?}",
        result
    );
}

#[tokio::test]
async fn test_update_presence_url_construction() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path_regex("/api/v1/users/me/presence"))
        .and(body_string_contains("status=active"))
        .and(body_string_contains("ping_only=true"))
        .and(body_string_contains("new_user_input=false"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(r#"{"result": "success", "msg": ""}"#),
        )
        .mount(&mock_server)
        .await;

    let bot = ZulipBot::new(
        mock_base_url(&mock_server),
        "test-bot@test.com".into(),
        "test-api-key".into(),
    );

    let result = bot.update_presence("active", true).await;
    assert!(result.is_ok(), "update_presence should succeed: {result:?}");
}

#[tokio::test]
async fn test_zulip_typing_indicator_sends_stop_on_drop() {
    let mock_server = MockServer::start().await;

    let typing_mock = Mock::given(method("POST"))
        .and(path_regex("/api/v1/typing"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(r#"{"result": "success", "msg": ""}"#),
        )
        .mount_as_scoped(&mock_server)
        .await;

    let bot = Arc::new(ZulipBot::new(
        mock_base_url(&mock_server),
        "test-bot@test.com".into(),
        "test-api-key".into(),
    ));
    bot.cache_typing_recipient_ids("user@test.com", vec![100]);
    let service = ZulipService::new(bot);
    let address = ConversationAddress::new("zulip", "user@test.com", None);

    let indicator = service.start_typing(&address).unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    drop(indicator);
    tokio::time::sleep(Duration::from_millis(50)).await;

    let request_bodies = typing_mock
        .received_requests()
        .await
        .into_iter()
        .map(|request| String::from_utf8(request.body).unwrap())
        .collect::<Vec<_>>();

    assert!(
        request_bodies.iter().any(|body| body.contains("op=start")
            && body.contains("to=%5B100%5D")
            && body.contains("type=direct")),
        "expected start typing request, got {request_bodies:?}"
    );
    assert!(
        request_bodies.iter().any(|body| body.contains("op=stop")
            && body.contains("to=%5B100%5D")
            && body.contains("type=direct")),
        "expected stop typing request, got {request_bodies:?}"
    );
}

#[tokio::test]
async fn test_register_queue_url_construction() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path_regex("/api/v1/register"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"queue_id": "abc123", "last_event_id": 0, "result": "success", "msg": ""}"#,
        ))
        .mount(&mock_server)
        .await;

    let bot = ZulipBot::new(
        mock_base_url(&mock_server),
        "test-bot@test.com".into(),
        "test-api-key".into(),
    );

    let result = bot.register_queue().await;
    assert!(
        result.is_ok(),
        "register_queue should succeed: {:?}",
        result
    );
    let queue = result.unwrap();
    assert_eq!(queue.queue_id, "abc123");
    assert_eq!(queue.last_event_id, Some(0));
}

#[tokio::test]
async fn test_get_events_url_construction() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path_regex("/api/v1/events"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(r#"{"events": [], "result": "success"}"#),
        )
        .mount(&mock_server)
        .await;

    let bot = ZulipBot::new(
        mock_base_url(&mock_server),
        "test-bot@test.com".into(),
        "test-api-key".into(),
    );

    let result = bot.get_events("abc123", 0).await;
    assert!(result.is_ok(), "get_events should succeed: {:?}", result);
}

#[tokio::test]
async fn test_download_file_url_construction() {
    let mock_server = MockServer::start().await;

    // Zulip user uploads are at the site root, not under /api/v1/
    // The ZulipBot constructs: {site_url}{relative_path} where relative_path starts with /
    // So the full URL has double slash: http://host//user_uploads/...
    // Wiremock normalizes paths, so we match on the filename portion
    Mock::given(method("GET"))
        .and(path_regex(r"/user_uploads/1/99/abc/test\.pdf"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"%PDF-1.4 fake pdf content"))
        .mount(&mock_server)
        .await;

    let bot = ZulipBot::new(
        mock_base_url(&mock_server),
        "test-bot@test.com".into(),
        "test-api-key".into(),
    );

    let result = bot
        .download_file("/user_uploads/1/99/abc/test.pdf", 1_000_000)
        .await;
    assert!(result.is_ok(), "download_file should succeed: {:?}", result);
    let bytes = result.unwrap();
    assert_eq!(bytes.len(), 25); // "%PDF-1.4 fake pdf content" = 25 bytes
}

#[tokio::test]
async fn test_download_file_aborts_on_content_length_exceed() {
    let mock_server = MockServer::start().await;

    // Respond with Content-Length header indicating a large file
    // The body matches Content-Length to avoid wiremock validation errors
    Mock::given(method("GET"))
        .and(path_regex(r"/user_uploads/1/99/abc/big\.pdf"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-length", "500000")
                .set_body_bytes([0u8; 500000]),
        )
        .mount(&mock_server)
        .await;

    let bot = ZulipBot::new(
        mock_base_url(&mock_server),
        "test-bot@test.com".into(),
        "test-api-key".into(),
    );

    // max_bytes is 50, but Content-Length says 500,000
    // The download should fail immediately without reading the body
    let result = bot
        .download_file("/user_uploads/1/99/abc/big.pdf", 50)
        .await;
    assert!(
        result.is_err(),
        "download_file should fail when Content-Length exceeds max_bytes"
    );
    let err = result.unwrap_err();
    let err_str = format!("{:?}", err);
    assert!(
        err_str.contains("exceeds maximum"),
        "error should mention exceeding limit: {}",
        err_str
    );
}

#[tokio::test]
async fn test_download_file_aborts_on_stream_exceed() {
    let mock_server = MockServer::start().await;

    // No Content-Length header (so pre-check is skipped), but response body is large
    Mock::given(method("GET"))
        .and(path_regex(r"/user_uploads/1/99/abc/streaming\.pdf"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes([0u8; 500]))
        .mount(&mock_server)
        .await;

    let bot = ZulipBot::new(
        mock_base_url(&mock_server),
        "test-bot@test.com".into(),
        "test-api-key".into(),
    );

    // max_bytes is 100, but response body is 500 bytes
    let result = bot
        .download_file("/user_uploads/1/99/abc/streaming.pdf", 100)
        .await;
    assert!(
        result.is_err(),
        "download_file should fail when stream exceeds max_bytes"
    );
    let err = result.unwrap_err();
    let err_str = format!("{:?}", err);
    assert!(
        err_str.contains("exceeds maximum"),
        "error should mention exceeding limit: {}",
        err_str
    );
}

// ── Bot Info Fetching Tests ───────────────────────────────────────

#[tokio::test]
async fn test_get_me_returns_bot_info() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path_regex("/api/v1/users/me"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{
                "user_id": 42,
                "email": "nerd-bot@test.zulipchat.com",
                "full_name": "NerdBot"
            }"#,
        ))
        .mount(&mock_server)
        .await;

    let bot = ZulipBot::new(
        mock_base_url(&mock_server),
        "nerd-bot@test.zulipchat.com".into(),
        "test-api-key".into(),
    );

    let user_info = bot.get_me().await.unwrap();
    assert_eq!(user_info.user_id, 42);
    assert_eq!(user_info.email, "nerd-bot@test.zulipchat.com");
    assert_eq!(user_info.full_name, "NerdBot");
}

#[tokio::test]
async fn test_bot_name_resolution() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path_regex("/api/v1/users/me"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{
                "user_id": 42,
                "email": "bot@test.com",
                "full_name": "MyBot"
            }"#,
        ))
        .mount(&mock_server)
        .await;

    let bot = ZulipBot::new(
        mock_base_url(&mock_server),
        "bot@test.com".into(),
        "key".into(),
    );

    // Initially empty
    assert!(bot.bot_name().is_empty());

    // Fetch and set
    let info = bot.get_me().await.unwrap();
    bot.set_bot_name(info.full_name.clone());
    assert_eq!(bot.bot_name(), "MyBot");
}

// ── Mention Pattern Tests ─────────────────────────────────────────

#[test]
fn test_bot_mention_pattern_matches_bot_only() {
    let pattern = nerdbot::zulip::bot::build_bot_mention_pattern("NerdBot").unwrap();

    // Should match bot mention
    assert!(pattern.is_match("@**NerdBot** hello world"));
    assert!(pattern.is_match("@**NerdBot**"));

    // Should NOT match other mentions
    assert!(!pattern.is_match("@**Alice** hello"));
    // @**Bob** @**NerdBot** — the pattern only matches at the start, so this won't match
    assert!(!pattern.is_match("@**Bob** @**NerdBot** hello"));

    // Should not match without mention syntax
    assert!(!pattern.is_match("hello world"));
}

#[test]
fn test_bot_mention_pattern_replaces_only_bot_mention() {
    let pattern = nerdbot::zulip::bot::build_bot_mention_pattern("NerdBot").unwrap();
    let text = "@**NerdBot** hello world";
    let result = pattern.replace(text, "").to_string();
    assert_eq!(result, "hello world");
}

#[test]
fn test_bot_mention_pattern_no_match_returns_original() {
    let pattern = nerdbot::zulip::bot::build_bot_mention_pattern("NerdBot").unwrap();
    let text = "@**Alice** hello";
    let result = pattern.replace(text, "").to_string();
    assert_eq!(result, "@**Alice** hello");
}

#[test]
fn test_build_bot_mention_pattern_with_empty_name() {
    let result = nerdbot::zulip::bot::build_bot_mention_pattern("");
    assert!(result.is_none());
}

#[test]
fn test_build_bot_mention_pattern_escapes_special_chars() {
    // Bot names with special regex characters should be escaped
    let pattern = nerdbot::zulip::bot::build_bot_mention_pattern("Bot-With-Dashes").unwrap();
    assert!(pattern.is_match("@**Bot-With-Dashes** hello"));
    // Should not match unescaped version
    assert!(!pattern.is_match("@**Bot*With*Dashes** hello"));
}

// ── Attachment Processing Tests ───────────────────────────────────

#[tokio::test]
async fn test_attachment_processing_with_user_uploads() {
    let mock_server = MockServer::start().await;

    // Mock the user_uploads endpoint (at site root, not under /api/v1/)
    Mock::given(method("GET"))
        .and(path_regex(r"/user_uploads/1/99/abc/report\.pdf"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"%PDF-1.4 fake pdf content"))
        .mount(&mock_server)
        .await;

    // Mock the user_uploads endpoint for image
    Mock::given(method("GET"))
        .and(path_regex(r"/user_uploads/2/88/xyz/photo\.jpg"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes([0xFF, 0xD8, 0xFF, 0xE0]))
        .mount(&mock_server)
        .await;

    let bot = ZulipBot::new(
        mock_base_url(&mock_server),
        "bot@test.com".into(),
        "key".into(),
    );

    let raw_content = r#"Check out [report.pdf](/user_uploads/1/99/abc/report.pdf) and [photo.jpg](/user_uploads/2/88/xyz/photo.jpg)"#;

    let (clean_text, parts, _infos) =
        process_inbound_attachments(raw_content, &bot, 5_242_880, 32_768)
            .await
            .unwrap();

    // Should have cleaned the text
    assert!(!clean_text.contains("/user_uploads/"));
    assert!(clean_text.contains("[Attachment: report.pdf]"));
    assert!(clean_text.contains("[Attachment: photo.jpg]"));

    // Should have 2 attachment parts
    assert_eq!(parts.len(), 2);
}

#[tokio::test]
async fn test_attachment_processing_exceeds_size_limit() {
    let mock_server = MockServer::start().await;

    // Mock a large file that exceeds the limit
    // Use a broad matcher since wiremock path_regex may not match double-slash URLs
    Mock::given(method("GET"))
        .and(path_regex(r"user_uploads.*large"))
        .respond_with(
            ResponseTemplate::new(200).set_body_bytes(vec![0u8; 10_000_000]), // 10 MB
        )
        .mount(&mock_server)
        .await;

    let bot = ZulipBot::new(
        mock_base_url(&mock_server),
        "bot@test.com".into(),
        "key".into(),
    );

    let raw_content = "Check [large.pdf](/user_uploads/1/99/abc/large.pdf)";

    let result = process_inbound_attachments(raw_content, &bot, 5_000_000, 32_768).await;
    match &result {
        Ok((clean_text, parts, _infos)) => {
            // Large file should be skipped (not in parts)
            assert_eq!(parts.len(), 0, "parts should be empty, got {}", parts.len());
            // Text should still be cleaned
            assert!(
                !clean_text.contains("/user_uploads/"),
                "clean_text still contains /user_uploads/: {:?}",
                clean_text
            );
        }
        Err(e) => panic!("process_inbound_attachments failed: {:?}", e),
    }
}

#[tokio::test]
async fn test_attachment_text_document_truncation() {
    let mock_server = MockServer::start().await;

    // Mock a text file with content exceeding the limit
    let long_content = "x".repeat(50_000);
    Mock::given(method("GET"))
        .and(path_regex(r"/user_uploads/1/99/abc/long\.txt"))
        .respond_with(ResponseTemplate::new(200).set_body_string(long_content))
        .mount(&mock_server)
        .await;

    let bot = ZulipBot::new(
        mock_base_url(&mock_server),
        "bot@test.com".into(),
        "key".into(),
    );

    let raw_content = r#"See [long.txt](/user_uploads/1/99/abc/long.txt)"#;

    let (_clean_text, parts, _infos) =
        process_inbound_attachments(raw_content, &bot, 5_242_880, 100)
            .await
            .unwrap();

    assert_eq!(parts.len(), 1);
    // Should contain truncation notice
    let text = parts[0].as_text().unwrap();
    assert!(text.contains("Truncated"));
    assert!(text.contains("100 character limit"));
}

// ── Address Resolution Tests ──────────────────────────────────────

#[test]
fn test_resolve_stream_address() {
    let msg = ZulipMessage {
        id: 1,
        sender_id: 100,
        sender_email: "user@test.com".into(),
        sender_full_name: "Test User".into(),
        content: "hello".into(),
        message_type: "stream".into(),
        display_recipient: ZulipDisplayRecipient::Stream("engineering".into()),
        subject: Some("deployment".into()),
        stream_id: Some(42),
    };

    let address = resolve_zulip_address(&msg, "bot@test.com");
    assert_eq!(address.channel_id, "zulip");
    assert_eq!(address.conversation_id, "engineering");
    assert_eq!(address.thread_id, Some("deployment".into()));
}

#[test]
fn test_resolve_stream_address_no_topic() {
    let msg = ZulipMessage {
        id: 1,
        sender_id: 100,
        sender_email: "user@test.com".into(),
        sender_full_name: "Test User".into(),
        content: "hello".into(),
        message_type: "stream".into(),
        display_recipient: ZulipDisplayRecipient::Stream("general".into()),
        subject: None,
        stream_id: Some(1),
    };

    let address = resolve_zulip_address(&msg, "bot@test.com");
    assert_eq!(address.conversation_id, "general");
    assert_eq!(address.thread_id, Some("general".into()));
}

#[test]
fn test_resolve_private_message_address() {
    let msg = ZulipMessage {
        id: 1,
        sender_id: 100,
        sender_email: "alice@test.com".into(),
        sender_full_name: "Alice".into(),
        content: "hello".into(),
        message_type: "private".into(),
        display_recipient: ZulipDisplayRecipient::Private(vec![
            ZulipPrivateRecipient {
                id: 100,
                email: "alice@test.com".into(),
                full_name: "Alice".into(),
            },
            ZulipPrivateRecipient {
                id: 200,
                email: "bot@test.com".into(),
                full_name: "Bot".into(),
            },
        ]),
        subject: None,
        stream_id: None,
    };

    let address = resolve_zulip_address(&msg, "bot@test.com");
    assert_eq!(address.channel_id, "zulip");
    // Emails should be sorted and comma-separated, excluding the bot
    assert_eq!(address.conversation_id, "alice@test.com");
    assert_eq!(address.thread_id, None);

    let recipient_ids = resolve_zulip_private_recipient_ids(&msg, "bot@test.com");
    assert_eq!(recipient_ids, vec![100]);
}

#[test]
fn test_resolve_private_message_multi_participant() {
    let msg = ZulipMessage {
        id: 1,
        sender_id: 100,
        sender_email: "bob@test.com".into(),
        sender_full_name: "Bob".into(),
        content: "hello".into(),
        message_type: "private".into(),
        display_recipient: ZulipDisplayRecipient::Private(vec![
            ZulipPrivateRecipient {
                id: 100,
                email: "bob@test.com".into(),
                full_name: "Bob".into(),
            },
            ZulipPrivateRecipient {
                id: 200,
                email: "alice@test.com".into(),
                full_name: "Alice".into(),
            },
            ZulipPrivateRecipient {
                id: 300,
                email: "bot@test.com".into(),
                full_name: "Bot".into(),
            },
        ]),
        subject: None,
        stream_id: None,
    };

    let address = resolve_zulip_address(&msg, "bot@test.com");
    // Should be sorted: alice@test.com,bob@test.com
    assert_eq!(address.conversation_id, "alice@test.com,bob@test.com");
    assert_eq!(address.thread_id, None);

    let recipient_ids = resolve_zulip_private_recipient_ids(&msg, "bot@test.com");
    assert_eq!(recipient_ids, vec![100, 200]);
}

#[test]
fn test_resolve_private_message_excludes_current_user_by_id_when_email_differs() {
    let msg = ZulipMessage {
        id: 1,
        sender_id: 100,
        sender_email: "alice@test.com".into(),
        sender_full_name: "Alice".into(),
        content: "hello".into(),
        message_type: "private".into(),
        display_recipient: ZulipDisplayRecipient::Private(vec![
            ZulipPrivateRecipient {
                id: 100,
                email: "alice@test.com".into(),
                full_name: "Alice".into(),
            },
            ZulipPrivateRecipient {
                id: 200,
                email: "hidden-bot-email@test.com".into(),
                full_name: "NerdBot User".into(),
            },
        ]),
        subject: None,
        stream_id: None,
    };

    let address = resolve_zulip_address_for_user(&msg, "login-email@test.com", Some(200));
    assert_eq!(address.conversation_id, "alice@test.com");

    let recipient_ids =
        resolve_zulip_private_recipient_ids_for_user(&msg, "login-email@test.com", Some(200));
    assert_eq!(recipient_ids, vec![100]);
}

#[test]
fn test_zulip_bot_detects_own_message_by_user_id_when_email_differs() {
    let bot = ZulipBot::new(
        "https://test.zulipchat.com".into(),
        "login-email@test.com".into(),
        "key".into(),
    );
    bot.set_user_id(200);

    let msg = ZulipMessage {
        id: 1,
        sender_id: 200,
        sender_email: "hidden-bot-email@test.com".into(),
        sender_full_name: "NerdBot User".into(),
        content: "hello".into(),
        message_type: "private".into(),
        display_recipient: ZulipDisplayRecipient::Private(vec![ZulipPrivateRecipient {
            id: 200,
            email: "hidden-bot-email@test.com".into(),
            full_name: "NerdBot User".into(),
        }]),
        subject: None,
        stream_id: None,
    };

    assert!(bot.is_own_message(&msg));
}

// ── Mention Stripping Tests ───────────────────────────────────────

#[test]
fn test_strip_bot_mention_precise() {
    let text = "@**NerdBot** hello world";
    let result = strip_bot_mention(text, "NerdBot");
    assert_eq!(result, "hello world");
}

#[test]
fn test_strip_bot_mention_no_mention() {
    let text = "hello world";
    let result = strip_bot_mention(text, "NerdBot");
    assert_eq!(result, "hello world");
}

#[test]
fn test_strip_bot_mention_other_mention_preserved() {
    let text = "@**NerdBot** @**Alice** hello";
    let result = strip_bot_mention(text, "NerdBot");
    // Should only strip NerdBot mention at the start, not Alice's
    assert_eq!(result, "@**Alice** hello");
}

#[test]
fn test_strip_bot_mention_broad_fallback() {
    // When bot name is empty, uses broad pattern (may strip wrong mention)
    let text = "@**Alice** hello";
    let result = strip_bot_mention(text, "");
    // Broad pattern strips any first mention
    assert_eq!(result, "hello");
}

// ── URL Normalization Tests ───────────────────────────────────────

#[tokio::test]
async fn test_site_url_normalization_with_trailing_slash() {
    let bot = ZulipBot::new(
        "https://test.zulipchat.com/".into(),
        "bot@test.com".into(),
        "key".into(),
    );
    assert!(bot.site_url().ends_with('/'));
}

#[tokio::test]
async fn test_site_url_normalization_without_trailing_slash() {
    let bot = ZulipBot::new(
        "https://test.zulipchat.com".into(),
        "bot@test.com".into(),
        "key".into(),
    );
    assert!(bot.site_url().ends_with('/'));
}

// ── BAD_EVENT_QUEUE_ID Error Tests ────────────────────────────────

#[tokio::test]
async fn test_get_events_bad_queue_id_error() {
    let mock_server = MockServer::start().await;

    // Real Zulip BAD_EVENT_QUEUE_ID error response does NOT include an "events" field.
    Mock::given(method("GET"))
        .and(path_regex("/api/v1/events"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"result": "error", "msg": "Bad event queue id", "code": "BAD_EVENT_QUEUE_ID"}"#,
        ))
        .mount(&mock_server)
        .await;

    let bot = ZulipBot::new(
        mock_base_url(&mock_server),
        "bot@test.com".into(),
        "key".into(),
    );

    let result = bot.get_events("expired-queue", 0).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    let err_str = format!("{:?}", err);
    assert!(err_str.contains("BAD_EVENT_QUEUE_ID"));
}

// ── Zulip API Error Response Tests (HTTP 200 with result="error") ───

#[tokio::test]
async fn test_register_queue_api_error() {
    let mock_server = MockServer::start().await;

    // Zulip returns HTTP 200 with result="error" and no queue_id/last_event_id
    Mock::given(method("POST"))
        .and(path_regex("/api/v1/register"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"result": "error", "msg": "Bad event queue id", "code": "BAD_EVENT_QUEUE_ID"}"#,
        ))
        .mount(&mock_server)
        .await;

    let bot = ZulipBot::new(
        mock_base_url(&mock_server),
        "bot@test.com".into(),
        "key".into(),
    );

    let result = bot.register_queue().await;
    assert!(result.is_err(), "register_queue should fail on API error");
    let err = result.unwrap_err();
    let err_str = format!("{:?}", err);
    assert!(
        err_str.contains("BAD_EVENT_QUEUE_ID"),
        "error should contain Zulip error code: {}",
        err_str
    );
}

#[tokio::test]
async fn test_send_message_api_error() {
    let mock_server = MockServer::start().await;

    // Zulip returns HTTP 200 with result="error" and no id field
    Mock::given(method("POST"))
        .and(path_regex("/api/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(
                r#"{"result": "error", "msg": "Could not find the target stream"}"#,
            ),
        )
        .mount(&mock_server)
        .await;

    let bot = ZulipBot::new(
        mock_base_url(&mock_server),
        "bot@test.com".into(),
        "key".into(),
    );

    let recipient = ZulipRecipient::Stream {
        stream_name: "nonexistent".into(),
        topic: "test".into(),
    };
    let result = bot.send_message(&recipient, "hello").await;
    assert!(result.is_err(), "send_message should fail on API error");
    let err = result.unwrap_err();
    let err_str = format!("{:?}", err);
    assert!(
        err_str.contains("Could not find the target stream"),
        "error should contain Zulip error message: {}",
        err_str
    );
}

#[tokio::test]
async fn test_typing_notification_api_error() {
    let mock_server = MockServer::start().await;

    // Zulip returns HTTP 200 with result="error"
    Mock::given(method("POST"))
        .and(path_regex("/api/v1/typing"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(r#"{"result": "error", "msg": "Not a private message"}"#),
        )
        .mount(&mock_server)
        .await;

    let bot = ZulipBot::new(
        mock_base_url(&mock_server),
        "bot@test.com".into(),
        "key".into(),
    );

    let result = bot.send_typing_notification(&[100], "start").await;
    assert!(
        result.is_err(),
        "send_typing_notification should fail on API error"
    );
    let err = result.unwrap_err();
    let err_str = format!("{:?}", err);
    assert!(
        err_str.contains("Not a private message"),
        "error should contain Zulip error message: {}",
        err_str
    );
}

// ── Malformed Success Response Tests ──────────────────────────────

#[tokio::test]
async fn test_register_queue_missing_queue_id() {
    let mock_server = MockServer::start().await;

    // Zulip returns HTTP 200 with result="success" but missing queue_id/last_event_id
    Mock::given(method("POST"))
        .and(path_regex("/api/v1/register"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(r#"{"result": "success", "msg": ""}"#),
        )
        .mount(&mock_server)
        .await;

    let bot = ZulipBot::new(
        mock_base_url(&mock_server),
        "bot@test.com".into(),
        "key".into(),
    );

    let result = bot.register_queue().await;
    assert!(
        result.is_err(),
        "register_queue should fail when queue_id is empty"
    );
    let err = result.unwrap_err();
    let err_str = format!("{:?}", err);
    assert!(
        err_str.contains("empty queue_id"),
        "error should mention empty queue_id: {}",
        err_str
    );
}

#[tokio::test]
async fn test_register_queue_missing_last_event_id() {
    let mock_server = MockServer::start().await;

    // Zulip returns HTTP 200 with result="success" and queue_id but missing last_event_id
    Mock::given(method("POST"))
        .and(path_regex("/api/v1/register"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(r#"{"result": "success", "msg": "", "queue_id": "abc123"}"#),
        )
        .mount(&mock_server)
        .await;

    let bot = ZulipBot::new(
        mock_base_url(&mock_server),
        "bot@test.com".into(),
        "key".into(),
    );

    let result = bot.register_queue().await;
    assert!(
        result.is_err(),
        "register_queue should fail when last_event_id is missing"
    );
    let err = result.unwrap_err();
    let err_str = format!("{:?}", err);
    assert!(
        err_str.contains("missing last_event_id"),
        "error should mention missing last_event_id: {}",
        err_str
    );
}
