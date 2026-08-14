//! Adapter-private Anthropic Messages wire representation.
//!
//! These types mirror the current `/v1/messages` streaming protocol and are
//! private to the Anthropic adapter. The stream is parsed as raw JSON first
//! so that unknown top-level event types (which the API versioning policy
//! explicitly allows) never crash the parser; only events carrying known
//! output semantics are interpreted.

use serde::Deserialize;

/// A parse failure of one stream event.
#[derive(Debug)]
pub(crate) enum WireParseError {
    /// The event payload was not valid JSON or did not fit its wire shape.
    Json(serde_json::Error),
    /// A content-block event carried no `index` field.
    MissingIndex(&'static str),
    /// A content-block event carried an `index` that is not a non-negative
    /// integer representable as `u32`.
    InvalidIndex(&'static str),
}

impl std::fmt::Display for WireParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(error) => write!(f, "{error}"),
            Self::MissingIndex(event) => {
                write!(f, "{event} event lacks the required integer index field")
            }
            Self::InvalidIndex(event) => {
                write!(
                    f,
                    "{event} event carries an invalid (missing, negative, non-integer, or overflowing) index"
                )
            }
        }
    }
}

impl std::error::Error for WireParseError {}

impl From<serde_json::Error> for WireParseError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

/// One parsed stream event, decoded from the SSE `data` payload.
#[derive(Debug, Clone)]
pub(crate) enum WireEvent {
    MessageStart {
        message: WireMessage,
    },
    ContentBlockStart {
        index: u32,
        block: WireContentBlock,
    },
    ContentBlockDelta {
        index: u32,
        delta: WireDelta,
    },
    ContentBlockStop {
        index: u32,
    },
    MessageDelta {
        delta: WireMessageDelta,
        usage: Option<WireUsage>,
        stop_details: Option<WireStopDetails>,
    },
    MessageStop,
    Ping,
    Error {
        error: WireError,
    },
    /// A top-level event type with no known output semantics; safely ignored.
    Unknown,
}

/// Decodes a stream event from its JSON payload.
///
/// Content-block events require a valid non-negative `index`; a missing,
/// negative, non-integer, or overflowing index is a provider protocol error,
/// never silently reinterpreted as `0`.
pub(crate) fn parse_event(data: &str) -> Result<WireEvent, WireParseError> {
    let value: serde_json::Value = serde_json::from_str(data)?;
    let event_type = value.get("type").and_then(serde_json::Value::as_str);
    match event_type {
        Some("message_start") => {
            let message: WireMessage = serde_json::from_value(
                value
                    .get("message")
                    .cloned()
                    .unwrap_or(serde_json::json!({})),
            )?;
            Ok(WireEvent::MessageStart { message })
        }
        Some("content_block_start") => {
            let index = required_index(&value, "content_block_start")?;
            let block: WireContentBlock = serde_json::from_value(
                value
                    .get("content_block")
                    .cloned()
                    .unwrap_or(serde_json::json!({})),
            )?;
            Ok(WireEvent::ContentBlockStart { index, block })
        }
        Some("content_block_delta") => {
            let index = required_index(&value, "content_block_delta")?;
            let delta: WireDelta = serde_json::from_value(
                value.get("delta").cloned().unwrap_or(serde_json::json!({})),
            )?;
            Ok(WireEvent::ContentBlockDelta { index, delta })
        }
        Some("content_block_stop") => {
            let index = required_index(&value, "content_block_stop")?;
            Ok(WireEvent::ContentBlockStop { index })
        }
        Some("message_delta") => {
            let delta: WireMessageDelta = serde_json::from_value(
                value.get("delta").cloned().unwrap_or(serde_json::json!({})),
            )?;
            let usage: Option<WireUsage> = value
                .get("usage")
                .cloned()
                .map(serde_json::from_value)
                .transpose()?;
            let top_level_stop_details: Option<WireStopDetails> = value
                .get("stop_details")
                .cloned()
                .map(serde_json::from_value)
                .transpose()?;
            let stop_details = delta.details.clone().or(top_level_stop_details);
            Ok(WireEvent::MessageDelta {
                delta,
                usage,
                stop_details,
            })
        }
        Some("message_stop") => Ok(WireEvent::MessageStop),
        Some("ping") => Ok(WireEvent::Ping),
        Some("error") => {
            let error: WireError = serde_json::from_value(
                value.get("error").cloned().unwrap_or(serde_json::json!({})),
            )?;
            Ok(WireEvent::Error { error })
        }
        Some(_) | None => Ok(WireEvent::Unknown),
    }
}

