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
//! its textual bound, under one monotonic sequence (`output_1.log`,
//! `output_2.log`, ...). Small output never touches the filesystem. Writes
//! stream through an open file handle, so a large output is never buffered
//! in memory.
//!
//! The sequence is **restart safe** without being durable semantic state:
//! construction seeds the process-local high-water mark from the existing
//! `output_N.log` names in the managed root, and every allocation opens
//! with `create_new`, advancing past any name that already exists. A
//! reconstructed runtime over a retained storage root therefore never
//! fails merely because older spill files exist, and a spill is never
//! truncated or overwritten. The sequence itself is auxiliary storage
//! identity, not conversation state; nothing about it is persisted.

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
    /// The managed-output root already exists as a symlink. The dedicated
    /// root must be a real owned directory, never an alias of another
    /// region (the workspace, the artifact root, or an arbitrary host
    /// directory), because its canonical path is an authorized read root.
    SymlinkRoot(String),
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
            Self::SymlinkRoot(message) => write!(
                f,
                "the managed tool-output root must be a real directory, not a symlink: {message}"
            ),
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
    /// Test-only seam: when set, spill writes fail after this many bytes,
    /// so tests can prove a partial spill is never advertised as complete.
    /// Never set outside `#[cfg(test)]`.
    #[cfg(test)]
    fail_writes_after: Option<u64>,
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
    /// against one canonical root. A pre-existing symlink at the root is
    /// rejected: the dedicated root must be a real owned directory, never
    /// an alias of another filesystem region, because its canonical target
    /// becomes an authorized model-readable root.
    ///
    /// The allocation sequence is seeded from the existing `output_N.log`
    /// names in the root, so reconstructing a store over a retained
    /// storage root continues monotonically instead of colliding with
    /// older spill files.
    ///
    /// # Errors
    ///
    /// Returns [`ManagedOutputError::SymlinkRoot`] when the root already
    /// exists as a symlink and [`ManagedOutputError::RootUnavailable`] when
    /// the root cannot be created, read, or canonicalized.
    pub fn new(
        conversation_id: ConversationId,
        root: impl AsRef<Path>,
    ) -> Result<Self, ManagedOutputError> {
        let root = root.as_ref();
        if let Ok(metadata) = std::fs::symlink_metadata(root)
            && metadata.file_type().is_symlink()
        {
            return Err(ManagedOutputError::SymlinkRoot(root.display().to_string()));
        }
        std::fs::create_dir_all(root).map_err(|error| {
            ManagedOutputError::RootUnavailable(format!("{}: {error}", root.display()))
        })?;
        let canonical = std::fs::canonicalize(root).map_err(|error| {
            ManagedOutputError::RootUnavailable(format!("{}: {error}", root.display()))
        })?;
        let next = spill_high_water(&canonical)?;
        Ok(Self {
            conversation_id,
            root: canonical,
            state: Arc::new(Mutex::new(ManagedOutputState {
                next,
                #[cfg(test)]
                force_open_failures: false,
                #[cfg(test)]
                fail_writes_after: None,
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

    /// Test-only seam: spill writes fail after `bytes` successfully written
    /// bytes, so tests can fail a spill *after* allocation and prove a
    /// partial spill is never advertised as complete. Only available under
    /// `#[cfg(test)]`.
    #[cfg(test)]
    pub(crate) fn fail_spill_writes_after(&self, bytes: u64) {
        self.state
            .lock()
            .expect("managed tool-output lock poisoned")
            .fail_writes_after = Some(bytes);
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
    /// Allocation never overwrites an existing spill: the file is opened
    /// with `create_new`, and when the name already exists — stale spill of
    /// an earlier runtime lifetime over this retained root, or a second
    /// store momentarily sharing the root — the sequence advances to the
    /// next name instead of failing or truncating. The high-water seeding
    /// at construction makes the common restart case collision-free.
    ///
    /// # Errors
    ///
    /// Returns [`ManagedOutputError::SequenceExhausted`] when the sequence
    /// space is exhausted and [`ManagedOutputError::OpenFailed`] when the
    /// file cannot be opened for any reason other than a name collision.
    ///
    /// # Panics
    ///
    /// Panics only if the allocation lock is poisoned, which would mean a
    /// previous operation panicked while holding the lock.
    pub fn open_spill(&self) -> Result<ToolOutputSpill, ManagedOutputError> {
        #[cfg(test)]
        let fail_writes_after = self
            .state
            .lock()
            .expect("managed tool-output allocation lock poisoned")
            .fail_writes_after;
        loop {
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
            match File::options().create_new(true).write(true).open(&path) {
                Ok(file) => {
                    return Ok(ToolOutputSpill {
                        file,
                        path,
                        #[cfg(test)]
                        fail_writes_after,
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    // The name is already owned by an older spill: the loop
                    // advances past it and never overwrites it.
                }
                Err(error) => {
                    return Err(ManagedOutputError::OpenFailed(format!(
                        "{}: {error}",
                        path.display()
                    )));
                }
            }
        }
    }
}

/// The high-water mark of the existing `output_N.log` spill names in one
/// canonical managed root, so a reconstructed store continues the sequence
/// monotonically instead of colliding with spills of an earlier runtime
/// lifetime.
fn spill_high_water(root: &Path) -> Result<u64, ManagedOutputError> {
    let mut high = 0u64;
    let entries = std::fs::read_dir(root).map_err(|error| {
        ManagedOutputError::RootUnavailable(format!("{}: {error}", root.display()))
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            ManagedOutputError::RootUnavailable(format!("{}: {error}", root.display()))
        })?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(number) = name
            .strip_prefix("output_")
            .and_then(|rest| rest.strip_suffix(".log"))
        else {
            continue;
        };
        if let Ok(number) = number.parse::<u64>() {
            high = high.max(number);
        }
    }
    Ok(high)
}

/// One open spill file: the complete output of one textual capture streams
/// through it, and its absolute path is the model-facing locator.
#[derive(Debug)]
pub struct ToolOutputSpill {
    file: File,
    path: PathBuf,
    /// Test-only write-failure allowance: once exhausted, every further
    /// write fails. Never set outside `#[cfg(test)]`.
    #[cfg(test)]
    fail_writes_after: Option<u64>,
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
        #[cfg(test)]
        if let Some(remaining) = &mut self.fail_writes_after {
            let len = bytes.len() as u64;
            if len > *remaining {
                return Err(std::io::Error::other("test-forced spill write failure"));
            }
            *remaining -= len;
        }
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

    /// Restart safety: reconstructing a store over a retained managed root
    /// never collides with or overwrites the spills of the earlier runtime
    /// lifetime; the new allocation succeeds with a distinct path and its
    /// own complete bytes.
    #[test]
    fn reconstruction_over_a_retained_root_never_collides_or_overwrites() {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path().join("tool-output");
        let first_store =
            ManagedToolOutput::new(ConversationId::new("conv-1"), &root).expect("first store");
        let mut first = first_store.open_spill().expect("first spill");
        first.write_all(b"first complete bytes").expect("write");
        let first_path = first.path().to_path_buf();
        drop(first);
        drop(first_store);

        // A fresh store over the SAME directory: the in-memory sequence
        // restarts, but allocation must not fail or overwrite the old file.
        let second_store =
            ManagedToolOutput::new(ConversationId::new("conv-1"), &root).expect("second store");
        let mut second = second_store.open_spill().expect("second spill");
        second.write_all(b"second complete bytes").expect("write");
        let second_path = second.path().to_path_buf();
        drop(second);

        assert_ne!(first_path, second_path, "spill paths are distinct");
        assert_eq!(
            std::fs::read(&first_path).expect("first spill"),
            b"first complete bytes",
            "the old spill was not overwritten"
        );
        assert_eq!(
            std::fs::read(&second_path).expect("second spill"),
            b"second complete bytes",
            "the new spill holds its own complete bytes"
        );
    }

    /// Two stores momentarily sharing one root can never truncate each
    /// other's spill: a name collision advances the sequence instead.
    #[test]
    fn concurrent_stores_sharing_one_root_never_overwrite() {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path().join("tool-output");
        let first =
            ManagedToolOutput::new(ConversationId::new("conv-1"), &root).expect("first store");
        let second =
            ManagedToolOutput::new(ConversationId::new("conv-1"), &root).expect("second store");
        let mut a = first.open_spill().expect("spill a");
        a.write_all(b"a").expect("write a");
        // `second` was constructed before `output_1.log` existed, so its
        // high-water mark is stale; the collision must advance, never
        // truncate.
        let mut b = second.open_spill().expect("spill b");
        b.write_all(b"b").expect("write b");
        assert_ne!(a.path(), b.path());
        assert_eq!(std::fs::read(a.path()).expect("read a"), b"a");
        assert_eq!(std::fs::read(b.path()).expect("read b"), b"b");
    }

    /// A pre-existing symlink at the managed-output root is rejected: the
    /// dedicated root must be a real owned directory, never an alias of
    /// another filesystem region.
    #[cfg(unix)]
    #[test]
    fn a_symlink_root_is_rejected() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().expect("temp dir");
        let target = dir.path().join("target");
        std::fs::create_dir_all(&target).expect("target");
        let link = dir.path().join("tool-output");
        symlink(&target, &link).expect("symlink");
        let error = ManagedToolOutput::new(ConversationId::new("conv-1"), &link)
            .expect_err("a symlinked managed root is rejected");
        assert!(
            matches!(error, ManagedOutputError::SymlinkRoot(_)),
            "got {error:?}"
        );
    }

    /// The test-only write-failure seam fails writes after the allowance
    /// without failing the open itself.
    #[test]
    fn the_write_failure_seam_fails_writes_after_allocation() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = ManagedToolOutput::new(
            ConversationId::new("conv-1"),
            dir.path().join("tool-output"),
        )
        .expect("store");
        store.fail_spill_writes_after(4);
        let mut spill = store.open_spill().expect("the open itself succeeds");
        spill.write_all(b"abcd").expect("within the allowance");
        assert!(
            spill.write_all(b"e").is_err(),
            "a write past the allowance fails"
        );
        assert!(
            spill.write_all(b"f").is_err(),
            "every later write keeps failing"
        );
    }
}
