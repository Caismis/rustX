//! The finite model-visible projection of one exact conversation Surface.
//!
//! The projection contains complete canonical messages and the exact
//! request-time Effective System Prompt. It owns no contributor, provenance,
//! admission, or provider semantics; those are settled before this boundary.

use serde::{Deserialize, Serialize};

use crate::conversation::SurfaceRevision;
use crate::message::types::MessageBlock;
use crate::runtime::types::TokenMeasurement;

/// The deterministic model-visible projection of one Surface revision.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextProjection {
    /// The exact Surface revision used for this projection.
    pub surface_revision: SurfaceRevision,
    /// Ordered complete canonical messages selected by the Surface.
    pub messages: Vec<MessageBlock>,
    /// The exact rustX-owned Effective System Prompt for this request.
    #[serde(default)]
    pub effective_system_prompt: String,
    /// The measured or estimated input for the full provider-neutral request.
    pub estimated_input: TokenMeasurement,
}

impl ContextProjection {
    /// Returns a deterministic fingerprint of the exact projected context.
    ///
    /// Provider measurements are reusable only when this fingerprint matches:
    /// the revision, messages, and rendered system prompt are all part of the
    /// measured input identity.
    ///
    /// # Panics
    ///
    /// Panics only if a runtime-owned projection value cannot be serialized;
    /// all fields have infallible serde representations.
    #[must_use]
    pub fn fingerprint(&self) -> u64 {
        let bytes = serde_json::to_vec(&self.surface_revision)
            .expect("surface revision serializes")
            .into_iter()
            .chain(serde_json::to_vec(&self.messages).expect("canonical messages serialize"))
            .chain(
                serde_json::to_vec(&self.effective_system_prompt)
                    .expect("system prompt serializes"),
            );
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for byte in bytes {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash
    }
}

#[cfg(test)]
mod tests {
    use super::ContextProjection;
    use crate::conversation::SurfaceRevision;
    use crate::message::content::TextBlock;
    use crate::message::types::{
        InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
    };
    use crate::runtime::identity::MessageId;
    use crate::runtime::types::{TokenMeasurement, TokenMeasurementSource};

    fn projection() -> ContextProjection {
        ContextProjection {
            surface_revision: SurfaceRevision::new(1),
            messages: vec![MessageBlock::User(UserMessageBlock {
                id: MessageId::new("msg-1"),
                content: vec![UserContentBlock::Text(TextBlock {
                    text: "hello".to_owned(),
                })],
                source: UserSource::Human,
                kind: InboundKind::Message,
                timestamp: None,
            })],
            effective_system_prompt: "runtime identity".to_owned(),
            estimated_input: TokenMeasurement {
                input_tokens: 7,
                source: TokenMeasurementSource::Estimated,
            },
        }
    }

    #[test]
    fn fingerprint_includes_revision_messages_and_effective_prompt() {
        let projection = projection();
        assert_eq!(projection.fingerprint(), projection.clone().fingerprint());

        let mut other_revision = projection.clone();
        other_revision.surface_revision = SurfaceRevision::new(2);
        assert_ne!(projection.fingerprint(), other_revision.fingerprint());

        let mut other_messages = projection.clone();
        other_messages.messages.clear();
        assert_ne!(projection.fingerprint(), other_messages.fingerprint());

        let mut other_prompt = projection;
        other_prompt.effective_system_prompt = "changed".to_owned();
        assert_ne!(other_messages.fingerprint(), other_prompt.fingerprint());
    }
}
