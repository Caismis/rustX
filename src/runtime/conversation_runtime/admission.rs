//! The one admission owner: inbound acceptance, idle observation, the
//! admission worker, and the admission linearization that publishes the next
//! Attempt.

use super::*;

/// The runtime admission worker's wake boundary.
///
/// A leaf: one `Notify` plus a closed flag, owned by the worker task and the
/// runtime. The mailbox observer wakes it on every enqueue, so an idle
/// conversation admits asynchronous inbound without any client request.
pub(super) struct WakeGate {
    /// The wake signal.
    pub(super) notify: tokio::sync::Notify,
    /// Set by explicit drain or by the runtime's `Drop`. Terminal for the
    /// worker.
    pub(super) closed: AtomicBool,
    /// Set by the worker after it has observed the closed gate and exited.
    pub(super) exited: AtomicBool,
    /// Wakes a drain waiter after worker exit.
    pub(super) exit_notify: tokio::sync::Notify,
    /// Asks the worker to re-derive configuration-adoption eligibility only.
    /// It never admits work: it covers coordinator-owned eligibility inputs
    /// that change without a lifecycle announcement.
    pub(super) eligibility: tokio::sync::Notify,
    /// Test-only worker-exit signal.
    #[cfg(test)]
    pub(super) worker_exit: Mutex<Option<std::sync::mpsc::Sender<()>>>,
}

impl WakeGate {
    pub(super) fn new() -> Self {
        Self {
            notify: tokio::sync::Notify::new(),
            closed: AtomicBool::new(false),
            exited: AtomicBool::new(false),
            exit_notify: tokio::sync::Notify::new(),
            eligibility: tokio::sync::Notify::new(),
            #[cfg(test)]
            worker_exit: Mutex::new(None),
        }
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    pub(super) fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.notify.notify_one();
    }

    pub(super) fn mark_exited(&self) {
        self.exited.store(true, Ordering::Release);
        self.exit_notify.notify_waiters();
    }

    pub(super) async fn wait_until_exited(&self) {
        loop {
            if self.exited.load(Ordering::Acquire) {
                return;
            }
            let notified = self.exit_notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.exited.load(Ordering::Acquire) {
                return;
            }
            notified.await;
        }
    }

    /// Fires the test-only worker-exit signal, once.
    #[cfg(test)]
    pub(crate) fn signal_worker_exit(&self) {
        if let Some(sender) = self
            .worker_exit
            .lock()
            .expect("worker exit probe lock poisoned")
            .take()
        {
            let _ = sender.send(());
        }
    }
}

/// Conservative native idle refusal; no App Server copy of execution state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IdleBusyReason {
    LifecycleOwner,
    Foreground,
    Recovery,
    Durability,
    AutonomousExtension,
    Inbound,
    Interaction,
    Background,
    Subagent,
    AdmissionChanged,
}

/// Which owners one evaluation of the native idle rule consults.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum IdleProbe {
    /// Every owner, including the durable pending-inbox and Goal probes.
    Durable,
    /// Only in-memory owners; never reads the durable store.
    InMemory,
}

/// The accepted identity of one submitted inbound message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboundAdmission {
    /// The runtime-assigned message identity.
    pub message_id: MessageId,
    /// The mailbox-assigned inbound sequence.
    pub inbound_sequence: InboundSequence,
}

/// An inbound publish failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InboundAdmissionError {
    /// The runtime was not activated: an inert conversation accepts no
    /// inbound work.
    Inactive,
    /// The conversation's parent-authored guidance admission is sealed
    /// (Issue #193): its terminal linearization point already committed, so
    /// no further semantic input can reach an Agent Loop boundary.
    GuidanceSealed,
    /// Runtime drain has begun: no further inbound admission occurs.
    Shutdown,
    /// Inbound content must not be empty.
    EmptyContent,
    /// The runtime's durable authority failed persistently: no new inbound
    /// work may be accepted until the runtime is reconstructed.
    DurabilityFailed {
        /// The human-readable failure diagnostic.
        message: String,
    },
    /// The Session's published Model Catalog does not admit its configured
    /// selection: no work is accepted until a valid selection is committed.
    ModelUnavailable {
        /// Why the selection is unavailable.
        diagnostic: String,
    },
    /// The authoritative mailbox rejected the message.
    Mailbox(MailboxError),
}

impl core::fmt::Display for InboundAdmissionError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Inactive => f.write_str("the conversation runtime is not activated"),
            Self::GuidanceSealed => {
                f.write_str("the child conversation already committed its terminal seal")
            }
            Self::Shutdown => f.write_str("the conversation runtime is shutting down"),
            Self::EmptyContent => f.write_str("inbound content must not be empty"),
            Self::DurabilityFailed { message } => write!(
                f,
                "the conversation runtime durability authority failed: {message}"
            ),
            Self::ModelUnavailable { diagnostic } => {
                write!(
                    f,
                    "the configured session model is unavailable: {diagnostic}"
                )
            }
            Self::Mailbox(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for InboundAdmissionError {}

/// The semantic class of one trusted in-process inbound submission.
///
/// The class exists only to select the extra admission gates that one class
/// carries; both classes share the same durable acceptance linearization
/// point, the same lifecycle and durability gates, and the same ordinary
/// Agent Loop consumption. There is deliberately no second inbound path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SemanticInboundClass {
    /// Every ordinary producer, including the child's own delegation.
    Ordinary,
    /// Parent-authored in-flight guidance for a running child (Issue #193),
    /// which additionally linearizes against the child conversation's
    /// terminal seal and committed cancellation intent.
    ParentGuidance,
}

