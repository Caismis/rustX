//! Process-owned local execution binding claims for Sessions.
//!
//! The registry reserves one Server-side execution binding per Session for
//! the trusted native composer only. Nothing here is exposed through the
//! App Server protocol: credential and bootstrap material never cross a
//! client request. Admission is decided under the catalog visibility cut
//! and the residency mutex together; the private publication — credential
//! generation, socket bind, bootstrap write — happens exactly once on a
//! spawned blocking task after the slot is claimed. A published endpoint is
//! never replaced, a closed claim is never recovered, and endpoint teardown
//! never runs under a registry or residency mutex. Each published endpoint
//! can admit exactly one Client connection — stored once, never replaced —
//! and dispatch bounded range reads over it; at most one synchronous
//! exchange runs at a time and a concurrent dispatch is refused as busy,
//! never queued.

pub(super) mod local_sessions;
mod mcp;
mod output;
mod sibling;
mod skill_sync;
pub(crate) use mcp::{ReceiveExchange, StdioMcpBinding};
pub(crate) use output::OutputBinding;
use sibling::SiblingClient;
#[cfg(target_os = "linux")]
pub(crate) use sibling::maintain as maintain_local_client;
pub(crate) use sibling::{LocalClient, launch_sibling};

use std::collections::{BTreeSet, HashMap};
use std::io;
use std::num::NonZeroU64;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, TryLockError, Weak};
use std::time::Instant;

use rustx_execution_protocol::watch::{WatchChange, WatchId};
use rustx_execution_protocol::{
    Capability, ClientInstanceId, EntryKind, EntryStamp, EnvironmentId, FileStamp, Generation,
    MAX_DATA_RECORDS_PER_CONNECTION, Reject, TreeMutation, is_normalized_absolute_path,
    transport::{LOCAL_HANDSHAKE_TIMEOUT, PAGE_REQUEST_TIMEOUT},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::runtime::ExecutionCancellation;
use rustx_execution_server::{
    Binding, DirectoryListing, DocumentPage, LinesPage, LocalConnection, RangeRead, Reservation,
    SearchResult,
};

use super::super::session::SessionId;
use super::{RuntimeManagerError, SessionRuntimeManager, error};

mod delegation;
pub(crate) use delegation::{ChildExecutionGrant, ChildExecutionRelay, ChildExecutionSettlement};

/// One Session's local execution endpoint, published once.
///
/// Owned by the trusted native composer: the App Server protocol has no
/// path to the listener, the bundle, or the credential inside it. Nothing
/// spawns a Client or connects by itself — the composer hands each
/// accepted stream through [`admit_local_stream`](Self::admit_local_stream)
/// and dispatches through [`read_range`](Self::read_range).
pub struct LocalExecutionEndpoint {
    authority: EndpointAuthority,
    /// The data lane of the one admitted Client. A recovered pair replaces
    /// only an already-detached lane of the same authorized instance.
    connection: Mutex<Option<LocalConnection>>,
    connection_released: tokio::sync::Notify,
    accept_stop: tokio_util::sync::CancellationToken,
    settlement: Mutex<SettlementState>,
    settlement_changed: std::sync::Condvar,
    #[cfg(test)]
    pub(crate) settlement_joined: tokio::sync::Notify,
    #[cfg(test)]
    test_hooks: Mutex<ExecutionTestHooks>,
    #[cfg(feature = "test-support")]
    save_abort_gate: Mutex<Option<output::SaveAbortGate>>,
    /// The total in-flight data budget on the connection — Agent page
    /// exchanges *and* management queries share these
    /// [`MAX_DATA_RECORDS_PER_CONNECTION`] permits, so the wire never
    /// carries more work than one connection can account for.
    /// Acquiring is `try_acquire` everywhere: a full budget is the
    /// bounded busy signal, never a queue.
    data_slots: Arc<Semaphore>,
    /// The single Agent page writer. An admitted Agent dispatch waits
    /// for this permit on the async side — under its absolute deadline
    /// and cancellation, never on a blocking-pool thread — so concurrent
    /// Agent reads take turns instead of failing the connection's busy
    /// signal. Management callers never take it: they still answer
    /// `WouldBlock` immediately.
    agent_writer: Arc<Semaphore>,
    /// The reserved logical workspace — trusted catalog identity,
    /// recorded once at claim time, never probed on disk.
    workspace: PathBuf,
    /// The committed pair's admission — set once when the Live
    /// connection lands; the operation-identity source for every
    /// managed process this endpoint dispatches.
    admission: std::sync::OnceLock<rustx_execution_protocol::Admission>,
    user_root: std::sync::OnceLock<PathBuf>,
    managed_output: std::sync::OnceLock<bool>,
    stdio_mcp: std::sync::OnceLock<bool>,
    user_skill_flights: skill_sync::SyncFlights,
    recovery_fence: Option<rustx_execution_server::RecoveryFence>,
}

enum Settlement {
    Open,
    Closing,
    Revoking,
    Finished(Result<(), Arc<io::Error>>),
}

/// The endpoint publishes its physical startup owner under the same lock as
/// retirement. The lease holds no mutex while the child is started or reaped.
struct SettlementState {
    phase: Settlement,
    startup: ClientStartup,
}

impl Default for SettlementState {
    fn default() -> Self {
        Self {
            phase: Settlement::Open,
            startup: ClientStartup::Idle,
        }
    }
}

enum ClientStartup {
    Idle,
    Owned,
    FinishedAt(Instant),
    Unconfirmed(Arc<io::Error>),
}

struct ClientStartupOwner<'a> {
    endpoint: &'a LocalExecutionEndpoint,
    complete: bool,
}

impl ClientStartupOwner<'_> {
    fn finish(mut self, unconfirmed: Option<Arc<io::Error>>) {
        self.publish(unconfirmed);
        self.complete = true;
    }

    fn publish(&self, unconfirmed: Option<Arc<io::Error>>) {
        let mut state = self
            .endpoint
            .settlement
            .lock()
            .expect("execution settlement");
        state.startup = unconfirmed.map_or_else(
            || ClientStartup::FinishedAt(Instant::now()),
            ClientStartup::Unconfirmed,
        );
        self.endpoint.settlement_changed.notify_all();
        self.endpoint.connection_released.notify_waiters();
    }
}

impl Drop for ClientStartupOwner<'_> {
    fn drop(&mut self) {
        if !self.complete {
            self.publish(Some(Arc::new(io::Error::other(
                "local Client startup owner was dropped",
            ))));
        }
    }
}

#[cfg(test)]
#[derive(Default)]
struct ExecutionTestHooks {
    expire_bash_capture_query: bool,
    after_user_skill_capture: Option<Box<dyn FnOnce() + Send>>,
    local_startup: Option<super::super::configuration::ConfigurationHooks>,
    before_settlement_cleanup: Option<(
        tokio::sync::oneshot::Sender<()>,
        std::sync::mpsc::Receiver<()>,
    )>,
    delegated_open: Option<delegation::OpenCut>,
    preparation_cleanup: std::collections::BTreeMap<
        rustx_execution_protocol::DecimalU64,
        tokio::sync::oneshot::Sender<()>,
    >,
    cancel_fence: std::collections::BTreeMap<rustx_execution_protocol::DecimalU64, CancelFenceHook>,
}

#[cfg(test)]
struct CancelFenceHook {
    ready: tokio::sync::oneshot::Sender<Option<rustx_execution_protocol::CancelDisposition>>,
    resume: tokio::sync::oneshot::Receiver<()>,
    done: tokio::sync::oneshot::Sender<()>,
}

#[cfg(test)]
pub(crate) struct CancelMutationFenceTest {
    pub(crate) decision_ready:
        tokio::sync::oneshot::Receiver<Option<rustx_execution_protocol::CancelDisposition>>,
    pub(crate) resume: tokio::sync::oneshot::Sender<()>,
    pub(crate) decision_done: tokio::sync::oneshot::Receiver<()>,
}

enum EndpointAuthority {
    Reserved(Box<ReservedExecutionEndpoint>),
    Delegated {
        control: rustx_execution_server::ControlHandle,
        shutdown: rustx_execution_transport::ChannelShutdown,
        work: rustx_execution_server::delegation::DelegatedWork,
    },
}

struct ReservedExecutionEndpoint {
    binding: Binding,
    listener: ExecutionListener,
    bootstrap: PathBuf,
    /// The fd-and-dirfd proof of the bundle `create_new` wrote at
    /// reservation. `Drop` removes the file only while `fstatat` still
    /// matches this proof — a foreign replacement is never unlinked and
    /// a FIFO at the path can never block teardown.
    bundle: rustx_execution_server::PublishedBundle,
    /// The private directory owning the local composition's socket and
    /// bootstrap. A remote-mode endpoint publishes its bundle into an
    /// operator-owned `0700` directory instead, so this is `None` there.
    _private: Option<tempfile::TempDir>,
}

fn settlement_result(result: &Result<(), Arc<io::Error>>) -> io::Result<()> {
    result
        .as_ref()
        .copied()
        .map_err(|error| io::Error::new(error.kind(), Arc::clone(error)))
}

/// The transport a Session's execution endpoint was reserved with. It is
/// decided at claim time — before any socket or credential exists — and
/// an occupied slot refuses a different transport rather than swapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExecutionTransport {
    /// The owner-only Unix socket of the local sibling composition.
    Unix,
    /// The loopback TCP WebSocket listener of the remote composition.
    WebSocket,
}

/// The outcome of one accepted connection on the endpoint.
///
/// `Pending` is a real half-admission — the pairing challenge is out —
/// but nothing is dispatchable and no connection is stored: the caller
/// keeps accepting for the control lane inside the same budget. `Live`
/// is the committed, stored pair. `Rejected` delivered the typed reject
/// on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointAdmission {
    /// The data half was admitted; the pairing control lane is awaited.
    Pending,
    /// The pair committed and the connection is stored.
    Live,
    /// The peer was refused; the reject was delivered.
    Rejected(Reject),
}

/// The single live listener of one reserved endpoint — exactly one mode,
/// never both.
pub enum ExecutionListener {
    /// The Unix listener of the local sibling composition.
    Unix(UnixListener),
    /// The loopback TCP listener of the remote composition.
    WebSocket(std::net::TcpListener),
}

impl LocalExecutionEndpoint {
    /// Reserves the binding inside a fresh owner-only private directory.
    ///
    /// `workspace` must be the exact normalized absolute path the Session
    /// catalog recorded. It is validated lexically and never touched on
    /// disk — the path authorizes the binding's workspace claim, it is not
    /// opened, canonicalized, or created.
    ///
    /// # Errors
    ///
    /// Returns `io::ErrorKind::InvalidInput` when `workspace` is not a
    /// normalized absolute path, or the OS error when the private
    /// directory, socket, or bootstrap bundle cannot be created.
    fn new(session: &SessionId, workspace: PathBuf) -> io::Result<Self> {
        Self::new_local(session, workspace, Generation::new(NonZeroU64::MIN))
    }

