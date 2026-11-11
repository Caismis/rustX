//! The coordinator's ownership slot for the canonical conversation state.
//!
//! A reopened conversation does not hydrate its active working set at
//! construction or attach: the state stays in the durable store until the
//! first coordinator operation that needs it (inbound adoption, a recovered
//! continuation, manual compaction, or an explicit inspection), which loads
//! it once from the durable Surface head. From then on ownership moves
//! between the coordinator and its attempt or maintenance task exactly as
//! before.

use super::ConversationRuntimeError;
use crate::conversation::ConversationState;
use crate::durable::ConversationStore;

// One long-lived slot per runtime, like the `Option` it replaces: boxing the
// owned state would only add an allocation to every ownership transfer.
#[allow(clippy::large_enum_variant)]
pub(super) enum ConversationSlot {
    /// Not hydrated since construction; the durable head is the authority.
    Durable,
    /// Owned by the coordinator between attempts.
    Owned(ConversationState),
    /// Moved into the running attempt or maintenance task.
    Lent,
}

impl ConversationSlot {
    /// Whether an attempt or maintenance task owns the state.
    pub(super) fn is_lent(&self) -> bool {
        matches!(self, Self::Lent)
    }

    /// The coordinator-owned state, hydrating it from the durable head the
    /// first time it is needed. `None` while the state is lent.
    pub(super) fn hydrate(
        &mut self,
        store: &dyn ConversationStore,
    ) -> Result<Option<&mut ConversationState>, ConversationRuntimeError> {
        if matches!(self, Self::Durable) {
            *self = Self::Owned(super::ConversationRuntime::recovered_conversation(store)?);
        }
        Ok(match self {
            Self::Owned(conversation) => Some(conversation),
            Self::Durable | Self::Lent => None,
        })
    }

    /// The coordinator-owned state once hydrated.
    pub(super) fn owned(&self) -> Option<&ConversationState> {
        match self {
            Self::Owned(conversation) => Some(conversation),
            Self::Durable | Self::Lent => None,
        }
    }

    /// Moves the hydrated, coordinator-owned state out to its next owner.
    pub(super) fn lend(&mut self) -> Option<ConversationState> {
        match std::mem::replace(self, Self::Lent) {
            Self::Owned(conversation) => Some(conversation),
            previous => {
                *self = previous;
                None
            }
        }
    }

    /// Returns the authoritative state from its settled owner.
    pub(super) fn restore(&mut self, conversation: ConversationState) {
        *self = Self::Owned(conversation);
    }
}
