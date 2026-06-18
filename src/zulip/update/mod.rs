//! Zulip ingress — long polling and webhook event loaders.
//!
//! Provides shared utilities for both ingress modes: address resolution,
//! bot mention stripping, and attachment processing helpers.

use std::sync::LazyLock;

use regex::Regex;
use tracing::{debug, instrument};

use crate::channel::{ChannelInboundEvent, ConversationAddress, InboundMessage, SenderIdentity};
use crate::error::AgentError;
use crate::zulip::attachment::process_inbound_attachments;
use crate::zulip::bot::{ZulipBot, ZulipDisplayRecipient, ZulipMessage};

/// Lightweight pre-processing that resolves address and sender without downloading attachments.
/// Returns (address, sender_email, sender_full_name, content) for use by access policy checks.
pub fn resolve_zulip_message(
    msg: &ZulipMessage,
    bot_email: &str,
) -> (ConversationAddress, String, String, String) {
    let address = resolve_zulip_address(msg, bot_email);
    let sender_full_name = msg.sender_full_name.clone();
    let content = msg.content.clone();
    (address, msg.sender_email.clone(), sender_full_name, content)
}

/// Lightweight pre-processing using the authenticated user's stable Zulip ID.
pub fn resolve_zulip_message_for_user(
    msg: &ZulipMessage,
    bot_email: &str,
    bot_user_id: Option<i64>,
) -> (ConversationAddress, String, String, String) {
    let address = resolve_zulip_address_for_user(msg, bot_email, bot_user_id);
    let sender_full_name = msg.sender_full_name.clone();
    let content = msg.content.clone();
    (address, msg.sender_email.clone(), sender_full_name, content)
}

pub mod hook;
pub mod poll;

pub use hook::ZulipHook;
pub use poll::ZulipPoll;

/// Retrieves raw Zulip messages from a configured ingress transport.
///
/// Raw messages are yielded intentionally so the central channel handler can
/// resolve sender/address and enforce access policy before attachment downloads.
#[async_trait::async_trait]
pub trait ZulipUpdate {
    /// Perform transport initialization before polling begins.
    async fn init(&mut self) -> Result<(), AgentError>;

    /// Retrieve the next raw Zulip message, if one is currently available.
    async fn poll(&mut self) -> Result<Option<ZulipMessage>, AgentError>;
}

/// Pattern to strip ANY mention at the start of text (fallback when bot name is unknown).
static BROAD_MENTION_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^@\*\*.*?\*\*\s*").unwrap());

/// Resolve a Zulip message to a ConversationAddress.
///
/// - Stream messages: `conversation_id` = stream name, `thread_id` = topic
/// - Private messages: `conversation_id` = sorted comma-separated participant emails
pub fn resolve_zulip_address(msg: &ZulipMessage, bot_email: &str) -> ConversationAddress {
    resolve_zulip_address_for_user(msg, bot_email, None)
}

/// Resolve a Zulip message to a ConversationAddress, preferring the stable
/// authenticated user ID when available.
pub fn resolve_zulip_address_for_user(
    msg: &ZulipMessage,
    bot_email: &str,
    bot_user_id: Option<i64>,
) -> ConversationAddress {
    if msg.message_type == "stream" {
        let stream_name = match &msg.display_recipient {
            ZulipDisplayRecipient::Stream(s) => s.clone(),
            _ => msg.stream_id.map(|id| id.to_string()).unwrap_or_default(),
        };
        let topic = msg.subject.clone().unwrap_or_else(|| "general".to_string());
        ConversationAddress::new("zulip", stream_name, Some(topic))
    } else {
        let mut participants = match &msg.display_recipient {
            ZulipDisplayRecipient::Private(recs) => recs
                .iter()
                .map(|r| r.email.clone())
                .zip(recs.iter().map(|r| r.id))
                .filter(|(email, id)| Some(*id) != bot_user_id && email != bot_email)
                .map(|(email, _id)| email)
                .collect::<Vec<String>>(),
            _ => Vec::new(),
        };

        if participants.is_empty() {
            participants.push(msg.sender_email.clone());
        }
        participants.sort();
        let conversation_id = participants.join(",");

        ConversationAddress::new("zulip", conversation_id, None)
    }
}

/// Resolve numeric participant IDs for Zulip private-message typing notifications.
pub fn resolve_zulip_private_recipient_ids(msg: &ZulipMessage, bot_email: &str) -> Vec<i64> {
    resolve_zulip_private_recipient_ids_for_user(msg, bot_email, None)
}

/// Resolve numeric participant IDs for private-message typing notifications,
/// excluding the authenticated user by stable Zulip user ID when available.
pub fn resolve_zulip_private_recipient_ids_for_user(
    msg: &ZulipMessage,
    bot_email: &str,
    bot_user_id: Option<i64>,
) -> Vec<i64> {
    if msg.message_type == "stream" {
        return Vec::new();
    }

    let mut user_ids = match &msg.display_recipient {
        ZulipDisplayRecipient::Private(recs) => recs
            .iter()
            .filter(|r| Some(r.id) != bot_user_id && r.email != bot_email)
            .map(|r| r.id)
            .collect::<Vec<i64>>(),
        _ => Vec::new(),
    };

    if user_ids.is_empty() {
        user_ids.push(msg.sender_id);
    }
    user_ids.sort_unstable();
    user_ids.dedup();
    user_ids
}

/// Strip the bot's mention from the start of a Zulip message text.
///
/// If a bot-specific mention pattern is available, it is used for precise matching.
/// Otherwise, a broad pattern is used (which may incorrectly strip other mentions).
pub fn strip_bot_mention(text: &str, bot_name: &str) -> String {
    if let Some(pattern) = crate::zulip::bot::build_bot_mention_pattern(bot_name) {
        pattern.replace(text, "").to_string()
    } else {
        BROAD_MENTION_PATTERN.replace(text, "").to_string()
    }
}

/// Process a Zulip message into a ChannelInboundEvent.
///
/// Handles attachment extraction, mention stripping, and address resolution.
/// Returns `None` if the message should be skipped (e.g., sent by the bot itself).
#[instrument(skip(msg, bot), fields(message_id = msg.id, sender = msg.sender_email))]
pub async fn process_zulip_message(
    msg: &ZulipMessage,
    bot: &ZulipBot,
    max_bytes: usize,
    max_chars: usize,
) -> Result<Option<ChannelInboundEvent>, AgentError> {
    // Skip messages sent by the bot itself
    if bot.is_own_message(msg) {
        debug!(
            message_id = msg.id,
            sender = msg.sender_email,
            "skipping Zulip message sent by bot"
        );
        return Ok(None);
    }

    // Process attachments
    let (clean_text, attachment_parts, attachments) =
        process_inbound_attachments(&msg.content, bot, max_bytes, max_chars).await?;

    // Strip bot mentions
    let bot_name = bot.bot_name();
    let clean_text = strip_bot_mention(&clean_text, &bot_name);

    let address = resolve_zulip_address_for_user(msg, bot.bot_email(), bot.user_id());
    if address.thread_id.is_none() {
        bot.cache_typing_recipient_ids(
            &address.conversation_id,
            resolve_zulip_private_recipient_ids_for_user(msg, bot.bot_email(), bot.user_id()),
        );
    }
    let sender = SenderIdentity::new(msg.sender_email.clone(), Some(msg.sender_full_name.clone()));
    let inbound = InboundMessage {
        text: clean_text,
        attachments,
        attachment_parts,
    };

    Ok(Some(ChannelInboundEvent {
        address,
        sender,
        message: inbound,
    }))
}