    fn new_local(
        session: &SessionId,
        workspace: PathBuf,
        generation: Generation,
    ) -> io::Result<Self> {
        if !is_normalized_absolute_path(&workspace) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "workspace must be a normalized absolute path",
            ));
        }
        let private = tempfile::Builder::new()
            .prefix("rx-")
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()?;
        // Only the private directory is canonicalized — the socket and
        // bundle must be normalized absolute children for `reserve_local`.
        let root = private.path().canonicalize()?;
        let reservation = Reservation {
            session: rustx_execution_protocol::SessionId::new(session.as_str()),
            environment: EnvironmentId::new(uuid::Uuid::now_v7().to_string()),
            instance: ClientInstanceId::new(uuid::Uuid::now_v7().to_string()),
            generation,
            workspace: workspace.clone(),
            required_capabilities: BTreeSet::from([Capability::Connection]),
        };
        let (binding, listener, bundle) =
            Binding::reserve_local(reservation, &root.join("s"), &root.join("b"))?;
        Ok(Self {
            authority: EndpointAuthority::Reserved(Box::new(ReservedExecutionEndpoint {
                binding,
                listener: ExecutionListener::Unix(listener),
                bootstrap: root.join("b"),
                bundle,
                _private: Some(private),
            })),
            connection: Mutex::new(None),
            connection_released: tokio::sync::Notify::new(),
            accept_stop: tokio_util::sync::CancellationToken::new(),
            settlement: Mutex::default(),
            settlement_changed: std::sync::Condvar::new(),
            #[cfg(test)]
            settlement_joined: tokio::sync::Notify::new(),
            #[cfg(test)]
            test_hooks: Mutex::default(),
            #[cfg(feature = "test-support")]
            save_abort_gate: Mutex::default(),
            data_slots: Arc::new(Semaphore::new(MAX_DATA_RECORDS_PER_CONNECTION)),
            agent_writer: Arc::new(Semaphore::new(1)),
            admission: std::sync::OnceLock::new(),
            user_root: std::sync::OnceLock::new(),
            managed_output: std::sync::OnceLock::new(),
            stdio_mcp: std::sync::OnceLock::new(),
            user_skill_flights: skill_sync::SyncFlights::default(),
            recovery_fence: None,
            workspace,
        })
    }

    /// Reserves a WebSocket binding: the loopback TCP listener plus the
    /// `0600` bootstrap bundle the operator privately hands to the remote
    /// Client. `bootstrap` must be a new path inside an existing
    /// owner-only `0700` directory the caller owns — a collision with a
    /// user file is refused by `create_new` and left untouched.
    ///
    /// `public_url`, when given, is the operator's `wss://…/execution/v10`
    /// ingress endpoint published in the bundle instead of the binding's
    /// dedicated loopback TLS address.
    fn new_websocket(
        session: &SessionId,
        workspace: PathBuf,
        listen: std::net::SocketAddr,
        bootstrap: PathBuf,
        public_url: Option<&str>,
    ) -> io::Result<Self> {
        if !is_normalized_absolute_path(&workspace) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "workspace must be a normalized absolute path",
            ));
        }
        let reservation = Reservation {
            session: rustx_execution_protocol::SessionId::new(session.as_str()),
            environment: EnvironmentId::new(uuid::Uuid::now_v7().to_string()),
            instance: ClientInstanceId::new(uuid::Uuid::now_v7().to_string()),
            generation: Generation::new(NonZeroU64::MIN),
            workspace: workspace.clone(),
            required_capabilities: BTreeSet::from([Capability::Connection]),
        };
        let (binding, listener, bundle) =
            Binding::reserve_websocket(reservation, listen, &bootstrap, public_url)?;
        Ok(Self {
            authority: EndpointAuthority::Reserved(Box::new(ReservedExecutionEndpoint {
                binding,
                listener: ExecutionListener::WebSocket(listener),
                bootstrap,
                bundle,
                _private: None,
            })),
            connection: Mutex::new(None),
            connection_released: tokio::sync::Notify::new(),
            accept_stop: tokio_util::sync::CancellationToken::new(),
            settlement: Mutex::default(),
            settlement_changed: std::sync::Condvar::new(),
            #[cfg(test)]
            settlement_joined: tokio::sync::Notify::new(),
            #[cfg(test)]
            test_hooks: Mutex::default(),
            #[cfg(feature = "test-support")]
            save_abort_gate: Mutex::default(),
            data_slots: Arc::new(Semaphore::new(MAX_DATA_RECORDS_PER_CONNECTION)),
            agent_writer: Arc::new(Semaphore::new(1)),
            admission: std::sync::OnceLock::new(),
            user_root: std::sync::OnceLock::new(),
            managed_output: std::sync::OnceLock::new(),
            stdio_mcp: std::sync::OnceLock::new(),
            user_skill_flights: skill_sync::SyncFlights::default(),
            recovery_fence: None,
            workspace,
        })
    }

    fn reservation(&self) -> &ReservedExecutionEndpoint {
        match &self.authority {
            EndpointAuthority::Reserved(reservation) => reservation,
            EndpointAuthority::Delegated { .. } => {
                unreachable!("delegated endpoints never reserve")
            }
        }
    }

    fn with_recovery_fence(mut self, product_root: &Path) -> io::Result<Self> {
        self.recovery_fence = Some(
            self.reservation()
                .binding
                .retain_recovery_fence(product_root)?,
        );
        Ok(self)
    }

    #[cfg(target_os = "linux")]
    fn arm_local_domain(&self) -> io::Result<rustx_execution_server::LocalDomainGrant> {
        self.recovery_fence
            .as_ref()
            .ok_or_else(|| io::Error::other("local Client requires a durable ownership marker"))?
            .arm_local_domain()
    }

    pub(crate) fn publish_external_settlement_grant(&self, path: &Path) -> io::Result<()> {
        self.recovery_fence
            .as_ref()
            .ok_or_else(|| io::Error::other("remote Client requires a durable ownership marker"))?
            .publish_external_grant(path)
    }

    /// The exact logical workspace this binding reserved — the trusted
    /// catalog identity recorded at claim time, never probed on disk.
    #[must_use]
    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    /// The owner-only bootstrap bundle path the trusted composer hands to
    /// the Execution Client it chooses to spawn.
    #[must_use]
    pub fn bootstrap_path(&self) -> &Path {
        &self.reservation().bootstrap
    }

    /// The bound listener for this binding — exactly one transport mode.
    #[must_use]
    pub fn listener(&self) -> &ExecutionListener {
        &self.reservation().listener
    }

    /// Admits the Client on one accepted local stream through this
    /// endpoint's binding.
    ///
    /// The full handshake runs here — Hello read, credential and identity
    /// arbitration, admission delivery — exactly as
    /// `Binding::accept_local` performs it, without holding the
    /// connection slot: the binding's own mutex is the arbiter. On
    /// success the live connection is stored once and never replaced: a
    /// later Hello is refused by the binding itself
    /// (`AlreadyConnected`/`RecoveryRequired`), never by swapping the
    /// slot.
    ///
    /// # Errors
    ///
    /// Returns the transport error when the handshake cannot complete. An
    /// authenticated-but-refused Hello is not an I/O error: the reject is
    /// delivered on the wire and returned as `Ok(Err(Reject))`.
    ///
    /// # Panics
    ///
    /// Panics if the connection mutex is poisoned.
    pub fn admit_local_stream(&self, stream: UnixStream) -> io::Result<EndpointAdmission> {
        self.admit_local_stream_deadline(
            stream,
            std::time::Instant::now() + LOCAL_HANDSHAKE_TIMEOUT,
        )
    }

    /// `admit_local_stream` against an absolute deadline armed by the
    /// caller — one budget covering the whole admission handshake, shared
    /// with whatever bounded wait produced the stream.
    ///
    /// # Errors
    ///
    /// Returns the transport error when the handshake cannot complete. An
    /// authenticated-but-refused Hello is not an I/O error: the reject is
    /// delivered on the wire and returned as `Ok(Err(Reject))`.
    ///
    /// # Panics
    ///
    /// Panics if the connection mutex is poisoned.
    pub fn admit_local_stream_deadline(
        &self,
        stream: UnixStream,
        deadline: std::time::Instant,
    ) -> io::Result<EndpointAdmission> {
        let admitted = self
            .reservation()
            .binding
            .accept_local_deadline(stream, deadline)?;
        Ok(self.store(admitted))
    }

    /// Admits the Client on one accepted WebSocket TCP peer: the RFC 6455
    /// upgrade — path and `Origin` enforcement included — runs inside
    /// `rustx_execution_transport::accept` under the same absolute
    /// deadline as the Hello handshake that follows, then the identical
    /// credential/identity arbitration and admission delivery run through
    /// the binding's transport-agnostic accept.
    ///
    /// # Errors
    ///
    /// Returns the transport error when the upgrade or handshake cannot
    /// complete. An authenticated-but-refused Hello is not an I/O error:
    /// the reject is delivered on the wire and returned as
    /// `Ok(Err(Reject))`.
    ///
    /// # Panics
    ///
    /// Panics if the connection mutex is poisoned.
    pub fn admit_websocket_stream_deadline(
        &self,
        stream: std::net::TcpStream,
        deadline: std::time::Instant,
    ) -> io::Result<EndpointAdmission> {
        let admitted = self
            .reservation()
            .binding
            .accept_websocket_deadline(stream, deadline)?;
        Ok(self.store(admitted))
    }

    /// Stores only the current paired data lane. Separate accept workers can
    /// finish out of order after loss and recovery; the binding owns that cut.
    fn store(&self, admitted: rustx_execution_server::ChannelAccept) -> EndpointAdmission {
        match admitted {
            rustx_execution_server::ChannelAccept::Pending => EndpointAdmission::Pending,
            rustx_execution_server::ChannelAccept::Live(connection) => {
                let mut slot = self.connection.lock().expect("endpoint connection");
                if !connection.is_current() {
                    return EndpointAdmission::Rejected(
                        rustx_execution_protocol::Reject::RecoveryRequired,
                    );
                }
                // Operation identity and capability coverage remain frozen.
                let _ = self
                    .managed_output
                    .set(connection.supports(Capability::ManagedOutput));
                let _ = self
                    .stdio_mcp
                    .set(connection.supports(Capability::StdioMcp));
                let _ = self.user_root.set(connection.user_root().to_owned());
                let _ = self.admission.set(connection.admission().clone());
                let retired = slot.replace(connection);
                drop(slot);
                drop(retired);
                self.connection_released.notify_waiters();
                let cleanup_only = {
                    let state = self.settlement.lock().expect("execution settlement");
                    // The physical startup owner is awaiting this admission.
                    // Let it receive the pair and transfer or reap the child;
                    // dispatch is already closed and settlement is joining it.
                    !matches!(state.startup, ClientStartup::Owned)
                        && matches!(state.phase, Settlement::Closing | Settlement::Revoking)
                };
                if cleanup_only {
                    // A logically closed owner can accept its old Client only
                    // to obtain physical cleanup. dispatch_open stays false.
                    if self
                        .reservation()
                        .binding
                        .revocation_cutoff()
                        .is_some_and(|host| Instant::now() >= host)
                    {
                        self.close_transport();
                        return EndpointAdmission::Rejected(
                            rustx_execution_protocol::Reject::Revoked,
                        );
                    }
                    let command_deadline = Instant::now()
                        + rustx_execution_protocol::transport::CONTROL_COMMAND_TIMEOUT;
                    let cutoff = self
                        .reservation()
                        .binding
                        .revocation_cutoff()
                        .map_or(command_deadline, |host| command_deadline.min(host));
                    let mut result = self.fence_connection_by(cutoff);
                    // A prior lane may have failed while this authenticated pair
                    // was being committed. Join that attempt before using the new lane.
                    if result.is_err()
                        && self.reservation().binding.control().is_some()
                        && matches!(
                            self.settlement.lock().expect("execution settlement").phase,
                            Settlement::Closing
                        )
                    {
                        result = self.fence_connection_by(cutoff);
                    }
                    if let Err(failure) = result {
                        tracing::warn!(%failure, "recovered Client cleanup is unconfirmed");
                    }
                    return EndpointAdmission::Rejected(rustx_execution_protocol::Reject::Revoked);
                }
                EndpointAdmission::Live
            }
            rustx_execution_server::ChannelAccept::Rejected(reject) => {
                EndpointAdmission::Rejected(reject)
            }
        }
    }

    /// The live pair's cloneable control handle — health, mutation
    /// cancellation, process cancellation, and revocation independent
    /// of the data lane's dispatch lock. `None` while no pair is live.
    #[must_use]
    pub fn control(&self) -> Option<rustx_execution_server::ControlHandle> {
        match &self.authority {
            EndpointAuthority::Reserved(reservation) => reservation.binding.control(),
            EndpointAuthority::Delegated { control, .. } => Some(control.clone()),
        }
    }

    /// The committed pair's admission — the complete identity source
    /// for managed-process operation keys. `None` before a pair lands.
    #[must_use]
    pub fn admission(&self) -> Option<&rustx_execution_protocol::Admission> {
        self.admission.get()
    }

    /// One managed-process exchange through the admitted Client.
    ///
    /// Scheduled per RPC — the connection lock is released the moment
    /// the reply arrives, so a caller driving one operation through
    /// `Register`/`Start`/`Query`/`OutputPage`/`InputReceipt`/`Release`
    /// exchanges never holds the lane across the process's lifetime; a
    /// concurrent dispatch answers `WouldBlock` like every other
    /// management exchange.
    ///
    /// # Errors
    ///
    /// Returns the same gating kinds as [`read_range`](Self::read_range)
    /// and the typed exchange's own errors.
    pub fn process(
        &self,
        request: &rustx_execution_protocol::process::ProcessRequest,
    ) -> io::Result<rustx_execution_protocol::process::ProcessReply> {
        self.with_connection(None, false, |connection| connection.process(request))
    }

    /// [`Self::process`] with the exchange bounded by one absolute `deadline`.
    ///
    /// # Errors
    ///
    /// As [`Self::process`], plus [`io::ErrorKind::TimedOut`] when the
    /// deadline passed before the request was sent.
    pub fn process_until(
        &self,
        request: &rustx_execution_protocol::process::ProcessRequest,
        deadline: std::time::Instant,
    ) -> io::Result<rustx_execution_protocol::process::ProcessReply> {
        self.with_connection(None, false, |connection| {
            connection.process_until(request, deadline)
        })
    }

    pub(crate) fn retry_process_after_lane_loss(
        &self,
        request: &rustx_execution_protocol::process::ProcessRequest,
        error: &io::Error,
    ) -> bool {
        use rustx_execution_protocol::process::ProcessRequest;
        matches!(
            request,
            ProcessRequest::Start { .. }
                | ProcessRequest::Query { .. }
                | ProcessRequest::OutputPage { .. }
                | ProcessRequest::BashCapture { .. }
                | ProcessRequest::BashOutputTail { .. }
                | ProcessRequest::ReleaseOutput { .. }
        ) && rustx_execution_protocol::transport::recoverable_lane_loss(error)
            && self.dispatch_open()
            && match &self.authority {
                EndpointAuthority::Reserved(reserved) => {
                    reserved.binding.may_recover() || reserved.binding.control().is_some()
                }
                EndpointAuthority::Delegated { .. } => false,
            }
    }

    /// The input owner's final receipt poll waits for data capacity before
    /// leaving the sequence to binding teardown. Called from the blocking
    /// exchange worker under the same absolute wire deadline.
    pub(crate) fn finish_input_receipt_by(
        &self,
        request: &rustx_execution_protocol::process::ProcessRequest,
        deadline: Instant,
    ) -> io::Result<rustx_execution_protocol::process::ProcessReply> {
        self.with_bounded_data_connection(deadline, |connection| {
            connection.process_until(request, deadline)
        })
    }

    pub(crate) fn process_key(
        &self,
        operation: rustx_execution_protocol::process::OperationId,
    ) -> io::Result<rustx_execution_protocol::process::ProcessOperationKey> {
        let admission = self.admission().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotConnected,
                "Session execution unavailable: no admitted Execution Client",
            )
        })?;
        Ok(rustx_execution_protocol::process::ProcessOperationKey {
            incarnation: admission.server_incarnation,
            session: admission.session.clone(),
            instance: admission.instance.clone(),
            generation: admission.generation,
            operation,
        })
    }

    pub(crate) fn agent_process(
        &self,
        request: &rustx_execution_protocol::process::ProcessRequest,
        deadline: Instant,
        admit: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<rustx_execution_protocol::process::ProcessReply> {
        self.with_connection(None, false, |connection| {
            connection.process_admitted(request, deadline, admit)
        })
    }

    /// One bounded, revision-checked range read through the admitted
    /// Client — the first dispatch path on the endpoint.
    ///
    /// At most one synchronous exchange runs per connection: a concurrent
    /// caller gets `WouldBlock` — a bounded busy signal, never an
    /// unbounded wait queue. Without an admitted Client the call is
    /// `NotConnected`. `path` must name a file inside the Session's
    /// reserved workspace lexically: a relative path carries normal
    /// components only, an absolute path must strip the exact reserved
    /// workspace prefix — `..`, `.`, the workspace root itself, an empty
    /// path, and control or NUL bytes are all refused before anything is
    /// sent. The workspace is never canonicalized or statted; the gate
    /// that actually authorizes the send is the binding's live-connection
    /// check inside [`LocalConnection::read_range`], evaluated at dispatch
    /// time rather than by snapshot.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::WouldBlock`] while another exchange holds
    /// the connection, [`io::ErrorKind::NotConnected`] before a Client is
    /// admitted, [`io::ErrorKind::PermissionDenied`] for a path outside
    /// the Session's workspace or a revoked/detached binding, and the
    /// typed exchange's own errors for an untruthful or failed read.
    pub fn read_range(
        &self,
        path: &Path,
        offset: u64,
        max_bytes: u32,
        expected_revision: Option<&str>,
    ) -> io::Result<RangeRead> {
        self.with_connection(None, false, |connection| {
            let relative = workspace_relative(&connection.admission().workspace, path)?;
            connection.read_range(&relative, offset, max_bytes, expected_revision)
        })
    }

    /// Capture the bound Workspace document under the same data budget as
    /// other authorized reads. A missing leaf is meaningful only after the
    /// live Client has proved the Workspace directory.
    pub(crate) fn workspace_document(
        &self,
        relative: &Path,
    ) -> io::Result<WorkspaceDocumentSnapshot> {
        const CAPTURE_BUDGET: std::time::Duration = std::time::Duration::from_secs(5);
        let deadline = Instant::now() + CAPTURE_BUDGET;
        self.with_bounded_data_connection(deadline, |connection| {
            let root = connection.entry_metadata_until(Path::new(""), deadline)?;
            if root.kind != EntryKind::Directory {
                return Err(io::Error::other("Client Workspace root is not a directory"));
            }
            let document = connection.read_bounded_file(
                relative,
                rustx_bounded_file::MAX_RESOURCE_BYTES,
                deadline,
                || Ok(()),
            );
            let document = match document {
                Ok(document) => Some(document),
                Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                Err(error) => return Err(error),
            };
            Ok(WorkspaceDocumentSnapshot {
                directory_identity: root.stamp.identity,
                document,
            })
        })
    }

    // Callers run on a blocking thread. Capacity, connection contention and
    // wire I/O share one deadline.
    fn with_bounded_data_connection<T>(
        &self,
        deadline: Instant,
        dispatch: impl FnMut(&mut LocalConnection) -> io::Result<T>,
    ) -> io::Result<T> {
        tokio::runtime::Handle::current().block_on(async {
            let _data = wait_for_execution_permit(self.data_slots.clone(), deadline).await?;
            self.wait_for_connection(deadline, dispatch).await
        })
    }

    pub(crate) fn user_skills_inquiry(
        &self,
        request: &rustx_execution_protocol::skills::Request,
        deadline: Instant,
    ) -> io::Result<rustx_execution_protocol::skills::Reply> {
        self.with_bounded_data_connection(deadline, |connection| {
            connection.user_skills_until(request, deadline)
        })
    }

    /// Inspect the Client-visible `~/rustx` root and an optional fresh probe.
    pub(crate) fn inspect_user_skill_root(
        &self,
        probe: Option<String>,
        deadline: Instant,
    ) -> io::Result<(PathBuf, Option<rustx_execution_protocol::skills::Probe>)> {
        match self.user_skills_inquiry(
            &rustx_execution_protocol::skills::Request::Root { probe },
            deadline,
        )? {
            rustx_execution_protocol::skills::Reply::Root { path, probe }
                if self.user_root.get() == Some(&path) =>
            {
                Ok((path, probe))
            }
            _ => Err(io::ErrorKind::InvalidData.into()),
        }
    }

    pub(crate) fn read_workspace_source<T>(
        &self,
        path: &Path,
        deadline: Instant,
        mut read: impl FnMut(&mut LocalConnection, &Path) -> io::Result<T>,
    ) -> io::Result<T> {
        self.with_bounded_data_connection(deadline, |connection| {
            let relative = workspace_entry(self.workspace(), path)?;
            read(connection, &relative)
        })
    }

    pub(crate) fn managed_python_source(
        &self,
        request: &rustx_execution_protocol::mcp::PythonSourceRequest,
        deadline: Instant,
    ) -> io::Result<rustx_execution_protocol::mcp::PythonSourceReply> {
        self.with_bounded_data_connection(deadline, |connection| {
            connection.python_source_by(request, deadline)
        })
    }

    pub(crate) async fn release_managed_python_source(
        self: Arc<Self>,
        key: rustx_execution_protocol::process::ProcessOperationKey,
    ) {
        use rustx_execution_protocol::mcp::{PythonSourceReply, PythonSourceRequest};

        let request = PythonSourceRequest::Release { key };
        let released = self
            .release_retained_record(move |connection, deadline| {
                connection
                    .python_source_by(&request, deadline)
                    .map(|reply| reply == PythonSourceReply::Released)
            })
            .await;
        if !released {
            self.cut_pair_if_open();
        }
    }

    pub(crate) async fn release_workspace_observation(
        self: Arc<Self>,
        key: rustx_execution_protocol::process::ProcessOperationKey,
    ) {
        use rustx_execution_protocol::changes::{ChangeReply, ChangeRequest};

        let request = ChangeRequest::Release { key };
        let released = self
            .release_retained_record(move |connection, deadline| {
                connection
                    .workspace_changes(request.clone(), deadline, || Ok(()))
                    .map(|reply| reply == ChangeReply::Accepted)
            })
            .await;
        if !released {
            self.cut_pair_if_open();
        }
    }

    // Release is idempotent on the incarnation that accepted the record.
    // An owner keeps that endpoint through transient capacity and lane loss.
    // If revoke has already closed admission, it owns the remaining cleanup.
    async fn release_retained_record(
        self: &Arc<Self>,
        dispatch: impl Fn(&mut LocalConnection, Instant) -> io::Result<bool> + Send + Sync + 'static,
    ) -> bool {
        use rustx_execution_protocol::{
            SAME_INSTANCE_RECOVERY_WINDOW, transport::recoverable_lane_loss,
        };

        let deadline = Instant::now() + SAME_INSTANCE_RECOVERY_WINDOW + PAGE_REQUEST_TIMEOUT;
        let Ok(_slot) = wait_for_execution_permit(self.data_slots.clone(), deadline).await else {
            return false;
        };
        let dispatch = Arc::new(dispatch);
        while Instant::now() < deadline {
            let released = self.connection_released.notified();
            tokio::pin!(released);
            released.as_mut().enable();
            let endpoint = self.clone();
            let dispatch = dispatch.clone();
            let result = tokio::task::spawn_blocking(move || {
                let exchange_cut = deadline.min(Instant::now() + PAGE_REQUEST_TIMEOUT);
                endpoint
                    .with_connection_slot(false, |connection| dispatch(connection, exchange_cut))
            })
            .await;
            match result {
                Ok(Ok(released)) => return released,
                Ok(Err(error))
                    if error.kind() == io::ErrorKind::WouldBlock
                        || recoverable_lane_loss(&error) =>
                {
                    if tokio::time::timeout_at(deadline.into(), released)
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                _ => break,
            }
        }
        false
    }

    pub(crate) fn prepare_managed_python_source(
        &self,
        key: rustx_execution_protocol::process::ProcessOperationKey,
        deadline: Instant,
        cancellation: &crate::runtime::CancellationSignal,
    ) -> io::Result<rustx_execution_protocol::mcp::PythonSourceReply> {
        let request = rustx_execution_protocol::mcp::PythonSourceRequest::Prepare { key };
        self.with_bounded_data_connection(deadline, |connection| {
            connection.python_source_admitted(&request, deadline, || {
                cancellation
                    .with_active(|| ())
                    .ok_or_else(|| io::ErrorKind::Interrupted.into())
            })
        })
    }

    async fn wait_for_connection<T>(
        &self,
        deadline: Instant,
        mut dispatch: impl FnMut(&mut LocalConnection) -> io::Result<T>,
    ) -> io::Result<T> {
        let limit = tokio::time::Instant::from_std(deadline);
        loop {
            let released = self.connection_released.notified();
            if Instant::now() >= deadline {
                return Err(execution_wait_timeout());
            }
            match self.with_connection_slot(false, &mut dispatch) {
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    tokio::time::timeout_at(limit, released)
                        .await
                        .map_err(|_| execution_wait_timeout())?;
                }
                result => return result,
            }
        }
    }

    /// The metadata of one entry through the admitted Client — `path`
    /// may name the workspace root itself (the exact reserved absolute
    /// path or a bare `.`), never an embedded `.`/`..`.
    ///
    /// Serializes with every other dispatch on the connection: `path`
    /// gates lexically on the reserved workspace, and the binding's
    /// live-connection check authorizes the send at dispatch time.
    ///
    /// # Errors
    ///
    /// Returns the same gating kinds as [`read_range`](Self::read_range)
    /// and the typed exchange's own errors.
    pub fn entry_metadata(&self, path: &Path) -> io::Result<rustx_confined_fs::FileMetadata> {
        self.with_connection(None, false, |connection| {
            let relative = workspace_entry(&connection.admission().workspace, path)?;
            connection.entry_metadata(&relative).map(project_metadata)
        })
    }

    /// One bounded, revision-checked directory page through the admitted
    /// Client — `path` may name the workspace root itself under the same
    /// lexical rule as [`entry_metadata`](Self::entry_metadata).
    ///
    /// # Errors
    ///
    /// Returns the same gating kinds as [`read_range`](Self::read_range)
    /// and the typed exchange's own errors.
    pub fn directory_page(
        &self,
        path: &Path,
        offset: u32,
        limit: u32,
        expected_revision: Option<&str>,
    ) -> io::Result<rustx_confined_fs::DirectoryPage> {
        self.with_connection(None, false, |connection| {
            let relative = workspace_entry(&connection.admission().workspace, path)?;
            connection
                .directory_page(&relative, offset, limit, expected_revision)
                .map(project_page)
        })
    }

    /// One bounded logical-text page through the admitted Client — the
    /// complete-line projection the Read tool owns — plus the verified
    /// source stamp. `path` resolves once against the reserved workspace
    /// at dispatch and names a file leaf, never the workspace root.
    ///
    /// Serializes with every other dispatch on the connection: `path`
    /// gates lexically on the reserved workspace, and the binding's
    /// live-connection check authorizes the send at dispatch time. The
    /// reply is trusted only after the connection's page validation; a
    /// semantic refusal — a non-UTF-8 source included — never retires a
    /// healthy connection.
    ///
    /// # Errors
    ///
    /// Returns the same gating kinds as [`read_range`](Self::read_range)
    /// plus [`io::ErrorKind::InvalidInput`] for a zero offset or limit,
    /// [`io::ErrorKind::InvalidData`] for a non-UTF-8 source or an
    /// untruthful page, and the typed exchange's own errors.
    pub fn read_lines(
        &self,
        path: &Path,
        offset: u64,
        limit: Option<u64>,
        expected_revision: Option<&str>,
    ) -> io::Result<(rustx_confined_fs::FileMetadata, rustx_confined_fs::TextPage)> {
        self.with_connection(None, false, |connection| {
            let relative = workspace_relative(&connection.admission().workspace, path)?;
            connection
                .read_lines(&relative, offset, limit, expected_revision)
                .map(project_lines)
        })
    }

    /// The conditional write through the admitted Client: `path` resolves
    /// once against the reserved workspace at dispatch, then the whole
    /// prepare–authorize–commit sequence runs on that same connection.
    ///
    /// `path` names the exact file leaf — never the workspace root. The
    /// relative path the gate produces is sent to the Client; it is never
    /// re-parsed or re-opened by the Server after approval — the physical
    /// target the Client retains under its own workspace is the only one
    /// that exists. `authorize` is invoked by the connection with the
    /// prepared original — `Some(&stamp)` for a replace, `None` for a
    /// create-only target — as the caller's last gate before the commit
    /// frame is written.
    ///
    /// # Errors
    ///
    /// Returns the same gating kinds as [`read_range`](Self::read_range)
    /// plus the write path's own: [`io::ErrorKind::Unsupported`] without
    /// the Client's write capability, the `authorize` denial verbatim,
    /// and a `rustx_execution_server::WriteOutcomeUnknown`-downcastable
    /// error once the commit outcome cannot be known.
    pub fn write_file(
        &self,
        path: &Path,
        bytes: &[u8],
        expected_revision: Option<&str>,
        authorize: impl FnOnce(Option<&FileStamp>) -> io::Result<()>,
    ) -> io::Result<rustx_confined_fs::FileMetadata> {
        self.with_connection(None, false, |connection| {
            let relative = workspace_relative(&connection.admission().workspace, path)?;
            connection
                .write_file(&relative, bytes, expected_revision, authorize)
                .map(project_stamp)
        })
    }

    pub(crate) fn mutate_tree(
        &self,
        mut mutation: TreeMutation,
        authorize: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<()> {
        self.with_connection(None, false, |connection| {
            let workspace = &connection.admission().workspace;
            match &mut mutation {
                TreeMutation::CreateDirectory { path, .. } | TreeMutation::Remove { path, .. } => {
                    *path = confined_relative(workspace, path, |byte| *byte == 0)?;
                }
                TreeMutation::Copy {
                    source,
                    destination,
                    ..
                } => {
                    *source = workspace_entry(workspace, source)?;
                    *destination = confined_relative(workspace, destination, |byte| *byte == 0)?;
                }
            }
            connection.mutate_tree(mutation, authorize)
        })
    }

    pub(crate) fn register_watch(
        &self,
        id: WatchId,
        path: &Path,
        authorize: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<(PathBuf, rustx_execution_server::ControlHandle)> {
        self.with_connection(None, false, |connection| {
            let relative = workspace_entry(&connection.admission().workspace, path)?;
            let lane = connection.control().ok_or(io::ErrorKind::NotConnected)?;
            connection.register_watch(id, &relative, authorize)?;
            Ok((relative, lane))
        })
    }

    pub(crate) fn poll_watch(
        &self,
        id: WatchId,
        path: &Path,
        lane: &rustx_execution_server::ControlHandle,
    ) -> io::Result<WatchChange> {
        self.with_connection(None, false, |connection| {
            if connection
                .control()
                .is_some_and(|current| current.same_pair(lane))
            {
                return connection.poll_watch(id, path);
            }
            // Reattachment follows physical retirement of the Client's
            // old watchers, and says nothing about what changed in the gap.
            Ok(WatchChange {
                id,
                changed_paths: Vec::new(),
                resync_required: true,
                retired: true,
            })
        })
    }

    fn revocation_confirmed(&self) -> bool {
        match &self.authority {
            EndpointAuthority::Reserved(reservation) => reservation.binding.revocation_confirmed(),
            EndpointAuthority::Delegated { .. } => false,
        }
    }

    pub(crate) fn stop_watch(
        &self,
        id: WatchId,
        lane: &rustx_execution_server::ControlHandle,
    ) -> io::Result<()> {
        if self.revocation_confirmed() {
            return Ok(());
        }
        let cutoff = Instant::now() + rustx_execution_protocol::transport::CONTROL_COMMAND_TIMEOUT;
        let result = self
            .await_cancellation_lane(cutoff)
            .ok_or(io::ErrorKind::NotConnected)
            .and_then(|control| {
                if control.same_pair(lane) {
                    control
                        .stop_watch_by(id, cutoff)
                        .map_err(|error| error.kind())
                } else {
                    Ok(())
                }
            });
        if result.is_err() && self.revocation_confirmed() {
            return Ok(());
        }
        if result.is_err()
            && let Some(current) = self.control()
        {
            if current.same_pair(lane) {
                self.cut_pair_if_open();
            } else {
                return Ok(());
            }
        }
        result.map_err(Into::into)
    }

    pub(crate) fn with_control_lane<T>(
        &self,
        lane: &rustx_execution_server::ControlHandle,
        publish: impl FnOnce() -> T,
    ) -> Option<T> {
        match &self.authority {
            EndpointAuthority::Reserved(reserved) => {
                reserved.binding.with_control_lane(lane, publish)
            }
            EndpointAuthority::Delegated { control, .. } => control.same_pair(lane).then(publish),
        }
    }

    pub(crate) fn with_live_control_lane<T>(
        &self,
        lane: &rustx_execution_server::ControlHandle,
        publish: impl FnOnce() -> T,
    ) -> Option<T> {
        match &self.authority {
            EndpointAuthority::Reserved(reserved) => {
                reserved.binding.with_live_control_lane(lane, publish)
            }
            EndpointAuthority::Delegated { control, .. } => {
                (control.same_pair(lane) && control.is_live()).then(publish)
            }
        }
    }

    /// One bounded logical-text page of a whitelisted document through
    /// the admitted Client — the same complete-line projection the Read
    /// tool owns, decoded by the shared parser policy — plus the verified
    /// source stamp. `path` resolves once against the reserved workspace
    /// at dispatch and names a file leaf, never the workspace root.
    ///
    /// The returned [`rustx_confined_fs::TextPage`]'s `original_bytes` is
    /// the decoded Markdown length — the logical source the line
    /// accounting answers to — not the compressed document's stamped
    /// size, which stays available on the returned stamp.
    ///
    /// # Errors
    ///
    /// Returns the same gating kinds as [`read_lines`](Self::read_lines):
    /// [`io::ErrorKind::Unsupported`] without the Client's document
    /// capability or for a non-whitelisted extension,
    /// [`io::ErrorKind::FileTooLarge`] for an over-cap source, and the
    /// typed decode refusals — a corrupt document or a missing text
    /// layer — as [`io::ErrorKind::InvalidData`].
    pub fn read_document(
        &self,
        path: &Path,
        offset: u64,
        limit: Option<u64>,
        expected_revision: Option<&str>,
    ) -> io::Result<(rustx_confined_fs::FileMetadata, rustx_confined_fs::TextPage)> {
        self.with_connection(None, false, |connection| {
            let relative = workspace_relative(&connection.admission().workspace, path)?;
            connection
                .read_document(&relative, offset, limit, expected_revision)
                .map(project_document)
        })
    }

    /// One admitted Agent page dispatch — `document` selects the
    /// whitelisted document decode, `false` the plain text page — for a
    /// caller that already holds the shared `data` budget and the
    /// `writer` permit. Both are retained through settlement: `data` so
    /// the in-flight accounting covers the whole exchange, `writer` so
    /// the next admitted Agent dispatch cannot interleave on the wire.
    ///
    /// The caller is a blocking worker: waiting on the connection lock
    /// is honest serialization — at most one in-flight non-agent
    /// exchange — never a queue. `deadline` is the caller's one absolute
    /// wire budget, carried unchanged into the exchange; `admit` runs
    /// after the request is encoded and the binding's live check passes,
    /// before a single request byte — its error never retires the
    /// connection; `sent` runs after the full frame is written.
    ///
    /// # Errors
    ///
    /// Returns the same gating kinds as [`read_lines`](Self::read_lines)
    /// plus the `admit` error verbatim — a cancelled caller — and
    /// [`io::ErrorKind::TimedOut`] when the deadline lapses, which
    /// retires the connection.
    #[allow(clippy::too_many_arguments)] // one admitted exchange's full context
    fn read_page_admitted(
        &self,
        path: &Path,
        offset: u64,
        limit: Option<u64>,
        document: bool,
        deadline: Instant,
        data: OwnedSemaphorePermit,
        _writer: OwnedSemaphorePermit,
        admit: impl FnOnce() -> io::Result<()>,
        sent: impl FnOnce(),
    ) -> io::Result<rustx_confined_fs::TextPage> {
        self.with_connection(Some(data), true, |connection| {
            let relative = workspace_relative(&connection.admission().workspace, path)?;
            if document {
                connection
                    .read_document_admitted(&relative, offset, limit, None, deadline, admit, sent)
                    .map(project_document)
                    .map(|(_metadata, page)| page)
            } else {
                connection
                    .read_lines_admitted(&relative, offset, limit, None, deadline, admit, sent)
                    .map(project_lines)
                    .map(|(_metadata, page)| page)
            }
        })
    }

    /// One admitted Agent search dispatch — the shared file-universe
    /// traversal rooted at `path` narrowed by `spec` — for a caller that
    /// already holds the shared `data` budget and the `writer` permit,
    /// under the same contract as
    /// [`read_page_admitted`](Self::read_page_admitted). `path` resolves
    /// through [`workspace_entry`]: the workspace root itself is a legal
    /// search root — the empty relative path the Client reads as "the
    /// root itself" — unlike the page operations, which require a leaf.
    ///
    /// # Errors
    ///
    /// Returns the same gating kinds as
    /// [`read_page_admitted`](Self::read_page_admitted).
    #[allow(clippy::too_many_arguments)] // one admitted exchange's full context
    fn search_admitted(
        &self,
        path: &Path,
        spec: &rustx_execution_protocol::SearchSpec,
        deadline: Instant,
        data: OwnedSemaphorePermit,
        _writer: OwnedSemaphorePermit,
        admit: impl FnOnce() -> io::Result<()>,
        sent: impl FnOnce(),
    ) -> io::Result<SearchResult> {
        self.with_connection(Some(data), true, |connection| {
            let relative = workspace_entry(&connection.admission().workspace, path)?;
            connection.search_admitted(&relative, spec, deadline, admit, sent)
        })
    }

    /// The bounded whole-file backing of [`SessionExecutionScope::image_bytes`]:
    /// resolves the already-interpreted absolute `path` once through the
    /// lexical workspace gate, then assembles the file from bounded
    /// `ReadRange` exchanges under the same deadline, permits, and
    /// per-request `with_active` cancellation grant as a page dispatch.
    fn image_bytes_admitted(
        &self,
        path: &Path,
        deadline: Instant,
        data: OwnedSemaphorePermit,
        _writer: OwnedSemaphorePermit,
        cancellation: &ExecutionCancellation,
    ) -> io::Result<Vec<u8>> {
        self.with_connection(Some(data), true, |connection| {
            let relative = workspace_relative(&connection.admission().workspace, path)?;
            connection
                .read_bounded_file(
                    &relative,
                    crate::tools::artifacts::ARTIFACT_TRANSFER_MAX as u64,
                    deadline,
                    || {
                        cancellation
                            .with_active(|| ())
                            .ok_or_else(cancelled_before_dispatch)
                    },
                )
                .map(|(bytes, _)| bytes)
        })
    }

    /// The admitted prepare half of one Agent write: resolves the
    /// already-interpreted absolute `path` once through the lexical
    /// workspace gate, then dispatches `PrepareWrite` — or
    /// `BeginWrite`/`WriteChunk` for an over-chunk payload — with the
    /// caller's `WriteExpectation` under the shared deadline and
    /// per-request `with_active` cancellation grant. The returned
    /// [`PreparedMutation`] holds no Server-side state: the ticket and
    /// the retained physical target live on the Client, and the caller
    /// later commits or aborts it by ticket.
    #[allow(clippy::too_many_arguments)]
    fn prepare_write_admitted(
        &self,
        path: &Path,
        expectation: &rustx_execution_protocol::WriteExpectation,
        bytes: &[u8],
        deadline: Instant,
        data: OwnedSemaphorePermit,
        _writer: OwnedSemaphorePermit,
        admit: impl Fn() -> io::Result<()>,
        sent: impl Fn(),
    ) -> io::Result<rustx_execution_server::PreparedMutation> {
        self.with_connection(Some(data), true, |connection| {
            let relative = workspace_relative(&connection.admission().workspace, path)?;
            connection.prepare_write(&relative, expectation, bytes, Some(deadline), &admit, &sent)
        })
    }

    /// The admitted prepare half of one Agent edit — the
    /// [`prepare_write_admitted`](Self::prepare_write_admitted) contract
    /// for a `PrepareEdit` whose typed refusals surface as
    /// `rustx_execution_server::RefusedEdit`-downcastable errors.
    #[allow(clippy::too_many_arguments)]
    fn prepare_edit_admitted(
        &self,
        path: &Path,
        expectation: &rustx_execution_protocol::WriteExpectation,
        replacements: &[rustx_execution_protocol::TextReplacement],
        deadline: Instant,
        data: OwnedSemaphorePermit,
        _writer: OwnedSemaphorePermit,
        admit: impl Fn() -> io::Result<()>,
        sent: impl Fn(),
    ) -> io::Result<rustx_execution_server::PreparedMutation> {
        self.with_connection(Some(data), true, |connection| {
            let relative = workspace_relative(&connection.admission().workspace, path)?;
            connection.prepare_edit(
                &relative,
                expectation,
                replacements,
                Some(deadline),
                &admit,
                &sent,
            )
        })
    }

    /// The admitted commit of one ticketed [`PreparedMutation`] — the
    /// publish crossing itself — under the caller's cancellation grant
    /// and absolute deadline. The `Written` stamp is projected to the
    /// Server-facing `FileMetadata`.
    fn commit_prepared_admitted(
        &self,
        prepared: &rustx_execution_server::PreparedMutation,
        deadline: Instant,
        data: OwnedSemaphorePermit,
        _writer: OwnedSemaphorePermit,
        admit: impl Fn() -> io::Result<()>,
        sent: impl Fn(),
    ) -> io::Result<rustx_confined_fs::FileMetadata> {
        self.with_connection(Some(data), true, |connection| {
            connection
                .commit_prepared(prepared, Some(deadline), &admit, &sent)
                .map(project_stamp)
        })
    }

    /// The admitted abort of one ticketed [`PreparedMutation`]: removes
    /// the Client-side preparation while the binding still admits the
    /// frame.
    fn abort_prepared_admitted(
        &self,
        prepared: &rustx_execution_server::PreparedMutation,
        deadline: Instant,
        data: OwnedSemaphorePermit,
        _writer: OwnedSemaphorePermit,
        admit: impl Fn() -> io::Result<()>,
        sent: impl Fn(),
    ) -> io::Result<()> {
        self.with_connection(Some(data), true, |connection| {
            connection.abort_prepared(prepared, Some(deadline), &admit, &sent)
        })
    }

    /// Serializes one synchronous exchange on the admitted connection:
    /// `WouldBlock` while another dispatch holds it or the shared data
    /// budget is spent — a bounded busy signal, never a queue — and
    /// `NotConnected` before admission. The connection stays locked
    /// across the whole dispatch, so the reply accounting of one
    /// exchange is never interleaved with the next.
    ///
    /// `data_permit` is the caller's already-held share of the
    /// [`MAX_DATA_RECORDS_PER_CONNECTION`] in-flight budget; `None` — a
    /// management dispatch — `try_acquire`s one here and still never
    /// waits. `wait` serializes the one admitted Agent dispatch that
    /// holds the writer permit: it may block on the connection lock at
    /// most one in-flight non-agent exchange rather than busy-failing.
    fn with_connection<T>(
        &self,
        data_permit: Option<OwnedSemaphorePermit>,
        wait: bool,
        dispatch: impl FnOnce(&mut LocalConnection) -> io::Result<T>,
    ) -> io::Result<T> {
        let busy = || io::Error::new(io::ErrorKind::WouldBlock, "execution connection is busy");
        let _data = match data_permit {
            Some(permit) => permit,
            None => self
                .data_slots
                .clone()
                .try_acquire_owned()
                .map_err(|_| busy())?,
        };
        self.with_connection_slot(wait, dispatch)
    }

    fn with_connection_slot<T>(
        &self,
        wait: bool,
        dispatch: impl FnOnce(&mut LocalConnection) -> io::Result<T>,
    ) -> io::Result<T> {
        let mut guard = if wait {
            self.connection
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        } else {
            match self.connection.try_lock() {
                Ok(guard) => guard,
                Err(TryLockError::WouldBlock) => {
                    return Err(io::Error::new(
                        io::ErrorKind::WouldBlock,
                        "execution connection is busy",
                    ));
                }
                Err(TryLockError::Poisoned(error)) => error.into_inner(),
            }
        };
        if !self.dispatch_open() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "execution dispatch admission is closed",
            ));
        }
        let slot_was_populated = guard.is_some();
        let Some(connection) = guard.as_mut().filter(|connection| connection.is_current()) else {
            let recovering = self.admission.get().is_some()
                && matches!(&self.authority, EndpointAuthority::Reserved(reserved)
                    if reserved.binding.may_recover() || reserved.binding.control().is_some());
            return Err(if recovering {
                // The replacement installation wakes waiters. Do not wake
                // a waiting dispatch for its own observation of this gap.
                io::Error::new(io::ErrorKind::WouldBlock, "execution pair is recovering")
            } else if slot_was_populated {
                io::Error::new(io::ErrorKind::PermissionDenied, "execution pair is retired")
            } else {
                io::Error::new(io::ErrorKind::NotConnected, "no client is admitted")
            });
        };
        let result = dispatch(connection);
        drop(guard);
        self.connection_released.notify_waiters();
        result
    }

    #[cfg(test)]
    pub(crate) fn hold_data_exchange_for_test<T>(&self, hold: impl FnOnce() -> T) -> io::Result<T> {
        self.with_connection(None, true, |_| Ok(hold()))
    }

    #[cfg(test)]
    pub(crate) fn expire_bash_capture_query_for_test(&self) {
        self.test_hooks.lock().unwrap().expire_bash_capture_query = true;
    }

    #[cfg(test)]
    pub(crate) fn take_bash_capture_expiry_for_test(&self) -> bool {
        std::mem::take(&mut self.test_hooks.lock().unwrap().expire_bash_capture_query)
    }

    /// The shared authority binding behind this endpoint — test-facing:
    /// fencing asserts the phase it drove.
    #[cfg(test)]
    fn binding(&self) -> &Binding {
        &self.reservation().binding
    }

    fn begin_client_startup(&self) -> Result<ClientStartupOwner<'_>, RuntimeManagerError> {
        let mut state = self.settlement.lock().expect("execution settlement");
        if !matches!(state.phase, Settlement::Open) {
            return Err(RuntimeManagerError::LoadingCancelled);
        }
        if !matches!(state.startup, ClientStartup::Idle) {
            return Err(error("local Client startup already owns the endpoint"));
        }
        state.startup = ClientStartup::Owned;
        Ok(ClientStartupOwner {
            endpoint: self,
            complete: false,
        })
    }

    #[cfg(test)]
    fn install_startup_hooks(&self, hooks: super::super::configuration::ConfigurationHooks) {
        self.test_hooks.lock().unwrap().local_startup = Some(hooks);
    }

    #[cfg(test)]
    fn reach_startup_hook(&self, point: &'static str) {
        let hooks = self.test_hooks.lock().unwrap().local_startup.clone();
        if let Some(hooks) = hooks {
            tokio::task::block_in_place(move || hooks.reach(point));
        }
    }

    fn dispatch_open(&self) -> bool {
        matches!(
            self.settlement.lock().expect("execution settlement").phase,
            Settlement::Open
        )
    }

    fn close_dispatch(&self) {
        let mut settlement = self.settlement.lock().expect("execution settlement");
        if matches!(settlement.phase, Settlement::Open) {
            self.reservation().binding.begin_revocation();
            settlement.phase = Settlement::Closing;
        }
        self.connection_released.notify_waiters();
    }

    fn close_dispatch_by(&self, cutoff: Instant) {
        let mut settlement = self.settlement.lock().expect("execution settlement");
        self.reservation().binding.begin_revocation_by(cutoff);
        if matches!(settlement.phase, Settlement::Open) {
            settlement.phase = Settlement::Closing;
        }
        self.settlement_changed.notify_all();
        self.connection_released.notify_waiters();
    }

    pub(crate) async fn settle(self: Arc<Self>) -> io::Result<()> {
        let deadline =
            Instant::now() + rustx_execution_protocol::transport::CONTROL_COMMAND_TIMEOUT;
        tokio::task::spawn_blocking(move || self.fence_connection_by(deadline))
            .await
            .map_err(io::Error::other)?
    }

    fn expire_drain_settlement(&self, attempt: io::Error) -> io::Result<()> {
        let mut state = self.settlement.lock().expect("execution settlement");
        if let Settlement::Finished(outcome) = &state.phase {
            return settlement_result(outcome);
        }
        let binding = &self.reservation().binding;
        let startup_settled = match state.startup {
            ClientStartup::Idle => true,
            ClientStartup::FinishedAt(finished) => binding
                .revocation_cutoff()
                .is_none_or(|cutoff| finished < cutoff),
            ClientStartup::Owned | ClientStartup::Unconfirmed(_) => false,
        };
        if startup_settled && binding.revocation_confirmed() {
            // The receipt met the cutoff. Join any owner still closing
            // the transport and releasing the durable recovery fence.
            if matches!(state.phase, Settlement::Revoking) {
                #[cfg(test)]
                self.settlement_joined.notify_one();
                state = self
                    .settlement_changed
                    .wait_while(state, |state| matches!(state.phase, Settlement::Revoking))
                    .expect("execution settlement");
            }
            if let Settlement::Finished(outcome) = &state.phase {
                return settlement_result(outcome);
            }
            state.phase = Settlement::Revoking;
            drop(state);
            return self.finish_revocation(Ok(()));
        }
        let result = Err(Arc::new(attempt));
        let outcome = settlement_result(&result);
        self.close_transport();
        state.phase = Settlement::Finished(result);
        self.settlement_changed.notify_all();
        self.connection_released.notify_waiters();
        outcome
    }

    pub(super) async fn settle_for_drain(self: Arc<Self>, deadline: Instant) -> io::Result<()> {
        self.close_dispatch_by(deadline);
        let endpoint = self.clone();
        let attempt = tokio::task::spawn_blocking(move || endpoint.fence_connection_by(deadline))
            .await
            .map_err(io::Error::other)?;
        let Err(attempt) = attempt else { return Ok(()) };
        loop {
            let changed = self.connection_released.notified();
            let cutoff = {
                let state = self.settlement.lock().expect("execution settlement");
                if let Settlement::Finished(outcome) = &state.phase {
                    return settlement_result(outcome);
                }
                match self.reservation().binding.revocation_recovery_deadline() {
                    Some(window) => deadline.min(window),
                    None if matches!(state.phase, Settlement::Revoking) => deadline,
                    None => break,
                }
            };
            if tokio::time::timeout_at(tokio::time::Instant::from_std(cutoff), changed)
                .await
                .is_err()
            {
                break;
            }
        }
        tokio::task::spawn_blocking(move || self.expire_drain_settlement(attempt))
            .await
            .map_err(io::Error::other)?
    }

    /// Own or join this incarnation's terminal settlement. The result
    /// survives deletion cancellation and remains available to Host drain.
    /// No caller holds a registry lock while waiting for this receipt.
    pub(super) fn fence_connection_by(&self, deadline: Instant) -> io::Result<()> {
        {
            let binding = &self.reservation().binding;
            let cutoff = || {
                binding
                    .revocation_cutoff()
                    .map_or(deadline, |host| deadline.min(host))
            };
            let mut state = self.settlement.lock().expect("execution settlement");
            if matches!(state.phase, Settlement::Open) {
                binding.begin_revocation();
                state.phase = Settlement::Closing;
            }
            while matches!(state.startup, ClientStartup::Owned)
                && !matches!(state.phase, Settlement::Finished(_))
            {
                let remaining = cutoff().saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "local Client startup is still settling",
                    ));
                }
                #[cfg(test)]
                self.settlement_joined.notify_one();
                state = self
                    .settlement_changed
                    .wait_timeout(state, remaining)
                    .expect("execution settlement")
                    .0;
            }
            if let Settlement::Finished(result) = &state.phase {
                return settlement_result(result);
            }
            if let ClientStartup::Unconfirmed(failure) = &state.startup {
                let result = Err(failure.clone());
                let outcome = settlement_result(&result);
                self.close_transport();
                state.phase = Settlement::Finished(result);
                self.settlement_changed.notify_all();
                return outcome;
            }
            if matches!(state.startup, ClientStartup::FinishedAt(finished) if finished >= cutoff())
            {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "local Client startup settled after the cutoff",
                ));
            }
            match &state.phase {
                Settlement::Open | Settlement::Closing => {
                    if binding.may_recover() {
                        // A live-instance credential can still identify the
                        // cleanup owner. Keep the durable fence and allow
                        // only the close path to accept its recovery.
                        state.phase = Settlement::Closing;
                        return Err(io::Error::new(
                            io::ErrorKind::NotConnected,
                            "the original instance is detached without physical settlement",
                        ));
                    }
                    state.phase = Settlement::Revoking;
                }
                Settlement::Revoking => return self.wait_for_settlement(state, deadline),
                Settlement::Finished(result) => return settlement_result(result),
            }
        }
        // Startup consumed only time from the caller's cutoff. The control
        // command retains its own shorter deadline once it can be dispatched.
        let command_deadline =
            Instant::now() + rustx_execution_protocol::transport::CONTROL_COMMAND_TIMEOUT;
        let result = self
            .reservation()
            .binding
            .settle_revocation_by(deadline.min(command_deadline));
        self.finish_revocation(result)
    }

    fn finish_revocation(&self, result: io::Result<()>) -> io::Result<()> {
        let result = result
            .and_then(|()| self.close_settled_transport())
            .map_err(Arc::new);
        let outcome = settlement_result(&result);
        let mut state = self.settlement.lock().expect("execution settlement");
        if let Settlement::Finished(existing) = &state.phase {
            return settlement_result(existing);
        }
        state.phase =
            if result.is_err() && self.reservation().binding.awaiting_revocation_recovery() {
                Settlement::Closing
            } else {
                if result.is_err() {
                    self.close_transport();
                }
                Settlement::Finished(result)
            };
        self.settlement_changed.notify_all();
        self.connection_released.notify_waiters();
        outcome
    }

    fn wait_for_settlement(
        &self,
        mut state: std::sync::MutexGuard<'_, SettlementState>,
        deadline: Instant,
    ) -> io::Result<()> {
        loop {
            if let Settlement::Finished(result) = &state.phase {
                return settlement_result(result);
            }
            if matches!(state.phase, Settlement::Closing) {
                return Err(io::Error::new(
                    io::ErrorKind::NotConnected,
                    "Client revocation requires recovery",
                ));
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Client revocation is still in progress",
                ));
            }
            #[cfg(test)]
            self.settlement_joined.notify_one();
            state = self
                .settlement_changed
                .wait_timeout(state, remaining)
                .expect("execution settlement")
                .0;
        }
    }

    fn close_settled_transport(&self) -> io::Result<()> {
        #[cfg(test)]
        {
            let gate = self
                .test_hooks
                .lock()
                .unwrap()
                .before_settlement_cleanup
                .take();
            if let Some((entered, resume)) = gate {
                let _ = entered.send(());
                let _ = resume.recv();
            }
        }
        self.close_transport();
        if let Some(fence) = &self.recovery_fence {
            self.reservation().binding.release_recovery_fence(fence)?;
        }
        Ok(())
    }

    fn close_transport(&self) {
        let binding = &self.reservation().binding;
        let fenced = binding.fence();
        self.accept_stop.cancel();
        match self.connection.try_lock() {
            Ok(mut guard) => match guard.as_mut() {
                // An idle stored connection earns the orderly Close.
                // The disconnect inside `close` is a no-op against the
                // already-fenced binding.
                Some(connection) => connection.close(),
                // Committed but not yet stored — or never stored: the
                // seized handles interrupt the physical lanes.
                None => fenced.shutdown(),
            },
            // A live exchange holds the data lock: interrupt the
            // physical lanes so the dispatch's next I/O fails and the
            // connection retires on its own. Never wait on the mutex;
            // a poisoned guard gets the same treatment — the peer is
            // interrupted either way.
            Err(TryLockError::WouldBlock | TryLockError::Poisoned(_)) => fenced.shutdown(),
        }
        self.connection_released.notify_waiters();
    }

    /// Called only by the owner of the reaped local helper. Terminal wire
    /// results and the helper's exit status supply no replacement authority.
    #[cfg(target_os = "linux")]
    fn retire_dead_local_domain(&self) -> io::Result<()> {
        let mut state = self.settlement.lock().expect("execution settlement");
        if matches!(state.phase, Settlement::Finished(Ok(()))) {
            return Ok(());
        }
        if matches!(state.phase, Settlement::Revoking) {
            let previous = self.wait_for_settlement(
                state,
                Instant::now() + rustx_execution_protocol::transport::CONTROL_COMMAND_TIMEOUT,
            );
            state = self.settlement.lock().expect("execution settlement");
            if previous.is_ok() || matches!(state.phase, Settlement::Revoking) {
                return previous;
            }
        }
        state.phase = Settlement::Revoking;
        drop(state);
        self.close_transport();
        let result = self
            .recovery_fence
            .as_ref()
            .ok_or_else(|| io::Error::other("local domain has no durable owner"))
            .and_then(|fence| self.reservation().binding.release_local_domain(fence))
            .map_err(Arc::new);
        let outcome = settlement_result(&result);
        self.settlement.lock().expect("execution settlement").phase = Settlement::Finished(result);
        self.settlement_changed.notify_all();
        outcome
    }

    /// Delivers one keyed cancellation on the control lane under one absolute
    /// `deadline` (queue wait and receipt included) and fails closed. Unless
    /// `reconciled` proves there is no operation left to cancel, an undeliverable or
    /// `Unknown` cancellation cuts the pair at once: the binding is fenced and
    /// its sockets shut without waiting on any in-flight data exchange. A
    /// delivered, known disposition keeps the pair live. A receipt is never
    /// containment — the caller still waits for the Client's settled snapshot.
    pub fn cancel_operation(
        &self,
        key: &rustx_execution_protocol::process::ProcessOperationKey,
        deadline: std::time::Instant,
        reconciled: impl Fn() -> bool,
    ) {
        if reconciled() {
            return;
        }
        while let Some(control) = self.await_cancellation_lane(deadline) {
            match control.cancel_process_by(key, deadline) {
                Ok(rustx_execution_protocol::ProcessCancelDisposition::Unknown) => break,
                Ok(_) => return,
                // Losing a cancellation receipt may leave the requested
                // operation cancelled already. A new lane can ask about the
                // exact same key, with the original deadline still in force.
                Err(error)
                    if Instant::now() < deadline
                        && rustx_execution_protocol::transport::recoverable_lane_loss(&error)
                        && self
                            .control()
                            .is_none_or(|current| !current.same_pair(&control)) =>
                {
                    if reconciled() {
                        return;
                    }
                }
                Err(_) => break,
            }
        }
        if !reconciled() {
            self.cut_pair_if_open();
        }
    }

    fn await_cancellation_lane(
        &self,
        deadline: Instant,
    ) -> Option<rustx_execution_server::ControlHandle> {
        let EndpointAuthority::Reserved(reserved) = &self.authority else {
            return self.control();
        };
        if let Some(control) = self.control() {
            return Some(control);
        }
        let runtime = tokio::runtime::Handle::try_current().ok()?;
        runtime.block_on(async {
            loop {
                let installed = self.connection_released.notified();
                if let Some(control) = self.control() {
                    return Some(control);
                }
                if !self.dispatch_open() || !reserved.binding.may_recover() {
                    return None;
                }
                tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), installed)
                    .await
                    .ok()?;
            }
        })
    }

    /// Dispose of a private Bash capture under the original binding. If
    /// capacity or an exact receipt cannot be obtained, retire the pair so
    /// the Client's binding teardown owns the outstanding capture.
    pub(crate) async fn discard_bash_capture(
        self: Arc<Self>,
        key: rustx_execution_protocol::process::ProcessOperationKey,
    ) {
        let deadline = Instant::now() + rustx_execution_protocol::transport::PAGE_REQUEST_TIMEOUT;
        let _ =
            tokio::task::spawn_blocking(move || self.discard_bash_capture_by(key, deadline)).await;
    }

    fn discard_bash_capture_by(
        &self,
        key: rustx_execution_protocol::process::ProcessOperationKey,
        deadline: Instant,
    ) {
        use rustx_execution_protocol::process::{ProcessReply, ProcessRequest};
        if !self.dispatch_open() {
            return;
        }
        let request = ProcessRequest::DiscardBashOutput { key };
        let exchange = || {
            self.with_bounded_data_connection(deadline, |connection| {
                connection.process_until(&request, deadline)
            })
        };
        let mut reply = exchange();
        // Conflict confirms the Client retained the file for this identity.
        // Keep the original deadline and never retry an uncertain exchange.
        if matches!(reply, Ok(ProcessReply::Conflict { .. })) {
            reply = exchange();
        }
        if !matches!(reply, Ok(ProcessReply::OutputReleased { .. })) {
            self.cut_pair_if_open();
        }
    }

    /// A local cleanup failure can retire an active pair. Once terminal
    /// settlement owns it, that owner needs the control lane for Revoke.
    pub(crate) fn cut_pair_if_open(&self) {
        let state = self.settlement.lock().expect("execution settlement");
        if matches!(state.phase, Settlement::Open) {
            self.cut_pair();
        }
    }

    /// A child reports successful cleanup only after its own receipts or the
    /// physical binding's terminal settlement. The decision to fence a live
    /// pair is serialized with the terminal owner's claim.
    fn finish_child_cleanup(&self, deadline: Instant) -> io::Result<()> {
        let state = self.settlement.lock().expect("execution settlement");
        if matches!(state.phase, Settlement::Open) {
            self.cut_pair();
            return Err(io::Error::other(
                "child execution cleanup remains unconfirmed",
            ));
        }
        self.wait_for_settlement(state, deadline)
    }

    #[cfg(test)]
    pub(crate) fn writer_for_test(&self) -> Arc<Semaphore> {
        self.agent_writer.clone()
    }

    #[cfg(test)]
    pub(crate) fn available_data_for_test(&self) -> usize {
        self.data_slots.available_permits()
    }

    #[cfg(test)]
    pub(crate) fn watch_prepared_cleanup_for_test(
        &self,
        ticket: rustx_execution_protocol::DecimalU64,
    ) -> tokio::sync::oneshot::Receiver<()> {
        let (send, done) = tokio::sync::oneshot::channel();
        self.test_hooks
            .lock()
            .unwrap()
            .preparation_cleanup
            .insert(ticket, send);
        done
    }

    #[cfg(test)]
    pub(crate) fn hold_cancel_mutation_fence_for_test(
        &self,
        ticket: rustx_execution_protocol::DecimalU64,
    ) -> CancelMutationFenceTest {
        let (ready, decision_ready) = tokio::sync::oneshot::channel();
        let (resume, parked) = tokio::sync::oneshot::channel();
        let (done, decision_done) = tokio::sync::oneshot::channel();
        self.test_hooks.lock().unwrap().cancel_fence.insert(
            ticket,
            CancelFenceHook {
                ready,
                resume: parked,
                done,
            },
        );
        CancelMutationFenceTest {
            decision_ready,
            resume,
            decision_done,
        }
    }

    #[cfg(test)]
    fn prepared_cleanup_completed_for_test(&self, ticket: rustx_execution_protocol::DecimalU64) {
        if let Some(done) = self
            .test_hooks
            .lock()
            .unwrap()
            .preparation_cleanup
            .remove(&ticket)
        {
            let _ = done.send(());
        }
    }

    #[cfg(test)]
    async fn pause_cancel_mutation_fence_for_test(
        &self,
        ticket: rustx_execution_protocol::DecimalU64,
        disposition: Option<rustx_execution_protocol::CancelDisposition>,
    ) -> Option<tokio::sync::oneshot::Sender<()>> {
        let gate = self.test_hooks.lock().unwrap().cancel_fence.remove(&ticket);
        let CancelFenceHook {
            ready,
            resume,
            done,
        } = gate?;
        ready.send(disposition).unwrap();
        resume.await.unwrap();
        Some(done)
    }

    /// Fences the binding and shuts its sockets without taking the data lock:
    /// a wedged exchange fails on its own read, and the pair cannot be reused.
    pub(crate) fn cut_pair(&self) {
        match &self.authority {
            EndpointAuthority::Reserved(reservation) => reservation.binding.fence().shutdown(),
            EndpointAuthority::Delegated { shutdown, .. } => shutdown.shutdown(),
        }
    }

    /// Retires the one admitted connection so its Client clears every
    /// live preparation on drop: the fail-closed answer when a cleanup
    /// abort cannot be delivered — capacity is never advertised as free
    /// while a ticket may still be stranded on the Client.
    pub fn retire_connection(&self) {
        let mut guard = self
            .connection
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(mut connection) = guard.take() {
            connection.disconnect();
        }
        drop(guard);
        self.connection_released.notify_waiters();
    }

    #[cfg(test)]
    pub(crate) fn detach_live_pair_for_test(&self) {
        let id = self
            .connection
            .lock()
            .expect("execution connection")
            .as_ref()
            .expect("live data lane")
            .connection_id()
            .clone();
        self.reservation().binding.disconnect(&id);
    }
}

