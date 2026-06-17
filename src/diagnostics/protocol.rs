//! Newline-delimited JSON protocol for the local diagnostics socket.

use serde::{Deserialize, Serialize};

use crate::context::compaction_service::CompactionState;
use crate::context::diagnostics::ContextDiagnosticsSnapshot;

/// Request accepted by the diagnostics Unix socket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DiagnosticsRequest {
    Ping,
    ListSessions,
    ShowSession {
        #[serde(default)]
        session_id: Option<String>,
        #[serde(default)]
        channel_id: Option<String>,
        #[serde(default)]
        conversation_id: Option<String>,
        #[serde(default)]
        thread_id: Option<String>,
        #[serde(default)]
        include_prompts: bool,
    },
}

/// Compact session identity returned by list and show operations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionDiagnostics {
    pub id: String,
    pub channel_id: String,
    pub conversation_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<crate::storage::sessions::ChatSession> for SessionDiagnostics {
    fn from(session: crate::storage::sessions::ChatSession) -> Self {
        Self {
            id: session.id,
            channel_id: session.channel_id,
            conversation_id: session.conversation_id,
            thread_id: session.thread_id,
            created_at: session.created_at,
            updated_at: session.updated_at,
        }
    }
}

/// Diagnostics for the effective personality prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersonalityDiagnostics {
    pub char_count: usize,
    pub token_estimate: usize,
    pub timezone: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
}

/// Diagnostics for the compaction summary prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SummaryPromptDiagnostics {
    pub char_count: usize,
    pub token_estimate: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
}

/// Diagnostics for the registered tools.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolSpecDiagnostics {
    pub count: usize,
    pub token_estimate: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub specs: Option<Vec<serde_json::Value>>,
}

/// Response emitted by the diagnostics Unix socket.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DiagnosticsResponse {
    Pong,
    Sessions {
        sessions: Vec<SessionDiagnostics>,
    },
    Session {
        session: SessionDiagnostics,
        context: Box<ContextDiagnosticsSnapshot>,
        compaction_state: CompactionState,
        effective_personality: Box<PersonalityDiagnostics>,
        summary_prompt: Box<SummaryPromptDiagnostics>,
        tool_spec: Box<ToolSpecDiagnostics>,
    },
    Error {
        message: String,
    },
}
