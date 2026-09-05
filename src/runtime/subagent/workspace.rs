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
//! acquire selected sources beneath a stable logical-workspace handle
//! validate the complete selection and freeze bytes from retained handles
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
use std::sync::Arc;

#[cfg(unix)]
use nix::fcntl::{OFlag, open, openat};
#[cfg(unix)]
use nix::sys::stat::{Mode, mkdirat};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
#[cfg(unix)]
use std::io::{Seek, SeekFrom, Write};
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
/// The maximum diagnostic carried by a durable unresolved workspace fact.
pub(crate) const MAX_WORKSPACE_SETTLEMENT_DETAIL_BYTES: usize = 2 * 1024;

/// Acquisition-internal immutable bytes selected from the parent logical
/// workspace. This value never crosses the child/durable protocol boundary.
#[derive(Debug)]
struct FrozenOverlayFile {
    relative_path: PathBuf,
    repository_relative_path: PathBuf,
    bytes: Vec<u8>,
}

/// One selected path after every path/Git eligibility check and secure source
/// acquisition, but before selected file content is read.
#[derive(Debug)]
struct ValidatedOverlayFile {
    relative: PathBuf,
    repository_relative: PathBuf,
    /// The exact regular file object acquired beneath the stable workspace
    /// directory handle. Freezing consumes this handle; it never reopens the
    /// source pathname.
    source: std::fs::File,
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
            if !is_git_object_id(&worktree.base_commit) {
                return Err(
                    "isolated workspace snapshot has an invalid base commit identity".to_owned(),
                );
            }
            if worktree.branch.is_empty() {
                return Err("isolated workspace snapshot has no branch/ref".to_owned());
            }
            if worktree.branch.contains(['\0', '\n', '\r']) {
                return Err("isolated workspace snapshot has an invalid branch/ref".to_owned());
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
    /// The preserved logical project scope used by the child. The source
    /// repository identity and repository-relative scope remain in the
    /// paired durable `WorkspaceSnapshot`; this handoff is never a
    /// standalone arbitrary-path deletion request.
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
        if self.branch.is_empty() || self.branch.contains(['\0', '\n', '\r']) {
            return Err("workspace handoff has an invalid branch/ref".to_owned());
        }
        if !is_git_object_id(&self.base_commit) {
            return Err("workspace handoff has an invalid base commit identity".to_owned());
        }
        if !is_git_object_id(&self.head_commit) {
            return Err("workspace handoff has an invalid head commit identity".to_owned());
        }
        Ok(())
    }
}

/// The reason rustX preserved an isolated workspace without a proven handoff.
///
/// This is durable resource authority, not a diagnostic-only label. In
/// particular, nested containment uncertainty is stricter than an inspection
/// failure: Git facts alone can never authorize destructive disposal while a
/// supervised process anchor remains unresolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceUnresolvedReason {
    /// A required physical workspace fact or cleanup operation could not be
    /// proven, so the worktree may still be runtime-owned.
    PhysicalSettlement,
    /// Nested supervised-process containment was not proven settled.
    NestedContainment,
}

/// The closed physical disposition produced by one workspace lease owner.
///
/// Keeping the resource disposition in one enum prevents the old invalid
/// combination `Preserved + no handoff + error` from being projected as an
/// absent resource. `Retained` and `PreservedUnresolved` are both physical
/// preservation, but only the former carries the stronger handoff contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceSettlementDisposition {
    /// Shared workspace: there is no runtime-owned isolated worktree.
    Shared,
    /// The runtime-created clean worktree and its branch were removed.
    Removed,
    /// The physical worktree remains and its exact handoff was proven. A
    /// cleanup error is retained only as terminal diagnostic context.
    Retained {
        /// The exact runtime-observed handoff.
        handoff: WorkspaceHandoff,
        /// A failed best-effort cleanup after the handoff was proven.
        cleanup_error: Option<String>,
    },
    /// A runtime-created physical workspace may remain, but the complete
    /// handoff/cleanup proof was not established.
    PreservedUnresolved {
        /// Why later disposal must remain fail-closed until re-proven.
        reason: WorkspaceUnresolvedReason,
        /// The bounded physical settlement diagnostic.
        detail: String,
    },
}

/// The physical cleanup class derived from [`WorkspaceSettlementDisposition`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceCleanup {
    /// Shared workspace: there is no runtime-owned worktree to remove.
    Shared,
    /// The runtime-created clean worktree and its branch were removed.
    Removed,
    /// An isolated worktree remains, either with a proven handoff or
    /// conservatively unresolved.
    Preserved,
}

/// The final workspace facts produced by the one lease owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceSettlement {
    /// The immutable selection facts.
    pub snapshot: WorkspaceSnapshot,
    /// The single authoritative physical settlement disposition.
    pub disposition: WorkspaceSettlementDisposition,
}

impl WorkspaceSettlement {
    /// A settlement for a shared workspace.
    #[must_use]
    pub fn shared(snapshot: WorkspaceSnapshot) -> Self {
        Self {
            snapshot,
            disposition: WorkspaceSettlementDisposition::Shared,
        }
    }

    fn removed(snapshot: WorkspaceSnapshot) -> Self {
        Self {
            snapshot,
            disposition: WorkspaceSettlementDisposition::Removed,
        }
    }

    fn retained(snapshot: WorkspaceSnapshot, handoff: WorkspaceHandoff) -> Self {
        Self {
            snapshot,
            disposition: WorkspaceSettlementDisposition::Retained {
                handoff,
                cleanup_error: None,
            },
        }
    }

    fn retained_with_error(
        snapshot: WorkspaceSnapshot,
        handoff: WorkspaceHandoff,
        cleanup_error: impl Into<String>,
    ) -> Self {
        Self {
            snapshot,
            disposition: WorkspaceSettlementDisposition::Retained {
                handoff,
                cleanup_error: Some(bound_settlement_detail(cleanup_error.into())),
            },
        }
    }

    fn unresolved(snapshot: WorkspaceSnapshot, error: impl Into<String>) -> Self {
        Self::unresolved_with_reason(
            snapshot,
            WorkspaceUnresolvedReason::PhysicalSettlement,
            error,
        )
    }

    fn unresolved_with_reason(
        snapshot: WorkspaceSnapshot,
        reason: WorkspaceUnresolvedReason,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            snapshot,
            disposition: WorkspaceSettlementDisposition::PreservedUnresolved {
                reason,
                detail: bound_settlement_detail(detail.into()),
            },
        }
    }

    /// The proven retained handoff, when the disposition is ordinary
    /// `Retained`. Unresolved preservation deliberately returns `None`.
    #[must_use]
    pub fn handoff(&self) -> Option<&WorkspaceHandoff> {
        match &self.disposition {
            WorkspaceSettlementDisposition::Retained { handoff, .. } => Some(handoff),
            WorkspaceSettlementDisposition::Shared
            | WorkspaceSettlementDisposition::Removed
            | WorkspaceSettlementDisposition::PreservedUnresolved { .. } => None,
        }
    }

    /// The derived cleanup class. This is a projection, never an independent
    /// state that can disagree with the disposition.
    #[must_use]
    pub const fn cleanup(&self) -> WorkspaceCleanup {
        match self.disposition {
            WorkspaceSettlementDisposition::Shared => WorkspaceCleanup::Shared,
            WorkspaceSettlementDisposition::Removed => WorkspaceCleanup::Removed,
            WorkspaceSettlementDisposition::Retained { .. }
            | WorkspaceSettlementDisposition::PreservedUnresolved { .. } => {
                WorkspaceCleanup::Preserved
            }
        }
    }

    /// The physical diagnostic, if settlement was unresolved or a retained
    /// worktree's best-effort cleanup failed.
    #[must_use]
    pub fn error(&self) -> Option<&str> {
        match &self.disposition {
            WorkspaceSettlementDisposition::Retained {
                cleanup_error: Some(error),
                ..
            }
            | WorkspaceSettlementDisposition::PreservedUnresolved { detail: error, .. } => {
                Some(error)
            }
            WorkspaceSettlementDisposition::Shared
            | WorkspaceSettlementDisposition::Removed
            | WorkspaceSettlementDisposition::Retained {
                cleanup_error: None,
                ..
            } => None,
        }
    }

    /// The durable reason for unresolved preservation, when applicable.
    #[must_use]
    pub const fn unresolved_reason(&self) -> Option<WorkspaceUnresolvedReason> {
        match self.disposition {
            WorkspaceSettlementDisposition::PreservedUnresolved { reason, .. } => Some(reason),
            WorkspaceSettlementDisposition::Shared
            | WorkspaceSettlementDisposition::Removed
            | WorkspaceSettlementDisposition::Retained { .. } => None,
        }
    }

    /// Whether a physical isolated workspace may remain without a proven
    /// handoff.
    #[must_use]
    pub const fn is_unresolved(&self) -> bool {
        matches!(
            self.disposition,
            WorkspaceSettlementDisposition::PreservedUnresolved { .. }
        )
    }
}

fn bound_settlement_detail(mut detail: String) -> String {
    if detail.len() > MAX_WORKSPACE_SETTLEMENT_DETAIL_BYTES {
        let mut end = MAX_WORKSPACE_SETTLEMENT_DETAIL_BYTES;
        while !detail.is_char_boundary(end) {
            end -= 1;
        }
        detail.truncate(end);
    }
    detail
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

/// A fail-closed retained-workspace disposal failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceDisposalError {
    /// The recorded facts and the current Git state did not prove one exact
    /// runtime-owned worktree/ref relationship. No destructive command was
    /// attempted.
    OwnershipMismatch {
        /// The bounded proof failure.
        detail: String,
    },
    /// Git rejected a command after ownership proof, or could not complete
    /// the exact removal transaction.
    Git {
        /// The bounded operation name.
        operation: String,
        /// The bounded Git diagnostic.
        detail: String,
    },
}

impl core::fmt::Display for WorkspaceDisposalError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::OwnershipMismatch { detail } => {
                write!(
                    formatter,
                    "retained workspace ownership could not be proven: {detail}"
                )
            }
            Self::Git { operation, detail } => {
                write!(
                    formatter,
                    "Git retained workspace disposal {operation} failed: {detail}"
                )
            }
        }
    }
}

impl std::error::Error for WorkspaceDisposalError {}

/// The bounded physical result of one authorized retained-workspace
/// disposal attempt.
///
/// The result is intentionally not an all-or-nothing `Result`: once Git has
/// removed the worktree, that fact must be carried to the registry even when
/// compare-and-delete cannot settle the branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceDisposalSettlement {
    /// No worktree removal was completed. The durable disposal intent remains
    /// retryable and no physical retained-resource state is cleared.
    NothingRemoved {
        /// The bounded retryable Git diagnostic.
        detail: String,
    },
    /// The exact worktree is gone, but the branch is still residual or its
    /// cleanup outcome could not be proven. The branch was never blindly
    /// deleted.
    WorktreeRemoved {
        /// The bounded branch-settlement diagnostic.
        detail: String,
    },
    /// This call removed both the exact worktree and the exact expected ref.
    Disposed,
    /// Both exact physical resources were already absent under a durable
    /// disposal authorization; no destructive command was needed.
    AlreadyDisposed,
}

/// The physical phase already established by the durable resource protocol.
///
/// `Authorized` may still have an intact worktree. `WorktreeRemoved` skips
/// all worktree ownership proof and only settles the exact branch. The
/// process-local `PhysicalResourcesRemoved` phase is used when the final
/// durable settlement append failed after both physical resources were gone;
/// it is never reconstructed from filesystem absence alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorkspaceDisposalPhase {
    /// The durable intent exists, but no worktree-removal settlement is known.
    Authorized,
    /// The worktree-removal boundary has been crossed.
    WorktreeRemoved,
    /// Both physical resources were observed settled, but final durable
    /// settlement is still pending in this process.
    PhysicalResourcesRemoved,
}

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
    /// Serializes runtime-owned retained disposal with other disposal calls
    /// made through clones of this manager. Git remains the physical
    /// authority; this lock only supplies the in-process linearization.
    disposal_lock: Arc<tokio::sync::Mutex<()>>,
    #[cfg(test)]
    acquisition_hook: Option<std::sync::Arc<WorkspaceAcquireHook>>,
    #[cfg(test)]
    overlay_source_hook: Option<std::sync::Arc<WorkspaceOverlaySourceHook>>,
    #[cfg(test)]
    overlay_validation_hook: Option<std::sync::Arc<WorkspaceOverlayValidationHook>>,
    #[cfg(test)]
    overlay_materialization_hook: Option<std::sync::Arc<WorkspaceOverlayMaterializationHook>>,
    #[cfg(test)]
    overlay_freeze_hook: Option<std::sync::Arc<WorkspaceOverlayFreezeHook>>,
    #[cfg(test)]
    settlement_hook: Option<std::sync::Arc<WorkspaceSettlementHook>>,
    #[cfg(test)]
    disposal_hook: Option<std::sync::Arc<WorkspaceDisposalHook>>,
}

