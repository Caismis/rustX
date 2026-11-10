//! Provider continuation / reasoning state boundary.
//!
//! Opaque reasoning and continuation state is never flattened into plain text: the
//! canonical layer preserves the provider-specific opaque state a later
//! adapter needs to continue a generation. Provider SDK-specific Rust types
//! are forbidden on this boundary; it holds rustX-owned serializable data
//! only. Model adapters (M2) are responsible for converting between provider
//! SDK state and this boundary.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// A continuation belongs to an actual accepted output, never a last-Assistant heuristic.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcceptedOutputContinuationSource {
    pub request_id: crate::runtime::identity::RequestId,
    pub assistant: crate::model::projected::CanonicalMessageRef,
    /// Ordered provider-neutral input of the request that produced this output.
    pub covered_input_fingerprint: String,
    /// Exact accepted output semantic content, even when projected without canonical identity.
    pub accepted_output_fingerprint: String,
    pub native_state: ProviderContinuationState,
}

impl AcceptedOutputContinuationSource {
    /// Capture the committed Assistant output after canonical acceptance.
    /// # Errors
    /// A non-Assistant is never a continuation source.
    pub fn from_accepted_output(
        request_id: crate::runtime::identity::RequestId,
        conversation: &crate::runtime::identity::ConversationId,
        previous_input: &crate::model::SelectedModelInput,
        output: &crate::message::types::MessageBlock,
    ) -> Result<Self, String> {
        let crate::message::types::MessageBlock::Assistant(assistant) = output else {
            return Err("continuation source is not accepted Assistant output".into());
        };
        let native_state = assistant
            .content
            .iter()
            .rev()
            .find_map(|block| {
                if let crate::message::types::AssistantContentBlock::Reasoning(reasoning) = block {
                    reasoning.provider_state.as_ref()
                } else {
                    None
                }
            })
            .ok_or("accepted Assistant output has no native continuation unit")?
            .clone();
        Ok(Self {
            request_id,
            native_state,
            assistant: crate::model::projected::CanonicalMessageRef::of(conversation, output),
            covered_input_fingerprint: previous_input.semantic_fingerprint(),
            accepted_output_fingerprint: crate::model::types::semantic_view_fingerprint(
                &crate::model::projected::ModelMessageView::from(output),
            ),
        })
    }
}

/// Provider-specific continuation state preserved by the runtime.
///
/// `None` (absence) is the natural representation for protocols that carry no
/// continuation state, such as `OpenAI` Chat Completions, which resend the
/// full context instead of referencing a previous response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(schemars::JsonSchema)]
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
#[derive(schemars::JsonSchema)]
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
        items: Arc<Vec<serde_json::Value>>,
    },
}

/// Continuation state for the `Anthropic` Messages protocol.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AnthropicContinuation {
    /// Opaque provider state preserved verbatim by the adapter.
    pub opaque: Arc<serde_json::Value>,
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
                ]
                .into(),
            });
        let json = serde_json::to_string(&value).expect("serialize state");
        assert!(json.contains("\"stateless\""));
        assert!(json.contains("opaque-encrypted-reasoning"));
        let decoded: ProviderContinuationState =
            serde_json::from_str(&json).expect("deserialize state");
        assert_eq!(decoded, value);
    }

    #[test]
    fn stateless_clones_share_opaque_payload_and_serialize_identically() {
        let items = std::sync::Arc::new(vec![serde_json::json!({
            "type": "reasoning", "encrypted_content": "opaque".repeat(65_536),
            "future": {"nested":[1, "exact"]}
        })]);
        let state =
            ProviderContinuationState::OpenAiResponses(OpenAiResponsesContinuation::Stateless {
                items: items.clone(),
            });
        let ProviderContinuationState::OpenAiResponses(OpenAiResponsesContinuation::Stateless {
            items: retry,
        }) = state.clone()
        else {
            unreachable!()
        };
        assert!(std::sync::Arc::ptr_eq(&items, &retry));
        let original = serde_json::json!({"openai_responses": {"stateless": {"items": &*items}}});
        assert_eq!(
            serde_json::to_vec(&state).unwrap(),
            serde_json::to_vec(&original).unwrap()
        );
    }

    /// Anthropic continuation state preserves opaque provider data verbatim.
    #[test]
    fn anthropic_continuation_preserves_opaque_state() {
        let value = ProviderContinuationState::Anthropic(AnthropicContinuation {
            opaque: serde_json::json!({
                "signature": "opaque-anthropic-signature",
                "internal": { "cursor": 7 },
            })
            .into(),
        });
        let json = serde_json::to_string(&value).expect("serialize state");
        let decoded: ProviderContinuationState =
            serde_json::from_str(&json).expect("deserialize state");
        assert_eq!(decoded, value);
    }
}
