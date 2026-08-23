//! Normalized runtime-owned model errors.

use serde::{Deserialize, Serialize};

/// Error classes the runtime distinguishes for retry/termination decisions.
///
/// Retry logic itself is a later milestone; M1 defines the typed classes and
/// the normalized diagnostic data retry code will need. Provider SDK error
/// structs never cross this boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelErrorKind {
    /// The request itself was invalid.
    InvalidRequest,
    /// Authentication or authorization failed.
    Authentication,
    /// The provider is rate limiting requests.
    RateLimit,
    /// The request timed out.
    Timeout,
    /// Transport-level failure.
    Transport,
    /// Provider/server failure.
    ProviderError,
    /// The context exceeds the provider window.
    ContextWindowExceeded,
    /// The request was cancelled.
    Cancelled,
    /// The requested capability or protocol is unsupported.
    Unsupported,
}

/// A normalized model error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelError {
    /// The normalized error class.
    pub kind: ModelErrorKind,
    /// Human-readable diagnostic message.
    pub message: String,
    /// Provider-requested retry delay, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
    /// Original provider error code, kept as plain runtime-owned data for
    /// diagnostics only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_code: Option<String>,
}

/// Whether provider-owned error data describes an exhausted context window.
///
/// Compatible providers do not share one error schema: the same condition
/// appears as a typed code, a human-readable message, or both. Keeping the
/// provider-neutral vocabulary here gives every adapter path (HTTP and
/// streaming) the same classification before the agent loop applies its
/// bounded compact-and-retry policy.
#[must_use]
pub(crate) fn is_context_window_error(message: &str, provider_code: Option<&str>) -> bool {
    let message = message.to_ascii_lowercase();

    // Some throttling responses use phrases such as "too many tokens" for a
    // rate limit. Those must remain retryable provider failures, not trigger
    // destructive conversation compaction.
    if [
        "rate limit",
        "too many requests",
        "throttling",
        "service unavailable",
    ]
    .iter()
    .any(|pattern| message.contains(pattern))
    {
        return false;
    }

    if provider_code
        .map(str::to_ascii_lowercase)
        .is_some_and(|code| {
            matches!(
                code.as_str(),
                "context_length_exceeded"
                    | "model_context_window_exceeded"
                    | "prompt_too_long"
                    | "input_too_long"
                    | "max_tokens_exceeded"
                    | "token_limit_exceeded"
                    | "string_too_long"
                    | "request_too_large"
            )
        })
    {
        return true;
    }

    [
        "prompt is too long",
        "prompt too long",
        "request_too_large",
        "request exceeds the maximum size",
        "input is too long for requested model",
        "exceeds the context window",
        "maximum context length",
        "context length exceeded",
        "context_length_exceeded",
        "context window",
        "maximum prompt length",
        "reduce the length of the messages",
        "maximum allowed input length",
        "longer than the model's context length",
        "exceeds the available context size",
        "greater than the context length",
        "exceeded model token limit",
        "configured context size",
        "model_context_window_exceeded",
        "range of input length should be",
        "token limit exceeded",
        "too many tokens",
    ]
    .iter()
    .any(|pattern| message.contains(pattern))
        || (message.contains("input token count") && message.contains("exceeds the maximum"))
}

#[cfg(test)]
mod tests {
    use super::{ModelError, ModelErrorKind, is_context_window_error};

    /// Model errors round-trip with stable kind discriminators.
    #[test]
    fn model_error_round_trip() {
        let error = ModelError {
            kind: ModelErrorKind::RateLimit,
            message: "requests per minute exceeded".to_owned(),
            retry_after_ms: Some(1_500),
            provider_code: Some("rate_limit_exceeded".to_owned()),
        };
        let json = serde_json::to_string(&error).expect("serialize error");
        assert!(json.contains("\"rate_limit\""));
        let decoded: ModelError = serde_json::from_str(&json).expect("deserialize error");
        assert_eq!(decoded, error);
    }

    /// Every error kind serializes to a stable string, not a Rust debug name.
    #[test]
    fn error_kind_discriminators_are_stable() {
        let cases = [
            (ModelErrorKind::InvalidRequest, "invalid_request"),
            (ModelErrorKind::Authentication, "authentication"),
            (ModelErrorKind::RateLimit, "rate_limit"),
            (ModelErrorKind::Timeout, "timeout"),
            (ModelErrorKind::Transport, "transport"),
            (ModelErrorKind::ProviderError, "provider_error"),
            (
                ModelErrorKind::ContextWindowExceeded,
                "context_window_exceeded",
            ),
            (ModelErrorKind::Cancelled, "cancelled"),
            (ModelErrorKind::Unsupported, "unsupported"),
        ];
        for (kind, expected) in cases {
            let value = serde_json::to_value(kind).expect("serialize kind");
            assert_eq!(value, expected);
        }
    }

    #[test]
    fn compatible_provider_context_errors_are_detected() {
        for (message, code) in [
            (
                "This model's maximum context length is 116800 tokens. However, you requested \
                 32768 output tokens and your prompt contains at least 84033 input tokens",
                Some("400"),
            ),
            (
                "Input length (265330) exceeds model's maximum context length (262144)",
                Some("BadRequestError"),
            ),
            (
                "Prompt has 140000 tokens, but the configured context size is 131072 tokens",
                None,
            ),
            (
                "provider rejected the request",
                Some("context_length_exceeded"),
            ),
        ] {
            assert!(is_context_window_error(message, code), "{message}");
        }
    }

    #[test]
    fn throttling_is_not_misclassified_as_context_overflow() {
        for message in [
            "rate limit exceeded: too many tokens per minute",
            "ThrottlingException: Too many tokens, please wait before trying again",
            "Service unavailable: context window workers are saturated",
        ] {
            assert!(!is_context_window_error(message, None), "{message}");
        }
    }
}
