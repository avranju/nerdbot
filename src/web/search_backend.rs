//! Web search backend backed by the Exa API.
//!
//! Uses the Exa REST API at `https://api.exa.ai/search` for web search
//! with optional content extraction (highlights).

use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::error::WebSearchError;

// ── Exa API types ─────────────────────────────────────────────────────────

/// Search result returned by the Exa API.
#[derive(Debug, Clone, Deserialize, Serialize)]
struct ExaSearchResult {
    title: String,
    url: String,
    id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    published_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    author: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(default)]
    highlights: Vec<String>,
    #[serde(default)]
    summary: Option<String>,
}

/// Response from the Exa search API.
#[derive(Debug, Deserialize)]
struct ExaSearchResponse {
    results: Vec<ExaSearchResult>,
}

// ── Public types ──────────────────────────────────────────────────────────

/// A search result entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub rank: Option<usize>,
}

/// Trait for web search backends.
#[async_trait::async_trait]
pub trait WebSearchBackend: Send + Sync {
    /// Perform a search and return results.
    async fn search(&self, query: &str) -> Result<Vec<SearchResult>, WebSearchError>;
}

/// Exa-powered search backend.
pub struct ExaSearchBackend {
    client: reqwest::Client,
    pub(crate) api_key: String,
    max_results: usize,
}

impl ExaSearchBackend {
    /// Create a new Exa search backend.
    pub fn new(api_key: String, max_results: usize) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("Failed to build HTTP client");

        Self {
            client,
            api_key,
            max_results,
        }
    }
}

#[async_trait::async_trait]
impl WebSearchBackend for ExaSearchBackend {
    async fn search(&self, query: &str) -> Result<Vec<SearchResult>, WebSearchError> {
        if query.trim().is_empty() {
            return Err(WebSearchError::InvalidQuery);
        }

        debug!(query, "performing Exa web search");

        let body = serde_json::json!({
            "query": query.trim(),
            "numResults": self.max_results,
            "type": "auto",
            "contents": {
                "highlights": true,
                "maxAgeHours": 720
            }
        });

        let resp = self
            .client
            .post("https://api.exa.ai/search")
            .header("x-api-key", &self.api_key)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| WebSearchError::Backend(format!("Request failed: {e}")))?;

        let status = resp.status();

        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(WebSearchError::RateLimited);
        }

        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_else(|_| "no body".to_string());
            return Err(WebSearchError::Backend(format!(
                "API error {status}: {body_text}"
            )));
        }

        let api_resp: ExaSearchResponse = resp
            .json()
            .await
            .map_err(|e| WebSearchError::Backend(format!("Failed to parse response: {e}")))?;

        let results: Vec<SearchResult> = api_resp
            .results
            .into_iter()
            .enumerate()
            .map(|(i, r)| {
                // Prefer highlights as snippet, fall back to text, then empty
                let snippet = if !r.highlights.is_empty() {
                    r.highlights.join(" ")
                } else if let Some(text) = r.text {
                    text.chars().take(500).collect()
                } else {
                    String::new()
                };

                SearchResult {
                    title: r.title,
                    url: r.url,
                    snippet,
                    rank: Some(i + 1),
                }
            })
            .collect();

        debug!(count = results.len(), "search complete");
        Ok(results)
    }
}

/// Skeleton stub — no actual search backend implemented yet.
pub struct NoopSearchBackend;

#[async_trait::async_trait]
impl WebSearchBackend for NoopSearchBackend {
    async fn search(&self, _query: &str) -> Result<Vec<SearchResult>, WebSearchError> {
        Err(WebSearchError::Backend(
            "No search backend configured".into(),
        ))
    }
}
