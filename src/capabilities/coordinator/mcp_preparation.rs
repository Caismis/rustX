//! Transferable physical MCP preparation. No Session, model, Skill or capability
//! authority exists here. The coordinator still validates and commits a complete candidate.
use super::{McpPreparationOutcome, SOURCE_PREPARATION_CONCURRENCY};
use crate::capabilities::CapabilityPreparationError;
use crate::capabilities::ToolSourceId;
use crate::runtime::cancellation::CancellationSignal;
use crate::runtime::identity::McpServerId;
use crate::runtime::types::ConversationLifecycle;
use crate::tools::executor::{ToolRegistration, ToolRegistry};
use crate::tools::mcp::{
    McpInvalidationState, McpRuntimeGeneration, McpRuntimeRetirementRegistry, McpServerBindings,
    McpServerRuntime,
};
use futures_util::{StreamExt, stream};
use std::future::Future;
#[cfg(test)]
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub(super) struct McpPreparationOwner {
    inner: Arc<McpPreparationResources>,
}
impl std::ops::Deref for McpPreparationOwner {
    type Target = McpPreparationResources;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}
pub(super) struct McpPreparationResources {
    pub(super) workspace: crate::tools::workspace::Workspace,
    #[cfg(test)]
    pub(super) mcp_preparations: AtomicU64,
    #[cfg(test)]
    pub(super) mcp_completion: Mutex<Option<tokio::sync::mpsc::UnboundedSender<McpServerId>>>,
    #[cfg(test)]
    pub(super) source_waiting: Mutex<Option<tokio::sync::mpsc::UnboundedSender<()>>>,
    /// The ownership cancellation root of every in-flight conversation-owned
    /// preparation: MCP connects (Issue #12, M9c) and Python/uv environment
    /// builds (Issue #145).
    ///
    /// Each in-flight preparation owner takes a child of this signal, so
    /// runtime drain can close in-flight *preparation* the same way it
    /// closes retained runtimes: by cancelling the owner, never by dropping
    /// a caller future. Cancelling drives an already-spawned stdio process
    /// or uv unit to its physical settlement proof before the owner
    /// releases its counted lifecycle admission.
    pub(super) preparation_cancellation: crate::runtime::cancellation::CancellationSignal,
    pub(super) source_preparation_slots: tokio::sync::Semaphore,
    /// Composition creates the lifecycle before any source ownership begins.
    pub(super) preparation_lifecycle: Mutex<Option<ConversationLifecycle>>,
    /// Test-only: parks the next conversation-owned MCP connect at the
    /// instant physical process ownership exists.
    #[cfg(test)]
    pub(super) connect_ownership_pause:
        Mutex<Option<Arc<crate::tools::mcp::test_sync::ConnectOwnershipPause>>>,
    /// The one shared MCP invalidation synchronization boundary: epoch
    /// mutation (`tools/list_changed`) and epoch validation + snapshot swap
    /// (commit) serialize through the same guard.
    pub(super) mcp_invalidation: Arc<McpInvalidationState>,
    /// Retired candidate/publication generations whose asynchronous physical
    /// close has not yet been reaped. This field follows the published state
    /// so current generation owners retire while the registry is still alive
    /// if the coordinator itself is dropped after normal runtime shutdown.
    pub(super) mcp_retirements: Arc<McpRuntimeRetirementRegistry>,
}
impl McpPreparationOwner {
    pub(super) fn new(workspace: crate::tools::workspace::Workspace) -> Self {
        Self::with_cancellation(workspace, CancellationSignal::new())
    }
    fn with_cancellation(
        workspace: crate::tools::workspace::Workspace,
        cancellation: CancellationSignal,
    ) -> Self {
        Self {
            inner: Arc::new(McpPreparationResources {
                workspace,
                #[cfg(test)]
                mcp_preparations: AtomicU64::new(0),
                #[cfg(test)]
                mcp_completion: Mutex::new(None),
                #[cfg(test)]
                source_waiting: Mutex::new(None),
                #[cfg(test)]
                connect_ownership_pause: Mutex::new(None),
                preparation_cancellation: cancellation,
                source_preparation_slots: tokio::sync::Semaphore::new(
                    SOURCE_PREPARATION_CONCURRENCY,
                ),
                preparation_lifecycle: Mutex::new(None),
                mcp_invalidation: Arc::new(McpInvalidationState::new()),
                mcp_retirements: McpRuntimeRetirementRegistry::new(),
            }),
        }
    }
    fn preparation_lifecycle(&self) -> Option<ConversationLifecycle> {
        self.preparation_lifecycle
            .lock()
            .expect("preparation lifecycle")
            .clone()
    }
    /// Prepares one configured MCP server: connect a candidate-owned runtime,
    /// snapshot the invalidation epoch, and fetch its complete catalog under
    /// the epoch-stability check. The physical runtime is not visible in the
    /// coordinator's published state until the complete candidate commits.
    ///
    /// Each server is one optional capability source: the caller records a
    /// failure as that server's own unavailable state and continues with
    /// the remaining servers.
    ///
    /// The candidate owner's cancellation is observed in addition to the
    /// coordinator's conversation root, on both optional and required paths.
    pub(super) async fn prepare_mcp_server(
        &self,
        server_id: &McpServerId,
        binding: &crate::tools::mcp::McpServerBinding,
        preparation_cancellation: Option<&crate::runtime::cancellation::CancellationSignal>,
    ) -> McpPreparationOutcome {
        if preparation_cancellation
            .is_some_and(crate::runtime::cancellation::CancellationSignal::is_cancelled)
        {
            return Err("MCP preparation cancelled".to_owned());
        }
        #[cfg(test)]
        self.inner.mcp_preparations.fetch_add(1, Ordering::Relaxed);
        let generation = self
            .connect_conversation_owned(server_id, binding, preparation_cancellation)
            .await
            .map_err(|error| error.to_string())?;
        let (epoch, generation, discovery) = self
            .prepare_mcp_generation(server_id, generation, preparation_cancellation)
            .await?;
        // Validate the canonical registration while this source still owns its
        // preparation slot and deadline. Final profile selection can then use
        // only already-validated live declarations.
        let registrations = crate::tools::mcp::definitions_owned_for_source(
            &ToolSourceId::Mcp(server_id.clone()),
            server_id,
            binding.policy,
            &generation.binding(),
            discovery.tools.clone(),
        )
        .into_iter()
        .map(|(definition, executor)| ToolRegistration::plain(definition, executor));
        if let Err(error) = ToolRegistry::from_registrations(registrations) {
            let close = generation.retire_and_close().await;
            return Err(
                close.map_or_else(|| error.to_string(), |close| format!("{error}; {close}"))
            );
        }
        Ok((epoch, generation, discovery))
    }

