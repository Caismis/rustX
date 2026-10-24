//! Attempt execution and settlement handoff, and explicit Attempt
//! cancellation.

use super::*;

/// A read-only receipt of the coordinator's existing execution handoff.
/// The ordinal is the already-admitted attempt count, not an observation cursor.
#[derive(Debug, Clone)]
pub(crate) struct NativeAttemptSettlement {
    pub ordinal: u64,
    pub attempt_id: AttemptId,
    pub outcome: NativeAttemptOutcome,
}

/// Finite classification borrowed from the authoritative Agent execution result.
#[derive(Debug, Clone, Copy)]
pub(crate) enum NativeAttemptOutcome {
    Completed,
    Cancelled,
    TimedOut,
    LimitExceeded,
    ModelFailed,
    RuntimeFailed,
}

impl NativeAttemptOutcome {
    pub(super) fn from_execution(outcome: &crate::events::types::AttemptOutcome) -> Self {
        use crate::events::types::{AttemptFailure, AttemptOutcome};
        match outcome {
            AttemptOutcome::Completed { .. } => Self::Completed,
            AttemptOutcome::Cancelled { .. } => Self::Cancelled,
            AttemptOutcome::TimedOut => Self::TimedOut,
            AttemptOutcome::LimitExceeded { .. } => Self::LimitExceeded,
            AttemptOutcome::Failed {
                error: AttemptFailure::Model { .. },
            } => Self::ModelFailed,
            AttemptOutcome::Failed {
                error: AttemptFailure::Runtime { .. },
            } => Self::RuntimeFailed,
        }
    }
}

/// A cancellation request failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CancelAttemptError {
    /// No attempt with the given identity is currently cancellable.
    NoCurrentAttempt,
    /// Storage or lifecycle refusal prevented an autonomous Goal attempt's
    /// pause operation. A superseded exact reference is an intentional no-op
    /// and successful cancellation, never this error.
    ///
    /// The attempt was still cancelled. Nothing pretends the Goal is paused:
    /// the durable phase is whatever storage actually holds, and a storage
    /// fault has additionally fenced further runtime admission.
    GoalPauseFailed {
        /// The durable or lifecycle diagnostic that refused the pause.
        diagnostic: String,
    },
}

