//! Message persistence — CRUD operations via SQLx.

use chrono::DateTime;
use genai::chat::{ChatMessage, ChatRole, ContentPart, MessageContent, ToolResponse};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::SqlitePool;
use tracing::debug;

use crate::error::AgentError;

/// A stored message row from the database.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct StoredMessage {
    pub id: String,
    pub chat_session_id: String,
    pub role: Value,
    pub content: String,
    pub structured_content_json: Option<Value>,
    pub token_estimate: Option<i64>,
    pub created_at: DateTime<chrono::Utc>,
}

impl StoredMessage {
    /// Deserialize the role from JSON.
    ///
    /// Supports both legacy lowercase strings ("user", "assistant", "tool", "system")
    /// and genai-style enum variants ("User", "Assistant", "Tool", "System").
    pub fn role(&self) -> Result<ChatRole, AgentError> {
        let role_str = self
            .role
            .as_str()
            .ok_or_else(|| AgentError::Storage("Role is not a string".to_string()))?;

        match role_str.to_lowercase().as_str() {
            "system" => Ok(ChatRole::System),
            "user" => Ok(ChatRole::User),
            "assistant" => Ok(ChatRole::Assistant),
            "tool" => Ok(ChatRole::Tool),
            _ => Err(AgentError::Storage(format!("Unknown role: {}", role_str))),
        }
    }

    /// Convert to a full `ChatMessage` with role.
    pub fn to_message(&self) -> Result<ChatMessage, AgentError> {
        let role = self.role()?;

        let content = if let Some(ref json) = self.structured_content_json {
            if json.is_null() {
                MessageContent::from_text(self.content.clone())
            } else {
                // Try new genai format first (Vec<ContentPart> with call_id/fn_name)
                match serde_json::from_value::<Vec<ContentPart>>(json.clone()) {
                    Ok(parts) => MessageContent::from_parts(parts),
                    Err(_) => {
                        // Fall back to old llm::types format
                        Self::deserialize_legacy_structured_content(json, self.content.clone())
                    }
                }
            }
        } else {
            MessageContent::from_text(self.content.clone())
        };

        Ok(ChatMessage::new(role, content))
    }

    /// Deserialize old llm::types structured content (ToolCall {id,name,arguments},
    /// ToolResult {tool_call_id,status,content}) into new genai ContentPart format.
    fn deserialize_legacy_structured_content(
        json: &Value,
        fallback_content: String,
    ) -> MessageContent {
        // Old MessageContent was serialized as {"Text":...} or {"Parts":[...]}
        if let Some(parts_array) = json
            .as_array()
            .or_else(|| json.get("Parts").and_then(|v| v.as_array()))
        {
            let converted: Vec<ContentPart> = parts_array
                .iter()
                .map(Self::convert_legacy_content_part)
                .collect();
            MessageContent::from_parts(converted)
        } else if let Some(text) = json.get("Text").and_then(|v| v.as_str()) {
            MessageContent::from_text(text.to_string())
        } else {
            // Last resort: fall back to plain text
            MessageContent::from_text(fallback_content)
        }
    }

