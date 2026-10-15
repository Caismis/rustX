//! Immutable canonical conversation facts and their bounded hot working set.
//!
//! The Message Ledger owns committed messages. Surface bookkeeping records only
//! ordinary canonical appends; it does not select compacted model history.
//! Durable typed Projection Checkpoints and their activation timeline own
//! historical inclusion and ordering. The Context Engine compiles a checkpoint
//! plus eligible Tail, while the Agent Loop adds and freezes current core input.
//!
//! There is one mutable hot `ConversationState`: the coordinator owns it between
//! attempts and `AgentExecution` owns it while running. It caches retained Original
//! facts, never gives Derived projected content a canonical `MessageId`.

pub mod ledger;
pub mod surface;

pub use ledger::{LedgerAccess, LedgerError, MessageLedger, message_id_of};
pub use surface::{
    ConversationSurface, ConversationSurfaceHead, SurfaceAccess, SurfaceError, SurfaceOp,
    SurfaceRevision, apply_surface_op,
};

use std::collections::BTreeSet;
use std::sync::Arc;

use crate::message::types::{AssistantContentBlock, MessageBlock};
use crate::runtime::identity::{MessageId, ToolCallId};

/// A conversation-state contract violation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConversationError {
    /// The Message Ledger rejected the commit.
    Ledger(LedgerError),
    /// The Conversation Surface rejected the mutation.
    Surface(SurfaceError),
    /// A Surface identity has no committed Ledger record. This is an
    /// impossible state by construction; it is reported rather than
    /// silently repaired.
    DanglingSurfaceIdentity(MessageId),
}

impl core::fmt::Display for ConversationError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Ledger(error) => write!(f, "{error}"),
            Self::Surface(error) => write!(f, "{error}"),
            Self::DanglingSurfaceIdentity(id) => {
                write!(f, "surface message {id} has no committed ledger record")
            }
        }
    }
}

impl std::error::Error for ConversationError {}

impl From<LedgerError> for ConversationError {
    fn from(error: LedgerError) -> Self {
        Self::Ledger(error)
    }
}

impl From<SurfaceError> for ConversationError {
    fn from(error: SurfaceError) -> Self {
        Self::Surface(error)
    }
}

/// A validated, not-yet-installed canonical commit.
///
/// Produced by [`ConversationState::prepare_commit`] after every fallible
/// condition (duplicate Ledger identity, already-active Surface identity) has
/// been checked against the current state; consumed by
/// [`ConversationState::install_prepared`], which is infallible under
/// exclusive ownership. The exact message that was validated is carried
/// inside the value, so a caller can never substitute a different message at
/// install time.
#[derive(Debug, Clone, PartialEq)]
pub struct PreparedCanonicalCommit {
    /// The validated message identity.
    id: MessageId,
    /// The exact message that was validated.
    message: MessageBlock,
}

impl PreparedCanonicalCommit {
    /// The exact message this commit validated.
    #[must_use]
    pub fn message(&self) -> &MessageBlock {
        &self.message
    }

    /// The validated message identity.
    #[must_use]
    pub fn message_id(&self) -> &MessageId {
        &self.id
    }
}

/// A canonical physical Tool call lacking its committed result.
///
/// This pure predicate accepts canonical execution facts only. Runtime admission
/// and recovery use the existing durable occurrence index, independently of
/// historical projection selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoverySafetyError {
    /// An Assistant message issued a tool call whose `ToolResult` sibling is
    /// not yet committed: resuming admission here would let asynchronous
    /// inbound cross the incomplete tool-call/result structure.
    IncompleteToolTurn {
        /// The tool call without a committed result.
        tool_call_id: ToolCallId,
    },
}

impl core::fmt::Display for RecoverySafetyError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::IncompleteToolTurn { tool_call_id } => write!(
                f,
                "the durable canonical prefix ends inside an incomplete tool turn: tool call {tool_call_id} has no committed ToolResult"
            ),
        }
    }
}

