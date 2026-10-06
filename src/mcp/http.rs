//! Keep HTTP failures safe before they reach the SDK's own tracing, and bound
//! ordinary requests without imposing an idle timeout on the long-lived GET stream.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use tracing::instrument::WithSubscriber;

use reqwest::header::{HeaderName, HeaderValue};
use rmcp::model::{ClientJsonRpcMessage, JsonRpcMessage, ServerJsonRpcMessage};
use rmcp::transport::common::client_side_sse::BoxedSseResponse;
use rmcp::transport::streamable_http_client::{
    SseError, StreamableHttpClient, StreamableHttpError, StreamableHttpPostResponse,
};

use crate::error::AgentError;

const DEFAULT_MAX_SSE_EVENT_SIZE: usize = 16 * 1024 * 1024;
const MAX_ERROR_CHARS: usize = 512;

#[derive(Clone)]
pub(super) struct McpHttpClient {
    request: reqwest::Client,
    stream: reqwest::Client,
    connect_timeout: Duration,
    token: Option<String>,
}

impl McpHttpClient {
    pub fn new(
        connect_timeout: Duration,
        call_timeout: Duration,
        token: Option<String>,
    ) -> Result<Self, AgentError> {
        let builder = || {
            reqwest::Client::builder()
                .pool_max_idle_per_host(0)
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(connect_timeout)
        };
        let request = builder()
            .timeout(connect_timeout.max(call_timeout))
            .build()
            .map_err(|_| AgentError::Mcp("could not construct HTTP request client".into()))?;
        let stream = builder()
            .build()
            .map_err(|_| AgentError::Mcp("could not construct HTTP stream client".into()))?;
        Ok(Self {
            request,
            stream,
            connect_timeout,
            token,
        })
    }

    fn safe_error(
        &self,
        error: StreamableHttpError<reqwest::Error>,
    ) -> StreamableHttpError<reqwest::Error> {
        let detail = match error {
            // Preserve the SDK's control-flow markers, but never its raw HTTP error body.
            StreamableHttpError::SessionExpired => return StreamableHttpError::SessionExpired,
            StreamableHttpError::ServerDoesNotSupportSse => {
                return StreamableHttpError::ServerDoesNotSupportSse;
            }
            StreamableHttpError::ServerDoesNotSupportDeleteSession => {
                return StreamableHttpError::ServerDoesNotSupportDeleteSession;
            }
            StreamableHttpError::UnexpectedServerResponse(body) => {
                let status = body
                    .strip_prefix("HTTP ")
                    .and_then(|body| body.get(..3))
                    .filter(|status| status.bytes().all(|byte| byte.is_ascii_digit()));
                status
                    .map(|status| format!("HTTP {status} response (body omitted)"))
                    .unwrap_or_else(|| "unexpected HTTP response (body omitted)".into())
            }
            StreamableHttpError::Client(error) => {
                if let Some(status) = error.status() {
                    format!("HTTP {status} response")
                } else if error.is_timeout() {
                    "HTTP request timed out".into()
                } else {
                    "HTTP request failed".into()
                }
            }
            error => sanitize_error(&error.to_string(), self.token.as_deref()),
        };
        StreamableHttpError::UnexpectedServerResponse(detail.into())
    }
}

impl StreamableHttpClient for McpHttpClient {
    type Error = reqwest::Error;

    async fn post_message(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
        self.post_message_with_max_sse_event_size(
            uri,
            message,
            session_id,
            auth_header,
            custom_headers,
            DEFAULT_MAX_SSE_EVENT_SIZE,
        )
        .await
    }

    async fn post_message_with_max_sse_event_size(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
        max_sse_event_size: usize,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
        self.request
            .post_message_with_max_sse_event_size(
                uri,
                message,
                session_id,
                auth_header,
                custom_headers,
                max_sse_event_size,
            )
            // The SDK helpers can log raw headers and parse errors before returning.
            // Only the adapter's sanitized error is allowed across this boundary.
            .with_subscriber(tracing::subscriber::NoSubscriber::default())
            .await
            .map(|response| match response {
                StreamableHttpPostResponse::Sse(stream, session) => {
                    StreamableHttpPostResponse::Sse(
                        safe_stream(stream, self.token.clone()),
                        session,
                    )
                }
                StreamableHttpPostResponse::Json(mut message, session) => {
                    sanitize_rpc_error(&mut message, self.token.as_deref());
                    StreamableHttpPostResponse::Json(message, session)
                }
                response => response,
            })
            .map_err(|error| self.safe_error(error))
    }

