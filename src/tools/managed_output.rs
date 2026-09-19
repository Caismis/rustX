//! The conversation-owned managed tool-output store.
//!
//! Two distinct output-storage concepts live here. They share the
//! authorized root and the low-level collision-safe file allocation, but
//! they have different lifecycles and must not be conflated:
//!
//! ```text
//! managed tool-output root
//!     |
//!     +-- results/result_<uuid-v7>.txt   ResultSpill: foreground result overflow
//!     |                          storage. Allocated lazily, only when a
//!     |                          textual result crosses the shared preview
//!     |                          threshold. Small output never touches the
//!     |                          filesystem.
//!     |
//!     +-- tasks/exec_<uuid-v7>.output    BackgroundOutput: the live output channel
//!                                of one accepted background execution.
//!                                Allocated at the background dispatch
//!                                commit point, before the accepted result
//!                                may advertise it, regardless of how much
//!                                output the execution eventually produces.
//! ```
//!
//! Both are auxiliary runtime-owned storage: a bounded model-visible
//! preview/message is the canonical replayable record, and the file holds
//! the complete textual output (or an honestly partial prefix after a
//! storage failure) addressed by its absolute path inside ordinary textual
//! tool output. Neither is a semantic artifact, a second canonical history,
//! or a model `File` modality. The model may explicitly Read or Grep an
//! advertised path while the file exists.
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
//! because textual output files are. The runtime locator
//! ([`crate::tools::locator`]) resolves advertised paths for read-only
//! operations (Read/Grep/Glob), while this type owns the separate invariant
//! that model-originated Write/Edit mutations are rejected for this root.
//!
//! # Allocation
//!
//! Foreground spills use fresh `UUIDv7` names and bounded collision retries.
//! Background output uses the execution identity without replacing collisions.
//! Every allocation uses `create_new`; reconstruction never scans filenames.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
#[cfg(test)]
use std::sync::Mutex;

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
    /// The bounded identity reservation attempts all collided.
    IdentityCollision,
    /// An output file cannot be opened.
    OpenFailed(String),
    /// A model-originated mutation attempted to target the managed-output
    /// namespace, which is runtime-owned read-only storage.
    ModelMutationReadOnly(String),
}

impl ManagedOutputError {
    /// Safe semantic diagnostic at the runtime-owned storage boundary.
    /// The detailed error retains private locators for runtime diagnostics only.
    pub(crate) const fn storage_diagnostic(&self) -> &'static str {
        match self {
            Self::IdentityCollision => "managed output storage identity reservation exhausted",
            Self::RootUnavailable(_)
            | Self::SymlinkRoot(_)
            | Self::OpenFailed(_)
            | Self::ModelMutationReadOnly(_) => "managed output storage is unavailable",
        }
    }
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
            Self::IdentityCollision => {
                write!(f, "managed tool-output identity reservation exhausted")
            }
            Self::OpenFailed(message) => write!(f, "cannot open an output file: {message}"),
            Self::ModelMutationReadOnly(path) => write!(
                f,
                "filesystem path {path:?} is inside the managed tool-output root, which is read-only auxiliary storage; Write/Edit never mutate it"
            ),
        }
    }
}

impl std::error::Error for ManagedOutputError {}

/// The dedicated subdirectory of foreground/settled result spills.
const RESULTS_DIR: &str = "results";
/// The dedicated subdirectory of background execution live output.
const TASKS_DIR: &str = "tasks";

/// The synchronized allocation state of one managed tool-output store.
#[cfg(test)]
#[derive(Debug)]
struct ManagedOutputState {
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
    lifecycle: Option<Arc<crate::runtime::local_storage::ConversationAccess>>,
    conversation_id: ConversationId,
    root: PathBuf,
    #[cfg(test)]
    state: Arc<Mutex<ManagedOutputState>>,
    identities: Arc<dyn crate::runtime::identity::UuidV7Generator>,
}

impl ManagedToolOutput {
    pub(crate) fn with_lifecycle(
        mut self,
        access: Option<Arc<crate::runtime::local_storage::ConversationAccess>>,
    ) -> Self {
        self.lifecycle = access;
        self
    }

