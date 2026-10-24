//! Conversation-owned scheduled execution: no synthetic `Attempt` or `ToolCall`.
use super::{
    ApprovalMode, Arc, CancellationReason, ConversationRuntime, Mutex, RuntimeInner,
    RuntimeResourceSnapshot, SessionModelConfig,
};
use crate::runtime::workflow::scheduled::{
    ScheduledWorkflowPreparation, ScheduledWorkflowPreparationError as PreparationError,
    lifecycle::{
        QuiescenceSource, ScheduledWorkflowOutcome, ScheduledWorkflowOwnerRetirement,
        ScheduledWorkflowQuiescence, ScheduledWorkflowSettlement, ScheduledWorkflowStart,
        ScheduledWorkflowStartId, ScheduledWorkflowState, bounded_reason,
    },
};
use crate::runtime::workflow::{WorkflowRunError, WorkflowRuntime};
use futures_util::FutureExt;

pub(super) struct CurrentScheduled {
    pub id: ScheduledWorkflowStartId,
    pub owner: String,
    pub cancellation: Arc<ScheduledCancellation>,
    pub graph_finished: bool,
    pub recovered_writer: bool,
    pub settlement_wake: Arc<tokio::sync::Notify>,
}
pub(super) struct ScheduledCancellation {
    signal: crate::runtime::cancellation::CancellationSignal,
    reason: Mutex<CancellationReason>,
}
impl ScheduledCancellation {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            signal: crate::runtime::cancellation::CancellationSignal::default(),
            reason: Mutex::new(CancellationReason::UserRequested),
        })
    }
    pub fn cancel(&self, reason: CancellationReason) {
        let mut held = self.reason.lock().expect("scheduled cancellation");
        if !self.signal.is_cancelled() {
            *held = reason;
            self.signal.cancel();
        }
    }
    fn execution(self: &Arc<Self>) -> crate::runtime::cancellation::ExecutionCancellation {
        crate::runtime::cancellation::ExecutionCancellation::new(self.signal.clone(), self.clone())
    }
}
impl crate::runtime::cancellation::CancellationCause for ScheduledCancellation {
    fn cause(&self) -> CancellationReason {
        *self.reason.lock().expect("scheduled cancellation")
    }
}

impl ConversationRuntime {
    /// A wake handle carries no runtime ownership or execution authority.
    pub(crate) fn scheduled_wake(&self) -> Arc<tokio::sync::Notify> {
        self.inner.mailbox.wake()
    }
}

