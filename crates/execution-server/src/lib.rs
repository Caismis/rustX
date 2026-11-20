//! Server-side binding arbitration for the rustX execution protocol.
//!
//! A `Binding` is the single arbitration point where the Server decides
//! which Client connection may receive environment dispatch. It is a
//! library owned by the Agent Server (package `rustx`) — not a second
//! Agent, and not a transport. This layer performs no dispatch and no
//! spawn; it proves authenticated connection ownership only.

use std::collections::{BTreeSet, VecDeque};
use std::io;
use std::net::TcpListener;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use base64::Engine as _;
use constant_time_eq::constant_time_eq_32;
use rustx_execution_protocol::process::ServerIncarnation;
#[cfg(test)]
use rustx_execution_protocol::transport::read_record;
use rustx_execution_protocol::watch::{
    MAX_WATCH_HINT_BYTES, MAX_WATCH_HINT_PATHS, WatchChange, WatchId, WatchReply, WatchRequest,
};
use rustx_execution_protocol::{
    Admission, BindingCredential, Bootstrap, BootstrapEndpoint, CancelDisposition, Capability,
    ClientInstanceId, ClientReply, ClientRequest, ConnectionId, ControlHello, ControlHelloReply,
    ControlPoll, ControlReply, ControlRequest, ControlTurn, DataAdmission, DataHelloReply,
    DecimalU64, EditRefusal, EntryStamp, EnvironmentId, FileStamp, Generation, Hello, LineEnd,
    MAX_DIRECTORY_NAMES, MAX_DIRECTORY_PAGE_ENTRIES, MAX_DOCUMENT_LOGICAL_BYTES,
    MAX_DOCUMENT_SOURCE_BYTES, MAX_EDIT_FILE_BYTES, MAX_EDIT_REPLACEMENTS, MAX_EDIT_REQUEST_BYTES,
    MAX_RAW_CHUNK_BYTES, MAX_SEARCH_CONTEXT_LINES, MAX_SEARCH_PATTERN_BYTES, MAX_TEXT_BYTES,
    MAX_TEXT_LINES, MAX_WRITE_BYTES, PROTOCOL_VERSION, PageEntry, PairingNonce, PeerHello,
    ProcessCancelDisposition, ReadError, Reject, SearchSpec, SessionId, TextReplacement,
    TreeMutation, WriteExpectation, is_normalized_absolute_path,
    transport::{
        CONTROL_COMMAND_TIMEOUT, CONTROL_FRAME_TIMEOUT, MAX_CONTROL_COMMANDS, PAGE_REQUEST_TIMEOUT,
        encode_record, write_record,
    },
};
use rustx_execution_transport::{ChannelShutdown, RecordChannel};
use sha2::{Digest, Sha256};

mod process_reply;

/// The deadline for a Client to present its Hello on an accepted local
/// connection — one shared budget owned by
/// [`rustx_execution_protocol::transport`].
pub use rustx_execution_protocol::transport::LOCAL_HANDSHAKE_TIMEOUT;

static SERVER_INCARNATION: std::sync::OnceLock<ServerIncarnation> = std::sync::OnceLock::new();

/// The one process-wide Server incarnation — allocated once, never per
/// runtime reload. Every [`Bootstrap`], [`Admission`], and process
/// operation identity is keyed under it.
#[must_use]
pub fn server_incarnation() -> ServerIncarnation {
    *SERVER_INCARNATION.get_or_init(ServerIncarnation::generate)
}

/// The fixed identity and requirements of one reserved binding.
///
/// The Server assigns every field at reservation time; a `Hello` can only
/// ever be checked against them. A `Hello` is never trusted to select the
/// required capabilities or the expected instance.
pub struct Reservation {
    /// The Session the binding belongs to.
    pub session: SessionId,
    /// The logical execution environment.
    pub environment: EnvironmentId,
    /// The only Client instance that may ever connect.
    pub instance: ClientInstanceId,
    /// The only generation accepted for that instance.
    pub generation: Generation,
    /// The exact normalized absolute workspace.
    pub workspace: PathBuf,
    /// Capabilities the Hello must claim.
    pub required_capabilities: BTreeSet<Capability>,
}

/// The observable phase of a [`Binding`].
///
/// `Reserved → Live → Detached` on disconnect; `Revoked` from any phase is
/// terminal. EOF moves `Live` to `Detached` — it never reopens the
/// reservation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingPhase {
    /// Credential issued, no connection yet committed.
    Reserved,
    /// Exactly one authenticated connection is committed.
    Live,
    /// The live connection ended. Every Hello is rejected with
    /// [`Reject::RecoveryRequired`] until live-instance recovery proof
    /// exists (#577).
    Detached,
    /// Terminal: revoked bindings are never dispatchable again.
    Revoked,
}

/// A consistent snapshot of a [`Binding`]'s phase and live connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindingStatus {
    /// The current phase.
    pub phase: BindingPhase,
    /// The live connection, present only in [`BindingPhase::Live`].
    pub connection: Option<ConnectionId>,
    /// Whether a pairing attempt is still negotiating — a pending half
    /// is never dispatchable: the binding only reports `Live` once the
    /// pair committed.
    pub pending: bool,
}

/// One execution binding: the single arbitration point where Hellos,
/// disconnects, and revocation linearize under one mutex.
pub struct Binding {
    reservation: Reservation,
    /// SHA-256 of the issued credential. The credential itself is never
    /// stored on the Server.
    verifier: [u8; 32],
    inner: Arc<ConnectionOwner>,
}

/// The shared arbitration state behind one [`Binding`]: the phase and
/// live connection linearize under one mutex, reached identically
/// through the binding and through any [`LocalConnection`] it already
/// handed out — a `LocalConnection` owns a share, it does not borrow.
struct ConnectionOwner {
    /// The one binding state lock — phase, the live pair, and the
    /// single pending attempt all linearize here.
    inner: Mutex<Inner>,
    /// Wakes a verified [`ControlHello`] whose correct nonce arrived
    /// while its data lane's channel was still being stored — the
    /// challenge reply is already on the wire, so an honest peer can
    /// legitimately beat the store; it parks here, bounded by the
    /// attempt's own deadline, never holding the state lock.
    pair_ready: Condvar,
}

impl ConnectionOwner {
    fn inner(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Ends the live pair identified by its data lane.
    ///
    /// Only the currently live data connection can detach the binding:
    /// a stale or foreign disconnect is ignored. The seized shutdown
    /// handles of both lanes are returned for the caller to invoke
    /// after locks release — a detached pair's control lane dies with
    /// its data lane, never lingers answering polls.
    fn disconnect(&self, connection: &ConnectionId) -> FencedHandles {
        let mut inner = self.inner();
        if inner.phase == BindingPhase::Live && inner.connection.as_ref() == Some(connection) {
            inner.phase = BindingPhase::Detached;
            inner.connection = None;
            FencedHandles(
                inner
                    .stream
                    .take()
                    .into_iter()
                    .chain(inner.control.take().map(|control| control.shutdown))
                    .collect(),
            )
        } else {
            FencedHandles::EMPTY
        }
    }

    /// Ends the live pair identified by its *control* lane — the lane
    /// ended or lied. The pair detaches identically to a data-lane
    /// loss: never a release, never a replacement — and the seized data
    /// handle lets the caller interrupt a data exchange that may be
    /// blocked mid-read.
    fn detach_control(&self, connection: &ConnectionId) -> FencedHandles {
        let mut inner = self.inner();
        if inner.phase == BindingPhase::Live
            && inner
                .control
                .as_ref()
                .is_some_and(|control| &control.connection == connection)
        {
            inner.phase = BindingPhase::Detached;
            inner.connection = None;
            FencedHandles(
                inner
                    .stream
                    .take()
                    .into_iter()
                    .chain(inner.control.take().map(|control| control.shutdown))
                    .collect(),
            )
        } else {
            FencedHandles::EMPTY
        }
    }

    /// The fence half of [`Binding::fence`] on the shared owner:
    /// terminal `Revoked`, every lane's handle seized.
    fn fence(&self) -> FencedHandles {
        let mut inner = self.inner();
        inner.phase = BindingPhase::Revoked;
        inner.connection = None;
        inner.pending = None;
        self.pair_ready.notify_all();
        FencedHandles(
            inner
                .stream
                .take()
                .into_iter()
                .chain(inner.control.take().map(|control| control.shutdown))
                .collect(),
        )
    }

    /// Discards the pending pairing attempt `attempt` — a watchdog's
    /// deadline expiry or peer-signal discard. A committed attempt is
    /// untouched; the reservation stays `Reserved` either way. Dropping
    /// the attempt closes its watchdog's cancel pipe, and every parked
    /// verifier is woken to judge the now-empty slot.
    fn discard_pending(&self, attempt: u64) {
        let mut inner = self.inner();
        if inner
            .pending
            .as_ref()
            .is_some_and(|pending| pending.attempt == attempt)
        {
            inner.pending = None;
            self.pair_ready.notify_all();
        }
    }

    /// Drops a pending attempt whose absolute deadline has passed —
    /// lazily, inside whatever lock hold observes it — waking parked
    /// verifiers so they judge the now-empty slot. Returns whether a
    /// pending attempt remains.
    fn pending_alive(&self, inner: &mut Inner) -> bool {
        if inner
            .pending
            .as_ref()
            .is_some_and(|pending| Instant::now() >= pending.deadline)
        {
            inner.pending = None;
            self.pair_ready.notify_all();
        }
        inner.pending.is_some()
    }
}

struct Inner {
    phase: BindingPhase,
    connection: Option<ConnectionId>,
    /// A shutdown-capable duplicate of the live data connection's
    /// transport — the Unix socket for a local binding or the raw TCP
    /// socket behind a WebSocket. It is never written or read: it exists
    /// so [`Binding::revoke`] can close the physical stream and wake a
    /// peer that no dispatch thread currently holds, without waiting on
    /// the data lock.
    stream: Option<ChannelShutdown>,
    /// The live control lane, committed atomically with the pair: its
    /// identity for lane-loss arbitration, its own shutdown duplicate,
    /// and the cloneable [`ControlHandle`] the endpoint hands out.
    control: Option<LiveControl>,
    /// The pairing half under negotiation — only ever while
    /// `phase == Reserved`. It holds the data lane's physical channel
    /// and shutdown handle so a pending half survives the accept worker
    /// returning, and never becomes dispatchable: nothing can send on it
    /// until the control lane commits the pair.
    pending: Option<PendingPair>,
    /// The monotonic pairing-attempt identity — incremented per data
    /// admission so a stale watchdog or a replaced half can never touch
    /// the attempt that superseded it.
    next_attempt: u64,
}

/// The live control lane the pair committed.
struct LiveControl {
    /// The lane's own connection identity.
    connection: ConnectionId,
    /// Its shutdown duplicate — fencing wakes a blocked control read.
    shutdown: ChannelShutdown,
    /// The command path independent of the data lane's lock.
    handle: ControlHandle,
}

/// One pairing attempt under negotiation: the data lane validated and
/// answered, the control lane not yet proven.
struct PendingPair {
    /// The unique attempt identity — watchdogs and channel stores match
    /// on it, so a superseded attempt dies cleanly.
    attempt: u64,
    /// SHA-256 verifier of the issued challenge; the nonce itself is
    /// never stored.
    verifier: [u8; 32],
    /// The absolute pairing deadline — the one 5-second budget armed at
    /// the data lane's accept, covering every lane's wait.
    deadline: Instant,
    /// The data lane's connection identity, reserved for the commit.
    connection: ConnectionId,
    /// The capabilities the data Hello claimed.
    capabilities: BTreeSet<Capability>,
    /// The held data channel — `None` only in the narrow window between
    /// pending allocation and the worker storing the stream, a state a
    /// replacement or fence can still discard safely.
    channel: Option<RecordChannel>,
    /// The data lane's shutdown duplicate — a fence or discard closes
    /// the physical stream — stored with the channel.
    shutdown: Option<ChannelShutdown>,
    /// The watchdog's cancel pipe write end — `None` until
    /// `watch_pending` installs it. Dropping the attempt — commit,
    /// discard, or fence — hangs the read end up so the parked watcher
    /// exits immediately instead of sleeping until data traffic or the
    /// deadline arrives.
    wake: Option<std::os::fd::OwnedFd>,
}

/// The physical shutdown handles a fence, detach, or disconnect seized.
/// The caller invokes [`Self::shutdown`] after every lock is released;
/// the struct itself performs no I/O.
pub struct FencedHandles(Vec<ChannelShutdown>);

impl FencedHandles {
    /// The empty seizure — nothing live to interrupt.
    const EMPTY: Self = Self(Vec::new());

    /// Shuts every seized lane down. Idempotent and best-effort: an
    /// already-dead lane's shutdown errors are discarded inside
    /// [`ChannelShutdown::shutdown`].
    pub fn shutdown(&self) {
        for handle in &self.0 {
            handle.shutdown();
        }
    }
}

impl Binding {
    /// Reserves a binding and issues its credential from OS randomness.
    ///
    /// The returned [`BindingCredential`] is the only copy; the `Binding`
    /// stores only its SHA-256 verifier. The workspace must already be a
    /// normalized absolute path — it is validated lexically and never
    /// touched on disk.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::InvalidInput`] if the workspace is not a
    /// normalized absolute path, or an OS error if randomness fails.
    pub fn reserve(reservation: Reservation) -> io::Result<(Self, BindingCredential)> {
        if !is_normalized_absolute_path(&reservation.workspace) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "workspace must be a normalized absolute path",
            ));
        }
        let mut secret = [0u8; BindingCredential::LENGTH];
        getrandom::fill(&mut secret).map_err(io::Error::other)?;
        let credential = BindingCredential::from_bytes(secret);
        let verifier: [u8; 32] = Sha256::digest(secret).into();
        let binding = Self {
            reservation,
            verifier,
            inner: Arc::new(ConnectionOwner {
                inner: Mutex::new(Inner {
                    phase: BindingPhase::Reserved,
                    connection: None,
                    stream: None,
                    control: None,
                    pending: None,
                    next_attempt: 0,
                }),
                pair_ready: Condvar::new(),
            }),
        };
        Ok((binding, credential))
    }

    /// Commits a live pair-less connection directly — the synthetic
    /// arbitration surface the phase tests exercise. The product path
    /// never reaches it: admission is always the paired
    /// [`PeerHello::Data`]/[`PeerHello::Control`] handshake.
    ///
    /// # Errors
    ///
    /// Returns the typed [`Reject`] the paired path would deliver on
    /// the wire.
    #[cfg(test)]
    pub fn connect(&self, hello: &Hello, connection: ConnectionId) -> Result<Admission, Reject> {
        let mut inner = self.inner();
        self.check_identity(
            &hello.credential,
            hello.version,
            &hello.session,
            &hello.environment,
            &hello.instance,
            hello.generation,
            &hello.workspace,
            Some(&hello.capabilities),
        )?;
        match inner.phase {
            BindingPhase::Reserved => {
                inner.phase = BindingPhase::Live;
                inner.connection = Some(connection);
                Ok(self.admission())
            }
            BindingPhase::Detached => Err(Reject::RecoveryRequired),
            BindingPhase::Live => Err(Reject::AlreadyConnected),
            BindingPhase::Revoked => Err(Reject::Revoked),
        }
    }

    /// The reserved identity every lane's Hello must echo — the
    /// [`Admission`] both pairing replies deliver.
    fn admission(&self) -> Admission {
        Admission {
            version: PROTOCOL_VERSION,
            server_incarnation: server_incarnation(),
            session: self.reservation.session.clone(),
            environment: self.reservation.environment.clone(),
            instance: self.reservation.instance.clone(),
            generation: self.reservation.generation,
            workspace: self.reservation.workspace.clone(),
        }
    }

    /// The shared identity gate both lanes pass: the credential verifier
    /// is checked first so no identity field can be probed without valid
    /// authentication; then every field must match the reservation
    /// exactly — the workspace by literal `OsStr` bytes, so
    /// `/work//root` never aliases `/work/root`. The data lane
    /// additionally proves the required capability subset.
    ///
    /// # Errors
    ///
    /// Returns the [`Reject`] matching the first failed check. Rejects
    /// never echo the submitted credential.
    #[allow(clippy::too_many_arguments)] // one Hello's fixed field set
    fn check_identity(
        &self,
        credential: &BindingCredential,
        version: u16,
        session: &SessionId,
        environment: &EnvironmentId,
        instance: &ClientInstanceId,
        generation: Generation,
        workspace: &Path,
        capabilities: Option<&BTreeSet<Capability>>,
    ) -> Result<(), Reject> {
        let offered: [u8; 32] = Sha256::digest(credential.as_bytes()).into();
        if !constant_time_eq_32(&self.verifier, &offered) {
            return Err(Reject::InvalidCredential);
        }
        if version != PROTOCOL_VERSION {
            return Err(Reject::UnsupportedVersion);
        }
        if session != &self.reservation.session {
            return Err(Reject::SessionMismatch);
        }
        if environment != &self.reservation.environment {
            return Err(Reject::EnvironmentMismatch);
        }
        if instance != &self.reservation.instance {
            return Err(Reject::InstanceMismatch);
        }
        if generation != self.reservation.generation {
            return Err(Reject::GenerationMismatch);
        }
        if workspace.as_os_str() != self.reservation.workspace.as_os_str() {
            return Err(Reject::WorkspaceMismatch);
        }
        if let Some(capabilities) = capabilities
            && !self
                .reservation
                .required_capabilities
                .is_subset(capabilities)
        {
            return Err(Reject::MissingCapabilities);
        }
        Ok(())
    }

    /// The phase check every lane's commit shares — under one mutex
    /// hold with the identity checks, so concurrent Hellos linearize.
    /// `Reserved` admits the attempt; `Detached` rejects every Hello
    /// with [`Reject::RecoveryRequired`] — EOF never reopens the
    /// reservation and re-admission needs live-instance recovery proof
    /// (#577), which this layer does not provide.
    fn check_phase(inner: &Inner) -> Result<(), Reject> {
        match inner.phase {
            BindingPhase::Reserved => Ok(()),
            BindingPhase::Detached => Err(Reject::RecoveryRequired),
            BindingPhase::Live => Err(Reject::AlreadyConnected),
            BindingPhase::Revoked => Err(Reject::Revoked),
        }
    }

    /// Allocates one pending pairing attempt for a validated data
    /// [`Hello`]: a fresh single-attempt challenge nonce, the attempt
    /// identity, and the absolute pairing deadline — the same 5-second
    /// budget the caller armed at accept time, covering every lane's
    /// wait. An attempt consumes nothing: `phase` stays `Reserved`
    /// until the control lane commits the pair, and a second data Hello
    /// while an attempt is live is refused — never a supersession — so
    /// a dead first half can hold the reservation only until its own
    /// bounded deadline expires it.
    ///
    /// # Errors
    ///
    /// Returns the [`Reject`] matching the first failed check.
    fn allocate_pending(
        &self,
        hello: &Hello,
        connection: ConnectionId,
        deadline: Instant,
    ) -> io::Result<Result<(PairingNonce, u64, Admission), Reject>> {
        let mut inner = self.inner();
        if let Err(reject) = self.check_identity(
            &hello.credential,
            hello.version,
            &hello.session,
            &hello.environment,
            &hello.instance,
            hello.generation,
            &hello.workspace,
            Some(&hello.capabilities),
        ) {
            return Ok(Err(reject));
        }
        if let Err(reject) = Self::check_phase(&inner) {
            return Ok(Err(reject));
        }
        if deadline.saturating_duration_since(Instant::now()).is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "handshake deadline expired",
            ));
        }
        // Expire a stale attempt before judging liveness; a live one is
        // never superseded — a second authenticated data claim while a
        // negotiation is underway is refused outright, so the honest
        // pair's half cannot be evicted and at most one pending attempt
        // (and one watchdog) ever exists per Session.
        if self.inner.pending_alive(&mut inner) {
            return Ok(Err(Reject::AlreadyConnected));
        }
        let mut secret = [0u8; PairingNonce::LENGTH];
        getrandom::fill(&mut secret).map_err(io::Error::other)?;
        let nonce = PairingNonce::from_bytes(secret);
        let verifier: [u8; 32] = Sha256::digest(secret).into();
        inner.next_attempt += 1;
        let attempt = inner.next_attempt;
        inner.pending = Some(PendingPair {
            attempt,
            verifier,
            deadline,
            connection,
            capabilities: hello.capabilities.clone(),
            channel: None,
            shutdown: None,
            wake: None,
        });
        Ok(Ok((nonce, attempt, self.admission())))
    }

    /// Stores the answered data lane's channel and shutdown handle into
    /// the pending attempt — the second half of
    /// [`Self::allocate_pending`] after the challenge reply is on the
    /// wire — and wakes any verified `ControlHello` already parked on
    /// [`ConnectionOwner::pair_ready`] waiting for exactly this. A dead
    /// attempt refuses the store: its channel is the caller's to close,
    /// and the peer's stale nonce can never pair.
    fn store_pending_channel(
        &self,
        attempt: u64,
        channel: RecordChannel,
        shutdown: ChannelShutdown,
    ) -> bool {
        let mut inner = self.inner();
        match inner.pending.as_mut() {
            Some(pending) if pending.attempt == attempt => {
                pending.channel = Some(channel);
                pending.shutdown = Some(shutdown);
                drop(inner);
                self.inner.pair_ready.notify_all();
                true
            }
            _ => false,
        }
    }

    /// Commits the live pair on a validated [`ControlHello`]: the nonce
    /// must verify against the pending attempt's verifier, the attempt
    /// must be inside its absolute deadline, and the transition to
    /// `Live` happens exactly once under the one mutex — the pending
    /// channel, identity, and capabilities move out as the data half of
    /// the committed pair.
    ///
    /// # Errors
    ///
    /// Returns [`Reject::InvalidPairing`] for any nonce problem —
    /// missing, expired, mismatched, or replayed — or the identity
    /// check's own [`Reject`].
    fn commit_pair(
        &self,
        hello: &ControlHello,
        connection: ConnectionId,
        control_shutdown: ChannelShutdown,
        control_handle: ControlHandle,
        deadline: Instant,
    ) -> Result<PairCommit, Reject> {
        self.check_identity(
            &hello.credential,
            hello.version,
            &hello.session,
            &hello.environment,
            &hello.instance,
            hello.generation,
            &hello.workspace,
            None,
        )?;
        let offered: [u8; 32] = Sha256::digest(hello.connection_nonce.as_bytes()).into();
        let mut inner = self.inner();
        Self::check_phase(&inner)?;
        // Waits for the data lane's channel store under the pairing
        // budget: a correct nonce proves the challenge reply reached
        // the peer — it is on the wire before `store_pending_channel`
        // runs — so an honest control lane can legitimately arrive in
        // the narrow delivery window. It parks on `pair_ready` with
        // the state lock released; a wrong nonce never waits and never
        // burns the honest attempt.
        let attempt = loop {
            Self::check_phase(&inner)?;
            // An expired attempt is discarded first: a nonce presented
            // past the pairing budget is invalid, never honoured late.
            if !self.inner.pending_alive(&mut inner) {
                return Err(Reject::InvalidPairing);
            }
            let pending = inner.pending.as_ref().expect("alive implies present");
            if !constant_time_eq_32(&pending.verifier, &offered) {
                return Err(Reject::InvalidPairing);
            }
            if pending.channel.is_some() {
                break pending.attempt;
            }
            // One absolute budget spans both lanes: the armed pairing
            // deadline from the data accept bounds this commit — the
            // control lane's own armed deadline can only tighten it.
            let budget = pending.deadline.min(deadline);
            let remaining = budget.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(Reject::InvalidPairing);
            }
            let (guard, _) = self
                .inner
                .pair_ready
                .wait_timeout(inner, remaining)
                .expect("binding state");
            inner = guard;
        };
        let mut pending = inner
            .pending
            .take()
            .expect("attempt held under the same lock hold");
        debug_assert_eq!(pending.attempt, attempt);
        // Recheck the data peer's liveness at the commit itself — never
        // rely on the asynchronous watchdog having already won: buffered
        // bytes or a close on the held lane both prove death or a
        // protocol violation, and either kills the attempt before the
        // pair ever lives. Dropping `pending` on refusal closes the held
        // channel and wakes its watchdog.
        let peer_gone = pending
            .shutdown
            .as_ref()
            .is_none_or(|shutdown| shutdown.readable(Instant::now()).unwrap_or(true));
        if peer_gone {
            return Err(Reject::InvalidPairing);
        }
        inner.phase = BindingPhase::Live;
        inner.connection = Some(pending.connection.clone());
        inner.stream = pending.shutdown.take();
        inner.control = Some(LiveControl {
            connection,
            shutdown: control_shutdown,
            handle: control_handle,
        });
        Ok(PairCommit {
            channel: pending.channel.take().expect("channel checked above"),
            connection: pending.connection,
            capabilities: pending.capabilities,
            admission: self.admission(),
            deadline: pending.deadline,
        })
    }

    /// Ends the live pair identified by its data lane — a stale or
    /// foreign disconnect is ignored. Both lanes' seized handles are
    /// shut down here: the caller of this terminal path holds no other
    /// endpoint lock.
    pub fn disconnect(&self, connection: &ConnectionId) {
        self.inner.disconnect(connection).shutdown();
    }

    /// Fences the binding atomically: marks it `Revoked`, clears the
    /// live connection record, and seizes the duplicated shutdown
    /// handle under the one inner lock. The state change and the handle
    /// transfer linearize with `connect_gated`'s commit, so a handshake
    /// either stored its handle before the fence — the fence then owns
    /// it — or meets `Revoked` at the gate and is refused. The caller,
    /// never the lock, decides when to invoke the handle: `fence`
    /// itself performs no I/O.
    ///
    /// A revoked binding rejects every future Hello and is never
    /// dispatchable again, whatever its previous phase. A pending pair
    /// is discarded with it — its held channel is closed under the
    /// lock, its peer observes EOF.
    #[must_use]
    pub fn fence(&self) -> FencedHandles {
        self.inner.fence()
    }

    /// Revokes the binding immediately and terminally: [`Self::fence`]
    /// then every seized lane's hard shutdown. A peer that is not
    /// currently locked for an exchange — including one holding an
    /// unpublished staged write — observes EOF, drops its
    /// per-connection write state, and cleans up its private stage. The
    /// shutdown never waits on the connection's data lock.
    pub fn revoke(&self) {
        self.fence().shutdown();
    }

    /// The live pair's cloneable control handle — the command path
    /// independent of the data lane's dispatch lock. `None` while no
    /// pair is live.
    #[must_use]
    pub fn control(&self) -> Option<ControlHandle> {
        self.inner
            .inner()
            .control
            .as_ref()
            .map(|control| control.handle.clone())
    }

    /// A consistent snapshot of the binding's phase and live connection.
    ///
    /// This is observation only: nothing may dispatch on a snapshot,
    /// because the phase can change before any use of it.
    #[must_use]
    pub fn status(&self) -> BindingStatus {
        let inner = self.inner();
        BindingStatus {
            phase: inner.phase,
            connection: inner.connection.clone(),
            pending: inner.pending.is_some(),
        }
    }

    /// Reserves a binding and publishes its local bootstrap bundle.
    ///
    /// Both `socket_path` and `bootstrap_path` must be normalized absolute
    /// paths that are direct children of an existing owner-only private
    /// directory (`0700`, owned by this process's euid); anything else is
    /// refused so an endpoint or credential can never be planted in a
    /// world-writable or foreign location. The socket is bound fresh —
    /// an existing filesystem entry is an error, never silently unlinked —
    /// and the bootstrap record is written with `create_new` at mode
    /// `0600` (enforced with `fchmod`, so a stricter process umask cannot
    /// weaken or strengthen it). The bundle carries only identities and the
    /// credential — never capabilities — so the file can never advertise a
    /// feature the Client does not implement.
    ///
    /// Failed publication leaves nothing behind: a created socket path is
    /// removed if the bootstrap write fails, and a partially written
    /// bootstrap file is removed too.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::InvalidInput`] for untrusted paths or an
    /// invalid workspace, [`io::ErrorKind::AlreadyExists`] when the socket
    /// or bootstrap path already exists, and the underlying OS error for
    /// bind/write failures.
    pub fn reserve_local(
        reservation: Reservation,
        socket_path: &Path,
        bootstrap_path: &Path,
    ) -> io::Result<(Self, UnixListener, PublishedBundle)> {
        let socket_slot = require_private_child(socket_path)?;
        let bootstrap_slot = require_private_child(bootstrap_path)?;
        let (binding, credential) = Self::reserve(reservation)?;
        let listener = UnixListener::bind(socket_path)?;
        let socket_stat = socket_slot.stat_child();
        match publish_bootstrap(
            BootstrapEndpoint::Unix {
                socket_path: socket_path.to_path_buf(),
            },
            &bootstrap_slot,
            &binding,
            credential,
        ) {
            Err(error) => {
                // The bound socket is removed only while it is still the
                // socket this call created — dev/inode proof through the
                // retained directory descriptor, never a blind unlink.
                if let Ok(proof) = socket_stat {
                    socket_slot.remove_if(&proof);
                }
                Err(error)
            }
            Ok(bundle) => Ok((binding, listener, bundle)),
        }
    }

    /// Reserves a binding and binds a WebSocket execution listener on an
    /// explicit numeric loopback address — the endpoint the remote
    /// composition publishes instead of a Unix socket.
    ///
    /// The actual listener is always cleartext loopback `ws://`: a public
    /// `wss://` endpoint is reachable only through operator-provisioned
    /// TLS ingress that terminates TLS and forwards to this loopback
    /// listener (proxy headers are never trusted — the binding still
    /// authenticates the Client Hello). `public_url`, when given, is the
    /// validated `wss://…/execution/v1` URL the bootstrap publishes;
    /// otherwise the bootstrap publishes this listener's `ws://` address
    /// directly.
    ///
    /// Like [`Self::reserve_local`], the binding, listener, and bootstrap
    /// file are created atomically: failure removes anything this call
    /// created and leaves the binding unreserved. `listen` must be a
    /// literal loopback address — port `0` selects an ephemeral port for
    /// tests. `bootstrap_path` must be a new direct child of an
    /// owner-only (`0700`) directory.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::InvalidInput`] for a non-loopback address,
    /// an invalid `public_url`, or an untrusted bootstrap path;
    /// [`io::ErrorKind::AlreadyExists`] when the bootstrap path already
    /// exists; and the underlying OS error for bind/write failures.
    pub fn reserve_websocket(
        reservation: Reservation,
        listen: std::net::SocketAddr,
        bootstrap_path: &Path,
        public_url: Option<&str>,
    ) -> io::Result<(Self, TcpListener, PublishedBundle)> {
        // The listener is a loopback-only plaintext endpoint: a remote
        // `wss://` reachability story terminates TLS at operator ingress,
        // never inside this process. Refusing non-loopback binds here
        // keeps an accidental `0.0.0.0` from exposing the upgrade.
        if !listen.ip().is_loopback() {
            return Err(invalid_input(
                "WebSocket execution listener must bind a loopback address",
            ));
        }
        let bootstrap_slot = require_private_child(bootstrap_path)?;
        let url = match public_url {
            Some(url) => {
                // The public endpoint must be a validated wss URL on the
                // exact protocol path — the same shape a Client demands.
                let validated = rustx_execution_transport::ExecutionUrl::validate(url)
                    .map_err(|reason| invalid_input_static(&reason))?;
                if !validated.is_secure() {
                    return Err(invalid_input(
                        "public execution endpoint must be a wss:// URL",
                    ));
                }
                url.to_owned()
            }
            None => String::new(),
        };
        let (binding, credential) = Self::reserve(reservation)?;
        let listener = TcpListener::bind(listen)?;
        let url = if url.is_empty() {
            format!("ws://{}/execution/v1", listener.local_addr()?)
        } else {
            url
        };
        let bundle = publish_bootstrap(
            BootstrapEndpoint::WebSocket { url },
            &bootstrap_slot,
            &binding,
            credential,
        )?;
        Ok((binding, listener, bundle))
    }

    /// Performs the handshake on one accepted local stream.
    ///
    /// The Client gets [`LOCAL_HANDSHAKE_TIMEOUT`] to present a Hello. The
    /// connection identity is generated fresh here — one per accept —
    /// Runs the paired handshake on one accepted local stream: the peer
    /// declares its lane with [`PeerHello`], the lane's own checks run
    /// under the armed deadline, and the result is a [`ChannelAccept`] —
    /// a pending data half, a committed pair, or a delivered reject.
    ///
    /// # Errors
    ///
    /// Returns the transport or socket error when a record cannot be
    /// read or written. An authenticated-but-refused Hello is not an I/O
    /// error: it returns `Ok(ChannelAccept::Rejected(..))`.
    pub fn accept_local(&self, stream: UnixStream) -> io::Result<ChannelAccept> {
        self.accept_local_deadline(stream, Instant::now() + LOCAL_HANDSHAKE_TIMEOUT)
    }

    /// `accept_local` against an explicit absolute deadline — one budget
    /// that covers the Hello read, the arbitration, and the admission
    /// delivery, never re-armed.
    ///
    /// A composer that performs its own bounded wait before admission
    /// (an accept deadline, a task queue) arms one absolute deadline and
    /// passes it here so the handshake shares the already-elapsed budget;
    /// a deadline that has already lapsed fails the first check.
    ///
    /// # Errors
    ///
    /// Returns the transport or socket error when the record cannot be
    /// read or written. An authenticated-but-refused Hello is not an I/O
    /// error: it returns `Ok(ChannelAccept::Rejected(..))`.
    pub fn accept_local_deadline(
        &self,
        stream: UnixStream,
        deadline: Instant,
    ) -> io::Result<ChannelAccept> {
        self.accept_channel_deadline(RecordChannel::unix(stream), deadline)
    }

    /// Performs the paired handshake on one already-upgraded record
    /// channel — the transport-agnostic form of
    /// [`Self::accept_local_deadline`], identical in budget, arbitration,
    /// and delivery semantics for Unix and WebSocket transports alike.
    ///
    /// On the data lane the validated Hello allocates a pending attempt;
    /// the challenge reply is delivered before the channel is stored —
    /// a peer that never sees its challenge can never pair, so a
    /// delivery failure discards the attempt and the reservation stays
    /// `Reserved`. On the control lane the nonce-verified commit, the
    /// admission delivery, and the control worker's spawn run in that
    /// order: a committed pair whose admission cannot be delivered is
    /// detached and both lanes shut — a binding never stays live on a
    /// pair the peer does not know it owns.
    ///
    /// # Errors
    ///
    /// Returns the transport or socket error when the record cannot be
    /// read or written. An authenticated-but-refused Hello is not an I/O
    /// error: it returns `Ok(ChannelAccept::Rejected(..))`.
    pub fn accept_channel_deadline(
        &self,
        mut channel: RecordChannel,
        deadline: Instant,
    ) -> io::Result<ChannelAccept> {
        // The shutdown handles are duplicated before the handshake takes
        // the channel's only borrow: one for the stored lane, one for
        // the pending watchdog's poll.
        let lane_handle = channel.shutdown_handle()?;
        let hello: PeerHello = channel.receive(Some(deadline))?;
        match hello {
            PeerHello::Data(hello) => {
                let connection = fresh_connection_id()?;
                let watch = channel.shutdown_handle()?;
                match self.allocate_pending(&hello, connection.clone(), deadline)? {
                    Ok((nonce, attempt, admission)) => {
                        let reply = DataHelloReply::Ok(DataAdmission {
                            admission,
                            challenge: nonce,
                        });
                        let sent = encode_record(&reply)
                            .and_then(|encoded| channel.send(&encoded, Some(deadline)));
                        if let Err(error) = sent {
                            self.inner.discard_pending(attempt);
                            channel.shutdown();
                            return Err(error);
                        }
                        if !self.store_pending_channel(attempt, channel, lane_handle) {
                            // A superseding attempt won the store race:
                            // this peer's nonce is already dead.
                            return Ok(ChannelAccept::Pending);
                        }
                        self.watch_pending(attempt, watch, deadline)?;
                        Ok(ChannelAccept::Pending)
                    }
                    Err(reject) => {
                        let encoded = encode_record(&DataHelloReply::Err(reject))?;
                        channel.send(&encoded, Some(deadline))?;
                        Ok(ChannelAccept::Rejected(reject))
                    }
                }
            }
            PeerHello::Control(hello) => {
                let connection = fresh_connection_id()?;
                let shared = Arc::new(ControlShared::new());
                let handle = ControlHandle {
                    shared: shared.clone(),
                };
                match self.commit_pair(
                    &hello,
                    connection.clone(),
                    lane_handle,
                    handle.clone(),
                    deadline,
                ) {
                    Ok(commit) => {
                        // The control lane's admission is delivered
                        // under `min(first, second)` — a late second
                        // half can never stretch the one pairing budget.
                        let reply = ControlHelloReply::Ok(commit.admission.clone());
                        let delivery = commit.deadline.min(deadline);
                        let sent = encode_record(&reply)
                            .and_then(|encoded| channel.send(&encoded, Some(delivery)));
                        if let Err(error) = sent {
                            // Committed but undelivered: detach the pair
                            // and interrupt both lanes.
                            self.inner.disconnect(&commit.connection).shutdown();
                            channel.shutdown();
                            return Err(error);
                        }
                        let worker =
                            ControlWorker::spawn(channel, self.inner.clone(), connection, shared);
                        Ok(ChannelAccept::Live(LocalConnection {
                            owner: self.inner.clone(),
                            channel: commit.channel,
                            control: Some(worker),
                            usable: true,
                            closed: false,
                            capabilities: commit.capabilities,
                            connection: commit.connection,
                            admission: commit.admission,
                        }))
                    }
                    Err(reject) => {
                        let encoded = encode_record(&ControlHelloReply::Err(reject))?;
                        channel.send(&encoded, Some(deadline))?;
                        Ok(ChannelAccept::Rejected(reject))
                    }
                }
            }
        }
    }

    /// Spawns the pending attempt's watchdog: it polls the held data
    /// lane's duplicated handle — buffered peer bytes and a peer close
    /// are equally proof of death or a protocol violation — and discards
    /// the attempt on either signal or at the absolute pairing deadline.
    /// The reservation stays `Reserved` whatever the watchdog observes.
    /// One bounded thread per attempt — and at most one attempt per
    /// Session — never a queue.
    ///
    /// A cancel pipe wakes the watcher immediately when the attempt is
    /// settled another way: commit, discard, or fence drops the
    /// `PendingPair` and the write end with it, so the thread exits at
    /// once instead of sleeping until data traffic or the deadline.
    fn watch_pending(
        &self,
        attempt: u64,
        watch: ChannelShutdown,
        deadline: Instant,
    ) -> io::Result<()> {
        let (cancel_read, cancel_write) = nix::unistd::pipe().map_err(io::Error::from)?;
        {
            let mut inner = self.inner();
            match inner.pending.as_mut() {
                Some(pending) if pending.attempt == attempt => {
                    pending.wake = Some(cancel_write);
                }
                // The attempt already settled — nothing to watch.
                _ => return Ok(()),
            }
        }
        let owner = self.inner.clone();
        let cleanup = self.inner.clone();
        std::thread::Builder::new()
            .name("execution-pair-watch".to_owned())
            .spawn(move || {
                use std::os::fd::AsFd;
                let data = watch.as_fd().expect("a watchdog needs a descriptor");
                loop {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        break;
                    }
                    let timeout: nix::poll::PollTimeout =
                        remaining.try_into().unwrap_or(nix::poll::PollTimeout::MAX);
                    let mut fds = [
                        nix::poll::PollFd::new(data, nix::poll::PollFlags::POLLIN),
                        nix::poll::PollFd::new(cancel_read.as_fd(), nix::poll::PollFlags::POLLIN),
                    ];
                    match nix::poll::poll(&mut fds, timeout) {
                        Ok(0) => break,
                        Ok(_) => {
                            // The cancel pipe readable means the attempt
                            // settled elsewhere — exit without touching
                            // anything.
                            if fds[1].revents().is_some_and(|events| !events.is_empty()) {
                                return;
                            }
                            if fds[0].revents().is_some_and(|events| !events.is_empty()) {
                                break;
                            }
                        }
                        Err(nix::errno::Errno::EINTR) => {}
                        Err(_) => break,
                    }
                }
                owner.discard_pending(attempt);
            })
            .inspect_err(|_| {
                // A watchdog that cannot spawn leaves an unbounded
                // pending channel: discard the attempt it was for.
                cleanup.discard_pending(attempt);
            })
            .map(|_| ())
    }

    fn inner(&self) -> MutexGuard<'_, Inner> {
        self.inner.inner()
    }
}

