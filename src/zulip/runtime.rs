//! Zulip runtime wiring and ingress loop.
//!
//! Keeps Zulip-specific startup, polling/webhook ingress, presence heartbeat,
//! and raw message dispatch out of the binary composition root.

use std::{future::Future, sync::Arc, time::Duration};

use tokio::sync::mpsc;
use tracing::{error, info, warn};

use super::bot::ZulipBot;
use super::update::hook::ZulipWebhookPayload;
use super::update::{ZulipHook, ZulipPoll, ZulipUpdate};
use crate::channel::ChannelMessageHandler;
use crate::config::{ZulipChannelConfig, ZulipIngress};
use crate::error::AgentError;

/// Identity lookups happen during startup, when a colocated Zulip server may
/// still be coming online. Retry for 31 seconds in total (1, 2, 4, 8, and 16
/// second waits) before allowing ingress to start without resolved identity.
const ZULIP_IDENTITY_MAX_ATTEMPTS: u32 = 6;
const ZULIP_IDENTITY_INITIAL_BACKOFF: Duration = Duration::from_secs(1);

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

    if let Err(e) = resolve_bot_identity(bot.as_ref()).await {
        warn!(
            attempts = ZULIP_IDENTITY_MAX_ATTEMPTS,
            error = %e,
            "Failed to resolve Zulip bot identity after retries; mention stripping will use broad pattern"
        );
    }

    Ok(Some(ZulipRuntime {
        bot,
        config: config.clone(),
        webhook: None,
    }))
}

async fn resolve_bot_identity(bot: &ZulipBot) -> Result<(), AgentError> {
    let user_info = retry_with_backoff(
        ZULIP_IDENTITY_MAX_ATTEMPTS,
        ZULIP_IDENTITY_INITIAL_BACKOFF,
        || bot.get_me(),
    )
    .await?;

    bot.set_user_id(user_info.user_id);
    bot.set_bot_name(user_info.full_name);
    let bot_name = bot.bot_name();
    info!(
        bot_name = %bot_name,
        bot_id = user_info.user_id,
        "Zulip bot identity resolved"
    );
    Ok(())
}

async fn retry_with_backoff<T, F, Fut>(
    max_attempts: u32,
    initial_backoff: Duration,
    mut operation: F,
) -> Result<T, AgentError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, AgentError>>,
{
    debug_assert!(max_attempts > 0, "retry attempts must be positive");
    let mut backoff = initial_backoff;

    for attempt in 1..=max_attempts {
        match operation().await {
            Ok(value) => return Ok(value),
            Err(error) if attempt == max_attempts => return Err(error),
            Err(error) => {
                warn!(
                    attempt,
                    max_attempts,
                    retry_in_secs = backoff.as_secs(),
                    error = %error,
                    "Failed to resolve Zulip bot identity; retrying"
                );
                tokio::time::sleep(backoff).await;
                backoff = backoff.saturating_mul(2);
            }
        }
    }

    unreachable!("positive retry attempts always return from the loop")
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

    #[tokio::test]
    async fn retries_identity_operation_with_exponential_backoff() {
        use std::sync::atomic::{AtomicU32, Ordering};

        let attempts = Arc::new(AtomicU32::new(0));
        let operation_attempts = attempts.clone();
        let result = retry_with_backoff(3, Duration::from_millis(1), move || {
            let attempt = operation_attempts.fetch_add(1, Ordering::SeqCst) + 1;
            async move {
                if attempt < 3 {
                    Err(AgentError::Zulip("Zulip is starting".to_string()))
                } else {
                    Ok("identity resolved")
                }
            }
        })
        .await;

        assert_eq!(result.unwrap(), "identity resolved");
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn detects_zulip_bot_presence_rejection() {
        let error = AgentError::Zulip(
            r#"Zulip presence update failed: HTTP 400 Bad Request: {"result":"error","msg":"This endpoint does not accept bot requests.","code":"BAD_REQUEST"}"#
                .to_string(),
        );

        assert!(presence_rejected_for_bot(&error));
    }
}
