//! Manual compaction: an idle runtime-maintenance operation owned by the
//! coordinator, not an Agent Attempt.

use super::*;

/// The runtime-owned manual compaction currently holding the conversation.
///
/// Manual compaction is not an attempt: it allocates no `AttemptId`, emits no
/// attempt lifecycle, and cannot execute tools. The coordinator nevertheless
/// retains its cancellation trigger so runtime drain can supervise the
/// provider-backed summary to terminal settlement.
pub(super) struct CurrentManualCompaction {
    pub(super) operation_id: crate::runtime::identity::CompactionOperationId,
    pub(super) cancellation: AgentCancellation,
}

pub(super) struct ManualCompactionTaskResult {
    pub(super) committed_failure: Option<ExecutedCompaction>,
    pub(super) conversation: ConversationState,
    pub(super) result: Result<ManualCompactionSuccess, ManualCompactionError>,
}

/// A successful task-local compaction whose durable facts have not yet been
/// published to Runtime Client observers. Publication belongs to coordinator
/// settlement, after the checked-out conversation and maintenance slot have
/// returned.
pub(super) struct ManualCompactionSuccess {
    pub(super) outcome: ManualCompactionOutcome,
    pub(super) completed: ExecutedCompaction,
}

/// One runtime-owned manual compaction completion. The provider operation is
/// spawned independently of the protocol waiter, so dropping an attachment
/// can never strand the checked-out conversation state.
#[derive(Debug, Default)]
pub(super) struct ManualCompactionCompletion {
    pub(super) completed: AtomicBool,
    pub(super) ready_to_join: AtomicBool,
    pub(super) joined: AtomicBool,
    pub(super) result: Mutex<Option<Result<ManualCompactionOutcome, ManualCompactionError>>>,
    pub(super) notify: tokio::sync::Notify,
}

pub(super) struct ManualTail {
    pub(super) worker: tokio::task::JoinHandle<()>,
    pub(super) completion: Arc<ManualCompactionCompletion>,
}

pub(super) struct ManualTailExit {
    pub(super) completion: Arc<ManualCompactionCompletion>,
    pub(super) wake: Arc<WakeGate>,
    pub(super) _admission: crate::runtime::types::LifecycleAdmission,
}

impl Drop for ManualTailExit {
    fn drop(&mut self) {
        // This announces that the native owner may await the actual Join;
        // it is deliberately not physical-exit evidence by itself.
        self.completion.ready_to_join.store(true, Ordering::Release);
        self.wake.notify.notify_one();
    }
}

/// A reply view after native ownership of the whole maintenance tail transferred.
pub(crate) struct ManualCompactionHandle(Arc<ManualCompactionCompletion>);

impl ManualCompactionHandle {
    pub(crate) async fn wait(self) -> Result<ManualCompactionOutcome, ManualCompactionError> {
        self.0.wait().await
    }
}

impl ManualCompactionCompletion {
    pub(super) fn complete(&self, result: Result<ManualCompactionOutcome, ManualCompactionError>) {
        *self
            .result
            .lock()
            .expect("manual compaction completion lock poisoned") = Some(result);
        self.completed.store(true, Ordering::Release);
        self.notify.notify_waiters();
    }

    pub(super) async fn wait(&self) -> Result<ManualCompactionOutcome, ManualCompactionError> {
        loop {
            if self.completed.load(Ordering::Acquire) && self.joined.load(Ordering::Acquire) {
                return self
                    .result
                    .lock()
                    .expect("manual compaction completion lock poisoned")
                    .clone()
                    .expect("completed manual compaction has a result");
            }
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.completed.load(Ordering::Acquire) && self.joined.load(Ordering::Acquire) {
                return self
                    .result
                    .lock()
                    .expect("manual compaction completion lock poisoned")
                    .clone()
                    .expect("completed manual compaction has a result");
            }
            notified.await;
        }
    }
}

