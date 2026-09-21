//! Canonical product identity, controller admission and Conversation lifecycle access.
use nix::fcntl::{Flock, FlockArg};
use std::fs::{File, OpenOptions};
use std::io;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// The private local-root namespace that records Conversation identity
/// consumption (Issue #387).
///
/// A marker file named after one canonical `ConversationId` is created with
/// exclusive create-new semantics. The marker's existence is the whole
/// reservation: it is identity consumption, never Session ownership, never
/// execution authority, and never canonical history. It is deliberately not
/// enumerable by any domain caller.
const CONVERSATION_RESERVATION_NAMESPACE: &str = "conversation-reservations";

/// Process-wide count of successful Conversation identity reservations.
/// Diagnostics-grade, mirroring `durable::conversation_store_open_count`:
/// benchmarks and regressions read a delta, never a semantic decision.
static CONVERSATION_RESERVATIONS: AtomicU64 = AtomicU64::new(0);

/// Process-wide count of exclusive-create conflicts observed by the
/// reservation primitive. A conflict means the identity was already consumed.
static CONVERSATION_RESERVATION_CONFLICTS: AtomicU64 = AtomicU64::new(0);

/// Process-wide count of legacy-layout probes performed by the storage owner
/// (Issue #387). A reservation on a root whose reservation namespace already
/// exists must perform zero of these; the counter exists so a regression can
/// prove the storage owner never inspects an existing `sessions/` tree to
/// establish identity uniqueness.
static LEGACY_LAYOUT_PROBES: AtomicU64 = AtomicU64::new(0);

/// The number of Conversation identities this process has reserved.
#[must_use]
pub fn conversation_reservation_count() -> u64 {
    CONVERSATION_RESERVATIONS.load(Ordering::Relaxed)
}

/// The number of exclusive-create conflicts this process has observed while
/// reserving a Conversation identity.
#[must_use]
pub fn conversation_reservation_conflict_count() -> u64 {
    CONVERSATION_RESERVATION_CONFLICTS.load(Ordering::Relaxed)
}

/// The number of legacy-layout probes performed by the storage owner. See
/// [`LEGACY_LAYOUT_PROBES`].
#[must_use]
pub fn conversation_legacy_layout_probe_count() -> u64 {
    LEGACY_LAYOUT_PROBES.load(Ordering::Relaxed)
}

/// Diagnostics-grade create-path filesystem-operation counters (Issue #387).
///
/// These counters exist so the benchmark and deterministic tests can report
/// and compare the real filesystem work the create path performs. They are
/// relaxed-atomic diagnostics only: no production decision reads them, and a
/// lost increment cannot change storage semantics. The counts are logical
/// syscall invocations, not physical device I/O and not byte counts (see
/// [`FsOperationCounts::catalog_logical_bytes_written`]).
pub mod fs_operations {
    use std::sync::atomic::{AtomicU64, Ordering};

    static CREATE_OPEN: AtomicU64 = AtomicU64::new(0);
    static MKDIR: AtomicU64 = AtomicU64::new(0);
    static WRITE: AtomicU64 = AtomicU64::new(0);
    static FSYNC: AtomicU64 = AtomicU64::new(0);
    static RENAME: AtomicU64 = AtomicU64::new(0);
    static DIR_FSYNC: AtomicU64 = AtomicU64::new(0);
    static CATALOG_LOGICAL_BYTES_WRITTEN: AtomicU64 = AtomicU64::new(0);

    /// A point-in-time snapshot of the create-path operation counters.
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
    pub struct FsOperationCounts {
        /// `open`/`openat` calls that may create a file (`O_CREAT`).
        pub create_open: u64,
        /// `mkdir`/`mkdirat` calls.
        pub mkdir: u64,
        /// `write` calls.
        pub write: u64,
        /// `fsync` calls on a regular file.
        pub fsync: u64,
        /// `rename`/`renameat` calls.
        pub rename: u64,
        /// `fsync` calls on a directory.
        pub dir_fsync: u64,
        /// Sum of the logical payload bytes supplied to the catalog write
        /// operation. This is the bytes handed to `write`, not physical device
        /// bytes and not a sampled file length.
        pub catalog_logical_bytes_written: u64,
    }

    /// The current counter snapshot.
    #[must_use]
    pub fn snapshot() -> FsOperationCounts {
        FsOperationCounts {
            create_open: CREATE_OPEN.load(Ordering::Relaxed),
            mkdir: MKDIR.load(Ordering::Relaxed),
            write: WRITE.load(Ordering::Relaxed),
            fsync: FSYNC.load(Ordering::Relaxed),
            rename: RENAME.load(Ordering::Relaxed),
            dir_fsync: DIR_FSYNC.load(Ordering::Relaxed),
            catalog_logical_bytes_written: CATALOG_LOGICAL_BYTES_WRITTEN.load(Ordering::Relaxed),
        }
    }

