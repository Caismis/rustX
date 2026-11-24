//! The unified foreground slot of the coordinator (Issue #606, Stage 1).
//!
//! `Attempt`, `ManualCompaction`, and `Scheduled` are mutually exclusive
//! **by construction**: at most one of them can occupy the single
//! [`ForegroundSlot`] at a time, and a `ConversationState` is lent out
//! exactly while the slot is `Attempt` or `ManualCompaction`. A `Scheduled`
//! owner holds the foreground execution right while the conversation state
//! stays coordinator-owned inside the slot (no hydration it does not need,
//! no hot state lost on failure). `Poisoned` is the absorbing terminal for
//! a lending task that panicked without returning the state: it fences all
//! later foreground admission while drain still settles with evidence.
//!
//! `ManualTailState` tracks the physical manual-compaction task *after* its
//! logical completion separately from slot state, under the same lock, so a
//! `JoinHandle` taken but not yet awaited is never misread as reaped.

use std::sync::{Arc, Mutex};

use super::compaction::ManualCompactionCompletion;
use super::{
    AdmittedConfiguration, AgentCancellation, AttemptId, CancellationReason, ConversationState,
    ConversationStore, RuntimeInner,
};

/// Whether the coordinator still owns the canonical conversation state
/// at rest, and whether that state was ever materialized in memory.
///
/// A reopened conversation does not hydrate its active working set at
/// construction or attach: the state stays in the durable store until
/// the first coordinator operation that needs it, which loads it once
/// from the durable Surface head. `Scheduled` execution keeps this slot
/// inside the coordinator the whole time, so a scheduled run never
/// triggers cold-state hydration it does not need.
#[allow(clippy::large_enum_variant)]
pub(crate) enum IdleConversation {
    /// Not hydrated since construction; the durable head is the authority.
    Durable,
    /// Owned by the coordinator between foreground operations.
    Owned(ConversationState),
}

/// The one foreground execution slot of the coordinator.
///
/// At most one of `Attempt`, `ManualCompaction`, or `Scheduled` exists:
/// they share the foreground exclusion domain in the type itself, not
/// by convention across separate `Option` fields.
#[allow(clippy::large_enum_variant)]
pub(crate) enum ForegroundSlot {
    /// No foreground operation owns the conversation.
    Idle(IdleConversation),
    /// An admitted attempt owns the lent `ConversationState`.
    Attempt(CurrentAttempt),
    /// A manual-compaction maintenance task owns the lent
    /// `ConversationState`.
    ManualCompaction(CurrentManualCompaction),
    /// A scheduled workflow run holds the foreground execution right
    /// while the coordinator retains the conversation state.
    Scheduled {
        /// The scheduled run's coordinator owner record.
        current: CurrentScheduled,
        /// The retained, coordinator-owned conversation state.
        conversation: IdleConversation,
    },
    /// A lending task (attempt or manual compaction) panicked without
    /// returning the conversation state. Absorbing: no further
    /// foreground admission, idle claim, or staged commit may proceed;
    /// drain still completes with the recorded failure evidence.
    Poisoned,
}

/// Arms the [`ForegroundSlot::Poisoned`] transition while its owning
/// attempt task can still panic before returning the `ConversationState`.
///
/// The guard lives in the spawned task itself: dropped during a panic
/// unwind while `armed`, it locks the coordinator, transitions its attempt
/// slot to `Poisoned`, commits the absorbing durability-failure fact, and
/// wakes every settlement/admission waiter so drain cannot hang. It is
/// disarmed at the top of `finish_attempt`, where the task provably
/// retains the state.
pub(crate) struct AttemptTaskPoisonGuard {
    inner: std::sync::Weak<RuntimeInner>,
    attempt_id: AttemptId,
    armed: bool,
}