/// The outcome of one accepted connection's paired handshake.
///
/// A `Pending` data half is real and held — its challenge was
/// delivered — but the pair is not live: the caller keeps accepting
/// for the control lane inside the same absolute budget.
#[allow(clippy::large_enum_variant)] // one accept per handshake — size is irrelevant
pub enum ChannelAccept {
    /// The data half validated and its challenge was delivered; the
    /// held channel awaits the pairing control lane.
    Pending,
    /// The pair committed: the live connection, dispatchable.
    Live(LocalConnection),
    /// The peer was refused; the typed reject was delivered on the wire.
    Rejected(Reject),
}

/// The data half a committed pair releases — the held channel plus the
/// reserved identity it was validated under.
struct PairCommit {
    /// The held data channel, dispatchable now.
    channel: RecordChannel,
    /// The data lane's reserved connection identity.
    connection: ConnectionId,
    /// The capabilities the data Hello claimed.
    capabilities: BTreeSet<Capability>,
    /// The shared admission both lanes' replies carry.
    admission: Admission,
    /// The data lane's armed pairing deadline — carried through so the
    /// control lane's admission delivery is clamped to
    /// `min(first, second)` and a late second half can never stretch the
    /// one bounded budget.
    deadline: Instant,
}

/// The shared state behind the cloneable [`ControlHandle`]: the bounded
/// command queue, the one in-flight command awaiting its receipt, and
/// the terminal error every orphan waiter is resolved with.
struct ControlShared {
    state: Mutex<ControlState>,
}

struct ControlState {
    /// Queued commands not yet delivered to the peer — FIFO, one per
    /// answered poll.
    queue: VecDeque<ControlCommand>,
    /// The command the last turn delivered, awaiting its receipt.
    inflight: Option<ControlCommand>,
    /// The worker's stop handle — set once at spawn.
    stop: Option<ChannelShutdown>,
    /// Whether [`ControlHandle::stop`] requested an orderly end — the
    /// worker exits without detaching the pair.
    stop_requested: bool,
    /// The terminal error new and orphan waiters resolve to once the
    /// lane is dead.
    closed: Option<io::Error>,
}

/// One queued or in-flight server command and its private waiter.
struct ControlCommand {
    request: ControlRequest,
    responder: mpsc::Sender<io::Result<ControlReply>>,
}

impl ControlShared {
    fn new() -> Self {
        Self {
            state: Mutex::new(ControlState {
                queue: VecDeque::new(),
                inflight: None,
                stop: None,
                stop_requested: false,
                closed: None,
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, ControlState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Queues `request` and returns the receipt future — refused
    /// immediately when the bounded queue is full or the lane is dead.
    fn enqueue(
        &self,
        request: ControlRequest,
    ) -> io::Result<mpsc::Receiver<io::Result<ControlReply>>> {
        let (responder, receipt) = mpsc::channel();
        let mut state = self.lock();
        if let Some(closed) = &state.closed {
            return Err(io::Error::new(closed.kind(), "control lane is closed"));
        }
        let in_flight = state.queue.len() + usize::from(state.inflight.is_some());
        if in_flight >= MAX_CONTROL_COMMANDS {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "control command budget is full",
            ));
        }
        state.queue.push_back(ControlCommand { request, responder });
        Ok(receipt)
    }

    /// Delivers the poll's receipt to the in-flight waiter. Every legal
    /// shape is checked against the command it answers: a receipt with
    /// no in-flight command, a missing receipt for a delivered one, or a
    /// receipt the wrong shape or echo for its request is a protocol
    /// violation that fails the lane, not just the waiter.
    fn resolve(&self, receipt: Option<ControlReply>) -> bool {
        let mut state = self.lock();
        match (state.inflight.take(), receipt) {
            (None, None) => true,
            (Some(command), Some(reply)) => {
                if receipt_matches(&command.request, &reply) {
                    let _ = command.responder.send(Ok(reply));
                    true
                } else {
                    // The dishonest receipt still consumes the command:
                    // the waiter learns the lane failed, never a false
                    // success.
                    let _ = command.responder.send(Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "control receipt does not match its request",
                    )));
                    false
                }
            }
            (Some(command), None) => {
                let _ = command.responder.send(Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "control poll carries no receipt for a delivered request",
                )));
                false
            }
            (None, Some(_)) => false,
        }
    }

    /// Moves the next queued command into the in-flight slot — only
    /// while nothing is in flight: at most one server command is ever
    /// outstanding on the lane.
    fn next_command(&self) -> Option<ControlRequest> {
        let mut state = self.lock();
        if state.inflight.is_some() {
            return None;
        }
        state.inflight = state.queue.pop_front();
        state
            .inflight
            .as_ref()
            .map(|command| command.request.clone())
    }

    /// Arms the worker's stop handle — once, at spawn.
    fn arm_stop(&self, stop: ChannelShutdown) {
        self.lock().stop = Some(stop);
    }

    /// Orderly stop: the worker exits without detaching the pair —
    /// the pair's own teardown is the caller's. Every waiter resolves
    /// closed and the worker's read wakes.
    fn close(&self, error: io::Error) {
        let mut state = self.lock();
        state.stop_requested = true;
        Self::drain(&mut state, error);
        if let Some(stop) = state.stop.take() {
            stop.shutdown();
        }
    }

    /// The worker's teardown: resolve every orphan waiter — commands
    /// never delivered and the in-flight one never answered — then mark
    /// the lane closed so late callers fail instead of queueing. This
    /// is never an orderly stop: the pair's fate is decided by the
    /// caller that observes the lane ended.
    fn terminate(&self, error: io::Error) {
        let mut state = self.lock();
        Self::drain(&mut state, error);
    }

    /// Resolves every outstanding waiter with `error` and marks the
    /// lane closed — under the caller's existing hold.
    fn drain(state: &mut ControlState, error: io::Error) {
        state.closed.get_or_insert(error);
        for command in state.queue.drain(..).chain(state.inflight.take()) {
            let _ = command.responder.send(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "control lane is closed",
            )));
        }
    }

    /// Whether the shutdown was requested through [`Self::close`].
    fn stop_requested(&self) -> bool {
        self.lock().stop_requested
    }
}

/// The cloneable Server command path on the paired control lane —
/// independent of the data lane's dispatch lock. `health` and
/// `cancel_mutation` queue bounded commands delivered on the peer's own
/// turn-taking polls and return only verified receipts;
/// the lane ends with the connection's own lifecycle teardown. Every
/// clone shares the one lane: at
/// most [`MAX_CONTROL_COMMANDS`] queued plus in-flight commands, probes
/// included.
#[derive(Clone)]
pub struct ControlHandle {
    shared: Arc<ControlShared>,
}

impl ControlHandle {
    /// A liveness round trip on the control lane: the fresh nonce must
    /// come back echoed — never a replayed proof.
    ///
    /// # Errors
    ///
    /// Returns the queue-full [`io::ErrorKind::WouldBlock`], the closed
    /// lane's terminal error, [`io::ErrorKind::TimedOut`] past the
    /// command budget, or the worker's protocol-violation error.
    pub fn health(&self) -> io::Result<()> {
        let mut nonce = [0u8; 8];
        getrandom::fill(&mut nonce).map_err(io::Error::other)?;
        let nonce = DecimalU64::new(u64::from_be_bytes(nonce));
        let receipt = self.shared.enqueue(ControlRequest::Health { nonce })?;
        match receipt.recv_timeout(CONTROL_COMMAND_TIMEOUT) {
            Ok(Ok(ControlReply::Healthy { nonce: echoed })) if echoed == nonce => Ok(()),
            Ok(Ok(_)) => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "control health receipt echoes a foreign nonce",
            )),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "control command outlived its budget",
            )),
        }
    }

    /// Cancels the live mutation commit for `ticket` on the Client:
    /// the disposition is the Client's own arbitration — a
    /// [`CancelDisposition::CancelledBeforeCommit`] proves the publish
    /// never crossed, [`CancelDisposition::AlreadyCrossed`] that it had,
    /// and [`CancelDisposition::Unknown`] that no live ticket matched.
    /// `Unknown` is returned as-is — never retried.
    ///
    /// # Errors
    ///
    /// The same error kinds as [`Self::health`].
    pub fn cancel_mutation(&self, ticket: DecimalU64) -> io::Result<CancelDisposition> {
        let receipt = self
            .shared
            .enqueue(ControlRequest::CancelMutation { ticket })?;
        match receipt.recv_timeout(CONTROL_COMMAND_TIMEOUT) {
            Ok(Ok(ControlReply::MutationCancelled {
                ticket: echoed,
                disposition,
            })) if echoed == ticket => Ok(disposition),
            Ok(Ok(_)) => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "control cancel receipt answers a foreign ticket",
            )),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "control command outlived its budget",
            )),
        }
    }

    /// Cancels one live process operation on the Client by its complete
    /// operation key: the disposition is the Client's own arbitration —
    /// [`ProcessCancelDisposition::CancelledBeforeStart`] proves zero
    /// business spawn, [`ProcessCancelDisposition::CancellationRequested`]
    /// that containment is physically underway (never already complete),
    /// and `Unknown` that no record matched the exact key.
    ///
    /// # Errors
    ///
    /// The same error kinds as [`Self::health`].
    pub fn cancel_process(
        &self,
        key: &rustx_execution_protocol::process::ProcessOperationKey,
    ) -> io::Result<ProcessCancelDisposition> {
        self.cancel_process_by(key, Instant::now() + CONTROL_COMMAND_TIMEOUT)
    }

    /// [`Self::cancel_process`] under one absolute `deadline` covering both
    /// the queue wait and the correlated receipt. Only a refusal the queue
    /// returns before anything is queued — the bounded budget being full — is
    /// retried, paced inside the deadline; a dead lane fails at once. A
    /// budget still full at the deadline is an explicit
    /// [`io::ErrorKind::WouldBlock`]: the command was never queued.
    ///
    /// # Errors
    ///
    /// [`io::ErrorKind::WouldBlock`] when the budget stayed full to the
    /// deadline, the closed lane's terminal error, or
    /// [`io::ErrorKind::TimedOut`] when the receipt missed the deadline.
    pub fn cancel_process_by(
        &self,
        key: &rustx_execution_protocol::process::ProcessOperationKey,
        deadline: Instant,
    ) -> io::Result<ProcessCancelDisposition> {
        let receipt = self.enqueue_until(
            &ControlRequest::CancelProcess { key: key.clone() },
            deadline,
        )?;
        match receipt.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(Ok(ControlReply::ProcessCancelled {
                key: echoed,
                disposition,
            })) if echoed == *key => Ok(disposition),
            Ok(Ok(_)) => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "control cancel receipt answers a foreign process key",
            )),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "control command outlived its budget",
            )),
        }
    }

    fn enqueue_until(
        &self,
        request: &ControlRequest,
        deadline: Instant,
    ) -> io::Result<mpsc::Receiver<io::Result<ControlReply>>> {
        loop {
            let now = Instant::now();
            if now >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "control budget stayed full to the deadline",
                ));
            }
            match self.shared.enqueue(request.clone()) {
                Ok(receipt) => return Ok(receipt),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5).min(deadline - now));
                }
                Err(error) => return Err(error),
            }
        }
    }

    /// Retire one exact native file observer without acquiring the data lock.
    ///
    /// # Errors
    ///
    /// A lost, late, or negative receipt leaves physical cleanup unconfirmed.
    pub fn stop_watch(&self, id: WatchId) -> io::Result<()> {
        let deadline = Instant::now() + CONTROL_COMMAND_TIMEOUT;
        let receipt = self.enqueue_until(&ControlRequest::StopWatch { id }, deadline)?;
        match receipt.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(Ok(ControlReply::WatchStopped {
                id: echoed,
                settled: true,
            })) if echoed == id => Ok(()),
            Ok(Err(error)) => Err(error),
            Ok(Ok(_)) => Err(io::Error::other("native watch cleanup was not confirmed")),
            Err(_) => Err(io::ErrorKind::TimedOut.into()),
        }
    }

    /// Requests terminal revocation from the Client. Its local commits
    /// are fenced before `Revoked` is echoed. The connection owner
    /// applies the local binding fence and closes the data transport.
    ///
    /// # Errors
    ///
    /// The same error kinds as [`Self::health`]. A failed request leaves
    /// Client settlement unconfirmed, so the owner must still fence.
    pub fn request_revoke(&self) -> io::Result<()> {
        let receipt = self.shared.enqueue(ControlRequest::Revoke);
        match receipt {
            Ok(receipt) => match receipt.recv_timeout(CONTROL_COMMAND_TIMEOUT) {
                Ok(Ok(ControlReply::Revoked)) => Ok(()),
                Ok(Ok(_)) => Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "control revoke acknowledged with a foreign receipt",
                )),
                Ok(Err(error)) => Err(error),
                Err(_) => Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "control command outlived its budget",
                )),
            },
            Err(error) => Err(error),
        }
    }

    /// Whether the lane is still delivering commands.
    #[must_use]
    pub fn is_live(&self) -> bool {
        self.shared.lock().closed.is_none()
    }

    /// Orderly stop: waiters resolve closed and the worker's read wakes.
    /// The pair's teardown is the connection's.
    fn stop(&self) {
        self.shared.close(io::Error::new(
            io::ErrorKind::BrokenPipe,
            "control lane is closed",
        ));
    }
}

/// The control worker's ownership: the handle for callers and the
/// bounded join for teardown.
struct ControlWorker {
    handle: ControlHandle,
    join: JoinHandle<()>,
}

impl ControlWorker {
    /// Spawns the one bounded worker that owns the committed control
    /// channel: it reads [`ControlPoll`]s, resolves the in-flight
    /// command's waiter from each receipt, and answers with
    /// [`ControlTurn`]s carrying the next queued command. Every frame
    /// runs under a per-frame absolute deadline; the lane's own
    /// shutdown handle wakes a blocked read on stop or fence.
    fn spawn(
        mut channel: RecordChannel,
        owner: Arc<ConnectionOwner>,
        connection: ConnectionId,
        shared: Arc<ControlShared>,
    ) -> Self {
        if let Ok(stop) = channel.shutdown_handle() {
            shared.arm_stop(stop);
        }
        let worker_shared = shared.clone();
        let worker_owner = owner;
        let join = std::thread::Builder::new()
            .name("execution-control".to_owned())
            .spawn(move || {
                let mut next_sequence = 1u64;
                loop {
                    let deadline = Instant::now() + CONTROL_FRAME_TIMEOUT;
                    let poll: ControlPoll = match channel.receive(Some(deadline)) {
                        Ok(poll) => poll,
                        Err(_) => break,
                    };
                    // The client's own sequence must advance exactly:
                    // a skip or replay is a protocol violation, not a
                    // dropped frame to tolerate.
                    if poll.sequence.get() != next_sequence {
                        break;
                    }
                    next_sequence = next_sequence.saturating_add(1);
                    if !shared.resolve(poll.receipt) {
                        break;
                    }
                    let request = shared.next_command();
                    let turn = ControlTurn {
                        sequence: poll.sequence,
                        health: poll.health,
                        request,
                    };
                    let Ok(encoded) = encode_record(&turn) else {
                        break;
                    };
                    if channel.send(&encoded, Some(deadline)).is_err() {
                        break;
                    }
                }
                shared.terminate(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "control lane ended",
                ));
                if !shared.stop_requested() {
                    // A lane that ended without an orderly stop detaches
                    // the pair: the seized data handle interrupts any
                    // blocked exchange, and no dispatch starts again.
                    worker_owner.detach_control(&connection).shutdown();
                }
            })
            .expect("the bounded control worker spawns");
        Self {
            handle: ControlHandle {
                shared: worker_shared,
            },
            join,
        }
    }

    /// Orderly stop: close the lane, wake the worker, join it. The join
    /// is bounded — the worker's own frame deadline plus the shutdown
    /// it just received — and the caller never holds a global lock.
    fn stop(self) {
        self.handle.stop();
        let _ = self.join.join();
    }
}

/// Whether a poll receipt is the exact answer the in-flight request
/// requires — shape and echo both.
fn receipt_matches(request: &ControlRequest, reply: &ControlReply) -> bool {
    match (request, reply) {
        (ControlRequest::Health { nonce }, ControlReply::Healthy { nonce: echoed }) => {
            nonce == echoed
        }
        (
            ControlRequest::CancelMutation { ticket },
            ControlReply::MutationCancelled { ticket: echoed, .. },
        ) => ticket == echoed,
        (
            ControlRequest::CancelProcess { key },
            ControlReply::ProcessCancelled { key: echoed, .. },
        ) => key == echoed,
        (ControlRequest::Revoke, ControlReply::Revoked) => true,
        (ControlRequest::StopWatch { id }, ControlReply::WatchStopped { id: echoed, .. }) => {
            id == echoed
        }
        _ => false,
    }
}

/// One admitted local connection. Dropping it disconnects the binding.
///
/// The only flows are typed exchanges the Server initiates: [`ping`](Self::ping)
/// liveness and [`read_range`](Self::read_range), the one implemented physical
/// operation. There is no raw message path. A transport, protocol, or
/// unexpected-reply failure retires the stream: it is never reused while a
/// reply could still be outstanding, and later calls fail closed.
pub struct LocalConnection {
    owner: Arc<ConnectionOwner>,
    /// The data lane — the only dispatch channel.
    channel: RecordChannel,
    /// The paired control lane's worker, stopped exactly once.
    control: Option<ControlWorker>,
    usable: bool,
    /// Whether the orderly transport close was already sent: `close`
    /// and `Drop` never send it twice.
    closed: bool,
    capabilities: BTreeSet<Capability>,
    connection: ConnectionId,
    admission: Admission,
}

/// A verified range reply from the Client.
///
/// The bytes were decoded and checked against the request bounds before
/// this value exists — the reply's own numbers are never trusted raw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RangeRead {
    /// The returned bytes, already base64-decoded and bounded.
    pub bytes: Vec<u8>,
    /// The offset the bytes start at; equals the requested offset.
    pub offset: u64,
    /// The next offset to read, absent at end of file.
    pub next_offset: Option<u64>,
    /// The source-descriptor stamp of the file the bytes came from.
    pub metadata: FileStamp,
}

/// A verified directory page reply from the Client.
///
/// Names, order, cursor, and every entry stamp were checked against the
/// request bounds before this value exists — the reply's own claims are
/// never trusted raw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryListing {
    /// The revision the listing was verified under.
    pub revision: String,
    /// The page's entries in stable name order.
    pub entries: Vec<PageEntry>,
    /// The index the next page starts at, absent at the end.
    pub next_offset: Option<u32>,
}

/// The result of one bounded Bash continuation query.
pub enum BashOutputPage {
    /// A checked logical-text page.
    Lines(LinesPage),
    /// A checked search page.
    Search(SearchResult),
}

/// A verified logical-text page reply from the Client.
///
/// The page's shown text, line and byte accounting, end reason, and
/// source stamp were checked against the request and the scanner's
/// complete-line contract before this value exists — the reply's own
/// claims are never trusted raw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinesPage {
    /// The shown text: the page's joined lines, bounded.
    pub text: String,
    /// The addressable line count of the whole source.
    pub total_lines: u64,
    /// The shown element count of this page.
    pub shown_lines: u32,
    /// Why the page ended.
    pub end: LineEnd,
    /// The source-descriptor stamp of the file the page came from.
    pub metadata: FileStamp,
}

/// A verified document page reply from the Client.
///
/// `source` stamps the physical document the page came from and
/// `logical_bytes` is the full decoded Markdown length — independent of
/// the compressed source's size. The page's shown text, line and byte
/// accounting, and end reason passed the same complete-line contract a
/// `Lines` reply answers to, so the reply's own claims are never trusted
/// raw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentPage {
    /// The source-descriptor stamp of the document the page came from.
    pub source: FileStamp,
    /// The byte length of the complete decoded Markdown text.
    pub logical_bytes: u64,
    /// The shown text: the page's joined lines, bounded.
    pub text: String,
    /// The addressable line count of the whole decoded text.
    pub total_lines: u64,
    /// The shown element count of this page.
    pub shown_lines: u32,
    /// Why the page ended.
    pub end: LineEnd,
}

/// A verified bounded-search reply from the Client.
///
/// `body` is the selected record text — newline-joined records only,
/// never notices — and `matches` the primary record count. The reply's
/// own bounds and accounting were checked against the request before
/// this value exists; record display bytes are never parsed back for
/// authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResult {
    /// The selected records, at most `MAX_TEXT_BYTES` joined bytes.
    pub body: String,
    /// The selected primary record count.
    pub matches: u64,
    /// At least one match was excluded past the requested bound.
    pub hit_limit: bool,
    /// The joined-body byte budget stopped the selection.
    pub byte_limit: bool,
    /// At least one record preview was shortened — grep only.
    pub short_lines: bool,
}

impl LocalConnection {
    /// The admission the peer was sent.
    #[must_use]
    pub fn admission(&self) -> &Admission {
        &self.admission
    }

    /// The server-generated identity of this connection.
    #[must_use]
    pub fn connection_id(&self) -> &ConnectionId {
        &self.connection
    }

