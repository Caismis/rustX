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

use super::composition::{LocalConversationRuntime, LocalRuntimeDependencies};
use super::configuration::UserConfigManager;
use super::session::{SessionId, SessionNodeId};
use super::session_controller::SessionController;
use crate::credentials::CredentialSnapshot;
use crate::runtime::conversation_runtime::ConversationRuntime;
use crate::runtime::identity::ConversationId;

mod client;
mod configuration;
mod drain;
mod lifecycle;
mod model_selection;
mod schedule;
mod schedule_driver;
mod sessions;

pub(crate) use client::ConfigurationObservationSource;
pub use client::ManagedRuntimeClient;
pub use configuration::SourceSettingsError;
pub(crate) use model_selection::ModelSelectionError;

pub(super) type NativeOperationReply = Result<
    crate::runtime_client::types::RuntimeClientResult,
    crate::runtime_client::types::RuntimeClientError,
>;
pub(super) type NativeOperationTail =
    futures_util::future::BoxFuture<'static, NativeOperationReply>;

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
    ScheduleStore(crate::durable::ConversationStoreError),
    Client(crate::runtime_client::types::RuntimeClientError),
    SessionAlreadyResident {
        session_id: SessionId,
        resident_conversation: ConversationId,
        requested_conversation: ConversationId,
    },
    StaleIncarnation,
    LoadingCancelled,
    Draining,
    ResidencyCapacity,
    TransitionFailed(String),
}
impl std::fmt::Display for RuntimeManagerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ScheduleStore(error) => error.fmt(f),
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
            Self::LoadingCancelled => f.write_str("runtime loading was cancelled"),
            Self::Draining => f.write_str("runtime manager is draining"),
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

/// One initialization's retirement provenance and native admission handoff.
/// The short control mutex never covers native retirement or activation.
#[derive(Debug, Default)]
pub(crate) struct Initialization {
    cancellation: crate::runtime::cancellation::CancellationSignal,
    control: Mutex<InitializationControl>,
}
#[derive(Debug, Default)]
struct InitializationControl {
    retirement_requested: bool,
    prepared_runtime: Option<ConversationRuntime>,
}
impl Initialization {
    /// Safe under residency/host locks: mark provenance, without native work.
    fn request_retirement(&self) {
        let mut control = self.control.lock().expect("initialization control");
        control.retirement_requested = true;
        if control.prepared_runtime.is_none() {
            // A publisher cannot install a native control until this signal is
            // cancelled, and will observe the request before activation.
            self.cancellation.cancel();
        }
    }
    /// Close native admission before publishing cancellation to observers.
    /// Call only after releasing host, registry and control locks.
    pub(crate) fn retire(&self) {
        let prepared = {
            let mut control = self.control.lock().expect("initialization control");
            control.retirement_requested = true;
            if control.prepared_runtime.is_none() {
                self.cancellation.cancel();
            }
            control.prepared_runtime.clone()
        };
        if let Some(runtime) = prepared {
            let _ = runtime.begin_initialization_retirement();
            self.cancellation.cancel();
        }
    }
    fn is_retiring(&self) -> bool {
        self.control
            .lock()
            .expect("initialization control")
            .retirement_requested
    }
    fn install_prepared(&self, runtime: ConversationRuntime) -> bool {
        let mut control = self.control.lock().expect("initialization control");
        assert!(control.prepared_runtime.replace(runtime).is_none());
        !control.retirement_requested
    }
    fn clear_prepared(&self) {
        self.control
            .lock()
            .expect("initialization control")
            .prepared_runtime = None;
    }
    #[cfg(test)]
    fn prepared_runtime(&self) -> Option<ConversationRuntime> {
        self.control
            .lock()
            .expect("initialization control")
            .prepared_runtime
            .clone()
    }
}

