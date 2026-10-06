//! Telegram runtime wiring and ingress loop.
//!
//! Keeps Telegram-specific startup, polling/webhook ingress, and raw update
//! dispatch out of the binary composition root.

use std::sync::Arc;

use tokio::sync::{mpsc, watch};
use tokio::task::JoinSet;
use tracing::{error, info, warn};

use super::bot::{TelegramBot, Update};
use super::commands::TelegramCommand;
use super::inbound::build_inbound_message;
use super::update::{TelegramHook, TelegramPoll, TelegramUpdate};
use crate::channel::{
    ChannelInboundEvent, ChannelMessageHandler, ConversationAddress, InboundMessage, SenderIdentity,
};
use crate::config::{TelegramChannelConfig, TelegramIngress};
use crate::error::AgentError;

pub struct TelegramRuntime {
    pub bot: Arc<TelegramBot>,
    pub config: TelegramChannelConfig,
    pub webhook: Option<TelegramWebhookRuntime>,
}

pub struct TelegramWebhookRuntime {
    pub secret_token: String,
    pub receiver: mpsc::Receiver<Update>,
}

pub async fn build_runtime(
    config: &TelegramChannelConfig,
) -> Result<Option<TelegramRuntime>, AgentError> {
    if !config.enabled {
        return Ok(None);
    }

    let bot_token = required_env(&config.bot_token_env, "Telegram bot token")?;
    let bot = Arc::new(TelegramBot::new(bot_token));

    match bot.get_me().await {
        Ok(user) => {
            info!(
                bot_username = ?user.username,
                bot_name = user.first_name,
                "Telegram bot authenticated"
            );
        }
        Err(e) => return Err(e),
    }

    if let Err(e) = bot.set_my_commands(&TelegramCommand::menu_commands()).await {
        warn!(
            error = %e,
            "failed to configure Telegram command menu; slash commands still work when typed manually"
        );
    }

    Ok(Some(TelegramRuntime {
        bot,
        config: config.clone(),
        webhook: None,
    }))
}

pub async fn run_ingress_loop(
    telegram: TelegramRuntime,
    handler: Arc<ChannelMessageHandler>,
    shutdown: watch::Receiver<bool>,
) {
    match telegram.config.ingress {
        TelegramIngress::Poll => {
            let updates =
                TelegramPoll::new(telegram.bot.clone(), telegram.config.poll_interval_secs);
            if let Err(e) =
                run_update_loop(updates, telegram.bot, handler, telegram.config, shutdown).await
            {
                error!(error = %e, "Telegram polling ingress failed");
            }
        }
        TelegramIngress::Webhook => {
            let Some(webhook) = telegram.webhook else {
                error!(
                    "Telegram webhook ingress is enabled but shared webhook receiver is missing"
                );
                return;
            };
            let updates = TelegramHook::new(
                telegram.bot.clone(),
                telegram.config.clone(),
                webhook.secret_token,
                webhook.receiver,
            );
            if let Err(e) =
                run_update_loop(updates, telegram.bot, handler, telegram.config, shutdown).await
            {
                error!(error = %e, "Telegram webhook ingress failed");
            }
        }
    }
}

async fn run_update_loop<T>(
    mut updates: T,
    bot: Arc<TelegramBot>,
    handler: Arc<ChannelMessageHandler>,
    telegram_config: TelegramChannelConfig,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), AgentError>
where
    T: TelegramUpdate,
{
    updates.init().await?;
    let mut handlers = JoinSet::new();

    loop {
        tokio::select! {
            biased;
            _ = async { let _ = shutdown.wait_for(|closed| *closed).await; } => break,
            result = handlers.join_next(), if !handlers.is_empty() => {
                if let Some(Err(error)) = result {
                    error!(%error, "Telegram message handler task failed");
                }
            }
            res = updates.poll() => {
                match res {
                    Ok(Some(update)) => {
                        let bot = bot.clone();
                        let handler = handler.clone();
                        let telegram_config = telegram_config.clone();
                        handlers.spawn(async move {
                            dispatch_update(
                                update,
                                bot,
                                handler,
                                telegram_config,
                            )
                            .await;
                        });
                    }
                    Ok(None) => {
                        continue;
                    }
                    Err(e) => {
                        error!(error = %e, "Telegram update polling failed, retrying in 5s");
                        tokio::select! {
                            _ = async { let _ = shutdown.wait_for(|closed| *closed).await; } => break,
                            _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {}
                            _ = tokio::signal::ctrl_c() => {
                                info!("received Ctrl-C during retry sleep, shutting down...");
                                break;
                            }
                        }
                    }
                }
            }
            _ = tokio::signal::ctrl_c() => {
                info!("received Ctrl-C, shutting down...");
                break;
            }
        }
    }

    // Stop accepting updates, then finish accepted messages before MCP sessions close.
    while let Some(result) = handlers.join_next().await {
        if let Err(error) = result {
            error!(%error, "Telegram message handler task failed during shutdown");
        }
    }
    Ok(())
}

