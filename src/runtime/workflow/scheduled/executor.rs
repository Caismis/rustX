//! The Scheduled Workflow Executor (Issue #606): scheduled preparation,
//! `claim_scheduled_start`, execution with deadline and panic containment,
//! failure-outcome mapping, settlement reconciliation and retry backoff all
//! live here, off `ConversationRuntime` — whose only responsibility is
//! exposing the `ForegroundSlot` and composing this executor at construction.
use std::sync::{Arc, Mutex};

use futures_util::FutureExt;

use super::lifecycle::{
    ClaimedScheduledWorkflow, QuiescenceSource, ScheduledWorkflowOutcome,
    ScheduledWorkflowOwnerRetirement, ScheduledWorkflowQuiescence, ScheduledWorkflowSettlement,
    ScheduledWorkflowStart, ScheduledWorkflowStartId, ScheduledWorkflowState, bounded_reason,
};
use super::{ScheduledWorkflowPreparation, ScheduledWorkflowPreparationError as PreparationError};
use crate::capabilities::CapabilityCoordinator;
use crate::runtime::auxiliary::executor::AuxiliaryExecutor;
use crate::runtime::conversation_runtime::activity::TaskActivity;
use crate::runtime::conversation_runtime::{
    CoordinatorState, CurrentScheduled, ScheduledCancellation,
};
use crate::runtime::inbound::ConversationInboundMailbox;
use crate::runtime::interaction::InteractionCoordinator;
use crate::runtime::types::DurabilityGate;
use crate::runtime::types::RuntimeClock;
use crate::runtime::workflow::{WorkflowRunError, WorkflowRuntime};
use crate::runtime::{
    ApprovalMode, CancellationReason, ConversationLifecycle, MonotonicClock,
    RuntimeResourceSnapshot,
};
use crate::tools::runtime::ConversationToolRuntime;

fn scheduled_failure_outcome(error: &WorkflowRunError) -> ScheduledWorkflowOutcome {
    match error.execution_status() {
        crate::tools::types::ToolExecutionStatus::Cancelled { .. } => {
            ScheduledWorkflowOutcome::Cancelled
        }
        crate::tools::types::ToolExecutionStatus::TimedOut => ScheduledWorkflowOutcome::TimedOut,
        crate::tools::types::ToolExecutionStatus::OutcomeUnknown { .. } => {
            ScheduledWorkflowOutcome::OutcomeUnknown {
                reason: bounded_reason(error.to_string()),
            }
        }
        _ => ScheduledWorkflowOutcome::Failed {
            reason: bounded_reason(error.to_string()),
        },
    }
}

/// Construction dependencies of the executor: every field is a shared
/// handle already owned elsewhere; nothing here pins a runtime.
pub(crate) struct ScheduledExecutorParts {
    pub(crate) state: Arc<Mutex<CoordinatorState>>,
    pub(crate) lifecycle: ConversationLifecycle,
    pub(crate) store: Arc<dyn crate::durable::ConversationStore>,
    pub(crate) mailbox: ConversationInboundMailbox,
    pub(crate) interaction: Arc<InteractionCoordinator>,
    pub(crate) subagents: Option<crate::runtime::subagent::SubagentRegistry>,
    pub(crate) capability: CapabilityCoordinator,
    pub(crate) tool_runtime: ConversationToolRuntime,
    pub(crate) auxiliary_executor: AuxiliaryExecutor,
    pub(crate) monotonic_clock: Arc<dyn MonotonicClock>,
    pub(crate) clock: Arc<dyn RuntimeClock>,
    pub(crate) activity: TaskActivity,
    pub(crate) settlement: Arc<tokio::sync::Notify>,
    pub(crate) durability_gate: Arc<DurabilityGate>,
    pub(crate) spawner: tokio::runtime::Handle,
    #[cfg(test)]
    pub(crate) probe: Arc<Mutex<Option<crate::runtime::conversation_runtime::CoordinatorProbe>>>,
}

