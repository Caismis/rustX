//! Normalized runtime-owned model errors.

use serde::{Deserialize, Serialize};

use crate::model::generation::GenerationFailure;

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
    /// The model generated tool intent that cannot become a trustworthy
    /// canonical [`ToolCall`](crate::tools::types::ToolCall).
    ///
    /// This is a *generation* defect, not a transport defect and not a Tool
    /// schema rejection: the proposal never crossed `ToolCall` acceptance, so
    /// nothing executed, nothing was settled, and nothing entered canonical
    /// history. The Agent Loop owns the bounded corrective regeneration this
    /// class authorizes; see [`MalformedToolProposalSource`] for the
    /// provider-independent provenance carried alongside it.
    MalformedToolProposal,
    /// One generated channel deterministically degenerated into repetition.
    ///
    /// Like [`Self::MalformedToolProposal`] this is a *generation* defect:
    /// the physical generation is discarded before the canonical commit
    /// boundary, and it shares the one semantic corrective-generation budget
    /// of the logical model step. The typed evidence is carried in
    /// [`ModelError::generation`].
    GenerationDegenerated,
    /// One generation budget was exhausted before the generation completed.
    ///
    /// Either the provider terminated at its own token limit, or the
    /// runtime's byte safeguard fired. Both mean the generation is
    /// incomplete, so it is never accepted as an ordinary successful
    /// assistant completion. The typed detail is carried in
    /// [`ModelError::generation`].
    GenerationBudgetExceeded,
}

/// Why a model-emitted tool proposal was refused at `ToolCall` acceptance.
///
/// The variants are the broad provider-independent classes a reader needs to
/// debug a regeneration. They deliberately carry no provider payload: the
/// adapter that owned the provider protocol has already translated its own
/// evidence into one of these, and [`ModelError::message`] carries the
/// bounded human-readable detail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MalformedToolProposalSource {
    /// The provider itself declared the function/tool call malformed.
    ProviderDeclared,
    /// The assembled proposal is structurally unusable: a broken tool
    /// envelope, an unusable tool identity, or an argument representation
    /// that is not one complete JSON value.
    AdapterStructural,
    /// The provider stream ended without ever delivering the parts one
    /// structurally valid invocation requires, such as a correlation
    /// identity or a function name.
    StreamAssembly,
    /// Reserved provider tool-protocol markup leaked into ordinary
    /// reasoning/content while the generation produced no structured call.
    ReservedProtocolLeak,
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
    /// The provider-independent provenance of a
    /// [`ModelErrorKind::MalformedToolProposal`] rejection.
    ///
    /// Present for exactly that class and absent for every other one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub malformed_tool_proposal: Option<MalformedToolProposalSource>,
    /// The typed provider-independent detail of one single-generation safety
    /// failure: degeneration, budget exhaustion, or a request deadline.
    ///
    /// Present for [`ModelErrorKind::GenerationDegenerated`],
    /// [`ModelErrorKind::GenerationBudgetExceeded`], and the runtime-owned
    /// [`ModelErrorKind::Timeout`]; absent otherwise. Consumers read this
    /// typed fact and never re-read [`Self::message`] for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<GenerationFailure>,
}

/// The byte bound of a [`ModelErrorKind::MalformedToolProposal`] diagnostic.
///
/// The evidence that refuses a tool proposal is provider/model-derived text:
/// a leaked protocol region, a truncated argument representation, an
/// undeclared tool name the model invented. None of it is size-bounded by
/// the provider, and all of it crosses into provider-independent runtime
/// semantics — the corrective prompt of a regeneration, the
/// `ModelRequestFailed` fact in the durable Event Journal, and the attempt's
/// terminal diagnostics. The bound is therefore enforced where the class is
/// constructed, not at any one consumer, so provider output can never author
/// an arbitrarily large durable diagnostic.
///
/// The bound covers the whole stored message, truncation marker included.
pub const MAX_MALFORMED_TOOL_PROPOSAL_MESSAGE_BYTES: usize = 512;

/// The marker appended to a malformed diagnostic that had to be shortened.
/// It is part of the bound, never an addition to it.
const MALFORMED_TOOL_PROPOSAL_TRUNCATION_MARKER: &str = "\u{2026}[truncated]";

