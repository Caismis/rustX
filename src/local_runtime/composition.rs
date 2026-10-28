//! The one Rust-side local runtime composition owner (Issue #42, Issue
//! #61).
//!
//! ```text
//! resolved launch input (from the Rust launch boundary)
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
//!         +-- into_bound(): bind RuntimeClientHost over the inert runtime
//!         |       -> the App Server's SessionRuntimeManager arms its
//!         |          pre-activation work, then activates
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
//! > Durable Session identity and graph state belong to `SessionController`.
//! > Concurrent runtime residency belongs to `SessionRuntimeManager`, which
//! > composes each resident runtime through this core for the App Server.
//!
//! A client — the TUI or the Web console — speaks the App Server protocol and
//! never assembles provider adapters, model parameters, context engines, tool
//! registries, capability coordinators, or summary models.
//!
//! # Ordering
//!
//! Composition follows a fixed order, and the initial capability candidate
//! is **prepared and committed before the runtime activates**. Every selected
//! MCP source reaches a live terminal result before publication. Early
//! preparation is unarmed until the first native submission receipt starts its
//! shared readiness budget. Workflow dependencies require successful live
//! validation. A *core*
//! composition failure therefore never leaves a partially initialized
//! runtime: composition returns an error before any client can attach.
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
//! isolated by the capability plane itself (preparation records
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
use std::path::PathBuf;
use std::sync::Arc;

use crate::capabilities::{AgentActivation, CapabilityCoordinator, CapabilityCoordinatorConfig};
use crate::context::{DefaultTokenEstimator, TokenEstimator};
use crate::model::catalog::{CredentialEnvironment, ModelCatalogError};
use crate::model::invocation::{ModelBindingRegistry, ModelInvocationError};
use crate::model::session::SessionModelState;
use crate::runtime::RuntimeResourceRevision;
use crate::runtime::conversation_runtime::{
    ConversationContextConfig, ConversationRuntime, ConversationRuntimeError,
    RuntimeConversationConfig,
};
use crate::runtime::identity::ConversationId;
use crate::runtime::resources::{PreparedRuntimeResources, RuntimeResourceSnapshot};
use crate::runtime::workflow::{WorkflowOutputLatch, WorkflowRuntime};
use crate::runtime_client::host::{
    HostConstructionError, RuntimeClientHost, RuntimeClientHostConfig,
};
use crate::skills::SkillDiscoveryConfig;
use crate::tools::executor::ToolRegistry;
use crate::tools::native::{NativeToolResources, register_native_tools};
use crate::tools::runtime::ConversationToolRuntime;

use super::config::{CurrentRuntimeConfig, CurrentRuntimeConfigError};
use super::configuration::AdmittedSessionConfig;

use super::session::{
    SessionCatalog, SessionError, SessionId, SessionNodeId, SessionNodeOrigin,
    SessionPersistentState,
};
use resources::validate_subagent_catalog;
use resources::validate_workflow_tool_name_collisions;

mod resources;
mod runtimes;
mod subagent_child;
#[cfg(all(test, feature = "mcp-fixture"))]
mod tests;

pub(crate) use resources::LocalRuntimeResourceLoader;
pub(in crate::local_runtime) use resources::admitted_source_demand;
pub(crate) use resources::captured_mcp_bindings;
pub use runtimes::{
    HeadlessConversationRuntime, LocalConversationRuntime, LocalConversationShutdownError,
    LocalProjectionJoinFailure,
};
pub(crate) use subagent_child::ChildPreparation;
#[cfg(test)]
pub(crate) use subagent_child::{arm_test_preparation_drain_gate, arm_test_preparation_gate};

/// Explicit composition startup routing, never a catalog-global selection.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum StartupSession {
    /// Create an independent empty Session for this composition.
    #[default]
    Empty,
    /// Bind an explicitly named Session and optionally choose its graph node.
    /// Reading the default node does not mutate the catalog.
    Select {
        /// The persisted Session to bind.
        session: SessionId,
        /// The lineage node to bind; the Session's own active node when
        /// absent.
        node: Option<SessionNodeId>,
    },
}

