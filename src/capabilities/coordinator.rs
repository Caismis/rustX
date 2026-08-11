//! The capability coordinator: preparation, quiescent commit, and attempt
//! leases (M6).

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Condvar, Mutex};

use crate::capabilities::error::{CapabilityCommitError, CapabilityPreparationError};
use crate::capabilities::snapshot::CapabilitySnapshot;
use crate::runtime::identity::{CapabilityRevision, ConversationId};
use crate::runtime::process_runner::RunnerBackedProcessRunner;
use crate::skills::environments::{
    EnvironmentStore, RunnerBackedSkillEnvironmentBackend, SkillEnvironmentBackend,
};
use crate::skills::{SkillDiscovery, SkillSnapshot, merge_dependency_manifests};
use crate::tools::environment::{ToolEnvironment, ToolEnvironmentOverlay};
use crate::tools::executor::ToolRegistry;
use crate::tools::workspace::Workspace;

/// The coordinator configuration of one conversation/capability owner.
#[derive(Clone)]
pub struct CapabilityCoordinatorConfig {
    /// The conversation that owns this coordinator and every lease it emits.
    pub conversation_id: ConversationId,
    /// The canonical conversation Workspace (the Skill root anchor).
    pub workspace: Workspace,
    /// The immutable `ToolRegistry` handle of the capability set.
    pub tool_registry: Arc<ToolRegistry>,
    /// The base authorized `ToolEnvironment` (without Skill overlays).
    pub base_environment: ToolEnvironment,
    /// The caller-configured runtime-private environment store root,
    /// disjoint from the Workspace.
    pub environment_store_root: PathBuf,
}

/// The synchronized coordinator state.
struct CoordinatorState {
    revision: CapabilityRevision,
    snapshot: Arc<CapabilitySnapshot>,
    /// The number of active attempt capability leases.
    active_attempts: u64,
    /// The next environment staging sequence (never enters any digest).
    _next_staging: AtomicU64,
}

/// The conversation/capability-owner coordination state.
struct CoordinatorInner {
    conversation_id: ConversationId,
    workspace: Workspace,
    tool_registry: Arc<ToolRegistry>,
    base_environment: ToolEnvironment,
    environment_store: EnvironmentStore,
    state: Mutex<CoordinatorState>,
    condvar: Condvar,
    /// Test-only commit-boundary synchronization hook.
    #[cfg(test)]
    commit_hook: Mutex<Option<Arc<test_sync::CommitBoundaryHook>>>,
}

/// The capability coordinator of one conversation/capability owner.
///
/// # Flow
///
/// ```text
/// prepare_candidate()  →  commit(candidate)  →  acquire_attempt_lease()
/// ```
///
/// A candidate may be prepared independently (including environment
/// materialization); only its activation respects the commit boundary.
/// Commit succeeds only when no attempt lease is active and the candidate
/// is not stale; an identical rediscovery/preparation is a no-op that does
/// not fabricate a new revision. Failed preparation/commit leaves the
/// current active revision authoritative.
#[derive(Clone)]
pub struct CapabilityCoordinator {
    inner: Arc<CoordinatorInner>,
}

/// A prepared but not yet committed candidate capability.
///
/// The candidate carries the base revision it was prepared from; commit
/// rejects it as stale when the active revision has advanced.
#[derive(Debug)]
pub struct PreparedCapabilityCandidate {
    base_revision: CapabilityRevision,
    skills: Arc<SkillSnapshot>,
    python: Option<crate::skills::environments::PythonEnvironment>,
    node: Option<crate::skills::environments::NodeEnvironment>,
    effective_environment: ToolEnvironment,
}

impl PreparedCapabilityCandidate {
    /// The discovered and validated Skill packages of the candidate.
    #[must_use]
    pub fn skill_packages(&self) -> &[Arc<crate::skills::SkillPackage>] {
        self.skills.packages()
    }

    /// The prepared immutable Python environment identity, when present.
    #[must_use]
    pub fn python_environment(&self) -> Option<&crate::skills::environments::PythonEnvironment> {
        self.python.as_ref()
    }

    /// The prepared immutable Node environment identity, when present.
    #[must_use]
    pub fn node_environment(&self) -> Option<&crate::skills::environments::NodeEnvironment> {
        self.node.as_ref()
    }
}

