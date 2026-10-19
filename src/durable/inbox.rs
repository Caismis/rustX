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

use crate::conversation::{SurfaceOp, SurfaceRevision};
use crate::events::types::{EVENT_SCHEMA_VERSION, RuntimeEvent, RuntimeEventEnvelope};
use crate::message::types::{
    InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
};
use crate::model::auxiliary::{
    AuxiliaryAuditCut, AuxiliaryAuditPage, AuxiliaryCallAudit, AuxiliaryCallStart,
    AuxiliaryCommitConfirmation, AuxiliaryRequestAudit, AuxiliaryRequestStart,
};
use crate::model::snapshot::{ModelRequestOwner, RequestSnapshot};
use crate::model::types::ModelRequest;
use crate::publication::{
    PublicationAudit, PublicationFrame, PublicationStreamRecord, PublicationStreamStart,
};
use crate::runtime::identity::{
    AttemptId, ConversationId, EventId, MessageId, PublicationStreamId, RequestId,
};
use crate::runtime::inbound::InboundSequence;

/// The default bounded page used when a Runtime Client first attaches.
pub const TRANSCRIPT_BOOTSTRAP_PAGE_LIMIT: usize = 64;

/// The largest page a transcript reader may request in one response.
pub const TRANSCRIPT_PAGE_LIMIT_MAX: usize = 256;

/// A durable transcript cursor.
///
/// This cursor belongs to the transcript ordering spine. It is deliberately
/// distinct from the Runtime Client observation cursor, the Event Journal
/// sequence, and the inbound mailbox sequence.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
)]
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
#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptCommitReceipt {
    pub transcript_cursor: Option<TranscriptCursor>,
    pub ledger_position: u64,
    pub ordinal: u64,
    /// One shared finite proof for the entire atomic native batch.
    pub observation: std::sync::Arc<CanonicalObservationBatch>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CanonicalObservationBatch {
    pub ready_through: Option<u64>,
    /// Existing owner-published T prefix captured in this native transaction.
    pub ready_t_through: Option<u64>,
    pub source_unavailable: bool,
    pub todos: Option<NativeTodoObservation>,
    pub read_cut: crate::durable::observation::ObservationReadCut,
    pub journal_range: Option<(u64, u64)>,
    pub first_position: u64,
    pub last_position: u64,
    pub count: u64,
    pub first_transcript: Option<TranscriptCursor>,
    pub last_transcript: Option<TranscriptCursor>,
    /// Exact visible canonical positions owned by this native atomic batch.
    pub transcript_count: u64,
    /// Complete newest eight descriptors at `read_cut`, in chronological ledger order.
    /// The original first/last/count fields still identify only this write's C/T ownership.
    pub head: Vec<crate::durable::observation::MessageHeader>,
    pub human_activity_at: Option<chrono::DateTime<chrono::Utc>>,
    pub has_opening_user: bool,
    pub opening_preview: Option<String>,
}

/// Finite display head of one canonical Todo publication; the original list
/// remains in its immutable native tool-result source.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeTodoObservation {
    pub source_ledger_position: u64,
    pub snapshot: crate::tools::todo::TodoSnapshot,
    pub total: u64,
    pub loaded: u64,
    pub next_offset: Option<u64>,
}

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

/// Finite source evidence of one native pending transition, never canonical content.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingObservationReceipt {
    pub ready_through: Option<u64>,
    /// Existing owner-published T prefix captured in this native transaction.
    pub ready_t_through: Option<u64>,
    pub read_cut: super::reading::ConversationReadCut,
    pub total: u64,
    pub pending_through: u64,
    pub head: Vec<super::observation::PendingHeader>,
    pub selected: Option<super::observation::PendingHeader>,
    pub removed_sequence: Option<u64>,
    pub removed_transcript: Option<TranscriptCursor>,
    pub adopted_through: Option<u64>,
    pub adopted_count: Option<u64>,
}

/// One finite Goal installation and its exact native transaction evidence.
#[derive(Debug, Clone)]
pub struct GoalObservationReceipt {
    pub view: crate::runtime_client::goal::RuntimeClientGoalView,
    /// Missing derived evidence fences observation without refusing the Goal write.
    pub native: Option<std::sync::Arc<super::observation::NativeObservationBatch>>,
    pub pending: Option<std::sync::Arc<PendingObservationReceipt>>,
}
impl PartialEq for GoalObservationReceipt {
    fn eq(&self, other: &Self) -> bool {
        self.view == other.view
            && self.pending == other.pending
            && match (&self.native, &other.native) {
                (None, None) => true,
                (Some(a), Some(b)) => {
                    a.read_cut == b.read_cut
                        && a.journal_range == b.journal_range
                        && a.source_unavailable == b.source_unavailable
                }
                _ => false,
            }
    }
}

/// Exact canonical sources selected by the native model-turn-start transaction.
#[derive(Debug, Clone, PartialEq)]
pub struct StatusObservationSource {
    pub contribution_ordinal: u64,
    pub request_id: crate::runtime::identity::RequestId,
    pub locator: crate::runtime_client::content::CanonicalContentLocator,
    pub rendered_ref: Option<crate::runtime_client::content::ContentRef>,
    pub target_locator: Option<crate::runtime_client::content::CanonicalContentLocator>,
    pub target_transcript: Option<TranscriptCursor>,
    pub target_metadata_ref: Option<crate::runtime_client::content::ContentRef>,
}

/// The committed result of one successful acceptance.
#[derive(Debug, Clone, PartialEq)]
pub struct AcceptedInbound {
    pub goal_observation: Option<std::sync::Arc<GoalObservationReceipt>>,
    pub pending_observation: Option<std::sync::Arc<PendingObservationReceipt>>,
    /// Native cut captured inside the acceptance transaction, before its commit.
    pub read_cut: super::reading::ConversationReadCut,
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

/// Exact durable pending occurrence and compare-and-set revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PendingInboundRef {
    /// Durable sequence; never reused.
    pub sequence: InboundSequence,
    /// Expected native content revision.
    pub revision: u64,
}

/// Typed pending mutation disposition; uncertainty requires authoritative reread.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum PendingMutationOutcome {
    /// Storage acknowledgement or post-commit readback failed; never replay.
    DurabilityUncertain,
    /// The transition committed; clients reread authority.
    Applied {
        #[serde(skip)]
        #[schemars(skip)]
        pending_observation: Option<std::sync::Arc<PendingObservationReceipt>>,
        /// Exact native mutation cut; never a later Store head.
        read_cut: super::reading::ConversationReadCut,
    },
    /// This occurrence was removed or adopted.
    NotPending,
    /// Another mutation replaced the expected version.
    Conflict,
    /// Content or provenance cannot be edited by this user control.
    InvalidItem,
}

/// One accepted-but-not-yet-adopted durable pending item.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingInboundItem {
    /// Monotonic compare-and-set content revision.
    pub revision: u64,
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

/// The committed result of one adoption transaction.
///
/// `adoption` is present exactly when `items` is non-empty: an adoption that
/// consumed nothing commits no fact, so there is nothing to publish.
#[derive(Debug, Clone, PartialEq)]
pub struct AdoptedBatch {
    pub pending_observation: Option<std::sync::Arc<PendingObservationReceipt>>,
    pub canonical_receipts: Vec<TranscriptCommitReceipt>,
    /// Exact pending/adoption cut captured inside the atomic claim transaction.
    pub read_cut: super::reading::ConversationReadCut,
    /// The adopted items, in strict sequence order.
    pub items: Vec<PendingInboundItem>,
    /// The committed `InboundTurnAdopted` fact, with its Journal sequence.
    pub adoption: Option<RuntimeEventEnvelope>,
}

