//! The one Rust-side local runtime composition owner (Issue #42, Issue
//! #61).
//!
//! ```text
//! explicit startup configuration
//!         |
//!         v
//! ModelCatalog + CurrentRuntimeConfig + SessionPersistentState
//!         |
//!         +--> authoritative session model state
//!         +--> immutable resolved model bindings
//!         +--> summary-model policy
//!         +--> ConversationToolRuntime
//!         +--> native base ToolRegistry
//!         +--> CapabilityCoordinator
//!         +--> context policy/estimator/status pieces
//!         |
//!         v
//! LocalConversationCore  (the shared semantic composition, inactive)
//!         |
//!         +-- into_interactive(): bind RuntimeClientHost, then activate
//!         |       -> LocalConversationRuntime (Runtime Client + endpoint)
//!         |
//!         +-- into_subagent_child_with_route(): bind RuntimeClientHost, subscribe a
//!         |       bounded parent-observation queue, then activate
//!         |       -> child ConversationRuntime + optional live inspection endpoint
//!         |
//!         +-- into_headless(): activate with no Runtime Client host
//!                 -> HeadlessConversationRuntime
//! ```
//!
//! The semantic composition — the model catalog/session/tool/capability/
//! context assembly — exists exactly once in
//! [`LocalConversationCore::compose`]. The interactive, subagent-child, and
//! headless production runtimes are the final paths over that same core:
//!
//! ```text
//! compose semantic inactive core
//!     |
//!     +-- interactive: bind RuntimeClientHost, activate, return
//!     +-- subagent child: bind RuntimeClientHost, add a bounded parent
//!     |                   observation subscriber, activate, return
//!     +-- headless:    activate, return
//! ```
//!
//! Activation is the one explicit lifecycle boundary in all three paths
//! (`ConversationRuntime::activate`), and each production path returns an
//! already-active handle.
//!
//! The governing invariant:
//!
//! > `LocalSessionProduct` owns the native SessionCatalog/SessionGraph and
//! > exactly one active linear ConversationRuntime. The active runtime owns
//! > one ConversationId, one ConversationToolRuntime identity, one
//! > CapabilityCoordinator, one context policy domain, and one linear
//! > ConversationSurface. A Session switch quiesces and releases that
//! > runtime before the product publishes a replacement and reconnects.
//! > Runtime Client attachments may come and go without replacing the
//! > semantic owners of the active lineage.
//!
//! A client — including the Issue #39 TUI — owns the child process
//! lifecycle and nothing else. It never assembles provider adapters, model
//! parameters, context engines, tool registries, capability coordinators,
//! or summary models.
//!
//! # Ordering
//!
//! Composition follows a fixed order, and the initial capability candidate
//! is **prepared and committed before any protocol input is served**. A
//! *core* startup failure therefore never leaves a partially initialized
//! protocol server: composition returns an error and the process exits
//! before a single protocol byte is written.
//!
//! # Fatal vs isolated startup failures (Issue #81)
//!
//! The boundary is ownership. Failures that prove the core runtime itself
//! cannot be constructed remain fatal composition errors: unreadable or
//! invalid startup files, model catalog/credential/binding failures, an
//! invalid current runtime configuration, workspace/private-store ownership
//! violations, native tool plane construction, and structurally invalid
//! capability-plane configuration (a workspace-overlapping environment
//! store, a malformed Skill, a dependency conflict, shared environment
//! materialization).
//!
//! Failures of **optional external capability sources** — each configured
//! MCP server and each managed Python tool package independently — are
//! isolated by the capability plane itself (`prepare_candidate` records
//! them as typed [`CapabilitySourceState::Unavailable`](crate::capabilities::CapabilitySourceState)
//! state instead of failing the candidate). The runtime therefore starts
//! with, e.g., native tools ready, one MCP server unavailable and another
//! ready; the base/native capability set is never
//! conditional on an optional source, and one optional source's failure
//! never suppresses another. The Runtime Client projection carries the
//! typed availability state, so a client observes the reason instead of an
//! opaque transport EOF.
//!
//! The conversation runtime is constructed **inactive** inside the core;
//! the optional Runtime Client host binds over the inert runtime, and the
//! final path then activates it explicitly. Binding a host is therefore a
//! composition decision, not a hot operation: a headless composition
//! (Issue #60 subagents) omits the host entirely and activates directly,
//! and a late bind over an activated runtime is refused with
//! `HostConstructionError::RuntimeAlreadyActivated`. Runtime Client
//! *attachments* remain fully dynamic after activation.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures_util::future::BoxFuture;

use crate::capabilities::{
    CapabilityCoordinator, CapabilityCoordinatorConfig, CapabilityResourceInputs,
    ToolActivationPolicy,
};
use crate::context::{AgentStatusEngine, DefaultTokenEstimator, TokenEstimator};
use crate::durable::{ConversationStoreBinding, SqliteConversationStore};
use crate::model::catalog::{
    CredentialEnvironment, ModelCatalog, ModelCatalogError, ProcessCredentialEnvironment,
};
use crate::model::invocation::{ModelBindingRegistry, ModelInvocationError};
use crate::model::session::SessionModelState;
use crate::runtime::RuntimeResourceRevision;
use crate::runtime::conversation_runtime::{
    ConversationContextConfig, ConversationRuntime, ConversationRuntimeError,
    RuntimeConversationConfig,
};
use crate::runtime::identity::ConversationId;
use crate::runtime::interaction::InteractionRoute;
use crate::runtime::resources::{
    PreparedRuntimeResources, ProjectContextFile, RuntimeResourceLoadError, RuntimeResourceLoader,
    RuntimeResourceSnapshot, load_project_context_files,
};
use crate::runtime::subagent::{
    ResolvedSubagentTool, SubagentCatalog, SubagentDefinition, SubagentProjectInstructionPolicy,
    SubagentResolver, SubagentWorkspaceManager, child_conversation_inspection_liveness_path,
    child_conversation_inspection_socket_path, child_conversation_store_path,
    is_safe_child_conversation_component,
};
use crate::runtime::workflow::{
    MAX_WORKFLOW_BYTES, WorkflowCatalog, WorkflowDefinition, WorkflowOutputLatch, WorkflowProgram,
    WorkflowRuntime,
};
use crate::runtime_client::endpoint::RuntimeClientEndpoint;
use crate::runtime_client::host::{
    HostConstructionError, RuntimeClientHost, RuntimeClientHostConfig, RuntimeClientSessionControl,
};
use crate::skills::SkillDiscoveryConfig;
use crate::tools::environment::ToolEnvironment;
use crate::tools::executor::ToolRegistry;
use crate::tools::native::{NativeToolResources, register_native_tools};
use crate::tools::runtime::ConversationToolRuntime;
use crate::tools::types::ToolDefinition;

use super::config::{
    CurrentRuntimeConfig, CurrentRuntimeConfigError, SubagentsDocument, WorkflowsDocument,
};
use super::session::{
    SessionCatalog, SessionError, SessionId, SessionNodeId, SessionNodeOrigin,
    SessionPersistentState,
};
use super::supervisor::{LocalSessionSupervisor, SessionSupervisorError};

/// The one project-owned namespace for Agent resources. Runtime-owned state
/// remains under the separately configured runtime root.
const AGENT_RESOURCES_DIRECTORY: &str = ".agents";
/// The native Workflow source directory inside [`AGENT_RESOURCES_DIRECTORY`].
const WORKFLOW_RESOURCES_DIRECTORY: &str = "workflows";

/// Which Session a launch binds.
///
/// Startup is not a resume. A process begins on an empty Session and leaves
/// every persisted Session as history reachable through `/resume`; binding a
/// persisted one is an explicit request, in exactly two forms. Continuing the
/// published active selection is the one a client repeats when it replaces
/// the process to complete a Session switch that was already published
/// durably; naming a Session is what a launch does when the user already
/// knows where they want to be.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum StartupSession {
    /// Start on an empty Session, publishing one when the active Session has
    /// already been used.
    #[default]
    Empty,
    /// Bind the Session/node the catalog publishes as active.
    ContinueActive,
    /// Bind the named persisted Session — and the named lineage node when
    /// one is given. The selection is *planned* before the runtime is
    /// composed and published with it: a launch that cannot compose the
    /// Session it named leaves the active selection where it found it.
    Select {
        /// The persisted Session to bind.
        session: SessionId,
        /// The lineage node to bind; the Session's own active node when
        /// absent.
        node: Option<SessionNodeId>,
    },
    /// Attach a Runtime Client to a known durable child conversation. This
    /// path owns no Session catalog selection and is read-only.
    InspectConversation {
        /// The durable conversation identity to inspect.
        conversation_id: ConversationId,
    },
}

/// The explicit startup paths of one local runtime process.
///
/// There is no discovery and no precedence: every path is given explicitly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalRuntimePaths {
    /// The model catalog (`models.jsonc`) path.
    pub models: PathBuf,
    /// The current runtime/project configuration path. It is read on every
    /// process start, including Session resume.
    pub config: PathBuf,
    /// Repeatable explicit Skill package/root paths from the command line.
    pub skill_paths: Vec<PathBuf>,
    /// Disable automatic/default Skill roots while retaining explicit paths.
    pub no_skills: bool,
    /// Disable optional native/built-in tools from startup activation;
    /// mandatory native Read remains active.
    pub no_builtin_tools: bool,
    /// Disable every optional Tool while retaining available metadata;
    /// mandatory native Read remains active.
    pub no_tools: bool,
    /// The Session this launch binds. Startup never resumes on its own;
    /// `--continue` is the explicit request behind
    /// [`StartupSession::ContinueActive`] and `--session` the one behind
    /// [`StartupSession::Select`].
    pub startup_session: StartupSession,
    /// The display name to give the Session this launch binds, from
    /// `--name`. A Session is otherwise unnamed and `/resume` shows it by
    /// its first message; naming one at startup is the same metadata
    /// operation `/name` performs, moved to the command line.
    pub session_name: Option<String>,
    /// Strict startup Tool allowlist, when supplied.
    pub tools: Option<Vec<String>>,
    /// Final startup Tool exclusions.
    pub exclude_tools: Vec<String>,
    /// The model-visible workspace root.
    pub workspace: PathBuf,
    /// The exact runtime-private root from which disjoint private
    /// subdirectories are derived. Child conversation stores live below its
    /// stable `subagents/<conversation-id>` semantic directory; child
    /// execution incarnations remain private and disposable.
    pub runtime_root: PathBuf,
}

impl LocalRuntimePaths {
    /// The runtime-private artifact root.
    #[must_use]
    pub fn artifacts_root(&self) -> PathBuf {
        self.runtime_root.join("artifacts")
    }

    /// The runtime-private capability environment store root.
    #[must_use]
    pub fn environment_store_root(&self) -> PathBuf {
        self.runtime_root.join("environments")
    }

    /// The capability environment store of one independent conversation
    /// lineage. Branches do not share mutable environment materialization.
    #[must_use]
    pub fn environment_store_root_for(
        &self,
        conversation_id: &crate::runtime::identity::ConversationId,
    ) -> PathBuf {
        self.environment_store_root().join(conversation_id.as_str())
    }
}

/// The injectable non-model dependencies of composition.
///
/// Model bindings are deliberately not injectable: production constructs the
/// supported adapters directly from the validated catalog binding. Tests that
/// need provider synchronization use an explicit catalog endpoint aimed at a
/// local HTTP fixture.
pub struct LocalRuntimeDependencies {
    /// The credential environment used to resolve `$ENV_VAR` sources.
    pub credentials: Arc<dyn CredentialEnvironment>,
    /// The deterministic token estimator.
    pub estimator: Arc<dyn TokenEstimator>,
    /// Optional executable override for the native child process. Production
    /// uses the current `rustx` executable; an explicit value lets an
    /// integration harness point the same native boundary at that binary
    /// when the parent is running inside a test executable.
    pub child_program: Option<PathBuf>,
}

impl Default for LocalRuntimeDependencies {
    fn default() -> Self {
        Self {
            credentials: Arc::new(ProcessCredentialEnvironment),
            estimator: Arc::new(DefaultTokenEstimator),
            child_program: None,
        }
    }
}

impl std::fmt::Debug for LocalRuntimeDependencies {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalRuntimeDependencies")
            .finish_non_exhaustive()
    }
}

/// Reload-time resource composition for one local runtime. Command-line
/// inputs are immutable; `rustx.jsonc` and filesystem resources are read only
/// when this loader is explicitly invoked by the runtime reload boundary.
struct LocalRuntimeResourceLoader {
    paths: LocalRuntimePaths,
    native_resources: NativeToolResources,
    workflow_runtime: WorkflowRuntime,
    /// The launch-scoped model authority. Reload re-reads `rustx.jsonc` but
    /// not `models.jsonc`, so an agent's explicit model reference is
    /// validated against exactly the catalog this process was launched with.
    models: ModelBindingRegistry,
}

impl LocalRuntimeResourceLoader {
    fn new(
        paths: LocalRuntimePaths,
        native_resources: NativeToolResources,
        models: ModelBindingRegistry,
        workflow_runtime: WorkflowRuntime,
    ) -> Self {
        Self {
            paths,
            native_resources,
            workflow_runtime,
            models,
        }
    }
}

impl RuntimeResourceLoader for LocalRuntimeResourceLoader {
    #[allow(clippy::too_many_lines)] // one atomic candidate construction pipeline
    fn prepare<'a>(
        &'a self,
        capability: &'a CapabilityCoordinator,
    ) -> BoxFuture<'a, Result<PreparedRuntimeResources, RuntimeResourceLoadError>> {
        Box::pin(async move {
            let config_bytes = std::fs::read(&self.paths.config).map_err(|error| {
                RuntimeResourceLoadError::new(format!(
                    "cannot read runtime config {}: {error}",
                    self.paths.config.display()
                ))
            })?;
            let config = CurrentRuntimeConfig::from_jsonc_slice(&config_bytes)
                .map_err(|error| RuntimeResourceLoadError::new(error.to_string()))?;
            let base_environment = config
                .tool_environment()
                .map_err(|error| RuntimeResourceLoadError::new(error.to_string()))?;
            let workspace = capability.current_snapshot().workspace_root().to_path_buf();
            // The catalog is built before the base registry, because the
            // `subagent` intrinsic's model-facing description is generated
            // from exactly the catalog this candidate generation admits.
            let subagents = load_subagent_catalog(&workspace, &config.subagents)?;
            let main_admission = config
                .subagents
                .main
                .iter()
                .cloned()
                .collect::<BTreeSet<_>>();
            let workflow_admission = config
                .subagents
                .workflow
                .iter()
                .cloned()
                .collect::<BTreeSet<_>>();
            let main_catalog = subagents
                .admitted(&main_admission)
                .map_err(|error| RuntimeResourceLoadError::new(format!("{error}")))?;
            let workflows =
                load_workflow_catalog(&workspace, &config.workflows, &workflow_admission)?;
            let default_tools = default_tools_with_workflows(&config.default_tools, &workflows);
            let mut registry = ToolRegistry::new();
            register_native_tools(
                &mut registry,
                NativeToolResources {
                    subagent_catalog: main_catalog,
                    ..self.native_resources.clone()
                },
                config.native_tools.to_policies(),
            )
            .map_err(|error| {
                RuntimeResourceLoadError::new(format!(
                    "cannot register reload-time native tools: {error}"
                ))
            })?;
            crate::tools::native::register_workflow_tools(
                &mut registry,
                &self.workflow_runtime,
                &workflows,
            )
            .map_err(|error| {
                RuntimeResourceLoadError::new(format!(
                    "cannot register reload-time Workflow Tools: {error}"
                ))
            })?;
            let workspace_authority =
                crate::tools::Workspace::new(&workspace).map_err(|error| {
                    RuntimeResourceLoadError::new(format!(
                        "cannot resolve reload workspace: {error}"
                    ))
                })?;
            let mut skill_discovery =
                SkillDiscoveryConfig::default_for_workspace(&workspace_authority);
            if self.paths.no_skills {
                skill_discovery.automatic_roots.clear();
            }
            skill_discovery.explicit_paths.extend(
                config
                    .skills
                    .iter()
                    .map(|path| resolve_workspace_path(&workspace, path)),
            );
            skill_discovery.explicit_paths.extend(
                self.paths
                    .skill_paths
                    .iter()
                    .map(|path| resolve_workspace_path(&workspace, path)),
            );
            let mcp_servers = config
                .mcp_bindings()
                .map_err(|error| RuntimeResourceLoadError::new(error.to_string()))?;
            let candidate = capability
                .prepare_candidate_with_inputs(CapabilityResourceInputs {
                    base_tool_registry: Arc::new(registry),
                    tool_activation: ToolActivationPolicy {
                        default_tools: Some(default_tools),
                        no_builtin_tools: self.paths.no_builtin_tools,
                        no_tools: self.paths.no_tools,
                        tools: self.paths.tools.clone(),
                        exclude_tools: self.paths.exclude_tools.clone(),
                    },
                    skill_discovery,
                    mcp_servers,
                    base_environment,
                })
                .await
                .map_err(|error| {
                    RuntimeResourceLoadError::new(format!(
                        "cannot prepare reload capability resources: {error}"
                    ))
                })?;
            validate_workflow_tool_name_collisions(&candidate, &workflows)?;
            let prepared = PreparedRuntimeResources::new(
                load_project_context_files(&workspace)?,
                None,
                crate::context::ContextAssembly::new(),
                candidate,
            )
            .with_subagent_catalog(subagents);
            let prepared = prepared
                .with_subagent_admissions(main_admission, workflow_admission)
                .with_workflow_catalog(workflows);
            // The catalog is admitted against the very candidate that is
            // about to be published, and rejection happens entirely
            // off-side: nothing of this candidate generation — catalog,
            // capability state, project instructions, Skills, model
            // selection, or the active generation — has been published yet,
            // so the previous complete generation stays authoritative.
            validate_subagent_catalog(&prepared, &self.models)?;
            Ok(prepared)
        })
    }
}

/// The cancellable-owned-work guard of one child preparation (Issue #145).
///
/// External capability composition is no longer a cheap synchronous
/// prelude: it may start an MCP process, negotiate a protocol, list a
/// catalog, and materialize the fingerprint-keyed uv environment of a
/// managed Python tool package (Issue #174). Two authorities must therefore
/// be able to end it *before* the child ever reaches semantic work:
///
/// ```text
/// attempt-derived cancellation   the spawn attempt that owns this child
/// parent control-channel EOF     the parent process disappeared
/// ```
///
/// Both converge on the ONE preparation cancellation signal this guard
/// owns — parent loss *is* a physical cancellation authority, not merely a
/// reason to drop a waiter — and every preparatory supervised process unit
/// of the child observes that signal, so either authority physically
/// cancels the in-flight preparation work.
pub(crate) struct ChildPreparation {
    cancellation: crate::runtime::cancellation::CancellationSignal,
    parent_lost: Option<crate::local_runtime::dispatcher::ChildControlHandle>,
}