impl std::error::Error for RecoverySafetyError {}

/// The first committed tool call with no committed `ToolResult` sibling.
///
/// This is the live-admission half of the recovery gate: an active Surface
/// (or a reconstructed prefix) that ends inside an incomplete tool turn must
/// not admit asynchronous inbound, which would cross the tool-call/result
/// structure.
#[must_use]
///
/// # Panics
///
/// Panics if an Assistant contains more blocks than `ContentBlockIndex` can represent.
pub fn pending_tool_call(messages: &[MessageBlock]) -> Option<ToolCallId> {
    let results: BTreeSet<_> = messages
        .iter()
        .filter_map(|message| match message {
            MessageBlock::Tool(tool) => Some(&tool.occurrence),
            _ => None,
        })
        .collect();
    for message in messages {
        if let MessageBlock::Assistant(assistant) = message {
            for (index, block) in assistant.content.iter().enumerate() {
                if let AssistantContentBlock::ToolCall(call) = block {
                    let occurrence = crate::message::types::ToolCallOccurrenceRef::new(
                        assistant.id.clone(),
                        crate::message::types::ContentBlockIndex::new(
                            u32::try_from(index).expect("canonical block index"),
                        ),
                    );
                    if !results.contains(&occurrence) {
                        return Some(call.id.clone());
                    }
                }
            }
        }
    }
    None
}

/// Whether a complete canonical working set may be resumed without crossing an
/// incomplete tool turn. Historical compaction state is validated and
/// reconstructed by the durable store before this predicate is called; it is
/// no longer a reason to reject a restart.
///
/// # Errors
///
/// Returns [`RecoverySafetyError::IncompleteToolTurn`] for an `Assistant` tool
/// call without its committed `ToolResult` sibling.
pub fn recovery_safety(messages: &[MessageBlock]) -> Result<(), RecoverySafetyError> {
    if let Some(tool_call_id) = pending_tool_call(messages) {
        return Err(RecoverySafetyError::IncompleteToolTurn { tool_call_id });
    }
    Ok(())
}

/// One immutable active Surface snapshot and its keyed canonical bodies.
///
/// `ConversationState::freeze_active_surface` obtains the Surface head once,
/// then hydrates exactly those active identities from the Message Ledger. It
/// is a transient read value, never a second mutable visibility authority.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ConversationSurfaceSnapshot {
    /// The exact Surface revision represented by this snapshot.
    pub revision: SurfaceRevision,
    /// The compaction generation represented by this snapshot.
    pub compaction_generation: u64,
    /// Canonical working-set identities in cache order.
    pub active_message_ids: Vec<MessageId>,
    /// Canonical message bodies keyed by the identities above, in the same
    /// order.
    pub messages: Vec<MessageBlock>,
}

/// The one canonical conversation state of a conversation.
///
/// This is the single mutable conversation authority. The Runtime Client is
/// a read model over it and never mutates it.
///
/// ```compile_fail
/// use rustx::conversation::ConversationState;
///
/// let state = ConversationState::new();
/// let _competing_authority = state.clone();
/// ```
#[derive(Debug, Default)]
pub struct ConversationState {
    ledger: MessageLedger,
    surface: ConversationSurface,
}

impl PartialEq for ConversationState {
    fn eq(&self, other: &Self) -> bool {
        self.ledger == other.ledger && self.surface == other.surface
    }
}

impl ConversationState {
    /// Creates an empty conversation state at [`SurfaceRevision::INITIAL`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Bootstraps a conversation state from ordered canonical messages.
    ///
    /// Every message is committed through the one ordinary commit path, so
    /// the bootstrapped Surface is exactly the bootstrapped Ledger order.
    ///
    /// # Errors
    ///
    /// Returns the [`ConversationError`] of the first rejected commit (for
    /// example a duplicate `MessageId`).
    pub fn from_messages(
        messages: impl IntoIterator<Item = MessageBlock>,
    ) -> Result<Self, ConversationError> {
        let mut state = Self::new();
        for message in messages {
            state.commit(message)?;
        }
        Ok(state)
    }

