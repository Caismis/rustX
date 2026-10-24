//! Deterministic test synchronization hooks of the coordinator. Every hook is
//! compiled only for tests; production paths call no-op probe entry points.

#[cfg(test)]
use super::{
    AgentId, Arc, ConversationId, ConversationRuntimeError, ConversationState, DurableOperation,
    MessageBlock, MessageId, Mutex, PendingObservations, RuntimeBootstrapError,
    RuntimeBootstrapSnapshot, RuntimeConversationConfig, Weak,
};
use super::{ConversationRuntime, RuntimeInner};

/// Test-only synchronization hooks of the conversation coordinator.
///
/// Every hook is an armed gate that parks exactly one operation and then
/// releases/disarms itself, so a test can construct exact interleavings
/// without timing assumptions:
///
/// - `admission_gate`: parked at the entrance of `admit_next_attempt`,
///   **before** the coordinator lock is acquired, so a competing publish
///   can still enqueue while the admission is gated. A gated admission
///   runs when the gate is released; a later admission (for example the
///   settlement handoff) passes unarmed.
/// - `settlement_gate`: parked inside `finish_attempt` after the
///   conversation state is restored and the current-attempt slot is
///   cleared, **before** the next-admission handoff, so an enqueue during
///   the park provably races the settlement-to-next-attempt boundary.
/// - `activation_gate`: parked at the entrance of `ConversationRuntime::activate`,
///   **before** the coordinator lock is acquired and before the lifecycle
///   transition, so while the park holds the conversation is provably
///   still `Inactive` and every competing runtime-owned commit or host
///   bind can still proceed. Releasing the gate commits the one
///   `Inactive -> Running` transition.
/// - `manual_compaction_settlement_gate`: parked after the durable compaction
///   commit and task-local hot-state installation, but before the coordinator
///   restores `ConversationState`, clears the maintenance slot, or publishes
///   manual completion to Runtime Client observers.
///
/// All synchronization is `std` (mutex + condvar) because the coordinator
/// boundary is a `std` mutex critical section; the parking blocks the OS
/// thread, so the race tests run on a multi-threaded runtime. These hooks
/// exist only under `#[cfg(test)]` and are never installed by production
/// code.
#[cfg(test)]
#[derive(Debug, Default)]
#[allow(clippy::struct_field_names)]
pub(crate) struct CoordinatorProbe {
    pub(crate) title_source_reads: Option<Arc<std::sync::atomic::AtomicUsize>>,
    /// Parks the next admission when armed.
    pub(crate) admission_gate: Option<Arc<Gate>>,
    /// Signals only after admission proves the coordinator lock is held by
    /// a competitor. Consumed once, immediately before normal lock acquisition.
    pub(crate) admission_contended: Option<Arc<tokio::sync::Notify>>,
    /// Parks after admission freezes and publishes its invocation while the
    /// coordinator lock is still held.
    pub(crate) admission_committed_gate: Option<Arc<Gate>>,
    /// Parks an admitted Attempt after releasing the coordinator, before its
    /// execution task can publish further observations.
    pub(crate) attempt_execution_gate: Option<Arc<Gate>>,
    /// Signals only after model selection proves the coordinator lock is held
    /// by a competitor. Consumed once before normal lock acquisition.
    pub(crate) selection_contended: Option<Arc<tokio::sync::Notify>>,
    /// Parks model selection after taking the coordinator lock and before commit.
    pub(crate) selection_locked_gate: Option<Arc<Gate>>,
    /// Parks after the mutation retains durability ordering, before persistence.
    pub(crate) selection_commit_gate: Option<Arc<Gate>>,
    /// Parks the next settlement handoff when armed.
    pub(crate) settlement_gate: Option<Arc<Gate>>,
    pub(crate) scheduled_settlement_gate: Option<Arc<Gate>>,
    /// Parks after durable terminal publication, before restoring conversation state.
    pub(crate) before_restore_gate: Option<Arc<Gate>>,
    /// Parks the next activation before the lifecycle transition when
    /// armed.
    pub(crate) activation_gate: Option<Arc<Gate>>,
    /// Parks the next manual compaction immediately before coordinator
    /// settlement restores ownership and publishes completion.
    pub(crate) manual_compaction_settlement_gate: Option<Arc<Gate>>,
    /// Parks the next `submit_inbound` **after** the coordinator lock is
    /// acquired and the shutdown/activation decision is read, but **before**
    /// the durable acceptance. This is the exact critical-section window the
    /// Issue #63 (Finding 1) fix closes.
    pub(crate) submit_gate: Option<Arc<Gate>>,
    /// Signals entry to `submit_inbound` before it attempts the coordinator
    /// lock. This lets a race test prove that a competing submit was started
    /// while a failure-drain critical section still owns that lock.
    pub(crate) submit_arrival: Option<Arc<tokio::sync::Notify>>,
    /// Signals that `shutdown` reached the point just before it attempts the
    /// coordinator lock. This makes the submit-vs-shutdown ordering provable
    /// by mutex exclusion instead of a timing assumption.
    pub(crate) shutdown_arrival: Option<Arc<tokio::sync::Notify>>,
    /// Parks the first MCP failure-drain operation after it has published the
    /// failure latch, lifecycle transition, and current-attempt cancellation
    /// arbitration, but before the coordinator lock is released.
    pub(crate) mcp_failure_drain_gate: Option<Arc<Gate>>,
    /// Signals immediately after `Running -> Draining` linearizes.
    pub(crate) drain_linearization: Option<Arc<tokio::sync::Notify>>,
    /// Signals immediately before the drain task **parks on one concrete
    /// runtime-owned owner** (the current attempt, or one background
    /// execution's settlement). Observing it proves supervision is committed
    /// to awaiting that owner: a drain that short-circuited on an
    /// already-known failure could never reach the park.
    pub(crate) drain_supervision: Option<Arc<tokio::sync::Notify>>,
    /// Parks the settled attempt **task** after the current-attempt slot is
    /// cleared and before its final admission callback and task exit.
    pub(crate) attempt_exit_gate: Option<Arc<Gate>>,
    /// Parks each [`ConversationRuntime::seal_parent_guidance`] evaluation
    /// **before** it acquires the coordinator lock (Issue #193).
    ///
    /// While the park holds, the attempt whose terminal the child driver
    /// just observed has passed its last inbound safe boundary and the seal
    /// has provably not committed, so a guidance submitted during the park
    /// races the terminal linearization point itself — not some arbitrary
    /// earlier instant. Like every gate above it parks exactly one
    /// evaluation and then disarms, so a seal answered `Open` and repeated
    /// runs through unparked.
    pub(crate) parent_guidance_seal_gate: Option<Arc<Gate>>,
    /// Parks the background settlement continuation **inside** its last
    /// conversation-facing callback: entered at the top of
    /// [`BackgroundFailureSink::terminal_publication_failed`], before the
    /// coordinator lock is taken and before the durability-health mutation,
    /// and therefore before the registry publishes `publication_abandoned`.
    /// While the park holds, the failing execution has provably not crossed
    /// its abandoned settlement boundary.
    pub(crate) background_failure_gate: Option<Arc<Gate>>,
    /// Parks after runtime degradation, before the registry publishes abandonment.
    pub(crate) subagent_failure_published_gate: Option<Arc<Gate>>,
    /// Installed into the **next** admitted attempt's execution: the M9b
    /// model-turn start-boundary pause (Issue #12). `take`n by the next
    /// `run_attempt`, so it arms exactly one attempt.
    pub(crate) start_boundary_pause: Option<crate::agent::execution::test_sync::StartBoundaryPause>,
    /// Installed into the next admitted attempt's model stream: parks after
    /// the selected provider item has been fully processed and before the
    /// next provider/cancellation arbitration. `take`n by the next
    /// `run_attempt`, so a drain test can hold an actually-started provider
    /// turn live while shutdown cancellation is requested.
    pub(crate) model_arbitration_pause:
        Option<crate::agent::execution::test_sync::ModelArbitrationPause>,
    /// Installed into the next admitted attempt's foreground tool batch:
    /// parks after the first `ToolExecutionStarted` fact so a test can
    /// linearize drain against the sibling start frontier.
    pub(crate) tool_start_pause: Option<crate::agent::execution::test_sync::ToolStartPause>,
}