    /// The permission gate, linearized under the binding mutex before any
    /// request byte is written: dispatch proceeds only while this exact
    /// connection is still the live one. A revocation or detach that wins
    /// the mutex first denies the exchange — no request is ever sent.
    fn gate(&self) -> io::Result<()> {
        if !self.usable {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "connection retired after a protocol or transport failure",
            ));
        }
        let inner = self.owner.inner();
        if inner.phase == BindingPhase::Live && inner.connection.as_ref() == Some(&self.connection)
        {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "binding is not live on this connection",
            ))
        }
    }

    /// Orderly teardown: the transport's graceful close — a real
    /// RFC 6455 Close frame sent and flushed under the teardown budget,
    /// or the Unix stream's `shutdown(2)` — then the binding's live
    /// record disconnects and the channel is shut down. There is no
    /// wait for the peer's answering Close. Unlike `retire` the
    /// connection is not marked failed: a later dispatch still meets
    /// the binding's typed `PermissionDenied` denial, never a bare
    /// retired-connection error. Idempotent: an already orderly-closed
    /// connection only disconnects and shuts down.
    pub fn close(&mut self) {
        if self.usable && !self.closed {
            self.closed = true;
            let _ = self
                .channel
                .close_graceful(Some(Instant::now() + ORDERLY_CLOSE_BUDGET));
        }
        if let Some(control) = self.control.take() {
            control.stop();
        }
        self.owner.disconnect(&self.connection).shutdown();
        self.channel.shutdown();
    }

    /// The paired control lane's cloneable command handle — `None`
    /// after teardown takes the worker.
    #[must_use]
    pub fn control(&self) -> Option<ControlHandle> {
        self.control.as_ref().map(|control| control.handle.clone())
    }

    /// Marks the stream unusable, disconnects the binding, and shuts the
    /// socket down: after a protocol violation, truncation, or unexpected
    /// reply the peer's response accounting is unknowable, so the
    /// connection is torn down and never used again.
    fn retire(&mut self, error: io::Error) -> io::Error {
        self.usable = false;
        if let Some(control) = self.control.take() {
            control.stop();
        }
        self.owner.disconnect(&self.connection).shutdown();
        self.channel.shutdown();
        error
    }

    /// Tears the connection down without a protocol verdict: the binding
    /// is disconnected and the socket shut, so the Client's drop clears
    /// every ticketed preparation it still holds. This is the fail-closed
    /// answer when a cleanup abort cannot reach the peer — the Session
    /// honestly loses execution rather than stranding bounded
    /// preparations while advertising a healthy binding.
    pub fn disconnect(&mut self) {
        let _ = self.retire(io::Error::other(
            "connection retired to clear stranded preparations",
        ));
    }

    /// Sends exactly one typed request and consumes its one typed reply.
    /// The request is encoded before the gate so an unencodable request
    /// fails without touching the binding or the wire; any transport
    /// failure after the gate retires the connection.
    fn exchange(&mut self, request: &ClientRequest) -> io::Result<ClientReply> {
        self.exchange_admitted(request, None, || Ok(()), || ())
    }

    /// `exchange` with an optional absolute deadline and the caller's
    /// acceptance gate. The request is encoded and the binding's live
    /// check passes before `admit` runs: a refused admission — a
    /// cancelled caller — returns its error before a single request byte
    /// is written and never retires the connection. `sent` runs once the
    /// whole request frame is on the wire, before the reply is read.
    /// With `deadline`, the write and the read share one channel armed to
    /// the same absolute `Instant`; a timeout, partial reply, or any
    /// transport failure after admission retires the connection — the
    /// reply accounting is then unknowable, so there are no retries.
    fn exchange_admitted(
        &mut self,
        request: &ClientRequest,
        deadline: Option<Instant>,
        admit: impl FnOnce() -> io::Result<()>,
        sent: impl FnOnce(),
    ) -> io::Result<ClientReply> {
        let encoded = encode_record(request)?;
        self.gate()?;
        admit()?;
        let result = self
            .channel
            .send(&encoded, deadline)
            .map(|()| sent())
            .and_then(|()| self.channel.receive(deadline));
        match result {
            Ok(reply) => Ok(reply),
            Err(error) => Err(self.retire(error)),
        }
    }

    /// A liveness round-trip: `Ping` is answered by `Pong`.
    ///
    /// # Errors
    ///
    /// Returns an error when the binding no longer owns this connection, the
    /// exchange fails, or the reply is not a `Pong`; failures retire the
    /// stream.
    pub fn ping(&mut self) -> io::Result<()> {
        match self.exchange(&ClientRequest::Ping)? {
            ClientReply::Pong => Ok(()),
            _ => Err(self.retire(io::Error::new(
                io::ErrorKind::InvalidData,
                "unexpected reply to ping",
            ))),
        }
    }

    /// The one physical operation: a bounded, revision-checked range read of
    /// `relative_path` under the binding workspace.
    ///
    /// The peer must have advertised [`Capability::ReadRange`] at admission
    /// or the call is [`io::ErrorKind::Unsupported`] and no request is sent.
    /// `max_bytes` must be within `1..=`[`MAX_RAW_CHUNK_BYTES`]. The reply is
    /// trusted only after validation: the base64 must decode to at most
    /// `max_bytes` (its encoded length is checked before decoding), the
    /// offset must echo the request, the revision must be a 64-character
    /// lowercase hexadecimal stamp, a `next_offset` must continue the range
    /// exactly (and then the full bound was delivered), and a supplied
    /// `expected_revision` must match the returned revision. A violation
    /// retires the stream — it is never reused with a reply accounting that
    /// cannot be trusted.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::Unsupported`] when the peer lacks the range
    /// capability, [`io::ErrorKind::InvalidInput`] for an out-of-range
    /// request, the [`ReadError`] the Client reported mapped onto an I/O
    /// error kind, or [`io::ErrorKind::InvalidData`] for a malformed or
    /// untruthful reply.
    pub fn read_range(
        &mut self,
        relative_path: &Path,
        offset: u64,
        max_bytes: u32,
        expected_revision: Option<&str>,
    ) -> io::Result<RangeRead> {
        self.read_range_admitted(
            relative_path,
            offset,
            max_bytes,
            expected_revision,
            Instant::now() + PAGE_REQUEST_TIMEOUT,
            || Ok(()),
            || (),
        )
    }

    /// `read_range` against a caller-armed absolute deadline and
    /// acceptance gate, under the same contract as
    /// [`read_lines_admitted`](Self::read_lines_admitted): one wire
    /// budget, an admission refusal that returns before any request byte
    /// without retiring, and a `sent` observation between the written
    /// frame and the reply.
    ///
    /// # Errors
    ///
    /// Returns the same kinds as [`read_range`](Self::read_range), plus
    /// the `admit` error verbatim and [`io::ErrorKind::TimedOut`] when
    /// the deadline lapses mid-exchange — which retires the connection.
    #[allow(clippy::too_many_arguments)] // one admitted exchange's full context
    pub fn read_range_admitted(
        &mut self,
        relative_path: &Path,
        offset: u64,
        max_bytes: u32,
        expected_revision: Option<&str>,
        deadline: Instant,
        admit: impl FnOnce() -> io::Result<()>,
        sent: impl FnOnce(),
    ) -> io::Result<RangeRead> {
        if !self.capabilities.contains(&Capability::ReadRange) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "peer admitted no range capability",
            ));
        }
        let bound = usize::try_from(max_bytes).unwrap_or(usize::MAX);
        if !(1..=MAX_RAW_CHUNK_BYTES).contains(&bound) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "range read exceeds chunk limit",
            ));
        }
        let reply = self.exchange_admitted(
            &ClientRequest::ReadRange {
                relative_path: relative_path.to_path_buf(),
                offset: DecimalU64::new(offset),
                max_bytes,
                expected_revision: expected_revision.map(str::to_owned),
            },
            Some(deadline),
            admit,
            sent,
        )?;
        if let ClientReply::Error { kind } = reply {
            return Err(read_error(kind));
        }
        self.validate_range(offset, bound, expected_revision, &reply)
    }

    /// One bounded whole-file fetch assembled from `ReadRange` chunks on
    /// this connection — the physical backing of the Agent's image
    /// transfer. `admit` is the caller's acceptance gate run after each
    /// request is encoded and the binding's live check passes, before
    /// that request's first byte; every chunk shares the one absolute
    /// `deadline`.
    ///
    /// The first chunk's stamp is pinned whole — `size`, `identity`,
    /// `revision`, `mtime` — and its `size` must not exceed `bound`
    /// before a single follow-up request is issued: an oversized source
    /// is an honest [`io::ErrorKind::FileTooLarge`] refusal that never
    /// retires the connection, exactly like a typed `Stale` or
    /// `conflict` from the peer. Every later request names the pinned
    /// revision; a reply stamped differently — or assembled bytes whose
    /// total contradicts the pinned size — means the peer cannot
    /// account for its answers, which retires the stream. At most
    /// `ceil(bound / MAX_RAW_CHUNK_BYTES)` requests are ever made for
    /// one call; there are no retries.
    ///
    /// # Errors
    ///
    /// Returns the same kinds as [`read_range`](Self::read_range), plus
    /// [`io::ErrorKind::FileTooLarge`] for a source larger than `bound`
    /// and the `admit` error verbatim.
    pub fn read_image(
        &mut self,
        relative_path: &Path,
        bound: u64,
        deadline: Instant,
        admit: impl Fn() -> io::Result<()>,
    ) -> io::Result<Vec<u8>> {
        let violation = |message: &'static str| io::Error::new(io::ErrorKind::InvalidData, message);
        let limit = usize::try_from(bound).unwrap_or(usize::MAX);
        let chunk_bound = u32::try_from(MAX_RAW_CHUNK_BYTES).unwrap_or(u32::MAX);
        let first =
            self.read_range_admitted(relative_path, 0, chunk_bound, None, deadline, &admit, || ())?;
        let stamp = first.metadata;
        let size = stamp.size.get();
        if size > bound {
            return Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                "image source exceeds the transfer bound",
            ));
        }
        let mut bytes = first.bytes;
        let mut next = first.next_offset;
        while let Some(offset) = next {
            let chunk = self.read_range_admitted(
                relative_path,
                offset,
                chunk_bound,
                Some(&stamp.revision),
                deadline,
                &admit,
                || (),
            )?;
            if chunk.metadata != stamp {
                return Err(self.retire(violation("image source stamp changed between chunks")));
            }
            if bytes.len() + chunk.bytes.len() > limit {
                return Err(self.retire(violation("image chunks exceed the stamped bound")));
            }
            bytes.extend_from_slice(&chunk.bytes);
            next = chunk.next_offset;
        }
        if bytes.len() as u64 != size {
            return Err(self.retire(violation("image chunks contradict the stamped size")));
        }
        Ok(bytes)
    }

    /// One managed-process exchange: the typed request is sent and its
    /// typed reply returned only after the exact key echo and the
    /// bounded wire shapes are verified — an `Output` page's encoded
    /// payload is length-checked against `MAX_PROCESS_DATA_BYTES`
    /// before the reply is trusted, and a reply answering any other key
    /// or shape retires the stream.
    ///
    /// The peer must have advertised [`Capability::Process`] at
    /// admission or the call is [`io::ErrorKind::Unsupported`] and no
    /// request is sent. Each call is one complete request/reply pair —
    /// the caller observes operation progress through successive
    /// `Query`/`OutputPage`/`InputReceipt` exchanges, so no exchange
    /// ever awaits a process lifetime.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::Unsupported`] when the peer lacks the
    /// process capability, [`io::ErrorKind::InvalidData`] for a reply
    /// that fails the key echo or the bounded-shape checks — which
    /// retires the stream — or the transport's own errors.
    pub fn process(
        &mut self,
        request: &rustx_execution_protocol::process::ProcessRequest,
    ) -> io::Result<rustx_execution_protocol::process::ProcessReply> {
        self.process_until(request, Instant::now() + PAGE_REQUEST_TIMEOUT)
    }

    /// [`Self::process`] under one absolute `deadline` covering the wire
    /// exchange. A deadline already passed refuses before anything is sent.
    ///
    /// # Errors
    ///
    /// As [`Self::process`], plus [`io::ErrorKind::TimedOut`] for a deadline
    /// that passed before the request was sent.
    pub fn process_until(
        &mut self,
        request: &rustx_execution_protocol::process::ProcessRequest,
        deadline: Instant,
    ) -> io::Result<rustx_execution_protocol::process::ProcessReply> {
        self.process_admitted(request, deadline, || Ok(()))
    }

    /// One process exchange with a cancellation gate immediately before send.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::process_until`], or the gate's refusal.
    pub fn process_admitted(
        &mut self,
        request: &rustx_execution_protocol::process::ProcessRequest,
        deadline: Instant,
        admit: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<rustx_execution_protocol::process::ProcessReply> {
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "process deadline passed before dispatch",
            ));
        }
        if !self.capabilities.contains(&Capability::Process) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "peer admitted no process capability",
            ));
        }
        if matches!(
            request,
            rustx_execution_protocol::process::ProcessRequest::RegisterAgentBash { .. }
        ) && !self.capabilities.contains(&Capability::AgentBash)
        {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "peer admitted no Agent Bash capability",
            ));
        }
        process_reply::validate_request(request)
            .map_err(|message| io::Error::new(io::ErrorKind::InvalidInput, message))?;
        let ClientReply::Process(reply) = self.exchange_admitted(
            &ClientRequest::Process(request.clone()),
            Some(deadline),
            admit,
            || (),
        )?
        else {
            return Err(self.retire(io::Error::new(
                io::ErrorKind::InvalidData,
                "unexpected reply to process request",
            )));
        };
        process_reply::validate(request, &reply)
            .map_err(|message| self.retire(io::Error::new(io::ErrorKind::InvalidData, message)))?;
        Ok(reply)
    }

    /// Run one bounded Read or Grep over the authorized Bash continuation.
    ///
    /// # Errors
    ///
    /// Returns capability, cancellation, transport or typed read/search errors.
    pub fn inspect_bash_output(
        &mut self,
        request: &ClientRequest,
        deadline: Instant,
        admit: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<BashOutputPage> {
        if !self.capabilities.contains(&Capability::AgentBash) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "peer admitted no Agent Bash capability",
            ));
        }
        if !matches!(
            request,
            ClientRequest::ReadBashOutput { .. }
                | ClientRequest::SearchBashOutput {
                    spec: SearchSpec::Grep { .. },
                    ..
                }
        ) {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        let reply = self.exchange_admitted(request, Some(deadline), admit, || ())?;
        if let ClientReply::Error { kind } = reply {
            return Err(read_error(kind));
        }
        match request {
            ClientRequest::ReadBashOutput { offset, limit, .. } => self
                .validate_lines(offset.get(), limit.map(DecimalU64::get), None, &reply)
                .map(BashOutputPage::Lines),
            ClientRequest::SearchBashOutput {
                spec: SearchSpec::Grep { context, limit, .. },
                ..
            } => self
                .validate_search(false, *context, *limit, &reply)
                .map(BashOutputPage::Search),
            _ => unreachable!("Bash output exchange must name Read or Grep"),
        }
    }

    /// Verifies a `Range` reply against the request before trusting it. Any
    /// deviation means the peer cannot account for its answers honestly, so
    /// the connection is retired.
    fn validate_range(
        &mut self,
        offset: u64,
        bound: usize,
        expected_revision: Option<&str>,
        reply: &ClientReply,
    ) -> io::Result<RangeRead> {
        let violation = |message: &'static str| io::Error::new(io::ErrorKind::InvalidData, message);
        let ClientReply::Range {
            data_base64,
            offset: reply_offset,
            next_offset,
            metadata,
        } = reply
        else {
            return Err(self.retire(violation("unexpected reply to range read")));
        };
        // The encoding inflates by 4/3: anything longer cannot decode to
        // a permitted range, so it is refused before decoding.
        if data_base64.len() > bound.div_ceil(3) * 4 {
            return Err(self.retire(violation("range reply exceeds requested bound")));
        }
        let bytes = match base64::engine::general_purpose::STANDARD.decode(data_base64) {
            Ok(bytes) if bytes.len() <= bound => bytes,
            _ => return Err(self.retire(violation("range reply exceeds requested bound"))),
        };
        if reply_offset.get() != offset {
            return Err(self.retire(violation("range reply offset does not echo request")));
        }
        if let Some(reason) = stamp_violation(metadata) {
            return Err(self.retire(violation(reason)));
        }
        if let Some(expected) = expected_revision
            && metadata.revision != expected
        {
            return Err(self.retire(violation("range reply revision ignores expectation")));
        }
        let Ok(length) = u64::try_from(bytes.len()) else {
            return Err(self.retire(violation("range length")));
        };
        let Some(end) = offset.checked_add(length) else {
            return Err(self.retire(violation("range reply overflows the offset space")));
        };
        let size = metadata.size.get();
        if offset > size
            || end > size
            || (*next_offset).map(DecimalU64::get) != (end < size).then_some(end)
            || (end < size && bytes.len() != bound)
        {
            return Err(self.retire(violation("range reply contradicts file size")));
        }
        Ok(RangeRead {
            bytes,
            offset,
            next_offset: (*next_offset).map(DecimalU64::get),
            metadata: metadata.clone(),
        })
    }

    /// Run one bounded tree mutation, subject to the final pre-wire authorization.
    ///
    /// # Errors
    ///
    /// Returns a clean error when the capability is missing, authorization fails,
    /// or the Client proves no named change. After admission, a transport failure
    /// or an uncertain reply returns [`WriteOutcomeUnknown`].
    pub fn mutate_tree(
        &mut self,
        mutation: TreeMutation,
        authorize: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<()> {
        if !self.capabilities.contains(&Capability::TreeMutations) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "peer admitted no tree mutation capability",
            ));
        }
        let admitted = std::cell::Cell::new(false);
        let reply = self.exchange_admitted(
            &ClientRequest::MutateTree(mutation),
            Some(Instant::now() + PAGE_REQUEST_TIMEOUT),
            || {
                authorize()?;
                admitted.set(true);
                Ok(())
            },
            || (),
        );
        match reply {
            Ok(ClientReply::TreeMutated) => Ok(()),
            Ok(ClientReply::Error { kind }) => Err(read_error(kind)),
            Ok(_) => Err(self.retire(io::Error::other(WriteOutcomeUnknown))),
            Err(_) if admitted.get() => Err(io::Error::other(WriteOutcomeUnknown)),
            Err(error) => Err(error),
        }
    }

    /// Register a descriptor watch under this Session's live binding.
    ///
    /// # Errors
    ///
    /// Refuses a missing capability, a pre-wire authorization denial or a
    /// typed physical refusal. An ambiguous registration retires the pair.
    pub fn register_watch(
        &mut self,
        id: WatchId,
        path: &Path,
        authorize: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<()> {
        if !self.capabilities.contains(&Capability::FileWatches) {
            return Err(io::ErrorKind::Unsupported.into());
        }
        let admitted = std::cell::Cell::new(false);
        let reply = self.exchange_admitted(
            &ClientRequest::Watch(WatchRequest::Register {
                id,
                path: path.to_path_buf(),
            }),
            Some(Instant::now() + PAGE_REQUEST_TIMEOUT),
            || {
                authorize()?;
                admitted.set(true);
                Ok(())
            },
            || (),
        );
        match reply {
            Ok(ClientReply::Watch(WatchReply::Registered { id: echoed })) if echoed == id => Ok(()),
            Ok(ClientReply::Watch(WatchReply::Refused { id: echoed, kind })) if echoed == id => {
                Err(read_error(kind))
            }
            Ok(_) => Err(self.retire(io::Error::other(WatchOutcomeUnknown))),
            Err(_) if admitted.get() => Err(io::Error::other(WatchOutcomeUnknown)),
            Err(error) => Err(error),
        }
    }

    /// Poll exactly one live native watcher for bounded, workspace-relative hints.
    ///
    /// # Errors
    ///
    /// The receipt must describe paths under the registered source. A malformed
    /// receipt or uncertain physical retirement retires the exact binding.
    pub fn poll_watch(&mut self, id: WatchId, source: &Path) -> io::Result<WatchChange> {
        let result = self.exchange_admitted(
            &ClientRequest::Watch(WatchRequest::Poll { id }),
            Some(Instant::now() + PAGE_REQUEST_TIMEOUT),
            || Ok(()),
            || (),
        )?;
        let notice = match result {
            ClientReply::Watch(WatchReply::Changed(notice)) if notice.id == id => notice,
            ClientReply::Watch(WatchReply::Refused { id: echoed, kind }) if echoed == id => {
                return Err(read_error(kind));
            }
            _ => {
                return Err(self.retire(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "watch receipt did not prove a change or retirement",
                )));
            }
        };
        let valid = notice.changed_paths.len() <= MAX_WATCH_HINT_PATHS
            && (!notice.retired || notice.resync_required && notice.changed_paths.is_empty())
            && notice.changed_paths.iter().all(|path| {
                !path.is_absolute()
                    && path.starts_with(source)
                    && path.to_str().is_some_and(|path| {
                        !path.bytes().any(|byte| byte == 0)
                            && !path.split('/').any(|part| part == "." || part == "..")
                    })
            })
            && notice
                .changed_paths
                .iter()
                .map(|p| p.as_os_str().len().saturating_mul(6) + 8)
                .sum::<usize>()
                <= MAX_WATCH_HINT_BYTES;
        if valid {
            Ok(notice)
        } else {
            Err(self.retire(io::Error::new(
                io::ErrorKind::InvalidData,
                "watch change exceeded its bounds or authority",
            )))
        }
    }

    /// The metadata query: `relative_path` names one entry under the
    /// binding workspace, stat'ed without following a final symlink.
    ///
    /// The peer must have advertised [`Capability::DirectoryMetadata`] at
    /// admission or the call is [`io::ErrorKind::Unsupported`] and no
    /// request is sent. The reply's stamp is trusted only after the shared
    /// `stamp_violation` validation; a malformed reply retires the
    /// stream.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::Unsupported`] when the peer lacks the
    /// capability, the [`ReadError`] the Client reported mapped onto an
    /// I/O error kind, or [`io::ErrorKind::InvalidData`] for a malformed
    /// reply.
    pub fn entry_metadata(&mut self, relative_path: &Path) -> io::Result<EntryStamp> {
        if !self.capabilities.contains(&Capability::DirectoryMetadata) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "peer admitted no directory metadata capability",
            ));
        }
        let reply = self.exchange(&ClientRequest::GetMetadata {
            relative_path: relative_path.to_path_buf(),
        })?;
        if let ClientReply::Error { kind } = reply {
            return Err(read_error(kind));
        }
        let violation = |message: &'static str| io::Error::new(io::ErrorKind::InvalidData, message);
        let ClientReply::Metadata { metadata } = reply else {
            return Err(self.retire(violation("unexpected reply to metadata query")));
        };
        if let Some(reason) = stamp_violation(&metadata.stamp) {
            return Err(self.retire(violation(reason)));
        }
        Ok(metadata)
    }

    /// One bounded, revision-checked directory page under the binding
    /// workspace.
    ///
    /// The peer must have advertised [`Capability::DirectoryMetadata`] at
    /// admission. `limit` must be within `1..=`[`MAX_DIRECTORY_PAGE_ENTRIES`]
    /// and `offset` within [`MAX_DIRECTORY_NAMES`] — bounds are enforced
    /// before any request is sent. The reply is trusted only after
    /// validation: at most `limit` entries, each name a single component
    /// of at most 255 bytes — never `/`, `.`, or `..` — in strictly
    /// increasing order, every stamp passing `stamp_violation`, the
    /// directory revision well-formed, and a `next_offset` cursor that
    /// equals exactly `offset + entries.len()`, may only continue a
    /// page that filled the requested `limit`, and must stay strictly
    /// inside [`MAX_DIRECTORY_NAMES`] — delivered names may end exactly
    /// at the cap, but a continuation there could never exist. A
    /// violation retires the
    /// stream — it is never reused with reply accounting that cannot be
    /// trusted.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::Unsupported`] when the peer lacks the
    /// capability, [`io::ErrorKind::InvalidInput`] for out-of-range paging
    /// arguments, the [`ReadError`] the Client reported mapped onto an
    /// I/O error kind, or [`io::ErrorKind::InvalidData`] for a malformed
    /// or untruthful reply.
    pub fn directory_page(
        &mut self,
        relative_path: &Path,
        offset: u32,
        limit: u32,
        expected_revision: Option<&str>,
    ) -> io::Result<DirectoryListing> {
        if !self.capabilities.contains(&Capability::DirectoryMetadata) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "peer admitted no directory metadata capability",
            ));
        }
        let name_cap = u32::try_from(MAX_DIRECTORY_NAMES).unwrap_or(u32::MAX);
        if !(1..=u32::try_from(MAX_DIRECTORY_PAGE_ENTRIES).unwrap_or(u32::MAX)).contains(&limit)
            || offset > name_cap
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid directory page request",
            ));
        }
        let reply = self.exchange(&ClientRequest::ReadDirectory {
            relative_path: relative_path.to_path_buf(),
            offset,
            limit,
            expected_revision: expected_revision.map(str::to_owned),
        })?;
        if let ClientReply::Error { kind } = reply {
            return Err(read_error(kind));
        }
        self.validate_directory(offset, limit, expected_revision, &reply)
    }

    /// Verifies a `Directory` reply against the request before trusting
    /// it — names, order, cursor, and every stamp. Any deviation means
    /// the peer cannot account for its answers honestly, so the
    /// connection is retired.
    fn validate_directory(
        &mut self,
        offset: u32,
        limit: u32,
        expected_revision: Option<&str>,
        reply: &ClientReply,
    ) -> io::Result<DirectoryListing> {
        let violation = |message: &'static str| io::Error::new(io::ErrorKind::InvalidData, message);
        let ClientReply::Directory {
            revision,
            entries,
            next_offset,
        } = reply
        else {
            return Err(self.retire(violation("unexpected reply to directory read")));
        };
        if !valid_revision(revision) {
            return Err(self.retire(violation("directory reply carries an invalid revision")));
        }
        if entries.len() > usize::try_from(limit).unwrap_or(usize::MAX) {
            return Err(self.retire(violation("directory reply exceeds requested page")));
        }
        let mut previous: Option<&str> = None;
        for entry in entries {
            let name = entry.name.as_str();
            // One normal component, verbatim: no `/`, never `.` or `..`,
            // no longer than a native name, never NUL — the one byte no
            // path API could ever carry. Every other byte — control
            // characters included — is ordinary literal name data the
            // Server never joins into a path it opens.
            if name.len() > 255
                || name.is_empty()
                || name.contains('/')
                || name.contains('\0')
                || name == "."
                || name == ".."
            {
                return Err(self.retire(violation("directory reply carries an invalid entry name")));
            }
            if previous.is_some_and(|previous| previous >= name) {
                return Err(self.retire(violation("directory reply names are out of order")));
            }
            previous = Some(name);
            if let Some(reason) = stamp_violation(&entry.metadata.stamp) {
                return Err(self.retire(violation(reason)));
            }
        }
        let delivered = u32::try_from(entries.len()).unwrap_or(u32::MAX);
        let Some(cursor) = offset.checked_add(delivered) else {
            return Err(self.retire(violation("directory reply overflows the name space")));
        };
        // The delivered names must stay inside the physical listing
        // bound even on a final page: a reply claiming entries past the
        // name cap is corrupt regardless of the cursor it carries.
        let name_cap = u32::try_from(MAX_DIRECTORY_NAMES).unwrap_or(u32::MAX);
        if cursor > name_cap {
            return Err(self.retire(violation("directory reply exceeds the name cap")));
        }
        // Continuation is legal only after a page that filled the
        // requested limit, and the cursor must land exactly past the
        // delivered entries — strictly inside the name cap, since a
        // resumption at the cap could never deliver another entry.
        match next_offset {
            Some(next) if delivered == limit && *next == cursor && cursor < name_cap => {}
            Some(_) => {
                return Err(self.retire(violation("directory reply cursor is inconsistent")));
            }
            None => {}
        }
        if let Some(expected) = expected_revision
            && revision != expected
        {
            return Err(self.retire(violation("directory reply revision ignores expectation")));
        }
        Ok(DirectoryListing {
            revision: revision.clone(),
            entries: entries.clone(),
            next_offset: *next_offset,
        })
    }

    /// One bounded logical-text page of `relative_path` under the binding
    /// workspace — the complete-line page contract the Read tool
    /// projects, scanned physically by the Client.
    ///
    /// The peer must have advertised [`Capability::ReadLines`] at
    /// admission or the call is [`io::ErrorKind::Unsupported`] and no
    /// request is sent. `offset` is 1-based and `limit`, when given, must
    /// be nonzero — both are enforced before anything crosses the wire.
    /// The reply is trusted only after validation: the stamp must pass
    /// `stamp_violation` and a supplied `expected_revision` must match,
    /// `total_lines` must be nonzero, `text` must stay within
    /// [`MAX_TEXT_BYTES`] and its `'\n'`-split must reproduce exactly
    /// `shown_lines` elements (never more than [`MAX_TEXT_LINES`]` + 1`),
    /// and `end` must agree with the request and the page's own numbers —
    /// `Complete` reaches the source end exactly, `RequestedLimit` shows
    /// precisely the requested bound with lines remaining, `LineLimit`
    /// and `ByteLimit` leave at least one more element, `FirstLineTooLarge`
    /// reports a first line beyond [`MAX_TEXT_BYTES`], and `OffsetPastEnd`
    /// addresses beyond the source. The page's accounting must also fit
    /// the stamped source size. Any deviation retires the stream — it is
    /// never reused with reply accounting that cannot be trusted.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::Unsupported`] when the peer lacks the
    /// capability, [`io::ErrorKind::InvalidInput`] for a zero offset or
    /// limit, the [`ReadError`] the Client reported mapped onto an I/O
    /// error kind — [`ReadError::InvalidUtf8`] included, a semantic
    /// refusal that never retires a healthy connection — or
    /// [`io::ErrorKind::InvalidData`] for a malformed or untruthful
    /// reply.
    pub fn read_lines(
        &mut self,
        relative_path: &Path,
        offset: u64,
        limit: Option<u64>,
        expected_revision: Option<&str>,
    ) -> io::Result<LinesPage> {
        self.read_lines_admitted(
            relative_path,
            offset,
            limit,
            expected_revision,
            Instant::now() + PAGE_REQUEST_TIMEOUT,
            || Ok(()),
            || (),
        )
    }

    /// `read_lines` against a caller-armed absolute deadline and
    /// acceptance gate: `deadline` bounds the request write and the
    /// reply read on the wire as one budget, `admit` runs after the
    /// request is encoded and the binding's live check passes but before
    /// the first request byte — its error returns verbatim and never
    /// retires the connection — and `sent` runs once the full request
    /// frame is written, before the reply is read.
    ///
    /// # Errors
    ///
    /// Returns the same kinds as [`read_lines`](Self::read_lines), plus
    /// the `admit` error verbatim and [`io::ErrorKind::TimedOut`] when
    /// the deadline lapses mid-exchange — which retires the connection.
    #[allow(clippy::too_many_arguments)] // one admitted exchange's full context
    pub fn read_lines_admitted(
        &mut self,
        relative_path: &Path,
        offset: u64,
        limit: Option<u64>,
        expected_revision: Option<&str>,
        deadline: Instant,
        admit: impl FnOnce() -> io::Result<()>,
        sent: impl FnOnce(),
    ) -> io::Result<LinesPage> {
        if !self.capabilities.contains(&Capability::ReadLines) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "peer admitted no lines capability",
            ));
        }
        if offset == 0 || limit == Some(0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid text page request",
            ));
        }
        let reply = self.exchange_admitted(
            &ClientRequest::ReadLines {
                relative_path: relative_path.to_path_buf(),
                offset: DecimalU64::new(offset),
                limit: limit.map(DecimalU64::new),
                expected_revision: expected_revision.map(str::to_owned),
            },
            Some(deadline),
            admit,
            sent,
        )?;
        if let ClientReply::Error { kind } = reply {
            return Err(read_error(kind));
        }
        self.validate_lines(offset, limit, expected_revision, &reply)
    }

    /// Verifies a `Lines` reply against the request and the scanner's
    /// complete-line contract before trusting it. Any deviation means
    /// the peer cannot account for its answers honestly, so the
    /// connection is retired.
    fn validate_lines(
        &mut self,
        offset: u64,
        limit: Option<u64>,
        expected_revision: Option<&str>,
        reply: &ClientReply,
    ) -> io::Result<LinesPage> {
        let violation = |message: &'static str| io::Error::new(io::ErrorKind::InvalidData, message);
        let ClientReply::Lines {
            metadata,
            text,
            total_lines,
            shown_lines,
            end,
        } = reply
        else {
            return Err(self.retire(violation("unexpected reply to lines read")));
        };
        if let Some(reason) = stamp_violation(metadata) {
            return Err(self.retire(violation(reason)));
        }
        if let Some(expected) = expected_revision
            && metadata.revision != expected
        {
            return Err(self.retire(violation("lines reply revision ignores expectation")));
        }
        let size = metadata.size.get();
        if let Some(reason) = page_accounting_violation(
            size,
            text,
            total_lines.get(),
            *shown_lines,
            end,
            offset,
            limit,
        ) {
            return Err(self.retire(violation(reason)));
        }
        Ok(LinesPage {
            text: text.clone(),
            total_lines: total_lines.get(),
            shown_lines: *shown_lines,
            end: *end,
            metadata: metadata.clone(),
        })
    }

    /// One bounded logical-text page of a whitelisted structured document
    /// under the binding workspace — the Client decodes the source with
    /// the shared rustX parser policy and serves the complete-line
    /// projection of the decoded Markdown.
    ///
    /// The peer must have advertised [`Capability::DocumentPages`] at
    /// admission or the call is [`io::ErrorKind::Unsupported`] and no
    /// request is sent. `offset` is 1-based and `limit`, when given, must
    /// be nonzero — both are enforced before anything crosses the wire.
    /// The reply is trusted only after validation: `source` must pass
    /// `stamp_violation`, stay within [`MAX_DOCUMENT_SOURCE_BYTES`], and a
    /// supplied `expected_revision` must match; `logical_bytes` — the
    /// decoded Markdown length, independent of the compressed source's
    /// size — must stay within [`MAX_DOCUMENT_LOGICAL_BYTES`]; and the
    /// page must pass the shared complete-line accounting every `Lines`
    /// reply answers to. A violation retires the stream — it is never
    /// reused with reply accounting that cannot be trusted. A typed
    /// semantic refusal — an unsupported extension, an oversized source,
    /// a parser rejection, or a missing text layer — is a distinct
    /// [`ReadError`] that never retires a healthy connection.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::Unsupported`] when the peer lacks the
    /// document capability, [`io::ErrorKind::InvalidInput`] for a zero
    /// offset or limit, the [`ReadError`] the Client reported mapped onto
    /// an I/O error kind, or [`io::ErrorKind::InvalidData`] for a
    /// malformed or untruthful reply.
    pub fn read_document(
        &mut self,
        relative_path: &Path,
        offset: u64,
        limit: Option<u64>,
        expected_revision: Option<&str>,
    ) -> io::Result<DocumentPage> {
        self.read_document_admitted(
            relative_path,
            offset,
            limit,
            expected_revision,
            Instant::now() + PAGE_REQUEST_TIMEOUT,
            || Ok(()),
            || (),
        )
    }

    /// `read_document` against a caller-armed absolute deadline and
    /// acceptance gate, under the same contract as
    /// [`read_lines_admitted`](Self::read_lines_admitted): one wire
    /// budget, an admission refusal that returns before any request byte
    /// without retiring, and a `sent` observation between the written
    /// frame and the reply.
    ///
    /// # Errors
    ///
    /// Returns the same kinds as [`read_document`](Self::read_document),
    /// plus the `admit` error verbatim and [`io::ErrorKind::TimedOut`]
    /// when the deadline lapses mid-exchange — which retires the
    /// connection.
    #[allow(clippy::too_many_arguments)] // one admitted exchange's full context
    pub fn read_document_admitted(
        &mut self,
        relative_path: &Path,
        offset: u64,
        limit: Option<u64>,
        expected_revision: Option<&str>,
        deadline: Instant,
        admit: impl FnOnce() -> io::Result<()>,
        sent: impl FnOnce(),
    ) -> io::Result<DocumentPage> {
        if !self.capabilities.contains(&Capability::DocumentPages) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "peer admitted no document capability",
            ));
        }
        if offset == 0 || limit == Some(0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid document page request",
            ));
        }
        let reply = self.exchange_admitted(
            &ClientRequest::ReadDocument {
                relative_path: relative_path.to_path_buf(),
                offset: DecimalU64::new(offset),
                limit: limit.map(DecimalU64::new),
                expected_revision: expected_revision.map(str::to_owned),
            },
            Some(deadline),
            admit,
            sent,
        )?;
        if let ClientReply::Error { kind } = reply {
            return Err(read_error(kind));
        }
        self.validate_document(offset, limit, expected_revision, &reply)
    }

    /// Verifies a `DocumentPage` reply against the request, the source
    /// and logical bounds, and the shared complete-line contract before
    /// trusting it. Any deviation means the peer cannot account for its
    /// answers honestly, so the connection is retired.
    fn validate_document(
        &mut self,
        offset: u64,
        limit: Option<u64>,
        expected_revision: Option<&str>,
        reply: &ClientReply,
    ) -> io::Result<DocumentPage> {
        let violation = |message: &'static str| io::Error::new(io::ErrorKind::InvalidData, message);
        let ClientReply::DocumentPage {
            source,
            logical_bytes,
            text,
            total_lines,
            shown_lines,
            end,
        } = reply
        else {
            return Err(self.retire(violation("unexpected reply to document read")));
        };
        if let Some(reason) = stamp_violation(source) {
            return Err(self.retire(violation(reason)));
        }
        if let Some(expected) = expected_revision
            && source.revision != expected
        {
            return Err(self.retire(violation("document reply revision ignores expectation")));
        }
        // The reply's own bounds are wire policy: a source the Client
        // could never have decoded and a projection larger than the
        // decoder's own cap are both untruthful on their face.
        let logical = logical_bytes.get();
        if source.size.get() > MAX_DOCUMENT_SOURCE_BYTES as u64
            || logical > MAX_DOCUMENT_LOGICAL_BYTES
        {
            return Err(self.retire(violation("document reply exceeds the document bounds")));
        }
        if let Some(reason) = page_accounting_violation(
            logical,
            text,
            total_lines.get(),
            *shown_lines,
            end,
            offset,
            limit,
        ) {
            return Err(self.retire(violation(reason)));
        }
        Ok(DocumentPage {
            source: source.clone(),
            logical_bytes: logical,
            text: text.clone(),
            total_lines: total_lines.get(),
            shown_lines: *shown_lines,
            end: *end,
        })
    }

    /// One bounded workspace search: the shared file-universe traversal
    /// rooted at `relative_root` — the workspace root itself when empty —
    /// narrowed by `spec` into one selected-records page.
    ///
    /// The peer must have advertised [`Capability::BoundedSearch`] at
    /// admission or the call is [`io::ErrorKind::Unsupported`] and no
    /// request is sent. The reply is trusted only after validation: the
    /// body must fit [`MAX_TEXT_BYTES`], the match count must stay within
    /// the requested bound and agree with the body's emptiness, a glob
    /// page can never claim shortened lines, and `hit_limit` is a proof —
    /// it requires `matches` to equal the request's bound exactly. A
    /// violation retires the stream — it is never reused with a reply
    /// accounting that cannot be trusted. Display bytes — record text
    /// that may contain any byte a Unix name carries — are never parsed
    /// back into accounting.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::Unsupported`] when the peer lacks the
    /// search capability, [`io::ErrorKind::InvalidInput`] for an
    /// out-of-range spec, the [`ReadError`] the Client reported mapped
    /// onto an I/O error kind, or [`io::ErrorKind::InvalidData`] for a
    /// malformed or untruthful reply.
    pub fn search(&mut self, relative_root: &Path, spec: &SearchSpec) -> io::Result<SearchResult> {
        self.search_admitted(
            relative_root,
            spec,
            Instant::now() + PAGE_REQUEST_TIMEOUT,
            || Ok(()),
            || (),
        )
    }

    /// `search` against a caller-armed absolute deadline and acceptance
    /// gate, under the same contract as
    /// [`read_lines_admitted`](Self::read_lines_admitted): one wire
    /// budget, an admission refusal that returns before any request byte
    /// without retiring, and a `sent` observation between the written
    /// frame and the reply.
    ///
    /// # Errors
    ///
    /// Returns the same kinds as [`search`](Self::search), plus the
    /// `admit` error verbatim and [`io::ErrorKind::TimedOut`] when the
    /// deadline lapses mid-exchange — which retires the connection.
    pub fn search_admitted(
        &mut self,
        relative_root: &Path,
        spec: &SearchSpec,
        deadline: Instant,
        admit: impl FnOnce() -> io::Result<()>,
        sent: impl FnOnce(),
    ) -> io::Result<SearchResult> {
        if !self.capabilities.contains(&Capability::BoundedSearch) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "peer admitted no search capability",
            ));
        }
        let limit = match spec {
            SearchSpec::Glob { pattern, limit } => {
                if pattern.len() > MAX_SEARCH_PATTERN_BYTES || limit.get() == 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "invalid search request",
                    ));
                }
                *limit
            }
            SearchSpec::Grep {
                pattern,
                glob,
                context,
                limit,
                ..
            } => {
                if pattern.len() > MAX_SEARCH_PATTERN_BYTES
                    || glob
                        .as_ref()
                        .is_some_and(|glob| glob.len() > MAX_SEARCH_PATTERN_BYTES)
                    || *context > MAX_SEARCH_CONTEXT_LINES
                    || limit.get() == 0
                {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "invalid search request",
                    ));
                }
                *limit
            }
        };
        let reply = self.exchange_admitted(
            &ClientRequest::Search {
                relative_root: relative_root.to_path_buf(),
                spec: spec.clone(),
            },
            Some(deadline),
            admit,
            sent,
        )?;
        if let ClientReply::Error { kind } = reply {
            return Err(read_error(kind));
        }
        let context = match spec {
            SearchSpec::Glob { .. } => 0,
            SearchSpec::Grep { context, .. } => *context,
        };
        self.validate_search(
            matches!(spec, SearchSpec::Glob { .. }),
            context,
            limit,
            &reply,
        )
    }

    /// Verifies a `SearchPage` reply against the request's bound before
    /// trusting it. Any deviation means the peer cannot account for its
    /// answers honestly, so the connection is retired.
    ///
    /// The one honest zero-match body: a Grep with before-context can
    /// fill the 50 KiB projection on context rows alone — the matching
    /// line's own record never fits — so `matches == 0` may pair with a
    /// nonempty body only when `context > 0` *and* `byte_limit` explains
    /// the cut; an empty body under `byte_limit` is the same overflow
    /// landing before the first record. Glob has no context rows: a
    /// nonempty body without a match is impossible there. Every other
    /// combination — an over-cap body, a count past the requested limit
    /// or past the body's own byte length, `short_lines` on Glob,
    /// `hit_limit` without a proven full limit, or the mutually
    /// exclusive `hit_limit && byte_limit` — retires the connection.
    fn validate_search(
        &mut self,
        glob: bool,
        context: u32,
        limit: DecimalU64,
        reply: &ClientReply,
    ) -> io::Result<SearchResult> {
        let violation = |message: &'static str| io::Error::new(io::ErrorKind::InvalidData, message);
        let ClientReply::SearchPage {
            body,
            matches,
            hit_limit,
            byte_limit,
            short_lines,
        } = reply
        else {
            return Err(self.retire(violation("unexpected reply to search")));
        };
        let matches = matches.get();
        let zero_match_context_overflow = !glob && context > 0 && *byte_limit;
        if body.len() > MAX_TEXT_BYTES
            || matches > limit.get()
            || matches > body.len() as u64
            || (matches > 0 && body.is_empty())
            || (!body.is_empty() && matches == 0 && !zero_match_context_overflow)
            || (glob && *short_lines)
            || (*hit_limit && matches != limit.get())
            || (*hit_limit && *byte_limit)
        {
            return Err(self.retire(violation("search reply accounting is impossible")));
        }
        Ok(SearchResult {
            body: body.clone(),
            matches,
            hit_limit: *hit_limit,
            byte_limit: *byte_limit,
            short_lines: *short_lines,
        })
    }

    /// The conditional write: `relative_path` under the binding workspace
    /// becomes exactly `bytes` when `expected_revision` still names the
    /// current file — or the target did not exist for `None`.
    ///
    /// The peer must have advertised [`Capability::ConditionalWrite`] at
    /// admission or the call is [`io::ErrorKind::Unsupported`] and no
    /// request is sent; `bytes` is bounded by [`MAX_RAW_CHUNK_BYTES`]
    /// before anything crosses the wire. The write is two phases on this
    /// same connection: one `PrepareWrite` retains the target and stages
    /// the payload privately, then — after `authorize` has re-checked its
    /// caller's gate against the prepared original — one `CommitWrite`
    /// publishes it, or one `AbortWrite` removes the stage when the gate
    /// denies and still admits the frame.
    ///
    /// The replies are verified, never trusted: the ticket must be a
    /// nonzero scalar the same connection commits or aborts under, and
    /// the prepared `original` must satisfy exactly the expectation that
    /// was sent — `Some` stamp with the expected revision for a replace,
    /// `None` for a create-only target. A violation retires the stream.
    ///
    /// The phases have different failure contracts. Before the commit
    /// request's first byte, every failure is a clean pre-commit error:
    /// the staged object cannot have published. Once that first byte is
    /// written the publish crossing may already have happened, so a
    /// transport error, a malformed reply, or a mismatched ticket retires
    /// the connection and returns a [`WriteOutcomeUnknown`]-downcastable
    /// error instead of a denial — an authoritative re-read is required,
    /// never a retry. An explicit typed [`ClientReply::Error`] still
    /// propagates as a refused write, and an explicit
    /// [`ClientReply::CommitUnknown`] with the correct ticket reports the
    /// same unknowable outcome.
    ///
    /// `authorize` runs on the verified prepared original before the
    /// commit is dispatched — the caller's last gate, invoked with
    /// `Some(&original)` for a replace or `None` for a create-only
    /// target. A denial sends `AbortWrite` while the gate still admits
    /// it and never commits.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::Unsupported`] when the peer lacks the
    /// write capability, [`io::ErrorKind::InvalidInput`] for an oversized
    /// payload, the [`ReadError`] the Client reported mapped onto an I/O
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
    /// frame is written. `expected_revision` maps onto
    /// [`WriteExpectation`]: `Some` requires that exact revision,
    /// `None` is create-only.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::Unsupported`] without the Client's write
    /// capability, [`io::ErrorKind::InvalidInput`] for an over-cap
    /// payload, the `authorize` error verbatim when its gate denies, a
    /// `rustx_execution_server::WriteOutcomeUnknown`-downcastable error
    /// once the commit outcome cannot be known, and the typed exchange's
    /// own errors.
    pub fn write_file(
        &mut self,
        relative_path: &Path,
        bytes: &[u8],
        expected_revision: Option<&str>,
        authorize: impl FnOnce(Option<&FileStamp>) -> io::Result<()>,
    ) -> io::Result<FileStamp> {
        // One absolute deadline covers prepare, the authorization abort,
        // and commit — it is armed once and never rearmed between phases.
        let deadline = Instant::now() + PAGE_REQUEST_TIMEOUT;
        let expectation = match expected_revision {
            Some(revision) => WriteExpectation::Revision(revision.to_owned()),
            None => WriteExpectation::Absent,
        };
        let prepared = self.prepare_write(
            relative_path,
            &expectation,
            bytes,
            Some(deadline),
            &|| Ok(()),
            &|| (),
        )?;
        if let Err(error) = authorize(prepared.original.as_ref()) {
            // The gate denied before commit: remove the preparation while
            // the binding still admits the frame — under the same
            // absolute deadline — and never commit. The denial is still
            // what the caller sees.
            if self.gate().is_ok() {
                let _ = self.abort_prepared(&prepared, Some(deadline), &|| Ok(()), &|| ());
            }
            return Err(error);
        }
        self.commit_prepared(&prepared, Some(deadline), &|| Ok(()), &|| ())
    }

    /// The prepare half of a conditional write: validates the request,
    /// dispatches `PrepareWrite` — or `BeginWrite` plus `WriteChunk`
    /// exchanges for a payload over [`MAX_RAW_CHUNK_BYTES`] — and returns
    /// the opaque ticketed [`PreparedMutation`] whose `size` is the
    /// exact byte count the later `Written` receipt must match.
    ///
    /// `expectation` decides what the named leaf must be: `Current`
    /// retains whatever is present — the pre-authorization object an
    /// Agent prepares before its permission gate and never itself a
    /// permission to publish — `Absent` is create-only, and `Revision`
    /// requires the exact current stamp.
    ///
    /// `deadline`, when present, is the caller's already-armed absolute
    /// wire budget — it is never rearmed here.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::Unsupported`] without the Client's write
    /// capability, [`io::ErrorKind::InvalidInput`] for an over-cap payload
    /// or malformed expectation, the [`ReadError`] the Client reported
    /// mapped onto an I/O error kind, or
    /// [`io::ErrorKind::InvalidData`] for a malformed or untruthful reply.
    pub fn prepare_write(
        &mut self,
        relative_path: &Path,
        expectation: &WriteExpectation,
        bytes: &[u8],
        deadline: Option<Instant>,
        admit: &impl Fn() -> io::Result<()>,
        sent: &impl Fn(),
    ) -> io::Result<PreparedMutation> {
        if !self.capabilities.contains(&Capability::ConditionalWrite) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "peer admitted no conditional write capability",
            ));
        }
        if bytes.len() > MAX_WRITE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "file write exceeds the write cap",
            ));
        }
        if let WriteExpectation::Revision(revision) = expectation
            && !valid_revision(revision)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "expected revision outside the bounded write policy",
            ));
        }
        let reply = if bytes.len() <= MAX_RAW_CHUNK_BYTES {
            self.exchange_admitted(
                &ClientRequest::PrepareWrite {
                    relative_path: relative_path.to_path_buf(),
                    expectation: expectation.clone(),
                    data_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
                },
                deadline,
                admit,
                sent,
            )?
        } else {
            return self.prepare_write_chunked(
                relative_path,
                expectation,
                bytes,
                deadline,
                admit,
                sent,
            );
        };
        if let ClientReply::Error { kind } = reply {
            return Err(read_error(kind));
        }
        self.valid_prepared(reply, expectation, bytes.len() as u64)
    }

    /// The chunked half of [`prepare_write`](Self::prepare_write) for a
    /// payload over [`MAX_RAW_CHUNK_BYTES`]: `BeginWrite` opens the
    /// ticketed preparation, then `WriteChunk` records transfer the
    /// payload at deterministic absolute offsets under the same armed
    /// deadline. A chunk-exchange failure aborts the opened ticket while
    /// the binding still admits the frame; a malformed echo retires.
    fn prepare_write_chunked(
        &mut self,
        relative_path: &Path,
        expectation: &WriteExpectation,
        bytes: &[u8],
        deadline: Option<Instant>,
        admit: &impl Fn() -> io::Result<()>,
        sent: &impl Fn(),
    ) -> io::Result<PreparedMutation> {
        let reply = self.exchange_admitted(
            &ClientRequest::BeginWrite {
                relative_path: relative_path.to_path_buf(),
                expectation: expectation.clone(),
                total: DecimalU64::new(bytes.len() as u64),
            },
            deadline,
            admit,
            sent,
        )?;
        if let ClientReply::Error { kind } = reply {
            return Err(read_error(kind));
        }
        let ClientReply::WritePrepared { ticket, .. } = reply else {
            return Err(self.retire(io::Error::new(
                io::ErrorKind::InvalidData,
                "unexpected reply to write begin",
            )));
        };
        if ticket.get() == 0 {
            return Err(self.retire(io::Error::new(
                io::ErrorKind::InvalidData,
                "write reply carries a zero ticket",
            )));
        }
        for (index, chunk) in bytes.chunks(MAX_RAW_CHUNK_BYTES).enumerate() {
            let offset = (index * MAX_RAW_CHUNK_BYTES) as u64;
            match self.exchange_admitted(
                &ClientRequest::WriteChunk {
                    ticket,
                    offset: DecimalU64::new(offset),
                    data_base64: base64::engine::general_purpose::STANDARD.encode(chunk),
                },
                deadline,
                admit,
                sent,
            ) {
                Err(error) => {
                    if self.gate().is_ok() {
                        let _ = self.abort_prepared(
                            &PreparedMutation {
                                ticket,
                                original: None,
                                size: bytes.len() as u64,
                                connection: self.connection.clone(),
                            },
                            deadline,
                            &|| Ok(()),
                            &|| (),
                        );
                    }
                    return Err(error);
                }
                Ok(ClientReply::WriteChunkReceived {
                    ticket: echoed_ticket,
                    offset: echoed,
                }) if echoed.get() == offset && echoed_ticket == ticket => {}
                Ok(_) => {
                    if self.gate().is_ok() {
                        let _ = self.abort_prepared(
                            &PreparedMutation {
                                ticket,
                                original: None,
                                size: bytes.len() as u64,
                                connection: self.connection.clone(),
                            },
                            deadline,
                            &|| Ok(()),
                            &|| (),
                        );
                    }
                    return Err(self.retire(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "unexpected reply to write chunk",
                    )));
                }
            }
        }
        self.valid_prepared(reply, expectation, bytes.len() as u64)
    }

    /// Validates one `WritePrepared`/`EditPrepared`-shaped reply into the
    /// opaque [`PreparedMutation`]: the ticket must be nonzero and the
    /// reported original must satisfy exactly the expectation that was
    /// sent — a valid stamp matching the revision for `Revision`, any
    /// valid stamp or `None` for `Current`, and `None` alone for
    /// `Absent`.
    fn valid_prepared(
        &mut self,
        reply: ClientReply,
        expectation: &WriteExpectation,
        size: u64,
    ) -> io::Result<PreparedMutation> {
        let violation = |message: &'static str| io::Error::new(io::ErrorKind::InvalidData, message);
        let ClientReply::WritePrepared { ticket, original } = reply else {
            return Err(self.retire(violation("unexpected reply to write prepare")));
        };
        if ticket.get() == 0 {
            return Err(self.retire(violation("write reply carries a zero ticket")));
        }
        let holds = match (expectation, &original) {
            (WriteExpectation::Revision(expected), Some(original)) => {
                original.revision == *expected && stamp_violation(original).is_none()
            }
            (WriteExpectation::Current, Some(original)) => stamp_violation(original).is_none(),
            (WriteExpectation::Current | WriteExpectation::Absent, None) => true,
            (WriteExpectation::Revision(_), None) | (WriteExpectation::Absent, Some(_)) => false,
        };
        if !holds {
            return Err(self.retire(violation("write reply ignores the expected revision")));
        }
        Ok(PreparedMutation {
            ticket,
            original,
            size,
            connection: self.connection.clone(),
        })
    }

    /// The commit half of [`prepare_write`](Self::prepare_write) and
    /// [`prepare_edit`](Self::prepare_edit): the caller's absolute
    /// deadline still applies and is never rearmed. `Written` is trusted
    /// only when its stamp is valid and its size equals `prepared.size`.
    ///
    /// # Errors
    ///
    /// Returns a [`WriteOutcomeUnknown`]-downcastable error once the
    /// outcome cannot be known, or the typed exchange's own errors.
    pub fn commit_prepared(
        &mut self,
        prepared: &PreparedMutation,
        deadline: Option<Instant>,
        admit: &impl Fn() -> io::Result<()>,
        sent: &impl Fn(),
    ) -> io::Result<FileStamp> {
        if prepared.connection != self.connection {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "prepared mutation belongs to a different connection",
            ));
        }
        self.commit_write(prepared.ticket, prepared.size, deadline, admit, sent)
    }

    /// The abort half: removes the ticketed preparation while the binding
    /// still admits the frame. A malformed abort reply retires the
    /// connection like any other protocol violation.
    ///
    /// # Errors
    ///
    /// Returns the typed exchange's own errors, or
    /// [`io::ErrorKind::InvalidData`] for a malformed reply.
    pub fn abort_prepared(
        &mut self,
        prepared: &PreparedMutation,
        deadline: Option<Instant>,
        admit: &impl Fn() -> io::Result<()>,
        sent: &impl Fn(),
    ) -> io::Result<()> {
        if prepared.connection != self.connection {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "prepared mutation belongs to a different connection",
            ));
        }
        match self.exchange_admitted(
            &ClientRequest::AbortWrite {
                ticket: prepared.ticket,
            },
            deadline,
            admit,
            sent,
        ) {
            Ok(ClientReply::WriteAborted { ticket: echoed }) if echoed == prepared.ticket => Ok(()),
            Ok(ClientReply::Error { kind }) => Err(read_error(kind)),
            Ok(_) => Err(self.retire(io::Error::new(
                io::ErrorKind::InvalidData,
                "unexpected reply to write abort",
            ))),
            Err(error) => Err(error),
        }
    }

    /// One conditional edit: `PrepareEdit` retains the exact named
    /// original, applies the bounded text-edit algorithm on the Client,
    /// and stages the result privately; `authorize` then gates the
    /// publish against the verified original stamp before the existing
    /// `CommitWrite`/`AbortWrite` machinery commits or abandons it.
    ///
    /// The whole sequence shares one absolute [`PAGE_REQUEST_TIMEOUT`]
    /// deadline armed once — it covers the prepare exchange, the
    /// authorization window, and the commit or abort, and it is never
    /// rearmed. A safe pre-wire refusal — missing capability, an
    /// out-of-policy request, an `EditRefused` semantic failure, or the
    /// `authorize` denial followed by a well-formed abort — never
    /// retires the connection. Once `CommitWrite`'s first byte is on
    /// the wire the publish may have crossed: a transport failure, a
    /// malformed or mismatched receipt, and an explicit
    /// `CommitUnknown` alike retire the connection and surface
    /// [`WriteOutcomeUnknown`], never an ordinary retryable denial.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::Unsupported`] when the peer lacks the
    /// edit capability, [`io::ErrorKind::InvalidInput`] for a request
    /// outside the bounded replacement policy, a [`RefusedEdit`]
    /// downcastable error carrying the typed [`EditRefusal`] when the
    /// Client refused semantically, the `authorize` error verbatim on
    /// denial, a [`WriteOutcomeUnknown`]-downcastable error once the
    /// publish outcome cannot be known, or
    /// [`io::ErrorKind::InvalidData`] for a malformed or untruthful
    /// reply.
    pub fn edit_file(
        &mut self,
        relative_path: &Path,
        expected_revision: &str,
        replacements: &[TextReplacement],
        authorize: impl FnOnce(&FileStamp) -> io::Result<()>,
    ) -> io::Result<FileStamp> {
        let deadline = Instant::now() + PAGE_REQUEST_TIMEOUT;
        let expectation = WriteExpectation::Revision(expected_revision.to_owned());
        let prepared = self.prepare_edit(
            relative_path,
            &expectation,
            replacements,
            Some(deadline),
            &|| Ok(()),
            &|| (),
        )?;
        let Some(original) = prepared.original.as_ref() else {
            // `prepare_edit` always validates `original: Some` — this is
            // unreachable for a healthy peer and must never panic.
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "edit prepare reported no original",
            ));
        };
        if let Err(error) = authorize(original) {
            // The gate denied before commit: remove the preparation
            // while the binding still admits the frame — under the same
            // absolute deadline — and never commit. The denial is still
            // what the caller sees.
            if self.gate().is_ok() {
                let _ = self.abort_prepared(&prepared, Some(deadline), &|| Ok(()), &|| ());
            }
            return Err(error);
        }
        self.commit_prepared(&prepared, Some(deadline), &|| Ok(()), &|| ())
    }

    /// The prepare half of a conditional edit: validates the request,
    /// dispatches `PrepareEdit`, and returns the ticketed
    /// [`PreparedMutation`] whose `size` is the promised published size.
    /// `expectation` must be `Revision` or `Current` — an edit always
    /// names an existing original, so `Absent` is malformed. A semantic
    /// `EditRefused` is shape-checked against the request and surfaces
    /// as a [`RefusedEdit`]-downcastable error that never retires a
    /// healthy connection.
    ///
    /// `deadline`, when present, is the caller's already-armed absolute
    /// wire budget — it is never rearmed here.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::Unsupported`] without the Client's edit
    /// capability, [`io::ErrorKind::InvalidInput`] for a request outside
    /// the bounded replacement policy, a [`RefusedEdit`]-downcastable
    /// error carrying the typed [`EditRefusal`], the [`ReadError`] the
    /// Client reported mapped onto an I/O error kind, or
    /// [`io::ErrorKind::InvalidData`] for a malformed or untruthful
    /// reply.
    pub fn prepare_edit(
        &mut self,
        relative_path: &Path,
        expectation: &WriteExpectation,
        replacements: &[TextReplacement],
        deadline: Option<Instant>,
        admit: &impl Fn() -> io::Result<()>,
        sent: &impl Fn(),
    ) -> io::Result<PreparedMutation> {
        if !self.capabilities.contains(&Capability::ConditionalEdit) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "peer admitted no conditional edit capability",
            ));
        }
        let request_bytes = replacements.iter().fold(0_usize, |total, edit| {
            total
                .saturating_add(edit.old_text.len())
                .saturating_add(edit.new_text.len())
        });
        if matches!(expectation, WriteExpectation::Absent)
            || replacements.is_empty()
            || replacements.len() > MAX_EDIT_REPLACEMENTS
            || request_bytes > MAX_EDIT_REQUEST_BYTES
            || replacements.iter().any(|edit| edit.old_text.is_empty())
            || matches!(expectation, WriteExpectation::Revision(revision) if !valid_revision(revision))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "edit request outside the bounded replacement policy",
            ));
        }
        let reply = self.exchange_admitted(
            &ClientRequest::PrepareEdit {
                relative_path: relative_path.to_path_buf(),
                expectation: expectation.clone(),
                replacements: replacements.to_vec(),
            },
            deadline,
            admit,
            sent,
        )?;
        let violation = |message: &'static str| io::Error::new(io::ErrorKind::InvalidData, message);
        match reply {
            ClientReply::Error { kind } => return Err(read_error(kind)),
            // A semantic refusal is only plausible when its shape could
            // come from this request: every index names a replacement we
            // sent, an ambiguity reports at least two occurrences, an
            // overlap names two distinct edits, and an empty-edits refusal
            // can never answer a nonempty request.
            ClientReply::EditRefused { reason } => {
                if !valid_edit_refusal(&reason, replacements) {
                    return Err(self.retire(violation("edit refusal contradicts the request")));
                }
                return Err(io::Error::new(
                    edit_refusal_kind(&reason),
                    RefusedEdit(reason),
                ));
            }
            _ => {}
        }
        let ClientReply::EditPrepared {
            ticket,
            original,
            published_size,
        } = reply
        else {
            return Err(self.retire(violation("unexpected reply to edit prepare")));
        };
        if ticket.get() == 0 {
            return Err(self.retire(violation("edit reply carries a zero ticket")));
        }
        // An edit always names an existing original: its stamp must be
        // valid and — for `Revision` — carry exactly the required
        // revision; for `Current` any valid stamp is the original.
        let revision_holds = match expectation {
            WriteExpectation::Revision(expected) => original.revision == *expected,
            _ => true,
        };
        if !revision_holds || stamp_violation(&original).is_some() {
            return Err(self.retire(violation("edit reply ignores the expected revision")));
        }
        // The source it claims to have applied is bounded too: a Client
        // could never have decoded an over-cap original, and the promised
        // result must stay within the file cap.
        if original.size.get() > MAX_EDIT_FILE_BYTES as u64
            || published_size.get() > MAX_EDIT_FILE_BYTES as u64
        {
            return Err(self.retire(violation("edit reply exceeds the file bound")));
        }
        Ok(PreparedMutation {
            ticket,
            original: Some(original),
            size: published_size.get(),
            connection: self.connection.clone(),
        })
    }

    /// The commit half of [`write_file`](Self::write_file): the request is
    /// encoded and gated before its first byte, so a denial there is still
    /// a clean pre-commit error. Once the first byte is written the publish
    /// crossing may already have happened — every later failure retires the
    /// connection and reports [`WriteOutcomeUnknown`], never a denial and
    /// never a retry. A `Written` receipt is trusted only when its stamp
    /// is valid *and* its size equals the exact byte count this connection
    /// submitted: a receipt describing different bytes cannot prove the
    /// publish, so the outcome stays unknown. `deadline`, when present,
    /// is the caller's already-armed absolute wire budget — it is never
    /// rearmed here.
    ///
    /// A correctly-ticketed `CommitUnknown` is valid *syntax* but never a
    /// safe ordinary outcome: the publish may have crossed, the ticket is
    /// consumed, and the peer's later accounting cannot be trusted — so
    /// it retires the connection like every other post-commit failure.
    fn commit_write(
        &mut self,
        ticket: DecimalU64,
        expected_size: u64,
        deadline: Option<Instant>,
        admit: &impl Fn() -> io::Result<()>,
        sent: &impl Fn(),
    ) -> io::Result<FileStamp> {
        let encoded = encode_record(&ClientRequest::CommitWrite { ticket })?;
        self.gate()?;
        admit()?;
        let outcome_unknown =
            |connection: &mut Self| connection.retire(io::Error::other(WriteOutcomeUnknown));
        let result = self
            .channel
            .send(&encoded, deadline)
            .map(|()| sent())
            .and_then(|()| self.channel.receive(deadline));
        let Ok(reply) = result else {
            return Err(outcome_unknown(self));
        };
        match reply {
            // An explicit typed refusal means the Client never crossed:
            // it propagates as an ordinary pre-commit error.
            ClientReply::Error { kind } => Err(read_error(kind)),
            // The Client crossed but its later stages failed — the same
            // unknowable outcome as a lost acknowledgement. The ticket
            // is consumed and the publish may be durably visible, so the
            // connection retires rather than stay live on untrusted
            // accounting.
            ClientReply::CommitUnknown { ticket: echoed } if echoed == ticket => {
                Err(outcome_unknown(self))
            }
            // The control lane's cancellation won arbitration before the
            // publish crossed: the Client proved nothing was written —
            // a typed clean-cancel outcome, never an unknown one.
            ClientReply::WriteCancelled { ticket: echoed } if echoed == ticket => {
                Err(io::Error::new(io::ErrorKind::Interrupted, CommitCancelled))
            }
            ClientReply::Written {
                ticket: echoed,
                metadata,
            } if echoed == ticket => {
                if stamp_violation(&metadata).is_none() && metadata.size.get() == expected_size {
                    Ok(metadata)
                } else {
                    Err(outcome_unknown(self))
                }
            }
            _ => Err(outcome_unknown(self)),
        }
    }
}

