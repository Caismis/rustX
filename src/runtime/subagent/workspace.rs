//! Deterministic workspace ownership for named subagents (Issue #146).
//!
//! This module is the only owner of Git/worktree operations.  The registry
//! supplies it with a resolved policy and an already allocated subagent
//! identity; it never constructs Git commands itself. A [`WorkspaceLease`]
//! is the physical ownership token that moves from preparation to the child
//! process driver at the same boundary as the process handle. The lease keeps
//! the child's logical project authority distinct from the physical worktree
//! root that the manager owns and settles.
//!
//! The important snapshot rule is intentionally visible in the types and in
//! the command order:
//!
//! ```text
//! capture HEAD = C
//! capture parent status
//! freeze selected ignored overlay bytes
//! create worktree at explicit C
//! materialize and verify the frozen overlay
//! ```
//!
//! The parent path is never consulted again to choose the child's base.  The
//! isolated-worktree default policy requires the ordinary source workspace to
//! be clean before a child is created (Issue #188); only an explicit
//! `require_clean_parent = false` permits a dirty parent, and that permissive
//! path still never copies any parent dirty bytes into the child. A later
//! parent commit cannot move an already acquired child.
//!
//! Terminal changed-state is deliberately two-dimensional:
//! `dirty = ordinary tracked/index/untracked-non-ignored Git status`, while
//! `changed = dirty || (final HEAD != base_commit)`. Ignored build/cache
//! artifacts alone are disposable execution output and do not force a
//! handoff.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::process::Stdio;
use tokio::process::Command;

use crate::runtime::cancellation::CancellationSignal;
use crate::runtime::identity::SubagentId;

/// The only manifest name recognized by isolated workspace acquisition.
const WORKTREE_INCLUDE_MANIFEST: &str = ".worktreeinclude";
/// A deliberately small v1 selection bound. Entries select files, never
/// directories or recursive trees.
const MAX_OVERLAY_FILES: usize = 64;
/// The total runtime-owned frozen content bound across every selected file.
const MAX_OVERLAY_BYTES: usize = 8 * 1024 * 1024;
/// Prevent an input manifest from becoming an unbounded pre-parse read.
const MAX_OVERLAY_MANIFEST_BYTES: u64 = 64 * 1024;

/// Acquisition-internal immutable bytes selected from the parent logical
/// workspace. This value never crosses the child/durable protocol boundary.
#[derive(Debug)]
struct FrozenOverlayFile {
    relative_path: PathBuf,
    repository_relative_path: PathBuf,
    bytes: Vec<u8>,
}

/// One selected path after every path/Git eligibility check and before any
/// selected file content is read.
#[derive(Debug)]
struct ValidatedOverlayFile {
    relative: PathBuf,
    repository_relative: PathBuf,
    source: PathBuf,
}

/// The bounded workspace policy resolved from a named subagent definition.
///
/// This is deliberately the complete policy vocabulary for this milestone:
/// shared workspace or one Git worktree.  It is not a provider/strategy
/// extension point.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SubagentWorkspacePolicy {
    /// The current shared-workspace behavior.
    #[default]
    SharedWorkspace,
    /// An isolated Git worktree based on one committed source snapshot.
    GitWorktree {
        /// Reject acquisition when the parent has uncommitted changes.
        ///
        /// The configuration boundary normalizes this field: enabled
        /// isolation is strict (`true`) by default (Issue #188). `false` is
        /// the explicit opt-out that runs from the captured committed `HEAD`
        /// while excluding dirty parent bytes.
        require_clean_parent: bool,
    },
}

impl SubagentWorkspacePolicy {
    /// Whether the policy acquires a separate Git worktree.
    #[must_use]
    pub const fn is_isolated(self) -> bool {
        matches!(self, Self::GitWorktree { .. })
    }
}

/// The immutable execution facts selected before child ownership commits.
///
/// This value is runtime-owned execution context, not named-definition state
/// and not model-authored content. `logical_workspace` is always the child's
/// project authority. Physical Git ownership exists only inside the closed
/// [`WorkspaceIsolation::GitWorktree`] variant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceSnapshot {
    /// The authoritative logical project path given to the child.
    pub logical_workspace: PathBuf,
    /// The closed physical-isolation facts for this selection.
    pub isolation: WorkspaceIsolation,
}

/// The physical isolation mode of an immutable workspace selection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "facts",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum WorkspaceIsolation {
    /// The child shares the parent's logical project workspace. The runtime
    /// owns no separate Git checkout.
    Shared,
    /// The runtime owns one physical worktree while the child is authorized
    /// only for the corresponding logical project path inside it.
    GitWorktree(GitWorktreeSnapshot),
}

/// Immutable Git ownership and scope facts for one isolated child.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GitWorktreeSnapshot {
    /// The canonical top-level of the source repository.
    pub source_repository_root: PathBuf,
    /// The parent logical workspace relative to the source repository root.
    /// The empty path denotes the repository root itself.
    pub repository_relative_workspace: PathBuf,
    /// The physical runtime-owned root registered by `git worktree add`.
    pub physical_worktree_root: PathBuf,
    /// The exact committed source `HEAD` selected before acquisition.
    pub base_commit: String,
    /// The runtime-created branch/ref.
    pub branch: String,
    /// Whether the parent had tracked/index/untracked changes at selection.
    pub parent_had_uncommitted_changes: bool,
}

impl WorkspaceSnapshot {
    /// Constructs the runtime-owned snapshot for the current shared-workspace
    /// mode. This is public so protocol/test fixtures can state the same
    /// explicit authority without manufacturing an isolated Git fact.
    #[must_use]
    pub fn shared(workspace: impl Into<PathBuf>) -> Self {
        Self {
            logical_workspace: workspace.into(),
            isolation: WorkspaceIsolation::Shared,
        }
    }

    fn worktree(
        logical_workspace: PathBuf,
        source_repository_root: PathBuf,
        repository_relative_workspace: PathBuf,
        physical_worktree_root: PathBuf,
        base_commit: String,
        branch: String,
        parent_had_uncommitted_changes: bool,
    ) -> Self {
        Self {
            logical_workspace,
            isolation: WorkspaceIsolation::GitWorktree(GitWorktreeSnapshot {
                source_repository_root,
                repository_relative_workspace,
                physical_worktree_root,
                base_commit,
                branch,
                parent_had_uncommitted_changes,
            }),
        }
    }

    /// Whether this child uses a runtime-created Git worktree.
    #[must_use]
    pub const fn is_isolated(&self) -> bool {
        matches!(self.isolation, WorkspaceIsolation::GitWorktree(_))
    }

    /// The isolated Git facts, when the runtime owns a physical worktree.
    #[must_use]
    pub const fn git_worktree(&self) -> Option<&GitWorktreeSnapshot> {
        match &self.isolation {
            WorkspaceIsolation::Shared => None,
            WorkspaceIsolation::GitWorktree(worktree) => Some(worktree),
        }
    }

    /// Validates the closed shared/worktree shape before it crosses a durable
    /// or process boundary.
    ///
    /// The manager constructs this value, but the durable store and child
    /// process also validate it so a malformed event/spec cannot silently
    /// turn an isolated child into an untracked arbitrary path.
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.logical_workspace.as_os_str().is_empty() {
            return Err("workspace snapshot has an empty logical project path".to_owned());
        }
        if let WorkspaceIsolation::GitWorktree(worktree) = &self.isolation {
            if !worktree.source_repository_root.is_absolute() {
                return Err(
                    "isolated workspace snapshot source repository root is not absolute".to_owned(),
                );
            }
            if !worktree.physical_worktree_root.is_absolute() {
                return Err(
                    "isolated workspace snapshot physical worktree root is not absolute".to_owned(),
                );
            }
            if !self.logical_workspace.is_absolute() {
                return Err(
                    "isolated workspace snapshot logical project path is not absolute".to_owned(),
                );
            }
            if worktree.base_commit.is_empty() {
                return Err("isolated workspace snapshot has no base commit".to_owned());
            }
            if worktree.branch.is_empty() {
                return Err("isolated workspace snapshot has no branch/ref".to_owned());
            }
            if !is_safe_repository_relative(&worktree.repository_relative_workspace) {
                return Err(
                    "isolated workspace snapshot has an invalid repository-relative workspace"
                        .to_owned(),
                );
            }
            if self.logical_workspace
                != worktree
                    .physical_worktree_root
                    .join(&worktree.repository_relative_workspace)
            {
                return Err(
                    "isolated workspace snapshot logical path does not match its physical worktree root and repository-relative workspace"
                        .to_owned(),
                );
            }
        }
        Ok(())
    }

    pub(crate) fn matches_handoff(&self, handoff: &WorkspaceHandoff) -> bool {
        let Some(worktree) = self.git_worktree() else {
            return false;
        };
        handoff.validate().is_ok()
            && handoff.logical_workspace == self.logical_workspace
            && handoff.physical_worktree_root == worktree.physical_worktree_root
            && handoff.branch == worktree.branch
            && handoff.base_commit == worktree.base_commit
    }
}

/// The user-recoverable facts of a retained child workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceHandoff {
    /// The preserved logical project scope used by the child.
    pub logical_workspace: PathBuf,
    /// The preserved physical Git worktree root a user/runtime can inspect.
    pub physical_worktree_root: PathBuf,
    /// The runtime-created branch/ref.
    pub branch: String,
    /// The source commit selected before child ownership.
    pub base_commit: String,
    /// The final child `HEAD` observed during settlement.
    pub head_commit: String,
    /// Whether the final ordinary Git status was dirty. This includes
    /// tracked/index changes and untracked non-ignored files; ignored files
    /// alone do not make a handoff dirty. A committed child change is tracked
    /// independently by `head_commit != base_commit`.
    pub dirty: bool,
}

impl WorkspaceHandoff {
    /// Validates the complete handoff fact before it crosses a durable or
    /// client boundary. The manager only constructs non-empty Git facts, but
    /// recovery and durable validation must not trust a decoded event to
    /// provide a usable path/ref or commit identity.
    pub(crate) fn validate(&self) -> Result<(), String> {
        if !self.logical_workspace.is_absolute() {
            return Err("workspace handoff logical project path is not absolute".to_owned());
        }
        if !self.physical_worktree_root.is_absolute() {
            return Err("workspace handoff physical worktree root is not absolute".to_owned());
        }
        if !self
            .logical_workspace
            .starts_with(&self.physical_worktree_root)
        {
            return Err(
                "workspace handoff logical path is outside its physical worktree root".to_owned(),
            );
        }
        if self.branch.is_empty() {
            return Err("workspace handoff has an empty branch/ref".to_owned());
        }
        if self.base_commit.is_empty() {
            return Err("workspace handoff has an empty base commit".to_owned());
        }
        if self.head_commit.is_empty() {
            return Err("workspace handoff has an empty head commit".to_owned());
        }
        Ok(())
    }
}

/// The physical disposition of a workspace lease after inspection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceCleanup {
    /// Shared workspace: there is no runtime-owned worktree to remove.
    Shared,
    /// The runtime-created clean worktree and its branch were removed.
    Removed,
    /// The worktree was deliberately retained for handoff or because cleanup
    /// could not be proven safe.
    Preserved,
}

/// The final workspace facts produced by the one lease owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceSettlement {
    /// The immutable selection facts.
    pub snapshot: WorkspaceSnapshot,
    /// Recovery metadata when the physical worktree remains available.
    pub handoff: Option<WorkspaceHandoff>,
    /// The cleanup disposition.
    pub cleanup: WorkspaceCleanup,
    /// A bounded physical inspection/cleanup failure, if any.
    pub error: Option<String>,
}

impl WorkspaceSettlement {
    /// A settlement for a shared workspace.
    #[must_use]
    pub fn shared(snapshot: WorkspaceSnapshot) -> Self {
        Self {
            snapshot,
            handoff: None,
            cleanup: WorkspaceCleanup::Shared,
            error: None,
        }
    }

    fn unresolved(snapshot: WorkspaceSnapshot, error: impl Into<String>) -> Self {
        Self {
            snapshot,
            handoff: None,
            cleanup: WorkspaceCleanup::Preserved,
            error: Some(error.into()),
        }
    }
}

/// A staged workspace acquisition/ownership failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceSettlementError {
    /// The reason the staged lease could not be settled as disposable.
    pub detail: String,
    /// The conservative physical settlement, including a handoff when one
    /// could be inspected.
    pub settlement: Box<WorkspaceSettlement>,
}

impl core::fmt::Display for WorkspaceSettlementError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(&self.detail)
    }
}

impl std::error::Error for WorkspaceSettlementError {}

/// A failure while selecting or creating a child workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceAcquireError {
    /// The invoking attempt's cancellation won before acquisition completed.
    Cancelled,
    /// The parent is dirty and the strict clean-parent policy rejected it.
    DirtyParent {
        /// The exact committed `HEAD` captured before the dirty observation.
        base_commit: String,
    },
    /// The logical-workspace manifest could not be read or parsed safely.
    OverlayManifest {
        /// A bounded diagnostic that never contains selected file bytes.
        detail: String,
    },
    /// One manifest entry is not an ordinary logical-workspace-relative path.
    OverlayUnsafePath {
        /// The one-based manifest line, when rejection occurs during parse.
        line: Option<usize>,
        /// The rejected textual entry.
        path: String,
    },
    /// Two entries resolve to the same canonical overlay destination.
    OverlayDuplicate {
        /// The duplicated logical-workspace-relative path.
        path: PathBuf,
    },
    /// An explicitly selected overlay file does not exist.
    OverlayMissing {
        /// The missing logical-workspace-relative path.
        path: PathBuf,
    },
    /// A selected path is tracked in the current index or captured commit.
    OverlayTracked {
        /// The tracked logical-workspace-relative path.
        path: PathBuf,
    },
    /// Git does not classify a selected path as ignored.
    OverlayNotIgnored {
        /// The non-ignored logical-workspace-relative path.
        path: PathBuf,
    },
    /// A selected file or one of its parent components is a symlink.
    OverlaySymlink {
        /// The affected logical-workspace-relative path.
        path: PathBuf,
    },
    /// A selected path exists but is not an individual regular file.
    OverlayNotFile {
        /// The affected logical-workspace-relative path.
        path: PathBuf,
    },
    /// The manifest selects more files than the fixed v1 bound.
    OverlayFileLimit {
        /// The fixed maximum accepted file count.
        limit: usize,
    },
    /// The selected content exceeds the fixed total frozen-byte bound.
    OverlayByteLimit {
        /// The fixed maximum accepted total bytes.
        limit: usize,
    },
    /// A validated source file could not be frozen.
    OverlayFreeze {
        /// The logical-workspace-relative path; never file contents.
        path: PathBuf,
        /// The bounded filesystem diagnostic.
        detail: String,
    },
    /// Frozen bytes could not be materialized and verified in the child.
    OverlayMaterialization {
        /// The logical-workspace-relative path; never file contents.
        path: PathBuf,
        /// The bounded filesystem diagnostic.
        detail: String,
    },
    /// Git is unavailable or returned a failure for a local operation.
    Git {
        /// The bounded operation name.
        operation: String,
        /// The bounded command detail.
        detail: String,
    },
    /// The deterministic path/ref is already occupied.
    Collision {
        /// The deterministic physical worktree root.
        physical_worktree_root: PathBuf,
        /// The deterministic ref.
        branch: String,
    },
    /// The created worktree did not prove the requested base.
    InvalidSnapshot {
        /// The bounded diagnostic.
        detail: String,
    },
    /// A staged worktree could not be safely settled after acquisition
    /// failed. The path is deliberately retained in that case.
    Settlement {
        /// The bounded settlement diagnostic.
        detail: String,
    },
}