/// One two-phase gate of a coordinator boundary (test-only).
#[cfg(test)]
#[derive(Debug, Default)]
pub(crate) struct Gate {
    pub(super) state: Mutex<GateState>,
    pub(super) condvar: std::sync::Condvar,
}

#[cfg(test)]
#[derive(Debug, Default)]
pub(super) struct GateState {
    pub(super) armed: bool,
    pub(super) entered: bool,
    pub(super) proceed: bool,
}

#[cfg(test)]
impl Gate {
    /// This boundary runs in the async admission worker. Hand its executor
    /// core (including any just-woken LIFO task) off before parking the OS
    /// thread. Other coordinator probes retain their original lock schedule.
    pub(super) fn enter_attempt_execution(&self) {
        tokio::task::block_in_place(|| self.enter());
    }

    /// Scope the parked worker to the test, including assertion unwinding.
    pub(crate) fn arm_scoped(self: &Arc<Self>) -> GateRelease {
        self.arm();
        GateRelease(self.clone())
    }
    /// Signals that the boundary was entered; when armed, parks until
    /// [`Gate::release`]. An unarmed gate never blocks.
    pub(crate) fn enter(&self) {
        let mut state = self.state.lock().expect("coordinator probe lock poisoned");
        if !state.armed {
            return;
        }
        state.entered = true;
        self.condvar.notify_all();
        while !state.proceed {
            state = self
                .condvar
                .wait(state)
                .expect("coordinator probe wait poisoned");
        }
        state.armed = false;
    }

