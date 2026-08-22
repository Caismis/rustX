//! The capability coordinator: preparation, quiescent commit, and attempt
//! leases (M6).

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use crate::capabilities::availability::{
    CapabilityAvailability, CapabilitySourceId, CapabilitySourceState,
};
use crate::capabilities::error::{CapabilityCommitError, CapabilityPreparationError};
use crate::capabilities::snapshot::CapabilitySnapshot;
use crate::capabilities::tools::{AvailableToolCatalog, ToolActivationPolicy, select_tools};
use crate::runtime::identity::{CapabilityRevision, ConversationId, McpServerId};
use crate::runtime::process_runner::RunnerBackedProcessRunner;
use crate::runtime::types::ConversationLifecycle;
use crate::skills::environments::{
    EnvironmentStore, RunnerBackedSkillEnvironmentBackend, SkillEnvironmentBackend,
};
use crate::skills::{
    SkillDiscovery, SkillDiscoveryConfig, SkillSnapshot, merge_dependency_manifests,
};
use crate::tools::environment::{ToolEnvironment, ToolEnvironmentOverlay};
use crate::tools::executor::{ToolRegistration, ToolRegistry};
use crate::tools::mcp::{McpInvalidationState, McpServerBindings, McpServerRuntime};
use crate::tools::python::{PythonToolDiscovery, PythonToolExecutor, PythonToolStore};
use crate::tools::workspace::Workspace;

/// The coordinator configuration of one conversation/capability owner.
#[derive(Clone)]
pub struct CapabilityCoordinatorConfig {
    /// The conversation that owns this coordinator and every lease it emits.
    pub conversation_id: ConversationId,
    /// The canonical conversation Workspace (the Skill root anchor).
    pub workspace: Workspace,
    /// The deterministic native/runtime registry used as the composition base.
    pub base_tool_registry: Arc<ToolRegistry>,
    /// Current startup availability/activation policy. It is not durable
    /// Session state and is re-applied for every process composition.
    pub tool_activation: ToolActivationPolicy,
    /// Current automatic and explicit Skill resource roots.
    pub skill_discovery: SkillDiscoveryConfig,
    /// The immutable configured MCP server set for this coordinator, keyed
    /// by the one authoritative server identity.
    pub mcp_servers: McpServerBindings,
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
    /// The authoritative availability of every evaluated optional
    /// capability source (Issue #81). Mutated only at the commit
    /// linearization point; projected outward read-only. Never part of the
    /// execution identity: it neither enters the `CapabilitiesManifest`
    /// nor advances `CapabilityRevision`.
    availability: CapabilityAvailability,
    /// The number of active attempt capability leases.
    active_attempts: u64,
    /// The next environment staging sequence (never enters any digest).
    _next_staging: AtomicU64,
    /// The shared activation lifecycle of the claiming conversation
    /// runtime (Issue #61).
    ///
    /// Set by [`CapabilityCoordinator::claim_conversation_runtime`] and
    /// read by `commit`, both under this state lock — so it can never
    /// disagree with the `coordinator_claimed` atomic. This is **not**
    /// another activation authority: the capability keeps no activation
    /// state of its own, and active/inactive is answered by the lifecycle
    /// itself, the same handle the mailbox, the background registry, and
    /// the coordinator observe. `None` = standalone/unclaimed, which
    /// commits unconditionally.
    conversation_lifecycle: Option<ConversationLifecycle>,
}

/// The conversation/capability-owner coordination state.
struct CoordinatorInner {
    conversation_id: ConversationId,
    workspace: Workspace,
    base_tool_registry: Arc<ToolRegistry>,
    tool_activation: ToolActivationPolicy,
    skill_discovery: SkillDiscoveryConfig,
    mcp_servers: McpServerBindings,
    mcp_runtimes: tokio::sync::Mutex<BTreeMap<McpServerId, Arc<McpServerRuntime>>>,
    /// The ownership cancellation root of every in-flight conversation-owned
    /// MCP connection (Issue #12, M9c).
    ///
    /// Each in-flight connect owner takes a child of this signal, so runtime
    /// drain can close in-flight *preparation* the same way it closes
    /// retained runtimes: by cancelling the owner, never by dropping a
    /// caller future. Cancelling drives an already-spawned stdio process to
    /// its physical settlement proof before the owner releases its counted
    /// lifecycle admission.
    mcp_preparation_cancellation: crate::runtime::cancellation::CancellationSignal,
    /// Test-only: parks the next conversation-owned MCP connect at the
    /// instant physical process ownership exists.
    #[cfg(test)]
    connect_ownership_pause:
        Mutex<Option<Arc<crate::tools::mcp::test_sync::ConnectOwnershipPause>>>,
    /// The one shared MCP invalidation synchronization boundary: epoch
    /// mutation (`tools/list_changed`) and epoch validation + snapshot swap
    /// (commit) serialize through the same guard.
    mcp_invalidation: Arc<McpInvalidationState>,
    /// The configured location of the Python tool store
    /// (`<environment store>/m7-tools`).
    ///
    /// The coordinator owns the *location* only. Opening/creating the
    /// Python-specific storage is part of the optional Python capability
    /// preparation ([`CapabilityCoordinator::prepare_python_tools`]), so a
    /// Python storage failure degrades Python availability and can never
    /// fail core coordinator construction — and a base-only/subagent
    /// coordinator never touches Python storage at all.
    python_store_root: PathBuf,
    /// The lazily initialized, coordinator-lifetime-stable Python tool
    /// store (Issue #81).
    ///
    /// Initialization timing is not lifetime ownership: the slot starts
    /// empty because Python is optional and its storage may fail, a failed
    /// initialization leaves the slot empty so the next preparation retries,
    /// and the first *successful* initialization publishes the one stable
    /// process-local store identity every later Python preparation — and
    /// every executor derived from it — reuses. The store owns the
    /// process-local coordination domains (in-flight environment build
    /// coalescing, invocation bundle allocation), so it must never be
    /// reconstructed per preparation: that would silently restart the
    /// invocation identifier allocation and let two executor generations
    /// claim the same execution bundle.
    ///
    /// The mutex is held only across the synchronous store construction
    /// (a bounded `create_dir_all` sequence) — never across `.await`.
    python_store: Mutex<Option<PythonToolStore>>,
    base_environment: ToolEnvironment,
    environment_store: EnvironmentStore,
    state: Mutex<CoordinatorState>,
    condvar: Condvar,
    /// The read-only state observer, installed by the owning runtime client
    /// boundary (Issue #37). It fires while the coordinator lock is held.
    observer: Mutex<Option<Arc<dyn CapabilityObserver>>>,
    /// The one-time Runtime Client binding of this coordinator identity.
    ///
    /// The coordinator carries one observer slot, so binding it twice would
    /// silently replace the first host's capability projection. Like the
    /// `ConversationToolRuntime` binding this is claimed once and never
    /// released, and every clone shares it.
    runtime_client_bound: AtomicBool,
    /// Claimed by the one conversation runtime coordinator of this
    /// identity.
    coordinator_claimed: AtomicBool,
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
/// A candidate may be prepared independently (including shared environment
/// materialization); once this coordinator is claimed by a conversation
/// runtime, preparation itself is counted as runtime-owned work so drain
/// waits for its owner to settle. Only its activation respects the commit
/// boundary. Commit succeeds only when no attempt lease is active and the
/// candidate is not stale; an identical rediscovery/preparation is a no-op
/// that does not fabricate a new revision. Failed preparation/commit leaves
/// the current active revision authoritative.
#[derive(Clone)]
pub struct CapabilityCoordinator {
    inner: Arc<CoordinatorInner>,
}

/// The read-only observation seam of the capability coordinator.
///
/// A state observer receives the authoritative active snapshot after every
/// actual capability activation (a revision swap), together with the
/// authoritative per-source availability state, and again whenever a commit
/// changes availability without changing the executable set (the revision
/// does not advance for a diagnostic-only change). A commit that changes
/// neither never fires the observer. The callback fires while the
/// coordinator synchronization boundary is held, so the observed order is
/// exactly the commit linearization order. An observer must never call
/// back into the coordinator; the Runtime Client projection (Issue #37)
/// treats each callback as one projection fold under its own
/// synchronization boundary.
pub trait CapabilityObserver: Send + Sync {
    /// Observes one activated immutable capability snapshot and its
    /// authoritative availability state.
    fn on_snapshot(&self, snapshot: &CapabilitySnapshot, availability: &CapabilityAvailability);
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
    candidate_registry: Arc<ToolRegistry>,
    available_tools: Arc<AvailableToolCatalog>,
    mcp_epochs: BTreeMap<McpServerId, u64>,
    /// The per-source availability outcome of this preparation (Issue
    /// #81). Only sources whose state is [`CapabilitySourceState::Ready`]
    /// contributed executors to `candidate_registry`.
    availability: CapabilityAvailability,
}

impl PreparedCapabilityCandidate {
    /// The prepared availability outcome of this candidate.
    #[must_use]
    pub fn availability(&self) -> &CapabilityAvailability {
        &self.availability
    }