/// The physical manual-compaction task reclamation state, under the
/// coordinator lock.
///
/// `Pending` means the task may still be running or its `JoinHandle` has
/// not been claimed; `Joining` means a reaper claimed the handle and is
/// awaiting it — the join is in flight and must **not** be misread as
/// reaped. A concurrent `wait` reaper observes `Joining` and awaits the
/// shared completion instead of racing the join.
pub(crate) enum ManualTailState {
    /// No unreaped manual-compaction task exists.
    None,
    /// The task may still be running; its handle is unreaped.
    Pending(ManualTail),
    /// A reaper claimed the `JoinHandle` and is awaiting it.
    Joining(Arc<ManualCompactionCompletion>),
}

/// The physical manual-compaction task owner record, claimed by exactly
/// one reaper.
pub(crate) struct ManualTail {
    pub(crate) _activity: super::activity::TaskActivityLease,
    pub(crate) worker: tokio::task::JoinHandle<()>,
    pub(crate) completion: Arc<ManualCompactionCompletion>,
}

impl ManualTailState {
    /// Whether a physical task remains unreaped (`Pending` or `Joining`):
    /// an in-flight join is **not** reaped.
    pub(crate) const fn is_unreaped(&self) -> bool {
        matches!(self, Self::Pending(_) | Self::Joining(_))
    }
}

/// The runtime-owned current attempt handle (control trigger only).
///
/// The coordinator keeps exactly the cancellation trigger the attempt task
/// runs against so that `cancel_current_attempt` can request cancellation.
/// It does **not** own a second cancellation state machine and never
/// decides a terminal outcome; [`AgentExecution`](crate::agent::AgentExecution)
/// remains the attempt execution/terminal authority.
pub(crate) struct CurrentAttempt {
    pub(crate) configuration: AdmittedConfiguration,
    /// The attempt identity.
    pub(crate) attempt_id: AttemptId,
    /// The attempt cancellation trigger observed by the loop.
    pub(crate) cancellation: AgentCancellation,
    /// What this runtime admitted (Issue #351). Runtime-owned admission
    /// provenance, never Goal lifecycle state.
    pub(crate) provenance: super::AttemptProvenance,
}

/// The runtime-owned manual compaction currently holding the conversation.
///
/// Manual compaction is not an attempt: it allocates no `AttemptId`, emits no
/// attempt lifecycle, and cannot execute tools. The coordinator nevertheless
/// retains its cancellation trigger so runtime drain can supervise the
/// provider-backed summary to terminal settlement.
pub(crate) struct CurrentManualCompaction {
    pub(crate) operation_id: crate::runtime::identity::CompactionOperationId,
    pub(crate) cancellation: AgentCancellation,
}

/// The scheduled workflow run's coordinator owner record.
pub(crate) struct CurrentScheduled {
    pub(crate) id: crate::runtime::workflow::scheduled::lifecycle::ScheduledWorkflowStartId,
    pub(crate) owner: String,
    pub(crate) cancellation: Arc<ScheduledCancellation>,
    pub(crate) graph_finished: bool,
    pub(crate) recovered_writer: bool,
    pub(crate) settlement_wake: Arc<tokio::sync::Notify>,
}

/// The scheduled cancellation trigger the coordinator retains so drain and
/// `WorkflowCancel` can request cancellation of the running graph.
pub(crate) struct ScheduledCancellation {
    pub(crate) signal: crate::runtime::cancellation::CancellationSignal,
    pub(crate) reason: Mutex<CancellationReason>,
}

impl ScheduledCancellation {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            signal: crate::runtime::cancellation::CancellationSignal::default(),
            reason: Mutex::new(CancellationReason::UserRequested),
        })
    }
    pub(crate) fn cancel(&self, reason: CancellationReason) {
        let mut held = self.reason.lock().expect("scheduled cancellation");
        if !self.signal.is_cancelled() {
            *held = reason;
            self.signal.cancel();
        }
    }
    pub(crate) fn execution(
        self: &Arc<Self>,
    ) -> crate::runtime::cancellation::ExecutionCancellation {
        crate::runtime::cancellation::ExecutionCancellation::new(self.signal.clone(), self.clone())
    }
}

