//! Durable-authority failure accounting: bounded per-cycle retries, the
//! absorbing `DurabilityFailed` fact, and the background/subagent terminal
//! publication failure sinks.

use super::{
    ConversationObservation, ConversationRuntime, CoordinatorState, DurableOperation, RuntimeInner,
    Weak,
};

/// The semantic durable operations of the coordinator's durability-health
/// contract (Issue #63).
///
/// The retry budget is owned by one finite admission cycle, not by the last
/// operation that happened to fail. [`AdmissionRetryBudget`] retains the
/// consumed allowance for each transient stage while that cycle moves from
/// selection to adoption.
///
/// Only genuine transient storage failures earn a retry. A semantic
/// contract failure (a pending item that cannot be prepared for canonical
/// adoption, an incomplete tool turn observed by the admission guard) is
/// persistent by nature — retrying the identical transition is futile — and
/// an already-terminal durable failure (an active attempt's canonical-write
/// failure, an exhausted background publication budget) has already consumed
/// its settlement; all of these fail closed immediately.
///
/// The bounded transient retry allowance of one finite admission cycle.
///
/// Each transient stage may fail once and receive one immediate re-kick. The
/// bits are deliberately retained when the cycle advances from selection to
/// adoption, so a later failure cannot erase earlier retry debt. The budget
/// is reset only when the cycle reaches a semantic completion boundary.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct AdmissionRetryBudget {
    /// Whether the one select retry has already been consumed.
    pub(super) select_retry_used: bool,
    /// Whether the one adopt retry has already been consumed.
    pub(super) adopt_retry_used: bool,
}

impl AdmissionRetryBudget {
    /// Consumes the one retry allowance for a transient operation.
    ///
    /// Returns `true` when the allowance was available and is now consumed;
    /// returns `false` when that operation has already failed once in the
    /// current admission cycle.
    pub(super) fn try_consume(&mut self, operation: DurableOperation) -> bool {
        let used = match operation {
            DurableOperation::SelectPendingBatch => &mut self.select_retry_used,
            DurableOperation::AdoptPendingBatch => &mut self.adopt_retry_used,
            _ => unreachable!("only transient operations have an admission retry budget"),
        };
        if *used {
            false
        } else {
            *used = true;
            true
        }
    }
}

/// The most recent transient failure whose bounded re-kick is in flight.
///
/// This is only wake/diagnostic state. It is intentionally separate from
/// [`AdmissionRetryBudget`], because operation identity must not own the
/// lifetime of retry debt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PendingDurabilityRetry {
    /// The operation that will be retried by the re-kick.
    pub(super) operation: DurableOperation,
    /// The first-failure diagnostic.
    pub(super) diagnostic: String,
}

/// The coordinator's durable-authority health state (Issue #63).
///
/// A storage failure that a required transition cannot proceed without is
/// never silently swallowed and never retried forever. The admission-cycle
/// The coordinator-owned transient admission-cycle retry bookkeeping
/// (Issue #63).
///
/// This is **not** the runtime durability-health authority: the absorbing
/// `DurabilityFailed` fact lives in exactly one place, the runtime-owned
/// [`DurabilityGate`](crate::runtime::types::DurabilityGate). The
/// coordinator keeps only the finite-cycle retry allowances and the latest
/// pending re-kick here; the decision to upgrade to the absorbing failure
/// is made under the coordinator lock and committed through
/// `DurabilityGate::commit_failure` — the one mutation API of the failure
/// authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AdmissionDurabilityCycle {
    /// The independent select/adopt retry allowances for this cycle.
    pub(super) budget: AdmissionRetryBudget,
    /// The latest transient failure whose re-kick is armed.
    pub(super) pending_retry: Option<PendingDurabilityRetry>,
}

