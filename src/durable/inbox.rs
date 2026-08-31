//! Backend-independent semantics of the native durable conversation store.
//!
//! This module owns the domain vocabulary and the [`ConversationStore`] trait,
//! which is the one durable authority boundary for Pending Inbound, the
//! Message Ledger, Surface revisions, Request Snapshots, Event Journal facts,
//! checkpoint metadata, and the derived transcript ordering spine.
//! There is deliberately no generic repository, queue, CRUD, or storage
//! strategy trait: the operations are the rustX semantic transitions a
//! `PostgreSQL` backend must reproduce exactly.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::conversation::{SurfaceOp, SurfaceRevision, SurfaceSpan};
use crate::events::types::{EVENT_SCHEMA_VERSION, RuntimeEvent, RuntimeEventEnvelope};
use crate::message::types::{
    AgentStatusModuleId, InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
};
use crate::model::snapshot::RequestSnapshot;
use crate::model::types::ModelRequest;
use crate::publication::{
    PublicationAudit, PublicationFrame, PublicationStreamRecord, PublicationStreamStart,
};
use crate::runtime::identity::{
    AttemptId, ConversationId, EventId, MessageId, PublicationStreamId, RequestId,
};
use crate::runtime::inbound::InboundSequence;
use crate::runtime::types::TokenMeasurement;

/// The default bounded page used when a Runtime Client first attaches.
pub const TRANSCRIPT_BOOTSTRAP_PAGE_LIMIT: usize = 64;

/// The largest page a transcript reader may request in one response.
pub const TRANSCRIPT_PAGE_LIMIT_MAX: usize = 256;

/// A durable transcript cursor.
///
/// This cursor belongs to the transcript ordering spine. It is deliberately
/// distinct from the Runtime Client observation cursor, the Event Journal
/// sequence, and the inbound mailbox sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TranscriptCursor(u64);

impl TranscriptCursor {
    /// Creates a cursor from its durable ordering position.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the durable ordering position.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl core::fmt::Display for TranscriptCursor {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// The transcript position allocated by one durable canonical-message
/// transition.  A `None` receipt is intentional for durable context messages,
/// which remain model history but are hidden from the ordinary transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TranscriptCommitReceipt {
    /// The durable transcript position, when the committed fact is visible.
    pub transcript_cursor: Option<TranscriptCursor>,
}

/// The durable receipt of one model-turn-start transition.
///
/// The receipt carries the complete start-owned Event Journal sequence in
/// durable order. `NewlyCommitted` is the only disposition that publishes
/// those events through the live observation seam; `IdempotentReplay` is an
/// exact historical verification and must not replay its facts to observers.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelTurnStartCommit {
    /// The `ModelRequestStarted` fact, first in the committed sequence.
    pub started: RuntimeEventEnvelope,
    /// The status emission facts committed immediately after `started`, in
    /// the prepared emission order (which is also durable sequence order).
    pub agent_status_emissions: Vec<RuntimeEventEnvelope>,
    /// Whether this call inserted the transition or only verified it.
    pub disposition: ModelTurnStartCommitDisposition,
}

