//! Backend-independent semantics of the native durable conversation store.
//!
//! This module owns the domain vocabulary and the [`ConversationStore`] trait,
//! which is the one durable authority boundary for Pending Inbound, the
//! Message Ledger, Surface revisions, Request Snapshots, Event Journal facts,
//! and checkpoint metadata.
//! There is deliberately no generic repository, queue, CRUD, or storage
//! strategy trait: the operations are the rustX semantic transitions a
//! `PostgreSQL` backend must reproduce exactly.

use std::sync::Arc;

use chrono::{DateTime, Utc};

use crate::conversation::{SurfaceRevision, SurfaceSpan};
use crate::events::types::RuntimeEventEnvelope;
use crate::message::types::{
    InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
};
use crate::model::snapshot::RequestSnapshot;
use crate::model::types::ModelRequest;
use crate::runtime::identity::{ConversationId, MessageId, RequestId};
use crate::runtime::inbound::InboundSequence;
use crate::runtime::types::TokenMeasurement;

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

/// The bounded current working set loaded from the durable Conversation
/// Surface at runtime bootstrap.
///
/// Historical Surface operations remain in the durable store. The runtime
/// only needs the current active identity order and the immutable head
/// metadata for its normal hot path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableConversationHead {
    /// The current immutable Surface revision.
    pub revision: SurfaceRevision,
    /// The current compaction generation.
    pub compaction_generation: u64,
    /// Current active `MessageIds` in model-visible order.
    pub active_message_ids: Vec<MessageId>,
}

/// The semantic input to one atomic canonical compaction transition.
#[derive(Debug, Clone, PartialEq)]
pub struct CompactionCommitInput {
    /// The canonical runtime summary to append to the Ledger.
    pub summary: UserMessageBlock,
    /// The inclusive active span to replace.
    pub span: SurfaceSpan,
    /// The Surface revision validated by the caller.
    pub expected_revision: SurfaceRevision,
    /// The pre-compaction token measurement for the completion fact.
    pub tokens_before: TokenMeasurement,
    /// The deterministic estimate after rebuilding the request context.
    pub estimated_tokens_after: u64,
    /// The owning attempt, when the transition is executing in an attempt.
    pub attempt_id: Option<crate::runtime::identity::AttemptId>,
    /// The owning turn, when the transition is executing in a turn.
    pub turn_id: Option<crate::runtime::identity::TurnId>,
    /// The persisted UTC event timestamp.
    pub timestamp: DateTime<Utc>,
}

/// A page of durable canonical messages.
#[derive(Debug, Clone, PartialEq)]
pub struct CanonicalMessagePage {
    /// Messages in stable Ledger commit order.
    pub messages: Vec<MessageBlock>,
    /// The last Ledger position in this page, when non-empty.
    pub next_position: Option<u64>,
}

/// A page of durable Event Journal envelopes.
#[derive(Debug, Clone, PartialEq)]
pub struct EventPage {
    /// Events in stable conversation sequence order.
    pub events: Vec<RuntimeEventEnvelope>,
    /// The last event sequence in this page, when non-empty.
    pub next_sequence: Option<u64>,
}

/// A bounded page of immutable Request Snapshots.
#[derive(Debug, Clone, PartialEq)]
pub struct RequestSnapshotPage {
    /// Snapshots ordered by their durable `ModelRequestStarted` sequence.
    pub snapshots: Vec<RequestSnapshot>,
    /// The exclusive cursor for the next page, when this page is non-empty.
    pub next_sequence: Option<u64>,
}

/// The one composition-time binding of a conversation's durable authority.
///
/// A binding owns the full backend-independent store handle and is the only
/// production composition object from which the narrow inbound capability is
/// derived. Keeping those handles together means a mailbox cannot be selected
/// independently from the full store used by the conversation runtime.
#[derive(Clone)]
pub struct ConversationStoreBinding {
    store: Arc<dyn ConversationStore>,
}

impl std::fmt::Debug for ConversationStoreBinding {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ConversationStoreBinding")
            .field("conversation_id", self.store.conversation_id())
            .finish_non_exhaustive()
    }
}

impl ConversationStoreBinding {
    /// Binds one full durable authority for composition.
    #[must_use]
    pub fn new(store: Arc<dyn ConversationStore>) -> Self {
        Self { store }
    }

