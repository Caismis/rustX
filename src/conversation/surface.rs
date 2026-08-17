//! The Conversation Surface: the sole authority for current active
//! model-visible message identity, order, and visibility.
//!
//! The Surface holds only identities, never message bodies: bodies live in
//! the [`MessageLedger`](crate::conversation::ledger::MessageLedger) and are
//! resolved by keyed lookup after the Surface has answered *which* messages
//! are active and in *what* order.
//!
//! The mutation vocabulary is deliberately minimal — there is no generic
//! insert/move/delete/reorder/patch operation:
//!
//! ```text
//! Append  { message_id }
//! Replace { start, end, replacement }
//! ```
//!
//! Every accepted mutation produces the deterministic next
//! [`SurfaceRevision`]. Revisions form their own identity domain, distinct
//! from `MessageId`, `AttemptId`, `RuntimeClientCursor`, `InboundSequence`,
//! the Event Journal sequence, and `CapabilityRevision`. The ordered
//! operation log is retained so any historical revision reconstructs its
//! exact active ordered `MessageId` list **without touching the Ledger**.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

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
/// operation: an ordinary commit appends, and compaction replaces one
/// structurally valid span with one canonical summary message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum SurfaceOp {
    /// Appends one canonical message at the end of the active order.
    Append {
        /// The appended message identity.
        message_id: MessageId,
    },
    /// Replaces the **inclusive** active span `[start ..= end]` with one
    /// canonical replacement message, at the position `start` occupied.
    ///
    /// Both endpoints are part of the replaced span; `start == end` replaces
    /// exactly one message.
    Replace {
        /// The first replaced active message, inclusive.
        start: MessageId,
        /// The last replaced active message, inclusive.
        end: MessageId,
        /// The canonical replacement message.
        replacement: MessageId,
    },
}

/// One inclusive active span of the Conversation Surface.
///
/// The convention is frozen and tested: `[start ..= end]`, both endpoints
/// included, `start == end` selecting exactly one message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceSpan {
    /// The first message of the span, inclusive.
    pub start: MessageId,
    /// The last message of the span, inclusive.
    pub end: MessageId,
}

impl SurfaceSpan {
    /// Creates an inclusive span.
    #[must_use]
    pub const fn new(start: MessageId, end: MessageId) -> Self {
        Self { start, end }
    }
}

/// A Conversation Surface contract violation.
///
/// Every variant is a rejected mutation: the Surface is never left partly
/// mutated, because validation completes before any state changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SurfaceError {
    /// The appended (or replacement) identity is already active.
    AlreadyActive(MessageId),
    /// The span endpoint is not an active message of the current Surface.
    NotActive(MessageId),
    /// The span endpoints are reversed: `end` precedes `start`.
    ReversedSpan {
        /// The requested first endpoint.
        start: MessageId,
        /// The requested last endpoint, which precedes `start`.
        end: MessageId,
    },
    /// The requested revision does not exist in this Surface's history.
    UnknownRevision(SurfaceRevision),
    /// The operation was validated against a Surface revision that is no
    /// longer current.
    StaleRevision {
        /// The revision the caller validated against.
        expected: SurfaceRevision,
        /// The Surface's actual current revision.
        actual: SurfaceRevision,
    },
}

impl core::fmt::Display for SurfaceError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::AlreadyActive(id) => {
                write!(f, "message {id} is already active on the surface")
            }
            Self::NotActive(id) => write!(f, "message {id} is not active on the surface"),
            Self::ReversedSpan { start, end } => write!(
                f,
                "surface span [{start} ..= {end}] is reversed: {end} precedes {start}"
            ),
            Self::UnknownRevision(revision) => {
                write!(f, "surface revision {revision} does not exist")
            }
            Self::StaleRevision { expected, actual } => write!(
                f,
                "surface revision {expected} is stale; the current revision is {actual}"
            ),
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