/// Reads the required content-block `index` field as a `u32`.
///
/// Missing, negative, non-integer, and overflowing values are all rejected:
/// the index is required for deterministic block association and must never
/// default to `0`.
fn required_index(value: &serde_json::Value, event: &'static str) -> Result<u32, WireParseError> {
    match value.get("index") {
        None => Err(WireParseError::MissingIndex(event)),
        Some(index) => index
            .as_u64()
            .and_then(|v| u32::try_from(v).ok())
            .ok_or(WireParseError::InvalidIndex(event)),
    }
}

/// The `message` object of `message_start`.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)] // fields preserved for provider wire-shape fidelity
pub(crate) struct WireMessage {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub usage: Option<WireUsage>,
    #[serde(default)]
    pub content: Option<Vec<serde_json::Value>>,
}

/// The `content_block` object of `content_block_start`.
///
/// One flexible struct covers all known block types; `block_type` names the
/// discriminator and the remaining fields are optional by construction.
/// `redacted_thinking` blocks carry their opaque provider state in `data`
/// (the provider wire field); no synthetic field is invented.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)] // `input` preserved for wire-shape fidelity
pub(crate) struct WireContentBlock {
    #[serde(rename = "type", default)]
    pub block_type: Option<String>,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub input: Option<serde_json::Value>,
    #[serde(default)]
    pub thinking: Option<String>,
    #[serde(default)]
    pub signature: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub citations: Option<Vec<serde_json::Value>>,
    /// Opaque encrypted provider state of `redacted_thinking` blocks.
    #[serde(default)]
    pub data: Option<String>,
}

/// The `delta` object of `content_block_delta`.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct WireDelta {
    #[serde(rename = "type", default)]
    pub delta_type: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub thinking: Option<String>,
    #[serde(default)]
    pub signature: Option<String>,
    #[serde(default)]
    pub partial_json: Option<String>,
}

/// The `delta` object of `message_delta`, plus the top-level `stop_details`
/// carried by refusal terminations.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)] // `sequence` preserved for wire-shape fidelity
pub(crate) struct WireMessageDelta {
    #[serde(default, rename = "stop_reason")]
    pub reason: Option<String>,
    #[serde(default, rename = "stop_sequence")]
    pub sequence: Option<String>,
    #[serde(default, rename = "stop_details")]
    pub details: Option<WireStopDetails>,
}

/// The `stop_details` object of a refusal `message_delta`.
///
/// Per the current Messages API, `stop_details` is `null` for every stop
/// reason other than `refusal`; on a refusal it names the policy category and
/// carries a human-readable, not-stable explanation.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)] // `category`/`stop_details_type` preserved for wire fidelity
pub(crate) struct WireStopDetails {
    #[serde(rename = "type", default)]
    pub stop_details_type: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub explanation: Option<String>,
}

/// The `usage` object. `message_delta` usage snapshots are cumulative; they
/// are never summed.
#[derive(Debug, Clone, Deserialize)]
// `cache_creation_input_tokens` preserved for wire-shape fidelity; the field
// names mirror the provider wire schema by design.
#[allow(dead_code, clippy::struct_field_names)]
pub(crate) struct WireUsage {
    #[serde(default)]
    pub input_tokens: Option<u64>,
    #[serde(default)]
    pub output_tokens: Option<u64>,
    #[serde(default)]
    pub cache_read_input_tokens: Option<u64>,
    #[serde(default)]
    pub cache_creation_input_tokens: Option<u64>,
    #[serde(default)]
    pub output_tokens_details: Option<WireOutputTokensDetails>,
}

/// The `output_tokens_details` object reported by current models.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct WireOutputTokensDetails {
    #[serde(default)]
    pub thinking_tokens: Option<u64>,
}