impl RuntimeInner {
    /// Reconciliation never takes terminal publication from a registered owner.
    pub(super) fn reconcile_scheduled(
        &self,
        state: &super::CoordinatorState,
        binding: &crate::scheduler::ScheduleBinding,
        id: &ScheduledWorkflowStartId,
        (after, limit): (u64, u32),
        clock: &dyn crate::runtime::types::RuntimeClock,
    ) -> Result<crate::scheduler::ScheduleOperationResult, crate::runtime::inbound::MailboxError>
    {
        let own = state.scheduled.as_ref().filter(|owner| &owner.id == id);
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
    pub(super) fn admit_next_scheduled(self: &Arc<Self>) -> bool {
        let mut state = self.lock_state();
        if !self.lifecycle.is_running()
            || state.current_attempt.is_some()
            || state.scheduled.is_some()
            || state.manual_compaction.is_some()
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
        let resources = state.resources.clone();
        let model = state.model.config().clone();
        let approval = state.effective_approval_mode;
        let owner = format!("scheduled:{}", uuid::Uuid::now_v7());
        let cancellation = ScheduledCancellation::new();
        let recovered_writer = matches!(next.state, ScheduledWorkflowState::Claimed { .. });
        state.scheduled = Some(CurrentScheduled {
            id: next.id.clone(),
            owner: owner.clone(),
            cancellation: cancellation.clone(),
            graph_finished: recovered_writer,
            recovered_writer,
            settlement_wake: Arc::default(),
        });
        let admission = self
            .lifecycle
            .try_enter_running()
            .expect("native scheduled admission owns coordinator fence");
        drop(state);
        let inner = self.clone();
        self.executor.spawn(async move {
            let result = if recovered_writer {
                let _lease = inner
                    .capability
                    .acquire_attempt_lease_for(resources.capability().clone());
                inner
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
                inner
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
            let mut state = inner.lock_state();
            if state
                .scheduled
                .as_ref()
                .is_some_and(|current| current.owner == owner)
            {
                state.scheduled = None;
            }
            drop(state);
            drop(admission);
            inner.settlement.notify_waiters();
            inner.mailbox.wake().notify_one();
        });
        true
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    async fn run_scheduled_start(
        self: Arc<Self>,
        start: ScheduledWorkflowStart,
        resources: Arc<RuntimeResourceSnapshot>,
        model: SessionModelConfig,
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
                let mut state = self.lock_state();
                if self.lifecycle.is_running() {
                    if matches!(
                        error,
                        PreparationError::DependencyUnavailable
                            | PreparationError::UnpreparedResources
                    ) {
                        state.scheduled_retries.insert(
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
        let (claim, _resources_lease) = {
            let mut state = self.lock_state();
            if !self.lifecycle.is_running() || cancellation.signal.is_cancelled() {
                return Ok(());
            }
            if !Arc::ptr_eq(&resources, &state.resources)
                || state.model.config() != &model
                || state.effective_approval_mode != approval
            {
                state.scheduled_retries.insert(
                    id.clone(),
                    self.monotonic_clock.now_millis().saturating_add(1000),
                );
                self.store
                    .scheduled_workflow_wait(
                        &id,
                        "native configuration changed before Claim",
                        self.clock.now() + chrono::Duration::seconds(1),
                    )
                    .map_err(|e| e.to_string())?;
                return Ok(());
            }
            let lease = self
                .capability
                .acquire_attempt_lease_for(resources.capability().clone());
            let claim = self
                .lifecycle
                .with_running_commit(|| {
                    self.store.scheduled_workflow_claim(
                        &id,
                        prepared.contract(),
                        owner,
                        self.clock.now(),
                    )
                })
                .map_err(|e| format!("scheduled Claim lifecycle: {e:?}"))?
                .map_err(|e| e.to_string())?;
            let Some(claim) = claim else { return Ok(()) };
            (claim, lease)
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
            .with_workflow_models(self.auxiliary.clone())
            .with_workflow_deadline(self.monotonic_clock.clone(), deadline, trigger.clone());
        context.native = Some(Arc::new(
            crate::tools::invocation::NativeInvocationServices {
                lifecycle: crate::agent::AttemptLifecycle::inert()
                    .with_native_interaction(self.interaction.clone())
                    .with_approval_mode(approval),
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
            Err(WorkflowRunError::Cancelled(_)) => (ScheduledWorkflowOutcome::Cancelled, None),
            Err(WorkflowRunError::Deadline(_)) => (ScheduledWorkflowOutcome::TimedOut, None),
            Err(error) => (
                ScheduledWorkflowOutcome::Failed {
                    reason: bounded_reason(error.to_string()),
                },
                None,
            ),
        };
        cancellation.cancel(CancellationReason::ParentCancelled);
        {
            let mut state = self.lock_state();
            if let Some(current) = state
                .scheduled
                .as_mut()
                .filter(|current| current.owner == owner)
            {
                current.graph_finished = true;
            }
        }
        #[cfg(test)]
        tokio::task::block_in_place(|| {
            self.enter_probe_gate(|probe| probe.scheduled_settlement_gate.clone());
        });
        self.finish_scheduled_start(&id, outcome, output, false)
            .await
    }

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
            .lock_state()
            .scheduled
            .as_ref()
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
