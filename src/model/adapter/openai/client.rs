//! `OpenAI` HTTP transport with automatic retry bypassed.
//!
//! `async-openai`'s default executor wraps the plain `ReqwestService` in
//! `OpenAIRetryLayer`, which retries 429/5xx/connection failures with
//! exponential backoff. Runtime policy owns retries in rustX, so the adapters
//! never use that default executor. This module installs a custom
//! [`NoRetryService`] that executes exactly one `reqwest` request per call,
//! performs no retry, and normalizes HTTP failures (including `Retry-After`)
//! into a runtime-owned [`HttpFailure`] value before any SDK error mapping.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use async_openai::Client;
use async_openai::config::OpenAIConfig;
use async_openai::error::OpenAIError;
use async_openai::middleware::HttpRequestFactory;
use reqwest::{Response, StatusCode};

/// A tower service that executes one plain `reqwest` request.
///
/// Deliberately contains no retry middleware: a single `call` performs a
/// single HTTP attempt. `ReqwestService` from the SDK is intentionally not
/// reused here so the no-retry transport (including error normalization and
/// `Retry-After` extraction) is fully rustX-owned and visible in this file.
#[derive(Clone)]
pub(crate) struct NoRetryService {
    client: reqwest::Client,
}

impl NoRetryService {
    pub(crate) fn new(client: reqwest::Client) -> Self {
        Self { client }
    }
}

impl tower::Service<HttpRequestFactory> for NoRetryService {
    type Response = Response;
    type Error = tower::BoxError;
    type Future = Pin<Box<dyn Future<Output = Result<Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: HttpRequestFactory) -> Self::Future {
        let client = self.client.clone();
        Box::pin(async move {
            let request = request.build().await.map_err(tower::BoxError::from)?;
            let response = client
                .execute(request)
                .await
                .map_err(tower::BoxError::from)?;
            if !response.status().is_success() {
                return Err(tower::BoxError::from(
                    HttpFailure::from_response(response).await,
                ));
            }
            Ok(response)
        })
    }
}

/// A normalized `OpenAI` HTTP failure captured at the transport boundary.
///
/// This is the only carrier of the provider HTTP status, `Retry-After`
/// header, and provider error payload; the SDK's own `ApiErrorResponse`
/// drops response headers, so the transport must capture them here.
#[derive(Debug, Clone)]
pub(crate) struct HttpFailure {
    pub(crate) status: StatusCode,
    pub(crate) retry_after_ms: Option<u64>,
    pub(crate) message: String,
    pub(crate) provider_code: Option<String>,
}

impl std::fmt::Display for HttpFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "OpenAI HTTP {}: {}",
            self.status,
            if self.message.is_empty() {
                "provider error"
            } else {
                &self.message
            }
        )
    }
}

impl std::error::Error for HttpFailure {}

impl HttpFailure {
    async fn from_response(response: Response) -> Self {
        let status = response.status();
        let retry_after_ms = parse_retry_after(response.headers());
        let body = response
            .bytes()
            .await
            .map(|b| b.to_vec())
            .unwrap_or_default();
        let (message, provider_code) = parse_error_body(&body, status);
        Self {
            status,
            retry_after_ms,
            message,
            provider_code,
        }
    }
}

/// Parses the `Retry-After` header as whole seconds, matching `OpenAI`'s
/// documented response format.
fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    headers
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .parse::<u64>()
        .ok()
        .map(|seconds| seconds.saturating_mul(1000))
}

#[derive(serde::Deserialize)]
struct ErrorBody {
    #[serde(default)]
    error: Option<ErrorDetail>,
}

#[derive(serde::Deserialize)]
struct ErrorDetail {
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    r#type: Option<String>,
}

fn parse_error_body(body: &[u8], status: StatusCode) -> (String, Option<String>) {
    match serde_json::from_slice::<ErrorBody>(body) {
        Ok(parsed) => match parsed.error {
            Some(detail) => (
                detail
                    .message
                    .unwrap_or_else(|| format!("OpenAI HTTP {status}")),
                detail.code.or(detail.r#type),
            ),
            None => (String::from_utf8_lossy(body).into_owned(), None),
        },
        Err(_) => (String::from_utf8_lossy(body).into_owned(), None),
    }
}

/// Builds an SDK client whose transport is the rustX-owned no-retry service.
pub(crate) fn build_client(
    api_key: &str,
    api_base: &str,
    http_client: Option<reqwest::Client>,
) -> Client<OpenAIConfig> {
    let service = NoRetryService::new(http_client.unwrap_or_default());
    let openai_config = OpenAIConfig::new()
        .with_api_key(api_key.to_owned())
        .with_api_base(api_base.to_owned());
    Client::with_config(openai_config).with_http_service(service)
}

/// Recovers a captured [`HttpFailure`] from an SDK error.
pub(crate) fn http_failure_of(error: &OpenAIError) -> Option<&HttpFailure> {
    match error {
        OpenAIError::Boxed(boxed) => boxed.downcast_ref::<HttpFailure>(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::parse_retry_after;

    /// `Retry-After` seconds convert to milliseconds.
    #[test]
    fn retry_after_seconds_convert_to_milliseconds() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(reqwest::header::RETRY_AFTER, "3".parse().unwrap());
        assert_eq!(parse_retry_after(&headers), Some(3000));
    }

    /// A missing or malformed `Retry-After` yields no retry delay.
    #[test]
    fn retry_after_absence_is_none() {
        assert_eq!(parse_retry_after(&reqwest::header::HeaderMap::new()), None);
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(reqwest::header::RETRY_AFTER, "soon".parse().unwrap());
        assert_eq!(parse_retry_after(&headers), None);
    }
}