    pub(crate) fn record_create_open() {
        CREATE_OPEN.fetch_add(1, Ordering::Relaxed);
    }
    pub(crate) fn record_mkdir() {
        MKDIR.fetch_add(1, Ordering::Relaxed);
    }
    pub(crate) fn record_write() {
        WRITE.fetch_add(1, Ordering::Relaxed);
    }
    pub(crate) fn record_fsync() {
        FSYNC.fetch_add(1, Ordering::Relaxed);
    }
    pub(crate) fn record_rename() {
        RENAME.fetch_add(1, Ordering::Relaxed);
    }
    pub(crate) fn record_dir_fsync() {
        DIR_FSYNC.fetch_add(1, Ordering::Relaxed);
    }
    /// Record one catalog `write` and the logical payload bytes it supplied.
    pub(crate) fn record_catalog_write(bytes: usize) {
        WRITE.fetch_add(1, Ordering::Relaxed);
        CATALOG_LOGICAL_BYTES_WRITTEN
            .fetch_add(u64::try_from(bytes).unwrap_or(u64::MAX), Ordering::Relaxed);
    }
}

// Test-only hooks for the namespace-initialization durability contract.
//
// The barrier count proves a caller established the product-root parent-entry
// barrier rather than inferring durability from the namespace's visibility.
// The failure injection proves a failed barrier leaves visible residue that a
// later retry cannot mistake for completed initialization. The gate makes the
// concurrent fresh-root initialization race deterministic without a sleep.
#[cfg(test)]
thread_local! {
    static NAMESPACE_ROOT_BARRIERS_ON_THREAD: std::cell::Cell<u64> =
        const { std::cell::Cell::new(0) };
    static NAMESPACE_ROOT_BARRIER_FAILURES: std::cell::Cell<u32> =
        const { std::cell::Cell::new(0) };
    static NAMESPACE_INIT_GATE: std::cell::RefCell<
        Option<std::sync::Arc<crate::runtime::conversation_runtime::Gate>>,
    > = const { std::cell::RefCell::new(None) };
}

/// The number of product-root durability barriers performed on the calling
/// thread, regardless of whether the namespace was freshly created.
#[cfg(test)]
#[must_use]
pub(crate) fn reservation_namespace_root_barrier_count_on_thread() -> u64 {
    NAMESPACE_ROOT_BARRIERS_ON_THREAD.with(std::cell::Cell::get)
}

/// Fail the next `count` product-root durability barriers on this thread with
/// a deterministic I/O error. Used only to prove retry behavior after residue.
#[cfg(test)]
pub(crate) fn fail_next_reservation_namespace_root_barriers(count: u32) {
    NAMESPACE_ROOT_BARRIER_FAILURES.with(|failures| failures.set(count));
}

/// Park this thread inside `validate_or_fresh` after the namespace read and
/// before the legacy `sessions/` probe, so a concurrent initializer can win
/// the race deterministically. Test-only.
#[cfg(test)]
pub(crate) fn install_reservation_init_gate(
    gate: std::sync::Arc<crate::runtime::conversation_runtime::Gate>,
) {
    NAMESPACE_INIT_GATE.with(|slot| *slot.borrow_mut() = Some(gate));
}

#[cfg(test)]
fn enter_reservation_init_gate() {
    NAMESPACE_INIT_GATE.with(|slot| {
        if let Some(gate) = slot.borrow().as_ref() {
            gate.enter();
        }
    });
}

#[cfg(not(test))]
fn enter_reservation_init_gate() {}

/// Establish the product-root parent-entry durability barrier for the
/// reservation namespace. Visibility is not durability, so a caller that
/// merely observes the namespace directory must still perform this barrier.
fn reservation_namespace_root_barrier(root: &Path) -> io::Result<()> {
    #[cfg(test)]
    {
        NAMESPACE_ROOT_BARRIERS_ON_THREAD.with(|count| count.set(count.get() + 1));
        let failures = NAMESPACE_ROOT_BARRIER_FAILURES.with(std::cell::Cell::get);
        if failures > 0 {
            NAMESPACE_ROOT_BARRIER_FAILURES.with(|cell| cell.set(failures - 1));
            return Err(io::Error::other(
                "deterministic reservation namespace root durability barrier fault",
            ));
        }
    }
    fs_operations::record_dir_fsync();
    File::open(root)?.sync_all()
}

