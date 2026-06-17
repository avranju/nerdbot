use async_trait::async_trait;

use crate::channel::types::{ChannelInboundEvent, ConversationAddress, OutboundMessage};
use crate::error::AgentError;

pub struct ChannelTypingIndicator {
    refresh_task: tokio::task::JoinHandle<()>,
}

impl ChannelTypingIndicator {
    pub fn new(refresh_task: tokio::task::JoinHandle<()>) -> Self {
        Self { refresh_task }
    }
}

impl Drop for ChannelTypingIndicator {
    fn drop(&mut self) {
        self.refresh_task.abort();
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
    async fn init(&mut self) -> Result<(), AgentError>;

    async fn next_event(&mut self) -> Result<Option<ChannelInboundEvent>, AgentError>;
}