    /// The conversation identity enforced by the bound store.
    #[must_use]
    pub fn conversation_id(&self) -> &ConversationId {
        self.store.conversation_id()
    }

    /// Returns the full authority to the owning conversation runtime.
    pub(crate) fn full_store(&self) -> Arc<dyn ConversationStore> {
        Arc::clone(&self.store)
    }

    /// Derives the narrow producer capability from this same authority.
    pub(crate) fn inbound_capability(&self) -> Arc<dyn ConversationInboundCapability> {
        Arc::new(StoreInboundCapability {
            store: Arc::clone(&self.store),
        })
    }
}

/// A `ConversationStore` contract violation or storage failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConversationStoreError {
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
    /// The database was created by an incompatible development schema.
    SchemaVersionMismatch {
        /// The schema version found in the database.
        stored: i64,
        /// The only schema version this build accepts.
        expected: i64,
    },
    /// The database advertises the current version but does not have the
    /// complete schema shape required by that version.
    IncompatibleSchema(String),
    /// A durable reference points at a fact that is not present.
    InvalidReference(String),
    /// A requested immutable Request Snapshot does not exist.
    RequestNotFound(RequestId),
    /// A lifecycle event violates terminal uniqueness or terminal ordering.
    TerminalViolation(String),
    /// The underlying storage rejected the operation.
    Storage(String),
}

impl core::fmt::Display for ConversationStoreError {
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
                "the durable ConversationStore is bound to conversation {stored}, not {requested}"
            ),
            Self::CorrelationConflict { correlation } => write!(
                f,
                "correlation {correlation} was retried with a conflicting semantic payload"
            ),
            Self::InitialHistoryMismatch => write!(
                f,
                "the re-supplied initial canonical messages do not equal the durable bootstrap initial-history identity"
            ),
            Self::SchemaVersionMismatch { stored, expected } => write!(
                f,
                "incompatible durable schema version {stored}; this build requires {expected}"
            ),
            Self::IncompatibleSchema(detail) => {
                write!(f, "incompatible durable schema shape: {detail}")
            }
            Self::InvalidReference(message) => write!(f, "invalid durable reference: {message}"),
            Self::RequestNotFound(request_id) => {
                write!(f, "request snapshot {request_id} is not present")
            }
            Self::TerminalViolation(message) => {
                write!(f, "invalid terminal lifecycle event: {message}")
            }
            Self::Storage(message) => write!(f, "durable ConversationStore failed: {message}"),
        }
    }
}

impl std::error::Error for ConversationStoreError {}

/// Commits the durable background-ownership fact of one detached execution
/// through a store handle, rejecting every other event payload.
///
/// The narrow capability must not become a general Event Journal seam: the
/// background plane may commit exactly the one execution fact that grants it
/// the right to start a detached side effect, and nothing else.
fn commit_background_ownership_through(
    store: &(impl ConversationStore + ?Sized),
    event: RuntimeEventEnvelope,
) -> Result<RuntimeEventEnvelope, ConversationStoreError> {
    if !matches!(
        event.event,
        crate::events::types::RuntimeEvent::BackgroundExecutionCommitted { .. }
    ) {
        return Err(ConversationStoreError::InvalidReference(
            "the background capability commits only a background ownership fact".to_owned(),
        ));
    }
    store.append_event(event)
}

/// Commits the durable subagent-ownership fact of one child through a store
/// handle, rejecting every other event payload.
///
/// The narrow capability must not become a general Event Journal seam: the
/// subagent plane may commit exactly the one execution fact that grants a
/// child the right to begin detached semantic work, and nothing else.
fn commit_subagent_ownership_through(
    store: &(impl ConversationStore + ?Sized),
    event: RuntimeEventEnvelope,
) -> Result<RuntimeEventEnvelope, ConversationStoreError> {
    if !matches!(
        event.event,
        crate::events::types::RuntimeEvent::SubagentOwnershipCommitted { .. }
    ) {
        return Err(ConversationStoreError::InvalidReference(
            "the subagent capability commits only a subagent ownership fact".to_owned(),
        ));
    }
    store.append_event(event)
}

