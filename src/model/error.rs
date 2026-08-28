//! Normalized runtime-owned model errors.

use serde::{Deserialize, Serialize};

/// Error classes the runtime distinguishes for retry/termination decisions.
/// Provider SDK error structs never cross this boundary.
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

/// The retry disposition of one normalized model failure.
///
/// Provider adapters assign it from provider-specific retry evidence. A
/// runtime owner may assign the appropriate disposition when it constructs a
/// normalized runtime failure, such as a request deadline timeout. The Agent
/// Loop always owns the retry budget, scheduling, and actual retry execution;
/// provider-supplied delay remains separate in [`ModelError::retry_after_ms`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelRetryDisposition {
    /// The failure is terminal for this request.
    Never,
    /// The failure is eligible for the bounded Agent-Loop retry budget.
    Transient,
}

/// The typed provider measurements of one rejected oversized request.
///
/// A provider states how large the request actually was, and how large it
/// was allowed to be, in its own prose. Recovering those two numbers is a
/// provider concern, so it happens exactly once — in the adapter that owns
/// the provider's error shape — and the result crosses the model boundary
/// as data. No layer above the adapter parses a provider message.
///
/// Both numbers are optional because not every provider reports either one.
/// An absent number is reported as absent; it is never guessed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextOverflowReport {
    /// The provider-counted input size of the rejected request, in tokens.
    ///
    /// This is the only authoritative measurement of how far this runtime's
    /// deterministic token estimate was off for a concrete request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reported_input_tokens: Option<u64>,
    /// The provider-stated context limit the request exceeded, in tokens.
    ///
    /// Carried for diagnostics: it explains a rejection without implying
    /// anything about this runtime's estimate, so no budget is derived from
    /// it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_limit: Option<u64>,
}

impl ContextOverflowReport {
    /// Whether the report carries no measurement at all.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.reported_input_tokens.is_none() && self.context_limit.is_none()
    }
}

/// A normalized model error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelError {
    /// The normalized error class.
    pub kind: ModelErrorKind,
    /// Human-readable diagnostic message.
    pub message: String,
    /// The retry disposition assigned by the owner that normalized this
    /// failure. Delay is intentionally a separate field so there is exactly
    /// one source of `retry_after_ms`.
    pub retry_disposition: ModelRetryDisposition,
    /// Provider-requested retry delay, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
    /// Original provider error code, kept as plain runtime-owned data for
    /// diagnostics only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_code: Option<String>,
    /// The typed measurements of a [`ModelErrorKind::ContextWindowExceeded`]
    /// rejection, when the provider reported any.
    ///
    /// Absent for every other error class, and absent for an overflow whose
    /// message carried no recognizable count. Consumers read this field;
    /// they never re-read [`Self::message`] looking for numbers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_overflow: Option<ContextOverflowReport>,
}

impl ModelError {
    /// Completes one adapter-produced error at the model boundary.
    ///
    /// A [`ModelErrorKind::ContextWindowExceeded`] error gains the typed
    /// measurements recovered from the provider's own diagnostic; every
    /// other class is returned unchanged. This is the last point at which a
    /// provider message is read for numbers — every consumer above the
    /// model layer reads [`Self::context_overflow`].
    #[must_use]
    pub(crate) fn normalized(mut self) -> Self {
        if matches!(self.kind, ModelErrorKind::ContextWindowExceeded)
            && self.context_overflow.is_none()
        {
            let report = context_overflow_report(&self.message);
            self.context_overflow = (!report.is_empty()).then_some(report);
        }
        self
    }
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

/// The typed measurements a provider-owned context-overflow message
/// carries, recovered for the adapter that owns that provider's error
/// shape.
///
/// This is deliberately the *only* place a provider diagnostic is read for
/// numbers, and it is called from adapter normalization — never from the
/// agent loop or the context engine, which see
/// [`ModelError::context_overflow`] and nothing else.
///
/// Recovery is marker-driven and nothing else. Providers spell the counts
/// differently, so each known spelling is named explicitly; a message with
/// no known marker reports no measurement. There is deliberately no
/// "largest number in the message" fallback: an unstructured diagnostic
/// routinely carries unrelated large integers — a request id, a byte size,
/// an epoch timestamp — and one of those parsed as an input-token count
/// produces a correction ratio that silently shrinks the compaction budget
/// toward nothing. An absent measurement costs one conservative
/// unquantified correction; a wrong one corrupts every budget derived from
/// it.
#[must_use]
pub(crate) fn context_overflow_report(message: &str) -> ContextOverflowReport {
    let lowered = message.to_ascii_lowercase();
    ContextOverflowReport {
        reported_input_tokens: marked_number(
            &lowered,
            &[
                "prompt contains at least ",
                "prompt contains ",
                "in the messages",
                "input token count (",
                "input length (",
                "prompt has ",
                "prompt is too long: ",
                "the request contains ",
            ],
        ),
        context_limit: marked_number(
            &lowered,
            &[
                "maximum context length is ",
                "maximum context length (",
                "maximum prompt length is ",
                "configured context size is ",
                "maximum number of tokens allowed (",
                "context window of ",
            ],
        ),
    }
}

/// The first number recoverable from any of `markers`, in order.
fn marked_number(lowered: &str, markers: &[&str]) -> Option<u64> {
    markers
        .iter()
        .find_map(|marker| number_near(lowered, marker))
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

#[cfg(test)]
mod tests {
    use super::{
        ModelError, ModelErrorKind, ModelRetryDisposition, context_overflow_report,
        is_context_window_error,
    };

    /// Model errors round-trip with stable kind discriminators.
    #[test]
    fn model_error_round_trip() {
        let error = ModelError {
            kind: ModelErrorKind::RateLimit,
            message: "requests per minute exceeded".to_owned(),
            retry_disposition: ModelRetryDisposition::Transient,
            retry_after_ms: Some(1_500),
            provider_code: Some("rate_limit_exceeded".to_owned()),
            context_overflow: None,
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
    fn context_overflow_report_recovers_the_provider_count() {
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
            assert_eq!(
                context_overflow_report(message).reported_input_tokens,
                Some(expected),
                "{message}"
            );
        }
    }

    /// The stated limit is recovered alongside the input count, and stays a
    /// separate number: it is never mistaken for what the request measured.
    #[test]
    fn context_overflow_report_separates_the_stated_limit() {
        let report = context_overflow_report(
            "Input length (265330) exceeds model's maximum context length (262144)",
        );
        assert_eq!(report.reported_input_tokens, Some(265_330));
        assert_eq!(report.context_limit, Some(262_144));
    }

    /// A message with no usable count reports nothing rather than a
    /// fabricated one.
    #[test]
    fn context_overflow_report_declines_a_countless_message() {
        assert!(context_overflow_report("context length exceeded").is_empty());
        assert_eq!(
            context_overflow_report("400 status code (no body)").reported_input_tokens,
            None
        );
    }

    /// An unrelated large integer in a provider diagnostic is never read as
    /// an input-token count. The removed "largest number wins" fallback
    /// turned a request id into a measurement, and the correction derived
    /// from that ratio collapsed the compaction budget.
    #[test]
    fn unrelated_large_numbers_are_never_read_as_a_token_count() {
        for message in [
            "context window exceeded (request_id=999999999)",
            "maximum context length is 128000 tokens; trace 20260826123000",
            "context length exceeded after 4294967295 bytes were buffered",
        ] {
            assert_eq!(
                context_overflow_report(message).reported_input_tokens,
                None,
                "{message}"
            );
        }
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