impl RuntimeInner {
    /// Runs one attempt to settlement against the coordinator-owned
    /// cancellation trigger (the same handle `cancel_current_attempt`
    /// requests cancellation on).
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_lines)] // one ordered ownership transaction
    pub(super) async fn run_attempt(
        self: &Arc<Self>,
        attempt_id: AttemptId,
        conversation: ConversationState,
        fresh: Option<FreshInboundTurn>,
        cancellation: &AgentCancellation,
        model: AttemptModelSnapshot,
        model_timeout_policy: ModelTimeoutPolicy,
        tool_deadline_policy: crate::tools::deadline::ToolExecutionDeadlinePolicy,
        approval_mode: ApprovalMode,
        resources: Arc<RuntimeResourceSnapshot>,
        model_config: crate::model::session::SessionModelConfig,
        model_registry: Option<crate::model::invocation::ModelBindingRegistry>,
        lease: crate::capabilities::AttemptCapabilityLease,
    ) -> crate::agent::AgentExecutionResult {
        let observer = RuntimeObserver::new(self);
        let monotonic_clock = Arc::clone(&self.monotonic_clock);
        // The context runtime is derived from the frozen snapshot, so the
        // attempt's window, output budget, and summary invocation all agree
        // with the model it was admitted with.
        let context_runtime = self
            .context_runtime_with_assembly(
                &model,
                resources.context_assembly().clone(),
                model_timeout_policy,
                &resources,
                Arc::new(self.auxiliary.clone()),
            )
            .expect("admission validated this model against the session context policy");
        let context_runtime = context_runtime.with_runtime_resources(&resources);
        let execution_policy = crate::agent::execution::AgentExecutionRuntimePolicy {
            model_timeout_policy,
            tool_deadline_policy,
            monotonic_clock,
            // The attempt-scoped subagent seam. It is derived from the very
            // generation and model configuration this attempt was admitted
            // with, under the same admission linearization, so a subagent
            // invoked by this attempt resolves exactly that generation — not
            // whatever generation happens to be runtime-current when the
            // model issues the call.
            // A runtime whose model authority is frozen (a subagent child)
            // owns neither a subagent registry nor a catalog to resolve a
            // named definition's explicit model against, so the seam is
            // absent on both counts rather than half-present.
            subagent_context: self
                .subagents
                .is_some()
                .then_some(model_registry)
                .flatten()
                .map(|models| {
                    crate::runtime::subagent::NativeExecutionContext::new(
                        self.capability.clone(),
                        attempt_id.clone(),
                        Arc::clone(&resources),
                        model_config,
                        models,
                        approval_mode,
                        crate::runtime::subagent::InheritedExecutionPolicy {
                            model_timeout: model_timeout_policy,
                            tool_deadline: tool_deadline_policy,
                            context: resources
                                .configuration()
                                .map_or(self.context.policy, |generation| {
                                    generation.config.context_policy()
                                }),
                        },
                    )
                    .with_workflow_models(self.auxiliary.clone())
                }),
            workflow_output: self.workflow_output.clone(),
        };
        let request = AgentExecutionRequest {
            agent_id: self.agent_id.clone(),
            conversation_id: self.conversation_id.clone(),
            attempt_id,
            conversation,
            initial_turn_trigger: match fresh {
                Some(fresh) => InitialTurnTrigger::FreshInbound(fresh),
                None => InitialTurnTrigger::Continuation,
            },
            model,
        };
        let lifecycle = crate::agent::AttemptLifecycle::inert()
            .with_native_interaction(self.interaction.clone())
            .with_approval_mode(approval_mode);
        #[cfg(test)]
        let lifecycle = {
            let test_policy = self
                .test_pre_tool_policy
                .lock()
                .expect("test pre-tool policy lock")
                .take();
            match test_policy {
                Some(policy) => lifecycle.with_pre_tool_policy(policy),
                None => lifecycle,
            }
        };
        let mut execution = AgentExecution::new(
            request,
            lease,
            cancellation,
            execution_policy,
            context_runtime,
            &self.tool_runtime,
            lifecycle,
        )
        // Neither rejection is reachable: `conversation_id` *is* the tool
        // runtime's own identity (the runtime has no independent
        // conversation authority to disagree with it), and construction
        // validated the coordinator against that same runtime.
        .expect("the conversation runtime derives its identity from this tool runtime");
        // Test-only: hand the armed M9b start-boundary pause to exactly this
        // attempt's execution.
        #[cfg(test)]
        {
            let pause = self
                .probe
                .lock()
                .expect("coordinator probe lock poisoned")
                .as_mut()
                .and_then(|probe| probe.start_boundary_pause.take());
            if let Some(pause) = pause {
                execution.install_start_boundary_pause(pause);
            }
            if let Some(pause) = self.take_model_arbitration_pause() {
                execution.install_model_arbitration_pause(pause);
            }
            let pause = self
                .probe
                .lock()
                .expect("coordinator probe lock poisoned")
                .as_mut()
                .and_then(|probe| probe.tool_start_pause.take());
            if let Some(pause) = pause {
                execution.install_tool_start_pause(pause);
            }
        }
        execution.observe(&observer);
        execution.run().await
    }

    /// The settlement path of one attempt: transfer the authoritative
    /// conversation state back to the coordinator, clear the current-attempt
    /// slot, then hand off to the next admission when the mailbox holds
    /// pending work.
    ///
    /// The result is consumed by value: its `conversation` becomes the
    /// coordinator's authoritative conversation state again.
    #[allow(clippy::needless_pass_by_value)]
    pub(super) fn finish_attempt(
        self: &Arc<Self>,
        attempt_id: AttemptId,
        result: crate::agent::AgentExecutionResult,
    ) {
        #[cfg(test)]
        let restore_gate = self
            .probe
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|probe| probe.before_restore_gate.clone());
        #[cfg(test)]
        if let Some(gate) = restore_gate {
            gate.enter();
        }
        {
            let mut state = self.lock_state();
            // An active-attempt durable canonical-write failure means the
            // durable authority rejected a required commit while the
            // conversation's single mutable state was checked out: the
            // runtime must not silently return to a false `Healthy` state
            // and admit future work as though storage were fine (Issue
            // #63). This is an already-terminal durable failure, so it
            // enters the explicit `DurabilityFailed` state immediately;
            // the settled attempt's conversation state is still restored
            // (its in-memory content stayed consistent with the durable
            // Ledger: the failed commit installed nothing).
            let durable_failure = result.durable_failure.clone();
            state.latest_settlement = Some(NativeAttemptSettlement {
                ordinal: state.admitted_attempts,
                attempt_id: attempt_id.clone(),
                outcome: NativeAttemptOutcome::from_execution(&result.outcome),
            });
            if let Some(fanout) = self.pending.get() {
                fanout.child_settled();
            }
            state.conversation = Some(result.conversation);
            if let Some(diagnostic) = durable_failure {
                let operation = match result.durable_failure_kind {
                    Some(crate::agent::DurableFailureKind::Compaction) => {
                        DurableOperation::CanonicalCommit
                    }
                    Some(crate::agent::DurableFailureKind::RequestStart) => {
                        DurableOperation::RequestStart
                    }
                    Some(crate::agent::DurableFailureKind::EventJournal) => {
                        DurableOperation::EventJournal
                    }
                    _ => DurableOperation::CanonicalCommit,
                };
                self.record_durability_failure(&mut state, operation, diagnostic);
            }
            if state
                .current_attempt
                .as_ref()
                .is_some_and(|current| current.attempt_id == attempt_id)
            {
                state.current_attempt = None;
            }
            self.settlement.notify_waiters();
            // Test-only gate: the conversation state is restored and the
            // current-attempt slot is cleared, but the next-admission
            // handoff has not run yet. An enqueue during this park
            // deterministically races the settlement boundary. The gate
            // parks only when armed and disarms after one park. The gate
            // handle is extracted before the park so the probe mutex is not
            // held while parked.
            #[cfg(test)]
            let settlement_gate = self
                .probe
                .lock()
                .expect("coordinator probe lock poisoned")
                .as_ref()
                .and_then(|probe| probe.settlement_gate.clone());
            #[cfg(test)]
            if let Some(gate) = settlement_gate {
                gate.enter();
            }
        }
        // Test-only gate: the coordinator lock is released and the
        // current-attempt slot is already clear, but this task has not run
        // its final admission callback and has not returned, so it still
        // holds the attempt-task admission. A drain that observed the empty
        // slot must not be able to publish quiescence here.
        #[cfg(test)]
        let attempt_exit_gate = self
            .probe
            .lock()
            .expect("coordinator probe lock poisoned")
            .as_ref()
            .and_then(|probe| probe.attempt_exit_gate.clone());
        #[cfg(test)]
        if let Some(gate) = attempt_exit_gate {
            gate.enter();
        }
        self.admit_next_attempt();
    }
}

