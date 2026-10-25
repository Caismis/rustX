//! Residency transitions: load, compose, unload, retire and replace. Every
//! transition task owns a terminal guard; callers only watch its flight, and
//! no await or external work runs under the registry mutex.

use super::super::composition::LocalConversationCore;
use super::super::session::DisplayPreviewSubject;
use super::super::session::SessionId;
use super::super::session::SessionNodeId;
use super::super::session_controller::DisplayPreviewRepair;
use super::super::session_controller::SessionAccess;
use super::CompositionOutcome;
use super::Entry;
use super::Flight;
use super::ManagedRuntime;
use super::NEXT_INCARNATION;
use super::Outcome;
use super::ResidentRuntime;
use super::RuntimeIncarnationId;
use super::RuntimeManagerError;
use super::RuntimeRegistry;
use super::SessionRuntimeManager;
use super::WriterCertainty;
use super::error;
use crate::runtime::identity::ConversationId;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use tokio::sync::watch;

impl SessionRuntimeManager {
    /// Register the accepted exact-node receipt before any async catalog lookup.
    /// Existing and future loading owners observe it under the same registry cut.
    pub(crate) fn accept_submission_intent(
        &self,
        session: &SessionId,
        node: &SessionNodeId,
        intent: super::super::composition::SubmissionIntent,
    ) -> Result<super::SubmissionReceipt, RuntimeManagerError> {
        let accepted = Arc::new(super::AcceptedSubmission {
            session: session.clone(),
            node: node.clone(),
            intent,
            initialization: Mutex::new(None),
        });
        let mut registry = self.registry.0.lock().expect("registry mutex");
        if registry.draining {
            return Err(RuntimeManagerError::Draining);
        }
        if registry.retiring_sessions.contains(session) {
            return Err(error("Session retirement has fenced runtime admission"));
        }
        if let Some(id) = registry.by_session.get(session)
            && let Some(Entry::Loading(flight) | Entry::Unloading { flight, .. }) =
                registry.entries.get(id)
            && flight
                .target
                .get()
                .is_some_and(|target| target.0 == *session && target.1 == *node)
        {
            accepted.bind(&flight.initialization);
            flight.startup.record(intent).map_err(error)?;
        } else if let Some(id) = registry.by_session.get(session)
            && let Some(Entry::Loaded(runtime)) = registry.entries.get(id)
            && runtime.node == *node
        {
            accepted.bind(&runtime.initialization);
        }
        registry
            .submission_receipts
            .retain(|receipt| receipt.strong_count() > 0);
        registry.submission_receipts.push(Arc::downgrade(&accepted));
        Ok(super::SubmissionReceipt {
            accepted,
            registry: Arc::downgrade(&self.registry),
        })
    }

    /// Explicit user recovery replaces only a known failed initialization.
    /// Automatic attach/reconnect never invokes this boundary or replays input.
    pub(crate) fn retry_failed_initialization(
        &self,
        session: &SessionId,
        intent: super::super::composition::SubmissionIntent,
    ) -> Result<(), RuntimeManagerError> {
        let mut registry = self.registry.0.lock().expect("registry mutex");
        if registry.draining {
            return Err(RuntimeManagerError::Draining);
        }
        let Some(id) = registry.by_session.get(session).cloned() else {
            return Ok(());
        };
        registry.check_session(session, &id)?;
        let Some(Entry::FailedSubmission(previous)) = registry.entries.get(&id) else {
            return Ok(());
        };
        let (_, node) = previous
            .target
            .get()
            .expect("failed initialization target")
            .clone();
        registry.reserve()?;
        let flight = Flight::new();
        flight.startup.record(intent).map_err(error)?;
        registry.initialize_loading(&flight, session, &node)?;
        registry
            .entries
            .insert(id.clone(), Entry::Loading(flight.clone()));
        self.spawn_load(id, session.clone(), node, flight);
        Ok(())
    }

