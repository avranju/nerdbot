//! Telegram update ingress implementations.
//!
//! This module keeps transport-specific update retrieval out of the main
//! application loop. Polling uses Telegram's `getUpdates`; webhook push
//! consumes validated updates queued by the shared webhook server.

use async_trait::async_trait;

use crate::error::AgentError;

use super::bot::Update;

mod hook;
mod poll;

pub use hook::TelegramHook;
pub use poll::TelegramPoll;

/// Retrieves Telegram updates from a configured ingress transport.
#[async_trait]
pub trait TelegramUpdate {
    /// Perform transport initialization before polling begins.
    async fn init(&mut self) -> Result<(), AgentError>;

    /// Retrieve the next update, if one is currently available.
    async fn poll(&mut self) -> Result<Option<Update>, AgentError>;
}