impl CapabilityCoordinator {
    /// Creates the coordinator with an initial empty capability set at
    /// revision zero.
    ///
    /// The environment store root must be disjoint from the model
    /// Workspace (neither may nest inside or alias the other).
    ///
    /// # Errors
    ///
    /// Returns [`CapabilityPreparationError`] when the environment store
    /// root overlaps the Workspace or cannot be established.
    pub fn new(config: CapabilityCoordinatorConfig) -> Result<Self, CapabilityPreparationError> {
        Self::with_backend(
            config,
            Arc::new(RunnerBackedSkillEnvironmentBackend::new(Arc::new(
                RunnerBackedProcessRunner::default(),
            ))),
        )
    }

    /// The coordinator constructor with an explicit materialization
    /// backend (deterministic testing seam).
    ///
    /// # Errors
    ///
    /// See [`CapabilityCoordinator::new`].
    pub fn with_backend(
        config: CapabilityCoordinatorConfig,
        backend: Arc<dyn SkillEnvironmentBackend>,
    ) -> Result<Self, CapabilityPreparationError> {
        let prospective_store_root = prospective_canonical_path(&config.environment_store_root)
            .map_err(|detail| {
                CapabilityPreparationError::Environment(
                    crate::skills::EnvironmentPreparationError::StagingFailed { detail },
                )
            })?;
        if paths_overlap(config.workspace.root(), &prospective_store_root) {
            return Err(
                CapabilityPreparationError::EnvironmentStoreOverlapsWorkspace {
                    store_root: config.environment_store_root.display().to_string(),
                },
            );
        }
        // Create through the prospective canonical target, not through the
        // unresolved configured spelling. This prevents a symlink-prefix
        // path from redirecting creation into the model Workspace after the
        // overlap check has completed.
        let environment_store = EnvironmentStore::new(prospective_store_root, backend)?;
        if paths_overlap(config.workspace.root(), environment_store.root()) {
            return Err(
                CapabilityPreparationError::EnvironmentStoreOverlapsWorkspace {
                    store_root: environment_store.root().display().to_string(),
                },
            );
        }
        let initial_skills = Arc::new(SkillSnapshot::new(Vec::new()));
        let initial_snapshot = Arc::new(CapabilitySnapshot::new(
            config.conversation_id.clone(),
            config.workspace.root().to_path_buf(),
            CapabilityRevision::default(),
            config.tool_registry.clone(),
            initial_skills,
            None,
            None,
            config.base_environment.clone(),
        ));
        Ok(Self {
            inner: Arc::new(CoordinatorInner {
                conversation_id: config.conversation_id,
                workspace: config.workspace,
                tool_registry: config.tool_registry,
                base_environment: config.base_environment,
                environment_store,
                state: Mutex::new(CoordinatorState {
                    revision: CapabilityRevision::default(),
                    snapshot: initial_snapshot,
                    active_attempts: 0,
                    _next_staging: AtomicU64::new(0),
                }),
                condvar: Condvar::new(),
                #[cfg(test)]
                commit_hook: Mutex::new(None),
            }),
        })
    }

    /// The current active capability snapshot.
    ///
    /// # Panics
    ///
    /// Panics only if the capability state lock is poisoned, which would
    /// mean a previous operation panicked while holding the lock.
    #[must_use]
    pub fn current_snapshot(&self) -> Arc<CapabilitySnapshot> {
        self.inner
            .state
            .lock()
            .expect("capability state lock poisoned")
            .snapshot
            .clone()
    }