impl Drop for LocalExecutionEndpoint {
    fn drop(&mut self) {
        self.accept_stop.cancel();
    }
}

/// The ticketed [`PreparedMutation`](rustx_execution_server::PreparedMutation)
/// built inside the blocking worker: it owns the validated reply *and*
/// its cleanup from the instant it exists, so a discarded result — a
/// waiter cancelled mid-dispatch, a failed handoff — never strands the
/// ticket. [`take`](OwnedPrepared::take) hands the ticket to the caller
/// and disarms; any other drop runs the abort on a short-lived detached
/// thread. When the abort cannot reach the Client — a full budget, a
/// dead connection — the guard fails closed by retiring the connection
/// so the Client's drop clears the stranded ticket, instead of
/// advertising capacity that was never cleaned.
struct OwnedPrepared {
    prepared: rustx_execution_server::PreparedMutation,
    endpoint: Arc<LocalExecutionEndpoint>,
    live: bool,
}

impl OwnedPrepared {
    fn take(mut self) -> rustx_execution_server::PreparedMutation {
        self.live = false;
        self.prepared.clone()
    }
}

impl Drop for OwnedPrepared {
    fn drop(&mut self) {
        if !self.live {
            return;
        }
        let endpoint = self.endpoint.clone();
        let prepared = self.prepared.clone();
        let deadline = Instant::now() + PAGE_REQUEST_TIMEOUT;
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            endpoint.cut_pair_if_open();
            #[cfg(test)]
            endpoint.prepared_cleanup_completed_for_test(prepared.ticket);
            return;
        };
        runtime.spawn(async move {
            if endpoint.dispatch_open()
                && endpoint
                    .cleanup_prepared(&prepared, deadline)
                    .await
                    .is_err()
            {
                endpoint.cut_pair_if_open();
            }
            #[cfg(test)]
            endpoint.prepared_cleanup_completed_for_test(prepared.ticket);
        });
    }
}

fn execution_wait_timeout() -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, "execution data request timed out")
}

async fn wait_for_execution_permit(
    slots: Arc<Semaphore>,
    deadline: Instant,
) -> io::Result<OwnedSemaphorePermit> {
    tokio::time::timeout_at(
        tokio::time::Instant::from_std(deadline),
        slots.acquire_owned(),
    )
    .await
    .map_err(|_| execution_wait_timeout())?
    .map_err(|_| io::ErrorKind::NotConnected.into())
}

impl LocalExecutionEndpoint {
    async fn cleanup_prepared(
        self: &Arc<Self>,
        prepared: &rustx_execution_server::PreparedMutation,
        deadline: Instant,
    ) -> io::Result<()> {
        let data = wait_for_execution_permit(self.data_slots.clone(), deadline).await?;
        let writer = wait_for_execution_permit(self.agent_writer.clone(), deadline).await?;
        let endpoint = self.clone();
        let prepared = prepared.clone();
        let runtime = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || {
            let (_data, _writer) = (data, writer);
            runtime.block_on(endpoint.wait_for_connection(deadline, |connection| {
                connection.abort_prepared(&prepared, Some(deadline), &|| Ok(()), &|| ())
            }))
        })
        .await
        .map_err(io::Error::other)?
    }
}

/// Resolves `path` to a workspace-relative file target, purely lexically:
/// the workspace is the exact prefix the binding reserved, never
/// canonicalized or statted. A relative path must be normal components
/// only; an absolute path must strip the reserved workspace exactly —
/// `..` or `.` anywhere (including after the prefix), the workspace
/// root itself, an empty path, and any control or NUL byte are refused.
fn workspace_relative(workspace: &Path, path: &Path) -> io::Result<PathBuf> {
    confined_relative(workspace, path, u8::is_ascii_control)
}

fn confined_relative(
    workspace: &Path,
    path: &Path,
    forbidden: fn(&u8) -> bool,
) -> io::Result<PathBuf> {
    let denied = || {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            "path is outside the Session workspace",
        )
    };
    // `components()`/`strip_prefix()` silently normalize interior `.` and
    // repeated separators, so the raw spelling is checked first: no `.`
    // or `..` segment and no byte the calling operation disallows.
    let raw = path.as_os_str().as_bytes();
    if raw.is_empty()
        || raw.iter().any(forbidden)
        || raw
            .split(|byte| *byte == b'/')
            .any(|segment| segment == b"." || segment == b"..")
    {
        return Err(denied());
    }
    let relative = if path.is_absolute() {
        path.strip_prefix(workspace).map_err(|_| denied())?
    } else {
        path
    };
    let mut has_file = false;
    for component in relative.components() {
        let Component::Normal(_) = component else {
            return Err(denied());
        };
        has_file = true;
    }
    if !has_file {
        return Err(denied());
    }
    Ok(relative.to_path_buf())
}

/// `workspace_relative` for the query operations, which may name the
/// workspace root itself: only the exact reserved absolute path or a
/// bare `.` spell the root — an embedded `.`/`..` anywhere is still
/// refused. The result is the empty relative path the Client reads as
/// "the root itself".
pub(crate) fn workspace_entry(workspace: &Path, path: &Path) -> io::Result<PathBuf> {
    if path.as_os_str() == workspace.as_os_str() || path.as_os_str().as_bytes() == b"." {
        return Ok(PathBuf::new());
    }
    confined_relative(workspace, path, |byte| *byte == 0)
}

/// Projects a verified wire `FileStamp` onto the shared product shape —
/// used by replies whose wire file type is implicitly regular.
fn project_stamp(stamp: FileStamp) -> rustx_confined_fs::FileMetadata {
    rustx_confined_fs::FileMetadata {
        is_directory: false,
        is_file: true,
        is_symlink: false,
        size: stamp.size.get(),
        revision: stamp.revision,
        identity: stamp.identity,
        modified_at_ms: stamp.modified_at_ms,
    }
}