/// The narrow backend-independent capability used by the Pending Inbound
/// Inbox and its process-local mailbox.
///
/// This capability deliberately exposes no Ledger, Surface, Request Snapshot,
/// or general Event Journal operation. Background/tool code receives this
/// interface only; the conversation execution plane receives the full
/// [`ConversationStore`] separately.
///
/// Four — and only four — Event Journal facts are reachable here, all
/// because they are inseparable from a detached execution's own durable
/// ownership:
///
/// ```text
/// commit_background_ownership  -> BackgroundExecutionCommitted   (background start commit)
/// commit_subagent_ownership    -> SubagentOwnershipCommitted     (subagent start commit)
/// accept_inbound_with_event    -> BackgroundTerminalPublished    (background terminal commit)
/// accept_inbound_with_event    -> SubagentTerminalPublished      (subagent terminal commit)
/// ```
///
/// Each is a typed single-purpose transition, never a generic event append.
#[allow(clippy::missing_errors_doc)]
pub trait ConversationInboundCapability: Send + Sync + 'static {
    /// The conversation this capability serves.
    fn conversation_id(&self) -> &ConversationId;

    /// Accepts one inbound item durably.
    fn accept_inbound(
        &self,
        draft: InboundDraft,
    ) -> Result<AcceptedInbound, ConversationStoreError>;

    /// Atomically accepts one inbound item and its dependent background fact.
    fn accept_inbound_with_event(
        &self,
        draft: InboundDraft,
        event: RuntimeEventEnvelope,
    ) -> Result<(AcceptedInbound, RuntimeEventEnvelope), ConversationStoreError>;

    /// Commits the durable background-ownership fact of one detached
    /// execution (Issue #12, M9a).
    ///
    /// The commit happens strictly **before** the detached runner's start
    /// gate is released, so no external background side effect can begin
    /// without durable evidence of the owning `ToolExecutionId`. The payload
    /// must be a
    /// [`RuntimeEvent::BackgroundExecutionCommitted`](crate::events::types::RuntimeEvent::BackgroundExecutionCommitted);
    /// every other event is rejected.
    fn commit_background_ownership(
        &self,
        event: RuntimeEventEnvelope,
    ) -> Result<RuntimeEventEnvelope, ConversationStoreError>;

    /// Commits the durable subagent-ownership fact of one child runtime
    /// (Issue #60).
    ///
    /// The commit happens strictly **before** the child receives its
    /// delegation, so no child semantic side effect can begin without
    /// durable evidence of the owning `SubagentId`. The payload must be a
    /// [`RuntimeEvent::SubagentOwnershipCommitted`](crate::events::types::RuntimeEvent::SubagentOwnershipCommitted);
    /// every other event is rejected.
    fn commit_subagent_ownership(
        &self,
        event: RuntimeEventEnvelope,
    ) -> Result<RuntimeEventEnvelope, ConversationStoreError>;

    /// Selects a finite pending batch without consuming it.
    fn select_pending_batch(&self) -> Result<Option<PendingBatch>, ConversationStoreError>;

    /// Adopts the selected pending watermark atomically.
    fn adopt_pending_batch(
        &self,
        watermark: InboundSequence,
    ) -> Result<Vec<MessageBlock>, ConversationStoreError>;

    /// Reads pending items for bootstrap.
    fn load_pending(&self) -> Result<Vec<PendingInboundItem>, ConversationStoreError>;
}

