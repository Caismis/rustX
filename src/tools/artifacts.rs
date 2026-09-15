//! The conversation-owned development artifact store.
//!
//! M5 implements the smallest conversation/runtime-owned artifact store
//! shared by the tool plane and bounded App Server carrier: opaque monotonic [`ArtifactId`] allocation,
//! local filesystem storage outside the model workspace, and streaming
//! spooling so large subprocess output never has to be held entirely in
//! memory. The mapping from `ArtifactId` to physical path stays internal;
//! [`FileReference`](crate::message::content::FileReference) remains the
//! model/runtime reference. Session-owned roots survive cold reopen and are
//! removed by Session deletion. Existing artifact files are never truncated.

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
    /// The artifact sequence space is exhausted.
    SequenceExhausted,
    /// The artifact cannot be written.
    WriteFailed(String),
}

impl core::fmt::Display for ArtifactError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::RootUnavailable(message) => write!(f, "artifact root unavailable: {message}"),
            Self::SequenceExhausted => write!(f, "the artifact sequence space is exhausted"),
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
    /// The first allocation receives `artifact_1` and successful allocations
    /// advance strictly monotonically with checked arithmetic; exhaustion
    /// fails explicitly instead of wrapping.
    ///
    /// # Errors
    ///
    /// Returns [`ArtifactError::SequenceExhausted`] when the sequence space
    /// is exhausted.
    pub fn create_artifact(&self) -> Result<ArtifactId, ArtifactError> {
        let mut state = self.state();
        let next = state
            .next
            .checked_add(1)
            .ok_or(ArtifactError::SequenceExhausted)?;
        state.next = next;
        let reservation = self.root.join(format!("artifact_{next}.reserved"));
        File::options()
            .create_new(true)
            .write(true)
            .open(reservation)
            .and_then(|file| file.sync_all())
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
        Ok(ArtifactWriter {
            file,
            _lifecycle: self.lifecycle.clone(),
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
    pub fn put_bounded(&self, bytes: &[u8]) -> Result<ArtifactId, ArtifactError> {
        if bytes.len() > ARTIFACT_TRANSFER_MAX {
            return Err(ArtifactError::WriteFailed(
                "artifact exceeds upload limit".into(),
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
    file: File,
}

impl Write for ArtifactWriter {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.file.write(buffer)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
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
        let store = ArtifactStore::new(ConversationId::new("A"), &dir).unwrap();
        let id = store.put_bounded(b"canonical attachment").unwrap();
        let reserved = store.create_artifact().unwrap();
        drop(store);
        let reopened = ArtifactStore::new(ConversationId::new("A"), &dir).unwrap();
        let next = reopened.put_bounded(b"new attachment").unwrap();
        assert_ne!(id, next);
        assert_ne!(reserved, next);
        assert_eq!(reopened.read_bounded(&id).unwrap(), b"canonical attachment");
        assert!(reopened.open_writer(&id).is_err());
        assert_eq!(reopened.read_bounded(&id).unwrap(), b"canonical attachment");
        let other = tempfile::tempdir().unwrap();
        let other = ArtifactStore::new(ConversationId::new("B"), other.path()).unwrap();
        assert!(other.read_bounded(&id).is_err());
    }

    #[test]
    fn bounded_carrier_rejects_paths_missing_symlinks_and_oversize() {
        use crate::runtime::identity::ArtifactId;
        let dir = tempfile::tempdir().unwrap();
        let store = ArtifactStore::new(ConversationId::new("A"), &dir).unwrap();
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
        let store = ArtifactStore::new(ConversationId::new("conv-1"), &dir).expect("store");
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

    #[test]
    fn sequence_exhaustion_fails_explicitly() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = ArtifactStore::new(ConversationId::new("conv-1"), &dir).expect("store");
        store.state.lock().expect("lock").next = u64::MAX;
        assert_eq!(
            store.create_artifact().expect_err("exhausted"),
            ArtifactError::SequenceExhausted
        );
    }

    #[test]
    fn written_bytes_are_retained_verbatim() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = ArtifactStore::new(ConversationId::new("conv-1"), &dir).expect("store");
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
