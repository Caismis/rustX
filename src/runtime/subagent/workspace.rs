//! Deterministic workspace ownership for named subagents (Issue #146).
//!
//! This module is the only owner of Git/worktree operations.  The registry
//! supplies it with a resolved policy and an already allocated subagent
//! identity; it never constructs Git commands itself.  A [`WorkspaceLease`]
//! is the physical ownership token that moves from preparation to the child
//! process driver at the same boundary as the process handle.
//!
//! The important snapshot rule is intentionally visible in the types and in
//! the command order:
//!
//! ```text
//! capture HEAD = C
//! capture parent status
//! create worktree at explicit C
//! ```
//!
//! The parent path is never consulted again to choose the child's base.  A
//! dirty parent is therefore allowed by default without copying any of its
//! bytes, and a later parent commit cannot move an already acquired child.
//!
//! Terminal changed-state is deliberately two-dimensional:
//! `dirty = ordinary tracked/index/untracked-non-ignored Git status`, while
//! `changed = dirty || (final HEAD != base_commit)`. Ignored build/cache
//! artifacts alone are disposable execution output and do not force a
//! handoff.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::process::Stdio;
use tokio::process::Command;

use crate::runtime::cancellation::CancellationSignal;
use crate::runtime::identity::SubagentId;

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
/// and not model-authored content.  In shared mode only `workspace` is
/// meaningful.  In worktree mode every Git field is present and describes the
/// exact source snapshot and runtime-created ref.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceSnapshot {
    /// The authoritative project workspace path given to the child.
    pub workspace: PathBuf,
    /// Whether this child uses a runtime-created Git worktree.
    pub isolated: bool,
    /// The canonical source repository top-level path, in worktree mode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository: Option<PathBuf>,
    /// The exact committed source `HEAD` selected before acquisition.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_commit: Option<String>,
    /// The runtime-created branch/ref, in worktree mode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
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
            workspace: workspace.into(),
            isolated: false,
            repository: None,
            base_commit: None,
            branch: None,
            parent_had_uncommitted_changes: false,
        }
    }

    fn worktree(
        workspace: PathBuf,
        repository: PathBuf,
        base_commit: String,
        branch: String,
        parent_had_uncommitted_changes: bool,
    ) -> Self {
        Self {
            workspace,
            isolated: true,
            repository: Some(repository),
            base_commit: Some(base_commit),
            branch: Some(branch),
            parent_had_uncommitted_changes,
        }
    }

    /// Validates the closed shared/worktree shape before it crosses a durable
    /// or process boundary.
    ///
    /// The manager constructs this value, but the durable store and child
    /// process also validate it so a malformed event/spec cannot silently
    /// turn an isolated child into an untracked arbitrary path.
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.workspace.as_os_str().is_empty() {
            return Err("workspace snapshot has an empty project path".to_owned());
        }
        if self.isolated {
            if self
                .repository
                .as_ref()
                .is_none_or(|repository| repository.as_os_str().is_empty())
            {
                return Err("isolated workspace snapshot has no repository".to_owned());
            }
            if self.base_commit.as_deref().is_none_or(str::is_empty) {
                return Err("isolated workspace snapshot has no base commit".to_owned());
            }
            if self.branch.as_deref().is_none_or(str::is_empty) {
                return Err("isolated workspace snapshot has no branch/ref".to_owned());
            }
        } else if self.repository.is_some() || self.base_commit.is_some() || self.branch.is_some() {
            return Err("shared workspace snapshot carries isolated Git facts".to_owned());
        }
        Ok(())
    }
}