/// Host controls and injectable non-model dependencies of composition.
///
/// Model bindings are deliberately not injectable: production constructs the
/// supported adapters directly from the validated catalog binding. Tests that
/// need provider synchronization use an explicit catalog endpoint aimed at a
/// local HTTP fixture.
pub struct LocalRuntimeDependencies {
    /// Local product Session selection; never part of effective configuration.
    pub startup_session: StartupSession,
    /// Optional test/embedding provider environment. Production uses the
    /// resolver's captured host snapshot, never a second launch-time read.
    pub credentials: Option<Arc<dyn CredentialEnvironment>>,
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
            startup_session: StartupSession::Empty,
            credentials: None,
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

/// Captured at native submission receipt, before any load or source-resolution wait.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SubmissionIntent(tokio::time::Instant);
impl SubmissionIntent {
    pub(crate) fn now() -> Self {
        Self(tokio::time::Instant::now())
    }
}

#[derive(Debug, Default)]
struct InitialSubmissionState {
    received: Option<tokio::time::Instant>,
    recovery_received: Option<tokio::time::Instant>,
    policy: Option<super::config::McpStartupPolicyDocument>,
}

/// Shared by a loading owner and a submission that may arrive later. Early
/// preparation has no elapsed deadline; the first native intent arms it once.
#[derive(Clone, Debug, Default)]
pub(crate) struct McpInitialSubmission {
    state: Arc<std::sync::Mutex<InitialSubmissionState>>,
    deadline: crate::capabilities::McpStartupDeadline,
    claim: Arc<std::sync::Mutex<Option<super::mcp_drafts::DraftClaim>>>,
}
impl McpInitialSubmission {
    pub(crate) fn with_claim(self, claim: super::mcp_drafts::DraftClaim) -> Self {
        Self {
            claim: Arc::new(std::sync::Mutex::new(Some(claim))),
            ..self
        }
    }
    pub(crate) fn has_submission(&self) -> bool {
        self.state
            .lock()
            .expect("initial submission")
            .received
            .is_some()
    }
    pub(crate) fn release_preparation(&self) {
        self.claim.lock().expect("initial MCP claim").take();
    }
    async fn consume_preparation(
        &self,
        paths: &AdmittedSessionConfig,
        cancellation: &crate::runtime::cancellation::CancellationSignal,
    ) -> Result<Option<crate::capabilities::McpPreparation>, LocalRuntimeError> {
        let claim = self.claim.lock().expect("initial MCP claim").take();
        match claim {
            Some(claim) => claim.consume(paths, cancellation).await.map_err(|detail| {
                if cancellation.is_cancelled() {
                    LocalRuntimeError::Cancelled
                } else {
                    LocalRuntimeError::Capability { detail }
                }
            }),
            None => Ok(None),
        }
    }
    pub(crate) fn record(&self, intent: SubmissionIntent) -> Result<(), String> {
        let mut state = self.state.lock().expect("initial submission");
        state.received = Some(
            state
                .received
                .map_or(intent.0, |received| received.min(intent.0)),
        );
        self.arm(&state)
    }
    /// A load is only a prospective recovery receipt. Ordinary empty attaches
    /// remain unarmed until durable evidence proves accepted executable work.
    pub(crate) fn note_recovery_receipt(&self, receipt: SubmissionIntent) {
        let mut state = self.state.lock().expect("initial submission");
        state.recovery_received = Some(
            state
                .recovery_received
                .map_or(receipt.0, |received| received.min(receipt.0)),
        );
    }
    fn recognize_recovery_intent(
        &self,
        store: &dyn crate::durable::ConversationStore,
    ) -> Result<(), LocalRuntimeError> {
        let received = self
            .state
            .lock()
            .expect("initial submission")
            .recovery_received;
        let Some(received) = received else {
            return Ok(());
        };
        // Recognition is read-only. Native bootstrap still owns reconciliation
        // and consumes the same existing recovery policy after ownership transfer.
        let pending = store
            .load_pending()
            .map_err(|error| LocalRuntimeError::ToolRuntime {
                detail: error.to_string(),
            })?;
        // A recovered Claim only retires its prior owner; only an Accepted
        // Workflow can start fresh work and arm this load's original receipt.
        let scheduled = store.scheduled_workflow_pending(256).map_err(|error| {
            LocalRuntimeError::ToolRuntime {
                detail: error.to_string(),
            }
        })?;
        let accepted_workflow = scheduled.iter().any(|head| {
            matches!(
                head.state,
                crate::runtime::workflow::scheduled::lifecycle::ScheduledWorkflowState::Accepted { .. }
            )
        });
        let executable = if pending.is_empty() && !accepted_workflow {
            let evidence = crate::runtime::recovery::RecoveryEvidence::reconstruct(store).map_err(
                |error| LocalRuntimeError::ToolRuntime {
                    detail: error.to_string(),
                },
            )?;
            matches!(
                crate::runtime::recovery::RecoveryPlan::classify(&evidence).resume(),
                crate::runtime::recovery::ResumeDisposition::ContinueAdoptedTurn { .. }
            )
        } else {
            true
        };
        if executable {
            self.record(SubmissionIntent(received))
                .map_err(|detail| LocalRuntimeError::Capability { detail })?;
        }
        Ok(())
    }
    pub(crate) fn bind_policy(
        &self,
        policy: super::config::McpStartupPolicyDocument,
    ) -> Result<(), String> {
        let mut state = self.state.lock().expect("initial submission");
        state.policy.get_or_insert(policy);
        self.arm(&state)
    }
    fn arm(&self, state: &InitialSubmissionState) -> Result<(), String> {
        if let (Some(received), Some(policy)) = (state.received, state.policy) {
            let deadline = policy.deadline_from(received)?;
            self.deadline.arm(deadline);
        }
        Ok(())
    }
    pub(crate) fn deadline(&self) -> crate::capabilities::McpStartupDeadline {
        self.deadline.clone()
    }
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
    /// The configuration-owner resource preparation of a root Session
    /// composition. A subagent child's generation is frozen by its parent and
    /// is never prepared again, so it has none.
    resource_loader: Option<Arc<LocalRuntimeResourceLoader>>,
}

impl std::fmt::Debug for LocalConversationCore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalConversationCore")
            .field("conversation_id", self.tool_runtime.conversation_id())
            .finish_non_exhaustive()
    }
}

