//! Binding reservations, paired admission, and shared connection ownership.

use super::*;

static SERVER_INCARNATION: std::sync::OnceLock<ServerIncarnation> = std::sync::OnceLock::new();

/// The one process-wide Server incarnation — allocated once, never per
/// runtime reload. Every [`Bootstrap`](rustx_execution_protocol::Bootstrap), [`Admission`], and process
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
/// `Reserved → Live → Detached` on disconnect. The authenticated live
/// instance can re-admit from `Detached`. `Revoked` is terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingPhase {
    /// Credential issued, no connection yet committed.
    Reserved,
    /// Exactly one authenticated connection is committed.
    Live,
    /// The live pair ended. Bootstrap cannot reopen admission.
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
    pub(super) reservation: Reservation,
    /// SHA-256 of the issued credential. The credential itself is never
    /// stored on the Server.
    verifier: [u8; 32],
    inner: Arc<ConnectionOwner>,
    loopback_tls: Option<rustx_execution_transport::LoopbackTls>,
}

/// The shared arbitration state behind one [`Binding`]: the phase and
/// live connection linearize under one mutex, reached identically
/// through the binding and through any [`LocalConnection`] it already
/// handed out — a `LocalConnection` owns a share, it does not borrow.
pub(super) struct ConnectionOwner {
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

    pub(super) fn is_live(&self, connection: &ConnectionId) -> bool {
        let inner = self.inner();
        inner.phase == BindingPhase::Live && inner.connection.as_ref() == Some(connection)
    }

    /// Ends the live pair identified by its data lane.
    ///
    /// Only the currently live data connection can detach the binding:
    /// a stale or foreign disconnect is ignored. The seized shutdown
    /// handles of both lanes are returned for the caller to invoke
    /// after locks release — a detached pair's control lane dies with
    /// its data lane, never lingers answering polls.
    pub(super) fn disconnect(&self, connection: &ConnectionId, recoverable: bool) -> FencedHandles {
        let mut inner = self.inner();
        if inner.phase == BindingPhase::Live && inner.connection.as_ref() == Some(connection) {
            inner.end_pair(recoverable)
        } else {
            FencedHandles::EMPTY
        }
    }

    /// Ends the live pair identified by its *control* lane — the lane
    /// ended or lied. The pair detaches identically to a data-lane
    /// loss: never a release, never a replacement — and the seized data
    /// handle lets the caller interrupt a data exchange that may be
    /// blocked mid-read.
    pub(super) fn detach_control(
        &self,
        connection: &ConnectionId,
        recoverable: bool,
    ) -> FencedHandles {
        let mut inner = self.inner();
        if inner.phase == BindingPhase::Live
            && inner
                .control
                .as_ref()
                .is_some_and(|control| &control.connection == connection)
        {
            inner.end_pair(recoverable)
        } else {
            FencedHandles::EMPTY
        }
    }

    pub(super) fn confirm_revocation(&self, connection: &ConnectionId) {
        let mut inner = self.inner();
        if matches!(&inner.settlement, PhysicalSettlement::Unconfirmed { control } if control == connection)
        {
            inner.settlement = PhysicalSettlement::Confirmed;
            if let Some(proof) = &mut inner.recovery {
                proof.deadline = None;
            }
            inner.pending = None;
            self.pair_ready.notify_all();
        }
    }

    pub(super) fn revocation_confirmed(&self) -> bool {
        self.inner().settlement == PhysicalSettlement::Confirmed
    }

