//! Zulip channel service — outbound messaging with typing indicators.
//!
//! Wraps ZulipBot to provide a clean interface for sending messages,
//! automatically splitting them if they exceed Zulip's 10,000-character limit.

use std::sync::Arc;
use std::time::Duration;

use tracing::{debug, warn};

use super::bot::{ZulipBot, ZulipRecipient};
use crate::channel::{
    ChannelService, ChannelTypingIndicator, ConversationAddress, OutboundMessage,
};
use crate::error::AgentError;

/// Maximum message length Zulip accepts (10,000 UTF-8 code points).
pub const ZULIP_MAX_MESSAGE_LENGTH: usize = 10_000;

/// High-level service for sending messages through Zulip.
#[derive(Debug, Clone)]
pub struct ZulipService {
    bot: Arc<ZulipBot>,
}

impl ZulipService {
    /// Create a new service wrapping a Zulip bot client.
    pub fn new(bot: Arc<ZulipBot>) -> Self {
        Self { bot }
    }

    /// Get a reference to the underlying bot.
    pub fn bot(&self) -> &Arc<ZulipBot> {
        &self.bot
    }
}

#[async_trait::async_trait]
impl ChannelService for ZulipService {
    fn channel_id(&self) -> &str {
        "zulip"
    }

    async fn send_message(
        &self,
        address: &ConversationAddress,
        message: OutboundMessage,
    ) -> Result<(), AgentError> {
        let recipient = if let Some(ref topic) = address.thread_id {
            ZulipRecipient::Stream {
                stream_name: address.conversation_id.clone(),
                topic: topic.clone(),
            }
        } else {
            // Private message — conversation_id is comma-separated email list
            let emails: Vec<String> = address
                .conversation_id
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            ZulipRecipient::Private { emails }
        };

        let text = message.text;
        if text.chars().count() <= ZULIP_MAX_MESSAGE_LENGTH {
            debug!(
                recipient_type = ?recipient,
                text_len = text.chars().count(),
                "sending Zulip message"
            );
            self.bot.send_message(&recipient, &text).await?;
        } else {
            // Split long message into chunks
            let chars: Vec<char> = text.chars().collect();
            for chunk in chars.chunks(ZULIP_MAX_MESSAGE_LENGTH) {
                let chunk_str: String = chunk.iter().collect();
                debug!(
                    recipient_type = ?recipient,
                    chunk_len = chunk_str.chars().count(),
                    "sending Zulip message chunk"
                );
                self.bot.send_message(&recipient, &chunk_str).await?;
            }
        }

        Ok(())
    }

    fn start_typing(&self, address: &ConversationAddress) -> Option<ChannelTypingIndicator> {
        // Zulip typing indicators are primarily supported for private messages.
        // Stream typing indicators are not reliably supported across all Zulip versions.
        if address.thread_id.is_some() {
            return None;
        }

        let Some(user_ids) = self.bot.typing_recipient_ids(&address.conversation_id) else {
            debug!(
                conversation_id = %address.conversation_id,
                "skipping Zulip typing notification: no cached user IDs"
            );
            return None;
        };

        let bot = self.bot.clone();
        let refresh_user_ids = user_ids.clone();
        let refresh_task = tokio::spawn(async move {
            loop {
                if let Err(e) = bot
                    .send_typing_notification(&refresh_user_ids, "start")
                    .await
                {
                    warn!(?refresh_user_ids, error = %e, "failed to send Zulip typing notification");
                }
                // Zulip typing indicators expire after ~10-15 seconds
                tokio::time::sleep(Duration::from_secs(8)).await;
            }
        });

        let bot = self.bot.clone();
        Some(ChannelTypingIndicator::with_on_drop(
            refresh_task,
            move || {
                tokio::spawn(async move {
                    if let Err(e) = bot.send_typing_notification(&user_ids, "stop").await {
                        warn!(?user_ids, error = %e, "failed to stop Zulip typing notification");
                    }
                });
            },
        ))
    }
}