/// The backend-independent durable authority of the Pending Inbound Inbox,
/// Message Ledger, Conversation Surface, Request Snapshots, Event Journal,
/// and checkpoint metadata.
///
/// One store instance is bound to exactly one [`ConversationId`] (the same
/// one-conversation boundary the conversation runtime owns). Implementations
/// must hold the following invariants:
///
/// - [`ConversationStore::accept_inbound`] is the acceptance linearization point:
///   sequence allocation, pending persistence, and correlation state commit
///   in one transaction. No success is reported before the commit, and a
///   failed acceptance exposes no sequence, no pending record, and no
///   correlation.
/// - [`ConversationStore::select_pending_batch`] is non-destructive: it returns a
///   finite watermark snapshot without removing any pending record.
/// - [`ConversationStore::adopt_pending_batch`] is the canonical-adoption
///   linearization point: it appends the selected messages to the durable
///   canonical ledger and removes their pending records in one transaction,
///   so a crash can never observe a pending record whose canonical message is
///   absent, nor a canonical message that remains independently re-adoptable.
#[allow(clippy::missing_errors_doc)]
pub trait ConversationStore: Send + Sync + 'static {
    /// The conversation this store is the durable inbound authority of.
    fn conversation_id(&self) -> &ConversationId;

    /// Accepts one inbound item durably.
    ///
    /// # Errors
    ///
    /// Returns [`ConversationStoreError::EmptyContent`] for empty content,
    /// [`ConversationStoreError::SequenceExhausted`] when the sequence domain is
    /// exhausted, [`ConversationStoreError::DuplicateMessageId`] when a
    /// producer-supplied identity collides, and
    /// [`ConversationStoreError::Storage`] on a backend failure.
    fn accept_inbound(
        &self,
        draft: InboundDraft,
    ) -> Result<AcceptedInbound, ConversationStoreError>;

    /// Atomically accepts one inbound notification and the execution fact
    /// that grants it durable publication ownership. This specialized
    /// transition is used by detached background terminal settlement; the
    /// Event Journal fact references the accepted `MessageId` and is committed
    /// in the same transaction as the Pending Inbound row.
    fn accept_inbound_with_event(
        &self,
        draft: InboundDraft,
        event: RuntimeEventEnvelope,
    ) -> Result<(AcceptedInbound, RuntimeEventEnvelope), ConversationStoreError>;

    /// Selects the currently pending items as one finite watermark-bounded
    /// batch, without consuming them.
    ///
    /// # Errors
    ///
    /// Returns [`ConversationStoreError::Storage`] on a backend read failure.
    fn select_pending_batch(&self) -> Result<Option<PendingBatch>, ConversationStoreError>;

    /// Atomically adopts every pending item through `watermark` into the
    /// durable canonical message ledger, in strict sequence order, returning
    /// the adopted canonical messages.
    ///
    /// Adoption and pending removal share one transaction. Adopting an empty
    /// (or already-adopted) watermark returns an empty vector.
    ///
    /// # Errors
    ///
    /// Returns [`ConversationStoreError::Storage`] when the adoption transaction
    /// fails; on failure the selected items remain pending and recoverable.
    fn adopt_pending_batch(
        &self,
        watermark: InboundSequence,
    ) -> Result<Vec<MessageBlock>, ConversationStoreError>;

    /// Loads every accepted-but-not-yet-adopted pending item in strict
    /// sequence order (recovery/bootstrap seam).
    ///
    /// # Errors
    ///
    /// Returns [`ConversationStoreError::Storage`] on a backend read failure.
    fn load_pending(&self) -> Result<Vec<PendingInboundItem>, ConversationStoreError>;

    /// Initializes the durable Ledger and Surface from one immutable bootstrap
    /// history and establishes its exact immutable bootstrap identity.
    /// Reopening verifies the original identity instead of inferring it from
    /// current rows. The first call atomically commits the initial messages
    /// and the identity; every later call must re-supply the exact original
    /// history. An explicitly empty initial history is valid and remains
    /// distinguishable from an uninitialized store.
    ///
    /// # Errors
    ///
    /// Returns [`ConversationStoreError::InitialHistoryMismatch`] when the
    /// re-supplied history differs, and [`ConversationStoreError::Storage`]
    /// when the initialization transaction fails or a canonical Ledger exists
    /// without its bootstrap identity.
    fn initialize(&self, messages: &[MessageBlock]) -> Result<(), ConversationStoreError>;

    /// Loads the current Surface head and checkpoint metadata without
    /// materializing historical revisions.
    fn load_head(&self) -> Result<DurableConversationHead, ConversationStoreError>;

    /// Resolves the requested `MessageIds` through keyed Ledger reads.
    fn load_messages(&self, ids: &[MessageId])
    -> Result<Vec<MessageBlock>, ConversationStoreError>;

    /// Reconstructs one exact historical Surface revision from immutable
    /// Surface operations.
    fn reconstruct_surface(
        &self,
        revision: SurfaceRevision,
    ) -> Result<Vec<MessageId>, ConversationStoreError>;

    /// Appends one canonical [`MessageBlock`] to the durable Message Ledger.
    ///
    /// This is the canonical-append durability seam for ordinary
    /// **non-inbound** commits (Assistant messages, `ToolResult`s, and
    /// admitted context facts). It must be called in canonical commit order
    /// so the durable Ledger records the exact committed fact. Inbound
    /// adoption appends its
    /// selected User messages through [`ConversationStore::adopt_pending_batch`]
    /// and compaction summaries use [`ConversationStore::commit_compaction`];
    /// neither structurally special transition can be split through this
    /// method. The store remains the historical Ledger authority; hot runtime
    /// state is only a bounded current read model.
    ///
    /// # Errors
    ///
    /// Returns [`ConversationStoreError::DuplicateMessageId`] when the identity is
    /// already committed to the durable Ledger and
    /// [`ConversationStoreError::Storage`] on a backend failure.
    fn append_canonical(&self, message: &MessageBlock) -> Result<(), ConversationStoreError>;

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
    /// Returns [`ConversationStoreError::DuplicateMessageId`] when an identity is
    /// already committed to the durable Ledger and
    /// [`ConversationStoreError::Storage`] on a backend failure.
    fn append_canonical_batch(
        &self,
        messages: &[MessageBlock],
    ) -> Result<(), ConversationStoreError>;

    /// Commits a canonical message and its committed-message Event Journal
    /// fact in one `SQLite` transaction.
    fn append_canonical_with_event(
        &self,
        message: &MessageBlock,
        event: RuntimeEventEnvelope,
    ) -> Result<RuntimeEventEnvelope, ConversationStoreError>;

    /// Commits a structurally atomic canonical batch and all corresponding
    /// committed-message events in one `SQLite` transaction.
    fn append_canonical_batch_with_events(
        &self,
        messages: &[MessageBlock],
        events: &[RuntimeEventEnvelope],
    ) -> Result<Vec<RuntimeEventEnvelope>, ConversationStoreError>;

    /// Commits the summary Ledger row, immutable Surface Replace revision,
    /// checkpoint metadata, and `CompactionCompleted` fact atomically.
    fn commit_compaction(
        &self,
        input: CompactionCommitInput,
    ) -> Result<(SurfaceRevision, u64, RuntimeEventEnvelope), ConversationStoreError>;

    /// Loads the durable canonical Message Ledger in commit order (the
    /// complete crash-recoverable prefix of the Message Ledger).
    ///
    /// # Errors
    ///
    /// Returns [`ConversationStoreError::Storage`] on a backend read failure.
    fn load_canonical(&self) -> Result<Vec<MessageBlock>, ConversationStoreError>;

    /// Reads a bounded Ledger page. A caller can walk history without
    /// retaining the complete Ledger as hot state.
    fn load_canonical_page(
        &self,
        after_position: Option<u64>,
        limit: usize,
    ) -> Result<CanonicalMessagePage, ConversationStoreError>;

    /// Commits one model-turn start atomically (Issue #12, M9b): the
    /// request-scoped canonical context messages (Ledger append + Surface
    /// advance), the immutable Request Snapshot, and the exact
    /// `ModelRequestStarted` evidence in **one** transaction.
    ///
    /// This is the one durable request-start transition of every actual
    /// primary model request — the first turn, every tool→model
    /// continuation, every recovered continuation, and every overflow retry.
    /// A successful commit is the durable fact that the model request
    /// started; a failure commits none of the inputs. The Agent Loop
    /// arbitrates cancellation against exactly this commit, so a
    /// `RequestSnapshot` is always evidence of an actually started model
    /// request and request-scoped context never becomes canonical without
    /// its request starting.
    ///
    /// The store validates structure and durability only; it owns no
    /// cancellation policy. The retry-safe idempotency rule is unchanged:
    /// repeating the exact same start (same snapshot, same context) returns
    /// the original start fact, and a conflicting retry is rejected.
    fn commit_model_turn_start(
        &self,
        context: &[MessageBlock],
        snapshot: &RequestSnapshot,
        timestamp: DateTime<Utc>,
    ) -> Result<RuntimeEventEnvelope, ConversationStoreError>;

    /// Loads one immutable Request Snapshot on demand.
    fn load_request_snapshot(
        &self,
        request_id: &RequestId,
    ) -> Result<RequestSnapshot, ConversationStoreError>;

    /// Reconstructs a historical provider-neutral request entirely from
    /// durable Request Snapshot, Surface, and Ledger facts.
    fn reconstruct_model_request(
        &self,
        request_id: &RequestId,
    ) -> Result<ModelRequest, ConversationStoreError>;

    /// Reads a bounded page of immutable Request Snapshots in durable request
    /// start order. `after_sequence` is an exclusive Event Journal sequence
    /// cursor; the returned cursor is the last snapshot's start sequence.
    fn read_request_snapshots(
        &self,
        after_sequence: Option<u64>,
        limit: usize,
    ) -> Result<RequestSnapshotPage, ConversationStoreError>;

    /// Appends one standalone execution fact after validating every durable
    /// reference and lifecycle terminal rule. Canonical-message,
    /// compaction-completion, request-start, and background-publication facts
    /// must use their specialized combined transition; this method rejects
    /// them so a reference event cannot be split from its durable authority.
    fn append_event(
        &self,
        event: RuntimeEventEnvelope,
    ) -> Result<RuntimeEventEnvelope, ConversationStoreError>;

    /// Reads a bounded Event Journal page in stable sequence order.
    fn read_events(
        &self,
        after_sequence: Option<u64>,
        limit: usize,
    ) -> Result<EventPage, ConversationStoreError>;
}

