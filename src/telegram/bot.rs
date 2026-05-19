//! Telegram bot client — long polling implementation.
//!
//! Communicates with the Telegram Bot API via HTTP (reqwest).
//! Uses getUpdates with long polling for inbound messages and
//! sendMessage for outbound delivery.
//!
//! API reference: https://core.telegram.org/bots/api

use crate::error::AgentError;
use serde::{Deserialize, Serialize};
use tracing::{debug, error, info, warn};

/// Maximum message length before Telegram rejects it (4096 UTF-8 code points).
pub const TELEGRAM_MAX_MESSAGE_LENGTH: usize = 4096;

/// Base URL template for the Telegram Bot API.
const TELEGRAM_API_BASE: &str = "https://api.telegram.org/bot";

// ── Telegram API JSON types ──────────────────────────────────────────

/// Top-level response wrapper for every Telegram API call.
#[derive(Debug, Deserialize)]
pub struct TelegramApiResponse<T> {
    pub ok: bool,
    pub result: Option<T>,
    pub description: Option<String>,
}

/// An update from the getUpdates long-polling endpoint.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct Update {
    pub update_id: i64,
    #[serde(default)]
    pub message: Option<Message>,
    #[serde(default)]
    pub edited_message: Option<Message>,
}

/// A Telegram message.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct Message {
    pub message_id: i64,
    pub from: Option<User>,
    pub chat: Chat,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub entities: Option<Vec<MessageEntity>>,
}

/// A Telegram user (sender).
#[derive(Debug, Clone, Deserialize, Default)]
pub struct User {
    pub id: i64,
    #[serde(default)]
    pub first_name: String,
    #[serde(default)]
    pub last_name: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
}

/// A Telegram chat.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct Chat {
    pub id: i64,
    #[serde(rename = "type")]
    pub chat_type: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
}

/// A message entity (bold, italic, bot_command, etc.)
#[derive(Debug, Clone, Deserialize, Default)]
pub struct MessageEntity {
    #[serde(rename = "type")]
    pub entity_type: String,
    pub offset: usize,
    pub length: usize,
}

/// Payload for the sendMessage call.
#[derive(Debug, Serialize)]
struct SendMessageRequest {
    chat_id: i64,
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    parse_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    disable_notification: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reply_to_message_id: Option<i64>,
}

/// Response from sendMessage (we only need ok/description but destructure the result).
#[derive(Debug, Deserialize, Default)]
pub struct SentMessage {
    pub message_id: i64,
    pub chat: Chat,
    pub text: Option<String>,
}

// ── TelegramBot ───────────────────────────────────────────────────────

/// Low-level Telegram Bot API client.
///
/// Handles HTTP communication with the Telegram Bot API including
/// long polling for updates and sending messages.
#[derive(Debug)]
pub struct TelegramBot {
    token: String,
    http: reqwest::Client,
    base_url: String,
}

impl TelegramBot {
    /// Create a new bot client.
    pub fn new(token: String) -> Self {
        let base_url = format!("{TELEGRAM_API_BASE}{token}");
        Self {
            token,
            http: reqwest::Client::new(),
            base_url,
        }
    }

    /// Get the bot token (for sharing with tools).
    pub fn token(&self) -> &str {
        &self.token
    }