    pub(super) async fn prepare_mcp_generation(
        &self,
        server_id: &McpServerId,
        generation: McpRuntimeGeneration,
        preparation_cancellation: Option<&crate::runtime::cancellation::CancellationSignal>,
    ) -> McpPreparationOutcome {
        // The epoch snapshot is taken under the shared invalidation
        // guard; the pagination itself never holds it.
        let epoch_before = self.inner.mcp_invalidation.epoch(server_id);
        // Discovery resolves its transport through the same health-
        // arbitrating connection path a dispatch uses, so a candidate is
        // never built from a generation the connection already knows is
        // unusable.
        let acquired = {
            let mut acquire = std::pin::pin!(generation.connection().acquire());
            match preparation_cancellation {
                Some(cancellation) => tokio::select! {
                    biased;
                    () = cancellation.cancelled() => {
                        // acquire may own a replacement stdio process. Retire
                        // fires connection close, but keep polling that owner
                        // until it publishes physical settlement evidence.
                        generation.retire();
                        acquire.await
                    }
                    outcome = &mut acquire => outcome,
                },
                None => acquire.await,
            }
        };
        let listing = match acquired {
            Ok(transport) => match preparation_cancellation {
                Some(cancellation) => tokio::select! {
                    biased;
                    () = cancellation.cancelled() => Err(crate::tools::mcp::McpError::Execution(
                        "MCP preparation cancelled".to_owned(),
                    )),
                    outcome = generation.connection().discover_acquired_catalog(transport) => outcome,
                },
                None => {
                    generation
                        .connection()
                        .discover_acquired_catalog(transport)
                        .await
                }
            },
            Err(error) => Err(error),
        };
        let tools = match listing {
            Ok(tools) => tools,
            Err(error) => {
                if matches!(error, crate::tools::mcp::McpError::PhysicalSettlement(_)) {
                    self.inner
                        .mcp_retirements
                        .record_failure(server_id, &error.to_string());
                }
                let close_error = generation.retire_and_close().await;
                return Err(match close_error {
                    Some(close_error) => format!("{error}; candidate close failed: {close_error}"),
                    None => error.to_string(),
                });
            }
        };
        let epoch_after = self.inner.mcp_invalidation.epoch(server_id);
        if epoch_before != epoch_after {
            let close_error = generation.retire_and_close().await;
            return Err(match close_error {
                Some(close_error) => format!(
                    "MCP tool catalog changed during discovery; candidate close failed: {close_error}"
                ),
                None => "MCP tool catalog changed during discovery".to_owned(),
            });
        }
        Ok((epoch_after, generation, tools))
    }

