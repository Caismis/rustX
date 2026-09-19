//! The conversation-owned development artifact store.
//!
//! M5 implements the smallest conversation/runtime-owned artifact store
//! used by Tool-generated semantic artifacts and their bounded App Server read
//! presentation: opaque monotonic [`ArtifactId`] allocation and local filesystem
//! storage outside the model workspace. Managed text spill has its separate owner. The mapping from `ArtifactId` to physical path stays internal;
//! [`FileReference`](crate::message::content::FileReference) remains the
//! model/runtime reference. Session-owned roots survive cold reopen and are
//! removed by Session deletion. Existing artifact files are never truncated.
//! The shared lifetime identity capacity is [`MAX_ARTIFACTS_PER_STORE`]; durable
//! reservations consume slots even when the subsequent byte write is abandoned.

use nix::fcntl::{Flock, FlockArg};
use std::fs::File;
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::runtime::identity::{ArtifactId, ConversationId};

/// An artifact store failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactError {
    /// The artifact root cannot be created.
    RootUnavailable(String),
    /// The store's lifetime allocation capacity has been consumed.
    CapacityExhausted {
        /// Fixed maximum number of allocated identities per store.
        max_artifacts: u64,
    },
    /// The artifact cannot be written.
    WriteFailed(String),
}

impl core::fmt::Display for ArtifactError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::RootUnavailable(message) => write!(f, "artifact root unavailable: {message}"),
            Self::CapacityExhausted { max_artifacts } => {
                write!(
                    f,
                    "artifact capacity exhausted (maximum {max_artifacts} identities)"
                )
            }
            Self::WriteFailed(message) => write!(f, "artifact write failed: {message}"),
        }
    }
}

impl std::error::Error for ArtifactError {}

/// The synchronized allocation state of one artifact store.
#[derive(Debug)]
struct ArtifactStoreState {
    next: u64,
}

/// A conversation-owned artifact store.
///
/// # Panics
///
/// Panics only if the allocation lock is poisoned, which would mean a
/// previous operation panicked while holding the lock.
///
/// All operations are synchronous and bounded: allocation happens under one
/// small mutex critical section, and writes append through an opened file
/// handle obtained from the store. The store is cheaply cloneable and shared
/// by foreground executors and detached background runners of one
/// conversation.
#[derive(Clone, Debug)]
pub struct ArtifactStore {
    lifecycle: Option<Arc<crate::runtime::local_storage::ConversationAccess>>,
    conversation_id: ConversationId,
    root: PathBuf,
    state: Arc<Mutex<ArtifactStoreState>>,
}

impl ArtifactStore {
    pub(crate) fn with_lifecycle(
        mut self,
        access: Option<Arc<crate::runtime::local_storage::ConversationAccess>>,
    ) -> Self {
        self.lifecycle = access;
        self
    }