    /// Fetch pending updates from Telegram with long polling.
    ///
    /// Blocks for up to `timeout_secs` waiting for new updates.
    /// Returns the list of updates since `offset` (exclusive).
    pub async fn get_updates(
        &self,
        offset: Option<i64>,
        timeout_secs: u32,
    ) -> Result<Vec<Update>, AgentError> {
        let url = format!("{}/getUpdates", self.base_url);

        let mut params: Vec<(&str, String)> = vec![("timeout", timeout_secs.to_string())];

        // Only include offset if it's > 0 (Telegram uses 0-based +1 to acknowledge)
        if let Some(o) = offset
            && o > 0
        {
            params.push(("offset", o.to_string()));
        }

        // Allow only message updates for now
        params.push((
            "allowed_updates",
            r#"["message","edited_message"]"#.to_string(),
        ));

        debug!(?offset, timeout_secs, "polling for Telegram updates");

        let response = self
            .http
            .post(&url)
            .form(&params)
            .send()
            .await
            .map_err(|e| AgentError::Telegram(format!("HTTP error during getUpdates: {e}")))?;

        let status = response.status();
        let body_text = response
            .text()
            .await
            .map_err(|e| AgentError::Telegram(format!("Failed to read getUpdates body: {e}")))?;

        if !status.is_success() {
            return Err(AgentError::Telegram(format!(
                "getUpdates returned HTTP {status}: {body_text}"
            )));
        }

        let api_response: TelegramApiResponse<Vec<Update>> = serde_json::from_str(&body_text)
            .map_err(|e| {
                AgentError::Telegram(format!(
                    "Failed to parse getUpdates response: {e}. Body: {body_text}"
                ))
            })?;

        if !api_response.ok {
            return Err(AgentError::Telegram(format!(
                "Telegram API error: {}",
                api_response.description.as_deref().unwrap_or("unknown")
            )));
        }

        let updates = api_response.result.unwrap_or_default();
        debug!(count = updates.len(), "received Telegram updates");
        Ok(updates)
    }

    /// Send a text message to a chat.
    ///
    /// Returns the sent message metadata on success.
    pub async fn send_message(
        &self,
        chat_id: i64,
        text: &str,
        parse_mode: Option<&str>,
    ) -> Result<SentMessage, AgentError> {
        // Validate text length — Telegram limit is 4096 UTF-8 code points
        if text.chars().count() > TELEGRAM_MAX_MESSAGE_LENGTH {
            return Err(AgentError::Telegram(format!(
                "Message too long: {} chars (max {TELEGRAM_MAX_MESSAGE_LENGTH})",
                text.chars().count()
            )));
        }

        let url = format!("{}/sendMessage", self.base_url);

        let payload = SendMessageRequest {
            chat_id,
            text: text.to_string(),
            parse_mode: parse_mode.map(|s| s.to_string()),
            disable_notification: None,
            reply_to_message_id: None,
        };

        debug!(chat_id, text_len = text.len(), "sending Telegram message");

        let response = self
            .http
            .post(&url)
            .json(&payload)
            .send()
            .await
            .map_err(|e| AgentError::Telegram(format!("HTTP error during sendMessage: {e}")))?;

        let status = response.status();
        let body_text = response
            .text()
            .await
            .map_err(|e| AgentError::Telegram(format!("Failed to read sendMessage body: {e}")))?;

        if !status.is_success() {
            return Err(AgentError::Telegram(format!(
                "sendMessage returned HTTP {status}: {body_text}"
            )));
        }

        let api_response: TelegramApiResponse<SentMessage> = serde_json::from_str(&body_text)
            .map_err(|e| {
                AgentError::Telegram(format!(
                    "Failed to parse sendMessage response: {e}. Body: {body_text}"
                ))
            })?;

        if !api_response.ok {
            return Err(AgentError::Telegram(format!(
                "Telegram sendMessage failed: {}",
                api_response.description.as_deref().unwrap_or("unknown")
            )));
        }

        let sent = api_response.result.ok_or_else(|| {
            AgentError::Telegram("sendMessage response had no result field".into())
        })?;

        debug!(
            chat_id,
            message_id = sent.message_id,
            "Telegram message sent"
        );
        Ok(sent)
    }

    /// Delete the webhook if one is configured, so long polling can work.
    pub async fn delete_webhook(&self) -> Result<(), AgentError> {
        let url = format!("{}/deleteWebhook", self.base_url);

        debug!("deleting Telegram webhook");

        let response = self
            .http
            .post(&url)
            .json(&serde_json::json!({ "drop_pending_updates": false }))
            .send()
            .await
            .map_err(|e| AgentError::Telegram(format!("HTTP error during deleteWebhook: {e}")))?;

        let body_text = response
            .text()
            .await
            .map_err(|e| AgentError::Telegram(format!("Failed to read deleteWebhook body: {e}")))?;

        let api_response: TelegramApiResponse<serde_json::Value> = serde_json::from_str(&body_text)
            .map_err(|e| {
                AgentError::Telegram(format!(
                    "Failed to parse deleteWebhook response: {e}. Body: {body_text}"
                ))
            })?;

        if !api_response.ok {
            warn!(
                "deleteWebhook returned not-ok (may already be unset): {:?}",
                api_response.description
            );
        } else {
            info!("Telegram webhook deleted, long polling ready");
        }

        Ok(())
    }

