//! Native, provider-independent human interaction coordination (Issue #64).
//!
//! An interaction is a small runtime-owned rendezvous.  The coordinator owns
//! identity allocation, pending publication, the one terminal transition,
//! and the waiter handoff; it never owns Agent Loop execution, canonical
//! history, or a tool executor.
//!
//! ```text
//! pre-tool policy -> approval facts -> InteractionCoordinator
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

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use crate::agent::cancellation::AgentCancellation;
use crate::runtime::identity::{AttemptId, ConversationId, InteractionId, ToolCallId, ToolId};
use crate::runtime::types::{CancellationReason, ConversationLifecycle, LifecycleAdmission};
use crate::tools::types::{ToolInvocationMode, ToolOrigin};

/// The bounded native interaction vocabulary of the 0.1 protocol.
///
/// Approval is the only concrete interaction in this release.  A question
/// or form framework is intentionally not part of the native contract until
/// a real runtime owner needs one.
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
/// This type is intentionally owned by the coordinator boundary rather than
/// by the Runtime Client.  It contains no executor, cancellation handle,
/// canonical mutation handle, or replacement argument channel.
#[derive(Debug, Clone, PartialEq)]
pub struct ApprovalFacts {
    /// The conversation identity.
    pub conversation_id: ConversationId,
    /// The attempt identity.
    pub attempt_id: AttemptId,
    /// The model turn.
    pub turn: u32,
    /// The model-issued call identity.
    pub call_id: ToolCallId,
    /// The registry-resolved tool identity.
    pub tool_id: ToolId,
    /// The registry-resolved model-facing name.
    pub tool_name: String,
    /// The registry-resolved origin.
    pub origin: ToolOrigin,
    /// The registry-resolved execution mode.
    pub mode: ToolInvocationMode,
    /// The schema-validated business arguments.
    pub arguments: serde_json::Value,
    /// The bounded policy explanation.
    pub reason: String,
}

impl ApprovalFacts {
    fn into_request(self, id: InteractionId) -> InteractionRequest {
        InteractionRequest {
            id,
            conversation_id: self.conversation_id,
            attempt_id: self.attempt_id,
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
        }
    }
}

/// A Runtime Client-facing observation sink for the coordinator.
///
/// Implementations must be leaf publications.  The coordinator calls them
/// while its pending-state lock is held, so they must not call back into the
/// coordinator or acquire the Runtime Client projection lock.
pub trait InteractionObserver: Send + Sync {
    /// Publishes one newly pending request.
    fn on_pending(&self, request: &InteractionRequest);
    /// Publishes the one terminal transition for a request after the owning
    /// waiter has released its callback authority.
    fn on_settled(&self, interaction_id: &InteractionId, outcome: &InteractionOutcome);
}

/// The rendezvous seam carried by an [`AttemptLifecycle`](crate::agent::AttemptLifecycle).
///
/// This is a required typed owner.  It is not a callback into the TUI and it
/// does not receive permission to execute or rewrite a tool.
pub trait InteractionRendezvous: Send + Sync {
    /// Publishes an approval request and waits using the owning attempt's
    /// cancellation authority.
    fn request_approval<'a>(
        &'a self,
        facts: ApprovalFacts,
        cancellation: &'a AgentCancellation,
    ) -> BoxFuture<'a, InteractionOutcome>;
}

/// The identity rendezvous used by an attempt that has no native approval
/// consumer.  It fails closed if an `Ask` decision is supplied by a custom
/// policy, never silently allowing execution.
#[derive(Debug, Default, Clone, Copy)]
pub struct UnavailableInteraction;

impl InteractionRendezvous for UnavailableInteraction {
    fn request_approval<'a>(
        &'a self,
        _facts: ApprovalFacts,
        _cancellation: &'a AgentCancellation,
    ) -> BoxFuture<'a, InteractionOutcome> {
        Box::pin(async { InteractionOutcome::Unavailable })
    }
}

/// A published interaction ticket owned by the semantic operation that is
/// blocked on the interaction.
pub struct InteractionTicket {
    /// The interaction identity exposed to the client.
    pub id: InteractionId,
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
}