/// One live ticketed mutation preparation on this connection: the
/// opaque handle [`LocalConnection::prepare_write`] and
/// [`LocalConnection::prepare_edit`] return, consumable exactly once by
/// [`commit_prepared`](LocalConnection::commit_prepared) or
/// [`abort_prepared`](LocalConnection::abort_prepared). `size` is the
/// byte count the `Written` receipt must match exactly.
#[derive(Debug, Clone)]
pub struct PreparedMutation {
    /// The Client-issued ticket — never zero, never reused.
    pub ticket: DecimalU64,
    /// The prepared original: `Some` for a replace, `None` for a
    /// create-only target.
    pub original: Option<FileStamp>,
    /// The exact published byte count the commit receipt must carry.
    pub size: u64,
    /// The unique identity of the admitted connection the ticket belongs
    /// to. Tickets restart at 1 on every connection, so commit and abort
    /// check this before the wire: a `PreparedMutation` moved to a
    /// different `LocalConnection` is refused locally and never consumes
    /// another Session's same-numbered ticket.
    pub connection: ConnectionId,
}

/// The peer refused a conditional edit for one stable typed semantic
/// reason. Downcast the `io::Error` to recover the [`EditRefusal`];
/// nothing was staged and the connection stayed live.
#[derive(Debug)]
pub struct RefusedEdit(pub EditRefusal);

impl std::fmt::Display for RefusedEdit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "edit refused: {:?}", self.0)
    }
}

impl std::error::Error for RefusedEdit {}

/// Whether one typed [`EditRefusal`] could honestly answer a request
/// carrying `replacements` edits — every index must name a sent
/// replacement and the shape must be plausible. A contradiction is a
/// protocol violation, not a refusal.
fn valid_edit_refusal(reason: &EditRefusal, replacements: &[TextReplacement]) -> bool {
    let in_range = |index: u32| (index as usize) < replacements.len();
    match *reason {
        // A pre-checked request is never empty and carries no empty
        // anchor: either refusal answering one is a contradiction.
        EditRefusal::EmptyEdits => replacements.is_empty(),
        EditRefusal::EmptyAnchor { index } => replacements
            .get(index as usize)
            .is_some_and(|edit| edit.old_text.is_empty()),
        EditRefusal::Missing { index } | EditRefusal::UnsafeNormalization { index } => {
            in_range(index)
        }
        EditRefusal::Ambiguous { index, occurrences } => in_range(index) && occurrences >= 2,
        EditRefusal::Overlap { first, second } => {
            first != second && in_range(first) && in_range(second)
        }
        EditRefusal::NoChange | EditRefusal::TooLarge => true,
    }
}

/// The I/O error kind one [`EditRefusal`] maps onto — the typed reason
/// in [`RefusedEdit`] is authoritative; the kind only classifies.
fn edit_refusal_kind(reason: &EditRefusal) -> io::ErrorKind {
    match reason {
        EditRefusal::Missing { .. } => io::ErrorKind::NotFound,
        EditRefusal::TooLarge => io::ErrorKind::FileTooLarge,
        _ => io::ErrorKind::InvalidInput,
    }
}

/// The commit outcome is unknowable: the `CommitWrite` request was
/// already written — or the Client already reported crossing — when the
/// reply could not be delivered, trusted, or finished.
///
/// The write may or may not have published. It is never a clean
/// pre-commit denial and must never be retried blindly: callers downcast
/// the `io::Error` to this marker, report an unknown outcome, and learn
/// the truth only from an authoritative re-read.
#[derive(Debug)]
pub struct WriteOutcomeUnknown;

impl std::fmt::Display for WriteOutcomeUnknown {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "write commit outcome unknown; authoritative reread required"
        )
    }
}

impl std::error::Error for WriteOutcomeUnknown {}

/// The peer did not confirm the physical watch registration outcome.
#[derive(Debug)]
pub struct WatchOutcomeUnknown;

impl std::fmt::Display for WatchOutcomeUnknown {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("the Client did not confirm watch registration")
    }
}

impl std::error::Error for WatchOutcomeUnknown {}

/// The commit was arbitrated cancelled on the independent control lane
/// before the Client's publish crossing: the typed `WriteCancelled`
/// reply proves nothing named by the ticket was written. Downcast the
/// `io::Error` to this marker to distinguish a proven clean cancel —
/// reportable as an ordinary cancellation — from
/// [`WriteOutcomeUnknown`], which never shares that claim.
#[derive(Debug)]
pub struct CommitCancelled;

impl std::fmt::Display for CommitCancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "write commit cancelled before publish")
    }
}

impl std::error::Error for CommitCancelled {}

/// The shared content-revision check: a SHA-256 stamp is exactly 64
/// lowercase hexadecimal characters, nothing else.
fn valid_revision(revision: &str) -> bool {
    revision.len() == 64
        && revision
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// The shared source-stamp validator every verified reply passes: the
/// revision is a well-formed stamp and the identity is the confined
/// `dev:ino` layout — two canonical decimal halves under the u64 pair
/// bound. Anything else means the peer cannot account for its answers
/// honestly.
fn stamp_violation(stamp: &FileStamp) -> Option<&'static str> {
    if !valid_revision(&stamp.revision) {
        return Some("reply carries an invalid revision");
    }
    if stamp.identity.len() > 41
        || !stamp.identity.split_once(':').is_some_and(|(dev, ino)| {
            [dev, ino]
                .into_iter()
                .all(|part| part.parse::<DecimalU64>().is_ok())
        })
    {
        return Some("reply carries an invalid identity");
    }
    None
}