async fn dispatch_update(
    update: Update,
    bot: Arc<TelegramBot>,
    handler: Arc<ChannelMessageHandler>,
    telegram_config: TelegramChannelConfig,
) {
    let msg = match update.message {
        Some(m) => m,
        None => return,
    };

    let chat_id = msg.chat.id;
    let user_id = msg.from.as_ref().map(|u| u.id).unwrap_or(0);
    let address = ConversationAddress::telegram_chat(chat_id);
    let sender = SenderIdentity::new(
        user_id.to_string(),
        msg.from
            .as_ref()
            .map(|u| u.username.clone().unwrap_or_else(|| u.first_name.clone())),
    );

    // Allowlist check BEFORE building/downloading attachments.
    // This prevents untrusted users from triggering expensive downloads.
    let allowed = (telegram_config.allowed_conversations.is_empty()
        || telegram_config
            .allowed_conversations
            .contains(&address.conversation_id))
        && (telegram_config.allowed_senders.is_empty()
            || telegram_config.allowed_senders.contains(&sender.sender_id));
    if !allowed {
        warn!(?address, ?sender, "skipping message: not in allowlist");
        return;
    }

    let (text, attachment_parts, attachment_infos) = build_inbound_message(
        &msg,
        bot.as_ref(),
        telegram_config.max_attachment_bytes,
        telegram_config.max_text_document_chars,
    )
    .await;

    let inbound = InboundMessage {
        text,
        attachment_parts,
        attachments: attachment_infos,
    };

    let event = ChannelInboundEvent {
        address,
        sender,
        message: inbound,
    };

    match handler.handle_event(event).await {
        Ok(()) => {}
        Err(AgentError::PermissionDenied) => {
            error!(chat_id, user_id, "permission denied");
        }
        Err(e) => {
            error!(chat_id, error = %e, "message handler error");
        }
    }
}

fn required_env(env_var: &str, description: &str) -> Result<String, AgentError> {
    match std::env::var(env_var) {
        Ok(value) if !value.is_empty() => Ok(value),
        Ok(_) => Err(AgentError::Config(format!(
            "{description} environment variable {env_var} is empty"
        ))),
        Err(_) => Err(AgentError::Config(format!(
            "{description} environment variable {env_var} is not set"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::{AtomicBool, Ordering};

    use crate::agent::personality::Personality;
    use crate::channel::handler::ChannelMessageHandlerInput;
    use crate::channel::{ChannelRegistry, ChannelService, OutboundMessage};
    use crate::config::AppConfig;
    use crate::context::budget::ContextBudget;
    use crate::context::compaction_service::CompactionService;
    use crate::context::compaction_worker::CompactionWorker;
    use crate::llm::fake::FakeProvider;
    use crate::tools::registry::ToolRegistry;

    struct OneUpdate(Option<Update>);

    #[async_trait::async_trait]
    impl TelegramUpdate for OneUpdate {
        async fn init(&mut self) -> Result<(), AgentError> {
            Ok(())
        }

        async fn poll(&mut self) -> Result<Option<Update>, AgentError> {
            match self.0.take() {
                Some(update) => Ok(Some(update)),
                None => std::future::pending().await,
            }
        }
    }

    #[derive(Default)]
    struct GatedReply {
        entered: tokio::sync::Notify,
        release: tokio::sync::Notify,
        completed: AtomicBool,
    }

    #[async_trait::async_trait]
    impl ChannelService for GatedReply {
        fn channel_id(&self) -> &str {
            "telegram"
        }

        async fn send_message(
            &self,
            _: &ConversationAddress,
            _: OutboundMessage,
        ) -> Result<(), AgentError> {
            self.entered.notify_one();
            self.release.notified().await;
            self.completed.store(true, Ordering::SeqCst);
            Ok(())
        }
    }

    #[tokio::test]
    async fn shutdown_drains_an_accepted_telegram_message() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                sqlx::sqlite::SqliteConnectOptions::new()
                    .in_memory(true)
                    .foreign_keys(true),
            )
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        let reply = Arc::new(GatedReply::default());
        let provider = Arc::new(FakeProvider::new(vec![]));
        let worker = CompactionWorker::new(provider.clone(), "fake".into(), 0.0);
        let compaction = Arc::new(CompactionService::new(
            pool.clone(),
            Arc::new(worker),
            ContextBudget::default(),
            30,
        ));
        let directory = tempfile::tempdir().unwrap();
        let mut config = AppConfig::default();
        config.agent.personality_file = directory.path().join("personality.md");
        std::fs::write(&config.agent.personality_file, "test personality").unwrap();
        let handler = Arc::new(ChannelMessageHandler::new(ChannelMessageHandlerInput {
            pool,
            llm: provider,
            registry: Arc::new(ToolRegistry::new()),
            channel_registry: Arc::new(ChannelRegistry::new(vec![reply.clone()])),
            personality: Personality::from_config(&config),
            config: config.clone(),
            scheduler_notifier: None,
            compaction_service: compaction,
        }));
        let update = serde_json::from_value(serde_json::json!({
            "update_id": 1,
            "message": {"message_id": 1, "chat": {"id": 123, "type": "private"},
                "from": {"id": 456, "is_bot": false, "first_name": "tester"}, "text": "/help"}
        }))
        .unwrap();
        let (shutdown, receiver) = watch::channel(false);
        let task = tokio::spawn(run_update_loop(
            OneUpdate(Some(update)),
            Arc::new(TelegramBot::new("unused-test-token".into())),
            handler,
            config.channels.telegram,
            receiver,
        ));
        tokio::time::timeout(std::time::Duration::from_secs(1), reply.entered.notified())
            .await
            .unwrap();
        shutdown.send_replace(true);
        tokio::task::yield_now().await;
        assert!(
            !task.is_finished(),
            "accepted handlers must be drained before ingress returns"
        );
        reply.release.notify_one();
        tokio::time::timeout(std::time::Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(reply.completed.load(Ordering::SeqCst));
    }
}
