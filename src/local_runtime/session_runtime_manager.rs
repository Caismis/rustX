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
use std::sync::{Arc, Mutex, Weak};

use crate::runtime::monotonic::{MonotonicClock, SystemMonotonicClock};
use tokio::sync::watch;

use super::composition::{
    LocalConversationCore, LocalConversationRuntime, LocalRuntimeDependencies,
};
use super::configuration::UserConfigManager;
use super::session::{DisplayPreviewSubject, SessionId, SessionNodeId};
use super::session_controller::{DisplayPreviewRepair, SessionAccess, SessionController};
use crate::credentials::CredentialSnapshot;
use crate::runtime::conversation_runtime::ConversationRuntime;
use crate::runtime::identity::ConversationId;

/// Process-local live composition identity. Never a durable or transport identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
#[derive(schemars::JsonSchema)]
pub struct RuntimeIncarnationId(u64);

// Identity only, never a global active Session or runtime state. A stale handle
// cannot alias a later manager even if the durable controller is reopened in
// this process after all its live allocations have been released.
static NEXT_INCARNATION: AtomicU64 = AtomicU64::new(1);

/// Residency only; execution and interaction state remain runtime-owned.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub enum ResidencyState {
    Unloaded,
    Loading,
    Loaded,
    Unloading,
}

/// A terminal operation error shared verbatim by every waiter in its flight.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimeManagerError {
    Client(crate::runtime_client::types::RuntimeClientError),
    SessionAlreadyResident {
        session_id: SessionId,
        resident_conversation: ConversationId,
        requested_conversation: ConversationId,
    },
    StaleIncarnation,
    ResidencyCapacity,
    TransitionFailed(String),
}
impl std::fmt::Display for RuntimeManagerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Client(error) => write!(f, "{error:?}"),
            Self::SessionAlreadyResident {
                session_id,
                resident_conversation,
                requested_conversation,
            } => write!(
                f,
                "Session {session_id:?} already owns {resident_conversation:?}; cannot load {requested_conversation:?}"
            ),
            Self::ResidencyCapacity => f.write_str("runtime residency capacity exhausted"),
            Self::StaleIncarnation => f.write_str("runtime incarnation is no longer current"),
            Self::TransitionFailed(message) => message.fmt(f),
        }
    }
}
impl std::error::Error for RuntimeManagerError {}
fn error(e: impl std::fmt::Display) -> RuntimeManagerError {
    RuntimeManagerError::TransitionFailed(e.to_string())
}

/// Operation failure is independent of writer retirement certainty.
#[derive(Clone, Debug)]
enum Outcome {
    Resident(Arc<ManagedRuntime>),
    WriterAbsent(Result<(), RuntimeManagerError>),
    RetirementUnproven(RuntimeManagerError),
}
impl Outcome {
    fn operation_result(self) -> Result<Option<Arc<ManagedRuntime>>, RuntimeManagerError> {
        match self {
            Self::Resident(runtime) => Ok(Some(runtime)),
            Self::WriterAbsent(result) => result.map(|()| None),
            Self::RetirementUnproven(error) => Err(error),
        }
    }
}
#[derive(Clone, Copy)]
enum WriterCertainty {
    Absent,
    Unproven,
}
type CompositionOutcome = Result<Option<Arc<ResidentRuntime>>, RuntimeManagerError>;

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

/// Non-owning incarnation identity. Only manager transitions own composition.
/// Holding this identity never prevents successful unload from releasing resources.
/// No production API lends or clones its runtime, host, or allocation authority.
#[derive(Debug)]
pub struct ManagedRuntime {
    conversation: ConversationId,
    incarnation: RuntimeIncarnationId,
    workspace_identity: String,
    resident: Weak<ResidentRuntime>,
    registry: Weak<RuntimeRegistry>,
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
    /// Create a non-owning native client handle for this incarnation.
    #[must_use]
    pub fn client(self: &Arc<Self>) -> ManagedRuntimeClient {
        ManagedRuntimeClient {
            runtime: Arc::downgrade(self),
        }
    }

    /// Test-only inspection for parking real native admission/settlement gates.
    #[cfg(test)]
    fn inspect_runtime(&self) -> Option<ConversationRuntime> {
        self.resident.upgrade()?.shutdown_runtime()
    }
}

/// Sole strong composition owner, retained only by registry/transition machinery.
#[derive(Debug)]
struct ResidentRuntime {
    identity: Arc<ManagedRuntime>,
    composition: Mutex<Option<LocalConversationRuntime>>,
    operations: watch::Sender<usize>,
    external: std::sync::atomic::AtomicUsize,
    activity: AtomicU64,
    idle: Mutex<Option<(u64, u64, u64)>>,
}