impl LocalConversationCore {
    /// Settle all semantic owners of this core, which has no presentation worker.
    ///
    /// # Errors
    /// Returns the native runtime settlement failure.
    pub async fn shutdown(
        &self,
    ) -> Result<(), crate::runtime::conversation_runtime::ShutdownError> {
        self.runtime.shutdown().await
    }

    /// Composes the shared semantic runtime from validated resolved launch inputs.
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
    /// # Panics
    /// Panics only if a generated Conversation database path has no parent.
    pub async fn compose(
        paths: &AdmittedSessionConfig,
        dependencies: &LocalRuntimeDependencies,
    ) -> Result<Self, LocalRuntimeError> {
        Self::compose_for_submission(paths, dependencies, McpInitialSubmission::default()).await
    }

    pub(crate) async fn compose_for_submission(
        paths: &AdmittedSessionConfig,
        dependencies: &LocalRuntimeDependencies,
        startup: McpInitialSubmission,
    ) -> Result<Self, LocalRuntimeError> {
        startup.note_recovery_receipt(SubmissionIntent::now());
        let lifecycle = Arc::new(
            crate::runtime::local_storage::ProductController::acquire(&paths.runtime_root)
                .map_err(|error| LocalRuntimeError::ToolRuntime {
                    detail: error.to_string(),
                })?,
        );
        let mut state = SessionPersistentState::from_input(&paths.input);
        state.model = Some(paths.config.initial_model().clone());
        let mut catalog = match SessionCatalog::open_existing(lifecycle.root())? {
            Some(catalog) => catalog,
            None => SessionCatalog::create_unpublished(lifecycle.root(), &state)?,
        };
        catalog.retain_lifecycle(lifecycle.clone());
        let planned = match &dependencies.startup_session {
            StartupSession::Empty if !catalog.is_published() => {
                catalog.plan_unchanged(&catalog.persisted_session_ids()[0])
            }
            StartupSession::Empty => {
                let prepared = catalog.prepare_session(&state, &[])?;
                catalog.plan_session(&prepared, SessionNodeOrigin::New)?
            }
            StartupSession::Select { session, node } => {
                catalog.plan_attachment(session, node.as_ref())?
            }
        };
        let (session_id, node, intent) = planned.destination_lineage()?;
        let manager = super::configuration::UserConfigManager::new(paths.sources.clone())
            .map_err(|detail| LocalRuntimeError::Capability { detail })?;
        let current = manager
            .resolve_session(&intent.input())
            .map_err(|error| LocalRuntimeError::SessionConfiguration {
                session_id: session_id.clone(),
                diagnostic: error.diagnostic,
            })?
            .admit(|| paths.credentials.clone())
            .map_err(|detail| LocalRuntimeError::Capability { detail })?;
        startup
            .bind_policy(current.config.mcp_startup_policy)
            .map_err(|detail| LocalRuntimeError::Capability { detail })?;
        let registry = load_model_registry(&current, dependencies)?;
        let database = catalog.database_path(&session_id, &node.conversation_id);
        let allocation = database
            .parent()
            .expect("Conversation database has an allocation")
            .to_path_buf();
        let core = Self::compose_from_config_started(
            &current,
            dependencies,
            registry,
            current.config.as_ref().clone(),
            intent,
            node.conversation_id,
            allocation,
            lifecycle,
            startup,
        )
        .await?;
        catalog.commit_planned(planned)?;
        Ok(core)
    }

