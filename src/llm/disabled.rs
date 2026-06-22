//! Disabled LLM executor used during maintenance mode.
//!
/// This implementation always returns an error if invoked, because
/// maintenance mode should short-circuit before any LLM call is made.
use async_trait::async_trait;
use genai::chat::{ChatOptions, ChatRequest, ChatResponse};

use super::LlmExecutor;
use crate::error::AgentError;

/// A no-op LLM executor that returns an error on every call.
///
/// Used when maintenance mode is active so the runtime can start
/// without requiring a configured or reachable LLM endpoint.
#[derive(Clone)]
pub struct DisabledLlm;

#[async_trait]
impl LlmExecutor for DisabledLlm {
    async fn complete(
        &self,
        _model: &str,
        _request: ChatRequest,
        _options: ChatOptions,
    ) -> Result<ChatResponse, AgentError> {
        Err(AgentError::Config(
            "LLM is disabled while maintenance mode is active".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn disabled_llm_returns_config_error() {
        let llm = DisabledLlm;
        let result = llm
            .complete("gpt-4o", ChatRequest::default(), ChatOptions::default())
            .await;
        assert!(matches!(result, Err(AgentError::Config(msg)) if msg.contains("maintenance mode")));
    }
}
