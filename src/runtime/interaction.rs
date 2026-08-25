//! Native, provider-independent human interaction coordination (Issue #100,
//! extending the Issue #64 approval seam).
//!
//! An interaction is a small runtime-owned rendezvous.  The coordinator owns
//! identity allocation, pending publication, the one terminal transition,
//! and the waiter handoff; it never owns Agent Loop execution, canonical
//! history, or a tool executor.
//!
//! ```text
//! pre-tool policy/tool -> typed interaction facts -> InteractionCoordinator
//!                                      |
//!                               Runtime Client projection
//!                                      |
//!                                typed response
//!                                      |
//!                               original owner resumes
//! ```
//!
//! Pending interactions are process/operation-owned in 0.1.  They are not
//! durable workflow records and are not recovered from client state or
//! current policy configuration.
//!
//! # The two planes (Issue #109)
//!
//! ```text
//! pending waiter / prompt lifecycle  = process-owned workflow state (never durable)
//! requested / settled semantic facts = durable audit evidence (Event Journal)
//! ```
//!
//! The coordinator owns both, and keeps them strictly separate. It commits
//! the requested fact **before** the prompt is released to a client, and the
//! settled fact **before** the semantic waiter is released, which is what
//! makes the durable order
//!
//! ```text
//! InteractionRequested -> prompt reaches the client
//! InteractionSettled(Approved) -> ToolExecutionStarted -> external side effect
//! ```
//!
//! observable rather than merely intended. Nothing recovers from those facts:
//! a process death settles or reconciles the enclosing operation through the
//! existing recovery semantics and never resurrects a prompt waiter, and a
//! historical `Approved` is audit evidence that never grants execution
//! authority to a later process.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
#[cfg(test)]
use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use crate::durable::{ConversationInteractionAudit, TranscriptCursor};
use crate::events::interaction::{
    InteractionSettlement, InteractionSubject, interaction_arguments_digest,
    validate_interaction_settlement, validate_interaction_subject, validate_question_contract,
};
use crate::events::types::{EVENT_SCHEMA_VERSION, RuntimeEvent, RuntimeEventEnvelope};
use crate::runtime::cancellation::ExecutionCancellation;
use crate::runtime::identity::{
    AttemptId, ConversationId, EventId, InteractionId, ToolCallId, ToolId, TurnId,
};
use crate::runtime::types::{CancellationReason, ConversationLifecycle, LifecycleAdmission};
use crate::tools::types::{ToolInvocationMode, ToolOrigin};

/// The bounded native interaction vocabulary of the 0.1 protocol.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum InteractionKind {
    /// Ask a client whether the already-resolved tool invocation may start.
    Approval {
        /// The canonical model-issued call identity.
        call_id: ToolCallId,
        /// The canonical registry-resolved tool identity.
        tool_id: ToolId,
        /// The safe model-facing tool name.
        tool_name: String,
        /// The registry-resolved tool origin.
        origin: ToolOrigin,
        /// The registry-resolved execution mode.
        mode: ToolInvocationMode,
        /// The already validated business arguments.  This is descriptive
        /// data only; it is never accepted back as replacement input.
        arguments: serde_json::Value,
        /// The native policy's bounded explanation for asking.
        reason: String,
    },
    /// Ask the user one bounded question. This is deliberately not a generic
    /// form or workflow vocabulary.
    Question {
        /// The bounded human-facing prompt.
        prompt: String,
        /// Optional finite choice labels.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        choices: Option<Vec<String>>,
        /// Whether a free-text answer is accepted in addition to choices.
        allow_free_text: bool,
    },
}

/// One live interaction request projected to a Runtime Client.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionRequest {
    /// The non-reused runtime-owned interaction identity.
    pub id: InteractionId,
    /// The conversation that owns the interaction.
    pub conversation_id: ConversationId,
    /// The attempt whose semantic owner is waiting.
    pub attempt_id: AttemptId,
    /// The primary model turn that reached the policy boundary.
    pub turn: u32,
    /// The bounded interaction facts.
    pub kind: InteractionKind,
}

/// The finite approval decision accepted from a client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ApprovalDecision {
    /// Continue with the exact `PreparedInvocation` that was already
    /// resolved by the Tool Registry.
    Allow,
    /// Do not start the invocation and settle its canonical result slot as
    /// policy-denied.
    Deny {
        /// A bounded client-facing reason.
        reason: String,
    },
}

/// A typed response to one native interaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum InteractionResponse {
    /// The response to an approval request.
    Approval {
        /// The finite approval decision.  It has no tool arguments.
        decision: ApprovalDecision,
    },
    /// The response to a Question request.
    Question {
        /// The typed answer, with no tool-argument replacement channel.
        answer: QuestionAnswer,
    },
}

/// The finite typed answer accepted by a Question interaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum QuestionAnswer {
    /// A selected value from the request's finite choices.
    Choice {
        /// The exact selected choice value.
        value: String,
    },
    /// A bounded free-text response.
    FreeText {
        /// The user's text.
        value: String,
    },
}

/// The terminal outcome delivered to the semantic waiter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum InteractionOutcome {
    /// A client supplied the terminal typed response.
    Answered {
        /// The accepted response.
        response: InteractionResponse,
    },
    /// The owning attempt cancellation won the rendezvous.
    Cancelled {
        /// The first-winner cancellation cause from the owning attempt.
        reason: CancellationReason,
    },
    /// No interaction-capable Runtime Client was attached at publication.
    /// Approval maps this outcome to a fail-closed denial.
    Unavailable,
}

/// The immutable facts used to construct one approval request.
///
/// This is an internal handoff from the owning Agent Execution to the
/// conversation-owned coordinator.  Conversation and attempt identity are
/// deliberately absent: the coordinator injects the conversation identity,
/// and the owning execution supplies the attempt identity at the narrow
/// request boundary.  It contains no executor, cancellation handle,
/// canonical mutation handle, or replacement argument channel.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ApprovalFacts {
    /// The model turn.
    pub(crate) turn: u32,
    /// The model-issued call identity.
    pub(crate) call_id: ToolCallId,
    /// The registry-resolved tool identity.
    pub(crate) tool_id: ToolId,
    /// The registry-resolved model-facing name.
    pub(crate) tool_name: String,
    /// The registry-resolved origin.
    pub(crate) origin: ToolOrigin,
    /// The registry-resolved execution mode.
    pub(crate) mode: ToolInvocationMode,
    /// The schema-validated business arguments. These are what a client
    /// renders: they are the exact invocation that will run.
    pub(crate) arguments: serde_json::Value,
    /// The exact model-issued arguments of the canonical `ToolCall`, before
    /// reserved-metadata stripping and normalization.
    ///
    /// The durable audit subject pins *this* value, because this is the value
    /// the Message Ledger already owns by value and therefore the only one the
    /// durable authority can verify the subject against.
    pub(crate) canonical_arguments: serde_json::Value,
    /// The bounded policy explanation.
    pub(crate) reason: String,
}

impl ApprovalFacts {
    /// Splits the owned facts into the live client-facing request and its
    /// bounded durable audit subject.
    ///
    /// The two are produced together, from one immutable set of facts, so the
    /// prompt a client is shown and the audit fact the Journal commits can
    /// never describe different calls.
    fn into_published(
        self,
        conversation_id: ConversationId,
        attempt_id: AttemptId,
        id: InteractionId,
    ) -> (InteractionRequest, InteractionSubject) {
        let subject = InteractionSubject::Approval {
            call_id: self.call_id.clone(),
            tool_id: self.tool_id.clone(),
            tool_name: self.tool_name.clone(),
            arguments_digest: interaction_arguments_digest(&self.canonical_arguments),
            reason: self.reason.clone(),
        };
        let request = InteractionRequest {
            id,
            conversation_id,
            attempt_id,
            turn: self.turn,
            kind: InteractionKind::Approval {
                call_id: self.call_id,
                tool_id: self.tool_id,
                tool_name: self.tool_name,
                origin: self.origin,
                mode: self.mode,
                arguments: self.arguments,
                reason: self.reason,
            },
        };
        (request, subject)
    }
}

/// The bounded facts used to construct one Question request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QuestionFacts {
    /// The model turn or tool turn that owns the question.
    pub(crate) turn: u32,
    /// The bounded prompt.
    pub(crate) prompt: String,
    /// Optional finite choices.
    pub(crate) choices: Option<Vec<String>>,
    /// Whether free text is accepted.
    pub(crate) allow_free_text: bool,
}

impl QuestionFacts {
    /// Validates the bounded Question contract before publication.
    ///
    /// The native `ask_user` Tool uses this same validator before it checks
    /// provider availability, so malformed model arguments become a clear
    /// `ToolResult` failure rather than being reported as an unavailable
    /// interaction provider.
    ///
    /// # Errors
    ///
    /// Returns a bounded argument diagnostic when the prompt, choice list, or
    /// answer mode cannot produce an answerable Question.
    pub(crate) fn validate(&self) -> Result<(), String> {
        validate_question_contract(&self.prompt, self.choices.as_deref(), self.allow_free_text)
    }

    /// Splits the owned facts into the live client-facing request and its
    /// bounded durable audit subject. A Question subject is stored by value
    /// because the Question contract already bounds it.
    fn into_published(
        self,
        conversation_id: ConversationId,
        attempt_id: AttemptId,
        id: InteractionId,
    ) -> (InteractionRequest, InteractionSubject) {
        let subject = InteractionSubject::Question {
            prompt: self.prompt.clone(),
            choices: self.choices.clone(),
            allow_free_text: self.allow_free_text,
        };
        let request = InteractionRequest {
            id,
            conversation_id,
            attempt_id,
            turn: self.turn,
            kind: InteractionKind::Question {
                prompt: self.prompt,
                choices: self.choices,
                allow_free_text: self.allow_free_text,
            },
        };
        (request, subject)
    }
}

/// The canonical durable identity of one interaction's requested fact.
///
/// The identity is derived from the interaction identity alone, so the
/// durable authority can resolve the requested/settled pair through its unique
/// `event_id` index instead of scanning the Event Journal.
#[must_use]
pub(crate) fn interaction_requested_event_id(interaction_id: &InteractionId) -> EventId {
    EventId::new(format!("interaction-requested-event:{interaction_id}"))
}

/// The canonical durable identity of one interaction's settled fact.
#[must_use]
pub(crate) fn interaction_settled_event_id(interaction_id: &InteractionId) -> EventId {
    EventId::new(format!("interaction-settled-event:{interaction_id}"))
}