    /// Connects one conversation-owned MCP server through an owner whose
    /// lifetime is independent of this caller (Issue #12, M9c).
    ///
    /// # Ownership phases
    ///
    /// ```text
    /// no physical owner
    ///   -> conversation-counted preparation owner (spawned task, counted
    ///      lifecycle admission, ownership cancellation child)
    ///   -> physical MCP process ownership established (inside connect)
    ///   -> candidate-owned generation
    ///   -> either  A. transferred into published generation state
    ///      or      B. retired and driven to physical settlement
    /// ```
    ///
    /// The counted admission is released only after A or B, so aborting or
    /// dropping *this* future never removes the physical owner from the
    /// conversation's quiescence proof. The waiter is not the owner.
    ///
    /// `preparation_cancellation` is an additional lifecycle authority the
    /// physical connect must observe (the child preparation's pre-commit
    /// authority, Issue #145). It is forwarded into the owner's own
    /// cancellation signal: once fired, the owner drives its physical
    /// process to settlement before answering, exactly as it does for the
    /// coordinator's preparation root.
    pub(super) async fn connect_conversation_owned(
        &self,
        server_id: &McpServerId,
        binding: &crate::tools::mcp::McpServerBinding,
        preparation_cancellation: Option<&crate::runtime::cancellation::CancellationSignal>,
    ) -> Result<McpRuntimeGeneration, CapabilityPreparationError> {
        let lifecycle = self.preparation_lifecycle();
        // The owner's own counted admission, acquired before any process is
        // spawned: if drain already won, nothing is spawned at all.
        let owner_admission = match &lifecycle {
            Some(lifecycle) => Some(
                lifecycle
                    .try_enter_preparation()
                    .map_err(|_| CapabilityPreparationError::ConversationInactive)?,
            ),
            None => None,
        };
        let cancellation = self.inner.preparation_cancellation.child();
        #[cfg(test)]
        let ownership_pause = self
            .inner
            .connect_ownership_pause
            .lock()
            .expect("connect ownership pause lock")
            .clone();
        let inner = Arc::clone(&self.inner);
        let server_id_owned = server_id.clone();
        let binding_owned = binding.clone();
        let preparation_cancellation = preparation_cancellation.cloned();
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        let handle = tokio::runtime::Handle::current();
        tokio::spawn(async move {
            let request = crate::tools::mcp::OwnedConnect::new(
                &server_id_owned,
                &binding_owned,
                &inner.workspace,
                inner.mcp_invalidation.clone(),
                cancellation.clone(),
            );
            #[cfg(test)]
            let request = request.with_ownership_pause(ownership_pause);
            let outcome = {
                let mut connect = std::pin::pin!(McpServerRuntime::connect_owned(request));
                match preparation_cancellation {
                    // The child preparation's authority fires into the owner's
                    // own cancellation signal; the connect then drives its
                    // physical process to settlement before answering, exactly
                    // as it does for the coordinator's preparation root.
                    Some(authority) => {
                        tokio::select! {
                            outcome = &mut connect => outcome,
                            () = authority.cancelled() => {
                                cancellation.cancel();
                                connect.await
                            }
                        }
                    }
                    None => connect.await,
                }
            };
            let outcome = match outcome {
                // The preparation owner transfers its lifecycle admission into
                // the candidate generation before answering the caller. If
                // the caller has already been cancelled, the send-failure
                // branch below retires that generation and awaits its close.
                // The connection — not the transport — is what the
                // published capability generation owns (Issue #205). It
                // retains exactly the inputs a bounded replacement needs and
                // its own ownership cancellation root, so a later dispatch
                // can re-establish a transport without any capability
                // knowledge changing and without any detached task.
                Ok(runtime) => Ok(McpRuntimeGeneration::from_connection(
                    server_id_owned.clone(),
                    crate::tools::mcp::McpConnection::reconnectable(
                        runtime,
                        binding_owned.clone(),
                        inner.workspace.clone(),
                        inner.mcp_invalidation.clone(),
                        cancellation.clone(),
                    ),
                    owner_admission,
                    handle,
                    &inner.mcp_retirements,
                )),
                Err(error) => {
                    // Failed connects may never create a generation. Retain
                    // their explicit uncertainty before formatting the public
                    // preparation error, so drain cannot mistake an empty
                    // generation set for proven physical settlement.
                    if matches!(error, crate::tools::mcp::McpError::PhysicalSettlement(_)) {
                        inner
                            .mcp_retirements
                            .record_failure(&server_id_owned, &error.to_string());
                    }
                    Err(CapabilityPreparationError::Mcp(error.to_string()))
                }
            };
            if let Err(outcome) = result_tx.send(outcome)
                && let Ok(generation) = outcome
            {
                let _ = generation.retire_and_close().await;
            }
        });
        result_rx.await.unwrap_or_else(|_| {
            let detail = "the MCP connection owner terminated without an outcome";
            self.inner.mcp_retirements.record_failure(server_id, detail);
            Err(CapabilityPreparationError::Mcp(detail.to_owned()))
        })
    }

