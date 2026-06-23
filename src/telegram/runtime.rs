//! Telegram runtime wiring and ingress loop.
//!
//! Keeps Telegram-specific startup, polling/webhook ingress, and raw update
//! dispatch out of the binary composition root.

use std::sync::Arc;

use tokio::sync::mpsc;
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

pub async fn run_ingress_loop(telegram: TelegramRuntime, handler: Arc<ChannelMessageHandler>) {
    match telegram.config.ingress {
        TelegramIngress::Poll => {
            let updates =
                TelegramPoll::new(telegram.bot.clone(), telegram.config.poll_interval_secs);
            if let Err(e) = run_update_loop(updates, telegram.bot, handler, telegram.config).await {
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
            if let Err(e) = run_update_loop(updates, telegram.bot, handler, telegram.config).await {
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
) -> Result<(), AgentError>
where
    T: TelegramUpdate,
{
    updates.init().await?;

    loop {
        tokio::select! {
            res = updates.poll() => {
                match res {
                    Ok(Some(update)) => {
                        let bot = bot.clone();
                        let handler = handler.clone();
                        let telegram_config = telegram_config.clone();
                        tokio::spawn(async move {
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