/// Projects one terminal outcome onto its bounded durable settlement.
///
/// [`InteractionOutcome::Unavailable`] has no settlement: it is refused before
/// the requested fact commits, so there is never an audit record for a prompt
/// no user saw.
fn audit_settlement(outcome: &InteractionOutcome) -> Option<InteractionSettlement> {
    match outcome {
        InteractionOutcome::Answered { response } => match response {
            InteractionResponse::Approval { decision } => Some(match decision {
                ApprovalDecision::Allow => InteractionSettlement::Approved,
                ApprovalDecision::Deny { reason } => InteractionSettlement::Denied {
                    reason: reason.clone(),
                },
            }),
            InteractionResponse::Question { answer } => Some(InteractionSettlement::Answered {
                answer: answer.clone(),
            }),
        },
        InteractionOutcome::Cancelled { reason } => {
            Some(InteractionSettlement::Cancelled { reason: *reason })
        }
        InteractionOutcome::Unavailable => None,
    }
}

/// Builds the durable requested envelope of one live request.
fn requested_envelope(
    request: &InteractionRequest,
    subject: InteractionSubject,
    timestamp: DateTime<Utc>,
) -> RuntimeEventEnvelope {
    RuntimeEventEnvelope {
        schema_version: EVENT_SCHEMA_VERSION,
        event_id: interaction_requested_event_id(&request.id),
        sequence: 0,
        conversation_id: request.conversation_id.clone(),
        attempt_id: Some(request.attempt_id.clone()),
        turn_id: Some(TurnId::new(request.turn.to_string())),
        timestamp,
        event: RuntimeEvent::InteractionRequested {
            interaction_id: request.id.clone(),
            subject,
        },
    }
}

/// Builds the durable settled envelope of one interaction, pinned to the
/// exact attempt/turn envelope its requested fact committed under.
fn settled_envelope(
    request: &InteractionRequest,
    settlement: InteractionSettlement,
    timestamp: DateTime<Utc>,
) -> RuntimeEventEnvelope {
    RuntimeEventEnvelope {
        schema_version: EVENT_SCHEMA_VERSION,
        event_id: interaction_settled_event_id(&request.id),
        sequence: 0,
        conversation_id: request.conversation_id.clone(),
        attempt_id: Some(request.attempt_id.clone()),
        turn_id: Some(TurnId::new(request.turn.to_string())),
        timestamp,
        event: RuntimeEvent::InteractionSettled {
            interaction_id: request.id.clone(),
            settlement,
        },
    }
}

/// A deterministic in-memory interaction audit capability for tests.
///
/// It records exactly the envelopes the coordinator commits, in commit order,
/// and can be armed to fail the next requested or settled commit. It is a
/// recording seam only: it is never a durable authority, and the real
/// exactly-once/ordering rules are proven against
/// [`SqliteConversationStore`](crate::durable::SqliteConversationStore).
#[cfg(test)]
#[derive(Debug)]
pub(crate) struct RecordingInteractionAudit {
    conversation_id: ConversationId,
    state: Mutex<RecordingInteractionAuditState>,
}

#[cfg(test)]
#[derive(Debug, Default)]
struct RecordingInteractionAuditState {
    committed: Vec<RuntimeEventEnvelope>,
    fail_requested: bool,
    fail_settled: bool,
}

#[cfg(test)]
impl RecordingInteractionAudit {
    pub(crate) fn new(conversation_id: ConversationId) -> Arc<Self> {
        Arc::new(Self {
            conversation_id,
            state: Mutex::new(RecordingInteractionAuditState::default()),
        })
    }

    /// The committed audit facts in durable commit order.
    pub(crate) fn events(&self) -> Vec<RuntimeEvent> {
        self.state
            .lock()
            .expect("recording interaction audit lock")
            .committed
            .iter()
            .map(|envelope| envelope.event.clone())
            .collect()
    }

    /// The committed audit envelopes in durable commit order.
    pub(crate) fn committed(&self) -> Vec<RuntimeEventEnvelope> {
        self.state
            .lock()
            .expect("recording interaction audit lock")
            .committed
            .clone()
    }

    /// Fails the next requested commit exactly once.
    pub(crate) fn fail_next_requested(&self) {
        self.state
            .lock()
            .expect("recording interaction audit lock")
            .fail_requested = true;
    }

    /// Fails the next settled commit exactly once.
    pub(crate) fn fail_next_settled(&self) {
        self.state
            .lock()
            .expect("recording interaction audit lock")
            .fail_settled = true;
    }

    fn commit(
        &self,
        event: RuntimeEventEnvelope,
        fail: bool,
    ) -> Result<(RuntimeEventEnvelope, TranscriptCursor), crate::durable::ConversationStoreError>
    {
        let mut state = self.state.lock().expect("recording interaction audit lock");
        if fail {
            return Err(crate::durable::ConversationStoreError::Storage(
                "fault injected: interaction audit commit".to_owned(),
            ));
        }
        state.committed.push(event.clone());
        Ok((event, TranscriptCursor::new(state.committed.len() as u64)))
    }
}

#[cfg(test)]
impl ConversationInteractionAudit for RecordingInteractionAudit {
    fn conversation_id(&self) -> &ConversationId {
        &self.conversation_id
    }

    fn commit_interaction_requested(
        &self,
        event: RuntimeEventEnvelope,
    ) -> Result<(RuntimeEventEnvelope, TranscriptCursor), crate::durable::ConversationStoreError>
    {
        let fail = std::mem::take(
            &mut self
                .state
                .lock()
                .expect("recording interaction audit lock")
                .fail_requested,
        );
        self.commit(event, fail)
    }

    fn commit_interaction_settled(
        &self,
        event: RuntimeEventEnvelope,
    ) -> Result<(RuntimeEventEnvelope, TranscriptCursor), crate::durable::ConversationStoreError>
    {
        let fail = std::mem::take(
            &mut self
                .state
                .lock()
                .expect("recording interaction audit lock")
                .fail_settled,
        );
        self.commit(event, fail)
    }
}

/// A Runtime Client-facing observation sink for the coordinator.
///
/// Implementations must be leaf publications. `on_pending` runs while the
/// coordinator's pending-state lock is held, so it must not call back into the
/// coordinator or acquire the Runtime Client projection lock. `on_settled`
/// runs after the terminal map transition, under a separate counted
/// settlement admission.
pub(crate) trait InteractionObserver: Send + Sync {
    /// Publishes one newly pending request.
    fn on_pending(
        &self,
        request: &InteractionRequest,
        audit: &RuntimeEventEnvelope,
        transcript_cursor: TranscriptCursor,
    );
    /// Publishes the one terminal transition for a request after the owning
    /// waiter has released its callback authority.
    fn on_settled(
        &self,
        interaction_id: &InteractionId,
        outcome: &InteractionOutcome,
        audit: Option<&(RuntimeEventEnvelope, TranscriptCursor)>,
    );
}

/// A test-only replacement for the concrete native binding.
///
/// Production attempts can bind only the conversation-owned
/// [`InteractionCoordinator`].  The seam exists solely inside the crate's
/// deterministic test build so Agent Execution tests can control the waiter
/// without making an alternate production owner configurable.
#[cfg(test)]
pub(crate) trait TestInteractionRendezvous: Send + Sync {
    fn request_approval(
        &self,
        facts: ApprovalFacts,
        cancellation: ExecutionCancellation,
    ) -> BoxFuture<'_, InteractionOutcome>;
}

/// A published interaction ticket owned by the semantic operation that is
/// blocked on the interaction.
pub(crate) struct InteractionTicket {
    /// The interaction identity exposed to the client.
    pub(crate) id: InteractionId,
    receiver: oneshot::Receiver<WaiterPayload>,
}

impl core::fmt::Debug for InteractionTicket {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("InteractionTicket")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

/// The terminal payload retains lifecycle callback authority until the owner
/// receives and drops it.  Removing a map entry alone is therefore not
/// settlement for quiescence.
struct WaiterPayload {
    outcome: InteractionOutcome,
    waiter_admission: Option<LifecycleAdmission>,
    settlement: Option<SettlementNotification>,
}

/// The observation callback is itself kept inside a counted settlement
/// admission. This closes the small gap between releasing the waiter's
/// callback authority and publishing the Runtime Client settlement fact: the
/// lifecycle cannot become Quiescent while this leaf callback is running.
struct SettlementNotification {
    admission: Option<LifecycleAdmission>,
    observer: Option<Arc<dyn InteractionObserver>>,
    interaction_id: InteractionId,
    outcome: InteractionOutcome,
    audit: Option<(RuntimeEventEnvelope, TranscriptCursor)>,
}

impl SettlementNotification {
    fn complete(&mut self) {
        if let Some(observer) = self.observer.take() {
            observer.on_settled(&self.interaction_id, &self.outcome, self.audit.as_ref());
        }
        // Release the publication admission only after the observation
        // callback has returned. No interaction callback can begin after
        // this guard reaches zero and Quiescent may be published.
        self.admission.take();
    }
}

impl Drop for WaiterPayload {
    fn drop(&mut self) {
        // The semantic owner has either consumed the outcome or dropped its
        // waiter. Release its callback authority before publishing the
        // terminal Runtime Client observation.
        self.waiter_admission.take();
        if let Some(mut settlement) = self.settlement.take() {
            settlement.complete();
        }
    }
}

struct PendingInteraction {
    request: InteractionRequest,
    /// The exact durable audit subject that was committed before this prompt
    /// was released. Response validation and the settled fact both resolve
    /// against it, so the live acceptance rule and the durable settlement rule
    /// are literally the same rule applied to the same value.
    subject: InteractionSubject,
    /// An owner-observing cancellation view. This is the same live view used
    /// by the waiter and lets a response that arrives
    /// after cancellation became observable consume the already-selected
    /// cause instead of publishing an `Answered` terminal outcome. The
    /// coordinator never receives the authority that can request or
    /// arbitrate cancellation.
    cancellation: ExecutionCancellation,
    sender: oneshot::Sender<WaiterPayload>,
    admission: LifecycleAdmission,
}

/// Test-only gate at the exact point after the terminal transition has
/// removed a pending entry but before its waiter is notified. It makes the
/// response-vs-cancellation interleaving observable without changing the
/// production state machine.
#[cfg(test)]
#[derive(Debug, Default)]
pub(crate) struct InteractionSettleGate {
    state: Mutex<InteractionSettleGateState>,
    condvar: std::sync::Condvar,
}

#[cfg(test)]
#[derive(Debug, Default)]
struct InteractionSettleGateState {
    armed: bool,
    entered: bool,
    released: bool,
}

#[cfg(test)]
impl InteractionSettleGate {
    pub(crate) fn arm(&self) {
        let mut state = self.state.lock().expect("interaction gate lock");
        state.armed = true;
        state.entered = false;
        state.released = false;
    }