    /// Start or join the ordinary bounded load owner without awaiting MCP.
    /// The caller captures the native receipt before its first await.
    pub(crate) async fn record_submission_intent(
        &self,
        receipt: super::SubmissionReceipt,
    ) -> Result<(), RuntimeManagerError> {
        if receipt.accepted.is_cancelled() {
            return Err(RuntimeManagerError::LoadingCancelled);
        }
        let session = &receipt.accepted.session;
        let node = &receipt.accepted.node;
        let intent = receipt.accepted.intent;
        let target = self
            .sessions
            .resolve_session_target(session, Some(node))
            .await
            .map_err(error)?;
        #[cfg(test)]
        self.probe(&target.conversation_id)
            .before_submission_registration
            .park()
            .await;
        let start = {
            let mut registry = self.registry.0.lock().expect("registry mutex");
            if receipt.accepted.is_cancelled() {
                return Err(RuntimeManagerError::LoadingCancelled);
            }
            registry.check_session(session, &target.conversation_id)?;
            match registry.entries.get(&target.conversation_id) {
                Some(Entry::Loading(flight)) => {
                    if !receipt.accepted.bind(&flight.initialization) {
                        return Err(RuntimeManagerError::LoadingCancelled);
                    }
                    flight.startup.record(intent).map_err(error)?;
                    None
                }
                Some(Entry::Loaded(runtime)) => {
                    return if receipt.accepted.bind(&runtime.initialization) {
                        Ok(())
                    } else {
                        Err(RuntimeManagerError::LoadingCancelled)
                    };
                }
                Some(Entry::Unloading { flight, .. }) => {
                    if flight.prepares_runtime.load(Ordering::Acquire)
                        && !receipt.accepted.bind(&flight.initialization)
                    {
                        return Err(RuntimeManagerError::LoadingCancelled);
                    }
                    flight.startup.record(intent).map_err(error)?;
                    flight
                        .submission_restart
                        .lock()
                        .expect("submission restart")
                        .get_or_insert((session.clone(), target.id.clone()));
                    None
                }
                None | Some(Entry::FailedSubmission(_)) => {
                    registry.reserve()?;
                    let flight = Flight::new();
                    flight.startup.note_recovery_receipt(intent);
                    if !receipt.accepted.bind(&flight.initialization) {
                        return Err(RuntimeManagerError::LoadingCancelled);
                    }
                    flight.startup.record(intent).map_err(error)?;
                    registry.initialize_loading(&flight, session, &target.id)?;
                    registry
                        .by_session
                        .insert(session.clone(), target.conversation_id.clone());
                    registry.entries.insert(
                        target.conversation_id.clone(),
                        Entry::Loading(flight.clone()),
                    );
                    Some(flight)
                }
            }
        };
        if let Some(flight) = start {
            self.spawn_load(target.conversation_id, session.clone(), target.id, flight);
        }
        Ok(())
    }

    /// Warm loads reuse the current incarnation. Loads racing unload wait for
    /// its terminal result, then cold load; failed unload remains a closed slot.
    /// # Errors
    /// Acquisition, resolution, admission, composition and shutdown errors.
    /// # Panics
    /// Panics if an internal residency mutex was poisoned.
    pub async fn load(
        &self,
        session: &SessionId,
        node: Option<&SessionNodeId>,
    ) -> Result<Arc<ManagedRuntime>, RuntimeManagerError> {
        self.load_with_startup(session, node, None).await
    }