/// Persist every directory entry created for the product root by
/// `create_dir_all`: the root itself and any missing ancestors. Syncing only
/// the root would not prove that a newly created parent survives a crash.
fn sync_directory_ancestry(path: &Path) -> io::Result<()> {
    for directory in path.ancestors() {
        fs_operations::record_dir_fsync();
        File::open(directory)?.sync_all()?;
    }
    Ok(())
}

/// Canonical product identity; this is not a live storage-access guard.
#[derive(Debug, Clone)]
pub struct ProductRoot {
    root: PathBuf,
}
impl ProductRoot {
    /// Establish canonical identity at explicit startup, creating the product root
    /// and initializing the reservation namespace of a genuinely fresh root.
    /// # Errors
    /// Returns filesystem errors without weakening private-path confinement.
    pub fn create(root: &Path) -> io::Result<Self> {
        std::fs::create_dir_all(root)?;
        let product = Self::existing(root)?;
        // `create_dir_all` may have created the product root and any missing
        // ancestors in this process. Persist those directory entries before
        // the reservation namespace (or any later durable success) can claim
        // local-root durability. Exclusive directory creation alone is not a
        // crash-durability guarantee.
        sync_directory_ancestry(product.root())?;
        product.ensure_reservation_namespace()?;
        Ok(product)
    }
    /// Resolve existing native product state without creating anything.
    ///
    /// A populated root that predates the reservation namespace is refused:
    /// absence of a marker cannot prove an identity was never allocated under
    /// the older layout, so the root is not silently reinterpreted. A
    /// genuinely fresh root (no reservations and no `sessions` tree) is
    /// accepted unchanged; [`Self::create`] or the first reservation
    /// initializes its namespace.
    /// # Errors
    /// Missing roots, unsafe layouts and filesystem errors are returned.
    pub fn existing(root: &Path) -> io::Result<Self> {
        let root = root.canonicalize()?;
        directory(&root)?;
        let product = Self { root };
        product.validate_or_fresh()?;
        Ok(product)
    }

    /// The private reservation namespace path, never exposed to domain callers.
    fn reservation_namespace(&self) -> PathBuf {
        self.root.join(CONVERSATION_RESERVATION_NAMESPACE)
    }

    /// Validate the accepted local-root format without creating anything.
    ///
    /// The reservation namespace is the local-root format boundary. When it
    /// exists the root is supported. When it is absent, a root with a
    /// `sessions` tree is an unsupported older layout and is refused; a root
    /// with neither is genuinely fresh and is accepted for initialization.
    fn validate_or_fresh(&self) -> io::Result<()> {
        if self.reservation_namespace_present()? {
            return Ok(());
        }
        // Test-only rendezvous: the other initializer creates the namespace
        // and the `sessions` tree while this thread is parked here, so the
        // race below is exercised deterministically instead of by timing.
        enter_reservation_init_gate();
        if self.probe_legacy_sessions()? {
            // Fresh-root initialization creates the reservation namespace
            // strictly before any `sessions` tree. A namespace that appeared
            // since our first read therefore means a concurrent initializer
            // won and this is a valid new-format root, not a legacy layout.
            // Re-checking after the probe is the linearization point that
            // keeps the format decision coherent.
            if self.reservation_namespace_present()? {
                return Ok(());
            }
            return Err(unsupported_layout_error());
        }
        Ok(())
    }

