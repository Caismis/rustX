//! The conversation-owned tool runtime bundle.
//!
//! One conversation owns one [`ConversationToolRuntime`]: the canonical
//! workspace boundary, the artifact store, the explicit authorized tool
//! environment, the conversation inbound mailbox, and the authoritative
//! background registry. An `AgentExecution` receives a reference to the
//! bundle and drains `tool_runtime.mailbox()`; detached background runners
//! receive cloned handles. No process-global tool state exists.
//!
//! # Immutable resource binding
//!
//! Every background-runtime dependency (mailbox, clock, event sink,
//! environment, workspace, artifact store) is bound at construction time
//! through [`ConversationRuntimeConfig`]. After construction the
//! conversation background registry identity is stable: configuration can
//! never replace or reset the registry, so existing execution records can
//! never disappear because of a later configuration change.
//!
//! # Storage disjointness
//!
//! The artifact store and the model workspace must be disjoint filesystem
//! regions: runtime-private output files must never be observable through
//! Glob/Grep/Bash. Construction canonicalizes both roots and rejects an
//! artifact root that equals the workspace root, nests inside it, or
//! contains it (including symlink-resolved overlap).
//!
//! # Mailbox identity
//!
//! A `ConversationToolRuntime` may only contain resources belonging to its
//! own [`ConversationId`]. A configured mailbox must belong to the same
//! conversation as the runtime; a mismatch is rejected at construction,
//! before the background registry is built, so
//!
//! ```text
//! request.conversation_id
//! == tool_runtime.conversation_id
//! == tool_runtime.mailbox().conversation_id
//! == background_registry.conversation_id
//! ```
//!
//! holds structurally. An omitted mailbox constructs the canonical mailbox
//! of the runtime's own conversation.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::events::RuntimeEventSink;
use crate::runtime::RuntimeClock;
use crate::runtime::SystemClock;
use crate::runtime::identity::ConversationId;
use crate::runtime::inbound::ConversationInboundMailbox;
use crate::tools::artifacts::{ArtifactError, ArtifactStore};
use crate::tools::background::{BackgroundResources, ConversationBackgroundRegistry};
use crate::tools::environment::ToolEnvironment;
use crate::tools::workspace::{Workspace, WorkspaceError};

/// A conversation tool runtime construction failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConversationRuntimeError {
    /// The workspace root is invalid.
    Workspace(WorkspaceError),
    /// The artifact root cannot be prepared.
    Artifacts(ArtifactError),
    /// The artifact store and the model workspace overlap; the artifact
    /// root equals the workspace root, nests inside it, or contains it.
    OverlappingStorage {
        /// The canonical workspace root.
        workspace: PathBuf,
        /// The canonical artifact root.
        artifacts: PathBuf,
    },
    /// The configured mailbox belongs to a different conversation: a
    /// conversation runtime may only bind resources of its own
    /// [`ConversationId`].
    MailboxConversationMismatch {
        /// The conversation the runtime is being constructed for.
        expected: ConversationId,
        /// The conversation the configured mailbox belongs to.
        actual: ConversationId,
    },
}

impl core::fmt::Display for ConversationRuntimeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Workspace(error) => write!(f, "{error}"),
            Self::Artifacts(error) => write!(f, "{error}"),
            Self::OverlappingStorage {
                workspace,
                artifacts,
            } => write!(
                f,
                "the artifact root {} and the workspace root {} must be disjoint \
                 filesystem regions",
                artifacts.display(),
                workspace.display()
            ),
            Self::MailboxConversationMismatch { expected, actual } => write!(
                f,
                "the configured mailbox belongs to conversation {actual}, but this \
                 conversation tool runtime is being constructed for {expected}",
            ),
        }
    }
}

impl std::error::Error for ConversationRuntimeError {}