/// Bounds one malformed-proposal diagnostic to
/// [`MAX_MALFORMED_TOOL_PROPOSAL_MESSAGE_BYTES`] without ever splitting a
/// Unicode scalar value.
///
/// This is the smallest helper the invariant needs and it is owned by the
/// model layer itself: the model plane must not reach into the runtime
/// subagent or tool modules for a primitive, which would point the
/// dependency the wrong way.
fn bound_malformed_message(mut message: String) -> String {
    if message.len() <= MAX_MALFORMED_TOOL_PROPOSAL_MESSAGE_BYTES {
        return message;
    }
    let mut end =
        MAX_MALFORMED_TOOL_PROPOSAL_MESSAGE_BYTES - MALFORMED_TOOL_PROPOSAL_TRUNCATION_MARKER.len();
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    message.truncate(end);
    message.push_str(MALFORMED_TOOL_PROPOSAL_TRUNCATION_MARKER);
    message
}

impl ModelError {
    /// Builds the normalized failure of one refused tool proposal.
    ///
    /// This is the only constructor of [`ModelErrorKind::MalformedToolProposal`],
    /// so the class and its provenance can never disagree. The disposition is
    /// deliberately [`ModelRetryDisposition::Never`]: a malformed generation
    /// is not a transient transport failure, and the bounded corrective
    /// regeneration it authorizes is a separate Agent-Loop budget keyed on
    /// the error class.
    ///
    /// The diagnostic is bounded **here**, where adapter evidence becomes
    /// provider-independent runtime semantics, so every downstream surface —
    /// corrective prompt, Event Journal, terminal diagnostics — inherits one
    /// bound from one owner. See
    /// [`MAX_MALFORMED_TOOL_PROPOSAL_MESSAGE_BYTES`].
    #[must_use]
    pub fn malformed_tool_proposal(
        source: MalformedToolProposalSource,
        message: impl Into<String>,
    ) -> Self {
        Self {
            kind: ModelErrorKind::MalformedToolProposal,
            message: bound_malformed_message(message.into()),
            retry_disposition: ModelRetryDisposition::Never,
            retry_after_ms: None,
            provider_code: None,
            context_overflow: None,
            malformed_tool_proposal: Some(source),
            generation: None,
        }
    }

    /// Builds the normalized failure of one single-generation safety fact.
    ///
    /// This is the only constructor of
    /// [`ModelErrorKind::GenerationDegenerated`] and
    /// [`ModelErrorKind::GenerationBudgetExceeded`], and the only place a
    /// runtime-owned request-deadline failure is built, so the class, its
    /// retry disposition, and its typed detail can never disagree.
    ///
    /// The disposition follows the contract rather than the call site:
    ///
    /// - a degenerate or over-budget generation is a **generation** defect,
    ///   so it is [`ModelRetryDisposition::Never`]. Repeating the identical
    ///   request is not recovery; the bounded semantic corrective generation
    ///   the Agent Loop owns is;
    /// - a deadline is a **transport** failure, so it stays
    ///   [`ModelRetryDisposition::Transient`] and keeps using the existing
    ///   Agent-Loop transient retry budget unchanged.
    ///
    /// The diagnostic is rendered from the typed fact, so it is authored by
    /// this runtime and can never carry provider output.
    #[must_use]
    pub fn generation_failure(failure: GenerationFailure) -> Self {
        let (kind, retry_disposition) = match failure {
            GenerationFailure::Degenerated { .. } => (
                ModelErrorKind::GenerationDegenerated,
                ModelRetryDisposition::Never,
            ),
            GenerationFailure::ProviderLengthLimit
            | GenerationFailure::RuntimeBudgetExceeded { .. } => (
                ModelErrorKind::GenerationBudgetExceeded,
                ModelRetryDisposition::Never,
            ),
            GenerationFailure::Timeout { .. } => {
                (ModelErrorKind::Timeout, ModelRetryDisposition::Transient)
            }
        };
        Self {
            kind,
            message: failure.message(),
            retry_disposition,
            retry_after_ms: None,
            provider_code: None,
            context_overflow: None,
            malformed_tool_proposal: None,
            generation: Some(failure),
        }
    }

