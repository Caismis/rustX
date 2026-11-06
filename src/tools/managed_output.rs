//! Conversation-owned disposable output, shared by Native and MCP producers.
//!
//! Foreground `r-<16 hex>.txt` spills allocate lazily at the preview threshold;
//! background `e-<16 hex>.output` files allocate before dispatch acceptance.
//! Both live in private `rx-<16 hex>` namespaces under the OS temporary root.
//! Published files survive runtime drop and normal exit, but the OS may remove
//! them. Committed previews and capture statuses remain canonical facts even
//! when their original absolute locators are no longer available.
//!
//! Local namespace bindings grant allocation, read and deletion ownership.
//! Inherited history grants none. Native filesystem tools retain their ordinary
//! host-path permissions; only managed roots are protected against Write/Edit.

pub(crate) mod storage;

use std::collections::BTreeMap;
use std::fs::File;
use std::io::Write;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::runtime::identity::{ConversationId, ToolExecutionId};

/// A managed tool-output failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManagedOutputError {
    /// Temporary output or its trusted ownership binding is unavailable.
    RootUnavailable(String),
    /// The bounded exclusive allocation attempts all collided.
    IdentityCollision,
    /// An output file cannot be opened.
    OpenFailed(String),
    /// Model-originated mutation of managed storage was refused.
    ModelMutationReadOnly(String),
}
impl core::fmt::Display for ManagedOutputError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::RootUnavailable(s) => write!(f, "managed tool-output root unavailable: {s}"),
            Self::IdentityCollision => {
                write!(f, "managed tool-output identity reservation exhausted")
            }
            Self::OpenFailed(s) => write!(f, "cannot open an output file: {s}"),
            Self::ModelMutationReadOnly(s) => write!(
                f,
                "filesystem path {s:?} is inside a managed tool-output root, which is read-only auxiliary storage; Write/Edit never mutate it"
            ),
        }
    }
}
impl std::error::Error for ManagedOutputError {}

#[derive(Debug)]
struct AllocatedBackground {
    path: PathBuf,
    // Retain the physical identity until executor settlement, so unlink cannot
    // recycle its inode before a sink validates the published path.
    file: Option<File>,
}
#[derive(Debug)]
struct ManagedOutputState {
    namespaces: storage::Namespaces,
    background: BTreeMap<ToolExecutionId, AllocatedBackground>,
    #[cfg(test)]
    force_open_failures: bool,
    #[cfg(test)]
    fail_writes_after: Option<u64>,
}

/// The shared storage owner of one Conversation, including lifecycle pins.
#[derive(Clone, Debug)]
pub struct ManagedToolOutput {
    lifecycle: Option<Arc<crate::runtime::local_storage::ConversationAccess>>,
    conversation_id: ConversationId,
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
    /// Bind disposable output to a trusted Conversation-local resource directory.
    /// Output itself is allocated beneath `std::env::temp_dir()`. The supplied
    /// directory stores only ownership metadata, never full tool output.
    /// Reopening retains prior bindings and starts a fresh allocation namespace.
    /// # Errors
    /// Invalid ownership metadata or inaccessible storage fails explicitly.
    pub fn new(
        conversation_id: ConversationId,
        local_resources: impl AsRef<Path>,
    ) -> Result<Self, ManagedOutputError> {
        Self::in_temp_root(
            conversation_id,
            local_resources.as_ref(),
            &std::env::temp_dir(),
        )
    }
    fn in_temp_root(
        conversation_id: ConversationId,
        local_resources: &Path,
        temp_root: &Path,
    ) -> Result<Self, ManagedOutputError> {
        let namespaces = storage::Namespaces::new(local_resources, &conversation_id, temp_root)
            .map_err(|e| ManagedOutputError::RootUnavailable(e.to_string()))?;
        Ok(Self {
            lifecycle: None,
            conversation_id,
            state: Arc::new(Mutex::new(ManagedOutputState {
                namespaces,
                background: BTreeMap::new(),
                #[cfg(test)]
                force_open_failures: false,
                #[cfg(test)]
                fail_writes_after: None,
            })),
            identities: Arc::new(crate::runtime::identity::SystemUuidV7Generator),
        })
    }