/// Projects a validated wire stamp onto the shared product shape —
/// the kind becomes the boolean triple the wire contract carries.
fn project_metadata(entry: EntryStamp) -> rustx_confined_fs::FileMetadata {
    rustx_confined_fs::FileMetadata {
        is_directory: entry.kind == EntryKind::Directory,
        is_file: entry.kind == EntryKind::RegularFile,
        is_symlink: entry.kind == EntryKind::Symlink,
        size: entry.stamp.size.get(),
        revision: entry.stamp.revision,
        identity: entry.stamp.identity,
        modified_at_ms: entry.stamp.modified_at_ms,
    }
}

/// Projects a validated wire text page onto the shared product shape —
/// the wire `LineEnd` mirrors the scanner's [`rustx_confined_fs::PageEnd`]
/// one to one, and the stamped source size is the page's original bytes.
fn project_lines(
    page: LinesPage,
) -> (rustx_confined_fs::FileMetadata, rustx_confined_fs::TextPage) {
    let metadata = project_stamp(page.metadata);
    let page = rustx_confined_fs::TextPage {
        text: page.text,
        total_lines: page.total_lines,
        original_bytes: metadata.size,
        shown_lines: page.shown_lines,
        end: match page.end {
            rustx_execution_protocol::LineEnd::Complete => rustx_confined_fs::PageEnd::Complete,
            rustx_execution_protocol::LineEnd::RequestedLimit => {
                rustx_confined_fs::PageEnd::RequestedLimit
            }
            rustx_execution_protocol::LineEnd::LineLimit => rustx_confined_fs::PageEnd::LineLimit,
            rustx_execution_protocol::LineEnd::ByteLimit => rustx_confined_fs::PageEnd::ByteLimit,
            rustx_execution_protocol::LineEnd::FirstLineTooLarge { bytes } => {
                rustx_confined_fs::PageEnd::FirstLineTooLarge { bytes: bytes.get() }
            }
            rustx_execution_protocol::LineEnd::OffsetPastEnd => {
                rustx_confined_fs::PageEnd::OffsetPastEnd
            }
        },
    };
    (metadata, page)
}

/// Projects a validated wire document page onto the shared product
/// shape — `source` becomes the entry stamp while `logical_bytes`, the
/// decoded Markdown length, is the page's original bytes.
fn project_document(
    page: DocumentPage,
) -> (rustx_confined_fs::FileMetadata, rustx_confined_fs::TextPage) {
    let metadata = project_stamp(page.source);
    let page = rustx_confined_fs::TextPage {
        text: page.text,
        total_lines: page.total_lines,
        original_bytes: page.logical_bytes,
        shown_lines: page.shown_lines,
        end: match page.end {
            rustx_execution_protocol::LineEnd::Complete => rustx_confined_fs::PageEnd::Complete,
            rustx_execution_protocol::LineEnd::RequestedLimit => {
                rustx_confined_fs::PageEnd::RequestedLimit
            }
            rustx_execution_protocol::LineEnd::LineLimit => rustx_confined_fs::PageEnd::LineLimit,
            rustx_execution_protocol::LineEnd::ByteLimit => rustx_confined_fs::PageEnd::ByteLimit,
            rustx_execution_protocol::LineEnd::FirstLineTooLarge { bytes } => {
                rustx_confined_fs::PageEnd::FirstLineTooLarge { bytes: bytes.get() }
            }
            rustx_execution_protocol::LineEnd::OffsetPastEnd => {
                rustx_confined_fs::PageEnd::OffsetPastEnd
            }
        },
    };
    (metadata, page)
}

/// Projects a validated wire page onto the shared product shape.
fn project_page(listing: DirectoryListing) -> rustx_confined_fs::DirectoryPage {
    rustx_confined_fs::DirectoryPage {
        entries: listing
            .entries
            .into_iter()
            .map(|entry| rustx_confined_fs::DirectoryEntry {
                file_name: entry.name,
                metadata: project_metadata(entry.metadata),
            })
            .collect(),
        next_offset: listing
            .next_offset
            .map(|offset| usize::try_from(offset).expect("u32 cursor fits usize")),
        revision: listing.revision,
    }
}

impl std::fmt::Debug for LocalExecutionEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalExecutionEndpoint")
            .field("workspace", &self.workspace)
            .finish_non_exhaustive()
    }
}

/// A dispatch route to one Session's execution endpoint.
///
/// Normal runtime scopes hold the claim registry weakly. Delegation and
/// finite management work can retain one exact endpoint through settlement.
/// Holding either scope grants no admission. Dispatch still uses the
/// binding's live-connection check.
#[derive(Clone)]
pub struct SessionExecutionScope {
    session: SessionId,
    route: ExecutionRoute,
}

#[derive(Clone)]
enum ExecutionRoute {
    Registry(Weak<SessionExecutionRegistry>),
    Fixed(Arc<LocalExecutionEndpoint>),
}

enum DataCapacity {
    Immediate,
    WaitForSlot,
}

#[derive(PartialEq, Eq)]
pub(crate) struct WorkspaceDocumentSnapshot {
    directory_identity: String,
    pub(crate) document: Option<(Vec<u8>, FileStamp)>,
}

impl SessionExecutionScope {
    /// Freeze a finite management operation to its admitted generation.
    pub(crate) fn pin(&self) -> io::Result<Self> {
        Ok(Self {
            session: self.session.clone(),
            route: ExecutionRoute::Fixed(self.endpoint()?),
        })
    }

    pub(crate) fn delegated_work(
        &self,
    ) -> Option<rustx_execution_server::delegation::DelegatedWork> {
        if let ExecutionRoute::Fixed(endpoint) = &self.route
            && let EndpointAuthority::Delegated { work, .. } = &endpoint.authority
        {
            return Some(work.clone());
        }
        None
    }

    /// The Session's published endpoint, resolved at dispatch time — a
    /// lookup, never a claim. An absent, preparing, fenced, or dropped
    /// endpoint is `NotConnected`; the scope itself is never proof of
    /// admission.
    pub(crate) fn endpoint(&self) -> io::Result<Arc<LocalExecutionEndpoint>> {
        let unavailable = || {
            io::Error::new(
                io::ErrorKind::NotConnected,
                "Session execution unavailable: no admitted Execution Client",
            )
        };
        match &self.route {
            ExecutionRoute::Registry(registry) => registry
                .upgrade()
                .ok_or_else(unavailable)?
                .endpoint(&self.session)
                .map_err(|_| unavailable()),
            ExecutionRoute::Fixed(endpoint) => Ok(Arc::clone(endpoint)),
        }
    }

    pub(crate) async fn read_bash_output(
        &self,
        operation: crate::tools::native::BashOutputLocator,
        conversation: String,
        offset: u64,
        limit: Option<u64>,
        cancellation: &ExecutionCancellation,
    ) -> io::Result<rustx_confined_fs::TextPage> {
        self.inspect_bash_output(
            operation,
            move |key| rustx_execution_protocol::ClientRequest::ReadBashOutput {
                key,
                conversation,
                offset: offset.into(),
                limit: limit.map(Into::into),
            },
            cancellation,
        )
        .await
        .map(|page| match page {
            rustx_execution_server::ContinuationPage::Lines(lines) => project_lines(lines).1,
            _ => {
                unreachable!("validated Bash Read reply")
            }
        })
    }

    pub(crate) async fn inspect_bash_output(
        &self,
        operation: crate::tools::native::BashOutputLocator,
        request: impl FnOnce(
            rustx_execution_protocol::process::ProcessOperationKey,
        ) -> rustx_execution_protocol::ClientRequest
        + Send
        + 'static,
        cancellation: &ExecutionCancellation,
    ) -> io::Result<rustx_execution_server::ContinuationPage> {
        self.dispatch_data(
            cancellation,
            DataCapacity::Immediate,
            move |endpoint, data, writer, deadline, cancellation| {
                let _writer = writer;
                let key = operation.key_for(&endpoint)?;
                endpoint.with_connection(Some(data), true, |connection| {
                    connection.inspect_output(&request(key), deadline, move || {
                        cancellation
                            .with_active(|| ())
                            .ok_or_else(cancelled_before_dispatch)
                    })
                })
            },
        )
        .await
    }

    /// One bounded logical-text page of `path` through the Session's
    /// admitted Client — `document` selects the whitelisted structured
    /// document decode, `false` the plain text scan. `path` is the
    /// already-interpreted absolute target; the endpoint's lexical
    /// workspace gate resolves it once, and the verified page is returned
    /// with the source stamp discarded after validation.
    ///
    /// Admission is one shared budget: at most
    /// [`MAX_DATA_RECORDS_PER_CONNECTION`] data exchanges — Agent pages
    /// and management queries together — may be queued or in flight on
    /// the connection; a full budget is `WouldBlock`, never an unbounded
    /// queue. An admitted dispatch then waits for the single writer
    /// permit on the async side under one absolute
    /// [`PAGE_REQUEST_TIMEOUT`] deadline and biased cancellation — the
    /// wait holds no blocking-pool thread. The synchronous exchange runs
    /// on the blocking pool with both permits retained through
    /// settlement; cancellation is linearized by `with_active` inside
    /// the exchange's admit gate — after the request is encoded and the
    /// binding's live check passes, before the first request byte — and
    /// by the biased race afterwards: a cancelled post-dispatch exchange
    /// is still awaited to wire settlement (or the deadline) so the
    /// reply accounting is never abandoned.
    ///
    /// # Errors
    ///
    /// Returns `NotConnected` when the Session has no live endpoint,
    /// `WouldBlock` for an immediate request when the shared data budget is full,
    /// `io::ErrorKind::Interrupted` when cancellation wins any cut,
    /// `io::ErrorKind::TimedOut` when the absolute deadline lapses, and
    /// the endpoint's own gating and typed-exchange errors otherwise —
    /// including the `rustx_execution_server::PhysicalError`-downcastable
    /// refusal of a semantic source failure.
    pub(crate) async fn read_page(
        &self,
        path: &Path,
        offset: u64,
        limit: Option<u64>,
        document: bool,
        cancellation: &ExecutionCancellation,
    ) -> io::Result<rustx_confined_fs::TextPage> {
        let worker_path = path.to_path_buf();
        self.dispatch_data(
            cancellation,
            DataCapacity::Immediate,
            move |endpoint, data, writer, deadline, worker_cancellation| {
                #[cfg(test)]
                let admit_path = worker_path.clone();
                #[cfg(test)]
                let sent_path = worker_path.clone();
                #[cfg(test)]
                let reply_path = worker_path.clone();
                let admit_cancellation = worker_cancellation.clone();
                let admit = move || {
                    #[cfg(test)]
                    crate::tools::native::read_hooks::record_admit(&admit_path);
                    admit_cancellation
                        .with_active(|| ())
                        .ok_or_else(cancelled_before_dispatch)
                };
                let sent = move || {
                    #[cfg(test)]
                    crate::tools::native::read_hooks::record_sent(&sent_path);
                };
                let result = endpoint.read_page_admitted(
                    &worker_path,
                    offset,
                    limit,
                    document,
                    deadline,
                    data,
                    writer,
                    admit,
                    sent,
                );
                #[cfg(test)]
                if result.is_ok() {
                    crate::tools::native::read_hooks::record_reply(&reply_path);
                }
                result
            },
        )
        .await
    }

    pub(crate) async fn prepare_workspace_python(
        &self,
        cancellation: &ExecutionCancellation,
    ) -> io::Result<()> {
        self.dispatch_data(
            cancellation,
            DataCapacity::WaitForSlot,
            move |endpoint, data, _writer, deadline, cancellation| {
                endpoint.with_connection(Some(data), true, |connection| {
                    connection.prepare_workspace_python(deadline, || {
                        cancellation
                            .with_active(|| ())
                            .ok_or_else(cancelled_before_dispatch)
                    })
                })
            },
        )
        .await
    }

    #[cfg(test)]
    pub(crate) async fn workspace_python_status(
        &self,
        cancellation: &ExecutionCancellation,
    ) -> io::Result<rustx_execution_protocol::WorkspacePythonStatus> {
        self.dispatch_data(
            cancellation,
            DataCapacity::WaitForSlot,
            |endpoint, data, _writer, _deadline, _cancellation| {
                endpoint.with_connection(Some(data), true, |connection| {
                    connection.workspace_python_status()
                })
            },
        )
        .await
    }

    pub(crate) async fn workspace_changes(
        &self,
        request: rustx_execution_protocol::changes::ChangeRequest,
        cancellation: &ExecutionCancellation,
    ) -> io::Result<rustx_execution_protocol::changes::ChangeReply> {
        self.dispatch_data(
            cancellation,
            DataCapacity::WaitForSlot,
            move |endpoint, data, _writer, deadline, cancellation| {
                if endpoint.process_key(request.key().operation)? != *request.key() {
                    return Err(io::ErrorKind::PermissionDenied.into());
                }
                endpoint.with_connection(Some(data), true, |connection| {
                    connection.workspace_changes(request, deadline, || {
                        cancellation
                            .with_active(|| ())
                            .ok_or_else(cancelled_before_dispatch)
                    })
                })
            },
        )
        .await
    }

    /// The Session's bounded whole-file bytes for `path` — the physical
    /// backing of native `read_image`, assembled from at most four
    /// [`ReadRange`](rustx_execution_protocol::ClientRequest::ReadRange)
    /// exchanges on the one admitted connection under the shared
    /// [`ARTIFACT_TRANSFER_MAX`](crate::tools::artifacts::ARTIFACT_TRANSFER_MAX)
    /// budget. The stamp, size, and contradiction policy are the
    /// connection's [`LocalConnection::read_image`] contract; PNG
    /// validity itself stays an Agent-side concern — the wire carries
    /// only bytes.
    ///
    /// # Errors
    ///
    /// The [`dispatch_data`](Self::dispatch_data) kinds plus the
    /// connection's own: `FileTooLarge` for a source over the bound,
    /// typed `Stale`/`denied`/`not-found` refusals, and a retired
    /// `InvalidData` for a contradictory peer.
    pub(crate) async fn image_bytes(
        &self,
        path: &Path,
        cancellation: &ExecutionCancellation,
    ) -> io::Result<Vec<u8>> {
        let worker_path = path.to_path_buf();
        self.dispatch_data(
            cancellation,
            // An accepted image batch can exceed the eight wire slots.
            // Wait before acquiring a slot or reading any image bytes.
            DataCapacity::WaitForSlot,
            move |endpoint, data, writer, deadline, worker_cancellation| {
                endpoint.image_bytes_admitted(
                    &worker_path,
                    deadline,
                    data,
                    writer,
                    &worker_cancellation,
                )
            },
        )
        .await
    }

    /// One bounded workspace search through the Session's admitted
    /// Client — the physical backing of native `glob` and `grep`. `root`
    /// is the already-interpreted absolute search root — the workspace
    /// root itself included; `spec` is the typed wire spec with its
    /// bounds pre-validated. The shared file universe, traversal
    /// budgets, and record projection are the Client's contract; the
    /// Server owns the model-facing notices over the verified page.
    ///
    /// # Errors
    ///
    /// The [`dispatch_data`](Self::dispatch_data) kinds plus the
    /// connection's own: typed `NotFound`/`Denied`/`Conflict`/budget
    /// refusals, and a retired `InvalidData` for a contradictory peer.
    pub(crate) async fn search(
        &self,
        root: &Path,
        spec: rustx_execution_protocol::SearchSpec,
        cancellation: &ExecutionCancellation,
    ) -> io::Result<SearchResult> {
        let worker_root = root.to_path_buf();
        self.dispatch_data(
            cancellation,
            DataCapacity::Immediate,
            move |endpoint, data, writer, deadline, worker_cancellation| {
                endpoint.search_admitted(
                    &worker_root,
                    &spec,
                    deadline,
                    data,
                    writer,
                    move || {
                        worker_cancellation
                            .with_active(|| ())
                            .ok_or_else(cancelled_before_dispatch)
                    },
                    || (),
                )
            },
        )
        .await
    }

    /// The prepare half of one Agent write through the Session's
    /// admitted Client: `path` is the already-interpreted absolute
    /// target the lexical gate resolves once; `bytes` is the model's
    /// complete planned payload — over the wire chunk bound it arrives
    /// as `BeginWrite`/`WriteChunk` exchanges on the same dispatch. The
    /// Client retains the target and the bounded payload in memory;
    /// nothing is staged or published before the commit grant.
    ///
    /// `expectation` is `Current` for the model write — the
    /// pre-authorization retain, never a permission to publish.
    ///
    /// # Errors
    ///
    /// The [`dispatch_data`](Self::dispatch_data) kinds plus the
    /// connection's own: typed confinement/refusal errors and a retired
    /// `InvalidData` for a contradictory peer.
    pub(crate) async fn prepare_write(
        &self,
        path: &Path,
        bytes: Vec<u8>,
        cancellation: &ExecutionCancellation,
    ) -> io::Result<rustx_execution_server::PreparedMutation> {
        let worker_path = path.to_path_buf();
        self.dispatch_data(
            cancellation,
            DataCapacity::Immediate,
            move |endpoint, data, writer, deadline, worker_cancellation| {
                let admit = move || {
                    worker_cancellation
                        .with_active(|| ())
                        .ok_or_else(cancelled_before_dispatch)
                };
                endpoint
                    .prepare_write_admitted(
                        &worker_path,
                        &rustx_execution_protocol::WriteExpectation::Current,
                        &bytes,
                        deadline,
                        data,
                        writer,
                        admit,
                        || (),
                    )
                    .map(|prepared| OwnedPrepared {
                        prepared,
                        endpoint,
                        live: true,
                    })
            },
        )
        .await
        .map(OwnedPrepared::take)
    }

    /// The prepare half of one Agent edit — the
    /// [`prepare_write`](Self::prepare_write) contract for a
    /// `PrepareEdit` carrying the model's typed replacements. The
    /// `RefusedEdit`-downcastable refusal preserves the typed
    /// [`rustx_execution_protocol::EditRefusal`] the tool renders into
    /// its stable diagnostic.
    pub(crate) async fn prepare_edit(
        &self,
        path: &Path,
        replacements: Vec<rustx_execution_protocol::TextReplacement>,
        cancellation: &ExecutionCancellation,
    ) -> io::Result<rustx_execution_server::PreparedMutation> {
        let worker_path = path.to_path_buf();
        self.dispatch_data(
            cancellation,
            DataCapacity::Immediate,
            move |endpoint, data, writer, deadline, worker_cancellation| {
                let admit = move || {
                    worker_cancellation
                        .with_active(|| ())
                        .ok_or_else(cancelled_before_dispatch)
                };
                endpoint
                    .prepare_edit_admitted(
                        &worker_path,
                        &rustx_execution_protocol::WriteExpectation::Current,
                        &replacements,
                        deadline,
                        data,
                        writer,
                        admit,
                        || (),
                    )
                    .map(|prepared| OwnedPrepared {
                        prepared,
                        endpoint,
                        live: true,
                    })
            },
        )
        .await
        .map(OwnedPrepared::take)
    }

    /// The commit of one ticketed preparation — the publish crossing.
    ///
    /// Once the `CommitWrite` frame starts on the data wire, a listener
    /// on the invocation's cancellation forwards `CancelMutation` over
    /// the paired control lane — an independent physical path the data
    /// exchange cannot jam. A `CommitCancelled`-downcastable result is
    /// the Client's proven clean pre-publish cancel; a
    /// `WriteOutcomeUnknown`-downcastable error means the outcome
    /// cannot be known and must be reported honestly, never retried and
    /// never overridden by a racing cancellation.
    pub(crate) async fn commit_prepared(
        &self,
        prepared: &rustx_execution_server::PreparedMutation,
        cancellation: &ExecutionCancellation,
    ) -> io::Result<rustx_confined_fs::FileMetadata> {
        let prepared = prepared.clone();
        let ticket = prepared.ticket;
        // The listener arms only once the request is on the wire (`sent`
        // fires the start gate), runs on the paired control lane, and is
        // awaited to physical settlement — a racing cancellation is
        // never dropped mid-command.
        let endpoint = self.endpoint()?;
        let control = endpoint.control();
        let (started_send, mut started_recv) = tokio::sync::oneshot::channel::<()>();
        let (done_send, mut done_recv) = tokio::sync::oneshot::channel::<()>();
        let started_send = std::sync::Mutex::new(Some(started_send));
        let sent = move || {
            if let Some(send) = started_send.lock().expect("commit start gate").take() {
                let _ = send.send(());
            }
        };
        let watcher = control.map(|control| {
            let signal = cancellation.clone();
            let endpoint = endpoint.clone();
            tokio::spawn(async move {
                tokio::select! {
                    started = &mut started_recv => {
                        if started.is_err() {
                            return;
                        }
                        tokio::select! {
                            biased;
                            () = signal.cancelled() => {
                                let deadline = Instant::now() + rustx_execution_protocol::transport::CONTROL_COMMAND_TIMEOUT;
                                let reply = tokio::task::spawn_blocking(move || {
                                    control.cancel_mutation_by(ticket, deadline)
                                }).await;
                                let known = matches!(reply, Ok(Ok(disposition))
                                    if disposition != rustx_execution_protocol::CancelDisposition::Unknown);
                                #[cfg(test)]
                                let decision_done = endpoint.pause_cancel_mutation_fence_for_test(
                                    ticket, reply.as_ref().ok().and_then(|reply| reply.as_ref().ok()).copied(),
                                ).await;
                                if !known && matches!(done_recv.try_recv(), Err(tokio::sync::oneshot::error::TryRecvError::Empty)) {
                                    endpoint.cut_pair_if_open();
                                }
                                #[cfg(test)]
                                if let Some(done) = decision_done {
                                    let _ = done.send(());
                                }
                            }
                            _ = &mut done_recv => {}
                        }
                    }
                    _ = &mut done_recv => {}
                }
            })
        });
        let result = self
            .dispatch_settled(
                cancellation,
                move |endpoint, data, writer, deadline, worker_cancellation| {
                    endpoint.commit_prepared_admitted(
                        &prepared,
                        deadline,
                        data,
                        writer,
                        move || {
                            worker_cancellation
                                .with_active(|| ())
                                .ok_or_else(cancelled_before_dispatch)
                        },
                        sent,
                    )
                },
            )
            .await;
        // Settlement: releasing `done_send` disarms the listener; when
        // cancellation already fired, awaiting the watcher waits out the
        // in-flight `cancel_mutation` — its reply is the Client-side
        // arbitration, while the CommitWrite reply carried in `result`
        // is the authoritative Server-side verdict.
        drop(done_send);
        if let Some(watcher) = watcher {
            let _ = watcher.await;
        }
        result
    }

    /// The abort of one ticketed preparation — removes the Client-side
    /// preparation. The caller passes the cancellation view it has; a
    /// detached owner passes a detached one so cleanup is never itself
    /// canceled away.
    pub(crate) async fn abort_prepared(
        &self,
        prepared: &rustx_execution_server::PreparedMutation,
        cancellation: &ExecutionCancellation,
    ) -> io::Result<()> {
        let prepared = prepared.clone();
        let result = self
            .dispatch_data(
                cancellation,
                DataCapacity::WaitForSlot,
                move |endpoint, data, writer, deadline, worker_cancellation| {
                    endpoint.abort_prepared_admitted(
                        &prepared,
                        deadline,
                        data,
                        writer,
                        move || {
                            worker_cancellation
                                .with_active(|| ())
                                .ok_or_else(cancelled_before_dispatch)
                        },
                        || (),
                    )
                },
            )
            .await;
        if result.is_err()
            && let Ok(endpoint) = self.endpoint()
        {
            // The ticket may still be live on the Client: fail closed
            // rather than advertising freed capacity that was never
            // cleaned.
            endpoint.cut_pair_if_open();
        }
        result
    }

    /// One admitted data dispatch's shared admission: the endpoint's
    /// non-owning resolution, the bounded data budget, and the single
    /// writer permit awaited on the async side under one absolute
    /// [`PAGE_REQUEST_TIMEOUT`] deadline with biased cancellation — the
    /// wait holds no blocking-pool thread. The synchronous `work` runs
    /// on the blocking pool with both permits retained through
    /// settlement; cancellation is linearized by `with_active` inside
    /// `work`'s per-request admit gates — after the request is encoded
    /// and the binding's live check passes, before the first request
    /// byte — and by the biased race afterwards: a cancelled
    /// post-dispatch exchange is still awaited to wire settlement (or
    /// the deadline) so the reply accounting is never abandoned.
    ///
    /// # Errors
    ///
    /// Returns `NotConnected` when the Session has no live endpoint,
    /// `WouldBlock` while the shared data budget is full,
    /// `io::ErrorKind::Interrupted` when cancellation wins any cut,
    /// `io::ErrorKind::TimedOut` when the absolute deadline lapses, and
    /// whatever `work` returns — including the endpoint's gating and
    /// typed-exchange errors.
    async fn dispatch_data<T: Send + 'static>(
        &self,
        cancel: &ExecutionCancellation,
        capacity: DataCapacity,
        work: impl FnOnce(
            Arc<LocalExecutionEndpoint>,
            OwnedSemaphorePermit,
            OwnedSemaphorePermit,
            Instant,
            ExecutionCancellation,
        ) -> io::Result<T>
        + Send
        + 'static,
    ) -> io::Result<T> {
        if cancel.is_cancelled() {
            return Err(cancelled_before_dispatch());
        }
        let endpoint = self.endpoint()?;
        let deadline = Instant::now() + PAGE_REQUEST_TIMEOUT;
        let result = Self::dispatch_data_on(endpoint, cancel, capacity, deadline, work).await;
        if cancel.is_cancelled() {
            return Err(cancelled_before_dispatch());
        }
        result
    }

    async fn dispatch_data_on<T: Send + 'static>(
        endpoint: Arc<LocalExecutionEndpoint>,
        cancel: &ExecutionCancellation,
        capacity: DataCapacity,
        deadline: Instant,
        work: impl FnOnce(
            Arc<LocalExecutionEndpoint>,
            OwnedSemaphorePermit,
            OwnedSemaphorePermit,
            Instant,
            ExecutionCancellation,
        ) -> io::Result<T>
        + Send
        + 'static,
    ) -> io::Result<T> {
        let data = match capacity {
            DataCapacity::Immediate => {
                endpoint
                    .data_slots
                    .clone()
                    .try_acquire_owned()
                    .map_err(|_| {
                        io::Error::new(io::ErrorKind::WouldBlock, "execution connection is busy")
                    })?
            }
            DataCapacity::WaitForSlot => tokio::select! {
                biased;
                () = cancel.cancelled() => return Err(cancelled_before_dispatch()),
                permit = tokio::time::timeout_at(
                    tokio::time::Instant::from_std(deadline),
                    endpoint.data_slots.clone().acquire_owned(),
                ) => permit
                    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "execution data slot request timed out"))?
                    .map_err(|_| io::Error::other("execution data slots are closed"))?,
            },
        };
        // Only admitted work waits, and the wait never parks a blocking
        // worker: cancellation wins ties; an elapsed deadline is an
        // honest timeout, not a release.
        let writer = tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(cancelled_before_dispatch()),
            permit = tokio::time::timeout_at(
                tokio::time::Instant::from_std(deadline),
                endpoint.agent_writer.clone().acquire_owned(),
            ) => {
                permit
                    .map_err(|_| {
                        io::Error::new(
                            io::ErrorKind::TimedOut,
                            "execution connection page request timed out",
                        )
                    })?
                    .map_err(|_| io::Error::other("execution writer queue is closed"))?
            }
        };
        let worker_cancellation = cancel.clone();
        let exchange = tokio::task::spawn_blocking(move || {
            if Instant::now() >= deadline || worker_cancellation.is_cancelled() {
                return Err(if worker_cancellation.is_cancelled() {
                    cancelled_before_dispatch()
                } else {
                    io::Error::new(
                        io::ErrorKind::TimedOut,
                        "execution connection page request timed out",
                    )
                });
            }
            work(endpoint, data, writer, deadline, worker_cancellation)
        });
        // Keep the correlated reply for effectful exchanges. In particular,
        // a successful Begin still owns a candidate after cancellation.
        match exchange.await {
            Ok(result) => result,
            Err(error) => Err(io::Error::other(format!(
                "read exchange task failed: {error}"
            ))),
        }
    }
}