/// External semantic relationship, independent of internal Runtime Client plumbing.
#[derive(Debug)]
pub(crate) struct RuntimeResidencyPin {
    registry: Weak<RuntimeRegistry>,
    resident: Weak<ResidentRuntime>,
    released: std::sync::atomic::AtomicBool,
}
impl RuntimeResidencyPin {
    /// # Panics
    /// Panics if an internal ownership mutex is poisoned.
    pub(crate) fn release(&self) {
        if let Some(registry) = self.registry.upgrade() {
            let _state = registry.0.lock().expect("registry mutex");
            if !self.released.swap(true, Ordering::Relaxed)
                && let Some(resident) = self.resident.upgrade()
            {
                resident.external.fetch_sub(1, Ordering::Relaxed);
                resident.activity.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}
impl Drop for RuntimeResidencyPin {
    fn drop(&mut self) {
        self.release();
    }
}

/// The manager receives only residency policy, resolved by its composer.
#[derive(Clone, Debug)]
pub struct RuntimeResidencyPolicy {
    pub max_resident_runtimes: usize,
    pub idle_grace_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeResidencyDiagnostics {
    pub loaded: usize,
    pub loading: usize,
    pub unloading: usize,
    pub active_roots: usize,
    pub residency_pins: usize,
    pub sessions: Vec<SessionResidencyDiagnostic>,
    pub admission_refusals: std::collections::BTreeMap<String, u64>,
    pub unload_failures: u64,
}
#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct SessionResidencyDiagnostic {
    pub session_id: SessionId,
    pub conversation_id: ConversationId,
    pub residency: ResidencyState,
    pub incarnation: Option<RuntimeIncarnationId>,
    pub external_attachments: usize,
    pub operations: usize,
    pub active_root: bool,
    pub idle_for_ms: Option<u64>,
    pub idle_remaining_ms: Option<u64>,
}

/// Private, non-cloneable server-operation ownership. Never lent to a client.
struct OperationLease(Option<Arc<ResidentRuntime>>);

impl Drop for OperationLease {
    fn drop(&mut self) {
        let resident = self.0.take().expect("operation resident");
        let operations = resident.operations.clone();
        // Release strong ownership before publishing the drain acknowledgement.
        drop(resident);
        operations.send_modify(|count| *count -= 1);
    }
}
impl ResidentRuntime {
    // Shutdown alone may clone execution authority. Never exposed to clients.
    fn shutdown_runtime(&self) -> Option<ConversationRuntime> {
        self.composition
            .lock()
            .expect("composition mutex")
            .as_ref()
            .map(|c| c.runtime().clone())
    }
}

/// Non-owning control/observation seam. Dropping it has no runtime side effects.
/// Operations return owned facts, never runtime/host/storage handles. The local
/// composition mutex covers bounded synchronous helpers, without await. App
/// Server work uses a registry-admitted server task and scoped operation lease;
/// unload drains these before native shutdown and composition release. No
/// global registry lock covers runtime work or an asynchronous wait.
#[derive(Clone, Debug)]
pub struct ManagedRuntimeClient {
    runtime: Weak<ManagedRuntime>,
}
impl ManagedRuntimeClient {
    fn admit_operation(&self) -> Result<OperationLease, RuntimeManagerError> {
        let identity = self
            .runtime
            .upgrade()
            .ok_or(RuntimeManagerError::StaleIncarnation)?;
        let registry = identity
            .registry
            .upgrade()
            .ok_or(RuntimeManagerError::StaleIncarnation)?;
        let state = registry.0.lock().expect("registry mutex");
        let Some(Entry::Loaded(resident)) = state.entries.get(&identity.conversation) else {
            return Err(RuntimeManagerError::StaleIncarnation);
        };
        if state.fenced(&identity.conversation)
            || resident.identity.incarnation != identity.incarnation
        {
            return Err(RuntimeManagerError::StaleIncarnation);
        }
        // Same lock as Loaded -> Unloading: no late increment is possible.
        resident.activity.fetch_add(1, Ordering::Relaxed);
        resident.operations.send_modify(|count| *count += 1);
        Ok(OperationLease(Some(resident.clone())))
    }

    /// Admit work to a server-owned task. The caller receives only a reply channel;
    /// cancellation or retention of that receiver cannot retain the operation lease.
    pub(crate) fn start_operation<T, F, Fut>(
        &self,
        operation: F,
    ) -> Result<tokio::sync::oneshot::Receiver<T>, RuntimeManagerError>
    where
        T: Send + 'static,
        F: FnOnce() -> Fut + Send + 'static,
        Fut: std::future::Future<Output = T> + Send + 'static,
    {
        let lease = self.admit_operation()?;
        #[cfg(test)]
        let probe = {
            let identity = &lease.0.as_ref().expect("operation resident").identity;
            identity
                .registry
                .upgrade()
                .expect("resident registry")
                .0
                .lock()
                .expect("registry mutex")
                .probes
                .get(&identity.conversation)
                .cloned()
        };
        // Capture native authority synchronously after residency admission. The
        // caller can serialize this cut with connection close; only execution
        // of the resulting future moves to the server-owned task.
        let operation = operation();
        let (sender, receiver) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            #[cfg(test)]
            if let Some(probe) = probe {
                probe.before_operation.park().await;
            }
            let result = operation.await;
            drop(lease);
            let _ = sender.send(result);
        });
        Ok(receiver)
    }
    /// Reject use of an unloaded, unloading or replaced incarnation.
    /// # Errors
    /// Returns `StaleIncarnation` when residency has ended.
    pub fn validate(&self) -> Result<(), RuntimeManagerError> {
        self.current().map(|_| ())
    }

    /// Admit a non-owning controller and linearize its snapshot/subscription.
    /// # Errors
    /// Stale incarnations and conflicting controllers are rejected.
    /// # Panics
    /// Panics if a composition mutex is poisoned.
    pub(crate) fn attach(
        &self,
    ) -> Result<
        (
            crate::runtime_client::attachment::AttachedSnapshot,
            RuntimeResidencyPin,
        ),
        RuntimeManagerError,
    > {
        let lease = self.admit_operation()?;
        let runtime = lease.0.as_ref().expect("operation resident");
        let registry = runtime
            .identity
            .registry
            .upgrade()
            .ok_or(RuntimeManagerError::StaleIncarnation)?;
        let state = registry.0.lock().expect("registry mutex");
        if state.fenced(&runtime.identity.conversation)
            || !matches!(state.entries.get(&runtime.identity.conversation), Some(Entry::Loaded(current)) if Arc::ptr_eq(current, runtime))
        {
            return Err(RuntimeManagerError::StaleIncarnation);
        }
        // Pin acquisition shares registry publication with idle eviction.
        runtime.external.fetch_add(1, Ordering::Relaxed);
        let external = RuntimeResidencyPin {
            registry: Arc::downgrade(&registry),
            resident: Arc::downgrade(runtime),
            released: std::sync::atomic::AtomicBool::new(false),
        };
        drop(state);
        let composition = runtime.composition.lock().expect("composition mutex");
        let snapshot = composition
            .as_ref()
            .ok_or(RuntimeManagerError::StaleIncarnation)?
            .host()
            .inner
            .admit_attachment(false, true)
            .map_err(RuntimeManagerError::Client)?;
        Ok((snapshot, external))
    }
    fn current(&self) -> Result<Arc<ResidentRuntime>, RuntimeManagerError> {
        let runtime = self
            .runtime
            .upgrade()
            .ok_or(RuntimeManagerError::StaleIncarnation)?;
        let registry = runtime
            .registry
            .upgrade()
            .ok_or(RuntimeManagerError::StaleIncarnation)?;
        let state = registry.0.lock().expect("registry mutex");
        let current = !state.fenced(&runtime.conversation)
            && matches!(state.entries.get(&runtime.conversation),
            Some(Entry::Loaded(resident)) if resident.identity.incarnation == runtime.incarnation);
        if current {
            runtime
                .resident
                .upgrade()
                .ok_or(RuntimeManagerError::StaleIncarnation)
        } else {
            Err(RuntimeManagerError::StaleIncarnation)
        }
    }

    /// Submit through the existing native admission gate.
    /// # Errors
    /// Rejects stale incarnations and native admission failures.
    /// # Panics
    /// Panics if an internal residency mutex was poisoned.
    pub fn submit_inbound(
        &self,
        content: Vec<crate::message::types::UserContentBlock>,
    ) -> Result<crate::runtime::conversation_runtime::InboundAdmission, RuntimeManagerError> {
        if content
            .iter()
            .any(|block| !matches!(block, crate::message::types::UserContentBlock::Text(_)))
        {
            return Err(error("user uploads require server-issued Session receipts"));
        }
        let lease = self.admit_operation()?;
        let runtime = lease.0.as_ref().expect("operation resident");
        let composition = runtime.composition.lock().expect("composition mutex");
        composition
            .as_ref()
            .ok_or(RuntimeManagerError::StaleIncarnation)?
            .runtime()
            .submit_inbound(content)
            .map_err(|e| error(format!("{e:?}")))
    }

    /// Read owned projection facts from the existing Runtime Client host.
    /// # Errors
    /// Rejects stale incarnations and projection failures.
    /// # Panics
    /// Panics if an internal residency mutex was poisoned.
    pub fn snapshot(
        &self,
    ) -> Result<
        (
            crate::runtime_client::snapshot::RuntimeClientSnapshot,
            crate::runtime_client::types::RuntimeClientCursor,
        ),
        RuntimeManagerError,
    > {
        let lease = self.admit_operation()?;
        let runtime = lease.0.as_ref().expect("operation resident");
        let composition = runtime.composition.lock().expect("composition mutex");
        composition
            .as_ref()
            .ok_or(RuntimeManagerError::StaleIncarnation)?
            .host()
            .snapshot()
            .map_err(|e| error(format!("{e:?}")))
    }
}

#[derive(Debug)]
enum Entry {
    Loading(Arc<Flight>),
    Loaded(Arc<ResidentRuntime>),
    Unloading {
        _runtime: Arc<ResidentRuntime>,
        flight: Arc<Flight>,
    },
}
#[derive(Debug)]
struct RegistryState {
    entries: HashMap<ConversationId, Entry>,
    policy: RuntimeResidencyPolicy,
    refusals: std::collections::BTreeMap<String, u64>,
    unload_failures: u64,

