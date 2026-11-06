//! The backend-independent Store contracts of the native durable conversation.
//!
//! [`ConversationStore`] is the one durable authority for a conversation. It
//! is the union of its domain contracts, each declared beside its own
//! vocabulary:
//!
//! ```text
//! InboxStore        Pending Inbound Inbox and Goal          (durable::inbox)
//! CanonicalStore    Message Ledger, Surface, transcript     (store::canonical)
//! RequestStore      Request Snapshots, model-turn start     (store::request)
//! JournalStore      Event Journal, Agent authority          (store::journal)
//! PublicationStore  durable publication plane               (store::publication)
//! ProjectionStore   Context projection, compaction, lineage (store::projection)
//! ObservationStore  bounded observation reads, readiness    (store::observation)
//! AuxiliaryStore    auxiliary Call/request ownership, audit (store::auxiliary)
//! ```
//!
//! The split is by semantic domain, never by table: a compound transition that
//! spans domains (canonical publication, model-turn start, subagent terminal
//! acceptance) is still one method of the domain that owns its linearization
//! point, and the backend commits it in one transaction. A consumer that needs
//! only one domain depends on that domain's contract.

pub mod auxiliary;
pub mod canonical;
pub mod journal;
pub mod observation;
pub mod projection;
pub mod publication;
pub mod request;
pub mod schedule;
pub mod scheduled_workflow;

use crate::runtime::identity::{ConversationId, MessageId, RequestId};

use crate::durable::{
    AuxiliaryStore, CanonicalStore, InboxStore, JournalStore, ObservationStore, ProjectionStore,
    PublicationStore, RequestStore,
};

/// A `ConversationStore` contract violation or storage failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConversationStoreError {
    /// A formerly valid mutable observation cut can no longer be reconstructed.
    ObservationCutExpired,
    /// Finite observation output exceeds its admitted encoded-byte target.
    ObservationCapacityExceeded,
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
    /// A byte range exceeds the protocol limit or retained readable prefix.
    InvalidContentRange,
    /// A finite source or snapshot exceeds an explicit logical materialization budget.
    ResourceBoundExceeded(String),
    /// A requested immutable Request Snapshot does not exist.
    RequestNotFound(RequestId),
    /// A lifecycle event violates terminal uniqueness or terminal ordering.
    TerminalViolation(String),
    /// A publication-plane transition violates the `C => U => P` ordering,
    /// the single-settlement rule, or the tool-proposal execution ban
    /// (Issue #108). The durable store — not only Agent Loop control flow —
    /// rejects impossible publication state combinations.
    PublicationViolation(String),
    /// A cancellable Store scope stopped its operation.
    OperationCancelled,
    /// The owning transaction cannot safely continue, including failed savepoint cleanup.
    TransactionInvalid(String),
    /// The underlying storage rejected the operation.
    Storage(String),
}

impl core::fmt::Display for ConversationStoreError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::ObservationCutExpired => write!(f, "the observation source cut expired"),
            Self::ObservationCapacityExceeded => {
                write!(f, "the observation output exceeds capacity")
            }
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
            Self::InvalidContentRange => write!(f, "invalid content byte range"),
            Self::ResourceBoundExceeded(message) => {
                write!(f, "resource materialization bound: {message}")
            }
            Self::RequestNotFound(request_id) => {
                write!(f, "request snapshot {request_id} is not present")
            }
            Self::TerminalViolation(message) => {
                write!(f, "invalid terminal lifecycle event: {message}")
            }
            Self::PublicationViolation(message) => {
                write!(f, "invalid publication transition: {message}")
            }
            Self::OperationCancelled => write!(f, "the Store operation was cancelled"),
            Self::TransactionInvalid(message) => write!(f, "invalid Store transaction: {message}"),
            Self::Storage(message) => write!(f, "durable ConversationStore failed: {message}"),
        }
    }
}

impl std::error::Error for ConversationStoreError {}

/// The backend-independent durable authority of one conversation: the union
/// of every domain contract, bound to exactly one [`ConversationId`] (the same
/// one-conversation boundary the conversation runtime owns).
///
/// Its own items are the handle-level obligations shared by every domain: the
/// bound identity and the isolated presentation-read scope. Each domain's
/// linearization points are documented on its contract.
#[allow(clippy::missing_errors_doc)]
pub trait ConversationStore:
    InboxStore
    + CanonicalStore
    + RequestStore
    + JournalStore
    + PublicationStore
    + ProjectionStore
    + ObservationStore
    + AuxiliaryStore
    + schedule::ScheduleStore
    + scheduled_workflow::ScheduledWorkflowStore
{
    /// The conversation this store is the durable authority of.
    fn conversation_id(&self) -> &ConversationId;
    /// Isolated cancellation scope for presentation reads. It shares storage,
    /// never execution ownership, and must be closed before joining its tasks.
    fn presentation_reader(&self) -> std::sync::Arc<dyn ConversationStore>;
    /// Cancel this reader's connection waits and running `SQLite` queries.
    fn close_presentation_reader(&self);
    /// A native required-write scope sharing the original connection owner.
    /// Cancellation is confined to this scope's exact connection lease.
    /// Already-owned settlement uses the original, uncancelled Store.
    fn auxiliary_writer(&self) -> std::sync::Arc<dyn ConversationStore>;

    /// Closes new work in this write scope and interrupts only its active lease.
    fn cancel_auxiliary_writer(&self);

    #[cfg(test)]
    fn park_presentation_read_until_cancelled(&self, entered: tokio::sync::oneshot::Sender<()>);
}
