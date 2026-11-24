//! Conversation-owned scheduled execution: no synthetic `Attempt` or `ToolCall`.
//!
//! The narrow coordinator interfaces the extracted Scheduled Workflow
//! Executor consumes (Issue #606). The executor owns preparation,
//! execution, panic containment, physical settlement confirmation and
//! retry backoff; the coordinator owns foreground arbitration, the frozen
//! configuration snapshot, the atomic reservation/Claim, lifecycle
//! ordering, and the exact-owner slot release.
use super::{Arc, ConversationRuntime, RuntimeInner};
use crate::runtime::workflow::scheduled::lifecycle::{
    ClaimedScheduledWorkflow, ScheduledWorkflowStartId,
};

impl ConversationRuntime {
    /// A wake handle carries no runtime ownership or execution authority.
    pub(crate) fn scheduled_wake(&self) -> Arc<tokio::sync::Notify> {
        self.inner.mailbox.wake()
    }
}

/// The atomic output of a scheduled foreground reservation: the durable
/// start to run, the frozen configuration generation, the exact owner
/// installed in the foreground slot, and the business admission guarding
/// lifecycle ordering until the slot is released.
pub(crate) struct ScheduledReservation {
    pub(crate) start: crate::runtime::workflow::scheduled::lifecycle::ScheduledWorkflowStart,
    /// A recovered Claim owns the retirement obligation; the graph is not replayed.
    pub(crate) recovered_writer: bool,
    pub(crate) resources: Arc<crate::runtime::RuntimeResourceSnapshot>,
    pub(crate) model: crate::model::session::SessionModelConfig,
    pub(crate) approval: crate::runtime::ApprovalMode,
    pub(crate) owner: String,
    pub(crate) cancellation: Arc<super::ScheduledCancellation>,
    pub(crate) admission: crate::runtime::types::LifecycleAdmission,
}

/// The frozen configuration inputs a scheduled command preparation or a
/// scheduled start executes against. The Claim critical section
/// re-validates the same generation before the durable Claim commits.
pub(crate) struct ScheduledSnapshot {
    pub(crate) resources: Arc<crate::runtime::RuntimeResourceSnapshot>,
    pub(crate) model: crate::model::session::SessionModelConfig,
    pub(crate) approval: crate::runtime::ApprovalMode,
}

/// The outcome of the one scheduled Claim critical section.
pub(crate) enum ScheduledClaim {
    /// The durable Claim committed: run the claimed start against the
    /// capability lease and the frozen permission-reviewer invocation.
    Claimed {
        claim: Box<ClaimedScheduledWorkflow>,
        lease: crate::capabilities::AttemptCapabilityLease,
        auxiliary_invocation: Box<crate::model::ResolvedModelInvocation>,
    },
    /// The configuration generation moved between reservation and Claim:
    /// the caller records a retry backoff and defers the start.
    Stale,
    /// The runtime stopped running or the cancellation signal fired before
    /// the Claim committed; the caller just retires its owner.
    Refused,
}

/// The read-only facts the reconciliation path needs about an exact
/// scheduled foreground owner, cloned out under the coordinator lock.
pub(crate) struct ScheduledOwnerProbe {
    pub(crate) graph_finished: bool,
    pub(crate) recovered_writer: bool,
    pub(crate) settlement_wake: Arc<tokio::sync::Notify>,
}

impl RuntimeInner {
    /// `admit_next` critical section (Issue #606): while the coordinator
    /// lock is held, prove lifecycle/slot/maintenance/commit/durability
    /// gates, select the next due durable start (recovered `Claimed`
    /// before any `Accepted`; `retry_ready` carries the executor's
    /// per-start backoff), freeze the configuration generation, install
    /// the exact-owner `Scheduled` slot, and hold the business admission
    /// until the executor releases the slot.
    pub(crate) fn reserve_scheduled_start(
        &self,
        retry_ready: impl Fn(&ScheduledWorkflowStartId) -> bool,
    ) -> Option<ScheduledReservation> {
        use crate::runtime::workflow::scheduled::lifecycle::ScheduledWorkflowState;
        let mut state = self.lock_state();
        if !self.lifecycle.is_running()
            || state.foreground.slot.is_foreground_active()
            || state.foreground.manual_tail.is_unreaped()
            || state.config.configuration_commit_waiting
            || self.durability_gate.is_failed()
        {
            return None;
        }
        let binding = self.tool_runtime.schedule().binding().cloned()?;
        let pending = self.store.scheduled_workflow_pending(256).ok()?;
        // A recovered Claim owns the foreground retirement obligation before
        // any fresh Human or Accepted graph can acquire this conversation.
        let recovered = pending
            .iter()
            .find(|head| matches!(head.state, ScheduledWorkflowState::Claimed { .. }))
            .cloned();
        if recovered.is_none() && !matches!(self.mailbox.select_pending_batch(), Ok(None)) {
            return None;
        }
        let next = recovered.or_else(|| {
            pending.into_iter().find(|head| {
                matches!(&head.state, ScheduledWorkflowState::Accepted { .. })
                    && retry_ready(&head.id)
            })
        })?;
        if next.binding != binding {
            return None;
        }
        let start = self.store.scheduled_workflow_get(&next.id).ok()??;
        let resources = state.config.resources.clone();
        let model = state.config.model.config().clone();
        let approval = state.config.effective_approval_mode;
        let owner = format!("scheduled:{}", uuid::Uuid::now_v7());
        let cancellation = super::ScheduledCancellation::new();
        let recovered_writer = matches!(next.state, ScheduledWorkflowState::Claimed { .. });
        let claimed = state
            .foreground
            .slot
            .start_scheduled(super::foreground::CurrentScheduled {
                id: next.id.clone(),
                owner: owner.clone(),
                cancellation: cancellation.clone(),
                graph_finished: recovered_writer,
                recovered_writer,
                settlement_wake: Arc::default(),
            });
        debug_assert!(claimed, "foreground idleness proven under this lock");
        let admission = self
            .lifecycle
            .try_enter_running()
            .expect("native scheduled admission owns coordinator fence");
        Some(ScheduledReservation {
            start,
            recovered_writer,
            resources,
            model,
            approval,
            owner,
            cancellation,
            admission,
        })
    }