/// The background-settlement failure sink of one conversation runtime
/// (Issue #63): the narrow seam through which the background registry
/// reports an exhausted terminal-publication budget.
///
/// The runtime is the durability-health owner of its background plane, so
/// the report moves it into the explicit `DurabilityFailed` state while the
/// unresolved terminal candidate stays retained and observable in the
/// registry. The sink is invoked by the background runner without the
/// registry lock held; it acquires only the coordinator lock, so the lock
/// graph keeps its single coordinator -> registry edge direction.
pub(super) struct BackgroundFailureSink {
    pub(super) inner: Weak<RuntimeInner>,
}

impl crate::tools::background::BackgroundDurabilityFailureSink for BackgroundFailureSink {
    fn terminal_publication_failed(
        &self,
        execution_id: &crate::runtime::identity::ToolExecutionId,
        diagnostic: String,
    ) {
        let Some(inner) = self.inner.upgrade() else {
            return;
        };
        // The real callback boundary: parked here, the runner has entered its
        // last conversation-facing callback and has provably not yet
        // published `publication_abandoned`. The coordinator lock is taken
        // only after the park, so drain and every other coordinator caller
        // stay live while the callback is held.
        #[cfg(test)]
        let background_failure_gate = inner
            .probe
            .lock()
            .expect("coordinator probe lock poisoned")
            .as_ref()
            .and_then(|probe| probe.background_failure_gate.clone());
        // The probe lock is released before the park: `shutdown` reads the
        // same probe, so holding it across the gate would block the very
        // caller this boundary exists to race.
        #[cfg(test)]
        if let Some(gate) = background_failure_gate {
            gate.enter();
        }
        let mut state = inner.lock_state();
        inner.record_durability_failure(
            &mut state,
            DurableOperation::BackgroundTerminalPublication,
            format!("background execution {execution_id}: {diagnostic}"),
        );
    }
}

/// The subagent-settlement failure sink of one conversation runtime
/// (Issue #60). The registry retains its immutable `PublishingTerminal`
/// candidate; this seam only transfers exhausted durable-health ownership to
/// the coordinator.
pub(super) struct SubagentFailureSink {
    pub(super) inner: Weak<RuntimeInner>,
}

impl crate::runtime::subagent::SubagentDurabilityFailureSink for SubagentFailureSink {
    fn terminal_publication_failed(
        &self,
        subagent_id: &crate::runtime::identity::SubagentId,
        diagnostic: &str,
    ) {
        let Some(inner) = self.inner.upgrade() else {
            return;
        };
        // This callback is reached only after the registry's bounded
        // publication attempts are exhausted. It is intentionally invoked
        // outside the registry mutex, preserving the one-way lock graph:
        // ConversationRuntime/coordinator -> SubagentRegistry.
        let mut state = inner.lock_state();
        inner.record_durability_failure(
            &mut state,
            DurableOperation::SubagentTerminalPublication,
            format!("subagent {subagent_id}: {diagnostic}"),
        );
        drop(state);
        #[cfg(test)]
        let gate = inner
            .probe
            .lock()
            .expect("probe lock")
            .as_ref()
            .and_then(|probe| probe.subagent_failure_published_gate.clone());
        #[cfg(test)]
        if let Some(gate) = gate {
            gate.enter();
        }
    }
}

impl RuntimeInner {
    /// Returns the durable failure that prevents this runtime from claiming
    /// successful quiescence, if one has been recorded.
    pub(super) fn durability_failure_diagnostic(&self) -> Option<String> {
        // The one authoritative absorbing failure fact lives in the
        // runtime-owned DurabilityGate; the coordinator keeps no second
        // failed-state copy.
        self.durability_gate
            .failure()
            .map(|failure| failure.diagnostic)
    }