impl ModelTurnStartCommit {
    /// Returns every start-owned event in exact durable sequence order.
    pub fn events(&self) -> impl Iterator<Item = &RuntimeEventEnvelope> {
        std::iter::once(&self.started).chain(self.agent_status_emissions.iter())
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
    /// The transcript position allocated at acceptance for visible inbound.
    /// Adoption into the Ledger reuses this exact position.
    pub transcript_cursor: Option<TranscriptCursor>,
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
    /// The transcript position allocated at acceptance for visible inbound.
    pub transcript_cursor: Option<TranscriptCursor>,
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

impl PendingBatch {
    /// The durable answer obligation this batch acquires when it is adopted.
    ///
    /// `attempt_id` is the attempt that owns the adoption: the running attempt
    /// of a safe-boundary drain, and `None` for the coordinator admission
    /// path, where no attempt exists yet.
    #[must_use]
    pub fn adoption_event(&self, attempt_id: Option<AttemptId>) -> RuntimeEventEnvelope {
        inbound_adoption_event(
            &self.conversation_id,
            attempt_id,
            self.items
                .iter()
                .map(|item| item.message_id.clone())
                .collect(),
        )
    }
}

/// Builds the [`RuntimeEvent::InboundTurnAdopted`] fact of one adoption.
///
/// The adoption transaction commits this fact with the canonical messages it
/// names, so the durable authority can never hold an adopted turn without the
/// obligation to answer it, nor an obligation naming messages it did not adopt.
#[must_use]
pub fn inbound_adoption_event(
    conversation_id: &ConversationId,
    attempt_id: Option<AttemptId>,
    message_ids: Vec<MessageId>,
) -> RuntimeEventEnvelope {
    RuntimeEventEnvelope {
        schema_version: EVENT_SCHEMA_VERSION,
        // The durable owner allocates the identity with the sequence.
        event_id: EventId::new(""),
        sequence: 0,
        conversation_id: conversation_id.clone(),
        attempt_id,
        turn_id: None,
        timestamp: Utc::now(),
        event: RuntimeEvent::InboundTurnAdopted { message_ids },
    }
}

/// The canonical lineage seed of one new conversation lineage.
///
/// A lineage is seeded with two distinguishable parts, because what a
/// conversation durably *means*, what the model currently *sees*, and how it
/// came to see that are not the same set of facts:
///
/// ```text
/// canonical        the Ledger cut the destination inherits, in commit order
/// surface history  the Surface operations that produced the destination's
///                  Surface from that Ledger, in revision order
/// ```
///
/// Compaction is what makes the two differ. It retires facts from the
/// Surface while leaving them canonical, so a seed carrying only the Surface
/// would silently drop every conversation-owned fact a compaction had
/// already retired — and a copy of a compacted conversation would then mean
/// something different from a copy of the same conversation taken one moment
/// earlier.
///
/// The seed carries the *operations*, not merely the final active set, for
/// the same reason one step further out. A final active set records which
/// messages the Surface shows; it cannot record why. Seeding
/// `[summary, C]` as two appends and seeding it as "append `C`, then replace
/// the earlier span with `summary`" produce the same Surface and two
/// different histories — and a fork or tree taken later *on the copy* reads
/// that history, not the final set. Only the second reproduces the source's
/// branch points, so only the second makes copying closed under the lineage
/// operations that follow it: a fork of a copy at a copied boundary means
/// what a fork of the source at the same boundary means.
///
/// A seed whose history is one `Append` per canonical message in Ledger
/// order is the ordinary case ([`LineageSeed::history`]), and it is what
/// every conversation that has never been compacted produces. The seed is
/// intentionally limited to canonical/domain messages and Surface
/// provenance; execution facts remain owned by the source `ConversationId`
/// and are not copied into the destination. In particular, the pending
/// unresolved-output carryover pointer is execution-recovery residue, not
/// lineage meaning: it is never a seed field, and a destination starts with
/// no pending carryover.
#[derive(Debug, Clone, PartialEq)]
pub struct LineageSeed {
    canonical: Vec<MessageBlock>,
    surface_history: Vec<SurfaceOp>,
    surface: Vec<MessageId>,
}

impl LineageSeed {
    /// The seed of a lineage whose whole canonical history is also its
    /// Surface: nothing was ever retired, so every operation is an append
    /// and the two parts coincide.
    #[must_use]
    pub fn history(canonical: Vec<MessageBlock>) -> Self {
        let surface: Vec<MessageId> = canonical
            .iter()
            .map(crate::conversation::message_id_of)
            .collect();
        let surface_history = surface
            .iter()
            .map(|message_id| SurfaceOp::Append {
                message_id: message_id.clone(),
            })
            .collect();
        Self {
            canonical,
            surface_history,
            surface,
        }
    }

    /// The seed of a lineage that inherits a Surface *history*, and so
    /// inherits canonical facts its Surface no longer shows together with
    /// the operations that retired them.
    ///
    /// The seed is checked against what durable transitions can actually
    /// reach, not merely against what replays. Every transition that adds a
    /// canonical row adds it together with the one Surface operation that
    /// introduces it, inside one transaction: an ordinary commit appends the
    /// message it committed, and a compaction appends its summary to the
    /// Ledger and replaces a span with that same summary. So the two orders
    /// this type carries are not independent — the canonical identities in
    /// Ledger order are exactly the identities the history introduces in
    /// revision order, one for one.
    ///
    /// Requiring that equality is what closes the two gaps a replay check
    /// alone leaves open. A canonical row no operation ever introduces
    /// replays fine and is still unreachable: it would be a conversation-owned
    /// fact with no Surface provenance at all, invisible to the history the
    /// destination's own forks read, yet fully visible to everything that
    /// rebuilds state from canonical history — the task list included. And a
    /// Ledger ordered against its own history replays fine too, while
    /// recording commits in an order no sequence of transitions produced.
    ///
    /// # Errors
    ///
    /// Returns [`ConversationStoreError::InvalidReference`] when the history
    /// names an identity `canonical` does not carry, does not replay against
    /// an initially empty Surface, replaces a span with a message that is not
    /// a `User(Runtime / CompactionSummary)`, or introduces identities that
    /// are not the seeded Ledger in its own order. None is a state any
    /// sequence of durable transitions could reach, and a store that accepted
    /// one would hold a Surface history it could never reconstruct.
    pub fn replayed(
        canonical: Vec<MessageBlock>,
        surface_history: Vec<SurfaceOp>,
    ) -> Result<Self, ConversationStoreError> {
        let known: std::collections::BTreeMap<MessageId, &MessageBlock> = canonical
            .iter()
            .map(|message| (crate::conversation::message_id_of(message), message))
            .collect();
        let mut surface: Vec<MessageId> = Vec::new();
        for operation in &surface_history {
            for id in operation.message_ids() {
                if !known.contains_key(id) {
                    return Err(ConversationStoreError::InvalidReference(format!(
                        "the seeded Surface history names {id}, which the seeded Ledger does \
                         not carry"
                    )));
                }
            }
            if let SurfaceOp::Replace { replacement, .. } = operation {
                let is_summary = matches!(
                    known.get(replacement),
                    Some(MessageBlock::User(user))
                        if user.source == UserSource::Runtime
                            && user.kind.is_compaction_summary()
                );
                if !is_summary {
                    return Err(ConversationStoreError::InvalidReference(format!(
                        "the seeded Surface Replace replacement {replacement} is not a \
                         User(Runtime / CompactionSummary) message"
                    )));
                }
            }
            crate::conversation::apply_surface_op(&mut surface, operation)
                .map_err(ConversationStoreError::InvalidReference)?;
        }
        // The provenance pairing: one operation per canonical row, in the
        // Ledger's own order.
        let introduced: Vec<&MessageId> =
            surface_history.iter().map(SurfaceOp::introduces).collect();
        let committed: Vec<MessageId> = canonical
            .iter()
            .map(crate::conversation::message_id_of)
            .collect();
        if introduced.len() != committed.len()
            || introduced
                .iter()
                .zip(&committed)
                .any(|(introduced, committed)| *introduced != committed)
        {
            let render = |ids: &[&MessageId]| {
                ids.iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            return Err(ConversationStoreError::InvalidReference(format!(
                "the seeded Surface history introduces [{}], which is not the seeded \
                 Ledger [{}] in commit order; a reachable lineage introduces each \
                 canonical row with exactly one operation, in the order the Ledger \
                 carries it",
                render(&introduced),
                render(&committed.iter().collect::<Vec<_>>())
            )));
        }
        Ok(Self {
            canonical,
            surface_history,
            surface,
        })
    }

    /// The seeded Ledger cut, in canonical commit order.
    #[must_use]
    pub fn canonical(&self) -> &[MessageBlock] {
        &self.canonical
    }

    /// The seeded Surface operation history, in revision order. Replaying it
    /// from an empty Surface yields [`Self::surface`].
    #[must_use]
    pub fn surface_history(&self) -> &[SurfaceOp] {
        &self.surface_history
    }

    /// The Surface the seeded history denotes, in Surface order. Always a
    /// subset of [`Self::canonical`].
    #[must_use]
    pub fn surface(&self) -> &[MessageId] {
        &self.surface
    }
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

/// One retained Surface revision in which a canonical user message first
/// appears. Backends return these boundaries directly so callers do not need
/// to materialize every historical Surface revision.
#[derive(Debug, Clone, PartialEq)]
pub struct SurfaceUserMessageBoundary {
    /// The first retained Surface revision containing the message.
    pub surface_revision: SurfaceRevision,
    /// The canonical user message body.
    pub message: UserMessageBlock,
}

/// A bounded page of retained user-message boundaries.
#[derive(Debug, Clone, PartialEq)]
pub struct SurfaceUserMessageBoundaryPage {
    /// The requested boundary rows.
    pub boundaries: Vec<SurfaceUserMessageBoundary>,
    /// Offset for the next page, when more rows exist.
    pub next_offset: Option<usize>,
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

/// One durable semantic Agent Status emission fact and its materialized latest
/// lookup position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentStatusEmissionRecord {
    /// The closed module that owns the reminder semantics.
    pub module_id: AgentStatusModuleId,
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
    pub todo_progress_origin: u64,
    /// The Event Journal sequence of the emission fact.
    pub event_sequence: u64,
}

/// The read-only bounded suppression-history view used during status
/// preparation.
pub trait AgentStatusEmissionLookup: Send + Sync {
    /// Reads the latest durable emission for one semantic module/key pair.
    ///
    /// # Errors
    ///
    /// Returns the conversation-store error when the bounded lookup cannot be
    /// completed or its durable row is malformed.
    fn latest_agent_status_emission(
        &self,
        module_id: AgentStatusModuleId,
        key: &str,
    ) -> Result<Option<AgentStatusEmissionRecord>, ConversationStoreError>;

    /// Reads the current conversation-owned Todo progress sequence through a
    /// bounded projection. One unit is one newly committed first request of a
    /// logical primary model step; overflow retries do not advance it.
    /// Preparation only reads this value, and it never schedules work.
    ///
    /// # Errors
    ///
    /// Returns the conversation-store error when the bounded lookup fails.
    fn current_todo_progress(&self) -> Result<u64, ConversationStoreError>;
}

/// A bounded page of immutable Request Snapshots.
#[derive(Debug, Clone, PartialEq)]
pub struct RequestSnapshotPage {
    /// Snapshots ordered by their durable `ModelRequestStarted` sequence.
    pub snapshots: Vec<RequestSnapshot>,
    /// The exclusive cursor for the next page, when this page is non-empty.
    pub next_sequence: Option<u64>,
}

/// One item in the derived durable transcript read model.
///
/// The item carries bodies only in the bounded read result. Durable storage
/// keeps the body in its owning Message Ledger, publication-audit, or Event
/// Journal domain and stores only a reference in the transcript ordering
/// spine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum TranscriptItem {
    /// A user, Assistant, or Tool message resolved from the canonical owner.
    Message {
        /// The canonical or durably accepted message body.
        message: MessageBlock,
    },
    /// A noncanonical Assistant publication audit.
    PublicationAudit {
        /// The bounded audit body owned by the publication plane.
        audit: PublicationAudit,
    },
    /// A historical requested interaction audit fact.
    InteractionRequested {
        /// The Event Journal envelope that owns the audit body.
        event: RuntimeEventEnvelope,
    },
    /// A historical settled interaction audit fact.
    InteractionSettled {
        /// The Event Journal envelope that owns the audit body.
        event: RuntimeEventEnvelope,
    },
}

/// One ordered item in a bounded transcript page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptEntry {
    /// The stable exclusive cursor for this item.
    pub cursor: TranscriptCursor,
    /// The resolved read-model item.
    pub item: TranscriptItem,
}

/// A bounded page of the derived durable transcript.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptPage {
    /// Items in chronological order within this page.
    pub entries: Vec<TranscriptEntry>,
    /// The cursor to pass as `before` for the next older page.
    pub next_cursor: Option<TranscriptCursor>,
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
    /// A publication-plane transition violates the `C => U => P` ordering,
    /// the single-settlement rule, or the tool-proposal execution ban
    /// (Issue #108). The durable store — not only Agent Loop control flow —
    /// rejects impossible publication state combinations.
    PublicationViolation(String),
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
            Self::PublicationViolation(message) => {
                write!(f, "invalid publication transition: {message}")
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

/// Commits the durable terminal-settlement fact of a Workflow-owned child
/// through a store handle. Unlike `SubagentTerminalPublished`, this fact has
/// no inbound message: `WorkflowRuntime` consumes the child result directly and
/// the durable transition exists only to close the ownership lifecycle and
/// preserve terminal evidence.
fn commit_subagent_terminal_through(
    store: &(impl ConversationStore + ?Sized),
    event: RuntimeEventEnvelope,
) -> Result<RuntimeEventEnvelope, ConversationStoreError> {
    let successful = matches!(
        &event.event,
        crate::events::types::RuntimeEvent::SubagentTerminalSettled {
            state: crate::events::types::SubagentTerminalState::Succeeded,
            ..
        }
    );
    if successful {
        return Err(ConversationStoreError::InvalidReference(
            "a successful Workflow terminal must commit its value with the native child terminal fact".to_owned(),
        ));
    }
    if !matches!(
        event.event,
        crate::events::types::RuntimeEvent::SubagentTerminalSettled { .. }
    ) {
        return Err(ConversationStoreError::InvalidReference(
            "the subagent capability commits only a Workflow terminal-settlement fact".to_owned(),
        ));
    }
    store.commit_subagent_terminal(event)
}

/// Commits a successful Workflow Agent's validated value and its native child
/// terminal lifecycle fact in one durable transaction. The two facts are
/// deliberately a narrow compound transition: a Workflow child has no
/// parent inbound notification, and neither fact may become a separate
/// settled/delivered authority phase.
fn commit_workflow_agent_terminal_through(
    store: &(impl ConversationStore + ?Sized),
    terminal: RuntimeEventEnvelope,
    output: RuntimeEventEnvelope,
) -> Result<(RuntimeEventEnvelope, RuntimeEventEnvelope), ConversationStoreError> {
    let valid_pair = matches!(
        (&terminal.event, &output.event),
        (
            crate::events::types::RuntimeEvent::SubagentTerminalSettled {
                subagent_id: terminal_subagent,
                state: crate::events::types::SubagentTerminalState::Succeeded,
                ..
            },
            crate::events::types::RuntimeEvent::WorkflowAgentOutputCommitted {
                subagent_id: output_subagent,
                ..
            }
        ) if terminal_subagent == output_subagent
    );
    if !valid_pair {
        return Err(ConversationStoreError::InvalidReference(
            "the Workflow terminal transition requires one successful SubagentTerminalSettled and its matching WorkflowAgentOutputCommitted fact".to_owned(),
        ));
    }
    store.commit_workflow_agent_terminal(terminal, output)
}

/// Commits one durable interaction audit fact through a store handle,
/// rejecting every other event payload.
///
/// The narrow capability must not become a general Event Journal seam: the
/// interaction plane may commit exactly the two semantic facts that make a
/// human decision auditable, and nothing else.
fn commit_interaction_audit_through(
    store: &(impl ConversationStore + ?Sized),
    event: RuntimeEventEnvelope,
) -> Result<(RuntimeEventEnvelope, TranscriptCursor), ConversationStoreError> {
    if !matches!(
        event.event,
        crate::events::types::RuntimeEvent::InteractionRequested { .. }
            | crate::events::types::RuntimeEvent::InteractionSettled { .. }
    ) {
        return Err(ConversationStoreError::InvalidReference(
            "the interaction capability commits only interaction audit facts".to_owned(),
        ));
    }
    store.append_interaction_audit(event)
}

/// The narrow backend-independent audit capability of the native interaction
/// plane (Issue #109).
///
/// The [`InteractionCoordinator`](crate::runtime::interaction::InteractionCoordinator)
/// owns a pending waiter, which is process-local workflow state, and a pair of
/// durable semantic facts, which are audit evidence. This capability is the
/// only durable authority it receives: no Ledger, no Surface, no Request
/// Snapshot, no publication plane, and no general Event Journal append.
///
/// ```text
/// commit_interaction_requested -> InteractionRequested  (before the prompt is released)
/// commit_interaction_settled   -> InteractionSettled    (before the waiter is released)
/// ```
///
/// Both are typed single-purpose transitions; every other payload is
/// rejected.
#[allow(clippy::missing_errors_doc)]
pub trait ConversationInteractionAudit: Send + Sync + 'static {
    /// The conversation this capability serves.
    fn conversation_id(&self) -> &ConversationId;

    /// Commits the durable requested fact of one interaction.
    ///
    /// The commit happens strictly **before** the prompt is released to a
    /// user-facing client, so no user can be asked without durable evidence
    /// that the interaction existed. The payload must be a
    /// [`RuntimeEvent::InteractionRequested`](crate::events::types::RuntimeEvent::InteractionRequested).
    fn commit_interaction_requested(
        &self,
        event: RuntimeEventEnvelope,
    ) -> Result<(RuntimeEventEnvelope, TranscriptCursor), ConversationStoreError>;

    /// Commits the durable settled fact of one interaction.
    ///
    /// The commit happens strictly **before** the semantic waiter is
    /// released, so an approval can never reach the tool-start frontier ahead
    /// of durable evidence that the approval existed. The payload must be a
    /// [`RuntimeEvent::InteractionSettled`](crate::events::types::RuntimeEvent::InteractionSettled).
    fn commit_interaction_settled(
        &self,
        event: RuntimeEventEnvelope,
    ) -> Result<(RuntimeEventEnvelope, TranscriptCursor), ConversationStoreError>;
}

/// The narrow backend-independent capability used by the Pending Inbound
/// Inbox and its process-local mailbox.
///
/// This capability deliberately exposes no Ledger, Surface, Request Snapshot,
/// or general Event Journal operation. Background/tool code receives this
/// interface only; the conversation execution plane receives the full
/// [`ConversationStore`] separately.
///
/// Six — and only six — Event Journal facts are reachable here, all
/// because they are inseparable from a detached execution's own durable
/// ownership:
///
/// ```text
/// commit_background_ownership  -> BackgroundExecutionCommitted   (background start commit)
/// commit_subagent_ownership    -> SubagentOwnershipCommitted     (subagent start commit)
/// accept_inbound_with_event    -> BackgroundTerminalPublished    (background terminal commit)
/// accept_inbound_with_event    -> SubagentTerminalPublished      (subagent terminal commit)
/// commit_subagent_terminal     -> SubagentTerminalSettled        (Workflow terminal commit)
/// commit_workflow_agent_terminal -> SubagentTerminalSettled + WorkflowAgentOutputCommitted
///                                  (successful Workflow terminal commit)
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

    /// Commits the durable terminal-settlement fact of a Workflow-owned
    /// child without creating a parent inbound notification. The payload
    /// must be a [`RuntimeEvent::SubagentTerminalSettled`](crate::events::types::RuntimeEvent::SubagentTerminalSettled).
    fn commit_subagent_terminal(
        &self,
        event: RuntimeEventEnvelope,
    ) -> Result<RuntimeEventEnvelope, ConversationStoreError>;

    /// Commits a successful Workflow Agent's structured value together with
    /// its native child terminal fact in one durable transaction. The payload
    /// pair is validated as a single typed transition; no parent inbound
    /// notification is created.
    fn commit_workflow_agent_terminal(
        &self,
        terminal: RuntimeEventEnvelope,
        output: RuntimeEventEnvelope,
    ) -> Result<(RuntimeEventEnvelope, RuntimeEventEnvelope), ConversationStoreError>;

    /// Selects a finite pending batch without consuming it.
    fn select_pending_batch(&self) -> Result<Option<PendingBatch>, ConversationStoreError>;

    /// Adopts the selected pending watermark atomically, together with the
    /// durable answer obligation of the adopted turn.
    fn adopt_pending_batch(
        &self,
        watermark: InboundSequence,
        adoption: RuntimeEventEnvelope,
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
/// - [`ConversationStore::has_accepted_inbound`] is monotonic: the durable
///   acceptance watermark advances inside the acceptance commit and no later
///   transition — adoption included — ever rewinds it, so once user work has
///   been durably accepted the answer stays `true` forever.
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
    /// Adoption, pending removal, and the adopted turn's durable **answer
    /// obligation** share one transaction: `adoption` must be a
    /// [`RuntimeEvent::InboundTurnAdopted`] naming exactly the adopted
    /// messages, in the same order. A crash can therefore never observe a
    /// canonical `UserMessage` whose obligation is missing, an obligation
    /// naming work that was not adopted, or a pending record whose canonical
    /// message already exists. Adopting an empty (or already-adopted)
    /// watermark returns an empty vector and commits no obligation.
    ///
    /// # Errors
    ///
    /// Returns [`ConversationStoreError::InvalidReference`] when `adoption` is
    /// not the obligation of exactly this adoption, and
    /// [`ConversationStoreError::Storage`] when the adoption transaction
    /// fails; on failure the selected items remain pending and recoverable.
    fn adopt_pending_batch(
        &self,
        watermark: InboundSequence,
        adoption: RuntimeEventEnvelope,
    ) -> Result<Vec<MessageBlock>, ConversationStoreError>;

    /// Loads every accepted-but-not-yet-adopted pending item in strict
    /// sequence order (recovery/bootstrap seam).
    ///
    /// # Errors
    ///
    /// Returns [`ConversationStoreError::Storage`] on a backend read failure.
    fn load_pending(&self) -> Result<Vec<PendingInboundItem>, ConversationStoreError>;

    /// Whether this conversation has ever committed one durable inbound
    /// acceptance.
    ///
    /// This is the monotonic usage fact Session lifecycle classification is
    /// built on. The acceptance transaction advances the durable inbound
    /// sequence watermark in the same commit as the pending record, and
    /// nothing ever rewinds it: adoption moves the accepted work from Pending
    /// Inbound into the canonical Ledger without touching the watermark, and
    /// lineage seeding writes canonical/Surface history without any
    /// acceptance at all. One atomic read therefore answers "has user work
    /// been durably accepted here" directly, instead of combining two
    /// independently changing projections (current Surface, current Pending
    /// Inbox) whose interleaved reads could assemble a state that never
    /// existed. A conversation with a pending prompt and the same
    /// conversation one adoption later both answer `true`; once `true`, the
    /// answer never becomes `false` again.
    ///
    /// # Errors
    ///
    /// Returns [`ConversationStoreError::Storage`] on a backend read failure.
    fn has_accepted_inbound(&self) -> Result<bool, ConversationStoreError>;

    /// Initializes the durable Ledger and Surface from one immutable
    /// [`LineageSeed`] and establishes its exact immutable bootstrap
    /// identity. Reopening verifies the original identity instead of
    /// inferring it from current rows. The first call atomically commits the
    /// seed and the identity; every later call must re-supply the exact
    /// original canonical history. An explicitly empty seed is valid and
    /// remains distinguishable from an uninitialized store. The initialization
    /// transaction deliberately initializes the execution-recovery pointer to
    /// `NULL`; pending unresolved-output carryover never crosses a lineage
    /// boundary.
    ///
    /// The seed's two parts are written to their two durable homes: every
    /// canonical message becomes a Ledger row in the given order, and the
    /// seed's Surface history becomes this lineage's own retained operation
    /// log, replayed from revision 1. A canonical fact the seed's history
    /// retires is therefore inherited exactly as a compaction leaves it in
    /// the source — durable, readable, not model-visible, and still carrying
    /// the operation that retired it, so this lineage's own historical
    /// branch points are the ones its source had.
    ///
    /// # Errors
    ///
    /// Returns [`ConversationStoreError::InitialHistoryMismatch`] when the
    /// re-supplied canonical history differs, and
    /// [`ConversationStoreError::Storage`] when the initialization
    /// transaction fails or a canonical Ledger exists without its bootstrap
    /// identity.
    fn initialize_lineage(&self, seed: &LineageSeed) -> Result<(), ConversationStoreError>;

    /// Initializes a lineage whose whole canonical history is also its
    /// Surface.
    ///
    /// This is the shape every uncompacted bootstrap has, and it is what a
    /// store opened over a fresh conversation re-supplies on every reopen.
    ///
    /// # Errors
    ///
    /// The errors of [`ConversationStore::initialize_lineage`].
    fn initialize(&self, messages: &[MessageBlock]) -> Result<(), ConversationStoreError> {
        self.initialize_lineage(&LineageSeed::history(messages.to_vec()))
    }

    /// Loads the immutable bootstrap history originally supplied to
    /// [`ConversationStore::initialize_lineage`] — its canonical part, which
    /// is the half the bootstrap identity is taken over. Reopening a lineage
    /// must validate against this prefix, not against its later canonical
    /// transcript, and not against the Surface projection the seed also
    /// carried: that projection is durable state the store already holds, so
    /// a reopen has nothing to re-supply and nothing to contradict.
    fn load_bootstrap_history(&self) -> Result<Vec<MessageBlock>, ConversationStoreError> {
        self.load_canonical()
    }

    /// Loads the current Surface head and checkpoint metadata without
    /// materializing historical revisions.
    fn load_head(&self) -> Result<DurableConversationHead, ConversationStoreError>;

    /// Resolves the requested `MessageIds` through keyed Ledger reads.
    fn load_messages(&self, ids: &[MessageId])
    -> Result<Vec<MessageBlock>, ConversationStoreError>;

    /// Reads the retained Surface operations through `through`, in revision
    /// order, exactly as they were committed.
    ///
    /// This is the provenance half of a lineage copy. A Surface snapshot says
    /// which messages are active; this says how they became active, which is
    /// what a later fork or tree of the copy reads when it looks for its own
    /// branch points. See [`LineageSeed`].
    ///
    /// # Errors
    ///
    /// Returns [`ConversationStoreError::InvalidReference`] when `through` is
    /// not a retained revision, and [`ConversationStoreError::Storage`] on a
    /// backend read failure.
    fn load_surface_history(
        &self,
        through: SurfaceRevision,
    ) -> Result<Vec<SurfaceOp>, ConversationStoreError>;

    /// Reconstructs one exact historical Surface revision from immutable
    /// Surface operations.
    fn reconstruct_surface(
        &self,
        revision: SurfaceRevision,
    ) -> Result<Vec<MessageId>, ConversationStoreError>;

    /// Materializes one exact historical Surface revision from durable
    /// facts. Implementations must not invoke Context Assembly, compaction,
    /// provider code, or any runtime execution while answering this read.
    ///
    /// The default is deliberately expressed in terms of the two primitive
    /// durable reads so backend implementations remain small; `SQLite`
    /// overrides it with one connection-locked read section.
    fn load_surface_snapshot(
        &self,
        revision: SurfaceRevision,
    ) -> Result<Vec<MessageBlock>, ConversationStoreError> {
        let ids = self.reconstruct_surface(revision)?;
        self.load_messages(&ids)
    }

    /// Loads every ordinary inbound user message through one retained Surface
    /// revision together with the exact first revision in which it appears.
    /// The result is ordered by that first appearance and does not materialize
    /// each intermediate Surface snapshot.
    fn load_user_message_boundaries(
        &self,
        through: SurfaceRevision,
    ) -> Result<Vec<SurfaceUserMessageBoundary>, ConversationStoreError>;

    /// Loads one bounded page of ordinary inbound user-message boundaries.
    ///
    /// The page is ordered by first appearance in the selected retained
    /// Surface history. The offset/limit seam belongs specifically to the
    /// Session tree projection; it is not a general durable pagination API.
    fn load_user_message_boundaries_page(
        &self,
        through: SurfaceRevision,
        offset: usize,
        limit: usize,
    ) -> Result<SurfaceUserMessageBoundaryPage, ConversationStoreError>;

    /// Appends one canonical [`MessageBlock`] to the durable Message Ledger.
    ///
    /// This is the canonical-append durability seam for ordinary
    /// **non-inbound** commits (Assistant messages, `ToolResult`s, and
    /// admitted context facts). It must be called in canonical commit order
    /// so the durable Ledger records the exact committed fact. Generated
    /// Agent Status is admitted by `commit_model_turn_start`, which couples
    /// its canonical message to the start-owned emission settlement; this
    /// method is not that generation path. Inbound adoption appends its
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
    fn append_canonical(
        &self,
        message: &MessageBlock,
    ) -> Result<TranscriptCommitReceipt, ConversationStoreError>;

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
    ) -> Result<Vec<TranscriptCommitReceipt>, ConversationStoreError>;

    /// Commits a canonical message and its committed-message Event Journal
    /// fact in one `SQLite` transaction.
    fn append_canonical_with_event(
        &self,
        message: &MessageBlock,
        event: RuntimeEventEnvelope,
    ) -> Result<(RuntimeEventEnvelope, TranscriptCommitReceipt), ConversationStoreError>;

    /// Commits a structurally atomic canonical batch and all corresponding
    /// committed-message events in one `SQLite` transaction.
    fn append_canonical_batch_with_events(
        &self,
        messages: &[MessageBlock],
        events: &[RuntimeEventEnvelope],
    ) -> Result<(Vec<RuntimeEventEnvelope>, Vec<TranscriptCommitReceipt>), ConversationStoreError>;

    /// Commits the summary Ledger row, immutable Surface Replace revision,
    /// checkpoint metadata, and `CompactionCompleted` fact atomically.
    fn commit_compaction(
        &self,
        input: CompactionCommitInput,
    ) -> Result<
        (SurfaceRevision, u64, RuntimeEventEnvelope, TranscriptCursor),
        ConversationStoreError,
    >;

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

    /// Reads one bounded page of the derived durable transcript.
    ///
    /// With no `before` cursor, the newest page is returned. With a cursor,
    /// only rows strictly older than that cursor are eligible. Rows are
    /// returned in chronological order and the next cursor is exclusive, so
    /// appends after a page has been read cannot create duplicates or gaps in
    /// an older-page walk.
    fn load_transcript_page(
        &self,
        before: Option<TranscriptCursor>,
        limit: usize,
    ) -> Result<TranscriptPage, ConversationStoreError>;

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

    /// Reads the materialized latest Agent Status emission for one bounded
    /// semantic module/key pair. Normal status preparation never scans the
    /// Event Journal; this projection is advanced only by the combined
    /// model-turn-start transition that commits the referenced status message
    /// and emission fact together.
    fn latest_agent_status_emission(
        &self,
        module_id: AgentStatusModuleId,
        key: &str,
    ) -> Result<Option<AgentStatusEmissionRecord>, ConversationStoreError>;

    /// Reads the bounded Todo progress sequence used by the concrete Todo
    /// reminder policy. It advances only in the fresh logical model-turn
    /// start transaction, never for request-scoped context or Agent Status
    /// appends.
    fn current_todo_progress(&self) -> Result<u64, ConversationStoreError>;

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

    /// Commits a non-success Workflow-owned child terminal fact without a
    /// parent inbound notification. Successful Workflow terminals must use
    /// [`ConversationStore::commit_workflow_agent_terminal`] so their value
    /// and lifecycle fact cannot split across durable transitions.
    fn commit_subagent_terminal(
        &self,
        event: RuntimeEventEnvelope,
    ) -> Result<RuntimeEventEnvelope, ConversationStoreError>;

    /// Commits a successful Workflow Agent's structured value and its native
    /// `SubagentTerminalSettled` lifecycle fact in one durable transaction.
    /// This specialized transition is the Workflow terminal boundary: it
    /// records execution evidence without creating a parent message or a
    /// durable `settled -> delivered` state.
    fn commit_workflow_agent_terminal(
        &self,
        terminal: RuntimeEventEnvelope,
        output: RuntimeEventEnvelope,
    ) -> Result<(RuntimeEventEnvelope, RuntimeEventEnvelope), ConversationStoreError>;

    /// Commits one interaction audit event and returns the transcript
    /// position allocated in the same durable transaction.
    fn append_interaction_audit(
        &self,
        event: RuntimeEventEnvelope,
    ) -> Result<(RuntimeEventEnvelope, TranscriptCursor), ConversationStoreError>;

    /// Reads a bounded Event Journal page in stable sequence order.
    fn read_events(
        &self,
        after_sequence: Option<u64>,
        limit: usize,
    ) -> Result<EventPage, ConversationStoreError>;

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

    /// Stages publication frames durably, before any of them is released.
    ///
    /// This is the non-terminal staging commit. Frames must belong to one
    /// open, unsettled stream and must continue its sequence without a gap or
    /// a repeat. A stream that already committed U accepts no further frames.
    fn stage_publication_frames(
        &self,
        frames: &[PublicationFrame],
    ) -> Result<(), ConversationStoreError>;

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
        stream_id: &PublicationStreamId,
        frames: &[PublicationFrame],
    ) -> Result<(), ConversationStoreError>;