    async fn load_with_startup(
        &self,
        session: &SessionId,
        node: Option<&SessionNodeId>,
        mut startup: Option<super::super::composition::McpInitialSubmission>,
    ) -> Result<Arc<ManagedRuntime>, RuntimeManagerError> {
        let recovery_received = super::super::composition::SubmissionIntent::now();
        loop {
            // Identity resolution takes no ConversationAccess. Only a registered
            // flight may acquire allocation authority and compose a runtime.
            let target = self
                .sessions
                .resolve_session_target(session, node)
                .await
                .map_err(error)?;
            let id = target.conversation_id.clone();
            #[cfg(test)]
            let id_for_probe = id.clone();
            let flight = {
                let mut registry = self.registry.0.lock().expect("registry mutex");
                registry.check_session(session, &id)?;
                match registry.entries.get(&id) {
                    Some(Entry::Loaded(runtime)) => {
                        let identity = runtime.identity.clone();
                        drop(registry);
                        self.rebind_configuration_runtime(session);
                        return Ok(identity);
                    }
                    Some(
                        Entry::FailedSubmission(flight)
                        | Entry::Loading(flight)
                        | Entry::Unloading { flight, .. },
                    ) => flight.clone(),
                    None => {
                        registry.reserve()?;
                        let flight = Flight::with_startup(startup.take().unwrap_or_default());
                        flight.startup.note_recovery_receipt(recovery_received);
                        registry.initialize_loading(&flight, session, &target.id)?;
                        // Session claim and Conversation flight publish atomically.
                        registry.by_session.insert(session.clone(), id.clone());
                        // Same-id load linearization: exactly one claimant can
                        // install this Loading flight before releasing the lock.
                        registry
                            .entries
                            .insert(id.clone(), Entry::Loading(flight.clone()));
                        self.spawn_load(id, session.clone(), target.id, flight.clone());
                        flight
                    }
                }
            };
            #[cfg(test)]
            self.probe(&id_for_probe).joined.send_modify(|n| *n += 1);
            let inherited_startup = flight.startup.clone();
            let result = flight.wait().await.operation_result()?;
            if flight.initialization.is_retiring() {
                return Err(RuntimeManagerError::LoadingCancelled);
            }
            if let Some(runtime) = result {
                self.registry
                    .0
                    .lock()
                    .expect("registry mutex")
                    .check_session(session, runtime.conversation_id())?;
                self.rebind_configuration_runtime(session);
                return Ok(runtime);
            }
            startup.get_or_insert(inherited_startup);
        }
    }

    fn rebind_configuration_runtime(&self, session: &SessionId) {
        if let Some(runtime) = self.configuration_runtime(session)
            && self.applications.rebind_runtime(session.as_ref(), &runtime)
        {
            self.applications.run(self.clone());
        }
    }

    pub(super) fn spawn_load(
        &self,
        id: ConversationId,
        session: SessionId,
        node: SessionNodeId,
        flight: Arc<Flight>,
    ) {
        flight.prepares_runtime.store(true, Ordering::Release);
        let owner = self.clone();
        let startup = flight.startup.clone();
        let initialization = flight.initialization.clone();
        let terminal = TerminalGuard::new(self, id, flight, WriterCertainty::Absent);
        // The flight is registered before this task can acquire allocation access.
        tokio::spawn(async move {
            let result = owner
                .acquire_and_compose(&session, &node, startup, &initialization)
                .await
                .map(Some);
            terminal.finish(result);
        });
    }

