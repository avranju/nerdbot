use async_trait::async_trait;

use crate::channel::types::{ChannelInboundEvent, ConversationAddress, OutboundMessage};
use crate::error::AgentError;

pub struct ChannelTypingIndicator {
    refresh_task: tokio::task::JoinHandle<()>,
    on_drop: Option<Box<dyn FnOnce() + Send + 'static>>,
}

impl ChannelTypingIndicator {
    pub fn new(refresh_task: tokio::task::JoinHandle<()>) -> Self {
        Self {
            refresh_task,
            on_drop: None,
        }
    }

    pub fn with_on_drop(
        refresh_task: tokio::task::JoinHandle<()>,
        on_drop: impl FnOnce() + Send + 'static,
    ) -> Self {
        Self {
            refresh_task,
            on_drop: Some(Box::new(on_drop)),
        }
    }
}

impl Drop for ChannelTypingIndicator {
    fn drop(&mut self) {
        self.refresh_task.abort();
        if let Some(on_drop) = self.on_drop.take() {
            on_drop();
        }
    }
}

#[async_trait]
pub trait ChannelService: Send + Sync {
    fn channel_id(&self) -> &str;

    async fn send_message(
        &self,
        address: &ConversationAddress,
        message: OutboundMessage,
    ) -> Result<(), AgentError>;

    fn start_typing(&self, _address: &ConversationAddress) -> Option<ChannelTypingIndicator> {
        None
    }
}

#[async_trait]
pub trait ChannelIngress: Send {
    /// Initialize the ingress (e.g., register an event queue or webhook URL).
    async fn init(&mut self) -> Result<(), AgentError>;

    /// Return the next inbound event from the channel.
    ///
    /// **Implementation note:** For Zulip, this compatibility method returns
    /// raw/unprocessed events without attachment parts. Runtime wiring should use
    /// the ZulipUpdate raw-message trait plus `handle_zulip_message()` to enforce
    /// two-phase handling (access policy before attachment download). Do not call
    /// `ChannelMessageHandler::handle_event()` directly on Zulip events from this
    /// method.
    async fn next_event(&mut self) -> Result<Option<ChannelInboundEvent>, AgentError>;
}