    /// Whether the private reservation namespace exists and is a directory.
    /// This is a visibility check only; it says nothing about whether the
    /// namespace-init durability barrier has completed.
    fn reservation_namespace_present(&self) -> io::Result<bool> {
        match std::fs::symlink_metadata(self.reservation_namespace()) {
            Ok(metadata) if metadata.is_dir() => Ok(true),
            Ok(_) => Err(io::Error::other(
                "conversation reservation namespace is not a directory",
            )),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }

    /// Probe for the one legacy marker that matters: an existing `sessions`
    /// tree. This is the only place the storage owner inspects the old
    /// allocation root, it is reached only while the reservation namespace is
    /// absent, and it increments [`LEGACY_LAYOUT_PROBES`] so the reservation
    /// path's zero-inspection contract is measurable.
    fn probe_legacy_sessions(&self) -> io::Result<bool> {
        LEGACY_LAYOUT_PROBES.fetch_add(1, Ordering::Relaxed);
        match std::fs::symlink_metadata(self.root.join("sessions")) {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }

    /// Ensure the private reservation namespace exists and its directory entry
    /// is durable, or refuse a legacy populated root. This never enumerates
    /// Session or Conversation allocations.
    fn ensure_reservation_namespace(&self) -> io::Result<PathBuf> {
        let namespace = self.reservation_namespace();
        if self.reservation_namespace_present()? {
            // The namespace name is visible, but visibility is not durability:
            // it may be residue from an initializer that created it and then
            // failed or crashed before the product-root parent-entry barrier.
            // A later caller must not infer completed initialization from the
            // directory's existence, so re-establish the barrier here. This is
            // idempotent and also handles a retry after a failed barrier left
            // the namespace visible.
            self.complete_namespace_initialization()?;
            return Ok(namespace);
        }
        enter_reservation_init_gate();
        if self.probe_legacy_sessions()? {
            // Same concurrent-initializer reasoning as `validate_or_fresh`.
            if self.reservation_namespace_present()? {
                self.complete_namespace_initialization()?;
                return Ok(namespace);
            }
            return Err(unsupported_layout_error());
        }
        fs_operations::record_mkdir();
        match std::fs::create_dir(&namespace) {
            Ok(()) => {}
            // A concurrent initializer won the same exclusive directory creation.
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
        self.complete_namespace_initialization()?;
        Ok(namespace)
    }

    /// Complete the namespace-initialization durability obligation: persist
    /// the namespace entry in the product root (and the root's own entry in its
    /// parent when `create_dir_all` created it). Safe to call repeatedly and
    /// required on every path that observes the namespace, because an
    /// existing directory is not proof that the barrier already ran. A crash
    /// before the barrier may lose the empty namespace, which is safe: nothing
    /// was reserved yet, and a root with no `sessions` tree is fresh again.
    fn complete_namespace_initialization(&self) -> io::Result<()> {
        reservation_namespace_root_barrier(&self.root)
    }

    /// Exclusively reserve one Conversation identity.
    ///
    /// The exclusive create-new of the marker is the one allocation
    /// linearization point: exactly one caller can observe `Ok`, and every
    /// later attempt for the same identity observes `AlreadyExists` without
    /// overwriting the consumed marker. The operation performs no enumeration
    /// of existing Session directories or Conversation allocations.
    ///
    /// Durability: the marker file data and its directory entry are synced
    /// before success is reported. The filesystem's exclusive create alone is
    /// not a power-loss guarantee; the file fsync plus the namespace-directory
    /// fsync are.
    ///
    /// A reserved identity is consumed for the lifetime of the root. This
    /// method never unlinks a marker, including after later preparation or
    /// publication failure; ordinary deletion and orphan cleanup must not make
    /// the identity reusable.
    /// # Errors
    /// Returns `AlreadyExists` when the identity is already consumed, and the
    /// unsupported-layout error when the root predates the reservation
    /// namespace.
    pub(crate) fn reserve_conversation(
        &self,
        conversation: &crate::runtime::identity::ConversationId,
    ) -> io::Result<ConversationReservation> {
        let namespace = self.ensure_reservation_namespace()?;
        let marker = self.confined(&namespace.join(conversation.as_str()))?;
        fs_operations::record_create_open();
        let mut file = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&marker)
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                CONVERSATION_RESERVATION_CONFLICTS.fetch_add(1, Ordering::Relaxed);
                return Err(error);
            }
            Err(error) => return Err(error),
        };
        fs_operations::record_write();
        file.write_all(conversation.as_str().as_bytes())?;
        fs_operations::record_fsync();
        file.sync_all()?;
        drop(file);
        // Persist the marker's directory entry in the namespace before
        // reporting durable reservation success.
        fs_operations::record_dir_fsync();
        File::open(&namespace)?.sync_all()?;
        CONVERSATION_RESERVATIONS.fetch_add(1, Ordering::Relaxed);
        Ok(ConversationReservation)
    }
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }
    /// Freeze native ownership transitions, without excluding ordinary activity.
    /// # Errors
    /// A concurrent ownership transaction or preflight returns `WouldBlock`.
    pub(crate) fn freeze_ownership(&self) -> io::Result<OwnershipSnapshot> {
        Ok(OwnershipSnapshot {
            _lock: lock(directory(&self.root)?, FlockArg::LockExclusiveNonblock)?,
        })
    }
    /// Enter a native ownership transaction before accessing SQLite/catalog state.
    /// # Errors
    /// An ownership snapshot returns `WouldBlock`; callers must not mutate.
    pub(crate) fn ownership_mutation(&self) -> io::Result<OwnershipMutation> {
        Ok(OwnershipMutation {
            _lock: lock(directory(&self.root)?, FlockArg::LockSharedNonblock)?,
        })
    }
    /// Admit a runtime identity reservation or ownership commit behind a snapshot.
    /// The uncontended path stays synchronous (including native capacity ordering).
    /// Only OS lock contention moves to the blocking pool; callers must acquire
    /// this before allocation/registry/lifecycle/durable commit mutexes, and release it before
    /// child staging, capacity waiting, physical settlement or driver handoff.
    pub(crate) async fn runtime_ownership_admission(&self) -> io::Result<OwnershipMutation> {
        match self.ownership_mutation() {
            Ok(admission) => return Ok(admission),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(error),
        }
        let root = self.clone();
        tokio::task::spawn_blocking(move || {
            Ok(OwnershipMutation {
                _lock: lock(directory(&root.root)?, FlockArg::LockShared)?,
            })
        })
        .await
        .map_err(io::Error::other)?
    }

    /// Validates an identity-derived allocation, including missing leaves.
    /// No symlink below the canonical product root is a storage identity.
    ///
    /// # Errors
    /// Rejects escapes, non-normal components and symlinks, including dangling ones.
    pub fn confined(&self, path: &Path) -> io::Result<PathBuf> {
        let relative = path
            .strip_prefix(&self.root)
            .map_err(|_| io::Error::other("storage path escapes canonical runtime root"))?;
        let mut current = self.root.clone();
        for component in relative.components() {
            let std::path::Component::Normal(name) = component else {
                return Err(io::Error::other("invalid storage path component"));
            };
            current.push(name);
            match std::fs::symlink_metadata(&current) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return Err(io::Error::other(
                        "symlink is not a private storage identity",
                    ));
                }
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        Ok(current)
    }
}