    /// The complete available Tool catalog, including inactive Tools.
    #[must_use]
    pub fn available_tools(&self) -> &AvailableToolCatalog {
        &self.available_tools
    }

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
        // Uniqueness and ordering are structural: `McpServerBindings` is a
        // `BTreeMap` keyed by identity. Only emptiness stays checkable.
        if config
            .mcp_servers
            .keys()
            .any(|server_id| server_id.as_str().is_empty())
        {
            return Err(CapabilityPreparationError::Mcp(
                "MCP server ids must be non-empty".to_owned(),
            ));
        }
        let mcp_servers = config.mcp_servers;
        let tool_activation = config.tool_activation;
        let skill_discovery = config.skill_discovery;
        // Only the Python store *location* is computed here; the store
        // itself is opened inside the optional Python preparation
        // boundary (Issue #81), never in core construction.
        let python_store_root = environment_store.root().join("m7-tools");
        let initial_skills = Arc::new(SkillSnapshot::new(Vec::new()));
        let initial_snapshot = Arc::new(CapabilitySnapshot::new(
            config.conversation_id.clone(),
            config.workspace.root().to_path_buf(),
            CapabilityRevision::default(),
            config.base_tool_registry.clone(),
            Arc::new(AvailableToolCatalog::new(
                config.base_tool_registry.definitions(),
            )),
            initial_skills,
            None,
            None,
            config.base_environment.clone(),
        ));
        Ok(Self {
            inner: Arc::new(CoordinatorInner {
                conversation_id: config.conversation_id,
                workspace: config.workspace,
                base_tool_registry: config.base_tool_registry,
                tool_activation,
                skill_discovery,
                mcp_servers,
                mcp_runtimes: tokio::sync::Mutex::new(BTreeMap::new()),
                mcp_preparation_cancellation: crate::runtime::cancellation::CancellationSignal::new(
                ),
                #[cfg(test)]
                connect_ownership_pause: Mutex::new(None),
                mcp_invalidation: Arc::new(McpInvalidationState::new()),
                python_store_root,
                python_store: Mutex::new(None),
                base_environment: config.base_environment,
                environment_store,
                state: Mutex::new(CoordinatorState {
                    revision: CapabilityRevision::default(),
                    snapshot: initial_snapshot,
                    availability: CapabilityAvailability::new(),
                    active_attempts: 0,
                    _next_staging: AtomicU64::new(0),
                    conversation_lifecycle: None,
                }),
                condvar: Condvar::new(),
                observer: Mutex::new(None),
                runtime_client_bound: AtomicBool::new(false),
                coordinator_claimed: AtomicBool::new(false),
                #[cfg(test)]
                commit_hook: Mutex::new(None),
            }),
        })
    }

    /// Claims the one-time Runtime Client binding of this coordinator
    /// identity.
    ///
    /// Returns `true` for the one claim that wins and `false` for every
    /// later claim on any clone. Never reset by dropping the bound host.
    pub(crate) fn claim_runtime_client(&self) -> bool {
        self.inner
            .runtime_client_bound
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    /// Releases a Runtime Client binding claimed by a host construction
    /// that then failed.
    ///
    /// This exists only so a rejected `RuntimeClientHost::new` (whose
    /// observation bridge install failed after the claim) leaves no trace;
    /// it is never called on host drop, and a successfully constructed
    /// host never releases its binding.
    pub(crate) fn release_runtime_client_claim(&self) {
        self.inner
            .runtime_client_bound
            .store(false, Ordering::Release);
    }

    /// Whether this coordinator identity is already bound to a Runtime
    /// Client host.
    #[must_use]
    pub fn is_runtime_client_bound(&self) -> bool {
        self.inner.runtime_client_bound.load(Ordering::Acquire)
    }

    /// Claims the one-time conversation-runtime-coordinator binding of this
    /// coordinator identity, together with the claiming runtime's shared
    /// activation lifecycle.
    ///
    /// Returns `true` for the one claim that wins and `false` for every
    /// later claim on any clone. Never reset by dropping the bound
    /// coordinator.
    ///
    /// The claim and the lifecycle attachment share the capability state
    /// lock — the same boundary `commit` reads them under — so a
    /// runtime-owned `commit` can never observe a claimed coordinator
    /// without its lifecycle. A standalone (unclaimed) coordinator keeps no
    /// lifecycle and commits unconditionally.
    pub(crate) fn claim_conversation_runtime(&self, lifecycle: &ConversationLifecycle) -> bool {
        let mut state = self
            .inner
            .state
            .lock()
            .expect("capability state lock poisoned");
        if self
            .inner
            .coordinator_claimed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return false;
        }
        state.conversation_lifecycle = Some(lifecycle.clone());
        drop(state);
        true
    }

    /// Reverts a conversation-runtime claim whose construction then failed.
    ///
    /// This exists only so a rejected `ConversationRuntime::new` leaves no
    /// trace: the claim and the attached lifecycle are released together,
    /// under the same capability state lock that took them, so the
    /// coordinator returns to its exact previous standalone state. It is
    /// never called on drop, and a successfully constructed runtime never
    /// releases its binding.
    pub(crate) fn release_conversation_runtime_claim(&self) {
        let mut state = self
            .inner
            .state
            .lock()
            .expect("capability state lock poisoned");
        state.conversation_lifecycle = None;
        self.inner
            .coordinator_claimed
            .store(false, Ordering::Release);
        drop(state);
    }