    /// Arms the gate: the next [`Gate::enter`] parks.
    pub(crate) fn arm(&self) {
        let mut state = self.state.lock().expect("coordinator probe lock poisoned");
        state.armed = true;
        state.entered = false;
        state.proceed = false;
    }

    /// Blocks until the boundary was entered.
    pub(crate) fn wait_entered(&self) {
        let mut state = self.state.lock().expect("coordinator probe lock poisoned");
        while !state.entered {
            state = self
                .condvar
                .wait(state)
                .expect("coordinator probe wait poisoned");
        }
    }

    /// Releases a parked boundary and disarms the gate.
    pub(crate) fn release(&self) {
        let mut state = self.state.lock().expect("coordinator probe lock poisoned");
        state.proceed = true;
        self.condvar.notify_all();
    }
}

#[cfg(test)]
pub(crate) struct GateRelease(Arc<Gate>);
#[cfg(test)]
impl Drop for GateRelease {
    fn drop(&mut self) {
        self.0.release();
    }
}

#[cfg(test)]
impl ConversationRuntime {
    pub(crate) fn hold_auxiliary_retirement_for_test(
        &self,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        std::sync::mpsc::Sender<()>,
    ) {
        self.inner.auxiliary.hold_retirement_for_test()
    }

    pub(crate) fn hold_auxiliary_settlement_for_test(
        &self,
    ) -> crate::runtime::auxiliary::executor::AuxiliaryWriteTestGates {
        self.inner.auxiliary.hold_settlement_for_test()
    }

    pub(crate) async fn join_auxiliary_for_test(&self) {
        self.inner.auxiliary.drain().await;
    }

    pub(crate) fn install_durability_mutation_probe(
        &self,
        submit: Option<Arc<Gate>>,
        selection: Option<Arc<Gate>>,
    ) {
        let mut probe = self.inner.probe.lock().expect("probe lock");
        let probe = probe.get_or_insert_with(CoordinatorProbe::default);
        probe.submit_gate = submit;
        probe.selection_commit_gate = selection;
    }

    /// Execution-boundary fixtures observe original Journal facts after
    /// native receipt notice, without pretending to install a client cut.
    pub(super) fn install_fixture_observation_bridge(
        &self,
        queue: &Arc<PendingObservations>,
    ) -> Result<RuntimeBootstrapSnapshot, RuntimeBootstrapError> {
        let seed = self.install_observation_bridge(queue.clone())?;
        let through = self
            .inner
            .store
            .presentation_frontier()
            .map_err(|error| RuntimeBootstrapError::Durable(error.to_string()))?;
        let recorder = crate::scripted_suites::support::native_journal::NativeJournalRecorder::bind(
            self,
            &self.inner.store,
            queue,
            through,
        );
        queue.bind_fixture_journal(recorder, through);
        Ok(seed)
    }

    /// Runs the ordinary coordinator admission boundary synchronously in a test.
    pub(crate) fn admit_now_for_test(&self) {
        self.inner.admit_next_attempt();
    }