    /// Composes one selected native `SessionNode`'s linear conversation from
    /// current runtime configuration plus the Session-local persistent state.
    /// This method never reads or writes `SessionCatalog` state.
    #[cfg(test)]
    #[allow(clippy::too_many_lines, clippy::too_many_arguments)]
    pub(crate) async fn compose_from_config(
        paths: &AdmittedSessionConfig,
        dependencies: &LocalRuntimeDependencies,
        registry: ModelBindingRegistry,
        runtime_config: CurrentRuntimeConfig,
        session_state: SessionPersistentState,
        conversation_id: ConversationId,
        artifacts_root: PathBuf,
        lifecycle: Arc<crate::runtime::local_storage::ProductController>,
    ) -> Result<Self, LocalRuntimeError> {
        Self::compose_from_config_started(
            paths,
            dependencies,
            registry,
            runtime_config,
            session_state,
            conversation_id,
            artifacts_root,
            lifecycle,
            McpInitialSubmission::default(),
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn compose_from_config_started(
        paths: &AdmittedSessionConfig,
        dependencies: &LocalRuntimeDependencies,
        registry: ModelBindingRegistry,
        runtime_config: CurrentRuntimeConfig,
        session_state: SessionPersistentState,
        conversation_id: ConversationId,
        artifacts_root: PathBuf,
        lifecycle: Arc<crate::runtime::local_storage::ProductController>,
        startup: McpInitialSubmission,
    ) -> Result<Self, LocalRuntimeError> {
        startup.note_recovery_receipt(SubmissionIntent::now());
        startup
            .bind_policy(paths.config.mcp_startup_policy)
            .map_err(|detail| LocalRuntimeError::Capability { detail })?;
        let access = Arc::new(
            crate::runtime::local_storage::ConversationAccess::start(lifecycle, &artifacts_root)
                .map_err(|e| LocalRuntimeError::ToolRuntime {
                    detail: e.to_string(),
                })?,
        );
        Self::compose_with_access(
            paths,
            dependencies,
            registry,
            runtime_config,
            session_state,
            conversation_id,
            artifacts_root,
            access,
            startup,
            &crate::runtime::cancellation::CancellationSignal::new(),
        )
        .await
    }

    /// The shared native composition consumes already-admitted allocation authority.
    /// Session callers must acquire it before resolving authoritative cold settings.
    #[allow(clippy::too_many_lines, clippy::too_many_arguments)]
    pub(crate) async fn compose_with_access(
        paths: &AdmittedSessionConfig,
        dependencies: &LocalRuntimeDependencies,
        registry: ModelBindingRegistry,
        runtime_config: CurrentRuntimeConfig,
        session_state: SessionPersistentState,
        conversation_id: ConversationId,
        artifacts_root: PathBuf,
        conversation_access: Arc<crate::runtime::local_storage::ConversationAccess>,
        startup: McpInitialSubmission,
        cancellation: &crate::runtime::cancellation::CancellationSignal,
    ) -> Result<Self, LocalRuntimeError> {
        startup.note_recovery_receipt(SubmissionIntent::now());
        // Keep the one-shot preparation state off the awaiting caller's stack.
        Box::pin(async move {
            startup
                .bind_policy(paths.config.mcp_startup_policy)
                .map_err(|detail| LocalRuntimeError::Capability { detail })?;
            if cancellation.is_cancelled() {
                return Err(LocalRuntimeError::Cancelled);
            }
            let product_root = conversation_access.root().to_path_buf();
            // The current runtime default was validated by the composition
            // caller before any first-Session publication. Validate it here too
            // for direct low-level callers, while the selected durable Session
            // model remains an independent Session-local choice.
            paths
                .validate_resource_authority()
                .map_err(|detail| LocalRuntimeError::Capability { detail })?;
            let project_context_files = paths.project_context_files.clone();
            SessionModelState::new(registry.clone(), runtime_config.initial_model().clone())?;
            // A persisted selection the catalog no longer admits composes an
            // unavailable Session: it loads, advertises this catalog and admits
            // nothing until a valid selection is committed.
            let model = SessionModelState::restore(
                registry.clone(),
                session_state
                    .model
                    .clone()
                    .unwrap_or_else(|| runtime_config.initial_model().clone()),
            )?;

            // Initial Root Plugin selection comes from the resolved CFG3 profile.
            // Later configuration adoption publishes a new profile; admitted work
            // retains its generation. Conversation Todo/Goal state owners remain
            // stable independently of whether a generation exposes their Tools.
            // Named Agents resolve their own independent Plugin profiles.
            let extensions = runtime_config.extension_composition();

            // 5-6. The conversation identity authority and the one conversation
            // tool runtime (workspace, runtime-private artifact root, canonical
            // mailbox, background registry, base authorized environment, and the
            // Todo/Goal domain owners).
            let base_environment = runtime_config.tool_environment()?;
            let mut tool_runtime_config = crate::tools::runtime::ConversationRuntimeConfig::new(
                &paths.workspace,
                artifacts_root.clone(),
            )
            // The one frozen composition, handed to its materialization owner.
            // Everything Todo downstream — the Tool plane the coordinator
            // composes, the presentation Agent Status consumes, the Runtime
            // Client projection — is derived from what this materializes, never
            // configured a second time.
            .with_extensions(extensions.clone());
            tool_runtime_config.lifecycle = Some(conversation_access.clone());
            tool_runtime_config.environment = Some(base_environment.clone());
            let mut tool_runtime =
                ConversationToolRuntime::from_config(conversation_id.clone(), tool_runtime_config)
                    .map_err(|error| LocalRuntimeError::ToolRuntime {
                        detail: format!("{error:?}"),
                    })?;

            tool_runtime.uploads = Some(Arc::new(
                super::session::uploads::SessionUploadResolver::new(
                    &product_root,
                    conversation_id.clone(),
                ),
            ));

            // 7-8. The base tool registry with the explicit native composition,
            // using *this* conversation's background registry for the
            // `execution` intrinsic and this conversation's subagent
            // registry for the `subagent` intrinsic (Issue #60).
            // The named-subagent catalog of the launch generation. It is
            // loaded before the base registry, because the `subagent`
            // intrinsic's model-facing description is generated from exactly
            // the catalog this generation admits.
            let subagent_catalog = paths.subagents.clone();
            let main_admission = runtime_config
                .agent
                .agents
                .iter()
                .cloned()
                .collect::<BTreeSet<_>>();
            let main_catalog = subagent_catalog.selected_definitions(&main_admission);
            let mut workflows = paths.workflows.as_ref().clone();
            //
            // The frozen model timeout policy is resolved once here so the
            // parent runtime and every launched subagent child share exactly
            // the same deadlines (Issue #138): the child inherits the policy
            // through its typed startup specification and applies it locally.
            let model_timeout_policy = runtime_config.timeout_policy()?;
            // The frozen tool execution-liveness policy (Issue #204) follows the
            // same inheritance: one generic policy for the parent runtime and
            // every subagent child, carried through the typed child spec.
            let tool_deadline_policy = runtime_config.tool_deadline_policy()?;
            let subagents = crate::runtime::subagent::SubagentRegistry::new(
                crate::runtime::subagent::SubagentRegistryConfig {
                    conversation_id: tool_runtime.conversation_id().clone(),
                    agent_id: runtime_config.agent_id.clone(),
                    mailbox: tool_runtime.mailbox(),
                    clock: Arc::new(crate::runtime::types::SystemClock),
                    monotonic_clock: Arc::new(crate::runtime::SystemMonotonicClock::new()),
                    spawn: crate::runtime::subagent::SubagentSpawnPlan {
                        session_id: crate::runtime::identity::SessionId::parse(
                            artifacts_root
                                .parent()
                                .and_then(std::path::Path::parent)
                                .and_then(std::path::Path::file_name)
                                .and_then(std::ffi::OsStr::to_str)
                                .ok_or_else(|| LocalRuntimeError::ToolRuntime {
                                    detail: "Conversation has no Session allocation".into(),
                                })?,
                        )
                        .map_err(|error| {
                            LocalRuntimeError::ToolRuntime {
                                detail: error.clone(),
                            }
                        })?,
                        program: match dependencies.child_program.clone() {
                            Some(program) => program,
                            None => {
                                std::env::current_exe().map_err(|error| LocalRuntimeError::Io {
                                    path: PathBuf::from("<current exe>"),
                                    detail: error.to_string(),
                                })?
                            }
                        },
                        // Child conversation stores and their disposable
                        // physical incarnations live below the launch's one
                        // runtime-private root. Keeping this identity namespace
                        // independent of the selected Session's artifact path
                        // lets a later generic inspection attach by
                        // `child_conversation_id` alone.
                        product_root: crate::runtime::local_storage::ProductRoot::clone(
                            &conversation_access,
                        ),
                    },
                    // The Session's authorized execution binding. Children
                    // share it; no Git or isolated resource is derived from it.
                    workspace: tool_runtime.workspace().root().to_path_buf(),
                    // Generation-owned capacity belongs to the live registry;
                    // safe-boundary publication replaces capacity when no child owns it.
                    max_active: runtime_config.subagents.max_concurrent,
                },
            );
            let workflow_runtime = WorkflowRuntime::new(
                subagents.clone(),
                tool_runtime.durable_store(),
                tool_runtime.workflows().clone(),
            );
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
            crate::tools::native::register_workflow_sources(
                &mut base_registry,
                &workflow_runtime,
                &workflows,
            )
            .map_err(|error| LocalRuntimeError::NativeTools {
                detail: format!("{error:?}"),
            })?;

            // Launch resolution froze the fixed User and Workspace Skill roots.
            // Package discovery reserves Workspace shadows before validation;
            // Agent profiles independently select prompt visibility.
            let skill_discovery = SkillDiscoveryConfig {
                automatic: paths.skill_sources.clone(),
            };

            // 9. The resolver supplied the complete CFG3 profile and policies.
            // The existing coordinator prepares only finite admitted source demand.
            let source_demand = admitted_source_demand(
                &runtime_config,
                &paths.subagents,
                &workflows,
                paths.managed_python.clone(),
            );
            if !source_demand.sources.is_empty() {
                startup.recognize_recovery_intent(tool_runtime.durable_store().as_ref())?;
            }
            let preparation = startup.consume_preparation(paths, cancellation).await?;
            let capability = CapabilityCoordinator::new_with_mcp_preparation(
                CapabilityCoordinatorConfig {
                    source_demand,
                    conversation_id: tool_runtime.conversation_id().clone(),
                    workspace: tool_runtime.workspace().clone(),
                    base_tool_registry: Arc::new(base_registry),
                    // Stable owner-backed Plugin registrations. Each generation's
                    // authored profile selects enabled Plugins independently of
                    // ordinary Tool availability; Todo/Goal state remains domain-owned.
                    extension_tools: tool_runtime.extension_tool_plane(),
                    agent_activation: AgentActivation {
                        profile: runtime_config.agent.clone(),
                        admitted_agents: subagent_catalog.names().into_iter().cloned().collect(),
                        admitted_workflows: workflows.entries().keys().cloned().collect(),
                        project_files: paths.root_agent_project_files.clone(),
                    },
                    skill_discovery,
                    mcp_servers: captured_mcp_bindings(&runtime_config, &paths.credentials)?,
                    base_environment,
                    environment_store_root: product_root
                        .join("environments")
                        .join(conversation_id.as_str()),
                },
                preparation,
            )
            .map_err(|error| LocalRuntimeError::Capability {
                detail: format!("{error:?}"),
            })?;

            let startup_owner = capability.clone();
            let outcome = async {
                // 10-11. Build one complete candidate off-side before publishing any
                // capability or resource state. The named-subagent catalog, its two
                // admission domains, the discovered Workflow programs, project
                // instructions, and the capability candidate are validated as one
                // coherent generation. Optional-source failures (individual MCP
                // servers, including managed Python packages) remain typed
                // availability state inside `prepare_candidate` (Issue #81); base
                // capability failures remain fatal.
                let mut retried = false;
                let (capability_snapshot, resource_data) = loop {
                    workflows = paths.workflows.as_ref().clone();
                    let mut candidate = capability
                        .prepare_startup_candidate(
                            paths.skill_discovery.clone(),
                            resources::workflow_source_demand(
                                &runtime_config,
                                &paths.subagents,
                                &workflows,
                            ),
                            startup.deadline(),
                            cancellation,
                        )
                        .await
                        .map_err(|error| LocalRuntimeError::Capability {
                            detail: format!("{error:?}"),
                        })?;
                    validate_workflow_tool_name_collisions(&candidate, &workflows).map_err(
                        |error| LocalRuntimeError::Capability {
                            detail: error.to_string(),
                        },
                    )?;
                    candidate
                        .admit_workflows(&mut workflows, &subagent_catalog, &workflow_runtime)
                        .map_err(|detail| LocalRuntimeError::Capability { detail })?;
                    let prepared = PreparedRuntimeResources::new(
                        project_context_files.clone(),
                        None,
                        crate::context::ContextAssembly::new(),
                        candidate,
                    )
                    .with_subagent_catalog(subagent_catalog.clone())
                    .with_workflow_catalog(workflows.clone())
                    .with_managed_python_catalog(paths.managed_python.clone())
                    .with_configuration(
                        crate::runtime::resources::RuntimeConfiguration {
                            resource_definitions: paths.inspection.definitions.clone(),
                            resource_diagnostics: paths.inspection.resource_diagnostics.clone(),
                            config: Arc::new(runtime_config.clone()),
                            models: registry.clone(),
                            authored: paths.authored(),
                        },
                    );
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
                    match capability.commit(candidate) {
                        Ok(snapshot) => break (snapshot, resource_data),
                        Err(crate::capabilities::CapabilityCommitError::StaleMcpCandidate {
                            ..
                        }) if !retried => {
                            retried = true;
                            capability
                                .settle_rejected_startup()
                                .await
                                .map_err(|error| LocalRuntimeError::Capability {
                                    detail: error.to_string(),
                                })?;
                        }
                        Err(error) => {
                            return Err(LocalRuntimeError::Capability {
                                detail: format!("{error:?}"),
                            });
                        }
                    }
                };
                let resources = Arc::new(RuntimeResourceSnapshot::from_prepared(
                    RuntimeResourceRevision::new(1),
                    resource_data,
                    capability_snapshot,
                ));
                let resource_loader = Arc::new(LocalRuntimeResourceLoader::new(
                    paths.clone(),
                    native_resources,
                    dependencies
                        .credentials
                        .clone()
                        .unwrap_or_else(|| Arc::new(paths.credentials.clone())),
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
                let runtime = ConversationRuntime::new_session_owned(RuntimeConversationConfig {
                    explicit_model: session_state.model.is_some(),
                    agent_id: runtime_config.agent_id.clone(),
                    model,
                    approval_mode: runtime_config.approval_mode,
                    model_timeout_policy,
                    tool_deadline_policy,
                    context: ConversationContextConfig {
                        policy: runtime_config.context_policy(),
                        estimator: Arc::clone(&dependencies.estimator),
                        // The root Agent's frozen native Agent Extension composition
                        // is materialized here, once, and never again (Issue #256).
                        // Its Todo half was materialized alongside the conversation's
                        // other owned resources in the tool runtime above.
                        status_engine: extensions
                            .agent_status_engine(Arc::new(crate::context::SystemClock)),
                    },
                    tool_runtime: tool_runtime.clone(),
                    capability: capability.clone(),
                    resources,
                    clock: None,
                    initial_messages,
                    subagents: Some(subagents),
                    workflow_output: None,
                })?;

                runtime.install_initialization_cancellation(cancellation.clone());

                let catalog_loader = resource_loader.clone();
                runtime.install_runtime_catalog_preparer(Arc::new(move |capability, adopted| {
                    catalog_loader.prepare_runtime_catalogs(capability, adopted)
                }));

                Ok(Self {
                    runtime,
                    tool_runtime,
                    capability,
                    workflow_output: None,
                    resource_loader: Some(resource_loader),
                })
            }
            .await;
            if outcome.is_err() {
                let failures = startup_owner.settle_failed_startup().await;
                if !failures.is_empty() {
                    return Err(LocalRuntimeError::Capability {
                        detail: format!(
                            "startup failed and MCP settlement failed: {}",
                            failures.join("; ")
                        ),
                    });
                }
            }
            if outcome.is_err() && cancellation.is_cancelled() {
                return Err(LocalRuntimeError::Cancelled);
            }
            outcome
        })
        .await
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
        let runtime = self.into_bound()?;
        runtime.activate();
        Ok(runtime)
    }

    /// Binds the Runtime Client host over the composed runtime and stops
    /// there: the returned runtime is **inert**.
    ///
    /// Binding and activation are separated so a caller with work of its
    /// own before activation — the runtime manager's display-projection
    /// arming, a child's interaction route and observation subscription — can
    /// place it between them. Every fallible composition step is then on the
    /// pre-activation side, and the activation that follows cannot fail.
    pub(crate) fn into_bound(self) -> Result<LocalConversationRuntime, LocalRuntimeError> {
        // 14. The Runtime Client projection/control/attachment adapter over
        // that runtime. Binding is a pre-activation composition decision
        // (Issue #61): the runtime is still inert here, so the host's
        // initial snapshot is the runtime's real state at the activation
        // cut and no bootstrap fact can fabricate a live client event.
        let host = RuntimeClientHost::new(RuntimeClientHostConfig {
            runtime: self.runtime.clone(),
            replay_limit: None,
        })?;

        Ok(LocalConversationRuntime { core: self, host })
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

/// Loads the current model catalog and constructs its resolved binding
/// authority. This is intentionally a composition concern, not a
/// `SessionCatalog` concern: callers can validate the current runtime default
/// before publishing a first durable Session.
pub(crate) fn load_model_registry(
    paths: &AdmittedSessionConfig,
    dependencies: &LocalRuntimeDependencies,
) -> Result<ModelBindingRegistry, LocalRuntimeError> {
    let resolved = paths.models.resolve(
        dependencies
            .credentials
            .as_deref()
            .unwrap_or(&paths.credentials),
    )?;
    Ok(ModelBindingRegistry::new(resolved)?)
}

/// A local runtime composition failure.
///
/// Failures arise before composition can publish a usable runtime. Cancellation
/// reports the owning preparation's settled termination. No variant carries a
/// credential value.
#[derive(Debug, Clone, PartialEq)]
pub enum LocalRuntimeError {
    /// The composition owner cancelled and settled initial preparation.
    Cancelled,
    /// Current sources rejected an explicitly persisted Session selection.
    SessionConfiguration {
        session_id: SessionId,
        diagnostic: Box<super::diagnostics::Diagnostic>,
    },
    /// A startup file could not be read.
    Io {
        /// The path that failed.
        path: PathBuf,
        /// The failure detail.
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
}

impl std::fmt::Display for LocalRuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => f.write_str("runtime composition was cancelled"),
            Self::Io { path, detail } => write!(f, "cannot read {}: {detail}", path.display()),
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
            Self::SessionConfiguration {
                session_id,
                diagnostic,
            } => write!(
                f,
                "Session {session_id} configuration at {}: {} ({})",
                diagnostic.path, diagnostic.reason, diagnostic.correction
            ),
            Self::SessionCatalog(error) => write!(f, "session catalog: {error}"),
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