/// The bounded construction-time configuration of one conversation tool
/// runtime.
///
/// Every background-runtime dependency is bound here exactly once; there is
/// no post-construction mutation API, so the background registry identity
/// and its execution records are stable for the conversation lifetime.
#[derive(Clone)]
pub struct ConversationRuntimeConfig {
    /// The model-visible workspace root.
    pub workspace_root: PathBuf,
    /// The runtime-private artifact root, disjoint from the workspace.
    pub artifacts_dir: PathBuf,
    /// The canonical conversation inbound mailbox; a fresh mailbox bound to
    /// the conversation is created when omitted. A provided mailbox must
    /// belong to the same conversation as the runtime being constructed;
    /// a mismatched mailbox is rejected at construction.
    pub mailbox: Option<ConversationInboundMailbox>,
    /// The runtime clock stamping terminal inbound messages; the system
    /// clock is used when omitted.
    pub clock: Option<Arc<dyn RuntimeClock>>,
    /// The narrow non-durable execution-fact sink, when attached.
    pub event_sink: Option<Arc<dyn RuntimeEventSink>>,
    /// The explicit authorized tool environment; an empty environment is
    /// used when omitted.
    pub environment: Option<ToolEnvironment>,
}

impl ConversationRuntimeConfig {
    /// Creates the configuration for one conversation with the required
    /// storage roots and default background-runtime dependencies.
    #[must_use]
    pub fn new(workspace_root: impl AsRef<Path>, artifacts_dir: impl AsRef<Path>) -> Self {
        Self {
            workspace_root: workspace_root.as_ref().to_path_buf(),
            artifacts_dir: artifacts_dir.as_ref().to_path_buf(),
            mailbox: None,
            clock: None,
            event_sink: None,
            environment: None,
        }
    }
}

/// The conversation-owned tool runtime of one conversation.
///
/// Construction canonicalizes the workspace root once and requires it to be
/// a directory; artifact storage must be placed outside the model workspace
/// so Glob/Grep cannot accidentally surface runtime artifact internals. The
/// conversation background registry is constructed exactly once and never
/// replaced.
#[derive(Debug, Clone)]
pub struct ConversationToolRuntime {
    conversation_id: ConversationId,
    workspace: Workspace,
    artifacts: ArtifactStore,
    environment: ToolEnvironment,
    background: ConversationBackgroundRegistry,
}

impl ConversationToolRuntime {
    /// Creates the tool runtime of one conversation with default background
    /// dependencies: a fresh inbound mailbox for the conversation, the
    /// system clock, no event sink, and an empty tool environment.
    ///
    /// # Errors
    ///
    /// Returns [`ConversationRuntimeError::Workspace`] when the workspace
    /// root is missing, not a directory, or cannot be canonicalized,
    /// [`ConversationRuntimeError::Artifacts`] when the artifact root cannot
    /// be prepared, and
    /// [`ConversationRuntimeError::OverlappingStorage`] when the artifact
    /// root and the workspace root overlap.
    pub fn new(
        conversation_id: ConversationId,
        workspace_root: impl AsRef<Path>,
        artifacts_dir: impl AsRef<Path>,
    ) -> Result<Self, ConversationRuntimeError> {
        Self::from_config(
            conversation_id,
            ConversationRuntimeConfig::new(workspace_root, artifacts_dir),
        )
    }