impl ChildPreparation {
    /// The production guard: the child's own preparation cancellation plus
    /// the control channel that is its parent-liveness authority.
    pub(crate) fn new(
        cancellation: crate::runtime::cancellation::CancellationSignal,
        parent_lost: crate::local_runtime::dispatcher::ChildControlHandle,
    ) -> Self {
        Self {
            cancellation,
            parent_lost: Some(parent_lost),
        }
    }

    /// A guard with no parent-liveness authority, for compositions that are
    /// not driven by a control channel (tests and in-process fixtures).
    #[cfg(test)]
    pub(crate) fn detached() -> Self {
        Self {
            cancellation: crate::runtime::cancellation::CancellationSignal::new(),
            parent_lost: None,
        }
    }

    /// The one pre-commit cancellation authority of this preparation.
    ///
    /// Every cancellation-aware preparation step is built with it, so a
    /// settled preparation physically cancels its preparatory supervised
    /// units rather than merely dropping their waiters.
    pub(crate) fn cancellation(&self) -> crate::runtime::cancellation::CancellationSignal {
        self.cancellation.clone()
    }

    /// Whether a settlement authority has already won.
    ///
    /// A guarded step can complete *after* a settlement authority fired
    /// (the race is inherent), so the caller must check this before
    /// treating the step's output as publishable: once pre-commit
    /// cancellation has won, `Ready` must be impossible.
    pub(crate) fn is_settled(&self) -> bool {
        self.cancellation.is_cancelled()
            || self
                .parent_lost
                .as_ref()
                .is_some_and(crate::local_runtime::dispatcher::ChildControlHandle::parent_lost)
    }

    /// Runs one preparation step under both settlement authorities.
    ///
    /// The step is built with [`ChildPreparation::cancellation`]: it is
    /// cancellation-aware owned work that physically settles its
    /// preparatory supervised units before it returns. A settlement
    /// authority that wins therefore does **not** merely drop the step
    /// future — dropping a waiter never settles physical work — it fires
    /// the cancellation (parent loss first cancels the signal) and then
    /// awaits the step, so no MCP process, runtime probe, or uv build
    /// outlives a settled preparation. If the cancellation is observed
    /// *during* the step's final unguarded stretch, the step may still
    /// complete: the caller checks [`ChildPreparation::is_settled`] before
    /// publishing anything derived from it.
    pub(crate) async fn guard<T, F>(
        &self,
        step: F,
    ) -> Result<T, crate::capabilities::CapabilityPreparationError>
    where
        F: std::future::Future<Output = Result<T, crate::capabilities::CapabilityPreparationError>>,
    {
        let parent_lost = async {
            match &self.parent_lost {
                Some(handle) => handle.parent_lost_signal().await,
                None => std::future::pending().await,
            }
        };
        tokio::pin!(step);
        tokio::select! {
            biased;
            () = self.cancellation.cancelled() => {
                // Await the step's physical settlement: the step observes
                // this same signal and settles its preparatory units
                // before returning.
                let _ = step.await;
                Err(
                    crate::capabilities::CapabilityPreparationError::PreparationSettled(
                        "the spawn attempt cancelled this child before it was owned".to_owned(),
                    ),
                )
            },
            () = parent_lost => {
                // Parent control-channel loss is a physical cancellation
                // authority: fire the one preparation signal so every
                // preparatory unit is cancelled, then await the step's
                // settlement.
                self.cancellation.cancel();
                let _ = step.await;
                Err(
                    crate::capabilities::CapabilityPreparationError::PreparationSettled(
                        "the parent runtime disappeared while this child was still composing"
                            .to_owned(),
                    ),
                )
            },
            outcome = &mut step => outcome,
        }
    }
}

/// The controllable external-preparation seam of the Issue #145
/// regression tests (test builds only; in every other build the two entry
/// points below are inert and the environment variable is read by
/// nothing).
///
/// When armed, the child composition parks **inside the guarded external
/// preparation step** over a test-owned Unix socket: it announces the
/// exact boundary (`entered-external-preparation`) and then resolves
/// either by the test's `release` line or by the preparation cancellation
/// — exactly the contract of a well-behaved external preparation step (an
/// MCP connect, a uv build). The release arm is deliberately biased ahead
/// of the cancellation arm, so a test can drive the dangerous ordering —
/// cancellation committed, *then* the external step completes — and prove
/// the surrounding settlement checks make `Ready` impossible anyway.
#[cfg(test)]
const TEST_PREPARATION_GATE_ENV: &str = "RUSTX_ISSUE145_PREPARATION_GATE";

/// An in-process registration of the test preparation gate for ONE
/// specific child runtime root. In-process tests (which cannot arm a
/// process-global environment variable without poisoning concurrent
/// tests) register the gate for the exact child they compose, and gain
/// direct observability of the exact preparation cancellation signal the
/// gated step runs under — the proof handle of the Issue #145 local
/// race: Cancel consumed (signal set), then the step completes.
#[cfg(test)]
pub(crate) struct TestPreparationGate {
    entered: tokio::sync::watch::Sender<bool>,
    release: tokio::sync::watch::Sender<bool>,
    cancellation: std::sync::Mutex<Option<crate::runtime::cancellation::CancellationSignal>>,
}

/// The in-process gate registrations, keyed by the child runtime root.
#[cfg(test)]
static TEST_PREPARATION_GATES: std::sync::Mutex<
    Vec<(std::path::PathBuf, std::sync::Weak<TestPreparationGate>)>,
> = std::sync::Mutex::new(Vec::new());

/// Arms the test preparation gate for one child runtime root (test
/// builds only). The returned handle is the test's control and
/// observation channel; dropping it unregisters the gate.
#[cfg(test)]
pub(crate) fn arm_test_preparation_gate(
    runtime_root: &std::path::Path,
) -> std::sync::Arc<TestPreparationGate> {
    let (entered, _) = tokio::sync::watch::channel(false);
    let (release, _) = tokio::sync::watch::channel(false);
    let gate = std::sync::Arc::new(TestPreparationGate {
        entered,
        release,
        cancellation: std::sync::Mutex::new(None),
    });
    let mut gates = TEST_PREPARATION_GATES.lock().expect("gate registry");
    gates.retain(|(root, weak)| weak.strong_count() > 0 && root != runtime_root);
    gates.push((runtime_root.to_path_buf(), std::sync::Arc::downgrade(&gate)));
    gate
}

/// The gate registered for this child runtime root, if any.
#[cfg(test)]
fn registered_preparation_gate(
    runtime_root: &std::path::Path,
) -> Option<std::sync::Arc<TestPreparationGate>> {
    TEST_PREPARATION_GATES
        .lock()
        .expect("gate registry")
        .iter()
        .find_map(|(root, weak)| (root == runtime_root).then(|| weak.upgrade()).flatten())
}

#[cfg(test)]
impl TestPreparationGate {
    /// Parks until the gated step entered: the child is provably inside
    /// external preparation.
    pub(crate) async fn entered(&self) {
        let mut entered = self.entered.subscribe();
        while !*entered.borrow_and_update() {
            entered.changed().await.expect("the gate outlives the step");
        }
    }

    /// The exact preparation cancellation signal the gated step runs
    /// under; valid once [`Self::entered`] resolved (the step captures its
    /// authority before announcing its entry).
    pub(crate) fn cancellation(&self) -> crate::runtime::cancellation::CancellationSignal {
        self.cancellation
            .lock()
            .expect("gate cancellation")
            .clone()
            .expect("the step captured its cancellation authority before entering")
    }

    /// Releases the gated step to completion (idempotent).
    ///
    /// `send_replace`, not `send`: a `watch` send drops the value when no
    /// receiver is currently subscribed, and a lost release would park the
    /// gated step forever.
    pub(crate) fn release(&self) {
        self.release.send_replace(true);
    }

    /// The gated step itself: announces the boundary, then resolves by
    /// release or cancellation — biased towards completion, exactly like
    /// the socket seam, so a settled authority still loses to a released
    /// step and the surrounding settlement checks are what must hold.
    async fn run(
        &self,
        cancellation: &crate::runtime::cancellation::CancellationSignal,
    ) -> Result<(), crate::capabilities::CapabilityPreparationError> {
        *self.cancellation.lock().expect("gate cancellation") = Some(cancellation.clone());
        // Subscribe BEFORE announcing the entry so a release can never be
        // missed once `entered` resolved, and `send_replace` the entry
        // itself: a `watch` send drops the value when the test has not
        // subscribed yet, which would park its `entered` wait forever.
        let mut release = self.release.subscribe();
        self.entered.send_replace(true);
        tokio::select! {
            biased;
            released = release.changed() => match released {
                Ok(()) => Ok(()),
                Err(error) => Err(
                    crate::capabilities::CapabilityPreparationError::PreparationSettled(format!(
                        "the test preparation gate closed without a release: {error}"
                    )),
                ),
            },
            () = cancellation.cancelled() => Err(
                crate::capabilities::CapabilityPreparationError::PreparationSettled(
                    "the preparation cancellation settled the gated external step".to_owned(),
                ),
            ),
        }
    }
}

/// Whether the test-only preparation gate is armed for this child.
#[cfg(test)]
fn preparation_gate_armed(runtime_root: &std::path::Path) -> bool {
    registered_preparation_gate(runtime_root).is_some()
        || std::env::var_os(TEST_PREPARATION_GATE_ENV).is_some()
}

/// Whether the test-only preparation gate is armed: never outside test
/// builds.
#[cfg(not(test))]
const fn preparation_gate_armed(_runtime_root: &std::path::Path) -> bool {
    false
}

/// Parks at the test preparation gate when armed (test builds only).
#[cfg(test)]
async fn run_preparation_gate_if_armed(
    runtime_root: &std::path::Path,
    cancellation: &crate::runtime::cancellation::CancellationSignal,
) -> Result<(), crate::capabilities::CapabilityPreparationError> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

    if let Some(gate) = registered_preparation_gate(runtime_root) {
        return gate.run(cancellation).await;
    }
    let Some(path) = std::env::var_os(TEST_PREPARATION_GATE_ENV) else {
        return Ok(());
    };
    let stream = tokio::net::UnixStream::connect(path)
        .await
        .map_err(|error| {
            crate::capabilities::CapabilityPreparationError::PreparationSettled(format!(
                "the test preparation gate is unreachable: {error}"
            ))
        })?;
    let (read, mut write) = stream.into_split();
    write
        .write_all(b"entered-external-preparation\n")
        .await
        .map_err(|error| {
            crate::capabilities::CapabilityPreparationError::PreparationSettled(format!(
                "the test preparation gate announcement failed: {error}"
            ))
        })?;
    let mut lines = tokio::io::BufReader::new(read).lines();
    tokio::select! {
        biased;
        // Biased towards completion: when the test released the gate the
        // step completes even if the cancellation already fired, modelling
        // the racy external step the settlement checks exist for.
        line = lines.next_line() => match line {
            Ok(Some(line)) if line == "release" => Ok(()),
            other => Err(
                crate::capabilities::CapabilityPreparationError::PreparationSettled(format!(
                    "the test preparation gate closed without a release: {other:?}"
                )),
            ),
        },
        () = cancellation.cancelled() => Err(
            crate::capabilities::CapabilityPreparationError::PreparationSettled(
                "the preparation cancellation settled the gated external step".to_owned(),
            ),
        ),
    }
}

/// The inert non-test twin of the gate: external preparation runs no
/// test seam in a shipped build.
#[cfg(not(test))]
#[allow(clippy::unused_async)] // async only to match the test seam's shape
async fn run_preparation_gate_if_armed(
    _runtime_root: &std::path::Path,
    _cancellation: &crate::runtime::cancellation::CancellationSignal,
) -> Result<(), crate::capabilities::CapabilityPreparationError> {
    Ok(())
}

/// Materializes the child's frozen Skill packages and returns the catalog
/// entries with their locations remapped onto the child's own copies.
fn materialize_frozen_skills(
    spec: &crate::runtime::subagent::ipc::SubagentChildSpec,
) -> Result<Vec<crate::skills::SkillCatalogEntry>, LocalRuntimeError> {
    let root = spec.runtime_root.join("skills");
    let mut entries = Vec::with_capacity(spec.resolved.skills.len());
    for skill in &spec.resolved.skills {
        let destination = root.join(skill.binding.skill_id.as_str());
        let location = crate::skills::materialization::materialize_skill(
            &skill.binding,
            &skill.source_root,
            &skill.files,
            &destination,
        )
        .map_err(|error| LocalRuntimeError::Capability {
            detail: error.to_string(),
        })?;
        entries.push(crate::skills::SkillCatalogEntry {
            location,
            ..skill.catalog_entry.clone()
        });
    }
    Ok(entries)
}

/// Projects the frozen specification into the child's selected-only
/// capability materialization plan.
fn selected_capability_plan(
    spec: &crate::runtime::subagent::ipc::SubagentChildSpec,
) -> crate::capabilities::SelectedCapabilityPlan {
    use crate::runtime::subagent::ResolvedSubagentTool;
    let mut plan = crate::capabilities::SelectedCapabilityPlan::default();
    for tool in &spec.resolved.tools {
        match tool {
            ResolvedSubagentTool::Builtin { .. } => {}
            ResolvedSubagentTool::Mcp {
                server_id,
                name,
                identity,
                ..
            } => plan.mcp_tools.push(crate::capabilities::SelectedMcpTool {
                server_id: server_id.clone(),
                name: name.clone(),
                identity: identity.clone(),
            }),
        }
    }
    plan
}

/// Builds the child's exact base registry from its frozen specification.
fn subagent_child_registry(
    resolved: &crate::runtime::subagent::ResolvedSubagentSpec,
) -> Result<ToolRegistry, LocalRuntimeError> {
    // The exact parent-frozen definitions — identity, description, input
    // schema, and all three invocation-policy axes — never just their
    // names. Rebuilding a definition from a default policy table here would
    // silently substitute different semantics for the ones the invoking
    // generation admitted.
    let builtin: Vec<ToolDefinition> = resolved
        .tools
        .iter()
        .filter(|tool| matches!(tool, ResolvedSubagentTool::Builtin { .. }))
        .map(|tool| tool.definition().clone())
        .collect();
    let mut registry = ToolRegistry::new();
    crate::tools::native::register_subagent_child_tools(&mut registry, &builtin).map_err(
        |error| LocalRuntimeError::NativeTools {
            detail: format!("{error:?}"),
        },
    )?;
    Ok(registry)
}

/// The subagent child's resource loader.
///
/// A child never discovers resources: its whole generation was frozen by the
/// invoking parent generation. The loader therefore replays exactly the
/// frozen project instruction chain, instruction document, and Skill catalog
/// alongside a fresh base-only capability candidate, and reads no
/// configuration file, no `AGENTS.md` ancestor chain, and no Skill root. The
/// structure is the guarantee: there is no filesystem discovery path here to
/// be reached.
struct FrozenSubagentResourceLoader {
    resources: Arc<RuntimeResourceSnapshot>,
}

impl RuntimeResourceLoader for FrozenSubagentResourceLoader {
    fn prepare<'a>(
        &'a self,
        capability: &'a CapabilityCoordinator,
    ) -> BoxFuture<'a, Result<PreparedRuntimeResources, RuntimeResourceLoadError>> {
        Box::pin(async move {
            let candidate = capability.prepare_base_only_candidate().map_err(|error| {
                RuntimeResourceLoadError::new(format!(
                    "cannot prepare base capability resources: {error}"
                ))
            })?;
            Ok(PreparedRuntimeResources::new(
                self.resources.project_context_files().to_vec(),
                self.resources.agent_profile().map(str::to_owned),
                self.resources.context_assembly().clone(),
                candidate,
            ))
        })
    }
}

/// Loads one named-subagent catalog from the current configuration document.
///
/// Parent-side resource composition owns every filesystem read here: the
/// instruction document and each explicit project-instruction file are read
/// and frozen into the definition, so the child never resolves a path or
/// walks an ancestor of its own.
fn load_subagent_catalog(
    workspace: &Path,
    document: &SubagentsDocument,
) -> Result<SubagentCatalog, RuntimeResourceLoadError> {
    let mut definitions = Vec::with_capacity(document.definitions.len());
    for (name, agent) in &document.definitions {
        let instructions_source = resolve_workspace_path(workspace, &agent.instructions_file);
        let instructions = read_resource(&instructions_source, name.as_str(), "instructionsFile")?;
        let mut files = Vec::with_capacity(agent.agents_md.files.len());
        for file in &agent.agents_md.files {
            let path = resolve_workspace_path(workspace, file);
            let content = read_resource(&path, name.as_str(), "agentsMd.files")?;
            files.push(ProjectContextFile { path, content });
        }
        definitions.push(
            SubagentDefinition::new(
                name.clone(),
                agent.description.clone(),
                instructions,
                instructions_source,
                agent.model.clone(),
                agent
                    .execution_deadline()
                    .map_err(RuntimeResourceLoadError::new)?,
                agent.tools.selectors(),
                agent.skills.clone(),
                SubagentProjectInstructionPolicy {
                    inherit: agent.agents_md.inherit,
                    files,
                },
                agent.worktree.to_policy(),
            )
            .map_err(|error| RuntimeResourceLoadError::new(error.to_string()))?,
        );
    }
    SubagentCatalog::new(definitions)
        .map_err(|error| RuntimeResourceLoadError::new(error.to_string()))
}

/// Loads and compiles exactly the configured Workflow definitions.
///
/// The configured id is the only filesystem identity: a registered `id` is
/// read from `.agents/workflows/{id}.yaml`. Directory contents are never
/// scanned, so an unregistered YAML file cannot become model-visible by
/// accident. Compilation happens before the candidate reaches the runtime
/// resource publication boundary.
fn load_workflow_catalog(
    workspace: &Path,
    document: &WorkflowsDocument,
    workflow_profiles: &BTreeSet<crate::runtime::subagent::SubagentName>,
) -> Result<WorkflowCatalog, RuntimeResourceLoadError> {
    let mut programs = Vec::with_capacity(document.definitions.len());
    for id in &document.definitions {
        let path = workspace_workflow_path(workspace, id);
        let bytes = std::fs::read(&path).map_err(|error| {
            RuntimeResourceLoadError::new(format!(
                "cannot read registered workflow {id} at {}: {error}",
                path.display()
            ))
        })?;
        if bytes.len() > MAX_WORKFLOW_BYTES {
            return Err(RuntimeResourceLoadError::new(format!(
                "workflow {id} at {} exceeds the {MAX_WORKFLOW_BYTES}-byte bound",
                path.display()
            )));
        }
        let definition: WorkflowDefinition = serde_yaml::from_slice(&bytes).map_err(|error| {
            RuntimeResourceLoadError::new(format!(
                "cannot deserialize registered workflow {id} at {}: {error}",
                path.display()
            ))
        })?;
        let program = WorkflowProgram::compile(id.clone(), definition, workflow_profiles).map_err(
            |error| {
                RuntimeResourceLoadError::new(format!(
                    "cannot compile registered workflow {id} at {}: {error}",
                    path.display()
                ))
            },
        )?;
        programs.push(program);
    }
    WorkflowCatalog::new(programs, document.main.clone()).map_err(|error| {
        RuntimeResourceLoadError::new(format!("cannot admit Workflow catalog: {error}"))
    })
}

