//! The projection worker lifecycle: state locks, worker admission, presentation
//! reads and drain. The worker never owns runtime cancellation or settlement.
use super::publication::commit_display;
use super::publication::prepared_lock;
use super::{
    Arc, ClientInner, ClientState, ConversationStore, MutexGuard, Ordering, PendingObservations,
    ProjectionReadResult, ProjectionReadSlot, ProjectionWorker, RuntimeClientError,
    RuntimeClientProjection, RuntimeClientResult,
};

pub(super) fn completed_projection_read(
    result: Result<ProjectionReadResult, tokio::task::JoinError>,
    cut: Option<super::super::projection::ReadDomainCut>,
) -> Option<ProjectionReadResult> {
    match result {
        Ok(value) => Some(value),
        Err(error) => cut.map(|cut| {
            let message = if error.is_panic() {
                "native projection Source task panicked"
            } else {
                "native projection Source task was cancelled"
            };
            (
                cut,
                Err(crate::durable::ConversationStoreError::Storage(
                    message.into(),
                )),
            )
        }),
    }
}
pub(super) async fn next_projection_read(
    reads: &ProjectionReadSlot,
) -> Result<ProjectionReadResult, tokio::task::JoinError> {
    let mut slot = reads.lock().await;
    let Some(task) = slot.as_mut() else {
        drop(slot);
        return std::future::pending().await;
    };
    let result = task.await;
    slot.take();
    result
}
pub(super) async fn drain_projection_read(
    reads: &ProjectionReadSlot,
) -> Result<(), tokio::task::JoinError> {
    let mut slot = reads.lock().await;
    let result = match slot.as_mut() {
        Some(task) => task.await.map(|_| ()),
        None => Ok(()),
    };
    slot.take();
    result
}
pub(super) fn read_domains_installable(
    projection: &RuntimeClientProjection,
    pending: &PendingObservations,
) -> bool {
    !pending.is_closed()
        && pending.can_publish_prepared_reads()
        && !pending.has_unpublished()
        && !projection.snapshot_ref().shutting_down
}
pub(super) fn close_shutting_down_reader(
    projection: &RuntimeClientProjection,
    store: &std::sync::Weak<dyn ConversationStore>,
) {
    if projection.snapshot_ref().shutting_down
        && let Some(store) = store.upgrade()
    {
        store.close_presentation_reader();
    }
}

impl ClientInner {
    #[cfg(test)]
    pub(crate) fn projection_source_debug(&self) -> String {
        let projection = self.state.try_lock().map_or_else(
            |_| "projection state busy or poisoned".into(),
            |state| state.projection.source_prefix_debug(),
        );
        let last_wait = self.publication_gate.settlement_wait.try_lock().ok();
        let wait = last_wait.as_ref().and_then(|guard| guard.as_ref()).map(|wait| {
            format!(
                "checked_ago={:?} elapsed={:?} worker_turn={} pending_settled={} read_cut={:?} source={} queue={}",
                wait.checked.elapsed(), wait.elapsed, wait.worker_turn, wait.pending_settled,
                wait.read_cut, wait.source_status, wait.queue,
            )
        });
        format!(
            "{projection} queue={} last_settlement_wait={wait:?} stages={}",
            self.pending.source_progress_debug(),
            self.publication_gate.diagnostic()
        )
    }

    pub(crate) fn observation_activation_id(&self) -> Option<crate::runtime::identity::SubagentId> {
        self.state
            .lock()
            .expect("native observation scope")
            .projection
            .activation_id()
            .cloned()
    }