    /// Creates the tool runtime of one conversation from the complete
    /// construction-time configuration.
    ///
    /// Every background-runtime dependency is bound exactly once here: the
    /// workspace is canonicalized, the artifact root is prepared and
    /// validated to be disjoint from the workspace, and the conversation
    /// background registry is constructed. Later configuration changes are
    /// structurally impossible, so the registry identity and its execution
    /// records are stable for the conversation lifetime.
    ///
    /// A configured mailbox must belong to the same conversation as the
    /// runtime itself: a `ConversationToolRuntime` may only contain
    /// resources belonging to its own [`ConversationId`]. The mismatch is
    /// rejected here, before the background registry is constructed — never
    /// deferred to `AgentExecution`.
    ///
    /// # Errors
    ///
    /// Returns [`ConversationRuntimeError::Workspace`] when the workspace
    /// root is missing, not a directory, or cannot be canonicalized,
    /// [`ConversationRuntimeError::Artifacts`] when the artifact root cannot
    /// be prepared,
    /// [`ConversationRuntimeError::OverlappingStorage`] when the artifact
    /// root and the workspace root overlap (directly, nested, or through a
    /// symlink), and
    /// [`ConversationRuntimeError::MailboxConversationMismatch`] when the
    /// configured mailbox belongs to a different conversation.
    pub fn from_config(
        conversation_id: ConversationId,
        config: ConversationRuntimeConfig,
    ) -> Result<Self, ConversationRuntimeError> {
        // The mailbox identity is validated before any resource binding:
        // the runtime construction boundary owns this invariant, so a
        // mailbox of another conversation can never enter the registry or
        // any other runtime resource.
        let mailbox = match config.mailbox {
            Some(mailbox) => {
                if mailbox.conversation_id() != &conversation_id {
                    return Err(ConversationRuntimeError::MailboxConversationMismatch {
                        expected: conversation_id.clone(),
                        actual: mailbox.conversation_id().clone(),
                    });
                }
                mailbox
            }
            None => ConversationInboundMailbox::new(conversation_id.clone()),
        };
        let workspace =
            Workspace::new(&config.workspace_root).map_err(ConversationRuntimeError::Workspace)?;
        let artifacts_root = prepare_artifact_root(&config.artifacts_dir)
            .map_err(ConversationRuntimeError::Artifacts)?;
        validate_disjoint_storage(workspace.root(), &artifacts_root)?;
        let artifacts = ArtifactStore::new(conversation_id.clone(), &artifacts_root)
            .map_err(ConversationRuntimeError::Artifacts)?;
        let clock = config
            .clock
            .unwrap_or_else(|| Arc::new(SystemClock) as Arc<dyn RuntimeClock>);
        let environment = config.environment.unwrap_or_default();
        let background = ConversationBackgroundRegistry::new(
            conversation_id.clone(),
            BackgroundResources {
                mailbox: mailbox.clone(),
                workspace: workspace.clone(),
                artifacts: artifacts.clone(),
                environment: environment.clone(),
                clock,
                event_sink: config.event_sink,
            },
        );
        Ok(Self {
            conversation_id,
            workspace,
            artifacts,
            environment,
            background,
        })
    }

    /// The owning conversation.
    #[must_use]
    pub fn conversation_id(&self) -> &ConversationId {
        &self.conversation_id
    }

    /// The canonical workspace boundary.
    #[must_use]
    pub fn workspace(&self) -> &Workspace {
        &self.workspace
    }

    /// The conversation artifact store.
    #[must_use]
    pub fn artifacts(&self) -> &ArtifactStore {
        &self.artifacts
    }

    /// The explicit authorized tool environment.
    #[must_use]
    pub fn environment(&self) -> &ToolEnvironment {
        &self.environment
    }

    /// The authoritative conversation background registry.
    #[must_use]
    pub fn background(&self) -> &ConversationBackgroundRegistry {
        &self.background
    }

    /// The canonical conversation inbound mailbox.
    ///
    /// Background terminal notifications are published into exactly this
    /// mailbox, and an `AgentExecution` over this runtime drains exactly
    /// this mailbox at every safe boundary: one conversation has one
    /// canonical inbound ordering domain.
    #[must_use]
    pub fn mailbox(&self) -> ConversationInboundMailbox {
        self.background.resources().mailbox.clone()
    }
}

/// Prepares the artifact root: creates it when missing and canonicalizes it
/// so the disjointness check operates on resolved filesystem locations.
fn prepare_artifact_root(root: &Path) -> Result<PathBuf, ArtifactError> {
    std::fs::create_dir_all(root)
        .map_err(|error| ArtifactError::RootUnavailable(format!("{}: {error}", root.display())))?;
    std::fs::canonicalize(root)
        .map_err(|error| ArtifactError::RootUnavailable(format!("{}: {error}", root.display())))
}

