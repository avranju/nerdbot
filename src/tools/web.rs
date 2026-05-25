//! Web tools — web_search and web_fetch backed by Exa.

use serde_json::json;

use crate::error::AgentError;
use crate::tools::traits::{Tool, ToolContext, ToolOutput};
use crate::web::fetcher::ExaFetcher;
use crate::web::search_backend::{ExaSearchBackend, WebSearchBackend};

// ── Web Search ────────────────────────────────────────────────────────────

/// Web search tool powered by the Exa API.
///
/// Searches the web for the given query and returns ranked results
/// with titles, URLs, and content highlights.
pub struct WebSearch {
    backend: Option<ExaSearchBackend>,
    default_max_results: usize,
}

impl WebSearch {
    /// Create a new WebSearch tool with the given Exa API key.
    pub fn new(api_key: String, default_max_results: usize) -> Self {
        Self {
            backend: Some(ExaSearchBackend::new(api_key, default_max_results)),
            default_max_results,
        }
    }
}

#[async_trait::async_trait]
impl Tool for WebSearch {
    fn name(&self) -> &'static str {
        "web_search"
    }

    fn description(&self) -> &'static str {
        "Search the web for information using Exa AI. Returns ranked results with titles, URLs, and content highlights/snippets. Use this to find current information, verify facts, or gather context on any topic."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query. Be specific and include relevant keywords for best results."
                },
                "num_results": {
                    "type": "integer",
                    "description": "Number of results to return (1-10). Defaults to 5.",
                    "minimum": 1,
                    "maximum": 10
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        _ctx: ToolContext,
    ) -> Result<ToolOutput, AgentError> {
        let query = args.get("query").and_then(|v| v.as_str()).ok_or_else(|| {
            AgentError::InvalidToolArgs("missing or invalid 'query' field".into())
        })?;

        let num_results = match args.get("num_results") {
            Some(v) => {
                let n = v.as_i64().ok_or_else(|| {
                    AgentError::InvalidToolArgs("'num_results' must be an integer".into())
                })?;
                if !(1..=10).contains(&n) {
                    return Err(AgentError::InvalidToolArgs(
                        "'num_results' must be between 1 and 10".into(),
                    ));
                }
                n as usize
            }
            None => self.default_max_results,
        };

        let backend = self
            .backend
            .as_ref()
            .ok_or_else(|| AgentError::WebSearch("Web search backend not configured".into()))?;

        // Override num_results for this call
        let backend = ExaSearchBackend::new(backend.api_key.clone(), num_results);

        let results = backend
            .search(query)
            .await
            .map_err(|e| AgentError::WebSearch(e.to_string()))?;

        if results.is_empty() {
            return Ok(ToolOutput {
                success: true,
                data: json!({
                    "results": [],
                    "query": query,
                    "message": "No results found"
                }),
                summary: format!("No results found for \"{query}\""),
            });
        }

        let results_json: Vec<serde_json::Value> = results
            .iter()
            .map(|r| {
                json!({
                    "rank": r.rank,
                    "title": r.title,
                    "url": r.url,
                    "snippet": r.snippet.chars().take(300).collect::<String>()
                })
            })
            .collect();

        let snippet_preview: Vec<String> = results
            .iter()
            .take(3)
            .map(|r| {
                format!(
                    "{}: {}",
                    r.title,
                    r.snippet.chars().take(150).collect::<String>()
                )
            })
            .collect();

        Ok(ToolOutput {
            success: true,
            data: json!({
                "query": query,
                "count": results.len(),
                "results": results_json,
            }),
            summary: format!(
                "Found {} results for \"{}\":\n{}",
                results.len(),
                query,
                snippet_preview.join("\n")
            ),
        })
    }
}

// ── Web Fetch ─────────────────────────────────────────────────────────────

/// Web fetch tool powered by the Exa Contents API.
///
/// Fetches a URL and extracts readable content as clean markdown.
/// Includes SSRF protections to prevent accessing internal addresses.
pub struct WebFetch {
    fetcher: Option<ExaFetcher>,
}

impl WebFetch {
    /// Create a new WebFetch tool with the given Exa API key.
    pub fn new(api_key: String, max_text_chars: usize) -> Self {
        Self {
            fetcher: Some(ExaFetcher::new(api_key, max_text_chars)),
        }
    }
}