    #[cfg(test)]
    pub(crate) fn new_in_temp_root(
        conversation: ConversationId,
        owner: &Path,
        temporary: &Path,
    ) -> Result<Self, ManagedOutputError> {
        Self::in_temp_root(conversation, owner, temporary)
    }
    #[cfg(test)]
    pub(crate) fn set_force_open_failures(&self, enabled: bool) {
        self.state.lock().unwrap().force_open_failures = enabled;
    }
    #[cfg(test)]
    pub(crate) fn fail_writes_after(&self, bytes: u64) {
        self.state.lock().unwrap().fail_writes_after = Some(bytes);
    }
    /// Inject the semantic execution identity authority. Filesystem names do not
    /// derive from this identity.
    #[must_use]
    pub fn with_identity_generator(
        mut self,
        identities: Arc<dyn crate::runtime::identity::UuidV7Generator>,
    ) -> Self {
        self.identities = identities;
        self
    }
    /// Allocate a full semantic execution identity, independently of its locator.
    /// # Errors
    /// Rejects an injected identity that is not `UUIDv7`.
    pub fn execution_identity(&self) -> Result<ToolExecutionId, ManagedOutputError> {
        ToolExecutionId::from_uuid(self.identities.next_uuid())
            .map_err(ManagedOutputError::OpenFailed)
    }
    /// The owning Conversation.
    #[must_use]
    pub fn conversation_id(&self) -> &ConversationId {
        &self.conversation_id
    }
    /// Prepare the current temporary namespace, allocating lazily if necessary.
    /// Historical namespaces retain their original paths.
    /// # Errors
    /// Temporary storage may be unavailable without preventing Session recovery.
    /// # Panics
    /// Panics if another storage operation poisoned the owner mutex.
    pub fn root(&self) -> Result<PathBuf, ManagedOutputError> {
        self.state
            .lock()
            .expect("managed output lock")
            .namespaces
            .root()
            .map_err(|e| ManagedOutputError::RootUnavailable(e.to_string()))
    }
    pub(crate) fn with_workspace_root(self, workspace: &Path) -> Self {
        self.state
            .lock()
            .expect("managed output lock")
            .namespaces
            .set_workspace(workspace);
        self
    }
    /// All recorded namespaces, never the system temporary root.
    /// # Panics
    /// Panics if another storage operation poisoned the owner mutex.
    #[must_use]
    pub fn roots(&self) -> Vec<PathBuf> {
        self.state
            .lock()
            .expect("managed output lock")
            .namespaces
            .roots()
    }
    pub(crate) fn validate_locator(&self, path: &Path) -> std::io::Result<()> {
        self.state
            .lock()
            .expect("managed output lock")
            .namespaces
            .validate(path)
    }
    pub(crate) fn ensure_model_mutation_allowed(
        &self,
        effective_target: &Path,
    ) -> Result<(), ManagedOutputError> {
        if self
            .roots()
            .iter()
            .any(|root| effective_target.starts_with(root))
        {
            return Err(ManagedOutputError::ModelMutationReadOnly(
                effective_target.display().to_string(),
            ));
        }
        Ok(())
    }
    fn create(
        state: &mut ManagedOutputState,
        prefix: &str,
        extension: &str,
    ) -> Result<(File, PathBuf), ManagedOutputError> {
        #[cfg(test)]
        if state.force_open_failures {
            return Err(ManagedOutputError::OpenFailed(
                "test-forced output open failure".into(),
            ));
        }
        state.namespaces.create(prefix, extension).map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                ManagedOutputError::IdentityCollision
            } else {
                ManagedOutputError::OpenFailed(format!(
                    "{}: {e}",
                    state.namespaces.allocation_location().display()
                ))
            }
        })
    }
    /// Allocate a lazy foreground spill with exclusive creation and bounded retries.
    /// # Errors
    /// Reports allocation failure without overwriting any existing file.
    /// # Panics
    /// Panics if another storage operation poisoned the owner mutex.
    pub fn open_spill(&self) -> Result<ResultSpill, ManagedOutputError> {
        let mut state = self.state.lock().expect("managed output lock");
        let (file, path) = Self::create(&mut state, "r", ".txt")?;
        Ok(ResultSpill {
            _lifecycle: self.lifecycle.clone(),
            file,
            path,
            #[cfg(test)]
            fail_writes_after: state.fail_writes_after,
        })
    }
    /// Look up an allocated background locator, never synthesize one from an ID.
    /// # Panics
    /// The caller must have successfully allocated this execution before use.
    #[must_use]
    pub fn background_output_path(&self, execution_id: &ToolExecutionId) -> PathBuf {
        self.state
            .lock()
            .expect("managed output lock")
            .background
            .get(execution_id)
            .expect("background output allocated before use")
            .path
            .clone()
    }
    /// Allocate the live file before background acceptance. The owner binds the
    /// unchanged execution identity to its independently random short locator.
    /// # Errors
    /// A duplicate execution or failed file creation refuses dispatch.
    /// # Panics
    /// Panics if another storage operation poisoned the owner mutex.
    pub fn allocate_background_output(
        &self,
        execution_id: &ToolExecutionId,
    ) -> Result<PathBuf, ManagedOutputError> {
        let mut state = self.state.lock().expect("managed output lock");
        if state.background.contains_key(execution_id) {
            return Err(ManagedOutputError::IdentityCollision);
        }
        let (file, path) = Self::create(&mut state, "e", ".output")?;
        state.background.insert(
            execution_id.clone(),
            AllocatedBackground {
                path: path.clone(),
                file: Some(file),
            },
        );
        Ok(path)
    }
    /// Discard only a pre-commit allocation; unknown executions are a no-op.
    pub(crate) fn discard_background_output(&self, execution_id: &ToolExecutionId) {
        let mut state = self.state.lock().expect("managed output lock");
        if let Some(allocated) = state.background.remove(execution_id)
            && state.namespaces.validate(&allocated.path).is_ok()
        {
            let _ = state.namespaces.discard(&allocated.path);
        }
    }
    /// Executor settlement releases the allocation handle without removing
    /// published output. The locator remains available for terminal projection.
    pub(crate) fn settle_background_output(&self, execution: &ToolExecutionId) {
        if let Some(allocated) = self
            .state
            .lock()
            .expect("managed output lock")
            .background
            .get_mut(execution)
        {
            allocated.file.take();
        }
    }
    /// Open exactly the allocated background file without creating or repairing it.
    /// # Errors
    /// Missing, redirected or replaced output is unavailable, never empty output.
    /// # Panics
    /// Panics if another storage operation poisoned the owner mutex.
    pub fn open_background_output_sink(
        &self,
        execution_id: &ToolExecutionId,
    ) -> std::io::Result<BackgroundOutput> {
        let state = self.state.lock().expect("managed output lock");
        #[cfg(test)]
        if state.force_open_failures {
            return Err(std::io::Error::other("test-forced output open failure"));
        }
        let allocated = state
            .background
            .get(execution_id)
            .ok_or_else(|| std::io::Error::other("background output was not allocated"))?;
        state.namespaces.validate(&allocated.path)?;
        let retained = allocated
            .file
            .as_ref()
            .ok_or_else(|| std::io::Error::other("background output has physically settled"))?;
        let visible = state.namespaces.open_file(&allocated.path)?;
        let metadata = visible.metadata()?;
        let original = retained.metadata()?;
        if !metadata.is_file()
            || metadata.dev() != original.dev()
            || metadata.ino() != original.ino()
        {
            return Err(std::io::Error::other("background output was replaced"));
        }
        Ok(BackgroundOutput {
            _lifecycle: self.lifecycle.clone(),
            file: retained.try_clone()?,
            path: allocated.path.clone(),
            #[cfg(test)]
            fail_writes_after: state.fail_writes_after,
        })
    }
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
            let mut bytes = usize::try_from(*remaining)
                .unwrap_or(usize::MAX)
                .min(text.len());
            while !text.is_char_boundary(bytes) {
                bytes -= 1;
            }
            file.write_all(&text.as_bytes()[..bytes])?;
            *remaining = 0;
            return Err(std::io::Error::other("test-forced output write failure"));
        }
        *remaining -= len;
    }
    file.write_all(text.as_bytes())
}

#[cfg(test)]
mod tests;