    fn enter(&self) {
        let mut state = self.state.lock().expect("interaction gate lock");
        if !state.armed {
            return;
        }
        state.entered = true;
        self.condvar.notify_all();
        while !state.released {
            state = self.condvar.wait(state).expect("interaction gate wait");
        }
        state.armed = false;
    }

    pub(crate) fn wait_entered(&self) {
        let mut state = self.state.lock().expect("interaction gate lock");
        while !state.entered {
            state = self.condvar.wait(state).expect("interaction gate wait");
        }
    }

    pub(crate) fn release(&self) {
        let mut state = self.state.lock().expect("interaction gate lock");
        state.released = true;
        self.condvar.notify_all();
    }
}

/// Test-only gate after an owning cancellation has become observable by the
/// waiter, but before that waiter attempts the coordinator terminal
/// transition. It proves that runtime drain can settle the pending entry using
/// the already-selected owner cause without relying on waiter scheduling.
#[cfg(test)]
#[derive(Debug, Default)]
pub(crate) struct InteractionWaitCancellationGate {
    state: Mutex<InteractionWaitCancellationGateState>,
    condvar: std::sync::Condvar,
}

#[cfg(test)]
#[derive(Debug, Default)]
struct InteractionWaitCancellationGateState {
    armed: bool,
    entered: bool,
    released: bool,
}

#[cfg(test)]
impl InteractionWaitCancellationGate {
    pub(crate) fn arm(&self) {
        let mut state = self.state.lock().expect("interaction waiter gate lock");
        state.armed = true;
        state.entered = false;
        state.released = false;
    }

    fn enter(&self) {
        let mut state = self.state.lock().expect("interaction waiter gate lock");
        if !state.armed {
            return;
        }
        state.entered = true;
        self.condvar.notify_all();
        while !state.released {
            state = self
                .condvar
                .wait(state)
                .expect("interaction waiter gate wait");
        }
        state.armed = false;
    }

    pub(crate) fn wait_entered(&self) {
        let mut state = self.state.lock().expect("interaction waiter gate lock");
        while !state.entered {
            state = self
                .condvar
                .wait(state)
                .expect("interaction waiter gate wait");
        }
    }

    pub(crate) fn release(&self) {
        let mut state = self.state.lock().expect("interaction waiter gate lock");
        state.released = true;
        self.condvar.notify_all();
    }
}

#[derive(Default)]
struct CoordinatorState {
    next_ordinal_by_attempt: BTreeMap<AttemptId, u64>,
    pending: BTreeMap<InteractionId, PendingInteraction>,
    provider_available: bool,
}

/// The one conversation-owned native interaction coordinator.
pub(crate) struct InteractionCoordinator {
    conversation_id: ConversationId,
    lifecycle: ConversationLifecycle,
    /// The narrow durable audit capability. It carries no Ledger, Surface,
    /// publication, or general Journal authority: the coordinator may commit
    /// exactly the requested and settled facts of its own interactions.
    audit: Arc<dyn ConversationInteractionAudit>,
    state: Mutex<CoordinatorState>,
    observer: Mutex<Option<Arc<dyn InteractionObserver>>>,
    #[cfg(test)]
    settle_gate: Mutex<Option<Arc<InteractionSettleGate>>>,
    #[cfg(test)]
    wait_cancellation_gate: Mutex<Option<Arc<InteractionWaitCancellationGate>>>,
}

/// The bounded native Question capability bound to one Agent Loop attempt.
///
/// This is intentionally a concrete crate-private value rather than a public
/// generic interaction trait. It carries only the attempt identity, a read-only
/// execution-cancellation view, and the one conversation-owned coordinator.
/// Its sole operation is to publish and await a Question; it cannot request
/// cancellation, arbitrate model-turn start, settle Approval, or mutate
/// canonical history.
#[derive(Clone)]
pub(crate) struct QuestionRequester {
    coordinator: Arc<InteractionCoordinator>,
    attempt_id: AttemptId,
    cancellation: ExecutionCancellation,
    turn: u32,
}

impl QuestionRequester {
    pub(crate) fn new(
        coordinator: Arc<InteractionCoordinator>,
        attempt_id: AttemptId,
        cancellation: ExecutionCancellation,
        turn: u32,
    ) -> Self {
        Self {
            coordinator,
            attempt_id,
            cancellation,
            turn,
        }
    }

    /// Publishes and awaits one bounded Question through the existing
    /// runtime-owned coordinator.
    pub(crate) async fn request_question(&self, mut facts: QuestionFacts) -> InteractionOutcome {
        facts.turn = self.turn;
        self.coordinator
            .request_question(self.attempt_id.clone(), facts, self.cancellation.clone())
            .await
    }
}

impl core::fmt::Debug for InteractionCoordinator {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("InteractionCoordinator")
            .field("conversation_id", &self.conversation_id)
            .field("provider_available", &self.provider_available())
            .field("pending", &self.pending_count())
            .finish_non_exhaustive()
    }
}

impl InteractionCoordinator {
    /// Creates the coordinator for one conversation, shared lifecycle, and
    /// narrow durable audit capability.
    #[must_use]
    pub(crate) fn new(
        conversation_id: ConversationId,
        lifecycle: ConversationLifecycle,
        audit: Arc<dyn ConversationInteractionAudit>,
    ) -> Self {
        debug_assert_eq!(
            audit.conversation_id(),
            &conversation_id,
            "the interaction audit capability serves one conversation"
        );
        Self {
            conversation_id,
            lifecycle,
            audit,
            state: Mutex::new(CoordinatorState::default()),
            observer: Mutex::new(None),
            #[cfg(test)]
            settle_gate: Mutex::new(None),
            #[cfg(test)]
            wait_cancellation_gate: Mutex::new(None),
        }
    }

    #[cfg(test)]
    pub(crate) fn install_settle_gate(&self, gate: Arc<InteractionSettleGate>) {
        *self.settle_gate.lock().expect("interaction gate lock") = Some(gate);
    }

    #[cfg(test)]
    pub(crate) fn install_wait_cancellation_gate(
        &self,
        gate: Arc<InteractionWaitCancellationGate>,
    ) {
        *self
            .wait_cancellation_gate
            .lock()
            .expect("interaction waiter gate lock") = Some(gate);
    }

    #[cfg(test)]
    fn park_after_terminal_transition(&self) {
        let gate = self
            .settle_gate
            .lock()
            .expect("interaction gate lock")
            .take();
        if let Some(gate) = gate {
            gate.enter();
        }
    }

    #[cfg(test)]
    fn park_before_waiter_cancellation(&self) {
        let gate = self
            .wait_cancellation_gate
            .lock()
            .expect("interaction waiter gate lock")
            .take();
        if let Some(gate) = gate {
            gate.enter();
        }
    }

