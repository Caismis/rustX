//! Session-owned, bounded proactive Summary supervision.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

use futures_util::future::BoxFuture;

use super::{
    AgentCancellation, AttemptId, CancellationReason, ConversationId, ConversationRuntime,
    CurrentManualCompaction, DurableOperation, RuntimeEvent, RuntimeInner, WakeGate,
    render_effective_system_prompt,
};
use crate::context::asynchronous::{
    CompactionAdmission, CompactionCoordinator, CompactionDelivery, CompactionDisposition,
    ReadyCompaction,
};
use crate::context::compaction::{
    ActivationOutcome, CapturedCompaction, CompactionExecutionError, GeneratedCompaction,
};
use crate::context::operation::{CompactionOperationPhase, CompactionOperationStatus};
use crate::context::{ContextError, ContextErrorKind};
use crate::runtime::cancellation::CancellationSignal;

#[derive(Default)]
pub(super) struct Registry {
    next_generation: u64,
    proactive: Option<Arc<Work>>,
}

struct Work {
    admission: CompactionAdmission,
    state: Mutex<WorkState>,
    publication: Mutex<()>,
    #[cfg(test)]
    ready_publication_gate: Mutex<Option<ReadyPublicationGate>>,
    #[cfg(test)]
    defer_settlement_gate: Mutex<Option<ArbitrationTestGate>>,
    #[cfg(test)]
    generation_settlement_gate: Mutex<Option<ArbitrationTestGate>>,
    #[cfg(test)]
    cancellation_request_gate: Mutex<Option<ArbitrationTestGate>>,
    #[cfg(test)]
    named_cancellation_publication_gate: Mutex<Option<ArbitrationTestGate>>,
    changed: tokio::sync::Notify,
    join: Mutex<Option<tokio::task::JoinHandle<()>>>,
    exit_announced: AtomicBool,
    joined: AtomicBool,
    lifecycle: Mutex<Option<crate::runtime::types::LifecycleAdmission>>,
}

#[cfg(test)]
struct ReadyPublicationGate {
    entered: tokio::sync::oneshot::Sender<()>,
    release: std::sync::mpsc::Receiver<()>,
}

#[cfg(test)]
struct ArbitrationTestGate {
    entered: tokio::sync::oneshot::Sender<()>,
    release: std::sync::mpsc::Receiver<()>,
}

#[cfg(test)]
fn wait_for_arbitration_test_gate(slot: &Mutex<Option<ArbitrationTestGate>>) {
    let gate = slot
        .lock()
        .expect("compaction arbitration test gate")
        .take();
    if let Some(gate) = gate {
        let _ = gate.entered.send(());
        let _ = gate.release.recv();
    }
}

// Publication, supersession, cancellation and retry are independent facts;
// Phase alone cannot represent their concurrent combinations.
#[allow(clippy::struct_excessive_bools)]
struct WorkState {
    status: CompactionOperationStatus,
    published: Option<CompactionOperationStatus>,
    phase: Phase,
    retired_originals: Vec<crate::runtime::identity::MessageId>,
    pending_terminal_publication: bool,
    planned_coverage: Option<PlannedCoverage>,
    superseded: bool,
    cancel_requested: bool,
    retrying: bool,
    #[cfg(test)]
    force_blocking_competition: bool,
}

enum Phase {
    Capturing,
    Generating,
    Ready(Arc<GeneratedCompaction>),
    Installing(Arc<GeneratedCompaction>),
    Failed(ContextError),
    Terminal,
}

/// An internal upper bound used only to stop provably superseded work. It
/// contains no rendered-source evidence and can never authorize a checkpoint.
#[derive(Clone)]
struct PlannedCoverage {
    root: crate::context::coverage::CoverageRoot,
    retired_through: u64,
}

impl PlannedCoverage {
    fn from_capture(captured: &CapturedCompaction) -> Option<Self> {
        use crate::context::coverage::CoverageRoot;
        let count = u64::try_from(captured.retired_count())
            .ok()
            .filter(|count| *count > 0)?;
        let cut = captured.source_cut();
        match captured.source_checkpoint() {
            None if cut.projection_activation.0 == 0 => Some(Self {
                root: CoverageRoot::LedgerGenesis {
                    conversation_id: cut.conversation_id.clone(),
                },
                retired_through: count,
            }),
            Some(previous) if cut.projection_activation.0 > 0 => {
                if let Some(prior) = &previous.candidate.prefix_coverage {
                    prior.validate_shape().ok()?;
                    Some(Self {
                        root: prior.root.clone(),
                        retired_through: prior.retired_through.checked_add(count - 1)?,
                    })
                } else {
                    Some(Self {
                        root: CoverageRoot::OpaqueCheckpoint {
                            source: crate::model::projected::CheckpointSourceRef {
                                conversation_id: cut.conversation_id.clone(),
                                checkpoint_id: previous.candidate.checkpoint_id.clone(),
                                content_digest: previous.fingerprint.clone(),
                            },
                        },
                        retired_through: count,
                    })
                }
            }
            _ => None,
        }
    }

    fn retained_offset(
        &self,
        conversation_id: &ConversationId,
        head: Option<&crate::durable::projection::ProjectionCheckpoint>,
    ) -> Option<usize> {
        use crate::context::coverage::CoverageRoot;
        match head {
            None if self.root
                == (CoverageRoot::LedgerGenesis {
                    conversation_id: conversation_id.clone(),
                }) =>
            {
                usize::try_from(self.retired_through).ok()
            }
            Some(head) if head.candidate.prefix_coverage.is_some() => {
                let active = head.candidate.prefix_coverage.as_ref()?;
                if self.root != active.root || self.retired_through <= active.retired_through {
                    return None;
                }
                usize::try_from(self.retired_through - active.retired_through)
                    .ok()?
                    .checked_add(1)
            }
            Some(head) => {
                let source = crate::model::projected::CheckpointSourceRef {
                    conversation_id: conversation_id.clone(),
                    checkpoint_id: head.candidate.checkpoint_id.clone(),
                    content_digest: head.fingerprint.clone(),
                };
                if self.root != (CoverageRoot::OpaqueCheckpoint { source }) {
                    return None;
                }
                usize::try_from(self.retired_through).ok()
            }
            _ => None,
        }
    }
}

/// Agent execution holds only a weak reference to its Session authority.
pub(super) struct RuntimeCompactionCoordinator(pub(super) Weak<RuntimeInner>);

struct WorkerExit {
    work: Arc<Work>,
    wake: Arc<WakeGate>,
}

/// A dropped wait is not cancellation of the independently owned operation.
struct WaitingAttempt {
    owner: Weak<RuntimeInner>,
    work: Arc<Work>,
    attempt: AttemptId,
}