impl SubagentWorkspaceManager {
    /// Creates a manager over the already-canonical parent workspace and the
    /// disjoint runtime-private artifact root.
    #[must_use]
    pub fn new(parent_workspace: impl AsRef<Path>, runtime_root: impl AsRef<Path>) -> Self {
        Self {
            parent_logical_workspace: parent_workspace.as_ref().to_path_buf(),
            runtime_root: runtime_root.as_ref().to_path_buf(),
            disposal_lock: Arc::new(tokio::sync::Mutex::new(())),
            #[cfg(test)]
            acquisition_hook: None,
            #[cfg(test)]
            overlay_source_hook: None,
            #[cfg(test)]
            overlay_validation_hook: None,
            #[cfg(test)]
            overlay_materialization_hook: None,
            #[cfg(test)]
            overlay_freeze_hook: None,
            #[cfg(test)]
            settlement_hook: None,
            #[cfg(test)]
            disposal_hook: None,
        }
    }

    /// Re-proves the complete ownership relationship of one retained
    /// runtime-created worktree without mutating it.
    ///
    /// The registry uses this proof before committing the durable disposal
    /// intent. The physical operation repeats the proof after that commit,
    /// because the intent boundary and the Git mutation are separate
    /// processes.
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceDisposalError::OwnershipMismatch`] when the
    /// recorded handoff cannot be proven against the current Git repository.
    pub async fn prove_retained_workspace(
        &self,
        subagent_id: &SubagentId,
        snapshot: &WorkspaceSnapshot,
        handoff: &WorkspaceHandoff,
    ) -> Result<(), WorkspaceDisposalError> {
        let _disposal = self.disposal_lock.lock().await;
        self.verify_retained_workspace(subagent_id, snapshot, handoff)
            .await
            .map(|_| ())
    }

    /// Re-proves an unresolved isolated workspace and, only after the full
    /// current Git relationship succeeds, derives a fresh handoff for the
    /// existing resource. Absence of the path or registration is a proof
    /// failure; it is never treated as evidence that disposal already won.
    ///
    /// Nested-containment unresolved resources are rejected by the registry
    /// before this method is called. Git ownership facts cannot prove that a
    /// supervised process anchor is gone.
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceDisposalError::OwnershipMismatch`] when the
    /// unresolved workspace cannot be re-proven as the exact runtime-owned
    /// Git worktree recorded in `snapshot`.
    pub async fn reprove_unresolved_workspace(
        &self,
        subagent_id: &SubagentId,
        snapshot: &WorkspaceSnapshot,
    ) -> Result<WorkspaceHandoff, WorkspaceDisposalError> {
        let _disposal = self.disposal_lock.lock().await;
        self.verify_unresolved_workspace(subagent_id, snapshot)
            .await
    }

    /// Disposes one retained runtime-created worktree and its exact runtime
    /// branch after re-proving the complete ownership relationship.
    ///
    /// The caller supplies the authoritative subagent identity and the two
    /// runtime-owned facts captured at terminal settlement. The identity
    /// checks bind the requested resource to this manager's deterministic
    /// allocation namespace; the current repository root, worktree
    /// registration, physical `HEAD`, branch attachment, and ref value then
    /// bind the stale durable facts to what Git reports now. A proof failure
    /// returns [`WorkspaceDisposalError::OwnershipMismatch`] before any
    /// mutation. Once the proof succeeds, worktree removal is forceful by
    /// design: this is the explicit user/runtime operation that discards
    /// dirty retained source work.
    ///
    /// The physical linearization point is the exact `git worktree remove
    /// --force` command after the final proof. The following `update-ref` is
    /// a compare-and-delete using the recorded retained `HEAD`, so a ref that
    /// moved cannot be deleted. The manager lock serializes runtime calls;
    /// Git's registration/ref checks fail closed if external state changed.
    /// The typed result preserves a successful worktree removal even when
    /// later branch settlement cannot complete.
    ///
    /// # Errors
    ///
    /// Returns an ownership mismatch before mutation when any recorded or
    /// current Git fact disagrees. A failed worktree command is returned as a
    /// typed `NothingRemoved` result; a failed branch compare-delete is
    /// returned as typed `WorktreeRemoved` so the caller cannot advertise the
    /// physical worktree as retained.
    pub async fn dispose_retained_workspace(
        &self,
        subagent_id: &SubagentId,
        snapshot: &WorkspaceSnapshot,
        handoff: &WorkspaceHandoff,
    ) -> Result<WorkspaceDisposalSettlement, WorkspaceDisposalError> {
        self.dispose_authorized_workspace_inner(
            subagent_id,
            snapshot,
            handoff,
            WorkspaceDisposalPhase::Authorized,
            false,
        )
        .await
    }

    /// Continues a durable disposal intent at the exact physical phase it
    /// recorded. Missing worktree registration/path is accepted only after
    /// the durable intent has crossed this method's boundary; an ordinary
    /// retained request still uses [`Self::dispose_retained_workspace`]'s
    /// complete ownership proof.
    pub(crate) async fn dispose_authorized_workspace(
        &self,
        subagent_id: &SubagentId,
        snapshot: &WorkspaceSnapshot,
        handoff: &WorkspaceHandoff,
        phase: WorkspaceDisposalPhase,
    ) -> Result<WorkspaceDisposalSettlement, WorkspaceDisposalError> {
        self.dispose_authorized_workspace_inner(subagent_id, snapshot, handoff, phase, true)
            .await
    }

    #[allow(clippy::too_many_lines)] // One ordered physical settlement protocol.
    async fn dispose_authorized_workspace_inner(
        &self,
        subagent_id: &SubagentId,
        snapshot: &WorkspaceSnapshot,
        handoff: &WorkspaceHandoff,
        phase: WorkspaceDisposalPhase,
        durable_intent_committed: bool,
    ) -> Result<WorkspaceDisposalSettlement, WorkspaceDisposalError> {
        fn mismatch(detail: impl Into<String>) -> WorkspaceDisposalError {
            WorkspaceDisposalError::OwnershipMismatch {
                detail: detail.into(),
            }
        }
        let _disposal = self.disposal_lock.lock().await;

        snapshot.validate().map_err(mismatch)?;
        handoff.validate().map_err(&mut |detail| mismatch(detail))?;
        if !snapshot.matches_handoff(handoff) {
            return Err(mismatch(
                "the retained handoff does not match its owned workspace snapshot",
            ));
        }
        let Some(worktree) = snapshot.git_worktree() else {
            return Err(mismatch("the requested subagent has no isolated worktree"));
        };
        let expected_branch = format!(
            "rustx/subagent/{}",
            deterministic_worktree_name(subagent_id)
        );
        let expected_root = self
            .runtime_root
            .join("worktrees")
            .join(deterministic_worktree_name(subagent_id));
        if worktree.branch != expected_branch || worktree.physical_worktree_root != expected_root {
            return Err(mismatch(
                "the recorded worktree is outside this runtime's deterministic allocation",
            ));
        }

        // The source repository identity remains authoritative in every
        // phase, including the phase where the worktree itself is already
        // gone. This is what prevents a pending intent from widening into a
        // ref operation in another repository.
        self.verify_source_repository(worktree).await?;

        let worktree_removed = match phase {
            WorkspaceDisposalPhase::WorktreeRemoved
            | WorkspaceDisposalPhase::PhysicalResourcesRemoved => true,
            WorkspaceDisposalPhase::Authorized => {
                #[cfg(test)]
                if let Some(hook) = &self.disposal_hook {
                    hook.pause_before_recheck().await;
                }
                // Re-prove after the deterministic test seam and immediately
                // before mutation. The intent has already committed by the
                // time this method is called from the registry, but the
                // final proof still gates the first destructive command.
                let listing = self
                    .git_text(
                        &worktree.source_repository_root,
                        vec!["worktree".into(), "list".into(), "--porcelain".into()],
                        None,
                    )
                    .await
                    .map_err(|error| {
                        mismatch(format!(
                            "current Git worktree registration is unavailable: {error}"
                        ))
                    })?;
                let registered = worktree_listing_contains_registration(&listing, snapshot);
                let occupied = path_is_occupied(&worktree.physical_worktree_root);
                match (registered, occupied) {
                    (true, true) => {
                        // The complete proof repeats the path, registration,
                        // worktree HEAD, branch attachment, and ref checks.
                        self.verify_retained_workspace(subagent_id, snapshot, handoff)
                            .await?;
                        let removed = self
                            .git_raw(
                                &worktree.source_repository_root,
                                vec![
                                    "worktree".into(),
                                    "remove".into(),
                                    "--force".into(),
                                    "--".into(),
                                    worktree.physical_worktree_root.clone().into_os_string(),
                                ],
                                None,
                            )
                            .await;
                        match removed {
                            Ok(output) if output.status.success() => {
                                crate::runtime::process_death::reach(
                                    "after:subagent_workspace_worktree_remove",
                                );
                                #[cfg(test)]
                                if let Some(hook) = &self.disposal_hook {
                                    hook.pause_after_worktree_removal().await;
                                }
                                true
                            }
                            Ok(output) => {
                                return Ok(WorkspaceDisposalSettlement::NothingRemoved {
                                    detail: git_failure_detail(&output),
                                });
                            }
                            Err(error) => {
                                return Ok(WorkspaceDisposalSettlement::NothingRemoved {
                                    detail: error.to_string(),
                                });
                            }
                        }
                    }
                    // A durable disposal intent authorizes continuation of
                    // the exact resource when both its path and registration
                    // are gone. An ordinary direct disposal has no such
                    // authority and must fail closed instead of inferring
                    // success from absence.
                    (false, false) if durable_intent_committed => true,
                    (false, false) => {
                        return Err(mismatch(
                            "the retained worktree path and Git registration are both absent without a durable disposal intent",
                        ));
                    }
                    (true, false) => {
                        return Err(mismatch(
                            "Git still registers the retained worktree, but its exact physical path is gone",
                        ));
                    }
                    (false, true) => {
                        return Err(mismatch(
                            "the retained physical path exists without its exact Git worktree registration",
                        ));
                    }
                }
            }
        };

        debug_assert!(worktree_removed);
        self.settle_authorized_branch(&worktree.source_repository_root, handoff)
            .await
    }

    async fn verify_source_repository(
        &self,
        worktree: &GitWorktreeSnapshot,
    ) -> Result<(), WorkspaceDisposalError> {
        fn mismatch(detail: impl Into<String>) -> WorkspaceDisposalError {
            WorkspaceDisposalError::OwnershipMismatch {
                detail: detail.into(),
            }
        }
        let recorded_source =
            std::fs::canonicalize(&worktree.source_repository_root).map_err(|error| {
                mismatch(format!(
                    "recorded source repository is unavailable: {error}"
                ))
            })?;
        let current_source = self
            .git_text(
                &worktree.source_repository_root,
                vec!["rev-parse".into(), "--show-toplevel".into()],
                None,
            )
            .await
            .map_err(|error| {
                mismatch(format!(
                    "current source repository identity is unavailable: {error}"
                ))
            })?;
        let current_source = std::fs::canonicalize(current_source).map_err(|error| {
            mismatch(format!(
                "current source repository identity is invalid: {error}"
            ))
        })?;
        if current_source != recorded_source {
            return Err(mismatch(
                "the recorded source repository is not the current Git repository",
            ));
        }
        Ok(())
    }

    async fn settle_authorized_branch(
        &self,
        source_repository_root: &Path,
        handoff: &WorkspaceHandoff,
    ) -> Result<WorkspaceDisposalSettlement, WorkspaceDisposalError> {
        fn partial(detail: impl Into<String>) -> WorkspaceDisposalSettlement {
            WorkspaceDisposalSettlement::WorktreeRemoved {
                detail: detail.into(),
            }
        }
        let reference = format!("refs/heads/{}", handoff.branch);
        let exists = match self
            .git_raw(
                source_repository_root,
                vec![
                    "show-ref".into(),
                    "--verify".into(),
                    "--quiet".into(),
                    reference.clone().into(),
                ],
                None,
            )
            .await
        {
            Ok(output) => output,
            Err(error) => return Ok(partial(error.to_string())),
        };
        if !exists.status.success() {
            if exists.status.code() == Some(1) {
                return Ok(WorkspaceDisposalSettlement::AlreadyDisposed);
            }
            return Ok(partial(git_failure_detail(&exists)));
        }
        let current = match self
            .git_text(
                source_repository_root,
                vec![
                    "rev-parse".into(),
                    "--verify".into(),
                    reference.clone().into(),
                ],
                None,
            )
            .await
        {
            Ok(current) => current,
            Err(error) => return Ok(partial(error.to_string())),
        };
        if current != handoff.head_commit {
            return Ok(partial(format!(
                "runtime branch {} moved from expected commit {} to {}; it was preserved",
                handoff.branch, handoff.head_commit, current
            )));
        }
        #[cfg(test)]
        if let Some(hook) = &self.disposal_hook
            && let Some(detail) = hook.take_branch_failure()
        {
            return Ok(partial(detail));
        }
        let deleted = match self
            .git_raw(
                source_repository_root,
                vec![
                    "update-ref".into(),
                    "-d".into(),
                    reference.into(),
                    handoff.head_commit.clone().into(),
                ],
                None,
            )
            .await
        {
            Ok(output) => output,
            Err(error) => return Ok(partial(error.to_string())),
        };
        if deleted.status.success() {
            Ok(WorkspaceDisposalSettlement::Disposed)
        } else {
            Ok(partial(git_failure_detail(&deleted)))
        }
    }

    #[allow(clippy::too_many_lines)] // The ownership proof is intentionally one ordered sequence.
    async fn verify_retained_workspace(
        &self,
        subagent_id: &SubagentId,
        snapshot: &WorkspaceSnapshot,
        handoff: &WorkspaceHandoff,
    ) -> Result<GitWorktreeSnapshot, WorkspaceDisposalError> {
        fn mismatch(detail: impl Into<String>) -> WorkspaceDisposalError {
            WorkspaceDisposalError::OwnershipMismatch {
                detail: detail.into(),
            }
        }
        snapshot.validate().map_err(mismatch)?;
        handoff.validate().map_err(mismatch)?;
        if !snapshot.matches_handoff(handoff) {
            return Err(mismatch(
                "the retained handoff does not match its owned workspace snapshot",
            ));
        }
        let Some(worktree) = snapshot.git_worktree() else {
            return Err(mismatch("the requested subagent has no isolated worktree"));
        };
        let expected_branch = format!(
            "rustx/subagent/{}",
            deterministic_worktree_name(subagent_id)
        );
        let expected_root = self
            .runtime_root
            .join("worktrees")
            .join(deterministic_worktree_name(subagent_id));
        if worktree.branch != expected_branch || worktree.physical_worktree_root != expected_root {
            return Err(mismatch(
                "the recorded worktree is outside this runtime's deterministic allocation",
            ));
        }

        let recorded_source =
            std::fs::canonicalize(&worktree.source_repository_root).map_err(|error| {
                mismatch(format!(
                    "recorded source repository is unavailable: {error}"
                ))
            })?;
        let current_source = self
            .git_text(
                &worktree.source_repository_root,
                vec!["rev-parse".into(), "--show-toplevel".into()],
                None,
            )
            .await
            .map_err(|error| {
                mismatch(format!(
                    "current source repository identity is unavailable: {error}"
                ))
            })?;
        let current_source = std::fs::canonicalize(current_source).map_err(|error| {
            mismatch(format!(
                "current source repository identity is invalid: {error}"
            ))
        })?;
        if current_source != recorded_source {
            return Err(mismatch(
                "the recorded source repository is not the current Git repository",
            ));
        }

        let physical_top = self
            .git_text(
                &worktree.physical_worktree_root,
                vec!["rev-parse".into(), "--show-toplevel".into()],
                None,
            )
            .await
            .map_err(|error| {
                mismatch(format!(
                    "the recorded physical path is not a Git worktree: {error}"
                ))
            })?;
        let physical_top = std::fs::canonicalize(physical_top).map_err(|error| {
            mismatch(format!(
                "the current physical worktree identity is invalid: {error}"
            ))
        })?;
        let recorded_physical =
            std::fs::canonicalize(&worktree.physical_worktree_root).map_err(|error| {
                mismatch(format!(
                    "the recorded physical worktree is unavailable: {error}"
                ))
            })?;
        if physical_top != recorded_physical {
            return Err(mismatch(
                "the recorded physical path resolves to a different Git worktree",
            ));
        }

        let listing = self
            .git_text(
                &worktree.source_repository_root,
                vec!["worktree".into(), "list".into(), "--porcelain".into()],
                None,
            )
            .await
            .map_err(|error| {
                mismatch(format!(
                    "current Git worktree registration is unavailable: {error}"
                ))
            })?;
        if !worktree_listing_contains_handoff(&listing, snapshot, handoff) {
            return Err(mismatch(
                "current Git registration does not match the recorded worktree, branch, and HEAD",
            ));
        }

        let physical_head = self
            .git_text(
                &worktree.physical_worktree_root,
                vec!["rev-parse".into(), "HEAD".into()],
                None,
            )
            .await
            .map_err(|error| mismatch(format!("current worktree HEAD is unavailable: {error}")))?;
        if physical_head != handoff.head_commit {
            return Err(mismatch(
                "the physical worktree HEAD changed after terminal handoff",
            ));
        }

        let reference = format!("refs/heads/{}", handoff.branch);
        let branch_head = self
            .git_text(
                &worktree.source_repository_root,
                vec!["rev-parse".into(), "--verify".into(), reference.into()],
                None,
            )
            .await
            .map_err(|error| {
                mismatch(format!("the retained branch ref is unavailable: {error}"))
            })?;
        if branch_head != handoff.head_commit {
            return Err(mismatch(
                "the recorded runtime branch now names a different commit",
            ));
        }
        Ok(worktree.clone())
    }

    #[allow(clippy::too_many_lines)] // The unresolved re-proof is one ordered ownership check.
    async fn verify_unresolved_workspace(
        &self,
        subagent_id: &SubagentId,
        snapshot: &WorkspaceSnapshot,
    ) -> Result<WorkspaceHandoff, WorkspaceDisposalError> {
        fn mismatch(detail: impl Into<String>) -> WorkspaceDisposalError {
            WorkspaceDisposalError::OwnershipMismatch {
                detail: detail.into(),
            }
        }
        snapshot.validate().map_err(mismatch)?;
        let Some(worktree) = snapshot.git_worktree() else {
            return Err(mismatch("the unresolved resource has no isolated worktree"));
        };
        let expected_branch = format!(
            "rustx/subagent/{}",
            deterministic_worktree_name(subagent_id)
        );
        let expected_root = self
            .runtime_root
            .join("worktrees")
            .join(deterministic_worktree_name(subagent_id));
        if worktree.branch != expected_branch || worktree.physical_worktree_root != expected_root {
            return Err(mismatch(
                "the unresolved worktree is outside this runtime's deterministic allocation",
            ));
        }

        self.verify_source_repository(worktree).await?;
        let physical_top = self
            .git_text(
                &worktree.physical_worktree_root,
                vec!["rev-parse".into(), "--show-toplevel".into()],
                None,
            )
            .await
            .map_err(|error| {
                mismatch(format!(
                    "the unresolved physical path is not a Git worktree: {error}"
                ))
            })?;
        let physical_top = std::fs::canonicalize(physical_top).map_err(|error| {
            mismatch(format!(
                "the current unresolved worktree identity is invalid: {error}"
            ))
        })?;
        let recorded_physical =
            std::fs::canonicalize(&worktree.physical_worktree_root).map_err(|error| {
                mismatch(format!(
                    "the unresolved physical worktree is unavailable: {error}"
                ))
            })?;
        if physical_top != recorded_physical {
            return Err(mismatch(
                "the unresolved physical path resolves to a different Git worktree",
            ));
        }

        let listing = self
            .git_text(
                &worktree.source_repository_root,
                vec!["worktree".into(), "list".into(), "--porcelain".into()],
                None,
            )
            .await
            .map_err(|error| {
                mismatch(format!(
                    "current Git worktree registration is unavailable: {error}"
                ))
            })?;
        if !worktree_listing_contains_registration(&listing, snapshot) {
            return Err(mismatch(
                "current Git registration does not match the unresolved worktree path",
            ));
        }

        let physical_head = self
            .git_text(
                &worktree.physical_worktree_root,
                vec!["rev-parse".into(), "HEAD".into()],
                None,
            )
            .await
            .map_err(|error| {
                mismatch(format!(
                    "current unresolved worktree HEAD is unavailable: {error}"
                ))
            })?;
        let reference = format!("refs/heads/{}", worktree.branch);
        let branch_head = self
            .git_text(
                &worktree.source_repository_root,
                vec!["rev-parse".into(), "--verify".into(), reference.into()],
                None,
            )
            .await
            .map_err(|error| {
                mismatch(format!(
                    "the unresolved runtime branch ref is unavailable: {error}"
                ))
            })?;
        if branch_head != physical_head {
            return Err(mismatch(
                "the unresolved worktree HEAD and runtime branch no longer match",
            ));
        }
        // An unresolved settlement has no durable terminal handoff HEAD.
        // The immutable acquisition snapshot can therefore authorize a later
        // disposal only while both current heads remain at the selected base.
        // A changed commit may be legitimate child work, but without a proven
        // handoff rustX cannot distinguish it from an external ref mutation.
        if physical_head != worktree.base_commit || branch_head != worktree.base_commit {
            return Err(mismatch(
                "the unresolved worktree changed from its immutable base without a durable handoff",
            ));
        }
        let status = self
            .git_text(
                &worktree.physical_worktree_root,
                ordinary_workspace_status_args(),
                None,
            )
            .await
            .map_err(|error| {
                mismatch(format!(
                    "current unresolved worktree status is unavailable: {error}"
                ))
            })?;
        let (dirty, _) =
            workspace_change_facts(Some(&worktree.base_commit), &physical_head, &status);
        let handoff = WorkspaceHandoff {
            logical_workspace: snapshot.logical_workspace.clone(),
            physical_worktree_root: worktree.physical_worktree_root.clone(),
            branch: worktree.branch.clone(),
            base_commit: worktree.base_commit.clone(),
            head_commit: physical_head,
            dirty,
        };
        if !worktree_listing_contains_handoff(&listing, snapshot, &handoff) {
            return Err(mismatch(
                "current Git registration does not match the unresolved worktree branch and HEAD",
            ));
        }
        Ok(handoff)
    }

    /// Installs a test-only barrier after worktree and overlay verification,
    /// immediately before the prepared lease can leave acquisition.
    #[cfg(test)]
    pub(crate) fn install_acquisition_hook(&mut self, hook: std::sync::Arc<WorkspaceAcquireHook>) {
        self.acquisition_hook = Some(hook);
    }

    /// Installs a test-only barrier immediately before selected source files
    /// are securely acquired beneath the stable logical-workspace handle.
    #[cfg(test)]
    fn install_overlay_source_hook(&mut self, hook: std::sync::Arc<WorkspaceOverlaySourceHook>) {
        self.overlay_source_hook = Some(hook);
    }

    /// Installs a test-only barrier after all selected source handles and Git
    /// eligibility facts are ready, immediately before byte freezing.
    #[cfg(test)]
    fn install_overlay_validation_hook(
        &mut self,
        hook: std::sync::Arc<WorkspaceOverlayValidationHook>,
    ) {
        self.overlay_validation_hook = Some(hook);
    }

    /// Installs a test-only barrier after the first frozen overlay file has
    /// been materialized and before the remaining files are attempted.
    #[cfg(test)]
    fn install_overlay_materialization_hook(
        &mut self,
        hook: std::sync::Arc<WorkspaceOverlayMaterializationHook>,
    ) {
        self.overlay_materialization_hook = Some(hook);
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

    /// Installs the deterministic re-proof seam used by disposal race tests.
    #[cfg(test)]
    pub(crate) fn install_disposal_hook(&mut self, hook: std::sync::Arc<WorkspaceDisposalHook>) {
        self.disposal_hook = Some(hook);
    }

    /// Acquires one workspace according to the resolved named-agent policy.
    ///
    /// For a Git worktree, the operations are deliberately ordered as
    /// repository resolution, exact `HEAD` capture, parent status observation,
    /// strict-policy enforcement, stable-handle overlay source
    /// acquisition/validation/freezing, worktree creation at the captured
    /// commit, and frozen-overlay materialization and verification. A
    /// cancellation during an in-flight Git command kills that command and
    /// settles any partially created worktree without force deleting dirty
    /// state.
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
        let overlay_workspace = open_stable_directory(&canonical_parent_logical_workspace).map_err(
            |error| WorkspaceAcquireError::OverlayManifest {
                detail: format!(
                    "cannot open the authoritative logical workspace for overlay acquisition: {error}"
                ),
            },
        )?;
        let overlay_selection = self
            .resolve_overlay_selection(
                &overlay_workspace,
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
        let stable_runtime_worktrees = if frozen_overlay.is_empty() {
            None
        } else {
            Some(
                open_stable_runtime_worktrees(&self.runtime_root).map_err(|error| {
                    WorkspaceAcquireError::OverlayMaterialization {
                        path: PathBuf::new(),
                        detail: format!(
                            "cannot open the runtime worktree root as a stable destination: {error}"
                        ),
                    }
                })?,
            )
        };

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
        if !frozen_overlay.is_empty() {
            let stable_runtime_worktrees = stable_runtime_worktrees
                .as_ref()
                .expect("non-empty frozen overlay has a stable runtime root");
            if let Err(error) = self
                .materialize_frozen_overlay(
                    &lease.snapshot,
                    stable_runtime_worktrees,
                    &frozen_overlay,
                    cancellation,
                )
                .await
            {
                return Err(self.settle_acquisition_failure(lease, error).await);
            }
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

    /// Resolves the one logical-workspace manifest, securely acquires every
    /// selected source object, and validates the complete selection before any
    /// selected file content is read.
    async fn resolve_overlay_selection(
        &self,
        parent_logical_workspace: &std::fs::File,
        source_repository_root: &Path,
        repository_relative_workspace: &Path,
        base_commit: &str,
        cancellation: &CancellationSignal,
    ) -> Result<Vec<ValidatedOverlayFile>, WorkspaceAcquireError> {
        let selected = read_overlay_manifest(parent_logical_workspace)?;
        // Acquire every source object before any Git eligibility check. This
        // keeps path validation and object acquisition in one handle-relative
        // operation while still deferring all content reads until the whole
        // selection is eligible.
        #[cfg(test)]
        if let Some(hook) = &self.overlay_source_hook {
            hook.pause_before_source_acquisition().await;
        }

        let mut validated = Vec::with_capacity(selected.len());
        #[cfg(unix)]
        let mut source_objects = BTreeSet::new();
        for relative_path in selected {
            if cancellation.is_cancelled() {
                return Err(WorkspaceAcquireError::Cancelled);
            }
            let repository_relative_path = repository_relative_workspace.join(&relative_path);
            let source = open_overlay_source(parent_logical_workspace, &relative_path)
                .map_err(|error| overlay_source_open_error(&relative_path, &error))?;
            let metadata =
                source
                    .metadata()
                    .map_err(|error| WorkspaceAcquireError::OverlayFreeze {
                        path: relative_path.clone(),
                        detail: error.to_string(),
                    })?;
            if !metadata.is_file() {
                return Err(WorkspaceAcquireError::OverlayNotFile {
                    path: relative_path,
                });
            }
            #[cfg(unix)]
            if !source_objects.insert(overlay_source_identity(&metadata)) {
                return Err(WorkspaceAcquireError::OverlayDuplicate {
                    path: relative_path,
                });
            }
            validated.push(ValidatedOverlayFile {
                relative: relative_path,
                repository_relative: repository_relative_path,
                source,
            });
        }

        for file in &validated {
            if cancellation.is_cancelled() {
                return Err(WorkspaceAcquireError::Cancelled);
            }
            if self
                .overlay_path_is_tracked(
                    source_repository_root,
                    base_commit,
                    &file.repository_relative,
                    cancellation,
                )
                .await?
            {
                return Err(WorkspaceAcquireError::OverlayTracked {
                    path: file.relative.clone(),
                });
            }
            if !self
                .overlay_path_is_ignored(
                    source_repository_root,
                    &file.repository_relative,
                    cancellation,
                )
                .await?
            {
                return Err(WorkspaceAcquireError::OverlayNotIgnored {
                    path: file.relative.clone(),
                });
            }
        }
        #[cfg(test)]
        if let Some(hook) = &self.overlay_validation_hook {
            hook.pause_after_source_validation().await;
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
    /// only frozen bytes through a stable child-workspace handle, and verifies
    /// each destination from the file object that was created.
    async fn materialize_frozen_overlay(
        &self,
        snapshot: &WorkspaceSnapshot,
        stable_runtime_worktrees: &std::fs::File,
        frozen: &[FrozenOverlayFile],
        cancellation: &CancellationSignal,
    ) -> Result<(), WorkspaceAcquireError> {
        let worktree = snapshot
            .git_worktree()
            .expect("isolated acquisition has Git worktree facts");
        let physical_worktree_root = worktree.physical_worktree_root.as_path();
        let destination_root = open_stable_child_logical_workspace(
            stable_runtime_worktrees,
            &worktree.physical_worktree_root,
            &worktree.repository_relative_workspace,
        )
        .map_err(|error| WorkspaceAcquireError::OverlayMaterialization {
            path: PathBuf::new(),
            detail: format!(
                "cannot open the child logical workspace as a stable destination: {error}"
            ),
        })?;
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
            validate_overlay_destination(&destination_root, &file.relative_path)?;
        }
        #[cfg(test)]
        let mut first_materialization = true;
        for file in frozen {
            if cancellation.is_cancelled() {
                return Err(WorkspaceAcquireError::Cancelled);
            }
            materialize_overlay_file(&destination_root, &file.relative_path, &file.bytes).map_err(
                |error| WorkspaceAcquireError::OverlayMaterialization {
                    path: file.relative_path.clone(),
                    detail: error.to_string(),
                },
            )?;
            #[cfg(test)]
            if first_materialization && let Some(hook) = &self.overlay_materialization_hook {
                first_materialization = false;
                hook.pause_after_first_materialization().await;
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
            disposition: WorkspaceSettlementDisposition::Retained {
                handoff: WorkspaceHandoff {
                    logical_workspace: snapshot.logical_workspace.clone(),
                    physical_worktree_root: worktree.physical_worktree_root.clone(),
                    branch,
                    base_commit,
                    head_commit: head,
                    dirty,
                },
                cleanup_error: None,
            },
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
        if !self.snapshot.is_isolated() {
            // Nested containment is still reported by the caller as a
            // physical settlement failure, but a shared workspace has no
            // runtime-created worktree whose ownership can be preserved.
            return WorkspaceSettlement::shared(self.snapshot);
        }
        WorkspaceSettlement::unresolved_with_reason(
            self.snapshot,
            WorkspaceUnresolvedReason::NestedContainment,
            detail,
        )
    }

    /// Settles a lease that never crossed durable child ownership.  Any dirty
    /// or otherwise unproven state is returned as an error and retained.
    pub(crate) async fn settle_staged(
        self,
    ) -> Result<WorkspaceSettlement, WorkspaceSettlementError> {
        let settlement = self.settle().await;
        if settlement.cleanup() == WorkspaceCleanup::Preserved {
            let detail = settlement.error().map_or_else(
                || {
                    "staged workspace is dirty or has a committed child change; it was preserved"
                        .to_owned()
                },
                str::to_owned,
            );
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
        WorkspaceSettlement::removed(snapshot)
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
            return WorkspaceSettlement::retained(snapshot, handoff);
        }
        match self.manager.remove_clean_worktree(&snapshot).await {
            Ok(()) => WorkspaceSettlement::removed(snapshot),
            Err(error) => WorkspaceSettlement::retained_with_error(snapshot, handoff, error),
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

/// Git object names emitted by `rev-parse` are full SHA-1 or SHA-256 hex
/// identities. Requiring that closed shape keeps a malformed durable fact
/// from passing the later branch/worktree equality checks merely because its
/// other fields happen to line up.
fn is_git_object_id(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
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

/// Reads the securely acquired source objects into the immutable, bounded
/// acquisition representation. The final byte bound is enforced against the
/// bytes actually read from each retained file handle, not only its metadata.
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
        let metadata =
            file.source
                .metadata()
                .map_err(|error| WorkspaceAcquireError::OverlayFreeze {
                    path: file.relative.clone(),
                    detail: error.to_string(),
                })?;
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
        let remaining = MAX_OVERLAY_BYTES - total_bytes;
        let read_limit = u64::try_from(remaining.saturating_add(1)).unwrap_or(u64::MAX);
        let mut bytes = Vec::with_capacity(metadata_bytes.min(remaining));
        std::io::Read::read_to_end(&mut file.source.take(read_limit), &mut bytes).map_err(
            |error| WorkspaceAcquireError::OverlayFreeze {
                path: file.relative.clone(),
                detail: error.to_string(),
            },
        )?;
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

/// Opens a directory from the filesystem root, retaining every directory
/// handle in the traversal. The resulting handle is the authority for all
/// later overlay source and destination operations.
fn open_stable_directory(path: &Path) -> std::io::Result<std::fs::File> {
    #[cfg(unix)]
    {
        if !path.is_absolute() {
            return Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                "stable directory paths must be absolute",
            ));
        }
        let mut current = std::fs::File::from(
            open(
                Path::new("/"),
                OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
                Mode::empty(),
            )
            .map_err(std::io::Error::from)?,
        );
        for component in path.components() {
            match component {
                std::path::Component::RootDir | std::path::Component::CurDir => {}
                std::path::Component::Normal(name) => {
                    current = open_directory_at(&current, name)?;
                }
                std::path::Component::ParentDir | std::path::Component::Prefix(_) => {
                    return Err(std::io::Error::new(
                        ErrorKind::InvalidInput,
                        "stable directory paths must not contain parent or prefix components",
                    ));
                }
            }
        }
        Ok(current)
    }
    #[cfg(not(unix))]
    {
        std::fs::File::open(path)
    }
}

/// Opens the runtime-private worktree allocation directory through a stable
/// handle. The runtime root is an application-owned path, so its canonical
/// spelling is resolved once here to accommodate platform aliases such as
/// macOS `/var`; the child worktree itself is still traversed without
/// symlinks from this retained handle.
fn open_stable_runtime_worktrees(runtime_root: &Path) -> std::io::Result<std::fs::File> {
    let canonical_runtime_root = std::fs::canonicalize(runtime_root)?;
    let runtime = open_stable_directory(&canonical_runtime_root)?;
    #[cfg(unix)]
    {
        open_directory_at(&runtime, std::ffi::OsStr::new("worktrees"))
    }
    #[cfg(not(unix))]
    {
        let _ = runtime;
        std::fs::File::open(canonical_runtime_root.join("worktrees"))
    }
}

/// Opens the child logical workspace by walking from the retained runtime
/// allocation handle. This avoids re-resolving a possibly aliased physical
/// pathname and keeps every destination component under the exact child
/// worktree directory Git registered.
fn open_stable_child_logical_workspace(
    stable_runtime_worktrees: &std::fs::File,
    physical_worktree_root: &Path,
    repository_relative_workspace: &Path,
) -> std::io::Result<std::fs::File> {
    #[cfg(unix)]
    {
        let token = physical_worktree_root.file_name().ok_or_else(|| {
            std::io::Error::new(
                ErrorKind::InvalidInput,
                "the physical worktree path has no final component",
            )
        })?;
        let worktree = open_directory_at(stable_runtime_worktrees, token)?;
        open_directory_relative(&worktree, repository_relative_workspace)
    }
    #[cfg(not(unix))]
    {
        let _ = (
            stable_runtime_worktrees,
            physical_worktree_root,
            repository_relative_workspace,
        );
        Err(std::io::Error::new(
            ErrorKind::Unsupported,
            "secure overlay destinations are only available on Unix",
        ))
    }
}

#[cfg(unix)]
fn open_directory_relative(
    root: &std::fs::File,
    relative_path: &Path,
) -> std::io::Result<std::fs::File> {
    let mut current = root.try_clone()?;
    for component in relative_path.components() {
        let std::path::Component::Normal(name) = component else {
            return Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                "relative directory path contains an unsafe component",
            ));
        };
        current = open_directory_at(&current, name)?;
    }
    Ok(current)
}

/// Opens one selected source by traversing every component relative to the
/// stable logical-workspace handle. Every intermediate directory and the
/// final file reject symlinks; the returned file is the source object later
/// consumed by [`freeze_overlay`].
fn open_overlay_source(
    workspace: &std::fs::File,
    relative_path: &Path,
) -> std::io::Result<std::fs::File> {
    #[cfg(unix)]
    {
        let mut components = relative_path.components();
        let Some(first) = components.next() else {
            return Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                "overlay source path has no file component",
            ));
        };
        let mut current = workspace.try_clone()?;
        let mut final_component = first;
        for component in components {
            let std::path::Component::Normal(name) = final_component else {
                return Err(std::io::Error::new(
                    ErrorKind::InvalidInput,
                    "overlay source path contains a non-normal component",
                ));
            };
            current = open_directory_at(&current, name)?;
            final_component = component;
        }
        let std::path::Component::Normal(name) = final_component else {
            return Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                "overlay source path contains a non-normal component",
            ));
        };
        let fd = openat(
            &current,
            name,
            OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC | OFlag::O_NONBLOCK,
            Mode::empty(),
        )
        .map_err(std::io::Error::from)?;
        Ok(std::fs::File::from(fd))
    }
    #[cfg(not(unix))]
    {
        let _ = (workspace, relative_path);
        Err(std::io::Error::new(
            ErrorKind::Unsupported,
            "secure overlay source acquisition is only available on Unix",
        ))
    }
}

#[cfg(unix)]
fn open_directory_at(
    directory: &std::fs::File,
    name: &std::ffi::OsStr,
) -> std::io::Result<std::fs::File> {
    let fd = openat(
        directory,
        name,
        // `O_NOFOLLOW` rejects a symlink on the component itself. Open
        // without `O_DIRECTORY` so platforms that report a symlink plus
        // `O_DIRECTORY` as `ENOTDIR` preserve the distinct symlink error;
        // `O_NONBLOCK` prevents an unexpected FIFO from blocking before the
        // descriptor metadata is checked below.
        OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC | OFlag::O_NONBLOCK,
        Mode::empty(),
    )
    .map_err(std::io::Error::from)?;
    let opened = std::fs::File::from(fd);
    if !opened.metadata()?.is_dir() {
        return Err(std::io::Error::new(
            ErrorKind::NotADirectory,
            "opened component is not a directory",
        ));
    }
    Ok(opened)
}

fn overlay_source_open_error(path: &Path, error: &std::io::Error) -> WorkspaceAcquireError {
    if error.kind() == ErrorKind::NotFound {
        WorkspaceAcquireError::OverlayMissing {
            path: path.to_path_buf(),
        }
    } else if is_symlink_error(error) {
        WorkspaceAcquireError::OverlaySymlink {
            path: path.to_path_buf(),
        }
    } else if error.kind() == ErrorKind::NotADirectory {
        WorkspaceAcquireError::OverlayNotFile {
            path: path.to_path_buf(),
        }
    } else {
        WorkspaceAcquireError::OverlayFreeze {
            path: path.to_path_buf(),
            detail: error.to_string(),
        }
    }
}

#[cfg(unix)]
fn is_symlink_error(error: &std::io::Error) -> bool {
    error.raw_os_error() == Some(libc::ELOOP)
}

#[cfg(not(unix))]
fn is_symlink_error(_error: &std::io::Error) -> bool {
    false
}

#[cfg(unix)]
fn overlay_source_identity(metadata: &std::fs::Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt;

    (metadata.dev(), metadata.ino())
}

/// Reads and parses `<logical-workspace>/.worktreeinclude`. A missing
/// manifest is the one no-overlay representation. Each nonblank,
/// non-comment line is one exact file path; surrounding whitespace is not
/// part of the path, `#` starts a comment only as the first trimmed byte, and
/// there is no glob, negation, escaping, or directory syntax in v1.
fn read_overlay_manifest(
    parent_logical_workspace: &std::fs::File,
) -> Result<Vec<PathBuf>, WorkspaceAcquireError> {
    let source = match open_overlay_source(
        parent_logical_workspace,
        Path::new(WORKTREE_INCLUDE_MANIFEST),
    ) {
        Ok(source) => source,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) if is_symlink_error(&error) => {
            return Err(WorkspaceAcquireError::OverlayManifest {
                detail: "the manifest must not be a symlink".to_owned(),
            });
        }
        Err(error) => {
            return Err(WorkspaceAcquireError::OverlayManifest {
                detail: error.to_string(),
            });
        }
    };
    let metadata = source
        .metadata()
        .map_err(|error| WorkspaceAcquireError::OverlayManifest {
            detail: error.to_string(),
        })?;
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

/// Checks the committed checkout through the stable child directory handle
/// before any overlay destination is created. This is an early rejection
/// optimization only; the subsequent `create_new` open remains authoritative.
fn validate_overlay_destination(
    child_logical_workspace: &std::fs::File,
    relative_path: &Path,
) -> Result<(), WorkspaceAcquireError> {
    #[cfg(unix)]
    {
        let Some(file_name) = relative_path.file_name() else {
            return Err(WorkspaceAcquireError::OverlayMaterialization {
                path: relative_path.to_path_buf(),
                detail: "the destination has no file component".to_owned(),
            });
        };
        let mut current = child_logical_workspace.try_clone().map_err(|error| {
            WorkspaceAcquireError::OverlayMaterialization {
                path: relative_path.to_path_buf(),
                detail: error.to_string(),
            }
        })?;
        if let Some(parent) = relative_path.parent() {
            for component in parent.components() {
                let std::path::Component::Normal(name) = component else {
                    return Err(WorkspaceAcquireError::OverlayMaterialization {
                        path: relative_path.to_path_buf(),
                        detail: "the destination has an unsafe path component".to_owned(),
                    });
                };
                match open_directory_at(&current, name) {
                    Ok(next) => current = next,
                    Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
                    Err(error) if is_symlink_error(&error) => {
                        return Err(WorkspaceAcquireError::OverlayMaterialization {
                            path: relative_path.to_path_buf(),
                            detail: "the destination contains a symlink".to_owned(),
                        });
                    }
                    Err(error) => {
                        return Err(WorkspaceAcquireError::OverlayMaterialization {
                            path: relative_path.to_path_buf(),
                            detail: error.to_string(),
                        });
                    }
                }
            }
        }
        match openat(
            &current,
            file_name,
            OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC | OFlag::O_NONBLOCK,
            Mode::empty(),
        ) {
            Ok(_) => Err(WorkspaceAcquireError::OverlayMaterialization {
                path: relative_path.to_path_buf(),
                detail: "the destination already exists".to_owned(),
            }),
            Err(error) => {
                let error = std::io::Error::from(error);
                if error.kind() == ErrorKind::NotFound {
                    Ok(())
                } else if is_symlink_error(&error) {
                    Err(WorkspaceAcquireError::OverlayMaterialization {
                        path: relative_path.to_path_buf(),
                        detail: "the destination contains a symlink".to_owned(),
                    })
                } else {
                    Err(WorkspaceAcquireError::OverlayMaterialization {
                        path: relative_path.to_path_buf(),
                        detail: error.to_string(),
                    })
                }
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = child_logical_workspace;
        Err(WorkspaceAcquireError::OverlayMaterialization {
            path: relative_path.to_path_buf(),
            detail: "secure overlay destinations are only available on Unix".to_owned(),
        })
    }
}

#[cfg(unix)]
fn open_overlay_destination_parent(
    child_logical_workspace: &std::fs::File,
    relative_path: &Path,
) -> std::io::Result<std::fs::File> {
    let mut current = child_logical_workspace.try_clone()?;
    let Some(parent) = relative_path.parent() else {
        return Ok(current);
    };
    for component in parent.components() {
        let std::path::Component::Normal(name) = component else {
            return Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                "the destination has an unsafe path component",
            ));
        };
        match open_directory_at(&current, name) {
            Ok(next) => current = next,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                match mkdirat(&current, name, Mode::from_bits_truncate(0o777)) {
                    Ok(()) => {}
                    Err(error) if is_exists_error(&std::io::Error::from(error)) => {}
                    Err(error) => return Err(std::io::Error::from(error)),
                }
                current = open_directory_at(&current, name)?;
            }
            Err(error) => return Err(error),
        }
    }
    Ok(current)
}

