//! Web search backend trait.
//!
//! Implementations come in Phase 8.

use serde::{Deserialize, Serialize};

use crate::error::WebSearchError;

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
