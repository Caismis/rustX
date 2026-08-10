//! The capability coordinator: preparation, quiescent commit, and attempt
//! leases (M6).

use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Condvar, Mutex};

use crate::capabilities::error::{CapabilityCommitError, CapabilityPreparationError};
use crate::capabilities::snapshot::CapabilitySnapshot;
use crate::runtime::identity::CapabilityRevision;
use crate::runtime::process_runner::RunnerBackedProcessRunner;
use crate::skills::environments::{
    EnvironmentStore, RunnerBackedSkillEnvironmentBackend, SkillEnvironmentBackend,
};
use crate::skills::{
    SkillDiscovery, SkillSnapshot, merge_dependency_manifests,
};
use crate::tools::environment::{ToolEnvironment, ToolEnvironmentOverlay};
use crate::tools::executor::ToolRegistry;
use crate::tools::workspace::Workspace;

/// The coordinator configuration of one conversation/capability owner.
#[derive(Clone)]
pub struct CapabilityCoordinatorConfig {
    /// The canonical conversation Workspace (the Skill root anchor).
    pub workspace: Workspace,
    /// The immutable ToolRegistry handle of the capability set.
    pub tool_registry: Arc<ToolRegistry>,
    /// The base authorized ToolEnvironment (without Skill overlays).
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
    workspace: Workspace,
    tool_registry: Arc<ToolRegistry>,
    base_environment: ToolEnvironment,
    environment_store: EnvironmentStore,
    state: Mutex<CoordinatorState>,
    condvar: Condvar,
    /// Test-only commit-boundary synchronization hook.
    #[cfg(test)]
    commit_hook: Option<Arc<test_sync::CommitBoundaryHook>>,
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
pub struct PreparedCapabilityCandidate {
    base_revision: CapabilityRevision,
    skills: Arc<SkillSnapshot>,
    python: Option<crate::skills::environments::PythonEnvironment>,
    node: Option<crate::skills::environments::NodeEnvironment>,
    effective_environment: ToolEnvironment,
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
        Self::with_backend(config, Arc::new(RunnerBackedSkillEnvironmentBackend::new(
            Arc::new(RunnerBackedProcessRunner),
        )))
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
        let store_root = std::fs::canonicalize(&config.environment_store_root).unwrap_or_else(|_| {
            config.environment_store_root.clone()
        });
        if store_root.starts_with(config.workspace.root())
            || config.workspace.root().starts_with(&store_root)
        {
            return Err(CapabilityPreparationError::EnvironmentStoreOverlapsWorkspace {
                store_root: config.environment_store_root.display().to_string(),
            });
        }
        let environment_store = EnvironmentStore::new(config.environment_store_root, backend)?;
        let initial_skills = Arc::new(SkillSnapshot::new(Vec::new()));
        let initial_snapshot = Arc::new(CapabilitySnapshot::new(
            CapabilityRevision::default(),
            config.tool_registry.clone(),
            initial_skills,
            None,
            None,
            config.base_environment.clone(),
        ));
        Ok(Self {
            inner: Arc::new(CoordinatorInner {
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
                commit_hook: None,
            }),
        })
    }

    /// The current active capability snapshot.
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
        if let Some(hook) = &self.inner.commit_hook {
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
    #[must_use]
    pub fn active_attempts(&self) -> u64 {
        self.inner
            .state
            .lock()
            .expect("capability state lock poisoned")
            .active_attempts
    }
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
/// One `AgentExecution` holds exactly one lease for its complete lifetime;
/// every model/tool cycle inside that attempt uses the pinned immutable
/// snapshot and never re-discovers Skills. Conversation-owned detached
/// background executions do not hold an attempt lease.
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
