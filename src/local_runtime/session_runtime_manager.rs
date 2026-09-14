//! Process-local residency, separate from durable Session ownership and execution.
//!
//! One manager is allocated per `SessionController`; manager clones share its registry.
//! The durable controller retains no live runtime or registry. The registry mutex
//! protects only claim/publication transitions; no await or external work occurs
//! under it. Every transition task owns a terminal guard and an allocation guard.
//! Callers are watch receivers, never owners of the transition task. Dropping any
//! caller (including the claimant) leaves the flight running to terminal settlement.
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::watch;

use super::composition::{
    LocalConversationCore, LocalConversationRuntime, LocalRuntimeDependencies,
};
use super::configuration::UserConfigManager;
use super::session::{SessionId, SessionNodeId};
use super::session_controller::{SessionAccess, SessionController};
use crate::credentials::CredentialSnapshot;
use crate::runtime::conversation_runtime::ConversationRuntime;
use crate::runtime::identity::ConversationId;
use crate::runtime_client::endpoint::RuntimeClientEndpoint;

/// Process-local live composition identity. Never a durable or transport identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RuntimeIncarnationId(u64);

// Identity only, never a global active Session or runtime state. A stale handle
// cannot alias a later manager even if the durable controller is reopened in
// this process after all its live allocations have been released.
static NEXT_INCARNATION: AtomicU64 = AtomicU64::new(1);

/// Residency only; execution and interaction state remain runtime-owned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResidencyState {
    Unloaded,
    Loading,
    Loaded,
    Unloading,
}

/// A terminal operation error shared verbatim by every waiter in its flight.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeManagerError(pub String);
impl std::fmt::Display for RuntimeManagerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}
impl std::error::Error for RuntimeManagerError {}
fn error(e: impl std::fmt::Display) -> RuntimeManagerError {
    RuntimeManagerError(e.to_string())
}

type Outcome = Result<Option<Arc<ManagedRuntime>>, RuntimeManagerError>;

#[derive(Debug)]
struct Flight {
    result: watch::Sender<Option<Outcome>>,
}
impl Flight {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            result: watch::channel(None).0,
        })
    }
    async fn wait(&self) -> Outcome {
        self.result
            .subscribe()
            .wait_for(Option::is_some)
            .await
            .expect("flight owns terminal sender")
            .as_ref()
            .expect("terminal result")
            .clone()
    }
}

/// Manager-owned host/composition. Retained handles identify an incarnation even
/// after unload; they cannot resurrect its composition. Existing runtime/endpoint
/// clones are permanently closed by the runtime's own shutdown admission gate.
#[derive(Debug)]
pub struct ManagedRuntime {
    conversation: ConversationId,
    incarnation: RuntimeIncarnationId,
    workspace_identity: String,
    composition: Mutex<Option<LocalConversationRuntime>>,
}
impl ManagedRuntime {
    #[must_use]
    pub fn conversation_id(&self) -> &ConversationId {
        &self.conversation
    }
    #[must_use]
    pub const fn incarnation_id(&self) -> RuntimeIncarnationId {
        self.incarnation
    }
    #[must_use]
    pub fn workspace_identity(&self) -> &str {
        &self.workspace_identity
    }
    /// Obtain a runtime handle. Its native admission gate remains authoritative
    /// when this call races unload; retaining it never retains current residency.
    #[must_use]
    /// # Panics
    /// Panics if an internal residency mutex was poisoned.
    pub fn runtime(&self) -> Option<ConversationRuntime> {
        self.composition
            .lock()
            .expect("composition mutex")
            .as_ref()
            .map(|c| c.runtime().clone())
    }
    /// Attach to the already-bound host. Endpoint drop does not unload or cancel.
    #[must_use]
    /// # Panics
    /// Panics if an internal residency mutex was poisoned.
    pub fn endpoint(&self) -> Option<RuntimeClientEndpoint> {
        self.composition
            .lock()
            .expect("composition mutex")
            .as_ref()
            .map(LocalConversationRuntime::endpoint)
    }
}

#[derive(Debug)]
enum Entry {
    Loading(Arc<Flight>),
    Loaded(Arc<ManagedRuntime>),
    Unloading {
        _runtime: Arc<ManagedRuntime>,
        flight: Arc<Flight>,
    },
}
#[derive(Debug, Default)]
struct RegistryState {
    entries: HashMap<ConversationId, Entry>,
    #[cfg(test)]
    probes: HashMap<ConversationId, Arc<tests::Probe>>,
}
/// Owned only by the process runtime manager and its in-progress flights.
#[derive(Debug, Default)]
struct RuntimeRegistry(Mutex<RegistryState>);