    /// Hydrates only the current durable Surface head and its active Ledger
    /// bodies. Retired Ledger facts and historical Surface operations remain
    /// in the durable store and are read on demand.
    ///
    /// # Errors
    ///
    /// Returns an error if a supplied active Surface identity is absent from
    /// the supplied current Ledger working set.
    pub fn from_durable_head(
        messages: impl IntoIterator<Item = MessageBlock>,
        active_ids: Vec<MessageId>,
        revision: SurfaceRevision,
        compaction_generation: u64,
    ) -> Result<Self, ConversationError> {
        let mut ledger = MessageLedger::new();
        for message in messages {
            if ledger
                .get(message.id())
                .is_some_and(|existing| existing == &message)
            {
                continue;
            }
            ledger.append(message)?;
        }
        let mut active_ids = active_ids;
        let mut seen = BTreeSet::new();
        active_ids.retain(|id| seen.insert(id.clone()));
        for id in &active_ids {
            if !ledger.contains(id) {
                return Err(ConversationError::DanglingSurfaceIdentity(id.clone()));
            }
        }
        Ok(Self {
            ledger,
            surface: ConversationSurface::from_current_head(
                active_ids,
                revision,
                compaction_generation,
            ),
        })
    }

    /// The immutable hot Message Ledger read model. Retired durable records
    /// are resolved through the `ConversationStore`.
    #[must_use]
    pub fn ledger(&self) -> &MessageLedger {
        &self.ledger
    }

    /// The Conversation Surface.
    #[must_use]
    pub fn surface(&self) -> &ConversationSurface {
        &self.surface
    }

    /// The current Surface revision.
    #[must_use]
    pub fn revision(&self) -> SurfaceRevision {
        self.surface.revision()
    }

    /// The shared Ledger read instrumentation handle.
    #[must_use]
    pub fn ledger_access(&self) -> &Arc<LedgerAccess> {
        self.ledger.access()
    }

    /// The shared Surface read instrumentation handle.
    #[must_use]
    pub fn surface_access(&self) -> &Arc<SurfaceAccess> {
        self.surface.access()
    }

    /// The current active ordered message identities.
    #[must_use]
    pub fn active_ids(&self) -> &[MessageId] {
        self.surface.active()
    }

    /// The **one** ordinary canonical commit path: append the Ledger fact
    /// and append it to the Surface.
    ///
    /// Independent `ledger.push()` / `surface.push()` call sites do not
    /// exist anywhere else in the runtime.
    ///
    /// # Errors
    ///
    /// Returns [`ConversationError::Ledger`] for a duplicate `MessageId` and
    /// [`ConversationError::Surface`] when the identity is already active.
    pub fn commit(&mut self, message: MessageBlock) -> Result<MessageId, ConversationError> {
        let id = self.validate_commit(&message)?;
        Ok(self.install_prepared(PreparedCanonicalCommit { id, message }))
    }

    fn validate_commit(&self, message: &MessageBlock) -> Result<MessageId, ConversationError> {
        let id = message_id_of(message);
        if self.surface.is_active(&id) {
            return Err(ConversationError::Surface(SurfaceError::AlreadyActive(id)));
        }
        if self.ledger.contains(&id) {
            return Err(ConversationError::Ledger(LedgerError::DuplicateMessageId(
                id,
            )));
        }
        Ok(id)
    }

