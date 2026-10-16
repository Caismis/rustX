//! Bounded canonical-message working-set cache and ordinary append history.
//! Historical model selection belongs to durable Projection Checkpoints and the
//! Context compiler. This cache cannot replace, retire or synthesize messages.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use super::selection::CanonicalSelection;
use crate::message::types::MessageBlock;
use crate::runtime::identity::MessageId;

/// The identity of one exact historical Conversation Surface state.
///
/// A revision is a monotonic counter in its own identity domain. The empty
/// Surface of a new conversation is [`SurfaceRevision::INITIAL`] (`0`), and
/// every accepted [`SurfaceOp`] advances it by exactly one, so revision `n`
/// is precisely "the Surface after the first `n` accepted operations".
///
/// A revision is deliberately **not** a `MessageId`, an `AttemptId`, a
/// `RuntimeClientCursor`, an `InboundSequence`, an Event Journal sequence,
/// or a `CapabilityRevision`: none of those identify a Surface state, and
/// none of them may be substituted for one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
#[derive(schemars::JsonSchema)]
pub struct SurfaceRevision(u64);

impl SurfaceRevision {
    /// The revision of an empty Conversation Surface.
    pub const INITIAL: Self = Self(0);

    /// Creates a revision from a raw counter value.
    #[must_use]
    pub const fn new(revision: u64) -> Self {
        Self(revision)
    }

    /// The raw counter value.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    /// The deterministic next revision.
    ///
    /// # Panics
    ///
    /// Panics only when the revision domain is exhausted, which is
    /// unreachable for an in-process conversation.
    #[must_use]
    pub const fn next(self) -> Self {
        match self.0.checked_add(1) {
            Some(next) => Self(next),
            None => panic!("the surface revision domain is exhausted"),
        }
    }
}

impl Default for SurfaceRevision {
    fn default() -> Self {
        Self::INITIAL
    }
}

impl core::fmt::Display for SurfaceRevision {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The complete mutation vocabulary of the Conversation Surface.
///
/// There is intentionally no insert, move, delete, reorder, or patch
/// operation: this view records ordinary canonical append membership only.
/// Compaction selection lives in typed checkpoints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum SurfaceOp {
    /// Appends one canonical message at the end of the active order.
    Append {
        /// The appended message identity.
        message_id: MessageId,
    },
}

/// One immutable read of the current Conversation Surface head.
///
/// The snapshot captures identity/order and head metadata together. Callers
/// that need canonical bodies can capture the shared immutable selection;
/// the Ledger retains canonical ownership of those allocations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationSurfaceHead {
    /// The exact Surface revision represented by this head.
    pub revision: SurfaceRevision,
    /// The compaction generation represented by this head.
    pub compaction_generation: u64,
    /// Canonical message identities in working-set cache order.
    pub active_message_ids: Vec<MessageId>,
}

impl SurfaceOp {
    /// Every canonical identity this operation names, in a stable order.
    ///
    /// Lineage replay resolves each append against its immutable Ledger dependency.
    #[must_use]
    pub fn message_ids(&self) -> Vec<&MessageId> {
        match self {
            Self::Append { message_id } => vec![message_id],
        }
    }

    /// The one canonical identity this operation brings onto the Surface.
    ///
    /// Each append introduces exactly the message it names.
    ///
    /// This is the pairing between the two durable orders. A canonical row
    /// and the operation that introduces it are committed in the same
    /// transaction, so a lineage the runtime can reach names each canonical
    /// identity here exactly once, in Ledger order.
    #[must_use]
    pub const fn introduces(&self) -> &MessageId {
        match self {
            Self::Append { message_id } => message_id,
        }
    }
}

/// Applies one Surface operation to an active identity order.
///
/// This is the single definition of what an operation *means*. Every reader
/// of a retained operation log — durable reconstruction, seed validation, and
/// the Session lineage cut — replays through here, so no two of them can
/// disagree about the Surface a history denotes.
///
/// # Errors
///
/// Returns the reason the operation does not apply to `active`. The caller
/// wraps it in its own error domain.
pub fn apply_surface_op(active: &mut Vec<MessageId>, operation: &SurfaceOp) -> Result<(), String> {
    match operation {
        SurfaceOp::Append { message_id } => {
            if active.contains(message_id) {
                return Err(format!(
                    "Surface Append repeats active message {message_id}"
                ));
            }
            active.push(message_id.clone());
        }
    }
    Ok(())
}

