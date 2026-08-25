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

    // These codes carry context/token semantics strongly enough to stand on
    // their own. Generic request/string size codes deliberately do not: for
    // example Anthropic `request_too_large` is an HTTP byte-size limit and
    // says nothing about conversation-history pressure.
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
            )
        })
    {
        return true;
    }

    [
        "prompt is too long",
        "prompt too long",
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

/// The provider-reported input size of a rejected oversized request, in
/// tokens, when the diagnostic message carries one.
///
/// This is the only authoritative measurement of how far this runtime's
/// deterministic token estimate was off for a concrete request, so it is
/// worth recovering even though it lives in an unstructured message: without
/// it, the compaction that recovers from an overflow replans against exactly
/// the estimate the provider just rejected.
///
/// Providers spell the number differently, so an explicit input/prompt
/// marker is preferred and the largest plausible token count in the message
/// is the fallback. The fallback is deliberately conservative: in every
/// known spelling it is at least the real input size (it may be the window
/// or input-plus-output), and over-reporting only makes the derived budget
/// correction stricter, never looser.
#[must_use]
pub(crate) fn reported_input_tokens(message: &str) -> Option<u64> {
    let lowered = message.to_ascii_lowercase();
    for marker in [
        "prompt contains at least ",
        "prompt contains ",
        "in the messages",
        "input token count (",
        "input length (",
        "prompt has ",
        "prompt is too long: ",
        "the request contains ",
    ] {
        if let Some(found) = number_near(&lowered, marker) {
            return Some(found);
        }
    }
    largest_number(&lowered).filter(|value| *value >= 1_000)
}

/// The number adjacent to `marker`: the first number after it, or — for a
/// trailing marker such as `in the messages` — the last number before it.
fn number_near(haystack: &str, marker: &str) -> Option<u64> {
    let at = haystack.find(marker)?;
    if marker.starts_with("in the") {
        return last_number(&haystack[..at]);
    }
    first_number(&haystack[at + marker.len()..])
}

/// Parses the first decimal number of `text`, tolerating digit grouping.
fn first_number(text: &str) -> Option<u64> {
    let mut digits = String::new();
    for character in text.chars() {
        if character.is_ascii_digit() {
            digits.push(character);
        } else if character == ',' && !digits.is_empty() {
            // A digit separator inside a number; a trailing comma simply
            // ends it, which the parse below tolerates.
        } else if digits.is_empty() {
            if character.is_alphabetic() {
                // The marker was not immediately followed by a count.
                return None;
            }
        } else {
            break;
        }
    }
    digits.parse().ok()
}

/// Parses the last decimal number of `text`.
fn last_number(text: &str) -> Option<u64> {
    let mut best = None;
    let mut digits = String::new();
    for character in text.chars() {
        if character.is_ascii_digit() {
            digits.push(character);
        } else if character != ',' && !digits.is_empty() {
            best = digits.parse().ok();
            digits.clear();
        }
    }
    if digits.is_empty() {
        best
    } else {
        digits.parse().ok().or(best)
    }
}

/// The largest decimal number appearing in `text`.
fn largest_number(text: &str) -> Option<u64> {
    let mut best: Option<u64> = None;
    let mut digits = String::new();
    let flush = |digits: &mut String, best: &mut Option<u64>| {
        if let Ok(value) = digits.parse::<u64>()
            && best.is_none_or(|current| value > current)
        {
            *best = Some(value);
        }
        digits.clear();
    };
    for character in text.chars() {
        if character.is_ascii_digit() {
            digits.push(character);
        } else if character != ',' {
            flush(&mut digits, &mut best);
        }
    }
    flush(&mut digits, &mut best);
    best
}

#[cfg(test)]
mod tests {
    use super::{ModelError, ModelErrorKind, is_context_window_error, reported_input_tokens};

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

    #[test]
    fn generic_request_size_errors_are_not_context_overflow() {
        for (message, code) in [
            (
                "Request exceeds the maximum size of 32 MB",
                Some("request_too_large"),
            ),
            (
                "String should have at most 1048576 characters",
                Some("string_too_long"),
            ),
        ] {
            assert!(!is_context_window_error(message, code), "{message}");
        }
    }

    /// The provider-reported input size is recovered from every spelling
    /// this runtime knows, and is never below the real input count.
    #[test]
    fn reported_input_tokens_recovers_the_provider_count() {
        for (message, expected) in [
            (
                "prompt is too long: 213462 tokens > 200000 maximum",
                213_462,
            ),
            (
                "This model's maximum context length is 128000 tokens. However, you requested \
                 32768 output tokens and your prompt contains at least 84033 input tokens",
                84_033,
            ),
            (
                "Input length (265330) exceeds model's maximum context length (262144)",
                265_330,
            ),
            (
                "The input token count (1196265) exceeds the maximum number of tokens allowed (1048575)",
                1_196_265,
            ),
            (
                "Prompt has 140000 tokens, but the configured context size is 131072 tokens",
                140_000,
            ),
            (
                "This model's maximum prompt length is 131072 but the request contains 537812 tokens",
                537_812,
            ),
        ] {
            assert_eq!(reported_input_tokens(message), Some(expected), "{message}");
        }
    }

    /// A message with no usable count reports nothing rather than a
    /// fabricated one.
    #[test]
    fn reported_input_tokens_declines_a_countless_message() {
        assert_eq!(reported_input_tokens("context length exceeded"), None);
        assert_eq!(reported_input_tokens("400 status code (no body)"), None);
    }

    #[test]
    fn ambiguous_size_code_requires_context_specific_message_evidence() {
        for code in ["request_too_large", "string_too_long"] {
            assert!(is_context_window_error(
                "input tokens exceed the model's maximum context length",
                Some(code),
            ));
        }
    }
}
