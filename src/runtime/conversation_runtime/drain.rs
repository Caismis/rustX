//! Lifecycle drain: the `Running -> Draining` linearization, supervision of
//! every owner to its native terminal boundary, and quiescence publication.

use super::*;

/// The one shared completion of the runtime drain. Every concurrent shutdown
/// caller waits on this same object; no caller creates a competing drain
/// state machine.
#[derive(Debug, Default)]
pub(super) struct DrainCompletion {
    pub(super) completed: AtomicBool,
    pub(super) result: Mutex<Option<NativeDrainOutcome>>,
    pub(super) notify: tokio::sync::Notify,
}

/// Native physical supervision and logical/durable quiescence are distinct
/// facts. A publication failure may keep the runtime Draining after all native
/// owners have positively settled. The child's durable physical receipt must
/// not lose that proof merely because logical publication failed.
#[derive(Debug, Clone)]
pub(crate) struct NativeDrainOutcome {
    pub(crate) logical: Result<(), ShutdownError>,
    pub(crate) physical: Result<(), Vec<String>>,
}

impl NativeDrainOutcome {
    pub(super) fn failed(
        failures: &std::collections::BTreeSet<String>,
        physical_failures: std::collections::BTreeSet<String>,
    ) -> Self {
        Self {
            logical: Err(ShutdownError::RuntimeOwnedSettlement {
                detail: aggregate_settlement_failures(failures),
            }),
            // Call only after every native supervision boundary. Durable
            // diagnostics cannot erase that positive physical proof.
            physical: if physical_failures.is_empty() {
                Ok(())
            } else {
                Err(physical_failures.into_iter().collect())
            },
        }
    }
}

/// The narrow set of operations that can start the one runtime drain.
///
/// An MCP settlement failure is a drain trigger rather than a separate
/// pre-drain mutation: its diagnostic latch and lifecycle transition are
/// published by the same coordinator critical section.
pub(super) enum DrainTrigger {
    /// Explicit runtime shutdown.
    RuntimeShutdown,
    /// The parent transport died; interaction requests have no outcome owner.
    ParentLost,
    /// Child composition is being rolled back before semantic activation.
    ChildPreparationRollback,
    /// An authoritative MCP physical-settlement failure.
    McpSettlementFailure(String),
}

impl DrainCompletion {
    pub(super) fn complete(&self, result: NativeDrainOutcome) {
        *self.result.lock().expect("drain completion lock poisoned") = Some(result);
        self.completed.store(true, Ordering::Release);
        self.notify.notify_waiters();
    }

    pub(super) async fn wait_outcome(&self) -> NativeDrainOutcome {
        loop {
            if self.completed.load(Ordering::Acquire) {
                return self
                    .result
                    .lock()
                    .expect("drain completion lock poisoned")
                    .clone()
                    .expect("completed drain has a result");
            }
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.completed.load(Ordering::Acquire) {
                return self
                    .result
                    .lock()
                    .expect("drain completion lock poisoned")
                    .clone()
                    .expect("completed drain has a result");
            }
            notified.await;
        }
    }
}

/// Renders the collected settlement failures as one bounded deterministic
/// diagnostic.
///
/// The set is already in deterministic identity order, so the same
/// interleaving always yields the same diagnostic. This is a diagnostic
/// aggregation, not an error framework: shutdown has exactly one failure
/// variant and it carries exactly one bounded string.
pub(super) fn aggregate_settlement_failures(
    failures: &std::collections::BTreeSet<String>,
) -> String {
    if failures.len() == 1 {
        return failures
            .iter()
            .next()
            .expect("a single-element set has one element")
            .clone();
    }
    format!(
        "{} runtime-owned settlement failures: {}",
        failures.len(),
        failures.iter().cloned().collect::<Vec<_>>().join("; ")
    )
}