impl SessionExecutionScope {
    /// [`dispatch_data`](Self::dispatch_data) with one different settlement
    /// rule, used only by the mutation commit: when cancellation wins the
    /// biased race the settled worker outcome still takes precedence —
    /// a confirmed `Written` stays confirmed, a
    /// `WriteOutcomeUnknown` stays unknown, and a clean pre-wire refusal
    /// stays a refusal — a racing cancellation never masks the true
    /// outcome. `Interrupted` can still surface only from the
    /// pre-dispatch checks or the worker's own pre-byte admit gate —
    /// cases where provably nothing crossed.
    async fn dispatch_settled<T: Send + 'static>(
        &self,
        cancel: &ExecutionCancellation,
        work: impl FnOnce(
            Arc<LocalExecutionEndpoint>,
            OwnedSemaphorePermit,
            OwnedSemaphorePermit,
            Instant,
            ExecutionCancellation,
        ) -> io::Result<T>
        + Send
        + 'static,
    ) -> io::Result<T> {
        if cancel.is_cancelled() {
            return Err(cancelled_before_dispatch());
        }
        let endpoint = self.endpoint()?;
        let deadline = Instant::now() + PAGE_REQUEST_TIMEOUT;
        Self::dispatch_data_on(endpoint, cancel, DataCapacity::Immediate, deadline, work).await
    }
}

/// The admission-time cancellation error every Agent data dispatch maps
/// to the tool's cancelled result.
fn cancelled_before_dispatch() -> io::Error {
    io::Error::new(io::ErrorKind::Interrupted, "read cancelled before dispatch")
}

impl std::fmt::Debug for SessionExecutionScope {
    /// Identity only: the Session and whether the claim registry is
    /// still reachable. No credential, path, or endpoint internals are
    /// ever rendered.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionExecutionScope")
            .field("session", &self.session)
            .field(
                "owner_available",
                &match &self.route {
                    ExecutionRoute::Registry(registry) => registry.strong_count() != 0,
                    ExecutionRoute::Fixed(_) => true,
                },
            )
            .finish_non_exhaustive()
    }
}

impl PartialEq for SessionExecutionScope {
    /// Route identity: the same Session routed through the same claim
    /// registry. Two scopes built by different managers are different
    /// routes even when they name the same Session.
    fn eq(&self, other: &Self) -> bool {
        self.session == other.session
            && match (&self.route, &other.route) {
                (ExecutionRoute::Registry(left), ExecutionRoute::Registry(right)) => {
                    left.ptr_eq(right)
                }
                (ExecutionRoute::Fixed(left), ExecutionRoute::Fixed(right)) => {
                    Arc::ptr_eq(left, right)
                }
                _ => false,
            }
    }
}

impl Eq for SessionExecutionScope {}

impl Drop for ReservedExecutionEndpoint {
    /// Removes the bootstrap bundle — but only while `fstatat` through
    /// the retained parent descriptor still proves the file is the one
    /// `create_new` wrote (same type, device, and inode). A replaced,
    /// symlinked, FIFO, or identical-bytes foreign file at the path is
    /// preserved untouched, and no pathname is ever reopened.
    fn drop(&mut self) {
        self.bundle.remove();
    }
}

/// The claimable state of one Session's execution endpoint.
struct Slot {
    /// The catalog workspace captured at claim; compared exactly, never
    /// re-derived while the slot lives.
    workspace: PathBuf,
    /// The transport fixed by the first claim — a claim for the other
    /// mode is refused, never swapped.
    transport: ExecutionTransport,
    phase: Mutex<Phase>,
}

enum Phase {
    /// Claimed; the private endpoint is being prepared on a blocking task.
    Preparing,
    /// The one published endpoint; every later claim returns this value.
    Published(Arc<LocalExecutionEndpoint>),
    /// Admission is terminally closed. Keep any published endpoint for
    /// settlement until a successful durable deletion can remove it.
    Closed(Option<Arc<LocalExecutionEndpoint>>),
}

impl Slot {
    fn publish(&self, endpoint: &Arc<LocalExecutionEndpoint>) -> bool {
        let mut phase = self.phase.lock().expect("execution slot phase");
        if matches!(*phase, Phase::Preparing) {
            *phase = Phase::Published(endpoint.clone());
            return true;
        }
        false
    }

    fn close_admission(&self, cutoff: Option<Instant>) -> Option<Arc<LocalExecutionEndpoint>> {
        let mut phase = self.phase.lock().expect("execution slot phase");
        match &*phase {
            Phase::Preparing => {
                *phase = Phase::Closed(None);
                None
            }
            Phase::Published(endpoint) => {
                let endpoint = endpoint.clone();
                if let Some(cutoff) = cutoff {
                    endpoint.close_dispatch_by(cutoff);
                } else {
                    endpoint.close_dispatch();
                }
                *phase = Phase::Closed(Some(endpoint.clone()));
                Some(endpoint)
            }
            Phase::Closed(endpoint) => {
                if let (Some(endpoint), Some(cutoff)) = (endpoint, cutoff) {
                    endpoint.close_dispatch_by(cutoff);
                }
                endpoint.clone()
            }
        }
    }
}

/// The claim a caller won under the registry mutex.
enum Claim {
    /// The previously published endpoint; never replaced.
    Existing(Arc<LocalExecutionEndpoint>),
    /// The worker prepares and publishes the endpoint on this slot.
    Create(Arc<Slot>),
}

/// Runs preparation and publication inside the blocking task: a caller that
/// drops the returned handle can never strand the slot in `Preparing` —
/// the worker either publishes the endpoint or closes the claim.
fn spawn_preparation(
    slot: Arc<Slot>,
    prepare: impl FnOnce(PathBuf) -> io::Result<LocalExecutionEndpoint> + Send + 'static,
) -> tokio::task::JoinHandle<Result<Arc<LocalExecutionEndpoint>, RuntimeManagerError>> {
    tokio::task::spawn_blocking(move || {
        struct CloseOnFailure<'a>(Option<&'a Slot>);
        impl Drop for CloseOnFailure<'_> {
            fn drop(&mut self) {
                if let Some(slot) = self.0 {
                    slot.close_admission(None);
                }
            }
        }
        let mut guard = CloseOnFailure(Some(slot.as_ref()));
        let endpoint = Arc::new(prepare(slot.workspace.clone()).map_err(error)?);
        if !slot.publish(&endpoint) {
            endpoint
                .fence_connection_by(Instant::now())
                .map_err(error)?;
            return Err(RuntimeManagerError::LoadingCancelled);
        }
        guard.0 = None;
        Ok(endpoint)
    })
}

/// Per-process claim registry for local execution endpoints.
///
/// Session admission is fenced by residency state — a draining manager or a
/// Session under retirement — never by endpoint status. Endpoint drop order
/// guarantees teardown only outside locks.
#[derive(Default)]
pub(super) struct SessionExecutionRegistry {
    slots: Mutex<HashMap<SessionId, Arc<Slot>>>,
    local: local_sessions::LocalSessions,
}

/// The explicitly local Agent entry owns a single Session and its sibling Client.
pub(crate) struct StandaloneExecution {
    session: SessionId,
    registry: Arc<SessionExecutionRegistry>,
    child: tokio::sync::Mutex<SiblingClient>,
}

impl StandaloneExecution {
    pub(crate) async fn start(
        session: SessionId,
        workspace: PathBuf,
        product_root: PathBuf,
    ) -> Result<Self, String> {
        let registry = Arc::new(SessionExecutionRegistry::default());
        {
            let product_root = product_root.clone();
            let session = rustx_execution_protocol::SessionId::new(session.as_str());
            tokio::task::spawn_blocking(move || {
                rustx_execution_server::RecoveryFence::require_session_settled(
                    &product_root,
                    &session,
                )
            })
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())?;
        }
        let Claim::Create(slot) = registry
            .claim(&session, workspace, ExecutionTransport::Unix)
            .map_err(|error| error.to_string())?
        else {
            unreachable!("a new registry has no published binding")
        };
        let session_for_endpoint = session.clone();
        let endpoint = spawn_preparation(slot, move |workspace| {
            LocalExecutionEndpoint::new(&session_for_endpoint, workspace)?
                .with_recovery_fence(&product_root)
        })
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
        let child = launch_sibling(endpoint)
            .await
            .map_err(|error| error.to_string())?;
        Ok(Self {
            session,
            registry,
            child: tokio::sync::Mutex::new(child),
        })
    }

    pub(crate) fn scope(&self) -> SessionExecutionScope {
        SessionExecutionScope {
            session: self.session.clone(),
            route: ExecutionRoute::Registry(Arc::downgrade(&self.registry)),
        }
    }

    pub(crate) async fn capture_configuration(
        &self,
        manager: super::super::configuration::UserConfigManager,
        input: super::super::configuration::SessionConfigInput,
        credentials: crate::credentials::CredentialSnapshot,
    ) -> Result<super::super::configuration::AdmittedSessionConfig, String> {
        let endpoint = self
            .registry
            .endpoint(&self.session)
            .map_err(|error| error.to_string())?;
        tokio::task::spawn_blocking(move || {
            super::super::configuration::application::ApplicationState::initial_bound_session(
                &manager,
                &input,
                &credentials,
                &endpoint,
                true,
            )
        })
        .await
        .map_err(|error| error.to_string())?
    }

    pub(crate) async fn shutdown(&self) -> Result<(), String> {
        let endpoint = self.registry.close_session_admission(&self.session);
        let settlement = if let Some(endpoint) = endpoint {
            endpoint.settle().await
        } else {
            Ok(())
        };
        let mut child = self.child.lock().await;
        let status = tokio::time::timeout(
            rustx_execution_protocol::transport::LOCAL_HANDSHAKE_TIMEOUT,
            child.wait(),
        )
        .await
        .map_err(|_| "local Execution Client did not exit after Session drain".to_owned())?
        .map_err(|error| error.to_string())?;
        if status.success() {
            settlement.map_err(|error| format!("Client revocation is unconfirmed: {error}"))
        } else {
            Err(format!("local Execution Client exited with {status}"))
        }
    }
}

impl SessionExecutionRegistry {
    pub(super) fn reserves_workspace(&self, workspace: &Path) -> bool {
        self.slots
            .lock()
            .expect("execution registry mutex")
            .values()
            .any(|slot| slot.workspace.as_os_str() == workspace.as_os_str())
    }

    pub(super) fn reserved_workspaces(&self) -> BTreeSet<PathBuf> {
        self.slots
            .lock()
            .expect("execution registry mutex")
            .values()
            .map(|slot| slot.workspace.clone())
            .collect()
    }

    /// Claims the Session's execution slot under the registry mutex alone.
    /// An occupied slot must name the identical workspace *and* transport;
    /// an occupied claim is never swapped to a different path or mode — a
    /// cross-transport request for a published endpoint is refused, never
    /// a second allocation.
    fn claim(
        &self,
        session: &SessionId,
        workspace: PathBuf,
        transport: ExecutionTransport,
    ) -> Result<Claim, RuntimeManagerError> {
        self.claim_with_replacement(session, workspace, transport, None)
    }

    fn claim_with_replacement(
        &self,
        session: &SessionId,
        workspace: PathBuf,
        transport: ExecutionTransport,
        replaces: Option<&Arc<LocalExecutionEndpoint>>,
    ) -> Result<Claim, RuntimeManagerError> {
        let mut slots = self.slots.lock().expect("execution registry mutex");
        if let Some(slot) = slots.get(session) {
            if slot.workspace.as_os_str() != workspace.as_os_str() {
                return Err(error("Session workspace changed since the execution claim"));
            }
            if slot.transport != transport {
                return Err(error(
                    "Session execution transport differs from the existing claim",
                ));
            }
            let phase = slot.phase.lock().expect("execution slot phase");
            match (&*phase, replaces) {
                (Phase::Published(endpoint), Some(original)) if Arc::ptr_eq(endpoint, original) => {
                    // The process owner already retired the exact physical
                    // domain. The old endpoint remains fenced for its callers.
                }
                (Phase::Published(endpoint), None) => return Ok(Claim::Existing(endpoint.clone())),
                (Phase::Preparing, _) => {
                    return Err(error("execution binding preparation in progress"));
                }
                _ => return Err(error("execution binding is closed or has changed")),
            }
        } else if replaces.is_some() {
            return Err(error("the original execution binding no longer exists"));
        }
        // The slot is installed before any credential or socket exists, so
        // exactly one claimant ever prepares.
        let slot = Arc::new(Slot {
            workspace,
            transport,
            phase: Mutex::new(Phase::Preparing),
        });
        slots.insert(session.clone(), slot.clone());
        Ok(Claim::Create(slot))
    }

    /// The Session's published endpoint, or an explicit absence: a lookup
    /// never creates, prepares, or replaces — no credential, socket, or
    /// bootstrap material is allocated for an unclaimed Session. A
    /// `Preparing` or fenced claim is absent for dispatch purposes.
    fn endpoint(
        &self,
        session: &SessionId,
    ) -> Result<Arc<LocalExecutionEndpoint>, RuntimeManagerError> {
        self.configuration_endpoint(session)?
            .ok_or_else(|| error("Session has no execution endpoint"))
    }

    /// Preserve the distinction between an unclaimed Session and one
    /// whose claimed Workspace cannot currently answer a physical read.
    pub(super) fn configuration_endpoint(
        &self,
        session: &SessionId,
    ) -> Result<Option<Arc<LocalExecutionEndpoint>>, RuntimeManagerError> {
        let slots = self.slots.lock().expect("execution registry mutex");
        let Some(slot) = slots.get(session) else {
            return Ok(None);
        };
        let phase = slot.phase.lock().expect("execution slot phase");
        match &*phase {
            Phase::Published(endpoint) => Ok(Some(endpoint.clone())),
            Phase::Preparing => Err(error("execution binding preparation in progress")),
            Phase::Closed(_) => Err(error("execution binding is closed; recovery is required")),
        }
    }

    /// Close a Session's admission and return its settlement owner without
    /// doing IPC. A concurrent Host drain can take the same owner.
    pub(super) fn close_session_admission(
        &self,
        session: &SessionId,
    ) -> Option<Arc<LocalExecutionEndpoint>> {
        self.slots
            .lock()
            .expect("execution registry mutex")
            .get(session)?
            .close_admission(None)
    }

    pub(super) fn close_admission(&self, cutoff: Instant) -> Vec<Arc<LocalExecutionEndpoint>> {
        self.slots
            .lock()
            .expect("execution registry mutex")
            .values()
            .filter_map(|slot| slot.close_admission(Some(cutoff)))
            .collect()
    }

    /// Remove only a closed, settled claim after durable absence. A
    /// failed revocation stays in the inventory for Host drain.
    pub(super) fn retire(&self, session: &SessionId) {
        let retired = {
            let mut slots = self.slots.lock().expect("execution registry mutex");
            if slots.get(session).is_some_and(|slot| {
                let phase = slot.phase.lock().expect("execution slot phase");
                match &*phase {
                    Phase::Closed(None) => true,
                    Phase::Closed(Some(endpoint)) => matches!(
                        endpoint
                            .settlement
                            .lock()
                            .expect("execution settlement")
                            .phase,
                        Settlement::Finished(Ok(()))
                    ),
                    _ => false,
                }
            }) {
                slots.remove(session)
            } else {
                None
            }
        };
        drop(retired);
    }
}

impl SessionRuntimeManager {
    pub(crate) fn execution_user_home(&self) -> PathBuf {
        self.configuration.sources.home_directory.clone()
    }
    /// A private binding for a trusted local Workspace observation. The caller
    /// owns it through shutdown. It has no durable Session or Agent identity.
    pub(crate) async fn start_local_workspace_probe(
        &self,
        root: PathBuf,
    ) -> Result<StandaloneExecution, String> {
        StandaloneExecution::start(
            SessionId::generate(),
            root,
            self.configuration.runtime_root().to_path_buf(),
        )
        .await
    }

    /// Reserves the Session's process-owned local execution endpoint for the
    /// trusted native composer.
    ///
    /// Admission is decided under the catalog visibility cut and the
    /// residency mutex together: a draining manager or a Session under
    /// retirement never prepares a binding. At most one preparation runs —
    /// the slot is claimed synchronously before any credential, socket, or
    /// bootstrap exists — and every later claim of a published endpoint
    /// returns the identical endpoint. Local replacement is owned by the
    /// composer and requires independent settlement of that endpoint.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeManagerError::Draining`] while the manager drains and
    /// [`RuntimeManagerError::TransitionFailed`] for missing catalog
    /// metadata, retirement fencing, a concurrent preparation, a closed
    /// claim, or a failed endpoint preparation.
    ///
    /// # Panics
    ///
    /// Panics if the residency mutex is poisoned.
    pub async fn reserve_local_execution(
        &self,
        session: &SessionId,
    ) -> Result<Arc<LocalExecutionEndpoint>, RuntimeManagerError> {
        self.reserve_local_execution_for(session, None).await
    }

    pub(crate) async fn reserve_local_execution_for(
        &self,
        session: &SessionId,
        workspace: Option<&Path>,
    ) -> Result<Arc<LocalExecutionEndpoint>, RuntimeManagerError> {
        self.reserve_execution(
            session,
            ExecutionTransport::Unix,
            None,
            workspace,
            LocalExecutionEndpoint::new,
        )
        .await
    }

    #[cfg(target_os = "linux")]
    pub(crate) async fn replace_dead_local_execution(
        &self,
        session: &SessionId,
        original: Arc<LocalExecutionEndpoint>,
    ) -> Result<Arc<LocalExecutionEndpoint>, RuntimeManagerError> {
        let next = original
            .admission()
            .ok_or_else(|| error("the original local Client was never admitted"))?
            .generation
            .get()
            .checked_add(1)
            .and_then(NonZeroU64::new)
            .map(Generation::new)
            .ok_or_else(|| error("binding generation exhausted"))?;
        if !matches!(
            original
                .settlement
                .lock()
                .expect("execution settlement")
                .phase,
            Settlement::Finished(Ok(()))
        ) {
            return Err(error("the original local domain has not settled"));
        }
        self.reserve_execution(
            session,
            ExecutionTransport::Unix,
            Some(&original),
            Some(original.workspace()),
            move |session, workspace| LocalExecutionEndpoint::new_local(session, workspace, next),
        )
        .await
    }

    #[cfg(target_os = "linux")]
    pub(crate) async fn retire_dead_local_execution(
        original: Arc<LocalExecutionEndpoint>,
    ) -> Result<(), RuntimeManagerError> {
        tokio::task::spawn_blocking(move || original.retire_dead_local_domain())
            .await
            .map_err(error)?
            .map_err(error)
    }

    /// Reserves the Session's execution endpoint for the remote
    /// composition: one loopback WebSocket listener plus the `0600`
    /// bootstrap bundle the operator hands to the Client out of band.
    /// `bootstrap` must be a new path inside an existing owner-only
    /// `0700` directory; `public_url` is the optional `wss` ingress
    /// endpoint the bundle publishes instead of the loopback address.
    ///
    /// Workspace authority stays exactly the same as the local path: it
    /// is resolved from the trusted Session catalog under the same
    /// residency gates — never from remote-supplied data.
    ///
    /// # Errors
    ///
    /// Shares [`reserve_local_execution`](Self::reserve_local_execution)'s
    /// contract, plus `InvalidInput`-shaped failures for a non-loopback
    /// `listen` or an invalid `public_url`.
    pub async fn reserve_websocket_execution(
        &self,
        session: &SessionId,
        listen: std::net::SocketAddr,
        bootstrap: PathBuf,
        public_url: Option<String>,
    ) -> Result<Arc<LocalExecutionEndpoint>, RuntimeManagerError> {
        self.reserve_execution(
            session,
            ExecutionTransport::WebSocket,
            None,
            None,
            move |session, workspace| {
                LocalExecutionEndpoint::new_websocket(
                    session,
                    workspace,
                    listen,
                    bootstrap,
                    public_url.as_deref(),
                )
            },
        )
        .await
    }

    /// The shared claim-and-prepare entry every transport reservation
    /// routes through: identical residency gates, identical registry
    /// claim, identical workspace authority — only the endpoint
    /// preparation differs by transport.
    async fn reserve_execution(
        &self,
        session: &SessionId,
        transport: ExecutionTransport,
        replaces: Option<&Arc<LocalExecutionEndpoint>>,
        expected_workspace: Option<&Path>,
        prepare: impl FnOnce(&SessionId, PathBuf) -> io::Result<LocalExecutionEndpoint> + Send + 'static,
    ) -> Result<Arc<LocalExecutionEndpoint>, RuntimeManagerError> {
        let claim = self
            .claim_persisted_execution(session, transport, replaces, expected_workspace)
            .await?;
        match claim {
            Claim::Existing(endpoint) => Ok(endpoint),
            Claim::Create(slot) => {
                // The spawned preparation owns publication: a dropped
                // caller never reissues and never strands the slot.
                let session = session.clone();
                let product_root = self.configuration.runtime_root().to_path_buf();
                spawn_preparation(slot, move |workspace| {
                    prepare(&session, workspace)?.with_recovery_fence(&product_root)
                })
                .await
                .map_err(error)?
            }
        }
    }

    async fn claim_persisted_execution(
        &self,
        session: &SessionId,
        transport: ExecutionTransport,
        replaces: Option<&Arc<LocalExecutionEndpoint>>,
        expected_workspace: Option<&Path>,
    ) -> Result<Claim, RuntimeManagerError> {
        let catalog = self.sessions.catalog.lock().await;
        let (_, settings) = catalog.lineage(session, None).map_err(error)?;
        if expected_workspace.is_some_and(|path| path.as_os_str() != settings.cwd.as_os_str()) {
            return Err(error("workspace is not the Session's reserved workspace"));
        }
        // Under the same catalog visibility cut: residency admission is
        // the only authority that may fence a claim.
        let registry = self.registry.0.lock().expect("registry mutex");
        if registry.draining {
            return Err(RuntimeManagerError::Draining);
        }
        if registry.retiring_sessions.contains(session) {
            return Err(RuntimeManagerError::LoadingCancelled);
        }
        self.execution
            .claim_with_replacement(session, settings.cwd.clone(), transport, replaces)
        // Both guards release here, before any I/O or await.
    }

    /// Returns the Session's already-published local execution endpoint
    /// for the trusted native dispatch path — a lookup, never a claim.
    ///
    /// Nothing is prepared, allocated, or bound for a Session without a
    /// published endpoint: the registry answers absence explicitly, never
    /// a global default. The residency admission gate still applies — a
    /// draining manager or a Session under retirement reports no
    /// endpoint. The lookup itself is not authorization: every dispatch
    /// on the returned endpoint is gated by the binding's live-connection
    /// check at send time.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeManagerError::Draining`] while the manager drains
    /// and [`RuntimeManagerError::TransitionFailed`] for a retiring
    /// Session or when no published endpoint exists.
    ///
    /// # Panics
    ///
    /// Panics if the residency mutex is poisoned.
    pub fn execution_endpoint(
        &self,
        session: &SessionId,
    ) -> Result<Arc<LocalExecutionEndpoint>, RuntimeManagerError> {
        {
            let registry = self.registry.0.lock().expect("registry mutex");
            if registry.draining {
                return Err(RuntimeManagerError::Draining);
            }
            if registry.retiring_sessions.contains(session) {
                return Err(error("Session retirement has fenced runtime admission"));
            }
            // The residency guard releases before the registry lookup;
            // no mutex is held while the slot's phase is read.
        }
        self.execution.endpoint(session)
    }

    /// Builds the Session's non-owning execution dispatch scope for
    /// composition handoff.
    ///
    /// This allocates nothing and proves nothing: the scope is the
    /// Session identity plus a weak registry route, so it is cheap to
    /// build for every loaded Session and never extends an endpoint's
    /// life. Whether a dispatch works is decided at use time by the
    /// registry lookup and the binding's live-connection check — a scope
    /// for a Session with no admitted Client is an honest
    /// `NotConnected`, not a fallback.
    #[must_use]
    pub fn execution_scope(&self, session: &SessionId) -> SessionExecutionScope {
        SessionExecutionScope {
            session: session.clone(),
            route: ExecutionRoute::Registry(Arc::downgrade(&self.execution)),
        }
    }

    /// A restart loses its in-memory slots. Only a Session without a
    /// durable physical claim may use authored Workspace configuration.
    pub(super) fn configuration_endpoint(
        &self,
        session: &SessionId,
    ) -> Result<Option<Arc<LocalExecutionEndpoint>>, RuntimeManagerError> {
        let endpoint = self.execution.configuration_endpoint(session)?;
        if endpoint.is_none() {
            self.require_unclaimed_workspace(session)?;
        }
        Ok(endpoint)
    }

    pub(super) fn require_unclaimed_workspace(
        &self,
        session: &SessionId,
    ) -> Result<(), RuntimeManagerError> {
        if rustx_execution_server::RecoveryFence::session_pending(
            self.configuration.runtime_root(),
            &rustx_execution_protocol::SessionId::new(session.as_str()),
        )
        .map_err(error)?
        {
            return Err(error(
                "the selected Workspace has an unsettled execution binding",
            ));
        }
        Ok(())
    }

    /// Selects the authority for a read-only configuration catalog.
    /// Workspaces managed by the local composition require Client authority
    /// even between bindings. A reserved Workspace with an exact Session
    /// pins that binding's generation.
    pub(crate) fn source_catalog_scope(
        &self,
        session: Option<&SessionId>,
        workspace: Option<&Path>,
    ) -> Result<Option<SessionExecutionScope>, RuntimeManagerError> {
        {
            let registry = self.registry.0.lock().expect("registry mutex");
            if registry.draining {
                return Err(RuntimeManagerError::Draining);
            }
            if session.is_some_and(|id| registry.retiring_sessions.contains(id)) {
                return Err(error("Session retirement has fenced runtime admission"));
            }
        }
        let slots = self
            .execution
            .slots
            .lock()
            .expect("execution registry mutex");
        if let Some((session, slot)) = session.and_then(|id| slots.get(id).map(|slot| (id, slot))) {
            let phase = slot.phase.lock().expect("execution slot phase");
            if let Phase::Published(endpoint) = &*phase {
                if workspace.is_some_and(|ws| endpoint.workspace().as_os_str() != ws.as_os_str()) {
                    return Err(error(
                        "the Session execution binding belongs to another Workspace",
                    ));
                }
                return Ok(Some(SessionExecutionScope {
                    session: session.clone(),
                    route: ExecutionRoute::Fixed(endpoint.clone()),
                }));
            }
            return Err(error(
                "the selected Workspace execution binding is unavailable",
            ));
        }
        if workspace.is_some_and(|workspace| {
            slots
                .values()
                .any(|slot| slot.workspace.as_os_str() == workspace.as_os_str())
        }) {
            return Err(error("the reserved Workspace requires its exact Session"));
        }
        drop(slots);
        if workspace
            .is_some_and(|workspace| self.local_workspace_catalog_requires_client(workspace))
        {
            return Err(error(
                "the selected Workspace execution binding is unavailable",
            ));
        }
        if let Some(session) = session {
            self.require_unclaimed_workspace(session)?;
        }
        if let Some(workspace) = workspace {
            self.require_workspace_settled(workspace)?;
        }
        Ok(None)
    }