    /// Prepares one candidate capability from the current active revision.
    ///
    /// Discovery, dependency merge, and environment materialization run
    /// independently of the commit boundary. Any malformed Skill fails the
    /// whole candidate; a dependency conflict is detected before any
    /// package-manager subprocess; a materialization failure leaves the
    /// active capability unchanged.
    ///
    /// # Errors
    ///
    /// Returns [`CapabilityPreparationError`] for discovery, conflict,
    /// environment, or store failures.
    ///
    /// # Panics
    ///
    /// Panics only if the capability state lock is poisoned, which would
    /// mean a previous operation panicked while holding the lock.
    pub async fn prepare_candidate(
        &self,
    ) -> Result<PreparedCapabilityCandidate, CapabilityPreparationError> {
        let base_revision = self
            .inner
            .state
            .lock()
            .expect("capability state lock poisoned")
            .revision;
        let packages = SkillDiscovery::new(&self.inner.workspace).discover()?;
        let merged = merge_dependency_manifests(&packages)?;
        let python = self
            .inner
            .environment_store
            .ensure_python_environment(&merged.python)
            .await?;
        let node = self
            .inner
            .environment_store
            .ensure_node_environment(&merged.node)
            .await?;
        let mut overlay = ToolEnvironmentOverlay::default();
        if let Some(python) = &python {
            overlay = overlay.merge(ToolEnvironmentOverlay::python(&python.root));
        }
        if let Some(node) = &node {
            overlay = overlay.merge(ToolEnvironmentOverlay::node(&node.root));
        }
        let effective_environment = self.inner.base_environment.with_overlay(&overlay);
        let skills = Arc::new(SkillSnapshot::new(
            packages.into_iter().map(Arc::new).collect(),
        ));
        Ok(PreparedCapabilityCandidate {
            base_revision,
            skills,
            python,
            node,
            effective_environment,
        })
    }

    /// Activates the prepared candidate (the quiescent atomic commit).
    ///
    /// Both the quiescence decision (zero active attempt leases) and the
    /// active-snapshot swap happen inside the one synchronization boundary,
    /// so there is no unchecked window between the deciding observation and
    /// the swap. A candidate prepared from an obsolete base revision is
    /// rejected as stale; an identical candidate is a no-op that returns
    /// the current snapshot without fabricating a new revision.
    ///
    /// # Errors
    ///
    /// Returns [`CapabilityCommitError::Busy`] while an attempt lease is
    /// active and [`CapabilityCommitError::StaleCandidate`] for an obsolete
    /// base revision.
    ///
    /// # Panics
    ///
    /// Panics only if the capability state lock is poisoned, which would
    /// mean a previous operation panicked while holding the lock.
    pub fn commit(
        &self,
        candidate: PreparedCapabilityCandidate,
    ) -> Result<Arc<CapabilitySnapshot>, CapabilityCommitError> {
        let mut state = self
            .inner
            .state
            .lock()
            .expect("capability state lock poisoned");
        // TEST-ONLY commit-boundary hook: the lock is held and the deciding
        // quiescence observation is next.
        #[cfg(test)]
        if let Some(hook) = self
            .inner
            .commit_hook
            .lock()
            .expect("commit hook lock")
            .clone()
        {
            hook.enter();
        }
        if candidate.base_revision != state.revision {
            return Err(CapabilityCommitError::StaleCandidate {
                prepared_from: candidate.base_revision,
                current: state.revision,
            });
        }
        if state.active_attempts > 0 {
            return Err(CapabilityCommitError::Busy);
        }
        if candidate_is_noop(&candidate, &state.snapshot) {
            return Ok(state.snapshot.clone());
        }
        let revision = CapabilityRevision::new(state.revision.get() + 1);
        let snapshot = Arc::new(CapabilitySnapshot::new(
            self.inner.conversation_id.clone(),
            self.inner.workspace.root().to_path_buf(),
            revision,
            self.inner.tool_registry.clone(),
            candidate.skills,
            candidate.python,
            candidate.node,
            candidate.effective_environment,
        ));
        state.revision = revision;
        state.snapshot = snapshot.clone();
        self.inner.condvar.notify_all();
        Ok(snapshot)
    }

    /// Acquires one attempt capability lease (RAII).
    ///
    /// The lease pins the current active immutable snapshot for the
    /// complete lifetime of one `AgentExecution`; dropping the lease
    /// releases it. The acquisition and the commit share the same
    /// synchronization boundary, so a commit that wins first is observed by
    /// the next acquisition and an acquisition that wins first makes the
    /// next commit observe busy.
    ///
    /// # Panics
    ///
    /// Panics only if the capability state lock is poisoned, which would
    /// mean a previous operation panicked while holding the lock.
    #[must_use]
    pub fn acquire_attempt_lease(&self) -> AttemptCapabilityLease {
        let mut state = self
            .inner
            .state
            .lock()
            .expect("capability state lock poisoned");
        state.active_attempts += 1;
        let snapshot = state.snapshot.clone();
        AttemptCapabilityLease {
            inner: self.inner.clone(),
            snapshot,
        }
    }

