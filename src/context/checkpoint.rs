//! Context checkpoints.
//!
//! M4 owns the checkpoint contract; M8 owns the final durable
//! database/event-journal implementation. The persistence abstraction is a
//! synchronous [`ContextCheckpointStore`] so commit semantics stay simple
//! and deterministic; an in-memory implementation exists for tests and for
//! the M4 development backend.
//!
//! A checkpoint carries stable identity, never a raw vector position: the
//! boundary references canonical [`MessageId`] values (and a
//! [`ContentBlockIndex`] for split turns), and the summary message has a
//! deterministic identity derived from the conversation id and the
//! checkpoint generation.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::context::error::{ContextError, ContextErrorKind};
use crate::context::tokens::TokenMeasurement;
use crate::message::types::{ContentBlockIndex, UserMessageBlock};
use crate::runtime::identity::{ConversationId, MessageId};

/// Where one checkpoint's coverage ends.
///
/// The boundary is a projection fact, not canonical history: it describes
/// how far compaction covered the compactable region, never a deletion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContextBoundary {
    /// Compacted non-pinned history is covered through this canonical
    /// message, inclusive.
    AfterMessage {
        /// The last covered canonical message.
        message_id: MessageId,
    },
    /// The turn of this canonical agent message was split: its prefix before
    /// `first_retained_block` (and the tool results of the tool calls in
    /// that prefix) is covered; the retained projection starts at the
    /// remaining content slice.
    InsideAgent {
        /// The split canonical agent message.
        message_id: MessageId,
        /// The first content block of the retained slice.
        first_retained_block: ContentBlockIndex,
    },
}

/// One durable context checkpoint.
///
/// A checkpoint contains enough stable information to reconstruct the
/// projection without vector indices: the conversation identity, the
/// monotonically increasing generation, the runtime-owned summary message,
/// the coverage boundary, and the token measurements around the compaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextCheckpoint {
    /// The conversation this checkpoint belongs to.
    pub conversation_id: ConversationId,
    /// Monotonic checkpoint generation, starting at 1.
    pub generation: u64,
    /// The compaction summary message: a runtime-provided inbound
    /// `UserMessageBlock` with `InboundKind::CompactionSummary`.
    pub summary: UserMessageBlock,
    /// The coverage boundary of this checkpoint.
    pub boundary: ContextBoundary,
    /// The measured input of the projection before compaction.
    pub tokens_before: TokenMeasurement,
    /// The estimated input of the projection after compaction.
    pub estimated_tokens_after: u64,
}

/// The checkpoint persistence abstraction.
///
/// A synchronous contract keeps deterministic commit semantics simple in M4;
/// M8 owns the durable implementation. Saving replaces/advances the latest
/// checkpoint for a conversation; generations increase monotonically.
pub trait ContextCheckpointStore: Send + Sync {
    /// Loads the latest checkpoint of a conversation, if any.
    ///
    /// # Errors
    ///
    /// Returns a checkpoint-store failure.
    fn load(
        &self,
        conversation_id: &ConversationId,
    ) -> Result<Option<ContextCheckpoint>, ContextError>;

    /// Saves a checkpoint as the latest checkpoint of its conversation.
    ///
    /// # Errors
    ///
    /// Returns [`ContextErrorKind::CheckpointSaveFailed`] when the store
    /// cannot persist the checkpoint.
    fn save(&self, checkpoint: &ContextCheckpoint) -> Result<(), ContextError>;
}

/// An in-memory checkpoint store.
///
/// One store instance can be shared across attempts of one conversation,
/// which is how repeated compaction advances generation across attempts.
#[derive(Debug, Default)]
pub struct InMemoryCheckpointStore {
    checkpoints: Mutex<HashMap<ConversationId, ContextCheckpoint>>,
}

impl InMemoryCheckpointStore {
    /// Creates an empty in-memory store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Wraps the store in a shared reference for a [`ContextCheckpointStore`]
    /// slot.
    #[must_use]
    pub fn shared(self) -> Arc<Self> {
        Arc::new(self)
    }
}

impl ContextCheckpointStore for InMemoryCheckpointStore {
    fn load(
        &self,
        conversation_id: &ConversationId,
    ) -> Result<Option<ContextCheckpoint>, ContextError> {
        Ok(self
            .checkpoints
            .lock()
            .expect("in-memory checkpoint lock")
            .get(conversation_id)
            .cloned())
    }

