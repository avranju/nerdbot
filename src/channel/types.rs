use genai::chat::ContentPart;
use serde::{Deserialize, Serialize};

pub type ChannelId = String;

/// Pattern for matching a conversation address against an allowlist.
///
/// The pattern supports three matching modes:
/// - `channel_id` must always match exactly.
/// - `conversation_id` must always match exactly.
/// - `thread_id` controls thread granularity:
///   - `Some(id)` — only matches when the target thread equals `id`.
///   - `None` — matches any thread (acts as a wildcard over threads
///     within the channel + conversation).
///
/// This design lets a Zulip stream owner allow "all topics in stream 42"
/// by setting `thread_id: None`, while a Telegram group owner can lock
/// down to a specific reply thread with `thread_id: Some("abc")`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ConversationAddressPattern {
    pub channel_id: ChannelId,
    pub conversation_id: String,
    /// `None` means "any thread"; `Some(id)` means exact thread match.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
}

impl ConversationAddressPattern {
    /// Create a new pattern with no thread restriction (matches all threads).
    pub fn new(channel_id: impl Into<String>, conversation_id: impl Into<String>) -> Self {
        Self {
            channel_id: channel_id.into(),
            conversation_id: conversation_id.into(),
            thread_id: None,
        }
    }

    /// Create a new pattern with an explicit thread restriction.
    pub fn with_thread(
        channel_id: impl Into<String>,
        conversation_id: impl Into<String>,
        thread_id: String,
    ) -> Self {
        Self {
            channel_id: channel_id.into(),
            conversation_id: conversation_id.into(),
            thread_id: Some(thread_id),
        }
    }

    /// Check whether this pattern matches the given [`ConversationAddress`].
    ///
    /// Matching rules:
    /// 1. `channel_id` must match exactly.
    /// 2. `conversation_id` must match exactly.
    /// 3. If the pattern's `thread_id` is `Some`, the target's `thread_id` must
    ///    also be `Some` and equal.
    /// 4. If the pattern's `thread_id` is `None`, any target thread is accepted
    ///    (wildcard over threads).
    pub fn matches(&self, address: &ConversationAddress) -> bool {
        self.channel_id == address.channel_id
            && self.conversation_id == address.conversation_id
            && match &self.thread_id {
                Some(pat_thread) => address.thread_id.as_ref() == Some(pat_thread),
                None => true, // wildcard: any thread is fine
            }
    }
}

/// Channel-qualified access policy for a single communication channel.
///
/// When `allowed_conversations` is empty, all conversations on that channel
/// are permitted. When configured, only conversations matching at least one
/// pattern are allowed.
#[derive(Debug, Clone, Default)]
pub struct ChannelAccessPolicy {
    /// Patterns that qualify a conversation for tool execution and messaging.
    pub allowed_conversations: Vec<ConversationAddressPattern>,
    /// Sender/user IDs that are permitted on this channel.
    pub allowed_senders: Vec<String>,
}

impl ChannelAccessPolicy {
    /// Create a policy that allows all conversations and senders.
    pub fn allow_all() -> Self {
        Self::default()
    }

    /// Check whether the given address and sender are permitted.
    ///
    /// - If `allowed_conversations` is empty, any conversation is allowed.
    /// - If configured, the address must match at least one pattern.
    /// - If `allowed_senders` is empty, any sender is allowed.
    /// - If configured, the sender must be in the list.
    pub fn is_allowed(&self, address: &ConversationAddress, sender: &SenderIdentity) -> bool {
        let conv_allowed = self.allowed_conversations.is_empty()
            || self
                .allowed_conversations
                .iter()
                .any(|p| p.matches(address));
        let sender_allowed =
            self.allowed_senders.is_empty() || self.allowed_senders.contains(&sender.sender_id);
        conv_allowed && sender_allowed
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ConversationAddress {
    pub channel_id: ChannelId,
    pub conversation_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
}

impl ConversationAddress {
    pub fn new(
        channel_id: impl Into<String>,
        conversation_id: impl Into<String>,
        thread_id: Option<String>,
    ) -> Self {
        Self {
            channel_id: channel_id.into(),
            conversation_id: conversation_id.into(),
            thread_id,
        }
    }

    pub fn telegram_chat(chat_id: i64) -> Self {
        Self::new("telegram", chat_id.to_string(), None)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SenderIdentity {
    pub sender_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}

impl SenderIdentity {
    pub fn new(sender_id: impl Into<String>, display_name: Option<String>) -> Self {
        Self {
            sender_id: sender_id.into(),
            display_name,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AttachmentInfo {
    pub display_name: String,
    pub mime_type: String,
    pub size_bytes: u64,
    pub downloaded: bool,
    pub persistence_marker: String,
    pub extracted_text: Option<String>,
}

#[derive(Debug, Clone)]
pub struct InboundMessage {
    pub text: String,
    pub attachments: Vec<AttachmentInfo>,
    pub attachment_parts: Vec<ContentPart>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageFormat {
    PlainText,
    Markdown,
    MarkdownRaw,
}

#[derive(Debug, Clone)]
pub struct OutboundMessage {
    pub text: String,
    pub format: MessageFormat,
    pub disable_notification: Option<bool>,
}

impl OutboundMessage {
    pub fn markdown(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            format: MessageFormat::Markdown,
            disable_notification: None,
        }
    }

    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            format: MessageFormat::PlainText,
            disable_notification: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ChannelInboundEvent {
    pub address: ConversationAddress,
    pub sender: SenderIdentity,
    pub message: InboundMessage,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ChannelConfig {
    #[serde(default)]
    pub telegram: crate::config::TelegramChannelConfig,
    #[serde(default)]
    pub zulip: crate::config::ZulipChannelConfig,
}

impl ChannelConfig {
    /// Derive a channel-qualified access policy from a channel-specific config.
    ///
    /// Currently supports `"telegram"` only. Unknown channel IDs return an
    /// empty (allow-all) policy so that future channels don't break startup.
    pub fn access_policy_for(&self, channel_id: &str) -> ChannelAccessPolicy {
        match channel_id {
            "telegram" => {
                let telegram = &self.telegram;
                let allowed_conversations = telegram
                    .allowed_conversations
                    .iter()
                    .map(|cid| ConversationAddressPattern {
                        channel_id: "telegram".to_string(),
                        conversation_id: cid.clone(),
                        thread_id: None,
                    })
                    .collect();
                ChannelAccessPolicy {
                    allowed_conversations,
                    allowed_senders: telegram.allowed_senders.clone(),
                }
            }
            "zulip" => {
                let zulip = &self.zulip;
                ChannelAccessPolicy {
                    allowed_conversations: zulip.allowed_conversations.clone(),
                    allowed_senders: zulip.allowed_senders.clone(),
                }
            }
            _ => ChannelAccessPolicy::default(),
        }
    }
}