impl SemanticInboundClass {
    pub(super) const fn is_parent_guidance(self) -> bool {
        matches!(self, Self::ParentGuidance)
    }
}

impl RuntimeInner {
    /// Begins or joins the one semantic runtime drain.
    ///
    /// The coordinator lock serializes the lifecycle transition with inbound
    /// acceptance, model updates, and attempt admission. The shared
    /// lifecycle admission guards serialize it with background ownership and
    /// capability commits that have their own native synchronization owner.
    pub(super) fn idle_epoch_locked(
        &self,
        state: &CoordinatorState,
    ) -> Result<u64, IdleBusyReason> {
        self.idle_epoch_probed(state, IdleProbe::Durable)
    }

    /// The one native idle rule. `IdleProbe::Durable` is exact and is what
    /// residency and the adoption gate use. `IdleProbe::InMemory` omits the two
    /// durable-store probes — pending inbox and Goal continuation authority —
    /// for the advisory eligibility publisher, which must never become a
    /// second consumer of durable reads: whenever either holds while the
    /// runtime is otherwise idle, the admission pass that follows admits an
    /// attempt, and that admission is itself an announced busy owner.
    pub(super) fn idle_epoch_probed(
        &self,
        state: &CoordinatorState,
        probe: IdleProbe,
    ) -> Result<u64, IdleBusyReason> {
        use IdleBusyReason as Busy;
        let epoch = self.lifecycle.idle_epoch().ok_or(Busy::LifecycleOwner)?;
        if (state.current_attempt.is_some() || state.scheduled.is_some())
            || state.manual_compaction.is_some()
            || self
                .manual_tail
                .lock()
                .expect("manual tail owner")
                .is_some()
        {
            return Err(Busy::Foreground);
        }
        // Work that waits for admission owns nothing while the published
        // catalog does not admit the configured selection: admission is closed
        // until a valid selection is committed, and committing one is exactly
        // the idle mutation this rule must leave open.
        let admissible = state.model.unavailable().is_none();
        if admissible && state.recovered_continuation.is_some() {
            return Err(Busy::Recovery);
        }
        if self.durability_gate.is_failed()
            || state.mcp_settlement_failure.is_some()
            || state.admission_durability_cycle.pending_retry.is_some()
        {
            return Err(Busy::Durability);
        }
        // Issue #351: residency follows real semantics. A composed Goal that
        // is durably Active with autonomous budget remaining still owns future
        // work, so evicting it would race its own eligible continuation.
        // Paused, Blocked, Complete, absent, an uncomposed extension, and an
        // Active Goal whose budget is exhausted all own nothing — an
        // exhausted Goal therefore cannot spin to pin residency forever.
        if admissible
            && probe == IdleProbe::Durable
            && self
                .goal_owns_future_work(state)
                .map_err(|_| Busy::Durability)?
        {
            return Err(Busy::AutonomousExtension);
        }
        if admissible
            && probe == IdleProbe::Durable
            && self.mailbox.has_pending().map_err(|_| Busy::Durability)?
        {
            return Err(Busy::Inbound);
        }
        if self.interaction.pending_count() != 0 {
            return Err(Busy::Interaction);
        }
        if self.tool_runtime.background().owns_idle_work() {
            return Err(Busy::Background);
        }
        if self
            .subagents
            .as_ref()
            .is_some_and(crate::runtime::subagent::SubagentRegistry::owns_idle_work)
        {
            return Err(Busy::Subagent);
        }
        if self.lifecycle.idle_epoch() != Some(epoch) {
            return Err(Busy::AdmissionChanged);
        }
        Ok(epoch)
    }

    /// Whether a composed Goal still owns future autonomous work.
    ///
    /// Both halves are required and neither is an activation bit: the current
    /// resource generation must actually compose the Goal extension (a
    /// Goal-disabled composition never executes stored Goal state), and the
    /// durable snapshot must still authorize continuation. This is the same
    /// answer the Goal-round admission decision uses, so residency and
    /// admission can never disagree.
    pub(super) fn goal_owns_future_work(
        &self,
        state: &CoordinatorState,
    ) -> Result<bool, crate::durable::ConversationStoreError> {
        let Some(domain) = self.composed_goal(state) else {
            return Ok(false);
        };
        domain.authorizes_continuation()
    }