    async fn acquire_and_compose(
        &self,
        session: &SessionId,
        node: &SessionNodeId,
        startup: super::super::composition::McpInitialSubmission,
        initialization: &Arc<super::Initialization>,
    ) -> Result<Arc<ResidentRuntime>, RuntimeManagerError> {
        if self.registry.0.lock().expect("registry mutex").draining {
            return Err(RuntimeManagerError::Draining);
        }
        #[cfg(test)]
        {
            let target = self
                .sessions
                .resolve_session_target(session, Some(node))
                .await
                .map_err(error)?;
            let probe = self.probe(&target.conversation_id);
            probe.before_allocation.park().await;
            probe
                .acquisitions
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        if initialization.is_retiring() {
            return Err(RuntimeManagerError::LoadingCancelled);
        }
        let access = self
            .sessions
            .acquire_session(session, Some(node))
            .await
            .map_err(error)?;
        self.compose(access, startup, initialization).await
    }

    #[allow(clippy::too_many_lines)] // one ordered ownership transaction
    async fn compose(
        &self,
        access: SessionAccess,
        startup: super::super::composition::McpInitialSubmission,
        initialization: &Arc<super::Initialization>,
    ) -> Result<Arc<ResidentRuntime>, RuntimeManagerError> {
        #[cfg(test)]
        {
            let probe = self.probe(&access.node.conversation_id);
            probe
                .compositions
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            probe.before_compose.park().await;
            if probe
                .fail_compose_once
                .swap(false, std::sync::atomic::Ordering::SeqCst)
            {
                return Err(error("injected composition failure before writer creation"));
            }
            assert!(
                !probe
                    .panic_once
                    .swap(false, std::sync::atomic::Ordering::SeqCst),
                "injected composition task panic"
            );
        }
        let configuration = self.configuration.clone();
        let credentials = self.credentials.clone();
        let bindings = self.sessions.configuration_bindings.clone();
        let applications = self.applications.clone();
        let owner = self.clone();
        let (access, paths) = tokio::task::spawn_blocking(move || {
            let retained = bindings
                .lock()
                .expect("Session configuration bindings")
                .get(&access.session.id)
                .cloned();
            let paths = if let Some(paths) = retained {
                paths
            } else {
                // Only a fresh resolution reads sources; repair them first.
                owner.repair_session_sources(&access.settings.input());
                let paths = applications
                    .lock()
                    .initial_binding(&configuration, &access.settings.input(), &credentials, true)
                    .map_err(error)?;
                bindings
                    .lock()
                    .expect("Session configuration bindings")
                    .insert(access.session.id.clone(), paths.clone());
                paths
            };
            Ok::<_, RuntimeManagerError>((access, paths))
        })
        .await
        .map_err(error)??;
        if paths.runtime_root != access.allocation.root() {
            return Err(error(
                "configuration sources and Session controller belong to different user roots",
            ));
        }
        startup
            .bind_policy(paths.config.mcp_startup_policy)
            .map_err(error)?;
        #[cfg(test)]
        self.probe(&access.node.conversation_id)
            .before_recovery_inspection
            .park()
            .await;
        let identity = paths.identity().to_owned();
        let registry = super::super::composition::load_model_registry(&paths, &self.dependencies)
            .map_err(error)?;
        if self.registry.0.lock().expect("registry mutex").draining {
            return Err(RuntimeManagerError::Draining);
        }
        let core = LocalConversationCore::compose_with_access(
            &paths,
            &self.dependencies,
            registry,
            paths.config().clone(),
            super::super::session::SessionPersistentState::from_input(&paths.input),
            access.node.conversation_id.clone(),
            access
                .database_path
                .parent()
                .expect("allocation path")
                .to_path_buf(),
            access.allocation,
            startup,
            &initialization.cancellation,
        )
        .await
        .map_err(|failure| match failure {
            super::super::composition::LocalRuntimeError::Cancelled => {
                RuntimeManagerError::LoadingCancelled
            }
            failure => error(failure),
        })?;
        core.runtime()
            .restore_configuration_binding(paths.binding_revision);
        #[cfg(test)]
        if let Some(gate) = self
            .probe(&access.node.conversation_id)
            .activation
            .lock()
            .unwrap()
            .clone()
        {
            core.runtime().install_activation_gate(gate);
        }
        #[cfg(test)]
        if let Some(gate) = self
            .probe(&access.node.conversation_id)
            .initialization_admission
            .lock()
            .unwrap()
            .clone()
        {
            core.runtime().install_admission_gate(gate);
        }
        let composition = core.into_bound().map_err(error)?;
        // The App Server display-projection seam (Issue #386). Two different
        // conditions live here and must not be confused:
        //
        // *Repair* is Session-owned metadata. It runs for **any** composed
        // node, because the subject it derives is the Session's root lineage,
        // not the node being composed: a Session reopened straight onto a
        // branch would otherwise keep a missing projection forever even though
        // its root already holds the authoritative first message. Repair is
        // idempotent and write-free when the projection is already correct, so
        // ordinary routing still writes nothing.
        //
        // *Arming the live publisher* is root-runtime-specific. Only the root
        // runtime can observe the Session's first ordinary user boundary, so
        // only a root composition whose root lineage has no boundary yet arms
        // the one-shot publisher on this still-inert runtime.
        //
        // Both are best-effort: canonical history is unaffected, and the row
        // falls back to identity until a later seam succeeds.
        let session_id = access.session.id.clone();
        if access.node.parent.is_none() {
            super::super::session_title::arm(
                self.sessions.downgrade_catalog(),
                session_id.clone(),
                composition.runtime(),
            );
        }
        if let Err(error) = self.sessions.repair_activity(&session_id).await {
            tracing::warn!(%session_id, %error, "Session activity repair failed during composition");
        }
        super::super::session_activity::arm_activity(
            self.sessions.downgrade_catalog(),
            session_id.clone(),
            composition.runtime(),
        );
        match self
            .sessions
            .repair_display_preview_report(&session_id)
            .await
        {
            Ok(report) => {
                if access.node.parent.is_none()
                    && report.repair == DisplayPreviewRepair::EmptySubject
                    && report.subject == Some(DisplayPreviewSubject::NoBoundary)
                {
                    super::super::session_display_projection::arm_display_projection(
                        self.sessions.downgrade_catalog(),
                        session_id,
                        composition.runtime(),
                    );
                }
            }
            Err(error) => {
                tracing::warn!(
                    %error,
                    "Session display-preview repair failed during runtime composition"
                );
            }
        }
        let incarnation = RuntimeIncarnationId(
            NEXT_INCARNATION
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |next| {
                    next.checked_add(1)
                })
                .expect("process incarnation identity exhausted"),
        );
        Ok(Arc::new_cyclic(|resident| ResidentRuntime {
            node: access.node.id.clone(),
            initialization: initialization.clone(),
            operations: watch::channel(0).0,
            external: std::sync::atomic::AtomicUsize::new(0),
            activity: AtomicU64::new(0),
            idle: Mutex::new(None),
            identity: Arc::new(ManagedRuntime {
                conversation: access.node.conversation_id,
                incarnation,
                workspace_identity: identity,
                resident: resident.clone(),
                registry: Arc::downgrade(&self.registry),
            }),
            composition: Mutex::new(Some(composition)),
        }))
    }