/// Validates that the artifact root and the workspace root are disjoint
/// filesystem regions. Both roots are canonical, so symlink-resolved
/// overlap is rejected.
fn validate_disjoint_storage(
    workspace_root: &Path,
    artifacts_root: &Path,
) -> Result<(), ConversationRuntimeError> {
    if artifacts_root == workspace_root
        || artifacts_root.starts_with(workspace_root)
        || workspace_root.starts_with(artifacts_root)
    {
        return Err(ConversationRuntimeError::OverlappingStorage {
            workspace: workspace_root.to_path_buf(),
            artifacts: artifacts_root.to_path_buf(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{ConversationRuntimeConfig, ConversationRuntimeError, ConversationToolRuntime};
    use crate::runtime::identity::ConversationId;
    use std::fs;

    fn unique_dir(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "rustx-crt-{}-{}-{}",
            name,
            std::process::id(),
            std::thread::current().name().unwrap_or("t")
        ))
    }

    #[test]
    fn construction_validates_the_workspace_root() {
        let dir = unique_dir("ws-root");
        let runtime = ConversationToolRuntime::new(
            ConversationId::new("conv-1"),
            &dir,
            dir.join("artifacts"),
        );
        assert!(
            runtime.is_err(),
            "a missing workspace root must be rejected"
        );
        fs::create_dir_all(dir.join("workspace")).expect("create");
        let runtime = ConversationToolRuntime::new(
            ConversationId::new("conv-1"),
            dir.join("workspace"),
            dir.join("artifacts"),
        )
        .expect("runtime");
        assert_eq!(runtime.conversation_id(), &ConversationId::new("conv-1"));
        assert!(runtime.workspace().root().is_dir());
        assert!(runtime.artifacts().root().is_dir());
        assert_eq!(
            runtime.mailbox().conversation_id(),
            &ConversationId::new("conv-1")
        );
        fs::remove_dir_all(&dir).expect("remove");
    }

    #[test]
    fn artifact_root_equal_to_workspace_is_rejected() {
        let dir = unique_dir("overlap-equal");
        fs::create_dir_all(&dir).expect("create");
        let error = ConversationToolRuntime::new(ConversationId::new("conv-1"), &dir, &dir)
            .expect_err("equal roots must be rejected");
        assert!(matches!(
            error,
            ConversationRuntimeError::OverlappingStorage { .. }
        ));
        fs::remove_dir_all(&dir).expect("remove");
    }

    #[test]
    fn artifact_root_nested_inside_workspace_is_rejected() {
        let dir = unique_dir("overlap-nested");
        fs::create_dir_all(dir.join("workspace")).expect("create");
        let error = ConversationToolRuntime::new(
            ConversationId::new("conv-1"),
            dir.join("workspace"),
            dir.join("workspace/artifacts"),
        )
        .expect_err("nested artifact root must be rejected");
        assert!(matches!(
            error,
            ConversationRuntimeError::OverlappingStorage { .. }
        ));
        fs::remove_dir_all(&dir).expect("remove");
    }

    #[test]
    fn workspace_root_nested_inside_artifact_root_is_rejected() {
        let dir = unique_dir("overlap-reverse");
        fs::create_dir_all(dir.join("artifacts/workspace")).expect("create");
        let error = ConversationToolRuntime::new(
            ConversationId::new("conv-1"),
            dir.join("artifacts/workspace"),
            dir.join("artifacts"),
        )
        .expect_err("the workspace root inside the artifact root must be rejected");
        assert!(matches!(
            error,
            ConversationRuntimeError::OverlappingStorage { .. }
        ));
        fs::remove_dir_all(&dir).expect("remove");
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_artifact_root_resolving_inside_workspace_is_rejected() {
        use std::os::unix::fs::symlink;
        let dir = unique_dir("overlap-symlink");
        fs::create_dir_all(dir.join("workspace/real")).expect("create");
        symlink(dir.join("workspace/real"), dir.join("linked-artifacts")).expect("symlink");
        let error = ConversationToolRuntime::new(
            ConversationId::new("conv-1"),
            dir.join("workspace"),
            dir.join("linked-artifacts"),
        )
        .expect_err("a symlinked artifact root inside the workspace must be rejected");
        assert!(matches!(
            error,
            ConversationRuntimeError::OverlappingStorage { .. }
        ));
        fs::remove_dir_all(&dir).expect("remove");
    }

    #[test]
    fn sibling_storage_layout_is_accepted() {
        let dir = unique_dir("sibling");
        fs::create_dir_all(dir.join("workspace")).expect("create");
        let runtime = ConversationToolRuntime::new(
            ConversationId::new("conv-1"),
            dir.join("workspace"),
            dir.join("artifacts"),
        )
        .expect("sibling roots are disjoint");
        assert!(runtime.workspace().root().is_dir());
        assert!(runtime.artifacts().root().is_dir());
        fs::remove_dir_all(&dir).expect("remove");
    }

    #[test]
    fn configuration_binds_resources_exactly_once() {
        use crate::runtime::inbound::ConversationInboundMailbox;
        let dir = unique_dir("bind-once");
        fs::create_dir_all(dir.join("workspace")).expect("create");
        let mailbox = ConversationInboundMailbox::new(ConversationId::new("conv-1"));
        let runtime = ConversationToolRuntime::from_config(
            ConversationId::new("conv-1"),
            ConversationRuntimeConfig {
                mailbox: Some(mailbox.clone()),
                ..ConversationRuntimeConfig::new(dir.join("workspace"), dir.join("artifacts"))
            },
        )
        .expect("runtime");
        // The configured mailbox is the canonical mailbox shared by the
        // background registry: terminal notifications reach exactly it.
        assert_eq!(
            runtime.mailbox().conversation_id(),
            mailbox.conversation_id()
        );
        let enqueued = runtime.background().resources().mailbox.clone();
        assert_eq!(enqueued.conversation_id(), mailbox.conversation_id());
        fs::remove_dir_all(&dir).expect("remove");
    }

    /// A configured mailbox belonging to the runtime's own conversation is
    /// accepted, and the constructed runtime exposes exactly that mailbox.
    #[test]
    fn matching_mailbox_conversation_is_accepted() {
        use crate::runtime::inbound::ConversationInboundMailbox;
        let dir = unique_dir("mailbox-match");
        fs::create_dir_all(dir.join("workspace")).expect("create");
        let mailbox = ConversationInboundMailbox::new(ConversationId::new("conv-A"));
        let runtime = ConversationToolRuntime::from_config(
            ConversationId::new("conv-A"),
            ConversationRuntimeConfig {
                mailbox: Some(mailbox.clone()),
                ..ConversationRuntimeConfig::new(dir.join("workspace"), dir.join("artifacts"))
            },
        )
        .expect("a matching mailbox must be accepted");
        // The exposed canonical mailbox is the configured one and belongs to
        // the runtime's own conversation.
        assert_eq!(
            runtime.mailbox().conversation_id(),
            &ConversationId::new("conv-A")
        );
        assert_eq!(
            runtime.background().resources().mailbox.conversation_id(),
            &ConversationId::new("conv-A")
        );
        fs::remove_dir_all(&dir).expect("remove");
    }

    /// A configured mailbox belonging to a different conversation is
    /// rejected at construction: the runtime may only bind resources of its
    /// own conversation.
    #[test]
    fn mismatched_mailbox_conversation_is_rejected() {
        use crate::runtime::inbound::ConversationInboundMailbox;
        let dir = unique_dir("mailbox-mismatch");
        fs::create_dir_all(dir.join("workspace")).expect("create");
        let mailbox = ConversationInboundMailbox::new(ConversationId::new("conv-B"));
        let error = ConversationToolRuntime::from_config(
            ConversationId::new("conv-A"),
            ConversationRuntimeConfig {
                mailbox: Some(mailbox),
                ..ConversationRuntimeConfig::new(dir.join("workspace"), dir.join("artifacts"))
            },
        )
        .expect_err("a foreign mailbox must be rejected");
        assert_eq!(
            error,
            ConversationRuntimeError::MailboxConversationMismatch {
                expected: ConversationId::new("conv-A"),
                actual: ConversationId::new("conv-B"),
            }
        );
        fs::remove_dir_all(&dir).expect("remove");
    }

    /// An omitted mailbox constructs the canonical mailbox of the runtime's
    /// own conversation.
    #[test]
    fn omitted_mailbox_constructs_the_canonical_conversation_mailbox() {
        let dir = unique_dir("mailbox-omitted");
        fs::create_dir_all(dir.join("workspace")).expect("create");
        let runtime = ConversationToolRuntime::new(
            ConversationId::new("conv-A"),
            dir.join("workspace"),
            dir.join("artifacts"),
        )
        .expect("runtime");
        assert_eq!(
            runtime.mailbox().conversation_id(),
            &ConversationId::new("conv-A"),
            "the canonical mailbox belongs to the runtime's own conversation"
        );
        fs::remove_dir_all(&dir).expect("remove");
    }
}