impl crate::runtime::cancellation::CancellationCause for ScheduledCancellation {
    fn cause(&self) -> CancellationReason {
        *self.reason.lock().expect("scheduled cancellation")
    }
}

impl IdleConversation {
    /// The coordinator-owned state, hydrating it from the durable head the
    /// first time it is needed.
    pub(crate) fn hydrate(
        &mut self,
        store: &dyn ConversationStore,
    ) -> Result<Option<&mut ConversationState>, super::ConversationRuntimeError> {
        if matches!(self, Self::Durable) {
            *self = Self::Owned(super::ConversationRuntime::recovered_conversation(store)?);
        }
        Ok(match self {
            Self::Owned(conversation) => Some(conversation),
            Self::Durable => None,
        })
    }

    /// The coordinator-owned state once hydrated.
    pub(crate) fn owned(&self) -> Option<&ConversationState> {
        match self {
            Self::Owned(conversation) => Some(conversation),
            Self::Durable => None,
        }
    }

    /// Moves the hydrated, coordinator-owned state out to its next owner.
    pub(crate) fn lend(&mut self) -> Option<ConversationState> {
        match std::mem::replace(self, Self::Durable) {
            Self::Owned(conversation) => Some(conversation),
            previous => {
                *self = previous;
                None
            }
        }
    }
}

impl ForegroundSlot {
    /// Whether no foreground operation (or poison terminal) occupies the
    /// conversation: `Idle` is the only admit-capable state.
    pub(crate) const fn is_idle(&self) -> bool {
        matches!(self, Self::Idle(_))
    }

    /// Whether any foreground operation owns the execution right.
    /// `Poisoned` counts: it also blocks admission.
    pub(crate) const fn is_foreground_active(&self) -> bool {
        !matches!(self, Self::Idle(_))
    }

    /// Whether a lending task lost the conversation state to a panic.
    #[cfg(test)]
    pub(crate) const fn is_poisoned(&self) -> bool {
        matches!(self, Self::Poisoned)
    }

    /// The coordinator-owned `IdleConversation`, wherever it is retained:
    /// inside `Idle` directly, or inside a `Scheduled` owner.
    pub(crate) const fn idle_conversation(&self) -> Option<&IdleConversation> {
        match self {
            Self::Idle(conversation) | Self::Scheduled { conversation, .. } => Some(conversation),
            Self::Attempt(_) | Self::ManualCompaction(_) | Self::Poisoned => None,
        }
    }

    /// The mutable coordinator-owned `IdleConversation`, wherever it is
    /// retained.
    pub(crate) fn idle_conversation_mut(&mut self) -> Option<&mut IdleConversation> {
        match self {
            Self::Idle(conversation) | Self::Scheduled { conversation, .. } => Some(conversation),
            Self::Attempt(_) | Self::ManualCompaction(_) | Self::Poisoned => None,
        }
    }

    /// Whether the canonical `ConversationState` is lent to a foreground
    /// task (attempt or manual compaction), so the coordinator owns none.
    pub(crate) const fn is_lent(&self) -> bool {
        matches!(self, Self::Attempt(_) | Self::ManualCompaction(_))
    }

    /// Hydrates the coordinator-owned conversation state when it is
    /// retained (idle or scheduled). `None` while lent or poisoned.
    pub(crate) fn hydrate(
        &mut self,
        store: &dyn ConversationStore,
    ) -> Result<Option<&mut ConversationState>, super::ConversationRuntimeError> {
        match self.idle_conversation_mut() {
            Some(conversation) => conversation.hydrate(store),
            None => Ok(None),
        }
    }

    /// The coordinator-owned hydrated conversation state.
    pub(crate) fn owned(&self) -> Option<&ConversationState> {
        self.idle_conversation().and_then(IdleConversation::owned)
    }

