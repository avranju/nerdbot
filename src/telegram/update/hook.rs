use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;
use tracing::info;

use crate::config::TelegramChannelConfig;
use crate::error::AgentError;
use crate::telegram::bot::{TelegramBot, Update};

use super::TelegramUpdate;

/// Telegram update ingress via webhook push.
///
/// The shared webhook server owns the HTTP listener and validates incoming
/// requests. This type registers the public webhook with Telegram and consumes
/// validated updates from the shared server queue.
pub struct TelegramHook {
    bot: Arc<TelegramBot>,
    config: TelegramChannelConfig,
    secret_token: String,
    receiver: mpsc::Receiver<Update>,
}

impl TelegramHook {
    pub fn new(
        bot: Arc<TelegramBot>,
        config: TelegramChannelConfig,
        secret_token: String,
        receiver: mpsc::Receiver<Update>,
    ) -> Self {
        Self {
            bot,
            config,
            secret_token,
            receiver,
        }
    }
}

#[async_trait]
impl TelegramUpdate for TelegramHook {
    async fn init(&mut self) -> Result<(), AgentError> {
        let webhook_url = self.config.web_hook_url.clone().ok_or_else(|| {
            AgentError::Config(
                "channels.telegram.web_hook_url is required when channels.telegram.ingress is \"webhook\"".into(),
            )
        })?;

        self.bot
            .set_webhook(&webhook_url, &self.secret_token)
            .await?;

        info!("Telegram webhook ingress initialized");
        Ok(())
    }

    async fn poll(&mut self) -> Result<Option<Update>, AgentError> {
        self.receiver
            .recv()
            .await
            .map(Some)
            .ok_or_else(|| AgentError::Telegram("Telegram webhook update channel closed".into()))
    }
}
