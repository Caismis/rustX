//! Backend-independent domain semantics of the durable Pending Inbound Inbox.
//!
//! This module owns the domain vocabulary and the [`InboundStore`] trait,
//! which is the one durable authority boundary every inbound producer (and
//! the conversation coordinator's safe-boundary adoption) speaks through.
//! There is deliberately no generic repository, queue, CRUD, or storage
//! strategy trait: the operations are the rustX semantic transitions a
//! `PostgreSQL` backend must reproduce exactly.

use chrono::{DateTime, Utc};

use crate::message::types::{
    InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
};
use crate::runtime::identity::{ConversationId, MessageId};
use crate::runtime::inbound::InboundSequence;

/// A producer-supplied draft of one inbound item, before acceptance.
///
/// The producer supplies destination content/provenance/correlation and, for
/// producers that own their message identity (for example a background
/// execution terminal notification), an explicit [`MessageId`]. When the
/// producer owns no stable identity, the acceptance owner allocates a
/// deterministic message id from the allocated [`InboundSequence`].
#[derive(Debug, Clone)]
pub struct InboundDraft {
    /// The producer-supplied stable message identity, when the producer owns
    /// one. `None` means the acceptance owner allocates a deterministic id.
    pub message_id: Option<MessageId>,
    /// Provenance of the inbound work.
    pub source: UserSource,
    /// The typed inbound kind. The ordinary inbound seam accepts only
    /// [`InboundKind::Message`]; a compaction summary is not new work.
    pub kind: InboundKind,
    /// The bounded canonical content.
    pub content: Vec<UserContentBlock>,
    /// The persisted producer timestamp. Never fabricated by the owner.
    pub timestamp: DateTime<Utc>,
    /// Producer correlation/idempotency identity. When present, a retry with
    /// the same correlation returns the same acceptance exactly once.
    pub correlation: Option<String>,
}

/// The committed result of one successful acceptance.
#[derive(Debug, Clone, PartialEq)]
pub struct AcceptedInbound {
    /// The durable per-conversation inbound sequence allocated by the owner.
    pub sequence: InboundSequence,
    /// The stable message identity the item becomes canonical under.
    pub message_id: MessageId,
    /// The persisted canonical inbound message.
    pub message: UserMessageBlock,
    /// Whether this acceptance was an idempotent correlation retry of an
    /// already-committed acceptance (no new sequence was allocated).
    pub retried: bool,
}

/// One accepted-but-not-yet-adopted durable pending item.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingInboundItem {
    /// The durable inbound sequence.
    pub sequence: InboundSequence,
    /// The stable message identity.
    pub message_id: MessageId,
    /// The persisted canonical inbound message.
    pub message: UserMessageBlock,
    /// The producer correlation, when one was supplied.
    pub correlation: Option<String>,
}

/// One finite watermark-bounded pending batch.
///
/// A batch is non-empty, its items are in strictly increasing
/// [`InboundSequence`] order, and its watermark equals the highest selected
/// sequence. Items accepted after the watermark belong to the next batch.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingBatch {
    /// The conversation the batch belongs to.
    pub conversation_id: ConversationId,
    /// The watermark: the highest selected inbound sequence.
    pub watermark: InboundSequence,
    /// The selected items in strict sequence order.
    pub items: Vec<PendingInboundItem>,
}