    pub(crate) fn require_workspace_settled(
        &self,
        workspace: &Path,
    ) -> Result<(), RuntimeManagerError> {
        if rustx_execution_server::RecoveryFence::workspace_pending(
            self.configuration.runtime_root(),
            workspace,
        )
        .map_err(error)?
        {
            return Err(error(
                "the selected Workspace has an unsettled execution binding",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::io::Read;
    use std::time::Duration;

    use base64::Engine as _;
    use rustx_execution_protocol::transport::{read_record, write_record};
    use rustx_execution_protocol::{
        Bootstrap, BootstrapEndpoint, ClientReply, ClientRequest, ControlHelloReply, ControlPoll,
        ControlReply, ControlRequest, ControlTurn, DataHelloReply, DecimalU64, FileStamp, Hello,
        MAX_RAW_CHUNK_BYTES, PROTOCOL_VERSION, PeerHello, ReadError,
    };
    use rustx_execution_server::BindingPhase;

    use crate::credentials::CredentialSnapshot;
    use crate::local_runtime::composition::LocalRuntimeDependencies;
    use crate::local_runtime::configuration::UserConfigManager;
    use crate::local_runtime::configuration::{SessionConfigInput, UserConfigSources};
    use crate::local_runtime::session::SessionPersistentState;
    use crate::local_runtime::session::deletion::SessionDeleteResult;
    use crate::local_runtime::session_controller::SessionController;

    use super::*;

    fn workspace() -> (tempfile::TempDir, PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().canonicalize().unwrap();
        (directory, path)
    }

    fn bundle(endpoint: &LocalExecutionEndpoint) -> Bootstrap {
        let mut file = std::fs::File::open(endpoint.bootstrap_path()).unwrap();
        read_record(&mut file).unwrap()
    }

    fn socket_path(bundle: &Bootstrap) -> &Path {
        match &bundle.endpoint {
            BootstrapEndpoint::Unix { socket_path } => socket_path,
            BootstrapEndpoint::WebSocket { .. } => panic!("a unix bundle is expected"),
        }
    }

    fn create(
        registry: &SessionExecutionRegistry,
        session: &SessionId,
        workspace: &Path,
    ) -> Arc<Slot> {
        match registry
            .claim(session, workspace.to_path_buf(), ExecutionTransport::Unix)
            .unwrap()
        {
            Claim::Create(slot) => slot,
            Claim::Existing(_) => panic!("no endpoint has been published yet"),
        }
    }

    fn prepare(slot: &Slot, session: &SessionId, workspace: &Path) -> Arc<LocalExecutionEndpoint> {
        let endpoint =
            Arc::new(LocalExecutionEndpoint::new(session, workspace.to_path_buf()).unwrap());
        assert!(slot.publish(&endpoint));
        endpoint
    }

    #[test]
    fn concurrent_claims_publish_one_endpoint_that_every_later_claim_shares() {
        let registry = SessionExecutionRegistry::default();
        let session = SessionId::generate();
        let (_dir, workspace) = workspace();
        let barrier = std::sync::Barrier::new(2);
        let outcomes: Vec<_> = std::thread::scope(|scope| {
            let mut handles = Vec::new();
            for _ in 0..2 {
                let barrier = &barrier;
                let registry = &registry;
                let session = &session;
                let workspace = &workspace;
                handles.push(scope.spawn(move || {
                    barrier.wait();
                    registry.claim(session, workspace.clone(), ExecutionTransport::Unix)
                }));
            }
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .collect()
        });
        let mut slot = None;
        for outcome in outcomes {
            match outcome {
                Ok(Claim::Create(claimed)) => assert!(slot.replace(claimed).is_none()),
                Ok(Claim::Existing(_)) => panic!("no endpoint is published yet"),
                Err(error) => assert!(
                    error.to_string().contains("preparation in progress"),
                    "{error}"
                ),
            }
        }
        let slot = slot.expect("exactly one claimant prepares");
        let endpoint = prepare(&slot, &session, &workspace);
        let published = match registry
            .claim(&session, workspace.clone(), ExecutionTransport::Unix)
            .unwrap()
        {
            Claim::Existing(endpoint) => endpoint,
            Claim::Create(_) => panic!("the published endpoint is claimed as-is"),
        };
        assert!(Arc::ptr_eq(&published, &endpoint));
        // The one bundle — including its credential — is byte-identical for
        // every claim because a Session ever publishes a single bootstrap.
        assert_eq!(
            std::fs::read(published.bootstrap_path()).unwrap(),
            std::fs::read(endpoint.bootstrap_path()).unwrap()
        );
    }

    #[test]
    fn two_sessions_sharing_one_workspace_hold_distinct_binding_material() {
        let registry = SessionExecutionRegistry::default();
        let (_dir, workspace) = workspace();
        let sessions = [SessionId::generate(), SessionId::generate()];
        let endpoints: Vec<_> = sessions
            .iter()
            .map(|session| {
                let slot = create(&registry, session, &workspace);
                prepare(&slot, session, &workspace)
            })
            .collect();
        let [a, b]: [_; 2] = endpoints.try_into().expect("two endpoints");
        let a_bundle = bundle(&a);
        let b_bundle = bundle(&b);
        assert_eq!(a_bundle.workspace, b_bundle.workspace);
        assert_ne!(a_bundle.session, b_bundle.session);
        assert_ne!(
            socket_path(&a_bundle).to_path_buf(),
            socket_path(&b_bundle).to_path_buf()
        );
        assert_ne!(a_bundle.environment, b_bundle.environment);
        assert_ne!(a_bundle.instance, b_bundle.instance);
        assert_ne!(
            std::fs::read(a.bootstrap_path()).unwrap(),
            std::fs::read(b.bootstrap_path()).unwrap(),
            "distinct credentials: bundles never repeat"
        );
        assert!(
            a_bundle.credential.as_bytes() != b_bundle.credential.as_bytes(),
            "two Sessions never share a binding credential"
        );
    }

    #[test]
    fn a_fenced_claim_rejects_the_late_publication_forever() {
        let registry = SessionExecutionRegistry::default();
        let session = SessionId::generate();
        let (_dir, workspace) = workspace();
        let slot = create(&registry, &session, &workspace);
        assert!(registry.close_session_admission(&session).is_none());
        let endpoint = Arc::new(LocalExecutionEndpoint::new(&session, workspace.clone()).unwrap());
        // The prepared endpoint is real but its binding is still Reserved;
        // publication is refused, and the stale endpoint never goes live.
        assert_eq!(endpoint.binding().status().phase, BindingPhase::Reserved);
        assert!(!slot.publish(&endpoint));
        drop(endpoint);
        match registry.claim(&session, workspace.clone(), ExecutionTransport::Unix) {
            Err(error) => assert_eq!(
                error.to_string(),
                "execution binding is closed or has changed",
            ),
            Ok(_) => panic!("a fenced claim is never reissued"),
        }
    }

    #[tokio::test]
    async fn overdue_startup_keeps_the_recovery_fence() {
        for publish_before_wait in [false, true] {
            let (_root, workspace) = workspace();
            let session = SessionId::generate();
            let endpoint = Arc::new(
                LocalExecutionEndpoint::new(&session, workspace.clone())
                    .unwrap()
                    .with_recovery_fence(&workspace)
                    .unwrap(),
            );
            let reservation = rustx_execution_protocol::SessionId::new(session.as_str());
            let pending = || {
                rustx_execution_server::RecoveryFence::session_pending(&workspace, &reservation)
                    .unwrap()
            };
            let mut starting = Some(endpoint.begin_client_startup().unwrap());
            let cutoff = Instant::now();
            endpoint.close_dispatch_by(cutoff);
            if publish_before_wait {
                starting.take().unwrap().finish(None);
                // Even a caller with a longer budget must honor the Host cut.
                assert_eq!(
                    endpoint
                        .fence_connection_by(cutoff + Duration::from_secs(60))
                        .unwrap_err()
                        .kind(),
                    io::ErrorKind::TimedOut
                );
                assert!(pending());
            }
            let failure = endpoint.clone().settle_for_drain(cutoff).await.unwrap_err();
            assert_eq!(failure.kind(), io::ErrorKind::TimedOut);
            assert!(pending());
            if let Some(starting) = starting {
                starting.finish(None);
            }
            assert_eq!(
                endpoint.clone().settle().await.unwrap_err().kind(),
                io::ErrorKind::TimedOut
            );
            assert!(pending(), "late startup completion cannot clear the marker");
        }
    }

    #[tokio::test]
    async fn a_dropped_caller_never_strands_the_claim() {
        let registry = SessionExecutionRegistry::default();
        let session = SessionId::generate();
        let (_dir, workspace) = workspace();
        let slot = create(&registry, &session, &workspace);
        // The worker parks inside preparation so the caller's disappearance
        // is provably ordered before publication.
        let (entered_tx, entered_rx) = std::sync::mpsc::channel::<()>();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let parked = session.clone();
        let handle = spawn_preparation(slot, move |path| {
            let _ = entered_tx.send(());
            let _ = release_rx.recv();
            LocalExecutionEndpoint::new(&parked, path)
        });
        // A secondary task awaits the real worker handle and forwards its
        // result to a oneshot waiter the test then drops; joining that task
        // proves the worker finished without any sleep.
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        let forward = tokio::spawn(async move {
            let _ = done_tx.send(handle.await);
        });
        entered_rx.recv().unwrap();
        drop(done_rx);
        let _ = release_tx.send(());
        forward.await.unwrap();
        // The worker published despite the dropped waiter: the registry
        // hands every later claim the identical endpoint.
        let first = match registry
            .claim(&session, workspace.clone(), ExecutionTransport::Unix)
            .unwrap()
        {
            Claim::Existing(endpoint) => endpoint,
            Claim::Create(_) => panic!("preparation published without a waiter"),
        };
        let second = match registry
            .claim(&session, workspace.clone(), ExecutionTransport::Unix)
            .unwrap()
        {
            Claim::Existing(endpoint) => endpoint,
            Claim::Create(_) => panic!("preparation published without a waiter"),
        };
        assert!(Arc::ptr_eq(&first, &second));
    }

    /// A real controller, catalog and manager: two cold Sessions recorded
    /// against one canonical workspace, durable only — never loaded.
    async fn fixture(count: usize) -> (tempfile::TempDir, SessionRuntimeManager, Vec<SessionId>) {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        std::fs::create_dir_all(home.join("rustx")).unwrap();
        let runtime = root.path().join("runtime");
        std::fs::create_dir(&runtime).unwrap();
        let controller = SessionController::open(&runtime).unwrap();
        let workspace_dir = root.path().join("workspace");
        std::fs::create_dir(&workspace_dir).unwrap();
        let workspace = workspace_dir.canonicalize().unwrap();
        let mut sessions = Vec::new();
        for _ in 0..count {
            sessions.push(
                controller
                    .create_session(SessionPersistentState::from_input(
                        &SessionConfigInput::new(workspace.clone()),
                    ))
                    .await
                    .unwrap()
                    .session
                    .id,
            );
        }
        let manager = SessionRuntimeManager::new(
            controller,
            UserConfigManager::new(UserConfigSources {
                home_directory: home.clone(),
                config_path: home.join("rustx").join("rustx.toml"),
                runtime_root: runtime,
            })
            .unwrap(),
            CredentialSnapshot::default(),
            LocalRuntimeDependencies::default(),
            super::super::RuntimeResidencyPolicy {
                max_resident_runtimes: Some(8),
                idle_grace_ms: 300_000,
            },
        )
        .unwrap();
        (root, manager, sessions)
    }

    /// A scripted peer: returns its socket after answering exactly one
    /// range request with a checked reply.
    fn answer_range(mut client: UnixStream, expected: &Path) -> UnixStream {
        let request: ClientRequest = read_record(&mut client).unwrap();
        let ClientRequest::ReadRange {
            relative_path,
            offset,
            ..
        } = request
        else {
            panic!("expected a range request");
        };
        assert_eq!(relative_path, expected);
        write_record(
            &mut client,
            &ClientReply::Range {
                data_base64: base64::engine::general_purpose::STANDARD.encode(b"ab"),
                offset,
                next_offset: None,
                metadata: FileStamp {
                    size: DecimalU64::new(2),
                    revision: "a".repeat(64),
                    identity: String::from("1:1"),
                    modified_at_ms: None,
                },
            },
        )
        .unwrap();
        client
    }

    /// A scripted metadata answer for exactly one `GetMetadata` request.
    fn answer_metadata(mut client: UnixStream, expected: &Path) -> UnixStream {
        let request: ClientRequest = read_record(&mut client).unwrap();
        let ClientRequest::GetMetadata { relative_path } = request else {
            panic!("expected a metadata request");
        };
        assert_eq!(relative_path, expected);
        write_record(
            &mut client,
            &ClientReply::Metadata {
                metadata: EntryStamp {
                    kind: EntryKind::Symlink,
                    stamp: FileStamp {
                        size: DecimalU64::new(0),
                        revision: "b".repeat(64),
                        identity: String::from("2:2"),
                        modified_at_ms: None,
                    },
                },
            },
        )
        .unwrap();
        client
    }

    /// A scripted page answer for exactly one `ReadDirectory` request.
    fn answer_directory(mut client: UnixStream, expected: &Path) -> UnixStream {
        let request: ClientRequest = read_record(&mut client).unwrap();
        let ClientRequest::ReadDirectory {
            relative_path,
            offset,
            limit,
            ..
        } = request
        else {
            panic!("expected a directory request");
        };
        assert_eq!((relative_path.as_path(), offset, limit), (expected, 0, 16));
        let stamp = |revision: char, identity: &str| EntryStamp {
            kind: EntryKind::RegularFile,
            stamp: FileStamp {
                size: DecimalU64::new(1),
                revision: revision.to_string().repeat(64),
                identity: identity.into(),
                modified_at_ms: None,
            },
        };
        write_record(
            &mut client,
            &ClientReply::Directory {
                skipped: 0,
                revision: "c".repeat(64),
                entries: vec![
                    rustx_execution_protocol::PageEntry {
                        name: String::from("a"),
                        metadata: stamp('a', "1:1"),
                    },
                    rustx_execution_protocol::PageEntry {
                        name: String::from("b"),
                        metadata: stamp('b', "1:2"),
                    },
                ],
                next_offset: None,
            },
        )
        .unwrap();
        client
    }

    /// The client-side `ControlHello` a paired test lane carries: the
    /// data Hello's exact identity plus the challenge the Server issued.
    fn control_peer_hello(
        hello: &Hello,
        challenge: rustx_execution_protocol::PairingNonce,
    ) -> PeerHello {
        PeerHello::Control(rustx_execution_protocol::ControlHello {
            version: hello.version,
            session: hello.session.clone(),
            environment: hello.environment.clone(),
            instance: hello.instance.clone(),
            generation: hello.generation,
            workspace: hello.workspace.clone(),
            credential: hello.credential.clone(),
            connection_nonce: challenge,
        })
    }

    /// The minimal control-lane loop a scripted peer runs so the paired
    /// binding stays live: each `ControlPoll` carries a fresh nonce and
    /// the receipt for the previous request; `Health` echoes, `Revoke`
    /// acknowledges, and `CancelMutation` answers `Unknown` — these
    /// scripted peers stage no local mutations. The loop ends with the
    /// lane.
    fn control_peer_loop(peer: UnixStream) {
        control_peer_loop_with(peer, |_| ());
    }

    fn control_peer_loop_with(mut peer: UnixStream, before_reply: impl Fn(&ControlRequest)) {
        let mut sequence = 1_u64;
        let mut receipt: Option<ControlReply> = None;
        loop {
            let health = DecimalU64::new(sequence);
            if write_record(
                &mut peer,
                &ControlPoll {
                    sequence: DecimalU64::new(sequence),
                    receipt: receipt.take(),
                    health,
                },
            )
            .is_err()
            {
                return;
            }
            let Ok(turn) = read_record::<ControlTurn>(&mut peer) else {
                return;
            };
            if turn.sequence != DecimalU64::new(sequence) || turn.health != health {
                return;
            }
            sequence += 1;
            let Some(request) = turn.request else {
                std::thread::sleep(Duration::from_millis(20));
                continue;
            };
            before_reply(&request);
            let done = matches!(request, ControlRequest::Revoke);
            receipt = Some(match request {
                ControlRequest::Health { nonce } => ControlReply::Healthy { nonce },
                ControlRequest::CancelMutation { ticket } => ControlReply::MutationCancelled {
                    ticket,
                    disposition: rustx_execution_protocol::CancelDisposition::Unknown,
                },
                ControlRequest::Revoke => ControlReply::Revoked,
                ControlRequest::StopWatch { id } => {
                    ControlReply::WatchStopped { id, settled: true }
                }
                ControlRequest::CancelProcess { key } => ControlReply::ProcessCancelled {
                    key,
                    disposition: rustx_execution_protocol::ProcessCancelDisposition::Unknown,
                },
            });
            if done {
                let _ = write_record(
                    &mut peer,
                    &ControlPoll {
                        sequence: DecimalU64::new(sequence),
                        receipt,
                        health: DecimalU64::new(sequence),
                    },
                );
                return;
            }
        }
    }

    /// Admits a scripted peer through the endpoint's paired typed path:
    /// the data Hello earns `Pending` and the challenge, a second
    /// socket carries `ControlHello`, and the committed pair replies
    /// `Live`; the control peer then polls on its own thread.
    fn admit(endpoint: &LocalExecutionEndpoint, client: &mut UnixStream) {
        let published = bundle(endpoint);
        let hello = Hello {
            version: PROTOCOL_VERSION,
            session: published.session.clone(),
            environment: published.environment.clone(),
            instance: published.instance.clone(),
            generation: published.generation,
            workspace: published.workspace.clone(),
            user_root: "/client/rustx".into(),
            capabilities: BTreeSet::from([
                Capability::Connection,
                Capability::ReadRange,
                Capability::DirectoryMetadata,
            ]),
            credential: published.credential.clone(),
        };
        let (accepted, _) = match endpoint.listener() {
            ExecutionListener::Unix(listener) => listener.accept().unwrap(),
            ExecutionListener::WebSocket(_) => panic!("unix endpoint expected"),
        };
        write_record(client, &PeerHello::Data(hello.clone())).unwrap();
        assert!(matches!(
            endpoint.admit_local_stream(accepted).unwrap(),
            EndpointAdmission::Pending
        ));
        let reply: DataHelloReply = read_record(client).unwrap();
        let Ok(data) = reply else {
            panic!("the scripted Hello is admitted");
        };

        // The pairing control lane: a second real connection carrying
        // the challenge — mandatory, never optional.
        let mut control = UnixStream::connect(socket_path(&bundle(endpoint))).unwrap();
        let (accepted, _) = match endpoint.listener() {
            ExecutionListener::Unix(listener) => listener.accept().unwrap(),
            ExecutionListener::WebSocket(_) => panic!("unix endpoint expected"),
        };
        write_record(&mut control, &control_peer_hello(&hello, data.challenge)).unwrap();
        assert!(matches!(
            endpoint.admit_local_stream(accepted).unwrap(),
            EndpointAdmission::Live
        ));
        let reply: ControlHelloReply = read_record(&mut control).unwrap();
        assert!(reply.is_ok(), "the honest control lane commits: {reply:?}");
        std::thread::spawn(move || control_peer_loop(control));
    }

    /// Asserts the peer socket holds no further bytes.
    fn wire_is_quiet(client: &mut UnixStream) {
        client.set_nonblocking(true).unwrap();
        let mut byte = [0u8; 1];
        assert_eq!(
            client.read(&mut byte).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        client.set_nonblocking(false).unwrap();
    }

    /// Asserts the peer socket is at EOF — only a terminal revoke shuts
    /// the physical stream; an ordinary denied dispatch stays quiet.
    fn wire_is_closed(client: &mut UnixStream) {
        client.set_nonblocking(true).unwrap();
        let mut byte = [0u8; 1];
        assert_eq!(client.read(&mut byte).unwrap(), 0);
        client.set_nonblocking(false).unwrap();
    }

    #[tokio::test]
    async fn the_endpoint_dispatches_one_bounded_read_for_its_own_session() {
        let (_root, manager, sessions) = fixture(2).await;

        // A lookup never creates: unknown and unreserved Sessions get an
        // explicit absence and no registry state is allocated.
        let unknown = SessionId::generate();
        assert!(manager.execution_endpoint(&unknown).is_err());
        assert!(manager.execution_endpoint(&sessions[0]).is_err());
        assert!(manager.execution_endpoint(&sessions[1]).is_err());
        assert!(
            manager
                .execution
                .slots
                .lock()
                .expect("execution registry mutex")
                .is_empty(),
            "a lookup allocates nothing"
        );

        let endpoint = manager.reserve_local_execution(&sessions[0]).await.unwrap();
        assert!(Arc::ptr_eq(
            &manager.execution_endpoint(&sessions[0]).unwrap(),
            &endpoint
        ));
        let published = bundle(&endpoint);
        let mut client = std::os::unix::net::UnixStream::connect(socket_path(&published)).unwrap();
        admit(&endpoint, &mut client);
        assert_eq!(endpoint.binding().status().phase, BindingPhase::Live);

        // Session B shares the exact lexical workspace but has no admitted
        // Client: dispatch is NotConnected, never a substitute.
        let endpoint_b = manager.reserve_local_execution(&sessions[1]).await.unwrap();
        assert_eq!(
            endpoint_b
                .read_range(Path::new("blob"), 0, 64, None)
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotConnected
        );

        // Session A dispatches: the scripted peer answers a checked reply.
        let answer = std::thread::spawn(move || answer_range(client, Path::new("blob")));
        let range = endpoint.read_range(Path::new("blob"), 0, 64, None).unwrap();
        assert_eq!(range.bytes, b"ab");
        assert_eq!(range.metadata.size.get(), 2);
        assert_eq!(range.metadata.revision, "a".repeat(64));
        let client = answer.join().unwrap();

        // One exchange at a time: a held connection refuses a concurrent
        // dispatch instead of queueing behind it.
        {
            let _held = endpoint.connection.lock().expect("endpoint connection");
            assert_eq!(
                endpoint
                    .read_range(Path::new("blob"), 0, 64, None)
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::WouldBlock
            );
        }

        // A caller's absolute path inside the workspace strips the exact
        // reserved prefix before dispatch.
        let answer = std::thread::spawn(move || answer_range(client, Path::new("blob")));
        let range = endpoint
            .read_range(&published.workspace.join("blob"), 0, 64, None)
            .unwrap();
        assert_eq!(range.bytes, b"ab");
        let mut client = answer.join().unwrap();

        // The root itself is a legal query target but never a file:
        // `read_range` denies it along with the empty path.
        for path in [published.workspace.clone(), PathBuf::new()] {
            assert_eq!(
                endpoint.read_range(&path, 0, 64, None).unwrap_err().kind(),
                io::ErrorKind::PermissionDenied,
                "{path:?}"
            );
        }

        // Foreign and traversal paths are refused before any request byte
        // is written.
        for path in [
            PathBuf::from("/etc/hostname"),
            PathBuf::from("../escape"),
            published.workspace.join("../escape"),
            published.workspace.join("a/../../x"),
            // `components()`/`strip_prefix()` would silently normalize
            // the interior `.` away — the raw segment must be refused.
            published.workspace.join("./blob"),
            PathBuf::from("a/./blob"),
            PathBuf::from("a\0b"),
        ] {
            assert_eq!(
                endpoint.read_range(&path, 0, 64, None).unwrap_err().kind(),
                io::ErrorKind::PermissionDenied,
                "{path:?}"
            );
        }
        wire_is_quiet(&mut client);
    }

    /// The metadata and directory dispatches share the same serialization
    /// and lexical gate; the workspace root itself is legal for them only.
    #[tokio::test]
    async fn the_endpoint_dispatches_workspace_queries_for_its_own_session() {
        let (_root, manager, sessions) = fixture(1).await;
        let endpoint = manager.reserve_local_execution(&sessions[0]).await.unwrap();
        let published = bundle(&endpoint);
        let mut client = std::os::unix::net::UnixStream::connect(socket_path(&published)).unwrap();
        admit(&endpoint, &mut client);

        // Leaf and root metadata, then a bounded page of the root.
        let answer = std::thread::spawn(move || answer_metadata(client, Path::new("link")));
        let metadata = endpoint.entry_metadata(Path::new("link")).unwrap();
        assert!(metadata.is_symlink && !metadata.is_file);
        let client = answer.join().unwrap();
        let answer = std::thread::spawn(move || answer_metadata(client, Path::new("")));
        let root_meta = endpoint.entry_metadata(&published.workspace).unwrap();
        assert!(root_meta.is_symlink && !root_meta.is_file);
        let client = answer.join().unwrap();
        let answer = std::thread::spawn(move || answer_directory(client, Path::new("")));
        let page = endpoint
            .directory_page(&published.workspace, 0, 16, None)
            .unwrap();
        assert_eq!(page.entries.len(), 2);
        assert_eq!(page.entries[0].file_name, "a");
        assert_eq!(page.next_offset, None);
        let client = answer.join().unwrap();

        // A bare `.` spells the same root only for the queries.
        let answer = std::thread::spawn(move || answer_metadata(client, Path::new("")));
        endpoint.entry_metadata(Path::new(".")).unwrap();
        let mut client = answer.join().unwrap();

        // Foreign and traversal paths are refused before any request byte
        // is written.
        for path in [
            PathBuf::from("/etc/hostname"),
            PathBuf::from("../escape"),
            published.workspace.join("../escape"),
            published.workspace.join("a/../../x"),
            published.workspace.join("./blob"),
            PathBuf::from("a/./blob"),
            PathBuf::from("a\0b"),
        ] {
            assert_eq!(
                endpoint.entry_metadata(&path).unwrap_err().kind(),
                io::ErrorKind::PermissionDenied,
                "{path:?}"
            );
            assert_eq!(
                endpoint
                    .directory_page(&path, 0, 16, None)
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::PermissionDenied,
                "{path:?}"
            );
        }
        wire_is_quiet(&mut client);
    }

    #[tokio::test]
    async fn revocation_denies_the_live_connection_and_draining_rejects_new_claims() {
        let (_root, manager, sessions) = fixture(2).await;
        let endpoint = manager.reserve_local_execution(&sessions[0]).await.unwrap();
        let published = bundle(&endpoint);
        let mut client = std::os::unix::net::UnixStream::connect(socket_path(&published)).unwrap();
        admit(&endpoint, &mut client);
        assert_eq!(endpoint.binding().status().phase, BindingPhase::Live);
        manager
            .execution
            .close_session_admission(&sessions[0])
            .unwrap()
            .settle()
            .await
            .unwrap();
        assert_eq!(endpoint.binding().status().phase, BindingPhase::Revoked);
        // The typed gate denies before the wire: the client socket carries
        // only the earlier admission reply — and then EOF, because the
        // revoke already shut the physical stream to wake an idle peer.
        let error = endpoint
            .read_range(Path::new("file"), 0, 64, None)
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        wire_is_closed(&mut client);
        // Draining permanently closes claims for Sessions that were never
        // bound — the permanent gate, not endpoint status, refuses them.
        manager.begin_draining(
            Instant::now() + rustx_execution_protocol::transport::CONTROL_COMMAND_TIMEOUT,
        );
        match manager.reserve_local_execution(&sessions[1]).await {
            Err(RuntimeManagerError::Draining) => {}
            other => panic!("draining rejects a fresh claim: {other:?}"),
        }
    }

    #[tokio::test]
    async fn the_manager_claims_cold_catalog_metadata_and_delete_revokes() {
        let (_root, manager, sessions) = fixture(2).await;
        let endpoint = manager.reserve_local_execution(&sessions[0]).await.unwrap();
        let published = bundle(&endpoint);
        assert_eq!(published.session.as_str(), sessions[0].as_str());
        assert_eq!(published.generation.get(), 1);
        let again = manager.reserve_local_execution(&sessions[0]).await.unwrap();
        assert!(Arc::ptr_eq(&again, &endpoint));
        // A stale user-supplied revision is rejected eagerly: no admission
        // fence and no endpoint state is created before it.
        let SessionDeleteResult::Stale {
            actual_revision, ..
        } = manager
            .delete_session(&sessions[0], "not-the-revision")
            .await
            .unwrap()
        else {
            panic!("a wrong revision is stale before any fencing")
        };
        assert!(Arc::ptr_eq(
            &manager.reserve_local_execution(&sessions[0]).await.unwrap(),
            &endpoint
        ));
        let SessionDeleteResult::Preview { preview } =
            manager.sessions.delete_preview(&sessions[0]).await
        else {
            panic!("the cold Session is deletable")
        };
        let result = manager
            .delete_session(&sessions[0], &preview.target_revision)
            .await
            .unwrap();
        assert!(matches!(result, SessionDeleteResult::Deleted { .. }));
        assert_eq!(preview.target_revision, actual_revision);
        assert_eq!(endpoint.binding().status().phase, BindingPhase::Revoked);
        assert!(
            manager.reserve_local_execution(&sessions[0]).await.is_err(),
            "a deleted Session can never reclaim an execution binding"
        );
        // The retired slot dropped its own endpoint reference; once the
        // test's handles are gone the private directory is gone too.
        drop(again);
        drop(endpoint);
        assert!(
            !socket_path(&published).exists(),
            "the retired endpoint's private directory was removed"
        );
    }

    /// One registry-claimed, published endpoint with a scripted peer
    /// admitted under `capabilities`: the claim registry, the Session it
    /// names, the endpoint, the peer socket, the workspace root, and the
    /// workspace directory kept alive.
    fn admitted_endpoint(
        capabilities: BTreeSet<Capability>,
    ) -> (
        Arc<SessionExecutionRegistry>,
        SessionId,
        Arc<LocalExecutionEndpoint>,
        UnixStream,
        PathBuf,
        tempfile::TempDir,
    ) {
        admitted_endpoint_with(capabilities, control_peer_loop)
    }

    fn admitted_endpoint_with(
        capabilities: BTreeSet<Capability>,
        control_loop: impl FnOnce(UnixStream) + Send + 'static,
    ) -> (
        Arc<SessionExecutionRegistry>,
        SessionId,
        Arc<LocalExecutionEndpoint>,
        UnixStream,
        PathBuf,
        tempfile::TempDir,
    ) {
        let registry = Arc::new(SessionExecutionRegistry::default());
        let session = SessionId::generate();
        let (dir, workspace) = workspace();
        let slot = create(&registry, &session, &workspace);
        let endpoint = prepare(&slot, &session, &workspace);
        let client = admit_scripted_peer(&endpoint, capabilities, control_loop);
        (registry, session, endpoint, client, workspace, dir)
    }

    fn admit_scripted_peer(
        endpoint: &LocalExecutionEndpoint,
        capabilities: BTreeSet<Capability>,
        control_loop: impl FnOnce(UnixStream) + Send + 'static,
    ) -> UnixStream {
        let published = bundle(endpoint);
        let mut client = UnixStream::connect(socket_path(&published)).unwrap();
        let hello = Hello {
            version: PROTOCOL_VERSION,
            session: published.session.clone(),
            environment: published.environment.clone(),
            instance: published.instance.clone(),
            generation: published.generation,
            workspace: published.workspace.clone(),
            user_root: "/client/rustx".into(),
            capabilities,
            credential: published.credential.clone(),
        };
        let (accepted, _) = match endpoint.listener() {
            ExecutionListener::Unix(listener) => listener.accept().unwrap(),
            ExecutionListener::WebSocket(_) => panic!("unix endpoint expected"),
        };
        write_record(&mut client, &PeerHello::Data(hello.clone())).unwrap();
        assert!(matches!(
            endpoint.admit_local_stream(accepted).unwrap(),
            EndpointAdmission::Pending
        ));
        let reply: DataHelloReply = read_record(&mut client).unwrap();
        let Ok(data) = reply else {
            panic!("the scripted Hello is admitted");
        };
        let mut control = UnixStream::connect(socket_path(&bundle(endpoint))).unwrap();
        let (accepted, _) = match endpoint.listener() {
            ExecutionListener::Unix(listener) => listener.accept().unwrap(),
            ExecutionListener::WebSocket(_) => panic!("unix endpoint expected"),
        };
        write_record(&mut control, &control_peer_hello(&hello, data.challenge)).unwrap();
        assert!(matches!(
            endpoint.admit_local_stream(accepted).unwrap(),
            EndpointAdmission::Live
        ));
        let reply: ControlHelloReply = read_record(&mut control).unwrap();
        assert!(reply.is_ok(), "the honest control lane commits: {reply:?}");
        std::thread::spawn(move || control_loop(control));
        client
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn drain_rejects_new_requests_while_the_client_withholds_revocation() {
        use crate::app_server::{
            RequestLane,
            host::{AppServerHost, HostAdmissionError},
        };
        let (_root, manager, sessions) = fixture(1).await;
        let endpoint = manager.reserve_local_execution(&sessions[0]).await.unwrap();
        let (seen_tx, seen) = tokio::sync::oneshot::channel();
        let seen_tx = std::sync::Mutex::new(Some(seen_tx));
        let (release, hold) = std::sync::mpsc::channel::<()>();
        let _client = admit_scripted_peer(
            &endpoint,
            BTreeSet::from([Capability::Connection]),
            move |control| {
                control_peer_loop_with(control, |request| {
                    if matches!(request, ControlRequest::Revoke) {
                        seen_tx.lock().unwrap().take().unwrap().send(()).unwrap();
                        let _ = hold.recv();
                    }
                });
            },
        );
        let host = AppServerHost::new(
            manager,
            crate::local_runtime::app_server_policy::AppServerPolicy::default(),
        );
        let deadline = Instant::now() + std::time::Duration::from_secs(2);
        host.begin_drain_by(deadline);
        let settlement = tokio::spawn({
            let host = host.clone();
            async move { host.settle_execution().await }
        });
        tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), seen)
            .await
            .unwrap()
            .unwrap();
        assert!(!host.admission_boundary_is_held());
        assert!(host.server_draining());
        assert_eq!(
            endpoint
                .read_range(Path::new("file"), 0, 8, None)
                .unwrap_err()
                .kind(),
            io::ErrorKind::PermissionDenied,
        );
        assert!(matches!(
            host.admit_request(RequestLane::Control, |_| panic!("draining request entered")),
            Err(HostAdmissionError::ServerDraining)
        ));
        let failures = tokio::time::timeout(std::time::Duration::from_secs(5), settlement)
            .await
            .unwrap()
            .unwrap();
        assert!(Instant::now() >= deadline);
        assert_eq!(
            failures,
            ["Client revocation is unconfirmed: control command outlived its budget"]
        );
        assert_eq!(endpoint.binding().status().phase, BindingPhase::Revoked);
        assert!(!endpoint.binding().awaiting_revocation_recovery());
        assert!(!endpoint.binding().revocation_confirmed());
        release.send(()).unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn drain_at_cutoff_joins_confirmed_revocation_before_reporting_success() {
        let (_root, workspace) = workspace();
        let session = SessionId::generate();
        let endpoint = Arc::new(
            LocalExecutionEndpoint::new(&session, workspace.clone())
                .unwrap()
                .with_recovery_fence(&workspace)
                .unwrap(),
        );
        let reservation = rustx_execution_protocol::SessionId::new(session.as_str());
        let mut data = admit_scripted_peer(
            &endpoint,
            BTreeSet::from([Capability::Connection]),
            control_peer_loop,
        );
        let (entered, confirmed) = tokio::sync::oneshot::channel();
        let (release, resume) = std::sync::mpsc::channel();
        endpoint
            .test_hooks
            .lock()
            .unwrap()
            .before_settlement_cleanup = Some((entered, resume));
        let settling = tokio::spawn(endpoint.clone().settle());
        tokio::time::timeout(Duration::from_secs(10), confirmed)
            .await
            .unwrap()
            .unwrap();
        assert!(endpoint.binding().revocation_confirmed());
        assert!(
            rustx_execution_server::RecoveryFence::session_pending(&workspace, &reservation)
                .unwrap()
        );

        let mut drain = tokio::spawn(endpoint.clone().settle_for_drain(Instant::now()));
        tokio::select! {
            result = &mut drain => panic!("drain returned {result:?} before cleanup"),
            () = endpoint.settlement_joined.notified() => (),
            () = tokio::time::sleep(Duration::from_secs(10)) => panic!("drain did not join settlement"),
        }
        release.send(()).unwrap();
        drain.await.unwrap().unwrap();
        settling.await.unwrap().unwrap();
        assert_eq!(endpoint.binding().status().phase, BindingPhase::Revoked);
        wire_is_closed(&mut data);
        rustx_execution_server::RecoveryFence::require_session_settled(&workspace, &reservation)
            .unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn drain_accepts_revoke_receipt_after_the_normal_control_frame_timeout() {
        use rustx_execution_protocol::transport::{CONTROL_COMMAND_TIMEOUT, CONTROL_FRAME_TIMEOUT};
        let (_root, manager, sessions) = fixture(1).await;
        let endpoint = manager.reserve_local_execution(&sessions[0]).await.unwrap();
        let (seen, pending) = tokio::sync::oneshot::channel();
        let seen = std::sync::Mutex::new(Some(seen));
        let (release, held) = std::sync::mpsc::channel();
        let _data_peer = admit_scripted_peer(
            &endpoint,
            BTreeSet::from([Capability::Connection]),
            move |control| {
                control_peer_loop_with(control, |request| {
                    if request == &ControlRequest::Revoke {
                        seen.lock()
                            .unwrap()
                            .take()
                            .unwrap()
                            .send(Instant::now())
                            .unwrap();
                        held.recv().unwrap();
                    }
                });
            },
        );
        let host = crate::app_server::host::AppServerHost::new(
            manager,
            crate::local_runtime::app_server_policy::AppServerPolicy::default(),
        );
        let deadline = Instant::now() + CONTROL_COMMAND_TIMEOUT;
        std::thread::spawn({
            let host = host.clone();
            move || host.begin_drain_by(deadline)
        })
        .join()
        .unwrap();
        assert!(host.server_draining());
        let settling = tokio::spawn(async move { host.settle_execution().await });
        let received_at =
            tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), pending)
                .await
                .unwrap()
                .unwrap();
        let release_at = received_at + CONTROL_FRAME_TIMEOUT + Duration::from_millis(50);
        assert!(release_at < deadline);
        // The channel proves delivery first. The timer advances only the
        // deadline interval; the peer cannot issue the receipt until released.
        tokio::time::sleep_until(tokio::time::Instant::from_std(release_at)).await;
        assert!(!settling.is_finished());
        release.send(()).unwrap();
        assert_eq!(settling.await.unwrap(), [] as [String; 0]);
        assert!(endpoint.binding().revocation_confirmed());
    }

    /// A control peer that reports each `CancelProcess` key on `seen` and
    /// answers only once `ack` releases it, with `disposition`.
    fn held_cancel_control_peer(
        mut peer: UnixStream,
        seen: &std::sync::mpsc::Sender<rustx_execution_protocol::process::ProcessOperationKey>,
        ack: &std::sync::mpsc::Receiver<()>,
        disposition: rustx_execution_protocol::ProcessCancelDisposition,
    ) {
        let mut sequence = 1_u64;
        let mut receipt: Option<ControlReply> = None;
        loop {
            let health = DecimalU64::new(sequence);
            if write_record(
                &mut peer,
                &ControlPoll {
                    sequence: DecimalU64::new(sequence),
                    receipt: receipt.take(),
                    health,
                },
            )
            .is_err()
            {
                return;
            }
            let Ok(turn) = read_record::<ControlTurn>(&mut peer) else {
                return;
            };
            if turn.sequence != DecimalU64::new(sequence) || turn.health != health {
                return;
            }
            sequence += 1;
            let Some(request) = turn.request else {
                std::thread::sleep(Duration::from_millis(5));
                continue;
            };
            receipt = Some(match request {
                ControlRequest::Health { nonce } => ControlReply::Healthy { nonce },
                ControlRequest::CancelProcess { key } => {
                    let _ = seen.send(key.clone());
                    if ack.recv_timeout(Duration::from_secs(10)).is_err() {
                        return;
                    }
                    ControlReply::ProcessCancelled { key, disposition }
                }
                ControlRequest::CancelMutation { ticket } => ControlReply::MutationCancelled {
                    ticket,
                    disposition: rustx_execution_protocol::CancelDisposition::Unknown,
                },
                ControlRequest::Revoke => ControlReply::Revoked,
                ControlRequest::StopWatch { id } => {
                    ControlReply::WatchStopped { id, settled: true }
                }
            });
        }
    }

    /// Runs a parked data exchange on a real admitted pair, then delivers the
    /// keyed cancel with `cancel_operation`. The data peer observes the request
    /// on the wire and parks; the control peer holds its acknowledgement until
    /// the test releases it. Returns the observed cancel key, the endpoint, and
    /// the data-exchange result joiner.
    #[allow(clippy::type_complexity)]
    fn parked_cancel_rig(
        disposition: rustx_execution_protocol::ProcessCancelDisposition,
    ) -> (
        Arc<LocalExecutionEndpoint>,
        rustx_execution_protocol::process::ProcessOperationKey,
        std::sync::mpsc::Receiver<rustx_execution_protocol::process::ProcessOperationKey>,
        std::sync::mpsc::Sender<()>,
        std::sync::mpsc::Sender<()>,
        std::thread::JoinHandle<io::Result<rustx_execution_protocol::process::ProcessReply>>,
        std::thread::JoinHandle<()>,
    ) {
        use rustx_execution_protocol::process::{OperationId, ProcessOperationKey, ProcessRequest};
        let (seen_tx, seen_rx) = std::sync::mpsc::channel();
        let (ack_tx, ack_rx) = std::sync::mpsc::channel();
        let (on_wire_tx, on_wire_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let (_registry, _session, endpoint, client, _workspace, _dir) = admitted_endpoint_with(
            BTreeSet::from([Capability::Connection, Capability::Process]),
            move |control| held_cancel_control_peer(control, &seen_tx, &ack_rx, disposition),
        );
        let admission = endpoint.admission().expect("admitted").clone();
        let key = ProcessOperationKey {
            incarnation: admission.server_incarnation,
            session: admission.session.clone(),
            instance: admission.instance.clone(),
            generation: admission.generation,
            operation: OperationId::new(),
        };
        // The data peer reads the request off the wire, reports it, and keeps
        // the stream open until the test releases it.
        let data_peer = std::thread::spawn(move || {
            let mut client = client;
            let _request: ClientRequest = read_record(&mut client).unwrap();
            on_wire_tx.send(()).unwrap();
            let _ = release_rx.recv();
            drop(client);
        });
        let data_endpoint = endpoint.clone();
        let request = ProcessRequest::Query { key: key.clone() };
        let exchange = std::thread::spawn(move || data_endpoint.process(&request));
        on_wire_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("the data request reaches the wire");
        (
            endpoint, key, seen_rx, ack_tx, release_tx, exchange, data_peer,
        )
    }

    /// An `Unknown` cancellation answer while a data exchange is parked on the
    /// wire fails closed without waiting on the data lock: the pair is cut,
    /// the parked exchange fails on its own, and only then does the data peer
    /// release.
    #[test]
    fn an_unknown_cancel_cuts_the_pair_while_a_data_exchange_is_parked() {
        let (endpoint, key, seen, ack, release, exchange, data_peer) =
            parked_cancel_rig(rustx_execution_protocol::ProcessCancelDisposition::Unknown);
        let deadline = Instant::now() + Duration::from_secs(10);
        let cancel = {
            let endpoint = endpoint.clone();
            let key = key.clone();
            std::thread::spawn(move || endpoint.cancel_operation(&key, deadline, || false))
        };
        let observed = seen
            .recv_timeout(Duration::from_secs(10))
            .expect("the keyed cancel reaches the control lane");
        assert_eq!(observed, key);
        assert!(!exchange.is_finished(), "the data exchange is still parked");
        ack.send(()).unwrap();
        cancel.join().unwrap();
        // Liveness watchdog only: the parked exchange must fail before the
        // data peer is released.
        let until = Instant::now() + Duration::from_secs(10);
        while !exchange.is_finished() {
            assert!(Instant::now() < until, "the parked exchange never failed");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(exchange.join().unwrap().is_err());
        assert_ne!(endpoint.binding().status().phase, BindingPhase::Live);
        release.send(()).unwrap();
        data_peer.join().unwrap();
    }

    /// A delivered, known cancellation keeps the admitted pair live: no fence,
    /// the control lane stays attached, and the parked exchange is unaffected
    /// by the cancel itself.
    #[test]
    fn a_delivered_known_cancel_keeps_the_pair_live() {
        let (endpoint, key, seen, ack, release, exchange, data_peer) = parked_cancel_rig(
            rustx_execution_protocol::ProcessCancelDisposition::CancelledBeforeStart,
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        let cancel = {
            let endpoint = endpoint.clone();
            let key = key.clone();
            std::thread::spawn(move || endpoint.cancel_operation(&key, deadline, || false))
        };
        assert_eq!(
            seen.recv_timeout(Duration::from_secs(10))
                .expect("the keyed cancel reaches the control lane"),
            key
        );
        ack.send(()).unwrap();
        cancel.join().unwrap();
        assert_eq!(endpoint.binding().status().phase, BindingPhase::Live);
        assert!(endpoint.control().is_some());
        assert!(!exchange.is_finished(), "the parked exchange is untouched");
        release.send(()).unwrap();
        data_peer.join().unwrap();
        let _ = exchange.join();
    }

    #[test]
    fn an_unknown_cancel_uses_the_terminal_evidence_observed_while_waiting() {
        use rustx_execution_protocol::process::{
            OperationId, OperationState, ProcessReply, ProcessRequest, SettledDisposition,
            SettledSnapshot,
        };
        let (seen, pending) = std::sync::mpsc::channel();
        let (ack, held) = std::sync::mpsc::channel();
        let (_registry, _session, endpoint, mut peer, _workspace, _dir) = admitted_endpoint_with(
            BTreeSet::from([Capability::Connection, Capability::Process]),
            move |control| {
                held_cancel_control_peer(
                    control,
                    &seen,
                    &held,
                    rustx_execution_protocol::ProcessCancelDisposition::Unknown,
                );
            },
        );
        let key = endpoint.process_key(OperationId::new()).unwrap();
        let reconciled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let cancellation = std::thread::spawn({
            let endpoint = endpoint.clone();
            let key = key.clone();
            let reconciled = reconciled.clone();
            move || {
                endpoint.cancel_operation(&key, Instant::now() + Duration::from_secs(5), || {
                    reconciled.load(std::sync::atomic::Ordering::Acquire)
                });
            }
        });
        assert_eq!(pending.recv_timeout(Duration::from_secs(5)).unwrap(), key);
        let reply = ProcessReply::State {
            key: key.clone(),
            state: OperationState::Settled(SettledSnapshot {
                disposition: SettledDisposition::Known,
                physical_settlement_proven: true,
                exit_code: Some(0),
                stdout_total: DecimalU64::new(0),
                stderr_total: DecimalU64::new(0),
                stdout_truncated: false,
                stderr_truncated: false,
                final_sequence: DecimalU64::new(0),
            }),
        };
        let query = ProcessRequest::Query { key };
        let (retire, retired) = std::sync::mpsc::channel::<()>();
        let data = std::thread::spawn({
            let query = query.clone();
            let reply = reply.clone();
            move || {
                assert_eq!(
                    read_record::<ClientRequest>(&mut peer).unwrap(),
                    ClientRequest::Process(query)
                );
                write_record(&mut peer, &ClientReply::Process(reply)).unwrap();
                retired.recv_timeout(Duration::from_secs(5)).unwrap();
            }
        });
        assert_eq!(endpoint.process(&query).unwrap(), reply);
        reconciled.store(true, std::sync::atomic::Ordering::Release);
        ack.send(()).unwrap();
        cancellation.join().unwrap();
        assert_eq!(endpoint.binding().status().phase, BindingPhase::Live);
        endpoint.control().unwrap().health().unwrap();
        retire.send(()).unwrap();
        data.join().unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_capture_discard_that_cannot_be_sent_retires_the_owning_binding() {
        use rustx_execution_protocol::process::OperationId;

        let (_registry, _session, endpoint, mut data, _workspace, _directory) =
            admitted_endpoint_with(
                BTreeSet::from([Capability::Connection, Capability::Process]),
                control_peer_loop,
            );
        let (held, waiting) = tokio::sync::oneshot::channel();
        let (release, released) = std::sync::mpsc::channel();
        let contending_data = tokio::task::spawn_blocking({
            let endpoint = endpoint.clone();
            move || {
                endpoint.hold_data_exchange_for_test(|| {
                    held.send(()).unwrap();
                    released.recv().unwrap();
                })
            }
        });
        waiting.await.unwrap();
        let key = endpoint.process_key(OperationId::new()).unwrap();
        tokio::task::spawn_blocking({
            let endpoint = endpoint.clone();
            move || endpoint.discard_bash_capture_by(key, Instant::now())
        })
        .await
        .unwrap();
        assert_eq!(endpoint.binding().status().phase, BindingPhase::Revoked);
        data.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        assert_eq!(
            tokio::task::spawn_blocking(move || data.read(&mut [0]))
                .await
                .unwrap()
                .unwrap(),
            0
        );
        release.send(()).unwrap();
        contending_data.await.unwrap().unwrap();
    }

    /// A busy exchange parks on the reply read holding the connection's
    /// data lock; the fence must still complete — atomically and without
    /// ever waiting on that lock — by interrupting the physical stream,
    /// and the revoked binding must refuse every later admission without
    /// the peer ever cooperating. `PAGE_REQUEST_TIMEOUT` is 60 s: every
    /// completion below is awaited on a 10 s channel bound, comfortably
    /// proving the fence did not wait out the pending request.
    #[test]
    fn a_busy_peer_cannot_stall_the_fence_and_the_binding_stays_revoked() {
        let registry = SessionExecutionRegistry::default();
        let session = SessionId::generate();
        let (dir, workspace) = workspace();
        let slot = create(&registry, &session, &workspace);
        let endpoint = prepare(&slot, &session, &workspace);
        let mut client = UnixStream::connect(socket_path(&bundle(&endpoint))).unwrap();
        admit(&endpoint, &mut client);

        // Park a real dispatch on the reply read: the peer takes the
        // request off the wire — proving the exchange is in flight and
        // the data lock is held — then deliberately never answers.
        let dispatched = endpoint.clone();
        let (parked_tx, parked_rx) = std::sync::mpsc::channel::<()>();
        let exchange = std::thread::spawn(move || {
            parked_tx.send(()).unwrap();
            dispatched.entry_metadata(&workspace)
        });
        parked_rx.recv().unwrap();
        match read_record::<ClientRequest>(&mut client).unwrap() {
            ClientRequest::GetMetadata { .. } => {}
            request => panic!("unexpected in-flight request: {request:?}"),
        }

        // The fence runs on its own thread only so the test can bound its
        // completion: a fence that waited on the held data lock would
        // park for the whole 60-second exchange deadline.
        let (fenced_tx, fenced_rx) = std::sync::mpsc::channel::<()>();
        let fencer = std::thread::spawn(move || {
            slot.close_admission(None)
                .unwrap()
                .fence_connection_by(
                    Instant::now() + rustx_execution_protocol::transport::CONTROL_COMMAND_TIMEOUT,
                )
                .unwrap();
            fenced_tx.send(()).unwrap();
        });
        fenced_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("the fence never waits on a busy exchange");

        // The interrupted exchange surfaces an I/O error — the raw
        // shutdown woke its reply read — long before its deadline.
        let result = exchange.join().expect("the dispatch thread exits");
        assert!(result.is_err(), "a fenced exchange cannot complete");

        // The binding is terminally revoked: a second well-formed Hello is
        // refused, and no admission ever lands on this endpoint again.
        let mut second = UnixStream::connect(socket_path(&bundle(&endpoint))).unwrap();
        let (accepted, _) = match endpoint.listener() {
            ExecutionListener::Unix(listener) => listener.accept().unwrap(),
            ExecutionListener::WebSocket(_) => panic!("unix endpoint expected"),
        };
        let published = bundle(&endpoint);
        write_record(
            &mut second,
            &PeerHello::Data(Hello {
                version: PROTOCOL_VERSION,
                session: published.session.clone(),
                environment: published.environment.clone(),
                instance: published.instance.clone(),
                generation: published.generation,
                workspace: published.workspace.clone(),
                user_root: "/client/rustx".into(),
                capabilities: BTreeSet::from([Capability::Connection]),
                credential: published.credential.clone(),
            }),
        )
        .unwrap();
        assert!(matches!(
            endpoint.admit_local_stream(accepted).unwrap(),
            EndpointAdmission::Rejected(rustx_execution_protocol::Reject::Revoked)
        ));
        fencer.join().unwrap();
        drop(dir);
        drop(registry);
    }

    fn admitted_lines_endpoint() -> (
        Arc<SessionExecutionRegistry>,
        SessionId,
        Arc<LocalExecutionEndpoint>,
        UnixStream,
        PathBuf,
        tempfile::TempDir,
    ) {
        admitted_endpoint(BTreeSet::from([
            Capability::Connection,
            Capability::ReadLines,
        ]))
    }

    fn admitted_ranges_endpoint() -> (
        Arc<SessionExecutionRegistry>,
        SessionId,
        Arc<LocalExecutionEndpoint>,
        UnixStream,
        PathBuf,
        tempfile::TempDir,
    ) {
        admitted_endpoint(BTreeSet::from([
            Capability::Connection,
            Capability::ReadRange,
        ]))
    }

    /// An independently-created scope over `registry` for `session` —
    /// every scope built for the same Session routes through the same
    /// endpoint and therefore shares its permits.
    fn scope_for(
        registry: &Arc<SessionExecutionRegistry>,
        session: &SessionId,
    ) -> SessionExecutionScope {
        SessionExecutionScope {
            session: session.clone(),
            route: ExecutionRoute::Registry(Arc::downgrade(registry)),
        }
    }

    /// A checked `Lines` reply for `path`: one trailing newline, so the
    /// page is `Complete` with the file's own bytes — the size equals
    /// the text length, the exact accounting a whole first page needs.
    fn lines_reply(path: &Path) -> ClientReply {
        let text = format!("{}\n", path.display());
        ClientReply::Lines {
            metadata: FileStamp {
                size: DecimalU64::new(text.len() as u64),
                revision: "a".repeat(64),
                identity: String::from("1:1"),
                modified_at_ms: None,
            },
            text,
            total_lines: DecimalU64::new(2),
            shown_lines: 2,
            end: rustx_execution_protocol::LineEnd::Complete,
        }
    }

    /// Waits until the endpoint's shared data budget drains to
    /// `remaining` free permits — bounded so a stuck dispatch fails the
    /// test instead of hanging it.
    async fn wait_data_slots(endpoint: &Arc<LocalExecutionEndpoint>, remaining: usize) {
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            while endpoint.data_slots.available_permits() != remaining {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("data slot budget reached");
    }

    /// A scripted peer for exactly one `ReadLines` request.
    fn answer_lines(mut client: UnixStream, expected: &Path) -> UnixStream {
        let request: ClientRequest = read_record(&mut client).unwrap();
        let ClientRequest::ReadLines { relative_path, .. } = request else {
            panic!("expected a lines request");
        };
        assert_eq!(relative_path, expected);
        write_record(&mut client, &lines_reply(&relative_path)).unwrap();
        client
    }

    /// The shared data budget and the single writer permit are per
    /// *connection*: eight admitted Agent dispatches — one holding the
    /// wire, seven waiting on the async side — exhaust the budget, so a
    /// ninth scope's dispatch is `WouldBlock` with a quiet wire. A
    /// cancelled waiter releases its permit and writes no frame; every
    /// released exchange returns its own path's bytes — no
    /// cross-accounting between queued dispatches.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn agent_page_dispatches_share_one_bounded_writer_budget() {
        let (registry, session, endpoint, mut client, _workspace, _dir) = admitted_lines_endpoint();
        // The parked peer: each request's path is reported, the reply is
        // held until the test releases it — so exactly one dispatch is
        // on the wire while the rest wait.
        let (arrived_tx, mut arrived_rx) = tokio::sync::mpsc::unbounded_channel::<PathBuf>();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let peer = std::thread::spawn(move || {
            while let Ok(request) = read_record::<ClientRequest>(&mut client) {
                let ClientRequest::ReadLines { relative_path, .. } = request else {
                    panic!("expected a lines request");
                };
                arrived_tx.send(relative_path.clone()).unwrap();
                if release_rx.recv().is_err() {
                    break;
                }
                write_record(&mut client, &lines_reply(&relative_path)).unwrap();
            }
            client
        });

        let reason = crate::runtime::types::CancellationReason::UserRequested;
        let mut tasks = Vec::new();
        let mut signals = Vec::new();
        for index in 0..MAX_DATA_RECORDS_PER_CONNECTION {
            let scope = scope_for(&registry, &session);
            let signal = crate::runtime::CancellationSignal::new();
            signals.push(signal.clone());
            let cancellation = crate::runtime::ExecutionCancellation::detached(signal, reason);
            let path = PathBuf::from(format!("p{index}"));
            tasks.push(tokio::spawn(async move {
                (
                    path.clone(),
                    scope.read_page(&path, 1, None, false, &cancellation).await,
                )
            }));
        }
        // All eight admitted: the shared budget is exhausted.
        wait_data_slots(&endpoint, 0).await;
        // One is on the wire; the rest wait on the writer permit.
        let active = arrived_rx.recv().await.expect("the first request arrived");
        assert_eq!(endpoint.data_slots.available_permits(), 0);

        // The ninth admitted-scope dispatch — an independently created
        // scope — is refused by the spent budget, before the wire.
        let ninth_scope = scope_for(&registry, &session);
        let ninth_cancellation = crate::runtime::ExecutionCancellation::detached(
            crate::runtime::CancellationSignal::new(),
            reason,
        );
        let error = ninth_scope
            .read_page(Path::new("p9"), 1, None, false, &ninth_cancellation)
            .await
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock, "{error}");

        // Cancel one waiter: its permit returns and no frame is ever
        // written for its path.
        let active_index: usize = active
            .to_str()
            .unwrap()
            .strip_prefix('p')
            .unwrap()
            .parse()
            .unwrap();
        let cancelled_index = usize::from(active_index == 0);
        signals[cancelled_index].cancel();
        let (cancelled_path, cancelled_result) = tasks.remove(cancelled_index).await.unwrap();
        assert_eq!(cancelled_path, PathBuf::from(format!("p{cancelled_index}")));
        assert_eq!(
            cancelled_result.unwrap_err().kind(),
            io::ErrorKind::Interrupted
        );
        wait_data_slots(&endpoint, 1).await;

        // Release every exchange in arrival order; the cancelled path
        // never reaches the wire and each reply belongs to its request.
        release_tx.send(()).unwrap();
        let mut served = vec![active];
        while served.len() < MAX_DATA_RECORDS_PER_CONNECTION - 1 {
            let arrived =
                tokio::time::timeout(std::time::Duration::from_secs(30), arrived_rx.recv())
                    .await
                    .expect("queued dispatches proceed")
                    .expect("peer stays live");
            assert_ne!(
                arrived,
                PathBuf::from(format!("p{cancelled_index}")),
                "a cancelled waiter must never write a frame"
            );
            release_tx.send(()).unwrap();
            served.push(arrived);
        }
        for task in tasks {
            let (path, result) = task.await.unwrap();
            assert_eq!(
                result.unwrap().text,
                format!("{}\n", path.display()),
                "each exchange accounts for its own path"
            );
        }
        drop(arrived_rx);
        drop(release_tx);
        // The registry's claim slot also holds an Arc to the endpoint —
        // both must go before the connection closes and the peer's
        // blocking read sees EOF.
        drop(registry);
        drop(endpoint);
        let mut client = peer.join().unwrap();
        wire_is_closed(&mut client);
    }

    /// The independent control lane is never charged against the data
    /// budget: eight real `read_page` futures own every data permit —
    /// one exchange parked on the wire, seven waiting — a ninth
    /// dispatch is `WouldBlock`, and *while that stands* fresh
    /// bidirectional health proofs and a real cancel receipt still
    /// round-trip on the paired control lane, before any data drains.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn the_control_lane_progresses_while_every_data_permit_is_parked() {
        let (registry, session, endpoint, mut client, _workspace, _dir) = admitted_lines_endpoint();
        // The parked peer: each request's path is reported, the reply is
        // held until the test releases it — every data permit stays
        // genuinely occupied by a live exchange.
        let (arrived_tx, mut arrived_rx) = tokio::sync::mpsc::unbounded_channel::<PathBuf>();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let peer = std::thread::spawn(move || {
            while let Ok(request) = read_record::<ClientRequest>(&mut client) {
                let ClientRequest::ReadLines { relative_path, .. } = request else {
                    panic!("expected a lines request");
                };
                arrived_tx.send(relative_path.clone()).unwrap();
                if release_rx.recv().is_err() {
                    break;
                }
                write_record(&mut client, &lines_reply(&relative_path)).unwrap();
            }
            client
        });

        let reason = crate::runtime::types::CancellationReason::UserRequested;
        let mut tasks = Vec::new();
        for index in 0..MAX_DATA_RECORDS_PER_CONNECTION {
            let scope = scope_for(&registry, &session);
            let cancellation = crate::runtime::ExecutionCancellation::detached(
                crate::runtime::CancellationSignal::new(),
                reason,
            );
            let path = PathBuf::from(format!("p{index}"));
            tasks.push(tokio::spawn(async move {
                scope.read_page(&path, 1, None, false, &cancellation).await
            }));
        }
        // All eight permits held by real in-flight exchanges; the ninth
        // is refused — the budget is real, not a semaphore claim.
        wait_data_slots(&endpoint, 0).await;
        let first = arrived_rx
            .recv()
            .await
            .expect("one exchange is on the wire");
        assert_eq!(endpoint.data_slots.available_permits(), 0);
        let ninth_scope = scope_for(&registry, &session);
        let ninth_cancellation = crate::runtime::ExecutionCancellation::detached(
            crate::runtime::CancellationSignal::new(),
            reason,
        );
        let error = ninth_scope
            .read_page(Path::new("p9"), 1, None, false, &ninth_cancellation)
            .await
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock, "{error}");

        // The paired control lane — a separate physical socket — is
        // not charged to that budget: fresh health proofs and a cancel
        // receipt advance while every data permit stays parked.
        let control = endpoint
            .control()
            .expect("a live pair has a control handle");
        control.health().unwrap();
        assert_eq!(
            control.cancel_mutation(DecimalU64::new(41)).unwrap(),
            rustx_execution_protocol::CancelDisposition::Unknown
        );
        control.health().unwrap();
        assert_eq!(endpoint.data_slots.available_permits(), 0);

        // Drain every exchange in arrival order — the budget unwinds
        // exactly, one reply per held request.
        release_tx.send(()).unwrap();
        let mut served = vec![first];
        while served.len() < MAX_DATA_RECORDS_PER_CONNECTION {
            let arrived =
                tokio::time::timeout(std::time::Duration::from_secs(30), arrived_rx.recv())
                    .await
                    .expect("queued dispatches proceed")
                    .expect("peer stays live");
            release_tx.send(()).unwrap();
            served.push(arrived);
        }
        for task in tasks {
            assert!(task.await.unwrap().is_ok(), "each exchange completes");
        }
        drop(arrived_rx);
        drop(release_tx);
        drop(registry);
        drop(endpoint);
        let mut client = peer.join().unwrap();
        wire_is_closed(&mut client);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[allow(clippy::too_many_lines)] // Keep the admitted wire sequence and competing writer order together.
    async fn cancelled_preparation_waits_for_the_competing_writer_before_abort() {
        let (registry, session, endpoint, mut client, _workspace, _dir) =
            admitted_endpoint(BTreeSet::from([
                Capability::Connection,
                Capability::ConditionalWrite,
                Capability::ReadLines,
            ]));
        let (arrived, prepared) = tokio::sync::oneshot::channel();
        let (reply, prepared_reply) = std::sync::mpsc::channel::<()>();
        let (aborted, abort_receipt) = tokio::sync::oneshot::channel();
        let ticket = DecimalU64::new(1);
        let next_ticket = DecimalU64::new(2);
        let peer = std::thread::spawn(move || {
            assert!(matches!(read_record::<ClientRequest>(&mut client).unwrap(),
                ClientRequest::PrepareWrite { relative_path, .. } if relative_path == Path::new("cancelled")));
            arrived.send(()).unwrap();
            prepared_reply.recv().unwrap();
            write_record(
                &mut client,
                &ClientReply::WritePrepared {
                    ticket,
                    original: None,
                },
            )
            .unwrap();
            assert_eq!(
                read_record::<ClientRequest>(&mut client).unwrap(),
                ClientRequest::AbortWrite { ticket }
            );
            write_record(&mut client, &ClientReply::WriteAborted { ticket }).unwrap();
            aborted.send(()).unwrap();
            let mut client = answer_lines(client, Path::new("note"));
            assert!(matches!(read_record::<ClientRequest>(&mut client).unwrap(),
                ClientRequest::PrepareWrite { relative_path, .. } if relative_path == Path::new("success")));
            write_record(
                &mut client,
                &ClientReply::WritePrepared {
                    ticket: next_ticket,
                    original: None,
                },
            )
            .unwrap();
            assert_eq!(
                read_record::<ClientRequest>(&mut client).unwrap(),
                ClientRequest::CommitWrite {
                    ticket: next_ticket
                }
            );
            write_record(
                &mut client,
                &ClientReply::Written {
                    ticket: next_ticket,
                    metadata: FileStamp {
                        revision: "a".repeat(64),
                        identity: "1:1".into(),
                        size: DecimalU64::new(3),
                        modified_at_ms: None,
                    },
                },
            )
            .unwrap();
            client
        });
        let scope = scope_for(&registry, &session);
        let cancelled = crate::runtime::CancellationSignal::new();
        let cancellation = ExecutionCancellation::detached(
            cancelled.clone(),
            crate::runtime::types::CancellationReason::UserRequested,
        );
        let owner = scope.clone();
        let preparation = tokio::spawn(async move {
            owner
                .prepare_write(Path::new("cancelled"), b"one".to_vec(), &cancellation)
                .await
        });
        prepared.await.unwrap();
        let mut competing = Box::pin(endpoint.agent_writer.clone().acquire_owned());
        assert!(futures_util::poll!(&mut competing).is_pending());
        cancelled.cancel();
        reply.send(()).unwrap();
        let competing = competing.await.unwrap();
        assert_eq!(
            preparation.await.unwrap().unwrap_err().kind(),
            io::ErrorKind::Interrupted
        );
        wait_data_slots(&endpoint, MAX_DATA_RECORDS_PER_CONNECTION - 1).await;
        assert_eq!(endpoint.binding().status().phase, BindingPhase::Live);
        drop(competing);
        abort_receipt.await.unwrap();
        let cancellation = detached_cancellation();
        assert_eq!(
            scope
                .read_page(Path::new("note"), 1, None, false, &cancellation)
                .await
                .unwrap()
                .text,
            "note\n"
        );
        let prepared = scope
            .prepare_write(Path::new("success"), b"two".to_vec(), &cancellation)
            .await
            .unwrap();
        let written = scope
            .commit_prepared(&prepared, &cancellation)
            .await
            .unwrap();
        assert_eq!(written.size, 3);
        assert_eq!(endpoint.binding().status().phase, BindingPhase::Live);
        drop(peer.join().unwrap());
    }

    /// An `admit` refusal runs after encode + the binding check but
    /// before the first request byte: the caller's error returns
    /// verbatim, the wire stays quiet, and the connection serves the
    /// next dispatch normally — a refused admission never retires.
    #[test]
    fn a_refused_page_admission_writes_no_frame_and_keeps_the_connection() {
        let (_registry, _session, endpoint, mut client, _workspace, _dir) =
            admitted_lines_endpoint();
        let data = endpoint.data_slots.clone().try_acquire_owned().unwrap();
        let writer = endpoint.agent_writer.clone().try_acquire_owned().unwrap();
        let refused = io::Error::new(io::ErrorKind::Interrupted, "read cancelled before dispatch");
        let error = endpoint
            .read_page_admitted(
                Path::new("note"),
                1,
                None,
                false,
                Instant::now() + PAGE_REQUEST_TIMEOUT,
                data,
                writer,
                || Err(refused),
                || panic!("a refused admission never reaches the wire"),
            )
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert_eq!(error.to_string(), "read cancelled before dispatch");
        wire_is_quiet(&mut client);

        // The connection stayed live: an ordinary page dispatch works.
        let answer = std::thread::spawn(move || answer_lines(client, Path::new("note")));
        let page = endpoint
            .read_lines(Path::new("note"), 1, None, None)
            .unwrap()
            .1;
        assert_eq!(page.text, "note\n");
        let mut client = answer.join().unwrap();
        wire_is_quiet(&mut client);
    }

    /// An absolute deadline carried into the exchange bounds the whole
    /// round trip: an already-expired budget writes nothing, returns
    /// `TimedOut`, and retires the connection — a partial reply
    /// accounting can never be trusted.
    #[test]
    fn an_expired_page_deadline_times_out_and_retires_the_connection() {
        let (_registry, _session, endpoint, mut client, _workspace, _dir) =
            admitted_lines_endpoint();
        let data = endpoint.data_slots.clone().try_acquire_owned().unwrap();
        let writer = endpoint.agent_writer.clone().try_acquire_owned().unwrap();
        let error = endpoint
            .read_page_admitted(
                Path::new("note"),
                1,
                None,
                false,
                Instant::now()
                    .checked_sub(std::time::Duration::from_secs(1))
                    .unwrap(),
                data,
                writer,
                || Ok(()),
                || panic!("an expired deadline never reaches the wire"),
            )
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(matches!(
            endpoint.binding().status().phase,
            BindingPhase::Detached
        ));
        wire_is_closed(&mut client);
    }

    /// A detached caller token for Agent data dispatches.
    fn detached_cancellation() -> ExecutionCancellation {
        ExecutionCancellation::detached(
            crate::runtime::CancellationSignal::new(),
            crate::runtime::types::CancellationReason::UserRequested,
        )
    }

    /// A checked `Range` reply: `bytes` at `offset`, `next_offset`
    /// continuing the file, stamped `size`/`revision`/`identity`.
    fn range_reply(
        offset: u64,
        bytes: &[u8],
        next_offset: Option<u64>,
        size: u64,
        revision: &str,
        identity: &str,
    ) -> ClientReply {
        use base64::Engine;
        ClientReply::Range {
            data_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
            offset: DecimalU64::new(offset),
            next_offset: next_offset.map(DecimalU64::new),
            metadata: FileStamp {
                size: DecimalU64::new(size),
                revision: revision.to_owned(),
                identity: identity.to_owned(),
                modified_at_ms: None,
            },
        }
    }

    /// A scripted peer for one `ReadRange` request answered by `reply`,
    /// returned for assertion.
    fn answer_range_reply(
        mut client: UnixStream,
        reply: &ClientReply,
    ) -> (UnixStream, ClientRequest) {
        let request: ClientRequest = read_record(&mut client).unwrap();
        let ClientRequest::ReadRange { .. } = request else {
            panic!("expected a range request");
        };
        write_record(&mut client, reply).unwrap();
        (client, request)
    }

    /// A source whose stamped size exceeds the artifact transfer bound
    /// is refused after exactly one `ReadRange` — `FileTooLarge`, no
    /// follow-up request, and the honest refusal never retires the
    /// connection: the next dispatch is served normally.
    #[tokio::test]
    async fn an_oversized_image_source_reads_one_range_and_stays_live() {
        let (registry, session, endpoint, mut client, workspace, _dir) = admitted_ranges_endpoint();
        let oversized = crate::tools::artifacts::ARTIFACT_TRANSFER_MAX as u64 + 1;
        let peer = std::thread::spawn(move || {
            let request: ClientRequest = read_record(&mut client).unwrap();
            let ClientRequest::ReadRange {
                relative_path,
                offset,
                max_bytes,
                expected_revision,
            } = request
            else {
                panic!("expected a range request");
            };
            assert_eq!(relative_path, Path::new("image.png"));
            assert_eq!(offset, DecimalU64::new(0));
            assert_eq!(usize::try_from(max_bytes), Ok(MAX_RAW_CHUNK_BYTES));
            assert!(expected_revision.is_none());
            write_record(
                &mut client,
                &range_reply(
                    0,
                    &vec![0xAB; MAX_RAW_CHUNK_BYTES],
                    Some(MAX_RAW_CHUNK_BYTES as u64),
                    oversized,
                    "a".repeat(64).as_str(),
                    "1:1",
                ),
            )
            .unwrap();
            client
        });
        let error = scope_for(&registry, &session)
            .image_bytes(&workspace.join("image.png"), &detached_cancellation())
            .await
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::FileTooLarge, "{error}");
        let mut client = peer.join().unwrap();
        wire_is_quiet(&mut client);

        // The honest oversize refusal kept the connection live.
        let answer = std::thread::spawn(move || {
            answer_range_reply(
                client,
                &range_reply(0, b"x", None, 1, "a".repeat(64).as_str(), "1:1"),
            )
        });
        let range = endpoint
            .read_range(&workspace.join("image.png"), 0, 64, None)
            .unwrap();
        assert_eq!(range.bytes, b"x");
        let (mut client, _) = answer.join().unwrap();
        wire_is_quiet(&mut client);
    }

    /// A file replaced between chunks surfaces as the peer's typed
    /// `conflict` — an honest refusal that never publishes assembled
    /// bytes and never retires the connection.
    #[tokio::test]
    async fn a_replacement_between_image_chunks_is_an_honest_conflict() {
        let (registry, session, endpoint, client, workspace, _dir) = admitted_ranges_endpoint();
        let revision = "a".repeat(64);
        let peer = std::thread::spawn(move || {
            let (client, _) = answer_range_reply(
                client,
                &range_reply(
                    0,
                    &vec![0xAB; MAX_RAW_CHUNK_BYTES],
                    Some(MAX_RAW_CHUNK_BYTES as u64),
                    (MAX_RAW_CHUNK_BYTES * 2) as u64,
                    &revision,
                    "1:1",
                ),
            );
            let mut client = client;
            let request: ClientRequest = read_record(&mut client).unwrap();
            let ClientRequest::ReadRange {
                offset,
                expected_revision,
                ..
            } = request
            else {
                panic!("expected the follow-up range request");
            };
            assert_eq!(offset, DecimalU64::new(MAX_RAW_CHUNK_BYTES as u64));
            assert_eq!(expected_revision.as_deref(), Some(revision.as_str()));
            write_record(
                &mut client,
                &ClientReply::Error {
                    kind: ReadError::Conflict,
                },
            )
            .unwrap();
            client
        });
        let error = scope_for(&registry, &session)
            .image_bytes(&workspace.join("image.png"), &detached_cancellation())
            .await
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists, "{error}");
        let mut client = peer.join().unwrap();
        wire_is_quiet(&mut client);

        // A typed refusal stays live: the next dispatch is served.
        let answer = std::thread::spawn(move || {
            answer_range_reply(
                client,
                &range_reply(0, b"y", None, 1, "b".repeat(64).as_str(), "2:2"),
            )
        });
        assert_eq!(
            endpoint
                .read_range(&workspace.join("image.png"), 0, 64, None)
                .unwrap()
                .bytes,
            b"y"
        );
        let (mut client, _) = answer.join().unwrap();
        wire_is_quiet(&mut client);
    }

    /// A reply stamped differently than the pinned first chunk — same
    /// revision, another identity — is a peer that cannot account for
    /// its bytes: the assembled image is never returned and the
    /// connection retires.
    #[tokio::test]
    async fn a_stamped_mismatch_between_image_chunks_retires_the_connection() {
        let (registry, session, endpoint, client, workspace, _dir) = admitted_ranges_endpoint();
        let revision = "a".repeat(64);
        let peer = std::thread::spawn(move || {
            let (client, _) = answer_range_reply(
                client,
                &range_reply(
                    0,
                    &vec![0xAB; MAX_RAW_CHUNK_BYTES],
                    Some(MAX_RAW_CHUNK_BYTES as u64),
                    (MAX_RAW_CHUNK_BYTES * 2) as u64,
                    &revision,
                    "1:1",
                ),
            );
            let (client, _) = answer_range_reply(
                client,
                &range_reply(
                    MAX_RAW_CHUNK_BYTES as u64,
                    &vec![0xCD; MAX_RAW_CHUNK_BYTES],
                    None,
                    (MAX_RAW_CHUNK_BYTES * 2) as u64,
                    &revision,
                    "2:2",
                ),
            );
            client
        });
        let error = scope_for(&registry, &session)
            .image_bytes(&workspace.join("image.png"), &detached_cancellation())
            .await
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData, "{error}");
        assert!(matches!(
            endpoint.binding().status().phase,
            BindingPhase::Detached
        ));
        let mut client = peer.join().unwrap();
        wire_is_closed(&mut client);
    }

    /// A verified `SearchPage` crosses the scope's bounded dispatch; a
    /// peer that cannot account for its search — a match count beyond
    /// the requested limit, or `hit_limit` without a proven further
    /// match — retires the connection rather than publish dishonest
    /// records.
    #[tokio::test]
    async fn a_search_page_is_verified_and_a_dishonest_one_retires() {
        let (registry, session, endpoint, mut client, workspace, _dir) =
            admitted_endpoint(BTreeSet::from([
                Capability::Connection,
                Capability::BoundedSearch,
            ]));
        let peer = std::thread::spawn(move || {
            let Ok(ClientRequest::Search { spec, .. }) = read_record(&mut client) else {
                panic!("expected a search request");
            };
            assert!(matches!(
                spec,
                rustx_execution_protocol::SearchSpec::Glob { .. }
            ));
            write_record(
                &mut client,
                &ClientReply::SearchPage {
                    body: "a.txt".to_owned(),
                    matches: DecimalU64::new(1),
                    hit_limit: false,
                    byte_limit: false,
                    short_lines: false,
                },
            )
            .unwrap();
            // Dishonest: the count exceeds the requested limit.
            let Ok(ClientRequest::Search { .. }) = read_record(&mut client) else {
                panic!("expected a second search request");
            };
            write_record(
                &mut client,
                &ClientReply::SearchPage {
                    body: "x".to_owned(),
                    matches: DecimalU64::new(5),
                    hit_limit: false,
                    byte_limit: false,
                    short_lines: false,
                },
            )
            .unwrap();
            client
        });
        let scope = scope_for(&registry, &session);
        let spec = |limit| rustx_execution_protocol::SearchSpec::Glob {
            pattern: "*.txt".to_owned(),
            limit: DecimalU64::new(limit),
        };
        let result = scope
            .search(&workspace, spec(10), &detached_cancellation())
            .await
            .unwrap();
        assert_eq!(result.body, "a.txt");
        assert_eq!(result.matches, 1);
        let error = scope
            .search(&workspace, spec(2), &detached_cancellation())
            .await
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData, "{error}");
        assert!(matches!(
            endpoint.binding().status().phase,
            BindingPhase::Detached
        ));
        let mut client = peer.join().unwrap();
        wire_is_closed(&mut client);
    }

    /// One scripted `SearchPage` case: a peer answers `Search` with
    /// `reply`; `accepted` selects verified-`Ok`-and-quiet-wire versus
    /// `InvalidData`-and-retired.
    async fn search_case(
        spec: rustx_execution_protocol::SearchSpec,
        reply: ClientReply,
        accepted: bool,
    ) {
        let (registry, session, endpoint, mut client, workspace, _dir) =
            admitted_endpoint(BTreeSet::from([
                Capability::Connection,
                Capability::BoundedSearch,
            ]));
        let peer = std::thread::spawn(move || {
            let Ok(ClientRequest::Search { .. }) = read_record(&mut client) else {
                panic!("expected a search request");
            };
            write_record(&mut client, &reply).unwrap();
            client
        });
        let result = scope_for(&registry, &session)
            .search(&workspace, spec, &detached_cancellation())
            .await;
        let mut client = peer.join().unwrap();
        if accepted {
            result.unwrap();
            wire_is_quiet(&mut client);
        } else {
            assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidData);
            assert!(matches!(
                endpoint.binding().status().phase,
                BindingPhase::Detached
            ));
            wire_is_closed(&mut client);
        }
        drop(endpoint);
        drop(registry);
    }

    /// A scripted `SearchPage` reply matrix: the one honest zero-match
    /// body — before-context rows that filled the projection before the
    /// match record fit — is accepted; every impossible accounting
    /// combination retires the connection rather than reach the tool.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn search_reply_accounting_is_verified_before_trust() {
        let grep = |context: u32| rustx_execution_protocol::SearchSpec::Grep {
            pattern: "needle".to_owned(),
            glob: None,
            literal: false,
            ignore_case: false,
            context,
            limit: DecimalU64::new(10),
        };
        let page = |body: &str, matches: u64, hit: bool, byte: bool, short: bool| {
            ClientReply::SearchPage {
                body: body.to_owned(),
                matches: DecimalU64::new(matches),
                hit_limit: hit,
                byte_limit: byte,
                short_lines: short,
            }
        };
        let cases: &[(u32, ClientReply, bool)] = &[
            // Honest before-context overflow: context rows, zero matches.
            (20, page("d-1- ctx", 0, false, true, false), true),
            // The same overflow landing before the first record.
            (20, page("", 0, false, true, false), true),
            // A nonempty body without a match is impossible for context=0.
            (0, page("d-1- ctx", 0, false, true, false), false),
            // Glob has no context rows at all.
            (0, page("d:1: hit", 0, false, false, false), false),
            // Mutually exclusive limits.
            (0, page("x", 10, true, true, false), false),
            // A count past the body's own byte length.
            (0, page("x", 3, false, false, false), false),
        ];
        for (context, reply, accepted) in cases {
            search_case(grep(*context), reply.clone(), *accepted).await;
        }
        // A Glob `short_lines` flag is impossible on its own request.
        search_case(
            rustx_execution_protocol::SearchSpec::Glob {
                pattern: "*.txt".to_owned(),
                limit: DecimalU64::new(10),
            },
            page("a.txt", 1, false, false, true),
            false,
        )
        .await;
    }
}
