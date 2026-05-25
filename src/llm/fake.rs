//! Fake/mock LLM provider for testing.
//!
//! Allows tests to control provider responses via a response sequence
//! (e.g. "return tool calls for the first N calls, then return final text")
//! and inspects the messages sent to the provider.

use async_trait::async_trait;

use genai::chat::{
    ChatMessage, ChatOptions, ChatRequest, ChatResponse, ChatRole, ContentPart, MessageContent,
    StopReason, ToolCall, Usage,
};

use crate::error::AgentError;
use crate::llm::LlmExecutor;

/// A pre-recorded response that the fake provider returns for a single call.
#[derive(Debug, Clone)]
pub struct FakeResponse {
    /// Assistant text when no tool calls are expected.
    pub assistant_text: Option<String>,
    /// Tool calls the model wants to make.
    pub tool_calls: Vec<ToolCall>,
    /// Finish reason.
    pub stop_reason: Option<StopReason>,
    /// Optional token usage metadata.
    pub token_usage: Option<(i32, i32)>, // (input, output)
}

impl FakeResponse {
    /// Create a response with final text (no tool calls).
    pub fn final_text(text: impl Into<String>) -> Self {
        Self {
            assistant_text: Some(text.into()),
            tool_calls: Vec::new(),
            stop_reason: Some(StopReason::Completed("stop".to_string())),
            token_usage: None,
        }
    }

    /// Create a response with a single tool call.
    pub fn tool_call(name: impl Into<String>, args: serde_json::Value) -> Self {
        Self {
            assistant_text: None,
            tool_calls: vec![ToolCall {
                call_id: "fc_1".into(),
                fn_name: name.into(),
                fn_arguments: args,
                thought_signatures: None,
            }],
            stop_reason: Some(StopReason::ToolCall("function_call".to_string())),
            token_usage: None,
        }
    }

    /// Create a response with multiple tool calls.
    pub fn tool_calls(calls: Vec<ToolCall>) -> Self {
        Self {
            assistant_text: None,
            tool_calls: calls,
            stop_reason: Some(StopReason::ToolCall("function_call".to_string())),
            token_usage: None,
        }
    }

    /// Create a provider error response.
    pub fn error(_msg: impl Into<String>) -> Self {
        Self {
            assistant_text: None,
            tool_calls: Vec::new(),
            stop_reason: Some(StopReason::Other("error".to_string())),
            token_usage: None,
        }
    }
}

/// A mock LLM provider that returns pre-configured responses in sequence.
///
/// The provider cycles through the response sequence on each `complete()` call.
/// When the sequence is exhausted, it returns the last response again.
#[derive(Debug, Default)]
pub struct FakeProvider {
    /// Responses to return in sequence.
    responses: Vec<FakeResponse>,
    /// Index of the next response to return.
    index: std::sync::atomic::AtomicUsize,
    /// The last request received (for inspection in tests).
    last_request: std::sync::RwLock<Option<ChatRequest>>,
    /// Count of how many times `complete` was called.
    call_count: std::sync::atomic::AtomicUsize,
}

impl FakeProvider {
    /// Create a new fake provider with the given response sequence.
    pub fn new(responses: Vec<FakeResponse>) -> Self {
        Self {
            responses,
            index: std::sync::atomic::AtomicUsize::new(0),
            last_request: std::sync::RwLock::new(None),
            call_count: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// Return the number of times `complete` has been called.
    pub fn call_count(&self) -> usize {
        self.call_count.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Return the last request received (for inspection in tests).
    pub fn last_request(&self) -> Option<ChatRequest> {
        self.last_request.read().unwrap().clone()
    }

    /// Get the number of tool calls received across all requests.
    pub fn total_tool_calls_received(&self) -> usize {
        let lock = self.last_request.read().unwrap();
        if let Some(ref req) = *lock {
            req.messages
                .iter()
                .filter(|m| matches!(m.role, ChatRole::Assistant | ChatRole::Tool))
                .flat_map(|m| {
                    m.content.parts().iter().filter_map(|p| {
                        matches!(p, ContentPart::ToolCall(_) | ContentPart::ToolResponse(_))
                            .then_some(())
                    })
                })
                .count()
        } else {
            0
        }
    }

    /// Reset the call count and last request (useful for multiple test scenarios).
    pub fn reset(&self) {
        self.call_count
            .store(0, std::sync::atomic::Ordering::SeqCst);
        *self.last_request.write().unwrap() = None;
    }

    /// Create a response sequence: one tool call, then final text.
    pub fn tool_then_final(text: impl Into<String>) -> Self {
        Self::new(vec![
            FakeResponse::tool_call("test_tool", serde_json::json!({})),
            FakeResponse::final_text(text),
        ])
    }

    /// Create a response sequence: multiple tool calls in first response, then final text.
    pub fn multi_tool_then_final(text: impl Into<String>) -> Self {
        Self::new(vec![
            FakeResponse::tool_calls(vec![
                ToolCall {
                    call_id: "fc_1".into(),
                    fn_name: "tool_a".into(),
                    fn_arguments: serde_json::json!({"x": 1}),
                    thought_signatures: None,
                },
                ToolCall {
                    call_id: "fc_2".into(),
                    fn_name: "tool_b".into(),
                    fn_arguments: serde_json::json!({"y": 2}),
                    thought_signatures: None,
                },
            ]),
            FakeResponse::final_text(text),
        ])
    }
}

#[async_trait]
impl LlmExecutor for FakeProvider {
    async fn complete(
        &self,
        _model: &str,
        request: ChatRequest,
        _options: ChatOptions,
    ) -> Result<ChatResponse, AgentError> {
        self.call_count
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        *self.last_request.write().unwrap() = Some(request);

        let idx = self.index.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        let response = if idx < self.responses.len() {
            self.responses[idx].clone()
        } else {
            self.responses.last().unwrap().clone()
        };

        // Check for error responses
        if matches!(response.stop_reason, Some(StopReason::Other(ref s)) if s == "error") {
            return Err(AgentError::LlmProvider(
                "Fake provider returning error response".into(),
            ));
        }

        let content = match &response.assistant_text {
            Some(text) if !text.is_empty() => MessageContent::from_text(text),
            _ if !response.tool_calls.is_empty() => {
                MessageContent::from_tool_calls(response.tool_calls.clone())
            }
            _ => MessageContent::default(),
        };

        let usage = if let Some((input, output)) = response.token_usage {
            Usage {
                prompt_tokens: Some(input),
                completion_tokens: Some(output),
                total_tokens: Some(input + output),
                prompt_tokens_details: None,
                completion_tokens_details: None,
            }
        } else {
            Usage::default()
        };

        Ok(ChatResponse {
            content,
            reasoning_content: None,
            model_iden: genai::ModelIden::new(genai::adapter::AdapterKind::OpenAI, "fake-model"),
            provider_model_iden: genai::ModelIden::new(
                genai::adapter::AdapterKind::OpenAI,
                "fake-model",
            ),
            stop_reason: response.stop_reason,
            usage,
            captured_raw_body: None,
            response_id: None,
        })
    }
}
