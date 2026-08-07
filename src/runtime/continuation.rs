//! Provider continuation / reasoning state boundary.
//!
//! Reasoning and continuation state is never flattened into plain text: the
//! canonical layer preserves the provider-specific opaque state a later
//! adapter needs to continue a generation. Provider SDK-specific Rust types
//! are forbidden on this boundary; it holds rustX-owned serializable data
//! only. Model adapters (M2) are responsible for converting between provider
//! SDK state and this boundary.

use serde::{Deserialize, Serialize};

/// Provider-specific continuation state preserved by the runtime.
///
/// `None` (absence) is the natural representation for protocols that carry no
/// continuation state, such as `OpenAI` Chat Completions, which resend the
/// full context instead of referencing a previous response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderContinuationState {
    /// Continuation state for the `OpenAI` Responses protocol.
    #[serde(rename = "openai_responses")]
    OpenAiResponses(OpenAiResponsesContinuation),
    /// Continuation state for the `Anthropic` Messages protocol.
    Anthropic(AnthropicContinuation),
}

/// Continuation state for the `OpenAI` Responses protocol.
///
/// The Responses protocol supports two continuation modes, and the canonical
/// boundary preserves either one without depending on a provider SDK:
///
/// - [`OpenAiResponsesContinuation::Stored`]: stateful operation, continued
///   by referencing the provider-stored previous response.
/// - [`OpenAiResponsesContinuation::Stateless`]: stateless operation
///   (`store: false`, zero-data-retention), continued by preserving the
///   previous response's output/reasoning items and passing them back on
///   later requests. This includes opaque encrypted reasoning content.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenAiResponsesContinuation {
    /// Stateful continuation by reference to the stored previous response.
    Stored {
        /// The provider-assigned response id of the previous response to
        /// continue.
        previous_response_id: String,
    },
    /// Stateless continuation by preserved opaque response state.
    Stateless {
        /// Ordered output/reasoning items from the previous response that
        /// must be passed back on the next request, including opaque
        /// encrypted reasoning content.
        items: Vec<serde_json::Value>,
    },
}

/// Continuation state for the `Anthropic` Messages protocol.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnthropicContinuation {
    /// Opaque provider state preserved verbatim by the adapter.
    pub opaque: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::{AnthropicContinuation, OpenAiResponsesContinuation, ProviderContinuationState};

    /// Stored continuation round-trips with its provider response id.
    #[test]
    fn openai_responses_stored_continuation_round_trip() {
        let value =
            ProviderContinuationState::OpenAiResponses(OpenAiResponsesContinuation::Stored {
                previous_response_id: "resp_abc123".to_owned(),
            });
        let json = serde_json::to_string(&value).expect("serialize state");
        assert!(json.contains("\"openai_responses\""));
        assert!(json.contains("\"stored\""));
        assert!(json.contains("resp_abc123"));
        let decoded: ProviderContinuationState =
            serde_json::from_str(&json).expect("deserialize state");
        assert_eq!(decoded, value);
    }

    /// Stateless continuation preserves opaque reasoning/output items.
    #[test]
    fn openai_responses_stateless_continuation_round_trip() {
        let value =
            ProviderContinuationState::OpenAiResponses(OpenAiResponsesContinuation::Stateless {
                items: vec![
                    serde_json::json!({
                        "type": "reasoning",
                        "id": "rs_1",
                        "summary": [],
                        "encrypted_content": "opaque-encrypted-reasoning"
                    }),
                    serde_json::json!({"type": "output_text", "text": "hello"}),
                ],
            });
        let json = serde_json::to_string(&value).expect("serialize state");
        assert!(json.contains("\"stateless\""));
        assert!(json.contains("opaque-encrypted-reasoning"));
        let decoded: ProviderContinuationState =
            serde_json::from_str(&json).expect("deserialize state");
        assert_eq!(decoded, value);
    }

    /// Anthropic continuation state preserves opaque provider data verbatim.
    #[test]
    fn anthropic_continuation_preserves_opaque_state() {
        let value = ProviderContinuationState::Anthropic(AnthropicContinuation {
            opaque: serde_json::json!({
                "signature": "opaque-anthropic-signature",
                "internal": { "cursor": 7 },
            }),
        });
        let json = serde_json::to_string(&value).expect("serialize state");
        let decoded: ProviderContinuationState =
            serde_json::from_str(&json).expect("deserialize state");
        assert_eq!(decoded, value);
    }
}
