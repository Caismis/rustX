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

use std::collections::{BTreeSet, HashMap};
use std::io;
use std::num::NonZeroU64;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, TryLockError};

use rustx_execution_protocol::{
    Capability, ClientInstanceId, EntryKind, EntryStamp, EnvironmentId, FileStamp, Generation,
    Reject, is_normalized_absolute_path, transport::LOCAL_HANDSHAKE_TIMEOUT,
};
use rustx_execution_server::{
    Binding, DirectoryListing, LinesPage, LocalConnection, RangeRead, Reservation,
};

use super::super::session::SessionId;
use super::{RuntimeManagerError, SessionRuntimeManager, error};

/// One Session's local execution endpoint, published once.
///
/// Owned by the trusted native composer: the App Server protocol has no
/// path to the listener, the bundle, or the credential inside it. Nothing
/// spawns a Client or connects by itself — the composer hands each
/// accepted stream through [`admit_local_stream`](Self::admit_local_stream)
/// and dispatches through [`read_range`](Self::read_range).
pub struct LocalExecutionEndpoint {
    binding: Binding,
    /// The one admitted Client connection — at most one, never replaced.
    /// There is no recovery path (#577), so the first `Live` peer owns the
    /// slot for the binding's whole life.
    connection: Mutex<Option<LocalConnection>>,
    listener: UnixListener,
    bootstrap: PathBuf,
    /// The reserved logical workspace — trusted catalog identity,
    /// recorded once at claim time, never probed on disk.
    workspace: PathBuf,
    _private: tempfile::TempDir,
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
            generation: Generation::new(NonZeroU64::MIN),
            workspace: workspace.clone(),
            required_capabilities: BTreeSet::from([Capability::Connection]),
        };
        let (binding, listener) =
            Binding::reserve_local(reservation, &root.join("s"), &root.join("b"))?;
        Ok(Self {
            binding,
            connection: Mutex::new(None),
            listener,
            bootstrap: root.join("b"),
            workspace,
            _private: private,
        })
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
        &self.bootstrap
    }

    /// The bound local listener for this binding.
    #[must_use]
    pub fn listener(&self) -> &UnixListener {
        &self.listener
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
    pub fn admit_local_stream(&self, stream: UnixStream) -> io::Result<Result<(), Reject>> {
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
    ) -> io::Result<Result<(), Reject>> {
        match self.binding.accept_local_deadline(stream, deadline)? {
            Ok(connection) => {
                let mut slot = self.connection.lock().expect("endpoint connection");
                assert!(
                    slot.replace(connection).is_none(),
                    "a binding admits at most one live connection"
                );
                Ok(Ok(()))
            }
            Err(reject) => Ok(Err(reject)),
        }
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
        self.with_connection(|connection| {
            let relative = workspace_relative(&connection.admission().workspace, path)?;
            connection.read_range(&relative, offset, max_bytes, expected_revision)
        })
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
        self.with_connection(|connection| {
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
        self.with_connection(|connection| {
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
        self.with_connection(|connection| {
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
        self.with_connection(|connection| {
            let relative = workspace_relative(&connection.admission().workspace, path)?;
            connection
                .write_file(&relative, bytes, expected_revision, authorize)
                .map(project_stamp)
        })
    }

    /// Serializes one synchronous exchange on the admitted connection:
    /// `WouldBlock` while another dispatch holds it — a bounded busy
    /// signal, never a queue — and `NotConnected` before admission.
    /// The connection stays locked across the whole dispatch, so the
    /// reply accounting of one exchange is never interleaved with the
    /// next.
    fn with_connection<T>(
        &self,
        dispatch: impl FnOnce(&mut LocalConnection) -> io::Result<T>,
    ) -> io::Result<T> {
        let mut guard = match self.connection.try_lock() {
            Ok(guard) => guard,
            Err(TryLockError::WouldBlock) => {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "execution connection is busy",
                ));
            }
            Err(TryLockError::Poisoned(error)) => error.into_inner(),
        };
        let Some(connection) = guard.as_mut() else {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "no client is admitted",
            ));
        };
        dispatch(connection)
    }

    /// The single arbitration point the endpoint itself arbitrates
    /// through — module-internal: callers dispatch through the typed
    /// methods, never around them.
    fn binding(&self) -> &Binding {
        &self.binding
    }
}

/// Resolves `path` to a workspace-relative file target, purely lexically:
/// the workspace is the exact prefix the binding reserved, never
/// canonicalized or statted. A relative path must be normal components
/// only; an absolute path must strip the reserved workspace exactly —
/// `..` or `.` anywhere (including after the prefix), the workspace
/// root itself, an empty path, and any control or NUL byte are refused.
fn workspace_relative(workspace: &Path, path: &Path) -> io::Result<PathBuf> {
    let denied = || {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            "path is outside the Session workspace",
        )
    };
    // `components()`/`strip_prefix()` silently normalize interior `.` and
    // repeated separators, so the raw spelling is checked first: no `.`
    // or `..` segment and no control or NUL byte anywhere in the path.
    let raw = path.as_os_str().as_bytes();
    if raw.is_empty()
        || raw.iter().any(u8::is_ascii_control)
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
fn workspace_entry(workspace: &Path, path: &Path) -> io::Result<PathBuf> {
    if path.as_os_str() == workspace.as_os_str() || path.as_os_str().as_bytes() == b"." {
        return Ok(PathBuf::new());
    }
    workspace_relative(workspace, path)
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
            .field("bootstrap", &self.bootstrap)
            .finish_non_exhaustive()
    }
}

/// The claimable state of one Session's execution endpoint.
struct Slot {
    /// The catalog workspace captured at claim; compared exactly, never
    /// re-derived while the slot lives.
    workspace: PathBuf,
    phase: Mutex<Phase>,
}

enum Phase {
    /// Claimed; the private endpoint is being prepared on a blocking task.
    Preparing,
    /// The one published endpoint; every later claim returns this value.
    Published(Arc<LocalExecutionEndpoint>),
    /// Fenced by delete/drain or a failed preparation. A published endpoint
    /// is kept inside the slot so its teardown never happens under the
    /// phase or registry mutex — it is released when the slot itself is
    /// dropped outside all locks. The value is deliberately never read.
    #[allow(dead_code)]
    Closed(Option<Arc<LocalExecutionEndpoint>>),
}

impl Slot {
    /// Publishes the prepared endpoint while the slot is still open. On
    /// refusal the caller still owns `endpoint` and drops it outside all
    /// locks.
    fn publish(&self, endpoint: &Arc<LocalExecutionEndpoint>) -> bool {
        let mut phase = self.phase.lock().expect("execution slot phase");
        if matches!(*phase, Phase::Preparing) {
            *phase = Phase::Published(endpoint.clone());
            return true;
        }
        false
    }

    /// Terminally closes the slot: an in-flight publication is refused, a
    /// published binding is revoked under its own mutex, and its endpoint
    /// stays owned by the slot until the slot drops outside locks. No
    /// filesystem I/O happens here.
    fn close(&self) {
        let mut phase = self.phase.lock().expect("execution slot phase");
        match &*phase {
            Phase::Preparing => *phase = Phase::Closed(None),
            Phase::Published(endpoint) => {
                let endpoint = endpoint.clone();
                endpoint.binding().revoke();
                *phase = Phase::Closed(Some(endpoint));
            }
            Phase::Closed(_) => {}
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
                    slot.close();
                }
            }
        }
        let mut guard = CloseOnFailure(Some(slot.as_ref()));
        let endpoint = Arc::new(prepare(slot.workspace.clone()).map_err(error)?);
        if !slot.publish(&endpoint) {
            return Err(error("execution binding fenced before publication"));
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
pub(super) struct SessionExecutionRegistry(Mutex<HashMap<SessionId, Arc<Slot>>>);

impl SessionExecutionRegistry {
    /// Claims the Session's execution slot under the registry mutex alone.
    /// An occupied slot must name the identical workspace; an occupied claim
    /// is never swapped to a different path.
    fn claim(&self, session: &SessionId, workspace: PathBuf) -> Result<Claim, RuntimeManagerError> {
        let mut slots = self.0.lock().expect("execution registry mutex");
        if let Some(slot) = slots.get(session) {
            if slot.workspace.as_os_str() != workspace.as_os_str() {
                return Err(error("Session workspace changed since the execution claim"));
            }
            let phase = slot.phase.lock().expect("execution slot phase");
            return match &*phase {
                Phase::Published(endpoint) => Ok(Claim::Existing(endpoint.clone())),
                Phase::Preparing => Err(error("execution binding preparation in progress")),
                Phase::Closed(_) => Err(error("execution binding is closed; recovery is required")),
            };
        }
        // The slot is installed before any credential or socket exists, so
        // exactly one claimant ever prepares.
        let slot = Arc::new(Slot {
            workspace,
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
        let slots = self.0.lock().expect("execution registry mutex");
        let Some(slot) = slots.get(session) else {
            return Err(error("Session has no execution endpoint"));
        };
        let phase = slot.phase.lock().expect("execution slot phase");
        match &*phase {
            Phase::Published(endpoint) => Ok(endpoint.clone()),
            Phase::Preparing => Err(error("execution binding preparation in progress")),
            Phase::Closed(_) => Err(error("execution binding is closed; recovery is required")),
        }
    }

    /// Fences the Session's existing claim. An absent Session gains no
    /// state: fencing never creates a slot.
    pub(super) fn fence(&self, session: &SessionId) {
        let slot = self
            .0
            .lock()
            .expect("execution registry mutex")
            .get(session)
            .cloned();
        if let Some(slot) = slot {
            slot.close();
        }
    }

    /// Fences every existing claim. Later claims are refused by the
    /// manager's permanent `draining` admission gate.
    pub(super) fn fence_all(&self) {
        let slots: Vec<Arc<Slot>> = self
            .0
            .lock()
            .expect("execution registry mutex")
            .values()
            .cloned()
            .collect();
        for slot in slots {
            slot.close();
        }
    }

    /// Removes the Session's claim so its endpoint teardown runs outside
    /// the registry and residency mutexes. The slot closes first in case a
    /// publication is still in flight; the last `Arc` owner drops the
    /// endpoint.
    pub(super) fn retire(&self, session: &SessionId) {
        let slot = self
            .0
            .lock()
            .expect("execution registry mutex")
            .remove(session);
        if let Some(slot) = slot {
            slot.close();
            drop(slot);
        }
    }
}

impl SessionRuntimeManager {
    /// Reserves the Session's process-owned local execution endpoint for the
    /// trusted native composer.
    ///
    /// Admission is decided under the catalog visibility cut and the
    /// residency mutex together: a draining manager or a Session under
    /// retirement never prepares a binding. At most one preparation runs —
    /// the slot is claimed synchronously before any credential, socket, or
    /// bootstrap exists — and every later claim of a published endpoint
    /// returns the identical endpoint, never a replacement. A closed claim
    /// stays closed; there is no recovery path.
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
        let claim = {
            let catalog = self.sessions.catalog.lock().await;
            let (_, settings) = catalog.lineage(session, None).map_err(error)?;
            // Under the same catalog visibility cut: residency admission is
            // the only authority that may fence a claim.
            let registry = self.registry.0.lock().expect("registry mutex");
            if registry.draining {
                return Err(RuntimeManagerError::Draining);
            }
            if registry.retiring_sessions.contains(session) {
                return Err(error("Session retirement has fenced runtime admission"));
            }
            self.execution.claim(session, settings.cwd.clone())
            // Both guards release here, before any I/O or await.
        };
        match claim? {
            Claim::Existing(endpoint) => Ok(endpoint),
            Claim::Create(slot) => {
                // The spawned preparation owns publication: a dropped
                // caller never reissues and never strands the slot.
                let session = session.clone();
                spawn_preparation(slot, move |workspace| {
                    LocalExecutionEndpoint::new(&session, workspace)
                })
                .await
                .map_err(error)?
            }
        }
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
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use base64::Engine as _;
    use rustx_execution_protocol::transport::{read_record, write_record};
    use rustx_execution_protocol::{
        Bootstrap, ClientReply, ClientRequest, DecimalU64, FileStamp, Hello, HelloReply,
        PROTOCOL_VERSION,
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

    fn create(
        registry: &SessionExecutionRegistry,
        session: &SessionId,
        workspace: &Path,
    ) -> Arc<Slot> {
        match registry.claim(session, workspace.to_path_buf()).unwrap() {
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
                    registry.claim(session, workspace.clone())
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
        let published = match registry.claim(&session, workspace.clone()).unwrap() {
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
        assert_ne!(a_bundle.socket_path, b_bundle.socket_path);
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
        registry.fence(&session);
        let endpoint = Arc::new(LocalExecutionEndpoint::new(&session, workspace.clone()).unwrap());
        // The prepared endpoint is real but its binding is still Reserved;
        // publication is refused, and the stale endpoint never goes live.
        assert_eq!(endpoint.binding().status().phase, BindingPhase::Reserved);
        assert!(!slot.publish(&endpoint));
        drop(endpoint);
        match registry.claim(&session, workspace.clone()) {
            Err(error) => assert!(error.to_string().contains("recovery"), "{error}"),
            Ok(_) => panic!("a fenced claim is never reissued"),
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
        let first = match registry.claim(&session, workspace.clone()).unwrap() {
            Claim::Existing(endpoint) => endpoint,
            Claim::Create(_) => panic!("preparation published without a waiter"),
        };
        let second = match registry.claim(&session, workspace.clone()).unwrap() {
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
                max_resident_runtimes: 8,
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

    /// Admits a scripted peer through the endpoint's typed path: a valid
    /// Hello from the published bundle, then the admission reply.
    fn admit(endpoint: &LocalExecutionEndpoint, client: &mut UnixStream) {
        let published = bundle(endpoint);
        let hello = Hello {
            version: PROTOCOL_VERSION,
            session: published.session.clone(),
            environment: published.environment.clone(),
            instance: published.instance.clone(),
            generation: published.generation,
            workspace: published.workspace.clone(),
            capabilities: BTreeSet::from([
                Capability::Connection,
                Capability::ReadRange,
                Capability::DirectoryMetadata,
            ]),
            credential: published.credential.clone(),
        };
        let (accepted, _) = endpoint.listener().accept().unwrap();
        write_record(client, &hello).unwrap();
        assert!(matches!(
            endpoint.admit_local_stream(accepted).unwrap(),
            Ok(())
        ));
        assert!(matches!(
            read_record::<HelloReply>(client).unwrap(),
            HelloReply::Ok(_)
        ));
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
                .0
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
        let mut client = std::os::unix::net::UnixStream::connect(&published.socket_path).unwrap();
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
        let mut client = std::os::unix::net::UnixStream::connect(&published.socket_path).unwrap();
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
        let mut client = std::os::unix::net::UnixStream::connect(&published.socket_path).unwrap();
        admit(&endpoint, &mut client);
        assert_eq!(endpoint.binding().status().phase, BindingPhase::Live);
        manager.execution.fence(&sessions[0]);
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
        manager.begin_draining();
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
            !std::path::Path::new(&published.socket_path).exists(),
            "the retired endpoint's private directory was removed"
        );
    }
}