/// The shared complete-line page accounting a `Lines` or `DocumentPage`
/// reply answers to: the page's own numbers must be internally consistent
/// and must agree with the request's `offset`/`limit` and the source's
/// `logical_bytes` (the stamped source size for a `Lines` page, the
/// decoded Markdown length for a `DocumentPage`). Returns the violation
/// message — never `Ok` data — because an inconsistent reply means the
/// peer cannot account for its answers honestly.
fn page_accounting_violation(
    logical_bytes: u64,
    text: &str,
    total: u64,
    shown_lines: u32,
    end: &LineEnd,
    offset: u64,
    limit: Option<u64>,
) -> Option<&'static str> {
    let shown = u64::from(shown_lines);
    let shown_limit = u64::try_from(MAX_TEXT_LINES + 1).unwrap_or(u64::MAX);
    let line_limit = u64::try_from(MAX_TEXT_LINES).unwrap_or(u64::MAX);
    let byte_limit = u64::try_from(MAX_TEXT_BYTES).unwrap_or(u64::MAX);
    let text_bytes = u64::try_from(text.len()).unwrap_or(u64::MAX);
    if total == 0
        || shown > shown_limit
        || shown > total
        || text.len() > MAX_TEXT_BYTES
        || total - 1 > logical_bytes
        || text_bytes > logical_bytes
    {
        return Some("lines reply accounting is impossible");
    }
    // The shown text is the shown elements joined by `\n`: `shown` zero
    // is a diagnostic-only page that must carry no bytes, and a nonempty
    // page must reproduce exactly `shown` split elements — every one of
    // the `total - shown` unshown source delimiters must then fit inside
    // the logical length too.
    if shown == 0 {
        if !text.is_empty() {
            return Some("lines reply shown count is inconsistent");
        }
    } else {
        if text.split('\n').count() as u64 != shown {
            return Some("lines reply shown count is inconsistent");
        }
        let minimal = text_bytes.saturating_add(total - shown);
        if minimal > logical_bytes {
            return Some("lines reply contradicts the stamped size");
        }
        // The `line_limit + 1` shape exists only for a page that retained
        // the trailing empty element — text then ends in a newline and
        // the page completed or hit its requested bound, never a budget
        // stop.
        if shown == shown_limit
            && !(text.ends_with('\n') && matches!(end, LineEnd::Complete | LineEnd::RequestedLimit))
        {
            return Some("lines reply shown count is inconsistent");
        }
    }
    // `end` must agree with the request and the page's own numbers:
    // `remaining` is the elements the offset can still address, `selected`
    // what the window asked for — both derived without overflow. A budget
    // stop must leave the window unconsumed.
    let remaining = total.saturating_sub(offset.saturating_sub(1));
    let selected = limit.unwrap_or(u64::MAX).min(remaining);
    let valid_end = match end {
        LineEnd::Complete => shown > 0 && shown == remaining && shown == selected,
        LineEnd::RequestedLimit => shown > 0 && shown == selected && selected < remaining,
        LineEnd::LineLimit => shown == line_limit && shown < selected,
        LineEnd::ByteLimit => shown > 0 && shown <= line_limit && shown < selected,
        LineEnd::FirstLineTooLarge { bytes } => {
            shown == 0
                && remaining > 0
                && bytes.get() > byte_limit
                && bytes.get() <= logical_bytes - (total - 1)
        }
        LineEnd::OffsetPastEnd => shown == 0 && remaining == 0,
    };
    if !valid_end {
        return Some("lines reply end reason is inconsistent");
    }
    // A complete page from the first line is the whole logical source:
    // its joined text must reproduce the logical length exactly.
    if *end == LineEnd::Complete && offset == 1 && text_bytes != logical_bytes {
        return Some("lines reply contradicts the stamped size");
    }
    None
}

/// The stable physical-read refusal carried inside an `io::Error`.
///
/// A typed exchange reports a peer's `ReadError` by wrapping the code
/// here so the caller can downcast `io::Error::get_ref` to
/// [`PhysicalError`] and render an exact diagnostic — never by parsing
/// message text. The display is the same stable static description; no
/// raw path, parser message, or payload ever rides inside it.
#[derive(Debug)]
pub struct PhysicalError(pub ReadError);

impl PhysicalError {
    /// The stable static description of the wrapped code.
    fn message(&self) -> &'static str {
        match self.0 {
            ReadError::Invalid => "physical request invalid",
            ReadError::Denied => "physical access denied",
            ReadError::NotFound => "physical path not found",
            ReadError::Conflict => "physical revision conflict",
            ReadError::InvalidUtf8 => "physical source is not UTF-8",
            ReadError::UnsupportedDocument => "not a supported document source",
            ReadError::DocumentTooLarge => "document exceeds the source bound",
            ReadError::DocumentDecodeFailed => "document decode failed",
            ReadError::NoTextLayer => "document has no extractable text layer",
            ReadError::SearchBudgetExceeded => "search exceeded a bounded budget",
            ReadError::SearchNeedsDirectory => "search root is not a directory",
            ReadError::Unavailable => "physical operation unavailable",
            ReadError::Busy => "physical preparation budget is full",
        }
    }
}

impl std::fmt::Display for PhysicalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for PhysicalError {}

/// Maps a stable [`ReadError`] wire code onto an I/O error kind. No raw
/// path or payload crosses here — only the kind, with the code preserved
/// downcastable inside the error.
fn read_error(kind: ReadError) -> io::Error {
    let error_kind = match kind {
        ReadError::Denied => io::ErrorKind::PermissionDenied,
        ReadError::NotFound => io::ErrorKind::NotFound,
        ReadError::Conflict => io::ErrorKind::AlreadyExists,
        ReadError::InvalidUtf8 | ReadError::DocumentDecodeFailed | ReadError::NoTextLayer => {
            io::ErrorKind::InvalidData
        }
        ReadError::UnsupportedDocument => io::ErrorKind::Unsupported,
        ReadError::DocumentTooLarge => io::ErrorKind::FileTooLarge,
        ReadError::SearchBudgetExceeded => io::ErrorKind::QuotaExceeded,
        ReadError::Invalid | ReadError::SearchNeedsDirectory => io::ErrorKind::InvalidInput,
        ReadError::Unavailable => io::ErrorKind::Other,
        ReadError::Busy => io::ErrorKind::ResourceBusy,
    };
    io::Error::new(error_kind, PhysicalError(kind))
}

/// The small absolute budget a still-healthy connection gets to send its
/// WebSocket Close frame at orderly teardown — long enough for loopback,
/// short enough that a dead peer cannot stall `Drop`.
const ORDERLY_CLOSE_BUDGET: Duration = Duration::from_secs(1);

impl Drop for LocalConnection {
    /// Orderly teardown sends the transport's graceful close first — a
    /// WebSocket peer reads a real RFC 6455 Close frame, not a bare TCP
    /// reset — then detaches the binding. Failure paths (`retire`,
    /// `disconnect`, `revoke`) already shut the channel down abruptly
    /// and never take this path: a forced close must not masquerade as a
    /// clean finish.
    fn drop(&mut self) {
        self.close();
    }
}

/// Generates a fresh server-side connection identity from OS randomness.
fn fresh_connection_id() -> io::Result<ConnectionId> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(io::Error::other)?;
    let mut id = String::with_capacity(5 + bytes.len() * 2);
    id.push_str("conn-");
    for byte in bytes {
        use std::fmt::Write;
        write!(id, "{byte:02x}").expect("string write");
    }
    Ok(ConnectionId::new(id))
}

/// The retained proof of one published bootstrap bundle: the open file
/// descriptor from `create_new`, the private parent directory descriptor,
/// and the file name. Teardown compares `fstatat(AT_SYMLINK_NOFOLLOW)`
/// against this proof and unlinks only when device and inode still match
/// — a replaced, symlinked, FIFO, or identical-bytes foreign file is
/// preserved, and no pathname is ever reopened.
pub struct PublishedBundle {
    file: std::fs::File,
    parent: PrivateParent,
}

impl PublishedBundle {
    /// Removes the bundle only while it is still provably this process's
    /// file. An `Err` from `stat_child` — already unlinked, or the parent
    /// renamed away — just preserves whatever is there.
    pub fn remove(&self) {
        let Ok(proof) = nix::sys::stat::fstat(&self.file) else {
            return;
        };
        self.parent.remove_if(&proof);
    }
}

/// Writes the bootstrap bundle as exactly one framed record into a
/// freshly created owner-only file inside `parent` — `O_CREAT|O_EXCL`
/// through the verified directory descriptor, so the pathname is never
/// re-resolved.
///
/// If `create_new` refuses an existing path, nothing is touched. Once the
/// file exists and is ours, any later failure removes it by the same
/// dev/inode proof teardown uses — failed publication never leaves a
/// partial bootstrap behind and never deletes a foreign replacement.
fn publish_bootstrap(
    endpoint: BootstrapEndpoint,
    parent: &PrivateParent,
    binding: &Binding,
    credential: BindingCredential,
) -> io::Result<PublishedBundle> {
    let reservation = &binding.reservation;
    let bootstrap = Bootstrap {
        endpoint,
        version: PROTOCOL_VERSION,
        server_incarnation: server_incarnation(),
        session: reservation.session.clone(),
        environment: reservation.environment.clone(),
        instance: reservation.instance.clone(),
        generation: reservation.generation,
        workspace: reservation.workspace.clone(),
        credential,
    };
    // The parent descriptor the returned proof will own is duplicated
    // before the file exists: a dup failure here fails cleanly, while a
    // late one could strand a published credential.
    let publication = PrivateParent {
        dir: parent.dir.try_clone()?,
        name: parent.name.clone(),
    };
    let fd = nix::fcntl::openat(
        &parent.dir,
        parent.name.as_os_str(),
        nix::fcntl::OFlag::O_WRONLY
            | nix::fcntl::OFlag::O_CREAT
            | nix::fcntl::OFlag::O_EXCL
            | nix::fcntl::OFlag::O_NOFOLLOW
            | nix::fcntl::OFlag::O_CLOEXEC,
        nix::sys::stat::Mode::from_bits_truncate(0o600),
    )
    .map_err(io::Error::from)?;
    let mut file = std::fs::File::from(fd);
    let result = (|| {
        // Enforce the mode explicitly so the process umask cannot weaken
        // or strengthen it.
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        write_record(&mut file, &bootstrap)?;
        file.sync_all()
    })();
    if let Err(error) = result {
        if let Ok(proof) = nix::sys::stat::fstat(&file) {
            publication.remove_if(&proof);
        }
        return Err(error);
    }
    Ok(PublishedBundle {
        file,
        parent: publication,
    })
}

/// The verified private parent of a publication target: the directory's
/// own descriptor and the child's file name. `create_new` publication and
/// identity-proved teardown both run through this descriptor, so the
/// pathname is never reopened and a swapped or renamed ancestor cannot
/// redirect an unlink to a foreign file.
struct PrivateParent {
    dir: std::fs::File,
    name: std::ffi::OsString,
}

impl PrivateParent {
    /// `fstatat(AT_SYMLINK_NOFOLLOW)` on the child through the retained
    /// directory descriptor — never a pathname lookup, never an `open`,
    /// so a FIFO replacement cannot block teardown and a symlink is seen
    /// as a symlink.
    fn stat_child(&self) -> io::Result<nix::sys::stat::FileStat> {
        nix::sys::stat::fstatat(
            &self.dir,
            self.name.as_os_str(),
            nix::fcntl::AtFlags::AT_SYMLINK_NOFOLLOW,
        )
        .map_err(io::Error::from)
    }

    /// Unlinks the child only while `fstatat` still proves it is the file
    /// `proof` describes — same type, device, and inode. A replaced,
    /// symlinked, or foreign file is preserved untouched.
    fn remove_if(&self, proof: &nix::sys::stat::FileStat) {
        let Ok(current) = self.stat_child() else {
            return;
        };
        if file_is_regular(current.st_mode)
            && current.st_dev == proof.st_dev
            && current.st_ino == proof.st_ino
        {
            let _ = nix::unistd::unlinkat(
                &self.dir,
                self.name.as_os_str(),
                nix::unistd::UnlinkatFlags::NoRemoveDir,
            );
        }
    }
}

fn file_is_regular(mode: nix::sys::stat::mode_t) -> bool {
    nix::sys::stat::SFlag::from_bits_truncate(mode).contains(nix::sys::stat::SFlag::S_IFREG)
}

/// Requires `path` to be a normalized absolute direct child of an
/// existing owner-only (`0700`) directory owned by this process's euid,
/// reached through a chain containing no symlinks: the parent is opened
/// by a real no-follow walk — `openat(O_DIRECTORY|O_NOFOLLOW|O_CLOEXEC)`
/// per component from `/` — so the returned descriptor is provably the
/// verified directory; a later ancestor rename or symlink swap cannot
/// redirect a lookup done through it.
fn require_private_child(path: &Path) -> io::Result<PrivateParent> {
    if !is_normalized_absolute_path(path) {
        return Err(invalid_input("path must be a normalized absolute path"));
    }
    let Some(parent) = path.parent() else {
        return Err(invalid_input("path has no parent directory"));
    };
    let Some(name) = path.file_name() else {
        return Err(invalid_input("path has no file name"));
    };
    // The parent is pinned by a real no-follow walk: every ancestor is
    // `openat`'d `O_DIRECTORY|O_NOFOLLOW` from `/`, so the descriptor is
    // provably the verified directory itself — a later ancestor rename
    // or symlink swap cannot redirect a lookup done through it.
    let dir = rustx_execution_transport::open_dir_nofollow(parent)
        .map_err(|_| invalid_input("parent directory is missing or unreadable"))?;
    let metadata = dir
        .metadata()
        .map_err(|_| invalid_input("parent directory is missing or unreadable"))?;
    if !metadata.file_type().is_dir()
        || metadata.uid() != euid()
        || metadata.mode() & 0o777 != 0o700
    {
        return Err(invalid_input(
            "parent must be an owner-only private directory",
        ));
    }
    Ok(PrivateParent {
        dir,
        name: name.to_owned(),
    })
}

fn euid() -> u32 {
    nix::unistd::geteuid().as_raw()
}

fn invalid_input(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

fn invalid_input_static(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.to_owned())
}

#[cfg(test)]
mod tests {
    use std::io::Read;
    use std::num::NonZeroU64;
    use std::sync::Barrier;
    use std::thread;
    use std::time::Duration;

    use rustx_execution_protocol::EntryKind;
    use rustx_execution_protocol::transport::MAX_RECORD_BYTES;

    use super::*;

    fn required() -> BTreeSet<Capability> {
        BTreeSet::from([Capability::Files, Capability::Process])
    }

    fn reservation() -> Reservation {
        reservation_in(Path::new("/work/root"))
    }

    fn reservation_in(workspace: &Path) -> Reservation {
        Reservation {
            session: SessionId::new("session-1"),
            environment: EnvironmentId::new("env-1"),
            instance: ClientInstanceId::new("instance-1"),
            generation: Generation::new(NonZeroU64::MIN),
            workspace: workspace.to_path_buf(),
            required_capabilities: required(),
        }
    }

    fn valid_hello(credential: &BindingCredential) -> Hello {
        Hello {
            version: PROTOCOL_VERSION,
            session: SessionId::new("session-1"),
            environment: EnvironmentId::new("env-1"),
            instance: ClientInstanceId::new("instance-1"),
            generation: Generation::new(NonZeroU64::MIN),
            workspace: PathBuf::from("/work/root"),
            capabilities: required(),
            credential: credential.clone(),
        }
    }

