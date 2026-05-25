//! LLM provider layer — wraps the `genai` crate for provider-agnostic access.
//!
//! Uses `genai` for all provider-specific HTTP/serialization.
//! Our `LlmClient` wraps `genai::Client` with optional endpoint/auth overrides
//! from our config.
//!
//! A `FakeProvider` is kept for testing the agent loop without hitting real APIs.

use async_trait::async_trait;
use genai::chat::{ChatOptions, ChatRequest, ChatResponse};
use genai::resolver::{AuthData, Endpoint, ServiceTargetResolver};

use crate::config::AppConfig;
use crate::error::AgentError;

pub mod fake;

/// Trait abstracting LLM chat completion.
///
/// Implemented by both the real `LlmClient` (wrapping `genai`) and
/// the `FakeProvider` (for tests).
#[async_trait]
pub trait LlmExecutor: Send + Sync {
    /// Execute a chat completion request for the given model.
    async fn complete(
        &self,
        model: &str,
        request: ChatRequest,
        options: ChatOptions,
    ) -> Result<ChatResponse, AgentError>;
}

/// Concrete LLM client wrapping `genai::Client`.
///
/// When config provides endpoint/auth overrides, uses a `ServiceTargetResolver`
/// to redirect requests. Otherwise relies on `genai`'s built-in model name
/// → adapter resolution.
#[derive(Clone)]
pub struct LlmClient {
    client: genai::Client,
}

impl LlmClient {
    /// Create an `LlmClient` from the application configuration.
    ///
    /// If `config.llm.endpoint` or `config.llm.api_key_env` is set,
    /// a `ServiceTargetResolver` is installed to override the default
    /// endpoint and/or auth for every request.
    pub fn from_config(config: &AppConfig) -> Result<Self, AgentError> {
        let mut builder = genai::Client::builder();

        // Cache optional overrides at construction time.
        let endpoint = config.llm.endpoint.clone();
        let api_key = if let Some(ref env_var) = config.llm.api_key_env {
            Some(
                std::env::var(env_var).map_err(|_| {
                    AgentError::Config(format!("Missing LLM API key env var: {env_var}"))
                })?,
            )
        } else {
            None
        };

        // Only install a resolver if at least one override is configured.
        if endpoint.is_some() || api_key.is_some() {
            builder = builder.with_service_target_resolver_fn(
                move |mut target: genai::ServiceTarget| {
                    if let Some(ref ep) = endpoint {
                        target.endpoint = Endpoint::from_owned(ep.clone());
                    }
                    if let Some(ref key) = api_key {
                        target.auth = AuthData::from_single(key.clone());
                    }
                    Ok(target)
                },
            );
        }

        Ok(Self {
            client: builder.build(),
        })
    }
}

#[async_trait]
impl LlmExecutor for LlmClient {
    async fn complete(
        &self,
        model: &str,
        request: ChatRequest,
        options: ChatOptions,
    ) -> Result<ChatResponse, AgentError> {
        self.client
            .exec_chat(model, request, Some(&options))
            .await
            .map_err(|e| AgentError::LlmProvider(e.to_string()))
    }
}
