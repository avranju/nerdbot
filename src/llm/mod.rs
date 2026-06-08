//! LLM provider layer — wraps the `genai` crate for provider-agnostic access.
//!
//! Uses `genai` for all provider-specific HTTP/serialization.
//! Our `LlmClient` wraps `genai::Client` with optional endpoint/auth overrides
//! from our config.
//!
//! A `FakeProvider` is kept for testing the agent loop without hitting real APIs.

use std::time::Duration;

use async_trait::async_trait;
use genai::adapter::AdapterKind;
use genai::chat::{ChatOptions, ChatRequest, ChatResponse};
use genai::resolver::{AuthData, Endpoint};
use reqwest::StatusCode;
use tracing::warn;

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
    max_retries: u32,
    retry_interval: Duration,
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
        let endpoint = config.llm.endpoint.as_deref().map(normalize_endpoint);
        let api_key = if let Some(ref env_var) = config.llm.api_key_env {
            Some(std::env::var(env_var).map_err(|_| {
                AgentError::Config(format!("Missing LLM API key env var: {env_var}"))
            })?)
        } else if endpoint.is_some() {
            // OpenAI-compatible local servers often do not require auth, but
            // genai's OpenAI adapter still expects a single auth value.
            Some(String::new())
        } else {
            None
        };

        // Custom endpoints are documented as OpenAI-compatible. Binding the
        // adapter avoids genai's fallback to native Ollama routing for model
        // names that do not have a recognized provider prefix.
        if endpoint.is_some() {
            builder = builder.with_adapter_kind(AdapterKind::OpenAI);
        }

        // Only install a resolver if at least one override is configured.
        if endpoint.is_some() || api_key.is_some() {
            builder =
                builder.with_service_target_resolver_fn(move |mut target: genai::ServiceTarget| {
                    if let Some(ref ep) = endpoint {
                        target.endpoint = Endpoint::from_owned(ep.clone());
                    }
                    if let Some(ref key) = api_key {
                        target.auth = AuthData::from_single(key.clone());
                    }
                    Ok(target)
                });
        }

        Ok(Self {
            client: builder.build(),
            max_retries: config.llm.max_retries,
            retry_interval: Duration::from_secs(config.llm.retry_interval_secs),
        })
    }
}

fn normalize_endpoint(endpoint: &str) -> String {
    if endpoint.ends_with('/') {
        endpoint.into()
    } else {
        format!("{endpoint}/")
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
        let mut attempt = 0;

        loop {
            match self
                .client
                .exec_chat(model, request.clone(), Some(&options))
                .await
            {
                Ok(response) => return Ok(response),
                Err(err) if attempt < self.max_retries && is_transient_llm_error(&err) => {
                    attempt += 1;
                    warn!(
                        attempt,
                        max_retries = self.max_retries,
                        retry_interval_secs = self.retry_interval.as_secs(),
                        error = %err,
                        "transient LLM call failed; retrying"
                    );
                    tokio::time::sleep(self.retry_interval).await;
                }
                Err(err) => return Err(AgentError::LlmProvider(err.to_string())),
            }
        }
    }
}

fn is_transient_llm_error(error: &genai::Error) -> bool {
    match error {
        genai::Error::WebAdapterCall { webc_error, .. }
        | genai::Error::WebModelCall { webc_error, .. } => is_transient_webc_error(webc_error),
        genai::Error::HttpError { status, .. } => is_transient_status(*status),
        genai::Error::WebStream { error, .. } => error
            .downcast_ref::<reqwest::Error>()
            .is_some_and(is_transient_reqwest_error),
        _ => false,
    }
}

fn is_transient_webc_error(error: &genai::webc::Error) -> bool {
    match error {
        genai::webc::Error::ResponseFailedStatus { status, .. } => is_transient_status(*status),
        genai::webc::Error::Reqwest(err) => is_transient_reqwest_error(err),
        _ => false,
    }
}

fn is_transient_reqwest_error(error: &reqwest::Error) -> bool {
    error.is_connect() || error.is_timeout() || (error.is_request() && !error.is_builder())
}

fn is_transient_status(status: StatusCode) -> bool {
    status == StatusCode::TOO_MANY_REQUESTS
        || status == StatusCode::REQUEST_TIMEOUT
        || status.is_server_error()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn custom_endpoint_routes_unknown_model_through_openai_adapter() {
        let mut config = AppConfig::default();
        config.llm.endpoint = Some("http://localhost:8001/v1".into());

        let client = LlmClient::from_config(&config).unwrap();
        let target = client
            .client
            .resolve_service_target("qwen3.6-35b-a3b")
            .await
            .unwrap();

        assert_eq!(target.model.adapter_kind, AdapterKind::OpenAI);
        assert_eq!(target.endpoint.base_url(), "http://localhost:8001/v1/");
        assert_eq!(target.auth.single_key_value().unwrap(), "");
    }

    #[test]
    fn retry_config_is_loaded_from_app_config() {
        let mut config = AppConfig::default();
        config.llm.max_retries = 7;
        config.llm.retry_interval_secs = 3;

        let client = LlmClient::from_config(&config).unwrap();

        assert_eq!(client.max_retries, 7);
        assert_eq!(client.retry_interval, Duration::from_secs(3));
    }

    #[test]
    fn http_retry_classification_only_retries_transient_statuses() {
        assert!(is_transient_status(StatusCode::REQUEST_TIMEOUT));
        assert!(is_transient_status(StatusCode::TOO_MANY_REQUESTS));
        assert!(is_transient_status(StatusCode::BAD_GATEWAY));
        assert!(!is_transient_status(StatusCode::UNAUTHORIZED));
        assert!(!is_transient_status(StatusCode::BAD_REQUEST));
    }

    #[test]
    fn endpoint_normalization_preserves_existing_trailing_slash() {
        assert_eq!(
            normalize_endpoint("http://localhost:8001/v1/"),
            "http://localhost:8001/v1/"
        );
    }
}
