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
use crate::model::types::AgentStatusAttachment;
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
/// checkpoint, tool definitions, observed provider usage, the pending fresh
/// inbound Agent Status attachment): identical inputs produce an identical
/// projection, including its estimated input measurement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextProjection {
    /// The ordered model-visible items: pinned system prefix, checkpoint
    /// summary (if one exists), then the retained literal suffix.
    pub items: Vec<ProjectionItem>,
    /// The ephemeral Agent Status attachment of a pending fresh inbound
    /// turn, when one exists. The attachment is projection-only: it is never
    /// canonical history, never checkpoint history, and never returned in
    /// `AgentExecutionResult.messages`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_status: Option<AgentStatusAttachment>,
    /// The deterministic planned input measurement of the full model
    /// request, including non-compacted contributors such as tool
    /// definitions and the Agent Status attachment.
    pub estimated_input: TokenMeasurement,
    /// The checkpoint generation that contributed to this projection, if
    /// one did.
    pub checkpoint_generation: Option<u64>,
}

impl ContextProjection {
    /// A deterministic fingerprint of this projection.
    ///
    /// The fingerprint is a FNV-1a hash over the canonical JSON of the
    /// projection items, the checkpoint generation, and the exact Agent
    /// Status attachment. It is used to decide whether a provider-reported
    /// input measurement applies to exactly this projection: a reported
    /// measurement is authoritative only when the projection being measured
    /// is byte-for-byte identical. Changing the sampled status (for example
    /// a new `current_time`) therefore changes the fingerprint.
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
            )
            .chain(
                serde_json::to_vec(&self.agent_status).expect("agent status attachment serializes"),
            );
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for byte in bytes {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash
    }
}

/// The compiled provider-neutral model context of one projection.
///
/// This is the explicit M4 "provider context compiler" boundary: the compiled
/// canonical messages plus the ephemeral Agent Status attachment travel
/// together into the `ModelRequest`. Agent Status is never encoded as a fake
/// canonical `MessageBlock`.
#[derive(Debug, Clone, PartialEq)]
pub struct CompiledContext {
    /// The compiled canonical messages of the projection.
    pub messages: Vec<MessageBlock>,
    /// The ephemeral Agent Status attachment, when the projection carries
    /// one.
    pub agent_status: Option<AgentStatusAttachment>,
}

/// Compiles a projection into a [`CompiledContext`].
///
/// A [`ProjectionItem::AgentSlice`] is materialized transiently as a
/// canonical `AgentMessageBlock` under its original source `MessageId`.
/// This is explicitly a model-context view: the resulting message is never
/// authoritative ledger content, is never committed, and never appears in
/// `AgentExecutionResult.messages`. The ephemeral Agent Status attachment
/// travels alongside and is never inserted into the canonical message list.
///
/// # Panics
///
/// Panics only if the canonical projection fails to serialize, which is
/// unreachable for the canonical runtime-owned types.
#[must_use]
pub fn compile_projection(projection: &ContextProjection) -> CompiledContext {
    CompiledContext {
        messages: projection
            .items
            .iter()
            .map(compile_item)
            .collect::<Vec<_>>(),
        agent_status: projection.agent_status.clone(),
    }
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
    use super::{CompiledContext, ContextProjection, ProjectionItem, compile_projection};
    use crate::context::status::{AgentStatusFact, AgentStatusSectionData, AgentStatusSectionId};
    use crate::context::tokens::{TokenMeasurement, TokenMeasurementSource};
    use crate::message::content::TextBlock;
    use crate::message::types::{MessageBlock, UserContentBlock, UserMessageBlock, UserSource};
    use crate::model::types::AgentStatusAttachment;
    use crate::runtime::identity::MessageId;

    fn projection() -> ContextProjection {
        ContextProjection {
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
            agent_status: None,
            estimated_input: TokenMeasurement {
                input_tokens: 7,
                source: TokenMeasurementSource::Estimated,
            },
            checkpoint_generation: Some(1),
        }
    }

    /// Identical projections produce identical fingerprints; different
    /// projections do not.
    #[test]
    fn fingerprints_are_deterministic_and_discriminating() {
        let projection = projection();
        let clone = projection.clone();
        assert_eq!(projection.fingerprint(), clone.fingerprint());
        let mut different = projection.clone();
        different.checkpoint_generation = Some(2);
        assert_ne!(projection.fingerprint(), different.fingerprint());
    }

    /// The exact Agent Status attachment participates in the fingerprint: a
    /// different status snapshot (for example a new `current_time`) changes
    /// the fingerprint even when every projection item is identical.
    #[test]
    fn agent_status_changes_the_fingerprint() {
        let projection = projection();
        let mut with_status = projection.clone();
        with_status.agent_status = Some(AgentStatusAttachment {
            target_message_id: MessageId::new("msg-1"),
            rendered:
                "<system-reminder>\nCurrent time: 2026-08-08T16:31:00+08:00\n</system-reminder>"
                    .to_owned(),
        });
        let mut other_snapshot = with_status.clone();
        other_snapshot
            .agent_status
            .as_mut()
            .expect("status present")
            .rendered =
            "<system-reminder>\nCurrent time: 2026-08-08T16:32:00+08:00\n</system-reminder>"
                .to_owned();
        assert_ne!(projection.fingerprint(), with_status.fingerprint());
        assert_ne!(
            with_status.fingerprint(),
            other_snapshot.fingerprint(),
            "a different status snapshot must invalidate the old projection fingerprint"
        );
    }

    /// Compiling a projection preserves whole messages, materializes agent
    /// slices under their original source message id, and carries the
    /// ephemeral status attachment separately — never as a fake message.
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
            agent_status: Some(AgentStatusAttachment {
                target_message_id: MessageId::new("msg-user"),
                rendered: "status footer".to_owned(),
            }),
            estimated_input: TokenMeasurement {
                input_tokens: 4,
                source: TokenMeasurementSource::Estimated,
            },
            checkpoint_generation: None,
        };
        let CompiledContext {
            messages,
            agent_status,
        } = compile_projection(&projection);
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
        let attachment = agent_status.expect("status attachment compiled");
        assert_eq!(attachment.target_message_id, MessageId::new("msg-user"));
        assert!(
            messages.iter().all(|message| {
                !matches!(message, MessageBlock::User(user) if user.content.iter().any(|block| {
                    matches!(block, UserContentBlock::Text(text) if text.text == "status footer")
                }))
            }),
            "the status attachment must never be compiled as a canonical message"
        );
    }

    /// Reserved section ids are recognized by the status subsystem.
    #[test]
    fn reserved_section_ids_are_stable() {
        assert_eq!(AgentStatusSectionId::TEMPORAL, "temporal");
        assert_eq!(
            AgentStatusSectionId::BACKGROUND_EXECUTION,
            "background_execution"
        );
        let temporal = AgentStatusSectionId::new("temporal");
        assert!(temporal.is_reserved());
        let custom = AgentStatusSectionId::new("custom");
        assert!(!custom.is_reserved());
        assert_eq!(
            AgentStatusSectionData::Facts {
                facts: vec![AgentStatusFact {
                    label: "running".to_owned(),
                    value: "1".to_owned(),
                }],
            },
            AgentStatusSectionData::Facts {
                facts: vec![AgentStatusFact {
                    label: "running".to_owned(),
                    value: "1".to_owned(),
                }],
            }
        );
    }
}
