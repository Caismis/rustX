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
use crate::tools::mcp::{
    McpInvalidationState, McpRuntimeGeneration, McpRuntimeLeaseAuthority, McpRuntimeLeaseSet,
    McpRuntimeRetirementRegistry, McpServerBindings, McpServerRuntime,
};
use crate::tools::python::{PythonToolStore, discover_python_packages};
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
    /// The currently published physical MCP generation owners. These owners
    /// are replaced only at the capability commit linearization point; each
    /// retired owner closes after its explicit attempt/background leases
    /// settle.
    mcp_runtimes: Vec<McpRuntimeGeneration>,
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
    resource_inputs: Mutex<CapabilityResourceInputs>,
    /// The ownership cancellation root of every in-flight conversation-owned
    /// preparation: MCP connects (Issue #12, M9c) and Python/uv environment
    /// builds (Issue #145).
    ///
    /// Each in-flight preparation owner takes a child of this signal, so
    /// runtime drain can close in-flight *preparation* the same way it
    /// closes retained runtimes: by cancelling the owner, never by dropping
    /// a caller future. Cancelling drives an already-spawned stdio process
    /// or uv unit to its physical settlement proof before the owner
    /// releases its counted lifecycle admission.
    preparation_cancellation: crate::runtime::cancellation::CancellationSignal,
    /// Test-only: parks the next conversation-owned MCP connect at the
    /// instant physical process ownership exists.
    #[cfg(test)]
    connect_ownership_pause:
        Mutex<Option<Arc<crate::tools::mcp::test_sync::ConnectOwnershipPause>>>,
    /// The one shared MCP invalidation synchronization boundary: epoch
    /// mutation (`tools/list_changed`) and epoch validation + snapshot swap
    /// (commit) serialize through the same guard.
    mcp_invalidation: Arc<McpInvalidationState>,
    /// The configured location of the managed Python package store
    /// (`<environment store>/python-tools`).
    ///
    /// The coordinator owns the *location* only. Opening/creating the
    /// Python-specific storage is part of the optional managed-package
    /// preparation ([`CapabilityCoordinator::prepare_python_packages`]), so
    /// a Python storage failure degrades the discovered packages'
    /// availability and can never fail core coordinator construction — and
    /// a base-only/subagent coordinator never touches Python storage at
    /// all.
    python_store_root: PathBuf,
    /// The lazily initialized, coordinator-lifetime-stable Python package
    /// store (Issue #81).
    ///
    /// Initialization timing is not lifetime ownership: the slot starts
    /// empty because Python packages are optional and their storage may
    /// fail, a failed initialization leaves the slot empty so the next
    /// preparation retries, and the first *successful* initialization
    /// publishes the one stable process-local store identity every later
    /// preparation reuses. The store owns the process-local build
    /// coalescing domain, so it must never be reconstructed per
    /// preparation.
    ///
    /// The mutex is held only across the synchronous store construction
    /// (a bounded `create_dir_all` sequence) — never across `.await`.
    python_store: Mutex<Option<PythonToolStore>>,
    environment_store: EnvironmentStore,
    state: Mutex<CoordinatorState>,
    condvar: Condvar,
    /// Retired candidate/publication generations whose asynchronous physical
    /// close has not yet been reaped. This field follows the published state
    /// so current generation owners retire while the registry is still alive
    /// if the coordinator itself is dropped after normal runtime shutdown.
    mcp_retirements: Arc<McpRuntimeRetirementRegistry>,
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

/// One materialized capability: its exact definition and its executor.
type MaterializedTool = (
    crate::tools::types::ToolDefinition,
    Arc<dyn crate::tools::executor::ToolExecutor>,
);

/// Verifies and materializes exactly the selected tools of one connected
/// MCP server (Issue #145).
///
/// The verification is the whole point of the cross-process identity: the
/// child derives the canonical identity of what the server *actually*
/// publishes right now and compares it to what its parent generation froze.
/// A missing tool and a changed tool are both refusals, never silent
/// omissions and never silent substitutions.
fn select_verified_mcp_tools(
    server_id: &McpServerId,
    binding: &crate::tools::mcp::McpServerBinding,
    generation: &McpRuntimeGeneration,
    published: &[crate::tools::mcp::CanonicalMcpTool],
    selected: &[&crate::capabilities::selected::SelectedMcpTool],
) -> Result<Vec<MaterializedTool>, crate::capabilities::selected::SelectedMaterializationError> {
    use crate::capabilities::selected::SelectedMaterializationError;
    let mut materialized = Vec::with_capacity(selected.len());
    for wanted in selected {
        let Some(candidate) = published.iter().find(|tool| tool.name == wanted.name) else {
            return Err(SelectedMaterializationError::McpToolMissing {
                server_id: server_id.clone(),
                name: wanted.name.clone(),
            });
        };
        let observed = crate::tools::mcp::identity::mcp_tool_identity(
            server_id,
            &candidate.name,
            &candidate.description,
            &candidate.input_schema,
            binding.policy.execution,
            binding.policy.concurrency,
            binding.policy.approval,
        );
        if observed != wanted.identity {
            return Err(SelectedMaterializationError::McpIdentityMismatch {
                server_id: server_id.clone(),
                name: wanted.name.clone(),
                expected: wanted.identity.clone(),
                observed,
            });
        }
        // The executor is constructed only after verification, from a
        // child-owned runtime binding. No parent executor, lease, transport
        // handle, or process-local epoch is ever copied across the process
        // boundary.
        materialized.extend(crate::tools::mcp::definitions_owned(
            server_id,
            binding.policy,
            &generation.binding(),
            vec![candidate.clone()],
        ));
    }
    Ok(materialized)
}