    /// Completes one adapter-produced error at the model boundary.
    ///
    /// A [`ModelErrorKind::ContextWindowExceeded`] error gains the typed
    /// measurements recovered from the provider's own diagnostic; a
    /// [`ModelErrorKind::MalformedToolProposal`] error is re-bounded so an
    /// error built literally rather than through
    /// [`Self::malformed_tool_proposal`] still cannot carry unbounded
    /// provider text past normalization; every other class is returned
    /// unchanged. This is the last point at which a provider message is read
    /// for numbers — every consumer above the model layer reads
    /// [`Self::context_overflow`].
    #[must_use]
    pub(crate) fn normalized(mut self) -> Self {
        if matches!(self.kind, ModelErrorKind::ContextWindowExceeded)
            && self.context_overflow.is_none()
        {
            let report = context_overflow_report(&self.message);
            self.context_overflow = (!report.is_empty()).then_some(report);
        }
        if matches!(self.kind, ModelErrorKind::MalformedToolProposal) {
            self.message = bound_malformed_message(self.message);
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
        GenerationFailure, MAX_MALFORMED_TOOL_PROPOSAL_MESSAGE_BYTES, MalformedToolProposalSource,
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
            malformed_tool_proposal: None,
            generation: None,
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
            (
                ModelErrorKind::MalformedToolProposal,
                "malformed_tool_proposal",
            ),
        ];
        for (kind, expected) in cases {
            let value = serde_json::to_value(kind).expect("serialize kind");
            assert_eq!(value, expected);
        }
    }

    /// A refused tool proposal is always non-retryable, always carries its
    /// provenance, and round-trips with both facts intact.
    #[test]
    fn malformed_tool_proposal_carries_its_provenance() {
        for source in [
            MalformedToolProposalSource::ProviderDeclared,
            MalformedToolProposalSource::AdapterStructural,
            MalformedToolProposalSource::StreamAssembly,
            MalformedToolProposalSource::ReservedProtocolLeak,
        ] {
            let error = ModelError::malformed_tool_proposal(source, "refused");
            assert_eq!(error.kind, ModelErrorKind::MalformedToolProposal);
            assert_eq!(error.retry_disposition, ModelRetryDisposition::Never);
            assert_eq!(error.malformed_tool_proposal, Some(source));
            let json = serde_json::to_string(&error).expect("serialize error");
            assert!(json.contains("\"malformed_tool_proposal\""));
            let decoded: ModelError = serde_json::from_str(&json).expect("deserialize error");
            assert_eq!(decoded, error);
        }
    }

    /// Provider/model-derived evidence cannot author an unbounded runtime
    /// diagnostic. The bound is enforced at construction — before the class
    /// crosses into the corrective prompt, the Event Journal, or terminal
    /// diagnostics — and it covers the truncation marker rather than being
    /// exceeded by it. Class, provenance, and disposition are untouched, and
    /// the bounded error still round-trips.
    #[test]
    fn a_malformed_diagnostic_is_bounded_at_construction() {
        let oversized = "provider evidence ".repeat(4_096);
        assert!(oversized.len() > MAX_MALFORMED_TOOL_PROPOSAL_MESSAGE_BYTES);
        let error = ModelError::malformed_tool_proposal(
            MalformedToolProposalSource::ReservedProtocolLeak,
            oversized,
        );
        assert!(error.message.len() <= MAX_MALFORMED_TOOL_PROPOSAL_MESSAGE_BYTES);
        assert!(error.message.ends_with("\u{2026}[truncated]"));
        assert_eq!(error.kind, ModelErrorKind::MalformedToolProposal);
        assert_eq!(
            error.malformed_tool_proposal,
            Some(MalformedToolProposalSource::ReservedProtocolLeak)
        );
        assert_eq!(error.retry_disposition, ModelRetryDisposition::Never);
        let json = serde_json::to_string(&error).expect("serialize error");
        let decoded: ModelError = serde_json::from_str(&json).expect("deserialize error");
        assert_eq!(decoded, error);
    }

    /// Truncation never splits a Unicode scalar value: a diagnostic made of
    /// multi-byte characters is cut at a character boundary, so the stored
    /// message is still valid UTF-8 and still within the bound.
    #[test]
    fn malformed_diagnostic_truncation_is_utf8_safe() {
        for filler in ["\u{4f60}\u{597d}", "\u{1f600}", "e\u{301}"] {
            let oversized = filler.repeat(MAX_MALFORMED_TOOL_PROPOSAL_MESSAGE_BYTES);
            let error = ModelError::malformed_tool_proposal(
                MalformedToolProposalSource::AdapterStructural,
                oversized,
            );
            assert!(error.message.len() <= MAX_MALFORMED_TOOL_PROPOSAL_MESSAGE_BYTES);
            // `String` cannot hold invalid UTF-8; re-decoding its bytes
            // proves the cut landed on a real character boundary.
            assert_eq!(
                std::str::from_utf8(error.message.as_bytes()).expect("valid UTF-8"),
                error.message
            );
        }
    }

