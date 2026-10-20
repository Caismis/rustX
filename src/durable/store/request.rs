//! Store contract for immutable Request Snapshots and the model-turn start transition.

use crate::events::types::RuntimeEventEnvelope;
use crate::message::types::MessageBlock;
use crate::model::snapshot::RequestSnapshot;
use crate::model::types::ModelRequest;
use crate::runtime::identity::{AttemptId, MessageId, PublicationStreamId, RequestId};
use chrono::{DateTime, Utc};

use crate::durable::{
    ConversationStore, ConversationStoreError, TranscriptCommitReceipt, TranscriptCursor,
};

/// The durable receipt of one model-turn-start transition.
///
/// The receipt carries the complete start-owned Event Journal sequence in
/// durable order. `NewlyCommitted` is the only disposition that publishes
/// those events through the live observation seam; `IdempotentReplay` is an
/// exact historical verification and must not replay its facts to observers.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelTurnStartCommit {
    /// Native admission evidence installed only after successful transaction commit.
    pub(crate) verified_snapshot: Option<Box<RequestSnapshot>>,
    pub status_sources: Vec<StatusObservationSource>,
    /// Commit-owned canonical context descriptors; never reloaded at a later head.
    pub context_receipts: Vec<TranscriptCommitReceipt>,
    /// The `ModelRequestStarted` fact, first in the committed sequence.
    pub started: RuntimeEventEnvelope,
    /// The status emission facts committed immediately after `started`, in
    /// the prepared emission order (which is also durable sequence order).
    pub contribution_emissions: Vec<RuntimeEventEnvelope>,
    /// Whether this call inserted the transition or only verified it.
    pub disposition: ModelTurnStartCommitDisposition,
}

impl ModelTurnStartCommit {
    /// Proves the immutable prepared values are precisely those validated at commit.
    /// This proof does not reload mutable current history after the start frontier.
    pub(crate) fn accepts_prepared(
        &self,
        snapshot: &RequestSnapshot,
        request: &crate::model::ModelRequest,
    ) -> bool {
        self.verified_snapshot
            .as_deref()
            .is_some_and(|verified| verified == snapshot)
            && snapshot.input.messages().same_input(&request.messages)
            && snapshot.invocation == request.invocation
            && snapshot.effective_system_prompt == request.effective_system_prompt
            && snapshot.tool_definitions == request.tools
            && snapshot.images == request.images
            && snapshot.continuation == request.continuation
            && snapshot.continuation_source == request.continuation_source
    }
    /// Returns every start-owned event in exact durable sequence order.
    pub fn events(&self) -> impl Iterator<Item = &RuntimeEventEnvelope> {
        std::iter::once(&self.started).chain(self.contribution_emissions.iter())
    }
}

/// Whether a model-turn-start receipt committed new durable facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelTurnStartCommitDisposition {
    /// The transition was committed by this call.
    NewlyCommitted,
    /// The exact transition was already committed and was verified again.
    IdempotentReplay,
}

/// Exact canonical sources selected by the native model-turn-start transaction.
#[derive(Debug, Clone, PartialEq)]
pub struct StatusObservationSource {
    pub contribution_ordinal: u64,
    pub request_id: crate::runtime::identity::RequestId,
    pub locator: crate::observation::content::CanonicalContentLocator,
    pub rendered_ref: Option<crate::observation::content::ContentRef>,
    pub target_locator: Option<crate::observation::content::CanonicalContentLocator>,
    pub target_transcript: Option<TranscriptCursor>,
    pub target_metadata_ref: Option<crate::observation::content::ContentRef>,
}

/// One durable semantic contribution emission fact and its materialized latest
/// lookup position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContributionEmissionRecord {
    /// Registration-bound producer namespace.
    pub producer: crate::runtime::identity::ContextContributorIdentity,
    /// Stable semantic reminder identity.
    pub key: String,
    /// Fingerprint of the bounded relevant content that was emitted.
    pub fingerprint: String,
    /// The durable event timestamp.
    pub emitted_at: DateTime<Utc>,
    /// The exact model request start that made the emission visible.
    pub request_id: RequestId,
    /// The exact canonical Agent Status User message referenced by the fact.
    pub canonical_message_id: MessageId,
    /// The store-assigned Todo progress sequence at which the corresponding
    /// model-turn start became durable. This is the reminder's cooldown
    /// origin, not an evaluation coordinate supplied by the caller.
    pub logical_step_origin: u64,
    /// The Event Journal sequence of the emission fact.
    pub event_sequence: u64,
}

/// The read-only bounded suppression-history view used during status
/// preparation.
pub trait ContributionEmissionLookup: Send + Sync {
    /// Reads the latest durable emission for one semantic module/key pair.
    ///
    /// # Errors
    ///
    /// Returns the conversation-store error when the bounded lookup cannot be
    /// completed or its durable row is malformed.
    fn latest_contribution_emission(
        &self,
        producer: &crate::runtime::identity::ContextContributorIdentity,
        key: &str,
    ) -> Result<Option<ContributionEmissionRecord>, ConversationStoreError>;

