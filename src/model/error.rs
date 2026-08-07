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

#[cfg(test)]
mod tests {
    use super::{ModelError, ModelErrorKind};

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
}