/// Retires every MCP runtime a failed selected-only preparation connected.
///
/// A candidate is only "owned" once it commits; before that, the preparation
/// itself owns the physical runtimes it created and must settle them on
/// every failure path rather than dropping the handles.
async fn retire_candidate_runtimes(runtimes: Vec<McpRuntimeGeneration>) {
    for generation in runtimes {
        let _: Option<String> = generation.retire_and_close().await;
    }
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

/// The unforgeable capability-publication authority held by a live
/// `ConversationRuntime`. A runtime resource reload must present this token
/// to advance the capability generation; ordinary callers only retain the
/// standalone coordinator commit API.
pub(crate) struct RuntimeCapabilityPublication {
    coordinator: Arc<CoordinatorInner>,
}

/// Complete reloadable capability inputs owned by one runtime resource
/// generation. The coordinator publishes these only with the prepared
/// snapshot derived from them.
#[derive(Debug, Clone)]
pub struct CapabilityResourceInputs {
    /// Native/extension Tool registrations.
    pub base_tool_registry: Arc<ToolRegistry>,
    /// Effective Tool activation policy.
    pub tool_activation: ToolActivationPolicy,
    /// Skill discovery roots and explicit sources.
    pub skill_discovery: SkillDiscoveryConfig,
    /// Configured MCP sources.
    pub mcp_servers: McpServerBindings,
    /// Base authorized environment before Skill overlays.
    pub base_environment: ToolEnvironment,
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
///
/// **Exception.** A commit made by the claiming `ConversationRuntime`
/// (see [`CapabilityCoordinator::commit_runtime`]) fires no callback at
/// all. That commit is one half of a runtime resource reload, and the
/// runtime publishes the whole generation — capability, availability, and
/// resources — as a single observation. Firing here as well would let a
/// consumer fold the capability half on its own and briefly present a
/// generation that never existed.
pub trait CapabilityObserver: Send + Sync {
    /// Observes one activated immutable capability snapshot and its
    /// authoritative availability state.
    fn on_snapshot(&self, snapshot: &CapabilitySnapshot, availability: &CapabilityAvailability);
}

/// One committed capability generation, returned to a caller that owns its
/// publication.
///
/// The pair is exactly what the observer callback would have carried. It is
/// handed back instead of fired for
/// [`CapabilityCoordinator::commit_runtime`], whose caller must publish it
/// together with the matching resource generation.
#[derive(Debug, Clone)]
pub(crate) struct CommittedCapability {
    /// The activated immutable capability snapshot.
    pub(crate) snapshot: Arc<CapabilitySnapshot>,
    /// The authoritative per-source availability state at that commit.
    pub(crate) availability: CapabilityAvailability,
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
    /// Physical MCP runtimes prepared for this candidate. They are owned by
    /// the candidate until `commit` transfers them into the coordinator's
    /// published generation state.
    mcp_runtimes: Vec<McpRuntimeGeneration>,
    /// The effective MCP server set of this candidate: the configured
    /// bindings plus the synthesized managed-Python-package bindings (Issue
    /// #174). The committed snapshot freezes this set for the subagent
    /// materialization crossing; it is deliberately separate from
    /// `resource_inputs`, which carries the *configured* inputs that commit
    /// publishes back as the coordinator's authoritative reload state.
    effective_mcp_servers: crate::tools::mcp::McpServerBindings,
    resource_inputs: CapabilityResourceInputs,
    /// Explicit runtime-resource reload must publish the candidate registry
    /// even when model-facing definitions are byte-identical: executor
    /// configuration is not represented by those definitions.
    force_publish: bool,
}

impl PreparedCapabilityCandidate {
    /// Settles every physical runtime of a prepared candidate that will
    /// never commit (Issue #145): pre-commit cancellation won the race
    /// against this candidate's completion, so the preparation that owns
    /// these runtimes retires them to physical settlement rather than
    /// dropping the handles.
    pub(crate) async fn retire_uncommitted(self) {
        retire_candidate_runtimes(self.mcp_runtimes).await;
    }

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

    /// Installs a deterministic close probe on every MCP runtime prepared by
    /// this candidate. Test loaders use this before deliberately failing
    /// after capability preparation, so candidate-only physical cleanup is
    /// observable without exposing ownership internals to production code.
    #[cfg(test)]
    pub(crate) fn install_mcp_close_probe(
        &self,
        probe: &Arc<crate::tools::mcp::test_sync::CloseProbe>,
    ) {
        for generation in &self.mcp_runtimes {
            generation.runtime().install_close_probe(probe.clone());
        }
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
        // itself is opened inside the optional managed-package preparation
        // boundary (Issue #81), never in core construction.
        let python_store_root = environment_store.root().join("python-tools");
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
            Arc::new(McpRuntimeLeaseAuthority::empty()),
            Arc::new(mcp_servers.clone()),
        ));
        Ok(Self {
            inner: Arc::new(CoordinatorInner {
                conversation_id: config.conversation_id,
                workspace: config.workspace,
                resource_inputs: Mutex::new(CapabilityResourceInputs {
                    base_tool_registry: config.base_tool_registry,
                    tool_activation,
                    skill_discovery,
                    mcp_servers,
                    base_environment: config.base_environment.clone(),
                }),
                preparation_cancellation: crate::runtime::cancellation::CancellationSignal::new(),
                #[cfg(test)]
                connect_ownership_pause: Mutex::new(None),
                mcp_invalidation: Arc::new(McpInvalidationState::new()),
                python_store_root,
                python_store: Mutex::new(None),
                environment_store,
                state: Mutex::new(CoordinatorState {
                    revision: CapabilityRevision::default(),
                    snapshot: initial_snapshot,
                    availability: CapabilityAvailability::new(),
                    mcp_runtimes: Vec::new(),
                    active_attempts: 0,
                    _next_staging: AtomicU64::new(0),
                    conversation_lifecycle: None,
                }),
                condvar: Condvar::new(),
                mcp_retirements: McpRuntimeRetirementRegistry::new(),
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
    /// Returns the one publication authority for the claim that wins and
    /// `None` for every later claim on any clone. Never reset by dropping the
    /// bound coordinator.
    ///
    /// The claim and the lifecycle attachment share the capability state
    /// lock — the same boundary `commit` reads them under — so a
    /// runtime-owned `commit` can never observe a claimed coordinator
    /// without its lifecycle. A standalone (unclaimed) coordinator keeps no
    /// lifecycle and commits unconditionally.
    pub(crate) fn claim_conversation_runtime(
        &self,
        lifecycle: &ConversationLifecycle,
    ) -> Option<RuntimeCapabilityPublication> {
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
            return None;
        }
        state.conversation_lifecycle = Some(lifecycle.clone());
        drop(state);
        Some(RuntimeCapabilityPublication {
            coordinator: self.inner.clone(),
        })
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
        let inputs = self
            .inner
            .resource_inputs
            .lock()
            .expect("capability resource-input lock poisoned")
            .clone();
        self.prepare_candidate_from_inputs(inputs, false).await
    }

    /// Prepares a complete candidate from explicit reload-time inputs.
    pub(crate) async fn prepare_candidate_with_inputs(
        &self,
        inputs: CapabilityResourceInputs,
    ) -> Result<PreparedCapabilityCandidate, CapabilityPreparationError> {
        self.prepare_candidate_from_inputs(inputs, true).await
    }

    #[allow(clippy::too_many_lines)]
    async fn prepare_candidate_from_inputs(
        &self,
        inputs: CapabilityResourceInputs,
        force_publish: bool,
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
            SkillDiscovery::with_config(&self.inner.workspace, inputs.skill_discovery.clone())
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
        let effective_environment = inputs.base_environment.with_overlay(&overlay);
        let skills = Arc::new(SkillSnapshot::new(
            packages.into_iter().map(Arc::new).collect(),
        ));
        // ---- Optional capability sources (Issue #81) ----
        //
        // Each optional source is prepared in isolation: its failure is
        // recorded as its own typed availability state, never as a
        // preparation error of the whole candidate.
        let mut availability = CapabilityAvailability::new();
        // ---- Managed Python tool packages (Issue #174) ----
        //
        // Each discovered package is prepared into its isolated uv
        // environment and compiled into one synthesized MCP server binding.
        // The bindings merge into the set the generic MCP path below
        // iterates — connect, tools/list, epoch checks, availability,
        // commit, leases, and the frozen snapshot are exactly the generic
        // machinery — so a managed package is never a second protocol.
        // A package that fails discovery or preparation records its own
        // synthesized source as unavailable and contributes nothing.
        //
        // The merge is deliberately NOT a mutation of `inputs`: the
        // candidate's `resource_inputs` are the *configured* inputs (commit
        // publishes them back as the coordinator's authoritative reload
        // state), while the synthesized bindings are re-derived from
        // workspace discovery on every preparation. Persisting the merged
        // set would make the next preparation collide with its own earlier
        // synthesis.
        let mut effective_mcp_servers = inputs.mcp_servers.clone();
        let mut rejected_sources: Vec<(McpServerId, String)> = Vec::new();
        for (server_id, outcome) in self.prepare_python_packages().await? {
            match outcome {
                Ok(binding) => match effective_mcp_servers.entry(server_id.clone()) {
                    std::collections::btree_map::Entry::Vacant(slot) => {
                        slot.insert(binding);
                    }
                    std::collections::btree_map::Entry::Occupied(_) => {
                        // Structurally unreachable: configured `mcpServers`
                        // entries are rejected at configuration validation
                        // when they claim the reserved `python:` namespace
                        // (see `mcp_bindings`), and discovery yields at
                        // most one package per folder, so one `McpServerId`
                        // can never have two owners. A collision here is an
                        // internal invariant violation, never a supported
                        // runtime state — there is no precedence rule and no
                        // second semantic path.
                        return Err(CapabilityPreparationError::Mcp(format!(
                            "internal invariant violated: MCP server identity {server_id} \
                             already has an owner"
                        )));
                    }
                },
                Err(reason) => rejected_sources.push((server_id, reason)),
            }
        }
        let mut discovered_tools = Vec::<ToolRegistration>::new();
        discovered_tools.extend(inputs.base_tool_registry.registrations());
        let mut mcp_tools = Vec::new();
        let mut mcp_epochs = BTreeMap::new();
        let mut mcp_runtimes = Vec::new();
        // `BTreeMap` iteration is the deterministic identity order.
        for (server_id, binding) in &effective_mcp_servers {
            match self.prepare_mcp_server(server_id, binding, None).await {
                Ok((epoch, generation, tools)) => {
                    mcp_epochs.insert(server_id.clone(), epoch);
                    mcp_tools.extend(crate::tools::mcp::definitions_owned(
                        server_id,
                        binding.policy,
                        &generation.binding(),
                        tools,
                    ));
                    mcp_runtimes.push(generation);
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
        // Package-local failures land on the package's own synthesized
        // source, after the generic loop so a sibling failure can never
        // silently overwrite another source's rejection diagnostic.
        for (server_id, reason) in rejected_sources {
            availability.insert(
                CapabilitySourceId::Mcp(server_id),
                CapabilitySourceState::unavailable(reason),
            );
        }
        discovered_tools.extend(
            mcp_tools
                .into_iter()
                .map(|(definition, executor)| ToolRegistration::plain(definition, executor)),
        );
        let (available_tools, candidate_registry) =
            select_tools(&discovered_tools, &inputs.tool_activation)
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
            mcp_runtimes,
            effective_mcp_servers,
            resource_inputs: inputs,
            force_publish,
        })
    }

    /// Discovers and prepares the managed Python tool packages of the
    /// Workspace, returning one synthesized MCP server identity per package
    /// folder with either its prepared launch binding or its rejection
    /// diagnostic (Issue #174).
    ///
    /// Every package is its own optional capability source: a failure —
    /// including opening/creating the Python store itself — rejects only
    /// the packages that depend on it (the caller records each synthesized
    /// source unavailable), so Python storage can never fail core
    /// coordinator construction. Only walking the `.agents/tools/`
    /// container itself is a candidate-level preparation error, the same
    /// layering Skill discovery already has.
    async fn prepare_python_packages(
        &self,
    ) -> Result<
        Vec<(
            McpServerId,
            Result<crate::tools::mcp::McpServerBinding, String>,
        )>,
        CapabilityPreparationError,
    > {
        let discovered = discover_python_packages(&self.inner.workspace).map_err(|error| {
            CapabilityPreparationError::Mcp(format!(
                "Python tool package discovery failed: {error}"
            ))
        })?;
        if discovered.is_empty() {
            return Ok(Vec::new());
        }
        let store = match self.python_store() {
            Ok(store) => store,
            Err(error) => {
                // The store is shared preparation infrastructure: its
                // failure rejects every discovered package with the same
                // diagnostic, but nothing else.
                let reason = error.to_string();
                return Ok(discovered
                    .into_iter()
                    .map(|entry| {
                        let outcome = match entry.outcome {
                            Ok(_) => Err(reason.clone()),
                            Err(error) => Err(error.to_string()),
                        };
                        (entry.server_id, outcome)
                    })
                    .collect());
            }
        };
        let mut bindings = Vec::with_capacity(discovered.len());
        for entry in discovered {
            let outcome = match entry.outcome {
                Err(error) => Err(error.to_string()),
                Ok(package) => match store
                    .ensure_prepared(&package, &self.inner.preparation_cancellation.child())
                    .await
                {
                    Ok(prepared) => Ok(prepared.server_binding()),
                    Err(error) => Err(error.to_string()),
                },
            };
            bindings.push((entry.server_id, outcome));
        }
        Ok(bindings)
    }

    /// Returns the coordinator-lifetime-stable Python package store,
    /// initializing it on first use.
    ///
    /// The slot starts empty because Python packages are optional: a
    /// construction failure leaves it empty (the caller records the
    /// discovered packages unavailable and the next preparation retries);
    /// the first successful construction is published into the slot under
    /// the mutex, so concurrent first preparations converge to exactly one
    /// store identity — the single process-local coordination domain for
    /// environment build coalescing. The mutex is never held across
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

    /// Prepares one configured MCP server: connect a candidate-owned runtime,
    /// snapshot the invalidation epoch, and fetch its complete catalog under
    /// the epoch-stability check. The physical runtime is not visible in the
    /// coordinator's published state until the complete candidate commits.
    ///
    /// Each server is one optional capability source: the caller records a
    /// failure as that server's own unavailable state and continues with
    /// the remaining servers.
    ///
    /// `preparation_cancellation` is `Some` only on the selected-only child
    /// realization path (Issue #145): the child preparation's lifecycle
    /// authority, which the physical connect owner must observe in addition
    /// to this coordinator's own preparation root.
    async fn prepare_mcp_server(
        &self,
        server_id: &McpServerId,
        binding: &crate::tools::mcp::McpServerBinding,
        preparation_cancellation: Option<&crate::runtime::cancellation::CancellationSignal>,
    ) -> Result<
        (
            u64,
            McpRuntimeGeneration,
            Vec<crate::tools::mcp::CanonicalMcpTool>,
        ),
        String,
    > {
        let generation = self
            .connect_conversation_owned(server_id, binding, preparation_cancellation)
            .await
            .map_err(|error| error.to_string())?;
        // The epoch snapshot is taken under the shared invalidation
        // guard; the pagination itself never holds it.
        let epoch_before = self.inner.mcp_invalidation.epoch(server_id);
        let tools = match generation.binding().runtime().list_tools().await {
            Ok(tools) => tools,
            Err(error) => {
                let close_error = generation.retire_and_close().await;
                return Err(match close_error {
                    Some(close_error) => format!("{error}; candidate close failed: {close_error}"),
                    None => error.to_string(),
                });
            }
        };
        let epoch_after = self.inner.mcp_invalidation.epoch(server_id);
        if epoch_before != epoch_after {
            let close_error = generation.retire_and_close().await;
            return Err(match close_error {
                Some(close_error) => format!(
                    "MCP tool catalog changed during discovery; candidate close failed: {close_error}"
                ),
                None => "MCP tool catalog changed during discovery".to_owned(),
            });
        }
        Ok((epoch_after, generation, tools))
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
        let inputs = self
            .inner
            .resource_inputs
            .lock()
            .expect("capability resource-input lock poisoned")
            .clone();
        let base_registrations = inputs.base_tool_registry.registrations();
        let (available_tools, candidate_registry) =
            select_tools(&base_registrations, &inputs.tool_activation)
                .map_err(CapabilityPreparationError::ToolActivation)?;
        let candidate_registry = Arc::new(candidate_registry);
        Ok(PreparedCapabilityCandidate {
            base_revision,
            skills: Arc::new(SkillSnapshot::new(Vec::new())),
            python: None,
            node: None,
            effective_environment: inputs.base_environment.clone(),
            candidate_registry,
            available_tools: Arc::new(available_tools),
            mcp_epochs: BTreeMap::new(),
            availability: CapabilityAvailability::new(),
            mcp_runtimes: Vec::new(),
            // The base-only candidate connects nothing; its effective server
            // set is the configured one, exactly as before.
            effective_mcp_servers: inputs.mcp_servers.clone(),
            resource_inputs: inputs,
            force_publish: false,
        })
    }

    /// Prepares the **selected-only** candidate of a subagent child runtime
    /// (Issue #145).
    ///
    /// This is the physical realization of a frozen
    /// `ResolvedSubagentSpec`, and it is deliberately *not* the parent's
    /// discovery pipeline with a filter bolted on:
    ///
    /// ```text
    /// no Skill discovery            the child's Skills are already frozen
    ///                               and materialized by its composition
    /// no workspace Python discovery managed Python packages cross as
    ///                               ordinary frozen MCP bindings
    /// no "every configured server"  only the servers the selection names
    ///                               are connected at all
    /// no activation policy pass     the frozen set IS the active set
    /// ```
    ///
    /// Each externally sourced capability is verified against the identity
    /// the parent generation froze before it becomes executable, and a
    /// failure is a **preparation error**, never an availability
    /// degradation: a child starts with exactly the authority it was given
    /// or it does not start.
    ///
    /// # Errors
    ///
    /// Returns [`CapabilityPreparationError::ConversationInactive`] when the
    /// claiming conversation is draining,
    /// [`CapabilityPreparationError::Mcp`] when a required server cannot be
    /// connected or listed,
    /// [`CapabilityPreparationError::SelectedMaterialization`] when a frozen
    /// identity cannot be reproduced, and
    /// [`CapabilityPreparationError::ToolActivation`] when the composed
    /// registry is invalid.
    ///
    /// # Panics
    ///
    /// Panics only if the coordinator state lock is poisoned.
    #[allow(clippy::too_many_lines)] // one coherent selected-only realization pipeline
    pub async fn prepare_selected_candidate(
        &self,
        plan: &crate::capabilities::selected::SelectedCapabilityPlan,
        preparation_cancellation: &crate::runtime::cancellation::CancellationSignal,
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
        let inputs = self
            .inner
            .resource_inputs
            .lock()
            .expect("capability resource-input lock poisoned")
            .clone();

        // The frozen Builtin registrations, exactly as the parent admitted
        // them. Deny by construction: this registry was composed from the
        // frozen `ToolDefinition`s alone.
        let mut registrations: Vec<ToolRegistration> = inputs.base_tool_registry.registrations();

        // ---- MCP: connect only the required servers ----
        //
        // `inputs.mcp_servers` is already exactly the frozen selected set —
        // the child's composition never learns about any other server — so
        // "connect only what is required" is structural here, not a filter.
        let mut mcp_runtimes: Vec<McpRuntimeGeneration> = Vec::new();
        let mut mcp_epochs = BTreeMap::new();
        let required = plan.required_mcp_servers();
        for server_id in &required {
            let Some(binding) = inputs.mcp_servers.get(server_id) else {
                return Err(CapabilityPreparationError::Mcp(format!(
                    "the frozen specification requires MCP server {server_id}, which this \
                     child was not given a binding for"
                )));
            };
            let (epoch, generation, tools) = match self
                .prepare_mcp_server(server_id, binding, Some(preparation_cancellation))
                .await
            {
                Ok(prepared) => prepared,
                Err(reason) => {
                    // A required source is not optional. Retire everything
                    // already connected before failing, so no MCP process
                    // survives a failed child preparation.
                    retire_candidate_runtimes(mcp_runtimes).await;
                    return Err(CapabilityPreparationError::Mcp(reason));
                }
            };
            let selected: Vec<_> = plan
                .mcp_tools
                .iter()
                .filter(|tool| tool.server_id == *server_id)
                .collect();
            match select_verified_mcp_tools(server_id, binding, &generation, &tools, &selected) {
                Ok(definitions) => {
                    mcp_epochs.insert(server_id.clone(), epoch);
                    registrations.extend(definitions.into_iter().map(|(definition, executor)| {
                        ToolRegistration::plain(definition, executor)
                    }));
                    mcp_runtimes.push(generation);
                }
                Err(error) => {
                    mcp_runtimes.push(generation);
                    retire_candidate_runtimes(mcp_runtimes).await;
                    return Err(error.into());
                }
            }
        }

        // The frozen set IS the active set: activation policy has nothing
        // left to decide, so the default (activate everything composed) is
        // exactly the authorized projection.
        let (available_tools, candidate_registry) =
            match select_tools(&registrations, &ToolActivationPolicy::default()) {
                Ok(selected) => selected,
                Err(error) => {
                    retire_candidate_runtimes(mcp_runtimes).await;
                    return Err(CapabilityPreparationError::ToolActivation(error));
                }
            };
        Ok(PreparedCapabilityCandidate {
            base_revision,
            skills: Arc::new(SkillSnapshot::new(Vec::new())),
            python: None,
            node: None,
            effective_environment: inputs.base_environment.clone(),
            candidate_registry: Arc::new(candidate_registry),
            available_tools: Arc::new(available_tools),
            mcp_epochs,
            availability: CapabilityAvailability::new(),
            mcp_runtimes,
            // The frozen selected set IS the effective set: the child's
            // composition never learns about any other server.
            effective_mcp_servers: inputs.mcp_servers.clone(),
            resource_inputs: inputs,
            force_publish: false,
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
    ///   -> candidate-owned generation
    ///   -> either  A. transferred into published generation state
    ///      or      B. retired and driven to physical settlement
    /// ```
    ///
    /// The counted admission is released only after A or B, so aborting or
    /// dropping *this* future never removes the physical owner from the
    /// conversation's quiescence proof. The waiter is not the owner.
    ///
    /// `preparation_cancellation` is an additional lifecycle authority the
    /// physical connect must observe (the child preparation's pre-commit
    /// authority, Issue #145). It is forwarded into the owner's own
    /// cancellation signal: once fired, the owner drives its physical
    /// process to settlement before answering, exactly as it does for the
    /// coordinator's preparation root.
    async fn connect_conversation_owned(
        &self,
        server_id: &McpServerId,
        binding: &crate::tools::mcp::McpServerBinding,
        preparation_cancellation: Option<&crate::runtime::cancellation::CancellationSignal>,
    ) -> Result<McpRuntimeGeneration, CapabilityPreparationError> {
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
        let cancellation = self.inner.preparation_cancellation.child();
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
        let preparation_cancellation = preparation_cancellation.cloned();
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        let handle = tokio::runtime::Handle::current();
        tokio::spawn(async move {
            let request = crate::tools::mcp::OwnedConnect::new(
                &server_id_owned,
                &binding_owned,
                &inner.workspace,
                inner.mcp_invalidation.clone(),
                cancellation.clone(),
            );
            #[cfg(test)]
            let request = request.with_ownership_pause(ownership_pause);
            let outcome = {
                let mut connect = std::pin::pin!(McpServerRuntime::connect_owned(request));
                match preparation_cancellation {
                    // The child preparation's authority fires into the owner's
                    // own cancellation signal; the connect then drives its
                    // physical process to settlement before answering, exactly
                    // as it does for the coordinator's preparation root.
                    Some(authority) => {
                        tokio::select! {
                            outcome = &mut connect => outcome,
                            () = authority.cancelled() => {
                                cancellation.cancel();
                                connect.await
                            }
                        }
                    }
                    None => connect.await,
                }
            };
            let outcome = match outcome {
                // The preparation owner transfers its lifecycle admission into
                // the candidate generation before answering the caller. If
                // the caller has already been cancelled, the send-failure
                // branch below retires that generation and awaits its close.
                Ok(runtime) => Ok(McpRuntimeGeneration::from_connected(
                    server_id_owned,
                    runtime,
                    owner_admission,
                    handle,
                    &inner.mcp_retirements,
                )),
                // Phase B: the connect owner already drove its physical
                // process (when one existed) to settlement before returning.
                Err(error) => Err(CapabilityPreparationError::Mcp(error.to_string())),
            };
            if let Err(outcome) = result_tx.send(outcome)
                && let Ok(generation) = outcome
            {
                let _ = generation.retire_and_close().await;
            }
        });
        result_rx.await.unwrap_or_else(|_| {
            Err(CapabilityPreparationError::Mcp(
                "the MCP connection owner terminated without an outcome".to_owned(),
            ))
        })
    }

    /// Requests cancellation of every in-flight conversation-owned
    /// preparation owner (Issue #12, M9c; Issue #145).
    ///
    /// This is a synchronous non-blocking control operation taken by the
    /// runtime drain transition and by a settled child preparation. It
    /// never waits: each owner settles its own physical process and only
    /// then releases the counted lifecycle admission that quiescence waits
    /// on.
    pub(crate) fn cancel_conversation_preparation(&self) {
        self.inner.preparation_cancellation.cancel();
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
        self.retire_current_mcp_runtimes();
        let failures = self.inner.mcp_retirements.settle_all().await;
        if failures.is_empty() {
            Ok(())
        } else {
            Err(failures)
        }
    }

    /// Retires the currently published MCP generation owners. This is called
    /// at runtime drain before lifecycle quiescence is attempted, so the
    /// current generation's physical-owner admission can settle rather than
    /// becoming a self-waiting drain dependency.
    pub(crate) fn retire_current_mcp_runtimes(&self) {
        let state = self
            .inner
            .state
            .lock()
            .expect("capability state lock poisoned");
        for generation in &state.mcp_runtimes {
            generation.retire();
        }
    }

    /// Reaps retired MCP generations that have no legitimate execution
    /// owners. A live detached background owner is intentionally left in the
    /// retirement registry until it settles.
    pub(crate) async fn settle_ready_mcp_runtimes(&self) -> Result<(), Vec<String>> {
        self.inner.mcp_retirements.settle_ready().await
    }

    /// Installs the runtime-owned seam that fences healthy continuation when
    /// a retired MCP generation cannot prove physical settlement.
    pub(crate) fn install_mcp_retirement_failure_callback(
        &self,
        callback: &Arc<dyn Fn(String) + Send + Sync>,
    ) {
        self.inner
            .mcp_retirements
            .install_failure_callback(callback);
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
    /// Once a `ConversationRuntime` owns this coordinator, this public
    /// standalone commit path is closed permanently. Live capability
    /// mutation must be presented by the runtime's private publication
    /// authority so the matching `RuntimeResourceSnapshot` is published at
    /// the same runtime boundary. A coordinator that has not been claimed by
    /// a runtime retains this independent prepare/commit API for composition
    /// and capability-layer use.
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
    /// Returns [`CapabilityCommitError::RuntimePublicationRequired`] after
    /// the coordinator has been claimed by a conversation runtime,
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
        self.commit_with_authority(candidate, None)
            .map(|committed| committed.snapshot)
    }

    /// Publishes a prepared capability candidate for the claiming
    /// `ConversationRuntime`. The token is created only by
    /// [`Self::claim_conversation_runtime`] and is not exposed to ordinary
    /// coordinator callers.
    ///
    /// **This path deliberately fires no capability observation.** A
    /// runtime-owned commit is one half of a larger fact: the runtime
    /// publishes a new capability generation and a new resource generation
    /// together, and a consumer that sees the first without the second sees
    /// a generation that never existed — new tools alongside retired
    /// project instruction files. The committed pair is returned so the
    /// runtime can publish the complete generation as one observation.
    /// Every other commit path still observes itself.
    pub(crate) fn commit_runtime(
        &self,
        publication: &RuntimeCapabilityPublication,
        candidate: PreparedCapabilityCandidate,
    ) -> Result<CommittedCapability, CapabilityCommitError> {
        self.commit_with_authority(candidate, Some(publication))
    }

    #[allow(clippy::too_many_lines)]
    fn commit_with_authority(
        &self,
        mut candidate: PreparedCapabilityCandidate,
        publication: Option<&RuntimeCapabilityPublication>,
    ) -> Result<CommittedCapability, CapabilityCommitError> {
        let mut state = self
            .inner
            .state
            .lock()
            .expect("capability state lock poisoned");
        match publication {
            None if state.conversation_lifecycle.is_some() => {
                return Err(CapabilityCommitError::RuntimePublicationRequired);
            }
            Some(publication)
                if !Arc::ptr_eq(&self.inner, &publication.coordinator)
                    || state.conversation_lifecycle.is_none() =>
            {
                return Err(CapabilityCommitError::RuntimePublicationRequired);
            }
            _ => {}
        }
        // Hold the shared lifecycle admission guard across the capability
        // state lock, MCP validation, snapshot swap, and observer callback.
        // The final revision swap below also takes the lifecycle commit
        // boundary, so drain and this publication have one exact order at
        // the authoritative revision point. Standalone coordinators have no
        // conversation lifecycle and retain their independent semantics.
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
        let candidate_inputs = candidate.resource_inputs.clone();
        // A runtime-owned commit hands its observation to the runtime,
        // which publishes it together with the resource generation of the
        // same reload. See `commit_runtime`.
        let defers_observation = publication.is_some();
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
                *self
                    .inner
                    .resource_inputs
                    .lock()
                    .expect("capability resource-input lock poisoned") = candidate_inputs.clone();
                let availability_changed =
                    Self::install_availability(&mut state, &candidate.availability);
                let snapshot = state.snapshot.clone();
                let availability = state.availability.clone();
                drop(invalidation);
                if availability_changed && !defers_observation {
                    // An availability-only change never advances the
                    // revision, but it is still observed.
                    Self::fire_observer(&self.inner.observer, &snapshot, &availability);
                }
                return Ok(CommittedCapability {
                    snapshot,
                    availability,
                });
            }
            Self::install_availability(&mut state, &candidate.availability);
            let revision = CapabilityRevision::new(state.revision.get() + 1);
            let mcp_lease_authority = Arc::new(McpRuntimeLeaseAuthority::from_generations(
                &candidate.mcp_runtimes,
            ));
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
                mcp_lease_authority,
                Arc::new(candidate.effective_mcp_servers.clone()),
            ));
            let previous_mcp_runtimes = std::mem::replace(
                &mut state.mcp_runtimes,
                std::mem::take(&mut candidate.mcp_runtimes),
            );
            for generation in previous_mcp_runtimes {
                generation.retire();
            }
            state.revision = revision;
            state.snapshot = snapshot.clone();
            *self
                .inner
                .resource_inputs
                .lock()
                .expect("capability resource-input lock poisoned") = candidate_inputs.clone();
            let availability = state.availability.clone();
            drop(invalidation);
            if !defers_observation {
                Self::fire_observer(&self.inner.observer, &snapshot, &availability);
            }
            Ok(CommittedCapability {
                snapshot,
                availability,
            })
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
        let snapshot = state.snapshot.clone();
        state.active_attempts += 1;
        AttemptCapabilityLease {
            inner: self.inner.clone(),
            mcp_leases: snapshot
                .acquire_mcp_leases()
                .expect("a current MCP generation must accept an attempt lease"),
            snapshot,
        }
    }

    /// Pins the capability snapshot already paired with an admitted runtime
    /// resource generation. The runtime admission lock selects the pair;
    /// this boundary only accounts the attempt lease.
    pub(crate) fn acquire_attempt_lease_for(
        &self,
        snapshot: Arc<CapabilitySnapshot>,
    ) -> AttemptCapabilityLease {
        assert_eq!(snapshot.conversation_id(), &self.inner.conversation_id);
        assert_eq!(snapshot.workspace_root(), self.inner.workspace.root());
        let mut state = self
            .inner
            .state
            .lock()
            .expect("capability state lock poisoned");
        state.active_attempts += 1;
        let mcp_leases = snapshot
            .acquire_mcp_leases()
            .expect("an admitted MCP generation must accept an attempt lease");
        AttemptCapabilityLease {
            inner: self.inner.clone(),
            snapshot,
            mcp_leases,
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

    /// Installs a pre-built Python tool store into the lazy slot, replacing
    /// the production construction. The deterministic scripted suites use
    /// this to give the coordinator a store backed by a recorded process
    /// runner instead of the real uv. Only available under `#[cfg(test)]`;
    /// never used by production code.
    #[cfg(test)]
    pub(crate) fn install_python_store(&self, store: PythonToolStore) {
        *self
            .inner
            .python_store
            .lock()
            .expect("python store lock poisoned") = Some(store);
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

    /// Returns one published MCP runtime for deterministic ownership tests.
    #[cfg(test)]
    pub(crate) fn current_mcp_runtime(
        &self,
        server_id: &McpServerId,
    ) -> Option<Arc<McpServerRuntime>> {
        self.inner
            .state
            .lock()
            .expect("capability state lock poisoned")
            .mcp_runtimes
            .iter()
            .find(|generation| generation.server_id() == server_id)
            .map(McpRuntimeGeneration::runtime)
    }

    /// Returns the number of retired generations still tracked by the
    /// coordinator's close registry.
    #[cfg(test)]
    pub(crate) fn pending_mcp_retirements(&self) -> usize {
        self.inner.mcp_retirements.pending_count()
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
///
/// The no-op equivalence covers **both** halves of a published capability
/// generation:
///
/// - the model-visible capability contract: the tool definitions, the
///   available catalog, the Skills, and the shared Python/Node environment
///   digests;
/// - the effective executable/runtime binding identity: the frozen MCP
///   server bindings (transport command/args/env/cwd/endpoint/headers and
///   invocation policy).
///
/// A candidate may be considered a no-op only when both halves are
/// equivalent. In particular a source or runtime binding change — a
/// configured server whose command/env/endpoint changed, or a managed
/// Python package whose source edit moved its prepared state to a new
/// fingerprint-keyed directory and therefore changed the synthesized
/// binding's launch program — is a real publication even when the
/// model-facing `tools/list` catalog is byte-identical: after a successful
/// commit, newly admitted executions must use the new executable
/// generation, never the old one.
fn candidate_is_noop(
    candidate: &PreparedCapabilityCandidate,
    current: &CapabilitySnapshot,
) -> bool {
    !candidate.force_publish
        && candidate.skills.semantically_equivalent(current.skills())
        && candidate.candidate_registry.definitions() == current.tool_registry().definitions()
        && candidate.available_tools.as_ref() == current.available_tools()
        && candidate.python.as_ref().map(|env| env.digest.clone())
            == current.python_environment().map(|env| env.digest.clone())
        && candidate.node.as_ref().map(|env| env.digest.clone())
            == current.node_environment().map(|env| env.digest.clone())
        // The effective frozen server bindings are part of the no-op
        // equivalence (`McpServerBinding` derives deterministic equality).
        // The snapshot freezes exactly the bindings the candidate would
        // publish, so a changed executable identity can never be discarded
        // as a capability no-op merely because the tool definitions match.
        && candidate.effective_mcp_servers == *current.mcp_servers()
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
    mcp_leases: McpRuntimeLeaseSet,
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

    /// Clones the explicit physical MCP leases captured for this attempt.
    /// Detached background dispatch transfers these leases to its own runner
    /// before the attempt releases its capability lease.
    pub(crate) fn mcp_leases(&self) -> Option<McpRuntimeLeaseSet> {
        self.mcp_leases.try_clone()
    }

    #[cfg(test)]
    pub(crate) fn mcp_lease_uses_runtime(&self, runtime: &Arc<McpServerRuntime>) -> bool {
        self.mcp_leases.contains_runtime(runtime)
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
            // Keep this unit fixture independent of the developer's HOME:
            // the relocation proof owns both current roots explicitly.
            skill_discovery: crate::skills::SkillDiscoveryConfig {
                automatic_roots: vec![workspace.root().join(".agents/skills")],
                explicit_paths: Vec::new(),
            },
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn relocating_identical_skill_packages_is_a_new_capability_revision() {
        let (dir, mut coordinator) = coordinator();
        let first = coordinator
            .commit(prepare(&coordinator).await)
            .expect("first commit");
        let root_a = dir.path().join("workspace/.agents/skills");
        let root_b = dir.path().join("relocated-skills");
        std::fs::create_dir_all(&root_b).expect("root B");
        std::fs::rename(root_a.join("pdf"), root_b.join("pdf")).expect("relocate Skill");

        // The package content and version identity are unchanged. Only the
        // published host location moved, so rediscovery must not be treated
        // as an activation no-op.
        let inner = Arc::get_mut(&mut coordinator.inner).expect("unshared coordinator");
        inner
            .resource_inputs
            .get_mut()
            .expect("resource inputs")
            .skill_discovery = crate::skills::SkillDiscoveryConfig {
            automatic_roots: vec![root_b.clone()],
            explicit_paths: Vec::new(),
        };
        let candidate = prepare(&coordinator).await;
        let second = coordinator.commit(candidate).expect("relocated commit");
        assert_eq!(first.revision(), CapabilityRevision::new(1));
        assert_eq!(second.revision(), CapabilityRevision::new(2));
        assert_eq!(first.skills().bindings(), second.skills().bindings());
        assert_ne!(
            first.skills().locations(),
            second.skills().locations(),
            "the published host locations changed"
        );
        // Canonical, because discovery publishes the canonical root: the
        // fixture's own spelling is not the published one on a platform
        // whose temporary directory is reached through a symlink.
        assert_eq!(
            second.skills().catalog_entries()[0].location,
            std::fs::canonicalize(root_b.join("pdf/SKILL.md"))
                .expect("canonical relocated SKILL.md")
                .to_string_lossy()
        );
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

    /// A runtime claim closes the ordinary coordinator publication path, but
    /// leaves standalone commit semantics intact for unclaimed coordinators
    /// and gives the claiming runtime its private publication operation.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn claimed_coordinator_rejects_direct_commit_without_mutating_authority() {
        let (_dir, coordinator) = coordinator();
        let lifecycle = crate::runtime::types::ConversationLifecycle::new();
        let publication = coordinator
            .claim_conversation_runtime(&lifecycle)
            .expect("runtime publication authority");
        let before = coordinator.current_snapshot();
        let direct = coordinator
            .prepare_candidate()
            .await
            .expect("candidate preparation remains independent");
        assert_eq!(
            coordinator.commit(direct),
            Err(CapabilityCommitError::RuntimePublicationRequired)
        );
        assert!(Arc::ptr_eq(&before, &coordinator.current_snapshot()));
        assert_eq!(coordinator.active_attempts(), 0);

        assert!(lifecycle.activate());
        let runtime_candidate = coordinator
            .prepare_candidate()
            .await
            .expect("runtime-owned preparation");
        let published = coordinator
            .commit_runtime(&publication, runtime_candidate)
            .expect("the private runtime publication path remains available");
        assert_eq!(published.snapshot.revision(), CapabilityRevision::new(1));
    }

    /// Runtime drain wins the capability commit boundary: a candidate may be
    /// prepared while the runtime is running, but the revision swap is
    /// refused when drain linearizes before the final lifecycle read.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn runtime_drain_wins_capability_commit_boundary() {
        let (_dir, coordinator) = coordinator();
        let lifecycle = crate::runtime::types::ConversationLifecycle::new();
        let publication = coordinator
            .claim_conversation_runtime(&lifecycle)
            .expect("runtime publication authority");
        assert!(lifecycle.activate());
        let candidate = prepare(&coordinator).await;
        let hook = Arc::new(CommitBoundaryHook::default());
        coordinator.install_commit_boundary_hook(hook.clone());

        let coordinator_for_task = coordinator.clone();
        let commit_task = std::thread::spawn(move || {
            coordinator_for_task.commit_runtime(&publication, candidate)
        });
        hook.wait_entered();
        assert!(lifecycle.begin_drain());
        hook.proceed();
        let result = commit_task.join().expect("commit task").map(|_| ());
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

    /// Lazy Python store ownership (Issue #81, revised by Issue #174): the
    /// managed-package store initialization is optional and retryable, but
    /// once it succeeds the coordinator retains one stable process-local
    /// store identity for its lifetime.
    ///
    /// Phase 1: the store's `packages/` path is a conflicting regular file —
    /// preparation degrades the discovered package to `Unavailable` and the
    /// lazy slot stays empty (no permanently poisoned state). Phase 2: the
    /// filesystem is repaired and the next preparation retries and
    /// initializes the store (the package itself is made invalid first, so
    /// no environment build runs). Phase 3: later preparations reuse the
    /// same store identity instead of constructing a new coordination
    /// domain (which would restart environment build coalescing).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn python_store_initialization_is_lazy_retryable_and_stable() {
        use crate::capabilities::{CapabilitySourceId, CapabilitySourceState};
        use crate::tools::python::python_server_id;

        let dir = tempfile::tempdir().expect("temp dir");
        let workspace_root = dir.path().join("workspace");
        let package_root = workspace_root.join(".agents/tools/demo");
        std::fs::create_dir_all(&package_root).expect("package folder");
        std::fs::write(
            package_root.join("server.py"),
            b"from fastmcp import FastMCP\nmcp = FastMCP('demo')\n",
        )
        .expect("server source");
        std::fs::write(
            package_root.join("requirements.txt"),
            b"# no extra dependencies\n",
        )
        .expect("requirements");
        let store_root = dir.path().join("skill-env");
        std::fs::create_dir_all(store_root.join("python-tools")).expect("environment store root");
        let conflict = store_root.join("python-tools/packages");
        std::fs::write(&conflict, b"not a directory").expect("conflicting regular file");
        let source_id = CapabilitySourceId::Mcp(python_server_id("demo"));
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
        // itself succeeds with the package's server unavailable.
        assert_eq!(coordinator.python_store_identity_token(), None);
        let candidate = coordinator.prepare_candidate().await.expect("prepare");
        assert!(
            matches!(
                candidate.availability().get(&source_id),
                Some(CapabilitySourceState::Unavailable { .. })
            ),
            "the store-opening failure degrades the package's server availability: {:?}",
            candidate.availability()
        );
        assert_eq!(
            coordinator.python_store_identity_token(),
            None,
            "a failed initialization must not poison the lazy slot"
        );

        // Phase 2: the filesystem is repaired; the next preparation retries
        // and publishes the one stable store identity. The package is made
        // invalid first so the retry never starts an environment build.
        std::fs::remove_file(&conflict).expect("repair the filesystem");
        std::fs::remove_file(package_root.join("requirements.txt"))
            .expect("invalidate the package");
        let candidate = coordinator
            .prepare_candidate()
            .await
            .expect("retry prepare");
        assert!(
            matches!(
                candidate.availability().get(&source_id),
                Some(CapabilitySourceState::Unavailable { .. })
            ),
            "the invalid package stays unavailable: {:?}",
            candidate.availability()
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
        assert!(
            matches!(
                candidate.availability().get(&source_id),
                Some(CapabilitySourceState::Unavailable { .. })
            ),
            "the invalid package stays unavailable: {:?}",
            candidate.availability()
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
    use crate::tools::mcp::{McpInvalidationState, McpRuntimeGeneration, McpTransportConfig};
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
        assert!(coordinator.claim_conversation_runtime(&lifecycle).is_some());
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
        let workspace = coordinator.inner.workspace.clone();
        let invalidation = coordinator.inner.mcp_invalidation.clone();
        let mut generations = Vec::new();
        for server_id in &ids {
            let binding = coordinator
                .inner
                .resource_inputs
                .lock()
                .expect("resource inputs")
                .mcp_servers
                .get(server_id)
                .expect("fixture binding")
                .clone();
            let admission = lifecycle
                .try_enter_preparation()
                .expect("generation admission");
            let runtime = crate::tools::mcp::McpServerRuntime::connect(
                server_id,
                &binding,
                &workspace,
                invalidation.clone(),
            )
            .await
            .expect("fixture connection");
            generations.push(McpRuntimeGeneration::from_connected(
                server_id.clone(),
                runtime,
                Some(admission),
                tokio::runtime::Handle::current(),
                &coordinator.inner.mcp_retirements,
            ));
        }
        coordinator
            .inner
            .state
            .lock()
            .expect("capability state")
            .mcp_runtimes = generations;

        let alpha = Arc::new(CloseProbe::failing("injected unproven physical settlement"));
        let beta = Arc::new(CloseProbe::parking());
        coordinator
            .current_mcp_runtime(&ids[0])
            .expect("alpha runtime")
            .install_close_probe(alpha.clone());
        coordinator
            .current_mcp_runtime(&ids[1])
            .expect("beta runtime")
            .install_close_probe(beta.clone());

        assert!(lifecycle.begin_drain());
        let (done_tx, mut done_rx) = tokio::sync::oneshot::channel();
        let drain_coordinator = coordinator.clone();
        tokio::spawn(async move {
            let _ = done_tx.send(drain_coordinator.drain_conversation_owned().await);
        });

        // The two close tasks may be entered in either order, but the drain
        // cannot report until both physical owners have settled.
        tokio::time::timeout(std::time::Duration::from_mins(1), async {
            tokio::join!(alpha.wait_entered(), beta.wait_entered());
        })
        .await
        .expect("both sibling runtimes must receive close");
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
        let result = tokio::time::timeout(std::time::Duration::from_mins(1), done_rx)
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
        tokio::time::timeout(std::time::Duration::from_mins(1), pause.wait_entered())
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
            std::time::Duration::from_mins(1),
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
        let _ = coordinator.settle_ready_mcp_runtimes().await;
        assert_eq!(coordinator.pending_mcp_retirements(), 0);
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
            .drain_conversation_owned()
            .await
            .expect("the published generation settles");
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
        let runtime = candidate
            .mcp_runtimes
            .first()
            .expect("candidate runtime")
            .runtime();
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
        let result = crate::tools::executor::ToolExecutor::start(
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
                questionnaire_requester: None,
                todos: None,
                subagent: None,
            },
        )
        .completion
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
        let _ = coordinator.settle_ready_mcp_runtimes().await;
        assert_eq!(coordinator.pending_mcp_retirements(), 0);
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
        let (coordinator, _server_id) = coordinator_with_fixture(
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
            .drain_conversation_owned()
            .await
            .expect("the published generation settles");
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

    /// Mutates the configured stdio binding of the fixture server by adding
    /// one environment key the fixture ignores: the model-facing `tools/list`
    /// catalog stays byte-identical while the executable binding differs.
    fn fixture_binding_with_marker(
        coordinator: &CapabilityCoordinator,
        server_id: &McpServerId,
        marker: &str,
    ) {
        let mut inputs = coordinator
            .inner
            .resource_inputs
            .lock()
            .expect("capability resource-input lock poisoned");
        let binding = inputs
            .mcp_servers
            .get_mut(server_id)
            .expect("fixture binding");
        binding.transport = match &binding.transport {
            McpTransportConfig::Stdio {
                program,
                args,
                cwd,
                environment,
            } => {
                let mut environment = environment.clone();
                environment.insert("RUSTX_NOOP_BINDING_MARKER".to_owned(), marker.to_owned());
                McpTransportConfig::Stdio {
                    program: program.clone(),
                    args: args.clone(),
                    cwd: cwd.clone(),
                    environment,
                }
            }
            other @ McpTransportConfig::StreamableHttp { .. } => {
                panic!("the fixture binding is stdio: {other:?}")
            }
        };
    }

    /// Blocker 2 executable identity: identical model-facing definitions with
    /// a changed executable binding is a NEW publication, never a no-op.
    ///
    /// Proves through the actual coordinator commit path: commit v1, change
    /// the effective binding while preserving the `tools/list` schema,
    /// prepare and commit v2, the executable generation advances, and a
    /// future attempt lease resolves to v2.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_changed_executable_binding_is_a_new_publication_even_with_identical_definitions() {
        if serve_if_fixture_mode(FixtureServer::with_list_changed()).await {
            return;
        }
        let dir = tempfile::tempdir().expect("temp dir");
        let (coordinator, server_id) = coordinator_with_fixture(
            &dir,
            "fixture",
            "capabilities::coordinator::mcp_race_tests::a_changed_executable_binding_is_a_new_publication_even_with_identical_definitions",
        );
        let candidate_v1 = coordinator.prepare_candidate().await.expect("prepare v1");
        let v1_runtime = candidate_v1
            .mcp_runtimes
            .first()
            .expect("v1 candidate runtime")
            .runtime();
        let v1_snapshot = coordinator.commit(candidate_v1).expect("commit v1");
        assert_eq!(v1_snapshot.revision().get(), 1);

        // The binding changes (a new environment key the fixture ignores);
        // the tool catalog stays identical.
        fixture_binding_with_marker(&coordinator, &server_id, "v2");
        let candidate_v2 = coordinator.prepare_candidate().await.expect("prepare v2");
        assert_eq!(
            candidate_v2.candidate_registry.definitions(),
            v1_snapshot.tool_registry().definitions(),
            "the model-facing tools/list contract is byte-identical"
        );
        let v2_snapshot = coordinator.commit(candidate_v2).expect("commit v2");
        assert_eq!(
            v2_snapshot.revision().get(),
            2,
            "a changed executable binding is a new publication, never a no-op"
        );

        // The executable generation advanced: the current runtime is a
        // different physical generation, and a future attempt lease resolves
        // to it.
        let v2_runtime = coordinator
            .current_mcp_runtime(&server_id)
            .expect("v2 runtime");
        assert!(
            !Arc::ptr_eq(&v1_runtime, &v2_runtime),
            "the committed executable generation advanced"
        );
        let future_lease = coordinator.acquire_attempt_lease();
        assert_eq!(future_lease.revision().get(), 2);
        assert!(
            future_lease
                .mcp_leases()
                .expect("future leases")
                .contains_runtime(&v2_runtime),
            "a future admission executes on the new generation"
        );
        drop(future_lease);
        let _ = coordinator.settle_ready_mcp_runtimes().await;
        assert_eq!(coordinator.pending_mcp_retirements(), 0);
    }

    /// Blocker 2 no-op boundary: an unchanged executable binding with
    /// unchanged definitions is a true no-op — the commit never fabricates a
    /// revision.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_unchanged_binding_and_definitions_is_a_true_noop() {
        if serve_if_fixture_mode(FixtureServer::with_list_changed()).await {
            return;
        }
        let dir = tempfile::tempdir().expect("temp dir");
        let (coordinator, _server_id) = coordinator_with_fixture(
            &dir,
            "fixture",
            "capabilities::coordinator::mcp_race_tests::an_unchanged_binding_and_definitions_is_a_true_noop",
        );
        let first = coordinator.prepare_candidate().await.expect("prepare");
        let snapshot = coordinator.commit(first).expect("first commit");
        assert_eq!(snapshot.revision().get(), 1);

        // An identical rediscovery is a commit no-op: same definitions, same
        // effective bindings.
        let refresh = coordinator
            .prepare_candidate()
            .await
            .expect("refresh prepare");
        let second = coordinator.commit(refresh).expect("refresh commit");
        assert_eq!(
            second.revision().get(),
            1,
            "identical content and identical executable bindings never fabricate a revision"
        );
        let _ = coordinator.settle_ready_mcp_runtimes().await;
        assert_eq!(coordinator.pending_mcp_retirements(), 0);
    }

    /// Blocker 2 old-lease/new-generation ownership: an execution already
    /// admitted before the commit keeps its old generation (and can still
    /// call it) while a future admission resolves to the new generation; the
    /// old generation retires only after its leases settle.
    ///
    /// The lease is a direct MCP generation lease (not an attempt lease): a
    /// reload commit is refused while an *attempt* lease is active (the
    /// `Busy` contract), so reloads happen between attempts. The generation
    /// lease alone pins the old physical generation without blocking the
    /// commit — exactly the ownership an in-flight execution holds while a
    /// reload lands.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_old_lease_keeps_serving_its_generation_while_future_leases_resolve_to_the_new_one()
    {
        if serve_if_fixture_mode(FixtureServer::with_list_changed()).await {
            return;
        }
        let dir = tempfile::tempdir().expect("temp dir");
        let (coordinator, server_id) = coordinator_with_fixture(
            &dir,
            "fixture",
            "capabilities::coordinator::mcp_race_tests::an_old_lease_keeps_serving_its_generation_while_future_leases_resolve_to_the_new_one",
        );
        let candidate_v1 = coordinator.prepare_candidate().await.expect("prepare v1");
        let v1_runtime = candidate_v1
            .mcp_runtimes
            .first()
            .expect("v1 candidate runtime")
            .runtime();
        let v1_snapshot = coordinator.commit(candidate_v1).expect("commit v1");
        assert_eq!(v1_snapshot.revision().get(), 1);

        // The old execution is admitted before the source/runtime change:
        // it holds a direct generation lease on the old physical runtime.
        let old_leases = v1_snapshot
            .acquire_mcp_leases()
            .expect("old generation accepts a lease");
        assert!(
            old_leases.contains_runtime(&v1_runtime),
            "the old execution owns the old generation"
        );

        // The source/runtime binding changes and commits while the old lease
        // is held (no attempt lease is active, so the reload is legal).
        fixture_binding_with_marker(&coordinator, &server_id, "v2");
        let candidate_v2 = coordinator.prepare_candidate().await.expect("prepare v2");
        let v2_snapshot = coordinator.commit(candidate_v2).expect("commit v2");
        assert_eq!(v2_snapshot.revision().get(), 2);
        let v2_runtime = coordinator
            .current_mcp_runtime(&server_id)
            .expect("v2 runtime");

        // The old lease is untouched by the commit: it still owns the old
        // generation and can still call it.
        assert!(
            old_leases.contains_runtime(&v1_runtime),
            "an already-admitted execution keeps its old generation"
        );
        let tools = v1_runtime.list_tools().await.expect("old catalog");
        assert!(
            tools.iter().any(|tool| tool.name == "echo"),
            "the old generation still serves its catalog"
        );
        assert_eq!(
            coordinator.pending_mcp_retirements(),
            1,
            "the retired old generation cannot close while its lease is held"
        );

        // A future admission resolves to the new generation.
        let future_lease = coordinator.acquire_attempt_lease();
        assert_eq!(future_lease.revision().get(), 2);
        assert!(
            future_lease
                .mcp_leases()
                .expect("future leases")
                .contains_runtime(&v2_runtime),
            "a future admission executes on the new generation"
        );
        drop(future_lease);

        // Once the old lease settles, the old generation retires: its close
        // completes and the retirement registry reaps it.
        drop(old_leases);
        let _ = coordinator.settle_ready_mcp_runtimes().await;
        assert_eq!(
            coordinator.pending_mcp_retirements(),
            0,
            "the old generation retires only after its leases settle"
        );
    }
}
