//! Zulip Bot API client.
//!
//! Communicates with the Zulip REST API using Basic authentication
//! (bot_email + api_key). Supports sending messages, typing notifications,
//! event queue registration/long-polling, and authenticated file downloads.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use futures::StreamExt;
use serde::Deserialize;
use tracing::debug;

use crate::error::AgentError;

/// Base Zulip API version prefix.
const ZULIP_API_PREFIX: &str = "api/v1";

/// Build a regex pattern to match only the bot's own mention at the start of text.
///
/// Returns `None` if no bot name is configured (falls back to broad matching).
/// Pattern: `^@\*\*BotName\*\*\s*`
pub fn build_bot_mention_pattern(bot_name: &str) -> Option<regex::Regex> {
    if bot_name.is_empty() {
        return None;
    }
    // Escape special regex characters in the bot name (Zulip names shouldn't have them,
    // but be defensive)
    let escaped = regex::escape(bot_name);
    let pattern = format!("^@\\*\\*{}\\*\\*\\s*", escaped);
    regex::Regex::new(&pattern).ok()
}

// ── Zulip API JSON types ───────────────────────────────────────────

/// A Zulip message recipient (stream or private).
#[derive(Debug, Clone)]
pub enum ZulipRecipient {
    /// A stream message with a topic.
    Stream { stream_name: String, topic: String },
    /// A private message to a list of user email addresses.
    Private { emails: Vec<String> },
}

/// Response wrapper for Zulip API write operations.
#[derive(Debug, Deserialize)]
pub struct ZulipMessageResponse {
    pub id: i64,
    pub result: String,
    pub msg: String,
}

/// Zulip message response data (only included when result == "success").
#[derive(Debug, Deserialize)]
pub struct ZulipMessageData {
    pub id: i64,
}

/// Zulip API response for send_message.
/// Uses a generic wrapper to handle both success and error cases.
#[derive(Debug, Deserialize)]
pub struct ZulipSendMessageResponse {
    pub result: String,
    #[serde(default)]
    pub msg: Option<String>,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub id: Option<i64>,
}

/// Information returned from queue registration.
#[derive(Debug, Default, Deserialize)]
pub struct ZulipQueueInfo {
    #[serde(default)]
    pub queue_id: String,
    /// last_event_id is optional so we can detect malformed responses.
    /// A missing field indicates a malformed success response.
    #[serde(default)]
    pub last_event_id: Option<i64>,
}

/// Shared Zulip API response wrapper.
///
/// All Zulip API responses include a `result` field ("success" or "error"),
/// an optional `msg` field with a human-readable message, and an optional
/// `code` field with an error code. The `data` field is optional so that
/// error responses (which lack success data) can be parsed without failing.
///
/// This wrapper validates `result == "success"` and includes `msg`/`code`
/// in error messages for consistent error handling.
#[derive(Debug, Deserialize)]
pub struct ZulipApiResponse<T> {
    pub result: String,
    #[serde(default)]
    pub msg: Option<String>,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    #[serde(flatten)]
    pub data: Option<T>,
}

impl<T> ZulipApiResponse<T> {
    /// Check that the API returned success and extract the data.
    /// Returns an error with the Zulip error message if `result != "success"`.
    pub fn into_result(self) -> Result<T, AgentError> {
        if self.result == "success" {
            self.data.ok_or_else(|| {
                AgentError::Zulip("Zulip API success response missing data".to_string())
            })
        } else {
            let msg = self.msg.unwrap_or_else(|| "Unknown API error".into());
            let code = self.code.as_deref().unwrap_or("unknown");
            Err(AgentError::Zulip(format!(
                "Zulip API error [{code}]: {msg}"
            )))
        }
    }
}

/// Zero-sized wrapper for API calls that don't return additional data.
#[derive(Debug, Deserialize)]
pub struct ZulipApiEmptyResponse {
    pub result: String,
    #[serde(default)]
    pub msg: Option<String>,
    #[serde(default)]
    pub code: Option<String>,
}

impl ZulipApiEmptyResponse {
    /// Check that the API returned success.
    pub fn into_result(self) -> Result<(), AgentError> {
        if self.result == "success" {
            Ok(())
        } else {
            let msg = self.msg.unwrap_or_else(|| "Unknown API error".into());
            let code = self.code.as_deref().unwrap_or("unknown");
            Err(AgentError::Zulip(format!(
                "Zulip API error [{code}]: {msg}"
            )))
        }
    }
}

