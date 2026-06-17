use std::collections::HashMap;
use std::sync::Arc;

use crate::channel::traits::{ChannelService, ChannelTypingIndicator};
use crate::channel::types::{ConversationAddress, OutboundMessage};
use crate::error::AgentError;

#[derive(Clone, Default)]
pub struct ChannelRegistry {
    services: Arc<HashMap<String, Arc<dyn ChannelService>>>,
}

impl std::fmt::Debug for ChannelRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChannelRegistry")
            .field("channels", &self.services.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl ChannelRegistry {
    pub fn new(services: Vec<Arc<dyn ChannelService>>) -> Self {
        let services = services
            .into_iter()
            .map(|service| (service.channel_id().to_string(), service))
            .collect();
        Self {
            services: Arc::new(services),
        }
    }

    pub fn get(&self, channel_id: &str) -> Option<Arc<dyn ChannelService>> {
        self.services.get(channel_id).cloned()
    }

    pub async fn send_message(
        &self,
        address: &ConversationAddress,
        message: OutboundMessage,
    ) -> Result<(), AgentError> {
        let service = self.get(&address.channel_id).ok_or_else(|| {
            AgentError::Generic(format!(
                "No communication channel configured for '{}'",
                address.channel_id
            ))
        })?;
        service.send_message(address, message).await
    }

    pub fn start_typing(&self, address: &ConversationAddress) -> Option<ChannelTypingIndicator> {
        self.get(&address.channel_id)
            .and_then(|service| service.start_typing(address))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;

    use super::*;
    use crate::channel::{MessageFormat, SenderIdentity};

    #[derive(Default)]
    struct FakeChannel {
        sent: Arc<Mutex<Vec<(ConversationAddress, OutboundMessage)>>>,
    }

    #[async_trait]
    impl ChannelService for FakeChannel {
        fn channel_id(&self) -> &str {
            "fake"
        }

        async fn send_message(
            &self,
            address: &ConversationAddress,
            message: OutboundMessage,
        ) -> Result<(), AgentError> {
            self.sent.lock().unwrap().push((address.clone(), message));
            Ok(())
        }
    }

    #[test]
    fn telegram_address_uses_string_conversation_id() {
        let address = ConversationAddress::telegram_chat(123);
        assert_eq!(address.channel_id, "telegram");
        assert_eq!(address.conversation_id, "123");
        assert_eq!(address.thread_id, None);
    }

    #[test]
    fn sender_identity_records_optional_display_name() {
        let sender = SenderIdentity::new("42", Some("Ada".into()));
        assert_eq!(sender.sender_id, "42");
        assert_eq!(sender.display_name.as_deref(), Some("Ada"));
    }

    #[tokio::test]
    async fn registry_routes_message_by_channel_id() {
        let fake = Arc::new(FakeChannel::default());
        let sent = fake.sent.clone();
        let registry = ChannelRegistry::new(vec![fake]);
        let address = ConversationAddress::new("fake", "room", Some("topic".into()));

        registry
            .send_message(
                &address,
                OutboundMessage {
                    text: "hello".into(),
                    format: MessageFormat::PlainText,
                    disable_notification: None,
                },
            )
            .await
            .unwrap();

        let sent = sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0, address);
        assert_eq!(sent[0].1.text, "hello");
    }
}