/// The user-process residency owner. Durable metadata belongs to `sessions`,
/// current configuration sources to `configuration`, and execution to each core.
#[derive(Clone, Debug)]
pub struct SessionRuntimeManager {
    sessions: SessionController,
    registry: Arc<RuntimeRegistry>,
    configuration: UserConfigManager,
    credentials: CredentialSnapshot,
    dependencies: Arc<LocalRuntimeDependencies>,
}
impl SessionRuntimeManager {
    /// Allocate the process runtime owner once; clone it for request handlers.
    /// The durable controller retains only this allocation claim, never residency.
    /// # Errors
    /// A second independently constructed owner for this controller is rejected.
    pub fn new(
        sessions: SessionController,
        configuration: UserConfigManager,
        credentials: CredentialSnapshot,
        dependencies: LocalRuntimeDependencies,
    ) -> Result<Self, RuntimeManagerError> {
        sessions.runtime_owner.set(()).map_err(|()| {
            error("SessionController already allocated its runtime manager; clone that manager")
        })?;
        Ok(Self {
            sessions,
            registry: Arc::default(),
            configuration,
            credentials,
            dependencies: Arc::new(dependencies),
        })
    }
    #[must_use]
    /// # Panics
    /// Panics if an internal residency mutex was poisoned.
    pub fn residency(&self, id: &ConversationId) -> ResidencyState {
        match self
            .registry
            .0
            .lock()
            .expect("registry mutex")
            .entries
            .get(id)
        {
            None => ResidencyState::Unloaded,
            Some(Entry::Loading(_)) => ResidencyState::Loading,
            Some(Entry::Loaded(_)) => ResidencyState::Loaded,
            Some(Entry::Unloading { .. }) => ResidencyState::Unloading,
        }
    }
    /// Semantic seam for stale control rejection, without transport DTOs.
    #[must_use]
    /// # Panics
    /// Panics if an internal residency mutex was poisoned.
    pub fn is_current(&self, id: &ConversationId, incarnation: RuntimeIncarnationId) -> bool {
        matches!(self.registry.0.lock().expect("registry mutex").entries.get(id),
            Some(Entry::Loaded(runtime)) if runtime.incarnation == incarnation)
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
        loop {
            // Native allocation exclusion precedes trusting cold settings. No
            // catalog guard survives this call. Warm loads never resolve config.
            let access = self
                .sessions
                .acquire_session(session, node)
                .await
                .map_err(error)?;
            let id = access.node.conversation_id.clone();
            #[cfg(test)]
            let id_for_probe = id.clone();
            let flight = {
                let mut registry = self.registry.0.lock().expect("registry mutex");
                match registry.entries.get(&id) {
                    Some(Entry::Loaded(runtime)) => return Ok(runtime.clone()),
                    Some(Entry::Loading(flight) | Entry::Unloading { flight, .. }) => {
                        flight.clone()
                    }
                    None => {
                        let flight = Flight::new();
                        // Same-id load linearization: exactly one claimant can
                        // install this Loading flight before releasing the lock.
                        registry
                            .entries
                            .insert(id.clone(), Entry::Loading(flight.clone()));
                        self.spawn_load(id, access, flight.clone());
                        flight
                    }
                }
            };
            #[cfg(test)]
            self.probe(&id_for_probe).joined.send_modify(|n| *n += 1);
            if let Some(runtime) = flight.wait().await? {
                return Ok(runtime);
            }
        }
    }
    fn spawn_load(&self, id: ConversationId, access: SessionAccess, flight: Arc<Flight>) {
        let owner = self.clone();
        let terminal = TerminalGuard::new(self, id, flight);
        // Explicitly owned transition: terminal guard retains registry and sends
        // failure even on panic/task destruction. No caller owns an abort handle.
        tokio::spawn(async move {
            let result = owner.compose(access).await.map(Some);
            terminal.finish(result);
        });
    }
    async fn compose(
        &self,
        access: SessionAccess,
    ) -> Result<Arc<ManagedRuntime>, RuntimeManagerError> {
        #[cfg(test)]
        {
            let probe = self.probe(&access.node.conversation_id);
            probe
                .compositions
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            probe.before_compose.park().await;
            assert!(
                !probe
                    .panic_once
                    .swap(false, std::sync::atomic::Ordering::SeqCst),
                "injected composition task panic"
            );
        }
        let configuration = self.configuration.clone();
        let credentials = self.credentials.clone();
        // Keep allocation inside blocking resolution too: abandoning a Tokio
        // runtime cannot release authority while the blocking reader still runs.
        let (access, paths) = tokio::task::spawn_blocking(move || {
            let paths = configuration
                .resolve_session(&access.settings.input())
                .map_err(error)?
                .admit(|| credentials)
                .map_err(error)?;
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
        let registry =
            super::composition::load_model_registry(&paths, &self.dependencies).map_err(error)?;
        let core = LocalConversationCore::compose_with_access(
            &paths,
            &self.dependencies,
            registry,
            paths.config().clone(),
            access.settings,
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
        let composition = core.into_bound_with_control(None).map_err(error)?;
        let incarnation = RuntimeIncarnationId(
            NEXT_INCARNATION
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |next| {
                    next.checked_add(1)
                })
                .expect("process incarnation identity exhausted"),
        );
        Ok(Arc::new(ManagedRuntime {
            conversation: access.node.conversation_id,
            incarnation,
            workspace_identity: identity,
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
        loop {
            let flight = {
                let mut registry = self.registry.0.lock().expect("registry mutex");
                match registry.entries.get(id) {
                    None => return Ok(()),
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
            if flight.wait().await?.is_none() {
                return Ok(());
            }
        }
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
            let access = self
                .sessions
                .acquire_session(session, node)
                .await
                .map_err(error)?;
            let id = access.node.conversation_id.clone();
            let (flight, claimed) = {
                let mut registry = self.registry.0.lock().expect("registry mutex");
                match registry.entries.get(&id) {
                    None => {
                        let flight = Flight::new();
                        registry
                            .entries
                            .insert(id.clone(), Entry::Loading(flight.clone()));
                        self.spawn_load(id, access, flight.clone());
                        (flight, true)
                    }
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
                        self.spawn_unload(id, runtime, flight.clone(), Some(access));
                        (flight, true)
                    }
                }
            };
            let outcome = flight.wait().await?;
            if claimed {
                return outcome.ok_or_else(|| error("replacement did not publish a runtime"));
            }
        }
    }
    fn spawn_unload(
        &self,
        id: ConversationId,
        runtime: Arc<ManagedRuntime>,
        flight: Arc<Flight>,
        replacement: Option<SessionAccess>,
    ) {
        let owner = self.clone();
        let terminal = TerminalGuard::new(self, id.clone(), flight.clone());
        tokio::spawn(async move {
            #[cfg(test)]
            owner.probe(&id).before_shutdown.park().await;
            let live = runtime.runtime().expect("resident composition");
            if let Err(e) = live.shutdown().await {
                terminal.finish(Err(error(format!("{e:?}"))));
                return;
            }
            drop(live);
            // Writer-transfer boundary: shutdown has permanently closed native
            // admission and proved settlement. Stale handles cannot reopen it.
            runtime
                .composition
                .lock()
                .expect("composition mutex")
                .take();
            if let Some(access) = replacement {
                owner
                    .registry
                    .0
                    .lock()
                    .expect("registry mutex")
                    .entries
                    .insert(id, Entry::Loading(flight));
                // Refresh exact persisted selections after settlement while the
                // original allocation remains retained across the handoff.
                let fresh = owner
                    .sessions
                    .acquire_session(&access.session.id, Some(&access.node.id))
                    .await
                    .map_err(error);
                drop(access);
                let result = match fresh {
                    Ok(access) => owner.compose(access).await.map(Some),
                    Err(e) => Err(e),
                };
                terminal.finish(result);
            } else {
                terminal.finish(Ok(None));
            }
        });
    }
}

/// Owns terminal publication even if a transition panics or the executor drops
/// it. Failed Loading is removed; failed Unloading retains the closed old writer.
/// There is no permanent abandoned Loading entry and no fabricated rollback.
struct TerminalGuard {
    registry: Arc<RuntimeRegistry>,
    id: ConversationId,
    flight: Arc<Flight>,
    finished: bool,
    candidate: Option<Arc<ManagedRuntime>>,
}
impl TerminalGuard {
    fn new(owner: &SessionRuntimeManager, id: ConversationId, flight: Arc<Flight>) -> Self {
        Self {
            registry: owner.registry.clone(),
            id,
            flight,
            finished: false,
            candidate: None,
        }
    }
    fn finish(mut self, result: Outcome) {
        if let Ok(Some(runtime)) = &result {
            // Install the incarnation in the flight owner while Loading still
            // excludes all other claimants. Activation can synchronously admit
            // recovered work (including SQLite), so never hold the map lock.
            // No await separates activation and Loaded/result publication.
            self.candidate = Some(runtime.clone());
            runtime
                .composition
                .lock()
                .expect("composition mutex")
                .as_ref()
                .expect("composition")
                .activate();
        }
        self.publish(result);
        self.finished = true;
    }
    fn publish(&self, result: Outcome) {
        let mut registry = self.registry.0.lock().expect("registry mutex");
        match &result {
            Ok(Some(runtime)) => {
                // External publication is the ready incarnation. All callers
                // that arrived during activation joined the same Loading flight.
                registry
                    .entries
                    .insert(self.id.clone(), Entry::Loaded(runtime.clone()));
            }
            Ok(None) => {
                registry.entries.remove(&self.id);
            }
            Err(_) if matches!(registry.entries.get(&self.id), Some(Entry::Loading(_))) => {
                registry.entries.remove(&self.id);
            }
            Err(_) => {} // Retain Unloading and the actual shutdown diagnostic.
        }
        self.flight.result.send_replace(Some(result));
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

#[cfg(test)]
impl SessionRuntimeManager {
    fn probe(&self, id: &ConversationId) -> Arc<tests::Probe> {
        self.registry
            .0
            .lock()
            .unwrap()
            .probes
            .entry(id.clone())
            .or_default()
            .clone()
    }
}
#[cfg(test)]
#[path = "../../tests/scripted/app_server/mod.rs"]
mod tests;
