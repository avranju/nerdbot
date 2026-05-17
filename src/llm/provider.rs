//! LLM provider trait — the adapter boundary for all model backends.


use crate::error::AgentError;
use crate::llm::types::{ModelRequest, ModelResponse, TokenEstimate};

/// Trait that all LLM provider adapters must implement.
///
/// Providers (OpenAI, Anthropic, Gemini, OpenRouter, OpenAI-compatible)
/// translate their native request/response shapes into the
/// provider-neutral types defined in [`crate::llm::types`].
#[async_trait::async_trait]
pub trait LlmProvider: Send + Sync {
    /// Send a model request and return a normalized response.
    ///
    /// The provider is responsible for:
    /// - Serializing the request into its native wire format
    /// - Handling authentication and retries at the provider level
    /// - Normalizing the response into [`ModelResponse`]
    async fn complete(
        &self,
        request: ModelRequest,
    ) -> Result<ModelResponse, AgentError>;

    /// Estimate token consumption for a request.
    ///
    /// Implementations may return a heuristic estimate if the provider
    /// does not expose an official token counter.
    async fn estimate_tokens(
        &self,
        request: &ModelRequest,
    ) -> Result<TokenEstimate, AgentError>;
}