    /// Reads the current conversation-owned Todo progress sequence through a
    /// bounded projection. One unit is one newly committed first request of a
    /// logical primary model step; overflow retries do not advance it.
    /// Preparation only reads this value, and it never schedules work.
    ///
    /// # Errors
    ///
    /// Returns the conversation-store error when the bounded lookup fails.
    fn current_logical_step_progress(&self) -> Result<u64, ConversationStoreError>;
}

/// A bounded page of immutable Request Snapshots.
#[derive(Debug, Clone, PartialEq)]
pub struct RequestSnapshotPage {
    /// Snapshots ordered by their durable `ModelRequestStarted` sequence.
    pub snapshots: Vec<RequestSnapshot>,
    /// The exclusive cursor for the next page, when this page is non-empty.
    pub next_sequence: Option<u64>,
}

impl<T: ConversationStore + ?Sized> ContributionEmissionLookup for T {
    fn latest_contribution_emission(
        &self,
        producer: &crate::runtime::identity::ContextContributorIdentity,
        key: &str,
    ) -> Result<Option<ContributionEmissionRecord>, ConversationStoreError> {
        RequestStore::latest_contribution_emission(self, producer, key)
    }

    fn current_logical_step_progress(&self) -> Result<u64, ConversationStoreError> {
        RequestStore::current_logical_step_progress(self)
    }
}

/// The immutable Request Snapshot domain of the conversation Store: the atomic
/// model-turn start, its frozen request evidence, contribution emissions, and
/// the attempt terminal that replaces pending carryover.
#[allow(clippy::missing_errors_doc)]
pub trait RequestStore: Send + Sync + 'static {
    /// Bounded native Attempt outline, including immutable copied origins.
    /// Native opening-input ownership for one actual Attempt at a Journal cut.
    fn attempt_prompt(
        &self,
        attempt: &AttemptId,
        through: u64,
    ) -> Result<Option<MessageId>, ConversationStoreError>;
    /// Commits one model-turn start atomically (Issue #12, M9b): the
    /// request-scoped canonical context messages (Ledger append + Surface
    /// advance), the immutable Request Snapshot, and the exact
    /// `ModelRequestStarted` evidence and, when the prepared request carries
    /// Agent Status, the exact canonical status message, emission facts, and
    /// latest-emission projection in **one** transaction.
    ///
    /// This is the one durable request-start transition of every actual
    /// primary model request — the first turn, every tool→model
    /// continuation, every recovered continuation, every transient retry, and
    /// every overflow retry.
    /// A successful commit is the durable fact that the model request
    /// started; a failure commits none of the inputs. When the snapshot names
    /// the pending unresolved-output source for the initial request of its
    /// logical step, this same transaction freezes the exact request-only
    /// representation and anchor and clears the pending pointer. Cancellation
    /// before this commit therefore leaves the pointer untouched; retries do
    /// not re-read or consume it. The Agent Loop
    /// arbitrates cancellation against exactly this commit, so a
    /// `RequestSnapshot` is always evidence of an actually started model
    /// request and request-scoped context never becomes canonical without
    /// its request starting.
    ///
    /// The store validates structure and durability only; it owns no
    /// cancellation policy. A fresh commit returns a typed receipt containing
    /// every newly committed start-owned event in Event Journal sequence
    /// order. An exact retry returns the same events with an
    /// [`ModelTurnStartCommitDisposition::IdempotentReplay`] disposition, so a
    /// live observer can avoid replaying historical facts. A conflicting
    /// retry is rejected.
    fn commit_model_turn_start(
        &self,
        context: &[MessageBlock],
        snapshot: &RequestSnapshot,
        timestamp: DateTime<Utc>,
    ) -> Result<ModelTurnStartCommit, ConversationStoreError>;
    /// Reads the one pending unresolved-output source, when one exists. The
    /// pointer is durable recovery state only; its body authority remains the
    /// keyed Publication Audit.
    fn load_pending_unresolved_output_stream_id(
        &self,
    ) -> Result<Option<PublicationStreamId>, ConversationStoreError>;
    /// Commits an attempt terminal together with the replacement pending
    /// unresolved-output source selected for its terminally unresolved model
    /// step. `None` explicitly clears a previously pending source. Live
    /// settlement and startup recovery call the same identity-keyed selector
    /// before entering this transition. The event and pointer are one semantic
    /// transaction so a recovery prefix can never expose one without the
    /// other; a newly unresolved step replaces any older pointer rather than
    /// extending a chain.
    fn commit_attempt_terminal_with_carryover(
        &self,
        event: RuntimeEventEnvelope,
        pending_source: Option<PublicationStreamId>,
    ) -> Result<RuntimeEventEnvelope, ConversationStoreError>;
    /// Reads the materialized latest contribution emission for one bounded
    /// semantic module/key pair. Normal status preparation never scans the
    /// Event Journal; this projection is advanced only by the combined
    /// model-turn-start transition that commits the referenced status message
    /// and emission fact together.
    fn latest_contribution_emission(
        &self,
        producer: &crate::runtime::identity::ContextContributorIdentity,
        key: &str,
    ) -> Result<Option<ContributionEmissionRecord>, ConversationStoreError>;
    /// Reads the bounded Todo progress sequence used by the concrete Todo
    /// reminder policy. It advances only in the fresh logical model-turn
    /// start transaction, never for request-scoped context or Agent Status
    /// appends.
    fn current_logical_step_progress(&self) -> Result<u64, ConversationStoreError>;
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
}