impl Drop for WaitingAttempt {
    fn drop(&mut self) {
        let publish = {
            let mut state = self
                .work
                .state
                .lock()
                .expect("asynchronous compaction state");
            if state.status.phase.is_terminal()
                || state.status.waiting_attempt_id.as_ref() != Some(&self.attempt)
            {
                false
            } else {
                state.status.waiting_attempt_id = None;
                let phase = match &state.phase {
                    Phase::Ready(_) | Phase::Installing(_) => Some(CompactionOperationPhase::Ready),
                    Phase::Capturing | Phase::Generating => Some(if state.retrying {
                        CompactionOperationPhase::Retrying
                    } else {
                        CompactionOperationPhase::Generating
                    }),
                    Phase::Failed(_) | Phase::Terminal => None,
                };
                if let Some(phase) = phase {
                    state.status.phase = phase;
                }
                true
            }
        };
        if publish
            && let Some(owner) = self.owner.upgrade()
            && let Err(error) = owner.publish_async_status(&self.work)
        {
            owner.fail_async_durability(&error);
        }
    }
}

impl Drop for WorkerExit {
    fn drop(&mut self) {
        self.work.exit_announced.store(true, Ordering::Release);
        self.work.changed.notify_waiters();
        self.wake.notify.notify_one();
    }
}

impl Work {
    /// A physical join failure settles only work still owned by generation.
    /// Other owners keep their decision, including an unpaid terminal status.
    fn worker_join_failed(&self) -> bool {
        let mut state = self.state.lock().expect("asynchronous compaction state");
        match &state.phase {
            Phase::Installing(_) => return false,
            Phase::Terminal | Phase::Failed(_) => {
                return state.published.as_ref() != Some(&state.status);
            }
            Phase::Ready(_)
                if state
                    .published
                    .as_ref()
                    .is_some_and(|status| status.phase == CompactionOperationPhase::Ready) =>
            {
                return false;
            }
            Phase::Capturing | Phase::Generating | Phase::Ready(_) => {}
        }
        state.status.waiting_attempt_id = None;
        if state.superseded || state.cancel_requested || self.admission.cancellation.is_cancelled()
        {
            state.phase = Phase::Terminal;
            state.status.phase = if state.superseded {
                CompactionOperationPhase::Superseded
            } else {
                CompactionOperationPhase::Cancelled
            };
            state.status.diagnostic = None;
        } else {
            let error = ContextError::new(
                ContextErrorKind::AuxiliarySupervisionFailed,
                "asynchronous compaction worker exited unexpectedly",
            );
            state.status.phase = CompactionOperationPhase::Failed;
            state.status.diagnostic = Some(error.message.clone());
            state.phase = Phase::Failed(error);
        }
        true
    }
}

impl RuntimeInner {
    fn async_work(&self, admission: &CompactionAdmission) -> Option<Arc<Work>> {
        self.async_compaction
            .lock()
            .expect("asynchronous compaction registry")
            .as_ref()?
            .proactive
            .as_ref()
            .filter(|work| {
                work.admission.operation_id == admission.operation_id
                    && work.admission.generation == admission.generation
            })
            .cloned()
    }

    /// The capture owner has settled without transferring a worker. A named
    /// attempt may retire only its own capture; drain calls this without an ID
    /// after all foreground owners have returned.
    pub(super) fn settle_abandoned_async_capture(self: &Arc<Self>, trigger: Option<&AttemptId>) {
        let work = self
            .async_compaction
            .lock()
            .expect("asynchronous compaction registry")
            .as_ref()
            .and_then(|registry| registry.proactive.clone());
        let Some(work) = work else { return };
        let abandoned = {
            let mut state = work.state.lock().expect("asynchronous compaction state");
            if !matches!(state.phase, Phase::Capturing)
                || trigger.is_some_and(|id| state.status.triggering_attempt_id.as_ref() != Some(id))
            {
                false
            } else {
                state.phase = Phase::Terminal;
                state.status.phase = CompactionOperationPhase::Cancelled;
                state.status.waiting_attempt_id = None;
                state.status.diagnostic = None;
                true
            }
        };
        if !abandoned {
            return;
        }
        // The phase transition fences a late start before cancellation takes
        // its independent gate. There can be no physical worker in Capturing.
        work.admission.cancellation.cancel();
        if let Err(error) = self.publish_async_status(&work) {
            self.fail_async_durability(&error);
        }
        work.joined.store(true, Ordering::Release);
        work.lifecycle
            .lock()
            .expect("asynchronous lifecycle")
            .take();
        work.changed.notify_waiters();
        self.wake.notify.notify_one();
    }

    fn publish_async_status(&self, work: &Work) -> Result<(), ContextError> {
        let _publication = work
            .publication
            .lock()
            .expect("asynchronous status publication");
        let status = {
            let state = work.state.lock().expect("asynchronous compaction state");
            if state.published.as_ref() == Some(&state.status) {
                return Ok(());
            }
            state.status.clone()
        };
        #[cfg(test)]
        if status.phase == CompactionOperationPhase::Ready {
            let gate = work
                .ready_publication_gate
                .lock()
                .expect("ready publication gate")
                .take();
            if let Some(gate) = gate {
                let _ = gate.entered.send(());
                let _ = gate.release.recv();
            }
        }
        self.publish_compaction_status(status.clone())?;
        work.state
            .lock()
            .expect("asynchronous compaction state")
            .published = Some(status);
        Ok(())
    }

    fn fail_async_durability(self: &Arc<Self>, error: &ContextError) {
        self.fail_async_native(DurableOperation::EventJournal, error.message.clone());
    }

    fn fail_async_native(self: &Arc<Self>, operation: DurableOperation, diagnostic: String) {
        {
            let mut state = self.lock_state();
            // Close the live Attempt's start gate before publishing the
            // absorbing failure. A start already granted remains real; no
            // later primary request can cross this native arbitration.
            if let Some(current) = state.foreground.slot.as_attempt() {
                let _ = current
                    .cancellation
                    .request_cancel(CancellationReason::RuntimeShutdown);
            }
            self.record_durability_failure(&mut state, operation, diagnostic);
        }
        // Required async facts share the native durability frontier. A real
        // drain arbitrates with an active Attempt's next start grant.
        let _ = self.begin_drain();
    }

    /// Drain revokes starts and installation grants but preserves join ownership.
    pub(super) fn cancel_async_compaction(&self) {
        let work = self
            .async_compaction
            .lock()
            .expect("asynchronous compaction registry")
            .as_ref()
            .and_then(|registry| registry.proactive.clone());
        if let Some(work) = work {
            work.admission.cancellation.cancel();
            {
                let mut state = work.state.lock().expect("asynchronous compaction state");
                if matches!(state.phase, Phase::Ready(_)) {
                    state.phase = Phase::Terminal;
                    state.status.phase = CompactionOperationPhase::Cancelled;
                    state.status.diagnostic = None;
                    state.status.waiting_attempt_id = None;
                    state.pending_terminal_publication = true;
                }
            }
            work.changed.notify_waiters();
        }
    }

