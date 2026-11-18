//! Provider continuation / reasoning state boundary.
//!
//! Opaque reasoning and continuation state is never flattened into plain text: the
//! canonical layer preserves the provider-specific opaque state a later
//! adapter needs to continue a generation. Provider SDK-specific Rust types
//! are forbidden on this boundary; it holds rustX-owned serializable data
//! only. Model adapters (M2) are responsible for converting between provider
//! SDK state and this boundary.

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
pub use rustx_types::runtime::continuation::{
    AnthropicContinuation, OpenAiResponsesContinuation, ProviderContinuationState,
};
