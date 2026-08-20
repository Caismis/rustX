//! The conversation-owned managed tool-output store.
//!
//! Oversized textual tool output stays textual: the bounded model-visible
//! preview is the canonical replayable record, and only when that textual
//! bound is crossed does the runtime spill the *complete* text into this
//! managed store. A spill file is auxiliary runtime-owned storage — it is
//! not a semantic artifact, not a second canonical history, and never a
//! model `File` modality. The model receives the spill file's absolute path
//! inside ordinary textual tool output and may explicitly Read or Grep it
//! while the file exists.
//!
//! # Ownership boundary
//!
//! The managed-output root is deliberately **not** the artifact store and
//! **not** the enclosing runtime-private directory: the runtime-private
//! region also holds the durable conversation database and semantic
//! artifact internals, which must never become model-readable merely
//! because textual spills are. The filesystem locator authority
//! ([`crate::tools::locator`]) authorizes exactly this root for read-only
//! operations (Read/Grep/Glob) and rejects Write/Edit against it.
//!
//! # Allocation
//!
//! Spill files are allocated lazily, only at the moment a capture crosses
//! its textual bound, under one monotonic per-conversation sequence
//! (`output_1.log`, `output_2.log`, ...). Small output never touches the
//! filesystem. Writes stream through an open file handle, so a large output
//! is never buffered in memory.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::runtime::identity::ConversationId;

/// A managed tool-output failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManagedOutputError {
    /// The managed-output root cannot be created or canonicalized.
    RootUnavailable(String),
    /// The output sequence space is exhausted.
    SequenceExhausted,
    /// A spill file cannot be opened.
    OpenFailed(String),
}

impl core::fmt::Display for ManagedOutputError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::RootUnavailable(message) => {
                write!(f, "managed tool-output root unavailable: {message}")
            }
            Self::SequenceExhausted => {
                write!(f, "the managed tool-output sequence space is exhausted")
            }
            Self::OpenFailed(message) => write!(f, "cannot open a spill file: {message}"),
        }
    }
}

impl std::error::Error for ManagedOutputError {}

/// The synchronized allocation state of one managed tool-output store.
#[derive(Debug)]
struct ManagedOutputState {
    next: u64,
    /// Test-only seam: when set, every spill open fails, so tests can prove
    /// executors represent spill failure explicitly. Never set outside
    /// `#[cfg(test)]`.
    #[cfg(test)]
    force_open_failures: bool,
}

/// The conversation-owned managed tool-output store.
///
/// Cheaply cloneable and shared by the foreground and background executors
/// of one conversation; allocation is one small mutex critical section and
/// writes append through the returned file handle.
#[derive(Clone, Debug)]
pub struct ManagedToolOutput {
    conversation_id: ConversationId,
    root: PathBuf,
    state: Arc<Mutex<ManagedOutputState>>,
}

impl ManagedToolOutput {
    /// Creates the managed tool-output store rooted at `root`.
    ///
    /// The root directory is created when missing and canonicalized once,
    /// so the locator authority compares every managed-output locator
    /// against one canonical root.
    ///
    /// # Errors
    ///
    /// Returns [`ManagedOutputError::RootUnavailable`] when the root cannot
    /// be created or canonicalized.
    pub fn new(
        conversation_id: ConversationId,
        root: impl AsRef<Path>,
    ) -> Result<Self, ManagedOutputError> {
        let root = root.as_ref();
        std::fs::create_dir_all(root).map_err(|error| {
            ManagedOutputError::RootUnavailable(format!("{}: {error}", root.display()))
        })?;
        let canonical = std::fs::canonicalize(root).map_err(|error| {
            ManagedOutputError::RootUnavailable(format!("{}: {error}", root.display()))
        })?;
        Ok(Self {
            conversation_id,
            root: canonical,
            state: Arc::new(Mutex::new(ManagedOutputState {
                next: 0,
                #[cfg(test)]
                force_open_failures: false,
            })),
        })
    }

    /// Test-only seam: forces every subsequent spill open to fail, so tests
    /// can prove that a capture never reports successful retention while
    /// silently losing full output. Only available under `#[cfg(test)]`.
    #[cfg(test)]
    pub(crate) fn set_force_open_failures(&self, enabled: bool) {
        self.state
            .lock()
            .expect("managed tool-output lock poisoned")
            .force_open_failures = enabled;
    }