    // Ownership index only, not a second state machine. Covers every entry,
    // including replacement handoff and failed (unproven) shutdown.
    by_session: HashMap<SessionId, ConversationId>,
    // Session admission fencing shares every registry transition, including absence.
    retiring_sessions: std::collections::HashSet<SessionId>,
    #[cfg(test)]
    probes: HashMap<ConversationId, Arc<tests::Probe>>,
    #[cfg(test)]
    reaper_waiting: Option<watch::Sender<u64>>,
}
impl RegistryState {
    fn fenced(&self, id: &ConversationId) -> bool {
        self.by_session
            .iter()
            .any(|(session, resident)| resident == id && self.retiring_sessions.contains(session))
    }

    fn refuse(&mut self, reason: &str) {
        let count = self.refusals.entry(reason.into()).or_default();
        *count = count.saturating_add(1);
    }
    fn reserve(&mut self) -> Result<(), RuntimeManagerError> {
        if self.entries.len() >= self.policy.max_resident_runtimes {
            self.refuse("residency_capacity");
            return Err(RuntimeManagerError::ResidencyCapacity);
        }
        Ok(())
    }

    fn check_session(
        &self,
        session: &SessionId,
        conversation: &ConversationId,
    ) -> Result<(), RuntimeManagerError> {
        if self.retiring_sessions.contains(session) {
            return Err(error("Session retirement has fenced runtime admission"));
        }
        if let Some(resident) = self.by_session.get(session)
            && resident != conversation
        {
            return Err(RuntimeManagerError::SessionAlreadyResident {
                session_id: session.clone(),
                resident_conversation: resident.clone(),
                requested_conversation: conversation.clone(),
            });
        }
        Ok(())
    }
    fn remove(&mut self, id: &ConversationId) {
        self.entries.remove(id);
        self.by_session.retain(|_, resident| resident != id);
    }
}
/// Owned only by the process runtime manager and its in-progress flights.
#[derive(Debug)]
struct RuntimeRegistry(Mutex<RegistryState>);

/// The user-process residency owner. Durable metadata belongs to `sessions`,
/// current configuration sources to `configuration`, and execution to each core.
#[derive(Clone, Debug)]
pub struct SessionRuntimeManager {
    pub(super) sessions: SessionController,
    registry: Arc<RuntimeRegistry>,
    pub(super) configuration: UserConfigManager,
    pub(super) credentials: CredentialSnapshot,
    pub(super) process_policy: Arc<std::sync::RwLock<super::app_server_policy::AppServerPolicy>>,
    pub(super) applications: super::configuration::application::ConfigurationApplications,
    dependencies: Arc<LocalRuntimeDependencies>,
    clock: Arc<dyn MonotonicClock>,
}
impl SessionRuntimeManager {
    #[must_use]
    /// # Panics
    /// Panics if an internal ownership mutex is poisoned.
    pub fn diagnostics(&self) -> RuntimeResidencyDiagnostics {
        let (mut snapshot, candidates) = {
            let state = self.registry.0.lock().expect("registry mutex");
            let candidates: Vec<_> = state
                .by_session
                .iter()
                .map(|(session, id)| {
                    let (residency, resident) = match &state.entries[id] {
                        Entry::Loading(_) => (ResidencyState::Loading, None),
                        Entry::Loaded(runtime) => (ResidencyState::Loaded, Some(runtime.clone())),
                        Entry::Unloading {
                            _runtime: runtime, ..
                        } => (ResidencyState::Unloading, Some(runtime.clone())),
                    };
                    (session.clone(), id.clone(), residency, resident)
                })
                .collect();
            (
                RuntimeResidencyDiagnostics {
                    loaded: 0,
                    loading: 0,
                    unloading: 0,
                    active_roots: 0,
                    residency_pins: 0,
                    sessions: Vec::new(),
                    admission_refusals: state.refusals.clone(),
                    unload_failures: state.unload_failures,
                },
                candidates,
            )
        };
        // Native observations can wait behind storage work. They must never
        // hold the process admission lock or become admission authority.
        let now = self.clock.now_millis();
        for (session_id, conversation_id, residency, resident) in candidates {
            match residency {
                ResidencyState::Loading => snapshot.loading += 1,
                ResidencyState::Loaded => snapshot.loaded += 1,
                ResidencyState::Unloading => snapshot.unloading += 1,
                ResidencyState::Unloaded => {}
            }
            let active_root = resident
                .as_ref()
                .and_then(|r| r.shutdown_runtime())
                .is_some_and(|r| r.has_current_attempt());
            snapshot.active_roots += usize::from(active_root);
            let idle_for_ms = resident
                .as_ref()
                .and_then(|r| {
                    r.idle
                        .lock()
                        .expect("idle mutex")
                        .filter(|(_, _, activity)| {
                            !active_root
                                && r.external.load(Ordering::Relaxed) == 0
                                && *r.operations.borrow() == 0
                                && *activity == r.activity.load(Ordering::Relaxed)
                        })
                })
                .map(|(since, _, _)| now.saturating_sub(since));
            snapshot.sessions.push(SessionResidencyDiagnostic {
                session_id,
                conversation_id,
                residency,
                incarnation: resident.as_ref().map(|r| r.identity.incarnation),
                external_attachments: resident
                    .as_ref()
                    .map_or(0, |r| r.external.load(Ordering::Relaxed)),
                operations: resident.as_ref().map_or(0, |r| *r.operations.borrow()),
                active_root,
                idle_for_ms,
                idle_remaining_ms: idle_for_ms
                    .map(|elapsed| self.policy().idle_grace_ms.saturating_sub(elapsed)),
            });
        }
        snapshot.residency_pins = snapshot
            .sessions
            .iter()
            .map(|s| s.external_attachments)
            .sum();
        snapshot
            .sessions
            .sort_by(|a, b| a.session_id.cmp(&b.session_id));
        snapshot
    }
    #[must_use]
    /// # Panics
    /// Panics if an internal ownership mutex is poisoned.
    pub fn policy(&self) -> RuntimeResidencyPolicy {
        self.registry
            .0
            .lock()
            .expect("registry mutex")
            .policy
            .clone()
    }