    /// Creates the managed tool-output store rooted at `root`.
    ///
    /// The root and its two dedicated subdirectories (`results/` for
    /// result spills, `tasks/` for background execution output) are created
    /// when missing, and the root is canonicalized once, so runtime read
    /// locators and the model-mutation guard compare every managed-output
    /// path against one canonical root. A pre-existing symlink at the root or at either
    /// dedicated subdirectory is rejected: the managed region must be real
    /// owned directories, never aliases of another filesystem region,
    /// because the canonical root becomes an authorized model-readable
    /// root and the runtime itself appends through these paths.
    ///
    /// Reconstruction retains existing files and never scans names to derive identity.
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
        Ok(Self {
            lifecycle: None,
            identities: Arc::new(crate::runtime::identity::SystemUuidV7Generator),
            conversation_id,
            root: canonical,
            #[cfg(test)]
            state: Arc::new(Mutex::new(ManagedOutputState {
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

    /// Inject identity allocation to exercise collision paths deterministically.
    #[must_use]
    pub fn with_identity_generator(
        mut self,
        identities: Arc<dyn crate::runtime::identity::UuidV7Generator>,
    ) -> Self {
        self.identities = identities;
        self
    }

    /// Allocate an execution identity using the same injectable identity authority.
    /// The caller must reserve its output path before accepting the execution.
    /// # Errors
    /// Rejects an injected identity that is not a valid `UUIDv7`.
    pub fn execution_identity(&self) -> Result<ToolExecutionId, ManagedOutputError> {
        ToolExecutionId::from_uuid(self.identities.next_uuid())
            .map_err(ManagedOutputError::OpenFailed)
    }

    /// The conversation this store belongs to.
    #[must_use]
    pub fn conversation_id(&self) -> &ConversationId {
        &self.conversation_id
    }

    /// The canonical managed tool-output root.
    ///
    /// This root — and nothing outside it, in particular not the enclosing
    /// runtime-private directory — is the read-only filesystem region exposed
    /// to model Read/Grep/Glob. Write/Edit mutation ownership is enforced by
    /// [`Self::ensure_model_mutation_allowed`].
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Rejects a model-originated mutation whose already-resolved effective
    /// target belongs to this runtime-owned namespace.
    ///
    /// Native file tools intentionally accept arbitrary cwd-relative and
    /// absolute host paths. This is the one narrow exception: the managed
    /// output root remains readable/searchable by Read/Grep/Glob, but Write/Edit
    /// may never mutate it. The caller must pass the effective target after
    /// filesystem symlinks and non-existent descendants have been resolved.
    pub(crate) fn ensure_model_mutation_allowed(
        &self,
        effective_target: &Path,
    ) -> Result<(), ManagedOutputError> {
        if effective_target.starts_with(&self.root) {
            return Err(ManagedOutputError::ModelMutationReadOnly(
                effective_target.display().to_string(),
            ));
        }
        Ok(())
    }

    /// Allocates and opens one result spill for streaming complete output.
    ///
    /// Allocation uses `UUIDv7` and `create_new`. A collision retries at most sixteen
    /// times, then fails without touching any existing file.
    ///
    /// # Errors
    /// Returns identity exhaustion or an I/O error without overwriting files.
    /// # Panics
    /// Test builds panic if the injected allocation-failure lock is poisoned.
    pub fn open_spill(&self) -> Result<ResultSpill, ManagedOutputError> {
        #[cfg(test)]
        let fail_writes_after = self
            .state
            .lock()
            .expect("managed tool-output allocation lock poisoned")
            .fail_writes_after;
        for _ in 0..16 {
            #[cfg(test)]
            if self
                .state
                .lock()
                .expect("managed output lock")
                .force_open_failures
            {
                return Err(ManagedOutputError::OpenFailed(
                    "test-forced output open failure".into(),
                ));
            }
            let identity = self.identities.next_uuid();
            if !crate::runtime::identity::is_uuid_v7(identity) {
                return Err(ManagedOutputError::OpenFailed(
                    "identity generator returned a non-UUIDv7 value".into(),
                ));
            }
            let path = self
                .root
                .join(RESULTS_DIR)
                .join(format!("result_{identity}.txt"));
            match File::options().create_new(true).write(true).open(&path) {
                Ok(file) => {
                    return Ok(ResultSpill {
                        _lifecycle: self.lifecycle.clone(),
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
        Err(ManagedOutputError::IdentityCollision)
    }

    /// The canonical absolute live-output locator of one background
    /// execution, allocated or not.
    ///
    /// The path derives deterministically from the execution identity: the
    /// execution identity is also the locator identity.
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
    /// An existing path always refuses allocation, including pre-commit residue.
    /// The owner may retry with a new identity before accepting any execution.
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
        File::options()
            .create_new(true)
            .write(true)
            .open(&path)
            .map_err(|error| {
                ManagedOutputError::OpenFailed(format!("{}: {error}", path.display()))
            })?;
        Ok(path)
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
        #[cfg(test)]
        if self
            .state
            .lock()
            .expect("managed tool-output allocation lock poisoned")
            .force_open_failures
        {
            return Err(std::io::Error::other("test-forced output open failure"));
        }
        let path = self.background_output_path(execution_id);
        let file = File::options().append(true).open(&path)?;
        #[cfg(test)]
        let fail_writes_after = self
            .state
            .lock()
            .expect("managed tool-output allocation lock poisoned")
            .fail_writes_after;
        Ok(BackgroundOutput {
            _lifecycle: self.lifecycle.clone(),
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

/// One open result spill: the complete textual content of one oversized
/// tool result streams through it, and its absolute path is the
/// model-facing continuation locator.
///
/// A result spill is published as
/// [`crate::tools::types::ManagedOutputContinuation::Complete`] only
/// after every write succeeds. If storage fails after allocation, the file is
/// retained as auxiliary partial output and the producer publishes a typed
/// [`crate::tools::types::ManagedOutputContinuation::Partial`] locator instead
/// of claiming that it
/// contains the complete result.
#[derive(Debug)]
pub struct ResultSpill {
    _lifecycle: Option<Arc<crate::runtime::local_storage::ConversationAccess>>,
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
    _lifecycle: Option<Arc<crate::runtime::local_storage::ConversationAccess>>,
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
        ManagedToolOutput::new(
            ConversationId::new("conv_01900000-0000-7000-8000-000000000001"),
            root.join("tool-output"),
        )
        .expect("store")
    }

    #[test]
    fn spill_allocation_has_distinct_uuid_v7_names_and_absolute_paths() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = store(dir.path());
        let first = store.open_spill().expect("first");
        let second = store.open_spill().expect("second");
        assert!(first.path().is_absolute());
        assert_ne!(first.path(), second.path());
        for path in [first.path(), second.path()] {
            assert_eq!(path.parent(), Some(store.root().join("results").as_path()));
            let name = path
                .file_stem()
                .unwrap()
                .to_str()
                .unwrap()
                .strip_prefix("result_")
                .unwrap();
            assert_eq!(uuid::Uuid::parse_str(name).unwrap().get_version_num(), 7);
        }
        assert!(
            first.path().starts_with(store.root()),
            "spill lives under the managed root"
        );
        assert_eq!(std::fs::read(first.path()).expect("read"), b"");
    }

    #[test]
    fn spilled_text_is_retained_verbatim() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = store(dir.path());
        let mut spill = store.open_spill().expect("open");
        spill.write_all("hello\n").expect("write");
        spill.write_all("emoji: 😀\n").expect("write");
        let path = spill.path().to_owned();
        drop(spill);
        let text = std::fs::read_to_string(path).expect("the spill is valid UTF-8 text");
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
        let first_store = ManagedToolOutput::new(
            ConversationId::new("conv_01900000-0000-7000-8000-000000000001"),
            &root,
        )
        .expect("first store");
        let mut first = first_store.open_spill().expect("first spill");
        first.write_all("first complete text").expect("write");
        let first_path = first.path().to_path_buf();
        drop(first);
        drop(first_store);

        // A fresh store over the SAME directory: the allocator is reconstructed, but allocation must not fail or overwrite the old file.
        let second_store = ManagedToolOutput::new(
            ConversationId::new("conv_01900000-0000-7000-8000-000000000001"),
            &root,
        )
        .expect("second store");
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
    /// other's spill: a collision retries a fresh UUID without overwrite.
    #[test]
    fn concurrent_stores_sharing_one_root_never_overwrite() {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path().join("tool-output");
        let first = ManagedToolOutput::new(
            ConversationId::new("conv_01900000-0000-7000-8000-000000000001"),
            &root,
        )
        .expect("first store");
        let second = ManagedToolOutput::new(
            ConversationId::new("conv_01900000-0000-7000-8000-000000000001"),
            &root,
        )
        .expect("second store");
        let mut a = first.open_spill().expect("spill a");
        a.write_all("a").expect("write a");
        // Concurrent owners use create-new publication, never truncation.
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
        let error = ManagedToolOutput::new(
            ConversationId::new("conv_01900000-0000-7000-8000-000000000001"),
            &link,
        )
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
        let error = ManagedToolOutput::new(
            ConversationId::new("conv_01900000-0000-7000-8000-000000000001"),
            &root,
        )
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
        let execution_id = ToolExecutionId::new("exec_01900000-0000-7000-8000-000000000012");
        let path = store
            .allocate_background_output(&execution_id)
            .expect("allocate");
        assert!(path.is_absolute());
        assert!(path.ends_with(format!("tasks/{execution_id}.output")));
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
        let first_id = ToolExecutionId::new("exec_01900000-0000-7000-8000-000000000012");
        let first_path = store.allocate_background_output(&first_id).expect("first");
        store
            .open_background_output_sink(&first_id)
            .expect("sink")
            .append("retained output")
            .expect("append");

        // Execution identities are independent of ordinal recovery.
        let second_id = ToolExecutionId::new("exec_01900000-0000-7000-8000-000000000013");
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

    /// Any existing path is retained, including uncommitted crash residue.
    #[test]
    fn pre_commit_crash_residue_is_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        let execution_id = store.execution_identity().unwrap();
        let path = store.allocate_background_output(&execution_id).unwrap();
        std::fs::write(&path, "stale residue").unwrap();
        assert!(store.allocate_background_output(&execution_id).is_err());
        assert_eq!(std::fs::read_to_string(path).unwrap(), "stale residue");
    }

    /// A rollback discard removes the allocated file best-effort, so a
    /// failed pre-commit dispatch leaves no orphan behind.
    #[test]
    fn discard_removes_a_rolled_back_background_output() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = store(dir.path());
        let execution_id = ToolExecutionId::new("exec_01900000-0000-7000-8000-000000000003");
        let path = store
            .allocate_background_output(&execution_id)
            .expect("allocate");
        assert!(path.exists());
        store.discard_background_output(&execution_id);
        assert!(!path.exists(), "the rolled-back output file is removed");
        // Discarding an unknown execution is a no-op.
        store.discard_background_output(&ToolExecutionId::new(
            "exec_01900000-0000-7000-8000-000000000099",
        ));
    }

    /// The forced-open-failure seam fails background allocation explicitly.
    #[test]
    fn the_open_failure_seam_fails_background_allocation() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = store(dir.path());
        store.set_force_open_failures(true);
        let error = store
            .allocate_background_output(&ToolExecutionId::new(
                "exec_01900000-0000-7000-8000-000000000001",
            ))
            .expect_err("forced allocation failure");
        assert!(matches!(error, ManagedOutputError::OpenFailed(_)));
        assert!(
            !store
                .background_output_path(&ToolExecutionId::new(
                    "exec_01900000-0000-7000-8000-000000000001"
                ))
                .exists(),
            "a failed allocation leaves no file"
        );
    }
}