impl<T: ConversationStore + ?Sized> ConversationInboundCapability for T {
    fn conversation_id(&self) -> &ConversationId {
        ConversationStore::conversation_id(self)
    }

    fn accept_inbound(
        &self,
        draft: InboundDraft,
    ) -> Result<AcceptedInbound, ConversationStoreError> {
        ConversationStore::accept_inbound(self, draft)
    }

    fn accept_inbound_with_event(
        &self,
        draft: InboundDraft,
        event: RuntimeEventEnvelope,
    ) -> Result<(AcceptedInbound, RuntimeEventEnvelope), ConversationStoreError> {
        ConversationStore::accept_inbound_with_event(self, draft, event)
    }

    fn commit_background_ownership(
        &self,
        event: RuntimeEventEnvelope,
    ) -> Result<RuntimeEventEnvelope, ConversationStoreError> {
        commit_background_ownership_through(self, event)
    }

    fn commit_subagent_ownership(
        &self,
        event: RuntimeEventEnvelope,
    ) -> Result<RuntimeEventEnvelope, ConversationStoreError> {
        commit_subagent_ownership_through(self, event)
    }

    fn select_pending_batch(&self) -> Result<Option<PendingBatch>, ConversationStoreError> {
        ConversationStore::select_pending_batch(self)
    }