/// Maps one explicitly registered Workflow identity to its one source file.
/// This is intentionally not a discovery helper: the caller must provide the
/// id from `workflows.definitions`.
fn workspace_workflow_path(workspace: &Path, id: &crate::runtime::workflow::WorkflowId) -> PathBuf {
    workspace
        .join(AGENT_RESOURCES_DIRECTORY)
        .join(WORKFLOW_RESOURCES_DIRECTORY)
        .join(format!("{}.yaml", id.as_str()))
}

/// Workflow `main` admission is model-facing capability admission. Include
/// those concrete Tool names in the normal optional built-in default set so
/// a registered Workflow is available without duplicating its id in the
/// unrelated `defaultTools` selector. Explicit `--no-tools`,
/// `--no-builtin-tools`, or a strict `--tools` allowlist still has the
/// existing higher-priority meaning.
fn default_tools_with_workflows(
    default_tools: &[String],
    workflows: &WorkflowCatalog,
) -> Vec<String> {
    let mut result = default_tools.to_vec();
    for id in workflows.main() {
        if !result.iter().any(|name| name == id.as_str()) {
            result.push(id.to_string());
        }
    }
    result
}

fn read_resource(
    path: &Path,
    agent: &str,
    field: &str,
) -> Result<String, RuntimeResourceLoadError> {
    let bytes = std::fs::read(path).map_err(|error| {
        RuntimeResourceLoadError::new(format!(
            "cannot read subagents.definitions.{agent}.{field} {}: {error}",
            path.display()
        ))
    })?;
    let content = String::from_utf8(bytes).map_err(|error| {
        RuntimeResourceLoadError::new(format!(
            "subagents.definitions.{agent}.{field} {} is not UTF-8: {error}",
            path.display()
        ))
    })?;
    Ok(match content.strip_prefix('\u{feff}') {
        Some(without_bom) => without_bom.to_owned(),
        None => content,
    })
}

/// Admits the prepared catalog against the very capability/Skill/model
/// authority of the candidate generation that will publish it.
fn validate_subagent_catalog(
    prepared: &PreparedRuntimeResources,
    models: &ModelBindingRegistry,
) -> Result<(), RuntimeResourceLoadError> {
    let candidate = prepared.capability_candidate();
    let skills = crate::skills::SkillSnapshot::new(candidate.skill_packages().to_vec());
    SubagentResolver::validate_catalog(
        prepared.subagent_catalog(),
        candidate.available_tools(),
        candidate.availability(),
        &skills,
        models,
    )
    .map_err(|(agent, error)| {
        RuntimeResourceLoadError::new(format!("subagents.definitions.{agent}: {error}"))
    })
}

/// Rejects a model-facing Workflow id that is already used by another
/// capability in the same candidate generation. The active Tool selection can
/// hide a duplicate under `noTools`, but hiding it must not turn an identity
/// collision into a valid configuration.
fn validate_workflow_tool_name_collisions(
    candidate: &crate::capabilities::PreparedCapabilityCandidate,
    workflows: &WorkflowCatalog,
) -> Result<(), RuntimeResourceLoadError> {
    for workflow_id in workflows.main() {
        let expected_id = format!("tool-workflow-{workflow_id}");
        if workflow_id.as_str() == crate::tools::native::SUBAGENT_TOOL_NAME {
            return Err(RuntimeResourceLoadError::new(format!(
                "Workflow id {workflow_id:?} is reserved by the native subagent Tool"
            )));
        }
        if let Some(conflict) = candidate.available_tools().tools().iter().find(|tool| {
            tool.definition.name == workflow_id.as_str()
                && tool.definition.id.as_str() != expected_id
        }) {
            return Err(RuntimeResourceLoadError::new(format!(
                "Workflow id {workflow_id:?} collides with Tool {} ({})",
                conflict.definition.name, conflict.definition.id
            )));
        }
    }
    Ok(())
}

/// The shared semantic composition of one local runtime (Issue #61).
///
/// This is the single assembly point of the model catalog/session/tool/
/// capability/context pieces. It owns exactly the semantic owners of the
/// process — one `ConversationToolRuntime`, one `CapabilityCoordinator`,
/// and one `ConversationRuntime` — and nothing protocol-shaped. The
/// conversation runtime is constructed **inactive**; the final paths over
/// this core are the interactive runtime (which binds a Runtime Client
/// host), the subagent-child runtime (which binds that host and adds a
/// bounded parent-observation subscriber), and the headless runtime (which
/// activates without any Runtime Client host).
pub struct LocalConversationCore {
    runtime: ConversationRuntime,
    tool_runtime: ConversationToolRuntime,
    capability: CapabilityCoordinator,
    workflow_output: Option<Arc<WorkflowOutputLatch>>,
}

impl std::fmt::Debug for LocalConversationCore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalConversationCore")
            .field("conversation_id", self.tool_runtime.conversation_id())
            .finish_non_exhaustive()
    }
}

impl LocalConversationCore {
    /// Composes the shared semantic runtime from explicit startup paths.
    ///
    /// The runtime is left **inactive**: the caller must finish through
    /// [`LocalConversationCore::into_interactive`],
    /// [`LocalConversationCore::into_subagent_child_with_route`], or
    /// [`LocalConversationCore::into_headless`] (or, for low-level
    /// composition callers, activate the runtime explicitly). Prefer the
    /// final composition paths of
    /// [`LocalConversationRuntime`]/[`HeadlessConversationRuntime`], which
    /// return already-active handles.
    ///
    /// # Errors
    ///
    /// Returns the first composition failure. Every failure happens before
    /// any protocol output exists.
    pub async fn compose(
        paths: &LocalRuntimePaths,
        dependencies: &LocalRuntimeDependencies,
    ) -> Result<Self, LocalRuntimeError> {
        // This low-level path has no SessionCatalog control surface. It uses
        // one deterministic standalone lineage so repeated composition over
        // the same runtime root still recovers the same durable conversation.
        let config_bytes = read_file(&paths.config)?;
        let runtime_config = CurrentRuntimeConfig::from_jsonc_slice(&config_bytes)?;
        let registry = load_model_registry(paths, dependencies)?;
        Self::compose_from_config(
            paths,
            dependencies,
            registry,
            runtime_config.clone(),
            SessionPersistentState {
                model: runtime_config.model.clone(),
            },
            ConversationId::new("conversation-standalone"),
            paths.artifacts_root(),
        )
        .await
    }

    /// Composes one selected native `SessionNode`'s linear conversation from
    /// current runtime configuration plus the Session-local persistent state.
    /// This method never reads or writes `SessionCatalog` state.
    #[allow(clippy::too_many_lines)]
    pub(crate) async fn compose_from_config(
        paths: &LocalRuntimePaths,
        dependencies: &LocalRuntimeDependencies,
        registry: ModelBindingRegistry,
        runtime_config: CurrentRuntimeConfig,
        session_state: SessionPersistentState,
        conversation_id: ConversationId,
        artifacts_root: PathBuf,
    ) -> Result<Self, LocalRuntimeError> {
        // The current runtime default was validated by the composition
        // caller before any first-Session publication. Validate it here too
        // for direct low-level callers, while the selected durable Session
        // model remains an independent Session-local choice.
        SessionModelState::new(registry.clone(), runtime_config.model.clone())?;
        let model = SessionModelState::new(registry.clone(), session_state.model.clone())?;

        // 5-6. The conversation identity authority and the one conversation
        // tool runtime (workspace, runtime-private artifact root, canonical
        // mailbox, background registry, base authorized environment).
        let base_environment = runtime_config.tool_environment()?;
        let mut tool_runtime_config = crate::tools::runtime::ConversationRuntimeConfig::new(
            &paths.workspace,
            artifacts_root.clone(),
        );
        tool_runtime_config.environment = Some(base_environment.clone());
        let tool_runtime =
            ConversationToolRuntime::from_config(conversation_id.clone(), tool_runtime_config)
                .map_err(|error| LocalRuntimeError::ToolRuntime {
                    detail: format!("{error:?}"),
                })?;

        // 7-8. The base tool registry with the explicit native composition,
        // using *this* conversation's background registry for the
        // `execution` intrinsic and this conversation's subagent
        // registry for the `subagent` intrinsic (Issue #60).
        // The named-subagent catalog of the launch generation. It is
        // loaded before the base registry, because the `subagent`
        // intrinsic's model-facing description is generated from exactly
        // the catalog this generation admits.
        let subagent_catalog = load_subagent_catalog(&paths.workspace, &runtime_config.subagents)
            .map_err(|error| LocalRuntimeError::Capability {
            detail: error.to_string(),
        })?;
        let main_admission = runtime_config
            .subagents
            .main
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        let workflow_admission = runtime_config
            .subagents
            .workflow
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        let main_catalog = subagent_catalog
            .admitted(&main_admission)
            .map_err(|error| LocalRuntimeError::Capability {
                detail: error.to_string(),
            })?;
        let workflows = load_workflow_catalog(
            &paths.workspace,
            &runtime_config.workflows,
            &workflow_admission,
        )
        .map_err(|error| LocalRuntimeError::Capability {
            detail: error.to_string(),
        })?;
        let default_tools = default_tools_with_workflows(&runtime_config.default_tools, &workflows);
        //
        // The frozen model timeout policy is resolved once here so the
        // parent runtime and every launched subagent child share exactly
        // the same deadlines (Issue #138): the child inherits the policy
        // through its typed startup specification and applies it locally.
        let model_timeout_policy = runtime_config.timeout_policy()?;
        let subagents = crate::runtime::subagent::SubagentRegistry::new(
            crate::runtime::subagent::SubagentRegistryConfig {
                conversation_id: tool_runtime.conversation_id().clone(),
                agent_id: runtime_config.agent_id.clone(),
                mailbox: tool_runtime.mailbox(),
                clock: Arc::new(crate::runtime::types::SystemClock),
                monotonic_clock: Arc::new(crate::runtime::SystemMonotonicClock::new()),
                spawn: crate::runtime::subagent::SubagentSpawnPlan {
                    program: match dependencies.child_program.clone() {
                        Some(program) => program,
                        None => std::env::current_exe().map_err(|error| LocalRuntimeError::Io {
                            path: PathBuf::from("<current exe>"),
                            detail: error.to_string(),
                        })?,
                    },
                    // Child conversation stores and their disposable
                    // physical incarnations live below the launch's one
                    // runtime-private root. Keeping this identity namespace
                    // independent of the selected Session's artifact path
                    // lets a later generic inspection attach by
                    // `child_conversation_id` alone.
                    runtime_root: paths.runtime_root.clone(),
                    model_timeout_policy,
                    agent_status: runtime_config.agent_status.clone(),
                    context: runtime_config.context_policy(),
                },
                workspace: SubagentWorkspaceManager::new(
                    tool_runtime.workspace().root(),
                    tool_runtime.artifacts().root(),
                ),
                // Launch-scoped: capacity belongs to the live registry, and
                // resource reload deliberately never resizes it.
                max_active: runtime_config.subagents.max_concurrent,
            },
        );
        let workflow_runtime =
            WorkflowRuntime::new(subagents.clone(), tool_runtime.durable_store());
        let mut base_registry = ToolRegistry::new();
        let native_resources = NativeToolResources {
            background: tool_runtime.background().clone(),
            subagents: Some(subagents.clone()),
            subagent_catalog: main_catalog,
        };
        register_native_tools(
            &mut base_registry,
            native_resources.clone(),
            runtime_config.native_tools.to_policies(),
        )
        .map_err(|error| LocalRuntimeError::NativeTools {
            detail: format!("{error:?}"),
        })?;
        crate::tools::native::register_workflow_tools(
            &mut base_registry,
            &workflow_runtime,
            &workflows,
        )
        .map_err(|error| LocalRuntimeError::NativeTools {
            detail: format!("{error:?}"),
        })?;

        let mut skill_discovery =
            SkillDiscoveryConfig::default_for_workspace(tool_runtime.workspace());
        if paths.no_skills {
            skill_discovery.automatic_roots.clear();
        }
        // Relative Skill paths resolve against the *canonical* Workspace
        // root, not the raw `--workspace` spelling: `--workspace w` would
        // otherwise produce a candidate root relative to the process cwd
        // while every consumer of the published location resolves against
        // the canonical root. Discovery canonicalizes accepted roots too;
        // this keeps the input meaningful rather than merely recoverable.
        let workspace_root = tool_runtime.workspace().root().to_path_buf();
        skill_discovery.explicit_paths.extend(
            runtime_config
                .skills
                .iter()
                .map(|path| resolve_workspace_path(&workspace_root, path)),
        );
        skill_discovery.explicit_paths.extend(
            paths
                .skill_paths
                .iter()
                .map(|path| resolve_workspace_path(&workspace_root, path)),
        );

        // 9. Composition owns CLI/config precedence resolution. The
        // coordinator receives only this already-resolved activation policy
        // and applies it to the available capability registrations.
        let capability = CapabilityCoordinator::new(CapabilityCoordinatorConfig {
            conversation_id: tool_runtime.conversation_id().clone(),
            workspace: tool_runtime.workspace().clone(),
            base_tool_registry: Arc::new(base_registry),
            tool_activation: ToolActivationPolicy {
                default_tools: Some(default_tools),
                no_builtin_tools: paths.no_builtin_tools,
                no_tools: paths.no_tools,
                tools: paths.tools.clone(),
                exclude_tools: paths.exclude_tools.clone(),
            },
            skill_discovery,
            mcp_servers: runtime_config.mcp_bindings()?,
            base_environment,
            environment_store_root: paths.environment_store_root_for(&conversation_id),
        })
        .map_err(|error| LocalRuntimeError::Capability {
            detail: format!("{error:?}"),
        })?;

        // 10-11. Build one complete candidate off-side before publishing any
        // capability or resource state. The named-subagent catalog, its two
        // admission domains, the registered Workflow programs, project
        // instructions, and the capability candidate are validated as one
        // coherent generation. Optional-source failures (individual MCP
        // servers, including managed Python packages) remain typed
        // availability state inside `prepare_candidate` (Issue #81); base
        // capability failures remain fatal.
        let candidate = capability.prepare_candidate().await.map_err(|error| {
            LocalRuntimeError::Capability {
                detail: format!("{error:?}"),
            }
        })?;
        validate_workflow_tool_name_collisions(&candidate, &workflows).map_err(|error| {
            LocalRuntimeError::Capability {
                detail: error.to_string(),
            }
        })?;
        let prepared = PreparedRuntimeResources::new(
            load_project_context_files(tool_runtime.workspace().root()).map_err(|error| {
                LocalRuntimeError::Capability {
                    detail: error.to_string(),
                }
            })?,
            None,
            crate::context::ContextAssembly::new(),
            candidate,
        )
        .with_subagent_catalog(subagent_catalog)
        .with_subagent_admissions(main_admission, workflow_admission)
        .with_workflow_catalog(workflows);
        validate_subagent_catalog(&prepared, &registry).map_err(|error| {
            LocalRuntimeError::Capability {
                detail: error.to_string(),
            }
        })?;

        // This is the sole startup publication boundary. Every value below
        // was admitted against the exact candidate that is now committed,
        // so startup cannot expose a capability generation whose subagent or
        // Workflow catalog belongs to another configuration generation.
        let (candidate, resource_data) = prepared.into_parts();
        let capability_snapshot =
            capability
                .commit(candidate)
                .map_err(|error| LocalRuntimeError::Capability {
                    detail: format!("{error:?}"),
                })?;
        let resources = Arc::new(RuntimeResourceSnapshot::from_prepared(
            RuntimeResourceRevision::new(1),
            resource_data,
            capability_snapshot,
        ));
        let resource_loader: Arc<dyn RuntimeResourceLoader> =
            Arc::new(LocalRuntimeResourceLoader::new(
                paths.clone(),
                native_resources,
                registry,
                workflow_runtime,
            ));

        // Reopening a selected lineage must re-supply only its immutable
        // bootstrap prefix to ConversationRuntime. Later canonical turns are
        // recovered by the runtime itself; passing the whole transcript here
        // would incorrectly change the store's bootstrap identity.
        let initial_messages = tool_runtime
            .durable_store()
            .load_bootstrap_history()
            .map_err(|error| LocalRuntimeError::ToolRuntime {
                detail: error.to_string(),
            })?;

        // 12-13. The context policy/estimator/status pieces and the one
        // authoritative conversation runtime coordinator, constructed
        // **inactive**: the final composition path activates it after the
        // optional Runtime Client host binds.
        let runtime = ConversationRuntime::new(RuntimeConversationConfig {
            agent_id: runtime_config.agent_id.clone(),
            model,
            approval_mode: runtime_config.approval_mode,
            model_timeout_policy,
            context: ConversationContextConfig {
                policy: runtime_config.context_policy(),
                estimator: Arc::clone(&dependencies.estimator),
                status_engine: AgentStatusEngine::new(
                    runtime_config.agent_status.clone(),
                    Arc::new(crate::context::SystemClock),
                ),
            },
            tool_runtime: tool_runtime.clone(),
            capability: capability.clone(),
            resources,
            resource_loader,
            clock: None,
            initial_messages,
            subagents: Some(subagents),
            workflow_output: None,
        })?;

        Ok(Self {
            runtime,
            tool_runtime,
            capability,
            workflow_output: None,
        })
    }