    /// Prepares queued evidence for explicit worker/control operations.
    /// Public observation reads use the already published cut without draining.
    pub(crate) fn lock_state(&self) -> MutexGuard<'_, ClientState> {
        prepared_lock(
            &self.state,
            &self.observation_preparation,
            &self.publication_gate,
            &self.pending,
            self.store.as_ref(),
            self.runtime
                .as_ref()
                .map(|runtime| runtime.tool_runtime().workflows()),
        )
    }

    /// Capture published read authority without preparing or publishing work.
    pub(super) fn lock_published_state(&self) -> MutexGuard<'_, ClientState> {
        self.state.lock().expect("published native read state")
    }

    /// Pending readback is advisory repair at its captured fence. A raced or
    /// unpublished mutation uses the already represented semantic pending view;
    /// candidate acquisition never retries to catch the head.
    #[cfg(test)]
    pub(super) fn snapshot_candidate(
        &self,
    ) -> Result<super::super::projection::SnapshotCandidate, RuntimeClientError> {
        let fence = self.lock_state().projection.read_domain_fence();
        let items = if let Some(runtime) = &self.runtime {
            runtime
                .tool_runtime()
                .mailbox()
                .with_pending_snapshot(|items| items)
                .map_err(|error| RuntimeClientError::RuntimeFailure {
                    message: format!("durable pending snapshot failed: {error}"),
                })?
        } else {
            None
        };
        let candidate = {
            let mut state = self.lock_state();
            if !self.pending.has_unpublished()
                && state.projection.read_domain_fence() == fence
                && let Some(items) = items
            {
                state.projection.repair_pending(items);
            }
            state.projection.snapshot_candidate()?
        };
        #[cfg(test)]
        if let Some(hook) = self.snapshot_read_hook.lock().unwrap().as_mut() {
            hook();
        }
        let candidate = candidate.materialize(self.store.as_ref())?;
        // Completion belongs to C, regardless of whether C may still repair live
        // state. Reuse the background installation owner after folding new facts.
        drop(self.lock_state());
        let trace_total = self
            .store
            .observation_trace_total(candidate.trace_through)
            .map_err(RuntimeClientError::observation_store_error)?;
        commit_display(&self.state, &self.publication_gate, |projection| {
            if !self.read_only
                && !self.pending.is_closed()
                && !self.pending.has_unpublished()
                && !projection.snapshot_ref().shutting_down
            {
                projection.install_read_domains(
                    &candidate.read_cut,
                    Ok(super::super::projection::ReadDomainValues {
                        projection_head: candidate.snapshot.context.projection_head.clone(),
                        transcript: candidate.snapshot.transcript.clone(),
                        occupancy: candidate.snapshot.context.occupancy.clone(),
                        trace: candidate.snapshot.trace.clone(),
                        trace_total,
                    }),
                    true,
                );
            }
        });
        Ok(candidate)
    }

    /// A bound live projection can expose interactions to a future client.
    /// Admission follows runtime binding, never external attachment presence.
    pub(crate) fn admits_interaction_publication(&self) -> bool {
        self.runtime.is_some() && !self.read_only
    }

    /// Claims the one worker when a Tokio runtime is available.
    pub(super) fn claim_worker(&self) -> Option<tokio::sync::MutexGuard<'_, ProjectionWorker>> {
        let worker_slot = self.worker.try_lock().ok()?;
        if self.pending.is_closed()
            || self.runtime.is_none()
            || tokio::runtime::Handle::try_current().is_err()
            || self.worker_started.swap(true, Ordering::SeqCst)
        {
            return None;
        }
        Some(worker_slot)
    }

    pub(super) fn workflow_read_model(
        &self,
    ) -> Option<crate::runtime::workflow::read_model::WorkflowReadModel> {
        self.runtime
            .as_ref()
            .map(|runtime| runtime.tool_runtime().workflows().clone())
    }

    /// Advances source-backed Journal and read-domain observations for idle subscribers.
    /// Native owner callbacks publish their finite receipts synchronously; this worker
    /// reads remaining indexed sources outside the Projection lock. Its cancellable
    /// blocking read is joined before shutdown releases physical source ownership.
    #[allow(clippy::too_many_lines)] // Test-only stage probes preserve the production owner sequence.
    pub(crate) fn ensure_worker(self: &Arc<Self>) {
        let Some(mut worker_slot) = self.claim_worker() else {
            return;
        };
        let state = self.state.clone();
        let preparation = self.observation_preparation.clone();
        let publication_gate = self.publication_gate.clone();
        let store = Arc::downgrade(&self.read_store);
        let pending = Arc::clone(&self.pending);
        let workflow_state = self.workflow_read_model();
        let mut workflows = workflow_state
            .as_ref()
            .map(crate::runtime::workflow::read_model::WorkflowReadModel::subscribe);
        #[cfg(test)]
        let read_domain_hook = self.read_domain_hook.clone();
        #[cfg(test)]
        let worker_turns = Arc::clone(&self.worker_turns);
        let reads = self.projection_read.clone();
        let worker = tokio::spawn(async move {
            let mut reading = None;
            loop {
                #[cfg(test)]
                publication_gate.worker_stage("wait");
                tokio::select! {
                    () = pending.wait() => {},
                    () = async {
                        match &mut workflows {
                            Some(receiver) => { let _ = receiver.changed().await; },
                            None => std::future::pending::<()>().await,
                        }
                    } => {},
                    result = next_projection_read(&reads) => {
                        #[cfg(test)]
                        publication_gate.worker_stage("read_completion");
                        if let Some((cut, domains)) = completed_projection_read(result, reading.take()) {
                            let Some(source)=store.upgrade() else { break };
                            drop(prepared_lock(&state,&preparation,&publication_gate,&pending,source.as_ref(),workflow_state.as_ref()));
                            commit_display(&state, &publication_gate, |projection| {
                                if read_domains_installable(projection, &pending) {
                                    projection.install_read_domains(&cut, domains, true);
                                }
                            });
                        }
                    },
                }
                if pending.is_closed() {
                    break;
                }
                #[cfg(test)]
                publication_gate.worker_stage("prepare");
                let cut = {
                    let Some(source) = store.upgrade() else { break };
                    let guard = prepared_lock(
                        &state,
                        &preparation,
                        &publication_gate,
                        &pending,
                        source.as_ref(),
                        workflow_state.as_ref(),
                    );
                    close_shutting_down_reader(&guard.projection, &store);
                    if reading.is_some()
                        || !pending.can_publish_prepared_reads()
                        || pending.has_unpublished()
                        || guard.projection.snapshot_ref().shutting_down
                    {
                        None
                    } else {
                        guard.projection.read_domain_candidate()
                    }
                };
                if reading.is_none()
                    && let Some(cut) = cut
                {
                    #[cfg(test)]
                    publication_gate.worker_stage("spawn_read");
                    let Some(store) = store.upgrade() else {
                        break;
                    };
                    // One bounded read in flight. It owns no host/runtime or
                    // projection lock; semantic delivery continues while it waits.
                    #[cfg(test)]
                    let hook = read_domain_hook.lock().unwrap().take();
                    let mut read_slot = reads.lock().await;
                    reading = Some(cut.clone());
                    #[cfg(test)]
                    let read_diagnostic = publication_gate.clone();
                    *read_slot = Some(tokio::task::spawn_blocking(move || {
                        #[cfg(test)]
                        read_diagnostic.read_stage("hook", &cut);
                        #[cfg(test)]
                        if let Some(hook) = hook
                            && let Err(error) = hook()
                        {
                            read_diagnostic.read_stage("hook_failed", &cut);
                            return (cut, Err(error));
                        }
                        #[cfg(test)]
                        read_diagnostic.read_stage("source", &cut);
                        let domains = cut.read(store.as_ref());
                        #[cfg(test)]
                        read_diagnostic.read_stage("completed", &cut);
                        (cut, domains)
                    }));
                }
                #[cfg(test)]
                worker_turns.send_modify(|turn| *turn += 1);
            }
            if let Some(store) = store.upgrade() {
                store.close_presentation_reader();
            }
            let _ = drain_projection_read(&reads).await;
            // Cancellation is followed by terminal proof, not handle detachment.
            #[cfg(test)]
            pending.signal_worker_exit();
        });
        *worker_slot = ProjectionWorker::Running(worker);
    }

    pub(crate) fn cancel_presentation_reads(&self) {
        self.read_store.close_presentation_reader();
    }

    /// Cancel Store waits/queries, then join semantic delivery and its finite read.
    pub(crate) async fn drain_projection(&self) -> Result<(), tokio::task::JoinError> {
        self.read_store.close_presentation_reader();
        self.pending.close();
        let mut worker_slot = self.worker.lock().await;
        // A cancelled waiter releases this guard, not the native JoinHandle.
        // A retry must still join the original worker and its finite Source read.
        if let ProjectionWorker::Running(worker) = &mut *worker_slot {
            *worker_slot = ProjectionWorker::Joined(worker.await);
        }
        // The read slot outlives worker panic/abort. Neither await removes its
        // original handle until completion, including a cancelled drain waiter.
        let read_result = drain_projection_read(&self.projection_read).await;
        let worker_result = match std::mem::take(&mut *worker_slot) {
            ProjectionWorker::Joined(result) => result,
            ProjectionWorker::Vacant => Ok(()),
            ProjectionWorker::Running(_) => unreachable!("worker was joined in place"),
        };
        worker_result.and(read_result)
    }

    /// Drains the conversation runtime and returns only after it is
    /// quiescent. Runtime Client remains a control adapter: the semantic
    /// lifecycle and all settlement ownership remain in `ConversationRuntime`.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::InvalidState`] while the runtime is
    /// not yet activated: an inert conversation has no runtime lifecycle
    /// to end, so the request is refused and nothing is published.
    pub(crate) async fn shutdown(&self) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.ensure_writable_runtime()?;
        self.runtime
            .as_ref()
            .expect("a writable Runtime Client host has a runtime")
            .shutdown()
            .await
            .map_err(|error| match error {
                crate::runtime::conversation_runtime::ShutdownError::Inactive => {
                    RuntimeClientError::InvalidState {
                        message: "the conversation runtime is not activated".to_owned(),
                    }
                }
                crate::runtime::conversation_runtime::ShutdownError::RuntimeOwnedSettlement {
                    detail,
                } => RuntimeClientError::RuntimeFailure {
                    message: format!("runtime-owned shutdown settlement failed: {detail}"),
                },
            })?;
        Ok(RuntimeClientResult::ShutdownCompleted)
    }
}