/// The active model-visible order of one conversation.
///
/// The Surface owns identity/order/visibility. It carries no visibility
/// flags on Ledger records (there are none) and no message bodies.
#[derive(Debug, Default)]
pub struct ConversationSurface {
    /// The current active ordered message identities.
    active: Vec<MessageId>,
    /// The current revision, maintained as head metadata.
    revision: SurfaceRevision,
    /// The first revision represented by the bounded hot operation suffix.
    /// Revisions before this base remain durable-store reads after restart.
    history_base_revision: SurfaceRevision,
    /// The active identity order at `history_base_revision`.
    history_base_active: Vec<MessageId>,
    /// The number of accepted replacements, maintained as head metadata.
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

impl Eq for ConversationSurface {}

impl ConversationSurface {
    /// Hydrates the current durable head without materializing historical
    /// operations. The operation log remains a store concern; this value is
    /// only the bounded hot projection used by the runtime.
    #[must_use]
    pub fn from_current_head(
        active: Vec<MessageId>,
        revision: SurfaceRevision,
        compaction_generation: u64,
    ) -> Self {
        Self {
            history_base_active: active.clone(),
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

    /// The current active ordered message identities.
    #[must_use]
    pub fn active(&self) -> &[MessageId] {
        self.mark_current_head_read();
        &self.active
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
        self.active.iter().position(|id| id == message_id)
    }

    /// Whether the identity is currently active.
    #[must_use]
    pub fn is_active(&self, message_id: &MessageId) -> bool {
        self.position_of(message_id).is_some()
    }

    /// The number of accepted [`SurfaceOp::Replace`] operations.
    ///
    /// This is current Surface head metadata. It is updated when a Replace
    /// commits and never derived by scanning historical operations.
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
    pub fn append(&mut self, message_id: MessageId) -> Result<SurfaceRevision, SurfaceError> {
        if self.is_active(&message_id) {
            return Err(SurfaceError::AlreadyActive(message_id));
        }
        Ok(self.append_after_validation(message_id))
    }

    /// Applies an append after the caller has validated the current Surface
    /// under exclusive ownership. This is infallible so an ordinary Ledger
    /// append cannot be followed by a recoverable Surface error.
    pub(crate) fn append_after_validation(&mut self, message_id: MessageId) -> SurfaceRevision {
        debug_assert!(!self.is_active(&message_id));
        self.active.push(message_id.clone());
        self.ops.push(SurfaceOp::Append { message_id });
        self.revision = self.revision.next();
        self.revision
    }

    /// Validates one replacement against the **current** revision without
    /// mutating anything, returning the inclusive active index range the
    /// span resolves to.
    ///
    /// # Errors
    ///
    /// Returns [`SurfaceError::NotActive`] for an unknown or already retired
    /// endpoint, [`SurfaceError::ReversedSpan`] for reversed endpoints, and
    /// [`SurfaceError::AlreadyActive`] for a replacement identity that is
    /// already active.
    pub fn validate_replace(
        &self,
        span: &SurfaceSpan,
        replacement: &MessageId,
    ) -> Result<(usize, usize), SurfaceError> {
        let start = self
            .position_of(&span.start)
            .ok_or_else(|| SurfaceError::NotActive(span.start.clone()))?;
        let end = self
            .position_of(&span.end)
            .ok_or_else(|| SurfaceError::NotActive(span.end.clone()))?;
        if end < start {
            return Err(SurfaceError::ReversedSpan {
                start: span.start.clone(),
                end: span.end.clone(),
            });
        }
        if self.is_active(replacement) {
            return Err(SurfaceError::AlreadyActive(replacement.clone()));
        }
        Ok((start, end))
    }

    /// Replaces the inclusive active span `[span.start ..= span.end]` with
    /// `replacement`, at the position `span.start` occupied.
    ///
    /// Validation completes before any mutation, so a rejected replacement
    /// leaves the Surface exactly as it was.
    ///
    /// # Errors
    ///
    /// Returns the [`SurfaceError`] of the first violation.
    pub fn replace(
        &mut self,
        span: &SurfaceSpan,
        replacement: MessageId,
    ) -> Result<SurfaceRevision, SurfaceError> {
        let (start, end) = self.validate_replace(span, &replacement)?;
        Ok(self.replace_after_validation(span, replacement, start, end))
    }

    /// Applies a replacement after all recoverable validation has completed.
    ///
    /// This is intentionally infallible: `ConversationState` uses it only
    /// after validating the current revision, endpoints, identity, and
    /// active structural index. Once that validation has run under exclusive
    /// ownership, a normal recoverable Surface error cannot occur after a
    /// Ledger append.
    pub(crate) fn replace_after_validation(
        &mut self,
        span: &SurfaceSpan,
        replacement: MessageId,
        start: usize,
        end: usize,
    ) -> SurfaceRevision {
        debug_assert_eq!(self.active.get(start), Some(&span.start));
        debug_assert_eq!(self.active.get(end), Some(&span.end));
        debug_assert!(start <= end);
        debug_assert!(!self.is_active(&replacement));
        self.active.splice(start..=end, [replacement.clone()]);
        self.ops.push(SurfaceOp::Replace {
            start: span.start.clone(),
            end: span.end.clone(),
            replacement,
        });
        self.revision = self.revision.next();
        self.compaction_generation = self
            .compaction_generation
            .checked_add(1)
            .expect("the surface compaction generation cannot overflow");
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
                SurfaceOp::Replace {
                    start,
                    end,
                    replacement,
                } => {
                    let (Some(from), Some(to)) = (
                        active.iter().position(|id| id == start),
                        active.iter().position(|id| id == end),
                    ) else {
                        unreachable!("an accepted replace always resolves during replay");
                    };
                    active.splice(from..=to, [replacement.clone()]);
                }
            }
        }
        Ok(active)
    }

