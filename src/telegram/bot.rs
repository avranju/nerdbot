//! Telegram bot client — long polling implementation.
//!
//! Communicates with the Telegram Bot API via HTTP (reqwest).
//! Uses getUpdates with long polling for inbound messages and
//! sendMessage for outbound delivery.
//!
//! API reference: https://core.telegram.org/bots/api

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use futures::StreamExt;
use genai::chat::ContentPart;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use super::attachment::{self, AttachmentKind};
use crate::error::AgentError;

/// Maximum message length before Telegram rejects it (4096 UTF-8 code points).
pub const TELEGRAM_MAX_MESSAGE_LENGTH: usize = 4096;

/// Base URL template for the Telegram Bot API.
const TELEGRAM_API_BASE: &str = "https://api.telegram.org/bot";
const TELEGRAM_FILE_BASE: &str = "https://api.telegram.org/file/bot";

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
    pub caption: Option<String>,
    #[serde(default)]
    pub entities: Option<Vec<MessageEntity>>,
    #[serde(default)]
    pub photo: Vec<PhotoSize>,
    #[serde(default)]
    pub document: Option<Document>,
}

/// A photo size object from Telegram.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct PhotoSize {
    pub file_id: String,
    pub file_unique_id: String,
    pub width: Option<i64>,
    pub height: Option<i64>,
    #[serde(default)]
    pub file_size: Option<u64>,
}

/// A Telegram document object.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct Document {
    #[serde(default)]
    pub file_id: String,
    #[serde(default)]
    pub file_unique_id: String,
    #[serde(default)]
    pub file_size: Option<u64>,
    #[serde(default)]
    pub file_name: Option<String>,
    #[serde(default)]
    pub mime_type: Option<String>,
    #[serde(default, flatten)]
    pub thumbnail: Option<PhotoSize>,
}

/// Response from the Telegram `getFile` API endpoint.
#[derive(Debug, Deserialize)]
struct TelegramGetFileResponse {
    pub file_path: Option<String>,
}

/// Response from the Telegram `getFile` API (wrapped).
#[derive(Debug, Deserialize)]
struct TelegramGetFileWrapper {
    pub ok: bool,
    pub result: Option<TelegramGetFileResponse>,
    pub description: Option<String>,
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

/// Payload for the sendChatAction call.
#[derive(Debug, Serialize)]
struct SendChatActionRequest {
    chat_id: i64,
    action: &'static str,
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
    file_base_url: String,
}

impl TelegramBot {
    /// Create a new bot client with the default Telegram API base URL.
    pub fn new(token: String) -> Self {
        let base_url = format!("{TELEGRAM_API_BASE}{token}");
        let file_base_url = format!("{TELEGRAM_FILE_BASE}{token}");
        Self::new_with_base_urls(token, base_url, file_base_url)
    }

    /// Create a new bot client with a custom base URL (useful for testing).
    pub fn new_with_base_url(token: String, base_url: String) -> Self {
        Self::new_with_base_urls(token, base_url.clone(), base_url)
    }

