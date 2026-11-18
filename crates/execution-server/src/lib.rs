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
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use base64::Engine as _;
use constant_time_eq::constant_time_eq_32;
use rustx_execution_protocol::{
    Admission, BindingCredential, Bootstrap, Capability, ClientInstanceId, ClientReply,
    ClientRequest, ConnectionId, DecimalU64, EnvironmentId, Generation, Hello, HelloReply,
    MAX_RAW_CHUNK_BYTES, PROTOCOL_VERSION, ReadError, Reject, SessionId,
    is_normalized_absolute_path,
    transport::{read_record, write_record},
};
use sha2::{Digest, Sha256};

/// The deadline for a Client to present its Hello on an accepted local
/// connection.
pub const LOCAL_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

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
    inner: Mutex<Inner>,
}

struct Inner {
    phase: BindingPhase,
    connection: Option<ConnectionId>,
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
            inner: Mutex::new(Inner {
                phase: BindingPhase::Reserved,
                connection: None,
            }),
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
        let mut inner = self.inner();
        let offered: [u8; 32] = Sha256::digest(hello.credential.as_bytes()).into();
        if !constant_time_eq_32(&self.verifier, &offered) {
            return Err(Reject::InvalidCredential);
        }
        if hello.version != PROTOCOL_VERSION {
            return Err(Reject::UnsupportedVersion);
        }
        if hello.session != self.reservation.session {
            return Err(Reject::SessionMismatch);
        }
        if hello.environment != self.reservation.environment {
            return Err(Reject::EnvironmentMismatch);
        }
        if hello.instance != self.reservation.instance {
            return Err(Reject::InstanceMismatch);
        }
        if hello.generation != self.reservation.generation {
            return Err(Reject::GenerationMismatch);
        }
        if hello.workspace.as_os_str() != self.reservation.workspace.as_os_str() {
            return Err(Reject::WorkspaceMismatch);
        }
        if !self
            .reservation
            .required_capabilities
            .is_subset(&hello.capabilities)
        {
            return Err(Reject::MissingCapabilities);
        }
        match inner.phase {
            BindingPhase::Reserved => {
                inner.phase = BindingPhase::Live;
                inner.connection = Some(connection);
                Ok(Admission {
                    version: PROTOCOL_VERSION,
                    session: self.reservation.session.clone(),
                    environment: self.reservation.environment.clone(),
                    instance: self.reservation.instance.clone(),
                    generation: self.reservation.generation,
                    workspace: self.reservation.workspace.clone(),
                })
            }
            BindingPhase::Detached => Err(Reject::RecoveryRequired),
            BindingPhase::Live => Err(Reject::AlreadyConnected),
            BindingPhase::Revoked => Err(Reject::Revoked),
        }
    }

    /// Ends the live connection identified by `connection`.
    ///
    /// Only the currently live connection can detach the binding: a stale
    /// or foreign disconnect — for example the late EOF of a connection an
    /// admitted reconnect already replaced — is ignored.
    pub fn disconnect(&self, connection: &ConnectionId) {
        let mut inner = self.inner();
        if inner.phase == BindingPhase::Live && inner.connection.as_ref() == Some(connection) {
            inner.phase = BindingPhase::Detached;
            inner.connection = None;
        }
    }

    /// Revokes the binding immediately and terminally.
    ///
    /// A revoked binding rejects every future Hello and is never
    /// dispatchable again, whatever its previous phase.
    pub fn revoke(&self) {
        let mut inner = self.inner();
        inner.phase = BindingPhase::Revoked;
        inner.connection = None;
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
    pub fn accept_local(
        &self,
        mut stream: UnixStream,
    ) -> io::Result<Result<LocalConnection<'_>, Reject>> {
        stream.set_read_timeout(Some(LOCAL_HANDSHAKE_TIMEOUT))?;
        stream.set_write_timeout(Some(LOCAL_HANDSHAKE_TIMEOUT))?;
        let hello: Hello = read_record(&mut stream)?;
        let capabilities = hello.capabilities.clone();
        let connection = fresh_connection_id()?;
        match self.connect(&hello, connection.clone()) {
            Ok(admission) => {
                let delivered = write_record(&mut stream, &HelloReply::Ok(admission.clone()))
                    .and_then(|()| {
                        stream.set_read_timeout(None)?;
                        stream.set_write_timeout(None)
                    });
                if let Err(error) = delivered {
                    // The peer may never learn it owns this connection: it
                    // is disconnected and its stream shut down, never left
                    // live or reusable.
                    self.disconnect(&connection);
                    let _ = stream.shutdown(std::net::Shutdown::Both);
                    return Err(error);
                }
                Ok(Ok(LocalConnection {
                    binding: self,
                    stream,
                    usable: true,
                    capabilities,
                    connection,
                    admission,
                }))
            }
            Err(reject) => {
                write_record(&mut stream, &HelloReply::Err(reject))?;
                Ok(Err(reject))
            }
        }
    }

    fn inner(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// One admitted local connection. Dropping it disconnects the binding.
///
/// The only flows are typed exchanges the Server initiates: [`ping`](Self::ping)
/// liveness and [`read_range`](Self::read_range), the one implemented physical
/// operation. There is no raw message path. A transport, protocol, or
/// unexpected-reply failure retires the stream: it is never reused while a
/// reply could still be outstanding, and later calls fail closed.
pub struct LocalConnection<'b> {
    binding: &'b Binding,
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
    /// The revision the range was verified under.
    pub revision: String,
}