impl core::fmt::Display for WorkspaceAcquireError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Cancelled => formatter.write_str("workspace acquisition was cancelled"),
            Self::DirtyParent { .. } => formatter.write_str(
                "the parent workspace has uncommitted changes and the clean-parent \
                 policy rejected isolated workspace acquisition",
            ),
            Self::OverlayManifest { detail } => {
                write!(
                    formatter,
                    "the .worktreeinclude manifest is invalid: {detail}"
                )
            }
            Self::OverlayUnsafePath {
                line: Some(line),
                path,
            } => write!(
                formatter,
                ".worktreeinclude line {line} is not a safe logical-workspace-relative file path: {path:?}"
            ),
            Self::OverlayUnsafePath { line: None, path } => write!(
                formatter,
                ".worktreeinclude path escaped the canonical logical workspace: {path:?}"
            ),
            Self::OverlayDuplicate { path } => write!(
                formatter,
                ".worktreeinclude selects the same overlay destination more than once: {}",
                path.display()
            ),
            Self::OverlayMissing { path } => write!(
                formatter,
                ".worktreeinclude selected a missing file: {}",
                path.display()
            ),
            Self::OverlayTracked { path } => write!(
                formatter,
                ".worktreeinclude selected a tracked file; only Git-ignored local files are eligible: {}",
                path.display()
            ),
            Self::OverlayNotIgnored { path } => write!(
                formatter,
                ".worktreeinclude selected a file that Git does not classify as ignored: {}",
                path.display()
            ),
            Self::OverlaySymlink { path } => write!(
                formatter,
                ".worktreeinclude selected a symlink or a path below one: {}",
                path.display()
            ),
            Self::OverlayNotFile { path } => write!(
                formatter,
                ".worktreeinclude entries must select individual regular files: {}",
                path.display()
            ),
            Self::OverlayFileLimit { limit } => write!(
                formatter,
                ".worktreeinclude selects more than the fixed limit of {limit} files"
            ),
            Self::OverlayByteLimit { limit } => write!(
                formatter,
                ".worktreeinclude selected content exceeds the fixed total limit of {limit} bytes"
            ),
            Self::OverlayFreeze { path, detail } => write!(
                formatter,
                "could not freeze .worktreeinclude file {}: {detail}",
                path.display()
            ),
            Self::OverlayMaterialization { path, detail } => write!(
                formatter,
                "could not materialize and verify .worktreeinclude file {}: {detail}",
                path.display()
            ),
            Self::Git { operation, detail } => {
                write!(formatter, "Git {operation} failed: {detail}")
            }
            Self::Collision {
                physical_worktree_root,
                branch,
            } => write!(
                formatter,
                "the deterministic child worktree path {} or branch {branch} is already occupied",
                physical_worktree_root.display()
            ),
            Self::InvalidSnapshot { detail } => {
                write!(
                    formatter,
                    "the acquired worktree did not prove its requested snapshot: {detail}"
                )
            }
            Self::Settlement { detail } => {
                write!(
                    formatter,
                    "workspace staging settlement was not proven safe: {detail}"
                )
            }
        }
    }
}

impl std::error::Error for WorkspaceAcquireError {}

/// The one manager of physical named-subagent workspaces.
#[derive(Debug, Clone)]
pub struct SubagentWorkspaceManager {
    parent_logical_workspace: PathBuf,
    runtime_root: PathBuf,
    #[cfg(test)]
    acquisition_hook: Option<std::sync::Arc<WorkspaceAcquireHook>>,
    #[cfg(test)]
    overlay_freeze_hook: Option<std::sync::Arc<WorkspaceOverlayFreezeHook>>,
    #[cfg(test)]
    settlement_hook: Option<std::sync::Arc<WorkspaceSettlementHook>>,
}

impl SubagentWorkspaceManager {
    /// Creates a manager over the already-canonical parent workspace and the
    /// disjoint runtime-private artifact root.
    #[must_use]
    pub fn new(parent_workspace: impl AsRef<Path>, runtime_root: impl AsRef<Path>) -> Self {
        Self {
            parent_logical_workspace: parent_workspace.as_ref().to_path_buf(),
            runtime_root: runtime_root.as_ref().to_path_buf(),
            #[cfg(test)]
            acquisition_hook: None,
            #[cfg(test)]
            overlay_freeze_hook: None,
            #[cfg(test)]
            settlement_hook: None,
        }
    }

    /// Installs a test-only barrier after worktree and overlay verification,
    /// immediately before the prepared lease can leave acquisition.
    #[cfg(test)]
    pub(crate) fn install_acquisition_hook(&mut self, hook: std::sync::Arc<WorkspaceAcquireHook>) {
        self.acquisition_hook = Some(hook);
    }

    /// Installs a test-only barrier after overlay bytes are frozen and before
    /// the physical worktree exists. It proves later parent edits cannot
    /// influence materialization without introducing production timing.
    #[cfg(test)]
    fn install_overlay_freeze_hook(&mut self, hook: std::sync::Arc<WorkspaceOverlayFreezeHook>) {
        self.overlay_freeze_hook = Some(hook);
    }

    /// Installs a one-shot test seam immediately before final workspace Git
    /// inspection. The hook is intentionally manager-local: it injects a
    /// physical settlement failure after the child driver has completed its
    /// semantic result, without adding a production failure mode.
    #[cfg(test)]
    pub(crate) fn install_settlement_hook(
        &mut self,
        hook: std::sync::Arc<WorkspaceSettlementHook>,
    ) {
        self.settlement_hook = Some(hook);
    }

    /// Acquires one workspace according to the resolved named-agent policy.
    ///
    /// For a Git worktree, the operations are deliberately ordered as
    /// repository resolution, exact `HEAD` capture, parent status observation,
    /// strict-policy enforcement, overlay selection/freezing, worktree
    /// creation at the captured commit, and frozen-overlay materialization and
    /// verification. A cancellation during an in-flight Git command kills
    /// that command and settles any partially created worktree without force
    /// deleting dirty state.
    ///
    /// # Errors
    ///
    /// Returns a typed local acquisition error when Git is unavailable, the
    /// strict parent policy rejects dirty state, cancellation wins, or the
    /// deterministic path/ref is occupied. A staged-settlement error is
    /// returned when physical cleanup cannot be proven safe.
    ///
    /// # Panics
    ///
    /// Panics only if the fixed `worktrees/<token>` allocation layout loses
    /// its parent component.
    #[allow(clippy::too_many_lines)] // one ordered Git acquisition pipeline
    pub async fn acquire(
        &self,
        policy: SubagentWorkspacePolicy,
        subagent_id: &SubagentId,
        cancellation: &CancellationSignal,
    ) -> Result<WorkspaceLease, WorkspaceAcquireError> {
        if cancellation.is_cancelled() {
            return Err(WorkspaceAcquireError::Cancelled);
        }
        if matches!(policy, SubagentWorkspacePolicy::SharedWorkspace) {
            return Ok(WorkspaceLease {
                manager: self.clone(),
                snapshot: WorkspaceSnapshot::shared(self.parent_logical_workspace.clone()),
                branch_created: false,
                created: false,
            });
        }
        let SubagentWorkspacePolicy::GitWorktree {
            require_clean_parent,
        } = policy
        else {
            unreachable!("the shared policy returned above")
        };

        let source_repository_root = self
            .git_text(
                &self.parent_logical_workspace,
                vec!["rev-parse".into(), "--show-toplevel".into()],
                Some(cancellation),
            )
            .await?;
        let source_repository_root =
            std::fs::canonicalize(source_repository_root).map_err(|error| {
                WorkspaceAcquireError::InvalidSnapshot {
                    detail: format!("cannot canonicalize the source repository root: {error}"),
                }
            })?;
        let canonical_parent_logical_workspace =
            std::fs::canonicalize(&self.parent_logical_workspace).map_err(|error| {
                WorkspaceAcquireError::InvalidSnapshot {
                    detail: format!("cannot canonicalize the parent logical workspace: {error}"),
                }
            })?;
        let repository_relative_workspace = canonical_parent_logical_workspace
            .strip_prefix(&source_repository_root)
            .map_err(|_| WorkspaceAcquireError::InvalidSnapshot {
                detail: format!(
                    "parent logical workspace {} is outside source repository root {}",
                    canonical_parent_logical_workspace.display(),
                    source_repository_root.display()
                ),
            })?
            .to_path_buf();
        if !is_safe_repository_relative(&repository_relative_workspace) {
            return Err(WorkspaceAcquireError::InvalidSnapshot {
                detail: "the derived repository-relative logical workspace is invalid".to_owned(),
            });
        }
        let base_commit = self
            .git_text(
                &self.parent_logical_workspace,
                vec!["rev-parse".into(), "HEAD".into()],
                Some(cancellation),
            )
            .await?;
        let parent_status = self
            .git_text(
                &self.parent_logical_workspace,
                ordinary_workspace_status_args(),
                Some(cancellation),
            )
            .await?;
        let parent_dirty = !parent_status.is_empty();
        if require_clean_parent && parent_dirty {
            return Err(WorkspaceAcquireError::DirtyParent { base_commit });
        }
        if cancellation.is_cancelled() {
            return Err(WorkspaceAcquireError::Cancelled);
        }
        let overlay_selection = self
            .resolve_overlay_selection(
                &canonical_parent_logical_workspace,
                &source_repository_root,
                &repository_relative_workspace,
                &base_commit,
                cancellation,
            )
            .await?;
        let frozen_overlay = freeze_overlay(overlay_selection, cancellation)?;
        #[cfg(test)]
        if let Some(hook) = &self.overlay_freeze_hook {
            hook.pause_after_freeze().await;
        }
        if cancellation.is_cancelled() {
            return Err(WorkspaceAcquireError::Cancelled);
        }

        let token = deterministic_worktree_name(subagent_id);
        let branch = format!("rustx/subagent/{token}");
        let physical_worktree_root = self.runtime_root.join("worktrees").join(token);
        if path_is_occupied(&physical_worktree_root)
            || self.branch_exists(&branch, cancellation).await?
        {
            return Err(WorkspaceAcquireError::Collision {
                physical_worktree_root,
                branch,
            });
        }
        std::fs::create_dir_all(
            physical_worktree_root
                .parent()
                .expect("the deterministic worktree path has a parent"),
        )
        .map_err(|error| WorkspaceAcquireError::Git {
            operation: "prepare worktree allocation root".to_owned(),
            detail: error.to_string(),
        })?;

        let child_logical_workspace = physical_worktree_root.join(&repository_relative_workspace);
        let snapshot = WorkspaceSnapshot::worktree(
            child_logical_workspace.clone(),
            source_repository_root,
            repository_relative_workspace,
            physical_worktree_root.clone(),
            base_commit.clone(),
            branch.clone(),
            parent_dirty,
        );
        let mut lease = WorkspaceLease {
            manager: self.clone(),
            snapshot,
            branch_created: false,
            created: false,
        };
        // `git -c core.hooksPath=/dev/null worktree add -b` is Git's atomic
        // branch-registration operation with checkout hooks suppressed. A
        // repository hook is outside the child's frozen execution authority
        // and could mutate the new path before the child owns it.
        // A losing same-identity acquisition must never infer ownership from
        // the path/ref that the winning command just created, so `created` is
        // set only when this command itself returned success *and* the exact
        // registration is present.
        let add = self
            .git_raw(
                &self.parent_logical_workspace,
                vec![
                    // Worktree creation performs a checkout, and Git runs
                    // repository checkout hooks for it. Disable hooks for
                    // this one native allocation command so the acquired
                    // bytes are exactly the selected commit.
                    "-c".into(),
                    "core.hooksPath=/dev/null".into(),
                    "worktree".into(),
                    "add".into(),
                    "-b".into(),
                    branch.clone().into(),
                    physical_worktree_root.clone().into_os_string(),
                    base_commit.clone().into(),
                ],
                Some(cancellation),
            )
            .await;
        let registered = self
            .worktree_registered(&lease.snapshot, true)
            .await
            .unwrap_or(false);
        if matches!(&add, Ok(output) if output.status.success()) {
            lease.branch_created = true;
            lease.created = registered;
        }
        let output = match add {
            Ok(output) if output.status.success() => output,
            Ok(output) => {
                let detail = git_failure_detail(&output);
                return Err(self
                    .settle_acquisition_failure(
                        lease,
                        WorkspaceAcquireError::Git {
                            operation: "worktree add".to_owned(),
                            detail,
                        },
                    )
                    .await);
            }
            Err(error) => {
                return Err(self.settle_acquisition_failure(lease, error).await);
            }
        };
        let _ = output;
        if !lease.created {
            return Err(self
                .settle_acquisition_failure(
                    lease,
                    WorkspaceAcquireError::InvalidSnapshot {
                        detail: "Git worktree add succeeded but did not register the exact path/ref/base"
                            .to_owned(),
                    },
                )
                .await);
        }
        if cancellation.is_cancelled() {
            return Err(self
                .settle_acquisition_failure(lease, WorkspaceAcquireError::Cancelled)
                .await);
        }
        let observed_head = match self
            .git_text(
                &physical_worktree_root,
                vec!["rev-parse".into(), "HEAD".into()],
                Some(cancellation),
            )
            .await
        {
            Ok(head) => head,
            Err(error) => return Err(self.settle_acquisition_failure(lease, error).await),
        };
        if observed_head != base_commit {
            return Err(self
                .settle_acquisition_failure(
                    lease,
                    WorkspaceAcquireError::InvalidSnapshot {
                        detail: format!("expected {base_commit}, observed {observed_head}"),
                    },
                )
                .await);
        }
        if !child_logical_workspace.is_dir() {
            return Err(self
                .settle_acquisition_failure(
                    lease,
                    WorkspaceAcquireError::InvalidSnapshot {
                        detail: format!(
                            "the committed checkout does not contain the preserved logical workspace {}",
                            child_logical_workspace.display()
                        ),
                    },
                )
                .await);
        }
        if let Err(error) = self
            .materialize_frozen_overlay(
                &lease.snapshot,
                &child_logical_workspace,
                &frozen_overlay,
                cancellation,
            )
            .await
        {
            return Err(self.settle_acquisition_failure(lease, error).await);
        }
        if let Err(detail) = lease.snapshot.validate() {
            return Err(self
                .settle_acquisition_failure(
                    lease,
                    WorkspaceAcquireError::InvalidSnapshot { detail },
                )
                .await);
        }
        #[cfg(test)]
        if let Some(hook) = &self.acquisition_hook {
            hook.pause_before_acquisition_return().await;
        }
        if cancellation.is_cancelled() {
            return Err(self
                .settle_acquisition_failure(lease, WorkspaceAcquireError::Cancelled)
                .await);
        }
        Ok(lease)
    }

    /// Resolves the one logical-workspace manifest and validates the complete
    /// selection before any selected file content is read.
    async fn resolve_overlay_selection(
        &self,
        parent_logical_workspace: &Path,
        source_repository_root: &Path,
        repository_relative_workspace: &Path,
        base_commit: &str,
        cancellation: &CancellationSignal,
    ) -> Result<Vec<ValidatedOverlayFile>, WorkspaceAcquireError> {
        let selected = read_overlay_manifest(parent_logical_workspace)?;
        let mut validated = Vec::with_capacity(selected.len());
        let mut canonical_sources = BTreeSet::new();
        for relative_path in selected {
            if cancellation.is_cancelled() {
                return Err(WorkspaceAcquireError::Cancelled);
            }
            let source_path = validate_overlay_source(parent_logical_workspace, &relative_path)?;
            let canonical_source = std::fs::canonicalize(&source_path).map_err(|error| {
                WorkspaceAcquireError::OverlayFreeze {
                    path: relative_path.clone(),
                    detail: error.to_string(),
                }
            })?;
            if !canonical_source.starts_with(parent_logical_workspace) {
                return Err(WorkspaceAcquireError::OverlayUnsafePath {
                    line: None,
                    path: relative_path.display().to_string(),
                });
            }
            if !canonical_sources.insert(canonical_source.clone()) {
                return Err(WorkspaceAcquireError::OverlayDuplicate {
                    path: relative_path,
                });
            }
            let repository_relative_path = repository_relative_workspace.join(&relative_path);
            if self
                .overlay_path_is_tracked(
                    source_repository_root,
                    base_commit,
                    &repository_relative_path,
                    cancellation,
                )
                .await?
            {
                return Err(WorkspaceAcquireError::OverlayTracked {
                    path: relative_path,
                });
            }
            if !self
                .overlay_path_is_ignored(
                    source_repository_root,
                    &repository_relative_path,
                    cancellation,
                )
                .await?
            {
                return Err(WorkspaceAcquireError::OverlayNotIgnored {
                    path: relative_path,
                });
            }
            validated.push(ValidatedOverlayFile {
                relative: relative_path,
                repository_relative: repository_relative_path,
                source: canonical_source,
            });
        }
        Ok(validated)
    }