    #[test]
    fn concurrent_hellos_commit_exactly_one_live_connection() {
        let (binding, credential) = Binding::reserve(reservation()).unwrap();
        let hello = valid_hello(&credential);
        let barrier = Barrier::new(2);
        let results: Vec<_> = thread::scope(|scope| {
            let binding = &binding;
            let hello = &hello;
            let barrier = &barrier;
            let handles: Vec<_> = ["conn-a", "conn-b"]
                .into_iter()
                .map(|name| {
                    scope.spawn(move || {
                        barrier.wait();
                        binding.connect(hello, ConnectionId::new(name))
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });

        assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|r| **r == Err(Reject::AlreadyConnected))
                .count(),
            1
        );
        let status = binding.status();
        assert_eq!(status.phase, BindingPhase::Live);
        assert!(matches!(
            status.connection.as_ref().map(ConnectionId::as_str),
            Some("conn-a" | "conn-b")
        ));
    }

    #[test]
    fn wrong_hello_fields_never_consume_a_reservation() {
        type Mutation = Box<dyn Fn(&mut Hello)>;
        let cases: Vec<(&str, Mutation, Reject)> = vec![
            (
                "version",
                Box::new(|h: &mut Hello| h.version = 2),
                Reject::UnsupportedVersion,
            ),
            (
                "session",
                Box::new(|h: &mut Hello| h.session = SessionId::new("other")),
                Reject::SessionMismatch,
            ),
            (
                "environment",
                Box::new(|h: &mut Hello| {
                    h.environment = EnvironmentId::new("other");
                }),
                Reject::EnvironmentMismatch,
            ),
            (
                "instance",
                Box::new(|h: &mut Hello| {
                    h.instance = ClientInstanceId::new("other");
                }),
                Reject::InstanceMismatch,
            ),
            (
                "generation",
                Box::new(|h: &mut Hello| {
                    h.generation = Generation::new(NonZeroU64::new(2).unwrap());
                }),
                Reject::GenerationMismatch,
            ),
            (
                "workspace",
                Box::new(|h: &mut Hello| {
                    h.workspace = PathBuf::from("/work/other");
                }),
                Reject::WorkspaceMismatch,
            ),
            // `Path` equality would normalize `//` away and admit this;
            // the literal OsStr comparison must not.
            (
                "workspace separator alias",
                Box::new(|h: &mut Hello| {
                    h.workspace = PathBuf::from("/work//root");
                }),
                Reject::WorkspaceMismatch,
            ),
            (
                "workspace dot alias",
                Box::new(|h: &mut Hello| {
                    h.workspace = PathBuf::from("/work/./root");
                }),
                Reject::WorkspaceMismatch,
            ),
            (
                "required capability",
                Box::new(|h: &mut Hello| {
                    h.capabilities = BTreeSet::from([Capability::Files]);
                }),
                Reject::MissingCapabilities,
            ),
            (
                "credential",
                Box::new(|h: &mut Hello| {
                    h.credential = BindingCredential::from_bytes([0xa5; 32]);
                }),
                Reject::InvalidCredential,
            ),
        ];

        for (name, mutate, expected) in cases {
            let (binding, credential) = Binding::reserve(reservation()).unwrap();
            let mut wrong = valid_hello(&credential);
            mutate(&mut wrong);
            assert_eq!(
                binding.connect(&wrong, ConnectionId::new("conn-1")),
                Err(expected),
                "{name}"
            );
            assert_eq!(binding.status().phase, BindingPhase::Reserved, "{name}");
            // The failed Hello consumed nothing: a correct Hello still wins.
            assert!(
                binding
                    .connect(&valid_hello(&credential), ConnectionId::new("conn-1"))
                    .is_ok()
            );
            assert_eq!(binding.status().phase, BindingPhase::Live, "{name}");
        }
    }

    #[test]
    fn reject_after_live_does_not_displace_the_live_connection() {
        let (binding, credential) = Binding::reserve(reservation()).unwrap();
        let hello = valid_hello(&credential);
        binding
            .connect(&hello, ConnectionId::new("conn-1"))
            .unwrap();

        let mut wrong_instance = hello.clone();
        wrong_instance.instance = ClientInstanceId::new("other");
        assert_eq!(
            binding.connect(&wrong_instance, ConnectionId::new("conn-2")),
            Err(Reject::InstanceMismatch)
        );
        let status = binding.status();
        assert_eq!(status.phase, BindingPhase::Live);
        assert_eq!(status.connection.unwrap().as_str(), "conn-1");
    }

    #[test]
    fn revoke_is_immediate_and_terminal() {
        let (binding, credential) = Binding::reserve(reservation()).unwrap();
        let hello = valid_hello(&credential);

        binding.revoke();
        assert_eq!(
            binding.connect(&hello, ConnectionId::new("conn-1")),
            Err(Reject::Revoked)
        );
        assert_eq!(binding.status().phase, BindingPhase::Revoked);

        let (binding, credential) = Binding::reserve(reservation()).unwrap();
        let valid = valid_hello(&credential);
        binding
            .connect(&valid, ConnectionId::new("conn-1"))
            .unwrap();
        binding.revoke();
        let status = binding.status();
        assert_eq!(status.phase, BindingPhase::Revoked);
        assert_eq!(status.connection, None);
        assert_eq!(
            binding.connect(&valid, ConnectionId::new("conn-2")),
            Err(Reject::Revoked)
        );
    }

    #[test]
    fn stale_disconnect_is_ignored_and_detach_never_reopens_the_reservation() {
        let (binding, credential) = Binding::reserve(reservation()).unwrap();
        let hello = valid_hello(&credential);
        binding
            .connect(&hello, ConnectionId::new("conn-1"))
            .unwrap();

        // A foreign or stale disconnect cannot tear down the live connection.
        binding.disconnect(&ConnectionId::new("stale"));
        assert_eq!(binding.status().connection.unwrap().as_str(), "conn-1");

        // The real EOF detaches; the reservation does not reopen.
        binding.disconnect(&ConnectionId::new("conn-1"));
        let status = binding.status();
        assert_eq!(status.phase, BindingPhase::Detached);
        assert_eq!(status.connection, None);

        // A different instance cannot take over a detached binding.
        let mut replacement = hello.clone();
        replacement.instance = ClientInstanceId::new("instance-2");
        assert_eq!(
            binding.connect(&replacement, ConnectionId::new("conn-2")),
            Err(Reject::InstanceMismatch)
        );
        assert_eq!(binding.status().phase, BindingPhase::Detached);

        // Even the fully valid reserved identity cannot replay its own
        // credential after EOF: re-admission needs recovery proof (#577).
        assert_eq!(
            binding.connect(&hello, ConnectionId::new("conn-2")),
            Err(Reject::RecoveryRequired)
        );
        assert_eq!(binding.status().phase, BindingPhase::Detached);
    }

    #[test]
    fn credential_is_verified_before_any_field_probe() {
        let (binding, credential) = Binding::reserve(reservation()).unwrap();
        // Every field wrong, credential wrong: the verifier runs first.
        let mut probe = valid_hello(&credential);
        probe.credential = BindingCredential::from_bytes([0x00; 32]);
        probe.session = SessionId::new("other");
        probe.version = 2;
        assert_eq!(
            binding.connect(&probe, ConnectionId::new("conn-1")),
            Err(Reject::InvalidCredential)
        );
        // Every field wrong, credential right: field checks then report.
        let mut probe = valid_hello(&credential);
        probe.session = SessionId::new("other");
        probe.environment = EnvironmentId::new("other");
        assert_eq!(
            binding.connect(&probe, ConnectionId::new("conn-1")),
            Err(Reject::SessionMismatch)
        );
        assert_eq!(binding.status().phase, BindingPhase::Reserved);
    }

    #[test]
    fn reserve_rejects_unnormalized_or_relative_workspaces() {
        for workspace in ["work/root", "/work//root", "/work/../root", "/work/"] {
            let mut reservation = reservation();
            reservation.workspace = PathBuf::from(workspace);
            assert!(Binding::reserve(reservation).is_err(), "{workspace}");
        }
    }

    #[test]
    fn reserve_local_requires_private_parent_and_leaves_no_leftovers() {
        use std::os::unix::fs::PermissionsExt;

        let workspace = tempfile::tempdir().unwrap();
        let public = tempfile::tempdir().unwrap();
        std::fs::set_permissions(public.path(), std::fs::Permissions::from_mode(0o755)).unwrap();

        // A world-readable parent is never trusted, even for a valid child.
        assert!(
            Binding::reserve_local(
                reservation_in(workspace.path()),
                &public.path().join("client.sock"),
                &public.path().join("bootstrap"),
            )
            .is_err()
        );
        assert!(!public.path().join("client.sock").exists());

        // A preexisting socket path is never silently unlinked.
        let private = tempfile::tempdir().unwrap();
        std::fs::set_permissions(private.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let socket = private.path().join("client.sock");
        let bundle = private.path().join("bootstrap");
        std::fs::write(&socket, b"occupied").unwrap();
        assert!(
            Binding::reserve_local(reservation_in(workspace.path()), &socket, &bundle).is_err()
        );
        assert_eq!(std::fs::read(&socket).unwrap(), b"occupied");

        // A preexisting bootstrap path fails publication and removes the
        // freshly bound socket: no leftovers.
        std::fs::remove_file(&socket).unwrap();
        std::fs::write(&bundle, b"occupied").unwrap();
        assert!(
            Binding::reserve_local(reservation_in(workspace.path()), &socket, &bundle).is_err()
        );
        assert!(!socket.exists());
        assert_eq!(std::fs::read(&bundle).unwrap(), b"occupied");
    }

    /// Reserves a real binding inside a private `0700` directory and
    /// returns its retained published-bundle proof for tamper tests.
    fn reserved_bundle(
        workspace: &Path,
    ) -> (tempfile::TempDir, std::path::PathBuf, PublishedBundle) {
        use std::os::unix::fs::PermissionsExt;
        let private = tempfile::tempdir().unwrap();
        std::fs::set_permissions(private.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let socket = private.path().join("s");
        let bundle = private.path().join("b");
        let (_binding, _listener, published) =
            Binding::reserve_local(reservation_in(workspace), &socket, &bundle).unwrap();
        (private, bundle, published)
    }

    #[test]
    fn published_bundle_remove_preserves_foreign_replacements() {
        use std::os::unix::fs::FileTypeExt;

        let workspace = tempfile::tempdir().unwrap();
        let (private, bundle, published) = reserved_bundle(workspace.path());
        let original = std::fs::read(&bundle).unwrap();
        assert!(
            std::fs::symlink_metadata(&bundle)
                .unwrap()
                .file_type()
                .is_file()
        );

        // Identical bytes on a different inode: the foreign file is
        // never ours, so it is preserved byte-for-byte.
        std::fs::remove_file(&bundle).unwrap();
        std::fs::write(&bundle, &original).unwrap();
        published.remove();
        assert_eq!(std::fs::read(&bundle).unwrap(), original);

        // A FIFO replacement is never opened — removal is prompt and
        // the FIFO survives untouched.
        std::fs::remove_file(&bundle).unwrap();
        nix::unistd::mkfifo(
            &bundle,
            nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
        )
        .unwrap();
        published.remove();
        assert!(
            std::fs::symlink_metadata(&bundle)
                .unwrap()
                .file_type()
                .is_fifo()
        );

        // A symlink replacement — even one pointing at a real user file
        // inside the same directory — preserves both link and target.
        std::fs::remove_file(&bundle).unwrap();
        let target = private.path().join("target");
        std::fs::write(&target, b"user data").unwrap();
        std::os::unix::fs::symlink(&target, &bundle).unwrap();
        published.remove();
        assert!(
            std::fs::symlink_metadata(&bundle)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read(&target).unwrap(), b"user data");
        std::fs::remove_file(&bundle).unwrap();
        std::fs::remove_file(&target).unwrap();

        // And while our own file still occupies the path, removal does
        // unlink exactly it.
        std::fs::write(&bundle, &original).unwrap();
        let (private2, bundle2, published2) = reserved_bundle(workspace.path());
        let _ = private;
        published2.remove();
        assert!(!bundle2.exists());
        assert!(private2.path().join("s").exists());
    }

    #[test]
    fn published_bundle_remove_survives_ancestor_rename() {
        let workspace = tempfile::tempdir().unwrap();
        let (private, bundle, published) = reserved_bundle(workspace.path());

        // Renaming the parent away moves the real directory — and the
        // retained descriptor still tracks it, so our file inside is
        // removed while a foreign replacement at the original path is
        // never even seen.
        let destination = tempfile::tempdir().unwrap();
        let moved = destination.path().join("moved");
        std::fs::rename(private.path(), &moved).unwrap();
        std::fs::create_dir(private.path()).unwrap();
        std::fs::write(private.path().join("b"), b"foreign").unwrap();

        published.remove();
        assert!(!moved.join("b").exists(), "our file was removed");
        assert_eq!(
            std::fs::read(private.path().join("b")).unwrap(),
            b"foreign",
            "the foreign replacement is preserved"
        );
        let _ = bundle;
    }

    #[test]
    fn extra_advertised_capabilities_do_not_relax_requirements() {
        let (binding, credential) = Binding::reserve(reservation()).unwrap();
        let mut hello = valid_hello(&credential);
        hello.capabilities = BTreeSet::from([
            Capability::Files,
            Capability::Process,
            Capability::BoundedSearch,
        ]);
        assert!(binding.connect(&hello, ConnectionId::new("conn-1")).is_ok());
    }

    fn reserved(capabilities: BTreeSet<Capability>) -> (Binding, Hello) {
        let mut reserved = reservation();
        reserved.required_capabilities = capabilities.clone();
        let (binding, credential) = Binding::reserve(reserved).unwrap();
        let mut hello = valid_hello(&credential);
        hello.capabilities = capabilities;
        (binding, hello)
    }

    /// The client-side `ControlHello` a paired lane carries — the data
    /// Hello's exact identity plus the challenge the Server issued.
    fn control_hello(hello: &Hello, challenge: PairingNonce) -> PeerHello {
        PeerHello::Control(ControlHello {
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

    /// The minimal client-side control loop the tests' paired lanes run:
    /// every `ControlPoll` carries a fresh deterministic nonce and the
    /// receipt for the previous request; `Health` echoes, `Revoke`
    /// acknowledges — `CancelMutation` answers `Unknown` because these
    /// peers stage no local mutations. The loop ends when the lane does.
    fn control_peer_loop(channel: &mut rustx_execution_transport::RecordChannel) {
        let mut sequence = 1_u64;
        let mut receipt: Option<ControlReply> = None;
        loop {
            let deadline = Instant::now() + Duration::from_secs(5);
            let poll = ControlPoll {
                sequence: DecimalU64::new(sequence),
                receipt: receipt.take(),
                health: DecimalU64::new(sequence),
            };
            if channel
                .send(&encode_record(&poll).unwrap(), Some(deadline))
                .is_err()
            {
                return;
            }
            let Ok(turn) = channel.receive::<ControlTurn>(Some(deadline)) else {
                return;
            };
            if turn.sequence != poll.sequence || turn.health != poll.health {
                return;
            }
            sequence += 1;
            let Some(request) = turn.request else {
                std::thread::sleep(Duration::from_millis(20));
                continue;
            };
            let done = matches!(request, ControlRequest::Revoke);
            receipt = Some(match request {
                ControlRequest::Health { nonce } => ControlReply::Healthy { nonce },
                ControlRequest::CancelMutation { ticket } => ControlReply::MutationCancelled {
                    ticket,
                    disposition: CancelDisposition::Unknown,
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
                let poll = ControlPoll {
                    sequence: DecimalU64::new(sequence),
                    receipt,
                    health: DecimalU64::new(sequence),
                };
                let _ = channel.send(
                    &encode_record(&poll).unwrap(),
                    Some(Instant::now() + Duration::from_secs(5)),
                );
                return;
            }
        }
    }

    /// One real loopback WebSocket pair admitted through
    /// `accept_channel_deadline` — the genuine RFC 6455 upgrade on both
    /// client lanes, never a UDS stand-in. Returns the live connection
    /// and the data peer channel for driving typed replies; the control
    /// peer polls on its own bounded thread.
    fn ws_connection(
        binding: &Binding,
        hello: &Hello,
    ) -> (LocalConnection, rustx_execution_transport::RecordChannel) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let endpoint = || {
            rustx_execution_transport::ExecutionUrl::validate(&format!(
                "ws://{address}/execution/v1"
            ))
            .unwrap()
        };
        // The data half: `PeerHello::Data` earns `Pending` and the
        // pairing challenge.
        let (challenge, client) = thread::scope(|scope| {
            let server = scope.spawn(|| {
                let (stream, _) = listener.accept().unwrap();
                let channel = rustx_execution_transport::accept(stream, deadline).unwrap();
                binding.accept_channel_deadline(channel, deadline).unwrap()
            });
            let mut client =
                rustx_execution_transport::connect(&endpoint(), deadline, None).unwrap();
            client
                .send(
                    &encode_record(&PeerHello::Data(hello.clone())).unwrap(),
                    Some(deadline),
                )
                .unwrap();
            let reply: DataHelloReply = client.receive(Some(deadline)).unwrap();
            let Ok(data) = reply else {
                panic!("the honest data Hello is admitted");
            };
            assert!(matches!(server.join().unwrap(), ChannelAccept::Pending));
            (data.challenge, client)
        });
        // The pairing control lane: a second real WebSocket connection
        // carrying the challenge commits the pair; its peer keeps
        // polling on a bounded thread.
        let connection = thread::scope(|scope| {
            let server = scope.spawn(|| {
                let (stream, _) = listener.accept().unwrap();
                let channel = rustx_execution_transport::accept(stream, deadline).unwrap();
                binding.accept_channel_deadline(channel, deadline).unwrap()
            });
            let mut control =
                rustx_execution_transport::connect(&endpoint(), deadline, None).unwrap();
            control
                .send(
                    &encode_record(&control_hello(hello, challenge)).unwrap(),
                    Some(deadline),
                )
                .unwrap();
            let reply: ControlHelloReply = control.receive(Some(deadline)).unwrap();
            assert!(reply.is_ok(), "the honest control lane commits: {reply:?}");
            let ChannelAccept::Live(connection) = server.join().unwrap() else {
                panic!("the paired control lane commits the connection");
            };
            thread::spawn(move || control_peer_loop(&mut control));
            connection
        });
        (connection, client)
    }

    #[test]
    fn a_failed_local_preencode_never_retires_a_websocket_connection() {
        let (binding, hello) = reserved(required());
        let (mut connection, mut client) = ws_connection(&binding, &hello);
        // A request that cannot be encoded fails locally before the gate
        // and before a single byte reaches the wire — the channel is
        // never retired for it.
        let oversized = ClientRequest::WriteChunk {
            ticket: DecimalU64::new(1),
            offset: DecimalU64::new(0),
            data_base64: "a".repeat(MAX_RECORD_BYTES + 1),
        };
        assert!(connection.exchange(&oversized).is_err());
        let deadline = Instant::now() + Duration::from_secs(10);
        let pong = thread::spawn(move || {
            let request: ClientRequest = client.receive(Some(deadline)).unwrap();
            assert_eq!(request, ClientRequest::Ping);
            client
                .send(&encode_record(&ClientReply::Pong).unwrap(), Some(deadline))
                .unwrap();
        });
        connection.ping().unwrap();
        pong.join().unwrap();
    }

    #[test]
    fn websocket_exchange_carries_identical_typed_records() {
        let (binding, hello) = reserved(required());
        let (mut connection, mut client) = ws_connection(&binding, &hello);
        let deadline = Instant::now() + Duration::from_secs(10);
        let answer = thread::spawn(move || {
            let request: ClientRequest = client.receive(Some(deadline)).unwrap();
            assert_eq!(request, ClientRequest::Ping);
            client
                .send(&encode_record(&ClientReply::Pong).unwrap(), Some(deadline))
                .unwrap();
        });
        connection.ping().unwrap();
        answer.join().unwrap();
        assert_eq!(binding.status().phase, BindingPhase::Live);
    }

    #[test]
    fn websocket_revocation_wakes_a_blocked_exchange() {
        let (binding, hello) = reserved(required());
        let (mut connection, _client) = ws_connection(&binding, &hello);
        // The peer never replies: the exchange blocks until revoke shuts
        // the raw TCP handle the binding committed at admission.
        let exchange = thread::spawn(move || {
            connection
                .read_range(Path::new("file"), 0, 64, None)
                .unwrap_err()
        });
        thread::sleep(Duration::from_millis(100));
        binding.revoke();
        let error = exchange.join().unwrap();
        assert!(
            !matches!(error.kind(), io::ErrorKind::TimedOut),
            "revocation woke the blocked exchange: {error:?}"
        );
        assert_eq!(binding.status().phase, BindingPhase::Revoked);
    }

    #[test]
    fn a_peer_without_the_range_capability_gets_no_request() {
        let (binding, hello) = reserved(BTreeSet::from([Capability::Connection]));
        let (mut connection, mut peer) = live(&binding, &hello);

        // No admitted capability: the call fails locally and no request is
        // ever placed on the wire.
        let error = connection
            .read_range(Path::new("file"), 0, 64, None)
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
        peer.set_read_timeout(Some(Duration::from_millis(200)))
            .unwrap();
        assert!(read_record::<ClientRequest>(&mut peer).is_err());
        peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();

        // The connection still serves the typed liveness exchange.
        let answer = thread::spawn(move || {
            let request: ClientRequest = read_record(&mut peer).unwrap();
            assert_eq!(request, ClientRequest::Ping);
            write_record(&mut peer, &ClientReply::Pong).unwrap();
        });
        connection.ping().unwrap();
        answer.join().unwrap();
        assert_eq!(binding.status().phase, BindingPhase::Live);
    }

    /// One fully admitted live pair over real socket pairs: the data
    /// Hello is written by the peer, its challenge pairs a second lane
    /// whose peer keeps polling on a bounded thread, and the data peer
    /// end comes back for scripted replies.
    fn live(binding: &Binding, hello: &Hello) -> (LocalConnection, UnixStream) {
        let (mut peer, stream) = UnixStream::pair().unwrap();
        write_record(&mut peer, &PeerHello::Data(hello.clone())).unwrap();
        (live_data_half(binding, hello, stream, &mut peer), peer)
    }

    /// The paired admission on an already-configured server stream: the
    /// data half must already carry `PeerHello::Data(hello)` — this
    /// admits it `Pending`, pairs the mandatory control lane on a fresh
    /// socket pair, and returns the committed connection. The control
    /// peer polls on its own bounded thread.
    fn live_data_half(
        binding: &Binding,
        hello: &Hello,
        stream: UnixStream,
        peer: &mut UnixStream,
    ) -> LocalConnection {
        assert!(matches!(
            binding.accept_local(stream).unwrap(),
            ChannelAccept::Pending
        ));
        let reply: DataHelloReply = read_record(peer).unwrap();
        let Ok(data) = reply else {
            panic!("the honest data Hello is admitted");
        };
        pair_control_half(binding, hello, &data.challenge)
    }

    /// Commits a pending data half: a second socket pair carries the
    /// `ControlHello` challenge, the committed connection comes back,
    /// and the scripted control peer keeps polling so the pair stays
    /// live for the test's lifetime.
    fn pair_control_half(
        binding: &Binding,
        hello: &Hello,
        challenge: &PairingNonce,
    ) -> LocalConnection {
        let (mut control_peer, control_stream) = UnixStream::pair().unwrap();
        write_record(&mut control_peer, &control_hello(hello, challenge.clone())).unwrap();
        let accepted = binding.accept_local(control_stream).unwrap();
        let ChannelAccept::Live(connection) = accepted else {
            panic!("the paired control lane commits the connection");
        };
        let reply: ControlHelloReply = read_record(&mut control_peer).unwrap();
        assert!(reply.is_ok(), "the honest control lane commits: {reply:?}");
        thread::spawn(move || {
            let mut channel = RecordChannel::unix(control_peer);
            control_peer_loop(&mut channel);
        });
        connection
    }

    #[test]
    fn an_unencodable_request_never_reaches_the_wire() {
        use std::os::unix::ffi::OsStrExt;

        let (binding, hello) = reserved(BTreeSet::from([
            Capability::Connection,
            Capability::ReadRange,
        ]));
        let (mut connection, mut peer) = live(&binding, &hello);

        // A relative path serde cannot encode and a bound-breaking
        // expected revision both fail at `encode_record`, before the
        // gate: the request never touches the wire.
        let error = connection
            .read_range(
                Path::new(std::ffi::OsStr::from_bytes(b"name\xff")),
                0,
                64,
                None,
            )
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        let oversized = "a".repeat(MAX_RECORD_BYTES);
        let error = connection
            .read_range(Path::new("file"), 0, 64, Some(&oversized))
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);

        // Prove nothing was written: a nonblocking probe reports
        // WouldBlock, then blocking I/O resumes.
        peer.set_nonblocking(true).unwrap();
        let mut probe = [0u8; 1];
        assert_eq!(
            peer.read(&mut probe).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        peer.set_nonblocking(false).unwrap();
        assert_eq!(binding.status().phase, BindingPhase::Live);

        // The still-live connection answers an exact Ping and then a
        // valid read.
        let answer = thread::spawn(move || {
            let request: ClientRequest = read_record(&mut peer).unwrap();
            assert_eq!(request, ClientRequest::Ping);
            write_record(&mut peer, &ClientReply::Pong).unwrap();
            let request: ClientRequest = read_record(&mut peer).unwrap();
            let ClientRequest::ReadRange { offset, .. } = request else {
                panic!("expected a range request");
            };
            write_record(
                &mut peer,
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
        });
        connection.ping().unwrap();
        let range = connection
            .read_range(Path::new("file"), 0, 64, None)
            .unwrap();
        assert_eq!(range.bytes, b"ab");
        answer.join().unwrap();
    }

    #[test]
    fn a_peer_closing_mid_request_detaches_the_binding() {
        let (binding, hello) = reserved(BTreeSet::from([
            Capability::Connection,
            Capability::ReadRange,
        ]));
        let (mut connection, mut peer) = live(&binding, &hello);

        // The peer reads the request's prefix and then shuts down: the
        // reply accounting can never be trusted again.
        let closer = thread::spawn(move || {
            let mut prefix = [0u8; 4];
            peer.read_exact(&mut prefix).unwrap();
            peer.shutdown(std::net::Shutdown::Both).unwrap();
        });
        let error = connection
            .read_range(Path::new("file"), 0, 64, None)
            .unwrap_err();
        assert!(matches!(
            error.kind(),
            io::ErrorKind::UnexpectedEof
                | io::ErrorKind::ConnectionReset
                | io::ErrorKind::BrokenPipe
        ));
        closer.join().unwrap();
        assert_eq!(binding.status().phase, BindingPhase::Detached);
        assert!(connection.ping().is_err());
    }

    #[test]
    fn an_expired_deadline_before_commit_keeps_the_reservation() {
        let (binding, hello) = reserved(BTreeSet::from([Capability::Connection]));

        // A deadline already expired at accept: no Hello is ever read.
        let (_peer, stream) = UnixStream::pair().unwrap();
        let Err(error) = binding.accept_local_deadline(stream, Instant::now()) else {
            panic!("an expired deadline must fail before the Hello");
        };
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert_eq!(binding.status().phase, BindingPhase::Reserved);

        // The honest Hello still wins the live connection afterwards.
        assert!(binding.connect(&hello, ConnectionId::new("conn-2")).is_ok());
        assert_eq!(binding.status().phase, BindingPhase::Live);
    }

    #[test]
    fn a_closed_data_half_before_pairing_discards_the_attempt() {
        let (binding, hello) = reserved(BTreeSet::from([Capability::Connection]));
        let (mut peer, stream) = UnixStream::pair().unwrap();
        write_record(&mut peer, &PeerHello::Data(hello.clone())).unwrap();
        assert!(matches!(
            binding.accept_local(stream).unwrap(),
            ChannelAccept::Pending
        ));
        let reply: DataHelloReply = read_record(&mut peer).unwrap();
        let Ok(data) = reply else {
            panic!("the honest data Hello is admitted");
        };
        assert_eq!(binding.status().phase, BindingPhase::Reserved);

        // The data half dies before the control lane pairs: the watchdog
        // sees the lane's end and discards the attempt — the reservation
        // stays Reserved and the now-stale nonce pairs nothing.
        drop(peer);
        let deadline = Instant::now() + Duration::from_secs(10);
        while binding.status().pending {
            assert!(
                Instant::now() < deadline,
                "the watchdog never discarded the attempt"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let (mut control_peer, control_stream) = UnixStream::pair().unwrap();
        write_record(
            &mut control_peer,
            &control_hello(&hello, data.challenge.clone()),
        )
        .unwrap();
        assert!(matches!(
            binding.accept_local(control_stream).unwrap(),
            ChannelAccept::Rejected(Reject::InvalidPairing)
        ));
        let reply: ControlHelloReply = read_record(&mut control_peer).unwrap();
        assert!(matches!(reply, Err(Reject::InvalidPairing)));
        assert_eq!(binding.status().phase, BindingPhase::Reserved);
    }

    #[test]
    fn an_invalid_revision_retires_the_stream_and_detaches_the_binding() {
        let (binding, hello) = reserved(BTreeSet::from([
            Capability::Connection,
            Capability::ReadRange,
        ]));
        let (mut connection, mut peer) = live(&binding, &hello);

        let answer = thread::spawn(move || {
            let request: ClientRequest = read_record(&mut peer).unwrap();
            let ClientRequest::ReadRange { offset, .. } = request else {
                panic!("expected a range request");
            };
            write_record(
                &mut peer,
                &ClientReply::Range {
                    data_base64: String::new(),
                    offset,
                    next_offset: None,
                    metadata: FileStamp {
                        size: DecimalU64::new(0),
                        revision: String::from("not-a-revision"),
                        identity: String::from("1:1"),
                        modified_at_ms: None,
                    },
                },
            )
            .unwrap();
        });
        let error = connection
            .read_range(Path::new("file"), 0, 64, None)
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        answer.join().unwrap();

        // The violation detached the binding and retired the stream:
        // nothing further is ever requested on it.
        assert_eq!(binding.status().phase, BindingPhase::Detached);
        assert!(connection.ping().is_err());
    }

    fn process_key() -> rustx_execution_protocol::process::ProcessOperationKey {
        use rustx_execution_protocol::process::{OperationId, ServerIncarnation};
        rustx_execution_protocol::process::ProcessOperationKey {
            incarnation: ServerIncarnation::generate(),
            session: rustx_execution_protocol::SessionId::new("session-1"),
            instance: rustx_execution_protocol::ClientInstanceId::new("client-1"),
            generation: rustx_execution_protocol::Generation::try_from(1u64).unwrap(),
            operation: OperationId::new(),
        }
    }

    #[test]
    fn a_contradictory_process_reply_retires_the_pair() {
        use rustx_execution_protocol::DecimalU64;
        use rustx_execution_protocol::process::{ProcessReply, ProcessRequest};

        let (binding, hello) = reserved(BTreeSet::from([
            Capability::Connection,
            Capability::Process,
        ]));
        let (mut connection, mut peer) = live(&binding, &hello);
        let key = process_key();
        let request = ProcessRequest::Start { key: key.clone() };

        let answer = thread::spawn(move || {
            let received: ClientRequest = read_record(&mut peer).unwrap();
            assert!(matches!(received, ClientRequest::Process(_)));
            write_record(
                &mut peer,
                &ClientReply::Process(ProcessReply::InputAccepted {
                    key,
                    sequence: DecimalU64::new(1),
                }),
            )
            .unwrap();
        });
        let error = connection.process(&request).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        answer.join().unwrap();

        assert_eq!(binding.status().phase, BindingPhase::Detached);
        assert!(connection.ping().is_err());
    }

    #[test]
    fn a_prewire_invalid_output_bound_leaves_the_pair_healthy() {
        use rustx_execution_protocol::DecimalU64;
        use rustx_execution_protocol::process::ProcessRequest;

        let (binding, hello) = reserved(BTreeSet::from([
            Capability::Connection,
            Capability::Process,
        ]));
        let (mut connection, mut peer) = live(&binding, &hello);
        let request = ProcessRequest::OutputPage {
            key: process_key(),
            cursor: DecimalU64::new(0),
            max_bytes: 0,
        };
        let error = connection.process(&request).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);

        let answer = thread::spawn(move || {
            let request: ClientRequest = read_record(&mut peer).unwrap();
            assert_eq!(request, ClientRequest::Ping);
            write_record(&mut peer, &ClientReply::Pong).unwrap();
        });
        connection.ping().unwrap();
        answer.join().unwrap();
        assert_eq!(binding.status().phase, BindingPhase::Live);
    }

    #[test]
    fn a_size_contradiction_retires_the_stream_and_detaches_the_binding() {
        let (binding, hello) = reserved(BTreeSet::from([
            Capability::Connection,
            Capability::ReadRange,
        ]));
        let (mut connection, mut peer) = live(&binding, &hello);

        // The peer declares premature EOF: bytes end below the stamped size
        // yet no continuation cursor is offered and the bound is unfilled.
        let answer = thread::spawn(move || {
            let request: ClientRequest = read_record(&mut peer).unwrap();
            let ClientRequest::ReadRange { offset, .. } = request else {
                panic!("expected a range request");
            };
            write_record(
                &mut peer,
                &ClientReply::Range {
                    data_base64: base64::engine::general_purpose::STANDARD.encode(b"ab"),
                    offset,
                    next_offset: None,
                    metadata: FileStamp {
                        size: DecimalU64::new(5),
                        revision: "a".repeat(64),
                        identity: String::from("1:1"),
                        modified_at_ms: None,
                    },
                },
            )
            .unwrap();
        });
        let error = connection
            .read_range(Path::new("file"), 0, 64, None)
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        answer.join().unwrap();

        assert_eq!(binding.status().phase, BindingPhase::Detached);
        assert!(connection.ping().is_err());
    }

    fn entry(name: &str) -> PageEntry {
        PageEntry {
            name: name.into(),
            metadata: EntryStamp {
                kind: EntryKind::RegularFile,
                stamp: FileStamp {
                    size: DecimalU64::new(1),
                    revision: "a".repeat(64),
                    identity: String::from("1:1"),
                    modified_at_ms: None,
                },
            },
        }
    }

    #[test]
    fn a_missing_directory_capability_denies_before_the_wire() {
        let (binding, hello) = reserved(BTreeSet::from([
            Capability::Connection,
            Capability::ReadRange,
        ]));
        let (mut connection, mut peer) = live(&binding, &hello);

        for error in [
            connection.entry_metadata(Path::new("file")).unwrap_err(),
            connection
                .directory_page(Path::new(""), 0, 16, None)
                .unwrap_err(),
        ] {
            assert_eq!(error.kind(), io::ErrorKind::Unsupported);
        }

        // Nothing was written; the connection stays live and still
        // answers an exact Ping.
        peer.set_nonblocking(true).unwrap();
        let mut byte = [0u8; 1];
        assert_eq!(
            peer.read(&mut byte).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        peer.set_nonblocking(false).unwrap();
        assert_eq!(binding.status().phase, BindingPhase::Live);
        let answer = thread::spawn(move || {
            let request: ClientRequest = read_record(&mut peer).unwrap();
            assert_eq!(request, ClientRequest::Ping);
            write_record(&mut peer, &ClientReply::Pong).unwrap();
        });
        connection.ping().unwrap();
        answer.join().unwrap();
    }

    #[test]
    fn an_out_of_order_directory_reply_retires_the_stream() {
        let (binding, hello) = reserved(BTreeSet::from([
            Capability::Connection,
            Capability::DirectoryMetadata,
        ]));
        let (mut connection, mut peer) = live(&binding, &hello);

        // Names that are not strictly increasing are untrusted wire
        // data: the stream retires and the binding detaches.
        let answer = thread::spawn(move || {
            let request: ClientRequest = read_record(&mut peer).unwrap();
            let ClientRequest::ReadDirectory { .. } = request else {
                panic!("expected a directory request");
            };
            write_record(
                &mut peer,
                &ClientReply::Directory {
                    revision: "a".repeat(64),
                    entries: vec![entry("b"), entry("a")],
                    next_offset: None,
                },
            )
            .unwrap();
        });
        let error = connection
            .directory_page(Path::new(""), 0, 16, None)
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        answer.join().unwrap();

        assert_eq!(binding.status().phase, BindingPhase::Detached);
        assert!(connection.ping().is_err());
    }

    #[test]
    fn a_directory_reply_with_a_wrong_cursor_retires_the_stream() {
        let (binding, hello) = reserved(BTreeSet::from([
            Capability::Connection,
            Capability::DirectoryMetadata,
        ]));
        let (mut connection, mut peer) = live(&binding, &hello);

        // A full page whose continuation cursor does not equal
        // offset + entries.len() violates the paging contract.
        let answer = thread::spawn(move || {
            let request: ClientRequest = read_record(&mut peer).unwrap();
            let ClientRequest::ReadDirectory { offset, .. } = request else {
                panic!("expected a directory request");
            };
            write_record(
                &mut peer,
                &ClientReply::Directory {
                    revision: "a".repeat(64),
                    entries: vec![entry("a"), entry("b")],
                    next_offset: Some(offset + 99),
                },
            )
            .unwrap();
        });
        let error = connection
            .directory_page(Path::new(""), 0, 16, None)
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        answer.join().unwrap();

        assert_eq!(binding.status().phase, BindingPhase::Detached);
        assert!(connection.ping().is_err());
    }

    #[test]
    fn a_nul_directory_name_retires_the_stream() {
        let (binding, hello) = reserved(BTreeSet::from([
            Capability::Connection,
            Capability::DirectoryMetadata,
        ]));
        let (mut connection, mut peer) = live(&binding, &hello);

        // Every other control byte is literal name data; NUL is the one
        // byte no path API could ever carry.
        let answer = thread::spawn(move || {
            let request: ClientRequest = read_record(&mut peer).unwrap();
            let ClientRequest::ReadDirectory { .. } = request else {
                panic!("expected a directory request");
            };
            write_record(
                &mut peer,
                &ClientReply::Directory {
                    revision: "a".repeat(64),
                    entries: vec![entry("a\0b"), entry("a\u{1f}")],
                    next_offset: None,
                },
            )
            .unwrap();
        });
        let error = connection
            .directory_page(Path::new(""), 0, 16, None)
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        answer.join().unwrap();

        assert_eq!(binding.status().phase, BindingPhase::Detached);
        assert!(connection.ping().is_err());
    }

    #[test]
    fn a_directory_reply_past_the_name_cap_retires_the_stream() {
        // A continuation cursor may never sit at the cap itself: a
        // resumption there could never deliver another entry, so the
        // `Some` is a lie even when the arithmetic agrees.
        let (binding, hello) = reserved(BTreeSet::from([
            Capability::Connection,
            Capability::DirectoryMetadata,
        ]));
        let (mut connection, mut peer) = live(&binding, &hello);

        let cap = u32::try_from(MAX_DIRECTORY_NAMES).unwrap();
        let answer = thread::spawn(move || {
            let request: ClientRequest = read_record(&mut peer).unwrap();
            let ClientRequest::ReadDirectory { offset, .. } = request else {
                panic!("expected a directory request");
            };
            assert_eq!(offset, cap - 1);
            write_record(
                &mut peer,
                &ClientReply::Directory {
                    revision: "a".repeat(64),
                    entries: vec![entry("a")],
                    next_offset: Some(cap),
                },
            )
            .unwrap();
        });
        let error = connection
            .directory_page(Path::new(""), cap - 1, 1, None)
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        answer.join().unwrap();
        assert_eq!(binding.status().phase, BindingPhase::Detached);
        assert!(connection.ping().is_err());

        // And a reply claiming entries past the cap is corrupt even on
        // a final page — the delivered names themselves exceed the
        // physical listing bound, `next=None` notwithstanding.
        let (binding, hello) = reserved(BTreeSet::from([
            Capability::Connection,
            Capability::DirectoryMetadata,
        ]));
        let (mut connection, mut peer) = live(&binding, &hello);

        let answer = thread::spawn(move || {
            let request: ClientRequest = read_record(&mut peer).unwrap();
            let ClientRequest::ReadDirectory { offset, .. } = request else {
                panic!("expected a directory request");
            };
            assert_eq!(offset, cap);
            write_record(
                &mut peer,
                &ClientReply::Directory {
                    revision: "a".repeat(64),
                    entries: vec![entry("a")],
                    next_offset: None,
                },
            )
            .unwrap();
        });
        let error = connection
            .directory_page(Path::new(""), cap, 1, None)
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        answer.join().unwrap();
        assert_eq!(binding.status().phase, BindingPhase::Detached);
        assert!(connection.ping().is_err());
    }

    #[test]
    fn a_directory_page_ending_exactly_at_the_name_cap_is_legitimate() {
        let (binding, hello) = reserved(BTreeSet::from([
            Capability::Connection,
            Capability::DirectoryMetadata,
        ]));
        let (mut connection, mut peer) = live(&binding, &hello);

        // The final entry may land exactly on the cap: `next=None` is
        // the honest answer — the directory can hold no more.
        let cap = u32::try_from(MAX_DIRECTORY_NAMES).unwrap();
        let answer = thread::spawn(move || {
            let request: ClientRequest = read_record(&mut peer).unwrap();
            let ClientRequest::ReadDirectory { offset, .. } = request else {
                panic!("expected a directory request");
            };
            assert_eq!(offset, cap - 2);
            write_record(
                &mut peer,
                &ClientReply::Directory {
                    revision: "a".repeat(64),
                    entries: vec![entry("a"), entry("b")],
                    next_offset: None,
                },
            )
            .unwrap();
        });
        let page = connection
            .directory_page(Path::new(""), cap - 2, 16, None)
            .unwrap();
        assert_eq!(page.entries.len(), 2);
        assert_eq!(page.next_offset, None);
        answer.join().unwrap();
        assert_eq!(binding.status().phase, BindingPhase::Live);
    }

    #[test]
    fn a_dot_segment_directory_name_retires_the_stream() {
        let (binding, hello) = reserved(BTreeSet::from([
            Capability::Connection,
            Capability::DirectoryMetadata,
        ]));
        let (mut connection, mut peer) = live(&binding, &hello);

        let answer = thread::spawn(move || {
            let request: ClientRequest = read_record(&mut peer).unwrap();
            let ClientRequest::ReadDirectory { .. } = request else {
                panic!("expected a directory request");
            };
            write_record(
                &mut peer,
                &ClientReply::Directory {
                    revision: "a".repeat(64),
                    entries: vec![entry("..")],
                    next_offset: None,
                },
            )
            .unwrap();
        });
        let error = connection
            .directory_page(Path::new(""), 0, 16, None)
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        answer.join().unwrap();

        assert_eq!(binding.status().phase, BindingPhase::Detached);
        assert!(connection.ping().is_err());
    }

    fn write_stamp(revision: &str) -> FileStamp {
        FileStamp {
            size: DecimalU64::new(4),
            revision: revision.to_owned(),
            identity: String::from("1:1"),
            modified_at_ms: None,
        }
    }

    fn write_rig() -> (Binding, Hello) {
        reserved(BTreeSet::from([
            Capability::Connection,
            Capability::ConditionalWrite,
        ]))
    }

    fn edit_rig() -> (Binding, Hello) {
        reserved(BTreeSet::from([
            Capability::Connection,
            Capability::ConditionalEdit,
        ]))
    }

    fn replacement(old_text: &str, new_text: &str) -> TextReplacement {
        TextReplacement {
            old_text: old_text.to_owned(),
            new_text: new_text.to_owned(),
        }
    }

    fn assert_outcome_unknown(error: &io::Error) {
        let unknown = match error.get_ref() {
            Some(source) => source.is::<WriteOutcomeUnknown>(),
            None => false,
        };
        assert!(unknown, "{error:?}");
    }

    #[test]
    fn a_peer_without_the_write_capability_gets_no_request() {
        let (binding, hello) = reserved(BTreeSet::from([Capability::Connection]));
        let (mut connection, mut peer) = live(&binding, &hello);

        let error = connection
            .write_file(Path::new("file"), b"bytes", None, |_| Ok(()))
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
        peer.set_read_timeout(Some(Duration::from_millis(200)))
            .unwrap();
        assert!(read_record::<ClientRequest>(&mut peer).is_err());
        assert_eq!(binding.status().phase, BindingPhase::Live);
    }

    #[test]
    fn a_conditional_write_prepares_authorizes_and_commits_on_one_connection() {
        let (binding, hello) = write_rig();
        let (mut connection, mut peer) = live(&binding, &hello);

        let answer = thread::spawn(move || {
            let request: ClientRequest = read_record(&mut peer).unwrap();
            let ClientRequest::PrepareWrite {
                relative_path,
                expectation,
                data_base64,
            } = request
            else {
                panic!("expected a prepare request");
            };
            assert_eq!(relative_path, PathBuf::from("file"));
            assert_eq!(expectation, WriteExpectation::Revision("a".repeat(64)));
            assert_eq!(
                base64::engine::general_purpose::STANDARD
                    .decode(&data_base64)
                    .unwrap(),
                b"data"
            );
            write_record(
                &mut peer,
                &ClientReply::WritePrepared {
                    ticket: DecimalU64::new(1),
                    original: Some(write_stamp(&"a".repeat(64))),
                },
            )
            .unwrap();
            let request: ClientRequest = read_record(&mut peer).unwrap();
            let ClientRequest::CommitWrite { ticket } = request else {
                panic!("expected a commit request");
            };
            assert_eq!(ticket, DecimalU64::new(1));
            write_record(
                &mut peer,
                &ClientReply::Written {
                    ticket,
                    metadata: write_stamp(&"b".repeat(64)),
                },
            )
            .unwrap();
        });
        let authorized = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let observed = authorized.clone();
        let metadata = connection
            .write_file(
                Path::new("file"),
                b"data",
                Some(&"a".repeat(64)),
                |original| {
                    // The gate sees the verified prepared original, not a
                    // re-stat: the exact revision that was requested.
                    assert_eq!(original.unwrap().revision, "a".repeat(64).as_str());
                    observed.store(true, std::sync::atomic::Ordering::Release);
                    Ok(())
                },
            )
            .unwrap();
        assert!(authorized.load(std::sync::atomic::Ordering::Acquire));
        assert_eq!(metadata.revision, "b".repeat(64));
        answer.join().unwrap();
        assert_eq!(binding.status().phase, BindingPhase::Live);
    }

    #[test]
    fn a_prepared_reply_that_ignores_the_expectation_retires_the_stream() {
        let (binding, hello) = write_rig();
        let (mut connection, mut peer) = live(&binding, &hello);

        let answer = thread::spawn(move || {
            let request: ClientRequest = read_record(&mut peer).unwrap();
            assert!(matches!(request, ClientRequest::PrepareWrite { .. }));
            // The peer reports an original that does not satisfy the
            // expected revision it was sent.
            write_record(
                &mut peer,
                &ClientReply::WritePrepared {
                    ticket: DecimalU64::new(1),
                    original: Some(write_stamp(&"b".repeat(64))),
                },
            )
            .unwrap();
        });
        let error = connection
            .write_file(Path::new("file"), b"data", Some(&"a".repeat(64)), |_| {
                panic!("the gate never runs on an untruthful prepare")
            })
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        answer.join().unwrap();
        assert_eq!(binding.status().phase, BindingPhase::Detached);
        assert!(connection.ping().is_err());
    }

    #[test]
    fn a_denied_commit_gate_aborts_the_stage_and_never_commits() {
        let (binding, hello) = write_rig();
        let (mut connection, mut peer) = live(&binding, &hello);

        let answer = thread::spawn(move || {
            let request: ClientRequest = read_record(&mut peer).unwrap();
            assert!(matches!(request, ClientRequest::PrepareWrite { .. }));
            write_record(
                &mut peer,
                &ClientReply::WritePrepared {
                    ticket: DecimalU64::new(1),
                    original: None,
                },
            )
            .unwrap();
            // The only frame after a gate denial is the abort — never a
            // commit.
            let request: ClientRequest = read_record(&mut peer).unwrap();
            let ClientRequest::AbortWrite { ticket } = request else {
                panic!("expected an abort request, never a commit");
            };
            assert_eq!(ticket, DecimalU64::new(1));
            write_record(&mut peer, &ClientReply::WriteAborted { ticket }).unwrap();
        });
        let error = connection
            .write_file(Path::new("file"), b"data", None, |original| {
                assert!(original.is_none());
                Err(io::Error::new(io::ErrorKind::PermissionDenied, "gate"))
            })
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        answer.join().unwrap();
        assert_eq!(binding.status().phase, BindingPhase::Live);
    }

    #[test]
    fn a_lost_commit_acknowledgement_reports_an_unknown_outcome() {
        let (binding, hello) = write_rig();
        let (mut connection, mut peer) = live(&binding, &hello);

        let answer = thread::spawn(move || {
            let request: ClientRequest = read_record(&mut peer).unwrap();
            assert!(matches!(request, ClientRequest::PrepareWrite { .. }));
            write_record(
                &mut peer,
                &ClientReply::WritePrepared {
                    ticket: DecimalU64::new(1),
                    original: Some(write_stamp(&"a".repeat(64))),
                },
            )
            .unwrap();
            // The commit request is consumed and the peer vanishes before
            // its reply: exactly one commit frame, then silence — the
            // publish may or may not have happened and no retry can tell.
            let request: ClientRequest = read_record(&mut peer).unwrap();
            let ClientRequest::CommitWrite { ticket } = request else {
                panic!("expected a commit request");
            };
            assert_eq!(ticket, DecimalU64::new(1));
            peer.shutdown(std::net::Shutdown::Both).unwrap();
        });
        let error = connection
            .write_file(
                Path::new("file"),
                b"data",
                Some(&"a".repeat(64)),
                |_| Ok(()),
            )
            .unwrap_err();
        assert_outcome_unknown(&error);
        answer.join().unwrap();
        assert_eq!(binding.status().phase, BindingPhase::Detached);
        assert!(connection.ping().is_err());
    }

    #[test]
    fn an_explicit_commit_unknown_reports_an_unknown_outcome() {
        let (binding, hello) = write_rig();
        let (mut connection, mut peer) = live(&binding, &hello);

        let answer = thread::spawn(move || {
            let request: ClientRequest = read_record(&mut peer).unwrap();
            assert!(matches!(request, ClientRequest::PrepareWrite { .. }));
            write_record(
                &mut peer,
                &ClientReply::WritePrepared {
                    ticket: DecimalU64::new(1),
                    original: None,
                },
            )
            .unwrap();
            let request: ClientRequest = read_record(&mut peer).unwrap();
            assert!(matches!(request, ClientRequest::CommitWrite { .. }));
            write_record(
                &mut peer,
                &ClientReply::CommitUnknown {
                    ticket: DecimalU64::new(1),
                },
            )
            .unwrap();
        });
        let error = connection
            .write_file(Path::new("file"), b"data", None, |_| Ok(()))
            .unwrap_err();
        assert_outcome_unknown(&error);
        // A correctly-ticketed `CommitUnknown` is valid syntax but never
        // a safe ordinary outcome — the publish may have crossed and the
        // ticket is consumed, so the connection retires like every other
        // post-commit failure.
        answer.join().unwrap();
        assert_eq!(binding.status().phase, BindingPhase::Detached);
        assert!(connection.ping().is_err());
    }

    #[test]
    fn a_written_receipt_with_the_wrong_size_reports_an_unknown_outcome() {
        let (binding, hello) = write_rig();
        let (mut connection, mut peer) = live(&binding, &hello);

        let answer = thread::spawn(move || {
            let request: ClientRequest = read_record(&mut peer).unwrap();
            assert!(matches!(request, ClientRequest::PrepareWrite { .. }));
            write_record(
                &mut peer,
                &ClientReply::WritePrepared {
                    ticket: DecimalU64::new(1),
                    original: None,
                },
            )
            .unwrap();
            let request: ClientRequest = read_record(&mut peer).unwrap();
            assert!(matches!(request, ClientRequest::CommitWrite { .. }));
            // The ticket is right and the stamp is well-formed, but the
            // size describes different bytes than were submitted: the
            // publish cannot be proven, so the outcome is unknown.
            write_record(
                &mut peer,
                &ClientReply::Written {
                    ticket: DecimalU64::new(1),
                    metadata: FileStamp {
                        size: DecimalU64::new(3),
                        revision: "b".repeat(64),
                        identity: String::from("1:1"),
                        modified_at_ms: None,
                    },
                },
            )
            .unwrap();
        });
        let error = connection
            .write_file(Path::new("file"), b"data", None, |_| Ok(()))
            .unwrap_err();
        assert_outcome_unknown(&error);
        answer.join().unwrap();
        assert_eq!(binding.status().phase, BindingPhase::Detached);
        assert!(connection.ping().is_err());
    }

    #[test]
    fn a_mismatched_commit_ticket_reports_an_unknown_outcome() {
        let (binding, hello) = write_rig();
        let (mut connection, mut peer) = live(&binding, &hello);

        let answer = thread::spawn(move || {
            let request: ClientRequest = read_record(&mut peer).unwrap();
            assert!(matches!(request, ClientRequest::PrepareWrite { .. }));
            write_record(
                &mut peer,
                &ClientReply::WritePrepared {
                    ticket: DecimalU64::new(1),
                    original: None,
                },
            )
            .unwrap();
            let request: ClientRequest = read_record(&mut peer).unwrap();
            assert!(matches!(request, ClientRequest::CommitWrite { .. }));
            // A `Written` under a foreign ticket after the commit byte is
            // unknowable — the reply accounting can never be trusted.
            write_record(
                &mut peer,
                &ClientReply::Written {
                    ticket: DecimalU64::new(9),
                    metadata: write_stamp(&"b".repeat(64)),
                },
            )
            .unwrap();
        });
        let error = connection
            .write_file(Path::new("file"), b"data", None, |_| Ok(()))
            .unwrap_err();
        assert_outcome_unknown(&error);
        answer.join().unwrap();
        assert_eq!(binding.status().phase, BindingPhase::Detached);
        assert!(connection.ping().is_err());
    }

    #[test]
    fn a_prepared_mutation_never_crosses_to_another_connection() {
        let (binding_a, hello_a) = write_rig();
        let (mut connection_a, peer_a) = live(&binding_a, &hello_a);
        let (binding_b, hello_b) = write_rig();
        let (mut connection_b, peer_b) = live(&binding_b, &hello_b);

        let script = |mut peer: UnixStream| {
            thread::spawn(move || {
                let request: ClientRequest = read_record(&mut peer).unwrap();
                assert!(matches!(request, ClientRequest::PrepareWrite { .. }));
                write_record(
                    &mut peer,
                    &ClientReply::WritePrepared {
                        ticket: DecimalU64::new(1),
                        original: None,
                    },
                )
                .unwrap();
                // The foreign-ticket refusals place no frame at all: the
                // one and only request after the prepare is the
                // connection's own commit — a refused call that still
                // touched the wire would arrive here first and fail the
                // shape check, or arrive later and fail the tail check.
                let request: ClientRequest = read_record(&mut peer).unwrap();
                let ClientRequest::CommitWrite { ticket } = request else {
                    panic!("expected a commit request");
                };
                assert_eq!(ticket, DecimalU64::new(1));
                write_record(
                    &mut peer,
                    &ClientReply::Written {
                        ticket,
                        metadata: write_stamp(&"b".repeat(64)),
                    },
                )
                .unwrap();
                peer.set_read_timeout(Some(Duration::from_millis(200)))
                    .unwrap();
                assert!(read_record::<ClientRequest>(&mut peer).is_err());
            })
        };
        let answer_a = script(peer_a);
        let answer_b = script(peer_b);

        let admit = || Ok(());
        let sent = || ();
        let prepared_a = connection_a
            .prepare_write(
                Path::new("file"),
                &WriteExpectation::Current,
                b"data",
                None,
                &admit,
                &sent,
            )
            .unwrap();
        let prepared_b = connection_b
            .prepare_write(
                Path::new("file"),
                &WriteExpectation::Current,
                b"data",
                None,
                &admit,
                &sent,
            )
            .unwrap();
        // Both peers issued ticket 1 — on different connections.
        assert_eq!(prepared_a.ticket, prepared_b.ticket);

        let error = connection_a
            .commit_prepared(&prepared_b, None, &admit, &sent)
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        let error = connection_b
            .abort_prepared(&prepared_a, None, &admit, &sent)
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);

        // Neither preparation was spent by the refused cross-connection
        // calls: each connection still commits its own ticket.
        connection_a
            .commit_prepared(&prepared_a, None, &admit, &sent)
            .unwrap();
        connection_b
            .commit_prepared(&prepared_b, None, &admit, &sent)
            .unwrap();
        answer_a.join().unwrap();
        answer_b.join().unwrap();
        assert_eq!(binding_a.status().phase, BindingPhase::Live);
        assert_eq!(binding_b.status().phase, BindingPhase::Live);
    }

    #[test]
    fn a_revoked_binding_after_prepare_sends_no_commit_frame() {
        let (binding, hello) = write_rig();
        let (mut connection, mut peer) = live(&binding, &hello);

        let answer = thread::spawn(move || {
            let request: ClientRequest = read_record(&mut peer).unwrap();
            assert!(matches!(request, ClientRequest::PrepareWrite { .. }));
            write_record(
                &mut peer,
                &ClientReply::WritePrepared {
                    ticket: DecimalU64::new(1),
                    original: None,
                },
            )
            .unwrap();
            peer
        });
        let error = connection
            .write_file(Path::new("file"), b"data", None, |_| {
                // The caller's last gate revokes the binding between the
                // prepared reply and the commit dispatch: the commit
                // frame is gated out before its first byte.
                binding.revoke();
                Ok(())
            })
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        let mut peer = answer.join().unwrap();
        // Zero commit frames: the revoke shutdown already closed the
        // physical stream, so the peer observes EOF — never a request.
        let mut buffer = [0u8; 16];
        assert_eq!(peer.read(&mut buffer).unwrap(), 0);
        assert_eq!(binding.status().phase, BindingPhase::Revoked);
    }

    fn lines_rig() -> (Binding, Hello) {
        reserved(BTreeSet::from([
            Capability::Connection,
            Capability::ReadLines,
        ]))
    }

    fn lines_stamp(size: u64, revision: &str) -> FileStamp {
        FileStamp {
            size: DecimalU64::new(size),
            revision: revision.to_owned(),
            identity: String::from("1:1"),
            modified_at_ms: None,
        }
    }

    /// A `Lines` reply shape the validator must accept: six source bytes
    /// as `"a\nb\nc"`, three addressable lines, one shown element.
    fn lines_reply(shown: &str, shown_lines: u32, total: u64, end: LineEnd) -> ClientReply {
        ClientReply::Lines {
            metadata: lines_stamp(6, &"a".repeat(64)),
            text: shown.to_owned(),
            total_lines: DecimalU64::new(total),
            shown_lines,
            end,
        }
    }

    #[test]
    fn a_peer_without_the_lines_capability_gets_no_request() {
        let (binding, hello) = reserved(BTreeSet::from([Capability::Connection]));
        let (mut connection, mut peer) = live(&binding, &hello);
        let error = connection
            .read_lines(Path::new("file"), 1, None, None)
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
        peer.set_read_timeout(Some(Duration::from_millis(200)))
            .unwrap();
        assert!(read_record::<ClientRequest>(&mut peer).is_err());
        peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let answer = thread::spawn(move || {
            let request: ClientRequest = read_record(&mut peer).unwrap();
            assert_eq!(request, ClientRequest::Ping);
            write_record(&mut peer, &ClientReply::Pong).unwrap();
        });
        connection.ping().unwrap();
        answer.join().unwrap();
        assert_eq!(binding.status().phase, BindingPhase::Live);
    }

    #[test]
    fn a_zero_lines_request_never_reaches_the_wire() {
        let (binding, hello) = lines_rig();
        let (mut connection, mut peer) = live(&binding, &hello);
        for (offset, limit) in [(0, None), (1, Some(0))] {
            let error = connection
                .read_lines(Path::new("file"), offset, limit, None)
                .unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        }
        peer.set_read_timeout(Some(Duration::from_millis(200)))
            .unwrap();
        assert!(read_record::<ClientRequest>(&mut peer).is_err());
        peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let answer = thread::spawn(move || {
            let request: ClientRequest = read_record(&mut peer).unwrap();
            assert_eq!(request, ClientRequest::Ping);
            write_record(&mut peer, &ClientReply::Pong).unwrap();
        });
        connection.ping().unwrap();
        answer.join().unwrap();
        assert_eq!(binding.status().phase, BindingPhase::Live);
    }

    #[test]
    fn a_lines_page_is_verified_against_the_request() {
        let (binding, hello) = lines_rig();
        let (mut connection, mut peer) = live(&binding, &hello);
        let expected = "a".repeat(64);
        let answer = thread::spawn(move || {
            let request: ClientRequest = read_record(&mut peer).unwrap();
            let ClientRequest::ReadLines {
                relative_path,
                offset,
                limit,
                expected_revision,
            } = request
            else {
                panic!("expected a lines request");
            };
            assert_eq!(relative_path, PathBuf::from("file"));
            assert_eq!(offset.get(), 2);
            assert_eq!(limit, Some(DecimalU64::new(1)));
            assert_eq!(expected_revision.as_deref(), Some(expected.as_str()));
            write_record(&mut peer, &lines_reply("b", 1, 3, LineEnd::RequestedLimit)).unwrap();
        });
        let page = connection
            .read_lines(Path::new("file"), 2, Some(1), Some(&"a".repeat(64)))
            .unwrap();
        answer.join().unwrap();
        assert_eq!(page.text, "b");
        assert_eq!(page.total_lines, 3);
        assert_eq!(page.shown_lines, 1);
        assert_eq!(page.end, LineEnd::RequestedLimit);
        assert_eq!(binding.status().phase, BindingPhase::Live);
    }

    #[test]
    fn a_complete_lines_page_may_show_the_trailing_empty_line() {
        let (binding, hello) = lines_rig();
        let (mut connection, mut peer) = live(&binding, &hello);
        let answer = thread::spawn(move || {
            let _: ClientRequest = read_record(&mut peer).unwrap();
            // A source ending in a newline keeps its trailing empty
            // element on a complete page: `"a\n"` shows 2 elements.
            write_record(
                &mut peer,
                &ClientReply::Lines {
                    metadata: lines_stamp(2, &"a".repeat(64)),
                    text: String::from("a\n"),
                    total_lines: DecimalU64::new(2),
                    shown_lines: 2,
                    end: LineEnd::Complete,
                },
            )
            .unwrap();
        });
        let page = connection
            .read_lines(Path::new("file"), 1, None, None)
            .unwrap();
        answer.join().unwrap();
        assert_eq!(page.text, "a\n");
        assert_eq!(page.shown_lines, 2);
        assert_eq!(page.end, LineEnd::Complete);
        assert_eq!(binding.status().phase, BindingPhase::Live);
    }

    #[test]
    fn an_untruthful_lines_end_retires_the_stream() {
        let (binding, hello) = lines_rig();
        let (mut connection, mut peer) = live(&binding, &hello);
        let answer = thread::spawn(move || {
            let _: ClientRequest = read_record(&mut peer).unwrap();
            // "Complete" while lines remain: the end reason cannot
            // account for the page.
            write_record(&mut peer, &lines_reply("a", 1, 3, LineEnd::Complete)).unwrap();
        });
        let error = connection
            .read_lines(Path::new("file"), 1, None, None)
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        answer.join().unwrap();
        assert_eq!(binding.status().phase, BindingPhase::Detached);
        assert!(connection.ping().is_err());
    }

    #[test]
    fn a_lines_reply_whose_text_contradicts_shown_retires_the_stream() {
        let (binding, hello) = lines_rig();
        let (mut connection, mut peer) = live(&binding, &hello);
        let answer = thread::spawn(move || {
            let _: ClientRequest = read_record(&mut peer).unwrap();
            // `shown` claims two elements; the text holds one.
            write_record(&mut peer, &lines_reply("a", 2, 3, LineEnd::LineLimit)).unwrap();
        });
        let error = connection
            .read_lines(Path::new("file"), 1, None, None)
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        answer.join().unwrap();
        assert_eq!(binding.status().phase, BindingPhase::Detached);
        assert!(connection.ping().is_err());
    }

    #[test]
    fn a_lines_reply_with_zero_total_retires_the_stream() {
        let (binding, hello) = lines_rig();
        let (mut connection, mut peer) = live(&binding, &hello);
        let answer = thread::spawn(move || {
            let _: ClientRequest = read_record(&mut peer).unwrap();
            // Even an empty source addresses one line: zero is impossible.
            write_record(&mut peer, &lines_reply("", 0, 0, LineEnd::OffsetPastEnd)).unwrap();
        });
        let error = connection
            .read_lines(Path::new("file"), 5, None, None)
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        answer.join().unwrap();
        assert_eq!(binding.status().phase, BindingPhase::Detached);
        assert!(connection.ping().is_err());
    }

    #[test]
    fn a_lines_reply_with_a_stale_revision_retires_the_stream() {
        let (binding, hello) = lines_rig();
        let (mut connection, mut peer) = live(&binding, &hello);
        let answer = thread::spawn(move || {
            let _: ClientRequest = read_record(&mut peer).unwrap();
            write_record(
                &mut peer,
                &ClientReply::Lines {
                    metadata: lines_stamp(6, &"b".repeat(64)),
                    text: String::from("a"),
                    total_lines: DecimalU64::new(1),
                    shown_lines: 1,
                    end: LineEnd::Complete,
                },
            )
            .unwrap();
        });
        let error = connection
            .read_lines(Path::new("file"), 1, None, Some(&"a".repeat(64)))
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        answer.join().unwrap();
        assert_eq!(binding.status().phase, BindingPhase::Detached);
        assert!(connection.ping().is_err());
    }

    #[test]
    fn a_lines_reply_at_the_offset_space_edge_does_not_panic() {
        let (binding, hello) = lines_rig();
        let (mut connection, mut peer) = live(&binding, &hello);
        let answer = thread::spawn(move || {
            let request: ClientRequest = read_record(&mut peer).unwrap();
            let ClientRequest::ReadLines { offset, .. } = request else {
                panic!("expected a lines request");
            };
            assert_eq!(offset.get(), u64::MAX);
            // `u64::MAX` is a legitimate out-of-range offset: the honest
            // `OffsetPastEnd` reply must survive validation — arithmetic
            // that overflowed here would panic, not retire.
            write_record(&mut peer, &lines_reply("", 0, 2, LineEnd::OffsetPastEnd)).unwrap();
        });
        let page = connection
            .read_lines(Path::new("file"), u64::MAX, None, None)
            .unwrap();
        answer.join().unwrap();
        assert_eq!(page.end, LineEnd::OffsetPastEnd);
        assert_eq!(binding.status().phase, BindingPhase::Live);
    }

    #[test]
    fn a_complete_lines_reply_that_violates_the_limit_retires() {
        let (binding, hello) = lines_rig();
        let (mut connection, mut peer) = live(&binding, &hello);
        let answer = thread::spawn(move || {
            let _: ClientRequest = read_record(&mut peer).unwrap();
            // `Complete` claims the window reached the source end, but
            // the request asked for one line and two arrived — the
            // accounting cannot be trusted.
            write_record(&mut peer, &lines_reply("a\nb", 2, 3, LineEnd::Complete)).unwrap();
        });
        let error = connection
            .read_lines(Path::new("file"), 1, Some(1), None)
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        answer.join().unwrap();
        assert_eq!(binding.status().phase, BindingPhase::Detached);
        assert!(connection.ping().is_err());
    }

    #[test]
    fn a_budget_stop_that_consumed_the_window_retires() {
        let (binding, hello) = lines_rig();
        let (mut connection, mut peer) = live(&binding, &hello);
        let answer = thread::spawn(move || {
            let _: ClientRequest = read_record(&mut peer).unwrap();
            // `ByteLimit` claims a budget stop, yet the shown count
            // consumed the whole requested window — nothing was left
            // for a budget to stop on.
            write_record(
                &mut peer,
                &lines_reply("a\nb\nc\nd\ne", 5, 10, LineEnd::ByteLimit),
            )
            .unwrap();
        });
        let error = connection
            .read_lines(Path::new("file"), 1, Some(5), None)
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        answer.join().unwrap();
        assert_eq!(binding.status().phase, BindingPhase::Detached);
        assert!(connection.ping().is_err());
    }

    #[test]
    fn a_2001_element_page_without_the_trailing_newline_retires() {
        let (binding, hello) = lines_rig();
        let (mut connection, mut peer) = live(&binding, &hello);
        let answer = thread::spawn(move || {
            let _: ClientRequest = read_record(&mut peer).unwrap();
            // `shown_lines` may exceed `MAX_TEXT_LINES` only for the
            // trailing-empty page: 2001 nonempty elements is a lie.
            let text = "x\n".repeat(MAX_TEXT_LINES) + "x";
            write_record(
                &mut peer,
                &ClientReply::Lines {
                    metadata: lines_stamp(5_000_000, &"a".repeat(64)),
                    text,
                    total_lines: DecimalU64::new(2_001),
                    shown_lines: u32::try_from(MAX_TEXT_LINES + 1).unwrap(),
                    end: LineEnd::Complete,
                },
            )
            .unwrap();
        });
        let error = connection
            .read_lines(Path::new("file"), 1, None, None)
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        answer.join().unwrap();
        assert_eq!(binding.status().phase, BindingPhase::Detached);
        assert!(connection.ping().is_err());
    }

    #[test]
    fn a_requested_limit_may_keep_the_trailing_empty_element() {
        let (binding, hello) = lines_rig();
        let (mut connection, mut peer) = live(&binding, &hello);
        let answer = thread::spawn(move || {
            let _: ClientRequest = read_record(&mut peer).unwrap();
            // The file `("x\n" * 2000) + "\nrest"`: the caller's limit
            // selects the empty element after the 2000th line — exempt
            // from the budgets exactly like the source's own trailing
            // empty, so 2001 shown elements ending in a newline is a
            // legitimate `RequestedLimit` page.
            write_record(
                &mut peer,
                &ClientReply::Lines {
                    metadata: lines_stamp(4_005, &"a".repeat(64)),
                    text: "x\n".repeat(MAX_TEXT_LINES),
                    total_lines: DecimalU64::new(u64::try_from(MAX_TEXT_LINES + 2).unwrap()),
                    shown_lines: u32::try_from(MAX_TEXT_LINES + 1).unwrap(),
                    end: LineEnd::RequestedLimit,
                },
            )
            .unwrap();
        });
        let page = connection
            .read_lines(
                Path::new("file"),
                1,
                Some(u64::try_from(MAX_TEXT_LINES + 1).unwrap()),
                None,
            )
            .unwrap();
        answer.join().unwrap();
        assert_eq!(page.shown_lines, u32::try_from(MAX_TEXT_LINES + 1).unwrap());
        assert!(page.text.ends_with('\n'));
        assert_eq!(page.end, LineEnd::RequestedLimit);
        assert_eq!(binding.status().phase, BindingPhase::Live);
    }

    #[test]
    fn an_invalid_utf8_error_keeps_the_connection_live() {
        let (binding, hello) = lines_rig();
        let (mut connection, mut peer) = live(&binding, &hello);
        let answer = thread::spawn(move || {
            let _: ClientRequest = read_record(&mut peer).unwrap();
            write_record(
                &mut peer,
                &ClientReply::Error {
                    kind: ReadError::InvalidUtf8,
                },
            )
            .unwrap();
            let request: ClientRequest = read_record(&mut peer).unwrap();
            assert_eq!(request, ClientRequest::Ping);
            write_record(&mut peer, &ClientReply::Pong).unwrap();
        });
        // The semantic refusal maps to InvalidData but never retires:
        // the same connection still answers Ping afterward.
        let error = connection
            .read_lines(Path::new("file"), 1, None, None)
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        connection.ping().unwrap();
        answer.join().unwrap();
        assert_eq!(binding.status().phase, BindingPhase::Live);
    }

    fn document_rig() -> (Binding, Hello) {
        reserved(BTreeSet::from([
            Capability::Connection,
            Capability::DocumentPages,
        ]))
    }

    /// A `DocumentPage` reply shape the validator must accept: a 512-byte
    /// compressed source decoding to a six-byte `"a\nb\nc"` Markdown
    /// whose logical length — not the source size — carries the page
    /// accounting.
    fn document_reply(
        logical_bytes: u64,
        text: &str,
        shown_lines: u32,
        total: u64,
        end: LineEnd,
    ) -> ClientReply {
        ClientReply::DocumentPage {
            source: lines_stamp(512, &"a".repeat(64)),
            logical_bytes: DecimalU64::new(logical_bytes),
            text: text.to_owned(),
            total_lines: DecimalU64::new(total),
            shown_lines,
            end,
        }
    }

    #[test]
    fn a_peer_without_the_document_capability_gets_no_request() {
        let (binding, hello) = reserved(BTreeSet::from([Capability::Connection]));
        let (mut connection, mut peer) = live(&binding, &hello);
        let error = connection
            .read_document(Path::new("report.pdf"), 1, None, None)
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
        peer.set_read_timeout(Some(Duration::from_millis(200)))
            .unwrap();
        assert!(read_record::<ClientRequest>(&mut peer).is_err());
        peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let answer = thread::spawn(move || {
            let request: ClientRequest = read_record(&mut peer).unwrap();
            assert_eq!(request, ClientRequest::Ping);
            write_record(&mut peer, &ClientReply::Pong).unwrap();
        });
        connection.ping().unwrap();
        answer.join().unwrap();
        assert_eq!(binding.status().phase, BindingPhase::Live);
    }

    #[test]
    fn a_document_page_is_verified_against_the_request() {
        let (binding, hello) = document_rig();
        let (mut connection, mut peer) = live(&binding, &hello);
        let expected = "a".repeat(64);
        let answer = thread::spawn(move || {
            let request: ClientRequest = read_record(&mut peer).unwrap();
            let ClientRequest::ReadDocument {
                relative_path,
                offset,
                limit,
                expected_revision,
            } = request
            else {
                panic!("expected a document request");
            };
            assert_eq!(relative_path, PathBuf::from("report.pdf"));
            assert_eq!(offset.get(), 2);
            assert_eq!(limit, Some(DecimalU64::new(1)));
            assert_eq!(expected_revision.as_deref(), Some(expected.as_str()));
            // The decoded text is longer than the page and independent
            // of the compressed source's stamped size.
            write_record(
                &mut peer,
                &document_reply(900, "b", 1, 3, LineEnd::RequestedLimit),
            )
            .unwrap();
        });
        let page = connection
            .read_document(Path::new("report.pdf"), 2, Some(1), Some(&"a".repeat(64)))
            .unwrap();
        answer.join().unwrap();
        assert_eq!(page.text, "b");
        assert_eq!(page.total_lines, 3);
        assert_eq!(page.shown_lines, 1);
        assert_eq!(page.end, LineEnd::RequestedLimit);
        assert_eq!(page.logical_bytes, 900);
        assert_eq!(page.source.size.get(), 512);
        assert_eq!(binding.status().phase, BindingPhase::Live);
    }

    #[test]
    fn a_wrong_document_reply_kind_retires_the_stream() {
        let (binding, hello) = document_rig();
        let (mut connection, mut peer) = live(&binding, &hello);
        let answer = thread::spawn(move || {
            let _: ClientRequest = read_record(&mut peer).unwrap();
            // A `Lines` reply to a `ReadDocument` request is a protocol
            // violation: the reply accounting cannot be trusted.
            write_record(&mut peer, &lines_reply("a", 1, 1, LineEnd::Complete)).unwrap();
        });
        let error = connection
            .read_document(Path::new("report.pdf"), 1, None, None)
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        answer.join().unwrap();
        assert_eq!(binding.status().phase, BindingPhase::Detached);
        assert!(connection.ping().is_err());
    }

    #[test]
    fn a_document_reply_with_a_conflicting_source_retires_the_stream() {
        let (binding, hello) = document_rig();
        let (mut connection, mut peer) = live(&binding, &hello);
        let answer = thread::spawn(move || {
            let _: ClientRequest = read_record(&mut peer).unwrap();
            write_record(
                &mut peer,
                &ClientReply::DocumentPage {
                    source: lines_stamp(512, &"b".repeat(64)),
                    logical_bytes: DecimalU64::new(1),
                    text: String::from("a"),
                    total_lines: DecimalU64::new(1),
                    shown_lines: 1,
                    end: LineEnd::Complete,
                },
            )
            .unwrap();
        });
        let error = connection
            .read_document(Path::new("report.pdf"), 1, None, Some(&"a".repeat(64)))
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        answer.join().unwrap();
        assert_eq!(binding.status().phase, BindingPhase::Detached);
        assert!(connection.ping().is_err());
    }

    #[test]
    fn a_document_reply_past_the_bounds_retires_the_stream() {
        let (binding, hello) = document_rig();
        let (mut connection, mut peer) = live(&binding, &hello);
        let answer = thread::spawn(move || {
            // A source the Client could never have decoded and a
            // projection beyond the decoder's own cap are both
            // untruthful — one exchange each, then the peer drains.
            let _: ClientRequest = read_record(&mut peer).unwrap();
            write_record(
                &mut peer,
                &ClientReply::DocumentPage {
                    source: lines_stamp(
                        u64::try_from(MAX_DOCUMENT_SOURCE_BYTES + 1).unwrap(),
                        &"a".repeat(64),
                    ),
                    logical_bytes: DecimalU64::new(1),
                    text: String::from("a"),
                    total_lines: DecimalU64::new(1),
                    shown_lines: 1,
                    end: LineEnd::Complete,
                },
            )
            .unwrap();
        });
        let error = connection
            .read_document(Path::new("report.pdf"), 1, None, None)
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        answer.join().unwrap();
        assert_eq!(binding.status().phase, BindingPhase::Detached);
        assert!(connection.ping().is_err());
    }

    #[test]
    fn a_document_reply_past_the_logical_bound_retires_the_stream() {
        let (binding, hello) = document_rig();
        let (mut connection, mut peer) = live(&binding, &hello);
        let answer = thread::spawn(move || {
            let _: ClientRequest = read_record(&mut peer).unwrap();
            write_record(
                &mut peer,
                &document_reply(MAX_DOCUMENT_LOGICAL_BYTES + 1, "a", 1, 1, LineEnd::Complete),
            )
            .unwrap();
        });
        let error = connection
            .read_document(Path::new("report.pdf"), 1, None, None)
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        answer.join().unwrap();
        assert_eq!(binding.status().phase, BindingPhase::Detached);
        assert!(connection.ping().is_err());
    }

    #[test]
    fn a_document_reply_shares_the_lines_accounting() {
        let (binding, hello) = document_rig();
        let (mut connection, mut peer) = live(&binding, &hello);
        let answer = thread::spawn(move || {
            let _: ClientRequest = read_record(&mut peer).unwrap();
            // `Complete` claims the window reached the end of the
            // decoded text, but the request asked for one line and two
            // arrived — the shared accounting catches it.
            write_record(
                &mut peer,
                &document_reply(900, "a\nb", 2, 3, LineEnd::Complete),
            )
            .unwrap();
        });
        let error = connection
            .read_document(Path::new("report.pdf"), 1, Some(1), None)
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        answer.join().unwrap();
        assert_eq!(binding.status().phase, BindingPhase::Detached);
        assert!(connection.ping().is_err());
    }

    #[test]
    fn a_document_semantic_error_keeps_the_connection_live() {
        let (binding, hello) = document_rig();
        let (mut connection, mut peer) = live(&binding, &hello);
        let answer = thread::spawn(move || {
            for kind in [
                ReadError::NoTextLayer,
                ReadError::DocumentDecodeFailed,
                ReadError::UnsupportedDocument,
                ReadError::DocumentTooLarge,
            ] {
                let _: ClientRequest = read_record(&mut peer).unwrap();
                write_record(&mut peer, &ClientReply::Error { kind }).unwrap();
            }
            let request: ClientRequest = read_record(&mut peer).unwrap();
            assert_eq!(request, ClientRequest::Ping);
            write_record(&mut peer, &ClientReply::Pong).unwrap();
        });
        // Every document refusal is a distinct stable kind — never
        // conflated — and none retires a healthy connection.
        assert_eq!(
            connection
                .read_document(Path::new("scanned.pdf"), 1, None, None)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(
            connection
                .read_document(Path::new("broken.docx"), 1, None, None)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(
            connection
                .read_document(Path::new("notes.md"), 1, None, None)
                .unwrap_err()
                .kind(),
            io::ErrorKind::Unsupported
        );
        assert_eq!(
            connection
                .read_document(Path::new("huge.xlsx"), 1, None, None)
                .unwrap_err()
                .kind(),
            io::ErrorKind::FileTooLarge
        );
        connection.ping().unwrap();
        answer.join().unwrap();
        assert_eq!(binding.status().phase, BindingPhase::Live);
    }

    #[test]
    fn a_conditional_edit_prepares_authorizes_and_commits() {
        let (binding, hello) = edit_rig();
        let (mut connection, mut peer) = live(&binding, &hello);
        let revision = "a".repeat(64);

        let answer = thread::spawn(move || {
            let request: ClientRequest = read_record(&mut peer).unwrap();
            let ClientRequest::PrepareEdit {
                relative_path,
                expectation,
                replacements,
            } = request
            else {
                panic!("expected a prepare-edit request");
            };
            assert_eq!(relative_path, Path::new("file"));
            assert_eq!(expectation, WriteExpectation::Revision("a".repeat(64)));
            assert_eq!(replacements.len(), 1);
            write_record(
                &mut peer,
                &ClientReply::EditPrepared {
                    ticket: DecimalU64::new(1),
                    original: write_stamp(&"a".repeat(64)),
                    published_size: DecimalU64::new(5),
                },
            )
            .unwrap();
            let request: ClientRequest = read_record(&mut peer).unwrap();
            let ClientRequest::CommitWrite { ticket } = request else {
                panic!("expected a commit request");
            };
            assert_eq!(ticket, DecimalU64::new(1));
            write_record(
                &mut peer,
                &ClientReply::Written {
                    ticket,
                    metadata: FileStamp {
                        size: DecimalU64::new(5),
                        revision: "b".repeat(64),
                        identity: String::from("1:1"),
                        modified_at_ms: None,
                    },
                },
            )
            .unwrap();
        });
        let published = connection
            .edit_file(
                Path::new("file"),
                &revision,
                &[replacement("old", "newer")],
                |original| {
                    assert_eq!(original.revision, "a".repeat(64));
                    Ok(())
                },
            )
            .unwrap();
        assert_eq!(published.size.get(), 5);
        answer.join().unwrap();
        assert_eq!(binding.status().phase, BindingPhase::Live);
    }

    #[test]
    fn an_edit_refusal_projects_the_typed_reason_and_stays_live() {
        let (binding, hello) = edit_rig();
        let (mut connection, mut peer) = live(&binding, &hello);

        let answer = thread::spawn(move || {
            let request: ClientRequest = read_record(&mut peer).unwrap();
            assert!(matches!(request, ClientRequest::PrepareEdit { .. }));
            write_record(
                &mut peer,
                &ClientReply::EditRefused {
                    reason: EditRefusal::Ambiguous {
                        index: 0,
                        occurrences: 3,
                    },
                },
            )
            .unwrap();
            let request: ClientRequest = read_record(&mut peer).unwrap();
            assert_eq!(request, ClientRequest::Ping);
            write_record(&mut peer, &ClientReply::Pong).unwrap();
        });
        let error = connection
            .edit_file(
                Path::new("file"),
                &"a".repeat(64),
                &[replacement("x", "y")],
                |_| panic!("a semantic refusal never reaches the gate"),
            )
            .unwrap_err();
        let refused = error.get_ref().unwrap().downcast_ref::<RefusedEdit>();
        assert_eq!(
            refused.map(|refused| refused.0),
            Some(EditRefusal::Ambiguous {
                index: 0,
                occurrences: 3
            })
        );
        connection.ping().unwrap();
        answer.join().unwrap();
        assert_eq!(binding.status().phase, BindingPhase::Live);
    }

    #[test]
    fn a_contradicting_edit_refusal_retires_the_connection() {
        let (binding, hello) = edit_rig();
        let (mut connection, mut peer) = live(&binding, &hello);

        let answer = thread::spawn(move || {
            let request: ClientRequest = read_record(&mut peer).unwrap();
            assert!(matches!(request, ClientRequest::PrepareEdit { .. }));
            // Index 4 names a replacement the request never carried.
            write_record(
                &mut peer,
                &ClientReply::EditRefused {
                    reason: EditRefusal::Missing { index: 4 },
                },
            )
            .unwrap();
        });
        let error = connection
            .edit_file(
                Path::new("file"),
                &"a".repeat(64),
                &[replacement("x", "y")],
                |_| panic!("a malformed refusal never reaches the gate"),
            )
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        answer.join().unwrap();
        assert_eq!(binding.status().phase, BindingPhase::Detached);
    }

    #[test]
    fn an_edit_prepared_reply_is_validated_before_the_gate() {
        let (binding, hello) = edit_rig();
        let (mut connection, mut peer) = live(&binding, &hello);

        let answer = thread::spawn(move || {
            let request: ClientRequest = read_record(&mut peer).unwrap();
            assert!(matches!(request, ClientRequest::PrepareEdit { .. }));
            // A published size above the file cap can never be committed.
            write_record(
                &mut peer,
                &ClientReply::EditPrepared {
                    ticket: DecimalU64::new(1),
                    original: write_stamp(&"a".repeat(64)),
                    published_size: DecimalU64::new(MAX_EDIT_FILE_BYTES as u64 + 1),
                },
            )
            .unwrap();
        });
        let error = connection
            .edit_file(
                Path::new("file"),
                &"a".repeat(64),
                &[replacement("x", "y")],
                |_| panic!("an oversized prepare never reaches the gate"),
            )
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        answer.join().unwrap();
        assert_eq!(binding.status().phase, BindingPhase::Detached);
    }

    #[test]
    fn a_denied_edit_gate_aborts_under_the_same_deadline() {
        let (binding, hello) = edit_rig();
        let (mut connection, mut peer) = live(&binding, &hello);

        let answer = thread::spawn(move || {
            let request: ClientRequest = read_record(&mut peer).unwrap();
            assert!(matches!(request, ClientRequest::PrepareEdit { .. }));
            write_record(
                &mut peer,
                &ClientReply::EditPrepared {
                    ticket: DecimalU64::new(1),
                    original: write_stamp(&"a".repeat(64)),
                    published_size: DecimalU64::new(5),
                },
            )
            .unwrap();
            let request: ClientRequest = read_record(&mut peer).unwrap();
            let ClientRequest::AbortWrite { ticket } = request else {
                panic!("expected an abort request, never a commit");
            };
            write_record(&mut peer, &ClientReply::WriteAborted { ticket }).unwrap();
        });
        let error = connection
            .edit_file(
                Path::new("file"),
                &"a".repeat(64),
                &[replacement("x", "y")],
                |_| Err(io::Error::new(io::ErrorKind::PermissionDenied, "gate")),
            )
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        answer.join().unwrap();
        assert_eq!(binding.status().phase, BindingPhase::Live);
    }

    #[test]
    fn an_edit_commit_unknown_retires_the_connection() {
        let (binding, hello) = edit_rig();
        let (mut connection, mut peer) = live(&binding, &hello);

        let answer = thread::spawn(move || {
            let request: ClientRequest = read_record(&mut peer).unwrap();
            assert!(matches!(request, ClientRequest::PrepareEdit { .. }));
            write_record(
                &mut peer,
                &ClientReply::EditPrepared {
                    ticket: DecimalU64::new(1),
                    original: write_stamp(&"a".repeat(64)),
                    published_size: DecimalU64::new(5),
                },
            )
            .unwrap();
            let request: ClientRequest = read_record(&mut peer).unwrap();
            assert!(matches!(request, ClientRequest::CommitWrite { .. }));
            write_record(
                &mut peer,
                &ClientReply::CommitUnknown {
                    ticket: DecimalU64::new(1),
                },
            )
            .unwrap();
        });
        let error = connection
            .edit_file(
                Path::new("file"),
                &"a".repeat(64),
                &[replacement("x", "y")],
                |_| Ok(()),
            )
            .unwrap_err();
        assert_outcome_unknown(&error);
        answer.join().unwrap();
        assert_eq!(binding.status().phase, BindingPhase::Detached);
        assert!(connection.ping().is_err());
    }
    #[test]
    fn an_over_cap_edit_original_retires_before_the_gate() {
        let (binding, hello) = edit_rig();
        let (mut connection, mut peer) = live(&binding, &hello);
        let authorized = std::sync::atomic::AtomicUsize::new(0);

        let answer = thread::spawn(move || {
            let request: ClientRequest = read_record(&mut peer).unwrap();
            assert!(matches!(request, ClientRequest::PrepareEdit { .. }));
            // A valid-looking stamp claiming a source the Client could
            // never have decoded.
            write_record(
                &mut peer,
                &ClientReply::EditPrepared {
                    ticket: DecimalU64::new(1),
                    original: FileStamp {
                        size: DecimalU64::new(MAX_EDIT_FILE_BYTES as u64 + 1),
                        revision: "a".repeat(64),
                        identity: String::from("1:1"),
                        modified_at_ms: None,
                    },
                    published_size: DecimalU64::new(4),
                },
            )
            .unwrap();
        });
        let error = connection
            .edit_file(
                Path::new("file"),
                &"a".repeat(64),
                &[replacement("x", "y")],
                |_| {
                    authorized.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    Ok(())
                },
            )
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        answer.join().unwrap();
        assert_eq!(authorized.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(binding.status().phase, BindingPhase::Detached);
    }

    #[test]
    fn an_empty_anchor_refusal_on_a_nonempty_anchor_retires() {
        let (binding, hello) = edit_rig();
        let (mut connection, mut peer) = live(&binding, &hello);
        let authorized = std::sync::atomic::AtomicUsize::new(0);

        let answer = thread::spawn(move || {
            let request: ClientRequest = read_record(&mut peer).unwrap();
            assert!(matches!(request, ClientRequest::PrepareEdit { .. }));
            // The request's anchor is nonempty — the refusal contradicts
            // the request it claims to answer.
            write_record(
                &mut peer,
                &ClientReply::EditRefused {
                    reason: EditRefusal::EmptyAnchor { index: 0 },
                },
            )
            .unwrap();
        });
        let error = connection
            .edit_file(
                Path::new("file"),
                &"a".repeat(64),
                &[replacement("x", "y")],
                |_| {
                    authorized.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    Ok(())
                },
            )
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        answer.join().unwrap();
        assert_eq!(authorized.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(binding.status().phase, BindingPhase::Detached);
    }

    #[test]
    fn an_empty_anchor_in_the_request_never_reaches_the_wire() {
        let (binding, hello) = edit_rig();
        let (mut connection, mut peer) = live(&binding, &hello);

        let error = connection
            .edit_file(
                Path::new("file"),
                &"a".repeat(64),
                &[
                    replacement("x", "y"),
                    TextReplacement {
                        old_text: String::new(),
                        new_text: "z".to_owned(),
                    },
                ],
                |_| panic!("a malformed request never reaches the gate"),
            )
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        // Quiet wire: the peer sees nothing.
        peer.set_nonblocking(true).unwrap();
        let mut buffer = [0u8; 16];
        assert_eq!(
            peer.read(&mut buffer).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        assert_eq!(binding.status().phase, BindingPhase::Live);
    }
    /// A control peer whose receipts the test gates by hand: each
    /// delivered request parks on `gate` before its correct receipt
    /// rides the next poll — the deterministic way to hold commands
    /// in-flight and queued.
    fn control_peer_loop_gated(
        channel: &mut rustx_execution_transport::RecordChannel,
        gate: &mpsc::Receiver<()>,
    ) {
        let mut sequence = 1_u64;
        let mut receipt: Option<ControlReply> = None;
        loop {
            let deadline = Instant::now() + Duration::from_secs(30);
            let poll = ControlPoll {
                sequence: DecimalU64::new(sequence),
                receipt: receipt.take(),
                health: DecimalU64::new(sequence),
            };
            if channel
                .send(&encode_record(&poll).unwrap(), Some(deadline))
                .is_err()
            {
                return;
            }
            let Ok(turn) = channel.receive::<ControlTurn>(Some(deadline)) else {
                return;
            };
            if turn.sequence != poll.sequence || turn.health != poll.health {
                return;
            }
            sequence += 1;
            let Some(request) = turn.request else {
                continue;
            };
            let done = matches!(request, ControlRequest::Revoke);
            // The receipt waits for the test's explicit release — the
            // command stays genuinely in-flight until then.
            if gate.recv_timeout(Duration::from_secs(30)).is_err() {
                return;
            }
            receipt = Some(match request {
                ControlRequest::Health { nonce } => ControlReply::Healthy { nonce },
                ControlRequest::CancelMutation { ticket } => ControlReply::MutationCancelled {
                    ticket,
                    disposition: CancelDisposition::Unknown,
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
                let poll = ControlPoll {
                    sequence: DecimalU64::new(sequence),
                    receipt,
                    health: DecimalU64::new(sequence),
                };
                let _ = channel.send(
                    &encode_record(&poll).unwrap(),
                    Some(Instant::now() + Duration::from_secs(5)),
                );
                return;
            }
        }
    }

    /// Pairs a control lane whose peer runs the gated loop — the test
    /// releases each receipt through the returned sender.
    fn pair_control_half_gated(
        binding: &Binding,
        hello: &Hello,
        challenge: &PairingNonce,
    ) -> (LocalConnection, mpsc::Sender<()>) {
        let (mut control_peer, control_stream) = UnixStream::pair().unwrap();
        write_record(&mut control_peer, &control_hello(hello, challenge.clone())).unwrap();
        let accepted = binding.accept_local(control_stream).unwrap();
        let ChannelAccept::Live(connection) = accepted else {
            panic!("the paired control lane commits the connection");
        };
        let reply: ControlHelloReply = read_record(&mut control_peer).unwrap();
        assert!(reply.is_ok(), "the honest control lane commits: {reply:?}");
        let (gate, releases) = mpsc::channel();
        thread::spawn(move || {
            let mut channel = RecordChannel::unix(control_peer);
            control_peer_loop_gated(&mut channel, &releases);
        });
        (connection, gate)
    }

    #[test]
    fn control_lane_progresses_while_a_data_write_is_physically_blocked() {
        let (binding, hello) = reserved(BTreeSet::from([Capability::ConditionalWrite]));
        let (mut peer, stream) = UnixStream::pair().unwrap();
        // A 4 KiB kernel send buffer on the server's lane: the 64 KiB
        // `PrepareWrite` frame cannot drain while the peer never reads —
        // the dispatch's `send` is physically blocked inside the kernel
        // write, not merely parked on a semaphore.
        nix::sys::socket::setsockopt(&stream, nix::sys::socket::sockopt::SndBuf, &4096).unwrap();
        write_record(&mut peer, &PeerHello::Data(hello.clone())).unwrap();
        let mut connection = live_data_half(&binding, &hello, stream, &mut peer);

        let payload = vec![b'x'; MAX_RAW_CHUNK_BYTES];
        let encoded_len = encode_record(&ClientRequest::PrepareWrite {
            relative_path: Path::new("file").to_path_buf(),
            expectation: WriteExpectation::Absent,
            data_base64: base64::engine::general_purpose::STANDARD.encode(&payload),
        })
        .unwrap()
        .as_bytes()
        .len();
        let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let dispatched = done.clone();
        let exchange = thread::spawn(move || {
            let result = connection.prepare_write(
                Path::new("file"),
                &WriteExpectation::Absent,
                &payload,
                None,
                &|| Ok(()),
                &|| (),
            );
            dispatched.store(true, std::sync::atomic::Ordering::SeqCst);
            result
        });
        // Give the kernel write room to stall, then prove it has: a
        // strict prefix of the encoded frame sits peeked-but-unread in
        // the peer's kernel queue — MSG_PEEK never consumes, so the
        // blocked writer cannot have pushed the tail either — and the
        // exchange has not returned.
        std::thread::sleep(Duration::from_millis(300));
        let mut peeked = vec![0u8; encoded_len];
        let queued = nix::sys::socket::recv(
            std::os::unix::io::AsRawFd::as_raw_fd(&peer),
            &mut peeked,
            nix::sys::socket::MsgFlags::MSG_PEEK | nix::sys::socket::MsgFlags::MSG_DONTWAIT,
        )
        .unwrap();
        assert!(
            queued > 0 && queued < encoded_len,
            "a strict prefix of the {encoded_len}-byte frame is queued: {queued} bytes"
        );
        assert!(!done.load(std::sync::atomic::Ordering::SeqCst));

        // The independent lane advances anyway: fresh health proofs and
        // a real cancel receipt — while the data write is still stuck.
        let control = binding.control().expect("a live pair has a control handle");
        control.health().unwrap();
        assert_eq!(
            control.cancel_mutation(DecimalU64::new(99)).unwrap(),
            CancelDisposition::Unknown
        );
        assert!(!done.load(std::sync::atomic::Ordering::SeqCst));

        // The fence still interrupts the blocked writer without ever
        // waiting on the data lock — the exchange resolves with an
        // error, never a completed write.
        binding.fence().shutdown();
        let outcome = exchange.join().unwrap();
        assert!(
            outcome.is_err(),
            "the fenced exchange must not complete: {outcome:?}"
        );
        assert!(done.load(std::sync::atomic::Ordering::SeqCst));
    }

    /// One real loopback WebSocket pair admitted through
    /// `accept_channel_deadline`, with the buffers squeezed on both
    /// ends: the server's send buffer and the client's receive buffer
    /// are each capped at 4 KiB — the duplicated handle configures the
    /// shared socket — so a 64 KiB frame physically stalls mid-write
    /// while the client never reads. Returns the live connection, the
    /// held data peer, and a read-only duplicate for kernel peeks.
    fn congested_websocket_pair(
        binding: &Binding,
        hello: &Hello,
    ) -> (
        LocalConnection,
        rustx_execution_transport::RecordChannel,
        ChannelShutdown,
    ) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let endpoint = || {
            rustx_execution_transport::ExecutionUrl::validate(&format!(
                "ws://{address}/execution/v1"
            ))
            .unwrap()
        };
        let (challenge, client, probe) = thread::scope(|scope| {
            let server = scope.spawn(|| {
                let (stream, _) = listener.accept().unwrap();
                nix::sys::socket::setsockopt(&stream, nix::sys::socket::sockopt::SndBuf, &4096)
                    .unwrap();
                let channel = rustx_execution_transport::accept(stream, deadline).unwrap();
                binding.accept_channel_deadline(channel, deadline).unwrap()
            });
            let mut client =
                rustx_execution_transport::connect(&endpoint(), deadline, None).unwrap();
            let probe = client.shutdown_handle().unwrap();
            nix::sys::socket::setsockopt(
                &probe.as_fd().unwrap(),
                nix::sys::socket::sockopt::RcvBuf,
                &4096,
            )
            .unwrap();
            client
                .send(
                    &encode_record(&PeerHello::Data(hello.clone())).unwrap(),
                    Some(deadline),
                )
                .unwrap();
            let reply: DataHelloReply = client.receive(Some(deadline)).unwrap();
            let Ok(data) = reply else {
                panic!("the honest data Hello is admitted");
            };
            assert!(matches!(server.join().unwrap(), ChannelAccept::Pending));
            (data.challenge, client, probe)
        });
        let connection = thread::scope(|scope| {
            let server = scope.spawn(|| {
                let (stream, _) = listener.accept().unwrap();
                let channel = rustx_execution_transport::accept(stream, deadline).unwrap();
                binding.accept_channel_deadline(channel, deadline).unwrap()
            });
            let mut control =
                rustx_execution_transport::connect(&endpoint(), deadline, None).unwrap();
            control
                .send(
                    &encode_record(&control_hello(hello, challenge.clone())).unwrap(),
                    Some(deadline),
                )
                .unwrap();
            let reply: ControlHelloReply = control.receive(Some(deadline)).unwrap();
            assert!(reply.is_ok(), "the honest control lane commits: {reply:?}");
            let ChannelAccept::Live(connection) = server.join().unwrap() else {
                panic!("the paired control lane commits the connection");
            };
            thread::spawn(move || control_peer_loop(&mut control));
            connection
        });
        (connection, client, probe)
    }

    #[test]
    fn control_lane_progresses_while_a_websocket_data_write_is_physically_blocked() {
        let (binding, hello) = reserved(BTreeSet::from([Capability::ConditionalWrite]));
        // The WebSocket client never reads: the 64 KiB `PrepareWrite`
        // frame physically stalls the server's writer.
        let (mut connection, _client, client_probe) = congested_websocket_pair(&binding, &hello);
        let payload = vec![b'x'; MAX_RAW_CHUNK_BYTES];
        let encoded_len = encode_record(&ClientRequest::PrepareWrite {
            relative_path: Path::new("file").to_path_buf(),
            expectation: WriteExpectation::Absent,
            data_base64: base64::engine::general_purpose::STANDARD.encode(&payload),
        })
        .unwrap()
        .as_bytes()
        .len();
        let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let dispatched = done.clone();
        let exchange = thread::spawn(move || {
            let result = connection.prepare_write(
                Path::new("file"),
                &WriteExpectation::Absent,
                &payload,
                None,
                &|| Ok(()),
                &|| (),
            );
            dispatched.store(true, std::sync::atomic::Ordering::SeqCst);
            result
        });
        std::thread::sleep(Duration::from_millis(300));
        // Proof the write is inside the kernel, not merely unparked:
        // peek — never consume — the client's receive queue: a strict
        // prefix of the frame sits there while the exchange has not
        // returned, so the tail cannot have been written either.
        let mut peeked = vec![0u8; encoded_len];
        let queued = nix::sys::socket::recv(
            std::os::fd::AsRawFd::as_raw_fd(&client_probe.as_fd().unwrap()),
            &mut peeked,
            nix::sys::socket::MsgFlags::MSG_PEEK | nix::sys::socket::MsgFlags::MSG_DONTWAIT,
        )
        .unwrap();
        assert!(
            queued > 0 && queued < encoded_len,
            "a strict prefix of the {encoded_len}-byte frame is queued: {queued} bytes"
        );
        assert!(!done.load(std::sync::atomic::Ordering::SeqCst));

        let control_handle = binding.control().expect("a live pair has a control handle");
        control_handle.health().unwrap();
        assert_eq!(
            control_handle.cancel_mutation(DecimalU64::new(99)).unwrap(),
            CancelDisposition::Unknown
        );
        assert!(!done.load(std::sync::atomic::Ordering::SeqCst));

        binding.fence().shutdown();
        let outcome = exchange.join().unwrap();
        assert!(
            outcome.is_err(),
            "the fenced exchange must not complete: {outcome:?}"
        );
        assert!(done.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn the_control_command_budget_refuses_the_fifth_command_promptly() {
        let (binding, hello) = reserved(required());
        let (mut peer, stream) = UnixStream::pair().unwrap();
        write_record(&mut peer, &PeerHello::Data(hello.clone())).unwrap();
        assert!(matches!(
            binding.accept_local(stream).unwrap(),
            ChannelAccept::Pending
        ));
        let reply: DataHelloReply = read_record(&mut peer).unwrap();
        let Ok(data) = reply else {
            panic!("the honest data Hello is admitted");
        };
        let (connection, gate) = pair_control_half_gated(&binding, &hello, &data.challenge);
        let control = connection
            .control()
            .expect("a live pair has a control handle");

        // Five concurrent health commands over a lane whose peer holds
        // every receipt behind the test's gate: the lane admits at most
        // MAX_CONTROL_COMMANDS queued plus in-flight — exactly one of
        // the five resolves WouldBlock promptly while the four admitted
        // commands stay genuinely pending for their gated receipts.
        let (done_tx, done_rx) = mpsc::channel();
        let mut threads = Vec::new();
        for _ in 0..5 {
            let control = control.clone();
            let done_tx = done_tx.clone();
            threads.push(thread::spawn(move || {
                let result = control.health();
                let _ = done_tx.send(result.map_err(|error| error.kind()));
            }));
        }
        match done_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Err(io::ErrorKind::WouldBlock)) => {}
            other => panic!("the fifth command was not refused promptly: {other:?}"),
        }
        assert!(
            matches!(
                done_rx.recv_timeout(Duration::from_millis(200)),
                Err(mpsc::RecvTimeoutError::Timeout)
            ),
            "the four admitted commands are still pending, not refused"
        );
        drop(done_rx);
        drop(done_tx);
        // Revocation drains the admitted waiters with the lane's
        // terminal error — every one resolves, never a false success —
        // and the gated peer exits when the lane ends.
        binding.revoke();
        drop(gate);
        for thread in threads {
            thread.join().unwrap();
        }
        drop(connection);
    }

    #[test]
    fn a_saturated_control_lane_refuses_a_cancel_explicitly_and_delivers_it_once_a_slot_frees() {
        let (binding, hello) = reserved(required());
        let (mut peer, stream) = UnixStream::pair().unwrap();
        write_record(&mut peer, &PeerHello::Data(hello.clone())).unwrap();
        assert!(matches!(
            binding.accept_local(stream).unwrap(),
            ChannelAccept::Pending
        ));
        let reply: DataHelloReply = read_record(&mut peer).unwrap();
        let Ok(data) = reply else {
            panic!("the honest data Hello is admitted");
        };
        let (connection, gate) = pair_control_half_gated(&binding, &hello, &data.challenge);
        let control = connection
            .control()
            .expect("a live pair has a control handle");

        // Fill the whole budget synchronously: four admissions are queued
        // before the test continues, so no timing decides the outcome.
        let mut admitted = Vec::new();
        for nonce in 1..=4_u64 {
            admitted.push(
                control
                    .shared
                    .enqueue(ControlRequest::Health {
                        nonce: DecimalU64::new(nonce),
                    })
                    .unwrap(),
            );
        }

        // A full budget is an explicit refusal at the deadline — the cancel
        // was never queued, so it cannot be silently lost.
        let key = process_key();
        let refused = control.cancel_process_by(&key, Instant::now()).unwrap_err();
        assert_eq!(refused.kind(), io::ErrorKind::WouldBlock);

        // Once the admitted commands resolve, the same cancel is queued,
        // delivered on the lane, and acknowledged.
        for _ in 0..4 {
            gate.send(()).unwrap();
        }
        for receipt in admitted {
            receipt
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .unwrap();
        }
        let delivered = {
            let control = control.clone();
            let key = key.clone();
            thread::spawn(move || {
                control.cancel_process_by(&key, Instant::now() + Duration::from_secs(5))
            })
        };
        gate.send(()).unwrap();
        assert_eq!(
            delivered.join().unwrap().unwrap(),
            rustx_execution_protocol::ProcessCancelDisposition::Unknown
        );
        binding.revoke();
        drop(gate);
        drop(connection);
    }

    #[test]
    fn a_replayed_or_crossed_pairing_nonce_never_pairs() {
        let (binding, hello) = reserved(required());
        let (mut peer, stream) = UnixStream::pair().unwrap();
        write_record(&mut peer, &PeerHello::Data(hello.clone())).unwrap();
        assert!(matches!(
            binding.accept_local(stream).unwrap(),
            ChannelAccept::Pending
        ));
        let reply: DataHelloReply = read_record(&mut peer).unwrap();
        let Ok(data) = reply else {
            panic!("the honest data Hello is admitted");
        };
        let connection = pair_control_half(&binding, &hello, &data.challenge);
        assert_eq!(binding.status().phase, BindingPhase::Live);

        // A second control lane replaying the consumed nonce: the
        // attempt is spent at commit and the binding is already Live —
        // the replay meets the typed already-connected reject, never a
        // second pair.
        let (mut replayed_peer, replayed_stream) = UnixStream::pair().unwrap();
        write_record(
            &mut replayed_peer,
            &control_hello(&hello, data.challenge.clone()),
        )
        .unwrap();
        assert!(matches!(
            binding.accept_local(replayed_stream).unwrap(),
            ChannelAccept::Rejected(Reject::AlreadyConnected)
        ));
        let reply: ControlHelloReply = read_record(&mut replayed_peer).unwrap();
        assert!(matches!(reply, Err(Reject::AlreadyConnected)));
        drop(connection);

        // A foreign binding's challenge can never cross-wire: Session B's
        // nonce is meaningless under Session A's attempt.
        let (binding_a, hello_a) = reserved(required());
        let mut reservation_b = reservation();
        reservation_b.session = SessionId::new("session-2");
        let (binding_b, credential_b) = Binding::reserve(reservation_b).unwrap();
        let mut hello_b = valid_hello(&credential_b);
        hello_b.session = SessionId::new("session-2");

        let (mut peer_a, stream_a) = UnixStream::pair().unwrap();
        write_record(&mut peer_a, &PeerHello::Data(hello_a.clone())).unwrap();
        assert!(matches!(
            binding_a.accept_local(stream_a).unwrap(),
            ChannelAccept::Pending
        ));
        let reply: DataHelloReply = read_record(&mut peer_a).unwrap();
        let Ok(data_a) = reply else {
            panic!("session A's data half is admitted");
        };

        let (mut peer_b, stream_b) = UnixStream::pair().unwrap();
        write_record(&mut peer_b, &PeerHello::Data(hello_b.clone())).unwrap();
        assert!(matches!(
            binding_b.accept_local(stream_b).unwrap(),
            ChannelAccept::Pending
        ));
        let reply: DataHelloReply = read_record(&mut peer_b).unwrap();
        let Ok(data_b) = reply else {
            panic!("session B's data half is admitted");
        };

        // B's nonce against A's attempt and A's nonce against B's: both
        // are typed pairing rejects — the reservations stay Reserved.
        let (mut cross_peer, cross_stream) = UnixStream::pair().unwrap();
        write_record(
            &mut cross_peer,
            &control_hello(&hello_a, data_b.challenge.clone()),
        )
        .unwrap();
        assert!(matches!(
            binding_a.accept_local(cross_stream).unwrap(),
            ChannelAccept::Rejected(Reject::InvalidPairing)
        ));
        let (mut cross_peer_b, cross_stream_b) = UnixStream::pair().unwrap();
        write_record(
            &mut cross_peer_b,
            &control_hello(&hello_b, data_a.challenge.clone()),
        )
        .unwrap();
        assert!(matches!(
            binding_b.accept_local(cross_stream_b).unwrap(),
            ChannelAccept::Rejected(Reject::InvalidPairing)
        ));

        // Each honest pair still commits — a failed cross-wire consumed
        // nothing.
        let _connection_a = pair_control_half(&binding_a, &hello_a, &data_a.challenge);
        let _connection_b = pair_control_half(&binding_b, &hello_b, &data_b.challenge);
        assert_eq!(binding_a.status().phase, BindingPhase::Live);
        assert_eq!(binding_b.status().phase, BindingPhase::Live);
    }

    /// The delivery-window race: a correct-nonce `ControlHello` can only
    /// have learned the challenge from the wire reply, yet thread
    /// scheduling lets it arrive before `store_pending_channel` runs.
    /// The verifier must wait — bounded by the pairing budget, the state
    /// lock released — and commit once the store lands, never refuse an
    /// honest pair for winning its own race.
    #[test]
    fn a_verified_control_hello_waits_for_the_in_flight_channel_store() {
        let (binding, hello) = reserved(required());
        let deadline = Instant::now() + Duration::from_secs(10);
        let (nonce, attempt, _admission) = binding
            .allocate_pending(&hello, ConnectionId::new("data-1"), deadline)
            .unwrap()
            .expect("the honest data Hello opens an attempt");
        assert!(binding.status().pending);

        // The data channel is deliberately unstored — the exact window
        // between the challenge reply hitting the wire and the accept
        // storing the stream.
        let (data_peer, data_stream) = UnixStream::pair().unwrap();
        let data_channel = RecordChannel::unix(data_stream);
        let data_shutdown = data_channel.shutdown_handle().unwrap();
        let (control_peer, control_stream) = UnixStream::pair().unwrap();
        let control_channel = RecordChannel::unix(control_stream);
        let control_shutdown = control_channel.shutdown_handle().unwrap();
        let PeerHello::Control(control_hello) = control_hello(&hello, nonce) else {
            panic!("control_hello builds a control record");
        };
        let shared = Arc::new(ControlShared::new());
        let handle = ControlHandle { shared };
        let (committed_tx, committed_rx) = mpsc::channel();
        thread::scope(|scope| {
            let committer = scope.spawn(|| {
                let commit = binding.commit_pair(
                    &control_hello,
                    ConnectionId::new("control-1"),
                    control_shutdown,
                    handle,
                    Instant::now() + Duration::from_secs(30),
                );
                committed_tx.send(()).unwrap();
                commit
            });
            // The verified lane is parked waiting for the store — a
            // prompt refusal would have resolved already.
            assert_eq!(
                committed_rx.recv_timeout(Duration::from_millis(200)),
                Err(mpsc::RecvTimeoutError::Timeout),
                "a verified nonce waits for the channel store"
            );
            // The store lands: the parked verifier wakes and commits.
            assert!(binding.store_pending_channel(attempt, data_channel, data_shutdown));
            let commit = committer.join().unwrap().expect("the honest pair commits");
            // The attempt's own deadline carries through — the control
            // lane's longer armed deadline cannot stretch it.
            assert_eq!(commit.deadline, deadline);
        });
        drop(data_peer);
        drop(control_peer);
        assert_eq!(binding.status().phase, BindingPhase::Live);
        binding.fence().shutdown();
    }

    /// A second authenticated data Hello while a pairing attempt is
    /// alive is refused — never a supersession — so the honest pair's
    /// pending half cannot be evicted by a parallel claim, and at most
    /// one attempt (and one watchdog) exists per Session.
    #[test]
    fn a_second_data_hello_cannot_evict_a_live_pending_attempt() {
        let (binding, hello) = reserved(required());
        let (mut first, first_stream) = UnixStream::pair().unwrap();
        write_record(&mut first, &PeerHello::Data(hello.clone())).unwrap();
        assert!(matches!(
            binding.accept_local(first_stream).unwrap(),
            ChannelAccept::Pending
        ));
        let reply: DataHelloReply = read_record(&mut first).unwrap();
        let Ok(data) = reply else {
            panic!("the first data Hello is admitted");
        };

        // A second well-formed data claim: typed refusal on the wire,
        // the live attempt untouched.
        let (mut second, second_stream) = UnixStream::pair().unwrap();
        write_record(&mut second, &PeerHello::Data(hello.clone())).unwrap();
        assert!(matches!(
            binding.accept_local(second_stream).unwrap(),
            ChannelAccept::Rejected(Reject::AlreadyConnected)
        ));
        let reply: DataHelloReply = read_record(&mut second).unwrap();
        assert!(matches!(reply, Err(Reject::AlreadyConnected)));
        assert_eq!(binding.status().phase, BindingPhase::Reserved);

        // The original attempt still pairs — the failed claim consumed
        // nothing.
        let _connection = pair_control_half(&binding, &hello, &data.challenge);
        assert_eq!(binding.status().phase, BindingPhase::Live);
        binding.fence().shutdown();
    }

    /// One absolute budget spans both lanes: a control half that arrives
    /// after the data lane's armed deadline is rejected as `InvalidPairing`
    /// — its own freshly armed accept deadline can never extend the
    /// pairing window.
    #[test]
    fn a_delayed_control_lane_cannot_extend_the_pairing_budget() {
        let (binding, hello) = reserved(required());
        let (mut peer, stream) = UnixStream::pair().unwrap();
        write_record(&mut peer, &PeerHello::Data(hello.clone())).unwrap();
        // A deliberately short armed budget on the data accept.
        assert!(matches!(
            binding
                .accept_local_deadline(stream, Instant::now() + Duration::from_millis(150))
                .unwrap(),
            ChannelAccept::Pending
        ));
        let reply: DataHelloReply = read_record(&mut peer).unwrap();
        let Ok(data) = reply else {
            panic!("the honest data Hello is admitted");
        };

        // The control half arrives armed with a generous deadline of
        // its own — but the pairing budget already expired.
        std::thread::sleep(Duration::from_millis(250));
        let (mut control_peer, control_stream) = UnixStream::pair().unwrap();
        write_record(
            &mut control_peer,
            &control_hello(&hello, data.challenge.clone()),
        )
        .unwrap();
        assert!(matches!(
            binding
                .accept_local_deadline(control_stream, Instant::now() + Duration::from_secs(30))
                .unwrap(),
            ChannelAccept::Rejected(Reject::InvalidPairing)
        ));
        let reply: ControlHelloReply = read_record(&mut control_peer).unwrap();
        assert!(matches!(reply, Err(Reject::InvalidPairing)));
        assert_eq!(binding.status().phase, BindingPhase::Reserved);
        assert!(!binding.status().pending);
    }

    #[test]
    fn a_lost_control_lane_detaches_the_pair_and_blocks_new_dispatch() {
        let (binding, hello) = reserved(BTreeSet::from([Capability::Connection]));
        let (mut peer, stream) = UnixStream::pair().unwrap();
        write_record(&mut peer, &PeerHello::Data(hello.clone())).unwrap();
        assert!(matches!(
            binding.accept_local(stream).unwrap(),
            ChannelAccept::Pending
        ));
        let reply: DataHelloReply = read_record(&mut peer).unwrap();
        let Ok(data) = reply else {
            panic!("the honest data Hello is admitted");
        };

        // Pair the control lane and keep the peer end — no poll loop,
        // so dropping it is a genuine mid-life lane loss.
        let (mut control_peer, control_stream) = UnixStream::pair().unwrap();
        write_record(
            &mut control_peer,
            &control_hello(&hello, data.challenge.clone()),
        )
        .unwrap();
        let accepted = binding.accept_local(control_stream).unwrap();
        let ChannelAccept::Live(mut connection) = accepted else {
            panic!("the paired control lane commits the connection");
        };
        let reply: ControlHelloReply = read_record(&mut control_peer).unwrap();
        assert!(reply.is_ok());
        assert_eq!(binding.status().phase, BindingPhase::Live);

        // The control peer vanishes: the worker's lane ends, the pair
        // detaches, and no dispatch may start — the data socket is
        // interrupted, never silently retained.
        drop(control_peer);
        let deadline = Instant::now() + Duration::from_secs(10);
        while binding.status().phase == BindingPhase::Live {
            assert!(
                Instant::now() < deadline,
                "the lost lane never detached the pair"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(binding.status().phase, BindingPhase::Detached);
        assert!(connection.ping().is_err() || binding.control().is_none());
        drop(connection);
    }
}