/// Metadata returned after one manual compaction committed successfully.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManualCompactionOutcome {
    /// The durable projection activation coordinate.
    pub generation: u64,
    /// The immutable committed checkpoint identity.
    pub checkpoint_id: crate::runtime::identity::CheckpointId,
    /// The independently durable historical activation.
    pub activation_id: crate::context::checkpoint::ProjectionActivationId,
    /// The request-context measurement before compaction.
    pub tokens_before: crate::runtime::types::TokenMeasurement,
    /// The deterministic request-context estimate after compaction.
    pub estimated_tokens_after: u64,
}

/// A manual context-compaction failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManualCompactionError {
    /// The runtime has not been activated.
    Inactive,
    /// Runtime drain has begun.
    Shutdown,
    /// An attempt or another manual compaction currently owns the mutable
    /// conversation state.
    Busy,
    /// The runtime's durable authority is already failed.
    DurabilityFailed { message: String },
    /// The Session's published Model Catalog does not admit its configured
    /// selection; a summary needs a valid one.
    ModelUnavailable { diagnostic: String },
    /// Planning, summary generation, cancellation, or fit validation failed
    /// before a durable transition committed.
    Context(ContextError),
    /// The atomic durable compaction transition failed. This also moves the
    /// runtime into its absorbing durability-failed state.
    Durable { message: String },
}

impl core::fmt::Display for ManualCompactionError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Inactive => formatter.write_str("the conversation runtime is not activated"),
            Self::Shutdown => formatter.write_str("the conversation runtime is shutting down"),
            Self::Busy => formatter
                .write_str("manual context compaction requires an idle conversation runtime"),
            Self::DurabilityFailed { message } => write!(
                formatter,
                "the conversation runtime durability authority failed: {message}"
            ),
            Self::ModelUnavailable { diagnostic } => write!(
                formatter,
                "the configured session model is unavailable: {diagnostic}"
            ),
            Self::Context(error) => formatter.write_str(&error.message),
            Self::Durable { message } => write!(
                formatter,
                "the compaction transition cannot be committed durably: {message}"
            ),
        }
    }
}

impl std::error::Error for ManualCompactionError {}

impl RuntimeInner {
    /// Persists maintenance lifecycle evidence before exposing it to clients.
    /// Completion uses the atomic compaction commit instead of this path.
    pub(super) fn publish_manual_compaction_event(
        &self,
        state: &mut CoordinatorState,
        request_id: Option<&crate::runtime::identity::ManualCompactionRequestId>,
        event: RuntimeEvent,
    ) -> Result<(), ManualCompactionError> {
        let envelope = crate::events::types::RuntimeEventEnvelope {
            tool_occurrence: None,
            observation: None,
            schema_version: crate::events::types::EVENT_SCHEMA_VERSION,
            event_id: crate::runtime::identity::EventId::new(""),
            sequence: 0,
            conversation_id: self.conversation_id.clone(),
            attempt_id: None,
            turn_id: None,
            timestamp: chrono::Utc::now(),
            event,
        };
        match self.store.append_event(envelope) {
            Ok(persisted) => {
                self.publish_manual_compaction_receipt(request_id, &persisted);
                Ok(())
            }
            Err(error) => {
                let message = format!("manual compaction event could not be persisted: {error}");
                self.record_durability_failure(
                    state,
                    DurableOperation::EventJournal,
                    message.clone(),
                );
                Err(ManualCompactionError::Durable { message })
            }
        }
    }