/// A Zulip event from the long-polling event stream.
#[derive(Debug, Clone, Deserialize)]
pub struct ZulipEvent {
    pub id: i64,
    #[serde(rename = "type")]
    pub event_type: String,
    #[serde(default)]
    pub message: Option<ZulipMessage>,
}

/// A Zulip message (incoming or outgoing).
#[derive(Debug, Clone, Deserialize)]
pub struct ZulipMessage {
    pub id: i64,
    pub sender_id: i64,
    pub sender_email: String,
    pub sender_full_name: String,
    pub content: String,
    #[serde(rename = "type")]
    pub message_type: String,
    pub display_recipient: ZulipDisplayRecipient,
    #[serde(default)]
    pub subject: Option<String>, // Topic name (stream only)
    #[serde(default)]
    pub stream_id: Option<i64>,
}

/// Zulip display recipient — stream name (string) or private message participants (array).
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum ZulipDisplayRecipient {
    Stream(String),
    Private(Vec<ZulipPrivateRecipient>),
}

/// A private message recipient in a Zulip conversation.
#[derive(Debug, Clone, Deserialize)]
pub struct ZulipPrivateRecipient {
    pub id: i64,
    pub email: String,
    pub full_name: String,
}

/// Response from GET /api/v1/me — current bot user info.
#[derive(Debug, Clone, Deserialize)]
pub struct ZulipUserInfo {
    pub user_id: i64,
    pub email: String,
    pub full_name: String,
}

// ── ZulipBot ────────────────────────────────────────────────────────

/// Low-level Zulip Bot API client.
///
/// All API calls use Basic authentication with the bot's email and API key.
#[derive(Debug, Clone)]
pub struct ZulipBot {
    client: reqwest::Client,
    site_url: String, // Normalized with trailing slash
    bot_email: String,
    api_key: String,
    /// Bot's full name as registered in Zulip (used for mention stripping).
    /// Uses interior mutability so it can be set after initialization.
    bot_name: Arc<Mutex<String>>,
    typing_recipient_ids: Arc<Mutex<HashMap<String, Vec<i64>>>>,
}