    /// Unload commits only after native shutdown proves quiescence. Failure
    /// retains the old allocation/composition as Unloading; never a new writer.
    /// # Errors
    /// Shutdown failures are shared by all observers and do not commit unload.
    /// # Panics
    /// Panics if an internal residency mutex was poisoned.
    pub async fn unload(&self, id: &ConversationId) -> Result<(), RuntimeManagerError> {
        self.retire(id).await.operation_result().map(|_| ())
    }

    pub(super) async fn retire(&self, id: &ConversationId) -> Outcome {
        loop {
            let (flight, preparing) = {
                let mut registry = self.registry.0.lock().expect("registry mutex");
                match registry.entries.get(id) {
                    None => return Outcome::WriterAbsent(Ok(())),
                    Some(Entry::FailedSubmission(_)) => {
                        registry.remove(id);
                        return Outcome::WriterAbsent(Ok(()));
                    }
                    Some(Entry::Loading(flight)) => {
                        #[cfg(test)]
                        if let Some(probe) = registry.probes.get(id) {
                            probe.unloads_joined.send_modify(|n| *n += 1);
                        }
                        (flight.clone(), true)
                    }
                    Some(Entry::Unloading { flight, .. }) => {
                        #[cfg(test)]
                        if let Some(probe) = registry.probes.get(id) {
                            probe.unloads_joined.send_modify(|n| *n += 1);
                        }
                        (
                            flight.clone(),
                            flight.prepares_runtime.load(Ordering::Acquire),
                        )
                    }
                    Some(Entry::Loaded(runtime)) => {
                        let runtime = runtime.clone();
                        let flight = Flight::new();
                        // Residency claim; native begin_drain owns the actual
                        // inbound/shutdown winner, not this registry transition.
                        registry.entries.insert(
                            id.clone(),
                            Entry::Unloading {
                                _runtime: runtime.clone(),
                                flight: flight.clone(),
                            },
                        );
                        self.spawn_unload(id.clone(), runtime, flight.clone(), None);
                        (flight, false)
                    }
                }
            };
            // Cancellation joins the owning preparation through physical settlement.
            // The native activation CAS and this cancellation share the
            // initialization signal; neither holds the registry through recovery.
            if preparing {
                flight.cancel_loading();
            }
            // Joining replacement must also unload its resulting incarnation;
            // a replacement publication is not a successful unload result.
            match flight.wait().await {
                Outcome::Resident(_) => {}
                Outcome::WriterAbsent(Err(
                    RuntimeManagerError::LoadingCancelled | RuntimeManagerError::Draining,
                )) => {
                    return Outcome::WriterAbsent(Ok(()));
                }
                terminal => return terminal,
            }
        }
    }