    /// `claim_scheduled_start` (Issue #606): the one critical section that
    /// re-checks lifecycle and cancellation, re-validates the frozen
    /// resources/model/approval snapshot, freezes the Auxiliary Reviewer
    /// Invocation, acquires the `AttemptCapabilityLease`, and durably claims
    /// the exact start via a Lifecycle Running Commit. A stale claim is
    /// reported to the executor for retry backoff, never replayed.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn claim_scheduled_start(
        &self,
        id: &ScheduledWorkflowStartId,
        contract: &crate::runtime::workflow::scheduled::PinnedScheduledWorkflow,
        snapshot: &ScheduledSnapshot,
        owner: &str,
        cancellation: &Arc<super::ScheduledCancellation>,
        acquire_lease: impl FnOnce() -> crate::capabilities::AttemptCapabilityLease,
    ) -> Result<ScheduledClaim, String> {
        let state = self.lock_state();
        if !self.lifecycle.is_running() || cancellation.signal.is_cancelled() {
            return Ok(ScheduledClaim::Refused);
        }
        if !Arc::ptr_eq(&snapshot.resources, &state.config.resources)
            || state.config.model.config() != &snapshot.model
            || state.config.effective_approval_mode != snapshot.approval
        {
            return Ok(ScheduledClaim::Stale);
        }
        // Freeze the permission-reviewer invocation in the same critical
        // section as Claim so a concurrent model-selection commit cannot
        // swap M1→M2 between durable Claim and reviewer construction.
        let auxiliary_invocation = state
            .config
            .model
            .snapshot()
            .map_err(|error| error.to_string())?
            .summary_invocation()
            .clone();
        let lease = acquire_lease();
        let claim = self
            .lifecycle
            .with_running_commit(|| {
                self.store
                    .scheduled_workflow_claim(id, contract, owner, self.clock.now())
            })
            .map_err(|e| format!("scheduled Claim lifecycle: {e:?}"))?
            .map_err(|e| e.to_string())?;
        Ok(
            claim.map_or(ScheduledClaim::Refused, |claim| ScheduledClaim::Claimed {
                claim: Box::new(claim),
                lease,
                auxiliary_invocation: Box::new(auxiliary_invocation),
            }),
        )
    }

    /// The exact-owner slot release, the settlement wake, and the admission
    /// worker wake, sequenced exactly once per native owner.
    pub(crate) fn finish_scheduled(&self, owner: &str) {
        self.lock_state().foreground.slot.finish_scheduled(owner);
        self.settlement.notify_waiters();
        self.mailbox.wake().notify_one();
    }

    /// The read-only facts about an exact scheduled foreground owner.
    pub(crate) fn probe_scheduled_owner(
        &self,
        id: &ScheduledWorkflowStartId,
    ) -> Option<ScheduledOwnerProbe> {
        self.lock_state()
            .foreground
            .slot
            .as_scheduled()
            .filter(|owner| &owner.id == id)
            .map(|owner| ScheduledOwnerProbe {
                graph_finished: owner.graph_finished,
                recovered_writer: owner.recovered_writer,
                settlement_wake: owner.settlement_wake.clone(),
            })
    }

    /// The executor marks its graph physically closed while it keeps the
    /// exact native owner for settlement reconciliation.
    pub(crate) fn mark_scheduled_graph_finished(&self, owner: &str) {
        if let Some(current) = self
            .lock_state()
            .foreground
            .slot
            .as_scheduled_mut()
            .filter(|current| current.owner == owner)
        {
            current.graph_finished = true;
        }
    }

    /// Cancel the running scheduled owner, if `id` still owns the slot.
    pub(crate) fn cancel_scheduled_owner(&self, id: &ScheduledWorkflowStartId) {
        if let Some(current) = self
            .lock_state()
            .foreground
            .slot
            .as_scheduled()
            .filter(|owner| &owner.id == id)
        {
            current
                .cancellation
                .cancel(crate::runtime::types::CancellationReason::UserRequested);
        }
    }

    /// The frozen configuration generation scheduled command preparation
    /// runs against, cloned out under the coordinator lock.
    pub(crate) fn scheduled_snapshot(&self) -> ScheduledSnapshot {
        let state = self.lock_state();
        ScheduledSnapshot {
            resources: state.config.resources.clone(),
            model: state.config.model.config().clone(),
            approval: state.config.effective_approval_mode,
        }
    }
}