    /// Poll independent source work together, then restore captured identity
    /// order before any registration, availability, or diagnostic is composed.
    /// The shared slots bound physical work across both phases and candidates.
    /// Cancelled queue entries return their unstarted input to the source adapter;
    /// futures that already acquired a slot retain it until they settle.
    pub(super) async fn prepare_sources<I, F, Fut, T>(
        &self,
        sources: I,
        cancellation: &crate::runtime::cancellation::CancellationSignal,
        prepare: F,
    ) -> Vec<Result<T, I::Item>>
    where
        I: IntoIterator,
        F: Fn(I::Item) -> Fut,
        Fut: Future<Output = T>,
    {
        let mut outcomes: Vec<_> = stream::iter(sources.into_iter().enumerate())
            .map(|(index, source)| {
                let prepare = &prepare;
                async move {
                    let mut acquire = std::pin::pin!(self.inner.source_preparation_slots.acquire());
                    #[cfg(test)]
                    let mut reported_wait = false;
                    let acquire = std::future::poll_fn(|context| {
                        let outcome = acquire.as_mut().poll(context);
                        #[cfg(test)]
                        if outcome.is_pending() && !reported_wait {
                            reported_wait = true;
                            if let Some(waiting) = &*self.inner.source_waiting.lock().unwrap() {
                                let _ = waiting.send(());
                            }
                        }
                        outcome
                    });
                    let permit = tokio::select! {
                        biased;
                        () = cancellation.cancelled() => None,
                        permit = acquire => Some(
                            permit.expect("source preparation semaphore is never closed")
                        ),
                    };
                    let outcome = match permit {
                        Some(_permit) => Ok(prepare(source).await),
                        None => Err(source),
                    };
                    (index, outcome)
                }
            })
            .buffer_unordered(SOURCE_PREPARATION_CONCURRENCY)
            .collect()
            .await;
        outcomes.sort_by_key(|(index, _)| *index);
        outcomes.into_iter().map(|(_, outcome)| outcome).collect()
    }
}

