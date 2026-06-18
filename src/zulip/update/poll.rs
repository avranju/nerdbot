//! Zulip long-polling ingress.
//!
//! Uses Zulip's event queue system:
//! 1. Register a message event queue via POST /api/v1/register
//! 2. Poll for events via GET /api/v1/events
//! 3. Re-register on BAD_EVENT_QUEUE_ID errors

use std::collections::VecDeque;
use std::sync::Arc;

use async_trait::async_trait;
use tracing::{debug, info};

use super::{ChannelInboundEvent, ZulipUpdate};
use crate::channel::ChannelIngress;
use crate::error::AgentError;
use crate::zulip::bot::ZulipBot;

/// Long-polling Zulip ingress implementation.
///
/// Maintains an internal buffer of pending events to avoid making a separate
/// HTTP roundtrip for each message when multiple events arrive in a single batch.
pub struct ZulipPoll {
    bot: Arc<ZulipBot>,
    queue_id: Option<String>,
    last_event_id: i64,
    poll_interval_secs: u64,
    /// Buffered events waiting to be yielded (FIFO order preserved).
    pending_events: VecDeque<crate::zulip::bot::ZulipEvent>,
    /// Attachment size limits from configuration.
    max_attachment_bytes: usize,
    max_text_document_chars: usize,
}

impl ZulipPoll {
    /// Create a new Zulip long-polling ingress.
    pub fn new(bot: Arc<ZulipBot>, poll_interval_secs: u64) -> Self {
        Self {
            bot,
            queue_id: None,
            last_event_id: -1,
            poll_interval_secs,
            pending_events: VecDeque::new(),
            max_attachment_bytes: 5_242_880, // Default 5 MB — overridden by main.rs wiring
            max_text_document_chars: 32_768, // Default 32 KB
        }
    }

    /// Configure attachment size limits from the loaded config.
    pub fn with_attachment_limits(mut self, max_bytes: usize, max_chars: usize) -> Self {
        self.max_attachment_bytes = max_bytes;
        self.max_text_document_chars = max_chars;
        self
    }

    /// Initialize the event queue by registering with Zulip.
    async fn register(&mut self) -> Result<(), AgentError> {
        let queue = self.bot.register_queue().await?;
        self.queue_id = Some(queue.queue_id);
        self.last_event_id = queue.last_event_id.unwrap_or(0);
        Ok(())
    }

    /// Try to yield a raw Zulip message from the buffer, cloned.
    /// Returns the message for two-phase handling (access policy check before download).
    pub fn try_yield_raw_message_clone(&mut self) -> Option<crate::zulip::bot::ZulipMessage> {
        while let Some(event) = self.pending_events.pop_front() {
            if event.event_type == "message"
                && let Some(msg) = event.message
            {
                debug!(
                    message_id = msg.id,
                    sender = msg.sender_email,
                    message_type = msg.message_type,
                    "yielding raw Zulip message from buffer (cloned)"
                );
                return Some(msg);
            }
        }
        None
    }
}

#[async_trait]
impl ChannelIngress for ZulipPoll {
    async fn init(&mut self) -> Result<(), AgentError> {
        ZulipUpdate::init(self).await
    }

    async fn next_event(&mut self) -> Result<Option<ChannelInboundEvent>, AgentError> {
        // Zulip's ChannelIngress::next_event() returns raw/unprocessed events.
        // The main loop uses fetch_events() + try_yield_raw_message_clone() for
        // two-phase handling (access policy before attachment download).
        // This trait method is kept for compatibility but returns unprocessed events.
        if let Some(msg) = ZulipUpdate::poll(self).await? {
            return Ok(Some(ChannelInboundEvent {
                address: super::resolve_zulip_address_for_user(
                    &msg,
                    self.bot.bot_email(),
                    self.bot.user_id(),
                ),
                sender: crate::channel::SenderIdentity::new(
                    msg.sender_email.clone(),
                    Some(msg.sender_full_name.clone()),
                ),
                message: crate::channel::InboundMessage {
                    text: msg.content.clone(),
                    attachments: Vec::new(),
                    attachment_parts: Vec::new(),
                },
            }));
        }

        Ok(None)
    }
}

#[async_trait]
impl ZulipUpdate for ZulipPoll {
    async fn init(&mut self) -> Result<(), AgentError> {
        self.register().await?;
        Ok(())
    }

    async fn poll(&mut self) -> Result<Option<crate::zulip::bot::ZulipMessage>, AgentError> {
        if let Some(msg) = self.try_yield_raw_message_clone() {
            return Ok(Some(msg));
        }

        self.fetch_events().await?;
        Ok(self.try_yield_raw_message_clone())
    }
}

/// Additional methods for ZulipPoll not part of the ChannelIngress trait.
impl ZulipPoll {
    /// Fetch events from the Zulip API and buffer them.
    /// Does NOT process or yield messages — only fetches and buffers.
    /// The main loop drains raw messages via `try_yield_raw_message_clone()`.
    pub async fn fetch_events(&mut self) -> Result<(), AgentError> {
        let queue_id = match &self.queue_id {
            Some(id) => id.clone(),
            None => {
                self.register().await?;
                self.queue_id.as_ref().unwrap().clone()
            }
        };

        // Sleep to throttle polling rate
        tokio::time::sleep(std::time::Duration::from_secs(self.poll_interval_secs)).await;

        let events = match self.bot.get_events(&queue_id, self.last_event_id).await {
            Ok(evs) => evs,
            Err(AgentError::Zulip(ref e)) if e == "BAD_EVENT_QUEUE_ID" => {
                // Re-register expired queue
                info!("Zulip event queue expired. Re-registering queue...");
                self.register().await?;
                return Ok(());
            }
            Err(e) => return Err(e),
        };

        // Buffer all message events for efficient batch processing.
        // Only update last_event_id for message events to avoid gaps.
        for event in events {
            self.last_event_id = i64::max(self.last_event_id, event.id);

            if event.event_type == "message" {
                self.pending_events.push_back(event);
            }
        }

        Ok(())
    }
}