    /// Request cancellation only of the exact independent operation. The
    /// cancellation gate arbitrates with its native Summary/start and commit
    /// grants; an already granted commit may still settle as Committed.
    fn cancel_named_async_compaction(
        self: &Arc<Self>,
        operation_id: &crate::runtime::identity::CompactionOperationId,
    ) -> Result<bool, ContextError> {
        let Ok(_admission) = self.lifecycle.try_enter_running() else {
            return Ok(false);
        };
        let work = self
            .async_compaction
            .lock()
            .expect("asynchronous compaction registry")
            .as_ref()
            .and_then(|registry| registry.proactive.as_ref())
            .filter(|work| &work.admission.operation_id == operation_id)
            .cloned();
        let Some(work) = work else { return Ok(false) };
        {
            let state = work.state.lock().expect("asynchronous compaction state");
            if state.superseded
                || state.cancel_requested
                || work.admission.cancellation.is_cancelled()
                || matches!(state.phase, Phase::Failed(_) | Phase::Terminal)
            {
                return Ok(false);
            }
        }
        let (accepted, publish) = work.admission.cancellation.cancel_with(|| {
            let decision = {
                let mut state = work.state.lock().expect("asynchronous compaction state");
                if state.superseded
                    || state.cancel_requested
                    || work.admission.cancellation.is_cancelled()
                {
                    (false, false)
                } else if matches!(&state.phase, Phase::Ready(_)) {
                    state.cancel_requested = true;
                    state.phase = Phase::Terminal;
                    state.status.phase = CompactionOperationPhase::Cancelled;
                    state.status.waiting_attempt_id = None;
                    state.status.diagnostic = None;
                    (true, true)
                } else if matches!(
                    &state.phase,
                    Phase::Capturing | Phase::Generating | Phase::Installing(_)
                ) {
                    state.cancel_requested = true;
                    (true, false)
                } else {
                    (false, false)
                }
            };
            #[cfg(test)]
            if decision.0 {
                wait_for_arbitration_test_gate(&work.cancellation_request_gate);
            }
            decision
        });
        if publish {
            #[cfg(test)]
            wait_for_arbitration_test_gate(&work.named_cancellation_publication_gate);
            if let Err(error) = self.publish_async_status(&work) {
                self.fail_async_durability(&error);
                return Err(error);
            }
            self.retire_async_compaction(&work);
        }
        work.changed.notify_waiters();
        if accepted {
            self.wake.notify.notify_one();
        }
        Ok(accepted)
    }

    /// Join only an announced physical worker; keep a ready result in its slot.
    pub(super) async fn reap_async_compaction(self: &Arc<Self>, wait: bool) {
        let work = self
            .async_compaction
            .lock()
            .expect("asynchronous compaction registry")
            .as_ref()
            .and_then(|registry| registry.proactive.clone());
        let Some(work) = work else { return };
        if !wait
            && !work.exit_announced.load(Ordering::Acquire)
            && !work.joined.load(Ordering::Acquire)
        {
            return;
        }
        let join = work
            .join
            .lock()
            .expect("asynchronous compaction join")
            .take();
        if let Some(join) = join {
            if join.await.is_err()
                && work.worker_join_failed()
                && let Err(error) = self.publish_async_status(&work)
            {
                self.fail_async_durability(&error);
            }
            work.joined.store(true, Ordering::Release);
            work.lifecycle
                .lock()
                .expect("asynchronous lifecycle")
                .take();
            work.changed.notify_waiters();
        } else if wait {
            // Another reaper or the capture owner may already be completing
            // this exact work. Cancellation alone does not prove its exit.
            loop {
                let changed = work.changed.notified();
                tokio::pin!(changed);
                changed.as_mut().enable();
                if work.joined.load(Ordering::Acquire) {
                    break;
                }
                changed.await;
            }
        } else if !work.joined.load(Ordering::Acquire) {
            // The other reaper still owns confirmation of physical exit.
            return;
        }
        // A Ready operation may be cancelled while this reaper awaits its
        // worker. Read publication debt only after physical join, and retain
        // the registry entry until its current terminal fact is confirmed.
        let publish_terminal = {
            let mut state = work.state.lock().expect("asynchronous compaction state");
            std::mem::take(&mut state.pending_terminal_publication)
        };
        if publish_terminal && let Err(error) = self.publish_async_status(&work) {
            self.fail_async_durability(&error);
        }
        self.retire_async_compaction(&work);
    }

    fn retire_async_compaction(&self, work: &Arc<Work>) {
        let terminal = {
            let state = work.state.lock().expect("asynchronous compaction state");
            (matches!(state.phase, Phase::Terminal)
                || (!self.lifecycle.is_running() && matches!(state.phase, Phase::Failed(_))))
                && !state.pending_terminal_publication
                && state.published.as_ref() == Some(&state.status)
        };
        if terminal && work.joined.load(Ordering::Acquire) {
            let mut slot = self
                .async_compaction
                .lock()
                .expect("asynchronous compaction registry");
            if let Some(registry) = slot.as_mut()
                && registry
                    .proactive
                    .as_ref()
                    .is_some_and(|current| Arc::ptr_eq(current, work))
            {
                registry.proactive = None;
            }
        }
    }