    /// Whether runtime MCP catalog publication or a staged configuration
    /// commit must wait: both are blocked by every foreground state
    /// (`Attempt`, `ManualCompaction`, `Scheduled`, `Poisoned`) alike —
    /// the unified publication matrix (Contract 2).
    pub(crate) const fn blocks_runtime_publication(&self) -> bool {
        self.is_foreground_active()
    }

    /// The current attempt owner, if any.
    pub(crate) const fn as_attempt(&self) -> Option<&CurrentAttempt> {
        match self {
            Self::Attempt(current) => Some(current),
            _ => None,
        }
    }

    /// The current scheduled owner, if any.
    pub(crate) const fn as_scheduled(&self) -> Option<&CurrentScheduled> {
        match self {
            Self::Scheduled { current, .. } => Some(current),
            _ => None,
        }
    }

    /// The current scheduled owner, mutably.
    pub(crate) fn as_scheduled_mut(&mut self) -> Option<&mut CurrentScheduled> {
        match self {
            Self::Scheduled { current, .. } => Some(current),
            _ => None,
        }
    }

    /// The current manual-compaction owner, if any.
    #[cfg(test)]
    pub(crate) const fn as_manual_compaction(&self) -> Option<&CurrentManualCompaction> {
        match self {
            Self::ManualCompaction(current) => Some(current),
            _ => None,
        }
    }

    /// Requests cancellation on every cancellable foreground owner, via one
    /// exhaustive match — drain cannot forget a variant.
    pub(crate) fn cancel_for_drain(&self) {
        match self {
            Self::Attempt(current) => {
                let _ = current
                    .cancellation
                    .request_cancel(CancellationReason::RuntimeShutdown);
            }
            Self::Scheduled { current, .. } => {
                current
                    .cancellation
                    .cancel(CancellationReason::RuntimeShutdown);
            }
            Self::ManualCompaction(current) => {
                let _ = current
                    .cancellation
                    .request_cancel(CancellationReason::RuntimeShutdown);
            }
            Self::Idle(_) | Self::Poisoned => {}
        }
    }

    /// The interaction cancellation reason of the active attempt's
    /// cancellation authority, for the drain's settlement relabeling.
    pub(crate) fn attempt_cancel_reason(&self) -> CancellationReason {
        match self {
            Self::Attempt(current) => current.cancellation.reason(),
            _ => CancellationReason::RuntimeShutdown,
        }
    }

    /// Whether steering can be evaluated at this boundary: `Scheduled`,
    /// `ManualCompaction`, and `Poisoned` all refuse steer admission.
    pub(crate) const fn steering_blocked(&self) -> bool {
        matches!(
            self,
            Self::Scheduled { .. } | Self::ManualCompaction(_) | Self::Poisoned
        )
    }

    /// Whether no live foreground owner remains for drain to await:
    /// `Idle` or `Poisoned`. `Poisoned` is terminal and already recorded
    /// its failure evidence, so drain proceeds instead of hanging.
    pub(crate) const fn drain_settled(&self) -> bool {
        matches!(self, Self::Idle(_) | Self::Poisoned)
    }

    /// Atomically lends the hydrated conversation state to a new manual
    /// compaction: `Idle(Owned)` -> `ManualCompaction`, returning the state.
    /// Any other state is refused without change.
    pub(crate) fn lend_for_manual_compaction(
        &mut self,
        current: CurrentManualCompaction,
    ) -> Option<ConversationState> {
        match std::mem::replace(self, Self::ManualCompaction(current)) {
            Self::Idle(IdleConversation::Owned(conversation)) => Some(conversation),
            Self::Idle(IdleConversation::Durable) => {
                *self = Self::Idle(IdleConversation::Durable);
                None
            }
            previous => {
                *self = previous;
                None
            }
        }
    }