/// An ordinary canonical-cache append or historical-read violation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SurfaceError {
    AlreadyActive(MessageId),
    UnknownRevision(SurfaceRevision),
}
impl core::fmt::Display for SurfaceError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::AlreadyActive(id) => {
                write!(f, "message {id} is already in the canonical working set")
            }
            Self::UnknownRevision(revision) => {
                write!(f, "canonical cache revision {revision} is not retained")
            }
        }
    }
}
impl std::error::Error for SurfaceError {}

/// Deterministic instrumentation for Conversation Surface reads.
///
/// Normal projection, planning, and compaction use only the current head:
/// active identities and O(1) head metadata. Historical operation reads are
/// counted separately so tests can prove that retired Surface history is not
/// part of the normal cost.
#[derive(Debug, Default)]
pub struct SurfaceAccess {
    current_head_reads: AtomicU64,
    history_enumerations: AtomicU64,
    history_steps: AtomicU64,
}

impl SurfaceAccess {
    /// The number of current-head reads.
    #[must_use]
    pub fn current_head_reads(&self) -> u64 {
        self.current_head_reads.load(Ordering::Relaxed)
    }

    /// The number of explicit historical operation-log reads.
    #[must_use]
    pub fn history_enumerations(&self) -> u64 {
        self.history_enumerations.load(Ordering::Relaxed)
    }

    /// The number of historical operations visited by diagnostic reads.
    #[must_use]
    pub fn history_steps(&self) -> u64 {
        self.history_steps.load(Ordering::Relaxed)
    }

    /// Resets all counters. Test/diagnostic use only.
    pub fn reset(&self) {
        self.current_head_reads.store(0, Ordering::Relaxed);
        self.history_enumerations.store(0, Ordering::Relaxed);
        self.history_steps.store(0, Ordering::Relaxed);
    }

    fn current_head_read(&self) {
        self.current_head_reads.fetch_add(1, Ordering::Relaxed);
    }

    fn history_read(&self, steps: usize) {
        self.history_enumerations.fetch_add(1, Ordering::Relaxed);
        self.history_steps
            .fetch_add(steps as u64, Ordering::Relaxed);
    }
}

/// The bounded canonical-message working set of one conversation.
///
/// This cache owns no model projection; typed checkpoint compilation controls
/// historical inclusion, exclusion and occurrence order.
#[derive(Debug, Default)]
pub struct ConversationSurface {
    /// The current canonical selection sharing Ledger allocations.
    active: CanonicalSelection,
    /// The current revision, maintained as head metadata.
    revision: SurfaceRevision,
    /// The first revision represented by the bounded hot operation suffix.
    /// Revisions before this base remain durable-store reads after restart.
    history_base_revision: SurfaceRevision,
    /// The active identity order at `history_base_revision`.
    history_base_active: Vec<MessageId>,
    /// The installed durable projection activation, cached as head metadata.
    compaction_generation: u64,
    /// The bounded operation suffix accepted after the hot bootstrap base.
    /// The durable `ConversationStore` owns the prefix before
    /// `history_base_revision`.
    ops: Vec<SurfaceOp>,
    /// Read instrumentation for current-head versus historical access.
    access: Arc<SurfaceAccess>,
}

impl PartialEq for ConversationSurface {
    fn eq(&self, other: &Self) -> bool {
        self.active == other.active
            && self.revision == other.revision
            && self.history_base_revision == other.history_base_revision
            && self.history_base_active == other.history_base_active
            && self.compaction_generation == other.compaction_generation
            && self.ops == other.ops
    }
}

