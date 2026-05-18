//! Fake/mock LLM provider for testing.
//!
//! Allows tests to control provider responses via a response sequence
//! (e.g. "return tool calls for the first N calls, then return final text")
//! and inspects the messages sent to the provider.

use crate::error::AgentError;
use crate::llm::provider::LlmProvider;
use crate::llm::types::{ModelRequest, ModelResponse, TokenEstimate};

/// A pre-recorded response that the fake provider returns for a single call.
#[derive(Debug, Clone)]
pub struct FakeResponse {
    /// Assistant text when no tool calls are expected.
    pub assistant_text: Option<String>,
    /// Tool calls the model wants to make.
    pub tool_calls: Vec<crate::llm::types::ToolCall>,
    /// Finish reason.
    pub finish_reason: crate::llm::types::FinishReason,
    /// Optional token usage metadata.
    pub token_usage: Option<(usize, usize)>, // (input, output)
}

impl FakeResponse {
    /// Create a response with final text (no tool calls).
    pub fn final_text(text: impl Into<String>) -> Self {
        Self {
            assistant_text: Some(text.into()),
            tool_calls: Vec::new(),
            finish_reason: crate::llm::types::FinishReason::Completed,
            token_usage: None,
        }
    }

    /// Create a response with a single tool call.
    pub fn tool_call(name: impl Into<String>, args: serde_json::Value) -> Self {
        Self {
            assistant_text: None,
            tool_calls: vec![crate::llm::types::ToolCall {
                id: "fc_1".into(),
                name: name.into(),
                arguments: args,
            }],
            finish_reason: crate::llm::types::FinishReason::ToolUse,
            token_usage: None,
        }
    }

    /// Create a response with multiple tool calls.
    pub fn tool_calls(calls: Vec<crate::llm::types::ToolCall>) -> Self {
        Self {
            assistant_text: None,
            tool_calls: calls,
            finish_reason: crate::llm::types::FinishReason::ToolUse,
            token_usage: None,
        }
    }

    /// Create a provider error response.
    pub fn error(msg: impl Into<String>) -> Self {
        Self {
            assistant_text: None,
            tool_calls: Vec::new(),
            finish_reason: crate::llm::types::FinishReason::Error,
            token_usage: None,
        }
    }
}

/// A mock LLM provider that returns pre-configured responses in sequence.
///
/// The provider cycles through the response sequence on each `complete()` call.
/// When the sequence is exhausted, it returns the last response again.
///
/// This is the primary tool for testing the agent loop end-to-end without
/// hitting a real LLM API.
#[derive(Debug, Default)]
pub struct FakeProvider {
    /// Responses to return in sequence.
    responses: Vec<FakeResponse>,
    /// Index of the next response to return.
    index: std::sync::atomic::AtomicUsize,
    /// The last request received (for inspection in tests).
    last_request: std::sync::RwLock<Option<ModelRequest>>,
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
    pub fn last_request(&self) -> Option<ModelRequest> {
        self.last_request.read().unwrap().clone()
    }

    /// Get the number of tool calls received across all requests.
    pub fn total_tool_calls_received(&self) -> usize {
        let lock = self.last_request.read().unwrap();
        if let Some(req) = lock.as_ref() {
            req.messages
                .iter()
                .filter(|m| {
                    matches!(m.role, crate::llm::types::Role::Assistant)
                        || matches!(m.role, crate::llm::types::Role::Tool)
                })
                .flat_map(|m| match &m.content {
                    crate::llm::types::MessageContent::Parts(parts) => parts
                    .iter()
                    .filter_map(|p| match p {
                        crate::llm::types::ContentPart::ToolCall(_) => Some(()),
                        crate::llm::types::ContentPart::ToolResult(_) => Some(()),
                        _ => None,
                    })
                    .collect::<Vec<_>>(),
                    crate::llm::types::MessageContent::Text(_) => Vec::new(),
                })
                .count()
        } else {
            0
        }
    }

    /// Reset the call count and last request (useful for multiple test scenarios).
    pub fn reset(&self) {
        self.call_count.store(0, std::sync::atomic::Ordering::SeqCst);
        *self.last_request.write().unwrap() = None;
    }

    /// Create a response sequence: one tool call, then final text.
    ///
    /// This is the most common pattern: the LLM proposes a tool, the harness
    /// executes it, and the LLM returns the final answer.
    pub fn tool_then_final(text: impl Into<String>) -> Self {
        Self::new(vec![
            FakeResponse::tool_call("test_tool", serde_json::json!({})),
            FakeResponse::final_text(text),
        ])
    }

    /// Create a response sequence: multiple tool calls in first response,
    /// then final text.
    pub fn multi_tool_then_final(text: impl Into<String>) -> Self {
        Self::new(vec![
            FakeResponse::tool_calls(vec![
                crate::llm::types::ToolCall {
                    id: "fc_1".into(),
                    name: "tool_a".into(),
                    arguments: serde_json::json!({"x": 1}),
                },
                crate::llm::types::ToolCall {
                    id: "fc_2".into(),
                    name: "tool_b".into(),
                    arguments: serde_json::json!({"y": 2}),
                },
            ]),
            FakeResponse::final_text(text),
        ])
    }
}

#[async_trait::async_trait]
impl LlmProvider for FakeProvider {
    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, AgentError> {
        self.call_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        *self.last_request.write().unwrap() = Some(request.clone());

        let idx = self
            .index
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        let response = if idx < self.responses.len() {
            self.responses[idx].clone()
        } else {
            // Return the last response when sequence is exhausted
            self.responses.last().unwrap().clone()
        };

        // Check for error responses
        if response.finish_reason == crate::llm::types::FinishReason::Error {
            return Err(AgentError::LlmProvider(
                "Fake provider returning error response".into(),
            ));
        }

        let mut model_response = ModelResponse {
            assistant_text: response.assistant_text,
            tool_calls: response.tool_calls,
            finish_reason: response.finish_reason,
            provider_metadata: None,
        };

        // Attach token usage metadata if provided
        if let Some((input, output)) = response.token_usage {
            model_response.provider_metadata = Some(serde_json::json!({
                "usage": {
                    "input_tokens": input,
                    "output_tokens": output,
                }
            }));
        }

        Ok(model_response)
    }

    async fn estimate_tokens(
        &self,
        _request: &ModelRequest,
    ) -> Result<TokenEstimate, AgentError> {
        // Simple heuristic: ~4 chars per token
        let total_chars: usize = _request
            .messages
            .iter()
            .map(|m| match &m.content {
                crate::llm::types::MessageContent::Text(t) => t.len(),
                crate::llm::types::MessageContent::Parts(parts) => {
                    parts.iter().fold(0, |acc, p| match p {
                        crate::llm::types::ContentPart::Text(t) => acc + t.len(),
                        _ => acc,
                    })
                }
            })
            .sum();

        let estimated = total_chars / 4 + 10; // rough estimate
        Ok(TokenEstimate::new(estimated, estimated))
    }
}