    /// The Goal domain of the current composition, or `None` when this
    /// runtime's frozen root profile does not compose the Goal extension.
    pub(super) fn composed_goal<'a>(
        &'a self,
        state: &CoordinatorState,
    ) -> Option<&'a crate::goal::GoalDomain> {
        self.tool_runtime.goal().filter(|_| {
            state
                .resources
                .root_profile()
                .is_some_and(|profile| profile.extensions.goal().is_some())
        })
    }

    /// Takes the one-shot model-arbitration pause for the next attempt.
    #[cfg(test)]
    pub(super) fn take_model_arbitration_pause(
        &self,
    ) -> Option<crate::agent::execution::test_sync::ModelArbitrationPause> {
        self.probe
            .lock()
            .expect("coordinator probe lock poisoned")
            .as_mut()
            .and_then(|probe| probe.model_arbitration_pause.take())
    }

    pub(super) fn context_runtime_with_assembly(
        &self,
        model: &AttemptModelSnapshot,
        assembly: crate::context::ContextAssembly,
        model_timeout_policy: ModelTimeoutPolicy,
        resources: &RuntimeResourceSnapshot,
        summary_backend: Arc<dyn crate::context::summarizer::SummaryBackend>,
    ) -> Result<ContextRuntime, crate::context::ContextError> {
        let configuration = resources.configuration();
        let policy = configuration.map_or(self.context.policy, |generation| {
            generation.config.context_policy()
        });
        let status_engine = match configuration {
            Some(generation) => generation
                .config
                .extension_composition()
                .agent_status_engine(Arc::new(crate::context::SystemClock)),
            None => self
                .context
                .status_engine
                .as_ref()
                .map(AgentStatusEngine::for_attempt),
        };
        let mut context = ContextRuntime::for_attempt_with_assembly(
            policy,
            Arc::clone(&self.context.estimator),
            status_engine,
            assembly,
            model,
            model_timeout_policy,
            Arc::clone(&self.monotonic_clock),
        )?;
        context
            .summarizer
            .bind_backend(summary_backend, resources.revision().get());
        if let Some(owner) = &self.tool_runtime.uploads {
            context.engine.set_upload_resolver(owner.clone());
        }
        context.engine.set_image_projection(
            self.tool_runtime.artifacts().clone(),
            model
                .primary()
                .capabilities()
                .input_modalities
                .contains(&crate::model::catalog::Modality::Image),
        );
        Ok(context)
    }

    /// Spawns the admission worker: admits the next attempt whenever the
    /// wake gate fires (any mailbox enqueue), so idle asynchronous inbound
    /// is admitted without any client request.
    ///
    /// Spawning uses the execution-runtime handle captured at
    /// construction, so activation from any thread starts the worker.
    ///
    /// # Lifetime
    ///
    /// The worker never owns the runtime. It captures `Weak<RuntimeInner>`
    /// plus an `Arc<WakeGate>` — the minimal wait state — and it upgrades
    /// the weak handle only inside an admission step, never across an
    /// ordinary idle wait. Native manual-tail joins retain their accountable
    /// owner until actual exit. A parked worker holds no strong reference, so it
    /// cannot keep a runtime alive that has no semantic owner left.
    ///
    /// Termination is deterministic, not timed: dropping the last
    /// `Arc<RuntimeInner>` runs `RuntimeInner::drop`, which closes the wake
    /// gate and wakes the worker; the worker observes the closed gate and
    /// exits. The upgrade check is a second, independent exit path.
    pub(super) fn ensure_worker(self: &Arc<Self>) {
        if self.worker_started.swap(true, Ordering::SeqCst) {
            return;
        }
        let weak = Arc::downgrade(self);
        let wake = Arc::clone(&self.wake);
        let lifecycle = self.lifecycle.clone();
        // The mailbox's shared admission wake: every ordinary inbound
        // enqueue notifies it at its publication linearization point, so an
        // idle conversation admits asynchronous inbound without any client
        // request.
        let mailbox_wake = self.mailbox.wake();
        // The execution runtime validated at construction, so activation
        // spawns the worker unconditionally: it can neither panic on a
        // missing runtime nor silently leave a conversation that never
        // admits anything.
        //
        // The worker is also the publisher of configuration-adoption
        // eligibility after activation. Every eligibility input either crosses a counted
        // lifecycle admission (attempts, compaction, inbound, interactions,
        // background and subagent ownership, Goal writes), moves the lifecycle
        // itself, or changes inside an admission pass this worker runs or is
        // woken for. The lifecycle announcement is enabled before each
        // derivation, so a change that lands after it re-derives: the
        // publication is level-triggered, not inferred from event traffic.
        self.executor.spawn(async move {
            let mut admit = false;
            loop {
                let lifecycle_changed = lifecycle.changed();
                tokio::pin!(lifecycle_changed);
                lifecycle_changed.as_mut().enable();
                // Retain the native owner across a concrete manual-tail join,
                // then release it before the ordinary idle wait below.
                {
                    let Some(inner) = weak.upgrade() else {
                        break;
                    };
                    inner.reap_manual_tail(false).await;
                    if admit && !inner.admit_next_scheduled() {
                        inner.admit_next_attempt();
                    }
                    inner.publish_adoption_eligibility();
                }
                admit = tokio::select! {
                    biased;
                    () = wake.notify.notified() => true,
                    () = mailbox_wake.notified() => true,
                    () = wake.eligibility.notified() => false,
                    () = &mut lifecycle_changed => false,
                };
                if wake.is_closed() {
                    break;
                }
            }
            // The lifecycle has left `Running`; publish that before the exit
            // boundary drain waits on, so no publication follows quiescence.
            if let Some(inner) = weak.upgrade() {
                inner.publish_adoption_eligibility();
            }
            wake.mark_exited();
            #[cfg(test)]
            wake.signal_worker_exit();
        });
    }

    /// Admits one attempt when the runtime is idle and the mailbox holds
    /// pending work.
    ///
    /// This is the **one** attempt-admission authority. Every producer —
    /// the Runtime Client human submit path, runtime/agent inbound,
    /// background completion, future subagent/fleet/external producers —
    /// publishes through the inbound mailbox and is admitted here or is
    /// consumed by the active attempt at its safe boundary.
    ///
    /// # Linearization
    ///
    /// The activation observation, the idle observation, the
    /// shutdown/admission-gate observation, the
    /// finite mailbox drain, the canonical-history commits, the attempt-id
    /// allocation, the model snapshot freeze, and the current-attempt
    /// publication all share the one coordinator lock. The mailbox drain
    /// fires its observer only into the leaf pending queue, never back into
    /// this lock. After the publication the lock is released and the
    /// attempt task is spawned, so at most one active [`AgentExecution`]
    /// exists per conversation.
    #[allow(clippy::too_many_lines)]
    pub(super) fn admit_next_attempt(self: &Arc<Self>) {
        // Test-only gate: parks before the coordinator lock, so a competing
        // publish can still enqueue while the admission is gated. The gate
        // parks only when armed and disarms after one park. The gate handle
        // is extracted before the park so the probe mutex is not held while
        // parked (a parked admission must never block a submit that holds the
        // coordinator lock and only briefly probes the gate).
        #[cfg(test)]
        let admission_gate = self
            .probe
            .lock()
            .expect("coordinator probe lock poisoned")
            .as_ref()
            .and_then(|probe| probe.admission_gate.clone());
        #[cfg(test)]
        if let Some(gate) = admission_gate {
            gate.enter();
        }
        #[cfg(test)]
        if let Some(contended) = self
            .probe
            .lock()
            .expect("coordinator probe lock poisoned")
            .as_mut()
            .and_then(|probe| probe.admission_contended.take())
        {
            assert!(matches!(
                self.state.try_lock(),
                Err(std::sync::TryLockError::WouldBlock)
            ));
            contended.notify_one();
        }
        let mut state = self.lock_state();
        if !self.lifecycle.is_running()
            || (state.current_attempt.is_some() || state.scheduled.is_some())
            || state.manual_compaction.is_some()
            || state
                .parent_turn_permits
                .is_some_and(|permits| state.admitted_attempts >= permits)
        {
            return;
        }
        // Persistent durable failure: no new admission may begin. The one
        // authoritative failure fact lives in the runtime-owned DurabilityGate;
        // this is not a hot loop because the fact is absorbing and the worker
        // is not re-kicked once it exists.
        if self.durability_gate.is_failed() {
            return;
        }
        // A Session whose published Model Catalog no longer admits its
        // configured selection admits nothing — no accepted inbound, recovered
        // continuation or Goal round — until a valid selection is committed;
        // committing one wakes this pass. The invocation it last resolved is
        // never a substitute.
        if state.model.unavailable().is_some() {
            return;
        }
        // Live admission guard: the coordinator may only adopt inbound when
        // the active conversation is at a safe boundary (no incomplete tool
        // call without its committed ToolResult sibling). This closes the
        // window a failed tool-result batch would otherwise leave open.
        match self.store.unsettled_canonical_tool_calls(1) {
            Ok(calls) if !calls.is_empty() => {
                self.record_durability_failure(
                    &mut state,
                    DurableOperation::IncompleteToolTurn,
                    format!(
                        "canonical Tool occurrence {:?} has no committed ToolResult",
                        calls[0].0
                    ),
                );
                return;
            }
            Err(error) => {
                self.record_durability_failure(
                    &mut state,
                    DurableOperation::IncompleteToolTurn,
                    error.to_string(),
                );
                return;
            }
            Ok(_) => {}
        }
        // Selection freezes the finite watermark (non-destructive): an
        // acceptance that linearizes after this point can never join the
        // selected batch. A storage failure here is never silently
        // swallowed (Finding 5): it is recorded, observed, and re-kicked
        // exactly once.
        let batch = match self.mailbox.select_pending_batch() {
            Ok(Some(batch)) => {
                Self::record_durability_success(&mut state, DurableOperation::SelectPendingBatch);
                // A fresh inbound batch subsumes the recovered continuation:
                // the new attempt sees the already-canonical unanswered turn
                // in its context anyway, so the one-shot permission is
                // consumed here rather than starting a second attempt later.
                batch
            }
            Ok(None) => {
                Self::record_durability_success(&mut state, DurableOperation::SelectPendingBatch);
                // No pending work is a semantic completion boundary for the
                // finite admission cycle. Only here, or after successful
                // batch adoption below, is the retry budget reset.
                Self::complete_admission_cycle(&mut state);
                // Issue #12 (M9a), phase 4 — resume only proven-safe work.
                // Startup recovery classified the crash as "admitted, no
                // external side effect ever crossed a start commit", so the
                // already-canonical user turn may continue through one **new**
                // attempt. Nothing is re-adopted and no `UserMessage` is
                // duplicated: the turn is already in the Ledger and on the
                // Surface, and the attempt runs as an explicit
                // `InitialTurnTrigger::Continuation`.
                //
                // A Class C conversation never reaches this branch: an
                // indeterminate external outcome leaves the permission unset,
                // so recovery starts nothing at all.
                if state.recovered_continuation.is_some() {
                    self.admit_continuation(state);
                    return;
                }
                // The one autonomous continuation decision (Issue #351).
                // Eligibility here is: this is the ordinary safe idle
                // admission boundary, no pending inbound won it, the Goal
                // extension is composed, no owned background or Subagent work
                // is outstanding, and the durable phase plus remaining budget
                // authorize another round. There is no activation precondition.
                if let Some(goal) = self.composed_goal(&state) {
                    let admission = self
                        .tool_runtime
                        .background()
                        .with_goal_idle(|| {
                            let request = || {
                                crate::goal::GoalRoundDriver::request(
                                    goal,
                                    &self.mailbox,
                                    self.clock.now(),
                                )
                            };
                            match &self.subagents {
                                Some(registry) => registry.with_goal_idle(request),
                                None => Some(request()),
                            }
                        })
                        .flatten();
                    if let Some(Err(error)) = admission {
                        // Issue #351: a Goal-round durability failure is a
                        // runtime-health fact, not a Goal intent change. The
                        // absorbing durability fence closes further admission
                        // — which is why this cannot become a hot retry loop —
                        // while the durable Goal phase stays truthful.
                        self.record_durability_failure(
                            &mut state,
                            DurableOperation::GoalRoundAdmission,
                            error.to_string(),
                        );
                    }
                }
                return;
            }
            Err(error) => {
                self.record_durability_failure(
                    &mut state,
                    DurableOperation::SelectPendingBatch,
                    error.to_string(),
                );
                return;
            }
        };
        // Prepare the canonical transition **before** the durable adoption
        // commit: validate every fallible in-memory condition now, so the
        // post-commit installation is infallible (Finding 2). These checks
        // validate stable identities; committed receipts supply the content. On a validation failure
        // nothing is durably adopted and the items remain pending.
        {
            let conversation = state
                .conversation
                .as_ref()
                .expect("the coordinator owns the conversation state while idle");
            for item in batch.items() {
                let block = crate::durable::inbox::canonical_block(item.message());
                match conversation.prepare_commit(&block) {
                    Ok(_) => {}
                    Err(error) => {
                        // A semantic contract failure (the durable pending
                        // item conflicts with canonical memory), not a
                        // transient storage failure: it fails closed
                        // immediately instead of consuming a storage retry.
                        self.record_durability_failure(
                            &mut state,
                            DurableOperation::PrepareAdoption,
                            format!(
                                "a pending inbound item cannot be prepared for canonical adoption: {error}"
                            ),
                        );
                        return;
                    }
                }
            }
        }
        let adopted = match self.mailbox.adopt_pending_batch(&batch, None) {
            Ok(adopted) => adopted,
            Err(error) => {
                self.record_durability_failure(
                    &mut state,
                    DurableOperation::AdoptPendingBatch,
                    error.to_string(),
                );
                return;
            }
        };
        let adoption = adopted.adoption;
        let canonical_receipts = adopted.canonical_receipts;
        let adopted = adopted.items;
        if adopted.is_empty() {
            Self::complete_admission_cycle(&mut state);
            self.mailbox.wake().notify_one();
            return;
        }
        state.recovered_continuation = None;
        // Mutation preserves identity. Every committed row is a subset of the
        // prevalidated identities; only the committed receipt supplies content.
        let conversation = state.conversation.as_ref().expect("idle conversation");
        let prepared_commits: Vec<_> = adopted
            .iter()
            .map(|item| {
                conversation
                    .prepare_commit(&crate::durable::inbox::canonical_block(item.message()))
                    .expect("adopted identity was validated under exclusive ownership")
            })
            .collect();
        let fresh = FreshInboundTurn::new(
            adopted
                .iter()
                .map(|item| item.message().id.clone())
                .collect(),
        )
        .expect("nonempty ordered committed identities");
        // Issue #351: admission provenance is derived here, from the durable
        // inbound this coordinator actually adopted — the only place that
        // knows it. A Goal continuation is accepted into an empty pending
        // queue at the Goal-round frontier, so a batch carrying one is
        // pursuing that committed round even if ordinary inbound joined it
        // before adoption.
        let provenance = adopted
            .iter()
            .find_map(|item| {
                if let InboundKind::GoalContinuation(reference) = &item.message().kind {
                    // Atomic acceptance advanced this exact authority once. Never
                    // sample today's Goal here: it may already have been replaced.
                    let mut admitted = reference.clone();
                    admitted.revision = admitted
                        .revision
                        .checked_add(1)
                        .expect("accepted Goal round advanced its revision");
                    Some(AttemptProvenance::GoalContinuation(admitted))
                } else {
                    None
                }
            })
            .unwrap_or(AttemptProvenance::Inbound);
        // Durable adoption completes this finite admission cycle. The next
        // cycle starts with a fresh select/adopt retry allowance.
        Self::complete_admission_cycle(&mut state);
        // Ownership transfer: the coordinator hands its conversation state
        // to the attempt. From here until settlement the coordinator holds
        // `None` and the attempt is the single mutable conversation
        // authority.
        let mut conversation = state
            .conversation
            .take()
            .expect("the coordinator owns the conversation state while idle");
        for ((prepared, _item), receipt) in prepared_commits
            .into_iter()
            .zip(&adopted)
            .zip(canonical_receipts)
        {
            // Infallible: every adopted identity was validated by
            // `prepare_commit` above under exclusive ownership.
            conversation.install_prepared(prepared);
            self.observe(ConversationObservation::CanonicalCommitted {
                attempt_id: None,
                receipt,
            });
        }
        // The adoption fact is published after its canonical messages, with
        // its exact Journal sequence, releasing the receipt the observation
        // queue is holding for it.
        if let Some(adoption) = adoption
            && let Some(fanout) = self.pending.get()
        {
            if let Some(receipt) = adoption.observation.as_deref() {
                RuntimeObserver::publish_native_batch(self, fanout, receipt, adoption.sequence);
            } else {
                fanout.source_failed();
            }
        }
        self.publish_attempt(state, conversation, Some(fresh), provenance);
    }

    /// Admits one **continuation** attempt over the already-canonical adopted
    /// turn recovered by startup recovery (Issue #12, M9a, recovery Class B).
    ///
    /// This shares the one admission linearization with
    /// [`RuntimeInner::admit_next_attempt`]: the caller still holds the
    /// coordinator lock, and every idle/shutdown/health/structure check has
    /// already run above. The only difference is the trigger — there is no new
    /// inbound to adopt, so the attempt runs as an explicit
    /// `InitialTurnTrigger::Continuation` and no `UserMessage` is committed a
    /// second time.
    pub(super) fn admit_continuation(
        self: &Arc<Self>,
        mut state: MutexGuard<'_, CoordinatorState>,
    ) {
        let conversation = state
            .conversation
            .take()
            .expect("the coordinator owns the conversation state while idle");
        let provenance = state
            .recovered_continuation
            .take()
            .expect("recovered answer obligation");
        self.publish_attempt(state, conversation, None, provenance);
    }

    /// The shared tail of every admission: allocate the attempt identity,
    /// publish the current-attempt slot, freeze the model snapshot, release
    /// the lock, and spawn the attempt task.
    ///
    /// The attempt ordinal comes from the coordinator's recovered allocator,
    /// so it is never an ordinal that already entered durable authority before
    /// a restart.
    pub(super) fn publish_attempt(
        self: &Arc<Self>,
        mut state: MutexGuard<'_, CoordinatorState>,
        conversation: ConversationState,
        fresh: Option<FreshInboundTurn>,
        provenance: AttemptProvenance,
    ) {
        let attempt_id = AttemptId::for_conversation(&self.conversation_id, state.next_attempt_seq);
        state.next_attempt_seq = state.next_attempt_seq.saturating_add(1);
        // The coordinator-owned cancellation handle is the exact trigger
        // `cancel_current_attempt` requests on: the attempt task runs
        // against the same signal, so protocol cancellation always reaches
        // the loop.
        let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
        // A one-shot child-cancellation intent (Issue #60 child side) that
        // committed before this admission linearizes here: the admitted
        // attempt starts already-cancelled, so its first model-turn
        // arbitration resolves `CancelledBeforeStart` and no provider
        // request ever starts. The intent is consumed exactly once; the
        // child is one-shot, so at most one admission can ever consume it.
        if let Some(reason) = state.one_shot_cancel.take() {
            // First winner by construction: the fresh attempt signal has no
            // prior cause.
            let _ = cancellation.request_cancel(reason);
        }
        state.current_attempt = Some(CurrentAttempt {
            configuration: AdmittedConfiguration {
                approval_mode: state.effective_approval_mode,
                model_timeout: state
                    .resources
                    .configuration()
                    .map(|generation| generation.config.model_timeout_policy),
                tool_deadline: state
                    .resources
                    .configuration()
                    .map(|generation| generation.config.tool_deadline_policy),
                attempt: attempt_id.clone(),
                generation: state.resources.revision(),
                model: state.model.view(),
                resources: state.resources.inspection().clone(),
            },
            attempt_id: attempt_id.clone(),
            cancellation: cancellation.clone(),
            provenance,
        });
        // The one admission publication point is also where the admitted
        // attempt becomes countable (Issue #193): the subagent child driver
        // compares this against the terminals it has observed to prove it
        // has seen every attempt an accepted steer could have opened.
        state.admitted_attempts = state.admitted_attempts.saturating_add(1);
        // The attempt model snapshot is taken at exactly this admission
        // linearization boundary, under the same lock that publishes the
        // attempt. A `model_set` that linearizes before this point is
        // observed by the attempt; one that linearizes after it affects only
        // future attempts.
        //
        // This is also the historical/future configuration boundary a restart
        // must respect: a recovered attempt is a **future** attempt and uses
        // the current session model, while a historical Request Snapshot is
        // reconstructed only from its own frozen durable facts and is never
        // rewritten to resemble the new configuration.
        let model = state
            .model
            .snapshot()
            .expect("admission checked the selection under this lock");
        // The attempt's frozen *effective* model configuration, taken at the
        // same linearization point as its resolved snapshot. A named
        // subagent with no explicit model inherits exactly this — never live
        // mutable session state, and never a composition-time capture.
        let model_config = state.model.config().clone();
        let model_registry = state.model.registry().cloned();
        // Freeze runtime execution policy at the same admission boundary as
        // the attempt's model snapshot. A later configuration change can
        // therefore affect only a future admitted attempt.
        let model_timeout_policy =
            state
                .resources
                .configuration()
                .map_or(self.model_timeout_policy, |generation| {
                    generation
                        .config
                        .timeout_policy()
                        .expect("validated generation")
                });
        // The tool execution-liveness policy freezes at the same boundary
        // (Issue #204): every foreground ToolCall of this attempt runs under
        // exactly this policy value.
        let tool_deadline_policy =
            state
                .resources
                .configuration()
                .map_or(self.tool_deadline_policy, |generation| {
                    generation
                        .config
                        .tool_deadline_policy()
                        .expect("validated generation")
                });
        let approval_mode = state.effective_approval_mode;
        let resources = state.resources.clone();
        let capability = resources.capability().clone();
        let lease = self.capability.acquire_attempt_lease_for(capability);
        self.observe(ConversationObservation::AttemptAdmitted {
            read_image_active: resources
                .capability()
                .tool_registry()
                .for_model(model.primary().capabilities())
                .definitions()
                .iter()
                .any(|tool| tool.id.as_str() == "tool-read-image"),
            attempt_id: attempt_id.clone(),
            model: crate::model::session::AttemptModelReadSource::new(model.clone()),
            resource_revision: resources.revision(),
            approval_mode,
        });
        #[cfg(test)]
        self.enter_probe_gate(|probe| probe.admission_committed_gate.clone());
        // The attempt **task** is a runtime-owned operation in its own
        // right, distinct from the current-attempt slot it settles into
        // (Issue #12, M9c). `finish_attempt` clears the slot and then still
        // calls back into the coordinator, so quiescence must cover the task
        // body, not just the slot. The admission is taken under the same
        // coordinator lock that publishes the slot — drain cannot linearize
        // in between because it needs that lock — and it is released only
        // after the task's final callback has returned.
        let attempt_admission = self
            .lifecycle
            .try_enter_running()
            .expect("the coordinator lock owns the attempt admission boundary");
        drop(state);
        #[cfg(test)]
        self.enter_attempt_execution_probe();
        let inner = Arc::clone(self);
        self.executor.spawn(async move {
            let result = inner
                .run_attempt(
                    attempt_id.clone(),
                    conversation,
                    fresh,
                    &cancellation,
                    model,
                    model_timeout_policy,
                    tool_deadline_policy,
                    approval_mode,
                    resources,
                    model_config,
                    model_registry,
                    lease,
                )
                .await;
            inner.finish_attempt(attempt_id, result);
            drop(attempt_admission);
        });
    }
}

