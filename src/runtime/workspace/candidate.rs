//! Run retention and exclusive access to one native physical lease.
//!
//! Handles are process-local authority, not paths or serialized references.
//! Abandoning an admitted handle poisons the scope: dropping a future cannot
//! establish physical settlement or make another writer eligible.
use super::{
    Arc, BTreeSet, CancellationSignal, Deserialize, Digest, ErrorKind, OsString, Path, PathBuf,
    Read, Serialize, Sha256, WorkspaceHandoff, WorkspaceLease, WorkspaceOwner, WorkspacePolicy,
    WorkspaceSettlement, WorkspaceSettlementDisposition, WorkspaceSettlementError,
    WorkspaceSnapshot, bound_settlement_detail, git_failure_detail, is_safe_repository_relative,
    open_directory_relative, open_overlay_source,
};
use crate::runtime::workflow::{WorkflowNodeInstance, WorkflowRunId};
use tokio::sync::{Mutex, OwnedMutexGuard};
mod mutation;

/// Historical source identity. Possession of this value grants no access.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateReference {
    pub run: WorkflowRunId,
    pub version: u64,
    pub content: String,
}

#[derive(Debug, Clone)]
struct UnresolvedCandidate {
    reason: super::WorkspaceUnresolvedReason,
    detail: String,
}
impl UnresolvedCandidate {
    fn physical(detail: String) -> Self {
        Self {
            reason: super::WorkspaceUnresolvedReason::PhysicalSettlement,
            detail,
        }
    }
}

#[derive(Debug)]
struct State {
    lease: Option<WorkspaceLease>,
    current: CandidateReference,
    admitted: bool,
    unresolved: Option<UnresolvedCandidate>,
    settlement: Option<WorkspaceSettlement>,
    final_reference: Option<CandidateReference>,
}

/// Logical access to native retained ownership. Clones duplicate neither the
/// physical lease nor disposal authority.
#[derive(Debug, Clone)]
pub(crate) struct CandidateScope {
    state: Arc<Mutex<State>>,
    run: WorkflowRunId,
}

/// One exclusive admitted physical user. Only explicit settlement returns it.
#[derive(Debug)]
pub(crate) struct WorkspaceAccess {
    state: OwnedMutexGuard<State>,
    node: WorkflowNodeInstance,
    input: CandidateReference,
    snapshot: WorkspaceSnapshot,
    mutation: mutation::MutationWatch,
}

/// Physical users either own a one-shot lease or borrow a run's lease.
/// This sum is the process driver's only workspace lifecycle input.
#[derive(Debug)]
pub(crate) enum WorkspaceUse {
    Owned(Box<WorkspaceLease>),
    Borrowed(Box<WorkspaceAccess>),
}

impl From<WorkspaceLease> for WorkspaceUse {
    fn from(lease: WorkspaceLease) -> Self {
        Self::Owned(Box::new(lease))
    }
}

impl From<WorkspaceAccess> for WorkspaceUse {
    fn from(access: WorkspaceAccess) -> Self {
        Self::Borrowed(Box::new(access))
    }
}

impl WorkspaceUse {
    pub(crate) fn snapshot(&self) -> &WorkspaceSnapshot {
        match self {
            Self::Owned(lease) => lease.snapshot(),
            Self::Borrowed(access) => access.snapshot(),
        }
    }
    pub(crate) fn logical_workspace(&self) -> &Path {
        &self.snapshot().logical_workspace
    }

    pub(crate) async fn settle_after_child(self) -> WorkspaceSettlement {
        match self {
            Self::Owned(lease) => lease.settle_after_child().await,
            Self::Borrowed(access) => {
                let snapshot = access.snapshot().clone();
                // Source inspection errors poison the run owner. They never
                // give the child a second retained/disposable worktree.
                let _ = access.finish(false).await;
                WorkspaceSettlement {
                    snapshot,
                    disposition: WorkspaceSettlementDisposition::Borrowed,
                }
            }
        }
    }
    pub(crate) async fn settle_staged(
        self,
    ) -> Result<WorkspaceSettlement, WorkspaceSettlementError> {
        match self {
            Self::Owned(lease) => lease.settle_staged().await,
            borrowed @ Self::Borrowed(_) => Ok(borrowed.settle_after_child().await),
        }
    }
    pub(crate) fn preserve_after_unresolved_nested(
        self,
        detail: impl Into<String>,
    ) -> WorkspaceSettlement {
        match self {
            Self::Owned(lease) => lease.preserve_after_unresolved_nested(detail),
            Self::Borrowed(access) => {
                let snapshot = access.snapshot().clone();
                access.unresolved(detail.into());
                WorkspaceSettlement {
                    snapshot,
                    disposition: WorkspaceSettlementDisposition::Borrowed,
                }
            }
        }
    }
}

impl WorkspaceLease {
    /// Commits native run retention after exact inspection. Failure returns
    /// the same lease so the caller must settle staged ownership.
    pub(crate) async fn retain_for_run(
        self,
        run: WorkflowRunId,
    ) -> Result<CandidateScope, (Box<Self>, String)> {
        if self.owner != WorkspaceOwner::Workflow(run.clone()) || !self.created {
            return Err((
                Box::new(self),
                "lease is not owned by this Workflow run".into(),
            ));
        }
        let content = match self.source_identity().await {
            Ok(content) => content,
            Err(error) => return Err((Box::new(self), error)),
        };
        Ok(CandidateScope {
            run: run.clone(),
            state: Arc::new(Mutex::new(State {
                lease: Some(self),
                current: CandidateReference {
                    run,
                    version: 0,
                    content,
                },
                admitted: false,
                unresolved: None,
                settlement: None,
                final_reference: None,
            })),
        })
    }

    /// Native content inspection under exclusive ownership. No Git index,
    /// source file, or branch is written by this operation.
    async fn source_identity(&self) -> Result<String, String> {
        inspect_source(&self.manager, &self.owner, &self.snapshot).await
    }