    fn adopt_pending_batch(
        &self,
        watermark: InboundSequence,
    ) -> Result<Vec<MessageBlock>, ConversationStoreError> {
        ConversationStore::adopt_pending_batch(self, watermark)
    }

    fn load_pending(&self) -> Result<Vec<PendingInboundItem>, ConversationStoreError> {
        ConversationStore::load_pending(self)
    }
}

/// Erases the full store behind the narrow capability exposed by one binding.
/// The wrapper carries the same store handle; it does not create another
/// durable authority or another identity domain.
struct StoreInboundCapability {
    store: Arc<dyn ConversationStore>,
}

impl ConversationInboundCapability for StoreInboundCapability {
    fn conversation_id(&self) -> &ConversationId {
        self.store.conversation_id()
    }

    fn accept_inbound(
        &self,
        draft: InboundDraft,
    ) -> Result<AcceptedInbound, ConversationStoreError> {
        self.store.accept_inbound(draft)
    }

    fn accept_inbound_with_event(
        &self,
        draft: InboundDraft,
        event: RuntimeEventEnvelope,
    ) -> Result<(AcceptedInbound, RuntimeEventEnvelope), ConversationStoreError> {
        self.store.accept_inbound_with_event(draft, event)
    }

    fn commit_background_ownership(
        &self,
        event: RuntimeEventEnvelope,
    ) -> Result<RuntimeEventEnvelope, ConversationStoreError> {
        commit_background_ownership_through(self.store.as_ref(), event)
    }

    fn commit_subagent_ownership(
        &self,
        event: RuntimeEventEnvelope,
    ) -> Result<RuntimeEventEnvelope, ConversationStoreError> {
        commit_subagent_ownership_through(self.store.as_ref(), event)
    }

    fn select_pending_batch(&self) -> Result<Option<PendingBatch>, ConversationStoreError> {
        self.store.select_pending_batch()
    }

    fn adopt_pending_batch(
        &self,
        watermark: InboundSequence,
    ) -> Result<Vec<MessageBlock>, ConversationStoreError> {
        self.store.adopt_pending_batch(watermark)
    }

    fn load_pending(&self) -> Result<Vec<PendingInboundItem>, ConversationStoreError> {
        self.store.load_pending()
    }
}

/// Convenience: the canonical [`MessageBlock`] for one accepted item.
#[must_use]
pub fn canonical_block(message: &UserMessageBlock) -> MessageBlock {
    MessageBlock::User(message.clone())
}