/// The scheduled-workflow owner: one instance per conversation, holding only
/// shared handles. `ConversationRuntime` keeps no scheduled business logic.
pub(crate) struct ScheduledExecutor {
    state: Arc<Mutex<CoordinatorState>>,
    lifecycle: ConversationLifecycle,
    store: Arc<dyn crate::durable::ConversationStore>,
    mailbox: ConversationInboundMailbox,
    interaction: Arc<InteractionCoordinator>,
    subagents: Option<crate::runtime::subagent::SubagentRegistry>,
    capability: CapabilityCoordinator,
    tool_runtime: ConversationToolRuntime,
    auxiliary_executor: AuxiliaryExecutor,
    monotonic_clock: Arc<dyn MonotonicClock>,
    clock: Arc<dyn RuntimeClock>,
    activity: TaskActivity,
    settlement: Arc<tokio::sync::Notify>,
    durability_gate: Arc<DurabilityGate>,
    spawner: tokio::runtime::Handle,
    #[cfg(test)]
    probe: Arc<Mutex<Option<crate::runtime::conversation_runtime::CoordinatorProbe>>>,
}

impl ScheduledExecutor {
    pub(crate) fn new(parts: ScheduledExecutorParts) -> Self {
        Self {
            state: parts.state,
            lifecycle: parts.lifecycle,
            store: parts.store,
            mailbox: parts.mailbox,
            interaction: parts.interaction,
            subagents: parts.subagents,
            capability: parts.capability,
            tool_runtime: parts.tool_runtime,
            auxiliary_executor: parts.auxiliary_executor,
            monotonic_clock: parts.monotonic_clock,
            clock: parts.clock,
            activity: parts.activity,
            settlement: parts.settlement,
            durability_gate: parts.durability_gate,
            spawner: parts.spawner,
            #[cfg(test)]
            probe: parts.probe,
        }
    }

    /// Reconciliation never takes terminal publication from a registered owner.
    pub(crate) fn reconcile(
        &self,
        state: &CoordinatorState,
        binding: &crate::scheduler::ScheduleBinding,
        id: &ScheduledWorkflowStartId,
        (after, limit): (u64, u32),
        clock: &dyn RuntimeClock,
    ) -> Result<crate::scheduler::ScheduleOperationResult, crate::runtime::inbound::MailboxError>
    {
        let own = state
            .foreground
            .slot
            .as_scheduled()
            .filter(|owner| &owner.id == id);
        let retirement = ScheduledWorkflowOwnerRetirement {
            start_id: id.clone(),
            recovered_writer: own.is_none_or(|owner| owner.recovered_writer),
            agent_domain_quiescent: self
                .subagents
                .as_ref()
                .is_some_and(|registry| !registry.owns_idle_work()),
            interactions_closed: !self.interaction.scheduled_scope_is_pending(id),
        };
        if let Some(owner) = own {
            // The graph closing does not transfer its known outcome or Return.
            if owner.graph_finished {
                self.store
                    .scheduled_workflow_recover_effects(id, after, limit, &retirement)?;
                owner.settlement_wake.notify_one();
            }
            return Ok(crate::scheduler::ScheduleOperationResult::WorkflowRecovery(
                self.store.scheduled_workflow_recovery(id, after, limit)?,
            ));
        }
        self.mailbox.schedule_operation(
            self.store.as_ref(),
            binding,
            &crate::scheduler::ScheduleOperation::WorkflowReconcileOwned {
                id: id.clone(),
                after,
                limit,
                retirement,
            },
            clock,
        )
    }