    async fn source_listing(&self, args: &[&str]) -> Result<Vec<u8>, String> {
        source_listing(&self.manager, &self.snapshot, args).await
    }
}

pub(super) async fn inspect_source(
    manager: &super::WorkspaceManager,
    owner: &WorkspaceOwner,
    snapshot: &WorkspaceSnapshot,
) -> Result<String, String> {
    let tree = snapshot
        .git_worktree()
        .ok_or("candidate requires isolation")?;
    let head = manager
        .git_text(
            &tree.physical_worktree_root,
            vec!["rev-parse".into(), "HEAD".into()],
            None,
        )
        .await
        .map_err(|e| e.to_string())?;
    let handoff = WorkspaceHandoff {
        logical_workspace: snapshot.logical_workspace.clone(),
        physical_worktree_root: tree.physical_worktree_root.clone(),
        branch: tree.branch.clone(),
        base_commit: tree.base_commit.clone(),
        head_commit: head.clone(),
        dirty: false,
    };
    manager
        .verify_retained_workspace(owner, snapshot, &handoff)
        .await
        .map_err(|e| e.to_string())?;
    let first = hash_source(manager, snapshot, &head).await?;
    let second = hash_source(manager, snapshot, &head).await?;
    if first != second {
        return Err("candidate changed during native source inspection".into());
    }
    manager
        .verify_retained_workspace(owner, snapshot, &handoff)
        .await
        .map_err(|e| e.to_string())?;
    Ok(first)
}

async fn source_listing(
    manager: &super::WorkspaceManager,
    snapshot: &WorkspaceSnapshot,
    args: &[&str],
) -> Result<Vec<u8>, String> {
    let root = &snapshot
        .git_worktree()
        .ok_or("missing worktree")?
        .physical_worktree_root;
    let output = manager
        .git_raw(root, args.iter().map(OsString::from).collect(), None)
        .await
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(git_failure_detail(&output));
    }
    if output.stdout.len() > 16 * 1024 * 1024 {
        return Err("candidate source listing exceeds 16 MiB".into());
    }
    Ok(output.stdout)
}

