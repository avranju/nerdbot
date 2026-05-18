//! Web page fetcher with SSRF protections.
//!
//! Implementations come in Phase 8.

use crate::error::WebFetchError;

/// Fetch a URL and extract readable content.
///
/// Phase 1 stub — full implementation with SSRF protections in Phase 8.
pub async fn fetch_url(_url: &str) -> Result<FetchedPage, WebFetchError> {
    Err(WebFetchError::Http(
        "Web fetcher not yet implemented".into(),
    ))
}

/// Result of a URL fetch.
#[derive(Debug)]
pub struct FetchedPage {
    pub final_url: String,
    pub title: Option<String>,
    pub text: String,
    pub truncated: bool,
}