    /// Supervise the current residency set without failing fast. To prove all
    /// residency settled, the composer must also settle upstream admissions and
    /// drain a final inventory; this owner has no server lifecycle/protocol gate.
    /// # Panics
    /// Panics if the residency mutex is poisoned.
    pub async fn drain_all_runtimes(&self) -> Vec<String> {
        let ids: Vec<_> = self
            .registry
            .0
            .lock()
            .expect("registry mutex")
            .entries
            .keys()
            .cloned()
            .collect();
        let results = futures_util::future::join_all(ids.iter().map(|id| async move {
            self.unload(id)
                .await
                .err()
                .map(|error| format!("{id:?}: {error}"))
        }))
        .await;
        let mut failures: Vec<_> = results.into_iter().flatten().collect();
        failures.sort();
        failures
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.registry
            .0
            .lock()
            .expect("registry mutex")
            .entries
            .is_empty()
    }

    /// Nonblocking inventory for a composer's forced termination report.
    pub(crate) fn unproven_resources(&self) -> String {
        let Ok(state) = self.registry.0.try_lock() else {
            return "residency snapshot unavailable (admission boundary busy)".into();
        };
        let mut resources: Vec<_> = state
            .entries
            .iter()
            .map(|(id, entry)| {
                let phase = match entry {
                    Entry::Loading(_) => "Loading",
                    Entry::Loaded(_) => "Loaded",
                    Entry::Unloading { .. } => "Unloading",
                };
                format!("{id:?}: {phase}")
            })
            .collect();
        resources.sort();
        format!("runtimes=[{}]", resources.join(", "))
    }

    /// # Panics
    /// Panics if an internal ownership mutex is poisoned.
    pub async fn run_idle_reaper(&self, stop: tokio_util::sync::CancellationToken) {
        loop {
            self.reap_idle();
            let deadline = self.clock.now_millis().saturating_add(1000);
            let waiting = self.clock.wait_until_millis(deadline);
            #[cfg(test)]
            if let Some(observer) = self
                .registry
                .0
                .lock()
                .expect("registry mutex")
                .reaper_waiting
                .clone()
            {
                observer.send_replace(deadline);
            }
            tokio::select! {
                () = stop.cancelled() => return,
                () = waiting => {},
            }
        }
    }