    /// Composes the **subagent child** core from its typed startup
    /// specification (Issue #60).
    ///
    /// This is the same semantic stack as [`LocalConversationCore::compose`]
    /// — the real `ConversationRuntime`, Agent Loop, Context Assembly, Tool
    /// Plane, and `ModelAdapter` — with the child-specific composition
    /// differences made explicit and deny-by-construction:
    ///
    /// - the startup input is the typed [`SubagentChildSpec`], never a
    ///   current runtime configuration file: the child never opens
    ///   `rustx.jsonc` and never looks its own agent name up;
    /// - the base tool registry is exactly the Builtin capability set the
    ///   parent's resolution froze, registered through
    ///   [`register_subagent_child_tools`]; nothing is added, substituted,
    ///   or force-activated; recursive `subagent` is structurally
    ///   unregistrable, while `ask_user` is materialized only when the frozen
    ///   definition explicitly selected it;
    /// - the capability plane is **base-only**: no Skill discovery, no
    ///   Python/Node environments, and no MCP servers; it never opens or
    ///   creates Python package storage (Issue #81), so a broken Python
    ///   store location cannot fail child composition;
    /// - the Skill catalog is exactly the parent-resolved allowlist, handed
    ///   over by value, with progressive disclosure preserved: only catalog
    ///   metadata is frozen, never a `SKILL.md` body;
    /// - project instructions are exactly the parent-frozen chain. The
    ///   child performs **no** ancestor discovery of its own, which is what
    ///   makes the boundary correct once a child's filesystem ancestry can
    ///   differ from the parent workspace;
    /// - the definition's instruction document, composed with the
    ///   runtime-owned terminal-mode-aware final-report rule (Issue #192),
    ///   is the immutable `AgentProfile` System authority and canonical
    ///   history starts empty;
    /// - the durable authority is the stable child store beside the physical
    ///   spawn-incarnation directory, disjoint from the parent's store. The
    ///   physical root remains disposable execution/artifact state.
    ///
    /// The caller (the child driver) still owns activation: the returned
    /// core is inert until `into_subagent_child_with_route` or `into_headless`.
    ///
    /// # Errors
    ///
    /// Returns the first composition failure, exactly like the ordinary
    /// composition path.
    #[allow(clippy::too_many_lines)] // one coherent child composition pipeline
    pub(crate) async fn compose_subagent_child(
        spec: &crate::runtime::subagent::ipc::SubagentChildSpec,
        dependencies: &LocalRuntimeDependencies,
        preparation: &ChildPreparation,
    ) -> Result<Self, LocalRuntimeError> {
        // 1-4. The child's model authority, materialized from the
        // parent-frozen resolved invocation. There is deliberately no model
        // catalog step here: `models.jsonc` is mutable, and reopening it
        // would let a catalog edit between the parent's freeze and this
        // composition silently change the child's provider binding,
        // protocol, context window, output budget, reasoning semantics,
        // request parameters, compat metadata, or effective capabilities —
        // or fail to resolve a model that was valid when the child was
        // authorized. The only work done here is physical: adapter
        // construction and credential resolution through this process's own
        // ordinary credential boundary.
        let model =
            SessionModelState::frozen(&spec.resolved.model, dependencies.credentials.as_ref())?;

        // 5-6. The child conversation tool runtime over the authoritative
        // project workspace selected by the parent and the exact
        // spawn-incarnation-private runtime root. The child authorizes no
        // environment entries. Its durable Message Ledger/Event Journal is
        // deliberately one level above that physical root: execution
        // artifacts can be discarded after settlement while the conversation
        // identity remains inspectable.
        let base_environment = ToolEnvironment::from_authorized(std::iter::empty())
            .map_err(CurrentRuntimeConfigError::Environment)
            .map_err(LocalRuntimeError::RuntimeConfig)?;
        let mut runtime_config = crate::tools::runtime::ConversationRuntimeConfig::new(
            &spec.workspace_snapshot.logical_workspace,
            spec.runtime_root.join("artifacts"),
        );
        runtime_config.environment = Some(base_environment.clone());
        let durable_store_path = spec
            .runtime_root
            .parent()
            .ok_or_else(|| LocalRuntimeError::ToolRuntime {
                detail: format!(
                    "physical child runtime root {} has no stable semantic parent",
                    spec.runtime_root.display()
                ),
            })?
            .join("conversation.sqlite");
        let durable_store = Arc::new(
            SqliteConversationStore::open(spec.child_conversation_id.clone(), &durable_store_path)
                .map_err(|error| LocalRuntimeError::ToolRuntime {
                    detail: format!(
                        "open child durable conversation store {}: {error}",
                        durable_store_path.display()
                    ),
                })?,
        );
        runtime_config.durable_binding = Some(ConversationStoreBinding::new(durable_store));
        let tool_runtime = ConversationToolRuntime::from_config(
            spec.child_conversation_id.clone(),
            runtime_config,
        )
        .map_err(|error| LocalRuntimeError::ToolRuntime {
            detail: format!("{error:?}"),
        })?;

        // 7. The exact frozen Skill packages are materialized into the
        // child-private runtime root and their frozen `SkillVersionId` is
        // re-proven over the materialized bytes. Discovery is never
        // restarted, and the model-visible location is remapped onto the
        // child's own copy so the frozen identity stays authoritative even
        // once the child's filesystem ancestry differs from the parent's.
        let skills = materialize_frozen_skills(spec)?;

        // 8. The deny-by-construction base registry: exactly the Builtin
        // capabilities the parent's resolution authorized, with their exact
        // admitted definitions.
        let base_registry = subagent_child_registry(&spec.resolved)?;

        // 9. The capability plane sees exactly the frozen selected MCP
        // server bindings — never a configured set the child could widen.
        // Managed Python packages cross as ordinary frozen MCP bindings
        // (Issue #174); the child never opens Python store state itself.
        let plan = selected_capability_plan(spec);
        let capability = CapabilityCoordinator::new(CapabilityCoordinatorConfig {
            conversation_id: tool_runtime.conversation_id().clone(),
            workspace: tool_runtime.workspace().clone(),
            base_tool_registry: Arc::new(base_registry),
            tool_activation: ToolActivationPolicy::default(),
            skill_discovery: SkillDiscoveryConfig::default(),
            mcp_servers: spec.resolved.materialization.mcp_servers.clone(),
            base_environment,
            environment_store_root: spec.runtime_root.join("environments"),
        })
        .map_err(|error| LocalRuntimeError::Capability {
            detail: format!("{error:?}"),
        })?;
        // 10-11. Materialization. A child with no external requirement takes
        // the deterministic base-only path it always did; a child with one
        // takes the selected-only realization path, which is cancellable
        // owned work: cancellation and parent loss settle it instead of
        // finishing a long MCP connect or uv build for an owner that is
        // already gone, and every preparatory supervised unit observes the
        // one preparation cancellation authority.
        let candidate = if plan.is_empty() && !preparation_gate_armed(&spec.runtime_root) {
            capability.prepare_base_only_candidate().map_err(|error| {
                LocalRuntimeError::Capability {
                    detail: format!("{error:?}"),
                }
            })?
        } else {
            let cancellation = preparation.cancellation();
            let step = async {
                run_preparation_gate_if_armed(&spec.runtime_root, &cancellation).await?;
                capability
                    .prepare_selected_candidate(&plan, &cancellation)
                    .await
            };
            match preparation.guard(step).await {
                Ok(candidate) => {
                    // The step may complete after a settlement authority
                    // already won (the race is inherent to the final
                    // unguarded stretch): a settled preparation must never
                    // publish a startable candidate. Retire its physical
                    // runtimes to settlement and report the settlement.
                    if preparation.is_settled() {
                        candidate.retire_uncommitted().await;
                        capability.cancel_conversation_preparation();
                        return Err(LocalRuntimeError::Capability {
                            detail: format!(
                                "{:?}",
                                crate::capabilities::CapabilityPreparationError::PreparationSettled(
                                    "a settlement authority won before the candidate completed"
                                        .to_owned(),
                                )
                            ),
                        });
                    }
                    candidate
                }
                Err(error) => {
                    // Settlement fires the one preparation cancellation
                    // authority, so every MCP connect owner and Python/uv
                    // build the step already spawned is physically
                    // cancelled and the guard awaited its settlement.
                    // Cancelling the coordinator's preparation root
                    // additionally covers any owner the step's own
                    // authority did not reach.
                    capability.cancel_conversation_preparation();
                    return Err(LocalRuntimeError::Capability {
                        detail: format!("{error:?}"),
                    });
                }
            }
        };
        capability
            .commit(candidate)
            .map_err(|error| LocalRuntimeError::Capability {
                detail: format!("{error:?}"),
            })?;
        // The child's project instructions and Skill catalog are the
        // parent-frozen values, by value. `load_project_context_files` is
        // deliberately never called on this path: the child observes only
        // what its invoking generation froze. The AgentProfile authority is
        // the definition's instruction document composed with the
        // runtime-owned terminal-mode-aware final-report rule (Issue #192).
        let resources = Arc::new(
            RuntimeResourceSnapshot::new(
                RuntimeResourceRevision::new(1),
                spec.resolved.project_instructions.clone(),
                Some(crate::runtime::subagent::compose_child_agent_profile(
                    &spec.resolved.instructions,
                    &spec.terminal,
                )),
                crate::context::ContextAssembly::new(),
                capability.current_snapshot(),
            )
            .with_frozen_skill_catalog(&skills),
        );
        let workflow_output_latch = match &spec.terminal {
            crate::runtime::subagent::ipc::ChildTerminalMode::Normal => None,
            crate::runtime::subagent::ipc::ChildTerminalMode::WorkflowOutput { output_schema } => {
                Some(Arc::new(
                    WorkflowOutputLatch::new(output_schema.clone())
                        .map_err(|detail| LocalRuntimeError::Capability { detail })?,
                ))
            }
        };
        let workflow_output = workflow_output_latch
            .clone()
            .map(|latch| -> Arc<dyn crate::runtime::workflow::WorkflowOutputTerminal> { latch });
        let resource_loader: Arc<dyn RuntimeResourceLoader> =
            Arc::new(FrozenSubagentResourceLoader {
                resources: Arc::clone(&resources),
            });

        // 12-13. The one child conversation runtime. The definition's
        // instructions enter the request-time AgentProfile System section,
        // never canonical history.
        let runtime = ConversationRuntime::new(RuntimeConversationConfig {
            agent_id: spec.child_agent_id.clone(),
            model,
            approval_mode: spec.approval_mode,
            model_timeout_policy: spec.model_timeout_policy,
            context: ConversationContextConfig {
                policy: spec.context,
                estimator: Arc::clone(&dependencies.estimator),
                status_engine: AgentStatusEngine::new(
                    spec.agent_status.clone(),
                    Arc::new(crate::context::SystemClock),
                ),
            },
            tool_runtime: tool_runtime.clone(),
            capability: capability.clone(),
            resources,
            resource_loader,
            clock: None,
            initial_messages: Vec::new(),
            // A child runtime has no subagent registry: recursive
            // delegation is absent by construction.
            subagents: None,
            workflow_output,
        })?;

        Ok(Self {
            runtime,
            tool_runtime,
            capability,
            workflow_output: workflow_output_latch,
        })
    }

    /// The one conversation runtime coordinator of this composition.
    #[must_use]
    pub const fn runtime(&self) -> &ConversationRuntime {
        &self.runtime
    }

    /// The one conversation tool runtime of this composition.
    #[must_use]
    pub const fn tool_runtime(&self) -> &ConversationToolRuntime {
        &self.tool_runtime
    }

    /// The one capability coordinator of this composition.
    #[must_use]
    pub const fn capability(&self) -> &CapabilityCoordinator {
        &self.capability
    }

    /// The child-local Workflow terminal latch, when this core was composed
    /// for a Workflow-owned `AgentRun`.
    pub(crate) fn workflow_output(&self) -> Option<Arc<WorkflowOutputLatch>> {
        self.workflow_output.clone()
    }

    /// Finishes the composition as an **interactive** runtime: the Runtime
    /// Client host binds over the still-inactive runtime, then the runtime
    /// activates. Binding a host is a pre-activation composition decision,
    /// so this is the only path that may construct a host.
    ///
    /// # Errors
    ///
    /// Returns [`LocalRuntimeError::Host`] when the Runtime Client host
    /// cannot bind (a fresh core leaves no reason: no bridge exists and
    /// the runtime is inactive).
    pub fn into_interactive(self) -> Result<LocalConversationRuntime, LocalRuntimeError> {
        self.into_interactive_with_control(None)
    }

    /// Finishes composition with the native Session supervisor installed as
    /// the typed Runtime Client control seam.
    ///
    /// # Errors
    ///
    /// Returns [`LocalRuntimeError`] when the Runtime Client host cannot be
    /// bound over the inactive runtime.
    pub fn into_interactive_with_session_control(
        self,
        control: Arc<dyn RuntimeClientSessionControl>,
    ) -> Result<LocalConversationRuntime, LocalRuntimeError> {
        self.into_interactive_with_control(Some(control))
    }

    /// Binds the native Session supervisor as the Runtime Client control
    /// seam and leaves the runtime **inert**, for a caller that must commit
    /// durable state between binding and activation.
    ///
    /// # Errors
    ///
    /// Returns [`LocalRuntimeError`] when the Runtime Client host cannot be
    /// bound over the inactive runtime.
    pub(crate) fn into_bound_with_session_control(
        self,
        control: Arc<dyn RuntimeClientSessionControl>,
    ) -> Result<LocalConversationRuntime, LocalRuntimeError> {
        self.into_bound_with_control(Some(control))
    }

    fn into_interactive_with_control(
        self,
        control: Option<Arc<dyn RuntimeClientSessionControl>>,
    ) -> Result<LocalConversationRuntime, LocalRuntimeError> {
        let runtime = self.into_bound_with_control(control)?;
        runtime.activate();
        Ok(runtime)
    }

    /// Binds the Runtime Client host over the composed runtime and stops
    /// there: the returned runtime is **inert**.
    ///
    /// Binding and activation are separated so a caller with a durable
    /// commit of its own — the local product composition and its startup
    /// catalog transaction — can place that commit between them. Every
    /// fallible composition step is then on the pre-commit side, and the
    /// activation that follows the commit cannot fail.
    fn into_bound_with_control(
        self,
        control: Option<Arc<dyn RuntimeClientSessionControl>>,
    ) -> Result<LocalConversationRuntime, LocalRuntimeError> {
        // 14. The Runtime Client projection/control/attachment adapter over
        // that runtime. Binding is a pre-activation composition decision
        // (Issue #61): the runtime is still inert here, so the host's
        // initial snapshot is the runtime's real state at the activation
        // cut and no bootstrap fact can fabricate a live client event.
        let host = match control {
            Some(control) => RuntimeClientHost::new_with_session_control(
                RuntimeClientHostConfig {
                    runtime: self.runtime.clone(),
                    replay_limit: None,
                },
                control,
            )?,
            None => RuntimeClientHost::new(RuntimeClientHostConfig {
                runtime: self.runtime.clone(),
                replay_limit: None,
            })?,
        };

        Ok(LocalConversationRuntime { core: self, host })
    }

    /// Finishes composition for a child with a reliable route to the parent
    /// registry. The route is installed before activation and carries only
    /// the child's already-authoritative interaction facts.
    pub(crate) fn into_subagent_child_with_route(
        self,
        route: Arc<dyn InteractionRoute>,
    ) -> Result<
        (
            LocalConversationRuntime,
            Arc<crate::runtime::observation::PendingObservations>,
        ),
        LocalRuntimeError,
    > {
        let runtime = self.into_bound_with_control(None)?;
        runtime.runtime().install_interaction_route(route);
        let observations = runtime
            .runtime()
            .subscribe_observations()
            .map_err(|error| LocalRuntimeError::Observation {
                detail: error.to_string(),
            })?;
        runtime.activate();
        Ok((runtime, observations))
    }

    /// Finishes the composition as a **headless** runtime: the runtime
    /// activates with no Runtime Client host at all (Issue #60 subagents,
    /// every zero-client deployment). The semantic composition is exactly
    /// the one [`LocalConversationCore::compose`] builds.
    #[must_use]
    pub fn into_headless(self) -> HeadlessConversationRuntime {
        // The one explicit lifecycle boundary, without step 14.
        self.runtime.activate();
        HeadlessConversationRuntime { core: self }
    }
}

/// The native local product composition: one SessionCatalog/Graph owner plus
/// exactly one active linear `ConversationRuntime` and its Runtime Client host.
pub struct LocalSessionProduct {
    runtime: LocalConversationRuntime,
    supervisor: Arc<LocalSessionSupervisor>,
}

impl std::fmt::Debug for LocalSessionProduct {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalSessionProduct")
            .field("conversation_id", self.runtime.runtime().conversation_id())
            .finish_non_exhaustive()
    }
}

impl LocalSessionProduct {
    /// Loads the native catalog, resolves the Session this launch starts on,
    /// composes that `ConversationRuntime`, binds typed Session control, and
    /// activates the runtime before serving protocol input.
    ///
    /// The startup Session is an empty one unless
    /// [`LocalRuntimePaths::startup_session`] asks for the catalog's
    /// published active selection or names a persisted Session. Whichever
    /// it is, the catalog transition is planned first and committed once,
    /// after composition and host binding have succeeded, so a launch that
    /// fails changes no published catalog state at all — including a first
    /// launch, which publishes no catalog. The destination conversation
    /// database may be seeded before that commit; a conversation the
    /// catalog does not name is neither selectable nor resumable.
    ///
    /// # Errors
    ///
    /// Returns [`LocalRuntimeError`] when startup configuration, catalog
    /// loading, capability composition, runtime recovery, or host binding
    /// fails.
    pub async fn compose(
        paths: &LocalRuntimePaths,
        dependencies: &LocalRuntimeDependencies,
    ) -> Result<Self, LocalRuntimeError> {
        // The current runtime/project configuration and current ModelCatalog
        // are resolved before opening or creating durable Session state. A
        // failed first launch therefore cannot publish an invalid initial
        // Session-local model.
        let config_bytes = read_file(&paths.config)?;
        let runtime_config = CurrentRuntimeConfig::from_jsonc_slice(&config_bytes)?;
        let registry = load_model_registry(paths, dependencies)?;
        SessionModelState::new(registry.clone(), runtime_config.model.clone())?;
        let state = SessionPersistentState {
            model: runtime_config.model.clone(),
        };
        // A first launch builds the root Session in memory and publishes
        // nothing yet. `catalog.json` is written by the one startup
        // transaction below, together with whatever else this launch
        // decided — so a first launch that fails to compose leaves a
        // runtime root with no catalog at all. The seeded conversation
        // database it leaves behind is not published state: nothing names
        // it, so it is neither selectable nor resumable.
        let catalog = if let Some(catalog) = SessionCatalog::open_existing(&paths.runtime_root)? {
            catalog
        } else {
            SessionCatalog::create_unpublished(&paths.runtime_root, &state)?
        };
        // Startup is not a resume. A launch begins on an empty Session and
        // leaves every persisted Session as history reachable through
        // `/resume`; only an explicit request binds a persisted one. An
        // active Session that was never used is that empty Session already,
        // so repeated launches publish nothing and cannot accumulate empty
        // rows.
        //
        // A named Session takes the catalog transition `/resume` takes,
        // decided ahead of composition rather than published ahead of it.
        // It is *planned* here and committed at the end: composing the destination is what can still fail — a
        // Session whose recorded model no longer exists in `models.jsonc`,
        // a database that will not open — and a launch that fails must not
        // leave the active selection somewhere the user never asked for.
        // A replacement spawn that continues the active selection therefore
        // lands on it without naming it, and an unknown identity fails the
        // launch instead of quietly opening something else.
        //
        // `prepare_session` still seeds its destination database here. That
        // is not a published fact: a seeded conversation the catalog does
        // not name is unreachable — neither selectable nor resumable — so
        // an abandoned plan leaves an inert orphan and nothing else.
        let planned = match &paths.startup_session {
            StartupSession::Empty => {
                if catalog.active_is_unused()? {
                    catalog.plan_unchanged()
                } else {
                    let prepared = catalog.prepare_session(&state, &[])?;
                    catalog.plan_session(&prepared, SessionNodeOrigin::New)?
                }
            }
            StartupSession::ContinueActive => catalog.plan_unchanged(),
            StartupSession::Select { session, node } => {
                catalog.plan_select(session, node.as_ref())?
            }
            StartupSession::InspectConversation { .. } => {
                unreachable!("durable conversation inspection uses its dedicated composition path")
            }
        };
        // `--name` names the Session this launch bound, whichever one that
        // is. It is the startup form of `/name` and nothing more: naming is
        // metadata, so it can only follow a decision about where the launch
        // starts and can never be part of making it. A launch that names a
        // Session it also asked to continue therefore renames that Session,
        // exactly as typing `/name` in it would — and, like the selection
        // itself, only if the launch actually starts.
        let planned = match &paths.session_name {
            Some(name) => planned
                .with_name(name)
                .map_err(LocalRuntimeError::SessionCatalog)?,
            None => planned,
        };
        // The destination is read from the plan, not from the catalog on
        // disk: this is where the launch is about to compose.
        let (session_id, node, session_state) = planned
            .active_lineage()
            .map_err(LocalRuntimeError::SessionCatalog)?;
        let database_path = catalog.database_path(&session_id, &node.conversation_id);
        let artifacts_root = database_path
            .parent()
            .ok_or_else(|| {
                LocalRuntimeError::SessionCatalog(SessionError::Catalog {
                    detail: "active conversation database has no parent".to_owned(),
                })
            })?
            .to_path_buf();

        // Everything fallible happens against the planned destination and
        // before the catalog changes: composition, recovery, and the
        // Runtime Client host binding. The runtime is left inert.
        let default_model = runtime_config.model.clone();
        let core = LocalConversationCore::compose_from_config(
            paths,
            dependencies,
            registry,
            runtime_config,
            session_state,
            node.conversation_id,
            artifacts_root,
        )
        .await?;
        let supervisor = Arc::new(LocalSessionSupervisor::new(catalog, default_model));
        let runtime = core.into_bound_with_session_control(supervisor.clone())?;

        // The one catalog transaction of startup. Before this line the
        // catalog is byte-for-byte what the launch found; after it, the
        // published selection and the composed runtime describe the same
        // lineage.
        supervisor
            .commit_startup(planned)
            .await
            .map_err(LocalRuntimeError::SessionCatalog)?;

        // Past the commit, nothing may fail on its own terms: the lineage
        // check below compares the committed selection with the runtime
        // composed from that same plan, and activation is infallible.
        supervisor
            .install_runtime(runtime.runtime().clone())
            .await
            .map_err(LocalRuntimeError::SessionSupervisor)?;
        runtime.activate();
        Ok(Self {
            runtime,
            supervisor,
        })
    }