/// A durable inbox contract violation or storage failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InboundStoreError {
    /// The per-conversation inbound sequence space is exhausted.
    SequenceExhausted,
    /// A producer-supplied message identity is already committed to the
    /// durable pending/canonical domain.
    DuplicateMessageId(MessageId),
    /// The acceptance owner requires non-empty inbound content.
    EmptyContent,
    /// The durable database is bound to a different [`ConversationId`] than
    /// the one the caller requested (Issue #63 store identity). The durable
    /// authority enforces its own identity, so a database created for one
    /// conversation cannot be reopened as another.
    ConversationIdMismatch {
        /// The conversation the database is bound to.
        stored: ConversationId,
        /// The conversation the caller requested.
        requested: ConversationId,
    },
    /// A producer retried an existing correlation with a conflicting
    /// semantic payload. Reusing an idempotency key to mask a producer bug
    /// is rejected rather than silently returning the original acceptance.
    CorrelationConflict {
        /// The correlation whose payload conflicts with its committed one.
        correlation: String,
    },
    /// The re-supplied bootstrap initial messages do not equal the
    /// immutable bootstrap initial-history identity this conversation was
    /// first seeded with (Issue #63 bootstrap identity). The identity is an
    /// explicit durable record (message count + content digest), never
    /// inferred from the current Ledger: neither a shorter prefix nor an
    /// empty replacement of a non-empty bootstrap is accepted.
    InitialHistoryMismatch,
    /// The underlying storage rejected the operation.
    Storage(String),
}

impl core::fmt::Display for InboundStoreError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::SequenceExhausted => write!(f, "the inbound sequence space is exhausted"),
            Self::DuplicateMessageId(id) => {
                write!(
                    f,
                    "message {id} is already committed to the durable inbound domain"
                )
            }
            Self::EmptyContent => write!(f, "inbound content must not be empty"),
            Self::ConversationIdMismatch { stored, requested } => write!(
                f,
                "the durable inbox is bound to conversation {stored}, not {requested}"
            ),
            Self::CorrelationConflict { correlation } => write!(
                f,
                "correlation {correlation} was retried with a conflicting semantic payload"
            ),
            Self::InitialHistoryMismatch => write!(
                f,
                "the re-supplied initial canonical messages do not equal the durable bootstrap initial-history identity"
            ),
            Self::Storage(message) => write!(f, "durable inbound storage failed: {message}"),
        }
    }
}

impl std::error::Error for InboundStoreError {}