    /// A bounded scan. Native probes run outside the registry lock. Only the
    /// short epoch-validated native admission claim runs inside publication.
    /// # Panics
    /// Panics if an internal ownership mutex is poisoned.
    pub fn reap_idle(&self) {
        let candidates: Vec<_> = {
            let state = self.registry.0.lock().expect("registry mutex");
            state
                .entries
                .values()
                .filter_map(|entry| match entry {
                    Entry::Loaded(runtime) => Some(runtime.clone()),
                    _ => None,
                })
                .collect()
        };
        let now = self.clock.now_millis();
        for resident in candidates {
            let activity = resident.activity.load(Ordering::Relaxed);
            let Some(native) = resident.shutdown_runtime() else {
                continue;
            };
            let epoch = native.idle_epoch().ok();
            #[cfg(test)]
            self.probe(&resident.identity.conversation)
                .idle_before_claim
                .enter();
            let mut state = self.registry.0.lock().expect("registry mutex");
            if !matches!(state.entries.get(&resident.identity.conversation), Some(Entry::Loaded(current)) if Arc::ptr_eq(current, &resident))
            {
                continue;
            }
            let mut idle = resident.idle.lock().expect("idle mutex");
            if epoch.is_none()
                || resident.external.load(Ordering::Relaxed) != 0
                || *resident.operations.borrow() != 0
                || resident.activity.load(Ordering::Relaxed) != activity
            {
                *idle = None;
                continue;
            }
            let epoch = epoch.expect("eligible epoch");
            let (since, _, _) = *idle.get_or_insert((now, epoch, activity));
            if idle
                .is_some_and(|(_, previous, operation)| previous != epoch || operation != activity)
            {
                *idle = Some((now, epoch, activity));
                continue;
            }
            if now.saturating_sub(since) < state.policy.idle_grace_ms {
                continue;
            }
            if !native.claim_idle(epoch) {
                *idle = None;
                continue;
            }
            let id = resident.identity.conversation.clone();
            let flight = Flight::new();
            state.entries.insert(
                id.clone(),
                Entry::Unloading {
                    _runtime: resident.clone(),
                    flight: flight.clone(),
                },
            );
            drop(idle);
            drop(state);
            #[cfg(test)]
            self.probe(&id).idle_after_claim.enter();
            self.spawn_unload(id, resident, flight, None);
        }
    }

    /// Durable authority shared with connection routing; no runtime is loaded.
    #[must_use]
    pub fn session_controller(&self) -> SessionController {
        self.sessions.clone()
    }
    /// Allocate the process runtime owner once; clone it for request handlers.
    /// The durable controller retains only this allocation claim, never residency.
    /// # Errors
    /// A second independently constructed owner for this controller is rejected.
    pub fn new(
        sessions: SessionController,
        configuration: UserConfigManager,
        credentials: CredentialSnapshot,
        dependencies: LocalRuntimeDependencies,
        policy: RuntimeResidencyPolicy,
    ) -> Result<Self, RuntimeManagerError> {
        if policy.max_resident_runtimes == 0 || policy.idle_grace_ms == 0 {
            return Err(error("residency limits must be positive"));
        }
        sessions.runtime_owner.set(()).map_err(|()| {
            error("SessionController already allocated its runtime manager; clone that manager")
        })?;
        Ok(Self {
            sessions,
            registry: Arc::new(RuntimeRegistry(Mutex::new(RegistryState {
                policy,
                entries: HashMap::new(),
                by_session: HashMap::new(),
                retiring_sessions: std::collections::HashSet::new(),
                refusals: std::collections::BTreeMap::default(),
                unload_failures: 0,
                #[cfg(test)]
                probes: HashMap::new(),
                #[cfg(test)]
                reaper_waiting: None,
            }))),
            process_policy: Arc::new(std::sync::RwLock::new(
                configuration.app_server_policy().map_err(error)?,
            )),
            configuration,
            credentials,
            applications: super::configuration::application::ConfigurationApplications::default(),
            dependencies: Arc::new(dependencies),
            clock: Arc::new(SystemMonotonicClock::new()),
        })
    }
    #[cfg(test)]
    pub(super) async fn configuration_test_gate(
        &self,
        session: &SessionId,
        publication: bool,
    ) -> bool {
        let node = self
            .sessions
            .catalog
            .lock()
            .await
            .lineage(session, None)
            .expect("test Session")
            .0;
        let probe = self.probe(&node.conversation_id);
        if publication {
            probe.before_configuration_publish.park().await;
            false
        } else {
            probe
                .configuration_preparations
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            probe.before_configuration_prepare.park().await;
            probe
                .fail_configuration_once
                .swap(false, std::sync::atomic::Ordering::SeqCst)
        }
    }

    pub(crate) fn process_policy(&self) -> super::app_server_policy::AppServerPolicy {
        self.process_policy.read().expect("process policy").clone()
    }

    pub(crate) fn bind_process_policy(&self, policy: super::app_server_policy::AppServerPolicy) {
        *self.process_policy.write().expect("process policy") = policy;
    }

    pub(super) fn apply_process_limits(
        &self,
        desired: &super::app_server_policy::AppServerPolicy,
    ) -> bool {
        let mut actual = self.process_policy.write().expect("process policy");
        let restart = actual.shutdown_deadline_ms != desired.shutdown_deadline_ms;
        actual.max_connections = desired.max_connections;
        actual.max_external_attachments = desired.max_external_attachments;
        actual.max_resident_runtimes = desired.max_resident_runtimes;
        actual.idle_grace_ms = desired.idle_grace_ms;
        let mut registry = self.registry.0.lock().expect("registry mutex");
        registry.policy.max_resident_runtimes = desired.max_resident_runtimes;
        registry.policy.idle_grace_ms = desired.idle_grace_ms;
        restart
    }