    /// The one active linear `ConversationRuntime`.
    #[must_use]
    pub const fn runtime(&self) -> &ConversationRuntime {
        self.runtime.runtime()
    }

    /// The native Session supervisor.
    #[must_use]
    pub fn supervisor(&self) -> &Arc<LocalSessionSupervisor> {
        &self.supervisor
    }

    /// The transport-neutral Runtime Client endpoint.
    #[must_use]
    pub fn endpoint(&self) -> RuntimeClientEndpoint {
        self.runtime.endpoint()
    }
}

/// The composed interactive local conversation runtime.
///
/// It owns the semantic owners of the process (see
/// [`LocalConversationCore`]) plus the Runtime Client host — the
/// projection/control adapter over the runtime — and the endpoint handed
/// to a transport is derived from that adapter.
pub struct LocalConversationRuntime {
    core: LocalConversationCore,
    host: RuntimeClientHost,
}

impl std::fmt::Debug for LocalConversationRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalConversationRuntime")
            .field("conversation_id", self.tool_runtime().conversation_id())
            .finish_non_exhaustive()
    }
}

impl LocalConversationRuntime {
    /// Composes the interactive runtime from explicit startup paths.
    ///
    /// The shared semantic composition (see
    /// [`LocalConversationCore::compose`]) is built once, the Runtime
    /// Client host binds over the inert runtime, and the runtime is then
    /// activated. The returned runtime is already active.
    ///
    /// # Errors
    ///
    /// Returns the first composition failure. Every failure happens before
    /// any protocol output exists.
    pub async fn compose(
        paths: &LocalRuntimePaths,
        dependencies: &LocalRuntimeDependencies,
    ) -> Result<Self, LocalRuntimeError> {
        LocalConversationCore::compose(paths, dependencies)
            .await?
            .into_interactive()
    }

    /// Performs the one shared Inactive -> Running lifecycle transition.
    ///
    /// The client host-binding decision is frozen by the time this runs,
    /// the admission worker starts, and semantic execution may begin. This
    /// is infallible by construction, which is what lets a caller place its
    /// own durable commit immediately before it.
    pub(crate) fn activate(&self) {
        self.core.runtime.activate();
    }

    /// The one conversation runtime coordinator of this process.
    #[must_use]
    pub const fn runtime(&self) -> &ConversationRuntime {
        &self.core.runtime
    }

    /// The one Runtime Client host (projection/control adapter) of this
    /// process.
    #[must_use]
    pub const fn host(&self) -> &RuntimeClientHost {
        &self.host
    }

    /// The one conversation tool runtime of this process.
    #[must_use]
    pub const fn tool_runtime(&self) -> &ConversationToolRuntime {
        &self.core.tool_runtime
    }

    /// The one capability coordinator of this process.
    #[must_use]
    pub const fn capability(&self) -> &CapabilityCoordinator {
        &self.core.capability
    }

    /// Creates the Runtime Client endpoint a transport wraps.
    #[must_use]
    pub fn endpoint(&self) -> RuntimeClientEndpoint {
        RuntimeClientEndpoint::new(self.host.clone())
    }
}

/// The current read authority of one known conversation inspection.
enum ConversationInspectionAuthority {
    /// The child process's actual live Runtime Client read projection.
    Live(tokio::net::UnixStream),
    /// The stable child conversation authorities after the live process is
    /// unavailable.
    Durable(RuntimeClientHost),
}

/// A read-only Runtime Client attachment to one known conversation.
///
/// Resolution first probes the identity-derived live endpoint. Only when the
/// child process no longer owns that endpoint does this type bootstrap the
/// ordinary Runtime Client projection from the child's durable authorities.
/// The caller and TUI use one conversation identity in both modes.
pub struct LocalConversationInspection {
    conversation_id: ConversationId,
    authority: ConversationInspectionAuthority,
}

impl std::fmt::Debug for LocalConversationInspection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalConversationInspection")
            .field("conversation_id", &self.conversation_id)
            .field(
                "authority",
                &match &self.authority {
                    ConversationInspectionAuthority::Live(_) => "live",
                    ConversationInspectionAuthority::Durable(_) => "durable",
                },
            )
            .finish()
    }
}

impl LocalConversationInspection {
    /// Resolves `conversation_id` to the child's live Runtime Client read
    /// projection when its process is still running. If the process owns the
    /// disposable liveness lease but endpoint setup failed, this returns an
    /// explicit [`LocalRuntimeError::LiveInspectionUnavailable`] instead of
    /// presenting a stale durable projection as live. Once the process is
    /// gone, the same identity resolves to the stable durable child
    /// conversation.
    ///
    /// The identity is resolved to the local runtime's stable child-store
    /// layout only at this Rust-owned composition boundary. The TUI and wire
    /// protocol never receive or store a filesystem path.
    ///
    /// # Errors
    ///
    /// Returns [`LocalRuntimeError::ConversationNotFound`] when the identity
    /// has no durable child store and no live endpoint, or
    /// [`LocalRuntimeError::LiveInspectionUnavailable`] when the child is
    /// still live but its optional endpoint is unavailable, or
    /// [`LocalRuntimeError::DurableConversation`] when that store cannot be
    /// opened.
    pub async fn compose(
        paths: &LocalRuntimePaths,
        conversation_id: &ConversationId,
    ) -> Result<Self, LocalRuntimeError> {
        if !is_safe_child_conversation_component(conversation_id) {
            return Err(LocalRuntimeError::ConversationNotFound {
                conversation_id: conversation_id.clone(),
                path: child_conversation_store_path(&paths.runtime_root, conversation_id),
            });
        }
        let socket_path =
            child_conversation_inspection_socket_path(&paths.runtime_root, conversation_id);
        if let Ok(stream) = crate::local_runtime::live_inspection::connect_live(&socket_path).await
        {
            return Ok(Self {
                conversation_id: conversation_id.clone(),
                authority: ConversationInspectionAuthority::Live(stream),
            });
        }
        let liveness_path =
            child_conversation_inspection_liveness_path(&paths.runtime_root, conversation_id);
        match crate::local_runtime::live_inspection::probe_liveness(&liveness_path) {
            Ok(Some(true)) => {
                return Err(LocalRuntimeError::LiveInspectionUnavailable {
                    conversation_id: conversation_id.clone(),
                    detail: format!(
                        "the child runtime is live but its Runtime Client inspection endpoint \
                         is unavailable ({})",
                        socket_path.display()
                    ),
                });
            }
            Ok(Some(false) | None) => {}
            Err(error) => {
                return Err(LocalRuntimeError::LiveInspectionUnavailable {
                    conversation_id: conversation_id.clone(),
                    detail: format!(
                        "the child runtime's live inspection status could not be resolved from \
                         {}: {error}",
                        liveness_path.display()
                    ),
                });
            }
        }
        let database_path = child_conversation_store_path(&paths.runtime_root, conversation_id);
        if !database_path.is_file() {
            return Err(LocalRuntimeError::ConversationNotFound {
                conversation_id: conversation_id.clone(),
                path: database_path,
            });
        }
        let store = Arc::new(
            SqliteConversationStore::open(conversation_id.clone(), &database_path).map_err(
                |error| LocalRuntimeError::DurableConversation {
                    path: database_path.clone(),
                    detail: error.to_string(),
                },
            )?,
        );
        Ok(Self {
            conversation_id: conversation_id.clone(),
            authority: ConversationInspectionAuthority::Durable(RuntimeClientHost::new_durable(
                store, None,
            )?),
        })
    }

    /// The in-process Runtime Client endpoint for a durable inspection.
    ///
    /// A live inspection is already connected to the child-owned endpoint and
    /// must be served through the local byte proxy; it has no local semantic
    /// endpoint in this process.
    ///
    /// # Errors
    ///
    /// Returns [`LocalRuntimeError::Observation`] when this inspection was
    /// resolved to a live child process.
    pub fn endpoint(&self) -> Result<RuntimeClientEndpoint, LocalRuntimeError> {
        match &self.authority {
            ConversationInspectionAuthority::Live(_) => Err(LocalRuntimeError::Observation {
                detail: "a live inspection is served by its child-owned endpoint".to_owned(),
            }),
            ConversationInspectionAuthority::Durable(host) => Ok(host.endpoint()),
        }
    }

    /// Serves this resolved inspection over the current process's stdio.
    pub(crate) async fn serve(
        self,
    ) -> Result<
        crate::runtime_client::transport::stdio::StdioSessionEnd,
        crate::runtime_client::transport::stdio::StdioTransportError,
    > {
        match self.authority {
            ConversationInspectionAuthority::Live(stream) => {
                crate::local_runtime::live_inspection::serve_live_stdio(stream).await
            }
            ConversationInspectionAuthority::Durable(host) => {
                crate::runtime_client::transport::stdio::serve_stdio_jsonl(host.endpoint()).await
            }
        }
    }

    /// Serves this resolved inspection over arbitrary async byte streams.
    /// The live and durable authorities use the same Runtime Client JSONL
    /// session semantics; only the byte-stream ownership differs.
    #[cfg(test)]
    pub(crate) async fn serve_with_io<R, W>(
        self,
        reader: R,
        writer: W,
    ) -> Result<
        crate::runtime_client::transport::stdio::StdioSessionEnd,
        crate::runtime_client::transport::stdio::StdioTransportError,
    >
    where
        R: tokio::io::AsyncRead + Unpin,
        W: tokio::io::AsyncWrite + Unpin,
    {
        match self.authority {
            ConversationInspectionAuthority::Live(stream) => {
                crate::local_runtime::live_inspection::serve_live_with_io(stream, reader, writer)
                    .await
            }
            ConversationInspectionAuthority::Durable(host) => {
                crate::runtime_client::transport::stdio::serve_stdio_jsonl_with_io(
                    host.endpoint(),
                    reader,
                    writer,
                )
                .await
            }
        }
    }
}

/// The composed headless local conversation runtime.
///
/// The same semantic composition as the interactive runtime
/// ([`LocalConversationCore`]), activated with **no** Runtime Client host:
/// no projection, no attachment policy, no protocol endpoint. Headless
/// drivers publish ordinary inbound through
/// [`ConversationRuntime::submit_inbound`](crate::runtime::conversation_runtime::ConversationRuntime::submit_inbound)
/// and await settlement through the runtime's settlement signal.
pub struct HeadlessConversationRuntime {
    core: LocalConversationCore,
}

impl std::fmt::Debug for HeadlessConversationRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HeadlessConversationRuntime")
            .field("conversation_id", self.tool_runtime().conversation_id())
            .finish_non_exhaustive()
    }
}

impl HeadlessConversationRuntime {
    /// Composes the headless runtime from explicit startup paths.
    ///
    /// The shared semantic composition (see
    /// [`LocalConversationCore::compose`]) is built once and activated
    /// directly, with no Runtime Client host ever constructed. The
    /// returned runtime is already active.
    ///
    /// # Errors
    ///
    /// Returns the first composition failure. Every failure happens before
    /// any protocol output exists.
    pub async fn compose(
        paths: &LocalRuntimePaths,
        dependencies: &LocalRuntimeDependencies,
    ) -> Result<Self, LocalRuntimeError> {
        Ok(LocalConversationCore::compose(paths, dependencies)
            .await?
            .into_headless())
    }

    /// The one conversation runtime coordinator of this runtime.
    #[must_use]
    pub const fn runtime(&self) -> &ConversationRuntime {
        &self.core.runtime
    }

    /// The one conversation tool runtime of this runtime.
    #[must_use]
    pub const fn tool_runtime(&self) -> &ConversationToolRuntime {
        &self.core.tool_runtime
    }

    /// The one capability coordinator of this runtime.
    #[must_use]
    pub const fn capability(&self) -> &CapabilityCoordinator {
        &self.core.capability
    }
}

fn read_file(path: &Path) -> Result<Vec<u8>, LocalRuntimeError> {
    std::fs::read(path).map_err(|error| LocalRuntimeError::Io {
        path: path.to_path_buf(),
        detail: error.to_string(),
    })
}

/// Loads the current model catalog and constructs its resolved binding
/// authority. This is intentionally a composition concern, not a
/// `SessionCatalog` concern: callers can validate the current runtime default
/// before publishing a first durable Session.
fn load_model_registry(
    paths: &LocalRuntimePaths,
    dependencies: &LocalRuntimeDependencies,
) -> Result<ModelBindingRegistry, LocalRuntimeError> {
    let catalog_bytes = read_file(&paths.models)?;
    let catalog = ModelCatalog::from_jsonc_slice(&catalog_bytes)?;
    let resolved = catalog.resolve(dependencies.credentials.as_ref())?;
    Ok(ModelBindingRegistry::new(resolved)?)
}

fn resolve_workspace_path(workspace: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace.join(path)
    }
}

/// A local runtime composition failure.
///
/// Every variant is a *startup configuration* failure surfaced on stderr
/// before any protocol frame exists. No variant carries a credential value.
#[derive(Debug, Clone, PartialEq)]
pub enum LocalRuntimeError {
    /// A startup file could not be read.
    Io {
        /// The path that failed.
        path: PathBuf,
        /// The failure detail.
        detail: String,
    },
    /// The requested durable child conversation does not exist at the
    /// identity-derived local store path.
    ConversationNotFound {
        /// The requested conversation identity.
        conversation_id: ConversationId,
        /// The identity-derived database path, for diagnostics only.
        path: PathBuf,
    },
    /// The requested durable conversation exists but cannot be opened.
    DurableConversation {
        /// The database path, for diagnostics only.
        path: PathBuf,
        /// The bounded store failure detail.
        detail: String,
    },
    /// The child runtime is still live, but its optional read-only inspection
    /// transport is unavailable. This must not be silently replaced by a
    /// durable snapshot because the durable authorities cannot reproduce all
    /// disposable live Runtime Client state.
    LiveInspectionUnavailable {
        /// The still-live child conversation identity.
        conversation_id: ConversationId,
        /// The bounded local routing diagnostic.
        detail: String,
    },
    /// The model catalog is invalid or its credentials are unresolved.
    Catalog(ModelCatalogError),
    /// A model binding or the initial session model could not be resolved.
    Model(ModelInvocationError),
    /// The current runtime/project configuration is invalid.
    RuntimeConfig(CurrentRuntimeConfigError),
    /// The conversation tool runtime could not be constructed.
    ToolRuntime {
        /// The failure detail.
        detail: String,
    },
    /// The native tool plane could not be composed.
    NativeTools {
        /// The failure detail.
        detail: String,
    },
    /// The base capability plane could not be constructed, prepared, or
    /// committed.
    ///
    /// This never carries an optional-source failure: each configured MCP
    /// server and each managed Python tool package fail into typed
    /// availability state instead (Issue #81).
    Capability {
        /// The failure detail.
        detail: String,
    },
    /// The conversation runtime could not be constructed.
    Runtime(ConversationRuntimeError),
    /// The Runtime Client host could not be constructed.
    Host(HostConstructionError),
    /// A local observation consumer could not be attached to the live
    /// Runtime Client observation stream before activation.
    Observation {
        /// The bounded composition diagnostic.
        detail: String,
    },
    /// The native SessionCatalog/Graph could not be loaded or published.
    SessionCatalog(SessionError),
    /// The native Session supervisor could not install or drain a lineage.
    SessionSupervisor(SessionSupervisorError),
}