    /// An idle coordinator is a current installation owner. The maintenance
    /// slot serializes this finite check with a new Attempt or manual command.
    #[allow(clippy::too_many_lines)] // The checked-out conversation and every installation settlement share one boundary.
    pub(super) async fn install_ready_async_when_idle(self: &Arc<Self>) {
        let coordinator = RuntimeCompactionCoordinator(Arc::downgrade(self));
        let (ready, mut conversation, mut context, mut request, staged_request_context, _admission) = {
            let mut state = self.lock_state();
            if !self.lifecycle.is_running()
                || self.durability_gate.is_failed()
                || !state.foreground.slot.is_idle()
                || state.foreground.manual_tail.is_unreaped()
            {
                return;
            }
            let Ok(model) = state.config.model.snapshot() else {
                return;
            };
            let resources = state.config.resources.clone();
            let Ok(context) = self.context_runtime_with_assembly(
                &model,
                resources.context_assembly().clone(),
                resources
                    .configuration()
                    .map_or(self.model_timeout_policy, |generation| {
                        generation
                            .config
                            .timeout_policy()
                            .expect("validated generation")
                    }),
                &resources,
                Arc::new(self.auxiliary_executor.clone()),
            ) else {
                return;
            };
            let native = resources.native_system_input();
            let Ok(sections) = resources.context_assembly().system_sections(&native) else {
                return;
            };
            let Some(selection) = RuntimeCompactionCoordinator::take_selected(self, None, true)
            else {
                return;
            };
            let ready = match selection {
                Ok(CompactionDelivery::Ready(ready)) => ready,
                Ok(CompactionDelivery::Failed(_)) => return,
                Err((work, error)) => {
                    // Selection and the absorbing gate share the coordinator
                    // boundary. No foreground retry owns this idle read.
                    let _admission = self.lifecycle.try_enter_running();
                    self.commit_failure_and_observe(
                        DurableOperation::SelectPendingBatch,
                        error.to_string(),
                    );
                    drop(state);
                    if let Err(error) = self.publish_async_status(&work) {
                        self.fail_async_durability(&error);
                    }
                    work.changed.notify_waiters();
                    let _ = self.begin_drain();
                    return;
                }
            };
            let Ok(admission) = self.lifecycle.try_enter_running() else {
                drop(state);
                let _ = coordinator.settle(&ready.admission, CompactionDisposition::Cancelled);
                return;
            };
            let request = crate::model::ModelRequest {
                prompt_fallback_authorization:
                    crate::model::types::PromptFallbackAuthorization::default(),
                response_schema: None,
                response_byte_limit: None,
                images: BTreeMap::default(),
                invocation: model.primary().invocation_config(),
                messages: crate::model::SelectedModelInput::default(),
                tools: resources
                    .capability()
                    .tool_registry()
                    .for_model(model.primary().capabilities())
                    .model_definitions(),
                effective_system_prompt: render_effective_system_prompt(&sections),
                continuation: None,
                continuation_source: None,
            };
            let staged_request_context = [super::compaction::prospective_permission_reminder(
                &ready.admission.operation_id,
                state.config.effective_approval_mode,
            )];
            match state.foreground.slot.hydrate(self.store.as_ref()) {
                Ok(Some(_)) => {}
                outcome => {
                    let diagnostic = outcome.err().map(|error| {
                        let diagnostic =
                            format!("idle compaction could not load the conversation: {error}");
                        self.record_durability_failure(
                            &mut state,
                            DurableOperation::HydrateConversation,
                            diagnostic.clone(),
                        );
                        diagnostic
                    });
                    drop(state);
                    let disposition = if let Some(diagnostic) = diagnostic {
                        let _ = self.begin_drain();
                        CompactionDisposition::Failed(diagnostic)
                    } else {
                        CompactionDisposition::Defer(None)
                    };
                    let _ = coordinator.settle(&ready.admission, disposition);
                    self.settlement.notify_waiters();
                    return;
                }
            }
            let conversation = state
                .foreground
                .slot
                .lend_for_manual_compaction(CurrentManualCompaction {
                    kind: super::foreground::CompactionOwnerKind::BackgroundInstall,
                    operation_id: ready.admission.operation_id.clone(),
                    cancellation: AgentCancellation::new(CancellationReason::RuntimeShutdown),
                })
                .expect("idle coordinator owns the conversation state");
            (
                ready,
                conversation,
                context.with_runtime_resources(&resources),
                request,
                staged_request_context,
                admission,
            )
        };
        let result = async {
            let (history, _) = self
                .store
                .auxiliary_history()
                .map_err(crate::context::compaction::resource_failure)?;
            context
                .engine
                .prepare_media(
                    &history.messages,
                    &history.messages,
                    &ready.admission.cancellation,
                )
                .await?;
            request.messages = history.messages.clone();
            let (images, _) = context
                .engine
                .materialize_selected_media(&mut request.messages, &history.input_manifest)
                .map_err(CompactionExecutionError::Context)?;
            request.images = images;
            // Price the same future request before and after installation.
            // Materialize first so the history still matches its input manifest.
            request.messages = crate::context::engine::assemble_context_input(
                &request.messages,
                &staged_request_context,
                None,
                None,
            )
            .map_err(|error| ContextError::new(ContextErrorKind::MalformedHistory, error))?;
            let constraints = crate::context::engine::CompactionConstraints {
                staged_request_context: &staged_request_context,
                ..crate::context::engine::CompactionConstraints::default()
            };
            crate::context::compaction::activate_generated(
                &ready.generated,
                &mut conversation,
                self.store.as_ref(),
                &ready.admission.cancellation,
                &crate::context::compaction::CurrentInstallation {
                    context: &context,
                    request: &request,
                    constraints: &constraints,
                    attempt_id: None,
                    turn_id: None,
                    installer_cancellation: None,
                },
            )
        }
        .await;
        let activated = match &result {
            Ok(ActivationOutcome::Committed(completed))
            | Err(CompactionExecutionError::Committed { completed, .. }) => {
                completed.newly_committed
            }
            _ => false,
        };
        let disposition = match result {
            Ok(ActivationOutcome::Committed(completed)) => {
                if completed.newly_committed {
                    self.publish_async_compaction_receipt(&completed.persisted_event);
                }
                CompactionDisposition::Committed
            }
            Ok(ActivationOutcome::Superseded) => CompactionDisposition::Superseded,
            Ok(ActivationOutcome::Incomparable) => CompactionDisposition::Incomparable,
            Ok(ActivationOutcome::Inadmissible(error)) => {
                CompactionDisposition::Defer(Some(error.message))
            }
            Err(CompactionExecutionError::HeadChanged) => CompactionDisposition::Defer(None),
            Err(CompactionExecutionError::Context(error))
                if error.kind == ContextErrorKind::Cancelled =>
            {
                CompactionDisposition::Cancelled
            }
            Err(CompactionExecutionError::Committed { completed, error }) => {
                if completed.newly_committed {
                    self.publish_async_compaction_receipt(&completed.persisted_event);
                }
                self.fail_async_native(
                    DurableOperation::CanonicalCommit,
                    format!("committed idle compaction could not be installed: {error}"),
                );
                CompactionDisposition::Committed
            }
            Err(CompactionExecutionError::Context(error))
                if error.kind != ContextErrorKind::AuxiliaryDurabilityFailed =>
            {
                CompactionDisposition::Failed(error.message)
            }
            Err(error) => {
                let diagnostic = format!("idle compaction installation failed: {error:?}");
                self.fail_async_native(DurableOperation::CanonicalCommit, diagnostic.clone());
                CompactionDisposition::Failed(diagnostic)
            }
        };
        if let Err(error) = coordinator.settle(&ready.admission, disposition) {
            tracing::error!(diagnostic = %error.message, "idle compaction status settlement failed");
        }
        let mut state = self.lock_state();
        let owner = state
            .foreground
            .slot
            .finish_manual_compaction(conversation)
            .expect("background installation owns the foreground maintenance slot");
        assert!(owner.kind == super::foreground::CompactionOwnerKind::BackgroundInstall);
        assert_eq!(owner.operation_id, ready.admission.operation_id);
        if activated {
            state.config.last_permission_reminder_mode = None;
        }
        drop(state);
        self.settlement.notify_waiters();
    }
}

impl ConversationRuntime {
    /// Request cancellation of one exact Session-owned proactive compaction.
    /// `true` acknowledges the request, not physical completion; existing
    /// native supervision retains its scope until actual settlement. A commit
    /// granted before cancellation remains authoritative. Unknown or settled
    /// operation IDs return `false` and do not affect Attempts or maintenance.
    ///
    /// # Errors
    /// A required cancellation-status write could not be persisted.
    pub fn cancel_compaction(
        &self,
        operation_id: &crate::runtime::identity::CompactionOperationId,
    ) -> Result<bool, ContextError> {
        self.inner.cancel_named_async_compaction(operation_id)
    }
}