    /// The number of active attempt leases (test/observability).
    ///
    /// # Panics
    ///
    /// Panics only if the capability state lock is poisoned, which would
    /// mean a previous operation panicked while holding the lock.
    #[must_use]
    pub fn active_attempts(&self) -> u64 {
        self.inner
            .state
            .lock()
            .expect("capability state lock poisoned")
            .active_attempts
    }

    /// Installs the test-only synchronization hook at the commit boundary.
    /// Only available under `#[cfg(test)]`; never used by production code.
    #[cfg(test)]
    pub(crate) fn install_commit_boundary_hook(&self, hook: Arc<test_sync::CommitBoundaryHook>) {
        *self.inner.commit_hook.lock().expect("commit hook lock") = Some(hook);
    }
}

/// Returns the canonical target that creation would reach without creating
/// any missing component. Existing ancestors are canonicalized (therefore
/// symlinks are resolved), and the non-existent suffix is appended only after
/// that resolution.
fn prospective_canonical_path(path: &Path) -> Result<PathBuf, String> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| format!("cannot resolve environment store path: {error}"))?
            .join(path)
    };
    let normalized = normalize_absolute_path(&absolute)?;
    let mut existing = normalized.clone();
    let mut suffix = Vec::<OsString>::new();
    while !existing.exists() {
        let Some(name) = existing.file_name() else {
            return Err(format!(
                "cannot resolve environment store path {}",
                path.display()
            ));
        };
        suffix.push(name.to_os_string());
        if !existing.pop() {
            return Err(format!(
                "cannot resolve environment store path {}",
                path.display()
            ));
        }
    }
    let canonical_existing = std::fs::canonicalize(&existing)
        .map_err(|error| format!("cannot canonicalize environment store ancestor: {error}"))?;
    let mut result = canonical_existing;
    for component in suffix.iter().rev() {
        result.push(component);
    }
    Ok(result)
}

/// Normalizes an absolute path before resolving its deepest existing
/// ancestor. Parent components are rejected because resolving them lexically
/// before following symlinks could describe a different filesystem target
/// from the path the OS will create.
fn normalize_absolute_path(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err(format!(
            "environment store path is not absolute: {}",
            path.display()
        ));
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(Path::new(std::path::MAIN_SEPARATOR_STR)),
            Component::CurDir => {}
            Component::Normal(value) => normalized.push(value),
            Component::ParentDir => {
                return Err(format!(
                    "environment store path contains an ambiguous parent component: {}",
                    path.display()
                ));
            }
        }
    }
    Ok(normalized)
}

fn paths_overlap(left: &Path, right: &Path) -> bool {
    left == right || left.starts_with(right) || right.starts_with(left)
}

/// Whether the candidate's capability content equals the current snapshot
/// (an identical rediscovery/preparation is a no-op that must not fabricate
/// a new revision).
fn candidate_is_noop(
    candidate: &PreparedCapabilityCandidate,
    current: &CapabilitySnapshot,
) -> bool {
    candidate.skills.bindings() == current.skills().bindings()
        && candidate.python.as_ref().map(|env| env.digest.clone())
            == current.python_environment().map(|env| env.digest.clone())
        && candidate.node.as_ref().map(|env| env.digest.clone())
            == current.node_environment().map(|env| env.digest.clone())
}

/// The RAII-style attempt capability lease.
///
/// One `AgentExecution` owns exactly one lease for its complete lifetime;
/// every model/tool cycle inside that attempt uses the pinned immutable
/// snapshot and never re-discovers Skills. The lease is acquired before
/// construction, moved into the execution, and released by normal
/// destruction when construction fails or the consumed execution settles.
/// Conversation-owned detached background executions do not hold an attempt
/// lease.
pub struct AttemptCapabilityLease {
    inner: Arc<CoordinatorInner>,
    snapshot: Arc<CapabilitySnapshot>,
}