impl ConversationRuntime {
    /// Requests cancellation of one named current attempt, durably pausing
    /// the Goal when that attempt is an autonomous Goal continuation.
    ///
    /// Acceptance is not terminal settlement: actual settlement remains
    /// owned by the Agent Loop and is observed asynchronously. The
    /// attempt-id check under the coordinator lock guarantees that the
    /// cancellation signal is never delivered to a *different* attempt that
    /// was admitted after the named one settled.
    ///
    /// # Ordering (Issue #351)
    ///
    /// Under the one coordinator lock, which also owns Goal-round admission
    /// and the Goal control commit boundary:
    ///
    /// 1. prove the named attempt is the current attempt and read its
    ///    runtime-owned [`AttemptProvenance`];
    /// 2. if — and only if — it is an autonomous Goal continuation, durably
    ///    commit `Active -> Paused` only while its admitted `GoalRef` is current;
    /// 3. request cancellation of that same attempt;
    /// 4. report success once the matching pause (if required) and cancellation
    ///    have won. Stale authority requires no pause and still succeeds.
    ///
    /// Because the durable pause commits **before** cancellation is
    /// requested, the matching authority cannot immediately restart. A newer
    /// Goal revision is left unchanged: the old attempt has no pause authority
    /// over later user intent. Settlement clears the named attempt under this
    /// same lock; a later interrupt returns `NoCurrentAttempt`.
    /// Because provenance decides step 2, cancelling an ordinary Human
    /// attempt never pauses an Active Goal merely because one exists — not
    /// even a Human attempt during which the model called `create_goal`.
    ///
    /// # Errors
    ///
    /// Returns [`CancelAttemptError::NoCurrentAttempt`] when no attempt with
    /// the given identity is currently cancellable, and
    /// [`CancelAttemptError::GoalPauseFailed`] when storage or lifecycle refusal
    /// prevents the pause operation. A stale/superseded `GoalRef` returning
    /// `Ok(None)` is success: no newer authority is substituted or retried.
    /// The failure case never fabricates a pause: cancellation of
    /// that exact attempt is still requested (containment), and a genuine
    /// storage failure is recorded through the runtime's own absorbing
    /// durability authority, which fences all further admission — so no later
    /// Goal round can be admitted even though the phase still reads `Active`.
    pub fn cancel_current_attempt(
        &self,
        attempt_id: &AttemptId,
    ) -> Result<AttemptId, CancelAttemptError> {
        let mut state = self.inner.lock_state();
        if state
            .current_attempt
            .as_ref()
            .is_none_or(|current| current.attempt_id != *attempt_id)
        {
            return Err(CancelAttemptError::NoCurrentAttempt);
        }
        self.cancel_attempt_locked(&mut state)
    }