/// The sole native Session/catalog controller, independent of target access.
#[derive(Debug)]
pub struct ProductController {
    root: ProductRoot,
    _lock: Flock<File>,
}
impl std::ops::Deref for ProductController {
    type Target = ProductRoot;
    fn deref(&self) -> &ProductRoot {
        &self.root
    }
}
impl ProductController {
    /// Admit a controller at explicit startup, creating only startup metadata.
    /// # Errors
    /// Another controller or invalid storage returns an error.
    pub fn acquire(root: &Path) -> io::Result<Self> {
        let root = ProductRoot::create(root)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(root.root().join(".product-writer.lock"))?;
        Ok(Self {
            root,
            _lock: lock(file, FlockArg::LockExclusiveNonblock)?,
        })
    }
}
#[derive(Debug)]
pub(crate) struct OwnershipSnapshot {
    _lock: Flock<File>,
}
/// OS-backed participation in local ownership transitions. Workspace disposal
/// retains this authority through physical mutation and durable settlement.
#[derive(Debug)]
pub(crate) struct OwnershipMutation {
    _lock: Flock<File>,
}

/// Shared native access to one authoritative Conversation allocation.
#[derive(Debug)]
pub struct ConversationAccess {
    root: ProductRoot,
    _lock: Flock<File>,
    controller: Option<Arc<ProductController>>,
}
impl std::ops::Deref for ConversationAccess {
    type Target = ProductRoot;
    fn deref(&self) -> &ProductRoot {
        &self.root
    }
}
impl ConversationAccess {
    /// Access an existing identity-derived allocation without creating it.
    /// # Errors
    /// Missing/unsafe allocations or a destructive owner are rejected.
    pub fn existing(root: &ProductRoot, allocation: &Path) -> io::Result<Self> {
        let path = root.confined(allocation)?;
        let access = Self {
            root: root.clone(),
            _lock: lock(directory(&path)?, FlockArg::LockSharedNonblock)?,
            controller: None,
        };
        // Acquire private access BEFORE consulting catalog visibility. A delete
        // cannot pass fresh preflight while this shared allocation lock exists;
        // if it committed first, the atomic catalog read rejects residue.
        crate::local_runtime::session::SessionCatalog::check_allocation_live(root, &path)?;
        Ok(access)
    }
    pub(crate) fn existing_for_controller(
        controller: Arc<ProductController>,
        allocation: &Path,
    ) -> io::Result<Self> {
        let mut access = Self::existing(&controller, allocation)?;
        access.controller = Some(controller);
        Ok(access)
    }

    /// Explicit runtime startup reserves an allocation before any private writes.
    /// # Errors
    /// Unsafe paths or a destructive owner are rejected.
    pub(crate) fn start(controller: Arc<ProductController>, allocation: &Path) -> io::Result<Self> {
        let path = controller.confined(allocation)?;
        let _mutation = controller.ownership_mutation()?;
        crate::local_runtime::session::SessionCatalog::check_allocation_live(&controller, &path)?;
        std::fs::create_dir_all(&path)?;
        Self::existing_for_controller(controller, &path)
    }
}
/// Exclusive access acquired in sorted authoritative `ConversationId` order.
#[derive(Debug)]
pub(crate) struct ConversationExclusion {
    _lock: Flock<File>,
}
/// One exclusive, durable reservation of a Conversation identity.
///
/// The value is the storage owner's explicit success result. Callers depend on
/// it rather than inspecting a marker path, `SQLite` filename, directory
/// existence, or traversal algorithm. Dropping it does not release the
/// reservation: the consumed identity is permanent for the life of the
/// product root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ConversationReservation;

