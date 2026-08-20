//! The conversation-owned development artifact store.
//!
//! M5 implements the smallest conversation/runtime-owned artifact store
//! needed by the tool plane: opaque monotonic [`ArtifactId`] allocation,
//! local filesystem storage outside the model workspace, and streaming
//! spooling so large subprocess output never has to be held entirely in
//! memory. The mapping from `ArtifactId` to physical path stays internal;
//! [`FileReference`](crate::message::content::FileReference) remains the
//! model/runtime reference. Conversation-lifetime retention is sufficient for
//! the current tool-plane contract; artifact recovery/database authority is
//! outside Issue #11.

use std::fs::File;
use std::io::Write;
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
    conversation_id: ConversationId,
    root: PathBuf,
    state: Arc<Mutex<ArtifactStoreState>>,
}

impl ArtifactStore {
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
        Ok(Self {
            conversation_id,
            root,
            state: Arc::new(Mutex::new(ArtifactStoreState { next: 0 })),
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
        let path = self.path_of(id);
        let file = File::options()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&path)
            .map_err(|error| ArtifactError::WriteFailed(format!("{}: {error}", path.display())))?;
        Ok(ArtifactWriter { file })
    }

    /// The physical path of an allocated artifact.
    fn path_of(&self, id: &ArtifactId) -> PathBuf {
        self.root.join(format!("{}.bin", id.as_str()))
    }
}

/// A streaming writer bound to one artifact file.
///
/// Writing appends bytes to the artifact; the file is persisted on drop.
/// This is not a durable recovery backend: fsync guarantees are outside M5.
pub struct ArtifactWriter {
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
    fn allocation_is_monotonic_and_deterministic() {
        let dir = std::env::temp_dir().join(format!(
            "rustx-art-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("t")
        ));
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
        std::fs::remove_dir_all(&dir).expect("remove");
    }

    #[test]
    fn sequence_exhaustion_fails_explicitly() {
        let dir = std::env::temp_dir().join(format!(
            "rustx-art-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("t")
        ));
        let store = ArtifactStore::new(ConversationId::new("conv-1"), &dir).expect("store");
        store.state.lock().expect("lock").next = u64::MAX;
        assert_eq!(
            store.create_artifact().expect_err("exhausted"),
            ArtifactError::SequenceExhausted
        );
        std::fs::remove_dir_all(&dir).expect("remove");
    }

    #[test]
    fn written_bytes_are_retained_verbatim() {
        let dir = std::env::temp_dir().join(format!(
            "rustx-art-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("t")
        ));
        let store = ArtifactStore::new(ConversationId::new("conv-1"), &dir).expect("store");
        let id = store.create_artifact().expect("allocate");
        let mut writer = store.open_writer(&id).expect("open");
        writer.write_all(b"hello\n").expect("write");
        writer.write_all(&[0xff, 0x00, b'x']).expect("write");
        drop(writer);
        let path = store.path_of(&id);
        let bytes = std::fs::read(&path).expect("read artifact");
        assert_eq!(bytes, b"hello\n\xff\x00x");
        std::fs::remove_dir_all(&dir).expect("remove");
    }
}
