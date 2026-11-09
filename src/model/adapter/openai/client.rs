//! `OpenAI` request transport. An encoded request body is moved into one HTTP
//! attempt. Response bytes stay with the polling stream and are bounded before
//! parsing where the invocation declares a budget.

use async_openai::config::{Config, OpenAIConfig};
use async_openai::error::{OpenAIError, StreamError};
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use reqwest::{Response, StatusCode};

#[derive(Clone)]
pub(crate) struct OpenAiTransport {
    client: reqwest::Client,
    config: OpenAIConfig,
}

impl OpenAiTransport {
    pub(crate) fn new(api_key: &str, api_base: &str, client: Option<reqwest::Client>) -> Self {
        Self {
            client: client.unwrap_or_else(super::super::transport::default_client),
            config: OpenAIConfig::new()
                .with_api_key(api_key)
                .with_api_base(api_base),
        }
    }

    async fn send(&self, path: &str, body: bytes::Bytes) -> Result<Response, OpenAIError> {
        self.client
            .post(self.config.url(path))
            .headers(self.config.headers())
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body)
            .send()
            .await
            .map_err(|error| OpenAIError::Boxed(Box::new(error)))
    }

    pub(crate) async fn stream(
        &self,
        path: &str,
        body: bytes::Bytes,
        response_byte_limit: Option<usize>,
    ) -> Result<async_openai::types::stream::StreamResponse<serde_json::Value>, OpenAIError> {
        let response = self.send(path, body).await?;
        let response = if let Some(limit) = response_byte_limit {
            super::super::response_budget::bounded(response, limit)
        } else {
            response
        };
        if !response.status().is_success() {
            return Err(OpenAIError::Boxed(Box::new(
                HttpFailure::from_response(response).await,
            )));
        }
        let events = Box::pin(response.bytes_stream().eventsource());
        Ok(Box::pin(futures_util::stream::unfold(
            events,
            |mut events| async move {
                loop {
                    let item = match events.next().await? {
                        Ok(event) if event.data == "[DONE]" => return None,
                        Ok(event) if event.event == "keepalive" => continue,
                        Ok(event) => serde_json::from_str(&event.data)
                            .map_err(|error| OpenAIError::JSONDeserialize(error, event.data)),
                        Err(error) => Err(OpenAIError::StreamError(Box::new(
                            StreamError::EventStream(error.to_string()),
                        ))),
                    };
                    return Some((item, events));
                }
            },
        )))
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
    pub(crate) provider_type: Option<String>,
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
        let (message, provider_code, provider_type) = parse_error_body(&body, status);
        Self {
            status,
            retry_after_ms,
            message,
            provider_code,
            provider_type,
        }
    }
}

fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    crate::model::adapter::retry::retry_after(headers)
}

fn parse_error_body(body: &[u8], status: StatusCode) -> (String, Option<String>, Option<String>) {
    let Ok(root) = serde_json::from_slice::<serde_json::Value>(body) else {
        return (String::from_utf8_lossy(body).into_owned(), None, None);
    };
    // OpenAI uses `{ "error": { ... } }`, while several compatible
    // providers return the same detail object at the top level.
    let detail = root
        .get("error")
        .filter(|error| error.is_object())
        .unwrap_or(&root);
    let message = detail
        .get("message")
        .and_then(serde_json::Value::as_str)
        .map_or_else(
            || {
                let raw = String::from_utf8_lossy(body).into_owned();
                if raw.is_empty() {
                    format!("OpenAI HTTP {status}")
                } else {
                    raw
                }
            },
            str::to_owned,
        );
    let provider_type = detail
        .get("type")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    let provider_code = detail
        .get("code")
        .and_then(provider_code_value)
        .or_else(|| provider_type.clone());
    (message, provider_code, provider_type)
}

fn provider_code_value(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(value) => Some(value.clone()),
        serde_json::Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
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
    use super::{HttpFailure, parse_error_body, parse_retry_after};
    use crate::model::error::{ModelErrorKind, ModelRetryDisposition};
    use reqwest::StatusCode;

    #[tokio::test]
    async fn default_transport_reuses_connection_without_sharing_binding_headers() {
        use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let server = tokio::spawn(async move {
                let (stream, _) = listener.accept().await.unwrap();
                let mut reader = BufReader::new(stream);
                let mut requests = Vec::new();
                // Both bindings must use this one persistent connection.
                for _ in 0..2 {
                    let mut headers = Vec::new();
                    loop {
                        let mut line = String::new();
                        assert!(reader.read_line(&mut line).await.unwrap() > 0);
                        if line == "\r\n" { break; }
                        headers.push(line);
                    }
                    let length = headers.iter().find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().unwrap())
                    }).unwrap_or(0);
                    reader.read_exact(&mut vec![0; length]).await.unwrap();
                    requests.push(headers);
                    reader.get_mut().write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}").await.unwrap();
                }
                requests
            });
            for (path, key) in [("left", "fixture-left"), ("right", "fixture-right")] {
                let client = super::OpenAiTransport::new(key, &format!("{base}/{path}"), None);
                assert_eq!(client.send("/chat/completions", bytes::Bytes::from_static(b"{}")).await.unwrap().bytes().await.unwrap().as_ref(), b"{}");
            }
            let requests = server.await.unwrap();
            for (headers, (path, key)) in requests.iter().zip([("left", "fixture-left"), ("right", "fixture-right")]) {
                assert!(headers[0].starts_with(&format!("POST /{path}/chat/completions ")));
                let authorization = headers.iter().find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("authorization").then(|| value.trim())
                });
                assert_eq!(authorization, Some(format!("Bearer {key}").as_str()));
            }
        }).await.expect("two bindings must reuse the credential-free HTTP pool");
    }

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

    #[test]
    fn top_level_compatible_error_body_is_preserved() {
        let body = br#"{"message":"This model's maximum context length is 116800 tokens","type":"BadRequestError","param":"input_tokens","code":400}"#;
        let (message, provider_code, provider_type) =
            parse_error_body(body, StatusCode::BAD_REQUEST);
        assert_eq!(
            message,
            "This model's maximum context length is 116800 tokens"
        );
        assert_eq!(provider_code.as_deref(), Some("400"));
        assert_eq!(provider_type.as_deref(), Some("BadRequestError"));
    }

    #[test]
    fn specific_context_code_survives_a_broad_permanent_http_type() {
        for message in ["request rejected", "请求被拒绝"] {
            let body = serde_json::to_vec(&serde_json::json!({"error": {
                "type": "invalid_request_error", "code": "context_length_exceeded", "message": message
            }})).unwrap();
            let (message, provider_code, provider_type) =
                parse_error_body(&body, StatusCode::BAD_REQUEST);
            let error = super::super::mapping::normalize_error(
                async_openai::error::OpenAIError::Boxed(Box::new(HttpFailure {
                    status: StatusCode::BAD_REQUEST,
                    retry_after_ms: None,
                    message,
                    provider_code,
                    provider_type,
                })),
            );
            assert_eq!(
                error.provider_code.as_deref(),
                Some("context_length_exceeded")
            );
            assert_eq!(error.kind, ModelErrorKind::ContextWindowExceeded);
            assert_eq!(error.retry_disposition, ModelRetryDisposition::Never);
        }
    }
}
