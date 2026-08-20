//! The conversation-owned managed tool-output store.
//!
//! Two distinct output-storage concepts live here. They share the
//! authorized root and the low-level collision-safe file allocation, but
//! they have different lifecycles and must not be conflated:
//!
//! ```text
//! managed tool-output root
//!     |
//!     +-- results/result_N.txt   ResultSpill: foreground/settled result
//!     |                          overflow storage. Allocated lazily, only
//!     |                          when a textual result crosses its
//!     |                          model-facing bound. Small output never
//!     |                          touches the filesystem.
//!     |
//!     +-- tasks/exec_N.output    BackgroundOutput: the live output channel
//!                                of one accepted background execution.
//!                                Allocated at the background dispatch
//!                                commit point, before the accepted result
//!                                may advertise it, regardless of how much
//!                                output the execution eventually produces.
//! ```
//!
//! Both are auxiliary runtime-owned storage: a bounded model-visible
//! preview/message is the canonical replayable record, and the file holds
//! the complete textual output addressed by its absolute path inside
//! ordinary textual tool output. Neither is a semantic artifact, a second
//! canonical history, or a model `File` modality. The model may explicitly
//! Read or Grep an advertised path while the file exists.
//!
//! Every advertised path contains valid UTF-8 text: producers decode each
//! byte stream with an incremental UTF-8 decoder before writing (invalid
//! sequences become U+FFFD), so the Read/Grep continuation guidance is
//! always honest.
//!
//! # Ownership boundary
//!
//! The managed-output root is deliberately **not** the artifact store and
//! **not** the enclosing runtime-private directory: the runtime-private
//! region also holds the durable conversation database and semantic
//! artifact internals, which must never become model-readable merely
//! because textual output files are. The filesystem locator authority
//! ([`crate::tools::locator`]) authorizes exactly this root for read-only
//! operations (Read/Grep/Glob) and rejects Write/Edit against it.
//!
//! # Allocation
//!
//! Result spills are allocated lazily under one monotonic sequence
//! (`result_1.txt`, `result_2.txt`, ...). Background output files are
//! allocated eagerly at dispatch under the execution identity itself
//! (`exec_N.output`), because the execution id is already the unique,
//! monotonic, restart-reseeded identity of the execution. All allocations
//! open with `create_new` and never overwrite an existing file.
//!
//! The result-spill sequence is **restart safe** without being durable
//! semantic state: construction seeds the process-local high-water mark
//! from the existing `result_N.txt` names, so a reconstructed runtime over
//! a retained storage root continues monotonically. Background execution
//! ids are reseeded above every durably committed ordinal at startup
//! recovery (Issue #12, M9a), so a new execution never reuses the identity
//! — and therefore never the output path — of a durably owned execution.
//! The one collision a background allocation can still observe is the
//! residue of a pre-commit crash (the file was allocated but the durable
//! ownership fact never committed, so the execution never existed); that
//! stale file is replaced explicitly. The sequences themselves are
//! auxiliary storage identity, not conversation state; nothing about them
//! is persisted.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::runtime::identity::{ConversationId, ToolExecutionId};

/// A managed tool-output failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManagedOutputError {
    /// The managed-output root cannot be created or canonicalized.
    RootUnavailable(String),
    /// The managed-output root (or one of its dedicated subdirectories)
    /// already exists as a symlink. The dedicated root must be a real owned
    /// directory, never an alias of another region (the workspace, the
    /// artifact root, or an arbitrary host directory), because its
    /// canonical path is an authorized read root.
    SymlinkRoot(String),
    /// The result-spill sequence space is exhausted.
    SequenceExhausted,
    /// An output file cannot be opened.
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
                "the managed tool-output root must be real directories, not symlinks: {message}"
            ),
            Self::SequenceExhausted => {
                write!(f, "the managed tool-output sequence space is exhausted")
            }
            Self::OpenFailed(message) => write!(f, "cannot open an output file: {message}"),
        }
    }
}

impl std::error::Error for ManagedOutputError {}

/// The dedicated subdirectory of foreground/settled result spills.
const RESULTS_DIR: &str = "results";
/// The dedicated subdirectory of background execution live output.
const TASKS_DIR: &str = "tasks";