    pub(crate) async fn create_session(
        &self,
        mut settings: super::session::SessionPersistentState,
    ) -> Result<super::session_controller::SessionTransitionResult, super::session::SessionError>
    {
        let configuration = self.configuration.clone();
        let credentials = self.credentials.clone();
        let input = settings.input();
        let applications = self.applications.clone();
        let capture = tokio::task::spawn_blocking(move || {
            applications
                .lock()
                .initial_binding(&configuration, &input, &credentials)
        })
        .await
        .map_err(|error| super::session::SessionError::Catalog {
            detail: error.to_string(),
        })?
        .map_err(|detail| super::session::SessionError::Catalog { detail })?;
        settings.model = capture.input.model.clone();
        let result = self.sessions.create_session(settings).await?;
        let mut application = self.applications.lock();
        application.register_session_scope(result.session.id.to_string(), &capture);
        self.sessions
            .configuration_bindings
            .lock()
            .expect("Session configuration bindings")
            .insert(result.session.id.clone(), capture);
        self.applications.notify(&application);
        Ok(result)
    }

    pub(super) fn configuration_runtime(&self, id: &SessionId) -> Option<ConversationRuntime> {
        let resident = {
            let state = self.registry.0.lock().expect("registry mutex");
            state.by_session.get(id).and_then(|conversation| {
                match state.entries.get(conversation) {
                    Some(Entry::Loaded(resident)) => Some(resident.clone()),
                    _ => None,
                }
            })
        };
        resident.and_then(|resident| resident.shutdown_runtime())
    }

    pub(crate) fn configuration_application(
        &self,
        id: &SessionId,
    ) -> Option<super::configuration::application::ConfigurationApplication> {
        let mut application = self.applications.lock().view(id.as_ref())?;
        if application.candidate.is_some() {
            application.eligibility = self.configuration_runtime(id).map_or(
                super::configuration::application::AdoptionEligibility::Unavailable,
                |runtime| runtime.configuration_adoption_eligibility(),
            );
        }
        Some(application)
    }

    pub(crate) fn configuration_changes(&self) -> watch::Receiver<u64> {
        self.applications.subscribe()
    }

    pub(crate) fn configuration_applications(
        &self,
    ) -> Vec<super::configuration::application::ConfigurationApplication> {
        self.applications.lock().views()
    }

    pub(crate) async fn set_model(
        &self,
        session: &SessionId,
        selection: crate::model::session::SessionModelConfig,
    ) -> Result<
        crate::model::session::SessionModelView,
        super::configuration::application::AdoptionError,
    > {
        use super::configuration::application::AdoptionError;
        let expected_settings = self
            .sessions
            .catalog
            .lock()
            .await
            .settings_revision(session)
            .map_err(|_| AdoptionError::Conflict)?;
        let owner = self.clone();
        let session = session.clone();
        tokio::task::spawn_blocking(move || {
            let mut application = owner.applications.lock();
            let adopted = owner
                .sessions
                .configuration_bindings
                .lock()
                .expect("Session configuration bindings")
                .get(&session)
                .cloned()
                .ok_or(AdoptionError::NotReady)?;
            let capture = owner
                .configuration
                .capture_session_model(&adopted, selection.clone())
                .map_err(|diagnostic| AdoptionError::Failed { diagnostic })?;
            let models = crate::model::invocation::ModelBindingRegistry::new(
                capture
                    .models
                    .resolve(&owner.credentials)
                    .map_err(|error| AdoptionError::Failed {
                        diagnostic: error.to_string(),
                    })?,
            )
            .map_err(|error| AdoptionError::Failed {
                diagnostic: error.to_string(),
            })?;
            let runtime = owner
                .configuration_runtime(&session)
                .ok_or(AdoptionError::NotReady)?;
            if runtime.model_view().configured == selection && capture.same_provider(&adopted) {
                return Ok(runtime.model_view());
            }
            let mut candidate = runtime
                .prepare_context_configuration(&capture, models, Some(selection.clone()))
                .map_err(|diagnostic| AdoptionError::Failed { diagnostic })?;
            candidate.selection_only = true;
            let baseline = candidate.baseline;
            let view = candidate.model.view();
            let mut retained = capture
                .admit(|| owner.credentials.clone())
                .map_err(|diagnostic| AdoptionError::Failed { diagnostic })?;
            retained.binding_revision = baseline + 1;
            let mut catalog = owner
                .sessions
                .catalog
                .try_lock()
                .map_err(|_| AdoptionError::Busy)?;
            let (_, mut settings) = catalog
                .lineage(&session, None)
                .map_err(|_| AdoptionError::Conflict)?;
            settings.model = Some(selection);
            runtime.adopt_configuration(&mut Some(candidate), baseline, true, || {
                catalog
                    .replace_settings(&session, expected_settings, settings)
                    .map_err(|error| AdoptionError::Failed {
                        diagnostic: error.to_string(),
                    })?;
                owner
                    .sessions
                    .configuration_bindings
                    .lock()
                    .expect("Session configuration bindings")
                    .insert(session.clone(), retained.clone());
                Ok(())
            })?;
            drop(catalog);
            let input = owner.configuration.capture_application(&retained.input);
            application.record_source(&retained.input.cwd, &input);
            application.capture_binding(session.to_string(), input);
            owner.applications.notify(&application);
            drop(application);
            owner.applications.run(owner.clone());
            Ok(view)
        })
        .await
        .map_err(|_| AdoptionError::Failed {
            diagnostic: "Session model operation interrupted".into(),
        })?
    }

