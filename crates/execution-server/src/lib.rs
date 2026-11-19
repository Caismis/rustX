//! Server-side binding arbitration for the rustX execution protocol.
//!
//! A `Binding` is the single arbitration point where the Server decides
//! which Client connection may receive environment dispatch. It is a
//! library owned by the Agent Server (package `rustx`) — not a second
//! Agent, and not a transport. This layer performs no dispatch and no
//! spawn; it proves authenticated connection ownership only.

use std::collections::BTreeSet;
use std::io;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use base64::Engine as _;
use constant_time_eq::constant_time_eq_32;
use rustx_execution_protocol::{
    Admission, BindingCredential, Bootstrap, Capability, ClientInstanceId, ClientReply,
    ClientRequest, ConnectionId, DecimalU64, EntryStamp, EnvironmentId, FileStamp, Generation,
    Hello, HelloReply, LineEnd, MAX_DIRECTORY_NAMES, MAX_DIRECTORY_PAGE_ENTRIES,
    MAX_DOCUMENT_LOGICAL_BYTES, MAX_DOCUMENT_SOURCE_BYTES, MAX_RAW_CHUNK_BYTES,
    MAX_SEARCH_CONTEXT_LINES, MAX_SEARCH_PATTERN_BYTES, MAX_TEXT_BYTES, MAX_TEXT_LINES,
    PROTOCOL_VERSION, PageEntry, ReadError, Reject, SearchSpec, SessionId,
    is_normalized_absolute_path,
    transport::{DeadlineStream, PAGE_REQUEST_TIMEOUT, encode_record, read_record, write_record},
};
use sha2::{Digest, Sha256};

/// The deadline for a Client to present its Hello on an accepted local
/// connection — one shared budget owned by
/// [`rustx_execution_protocol::transport`].
pub use rustx_execution_protocol::transport::LOCAL_HANDSHAKE_TIMEOUT;

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
struct ConnectionOwner(Mutex<Inner>);

impl ConnectionOwner {
    fn inner(&self) -> MutexGuard<'_, Inner> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Ends the live connection identified by `connection`.
    ///
    /// Only the currently live connection can detach the binding: a stale
    /// or foreign disconnect — for example the late EOF of a connection an
    /// admitted reconnect already replaced — is ignored.
    fn disconnect(&self, connection: &ConnectionId) {
        let mut inner = self.inner();
        if inner.phase == BindingPhase::Live && inner.connection.as_ref() == Some(connection) {
            inner.phase = BindingPhase::Detached;
            inner.connection = None;
            inner.stream = None;
        }
    }
}

