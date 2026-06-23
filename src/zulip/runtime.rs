//! Zulip runtime wiring and ingress loop.
//!
//! Keeps Zulip-specific startup, polling/webhook ingress, presence heartbeat,
//! and raw message dispatch out of the binary composition root.

use std::sync::Arc;

use tokio::sync::mpsc;
use tracing::{error, info, warn};

use super::bot::ZulipBot;
use super::update::hook::ZulipWebhookPayload;
use super::update::{ZulipHook, ZulipPoll, ZulipUpdate};
use crate::channel::ChannelMessageHandler;
use crate::config::{ZulipChannelConfig, ZulipIngress};
use crate::error::AgentError;

pub struct ZulipRuntime {
    pub bot: Arc<ZulipBot>,
    pub config: ZulipChannelConfig,
    pub webhook: Option<ZulipWebhookRuntime>,
}

pub struct ZulipWebhookRuntime {
    pub receiver: mpsc::Receiver<ZulipWebhookPayload>,
}

pub async fn build_runtime(
    config: &ZulipChannelConfig,
) -> Result<Option<ZulipRuntime>, AgentError> {
    if !config.enabled {
        return Ok(None);
    }

    let bot_email = required_env(&config.bot_email_env, "Zulip bot email")?;
    let api_key = required_env(&config.api_key_env, "Zulip API key")?;
    let bot = Arc::new(ZulipBot::new(config.site_url.clone(), bot_email, api_key));

    info!(
        site_url = config.site_url,
        bot_email = bot.bot_email(),
        "Zulip bot initialized"
    );

    match bot.get_me().await {
        Ok(user_info) => {
            bot.set_user_id(user_info.user_id);
            bot.set_bot_name(user_info.full_name);
            let bot_name = bot.bot_name();
            info!(
                bot_name = %bot_name,
                bot_id = user_info.user_id,
                "Zulip bot name resolved"
            );
        }
        Err(e) => {
            warn!(
                error = %e,
                "Failed to fetch Zulip bot name; mention stripping will use broad pattern"
            );
        }
    }

    Ok(Some(ZulipRuntime {
        bot,
        config: config.clone(),
        webhook: None,
    }))
}

pub async fn run_ingress_loop(zulip: ZulipRuntime, handler: Arc<ChannelMessageHandler>) {
    let presence_heartbeat = start_presence_heartbeat(
        zulip.bot.clone(),
        zulip.config.presence_enabled,
        zulip.config.presence_ping_interval_secs,
    );

    match zulip.config.ingress {
        ZulipIngress::Poll => {
            let updates = ZulipPoll::new(zulip.bot.clone(), zulip.config.poll_interval_secs)
                .with_attachment_limits(
                    zulip.config.max_attachment_bytes,
                    zulip.config.max_text_document_chars,
                );
            if let Err(e) = run_update_loop(updates, zulip.bot, handler, zulip.config).await {
                error!(error = %e, "Zulip polling ingress failed");
            }
        }
        ZulipIngress::Webhook => {
            let Some(webhook) = zulip.webhook else {
                error!("Zulip webhook ingress is enabled but shared webhook receiver is missing");
                return;
            };
            let updates = ZulipHook::new(webhook.receiver);
            if let Err(e) = run_update_loop(updates, zulip.bot, handler, zulip.config).await {
                error!(error = %e, "Zulip webhook ingress failed");
            }
        }
    }

    if let Some(handle) = presence_heartbeat {
        handle.abort();
    }
}

fn start_presence_heartbeat(
    bot: Arc<ZulipBot>,
    enabled: bool,
    interval_secs: u64,
) -> Option<tokio::task::JoinHandle<()>> {
    if !enabled {
        info!("Zulip presence heartbeat disabled");
        return None;
    }

    Some(tokio::spawn(async move {
        let interval = std::time::Duration::from_secs(interval_secs);
        info!(interval_secs, "starting Zulip active presence heartbeat");

        loop {
            if let Err(e) = bot.update_presence("active", true).await {
                if presence_rejected_for_bot(&e) {
                    warn!(
                        error = %e,
                        "Zulip rejected presence updates for the bot account; disabling presence heartbeat"
                    );
                    return;
                }
                warn!(error = %e, "failed to update Zulip presence");
            }
            tokio::time::sleep(interval).await;
        }
    }))
}

fn presence_rejected_for_bot(error: &AgentError) -> bool {
    error
        .to_string()
        .contains("This endpoint does not accept bot requests.")
}

async fn run_update_loop<T>(
    mut updates: T,
    bot: Arc<ZulipBot>,
    handler: Arc<ChannelMessageHandler>,
    config: ZulipChannelConfig,
) -> Result<(), AgentError>
where
    T: ZulipUpdate,
{
    updates.init().await?;

    loop {
        tokio::select! {
            res = updates.poll() => {
                match res {
                    Ok(Some(msg)) => {
                        if let Err(e) = handler
                            .handle_zulip_message(
                                &msg,
                                &bot,
                                bot.bot_email(),
                                config.max_attachment_bytes,
                                config.max_text_document_chars,
                            )
                            .await
                        {
                            error!(error = %e, "Zulip handler error");
                        }
                    }
                    Ok(None) => continue,
                    Err(e) => {
                        error!(error = %e, "Zulip polling failed, retrying in 5s");
                        tokio::select! {
                            _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {}
                            _ = tokio::signal::ctrl_c() => {
                                info!("received Ctrl-C during Zulip retry sleep, shutting down...");
                                return Ok(());
                            }
                        }
                    }
                }
            }
            _ = tokio::signal::ctrl_c() => {
                info!("received Ctrl-C, shutting down Zulip...");
                return Ok(());
            }
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

    #[test]
    fn detects_zulip_bot_presence_rejection() {
        let error = AgentError::Zulip(
            r#"Zulip presence update failed: HTTP 400 Bad Request: {"result":"error","msg":"This endpoint does not accept bot requests.","code":"BAD_REQUEST"}"#
                .to_string(),
        );

        assert!(presence_rejected_for_bot(&error));
    }
}