    /// The conversation identity owned by this coordinator.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn conversation_id(&self) -> &ConversationId {
        &self.conversation_id
    }

    /// Installs the one Runtime Client observation sink.
    pub(crate) fn install_observer(&self, observer: Arc<dyn InteractionObserver>) {
        let mut installed = self.observer.lock().expect("interaction observer poisoned");
        debug_assert!(installed.is_none(), "one interaction observer only");
        *installed = Some(observer);
    }

    /// Marks whether the one Runtime Client attachment is currently capable
    /// of answering newly published interactions.
    pub(crate) fn set_provider_available(&self, available: bool) {
        self.state
            .lock()
            .expect("interaction state poisoned")
            .provider_available = available;
    }

    /// Whether a capable attachment is currently present.
    ///
    /// # Panics
    ///
    /// Panics if the coordinator's internal synchronization state is poisoned.
    #[must_use]
    pub(crate) fn provider_available(&self) -> bool {
        self.state
            .lock()
            .expect("interaction state poisoned")
            .provider_available
    }

    /// Publishes an approval request and returns the owner-owned waiter.
    ///
    /// No provider at the publication boundary is an explicit fail-closed
    /// outcome. A provider that detaches after this method succeeds does not
    /// settle the already-published interaction.
    ///
    /// # Errors
    ///
    /// Returns [`InteractionOutcome::Unavailable`] when no interaction-capable
    /// provider is present, the shared lifecycle has already closed semantic
    /// admission, or the durable requested fact could not commit.
    ///
    /// # Panics
    ///
    /// Panics if the coordinator's internal synchronization state is poisoned.
    fn publish_approval_with_cancellation(
        &self,
        attempt_id: AttemptId,
        facts: ApprovalFacts,
        cancellation: &ExecutionCancellation,
    ) -> Result<InteractionTicket, InteractionOutcome> {
        let id = self.allocate_id(&attempt_id)?;
        let (request, subject) =
            facts.into_published(self.conversation_id.clone(), attempt_id, id.clone());
        self.publish(request, subject, cancellation)
    }

    /// Publishes one bounded Question request and returns the owner-owned
    /// waiter. It uses the exact same lifecycle admission, identity,
    /// durable-before-prompt publication, and terminal settlement path as
    /// Approval.
    fn publish_question_with_cancellation(
        &self,
        attempt_id: AttemptId,
        facts: QuestionFacts,
        cancellation: &ExecutionCancellation,
    ) -> Result<InteractionTicket, InteractionOutcome> {
        let id = self.allocate_id(&attempt_id)?;
        let (request, subject) =
            facts.into_published(self.conversation_id.clone(), attempt_id, id.clone());
        self.publish(request, subject, cancellation)
    }

    /// The one publication transition shared by Approval and Question.
    ///
    /// The durable requested fact commits inside the same critical section
    /// that admits the pending entry, and strictly **before**
    /// [`Self::notify_pending`] releases the prompt to a client. A failed
    /// commit therefore leaves no pending entry, publishes no prompt, and
    /// fails closed as [`InteractionOutcome::Unavailable`] — the same
    /// outcome as a missing provider — so no user is ever asked a question
    /// that durable state does not record.
    ///
    /// The bounded-payload contract is checked here through the same
    /// [`validate_interaction_subject`] the durable authority uses, so a
    /// payload the store would refuse never reaches a commit attempt and never
    /// reaches a user. There is one set of limits, not two.
    fn publish(
        &self,
        request: InteractionRequest,
        subject: InteractionSubject,
        cancellation: &ExecutionCancellation,
    ) -> Result<InteractionTicket, InteractionOutcome> {
        if validate_interaction_subject(&subject).is_err() {
            return Err(InteractionOutcome::Unavailable);
        }
        let id = request.id.clone();
        let (sender, receiver) = oneshot::channel();
        self.lifecycle
            .admit_running_commit(|admission| {
                let mut state = self.state.lock().expect("interaction state poisoned");
                if !state.provider_available {
                    return Err(InteractionOutcome::Unavailable);
                }
                let (requested_audit, requested_cursor) = self
                    .audit
                    .commit_interaction_requested(requested_envelope(
                        &request,
                        subject.clone(),
                        Utc::now(),
                    ))
                    .map_err(|_| InteractionOutcome::Unavailable)?;
                let previous = state.pending.insert(
                    id.clone(),
                    PendingInteraction {
                        request,
                        subject,
                        cancellation: cancellation.clone(),
                        sender,
                        admission,
                    },
                );
                debug_assert!(previous.is_none(), "interaction identity was reused");
                // The prompt is released from the admitted entry itself, so a
                // client can never be shown a request the pending map does not
                // already own.
                self.notify_pending(
                    &state.pending[&id].request,
                    &requested_audit,
                    requested_cursor,
                );
                drop(state);
                Ok(InteractionTicket { id, receiver })
            })
            .map_err(|_| InteractionOutcome::Unavailable)?
    }

    #[cfg(test)]
    fn publish_approval(
        &self,
        attempt_id: AttemptId,
        facts: ApprovalFacts,
    ) -> Result<InteractionTicket, InteractionOutcome> {
        let owner =
            crate::agent::cancellation::AgentCancellation::new(CancellationReason::UserRequested);
        let cancellation = owner.execution_cancellation();
        self.publish_approval_with_cancellation(attempt_id, facts, &cancellation)
    }

    /// Requests approval through the coordinator and waits for the owner.
    pub(crate) async fn request_approval(
        &self,
        attempt_id: AttemptId,
        facts: ApprovalFacts,
        cancellation: ExecutionCancellation,
    ) -> InteractionOutcome {
        let ticket = match self.publish_approval_with_cancellation(attempt_id, facts, &cancellation)
        {
            Ok(ticket) => ticket,
            Err(outcome) => return outcome,
        };
        self.wait(ticket, cancellation).await
    }

    /// Publishes and awaits one Question through the runtime-owned
    /// coordinator.
    pub(crate) async fn request_question(
        &self,
        attempt_id: AttemptId,
        facts: QuestionFacts,
        cancellation: ExecutionCancellation,
    ) -> InteractionOutcome {
        let ticket = match self.publish_question_with_cancellation(attempt_id, facts, &cancellation)
        {
            Ok(ticket) => ticket,
            Err(outcome) => return outcome,
        };
        self.wait(ticket, cancellation).await
    }

    /// Waits for one published interaction using the existing attempt
    /// cancellation authority.
    async fn wait(
        &self,
        ticket: InteractionTicket,
        cancellation: ExecutionCancellation,
    ) -> InteractionOutcome {
        let InteractionTicket { id, mut receiver } = ticket;
        let payload = tokio::select! {
            biased;
            payload = &mut receiver => payload.ok(),
            () = cancellation.cancelled() => {
                // The response and cancellation paths use the same pending
                // map transition.  If a response already won, this call is
                // stale and the receiver still returns the response.
                #[cfg(test)]
                self.park_before_waiter_cancellation();
                let _ = self.cancel(&id, cancellation.reason());
                receiver.await.ok()
            }
        };
        payload.map_or(InteractionOutcome::Unavailable, |payload| {
            let outcome = payload.outcome.clone();
            drop(payload);
            outcome
        })
    }

    /// Accepts one typed client response.  A missing entry is the complete
    /// stale/duplicate/unknown contract: no callback and no semantic action.
    ///
    /// # Errors
    ///
    /// Returns [`InteractionError::NotPending`] for a stale, duplicate, or
    /// already-cancelled identity, [`InteractionError::InvalidResponse`]
    /// when the typed response violates the bounded Approval or Question
    /// contract, and [`InteractionError::AuditFailed`] when the durable
    /// settled fact could not commit. The last case is reported to the client
    /// exactly because the response must never appear accepted ahead of the
    /// durable evidence that it existed.
    pub(crate) fn respond(
        &self,
        interaction_id: &InteractionId,
        response: InteractionResponse,
    ) -> Result<(), InteractionError> {
        let outcome = InteractionOutcome::Answered { response };
        let transition = self.settle(interaction_id, outcome, true)?;
        if transition.cancellation_won {
            return Err(InteractionError::NotPending {
                interaction_id: interaction_id.clone(),
            });
        }
        if !transition.audit_committed {
            return Err(InteractionError::AuditFailed {
                interaction_id: interaction_id.clone(),
            });
        }
        Ok(())
    }

    /// Cancels one pending interaction with the owner's first-winner cause.
    pub(crate) fn cancel(
        &self,
        interaction_id: &InteractionId,
        reason: CancellationReason,
    ) -> Result<(), InteractionError> {
        self.settle(
            interaction_id,
            InteractionOutcome::Cancelled { reason },
            false,
        )
        .map(|_| ())
    }

    /// Settles every interaction that was admitted before runtime drain.
    ///
    /// The runtime invokes this after `Running -> Draining`; no new entry can
    /// pass the lifecycle admission boundary after that transition.
    pub(crate) fn cancel_pending(&self, reason: CancellationReason) {
        let ids: Vec<_> = {
            let state = self.state.lock().expect("interaction state poisoned");
            state.pending.keys().cloned().collect()
        };
        for id in ids {
            let _ = self.cancel(&id, reason);
        }
    }

    /// Returns the authoritative live pending projection in deterministic id
    /// order. This is a live observation seed, not recovery input.
    ///
    /// # Panics
    ///
    /// Panics if the coordinator's internal synchronization state is poisoned.
    #[must_use]
    pub(crate) fn pending_snapshot(&self) -> Vec<InteractionRequest> {
        let state = self.state.lock().expect("interaction state poisoned");
        state
            .pending
            .values()
            .map(|pending| pending.request.clone())
            .collect()
    }

    /// Returns the number of live pending interactions.
    ///
    /// # Panics
    ///
    /// Panics if the coordinator's internal synchronization state is poisoned.
    #[must_use]
    pub(crate) fn pending_count(&self) -> usize {
        self.state
            .lock()
            .expect("interaction state poisoned")
            .pending
            .len()
    }

    fn allocate_id(&self, attempt_id: &AttemptId) -> Result<InteractionId, InteractionOutcome> {
        let mut state = self.state.lock().expect("interaction state poisoned");
        let next = state
            .next_ordinal_by_attempt
            .entry(attempt_id.clone())
            .or_insert(1);
        // Zero is an internal exhausted sentinel. It means the maximum
        // representable ordinal was already issued; refusing the next
        // publication is what preserves non-reuse even at integer overflow.
        if *next == 0 {
            return Err(InteractionOutcome::Unavailable);
        }
        let ordinal = *next;
        *next = ordinal.checked_add(1).unwrap_or(0);
        Ok(InteractionId::for_attempt(attempt_id, ordinal))
    }

    fn settle(
        &self,
        interaction_id: &InteractionId,
        mut outcome: InteractionOutcome,
        validate_response: bool,
    ) -> Result<SettleTransition, InteractionError> {
        // Keep the observer callback inside the lifecycle's narrow
        // settlement path. This admission is acquired before the pending
        // state lock, preserving the lifecycle -> coordinator lock order
        // used by publication and drain.
        let settlement_admission =
            self.lifecycle
                .try_enter_settlement()
                .map_err(|_| InteractionError::NotPending {
                    interaction_id: interaction_id.clone(),
                })?;
        let mut state = self.state.lock().expect("interaction state poisoned");
        let Some(pending) = state.pending.get(interaction_id) else {
            drop(settlement_admission);
            return Err(InteractionError::NotPending {
                interaction_id: interaction_id.clone(),
            });
        };
        let cancellation_won = validate_response && pending.cancellation.is_cancelled();
        if cancellation_won {
            // The owning AgentCancellation owns cause arbitration. A response that
            // arrives after that authority has already won can only trigger
            // the same cancellation terminal outcome; it cannot publish an
            // Answered result and leave the interaction out of sync with its
            // owning attempt.
            outcome = InteractionOutcome::Cancelled {
                reason: pending.cancellation.reason(),
            };
        }
        if validate_response && let InteractionOutcome::Answered { response } = &outcome {
            validate_response_for(&pending.subject, response)?;
        }
        // Removing the pending entry and selecting the terminal outcome are
        // one mutex-protected transition.  The losing response/cancellation
        // path cannot obtain a second sender or alter the winner.
        let pending = state
            .pending
            .remove(interaction_id)
            .expect("pending entry existed under the same lock");
        // Durable-before-release (Issue #109): the one terminal transition is
        // committed to the audit plane before the semantic waiter is notified
        // and before the responding client is told the response was accepted.
        // For Approval this is exactly what keeps
        // `InteractionSettled(Approved)` ahead of `ToolExecutionStarted`: the
        // waiter cannot reach the tool-start frontier until this returns.
        //
        // A failed commit must not grant authority the durable record does not
        // support, so the waiter receives `Unavailable` instead — the same
        // fail-closed outcome Approval maps to a denial. The interaction stays
        // durably open, which is the honest record: a prompt existed and its
        // settlement never committed.
        let settled_audit = audit_settlement(&outcome).and_then(|settlement| {
            self.audit
                .commit_interaction_settled(settled_envelope(
                    &pending.request,
                    settlement,
                    Utc::now(),
                ))
                .ok()
        });
        let settled = settled_audit.is_some();
        if !settled {
            outcome = InteractionOutcome::Unavailable;
        }
        let observer = self
            .observer
            .lock()
            .expect("interaction observer poisoned")
            .clone();
        let payload = WaiterPayload {
            outcome: outcome.clone(),
            waiter_admission: Some(pending.admission),
            settlement: Some(SettlementNotification {
                admission: Some(settlement_admission),
                observer,
                interaction_id: interaction_id.clone(),
                outcome,
                audit: settled_audit,
            }),
        };
        drop(state);
        #[cfg(test)]
        self.park_after_terminal_transition();
        let _ = pending.sender.send(payload);
        Ok(SettleTransition {
            cancellation_won,
            audit_committed: settled,
        })
    }

    fn notify_pending(
        &self,
        request: &InteractionRequest,
        audit: &RuntimeEventEnvelope,
        transcript_cursor: TranscriptCursor,
    ) {
        if let Some(observer) = self
            .observer
            .lock()
            .expect("interaction observer poisoned")
            .as_ref()
        {
            observer.on_pending(request, audit, transcript_cursor);
        }
    }
}