    /// Test-only seam: exhausts the output sequence so the next allocation
    /// fails explicitly. Only available under `#[cfg(test)]`.
    #[cfg(test)]
    pub(crate) fn exhaust_sequence(&self) {
        self.state
            .lock()
            .expect("managed tool-output lock poisoned")
            .next = u64::MAX;
    }

    /// The conversation this store belongs to.
    #[must_use]
    pub fn conversation_id(&self) -> &ConversationId {
        &self.conversation_id
    }

    /// The canonical managed tool-output root.
    ///
    /// This root — and nothing outside it, in particular not the enclosing
    /// runtime-private directory — is the read-only filesystem region the
    /// locator authority opens to the model.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Allocates and opens one spill file for streaming complete output.
    ///
    /// Allocation is collision-safe by construction: names come from one
    /// monotonic per-conversation sequence and the file is opened with
    /// `create_new`, so two stores sharing a root can never silently
    /// truncate each other's spill.
    ///
    /// # Errors
    ///
    /// Returns [`ManagedOutputError::SequenceExhausted`] when the sequence
    /// space is exhausted and [`ManagedOutputError::OpenFailed`] when the
    /// file cannot be opened.
    ///
    /// # Panics
    ///
    /// Panics only if the allocation lock is poisoned, which would mean a
    /// previous operation panicked while holding the lock.
    pub fn open_spill(&self) -> Result<ToolOutputSpill, ManagedOutputError> {
        let sequence = {
            let mut state = self
                .state
                .lock()
                .expect("managed tool-output allocation lock poisoned");
            let next = state
                .next
                .checked_add(1)
                .ok_or(ManagedOutputError::SequenceExhausted)?;
            #[cfg(test)]
            if state.force_open_failures {
                return Err(ManagedOutputError::OpenFailed(
                    "test-forced spill open failure".to_owned(),
                ));
            }
            state.next = next;
            next
        };
        let path = self.root.join(format!("output_{sequence}.log"));
        let file = File::options()
            .create_new(true)
            .write(true)
            .open(&path)
            .map_err(|error| {
                ManagedOutputError::OpenFailed(format!("{}: {error}", path.display()))
            })?;
        Ok(ToolOutputSpill { file, path })
    }
}

/// One open spill file: the complete output of one textual capture streams
/// through it, and its absolute path is the model-facing locator.
#[derive(Debug)]
pub struct ToolOutputSpill {
    file: File,
    path: PathBuf,
}

impl ToolOutputSpill {
    /// The canonical absolute model-facing locator of this spill file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Streams one chunk into the spill file.
    ///
    /// # Errors
    ///
    /// Returns the underlying I/O error; a failed spill write is an explicit
    /// capture failure, never silently lost output.
    pub fn write_all(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.file.write_all(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::{ManagedOutputError, ManagedToolOutput};
    use crate::runtime::identity::ConversationId;

    #[test]
    fn spill_allocation_is_monotonic_collision_safe_and_absolute() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = ManagedToolOutput::new(
            ConversationId::new("conv-1"),
            dir.path().join("tool-output"),
        )
        .expect("store");
        let first = store.open_spill().expect("first");
        let second = store.open_spill().expect("second");
        assert!(first.path().is_absolute());
        assert!(first.path().ends_with("output_1.log"));
        assert!(second.path().ends_with("output_2.log"));
        assert!(
            first.path().starts_with(store.root()),
            "spill lives under the managed root"
        );
        assert_eq!(std::fs::read(first.path()).expect("read"), b"");
    }

    #[test]
    fn sequence_exhaustion_fails_explicitly() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = ManagedToolOutput::new(
            ConversationId::new("conv-1"),
            dir.path().join("tool-output"),
        )
        .expect("store");
        store.exhaust_sequence();
        assert_eq!(
            store.open_spill().expect_err("exhausted"),
            ManagedOutputError::SequenceExhausted
        );
    }

    #[test]
    fn spilled_bytes_are_retained_verbatim() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = ManagedToolOutput::new(
            ConversationId::new("conv-1"),
            dir.path().join("tool-output"),
        )
        .expect("store");
        let mut spill = store.open_spill().expect("open");
        spill.write_all(b"hello\n").expect("write");
        spill.write_all(&[0xff, 0x00, b'x']).expect("write");
        drop(spill);
        let bytes = std::fs::read(store.root().join("output_1.log")).expect("read");
        assert_eq!(bytes, b"hello\n\xff\x00x");
    }
}