    /// The synchronized allocation state.
    fn state(&self) -> std::sync::MutexGuard<'_, ArtifactStoreState> {
        self.state
            .lock()
            .expect("artifact store allocation lock poisoned")
    }

    /// Creates the conversation artifact store rooted at `root`.
    ///
    /// The root directory is created when missing. The caller is responsible
    /// for placing the artifact root outside the model workspace.
    ///
    /// # Errors
    ///
    /// Returns [`ArtifactError::RootUnavailable`] when the root cannot be
    /// created.
    pub fn new(
        conversation_id: ConversationId,
        root: impl AsRef<Path>,
    ) -> Result<Self, ArtifactError> {
        let root = root.as_ref().to_path_buf();
        std::fs::create_dir_all(&root).map_err(|error| {
            ArtifactError::RootUnavailable(format!("{}: {error}", root.display()))
        })?;
        let mut next = 0;
        for entry in std::fs::read_dir(&root)
            .map_err(|_| ArtifactError::RootUnavailable("cannot enumerate artifacts".into()))?
        {
            let entry = entry
                .map_err(|_| ArtifactError::RootUnavailable("cannot enumerate artifact".into()))?;
            if let Some(value) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.strip_prefix("artifact_"))
                .and_then(|name| {
                    name.strip_suffix(".bin")
                        .or_else(|| name.strip_suffix(".reserved"))
                })
                .and_then(|name| name.parse::<u64>().ok())
            {
                next = next.max(value);
            }
        }
        Ok(Self {
            lifecycle: None,
            conversation_id,
            root,
            state: Arc::new(Mutex::new(ArtifactStoreState { next })),
        })
    }

    /// The conversation this store belongs to.
    #[must_use]
    pub fn conversation_id(&self) -> &ConversationId {
        &self.conversation_id
    }

    /// The artifact root directory.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Allocates one opaque monotonic artifact id.
    ///
    /// The first allocation receives `artifact_1`. Reservations consume the
    /// fixed lifetime capacity even if syncing or later byte writing fails.
    /// Cold reopen recovers the same frontier from reserved/written ordinals.
    ///
    /// # Errors
    ///
    /// Returns [`ArtifactError::CapacityExhausted`] without mutation at capacity,
    /// or [`ArtifactError::WriteFailed`] if the reservation cannot be persisted.
    pub fn create_artifact(&self) -> Result<ArtifactId, ArtifactError> {
        let mut state = self.state();
        if state.next >= MAX_ARTIFACTS_PER_STORE {
            return Err(ArtifactError::CapacityExhausted {
                max_artifacts: MAX_ARTIFACTS_PER_STORE,
            });
        }
        let next = state.next + 1;
        let reservation = self.root.join(format!("artifact_{next}.reserved"));
        let file = File::options()
            .create_new(true)
            .write(true)
            .open(reservation)
            .map_err(|error| {
                // An already-existing reservation also consumes this identity.
                // Never retry it as a fresh allocation within this runtime.
                if error.kind() == std::io::ErrorKind::AlreadyExists {
                    state.next = next;
                }
                ArtifactError::WriteFailed("artifact reservation failed".into())
            })?;
        // Creation consumes the slot in memory before either durability barrier.
        // A subsequent sync failure must not allow this identity to be reused.
        state.next = next;
        file.sync_all()
            .and_then(|()| File::open(&self.root)?.sync_all())
            .map_err(|_| ArtifactError::WriteFailed("artifact reservation failed".into()))?;
        Ok(ArtifactId::new(format!("artifact_{next}")))
    }

    /// Opens a write handle for streaming bytes into an allocated artifact.
    ///
    /// The physical path mapping is internal to the store; executors only
    /// ever hold an [`ArtifactId`] and a writer.
    ///
    /// # Errors
    ///
    /// Returns [`ArtifactError::WriteFailed`] when the artifact file cannot
    /// be opened.
    ///
    /// # Panics
    ///
    /// Panics only if the store lock is poisoned, which would mean a
    /// previous operation panicked while holding the lock.
    pub fn open_writer(&self, id: &ArtifactId) -> Result<ArtifactWriter, ArtifactError> {
        validate_id(id)?;
        let path = self.path_of(id);
        let file = File::options()
            .create_new(true)
            .write(true)
            .open(&path)
            .map_err(|error| ArtifactError::WriteFailed(format!("{}: {error}", path.display())))?;
        let file = Flock::lock(file, FlockArg::LockExclusiveNonblock)
            .map_err(|(_, error)| ArtifactError::WriteFailed(error.to_string()))?;
        Ok(ArtifactWriter {
            file,
            _lifecycle: self.lifecycle.clone(),
        })
    }

    /// Capture bounded native identity/settled-length facts. Reading unreferenced
    /// artifacts must not turn their active writers into required export data.
    pub(crate) fn archive_lengths(
        root: &Path,
        check: impl Fn() -> std::io::Result<()>,
    ) -> std::io::Result<std::collections::BTreeMap<ArtifactId, Option<u64>>> {
        let mut lengths = std::collections::BTreeMap::new();
        for entry in std::fs::read_dir(root)? {
            check()?;
            let entry = entry?;
            let name = entry.file_name();
            let Some(id) = name.to_str().and_then(|s| s.strip_suffix(".bin")) else {
                continue;
            };
            if !id.starts_with("artifact_") {
                continue;
            }
            let id = ArtifactId::new(id);
            validate_id(&id).map_err(std::io::Error::other)?;
            if u64::try_from(lengths.len()).map_err(std::io::Error::other)?
                >= MAX_ARTIFACTS_PER_STORE
            {
                return Err(std::io::Error::other("artifact identity capacity exceeded"));
            }
            // Successful shared admission proves the native one-shot writer has
            // ended. Create-new semantics make these bytes immutable thereafter.
            let len = Self::open_archive_reader(root, &id)
                .ok()
                .map(|reader| reader.len);
            lengths.insert(id, len);
        }
        Ok(lengths)
    }

    /// Open existing durable bytes without a presentation-size limit.
    /// The caller reads bounded chunks and retains the Conversation allocation.
    /// # Errors
    /// Invalid identities, symlinks, missing bytes and non-files fail explicitly.
    pub(crate) fn open_archive_reader(
        root: &Path,
        id: &ArtifactId,
    ) -> std::io::Result<ArtifactReadHandle> {
        validate_id(id).map_err(std::io::Error::other)?;
        let file = File::options()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(root.join(format!("{}.bin", id.as_str())))?;
        let file = Flock::lock(file, FlockArg::LockSharedNonblock)
            .map_err(|(_, error)| std::io::Error::other(error.to_string()))?;
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return Err(std::io::Error::other("artifact is not a regular file"));
        }
        Ok(ArtifactReadHandle {
            file,
            len: metadata.len(),
        })
    }

    /// Read a finite artifact through its conversation-owned identity.
    /// # Errors
    /// Missing, invalid, non-regular or oversized artifacts are refused.
    pub fn read_bounded(&self, id: &ArtifactId) -> Result<Vec<u8>, ArtifactError> {
        validate_id(id)?;
        let mut file = File::options()
            .read(true)
            .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK)
            .open(self.path_of(id))
            .map_err(|_| ArtifactError::WriteFailed("artifact unavailable".into()))?;
        let metadata = file
            .metadata()
            .map_err(|_| ArtifactError::WriteFailed("artifact unavailable".into()))?;
        if !metadata.is_file() || metadata.len() > ARTIFACT_TRANSFER_MAX as u64 {
            return Err(ArtifactError::WriteFailed(
                "artifact exceeds read limit or is not a regular file".into(),
            ));
        }
        let mut bytes = Vec::with_capacity(ARTIFACT_TRANSFER_MAX + 1);
        Read::by_ref(&mut file)
            .take((ARTIFACT_TRANSFER_MAX + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| ArtifactError::WriteFailed("artifact read failed".into()))?;
        if bytes.len() > ARTIFACT_TRANSFER_MAX {
            return Err(ArtifactError::WriteFailed(
                "artifact exceeds read limit".into(),
            ));
        }
        Ok(bytes)
    }

    /// Publish one bounded byte payload. Never replaces an existing artifact.
    /// # Errors
    /// Oversized content, allocation and durable write failures are returned.
    #[cfg(test)]
    pub(crate) fn put_bounded(&self, bytes: &[u8]) -> Result<ArtifactId, ArtifactError> {
        if bytes.len() > ARTIFACT_TRANSFER_MAX {
            return Err(ArtifactError::WriteFailed(
                "artifact exceeds fixture write limit".into(),
            ));
        }
        let id = self.create_artifact()?;
        let mut writer = self.open_writer(&id)?;
        writer
            .write_all(bytes)
            .and_then(|()| writer.file.sync_all())
            .map_err(|_| ArtifactError::WriteFailed("artifact write failed".into()))?;
        File::open(&self.root)
            .and_then(|directory| directory.sync_all())
            .map_err(|_| ArtifactError::WriteFailed("artifact directory sync failed".into()))?;
        Ok(id)
    }

    /// The physical path of an allocated artifact.
    fn path_of(&self, id: &ArtifactId) -> PathBuf {
        self.root.join(format!("{}.bin", id.as_str()))
    }
}