impl ZulipBot {
    /// Create a new Zulip bot client.
    ///
    /// The `site_url` is normalized to include a trailing slash.
    pub fn new(site_url: String, bot_email: String, api_key: String) -> Self {
        let mut site_url = site_url;
        if !site_url.ends_with('/') {
            site_url.push('/');
        }
        let bot_name = Arc::new(Mutex::new(String::new()));
        Self {
            client: reqwest::Client::new(),
            site_url,
            bot_email,
            api_key,
            bot_name,
            typing_recipient_ids: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Set the bot's full name (as registered in Zulip).
    pub fn set_bot_name(&self, name: String) {
        if let Ok(mut bot_name) = self.bot_name.lock() {
            *bot_name = name;
        }
    }

    /// Get the bot's full name.
    pub fn bot_name(&self) -> String {
        self.bot_name.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// Get the bot's email address.
    pub fn bot_email(&self) -> &str {
        &self.bot_email
    }

    /// Cache the numeric Zulip user IDs for a private-message conversation.
    ///
    /// Zulip accepts email recipients for sending private messages, but the
    /// typing endpoint expects numeric user IDs in its `to` array.
    pub fn cache_typing_recipient_ids(&self, conversation_id: &str, user_ids: Vec<i64>) {
        if user_ids.is_empty() {
            return;
        }
        if let Ok(mut cache) = self.typing_recipient_ids.lock() {
            cache.insert(conversation_id.to_string(), user_ids);
        }
    }

    pub fn typing_recipient_ids(&self, conversation_id: &str) -> Option<Vec<i64>> {
        self.typing_recipient_ids
            .lock()
            .ok()
            .and_then(|cache| cache.get(conversation_id).cloned())
    }

    /// Fetch the bot's user info from Zulip (includes full_name).
    pub async fn get_me(&self) -> Result<ZulipUserInfo, AgentError> {
        let url = format!("{}{}/users/me", self.site_url, ZULIP_API_PREFIX);

        debug!("fetching Zulip bot user info");

        let res = self
            .authenticate(self.client.get(&url))
            .send()
            .await
            .map_err(|e| AgentError::Zulip(format!("HTTP error during get_me: {e}")))?;

        let status = res.status();
        let body = res
            .text()
            .await
            .map_err(|e| AgentError::Zulip(format!("Failed to read get_me response body: {e}")))?;

        if !status.is_success() {
            return Err(AgentError::Zulip(format!(
                "Zulip get_me failed: HTTP {status}: {body}"
            )));
        }

        let user_info: ZulipUserInfo = serde_json::from_str(&body).map_err(|e| {
            AgentError::Zulip(format!(
                "Failed to parse get_me response: {e}. Body: {body}"
            ))
        })?;

        debug!(
            bot_id = user_info.user_id,
            bot_name = user_info.full_name,
            bot_email = user_info.email,
            "Zulip bot user info retrieved"
        );
        Ok(user_info)
    }

    /// Get the normalized site URL.
    pub fn site_url(&self) -> &str {
        &self.site_url
    }

    /// Attach Basic authentication headers to a request builder.
    fn authenticate(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        req.basic_auth(&self.bot_email, Some(&self.api_key))
    }

    /// Send a message via the Zulip API.
    ///
    /// # Arguments
    /// * `recipient` — Stream or private message recipient
    /// * `content` — Message content (markdown)
    pub async fn send_message(
        &self,
        recipient: &ZulipRecipient,
        content: &str,
    ) -> Result<ZulipMessageResponse, AgentError> {
        let url = format!("{}{}/messages", self.site_url, ZULIP_API_PREFIX);
        let mut params: Vec<(&str, String)> = vec![("content", content.to_string())];

        match recipient {
            ZulipRecipient::Stream { stream_name, topic } => {
                params.push(("type", "stream".to_string()));
                params.push(("to", stream_name.clone()));
                params.push(("topic", topic.clone()));
            }
            ZulipRecipient::Private { emails } => {
                params.push(("type", "private".to_string()));
                let to_json = serde_json::to_string(emails).map_err(|e| {
                    AgentError::Zulip(format!("Failed to serialize PM recipients: {e}"))
                })?;
                params.push(("to", to_json));
            }
        }

        debug!(recipient_type = ?recipient, content_len = content.len(), "sending Zulip message");

        let body = serde_urlencoded::to_string(&params).unwrap_or_default();
        let res = self
            .authenticate(self.client.post(&url))
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .body(body)
            .send()
            .await
            .map_err(|e| AgentError::Zulip(format!("HTTP error during send_message: {e}")))?;

        let status = res.status();
        let body = res.text().await.map_err(|e| {
            AgentError::Zulip(format!("Failed to read send_message response body: {e}"))
        })?;

        if !status.is_success() {
            return Err(AgentError::Zulip(format!(
                "Zulip send_message failed: HTTP {status}: {body}"
            )));
        }

        let response: ZulipSendMessageResponse = serde_json::from_str(&body).map_err(|e| {
            AgentError::Zulip(format!(
                "Failed to parse send_message response: {e}. Body: {body}"
            ))
        })?;

        if response.result != "success" {
            let msg = response.msg.unwrap_or_else(|| "Unknown API error".into());
            return Err(AgentError::Zulip(format!("Zulip API error: {msg}")));
        }

        let message_id = response.id.ok_or_else(|| {
            AgentError::Zulip("Zulip send_message response missing message id".to_string())
        })?;

        debug!(message_id, "Zulip message sent");
        Ok(ZulipMessageResponse {
            id: message_id,
            result: response.result,
            msg: response.msg.unwrap_or_default(),
        })
    }

    /// Send a typing notification via the Zulip API.
    ///
    /// # Arguments
    /// * `user_ids` — Recipient Zulip user IDs
    /// * `op` — `"start"` or `"stop"`
    pub async fn send_typing_notification(
        &self,
        user_ids: &[i64],
        op: &str,
    ) -> Result<(), AgentError> {
        let url = format!("{}{}/typing", self.site_url, ZULIP_API_PREFIX);
        let user_ids_json = serde_json::to_string(user_ids)
            .map_err(|e| AgentError::Zulip(format!("Failed to serialize typing user IDs: {e}")))?;

        let params = vec![
            ("op", op.to_string()),
            ("to", user_ids_json),
            ("type", "direct".to_string()),
        ];

        debug!(user_ids = ?user_ids, op, "sending Zulip typing notification");

        let body = serde_urlencoded::to_string(&params).unwrap_or_default();
        let res = self
            .authenticate(self.client.post(&url))
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .body(body)
            .send()
            .await
            .map_err(|e| {
                AgentError::Zulip(format!("HTTP error during typing notification: {e}"))
            })?;

        let status = res.status();
        let body = res
            .text()
            .await
            .map_err(|e| AgentError::Zulip(format!("Failed to read typing response body: {e}")))?;

        if !status.is_success() {
            return Err(AgentError::Zulip(format!(
                "Zulip typing failed: HTTP {status}: {body}"
            )));
        }

        let response: ZulipApiEmptyResponse = serde_json::from_str(&body).map_err(|e| {
            AgentError::Zulip(format!(
                "Failed to parse typing response: {e}. Body: {body}"
            ))
        })?;
        response.into_result()
    }

    /// Update the bot user's Zulip presence.
    pub async fn update_presence(&self, status: &str, ping_only: bool) -> Result<(), AgentError> {
        let url = format!("{}{}/users/me/presence", self.site_url, ZULIP_API_PREFIX);
        let params = vec![
            ("status", status.to_string()),
            ("ping_only", ping_only.to_string()),
            ("new_user_input", "false".to_string()),
        ];

        debug!(status, ping_only, "updating Zulip presence");

        let body = serde_urlencoded::to_string(&params).unwrap_or_default();
        let res = self
            .authenticate(self.client.post(&url))
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .body(body)
            .send()
            .await
            .map_err(|e| AgentError::Zulip(format!("HTTP error during presence update: {e}")))?;

        let status_code = res.status();
        let body = res.text().await.map_err(|e| {
            AgentError::Zulip(format!("Failed to read presence response body: {e}"))
        })?;

        if !status_code.is_success() {
            return Err(AgentError::Zulip(format!(
                "Zulip presence update failed: HTTP {status_code}: {body}"
            )));
        }

        let response: ZulipApiEmptyResponse = serde_json::from_str(&body).map_err(|e| {
            AgentError::Zulip(format!(
                "Failed to parse presence response: {e}. Body: {body}"
            ))
        })?;
        response.into_result()
    }

    /// Register a long-polling event queue.
    ///
    /// Returns the queue ID and initial last_event_id.
    pub async fn register_queue(&self) -> Result<ZulipQueueInfo, AgentError> {
        let url = format!("{}{}/register", self.site_url, ZULIP_API_PREFIX);
        let params = vec![
            ("event_types", "[\"message\"]".to_string()),
            ("queue_id", "".to_string()), // Empty for new registration
            ("all_public_streams", "true".to_string()),
        ];

        debug!("registering Zulip event queue");

        let body = serde_urlencoded::to_string(&params).unwrap_or_default();
        let res = self
            .authenticate(self.client.post(&url))
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .body(body)
            .send()
            .await
            .map_err(|e| AgentError::Zulip(format!("HTTP error during queue registration: {e}")))?;

        let status = res.status();
        let body = res.text().await.map_err(|e| {
            AgentError::Zulip(format!("Failed to read register response body: {e}"))
        })?;

        if !status.is_success() {
            return Err(AgentError::Zulip(format!(
                "Zulip register failed: HTTP {status}: {body}"
            )));
        }

        let response: ZulipApiResponse<ZulipQueueInfo> =
            serde_json::from_str(&body).map_err(|e| {
                AgentError::Zulip(format!(
                    "Failed to parse register response: {e}. Body: {body}"
                ))
            })?;

        let queue_info = response.into_result()?;

        // Validate that both queue_id and last_event_id are present to catch malformed
        // success responses. A response like {"result":"success","msg":""} without these
        // fields would otherwise deserialize to empty defaults and break polling.
        if queue_info.queue_id.is_empty() {
            return Err(AgentError::Zulip(
                "Zulip register_queue returned success with empty queue_id".to_string(),
            ));
        }
        let last_event_id = queue_info.last_event_id.ok_or_else(|| {
            AgentError::Zulip(
                "Zulip register_queue returned success with missing last_event_id".to_string(),
            )
        })?;

        debug!(
            queue_id = queue_info.queue_id,
            last_event_id, "Zulip event queue registered"
        );
        Ok(queue_info)
    }

    /// Retrieve events from the long-polling queue.
    ///
    /// # Arguments
    /// * `queue_id` — The registered queue ID
    /// * `last_event_id` — Return events with ID greater than this value
    pub async fn get_events(
        &self,
        queue_id: &str,
        last_event_id: i64,
    ) -> Result<Vec<ZulipEvent>, AgentError> {
        let url = format!(
            "{}{}/events?queue_id={}&last_event_id={}&dont_block=false",
            self.site_url, ZULIP_API_PREFIX, queue_id, last_event_id
        );

        debug!(queue_id, last_event_id, "fetching Zulip events");

        let res = self
            .authenticate(self.client.get(&url))
            .send()
            .await
            .map_err(|e| AgentError::Zulip(format!("HTTP error during events fetch: {e}")))?;

        let status = res.status();
        let body = res
            .text()
            .await
            .map_err(|e| AgentError::Zulip(format!("Failed to read events response body: {e}")))?;

        if !status.is_success() {
            return Err(AgentError::Zulip(format!(
                "Zulip events failed: HTTP {status}: {body}"
            )));
        }

        #[derive(Deserialize)]
        struct EventsWrapper {
            #[serde(default)]
            events: Vec<ZulipEvent>,
            result: String,
            msg: Option<String>,
            code: Option<String>,
        }

        let wrapper: EventsWrapper = serde_json::from_str(&body).map_err(|e| {
            AgentError::Zulip(format!(
                "Failed to parse events response: {e}. Body: {body}"
            ))
        })?;

        if wrapper.result != "success" {
            if let Some(code) = &wrapper.code
                && code == "BAD_EVENT_QUEUE_ID"
            {
                return Err(AgentError::Zulip("BAD_EVENT_QUEUE_ID".to_string()));
            }
            return Err(AgentError::Zulip(
                wrapper.msg.unwrap_or_else(|| "Unknown events error".into()),
            ));
        }

        debug!(event_count = wrapper.events.len(), "events fetched");
        Ok(wrapper.events)
    }

    /// Download an authenticated user upload file, bounded by `max_bytes`.
    ///
    /// Streams the response with a running byte counter that aborts once
    /// `max_bytes` is exceeded, matching Telegram's bounded download behavior.
    ///
    /// # Arguments
    /// * `relative_path` — Path starting with `/user_uploads/...`
    /// * `max_bytes` — Maximum bytes to download.
    pub async fn download_file(
        &self,
        relative_path: &str,
        max_bytes: usize,
    ) -> Result<Vec<u8>, AgentError> {
        let url = format!("{}{}", self.site_url, relative_path);

        debug!(path = relative_path, max_bytes, "downloading Zulip file");

        let res = self
            .authenticate(self.client.get(&url))
            .send()
            .await
            .map_err(|e| AgentError::Zulip(format!("HTTP error during file download: {e}")))?;

        let status = res.status();
        if !status.is_success() {
            let body = res.text().await.unwrap_or_default();
            return Err(AgentError::Zulip(format!(
                "File download failed: HTTP {status}: {body}"
            )));
        }

        // Check content-length before streaming
        if let Some(content_length) = res.content_length()
            && content_length as usize > max_bytes
        {
            return Err(AgentError::Zulip(format!(
                "File size {} exceeds maximum {} bytes",
                content_length, max_bytes
            )));
        }

        // Stream with a byte limit to prevent unbounded memory usage
        let mut collected: Vec<u8> = Vec::new();
        let mut stream = res.bytes_stream();
        while let Some(chunk_result) = stream.next().await {
            let chunk = chunk_result
                .map_err(|e| AgentError::Zulip(format!("Failed to read file chunk: {e}")))?;

            if collected.len() + chunk.len() > max_bytes {
                return Err(AgentError::Zulip(format!(
                    "File download exceeded maximum of {} bytes",
                    max_bytes
                )));
            }

            collected.extend_from_slice(&chunk);
        }

        debug!(
            path = relative_path,
            bytes = collected.len(),
            "file downloaded"
        );
        Ok(collected)
    }
}