/// A runtime shutdown failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShutdownError {
    /// The runtime has not been activated: an inert conversation has no
    /// runtime lifecycle to end, so the request is refused and nothing is
    /// published.
    Inactive,
    /// Supervision reached every settleable owner's native terminal boundary,
    /// but at least one required ownership/physical/durable terminal
    /// condition stayed unproven, so the lifecycle remains `Draining` and
    /// successful quiescence is not claimed.
    ///
    /// The detail is a bounded deterministic aggregation of every collected
    /// failure, in identity order. It is never returned while a known
    /// runtime-owned operation is merely in flight and has not been awaited.
    RuntimeOwnedSettlement {
        /// The owner-provided settlement diagnostic.
        detail: String,
    },
}

impl RuntimeInner {
    /// Fences ordinary runtime admission when a retired MCP generation proves
    /// that its physical terminal state is unproven. The logical resource
    /// publication already committed remains current; the existing drain
    /// lifecycle carries the failure to final settlement reporting. The
    /// failure latch and lifecycle admission closure are one coordinator
    /// linearization point.
    pub(super) fn fence_mcp_settlement_failure(self: &Arc<Self>, detail: String) {
        let _ = self.begin_drain_internal(DrainTrigger::McpSettlementFailure(detail));
    }

    pub(super) fn begin_drain(self: &Arc<Self>) -> Result<Arc<DrainCompletion>, ShutdownError> {
        self.begin_drain_internal(DrainTrigger::RuntimeShutdown)
    }