    /// A path is tracked if either the current index or the exact captured
    /// commit contains it. Checking both keeps the explicit tracked-file
    /// rejection correct under the permissive dirty-parent policy too.
    async fn overlay_path_is_tracked(
        &self,
        source_repository_root: &Path,
        base_commit: &str,
        repository_relative_path: &Path,
        cancellation: &CancellationSignal,
    ) -> Result<bool, WorkspaceAcquireError> {
        let index = self
            .git_raw(
                source_repository_root,
                vec![
                    "--literal-pathspecs".into(),
                    "ls-files".into(),
                    "-z".into(),
                    "--".into(),
                    repository_relative_path.as_os_str().to_owned(),
                ],
                Some(cancellation),
            )
            .await?;
        if !index.status.success() {
            return Err(WorkspaceAcquireError::Git {
                operation: "inspect overlay index eligibility".to_owned(),
                detail: git_failure_detail(&index),
            });
        }
        if !index.stdout.is_empty() {
            return Ok(true);
        }
        let committed = self
            .git_raw(
                source_repository_root,
                vec![
                    "--literal-pathspecs".into(),
                    "ls-tree".into(),
                    "-r".into(),
                    "--name-only".into(),
                    "-z".into(),
                    base_commit.into(),
                    "--".into(),
                    repository_relative_path.as_os_str().to_owned(),
                ],
                Some(cancellation),
            )
            .await?;
        if !committed.status.success() {
            return Err(WorkspaceAcquireError::Git {
                operation: "inspect captured overlay source eligibility".to_owned(),
                detail: git_failure_detail(&committed),
            });
        }
        Ok(!committed.stdout.is_empty())
    }

    async fn overlay_path_is_ignored(
        &self,
        repository_root: &Path,
        repository_relative_path: &Path,
        cancellation: &CancellationSignal,
    ) -> Result<bool, WorkspaceAcquireError> {
        let output = self
            .git_raw(
                repository_root,
                vec![
                    "check-ignore".into(),
                    "--quiet".into(),
                    "--no-index".into(),
                    "--".into(),
                    repository_relative_path.as_os_str().to_owned(),
                ],
                Some(cancellation),
            )
            .await?;
        match output.status.code() {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => Err(WorkspaceAcquireError::Git {
                operation: "inspect overlay ignore eligibility".to_owned(),
                detail: git_failure_detail(&output),
            }),
        }
    }

    /// Revalidates ignored eligibility in the exact checkout, materializes
    /// only frozen bytes, and reads every destination back before returning.
    async fn materialize_frozen_overlay(
        &self,
        snapshot: &WorkspaceSnapshot,
        child_logical_workspace: &Path,
        frozen: &[FrozenOverlayFile],
        cancellation: &CancellationSignal,
    ) -> Result<(), WorkspaceAcquireError> {
        let physical_worktree_root = snapshot
            .git_worktree()
            .expect("isolated acquisition has Git worktree facts")
            .physical_worktree_root
            .as_path();
        for file in frozen {
            if !self
                .overlay_path_is_ignored(
                    physical_worktree_root,
                    &file.repository_relative_path,
                    cancellation,
                )
                .await?
            {
                return Err(WorkspaceAcquireError::OverlayNotIgnored {
                    path: file.relative_path.clone(),
                });
            }
            validate_overlay_destination(child_logical_workspace, &file.relative_path)?;
        }
        for file in frozen {
            if cancellation.is_cancelled() {
                return Err(WorkspaceAcquireError::Cancelled);
            }
            let destination = child_logical_workspace.join(&file.relative_path);
            create_overlay_parent_directories(
                child_logical_workspace,
                &file.relative_path,
                &file.relative_path,
            )?;
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            let mut output = options.open(&destination).map_err(|error| {
                WorkspaceAcquireError::OverlayMaterialization {
                    path: file.relative_path.clone(),
                    detail: error.to_string(),
                }
            })?;
            std::io::Write::write_all(&mut output, &file.bytes).map_err(|error| {
                WorkspaceAcquireError::OverlayMaterialization {
                    path: file.relative_path.clone(),
                    detail: error.to_string(),
                }
            })?;
            std::io::Write::flush(&mut output).map_err(|error| {
                WorkspaceAcquireError::OverlayMaterialization {
                    path: file.relative_path.clone(),
                    detail: error.to_string(),
                }
            })?;
        }
        for file in frozen {
            if cancellation.is_cancelled() {
                return Err(WorkspaceAcquireError::Cancelled);
            }
            let destination = child_logical_workspace.join(&file.relative_path);
            let observed = std::fs::read(&destination).map_err(|error| {
                WorkspaceAcquireError::OverlayMaterialization {
                    path: file.relative_path.clone(),
                    detail: error.to_string(),
                }
            })?;
            if observed != file.bytes {
                return Err(WorkspaceAcquireError::OverlayMaterialization {
                    path: file.relative_path.clone(),
                    detail: "destination bytes do not match the frozen source".to_owned(),
                });
            }
        }
        Ok(())
    }

    async fn settle_acquisition_failure(
        &self,
        lease: WorkspaceLease,
        error: WorkspaceAcquireError,
    ) -> WorkspaceAcquireError {
        match lease.settle_staged().await {
            Ok(_) => error,
            Err(settlement) => WorkspaceAcquireError::Settlement {
                detail: format!("{error}; {}", settlement.detail),
            },
        }
    }

    async fn branch_exists(
        &self,
        branch: &str,
        cancellation: &CancellationSignal,
    ) -> Result<bool, WorkspaceAcquireError> {
        let reference = format!("refs/heads/{branch}");
        let output = self
            .git_raw(
                &self.parent_logical_workspace,
                vec![
                    "show-ref".into(),
                    "--verify".into(),
                    "--quiet".into(),
                    reference.clone().into(),
                ],
                Some(cancellation),
            )
            .await?;
        match output.status.code() {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => Err(WorkspaceAcquireError::Git {
                operation: "inspect deterministic worktree branch".to_owned(),
                detail: git_failure_detail(&output),
            }),
        }
    }

    /// Proves that the exact deterministic path/ref is registered as a Git
    /// worktree at the selected base. This is used after an interrupted or
    /// failed `worktree add`: filesystem existence alone is not ownership
    /// proof and must never authorize cleanup of a concurrent/foreign path.
    async fn worktree_registered(
        &self,
        snapshot: &WorkspaceSnapshot,
        require_base: bool,
    ) -> Result<bool, String> {
        let listing = self
            .git_text(
                &self.parent_logical_workspace,
                vec!["worktree".into(), "list".into(), "--porcelain".into()],
                None,
            )
            .await
            .map_err(|error| error.to_string())?;
        Ok(worktree_listing_contains(&listing, snapshot, require_base))
    }

    async fn git_text(
        &self,
        cwd: &Path,
        args: Vec<OsString>,
        cancellation: Option<&CancellationSignal>,
    ) -> Result<String, WorkspaceAcquireError> {
        let output = self.git_raw(cwd, args, cancellation).await?;
        if !output.status.success() {
            return Err(WorkspaceAcquireError::Git {
                operation: "command".to_owned(),
                detail: git_failure_detail(&output),
            });
        }
        let text =
            String::from_utf8(output.stdout).map_err(|error| WorkspaceAcquireError::Git {
                operation: "decode Git output".to_owned(),
                detail: error.to_string(),
            })?;
        Ok(text.trim_end_matches(['\r', '\n']).to_owned())
    }

    async fn git_raw(
        &self,
        cwd: &Path,
        args: Vec<OsString>,
        cancellation: Option<&CancellationSignal>,
    ) -> Result<GitOutput, WorkspaceAcquireError> {
        let mut command = Command::new("git");
        command
            .arg("-C")
            .arg(cwd)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        let child = command
            .spawn()
            .map_err(|error| WorkspaceAcquireError::Git {
                operation: "spawn git".to_owned(),
                detail: error.to_string(),
            })?;
        let child_id = child.id();
        // `Command::output()` owns the child inside an opaque future. That
        // makes cancellation return before we can prove the Git mutation
        // process has exited. Keep the child in a dedicated waiter instead;
        // cancellation kills its private process group and awaits that same
        // waiter before any workspace settlement can inspect or remove paths.
        let mut wait_handle = tokio::spawn(async move { child.wait_with_output().await });
        let wait_result = if let Some(cancellation) = cancellation {
            tokio::select! {
                biased;
                () = cancellation.cancelled() => {
                    kill_git_process_group(child_id);
                    let _ = (&mut wait_handle).await;
                    return Err(WorkspaceAcquireError::Cancelled);
                }
                output = &mut wait_handle => output,
            }
        } else {
            wait_handle.await
        };
        let output = wait_result
            .map_err(|error| WorkspaceAcquireError::Git {
                operation: "wait for git".to_owned(),
                detail: error.to_string(),
            })?
            .map_err(|error| WorkspaceAcquireError::Git {
                operation: "run git".to_owned(),
                detail: error.to_string(),
            })?;
        Ok(GitOutput {
            status: output.status,
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }

    /// Inspects a worktree recorded by durable ownership after a parent
    /// restart.  Recovery deliberately never removes it: the restarted
    /// process has no direct-child/nested-anchor proof, so preserving the
    /// exact path is the only safe disposition.
    #[must_use]
    pub fn inspect_recovered(snapshot: &WorkspaceSnapshot) -> WorkspaceSettlement {
        if let Err(error) = snapshot.validate() {
            return WorkspaceSettlement::unresolved(snapshot.clone(), error);
        }
        let Some(worktree) = snapshot.git_worktree() else {
            return WorkspaceSettlement::shared(snapshot.clone());
        };
        let head = run_git_sync(&worktree.physical_worktree_root, &["rev-parse", "HEAD"]);
        let status = run_git_sync(
            &worktree.physical_worktree_root,
            &ORDINARY_WORKSPACE_STATUS_ARGS,
        );
        let (head, status) = match (head, status) {
            (Ok(head), Ok(status)) => (head, status),
            (Err(error), _) | (_, Err(error)) => {
                return WorkspaceSettlement::unresolved(snapshot.clone(), error);
            }
        };
        let branch = worktree.branch.clone();
        let base_commit = worktree.base_commit.clone();
        let listing = match run_git_sync(
            &worktree.source_repository_root,
            &["worktree", "list", "--porcelain"],
        ) {
            Ok(listing) => listing,
            Err(error) => return WorkspaceSettlement::unresolved(snapshot.clone(), error),
        };
        if !worktree_listing_contains(&listing, snapshot, false) {
            return WorkspaceSettlement::unresolved(
                snapshot.clone(),
                format!(
                    "recovered worktree {} is not registered in the owned repository {}",
                    worktree.physical_worktree_root.display(),
                    worktree.source_repository_root.display()
                ),
            );
        }
        let reference = format!("refs/heads/{branch}");
        let branch_head = match run_git_sync(
            &worktree.source_repository_root,
            &["rev-parse", "--verify", &reference],
        ) {
            Ok(branch_head) => branch_head,
            Err(error) => return WorkspaceSettlement::unresolved(snapshot.clone(), error),
        };
        if branch_head != head {
            return WorkspaceSettlement::unresolved(
                snapshot.clone(),
                format!(
                    "recovered branch {branch} points at {branch_head}, but the worktree HEAD is {head}"
                ),
            );
        }
        let (dirty, _) = workspace_change_facts(Some(&worktree.base_commit), &head, &status);
        WorkspaceSettlement {
            snapshot: snapshot.clone(),
            handoff: Some(WorkspaceHandoff {
                logical_workspace: snapshot.logical_workspace.clone(),
                physical_worktree_root: worktree.physical_worktree_root.clone(),
                branch,
                base_commit,
                head_commit: head,
                dirty,
            }),
            cleanup: WorkspaceCleanup::Preserved,
            error: None,
        }
    }
}

/// A lease is the explicit physical owner of one workspace selection.
///
/// It has no destructive `Drop` behavior.  The owner must call one of the
/// consuming settlement methods; if an unexpected panic/drop abandons it, the
/// conservative result is an on-disk worktree that remains available for
/// recovery, never a force-removal of unknown user work.
#[derive(Debug)]
pub struct WorkspaceLease {
    manager: SubagentWorkspaceManager,
    snapshot: WorkspaceSnapshot,
    /// Proof that this lease's successful atomic `worktree add -b` created the
    /// deterministic runtime branch. An ambiguous/cancelled Git command does
    /// not set this bit, so settlement cannot delete a concurrent owner's ref.
    branch_created: bool,
    /// Proof that the branch is registered at the exact lease path/base.
    created: bool,
}

impl WorkspaceLease {
    /// The authoritative logical child project workspace path.
    #[must_use]
    pub fn logical_workspace(&self) -> &Path {
        &self.snapshot.logical_workspace
    }

    /// The runtime-owned physical worktree root, when this lease is isolated.
    #[must_use]
    pub fn physical_worktree_root(&self) -> Option<&Path> {
        self.snapshot
            .git_worktree()
            .map(|worktree| worktree.physical_worktree_root.as_path())
    }

    /// The immutable selection facts.
    #[must_use]
    pub fn snapshot(&self) -> &WorkspaceSnapshot {
        &self.snapshot
    }

    /// Settles a child after direct process and nested-unit physical
    /// settlement. The caller must invoke this only after nested containment
    /// is proven; unresolved nested ownership uses
    /// [`Self::preserve_after_unresolved_nested`] instead.
    pub(crate) async fn settle_after_child(self) -> WorkspaceSettlement {
        self.settle().await
    }

    /// Preserves a lease without claiming final Git facts when a retained
    /// nested process anchor could not be proven physically settled. The
    /// physical worktree root remains the conservative recovery authority; a
    /// later recovery inspection can observe the final state once no process
    /// may mutate it.
    pub(crate) fn preserve_after_unresolved_nested(
        self,
        detail: impl Into<String>,
    ) -> WorkspaceSettlement {
        WorkspaceSettlement::unresolved(self.snapshot, detail)
    }

    /// Settles a lease that never crossed durable child ownership.  Any dirty
    /// or otherwise unproven state is returned as an error and retained.
    pub(crate) async fn settle_staged(
        self,
    ) -> Result<WorkspaceSettlement, WorkspaceSettlementError> {
        let settlement = self.settle().await;
        if settlement.handoff.is_some() || settlement.error.is_some() {
            let detail = settlement.error.clone().unwrap_or_else(|| {
                "staged workspace is dirty or has a committed child change; it was preserved"
                    .to_owned()
            });
            return Err(WorkspaceSettlementError {
                detail,
                settlement: Box::new(settlement),
            });
        }
        Ok(settlement)
    }

    async fn settle(self) -> WorkspaceSettlement {
        if !self.snapshot.is_isolated() {
            return WorkspaceSettlement::shared(self.snapshot);
        }
        if !self.created {
            return self.settle_unregistered().await;
        }
        self.settle_registered().await
    }

    async fn settle_unregistered(self) -> WorkspaceSettlement {
        let snapshot = self.snapshot;
        let Some(worktree) = snapshot.git_worktree() else {
            return WorkspaceSettlement::unresolved(
                snapshot,
                "an unregistered isolated lease has no physical worktree facts",
            );
        };
        if path_is_occupied(&worktree.physical_worktree_root) {
            return WorkspaceSettlement::unresolved(
                snapshot,
                "the deterministic physical worktree root exists but is not a proven Git worktree owned by this lease",
            );
        }
        if self.branch_created
            && let Err(error) = self.manager.remove_runtime_branch(&snapshot).await
        {
            return WorkspaceSettlement::unresolved(snapshot, error);
        }
        WorkspaceSettlement {
            snapshot,
            handoff: None,
            cleanup: WorkspaceCleanup::Removed,
            error: None,
        }
    }

    async fn settle_registered(self) -> WorkspaceSettlement {
        let snapshot = self.snapshot.clone();
        #[cfg(test)]
        if let Some(hook) = &self.manager.settlement_hook
            && let Some(settlement) = hook.maybe_fail(&snapshot).await
        {
            return settlement;
        }
        let Some(worktree) = snapshot.git_worktree() else {
            return WorkspaceSettlement::unresolved(
                snapshot,
                "a registered isolated lease has no physical worktree facts",
            );
        };
        let head = match self
            .manager
            .git_text(
                &worktree.physical_worktree_root,
                vec!["rev-parse".into(), "HEAD".into()],
                None,
            )
            .await
        {
            Ok(head) => head,
            Err(error) => return WorkspaceSettlement::unresolved(snapshot, error.to_string()),
        };
        let status = match self
            .manager
            .git_text(
                &worktree.physical_worktree_root,
                ordinary_workspace_status_args(),
                None,
            )
            .await
        {
            Ok(status) => status,
            Err(error) => return WorkspaceSettlement::unresolved(snapshot, error.to_string()),
        };
        if !self
            .manager
            .worktree_registered(&snapshot, false)
            .await
            .unwrap_or(false)
        {
            return WorkspaceSettlement::unresolved(
                snapshot,
                "the leased path is no longer registered as the runtime-created Git worktree",
            );
        }
        let branch = worktree.branch.clone();
        let branch_head = match self.manager.runtime_branch_head(&branch).await {
            Ok(branch_head) => branch_head,
            Err(error) => return WorkspaceSettlement::unresolved(snapshot, error),
        };
        if branch_head != head {
            return WorkspaceSettlement::unresolved(
                snapshot,
                format!(
                    "runtime-created branch {branch} points at {branch_head}, but the worktree HEAD is {head}"
                ),
            );
        }
        let (dirty, changed) = workspace_change_facts(Some(&worktree.base_commit), &head, &status);
        let handoff = WorkspaceHandoff {
            logical_workspace: snapshot.logical_workspace.clone(),
            physical_worktree_root: worktree.physical_worktree_root.clone(),
            branch,
            base_commit: worktree.base_commit.clone(),
            head_commit: head.clone(),
            dirty,
        };
        if changed {
            return WorkspaceSettlement {
                snapshot,
                handoff: Some(handoff),
                cleanup: WorkspaceCleanup::Preserved,
                error: None,
            };
        }
        match self.manager.remove_clean_worktree(&snapshot).await {
            Ok(()) => WorkspaceSettlement {
                snapshot,
                handoff: None,
                cleanup: WorkspaceCleanup::Removed,
                error: None,
            },
            Err(error) => WorkspaceSettlement {
                snapshot,
                handoff: Some(handoff),
                cleanup: WorkspaceCleanup::Preserved,
                error: Some(error),
            },
        }
    }
}

impl SubagentWorkspaceManager {
    async fn runtime_branch_head(&self, branch: &str) -> Result<String, String> {
        let reference = format!("refs/heads/{branch}");
        self.git_text(
            &self.parent_logical_workspace,
            vec!["rev-parse".into(), "--verify".into(), reference.into()],
            None,
        )
        .await
        .map_err(|error| error.to_string())
    }

    async fn remove_clean_worktree(&self, snapshot: &WorkspaceSnapshot) -> Result<(), String> {
        let worktree = snapshot
            .git_worktree()
            .ok_or_else(|| "shared workspace has no physical worktree to remove".to_owned())?;
        let output = self
            .git_raw(
                &self.parent_logical_workspace,
                vec![
                    "worktree".into(),
                    "remove".into(),
                    "--".into(),
                    worktree.physical_worktree_root.clone().into_os_string(),
                ],
                None,
            )
            .await
            .map_err(|error| error.to_string())?;
        if !output.status.success() {
            return Err(format!(
                "Git worktree remove failed: {}",
                git_failure_detail(&output)
            ));
        }
        self.remove_runtime_branch(snapshot).await
    }

    /// Removes a runtime-created ref only after proving that it still points
    /// at the exact selected base. This permits cleanup when the parent moved
    /// to a branch where Git would reject `branch -d`, without ever deleting
    /// a ref that acquired child work or was changed by another owner.
    async fn remove_runtime_branch(&self, snapshot: &WorkspaceSnapshot) -> Result<(), String> {
        let Some(worktree) = snapshot.git_worktree() else {
            return Ok(());
        };
        let branch = &worktree.branch;
        let reference = format!("refs/heads/{branch}");
        let exists = self
            .git_raw(
                &self.parent_logical_workspace,
                vec![
                    "show-ref".into(),
                    "--verify".into(),
                    "--quiet".into(),
                    reference.clone().into(),
                ],
                None,
            )
            .await
            .map_err(|error| error.to_string())?;
        if !exists.status.success() {
            if exists.status.code() == Some(1) {
                return Ok(());
            }
            return Err(format!(
                "Git branch lookup failed: {}",
                git_failure_detail(&exists)
            ));
        }
        let base_commit = worktree.base_commit.as_str();
        let current = self
            .git_text(
                &self.parent_logical_workspace,
                vec![
                    "rev-parse".into(),
                    "--verify".into(),
                    reference.clone().into(),
                ],
                None,
            )
            .await
            .map_err(|error| error.to_string())?;
        if current != base_commit {
            return Err(format!(
                "runtime-created branch {branch} moved from base {base_commit} to {current}"
            ));
        }
        // `git branch -d` rejects an unmerged branch when the parent moved
        // away from the selected base. The ref is still runtime-owned, and
        // the exact base/hash proof above means deleting it cannot discard
        // child work. `update-ref -d <ref> <old-value>` also makes the
        // deletion compare-and-delete rather than a blind ref overwrite.
        let deleted = self
            .git_raw(
                &self.parent_logical_workspace,
                vec![
                    "update-ref".into(),
                    "-d".into(),
                    reference.into(),
                    base_commit.into(),
                ],
                None,
            )
            .await
            .map_err(|error| error.to_string())?;
        if deleted.status.success() {
            Ok(())
        } else {
            Err(format!(
                "Git branch cleanup failed: {}",
                git_failure_detail(&deleted)
            ))
        }
    }
}

/// The one Git status definition used by live and recovery workspace
/// settlement. Git's ordinary porcelain status reports tracked/index deltas
/// and untracked non-ignored files; `--ignored=matching` is deliberately not
/// present because execution caches are not handoff work by themselves.
const ORDINARY_WORKSPACE_STATUS_ARGS: [&str; 3] =
    ["status", "--porcelain=v1", "--untracked-files=all"];

fn ordinary_workspace_status_args() -> Vec<OsString> {
    ORDINARY_WORKSPACE_STATUS_ARGS
        .iter()
        .map(OsString::from)
        .collect()
}

/// Returns the two independent terminal Git facts shared by live settlement
/// and recovery inspection. Ordinary status determines `dirty`; committed
/// child work is retained separately when `HEAD != base_commit`.
fn workspace_change_facts(
    base_commit: Option<&str>,
    head_commit: &str,
    status: &str,
) -> (bool, bool) {
    let dirty = !status.is_empty();
    let changed = dirty || base_commit != Some(head_commit);
    (dirty, changed)
}

/// Stable, race-independent runtime worktree identity derived from the
/// durable subagent identity and a versioned namespace.
#[must_use]
pub fn deterministic_worktree_name(subagent_id: &SubagentId) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"rustx-subagent-worktree-v1\n");
    hasher.update(subagent_id.as_str().len().to_string().as_bytes());
    hasher.update(b"\n");
    hasher.update(subagent_id.as_str().as_bytes());
    format!("{:x}", hasher.finalize())
}