    /// Check that the bot token is valid by calling getMe.
    pub async fn get_me(&self) -> Result<User, AgentError> {
        let url = format!("{}/getMe", self.base_url);

        debug!("calling getMe to verify bot token");

        let response = self
            .http
            .post(&url)
            .send()
            .await
            .map_err(|e| AgentError::Telegram(format!("HTTP error during getMe: {e}")))?;

        let body_text = response
            .text()
            .await
            .map_err(|e| AgentError::Telegram(format!("Failed to read getMe body: {e}")))?;

        let api_response: TelegramApiResponse<User> =
            serde_json::from_str(&body_text).map_err(|e| {
                AgentError::Telegram(format!(
                    "Failed to parse getMe response: {e}. Body: {body_text}"
                ))
            })?;

        if !api_response.ok {
            return Err(AgentError::Telegram(format!(
                "Invalid bot token or Telegram API error: {}",
                api_response.description.as_deref().unwrap_or("unknown")
            )));
        }

        let user = api_response
            .result
            .ok_or_else(|| AgentError::Telegram("getMe response had no result field".into()))?;

        info!(
            bot_id = user.id,
            bot_name = user.first_name,
            bot_username = ?user.username,
            "Telegram bot authenticated"
        );
        Ok(user)
    }

    /// Split a long message into chunks that fit Telegram's 4096-char limit.
    ///
    /// Splits on newlines where possible, then on word boundaries, then hard
    /// breaks at 4000 chars (leaving headroom for format markers).
    pub fn split_long_message(text: &str) -> Vec<String> {
        let limit = 4000; // Leave room for formatting / marker text
        let mut chunks = Vec::new();

        if text.chars().count() <= limit {
            chunks.push(text.to_string());
            return chunks;
        }

        let mut remaining = text;
        while !remaining.is_empty() {
            if remaining.chars().count() <= limit {
                chunks.push(remaining.to_string());
                break;
            }

            // Try to split on newline
            let chunk_end = find_split_point(remaining, limit);
            let chunk: String = remaining.chars().take(chunk_end).collect();
            let consumed = chunk.chars().count();

            chunks.push(chunk);
            remaining = &remaining[consumed..];
        }

        chunks
    }
}

/// Find a good split point within the limit, preferring newlines then spaces.
fn find_split_point(text: &str, limit: usize) -> usize {
    let chars: Vec<char> = text.chars().collect();

    if chars.len() <= limit {
        return chars.len();
    }

    // Look for a newline in the last third of the limit range
    let search_start = limit - (limit / 3);
    for (i, c) in chars
        .iter()
        .enumerate()
        .take(limit)
        .skip(search_start)
        .rev()
    {
        if *c == '\n' {
            return i + 1; // Include the newline in the current chunk
        }
    }

    // Look for a space in the last fifth
    let space_start = limit - (limit / 5);
    for (i, c) in chars.iter().enumerate().take(limit).skip(space_start).rev() {
        if *c == ' ' {
            return i + 1; // Include the space
        }
    }

    // Hard break at limit
    limit
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_short_message() {
        let result = TelegramBot::split_long_message("hello");
        assert_eq!(result, vec!["hello"]);
    }

    #[test]
    fn test_split_long_message_on_newline() {
        let line = "a".repeat(3000);
        let text = format!("{line}\n{line}");
        let result = TelegramBot::split_long_message(&text);
        assert_eq!(result.len(), 2);
        // The first chunk should end with the newline
        assert!(result[0].ends_with('\n'));
    }

    #[test]
    fn test_split_long_message_hard_break() {
        // A single long line with no spaces
        let text = "x".repeat(5000);
        let result = TelegramBot::split_long_message(&text);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_message_length_limit() {
        // 4096 chars is ok, 4097 is not
        let short = "a".repeat(TELEGRAM_MAX_MESSAGE_LENGTH);
        assert_eq!(short.chars().count(), TELEGRAM_MAX_MESSAGE_LENGTH);

        let long = "a".repeat(TELEGRAM_MAX_MESSAGE_LENGTH + 1);
        assert!(long.chars().count() > TELEGRAM_MAX_MESSAGE_LENGTH);
    }
}
