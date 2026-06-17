//! Zulip outgoing webhook ingress.
//!
//! The shared webhook server validates HTTP requests and forwards accepted
//! payloads through an mpsc queue. This ingress consumes that queue as raw
//! Zulip messages so the channel handler can enforce access policy before
//! downloading attachments.

use serde::Deserialize;
use tokio::sync::mpsc;

use crate::error::AgentError;
use crate::zulip::bot::ZulipMessage;

use super::ZulipUpdate;

/// Webhook payload sent by Zulip for outgoing webhooks.
#[derive(Debug, Deserialize)]
pub struct ZulipWebhookPayload {
    pub token: String,
    pub bot_email: String,
    pub message: ZulipMessage,
}

/// Queue-backed Zulip webhook ingress implementation.
pub struct ZulipHook {
    rx: mpsc::Receiver<ZulipWebhookPayload>,
}

impl ZulipHook {
    pub fn new(rx: mpsc::Receiver<ZulipWebhookPayload>) -> Self {
        Self { rx }
    }
}

#[async_trait::async_trait]
impl ZulipUpdate for ZulipHook {
    async fn init(&mut self) -> Result<(), AgentError> {
        Ok(())
    }

    async fn poll(&mut self) -> Result<Option<ZulipMessage>, AgentError> {
        self.rx
            .recv()
            .await
            .map(|payload| payload.message)
            .map(Some)
            .ok_or_else(|| AgentError::Zulip("Zulip webhook update channel closed".into()))
    }
}