    /// A diagnostic already within the bound is stored verbatim, with no
    /// marker and no reallocation of meaning.
    #[test]
    fn a_short_malformed_diagnostic_is_unchanged() {
        let error = ModelError::malformed_tool_proposal(
            MalformedToolProposalSource::StreamAssembly,
            "the model tool proposal carries no usable invocation id",
        );
        assert_eq!(
            error.message,
            "the model tool proposal carries no usable invocation id"
        );
    }

    /// The typed generation payload round-trips, is present for exactly the
    /// generation classes, and is absent for every other one.
    #[test]
    fn the_generation_payload_is_present_for_exactly_its_classes() {
        let cases = [
            (
                GenerationFailure::Degenerated {
                    channel: crate::model::generation::GenerationChannel::Reasoning,
                    period_bytes: 9,
                    repetitions: 114,
                    span_bytes: 1_026,
                },
                ModelErrorKind::GenerationDegenerated,
                ModelRetryDisposition::Never,
            ),
            (
                GenerationFailure::ProviderLengthLimit,
                ModelErrorKind::GenerationBudgetExceeded,
                ModelRetryDisposition::Never,
            ),
            (
                GenerationFailure::RuntimeBudgetExceeded {
                    budget: crate::model::generation::GenerationBudgetKind::Reasoning,
                    limit_bytes: 512,
                    observed_bytes: 540,
                },
                ModelErrorKind::GenerationBudgetExceeded,
                ModelRetryDisposition::Never,
            ),
            (
                GenerationFailure::Timeout {
                    phase: crate::model::generation::ModelTimeoutPhase::StreamIdle,
                },
                ModelErrorKind::Timeout,
                // A deadline is a transport failure and keeps the existing
                // transient retry architecture; a generation defect never
                // does.
                ModelRetryDisposition::Transient,
            ),
        ];
        for (failure, kind, disposition) in cases {
            let error = ModelError::generation_failure(failure);
            assert_eq!(error.kind, kind);
            assert_eq!(error.retry_disposition, disposition);
            assert_eq!(error.generation, Some(failure));
            assert!(error.malformed_tool_proposal.is_none());
            assert!(error.context_overflow.is_none());
            let json = serde_json::to_string(&error).expect("serialize error");
            let decoded: ModelError = serde_json::from_str(&json).expect("deserialize error");
            assert_eq!(decoded, error);
        }

        let unrelated = ModelError {
            kind: ModelErrorKind::Transport,
            message: "connection reset".to_owned(),
            retry_disposition: ModelRetryDisposition::Transient,
            retry_after_ms: None,
            provider_code: None,
            context_overflow: None,
            malformed_tool_proposal: None,
            generation: None,
        };
        let value = serde_json::to_value(&unrelated).expect("serialize error");
        assert!(value.get("generation").is_none());
    }

    /// Normalization re-bounds the class as well, so an error assembled from
    /// its public fields rather than through the constructor still cannot
    /// carry unbounded provider text past the model boundary.
    #[test]
    fn normalization_re_bounds_a_literal_malformed_error() {
        let error = ModelError {
            kind: ModelErrorKind::MalformedToolProposal,
            message: "x".repeat(MAX_MALFORMED_TOOL_PROPOSAL_MESSAGE_BYTES * 3),
            retry_disposition: ModelRetryDisposition::Never,
            retry_after_ms: None,
            provider_code: None,
            context_overflow: None,
            malformed_tool_proposal: Some(MalformedToolProposalSource::ProviderDeclared),
            generation: None,
        }
        .normalized();
        assert!(error.message.len() <= MAX_MALFORMED_TOOL_PROPOSAL_MESSAGE_BYTES);
        assert_eq!(
            error.malformed_tool_proposal,
            Some(MalformedToolProposalSource::ProviderDeclared)
        );
    }

    /// Every other class leaves the provenance field absent on the wire, so a
    /// reader can never mistake an unrelated failure for a refused proposal.
    #[test]
    fn other_error_classes_carry_no_malformed_provenance() {
        let error = ModelError {
            kind: ModelErrorKind::ProviderError,
            message: "upstream unavailable".to_owned(),
            retry_disposition: ModelRetryDisposition::Transient,
            retry_after_ms: None,
            provider_code: None,
            context_overflow: None,
            malformed_tool_proposal: None,
            generation: None,
        };
        let value = serde_json::to_value(&error).expect("serialize error");
        assert!(value.get("malformed_tool_proposal").is_none());
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