impl ConversationRuntime {
    /// Submits one ordinary inbound user message.
    ///
    /// The durable Pending Inbound Inbox owns authoritative metadata: the
    /// message identity (deterministic from the allocated sequence), the
    /// inbound sequence, the persisted timestamp, and the provenance are
    /// all owner-assigned and commit in one durable acceptance transaction.
    /// Success means durably accepted, never assistant-finished: the runtime
    /// wake gate admits the next attempt when the runtime is idle, and while
    /// an attempt is running the message stays durably pending for the next
    /// safe-boundary adoption.
    ///
    /// # Errors
    ///
    /// Returns [`InboundAdmissionError::Inactive`] before activation,
    /// [`InboundAdmissionError::Shutdown`] after shutdown,
    /// [`InboundAdmissionError::EmptyContent`] for empty content,
    /// [`InboundAdmissionError::DurabilityFailed`] while the runtime's
    /// durable authority is in the explicit failed state, and
    /// [`InboundAdmissionError::Mailbox`] for a durable acceptance failure.
    ///
    /// # Panics
    ///
    /// Panics only if the test-only coordinator probe lock is poisoned,
    /// which would mean a previous test hook panicked while holding it.
    pub fn submit_inbound(
        &self,
        content: Vec<UserContentBlock>,
    ) -> Result<InboundAdmission, InboundAdmissionError> {
        self.submit_sourced_inbound(UserSource::Human, content)
    }