    /// Validates that `message` can be committed without mutating anything,
    /// producing a typed [`PreparedCanonicalCommit`] that binds the exact
    /// validated message to its install.
    ///
    /// This is the prepare half of the Issue #63 canonical-commit seam: the
    /// caller validates every fallible condition (duplicate Ledger identity,
    /// already-active Surface identity) against the current state, then
    /// commits the exact same message durably, and only then installs it with
    /// [`ConversationState::install_prepared`], which is infallible under
    /// exclusive ownership.
    ///
    /// # Errors
    ///
    /// Returns [`ConversationError::Ledger`] for a duplicate `MessageId` and
    /// [`ConversationError::Surface`] when the identity is already active.
    pub fn prepare_commit(
        &self,
        message: &MessageBlock,
    ) -> Result<PreparedCanonicalCommit, ConversationError> {
        let id = self.validate_commit(message)?;
        Ok(PreparedCanonicalCommit {
            id,
            message: message.clone(),
        })
    }

    /// Installs a commit whose exact message was already validated by
    /// [`ConversationState::prepare_commit`].
    ///
    /// Infallible: the caller validated against this exact state and holds
    /// exclusive ownership, so neither the Ledger nor the Surface can reject
    /// the same message again. The installed message is the exact one carried
    /// inside the prepared value — no substitution is representable.
    pub(crate) fn install_prepared(&mut self, prepared: PreparedCanonicalCommit) -> MessageId {
        let id = prepared.id.clone();
        debug_assert!(!self.surface.is_active(&id));
        debug_assert!(!self.ledger.contains(&id));
        self.ledger.append_after_validation(prepared.message);
        self.surface.append_after_validation(id.clone());
        id
    }

    /// Hydrates the current Surface: the finite active messages in active
    /// order.
    ///
    /// This performs exactly one keyed Ledger lookup per active identity and
    /// never enumerates the Ledger, so its cost is a function of the active
    /// Surface size alone — never of retired history.
    ///
    /// # Errors
    ///
    /// Returns [`ConversationError::DanglingSurfaceIdentity`] when a Surface
    /// identity has no committed record.
    pub fn active_messages(&self) -> Result<Vec<MessageBlock>, ConversationError> {
        self.hydrate(self.surface.active())
    }

    /// Freezes the current canonical working set and hydrates its bodies.
    ///
    /// The Surface head is read once. Bodies are then resolved only through
    /// keyed Ledger lookups for the captured active identities; the complete
    /// Ledger is never enumerated by this path.
    ///
    /// # Errors
    ///
    /// Returns an error if an active Surface identity cannot be hydrated from
    /// the canonical Message Ledger.
    pub fn freeze_active_surface(&self) -> Result<ConversationSurfaceSnapshot, ConversationError> {
        let head = self.surface.head();
        let messages = self.hydrate(&head.active_message_ids)?;
        Ok(ConversationSurfaceSnapshot {
            revision: head.revision,
            compaction_generation: head.compaction_generation,
            active_message_ids: head.active_message_ids,
            messages,
        })
    }

    /// Hydrates an explicit ordered identity list through keyed lookups.
    ///
    /// # Errors
    ///
    /// Returns [`ConversationError::DanglingSurfaceIdentity`] for an
    /// identity with no committed record.
    pub fn hydrate(&self, ids: &[MessageId]) -> Result<Vec<MessageBlock>, ConversationError> {
        ids.iter()
            .map(|id| {
                self.ledger
                    .get(id)
                    .cloned()
                    .ok_or_else(|| ConversationError::DanglingSurfaceIdentity(id.clone()))
            })
            .collect()
    }

    /// Reconstructs the exact active ordered identities of a hot or
    /// pre-bootstrap Surface revision. After durable restart, revisions before
    /// the hydrated hot base are intentionally resolved by the
    /// `ConversationStore` read path instead.
    ///
    /// Identity and order come from Surface history alone; only afterwards
    /// may a caller resolve bodies with [`ConversationState::hydrate`].
    ///
    /// # Errors
    ///
    /// Returns [`ConversationError::Surface`] for a revision beyond this
    /// Surface's history.
    pub fn reconstruct(
        &self,
        revision: SurfaceRevision,
    ) -> Result<Vec<MessageId>, ConversationError> {
        Ok(self.surface.reconstruct(revision)?)
    }