/// The user-recoverable facts of a retained child workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceHandoff {
    /// The preserved worktree path.
    pub workspace: PathBuf,
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
        if self.workspace.as_os_str().is_empty() {
            return Err("workspace handoff has an empty project path".to_owned());
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
    /// The parent is dirty and strict clean-parent policy rejected it.
    DirtyParent {
        /// The exact committed `HEAD` captured before the dirty observation.
        base_commit: String,
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
        /// The deterministic path.
        workspace: PathBuf,
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
            Self::DirtyParent { base_commit } => write!(
                formatter,
                "the parent workspace is dirty; strict clean-parent policy rejected base {base_commit}"
            ),
            Self::Git { operation, detail } => {
                write!(formatter, "Git {operation} failed: {detail}")
            }
            Self::Collision { workspace, branch } => write!(
                formatter,
                "the deterministic child worktree path {} or branch {branch} is already occupied",
                workspace.display()
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
    parent_workspace: PathBuf,
    runtime_root: PathBuf,
    #[cfg(test)]
    acquisition_hook: Option<std::sync::Arc<WorkspaceAcquireHook>>,
    #[cfg(test)]
    settlement_hook: Option<std::sync::Arc<WorkspaceSettlementHook>>,
}

impl SubagentWorkspaceManager {
    /// Creates a manager over the already-canonical parent workspace and the
    /// disjoint runtime-private artifact root.
    #[must_use]
    pub fn new(parent_workspace: impl AsRef<Path>, runtime_root: impl AsRef<Path>) -> Self {
        Self {
            parent_workspace: parent_workspace.as_ref().to_path_buf(),
            runtime_root: runtime_root.as_ref().to_path_buf(),
            #[cfg(test)]
            acquisition_hook: None,
            #[cfg(test)]
            settlement_hook: None,
        }
    }

    /// Installs a test-only barrier after Git has created the worktree and
    /// before acquisition performs cancellation/verification settlement.
    #[cfg(test)]
    pub(crate) fn install_acquisition_hook(&mut self, hook: std::sync::Arc<WorkspaceAcquireHook>) {
        self.acquisition_hook = Some(hook);
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
    /// strict-policy enforcement, and only then worktree creation at the
    /// captured commit.  A cancellation during an in-flight Git command kills
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
                snapshot: WorkspaceSnapshot::shared(self.parent_workspace.clone()),
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

        let repository = self
            .git_text(
                &self.parent_workspace,
                vec!["rev-parse".into(), "--show-toplevel".into()],
                Some(cancellation),
            )
            .await?;
        let repository = PathBuf::from(repository);
        let base_commit = self
            .git_text(
                &self.parent_workspace,
                vec!["rev-parse".into(), "HEAD".into()],
                Some(cancellation),
            )
            .await?;
        let parent_status = self
            .git_text(
                &self.parent_workspace,
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

        let token = deterministic_worktree_name(subagent_id);
        let branch = format!("rustx/subagent/{token}");
        let workspace = self.runtime_root.join("worktrees").join(token);
        if path_is_occupied(&workspace) || self.branch_exists(&branch, cancellation).await? {
            return Err(WorkspaceAcquireError::Collision { workspace, branch });
        }
        std::fs::create_dir_all(
            workspace
                .parent()
                .expect("the deterministic worktree path has a parent"),
        )
        .map_err(|error| WorkspaceAcquireError::Git {
            operation: "prepare worktree allocation root".to_owned(),
            detail: error.to_string(),
        })?;

        let snapshot = WorkspaceSnapshot::worktree(
            workspace.clone(),
            repository,
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
        // `worktree add -b` is Git's atomic branch-registration operation.
        // A losing same-identity acquisition must never infer ownership from
        // the path/ref that the winning command just created, so `created` is
        // set only when this command itself returned success *and* the exact
        // registration is present.
        let add = self
            .git_raw(
                &self.parent_workspace,
                vec![
                    "worktree".into(),
                    "add".into(),
                    "-b".into(),
                    branch.clone().into(),
                    workspace.clone().into_os_string(),
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
        #[cfg(test)]
        if let Some(hook) = &self.acquisition_hook {
            hook.pause_after_creation().await;
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
                &workspace,
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
        Ok(lease)
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
                &self.parent_workspace,
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
                &self.parent_workspace,
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
        if !snapshot.isolated {
            return WorkspaceSettlement::shared(snapshot.clone());
        }
        let Some(repository) = snapshot.repository.as_ref() else {
            return WorkspaceSettlement::unresolved(
                snapshot.clone(),
                "isolated workspace snapshot has no repository",
            );
        };
        let head = run_git_sync(&snapshot.workspace, &["rev-parse", "HEAD"]);
        let status = run_git_sync(&snapshot.workspace, &ORDINARY_WORKSPACE_STATUS_ARGS);
        let (head, status) = match (head, status) {
            (Ok(head), Ok(status)) => (head, status),
            (Err(error), _) | (_, Err(error)) => {
                return WorkspaceSettlement::unresolved(snapshot.clone(), error);
            }
        };
        let Some(branch) = snapshot.branch.clone() else {
            return WorkspaceSettlement::unresolved(
                snapshot.clone(),
                "isolated workspace snapshot has no branch/ref",
            );
        };
        let Some(base_commit) = snapshot.base_commit.clone() else {
            return WorkspaceSettlement::unresolved(
                snapshot.clone(),
                "isolated workspace snapshot has no base commit",
            );
        };
        let listing = match run_git_sync(repository, &["worktree", "list", "--porcelain"]) {
            Ok(listing) => listing,
            Err(error) => return WorkspaceSettlement::unresolved(snapshot.clone(), error),
        };
        if !worktree_listing_contains(&listing, snapshot, false) {
            return WorkspaceSettlement::unresolved(
                snapshot.clone(),
                format!(
                    "recovered worktree {} is not registered in the owned repository {}",
                    snapshot.workspace.display(),
                    repository.display()
                ),
            );
        }
        let reference = format!("refs/heads/{branch}");
        let branch_head = match run_git_sync(repository, &["rev-parse", "--verify", &reference]) {
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
        let (dirty, _) = workspace_change_facts(snapshot.base_commit.as_deref(), &head, &status);
        WorkspaceSettlement {
            snapshot: snapshot.clone(),
            handoff: Some(WorkspaceHandoff {
                workspace: snapshot.workspace.clone(),
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
    /// The authoritative child project workspace path.
    #[must_use]
    pub fn workspace(&self) -> &Path {
        &self.snapshot.workspace
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
    /// path remains the conservative recovery authority; a later recovery
    /// inspection can observe the final state once no process may mutate it.
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
        if !self.snapshot.isolated {
            return WorkspaceSettlement::shared(self.snapshot);
        }
        if !self.created {
            return self.settle_unregistered().await;
        }
        self.settle_registered().await
    }

    async fn settle_unregistered(self) -> WorkspaceSettlement {
        let snapshot = self.snapshot;
        if path_is_occupied(&snapshot.workspace) {
            return WorkspaceSettlement::unresolved(
                snapshot,
                "the deterministic workspace path exists but is not a proven Git worktree owned by this lease",
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
        let head = match self
            .manager
            .git_text(
                &snapshot.workspace,
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
            .git_text(&snapshot.workspace, ordinary_workspace_status_args(), None)
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
        let Some(branch) = snapshot.branch.clone() else {
            return WorkspaceSettlement::unresolved(
                snapshot,
                "isolated workspace snapshot has no branch/ref",
            );
        };
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
        let (dirty, changed) =
            workspace_change_facts(snapshot.base_commit.as_deref(), &head, &status);
        let handoff = WorkspaceHandoff {
            workspace: snapshot.workspace.clone(),
            branch,
            base_commit: snapshot.base_commit.clone().unwrap_or_default(),
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
            &self.parent_workspace,
            vec!["rev-parse".into(), "--verify".into(), reference.into()],
            None,
        )
        .await
        .map_err(|error| error.to_string())
    }

    async fn remove_clean_worktree(&self, snapshot: &WorkspaceSnapshot) -> Result<(), String> {
        let output = self
            .git_raw(
                &self.parent_workspace,
                vec![
                    "worktree".into(),
                    "remove".into(),
                    "--".into(),
                    snapshot.workspace.clone().into_os_string(),
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
        let Some(branch) = snapshot.branch.as_deref() else {
            return Ok(());
        };
        let reference = format!("refs/heads/{branch}");
        let exists = self
            .git_raw(
                &self.parent_workspace,
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
        let base_commit = snapshot
            .base_commit
            .as_deref()
            .ok_or_else(|| "isolated workspace snapshot has no base commit".to_owned())?;
        let current = self
            .git_text(
                &self.parent_workspace,
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
                &self.parent_workspace,
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
    let expected_branch = snapshot
        .branch
        .as_deref()
        .map(|branch| format!("refs/heads/{branch}"));
    paths_refer_to_same_worktree(path, Some(snapshot.workspace.as_path()))
        && branch == expected_branch.as_deref()
        && (!require_base || head == snapshot.base_commit.as_deref())
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

    async fn pause_after_creation(&self) {
        self.after_creation.wait().await;
        self.release.wait().await;
    }

    async fn wait_until_created(&self) {
        self.after_creation.wait().await;
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
        SubagentWorkspaceManager, SubagentWorkspacePolicy, WorkspaceAcquireHook, WorkspaceCleanup,
        deterministic_worktree_name,
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
        assert_eq!(lease.workspace(), workspace);
        assert!(!lease.snapshot().isolated);
        let settlement = lease.settle_after_child().await;
        assert_eq!(settlement.cleanup, WorkspaceCleanup::Shared);
        assert!(workspace.exists());
        assert_eq!(
            std::fs::read_to_string(workspace.join("dirty.txt")).expect("shared file"),
            "shared bytes\n"
        );
    }

    #[tokio::test]
    async fn dirty_parent_is_not_copied_into_default_worktree() {
        let dir = repository();
        std::fs::write(dir.path().join("tracked.txt"), "parent dirty\n").expect("dirty file");
        std::fs::write(dir.path().join("untracked.txt"), "parent only\n").expect("untracked");
        let manager = SubagentWorkspaceManager::new(dir.path(), dir.path().join("artifacts"));
        let lease = manager
            .acquire(
                SubagentWorkspacePolicy::GitWorktree {
                    require_clean_parent: false,
                },
                &SubagentId::new("conversation-a-subagent-1"),
                &CancellationSignal::new(),
            )
            .await
            .expect("worktree");
        assert!(lease.snapshot().parent_had_uncommitted_changes);
        assert_eq!(
            std::fs::read_to_string(lease.workspace().join("tracked.txt")).expect("child file"),
            "committed\n"
        );
        assert!(!lease.workspace().join("untracked.txt").exists());
        let settlement = lease.settle_after_child().await;
        assert_eq!(settlement.cleanup, WorkspaceCleanup::Removed);
        assert!(!settlement.snapshot.workspace.exists());
    }

    #[tokio::test]
    async fn clean_parent_worktree_is_created_at_the_exact_head_and_removed_cleanly() {
        let dir = repository();
        let base = head(dir.path());
        let manager = SubagentWorkspaceManager::new(dir.path(), dir.path().join("artifacts"));
        let lease = manager
            .acquire(
                SubagentWorkspacePolicy::GitWorktree {
                    require_clean_parent: false,
                },
                &SubagentId::new("conversation-clean-subagent-1"),
                &CancellationSignal::new(),
            )
            .await
            .expect("worktree");
        assert_eq!(lease.snapshot().base_commit.as_deref(), Some(base.as_str()));
        assert_eq!(head(lease.workspace()), base);
        assert!(
            lease
                .snapshot()
                .branch
                .as_deref()
                .is_some_and(|branch| { branch.starts_with("rustx/subagent/") })
        );
        let settlement = lease.settle_after_child().await;
        assert_eq!(settlement.cleanup, WorkspaceCleanup::Removed);
        assert!(settlement.handoff.is_none());
    }

    #[tokio::test]
    async fn ignored_only_child_artifact_is_cleaned_without_a_handoff() {
        let dir = repository();
        ignore_target(dir.path());
        let manager = SubagentWorkspaceManager::new(dir.path(), dir.path().join("artifacts"));
        let lease = manager
            .acquire(
                SubagentWorkspacePolicy::GitWorktree {
                    require_clean_parent: false,
                },
                &SubagentId::new("conversation-ignored-only-subagent-1"),
                &CancellationSignal::new(),
            )
            .await
            .expect("worktree");
        let path = lease.workspace().to_path_buf();
        let branch = lease.snapshot().branch.clone().expect("branch");
        std::fs::create_dir_all(path.join("target/debug")).expect("target");
        std::fs::write(path.join("target/debug/generated"), "cache\n").expect("cache");

        let settlement = lease.settle_after_child().await;

        assert_eq!(settlement.cleanup, WorkspaceCleanup::Removed);
        assert!(settlement.handoff.is_none());
        assert!(!path.exists(), "ignored-only output is disposable cache");
        assert!(!ref_exists(dir.path(), &branch), "runtime ref was removed");
    }

    #[tokio::test]
    async fn parent_movement_after_acquisition_cannot_change_the_child_base() {
        let dir = repository();
        let manager = SubagentWorkspaceManager::new(dir.path(), dir.path().join("artifacts"));
        let lease = manager
            .acquire(
                SubagentWorkspacePolicy::GitWorktree {
                    require_clean_parent: false,
                },
                &SubagentId::new("conversation-snapshot-subagent-1"),
                &CancellationSignal::new(),
            )
            .await
            .expect("worktree");
        let base = lease.snapshot().base_commit.clone().expect("base");
        std::fs::write(dir.path().join("tracked.txt"), "parent commit two\n")
            .expect("parent update");
        commit(dir.path(), "parent second commit");
        let parent_head = head(dir.path());
        assert_ne!(parent_head, base);
        assert_eq!(head(lease.workspace()), base);
        assert_eq!(
            head(lease.workspace()),
            lease.snapshot().base_commit.clone().unwrap()
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
                SubagentWorkspacePolicy::GitWorktree {
                    require_clean_parent: false,
                },
                &SubagentId::new("conversation-dirty-child-subagent-1"),
                &CancellationSignal::new(),
            )
            .await
            .expect("worktree");
        std::fs::write(lease.workspace().join("tracked.txt"), "child work\n")
            .expect("child tracked change");
        std::fs::write(lease.workspace().join("new.txt"), "child artifact\n")
            .expect("child untracked change");
        let path = lease.workspace().to_path_buf();
        let settlement = lease.settle_after_child().await;
        let handoff = settlement.handoff.expect("dirty handoff");
        assert_eq!(settlement.cleanup, WorkspaceCleanup::Preserved);
        assert_eq!(handoff.workspace, path);
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
                SubagentWorkspacePolicy::GitWorktree {
                    require_clean_parent: false,
                },
                &SubagentId::new("conversation-committed-child-subagent-1"),
                &CancellationSignal::new(),
            )
            .await
            .expect("worktree");
        let base = lease.snapshot().base_commit.clone().expect("base");
        std::fs::write(lease.workspace().join("tracked.txt"), "committed child\n")
            .expect("child file");
        commit(lease.workspace(), "child commit");
        let settlement = lease.settle_after_child().await;
        let handoff = settlement.handoff.expect("committed handoff");
        assert_eq!(settlement.cleanup, WorkspaceCleanup::Preserved);
        assert!(!handoff.dirty);
        assert_ne!(handoff.head_commit, base);
        assert_eq!(handoff.head_commit, head(&handoff.workspace));
    }

    #[tokio::test]
    async fn committed_child_work_with_ignored_cache_is_dirty_false_but_changed() {
        let dir = repository();
        ignore_target(dir.path());
        let manager = SubagentWorkspaceManager::new(dir.path(), dir.path().join("artifacts"));
        let lease = manager
            .acquire(
                SubagentWorkspacePolicy::GitWorktree {
                    require_clean_parent: false,
                },
                &SubagentId::new("conversation-committed-ignored-subagent-1"),
                &CancellationSignal::new(),
            )
            .await
            .expect("worktree");
        let base = lease.snapshot().base_commit.clone().expect("base");
        std::fs::write(lease.workspace().join("tracked.txt"), "child commit\n")
            .expect("child file");
        commit(lease.workspace(), "child commit");
        std::fs::create_dir_all(lease.workspace().join("target/debug")).expect("target");
        std::fs::write(lease.workspace().join("target/debug/generated"), "cache\n").expect("cache");

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
                SubagentWorkspacePolicy::GitWorktree {
                    require_clean_parent: false,
                },
                &SubagentId::new("conversation-source-ignored-subagent-1"),
                &CancellationSignal::new(),
            )
            .await
            .expect("worktree");
        std::fs::write(
            lease.workspace().join("new-source.rs"),
            "fn generated() {}\n",
        )
        .expect("source");
        std::fs::create_dir_all(lease.workspace().join("target/debug")).expect("target");
        std::fs::write(lease.workspace().join("target/debug/generated"), "cache\n").expect("cache");

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
                SubagentWorkspacePolicy::GitWorktree {
                    require_clean_parent: false,
                },
                &SubagentId::new("conversation-recovered-ignored-subagent-1"),
                &CancellationSignal::new(),
            )
            .await
            .expect("worktree");
        let snapshot = lease.snapshot().clone();
        std::fs::create_dir_all(lease.workspace().join("target/debug")).expect("target");
        std::fs::write(lease.workspace().join("target/debug/generated"), "cache\n").expect("cache");
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
                SubagentWorkspacePolicy::GitWorktree {
                    require_clean_parent: false,
                },
                &SubagentId::new("conversation-committed-dirty-subagent-1"),
                &CancellationSignal::new(),
            )
            .await
            .expect("worktree");
        let base = lease.snapshot().base_commit.clone().expect("base");
        std::fs::write(lease.workspace().join("tracked.txt"), "child commit\n")
            .expect("child file");
        commit(lease.workspace(), "child commit");
        std::fs::write(lease.workspace().join("uncommitted.txt"), "still active\n")
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
                SubagentWorkspacePolicy::GitWorktree {
                    require_clean_parent: false,
                },
                &SubagentId::new("conversation-staged-dirty-subagent-1"),
                &CancellationSignal::new(),
            )
            .await
            .expect("worktree");
        let path = lease.workspace().to_path_buf();
        std::fs::write(path.join("staged.txt"), "unknown staged work\n").expect("staged file");
        let error = lease
            .settle_staged()
            .await
            .expect_err("unexpected staged work must block cleanup");
        assert!(error.settlement.handoff.is_some());
        assert!(error.settlement.snapshot.workspace.exists());
        assert!(path.join("staged.txt").exists());
    }

    #[tokio::test]
    async fn cancellation_after_worktree_creation_settles_the_staged_lease() {
        let dir = repository();
        let runtime_root = dir.path().join("artifacts");
        let mut manager = SubagentWorkspaceManager::new(dir.path(), &runtime_root);
        let hook = std::sync::Arc::new(WorkspaceAcquireHook::new());
        manager.install_acquisition_hook(hook.clone());
        let cancellation = CancellationSignal::new();
        let subagent = SubagentId::new("conversation-cancelled-acquisition-subagent-1");
        let task_manager = manager.clone();
        let task_cancellation = cancellation.clone();
        let task_subagent = subagent.clone();
        let task = tokio::spawn(async move {
            task_manager
                .acquire(
                    SubagentWorkspacePolicy::GitWorktree {
                        require_clean_parent: false,
                    },
                    &task_subagent,
                    &task_cancellation,
                )
                .await
        });
        hook.wait_until_created().await;
        let path = runtime_root
            .join("worktrees")
            .join(deterministic_worktree_name(&subagent));
        assert!(path.exists(), "the barrier is after Git worktree creation");
        cancellation.cancel();
        hook.release().await;
        let error = task
            .await
            .expect("acquisition task")
            .expect_err("cancellation");
        assert!(matches!(error, super::WorkspaceAcquireError::Cancelled));
        assert!(!path.exists(), "clean staged worktree is settled");
    }

    #[tokio::test]
    async fn concurrent_children_have_distinct_deterministic_paths_and_refs() {
        let dir = repository();
        let manager = SubagentWorkspaceManager::new(dir.path(), dir.path().join("artifacts"));
        let first_id = SubagentId::new("conversation-concurrent-subagent-1");
        let second_id = SubagentId::new("conversation-concurrent-subagent-2");
        let cancellation = CancellationSignal::new();
        let (first, second) = tokio::join!(
            manager.acquire(
                SubagentWorkspacePolicy::GitWorktree {
                    require_clean_parent: false,
                },
                &first_id,
                &cancellation,
            ),
            manager.acquire(
                SubagentWorkspacePolicy::GitWorktree {
                    require_clean_parent: false,
                },
                &second_id,
                &cancellation,
            )
        );
        let first = first.expect("first worktree");
        let second = second.expect("second worktree");
        assert_ne!(first.workspace(), second.workspace());
        assert_ne!(first.snapshot().branch, second.snapshot().branch);
        let first_settlement = first.settle_after_child().await;
        let second_settlement = second.settle_after_child().await;
        assert_eq!(first_settlement.cleanup, WorkspaceCleanup::Removed);
        assert_eq!(second_settlement.cleanup, WorkspaceCleanup::Removed);
    }

    #[tokio::test]
    async fn same_identity_collision_cannot_settle_another_lease() {
        let dir = repository();
        let manager = SubagentWorkspaceManager::new(dir.path(), dir.path().join("artifacts"));
        let subagent = SubagentId::new("conversation-same-identity-subagent-1");
        let first = manager
            .acquire(
                SubagentWorkspacePolicy::GitWorktree {
                    require_clean_parent: false,
                },
                &subagent,
                &CancellationSignal::new(),
            )
            .await
            .expect("first worktree");
        let path = first.workspace().to_path_buf();
        let second = manager
            .acquire(
                SubagentWorkspacePolicy::GitWorktree {
                    require_clean_parent: false,
                },
                &subagent,
                &CancellationSignal::new(),
            )
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
        let manager = SubagentWorkspaceManager::new(dir.path(), dir.path().join("artifacts"));
        let subagent = SubagentId::new("conversation-concurrent-same-identity-subagent-1");
        let cancellation = CancellationSignal::new();
        let (left, right) = tokio::join!(
            manager.acquire(
                SubagentWorkspacePolicy::GitWorktree {
                    require_clean_parent: false,
                },
                &subagent,
                &cancellation,
            ),
            manager.acquire(
                SubagentWorkspacePolicy::GitWorktree {
                    require_clean_parent: false,
                },
                &subagent,
                &cancellation,
            )
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

    #[tokio::test]
    async fn strict_parent_policy_rejects_before_worktree_creation() {
        let dir = repository();
        std::fs::write(dir.path().join("tracked.txt"), "dirty\n").expect("dirty file");
        let manager = SubagentWorkspaceManager::new(dir.path(), dir.path().join("artifacts"));
        let error = manager
            .acquire(
                SubagentWorkspacePolicy::GitWorktree {
                    require_clean_parent: true,
                },
                &SubagentId::new("conversation-a-subagent-1"),
                &CancellationSignal::new(),
            )
            .await
            .expect_err("dirty parent");
        assert!(matches!(
            error,
            super::WorkspaceAcquireError::DirtyParent { .. }
        ));
        assert!(!dir.path().join("artifacts/worktrees").exists());
    }
}