    /// Runs one manual compaction over the conversation state checked out by
    /// the coordinator. This is runtime-owned work: the task continues to
    /// settlement even if the requesting attachment disappears.
    pub(super) async fn run_manual_compaction(
        self: &Arc<Self>,
        mut conversation: ConversationState,
        context_runtime: ContextRuntime,
        tools: Vec<ModelToolDefinition>,
        system_sections: Vec<crate::context::AcceptedSystemSection>,
        cancellation: &AgentCancellation,
        budget: crate::context::compaction_budget::CompactionBudget,
    ) -> ManualCompactionTaskResult {
        let operation_id = budget.operation_id().clone();
        let effective_system_prompt = render_effective_system_prompt(&system_sections);
        let signal = cancellation.signal();
        let operation = match context_runtime
            .summarizer
            .operation(budget.clone(), signal.child())
        {
            Ok(operation) => operation,
            Err(error) => {
                return ManualCompactionTaskResult {
                    conversation,
                    result: Err(ManualCompactionError::Context(error)),
                    committed_failure: None,
                };
            }
        };
        let executed = execute_compaction_in_operation(
            &mut conversation,
            &context_runtime,
            &self.conversation_id,
            self.store.as_ref(),
            &tools,
            None,
            &effective_system_prompt,
            &CompactionConstraints::default(),
            &signal,
            CompactionAttribution {
                operation_id: operation_id.clone(),
                attempt_id: None,
                turn_id: None,
            },
            operation.as_ref(),
        )
        .await;
        operation.close().await;
        let result = match executed {
            Ok(completed) => {
                let RuntimeEvent::CompactionCompleted {
                    generation,
                    checkpoint_id,
                    activation_id,
                    tokens_before,
                    estimated_tokens_after,
                    ..
                } = &completed.persisted_event.event
                else {
                    return ManualCompactionTaskResult {
                        committed_failure: Some(completed),
                        conversation,
                        result: Err(ManualCompactionError::Durable {
                            message: "the durable compaction returned a non-compaction event"
                                .to_owned(),
                        }),
                    };
                };
                let outcome = ManualCompactionOutcome {
                    generation: *generation,
                    checkpoint_id: checkpoint_id.clone(),
                    activation_id: *activation_id,
                    tokens_before: *tokens_before,
                    estimated_tokens_after: *estimated_tokens_after,
                };
                Ok(ManualCompactionSuccess { outcome, completed })
            }
            Err(CompactionExecutionError::Context(error)) => {
                Err(ManualCompactionError::Context(error))
            }
            Err(CompactionExecutionError::Committed { completed, error }) => {
                return ManualCompactionTaskResult {
                    conversation,
                    committed_failure: Some(*completed),
                    result: Err(ManualCompactionError::Durable { message: error }),
                };
            }
            Err(CompactionExecutionError::Durable(message)) => {
                Err(ManualCompactionError::Durable { message })
            }
        };
        ManualCompactionTaskResult {
            committed_failure: None,
            conversation,
            result,
        }
    }

    pub(super) fn publish_manual_compaction_receipt(
        &self,
        request: Option<&crate::runtime::identity::ManualCompactionRequestId>,
        persisted: &crate::events::types::RuntimeEventEnvelope,
    ) {
        let Some(fanout) = self.pending.get() else {
            return;
        };
        let batch = persisted.observation.as_deref();
        if !fanout
            .native_manual_compaction(request, batch)
            .is_installed()
        {
            fanout.source_failed();
            return;
        }
        let Some(batch) = batch else {
            fanout.source_failed();
            return;
        };
        if let Some(canonical) = &batch.canonical
            && self
                .store
                .mark_canonical_transcript_ready(canonical.first_position, canonical.last_position)
                .is_err()
        {
            fanout.source_failed();
            return;
        }
        if let Some((first, last)) = batch.journal_range {
            match self.store.mark_observation_direct(first, last) {
                Ok(()) => fanout.source_ready(last),
                Err(_) => fanout.source_failed(),
            }
        }
    }