impl SettlementNotification {
    fn complete(&mut self) {
        if let Some(observer) = self.observer.take() {
            observer.on_settled(&self.interaction_id, &self.outcome);
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
    sender: oneshot::Sender<WaiterPayload>,
    admission: LifecycleAdmission,
}

/// Test-only gate at the exact point after the terminal transition has
/// removed a pending entry but before its waiter is notified. It makes the
/// response-vs-cancellation interleaving observable without changing the
/// production state machine.
#[cfg(test)]
#[derive(Debug, Default)]
struct InteractionSettleGate {
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
    fn arm(&self) {
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

    fn wait_entered(&self) {
        let mut state = self.state.lock().expect("interaction gate lock");
        while !state.entered {
            state = self.condvar.wait(state).expect("interaction gate wait");
        }
    }

    fn release(&self) {
        let mut state = self.state.lock().expect("interaction gate lock");
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
pub struct InteractionCoordinator {
    conversation_id: ConversationId,
    lifecycle: ConversationLifecycle,
    state: Mutex<CoordinatorState>,
    observer: Mutex<Option<Arc<dyn InteractionObserver>>>,
    #[cfg(test)]
    settle_gate: Mutex<Option<Arc<InteractionSettleGate>>>,
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
    /// Creates the coordinator for one conversation and shared lifecycle.
    #[must_use]
    pub fn new(conversation_id: ConversationId, lifecycle: ConversationLifecycle) -> Self {
        Self {
            conversation_id,
            lifecycle,
            state: Mutex::new(CoordinatorState::default()),
            observer: Mutex::new(None),
            #[cfg(test)]
            settle_gate: Mutex::new(None),
        }
    }

    #[cfg(test)]
    fn install_settle_gate(&self, gate: Arc<InteractionSettleGate>) {
        *self.settle_gate.lock().expect("interaction gate lock") = Some(gate);
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

    /// The conversation identity owned by this coordinator.
    #[must_use]
    pub fn conversation_id(&self) -> &ConversationId {
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
    pub fn provider_available(&self) -> bool {
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
    /// provider is present or the shared lifecycle has already closed
    /// semantic admission.
    ///
    /// # Panics
    ///
    /// Panics if the coordinator's internal synchronization state is poisoned.
    pub fn publish_approval(
        &self,
        facts: ApprovalFacts,
    ) -> Result<InteractionTicket, InteractionOutcome> {
        let id = self.allocate_id(&facts.attempt_id)?;
        let request = facts.into_request(id.clone());
        let (sender, receiver) = oneshot::channel();
        self.lifecycle
            .admit_running_commit(|admission| {
                let mut state = self.state.lock().expect("interaction state poisoned");
                if !state.provider_available {
                    return Err(InteractionOutcome::Unavailable);
                }
                let previous = state.pending.insert(
                    id.clone(),
                    PendingInteraction {
                        request: request.clone(),
                        sender,
                        admission,
                    },
                );
                debug_assert!(previous.is_none(), "interaction identity was reused");
                self.notify_pending(&request);
                drop(state);
                Ok(InteractionTicket { id, receiver })
            })
            .map_err(|_| InteractionOutcome::Unavailable)?
    }

    /// Requests approval through the coordinator and waits for the owner.
    pub async fn request_approval(
        &self,
        facts: ApprovalFacts,
        cancellation: &AgentCancellation,
    ) -> InteractionOutcome {
        let ticket = match self.publish_approval(facts) {
            Ok(ticket) => ticket,
            Err(outcome) => return outcome,
        };
        self.wait(ticket, cancellation).await
    }

    /// Waits for one published interaction using the existing attempt
    /// cancellation authority.
    pub async fn wait(
        &self,
        ticket: InteractionTicket,
        cancellation: &AgentCancellation,
    ) -> InteractionOutcome {
        let InteractionTicket { id, mut receiver } = ticket;
        let payload = tokio::select! {
            biased;
            payload = &mut receiver => payload.ok(),
            () = cancellation.cancelled() => {
                // The response and cancellation paths use the same pending
                // map transition.  If a response already won, this call is
                // stale and the receiver still returns the response.
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
    /// already-cancelled identity, and [`InteractionError::InvalidResponse`]
    /// when the typed response violates the bounded Approval contract.
    pub fn respond(
        &self,
        interaction_id: &InteractionId,
        response: InteractionResponse,
    ) -> Result<(), InteractionError> {
        let outcome = InteractionOutcome::Answered { response };
        self.settle(interaction_id, outcome, true)
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
    pub fn pending_snapshot(&self) -> Vec<InteractionRequest> {
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
    pub fn pending_count(&self) -> usize {
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
        outcome: InteractionOutcome,
        validate_response: bool,
    ) -> Result<(), InteractionError> {
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
        if validate_response && let InteractionOutcome::Answered { response } = &outcome {
            validate_response_for(&pending.request, response)?;
        }
        // Removing the pending entry and selecting the terminal outcome are
        // one mutex-protected transition.  The losing response/cancellation
        // path cannot obtain a second sender or alter the winner.
        let pending = state
            .pending
            .remove(interaction_id)
            .expect("pending entry existed under the same lock");
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
            }),
        };
        #[cfg(test)]
        self.park_after_terminal_transition();
        drop(state);
        let _ = pending.sender.send(payload);
        Ok(())
    }

    fn notify_pending(&self, request: &InteractionRequest) {
        if let Some(observer) = self
            .observer
            .lock()
            .expect("interaction observer poisoned")
            .as_ref()
        {
            observer.on_pending(request);
        }
    }
}

impl InteractionRendezvous for InteractionCoordinator {
    fn request_approval<'a>(
        &'a self,
        facts: ApprovalFacts,
        cancellation: &'a AgentCancellation,
    ) -> BoxFuture<'a, InteractionOutcome> {
        Box::pin(self.request_approval(facts, cancellation))
    }
}

/// A typed protocol-facing coordinator error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InteractionError {
    /// The interaction is no longer pending. This includes duplicate,
    /// post-cancel, post-quiescent, and pre-crash stale responses.
    NotPending { interaction_id: InteractionId },
    /// The response shape is not valid for the pending interaction.
    InvalidResponse { message: String },
}

impl core::fmt::Display for InteractionError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NotPending { interaction_id } => {
                write!(f, "interaction {interaction_id} is not pending")
            }
            Self::InvalidResponse { message } => f.write_str(message),
        }
    }
}

impl std::error::Error for InteractionError {}

const MAX_RESPONSE_REASON_BYTES: usize = 1024;

fn validate_response_for(
    request: &InteractionRequest,
    response: &InteractionResponse,
) -> Result<(), InteractionError> {
    match (&request.kind, response) {
        (InteractionKind::Approval { .. }, InteractionResponse::Approval { decision }) => {
            if let ApprovalDecision::Deny { reason } = decision
                && reason.len() > MAX_RESPONSE_REASON_BYTES
            {
                return Err(InteractionError::InvalidResponse {
                    message: format!(
                        "approval denial reason exceeds {MAX_RESPONSE_REASON_BYTES} bytes"
                    ),
                });
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::identity::ConversationId;
    use crate::runtime::types::ConversationLifecycleState;

    fn facts(attempt: &str, call: &str) -> ApprovalFacts {
        ApprovalFacts {
            conversation_id: ConversationId::new("conversation"),
            attempt_id: AttemptId::new(attempt),
            turn: 3,
            call_id: ToolCallId::new(call),
            tool_id: ToolId::new("tool.read"),
            tool_name: "read".to_owned(),
            origin: ToolOrigin::Builtin,
            mode: ToolInvocationMode::Foreground,
            arguments: serde_json::json!({"path":"a"}),
            reason: "native test policy".to_owned(),
        }
    }

    #[derive(Default)]
    struct RecordingObserver {
        pending: Mutex<Vec<InteractionRequest>>,
        settled: Mutex<Vec<(InteractionId, InteractionOutcome)>>,
    }

    impl InteractionObserver for RecordingObserver {
        fn on_pending(&self, request: &InteractionRequest) {
            self.pending.lock().unwrap().push(request.clone());
        }

        fn on_settled(&self, id: &InteractionId, outcome: &InteractionOutcome) {
            self.settled
                .lock()
                .unwrap()
                .push((id.clone(), outcome.clone()));
        }
    }

    fn coordinator() -> Arc<InteractionCoordinator> {
        let lifecycle = ConversationLifecycle::new();
        assert!(lifecycle.activate());
        Arc::new(InteractionCoordinator::new(
            ConversationId::new("conversation"),
            lifecycle,
        ))
    }

    #[test]
    fn identity_is_attempt_owned_and_non_reused_across_attempts() {
        let coordinator = coordinator();
        coordinator.set_provider_available(true);
        let observer = Arc::new(RecordingObserver::default());
        coordinator.install_observer(observer.clone());
        let first = coordinator.publish_approval(facts("conversation-attempt-1", "c1"));
        let first = first.expect("provider is available");
        let second = coordinator.publish_approval(facts("conversation-attempt-1", "c2"));
        let second = second.expect("provider is available");
        let restarted = coordinator.publish_approval(facts("conversation-attempt-2", "c3"));
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
            .publish_approval(facts(attempt_id.as_str(), "c1"))
            .expect("the maximum ordinal is issued once");
        assert_eq!(first.id, InteractionId::for_attempt(&attempt_id, u64::MAX));
        assert!(matches!(
            coordinator.publish_approval(facts(attempt_id.as_str(), "c2")),
            Err(InteractionOutcome::Unavailable)
        ));
        assert_eq!(coordinator.pending_count(), 1);
    }

    #[tokio::test]
    async fn response_wins_and_duplicate_response_is_not_pending() {
        let coordinator = coordinator();
        coordinator.set_provider_available(true);
        let ticket = coordinator
            .publish_approval(facts("conversation-attempt-1", "c1"))
            .expect("provider is available");
        let id = ticket.id.clone();
        let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
        let wait = coordinator.wait(ticket, &cancellation);
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

    /// The response winner is established while `settle` holds the pending
    /// state mutex. The gate parks after removal and before waiter notification;
    /// cancellation is then made to contend for that same mutex. Releasing the
    /// gate proves the response transition happened first, rather than merely
    /// observing whichever future happened to wake first.
    #[tokio::test(flavor = "multi_thread", worker_threads = 3)]
    async fn parked_response_transition_beats_cancellation() {
        let coordinator = coordinator();
        coordinator.set_provider_available(true);
        let ticket = coordinator
            .publish_approval(facts("conversation-attempt-1", "c1"))
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

        // The response task still owns the pending-state mutex here. The
        // cancellation task cannot linearize until the terminal gate releases.
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
            coordinator.wait(ticket, &cancellation).await,
            InteractionOutcome::Answered { response }
        );
    }

    #[tokio::test]
    async fn cancellation_wins_and_late_response_cannot_resume() {
        let coordinator = coordinator();
        coordinator.set_provider_available(true);
        let ticket = coordinator
            .publish_approval(facts("conversation-attempt-1", "c1"))
            .expect("provider is available");
        let id = ticket.id.clone();
        let cancellation = AgentCancellation::new(CancellationReason::RuntimeShutdown);
        let wait = coordinator.wait(ticket, &cancellation);
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
            .publish_approval(facts("conversation-attempt-1", "c1"))
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
            coordinator.wait(ticket, &cancellation).await,
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
            .publish_approval(facts("conversation-attempt-1", "c1"))
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
            coordinator.wait(ticket, &cancellation).await,
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
            fn on_pending(&self, _request: &InteractionRequest) {}

            fn on_settled(&self, _interaction_id: &InteractionId, _outcome: &InteractionOutcome) {
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
            .publish_approval(facts("conversation-attempt-1", "c1"))
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
            coordinator.wait(ticket, &cancellation).await,
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
            .publish_approval(facts("conversation-attempt-1", "c1"))
            .expect("admission wins first");
        assert_eq!(coordinator.pending_count(), 1);
        assert!(coordinator.lifecycle.begin_drain());
        coordinator.cancel_pending(CancellationReason::RuntimeShutdown);
        assert_eq!(coordinator.pending_count(), 0);
        drop(first);

        let after_drain = coordinator.publish_approval(facts("conversation-attempt-1", "c2"));
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
            .publish_approval(facts("conversation-attempt-1", "old"))
            .expect("old request");
        let old_id = old_ticket.id.clone();
        drop(old_ticket);

        let restarted = coordinator();
        restarted.set_provider_available(true);
        let new_ticket = restarted
            .publish_approval(facts("conversation-attempt-2", "new"))
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
        let coordinator = coordinator();
        let outcome = coordinator.publish_approval(facts("conversation-attempt-1", "c1"));
        assert!(matches!(outcome, Err(InteractionOutcome::Unavailable)));
        assert_eq!(coordinator.pending_count(), 0);
        assert_eq!(
            coordinator.lifecycle.state(),
            ConversationLifecycleState::Running
        );
    }

    /// Detach only closes future provider admission. It never fabricates an
    /// answer for a request that was already published, and reconnecting can
    /// answer the same live identity from the coordinator's snapshot.
    #[tokio::test]
    async fn provider_detach_preserves_live_pending_interaction() {
        let coordinator = coordinator();
        coordinator.set_provider_available(true);
        let ticket = coordinator
            .publish_approval(facts("conversation-attempt-1", "c1"))
            .expect("provider is available");
        let id = ticket.id.clone();
        coordinator.set_provider_available(false);
        assert!(!coordinator.provider_available());
        assert_eq!(coordinator.pending_count(), 1);
        assert_eq!(coordinator.pending_snapshot()[0].id, id);

        let cancellation = AgentCancellation::new(CancellationReason::RuntimeShutdown);
        let wait = coordinator.wait(ticket, &cancellation);
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
            .publish_approval(facts("conversation-attempt-1", "c1"))
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