async fn hash_source(
    manager: &super::WorkspaceManager,
    snapshot: &WorkspaceSnapshot,
    head: &str,
) -> Result<String, String> {
    let index = source_listing(manager, snapshot, &["ls-files", "--stage", "-z"]).await?;
    for entry in index.split(|b| *b == 0).filter(|entry| !entry.is_empty()) {
        let header = entry
            .split(|b| *b == b'\t')
            .next()
            .ok_or("invalid index record")?;
        if header.starts_with(b"160000 ") || !header.ends_with(b" 0") {
            return Err("candidate gitlinks and unmerged index stages are unsupported".into());
        }
    }
    let flags = source_listing(manager, snapshot, &["ls-files", "-v", "-z"]).await?;
    if flags
        .split(|b| *b == 0)
        .filter(|e| !e.is_empty())
        .any(|e| e[0] != b'H')
    {
        return Err("candidate sparse/assume-unchanged index entries are unsupported".into());
    }
    let paths = source_listing(
        manager,
        snapshot,
        &[
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ],
    )
    .await?;
    let committed = source_listing(
        manager,
        snapshot,
        &["ls-tree", "-r", "--name-only", "-z", "HEAD"],
    )
    .await?;
    let paths: BTreeSet<Vec<u8>> = paths
        .split(|b| *b == 0)
        .chain(committed.split(|b| *b == 0))
        .filter(|p| !p.is_empty())
        .map(<[u8]>::to_vec)
        .collect();
    if paths.len() > 100_000 {
        return Err("candidate exceeds 100000 source paths".into());
    }
    let mut hash = Sha256::new();
    field(&mut hash, b"rustx-candidate-source-v1");
    field(
        &mut hash,
        &serde_json::to_vec(snapshot).map_err(|e| e.to_string())?,
    );
    field(&mut hash, head.as_bytes());
    field(&mut hash, &index);
    let allocation =
        super::open_stable_runtime_worktrees(&manager.runtime_root).map_err(|e| e.to_string())?;
    let root = super::open_stable_child_logical_workspace(
        &allocation,
        &snapshot
            .git_worktree()
            .ok_or("missing worktree")?
            .physical_worktree_root,
        Path::new(""),
    )
    .map_err(|e| e.to_string())?;
    let mut remaining = 256 * 1024 * 1024;
    for path in paths {
        field(&mut hash, &path);
        hash_path(&root, &path, &mut hash, &mut remaining)?;
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn field(hash: &mut Sha256, bytes: &[u8]) {
    hash.update((bytes.len() as u64).to_be_bytes());
    hash.update(bytes);
}

#[cfg(unix)]
fn hash_path(
    root: &std::fs::File,
    bytes: &[u8],
    hash: &mut Sha256,
    remaining: &mut u64,
) -> Result<(), String> {
    use nix::fcntl::{AtFlags, readlinkat};
    use nix::sys::stat::{SFlag, fstatat};
    use std::os::unix::{ffi::OsStrExt, fs::MetadataExt};
    let path = Path::new(std::ffi::OsStr::from_bytes(bytes));
    if !is_safe_repository_relative(path) {
        return Err("unsafe candidate source path".into());
    }
    let parent = match open_directory_relative(root, path.parent().ok_or("missing parent")?) {
        Ok(parent) => parent,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            field(hash, b"deleted");
            return Ok(());
        }
        Err(error) => return Err(error.to_string()),
    };
    let name = path.file_name().ok_or("missing filename")?;
    let stat = match fstatat(&parent, name, AtFlags::AT_SYMLINK_NOFOLLOW) {
        Ok(stat) => stat,
        Err(nix::errno::Errno::ENOENT) => {
            field(hash, b"deleted");
            return Ok(());
        }
        Err(error) => return Err(error.to_string()),
    };
    let kind = SFlag::from_bits_truncate(stat.st_mode) & SFlag::S_IFMT;
    if kind == SFlag::S_IFLNK {
        field(hash, b"symlink");
        let target = readlinkat(&parent, name).map_err(|e| e.to_string())?;
        field(hash, target.as_bytes());
    } else if kind == SFlag::S_IFREG {
        let file = open_overlay_source(root, path).map_err(|e| e.to_string())?;
        let metadata = file.metadata().map_err(|e| e.to_string())?;
        if !metadata.is_file() {
            return Err("candidate file changed type during inspection".into());
        }
        field(
            hash,
            if metadata.mode() & 0o111 == 0 {
                b"regular"
            } else {
                b"executable"
            },
        );
        let mut bytes = Vec::new();
        file.take(*remaining + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        *remaining = remaining
            .checked_sub(bytes.len() as u64)
            .ok_or("candidate exceeds 256 MiB")?;
        field(hash, &bytes);
    } else {
        return Err("candidate directories, gitlinks and special files are unsupported".into());
    }
    Ok(())
}

#[cfg(not(unix))]
fn hash_path(_: &std::fs::File, _: &[u8], _: &mut Sha256, _: &mut u64) -> Result<(), String> {
    Err("candidate inspection requires Unix descriptor-relative filesystem support".into())
}

impl CandidateScope {
    /// Applicability only: this neither grants access nor changes the candidate.
    pub(crate) async fn assert_current(
        &self,
        reference: &CandidateReference,
    ) -> Result<(), String> {
        let state = self.state.lock().await;
        if reference.run != self.run || *reference != state.current {
            return Err("stale candidate reference".into());
        }
        if state.admitted || state.unresolved.is_some() || state.lease.is_none() {
            return Err("candidate currentness is unproven".into());
        }
        Ok(())
    }

    /// Queue before any descendant capacity or Tool scheduling admission.
    /// A stale expected version fails rather than being rebound to new bytes.
    pub(crate) async fn borrow(
        &self,
        node: WorkflowNodeInstance,
        expected: Option<&CandidateReference>,
        cancellation: &CancellationSignal,
    ) -> Result<WorkspaceAccess, String> {
        if node.block.run != self.run {
            return Err("wrong candidate run/node identity".into());
        }
        let mut state = tokio::select! {
            biased;
            () = cancellation.cancelled() => return Err("candidate admission cancelled".into()),
            state = self.state.clone().lock_owned() => state,
        };
        if cancellation.is_cancelled() {
            return Err("candidate admission cancelled".into());
        }
        if state.admitted || state.unresolved.is_some() {
            return Err("previous candidate user is unresolved".into());
        }
        if expected.is_some_and(|reference| *reference != state.current) {
            return Err("stale candidate reference".into());
        }
        let lease = state.lease.as_ref().ok_or("candidate lease was released")?;
        let mutation = mutation::MutationWatch::start(lease).await?;
        let content = lease.source_identity().await?;
        if content != state.current.content {
            state.unresolved = Some(UnresolvedCandidate::physical(
                "external source interference invalidated the candidate".into(),
            ));
            return Err("external source interference invalidated the candidate".into());
        }
        if cancellation.is_cancelled() {
            return Err("candidate admission cancelled".into());
        }
        state.admitted = true;
        let input = state.current.clone();
        let mut snapshot = state
            .lease
            .as_ref()
            .expect("admitted lease")
            .snapshot
            .clone();
        snapshot.borrowed_from = Some(self.run.clone());
        Ok(WorkspaceAccess {
            state,
            node,
            input,
            snapshot,
            mutation,
        })
    }

    /// One absorbing settlement, after every exclusive physical user returns.
    /// Repetition returns the committed result without re-inspection/removal.
    pub(crate) async fn settle(&self) -> WorkspaceSettlement {
        let mut state = self.state.lock().await;
        if let Some(settlement) = &state.settlement {
            return settlement.clone();
        }
        let lease = state.lease.take().expect("unsettled scope owns its lease");
        if !state.admitted && state.unresolved.is_none() {
            match lease.source_identity().await {
                Ok(content) if content == state.current.content => {
                    state.final_reference = Some(state.current.clone());
                }
                Ok(_) => {
                    state.unresolved = Some(UnresolvedCandidate::physical(
                        "external source interference before run settlement".into(),
                    ));
                }
                Err(error) => state.unresolved = Some(UnresolvedCandidate::physical(error)),
            }
        }
        let settlement = if state.admitted {
            lease.preserve_after_unresolved_nested(state.unresolved.as_ref().map_or_else(
                || "candidate user abandoned without physical settlement".into(),
                |unresolved| unresolved.detail.clone(),
            ))
        } else if let Some(unresolved) = &state.unresolved {
            match unresolved.reason {
                super::WorkspaceUnresolvedReason::NestedContainment => {
                    lease.preserve_after_unresolved_nested(unresolved.detail.clone())
                }
                super::WorkspaceUnresolvedReason::PhysicalSettlement => {
                    lease.preserve_after_settled_inspection(unresolved.detail.clone())
                }
            }
        } else {
            lease.settle().await
        };
        if settlement.unresolved_reason().is_some() {
            state.final_reference = None;
        }
        state.settlement = Some(settlement.clone());
        settlement
    }

    pub(crate) async fn final_reference(&self) -> Option<CandidateReference> {
        self.state.lock().await.final_reference.clone()
    }
}

impl WorkspaceAccess {
    pub(crate) fn snapshot(&self) -> &WorkspaceSnapshot {
        &self.snapshot
    }

    pub(crate) fn input(&self) -> &CandidateReference {
        &self.input
    }

    pub(crate) fn node(&self) -> &WorkflowNodeInstance {
        &self.node
    }

    pub(crate) fn policy(&self) -> WorkspacePolicy {
        self.state.lease.as_ref().expect("admitted lease").policy
    }

    /// Called only after physical work settles. Mutating consumers publish a
    /// new version; validation consumers fail if they changed their input.
    pub(crate) async fn finish(mut self, validation: bool) -> Result<CandidateReference, String> {
        let inspected = self
            .state
            .lease
            .as_ref()
            .ok_or("missing admitted lease")?
            .source_identity()
            .await;
        let mutation = if validation {
            self.mutation
                .changed(self.state.lease.as_ref().expect("admitted lease"))
                .await
        } else {
            Ok(false)
        };
        self.state.admitted = false;
        let mutation = match mutation {
            Ok(mutation) => mutation,
            Err(error) => {
                self.state.unresolved = Some(UnresolvedCandidate::physical(error.clone()));
                return Err(error);
            }
        };
        let content = match inspected {
            Ok(content) => content,
            Err(error) => {
                self.state.unresolved = Some(UnresolvedCandidate::physical(error.clone()));
                return Err(error);
            }
        };
        if content != self.input.content {
            self.state.current.version = self
                .state
                .current
                .version
                .checked_add(1)
                .ok_or("candidate version exhausted")?;
            self.state.current.content = content;
            if validation {
                return Err(
                    "validation mutated its candidate; the old reference is invalid".into(),
                );
            }
        }
        if validation && mutation {
            self.state.current.version = self
                .state
                .current
                .version
                .checked_add(1)
                .ok_or("candidate version exhausted")?;
            return Err(
                "source mutation during validation invalidated its candidate reference".into(),
            );
        }
        Ok(self.state.current.clone())
    }

    /// Unknown containment cannot release writer or disposal eligibility.
    pub(crate) fn unresolved(mut self, detail: String) {
        self.state.unresolved = Some(UnresolvedCandidate {
            reason: super::WorkspaceUnresolvedReason::NestedContainment,
            detail: bound_settlement_detail(detail),
        });
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::runtime::workspace::{
        WorkspaceCleanup, WorkspaceManager, WorkspaceUnresolvedReason,
    };
    use std::os::unix::fs::{PermissionsExt, symlink};

    fn git(path: &Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(path)
            .args(args)
            .env("GIT_AUTHOR_NAME", "fixture")
            .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
            .env("GIT_COMMITTER_NAME", "fixture")
            .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().into()
    }

    struct Fixture {
        source: tempfile::TempDir,
        runtime: tempfile::TempDir,
        manager: WorkspaceManager,
        node: WorkflowNodeInstance,
    }
    impl Fixture {
        fn new() -> Self {
            let source = tempfile::tempdir().unwrap();
            git(source.path(), &["init"]);
            std::fs::write(source.path().join("source"), b"baseline\n").unwrap();
            std::fs::write(source.path().join(".gitignore"), b"/target/\n").unwrap();
            git(source.path(), &["add", "."]);
            git(source.path(), &["commit", "-m", "baseline"]);
            let runtime = tempfile::tempdir().unwrap();
            let manager = WorkspaceManager::new(
                std::fs::canonicalize(source.path()).unwrap(),
                runtime.path(),
            );
            Self {
                source,
                runtime,
                manager,
                node: crate::runtime::workflow::test_instance("candidate", "write"),
            }
        }
        async fn acquire(&self) -> CandidateScope {
            let run = self.node.block.run.clone();
            self.manager
                .acquire(
                    WorkspacePolicy::GitWorktree {
                        require_clean_parent: true,
                    },
                    &WorkspaceOwner::Workflow(run.clone()),
                    &CancellationSignal::new(),
                )
                .await
                .unwrap()
                .retain_for_run(run)
                .await
                .unwrap()
        }
        async fn access(&self, scope: &CandidateScope) -> WorkspaceAccess {
            scope
                .borrow(self.node.clone(), None, &CancellationSignal::new())
                .await
                .unwrap()
        }
    }

    #[tokio::test]
    async fn exact_dirty_handoff_retains_lease_across_child_settlement_and_freezes_baseline() {
        let fixture = Fixture::new();
        let scope = fixture.acquire().await;
        let first = fixture.access(&scope).await;
        let path = first.snapshot().logical_workspace.clone();
        let base = first.snapshot().git_worktree().unwrap().base_commit.clone();
        std::fs::write(path.join("uncommitted"), b"exact upstream bytes\0\xff").unwrap();
        let settled = WorkspaceUse::from(first).settle_after_child().await;
        assert_eq!(
            settled.disposition,
            WorkspaceSettlementDisposition::Borrowed
        );
        assert!(path.exists());
        std::fs::write(fixture.source.path().join("source"), b"new parent").unwrap();
        git(fixture.source.path(), &["add", "."]);
        git(fixture.source.path(), &["commit", "-m", "later parent"]);
        let next = fixture.access(&scope).await;
        assert_eq!(next.snapshot().logical_workspace, path);
        assert_eq!(next.snapshot().git_worktree().unwrap().base_commit, base);
        assert_eq!(git(&path, &["rev-parse", "HEAD"]), base);
        assert_eq!(
            std::fs::read(path.join("uncommitted")).unwrap(),
            b"exact upstream bytes\0\xff"
        );
        assert!(!fixture.source.path().join("uncommitted").exists());
        next.finish(true).await.unwrap();
        let terminal = scope.settle().await;
        assert!(terminal.handoff().unwrap().dirty);
        assert_eq!(terminal, scope.settle().await);
    }

    #[tokio::test]
    async fn physical_user_blocks_next_writer_and_disposal_until_explicit_settlement() {
        let fixture = Fixture::new();
        let scope = fixture.acquire().await;
        let first = fixture.access(&scope).await;
        let path = first.snapshot().logical_workspace.clone();
        let cancellation = CancellationSignal::new();
        let mut next = Box::pin(scope.borrow(fixture.node.clone(), None, &cancellation));
        assert!(futures_util::poll!(&mut next).is_pending());
        let mut settlement = Box::pin(scope.settle());
        assert!(futures_util::poll!(&mut settlement).is_pending());
        assert!(path.exists());
        drop(settlement);
        first.finish(false).await.unwrap();
        next.await.unwrap().finish(false).await.unwrap();
        assert_eq!(scope.settle().await.cleanup(), WorkspaceCleanup::Removed);
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn dropping_access_never_proves_physical_settlement() {
        let fixture = Fixture::new();
        let scope = fixture.acquire().await;
        let access = fixture.access(&scope).await;
        let path = access.snapshot().logical_workspace.clone();
        drop(access);
        assert!(
            scope
                .borrow(fixture.node.clone(), None, &CancellationSignal::new())
                .await
                .is_err()
        );
        assert_eq!(
            scope.settle().await.unresolved_reason(),
            Some(WorkspaceUnresolvedReason::NestedContainment)
        );
        assert!(path.exists());
    }

    #[tokio::test]
    async fn dirty_bytes_index_modes_deletions_symlinks_and_untracked_files_change_identity() {
        let fixture = Fixture::new();
        let scope = fixture.acquire().await;
        let access = fixture.access(&scope).await;
        let path = access.snapshot().logical_workspace.clone();
        let mut reference = access.finish(false).await.unwrap();
        for change in 0..6 {
            let access = fixture.access(&scope).await;
            match change {
                0 => std::fs::write(path.join("source"), b"dirty").unwrap(),
                1 => {
                    git(&path, &["add", "source"]);
                }
                2 => std::fs::set_permissions(
                    path.join("source"),
                    std::fs::Permissions::from_mode(0o755),
                )
                .unwrap(),
                3 => std::fs::write(path.join("new"), b"untracked").unwrap(),
                4 => symlink("source", path.join("link")).unwrap(),
                _ => std::fs::remove_file(path.join("source")).unwrap(),
            }
            let next = access.finish(false).await.unwrap();
            assert_ne!(reference.content, next.content);
            assert!(
                scope
                    .borrow(
                        fixture.node.clone(),
                        Some(&reference),
                        &CancellationSignal::new()
                    )
                    .await
                    .is_err()
            );
            reference = next;
        }
        assert!(scope.settle().await.handoff().is_some());
    }

    #[tokio::test]
    async fn ignored_build_bytes_do_not_change_source_candidate() {
        let fixture = Fixture::new();
        let scope = fixture.acquire().await;
        let access = fixture.access(&scope).await;
        let target = access.snapshot().logical_workspace.join("target");
        std::fs::create_dir(&target).unwrap();
        access.finish(false).await.unwrap();
        let access = fixture.access(&scope).await;
        let input = access.input().clone();
        std::fs::write(target.join("cache"), b"build cache").unwrap();
        #[cfg(target_os = "linux")]
        assert_eq!(access.finish(true).await.unwrap(), input);
        #[cfg(target_os = "macos")]
        {
            let _ = input;
            assert!(
                access.finish(true).await.is_err(),
                "vnode cannot distinguish ignored child creation from new unwatched directories"
            );
        }
        scope.settle().await;
    }

    #[tokio::test]
    async fn validator_mutation_invalidates_old_reference_at_settlement() {
        let fixture = Fixture::new();
        let scope = fixture.acquire().await;
        let access = fixture.access(&scope).await;
        let old = access.input().clone();
        let path = access.snapshot().logical_workspace.clone();
        let (mutate, gate) = tokio::sync::oneshot::channel();
        let writer = tokio::spawn(async move {
            gate.await.unwrap();
            std::fs::write(path.join("source"), b"validator write").unwrap();
        });
        mutate.send(()).unwrap();
        writer.await.unwrap();
        assert!(access.finish(true).await.is_err());
        assert!(
            scope
                .borrow(fixture.node.clone(), Some(&old), &CancellationSignal::new())
                .await
                .is_err()
        );
        assert!(scope.settle().await.handoff().is_some());
    }

    #[tokio::test]
    async fn external_mutation_between_borrowers_invalidates_admission() {
        let fixture = Fixture::new();
        let scope = fixture.acquire().await;
        let access = fixture.access(&scope).await;
        let path = access.snapshot().logical_workspace.clone();
        access.finish(false).await.unwrap();
        std::fs::write(path.join("source"), b"external").unwrap();
        assert!(
            scope
                .borrow(fixture.node.clone(), None, &CancellationSignal::new())
                .await
                .is_err()
        );
        assert_eq!(scope.settle().await.cleanup(), WorkspaceCleanup::Preserved);
    }

    #[tokio::test]
    async fn wrong_run_cancelled_wait_and_use_after_release_fail_closed() {
        let fixture = Fixture::new();
        let scope = fixture.acquire().await;
        let mut wrong = fixture.node.clone();
        wrong.block.run.invocation += 1;
        assert!(
            scope
                .borrow(wrong, None, &CancellationSignal::new())
                .await
                .is_err()
        );
        let first = fixture.access(&scope).await;
        let signal = CancellationSignal::new();
        let mut waiting = Box::pin(scope.borrow(fixture.node.clone(), None, &signal));
        assert!(futures_util::poll!(&mut waiting).is_pending());
        signal.cancel();
        assert!(waiting.await.is_err());
        first.finish(false).await.unwrap();
        let terminal = scope.settle().await;
        assert!(
            scope
                .borrow(fixture.node.clone(), None, &CancellationSignal::new())
                .await
                .is_err()
        );
        assert_eq!(scope.settle().await, terminal);
    }

    #[tokio::test]
    async fn active_lease_rejects_disposal_even_with_exact_snapshot_and_head() {
        let fixture = Fixture::new();
        let scope = fixture.acquire().await;
        let access = fixture.access(&scope).await;
        let mut snapshot = access.snapshot().clone();
        snapshot.borrowed_from = None;
        let tree = snapshot.git_worktree().unwrap();
        let handoff = WorkspaceHandoff {
            logical_workspace: snapshot.logical_workspace.clone(),
            physical_worktree_root: tree.physical_worktree_root.clone(),
            branch: tree.branch.clone(),
            base_commit: tree.base_commit.clone(),
            head_commit: tree.base_commit.clone(),
            dirty: false,
        };
        let store = crate::durable::SqliteConversationStore::in_memory(
            fixture.node.block.run.conversation_id.clone(),
        )
        .unwrap();
        assert!(
            fixture
                .manager
                .dispose_workflow_workspace(&store, &fixture.node.block.run)
                .await
                .is_err()
        );
        assert!(
            fixture
                .manager
                .dispose_retained_workspace(
                    &crate::runtime::identity::SubagentId::for_conversation(
                        &fixture.node.block.run.conversation_id,
                        1
                    ),
                    &snapshot,
                    &handoff
                )
                .await
                .is_err()
        );
        assert!(snapshot.logical_workspace.exists());
        access.finish(false).await.unwrap();
        scope.settle().await;
    }

    #[tokio::test]
    async fn restored_bytes_still_invalidate_validation_and_stale_reference() {
        let fixture = Fixture::new();
        let scope = fixture.acquire().await;
        let access = fixture.access(&scope).await;
        let old = access.input().clone();
        let source = access.snapshot().logical_workspace.join("source");
        let original = std::fs::read(&source).unwrap();
        std::fs::write(&source, b"transient interference").unwrap();
        std::fs::write(&source, original).unwrap();
        assert!(access.finish(true).await.is_err());
        assert!(
            scope
                .borrow(fixture.node.clone(), Some(&old), &CancellationSignal::new())
                .await
                .is_err()
        );
        scope.settle().await;
    }

    #[tokio::test]
    async fn run_acquisition_cancellation_before_commit_cleans_only_unchanged_staging() {
        for dirty in [false, true] {
            let mut fixture = Fixture::new();
            let hook = Arc::new(super::super::WorkspaceAcquireHook::new());
            fixture.manager.install_acquisition_hook(hook.clone());
            let manager = fixture.manager.clone();
            let owner = WorkspaceOwner::Workflow(fixture.node.block.run.clone());
            let path = fixture
                .runtime
                .path()
                .join("worktrees")
                .join(super::super::deterministic_worktree_name(&owner));
            let signal = CancellationSignal::new();
            let cancellation = signal.clone();
            let task = tokio::spawn(async move {
                manager
                    .acquire(
                        WorkspacePolicy::GitWorktree {
                            require_clean_parent: true,
                        },
                        &owner,
                        &cancellation,
                    )
                    .await
            });
            hook.wait_until_ready_to_return().await;
            if dirty {
                std::fs::write(path.join("unexpected"), b"preserve pre-commit interference")
                    .unwrap();
            }
            signal.cancel();
            hook.release().await;
            assert!(task.await.unwrap().is_err());
            assert_eq!(path.exists(), dirty);
            if dirty {
                assert_eq!(
                    std::fs::read(path.join("unexpected")).unwrap(),
                    b"preserve pre-commit interference"
                );
            }
        }
    }

    #[tokio::test]
    async fn tampered_source_ref_and_path_fail_without_touching_unrelated_workspace() {
        for tamper in 0..3 {
            let fixture = Fixture::new();
            let scope = fixture.acquire().await;
            let unrelated = tempfile::tempdir().unwrap();
            std::fs::write(unrelated.path().join("keep"), b"unrelated").unwrap();
            {
                let mut state = scope.state.lock().await;
                let lease = state.lease.as_mut().unwrap();
                let super::super::WorkspaceIsolation::GitWorktree(tree) =
                    &mut lease.snapshot.isolation
                else {
                    unreachable!()
                };
                match tamper {
                    0 => tree.source_repository_root = unrelated.path().to_path_buf(),
                    1 => tree.branch = "unrelated-ref".into(),
                    _ => {
                        tree.physical_worktree_root = unrelated.path().to_path_buf();
                        lease.snapshot.logical_workspace = unrelated.path().to_path_buf();
                    }
                }
            }
            assert!(
                scope
                    .borrow(fixture.node.clone(), None, &CancellationSignal::new())
                    .await
                    .is_err()
            );
            assert_eq!(scope.settle().await.cleanup(), WorkspaceCleanup::Preserved);
            assert_eq!(
                std::fs::read(unrelated.path().join("keep")).unwrap(),
                b"unrelated"
            );
        }
    }
    fn journal(
        fixture: &Fixture,
        workspace: WorkspaceSettlement,
        candidate: Option<CandidateReference>,
    ) -> Arc<crate::durable::SqliteConversationStore> {
        use crate::durable::ConversationStore;
        use crate::events::types::{EVENT_SCHEMA_VERSION, RuntimeEvent, RuntimeEventEnvelope};
        let run = &fixture.node.block.run;
        let store = Arc::new(
            crate::durable::SqliteConversationStore::in_memory(run.conversation_id.clone())
                .unwrap(),
        );
        for (phase, event) in [
            (
                "owned",
                RuntimeEvent::WorkflowWorkspaceOwned {
                    run_id: run.clone(),
                    workspace: workspace.snapshot.clone(),
                },
            ),
            (
                "settled",
                RuntimeEvent::WorkflowWorkspaceSettled {
                    run_id: run.clone(),
                    workspace,
                    candidate,
                },
            ),
        ] {
            store
                .append_event(RuntimeEventEnvelope {
                    schema_version: EVENT_SCHEMA_VERSION,
                    event_id: super::super::workflow_resource_event_id(run, phase),
                    sequence: 0,
                    conversation_id: run.conversation_id.clone(),
                    attempt_id: None,
                    turn_id: None,
                    timestamp: chrono::Utc::now(),
                    event,
                })
                .unwrap();
        }
        store
    }

    #[tokio::test]
    async fn existing_empty_and_nested_directories_cannot_hide_write_and_restore() {
        for directory in ["tmp", "tmp/nested/empty"] {
            let fixture = Fixture::new();
            let scope = fixture.acquire().await;
            let writer = fixture.access(&scope).await;
            let path = writer.snapshot().logical_workspace.join(directory);
            std::fs::create_dir_all(&path).unwrap();
            writer.finish(false).await.unwrap();
            // Returning access proves all watches are installed before mutation.
            let check = fixture.access(&scope).await;
            let input = check.input().clone();
            let (go, gate) = tokio::sync::oneshot::channel();
            let physical = tokio::spawn(async move {
                gate.await.unwrap();
                let transient = path.join("transient");
                std::fs::write(&transient, b"transient source").unwrap();
                std::fs::remove_file(transient).unwrap();
            });
            go.send(()).unwrap();
            physical.await.unwrap();
            assert_eq!(
                check
                    .state
                    .lease
                    .as_ref()
                    .unwrap()
                    .source_identity()
                    .await
                    .unwrap(),
                input.content
            );
            assert!(check.finish(true).await.is_err());
            assert!(scope.assert_current(&input).await.is_err());
            scope.settle().await;
        }
    }

    #[tokio::test]
    async fn settled_inspection_and_watch_loss_release_active_ownership_for_exact_reproof() {
        for watch_loss in [false, true] {
            let fixture = Fixture::new();
            let scope = fixture.acquire().await;
            let writer = fixture.access(&scope).await;
            let root = writer.snapshot().logical_workspace.clone();
            std::fs::create_dir(root.join("empty")).unwrap();
            writer.finish(false).await.unwrap();
            let access = fixture.access(&scope).await;
            let physical_root = root.clone();
            let index = PathBuf::from(git(
                &root,
                &["rev-parse", "--path-format=absolute", "--git-path", "index"],
            ));
            let original_index = std::fs::read(&index).unwrap();
            let physical_index = index.clone();
            let (go, gate) = tokio::sync::oneshot::channel();
            let physical = tokio::spawn(async move {
                gate.await.unwrap();
                if watch_loss {
                    std::fs::remove_dir(physical_root.join("empty")).unwrap();
                } else {
                    std::fs::write(physical_index, b"invalid index for inspection").unwrap();
                }
            });
            go.send(()).unwrap();
            physical.await.unwrap(); // known physical settlement before inspection
            assert!(access.finish(true).await.is_err());
            let terminal = scope.settle().await;
            assert_eq!(
                terminal.unresolved_reason(),
                Some(WorkspaceUnresolvedReason::PhysicalSettlement)
            );
            assert!(
                fixture
                    .manager
                    .require_released(&WorkspaceOwner::Workflow(fixture.node.block.run.clone()))
                    .is_ok()
            );
            if !watch_loss {
                std::fs::write(index, original_index).unwrap();
            }
            let store = journal(&fixture, terminal, scope.final_reference().await);
            assert_eq!(
                fixture
                    .manager
                    .dispose_workflow_workspace(&*store, &fixture.node.block.run)
                    .await
                    .unwrap(),
                super::super::WorkspaceDisposalSettlement::Disposed
            );
        }
    }

    #[tokio::test]
    async fn live_nested_anchor_preserves_containment_authority_and_rejects_git_only_disposal() {
        let fixture = Fixture::new();
        let scope = fixture.acquire().await;
        let access = fixture.access(&scope).await;
        let mut process = tokio::process::Command::new("sh")
            .args(["-c", "read gate"])
            .stdin(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let gate = process.stdin.take().unwrap();
        access.unresolved("retained live process anchor".into());
        let terminal = scope.settle().await;
        assert_eq!(
            terminal.unresolved_reason(),
            Some(WorkspaceUnresolvedReason::NestedContainment)
        );
        let store = journal(&fixture, terminal, None);
        let run = &fixture.node.block.run;
        assert!(
            fixture
                .manager
                .require_released(&WorkspaceOwner::Workflow(run.clone()))
                .is_err()
        );
        assert!(
            fixture
                .manager
                .dispose_workflow_workspace(&*store, run)
                .await
                .is_err()
        );
        // Even a reopened manager without the process-local active set cannot
        // convert Git facts into missing descendant containment proof.
        let reopened = WorkspaceManager::new(fixture.source.path(), fixture.runtime.path());
        assert!(
            reopened
                .dispose_workflow_workspace(&*store, run)
                .await
                .is_err()
        );
        drop(gate);
        process.wait().await.unwrap();
    }

    #[tokio::test]
    async fn durable_disposal_retries_after_physical_removal_and_failed_settlement_append() {
        use crate::durable::ConversationStore;
        use crate::events::types::RuntimeEvent;
        for branch_removed in [false, true] {
            let mut fixture = Fixture::new();
            let hook = Arc::new(super::super::WorkspaceDisposalHook::new());
            fixture.manager.install_disposal_hook(hook.clone());
            let scope = fixture.acquire().await;
            let writer = fixture.access(&scope).await;
            let root = writer.snapshot().logical_workspace.clone();
            std::fs::write(root.join("source"), b"retained source").unwrap();
            writer.finish(false).await.unwrap();
            let terminal = scope.settle().await;
            let branch = terminal.snapshot.git_worktree().unwrap().branch.clone();
            let store = journal(&fixture, terminal, scope.final_reference().await);
            git(fixture.source.path(), &["branch", "unrelated"]);
            let unrelated = git(
                fixture.source.path(),
                &["rev-parse", "refs/heads/unrelated"],
            );
            hook.arm_after_worktree_removal();
            if !branch_removed {
                hook.fail_branch_cleanup("injected branch frontier failure");
            }
            let manager = fixture.manager.clone();
            let run = fixture.node.block.run.clone();
            let task_store = store.clone();
            let task_run = run.clone();
            let disposal = tokio::spawn(async move {
                manager
                    .dispose_workflow_workspace(&*task_store, &task_run)
                    .await
            });
            hook.wait_until_worktree_removed().await;
            assert!(!root.exists());
            assert!(
                store
                    .read_events(None, 100)
                    .unwrap()
                    .events
                    .iter()
                    .any(|e| matches!(
                        e.event,
                        RuntimeEvent::WorkflowWorkspaceDisposalStarted { .. }
                    ))
            );
            store.arm_fail_event_times(1);
            hook.release_after_worktree_removal().await;
            assert!(disposal.await.unwrap().is_err());
            let branch_exists = std::process::Command::new("git")
                .arg("-C")
                .arg(fixture.source.path())
                .args([
                    "show-ref",
                    "--verify",
                    "--quiet",
                    &format!("refs/heads/{branch}"),
                ])
                .status()
                .unwrap()
                .success();
            assert_eq!(branch_exists, !branch_removed);
            assert!(
                !store
                    .read_events(None, 100)
                    .unwrap()
                    .events
                    .iter()
                    .any(|e| matches!(
                        e.event,
                        RuntimeEvent::WorkflowWorkspaceDisposalSettled { .. }
                    ))
            );
            let retry = fixture
                .manager
                .dispose_workflow_workspace(&*store, &run)
                .await
                .unwrap();
            assert!(matches!(
                retry,
                super::super::WorkspaceDisposalSettlement::Disposed
                    | super::super::WorkspaceDisposalSettlement::AlreadyDisposed
            ));
            assert_eq!(
                fixture
                    .manager
                    .dispose_workflow_workspace(&*store, &run)
                    .await
                    .unwrap(),
                super::super::WorkspaceDisposalSettlement::AlreadyDisposed
            );
            assert_eq!(
                git(
                    fixture.source.path(),
                    &["rev-parse", "refs/heads/unrelated"]
                ),
                unrelated
            );
        }
    }

    #[tokio::test]
    async fn absent_worktree_without_durable_disposal_intent_is_not_authority() {
        let fixture = Fixture::new();
        let scope = fixture.acquire().await;
        let writer = fixture.access(&scope).await;
        let root = writer.snapshot().logical_workspace.clone();
        std::fs::write(root.join("source"), b"retained source").unwrap();
        writer.finish(false).await.unwrap();
        let terminal = scope.settle().await;
        let store = journal(&fixture, terminal, scope.final_reference().await);
        git(
            fixture.source.path(),
            &["worktree", "remove", "--force", root.to_str().unwrap()],
        );
        assert!(
            fixture
                .manager
                .dispose_workflow_workspace(&*store, &fixture.node.block.run)
                .await
                .is_err()
        );
    }
    #[tokio::test]
    async fn newly_created_directory_tree_cannot_certify_restored_source() {
        let fixture = Fixture::new();
        let scope = fixture.acquire().await;
        let access = fixture.access(&scope).await;
        let root = access.snapshot().logical_workspace.clone();
        let input = access.input().clone();
        std::fs::create_dir_all(root.join("new/nested")).unwrap();
        std::fs::write(root.join("new/nested/transient"), b"source").unwrap();
        std::fs::remove_dir_all(root.join("new")).unwrap();
        assert_eq!(
            access
                .state
                .lease
                .as_ref()
                .unwrap()
                .source_identity()
                .await
                .unwrap(),
            input.content
        );
        assert!(access.finish(true).await.is_err());
        scope.settle().await;
    }

    #[tokio::test]
    async fn directory_bound_blocks_candidate_access_before_validator_starts() {
        let fixture = Fixture::new();
        let scope = fixture.acquire().await;
        let writer = fixture.access(&scope).await;
        let mut path = writer.snapshot().logical_workspace.clone();
        for _ in 0..65 {
            path.push("d");
        }
        std::fs::create_dir_all(path).unwrap();
        writer.finish(false).await.unwrap();
        assert!(
            scope
                .borrow(fixture.node.clone(), None, &CancellationSignal::new())
                .await
                .unwrap_err()
                .contains("bound exceeded")
        );
        scope.settle().await;
    }

    #[tokio::test]
    async fn changed_source_after_disposal_intent_is_preserved_on_first_removal_and_retry() {
        let mut fixture = Fixture::new();
        let hook = Arc::new(super::super::WorkspaceDisposalHook::new());
        fixture.manager.install_disposal_hook(hook.clone());
        let scope = fixture.acquire().await;
        let writer = fixture.access(&scope).await;
        let root = writer.snapshot().logical_workspace.clone();
        std::fs::write(root.join("source"), b"retained").unwrap();
        writer.finish(false).await.unwrap();
        let terminal = scope.settle().await;
        let store = journal(&fixture, terminal, scope.final_reference().await);
        hook.arm_before_recheck();
        let manager = fixture.manager.clone();
        let task_store = store.clone();
        let run = fixture.node.block.run.clone();
        let task_run = run.clone();
        let task = tokio::spawn(async move {
            manager
                .dispose_workflow_workspace(&*task_store, &task_run)
                .await
        });
        hook.wait_until_verified().await; // durable intent exists, deletion has not begun
        std::fs::write(root.join("source"), b"later user work").unwrap();
        hook.release().await;
        assert!(task.await.unwrap().is_err());
        fixture
            .manager
            .install_disposal_hook(Arc::new(super::super::WorkspaceDisposalHook::new()));
        assert!(
            fixture
                .manager
                .dispose_workflow_workspace(&*store, &run)
                .await
                .is_err()
        );
        assert_eq!(
            std::fs::read(root.join("source")).unwrap(),
            b"later user work"
        );
    }
}