    /// Restores the sole conversation state, records any durable failure,
    /// clears the maintenance slot, and hands pending inbound back to the one
    /// admission path.
    pub(super) fn finish_manual_compaction(
        self: &Arc<Self>,
        task: ManualCompactionTaskResult,
        completion: &ManualCompactionCompletion,
        request_id: Option<&crate::runtime::identity::ManualCompactionRequestId>,
    ) {
        let ManualCompactionTaskResult {
            committed_failure,
            conversation,
            result,
        } = task;
        let completion_result;
        {
            let mut state = self.lock_state();
            state.conversation = Some(conversation);
            let maintenance = state
                .manual_compaction
                .take()
                .expect("manual compaction settlement owns the maintenance slot");
            if let Err(ManualCompactionError::Durable { message }) = &result {
                self.record_durability_failure(
                    &mut state,
                    DurableOperation::CanonicalCommit,
                    format!(
                        "the manual compaction transition cannot be committed durably: {message}"
                    ),
                );
            }
            let confirmed_commit = committed_failure.is_some();
            if let Some(completed) = committed_failure
                && completed.newly_committed
            {
                self.publish_manual_compaction_receipt(request_id, &completed.persisted_event);
            }
            completion_result = match result {
                Ok(success) => {
                    // Durable commit already happened in the task, but the
                    // live completion is published only after coordinator
                    // ownership is restored and the maintenance slot is
                    // clear. Neither observation has an attempt identity.
                    if success.completed.newly_committed {
                        self.publish_manual_compaction_receipt(
                            request_id,
                            &success.completed.persisted_event,
                        );
                    }
                    Ok(success.outcome)
                }
                Err(error) if confirmed_commit => Err(error),
                Err(error) => self
                    .publish_manual_compaction_event(
                        &mut state,
                        request_id,
                        RuntimeEvent::CompactionFailed {
                            operation_id: maintenance.operation_id,
                            error: error.to_string(),
                        },
                    )
                    .and(Err(error)),
            };
        }
        self.settlement.notify_waiters();
        // A closed launcher's stderr is not a maintenance failure.
        let _ = writeln!(
            std::io::stderr(),
            "rustx: manual context compaction settled for {}: {}",
            self.conversation_id,
            match &completion_result {
                Ok(_) => "committed".to_owned(),
                Err(error) => format!("failed: {error}"),
            }
        );
        completion.complete(completion_result);
        self.admit_next_attempt();
    }
}

impl ConversationRuntime {
    /// Manually compacts the current canonical Conversation Surface.
    ///
    /// This is an idle runtime-maintenance operation, not an Agent attempt:
    /// it allocates no attempt/turn identity and never invokes tools. The
    /// current session model and capability snapshot are frozen at admission,
    /// the provider-backed summary runs in a runtime-owned task, and success
    /// is returned only after typed checkpoint content, activation/head, receipt
    /// and actual local completion committed atomically. Inbound accepted while compaction runs remains
    /// pending and is admitted after the conversation state is restored.
    ///
    /// # Errors
    ///
    /// Returns [`ManualCompactionError::Busy`] while an attempt or another
    /// manual compaction owns the state, plus typed lifecycle, durability,
    /// planning, summary, cancellation, and durable-commit failures.
    ///
    /// # Panics
    ///
    /// Panics only if an idle coordinator has lost ownership of its sole
    /// conversation state, which would violate the runtime ownership
    /// invariant, or if the configured executor rejects the spawned task.
    #[allow(clippy::too_many_lines)] // Admission and terminal settlement remain visible in one transaction.
    pub async fn compact_context(
        &self,
        request_id: Option<crate::runtime::identity::ManualCompactionRequestId>,
    ) -> Result<ManualCompactionOutcome, ManualCompactionError> {
        self.admit_compaction(request_id)?.wait().await
    }

