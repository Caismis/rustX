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

// Per-thread count of legacy-layout probes. The process-wide counter is
// contaminated by unrelated parallel tests, so a regression that asserts the
// reservation path inspects zero existing Session trees measures the calling
// thread's delta. Reservation is synchronous on the caller's thread.
#[cfg(test)]
thread_local! {
    static LEGACY_LAYOUT_PROBES_ON_THREAD: std::cell::Cell<u64> =
        const { std::cell::Cell::new(0) };
}

/// The number of legacy-layout probes performed on the calling thread.
#[cfg(test)]
#[must_use]
pub(crate) fn conversation_legacy_layout_probe_count_on_thread() -> u64 {
    LEGACY_LAYOUT_PROBES_ON_THREAD.with(std::cell::Cell::get)
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
        match std::fs::symlink_metadata(self.reservation_namespace()) {
            Ok(metadata) if metadata.is_dir() => return Ok(()),
            Ok(_) => {
                return Err(io::Error::other(
                    "conversation reservation namespace is not a directory",
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        if self.probe_legacy_sessions()? {
            return Err(unsupported_layout_error());
        }
        Ok(())
    }

    /// Probe for the one legacy marker that matters: an existing `sessions`
    /// tree. This is the only place the storage owner inspects the old
    /// allocation root, it is reached only while the reservation namespace is
    /// absent, and it increments [`LEGACY_LAYOUT_PROBES`] so the reservation
    /// path's zero-inspection contract is measurable.
    fn probe_legacy_sessions(&self) -> io::Result<bool> {
        LEGACY_LAYOUT_PROBES.fetch_add(1, Ordering::Relaxed);
        #[cfg(test)]
        LEGACY_LAYOUT_PROBES_ON_THREAD.with(|probes| probes.set(probes.get() + 1));
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
        match std::fs::symlink_metadata(&namespace) {
            Ok(metadata) if metadata.is_dir() => return Ok(namespace),
            Ok(_) => {
                return Err(io::Error::other(
                    "conversation reservation namespace is not a directory",
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        if self.probe_legacy_sessions()? {
            return Err(unsupported_layout_error());
        }
        match std::fs::create_dir(&namespace) {
            Ok(()) => {}
            // A concurrent initializer won the same exclusive directory creation.
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
        // Persist the namespace entry in the product root before any marker
        // inside it can be considered durable. A crash before this point may
        // lose the empty namespace, which is safe: nothing was reserved yet,
        // and a root with no `sessions` tree is legitimately fresh again.
        File::open(&self.root)?.sync_all()?;
        Ok(namespace)
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
        file.write_all(conversation.as_str().as_bytes())?;
        file.sync_all()?;
        drop(file);
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