impl AttemptCapabilityLease {
    /// The pinned immutable capability snapshot of this attempt.
    #[must_use]
    pub fn snapshot(&self) -> &Arc<CapabilitySnapshot> {
        &self.snapshot
    }

    /// The monotonic capability revision pinned by this attempt.
    #[must_use]
    pub fn revision(&self) -> CapabilityRevision {
        self.snapshot.revision()
    }
}

impl Drop for AttemptCapabilityLease {
    fn drop(&mut self) {
        let mut state = self
            .inner
            .state
            .lock()
            .expect("capability state lock poisoned");
        state.active_attempts = state.active_attempts.saturating_sub(1);
        self.inner.condvar.notify_all();
    }
}

/// Test-only synchronization for the commit boundary.
///
/// [`CommitBoundaryHook::enter`] is called by `commit` while the
/// coordinator lock is held, immediately before the deciding quiescence
/// observation. It signals `entered` and parks the calling thread until
/// the test calls `proceed`, so a test can prove the exact linearization:
/// an attempt lease held after `entered` but before `proceed` is
/// necessarily observed at the protected boundary and rejects the commit
/// as busy; a commit released without an active lease is never reclaimable
/// by a later acquisition.
///
/// All synchronization is `std` (mutex + condvar) because the commit
/// boundary is a `std` mutex critical section; the parking blocks the OS
/// thread, so the race tests run on a multi-threaded runtime.
#[cfg(test)]
pub(crate) mod test_sync {
    use std::sync::{Condvar, Mutex};

    /// The two-phase gate of the commit boundary.
    #[derive(Debug, Default)]
    pub(crate) struct CommitBoundaryHook {
        state: Mutex<HookState>,
        condvar: Condvar,
    }

    #[derive(Debug, Default)]
    struct HookState {
        entered: bool,
        proceed: bool,
    }

    impl CommitBoundaryHook {
        /// Signals that the commit boundary was entered (the coordinator
        /// lock is held and the deciding quiescence observation is next),
        /// then blocks until [`CommitBoundaryHook::proceed`].
        pub(crate) fn enter(&self) {
            let mut state = self.state.lock().expect("commit hook lock poisoned");
            state.entered = true;
            self.condvar.notify_all();
            while !state.proceed {
                state = self.condvar.wait(state).expect("commit hook wait poisoned");
            }
        }

        /// Blocks until the commit boundary was entered.
        pub(crate) fn wait_entered(&self) {
            let mut state = self.state.lock().expect("commit hook lock poisoned");
            while !state.entered {
                state = self.condvar.wait(state).expect("commit hook wait poisoned");
            }
        }

        /// Releases a parked commit boundary.
        pub(crate) fn proceed(&self) {
            let mut state = self.state.lock().expect("commit hook lock poisoned");
            state.proceed = true;
            self.condvar.notify_all();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::test_sync::CommitBoundaryHook;
    use super::{CapabilityCoordinator, CapabilityCoordinatorConfig};
    use crate::capabilities::CapabilityCommitError;
    use crate::runtime::identity::CapabilityRevision;
    use crate::tools::environment::ToolEnvironment;
    use crate::tools::executor::ToolRegistry;
    use crate::tools::workspace::Workspace;

    /// A minimal coordinator with one active Skill candidate so commits
    /// are real semantic changes (an empty candidate is a no-op).
    fn coordinator() -> (tempfile::TempDir, CapabilityCoordinator) {
        let dir = tempfile::tempdir().expect("temp dir");
        let workspace_root = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace_root).expect("workspace");
        let skill_dir = workspace_root.join(".agents/skills/pdf");
        std::fs::create_dir_all(&skill_dir).expect("skill dir");
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---
name: pdf
description: PDF skill.
---
body
",
        )
        .expect("SKILL.md");
        let workspace = Workspace::new(&workspace_root).expect("workspace");
        let coordinator = CapabilityCoordinator::new(CapabilityCoordinatorConfig {
            conversation_id: crate::runtime::identity::ConversationId::new("conv-test"),
            workspace,
            tool_registry: Arc::new(ToolRegistry::new()),
            base_environment: ToolEnvironment::new(),
            environment_store_root: dir.path().join("skill-env"),
        })
        .expect("coordinator");
        (dir, coordinator)
    }