#[cfg(unix)]
fn materialize_overlay_file(
    child_logical_workspace: &std::fs::File,
    relative_path: &Path,
    bytes: &[u8],
) -> std::io::Result<()> {
    let Some(file_name) = relative_path.file_name() else {
        return Err(std::io::Error::new(
            ErrorKind::InvalidInput,
            "the destination has no file component",
        ));
    };
    let parent = open_overlay_destination_parent(child_logical_workspace, relative_path)?;
    let fd = openat(
        &parent,
        file_name,
        OFlag::O_RDWR | OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
        Mode::from_bits_truncate(0o666),
    )
    .map_err(std::io::Error::from)?;
    let mut output = std::fs::File::from(fd);
    output.write_all(bytes)?;
    output.flush()?;
    output.seek(SeekFrom::Start(0))?;
    let mut observed = Vec::with_capacity(bytes.len());
    let read_limit = u64::try_from(bytes.len().saturating_add(1)).unwrap_or(u64::MAX);
    output.take(read_limit).read_to_end(&mut observed)?;
    if observed != bytes {
        return Err(std::io::Error::new(
            ErrorKind::InvalidData,
            "destination bytes do not match the frozen source",
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
fn materialize_overlay_file(
    _child_logical_workspace: &std::fs::File,
    _relative_path: &Path,
    _bytes: &[u8],
) -> std::io::Result<()> {
    Err(std::io::Error::new(
        ErrorKind::Unsupported,
        "secure overlay destinations are only available on Unix",
    ))
}

#[cfg(unix)]
fn is_exists_error(error: &std::io::Error) -> bool {
    error.raw_os_error() == Some(libc::EEXIST)
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

/// Checks a current Git worktree listing against the complete retained
/// handoff. The registration path must be the recorded runtime allocation,
/// allowing only the fixed `/var` <-> `/private/var` spelling that macOS Git
/// can introduce. Arbitrary symlink resolution is intentionally not accepted
/// here: a stale or symlink-rebound recorded path is not an ownership proof.
fn worktree_listing_contains_handoff(
    listing: &str,
    snapshot: &WorkspaceSnapshot,
    handoff: &WorkspaceHandoff,
) -> bool {
    let Some(worktree) = snapshot.git_worktree() else {
        return false;
    };
    let expected_branch = format!("refs/heads/{}", handoff.branch);
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
        if git_registration_paths_match(
            path.as_deref(),
            Some(worktree.physical_worktree_root.as_path()),
        ) && head == Some(handoff.head_commit.as_str())
            && branch == Some(expected_branch.as_str())
        {
            return true;
        }
        path = None;
        head = None;
        branch = None;
    }
    false
}

/// Compares a Git-reported worktree path with the immutable runtime path.
/// macOS exposes `/var` as a symlink to `/private/var`, and recent Git
/// versions may choose either spelling when rendering `worktree list`. That
/// OS-owned alias is safe to normalize lexically; resolving arbitrary
/// symlinks would turn path identity into a mutable filesystem claim.
fn git_registration_paths_match(actual: Option<&Path>, expected: Option<&Path>) -> bool {
    let (Some(actual), Some(expected)) = (actual, expected) else {
        return actual == expected;
    };
    actual == expected
        || normalize_platform_git_path(actual) == normalize_platform_git_path(expected)
}

fn normalize_platform_git_path(path: &Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        let Some(rest) = path.strip_prefix("/private").ok() else {
            return path.to_path_buf();
        };
        if rest.starts_with("/var") {
            return rest.to_path_buf();
        }
    }
    path.to_path_buf()
}

/// Checks only whether the exact runtime allocation path is still registered
/// in Git. The complete retained-handoff proof is deliberately performed
/// separately once both registration and path occupancy are present.
fn worktree_listing_contains_registration(listing: &str, snapshot: &WorkspaceSnapshot) -> bool {
    let Some(worktree) = snapshot.git_worktree() else {
        return false;
    };
    listing.lines().any(|line| {
        line.strip_prefix("worktree ").is_some_and(|path| {
            paths_refer_to_same_worktree(
                Some(Path::new(path)),
                Some(worktree.physical_worktree_root.as_path()),
            )
        })
    })
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
struct WorkspaceOverlaySourceHook {
    before_acquisition: tokio::sync::Barrier,
    release: tokio::sync::Barrier,
}

#[cfg(test)]
impl WorkspaceOverlaySourceHook {
    fn new() -> Self {
        Self {
            before_acquisition: tokio::sync::Barrier::new(2),
            release: tokio::sync::Barrier::new(2),
        }
    }

    async fn pause_before_source_acquisition(&self) {
        self.before_acquisition.wait().await;
        self.release.wait().await;
    }

    async fn wait_until_ready_to_acquire(&self) {
        self.before_acquisition.wait().await;
    }

    async fn release(&self) {
        self.release.wait().await;
    }
}

#[cfg(test)]
#[derive(Debug)]
struct WorkspaceOverlayValidationHook {
    after_validation: tokio::sync::Barrier,
    release: tokio::sync::Barrier,
}

#[cfg(test)]
impl WorkspaceOverlayValidationHook {
    fn new() -> Self {
        Self {
            after_validation: tokio::sync::Barrier::new(2),
            release: tokio::sync::Barrier::new(2),
        }
    }

    async fn pause_after_source_validation(&self) {
        self.after_validation.wait().await;
        self.release.wait().await;
    }

    async fn wait_until_validated(&self) {
        self.after_validation.wait().await;
    }

    async fn release(&self) {
        self.release.wait().await;
    }
}

#[cfg(test)]
#[derive(Debug)]
struct WorkspaceOverlayMaterializationHook {
    after_first: tokio::sync::Barrier,
    release: tokio::sync::Barrier,
}

#[cfg(test)]
impl WorkspaceOverlayMaterializationHook {
    fn new() -> Self {
        Self {
            after_first: tokio::sync::Barrier::new(2),
            release: tokio::sync::Barrier::new(2),
        }
    }

    async fn pause_after_first_materialization(&self) {
        self.after_first.wait().await;
        self.release.wait().await;
    }

    async fn wait_until_after_first(&self) {
        self.after_first.wait().await;
    }

    async fn release(&self) {
        self.release.wait().await;
    }
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
#[derive(Debug)]
pub(crate) struct WorkspaceDisposalHook {
    before_recheck: tokio::sync::Barrier,
    release: tokio::sync::Barrier,
    before_recheck_armed: std::sync::atomic::AtomicBool,
    after_worktree_removal: tokio::sync::Barrier,
    after_worktree_removal_release: tokio::sync::Barrier,
    after_worktree_removal_armed: std::sync::atomic::AtomicBool,
    branch_failure: std::sync::Mutex<Option<String>>,
}

#[cfg(test)]
impl WorkspaceDisposalHook {
    pub(crate) fn new() -> Self {
        Self {
            before_recheck: tokio::sync::Barrier::new(2),
            release: tokio::sync::Barrier::new(2),
            before_recheck_armed: std::sync::atomic::AtomicBool::new(false),
            after_worktree_removal: tokio::sync::Barrier::new(2),
            after_worktree_removal_release: tokio::sync::Barrier::new(2),
            after_worktree_removal_armed: std::sync::atomic::AtomicBool::new(false),
            branch_failure: std::sync::Mutex::new(None),
        }
    }

    async fn pause_before_recheck(&self) {
        if !self
            .before_recheck_armed
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return;
        }
        self.before_recheck.wait().await;
        self.release.wait().await;
    }

    pub(crate) fn arm_before_recheck(&self) {
        self.before_recheck_armed
            .store(true, std::sync::atomic::Ordering::Release);
    }

    pub(crate) async fn wait_until_verified(&self) {
        self.before_recheck.wait().await;
    }

    pub(crate) async fn release(&self) {
        self.release.wait().await;
    }

    /// Arms a barrier after `git worktree remove --force` has succeeded and
    /// before branch compare-delete begins. The barrier is inert unless a
    /// test explicitly waits on it, so existing disposal tests do not block.
    pub(crate) async fn pause_after_worktree_removal(&self) {
        if self
            .after_worktree_removal_armed
            .load(std::sync::atomic::Ordering::Acquire)
        {
            self.after_worktree_removal.wait().await;
            self.after_worktree_removal_release.wait().await;
        }
    }

    pub(crate) fn arm_after_worktree_removal(&self) {
        self.after_worktree_removal_armed
            .store(true, std::sync::atomic::Ordering::Release);
    }

    pub(crate) async fn wait_until_worktree_removed(&self) {
        self.after_worktree_removal.wait().await;
    }

    pub(crate) async fn release_after_worktree_removal(&self) {
        self.after_worktree_removal_release.wait().await;
    }

    pub(crate) fn fail_branch_cleanup(&self, detail: impl Into<String>) {
        *self.branch_failure.lock().expect("workspace disposal hook") = Some(detail.into());
    }

    fn take_branch_failure(&self) -> Option<String> {
        self.branch_failure
            .lock()
            .expect("workspace disposal hook")
            .take()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_OVERLAY_BYTES, MAX_OVERLAY_FILES, SubagentWorkspaceManager, SubagentWorkspacePolicy,
        WorkspaceAcquireHook, WorkspaceCleanup, WorkspaceDisposalHook, WorkspaceIsolation,
        WorkspaceOverlayFreezeHook, WorkspaceOverlayMaterializationHook,
        WorkspaceOverlaySourceHook, WorkspaceOverlayValidationHook, WorkspaceSnapshot,
        deterministic_worktree_name, parse_overlay_manifest,
    };
    use crate::runtime::cancellation::CancellationSignal;
    use crate::runtime::identity::SubagentId;

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_git_worktree_path_alias_is_normalized_without_general_symlink_resolution() {
        let private = std::path::Path::new("/private/var/folders/rustx/worktree");
        let public = std::path::Path::new("/var/folders/rustx/worktree");
        assert_eq!(
            super::normalize_platform_git_path(private),
            public,
            "the OS-owned /private/var alias must compare with /var"
        );
        assert!(super::git_registration_paths_match(
            Some(private),
            Some(public)
        ));
    }

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
        assert_eq!(settlement.cleanup(), WorkspaceCleanup::Shared);
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
        assert_eq!(settlement.cleanup(), WorkspaceCleanup::Removed);
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
        let physical = worktree.physical_worktree_root.clone();
        let branch = worktree.branch.clone();
        assert!(ref_exists(dir.path(), &branch));
        let settlement = lease.settle_after_child().await;
        assert_eq!(settlement.cleanup(), WorkspaceCleanup::Removed);
        assert!(settlement.handoff().is_none());
        assert!(!physical.exists(), "unchanged isolated worktree is removed");
        assert!(
            !ref_exists(dir.path(), &branch),
            "runtime branch is removed"
        );
    }

    #[tokio::test]
    async fn changed_worktree_handoff_is_disposed_exactly_and_discards_dirty_source() {
        let repository = repository();
        let parent_before =
            std::fs::read_to_string(repository.path().join("tracked.txt")).expect("parent source");
        let runtime = tempfile::tempdir().expect("runtime root");
        let manager = SubagentWorkspaceManager::new(repository.path(), runtime.path());
        let subagent_id = SubagentId::new("conversation-dispose-subagent-1");
        let lease = manager
            .acquire(default_isolated(), &subagent_id, &CancellationSignal::new())
            .await
            .expect("retained worktree");
        let physical = lease
            .physical_worktree_root()
            .expect("physical worktree")
            .to_path_buf();
        let branch = lease
            .snapshot()
            .git_worktree()
            .expect("Git facts")
            .branch
            .clone();
        std::fs::write(
            lease.logical_workspace().join("tracked.txt"),
            "discard me\n",
        )
        .expect("dirty retained source");

        let settlement = lease.settle_after_child().await;
        assert_eq!(settlement.cleanup(), WorkspaceCleanup::Preserved);
        let handoff = settlement.handoff().expect("retained handoff");
        assert!(handoff.dirty);
        assert!(physical.exists());
        assert!(ref_exists(repository.path(), &branch));

        manager
            .dispose_retained_workspace(&subagent_id, &settlement.snapshot, handoff)
            .await
            .expect("exact retained resource disposal");

        assert!(!physical.exists(), "only the retained worktree was removed");
        assert!(
            !ref_exists(repository.path(), &branch),
            "only its branch was removed"
        );
        assert_eq!(
            std::fs::read_to_string(repository.path().join("tracked.txt"))
                .expect("parent source after disposal"),
            parent_before,
            "discarding a retained child never changes the parent workspace"
        );
    }

    #[tokio::test]
    async fn worktree_removal_is_a_typed_partial_settlement_when_branch_cleanup_fails() {
        let repository = repository();
        let parent_before =
            std::fs::read_to_string(repository.path().join("tracked.txt")).expect("parent");
        let runtime = tempfile::tempdir().expect("runtime root");
        let mut manager = SubagentWorkspaceManager::new(repository.path(), runtime.path());
        let hook = std::sync::Arc::new(WorkspaceDisposalHook::new());
        manager.install_disposal_hook(hook.clone());
        let subagent_id = SubagentId::new("conversation-disposal-partial-subagent-1");
        let lease = manager
            .acquire(default_isolated(), &subagent_id, &CancellationSignal::new())
            .await
            .expect("retained worktree");
        std::fs::write(
            lease.logical_workspace().join("tracked.txt"),
            "partial disposal\n",
        )
        .expect("retained source change");
        let settlement = lease.settle_after_child().await;
        let handoff = settlement.handoff().expect("retained handoff");
        let physical = handoff.physical_worktree_root.clone();
        let branch = handoff.branch.clone();

        hook.fail_branch_cleanup("injected compare-delete failure");
        let result = manager
            .dispose_retained_workspace(&subagent_id, &settlement.snapshot, handoff)
            .await
            .expect("the physical layer reports partial success as a value");
        assert!(matches!(
            result,
            super::WorkspaceDisposalSettlement::WorktreeRemoved { ref detail }
                if detail == "injected compare-delete failure"
        ));
        assert!(
            !physical.exists(),
            "the worktree removal is already committed"
        );
        assert!(
            ref_exists(repository.path(), &branch),
            "the branch remains residual"
        );
        assert_eq!(
            std::fs::read_to_string(repository.path().join("tracked.txt"))
                .expect("parent source after partial disposal"),
            parent_before,
            "partial disposal never changes the parent workspace"
        );

        // A direct, non-authorized retry cannot infer success from absence.
        // Only the registry's durable intent may use the continuation phase.
        let retry = manager
            .dispose_retained_workspace(&subagent_id, &settlement.snapshot, handoff)
            .await;
        assert!(matches!(
            retry,
            Err(super::WorkspaceDisposalError::OwnershipMismatch { .. })
        ));
        assert!(ref_exists(repository.path(), &branch));
    }

    #[tokio::test]
    async fn moved_branch_after_worktree_removal_is_preserved_by_compare_delete() {
        let repository = repository();
        let runtime = tempfile::tempdir().expect("runtime root");
        let mut manager = SubagentWorkspaceManager::new(repository.path(), runtime.path());
        let hook = std::sync::Arc::new(WorkspaceDisposalHook::new());
        manager.install_disposal_hook(hook.clone());
        let subagent_id = SubagentId::new("conversation-disposal-moved-branch-subagent-1");
        let lease = manager
            .acquire(default_isolated(), &subagent_id, &CancellationSignal::new())
            .await
            .expect("retained worktree");
        std::fs::write(
            lease.logical_workspace().join("tracked.txt"),
            "move branch after removal\n",
        )
        .expect("retained source change");
        let settlement = lease.settle_after_child().await;
        let handoff = settlement.handoff().expect("retained handoff");
        let physical = handoff.physical_worktree_root.clone();
        let branch = handoff.branch.clone();
        hook.arm_after_worktree_removal();

        let task_manager = manager.clone();
        let task_id = subagent_id.clone();
        let task_snapshot = settlement.snapshot.clone();
        let task_handoff = handoff.clone();
        let task = tokio::spawn(async move {
            task_manager
                .dispose_retained_workspace(&task_id, &task_snapshot, &task_handoff)
                .await
        });
        hook.wait_until_worktree_removed().await;
        assert!(!physical.exists(), "the first physical step has committed");

        // The worktree registration is gone, so an external actor can move
        // the exact runtime ref before the compare-delete step.
        git(
            repository.path(),
            &["commit", "--allow-empty", "-m", "move branch externally"],
        );
        let moved_head = head(repository.path());
        let reference = format!("refs/heads/{branch}");
        git(
            repository.path(),
            &["update-ref", reference.as_str(), moved_head.as_str()],
        );
        hook.release_after_worktree_removal().await;

        let result = task.await.expect("disposal task").expect("partial result");
        assert!(matches!(
            result,
            super::WorkspaceDisposalSettlement::WorktreeRemoved { ref detail }
                if detail.contains("moved") && detail.contains(&moved_head)
        ));
        assert!(!physical.exists());
        assert!(
            ref_exists(repository.path(), &branch),
            "the moved branch survives"
        );

        // The durable continuation phase can inspect the residual ref, but
        // compare-delete still refuses to remove its unexpected value.
        let continuation = manager
            .dispose_authorized_workspace(
                &subagent_id,
                &settlement.snapshot,
                handoff,
                super::WorkspaceDisposalPhase::WorktreeRemoved,
            )
            .await
            .expect("continuation result");
        assert!(matches!(
            continuation,
            super::WorkspaceDisposalSettlement::WorktreeRemoved { .. }
        ));
        assert!(ref_exists(repository.path(), &branch));
    }

    #[tokio::test]
    async fn externally_missing_worktree_without_intent_fails_closed() {
        let repository = repository();
        let runtime = tempfile::tempdir().expect("runtime root");
        let manager = SubagentWorkspaceManager::new(repository.path(), runtime.path());
        let subagent_id = SubagentId::new("conversation-disposal-external-missing-subagent-1");
        let lease = manager
            .acquire(default_isolated(), &subagent_id, &CancellationSignal::new())
            .await
            .expect("retained worktree");
        std::fs::write(
            lease.logical_workspace().join("tracked.txt"),
            "external removal\n",
        )
        .expect("retained source change");
        let settlement = lease.settle_after_child().await;
        let handoff = settlement.handoff().expect("retained handoff");
        let physical = handoff.physical_worktree_root.clone();
        let branch = handoff.branch.clone();
        git(
            repository.path(),
            &[
                "worktree",
                "remove",
                "--force",
                physical.to_str().expect("utf8 path"),
            ],
        );
        assert!(!physical.exists());
        assert!(ref_exists(repository.path(), &branch));

        let result = manager
            .dispose_retained_workspace(&subagent_id, &settlement.snapshot, handoff)
            .await;
        assert!(matches!(
            result,
            Err(super::WorkspaceDisposalError::OwnershipMismatch { .. })
        ));
        assert!(ref_exists(repository.path(), &branch));
    }

    #[tokio::test]
    async fn disposal_leaves_an_unrelated_registered_worktree_and_branch_untouched() {
        let repository = repository();
        let base = head(repository.path());
        let unrelated = tempfile::tempdir().expect("unrelated worktree root");
        let unrelated_path = unrelated.path().join("checkout");
        let unrelated_path_text = unrelated_path.to_str().expect("utf8 path");
        git(
            repository.path(),
            &[
                "worktree",
                "add",
                "-b",
                "unrelated",
                unrelated_path_text,
                &base,
            ],
        );
        let runtime = tempfile::tempdir().expect("runtime root");
        let manager = SubagentWorkspaceManager::new(repository.path(), runtime.path());
        let subagent_id = SubagentId::new("conversation-isolation-dispose-subagent-1");
        let lease = manager
            .acquire(default_isolated(), &subagent_id, &CancellationSignal::new())
            .await
            .expect("target worktree");
        let target = lease
            .physical_worktree_root()
            .expect("target physical worktree")
            .to_path_buf();
        let target_branch = lease
            .snapshot()
            .git_worktree()
            .expect("target Git facts")
            .branch
            .clone();
        std::fs::write(
            lease.logical_workspace().join("tracked.txt"),
            "retained target\n",
        )
        .expect("target source change");
        let settlement = lease.settle_after_child().await;
        let handoff = settlement.handoff().expect("changed handoff");

        manager
            .dispose_retained_workspace(&subagent_id, &settlement.snapshot, handoff)
            .await
            .expect("target disposal");

        assert!(!target.exists());
        assert!(!ref_exists(repository.path(), &target_branch));
        assert!(unrelated_path.exists(), "unrelated worktree remains");
        assert!(ref_exists(repository.path(), "unrelated"));
        git(
            repository.path(),
            &["worktree", "remove", "--force", unrelated_path_text],
        );
        assert!(ref_exists(repository.path(), "unrelated"));
        git(
            repository.path(),
            &["update-ref", "-d", "refs/heads/unrelated"],
        );
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)] // One test exercises each fail-closed proof branch.
    async fn disposal_fails_closed_for_tampered_path_branch_repository_and_registration() {
        let source_repository = repository();
        let runtime = tempfile::tempdir().expect("runtime root");
        let manager = SubagentWorkspaceManager::new(source_repository.path(), runtime.path());

        let tampered_path_id = SubagentId::new("conversation-tampered-path-subagent-1");
        let tampered_path_lease = manager
            .acquire(
                default_isolated(),
                &tampered_path_id,
                &CancellationSignal::new(),
            )
            .await
            .expect("path test worktree");
        std::fs::write(
            tampered_path_lease.logical_workspace().join("tracked.txt"),
            "retained path test\n",
        )
        .expect("path test source change");
        let tampered_path_settlement = tampered_path_lease.settle_after_child().await;
        let tampered_path_handoff = tampered_path_settlement.handoff().expect("handoff");
        let mut tampered_path = tampered_path_handoff.clone();
        tampered_path.physical_worktree_root = runtime.path().join("somewhere-else");
        let tampered_path_result = manager
            .dispose_retained_workspace(
                &tampered_path_id,
                &tampered_path_settlement.snapshot,
                &tampered_path,
            )
            .await;
        assert!(matches!(
            tampered_path_result,
            Err(super::WorkspaceDisposalError::OwnershipMismatch { .. })
        ));
        assert!(tampered_path_handoff.physical_worktree_root.exists());

        let mut malformed_handoff = tampered_path_handoff.clone();
        malformed_handoff.base_commit = "not-a-commit".to_owned();
        let malformed_result = manager
            .dispose_retained_workspace(
                &tampered_path_id,
                &tampered_path_settlement.snapshot,
                &malformed_handoff,
            )
            .await;
        assert!(matches!(
            malformed_result,
            Err(super::WorkspaceDisposalError::OwnershipMismatch { .. })
        ));
        assert!(tampered_path_handoff.physical_worktree_root.exists());

        let mismatched_branch_id = SubagentId::new("conversation-tampered-branch-subagent-1");
        let mismatched_branch_lease = manager
            .acquire(
                default_isolated(),
                &mismatched_branch_id,
                &CancellationSignal::new(),
            )
            .await
            .expect("branch test worktree");
        std::fs::write(
            mismatched_branch_lease
                .logical_workspace()
                .join("tracked.txt"),
            "retained branch test\n",
        )
        .expect("branch test source change");
        let mismatched_branch_settlement = mismatched_branch_lease.settle_after_child().await;
        let mismatched_branch_handoff = mismatched_branch_settlement.handoff().expect("handoff");
        let mismatched_branch_physical = mismatched_branch_handoff.physical_worktree_root.clone();
        let mismatched_branch_name = mismatched_branch_handoff.branch.clone();
        git(
            &mismatched_branch_physical,
            &["switch", "-c", "foreign/rebound"],
        );
        let rebound_branch_result = manager
            .dispose_retained_workspace(
                &mismatched_branch_id,
                &mismatched_branch_settlement.snapshot,
                mismatched_branch_handoff,
            )
            .await;
        assert!(matches!(
            rebound_branch_result,
            Err(super::WorkspaceDisposalError::OwnershipMismatch { .. })
        ));
        assert!(mismatched_branch_physical.exists());
        assert!(ref_exists(
            source_repository.path(),
            &mismatched_branch_name
        ));
        git(
            &mismatched_branch_physical,
            &["commit", "--allow-empty", "-m", "move retained branch"],
        );
        let mismatched_branch_result = manager
            .dispose_retained_workspace(
                &mismatched_branch_id,
                &mismatched_branch_settlement.snapshot,
                mismatched_branch_handoff,
            )
            .await;
        assert!(matches!(
            mismatched_branch_result,
            Err(super::WorkspaceDisposalError::OwnershipMismatch { .. })
        ));
        assert!(mismatched_branch_physical.exists());

        let missing_registration_id =
            SubagentId::new("conversation-missing-registration-subagent-1");
        let missing_registration_lease = manager
            .acquire(
                default_isolated(),
                &missing_registration_id,
                &CancellationSignal::new(),
            )
            .await
            .expect("registration test worktree");
        std::fs::write(
            missing_registration_lease
                .logical_workspace()
                .join("tracked.txt"),
            "retained registration test\n",
        )
        .expect("registration test source change");
        let missing_registration_settlement = missing_registration_lease.settle_after_child().await;
        let missing_registration_handoff =
            missing_registration_settlement.handoff().expect("handoff");
        let missing_registration_physical =
            missing_registration_handoff.physical_worktree_root.clone();
        let missing_registration_branch = missing_registration_handoff.branch.clone();
        git(
            source_repository.path(),
            &[
                "worktree",
                "remove",
                "--force",
                missing_registration_physical.to_str().expect("utf8 path"),
            ],
        );
        std::fs::create_dir_all(&missing_registration_physical).expect("replacement directory");
        std::fs::write(missing_registration_physical.join("sentinel"), "keep me")
            .expect("replacement sentinel");
        let missing_registration_result = manager
            .dispose_retained_workspace(
                &missing_registration_id,
                &missing_registration_settlement.snapshot,
                missing_registration_handoff,
            )
            .await;
        assert!(matches!(
            missing_registration_result,
            Err(super::WorkspaceDisposalError::OwnershipMismatch { .. })
        ));
        assert_eq!(
            std::fs::read_to_string(missing_registration_physical.join("sentinel"))
                .expect("sentinel remains"),
            "keep me"
        );
        assert!(ref_exists(
            source_repository.path(),
            &missing_registration_branch
        ));

        let other_repository = repository();
        let mismatch_repository_id = SubagentId::new("conversation-mismatch-repository-subagent-1");
        let mismatch_repository_lease = manager
            .acquire(
                default_isolated(),
                &mismatch_repository_id,
                &CancellationSignal::new(),
            )
            .await
            .expect("repository mismatch worktree");
        std::fs::write(
            mismatch_repository_lease
                .logical_workspace()
                .join("tracked.txt"),
            "retained repository test\n",
        )
        .expect("repository mismatch source change");
        let mismatch_repository_settlement = mismatch_repository_lease.settle_after_child().await;
        let mismatch_repository_handoff =
            mismatch_repository_settlement.handoff().expect("handoff");
        let mismatch_repository_physical =
            mismatch_repository_handoff.physical_worktree_root.clone();
        let mut mismatch_snapshot = mismatch_repository_settlement.snapshot.clone();
        let other_root = std::fs::canonicalize(other_repository.path()).expect("other root");
        if let super::WorkspaceIsolation::GitWorktree(worktree) = &mut mismatch_snapshot.isolation {
            worktree.source_repository_root = other_root.clone();
        }
        let mismatch_repository_result = manager
            .dispose_retained_workspace(
                &mismatch_repository_id,
                &mismatch_snapshot,
                mismatch_repository_handoff,
            )
            .await;
        assert!(matches!(
            mismatch_repository_result,
            Err(super::WorkspaceDisposalError::OwnershipMismatch { .. })
        ));
        assert!(mismatch_repository_physical.exists());
    }

    #[tokio::test]
    async fn disposal_rechecks_registration_after_a_deterministic_concurrent_change() {
        let repository = repository();
        let runtime = tempfile::tempdir().expect("runtime root");
        let mut manager = SubagentWorkspaceManager::new(repository.path(), runtime.path());
        let hook = std::sync::Arc::new(WorkspaceDisposalHook::new());
        manager.install_disposal_hook(hook.clone());
        hook.arm_before_recheck();
        let subagent_id = SubagentId::new("conversation-disposal-race-subagent-1");
        let lease = manager
            .acquire(default_isolated(), &subagent_id, &CancellationSignal::new())
            .await
            .expect("race test worktree");
        std::fs::write(
            lease.logical_workspace().join("tracked.txt"),
            "retained race\n",
        )
        .expect("race source change");
        let settlement = lease.settle_after_child().await;
        let handoff = settlement.handoff().expect("retained handoff");
        let physical = handoff.physical_worktree_root.clone();
        let task_manager = manager.clone();
        let task_id = subagent_id.clone();
        let task_snapshot = settlement.snapshot.clone();
        let task_handoff = handoff.clone();
        let task = tokio::spawn(async move {
            task_manager
                .dispose_retained_workspace(&task_id, &task_snapshot, &task_handoff)
                .await
        });

        hook.wait_until_verified().await;
        git(
            &physical,
            &["commit", "--allow-empty", "-m", "rebind retained branch"],
        );
        hook.release().await;
        let result = task.await.expect("disposal task");
        assert!(matches!(
            result,
            Err(super::WorkspaceDisposalError::OwnershipMismatch { .. })
        ));
        assert!(
            physical.exists(),
            "the changed registration remains untouched"
        );
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
            lease.settle_after_child().await.cleanup(),
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
            lease.settle_after_child().await.cleanup(),
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
            assert!(
                matches!(&error, super::WorkspaceAcquireError::OverlaySymlink { .. }),
                "unexpected symlink error: {error:?}"
            );
            assert!(!runtime.path().join("worktrees").exists());
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn ancestor_replacement_during_source_acquisition_fails_closed() {
        use std::os::unix::fs::symlink;

        let repository = repository();
        let workspace = declare_overlay(
            repository.path(),
            std::path::Path::new(""),
            &["local/runtime.env".to_owned()],
        );
        write_overlay_file(&workspace, "local/runtime.env", b"INSIDE\n");
        let outside = tempfile::tempdir().expect("outside");
        std::fs::write(outside.path().join("runtime.env"), b"OUTSIDE\n").expect("outside overlay");

        let runtime = tempfile::tempdir().expect("runtime root");
        let mut manager = SubagentWorkspaceManager::new(&workspace, runtime.path());
        let hook = std::sync::Arc::new(WorkspaceOverlaySourceHook::new());
        manager.install_overlay_source_hook(hook.clone());
        let subagent = SubagentId::new("conversation-overlay-ancestor-race-subagent-1");
        let task_manager = manager.clone();
        let task = tokio::spawn(async move {
            task_manager
                .acquire(default_isolated(), &subagent, &CancellationSignal::new())
                .await
        });

        // The source workspace handle is already stable, but no selected
        // descendant has been opened yet. Replacing the ancestor here proves
        // that the first secure descent rejects the symlink rather than
        // opening a pathname outside the logical workspace.
        hook.wait_until_ready_to_acquire().await;
        let original_local = workspace.join("local");
        std::fs::rename(&original_local, workspace.join("local-original"))
            .expect("move original local directory");
        symlink(outside.path(), &original_local).expect("replace local with symlink");
        hook.release().await;

        let error = task
            .await
            .expect("acquisition task")
            .expect_err("ancestor replacement must fail source acquisition");
        assert!(
            matches!(&error, super::WorkspaceAcquireError::OverlaySymlink { .. }),
            "unexpected race error: {error:?}"
        );

        let path = runtime
            .path()
            .join("worktrees")
            .join(deterministic_worktree_name(&SubagentId::new(
                "conversation-overlay-ancestor-race-subagent-1",
            )));
        let branch = format!(
            "rustx/subagent/{}",
            deterministic_worktree_name(&SubagentId::new(
                "conversation-overlay-ancestor-race-subagent-1",
            ))
        );
        assert!(!path.exists(), "outside bytes were never materialized");
        assert!(!runtime.path().join("worktrees").exists());
        assert!(!ref_exists(repository.path(), &branch));
        assert_eq!(
            std::fs::read(outside.path().join("runtime.env")).expect("outside file"),
            b"OUTSIDE\n",
            "the outside source was never frozen or modified"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn ancestor_replacement_after_acquisition_reads_only_the_retained_file() {
        use std::os::unix::fs::symlink;

        let repository = repository();
        let workspace = declare_overlay(
            repository.path(),
            std::path::Path::new(""),
            &["local/runtime.env".to_owned()],
        );
        write_overlay_file(&workspace, "local/runtime.env", b"INSIDE\n");
        let outside = tempfile::tempdir().expect("outside");
        std::fs::write(outside.path().join("runtime.env"), b"OUTSIDE\n").expect("outside overlay");

        let runtime = tempfile::tempdir().expect("runtime root");
        let mut manager = SubagentWorkspaceManager::new(&workspace, runtime.path());
        let hook = std::sync::Arc::new(WorkspaceOverlayValidationHook::new());
        manager.install_overlay_validation_hook(hook.clone());
        let subagent = SubagentId::new("conversation-overlay-retained-file-subagent-1");
        let task_manager = manager.clone();
        let task_subagent = subagent.clone();
        let task = tokio::spawn(async move {
            task_manager
                .acquire(
                    default_isolated(),
                    &task_subagent,
                    &CancellationSignal::new(),
                )
                .await
        });

        // All source handles and Git eligibility facts are ready. Replacing
        // the pathname now is exactly where the old implementation reopened
        // the canonical path and could freeze OUTSIDE instead of INSIDE.
        hook.wait_until_validated().await;
        let original_local = workspace.join("local");
        std::fs::rename(&original_local, workspace.join("local-original"))
            .expect("move original local directory");
        symlink(outside.path(), &original_local).expect("replace local with symlink");
        hook.release().await;

        let lease = task
            .await
            .expect("acquisition task")
            .expect("retained source handle keeps acquisition safe");
        assert_eq!(
            std::fs::read(lease.logical_workspace().join("local/runtime.env"))
                .expect("materialized overlay"),
            b"INSIDE\n",
            "the retained source object, not the replaced pathname, was frozen"
        );
        assert_eq!(
            std::fs::read(outside.path().join("runtime.env")).expect("outside file"),
            b"OUTSIDE\n"
        );
        assert_eq!(
            lease.settle_after_child().await.cleanup(),
            WorkspaceCleanup::Removed
        );
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
            accepted.settle_after_child().await.cleanup(),
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
            lease.settle_after_child().await.cleanup(),
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
            lease.settle_after_child().await.cleanup(),
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
            lease.settle_after_child().await.cleanup(),
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
        let handoff = settlement.handoff().expect("retained handoff");

        assert_eq!(settlement.cleanup(), WorkspaceCleanup::Preserved);
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
            "1111111111111111111111111111111111111111".to_owned(),
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
            lease.settle_after_child().await.cleanup(),
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

        assert_eq!(settlement.cleanup(), WorkspaceCleanup::Removed);
        assert!(settlement.handoff().is_none());
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

        assert_eq!(settlement.cleanup(), WorkspaceCleanup::Removed);
        assert!(settlement.handoff().is_none());
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
        let handoff = settlement.handoff().expect("source handoff");
        assert_eq!(settlement.cleanup(), WorkspaceCleanup::Preserved);
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
        assert_eq!(settlement.cleanup(), WorkspaceCleanup::Removed);
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
        let handoff = settlement.handoff().expect("dirty handoff");
        assert_eq!(settlement.cleanup(), WorkspaceCleanup::Preserved);
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
        let handoff = settlement.handoff().expect("committed handoff");
        assert_eq!(settlement.cleanup(), WorkspaceCleanup::Preserved);
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
        let handoff = settlement.handoff().expect("committed handoff");

        assert_eq!(settlement.cleanup(), WorkspaceCleanup::Preserved);
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
        let handoff = settlement.handoff().expect("source handoff");

        assert_eq!(settlement.cleanup(), WorkspaceCleanup::Preserved);
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
        let handoff = settlement.handoff().expect("recovered handoff");

        assert_eq!(settlement.cleanup(), WorkspaceCleanup::Preserved);
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
        let handoff = settlement.handoff().expect("combined handoff");
        assert_eq!(settlement.cleanup(), WorkspaceCleanup::Preserved);
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
        assert!(error.settlement.handoff().is_some());
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
    async fn cancellation_during_partial_overlay_materialization_settles_everything() {
        let dir = repository_with_workspace(std::path::Path::new("backend"));
        let workspace = declare_overlay(
            dir.path(),
            std::path::Path::new("backend"),
            &["first.env".to_owned(), "second.env".to_owned()],
        );
        write_overlay_file(&workspace, "first.env", b"FIRST=overlay\n");
        write_overlay_file(&workspace, "second.env", b"SECOND=overlay\n");

        let runtime_root = dir.path().join("artifacts");
        let mut manager = SubagentWorkspaceManager::new(&workspace, &runtime_root);
        let hook = std::sync::Arc::new(WorkspaceOverlayMaterializationHook::new());
        manager.install_overlay_materialization_hook(hook.clone());
        let cancellation = CancellationSignal::new();
        let subagent = SubagentId::new("conversation-partial-overlay-cancel-subagent-1");
        let task_manager = manager.clone();
        let task_cancellation = cancellation.clone();
        let task_subagent = subagent.clone();
        let task = tokio::spawn(async move {
            task_manager
                .acquire(default_isolated(), &task_subagent, &task_cancellation)
                .await
        });

        hook.wait_until_after_first().await;
        let path = runtime_root
            .join("worktrees")
            .join(deterministic_worktree_name(&subagent));
        let logical = path.join("backend");
        let branch = format!("rustx/subagent/{}", deterministic_worktree_name(&subagent));
        assert!(path.exists(), "the staged physical worktree exists");
        assert!(
            logical.join("first.env").is_file(),
            "the first overlay is materialized"
        );
        assert_eq!(
            std::fs::read(logical.join("first.env")).expect("first overlay"),
            b"FIRST=overlay\n"
        );
        assert!(
            !logical.join("second.env").exists(),
            "the second overlay has not yet been materialized"
        );
        assert!(!task.is_finished(), "ownership has not committed yet");

        cancellation.cancel();
        hook.release().await;
        let error = task
            .await
            .expect("acquisition task")
            .expect_err("partial materialization cancellation");
        assert!(matches!(error, super::WorkspaceAcquireError::Cancelled));
        assert!(!path.exists(), "the staged worktree is safely removed");
        assert!(!logical.join("first.env").exists());
        assert!(!logical.join("second.env").exists());
        assert!(
            !ref_exists(dir.path(), &branch),
            "the runtime branch is removed"
        );
        assert_eq!(
            std::fs::read(workspace.join("first.env")).expect("parent first overlay"),
            b"FIRST=overlay\n"
        );
        assert_eq!(
            std::fs::read(workspace.join("second.env")).expect("parent second overlay"),
            b"SECOND=overlay\n"
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
        assert_eq!(first_settlement.cleanup(), WorkspaceCleanup::Removed);
        assert_eq!(second_settlement.cleanup(), WorkspaceCleanup::Removed);
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
            first.settle_after_child().await.cleanup(),
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
            lease.settle_after_child().await.cleanup(),
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
            lease.settle_after_child().await.cleanup(),
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
        assert_eq!(settlement.cleanup(), WorkspaceCleanup::Removed);
    }
}