/// One-shot carrier bound: base64 is at most 349,528 bytes, below 1 MiB ingress.
pub const ARTIFACT_TRANSFER_MAX: usize = 256 * 1024;

/// Lifetime capacity for native Tool/MCP artifacts. The monotonic reserved/
/// written ordinal is the durable frontier, not a live-file count. Session
/// deletion is the reclamation boundary. Workspace uploads use another owner.
pub const MAX_ARTIFACTS_PER_STORE: u64 = 256;

fn validate_id(id: &ArtifactId) -> Result<(), ArtifactError> {
    if id
        .as_str()
        .strip_prefix("artifact_")
        .and_then(|value| value.parse::<u64>().ok())
        .is_none()
    {
        return Err(ArtifactError::WriteFailed(
            "invalid artifact identity".into(),
        ));
    }
    Ok(())
}

/// A streaming writer bound to one artifact file.
///
/// Writing appends bytes to the artifact; the file is persisted on drop.
/// This is not a durable recovery backend: fsync guarantees are outside M5.
pub struct ArtifactWriter {
    _lifecycle: Option<Arc<crate::runtime::local_storage::ConversationAccess>>,
    file: Flock<File>,
}

impl Write for ArtifactWriter {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.file.write(buffer)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

/// A settled, append-immutable artifact. An active native writer is refused.
/// Native writers are exclusive for their whole lifetime; `create_new` prevents
/// reopening a settled identity for mutation. Paths remain private to the store.
pub(crate) struct ArtifactReadHandle {
    file: Flock<File>,
    pub(crate) len: u64,
}
impl Read for ArtifactReadHandle {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.file.read(buffer)
    }
}

