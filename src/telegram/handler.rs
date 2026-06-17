//! Telegram-facing compatibility adapter over the generic channel message handler.

use std::sync::Arc;

pub use crate::channel::handler::build_persist_text;
use crate::channel::handler::{
    ChannelMessageHandler, ChannelMessageHandlerInput as GenericMessageHandlerInput,
};
pub use crate::channel::{AttachmentInfo, InboundMessage};
use crate::channel::{ChannelRegistry, ConversationAddress, SenderIdentity};
use crate::error::AgentError;

pub struct MessageHandler {
    inner: ChannelMessageHandler,
}

pub struct MessageHandlerInput {
    pub pool: sqlx::SqlitePool,
    pub llm: Arc<dyn crate::llm::LlmExecutor>,
    pub registry: Arc<crate::tools::registry::ToolRegistry>,
    pub config: crate::config::AppConfig,
    pub personality: crate::agent::personality::Personality,
    pub scheduler_notifier: Option<Arc<tokio::sync::Notify>>,
    pub telegram_service: Option<crate::telegram::service::TelegramService>,
    pub compaction_service: Arc<crate::context::compaction_service::CompactionService>,
}

impl MessageHandler {
    pub fn new(input: MessageHandlerInput) -> Self {
        let services = input
            .telegram_service
            .map(|service| {
                vec![Arc::new(service) as Arc<dyn crate::channel::traits::ChannelService>]
            })
            .unwrap_or_default();

        let inner = ChannelMessageHandler::new(GenericMessageHandlerInput {
            pool: input.pool,
            llm: input.llm,
            registry: input.registry,
            channel_registry: Arc::new(ChannelRegistry::new(services)),
            config: input.config,
            personality: input.personality,
            scheduler_notifier: input.scheduler_notifier,
            compaction_service: input.compaction_service,
        });

        Self { inner }
    }

    pub async fn handle_message(
        &self,
        chat_id: i64,
        user_id: i64,
        text: &str,
    ) -> Result<Option<String>, AgentError> {
        let inbound = InboundMessage {
            text: text.to_string(),
            attachments: Vec::new(),
            attachment_parts: Vec::new(),
        };
        self.handle_rich_message(chat_id, user_id, &inbound).await
    }

    pub async fn handle_rich_message(
        &self,
        chat_id: i64,
        user_id: i64,
        inbound: &InboundMessage,
    ) -> Result<Option<String>, AgentError> {
        let address = ConversationAddress::telegram_chat(chat_id);
        let sender = SenderIdentity::new(user_id.to_string(), None);
        self.inner
            .handle_rich_message(&address, &sender, inbound)
            .await
    }
}
