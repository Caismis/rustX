//! The Pending Inbound Inbox domain of the native durable conversation store.
//!
//! This module owns the Pending Inbound and Goal vocabulary, the
//! [`InboxStore`] contract, the composition-time [`ConversationStoreBinding`],
//! and the narrow capabilities derived from the full store: the inbound
//! producer capability and the interaction audit capability. There is
//! deliberately no generic repository, queue, CRUD, or storage strategy trait:
//! the operations are the rustX semantic transitions a `PostgreSQL` backend
//! must reproduce exactly.

use crate::events::types::{EVENT_SCHEMA_VERSION, RuntimeEvent, RuntimeEventEnvelope};
use crate::message::types::{
    InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
};
use crate::runtime::identity::{AttemptId, ConversationId, EventId, MessageId};
use crate::runtime::inbound::InboundSequence;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::durable::{
    ConversationStore, ConversationStoreError, TranscriptCommitReceipt, TranscriptCursor,
};

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
    pub view: crate::observation::goal::RuntimeClientGoalView,
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

impl<T: ConversationStore + ?Sized> ConversationInboundCapability for T {
    fn conversation_id(&self) -> &ConversationId {
        ConversationStore::conversation_id(self)
    }

    fn accept_inbound(
        &self,
        draft: InboundDraft,
    ) -> Result<AcceptedInbound, ConversationStoreError> {
        InboxStore::accept_inbound(self, draft)
    }

    fn accept_inbound_with_event(
        &self,
        draft: InboundDraft,
        event: RuntimeEventEnvelope,
    ) -> Result<(AcceptedInbound, RuntimeEventEnvelope), ConversationStoreError> {
        InboxStore::accept_inbound_with_event(self, draft, event)
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
        InboxStore::accept_subagent_terminal(self, notice, draft, event)
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
        InboxStore::select_pending_batch(self)
    }

    fn adopt_pending_batch(
        &self,
        watermark: InboundSequence,
        attempt_id: Option<AttemptId>,
    ) -> Result<AdoptedBatch, ConversationStoreError> {
        InboxStore::adopt_pending_batch(self, watermark, attempt_id)
    }

    fn edit_pending(
        &self,
        expected: &PendingInboundRef,
        text: &str,
    ) -> Result<PendingMutationOutcome, ConversationStoreError> {
        InboxStore::edit_pending(self, expected, text)
    }
    fn remove_pending(
        &self,
        expected: &PendingInboundRef,
    ) -> Result<PendingMutationOutcome, ConversationStoreError> {
        InboxStore::remove_pending(self, expected)
    }

    fn load_pending(&self) -> Result<Vec<PendingInboundItem>, ConversationStoreError> {
        InboxStore::load_pending(self)
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

/// The Pending Inbound Inbox and Goal domain of the conversation Store.
///
/// Implementations must hold the following invariants:
///
/// - [`InboxStore::accept_inbound`] is the acceptance linearization point:
///   sequence allocation, pending persistence, and correlation state commit
///   in one transaction. No success is reported before the commit, and a
///   failed acceptance exposes no sequence, no pending record, and no
///   correlation.
/// - [`InboxStore::select_pending_batch`] is non-destructive: it returns a
///   finite watermark snapshot without removing any pending record.
/// - [`InboxStore::adopt_pending_batch`] is the canonical-adoption
///   linearization point: it appends the selected messages to the durable
///   canonical ledger and removes their pending records in one transaction,
///   so a crash can never observe a pending record whose canonical message is
///   absent, nor a canonical message that remains independently re-adoptable.
/// - [`InboxStore::has_accepted_inbound`] is monotonic: the durable
///   acceptance watermark advances inside the acceptance commit and no later
///   transition — adoption included — ever rewinds it, so once user work has
///   been durably accepted the answer stays `true` forever.
#[allow(clippy::missing_errors_doc)]
pub trait InboxStore: Send + Sync + 'static {
    /// Current Goal state, independent of history and Event Journal.
    fn load_goal(&self) -> Result<Option<crate::goal::GoalSnapshot>, ConversationStoreError> {
        Err(ConversationStoreError::Storage(
            "Goal state is unsupported by this store".to_owned(),
        ))
    }
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
    /// through [`InboxStore::accept_subagent_terminal`] — the one
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
}