    /// Starts or joins the shared drain. For an MCP settlement failure, this
    /// operation owns the complete failure transition: it publishes the
    /// persistent diagnostic latch, arbitrates current-attempt cancellation,
    /// and closes lifecycle admission before releasing the coordinator lock.
    #[allow(clippy::too_many_lines)]
    pub(super) fn begin_drain_internal(
        self: &Arc<Self>,
        trigger: DrainTrigger,
    ) -> Result<Arc<DrainCompletion>, ShutdownError> {
        let mut first = false;
        let mcp_failure = matches!(&trigger, DrainTrigger::McpSettlementFailure(_));
        let parent_lost = matches!(&trigger, DrainTrigger::ParentLost);
        let preparation_rollback = matches!(&trigger, DrainTrigger::ChildPreparationRollback);
        // Runtime shutdown is only a cancellation contender. The active
        // attempt's AgentCancellation remains the one cause authority, so
        // every runtime-driven interaction settlement must use the winner it
        // records rather than blindly relabeling the work as RuntimeShutdown.
        let mut interaction_cancel_reason = CancellationReason::RuntimeShutdown;
        {
            let mut state = self.lock_state();
            let lifecycle_state = self.lifecycle.state();
            assert!(
                !(mcp_failure && lifecycle_state == ConversationLifecycleState::Quiescent),
                "an MCP physical-settlement failure arrived after Quiescent; the retirement ownership invariant is broken"
            );
            if let DrainTrigger::McpSettlementFailure(detail) = trigger
                && state.mcp_settlement_failure.is_none()
            {
                state.mcp_settlement_failure = Some(detail);
            }
            match lifecycle_state {
                ConversationLifecycleState::Inactive => {
                    if !mcp_failure && !preparation_rollback {
                        return Err(ShutdownError::Inactive);
                    }
                    let transitioned = self.lifecycle.begin_inactive_drain();
                    debug_assert!(
                        transitioned,
                        "the coordinator lock owns the inactive drain transition"
                    );
                    first = true;
                }
                ConversationLifecycleState::Running => {
                    if parent_lost {
                        self.interaction.abandon_after_parent_loss();
                    }
                    if let Some(current) = &state.current_attempt {
                        // Take the same M9b model-turn start gate as user
                        // cancellation *before* publishing `Draining`. If a
                        // start critical section already owns the gate, it
                        // linearizes before runtime drain and is allowed to
                        // settle; if this request wins the gate, no later
                        // model arbitration can start a request.
                        let _ = current
                            .cancellation
                            .request_cancel(CancellationReason::RuntimeShutdown);
                        interaction_cancel_reason = current.cancellation.reason();
                    }
                    if let Some(scheduled) = &state.scheduled {
                        scheduled
                            .cancellation
                            .cancel(CancellationReason::RuntimeShutdown);
                    }
                    // Issue #351: runtime shutdown is not Goal pause. Drain
                    // closes runtime admission and supervises runtime-owned
                    // work; the durable `GoalPhase` is untouched, so a later
                    // explicit reopen resumes it through ordinary admission.
                    if let Some(compaction) = &state.manual_compaction {
                        let _ = compaction
                            .cancellation
                            .request_cancel(CancellationReason::RuntimeShutdown);
                    }
                    let transitioned = self.lifecycle.begin_drain();
                    debug_assert!(
                        transitioned,
                        "the coordinator lock owns the runtime drain transition"
                    );
                    self.auxiliary.begin_drain();
                    first = true;
                    self.observe(ConversationObservation::Shutdown);
                    #[cfg(test)]
                    if let Some(linearized) = self
                        .probe
                        .lock()
                        .expect("coordinator probe lock poisoned")
                        .as_ref()
                        .and_then(|probe| probe.drain_linearization.clone())
                    {
                        linearized.notify_one();
                    }
                    self.wake.close();
                }
                ConversationLifecycleState::Draining => {
                    // An idle claim has already closed admission without starting
                    // external teardown under the manager registry lock.
                    first = !self.drain_started.load(Ordering::Acquire);
                    if first {
                        self.observe(ConversationObservation::Shutdown);
                    }
                    self.wake.close();
                }
                ConversationLifecycleState::Quiescent => {}
            }
            if first {
                // Reserve the existing supervisor under the same coordinator
                // boundary. A second shutdown cannot repeat external teardown.
                first = !self.drain_started.swap(true, Ordering::AcqRel);
            }
            // This test-only park is deliberately inside the coordinator
            // critical section. It proves that an MCP failure cannot expose
            // its latch and only later close lifecycle admission.
            #[cfg(test)]
            let mcp_failure_drain_gate = self
                .probe
                .lock()
                .expect("coordinator probe lock poisoned")
                .as_ref()
                .and_then(|probe| probe.mcp_failure_drain_gate.clone());
            #[cfg(test)]
            if mcp_failure
                && first
                && let Some(gate) = mcp_failure_drain_gate
            {
                gate.enter();
            }
        }

        let completion = self
            .drain
            .get_or_init(|| Arc::new(DrainCompletion::default()))
            .clone();
        if first {
            // Interaction publication is admitted through the same lifecycle
            // commit boundary as `Running -> Draining`, so this scan sees
            // every interaction that won the admission race. The pending
            // map may become empty before its waiter consumes the terminal
            // payload; the retained lifecycle guard keeps quiescence behind
            // that callback-authority settlement.
            // Cancellation intent is requested synchronously after the drain
            // transition wins, so a client-side background cancel cannot
            // create an unchecked post-drain ownership window. The registry
            // still owns the physical/native settlement and is awaited by the
            // drain task below.
            for execution in self.tool_runtime.background().active_snapshot() {
                self.tool_runtime.background().cancel_with_reason(
                    &execution.execution_id,
                    CancellationReason::RuntimeShutdown,
                );
            }
            self.tool_runtime.background().abort_prepared_for_drain();
            // Same containment for the subagent plane (Issue #60):
            // cancellation intent is committed synchronously; the registry
            // and its driver tasks own escalation, reap, and terminal
            // settlement, awaited by the drain task below.
            if let Some(subagents) = &self.subagents {
                subagents.cancel_all(CancellationReason::RuntimeShutdown);
            }
            // In-flight capability preparation owns real MCP processes. The
            // *owner* is cancelled here (never the caller's future), so each
            // one drives its physical process to settlement before releasing
            // the counted admission the drain below waits on.
            self.capability.cancel_conversation_preparation();
            // The published MCP generation is also retired at the drain
            // transition. Its explicit attempt/background leases, if any,
            // keep the physical runtime alive until those owners settle, but
            // the generation itself no longer blocks drain indefinitely.
            self.capability.retire_current_mcp_runtimes();
            let inner = Arc::clone(self);
            let completion_for_task = completion.clone();
            self.executor.spawn(async move {
                let result = inner
                    .drain_to_quiescence(completion_for_task.clone(), interaction_cancel_reason)
                    .await;
                // Release the drain task's native ownership before waking a
                // shutdown caller that may immediately relinquish the product.
                drop(inner);
                completion_for_task.complete(result);
            });
        }
        Ok(completion)
    }