impl ConversationSurface {
    /// Hydrates the current durable head without materializing historical
    /// operations. The operation log remains a store concern; this value is
    /// only the bounded hot projection used by the runtime.
    #[must_use]
    pub fn from_current_head(
        active: CanonicalSelection,
        revision: SurfaceRevision,
        compaction_generation: u64,
    ) -> Self {
        Self {
            history_base_active: active.ids(),
            active,
            revision,
            history_base_revision: revision,
            compaction_generation,
            ops: Vec::new(),
            access: Arc::new(SurfaceAccess::default()),
        }
    }

    fn mark_current_head_read(&self) {
        self.access.current_head_read();
    }
}

impl ConversationSurface {
    /// Creates an empty Surface at [`SurfaceRevision::INITIAL`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The current revision.
    #[must_use]
    pub fn revision(&self) -> SurfaceRevision {
        self.mark_current_head_read();
        self.revision
    }

    /// The shared read instrumentation handle.
    #[must_use]
    pub fn access(&self) -> &Arc<SurfaceAccess> {
        &self.access
    }

    /// Captures the current identity/order and head metadata in one read.
    #[must_use]
    pub fn head(&self) -> ConversationSurfaceHead {
        self.mark_current_head_read();
        ConversationSurfaceHead {
            revision: self.revision,
            compaction_generation: self.compaction_generation,
            active_message_ids: self.active.ids(),
        }
    }

    /// The current active ordered message identities.
    #[must_use]
    pub fn active(&self) -> Vec<MessageId> {
        self.mark_current_head_read();
        self.active.ids()
    }

    /// Share the exact canonical selection without revisiting old bodies.
    #[must_use]
    pub fn selection(&self) -> CanonicalSelection {
        self.mark_current_head_read();
        self.active.clone()
    }

    /// The number of active messages.
    #[must_use]
    pub fn len(&self) -> usize {
        self.active.len()
    }