    /// Commits a synthetic durable-authority failure through the exact
    /// production `record_durability_failure` path (Issue #60 regression
    /// seam).
    ///
    /// The given operation is committed through the one authority: a
    /// non-transient operation enters `DurabilityFailed` immediately and
    /// marks the shared durability frontier (`DurabilityGate`) failed under
    /// the same commit — exactly what a real exhaustion (subagent or
    /// background terminal publication) performs. Tests use it to force the
    /// health-failure side of the ownership-vs-failure total order
    /// deterministically.
    pub(crate) fn force_durability_failure_for_test(
        &self,
        operation: DurableOperation,
        diagnostic: &str,
    ) {
        let mut state = self.inner.lock_state();
        self.inner
            .record_durability_failure(&mut state, operation, diagnostic.to_owned());
    }

    /// The settled canonical ledger of this conversation, or `None` while
    /// an attempt owns the in-memory conversation state.
    pub(crate) fn settled_ledger(&self) -> Option<Vec<MessageBlock>> {
        self.inner
            .state
            .lock()
            .expect("runtime lock")
            .conversation
            .as_ref()
            .map(|conversation| conversation.ledger().audit_records())
    }

    /// The runtime-owned Message Ledger records, or `None` while an attempt
    /// owns the conversation state.
    pub(crate) fn coordinator_ledger(&self) -> Option<Vec<MessageBlock>> {
        self.settled_ledger()
    }

    /// The runtime-owned active Surface identities, or `None` while an
    /// attempt owns the conversation state.
    pub(crate) fn coordinator_active_ids(&self) -> Option<Vec<MessageId>> {
        self.inner
            .state
            .lock()
            .expect("runtime lock")
            .conversation
            .as_ref()
            .map(ConversationState::active_ids)
    }

    /// Whether idle maintenance currently owns the conversation state.
    #[cfg(test)]
    pub(crate) fn has_manual_compaction(&self) -> bool {
        self.inner
            .state
            .lock()
            .expect("runtime lock")
            .manual_compaction
            .is_some()
    }

    /// A non-owning handle to the shared runtime state, for lifetime tests.
    pub(crate) fn weak_inner(&self) -> Weak<RuntimeInner> {
        Arc::downgrade(&self.inner)
    }

    /// Installs one test-only pre-tool policy for the next runtime-created
    /// attempt. Production derives the policy from the admitted effective
    /// `ApprovalMode`; this hook exists solely to exercise the real
    /// `ConversationRuntime` ownership and drain path without exposing a
    /// public policy factory.
    #[cfg(test)]
    pub(crate) fn install_test_pre_tool_policy(
        &self,
        policy: Arc<dyn crate::agent::PreToolPolicy>,
    ) {
        *self
            .inner
            .test_pre_tool_policy
            .lock()
            .expect("test pre-tool policy lock") = Some(policy);
    }
}

#[cfg(test)]
pub(crate) async fn runtime_with_recovered_registry_for_test(
    dir: &tempfile::TempDir,
    conversation: &ConversationId,
    agent: &AgentId,
    store: Arc<dyn crate::durable::ConversationStore>,
    registry: crate::runtime::subagent::SubagentRegistry,
) -> (
    ConversationRuntime,
    crate::runtime::subagent::SubagentRegistry,
) {
    let (_, mut config) = super::test_support::subagent_runtime_config_with_registry(
        dir,
        conversation.as_str(),
        store.clone(),
        conversation,
        agent,
        agent,
    )
    .await;
    let registry = registry.fresh_with_mailbox_for_test(config.tool_runtime.mailbox());
    config.subagents = Some(registry.clone());
    (ConversationRuntime::new(config).unwrap(), registry)
}

impl RuntimeInner {
    #[cfg(test)]
    pub(super) fn enter_attempt_execution_probe(&self) {
        let gate = self
            .probe
            .lock()
            .expect("coordinator probe lock poisoned")
            .as_ref()
            .and_then(|probe| probe.attempt_execution_gate.clone());
        if let Some(gate) = gate {
            gate.enter_attempt_execution();
        }
    }

    #[cfg(test)]
    pub(super) fn enter_probe_gate(
        &self,
        select: impl FnOnce(&CoordinatorProbe) -> Option<Arc<Gate>>,
    ) {
        let gate = self
            .probe
            .lock()
            .expect("coordinator probe lock poisoned")
            .as_ref()
            .and_then(select);
        // Never retain the probe lock while parking a coordinator participant.
        if let Some(gate) = gate {
            gate.enter();
        }
    }
}