impl RuntimeCompactionCoordinator {
    fn take_selected(
        inner: &RuntimeInner,
        admission: Option<&CompactionAdmission>,
        idle: bool,
    ) -> Option<Result<CompactionDelivery, (Arc<Work>, crate::runtime::inbound::MailboxError)>>
    {
        let work = inner
            .async_compaction
            .lock()
            .expect("asynchronous compaction registry")
            .as_ref()?
            .proactive
            .clone()?;
        if admission.is_some_and(|selected| {
            selected.operation_id != work.admission.operation_id
                || selected.generation != work.admission.generation
        }) {
            return None;
        }
        let mut state = work.state.lock().expect("asynchronous compaction state");
        // A worker's status write can overlap this boundary. Never select a
        // Ready or Failed state until its exact status is durably published;
        // this check is local and never waits for the publication mutex.
        if state.published.as_ref() != Some(&state.status) {
            return None;
        }
        match &state.phase {
            Phase::Ready(generated) if state.status.phase == CompactionOperationPhase::Ready => {
                let _pending = if idle {
                    // Serialize the empty-inbox check and the idle claim with
                    // acceptance, including producers outside the coordinator.
                    let pending = inner.mailbox.pending_publication();
                    match inner.mailbox.has_pending() {
                        Ok(true) => return None,
                        Err(error) => {
                            state.phase = Phase::Terminal;
                            state.status.phase = CompactionOperationPhase::Failed;
                            state.status.waiting_attempt_id = None;
                            state.status.diagnostic = Some(
                                format!("idle compaction could not check pending input: {error}")
                                    .chars()
                                    .take(256)
                                    .collect(),
                            );
                            return Some(Err((work.clone(), error)));
                        }
                        Ok(false) => {}
                    }
                    Some(pending)
                } else {
                    None
                };
                let generated = generated.clone();
                state.phase = Phase::Installing(generated.clone());
                Some(Ok(CompactionDelivery::Ready(ReadyCompaction {
                    admission: work.admission.clone(),
                    generated,
                })))
            }
            Phase::Failed(error) if state.status.phase == CompactionOperationPhase::Failed => {
                let error = error.clone();
                state.phase = Phase::Terminal;
                inner.wake.notify.notify_one();
                Some(Ok(CompactionDelivery::Failed(error)))
            }
            _ => None,
        }
    }
}

impl CompactionCoordinator for RuntimeCompactionCoordinator {
    fn admit(
        &self,
        trigger: &AttemptId,
    ) -> Result<Option<CompactionAdmission>, CompactionExecutionError> {
        let Some(inner) = self.0.upgrade() else {
            return Ok(None);
        };
        let coordinator = inner.lock_state();
        if !inner.lifecycle.is_running() || inner.durability_gate.is_failed() {
            return Ok(None);
        }
        let mut slot = inner
            .async_compaction
            .lock()
            .expect("asynchronous compaction registry");
        if slot.is_none() {
            let cut = inner
                .store
                .compaction_resource_cut()
                .map_err(|error| CompactionExecutionError::Durable(error.to_string()))?;
            let next_generation = inner
                .store
                .latest_compaction_operations(cut.journal.0)
                .map_err(|error| CompactionExecutionError::Durable(error.to_string()))?
                .into_iter()
                .map(|status| status.admission_generation)
                .max()
                .unwrap_or(0);
            *slot = Some(Registry {
                next_generation,
                proactive: None,
            });
        }
        let registry = slot.as_mut().expect("registry was initialized");
        if registry.proactive.is_some() {
            return Ok(None);
        }
        let lifecycle = inner.lifecycle.try_enter_running().map_err(|_| {
            ContextError::new(
                ContextErrorKind::AuxiliaryScopeClosed,
                "Session is not accepting compaction",
            )
        })?;
        registry.next_generation = registry.next_generation.checked_add(1).ok_or_else(|| {
            ContextError::new(
                ContextErrorKind::Internal,
                "compaction admission generation exhausted",
            )
        })?;
        let admission = CompactionAdmission {
            operation_id: crate::runtime::identity::CompactionOperationId::new(
                uuid::Uuid::now_v7().to_string(),
            ),
            generation: registry.next_generation,
            cancellation: CancellationSignal::new(),
        };
        let status = CompactionOperationStatus {
            operation_id: admission.operation_id.clone(),
            admission_generation: admission.generation,
            triggering_attempt_id: Some(trigger.clone()),
            waiting_attempt_id: None,
            phase: CompactionOperationPhase::Generating,
            accepted_fragments: 0,
            diagnostic: None,
        };
        inner.publish_compaction_admission(status.clone())?;
        registry.proactive = Some(Arc::new(Work {
            admission: admission.clone(),
            state: Mutex::new(WorkState {
                published: Some(status.clone()),
                status,
                phase: Phase::Capturing,
                retired_originals: Vec::new(),
                pending_terminal_publication: false,
                planned_coverage: None,
                superseded: false,
                cancel_requested: false,
                retrying: false,
                #[cfg(test)]
                force_blocking_competition: false,
            }),
            publication: Mutex::new(()),
            #[cfg(test)]
            ready_publication_gate: Mutex::new(None),
            #[cfg(test)]
            defer_settlement_gate: Mutex::new(None),
            #[cfg(test)]
            generation_settlement_gate: Mutex::new(None),
            #[cfg(test)]
            cancellation_request_gate: Mutex::new(None),
            #[cfg(test)]
            named_cancellation_publication_gate: Mutex::new(None),
            changed: tokio::sync::Notify::new(),
            join: Mutex::new(None),
            exit_announced: AtomicBool::new(false),
            joined: AtomicBool::new(false),
            lifecycle: Mutex::new(Some(lifecycle)),
        }));
        drop(slot);
        drop(coordinator);
        Ok(Some(admission))
    }