    async fn prepare(coordinator: &CapabilityCoordinator) -> super::PreparedCapabilityCandidate {
        coordinator.prepare_candidate().await.expect("prepare")
    }

    /// Attempt acquisition wins first: the commit is parked inside its
    /// critical section (the deciding quiescence observation is next),
    /// the attempt lease is held, and the released commit must observe
    /// busy and cannot activate a new revision. The commit-boundary hook
    /// proves the linearization without any timing assumption.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn attempt_acquisition_wins_commit_observes_busy() {
        let (_dir, coordinator) = coordinator();
        let hook = Arc::new(CommitBoundaryHook::default());
        coordinator.install_commit_boundary_hook(hook.clone());
        let lease = coordinator.acquire_attempt_lease();
        let revision = lease.revision();
        let candidate = prepare(&coordinator).await;

        let coordinator_for_task = coordinator.clone();
        let commit_task = std::thread::spawn(move || coordinator_for_task.commit(candidate));
        hook.wait_entered();
        // The commit is parked before the deciding quiescence observation;
        // the attempt lease is still held (the coordinator lock is
        // unavailable while the commit parks, so the held lease itself is
        // the proof).
        hook.proceed();
        let result = commit_task.join().expect("commit task");
        assert_eq!(result, Err(CapabilityCommitError::Busy));
        assert_eq!(
            coordinator.current_snapshot().revision(),
            revision,
            "a busy commit cannot activate a new revision"
        );
        drop(lease);
    }

    /// Commit wins first: the commit is parked inside its critical section
    /// with no attempt lease active, released, and the next attempt
    /// acquisition snapshots the new revision. The hook proves the
    /// ordering.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn commit_wins_next_attempt_observes_the_new_revision() {
        let (_dir, coordinator) = coordinator();
        let hook = Arc::new(CommitBoundaryHook::default());
        coordinator.install_commit_boundary_hook(hook.clone());
        let candidate = prepare(&coordinator).await;

        let coordinator_for_task = coordinator.clone();
        let commit_task = std::thread::spawn(move || coordinator_for_task.commit(candidate));
        hook.wait_entered();
        // No attempt lease is active at the boundary: the commit was never
        // observed as busy, which is the linearization proof.
        hook.proceed();
        let snapshot = commit_task.join().expect("commit task").expect("commit");
        assert_eq!(
            snapshot.revision(),
            CapabilityRevision::new(1),
            "the first semantic change activates revision 1"
        );
        let lease = coordinator.acquire_attempt_lease();
        assert_eq!(lease.revision(), snapshot.revision());
        assert_eq!(lease.snapshot().as_ref(), &*snapshot);
        drop(lease);
    }

    /// A second commit while the first is parked at the boundary serializes
    /// through the same boundary: after the first commit wins, the second
    /// candidate (prepared from the obsolete base) is stale.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_commits_serialize_at_one_boundary() {
        let (_dir, coordinator) = coordinator();
        let hook = Arc::new(CommitBoundaryHook::default());
        coordinator.install_commit_boundary_hook(hook.clone());
        let first = prepare(&coordinator).await;
        let second = prepare(&coordinator).await;

        let coordinator_for_task = coordinator.clone();
        let first_task = std::thread::spawn(move || coordinator_for_task.commit(first));
        hook.wait_entered();
        // The second commit queues on the same mutex; release the first.
        hook.proceed();
        first_task.join().expect("first commit").expect("commit");
        let result = coordinator.commit(second);
        assert!(matches!(
            result,
            Err(CapabilityCommitError::StaleCandidate { .. })
        ));
    }

    /// The lease RAII release makes the next commit legal immediately; no
    /// waiting or sleeping is involved.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn lease_release_enables_the_next_commit_immediately() {
        let (_dir, coordinator) = coordinator();
        let lease = coordinator.acquire_attempt_lease();
        let candidate = prepare(&coordinator).await;
        assert_eq!(
            coordinator.commit(candidate),
            Err(CapabilityCommitError::Busy)
        );
        drop(lease);
        assert_eq!(coordinator.active_attempts(), 0);
        let candidate = prepare(&coordinator).await;
        assert_eq!(
            coordinator.commit(candidate).expect("commit").revision(),
            CapabilityRevision::new(1)
        );
    }
}