    /// Convert a single legacy ContentPart to the new genai format.
    fn convert_legacy_content_part(json: &Value) -> ContentPart {
        // Try new format first
        if let Ok(new_part) = serde_json::from_value::<ContentPart>(json.clone()) {
            return new_part;
        }

        // Old ContentPart::ToolCall { id, name, arguments }
        if let Some(old_call) = json.get("ToolCall")
            && let (Some(id), Some(name), Some(args)) = (
                old_call.get("id").and_then(|v| v.as_str()),
                old_call.get("name").and_then(|v| v.as_str()),
                old_call.get("arguments"),
            )
        {
            return ContentPart::ToolCall(genai::chat::ToolCall {
                call_id: id.to_string(),
                fn_name: name.to_string(),
                fn_arguments: args.clone(),
                thought_signatures: None,
            });
        }

        // Old ContentPart::ToolResult { tool_call_id, status, content }
        if let Some(old_result) = json.get("ToolResult") {
            let tool_call_id = old_result
                .get("tool_call_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let is_success = old_result
                .get("status")
                .and_then(|v| v.as_str())
                .map(|s| s == "success")
                .unwrap_or(false);
            let content_value = old_result.get("content").cloned().unwrap_or(Value::Null);

            let content = if is_success {
                serde_json::json!({"success": true, "data": content_value}).to_string()
            } else {
                let error_msg = old_result
                    .get("error")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown error")
                    .to_string();
                serde_json::json!({"success": false, "error": error_msg, "data": content_value})
                    .to_string()
            };

            ContentPart::ToolResponse(ToolResponse::new(&tool_call_id, content))
        } else {
            // Old ContentPart::Text(String)
            if let Some(text) = json.get("Text").and_then(|v| v.as_str()) {
                ContentPart::Text(text.to_string())
            } else {
                ContentPart::Text(format!("[Historical unrecognized content: {json}]"))
            }
        }
    }
}

/// Serialize a `ChatRole` as a lowercase JSON string for storage.
fn role_to_value(role: &ChatRole) -> Value {
    match role {
        ChatRole::System => Value::String("system".to_string()),
        ChatRole::User => Value::String("user".to_string()),
        ChatRole::Assistant => Value::String("assistant".to_string()),
        ChatRole::Tool => Value::String("tool".to_string()),
    }
}

/// Insert a message into the database.
pub async fn create_message(
    pool: &SqlitePool,
    session_id: &str,
    message: &ChatMessage,
    token_estimate: Option<usize>,
) -> Result<StoredMessage, AgentError> {
    let mut connection = pool
        .acquire()
        .await
        .map_err(|e| AgentError::Storage(format!("Failed to acquire message connection: {e}")))?;
    insert_message(&mut connection, session_id, message, token_estimate).await
}

async fn insert_message(
    connection: &mut sqlx::SqliteConnection,
    session_id: &str,
    message: &ChatMessage,
    token_estimate: Option<usize>,
) -> Result<StoredMessage, AgentError> {
    let (content, structured_json) = {
        // Try to join text parts for the plain content field
        let joined = message.content.joined_texts().unwrap_or_default();

        // Extract parts for structured storage
        let parts: Vec<ContentPart> = message.content.parts().clone();
        if parts.is_empty() || (parts.len() == 1 && matches!(&parts[0], ContentPart::Text(_))) {
            (joined, None)
        } else {
            let json = serde_json::to_value(parts)
                .map_err(|e| AgentError::Storage(format!("Failed to serialize message: {e}")))?;
            (String::new(), Some(json))
        }
    };

    let role_value = role_to_value(&message.role);
    let msg_id = uuid::Uuid::new_v4().to_string();

    sqlx::query(
        r#"
        INSERT INTO messages (id, chat_session_id, role, content, structured_content_json, token_estimate, created_at)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
        "#,
    )
    .bind(&msg_id)
    .bind(session_id)
    .bind(&role_value)
    .bind(&content)
    .bind(&structured_json)
    .bind(token_estimate.map(|t| t as i64))
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(&mut *connection)
    .await
    .map_err(|e| AgentError::Storage(format!("Failed to create message: {e}")))?;

    debug!(session_id, "created message");

    // Query back the row to return the actual stored values with the correct ID
    let stored: StoredMessage = sqlx::query_as::<_, StoredMessage>(
        r#"SELECT id, chat_session_id, role, content, structured_content_json, token_estimate, created_at
           FROM messages WHERE id = ?1"#,
    )
    .bind(&msg_id)
    .fetch_one(&mut *connection)
    .await
    .map_err(|e| AgentError::Storage(format!("Failed to retrieve inserted message: {e}")))?;

    Ok(stored)
}

/// Commit a complete assistant-call/result batch together. A failed insert or
/// commit is returned to the caller; a result is never stored independently.
pub async fn create_tool_exchange(
    pool: &SqlitePool,
    session_id: &str,
    assistant: &ChatMessage,
    results: &ChatMessage,
) -> Result<(), AgentError> {
    let mut transaction = pool
        .begin()
        .await
        .map_err(|e| AgentError::Storage(format!("Failed to begin tool exchange: {e}")))?;
    insert_message(&mut transaction, session_id, assistant, None).await?;
    insert_message(&mut transaction, session_id, results, None).await?;
    transaction
        .commit()
        .await
        .map_err(|e| AgentError::Storage(format!("Failed to commit tool exchange: {e}")))
}

/// Decode snapshots produced by both legacy NerdBot and current genai types.
pub fn deserialize_snapshot(snapshot: &str) -> Result<Vec<ChatMessage>, AgentError> {
    let values: Vec<Value> = serde_json::from_str(snapshot)
        .map_err(|e| AgentError::Storage(format!("Invalid history snapshot: {e}")))?;
    values
        .into_iter()
        .map(|value| {
            if let Ok(message) = serde_json::from_value::<ChatMessage>(value.clone()) {
                return Ok(message);
            }
            let stored = StoredMessage {
                id: String::new(),
                chat_session_id: String::new(),
                role: value.get("role").cloned().unwrap_or(Value::Null),
                content: value
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                structured_content_json: value.get("content").filter(|v| !v.is_string()).cloned(),
                token_estimate: None,
                created_at: chrono::Utc::now(),
            };
            stored.to_message()
        })
        .collect()
}

/// List messages for a session, ordered by creation time.
pub async fn list_messages(
    pool: &SqlitePool,
    session_id: &str,
    limit: Option<usize>,
) -> Result<Vec<StoredMessage>, AgentError> {
    let query = if limit.is_some() {
        "SELECT id, chat_session_id, role, content, structured_content_json, token_estimate, created_at FROM messages WHERE chat_session_id = ?1 ORDER BY created_at DESC, rowid DESC LIMIT ?2"
    } else {
        "SELECT id, chat_session_id, role, content, structured_content_json, token_estimate, created_at FROM messages WHERE chat_session_id = ?1 ORDER BY created_at DESC, rowid DESC"
    };

    let mut query = sqlx::query_as::<_, StoredMessage>(query).bind(session_id);
    if let Some(limit) = limit {
        let limit = i64::try_from(limit)
            .map_err(|e| AgentError::Storage(format!("Invalid message limit: {e}")))?;
        query = query.bind(limit);
    }

    query
        .fetch_all(pool)
        .await
        .map_err(|e| AgentError::Storage(format!("Failed to list messages: {e}")))
}