    /// Reserve only a native foreground preparation slot. Preparation is off
    /// the coordinator lock; the final publication fence still owns Claim.
    #[allow(clippy::too_many_lines)] // One foreground reservation and exact native retirement handoff.
    pub(crate) fn admit_next(self: &Arc<Self>) -> bool {
        let mut state = self.state.lock().expect("coordinator lock poisoned");
        if !self.lifecycle.is_running()
            || state.foreground.slot.is_foreground_active()
            || state.foreground.manual_tail.is_unreaped()
            || state.config.configuration_commit_waiting
            || self.durability_gate.is_failed()
        {
            return false;
        }
        let Some(binding) = self.tool_runtime.schedule().binding().cloned() else {
            return false;
        };
        let Ok(pending) = self.store.scheduled_workflow_pending(256) else {
            return false;
        };
        let now = self.monotonic_clock.now_millis();
        state
            .foreground
            .scheduled_retries
            .retain(|id, _| pending.iter().any(|head| &head.id == id));
        // A recovered Claim owns the foreground retirement obligation before
        // any fresh Human or Accepted graph can acquire this conversation.
        let recovered = pending
            .iter()
            .find(|head| matches!(head.state, ScheduledWorkflowState::Claimed { .. }))
            .cloned();
        if recovered.is_none() && !matches!(self.mailbox.select_pending_batch(), Ok(None)) {
            return false;
        }
        let Some(next) = recovered.or_else(|| {
            pending.into_iter().find(|head| {
                matches!(&head.state, ScheduledWorkflowState::Accepted { .. })
                    && state
                        .foreground
                        .scheduled_retries
                        .get(&head.id)
                        .is_none_or(|at| *at <= now)
            })
        }) else {
            return false;
        };
        if next.binding != binding {
            return false;
        }
        let Ok(Some(start)) = self.store.scheduled_workflow_get(&next.id) else {
            return false;
        };
        let resources = state.config.resources.clone();
        let model = state.config.model.config().clone();
        let approval = state.config.effective_approval_mode;
        let owner = format!("scheduled:{}", uuid::Uuid::now_v7());
        let cancellation = ScheduledCancellation::new();
        let recovered_writer = matches!(next.state, ScheduledWorkflowState::Claimed { .. });
        let claimed = state.foreground.slot.start_scheduled(CurrentScheduled {
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
        drop(state);
        let executor = self.clone();
        let activity = self.activity.lease();
        self.spawner.spawn(async move {
            let result = if recovered_writer {
                let _lease = executor
                    .capability
                    .acquire_attempt_lease_for(resources.capability().clone());
                executor
                    .finish_scheduled_start(
                        &start.head.id,
                        ScheduledWorkflowOutcome::OutcomeUnknown {
                            reason: "native writer recovered a Claim; graph was not replayed"
                                .into(),
                        },
                        None,
                        true,
                    )
                    .await
            } else {
                executor
                    .clone()
                    .run_scheduled_start(
                        start,
                        resources,
                        model,
                        approval,
                        &owner,
                        cancellation.clone(),
                    )
                    .await
            };
            if let Err(error) = result {
                tracing::warn!(%error,"scheduled Workflow native owner refused or failed");
            }
            let mut state = executor.state.lock().expect("coordinator lock poisoned");
            state.foreground.slot.finish_scheduled(&owner);
            drop(state);
            drop(admission);
            drop(activity);
            executor.settlement.notify_waiters();
            executor.mailbox.wake().notify_one();
        });
        true
    }

    /// `claim_scheduled_start` (Issue #606): the one critical section that
    /// re-checks lifecycle and cancellation, re-validates the frozen
    /// resources/model/approval snapshot, freezes the Auxiliary Reviewer
    /// Invocation, acquires the `AttemptCapabilityLease`, and durably claims
    /// the exact start via a Lifecycle Running Commit. A stale claim is
    /// rejected (retry backoff), never replayed.
    #[allow(clippy::too_many_arguments)]
    fn claim_scheduled_start(
        &self,
        id: &ScheduledWorkflowStartId,
        contract: &super::PinnedScheduledWorkflow,
        resources: &Arc<RuntimeResourceSnapshot>,
        model: &crate::model::session::SessionModelConfig,
        approval: ApprovalMode,
        owner: &str,
        cancellation: &Arc<ScheduledCancellation>,
    ) -> Result<
        Option<(
            ClaimedScheduledWorkflow,
            crate::capabilities::AttemptCapabilityLease,
            crate::model::ResolvedModelInvocation,
        )>,
        String,
    > {
        let mut state = self.state.lock().expect("coordinator lock poisoned");
        if !self.lifecycle.is_running() || cancellation.signal.is_cancelled() {
            return Ok(None);
        }
        if !Arc::ptr_eq(resources, &state.config.resources)
            || state.config.model.config() != model
            || state.config.effective_approval_mode != approval
        {
            state.foreground.scheduled_retries.insert(
                id.clone(),
                self.monotonic_clock.now_millis().saturating_add(1000),
            );
            self.store
                .scheduled_workflow_wait(
                    id,
                    "native configuration changed before Claim",
                    self.clock.now() + chrono::Duration::seconds(1),
                )
                .map_err(|e| e.to_string())?;
            return Ok(None);
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
        let lease = self
            .capability
            .acquire_attempt_lease_for(resources.capability().clone());
        let claim = self
            .lifecycle
            .with_running_commit(|| {
                self.store
                    .scheduled_workflow_claim(id, contract, owner, self.clock.now())
            })
            .map_err(|e| format!("scheduled Claim lifecycle: {e:?}"))?
            .map_err(|e| e.to_string())?;
        Ok(claim.map(|claim| (claim, lease, auxiliary_invocation)))
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    async fn run_scheduled_start(
        self: Arc<Self>,
        start: ScheduledWorkflowStart,
        resources: Arc<RuntimeResourceSnapshot>,
        model: crate::model::session::SessionModelConfig,
        approval: ApprovalMode,
        owner: &str,
        cancellation: Arc<ScheduledCancellation>,
    ) -> Result<(), String> {
        let id = start.head.id.clone();
        let binding = start.head.binding.clone();
        let saved = start
            .contract
            .clone()
            .ok_or("Accepted Workflow has no contract")?;
        let arguments = start
            .arguments
            .clone()
            .ok_or("Accepted Workflow has no arguments")?;
        let prepare_resources = resources.clone();
        let prepare_model = model.clone();
        let prepared = tokio::task::spawn_blocking(move || {
            let preparation =
                ScheduledWorkflowPreparation::new(binding, prepare_resources, prepare_model)?
                    .with_approval_mode(approval);
            let prepared = preparation.reconstruct(&saved, &arguments)?;
            Ok::<_, PreparationError>((preparation, prepared))
        })
        .await
        .map_err(|e| e.to_string())?;
        let (preparation, prepared) = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                let mut state = self.state.lock().expect("coordinator lock poisoned");
                if self.lifecycle.is_running() {
                    if matches!(
                        error,
                        PreparationError::DependencyUnavailable
                            | PreparationError::UnpreparedResources
                    ) {
                        state.foreground.scheduled_retries.insert(
                            id.clone(),
                            self.monotonic_clock.now_millis().saturating_add(30_000),
                        );
                        self.store
                            .scheduled_workflow_wait(
                                &id,
                                &error.to_string(),
                                self.clock.now() + chrono::Duration::seconds(30),
                            )
                            .map_err(|e| e.to_string())?;
                    } else {
                        self.store
                            .scheduled_workflow_reject(&id, &error.to_string(), self.clock.now())
                            .map_err(|e| e.to_string())?;
                    }
                }
                return Ok(());
            }
        };
        let registry = self
            .subagents
            .clone()
            .ok_or("native Workflow registry unavailable")?;
        let Some((claim, _resources_lease, auxiliary_invocation)) = self.claim_scheduled_start(
            &id,
            prepared.contract(),
            &resources,
            &model,
            approval,
            owner,
            &cancellation,
        )?
        else {
            return Ok(());
        };
        let policy = claim
            .start
            .contract
            .as_ref()
            .expect("native claim contract")
            .execution_policy;
        let timeout = claim
            .start
            .contract
            .as_ref()
            .expect("native claim contract")
            .limits
            .timeout_ms;
        let deadline = self.monotonic_clock.now_millis().saturating_add(timeout);
        let (trigger, execution_cancel) = cancellation.execution().child_execution();
        let deadline_view = execution_cancel.clone();
        let trigger = Arc::new(trigger);
        let mut context = preparation
            .execution_context(self.capability.clone(), id.clone())
            .with_workflow_models(self.auxiliary_executor.clone())
            .with_workflow_deadline(self.monotonic_clock.clone(), deadline, trigger.clone());
        let reviewer = Arc::new(
            crate::runtime::permission::review::AuxiliaryPermissionReviewer::new(
                self.auxiliary_executor.clone(),
                auxiliary_invocation,
                policy.model_timeout,
                resources.revision().get(),
                execution_cancel.child_signal(),
            ),
        );
        context.native = Some(Arc::new(
            crate::tools::invocation::NativeInvocationServices {
                task: Some(Arc::new(
                    crate::runtime::permission::PermissionTaskEvidence::ScheduledClaim {
                        claim: serde_json::to_value(&claim.start)
                            .expect("native scheduled Claim serialization"),
                    },
                )),
                lifecycle: crate::agent::AttemptLifecycle::inert()
                    .with_native_interaction(self.interaction.clone())
                    .with_approval_mode_and_reviewer(approval, reviewer),
                runtime: self.tool_runtime.clone(),
                clock: self.monotonic_clock.clone(),
                leaf_policy: policy.tool_deadline,
                execution: crate::runtime::interaction::InteractionExecution::Scheduled {
                    run: claim.start.head.run_id.clone(),
                },
                scheduling: Arc::new(tokio::sync::RwLock::new(())),
                descendants: crate::tools::invocation::NativeChildScope::default(),
            },
        ));
        let workflow = WorkflowRuntime::new(
            registry,
            self.store.clone(),
            self.tool_runtime.workflows().clone(),
        );
        // A panicking executor still leaves its required effect rows accountable.
        // Keep the native slot until the same proof reconciliation can retire it.
        let execute = std::panic::AssertUnwindSafe(workflow.run_scheduled(
            claim,
            prepared,
            context,
            execution_cancel,
        ))
        .catch_unwind();
        tokio::pin!(execute);
        let result = tokio::select! {biased;
            result=&mut execute=>result,
            ()=self.monotonic_clock.wait_until_millis(deadline)=>{
                if !deadline_view.is_cancelled() {trigger.cancel(crate::tools::deadline::ToolCancellationCause::Deadline(crate::tools::deadline::ToolDeadlineKind::Hard));}
                execute.await
            }
        };
        let result = result.unwrap_or_else(|_| {
            Err(WorkflowRunError::InvocationAuthority(
                "native Workflow executor panicked; effect ownership requires reconciliation"
                    .into(),
            ))
        });
        let (outcome, output) = match result {
            Ok(value) => (ScheduledWorkflowOutcome::Completed, Some(value)),
            Err(error) => (scheduled_failure_outcome(&error), None),
        };
        cancellation.cancel(CancellationReason::ParentCancelled);
        {
            let mut state = self.state.lock().expect("coordinator lock poisoned");
            if let Some(current) = state
                .foreground
                .slot
                .as_scheduled_mut()
                .filter(|current| current.owner == owner)
            {
                current.graph_finished = true;
            }
        }
        #[cfg(test)]
        tokio::task::block_in_place(|| self.enter_probe_gate());
        self.finish_scheduled_start(&id, outcome, output, false)
            .await
    }

    #[cfg(test)]
    fn enter_probe_gate(&self) {
        let gate = self
            .probe
            .lock()
            .expect("coordinator probe lock poisoned")
            .as_ref()
            .and_then(|probe| probe.scheduled_settlement_gate.clone());
        if let Some(gate) = gate {
            gate.enter();
        }
    }

    /// Settlement reconciliation: `AwaitingNativeProof` or a failed durable
    /// settlement retains `ForegroundSlot::Scheduled` and its lease, keeps
    /// reconciling, never releases early, and never replays the graph.
    async fn finish_scheduled_start(
        &self,
        id: &ScheduledWorkflowStartId,
        outcome: ScheduledWorkflowOutcome,
        output: Option<serde_json::Value>,
        recovered_writer: bool,
    ) -> Result<(), String> {
        // The graph is closed. A missing physical proof keeps this exact owner
        // and its resource lease accountable; neither cancellation nor elapsed
        // time permits a second executor or releases deletion/capacity.
        let settlement_wake = self
            .state
            .lock()
            .expect("coordinator lock poisoned")
            .foreground
            .slot
            .as_scheduled()
            .filter(|owner| &owner.id == id)
            .ok_or_else(|| "scheduled terminal publication lost its native owner".to_owned())?
            .settlement_wake
            .clone();
        let mut uncertain = false;
        let mut after = 0;
        loop {
            if let Ok(Some(start)) = self.store.scheduled_workflow_get(id)
                && matches!(start.head.state, ScheduledWorkflowState::Settled { .. })
            {
                return Ok(());
            }
            if let Some(registry) = &self.subagents {
                registry.reconcile_recovered_settlements();
            }
            let retired = ScheduledWorkflowOwnerRetirement {
                start_id: id.clone(),
                recovered_writer,
                agent_domain_quiescent: self
                    .subagents
                    .as_ref()
                    .is_some_and(|registry| !registry.owns_idle_work()),
                interactions_closed: !self.interaction.scheduled_scope_is_pending(id),
            };
            let _ = self
                .store
                .scheduled_workflow_recover_effects(id, after, 100, &retired);
            let final_outcome = if uncertain {
                ScheduledWorkflowOutcome::OutcomeUnknown{reason:"native physical ownership was uncertain; retired without replaying the graph".into()}
            } else {
                outcome.clone()
            };
            match self.store.scheduled_workflow_settle(
                id,
                final_outcome.clone(),
                ScheduledWorkflowQuiescence {
                    source: if recovered_writer {
                        QuiescenceSource::RecoveredOwners
                    } else {
                        QuiescenceSource::NativeExecutor
                    },
                },
                if matches!(final_outcome, ScheduledWorkflowOutcome::Completed) {
                    output.as_ref()
                } else {
                    None
                },
                self.clock.now(),
            ) {
                Ok(
                    ScheduledWorkflowSettlement::Settled
                    | ScheduledWorkflowSettlement::AlreadySettled,
                ) => return Ok(()),
                Ok(ScheduledWorkflowSettlement::AwaitingNativeProof) => {
                    uncertain = true;
                    let _=self.store.scheduled_workflow_blocked(id,"recovery_blocked: native physical proof is missing; inspect exact effect owners and reconcile, never replay this run");
                }
                Err(error) => {
                    tracing::warn!(%error,"scheduled settlement retains its native owner until durability recovers");
                }
            }
            after = self
                .store
                .scheduled_workflow_recovery(id, after, 100)
                .ok()
                .flatten()
                .and_then(|page| page.next_effect_ordinal)
                .unwrap_or(0);
            let until = self.monotonic_clock.now_millis().saturating_add(1000);
            tokio::select! {
                () = self.monotonic_clock.wait_until_millis(until) => {},
                () = settlement_wake.notified() => {},
            }
        }
    }
}

#[cfg(test)]
mod outcome_tests {
    use super::*;
    use crate::runtime::CancellationReason;
    use crate::tools::types::{ToolCancellationPhase, ToolExecutionStatus};
    #[test]
    fn scheduler85_review_wrapped_native_status_keeps_cancel_timeout_and_uncertainty() {
        for status in [
            ToolExecutionStatus::Cancelled {
                reason: CancellationReason::UserRequested,
                phase: ToolCancellationPhase::DuringExecution,
            },
            ToolExecutionStatus::TimedOut,
            ToolExecutionStatus::OutcomeUnknown {
                detail: "remote outcome is unconfirmed".into(),
            },
            ToolExecutionStatus::Failed {
                error: "ordinary failure".into(),
            },
        ] {
            let errors = [
                WorkflowRunError::ToolFailed {
                    node: "tool".into(),
                    status: status.clone(),
                },
                WorkflowRunError::ModelFailed {
                    node: "model".into(),
                    status: status.clone(),
                },
                WorkflowRunError::ChildOutcome {
                    node: "agent".into(),
                    status: status.clone(),
                },
                WorkflowRunError::NodesFailed {
                    failures: std::collections::BTreeMap::from([(
                        "branch".into(),
                        WorkflowRunError::ToolFailed {
                            node: "tool".into(),
                            status: status.clone(),
                        },
                    )]),
                },
            ];
            for error in errors {
                let outcome = scheduled_failure_outcome(&error);
                match status {
                    ToolExecutionStatus::Cancelled { .. } => {
                        assert_eq!(outcome, ScheduledWorkflowOutcome::Cancelled);
                    }
                    ToolExecutionStatus::TimedOut => {
                        assert_eq!(outcome, ScheduledWorkflowOutcome::TimedOut);
                    }
                    ToolExecutionStatus::OutcomeUnknown { .. } => assert!(matches!(
                        outcome,
                        ScheduledWorkflowOutcome::OutcomeUnknown { .. }
                    )),
                    _ => assert!(matches!(outcome, ScheduledWorkflowOutcome::Failed { .. })),
                }
            }
        }
    }
}