    /// The fence half of [`Binding::fence`] on the shared owner:
    /// terminal `Revoked`, every lane's handle seized.
    pub(super) fn fence(&self) -> FencedHandles {
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

#[derive(PartialEq, Eq)]
enum PhysicalSettlement {
    NeverAdmitted,
    Unconfirmed { control: ConnectionId },
    Confirmed,
}

struct Inner {
    phase: BindingPhase,
    settlement: PhysicalSettlement,
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
    recovery: Option<RecoveryProof>,
}

struct RecoveryProof {
    verifier: [u8; 32],
    capabilities: BTreeSet<Capability>,
    deadline: Option<Instant>,
}

#[cfg(test)]
#[test]
fn recovery_admission_is_closed_at_the_fixed_deadline() {
    use rustx_execution_protocol::{DETACHED_CLIENT_RETENTION, SAME_INSTANCE_RECOVERY_WINDOW};

    let reservation = Reservation {
        session: SessionId::new("session-recovery"),
        environment: EnvironmentId::new("test"),
        instance: ClientInstanceId::new("retained"),
        generation: Generation::new(std::num::NonZeroU64::MIN),
        workspace: "/workspace".into(),
        required_capabilities: BTreeSet::new(),
    };
    let (binding, _) = Binding::reserve(reservation).unwrap();
    let mut state = binding.inner();
    state.recovery = Some(RecoveryProof {
        verifier: [0; 32],
        capabilities: BTreeSet::new(),
        deadline: None,
    });
    let loss = Instant::now();
    state.detach(loss);
    let cutoff = loss + SAME_INSTANCE_RECOVERY_WINDOW;
    assert_eq!(
        Binding::check_phase_at(&state, cutoff.checked_sub(Duration::from_nanos(1)).unwrap()),
        Ok(())
    );
    assert_eq!(
        Binding::check_phase_at(&state, cutoff),
        Err(Reject::RecoveryRequired)
    );
    assert_eq!(
        DETACHED_CLIENT_RETENTION,
        SAME_INSTANCE_RECOVERY_WINDOW + Duration::from_secs(30)
    );
    state.recovery = None;
    assert_eq!(
        Binding::check_phase_at(&state, loss),
        Err(Reject::RecoveryRequired)
    );
}

#[cfg(test)]
#[test]
fn detached_retention_covers_either_detection_order() {
    use rustx_execution_protocol::{
        DETACHED_CLIENT_RETENTION, SAME_INSTANCE_RECOVERY_WINDOW,
        transport::{CONTROL_FRAME_TIMEOUT, CONTROL_POLL_INTERVAL},
    };

    // A turn already in flight can consume a full frame deadline. The
    // following idle poll and its deadline detect the missing direction.
    // This model assumes scheduling advances within each frame budget.
    let detection_skew = CONTROL_FRAME_TIMEOUT * 2 + CONTROL_POLL_INTERVAL;
    assert!(detection_skew <= Duration::from_secs(20));
    let first = Instant::now();
    for client_detects_first in [false, true] {
        let (client, server) = if client_detects_first {
            (first, first + detection_skew)
        } else {
            (first + detection_skew, first)
        };
        let last_admission = server + SAME_INSTANCE_RECOVERY_WINDOW;
        let cleanup_begins = client + DETACHED_CLIENT_RETENTION;
        assert!(cleanup_begins >= last_admission + Duration::from_secs(10));
    }
}

impl Inner {
    fn end_pair(&mut self, recoverable: bool) -> FencedHandles {
        if recoverable && self.settlement != PhysicalSettlement::Confirmed {
            self.detach(Instant::now());
        } else {
            self.phase = BindingPhase::Detached;
            if let Some(proof) = &mut self.recovery {
                proof.deadline = None;
            }
        }
        self.connection = None;
        FencedHandles(
            self.stream
                .take()
                .into_iter()
                .chain(self.control.take().map(|control| control.shutdown))
                .collect(),
        )
    }

    fn detach(&mut self, now: Instant) {
        self.phase = BindingPhase::Detached;
        if let Some(proof) = &mut self.recovery {
            proof.deadline = Some(now + rustx_execution_protocol::SAME_INSTANCE_RECOVERY_WINDOW);
        }
    }
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
            loopback_tls: None,
            reservation,
            verifier,
            inner: Arc::new(ConnectionOwner {
                inner: Mutex::new(Inner {
                    phase: BindingPhase::Reserved,
                    settlement: PhysicalSettlement::NeverAdmitted,
                    connection: None,
                    stream: None,
                    control: None,
                    pending: None,
                    next_attempt: 0,
                    recovery: None,
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
            &inner,
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
                inner.settlement = PhysicalSettlement::Unconfirmed {
                    control: connection.clone(),
                };
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
        inner: &Inner,
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
        let launch = constant_time_eq_32(&self.verifier, &offered);
        let recovery = inner
            .recovery
            .as_ref()
            .filter(|proof| constant_time_eq_32(&proof.verifier, &offered));
        if !launch && recovery.is_none() {
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
        match inner.phase {
            BindingPhase::Detached => {
                let proof = recovery.ok_or(Reject::RecoveryRequired)?;
                if capabilities.is_some_and(|capabilities| *capabilities != proof.capabilities) {
                    return Err(Reject::MissingCapabilities);
                }
            }
            BindingPhase::Reserved if !launch => return Err(Reject::InvalidCredential),
            _ => {}
        }
        Ok(())
    }

    /// The phase check every lane's commit shares — under one mutex
    /// hold with the identity checks, so concurrent Hellos linearize.
    /// Once detached, the independent pairing check requires the live
    /// instance's memory-only proof. The absolute window also bounds each
    /// wait at the control commit.
    fn check_phase(inner: &Inner) -> Result<(), Reject> {
        Self::check_phase_at(inner, Instant::now())
    }

    fn check_phase_at(inner: &Inner, now: Instant) -> Result<(), Reject> {
        match inner.phase {
            BindingPhase::Reserved => Ok(()),
            BindingPhase::Detached
                if inner
                    .recovery
                    .as_ref()
                    .is_some_and(|proof| proof.deadline.is_some_and(|deadline| now < deadline)) =>
            {
                Ok(())
            }
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
    pub(super) fn allocate_pending(
        &self,
        hello: &Hello,
        connection: ConnectionId,
        deadline: Instant,
    ) -> io::Result<Result<(PairingNonce, u64, Admission), Reject>> {
        let mut inner = self.inner();
        if let Err(reject) = self.check_identity(
            &inner,
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
        let deadline = inner
            .recovery
            .as_ref()
            .and_then(|proof| proof.deadline)
            .map_or(deadline, |limit| limit.min(deadline));
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
    pub(super) fn store_pending_channel(
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
    pub(super) fn commit_pair(
        &self,
        hello: &ControlHello,
        connection: ConnectionId,
        control_shutdown: ChannelShutdown,
        control_handle: ControlHandle,
        deadline: Instant,
    ) -> Result<PairCommit, Reject> {
        let mut inner = self.inner();
        self.check_identity(
            &inner,
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
            .is_none_or(|shutdown| shutdown.readable_now().unwrap_or(true));
        if peer_gone {
            return Err(Reject::InvalidPairing);
        }
        if let Some(proof) = &mut inner.recovery {
            proof.deadline = None;
        } else {
            let credential = hello
                .credential
                .recovery_credential(&hello.connection_nonce);
            inner.recovery = Some(RecoveryProof {
                verifier: Sha256::digest(credential.as_bytes()).into(),
                capabilities: pending.capabilities.clone(),
                deadline: None,
            });
        }
        inner.phase = BindingPhase::Live;
        inner.settlement = PhysicalSettlement::Unconfirmed {
            control: connection.clone(),
        };
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
        self.inner.disconnect(connection, true).shutdown();
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

    /// Whether the detached live instance still has time to prove recovery.
    #[must_use]
    pub fn may_recover(&self) -> bool {
        let inner = self.inner();
        inner.phase == BindingPhase::Detached && Self::check_phase(&inner).is_ok()
    }

    /// Applies a bounded local publication only while this exact lane is
    /// current. The action must not perform I/O or call back into Binding.
    pub fn with_control_lane<T>(
        &self,
        lane: &ControlHandle,
        publish: impl FnOnce() -> T,
    ) -> Option<T> {
        let state = self.inner();
        state
            .control
            .as_ref()
            .is_some_and(|control| control.handle.same_pair(lane))
            .then(publish)
    }

    /// Wait for the admitted Client's physical revocation receipt. A
    /// never-admitted reservation is closed atomically so a pairing cannot
    /// gain execution authority after it was counted as settled.
    ///
    /// # Errors
    ///
    /// Reports an unconfirmed revocation if a previously admitted Client
    /// lost the control path or did not return its receipt by the deadline.
    pub fn settle_revocation_by(&self, deadline: Instant) -> io::Result<()> {
        let control = {
            let mut inner = self.inner.inner();
            match &inner.settlement {
                PhysicalSettlement::NeverAdmitted => {
                    inner.phase = BindingPhase::Revoked;
                    inner.pending = None;
                    self.inner.pair_ready.notify_all();
                    return Ok(());
                }
                PhysicalSettlement::Confirmed => return Ok(()),
                PhysicalSettlement::Unconfirmed { .. } => {
                    inner.control.as_ref().map(|lane| lane.handle.clone())
                }
            }
        };
        let result = control
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotConnected,
                    "admitted Client has no control lane or physical settlement receipt",
                )
            })
            .and_then(|control| control.request_revoke_by(deadline));
        if self.revocation_confirmed() {
            return Ok(());
        }
        result
    }

    /// Whether this binding's control lane supplied a validated Revoke receipt.
    #[must_use]
    pub fn revocation_confirmed(&self) -> bool {
        self.inner.inner().settlement == PhysicalSettlement::Confirmed
    }

    /// Record this binding's identity before accepting its first lane.
    ///
    /// # Errors
    ///
    /// Refuses an existing unsettled marker, an unsafe directory or a write
    /// whose durability cannot be established. No credential is persisted.
    pub fn retain_recovery_fence(&self, product_root: &Path) -> io::Result<RecoveryFence> {
        RecoveryFence::create(product_root, &self.admission(), Arc::downgrade(&self.inner))
    }

    /// Remove the exact marker after this binding's physical settlement.
    ///
    /// # Errors
    ///
    /// Refuses missing settlement, foreign ownership or an unconfirmed unlink.
    pub fn release_recovery_fence(&self, fence: &RecoveryFence) -> io::Result<()> {
        if !fence.belongs_to(&self.inner) || !self.closed_with_settlement() {
            return Err(io::Error::other(
                "binding settlement does not authorize marker release",
            ));
        }
        fence.remove()
    }

    /// Consumes the exact independent settlement receipt for this closed
    /// local binding. Receipt validation and fence ownership are separate
    /// from the Client's control-lane Revoke result.
    /// # Errors
    /// Refuses a foreign record, open admission, or missing settlement.
    #[cfg(target_os = "linux")]
    pub fn release_local_domain(&self, fence: &RecoveryFence) -> io::Result<()> {
        if !fence.belongs_to(&self.inner) || self.status().phase != BindingPhase::Revoked {
            return Err(io::Error::other("local domain admission must be closed"));
        }
        fence.release_independent_domain()
    }

    fn closed_with_settlement(&self) -> bool {
        let state = self.inner();
        state.phase == BindingPhase::Revoked
            && matches!(
                state.settlement,
                PhysicalSettlement::NeverAdmitted | PhysicalSettlement::Confirmed
            )
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
    /// Direct bindings publish the listener's `wss://` address and a
    /// fresh binding-specific certificate. When `public_url` is set, the
    /// operator's TLS ingress authenticates the public endpoint and
    /// forwards to the trusted cleartext loopback listener.
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
        if !listen.ip().is_loopback() {
            return Err(invalid_input(
                "WebSocket execution listener must bind a loopback address",
            ));
        }
        let bootstrap_slot = require_private_child(bootstrap_path)?;
        if let Some(url) = public_url {
            let validated = rustx_execution_transport::ExecutionUrl::validate(url)
                .map_err(|reason| invalid_input_static(&reason))?;
            if !validated.is_secure() {
                return Err(invalid_input(
                    "public execution endpoint must be a wss:// URL",
                ));
            }
        }
        let (mut binding, credential) = Self::reserve(reservation)?;
        let listener = TcpListener::bind(listen)?;
        let (url, certificate_der) = if let Some(public_url) = public_url {
            (public_url.to_owned(), None)
        } else {
            let tls = rustx_execution_transport::LoopbackTls::new(listen.ip())?;
            let certificate = base64::engine::general_purpose::STANDARD.encode(tls.certificate());
            binding.loopback_tls = Some(tls);
            (
                format!(
                    "wss://{}{}",
                    listener.local_addr()?,
                    rustx_execution_protocol::EXECUTION_PATH
                ),
                Some(certificate),
            )
        };
        let bundle = publish_bootstrap(
            BootstrapEndpoint::WebSocket {
                url,
                certificate_der,
            },
            &bootstrap_slot,
            &binding,
            credential,
        )?;
        Ok((binding, listener, bundle))
    }

    /// Upgrades the reserved WebSocket listener and performs normal pair admission.
    ///
    /// # Errors
    ///
    /// Returns a TLS, WebSocket, or protocol failure under the supplied deadline.
    pub fn accept_websocket_deadline(
        &self,
        stream: std::net::TcpStream,
        deadline: Instant,
    ) -> io::Result<ChannelAccept> {
        let channel = if let Some(tls) = &self.loopback_tls {
            tls.accept(stream, deadline)?
        } else {
            rustx_execution_transport::accept(stream, deadline)?
        };
        self.accept_channel_deadline(channel, deadline)
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
                            self.inner
                                .disconnect(
                                    &commit.connection,
                                    rustx_execution_protocol::transport::recoverable_lane_loss(
                                        &error,
                                    ),
                                )
                                .shutdown();
                            channel.shutdown();
                            return Err(error);
                        }
                        let worker =
                            ControlWorker::spawn(channel, self.inner.clone(), connection, shared);
                        Ok(ChannelAccept::Live(LocalConnection {
                            owner: Some(self.inner.clone()),
                            delegated_work: None,
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
pub(super) struct PairCommit {
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
    pub(super) deadline: Instant,
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