impl ConversationRuntime {
    /// Creates the runtime with the test-only coordinator synchronization
    /// hooks installed. Only available under `#[cfg(test)]`; never used by
    /// production code.
    #[cfg(test)]
    pub(crate) fn with_probe(
        config: RuntimeConversationConfig,
        probe: CoordinatorProbe,
    ) -> Result<Self, ConversationRuntimeError> {
        let runtime = Self::new(config)?;
        *runtime
            .inner
            .probe
            .lock()
            .expect("coordinator probe lock poisoned") = Some(probe);
        Ok(runtime)
    }

    #[cfg(test)]
    pub(crate) fn install_configuration_admission_gate(&self, gate: Arc<Gate>) {
        self.inner
            .probe
            .lock()
            .expect("probe")
            .get_or_insert_with(Default::default)
            .admission_gate = Some(gate);
    }

    #[cfg(test)]
    pub(crate) fn install_drain_signals(
        &self,
        arrival: Arc<tokio::sync::Notify>,
        linearization: Arc<tokio::sync::Notify>,
    ) {
        let mut probe = self.inner.probe.lock().expect("probe lock");
        let probe = probe.get_or_insert_with(CoordinatorProbe::default);
        probe.shutdown_arrival = Some(arrival);
        probe.drain_linearization = Some(linearization);
    }

    #[cfg(test)]
    pub(crate) fn install_admission_gate(&self, gate: Arc<Gate>) {
        self.inner
            .probe
            .lock()
            .expect("probe lock")
            .get_or_insert_with(CoordinatorProbe::default)
            .admission_gate = Some(gate);
    }

    #[cfg(test)]
    pub(crate) fn install_attempt_execution_gate(&self, gate: Arc<Gate>) {
        self.inner
            .probe
            .lock()
            .expect("probe lock")
            .get_or_insert_with(CoordinatorProbe::default)
            .attempt_execution_gate = Some(gate);
    }

    #[cfg(test)]
    pub(crate) fn install_model_selection_race_probe(
        &self,
        admission_contended: Option<Arc<tokio::sync::Notify>>,
        admission_committed: Option<Arc<Gate>>,
        selection_contended: Option<Arc<tokio::sync::Notify>>,
        selection_locked: Option<Arc<Gate>>,
    ) {
        let mut probe = self.inner.probe.lock().expect("probe lock");
        let probe = probe.get_or_insert_with(CoordinatorProbe::default);
        probe.admission_contended = admission_contended;
        probe.admission_committed_gate = admission_committed;
        probe.selection_contended = selection_contended;
        probe.selection_locked_gate = selection_locked;
    }

    #[cfg(test)]
    pub(crate) fn install_activation_gate(&self, gate: Arc<Gate>) {
        self.inner
            .probe
            .lock()
            .expect("probe lock")
            .get_or_insert_with(CoordinatorProbe::default)
            .activation_gate = Some(gate);
    }

    /// Install the existing owner-boundary probes on a natively composed runtime.
    #[cfg(test)]
    pub(crate) fn install_residency_probe(
        &self,
        submit: Option<Arc<Gate>>,
        attempt_exit: Option<Arc<Gate>>,
    ) {
        let mut probe = self.inner.probe.lock().expect("probe lock");
        let probe = probe.get_or_insert_with(CoordinatorProbe::default);
        probe.submit_gate = submit;
        probe.attempt_exit_gate = attempt_exit;
    }

    /// Inject at the existing native settlement-failure callback, not at the
    /// manager's shutdown result, so residency tests exercise the real drain.
    #[cfg(test)]
    pub(crate) fn fail_residency_settlement(&self) {
        self.inner
            .fence_mcp_settlement_failure("residency test settlement failure".into());
    }

    #[cfg(test)]
    pub(crate) fn install_scheduled_settlement_gate(&self, gate: Arc<Gate>) {
        self.inner
            .probe
            .lock()
            .unwrap()
            .get_or_insert_with(CoordinatorProbe::default)
            .scheduled_settlement_gate = Some(gate);
    }

    #[cfg(test)]
    pub(crate) fn install_before_restore_gate(&self, gate: Arc<Gate>) {
        self.inner
            .probe
            .lock()
            .unwrap()
            .get_or_insert_with(CoordinatorProbe::default)
            .before_restore_gate = Some(gate);
    }
}

#[cfg(test)]
mod tests;