#[cfg(test)]
mod tests {
    use super::{ArtifactError, ArtifactStore};
    use crate::runtime::identity::ConversationId;
    use std::io::Write;

    #[test]
    fn cold_reopen_preserves_bytes_and_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let store = ArtifactStore::new(
            ConversationId::new("conv_559aead0-8264-7579-8d39-09718cdd05ab"),
            &dir,
        )
        .unwrap();
        let id = store.put_bounded(b"canonical attachment").unwrap();
        let reserved = store.create_artifact().unwrap();
        drop(store);
        let reopened = ArtifactStore::new(
            ConversationId::new("conv_559aead0-8264-7579-8d39-09718cdd05ab"),
            &dir,
        )
        .unwrap();
        let next = reopened.put_bounded(b"new attachment").unwrap();
        assert_ne!(id, next);
        assert_ne!(reserved, next);
        assert_eq!(reopened.read_bounded(&id).unwrap(), b"canonical attachment");
        assert!(reopened.open_writer(&id).is_err());
        assert_eq!(reopened.read_bounded(&id).unwrap(), b"canonical attachment");
        let other = tempfile::tempdir().unwrap();
        let other = ArtifactStore::new(
            ConversationId::new("conv_df7e70e5-0215-74f4-834b-bee64a9e3789"),
            other.path(),
        )
        .unwrap();
        assert!(other.read_bounded(&id).is_err());
    }

    #[test]
    fn bounded_carrier_rejects_paths_missing_symlinks_and_oversize() {
        use crate::runtime::identity::ArtifactId;
        let dir = tempfile::tempdir().unwrap();
        let store = ArtifactStore::new(
            ConversationId::new("conv_559aead0-8264-7579-8d39-09718cdd05ab"),
            &dir,
        )
        .unwrap();
        assert!(store.read_bounded(&ArtifactId::new("../secret")).is_err());
        assert!(store.read_bounded(&ArtifactId::new("artifact_99")).is_err());
        assert!(
            store
                .put_bounded(&vec![0; super::ARTIFACT_TRANSFER_MAX + 1])
                .is_err()
        );
        let id = store
            .put_bounded(&vec![0; super::ARTIFACT_TRANSFER_MAX])
            .unwrap();
        assert_eq!(
            store.read_bounded(&id).unwrap().len(),
            super::ARTIFACT_TRANSFER_MAX
        );
        let oversized = store.create_artifact().unwrap();
        store
            .open_writer(&oversized)
            .unwrap()
            .write_all(&vec![0; super::ARTIFACT_TRANSFER_MAX + 1])
            .unwrap();
        assert!(store.read_bounded(&oversized).is_err());
        std::os::unix::fs::symlink(store.path_of(&id), dir.path().join("artifact_99.bin")).unwrap();
        assert!(store.read_bounded(&ArtifactId::new("artifact_99")).is_err());
    }

    #[test]
    fn allocation_is_monotonic_and_deterministic() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = ArtifactStore::new(
            ConversationId::new("conv_36524fd8-f674-7fc2-8125-06d01fee0e18"),
            &dir,
        )
        .expect("store");
        assert_eq!(
            store.create_artifact().expect("first").as_str(),
            "artifact_1"
        );
        assert_eq!(
            store.create_artifact().expect("second").as_str(),
            "artifact_2"
        );
        assert_eq!(
            store.create_artifact().expect("third").as_str(),
            "artifact_3"
        );
    }

    fn root_contents(store: &ArtifactStore) -> std::collections::BTreeMap<String, Vec<u8>> {
        std::fs::read_dir(store.root())
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                (
                    entry.file_name().into_string().unwrap(),
                    std::fs::read(entry.path()).unwrap(),
                )
            })
            .collect()
    }

    fn assert_capacity_noop(store: &ArtifactStore) {
        let before = root_contents(store);
        let frontier = store.state().next;
        let error = ArtifactError::CapacityExhausted {
            max_artifacts: super::MAX_ARTIFACTS_PER_STORE,
        };
        assert_eq!(store.create_artifact(), Err(error.clone()));
        assert_eq!(store.put_bounded(b"rejected"), Err(error));
        assert_eq!(store.state().next, frontier);
        assert_eq!(
            root_contents(store),
            before,
            "no reservation, bytes or overwrite"
        );
        assert!(!store.root().join("artifact_257.reserved").exists());
        assert!(!store.root().join("artifact_257.bin").exists());
    }

    #[test]
    fn artifact_capacity_exact_boundary_is_mutation_free_and_survives_cold_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let store = ArtifactStore::new(
            ConversationId::new("conv_559aead0-8264-7579-8d39-09718cdd05ab"),
            &dir,
        )
        .unwrap();
        for ordinal in 1..=super::MAX_ARTIFACTS_PER_STORE {
            let bytes = ordinal.to_le_bytes();
            let id = store.put_bounded(&bytes).unwrap();
            assert_eq!(id.as_str(), format!("artifact_{ordinal}"));
        }
        assert_capacity_noop(&store);
        let before = root_contents(&store);
        drop(store);
        let reopened = ArtifactStore::new(
            ConversationId::new("conv_559aead0-8264-7579-8d39-09718cdd05ab"),
            &dir,
        )
        .unwrap();
        assert_capacity_noop(&reopened);
        assert_eq!(root_contents(&reopened), before);
        for ordinal in 1..=super::MAX_ARTIFACTS_PER_STORE {
            let id = crate::runtime::identity::ArtifactId::new(format!("artifact_{ordinal}"));
            assert_eq!(reopened.read_bounded(&id).unwrap(), ordinal.to_le_bytes());
        }
    }

    #[test]
    fn artifact_capacity_counts_unwritten_and_failed_reserved_slots_after_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let store = ArtifactStore::new(
            ConversationId::new("conv_559aead0-8264-7579-8d39-09718cdd05ab"),
            &dir,
        )
        .unwrap();
        let retained = store.put_bounded(b"retained").unwrap();
        for _ in 1..super::MAX_ARTIFACTS_PER_STORE {
            store.create_artifact().unwrap();
        }
        // The final identity has only a reservation; no byte writer ever ran.
        assert!(store.root().join("artifact_256.reserved").exists());
        assert!(!store.root().join("artifact_256.bin").exists());
        let unwritten = crate::runtime::identity::ArtifactId::new("artifact_256");
        // Force a byte-open failure after successful reservation, then leave
        // only the reservation as an abandoned producer would.
        std::fs::create_dir(store.path_of(&unwritten)).unwrap();
        assert!(store.open_writer(&unwritten).is_err());
        std::fs::remove_dir(store.path_of(&unwritten)).unwrap();
        assert_capacity_noop(&store);
        drop(store);
        let reopened = ArtifactStore::new(
            ConversationId::new("conv_559aead0-8264-7579-8d39-09718cdd05ab"),
            &dir,
        )
        .unwrap();
        assert_capacity_noop(&reopened);
        assert_eq!(reopened.read_bounded(&retained).unwrap(), b"retained");
    }

    #[test]
    fn artifact_capacity_old_stores_above_limit_remain_readable() {
        for suffix in ["bin", "reserved"] {
            for frontier in [super::MAX_ARTIFACTS_PER_STORE + 1, u64::MAX] {
                let dir = tempfile::tempdir().unwrap();
                std::fs::write(dir.path().join("artifact_1.bin"), b"retained").unwrap();
                std::fs::write(
                    dir.path().join(format!("artifact_{frontier}.{suffix}")),
                    b"",
                )
                .unwrap();
                let store = ArtifactStore::new(
                    ConversationId::new("conv_559aead0-8264-7579-8d39-09718cdd05ab"),
                    &dir,
                )
                .unwrap();
                let before = root_contents(&store);
                assert_eq!(store.state().next, frontier);
                assert!(matches!(
                    store.create_artifact(),
                    Err(ArtifactError::CapacityExhausted { max_artifacts: 256 })
                ));
                assert_eq!(store.state().next, frontier);
                assert_eq!(root_contents(&store), before);
                assert_eq!(
                    store
                        .read_bounded(&crate::runtime::identity::ArtifactId::new("artifact_1"))
                        .unwrap(),
                    b"retained"
                );
            }
        }
    }

    #[test]
    fn reservation_creation_failure_does_not_advance_but_existing_reservation_does() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("artifacts");
        let store = ArtifactStore::new(
            ConversationId::new("conv_559aead0-8264-7579-8d39-09718cdd05ab"),
            &root,
        )
        .unwrap();
        std::fs::remove_dir(&root).unwrap();
        assert!(store.create_artifact().is_err());
        assert_eq!(store.state().next, 0);
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("artifact_1.reserved"), b"").unwrap();
        assert!(store.create_artifact().is_err());
        assert_eq!(
            store.state().next,
            1,
            "an existing identity must not be reused"
        );
        assert_eq!(store.create_artifact().unwrap().as_str(), "artifact_2");
    }

    #[test]
    fn written_bytes_are_retained_verbatim() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = ArtifactStore::new(
            ConversationId::new("conv_36524fd8-f674-7fc2-8125-06d01fee0e18"),
            &dir,
        )
        .expect("store");
        let id = store.create_artifact().expect("allocate");
        let mut writer = store.open_writer(&id).expect("open");
        writer.write_all(b"hello\n").expect("write");
        writer.write_all(&[0xff, 0x00, b'x']).expect("write");
        drop(writer);
        let path = store.path_of(&id);
        let bytes = std::fs::read(&path).expect("read artifact");
        assert_eq!(bytes, b"hello\n\xff\x00x");
    }
}