#[derive(Debug)]
struct Flight {
    startup: super::composition::McpInitialSubmission,
    initialization: Arc<Initialization>,
    prepares_runtime: std::sync::atomic::AtomicBool,
    target: std::sync::OnceLock<(SessionId, SessionNodeId)>,
    submission_restart: Mutex<Option<(SessionId, SessionNodeId)>>,
    result: watch::Sender<Option<Outcome>>,
}
impl Flight {
    fn new() -> Arc<Self> {
        Self::with_startup(super::composition::McpInitialSubmission::default())
    }
    fn with_startup(startup: super::composition::McpInitialSubmission) -> Arc<Self> {
        Arc::new(Self {
            startup,
            initialization: Arc::new(Initialization::default()),
            prepares_runtime: std::sync::atomic::AtomicBool::new(false),
            target: std::sync::OnceLock::new(),
            submission_restart: Mutex::new(None),
            result: watch::channel(None).0,
        })
    }
    fn cancel_loading(&self) {
        self.initialization.retire();
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

/// One accepted real submission, retained by its counted native request until
/// exact target registration settles. This owns no Session or physical resource.
#[derive(Debug)]
struct AcceptedSubmission {
    session: SessionId,
    node: SessionNodeId,
    intent: super::composition::SubmissionIntent,
    initialization: Mutex<Option<Arc<Initialization>>>,
}
impl AcceptedSubmission {
    fn bind(&self, initialization: &Arc<Initialization>) -> bool {
        let mut owner = self
            .initialization
            .lock()
            .expect("submission initialization");
        if owner
            .as_ref()
            .is_some_and(|owner| owner.is_retiring() || !Arc::ptr_eq(owner, initialization))
        {
            return false;
        }
        if initialization.is_retiring() {
            return false;
        }
        owner.get_or_insert_with(|| initialization.clone());
        true
    }
    fn is_cancelled(&self) -> bool {
        self.initialization
            .lock()
            .expect("submission initialization")
            .as_ref()
            .is_some_and(|owner| owner.is_retiring())
    }
}

pub(crate) struct SubmissionReceipt {
    accepted: Arc<AcceptedSubmission>,
    registry: Weak<RuntimeRegistry>,
}
impl Drop for SubmissionReceipt {
    fn drop(&mut self) {
        if let Some(registry) = self.registry.upgrade() {
            registry
                .0
                .lock()
                .expect("registry mutex")
                .submission_receipts
                .retain(|receipt| {
                    receipt
                        .upgrade()
                        .is_some_and(|accepted| !Arc::ptr_eq(&accepted, &self.accepted))
                });
        }
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
    node: SessionNodeId,
    initialization: Arc<Initialization>,
    identity: Arc<ManagedRuntime>,
    composition: Mutex<Option<LocalConversationRuntime>>,
    scheduled_wake: Arc<tokio::sync::Notify>,
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
    pub storage_only: usize,
    pub schedule_commits: usize,
    pub schedule_reads: usize,
    pub schedule_drivers: usize,
    pub schedule_write_fenced: Vec<ConversationId>,
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

impl ResidentRuntime {
    /// Scoped native request owners release their leases before composition can retire.
    async fn settle_operations(&self) {
        self.operations
            .subscribe()
            .wait_for(|count| *count == 0)
            .await
            .expect("resident owns operation drain");
    }

    // Shutdown alone may clone execution authority. Never exposed to clients.
    fn shutdown_runtime(&self) -> Option<ConversationRuntime> {
        self.composition
            .lock()
            .expect("composition mutex")
            .as_ref()
            .map(|c| c.runtime().clone())
    }
}

#[derive(Debug)]
enum Entry {
    StorageOnly(Arc<schedule::StorageFlight>),
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
    draining: bool,
    policy: RuntimeResidencyPolicy,
    refusals: std::collections::BTreeMap<String, u64>,
    unload_failures: u64,

    schedule_open: bool,
    schedule_write_fenced: std::collections::HashSet<ConversationId>,
    schedule_reads: watch::Sender<usize>,
    schedule_drivers: watch::Sender<usize>,
    schedule_commits: watch::Sender<usize>,
    // Runtime residency index only, excluding finite StorageOnly entries.
    // Covers every runtime entry,
    // including replacement handoff and failed (unproven) shutdown.
    by_session: HashMap<SessionId, ConversationId>,
    // Only live, host-admitted Send requests can retain these weak entries.
    submission_receipts: Vec<Weak<AcceptedSubmission>>,
    // Session admission fencing shares every registry transition, including absence.
    retiring_sessions: std::collections::HashSet<SessionId>,
    #[cfg(test)]
    probes: HashMap<ConversationId, Arc<tests::Probe>>,
    #[cfg(test)]
    schedule_scan_done: Arc<tests::AsyncGate>,
    #[cfg(test)]
    schedule_activation_started: watch::Sender<Option<ConversationId>>,
    #[cfg(test)]
    schedule_waiting: Option<watch::Sender<u64>>,
    #[cfg(test)]
    reaper_waiting: Option<watch::Sender<u64>>,
}
impl RegistryState {
    fn initialize_loading(
        &self,
        flight: &Flight,
        session: &SessionId,
        node: &SessionNodeId,
    ) -> Result<(), RuntimeManagerError> {
        if self.draining {
            return Err(RuntimeManagerError::Draining);
        }
        let target = flight
            .target
            .get_or_init(|| (session.clone(), node.clone()));
        assert_eq!(
            target,
            &(session.clone(), node.clone()),
            "one Flight has one exact target"
        );
        for accepted in self.submission_receipts.iter().filter_map(Weak::upgrade) {
            if accepted.session == *session
                && accepted.node == *node
                && accepted.bind(&flight.initialization)
            {
                flight.startup.record(accepted.intent).map_err(error)?;
            }
        }
        Ok(())
    }

    fn fenced(&self, id: &ConversationId) -> bool {
        self.draining
            || matches!(self.entries.get(id),Some(Entry::StorageOnly(flight)) if self.retiring_sessions.contains(&flight.binding.session_id))
            || self.by_session.iter().any(|(session, resident)| {
                resident == id && self.retiring_sessions.contains(session)
            })
    }

    fn refuse(&mut self, reason: &str) {
        let count = self.refusals.entry(reason.into()).or_default();
        *count = count.saturating_add(1);
    }
    fn reserve(&mut self) -> Result<(), RuntimeManagerError> {
        if self.by_session.len() >= self.policy.max_resident_runtimes {
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
        if self.draining {
            return Err(RuntimeManagerError::Draining);
        }
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
    fn session_entries(&self, session: &SessionId) -> Vec<ConversationId> {
        self.entries
            .iter()
            .filter(|(id, entry)| {
                matches!(entry,Entry::StorageOnly(flight) if &flight.binding.session_id==session)
                    || self.by_session.get(session) == Some(*id)
            })
            .map(|(id, _)| id.clone())
            .collect()
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
#[derive(Clone)]
pub struct SessionRuntimeManager {
    pub(super) sessions: SessionController,
    registry: Arc<RuntimeRegistry>,
    pub(super) configuration: UserConfigManager,
    pub(super) credentials: CredentialSnapshot,
    pub(super) process_policy: Arc<std::sync::RwLock<super::app_server_policy::AppServerPolicy>>,
    pub(super) applications: super::configuration::application::ConfigurationApplications,
    dependencies: Arc<LocalRuntimeDependencies>,
    clock: Arc<dyn MonotonicClock>,
    schedule_clock: Arc<dyn crate::runtime::types::RuntimeClock>,
    schedule_changed: Arc<tokio::sync::Notify>,
    schedule_dirty:
        Arc<Mutex<std::collections::BTreeMap<ConversationId, crate::scheduler::ScheduleBinding>>>,
    schedule_stop: tokio_util::sync::CancellationToken,
    schedule_diagnostics: Arc<Mutex<crate::scheduler::ScheduleDriverDiagnostics>>,
}
impl std::fmt::Debug for SessionRuntimeManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionRuntimeManager")
            .field("registry", &self.registry)
            .finish_non_exhaustive()
    }
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
                        Entry::StorageOnly(_) => unreachable!("storage is not runtime residency"),
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
                    storage_only: state
                        .entries
                        .values()
                        .filter(|entry| matches!(entry, Entry::StorageOnly(_)))
                        .count(),
                    schedule_commits: *state.schedule_commits.borrow(),
                    schedule_write_fenced: state.schedule_write_fenced.iter().cloned().collect(),
                    schedule_reads: *state.schedule_reads.borrow(),
                    schedule_drivers: *state.schedule_drivers.borrow(),
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
                schedule_open: true,
                schedule_write_fenced: std::collections::HashSet::new(),
                schedule_reads: watch::channel(0).0,
                schedule_drivers: watch::channel(0).0,
                schedule_commits: watch::channel(0).0,
                draining: false,
                by_session: HashMap::new(),
                submission_receipts: Vec::new(),
                retiring_sessions: std::collections::HashSet::new(),
                refusals: std::collections::BTreeMap::default(),
                unload_failures: 0,
                #[cfg(test)]
                probes: HashMap::new(),
                #[cfg(test)]
                schedule_scan_done: Arc::default(),
                #[cfg(test)]
                schedule_activation_started: watch::channel(None).0,
                #[cfg(test)]
                schedule_waiting: None,
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
            schedule_clock: Arc::new(crate::runtime::types::SystemClock),
            schedule_changed: Arc::default(),
            schedule_dirty: Arc::default(),
            schedule_stop: tokio_util::sync::CancellationToken::new(),
            schedule_diagnostics: Arc::default(),
        })
    }

    /// A live node's read authorization comes from its sole native Projection.
    /// Loading/unloading residency cannot silently become a cold Store-head read.
    pub(crate) fn history_observation_cut(
        &self,
        conversation: &ConversationId,
    ) -> Result<Option<crate::observation::cut::CheckpointReadCut>, RuntimeManagerError> {
        let resident = {
            let state = self.registry.0.lock().expect("registry mutex");
            match state.entries.get(conversation) {
                None => return Ok(None),
                Some(Entry::Loaded(resident)) if !state.fenced(conversation) => resident.clone(),
                _ => return Err(RuntimeManagerError::StaleIncarnation),
            }
        };
        let lease = resident.identity.client().admit_operation()?;
        let host = resident
            .composition
            .lock()
            .expect("composition mutex")
            .as_ref()
            .ok_or(RuntimeManagerError::StaleIncarnation)?
            .host()
            .clone();
        let cut = host
            .published_read_cut()
            .map_err(RuntimeManagerError::Client)?;
        drop(lease);
        Ok(Some(cut))
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
            None | Some(Entry::StorageOnly(_)) => ResidencyState::Unloaded,
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
pub(crate) mod tests;