/// The result of the one terminal coordinator transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SettleTransition {
    /// The owning attempt's cancellation authority had already won, so the
    /// terminal outcome is that cancellation rather than the response.
    cancellation_won: bool,
    /// The durable settled fact committed. When false the waiter received the
    /// fail-closed [`InteractionOutcome::Unavailable`] instead of the
    /// requested terminal.
    audit_committed: bool,
}

/// A typed protocol-facing coordinator error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum InteractionError {
    /// The interaction is no longer pending. This includes duplicate,
    /// post-cancel, post-quiescent, and pre-crash stale responses.
    NotPending { interaction_id: InteractionId },
    /// The response shape is not valid for the pending interaction.
    InvalidResponse { message: String },
    /// The interaction left the pending map, but its durable settled fact
    /// could not commit. The waiter was released fail-closed and no execution
    /// authority was granted.
    AuditFailed { interaction_id: InteractionId },
}

impl core::fmt::Display for InteractionError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NotPending { interaction_id } => {
                write!(f, "interaction {interaction_id} is not pending")
            }
            Self::InvalidResponse { message } => f.write_str(message),
            Self::AuditFailed { interaction_id } => write!(
                f,
                "interaction {interaction_id} could not commit its durable settlement"
            ),
        }
    }
}

impl std::error::Error for InteractionError {}