struct GitOutput {
    status: std::process::ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

fn git_failure_detail(output: &GitOutput) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if stderr.is_empty() {
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if stdout.is_empty() {
            format!("exit status {}", output.status)
        } else {
            stdout
        }
    } else {
        stderr
    }
}

fn run_git_sync(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .map_err(|error| format!("cannot spawn Git for {}: {error}", cwd.display()))?;
    if !output.status.success() {
        return Err(format!(
            "Git {} failed: {}",
            args.join(" "),
            git_sync_detail(&output)
        ));
    }
    String::from_utf8(output.stdout)
        .map(|text| text.trim_end_matches(['\r', '\n']).to_owned())
        .map_err(|error| format!("Git {} returned invalid UTF-8: {error}", args.join(" ")))
}

fn git_sync_detail(output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if stderr.is_empty() {
        format!("exit status {}", output.status)
    } else {
        stderr
    }
}

fn path_is_occupied(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

/// Reads the already validated selection into the immutable, bounded
/// acquisition representation. A second metadata check catches replacement
/// with a symlink or non-file between validation and the actual freeze.
fn freeze_overlay(
    validated: Vec<ValidatedOverlayFile>,
    cancellation: &CancellationSignal,
) -> Result<Vec<FrozenOverlayFile>, WorkspaceAcquireError> {
    let mut total_bytes = 0_usize;
    let mut frozen = Vec::with_capacity(validated.len());
    for file in validated {
        if cancellation.is_cancelled() {
            return Err(WorkspaceAcquireError::Cancelled);
        }
        let metadata = std::fs::symlink_metadata(&file.source)
            .map_err(|error| overlay_source_metadata_error(&file.relative, &error))?;
        if metadata.file_type().is_symlink() {
            return Err(WorkspaceAcquireError::OverlaySymlink {
                path: file.relative,
            });
        }
        if !metadata.is_file() {
            return Err(WorkspaceAcquireError::OverlayNotFile {
                path: file.relative,
            });
        }
        let metadata_bytes = usize::try_from(metadata.len()).unwrap_or(usize::MAX);
        if total_bytes
            .checked_add(metadata_bytes)
            .is_none_or(|bytes| bytes > MAX_OVERLAY_BYTES)
        {
            return Err(WorkspaceAcquireError::OverlayByteLimit {
                limit: MAX_OVERLAY_BYTES,
            });
        }
        let source = open_overlay_source(&file.source).map_err(|error| {
            WorkspaceAcquireError::OverlayFreeze {
                path: file.relative.clone(),
                detail: error.to_string(),
            }
        })?;
        let opened_metadata =
            source
                .metadata()
                .map_err(|error| WorkspaceAcquireError::OverlayFreeze {
                    path: file.relative.clone(),
                    detail: error.to_string(),
                })?;
        if !opened_metadata.is_file() {
            return Err(WorkspaceAcquireError::OverlayNotFile {
                path: file.relative,
            });
        }
        let remaining = MAX_OVERLAY_BYTES - total_bytes;
        let read_limit = u64::try_from(remaining.saturating_add(1)).unwrap_or(u64::MAX);
        let mut bytes = Vec::with_capacity(metadata_bytes.min(remaining));
        std::io::Read::read_to_end(&mut source.take(read_limit), &mut bytes).map_err(|error| {
            WorkspaceAcquireError::OverlayFreeze {
                path: file.relative.clone(),
                detail: error.to_string(),
            }
        })?;
        total_bytes = total_bytes.checked_add(bytes.len()).ok_or(
            WorkspaceAcquireError::OverlayByteLimit {
                limit: MAX_OVERLAY_BYTES,
            },
        )?;
        if total_bytes > MAX_OVERLAY_BYTES {
            return Err(WorkspaceAcquireError::OverlayByteLimit {
                limit: MAX_OVERLAY_BYTES,
            });
        }
        frozen.push(FrozenOverlayFile {
            relative_path: file.relative,
            repository_relative_path: file.repository_relative,
            bytes,
        });
    }
    Ok(frozen)
}

fn open_overlay_source(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    options.open(path)
}

/// Reads and parses `<logical-workspace>/.worktreeinclude`. A missing
/// manifest is the one no-overlay representation. Each nonblank,
/// non-comment line is one exact file path; surrounding whitespace is not
/// part of the path, `#` starts a comment only as the first trimmed byte, and
/// there is no glob, negation, escaping, or directory syntax in v1.
fn read_overlay_manifest(
    parent_logical_workspace: &Path,
) -> Result<Vec<PathBuf>, WorkspaceAcquireError> {
    let manifest = parent_logical_workspace.join(WORKTREE_INCLUDE_MANIFEST);
    let metadata = match std::fs::symlink_metadata(&manifest) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(WorkspaceAcquireError::OverlayManifest {
                detail: error.to_string(),
            });
        }
    };
    if metadata.file_type().is_symlink() {
        return Err(WorkspaceAcquireError::OverlayManifest {
            detail: "the manifest must not be a symlink".to_owned(),
        });
    }
    if !metadata.is_file() {
        return Err(WorkspaceAcquireError::OverlayManifest {
            detail: "the manifest is not a regular file".to_owned(),
        });
    }
    if metadata.len() > MAX_OVERLAY_MANIFEST_BYTES {
        return Err(WorkspaceAcquireError::OverlayManifest {
            detail: format!(
                "the manifest exceeds the fixed limit of {MAX_OVERLAY_MANIFEST_BYTES} bytes"
            ),
        });
    }
    let source =
        open_overlay_source(&manifest).map_err(|error| WorkspaceAcquireError::OverlayManifest {
            detail: error.to_string(),
        })?;
    if !source
        .metadata()
        .map_err(|error| WorkspaceAcquireError::OverlayManifest {
            detail: error.to_string(),
        })?
        .is_file()
    {
        return Err(WorkspaceAcquireError::OverlayManifest {
            detail: "the manifest is not a regular file".to_owned(),
        });
    }
    let mut bytes = Vec::with_capacity(
        usize::try_from(metadata.len())
            .unwrap_or(usize::MAX)
            .min(usize::try_from(MAX_OVERLAY_MANIFEST_BYTES).unwrap_or(usize::MAX)),
    );
    std::io::Read::read_to_end(&mut source.take(MAX_OVERLAY_MANIFEST_BYTES + 1), &mut bytes)
        .map_err(|error| WorkspaceAcquireError::OverlayManifest {
            detail: error.to_string(),
        })?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_OVERLAY_MANIFEST_BYTES {
        return Err(WorkspaceAcquireError::OverlayManifest {
            detail: format!(
                "the manifest exceeds the fixed limit of {MAX_OVERLAY_MANIFEST_BYTES} bytes"
            ),
        });
    }
    let contents =
        std::str::from_utf8(&bytes).map_err(|error| WorkspaceAcquireError::OverlayManifest {
            detail: format!("the manifest is not UTF-8: {error}"),
        })?;
    parse_overlay_manifest(contents)
}

fn parse_overlay_manifest(contents: &str) -> Result<Vec<PathBuf>, WorkspaceAcquireError> {
    let mut selected = Vec::new();
    let mut destinations = BTreeSet::new();
    for (index, line) in contents.lines().enumerate() {
        let entry = line.trim();
        if entry.is_empty() || entry.starts_with('#') {
            continue;
        }
        let input = Path::new(entry);
        if entry.as_bytes().contains(&0)
            || input.is_absolute()
            || !is_safe_repository_relative(input)
        {
            return Err(WorkspaceAcquireError::OverlayUnsafePath {
                line: Some(index + 1),
                path: entry.to_owned(),
            });
        }
        let normalized = input.components().collect::<PathBuf>();
        if !destinations.insert(normalized.clone()) {
            return Err(WorkspaceAcquireError::OverlayDuplicate { path: normalized });
        }
        selected.push(normalized);
        if selected.len() > MAX_OVERLAY_FILES {
            return Err(WorkspaceAcquireError::OverlayFileLimit {
                limit: MAX_OVERLAY_FILES,
            });
        }
    }
    Ok(selected)
}

/// Validates every existing source component without following symlinks.
/// This happens for the complete selection before any selected contents are
/// read, so validation and freezing remain visibly separate phases.
fn validate_overlay_source(
    parent_logical_workspace: &Path,
    relative_path: &Path,
) -> Result<PathBuf, WorkspaceAcquireError> {
    let components = relative_path.components().collect::<Vec<_>>();
    let mut current = parent_logical_workspace.to_path_buf();
    for (index, component) in components.iter().enumerate() {
        current.push(component.as_os_str());
        let metadata = std::fs::symlink_metadata(&current)
            .map_err(|error| overlay_source_metadata_error(relative_path, &error))?;
        if metadata.file_type().is_symlink() {
            return Err(WorkspaceAcquireError::OverlaySymlink {
                path: relative_path.to_path_buf(),
            });
        }
        let final_component = index + 1 == components.len();
        if (final_component && !metadata.is_file()) || (!final_component && !metadata.is_dir()) {
            return Err(WorkspaceAcquireError::OverlayNotFile {
                path: relative_path.to_path_buf(),
            });
        }
    }
    Ok(current)
}

