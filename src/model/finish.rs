//! Typed normalized model finish reasons.

use serde::{Deserialize, Serialize};

/// Why a model generation finished.
///
/// Finish reasons are semantic outcomes, not raw provider strings. When a
/// provider returns a reason the runtime cannot normalize, it is preserved in
/// [`ModelFinishReason::Other`] rather than collapsing the entire model into
/// arbitrary strings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ModelFinishReason {
    /// The model stopped normally.
    Stop,
    /// The model emitted tool calls and stopped.
    ToolCalls,
    /// The generation hit a token/length limit.
    Length,
    /// The generation was terminated by content filtering or safety systems.
    ContentFilter,
    /// The model refused to comply.
    Refusal,
    /// A provider termination reason that is not yet normalized.
    Other {
        /// The original provider reason, preserved for diagnostics.
        reason: String,
    },
}

#[cfg(test)]
mod tests {
    use super::ModelFinishReason;

    /// Finish reasons serialize with stable discriminators.
    #[test]
    fn finish_reason_discriminators_are_stable() {
        let cases = [
            (ModelFinishReason::Stop, "stop"),
            (ModelFinishReason::ToolCalls, "tool_calls"),
            (ModelFinishReason::Length, "length"),
            (ModelFinishReason::ContentFilter, "content_filter"),
            (ModelFinishReason::Refusal, "refusal"),
        ];
        for (reason, expected) in cases {
            let value = serde_json::to_value(&reason).expect("serialize reason");
            assert_eq!(value["type"], expected);
            let decoded: ModelFinishReason =
                serde_json::from_value(value).expect("deserialize reason");
            assert_eq!(decoded, reason);
        }
    }

    /// Unknown provider reasons survive round-trip with their original text.
    #[test]
    fn unknown_finish_reason_is_preserved() {
        let reason = ModelFinishReason::Other {
            reason: "max_turn_reached".to_owned(),
        };
        let json = serde_json::to_string(&reason).expect("serialize reason");
        let decoded: ModelFinishReason = serde_json::from_str(&json).expect("deserialize reason");
        assert_eq!(decoded, reason);
    }
}