    /// Waits for the current Agent Execution or manual compaction to return
    /// the checked-out conversation state. Cancellation alone never clears an
    /// ownership slot.
    pub(super) async fn wait_for_foreground_operation(&self) {
        loop {
            let idle = {
                let state = self.lock_state();
                state.current_attempt.is_none()
                    && state.scheduled.is_none()
                    && state.manual_compaction.is_none()
            };
            if idle {
                return;
            }
            let notified = self.settlement.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let idle = {
                let state = self.lock_state();
                state.current_attempt.is_none()
                    && state.scheduled.is_none()
                    && state.manual_compaction.is_none()
            };
            if idle {
                return;
            }
            // The waiter is registered and the slot is still occupied: the
            // caller is now committed to awaiting this attempt's settlement.
            #[cfg(test)]
            self.signal_drain_supervision();
            notified.await;
        }
    }

    pub(super) async fn reap_manual_tail(self: &Arc<Self>, wait: bool) {
        let tail = {
            let mut owner = self.manual_tail.lock().expect("manual tail owner");
            if owner.as_ref().is_some_and(|tail| {
                wait || tail.completion.ready_to_join.load(Ordering::Acquire)
                    || tail.worker.is_finished()
            }) {
                owner.take()
            } else {
                None
            }
        };
        let Some(tail) = tail else {
            return;
        };
        if let Err(error) = tail.worker.await {
            let diagnostic = format!("native manual Compaction task failed: {error}");
            self.commit_failure_and_observe(DurableOperation::CanonicalCommit, diagnostic.clone());
            // A panicking business future has dropped its parent scopes. The
            // native Call registry still owns and joins every live child.
            self.auxiliary.drain().await;
            {
                let mut state = self
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                state.manual_compaction = None;
            }
            if !tail.completion.completed.load(Ordering::Acquire) {
                // No invented CompactionFailed fact after an unknown commit.
                tail.completion
                    .complete(Err(ManualCompactionError::Durable {
                        message: diagnostic,
                    }));
            }
        }
        tail.completion.joined.store(true, Ordering::Release);
        tail.completion.notify.notify_waiters();
        self.settlement.notify_waiters();
        self.wake.notify.notify_one();
    }