    /// Selects and cancels the native current attempt under one coordinator lock.
    ///
    /// This is the current-attempt control operation used by clients. It does
    /// not obtain an execution identity from an observation. Selection, the
    /// exact admitted Goal pause (when required), and cancellation are one
    /// critical section shared with settlement and successor admission. The
    /// returned identity belongs to that operation even if settlement and a
    /// successor admission precede delivery of the acceptance receipt.
    /// Acceptance does not claim terminal settlement.
    ///
    /// # Errors
    /// Returns [`CancelAttemptError::NoCurrentAttempt`] if no native attempt is
    /// current, or the same [`CancelAttemptError::GoalPauseFailed`] containment
    /// result as [`Self::cancel_current_attempt`].
    pub fn cancel_active_attempt(&self) -> Result<AttemptId, CancelAttemptError> {
        let mut state = self.inner.lock_state();
        self.cancel_attempt_locked(&mut state)
    }

    pub(super) fn cancel_attempt_locked(
        &self,
        state: &mut CoordinatorState,
    ) -> Result<AttemptId, CancelAttemptError> {
        let current = state
            .current_attempt
            .as_ref()
            .ok_or(CancelAttemptError::NoCurrentAttempt)?;
        let attempt_id = current.attempt_id.clone();
        let cancellation = current.cancellation.clone();
        let pause = match &current.provenance {
            AttemptProvenance::GoalContinuation(expected)
            | AttemptProvenance::RecoveredGoalContinuation(expected) => {
                // Composition gates future automation, not interrupt intent
                // for already-admitted work. Recovery can retain exact Goal
                // authority even when the reopened profile disables Goal.
                self.inner.tool_runtime.goal().map(|domain| {
                    self.inner
                        .mailbox
                        .with_running_commit(|| domain.pause_if_current(expected))
                })
            }
            _ => None,
        };
        let failure = match pause {
            None | Some(Ok(Ok(_))) => None,
            // A genuine durable failure: fail closed through the existing
            // absorbing runtime durability authority.
            Some(Ok(Err(error))) => {
                let diagnostic = error.to_string();
                self.inner.record_durability_failure(
                    state,
                    DurableOperation::GoalPause,
                    diagnostic.clone(),
                );
                Some(diagnostic)
            }
            // The runtime no longer admits semantic commits, so it can no
            // longer admit a Goal round either. This is a lifecycle refusal,
            // not a storage fault, and earns no durability fact.
            Some(Err(error)) => Some(error.to_string()),
        };
        // Containment is unconditional: the interrupted attempt is cancelled
        // whether or not the durable pause committed.
        let _ = cancellation.request_cancel(CancellationReason::UserRequested);
        match failure {
            None => Ok(attempt_id),
            Some(diagnostic) => Err(CancelAttemptError::GoalPauseFailed { diagnostic }),
        }
    }

    /// Commits a one-shot child cancellation intent into the runtime-owned
    /// cancellation state (Issue #60 child side).
    ///
    /// This is the subagent child control plane's `ParentFrame::Cancel`
    /// sink: the cancellation never waits for an observation to be
    /// delivered. Under the one coordinator lock that also owns attempt
    /// admission:
    ///
    /// - a current attempt exists ⇒ its [`AgentCancellation`] receives the
    ///   cancellation immediately, through the same M9b model-turn start
    ///   gate as every other cancellation request;
    /// - no attempt exists yet ⇒ the intent is retained as the coordinator's
    ///   one-shot sticky cancellation and the next admission consumes it, so
    ///   the admitted attempt starts already-cancelled and its first
    ///   model-turn arbitration resolves `CancelledBeforeStart`.
    ///
    /// The intent is sticky but one-shot: the child is a one-shot runtime,
    /// so at most one admission ever consumes it. The child control plane
    /// is its only producer; a parent conversation never arms it.
    ///
    /// Returns the cancelled current attempt identity, or `None` when the
    /// intent was retained for the next admission.
    pub(crate) fn cancel_current_or_next_attempt(
        &self,
        reason: CancellationReason,
    ) -> Option<AttemptId> {
        let mut state = self.inner.lock_state();
        if let Some(current) = &state.current_attempt {
            let _ = current.cancellation.request_cancel(reason);
            return Some(current.attempt_id.clone());
        }
        state.one_shot_cancel = Some(reason);
        None
    }
}

#[cfg(test)]
mod tests;