    /// Claim unload only for the explicitly addressed live incarnation.
    /// # Errors
    /// Stale identities fail before any residency transition; shutdown failures
    /// retain the native fail-closed Unloading slot.
    /// # Panics
    /// Panics if the registry mutex is poisoned.
    pub async fn unload_incarnation(
        &self,
        id: &ConversationId,
        expected: RuntimeIncarnationId,
    ) -> Result<(), RuntimeManagerError> {
        let flight = {
            let mut registry = self.registry.0.lock().expect("registry mutex");
            let Some(Entry::Loaded(runtime)) = registry.entries.get(id) else {
                return Err(RuntimeManagerError::StaleIncarnation);
            };
            if runtime.identity.incarnation != expected {
                return Err(RuntimeManagerError::StaleIncarnation);
            }
            let runtime = runtime.clone();
            let flight = Flight::new();
            registry.entries.insert(
                id.clone(),
                Entry::Unloading {
                    _runtime: runtime.clone(),
                    flight: flight.clone(),
                },
            );
            self.spawn_unload(id.clone(), runtime, flight.clone(), None);
            flight
        };
        flight.wait().await.operation_result().map(|_| ())
    }

    /// Explicit targeted replacement. Old shutdown must succeed before cold
    /// resolution/composition; failure after that boundary leaves Unloaded.
    /// # Errors
    /// Acquisition, shutdown, configuration and composition failures are explicit.
    /// # Panics
    /// Panics if an internal residency mutex was poisoned.
    pub async fn replace(
        &self,
        session: &SessionId,
        node: Option<&SessionNodeId>,
    ) -> Result<Arc<ManagedRuntime>, RuntimeManagerError> {
        let recovery_received = super::super::composition::SubmissionIntent::now();
        loop {
            let target = self
                .sessions
                .resolve_session_target(session, node)
                .await
                .map_err(error)?;
            let id = target.conversation_id.clone();
            let (flight, claimed) = {
                let mut registry = self.registry.0.lock().expect("registry mutex");
                registry.check_session(session, &id)?;
                match registry.entries.get(&id) {
                    None | Some(Entry::FailedSubmission(_)) => {
                        registry.reserve()?;
                        let flight = Flight::new();
                        flight.startup.note_recovery_receipt(recovery_received);
                        registry.initialize_loading(&flight, session, &target.id)?;
                        registry.by_session.insert(session.clone(), id.clone());
                        registry
                            .entries
                            .insert(id.clone(), Entry::Loading(flight.clone()));
                        self.spawn_load(id, session.clone(), target.id, flight.clone());
                        (flight, true)
                    }
                    Some(Entry::Loading(flight) | Entry::Unloading { flight, .. }) => {
                        (flight.clone(), false)
                    }
                    Some(Entry::Loaded(runtime)) => {
                        let runtime = runtime.clone();
                        let flight = Flight::new();
                        flight.startup.note_recovery_receipt(recovery_received);
                        registry.initialize_loading(&flight, session, &target.id)?;
                        registry.entries.insert(
                            id.clone(),
                            Entry::Unloading {
                                _runtime: runtime.clone(),
                                flight: flight.clone(),
                            },
                        );
                        self.spawn_unload(
                            id,
                            runtime,
                            flight.clone(),
                            Some((session.clone(), target.id)),
                        );
                        (flight, true)
                    }
                }
            };
            let outcome = flight.wait().await.operation_result()?;
            if claimed {
                return outcome.ok_or_else(|| error("replacement did not publish a runtime"));
            }
        }
    }