struct Inner {
    phase: BindingPhase,
    connection: Option<ConnectionId>,
    /// A shutdown-capable duplicate of the live connection's stream. It is
    /// never written or read — it exists so [`Binding::revoke`] can close
    /// the physical stream and wake a peer that no dispatch thread
    /// currently holds, without waiting on the data lock.
    stream: Option<UnixStream>,
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
            inner: Arc::new(ConnectionOwner(Mutex::new(Inner {
                phase: BindingPhase::Reserved,
                connection: None,
                stream: None,
            }))),
        };
        Ok((binding, credential))
    }

    /// Validates a [`Hello`] against the reservation and, on success,
    /// commits exactly one live connection.
    ///
    /// The credential verifier is checked first so no identity field can
    /// be probed without valid authentication; then every field must
    /// match the reservation exactly — the workspace by literal
    /// `OsStr` bytes, so `/work//root` never aliases `/work/root`. All
    /// checks and the phase transition happen under one mutex hold, so
    /// concurrent Hellos linearize and exactly one can win the live slot.
    /// A failed Hello consumes nothing: a `Reserved` binding stays
    /// `Reserved` and a `Live` binding stays `Live`.
    ///
    /// `Detached` rejects every Hello with
    /// [`Reject::RecoveryRequired`]: EOF never reopens the reservation and
    /// re-admission needs live-instance recovery proof (#577), which this
    /// layer does not provide. A static credential file therefore can
    /// never be replayed into a new session.
    ///
    /// # Errors
    ///
    /// Returns the [`Reject`] matching the first failed check. Rejects
    /// never echo the submitted credential.
    pub fn connect(&self, hello: &Hello, connection: ConnectionId) -> Result<Admission, Reject> {
        match self.connect_gated(hello, connection, None, None) {
            Ok(result) => result,
            Err(_) => unreachable!("no deadline is checked without one"),
        }
    }

    /// [`connect`](Self::connect) behind an optional deadline rechecked
    /// inside the binding mutex, immediately before commit: an expired
    /// handshake can never become `Live` and consumes nothing — the
    /// binding stays `Reserved`. `stream`, when given, is a shutdown-only
    /// duplicate of the admitted connection stored in the live slot as the
    /// revoke handle.
    fn connect_gated(
        &self,
        hello: &Hello,
        connection: ConnectionId,
        deadline: Option<Instant>,
        stream: Option<UnixStream>,
    ) -> io::Result<Result<Admission, Reject>> {
        let mut inner = self.inner();
        let offered: [u8; 32] = Sha256::digest(hello.credential.as_bytes()).into();
        if !constant_time_eq_32(&self.verifier, &offered) {
            return Ok(Err(Reject::InvalidCredential));
        }
        if hello.version != PROTOCOL_VERSION {
            return Ok(Err(Reject::UnsupportedVersion));
        }
        if hello.session != self.reservation.session {
            return Ok(Err(Reject::SessionMismatch));
        }
        if hello.environment != self.reservation.environment {
            return Ok(Err(Reject::EnvironmentMismatch));
        }
        if hello.instance != self.reservation.instance {
            return Ok(Err(Reject::InstanceMismatch));
        }
        if hello.generation != self.reservation.generation {
            return Ok(Err(Reject::GenerationMismatch));
        }
        if hello.workspace.as_os_str() != self.reservation.workspace.as_os_str() {
            return Ok(Err(Reject::WorkspaceMismatch));
        }
        if !self
            .reservation
            .required_capabilities
            .is_subset(&hello.capabilities)
        {
            return Ok(Err(Reject::MissingCapabilities));
        }
        match inner.phase {
            BindingPhase::Reserved => {
                if let Some(deadline) = deadline {
                    // The deadline is rechecked under the mutex right
                    // before commit: a handshake that ran out of budget
                    // while waiting for arbitration never goes live.
                    deadline
                        .checked_duration_since(Instant::now())
                        .filter(|remaining| !remaining.is_zero())
                        .ok_or_else(|| {
                            io::Error::new(io::ErrorKind::TimedOut, "handshake deadline expired")
                        })?;
                }
                inner.phase = BindingPhase::Live;
                inner.connection = Some(connection);
                inner.stream = stream;
                Ok(Ok(Admission {
                    version: PROTOCOL_VERSION,
                    session: self.reservation.session.clone(),
                    environment: self.reservation.environment.clone(),
                    instance: self.reservation.instance.clone(),
                    generation: self.reservation.generation,
                    workspace: self.reservation.workspace.clone(),
                }))
            }
            BindingPhase::Detached => Ok(Err(Reject::RecoveryRequired)),
            BindingPhase::Live => Ok(Err(Reject::AlreadyConnected)),
            BindingPhase::Revoked => Ok(Err(Reject::Revoked)),
        }
    }

    /// Ends the live connection identified by `connection`.
    ///
    /// Only the currently live connection can detach the binding: a stale
    /// or foreign disconnect — for example the late EOF of a connection an
    /// admitted reconnect already replaced — is ignored.
    pub fn disconnect(&self, connection: &ConnectionId) {
        self.inner.disconnect(connection);
    }

    /// Revokes the binding immediately and terminally.
    ///
    /// A revoked binding rejects every future Hello and is never
    /// dispatchable again, whatever its previous phase. The duplicated
    /// live stream, when one is held, is shut down in the same critical
    /// section: a peer that is not currently locked for an exchange —
    /// including one holding an unpublished staged write — observes EOF,
    /// drops its per-connection write state, and cleans up its private
    /// stage. The shutdown never waits on the connection's data lock.
    pub fn revoke(&self) {
        let mut inner = self.inner();
        inner.phase = BindingPhase::Revoked;
        inner.connection = None;
        if let Some(stream) = inner.stream.take() {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
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
    ) -> io::Result<(Self, UnixListener)> {
        require_private_child(socket_path)?;
        require_private_child(bootstrap_path)?;
        let (binding, credential) = Self::reserve(reservation)?;
        let listener = UnixListener::bind(socket_path)?;
        if let Err(error) = publish_bootstrap(socket_path, bootstrap_path, &binding, credential) {
            let _ = std::fs::remove_file(socket_path);
            return Err(error);
        }
        Ok((binding, listener))
    }

    /// Performs the handshake on one accepted local stream.
    ///
    /// The Client gets [`LOCAL_HANDSHAKE_TIMEOUT`] to present a Hello. The
    /// connection identity is generated fresh here — one per accept —
    /// never supplied from the transport or the peer. The reply is the
    /// typed [`HelloReply`] written as one framed record: on success the
    /// committed live connection is returned as a [`LocalConnection`]
    /// whose drop disconnects the binding; on failure the reject is
    /// delivered on the wire and returned.
    ///
    /// If the admission cannot be delivered, the committed connection is
    /// disconnected again — a binding is never left live on a connection
    /// the peer does not know it owns.
    ///
    /// # Errors
    ///
    /// Returns the transport or socket error when the record cannot be
    /// read or written. An authenticated-but-refused Hello is not an I/O
    /// error: it returns `Ok(Err(Reject))`.
    pub fn accept_local(&self, stream: UnixStream) -> io::Result<Result<LocalConnection, Reject>> {
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
    /// error: it returns `Ok(Err(Reject))`.
    pub fn accept_local_deadline(
        &self,
        mut stream: UnixStream,
        deadline: Instant,
    ) -> io::Result<Result<LocalConnection, Reject>> {
        // The revoke shutdown handle is duplicated before the handshake
        // wrapper takes the stream's only borrow.
        let revoke_handle = stream.try_clone()?;
        // The handshake wrapper itself normalizes the stream to
        // nonblocking mode, whatever flag an accept inherited.
        let mut handshake = DeadlineStream::new(&mut stream, deadline)?;
        let hello: Hello = read_record(&mut handshake)?;
        let capabilities = hello.capabilities.clone();
        let connection = fresh_connection_id()?;
        match self.connect_gated(
            &hello,
            connection.clone(),
            Some(deadline),
            Some(revoke_handle),
        )? {
            Ok(admission) => {
                self.deliver(&mut handshake, &connection, &admission)?;
                Ok(Ok(LocalConnection {
                    owner: self.inner.clone(),
                    stream,
                    usable: true,
                    capabilities,
                    connection,
                    admission,
                }))
            }
            Err(reject) => {
                write_record(&mut handshake, &HelloReply::Err(reject))?;
                handshake.finish()?;
                Ok(Err(reject))
            }
        }
    }

    /// Writes the admission and completes the handshake deadline. On any
    /// failure after the commit — an expired deadline, a transport error,
    /// or blocking mode that cannot be restored — the committed connection
    /// is disconnected again and the stream shut down: a binding never
    /// stays live on a connection the peer does not know it owns.
    fn deliver(
        &self,
        stream: &mut DeadlineStream<'_>,
        connection: &ConnectionId,
        admission: &Admission,
    ) -> io::Result<()> {
        let delivered =
            write_record(stream, &HelloReply::Ok(admission.clone())).and_then(|()| stream.finish());
        if let Err(error) = delivered {
            self.disconnect(connection);
            let _ = stream.get_mut().shutdown(std::net::Shutdown::Both);
            return Err(error);
        }
        Ok(())
    }

    fn inner(&self) -> MutexGuard<'_, Inner> {
        self.inner.inner()
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
    stream: UnixStream,
    usable: bool,
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

    /// Marks the stream unusable, disconnects the binding, and shuts the
    /// socket down: after a protocol violation, truncation, or unexpected
    /// reply the peer's response accounting is unknowable, so the
    /// connection is torn down and never used again.
    fn retire(&mut self, error: io::Error) -> io::Error {
        self.usable = false;
        self.owner.disconnect(&self.connection);
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
        error
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
    /// With `deadline`, the write and the read share one
    /// [`DeadlineStream`] armed to it; a timeout, partial reply, or any
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
        let result = match deadline {
            Some(deadline) => (|| -> io::Result<ClientReply> {
                let mut stream = DeadlineStream::new(&mut self.stream, deadline)?;
                encoded.write_to(&mut stream)?;
                sent();
                let reply = read_record(&mut stream)?;
                stream.finish()?;
                Ok(reply)
            })(),
            None => (|| -> io::Result<ClientReply> {
                encoded.write_to(&mut self.stream)?;
                sent();
                read_record(&mut self.stream)
            })(),
        };
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
    /// Returns the same kinds as [`read_range`], plus
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
    /// error kind, the `authorize` error verbatim when its gate denies, a
    /// [`WriteOutcomeUnknown`]-downcastable error once the outcome cannot
    /// be known, or [`io::ErrorKind::InvalidData`] for a malformed or
    /// untruthful reply.
    pub fn write_file(
        &mut self,
        relative_path: &Path,
        bytes: &[u8],
        expected_revision: Option<&str>,
        authorize: impl FnOnce(Option<&FileStamp>) -> io::Result<()>,
    ) -> io::Result<FileStamp> {
        if !self.capabilities.contains(&Capability::ConditionalWrite) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "peer admitted no conditional write capability",
            ));
        }
        if bytes.len() > MAX_RAW_CHUNK_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "file write exceeds chunk limit",
            ));
        }
        let reply = self.exchange(&ClientRequest::PrepareWrite {
            relative_path: relative_path.to_path_buf(),
            expected_revision: expected_revision.map(str::to_owned),
            data_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
        })?;
        if let ClientReply::Error { kind } = reply {
            return Err(read_error(kind));
        }
        let violation = |message: &'static str| io::Error::new(io::ErrorKind::InvalidData, message);
        let ClientReply::WritePrepared { ticket, original } = reply else {
            return Err(self.retire(violation("unexpected reply to write prepare")));
        };
        if ticket.get() == 0 {
            return Err(self.retire(violation("write reply carries a zero ticket")));
        }
        // The reported original must satisfy exactly the expectation that
        // was sent — a valid stamp matching the revision for a replace, or
        // no original at all for a create-only target.
        let expectation_holds = match (expected_revision, &original) {
            (Some(expected), Some(original)) => {
                original.revision == expected && stamp_violation(original).is_none()
            }
            (None, None) => true,
            _ => false,
        };
        if !expectation_holds {
            return Err(self.retire(violation("write reply ignores the expected revision")));
        }
        if let Err(error) = authorize(original.as_ref()) {
            // The gate denied before commit: remove the stage while the
            // binding still admits the frame, and never commit. A
            // malformed abort reply is a protocol violation like any
            // other — the original denial is still what the caller sees.
            if self.gate().is_ok() {
                match self.exchange(&ClientRequest::AbortWrite { ticket }) {
                    Ok(ClientReply::WriteAborted { ticket: echoed }) if echoed == ticket => {}
                    Ok(_) => {
                        self.retire(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "unexpected reply to write abort",
                        ));
                    }
                    Err(_) => {}
                }
            }
            return Err(error);
        }
        self.commit_write(ticket, bytes.len() as u64)
    }

    /// The commit half of [`write_file`](Self::write_file): the request is
    /// encoded and gated before its first byte, so a denial there is still
    /// a clean pre-commit error. Once the first byte is written the publish
    /// crossing may already have happened — every later failure retires the
    /// connection and reports [`WriteOutcomeUnknown`], never a denial and
    /// never a retry. A `Written` receipt is trusted only when its stamp
    /// is valid *and* its size equals the exact byte count this connection
    /// submitted: a receipt describing different bytes cannot prove the
    /// publish, so the outcome stays unknown.
    fn commit_write(&mut self, ticket: DecimalU64, expected_size: u64) -> io::Result<FileStamp> {
        let encoded = encode_record(&ClientRequest::CommitWrite { ticket })?;
        self.gate()?;
        let outcome_unknown =
            |connection: &mut Self| connection.retire(io::Error::other(WriteOutcomeUnknown));
        let Ok(reply) = encoded
            .write_to(&mut self.stream)
            .and_then(|()| read_record(&mut self.stream))
        else {
            return Err(outcome_unknown(self));
        };
        match reply {
            // An explicit typed refusal means the Client never crossed:
            // it propagates as an ordinary pre-commit error.
            ClientReply::Error { kind } => Err(read_error(kind)),
            // The Client crossed but its later stages failed — the same
            // unknowable outcome as a lost acknowledgement. The reply
            // itself is valid and the ticket is consumed, so the
            // connection is not retired for this one.
            ClientReply::CommitUnknown { ticket: echoed } if echoed == ticket => {
                Err(io::Error::other(WriteOutcomeUnknown))
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
    };
    io::Error::new(error_kind, PhysicalError(kind))
}

impl Drop for LocalConnection {
    fn drop(&mut self) {
        self.owner.disconnect(&self.connection);
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

/// Writes the bootstrap bundle as exactly one framed record into a
/// freshly created owner-only file.
///
/// If `create_new` refuses an existing path, nothing is touched. Once the
/// file exists and is ours, any later failure removes it — failed
/// publication never leaves a partial bootstrap behind.
fn publish_bootstrap(
    socket_path: &Path,
    bootstrap_path: &Path,
    binding: &Binding,
    credential: BindingCredential,
) -> io::Result<()> {
    let reservation = &binding.reservation;
    let bootstrap = Bootstrap {
        socket_path: socket_path.to_path_buf(),
        version: PROTOCOL_VERSION,
        session: reservation.session.clone(),
        environment: reservation.environment.clone(),
        instance: reservation.instance.clone(),
        generation: reservation.generation,
        workspace: reservation.workspace.clone(),
        credential,
    };
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(bootstrap_path)?;
    let result = (|| {
        // Enforce the mode explicitly so the process umask cannot weaken
        // or strengthen it.
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        write_record(&mut file, &bootstrap)?;
        file.sync_all()
    })();
    if result.is_err() {
        drop(file);
        let _ = std::fs::remove_file(bootstrap_path);
    }
    result
}

/// Requires `path` to be a normalized absolute direct child of an
/// existing owner-only (`0700`) directory owned by this process's euid.
fn require_private_child(path: &Path) -> io::Result<()> {
    if !is_normalized_absolute_path(path) {
        return Err(invalid_input("path must be a normalized absolute path"));
    }
    let Some(parent) = path.parent() else {
        return Err(invalid_input("path has no parent directory"));
    };
    let metadata = std::fs::symlink_metadata(parent)
        .map_err(|_| invalid_input("parent directory is missing or unreadable"))?;
    if !metadata.file_type().is_dir()
        || metadata.uid() != euid()
        || metadata.mode() & 0o777 != 0o700
    {
        return Err(invalid_input(
            "parent must be an owner-only private directory",
        ));
    }
    Ok(())
}

fn euid() -> u32 {
    nix::unistd::geteuid().as_raw()
}

fn invalid_input(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
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

    #[test]
    fn a_peer_without_the_range_capability_gets_no_request() {
        let (binding, hello) = reserved(BTreeSet::from([Capability::Connection]));
        let (mut peer, stream) = UnixStream::pair().unwrap();
        write_record(&mut peer, &hello).unwrap();
        let mut connection = binding.accept_local(stream).unwrap().unwrap();
        let reply: HelloReply = read_record(&mut peer).unwrap();
        assert!(reply.is_ok());

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

    /// One fully admitted live connection over a real socket pair: the
    /// Hello is written by the peer, `accept_local` completes, and the
    /// peer end comes back for scripted replies.
    fn live(binding: &Binding, hello: &Hello) -> (LocalConnection, UnixStream) {
        let (mut peer, stream) = UnixStream::pair().unwrap();
        write_record(&mut peer, hello).unwrap();
        let connection = binding.accept_local(stream).unwrap().unwrap();
        let reply: HelloReply = read_record(&mut peer).unwrap();
        assert!(reply.is_ok());
        (connection, peer)
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

        // The same check inside arbitration, right before commit: an
        // expired budget cannot become Live and consumes nothing.
        let Err(error) = binding.connect_gated(
            &hello,
            ConnectionId::new("conn-1"),
            Some(Instant::now()),
            None,
        ) else {
            panic!("an expired deadline must fail before commit");
        };
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert_eq!(binding.status().phase, BindingPhase::Reserved);

        // The honest Hello still wins the live connection afterwards.
        assert!(binding.connect(&hello, ConnectionId::new("conn-2")).is_ok());
        assert_eq!(binding.status().phase, BindingPhase::Live);
    }

    #[test]
    fn an_expired_deadline_during_delivery_detaches_the_binding() {
        let (binding, hello) = reserved(BTreeSet::from([Capability::Connection]));
        let connection = ConnectionId::new("conn-1");
        let admission = binding.connect(&hello, connection.clone()).unwrap();
        assert_eq!(binding.status().phase, BindingPhase::Live);

        // The deadline expires after the commit but before the admission
        // is delivered: the committed connection is disconnected and the
        // stream shut down, never left live on a peer that cannot know
        // it owns the connection.
        let (_peer, mut stream) = UnixStream::pair().unwrap();
        let mut handshake = DeadlineStream::new(&mut stream, Instant::now()).unwrap();
        let error = binding
            .deliver(&mut handshake, &connection, &admission)
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert_eq!(binding.status().phase, BindingPhase::Detached);
    }

    #[test]
    fn an_invalid_revision_retires_the_stream_and_detaches_the_binding() {
        let (binding, hello) = reserved(BTreeSet::from([
            Capability::Connection,
            Capability::ReadRange,
        ]));
        let (mut peer, stream) = UnixStream::pair().unwrap();
        write_record(&mut peer, &hello).unwrap();
        let mut connection = binding.accept_local(stream).unwrap().unwrap();
        let reply: HelloReply = read_record(&mut peer).unwrap();
        assert!(reply.is_ok());

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

    #[test]
    fn a_size_contradiction_retires_the_stream_and_detaches_the_binding() {
        let (binding, hello) = reserved(BTreeSet::from([
            Capability::Connection,
            Capability::ReadRange,
        ]));
        let (mut peer, stream) = UnixStream::pair().unwrap();
        write_record(&mut peer, &hello).unwrap();
        let mut connection = binding.accept_local(stream).unwrap().unwrap();
        let reply: HelloReply = read_record(&mut peer).unwrap();
        assert!(reply.is_ok());

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
        let (mut peer, stream) = UnixStream::pair().unwrap();
        write_record(&mut peer, &hello).unwrap();
        let mut connection = binding.accept_local(stream).unwrap().unwrap();
        let reply: HelloReply = read_record(&mut peer).unwrap();
        assert!(reply.is_ok());

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
        let (mut peer, stream) = UnixStream::pair().unwrap();
        write_record(&mut peer, &hello).unwrap();
        let mut connection = binding.accept_local(stream).unwrap().unwrap();
        let reply: HelloReply = read_record(&mut peer).unwrap();
        assert!(reply.is_ok());

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
        let (mut peer, stream) = UnixStream::pair().unwrap();
        write_record(&mut peer, &hello).unwrap();
        let mut connection = binding.accept_local(stream).unwrap().unwrap();
        let reply: HelloReply = read_record(&mut peer).unwrap();
        assert!(reply.is_ok());

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
        let (mut peer, stream) = UnixStream::pair().unwrap();
        write_record(&mut peer, &hello).unwrap();
        let mut connection = binding.accept_local(stream).unwrap().unwrap();
        let reply: HelloReply = read_record(&mut peer).unwrap();
        assert!(reply.is_ok());

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
        let (mut peer, stream) = UnixStream::pair().unwrap();
        write_record(&mut peer, &hello).unwrap();
        let mut connection = binding.accept_local(stream).unwrap().unwrap();
        let reply: HelloReply = read_record(&mut peer).unwrap();
        assert!(reply.is_ok());

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
        let (mut peer, stream) = UnixStream::pair().unwrap();
        write_record(&mut peer, &hello).unwrap();
        let mut connection = binding.accept_local(stream).unwrap().unwrap();
        let reply: HelloReply = read_record(&mut peer).unwrap();
        assert!(reply.is_ok());

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
        let (mut peer, stream) = UnixStream::pair().unwrap();
        write_record(&mut peer, &hello).unwrap();
        let mut connection = binding.accept_local(stream).unwrap().unwrap();
        let reply: HelloReply = read_record(&mut peer).unwrap();
        assert!(reply.is_ok());

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
                expected_revision,
                data_base64,
            } = request
            else {
                panic!("expected a prepare request");
            };
            assert_eq!(relative_path, PathBuf::from("file"));
            assert_eq!(expected_revision.as_deref(), Some("a".repeat(64).as_str()));
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
            let request: ClientRequest = read_record(&mut peer).unwrap();
            assert_eq!(request, ClientRequest::Ping);
            write_record(&mut peer, &ClientReply::Pong).unwrap();
        });
        let error = connection
            .write_file(Path::new("file"), b"data", None, |_| Ok(()))
            .unwrap_err();
        assert_outcome_unknown(&error);
        // The explicit typed reply is a valid answer — the connection is
        // not retired for it and stays dispatchable.
        connection.ping().unwrap();
        answer.join().unwrap();
        assert_eq!(binding.status().phase, BindingPhase::Live);
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
}
