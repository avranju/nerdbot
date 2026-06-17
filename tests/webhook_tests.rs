use std::time::Duration;

use nerdbot::webhook::{
    TelegramWebhookRoute, WebhookServer, WebhookServerConfig, ZulipWebhookRoute,
};
use tokio::sync::mpsc;

#[tokio::test]
async fn shared_webhook_server_serves_health_and_channel_routes() {
    let (telegram_tx, mut telegram_rx) = mpsc::channel(2);
    let (zulip_tx, mut zulip_rx) = mpsc::channel(2);

    let mut server = WebhookServer::start(WebhookServerConfig {
        host: "127.0.0.1".into(),
        port: 0,
        telegram: Some(TelegramWebhookRoute {
            path: "/telegram/hook".into(),
            secret_token: "telegram-secret".into(),
            sender: telegram_tx,
        }),
        zulip: Some(ZulipWebhookRoute {
            path: "/zulip/hook".into(),
            token: "zulip-secret".into(),
            sender: zulip_tx,
        }),
    })
    .await
    .unwrap();

    let client = reqwest::Client::new();
    let base_url = format!("http://{}", server.local_addr());

    let health = client
        .get(format!("{base_url}/health"))
        .send()
        .await
        .unwrap();
    assert_eq!(health.status(), reqwest::StatusCode::OK);
    assert!(health.text().await.unwrap().contains("NerdBot is healthy"));

    let rejected_telegram = client
        .post(format!("{base_url}/telegram/hook"))
        .json(&serde_json::json!({ "update_id": 1 }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        rejected_telegram.status(),
        reqwest::StatusCode::UNAUTHORIZED
    );

    let accepted_telegram = client
        .post(format!("{base_url}/telegram/hook"))
        .header("x-telegram-bot-api-secret-token", "telegram-secret")
        .json(&serde_json::json!({
            "update_id": 42,
            "message": {
                "message_id": 7,
                "chat": { "id": 123, "type": "private" },
                "text": "hello"
            }
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(accepted_telegram.status(), reqwest::StatusCode::OK);
    let telegram_update = tokio::time::timeout(Duration::from_secs(1), telegram_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(telegram_update.update_id, 42);

    let accepted_zulip = client
        .post(format!("{base_url}/zulip/hook"))
        .json(&zulip_payload(55, "zulip-secret"))
        .send()
        .await
        .unwrap();
    assert_eq!(accepted_zulip.status(), reqwest::StatusCode::OK);
    assert_eq!(
        accepted_zulip.json::<serde_json::Value>().await.unwrap(),
        serde_json::json!({ "response_not_required": true })
    );
    let received_zulip = tokio::time::timeout(Duration::from_secs(1), zulip_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(received_zulip.message.id, 55);

    let rejected_zulip = client
        .post(format!("{base_url}/zulip/hook"))
        .json(&zulip_payload(57, "wrong-secret"))
        .send()
        .await
        .unwrap();
    assert_eq!(rejected_zulip.status(), reqwest::StatusCode::UNAUTHORIZED);

    server.stop().await;
}

#[tokio::test]
async fn shared_webhook_server_rejects_reserved_route_conflicts() {
    let (telegram_tx, _telegram_rx) = mpsc::channel(1);

    let err = match WebhookServer::start(WebhookServerConfig {
        host: "127.0.0.1".into(),
        port: 0,
        telegram: Some(TelegramWebhookRoute {
            path: "/health".into(),
            secret_token: "telegram-secret".into(),
            sender: telegram_tx,
        }),
        zulip: None,
    })
    .await
    {
        Ok(_) => panic!("expected reserved route conflict"),
        Err(err) => err,
    };

    assert!(err.to_string().contains("must not be /health"));
}

fn zulip_payload(message_id: i64, token: &str) -> serde_json::Value {
    serde_json::json!({
        "token": token,
        "bot_email": "bot@example.test",
        "message": {
            "id": message_id,
            "sender_id": 10,
            "sender_email": "user@example.test",
            "sender_full_name": "Test User",
            "content": "hello",
            "type": "stream",
            "display_recipient": "general",
            "subject": "ops",
            "stream_id": 1
        }
    })
}
