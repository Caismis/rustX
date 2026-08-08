//! The explicit context projection boundary.
//!
//! Canonical history ([`MessageBlock`] values committed to the conversation)
//! is durable truth and never changes. What the model sees is a
//! deterministic *projection* of that truth: the pinned system prefix, the
//! latest compaction checkpoint summary, and the retained literal suffix.
//!
//! [`ContextProjection`] is that model-visible projection. It is clearly
//! distinguishable from canonical history and is never stored as history:
//!
//! - a projection item may be a whole canonical [`MessageBlock`] or a
//!   projection-only slice of an `AgentMessageBlock`
//!   ([`ProjectionItem::AgentSlice`]);
//! - a projection-only `AgentSlice` is never persisted, never emitted as
//!   `AgentMessageCommitted`, and never placed into
//!   `AgentExecutionResult.messages`; when the projection is compiled into
//!   the current `ModelRequest.messages` boundary it is materialized
//!   transiently under the original source `MessageId` as a model-context
//!   view only;
//! - the projected input measurement carries explicit provenance
//!   ([`TokenMeasurement`]).
//!
//! [`TokenMeasurement`]: crate::context::tokens::TokenMeasurement

use serde::{Deserialize, Serialize};

use crate::context::tokens::TokenMeasurement;
use crate::message::types::{AgentContentBlock, MessageBlock};
use crate::runtime::identity::MessageId;

/// One ordered model-visible projection item.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProjectionItem {
    /// A whole canonical message, retained literally.
    Message(MessageBlock),
    /// A projection-only slice of one canonical `AgentMessageBlock`.
    ///
    /// This is a model-context view only: it is never persisted into
    /// canonical history, never emitted as `AgentMessageCommitted`, and
    /// never returned inside `AgentExecutionResult.messages`.
    AgentSlice {
        /// The identity of the source canonical agent message.
        source_message_id: MessageId,
        /// The retained content blocks, in original block order.
        content: Vec<AgentContentBlock>,
    },
}

/// The deterministic model-visible projection of one canonical history.
///
/// The projection is a pure function of (canonical history, latest context
/// checkpoint, tool definitions, observed provider usage): identical inputs
/// produce an identical projection, including its estimated input
/// measurement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextProjection {
    /// The ordered model-visible items: pinned system prefix, checkpoint
    /// summary (if one exists), then the retained literal suffix.
    pub items: Vec<ProjectionItem>,
    /// The deterministic planned input measurement of the full model
    /// request, including non-compacted contributors such as tool
    /// definitions.
    pub estimated_input: TokenMeasurement,
    /// The checkpoint generation that contributed to this projection, if
    /// one did.
    pub checkpoint_generation: Option<u64>,
}

impl ContextProjection {
    /// A deterministic fingerprint of this projection.
    ///
    /// The fingerprint is a FNV-1a hash over the canonical JSON of the
    /// projection items and checkpoint generation. It is used to decide
    /// whether a provider-reported input measurement applies to exactly this
    /// projection: a reported measurement is authoritative only when the
    /// projection being measured is byte-for-byte identical.
    ///
    /// # Panics
    ///
    /// Panics only if the canonical projection fails to serialize, which is
    /// unreachable for the canonical runtime-owned types.
    #[must_use]
    pub fn fingerprint(&self) -> u64 {
        let bytes = serde_json::to_vec(&self.items)
            .expect("canonical projection serializes")
            .into_iter()
            .chain(
                serde_json::to_vec(&self.checkpoint_generation)
                    .expect("checkpoint generation serializes"),
            );
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for byte in bytes {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash
    }
}

/// Compiles a projection into the current `ModelRequest.messages` boundary.
///
/// This is the M4 "provider context compiler": it produces the canonical
/// messages the request carries before the adapter translates them to a
/// provider wire format. It never performs provider wire compilation, which
/// remains an adapter responsibility.
///
/// A [`ProjectionItem::AgentSlice`] is materialized transiently as a
/// canonical `AgentMessageBlock` under its original source `MessageId`.
/// This is explicitly a model-context view: the resulting message is never
/// authoritative ledger content, is never committed, and never appears in
/// `AgentExecutionResult.messages`.
///
/// # Panics
///
/// Panics only if the canonical projection fails to serialize, which is
/// unreachable for the canonical runtime-owned types.
#[must_use]
pub fn compile_projection(projection: &ContextProjection) -> Vec<MessageBlock> {
    projection
        .items
        .iter()
        .map(compile_item)
        .collect::<Vec<_>>()
}

fn compile_item(item: &ProjectionItem) -> MessageBlock {
    match item {
        ProjectionItem::Message(message) => message.clone(),
        ProjectionItem::AgentSlice {
            source_message_id,
            content,
        } => MessageBlock::Agent(crate::message::types::AgentMessageBlock {
            id: source_message_id.clone(),
            content: content.clone(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::{ContextProjection, ProjectionItem, compile_projection};
    use crate::context::tokens::{TokenMeasurement, TokenMeasurementSource};
    use crate::message::content::TextBlock;
    use crate::message::types::{MessageBlock, UserContentBlock, UserMessageBlock, UserSource};
    use crate::runtime::identity::MessageId;

    /// Identical projections produce identical fingerprints; different
    /// projections do not.
    #[test]
    fn fingerprints_are_deterministic_and_discriminating() {
        let projection = ContextProjection {
            items: vec![ProjectionItem::Message(MessageBlock::User(
                UserMessageBlock {
                    id: MessageId::new("msg-1"),
                    content: vec![UserContentBlock::Text(TextBlock {
                        text: "hello".to_owned(),
                    })],
                    source: UserSource::Human,
                    kind: crate::message::types::InboundKind::Message,
                    timestamp: None,
                },
            ))],
            estimated_input: TokenMeasurement {
                input_tokens: 7,
                source: TokenMeasurementSource::Estimated,
            },
            checkpoint_generation: Some(1),
        };
        let clone = projection.clone();
        assert_eq!(projection.fingerprint(), clone.fingerprint());
        let mut different = projection.clone();
        different.checkpoint_generation = Some(2);
        assert_ne!(projection.fingerprint(), different.fingerprint());
    }

    /// Compiling a projection preserves whole messages and materializes
    /// agent slices under their original source message id.
    #[test]
    fn compile_materializes_slices_under_source_identity() {
        let projection = ContextProjection {
            items: vec![
                ProjectionItem::Message(MessageBlock::User(UserMessageBlock {
                    id: MessageId::new("msg-user"),
                    content: vec![UserContentBlock::Text(TextBlock {
                        text: "hi".to_owned(),
                    })],
                    source: UserSource::Human,
                    kind: crate::message::types::InboundKind::Message,
                    timestamp: None,
                })),
                ProjectionItem::AgentSlice {
                    source_message_id: MessageId::new("msg-agent"),
                    content: vec![crate::message::types::AgentContentBlock::Text(TextBlock {
                        text: "tail".to_owned(),
                    })],
                },
            ],
            estimated_input: TokenMeasurement {
                input_tokens: 4,
                source: TokenMeasurementSource::Estimated,
            },
            checkpoint_generation: None,
        };
        let messages = compile_projection(&projection);
        assert_eq!(messages.len(), 2);
        let ProjectionItem::Message(first) = &projection.items[0] else {
            panic!("first item must be a message");
        };
        assert_eq!(&messages[0], first);
        let MessageBlock::Agent(agent) = &messages[1] else {
            panic!("slice must compile into an agent message");
        };
        assert_eq!(agent.id.as_str(), "msg-agent");
        assert_eq!(agent.content.len(), 1);
    }
}