impl std::fmt::Display for LocalRuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { path, detail } => write!(f, "cannot read {}: {detail}", path.display()),
            Self::ConversationNotFound {
                conversation_id,
                path,
            } => write!(
                f,
                "durable conversation {conversation_id} was not found at {}",
                path.display()
            ),
            Self::DurableConversation { path, detail } => write!(
                f,
                "cannot open durable conversation {}: {detail}",
                path.display()
            ),
            Self::LiveInspectionUnavailable {
                conversation_id,
                detail,
            } => write!(
                f,
                "live inspection of child conversation {conversation_id} is unavailable: {detail}"
            ),
            Self::Catalog(error) => write!(f, "model catalog: {error}"),
            Self::Model(error) => write!(f, "session model: {error}"),
            Self::RuntimeConfig(error) => write!(f, "{error}"),
            Self::ToolRuntime { detail } => write!(f, "conversation tool runtime: {detail}"),
            Self::NativeTools { detail } => write!(f, "native tool composition: {detail}"),
            Self::Capability { detail } => write!(f, "capability plane: {detail}"),
            Self::Runtime(error) => write!(f, "conversation runtime: {error}"),
            Self::Host(error) => write!(f, "runtime client host: {error}"),
            Self::Observation { detail } => {
                write!(f, "runtime observation subscription: {detail}")
            }
            Self::SessionCatalog(error) => write!(f, "session catalog: {error}"),
            Self::SessionSupervisor(error) => write!(f, "session supervisor: {error}"),
        }
    }
}

impl std::error::Error for LocalRuntimeError {}

impl From<ModelCatalogError> for LocalRuntimeError {
    fn from(error: ModelCatalogError) -> Self {
        Self::Catalog(error)
    }
}

impl From<ModelInvocationError> for LocalRuntimeError {
    fn from(error: ModelInvocationError) -> Self {
        Self::Model(error)
    }
}

impl From<CurrentRuntimeConfigError> for LocalRuntimeError {
    fn from(error: CurrentRuntimeConfigError) -> Self {
        Self::RuntimeConfig(error)
    }
}

impl From<ConversationRuntimeError> for LocalRuntimeError {
    fn from(error: ConversationRuntimeError) -> Self {
        Self::Runtime(error)
    }
}

impl From<HostConstructionError> for LocalRuntimeError {
    fn from(error: HostConstructionError) -> Self {
        Self::Host(error)
    }
}

impl From<SessionError> for LocalRuntimeError {
    fn from(error: SessionError) -> Self {
        Self::SessionCatalog(error)
    }
}

impl From<SessionSupervisorError> for LocalRuntimeError {
    fn from(error: SessionSupervisorError) -> Self {
        Self::SessionSupervisor(error)
    }
}

#[cfg(test)]
mod subagent_child_tests {
    use std::sync::Arc;

    use super::{ChildPreparation, LocalConversationCore, LocalRuntimeDependencies};
    use crate::model::catalog::MapCredentialEnvironment;
    use crate::runtime::identity::{AgentId, ConversationId, SubagentId, ToolId};
    use crate::runtime::resources::ProjectContextFile;
    use crate::runtime::subagent::catalog::SubagentName;
    use crate::runtime::subagent::ipc::{SUBAGENT_IPC_VERSION, SubagentChildSpec};
    use crate::runtime::subagent::resolver::{ResolvedSubagentSpec, ResolvedSubagentTool};
    use crate::tools::types::{
        ToolApprovalPolicy, ToolConcurrencyPolicy, ToolDefinition, ToolExecutionPolicy,
        ToolInvocationPolicy, ToolOrigin, ToolReplayPolicy,
    };