/// The backend-independent durable authority of the Pending Inbound Inbox.
///
/// One store instance is bound to exactly one [`ConversationId`] (the same
/// one-conversation boundary the conversation runtime owns). Implementations
/// must hold the following invariants:
///
/// - [`InboundStore::accept_inbound`] is the acceptance linearization point:
///   sequence allocation, pending persistence, and correlation state commit
///   in one transaction. No success is reported before the commit, and a
///   failed acceptance exposes no sequence, no pending record, and no
///   correlation.
/// - [`InboundStore::select_pending_batch`] is non-destructive: it returns a
///   finite watermark snapshot without removing any pending record.
/// - [`InboundStore::adopt_pending_batch`] is the canonical-adoption
///   linearization point: it appends the selected messages to the durable
///   canonical ledger and removes their pending records in one transaction,
///   so a crash can never observe a pending record whose canonical message is
///   absent, nor a canonical message that remains independently re-adoptable.
pub trait InboundStore: Send + Sync + 'static {
    /// The conversation this store is the durable inbound authority of.
    fn conversation_id(&self) -> &ConversationId;

    /// Accepts one inbound item durably.
    ///
    /// # Errors
    ///
    /// Returns [`InboundStoreError::EmptyContent`] for empty content,
    /// [`InboundStoreError::SequenceExhausted`] when the sequence domain is
    /// exhausted, [`InboundStoreError::DuplicateMessageId`] when a
    /// producer-supplied identity collides, and
    /// [`InboundStoreError::Storage`] on a backend failure.
    fn accept_inbound(&self, draft: InboundDraft) -> Result<AcceptedInbound, InboundStoreError>;

    /// Selects the currently pending items as one finite watermark-bounded
    /// batch, without consuming them.
    ///
    /// # Errors
    ///
    /// Returns [`InboundStoreError::Storage`] on a backend read failure.
    fn select_pending_batch(&self) -> Result<Option<PendingBatch>, InboundStoreError>;

    /// Atomically adopts every pending item through `watermark` into the
    /// durable canonical message ledger, in strict sequence order, returning
    /// the adopted canonical messages.
    ///
    /// Adoption and pending removal share one transaction. Adopting an empty
    /// (or already-adopted) watermark returns an empty vector.
    ///
    /// # Errors
    ///
    /// Returns [`InboundStoreError::Storage`] when the adoption transaction
    /// fails; on failure the selected items remain pending and recoverable.
    fn adopt_pending_batch(
        &self,
        watermark: InboundSequence,
    ) -> Result<Vec<MessageBlock>, InboundStoreError>;

    /// Loads every accepted-but-not-yet-adopted pending item in strict
    /// sequence order (recovery/bootstrap seam).
    ///
    /// # Errors
    ///
    /// Returns [`InboundStoreError::Storage`] on a backend read failure.
    fn load_pending(&self) -> Result<Vec<PendingInboundItem>, InboundStoreError>;

    /// Seeds the durable canonical Message Ledger with the conversation's
    /// initial canonical messages and establishes the immutable bootstrap
    /// initial-history identity.
    ///
    /// The first call atomically commits the initial messages **and** the
    /// bootstrap identity (exact message count and content digest). Every
    /// later call — across restarts — must re-supply an initial history
    /// exactly equal to the original one; a shorter prefix, an empty
    /// replacement of a non-empty bootstrap, or any content change is
    /// rejected. An empty initial history is a valid bootstrap and is
    /// recorded explicitly, so "initialized empty" is never confused with
    /// "never initialized".
    ///
    /// # Errors
    ///
    /// Returns [`InboundStoreError::InitialHistoryMismatch`] when the
    /// re-supplied initial history does not exactly equal the original
    /// bootstrap and [`InboundStoreError::Storage`] when the seed
    /// transaction fails or a canonical Ledger exists without its bootstrap
    /// identity (fail-closed: the boundary is never guessed).
    fn seed_canonical(&self, messages: &[MessageBlock]) -> Result<(), InboundStoreError>;

    /// Appends one canonical [`MessageBlock`] to the durable Message Ledger.
    ///
    /// This is the canonical-append durability seam every **non-inbound**
    /// canonical commit goes through (Assistant messages, `ToolResult`s, and
    /// runtime compaction summaries). It must be called in canonical commit
    /// order so the durable Ledger remains the exact ordered prefix of the
    /// authoritative in-memory Message Ledger. Inbound adoption appends its
    /// selected User messages through [`InboundStore::adopt_pending_batch`]
    /// instead, so the pending removal and the canonical append share one
    /// transaction.
    ///
    /// # Errors
    ///
    /// Returns [`InboundStoreError::DuplicateMessageId`] when the identity is
    /// already committed to the durable Ledger and
    /// [`InboundStoreError::Storage`] on a backend failure.
    fn append_canonical(&self, message: &MessageBlock) -> Result<(), InboundStoreError>;

    /// Appends a canonical [`MessageBlock`] batch atomically.
    ///
    /// Every message of the batch commits in **one** transaction: a failure
    /// appends none of them. This is the durable seam for structurally
    /// atomic canonical groups (an `Assistant` tool-call turn's complete
    /// `ToolResult` sibling batch), so a partial group can never become
    /// canonical. It must be called in canonical commit order.
    ///
    /// # Errors
    ///
    /// Returns [`InboundStoreError::DuplicateMessageId`] when an identity is
    /// already committed to the durable Ledger and
    /// [`InboundStoreError::Storage`] on a backend failure.
    fn append_canonical_batch(&self, messages: &[MessageBlock]) -> Result<(), InboundStoreError>;

    /// Loads the durable canonical Message Ledger in commit order (the
    /// complete crash-recoverable prefix of the Message Ledger).
    ///
    /// # Errors
    ///
    /// Returns [`InboundStoreError::Storage`] on a backend read failure.
    fn load_canonical(&self) -> Result<Vec<MessageBlock>, InboundStoreError>;
}

/// Convenience: the canonical [`MessageBlock`] for one accepted item.
#[must_use]
pub fn canonical_block(message: &UserMessageBlock) -> MessageBlock {
    MessageBlock::User(message.clone())
}