    /// Installs the attempt owner record after the caller lent the
    /// conversation state out of `Idle`. On a non-`Idle` slot it refuses
    /// and returns the owner back.
    pub(crate) fn start_attempt(&mut self, attempt: CurrentAttempt) -> Result<(), CurrentAttempt> {
        if !self.is_idle() {
            return Err(attempt);
        }
        *self = Self::Attempt(attempt);
        Ok(())
    }

    /// Claims the slot for a scheduled run while retaining the coordinator
    /// conversation. Requires `Idle`; on refusal the slot is unchanged.
    pub(crate) fn start_scheduled(&mut self, current: CurrentScheduled) -> bool {
        if !self.is_idle() {
            return false;
        }
        match std::mem::replace(self, Self::Poisoned) {
            Self::Idle(conversation) => {
                *self = Self::Scheduled {
                    current,
                    conversation,
                };
                true
            }
            _ => unreachable!("idle just proven"),
        }
    }

    /// Restores the conversation state after a settled attempt and frees
    /// the slot to `Idle`. Returns the finished owner record.
    pub(crate) fn finish_attempt(
        &mut self,
        attempt_id: &AttemptId,
        conversation: ConversationState,
    ) -> Option<CurrentAttempt> {
        let finished =
            matches!(self.as_attempt(), Some(current) if &current.attempt_id == attempt_id);
        if !finished {
            return None;
        }
        match std::mem::replace(self, Self::Idle(IdleConversation::Owned(conversation))) {
            Self::Attempt(current) => Some(current),
            previous => {
                *self = previous;
                None
            }
        }
    }

    /// Restores the conversation state after a settled manual compaction
    /// and frees the slot to `Idle`. The physical task tail is tracked
    /// separately in `ManualTailState` — this is only the slot transition.
    pub(crate) fn finish_manual_compaction(
        &mut self,
        conversation: ConversationState,
    ) -> Option<CurrentManualCompaction> {
        match std::mem::replace(self, Self::Idle(IdleConversation::Owned(conversation))) {
            Self::ManualCompaction(current) => Some(current),
            previous => {
                *self = previous;
                None
            }
        }
    }

    /// Releases the slot of a settled scheduled run owned by `owner`,
    /// restoring `Idle` with the retained conversation.
    pub(crate) fn finish_scheduled(&mut self, owner: &str) -> bool {
        let owned = matches!(self.as_scheduled(), Some(current) if current.owner == owner);
        if !owned {
            return false;
        }
        if let Self::Scheduled { conversation, .. } = std::mem::replace(self, Self::Poisoned) {
            *self = Self::Idle(conversation);
            true
        } else {
            unreachable!("scheduled owner just proven present")
        }
    }

    /// Transitions a lending slot (`Attempt` or `ManualCompaction`) whose
    /// task panicked before returning the conversation state into
    /// `Poisoned`. An `Idle`, `Scheduled`, or already-`Poisoned` slot is
    /// left unchanged and reported as not transitioned.
    pub(crate) fn poison(&mut self) -> bool {
        if self.is_lent() {
            *self = Self::Poisoned;
            true
        } else {
            false
        }
    }

    /// Lends the coordinator-owned conversation state to a new foreground
    /// owner. Only callable while `Idle` and hydrated.
    pub(crate) fn lend(&mut self) -> Option<ConversationState> {
        match self {
            Self::Idle(conversation) => conversation.lend(),
            _ => None,
        }
    }
}

impl Drop for AttemptTaskPoisonGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let Some(inner) = self.inner.upgrade() else {
            return;
        };
        inner.poison_foreground_for_task(&self.attempt_id);
    }
}

impl AttemptTaskPoisonGuard {
    pub(crate) fn arm(inner: &Arc<RuntimeInner>, attempt_id: AttemptId) -> Self {
        Self {
            inner: Arc::downgrade(inner),
            attempt_id,
            armed: true,
        }
    }

    /// Marks the lending task past its last panic-reachable point: the
    /// conversation state is returned normally from here.
    pub(crate) fn disarm(&mut self) {
        self.armed = false;
    }
}