    /// Submits one ordinary inbound message under an explicit provenance.
    ///
    /// This is the exact same admission path as
    /// [`ConversationRuntime::submit_inbound`] — the same lifecycle gate,
    /// the same durability-health gate, the same one durable acceptance
    /// linearization — with the provenance supplied by a trusted in-process
    /// owner instead of fixed to `Human`. The subagent child runtime driver
    /// (Issue #60) uses it to enter the delegated task with
    /// `UserSource::Agent(parent)`; IPC itself never appends anything.
    pub(crate) fn submit_sourced_inbound(
        &self,
        source: UserSource,
        content: Vec<UserContentBlock>,
    ) -> Result<InboundAdmission, InboundAdmissionError> {
        self.admit_sourced_inbound(source, content, SemanticInboundClass::Ordinary)
    }

    /// The shared admission path of every trusted in-process inbound
    /// producer.
    pub(super) fn admit_sourced_inbound(
        &self,
        source: UserSource,
        content: Vec<UserContentBlock>,
        class: SemanticInboundClass,
    ) -> Result<InboundAdmission, InboundAdmissionError> {
        if content.is_empty() {
            return Err(InboundAdmissionError::EmptyContent);
        }
        #[cfg(test)]
        let submit_arrival = self
            .inner
            .probe
            .lock()
            .expect("coordinator probe lock poisoned")
            .as_ref()
            .and_then(|probe| probe.submit_arrival.clone());
        #[cfg(test)]
        if let Some(arrival) = submit_arrival {
            arrival.notify_one();
        }
        // Issue #63 (Finding 1): the one coordinator lock is held across the
        // lifecycle/shutdown check, the durability-failure check, **and**
        // the durable acceptance, so a successful acceptance and shutdown
        // have one total ordering. Shutdown therefore linearizes either
        // entirely before the acceptance (and the acceptance fails with
        // `Shutdown`) or entirely after it (and the acceptance is a legal
        // pre-shutdown success). Holding the coordinator lock here nests
        // only the mailbox/store and the DurabilityGate locks inside it, the
        // same order the admission worker already takes; no mailbox/store →
        // coordinator edge exists, so the lock graph stays acyclic. The
        // guard is deliberately kept alive (underscore binding) for the
        // whole acceptance even though the absorbing failure fact itself is
        // read from the DurabilityGate.
        let state = self.inner.lock_state();
        // The child conversation's guidance seal (Issue #193) is absorbing
        // and outranks every other admission gate: once it commits, no
        // parent-authored guidance can reach an Agent Loop boundary, and
        // saying so precisely is what makes the terminal race provable. It
        // is read here, inside the very critical section that performs the
        // durable acceptance below, so acceptance and the seal have one
        // total order. Only a native child activation seals its local inbox.
        if class.is_parent_guidance() && state.parent_guidance_sealed {
            return Err(InboundAdmissionError::GuidanceSealed);
        }
        match self.inner.lifecycle.state() {
            ConversationLifecycleState::Inactive => {
                return Err(InboundAdmissionError::Inactive);
            }
            ConversationLifecycleState::Draining | ConversationLifecycleState::Quiescent => {
                return Err(InboundAdmissionError::Shutdown);
            }
            ConversationLifecycleState::Running => {}
        }
        let permission = self
            .inner
            .durability_gate
            .enter_coordinator_commit()
            .map_err(|failure| InboundAdmissionError::DurabilityFailed {
                message: failure.diagnostic,
            })?;
        // Explicit refusal, ordered with catalog publication and model
        // commits by this lock: nothing is accepted for a selection the
        // Session's published catalog does not admit.
        if let Some(error) = state.model.unavailable() {
            return Err(InboundAdmissionError::ModelUnavailable {
                diagnostic: error.to_string(),
            });
        }
        // The parent Agent registry owns message-versus-interrupt arbitration.
        // Envelopes already on its reliable lane won admission before stopping;
        // child cancellation must not retroactively reject their durable input.
        // Test-only gate: parked while holding the coordinator lock, after
        // the shutdown/activation decision and before the durable acceptance,
        // so a race regression can prove shutdown cannot slip between the
        // decision and the commit. The gate handle is extracted before the
        // park so the probe mutex is not held while parked.
        #[cfg(test)]
        let submit_gate = self
            .inner
            .probe
            .lock()
            .expect("coordinator probe lock poisoned")
            .as_ref()
            .and_then(|probe| probe.submit_gate.clone());
        #[cfg(test)]
        if let Some(gate) = submit_gate {
            gate.enter();
        }
        // The durable acceptance linearization point: the sequence, the
        // deterministic message identity, the pending record, and (when a
        // producer supplies one) the correlation commit here before success
        // is returned.
        let accepted = self
            .inner
            .mailbox
            .accept_draft(InboundDraft {
                message_id: None,
                source,
                kind: InboundKind::Message,
                content,
                timestamp: self.inner.clock.now(),
                correlation: None,
            })
            .map_err(InboundAdmissionError::Mailbox)?;
        drop(permission);
        drop(state);
        Ok(InboundAdmission {
            message_id: accepted.message_id,
            inbound_sequence: accepted.sequence,
        })
    }