/// The finite server reservation survives every initializer and retired generation.
/// Unknown settlement intentionally consumes capacity until the process exits.
struct BatchCapacity {
    permit: Option<tokio::sync::OwnedSemaphorePermit>,
    settled: Option<Arc<dyn Fn(Vec<String>) + Send + Sync>>,
}
impl BatchCapacity {
    fn finish(&mut self, failures: Vec<String>) {
        if let Some(permit) = self.permit.take() {
            if failures.is_empty() {
                drop(permit);
            } else {
                permit.forget();
            }
        }
        if let Some(settled) = self.settled.take() {
            settled(failures);
        }
    }
    fn release(&mut self) {
        self.finish(Vec::new());
    }
}
impl Drop for BatchCapacity {
    fn drop(&mut self) {
        if self.permit.is_some() {
            self.finish(vec![
                "MCP preparation terminated without physical settlement evidence".into(),
            ]);
        }
    }
}
struct PreparedBatch {
    outcomes: Vec<(McpServerId, McpPreparationOutcome)>,
    capacity: BatchCapacity,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum McpPreparationState {
    Pending,
    Prepared,
    Failed,
}

/// Unique draft/claim ownership of the actual running discovery task. Dropping
/// a waiter cancels and supervises settlement; it never drops an initializer.
pub(crate) struct McpPreparation {
    pub(super) owner: McpPreparationOwner,
    result: Option<tokio::sync::oneshot::Receiver<PreparedBatch>>,
    complete: tokio::sync::watch::Receiver<McpPreparationState>,
}
impl std::fmt::Debug for McpPreparation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpPreparation").finish_non_exhaustive()
    }
}
impl McpPreparation {
    pub(crate) fn start(
        workspace: crate::tools::workspace::Workspace,
        bindings: McpServerBindings,
        deadline: super::McpStartupDeadline,
        capacity: tokio::sync::OwnedSemaphorePermit,
        cancellation: CancellationSignal,
        settled: Arc<dyn Fn(Vec<String>) + Send + Sync>,
    ) -> Self {
        let owner = McpPreparationOwner::with_cancellation(workspace, cancellation);
        *owner
            .preparation_lifecycle
            .lock()
            .expect("preparation lifecycle") = Some(ConversationLifecycle::new());
        let worker = owner.clone();
        let (send, result) = tokio::sync::oneshot::channel();
        let (completed, complete) = tokio::sync::watch::channel(McpPreparationState::Pending);
        // Registration and finite reservation precede this spawn. This task
        // retains both even if the browser or native reply waiter disappears.
        let capacity = BatchCapacity {
            permit: Some(capacity),
            settled: Some(settled),
        };
        tokio::spawn(async move {
            let batch_cancellation = worker.preparation_cancellation.child();
            let cancellation = &batch_cancellation;
            let preparation = worker.prepare_sources(bindings, cancellation, |(id, binding)| {
                let deadline = &deadline;
                let worker = &worker;
                async move {
                    let outcome = worker
                        .prepare_mcp_server(&id, &binding, Some(cancellation))
                        .await;
                    let outcome =
                        super::startup::finish_source_before_deadline(outcome, deadline).await;
                    (id, outcome)
                }
            });
            let outcomes: Vec<_> =
                super::startup::settle_by_deadline(preparation, &deadline, cancellation)
                    .await
                    .into_iter()
                    .map(|result| {
                        result.unwrap_or_else(|(id, _)| {
                            let reason = if deadline.has_expired() {
                                super::startup::MCP_PREPARATION_TIMEOUT
                            } else {
                                "MCP preparation cancelled before source admission"
                            };
                            (id, Err(reason.to_owned()))
                        })
                    })
                    .collect();
            let state = if outcomes.iter().all(|(_, result)| result.is_ok()) {
                McpPreparationState::Prepared
            } else {
                McpPreparationState::Failed
            };
            let result = PreparedBatch { outcomes, capacity };
            if let Err(result) = send.send(result) {
                let _ = settle_batch(&worker, result).await;
            }
            completed.send_replace(state);
        });
        Self {
            owner,
            result: Some(result),
            complete,
        }
    }
    #[cfg(test)]
    pub(crate) fn completion(&self) -> tokio::sync::watch::Receiver<McpPreparationState> {
        self.complete.clone()
    }
    pub(crate) fn state(&self) -> McpPreparationState {
        *self.complete.borrow()
    }
    pub(crate) fn cancel(&self) {
        self.owner.preparation_cancellation.cancel();
        self.owner.mcp_retirements.retire_all();
    }
    pub(crate) async fn settle(mut self) -> Vec<String> {
        self.cancel();
        let result = self.result.take().expect("unique MCP preparation");
        match result.await {
            Ok(result) => settle_batch(&self.owner, result).await,
            Err(_) => vec!["MCP preparation owner terminated without settlement evidence".into()],
        }
    }
    pub(super) async fn receive(
        mut self,
        cancellation: Option<&CancellationSignal>,
    ) -> Vec<(McpServerId, McpPreparationOutcome)> {
        let result = self.result.take().expect("unique MCP preparation");
        tokio::pin!(result);
        let outcome = match cancellation {
            Some(cancellation) => tokio::select! {
                outcome = &mut result => outcome,
                () = cancellation.cancelled() => { self.cancel(); result.await },
            },
            None => result.await,
        };
        if let Ok(mut result) = outcome {
            // Physical ownership is already installed in the Conversation's
            // coordinator. Only its prewarm reservation is released here.
            result.capacity.release();
            std::mem::take(&mut result.outcomes)
        } else {
            self.owner.mcp_retirements.record_failure(
                &McpServerId::new("initial-preparation"),
                "MCP preparation owner terminated without an outcome",
            );
            Vec::new()
        }
    }
}
impl Drop for McpPreparation {
    fn drop(&mut self) {
        let Some(result) = self.result.take() else {
            return;
        };
        self.cancel();
        let owner = self.owner.clone();
        tokio::spawn(async move {
            if let Ok(result) = result.await {
                let failures = settle_batch(&owner, result).await;
                if !failures.is_empty() {
                    tracing::error!(
                        "draft MCP physical settlement failed: {}",
                        failures.join("; ")
                    );
                }
            }
        });
    }
}
async fn settle_batch(owner: &McpPreparationOwner, mut result: PreparedBatch) -> Vec<String> {
    owner.preparation_cancellation.cancel();
    owner.mcp_retirements.retire_all();
    result.outcomes.clear();
    if let Some(lifecycle) = owner.preparation_lifecycle() {
        lifecycle.wait_for_no_admissions().await;
    }
    let failures = owner.mcp_retirements.settle_all().await;
    result.capacity.finish(failures.clone());
    failures
}
