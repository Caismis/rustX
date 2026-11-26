//! The Scheduled Workflow Executor (Issue #606): scheduled preparation,
//! `claim_scheduled_start`, execution with deadline and panic containment,
//! failure-outcome mapping, settlement reconciliation and retry backoff all
//! live here, off `ConversationRuntime` — whose only responsibility is
//! exposing the `ForegroundSlot` and composing this executor at construction.
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock, Weak};

use futures_util::FutureExt;

use super::lifecycle::{
    QuiescenceSource, ScheduledWorkflowOutcome, ScheduledWorkflowOwnerRetirement,
    ScheduledWorkflowQuiescence, ScheduledWorkflowSettlement, ScheduledWorkflowStart,
    ScheduledWorkflowStartId, ScheduledWorkflowState, bounded_reason,
};
use super::{ScheduledWorkflowPreparation, ScheduledWorkflowPreparationError as PreparationError};
use crate::capabilities::CapabilityCoordinator;
use crate::runtime::auxiliary::executor::AuxiliaryExecutor;
use crate::runtime::conversation_runtime::activity::TaskActivity;
use crate::runtime::conversation_runtime::scheduled::{ScheduledClaim, ScheduledSnapshot};
use crate::runtime::conversation_runtime::{RuntimeInner, ScheduledCancellation};
use crate::runtime::inbound::ConversationInboundMailbox;
use crate::runtime::interaction::InteractionCoordinator;
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
/// handle already owned elsewhere; nothing here pins a runtime. The
/// runtime handle itself is bound later by [`ScheduledExecutor::bind_runtime`]
/// because the executor is composed before `RuntimeInner` exists.
pub(crate) struct ScheduledExecutorParts {
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
    pub(crate) spawner: tokio::runtime::Handle,
    #[cfg(test)]
    pub(crate) probe: Arc<Mutex<Option<crate::runtime::conversation_runtime::CoordinatorProbe>>>,
}

/// The scheduled-workflow owner: one instance per conversation, holding
/// only shared handles plus its own retry backoff bookkeeping.
/// `ConversationRuntime` keeps no scheduled business logic; the executor in
/// turn reaches coordinator state only through the runtime's narrow
/// reservation/Claim/finish interfaces (Issue #606).
pub(crate) struct ScheduledExecutor {
    /// The coordinator boundary: bound once after `RuntimeInner` is
    /// composed, held weakly so the executor never pins the runtime.
    runtime: OnceLock<Weak<RuntimeInner>>,
    /// Per-start rejection backoff bookkeeping for scheduled admission.
    retries: Mutex<BTreeMap<ScheduledWorkflowStartId, u64>>,
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
    spawner: tokio::runtime::Handle,
    #[cfg(test)]
    probe: Arc<Mutex<Option<crate::runtime::conversation_runtime::CoordinatorProbe>>>,
}

impl ScheduledExecutor {
    pub(crate) fn new(parts: ScheduledExecutorParts) -> Self {
        Self {
            runtime: OnceLock::new(),
            retries: Mutex::new(BTreeMap::new()),
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
            spawner: parts.spawner,
            #[cfg(test)]
            probe: parts.probe,
        }
    }

    /// Bind the runtime handle once `RuntimeInner` is composed. The executor
    /// is constructed before the runtime exists, so the handle arrives
    /// exactly once — and weakly, so it never pins the runtime.
    pub(crate) fn bind_runtime(&self, inner: &Arc<RuntimeInner>) {
        let _ = self.runtime.set(Arc::downgrade(inner));
    }

    /// The bound runtime, while it still exists.
    fn runtime(&self) -> Option<Arc<RuntimeInner>> {
        self.runtime.get().and_then(Weak::upgrade)
    }

    /// Contract preparation for a scheduled Workflow command: the frozen
    /// configuration generation comes from the coordinator's snapshot
    /// interface, and preparation itself runs off the coordinator lock.
    /// The later Claim critical section re-validates the same generation.
    pub(crate) fn prepare_workflow_command(
        &self,
        binding: &crate::scheduler::ScheduleBinding,
        workflow_id: &crate::runtime::workflow::WorkflowId,
        arguments: &serde_json::Value,
    ) -> Result<super::PreparedScheduledWorkflow, PreparationError> {
        let snapshot = self
            .runtime()
            .map(|runtime| runtime.scheduled_snapshot())
            .ok_or(PreparationError::UnpreparedResources)?;
        ScheduledWorkflowPreparation::new(binding.clone(), snapshot.resources, snapshot.model)
            .and_then(|preparation| {
                preparation
                    .with_approval_mode(snapshot.approval)
                    .prepare(workflow_id, arguments)
            })
    }