/// Builds the [`RuntimeEvent::InboundTurnAdopted`] fact of one adoption.
///
/// The adoption transaction commits this fact with the canonical messages it
/// names, so the durable authority can never hold an adopted turn without the
/// obligation to answer it, nor an obligation naming messages it did not adopt.
#[must_use]
pub(crate) fn inbound_adoption_event(
    conversation_id: &ConversationId,
    attempt_id: Option<AttemptId>,
    message_ids: Vec<MessageId>,
) -> RuntimeEventEnvelope {
    RuntimeEventEnvelope {
        tool_occurrence: None,
        observation: None,
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

/// Original provider JSON bytes retained for a canonical Tool call through lineage.
/// This is transient representation-copy input, not a second history owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalRawToolArguments {
    pub message_id: MessageId,
    pub block_index: crate::message::types::ContentBlockIndex,
    pub bytes: Vec<u8>,
}

/// Immutable canonical history and native lineage provenance for a new conversation.
/// Canonical append operations preserve historical boundaries. Checkpoint content
/// and activation provenance are carried independently, together with original
/// raw Tool argument bytes; compaction never manufactures canonical messages.
#[derive(Debug, Clone, PartialEq)]
pub struct LineageSeed {
    auxiliary_audits: Vec<super::reading::AuxiliaryAuditSnapshot>,
    raw_tool_arguments: Vec<CanonicalRawToolArguments>,
    projections: Vec<super::projection::ProjectionCheckpoint>,
    canonical: Vec<MessageBlock>,
    surface_history: Vec<SurfaceOp>,
    surface: Vec<MessageId>,
    completed_responses: Vec<super::response::CompletedResponseProvenance>,
    turns: Vec<super::reading::TurnReadingProvenance>,
}

impl LineageSeed {
    /// Validates exact original JSON against its immutable canonical Tool payload.
    /// # Errors
    /// Rejects an absent/repeated source, invalid UTF-8/JSON, or different arguments.
    pub fn with_raw_tool_arguments(
        mut self,
        raw: Vec<CanonicalRawToolArguments>,
    ) -> Result<Self, ConversationStoreError> {
        let mut seen = std::collections::BTreeSet::new();
        for item in &raw {
            let canonical = self
                .canonical
                .iter()
                .find(|message| message.id() == &item.message_id);
            let Some(MessageBlock::Assistant(assistant)) = canonical else {
                return Err(ConversationStoreError::InvalidReference(
                    "raw Tool source is not canonical Assistant".into(),
                ));
            };
            let Some(crate::message::types::AssistantContentBlock::ToolCall(call)) =
                assistant.content.get(item.block_index.get() as usize)
            else {
                return Err(ConversationStoreError::InvalidReference(
                    "raw Tool source block unavailable".into(),
                ));
            };
            if !seen.insert((item.message_id.clone(), item.block_index))
                || serde_json::from_slice::<serde_json::Value>(&item.bytes)
                    .ok()
                    .as_ref()
                    != Some(&call.arguments)
            {
                return Err(ConversationStoreError::InvalidReference(
                    "raw Tool representation differs from canonical arguments".into(),
                ));
            }
        }
        self.raw_tool_arguments = raw;
        Ok(self)
    }
    #[must_use]
    pub fn raw_tool_arguments(&self) -> &[CanonicalRawToolArguments] {
        &self.raw_tool_arguments
    }

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
            auxiliary_audits: Vec::new(),
            projections: Vec::new(),
            canonical,
            surface_history,
            surface,
            completed_responses: Vec::new(),
            turns: Vec::new(),
            raw_tool_arguments: Vec::new(),
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
    /// an initially empty canonical append view, or introduces identities that
    /// are not the seeded Ledger in its own order. None is a state any
    /// sequence of durable transitions could reach, and a store that accepted
    /// one would hold a Surface history it could never reconstruct.
    #[allow(clippy::too_many_lines)] // One ordered validation of paired Ledger, Surface and checkpoint facts.
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
            auxiliary_audits: Vec::new(),
            projections: Vec::new(),
            canonical,
            surface_history,
            surface,
            completed_responses: Vec::new(),
            turns: Vec::new(),
            raw_tool_arguments: Vec::new(),
        })
    }

    /// Immutable destination-owned checkpoint timeline for future forks of this seed.
    /// # Errors
    /// Rejects incompatible source boundaries, scopes, ordering or checkpoint integrity.
    pub fn with_projections(
        mut self,
        projections: Vec<super::projection::ProjectionCheckpoint>,
    ) -> Result<Self, ConversationStoreError> {
        let mut previous = crate::context::checkpoint::ProjectionActivationId(0);
        for checkpoint in &projections {
            checkpoint
                .candidate
                .validate_budget()
                .map_err(ConversationStoreError::InvalidReference)?;
            if checkpoint.fingerprint != checkpoint.candidate.fingerprint()
                || checkpoint.activation.activation <= previous
                || checkpoint.activation.destination_installation.is_none()
            {
                return Err(ConversationStoreError::InvalidReference(
                    "invalid inherited projection timeline".into(),
                ));
            }
            previous = checkpoint.activation.activation;
        }
        self.projections = projections;
        Ok(self)
    }
    #[must_use]
    pub fn projections(&self) -> &[super::projection::ProjectionCheckpoint] {
        &self.projections
    }

    /// Attach exactly the bounded source audits named by retained checkpoints.
    ///
    /// # Errors
    /// Rejects missing, extra, duplicate or oversized source evidence.
    pub fn with_auxiliary_audits(
        mut self,
        audits: Vec<super::reading::AuxiliaryAuditSnapshot>,
    ) -> Result<Self, ConversationStoreError> {
        let limits = crate::context::resources::ResourceLimits::default();
        if audits.len() > limits.total_items {
            return Err(ConversationStoreError::ResourceBoundExceeded(
                "inherited auxiliary audit count exceeds lineage budget".into(),
            ));
        }
        crate::context::resources::bounded_encoded_len(&audits, limits.total_bytes)?;
        let mut expected = std::collections::BTreeSet::new();
        for checkpoint in &self.projections {
            if let Some(reference) = &checkpoint.candidate.summary_generation {
                expected.insert((
                    reference.conversation_id.clone(),
                    reference.call_id.clone(),
                    reference.through_journal,
                ));
            }
        }
        let mut actual = std::collections::BTreeSet::new();
        for audit in &audits {
            if audit.through_journal == 0
                || audit.records.is_empty()
                || audit
                    .records
                    .iter()
                    .any(|record| record.journal_sequence > audit.through_journal)
                || !actual.insert((
                    audit.source_conversation_id.clone(),
                    audit.call_id.clone(),
                    audit.through_journal,
                ))
            {
                return Err(ConversationStoreError::InvalidReference(
                    "invalid or duplicate inherited Summary generation audit".into(),
                ));
            }
            let Some(crate::model::auxiliary::AuxiliaryAuditFact::CallStarted(call)) =
                audit.records.first().map(|record| &record.fact)
            else {
                return Err(ConversationStoreError::InvalidReference(
                    "inherited Summary generation audit lacks CallStarted".into(),
                ));
            };
            if call.call_id != audit.call_id || call.purpose.as_str() != "summary" {
                return Err(ConversationStoreError::InvalidReference(
                    "inherited generation audit is not the named Summary Call".into(),
                ));
            }
        }
        if actual != expected {
            return Err(ConversationStoreError::InvalidReference(
                "retained checkpoints and inherited Summary generation audits differ".into(),
            ));
        }
        self.auxiliary_audits = audits;
        Ok(self)
    }

    #[must_use]
    pub fn auxiliary_audits(&self) -> &[super::reading::AuxiliaryAuditSnapshot] {
        &self.auxiliary_audits
    }

    /// Attach immutable response provenance to this seed's exact canonical identities.
    /// # Errors
    /// Rejects duplicate/non-final Assistant anchors or invalid preceding input.
    pub fn with_completed_responses(
        mut self,
        responses: Vec<super::response::CompletedResponseProvenance>,
    ) -> Result<Self, ConversationStoreError> {
        let mut seen = std::collections::BTreeSet::new();
        let mut process_members = std::collections::BTreeSet::new();
        for response in &responses {
            let closing = self.canonical.iter().position(|message| matches!(message,
                MessageBlock::Assistant(assistant) if assistant.id == response.closing_message_id
                && !assistant.content.iter().any(|block| matches!(block, crate::message::types::AssistantContentBlock::ToolCall(_)))));
            let Some(closing) = closing.filter(|_| seen.insert(&response.closing_message_id))
            else {
                return Err(ConversationStoreError::InvalidReference(
                    "invalid or duplicate inherited response anchor".into(),
                ));
            };
            if !response
                .process_message_ids
                .contains(&response.closing_message_id)
                || response.process_message_ids.iter().any(|id| {
                    !process_members.insert(id)
                        || !self.canonical[..=closing].iter().any(|message| {
                            matches!(message,
                        MessageBlock::Assistant(assistant) if &assistant.id == id)
                        })
                })
            {
                return Err(ConversationStoreError::InvalidReference(
                    "invalid inherited process membership".into(),
                ));
            }
            if let Some(input) = &response.retry_message_id
                && !self.canonical[..closing].iter().any(|message| matches!(message,
                    MessageBlock::User(user) if user.id == *input && user.kind == crate::message::types::InboundKind::Message)) {
                return Err(ConversationStoreError::InvalidReference("inherited Retry input is not a retained preceding User".into()));
            }
        }
        self.completed_responses = responses;
        Ok(self)
    }

    /// Attach native reading ownership; unowned canonical content creates no turn.
    /// # Errors
    /// Rejects duplicate owners, foreign/duplicate members and invalid terminal locations.
    pub fn with_turns(
        mut self,
        turns: Vec<super::reading::TurnReadingProvenance>,
    ) -> Result<Self, ConversationStoreError> {
        let mut owners = std::collections::BTreeSet::new();
        let mut members = std::collections::BTreeSet::new();
        for turn in &turns {
            if let Some(reading) = turn
                .execution
                .as_ref()
                .and_then(|execution| execution.occupancy.as_ref())
            {
                reading.validate()?;
            }
            let unique_owner =
                owners.insert((turn.id.conversation_id.clone(), turn.id.attempt_id.clone()));
            let valid_members = turn.process_message_ids.iter().all(|id| {
                members.insert(id.clone()) && self.canonical.iter().any(|message| {
                    matches!(message, MessageBlock::Assistant(message) if &message.id == id)
                })
            });
            let valid_predecessor = turn.preceding_message_id.as_ref().is_none_or(|id| {
                self.canonical
                    .iter()
                    .any(|message| crate::conversation::message_id_of(message) == *id)
            });
            let valid_prompt = turn.prompt_message_id.as_ref().is_none_or(|id| {
                self.canonical.iter().any(
                    |message| matches!(message, MessageBlock::User(message) if &message.id == id),
                )
            });
            let valid_location = !turn.process_message_ids.is_empty()
                || !matches!(
                    turn.outcome,
                    super::reading::InheritedTurnOutcome::IncompleteAtCut
                        | super::reading::InheritedTurnOutcome::Completed
                );
            if !unique_owner
                || !valid_members
                || !valid_predecessor
                || !valid_prompt
                || !valid_location
            {
                return Err(ConversationStoreError::InvalidReference(
                    "invalid inherited turn provenance".into(),
                ));
            }
        }
        self.turns = turns;
        Ok(self)
    }

    #[must_use]
    pub fn turns(&self) -> &[super::reading::TurnReadingProvenance] {
        &self.turns
    }

    /// Finalized historical responses; not destination execution evidence.
    #[must_use]
    pub fn completed_responses(&self) -> &[super::response::CompletedResponseProvenance] {
        &self.completed_responses
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

/// One item in the derived durable transcript read model.
///
/// The item carries bodies only in the bounded read result. Durable storage
/// keeps the body in its owning Message Ledger, publication-audit, or Event
/// Journal domain and stores only a reference in the transcript ordering
/// spine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum TranscriptItem {
    /// Minimal retained lineage terminal, not a copied Event Journal.
    InheritedTurn {
        provenance: super::reading::TurnReadingProvenance,
    },
    /// Terminal execution fact, resolved from its Event Journal owner.
    AttemptTerminal { event: RuntimeEventEnvelope },
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
    /// Native call/result association resolved from the canonical ledger.
    pub tool_calls: Vec<TranscriptTool>,
    /// The stable exclusive cursor for this item.
    pub cursor: TranscriptCursor,
    /// The resolved read-model item.
    pub item: TranscriptItem,
}

/// One canonical call and its optional committed result, never client-assembled.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptTool {
    /// Canonical Assistant identity; provider call IDs can repeat across history.
    pub message_id: MessageId,
    /// Exact content position inside that Assistant.
    pub block_index: crate::message::types::ContentBlockIndex,
    pub call: crate::tools::types::ToolCall,
    pub result: Option<crate::tools::types::ToolExecutionResult>,
}

/// A bounded page of the derived durable transcript.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptPage {
    /// Last position of the immutable lineage seed, including terminal-only turns.
    /// Zero denotes an empty inherited prefix.
    pub inherited_through: TranscriptCursor,
    /// Items in chronological order within this page.
    pub entries: Vec<TranscriptEntry>,
    /// The cursor to pass as `before` for the next older page.
    pub next_cursor: Option<TranscriptCursor>,
}

/// One finite durable inspection seed, copied from a single database read cut.
/// Journal folding and decorations happen later through this fixed frontier.
pub struct InspectionSeed {
    pub messages: Vec<MessageBlock>,
    pub canonical: Vec<MessageBlock>,
    pub transcript: TranscriptPage,
    pub journal_through: u64,
}