    fn save(&self, checkpoint: &ContextCheckpoint) -> Result<(), ContextError> {
        let mut store = self.checkpoints.lock().expect("in-memory checkpoint lock");
        if let Some(latest) = store.get(&checkpoint.conversation_id)
            && latest.generation >= checkpoint.generation
        {
            return Err(ContextError::new(
                ContextErrorKind::CheckpointSaveFailed,
                format!(
                    "checkpoint generation {} does not advance beyond {}",
                    checkpoint.generation, latest.generation
                ),
            ));
        }
        store.insert(checkpoint.conversation_id.clone(), checkpoint.clone());
        Ok(())
    }
}

/// The deterministic message identity of a compaction summary.
///
/// The identity is a namespaced function of the conversation id and the
/// checkpoint generation, so summaries are reproducible without random ids.
#[must_use]
pub fn summary_message_id(conversation_id: &ConversationId, generation: u64) -> MessageId {
    MessageId::new(format!("{conversation_id}-summary-{generation}"))
}

#[cfg(test)]
mod tests {
    use super::{ContextBoundary, ContextCheckpoint, InMemoryCheckpointStore, summary_message_id};
    use crate::context::checkpoint::ContextCheckpointStore;
    use crate::context::tokens::{TokenMeasurement, TokenMeasurementSource};
    use crate::message::content::TextBlock;
    use crate::message::types::{
        ContentBlockIndex, InboundKind, UserContentBlock, UserMessageBlock, UserSource,
    };
    use crate::runtime::identity::{ConversationId, MessageId};

    fn checkpoint(conversation: &ConversationId, generation: u64) -> ContextCheckpoint {
        ContextCheckpoint {
            conversation_id: conversation.clone(),
            generation,
            summary: UserMessageBlock {
                id: summary_message_id(conversation, generation),
                content: vec![UserContentBlock::Text(TextBlock {
                    text: format!("summary {generation}"),
                })],
                source: UserSource::Runtime,
                kind: InboundKind::CompactionSummary,
            },
            boundary: ContextBoundary::AfterMessage {
                message_id: MessageId::new("msg-a1"),
            },
            tokens_before: TokenMeasurement {
                input_tokens: 900,
                source: TokenMeasurementSource::Estimated,
            },
            estimated_tokens_after: 100,
        }
    }

    /// Saving replaces the latest checkpoint and generations advance
    /// monotonically.
    #[test]
    fn save_advances_the_latest_checkpoint() {
        let conversation = ConversationId::new("conv-1");
        let store = InMemoryCheckpointStore::new();
        assert!(store.load(&conversation).expect("load").is_none());
        let first = checkpoint(&conversation, 1);
        store.save(&first).expect("save generation 1");
        assert_eq!(
            store.load(&conversation).expect("load").expect("latest"),
            first
        );
        let second = checkpoint(&conversation, 2);
        store.save(&second).expect("save generation 2");
        assert_eq!(
            store.load(&conversation).expect("load").expect("latest"),
            second
        );
        assert!(
            store.save(&first).is_err(),
            "a stale generation must never replace the latest checkpoint"
        );
    }

    /// Summary ids are deterministic and namespaced by conversation.
    #[test]
    fn summary_ids_are_deterministic_and_namespaced() {
        let conversation = ConversationId::new("conv-1");
        assert_eq!(
            summary_message_id(&conversation, 1).as_str(),
            "conv-1-summary-1"
        );
        assert_eq!(
            summary_message_id(&conversation, 1),
            summary_message_id(&conversation, 1)
        );
        assert_ne!(
            summary_message_id(&conversation, 1),
            summary_message_id(&conversation, 2)
        );
        assert_ne!(
            summary_message_id(&conversation, 1),
            summary_message_id(&ConversationId::new("conv-2"), 1)
        );
    }

    /// The checkpoint round-trips with an explicit boundary shape.
    #[test]
    fn checkpoint_round_trips() {
        let conversation = ConversationId::new("conv-1");
        let checkpoint = checkpoint(&conversation, 3);
        let json = serde_json::to_string(&checkpoint).expect("serialize");
        let decoded: ContextCheckpoint = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, checkpoint);
        assert!(matches!(
            decoded.boundary,
            ContextBoundary::AfterMessage { .. }
        ));
        let inside = ContextCheckpoint {
            boundary: ContextBoundary::InsideAgent {
                message_id: MessageId::new("msg-a2"),
                first_retained_block: ContentBlockIndex::new(2),
            },
            ..checkpoint
        };
        let json = serde_json::to_string(&inside).expect("serialize");
        let decoded: ContextCheckpoint = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, inside);
    }
}