    pub(crate) fn adopt_configuration(
        &self,
        session: &SessionId,
        identity: &super::configuration::application::ApplicationIdentity,
        expected_binding: u64,
    ) -> Result<
        super::configuration::application::ConfigurationApplication,
        super::configuration::application::AdoptionError,
    > {
        self.applications
            .adopt(self, session, identity, expected_binding)
    }

    pub(crate) async fn reconcile_configuration(
        &self,
        target: &super::configuration::settings::SourceTarget,
    ) -> Result<super::configuration::application::ConfigurationApplication, SourceSettingsError>
    {
        let owner = self.clone();
        let target = target.clone();
        tokio::spawn(async move {
            owner.coordinate_source(&target).await?;
            owner
                .applications
                .lock()
                .view(&target.application_scope())
                .ok_or(SourceSettingsError::Source(
                    super::configuration::settings::SettingsError::Io,
                ))
        })
        .await
        .map_err(|_| {
            SourceSettingsError::Source(super::configuration::settings::SettingsError::Committed)
        })?
    }

    async fn coordinate_source(
        &self,
        target: &super::configuration::settings::SourceTarget,
    ) -> Result<(), SourceSettingsError> {
        let owner = self.clone();
        let target = target.clone();
        tokio::task::spawn_blocking(move || {
            target.validate().map_err(SourceSettingsError::Source)?;
            let mut application = owner.applications.lock();
            owner.capture_source_consumers(&mut application, &target);
            owner.applications.notify(&application);
            drop(application);
            owner.applications.run(owner.clone());
            Ok(())
        })
        .await
        .map_err(|_| {
            SourceSettingsError::Source(super::configuration::settings::SettingsError::Io)
        })?
    }

    fn capture_source_consumers(
        &self,
        application: &mut super::configuration::application::ApplicationState,
        target: &super::configuration::settings::SourceTarget,
    ) {
        use super::configuration::settings::SourceTarget;
        let captured = self.configuration.capture_source_settings(target);
        let revision = captured.as_ref().ok().map(|(_, revision)| revision.clone());
        let process = captured
            .as_ref()
            .ok()
            .and_then(|(settings, _)| settings.user.authored.as_ref())
            .map(|document| document.app_server.clone().unwrap_or_default())
            .ok_or_else(|| "User process policy source is invalid or unreadable".into())
            .and_then(|policy| policy.validate().map(|()| policy));
        application.capture_source(target, revision, process);
        if let SourceTarget::Workspace { directory } = target {
            let input = super::configuration::SessionConfigInput::new(directory.clone());
            application.record_source(directory, &self.configuration.capture_application(&input));
        }
        if matches!(target, SourceTarget::User) {
            // A User commit also refreshes the desired capture of every
            // Workspace identity with retained native authority, including
            // those without any live Session. This never touches `available`;
            // only successful fenced preparation publishes there.
            for directory in application.known_source_directories() {
                let input = super::configuration::SessionConfigInput::new(directory.clone());
                application
                    .record_source(&directory, &self.configuration.capture_application(&input));
            }
        }
        let inputs: Vec<_> = self
            .sessions
            .configuration_bindings
            .lock()
            .expect("Session configuration bindings")
            .iter()
            .filter(|(_, binding)| {
                target
                    .workspace()
                    .is_none_or(|directory| directory == binding.input.cwd)
            })
            .map(|(session, binding)| (session.clone(), binding.input.clone()))
            .collect();
        for (session, input) in inputs {
            application.capture(
                session.to_string(),
                &input.cwd,
                self.configuration.capture_application(&input),
            );
        }
    }

    /// Source authoring never loads, creates, or derives authority from a Session.
    /// Writes transfer to a native-owned task before persistence begins. Source
    /// publication and coordinator capture are serialized by the application lock.
    /// The task survives destruction of the RPC waiter, including after commit.
    /// # Errors
    /// Returns typed source validation, CAS, I/O, or uncertain-commit errors.
    pub async fn source_settings(
        &self,
        target: &super::configuration::settings::SourceTarget,
        mutation: Option<(String, super::configuration::settings::SourceMutation)>,
    ) -> Result<super::configuration::settings::SourceSettings, SourceSettingsError> {
        let owner = self.clone();
        let target = target.clone();
        tokio::task::spawn_blocking(move || {
            target.validate().map_err(SourceSettingsError::Source)?;
            let mut application = owner.applications.lock();
            let committed = mutation.is_some();
            let result = match mutation {
                Some((expected, mutation)) => owner
                    .configuration
                    .write_source_settings(&target, &expected, mutation),
                None => owner.configuration.read_source_settings(&target),
            };
            if committed
                && (result.is_ok()
                    || matches!(
                        &result,
                        Err(super::configuration::settings::SettingsError::Committed)
                    ))
            {
                // Linearization: persisted source belongs to native coordination
                // before releasing the publication lock or replying to the client.
                owner.capture_source_consumers(&mut application, &target);
                owner.applications.notify(&application);
            }
            let mut projection = result;
            if let Ok(projection) = &mut projection {
                projection.application = application.view(&target.application_scope());
                projection.process_bindings = Some(owner.process_policy());
                projection.session_models = target.workspace().map(|directory| {
                    use super::configuration::settings::SessionModelsView;
                    match application.session_creation_models(
                        &owner.configuration,
                        directory,
                        &owner.credentials,
                    ) {
                        Ok((catalog, default_model)) => SessionModelsView::Available {
                            catalog,
                            default_model: Box::new(default_model),
                        },
                        Err(diagnostic) => SessionModelsView::Unavailable { diagnostic },
                    }
                });
            }
            drop(application);
            if committed {
                owner.applications.run(owner.clone());
                #[cfg(test)]
                owner
                    .configuration
                    .test_hooks
                    .reach("after_coordination_transfer");
            }
            projection.map_err(SourceSettingsError::Source)
        })
        .await
        .map_err(|_| {
            SourceSettingsError::Source(super::configuration::settings::SettingsError::Committed)
        })?
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
        let registry = self.registry.0.lock().expect("registry mutex");
        !registry.fenced(id)
            && matches!(registry.entries.get(id),
            Some(Entry::Loaded(runtime)) if runtime.identity.incarnation == incarnation)
    }
    /// Fence runtime admission, prove native writer retirement, then ask the
    /// durable owner to validate and commit the confirmed deletion target.
    /// Cancellation and uncertain retirement leave admission closed.
    pub(crate) async fn delete_session(
        &self,
        session: &SessionId,
        revision: &str,
    ) -> Result<super::session::deletion::SessionDeleteResult, RuntimeManagerError> {
        let resident = {
            let mut registry = self.registry.0.lock().expect("registry mutex");
            if !registry.retiring_sessions.insert(session.clone()) {
                return Err(error("Session retirement already owns admission"));
            }
            registry.by_session.get(session).cloned()
        };
        #[cfg(test)]
        if let Ok(snapshot) = self.sessions.read_session(session).await {
            self.probe(&snapshot.active_conversation_id)
                .after_delete_fence
                .park()
                .await;
        }
        if let Some(id) = resident {
            match self.retire(&id).await {
                Outcome::WriterAbsent(_) => {} // Composition errors cannot undo absence proof.
                Outcome::RetirementUnproven(error) => return Err(error),
                Outcome::Resident(_) => unreachable!("retirement joins every published writer"),
            }
        }
        // WriterAbsent is proof even if composition failed. Only now may catalog
        // deletion acquire destructive Conversation exclusion.
        let result = self
            .sessions
            .delete_session(session, revision)
            .await
            .map_err(error);
        if matches!(
            &result,
            Ok(
                super::session::deletion::SessionDeleteResult::Deleted { .. }
                    | super::session::deletion::SessionDeleteResult::CommittedCleanupPending { .. }
                    | super::session::deletion::SessionDeleteResult::CommittedDurabilityUncertain { .. }
                    | super::session::deletion::SessionDeleteResult::NotFound { .. }
            )
        ) {
            self.applications.forget(session.as_ref());
            self.sessions
                .configuration_bindings
                .lock()
                .expect("Session configuration bindings")
                .remove(session);
        }
        if !matches!(
            result,
            Ok(super::session::deletion::SessionDeleteResult::CommittedDurabilityUncertain { .. })
        ) {
            self.registry
                .0
                .lock()
                .expect("registry mutex")
                .retiring_sessions
                .remove(session);
        }
        result
    }

