//! Web page fetcher with SSRF protections and Exa contents API support.
//!
/// Exa-powered page fetcher.
///
/// Uses the Exa REST API at `https://api.exa.ai/contents` for fetching
/// full page content with SSRF protections.
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::error::WebFetchError;

// ── Exa API types ─────────────────────────────────────────────────────────

/// Response from the Exa contents API.
#[derive(Debug, Deserialize)]
struct ExaContentsResponse {
    results: Vec<ExaContentsResult>,
}

#[derive(Debug, Deserialize)]
struct ExaContentsResult {
    url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<String>,
}

// ── Public types ──────────────────────────────────────────────────────────

/// Result of a URL fetch.
#[derive(Debug)]
pub struct FetchedPage {
    pub final_url: String,
    pub title: Option<String>,
    pub text: String,
    pub truncated: bool,
}

// ── SSRF protections ─────────────────────────────────────────────────────

/// Check if an IPv6 address is in a private range.
fn ipv6_is_private(addr: &std::net::Ipv6Addr) -> bool {
    // Unique local addresses (FC00::/7)
    if (addr.segments()[0] & 0xFE00) == 0xFC00 {
        return true;
    }
    // IPv4-mapped addresses (::FFFF:0:0/96) — covers IPv4-compatible too
    if addr.segments()[0] == 0
        && addr.segments()[1] == 0
        && addr.segments()[2] == 0
        && addr.segments()[3] == 0
        && addr.segments()[4] == 0
        && addr.segments()[5] == 0xFFFF
    {
        return true;
    }
    false
}

/// Check if a URL resolves to a private, local, or otherwise unsafe address.
fn is_private_url(url: &url::Url) -> bool {
    // Reject non-http(s) schemes
    if !matches!(url.scheme(), "http" | "https") {
        return true;
    }

    // Check host type
    if let Some(host) = url.host() {
        match host {
            url::Host::Domain(domain) => {
                let lower = domain.to_lowercase();
                // Reject localhost variants and internal domains
                matches!(
                    lower.as_str(),
                    "localhost"
                        | "localhost.localdomain"
                        | "0.0.0.0"
                        | "127.0.0.1"
                        | "::1"
                        | "ip6-localhost"
                        | "ip6-loopback"
                ) || lower.ends_with(".local")
                    || lower.ends_with(".internal")
                    || lower.ends_with(".localdomain")
            }
            url::Host::Ipv4(addr) => {
                addr.is_loopback()
                    || addr.is_private()
                    || addr.is_link_local()
                    || addr.is_broadcast()
                    || addr == std::net::Ipv4Addr::UNSPECIFIED
            }
            url::Host::Ipv6(addr) => {
                addr.is_loopback()
                    || ipv6_is_private(&addr)
                    || addr.is_unicast_link_local()
                    || addr.is_unspecified()
            }
        }
    } else {
        true // No host = bad
    }
}

// ── Public API ────────────────────────────────────────────────────────────

/// Exa-powered page fetcher.
pub struct ExaFetcher {
    client: reqwest::Client,
    pub(crate) api_key: String,
    pub(crate) max_text_chars: usize,
}

impl ExaFetcher {
    /// Create a new Exa fetcher.
    pub fn new(api_key: String, max_text_chars: usize) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("Failed to build HTTP client");

        Self {
            client,
            api_key,
            max_text_chars,
        }
    }

    /// Validate that a URL is safe to fetch (SSRF protection).
    fn validate_url(&self, url: &str) -> Result<url::Url, WebFetchError> {
        let parsed = url::Url::parse(url).map_err(|_| WebFetchError::InvalidUrl(url.into()))?;

        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(WebFetchError::InvalidUrl(
                "URL must start with http:// or https://".into(),
            ));
        }

        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err(WebFetchError::InvalidUrl(
                "URL contains credentials which are not allowed".into(),
            ));
        }

        if is_private_url(&parsed) {
            return Err(WebFetchError::SsrfBlocked);
        }

        Ok(parsed)
    }
}

impl ExaFetcher {
    /// Fetch a single URL and return its content.
    pub async fn fetch(&self, url: &str) -> Result<FetchedPage, WebFetchError> {
        let parsed = self.validate_url(url)?;
        let url_str = parsed.to_string();

        debug!(url = url_str, "fetching page via Exa contents API");

        let body = serde_json::json!({
            "urls": [url_str.clone()],
            "text": {
                "maxCharacters": self.max_text_chars
            }
        });

        let resp = self
            .client
            .post("https://api.exa.ai/contents")
            .header("x-api-key", &self.api_key)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| WebFetchError::Http(format!("Request failed: {e}")))?;

        let status = resp.status();

        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(WebFetchError::RateLimited);
        }

        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_else(|_| "no body".to_string());
            return Err(WebFetchError::Http(format!(
                "API error {status}: {body_text}"
            )));
        }

        let api_resp: ExaContentsResponse = resp
            .json()
            .await
            .map_err(|e| WebFetchError::Http(format!("Failed to parse response: {e}")))?;

        let result = api_resp
            .results
            .into_iter()
            .next()
            .ok_or_else(|| WebFetchError::Http("No results returned".into()))?;

        let text = result.text.unwrap_or_default();
        let truncated = text.len() > self.max_text_chars;

        Ok(FetchedPage {
            final_url: url_str,
            title: result.title,
            text,
            truncated,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ssrf_blocks_localhost() {
        let url = url::Url::parse("http://localhost:8080/api").unwrap();
        assert!(is_private_url(&url));
    }

    #[test]
    fn test_ssrf_blocks_127_0_0_1() {
        let url = url::Url::parse("http://127.0.0.1:3000").unwrap();
        assert!(is_private_url(&url));
    }

    #[test]
    fn test_ssrf_blocks_private_ranges() {
        for addr in [
            "http://10.0.0.1/path",
            "http://192.168.1.1/",
            "http://172.16.0.1:9090",
        ] {
            let url = url::Url::parse(addr).unwrap();
            assert!(is_private_url(&url), "should block {addr}");
        }
    }

    #[test]
    fn test_ssrf_allows_public_urls() {
        for addr in [
            "https://example.com/article",
            "http://github.com/user/repo",
            "https://docs.rust-lang.org/book/",
        ] {
            let url = url::Url::parse(addr).unwrap();
            assert!(!is_private_url(&url), "should allow {addr}");
        }
    }

    #[test]
    fn test_ssrf_blocks_localdomain() {
        let url = url::Url::parse("http://myhost.local").unwrap();
        assert!(is_private_url(&url));

        let url = url::Url::parse("http://server.internal").unwrap();
        assert!(is_private_url(&url));
    }

    #[test]
    fn test_ssrf_blocks_non_http() {
        let url = url::Url::parse("file:///etc/passwd").unwrap();
        assert!(is_private_url(&url));

        let url = url::Url::parse("ftp://example.com/file").unwrap();
        assert!(is_private_url(&url));
    }
}