    pub(super) fn spawn_unload(
        &self,
        id: ConversationId,
        runtime: Arc<ResidentRuntime>,
        flight: Arc<Flight>,
        replacement: Option<(SessionId, SessionNodeId)>,
    ) {
        runtime.initialization.request_retirement();
        if replacement.is_some() {
            flight.prepares_runtime.store(true, Ordering::Release);
        }
        let owner = self.clone();
        let mut terminal =
            TerminalGuard::new(self, id.clone(), flight.clone(), WriterCertainty::Unproven);
        tokio::spawn(async move {
            #[cfg(test)]
            owner.probe(&id).before_shutdown.park().await;
            runtime.initialization.retire();
            #[cfg(test)]
            owner.probe(&id).draining_operations.send_replace(true);
            let host = runtime
                .composition
                .lock()
                .expect("composition mutex")
                .as_ref()
                .expect("resident composition")
                .host()
                .clone();
            // Presentation is downstream of admitted requests and native drain.
            // Wake read-only connection waits before waiting for those owners.
            host.inner.cancel_presentation_reads();
            runtime.settle_operations().await;
            let live = runtime.shutdown_runtime().expect("resident composition");
            let outcome = match if live.is_activated() {
                live.shutdown_outcome().await
            } else {
                live.rollback_preparation_outcome().await
            } {
                Ok(outcome) => outcome,
                Err(e) => {
                    terminal.finish(Err(error(format!("{e:?}"))));
                    return;
                }
            };
            if let Err(details) = outcome.physical {
                terminal.finish(Err(error(format!(
                    "native physical retirement unproven: {details:?}"
                ))));
                return;
            }
            runtime.initialization.clear_prepared();
            let shutdown = outcome.logical.map_err(|e| error(format!("{e:?}")));
            drop(live);
            if let Err(e) = host.inner.drain_projection().await {
                terminal.finish(Err(error(format!("projection drain failed: {e}"))));
                return;
            }
            drop(host);
            // Writer-transfer boundary: shutdown has permanently closed native
            // admission and proved settlement. Stale handles cannot reopen it.
            runtime
                .composition
                .lock()
                .expect("composition mutex")
                .take();
            terminal.writer = WriterCertainty::Absent;
            if let Err(error) = shutdown {
                owner
                    .registry
                    .0
                    .lock()
                    .expect("registry mutex")
                    .unload_failures += 1;
                terminal.finish(Err(error));
                return;
            }
            if let Some((session, node)) = replacement {
                let allowed = {
                    let mut registry = owner.registry.0.lock().expect("registry mutex");
                    if registry.draining {
                        false
                    } else {
                        registry
                            .entries
                            .insert(id.clone(), Entry::Loading(flight.clone()));
                        true
                    }
                };
                if !allowed {
                    terminal.finish(Err(RuntimeManagerError::Draining));
                    return;
                }
                #[cfg(test)]
                owner.probe(&id).after_writer_transfer.park().await;
                let result = owner
                    .acquire_and_compose(
                        &session,
                        &node,
                        flight.startup.clone(),
                        &flight.initialization,
                    )
                    .await
                    .map(Some);
                terminal.finish(result);
            } else {
                terminal.finish(Ok(None));
            }
        });
    }
}

/// Owns terminal publication even if a transition panics or the executor drops
/// it. Failed Loading is removed; failed Unloading retains the old writer only
/// when native or projection physical retirement remains unproven.
/// There is no permanent abandoned Loading entry and no fabricated rollback.
struct TerminalGuard {
    owner: SessionRuntimeManager,
    registry: Arc<RuntimeRegistry>,
    id: ConversationId,
    flight: Arc<Flight>,
    finished: bool,
    candidate: Option<Arc<ResidentRuntime>>,
    writer: WriterCertainty,
}