    /// Records a durable-authority failure without silently swallowing it
    /// (Issue #63, Finding 5).
    ///
    /// A transient storage failure ([`DurableOperation::is_transient`])
    /// consumes the allowance for that stage in the current finite admission
    /// cycle, publishes a [`ConversationObservation::DurableFailure`], and
    /// arms exactly one bounded re-kick. A second failure of that stage in
    /// the same cycle — or any non-transient failure — upgrades to the
    /// absorbing `DurabilityFailed` fact, committed through the one
    /// authority ([`DurabilityGate::commit_failure`]) and published as a
    /// [`ConversationObservation::DurabilityFailed`] exactly once; no
    /// further re-kick is armed, so a persistent or alternating fault cannot
    /// become a hot loop. The gate's one publisher reports the absorbing
    /// operation and diagnostic after any earlier ownership commit exits;
    /// repeated requests cannot publish a second observation. A failure of a different
    /// transient stage consumes its own allowance without erasing the first
    /// stage's debt.
    pub(super) fn record_durability_failure(
        &self,
        state: &mut CoordinatorState,
        operation: DurableOperation,
        diagnostic: String,
    ) {
        // Absorbing winner: the absorbing commit itself reports whether this
        // call installed the fact, so a caller-side guard is only needed
        // where retry bookkeeping must not run on an already-failed runtime.
        if !operation.is_transient() {
            self.commit_failure_and_observe(operation, diagnostic);
            return;
        }
        // The transient path arms coordinator-owned retry bookkeeping; that
        // bookkeeping must never run once the absorbing fact exists, so the
        // failed-gate check stays ahead of the admission cycle.
        if self.durability_gate.is_failed() {
            return;
        }
        let retry_armed = {
            let cycle = &mut state.admission_durability_cycle;
            if cycle.budget.try_consume(operation) {
                cycle.pending_retry = Some(PendingDurabilityRetry {
                    operation,
                    diagnostic: diagnostic.clone(),
                });
                true
            } else {
                false
            }
        };
        if retry_armed {
            self.observe(ConversationObservation::DurableFailure {
                message: diagnostic,
            });
            self.wake.notify.notify_one();
        } else {
            self.commit_failure_and_observe(operation, diagnostic);
        }
    }

    /// Commits the absorbing `DurabilityFailed` fact through the one
    /// authority and publishes the single
    /// [`ConversationObservation::DurabilityFailed`] only when THIS call
    /// installed the fact; the observation's operation and diagnostic are
    /// the authoritative ones returned by the commit.
    pub(super) fn commit_failure_and_observe(
        &self,
        operation: DurableOperation,
        diagnostic: String,
    ) {
        // An already-authorized writer can finish before this failure becomes
        // committed. The gate's one publisher runs after that physical boundary.
        let _ = self.durability_gate.commit_failure(operation, diagnostic);
    }

    /// Records progress through one durable stage. This clears only the
    /// matching pending re-kick marker; it never resets the admission-cycle
    /// budget. A stage success is not a semantic completion boundary.
    pub(super) fn record_durability_success(
        state: &mut CoordinatorState,
        operation: DurableOperation,
    ) {
        if let Some(pending_retry) = &mut state.admission_durability_cycle.pending_retry
            && pending_retry.operation == operation
        {
            state.admission_durability_cycle.pending_retry = None;
        }
    }

    /// Completes the current finite admission cycle and starts a fresh one.
    ///
    /// This is intentionally called only after selection proves there is no
    /// pending work or after the selected batch is durably adopted. Success
    /// of an intermediate select/adopt stage must retain the consumed bits.
    pub(super) fn complete_admission_cycle(state: &mut CoordinatorState) {
        state.admission_durability_cycle.budget = AdmissionRetryBudget::default();
        state.admission_durability_cycle.pending_retry = None;
    }
}

impl ConversationRuntime {
    /// The runtime's one absorbing durability-failure fact, if committed
    /// (test-only): the same fact every durable admission gate reads.
    #[cfg(test)]
    pub(crate) fn durability_failure(&self) -> Option<crate::runtime::types::DurabilityFailure> {
        self.inner.durability_gate.failure()
    }
}

#[cfg(test)]
mod tests;