fn unsupported_layout_error() -> io::Error {
    io::Error::other(
        "unsupported local runtime layout: this populated root predates the Conversation \
         reservation namespace, so its allocated identities cannot be proven unused. Back up the \
         runtime root, then start from a new empty root (or delete the old root outright after the \
         backup). rustX never deletes or reinterprets the old data.",
    )
}
impl ConversationExclusion {
    pub(crate) fn acquire(root: &ProductRoot, allocation: &Path) -> io::Result<Self> {
        Ok(Self {
            _lock: lock(
                directory(&root.confined(allocation)?)?,
                FlockArg::LockExclusiveNonblock,
            )?,
        })
    }
}
fn directory(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
}
fn lock(file: File, mode: FlockArg) -> io::Result<Flock<File>> {
    Flock::lock(file, mode).map_err(|(_, error)| {
        if error == nix::errno::Errno::EWOULDBLOCK {
            io::Error::new(io::ErrorKind::WouldBlock, "local product storage is in use")
        } else {
            io::Error::other(error)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::process::{Command, Stdio};

    #[test]
    fn lifecycle_process_gate() {
        let Some(root) = std::env::var_os("RUSTX_260_LOCK_ROOT") else {
            return;
        };
        let root = ProductRoot::existing(Path::new(&root)).unwrap();
        let allocation = root.root().join("conversation-b");
        let _guard: Box<dyn std::any::Any> = match std::env::var("RUSTX_260_LOCK_MODE")
            .unwrap()
            .as_str()
        {
            "controller" => Box::new(ProductController::acquire(root.root()).unwrap()),
            "access" => Box::new(ConversationAccess::existing(&root, &allocation).unwrap()),
            "exclusive" => Box::new(ConversationExclusion::acquire(&root, &allocation).unwrap()),
            _ => panic!("invalid gate mode"),
        };
        println!("LIFECYCLE_ACQUIRED");
        std::io::stdout().flush().unwrap();
        std::io::stdin().read_exact(&mut [0]).unwrap();
    }
    fn owner(root: &Path, mode: &str) -> std::process::Child {
        let mut process = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "runtime::local_storage::tests::lifecycle_process_gate",
                "--nocapture",
            ])
            .env("RUSTX_260_LOCK_ROOT", root)
            .env("RUSTX_260_LOCK_MODE", mode)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut output = BufReader::new(process.stdout.take().unwrap());
        loop {
            let mut line = String::new();
            assert_ne!(
                output.read_line(&mut line).unwrap(),
                0,
                "owner exited before gate"
            );
            if line.trim() == "LIFECYCLE_ACQUIRED" {
                break;
            }
        }
        process.stdout = Some(output.into_inner());
        process
    }
    #[test]
    fn cross_process_target_conflicts_aliases_death_and_independent_roots() {
        let directory = tempfile::tempdir().unwrap();
        let root = ProductRoot::existing(directory.path()).unwrap();
        let allocation = root.root().join("conversation-b");
        std::fs::create_dir(&allocation).unwrap();
        let aliases = tempfile::tempdir().unwrap();
        let alias = aliases.path().join("alias");
        std::os::unix::fs::symlink(root.root(), &alias).unwrap();
        for mode in ["access", "exclusive"] {
            let mut process = owner(root.root(), mode);
            for spelling in [
                root.root().to_path_buf(),
                alias.clone(),
                root.root()
                    .join("../")
                    .join(root.root().file_name().unwrap()),
            ] {
                let identity = ProductRoot::existing(&spelling).unwrap();
                assert_eq!(
                    ConversationExclusion::acquire(
                        &identity,
                        &identity.root().join("conversation-b")
                    )
                    .unwrap_err()
                    .kind(),
                    io::ErrorKind::WouldBlock
                );
            }
            let independent = tempfile::tempdir().unwrap();
            let other = ProductRoot::existing(independent.path()).unwrap();
            std::fs::create_dir(other.root().join("conversation-b")).unwrap();
            let _unrelated =
                ConversationExclusion::acquire(&other, &other.root().join("conversation-b"))
                    .unwrap();
            process.kill().unwrap();
            process.wait().unwrap();
            let exclusive = ConversationExclusion::acquire(&root, &allocation).unwrap();
            assert!(ConversationAccess::existing(&root, &allocation).is_err());
            drop(exclusive);
            assert!(ConversationAccess::existing(&root, &allocation).is_ok());
        }
    }
    /// The child side of R05: reserve one identity, announce it, then park so
    /// the owning test can SIGKILL a process that has provably completed the
    /// reservation's durability barrier and nothing else.
    #[test]
    fn reservation_process_gate() {
        let Some(root) = std::env::var_os("RUSTX_387_RESERVE_ROOT") else {
            return;
        };
        let conversation = crate::runtime::identity::ConversationId::new(
            std::env::var("RUSTX_387_RESERVE_ID").unwrap(),
        );
        let product = ProductRoot::create(Path::new(&root)).unwrap();
        product.reserve_conversation(&conversation).unwrap();
        println!("RESERVED");
        std::io::stdout().flush().unwrap();
        std::io::stdin().read_exact(&mut [0]).unwrap();
    }

    /// R05: SIGKILL a real process at the documented post-reservation boundary,
    /// restart, and prove the consumed identity cannot be allocated again while
    /// a fresh one still can. This is real process death, not a dropped object.
    #[test]
    fn r05_kill_after_reservation_never_reissues_consumed_identity() {
        let directory = tempfile::tempdir().unwrap();
        let consumed = crate::runtime::identity::ConversationId::new(
            "conv_01900000-0000-7000-8000-000000000099",
        );
        let mut process = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "runtime::local_storage::tests::reservation_process_gate",
                "--nocapture",
            ])
            .env("RUSTX_387_RESERVE_ROOT", directory.path())
            .env("RUSTX_387_RESERVE_ID", consumed.as_str())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut output = BufReader::new(process.stdout.take().unwrap());
        loop {
            let mut line = String::new();
            assert_ne!(
                output.read_line(&mut line).unwrap(),
                0,
                "child exited before its reservation gate"
            );
            if line.trim() == "RESERVED" {
                break;
            }
        }
        process.kill().unwrap();
        process.wait().unwrap();

        let product = ProductRoot::existing(directory.path()).unwrap();
        assert_eq!(
            product.reserve_conversation(&consumed).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists,
            "a consumed identity was reissued after process death"
        );
        let fresh = crate::runtime::identity::ConversationId::new(
            "conv_01900000-0000-7000-8000-000000000098",
        );
        assert!(product.reserve_conversation(&fresh).is_ok());
    }

    /// A visible namespace is not proof that initialization durability
    /// completed. Build the namespace without the barrier (exactly the residue
    /// an initializer leaves if it dies after `create_dir`), then prove a later
    /// reservation performs the product-root barrier itself instead of
    /// inferring durability from the directory's existence.
    #[test]
    fn namespace_visibility_is_not_initialization_durability() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir(directory.path().join("conversation-reservations")).unwrap();
        let before = reservation_namespace_root_barrier_count_on_thread();
        let product = ProductRoot::existing(directory.path()).unwrap();
        let conversation = crate::runtime::identity::ConversationId::new(
            "conv_01900000-0000-7000-8000-0000000000a1",
        );
        product.reserve_conversation(&conversation).unwrap();
        let barriers = reservation_namespace_root_barrier_count_on_thread() - before;
        assert!(
            barriers >= 1,
            "a reservation accepted a visible namespace without completing the \
             product-root durability barrier"
        );
    }

    /// A failed initialization barrier leaves the namespace visible. A later
    /// retry must not mistake that residue for completed durable initialization
    /// and must complete the obligation itself before reporting success.
    #[test]
    fn failed_initialization_barrier_residue_is_completed_by_retry() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir(directory.path().join("conversation-reservations")).unwrap();
        let product = ProductRoot::existing(directory.path()).unwrap();
        let conversation = crate::runtime::identity::ConversationId::new(
            "conv_01900000-0000-7000-8000-0000000000a2",
        );
        fail_next_reservation_namespace_root_barriers(1);
        assert!(
            product.reserve_conversation(&conversation).is_err(),
            "a failed initialization barrier must not report reservation success"
        );
        assert!(
            directory.path().join("conversation-reservations").is_dir(),
            "the failed barrier leaves the namespace visible as residue"
        );
        let before = reservation_namespace_root_barrier_count_on_thread();
        assert!(
            product.reserve_conversation(&conversation).is_ok(),
            "retry must complete the initialization barrier and reserve"
        );
        assert!(reservation_namespace_root_barrier_count_on_thread() > before);
    }

    /// A concurrent initializer that creates the namespace (and then the
    /// `sessions` tree) must not make a parked observer reject the valid
    /// new-format root as a legacy layout. The gate parks the observer between
    /// its namespace read and the legacy probe, so the race is deterministic.
    #[test]
    fn concurrent_fresh_root_initialization_is_not_rejected_as_legacy() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().to_path_buf();
        let gate = Arc::new(crate::runtime::conversation_runtime::Gate::default());
        let release = gate.arm_scoped();
        let racer_root = root.clone();
        let racer_gate = gate.clone();
        let racer = std::thread::spawn(move || {
            install_reservation_init_gate(racer_gate);
            ProductRoot::existing(&racer_root).map(|_| ())
        });
        gate.wait_entered();
        // The racer has read the namespace as absent. A concurrent initializer
        // now wins: namespace first, then the `sessions` tree.
        let winner = ProductRoot::create(&root).unwrap();
        std::fs::create_dir(root.join("sessions")).unwrap();
        winner
            .reserve_conversation(&crate::runtime::identity::ConversationId::new(
                "conv_01900000-0000-7000-8000-0000000000a3",
            ))
            .unwrap();
        drop(release);
        assert!(
            racer.join().unwrap().is_ok(),
            "a concurrently initialized fresh root was rejected as a legacy layout"
        );
    }

    /// Repeated open of a fully initialized root is valid and never consumes
    /// or resets an existing reservation.
    #[test]
    fn repeated_open_of_initialized_root_is_valid() {
        let directory = tempfile::tempdir().unwrap();
        let conversation = crate::runtime::identity::ConversationId::new(
            "conv_01900000-0000-7000-8000-0000000000a4",
        );
        {
            let product = ProductRoot::create(directory.path()).unwrap();
            product.reserve_conversation(&conversation).unwrap();
        }
        for _ in 0..3 {
            let product = ProductRoot::existing(directory.path()).unwrap();
            assert_eq!(
                product
                    .reserve_conversation(&conversation)
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::AlreadyExists
            );
        }
    }

    #[test]
    fn cross_process_controller_admission_is_independent_of_target_exclusion() {
        let directory = tempfile::tempdir().unwrap();
        let root = ProductRoot::existing(directory.path()).unwrap();
        std::fs::create_dir(root.root().join("conversation-b")).unwrap();
        let alias_dir = tempfile::tempdir().unwrap();
        let alias = alias_dir.path().join("alias");
        std::os::unix::fs::symlink(root.root(), &alias).unwrap();
        let mut process = owner(&alias, "controller");
        assert_eq!(
            ProductController::acquire(root.root()).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        let _snapshot = root.freeze_ownership().unwrap();
        let _target =
            ConversationExclusion::acquire(&root, &root.root().join("conversation-b")).unwrap();
        process.stdin.take().unwrap().write_all(b"x").unwrap();
        assert!(process.wait().unwrap().success());
        assert!(ProductController::acquire(root.root()).is_ok());
    }
    #[test]
    fn management_lock_lookup_is_noncreating_and_paths_fail_closed() {
        let directory = tempfile::tempdir().unwrap();
        let root = ProductRoot::existing(directory.path()).unwrap();
        let missing = root.root().join("unknown");
        assert!(ProductRoot::existing(&missing).is_err());
        assert!(ConversationAccess::existing(&root, &missing).is_err());
        assert!(ConversationExclusion::acquire(&root, &missing).is_err());
        assert!(!missing.exists());
        std::os::unix::fs::symlink("/tmp", root.root().join("escape")).unwrap();
        assert!(root.confined(&root.root().join("escape/unknown")).is_err());
        assert!(root.confined(&root.root().join("../outside")).is_err());
    }
    #[test]
    fn native_private_storage_rejects_symlinks_before_creating_stores() {
        use crate::runtime::identity::ConversationId;
        use crate::tools::runtime::{ConversationRuntimeConfig, ConversationToolRuntime};
        for leaf in [
            "artifacts",
            "artifacts/conversation.sqlite",
            "artifacts/tool-output",
        ] {
            let directory = tempfile::tempdir().unwrap();
            let product = ProductRoot::create(&directory.path().join("product")).unwrap();
            let allocation = product.root().join("conversation");
            std::fs::create_dir(&allocation).unwrap();
            let access = Arc::new(ConversationAccess::existing(&product, &allocation).unwrap());
            let external = directory.path().join("external");
            std::fs::create_dir(&external).unwrap();
            let path = allocation.join(leaf);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::os::unix::fs::symlink(&external, &path).unwrap();
            let mut config =
                ConversationRuntimeConfig::new(&external, allocation.join("artifacts"));
            config.lifecycle = Some(access);
            assert!(
                ConversationToolRuntime::from_config(
                    ConversationId::new("conv_8b34dbc2-c05e-74d7-825d-48efeace8245"),
                    config
                )
                .is_err()
            );
            assert_eq!(std::fs::read_dir(&external).unwrap().count(), 0);
        }
    }
}