    /// Commits **C** for a published stream as one compound transition.
    ///
    /// The transition validates that the exact stream is publication-complete
    /// (U committed, still unsettled), appends the canonical Assistant Ledger
    /// fact, advances the Surface, records `AssistantMessageCommitted`, and
    /// clears the stream's publication staging — all in one transaction. The
    /// store rejects C without U, and rejects C for a stream that already
    /// settled as an audit.
    fn commit_canonical_publication(
        &self,
        stream_id: &PublicationStreamId,
        message: &MessageBlock,
        event: RuntimeEventEnvelope,
    ) -> Result<(RuntimeEventEnvelope, TranscriptCursor), ConversationStoreError>;

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
        stream_id: &PublicationStreamId,
        timestamp: DateTime<Utc>,
    ) -> Result<(PublicationAudit, TranscriptCursor), ConversationStoreError>;

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

impl<T: ConversationStore + ?Sized> AgentStatusEmissionLookup for T {
    fn latest_agent_status_emission(
        &self,
        module_id: AgentStatusModuleId,
        key: &str,
    ) -> Result<Option<AgentStatusEmissionRecord>, ConversationStoreError> {
        ConversationStore::latest_agent_status_emission(self, module_id, key)
    }

    fn current_todo_progress(&self) -> Result<u64, ConversationStoreError> {
        ConversationStore::current_todo_progress(self)
    }
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