    /// Create a new bot client with custom API and file base URLs.
    pub fn new_with_base_urls(token: String, base_url: String, file_base_url: String) -> Self {
        Self {
            token,
            http: reqwest::Client::new(),
            base_url,
            file_base_url,
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
            .body(serde_urlencoded::to_string(&params).unwrap_or_default())
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
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
        disable_notification: Option<bool>,
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
            disable_notification,
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

    /// Notify Telegram that the bot is composing a response.
    pub async fn send_typing_action(&self, chat_id: i64) -> Result<(), AgentError> {
        let url = format!("{}/sendChatAction", self.base_url);
        let payload = SendChatActionRequest {
            chat_id,
            action: "typing",
        };

        debug!(chat_id, "sending Telegram typing action");

        let response = self
            .http
            .post(&url)
            .json(&payload)
            .send()
            .await
            .map_err(|e| AgentError::Telegram(format!("HTTP error during sendChatAction: {e}")))?;

        let status = response.status();
        let body_text = response.text().await.map_err(|e| {
            AgentError::Telegram(format!("Failed to read sendChatAction body: {e}"))
        })?;

        if !status.is_success() {
            return Err(AgentError::Telegram(format!(
                "sendChatAction returned HTTP {status}: {body_text}"
            )));
        }

        let api_response: TelegramApiResponse<bool> =
            serde_json::from_str(&body_text).map_err(|e| {
                AgentError::Telegram(format!(
                    "Failed to parse sendChatAction response: {e}. Body: {body_text}"
                ))
            })?;

        if !api_response.ok {
            return Err(AgentError::Telegram(format!(
                "Telegram sendChatAction failed: {}",
                api_response.description.as_deref().unwrap_or("unknown")
            )));
        }

        Ok(())
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

        let status = response.status();
        let body_text = response
            .text()
            .await
            .map_err(|e| AgentError::Telegram(format!("Failed to read deleteWebhook body: {e}")))?;

        if !status.is_success() {
            return Err(AgentError::Telegram(format!(
                "deleteWebhook returned HTTP {status}: {body_text}"
            )));
        }

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

        let status = response.status();
        let body_text = response
            .text()
            .await
            .map_err(|e| AgentError::Telegram(format!("Failed to read getMe body: {e}")))?;

        if !status.is_success() {
            return Err(AgentError::Telegram(format!(
                "getMe returned HTTP {status}: {body_text}"
            )));
        }

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

    /// Get the file path for a Telegram file_id.
    ///
    /// Returns `Ok(None)` if the file does not exist or Telegram considers it
    /// too large to download (no `file_path` is returned).
    pub async fn get_file(&self, file_id: &str) -> Result<Option<String>, AgentError> {
        let url = format!("{}/getFile", self.base_url);

        let response = self
            .http
            .post(&url)
            .header("Content-Type", "application/json")
            .body(
                serde_json::to_string(&serde_json::json!({
                    "file_id": file_id,
                }))
                .map_err(|e| {
                    AgentError::Telegram(format!("Failed to serialize getFile request: {e}"))
                })?,
            )
            .send()
            .await
            .map_err(|e| AgentError::Telegram(format!("HTTP error during getFile: {e}")))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(AgentError::Telegram(format!(
                "getFile returned HTTP {status}: {body}"
            )));
        }

        let wrapper: TelegramGetFileWrapper = response
            .json()
            .await
            .map_err(|e| AgentError::Telegram(format!("Failed to parse getFile response: {e}")))?;

        if !wrapper.ok {
            return Err(AgentError::Telegram(format!(
                "getFile failed: {}",
                wrapper.description.as_deref().unwrap_or("unknown")
            )));
        }

        let file_info = wrapper
            .result
            .ok_or_else(|| AgentError::Telegram("getFile response had no result".into()))?;

        if file_info.file_path.is_none() {
            warn!(
                file_id = file_id,
                "Telegram returned no file_path, file may be too large"
            );
        }

        Ok(file_info.file_path)
    }

    /// Download a file from Telegram's CDN, bounded by `max_bytes`.
    ///
    /// Calls `get_file` internally to obtain the file path, then streams
    /// the file from Telegram's CDN. The response is capped at `max_bytes`.
    ///
    /// # Arguments
    /// * `file_id` — Telegram file_id.
    /// * `max_bytes` — Maximum bytes to download.
    pub async fn download_file(
        &self,
        file_id: &str,
        max_bytes: usize,
    ) -> Result<Vec<u8>, AgentError> {
        let file_path = match self.get_file(file_id).await? {
            Some(path) => path,
            None => {
                return Err(AgentError::Telegram(
                    "No file path returned by Telegram; file may be too large".into(),
                ));
            }
        };

        let collected = self.download_file_via_path(&file_path, max_bytes).await?;

        debug!(
            file_id = file_id,
            downloaded_bytes = collected.len(),
            "file downloaded"
        );

        Ok(collected)
    }

    /// Process a Telegram attachment: download, validate, and convert to a ContentPart.
    ///
    /// This is the production attachment processing path — it uses `self.get_file`
    /// and `self.download_file` internally, ensuring a single download path through
    /// the bot client.
    ///
    /// # Arguments
    /// * `file_id` — Telegram file_id to download.
    /// * `filename` — Original filename (from Telegram, untrusted).
    /// * `mime_type` — MIME type from Telegram (validated).
    /// * `size_bytes` — File size in bytes (from Telegram metadata).
    /// * `max_attachment_bytes` — Maximum download size.
    /// * `max_text_chars` — Maximum characters for text documents.
    pub async fn process_attachment(
        &self,
        file_id: &str,
        filename: Option<&str>,
        mime_type: Option<&str>,
        size_bytes: u64,
        max_attachment_bytes: usize,
        max_text_chars: usize,
    ) -> Result<attachment::ProcessedAttachment, AgentError> {
        let kind = attachment::classify_attachment(mime_type, filename);
        let sanitized_name = filename.map(attachment::sanitize_filename);

        // Early returns for unsupported types or oversized files — no download needed.
        if kind == AttachmentKind::Unsupported {
            let display_name = sanitized_name.unwrap_or_else(|| "file".to_string());
            let mime = mime_type.unwrap_or("unknown");
            let marker = format!("[Unsupported attachment: {display_name}, type={mime}]");

            return Ok(attachment::ProcessedAttachment {
                content_part: ContentPart::Text(format!(
                    "Unsupported attachment type: {mime} ({display_name}).\n\
                        Supported: images (JPEG, PNG, WebP, GIF), PDFs, and text documents (txt, md, json, csv, etc.)."
                )),
                persistence_marker: marker,
                downloaded: false,
                extracted_text: None,
            });
        }

        // Early size check: reject before downloading
        if size_bytes > max_attachment_bytes as u64 {
            let display_name = sanitized_name.unwrap_or_else(|| "file".to_string());
            return Ok(attachment::ProcessedAttachment {
                content_part: ContentPart::Text(format!(
                    "⚠️ Attachment {display_name} is {size_bytes} bytes, exceeding the {max_attachment_bytes}-byte limit.\n\
                        Supported: images (JPEG, PNG, WebP, GIF), PDFs, and text documents (txt, md, json, csv, etc.)."
                )),
                persistence_marker: format!(
                    "[Attachment too large: {display_name}, {size_bytes} bytes]"
                ),
                downloaded: false,
                extracted_text: None,
            });
        }

        // Call get_file to obtain the CDN file path, then download from the CDN.
        let file_path = match self.get_file(file_id).await? {
            Some(path) => path,
            None => {
                let display_name = sanitized_name.as_deref().unwrap_or("file");
                return Ok(attachment::ProcessedAttachment {
                    content_part: ContentPart::Text(format!(
                        "⚠️ Failed to download {display_name}: file not available."
                    )),
                    persistence_marker: "[Attachment: not available]".to_string(),
                    downloaded: false,
                    extracted_text: None,
                });
            }
        };

        match kind {
            AttachmentKind::Binary => {
                // Download the file using the already-obtained file_path
                let data = self
                    .download_file_via_path(&file_path, max_attachment_bytes)
                    .await?;

                // Normalize MIME type based on signature inspection.
                // For images: reject if no known signature is found (prevents
                // forwarding arbitrary binary data as an image).
                // For PDFs: reject if magic bytes don't match.
                let mime = if let Some(inferred) = attachment::inspect_mime_signature(&data) {
                    debug!(
                        reported_mime = ?mime_type,
                        inferred_mime = inferred,
                        "MIME signature inspection"
                    );
                    inferred.to_string()
                } else if mime_type.map(|m| m == "application/pdf").unwrap_or(false) {
                    let display_name = sanitized_name.as_deref().unwrap_or("file");
                    return Err(AgentError::Telegram(format!(
                        "Attachment {display_name} claims to be a PDF but does not contain valid PDF magic bytes (%PDF-)."
                    )));
                } else if mime_type.map(|m| m.starts_with("image/")).unwrap_or(false) {
                    // Image with unrecognized signature — reject to prevent
                    // arbitrary binary data from being forwarded as an image.
                    let display_name = sanitized_name.as_deref().unwrap_or("file");
                    return Err(AgentError::Telegram(format!(
                        "Attachment {display_name} has an unrecognized image signature. The file may be corrupted or not a valid image."
                    )));
                } else {
                    mime_type.unwrap_or("application/octet-stream").to_string()
                };

                let content_part = ContentPart::from_binary_base64(
                    &mime,
                    &*BASE64.encode(&data),
                    sanitized_name.clone(),
                );

                let kb = size_bytes / 1024;
                let marker = if let Some(ref name) = sanitized_name {
                    format!("[Attached {mime}: {name}, {kb} KB]")
                } else {
                    format!("[Attached {mime}, {kb} KB]")
                };

                Ok(attachment::ProcessedAttachment {
                    content_part,
                    persistence_marker: marker,
                    downloaded: true,
                    extracted_text: None,
                })
            }

            AttachmentKind::Text => {
                // Download and decode as strict UTF-8 using the already-obtained file_path
                let data = self
                    .download_file_via_path(&file_path, max_attachment_bytes)
                    .await?;

                let text = match String::from_utf8(data) {
                    Ok(t) => t,
                    Err(e) => {
                        let display_name = sanitized_name.as_deref().unwrap_or("document");
                        return Err(AgentError::Telegram(format!(
                            "Attachment {display_name} is not valid UTF-8 text: {e}"
                        )));
                    }
                };

                let (text, actual_len) = if text.chars().count() > max_text_chars {
                    let truncated: String = text.chars().take(max_text_chars).collect();
                    let len = text.chars().count();
                    (
                        format!(
                            "{truncated}\n\n[Truncated — exceeded {max_text_chars} character limit]"
                        ),
                        len,
                    )
                } else {
                    let len = text.chars().count();
                    (text, len)
                };

                let display_name = sanitized_name.as_deref().unwrap_or("document");
                let marker =
                    format!("[Attached text document: {display_name}, {actual_len} chars]");

                Ok(attachment::ProcessedAttachment {
                    content_part: ContentPart::Text(format!(
                        "--- File: {display_name} ---\n{text}"
                    )),
                    persistence_marker: marker,
                    downloaded: true,
                    extracted_text: Some(text),
                })
            }

            AttachmentKind::Unsupported => {
                let display_name = sanitized_name.unwrap_or_else(|| "file".to_string());
                let mime = mime_type.unwrap_or("unknown");
                let marker = format!("[Unsupported attachment: {display_name}, type={mime}]");

                Ok(attachment::ProcessedAttachment {
                    content_part: ContentPart::Text(format!(
                        "Unsupported attachment type: {mime} ({display_name}).\n\
                            Supported: images (JPEG, PNG, WebP, GIF), PDFs, and text documents (txt, md, json, csv, etc.)."
                    )),
                    persistence_marker: marker,
                    downloaded: false,
                    extracted_text: None,
                })
            }
        }
    }

    /// Download a file from Telegram's CDN using a known file_path.
    ///
    /// This is an internal helper used by `process_attachment` to avoid
    /// calling `get_file` twice.
    async fn download_file_via_path(
        &self,
        file_path: &str,
        max_bytes: usize,
    ) -> Result<Vec<u8>, AgentError> {
        let url = format!("{}/{}", self.file_base_url, file_path);

        debug!(
            file_path = file_path,
            max_bytes, "downloading Telegram file via path"
        );

        let response =
            self.http.get(&url).send().await.map_err(|e| {
                AgentError::Telegram(format!("HTTP error during file download: {e}"))
            })?;

        let status = response.status();
        if !status.is_success() {
            return Err(AgentError::Telegram(format!(
                "File download returned HTTP {status}"
            )));
        }

        // Check content-length before streaming
        if let Some(content_length) = response.content_length()
            && content_length as usize > max_bytes
        {
            return Err(AgentError::Telegram(format!(
                "File size {} exceeds maximum {} bytes",
                content_length, max_bytes
            )));
        }

        // Stream with a byte limit to prevent unbounded memory usage
        let mut collected: Vec<u8> = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk_result) = stream.next().await {
            let chunk = chunk_result
                .map_err(|e| AgentError::Telegram(format!("Failed to read file chunk: {e}")))?;

            if collected.len() + chunk.len() > max_bytes {
                return Err(AgentError::Telegram(format!(
                    "File download exceeded maximum of {} bytes",
                    max_bytes
                )));
            }

            collected.extend_from_slice(&chunk);
        }

        Ok(collected)
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
    fn test_default_base_url_includes_token() {
        let bot = TelegramBot::new("test-token".into());
        assert_eq!(bot.base_url, "https://api.telegram.org/bottest-token");
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