impl TerminalGuard {
    fn new(
        owner: &SessionRuntimeManager,
        id: ConversationId,
        flight: Arc<Flight>,
        writer: WriterCertainty,
    ) -> Self {
        Self {
            owner: owner.clone(),
            registry: owner.registry.clone(),
            id,
            flight,
            finished: false,
            candidate: None,
            writer,
        }
    }
    fn finish(mut self, result: CompositionOutcome) {
        let mut catalog_observer = None;
        if let Ok(Some(runtime)) = &result {
            // Install the incarnation in the flight owner while Loading still
            // excludes all other claimants. Activation can synchronously admit
            // recovered work (including SQLite), so never hold the map lock.
            // No await separates activation and Loaded/result publication.
            self.candidate = Some(runtime.clone());
            self.writer = WriterCertainty::Unproven;
            // Register every retained native owner while the composition is
            // inert, outside registry/control locks. It starts observing only
            // after Loaded is visible; a losing initialization drops it here.
            catalog_observer = self.owner.prepare_runtime_catalog_observer(runtime);
            let can_activate = self
                .flight
                .initialization
                .install_prepared(runtime.shutdown_runtime().expect("prepared composition"));
            let fenced = self
                .registry
                .0
                .lock()
                .expect("registry mutex")
                .fenced(&self.id);
            if fenced {
                self.flight.cancel_loading();
            }
            if can_activate && !self.flight.initialization.is_retiring() {
                runtime
                    .composition
                    .lock()
                    .expect("composition mutex")
                    .as_ref()
                    .expect("composition")
                    .activate();
            }
            if self.flight.initialization.is_retiring() {
                self.registry
                    .0
                    .lock()
                    .expect("registry mutex")
                    .entries
                    .insert(
                        self.id.clone(),
                        Entry::Unloading {
                            _runtime: runtime.clone(),
                            flight: self.flight.clone(),
                        },
                    );
                self.owner.spawn_unload(
                    self.id.clone(),
                    runtime.clone(),
                    self.flight.clone(),
                    None,
                );
                self.finished = true;
                return;
            }
        }
        if let Ok(Some(runtime)) = &result {
            let mut registry = self.registry.0.lock().expect("registry mutex");
            let control = self
                .flight
                .initialization
                .control
                .lock()
                .expect("initialization control");
            if registry.fenced(&self.id) || control.retirement_requested {
                drop(control);
                // Never publish a usable incarnation after deletion wins. The
                // original Loading flight now completes only after native drain.
                registry.entries.insert(
                    self.id.clone(),
                    Entry::Unloading {
                        _runtime: runtime.clone(),
                        flight: self.flight.clone(),
                    },
                );
                self.owner.spawn_unload(
                    self.id.clone(),
                    runtime.clone(),
                    self.flight.clone(),
                    None,
                );
                self.finished = true;
                return;
            }
            // Publication must share the fence check's lock.
            registry
                .entries
                .insert(self.id.clone(), Entry::Loaded(runtime.clone()));
            self.flight
                .result
                .send_replace(Some(Outcome::Resident(runtime.identity.clone())));
            drop(control);
            drop(registry);
            if let Some(observer) = catalog_observer {
                tokio::spawn(observer);
            }
            self.finished = true;
            return;
        }
        self.publish(result.map(|_| ()));
        self.finished = true;
    }
    fn publish(&self, result: Result<(), RuntimeManagerError>) {
        if result.is_err() {
            self.flight.startup.release_preparation();
            self.flight.initialization.retire();
        }
        let mut registry = self.registry.0.lock().expect("registry mutex");
        let mut restart = None;
        let outcome = match self.writer {
            WriterCertainty::Absent => {
                let requested = self
                    .flight
                    .submission_restart
                    .lock()
                    .expect("submission restart")
                    .take();
                if let Some((session, node)) = requested.filter(|_| !registry.fenced(&self.id)) {
                    // Writer absence and the next ordinary load claim publish
                    // atomically. Every competing attach joins this exact clock.
                    let flight = Flight::with_startup(self.flight.startup.clone());
                    if let Err(error) = registry.initialize_loading(&flight, &session, &node) {
                        registry.remove(&self.id);
                        self.flight
                            .result
                            .send_replace(Some(Outcome::WriterAbsent(Err(error))));
                        return;
                    }
                    registry
                        .entries
                        .insert(self.id.clone(), Entry::Loading(flight.clone()));
                    registry.by_session.insert(session.clone(), self.id.clone());
                    restart = Some((session, node, flight));
                } else if result.is_err()
                    && self.flight.startup.has_submission()
                    && !registry.fenced(&self.id)
                    && !matches!(
                        result,
                        Err(RuntimeManagerError::LoadingCancelled | RuntimeManagerError::Draining)
                    )
                {
                    registry.entries.insert(
                        self.id.clone(),
                        Entry::FailedSubmission(self.flight.clone()),
                    );
                } else {
                    registry.remove(&self.id);
                }
                Outcome::WriterAbsent(result)
            }
            WriterCertainty::Unproven => {
                registry.unload_failures += 1;
                Outcome::RetirementUnproven(
                    result.expect_err("unproven writer cannot report retirement success"),
                )
            }
        };
        self.flight.result.send_replace(Some(outcome));
        drop(registry);
        if let Some((session, node, flight)) = restart {
            self.owner
                .spawn_load(self.id.clone(), session, node, flight);
        }
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if !self.finished {
            // Activation is synchronous/infallible in ordinary operation. If
            // an internal panic nevertheless interrupts it, retain the candidate
            // fail-closed: never remove a potentially activated writer's slot.
            if let Some(candidate) = self.candidate.take() {
                self.registry
                    .0
                    .lock()
                    .expect("registry mutex")
                    .entries
                    .insert(
                        self.id.clone(),
                        Entry::Unloading {
                            _runtime: candidate,
                            flight: self.flight.clone(),
                        },
                    );
            }
            self.publish(Err(error("runtime residency transition task terminated")));
        }
    }
}