/// Immutable membership evidence captured by the semantic projection. Mutable
/// pending bodies are copied; canonical/audit bodies retain their durable owner.
#[derive(Debug, Clone)]
pub struct TranscriptSnapshotCut {
    pub bootstrap_through: u64,
    pub journal_through: u64,
    pub messages: Vec<MessageId>,
    pub publications: Vec<PublicationStreamId>,
    pub pending: Vec<(TranscriptCursor, UserMessageBlock)>,
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

/// Accepts one subagent terminal publication through a store handle,
/// rejecting every other event payload and every non-runtime-authored
/// notice.
///
/// The narrow capability must not become a general multi-inbound seam: the
/// subagent plane may commit exactly its terminal report, its
/// runtime-authored terminal notice when the terminal succeeded, and the
/// one dependent terminal fact, in one transaction — nothing else
/// (Issue #192). The store itself owns the complete legal-state
/// validation; these checks are the capability's own cheap refusal.
fn accept_subagent_terminal_through(
    store: &(impl ConversationStore + ?Sized),
    notice: Option<InboundDraft>,
    draft: InboundDraft,
    event: RuntimeEventEnvelope,
) -> Result<
    (
        Option<AcceptedInbound>,
        AcceptedInbound,
        RuntimeEventEnvelope,
    ),
    ConversationStoreError,
> {
    let success = matches!(
        &event.event,
        crate::events::types::RuntimeEvent::SubagentTerminalPublished {
            state: crate::events::types::SubagentTerminalState::Succeeded,
            ..
        }
    );
    if !matches!(
        event.event,
        crate::events::types::RuntimeEvent::SubagentTerminalPublished { .. }
    ) {
        return Err(ConversationStoreError::InvalidReference(
            "the subagent terminal transition requires a subagent terminal fact".to_owned(),
        ));
    }
    if success != notice.is_some() {
        return Err(ConversationStoreError::InvalidReference(
            "exactly a successful subagent terminal carries the runtime-authored terminal notice"
                .to_owned(),
        ));
    }
    if let Some(notice) = &notice
        && (notice.source != crate::message::types::UserSource::Runtime
            || notice.kind != crate::message::types::InboundKind::Message)
    {
        return Err(ConversationStoreError::InvalidReference(
            "a subagent terminal notice is runtime-authored only".to_owned(),
        ));
    }
    store.accept_subagent_terminal(notice, draft, event)
}

fn commit_agent_activation_admission_through(
    store: &(impl ConversationStore + ?Sized),
    event: RuntimeEventEnvelope,
) -> Result<RuntimeEventEnvelope, ConversationStoreError> {
    if !matches!(event.event, RuntimeEvent::AgentActivationAdmission { .. }) {
        return Err(ConversationStoreError::InvalidReference(
            "Agent admission capability commits only exact staging facts".into(),
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
    authority: Option<&crate::runtime::subagent::DurableAgentAuthority>,
) -> Result<RuntimeEventEnvelope, ConversationStoreError> {
    if !matches!(
        event.event,
        crate::events::types::RuntimeEvent::SubagentOwnershipCommitted { .. }
    ) {
        return Err(ConversationStoreError::InvalidReference(
            "the subagent capability commits only a subagent ownership fact".to_owned(),
        ));
    }
    match authority {
        Some(authority) => store.append_agent_admission(event, authority),
        None => store.append_event(event),
    }
}

/// Commits exact native recovery proof without creating another child terminal
/// or canonical inbound message.
fn commit_subagent_physical_settlement_through(
    store: &(impl ConversationStore + ?Sized),
    event: RuntimeEventEnvelope,
) -> Result<RuntimeEventEnvelope, ConversationStoreError> {
    if !matches!(
        event.event,
        crate::events::types::RuntimeEvent::SubagentPhysicalSettlementProven { .. }
            | crate::events::types::RuntimeEvent::AgentActivationAdmission {
                phase: crate::events::types::AgentActivationAdmissionPhase::RolledBack {
                    physical_settlement_proven: true
                },
                ..
            }
    ) {
        return Err(ConversationStoreError::InvalidReference(
            "expected physical settlement proof".into(),
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
/// Nine — and only nine — Event Journal facts are reachable here, all
/// because they are inseparable from a detached execution's own durable
/// ownership:
///
/// ```text
/// commit_background_ownership  -> BackgroundExecutionCommitted   (background start commit)
/// commit_subagent_ownership    -> SubagentOwnershipCommitted     (subagent start commit)
/// accept_inbound_with_event    -> BackgroundTerminalPublished    (background terminal commit)
/// accept_subagent_terminal     -> SubagentTerminalPublished      (the one normal subagent
///                                  terminal authority, Issue #192)
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
    ///
    /// This transition owns exactly the detached background terminal domain
    /// (Issue #192): a [`RuntimeEvent::SubagentTerminalPublished`](crate::events::types::RuntimeEvent::SubagentTerminalPublished)
    /// payload is rejected — every normal subagent terminal fact commits
    /// through [`ConversationInboundCapability::accept_subagent_terminal`],
    /// the one semantic durable authority for that domain.
    fn accept_inbound_with_event(
        &self,
        draft: InboundDraft,
        event: RuntimeEventEnvelope,
    ) -> Result<(AcceptedInbound, RuntimeEventEnvelope), ConversationStoreError>;

    /// Atomically accepts one subagent terminal publication — the one
    /// durable authority for every normal `SubagentTerminalPublished` fact
    /// (Issue #192): on success, the runtime-authored terminal notice
    /// (the parent-model correlation projection naming the exact typed
    /// execution handle), ordered first; then the terminal report; then
    /// the dependent terminal fact — one transaction.
    ///
    /// Exactly a successful terminal carries the notice. The notice must be
    /// `UserSource::Runtime`-authored with `InboundKind::Message`: it
    /// states runtime-observed facts and must never be attributed to the
    /// child. The event references the report's accepted `MessageId`, so
    /// the terminal result remains the last item of the publication. The
    /// payload must be a
    /// [`RuntimeEvent::SubagentTerminalPublished`](crate::events::types::RuntimeEvent::SubagentTerminalPublished);
    /// every other event is rejected, and the store enforces the complete
    /// legal state — canonical identities and correlations, ownership-
    /// authorized provenance, and notice/workspace consistency — before
    /// accepting anything.
    fn accept_subagent_terminal(
        &self,
        notice: Option<InboundDraft>,
        draft: InboundDraft,
        event: RuntimeEventEnvelope,
    ) -> Result<
        (
            Option<AcceptedInbound>,
            AcceptedInbound,
            RuntimeEventEnvelope,
        ),
        ConversationStoreError,
    >;

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

    /// Commits only a durable precommit Agent activation staging fact.
    /// Reservation precedes physical staging; rollback records containment.
    fn commit_agent_activation_admission(
        &self,
        event: RuntimeEventEnvelope,
    ) -> Result<RuntimeEventEnvelope, ConversationStoreError>;

    /// Commits child ownership before delegation can begin semantic work.
    /// The payload must be `SubagentOwnershipCommitted`; initial native
    /// Agent admission also atomically captures private frozen authority.
    fn commit_subagent_ownership(
        &self,
        event: RuntimeEventEnvelope,
        authority: Option<&crate::runtime::subagent::DurableAgentAuthority>,
    ) -> Result<RuntimeEventEnvelope, ConversationStoreError>;

    /// Commits the durable terminal-settlement fact of a Workflow-owned
    /// child without creating a parent inbound notification. The payload
    /// must be a [`RuntimeEvent::SubagentTerminalSettled`](crate::events::types::RuntimeEvent::SubagentTerminalSettled).
    fn commit_subagent_terminal(
        &self,
        event: RuntimeEventEnvelope,
    ) -> Result<RuntimeEventEnvelope, ConversationStoreError>;

    /// Commits only a recovery owner's physical containment proof.
    fn commit_subagent_physical_settlement(
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
    ///
    /// The committed adoption fact travels back with the adopted items so
    /// its owner can publish it with its exact Journal sequence. Trace
    /// projects that fact as the ledger's User record, and the observation
    /// queue holds every Trace-affecting receipt until its owner publishes
    /// it; returning the envelope is what lets that publication happen.
    fn adopt_pending_batch(
        &self,
        watermark: InboundSequence,
        attempt_id: Option<AttemptId>,
    ) -> Result<AdoptedBatch, ConversationStoreError>;

    /// Edits exactly one pending text item using native compare-and-set.
    fn edit_pending(
        &self,
        expected: &PendingInboundRef,
        text: &str,
    ) -> Result<PendingMutationOutcome, ConversationStoreError>;
    /// Removes exactly one pending occurrence without cancelling execution.
    fn remove_pending(
        &self,
        expected: &PendingInboundRef,
    ) -> Result<PendingMutationOutcome, ConversationStoreError>;

    /// Reads all committed pending rows in sequence order for repair/bootstrap.
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
    /// Finite typed facts from original native J rows at one authentic cut.
    fn observation_trace_facts(
        &self,
        cut: &super::observation::ObservationReadCut,
        query: &super::observation::TraceFactQuery,
    ) -> Result<Vec<super::observation::TraceFactHeader>, ConversationStoreError>;
    /// None is original snapshot absence; an auxiliary summary hole is an error.
    fn observation_request_accepted_at(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        source_sequence: u64,
    ) -> Result<
        Option<crate::runtime_client::content::CanonicalContentLocator>,
        ConversationStoreError,
    >;
    fn observation_request_summary_at(
        &self,
        start_sequence: u64,
        through: u64,
    ) -> Result<Option<super::observation::RequestReadSummary>, ConversationStoreError>;
    fn observation_request_predecessor_at(
        &self,
        start_sequence: u64,
        through: u64,
    ) -> Result<Option<super::observation::TraceRequestPredecessorHeader>, ConversationStoreError>;
    fn observation_request_retry_failure_at(
        &self,
        start_sequence: u64,
        through: u64,
    ) -> Result<Option<crate::model::error::ModelErrorKind>, ConversationStoreError>;
    fn observation_trace_canonical_summary(
        &self,
        cut: &super::observation::ObservationReadCut,
        ledger_position: u64,
    ) -> Result<super::observation::TraceCanonicalSummary, ConversationStoreError>;
    /// Positive C result evidence is independent of a terminal J fact.
    fn observation_trace_tool_result(
        &self,
        cut: &super::observation::ObservationReadCut,
        occurrence: &crate::events::types::ToolObservationOccurrence,
    ) -> Result<Option<super::observation::TraceCanonicalSummary>, ConversationStoreError>;
    fn observation_trace_tool_proposal(
        &self,
        cut: &super::observation::ObservationReadCut,
        occurrence: &crate::events::types::ToolObservationOccurrence,
    ) -> Result<super::observation::TraceToolProposalSummary, ConversationStoreError>;
    /// Same-original-request catalog selection; a missing optional relationship is unavailable.
    fn observation_trace_tool_definition(
        &self,
        cut: &super::observation::ObservationReadCut,
        occurrence: &crate::events::types::ToolObservationOccurrence,
    ) -> Result<Option<crate::runtime_client::trace::TraceFieldSource>, ConversationStoreError>;
    /// Advancing original adoption ordinal membership at the exact J/C cut.
    fn observation_trace_adoption(
        &self,
        cut: &super::observation::ObservationReadCut,
        source_sequence: u64,
        offset: u64,
        limit: usize,
    ) -> Result<super::observation::TraceAdoptionPage, ConversationStoreError>;
    fn observation_request_context_at(
        &self,
        cut: &super::observation::ObservationReadCut,
        start_sequence: u64,
        offset: u64,
        limit: usize,
    ) -> Result<super::observation::TraceRequestContextPage, ConversationStoreError>;
    fn request_summary_header_at(
        &self,
        source_sequence: u64,
        through: u64,
    ) -> Result<super::observation::RequestObservationHeader, ConversationStoreError>;
    fn request_field_header_at(
        &self,
        source_sequence: u64,
        through: u64,
        path: &[String],
    ) -> Result<super::observation::RequestObservationField, ConversationStoreError>;
    fn request_field_page_at(
        &self,
        source_sequence: u64,
        through: u64,
        path: &[String],
        offset: u64,
        limit: usize,
    ) -> Result<super::observation::RequestObservationPage, ConversationStoreError>;
    fn request_value_read_at(
        &self,
        source_sequence: u64,
        through: u64,
        path: &[String],
        offset: u64,
        max_bytes: u32,
    ) -> Result<super::observation::ObservationValueChunk, ConversationStoreError>;
    fn request_outcome_header_at(
        &self,
        source_sequence: u64,
        through: u64,
    ) -> Result<Option<super::observation::RequestObservationOutcome>, ConversationStoreError>;
    fn request_outcome_read_at(
        &self,
        source_sequence: u64,
        through: u64,
        path: &[String],
        offset: u64,
        max_bytes: u32,
    ) -> Result<super::observation::ObservationValueChunk, ConversationStoreError>;
    /// Exact immutable predecessor from native request start ordering.
    fn request_previous_identity(
        &self,
        request: &RequestId,
        through: u64,
    ) -> Result<Option<RequestId>, ConversationStoreError>;
    /// Request terminal evidence at native J, separate from immutable input detail.
    fn request_outcome_header(
        &self,
        request: &RequestId,
        through: u64,
    ) -> Result<Option<super::observation::RequestObservationOutcome>, ConversationStoreError>;
    /// Exact normalized request terminal structural bytes; only fixed allowlisted paths.
    fn request_outcome_read(
        &self,
        request: &RequestId,
        through: u64,
        path: &[String],
        offset: u64,
        max_bytes: u32,
    ) -> Result<super::observation::ObservationValueChunk, ConversationStoreError>;
    /// Finite immutable field descriptor without loading its original bytes.
    fn request_field_header(
        &self,
        request: &RequestId,
        through: u64,
        path: &[String],
    ) -> Result<super::observation::RequestObservationField, ConversationStoreError>;
    /// Exact native request start identity, without reading frozen snapshot bodies.
    fn request_identity_at(
        &self,
        sequence: u64,
        through: u64,
    ) -> Result<Option<RequestId>, ConversationStoreError>;
    /// Frozen allowlisted request detail, captured at its original native start.
    fn request_summary_header(
        &self,
        request: &RequestId,
        through: u64,
    ) -> Result<super::observation::RequestObservationHeader, ConversationStoreError>;
    /// Indexed immediate children of one immutable request detail field.
    fn request_field_page(
        &self,
        request: &RequestId,
        through: u64,
        path: &[String],
        offset: u64,
        limit: usize,
    ) -> Result<super::observation::RequestObservationPage, ConversationStoreError>;
    /// Exact immutable allowlisted structural JSON bytes, with actual bounded physical work.
    fn request_value_read(
        &self,
        request: &RequestId,
        through: u64,
        path: &[String],
        offset: u64,
        max_bytes: u32,
    ) -> Result<super::observation::ObservationValueChunk, ConversationStoreError>;
    /// Validates immutable content against the server's observed native cut.
    fn validate_content_source_cut(
        &self,
        reference: &crate::runtime_client::content::ContentRef,
        cut: &super::observation::ObservationReadCut,
    ) -> Result<(), ConversationStoreError>;
    /// Original immutable owner field, selected by native provenance ordinal.
    fn observation_inherited_attempt_read(
        &self,
        cut: &super::observation::ObservationReadCut,
        kind: crate::runtime_client::response::InheritedAttemptIdentityKind,
        ordinal: u64,
        offset: u64,
        max_bytes: u32,
    ) -> Result<super::observation::ObservationValueChunk, ConversationStoreError>;
    /// Raw native-page fixture oracle; production uses exact-cut numeric selectors.
    #[cfg(test)]
    fn observation_inherited_native_oracle(
        &self,
        messages: &[MessageId],
    ) -> Result<super::observation::InheritedObservation, ConversationStoreError>;
    /// Exact inherited actual model names by advancing native ordinal.
    fn observation_inherited_models(
        &self,
        response_ordinal: u64,
        offset: u64,
        limit: usize,
    ) -> Result<super::observation::InheritedModelPage, ConversationStoreError>;
    /// Exact model JSON-string bytes under retained immutable lineage authority.
    fn observation_inherited_model_read(
        &self,
        response_ordinal: u64,
        model_ordinal: u64,
        offset: u64,
        max_bytes: u32,
    ) -> Result<super::observation::ObservationValueChunk, ConversationStoreError>;
    /// Exact normalized terminal diagnostic JSON bytes under the captured native J.
    fn observation_failure_value_read(
        &self,
        attempt: &AttemptId,
        through: u64,
        offset: u64,
        max_bytes: u32,
    ) -> Result<super::observation::ObservationValueChunk, ConversationStoreError>;
    /// Exact finite per-Attempt request/timing/usage aggregate at native J.
    fn response_identity_source(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        transcript_cursor: u64,
    ) -> Result<Option<crate::durable::observation::ResponseIdentitySource>, ConversationStoreError>;
    fn inherited_response_summary(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        ordinal: u64,
    ) -> Result<Option<crate::durable::observation::InheritedResponseHeader>, ConversationStoreError>;
    fn response_identity_fields(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        source: &crate::durable::observation::ResponseIdentitySource,
    ) -> Result<Vec<crate::durable::observation::ResponseIdentityField>, ConversationStoreError>;
    fn response_identity_read(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        source: &crate::durable::observation::ResponseIdentitySource,
        path: &[String],
        offset: u64,
        max_bytes: u32,
    ) -> Result<crate::durable::observation::ObservationValueChunk, ConversationStoreError>;
    /// Exact finite execution and member header from one original J row.
    /// Select the complete derived execution state through this authentic cut
    /// using the original numeric owner source; never an inline identity alias.
    fn observation_execution_at(
        &self,
        cut: &super::observation::ObservationReadCut,
        sequence: u64,
    ) -> Result<super::observation::AttemptExecutionSummary, ConversationStoreError>;
    fn observation_members_at(
        &self,
        cut: &super::observation::ObservationReadCut,
        sequence: u64,
    ) -> Result<super::observation::AttemptMemberCounts, ConversationStoreError>;
    fn observation_attempt_summary(
        &self,
        attempt: &AttemptId,
        through: u64,
    ) -> Result<super::observation::AttemptExecutionSummary, ConversationStoreError>;
    fn todo_observation_head(
        &self,
        cut: &super::observation::ObservationReadCut,
    ) -> Result<Option<NativeTodoObservation>, ConversationStoreError>;
    /// Discover a finite native source locator at the exact frozen J/L cut.
    fn observation_state_source(
        &self,
        domain: &str,
        cut: &super::observation::ObservationReadCut,
    ) -> Result<Option<String>, ConversationStoreError>;
    /// Finite source fields owned by current Goal, immutable canonical Todo, or Compaction J.
    fn observation_state_header(
        &self,
        domain: &str,
        source_id: &str,
        cut: &super::observation::ObservationReadCut,
        path: &[String],
    ) -> Result<super::observation::RequestObservationField, ConversationStoreError>;
    fn observation_state_page(
        &self,
        domain: &str,
        source_id: &str,
        cut: &super::observation::ObservationReadCut,
        path: &[String],
        offset: u64,
        limit: usize,
    ) -> Result<super::observation::RequestObservationPage, ConversationStoreError>;
    fn observation_state_value_read(
        &self,
        domain: &str,
        source_id: &str,
        cut: &super::observation::ObservationReadCut,
        path: &[String],
        offset: u64,
        max_bytes: u32,
    ) -> Result<super::observation::ObservationValueChunk, ConversationStoreError>;
    /// Native distinct actual model names, by advancing bounded ordinal.
    /// Numeric original owner selection, without loading its original identity.
    fn observation_models_at(
        &self,
        cut: &super::observation::ObservationReadCut,
        sequence: u64,
        offset: u64,
        limit: usize,
    ) -> Result<super::observation::AttemptModelNamePage, ConversationStoreError>;
    fn observation_model_read_at(
        &self,
        cut: &super::observation::ObservationReadCut,
        sequence: u64,
        ordinal: u64,
        offset: u64,
        max_bytes: u32,
    ) -> Result<super::observation::ObservationValueChunk, ConversationStoreError>;
    fn observation_failure_read_at(
        &self,
        cut: &super::observation::ObservationReadCut,
        sequence: u64,
        offset: u64,
        max_bytes: u32,
    ) -> Result<super::observation::ObservationValueChunk, ConversationStoreError>;
    fn observation_models_page(
        &self,
        attempt: &AttemptId,
        through: u64,
        offset: u64,
        limit: usize,
    ) -> Result<super::observation::AttemptModelNamePage, ConversationStoreError>;
    /// Exact immutable native model name JSON-string bytes, at most two finite rows.
    fn observation_model_value_read(
        &self,
        attempt: &AttemptId,
        through: u64,
        ordinal: u64,
        offset: u64,
        max_bytes: u32,
    ) -> Result<super::observation::ObservationValueChunk, ConversationStoreError>;
    /// Last ordinary canonical User at this request's exact native Surface cut.
    fn observation_request_retry_locator_at(
        &self,
        source_sequence: u64,
        through: u64,
    ) -> Result<
        Option<crate::runtime_client::content::CanonicalContentLocator>,
        ConversationStoreError,
    >;
    fn observation_request_retry_at(
        &self,
        source_sequence: u64,
        through: u64,
    ) -> Result<Option<MessageId>, ConversationStoreError>;
    fn observation_request_retry_locator(
        &self,
        request_id: &RequestId,
    ) -> Result<
        Option<crate::runtime_client::content::CanonicalContentLocator>,
        ConversationStoreError,
    >;
    fn observation_request_retry(
        &self,
        request_id: &RequestId,
    ) -> Result<Option<MessageId>, ConversationStoreError>;
    /// Exact process count/first-position evidence, without member body reads.
    fn observation_member_counts(
        &self,
        message_id: &MessageId,
        through: u64,
    ) -> Result<Option<super::observation::AttemptMemberCounts>, ConversationStoreError>;
    /// Same-owner finite membership at the frozen original Journal cut.
    fn observation_attempt_members(
        &self,
        attempt: &AttemptId,
        through: u64,
    ) -> Result<super::observation::AttemptMemberCounts, ConversationStoreError>;
    fn observation_transcript_process(
        &self,
        cut: &super::observation::ObservationReadCut,
        transcript: u64,
    ) -> Result<Option<crate::runtime_client::response::TurnProcessView>, ConversationStoreError>;
    fn audit_block_header_page(
        &self,
        cut: &super::observation::ObservationReadCut,
        transcript: u64,
        offset: u64,
        limit: usize,
    ) -> Result<super::observation::AuditBlockHeaderPage, ConversationStoreError>;
    /// Projection settlement head at this exact published native source cut.
    fn publication_settlement_head(
        &self,
        cut: &super::observation::ObservationReadCut,
    ) -> Result<
        Vec<crate::runtime_client::snapshot::RuntimeClientPublicationSettlementRecord>,
        ConversationStoreError,
    >;
    fn observation_trace_total(&self, through: u64) -> Result<u64, ConversationStoreError>;
    /// Finite occupancy evidence at the original native reading's journal cut.
    fn observation_occupancy(
        &self,
        through: u64,
    ) -> Result<
        Option<crate::runtime_client::occupancy::RuntimeClientContextOccupancy>,
        ConversationStoreError,
    >;
    /// One immutable compaction or inherited reading's original model field.
    fn observation_occupancy_model_value_read(
        &self,
        sequence: Option<u64>,
        through: u64,
        offset: u64,
        max_bytes: u32,
    ) -> Result<super::observation::ObservationValueChunk, ConversationStoreError>;
    /// Native-J scalar aggregate at an exact authoritative Journal frontier.
    fn observation_statistics(
        &self,
        through: u64,
    ) -> Result<crate::runtime_client::response::ConversationStatistics, ConversationStoreError>;
    /// Local child statistics and exact working intervals from same-J headers.
    fn observation_agent_statistics(
        &self,
        through: u64,
    ) -> Result<crate::runtime_client::agent_statistics::AgentStatistics, ConversationStoreError>;
    /// Mutable pending headers at one exact native mutation cut, by exclusive sequence.
    fn pending_header_page(
        &self,
        cut: &super::reading::ConversationReadCut,
        after_sequence: Option<u64>,
        limit: usize,
    ) -> Result<super::observation::PendingHeaderPage, ConversationStoreError>;
    /// Pure byte range over an exact still-pending `UserMessage` structural encoding.
    fn pending_value_read(
        &self,
        cut: &super::reading::ConversationReadCut,
        sequence: u64,
        offset: u64,
        max_bytes: u32,
    ) -> Result<super::observation::ObservationValueChunk, ConversationStoreError>;
    /// Native history/window positioning with only finite indexed display headers.
    fn read_window_headers(
        &self,
        at: &super::reading::ConversationWindowAt,
        limit: usize,
    ) -> Result<super::observation::ObservationWindow, ConversationStoreError>;
    /// Exact native T identity assigned to this J row, without decoding event bodies.
    fn event_transcript_cursor_at_sequence(
        &self,
        sequence: u64,
    ) -> Result<Option<TranscriptCursor>, ConversationStoreError>;
    /// Latest exact bounded native Journal display headers for named semantic families.
    /// Finite same-original-J kind; authorization never parses the original body.
    fn observation_event_kind(
        &self,
        sequence: u64,
        through: u64,
    ) -> Result<Option<String>, ConversationStoreError>;
    fn observation_event_witness(
        &self,
        sequence: u64,
        through: u64,
    ) -> Result<Option<crate::durable::observation::ObservationEventWitness>, ConversationStoreError>;
    fn event_value_page(
        &self,
        sequence: u64,
        through: u64,
        path: &[String],
        offset: u64,
        limit: usize,
    ) -> Result<crate::durable::observation::RequestObservationPage, ConversationStoreError>;
    fn event_value_read(
        &self,
        sequence: u64,
        through: u64,
        path: &[String],
        offset: u64,
        max_bytes: u32,
    ) -> Result<crate::durable::observation::ObservationValueChunk, ConversationStoreError>;
    fn latest_observation_events(
        &self,
        through: u64,
        kinds: &[String],
        attempt: Option<&AttemptId>,
        limit: usize,
    ) -> Result<Vec<super::observation::ObservationEventHeader>, ConversationStoreError>;
    /// Exact committed call occurrence under one native Attempt and frozen read cut.
    fn tool_call_occurrence_at(
        &self,
        call_id: &crate::runtime::identity::ToolCallId,
        attempt: &AttemptId,
        cut: &super::observation::ObservationReadCut,
    ) -> Result<Option<crate::message::types::ToolCallOccurrenceRef>, ConversationStoreError>;
    /// Canonical occurrence associations and finite result descriptors at one native cut.
    fn tool_association_headers(
        &self,
        cut: &super::observation::ObservationReadCut,
        message_id: &MessageId,
        offset: u64,
        limit: usize,
    ) -> Result<super::observation::ToolAssociationHeaderPage, ConversationStoreError>;
    /// An exact native canonical descriptor seek, with no body deserialization.
    fn tool_association_headers_by_locator(
        &self,
        cut: &super::observation::ObservationReadCut,
        locator: &crate::runtime_client::content::CanonicalContentLocator,
        offset: u64,
        limit: usize,
    ) -> Result<super::observation::ToolAssociationHeaderPage, ConversationStoreError>;
    /// Immutable canonical identity lookup from the authoritative Ledger,
    /// independent of optional display headers and body range indexes.
    fn ordinary_user_message(
        &self,
        locator: &crate::runtime_client::content::CanonicalContentLocator,
    ) -> Result<bool, ConversationStoreError>;
    fn message_ledger_position(
        &self,
        message_id: &MessageId,
    ) -> Result<Option<u64>, ConversationStoreError>;
    fn message_header(
        &self,
        message_id: &MessageId,
    ) -> Result<Option<super::observation::MessageHeader>, ConversationStoreError>;
    /// Advancing immutable canonical content-block descriptors at the captured native cut.
    fn message_block_header_page(
        &self,
        cut: &super::observation::ObservationReadCut,
        message_id: &MessageId,
        offset: u64,
        limit: usize,
    ) -> Result<super::observation::MessageBlockHeaderPage, ConversationStoreError>;
    fn message_block_header_page_by_locator(
        &self,
        cut: &super::observation::ObservationReadCut,
        locator: &crate::runtime_client::content::CanonicalContentLocator,
        offset: u64,
        limit: usize,
    ) -> Result<super::observation::MessageBlockHeaderPage, ConversationStoreError>;
    fn observation_tool_trace_sequence(
        &self,
        cut: &super::observation::ObservationReadCut,
        ledger_position: u64,
        block_index: crate::message::types::ContentBlockIndex,
    ) -> Result<Option<u64>, ConversationStoreError>;
    /// Captures only native finite metadata; never reads Surface/body JSON.
    fn observation_read_cut(
        &self,
    ) -> Result<super::observation::ObservationReadCut, ConversationStoreError>;
    /// Canonical display headers; None selects the newest bounded tail.
    fn message_header_page(
        &self,
        cut: &super::observation::ObservationReadCut,
        offset: Option<u64>,
        limit: usize,
    ) -> Result<super::observation::MessageHeaderPage, ConversationStoreError>;
    /// Transcript headers from its ordering spine, without source-body reads.
    fn transcript_header_page(
        &self,
        cut: &super::observation::ObservationReadCut,
        before: Option<TranscriptCursor>,
        limit: usize,
    ) -> Result<super::observation::TranscriptHeaderPage, ConversationStoreError>;

    /// Freeze the immutable compaction source domains in one short read transaction.
    fn compaction_resource_cut(
        &self,
    ) -> Result<crate::context::resources::ResourceReadCut, ConversationStoreError>;
    /// Bounded detached canonical reads, through the Resource Read Cut.
    fn compaction_message_page(
        &self,
        cut: &crate::context::resources::ResourceReadCut,
        after: crate::context::resources::LedgerFrontier,
        ids: Option<&[MessageId]>,
        limits: crate::context::resources::ResourceLimits,
    ) -> Result<crate::context::resources::ResourcePage<MessageBlock>, ConversationStoreError>;
    /// Keyed immutable provenance integrity, with body only when explicitly required.
    fn compaction_dependency(
        &self,
        cut: &crate::context::resources::ResourceReadCut,
        source: &crate::model::projected::CanonicalMessageRef,
        body_required: bool,
        limits: crate::context::resources::ResourceLimits,
    ) -> Result<crate::context::checkpoint::CanonicalDependency, ConversationStoreError>;
    /// Detached typed trajectory metadata and exact physical tool occurrences.
    fn compaction_trajectory_page(
        &self,
        cut: &crate::context::resources::ResourceReadCut,
        after: crate::context::resources::LedgerFrontier,
        limits: crate::context::resources::ResourceLimits,
    ) -> Result<
        crate::context::resources::ResourcePage<crate::context::resources::TrajectoryMessage>,
        ConversationStoreError,
    >;
    /// Bounded real Journal facts, through the Resource Read Cut.
    fn compaction_event_page(
        &self,
        cut: &crate::context::resources::ResourceReadCut,
        after: crate::context::resources::JournalFrontier,
        limits: crate::context::resources::ResourceLimits,
    ) -> Result<crate::context::resources::ResourcePage<RuntimeEventEnvelope>, ConversationStoreError>;
    /// One exact snapshot of native reading authority.
    fn conversation_read_cut(
        &self,
    ) -> Result<super::reading::ConversationReadCut, ConversationStoreError>;
    /// Finite original turn display headers; omitted offset selects newest page.
    fn observation_turns(
        &self,
        offset: Option<u64>,
        limit: usize,
    ) -> Result<crate::runtime_client::turn::ObservedTurnPage, ConversationStoreError>;
    /// Select one committed successful Tool delivery without loading its body.
    fn observation_file_delivery(
        &self,
        locator: &crate::runtime_client::content::CanonicalContentLocator,
        index: u64,
    ) -> Result<super::observation::NativeFileDelivery, ConversationStoreError>;
    fn observation_deliveries(
        &self,
        cut: &super::observation::ObservationReadCut,
        locator: &crate::runtime_client::content::CanonicalContentLocator,
        offset: u64,
        limit: usize,
    ) -> Result<crate::runtime_client::delivery::RuntimeClientDeliveryHead, ConversationStoreError>;
    fn observation_delivery_read(
        &self,
        cut: &super::observation::ObservationReadCut,
        locator: &crate::runtime_client::content::CanonicalContentLocator,
        index: u64,
        offset: u64,
        max_bytes: u32,
    ) -> Result<super::observation::ObservationValueChunk, ConversationStoreError>;
    /// Bounded native Attempt outline, including immutable copied origins.
    /// Native opening-input ownership for one actual Attempt at a Journal cut.
    fn attempt_prompt(
        &self,
        attempt: &AttemptId,
        through: u64,
    ) -> Result<Option<MessageId>, ConversationStoreError>;
    fn conversation_turns(
        &self,
        offset: usize,
        limit: usize,
    ) -> Result<super::reading::ConversationTurnPage, ConversationStoreError>;
    /// The newest transcript page, or the page before a boundary, at the current cut.
    fn conversation_window(
        &self,
        at: &super::reading::ConversationWindowAt,
        limit: usize,
    ) -> Result<super::reading::DurableConversationWindow, ConversationStoreError>;
    /// Exact local execution owners of these canonical Assistant identities.
    fn transcript_attempts(
        &self,
        messages: &[MessageId],
        through: u64,
    ) -> Result<Vec<AttemptId>, ConversationStoreError>;
    /// Only copied response provenance intersecting exact requested members.
    fn inherited_responses_for(
        &self,
        messages: &[MessageId],
    ) -> Result<Vec<super::response::CompletedResponseProvenance>, ConversationStoreError>;
    /// Captures lineage authority (R, C) in one native snapshot: revision R owns
    /// structure, invocation-time read cut C owns temporal provenance. The same
    /// R at a newer C may observe newer outcomes; captured (R, C) is immutable.
    /// Select a historical activation independently of its canonical Ledger frontier.
    fn read_projection_lineage_cut(
        &self,
        ledger: crate::context::resources::LedgerFrontier,
        activation: crate::context::checkpoint::ProjectionActivationId,
    ) -> Result<super::reading::LineageReadCut, ConversationStoreError>;
    fn read_lineage_cut(
        &self,
        revision: SurfaceRevision,
    ) -> Result<super::reading::LineageReadCut, ConversationStoreError>;
    fn load_inherited_turns(
        &self,
    ) -> Result<Vec<super::reading::TurnReadingProvenance>, ConversationStoreError>;
    fn inherited_turns_for(
        &self,
        messages: &[MessageId],
    ) -> Result<Vec<super::reading::TurnReadingProvenance>, ConversationStoreError>;
    /// Private executable Agent authority, never copied into public history or events.
    fn load_agent_authority(
        &self,
        agent_id: &crate::runtime::identity::AgentId,
    ) -> Result<crate::runtime::subagent::DurableAgentAuthority, ConversationStoreError>;

    /// Atomically admit one private authority and its public ownership reference.
    fn append_agent_admission(
        &self,
        event: RuntimeEventEnvelope,
        authority: &crate::runtime::subagent::DurableAgentAuthority,
    ) -> Result<RuntimeEventEnvelope, ConversationStoreError>;

    /// Current Goal state, independent of history and Event Journal.
    fn load_goal(&self) -> Result<Option<crate::goal::GoalSnapshot>, ConversationStoreError> {
        Err(ConversationStoreError::Storage(
            "Goal state is unsupported by this store".to_owned(),
        ))
    }

    /// Finite same-transaction Goal display header, without reading original body.
    fn goal_observation_view(
        &self,
    ) -> Result<crate::runtime_client::goal::RuntimeClientGoalView, ConversationStoreError>;

    /// One authoritative Goal mutation, validated and revisioned atomically.
    fn write_goal(
        &self,
        _write: crate::goal::GoalWrite,
    ) -> Result<
        (
            crate::goal::GoalResult,
            Option<std::sync::Arc<GoalObservationReceipt>>,
        ),
        ConversationStoreError,
    > {
        Err(ConversationStoreError::Storage(
            "Goal state is unsupported by this store".to_owned(),
        ))
    }

    /// Atomically CAS Goal accounting and accept ordinary pending inbound.
    /// Pending inbound or stale state wins without consuming a round.
    fn accept_goal_round(
        &self,
        _expected: &crate::goal::GoalRef,
        _draft: InboundDraft,
    ) -> Result<Option<AcceptedInbound>, ConversationStoreError> {
        Err(ConversationStoreError::Storage(
            "Goal admission is unsupported by this store".to_owned(),
        ))
    }
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
    /// in the same transaction as the Pending Inbound row. It owns no other
    /// domain (Issue #192): a `SubagentTerminalPublished` payload is
    /// rejected, because every normal subagent terminal fact commits
    /// through [`ConversationStore::accept_subagent_terminal`] — the one
    /// semantic durable authority for that domain.
    fn accept_inbound_with_event(
        &self,
        draft: InboundDraft,
        event: RuntimeEventEnvelope,
    ) -> Result<(AcceptedInbound, RuntimeEventEnvelope), ConversationStoreError>;

    /// Atomically accepts one subagent terminal publication — the one
    /// durable authority for every normal `SubagentTerminalPublished` fact
    /// (Issue #192). On success: the runtime-authored terminal notice first
    /// (the parent-model correlation projection naming the exact typed
    /// execution handle), then the child-authored terminal report, then
    /// the dependent terminal fact — one transaction, so a crash can never
    /// observe the report without its notice or the fact without either.
    /// A failed/cancelled/interrupted terminal carries no notice: its
    /// report is already the one runtime-authored terminal message.
    ///
    /// The transition is semantically closed. Before accepting anything it
    /// validates, against the committed ownership fact: the canonical event
    /// identity, the canonical report message identity and producer
    /// correlation, the state/provenance pairing (a successful report is
    /// authored by the committed child agent; every other terminal is
    /// runtime-authored), the notice rules (exactly a success carries it,
    /// with the canonical runtime-authored identity, correlation, and
    /// text), and the workspace-resource consistency (the retained semantic
    /// fact exactly when the terminal resource is `Retained`, and never a
    /// success over an unresolved resource). Both drafts carry their
    /// deterministic producer correlations, so an ambiguous commit retries
    /// idempotently and a pair retry with only one side durable is
    /// rejected.
    fn accept_subagent_terminal(
        &self,
        notice: Option<InboundDraft>,
        draft: InboundDraft,
        event: RuntimeEventEnvelope,
    ) -> Result<
        (
            Option<AcceptedInbound>,
            AcceptedInbound,
            RuntimeEventEnvelope,
        ),
        ConversationStoreError,
    >;

    /// Selects the currently pending items as one finite watermark-bounded
    /// batch, without consuming them.
    ///
    /// # Errors
    ///
    /// Returns [`ConversationStoreError::Storage`] on a backend read failure.
    fn select_pending_batch(&self) -> Result<Option<PendingBatch>, ConversationStoreError>;

    /// Atomically claims current pending rows through the selected watermark.
    /// The transaction constructs the answer obligation from those exact rows,
    /// appends them to Ledger/Surface, removes pending entries, and returns the
    /// committed receipt. Selection supplies only an upper bound: its payload
    /// must never be installed or published. An empty receipt commits no event.
    ///
    /// # Errors
    /// Returns a storage failure if the atomic claim cannot commit.
    fn adopt_pending_batch(
        &self,
        watermark: InboundSequence,
        attempt_id: Option<AttemptId>,
    ) -> Result<AdoptedBatch, ConversationStoreError>;

    /// Edits exactly one pending text item using native compare-and-set.
    fn edit_pending(
        &self,
        expected: &PendingInboundRef,
        text: &str,
    ) -> Result<PendingMutationOutcome, ConversationStoreError>;
    /// Removes exactly one pending occurrence without cancelling execution.
    fn remove_pending(
        &self,
        expected: &PendingInboundRef,
    ) -> Result<PendingMutationOutcome, ConversationStoreError>;

    /// Reads all committed pending rows in sequence order for repair/bootstrap.
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

    /// Reads the native initialization marker paired with its immutable bootstrap identity.
    /// An initialized empty or inherited lineage is initialized; this never infers
    /// readiness from current Ledger/Surface emptiness or hydrates bootstrap payloads.
    /// Existing native bootstrap operations retain their complete payload validation.
    ///
    /// # Errors
    ///
    /// Returns [`ConversationStoreError::Storage`] for missing or inconsistent
    /// marker/identity metadata, or a backend read failure.
    fn is_initialized(&self) -> Result<bool, ConversationStoreError>;

    /// Initializes the durable Ledger and Surface from one immutable
    /// [`LineageSeed`] and establishes its exact immutable bootstrap
    /// identity. Reopening verifies the original identity instead of
    /// inferring it from current rows. The first call atomically commits the
    /// seed and the identity; every later call must re-supply the exact
    /// original canonical history and response provenance. An explicitly empty seed is valid and
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
    /// re-supplied canonical history or response provenance differs, and
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
    fn initialize(&self, messages: &[MessageBlock]) -> Result<(), ConversationStoreError>;

    /// Loads the immutable bootstrap history originally supplied to
    /// [`ConversationStore::initialize_lineage`] — its canonical part, which
    /// is the canonical portion of bootstrap identity. Reopening a lineage
    /// must validate against this prefix, not against its later canonical
    /// transcript, and not against the Surface projection the seed also
    /// carried: that projection is durable state the store already holds, so
    /// a reopen does not re-supply Surface operations. `initialize` reconstructs
    /// the immutable Surface prefix and validates inherited provenance and occupancy.
    fn load_bootstrap_history(&self) -> Result<Vec<MessageBlock>, ConversationStoreError> {
        self.load_canonical()
    }

    /// Immutable Context Engine derivative, committed atomically with the selected seed.
    /// No local execution evidence or inherited history is loaded by this lookup.
    fn load_inherited_occupancy(
        &self,
    ) -> Result<Option<crate::context::occupancy::ContextOccupancy>, ConversationStoreError>;

    /// Read immutable lineage-bootstrap response provenance, never execution events.
    fn load_inherited_responses(
        &self,
    ) -> Result<Vec<super::response::CompletedResponseProvenance>, ConversationStoreError>;

    /// Loads the current Surface head and checkpoint metadata without
    /// materializing historical revisions.
    fn load_head(&self) -> Result<DurableConversationHead, ConversationStoreError>;

    /// Prove that every exact Ledger identity is an ordinary human input.
    /// Reads authorship metadata only; content is not part of this projection.
    fn messages_are_human_inputs(&self, ids: &[MessageId]) -> Result<bool, ConversationStoreError>;

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

    /// Exact native transcript position of a canonical process message.
    fn message_transcript_cursor(
        &self,
        message_id: &MessageId,
    ) -> Result<Option<TranscriptCursor>, ConversationStoreError>;

    /// Exact transcript position of a journal-backed terminal control.
    fn event_transcript_cursor(
        &self,
        event_id: &EventId,
    ) -> Result<Option<TranscriptCursor>, ConversationStoreError>;

    /// Exact immutable revision that first appended this canonical message.
    /// Compaction and later appends do not change this historical boundary.
    fn transcript_tool_headers(
        &self,
        cut: &super::observation::ObservationReadCut,
        locator: &crate::runtime_client::content::CanonicalContentLocator,
        offset: u64,
        limit: usize,
    ) -> Result<super::observation::NativeToolObservationPage, ConversationStoreError>;
    fn message_append_revision_by_locator(
        &self,
        locator: &crate::runtime_client::content::CanonicalContentLocator,
    ) -> Result<Option<SurfaceRevision>, ConversationStoreError>;
    fn message_append_revision(
        &self,
        message_id: &MessageId,
    ) -> Result<Option<SurfaceRevision>, ConversationStoreError>;

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
    fn surface_boundary_header_page(
        &self,
        through: SurfaceRevision,
        offset: u64,
        limit: usize,
    ) -> Result<super::observation::SurfaceBoundaryHeaderPage, ConversationStoreError>;
    fn surface_boundary_message_id(
        &self,
        boundary_revision: SurfaceRevision,
        locator: &crate::runtime_client::content::CanonicalContentLocator,
    ) -> Result<MessageId, ConversationStoreError>;
    /// Resolves a canonical boundary of any role from the original Ledger at
    /// the selected Surface revision, independently of display read indexes.
    /// The lineage mutation still validates its exact boundary semantics.
    fn canonical_boundary_message_id(
        &self,
        boundary_revision: SurfaceRevision,
        locator: &crate::runtime_client::content::CanonicalContentLocator,
    ) -> Result<MessageId, ConversationStoreError>;
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
    /// through its own atomic transition. Compaction uses `commit_projection`
    /// and creates no canonical Ledger message. The store remains the historical Ledger authority; hot runtime
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

    /// Canonical physical Tool occurrences still lacking a committed result.
    /// Projection selection cannot settle or reopen these execution obligations.
    /// Returns at most `limit` rows (1..=8192) from the existing occurrence index.
    fn unsettled_canonical_tool_calls(
        &self,
        limit: usize,
    ) -> Result<
        Vec<(
            crate::message::types::ToolCallOccurrenceRef,
            crate::runtime::identity::ToolCallId,
        )>,
        ConversationStoreError,
    >;

    /// Read the current independent projection head; a dangling head fails closed.
    fn projected_history(
        &self,
    ) -> Result<super::projection::HistoricalModelInput, ConversationStoreError>;
    /// Bounded original provenance of one actual immutable activation item.
    fn observation_projection_provenance_read(
        &self,
        cut: &super::observation::ObservationReadCut,
        activation: u64,
        item: u64,
        path: &[String],
        offset: u64,
        max_bytes: u32,
    ) -> Result<super::observation::ObservationValueChunk, ConversationStoreError>;
    /// Finite original activation view; never hydrates checkpoint candidate bodies.
    fn observation_projection_head(
        &self,
        through: u64,
    ) -> Result<
        Option<crate::runtime_client::snapshot::RuntimeClientProjectionHeadView>,
        ConversationStoreError,
    >;
    fn projection_at_journal_cut(
        &self,
        through: u64,
    ) -> Result<Option<super::projection::ProjectionCheckpoint>, ConversationStoreError>;
    /// Read one immutable checkpoint at the frozen resource cut with early byte admission.
    fn compaction_checkpoint_at(
        &self,
        cut: &crate::context::resources::ResourceReadCut,
        id: &crate::runtime::identity::CheckpointId,
        limits: crate::context::resources::ResourceLimits,
    ) -> Result<super::projection::ProjectionCheckpoint, ConversationStoreError>;
    fn compaction_request(
        &self,
        cut: &crate::context::resources::ResourceReadCut,
        request: &crate::runtime::identity::RequestId,
        limits: crate::context::resources::ResourceLimits,
    ) -> Result<crate::model::RequestSnapshot, ConversationStoreError>;
    fn projection_head(
        &self,
    ) -> Result<Option<super::projection::ProjectionCheckpoint>, ConversationStoreError>;
    fn projection_checkpoint(
        &self,
        id: &crate::runtime::identity::CheckpointId,
    ) -> Result<super::projection::ProjectionCheckpoint, ConversationStoreError>;
    fn projection_timeline(
        &self,
        after: crate::context::checkpoint::ProjectionActivationId,
        limit: usize,
    ) -> Result<Vec<super::projection::ProjectionCheckpoint>, ConversationStoreError>;
    /// Decide published-cut eligibility before immutable checkpoint hydration.
    fn projection_timeline_at_journal_cut(
        &self,
        through: u64,
        after: crate::context::checkpoint::ProjectionActivationId,
        limit: usize,
    ) -> Result<super::projection::ProjectionTimelinePage, ConversationStoreError>;
    fn projection_checkpoint_at_journal_cut(
        &self,
        through: u64,
        id: &crate::runtime::identity::CheckpointId,
    ) -> Result<super::projection::ProjectionCheckpoint, ConversationStoreError>;
    fn projection_owner(
        &self,
        attempt: Option<&crate::runtime::identity::AttemptId>,
    ) -> Result<super::projection::ProjectionOwner, ConversationStoreError>;
    fn projection_receipt(
        &self,
        operation: &crate::runtime::identity::CompactionOperationId,
        fingerprint: &str,
    ) -> Result<Option<super::projection::ProjectionCommitReceipt>, ConversationStoreError>;
    fn commit_projection(
        &self,
        input: super::projection::ProjectionCommitInput,
    ) -> Result<super::projection::ProjectionCommitReceipt, ConversationStoreError>;

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

    /// Capture the durable inspection authorities in one read transaction.
    fn load_inspection_seed(&self) -> Result<InspectionSeed, ConversationStoreError>;

    /// Materialize the newest bounded page belonging to a captured semantic cut.
    /// Later commits, pending edits/removals and Tool results cannot enter it.
    fn load_transcript_snapshot(
        &self,
        cut: &TranscriptSnapshotCut,
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

    /// Reads the ownership recorded by the exact request-start Journal fact.
    /// Missing or inconsistent ownership for an existing request is an error.
    fn load_request_owner(
        &self,
        request_id: &RequestId,
    ) -> Result<Option<ModelRequestOwner>, ConversationStoreError>;

    /// Commits one unique auxiliary Call start with bounded attribution and
    /// exact frozen semantic/invocation fingerprints.
    fn commit_auxiliary_call_start(
        &self,
        start: &AuxiliaryCallStart,
        timestamp: DateTime<Utc>,
    ) -> Result<crate::durable::AuxiliaryCallStartCommit, crate::durable::AuxiliaryWriteFailure>;

    /// Commits one sequential physical request start under an active Call.
    fn commit_auxiliary_request_start(
        &self,
        start: &AuxiliaryRequestStart,
        timestamp: DateTime<Utc>,
    ) -> Result<crate::durable::AuxiliaryRequestStartCommit, crate::durable::AuxiliaryWriteFailure>;

    /// Atomically commits the final open request terminal and Call terminal.
    /// Construct both with [`crate::durable::auxiliary::auxiliary_settlement_envelope`]
    /// and retain them unchanged for retry and confirmation.
    fn commit_auxiliary_request_settlement(
        &self,
        request_terminal: RuntimeEventEnvelope,
        call_settled: RuntimeEventEnvelope,
    ) -> Result<crate::durable::AuxiliarySettlementCommit, crate::durable::AuxiliaryWriteFailure>;

    /// Construct the terminal with [`crate::durable::auxiliary::auxiliary_settlement_envelope`].
    /// Settles one active zero-request Call. No physical request or primary
    /// Snapshot is fabricated. Intermediate request transitions extend this
    /// contract separately before a settled-request backoff cancellation.
    fn commit_auxiliary_call_settlement(
        &self,
        call_settled: RuntimeEventEnvelope,
    ) -> Result<crate::durable::AuxiliarySettlementCommit, crate::durable::AuxiliaryWriteFailure>;

    /// Reads exact authoritative confirmation of one Call start.
    fn confirm_auxiliary_call_start(
        &self,
        expected: &AuxiliaryCallStart,
    ) -> Result<
        AuxiliaryCommitConfirmation<crate::durable::AuxiliaryCallStartCommit>,
        ConversationStoreError,
    >;

    /// Reads exact authoritative confirmation of one request start.
    fn confirm_auxiliary_request_start(
        &self,
        expected: &AuxiliaryRequestStart,
    ) -> Result<
        AuxiliaryCommitConfirmation<crate::durable::AuxiliaryRequestStartCommit>,
        ConversationStoreError,
    >;

    /// Reads exact authoritative confirmation of one atomic settlement pair.
    fn confirm_auxiliary_settlement(
        &self,
        request_terminal: &RuntimeEventEnvelope,
        call_settled: &RuntimeEventEnvelope,
    ) -> Result<
        AuxiliaryCommitConfirmation<crate::durable::AuxiliarySettlementCommit>,
        ConversationStoreError,
    >;

    /// Reads exact authoritative confirmation of one Call-only settlement.
    fn confirm_auxiliary_call_settlement(
        &self,
        call_settled: &RuntimeEventEnvelope,
    ) -> Result<
        AuxiliaryCommitConfirmation<crate::durable::AuxiliarySettlementCommit>,
        ConversationStoreError,
    >;

    /// Captures the current Journal cut for bounded auxiliary audit reads.
    fn auxiliary_audit_cut(&self) -> Result<AuxiliaryAuditCut, ConversationStoreError>;

    /// Returns an advancing dense-offset page of auxiliary audit facts at `cut`.
    fn read_auxiliary_audit(
        &self,
        cut: AuxiliaryAuditCut,
        after_offset: Option<u64>,
        limit: usize,
    ) -> Result<AuxiliaryAuditPage, ConversationStoreError>;

    /// Returns the dense auxiliary audit total visible at `cut`.
    fn auxiliary_audit_total(&self, cut: AuxiliaryAuditCut) -> Result<u64, ConversationStoreError>;

    /// Reads one keyed Call audit projection at `cut`.
    fn load_auxiliary_call(
        &self,
        call_id: &crate::runtime::identity::AuxiliaryCallId,
        cut: AuxiliaryAuditCut,
    ) -> Result<Option<AuxiliaryCallAudit>, ConversationStoreError>;

    /// Reads one keyed physical-request audit projection at `cut`.
    fn load_auxiliary_request(
        &self,
        request_id: &RequestId,
        cut: AuxiliaryAuditCut,
    ) -> Result<Option<AuxiliaryRequestAudit>, ConversationStoreError>;

    /// Reads at most `limit` local requests of one Call in start order at `cut`.
    /// Pass the last returned `started_sequence` to advance; an empty page ends
    /// iteration. A supplied cursor must name a visible request start of this
    /// Call. Later facts and inherited source audit never enter this lookup.
    /// Limits use [`crate::model::auxiliary::AUXILIARY_AUDIT_PAGE_LIMIT_MAX`].
    fn read_auxiliary_call_requests(
        &self,
        call_id: &crate::runtime::identity::AuxiliaryCallId,
        cut: AuxiliaryAuditCut,
        after_started_sequence: Option<u64>,
        limit: usize,
    ) -> Result<Vec<AuxiliaryRequestAudit>, ConversationStoreError>;

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

    /// Mark only an existing native owner's committed publication range ready.
    /// Readiness does not authorize model-output bytes or acknowledge a client.
    fn mark_observation_ready(&self, first: u64, last: u64) -> Result<(), ConversationStoreError>;
    fn mark_observation_direct(&self, first: u64, last: u64) -> Result<(), ConversationStoreError>;

    /// Last contiguous ready Journal position within a committed source range.
    fn observation_ready_frontier(
        &self,
        after: u64,
        through: u64,
    ) -> Result<u64, ConversationStoreError>;

    /// Publish one exact native transcript position; removed pending positions are retired.
    fn mark_observation_transcript_ready(&self, cursor: u64) -> Result<(), ConversationStoreError>;
    /// Publish only canonical positions owned by this exact atomic C ledger range.
    fn mark_canonical_transcript_ready(
        &self,
        first_ledger: u64,
        last_ledger: u64,
    ) -> Result<(), ConversationStoreError>;
    /// Contiguous owner-published prefix across retained native transcript positions.
    fn observation_transcript_ready_frontier(
        &self,
        after: u64,
        through: u64,
    ) -> Result<u64, ConversationStoreError>;

    /// Capture the native read cut and owner-published T prefix in one read snapshot.
    fn transcript_source_ready(
        &self,
    ) -> Result<super::observation::TranscriptSourceReady, ConversationStoreError>;

    /// Atomically install the leaf Journal observer and capture its initial prefix.
    fn observe_journal(
        &self,
        observer: std::sync::Arc<dyn super::presentation::JournalObserver>,
    ) -> Result<u64, ConversationStoreError>;

    /// Isolated cancellation scope for presentation reads. It shares storage,
    /// never execution ownership, and must be closed before joining its tasks.
    fn presentation_reader(&self) -> std::sync::Arc<dyn ConversationStore>;

    /// Cancel this reader's connection waits and running `SQLite` queries.
    fn close_presentation_reader(&self);

    #[cfg(test)]
    fn park_presentation_read_until_cancelled(&self, entered: tokio::sync::oneshot::Sender<()>);

    /// Latest committed Journal position, read without enumerating history.
    fn presentation_frontier(&self) -> Result<u64, ConversationStoreError>;

    /// Indexed, bounded execution-fact reads for presentation consumers.
    /// `before` is exclusive; `through` fixes a durable read cut. This is an
    /// internal Journal query, never a public cursor or a semantic authority.
    fn read_presentation_events(
        &self,
        query: &super::presentation::FactQuery,
    ) -> Result<Vec<RuntimeEventEnvelope>, ConversationStoreError>;

    /// Reads a bounded Event Journal page in stable sequence order.
    fn read_events(
        &self,
        after_sequence: Option<u64>,
        limit: usize,
    ) -> Result<EventPage, ConversationStoreError>;

    /// Latest byte descriptor from a retained source, without loading its body.
    /// Exact committed frame/immutable revision evidence, never the latest staged head.
    /// Reads a native P-indexed block page at an explicitly observed release revision.
    fn live_block_header_page(
        &self,
        source: &crate::runtime_client::content::ContentSource,
        through_revision: u64,
        offset: u64,
        limit: usize,
    ) -> Result<crate::runtime_client::content::LiveBlockHeaderPage, ConversationStoreError>;
    fn content_descriptor_at(
        &self,
        source: &crate::runtime_client::content::ContentSource,
        representation: crate::runtime_client::content::ContentRepresentation,
        revision: u64,
    ) -> Result<Option<crate::runtime_client::content::ContentRef>, ConversationStoreError>;
    fn content_descriptor(
        &self,
        source: &crate::runtime_client::content::ContentSource,
        representation: crate::runtime_client::content::ContentRepresentation,
    ) -> Result<Option<crate::runtime_client::content::ContentRef>, ConversationStoreError>;

    fn live_tool_header_page(
        &self,
        source: &crate::runtime_client::content::ContentSource,
        through_revision: u64,
        offset: u64,
        limit: usize,
    ) -> Result<crate::runtime_client::content::LiveBlockHeaderPage, ConversationStoreError>;
    /// Pure bounded physical range read. Target authorization belongs to the caller.
    fn content_read(
        &self,
        reference: &crate::runtime_client::content::ContentRef,
        offset: u64,
        max_bytes: u32,
    ) -> Result<crate::runtime_client::content::ContentChunk, ConversationStoreError>;

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
    /// The finite receipts are constructed inside this transaction and returned
    /// only after commit; display publication requires no subsequent store read.
    fn stage_publication_frames(
        &self,
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
        stream_id: &PublicationStreamId,
        frames: &[PublicationFrame],
    ) -> Result<Vec<crate::publication::PublicationFrameReceipt>, ConversationStoreError>;

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
        stream_id: &PublicationStreamId,
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

impl<T: ConversationStore + ?Sized> ContributionEmissionLookup for T {
    fn latest_contribution_emission(
        &self,
        producer: &crate::runtime::identity::ContextContributorIdentity,
        key: &str,
    ) -> Result<Option<ContributionEmissionRecord>, ConversationStoreError> {
        ConversationStore::latest_contribution_emission(self, producer, key)
    }

    fn current_logical_step_progress(&self) -> Result<u64, ConversationStoreError> {
        ConversationStore::current_logical_step_progress(self)
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

    fn accept_subagent_terminal(
        &self,
        notice: Option<InboundDraft>,
        draft: InboundDraft,
        event: RuntimeEventEnvelope,
    ) -> Result<
        (
            Option<AcceptedInbound>,
            AcceptedInbound,
            RuntimeEventEnvelope,
        ),
        ConversationStoreError,
    > {
        ConversationStore::accept_subagent_terminal(self, notice, draft, event)
    }

    fn commit_background_ownership(
        &self,
        event: RuntimeEventEnvelope,
    ) -> Result<RuntimeEventEnvelope, ConversationStoreError> {
        commit_background_ownership_through(self, event)
    }

    fn commit_agent_activation_admission(
        &self,
        event: RuntimeEventEnvelope,
    ) -> Result<RuntimeEventEnvelope, ConversationStoreError> {
        commit_agent_activation_admission_through(self, event)
    }

    fn commit_subagent_ownership(
        &self,
        event: RuntimeEventEnvelope,
        authority: Option<&crate::runtime::subagent::DurableAgentAuthority>,
    ) -> Result<RuntimeEventEnvelope, ConversationStoreError> {
        commit_subagent_ownership_through(self, event, authority)
    }

    fn commit_subagent_terminal(
        &self,
        event: RuntimeEventEnvelope,
    ) -> Result<RuntimeEventEnvelope, ConversationStoreError> {
        commit_subagent_terminal_through(self, event)
    }

    fn commit_subagent_physical_settlement(
        &self,
        event: RuntimeEventEnvelope,
    ) -> Result<RuntimeEventEnvelope, ConversationStoreError> {
        commit_subagent_physical_settlement_through(self, event)
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
        attempt_id: Option<AttemptId>,
    ) -> Result<AdoptedBatch, ConversationStoreError> {
        ConversationStore::adopt_pending_batch(self, watermark, attempt_id)
    }

    fn edit_pending(
        &self,
        expected: &PendingInboundRef,
        text: &str,
    ) -> Result<PendingMutationOutcome, ConversationStoreError> {
        ConversationStore::edit_pending(self, expected, text)
    }
    fn remove_pending(
        &self,
        expected: &PendingInboundRef,
    ) -> Result<PendingMutationOutcome, ConversationStoreError> {
        ConversationStore::remove_pending(self, expected)
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

    fn accept_subagent_terminal(
        &self,
        notice: Option<InboundDraft>,
        draft: InboundDraft,
        event: RuntimeEventEnvelope,
    ) -> Result<
        (
            Option<AcceptedInbound>,
            AcceptedInbound,
            RuntimeEventEnvelope,
        ),
        ConversationStoreError,
    > {
        accept_subagent_terminal_through(self.store.as_ref(), notice, draft, event)
    }

    fn commit_background_ownership(
        &self,
        event: RuntimeEventEnvelope,
    ) -> Result<RuntimeEventEnvelope, ConversationStoreError> {
        commit_background_ownership_through(self.store.as_ref(), event)
    }

    fn commit_agent_activation_admission(
        &self,
        event: RuntimeEventEnvelope,
    ) -> Result<RuntimeEventEnvelope, ConversationStoreError> {
        commit_agent_activation_admission_through(self.store.as_ref(), event)
    }

    fn commit_subagent_ownership(
        &self,
        event: RuntimeEventEnvelope,
        authority: Option<&crate::runtime::subagent::DurableAgentAuthority>,
    ) -> Result<RuntimeEventEnvelope, ConversationStoreError> {
        commit_subagent_ownership_through(self.store.as_ref(), event, authority)
    }

    fn commit_subagent_terminal(
        &self,
        event: RuntimeEventEnvelope,
    ) -> Result<RuntimeEventEnvelope, ConversationStoreError> {
        commit_subagent_terminal_through(self.store.as_ref(), event)
    }

    fn commit_subagent_physical_settlement(
        &self,
        event: RuntimeEventEnvelope,
    ) -> Result<RuntimeEventEnvelope, ConversationStoreError> {
        commit_subagent_physical_settlement_through(self.store.as_ref(), event)
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
        attempt_id: Option<AttemptId>,
    ) -> Result<AdoptedBatch, ConversationStoreError> {
        self.store.adopt_pending_batch(watermark, attempt_id)
    }

    fn edit_pending(
        &self,
        expected: &PendingInboundRef,
        text: &str,
    ) -> Result<PendingMutationOutcome, ConversationStoreError> {
        self.store.edit_pending(expected, text)
    }
    fn remove_pending(
        &self,
        expected: &PendingInboundRef,
    ) -> Result<PendingMutationOutcome, ConversationStoreError> {
        self.store.remove_pending(expected)
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
