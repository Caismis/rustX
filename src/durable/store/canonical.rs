//! Store contract for the Message Ledger, Conversation Surface, and transcript spine.

use crate::conversation::{SurfaceOp, SurfaceRevision};
use crate::events::types::RuntimeEventEnvelope;
use crate::message::types::{MessageBlock, UserMessageBlock};
use crate::publication::PublicationAudit;
use crate::runtime::identity::{EventId, MessageId, PublicationStreamId};
use serde::{Deserialize, Serialize};

use crate::durable::ConversationStoreError;

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
    auxiliary_audits: Vec<crate::durable::reading::AuxiliaryAuditSnapshot>,
    raw_tool_arguments: Vec<CanonicalRawToolArguments>,
    projections: Vec<crate::durable::projection::ProjectionCheckpoint>,
    canonical: Vec<MessageBlock>,
    surface_history: Vec<SurfaceOp>,
    surface: Vec<MessageId>,
    completed_responses: Vec<crate::durable::response::CompletedResponseProvenance>,
    turns: Vec<crate::durable::reading::TurnReadingProvenance>,
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
        projections: Vec<crate::durable::projection::ProjectionCheckpoint>,
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
    pub fn projections(&self) -> &[crate::durable::projection::ProjectionCheckpoint] {
        &self.projections
    }

    /// Attach exactly the bounded source audits named by retained checkpoints.
    /// Audits are stored in canonical `(source_conversation_id, call_id,
    /// through_journal)` order, matching durable reload order.
    ///
    /// # Errors
    /// Rejects missing, extra, duplicate or oversized source evidence.
    pub fn with_auxiliary_audits(
        mut self,
        mut audits: Vec<crate::durable::reading::AuxiliaryAuditSnapshot>,
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
        audits.sort_by(|left, right| {
            let left_key = (
                left.source_conversation_id.as_str(),
                left.call_id.as_str(),
                left.through_journal,
            );
            let right_key = (
                right.source_conversation_id.as_str(),
                right.call_id.as_str(),
                right.through_journal,
            );
            left_key.cmp(&right_key)
        });
        self.auxiliary_audits = audits;
        Ok(self)
    }

    #[must_use]
    pub fn auxiliary_audits(&self) -> &[crate::durable::reading::AuxiliaryAuditSnapshot] {
        &self.auxiliary_audits
    }

    /// Attach immutable response provenance to this seed's exact canonical identities.
    /// # Errors
    /// Rejects duplicate/non-final Assistant anchors or invalid preceding input.
    pub fn with_completed_responses(
        mut self,
        responses: Vec<crate::durable::response::CompletedResponseProvenance>,
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
        turns: Vec<crate::durable::reading::TurnReadingProvenance>,
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
                    crate::durable::reading::InheritedTurnOutcome::IncompleteAtCut
                        | crate::durable::reading::InheritedTurnOutcome::Completed
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
    pub fn turns(&self) -> &[crate::durable::reading::TurnReadingProvenance] {
        &self.turns
    }

    /// Finalized historical responses; not destination execution evidence.
    #[must_use]
    pub fn completed_responses(&self) -> &[crate::durable::response::CompletedResponseProvenance] {
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
        provenance: crate::durable::reading::TurnReadingProvenance,
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

/// The Message Ledger, Conversation Surface, and derived transcript ordering
/// spine of the conversation Store, including lineage initialization.
///
/// Canonical history is immutable once committed; Surface revisions and the
/// transcript spine reference Ledger facts and never copy their bodies.
#[allow(clippy::missing_errors_doc)]
pub trait CanonicalStore: Send + Sync + 'static {
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
    /// The errors of [`CanonicalStore::initialize_lineage`].
    fn initialize(&self, messages: &[MessageBlock]) -> Result<(), ConversationStoreError>;
    /// Loads the immutable bootstrap history originally supplied to
    /// [`CanonicalStore::initialize_lineage`] — its canonical part, which
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
    ) -> Result<Vec<crate::durable::response::CompletedResponseProvenance>, ConversationStoreError>;
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
        cut: &crate::durable::observation::ObservationReadCut,
        locator: &crate::observation::content::CanonicalContentLocator,
        offset: u64,
        limit: usize,
    ) -> Result<crate::durable::observation::NativeToolObservationPage, ConversationStoreError>;
    fn message_append_revision_by_locator(
        &self,
        locator: &crate::observation::content::CanonicalContentLocator,
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
    ) -> Result<crate::durable::observation::SurfaceBoundaryHeaderPage, ConversationStoreError>;
    fn surface_boundary_message_id(
        &self,
        boundary_revision: SurfaceRevision,
        locator: &crate::observation::content::CanonicalContentLocator,
    ) -> Result<MessageId, ConversationStoreError>;
    /// Resolves a canonical boundary of any role from the original Ledger at
    /// the selected Surface revision, independently of display read indexes.
    /// The lineage mutation still validates its exact boundary semantics.
    fn canonical_boundary_message_id(
        &self,
        boundary_revision: SurfaceRevision,
        locator: &crate::observation::content::CanonicalContentLocator,
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
    /// selected User messages through [`InboxStore::adopt_pending_batch`]
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
}
