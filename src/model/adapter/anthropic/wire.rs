//! Adapter-private Anthropic Messages wire representation.
//!
//! These types mirror the current `/v1/messages` streaming protocol and are
//! private to the Anthropic adapter. The stream is parsed as raw JSON first
//! so that unknown top-level event types (which the API versioning policy
//! explicitly allows) never crash the parser; only events carrying known
//! output semantics are interpreted.

use serde::Deserialize;

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
pub(crate) fn parse_event(data: &str) -> Result<WireEvent, serde_json::Error> {
    let value: serde_json::Value = serde_json::from_str(data)?;
    let event_type = value.get("type").and_then(serde_json::Value::as_str);
    match event_type {
        Some("message_start") => {
            let message: WireMessage = serde_json::from_value(
                value.get("message").cloned().unwrap_or(serde_json::json!({})),
            )?;
            Ok(WireEvent::MessageStart { message })
        }
        Some("content_block_start") => {
            let index = integer_field(&value, "index").unwrap_or_default();
            let block: WireContentBlock = serde_json::from_value(
                value
                    .get("content_block")
                    .cloned()
                    .unwrap_or(serde_json::json!({})),
            )?;
            Ok(WireEvent::ContentBlockStart { index, block })
        }
        Some("content_block_delta") => {
            let index = integer_field(&value, "index").unwrap_or_default();
            let delta: WireDelta = serde_json::from_value(
                value.get("delta").cloned().unwrap_or(serde_json::json!({})),
            )?;
            Ok(WireEvent::ContentBlockDelta { index, delta })
        }
        Some("content_block_stop") => {
            let index = integer_field(&value, "index").unwrap_or_default();
            Ok(WireEvent::ContentBlockStop { index })
        }
        Some("message_delta") => {
            let delta: WireMessageDelta = serde_json::from_value(
                value
                    .get("delta")
                    .cloned()
                    .unwrap_or(serde_json::json!({})),
            )?;
            let usage: Option<WireUsage> = value
                .get("usage")
                .cloned()
                .map(serde_json::from_value)
                .transpose()?;
            Ok(WireEvent::MessageDelta { delta, usage })
        }
        Some("message_stop") => Ok(WireEvent::MessageStop),
        Some("ping") => Ok(WireEvent::Ping),
        Some("error") => {
            let error: WireError = serde_json::from_value(
                value
                    .get("error")
                    .cloned()
                    .unwrap_or(serde_json::json!({})),
            )?;
            Ok(WireEvent::Error { error })
        }
        Some(_) | None => Ok(WireEvent::Unknown),
    }
}

fn integer_field(value: &serde_json::Value, field: &str) -> Option<u32> {
    value
        .get(field)
        .and_then(serde_json::Value::as_u64)
        .and_then(|v| u32::try_from(v).ok())
}

/// The `message` object of `message_start`.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)] // fields preserved for provider wire-shape fidelity
pub(crate) struct WireMessage {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub usage: Option<WireUsage>,
}

/// The `content_block` object of `content_block_start`.
///
/// One flexible struct covers all known block types; `block_type` names the
/// discriminator and the remaining fields are optional by construction.
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
    pub redacted_thinking: Option<String>,
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

/// The `delta` object of `message_delta`.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)] // `stop_sequence` preserved for wire-shape fidelity
pub(crate) struct WireMessageDelta {
    #[serde(default)]
    pub stop_reason: Option<String>,
    #[serde(default)]
    pub stop_sequence: Option<String>,
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
}

/// The `error` object of an `error` stream event.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct WireError {
    #[serde(rename = "type", default)]
    pub error_type: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::{parse_event, WireEvent};

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
}
