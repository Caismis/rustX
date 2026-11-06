//! Store contract for the durable publication plane (Issue #108).

use crate::durable::{CommittedModelTurn, ModelTurnSettlement};
use crate::events::types::RuntimeEventEnvelope;
use crate::message::types::MessageBlock;
use crate::publication::{
    PublicationAudit, PublicationFrame, PublicationStreamRecord, PublicationStreamStart,
};
use crate::runtime::identity::PublicationStreamId;
use chrono::{DateTime, Utc};

use crate::durable::{ConversationStoreError, TranscriptCommitReceipt, TranscriptCursor};

#[derive(Debug, Clone, PartialEq)]
pub struct PublicationAuditReceipt {
    pub ready_through: Option<u64>,
    /// Existing owner-published T prefix captured in this native transaction.
    pub ready_t_through: Option<u64>,
    pub summary: PublicationAuditSummary,
    pub transcript_cursor: TranscriptCursor,
    pub header: crate::durable::observation::TranscriptHeader,
    pub read_cut: crate::durable::observation::ObservationReadCut,
}

/// Exact positive settlement and source identities, independent of body size.
#[derive(Debug, Clone, PartialEq)]
pub struct PublicationAuditSummary {
    pub stream_id: Option<crate::runtime::identity::PublicationStreamId>,
    pub attempt_id: Option<crate::runtime::identity::AttemptId>,
    pub turn_id: Option<crate::runtime::identity::TurnId>,
    pub request_id: Option<crate::runtime::identity::RequestId>,
    pub message_id: Option<crate::runtime::identity::MessageId>,
    pub kind: crate::publication::PublicationAuditKind,
    pub settled_at: chrono::DateTime<chrono::Utc>,
    pub block_count: u64,
}

/// The durable publication plane of the conversation Store (Issue #108).
///
/// Publication durability is a plane of its own, separate from provider
/// outcome (P) and canonical conversation acceptance (C). The store is the
/// authority for the ordering `C => U => P` and for the rule that a stream
/// settles exactly once; the Agent Loop may not be the only thing keeping
/// an impossible combination out of durable state.
#[allow(clippy::missing_errors_doc)]
pub trait PublicationStore: Send + Sync + 'static {
    // ---------------------------------------------------------------------
    // The durable publication plane (Issue #108, FND-03).
    //
    // Publication durability is a plane of its own, separate from provider
    // outcome (P) and canonical conversation acceptance (C). The store is the
    // authority for the ordering `C => U => P` and for the rule that a stream
    // settles exactly once; the Agent Loop may not be the only thing keeping
    // an impossible combination out of durable state.
    // ---------------------------------------------------------------------

    /// Opens one publication stream, pinning it to the exact attempt, turn,
    /// request, and provisional message identity that started it.
    ///
    /// Opening is idempotent for the identical start: re-opening the same
    /// stream with the same frozen identities succeeds and changes nothing,
    /// so a retried open cannot fork a stream. Re-opening with different
    /// identities is a [`ConversationStoreError::PublicationViolation`].
    fn open_publication_stream(
        &self,
        start: &PublicationStreamStart,
    ) -> Result<(), ConversationStoreError>;

    // ---------------------------------------------------------------------
    // The durable publication plane (Issue #108, FND-03).
    //
    // Publication durability is a plane of its own, separate from provider
    // outcome (P) and canonical conversation acceptance (C). The store is the
    // authority for the ordering `C => U => P` and for the rule that a stream
    // settles exactly once; the Agent Loop may not be the only thing keeping
    // an impossible combination out of durable state.
    // ---------------------------------------------------------------------

    /// Stages publication frames durably, before any of them is released.
    ///
    /// The first write admits the exact frozen request generation atomically
    /// with its frames. Frames must belong to one unsettled stream and must
    /// continue its sequence without a gap or a repeat. A stream that already committed U accepts no further frames.
    /// The finite receipts are constructed inside this transaction and returned
    /// only after commit; display publication requires no subsequent store read.
    fn stage_publication_frames(
        &self,
        start: &PublicationStreamStart,
        frames: &[PublicationFrame],
    ) -> Result<Vec<crate::publication::PublicationFrameReceipt>, ConversationStoreError>;
    /// Commits **U**: the final publication frame(s) and the publication
    /// terminal marker in one transaction.
    ///
    /// There is deliberately no "write final frame, publish, then mark
    /// complete" sequence to crash inside. When no visible payload remains,
    /// the caller supplies a single
    /// [`TerminalOnly`](crate::publication::PublicationPayload::TerminalOnly)
    /// frame, so the terminal transition always has a frame to commit
    /// atomically with its marker.
    ///
    /// The store rejects U without P: the exact request's
    /// [`RuntimeEvent::ModelRequestCompleted`](crate::events::types::RuntimeEvent::ModelRequestCompleted)
    /// must already be durable.
    fn commit_publication_terminal(
        &self,
        start: &PublicationStreamStart,
        frames: &[PublicationFrame],
    ) -> Result<Vec<crate::publication::PublicationFrameReceipt>, ConversationStoreError>;
    /// Atomically accepts ordinary model output, including provider completion,
    /// final frames and canonical acceptance. No-tool output also commits its
    /// finite inbox adoption or empty-inbox Attempt completion in this boundary.
    /// No prior provider completion is accepted by this operation.
    fn settle_model_turn(
        &self,
        settlement: ModelTurnSettlement<'_>,
    ) -> Result<CommittedModelTurn, ConversationStoreError>;

    /// Commits **C** for a published stream as one compound transition.
    ///
    /// The transition validates that the exact stream is publication-complete
    /// (U committed, still unsettled), appends the canonical Assistant Ledger
    /// fact, advances the Surface, records `AssistantMessageCommitted`, and
    /// clears the stream's publication staging — all in one transaction. The
    /// store rejects C without U, and rejects C for a stream that already
    /// settled as an audit.
    /// Workflow-owned preparation retains its separate owner boundary.
    fn commit_canonical_publication(
        &self,
        stream_id: &PublicationStreamId,
        message: &MessageBlock,
        event: RuntimeEventEnvelope,
    ) -> Result<(RuntimeEventEnvelope, TranscriptCommitReceipt), ConversationStoreError>;
    /// Terminalizes one unsettled publication stream as an audit.
    ///
    /// The audit kind is derived from durable evidence alone — U present
    /// means [`Unaccepted`](crate::publication::PublicationAuditKind::Unaccepted),
    /// U absent means [`Incomplete`](crate::publication::PublicationAuditKind::Incomplete)
    /// — so a caller can never mislabel a settlement. The transition
    /// consolidates the transient frames into one bounded immutable audit,
    /// removes the staging rows, and permanently forbids canonical
    /// acceptance of that stream.
    fn terminalize_publication_audit(
        &self,
        start: &PublicationStreamStart,
        timestamp: DateTime<Utc>,
    ) -> Result<(PublicationAudit, PublicationAuditReceipt), ConversationStoreError>;
    /// Loads every publication stream that has not settled, for recovery
    /// classification. The records carry frozen identities and the durable
    /// P/U evidence only; nothing here consults a provider or a workspace.
    fn load_unsettled_publication_streams(
        &self,
    ) -> Result<Vec<PublicationStreamRecord>, ConversationStoreError>;
    /// Loads one bounded immutable publication audit, when the stream settled
    /// as an audit.
    fn load_publication_audit(
        &self,
        stream_id: &PublicationStreamId,
    ) -> Result<Option<PublicationAudit>, ConversationStoreError>;
}