    const MODELS: &str = r#"{
      "providers": {
        "local": {
          "baseUrl": "http://127.0.0.1:9/v1",
          "apiKey": "$RUSTX_CHILD_KEY",
          "models": [
            {
              "id": "model-a",
              "protocol": "openai_chat_completions",
              "contextWindow": 128000,
              "maxOutputTokens": 512,
              "capabilities": {"inputModalities": ["text"], "outputModalities": ["text"], "toolCalls": true, "reasoning": false},
              "compat": {"chatReasoningReplay": "omit"}
            }
          ]
        }
      }
    }"#;

    fn dependencies() -> LocalRuntimeDependencies {
        LocalRuntimeDependencies {
            credentials: Arc::new(MapCredentialEnvironment::new([(
                "RUSTX_CHILD_KEY".to_owned(),
                "test-only-secret".to_owned(),
            )])),
            ..LocalRuntimeDependencies::default()
        }
    }

    /// A frozen Builtin capability carrying the **real** admitted native
    /// definition under the given policy: the child plane materializes the
    /// exact frozen definition, so a synthetic stand-in would (correctly)
    /// fail to materialize.
    fn builtin_with_policy(name: &str, policy: ToolInvocationPolicy) -> ResolvedSubagentTool {
        let definition = crate::tools::native::subagent_child_definition(name, policy)
            .expect("the child plane implements this native capability");
        ResolvedSubagentTool::Builtin {
            tool_id: definition.id.clone(),
            name: definition.name.clone(),
            definition,
        }
    }

    fn builtin(name: &str) -> ResolvedSubagentTool {
        builtin_with_policy(name, ToolInvocationPolicy::default())
    }

    fn spec(
        root: &std::path::Path,
        tools: Vec<ResolvedSubagentTool>,
        project_instructions: Vec<ProjectContextFile>,
        skills: Vec<crate::runtime::subagent::ResolvedSubagentSkill>,
    ) -> SubagentChildSpec {
        SubagentChildSpec {
            protocol_version: SUBAGENT_IPC_VERSION,
            subagent_id: SubagentId::new("conv-parent-subagent-1"),
            child_conversation_id: ConversationId::new("conv-parent-subagent-1"),
            child_agent_id: AgentId::new("agent-child"),
            parent_agent_id: AgentId::new("agent-parent"),
            resolved: ResolvedSubagentSpec {
                agent: SubagentName::parse("explore").expect("canonical name"),
                definition_digest: serde_json::from_value(serde_json::json!("sha256:frozen"))
                    .expect("digest"),
                execution_deadline: None,
                workspace_policy:
                    crate::runtime::subagent::SubagentWorkspacePolicy::SharedWorkspace,
                instructions: "frozen child instructions".to_owned(),
                model: crate::model::frozen::test_frozen_model_spec(
                    serde_json::from_value(serde_json::json!("local/model-a")).expect("model ref"),
                ),
                tools,
                skills,
                project_instructions,
                materialization:
                    crate::runtime::subagent::resolver::ResolvedSubagentMaterialization::default(),
            },
            approval_mode: crate::runtime::ApprovalMode::Policy,
            model_timeout_policy: crate::model::ModelTimeoutPolicy::default(),
            agent_status: crate::context::AgentStatusConfig::default(),
            context: crate::context::SessionContextPolicy {
                reserve_tokens: 0,
                keep_recent_tokens: 0,
                summary_output_cap: None,
            },
            workspace_snapshot: crate::runtime::subagent::WorkspaceSnapshot::shared(
                root.join("workspace"),
            ),
            runtime_root: root.join("child"),
            terminal: crate::runtime::subagent::ipc::ChildTerminalMode::Normal,
        }
    }

    /// A lab whose workspace ancestry is *full* of project instructions and
    /// Skills a discovering child would pick up.
    fn lab() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("lab");
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(workspace.join(".agents/skills/ambient")).expect("skills");
        std::fs::write(dir.path().join("models.jsonc"), MODELS).expect("models.jsonc");
        std::fs::write(
            workspace.join("AGENTS.md"),
            "ambient workspace instructions\n",
        )
        .expect("AGENTS.md");
        std::fs::write(
            workspace.join(".agents/skills/ambient/SKILL.md"),
            "---\nname: ambient\ndescription: an ambient skill\n---\n\nambient body\n",
        )
        .expect("SKILL.md");
        dir
    }

    /// Issue #144: a child observes only what its invoking generation froze.
    /// The workspace deliberately contains an `AGENTS.md` and a discoverable
    /// Skill; a child that ran discovery would show both.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_child_performs_no_project_instruction_or_skill_discovery() {
        let dir = lab();
        let frozen = vec![ProjectContextFile {
            path: dir.path().join("frozen/AGENTS.md"),
            content: "frozen parent chain".to_owned(),
        }];
        let core = LocalConversationCore::compose_subagent_child(
            &spec(dir.path(), vec![builtin("read")], frozen, Vec::new()),
            &dependencies(),
            &ChildPreparation::detached(),
        )
        .await
        .expect("the child composes");
        let resources = core.runtime().runtime_resources();

        assert_eq!(
            resources
                .project_context_files()
                .iter()
                .map(|file| file.content.clone())
                .collect::<Vec<_>>(),
            vec!["frozen parent chain".to_owned()],
            "the child observes exactly the frozen chain and never walks ancestors"
        );
        assert_eq!(
            resources.project_instructions(),
            Some("frozen parent chain")
        );
        assert_eq!(
            resources.skill_catalog(),
            None,
            "an empty frozen allowlist means an empty child Skill catalog, \
             whatever the workspace contains"
        );
        assert_eq!(
            resources.agent_profile(),
            Some(
                format!(
                    "frozen child instructions\n\n{}",
                    crate::runtime::subagent::SUBAGENT_FINAL_REPORT_INSTRUCTION
                )
                .as_str()
            ),
            "the definition's instructions, composed with the runtime-owned final-report rule, \
             are the child's AgentProfile authority"
        );
        assert_eq!(
            core.capability().current_snapshot().tool_registry().names(),
            vec!["read"],
            "the child's active set is exactly its authorized set"
        );
    }

    /// Issue #192: the runtime-owned final-report handoff rule is generic
    /// subagent execution semantics — a normal one-shot child receives it
    /// appended to its definition's instructions even when the user-authored
    /// document never mentions the handoff. A Workflow-owned child keeps its
    /// structured `workflow_output` terminal protocol instead: the free-form
    /// final-report rule would contradict it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_final_report_rule_is_runtime_owned_and_terminal_mode_aware() {
        let dir = lab();

        // The user-authored document says nothing about the handoff.
        let normal = LocalConversationCore::compose_subagent_child(
            &spec(dir.path(), vec![builtin("read")], Vec::new(), Vec::new()),
            &dependencies(),
            &ChildPreparation::detached(),
        )
        .await
        .expect("the normal child composes");
        let profile = normal
            .runtime()
            .runtime_resources()
            .agent_profile()
            .expect("agent profile")
            .to_owned();
        assert!(
            profile.starts_with("frozen child instructions\n\n"),
            "the user-authored instructions are preserved first: {profile}"
        );
        assert!(
            profile.contains("Your final response is the complete handoff to the parent agent."),
            "the runtime-owned handoff rule is composed for a normal child: {profile}"
        );

        let mut workflow_spec = spec(dir.path(), vec![builtin("read")], Vec::new(), Vec::new());
        workflow_spec.terminal = crate::runtime::subagent::ipc::ChildTerminalMode::WorkflowOutput {
            output_schema: serde_json::json!({"type": "object"}),
        };
        let workflow = LocalConversationCore::compose_subagent_child(
            &workflow_spec,
            &dependencies(),
            &ChildPreparation::detached(),
        )
        .await
        .expect("the Workflow child composes");
        assert_eq!(
            workflow.runtime().runtime_resources().agent_profile(),
            Some("frozen child instructions"),
            "a Workflow-owned child keeps its structured terminal protocol, unamended"
        );
    }

    /// Issue #146: changing the physical project root does not change the
    /// project-instruction authority. The unrelated AGENTS.md is deliberately
    /// above the worktree-shaped path; the child still consumes only the
    /// parent-frozen chain carried in its typed specification.
    ///
    /// The isolated policy here is an explicit synthetic fixture (no
    /// acquisition runs): it models a permissive capture whose parent was
    /// dirty (`parent_had_uncommitted_changes: true`), matching the
    /// Issue #188 explicit opt-out rather than the strict default.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_worktree_child_never_discovers_ancestor_project_instructions() {
        let dir = lab();
        let worktree = dir.path().join("runtime/worktrees/child");
        std::fs::create_dir_all(&worktree).expect("worktree-shaped workspace");
        std::fs::write(
            dir.path().join("AGENTS.md"),
            "unrelated ancestor instructions\n",
        )
        .expect("ancestor AGENTS.md");
        let mut child_spec = spec(
            dir.path(),
            vec![builtin("read")],
            vec![ProjectContextFile {
                path: dir.path().join("frozen/AGENTS.md"),
                content: "frozen parent chain".to_owned(),
            }],
            Vec::new(),
        );
        child_spec.resolved.workspace_policy =
            crate::runtime::subagent::SubagentWorkspacePolicy::GitWorktree {
                require_clean_parent: false,
            };
        child_spec.workspace_snapshot = crate::runtime::subagent::WorkspaceSnapshot {
            logical_workspace: worktree.clone(),
            isolation: crate::runtime::subagent::WorkspaceIsolation::GitWorktree(
                crate::runtime::subagent::GitWorktreeSnapshot {
                    source_repository_root: dir.path().to_path_buf(),
                    repository_relative_workspace: std::path::PathBuf::new(),
                    physical_worktree_root: worktree,
                    base_commit: "1111111111111111111111111111111111111111".to_owned(),
                    branch: "rustx/subagent/frozen".to_owned(),
                    parent_had_uncommitted_changes: true,
                },
            ),
        };

        let core = LocalConversationCore::compose_subagent_child(
            &child_spec,
            &dependencies(),
            &ChildPreparation::detached(),
        )
        .await
        .expect("the worktree child composes");
        let resources = core.runtime().runtime_resources();
        assert_eq!(
            resources.project_context_files(),
            &[ProjectContextFile {
                path: dir.path().join("frozen/AGENTS.md"),
                content: "frozen parent chain".to_owned(),
            }],
            "the worktree path is not project-instruction authority"
        );
        assert_eq!(
            resources.project_instructions(),
            Some("frozen parent chain")
        );
        assert_eq!(resources.skill_catalog(), None);
    }

    /// A frozen Skill allowlist is **materialized** into the child-private
    /// root, re-proven against its frozen `SkillVersionId`, and rendered as
    /// metadata only, so progressive disclosure survives the boundary
    /// (Issue #145).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_frozen_skill_allowlist_is_materialized_and_rendered_without_bodies() {
        let dir = lab();
        let source = dir.path().join("selected-source");
        std::fs::create_dir_all(&source).expect("source");
        std::fs::write(
            source.join("SKILL.md"),
            "---\nname: selected\ndescription: the selected skill\n---\nsecret body\n",
        )
        .expect("SKILL.md");
        let files = vec![std::path::PathBuf::from("SKILL.md")];
        let markdown = std::fs::read(source.join("SKILL.md")).expect("read");
        let version_id = crate::skills::identity::package_version_id(&source, &files, &markdown)
            .expect("frozen identity");
        let child_spec = spec(
            dir.path(),
            vec![builtin("read"), builtin("grep")],
            Vec::new(),
            vec![crate::runtime::subagent::ResolvedSubagentSkill {
                binding: crate::protocol::manifest::SkillBinding {
                    skill_id: crate::runtime::identity::SkillId::new("selected"),
                    version_id,
                },
                catalog_entry: crate::skills::SkillCatalogEntry {
                    name: "selected".to_owned(),
                    description: "the selected skill".to_owned(),
                    location: source.join("SKILL.md").display().to_string(),
                },
                source_root: source.clone(),
                files,
            }],
        );
        let core = LocalConversationCore::compose_subagent_child(
            &child_spec,
            &dependencies(),
            &ChildPreparation::detached(),
        )
        .await
        .expect("the child composes");
        let catalog = core
            .runtime()
            .runtime_resources()
            .skill_catalog()
            .expect("the frozen catalog")
            .to_owned();
        let materialized = child_spec
            .runtime_root
            .join("skills/selected/SKILL.md")
            .display()
            .to_string();
        assert!(catalog.contains("selected"));
        assert!(
            catalog.contains(&materialized),
            "the model-visible location is remapped onto the child's own copy: {catalog}"
        );
        assert!(
            std::path::Path::new(&materialized).is_file(),
            "the frozen bytes are materialized under the child-private root"
        );
        assert!(
            !catalog.contains("ambient"),
            "an unselected, discoverable Skill is absent from the child catalog: {catalog}"
        );
        assert!(
            !catalog.contains("secret body"),
            "only catalog metadata crosses the boundary: {catalog}"
        );
    }

    /// Skill bytes that changed after the parent froze them fail child
    /// composition rather than executing a different Skill version.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_changed_skill_source_fails_child_composition() {
        let dir = lab();
        let source = dir.path().join("selected-source");
        std::fs::create_dir_all(&source).expect("source");
        std::fs::write(
            source.join("SKILL.md"),
            "---\nname: selected\ndescription: the selected skill\n---\noriginal\n",
        )
        .expect("SKILL.md");
        let files = vec![std::path::PathBuf::from("SKILL.md")];
        let markdown = std::fs::read(source.join("SKILL.md")).expect("read");
        let version_id = crate::skills::identity::package_version_id(&source, &files, &markdown)
            .expect("frozen identity");
        // The source moves on after the freeze.
        std::fs::write(
            source.join("SKILL.md"),
            "---\nname: selected\ndescription: the selected skill\n---\nrewritten\n",
        )
        .expect("rewrite");
        let child_spec = spec(
            dir.path(),
            vec![builtin("read")],
            Vec::new(),
            vec![crate::runtime::subagent::ResolvedSubagentSkill {
                binding: crate::protocol::manifest::SkillBinding {
                    skill_id: crate::runtime::identity::SkillId::new("selected"),
                    version_id,
                },
                catalog_entry: crate::skills::SkillCatalogEntry {
                    name: "selected".to_owned(),
                    description: "the selected skill".to_owned(),
                    location: source.join("SKILL.md").display().to_string(),
                },
                source_root: source,
                files,
            }],
        );
        let error = LocalConversationCore::compose_subagent_child(
            &child_spec,
            &dependencies(),
            &ChildPreparation::detached(),
        )
        .await
        .expect_err("a changed Skill source fails closed");
        assert!(
            format!("{error}").contains("source bytes changed"),
            "unexpected failure: {error}"
        );
    }

    /// Issue #144: the child's model authority comes from the frozen
    /// specification, so there is no model catalog read to race with a
    /// later edit.
    ///
    /// The proof is structural rather than a timing argument: the catalog
    /// file is deleted before composition, and the child still composes with
    /// exactly the frozen semantics. There is no code path left that could
    /// observe a `models.jsonc` at all.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_child_composes_with_no_model_catalog_on_disk() {
        let dir = lab();
        std::fs::remove_file(dir.path().join("models.jsonc")).expect("remove the catalog");
        let spec = spec(dir.path(), vec![builtin("read")], Vec::new(), Vec::new());
        let core = LocalConversationCore::compose_subagent_child(
            &spec,
            &dependencies(),
            &ChildPreparation::detached(),
        )
        .await
        .expect("the child composes from the frozen model authority alone");

        let model = core.runtime().model_view();
        assert_eq!(
            model.effective.model, spec.resolved.model.primary.model,
            "the child runs exactly the model its parent froze"
        );
        assert_eq!(
            model.effective.protocol,
            spec.resolved.model.primary.protocol
        );
        assert_eq!(
            model.effective.context_window,
            spec.resolved.model.primary.context_window
        );
        assert_eq!(
            core.runtime().model_catalog().models.len(),
            1,
            "a frozen authority publishes exactly the model it froze"
        );
    }

    // ---- Issue #145: child preparation is cancellable owned work ----

    /// Cancellation derived from the invoking spawn attempt settles a long
    /// preparation — and the in-flight step is **not merely dropped**: the
    /// guard fires the one preparation signal and awaits the step's own
    /// settlement, so preparatory physical work settles before the
    /// preparation reports settled.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn attempt_cancellation_settles_a_long_preparation() {
        let cancellation = crate::runtime::cancellation::CancellationSignal::new();
        let (parent, child) = tokio::net::UnixStream::pair().expect("control pair");
        let (_observation_parent, observation_child) =
            tokio::net::UnixStream::pair().expect("observation pair");
        let dispatcher = crate::local_runtime::dispatcher::ChildControlDispatcher::start(
            child,
            observation_child,
        );
        let preparation = ChildPreparation::new(cancellation.clone(), dispatcher.handle());

        // A cancellation-aware step that would never finish on its own: it
        // parks on the preparation's one authority and records its
        // settlement before returning — exactly the contract of an MCP
        // connect or a uv build.
        let settled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let step = {
            let signal = preparation.cancellation();
            let settled = settled.clone();
            async move {
                signal.cancelled().await;
                settled.store(true, std::sync::atomic::Ordering::SeqCst);
                Err::<(), _>(
                    crate::capabilities::CapabilityPreparationError::PreparationSettled(
                        "the step settled its owned work".to_owned(),
                    ),
                )
            }
        };
        cancellation.cancel();
        let outcome = preparation.guard(step).await;
        assert!(
            matches!(
                outcome,
                Err(crate::capabilities::CapabilityPreparationError::PreparationSettled(_))
            ),
            "cancellation settles the preparation: {outcome:?}"
        );
        assert!(
            settled.load(std::sync::atomic::Ordering::SeqCst),
            "the guard awaited the step's settlement before reporting settled"
        );
        drop(parent);
    }

    /// Parent control-channel EOF is a **physical cancellation
    /// authority**, observable **while** composition is in progress: it
    /// fires the one preparation cancellation signal (so every preparatory
    /// supervised unit is cancelled), and the guard awaits the step's
    /// settlement rather than finishing work for a parent that no longer
    /// exists.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn parent_loss_settles_a_long_preparation() {
        let (parent, child) = tokio::net::UnixStream::pair().expect("control pair");
        let (_observation_parent, observation_child) =
            tokio::net::UnixStream::pair().expect("observation pair");
        let dispatcher = crate::local_runtime::dispatcher::ChildControlDispatcher::start(
            child,
            observation_child,
        );
        let handle = dispatcher.handle();
        let preparation = ChildPreparation::new(
            crate::runtime::cancellation::CancellationSignal::new(),
            handle.clone(),
        );
        // The step parks on the preparation's one authority: only the
        // guard's parent-loss arm can release it, by firing the signal.
        let settled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let step = {
            let signal = preparation.cancellation();
            let settled = settled.clone();
            async move {
                signal.cancelled().await;
                settled.store(true, std::sync::atomic::Ordering::SeqCst);
                Err::<(), _>(
                    crate::capabilities::CapabilityPreparationError::PreparationSettled(
                        "the step settled its owned work".to_owned(),
                    ),
                )
            }
        };
        // The EOF is published by the dispatcher's reader before the guard
        // is entered, so the ordering is a fact rather than a race.
        drop(parent);
        handle.parent_lost_signal().await;

        let outcome = preparation.guard(step).await;
        assert!(
            matches!(
                outcome,
                Err(crate::capabilities::CapabilityPreparationError::PreparationSettled(_))
            ),
            "parent loss settles the preparation: {outcome:?}"
        );
        assert!(
            settled.load(std::sync::atomic::Ordering::SeqCst),
            "parent loss fired the physical cancellation authority and the step settled"
        );
        assert!(
            preparation.is_settled(),
            "parent loss leaves the preparation settled"
        );
    }

    /// A preparation that completes before either settlement authority fires
    /// returns its ordinary result.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn an_uninterrupted_preparation_returns_its_result() {
        let (parent, child) = tokio::net::UnixStream::pair().expect("control pair");
        let (_observation_parent, observation_child) =
            tokio::net::UnixStream::pair().expect("observation pair");
        let dispatcher = crate::local_runtime::dispatcher::ChildControlDispatcher::start(
            child,
            observation_child,
        );
        let preparation = ChildPreparation::new(
            crate::runtime::cancellation::CancellationSignal::new(),
            dispatcher.handle(),
        );
        let outcome = preparation.guard(std::future::ready(Ok(7u32))).await;
        assert_eq!(outcome.expect("an uninterrupted preparation"), 7);
        drop(parent);
    }

    /// A step that completes **after** the cancellation won is not
    /// publishable: the biased cancellation arm plus the `is_settled`
    /// post-check make `Ready` impossible once pre-commit cancellation has
    /// won.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_step_completing_after_cancellation_is_not_publishable() {
        let cancellation = crate::runtime::cancellation::CancellationSignal::new();
        let (parent, child) = tokio::net::UnixStream::pair().expect("control pair");
        let (_observation_parent, observation_child) =
            tokio::net::UnixStream::pair().expect("observation pair");
        let dispatcher = crate::local_runtime::dispatcher::ChildControlDispatcher::start(
            child,
            observation_child,
        );
        let preparation = ChildPreparation::new(cancellation.clone(), dispatcher.handle());
        cancellation.cancel();
        // The cancellation already fired; the step completes anyway. The
        // guard's biased cancellation arm wins this poll...
        let outcome = preparation.guard(std::future::ready(Ok(7u32))).await;
        assert!(
            matches!(
                outcome,
                Err(crate::capabilities::CapabilityPreparationError::PreparationSettled(_))
            ),
            "a cancelled preparation never publishes: {outcome:?}"
        );
        // ...and where a step's completion wins a race elsewhere, the
        // caller-side `is_settled` check refuses to publish its output.
        assert!(preparation.is_settled());
        drop(parent);
    }

    // ---- Issue #145: selected-only MCP materialization ----

    /// The frozen MCP binding of the shared stdio fixture, re-executing this
    /// test binary as its own MCP server.
    #[cfg(feature = "mcp-fixture")]
    fn fixture_binding(test_name: &str, prefix: &str) -> crate::tools::mcp::McpServerBinding {
        crate::tools::mcp::McpServerBinding {
            transport: crate::tools::mcp::McpTransportConfig::Stdio {
                program: std::env::current_exe()
                    .expect("test executable")
                    .display()
                    .to_string(),
                args: crate::tools::mcp::fixture::fixture_spawn_args(test_name),
                cwd: None,
                environment: std::collections::BTreeMap::from([
                    (
                        crate::tools::mcp::fixture::FIXTURE_MODE_ENV.to_owned(),
                        "1".to_owned(),
                    ),
                    (
                        crate::tools::mcp::fixture::TOOL_PREFIX_ENV.to_owned(),
                        prefix.to_owned(),
                    ),
                ]),
            },
            policy: ToolInvocationPolicy::default(),
        }
    }

    /// Connects the fixture once, exactly as a parent generation would, and
    /// returns the canonical definition of one published tool.
    #[cfg(feature = "mcp-fixture")]
    async fn frozen_fixture_tool(
        server_id: &crate::runtime::identity::McpServerId,
        binding: &crate::tools::mcp::McpServerBinding,
        workspace: &crate::tools::Workspace,
        name: &str,
    ) -> ToolDefinition {
        let runtime = crate::tools::mcp::McpServerRuntime::connect(
            server_id,
            binding,
            workspace,
            std::sync::Arc::new(crate::tools::mcp::McpInvalidationState::new()),
        )
        .await
        .expect("the fixture connects");
        let tools = runtime.list_tools().await.expect("tools/list");
        let tool = tools
            .into_iter()
            .find(|tool| tool.name == name)
            .expect("the fixture publishes the tool");
        runtime.close().await.expect("the fixture runtime closes");
        ToolDefinition {
            id: ToolId::new(format!("mcp:{server_id}:{name}")),
            name: tool.name,
            description: tool.description,
            input_schema: tool.input_schema,
            execution_policy: binding.policy.execution,
            concurrency_policy: binding.policy.concurrency,
            approval_policy: binding.policy.approval,
            replay_policy: ToolReplayPolicy::Never,
            origin: ToolOrigin::Mcp {
                server_id: server_id.clone(),
            },
        }
    }

    /// A named agent that selects one **inactive-but-available** MCP tool
    /// starts, and its child Tool Plane contains exactly the frozen Builtin
    /// plus that one MCP tool — nothing the fixture also publishes, and
    /// nothing from the workspace.
    #[cfg(feature = "mcp-fixture")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_child_materializes_exactly_the_selected_mcp_tool() {
        if crate::tools::mcp::fixture::serve_if_fixture_mode(
            crate::tools::mcp::fixture::FixtureServer::from_env(),
        )
        .await
        {
            return;
        }
        let dir = lab();
        let workspace =
            crate::tools::Workspace::new(dir.path().join("workspace")).expect("workspace");
        let server_id = crate::runtime::identity::McpServerId::new("alpha");
        let binding = fixture_binding(
            "local_runtime::composition::subagent_child_tests::a_child_materializes_exactly_the_selected_mcp_tool",
            "alpha_",
        );
        let definition = frozen_fixture_tool(&server_id, &binding, &workspace, "alpha_echo").await;
        let identity = crate::tools::mcp::identity::definition_identity(&definition)
            .expect("an MCP definition has an MCP identity");
        let mcp = ResolvedSubagentTool::Mcp {
            server_id: server_id.clone(),
            tool_id: definition.id.clone(),
            name: definition.name.clone(),
            identity,
            definition,
        };
        let mut child_spec = spec(
            dir.path(),
            vec![builtin("read"), mcp],
            Vec::new(),
            Vec::new(),
        );
        child_spec
            .resolved
            .materialization
            .mcp_servers
            .insert(server_id, binding);

        let core = LocalConversationCore::compose_subagent_child(
            &child_spec,
            &dependencies(),
            &ChildPreparation::detached(),
        )
        .await
        .expect("the child materializes its frozen MCP selection");
        let snapshot = core.capability().current_snapshot();
        let mut names = snapshot.tool_registry().names();
        names.sort_unstable();
        assert_eq!(
            names,
            vec!["alpha_echo", "read"],
            "the child Tool Plane is exactly the frozen selection: the fixture also \
             publishes alpha_mutate and alpha_slow, and neither is materialized"
        );
    }

    /// A definition whose canonical cross-process identity is not the one the
    /// parent generation froze fails child preparation — before `Ready`, and
    /// therefore before any durable ownership commit.
    #[cfg(feature = "mcp-fixture")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_changed_mcp_definition_fails_before_the_child_is_ready() {
        if crate::tools::mcp::fixture::serve_if_fixture_mode(
            crate::tools::mcp::fixture::FixtureServer::from_env(),
        )
        .await
        {
            return;
        }
        let dir = lab();
        let workspace =
            crate::tools::Workspace::new(dir.path().join("workspace")).expect("workspace");
        let server_id = crate::runtime::identity::McpServerId::new("beta");
        let binding = fixture_binding(
            "local_runtime::composition::subagent_child_tests::a_changed_mcp_definition_fails_before_the_child_is_ready",
            "beta_",
        );
        let mut definition =
            frozen_fixture_tool(&server_id, &binding, &workspace, "beta_echo").await;
        // The parent froze a description the server no longer publishes.
        definition.description = format!("{} (as the parent froze it)", definition.description);
        let identity = crate::tools::mcp::identity::definition_identity(&definition)
            .expect("an MCP definition has an MCP identity");
        let mcp = ResolvedSubagentTool::Mcp {
            server_id: server_id.clone(),
            tool_id: definition.id.clone(),
            name: definition.name.clone(),
            identity,
            definition,
        };
        let mut child_spec = spec(
            dir.path(),
            vec![builtin("read"), mcp],
            Vec::new(),
            Vec::new(),
        );
        child_spec
            .resolved
            .materialization
            .mcp_servers
            .insert(server_id, binding);

        let error = LocalConversationCore::compose_subagent_child(
            &child_spec,
            &dependencies(),
            &ChildPreparation::detached(),
        )
        .await
        .expect_err("a changed definition must not be executed");
        let rendered = format!("{error:?}");
        assert!(
            rendered.contains("McpIdentityMismatch"),
            "the failure is a typed cross-process identity mismatch: {rendered}"
        );
        assert!(
            rendered.contains("beta_echo"),
            "the failure names the exact definition: {rendered}"
        );
    }

    /// A frozen tool the server no longer publishes fails child preparation
    /// rather than silently starting a weaker child.
    #[cfg(feature = "mcp-fixture")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_missing_mcp_definition_fails_before_the_child_is_ready() {
        if crate::tools::mcp::fixture::serve_if_fixture_mode(
            crate::tools::mcp::fixture::FixtureServer::from_env(),
        )
        .await
        {
            return;
        }
        let dir = lab();
        let server_id = crate::runtime::identity::McpServerId::new("gamma");
        let binding = fixture_binding(
            "local_runtime::composition::subagent_child_tests::a_missing_mcp_definition_fails_before_the_child_is_ready",
            "gamma_",
        );
        let definition = ToolDefinition {
            id: ToolId::new("mcp:gamma:gamma_absent"),
            name: "gamma_absent".to_owned(),
            description: "a tool the server does not publish".to_owned(),
            input_schema: serde_json::json!({"type": "object"}),
            execution_policy: ToolExecutionPolicy::ForegroundOnly,
            concurrency_policy: ToolConcurrencyPolicy::Sequential,
            approval_policy: ToolApprovalPolicy::Never,
            replay_policy: ToolReplayPolicy::Never,
            origin: ToolOrigin::Mcp {
                server_id: server_id.clone(),
            },
        };
        let mcp = ResolvedSubagentTool::Mcp {
            server_id: server_id.clone(),
            tool_id: definition.id.clone(),
            name: definition.name.clone(),
            identity: crate::tools::mcp::identity::definition_identity(&definition)
                .expect("an MCP definition has an MCP identity"),
            definition,
        };
        let mut child_spec = spec(
            dir.path(),
            vec![builtin("read"), mcp],
            Vec::new(),
            Vec::new(),
        );
        child_spec
            .resolved
            .materialization
            .mcp_servers
            .insert(server_id, binding);

        let error = LocalConversationCore::compose_subagent_child(
            &child_spec,
            &dependencies(),
            &ChildPreparation::detached(),
        )
        .await
        .expect_err("a missing definition must not be silently omitted");
        assert!(
            format!("{error:?}").contains("McpToolMissing"),
            "the failure is a typed missing-definition refusal: {error:?}"
        );
    }

    /// A frozen MCP requirement is **required**, not optional: a server the
    /// child cannot connect fails composition before `Ready` rather than
    /// degrading into an availability state and starting weaker than the
    /// child was authorized (Issue #145).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_unreachable_required_mcp_server_fails_child_composition() {
        let dir = lab();
        let server_id = crate::runtime::identity::McpServerId::new("github");
        let definition = ToolDefinition {
            id: ToolId::new("tool-get-issue"),
            name: "get_issue".to_owned(),
            description: "issue".to_owned(),
            input_schema: serde_json::json!({"type": "object"}),
            execution_policy: ToolExecutionPolicy::ForegroundOnly,
            concurrency_policy: ToolConcurrencyPolicy::Sequential,
            approval_policy: ToolApprovalPolicy::Never,
            replay_policy: ToolReplayPolicy::Never,
            origin: ToolOrigin::Mcp {
                server_id: server_id.clone(),
            },
        };
        let mcp = ResolvedSubagentTool::Mcp {
            server_id: server_id.clone(),
            tool_id: definition.id.clone(),
            name: definition.name.clone(),
            identity: crate::tools::mcp::identity::definition_identity(&definition)
                .expect("an MCP definition has an MCP identity"),
            definition,
        };
        let mut child_spec = spec(
            dir.path(),
            vec![builtin("read"), mcp],
            Vec::new(),
            Vec::new(),
        );
        child_spec.resolved.materialization.mcp_servers.insert(
            server_id,
            crate::tools::mcp::McpServerBinding {
                transport: crate::tools::mcp::McpTransportConfig::Stdio {
                    program: "rustx-no-such-mcp-server".to_owned(),
                    args: Vec::new(),
                    cwd: None,
                    environment: std::collections::BTreeMap::new(),
                },
                policy: ToolInvocationPolicy::default(),
            },
        );
        let error = LocalConversationCore::compose_subagent_child(
            &child_spec,
            &dependencies(),
            &ChildPreparation::detached(),
        )
        .await
        .expect_err("a required MCP source that cannot be connected fails composition");
        assert!(
            format!("{error:?}").contains("Mcp"),
            "the failure is an explicit MCP preparation failure: {error:?}"
        );
    }

    /// A frozen MCP selection whose materialization plane carries no binding
    /// for the required server can never be widened by the child: it fails.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_missing_frozen_mcp_binding_is_never_rediscovered() {
        let dir = lab();
        let server_id = crate::runtime::identity::McpServerId::new("github");
        let definition = ToolDefinition {
            id: ToolId::new("tool-get-issue"),
            name: "get_issue".to_owned(),
            description: "issue".to_owned(),
            input_schema: serde_json::json!({"type": "object"}),
            execution_policy: ToolExecutionPolicy::ForegroundOnly,
            concurrency_policy: ToolConcurrencyPolicy::Sequential,
            approval_policy: ToolApprovalPolicy::Never,
            replay_policy: ToolReplayPolicy::Never,
            origin: ToolOrigin::Mcp {
                server_id: server_id.clone(),
            },
        };
        let mcp = ResolvedSubagentTool::Mcp {
            server_id,
            tool_id: definition.id.clone(),
            name: definition.name.clone(),
            identity: crate::tools::mcp::identity::definition_identity(&definition)
                .expect("an MCP definition has an MCP identity"),
            definition,
        };
        let error = LocalConversationCore::compose_subagent_child(
            &spec(
                dir.path(),
                vec![builtin("read"), mcp],
                Vec::new(),
                Vec::new(),
            ),
            &dependencies(),
            &ChildPreparation::detached(),
        )
        .await
        .expect_err("a selection with no frozen binding cannot be materialized");
        assert!(
            format!("{error:?}").contains("was not given a binding"),
            "the child refuses rather than looking a server up for itself: {error:?}"
        );
    }
}

#[cfg(test)]
mod conversation_inspection_tests {
    use super::{LocalConversationInspection, LocalRuntimePaths, StartupSession};
    use crate::durable::{ConversationStore, SqliteConversationStore};
    use crate::local_runtime::live_inspection::LiveConversationInspectionLease;
    use crate::message::content::TextBlock;
    use crate::message::types::{MessageBlock, UserContentBlock, UserMessageBlock, UserSource};
    use crate::runtime::identity::{ConversationId, MessageId};
    use crate::runtime_client::types::{RuntimeClientRequest, RuntimeClientResult};