    /// Every identity retired in the bounded hot suffix that is no longer
    /// active. Older retired identities remain a durable-store read.
    ///
    /// This is a diagnostic/audit accessor over Surface history; normal
    /// projection and compaction never call it.
    #[must_use]
    pub fn retired(&self) -> Vec<MessageId> {
        self.access.history_read(self.ops.len());
        let active: BTreeSet<&MessageId> = self.active.iter().collect();
        let mut seen = BTreeSet::new();
        let mut retired = Vec::new();
        for op in &self.ops {
            let candidates = match op {
                SurfaceOp::Append { message_id } => vec![message_id],
                SurfaceOp::Replace {
                    start,
                    end,
                    replacement,
                } => vec![start, end, replacement],
            };
            for id in candidates {
                if !active.contains(id) && seen.insert(id.clone()) {
                    retired.push(id.clone());
                }
            }
        }
        retired
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
    use super::{ConversationSurface, SurfaceError, SurfaceOp, SurfaceRevision, SurfaceSpan};
    use crate::runtime::identity::MessageId;

    fn id(value: &str) -> MessageId {
        MessageId::new(value)
    }

    fn surface(ids: &[&str]) -> ConversationSurface {
        let mut surface = ConversationSurface::new();
        for value in ids {
            surface.append(id(value)).expect("append");
        }
        surface
    }

    /// The empty Surface starts at the initial revision and every accepted
    /// operation advances it by exactly one.
    #[test]
    fn revisions_start_at_initial_and_advance_by_one() {
        let mut surface = ConversationSurface::new();
        assert_eq!(surface.revision(), SurfaceRevision::INITIAL);
        assert_eq!(surface.revision().get(), 0);
        assert_eq!(
            surface.append(id("a")).expect("append"),
            SurfaceRevision::new(1)
        );
        assert_eq!(
            surface.append(id("b")).expect("append"),
            SurfaceRevision::new(2)
        );
        assert_eq!(surface.revision(), SurfaceRevision::new(2));
    }

    /// The span convention is inclusive on both ends.
    #[test]
    fn replace_span_is_inclusive_on_both_ends() {
        let mut surface = surface(&["a", "b", "c", "d"]);
        let revision = surface
            .replace(&SurfaceSpan::new(id("a"), id("c")), id("s1"))
            .expect("replace");
        assert_eq!(revision, SurfaceRevision::new(5));
        assert_eq!(surface.active(), &[id("s1"), id("d")]);
        // start == end replaces exactly one message.
        let mut single = surface_of(&["a", "b", "c"]);
        single
            .replace(&SurfaceSpan::new(id("b"), id("b")), id("s"))
            .expect("replace one");
        assert_eq!(single.active(), &[id("a"), id("s"), id("c")]);
    }

    fn surface_of(ids: &[&str]) -> ConversationSurface {
        surface(ids)
    }

    /// The replacement takes the position the span's first message held.
    #[test]
    fn replacement_takes_the_span_start_position() {
        let mut surface = surface(&["a", "b", "c", "d"]);
        surface
            .replace(&SurfaceSpan::new(id("b"), id("c")), id("s"))
            .expect("replace");
        assert_eq!(surface.active(), &[id("a"), id("s"), id("d")]);
    }

    /// Invalid replacements are rejected and never mutate the Surface.
    #[test]
    fn invalid_replacements_are_rejected_without_mutation() {
        let mut surface = surface(&["a", "b", "c"]);
        let before_active = surface.active().to_vec();
        let before_revision = surface.revision();
        let before_ops = surface.ops().to_vec();

        assert_eq!(
            surface
                .replace(&SurfaceSpan::new(id("ghost"), id("c")), id("s"))
                .expect_err("unknown start"),
            SurfaceError::NotActive(id("ghost"))
        );
        assert_eq!(
            surface
                .replace(&SurfaceSpan::new(id("a"), id("ghost")), id("s"))
                .expect_err("unknown end"),
            SurfaceError::NotActive(id("ghost"))
        );
        assert_eq!(
            surface
                .replace(&SurfaceSpan::new(id("c"), id("a")), id("s"))
                .expect_err("reversed"),
            SurfaceError::ReversedSpan {
                start: id("c"),
                end: id("a"),
            }
        );
        assert_eq!(
            surface
                .replace(&SurfaceSpan::new(id("a"), id("b")), id("c"))
                .expect_err("replacement already active"),
            SurfaceError::AlreadyActive(id("c"))
        );
        assert_eq!(surface.active(), before_active.as_slice());
        assert_eq!(surface.revision(), before_revision);
        assert_eq!(surface.ops(), before_ops.as_slice());
    }

    /// A retired span can never be replaced again.
    #[test]
    fn a_retired_span_is_no_longer_replaceable() {
        let mut surface = surface(&["a", "b", "c", "d"]);
        surface
            .replace(&SurfaceSpan::new(id("a"), id("c")), id("s1"))
            .expect("first replace");
        assert_eq!(
            surface
                .replace(&SurfaceSpan::new(id("a"), id("b")), id("s2"))
                .expect_err("retired span"),
            SurfaceError::NotActive(id("a"))
        );
    }

    /// An identity is never active twice.
    #[test]
    fn append_rejects_an_already_active_identity() {
        let mut surface = surface(&["a"]);
        assert_eq!(
            surface.append(id("a")).expect_err("duplicate"),
            SurfaceError::AlreadyActive(id("a"))
        );
    }

    /// Historical reconstruction is exact and stable under later mutation.
    #[test]
    fn historical_reconstruction_is_exact_and_stable() {
        let mut surface = surface(&["a", "b", "c", "d"]);
        let before = surface.revision();
        assert_eq!(
            surface.reconstruct(before).expect("reconstruct"),
            vec![id("a"), id("b"), id("c"), id("d")]
        );
        surface
            .replace(&SurfaceSpan::new(id("a"), id("c")), id("s1"))
            .expect("replace");
        surface.append(id("e")).expect("append");
        assert_eq!(
            surface.reconstruct(before).expect("reconstruct historical"),
            vec![id("a"), id("b"), id("c"), id("d")],
            "later mutations never change an earlier reconstruction"
        );
        assert_eq!(
            surface
                .reconstruct(surface.revision())
                .expect("reconstruct current"),
            surface.active().to_vec()
        );
        assert_eq!(
            surface
                .reconstruct(SurfaceRevision::INITIAL)
                .expect("empty"),
            Vec::<MessageId>::new()
        );
        let current_active = surface.active().to_vec();
        let current_revision = surface.revision();
        let current_ops = surface.ops().to_vec();
        assert_eq!(
            surface
                .reconstruct(SurfaceRevision::new(99))
                .expect_err("beyond history"),
            SurfaceError::UnknownRevision(SurfaceRevision::new(99))
        );
        assert_eq!(surface.active(), current_active.as_slice());
        assert_eq!(surface.revision(), current_revision);
        assert_eq!(surface.ops(), current_ops.as_slice());
    }

    /// The compaction generation is maintained as current-head metadata.
    #[test]
    fn compaction_generation_counts_replacements() {
        let mut surface = surface(&["a", "b", "c", "d"]);
        assert_eq!(surface.compaction_generation(), 0);
        surface
            .replace(&SurfaceSpan::new(id("a"), id("c")), id("s1"))
            .expect("replace");
        assert_eq!(surface.compaction_generation(), 1);
        surface.append(id("e")).expect("append");
        assert_eq!(surface.compaction_generation(), 1);
        surface
            .replace(&SurfaceSpan::new(id("s1"), id("d")), id("s2"))
            .expect("replace");
        assert_eq!(surface.compaction_generation(), 2);
    }

    /// The operation log records exactly the accepted vocabulary.
    #[test]
    fn operation_log_records_the_minimal_vocabulary() {
        let mut surface = surface(&["a", "b"]);
        surface
            .replace(&SurfaceSpan::new(id("a"), id("b")), id("s"))
            .expect("replace");
        assert_eq!(
            surface.ops(),
            &[
                SurfaceOp::Append {
                    message_id: id("a")
                },
                SurfaceOp::Append {
                    message_id: id("b")
                },
                SurfaceOp::Replace {
                    start: id("a"),
                    end: id("b"),
                    replacement: id("s"),
                },
            ]
        );
        assert_eq!(surface.retired(), vec![id("a"), id("b")]);
    }
}