    pub(crate) fn has_current_attempt(&self) -> bool {
        self.inner
            .state
            .lock()
            .expect("runtime lock")
            .current_attempt
            .is_some()
    }

    #[cfg(test)]
    pub(crate) async fn wait_for_configuration_admissions(&self) {
        self.inner.lifecycle.wait_for_no_admissions().await;
    }

    pub(crate) fn idle_epoch(&self) -> Result<u64, IdleBusyReason> {
        self.inner.idle_epoch_locked(&self.inner.lock_state())
    }

    /// Closes native admission through the existing shared drain owner. No
    /// busy owner is cancelled by an idle claim. Settlement remains asynchronous.
    pub(crate) fn claim_idle(&self, epoch: u64) -> bool {
        // Residency holds its own publication lock. Never wait there behind
        // coordinator storage work: contention is a conservative idle refusal.
        let Ok(state) = self.inner.state.try_lock() else {
            return false;
        };
        if (state.current_attempt.is_some() || state.scheduled.is_some())
            || state.manual_compaction.is_some()
            || state.conversation.is_none()
            || state.recovered_continuation.is_some()
            || !self.inner.durability_gate.healthy_for_idle_claim()
            || state.mcp_settlement_failure.is_some()
            || state.admission_durability_cycle.pending_retry.is_some()
        {
            return false;
        }
        self.inner.lifecycle.begin_idle_drain(epoch)
    }
}

#[cfg(test)]
mod tests;