/// The synchronized allocation state of one managed tool-output store.
#[derive(Debug)]
struct ManagedOutputState {
    next: u64,
    /// Test-only seam: when set, every output allocation/open fails, so
    /// tests can prove producers represent output failure explicitly.
    /// Never set outside `#[cfg(test)]`.
    #[cfg(test)]
    force_open_failures: bool,
    /// Test-only seam: when set, output writes fail after this many bytes,
    /// so tests can prove a partial output file is never advertised as
    /// complete. Never set outside `#[cfg(test)]`.
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
    /// The root and its two dedicated subdirectories (`results/` for
    /// result spills, `tasks/` for background execution output) are created
    /// when missing, and the root is canonicalized once, so the locator
    /// authority compares every managed-output locator against one
    /// canonical root. A pre-existing symlink at the root or at either
    /// dedicated subdirectory is rejected: the managed region must be real
    /// owned directories, never aliases of another filesystem region,
    /// because the canonical root becomes an authorized model-readable
    /// root and the runtime itself appends through these paths.
    ///
    /// The result-spill sequence is seeded from the existing
    /// `result_N.txt` names in `results/`, so reconstructing a store over
    /// a retained storage root continues monotonically instead of
    /// colliding with older spill files.
    ///
    /// # Errors
    ///
    /// Returns [`ManagedOutputError::SymlinkRoot`] when the root or a
    /// dedicated subdirectory already exists as a symlink and
    /// [`ManagedOutputError::RootUnavailable`] when the root cannot be
    /// created, read, or canonicalized.
    pub fn new(
        conversation_id: ConversationId,
        root: impl AsRef<Path>,
    ) -> Result<Self, ManagedOutputError> {
        let root = root.as_ref();
        reject_symlink(root)?;
        std::fs::create_dir_all(root).map_err(|error| {
            ManagedOutputError::RootUnavailable(format!("{}: {error}", root.display()))
        })?;
        let canonical = std::fs::canonicalize(root).map_err(|error| {
            ManagedOutputError::RootUnavailable(format!("{}: {error}", root.display()))
        })?;
        for dedicated in [RESULTS_DIR, TASKS_DIR] {
            let subdirectory = canonical.join(dedicated);
            reject_symlink(&subdirectory)?;
            std::fs::create_dir_all(&subdirectory).map_err(|error| {
                ManagedOutputError::RootUnavailable(format!("{}: {error}", subdirectory.display()))
            })?;
        }
        let next = spill_high_water(&canonical.join(RESULTS_DIR))?;
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

    /// Test-only seam: forces every subsequent output allocation/open to
    /// fail, so tests can prove that neither a capture nor a background
    /// dispatch reports successful retention while silently losing output.
    /// Only available under `#[cfg(test)]`.
    #[cfg(test)]
    pub(crate) fn set_force_open_failures(&self, enabled: bool) {
        self.state
            .lock()
            .expect("managed tool-output lock poisoned")
            .force_open_failures = enabled;
    }

    /// Test-only seam: output writes fail after `bytes` successfully
    /// written bytes, so tests can fail a write *after* allocation and
    /// prove a partial file is never advertised as complete. Applies to
    /// result spills and background output sinks opened after the call.
    /// Only available under `#[cfg(test)]`.
    #[cfg(test)]
    pub(crate) fn fail_writes_after(&self, bytes: u64) {
        self.state
            .lock()
            .expect("managed tool-output lock poisoned")
            .fail_writes_after = Some(bytes);
    }

    /// Test-only seam: exhausts the result-spill sequence so the next
    /// allocation fails explicitly. Only available under `#[cfg(test)]`.
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

    /// Allocates and opens one result spill for streaming complete output.
    ///
    /// A result spill is allocated lazily, only when a textual result has
    /// crossed its model-facing bound. Allocation never overwrites an
    /// existing spill: the file is opened with `create_new`, and when the
    /// name already exists — stale spill of an earlier runtime lifetime
    /// over this retained root, or a second store momentarily sharing the
    /// root — the sequence advances to the next name instead of failing or
    /// truncating. The high-water seeding at construction makes the common
    /// restart case collision-free.
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
    pub fn open_spill(&self) -> Result<ResultSpill, ManagedOutputError> {
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
                        "test-forced output open failure".to_owned(),
                    ));
                }
                state.next = next;
                next
            };
            let path = self
                .root
                .join(RESULTS_DIR)
                .join(format!("result_{sequence}.txt"));
            match File::options().create_new(true).write(true).open(&path) {
                Ok(file) => {
                    return Ok(ResultSpill {
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

    /// The canonical absolute live-output locator of one background
    /// execution, allocated or not.
    ///
    /// The path derives deterministically from the execution identity: the
    /// execution id is already the unique monotonic identity of the
    /// execution, so the output path needs no separate sequence.
    #[must_use]
    pub fn background_output_path(&self, execution_id: &ToolExecutionId) -> PathBuf {
        self.root
            .join(TASKS_DIR)
            .join(format!("{}.output", execution_id.as_str()))
    }

    /// Allocates the live-output file of one background execution.
    ///
    /// This is part of the background dispatch linearization point: the
    /// file is created (empty) before the dispatch commits, so the accepted
    /// result may advertise the locator immediately and the executor may
    /// append from byte zero. Allocation uses `create_new` and never
    /// overwrites.
    ///
    /// The only possible name collision is the residue of a pre-commit
    /// crash of an earlier runtime lifetime: the file was allocated but the
    /// durable ownership fact never committed, so that execution never
    /// existed durably and its allocated path is legitimately reclaimed.
    /// A durably owned execution can never collide, because startup
    /// recovery reseeds the execution sequence above every durable
    /// ordinal.
    ///
    /// # Errors
    ///
    /// Returns [`ManagedOutputError::OpenFailed`] when the file cannot be
    /// created. A failed allocation must abort the dispatch: an accepted
    /// background execution with an invalid locator must never exist.
    ///
    /// # Panics
    ///
    /// Panics only if the allocation lock is poisoned (the test-only failure
    /// seam), which would mean a previous operation panicked while holding
    /// the lock.
    pub fn allocate_background_output(
        &self,
        execution_id: &ToolExecutionId,
    ) -> Result<PathBuf, ManagedOutputError> {
        #[cfg(test)]
        if self
            .state
            .lock()
            .expect("managed tool-output allocation lock poisoned")
            .force_open_failures
        {
            return Err(ManagedOutputError::OpenFailed(
                "test-forced output open failure".to_owned(),
            ));
        }
        let path = self.background_output_path(execution_id);
        for attempt in 0..2u32 {
            match File::options().create_new(true).write(true).open(&path) {
                Ok(_file) => return Ok(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists && attempt == 0 => {
                    // Pre-commit crash residue of an execution that never
                    // committed durably (see the method documentation):
                    // reclaim the path explicitly instead of appending to
                    // or keeping a stale file.
                    std::fs::remove_file(&path).map_err(|error| {
                        ManagedOutputError::OpenFailed(format!("{}: {error}", path.display()))
                    })?;
                }
                Err(error) => {
                    return Err(ManagedOutputError::OpenFailed(format!(
                        "{}: {error}",
                        path.display()
                    )));
                }
            }
        }
        Err(ManagedOutputError::OpenFailed(format!(
            "{}: cannot reclaim the stale output file",
            path.display()
        )))
    }

    /// Discards the live-output file of a background dispatch that rolled
    /// back before commit. Best-effort: a failed pre-commit dispatch
    /// leaves no orphan file behind.
    pub(crate) fn discard_background_output(&self, execution_id: &ToolExecutionId) {
        let _ = std::fs::remove_file(self.background_output_path(execution_id));
    }

    /// Opens the append sink of the live-output file of one accepted
    /// background execution.
    ///
    /// The file was allocated at the dispatch commit point
    /// ([`ManagedToolOutput::allocate_background_output`]); the executor
    /// appends decoded textual output fragments through the returned sink,
    /// and every successful append is immediately observable to a
    /// concurrent reader (the handle is unbuffered). A missing file is an
    /// explicit failure, never a silently created second allocation: the
    /// dispatch commit owns allocation.
    ///
    /// # Errors
    ///
    /// Returns the underlying I/O error when the file cannot be opened.
    ///
    /// # Panics
    ///
    /// Panics only if the allocation lock is poisoned (the test-only failure
    /// seam), which would mean a previous operation panicked while holding
    /// the lock.
    pub fn open_background_output_sink(
        &self,
        execution_id: &ToolExecutionId,
    ) -> std::io::Result<BackgroundOutput> {
        let path = self.background_output_path(execution_id);
        let file = File::options().append(true).open(&path)?;
        #[cfg(test)]
        let fail_writes_after = self
            .state
            .lock()
            .expect("managed tool-output allocation lock poisoned")
            .fail_writes_after;
        Ok(BackgroundOutput {
            file,
            path,
            #[cfg(test)]
            fail_writes_after,
        })
    }
}

/// Rejects a pre-existing symlink at `path`: the managed region must be
/// real owned directories, never aliases of another filesystem region.
fn reject_symlink(path: &Path) -> Result<(), ManagedOutputError> {
    if let Ok(metadata) = std::fs::symlink_metadata(path)
        && metadata.file_type().is_symlink()
    {
        return Err(ManagedOutputError::SymlinkRoot(path.display().to_string()));
    }
    Ok(())
}

/// The high-water mark of the existing `result_N.txt` spill names in one
/// canonical results directory, so a reconstructed store continues the
/// sequence monotonically instead of colliding with spills of an earlier
/// runtime lifetime.
fn spill_high_water(results: &Path) -> Result<u64, ManagedOutputError> {
    let mut high = 0u64;
    let entries = std::fs::read_dir(results).map_err(|error| {
        ManagedOutputError::RootUnavailable(format!("{}: {error}", results.display()))
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            ManagedOutputError::RootUnavailable(format!("{}: {error}", results.display()))
        })?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(number) = name
            .strip_prefix("result_")
            .and_then(|rest| rest.strip_suffix(".txt"))
        else {
            continue;
        };
        if let Ok(number) = number.parse::<u64>() {
            high = high.max(number);
        }
    }
    Ok(high)
}

/// One open result spill: the complete textual content of one oversized
/// tool result streams through it, and its absolute path is the
/// model-facing continuation locator.
///
/// A result spill has exactly one terminal storage state: either it is
/// published complete (the producer finished every write successfully and
/// advertised the locator), or it is incomplete and is cleaned up without
/// ever being advertised.
#[derive(Debug)]
pub struct ResultSpill {
    file: File,
    path: PathBuf,
    /// Test-only write-failure allowance: once exhausted, every further
    /// write fails. Never set outside `#[cfg(test)]`.
    #[cfg(test)]
    fail_writes_after: Option<u64>,
}

impl ResultSpill {
    /// The canonical absolute model-facing locator of this spill file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Streams one decoded text fragment into the spill file.
    ///
    /// # Errors
    ///
    /// Returns the underlying I/O error; a failed spill write is an explicit
    /// capture failure, never silently lost output.
    pub fn write_all(&mut self, text: &str) -> std::io::Result<()> {
        write_text(
            &mut self.file,
            text,
            #[cfg(test)]
            &mut self.fail_writes_after,
        )
    }
}

/// The append sink of one background execution's live-output file.
///
/// Unlike a [`ResultSpill`], this file is advertised to the model from the
/// dispatch commit point on, while the execution is still running: every
/// successful append is the linearization point after which the fragment
/// is observable through Read/Grep. A sink failure after advertisement can
/// never be hidden by unpublishing the path; the producer must represent
/// it explicitly as incomplete output at settlement.
#[derive(Debug)]
pub struct BackgroundOutput {
    file: File,
    path: PathBuf,
    /// Test-only write-failure allowance: once exhausted, every further
    /// write fails. Never set outside `#[cfg(test)]`.
    #[cfg(test)]
    fail_writes_after: Option<u64>,
}

impl BackgroundOutput {
    /// The canonical absolute model-facing locator of this output file:
    /// the same path the dispatch advertised.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Appends one decoded text fragment to the live output. A successful
    /// return is the append linearization point: the fragment is
    /// subsequently observable through Read/Grep while the execution runs.
    ///
    /// # Errors
    ///
    /// Returns the underlying I/O error; a failed append is an explicit
    /// output-storage failure of the execution, never silently lost output.
    pub fn append(&mut self, text: &str) -> std::io::Result<()> {
        write_text(
            &mut self.file,
            text,
            #[cfg(test)]
            &mut self.fail_writes_after,
        )
    }
}

/// Writes one text fragment, honoring the test-only write-failure
/// allowance when compiled in.
fn write_text(
    file: &mut File,
    text: &str,
    #[cfg(test)] fail_writes_after: &mut Option<u64>,
) -> std::io::Result<()> {
    #[cfg(test)]
    if let Some(remaining) = fail_writes_after {
        let len = text.len() as u64;
        if len > *remaining {
            return Err(std::io::Error::other("test-forced output write failure"));
        }
        *remaining -= len;
    }
    file.write_all(text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::{ManagedOutputError, ManagedToolOutput};
    use crate::runtime::identity::{ConversationId, ToolExecutionId};

    fn store(root: &std::path::Path) -> ManagedToolOutput {
        ManagedToolOutput::new(ConversationId::new("conv-1"), root.join("tool-output"))
            .expect("store")
    }

    #[test]
    fn spill_allocation_is_monotonic_collision_safe_and_absolute() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = store(dir.path());
        let first = store.open_spill().expect("first");
        let second = store.open_spill().expect("second");
        assert!(first.path().is_absolute());
        assert!(first.path().ends_with("results/result_1.txt"));
        assert!(second.path().ends_with("results/result_2.txt"));
        assert!(
            first.path().starts_with(store.root()),
            "spill lives under the managed root"
        );
        assert_eq!(std::fs::read(first.path()).expect("read"), b"");
    }

    #[test]
    fn sequence_exhaustion_fails_explicitly() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = store(dir.path());
        store.exhaust_sequence();
        assert_eq!(
            store.open_spill().expect_err("exhausted"),
            ManagedOutputError::SequenceExhausted
        );
    }

    #[test]
    fn spilled_text_is_retained_verbatim() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = store(dir.path());
        let mut spill = store.open_spill().expect("open");
        spill.write_all("hello\n").expect("write");
        spill.write_all("emoji: 😀\n").expect("write");
        drop(spill);
        let text = std::fs::read_to_string(store.root().join("results/result_1.txt"))
            .expect("the spill is valid UTF-8 text");
        assert_eq!(text, "hello\nemoji: 😀\n");
    }

    /// Restart safety: reconstructing a store over a retained managed root
    /// never collides with or overwrites the spills of the earlier runtime
    /// lifetime; the new allocation succeeds with a distinct path and its
    /// own complete content.
    #[test]
    fn reconstruction_over_a_retained_root_never_collides_or_overwrites() {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path().join("tool-output");
        let first_store =
            ManagedToolOutput::new(ConversationId::new("conv-1"), &root).expect("first store");
        let mut first = first_store.open_spill().expect("first spill");
        first.write_all("first complete text").expect("write");
        let first_path = first.path().to_path_buf();
        drop(first);
        drop(first_store);

        // A fresh store over the SAME directory: the in-memory sequence
        // restarts, but allocation must not fail or overwrite the old file.
        let second_store =
            ManagedToolOutput::new(ConversationId::new("conv-1"), &root).expect("second store");
        let mut second = second_store.open_spill().expect("second spill");
        second.write_all("second complete text").expect("write");
        let second_path = second.path().to_path_buf();
        drop(second);

        assert_ne!(first_path, second_path, "spill paths are distinct");
        assert_eq!(
            std::fs::read_to_string(&first_path).expect("first spill"),
            "first complete text",
            "the old spill was not overwritten"
        );
        assert_eq!(
            std::fs::read_to_string(&second_path).expect("second spill"),
            "second complete text",
            "the new spill holds its own complete text"
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
        a.write_all("a").expect("write a");
        // `second` was constructed before `result_1.txt` existed, so its
        // high-water mark is stale; the collision must advance, never
        // truncate.
        let mut b = second.open_spill().expect("spill b");
        b.write_all("b").expect("write b");
        assert_ne!(a.path(), b.path());
        assert_eq!(std::fs::read_to_string(a.path()).expect("read a"), "a");
        assert_eq!(std::fs::read_to_string(b.path()).expect("read b"), "b");
    }

    /// A pre-existing symlink at the managed-output root or at one of its
    /// dedicated subdirectories is rejected: the managed region must be
    /// real owned directories, never aliases of another filesystem region.
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

    /// A pre-existing symlink at a dedicated subdirectory is rejected with
    /// the same authority rationale as a symlinked root.
    #[cfg(unix)]
    #[test]
    fn a_symlink_subdirectory_is_rejected() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path().join("tool-output");
        std::fs::create_dir_all(&root).expect("root");
        let target = dir.path().join("elsewhere");
        std::fs::create_dir_all(&target).expect("target");
        symlink(&target, root.join("tasks")).expect("symlink");
        let error = ManagedToolOutput::new(ConversationId::new("conv-1"), &root)
            .expect_err("a symlinked tasks directory is rejected");
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
        let store = store(dir.path());
        store.fail_writes_after(4);
        let mut spill = store.open_spill().expect("the open itself succeeds");
        spill.write_all("abcd").expect("within the allowance");
        assert!(
            spill.write_all("e").is_err(),
            "a write past the allowance fails"
        );
        assert!(
            spill.write_all("f").is_err(),
            "every later write keeps failing"
        );
    }

    /// The background live-output file is allocated by execution identity,
    /// is observable empty from allocation on, and appends are immediately
    /// readable through the path.
    #[test]
    fn background_output_is_allocated_by_execution_identity_and_appends() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = store(dir.path());
        let execution_id = ToolExecutionId::background(12);
        let path = store
            .allocate_background_output(&execution_id)
            .expect("allocate");
        assert!(path.is_absolute());
        assert!(path.ends_with("tasks/exec_12.output"));
        assert!(path.starts_with(store.root()));
        assert_eq!(std::fs::read(&path).expect("read"), b"", "starts empty");
        // The pure path computation agrees with the allocation.
        assert_eq!(path, store.background_output_path(&execution_id));

        let mut sink = store
            .open_background_output_sink(&execution_id)
            .expect("append sink");
        sink.append("line A\n").expect("append A");
        assert_eq!(
            std::fs::read_to_string(&path).expect("read while running"),
            "line A\n",
            "a committed append is observable through the path"
        );
        sink.append("line B\n").expect("append B");
        drop(sink);
        assert_eq!(
            std::fs::read_to_string(&path).expect("read after settlement"),
            "line A\nline B\n"
        );
    }

    /// A retained background output file of a durably owned execution is
    /// never overwritten: a new execution has a distinct identity and
    /// therefore a distinct path.
    #[test]
    fn a_new_execution_never_overwrites_a_retained_background_output() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = store(dir.path());
        let first_id = ToolExecutionId::background(12);
        let first_path = store.allocate_background_output(&first_id).expect("first");
        store
            .open_background_output_sink(&first_id)
            .expect("sink")
            .append("retained output")
            .expect("append");

        // Startup recovery reseeds the execution sequence above the durable
        // ordinal, so the next execution is exec_13, never exec_12 again.
        let second_id = ToolExecutionId::background(13);
        let second_path = store
            .allocate_background_output(&second_id)
            .expect("second");
        assert_ne!(first_path, second_path);
        assert_eq!(
            std::fs::read_to_string(&first_path).expect("first"),
            "retained output"
        );
        assert_eq!(std::fs::read(&second_path).expect("second"), b"");
    }

    /// The one legitimate collision — pre-commit crash residue of an
    /// execution that never committed durably — is reclaimed explicitly;
    /// allocation never fails over it and never appends to it.
    #[test]
    fn pre_commit_crash_residue_is_reclaimed_not_reused() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = store(dir.path());
        let execution_id = ToolExecutionId::background(7);
        let path = store
            .allocate_background_output(&execution_id)
            .expect("first allocation");
        // Simulate a crash after allocation but before any durable commit:
        // the sequence restarts below the durable watermark and the same
        // execution id is minted again.
        std::fs::write(&path, "stale residue").expect("stale");
        let reclaimed = store
            .allocate_background_output(&execution_id)
            .expect("the stale residue is reclaimed");
        assert_eq!(reclaimed, path);
        assert_eq!(
            std::fs::read(&reclaimed).expect("read"),
            b"",
            "the reclaimed file is empty, never appended to stale bytes"
        );
    }

    /// A rollback discard removes the allocated file best-effort, so a
    /// failed pre-commit dispatch leaves no orphan behind.
    #[test]
    fn discard_removes_a_rolled_back_background_output() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = store(dir.path());
        let execution_id = ToolExecutionId::background(3);
        let path = store
            .allocate_background_output(&execution_id)
            .expect("allocate");
        assert!(path.exists());
        store.discard_background_output(&execution_id);
        assert!(!path.exists(), "the rolled-back output file is removed");
        // Discarding an unknown execution is a no-op.
        store.discard_background_output(&ToolExecutionId::background(99));
    }

    /// The forced-open-failure seam fails background allocation explicitly.
    #[test]
    fn the_open_failure_seam_fails_background_allocation() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = store(dir.path());
        store.set_force_open_failures(true);
        let error = store
            .allocate_background_output(&ToolExecutionId::background(1))
            .expect_err("forced allocation failure");
        assert!(matches!(error, ManagedOutputError::OpenFailed(_)));
        assert!(
            !store
                .background_output_path(&ToolExecutionId::background(1))
                .exists(),
            "a failed allocation leaves no file"
        );
    }
}