/// Validates one typed client response against the exact audit subject that
/// was durably committed for the pending interaction.
///
/// The live acceptance rule and the durable settlement rule are deliberately
/// the same function applied to the same value: a response the coordinator
/// accepts is by construction a settlement the store will accept, and a
/// response the store would refuse is refused here first with a typed
/// diagnostic instead of failing at the commit.
fn validate_response_for(
    subject: &InteractionSubject,
    response: &InteractionResponse,
) -> Result<(), InteractionError> {
    let settlement = match response {
        InteractionResponse::Approval { decision } => match decision {
            ApprovalDecision::Allow => InteractionSettlement::Approved,
            ApprovalDecision::Deny { reason } => InteractionSettlement::Denied {
                reason: reason.clone(),
            },
        },
        InteractionResponse::Question { answer } => InteractionSettlement::Answered {
            answer: answer.clone(),
        },
    };
    validate_interaction_settlement(subject, &settlement)
        .map_err(|message| InteractionError::InvalidResponse { message })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::cancellation::AgentCancellation;
    use crate::events::interaction::{
        MAX_APPROVAL_REQUEST_REASON_CHARS, MAX_QUESTION_PROMPT_CHARS,
    };
    use crate::runtime::identity::ConversationId;
    use crate::runtime::types::ConversationLifecycleState;
    use tokio::sync::oneshot;

    fn facts(call: &str) -> ApprovalFacts {
        ApprovalFacts {
            turn: 3,
            call_id: ToolCallId::new(call),
            tool_id: ToolId::new("tool.read"),
            tool_name: "read".to_owned(),
            origin: ToolOrigin::Builtin,
            mode: ToolInvocationMode::Foreground,
            arguments: serde_json::json!({"path":"a"}),
            canonical_arguments: serde_json::json!({"path":"a"}),
            reason: "native test policy".to_owned(),
        }
    }

    fn publish(
        coordinator: &InteractionCoordinator,
        attempt: &str,
        call: &str,
    ) -> Result<InteractionTicket, InteractionOutcome> {
        coordinator.publish_approval(AttemptId::new(attempt), facts(call))
    }

    fn question_facts() -> QuestionFacts {
        QuestionFacts {
            turn: 4,
            prompt: "Choose a deployment target".to_owned(),
            choices: Some(vec!["staging".to_owned(), "production".to_owned()]),
            allow_free_text: false,
        }
    }

    fn publish_question(
        coordinator: &InteractionCoordinator,
        attempt: &str,
    ) -> Result<InteractionTicket, InteractionOutcome> {
        let cancellation =
            AgentCancellation::new(CancellationReason::UserRequested).execution_cancellation();
        coordinator.publish_question_with_cancellation(
            AttemptId::new(attempt),
            question_facts(),
            &cancellation,
        )
    }

    #[derive(Default)]
    struct RecordingObserver {
        pending: Mutex<Vec<InteractionRequest>>,
        settled: Mutex<Vec<(InteractionId, InteractionOutcome)>>,
    }

    impl InteractionObserver for RecordingObserver {
        fn on_pending(
            &self,
            request: &InteractionRequest,
            _audit: &RuntimeEventEnvelope,
            _transcript_cursor: TranscriptCursor,
        ) {
            self.pending.lock().unwrap().push(request.clone());
        }

        fn on_settled(
            &self,
            id: &InteractionId,
            outcome: &InteractionOutcome,
            _audit: Option<&(RuntimeEventEnvelope, TranscriptCursor)>,
        ) {
            self.settled
                .lock()
                .unwrap()
                .push((id.clone(), outcome.clone()));
        }
    }

    struct PendingIdObserver {
        sender: Mutex<Option<oneshot::Sender<InteractionId>>>,
    }

    impl InteractionObserver for PendingIdObserver {
        fn on_pending(
            &self,
            request: &InteractionRequest,
            _audit: &RuntimeEventEnvelope,
            _transcript_cursor: TranscriptCursor,
        ) {
            if let Some(sender) = self.sender.lock().unwrap().take() {
                let _ = sender.send(request.id.clone());
            }
        }

        fn on_settled(
            &self,
            _id: &InteractionId,
            _outcome: &InteractionOutcome,
            _audit: Option<&(RuntimeEventEnvelope, TranscriptCursor)>,
        ) {
        }
    }

    fn coordinator() -> Arc<InteractionCoordinator> {
        audited_coordinator().0
    }

    /// A live coordinator plus the recording audit capability it commits to.
    fn audited_coordinator() -> (Arc<InteractionCoordinator>, Arc<RecordingInteractionAudit>) {
        let lifecycle = ConversationLifecycle::new();
        assert!(lifecycle.activate());
        let conversation_id = ConversationId::new("conversation");
        let audit = RecordingInteractionAudit::new(conversation_id.clone());
        let coordinator = Arc::new(InteractionCoordinator::new(
            conversation_id,
            lifecycle,
            audit.clone(),
        ));
        (coordinator, audit)
    }

    #[test]
    fn identity_is_attempt_owned_and_non_reused_across_attempts() {
        let coordinator = coordinator();
        coordinator.set_provider_available(true);
        let observer = Arc::new(RecordingObserver::default());
        coordinator.install_observer(observer.clone());
        let first = publish(&coordinator, "conversation-attempt-1", "c1");
        let first = first.expect("provider is available");
        let second = publish(&coordinator, "conversation-attempt-1", "c2");
        let second = second.expect("provider is available");
        let restarted = publish(&coordinator, "conversation-attempt-2", "c3");
        let restarted = restarted.expect("provider is available");
        assert_eq!(first.id.as_str(), "conversation-attempt-1-interaction-1");
        assert_eq!(second.id.as_str(), "conversation-attempt-1-interaction-2");
        assert_eq!(
            restarted.id.as_str(),
            "conversation-attempt-2-interaction-1"
        );
        assert_eq!(coordinator.pending_snapshot().len(), 3);
        assert_eq!(observer.pending.lock().unwrap().len(), 3);
    }

    #[test]
    fn request_identity_is_injected_by_the_conversation_owner() {
        let coordinator = coordinator();
        coordinator.set_provider_available(true);

        let ticket = coordinator
            .publish_approval(AttemptId::new("attempt-owned"), facts("c1"))
            .expect("provider is available");
        let request = coordinator
            .pending_snapshot()
            .into_iter()
            .next()
            .expect("published request");

        assert_eq!(request.id, ticket.id);
        assert_eq!(request.conversation_id, *coordinator.conversation_id());
        assert_eq!(request.attempt_id, AttemptId::new("attempt-owned"));
    }

    #[test]
    fn exhausted_interaction_ordinal_fails_closed_without_reusing_an_identity() {
        let coordinator = coordinator();
        coordinator.set_provider_available(true);
        let attempt_id = AttemptId::new("conversation-attempt-exhausted");
        coordinator
            .state
            .lock()
            .expect("interaction state poisoned")
            .next_ordinal_by_attempt
            .insert(attempt_id.clone(), u64::MAX);

        let first = coordinator
            .publish_approval(attempt_id.clone(), facts("c1"))
            .expect("the maximum ordinal is issued once");
        assert_eq!(first.id, InteractionId::for_attempt(&attempt_id, u64::MAX));
        assert!(matches!(
            coordinator.publish_approval(attempt_id.clone(), facts("c2")),
            Err(InteractionOutcome::Unavailable)
        ));
        assert_eq!(coordinator.pending_count(), 1);
    }

    #[tokio::test]
    async fn response_wins_and_duplicate_response_is_not_pending() {
        let coordinator = coordinator();
        coordinator.set_provider_available(true);
        let ticket = coordinator
            .publish_approval(AttemptId::new("conversation-attempt-1"), facts("c1"))
            .expect("provider is available");
        let id = ticket.id.clone();
        let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
        let wait = coordinator.wait(ticket, cancellation.execution_cancellation());
        let response = InteractionResponse::Approval {
            decision: ApprovalDecision::Allow,
        };
        coordinator
            .respond(&id, response.clone())
            .expect("accepted");
        assert_eq!(wait.await, InteractionOutcome::Answered { response });
        assert_eq!(
            coordinator.respond(
                &id,
                InteractionResponse::Approval {
                    decision: ApprovalDecision::Allow,
                }
            ),
            Err(InteractionError::NotPending { interaction_id: id })
        );
    }

    #[tokio::test]
    async fn question_publishes_once_and_accepts_one_typed_answer() {
        let coordinator = coordinator();
        coordinator.set_provider_available(true);
        let ticket = publish_question(&coordinator, "question-attempt").expect("published");
        let id = ticket.id.clone();
        let request = coordinator
            .pending_snapshot()
            .into_iter()
            .next()
            .expect("one pending question");
        assert_eq!(request.id, id);
        assert!(matches!(request.kind, InteractionKind::Question { .. }));
        let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
        let waiter = coordinator.wait(ticket, cancellation.execution_cancellation());
        let response = InteractionResponse::Question {
            answer: QuestionAnswer::Choice {
                value: "staging".to_owned(),
            },
        };
        coordinator
            .respond(&id, response.clone())
            .expect("answered");
        assert_eq!(
            waiter.await,
            InteractionOutcome::Answered {
                response: response.clone()
            }
        );
        assert_eq!(coordinator.pending_count(), 0);
        assert_eq!(
            coordinator.respond(&id, response),
            Err(InteractionError::NotPending { interaction_id: id })
        );
    }

    #[tokio::test]
    async fn bounded_question_requester_observes_owner_cancellation_only() {
        let coordinator = coordinator();
        coordinator.set_provider_available(true);
        let (pending_sender, pending_receiver) = oneshot::channel();
        coordinator.install_observer(Arc::new(PendingIdObserver {
            sender: Mutex::new(Some(pending_sender)),
        }));
        let owner = AgentCancellation::new(CancellationReason::UserRequested);
        let requester = QuestionRequester::new(
            coordinator.clone(),
            AttemptId::new("bounded-question-attempt"),
            owner.execution_cancellation(),
            9,
        );
        let waiter =
            tokio::spawn(async move { requester.request_question(question_facts()).await });
        let interaction_id = pending_receiver.await.expect("Question was published");
        assert!(owner.request_cancel(CancellationReason::RuntimeShutdown));
        assert_eq!(
            waiter.await.expect("Question waiter"),
            InteractionOutcome::Cancelled {
                reason: CancellationReason::RuntimeShutdown
            }
        );
        assert_eq!(
            coordinator.respond(
                &interaction_id,
                InteractionResponse::Question {
                    answer: QuestionAnswer::Choice {
                        value: "staging".to_owned(),
                    },
                },
            ),
            Err(InteractionError::NotPending { interaction_id })
        );
    }

    #[tokio::test]
    async fn question_rejects_answers_outside_its_declared_vocabulary() {
        let coordinator = coordinator();
        coordinator.set_provider_available(true);
        let ticket = publish_question(&coordinator, "question-validation").expect("published");
        let id = ticket.id.clone();
        let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
        let waiter = coordinator.wait(ticket, cancellation.execution_cancellation());
        assert!(matches!(
            coordinator.respond(
                &id,
                InteractionResponse::Question {
                    answer: QuestionAnswer::FreeText {
                        value: "not allowed".to_owned(),
                    },
                },
            ),
            Err(InteractionError::InvalidResponse { .. })
        ));
        coordinator
            .respond(
                &id,
                InteractionResponse::Question {
                    answer: QuestionAnswer::Choice {
                        value: "production".to_owned(),
                    },
                },
            )
            .expect("valid answer");
        assert!(matches!(waiter.await, InteractionOutcome::Answered { .. }));
    }

    #[tokio::test]
    async fn cancellation_racing_question_settles_once_and_late_answer_is_stale() {
        let coordinator = coordinator();
        coordinator.set_provider_available(true);
        let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
        let ticket = coordinator
            .publish_question_with_cancellation(
                AttemptId::new("question-cancel"),
                question_facts(),
                &cancellation.execution_cancellation(),
            )
            .expect("published");
        let id = ticket.id.clone();
        assert!(cancellation.request_cancel(CancellationReason::RuntimeShutdown));
        assert_eq!(
            coordinator
                .wait(ticket, cancellation.execution_cancellation())
                .await,
            InteractionOutcome::Cancelled {
                reason: CancellationReason::RuntimeShutdown
            }
        );
        assert_eq!(
            coordinator.respond(
                &id,
                InteractionResponse::Question {
                    answer: QuestionAnswer::Choice {
                        value: "staging".to_owned(),
                    },
                },
            ),
            Err(InteractionError::NotPending { interaction_id: id })
        );
    }

    /// The response winner is established while `settle` holds the pending
    /// state mutex. The gate parks after the mutex-protected removal and before
    /// waiter notification; cancellation then observes the already-removed
    /// entry and must lose. Releasing the gate proves the response transition
    /// happened first, rather than merely observing whichever future happened
    /// to wake first.
    #[tokio::test(flavor = "multi_thread", worker_threads = 3)]
    async fn parked_response_transition_beats_cancellation() {
        let coordinator = coordinator();
        coordinator.set_provider_available(true);
        let ticket = coordinator
            .publish_approval(AttemptId::new("conversation-attempt-1"), facts("c1"))
            .expect("provider is available");
        let id = ticket.id.clone();
        let gate = Arc::new(InteractionSettleGate::default());
        gate.arm();
        coordinator.install_settle_gate(Arc::clone(&gate));

        let response = InteractionResponse::Approval {
            decision: ApprovalDecision::Allow,
        };
        let response_coordinator = Arc::clone(&coordinator);
        let response_id = id.clone();
        let response_value = response.clone();
        let response_task =
            tokio::spawn(async move { response_coordinator.respond(&response_id, response_value) });
        gate.wait_entered();

        let (cancel_started, cancel_started_rx) = tokio::sync::oneshot::channel();
        let cancel_coordinator = Arc::clone(&coordinator);
        let cancel_id = id.clone();
        let cancellation_task = tokio::spawn(async move {
            let _ = cancel_started.send(());
            cancel_coordinator.cancel(&cancel_id, CancellationReason::RuntimeShutdown)
        });
        cancel_started_rx
            .await
            .expect("cancellation contender started");

        // The response task has already released the pending-state mutex, but
        // its terminal transition is parked before waiter notification. The
        // cancellation task therefore sees the removed entry and cannot
        // manufacture a second terminal transition.
        gate.release();
        response_task
            .await
            .expect("response task")
            .expect("response wins");
        assert_eq!(
            cancellation_task.await.expect("cancellation task"),
            Err(InteractionError::NotPending {
                interaction_id: id.clone()
            })
        );

        let cancellation = AgentCancellation::new(CancellationReason::RuntimeShutdown);
        assert_eq!(
            coordinator
                .wait(ticket, cancellation.execution_cancellation())
                .await,
            InteractionOutcome::Answered { response }
        );
    }

    /// `AgentCancellation` owns the first-winner cause even if its waiter has
    /// not yet reached the coordinator. A client response in that scheduler
    /// window must settle the same cancellation outcome, never `Answered`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 3)]
    async fn cancellation_authority_wins_before_waiter_terminal_poll() {
        let coordinator = coordinator();
        coordinator.set_provider_available(true);
        let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
        let ticket = coordinator
            .publish_approval_with_cancellation(
                AttemptId::new("conversation-attempt-1"),
                facts("c1"),
                &cancellation.execution_cancellation(),
            )
            .expect("provider is available");
        let id = ticket.id.clone();
        let waiter_gate = Arc::new(InteractionWaitCancellationGate::default());
        waiter_gate.arm();
        coordinator.install_wait_cancellation_gate(waiter_gate.clone());

        let waiter_coordinator = Arc::clone(&coordinator);
        let waiter_cancellation = cancellation.clone();
        let waiter = tokio::spawn(async move {
            waiter_coordinator
                .wait(ticket, waiter_cancellation.execution_cancellation())
                .await
        });

        assert!(cancellation.request_cancel(CancellationReason::UserRequested));
        let waiter_gate_for_wait = waiter_gate.clone();
        tokio::task::spawn_blocking(move || waiter_gate_for_wait.wait_entered())
            .await
            .expect("waiter cancellation gate task");
        assert_eq!(cancellation.reason(), CancellationReason::UserRequested);

        assert_eq!(
            coordinator
                .respond(
                    &id,
                    InteractionResponse::Approval {
                        decision: ApprovalDecision::Allow,
                    },
                )
                .expect_err("the response is stale once cancellation already won"),
            InteractionError::NotPending {
                interaction_id: id.clone()
            }
        );
        assert_eq!(coordinator.pending_count(), 0);

        waiter_gate.release();
        assert_eq!(
            waiter.await.expect("waiter task"),
            InteractionOutcome::Cancelled {
                reason: CancellationReason::UserRequested
            }
        );
    }

    #[tokio::test]
    async fn cancellation_wins_and_late_response_cannot_resume() {
        let coordinator = coordinator();
        coordinator.set_provider_available(true);
        let ticket = coordinator
            .publish_approval(AttemptId::new("conversation-attempt-1"), facts("c1"))
            .expect("provider is available");
        let id = ticket.id.clone();
        let cancellation = AgentCancellation::new(CancellationReason::RuntimeShutdown);
        let wait = coordinator.wait(ticket, cancellation.execution_cancellation());
        assert!(cancellation.request_cancel(CancellationReason::RuntimeShutdown));
        assert_eq!(
            wait.await,
            InteractionOutcome::Cancelled {
                reason: CancellationReason::RuntimeShutdown
            }
        );
        assert_eq!(
            coordinator.respond(
                &id,
                InteractionResponse::Approval {
                    decision: ApprovalDecision::Allow,
                }
            ),
            Err(InteractionError::NotPending { interaction_id: id })
        );
    }

    /// The cancellation winner uses the same parked terminal transition in
    /// the opposite order. A response that enters while cancellation owns the
    /// state mutex is rejected after release and cannot wake a second waiter.
    #[tokio::test(flavor = "multi_thread", worker_threads = 3)]
    async fn parked_cancellation_transition_rejects_response() {
        let coordinator = coordinator();
        coordinator.set_provider_available(true);
        let ticket = coordinator
            .publish_approval(AttemptId::new("conversation-attempt-1"), facts("c1"))
            .expect("provider is available");
        let id = ticket.id.clone();
        let gate = Arc::new(InteractionSettleGate::default());
        gate.arm();
        coordinator.install_settle_gate(Arc::clone(&gate));

        let cancel_coordinator = Arc::clone(&coordinator);
        let cancel_id = id.clone();
        let cancel_task = tokio::spawn(async move {
            cancel_coordinator.cancel(&cancel_id, CancellationReason::RuntimeShutdown)
        });
        gate.wait_entered();

        let (response_started, response_started_rx) = tokio::sync::oneshot::channel();
        let response_coordinator = Arc::clone(&coordinator);
        let response_id = id.clone();
        let response_task = tokio::spawn(async move {
            let _ = response_started.send(());
            response_coordinator.respond(
                &response_id,
                InteractionResponse::Approval {
                    decision: ApprovalDecision::Allow,
                },
            )
        });
        response_started_rx
            .await
            .expect("response contender started");

        gate.release();
        cancel_task
            .await
            .expect("cancellation task")
            .expect("cancellation wins");
        assert_eq!(
            response_task.await.expect("response task"),
            Err(InteractionError::NotPending {
                interaction_id: id.clone()
            })
        );

        let cancellation = AgentCancellation::new(CancellationReason::RuntimeShutdown);
        assert_eq!(
            coordinator
                .wait(ticket, cancellation.execution_cancellation())
                .await,
            InteractionOutcome::Cancelled {
                reason: CancellationReason::RuntimeShutdown
            }
        );
    }

    /// A terminal transition removes the live map entry immediately, but the
    /// counted lifecycle admission remains inside the waiter payload. Drain
    /// therefore cannot publish `Quiescent` until the semantic owner consumes
    /// the outcome and releases its callback authority.
    #[tokio::test]
    async fn drain_waits_for_waiter_settlement_after_pending_map_is_empty() {
        let coordinator = coordinator();
        coordinator.set_provider_available(true);
        let ticket = coordinator
            .publish_approval(AttemptId::new("conversation-attempt-1"), facts("c1"))
            .expect("provider is available");
        let lifecycle = coordinator.lifecycle.clone();
        assert!(lifecycle.begin_drain());
        coordinator.cancel_pending(CancellationReason::RuntimeShutdown);
        assert_eq!(coordinator.pending_count(), 0, "the pending map is empty");
        assert_eq!(lifecycle.state(), ConversationLifecycleState::Draining);
        assert!(
            !lifecycle.mark_quiescent(),
            "the waiter payload still owns lifecycle callback authority"
        );

        let cancellation = AgentCancellation::new(CancellationReason::RuntimeShutdown);
        assert_eq!(
            coordinator
                .wait(ticket, cancellation.execution_cancellation())
                .await,
            InteractionOutcome::Cancelled {
                reason: CancellationReason::RuntimeShutdown
            }
        );
        assert!(lifecycle.mark_quiescent());
        assert_eq!(lifecycle.state(), ConversationLifecycleState::Quiescent);
        assert_eq!(
            coordinator.respond(
                &InteractionId::for_attempt(&AttemptId::new("conversation-attempt-1"), 1),
                InteractionResponse::Approval {
                    decision: ApprovalDecision::Allow,
                },
            ),
            Err(InteractionError::NotPending {
                interaction_id: InteractionId::for_attempt(
                    &AttemptId::new("conversation-attempt-1"),
                    1,
                )
            })
        );
    }

    /// The Runtime Client settlement observation is deliberately later than
    /// the terminal map transition. The waiter releases its callback
    /// authority first; the observation then runs inside a second counted
    /// settlement admission, so drain cannot publish Quiescent between those
    /// two actions.
    #[tokio::test]
    async fn settled_observation_follows_waiter_release_before_quiescence() {
        #[derive(Clone)]
        struct StateObserver {
            lifecycle: ConversationLifecycle,
            settled: Arc<Mutex<Vec<ConversationLifecycleState>>>,
        }

        impl InteractionObserver for StateObserver {
            fn on_pending(
                &self,
                _request: &InteractionRequest,
                _audit: &RuntimeEventEnvelope,
                _transcript_cursor: TranscriptCursor,
            ) {
            }

            fn on_settled(
                &self,
                _interaction_id: &InteractionId,
                _outcome: &InteractionOutcome,
                _audit: Option<&(RuntimeEventEnvelope, TranscriptCursor)>,
            ) {
                self.settled
                    .lock()
                    .expect("settled states lock")
                    .push(self.lifecycle.state());
            }
        }

        let coordinator = coordinator();
        coordinator.set_provider_available(true);
        let lifecycle = coordinator.lifecycle.clone();
        let settled = Arc::new(Mutex::new(Vec::new()));
        coordinator.install_observer(Arc::new(StateObserver {
            lifecycle: lifecycle.clone(),
            settled: settled.clone(),
        }));
        let ticket = coordinator
            .publish_approval(AttemptId::new("conversation-attempt-1"), facts("c1"))
            .expect("provider is available");
        let id = ticket.id.clone();

        assert!(lifecycle.begin_drain());
        coordinator
            .respond(
                &id,
                InteractionResponse::Approval {
                    decision: ApprovalDecision::Allow,
                },
            )
            .expect("response wins");
        assert_eq!(coordinator.pending_count(), 0);
        assert!(settled.lock().expect("settled states lock").is_empty());
        assert!(!lifecycle.mark_quiescent());

        let cancellation = AgentCancellation::new(CancellationReason::RuntimeShutdown);
        assert!(matches!(
            coordinator
                .wait(ticket, cancellation.execution_cancellation())
                .await,
            InteractionOutcome::Answered { .. }
        ));
        assert_eq!(
            settled.lock().expect("settled states lock").as_slice(),
            &[ConversationLifecycleState::Draining]
        );
        assert!(lifecycle.mark_quiescent());
    }

    /// The two lifecycle winner orders are explicit: admission first leaves
    /// an owner for drain to cancel, while drain first refuses publication at
    /// the shared lifecycle commit boundary.
    #[test]
    fn lifecycle_admission_and_drain_have_one_total_order() {
        let coordinator = coordinator();
        coordinator.set_provider_available(true);
        let first = coordinator
            .publish_approval(AttemptId::new("conversation-attempt-1"), facts("c1"))
            .expect("admission wins first");
        assert_eq!(coordinator.pending_count(), 1);
        assert!(coordinator.lifecycle.begin_drain());
        coordinator.cancel_pending(CancellationReason::RuntimeShutdown);
        assert_eq!(coordinator.pending_count(), 0);
        drop(first);

        let after_drain =
            coordinator.publish_approval(AttemptId::new("conversation-attempt-1"), facts("c2"));
        assert!(matches!(after_drain, Err(InteractionOutcome::Unavailable)));
        assert_eq!(coordinator.pending_count(), 0);
    }

    /// Restart creates a new coordinator with no old pending state. Because
    /// the new attempt identity is itself recovery-non-reused, the same
    /// process-local ordinal cannot alias the old interaction identity.
    #[test]
    fn delayed_pre_crash_response_cannot_name_post_restart_work() {
        let old = coordinator();
        old.set_provider_available(true);
        let old_ticket = old
            .publish_approval(AttemptId::new("conversation-attempt-1"), facts("old"))
            .expect("old request");
        let old_id = old_ticket.id.clone();
        drop(old_ticket);

        let restarted = coordinator();
        restarted.set_provider_available(true);
        let new_ticket = restarted
            .publish_approval(AttemptId::new("conversation-attempt-2"), facts("new"))
            .expect("new request");
        assert_ne!(old_id, new_ticket.id);
        assert_eq!(
            restarted.respond(
                &old_id,
                InteractionResponse::Approval {
                    decision: ApprovalDecision::Allow,
                },
            ),
            Err(InteractionError::NotPending {
                interaction_id: old_id
            })
        );
        assert_eq!(restarted.pending_count(), 1);
    }

    #[test]
    fn no_provider_fails_closed_without_creating_pending_work() {
        let (coordinator, audit) = audited_coordinator();
        let outcome =
            coordinator.publish_approval(AttemptId::new("conversation-attempt-1"), facts("c1"));
        assert!(matches!(outcome, Err(InteractionOutcome::Unavailable)));
        assert_eq!(coordinator.pending_count(), 0);
        assert!(
            audit.events().is_empty(),
            "a prompt no client could see leaves no audit record"
        );
        assert_eq!(
            coordinator.lifecycle.state(),
            ConversationLifecycleState::Running
        );
    }

    /// The requested fact is committed inside the same critical section that
    /// admits the pending entry, strictly before the prompt is published. The
    /// publication callback therefore always runs with the audit already
    /// durable, and a pending entry never exists without it.
    #[test]
    fn requested_audit_commits_before_the_pending_entry_is_observable() {
        struct AuditAtPrompt {
            audit: Arc<RecordingInteractionAudit>,
            seen: Mutex<Vec<RuntimeEvent>>,
        }

        impl InteractionObserver for AuditAtPrompt {
            fn on_pending(
                &self,
                _request: &InteractionRequest,
                _audit: &RuntimeEventEnvelope,
                _transcript_cursor: TranscriptCursor,
            ) {
                *self.seen.lock().unwrap() = self.audit.events();
            }
            fn on_settled(
                &self,
                _id: &InteractionId,
                _outcome: &InteractionOutcome,
                _audit: Option<&(RuntimeEventEnvelope, TranscriptCursor)>,
            ) {
            }
        }

        let (coordinator, audit) = audited_coordinator();
        coordinator.set_provider_available(true);
        let observer = Arc::new(AuditAtPrompt {
            audit: audit.clone(),
            seen: Mutex::new(Vec::new()),
        });
        coordinator.install_observer(observer.clone());
        let ticket = coordinator
            .publish_approval(AttemptId::new("conversation-attempt-1"), facts("c1"))
            .expect("provider is available");

        let seen = observer.seen.lock().unwrap().clone();
        assert!(
            matches!(
                seen.as_slice(),
                [RuntimeEvent::InteractionRequested { interaction_id, subject: InteractionSubject::Approval { call_id, tool_name, .. } }]
                    if *interaction_id == ticket.id
                        && *call_id == ToolCallId::new("c1")
                        && tool_name == "read"
            ),
            "the requested fact must be durable when the prompt is released, saw {seen:?}"
        );

        let envelope = audit.committed().remove(0);
        assert_eq!(
            envelope.event_id,
            interaction_requested_event_id(&ticket.id)
        );
        assert_eq!(
            envelope.attempt_id,
            Some(AttemptId::new("conversation-attempt-1"))
        );
        assert_eq!(envelope.turn_id, Some(TurnId::new("3")));
    }

    /// A requested fact that cannot commit publishes no prompt at all: the
    /// interaction fails closed exactly like a missing provider, so a user is
    /// never asked something durable state does not record.
    #[test]
    fn a_failed_requested_commit_publishes_no_prompt() {
        let (coordinator, audit) = audited_coordinator();
        coordinator.set_provider_available(true);
        let observer = Arc::new(RecordingObserver::default());
        coordinator.install_observer(observer.clone());
        audit.fail_next_requested();

        assert!(matches!(
            coordinator.publish_approval(AttemptId::new("conversation-attempt-1"), facts("c1")),
            Err(InteractionOutcome::Unavailable)
        ));
        assert_eq!(coordinator.pending_count(), 0);
        assert!(observer.pending.lock().unwrap().is_empty());
        assert!(audit.events().is_empty());

        // The next publication is unaffected: the fault was one commit, not a
        // coordinator state change.
        coordinator
            .publish_approval(AttemptId::new("conversation-attempt-1"), facts("c2"))
            .expect("the coordinator still publishes");
        assert_eq!(coordinator.pending_count(), 1);
    }

    /// The coordinator refuses exactly the payloads the durable authority
    /// refuses, because both call [`validate_interaction_subject`].
    ///
    /// The publication fails closed as `Unavailable` before any commit is
    /// attempted, so an out-of-contract payload never reaches a user and never
    /// reaches the Journal. There is one set of limits, not a coordinator set
    /// and a store set that can drift apart.
    #[test]
    fn the_coordinator_refuses_every_subject_the_durable_authority_refuses() {
        let (coordinator, audit) = audited_coordinator();
        coordinator.set_provider_available(true);
        let attempt = AttemptId::new("conversation-attempt-1");

        let mut oversized_reason = facts("c1");
        oversized_reason.reason = "r".repeat(MAX_APPROVAL_REQUEST_REASON_CHARS + 1);
        assert!(matches!(
            coordinator.publish_approval(attempt.clone(), oversized_reason),
            Err(InteractionOutcome::Unavailable)
        ));

        let cancellation =
            AgentCancellation::new(CancellationReason::UserRequested).execution_cancellation();
        for facts in [
            QuestionFacts {
                turn: 4,
                prompt: "p".repeat(MAX_QUESTION_PROMPT_CHARS + 1),
                choices: None,
                allow_free_text: true,
            },
            QuestionFacts {
                turn: 4,
                prompt: "Which target?".to_owned(),
                choices: Some(vec!["staging".to_owned(), "staging".to_owned()]),
                allow_free_text: false,
            },
            QuestionFacts {
                turn: 4,
                prompt: "Which target?".to_owned(),
                choices: None,
                allow_free_text: false,
            },
        ] {
            assert!(matches!(
                coordinator.publish_question_with_cancellation(
                    attempt.clone(),
                    facts,
                    &cancellation
                ),
                Err(InteractionOutcome::Unavailable)
            ));
        }

        assert!(
            audit.events().is_empty(),
            "no out-of-contract payload reached a durable commit attempt"
        );
        assert_eq!(coordinator.pending_count(), 0);
    }

    /// Every response the coordinator accepts is a settlement the durable
    /// authority accepts against the very subject it committed, and every
    /// response it rejects settles nothing at all.
    #[tokio::test]
    async fn an_accepted_response_is_always_a_settlement_the_store_accepts() {
        let (coordinator, audit) = audited_coordinator();
        coordinator.set_provider_available(true);
        let ticket = publish_question(&coordinator, "conversation-attempt-1")
            .expect("provider is available");
        let id = ticket.id.clone();
        let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
        let wait = coordinator.wait(ticket, cancellation.execution_cancellation());

        assert!(matches!(
            coordinator.respond(
                &id,
                InteractionResponse::Question {
                    answer: QuestionAnswer::Choice {
                        value: "canary".to_owned(),
                    },
                },
            ),
            Err(InteractionError::InvalidResponse { .. })
        ));
        assert!(
            matches!(
                audit.events().as_slice(),
                [RuntimeEvent::InteractionRequested { .. }]
            ),
            "a refused response settles nothing"
        );

        coordinator
            .respond(
                &id,
                InteractionResponse::Question {
                    answer: QuestionAnswer::Choice {
                        value: "staging".to_owned(),
                    },
                },
            )
            .expect("an offered choice is accepted");
        let _ = wait.await;

        let events = audit.events();
        let [
            RuntimeEvent::InteractionRequested { subject, .. },
            RuntimeEvent::InteractionSettled { settlement, .. },
        ] = events.as_slice()
        else {
            panic!("one requested fact and one settled fact, got {events:?}");
        };
        validate_interaction_settlement(subject, settlement)
            .expect("the committed pair satisfies the durable contract");
    }

    /// The settled fact commits before the waiter is released. When that
    /// commit fails the waiter receives the fail-closed `Unavailable` outcome
    /// and the responding client is told, rather than being shown an
    /// acceptance the audit does not support.
    ///
    /// `Unavailable` is the same value a missing provider produces, and the
    /// scripted suite's headless regression already proves that value maps to
    /// a denied result slot with no executor call and no `ToolExecutionStarted`
    /// — so a failed settled commit cannot authorize a side effect either. The
    /// interaction stays durably open, and no second settlement is invented to
    /// tidy the lifecycle.
    #[tokio::test]
    async fn a_failed_settled_commit_releases_the_waiter_fail_closed() {
        let (coordinator, audit) = audited_coordinator();
        coordinator.set_provider_available(true);
        let ticket = coordinator
            .publish_approval(AttemptId::new("conversation-attempt-1"), facts("c1"))
            .expect("provider is available");
        let id = ticket.id.clone();
        let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
        let wait = coordinator.wait(ticket, cancellation.execution_cancellation());

        audit.fail_next_settled();
        assert_eq!(
            coordinator.respond(
                &id,
                InteractionResponse::Approval {
                    decision: ApprovalDecision::Allow,
                },
            ),
            Err(InteractionError::AuditFailed {
                interaction_id: id.clone()
            })
        );
        assert_eq!(wait.await, InteractionOutcome::Unavailable);
        assert!(
            matches!(
                audit.events().as_slice(),
                [RuntimeEvent::InteractionRequested { .. }]
            ),
            "the interaction stays durably open, which is the honest record"
        );
        assert_eq!(
            coordinator.respond(
                &id,
                InteractionResponse::Approval {
                    decision: ApprovalDecision::Allow,
                },
            ),
            Err(InteractionError::NotPending { interaction_id: id }),
            "the identity is spent even though its settlement did not commit"
        );
    }

    /// A cancellation terminal is durable audit exactly like an answer: it
    /// records that a prompt existed and that the owning attempt's
    /// cancellation authority, not a user, settled it.
    #[tokio::test]
    async fn cancellation_settlement_is_durable_audit() {
        let (coordinator, audit) = audited_coordinator();
        coordinator.set_provider_available(true);
        let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
        let ticket = coordinator
            .publish_approval_with_cancellation(
                AttemptId::new("conversation-attempt-1"),
                facts("c1"),
                &cancellation.execution_cancellation(),
            )
            .expect("provider is available");
        assert!(cancellation.request_cancel(CancellationReason::RuntimeShutdown));
        assert_eq!(
            coordinator
                .wait(ticket, cancellation.execution_cancellation())
                .await,
            InteractionOutcome::Cancelled {
                reason: CancellationReason::RuntimeShutdown
            }
        );
        assert!(matches!(
            audit.events().as_slice(),
            [
                RuntimeEvent::InteractionRequested { .. },
                RuntimeEvent::InteractionSettled {
                    settlement: InteractionSettlement::Cancelled {
                        reason: CancellationReason::RuntimeShutdown
                    },
                    ..
                }
            ]
        ));
    }

    /// A coordinator constructed over durable state that already holds an
    /// unanswered interaction starts with **no** pending waiter. Pending
    /// interaction is process-owned workflow state; it is never reconstructed
    /// from the audit plane. The restarted coordinator asks its own new
    /// question under its own new identity.
    #[test]
    fn a_restarted_coordinator_reconstructs_no_pending_waiter() {
        let (first, audit) = audited_coordinator();
        first.set_provider_available(true);
        let ticket = first
            .publish_approval(AttemptId::new("conversation-attempt-1"), facts("c1"))
            .expect("provider is available");
        assert_eq!(first.pending_count(), 1);
        assert_eq!(audit.events().len(), 1, "requested, never settled");

        // The process dies here. A new coordinator is built over the very same
        // durable audit capability.
        drop(first);
        let lifecycle = ConversationLifecycle::new();
        assert!(lifecycle.activate());
        let restarted = Arc::new(InteractionCoordinator::new(
            ConversationId::new("conversation"),
            lifecycle,
            audit.clone(),
        ));
        restarted.set_provider_available(true);
        assert_eq!(
            restarted.pending_count(),
            0,
            "no waiter and no prompt is recreated from durable state"
        );
        assert!(restarted.pending_snapshot().is_empty());

        // The historical identity grants nothing; a new ask allocates a new
        // one and is refused for the old one.
        assert_eq!(
            restarted.respond(
                &ticket.id,
                InteractionResponse::Approval {
                    decision: ApprovalDecision::Allow,
                },
            ),
            Err(InteractionError::NotPending {
                interaction_id: ticket.id.clone()
            })
        );
        let fresh = restarted
            .publish_approval(AttemptId::new("conversation-attempt-2"), facts("c2"))
            .expect("a restarted coordinator asks its own new question");
        assert_ne!(fresh.id, ticket.id);
        assert_eq!(audit.events().len(), 2);
    }

    /// Detach only closes future provider admission. It never fabricates an
    /// answer for a request that was already published, and reconnecting can
    /// answer the same live identity from the coordinator's snapshot.
    #[tokio::test]
    async fn provider_detach_preserves_live_pending_interaction() {
        let coordinator = coordinator();
        coordinator.set_provider_available(true);
        let ticket = coordinator
            .publish_approval(AttemptId::new("conversation-attempt-1"), facts("c1"))
            .expect("provider is available");
        let id = ticket.id.clone();
        coordinator.set_provider_available(false);
        assert!(!coordinator.provider_available());
        assert_eq!(coordinator.pending_count(), 1);
        assert_eq!(coordinator.pending_snapshot()[0].id, id);

        let cancellation = AgentCancellation::new(CancellationReason::RuntimeShutdown);
        let wait = coordinator.wait(ticket, cancellation.execution_cancellation());
        coordinator.set_provider_available(true);
        coordinator
            .respond(
                &id,
                InteractionResponse::Approval {
                    decision: ApprovalDecision::Allow,
                },
            )
            .expect("reconnected provider can answer the old pending request");
        assert!(matches!(
            wait.await,
            InteractionOutcome::Answered {
                response: InteractionResponse::Approval {
                    decision: ApprovalDecision::Allow
                }
            }
        ));
    }

    #[test]
    fn request_response_has_no_argument_replacement_channel() {
        let coordinator = coordinator();
        coordinator.set_provider_available(true);
        let ticket = coordinator
            .publish_approval(AttemptId::new("conversation-attempt-1"), facts("c1"))
            .expect("provider is available");
        let request = coordinator.pending_snapshot().pop().expect("pending");
        assert_eq!(request.id, ticket.id);
        assert!(matches!(
            InteractionResponse::Approval {
                decision: ApprovalDecision::Allow
            },
            InteractionResponse::Approval { .. }
        ));
        assert!(!request.id.as_str().is_empty());
    }
}
