//! Provider-neutral core types for the agent runtime.
//!
//! All provider adapters normalize their responses into these types,
//! so the agent loop and tool system never see provider-specific APIs.

use serde::{Deserialize, Serialize};

/// Message role in a conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

/// A message in a conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    /// The sender role.
    pub role: Role,
    /// The content of the message.
    pub content: MessageContent,
    /// Optional metadata (provider-specific extras).
    pub metadata: Option<serde_json::Value>,
}

/// Content carried by a message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessageContent {
    /// Plain text content.
    Text(String),
    /// Structured parts (text + tool calls/results).
    Parts(Vec<ContentPart>),
}

/// A single content part within a message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContentPart {
    Text(String),
    ToolCall(ToolCall),
    ToolResult(ToolResult),
}

/// A specification for a tool the LLM can call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolSpec {
    /// Unique name of the tool.
    pub name: String,
    /// Human-readable description of what the tool does.
    pub description: String,
    /// JSON Schema describing the tool's input arguments.
    pub input_schema: serde_json::Value,
}

/// A tool invocation returned by the LLM.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    /// Opaque identifier for correlating call → result.
    pub id: String,
    /// Name of the tool to invoke.
    pub name: String,
    /// JSON-encoded arguments.
    pub arguments: serde_json::Value,
}

/// The result of executing a tool call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolResult {
    /// The tool_call_id this result corresponds to.
    pub tool_call_id: String,
    /// Whether execution succeeded or failed.
    pub status: ToolExecutionStatus,
    /// Structured result content.
    pub content: serde_json::Value,
}

/// Execution status of a tool call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum ToolExecutionStatus {
    Success,
    Error { error: String },
}

impl ToolExecutionStatus {
    pub fn is_success(&self) -> bool {
        matches!(self, ToolExecutionStatus::Success)
    }
}

/// A request sent to an LLM provider.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelRequest {
    /// Conversation messages.
    pub messages: Vec<Message>,
    /// Available tools the model may call.
    pub tools: Vec<ToolSpec>,
    /// Model identifier (provider-specific).
    pub model: String,
    /// Sampling temperature.
    pub temperature: Option<f32>,
    /// Maximum output tokens.
    pub max_output_tokens: Option<u32>,
    /// Optional provider-agnostic metadata.
    pub metadata: Option<serde_json::Value>,
}

/// Response from an LLM provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelResponse {
    /// Final assistant text (present when no tool calls).
    pub assistant_text: Option<String>,
    /// Tool calls the model wants to make.
    pub tool_calls: Vec<ToolCall>,
    /// Why the model stopped.
    pub finish_reason: FinishReason,
    /// Provider-specific metadata (kept opaque).
    pub provider_metadata: Option<serde_json::Value>,
}

/// Reason the model stopped generating.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FinishReason {
    /// Model returned final text with no tool calls.
    Completed,
    /// Model returned one or more tool calls.
    ToolUse,
    /// Output reached max tokens.
    Length,
    /// Content safety filter triggered.
    Safety,
    /// Provider-side error.
    Error,
    /// Unknown or unexpected reason.
    Unknown(String),
}

/// Estimate of token consumption for a request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenEstimate {
    /// Estimated input tokens.
    pub input_tokens: usize,
    /// Estimated output tokens.
    pub output_tokens: usize,
    /// Total estimated tokens.
    pub total_tokens: usize,
}

impl TokenEstimate {
    pub fn new(input_tokens: usize, output_tokens: usize) -> Self {
        let total_tokens = input_tokens + output_tokens;
        Self {
            input_tokens,
            output_tokens,
            total_tokens,
        }
    }
}

/// Build a Message with text content.
impl Message {
    pub fn new(role: Role, content: MessageContent) -> Self {
        Self {
            role,
            content,
            metadata: None,
        }
    }

    pub fn system(text: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: MessageContent::Text(text.into()),
            metadata: None,
        }
    }

    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: MessageContent::Text(text.into()),
            metadata: None,
        }
    }

    pub fn assistant(text: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: MessageContent::Text(text.into()),
            metadata: None,
        }
    }

    pub fn assistant_tool_calls(tool_calls: Vec<ToolCall>) -> Self {
        let parts: Vec<ContentPart> = tool_calls.into_iter().map(ContentPart::ToolCall).collect();
        Self {
            role: Role::Assistant,
            content: MessageContent::Parts(parts),
            metadata: None,
        }
    }

    pub fn tool_result(
        call: &ToolCall,
        status: ToolExecutionStatus,
        content: serde_json::Value,
    ) -> Self {
        Self {
            role: Role::Tool,
            content: MessageContent::Parts(vec![ContentPart::ToolResult(ToolResult {
                tool_call_id: call.id.clone(),
                status,
                content,
            })]),
            metadata: None,
        }
    }

    /// Create a tool message from a list of tool results.
    ///
    /// Each result becomes a separate ContentPart within a single Tool message.
    pub fn with_tool_results(results: Vec<ToolResult>) -> Self {
        let parts: Vec<ContentPart> = results.into_iter().map(ContentPart::ToolResult).collect();
        Self {
            role: Role::Tool,
            content: MessageContent::Parts(parts),
            metadata: None,
        }
    }
}

impl ModelRequest {
    /// Add messages from the front (e.g. system + summary + recent).
    pub fn with_messages(mut self, messages: Vec<Message>) -> Self {
        self.messages = messages;
        self
    }

    /// Add available tools.
    pub fn with_tools(mut self, tools: Vec<ToolSpec>) -> Self {
        self.tools = tools;
        self
    }

    /// Add tool results to the request (for the next iteration).
    pub fn with_tool_results(mut self, results: Vec<ToolResult>) -> Self {
        let parts: Vec<ContentPart> = results.into_iter().map(ContentPart::ToolResult).collect();
        self.messages
            .push(Message::new(Role::Assistant, MessageContent::Parts(parts)));
        self
    }
}

impl ToolSpec {
    /// Create a tool spec from a name, description, and JSON schema.
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema: serde_json::Value,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            input_schema,
        }
    }
}

impl ModelResponse {
    /// Check if the response contains tool calls (i.e., the loop should continue).
    pub fn has_tool_calls(&self) -> bool {
        !self.tool_calls.is_empty()
    }

    /// Check if the response is a final answer (no tool calls).
    pub fn is_final(&self) -> bool {
        self.finish_reason == FinishReason::Completed && self.tool_calls.is_empty()
    }
}
