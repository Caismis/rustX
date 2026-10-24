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
                    Some(Entry::StorageOnly(storage)) => storage.retirement.clone(),
                    Some(Entry::Loading(flight) | Entry::Unloading { flight, .. }) => {
                        flight.clone()
                    }
                    None => {
                        registry.reserve()?;
                        let flight = Flight::new();
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
            if let Some(runtime) = flight.wait().await.operation_result()? {
                self.registry
                    .0
                    .lock()
                    .expect("registry mutex")
                    .check_session(session, runtime.conversation_id())?;
                self.rebind_configuration_runtime(session);
                return Ok(runtime);
            }
        }
    }

    pub(super) async fn activate_scheduled_target(
        &self,
        binding: &crate::scheduler::ScheduleBinding,
        trigger: &crate::scheduler::ScheduleTriggerId,
    ) -> Result<(), RuntimeManagerError> {
        if trigger.schedule_id.conversation_id != binding.conversation_id {
            return Err(error("scheduled target binding mismatch"));
        }
        loop {
            let flight = {
                let mut state = self.registry.0.lock().expect("registry mutex");
                if !state.schedule_open {
                    return Err(error("Scheduler admission is closed"));
                }
                state.check_session(&binding.session_id, &binding.conversation_id)?;
                #[cfg(test)]
                state
                    .schedule_activation_started
                    .send_replace(Some(binding.conversation_id.clone()));
                match state.entries.get(&binding.conversation_id) {
                    Some(Entry::Loaded(resident)) => {
                        let wake = resident.scheduled_wake.clone();
                        drop(state);
                        wake.notify_one();
                        return Ok(());
                    }
                    Some(Entry::Loading(flight) | Entry::Unloading { flight, .. }) => {
                        flight.clone()
                    }
                    Some(Entry::StorageOnly(storage)) => storage.retirement.clone(),
                    None => {
                        state.reserve()?;
                        let flight = Flight::new();
                        state
                            .by_session
                            .insert(binding.session_id.clone(), binding.conversation_id.clone());
                        state.entries.insert(
                            binding.conversation_id.clone(),
                            Entry::Loading(flight.clone()),
                        );
                        let terminal = TerminalGuard::new(
                            self,
                            binding.conversation_id.clone(),
                            flight.clone(),
                            WriterCertainty::Absent,
                        );
                        let owner = self.clone();
                        let binding = binding.clone();
                        let trigger = trigger.clone();
                        tokio::spawn(async move {
                            let result = async {
                                let access = owner
                                    .sessions
                                    .acquire_session(&binding.session_id, Some(&binding.node_id))
                                    .await
                                    .map_err(error)?;
                                if access.node.conversation_id != binding.conversation_id {
                                    return Err(error("scheduled destination changed"));
                                }
                                let id = binding.conversation_id.clone();
                                let path = access.database_path.clone();
                                let allocation = access.allocation.clone();
                                let pending = tokio::task::spawn_blocking(move || {
                                    use crate::durable::ScheduleStore;
                                    let store =
                                        crate::durable::SqliteConversationStore::open_existing(
                                            id, &path,
                                        )
                                        .map_err(error)?
                                        .with_lifecycle(allocation);
                                    Ok::<_, RuntimeManagerError>(
                                        store
                                            .schedule_summary()
                                            .map_err(error)?
                                            .ready
                                            .contains(&trigger),
                                    )
                                })
                                .await
                                .map_err(error)??;
                                if !pending {
                                    return Ok(None);
                                }
                                owner.compose(access).await.map(Some)
                            }
                            .await;
                            terminal.finish(result);
                        });
                        flight
                    }
                }
            };
            // A successful cold admission whose target was already adopted is a no-op.
            match flight.wait().await {
                Outcome::Resident(_) | Outcome::WriterAbsent(Ok(())) => return Ok(()),
                other => {
                    other.operation_result()?;
                }
            }
        }
    }

    fn rebind_configuration_runtime(&self, session: &SessionId) {
        if let Some(runtime) = self.configuration_runtime(session)
            && self.applications.rebind_runtime(session.as_ref(), &runtime)
        {
            self.applications.run(self.clone());
        }
    }

    fn spawn_load(
        &self,
        id: ConversationId,
        session: SessionId,
        node: SessionNodeId,
        flight: Arc<Flight>,
    ) {
        let owner = self.clone();
        let terminal = TerminalGuard::new(self, id, flight, WriterCertainty::Absent);
        // The flight is registered before this task can acquire allocation access.
        tokio::spawn(async move {
            let result = owner.acquire_and_compose(&session, &node).await.map(Some);
            terminal.finish(result);
        });
    }

    async fn acquire_and_compose(
        &self,
        session: &SessionId,
        node: &SessionNodeId,
    ) -> Result<Arc<ResidentRuntime>, RuntimeManagerError> {
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
        let access = self
            .sessions
            .acquire_session(session, Some(node))
            .await
            .map_err(error)?;
        self.compose(access).await
    }

    #[allow(clippy::too_many_lines)] // one ordered ownership transaction
    async fn compose(
        &self,
        access: SessionAccess,
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
        let identity = paths.identity().to_owned();
        let registry = super::super::composition::load_model_registry(&paths, &self.dependencies)
            .map_err(error)?;
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
        )
        .await
        .map_err(error)?;
        core.runtime()
            .restore_configuration_binding(paths.binding_revision);
        core.runtime()
            .tool_runtime()
            .schedule()
            .bind(
                crate::scheduler::ScheduleBinding {
                    session_id: access.session.id.clone(),
                    node_id: access.node.id.clone(),
                    conversation_id: access.node.conversation_id.clone(),
                },
                self.schedule_dirty.clone(),
                self.schedule_changed.clone(),
            )
            .map_err(error)?;
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
            scheduled_wake: composition.runtime().scheduled_wake(),
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
            let flight = {
                let mut registry = self.registry.0.lock().expect("registry mutex");
                match registry.entries.get(id) {
                    None => return Outcome::WriterAbsent(Ok(())),
                    Some(Entry::StorageOnly(storage)) => storage.retirement.clone(),
                    Some(Entry::Loading(flight) | Entry::Unloading { flight, .. }) => {
                        #[cfg(test)]
                        if let Some(probe) = registry.probes.get(id) {
                            probe.unloads_joined.send_modify(|n| *n += 1);
                        }
                        flight.clone()
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
                        flight
                    }
                }
            };
            // Joining replacement must also unload its resulting incarnation;
            // a replacement publication is not a successful unload result.
            match flight.wait().await {
                Outcome::Resident(_) => {}
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
                    None => {
                        registry.reserve()?;
                        let flight = Flight::new();
                        registry.by_session.insert(session.clone(), id.clone());
                        registry
                            .entries
                            .insert(id.clone(), Entry::Loading(flight.clone()));
                        self.spawn_load(id, session.clone(), target.id, flight.clone());
                        (flight, true)
                    }
                    Some(Entry::StorageOnly(storage)) => (storage.retirement.clone(), false),
                    Some(Entry::Loading(flight) | Entry::Unloading { flight, .. }) => {
                        (flight.clone(), false)
                    }
                    Some(Entry::Loaded(runtime)) => {
                        let runtime = runtime.clone();
                        let flight = Flight::new();
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
        let owner = self.clone();
        let mut terminal =
            TerminalGuard::new(self, id.clone(), flight.clone(), WriterCertainty::Unproven);
        tokio::spawn(async move {
            #[cfg(test)]
            owner.probe(&id).before_shutdown.park().await;
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
            runtime
                .operations
                .subscribe()
                .wait_for(|count| *count == 0)
                .await
                .expect("resident owns operation drain");
            let live = runtime.shutdown_runtime().expect("resident composition");
            let outcome = match live.shutdown_outcome().await {
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
                owner
                    .registry
                    .0
                    .lock()
                    .expect("registry mutex")
                    .entries
                    .insert(id.clone(), Entry::Loading(flight));
                #[cfg(test)]
                owner.probe(&id).after_writer_transfer.park().await;
                let result = owner.acquire_and_compose(&session, &node).await.map(Some);
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
        if let Ok(Some(runtime)) = &result {
            // Install the incarnation in the flight owner while Loading still
            // excludes all other claimants. Activation can synchronously admit
            // recovered work (including SQLite), so never hold the map lock.
            // No await separates activation and Loaded/result publication.
            self.candidate = Some(runtime.clone());
            self.writer = WriterCertainty::Unproven;
            runtime
                .composition
                .lock()
                .expect("composition mutex")
                .as_ref()
                .expect("composition")
                .activate();
        }
        if let Ok(Some(runtime)) = &result {
            let mut registry = self.registry.0.lock().expect("registry mutex");
            if registry.fenced(&self.id) {
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
            self.finished = true;
            return;
        }
        self.publish(result.map(|_| ()));
        self.finished = true;
    }
    fn publish(&self, result: Result<(), RuntimeManagerError>) {
        let mut registry = self.registry.0.lock().expect("registry mutex");
        let outcome = match self.writer {
            WriterCertainty::Absent => {
                registry.remove(&self.id);
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