    /// Reconstructs the exact canonical messages of a hot Surface revision.
    /// The durable store is the historical read path for revisions older than
    /// a bounded bootstrap. Surface history supplies identities and order
    /// first; the hot Ledger is then queried only for those identities.
    ///
    /// # Errors
    ///
    /// Returns a conversation error when the revision is unavailable or a
    /// referenced message cannot be hydrated.
    pub fn reconstruct_messages(
        &self,
        revision: SurfaceRevision,
    ) -> Result<Vec<MessageBlock>, ConversationError> {
        let ids = self.reconstruct(revision)?;
        self.hydrate(&ids)
    }

    /// Allocates a core-owned context `MessageId`. Transient contributors do
    /// not receive this allocator and therefore cannot choose canonical ids.
    #[must_use]
    pub fn allocate_context_message_id(&self, namespace: &str) -> MessageId {
        // After durable bootstrap the hot Ledger contains only active bodies,
        // so its length may be smaller than the current Surface revision.
        // Revision is monotonic across retired facts and therefore provides
        // a collision-resistant lower bound without retaining the retired
        // Ledger as a second in-memory authority.
        let mut serial = self.surface.revision().get().max(self.ledger.len() as u64);
        loop {
            let candidate = MessageId::new(format!("rustx-context-{namespace}-{serial}"));
            if !self.ledger.contains(&candidate) {
                return candidate;
            }
            serial = serial.saturating_add(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::TextBlock;
    use crate::message::types::{InboundKind, UserContentBlock, UserMessageBlock, UserSource};
    fn user(id: &str, text: &str) -> MessageBlock {
        MessageBlock::User(UserMessageBlock {
            id: MessageId::new(id),
            content: vec![UserContentBlock::Text(TextBlock { text: text.into() })],
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: None,
        })
    }
    #[test]
    fn ordinary_commit_appends_immutable_canonical_fact_once() {
        let mut state = ConversationState::new();
        let message = user("a", "A");
        state.commit(message.clone()).unwrap();
        assert_eq!(
            state.active_messages().unwrap(),
            std::slice::from_ref(&message)
        );
        assert_eq!(state.ledger().get(message.id()), Some(&message));
        assert_eq!(state.revision(), SurfaceRevision::new(1));
        assert!(state.commit(message.clone()).is_err());
        assert_eq!(state.active_messages().unwrap(), [message]);
    }
    #[test]
    fn equal_content_preserves_distinct_canonical_identities() {
        let state =
            ConversationState::from_messages([user("a", "same"), user("b", "same")]).unwrap();
        assert_eq!(
            state.active_ids(),
            [MessageId::new("a"), MessageId::new("b")]
        );
        assert_eq!(
            state.reconstruct(SurfaceRevision::new(1)).unwrap(),
            [MessageId::new("a")]
        );
    }
    #[test]
    fn finite_hot_hydration_uses_only_keyed_canonical_reads() {
        let state = ConversationState::from_messages([user("a", "A"), user("b", "B")]).unwrap();
        state.ledger_access().reset();
        let snapshot = state.freeze_active_surface().unwrap();
        assert_eq!(snapshot.messages.len(), 2);
        assert_eq!(state.ledger_access().enumerations(), 0);
        assert_eq!(state.ledger_access().keyed_reads(), 2);
    }
    #[test]
    fn durable_working_set_keeps_global_commit_frontier_without_historical_hydration() {
        let message = user("retained", "visible");
        let mut state = ConversationState::from_durable_head(
            [message.clone()],
            vec![message.id().clone()],
            SurfaceRevision::new(100),
            2,
        )
        .unwrap();
        let allocated = state.allocate_context_message_id("test");
        assert!(allocated.as_str().ends_with("100"));
        state.commit(user("next", "new")).unwrap();
        assert_eq!(state.revision(), SurfaceRevision::new(101));
        assert_eq!(state.active_messages().unwrap().len(), 2);
    }
}