    #[tokio::test]
    async fn resolves_the_known_child_identity_to_the_ordinary_attachment() {
        let root = tempfile::tempdir().expect("runtime root");
        let workspace = root.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let conversation_id = ConversationId::new("conversation-parent-subagent-1");
        let database_path = root
            .path()
            .join("runtime/subagents")
            .join(conversation_id.as_str())
            .join("conversation.sqlite");
        std::fs::create_dir_all(database_path.parent().expect("child store parent"))
            .expect("child store directory");
        let store = SqliteConversationStore::open(conversation_id.clone(), &database_path)
            .expect("child store");
        store
            .initialize(&[MessageBlock::User(UserMessageBlock {
                id: MessageId::new("child-user"),
                content: vec![UserContentBlock::Text(TextBlock {
                    text: "durable child message".to_owned(),
                })],
                source: UserSource::Human,
                kind: crate::message::types::InboundKind::Message,
                timestamp: None,
            })])
            .expect("child history");

        let paths = LocalRuntimePaths {
            models: root.path().join("models.jsonc"),
            config: root.path().join("rustx.jsonc"),
            skill_paths: Vec::new(),
            no_skills: true,
            no_builtin_tools: false,
            no_tools: false,
            startup_session: StartupSession::Empty,
            session_name: None,
            tools: None,
            exclude_tools: Vec::new(),
            workspace,
            runtime_root: root.path().join("runtime"),
        };
        let inspection = LocalConversationInspection::compose(&paths, &conversation_id)
            .await
            .expect("identity resolves to the durable child store");
        let response = inspection
            .endpoint()
            .expect("durable inspection exposes a local endpoint")
            .handle_request(RuntimeClientRequest::Initialize {
                id: crate::runtime_client::RequestId::new(1),
                protocol_version: crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION,
            });
        let Some(RuntimeClientResult::Initialized {
            conversation_id: attached_id,
            snapshot,
            ..
        }) = response.result
        else {
            panic!("inspection must use the ordinary Runtime Client attachment: {response:?}");
        };
        assert_eq!(attached_id, conversation_id);
        assert_eq!(snapshot.messages.len(), 1);
        assert!(matches!(
            &snapshot.messages[0],
            MessageBlock::User(user)
                if user.id == MessageId::new("child-user")
                    && user.content.iter().any(|content| matches!(
                        content,
                        UserContentBlock::Text(text) if text.text == "durable child message"
                ))
        ));
    }

    /// A live child whose optional endpoint is unavailable is not silently
    /// rebuilt from durable state. Once the child-owned lease is released,
    /// the same identity resolves through the ordinary durable fallback.
    #[tokio::test]
    async fn live_uninspectable_identity_does_not_fake_a_durable_live_view() {
        let root = tempfile::tempdir().expect("runtime root");
        let workspace = root.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let runtime_root = root.path().join("runtime");
        let conversation_id = ConversationId::new("conversation-parent-subagent-live-gap");
        let database_path = crate::runtime::subagent::child_conversation_store_path(
            &runtime_root,
            &conversation_id,
        );
        std::fs::create_dir_all(database_path.parent().expect("child store parent"))
            .expect("child store directory");
        let store = SqliteConversationStore::open(conversation_id.clone(), &database_path)
            .expect("child store");
        store.initialize(&[]).expect("child history");
        let lease = LiveConversationInspectionLease::acquire(
            crate::runtime::subagent::child_conversation_inspection_liveness_path(
                &runtime_root,
                &conversation_id,
            ),
        )
        .expect("the running child owns its transient liveness lease");
        let paths = LocalRuntimePaths {
            models: root.path().join("models.jsonc"),
            config: root.path().join("rustx.jsonc"),
            skill_paths: Vec::new(),
            no_skills: true,
            no_builtin_tools: false,
            no_tools: false,
            startup_session: StartupSession::InspectConversation {
                conversation_id: conversation_id.clone(),
            },
            session_name: None,
            tools: None,
            exclude_tools: Vec::new(),
            workspace,
            runtime_root,
        };
        let error = LocalConversationInspection::compose(&paths, &conversation_id)
            .await
            .expect_err("a live but uninspectable child must not fake a durable live view");
        assert!(matches!(
            error,
            super::LocalRuntimeError::LiveInspectionUnavailable {
                conversation_id: ref actual,
                ..
            } if actual == &conversation_id
        ));

        drop(lease);
        let inspection = LocalConversationInspection::compose(&paths, &conversation_id)
            .await
            .expect("the same identity falls back after the live lease is gone");
        assert!(
            inspection.endpoint().is_ok(),
            "the post-terminal fallback is the local durable Runtime Client host"
        );
    }
}

#[cfg(all(test, feature = "mcp-fixture"))]
mod composition_tests {
    use std::sync::Arc;

    use super::{LocalConversationCore, LocalRuntimeDependencies, LocalRuntimePaths};
    use crate::events::types::{AttemptOutcome, RuntimeEvent};
    use crate::message::content::TextBlock;
    use crate::message::types::{AssistantContentBlock, MessageBlock, UserContentBlock};
    use crate::model::event::ModelEvent;
    use crate::model::finish::ModelFinishReason;
    use crate::model::types::{ModelProtocol, ModelRequest};
    use crate::runtime::identity::{ConversationId, McpServerId, ToolCallId, ToolId};
    use crate::scripted_suites::support::fake::{FakeModel, FakeStep, fake_model};
    use crate::scripted_suites::support::model::{
        FixtureModel, ScriptedAdapterFactory, fixture_catalog_document, fixture_registry,
    };
    use crate::tools::mcp::fixture::{
        ECHO_CALL_COUNT_FILE_ENV, FIXTURE_MODE_ENV, FixtureServer, TOOL_PREFIX_ENV,
        fixture_spawn_args, serve_if_fixture_mode,
    };
    use crate::tools::types::{ToolCall, ToolCallStart, ToolExecutionStatus};

    const SERVER_NAME: &str = "fixture";
    const TOOL_NAME: &str = "fixture_echo";
    const TEST_AGENT: &str = "explore";

    struct ComposedFixture {
        _root: tempfile::TempDir,
        runtime: crate::local_runtime::HeadlessConversationRuntime,
        model: Arc<FakeModel>,
        echo_call_count_file: std::path::PathBuf,
        tool_id: ToolId,
    }

    fn paths(root: &std::path::Path, workspace: std::path::PathBuf) -> LocalRuntimePaths {
        LocalRuntimePaths {
            models: root.join("models.jsonc"),
            config: root.join("rustx.jsonc"),
            skill_paths: Vec::new(),
            no_skills: true,
            no_builtin_tools: false,
            no_tools: false,
            startup_session: super::StartupSession::Empty,
            session_name: None,
            tools: None,
            exclude_tools: Vec::new(),
            workspace,
            runtime_root: root.join("runtime"),
        }
    }

    fn mcp_call_script(tool_id: &ToolId, arguments: serde_json::Value) -> Vec<FakeStep> {
        let call_id = ToolCallId::new("mcp-call-163");
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(ModelEvent::ToolCallStarted {
                block_index: crate::message::types::ContentBlockIndex::new(0),
                call: ToolCallStart {
                    id: call_id.clone(),
                    tool_id: tool_id.clone(),
                    name: TOOL_NAME.to_owned(),
                },
            }),
            FakeStep::Emit(ModelEvent::ToolCallCompleted {
                block_index: crate::message::types::ContentBlockIndex::new(0),
                call: ToolCall {
                    id: call_id,
                    tool_id: tool_id.clone(),
                    name: TOOL_NAME.to_owned(),
                    arguments,
                },
            }),
            FakeStep::Emit(ModelEvent::Completed {
                finish_reason: ModelFinishReason::ToolCalls,
                usage: None,
            }),
        ]
    }

    fn final_answer_script() -> Vec<FakeStep> {
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(ModelEvent::TextDelta {
                block_index: crate::message::types::ContentBlockIndex::new(0),
                text: "done".to_owned(),
            }),
            FakeStep::Emit(ModelEvent::Completed {
                finish_reason: ModelFinishReason::Stop,
                usage: None,
            }),
        ]
    }

    async fn compose_fixture(test_name: &str, arguments: serde_json::Value) -> ComposedFixture {
        let root = tempfile::tempdir().expect("fixture root");
        let workspace = root.path().join("workspace");
        std::fs::create_dir_all(workspace.join(".agents/subagents/explore")).expect("workspace");
        std::fs::write(
            workspace.join(".agents/subagents/explore/instructions.md"),
            "Read-only fixture subagent instructions.\n",
        )
        .expect("subagent instructions");

        let server_id = McpServerId::new(SERVER_NAME);
        let tool_id = ToolId::new(crate::tools::mcp::mcp_tool_id(&server_id, TOOL_NAME));
        let model = fake_model(vec![
            mcp_call_script(&tool_id, arguments),
            final_answer_script(),
        ]);
        let adapter: Arc<dyn crate::model::adapter::ModelAdapter> = model.clone();
        let fixture_model =
            FixtureModel::text("scripted/scripted", ModelProtocol::OpenAiChatCompletions);
        let factory = ScriptedAdapterFactory::new(adapter);
        let registry = fixture_registry(std::slice::from_ref(&fixture_model), &factory);
        std::fs::write(
            root.path().join("models.jsonc"),
            serde_json::to_vec_pretty(&fixture_catalog_document(std::slice::from_ref(
                &fixture_model,
            )))
            .expect("model catalog"),
        )
        .expect("models.jsonc");

        let echo_call_count_file = root.path().join("echo-call-count");
        let executable = std::env::current_exe().expect("test executable");
        let config_document = serde_json::json!({
            "schemaVersion": 6,
            "agentId": "agent-parent",
            "model": {"model": "scripted/scripted"},
            "context": {"reserveTokens": 0, "keepRecentTokens": 0},
            "defaultTools": ["read", "subagent", TOOL_NAME],
            "mcpServers": {
                SERVER_NAME: {
                    "type": "stdio",
                    "command": executable,
                    "args": fixture_spawn_args(test_name),
                    "env": {
                        FIXTURE_MODE_ENV: "1",
                        TOOL_PREFIX_ENV: "fixture_",
                        ECHO_CALL_COUNT_FILE_ENV: echo_call_count_file,
                    },
                },
            },
            "subagents": {
                "maxConcurrent": 4,
                "definitions": {
                    TEST_AGENT: {
                        "description": "Read the workspace",
                        "instructionsFile": ".agents/subagents/explore/instructions.md",
                        "tools": {"builtin": ["read"]},
                    },
                },
                "main": [TEST_AGENT],
                "workflow": [],
            },
        });
        let config_bytes = serde_json::to_vec_pretty(&config_document).expect("runtime config");
        std::fs::write(root.path().join("rustx.jsonc"), &config_bytes).expect("rustx.jsonc");
        let runtime_config =
            super::super::config::CurrentRuntimeConfig::from_jsonc_slice(&config_bytes)
                .expect("config");
        let runtime = LocalConversationCore::compose_from_config(
            &paths(root.path(), workspace),
            &LocalRuntimeDependencies::default(),
            registry,
            runtime_config.clone(),
            super::super::session::SessionPersistentState {
                model: runtime_config.model.clone(),
            },
            ConversationId::new("conv-163-composition"),
            root.path().join("artifacts"),
        )
        .await
        .expect("real production composition");

        ComposedFixture {
            _root: root,
            runtime: runtime.into_headless(),
            model,
            echo_call_count_file,
            tool_id,
        }
    }

    async fn settle(fixture: &ComposedFixture) {
        let settled = fixture.runtime.runtime().settlement_signal().notified();
        fixture
            .runtime
            .runtime()
            .submit_inbound(vec![UserContentBlock::Text(TextBlock {
                text: "use the fixture tool".to_owned(),
            })])
            .expect("inbound accepted");
        tokio::time::timeout(std::time::Duration::from_secs(10), settled)
            .await
            .expect("runtime settlement liveness");
    }

    fn request_tool_names(request: &ModelRequest) -> Vec<String> {
        request.tools.iter().map(|tool| tool.name.clone()).collect()
    }

    fn canonical_tool_call(ledger: &[MessageBlock]) -> ToolCall {
        ledger
            .iter()
            .find_map(|message| match message {
                MessageBlock::Assistant(assistant) => assistant.content.iter().find_map(|block| {
                    if let AssistantContentBlock::ToolCall(call) = block {
                        Some(call.clone())
                    } else {
                        None
                    }
                }),
                _ => None,
            })
            .expect("canonical assistant tool call")
    }

    fn journal(fixture: &ComposedFixture) -> Vec<RuntimeEvent> {
        fixture
            .runtime
            .tool_runtime()
            .durable_store()
            .read_events(None, 256)
            .expect("event journal")
            .events
            .into_iter()
            .map(|event| event.event)
            .collect()
    }

    /// The real local composition publishes a non-empty named-subagent
    /// intrinsic and an MCP fixture tool into one frozen request surface.
    /// The parent then calls only MCP, commits its result, and continues; the
    /// mere presence of `subagent` never enters child execution.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn named_subagent_and_mcp_share_one_frozen_parent_generation() {
        if serve_if_fixture_mode(FixtureServer::from_env()).await {
            return;
        }
        let test_name = "local_runtime::composition::composition_tests::named_subagent_and_mcp_share_one_frozen_parent_generation";
        let fixture = compose_fixture(test_name, serde_json::json!({})).await;
        settle(&fixture).await;

        let requests = fixture.model.requests();
        assert_eq!(
            requests.len(),
            2,
            "MCP result continues into one next model turn"
        );
        let request_surface = fixture.runtime.runtime().runtime_resources();
        let frozen_tools = request_surface
            .capability()
            .tool_registry()
            .model_definitions();
        assert_eq!(requests[0].tools, frozen_tools);
        assert_eq!(requests[1].tools, requests[0].tools);
        let names = request_tool_names(&requests[0]);
        assert!(names.iter().any(|name| name == "subagent"), "{names:?}");
        assert!(names.iter().any(|name| name == TOOL_NAME), "{names:?}");

        let expected_call = ToolCall {
            id: ToolCallId::new("mcp-call-163"),
            tool_id: fixture.tool_id.clone(),
            name: TOOL_NAME.to_owned(),
            arguments: serde_json::json!({}),
        };
        let ledger = fixture
            .runtime
            .runtime()
            .historical_canonical_history()
            .expect("canonical history");
        assert_eq!(canonical_tool_call(&ledger), expected_call);
        let tool_result = ledger
            .iter()
            .find_map(|message| match message {
                MessageBlock::Tool(tool) => Some(tool),
                _ => None,
            })
            .expect("committed MCP ToolResult");
        assert_eq!(tool_result.tool_call_id, expected_call.id);
        assert_eq!(tool_result.tool_id, expected_call.tool_id);
        assert!(matches!(
            tool_result.result.status,
            ToolExecutionStatus::Success
        ));
        assert_eq!(
            std::fs::read_to_string(&fixture.echo_call_count_file)
                .expect("MCP echo call count")
                .lines()
                .count(),
            1,
            "the MCP fixture executes exactly once"
        );
        assert!(requests[1].messages.iter().any(|message| matches!(
            message,
            crate::model::input::ModelInputMessage::Canonical(MessageBlock::Tool(tool))
                if tool.tool_call_id == expected_call.id
        )));

        let journal = journal(&fixture);
        assert!(matches!(
            journal.last(),
            Some(RuntimeEvent::AttemptCompleted {
                finish_reason: ModelFinishReason::Stop,
                ..
            })
        ));
        assert_eq!(
            journal
                .iter()
                .filter(|event| AttemptOutcome::from_terminal_event(event).is_some())
                .count(),
            1
        );
        assert!(
            fixture
                .runtime
                .runtime()
                .subagents()
                .expect("parent subagent registry")
                .all_snapshots()
                .is_empty()
        );
        assert!(!journal.iter().any(|event| matches!(
            event,
            RuntimeEvent::SubagentOwnershipCommitted { .. }
                | RuntimeEvent::SubagentTerminalPublished { .. }
        )));

        fixture
            .runtime
            .runtime()
            .shutdown()
            .await
            .expect("runtime shutdown");
    }

    /// A structurally complete MCP call with invalid business arguments still
    /// crosses canonical assembly and reaches the ordinary strict preflight
    /// rejection; the fixture executor is never entered.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn malformed_mcp_arguments_remain_a_normal_preflight_rejection() {
        if serve_if_fixture_mode(FixtureServer::from_env()).await {
            return;
        }
        let test_name = "local_runtime::composition::composition_tests::malformed_mcp_arguments_remain_a_normal_preflight_rejection";
        let fixture = compose_fixture(test_name, serde_json::json!({"unexpected": true})).await;
        settle(&fixture).await;

        let ledger = fixture
            .runtime
            .runtime()
            .historical_canonical_history()
            .expect("canonical history");
        let expected_call_id = ToolCallId::new("mcp-call-163");
        assert_eq!(
            canonical_tool_call(&ledger),
            ToolCall {
                id: expected_call_id.clone(),
                tool_id: fixture.tool_id.clone(),
                name: TOOL_NAME.to_owned(),
                arguments: serde_json::json!({"unexpected": true}),
            }
        );
        let tool_result = ledger
            .iter()
            .find_map(|message| match message {
                MessageBlock::Tool(tool) if tool.tool_call_id == expected_call_id => Some(tool),
                _ => None,
            })
            .expect("preflight ToolResult");
        let ToolExecutionStatus::Failed { error } = &tool_result.result.status else {
            panic!(
                "strict preflight must produce a failed result slot: {:?}",
                tool_result.result.status
            );
        };
        assert!(error.contains("canonical schema"));
        assert!(error.contains("unexpected"));
        assert_eq!(
            std::fs::read_to_string(&fixture.echo_call_count_file)
                .unwrap_or_default()
                .lines()
                .count(),
            0,
            "strict preflight rejects before MCP dispatch"
        );
        let requests = fixture.model.requests();
        assert_eq!(requests.len(), 2, "the normal failed result continues");
        assert!(requests[1].messages.iter().any(|message| matches!(
            message,
            crate::model::input::ModelInputMessage::Canonical(MessageBlock::Tool(tool))
                if tool.tool_call_id == expected_call_id
        )));
        let events = journal(&fixture);
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, RuntimeEvent::ToolExecutionStarted { .. }))
        );
        assert!(events.iter().any(|event| matches!(
            event,
            RuntimeEvent::ToolMessageCommitted { tool_call_id, .. }
                if *tool_call_id == expected_call_id
        )));
        assert!(matches!(
            events.last(),
            Some(RuntimeEvent::AttemptCompleted {
                finish_reason: ModelFinishReason::Stop,
                ..
            })
        ));

        fixture
            .runtime
            .runtime()
            .shutdown()
            .await
            .expect("runtime shutdown");
    }
}