    /// Whether the Surface is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.active.is_empty()
    }

    /// The active position of one message identity, when it is active.
    #[must_use]
    pub fn position_of(&self, message_id: &MessageId) -> Option<usize> {
        self.mark_current_head_read();
        self.active.position(message_id)
    }

    /// Whether the identity is currently active.
    #[must_use]
    pub fn is_active(&self, message_id: &MessageId) -> bool {
        self.position_of(message_id).is_some()
    }

    /// Projection activation metadata carried by the bounded working-set cache.
    /// It is supplied by the durable compiler and is never incremented by canonical appends.
    #[must_use]
    pub fn compaction_generation(&self) -> u64 {
        self.mark_current_head_read();
        self.compaction_generation
    }

    /// Appends one message identity to the active order.
    ///
    /// # Errors
    ///
    /// Returns [`SurfaceError::AlreadyActive`] when the identity is already
    /// active; an identity is never active twice.
    pub fn append(&mut self, message: Arc<MessageBlock>) -> Result<SurfaceRevision, SurfaceError> {
        let message_id = message.id().clone();
        if self.is_active(&message_id) {
            return Err(SurfaceError::AlreadyActive(message_id));
        }
        Ok(self.append_after_validation(message))
    }

    /// Applies an append after the caller has validated the current Surface
    /// under exclusive ownership. This is infallible so an ordinary Ledger
    /// append cannot be followed by a recoverable Surface error.
    pub(crate) fn append_after_validation(
        &mut self,
        message: Arc<MessageBlock>,
    ) -> SurfaceRevision {
        let message_id = message.id().clone();
        debug_assert!(!self.is_active(&message_id));
        self.active.append(message);
        self.ops.push(SurfaceOp::Append { message_id });
        self.revision = self.revision.next();
        self.revision
    }

    /// Reconstructs the exact active ordered identities of a retained hot
    /// revision. Revisions before a durable bootstrap base are read through
    /// the `ConversationStore` rather than materialized here.
    ///
    /// Reconstruction replays the bounded operation suffix only: it never
    /// reads the Message Ledger, and later mutations never change the
    /// reconstruction of an earlier retained revision.
    ///
    /// # Errors
    ///
    /// Returns [`SurfaceError::UnknownRevision`] for a revision beyond this
    /// Surface's history.
    pub fn reconstruct(&self, revision: SurfaceRevision) -> Result<Vec<MessageId>, SurfaceError> {
        if revision < self.history_base_revision || revision > self.revision {
            return Err(SurfaceError::UnknownRevision(revision));
        }
        let offset = revision
            .get()
            .checked_sub(self.history_base_revision.get())
            .ok_or(SurfaceError::UnknownRevision(revision))?;
        let upto = usize::try_from(offset).map_err(|_| SurfaceError::UnknownRevision(revision))?;
        if upto > self.ops.len() {
            return Err(SurfaceError::UnknownRevision(revision));
        }
        self.access.history_read(upto);
        let mut active = self.history_base_active.clone();
        for op in &self.ops[..upto] {
            match op {
                SurfaceOp::Append { message_id } => active.push(message_id.clone()),
            }
        }
        Ok(active)
    }

    /// The bounded accepted operation suffix, in acceptance order.
    #[must_use]
    pub fn ops(&self) -> &[SurfaceOp] {
        self.access.history_read(self.ops.len());
        &self.ops
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn id(s: &str) -> MessageId {
        MessageId::new(s)
    }
    fn body(s: &str) -> Arc<MessageBlock> {
        use crate::message::types::{InboundKind, UserMessageBlock, UserSource};
        Arc::new(MessageBlock::User(UserMessageBlock {
            id: id(s),
            content: Vec::new(),
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: None,
        }))
    }
    fn selected(s: &str) -> CanonicalSelection {
        let mut result = CanonicalSelection::default();
        result.append(body(s));
        result
    }
    #[test]
    fn append_order_and_contiguous_revision_are_exact() {
        let mut cache = ConversationSurface::new();
        assert_eq!(cache.revision(), SurfaceRevision::INITIAL);
        for (i, s) in ["a", "b", "c"].into_iter().enumerate() {
            assert_eq!(
                cache.append(body(s)).unwrap(),
                SurfaceRevision::new(i as u64 + 1)
            );
        }
        assert_eq!(cache.active(), [id("a"), id("b"), id("c")]);
        assert_eq!(
            cache
                .ops()
                .iter()
                .map(SurfaceOp::introduces)
                .cloned()
                .collect::<Vec<_>>(),
            cache.active()
        );
    }
    #[test]
    fn rejected_duplicate_never_advances_cache_or_history() {
        let mut cache = ConversationSurface::new();
        cache.append(body("a")).unwrap();
        let head = cache.head();
        assert_eq!(
            cache.append(body("a")),
            Err(SurfaceError::AlreadyActive(id("a")))
        );
        assert_eq!(cache.head(), head);
        assert_eq!(cache.ops().len(), 1);
    }
    #[test]
    fn append_reconstruction_preserves_earlier_canonical_prefix() {
        let mut cache = ConversationSurface::new();
        cache.append(body("a")).unwrap();
        let before = cache.revision();
        cache.append(body("b")).unwrap();
        assert_eq!(cache.reconstruct(before).unwrap(), [id("a")]);
        assert_eq!(
            cache.reconstruct(SurfaceRevision::INITIAL).unwrap(),
            Vec::<MessageId>::new()
        );
        assert!(cache.reconstruct(SurfaceRevision::new(3)).is_err());
    }
    #[test]
    fn hydrated_cache_reads_only_retained_hot_append_suffix() {
        let mut cache = ConversationSurface::from_current_head(
            selected("retained"),
            SurfaceRevision::new(100),
            7,
        );
        cache.append(body("new")).unwrap();
        assert_eq!(
            cache.reconstruct(SurfaceRevision::new(100)).unwrap(),
            [id("retained")]
        );
        assert_eq!(
            cache.reconstruct(SurfaceRevision::new(101)).unwrap(),
            [id("retained"), id("new")]
        );
        assert!(cache.reconstruct(SurfaceRevision::new(99)).is_err());
        cache.access().reset();
        let _ = cache.head();
        assert_eq!(cache.access().history_enumerations(), 0);
    }
}