impl LocalConnection<'_> {
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
        let inner = self.binding.inner();
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
        self.binding.disconnect(&self.connection);
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
        error
    }

    /// Sends exactly one typed request and consumes its one typed reply.
    /// Any transport failure retires the connection.
    fn exchange(&mut self, request: &ClientRequest) -> io::Result<ClientReply> {
        self.gate()?;
        match write_record(&mut self.stream, request).and_then(|()| read_record(&mut self.stream)) {
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
        let reply = self.exchange(&ClientRequest::ReadRange {
            relative_path: relative_path.to_path_buf(),
            offset: DecimalU64::new(offset),
            max_bytes,
            expected_revision: expected_revision.map(str::to_owned),
        })?;
        if let ClientReply::Error { kind } = reply {
            return Err(read_error(kind));
        }
        self.validate_range(offset, bound, expected_revision, &reply)
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
            revision,
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
        // The revision is a SHA-256 hex stamp: exactly 64 lowercase
        // hexadecimal characters, nothing else.
        if revision.len() != 64
            || !revision
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(self.retire(violation("range reply carries an invalid revision")));
        }
        if let Some(expected) = expected_revision
            && revision != expected
        {
            return Err(self.retire(violation("range reply revision ignores expectation")));
        }
        let Ok(length) = u64::try_from(bytes.len()) else {
            return Err(self.retire(violation("range length")));
        };
        let Some(end) = offset.checked_add(length) else {
            return Err(self.retire(violation("range reply overflows the offset space")));
        };
        match next_offset {
            // A continuing reply must land exactly at the end of the
            // delivered bytes, which then must have filled the bound.
            Some(next) if next.get() == end && bytes.len() == bound => {}
            Some(_) => return Err(self.retire(violation("range reply has an invalid cursor"))),
            None => {}
        }
        Ok(RangeRead {
            bytes,
            offset,
            next_offset: (*next_offset).map(DecimalU64::get),
            revision: revision.clone(),
        })
    }
}

/// Maps a stable [`ReadError`] wire code onto an I/O error kind. No raw
/// path or payload crosses here — only the kind.
fn read_error(kind: ReadError) -> io::Error {
    let (error_kind, message) = match kind {
        ReadError::Invalid => (io::ErrorKind::InvalidInput, "range request invalid"),
        ReadError::Denied => (io::ErrorKind::PermissionDenied, "range read denied"),
        ReadError::NotFound => (io::ErrorKind::NotFound, "range path not found"),
        ReadError::Conflict => (io::ErrorKind::AlreadyExists, "range revision conflict"),
        ReadError::Unavailable => (io::ErrorKind::Other, "range read unavailable"),
    };
    io::Error::new(error_kind, message)
}

impl Drop for LocalConnection<'_> {
    fn drop(&mut self) {
        self.binding.disconnect(&self.connection);
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
    use std::num::NonZeroU64;
    use std::sync::Barrier;
    use std::thread;

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
        hello.capabilities =
            BTreeSet::from([Capability::Files, Capability::Process, Capability::Search]);
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
                    revision: String::from("not-a-revision"),
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
}