fn overlay_source_metadata_error(path: &Path, error: &std::io::Error) -> WorkspaceAcquireError {
    if error.kind() == ErrorKind::NotFound {
        WorkspaceAcquireError::OverlayMissing {
            path: path.to_path_buf(),
        }
    } else {
        WorkspaceAcquireError::OverlayFreeze {
            path: path.to_path_buf(),
            detail: error.to_string(),
        }
    }
}

/// Checks the committed checkout before any overlay destination is created.
/// Existing symlinks and non-directory ancestors fail closed; the final path
/// must be absent because overlays never overwrite Git or hook output.
fn validate_overlay_destination(
    child_logical_workspace: &Path,
    relative_path: &Path,
) -> Result<(), WorkspaceAcquireError> {
    let components = relative_path.components().collect::<Vec<_>>();
    let mut current = child_logical_workspace.to_path_buf();
    for (index, component) in components.iter().enumerate() {
        current.push(component.as_os_str());
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    return Err(WorkspaceAcquireError::OverlayMaterialization {
                        path: relative_path.to_path_buf(),
                        detail: "the destination contains a symlink".to_owned(),
                    });
                }
                let final_component = index + 1 == components.len();
                if final_component || !metadata.is_dir() {
                    return Err(WorkspaceAcquireError::OverlayMaterialization {
                        path: relative_path.to_path_buf(),
                        detail: "the destination already exists or has a non-directory parent"
                            .to_owned(),
                    });
                }
            }
            Err(error) if error.kind() == ErrorKind::NotFound => break,
            Err(error) => {
                return Err(WorkspaceAcquireError::OverlayMaterialization {
                    path: relative_path.to_path_buf(),
                    detail: error.to_string(),
                });
            }
        }
    }
    Ok(())
}

fn create_overlay_parent_directories(
    child_logical_workspace: &Path,
    relative_path: &Path,
    selected_path: &Path,
) -> Result<(), WorkspaceAcquireError> {
    let mut current = child_logical_workspace.to_path_buf();
    let Some(parent) = relative_path.parent() else {
        return Ok(());
    };
    for component in parent.components() {
        current.push(component.as_os_str());
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Ok(_) => {
                return Err(WorkspaceAcquireError::OverlayMaterialization {
                    path: selected_path.to_path_buf(),
                    detail: "the destination has a symlink or non-directory parent".to_owned(),
                });
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {
                std::fs::create_dir(&current).map_err(|error| {
                    WorkspaceAcquireError::OverlayMaterialization {
                        path: selected_path.to_path_buf(),
                        detail: error.to_string(),
                    }
                })?;
            }
            Err(error) => {
                return Err(WorkspaceAcquireError::OverlayMaterialization {
                    path: selected_path.to_path_buf(),
                    detail: error.to_string(),
                });
            }
        }
    }
    Ok(())
}

/// A repository-relative logical workspace is either the empty root path or
/// a sequence of ordinary components. Absolute roots, platform prefixes, and
/// parent traversal would make `physical_worktree_root.join(relative)` an
/// authority-widening or escaping operation and are rejected at every decoded
/// boundary.
fn is_safe_repository_relative(path: &Path) -> bool {
    path.components()
        .all(|component| matches!(component, std::path::Component::Normal(_)))
}

fn kill_git_process_group(child_id: Option<u32>) {
    #[cfg(unix)]
    if let Some(child_id) = child_id.and_then(|id| i32::try_from(id).ok()) {
        let _ = nix::sys::signal::killpg(
            nix::unistd::Pid::from_raw(child_id),
            nix::sys::signal::Signal::SIGKILL,
        );
    }
    #[cfg(not(unix))]
    let _ = child_id;
}

/// Checks one `git worktree list --porcelain` entry against an immutable
/// workspace snapshot. Recovered inspection accepts any final `HEAD` because
/// a committed child is expected to move it; acquisition requires the exact
/// selected base.
fn worktree_listing_contains(
    listing: &str,
    snapshot: &WorkspaceSnapshot,
    require_base: bool,
) -> bool {
    let mut path = None;
    let mut head = None;
    let mut branch = None;
    for line in listing.lines().chain(std::iter::once("")) {
        if let Some(value) = line.strip_prefix("worktree ") {
            path = Some(PathBuf::from(value));
            head = None;
            branch = None;
            continue;
        }
        if let Some(value) = line.strip_prefix("HEAD ") {
            head = Some(value);
            continue;
        }
        if let Some(value) = line.strip_prefix("branch ") {
            branch = Some(value);
            continue;
        }
        if worktree_listing_entry_matches(path.as_deref(), head, branch, snapshot, require_base) {
            return true;
        }
        path = None;
        head = None;
        branch = None;
    }
    false
}

fn worktree_listing_entry_matches(
    path: Option<&Path>,
    head: Option<&str>,
    branch: Option<&str>,
    snapshot: &WorkspaceSnapshot,
    require_base: bool,
) -> bool {
    let Some(worktree) = snapshot.git_worktree() else {
        return false;
    };
    let expected_branch = format!("refs/heads/{}", worktree.branch);
    paths_refer_to_same_worktree(path, Some(worktree.physical_worktree_root.as_path()))
        && branch == Some(expected_branch.as_str())
        && (!require_base || head == Some(worktree.base_commit.as_str()))
}

/// Git may report a macOS temporary-directory path through its canonical
/// `/private` spelling even when the caller supplied the equivalent `/var`
/// spelling (and the inverse can occur for other symlinked system roots).
/// Compare the lexical form first, then the canonical physical path. This
/// preserves exact lease ownership while avoiding a platform-specific false
/// negative in the Git registration proof.
fn paths_refer_to_same_worktree(actual: Option<&Path>, expected: Option<&Path>) -> bool {
    let (Some(actual), Some(expected)) = (actual, expected) else {
        return actual == expected;
    };
    actual == expected
        || actual
            .canonicalize()
            .ok()
            .zip(expected.canonicalize().ok())
            .is_some_and(|(actual, expected)| actual == expected)
}

#[cfg(test)]
#[derive(Debug)]
pub(crate) struct WorkspaceAcquireHook {
    after_creation: tokio::sync::Barrier,
    release: tokio::sync::Barrier,
}

#[cfg(test)]
impl WorkspaceAcquireHook {
    fn new() -> Self {
        Self {
            after_creation: tokio::sync::Barrier::new(2),
            release: tokio::sync::Barrier::new(2),
        }
    }

    async fn pause_before_acquisition_return(&self) {
        self.after_creation.wait().await;
        self.release.wait().await;
    }

    async fn wait_until_ready_to_return(&self) {
        self.after_creation.wait().await;
    }

    async fn release(&self) {
        self.release.wait().await;
    }
}

#[cfg(test)]
#[derive(Debug)]
struct WorkspaceOverlayFreezeHook {
    after_freeze: tokio::sync::Barrier,
    release: tokio::sync::Barrier,
}

#[cfg(test)]
impl WorkspaceOverlayFreezeHook {
    fn new() -> Self {
        Self {
            after_freeze: tokio::sync::Barrier::new(2),
            release: tokio::sync::Barrier::new(2),
        }
    }

    async fn pause_after_freeze(&self) {
        self.after_freeze.wait().await;
        self.release.wait().await;
    }

    async fn wait_until_frozen(&self) {
        self.after_freeze.wait().await;
    }

    async fn release(&self) {
        self.release.wait().await;
    }
}

#[cfg(test)]
#[derive(Debug)]
pub(crate) struct WorkspaceSettlementHook {
    before_inspection: tokio::sync::Barrier,
    release: tokio::sync::Barrier,
    failure: std::sync::Mutex<Option<String>>,
}

#[cfg(test)]
impl WorkspaceSettlementHook {
    pub(crate) fn new() -> Self {
        Self {
            before_inspection: tokio::sync::Barrier::new(2),
            release: tokio::sync::Barrier::new(2),
            failure: std::sync::Mutex::new(None),
        }
    }

    /// Arms one deterministic inspection failure. The failure is consumed
    /// only when a committed child reaches final workspace settlement.
    pub(crate) fn fail_next(&self, detail: impl Into<String>) {
        *self.failure.lock().expect("workspace settlement hook") = Some(detail.into());
    }

    async fn maybe_fail(&self, snapshot: &WorkspaceSnapshot) -> Option<WorkspaceSettlement> {
        let detail = self
            .failure
            .lock()
            .expect("workspace settlement hook")
            .take()?;
        self.before_inspection.wait().await;
        self.release.wait().await;
        Some(WorkspaceSettlement::unresolved(snapshot.clone(), detail))
    }

    pub(crate) async fn wait_until_entered(&self) {
        self.before_inspection.wait().await;
    }

    pub(crate) async fn release(&self) {
        self.release.wait().await;
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_OVERLAY_BYTES, MAX_OVERLAY_FILES, SubagentWorkspaceManager, SubagentWorkspacePolicy,
        WorkspaceAcquireHook, WorkspaceCleanup, WorkspaceIsolation, WorkspaceOverlayFreezeHook,
        WorkspaceSnapshot, deterministic_worktree_name, parse_overlay_manifest,
    };
    use crate::runtime::cancellation::CancellationSignal;
    use crate::runtime::identity::SubagentId;

    fn git(cwd: &std::path::Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(args)
            .env("GIT_AUTHOR_NAME", "rustX tests")
            .env("GIT_AUTHOR_EMAIL", "rustx-tests@example.invalid")
            .env("GIT_COMMITTER_NAME", "rustX tests")
            .env("GIT_COMMITTER_EMAIL", "rustx-tests@example.invalid")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .expect("git");
        assert!(
            output.status.success(),
            "git {:?}: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).expect("utf8")
    }

    fn repository() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("temp dir");
        git(dir.path(), &["init"]);
        std::fs::write(dir.path().join("tracked.txt"), "committed\n").expect("file");
        git(dir.path(), &["add", "tracked.txt"]);
        git(dir.path(), &["commit", "-m", "initial"]);
        dir
    }