    #[allow(clippy::too_many_lines)] // One owned worker contains generation, observation and terminal settlement.
    fn start(
        &self,
        admission: CompactionAdmission,
        captured: CapturedCompaction,
        operation: Box<dyn crate::context::summarizer::SummaryOperation>,
    ) -> Result<(), CompactionExecutionError> {
        let inner = self.0.upgrade().ok_or_else(|| {
            ContextError::new(
                ContextErrorKind::Cancelled,
                "Session no longer owns compaction",
            )
        })?;
        let work = inner.async_work(&admission).ok_or_else(|| {
            ContextError::new(ContextErrorKind::Cancelled, "compaction no longer exists")
        })?;
        let retired_originals = captured
            .source_history()
            .messages
            .iter()
            .take(captured.retired_count())
            .filter_map(|message| message.canonical_id().cloned())
            .collect();
        let planned_coverage = PlannedCoverage::from_capture(&captured);
        let started = admission.cancellation.with_active(|| {
            let mut state = work.state.lock().expect("asynchronous compaction state");
            if !inner.lifecycle.is_running() || !matches!(state.phase, Phase::Capturing) {
                return false;
            }
            state.retired_originals = retired_originals;
            state.planned_coverage = planned_coverage;
            state.phase = Phase::Generating;
            true
        });
        if started != Some(true) {
            return Err(ContextError::new(
                ContextErrorKind::Cancelled,
                "compaction capture is no longer eligible to start",
            )
            .into());
        }
        // A foreground writer may have committed while capture was running.
        // Recheck after the planned source coordinate is available and before
        // the worker can start another physical Summary request.
        self.head_changed();
        let owner = inner.clone();
        let task_work = work.clone();
        let (release, registered) = tokio::sync::oneshot::channel();
        let exit = WorkerExit {
            work: work.clone(),
            wake: inner.wake.clone(),
        };
        let join = inner.executor.spawn(async move {
            if registered.await.is_err() { return; }
            let _exit = exit;
            let progress_owner = owner.clone();
            let progress_work = task_work.clone();
            let progress = move |event: crate::events::types::RuntimeEventEnvelope| -> Result<(), CompactionExecutionError> {
                if progress_owner.async_work(&progress_work.admission).is_none() {
                    return Err(ContextError::new(ContextErrorKind::Cancelled, "compaction operation was superseded").into());
                }
                let persisted = progress_owner.store.append_event(event).map_err(|error| CompactionExecutionError::Durable(error.to_string()))?;
                progress_owner.publish_async_compaction_receipt(&persisted);
                if let RuntimeEvent::CompactionChunkAccepted { accepted_fragments, .. } = persisted.event {
                    {
                        let mut state = progress_work.state.lock().expect("asynchronous compaction state");
                        state.status.accepted_fragments = accepted_fragments;
                        state.retrying = false;
                        if state.status.phase == CompactionOperationPhase::Retrying {
                            state.status.phase = CompactionOperationPhase::Generating;
                        }
                    }
                    progress_owner.publish_async_status(&progress_work)?;
                }
                RuntimeCompactionCoordinator(Arc::downgrade(&progress_owner)).head_changed();
                if let Some(failure) = progress_owner.durability_gate.admission_failure() {
                    return Err(ContextError::new(ContextErrorKind::AuxiliaryDurabilityFailed, failure.diagnostic).into());
                }
                if progress_work.admission.cancellation.is_cancelled() {
                    return Err(ContextError::new(ContextErrorKind::Cancelled, "compaction source was superseded or cancelled").into());
                }
                Ok(())
            };
            let recovery_owner = Arc::downgrade(&owner);
            let recovery_work = Arc::downgrade(&task_work);
            let recovery_admission = admission.clone();
            let recovery = operation.budget().authority().on_recovery(Arc::new(move || {
                let owner = recovery_owner.upgrade().ok_or_else(|| ContextError::new(ContextErrorKind::Cancelled, "Session no longer owns compaction"))?;
                let work = recovery_work.upgrade().ok_or_else(|| ContextError::new(ContextErrorKind::Cancelled, "compaction no longer exists"))?;
                if owner.async_work(&recovery_admission).is_none() {
                    return Err(ContextError::new(ContextErrorKind::Cancelled, "compaction was superseded"));
                }
                RuntimeCompactionCoordinator(Arc::downgrade(&owner)).head_changed();
                if let Some(failure) = owner.durability_gate.admission_failure() {
                    return Err(ContextError::new(ContextErrorKind::AuxiliaryDurabilityFailed, failure.diagnostic));
                }
                let publish = {
                    let mut state = work.state.lock().expect("asynchronous compaction state");
                    if state.superseded || !matches!(state.phase, Phase::Generating) {
                        return Err(ContextError::new(ContextErrorKind::Cancelled, "compaction is no longer generating"));
                    }
                    state.retrying = true;
                    if state.status.phase == CompactionOperationPhase::ForegroundWaiting {
                        false
                    } else {
                        state.status.phase = CompactionOperationPhase::Retrying;
                        true
                    }
                };
                if publish { owner.publish_async_status(&work)?; }
                Ok(())
            }));
            let before_owner = Arc::downgrade(&owner);
            let before_admission = admission.clone();
            let before_call: Arc<dyn Fn() -> Result<(), ContextError> + Send + Sync> = Arc::new(move || {
                let before_owner = before_owner.upgrade().ok_or_else(|| ContextError::new(ContextErrorKind::Cancelled, "Session no longer owns compaction"))?;
                if before_owner.async_work(&before_admission).is_none() {
                    return Err(ContextError::new(ContextErrorKind::Cancelled, "compaction is no longer current"));
                }
                RuntimeCompactionCoordinator(Arc::downgrade(&before_owner)).head_changed();
                if let Some(failure) = before_owner.durability_gate.admission_failure() {
                    return Err(ContextError::new(ContextErrorKind::AuxiliaryDurabilityFailed, failure.diagnostic));
                }
                if before_admission.cancellation.is_cancelled() {
                    return Err(ContextError::new(ContextErrorKind::Cancelled, "compaction source was superseded or cancelled"));
                }
                Ok(())
            });
            let dispatch_check = operation.budget().authority().on_dispatch_check(before_call.clone());
            let result = match (recovery, dispatch_check) {
                (Ok(()), Ok(())) => crate::context::compaction::generate_compaction(captured, owner.store.as_ref(), operation.as_ref(), &admission.cancellation, &progress, before_call.as_ref()).await,
                (Err(error), _) | (_, Err(error)) => Err(CompactionExecutionError::Context(error)),
            };
            operation.close().await;
            if owner.async_work(&admission).is_none() { return; }
            #[cfg(test)]
            wait_for_arbitration_test_gate(&task_work.generation_settlement_gate);
            let mut durability_failure = None;
            {
                let mut state = task_work.state.lock().expect("asynchronous compaction state");
                match result {
                    Ok(generated) if !state.superseded && !state.cancel_requested && !admission.cancellation.is_cancelled() => {
                        state.status.waiting_attempt_id = None;
                        state.phase = Phase::Ready(Arc::new(generated));
                        state.status.phase = CompactionOperationPhase::Ready;
                    }
                    Ok(_) => {
                        state.phase = Phase::Terminal;
                        state.status.waiting_attempt_id = None;
                        state.status.phase = if state.superseded { CompactionOperationPhase::Superseded } else { CompactionOperationPhase::Cancelled };
                    }
                    Err(error) => {
                        let context = match error {
                            CompactionExecutionError::Context(error) => error,
                            other => ContextError::new(ContextErrorKind::AuxiliaryDurabilityFailed, format!("{other:?}")),
                        };
                        if context.kind == ContextErrorKind::AuxiliaryDurabilityFailed {
                            durability_failure = Some(context.clone());
                        }
                        state.status.waiting_attempt_id = None;
                        state.status.phase = if state.superseded {
                            CompactionOperationPhase::Superseded
                        } else if state.cancel_requested || admission.cancellation.is_cancelled() || context.kind == ContextErrorKind::Cancelled {
                            CompactionOperationPhase::Cancelled
                        } else if context.kind == ContextErrorKind::NoEligibleHistory {
                            CompactionOperationPhase::NoEffect
                        } else {
                            CompactionOperationPhase::Failed
                        };
                        state.status.diagnostic = (state.status.phase == CompactionOperationPhase::Failed)
                            .then(|| context.message.chars().take(256).collect());
                        state.phase = if state.status.phase == CompactionOperationPhase::Failed {
                            Phase::Failed(context)
                        } else { Phase::Terminal };
                    }
                }
            }
            if let Some(error) = durability_failure { owner.fail_async_durability(&error); }
            if let Err(error) = owner.publish_async_status(&task_work) { owner.fail_async_durability(&error); }
            task_work.changed.notify_waiters();
            owner.wake.notify.notify_one();
        });
        *work.join.lock().expect("asynchronous compaction join") = Some(join);
        let _ = release.send(());
        Ok(())
    }