    /// Whether this coordinator identity is already bound to a conversation
    /// runtime coordinator.
    #[must_use]
    pub fn is_conversation_runtime_bound(&self) -> bool {
        self.inner.coordinator_claimed.load(Ordering::Acquire)
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
    /// # Optional-source isolation (Issue #81)
    ///
    /// The custom Python tool plane and each configured MCP server are
    /// **optional** capability sources: one's failure is recorded as its
    /// typed [`CapabilitySourceState::Unavailable`] state and preparation
    /// continues with the remaining sources. Only successfully prepared
    /// capability objects enter the candidate registry, so a failed source
    /// can never produce a partially initialized executor in the active
    /// snapshot, never erase the native base, and never suppress a sibling
    /// source that initialized successfully.
    ///
    /// # Errors
    ///
    /// Returns [`CapabilityPreparationError`] for failures of the *base*
    /// capability plane itself: Skill discovery, dependency conflicts,
    /// shared environment materialization, or registry composition.
    ///
    /// # Panics
    ///
    /// Panics only if the capability state lock is poisoned, which would
    /// mean a previous operation panicked while holding the lock.
    #[allow(clippy::too_many_lines)]
    pub async fn prepare_candidate(
        &self,
    ) -> Result<PreparedCapabilityCandidate, CapabilityPreparationError> {
        // Shared EnvironmentStore work is not cancelled by one conversation,
        // but a claimed conversation still counts the preparation owner
        // until it returns. This prevents an in-flight MCP/environment
        // preparation from creating a process or later callback after the
        // conversation reaches quiescence. A standalone coordinator keeps
        // its existing independent preparation semantics.
        let lifecycle = self
            .inner
            .state
            .lock()
            .expect("capability state lock poisoned")
            .conversation_lifecycle
            .clone();
        let _admission = if let Some(lifecycle) = lifecycle {
            Some(
                lifecycle
                    .try_enter_preparation()
                    .map_err(|_| CapabilityPreparationError::ConversationInactive)?,
            )
        } else {
            None
        };
        let base_revision = self
            .inner
            .state
            .lock()
            .expect("capability state lock poisoned")
            .revision;
        let packages =
            SkillDiscovery::with_config(&self.inner.workspace, self.inner.skill_discovery.clone())
                .discover()?;
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
        // ---- Optional capability sources (Issue #81) ----
        //
        // Each optional source is prepared in isolation: its failure is
        // recorded as its own typed availability state, never as a
        // preparation error of the whole candidate.
        let mut availability = CapabilityAvailability::new();
        let python_tools = match self.prepare_python_tools().await {
            Ok(tools) => {
                availability.insert(CapabilitySourceId::Python, CapabilitySourceState::Ready);
                tools
            }
            Err(reason) => {
                availability.insert(
                    CapabilitySourceId::Python,
                    CapabilitySourceState::unavailable(reason),
                );
                Vec::new()
            }
        };
        let mut discovered_tools = Vec::<ToolRegistration>::new();
        discovered_tools.extend(self.inner.base_tool_registry.registrations());
        let mut mcp_tools = Vec::new();
        let mut mcp_epochs = BTreeMap::new();
        // `BTreeMap` iteration is the deterministic identity order.
        for (server_id, binding) in &self.inner.mcp_servers {
            match self.prepare_mcp_server(server_id, binding).await {
                Ok((epoch, tools)) => {
                    mcp_epochs.insert(server_id.clone(), epoch);
                    mcp_tools.extend(tools);
                    availability.insert(
                        CapabilitySourceId::Mcp(server_id.clone()),
                        CapabilitySourceState::Ready,
                    );
                }
                Err(reason) => {
                    availability.insert(
                        CapabilitySourceId::Mcp(server_id.clone()),
                        CapabilitySourceState::unavailable(reason),
                    );
                }
            }
        }
        mcp_tools.extend(python_tools);
        discovered_tools.extend(
            mcp_tools
                .into_iter()
                .map(|(definition, executor)| ToolRegistration::plain(definition, executor)),
        );
        let (available_tools, candidate_registry) =
            select_tools(&discovered_tools, &self.inner.tool_activation)
                .map_err(CapabilityPreparationError::ToolActivation)?;
        let candidate_registry = Arc::new(candidate_registry);
        Ok(PreparedCapabilityCandidate {
            base_revision,
            skills,
            python,
            node,
            effective_environment,
            candidate_registry,
            available_tools: Arc::new(available_tools),
            mcp_epochs,
            availability,
        })
    }

    /// Prepares the complete custom Python tool plane: store opening,
    /// discovery, publication, environment materialization, and executor
    /// construction for every discovered package.
    ///
    /// The plane is one optional capability source: any failure —
    /// including opening/creating the Python-private store itself —
    /// rejects the whole plane (the caller records `Python` unavailable)
    /// so a partial Python executor set can never enter the candidate and
    /// Python storage can never fail core coordinator construction.
    async fn prepare_python_tools(
        &self,
    ) -> Result<
        Vec<(
            crate::tools::types::ToolDefinition,
            Arc<dyn crate::tools::executor::ToolExecutor>,
        )>,
        String,
    > {
        let store = self.python_store().map_err(|error| error.to_string())?;
        let python_packages = PythonToolDiscovery::new(&self.inner.workspace)
            .discover()
            .map_err(|error| error.to_string())?;
        let mut python_tools = Vec::new();
        for package in python_packages {
            let published = store.publish(&package).map_err(|error| error.to_string())?;
            let environment = store
                .ensure_environment(&published)
                .await
                .map_err(|error| error.to_string())?;
            let executor = Arc::new(PythonToolExecutor::new(&store, published, environment));
            python_tools.push((
                crate::tools::types::ToolDefinition {
                    id: crate::runtime::identity::ToolId::new(
                        crate::tools::python::python_tool_id(&package.name),
                    ),
                    name: package.name,
                    description: package.description,
                    input_schema: package.input_schema,
                    execution_policy: package.policy.execution,
                    concurrency_policy: package.policy.concurrency,
                    replay_policy: crate::tools::types::ToolReplayPolicy::Never,
                    origin: crate::tools::types::ToolOrigin::Python {
                        tool_version_id: package.tool_version_id,
                    },
                },
                executor as Arc<dyn crate::tools::executor::ToolExecutor>,
            ));
        }
        Ok(python_tools)
    }

    /// Returns the coordinator-lifetime-stable Python tool store,
    /// initializing it on first use.
    ///
    /// The slot starts empty because Python is optional: a construction
    /// failure leaves it empty (the caller records `Python` unavailable and
    /// the next preparation retries); the first successful construction is
    /// published into the slot under the mutex, so concurrent first
    /// preparations converge to exactly one store identity — the single
    /// process-local coordination domain for environment build coalescing
    /// and invocation bundle allocation. The mutex is never held across
    /// `.await`: construction is a bounded synchronous `create_dir_all`
    /// sequence.
    fn python_store(&self) -> Result<PythonToolStore, crate::tools::python::PythonToolError> {
        let mut slot = self
            .inner
            .python_store
            .lock()
            .expect("python store lock poisoned");
        if let Some(store) = &*slot {
            return Ok(store.clone());
        }
        let store = PythonToolStore::new(self.inner.python_store_root.clone())?;
        *slot = Some(store.clone());
        drop(slot);
        Ok(store)
    }

    /// Prepares one configured MCP server: connect (or reuse the retained
    /// runtime), snapshot the invalidation epoch, and fetch its complete
    /// catalog under the epoch-stability check.
    ///
    /// Each server is one optional capability source: the caller records a
    /// failure as that server's own unavailable state and continues with
    /// the remaining servers.
    async fn prepare_mcp_server(
        &self,
        server_id: &McpServerId,
        binding: &crate::tools::mcp::McpServerBinding,
    ) -> Result<
        (
            u64,
            Vec<(
                crate::tools::types::ToolDefinition,
                Arc<dyn crate::tools::executor::ToolExecutor>,
            )>,
        ),
        String,
    > {
        let runtimes = self.inner.mcp_runtimes.lock().await;
        let retained = runtimes.get(server_id).cloned();
        drop(runtimes);
        let runtime = match retained {
            Some(runtime) => runtime,
            None => self
                .connect_conversation_owned(server_id, binding)
                .await
                .map_err(|error| error.to_string())?,
        };
        // The epoch snapshot is taken under the shared invalidation
        // guard; the pagination itself never holds it.
        let epoch_before = self.inner.mcp_invalidation.epoch(server_id);
        let tools = runtime
            .list_tools()
            .await
            .map_err(|error| error.to_string())?;
        let epoch_after = self.inner.mcp_invalidation.epoch(server_id);
        if epoch_before != epoch_after {
            return Err("MCP tool catalog changed during discovery".to_owned());
        }
        Ok((
            epoch_after,
            crate::tools::mcp::definitions(server_id, binding.policy, &runtime, tools),
        ))
    }

    /// Prepares the **base-only** candidate of a subagent child runtime
    /// (Issue #60).
    ///
    /// The child's capability plane is its profile-frozen base registry and
    /// nothing else: no Skill discovery, no Python/Node environment
    /// materialization, no Python tool publication, and no MCP connection.
    /// The candidate is therefore deterministic, cheap, and exactly the
    /// deny-by-construction set the profile names.
    ///
    /// # Errors
    ///
    /// Returns [`CapabilityPreparationError::ConversationInactive`] when the
    /// claiming conversation is already draining, and
    /// [`CapabilityPreparationError::ToolRegistry`] if the base composition
    /// violates a registry invariant.
    ///
    /// # Panics
    ///
    /// Panics only if the coordinator state lock is poisoned, the same
    /// contract as every other coordinator boundary.
    pub fn prepare_base_only_candidate(
        &self,
    ) -> Result<PreparedCapabilityCandidate, CapabilityPreparationError> {
        let lifecycle = self
            .inner
            .state
            .lock()
            .expect("capability state lock poisoned")
            .conversation_lifecycle
            .clone();
        let _admission = if let Some(lifecycle) = lifecycle {
            Some(
                lifecycle
                    .try_enter_preparation()
                    .map_err(|_| CapabilityPreparationError::ConversationInactive)?,
            )
        } else {
            None
        };
        let base_revision = self
            .inner
            .state
            .lock()
            .expect("capability state lock poisoned")
            .revision;
        let base_registrations = self.inner.base_tool_registry.registrations();
        let (available_tools, candidate_registry) =
            select_tools(&base_registrations, &self.inner.tool_activation)
                .map_err(CapabilityPreparationError::ToolActivation)?;
        let candidate_registry = Arc::new(candidate_registry);
        Ok(PreparedCapabilityCandidate {
            base_revision,
            skills: Arc::new(SkillSnapshot::new(Vec::new())),
            python: None,
            node: None,
            effective_environment: self.inner.base_environment.clone(),
            candidate_registry,
            available_tools: Arc::new(available_tools),
            mcp_epochs: BTreeMap::new(),
            availability: CapabilityAvailability::new(),
        })
    }

    /// Connects one conversation-owned MCP server through an owner whose
    /// lifetime is independent of this caller (Issue #12, M9c).
    ///
    /// # Ownership phases
    ///
    /// ```text
    /// no physical owner
    ///   -> conversation-counted preparation owner (spawned task, counted
    ///      lifecycle admission, ownership cancellation child)
    ///   -> physical MCP process ownership established (inside connect)
    ///   -> either  A. transferred into the retained `mcp_runtimes` entry
    ///      or      B. cancelled/failed and driven to physical settlement
    /// ```
    ///
    /// The counted admission is released only after A or B, so aborting or
    /// dropping *this* future never removes the physical owner from the
    /// conversation's quiescence proof. The waiter is not the owner.
    async fn connect_conversation_owned(
        &self,
        server_id: &McpServerId,
        binding: &crate::tools::mcp::McpServerBinding,
    ) -> Result<Arc<McpServerRuntime>, CapabilityPreparationError> {
        let lifecycle = self
            .inner
            .state
            .lock()
            .expect("capability state lock poisoned")
            .conversation_lifecycle
            .clone();
        // The owner's own counted admission, acquired before any process is
        // spawned: if drain already won, nothing is spawned at all.
        let owner_admission = match &lifecycle {
            Some(lifecycle) => Some(
                lifecycle
                    .try_enter_preparation()
                    .map_err(|_| CapabilityPreparationError::ConversationInactive)?,
            ),
            None => None,
        };
        let cancellation = self.inner.mcp_preparation_cancellation.child();
        #[cfg(test)]
        let ownership_pause = self
            .inner
            .connect_ownership_pause
            .lock()
            .expect("connect ownership pause lock")
            .clone();
        let inner = Arc::clone(&self.inner);
        let server_id_owned = server_id.clone();
        let binding_owned = binding.clone();
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let request = crate::tools::mcp::OwnedConnect::new(
                &server_id_owned,
                &binding_owned,
                &inner.workspace,
                inner.mcp_invalidation.clone(),
                cancellation,
            );
            #[cfg(test)]
            let request = request.with_ownership_pause(ownership_pause);
            let outcome = match McpServerRuntime::connect_owned(request).await {
                // Phase A: ownership transfer. The runtime is retained
                // *before* the caller is answered and before this owner
                // leaves the quiescence accounting, so capability drain
                // always finds it — even if the caller was aborted.
                Ok(runtime) => {
                    let mut runtimes = inner.mcp_runtimes.lock().await;
                    // A concurrent owner may already have retained this
                    // identity. This connection is then *not* transferable, so
                    // this owner takes the cleanup path (phase B) instead of
                    // orphaning a live process outside the retained set.
                    if let Some(retained) = runtimes.get(&server_id_owned).cloned() {
                        drop(runtimes);
                        match runtime.close().await {
                            Ok(()) => Ok(retained),
                            Err(error) => Err(CapabilityPreparationError::Mcp(format!(
                                "duplicate MCP connection could not settle: {error}"
                            ))),
                        }
                    } else {
                        runtimes.insert(server_id_owned.clone(), runtime.clone());
                        drop(runtimes);
                        Ok(runtime)
                    }
                }
                // Phase B: the connect owner already drove its physical
                // process (when one existed) to settlement before returning.
                Err(error) => Err(CapabilityPreparationError::Mcp(error.to_string())),
            };
            let _ = result_tx.send(outcome);
            drop(owner_admission);
        });
        result_rx.await.unwrap_or_else(|_| {
            Err(CapabilityPreparationError::Mcp(
                "the MCP connection owner terminated without an outcome".to_owned(),
            ))
        })
    }

    /// Requests cancellation of every in-flight conversation-owned MCP
    /// preparation owner (Issue #12, M9c).
    ///
    /// This is a synchronous non-blocking control operation taken by the
    /// runtime drain transition. It never waits: each owner settles its own
    /// physical process and only then releases the counted lifecycle
    /// admission that quiescence waits on.
    pub(crate) fn cancel_conversation_preparation(&self) {
        self.inner.mcp_preparation_cancellation.cancel();
    }

    /// Settles the conversation-owned capability process plane before the
    /// conversation runtime publishes quiescence. Shared environment-store
    /// materialization is handled by the counted preparation admission above;
    /// this method closes only the MCP runtimes retained by this capability
    /// coordinator.
    ///
    /// Every retained runtime receives `close` and is awaited to its
    /// strongest local settlement, **including after a sibling reports a
    /// failure**: a failed participant is an error fact, never permission to
    /// abandon another participant that can still act. Failures are collected
    /// in deterministic identity order and returned together.
    ///
    /// # Errors
    ///
    /// Returns one physical-settlement diagnostic per owned MCP stdio unit
    /// that could not prove its terminality.
    pub(crate) async fn drain_conversation_owned(&self) -> Result<(), Vec<String>> {
        let lifecycle = self
            .inner
            .state
            .lock()
            .expect("capability state lock poisoned")
            .conversation_lifecycle
            .clone();
        let _settlement = match &lifecycle {
            Some(lifecycle) => match lifecycle.try_enter_settlement() {
                Ok(admission) => Some(admission),
                Err(state) => {
                    return Err(vec![format!(
                        "capability drain entered lifecycle state {state:?}"
                    )]);
                }
            },
            None => None,
        };
        let runtimes = {
            let runtimes = self.inner.mcp_runtimes.lock().await;
            runtimes.values().cloned().collect::<Vec<_>>()
        };
        let mut failures = Vec::new();
        for runtime in runtimes {
            if let Err(error) = runtime.close().await {
                failures.push(format!("MCP server {}: {error}", runtime.server_id()));
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(failures)
        }
    }

    /// Installs the test-only MCP connect ownership pause.
    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn install_connect_ownership_pause(
        &self,
        pause: Arc<crate::tools::mcp::test_sync::ConnectOwnershipPause>,
    ) {
        *self
            .inner
            .connect_ownership_pause
            .lock()
            .expect("connect ownership pause lock") = Some(pause);
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
    /// # Availability (Issue #81)
    ///
    /// The candidate's per-source availability outcome replaces the
    /// authoritative availability state at the same linearization point.
    /// Availability is control-plane health, not execution identity:
    /// [`CapabilityRevision`] advances only when the effective committed
    /// executable capability set changes, so an availability-only change
    /// never fabricates a revision — but it still fires the observer, so
    /// the Runtime Client projection observes the new state. A rejected
    /// candidate (stale, busy, or MCP-invalidated) mutates nothing.
    ///
    /// # Lifecycle (Issue #61)
    ///
    /// Once a `ConversationRuntime` owns this coordinator, live capability
    /// mutation follows the runtime lifecycle: a commit while the owning
    /// runtime is inactive is refused with
    /// [`CapabilityCommitError::ConversationInactive`] and changes nothing.
    /// The startup commit performed *before* the conversation runtime is
    /// constructed (the coordinator is unclaimed then) remains allowed, and
    /// after `ConversationRuntime::activate` commits follow the normal
    /// quiescence rules. The gate is observed under this same
    /// synchronization boundary, so a commit linearizes cleanly against
    /// activation: a commit that observes the pre-activation state is
    /// refused, one that observes the post-activation state is a real
    /// post-activation transition.
    ///
    /// # MCP invalidation linearization
    ///
    /// The final epoch validation and the snapshot swap happen under the
    /// shared MCP invalidation guard (the same guard a `tools/list_changed`
    /// notification holds when advancing its epoch). The synchronization
    /// order is therefore:
    ///
    /// ```text
    /// notification wins  → epoch advanced before the commit's validation
    ///                      → candidate is stale → active snapshot unchanged
    /// commit wins        → epoch validated and snapshot swapped first
    ///                      → the notification belongs to a future refresh
    ///                      and can never retroactively invalidate the
    ///                      committed snapshot
    /// ```
    ///
    /// # Lock ordering
    ///
    /// The capability state lock is always acquired before the MCP
    /// invalidation guard; the notification path acquires only the
    /// invalidation guard, so no cycle exists.
    ///
    /// # Errors
    ///
    /// Returns [`CapabilityCommitError::ConversationInactive`] while the
    /// owning conversation runtime is inactive,
    /// [`CapabilityCommitError::Busy`] while an attempt lease is active and
    /// [`CapabilityCommitError::StaleCandidate`] for an obsolete base
    /// revision.
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
        // Hold the shared lifecycle admission guard across the capability
        // state lock, MCP validation, snapshot swap, and observer callback.
        // The final revision swap below also takes the lifecycle commit
        // boundary, so drain and this non-coordinator commit have one exact
        // order at the authoritative revision point. Standalone
        // coordinators have no conversation lifecycle and retain their
        // existing independent semantics.
        let _admission = if let Some(lifecycle) = &state.conversation_lifecycle {
            Some(
                lifecycle
                    .try_enter_running()
                    .map_err(|_| CapabilityCommitError::ConversationInactive)?,
            )
        } else {
            None
        };
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
        let lifecycle = state.conversation_lifecycle.clone();
        let commit = || {
            // The lifecycle commit boundary is held around the final
            // validation and revision swap. If drain won while the
            // deterministic hook was parked, this operation is refused
            // before it can mutate the candidate revision. If it acquires
            // the boundary first, drain follows the complete observer
            // handoff.
            if candidate.base_revision != state.revision {
                return Err(CapabilityCommitError::StaleCandidate {
                    prepared_from: candidate.base_revision,
                    current: state.revision,
                });
            }
            if state.active_attempts > 0 {
                return Err(CapabilityCommitError::Busy);
            }
            // The MCP invalidation guard: final epoch validation and the
            // snapshot swap are one atomic step against notification epoch
            // mutation. Lock order: capability state lock -> invalidation
            // guard.
            let invalidation = self.inner.mcp_invalidation.lock();
            for (server_id, candidate_epoch) in &candidate.mcp_epochs {
                if invalidation.epoch(server_id) != *candidate_epoch {
                    return Err(CapabilityCommitError::StaleMcpCandidate {
                        server_id: server_id.clone(),
                    });
                }
            }
            if candidate_is_noop(&candidate, &state.snapshot) {
                let availability_changed =
                    Self::install_availability(&mut state, &candidate.availability);
                let snapshot = state.snapshot.clone();
                drop(invalidation);
                if availability_changed {
                    // An availability-only change never advances the
                    // revision, but it is still observed.
                    Self::fire_observer(&self.inner.observer, &snapshot, &state.availability);
                }
                return Ok(snapshot);
            }
            Self::install_availability(&mut state, &candidate.availability);
            let revision = CapabilityRevision::new(state.revision.get() + 1);
            let snapshot = Arc::new(CapabilitySnapshot::new(
                self.inner.conversation_id.clone(),
                self.inner.workspace.root().to_path_buf(),
                revision,
                candidate.candidate_registry,
                candidate.available_tools,
                candidate.skills,
                candidate.python,
                candidate.node,
                candidate.effective_environment,
            ));
            state.revision = revision;
            state.snapshot = snapshot.clone();
            let availability = state.availability.clone();
            drop(invalidation);
            Self::fire_observer(&self.inner.observer, &snapshot, &availability);
            Ok(snapshot)
        };
        let result = if let Some(lifecycle) = lifecycle {
            lifecycle
                .with_running_commit(commit)
                .map_err(|_| CapabilityCommitError::ConversationInactive)?
        } else {
            commit()
        };
        drop(state);
        self.inner.condvar.notify_all();
        result
    }

    /// The current authoritative availability state of every evaluated
    /// optional capability source (Issue #81).
    ///
    /// # Panics
    ///
    /// Panics only if the capability state lock is poisoned, which would
    /// mean a previous operation panicked while holding the lock.
    #[must_use]
    pub fn availability(&self) -> CapabilityAvailability {
        self.inner
            .state
            .lock()
            .expect("capability state lock poisoned")
            .availability
            .clone()
    }

    /// Installs the candidate's availability outcome into the authoritative
    /// state, returning whether it changed. Availability mutates only at
    /// the commit linearization point (a rejected candidate never reaches
    /// this).
    fn install_availability(
        state: &mut CoordinatorState,
        availability: &CapabilityAvailability,
    ) -> bool {
        if state.availability == *availability {
            return false;
        }
        state.availability.clone_from(availability);
        true
    }

    /// Fires the installed observer with the activated snapshot and the
    /// authoritative availability, under the same synchronization boundary
    /// the commit held (lock order: capability state lock -> observer
    /// lock).
    fn fire_observer(
        observer_slot: &Mutex<Option<Arc<dyn CapabilityObserver>>>,
        snapshot: &CapabilitySnapshot,
        availability: &CapabilityAvailability,
    ) {
        let observer = observer_slot
            .lock()
            .expect("capability observer lock poisoned")
            .clone();
        if let Some(observer) = &observer {
            observer.on_snapshot(snapshot, availability);
        }
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

    /// Test-only observation of the lazy Python store slot: `None` when no
    /// Python preparation has successfully initialized the store yet,
    /// otherwise the identity token of the one stable coordination domain.
    #[cfg(test)]
    pub(crate) fn python_store_identity_token(&self) -> Option<usize> {
        self.inner
            .python_store
            .lock()
            .expect("python store lock poisoned")
            .as_ref()
            .map(PythonToolStore::identity_token)
    }

    /// Installs the observer and captures the active snapshot and the
    /// authoritative availability state as one atomic coordinator section.
    ///
    /// This is the capability half of the Issue #61 adapter bootstrap
    /// handshake: installation and the snapshot capture share the one
    /// capability state synchronization boundary (the same section a
    /// commit holds while firing the observer), so an activation either
    /// linearizes before the section (its snapshot and availability are
    /// the returned seed and no observation was fired — the observer did
    /// not exist yet) or after it (the installed observer fires it into
    /// the bridge queue). No activation can be lost between the seed and
    /// the live observation stream and none can be applied twice.
    ///
    /// # Panics
    ///
    /// Panics only if the capability state lock or the observer lock is
    /// poisoned.
    pub(crate) fn install_observer_and_snapshot(
        &self,
        observer: Arc<dyn CapabilityObserver>,
    ) -> (Arc<CapabilitySnapshot>, CapabilityAvailability) {
        // Lock order: capability state lock -> observer lock, the same
        // order `commit` uses when it fires the observer.
        let state = self
            .inner
            .state
            .lock()
            .expect("capability state lock poisoned");
        *self.inner.observer.lock().expect("observer lock") = Some(observer);
        (state.snapshot.clone(), state.availability.clone())
    }

    /// The shared MCP invalidation state (test observability). Only
    /// available under `#[cfg(test)]`; never used by production code.
    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn mcp_invalidation(&self) -> Arc<McpInvalidationState> {
        self.inner.mcp_invalidation.clone()
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
        && candidate.candidate_registry.definitions() == current.tool_registry().definitions()
        && candidate.available_tools.as_ref() == current.available_tools()
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
            workspace: workspace.clone(),
            base_tool_registry: Arc::new(ToolRegistry::new()),
            tool_activation: crate::capabilities::ToolActivationPolicy::default(),
            skill_discovery: crate::skills::SkillDiscoveryConfig::default_for_workspace(&workspace),
            mcp_servers: std::collections::BTreeMap::new(),
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

    /// Runtime drain wins the capability commit boundary: a candidate may be
    /// prepared while the runtime is running, but the revision swap is
    /// refused when drain linearizes before the final lifecycle read.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn runtime_drain_wins_capability_commit_boundary() {
        let (_dir, coordinator) = coordinator();
        let lifecycle = crate::runtime::types::ConversationLifecycle::new();
        assert!(coordinator.claim_conversation_runtime(&lifecycle));
        assert!(lifecycle.activate());
        let candidate = prepare(&coordinator).await;
        let hook = Arc::new(CommitBoundaryHook::default());
        coordinator.install_commit_boundary_hook(hook.clone());

        let coordinator_for_task = coordinator.clone();
        let commit_task = std::thread::spawn(move || coordinator_for_task.commit(candidate));
        hook.wait_entered();
        assert!(lifecycle.begin_drain());
        hook.proceed();
        let result = commit_task.join().expect("commit task");
        assert_eq!(result, Err(CapabilityCommitError::ConversationInactive));
        assert_eq!(
            coordinator.current_snapshot().revision(),
            CapabilityRevision::default(),
            "drain wins before the capability revision swap"
        );
        lifecycle.wait_for_no_admissions().await;
        assert!(lifecycle.mark_quiescent());
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

    /// Lazy Python store ownership (Issue #81): store initialization is
    /// optional and retryable, but once it succeeds the coordinator retains
    /// one stable process-local store identity for its lifetime.
    ///
    /// Phase 1: the Python store path is a conflicting regular file —
    /// preparation degrades Python to `Unavailable` and the lazy slot stays
    /// empty (no permanently poisoned state). Phase 2: the filesystem is
    /// repaired and the next preparation retries, initializes the store,
    /// and Python becomes `Ready`. Phase 3: later preparations reuse the
    /// same store identity instead of constructing a new coordination
    /// domain (which would restart invocation bundle allocation).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn python_store_initialization_is_lazy_retryable_and_stable() {
        use crate::capabilities::{CapabilitySourceId, CapabilitySourceState};

        let dir = tempfile::tempdir().expect("temp dir");
        let workspace_root = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace_root).expect("workspace");
        let store_root = dir.path().join("skill-env");
        std::fs::create_dir_all(&store_root).expect("environment store root");
        let conflict = store_root.join("m7-tools");
        std::fs::write(&conflict, b"not a directory").expect("conflicting regular file");
        let coordinator = CapabilityCoordinator::new(CapabilityCoordinatorConfig {
            conversation_id: crate::runtime::identity::ConversationId::new("conv-lazy-store"),
            workspace: Workspace::new(&workspace_root).expect("workspace"),
            base_tool_registry: Arc::new(ToolRegistry::new()),
            tool_activation: crate::capabilities::ToolActivationPolicy::default(),
            skill_discovery: crate::skills::SkillDiscoveryConfig::default(),
            mcp_servers: std::collections::BTreeMap::new(),
            base_environment: ToolEnvironment::new(),
            environment_store_root: store_root,
        })
        .expect("coordinator construction never touches Python storage");

        // Phase 1: initialization fails, the slot stays empty, preparation
        // itself succeeds with Python unavailable.
        assert_eq!(coordinator.python_store_identity_token(), None);
        let candidate = coordinator.prepare_candidate().await.expect("prepare");
        assert!(
            matches!(
                candidate.availability().get(&CapabilitySourceId::Python),
                Some(CapabilitySourceState::Unavailable { .. })
            ),
            "the store-opening failure degrades Python availability: {:?}",
            candidate.availability()
        );
        assert_eq!(
            coordinator.python_store_identity_token(),
            None,
            "a failed initialization must not poison the lazy slot"
        );

        // Phase 2: the filesystem is repaired; the next preparation retries
        // and publishes the one stable store identity.
        std::fs::remove_file(&conflict).expect("repair the filesystem");
        let candidate = coordinator
            .prepare_candidate()
            .await
            .expect("retry prepare");
        assert_eq!(
            candidate.availability().get(&CapabilitySourceId::Python),
            Some(&CapabilitySourceState::Ready),
            "the retry initializes the store and Python becomes ready"
        );
        let first = coordinator
            .python_store_identity_token()
            .expect("the store is initialized after the successful retry");

        // Phase 3: a later preparation reuses the same coordination
        // identity rather than constructing a fresh store.
        let candidate = coordinator
            .prepare_candidate()
            .await
            .expect("third prepare");
        assert_eq!(
            candidate.availability().get(&CapabilitySourceId::Python),
            Some(&CapabilitySourceState::Ready)
        );
        assert_eq!(
            coordinator.python_store_identity_token(),
            Some(first),
            "the coordinator retains one stable PythonToolStore identity"
        );
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

/// Coordinator-level MCP invalidation-vs-commit linearization regressions
/// (Issue #10). These drive the exact section-1 interleavings through
/// `CapabilityCoordinator` against the official-rmcp local fixture server,
/// with deterministic synchronization hooks — never sleeps.
///
/// The fixture server runs as a self-spawned stdio process (the test binary
/// re-executes itself in fixture mode when `RUSTX_M7_MCP_FIXTURE` is set),
/// the same pattern the `m7_mcp` integration tests use.
#[cfg(all(test, feature = "mcp-fixture"))]
mod mcp_race_tests {
    use std::sync::Arc;

    use super::test_sync::CommitBoundaryHook;
    use super::{CapabilityCommitError, CapabilityCoordinator, CapabilityCoordinatorConfig};
    use crate::runtime::identity::{ConversationId, McpServerId};
    use crate::tools::environment::ToolEnvironment;
    use crate::tools::executor::ToolRegistry;
    use crate::tools::mcp::fixture::{FixtureServer, fixture_spawn_args, serve_if_fixture_mode};
    use crate::tools::mcp::{McpInvalidationState, McpTransportConfig};
    use crate::tools::workspace::Workspace;

    fn coordinator_with_fixture(
        dir: &tempfile::TempDir,
        server_id: &str,
        test_name: &str,
    ) -> (CapabilityCoordinator, McpServerId) {
        let workspace_root = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace_root).expect("workspace");
        let workspace = Workspace::new(&workspace_root).expect("workspace");
        let server_id = McpServerId::new(server_id);
        let coordinator = CapabilityCoordinator::new(CapabilityCoordinatorConfig {
            conversation_id: ConversationId::new("mcp-race"),
            workspace,
            base_tool_registry: Arc::new(ToolRegistry::new()),
            tool_activation: crate::capabilities::ToolActivationPolicy::default(),
            skill_discovery: crate::skills::SkillDiscoveryConfig::default(),
            mcp_servers: std::collections::BTreeMap::from([(
                server_id.clone(),
                crate::tools::mcp::McpServerBinding {
                    transport: McpTransportConfig::Stdio {
                        program: std::env::current_exe()
                            .expect("test executable")
                            .display()
                            .to_string(),
                        args: fixture_spawn_args(test_name),
                        cwd: None,
                        environment: std::collections::BTreeMap::from([(
                            crate::tools::mcp::fixture::FIXTURE_MODE_ENV.to_owned(),
                            "1".to_owned(),
                        )]),
                    },
                    policy: crate::tools::types::ToolInvocationPolicy::default(),
                },
            )]),
            base_environment: ToolEnvironment::new(),
            environment_store_root: dir.path().join("skill-env"),
        })
        .expect("coordinator");
        (coordinator, server_id)
    }

    /// A coordinator with `server_ids` self-spawned stdio fixture servers,
    /// claimed by (and activated with) its own conversation lifecycle so the
    /// M9c runtime-owned drain path is exercised exactly as the conversation
    /// runtime composes it.
    fn claimed_coordinator_with_fixtures(
        dir: &tempfile::TempDir,
        server_ids: &[&str],
        test_name: &str,
    ) -> (
        CapabilityCoordinator,
        crate::runtime::types::ConversationLifecycle,
        Vec<McpServerId>,
    ) {
        let workspace_root = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace_root).expect("workspace");
        let workspace = Workspace::new(&workspace_root).expect("workspace");
        let ids: Vec<McpServerId> = server_ids.iter().map(|id| McpServerId::new(*id)).collect();
        let mcp_servers = ids
            .iter()
            .map(|id| {
                (
                    id.clone(),
                    crate::tools::mcp::McpServerBinding {
                        transport: McpTransportConfig::Stdio {
                            program: std::env::current_exe()
                                .expect("test executable")
                                .display()
                                .to_string(),
                            args: fixture_spawn_args(test_name),
                            cwd: None,
                            environment: std::collections::BTreeMap::from([(
                                crate::tools::mcp::fixture::FIXTURE_MODE_ENV.to_owned(),
                                "1".to_owned(),
                            )]),
                        },
                        policy: crate::tools::types::ToolInvocationPolicy::default(),
                    },
                )
            })
            .collect();
        let coordinator = CapabilityCoordinator::new(CapabilityCoordinatorConfig {
            conversation_id: ConversationId::new("mcp-drain"),
            workspace,
            base_tool_registry: Arc::new(ToolRegistry::new()),
            tool_activation: crate::capabilities::ToolActivationPolicy::default(),
            skill_discovery: crate::skills::SkillDiscoveryConfig::default(),
            mcp_servers,
            base_environment: ToolEnvironment::new(),
            environment_store_root: dir.path().join("skill-env"),
        })
        .expect("coordinator");
        let lifecycle = crate::runtime::types::ConversationLifecycle::new();
        assert!(coordinator.claim_conversation_runtime(&lifecycle));
        assert!(lifecycle.activate());
        (coordinator, lifecycle, ids)
    }

    /// Issue #12 (M9c, Blocker A / 4.2): one owned MCP runtime reporting an
    /// unproven physical settlement must not release the capability drain
    /// from a sibling runtime that has not been closed yet.
    ///
    /// Happens-before: `alpha` is probed to fail its close and `beta` is
    /// probed to park inside its close. The test waits for `beta`'s close to
    /// be *entered* — which can only happen after `alpha`'s close already
    /// returned its failure — and asserts the drain has not returned. Only
    /// releasing `beta` lets the drain report the collected failure.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn one_failed_mcp_close_never_abandons_a_sibling_runtime() {
        use crate::tools::mcp::test_sync::CloseProbe;

        if serve_if_fixture_mode(FixtureServer::with_list_changed()).await {
            return;
        }
        let dir = tempfile::tempdir().expect("temp dir");
        let (coordinator, lifecycle, ids) = claimed_coordinator_with_fixtures(
            &dir,
            &["alpha", "beta"],
            "capabilities::coordinator::mcp_race_tests::one_failed_mcp_close_never_abandons_a_sibling_runtime",
        );
        // Both fixture servers publish an identical catalog, so the candidate
        // itself is rejected at registry composition. That is irrelevant here
        // and deliberately not asserted on: MCP *ownership* is established
        // when each connection owner retains its runtime, strictly before
        // composition, and the retained set is exactly what drain owns.
        let _ = coordinator.prepare_candidate().await;
        assert_eq!(
            coordinator.inner.mcp_runtimes.lock().await.len(),
            2,
            "both conversation-owned MCP runtimes are retained"
        );

        let alpha = Arc::new(CloseProbe::failing("injected unproven physical settlement"));
        let beta = Arc::new(CloseProbe::parking());
        {
            let runtimes = coordinator.inner.mcp_runtimes.lock().await;
            runtimes
                .get(&ids[0])
                .expect("alpha runtime")
                .install_close_probe(alpha.clone());
            runtimes
                .get(&ids[1])
                .expect("beta runtime")
                .install_close_probe(beta.clone());
        }

        assert!(lifecycle.begin_drain());
        let (done_tx, mut done_rx) = tokio::sync::oneshot::channel();
        let drain_coordinator = coordinator.clone();
        tokio::spawn(async move {
            let _ = done_tx.send(drain_coordinator.drain_conversation_owned().await);
        });

        // `beta` can only be entered after `alpha` already returned its
        // failure, because the drain closes retained runtimes in identity
        // order.
        tokio::time::timeout(std::time::Duration::from_secs(60), beta.wait_entered())
            .await
            .expect("the sibling runtime must still receive close after a failed sibling");
        assert!(
            alpha.was_entered(),
            "the failing runtime was attempted first"
        );
        assert!(
            matches!(
                done_rx.try_recv(),
                Err(tokio::sync::oneshot::error::TryRecvError::Empty)
            ),
            "the drain must not report before the sibling has settled"
        );

        beta.release();
        let result = tokio::time::timeout(std::time::Duration::from_secs(60), done_rx)
            .await
            .expect("drain completes once every owned runtime settled")
            .expect("drain result channel");
        let failures = result.expect_err("the failing close is reported");
        assert_eq!(failures.len(), 1, "exactly the failing runtime is reported");
        assert!(
            failures[0].contains("alpha") && failures[0].contains("injected"),
            "the collected diagnostic names the failing runtime: {failures:?}"
        );
    }

    /// Issue #12 (M9c, Blocker B): waiter lifetime is not ownership lifetime.
    /// Aborting the `prepare_candidate` future after an MCP stdio process has
    /// been spawned must not remove that physical owner from the
    /// conversation's quiescence accounting.
    ///
    /// Happens-before: the connect owner parks exactly once physical process
    /// ownership exists and before the handshake. The caller future is then
    /// aborted and *joined*, so it is provably gone. Drain linearizes and
    /// cancels preparation, yet the counted admission is still held and
    /// `mark_quiescent` refuses. Only releasing the parked owner — which then
    /// drives its process to physical settlement — completes the proof.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn dropped_mcp_preparation_still_owes_physical_settlement() {
        use crate::tools::mcp::test_sync::ConnectOwnershipPause;

        if serve_if_fixture_mode(FixtureServer::with_list_changed()).await {
            return;
        }
        let dir = tempfile::tempdir().expect("temp dir");
        let (coordinator, lifecycle, _ids) = claimed_coordinator_with_fixtures(
            &dir,
            &["alpha"],
            "capabilities::coordinator::mcp_race_tests::dropped_mcp_preparation_still_owes_physical_settlement",
        );
        let pause = Arc::new(ConnectOwnershipPause::default());
        coordinator.install_connect_ownership_pause(pause.clone());

        let preparing = coordinator.clone();
        let caller = tokio::spawn(async move { preparing.prepare_candidate().await });
        tokio::time::timeout(std::time::Duration::from_secs(60), pause.wait_entered())
            .await
            .expect("physical MCP process ownership is established");

        // The waiter is destroyed. Under the reviewed implementation this
        // released the whole preparation admission and left a detached
        // physical owner outside the quiescence proof.
        caller.abort();
        assert!(
            caller
                .await
                .expect_err("the caller future was aborted")
                .is_cancelled(),
            "the caller future is provably gone"
        );

        assert!(lifecycle.begin_drain());
        coordinator.cancel_conversation_preparation();
        assert!(
            !lifecycle.mark_quiescent(),
            "the physical MCP owner is still counted after the waiter was dropped"
        );
        // Release the owner: it observes its ownership cancellation and
        // drives the spawned process to its physical settlement proof.
        pause.release();
        tokio::time::timeout(
            std::time::Duration::from_secs(60),
            lifecycle.wait_for_no_admissions(),
        )
        .await
        .expect("the physical owner settles and releases its counted admission");
        coordinator
            .drain_conversation_owned()
            .await
            .expect("a cancelled preparation retains no runtime to close");
        assert!(
            lifecycle.mark_quiescent(),
            "quiescence is reachable only once the physical owner settled"
        );
    }

    /// The invalidation "notification" hook: the exact mutation the
    /// `tools/list_changed` notification performs, under the shared guard.
    fn advance_notification(coordinator: &CapabilityCoordinator, server_id: &McpServerId) {
        coordinator.mcp_invalidation().lock().advance(server_id);
    }

    /// Notification wins first: the commit is parked inside its critical
    /// section (the deciding quiescence observation is next), the
    /// notification advances the shared epoch, and the released commit must
    /// observe the stale epoch — the prepared candidate cannot commit and
    /// the active snapshot is unchanged.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn mcp_notification_wins_prepared_candidate_is_stale_and_active_snapshot_unchanged() {
        if serve_if_fixture_mode(FixtureServer::with_list_changed()).await {
            return;
        }
        let dir = tempfile::tempdir().expect("temp dir");
        let (coordinator, server_id) = coordinator_with_fixture(
            &dir,
            "fixture",
            "capabilities::coordinator::mcp_race_tests::mcp_notification_wins_prepared_candidate_is_stale_and_active_snapshot_unchanged",
        );
        let hook = Arc::new(CommitBoundaryHook::default());
        coordinator.install_commit_boundary_hook(hook.clone());
        let candidate = coordinator
            .prepare_candidate()
            .await
            .expect("prepare with the fixture catalog");

        let coordinator_for_task = coordinator.clone();
        let commit_task = std::thread::spawn(move || coordinator_for_task.commit(candidate));
        hook.wait_entered();
        // The notification wins while the commit is parked before its
        // deciding observations: the epoch mutation serializes against the
        // commit's epoch validation through the shared guard.
        advance_notification(&coordinator, &server_id);
        hook.proceed();
        let result = commit_task.join().expect("commit task");
        assert!(
            matches!(
                &result,
                Err(CapabilityCommitError::StaleMcpCandidate { server_id: id })
                    if *id == server_id
            ),
            "the notification-winning interleaving must reject the candidate: {result:?}"
        );
        assert_eq!(
            coordinator.current_snapshot().revision().get(),
            0,
            "the active snapshot is unchanged when the notification wins"
        );
        coordinator
            .inner
            .mcp_runtimes
            .lock()
            .await
            .get(&server_id)
            .expect("runtime")
            .close()
            .await
            .expect("the owned stdio unit must publish physical settlement");
    }

    /// Commit wins first: the candidate activates, a later notification
    /// advances the epoch, the already-committed snapshot is never
    /// retroactively invalidated (the old candidate is stale), and the next
    /// refresh — prepared against the new epoch — commits normally.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn mcp_commit_wins_later_notification_belongs_to_the_next_refresh() {
        if serve_if_fixture_mode(FixtureServer::with_list_changed()).await {
            return;
        }
        let dir = tempfile::tempdir().expect("temp dir");
        let (coordinator, server_id) = coordinator_with_fixture(
            &dir,
            "fixture",
            "capabilities::coordinator::mcp_race_tests::mcp_commit_wins_later_notification_belongs_to_the_next_refresh",
        );
        let first = coordinator
            .prepare_candidate()
            .await
            .expect("prepare with the fixture catalog");
        let snapshot = coordinator
            .commit(first)
            .expect("the commit wins with no active lease and a stable epoch");
        assert_eq!(snapshot.revision().get(), 1);

        // A candidate prepared before the notification, while the commit
        // already won.
        let pre_notification = coordinator
            .prepare_candidate()
            .await
            .expect("prepare before the notification");

        // The notification arrives after the commit: it advances the epoch
        // and can never touch the committed snapshot.
        advance_notification(&coordinator, &server_id);
        assert!(
            matches!(
                coordinator.commit(pre_notification),
                Err(CapabilityCommitError::StaleMcpCandidate { .. })
            ),
            "a candidate prepared before the notification is stale after it"
        );

        // The next refresh observes the advanced epoch — its candidate
        // carries the post-notification epoch — and commits (an unchanged
        // rediscovery is the M6 no-op that returns the active snapshot).
        let refresh = coordinator
            .prepare_candidate()
            .await
            .expect("refresh prepare");
        assert_eq!(
            refresh.mcp_epochs.get(&server_id),
            Some(&1),
            "the next refresh must observe the advanced epoch"
        );
        let refreshed = coordinator
            .commit(refresh)
            .expect("the refreshed candidate commits");
        assert_eq!(
            refreshed.revision().get(),
            1,
            "an unchanged rediscovery is a no-op, never a new revision"
        );
        coordinator
            .inner
            .mcp_runtimes
            .lock()
            .await
            .get(&server_id)
            .expect("runtime")
            .close()
            .await
            .expect("the owned stdio unit must publish physical settlement");
    }

    /// A real `tools/list_changed` notification from the fixture server
    /// advances the shared epoch, and a candidate prepared before it can no
    /// longer commit.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn mcp_real_list_changed_notification_rejects_the_prepared_candidate() {
        struct NoProgress;
        impl crate::tools::executor::ProgressReporter for NoProgress {
            fn report(&self, _progress: crate::tools::types::ToolProgress) {}
        }
        if serve_if_fixture_mode(FixtureServer::with_list_changed()).await {
            return;
        }
        let dir = tempfile::tempdir().expect("temp dir");
        let (coordinator, server_id) = coordinator_with_fixture(
            &dir,
            "fixture",
            "capabilities::coordinator::mcp_race_tests::mcp_real_list_changed_notification_rejects_the_prepared_candidate",
        );
        let candidate = coordinator
            .prepare_candidate()
            .await
            .expect("prepare with the fixture catalog");
        let runtime = coordinator
            .inner
            .mcp_runtimes
            .lock()
            .await
            .get(&server_id)
            .expect("runtime")
            .clone();
        drop(coordinator.inner.mcp_runtimes.lock().await);
        let initial_epoch = runtime.change_epoch();
        assert_eq!(initial_epoch, 0);

        // Drive the fixture's mutate tool: it emits a real
        // tools/list_changed notification.
        let tools = runtime.list_tools().await.expect("tools/list");
        let definitions = crate::tools::mcp::definitions(
            &server_id,
            crate::tools::types::ToolInvocationPolicy::default(),
            &runtime,
            tools,
        );
        let mutate_index = definitions
            .iter()
            .position(|(definition, _)| definition.name == "mutate")
            .expect("mutate definition");
        let executor = definitions[mutate_index].1.clone();
        let artifacts_dir = dir.path().join("mcp-race-artifacts");
        let runtime_bundle = crate::tools::runtime::ConversationToolRuntime::new(
            ConversationId::new("mcp-race"),
            dir.path().join("workspace"),
            &artifacts_dir,
        )
        .expect("tool runtime");
        let progress = NoProgress;
        let result = crate::tools::executor::ToolExecutor::execute(
            executor.as_ref(),
            crate::tools::types::ToolInvocation {
                call_id: crate::runtime::identity::ToolCallId::new("mutate"),
                tool_id: definitions[mutate_index].0.id.clone(),
                tool_name: "mutate".to_owned(),
                mode: crate::tools::types::ToolInvocationMode::Foreground,
                arguments: serde_json::json!({}),
            },
            crate::tools::executor::ToolExecutionContext {
                conversation_id: runtime_bundle.conversation_id(),
                execution_id: None,
                cancellation: crate::runtime::ExecutionCancellation::detached(
                    crate::runtime::CancellationSignal::new(),
                    crate::runtime::types::CancellationReason::UserRequested,
                ),
                workspace: runtime_bundle.workspace(),
                progress: &progress,
                artifacts: runtime_bundle.artifacts(),
                tool_output: runtime_bundle.tool_output(),
                environment: runtime_bundle.environment(),
                skill_resources: None,
            },
        )
        .await;
        assert!(matches!(
            result.status,
            crate::tools::types::ToolExecutionStatus::Success
        ));
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            runtime.wait_for_change(initial_epoch),
        )
        .await
        .expect("the real notification must advance the shared epoch");

        // The candidate prepared before the notification is stale.
        assert!(matches!(
            coordinator.commit(candidate),
            Err(CapabilityCommitError::StaleMcpCandidate { .. })
        ));
        runtime
            .close()
            .await
            .expect("the owned stdio unit must publish physical settlement");
    }

    /// The active-attempt lease contract holds with MCP servers configured:
    /// a commit while the lease is active is Busy, and the in-flight attempt
    /// keeps using its old registry after a later commit activates a new
    /// revision.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn mcp_active_attempt_lease_keeps_commit_busy_and_pins_the_old_registry() {
        if serve_if_fixture_mode(FixtureServer::with_list_changed()).await {
            return;
        }
        let dir = tempfile::tempdir().expect("temp dir");
        let (coordinator, server_id) = coordinator_with_fixture(
            &dir,
            "fixture",
            "capabilities::coordinator::mcp_race_tests::mcp_active_attempt_lease_keeps_commit_busy_and_pins_the_old_registry",
        );
        let lease = coordinator.acquire_attempt_lease();
        let old_registry = lease.snapshot().tool_registry().clone();
        let candidate = coordinator
            .prepare_candidate()
            .await
            .expect("prepare with the fixture catalog");
        assert_eq!(
            coordinator.commit(candidate),
            Err(CapabilityCommitError::Busy),
            "an active attempt lease keeps the commit busy"
        );
        drop(lease);
        let candidate = coordinator
            .prepare_candidate()
            .await
            .expect("re-prepare after release");
        let new_snapshot = coordinator.commit(candidate).expect("commit");
        assert_eq!(new_snapshot.revision().get(), 1);
        // The in-flight attempt that held the old lease continues using its
        // old registry; the commit never mutated it in place.
        assert_ne!(
            Arc::as_ptr(&old_registry),
            Arc::as_ptr(new_snapshot.tool_registry()),
            "the committed snapshot owns a fresh immutable registry"
        );
        assert!(old_registry.definitions().is_empty());
        assert!(!new_snapshot.tool_registry().definitions().is_empty());
        coordinator
            .inner
            .mcp_runtimes
            .lock()
            .await
            .get(&server_id)
            .expect("runtime")
            .close()
            .await
            .expect("the owned stdio unit must publish physical settlement");
    }

    /// The invalidation state itself is constructible and its epoch
    /// mutation is monotonic under the guard.
    #[test]
    fn mcp_invalidation_epochs_are_monotonic_under_the_shared_guard() {
        let state = Arc::new(McpInvalidationState::new());
        let server = McpServerId::new("s");
        assert_eq!(state.epoch(&server), 0);
        {
            let mut guard = state.lock();
            guard.advance(&server);
            guard.advance(&server);
            assert_eq!(guard.epoch(&server), 2);
        }
        assert_eq!(state.epoch(&server), 2);
    }
}