    /// Reconciliation never takes terminal publication from a registered owner.
    pub(crate) fn reconcile(
        &self,
        binding: &crate::scheduler::ScheduleBinding,
        id: &ScheduledWorkflowStartId,
        (after, limit): (u64, u32),
    ) -> Result<crate::scheduler::ScheduleOperationResult, crate::runtime::inbound::MailboxError>
    {
        let ownerless = move |recovered_writer: bool| {
            let retirement = ScheduledWorkflowOwnerRetirement {
                start_id: id.clone(),
                recovered_writer,
                agent_domain_quiescent: self
                    .subagents
                    .as_ref()
                    .is_some_and(|registry| !registry.owns_idle_work()),
                interactions_closed: !self.interaction.scheduled_scope_is_pending(id),
            };
            self.mailbox.schedule_operation(
                self.store.as_ref(),
                binding,
                &crate::scheduler::ScheduleOperation::WorkflowReconcileOwned {
                    id: id.clone(),
                    after,
                    limit,
                    retirement,
                },
                self.clock.as_ref(),
            )
        };
        // The owner probe and the ownerless `WorkflowReconcileOwned`
        // submission run under ONE coordinator critical section (Issue
        // #606 review): a queued workflow claiming its start between the
        // check and the durable write can no longer be retired to
        // `OutcomeUnknown` while its live owner still holds the slot.
        let (own, submitted) = self.runtime().map_or_else(
            || (None, Some(ownerless(true))),
            |runtime| runtime.reconcile_scheduled(id, || ownerless(true)),
        );
        if let Some(owner) = own {
            // The graph closing does not transfer its known outcome or Return.
            if owner.graph_finished {
                self.store.scheduled_workflow_recover_effects(
                    id,
                    after,
                    limit,
                    &ScheduledWorkflowOwnerRetirement {
                        start_id: id.clone(),
                        recovered_writer: owner.recovered_writer,
                        agent_domain_quiescent: self
                            .subagents
                            .as_ref()
                            .is_some_and(|registry| !registry.owns_idle_work()),
                        interactions_closed: !self.interaction.scheduled_scope_is_pending(id),
                    },
                )?;
                owner.settlement_wake.notify_one();
            }
            return Ok(crate::scheduler::ScheduleOperationResult::WorkflowRecovery(
                self.store.scheduled_workflow_recovery(id, after, limit)?,
            ));
        }
        submitted.expect("reconcile_scheduled returns one of owner probe or ownerless result")
    }