    fn commit_subagent_terminal(
        &self,
        event: RuntimeEventEnvelope,
    ) -> Result<RuntimeEventEnvelope, ConversationStoreError> {
        commit_subagent_terminal_through(self, event)
    }

    fn commit_workflow_agent_terminal(
        &self,
        terminal: RuntimeEventEnvelope,
        output: RuntimeEventEnvelope,
    ) -> Result<(RuntimeEventEnvelope, RuntimeEventEnvelope), ConversationStoreError> {
        commit_workflow_agent_terminal_through(self, terminal, output)
    }

    fn select_pending_batch(&self) -> Result<Option<PendingBatch>, ConversationStoreError> {
        ConversationStore::select_pending_batch(self)
    }

    fn adopt_pending_batch(
        &self,
        watermark: InboundSequence,
        adoption: RuntimeEventEnvelope,
    ) -> Result<Vec<MessageBlock>, ConversationStoreError> {
        ConversationStore::adopt_pending_batch(self, watermark, adoption)
    }

    fn load_pending(&self) -> Result<Vec<PendingInboundItem>, ConversationStoreError> {
        ConversationStore::load_pending(self)
    }
}

impl<T: ConversationStore + ?Sized> ConversationInteractionAudit for T {
    fn conversation_id(&self) -> &ConversationId {
        ConversationStore::conversation_id(self)
    }