    /// Supervises every runtime-owned operation to its strongest honest
    /// settlement, then publishes `Quiescent` only if nothing prevents it.
    ///
    /// # Failure is evidence, not a stop signal
    ///
    /// A settlement or durability failure is **collected**, never returned
    /// early. Returning at the first failure would release the supervisor
    /// from siblings — an active provider turn, another background
    /// execution, a retained MCP process — that are still externally capable
    /// of acting. The drain therefore runs the full supervision sequence
    /// (current attempt → background executions → counted subsystem
    /// admissions → admission worker → capability/MCP processes) and only
    /// afterwards decides between `Quiescent` and an aggregated settlement
    /// failure.
    ///
    /// Every waited-for owner has a *native terminal boundary*: a background
    /// record settles terminally or explicitly abandons its bounded durable
    /// publication (its runner has returned either way), an MCP runtime
    /// closes and proves or disproves physical settlement, and a counted
    /// admission is released by its owner. No wait here depends on a global
    /// health flag, so one owner's failure can never be mistaken for
    /// another's settlement.
    #[allow(clippy::too_many_lines)]
    pub(super) async fn drain_to_quiescence(
        self: &Arc<Self>,
        _completion: Arc<DrainCompletion>,
        interaction_cancel_reason: CancellationReason,
    ) -> NativeDrainOutcome {
        let mut failures: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        let mut physical_failures = std::collections::BTreeSet::new();
        // This is the async reliable route boundary for child-owned
        // interactions. It runs after `Running -> Draining` has closed new
        // admission, but before the drain waits for foreground ownership, so
        // every child settlement can reach the parent control lane without a
        // lossy synchronous try-send.
        self.interaction
            .cancel_pending_async(interaction_cancel_reason)
            .await;
        // Cancellation intent reaches every sibling before an irreducible
        // auxiliary write/confirmation wait can delay this drain owner.
        if let Some(subagents) = &self.subagents {
            for child in subagents.unsettled_snapshot() {
                let _ = subagents.cancel(&child.subagent_id, CancellationReason::RuntimeShutdown);
            }
        }
        for execution in self.tool_runtime.background().unsettled_snapshot() {
            self.tool_runtime
                .background()
                .cancel_with_reason(&execution.execution_id, CancellationReason::RuntimeShutdown);
        }
        self.auxiliary.drain().await;
        let title_task = self
            .session_title_task
            .lock()
            .expect("Session title owner")
            .take();
        if let Some(task) = title_task
            && task.await.is_err()
        {
            tracing::warn!("Session title owner exited without publishing its optional label");
        }
        self.reap_manual_tail(true).await;
        loop {
            self.wait_for_foreground_operation().await;

            // The subagent plane (Issue #60): cancellation intent is already
            // committed (begin_drain); here the drain awaits every child's
            // native terminal boundary — the driver task has reaped the
            // process and the registry has settled its durable terminal or
            // explicitly abandoned the publication.
            if let Some(subagents) = &self.subagents {
                let active = subagents.unsettled_snapshot();
                if !active.is_empty() {
                    for subagent in &active {
                        let _ = subagents
                            .cancel(&subagent.subagent_id, CancellationReason::RuntimeShutdown);
                    }
                    for subagent in active {
                        subagents.wait_until_settled(&subagent.subagent_id).await;
                    }
                    continue;
                }
            }

            // Records whose durable terminal publication was abandoned are
            // excluded: their runners have returned, so cancelling and
            // awaiting them again would spin. They remain explicit evidence
            // below.
            let active = self.tool_runtime.background().unsettled_snapshot();
            if !active.is_empty() {
                for execution in &active {
                    self.tool_runtime.background().cancel_with_reason(
                        &execution.execution_id,
                        CancellationReason::RuntimeShutdown,
                    );
                }
                for execution in active {
                    #[cfg(test)]
                    self.signal_drain_supervision();
                    self.tool_runtime
                        .background()
                        .wait_until_settled(&execution.execution_id)
                        .await;
                }
                continue;
            }

            self.lifecycle.wait_for_no_admissions().await;
            self.tool_runtime.background().abort_prepared_for_drain();
            if !self
                .tool_runtime
                .background()
                .unsettled_snapshot()
                .is_empty()
            {
                continue;
            }
            if self.worker_started.load(Ordering::Acquire) {
                self.wake.wait_until_exited().await;
            }
            self.lifecycle.wait_for_no_admissions().await;
            if let Err(details) = self.capability.drain_conversation_owned().await {
                physical_failures.extend(details.iter().cloned());
                failures.extend(details);
            }
            self.lifecycle.wait_for_no_admissions().await;

            // Supervision has reached every owner's native terminal
            // boundary. Only now is the runtime allowed to decide.
            for execution_id in self.tool_runtime.background().abandoned_publications() {
                failures.insert(format!(
                    "background execution {execution_id}: the durable terminal publication is unresolved"
                ));
            }
            if let Some(subagents) = &self.subagents {
                for snapshot in subagents.abandoned_publications() {
                    failures.insert(format!(
                        "subagent {}: the durable terminal publication is unresolved",
                        snapshot.subagent_id
                    ));
                }
            }
            if let Some(subagents) = &self.subagents {
                subagents.wait_recovery_reconciliation().await;
                for activation_id in subagents.unproven_settlements() {
                    let detail =
                        format!("subagent {activation_id}: physical settlement is unresolved");
                    physical_failures.insert(detail.clone());
                    failures.insert(detail);
                }
            }
            if let Some(detail) = self.durability_failure_diagnostic() {
                failures.insert(format!("durable authority: {detail}"));
            }
            if let Some(detail) = self.lock_state().mcp_settlement_failure.clone() {
                physical_failures.insert(detail.clone());
                failures.insert(detail);
            }
            if !failures.is_empty() {
                return NativeDrainOutcome::failed(&failures, physical_failures);
            }
            if self.lifecycle.mark_quiescent() {
                return NativeDrainOutcome {
                    logical: Ok(()),
                    physical: Ok(()),
                };
            }
        }
    }