    /// Reserve a native foreground execution slot through the
    /// coordinator's atomic reservation interface, then run the start
    /// off-lock. The coordinator owns the gates, the generation freeze and
    /// the exact-owner slot; this executor owns preparation, execution,
    /// settlement confirmation and retry backoff.
    #[allow(clippy::too_many_lines)] // One foreground reservation and exact native retirement handoff.
    pub(crate) fn admit_next(self: &Arc<Self>) -> bool {
        let Some(runtime) = self.runtime() else {
            return false;
        };
        let now = self.monotonic_clock.now_millis();
        // Prune backoff bookkeeping for starts that left the pending set.
        if let Ok(pending) = self.store.scheduled_workflow_pending(256) {
            self.retries
                .lock()
                .expect("scheduled retries poisoned")
                .retain(|id, _| pending.iter().any(|head| &head.id == id));
        }
        let retries = &self.retries;
        let Some(reservation) = runtime.reserve_scheduled_start(|id| {
            retries
                .lock()
                .expect("scheduled retries poisoned")
                .get(id)
                .is_none_or(|at| *at <= now)
        }) else {
            return false;
        };
        let executor = self.clone();
        let activity = self.activity.lease();
        self.spawner.spawn(async move {
            let crate::runtime::conversation_runtime::scheduled::ScheduledReservation {
                start,
                recovered_writer,
                resources,
                model,
                approval,
                owner,
                cancellation,
                admission,
            } = reservation;
            // The long-lived executor only upgrades a Weak handle for the
            // admission decision; an ADMITTED task keeps a strong runtime
            // reference until execution, physical proof, durable settlement,
            // exact slot release and admission release are all done — a
            // dropped last external ConversationRuntime handle can no longer
            // strand a Claimed start (Issue #606 review).
            let result = if recovered_writer {
                let _lease = executor
                    .capability
                    .acquire_attempt_lease_for(resources.capability().clone());
                executor
                    .finish_scheduled_start(
                        &runtime,
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
                        runtime.clone(),
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
            runtime.finish_scheduled(&owner);
            drop(runtime);
            drop(admission);
            drop(activity);
        });
        true
    }

    /// `claim_scheduled_start` (Issue #606): the one critical section
    /// still runs on the coordinator — lifecycle and cancellation
    /// re-check, frozen-generation re-validation, reviewer invocation
    /// freeze, capability lease, and the durable Claim inside a Lifecycle
    /// Running Commit. A stale claim is refused for retry backoff, never
    /// replayed; backoff bookkeeping stays on this executor.
    #[allow(clippy::too_many_arguments)]
    fn claim_scheduled_start(
        &self,
        runtime: &Arc<RuntimeInner>,
        id: &ScheduledWorkflowStartId,
        contract: &super::PinnedScheduledWorkflow,
        resources: &Arc<RuntimeResourceSnapshot>,
        model: &crate::model::session::SessionModelConfig,
        approval: ApprovalMode,
        owner: &str,
        cancellation: &Arc<ScheduledCancellation>,
    ) -> Result<
        Option<(
            crate::runtime::workflow::scheduled::lifecycle::ClaimedScheduledWorkflow,
            crate::capabilities::AttemptCapabilityLease,
            crate::model::ResolvedModelInvocation,
        )>,
        String,
    > {
        let snapshot = ScheduledSnapshot {
            resources: resources.clone(),
            model: model.clone(),
            approval,
        };
        let lease_resources = resources.clone();
        match runtime.claim_scheduled_start(id, contract, &snapshot, owner, cancellation, || {
            self.capability
                .acquire_attempt_lease_for(lease_resources.capability().clone())
        })? {
            ScheduledClaim::Claimed {
                claim,
                lease,
                auxiliary_invocation,
            } => Ok(Some((*claim, lease, *auxiliary_invocation))),
            ScheduledClaim::Stale => {
                self.retries
                    .lock()
                    .expect("scheduled retries poisoned")
                    .insert(
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
                Ok(None)
            }
            ScheduledClaim::Refused => Ok(None),
        }
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    async fn run_scheduled_start(
        self: Arc<Self>,
        runtime: Arc<RuntimeInner>,
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
                if self.lifecycle.is_running() {
                    if matches!(
                        error,
                        PreparationError::DependencyUnavailable
                            | PreparationError::UnpreparedResources
                    ) {
                        self.retries
                            .lock()
                            .expect("scheduled retries poisoned")
                            .insert(
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
        #[cfg(test)]
        {
            // Test barrier: preparation finished — frozen inputs, capability
            // lease and the durable `Claimed` intent are all ready — but the
            // Claim critical section has not run. A waiter parked here proves
            // prep happens outside the coordinator lock (Issue #606).
            let prep_gate = self
                .probe
                .lock()
                .expect("coordinator probe lock poisoned")
                .as_ref()
                .and_then(|probe| probe.scheduled_prep_gate.clone());
            if let Some(gate) = prep_gate {
                tokio::task::block_in_place(|| gate.enter());
            }
        }
        let Some((claim, _resources_lease, auxiliary_invocation)) = self.claim_scheduled_start(
            &runtime,
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
        runtime.mark_scheduled_graph_finished(owner);
        #[cfg(test)]
        tokio::task::block_in_place(|| self.enter_probe_gate());
        self.finish_scheduled_start(&runtime, &id, outcome, output, false)
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
        runtime: &Arc<RuntimeInner>,
        id: &ScheduledWorkflowStartId,
        outcome: ScheduledWorkflowOutcome,
        output: Option<serde_json::Value>,
        recovered_writer: bool,
    ) -> Result<(), String> {
        // The graph is closed. A missing physical proof keeps this exact owner
        // and its resource lease accountable; neither cancellation nor elapsed
        // time permits a second executor or releases deletion/capacity. The
        // admitted task carries the strong runtime owner, so this probe cannot
        // lose the coordinator while the run is still accountable.
        let settlement_wake = runtime
            .probe_scheduled_owner(id)
            .ok_or_else(|| "scheduled terminal publication lost its native owner".to_owned())?
            .settlement_wake;
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
                    #[cfg(test)]
                    {
                        let awaiting_gate = self
                            .probe
                            .lock()
                            .expect("coordinator probe lock poisoned")
                            .as_ref()
                            .and_then(|probe| probe.scheduled_awaiting_proof_gate.clone());
                        if let Some(gate) = awaiting_gate {
                            tokio::task::block_in_place(|| gate.enter());
                        }
                    }
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