    fn fail_admission(
        &self,
        admission: &CompactionAdmission,
        error: &CompactionExecutionError,
    ) -> Result<(), ContextError> {
        let inner = self.0.upgrade().ok_or_else(|| {
            ContextError::new(
                ContextErrorKind::Cancelled,
                "Session no longer owns compaction",
            )
        })?;
        let work = inner.async_work(admission).ok_or_else(|| {
            ContextError::new(ContextErrorKind::Cancelled, "compaction no longer exists")
        })?;
        {
            let mut state = work.state.lock().expect("asynchronous compaction state");
            let kind = match error {
                CompactionExecutionError::Context(context) => Some(context.kind),
                _ => None,
            };
            state.status.phase = if state.cancel_requested
                || admission.cancellation.is_cancelled()
                || kind == Some(ContextErrorKind::Cancelled)
            {
                CompactionOperationPhase::Cancelled
            } else if kind == Some(ContextErrorKind::NoEligibleHistory) {
                CompactionOperationPhase::NoEffect
            } else {
                CompactionOperationPhase::Failed
            };
            state.status.waiting_attempt_id = None;
            state.status.diagnostic = (state.status.phase == CompactionOperationPhase::Failed)
                .then(|| format!("{error:?}").chars().take(256).collect());
            state.phase = Phase::Terminal;
        }
        match error {
            CompactionExecutionError::Context(context)
                if context.kind == ContextErrorKind::AuxiliaryDurabilityFailed =>
            {
                inner.fail_async_durability(context);
            }
            CompactionExecutionError::Durable(diagnostic) => {
                inner.fail_async_native(DurableOperation::CanonicalCommit, diagnostic.clone());
            }
            _ => {}
        }
        let publication = inner.publish_async_status(&work);
        if let Err(error) = &publication {
            inner.fail_async_durability(error);
        }
        work.joined.store(true, Ordering::Release);
        work.lifecycle
            .lock()
            .expect("asynchronous lifecycle")
            .take();
        work.changed.notify_waiters();
        inner.wake.notify.notify_one();
        publication
    }

    fn take_ready(&self) -> Option<CompactionDelivery> {
        let inner = self.0.upgrade()?;
        Self::take_selected(&inner, None, false).map(|selection| {
            selection
                .unwrap_or_else(|_| unreachable!("foreground selection does not read the inbox"))
        })
    }

    fn active(&self) -> Option<CompactionAdmission> {
        let inner = self.0.upgrade()?;
        let work = inner
            .async_compaction
            .lock()
            .expect("asynchronous compaction registry")
            .as_ref()?
            .proactive
            .clone()?;
        let active = matches!(
            work.state
                .lock()
                .expect("asynchronous compaction state")
                .phase,
            Phase::Capturing | Phase::Generating | Phase::Ready(_)
        );
        active.then(|| work.admission.clone())
    }

    fn can_join(
        &self,
        admission: &CompactionAdmission,
        context: &crate::context::ContextRuntime,
        request: &crate::model::ModelRequest,
        constraints: &crate::context::engine::CompactionConstraints<'_>,
    ) -> Result<bool, CompactionExecutionError> {
        let Some(inner) = self.0.upgrade() else {
            return Ok(false);
        };
        let Some(work) = inner.async_work(admission) else {
            return Ok(false);
        };
        let planned = {
            let state = work.state.lock().expect("asynchronous compaction state");
            #[cfg(test)]
            if state.force_blocking_competition {
                return Ok(false);
            }
            if matches!(
                state.phase,
                Phase::Capturing | Phase::Terminal | Phase::Failed(_)
            ) || constraints
                .must_cover_through
                .is_some_and(|id| !state.retired_originals.contains(id))
            {
                return Ok(false);
            }
            state.planned_coverage.clone()
        };
        let Some(planned) = planned else {
            return Ok(true);
        };
        let (history, cut) = inner
            .store
            .auxiliary_history()
            .map_err(|error| CompactionExecutionError::Durable(error.to_string()))?;
        let head = inner
            .store
            .projection_at_journal_cut(cut.journal.0)
            .map_err(|error| CompactionExecutionError::Durable(error.to_string()))?;
        let Some(offset) = planned.retained_offset(&inner.conversation_id, head.as_ref()) else {
            return Ok(false);
        };
        if offset > history.messages.len() {
            return Ok(false);
        }
        let retired = history.messages.iter().take(offset);
        let protected: std::collections::BTreeSet<_> = constraints
            .protected_context_ids
            .iter()
            .chain(
                constraints
                    .fresh_inbound
                    .into_iter()
                    .flat_map(|fresh| fresh.message_ids().iter()),
            )
            .collect();
        if retired
            .filter_map(|message| message.canonical_id())
            .any(|id| protected.contains(id))
        {
            return Ok(false);
        }
        let engine = &context.engine;
        if !engine.supports_summary_floor()
            || !engine.supports_monotone_compaction(&history.messages)
        {
            // Without a lower-bound proof this remains potentially useful.
            return Ok(true);
        }
        let mut optimistic = crate::model::SelectedModelInput::default();
        optimistic.push(crate::context::planning::minimum_summary());
        for message in history.messages.iter().skip(offset) {
            optimistic.push(message.clone());
        }
        // Mandatory recovery can degrade request-only carryover completely;
        // omit it for a conservative lower bound, but retain all fixed inputs.
        let assembled = crate::context::engine::assemble_context_input(
            &optimistic,
            constraints.staged_request_context,
            None,
            None,
        )
        .map_err(|error| ContextError::new(ContextErrorKind::MalformedHistory, error))?;
        let mut optimistic_request = request.clone();
        optimistic_request.messages = engine
            .rendered_input(&assembled)
            .map_err(|error| ContextError::new(ContextErrorKind::MalformedHistory, error))?;
        optimistic_request.continuation = None;
        optimistic_request.continuation_source = None;
        let mut limit =
            engine.admissible_input_limit(context.compaction_budgets.primary_output_budget)?;
        if let Some(correction) = constraints.estimate_correction {
            limit = correction.apply(limit);
        }
        Ok(engine.estimate_frozen_request(&optimistic_request) <= limit)
    }