    fn repository_with_workspace(relative: &std::path::Path) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("temp dir");
        git(dir.path(), &["init"]);
        let workspace = dir.path().join(relative);
        std::fs::create_dir_all(&workspace).expect("logical workspace");
        std::fs::write(workspace.join("scoped.txt"), "committed scope\n").expect("file");
        git(dir.path(), &["add", "--all"]);
        git(dir.path(), &["commit", "-m", "initial scoped workspace"]);
        dir
    }

    fn head(path: &std::path::Path) -> String {
        git(path, &["rev-parse", "HEAD"]).trim().to_owned()
    }

    fn commit(path: &std::path::Path, message: &str) {
        git(path, &["add", "--all"]);
        git(path, &["commit", "-m", message]);
    }

    fn ignore_target(path: &std::path::Path) {
        std::fs::write(path.join(".gitignore"), "/target/\n").expect("ignore file");
        commit(path, "ignore build output");
    }

    fn declare_overlay(
        repository: &std::path::Path,
        workspace_relative: &std::path::Path,
        entries: &[String],
    ) -> std::path::PathBuf {
        let workspace = repository.join(workspace_relative);
        std::fs::create_dir_all(&workspace).expect("overlay logical workspace");
        let manifest = entries.join("\n") + "\n";
        std::fs::write(workspace.join(super::WORKTREE_INCLUDE_MANIFEST), manifest)
            .expect("overlay manifest");
        let ignored = entries
            .iter()
            .map(|entry| {
                let repository_relative = workspace_relative.join(entry);
                format!("/{}", repository_relative.display())
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        std::fs::write(repository.join(".gitignore"), ignored).expect("ignore overlay files");
        commit(repository, "declare local worktree overlay");
        workspace
    }

    fn write_overlay_file(workspace: &std::path::Path, relative: &str, bytes: &[u8]) {
        let path = workspace.join(relative);
        std::fs::create_dir_all(path.parent().expect("overlay file parent"))
            .expect("overlay parent");
        std::fs::write(path, bytes).expect("overlay file");
    }

    /// The resolved policy of an ordinary isolated definition under the
    /// Issue #188 default: strict clean-parent. Tests of unrelated
    /// acquisition/settlement semantics construct this policy so they keep
    /// exercising the current default rather than freezing the obsolete
    /// permissive default.
    fn default_isolated() -> SubagentWorkspacePolicy {
        SubagentWorkspacePolicy::GitWorktree {
            require_clean_parent: true,
        }
    }

    #[cfg(unix)]
    fn shell_quote(path: &std::path::Path) -> String {
        format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
    }

    fn ref_exists(path: &std::path::Path, branch: &str) -> bool {
        std::process::Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["show-ref", "--verify", "--quiet"])
            .arg(format!("refs/heads/{branch}"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .status()
            .expect("git")
            .success()
    }

    #[test]
    fn deterministic_name_is_identity_based_not_completion_based() {
        let first = SubagentId::new("conversation-a-subagent-1");
        let second = SubagentId::new("conversation-a-subagent-2");
        assert_eq!(
            deterministic_worktree_name(&first),
            deterministic_worktree_name(&first)
        );
        assert_ne!(
            deterministic_worktree_name(&first),
            deterministic_worktree_name(&second)
        );
        assert_eq!(deterministic_worktree_name(&first).len(), 64);
    }

    #[tokio::test]
    async fn shared_policy_preserves_the_existing_workspace_authority() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let workspace = directory.path().join("shared-workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        std::fs::write(workspace.join("dirty.txt"), "shared bytes\n").expect("file");
        let manager = SubagentWorkspaceManager::new(&workspace, directory.path().join("artifacts"));
        let lease = manager
            .acquire(
                SubagentWorkspacePolicy::SharedWorkspace,
                &SubagentId::new("conversation-shared-subagent-1"),
                &CancellationSignal::new(),
            )
            .await
            .expect("shared lease");
        assert_eq!(lease.logical_workspace(), workspace);
        assert!(!lease.snapshot().is_isolated());
        let settlement = lease.settle_after_child().await;
        assert_eq!(settlement.cleanup, WorkspaceCleanup::Shared);
        assert!(workspace.exists());
        assert_eq!(
            std::fs::read_to_string(workspace.join("dirty.txt")).expect("shared file"),
            "shared bytes\n"
        );
    }

    /// Issue #188: with an explicit permissive policy
    /// (`require_clean_parent = false`), a dirty parent is still allowed, but
    /// the child receives exactly the committed snapshot — tracked parent
    /// edits and untracked parent files are never copied.
    #[tokio::test]
    async fn dirty_parent_is_not_copied_into_an_explicitly_permissive_worktree() {
        let dir = repository();
        let base = head(dir.path());
        std::fs::write(dir.path().join("tracked.txt"), "parent dirty\n").expect("dirty file");
        std::fs::write(dir.path().join("untracked.txt"), "parent only\n").expect("untracked");
        let manager = SubagentWorkspaceManager::new(dir.path(), dir.path().join("artifacts"));
        let lease = manager
            .acquire(
                // The explicit opt-out: the child runs from the captured
                // committed HEAD while intentionally ignoring parent-local
                // dirty bytes (Issue #188).
                SubagentWorkspacePolicy::GitWorktree {
                    require_clean_parent: false,
                },
                &SubagentId::new("conversation-a-subagent-1"),
                &CancellationSignal::new(),
            )
            .await
            .expect("worktree");
        let facts = lease.snapshot().git_worktree().expect("Git worktree facts");
        assert!(facts.parent_had_uncommitted_changes);
        assert_eq!(
            facts.base_commit, base,
            "the child base is the exact committed HEAD captured before acquisition"
        );
        assert_eq!(
            std::fs::read_to_string(lease.logical_workspace().join("tracked.txt"))
                .expect("child file"),
            "committed\n"
        );
        assert!(!lease.logical_workspace().join("untracked.txt").exists());
        let settlement = lease.settle_after_child().await;
        assert_eq!(settlement.cleanup, WorkspaceCleanup::Removed);
        assert!(
            !settlement
                .snapshot
                .git_worktree()
                .expect("Git worktree facts")
                .physical_worktree_root
                .exists()
        );
    }

    #[tokio::test]
    async fn clean_parent_worktree_is_created_at_the_exact_head_and_removed_cleanly() {
        let dir = repository();
        let base = head(dir.path());
        let manager = SubagentWorkspaceManager::new(dir.path(), dir.path().join("artifacts"));
        let lease = manager
            .acquire(
                default_isolated(),
                &SubagentId::new("conversation-clean-subagent-1"),
                &CancellationSignal::new(),
            )
            .await
            .expect("worktree");
        let worktree = lease.snapshot().git_worktree().expect("Git worktree facts");
        assert_eq!(worktree.base_commit, base);
        assert_eq!(head(lease.logical_workspace()), base);
        assert!(worktree.branch.starts_with("rustx/subagent/"));
        let settlement = lease.settle_after_child().await;
        assert_eq!(settlement.cleanup, WorkspaceCleanup::Removed);
        assert!(settlement.handoff.is_none());
    }

    #[test]
    fn overlay_manifest_has_one_small_deterministic_line_format() {
        let parsed = parse_overlay_manifest(
            "\n  # full-line comment after trimming\n.env\n config/local.json \n\n",
        )
        .expect("manifest");
        assert_eq!(
            parsed,
            vec![
                std::path::PathBuf::from(".env"),
                std::path::PathBuf::from("config/local.json")
            ]
        );

        let duplicate = parse_overlay_manifest("config//local.json\nconfig/local.json\n")
            .expect_err("normalized duplicate");
        assert!(matches!(
            duplicate,
            super::WorkspaceAcquireError::OverlayDuplicate { .. }
        ));
    }

    #[test]
    fn overlay_file_count_accepts_the_limit_and_rejects_one_more() {
        let accepted = (0..MAX_OVERLAY_FILES)
            .map(|index| format!("local/file-{index}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(
            parse_overlay_manifest(&accepted)
                .expect("the fixed boundary is accepted")
                .len(),
            MAX_OVERLAY_FILES
        );
        let rejected = format!("{accepted}\nlocal/one-past-the-limit");
        assert!(matches!(
            parse_overlay_manifest(&rejected).expect_err("one past the fixed file limit"),
            super::WorkspaceAcquireError::OverlayFileLimit { limit }
                if limit == MAX_OVERLAY_FILES
        ));
    }

    #[tokio::test]
    async fn selected_ignored_file_is_materialized_with_exact_frozen_bytes() {
        let repository = repository();
        let workspace = declare_overlay(
            repository.path(),
            std::path::Path::new(""),
            &["local/runtime.env".to_owned()],
        );
        let expected = b"TOKEN=local-only\n\0binary-tail";
        write_overlay_file(&workspace, "local/runtime.env", expected);
        let runtime = tempfile::tempdir().expect("runtime root");
        let lease = SubagentWorkspaceManager::new(&workspace, runtime.path())
            .acquire(
                default_isolated(),
                &SubagentId::new("conversation-basic-overlay-subagent-1"),
                &CancellationSignal::new(),
            )
            .await
            .expect("worktree with overlay");

        assert_eq!(
            std::fs::read(lease.logical_workspace().join("local/runtime.env"))
                .expect("materialized overlay"),
            expected
        );
        assert_eq!(
            lease.settle_after_child().await.cleanup,
            WorkspaceCleanup::Removed
        );
    }

    #[tokio::test]
    async fn parent_edit_after_overlay_freeze_cannot_change_child_bytes() {
        let repository = repository();
        let workspace = declare_overlay(
            repository.path(),
            std::path::Path::new(""),
            &[".env".to_owned()],
        );
        write_overlay_file(&workspace, ".env", b"FROZEN=before\n");
        let runtime = tempfile::tempdir().expect("runtime root");
        let mut manager = SubagentWorkspaceManager::new(&workspace, runtime.path());
        let hook = std::sync::Arc::new(WorkspaceOverlayFreezeHook::new());
        manager.install_overlay_freeze_hook(hook.clone());
        let cancellation = CancellationSignal::new();
        let task_manager = manager.clone();
        let task_cancellation = cancellation.clone();
        let task = tokio::spawn(async move {
            task_manager
                .acquire(
                    default_isolated(),
                    &SubagentId::new("conversation-frozen-overlay-subagent-1"),
                    &task_cancellation,
                )
                .await
        });

        hook.wait_until_frozen().await;
        write_overlay_file(&workspace, ".env", b"FROZEN=after\n");
        hook.release().await;
        let lease = task.await.expect("acquisition task").expect("worktree");
        assert_eq!(
            std::fs::read(lease.logical_workspace().join(".env")).expect("child overlay"),
            b"FROZEN=before\n"
        );
        assert_eq!(
            std::fs::read(workspace.join(".env")).expect("parent overlay"),
            b"FROZEN=after\n"
        );
        assert_eq!(
            lease.settle_after_child().await.cleanup,
            WorkspaceCleanup::Removed
        );
    }

    #[tokio::test]
    async fn tracked_overlay_selection_is_rejected_explicitly() {
        let repository = repository();
        std::fs::write(
            repository.path().join(super::WORKTREE_INCLUDE_MANIFEST),
            "tracked.txt\n",
        )
        .expect("manifest");
        commit(repository.path(), "select tracked file");
        let runtime = tempfile::tempdir().expect("runtime root");
        let error = SubagentWorkspaceManager::new(repository.path(), runtime.path())
            .acquire(
                default_isolated(),
                &SubagentId::new("conversation-tracked-overlay-subagent-1"),
                &CancellationSignal::new(),
            )
            .await
            .expect_err("tracked overlay selection");
        assert!(matches!(
            error,
            super::WorkspaceAcquireError::OverlayTracked { ref path }
                if path == std::path::Path::new("tracked.txt")
        ));
        assert!(!runtime.path().join("worktrees").exists());
    }

    #[tokio::test]
    async fn non_ignored_overlay_selection_is_rejected_by_git_semantics() {
        let repository = repository();
        std::fs::write(
            repository.path().join(super::WORKTREE_INCLUDE_MANIFEST),
            "local.env\n",
        )
        .expect("manifest");
        commit(repository.path(), "select non-ignored local file");
        std::fs::write(repository.path().join("local.env"), "LOCAL=value\n")
            .expect("non-ignored local file");
        let runtime = tempfile::tempdir().expect("runtime root");
        let error = SubagentWorkspaceManager::new(repository.path(), runtime.path())
            .acquire(
                SubagentWorkspacePolicy::GitWorktree {
                    require_clean_parent: false,
                },
                &SubagentId::new("conversation-non-ignored-overlay-subagent-1"),
                &CancellationSignal::new(),
            )
            .await
            .expect_err("non-ignored overlay selection");
        assert!(matches!(
            error,
            super::WorkspaceAcquireError::OverlayNotIgnored { ref path }
                if path == std::path::Path::new("local.env")
        ));
        assert!(!runtime.path().join("worktrees").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn overlay_symlink_and_symlink_ancestor_are_rejected() {
        use std::os::unix::fs::symlink;

        for (identity, entry, create) in [
            ("leaf", "local.env", "leaf"),
            ("ancestor", "linked/local.env", "ancestor"),
        ] {
            let repository = repository();
            let workspace = declare_overlay(
                repository.path(),
                std::path::Path::new(""),
                &[entry.to_owned()],
            );
            let outside = tempfile::tempdir().expect("outside");
            std::fs::write(outside.path().join("local.env"), "outside\n").expect("outside file");
            if create == "leaf" {
                symlink(outside.path().join("local.env"), workspace.join(entry))
                    .expect("leaf symlink");
            } else {
                symlink(outside.path(), workspace.join("linked")).expect("ancestor symlink");
            }
            let runtime = tempfile::tempdir().expect("runtime root");
            let error = SubagentWorkspaceManager::new(&workspace, runtime.path())
                .acquire(
                    SubagentWorkspacePolicy::GitWorktree {
                        require_clean_parent: false,
                    },
                    &SubagentId::new(format!(
                        "conversation-{identity}-symlink-overlay-subagent-1"
                    )),
                    &CancellationSignal::new(),
                )
                .await
                .expect_err("symlink overlay selection");
            assert!(matches!(
                error,
                super::WorkspaceAcquireError::OverlaySymlink { .. }
            ));
            assert!(!runtime.path().join("worktrees").exists());
        }
    }

    #[test]
    fn overlay_traversal_and_absolute_paths_are_rejected() {
        for manifest in [
            "../outside.env\n",
            "/absolute.env\n",
            "a/../../outside.env\n",
        ] {
            assert!(matches!(
                parse_overlay_manifest(manifest).expect_err("unsafe overlay path"),
                super::WorkspaceAcquireError::OverlayUnsafePath { .. }
            ));
        }
    }

    #[tokio::test]
    async fn missing_selected_overlay_file_rejects_acquisition() {
        let repository = repository();
        let workspace = declare_overlay(
            repository.path(),
            std::path::Path::new(""),
            &["missing.env".to_owned()],
        );
        let runtime = tempfile::tempdir().expect("runtime root");
        let error = SubagentWorkspaceManager::new(&workspace, runtime.path())
            .acquire(
                default_isolated(),
                &SubagentId::new("conversation-missing-overlay-subagent-1"),
                &CancellationSignal::new(),
            )
            .await
            .expect_err("missing overlay selection");
        assert!(matches!(
            error,
            super::WorkspaceAcquireError::OverlayMissing { ref path }
                if path == std::path::Path::new("missing.env")
        ));
        assert!(!runtime.path().join("worktrees").exists());
    }

    #[tokio::test]
    async fn overlay_total_bytes_accepts_the_limit_and_rejects_one_more() {
        let accepted_repository = repository();
        let accepted_workspace = declare_overlay(
            accepted_repository.path(),
            std::path::Path::new(""),
            &["first.bin".to_owned(), "second.bin".to_owned()],
        );
        write_overlay_file(
            &accepted_workspace,
            "first.bin",
            &vec![b'a'; MAX_OVERLAY_BYTES / 2],
        );
        write_overlay_file(
            &accepted_workspace,
            "second.bin",
            &vec![b'b'; MAX_OVERLAY_BYTES - (MAX_OVERLAY_BYTES / 2)],
        );
        let accepted_runtime = tempfile::tempdir().expect("runtime root");
        let accepted = SubagentWorkspaceManager::new(&accepted_workspace, accepted_runtime.path())
            .acquire(
                default_isolated(),
                &SubagentId::new("conversation-byte-boundary-overlay-subagent-1"),
                &CancellationSignal::new(),
            )
            .await
            .expect("exact byte boundary");
        assert_eq!(
            std::fs::metadata(accepted.logical_workspace().join("first.bin"))
                .expect("first overlay")
                .len()
                + std::fs::metadata(accepted.logical_workspace().join("second.bin"))
                    .expect("second overlay")
                    .len(),
            u64::try_from(MAX_OVERLAY_BYTES).expect("limit fits u64")
        );
        assert_eq!(
            accepted.settle_after_child().await.cleanup,
            WorkspaceCleanup::Removed
        );

        let rejected_repository = repository();
        let rejected_workspace = declare_overlay(
            rejected_repository.path(),
            std::path::Path::new(""),
            &["first.bin".to_owned(), "second.bin".to_owned()],
        );
        write_overlay_file(
            &rejected_workspace,
            "first.bin",
            &vec![0; MAX_OVERLAY_BYTES / 2],
        );
        write_overlay_file(
            &rejected_workspace,
            "second.bin",
            &vec![0; MAX_OVERLAY_BYTES - (MAX_OVERLAY_BYTES / 2) + 1],
        );
        let rejected_runtime = tempfile::tempdir().expect("runtime root");
        let error = SubagentWorkspaceManager::new(&rejected_workspace, rejected_runtime.path())
            .acquire(
                default_isolated(),
                &SubagentId::new("conversation-over-byte-limit-overlay-subagent-1"),
                &CancellationSignal::new(),
            )
            .await
            .expect_err("one byte over the fixed total limit");
        assert!(matches!(
            error,
            super::WorkspaceAcquireError::OverlayByteLimit { limit }
                if limit == MAX_OVERLAY_BYTES
        ));
        assert!(!rejected_runtime.path().join("worktrees").exists());
    }

    #[tokio::test]
    async fn repository_root_scope_maps_to_the_physical_worktree_root() {
        let repository = repository();
        let runtime = tempfile::tempdir().expect("runtime root");
        let subagent_id = SubagentId::new("conversation-root-scope-subagent-1");
        let lease = SubagentWorkspaceManager::new(repository.path(), runtime.path())
            .acquire(default_isolated(), &subagent_id, &CancellationSignal::new())
            .await
            .expect("worktree");
        let physical = runtime
            .path()
            .join("worktrees")
            .join(deterministic_worktree_name(&subagent_id));
        let facts = lease.snapshot().git_worktree().expect("Git worktree facts");

        assert_eq!(
            facts.repository_relative_workspace,
            std::path::Path::new("")
        );
        assert_eq!(facts.physical_worktree_root, physical);
        assert_eq!(lease.logical_workspace(), physical);
        assert_eq!(lease.physical_worktree_root(), Some(physical.as_path()));
        assert_eq!(
            lease.settle_after_child().await.cleanup,
            WorkspaceCleanup::Removed
        );
    }

    #[tokio::test]
    async fn repository_subdirectory_scope_is_preserved_in_the_isolated_checkout() {
        let repository = repository_with_workspace(std::path::Path::new("backend"));
        let runtime = tempfile::tempdir().expect("runtime root");
        let subagent_id = SubagentId::new("conversation-subdir-scope-subagent-1");
        let lease =
            SubagentWorkspaceManager::new(repository.path().join("backend"), runtime.path())
                .acquire(default_isolated(), &subagent_id, &CancellationSignal::new())
                .await
                .expect("worktree");
        let physical = runtime
            .path()
            .join("worktrees")
            .join(deterministic_worktree_name(&subagent_id));
        let logical = physical.join("backend");
        let facts = lease.snapshot().git_worktree().expect("Git worktree facts");

        assert_eq!(
            facts.repository_relative_workspace,
            std::path::Path::new("backend")
        );
        assert_eq!(facts.physical_worktree_root, physical);
        assert_eq!(lease.logical_workspace(), logical);
        assert_eq!(
            std::fs::read_to_string(logical.join("scoped.txt")).expect("committed scoped file"),
            "committed scope\n"
        );
        assert_eq!(
            lease.settle_after_child().await.cleanup,
            WorkspaceCleanup::Removed
        );
    }

    #[tokio::test]
    async fn nested_repository_scope_is_preserved_exactly() {
        let relative = std::path::Path::new("a/b/c");
        let repository = repository_with_workspace(relative);
        let runtime = tempfile::tempdir().expect("runtime root");
        let subagent_id = SubagentId::new("conversation-nested-scope-subagent-1");
        let lease = SubagentWorkspaceManager::new(repository.path().join(relative), runtime.path())
            .acquire(default_isolated(), &subagent_id, &CancellationSignal::new())
            .await
            .expect("worktree");
        let physical = runtime
            .path()
            .join("worktrees")
            .join(deterministic_worktree_name(&subagent_id));
        let facts = lease.snapshot().git_worktree().expect("Git worktree facts");

        assert_eq!(facts.repository_relative_workspace, relative);
        assert_eq!(facts.physical_worktree_root, physical);
        assert_eq!(lease.logical_workspace(), physical.join(relative));
        assert_eq!(
            lease.settle_after_child().await.cleanup,
            WorkspaceCleanup::Removed
        );
    }

    /// The fixture is intentionally dirty: the logical scope exists only as
    /// an untracked directory, so reaching the committed-scope validation
    /// requires the explicit permissive opt-out (`require_clean_parent =
    /// false`). The test then proves the committed checkout's missing scope
    /// fails closed without widening or leaking the worktree.
    #[tokio::test]
    async fn absent_committed_scope_fails_without_widening_or_leaking_the_worktree() {
        let repository = repository();
        let parent_logical_workspace = repository.path().join("uncommitted-scope");
        std::fs::create_dir_all(&parent_logical_workspace).expect("uncommitted scope");
        std::fs::write(
            parent_logical_workspace.join("only-parent.txt"),
            "uncommitted\n",
        )
        .expect("uncommitted file");
        let runtime = tempfile::tempdir().expect("runtime root");
        let subagent_id = SubagentId::new("conversation-absent-scope-subagent-1");
        let physical = runtime
            .path()
            .join("worktrees")
            .join(deterministic_worktree_name(&subagent_id));
        let branch = format!(
            "rustx/subagent/{}",
            deterministic_worktree_name(&subagent_id)
        );

        let error = SubagentWorkspaceManager::new(&parent_logical_workspace, runtime.path())
            .acquire(
                SubagentWorkspacePolicy::GitWorktree {
                    require_clean_parent: false,
                },
                &subagent_id,
                &CancellationSignal::new(),
            )
            .await
            .expect_err("the committed checkout does not contain the logical scope");

        assert!(matches!(
            error,
            super::WorkspaceAcquireError::InvalidSnapshot { .. }
        ));
        assert!(
            !physical.exists(),
            "the staged physical worktree is removed"
        );
        assert!(
            !ref_exists(repository.path(), &branch),
            "the staged ref is removed"
        );
        assert!(
            parent_logical_workspace.exists(),
            "the parent logical workspace is untouched"
        );
    }

    #[tokio::test]
    async fn terminal_handoff_preserves_logical_scope_and_identifies_physical_worktree() {
        let relative = std::path::Path::new("backend/service");
        let repository = repository_with_workspace(relative);
        let runtime = tempfile::tempdir().expect("runtime root");
        let subagent_id = SubagentId::new("conversation-scoped-handoff-subagent-1");
        let lease = SubagentWorkspaceManager::new(repository.path().join(relative), runtime.path())
            .acquire(default_isolated(), &subagent_id, &CancellationSignal::new())
            .await
            .expect("worktree");
        let physical = lease
            .physical_worktree_root()
            .expect("physical worktree")
            .to_path_buf();
        let logical = physical.join(relative);
        std::fs::write(logical.join("child-work.txt"), "retain me\n").expect("child work");

        let settlement = lease.settle_after_child().await;
        let handoff = settlement.handoff.as_ref().expect("retained handoff");

        assert_eq!(settlement.cleanup, WorkspaceCleanup::Preserved);
        assert_eq!(settlement.snapshot.logical_workspace, logical);
        assert_eq!(
            settlement
                .snapshot
                .git_worktree()
                .expect("Git worktree facts")
                .physical_worktree_root,
            physical
        );
        assert_eq!(handoff.logical_workspace, logical);
        assert_eq!(handoff.physical_worktree_root, physical);
        assert!(handoff.dirty);
        assert!(physical.exists());
        assert!(logical.join("child-work.txt").exists());
    }

    #[test]
    fn isolated_snapshot_validation_rejects_authority_widening_shapes() {
        let physical = std::path::PathBuf::from("/runtime/worktrees/token");
        let valid = WorkspaceSnapshot::worktree(
            physical.join("backend"),
            std::path::PathBuf::from("/repo"),
            std::path::PathBuf::from("backend"),
            physical.clone(),
            "c1".to_owned(),
            "rustx/subagent/token".to_owned(),
            false,
        );
        assert!(valid.validate().is_ok());
        let wire = serde_json::to_value(&valid).expect("serialize isolated snapshot");
        assert_eq!(wire["logicalWorkspace"], "/runtime/worktrees/token/backend");
        assert_eq!(wire["isolation"]["type"], "git_worktree");
        assert_eq!(
            serde_json::from_value::<WorkspaceSnapshot>(wire)
                .expect("deserialize isolated snapshot"),
            valid
        );

        let mut widened = valid.clone();
        widened.logical_workspace = physical.clone();
        assert!(widened.validate().is_err());

        let mut escaping = valid;
        let WorkspaceIsolation::GitWorktree(worktree) = &mut escaping.isolation else {
            panic!("isolated facts");
        };
        worktree.repository_relative_workspace = std::path::PathBuf::from("../outside");
        escaping.logical_workspace = physical.join("../outside");
        assert!(escaping.validate().is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn worktree_creation_suppresses_checkout_hook_mutations() {
        use std::os::unix::fs::PermissionsExt;

        let dir = repository();
        let runtime_root = dir.path().join("artifacts");
        let subagent_id = SubagentId::new("conversation-hook-subagent-1");
        let workspace = runtime_root
            .join("worktrees")
            .join(super::deterministic_worktree_name(&subagent_id));
        let hook_marker = dir.path().join("post-checkout-hook-ran");
        let hook = dir.path().join(".git/hooks/post-checkout");
        std::fs::write(
            &hook,
            format!(
                "#!/bin/sh\nprintf 'hook mutation\\n' > {}\nprintf 'ran\\n' > {}\n",
                shell_quote(&workspace.join("hook-mutated.txt")),
                shell_quote(&hook_marker),
            ),
        )
        .expect("post-checkout hook");
        let mut permissions = std::fs::metadata(&hook)
            .expect("hook metadata")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&hook, permissions).expect("executable hook");

        let manager = SubagentWorkspaceManager::new(dir.path(), &runtime_root);
        let lease = manager
            .acquire(default_isolated(), &subagent_id, &CancellationSignal::new())
            .await
            .expect("worktree");

        assert_eq!(head(lease.logical_workspace()), head(dir.path()));
        assert!(
            !workspace.join("hook-mutated.txt").exists(),
            "checkout hook must not mutate the exact-snapshot worktree"
        );
        assert!(!hook_marker.exists(), "checkout hook must not run");
        assert_eq!(
            lease.settle_after_child().await.cleanup,
            WorkspaceCleanup::Removed
        );
    }

    #[tokio::test]
    async fn ignored_only_child_artifact_is_cleaned_without_a_handoff() {
        let dir = repository();
        ignore_target(dir.path());
        let manager = SubagentWorkspaceManager::new(dir.path(), dir.path().join("artifacts"));
        let lease = manager
            .acquire(
                default_isolated(),
                &SubagentId::new("conversation-ignored-only-subagent-1"),
                &CancellationSignal::new(),
            )
            .await
            .expect("worktree");
        let path = lease.logical_workspace().to_path_buf();
        let branch = lease
            .snapshot()
            .git_worktree()
            .expect("Git worktree facts")
            .branch
            .clone();
        std::fs::create_dir_all(path.join("target/debug")).expect("target");
        std::fs::write(path.join("target/debug/generated"), "cache\n").expect("cache");

        let settlement = lease.settle_after_child().await;

        assert_eq!(settlement.cleanup, WorkspaceCleanup::Removed);
        assert!(settlement.handoff.is_none());
        assert!(!path.exists(), "ignored-only output is disposable cache");
        assert!(!ref_exists(dir.path(), &branch), "runtime ref was removed");
    }

    #[tokio::test]
    async fn overlay_only_mutation_is_source_clean_and_disposable() {
        let repository = repository();
        let workspace = declare_overlay(
            repository.path(),
            std::path::Path::new(""),
            &[".env".to_owned()],
        );
        write_overlay_file(&workspace, ".env", b"VALUE=frozen\n");
        let runtime = tempfile::tempdir().expect("runtime root");
        let lease = SubagentWorkspaceManager::new(&workspace, runtime.path())
            .acquire(
                default_isolated(),
                &SubagentId::new("conversation-overlay-only-settlement-subagent-1"),
                &CancellationSignal::new(),
            )
            .await
            .expect("worktree with overlay");
        let path = lease.logical_workspace().to_path_buf();
        std::fs::write(path.join(".env"), "VALUE=child-mutated\n").expect("mutate overlay");

        let settlement = lease.settle_after_child().await;

        assert_eq!(settlement.cleanup, WorkspaceCleanup::Removed);
        assert!(settlement.handoff.is_none());
        assert!(!path.exists(), "overlay-only state does not force handoff");
    }

    #[tokio::test]
    async fn tracked_child_commit_with_overlay_still_produces_source_handoff() {
        let repository = repository();
        let workspace = declare_overlay(
            repository.path(),
            std::path::Path::new(""),
            &[".env".to_owned()],
        );
        write_overlay_file(&workspace, ".env", b"VALUE=local\n");
        let runtime = tempfile::tempdir().expect("runtime root");
        let lease = SubagentWorkspaceManager::new(&workspace, runtime.path())
            .acquire(
                default_isolated(),
                &SubagentId::new("conversation-overlay-source-change-subagent-1"),
                &CancellationSignal::new(),
            )
            .await
            .expect("worktree with overlay");
        let base = lease
            .snapshot()
            .git_worktree()
            .expect("Git worktree facts")
            .base_commit
            .clone();
        std::fs::write(
            lease.logical_workspace().join("tracked.txt"),
            "child source commit\n",
        )
        .expect("tracked child edit");
        commit(
            lease.logical_workspace(),
            "child source commit with overlay",
        );

        let settlement = lease.settle_after_child().await;
        let handoff = settlement.handoff.expect("source handoff");
        assert_eq!(settlement.cleanup, WorkspaceCleanup::Preserved);
        assert!(
            !handoff.dirty,
            "the overlay is not ordinary source dirtiness"
        );
        assert_ne!(handoff.head_commit, base);
    }

    #[tokio::test]
    async fn parent_movement_after_acquisition_cannot_change_the_child_base() {
        let dir = repository();
        let manager = SubagentWorkspaceManager::new(dir.path(), dir.path().join("artifacts"));
        let lease = manager
            .acquire(
                default_isolated(),
                &SubagentId::new("conversation-snapshot-subagent-1"),
                &CancellationSignal::new(),
            )
            .await
            .expect("worktree");
        let base = lease
            .snapshot()
            .git_worktree()
            .expect("Git worktree facts")
            .base_commit
            .clone();
        std::fs::write(dir.path().join("tracked.txt"), "parent commit two\n")
            .expect("parent update");
        commit(dir.path(), "parent second commit");
        let parent_head = head(dir.path());
        assert_ne!(parent_head, base);
        assert_eq!(head(lease.logical_workspace()), base);
        assert_eq!(
            head(lease.logical_workspace()),
            lease
                .snapshot()
                .git_worktree()
                .expect("Git worktree facts")
                .base_commit
        );
        let settlement = lease.settle_after_child().await;
        assert_eq!(settlement.cleanup, WorkspaceCleanup::Removed);
        assert_eq!(head(dir.path()), parent_head);
    }

    #[tokio::test]
    async fn child_dirty_work_is_preserved_as_a_handoff_without_touching_parent_bytes() {
        let dir = repository();
        let parent_before =
            std::fs::read_to_string(dir.path().join("tracked.txt")).expect("parent");
        let manager = SubagentWorkspaceManager::new(dir.path(), dir.path().join("artifacts"));
        let lease = manager
            .acquire(
                default_isolated(),
                &SubagentId::new("conversation-dirty-child-subagent-1"),
                &CancellationSignal::new(),
            )
            .await
            .expect("worktree");
        std::fs::write(
            lease.logical_workspace().join("tracked.txt"),
            "child work\n",
        )
        .expect("child tracked change");
        std::fs::write(
            lease.logical_workspace().join("new.txt"),
            "child artifact\n",
        )
        .expect("child untracked change");
        let path = lease.logical_workspace().to_path_buf();
        let settlement = lease.settle_after_child().await;
        let handoff = settlement.handoff.expect("dirty handoff");
        assert_eq!(settlement.cleanup, WorkspaceCleanup::Preserved);
        assert_eq!(handoff.logical_workspace, path);
        assert_eq!(handoff.physical_worktree_root, path);
        assert_eq!(handoff.base_commit, handoff.head_commit);
        assert!(handoff.dirty);
        assert!(path.exists());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("tracked.txt")).expect("parent"),
            parent_before
        );
    }

    #[tokio::test]
    async fn committed_child_work_is_preserved_even_when_the_worktree_is_clean() {
        let dir = repository();
        let manager = SubagentWorkspaceManager::new(dir.path(), dir.path().join("artifacts"));
        let lease = manager
            .acquire(
                default_isolated(),
                &SubagentId::new("conversation-committed-child-subagent-1"),
                &CancellationSignal::new(),
            )
            .await
            .expect("worktree");
        let base = lease
            .snapshot()
            .git_worktree()
            .expect("Git worktree facts")
            .base_commit
            .clone();
        std::fs::write(
            lease.logical_workspace().join("tracked.txt"),
            "committed child\n",
        )
        .expect("child file");
        commit(lease.logical_workspace(), "child commit");
        let settlement = lease.settle_after_child().await;
        let handoff = settlement.handoff.expect("committed handoff");
        assert_eq!(settlement.cleanup, WorkspaceCleanup::Preserved);
        assert!(!handoff.dirty);
        assert_ne!(handoff.head_commit, base);
        assert_eq!(handoff.head_commit, head(&handoff.physical_worktree_root));
    }

    #[tokio::test]
    async fn committed_child_work_with_ignored_cache_is_dirty_false_but_changed() {
        let dir = repository();
        ignore_target(dir.path());
        let manager = SubagentWorkspaceManager::new(dir.path(), dir.path().join("artifacts"));
        let lease = manager
            .acquire(
                default_isolated(),
                &SubagentId::new("conversation-committed-ignored-subagent-1"),
                &CancellationSignal::new(),
            )
            .await
            .expect("worktree");
        let base = lease
            .snapshot()
            .git_worktree()
            .expect("Git worktree facts")
            .base_commit
            .clone();
        std::fs::write(
            lease.logical_workspace().join("tracked.txt"),
            "child commit\n",
        )
        .expect("child file");
        commit(lease.logical_workspace(), "child commit");
        std::fs::create_dir_all(lease.logical_workspace().join("target/debug")).expect("target");
        std::fs::write(
            lease.logical_workspace().join("target/debug/generated"),
            "cache\n",
        )
        .expect("cache");

        let settlement = lease.settle_after_child().await;
        let handoff = settlement.handoff.expect("committed handoff");

        assert_eq!(settlement.cleanup, WorkspaceCleanup::Preserved);
        assert!(!handoff.dirty);
        assert_ne!(handoff.head_commit, base);
    }

    #[tokio::test]
    async fn untracked_source_and_ignored_cache_preserve_a_dirty_handoff() {
        let dir = repository();
        ignore_target(dir.path());
        let manager = SubagentWorkspaceManager::new(dir.path(), dir.path().join("artifacts"));
        let lease = manager
            .acquire(
                default_isolated(),
                &SubagentId::new("conversation-source-ignored-subagent-1"),
                &CancellationSignal::new(),
            )
            .await
            .expect("worktree");
        std::fs::write(
            lease.logical_workspace().join("new-source.rs"),
            "fn generated() {}\n",
        )
        .expect("source");
        std::fs::create_dir_all(lease.logical_workspace().join("target/debug")).expect("target");
        std::fs::write(
            lease.logical_workspace().join("target/debug/generated"),
            "cache\n",
        )
        .expect("cache");

        let settlement = lease.settle_after_child().await;
        let handoff = settlement.handoff.expect("source handoff");

        assert_eq!(settlement.cleanup, WorkspaceCleanup::Preserved);
        assert!(handoff.dirty);
        assert_eq!(handoff.base_commit, handoff.head_commit);
    }

    #[tokio::test]
    async fn recovered_inspection_uses_the_same_ordinary_dirty_definition() {
        let dir = repository();
        ignore_target(dir.path());
        let manager = SubagentWorkspaceManager::new(dir.path(), dir.path().join("artifacts"));
        let lease = manager
            .acquire(
                default_isolated(),
                &SubagentId::new("conversation-recovered-ignored-subagent-1"),
                &CancellationSignal::new(),
            )
            .await
            .expect("worktree");
        let snapshot = lease.snapshot().clone();
        std::fs::create_dir_all(lease.logical_workspace().join("target/debug")).expect("target");
        std::fs::write(
            lease.logical_workspace().join("target/debug/generated"),
            "cache\n",
        )
        .expect("cache");
        drop(lease);

        let settlement = SubagentWorkspaceManager::inspect_recovered(&snapshot);
        let handoff = settlement.handoff.expect("recovered handoff");

        assert_eq!(settlement.cleanup, WorkspaceCleanup::Preserved);
        assert!(!handoff.dirty);
        assert_eq!(handoff.base_commit, handoff.head_commit);
    }

    #[tokio::test]
    async fn committed_and_dirty_child_work_reports_both_terminal_facts() {
        let dir = repository();
        let manager = SubagentWorkspaceManager::new(dir.path(), dir.path().join("artifacts"));
        let lease = manager
            .acquire(
                default_isolated(),
                &SubagentId::new("conversation-committed-dirty-subagent-1"),
                &CancellationSignal::new(),
            )
            .await
            .expect("worktree");
        let base = lease
            .snapshot()
            .git_worktree()
            .expect("Git worktree facts")
            .base_commit
            .clone();
        std::fs::write(
            lease.logical_workspace().join("tracked.txt"),
            "child commit\n",
        )
        .expect("child file");
        commit(lease.logical_workspace(), "child commit");
        std::fs::write(
            lease.logical_workspace().join("uncommitted.txt"),
            "still active\n",
        )
        .expect("dirty child file");
        let settlement = lease.settle_after_child().await;
        let handoff = settlement.handoff.expect("combined handoff");
        assert_eq!(settlement.cleanup, WorkspaceCleanup::Preserved);
        assert!(handoff.dirty);
        assert_ne!(handoff.head_commit, base);
    }

    #[tokio::test]
    async fn staged_dirty_state_fails_closed_and_never_force_removes_work() {
        let dir = repository();
        let manager = SubagentWorkspaceManager::new(dir.path(), dir.path().join("artifacts"));
        let lease = manager
            .acquire(
                default_isolated(),
                &SubagentId::new("conversation-staged-dirty-subagent-1"),
                &CancellationSignal::new(),
            )
            .await
            .expect("worktree");
        let path = lease.logical_workspace().to_path_buf();
        std::fs::write(path.join("staged.txt"), "unknown staged work\n").expect("staged file");
        let error = lease
            .settle_staged()
            .await
            .expect_err("unexpected staged work must block cleanup");
        assert!(error.settlement.handoff.is_some());
        assert!(error.settlement.snapshot.logical_workspace.exists());
        assert!(path.join("staged.txt").exists());
    }

    #[tokio::test]
    async fn cancellation_after_overlay_staging_settles_the_staged_lease() {
        let dir = repository_with_workspace(std::path::Path::new("backend"));
        let workspace = declare_overlay(
            dir.path(),
            std::path::Path::new("backend"),
            &["local/runtime.env".to_owned()],
        );
        write_overlay_file(&workspace, "local/runtime.env", b"STAGED=overlay\n");
        let runtime_root = dir.path().join("artifacts");
        let mut manager = SubagentWorkspaceManager::new(&workspace, &runtime_root);
        let hook = std::sync::Arc::new(WorkspaceAcquireHook::new());
        manager.install_acquisition_hook(hook.clone());
        let cancellation = CancellationSignal::new();
        let subagent = SubagentId::new("conversation-cancelled-acquisition-subagent-1");
        let task_manager = manager.clone();
        let task_cancellation = cancellation.clone();
        let task_subagent = subagent.clone();
        let task = tokio::spawn(async move {
            task_manager
                .acquire(default_isolated(), &task_subagent, &task_cancellation)
                .await
        });
        hook.wait_until_ready_to_return().await;
        let path = runtime_root
            .join("worktrees")
            .join(deterministic_worktree_name(&subagent));
        let branch = format!("rustx/subagent/{}", deterministic_worktree_name(&subagent));
        assert!(path.exists(), "the barrier is after Git worktree creation");
        assert!(path.join("backend").exists(), "the logical scope exists");
        assert_eq!(
            std::fs::read(path.join("backend/local/runtime.env"))
                .expect("the barrier is after overlay materialization"),
            b"STAGED=overlay\n"
        );
        cancellation.cancel();
        hook.release().await;
        let error = task
            .await
            .expect("acquisition task")
            .expect_err("cancellation");
        assert!(matches!(error, super::WorkspaceAcquireError::Cancelled));
        assert!(!path.exists(), "the staged physical worktree is settled");
        assert!(
            !ref_exists(dir.path(), &branch),
            "the staged ref is removed"
        );
    }

    #[tokio::test]
    async fn concurrent_children_have_distinct_deterministic_paths_and_refs() {
        let dir = repository();
        let runtime = tempfile::tempdir().expect("runtime root");
        let manager = SubagentWorkspaceManager::new(dir.path(), runtime.path());
        let first_id = SubagentId::new("conversation-concurrent-subagent-1");
        let second_id = SubagentId::new("conversation-concurrent-subagent-2");
        let cancellation = CancellationSignal::new();
        let (first, second) = tokio::join!(
            manager.acquire(default_isolated(), &first_id, &cancellation,),
            manager.acquire(default_isolated(), &second_id, &cancellation,)
        );
        let first = first.expect("first worktree");
        let second = second.expect("second worktree");
        assert_ne!(first.logical_workspace(), second.logical_workspace());
        assert_ne!(
            first
                .snapshot()
                .git_worktree()
                .expect("first Git worktree")
                .branch,
            second
                .snapshot()
                .git_worktree()
                .expect("second Git worktree")
                .branch
        );
        let first_settlement = first.settle_after_child().await;
        let second_settlement = second.settle_after_child().await;
        assert_eq!(first_settlement.cleanup, WorkspaceCleanup::Removed);
        assert_eq!(second_settlement.cleanup, WorkspaceCleanup::Removed);
    }

    #[tokio::test]
    async fn same_identity_collision_cannot_settle_another_lease() {
        let dir = repository();
        let runtime = tempfile::tempdir().expect("runtime root");
        let manager = SubagentWorkspaceManager::new(dir.path(), runtime.path());
        let subagent = SubagentId::new("conversation-same-identity-subagent-1");
        let first = manager
            .acquire(default_isolated(), &subagent, &CancellationSignal::new())
            .await
            .expect("first worktree");
        let path = first.logical_workspace().to_path_buf();
        let second = manager
            .acquire(default_isolated(), &subagent, &CancellationSignal::new())
            .await
            .expect_err("same deterministic identity must collide");
        assert!(matches!(
            second,
            super::WorkspaceAcquireError::Collision { .. }
        ));
        assert!(
            path.exists(),
            "the losing acquisition did not touch the winner"
        );
        assert_eq!(
            first.settle_after_child().await.cleanup,
            WorkspaceCleanup::Removed
        );
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn concurrent_same_identity_acquisition_has_one_atomic_owner() {
        let dir = repository();
        let runtime = tempfile::tempdir().expect("runtime root");
        let manager = SubagentWorkspaceManager::new(dir.path(), runtime.path());
        let subagent = SubagentId::new("conversation-concurrent-same-identity-subagent-1");
        let cancellation = CancellationSignal::new();
        let (left, right) = tokio::join!(
            manager.acquire(default_isolated(), &subagent, &cancellation,),
            manager.acquire(default_isolated(), &subagent, &cancellation,)
        );
        let (lease, error) = match (left, right) {
            (Ok(lease), Err(error)) | (Err(error), Ok(lease)) => (lease, error),
            (Ok(_), Ok(_)) => panic!("two concurrent acquisitions claimed one identity"),
            (Err(left), Err(right)) => panic!("both acquisitions failed: {left:?}; {right:?}"),
        };
        assert!(
            matches!(
                error,
                super::WorkspaceAcquireError::Collision { .. }
                    | super::WorkspaceAcquireError::Settlement { .. }
            ),
            "unexpected losing acquisition error: {error:?}"
        );
        assert_eq!(
            lease.settle_after_child().await.cleanup,
            WorkspaceCleanup::Removed
        );
    }

    /// Issue #188: the default strict policy rejects a tracked unstaged
    /// parent modification before any child worktree is created, and the
    /// typed rejection retains the exact committed `HEAD` captured before the
    /// dirty observation.
    #[tokio::test]
    async fn tracked_unstaged_parent_change_is_rejected_before_worktree_creation() {
        let dir = repository();
        let base = head(dir.path());
        std::fs::write(dir.path().join("tracked.txt"), "dirty\n").expect("dirty file");
        let manager = SubagentWorkspaceManager::new(dir.path(), dir.path().join("artifacts"));
        let error = manager
            .acquire(
                default_isolated(),
                &SubagentId::new("conversation-a-subagent-1"),
                &CancellationSignal::new(),
            )
            .await
            .expect_err("dirty parent");
        match error {
            super::WorkspaceAcquireError::DirtyParent {
                base_commit: captured,
            } => assert_eq!(
                captured, base,
                "the rejection retains the exact pre-acquisition committed HEAD"
            ),
            other => panic!("unexpected acquisition error: {other:?}"),
        }
        assert!(!dir.path().join("artifacts/worktrees").exists());
    }

    /// Issue #188: a staged/index parent change also rejects under the
    /// default strict policy, before any worktree path or ref is created.
    #[tokio::test]
    async fn staged_parent_change_is_rejected_before_worktree_creation() {
        let dir = repository();
        let base = head(dir.path());
        std::fs::write(dir.path().join("tracked.txt"), "staged dirty\n").expect("dirty file");
        git(dir.path(), &["add", "tracked.txt"]);
        let manager = SubagentWorkspaceManager::new(dir.path(), dir.path().join("artifacts"));
        let error = manager
            .acquire(
                default_isolated(),
                &SubagentId::new("conversation-a-subagent-1"),
                &CancellationSignal::new(),
            )
            .await
            .expect_err("staged parent change");
        match error {
            super::WorkspaceAcquireError::DirtyParent {
                base_commit: captured,
            } => assert_eq!(
                captured, base,
                "the rejection retains the exact pre-acquisition committed HEAD"
            ),
            other => panic!("unexpected acquisition error: {other:?}"),
        }
        assert!(!dir.path().join("artifacts/worktrees").exists());
    }

    /// Issue #188: an untracked non-ignored parent file also rejects under
    /// the default strict policy, before any worktree path or ref is created.
    #[tokio::test]
    async fn untracked_parent_file_is_rejected_before_worktree_creation() {
        let dir = repository();
        let base = head(dir.path());
        std::fs::write(dir.path().join("parent-only.txt"), "untracked\n").expect("untracked file");
        let manager = SubagentWorkspaceManager::new(dir.path(), dir.path().join("artifacts"));
        let error = manager
            .acquire(
                default_isolated(),
                &SubagentId::new("conversation-a-subagent-1"),
                &CancellationSignal::new(),
            )
            .await
            .expect_err("untracked parent file");
        match error {
            super::WorkspaceAcquireError::DirtyParent {
                base_commit: captured,
            } => assert_eq!(
                captured, base,
                "the rejection retains the exact pre-acquisition committed HEAD"
            ),
            other => panic!("unexpected acquisition error: {other:?}"),
        }
        assert!(!dir.path().join("artifacts/worktrees").exists());
    }

    /// Issue #188 ownership: the workspace layer reports a typed execution
    /// fact and nothing more. Its domain diagnostic must not name the public
    /// configuration spelling, the definition file that carries it, or the
    /// Git commands used to observe the parent — those belong to higher
    /// layers, and the model-facing remediation is rendered at the native
    /// `subagent` tool boundary.
    #[tokio::test]
    async fn the_dirty_parent_diagnostic_owns_no_configuration_or_git_vocabulary() {
        let dir = repository();
        let base = head(dir.path());
        std::fs::write(dir.path().join("tracked.txt"), "dirty\n").expect("dirty file");
        let manager = SubagentWorkspaceManager::new(dir.path(), dir.path().join("artifacts"));
        let error = manager
            .acquire(
                default_isolated(),
                &SubagentId::new("conversation-a-subagent-1"),
                &CancellationSignal::new(),
            )
            .await
            .expect_err("dirty parent");

        // The typed fact, with the exact captured commit, stays available.
        let super::WorkspaceAcquireError::DirtyParent {
            base_commit: captured,
        } = &error
        else {
            panic!("unexpected acquisition error: {error:?}");
        };
        assert_eq!(captured, &base);

        let diagnostic = error.to_string();
        for leaked in [
            "requireCleanParent",
            "subagent definition",
            "rev-parse",
            "porcelain",
        ] {
            assert!(
                !diagnostic.contains(leaked),
                "the workspace diagnostic must not own {leaked:?}: {diagnostic}"
            );
        }
        assert!(
            diagnostic.contains("uncommitted changes") && diagnostic.contains("clean-parent"),
            "the workspace diagnostic still states its own domain fact: {diagnostic}"
        );
    }

    /// Issue #188: ignored-only parent artifacts (build/cache output) never
    /// make the ordinary source workspace dirty, so the default strict
    /// policy acquires from the clean committed snapshot.
    #[tokio::test]
    async fn ignored_only_parent_artifacts_are_allowed_by_the_default_policy() {
        let dir = repository();
        ignore_target(dir.path());
        let base = head(dir.path());
        std::fs::create_dir_all(dir.path().join("target/debug")).expect("target");
        std::fs::write(dir.path().join("target/debug/generated"), "cache\n").expect("cache");
        let manager = SubagentWorkspaceManager::new(dir.path(), dir.path().join("artifacts"));
        let lease = manager
            .acquire(
                default_isolated(),
                &SubagentId::new("conversation-ignored-only-parent-subagent-1"),
                &CancellationSignal::new(),
            )
            .await
            .expect("ignored-only parent artifacts must not reject");
        let facts = lease.snapshot().git_worktree().expect("Git worktree facts");
        assert_eq!(
            facts.base_commit, base,
            "the child is based on the exact committed HEAD"
        );
        assert!(
            !facts.parent_had_uncommitted_changes,
            "ignored-only artifacts are not uncommitted source changes"
        );
        assert_eq!(
            lease.settle_after_child().await.cleanup,
            WorkspaceCleanup::Removed
        );
    }

    /// Issue #188: with an explicit `require_clean_parent = false`, a dirty
    /// parent (tracked unstaged, staged, and untracked changes) still
    /// acquires, the child base is exactly the committed HEAD captured
    /// before acquisition, and none of the parent's dirty bytes reach the
    /// child. A later parent commit does not redefine the already selected
    /// child base.
    #[tokio::test]
    async fn permissive_dirty_parent_bases_the_child_on_the_captured_commit() {
        let dir = repository();
        let base = head(dir.path());
        std::fs::write(dir.path().join("tracked.txt"), "unstaged parent\n").expect("unstaged");
        std::fs::write(dir.path().join("staged.txt"), "staged parent\n").expect("staged");
        git(dir.path(), &["add", "staged.txt"]);
        std::fs::write(dir.path().join("parent-only.txt"), "untracked\n").expect("untracked");
        let runtime = tempfile::tempdir().expect("runtime root");
        let manager = SubagentWorkspaceManager::new(dir.path(), runtime.path());
        let lease = manager
            .acquire(
                // Explicit opt-out: run from committed HEAD while ignoring
                // parent-local dirty bytes (Issue #188).
                SubagentWorkspacePolicy::GitWorktree {
                    require_clean_parent: false,
                },
                &SubagentId::new("conversation-permissive-snapshot-subagent-1"),
                &CancellationSignal::new(),
            )
            .await
            .expect("permissive dirty parent acquisition");
        let facts = lease.snapshot().git_worktree().expect("Git worktree facts");
        assert_eq!(facts.base_commit, base);
        assert!(facts.parent_had_uncommitted_changes);
        assert_eq!(
            head(lease.logical_workspace()),
            base,
            "the child worktree is created at the captured pre-acquisition HEAD"
        );
        // No dirty parent byte is copied: tracked content is the committed
        // byte, staged content is absent, and untracked files are absent.
        assert_eq!(
            std::fs::read_to_string(lease.logical_workspace().join("tracked.txt"))
                .expect("child tracked file"),
            "committed\n"
        );
        assert!(!lease.logical_workspace().join("staged.txt").exists());
        assert!(!lease.logical_workspace().join("parent-only.txt").exists());
        // A later parent commit must not redefine the already selected base.
        commit(dir.path(), "parent commits after acquisition");
        let moved_parent_head = head(dir.path());
        assert_ne!(moved_parent_head, base);
        assert_eq!(head(lease.logical_workspace()), base);
        assert_eq!(
            head(lease.logical_workspace()),
            lease
                .snapshot()
                .git_worktree()
                .expect("Git worktree facts")
                .base_commit
        );
        let settlement = lease.settle_after_child().await;
        assert_eq!(settlement.cleanup, WorkspaceCleanup::Removed);
    }
}