    fn commit_interaction_requested(
        &self,
        event: RuntimeEventEnvelope,
    ) -> Result<(RuntimeEventEnvelope, TranscriptCursor), ConversationStoreError> {
        if !matches!(
            event.event,
            crate::events::types::RuntimeEvent::InteractionRequested { .. }
        ) {
            return Err(ConversationStoreError::InvalidReference(
                "the interaction capability commits a requested fact only".to_owned(),
            ));
        }
        commit_interaction_audit_through(self, event)
    }

    fn commit_interaction_settled(
        &self,
        event: RuntimeEventEnvelope,
    ) -> Result<(RuntimeEventEnvelope, TranscriptCursor), ConversationStoreError> {
        if !matches!(
            event.event,
            crate::events::types::RuntimeEvent::InteractionSettled { .. }
        ) {
            return Err(ConversationStoreError::InvalidReference(
                "the interaction capability commits a settled fact only".to_owned(),
            ));
        }
        commit_interaction_audit_through(self, event)
    }
}

/// Erases the full store behind the narrow interaction audit capability.
///
/// The wrapper carries the same store handle; it creates no second durable
/// authority. A `dyn ConversationStore` cannot be re-coerced to another trait
/// object directly, so this is the one place the narrowing happens.
#[must_use]
pub fn interaction_audit_capability(
    store: Arc<dyn ConversationStore>,
) -> Arc<dyn ConversationInteractionAudit> {
    Arc::new(StoreInteractionAudit { store })
}