    async fn delete_session(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<(), StreamableHttpError<Self::Error>> {
        self.request
            .delete_session(uri, session_id, auth_header, custom_headers)
            .with_subscriber(tracing::subscriber::NoSubscriber::default())
            .await
            .map_err(|error| self.safe_error(error))
    }

    async fn get_stream(
        &self,
        uri: Arc<str>,
        session_id: Option<Arc<str>>,
        last_event_id: Option<String>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<BoxedSseResponse, StreamableHttpError<Self::Error>> {
        self.get_stream_with_max_sse_event_size(
            uri,
            session_id,
            last_event_id,
            auth_header,
            custom_headers,
            DEFAULT_MAX_SSE_EVENT_SIZE,
        )
        .await
    }

    async fn get_stream_with_max_sse_event_size(
        &self,
        uri: Arc<str>,
        session_id: Option<Arc<str>>,
        last_event_id: Option<String>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
        max_sse_event_size: usize,
    ) -> Result<BoxedSseResponse, StreamableHttpError<Self::Error>> {
        match tokio::time::timeout(
            self.connect_timeout,
            self.stream
                .get_stream_with_max_sse_event_size(
                    uri,
                    session_id,
                    last_event_id,
                    auth_header,
                    custom_headers,
                    max_sse_event_size,
                )
                .with_subscriber(tracing::subscriber::NoSubscriber::default()),
        )
        .await
        {
            Ok(result) => result
                .map(|stream| safe_stream(stream, self.token.clone()))
                .map_err(|error| self.safe_error(error)),
            Err(_) => Err(StreamableHttpError::UnexpectedServerResponse(
                "HTTP stream connection timed out".into(),
            )),
        }
    }
}

fn sanitize_rpc_error(message: &mut ServerJsonRpcMessage, token: Option<&str>) {
    if let JsonRpcMessage::Error(message) = message {
        message.error.message = sanitize_error(&message.error.message, token).into();
        // Diagnostic data can include request headers or a whole upstream response. It is
        // neither required to correlate the error nor used by NerdBot's error interface.
        message.error.data = None;
    }
}

fn safe_stream(stream: BoxedSseResponse, token: Option<String>) -> BoxedSseResponse {
    stream
        .map(move |result| {
            let mut event = result.map_err(|_| {
                SseError::Body(Box::new(std::io::Error::other(
                    "MCP HTTP response stream failed",
                )))
            })?;
            if let Some(data) = &event.data
                && data.contains("\"error\"")
                && let Ok(mut message @ JsonRpcMessage::Error(_)) =
                    serde_json::from_str::<ServerJsonRpcMessage>(data)
            {
                sanitize_rpc_error(&mut message, token.as_deref());
                event.data =
                    Some(serde_json::to_string(&message).expect("JSON-RPC error is serializable"));
            }
            Ok(event)
        })
        .boxed()
}

pub(super) fn sanitize_error(value: &str, token: Option<&str>) -> String {
    let redacted = match token.filter(|token| !token.is_empty()) {
        Some(token) => value.replace(token, "[redacted]"),
        None => value.to_owned(),
    };
    redacted
        .chars()
        .filter(|character| !character.is_control())
        .take(MAX_ERROR_CHARS)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_redact_before_truncation_and_remove_control_characters() {
        let error = sanitize_error(
            &format!("token=secret\n{}", "x".repeat(10000)),
            Some("secret"),
        );
        assert!(error.starts_with("token=[redacted]"));
        assert!(!error.contains("secret"));
        assert!(!error.contains('\n'));
        assert_eq!(error.chars().count(), MAX_ERROR_CHARS);
    }
}