#[async_trait::async_trait]
impl Tool for WebFetch {
    fn name(&self) -> &'static str {
        "web_fetch"
    }

    fn description(&self) -> &'static str {
        "Fetch a URL and extract its full readable content as clean markdown. Includes SSRF protections to prevent accessing internal/private addresses. Use this to read the full content of an article, documentation page, or any web page."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The full URL to fetch (must start with http:// or https://)."
                }
            },
            "required": ["url"]
        })
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        _ctx: ToolContext,
    ) -> Result<ToolOutput, AgentError> {
        let url = args
            .get("url")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::InvalidToolArgs("missing or invalid 'url' field".into()))?;

        let fetcher = self
            .fetcher
            .as_ref()
            .ok_or_else(|| AgentError::WebFetch("Web fetcher not configured".into()))?;

        let page = fetcher
            .fetch(url)
            .await
            .map_err(|e| AgentError::WebFetch(e.to_string()))?;

        let len = page.text.len();
        let status = if page.truncated {
            "truncated"
        } else {
            "complete"
        };
        let display_title = page.title.as_deref().unwrap_or(url);
        let summary = format!("Fetched \"{display_title}\" ({len} chars, {status})");

        Ok(ToolOutput {
            success: true,
            data: json!({
                "url": url,
                "title": page.title,
                "text": page.text,
                "truncated": page.truncated,
                "text_length": len,
            }),
            summary,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_web_search_name() {
        let tool = WebSearch::new("test-key".into(), 5);
        assert_eq!(tool.name(), "web_search");
    }

    #[test]
    fn test_web_search_description() {
        let tool = WebSearch::new("test-key".into(), 5);
        let desc = tool.description();
        assert!(!desc.is_empty());
        assert!(desc.len() > 50);
        assert!(desc.to_lowercase().contains("search"));
    }

    #[test]
    fn test_web_search_schema() {
        let tool = WebSearch::new("test-key".into(), 5);
        let schema = tool.input_schema();
        assert!(schema.is_object());
        assert!(
            schema["required"]
                .as_array()
                .unwrap()
                .contains(&json!("query"))
        );
        assert!(schema["properties"]["query"]["type"].as_str().unwrap() == "string");
    }

    #[test]
    fn test_web_fetch_name() {
        let tool = WebFetch::new("test-key".into(), 8000);
        assert_eq!(tool.name(), "web_fetch");
    }

    #[test]
    fn test_web_fetch_description() {
        let tool = WebFetch::new("test-key".into(), 8000);
        let desc = tool.description();
        assert!(!desc.is_empty());
        assert!(desc.len() > 50);
        assert!(desc.to_lowercase().contains("fetch") || desc.to_lowercase().contains("url"));
    }

    #[test]
    fn test_web_fetch_schema() {
        let tool = WebFetch::new("test-key".into(), 8000);
        let schema = tool.input_schema();
        assert!(schema.is_object());
        assert!(
            schema["required"]
                .as_array()
                .unwrap()
                .contains(&json!("url"))
        );
        assert!(schema["properties"]["url"]["type"].as_str().unwrap() == "string");
    }

    #[tokio::test]
    async fn test_web_search_missing_query() {
        let tool = WebSearch::new("test-key".into(), 5);
        let result = tool
            .execute(serde_json::json!({}), ToolContext::default_for_test())
            .await;
        assert!(result.is_err());
        if let Err(AgentError::InvalidToolArgs(msg)) = result {
            assert!(msg.contains("query"));
        } else {
            panic!("expected InvalidToolArgs, got {:?}", result);
        }
    }

    #[tokio::test]
    async fn test_web_fetch_missing_url() {
        let tool = WebFetch::new("test-key".into(), 8000);
        let result = tool
            .execute(serde_json::json!({}), ToolContext::default_for_test())
            .await;
        assert!(result.is_err());
        if let Err(AgentError::InvalidToolArgs(msg)) = result {
            assert!(msg.contains("url"));
        } else {
            panic!("expected InvalidToolArgs, got {:?}", result);
        }
    }

    #[tokio::test]
    async fn test_web_fetch_invalid_url_scheme() {
        let tool = WebFetch::new("test-key".into(), 8000);
        let result = tool
            .execute(
                serde_json::json!({"url": "ftp://example.com"}),
                ToolContext::default_for_test(),
            )
            .await;
        assert!(result.is_err());
        if let Err(AgentError::WebFetch(msg)) = result {
            assert!(msg.contains("http"));
        } else {
            panic!("expected WebFetch, got {:?}", result);
        }
    }

    #[tokio::test]
    async fn test_web_fetch_rejects_url_credentials() {
        let tool = WebFetch::new("test-key".into(), 8000);
        let result = tool
            .execute(
                serde_json::json!({"url": "https://user:pass@example.com"}),
                ToolContext::default_for_test(),
            )
            .await;
        assert!(result.is_err());
        if let Err(AgentError::WebFetch(msg)) = result {
            assert!(msg.contains("credentials"));
        } else {
            panic!("expected WebFetch, got {:?}", result);
        }
    }

    #[tokio::test]
    async fn test_web_search_rejects_negative_num_results() {
        let tool = WebSearch::new("test-key".into(), 5);
        let result = tool
            .execute(
                serde_json::json!({"query": "rust", "num_results": -1}),
                ToolContext::default_for_test(),
            )
            .await;
        assert!(result.is_err());
        if let Err(AgentError::InvalidToolArgs(msg)) = result {
            assert!(msg.contains("between 1 and 10"));
        } else {
            panic!("expected InvalidToolArgs, got {:?}", result);
        }
    }
}
