use std::collections::VecDeque;
use std::sync::Arc;

use async_trait::async_trait;
use tracing::{error, info};

use crate::error::AgentError;

use super::TelegramUpdate;
use crate::telegram::bot::{TelegramBot, Update};

/// Telegram update ingress via getUpdates long polling.
pub struct TelegramPoll {
    bot: Arc<TelegramBot>,
    offset: Option<i64>,
    pending: VecDeque<Update>,
    timeout_secs: u32,
}

impl TelegramPoll {
    pub fn new(bot: Arc<TelegramBot>) -> Self {
        Self {
            bot,
            offset: None,
            pending: VecDeque::new(),
            timeout_secs: 30,
        }
    }
}

#[async_trait]
impl TelegramUpdate for TelegramPoll {
    async fn init(&mut self) -> Result<(), AgentError> {
        if let Err(e) = self.bot.delete_webhook().await {
            error!(error = %e, "failed to delete webhook, long polling may not work");
        }
        info!("starting Telegram long polling");
        Ok(())
    }

    async fn poll(&mut self) -> Result<Option<Update>, AgentError> {
        if let Some(update) = self.pending.pop_front() {
            return Ok(Some(update));
        }

        let updates = self.bot.get_updates(self.offset, self.timeout_secs).await?;

        for update in updates {
            let new_offset = update.update_id + 1;
            if self.offset.is_none_or(|offset| new_offset > offset) {
                self.offset = Some(new_offset);
            }
            self.pending.push_back(update);
        }

        Ok(self.pending.pop_front())
    }
}