/// The `error` object of an `error` stream event.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct WireError {
    #[serde(rename = "type", default)]
    pub error_type: Option<String>,
    /// `OpenRouter`'s stable cross-protocol error classification.
    #[serde(default, rename = "error_type")]
    pub precise_error_type: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::{WireEvent, WireParseError, parse_event};

    /// Unknown top-level event types do not crash the parser.
    #[test]
    fn unknown_top_level_events_are_tolerated() {
        let event = parse_event(r#"{"type": "future_event", "payload": [1, 2]}"#).expect("parse");
        assert!(matches!(event, WireEvent::Unknown));
    }

    /// A missing `type` is also tolerated as an unknown event.
    #[test]
    fn missing_type_is_tolerated() {
        let event = parse_event(r#"{"sequence_number": 1}"#).expect("parse");
        assert!(matches!(event, WireEvent::Unknown));
    }

    /// Malformed JSON is a hard parse failure, not an unknown event.
    #[test]
    fn malformed_json_is_a_parse_error() {
        assert!(parse_event("not json").is_err());
    }

    /// Known event families decode with their fields.
    #[test]
    fn known_events_decode() {
        let event = parse_event(
            r#"{"type": "content_block_delta", "index": 2, "delta": {"type": "text_delta", "text": "hi"}}"#,
        )
        .expect("parse");
        match event {
            WireEvent::ContentBlockDelta { index, delta } => {
                assert_eq!(index, 2);
                assert_eq!(delta.delta_type.as_deref(), Some("text_delta"));
                assert_eq!(delta.text.as_deref(), Some("hi"));
            }
            other => panic!("unexpected event {other:?}"),
        }
    }

    /// A content-block event without an `index` is a hard parse error, never
    /// an implicit zero.
    #[test]
    fn missing_index_is_a_parse_error() {
        for json in [
            r#"{"type": "content_block_start", "content_block": {"type": "text", "text": ""}}"#,
            r#"{"type": "content_block_delta", "delta": {"type": "text_delta", "text": "hi"}}"#,
            r#"{"type": "content_block_stop"}"#,
        ] {
            assert!(
                matches!(parse_event(json), Err(WireParseError::MissingIndex(_))),
                "missing index must be a hard parse error: {json}"
            );
        }
    }

    /// Non-integer, negative, and overflowing indexes are hard parse errors.
    #[test]
    fn invalid_index_is_a_parse_error() {
        for index in [r#""0""#, r"-1", r"1.5", r"4294967296"] {
            let json = format!(
                r#"{{"type": "content_block_delta", "index": {index}, "delta": {{"type": "text_delta", "text": "hi"}}}}"#
            );
            assert!(
                matches!(parse_event(&json), Err(WireParseError::InvalidIndex(_))),
                "invalid index {index} must be a hard parse error"
            );
        }
    }

    /// `message_delta` preserves top-level `stop_details` and cumulative usage
    /// including thinking-token details.
    #[test]
    fn message_delta_preserves_stop_details_and_thinking_tokens() {
        let event = parse_event(
            r#"{"type": "message_delta", "delta": {"stop_reason": "refusal", "stop_sequence": null}, "stop_details": {"type": "refusal", "category": "cyber", "explanation": "declined"}, "usage": {"input_tokens": 5, "output_tokens": 0, "output_tokens_details": {"thinking_tokens": 0}}}"#,
        )
        .expect("parse");
        match event {
            WireEvent::MessageDelta {
                stop_details,
                usage,
                ..
            } => {
                let stop_details = stop_details.expect("stop_details present");
                assert_eq!(stop_details.stop_details_type.as_deref(), Some("refusal"));
                assert_eq!(stop_details.category.as_deref(), Some("cyber"));
                assert_eq!(stop_details.explanation.as_deref(), Some("declined"));
                let usage = usage.expect("usage present");
                assert_eq!(usage.input_tokens, Some(5));
                assert_eq!(
                    usage
                        .output_tokens_details
                        .as_ref()
                        .and_then(|d| d.thinking_tokens),
                    Some(0)
                );
            }
            other => panic!("unexpected event {other:?}"),
        }
    }

    /// `redacted_thinking` blocks carry their opaque state in `data`.
    #[test]
    fn redacted_thinking_block_decodes_data() {
        let event = parse_event(
            r#"{"type": "content_block_start", "index": 0, "content_block": {"type": "redacted_thinking", "data": "opaque-redacted-blob"}}"#,
        )
        .expect("parse");
        match event {
            WireEvent::ContentBlockStart { index, block } => {
                assert_eq!(index, 0);
                assert_eq!(block.block_type.as_deref(), Some("redacted_thinking"));
                assert_eq!(block.data.as_deref(), Some("opaque-redacted-blob"));
            }
            other => panic!("unexpected event {other:?}"),
        }
    }
}