    /// Reconcile durable deletion state before retrying committed cleanup.
    /// Live observations never release another delete's admission ownership.
    pub(crate) async fn recover_session_deletion(
        &self,
        session: &SessionId,
    ) -> Result<super::session::deletion::SessionDeleteResult, RuntimeManagerError> {
        use super::session::deletion::SessionDeleteResult;
        let observed = self.sessions.delete_preview(session).await;
        if !matches!(
            observed,
            SessionDeleteResult::CommittedCleanupPending { .. }
                | SessionDeleteResult::CommittedDurabilityUncertain { .. }
                | SessionDeleteResult::NotFound { .. }
        ) {
            return Ok(observed);
        }
        // Durable recovery cannot substitute for retirement of a managed writer.
        if self
            .registry
            .0
            .lock()
            .expect("registry mutex")
            .by_session
            .contains_key(session)
        {
            return Err(error(
                "writer retirement must be proven before deletion recovery",
            ));
        }
        let result = if matches!(observed, SessionDeleteResult::NotFound { .. }) {
            self.sessions.confirm_deletion_absence(session).await
        } else {
            // These preview outcomes identify an existing frozen deletion record.
            self.sessions.recover_deletion(session).await
        };
        if matches!(
            result,
            SessionDeleteResult::Deleted { .. }
                | SessionDeleteResult::NotFound { .. }
                | SessionDeleteResult::CommittedCleanupPending { .. }
        ) {
            // Durable absence or a durable frozen record now excludes admission.
            // Idempotent, including recovery racing the original cleanup worker.
            self.registry
                .0
                .lock()
                .expect("registry mutex")
                .retiring_sessions
                .remove(session);
        }
        Ok(result)
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
        let (access, paths) = tokio::task::spawn_blocking(move || {
            let retained = bindings
                .lock()
                .expect("Session configuration bindings")
                .get(&access.session.id)
                .cloned();
            let paths = if let Some(paths) = retained {
                paths
            } else {
                let paths = applications
                    .lock()
                    .initial_binding(&configuration, &access.settings.input(), &credentials)
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
        let registry =
            super::composition::load_model_registry(&paths, &self.dependencies).map_err(error)?;
        let core = LocalConversationCore::compose_with_access(
            &paths,
            &self.dependencies,
            registry,
            paths.config().clone(),
            super::session::SessionPersistentState::from_input(&paths.input),
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
                    super::session_display_projection::arm_display_projection(
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
    async fn retire(&self, id: &ConversationId) -> Outcome {
        loop {
            let flight = {
                let mut registry = self.registry.0.lock().expect("registry mutex");
                match registry.entries.get(id) {
                    None => return Outcome::WriterAbsent(Ok(())),
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
    fn spawn_unload(
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
            runtime
                .operations
                .subscribe()
                .wait_for(|count| *count == 0)
                .await
                .expect("resident owns operation drain");
            let live = runtime.shutdown_runtime().expect("resident composition");
            if let Err(e) = live.shutdown().await {
                terminal.finish(Err(error(format!("{e:?}"))));
                return;
            }
            drop(live);
            let host = runtime
                .composition
                .lock()
                .expect("composition mutex")
                .as_ref()
                .expect("resident composition")
                .host()
                .clone();
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
/// it. Failed Loading is removed; failed Unloading retains the closed old writer.
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

#[derive(Debug)]
pub enum SourceSettingsError {
    Source(super::configuration::settings::SettingsError),
}