    /// Test-only: announces that the drain task has begun supervising
    /// runtime-owned owners.
    #[cfg(test)]
    pub(super) fn signal_drain_supervision(&self) {
        if let Some(signal) = self
            .probe
            .lock()
            .expect("coordinator probe lock poisoned")
            .as_ref()
            .and_then(|probe| probe.drain_supervision.clone())
        {
            signal.notify_one();
        }
    }
}

impl ConversationRuntime {
    /// Drains the conversation runtime to quiescence.
    ///
    /// Successful completion means the current attempt, all conversation-
    /// owned background executions, required durable terminal publication,
    /// counted subsystem commits, and the admission worker have settled. It
    /// is therefore stronger than cancellation request acceptance.
    ///
    /// # Lifecycle
    ///
    /// The `Running -> Draining` transition is linearized under the same
    /// coordinator lock as inbound acceptance and attempt admission. Repeated
    /// callers join the same drain completion, and only the first transition
    /// publishes [`ConversationObservation::Shutdown`].
    ///
    /// # Errors
    ///
    /// Returns [`ShutdownError::Inactive`] before activation. After
    /// activation, repeated shutdown calls are idempotent.
    ///
    /// `Ok(())` means exactly one thing: the lifecycle reached `Quiescent`,
    /// so no runtime-owned model, tool, background, capability, MCP,
    /// process, preparation, attempt task, or stale callback source can still
    /// produce an external effect or call back into the conversation.
    ///
    /// [`ShutdownError::RuntimeOwnedSettlement`] means admission is closed
    /// and supervision ran **every** settleable owner to its strongest
    /// available boundary, but rustX could not truthfully prove all required
    /// ownership/physical/durable terminal conditions. It never means
    /// supervision stopped early: a failure in one participant is collected
    /// as evidence and never releases the supervisor from a sibling that can
    /// still act.
    ///
    /// # Panics
    ///
    /// Panics only if the test-only coordinator probe lock is poisoned,
    /// which would mean a previous test hook panicked while holding it.
    pub async fn shutdown(&self) -> Result<(), ShutdownError> {
        self.shutdown_outcome().await?.logical?;
        self.inner.lifecycle.wait_until_quiescent().await;
        Ok(())
    }

    /// Retains physical-exit evidence for native writer retirement even when
    /// logical settlement failed. This never turns a failed shutdown into success.
    pub(crate) async fn shutdown_outcome(&self) -> Result<NativeDrainOutcome, ShutdownError> {
        // Test-only: signal that shutdown reached the point just before it
        // attempts the coordinator lock. Combined with the submit gate this
        // makes the submit-vs-shutdown ordering provable by mutex exclusion.
        #[cfg(test)]
        if let Some(arrival) = self
            .inner
            .probe
            .lock()
            .expect("coordinator probe lock poisoned")
            .as_ref()
            .and_then(|probe| probe.shutdown_arrival.clone())
        {
            arrival.notify_one();
        }
        let completion = self.inner.begin_drain()?;
        Ok(completion.wait_outcome().await)
    }
}

#[cfg(test)]
mod tests;