struct StoreInteractionAudit {
    store: Arc<dyn ConversationStore>,
}

impl ConversationInteractionAudit for StoreInteractionAudit {
    fn conversation_id(&self) -> &ConversationId {
        self.store.conversation_id()
    }

    fn commit_interaction_requested(
        &self,
        event: RuntimeEventEnvelope,
    ) -> Result<(RuntimeEventEnvelope, TranscriptCursor), ConversationStoreError> {
        self.store.commit_interaction_requested(event)
    }

    fn commit_interaction_settled(
        &self,
        event: RuntimeEventEnvelope,
    ) -> Result<(RuntimeEventEnvelope, TranscriptCursor), ConversationStoreError> {
        self.store.commit_interaction_settled(event)
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

    fn commit_subagent_terminal(
        &self,
        event: RuntimeEventEnvelope,
    ) -> Result<RuntimeEventEnvelope, ConversationStoreError> {
        commit_subagent_terminal_through(self.store.as_ref(), event)
    }

    fn commit_workflow_agent_terminal(
        &self,
        terminal: RuntimeEventEnvelope,
        output: RuntimeEventEnvelope,
    ) -> Result<(RuntimeEventEnvelope, RuntimeEventEnvelope), ConversationStoreError> {
        commit_workflow_agent_terminal_through(self.store.as_ref(), terminal, output)
    }

    fn select_pending_batch(&self) -> Result<Option<PendingBatch>, ConversationStoreError> {
        self.store.select_pending_batch()
    }

    fn adopt_pending_batch(
        &self,
        watermark: InboundSequence,
        adoption: RuntimeEventEnvelope,
    ) -> Result<Vec<MessageBlock>, ConversationStoreError> {
        self.store.adopt_pending_batch(watermark, adoption)
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