    fn wait<'a>(
        &'a self,
        admission: &'a CompactionAdmission,
        attempt: &'a AttemptId,
        cancellation: &'a CancellationSignal,
    ) -> BoxFuture<'a, Option<CompactionDelivery>> {
        Box::pin(async move {
            let inner = self.0.upgrade()?;
            let work = inner.async_work(admission)?;
            let publish_wait = {
                let mut state = work.state.lock().expect("asynchronous compaction state");
                if matches!(state.phase, Phase::Capturing | Phase::Generating) {
                    state.status.waiting_attempt_id = Some(attempt.clone());
                    state.status.phase = CompactionOperationPhase::ForegroundWaiting;
                    true
                } else {
                    false
                }
            };
            if publish_wait && let Err(error) = inner.publish_async_status(&work) {
                inner.fail_async_durability(&error);
                return None;
            }
            let _waiter = WaitingAttempt {
                owner: Arc::downgrade(&inner),
                work: work.clone(),
                attempt: attempt.clone(),
            };
            loop {
                let changed = work.changed.notified();
                tokio::pin!(changed);
                changed.as_mut().enable();
                if let Some(delivery) = Self::take_selected(&inner, Some(admission), false) {
                    return Some(delivery.unwrap_or_else(|_| {
                        unreachable!("foreground selection does not read the inbox")
                    }));
                }
                let terminal = matches!(
                    work.state
                        .lock()
                        .expect("asynchronous compaction state")
                        .phase,
                    Phase::Terminal
                );
                if cancellation.is_cancelled() || terminal || inner.async_work(admission).is_none()
                {
                    break;
                }
                tokio::select! { () = &mut changed => {}, () = cancellation.cancelled() => break }
            }
            None
        })
    }

    fn settle(
        &self,
        admission: &CompactionAdmission,
        outcome: CompactionDisposition,
    ) -> Result<(), ContextError> {
        let inner = self.0.upgrade().ok_or_else(|| {
            ContextError::new(
                ContextErrorKind::Cancelled,
                "Session no longer owns compaction",
            )
        })?;
        let work = inner.async_work(admission).ok_or_else(|| {
            ContextError::new(ContextErrorKind::Cancelled, "compaction no longer exists")
        })?;
        let deferred = {
            #[cfg(test)]
            if matches!(&outcome, CompactionDisposition::Defer(_)) {
                wait_for_arbitration_test_gate(&work.defer_settlement_gate);
            }
            let mut state = work.state.lock().expect("asynchronous compaction state");
            let Phase::Installing(generated) = &state.phase else {
                return Err(ContextError::new(
                    ContextErrorKind::AuxiliaryConflict,
                    "compaction result is not owned by this installer",
                ));
            };
            let generated = generated.clone();
            // Cancellation can publish its owner flag before its token. Read
            // both under the same lock that returns Installing work to Ready.
            let deferred = matches!(&outcome, CompactionDisposition::Defer(_))
                && !state.superseded
                && !state.cancel_requested
                && !work.admission.cancellation.is_cancelled();
            state.status.waiting_attempt_id = None;
            match outcome {
                CompactionDisposition::Defer(diagnostic) if deferred => {
                    state.phase = Phase::Ready(generated);
                    state.status.phase = CompactionOperationPhase::Ready;
                    state.status.diagnostic =
                        diagnostic.map(|value| value.chars().take(256).collect());
                }
                CompactionDisposition::Defer(_) => {
                    state.phase = Phase::Terminal;
                    state.status.phase = if state.superseded {
                        CompactionOperationPhase::Superseded
                    } else {
                        CompactionOperationPhase::Cancelled
                    };
                    state.status.diagnostic = None;
                }
                CompactionDisposition::Committed => {
                    state.phase = Phase::Terminal;
                    state.status.phase = CompactionOperationPhase::Committed;
                    state.status.diagnostic = None;
                }
                CompactionDisposition::Superseded => {
                    state.phase = Phase::Terminal;
                    state.status.phase = CompactionOperationPhase::Superseded;
                    state.status.diagnostic = None;
                }
                CompactionDisposition::Incomparable => {
                    state.phase = Phase::Terminal;
                    state.status.phase = CompactionOperationPhase::NoEffect;
                    state.status.diagnostic = None;
                }
                CompactionDisposition::Cancelled => {
                    state.phase = Phase::Terminal;
                    state.status.phase = CompactionOperationPhase::Cancelled;
                    state.status.diagnostic = None;
                }
                CompactionDisposition::Failed(error) => {
                    state.phase = Phase::Terminal;
                    state.status.phase = CompactionOperationPhase::Failed;
                    state.status.diagnostic = Some(error.chars().take(256).collect());
                }
            }
            deferred
        };
        if let Err(error) = inner.publish_async_status(&work) {
            inner.fail_async_durability(&error);
            return Err(error);
        }
        work.changed.notify_waiters();
        if !deferred {
            inner.wake.notify.notify_one();
        }
        Ok(())
    }

    fn head_changed(&self) {
        let Some(inner) = self.0.upgrade() else {
            return;
        };
        let work = inner
            .async_compaction
            .lock()
            .expect("asynchronous compaction registry")
            .as_ref()
            .and_then(|registry| registry.proactive.clone());
        let Some(work) = work else { return };
        let head = match inner.store.projection_head() {
            Ok(Some(head)) => head,
            Ok(None) => return,
            Err(error) => {
                inner.fail_async_native(
                    DurableOperation::CanonicalCommit,
                    format!("compaction supersession could not read the current head: {error}"),
                );
                return;
            }
        };
        let (cancel, publish) = {
            let mut state = work.state.lock().expect("asynchronous compaction state");
            if state.cancel_requested || work.admission.cancellation.is_cancelled() {
                return;
            }
            let Some(planned) = &state.planned_coverage else {
                return;
            };
            if !head
                .candidate
                .prefix_coverage
                .as_ref()
                .is_some_and(|active| {
                    planned.root == active.root && planned.retired_through <= active.retired_through
                })
            {
                return;
            }
            if matches!(state.phase, Phase::Generating | Phase::Installing(_)) {
                state.superseded = true;
                (true, false)
            } else if matches!(state.phase, Phase::Ready(_)) {
                state.superseded = true;
                state.phase = Phase::Terminal;
                state.status.phase = CompactionOperationPhase::Superseded;
                state.status.waiting_attempt_id = None;
                state.status.diagnostic = None;
                (true, true)
            } else {
                (false, false)
            }
        };
        if cancel {
            work.admission.cancellation.cancel();
            work.changed.notify_waiters();
        }
        if publish {
            if let Err(error) = inner.publish_async_status(&work) {
                inner.fail_async_durability(&error);
            }
            inner.wake.notify.notify_one();
        }
    }
}

#[cfg(test)]
mod tests;