    #[allow(clippy::too_many_lines)] // Native ownership transfers the complete maintenance tail once.
    pub(crate) fn admit_compaction(
        &self,
        request_id: Option<crate::runtime::identity::ManualCompactionRequestId>,
    ) -> Result<ManualCompactionHandle, ManualCompactionError> {
        let completion = {
            let mut state = self.inner.lock_state();
            match self.inner.lifecycle.state() {
                ConversationLifecycleState::Inactive => {
                    return Err(ManualCompactionError::Inactive);
                }
                ConversationLifecycleState::Draining | ConversationLifecycleState::Quiescent => {
                    return Err(ManualCompactionError::Shutdown);
                }
                ConversationLifecycleState::Running => {}
            }
            if let Some(failure) = self.inner.durability_gate.admission_failure() {
                return Err(ManualCompactionError::DurabilityFailed {
                    message: failure.diagnostic,
                });
            }
            if (state.current_attempt.is_some() || state.scheduled.is_some())
                || state.manual_compaction.is_some()
                || self
                    .inner
                    .manual_tail
                    .lock()
                    .expect("manual tail owner")
                    .is_some()
            {
                return Err(ManualCompactionError::Busy);
            }
            let operation_id = crate::runtime::identity::CompactionOperationId::new(
                uuid::Uuid::now_v7().to_string(),
            );
            let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
            let budget = crate::context::compaction_budget::CompactionBudget::new(
                operation_id.clone(),
                self.inner.monotonic_clock.clone(),
            );
            let summary_backend = self
                .inner
                .auxiliary
                .pre_admit_summary(budget.clone(), &cancellation.signal())
                .map_err(ManualCompactionError::Context)?;
            let model = state.model.snapshot().map_err(|error| {
                ManualCompactionError::ModelUnavailable {
                    diagnostic: error.to_string(),
                }
            })?;
            let resources = state.resources.clone();
            let context_runtime = self
                .inner
                .context_runtime_with_assembly(
                    &model,
                    resources.context_assembly().clone(),
                    resources.configuration().map_or(
                        self.inner.model_timeout_policy,
                        |generation| {
                            generation
                                .config
                                .timeout_policy()
                                .expect("validated generation")
                        },
                    ),
                    &resources,
                    summary_backend,
                )
                .map_err(ManualCompactionError::Context)?;
            // Manual maintenance has no attempt lease, but it still freezes
            // the exact capability definitions used for planning at the
            // same admission boundary as the model/context snapshot.
            let tools = resources.capability().tool_registry().model_definitions();
            let system_sections = resources
                .context_assembly()
                .system_sections(&NativeContextInput {
                    workspace_instructions: resources.project_instructions().map(str::to_owned),
                    skill_guidance: resources.skill_catalog().map(str::to_owned),
                    agent_profile: resources.agent_profile().map(str::to_owned),
                    ..NativeContextInput::default()
                })
                .map_err(|error| {
                    ManualCompactionError::Context(ContextError::new(
                        ContextErrorKind::Internal,
                        format!("the frozen capability system guidance is invalid: {error}"),
                    ))
                })?;
            self.inner.publish_manual_compaction_event(
                &mut state,
                request_id.as_ref(),
                RuntimeEvent::CompactionStarted {
                    operation_id: operation_id.clone(),
                },
            )?;
            let conversation = state
                .conversation
                .take()
                .expect("the idle coordinator owns the conversation state");
            state.manual_compaction = Some(CurrentManualCompaction {
                operation_id: operation_id.clone(),
                cancellation: cancellation.clone(),
            });
            let admission = self
                .inner
                .lifecycle
                .try_enter_running()
                .expect("the coordinator lock owns manual compaction admission");
            let completion = Arc::new(ManualCompactionCompletion::default());
            let completion_for_task = completion.clone();
            let inner = Arc::clone(&self.inner);
            let (release, registered) = tokio::sync::oneshot::channel();
            drop(state);
            // Diagnostics follow the runtime owner, not the requesting socket:
            // disconnecting the requester cannot hide maintenance settlement.
            let _ = writeln!(
                std::io::stderr(),
                "rustx: manual context compaction admitted for {}",
                self.inner.conversation_id
            );
            let task_completion = completion.clone();
            let worker = self.inner.executor.spawn(async move {
                if registered.await.is_err() {
                    return;
                }
                let _tail = ManualTailExit {
                    completion: task_completion,
                    wake: inner.wake.clone(),
                    _admission: admission,
                };
                let task = inner
                    .run_manual_compaction(
                        conversation,
                        context_runtime,
                        tools,
                        system_sections,
                        &cancellation,
                        budget,
                    )
                    .await;
                #[cfg(test)]
                {
                    let gate = inner
                        .probe
                        .lock()
                        .expect("coordinator probe lock poisoned")
                        .as_mut()
                        .and_then(|probe| probe.manual_compaction_settlement_gate.take());
                    if let Some(gate) = gate {
                        gate.enter();
                    }
                }
                inner.finish_manual_compaction(task, &completion_for_task, request_id.as_ref());
            });
            *self.inner.manual_tail.lock().expect("manual tail owner") = Some(ManualTail {
                worker,
                completion: completion.clone(),
            });
            let _ = release.send(());
            completion
        };
        Ok(ManualCompactionHandle(completion))
    }
}

#[cfg(test)]
mod tests;
