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
//!         +-- into_headless(): activate with no Runtime Client host
//!                 -> HeadlessConversationRuntime (Issue #60 subagents)
//! ```
//!
//! The semantic composition — the model catalog/session/tool/capability/
//! context assembly — exists exactly once in
//! [`LocalConversationCore::compose`]. The interactive and headless
//! production runtimes are the two final paths over that same core:
//!
//! ```text
//! compose semantic inactive core
//!     |
//!     +-- interactive: bind RuntimeClientHost, activate, return
//!     +-- headless:    activate, return
//! ```
//!
//! Activation is the one explicit lifecycle boundary in both paths
//! (`ConversationRuntime::activate`), and in both paths the returned
//! handles are already active.
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
//! Failures of **optional external capability sources** — the custom
//! Python tool plane and each configured MCP server independently — are
//! isolated by the capability plane itself (`prepare_candidate` records
//! them as typed [`CapabilitySourceState::Unavailable`](crate::capabilities::CapabilitySourceState)
//! state instead of failing the candidate). The runtime therefore starts
//! with, e.g., native tools ready, Python unavailable, one MCP server
//! unavailable and another ready; the base/native capability set is never
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

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::capabilities::{
    CapabilityCoordinator, CapabilityCoordinatorConfig, ToolActivationPolicy,
};
use crate::context::{DefaultTokenEstimator, TokenEstimator};
use crate::message::content::TextBlock;
use crate::message::types::{MessageBlock, SystemAuthority, SystemMessageBlock};
use crate::model::catalog::{
    CredentialEnvironment, ModelCatalog, ModelCatalogError, ProcessCredentialEnvironment,
};
use crate::model::invocation::{ModelBindingRegistry, ModelInvocationError};
use crate::model::session::SessionModelState;
use crate::runtime::conversation_runtime::{
    ConversationContextConfig, ConversationRuntime, ConversationRuntimeError,
    RuntimeConversationConfig,
};
use crate::runtime::identity::ConversationId;
use crate::runtime_client::endpoint::RuntimeClientEndpoint;
use crate::runtime_client::host::{
    HostConstructionError, RuntimeClientHost, RuntimeClientHostConfig, RuntimeClientSessionControl,
};
use crate::skills::SkillDiscoveryConfig;
use crate::tools::environment::ToolEnvironment;
use crate::tools::executor::ToolRegistry;
use crate::tools::native::{NativeToolResources, register_native_tools};
use crate::tools::runtime::ConversationToolRuntime;

use super::config::{CurrentRuntimeConfig, CurrentRuntimeConfigError};
use super::session::{SessionCatalog, SessionError, SessionPersistentState};
use super::supervisor::{LocalSessionSupervisor, SessionSupervisorError};

/// The explicit startup paths of one local runtime process.
///
/// There is no discovery and no precedence: every path is given explicitly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalRuntimePaths {
    /// The model catalog (`models.json`) path.
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
    /// Strict startup Tool allowlist, when supplied.
    pub tools: Option<Vec<String>>,
    /// Final startup Tool exclusions.
    pub exclude_tools: Vec<String>,
    /// The model-visible workspace root.
    pub workspace: PathBuf,
    /// The runtime-private root from which disjoint private subdirectories
    /// are derived.
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
}

impl Default for LocalRuntimeDependencies {
    fn default() -> Self {
        Self {
            credentials: Arc::new(ProcessCredentialEnvironment),
            estimator: Arc::new(DefaultTokenEstimator),
        }
    }
}

impl std::fmt::Debug for LocalRuntimeDependencies {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalRuntimeDependencies")
            .finish_non_exhaustive()
    }
}

/// The shared semantic composition of one local runtime (Issue #61).
///
/// This is the single assembly point of the model catalog/session/tool/
/// capability/context pieces. It owns exactly the semantic owners of the
/// process — one `ConversationToolRuntime`, one `CapabilityCoordinator`,
/// and one `ConversationRuntime` — and nothing protocol-shaped. The
/// conversation runtime is constructed **inactive**; the two final paths
/// over this core are [`LocalConversationRuntime::compose`] (interactive,
/// binds a Runtime Client host before activating) and
/// [`HeadlessConversationRuntime::compose`] (headless, activates without
/// any Runtime Client host).
pub struct LocalConversationCore {
    runtime: ConversationRuntime,
    tool_runtime: ConversationToolRuntime,
    capability: CapabilityCoordinator,
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
    /// [`LocalConversationCore::into_interactive`] or
    /// [`LocalConversationCore::into_headless`] (or, for low-level
    /// composition callers, activate the runtime explicitly). Prefer the
    /// two final composition paths of
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
        let runtime_config = CurrentRuntimeConfig::from_json_slice(&config_bytes)?;
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
        let model = SessionModelState::new(registry, session_state.model.clone())?;

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
        // `background_task` intrinsic and this conversation's subagent
        // registry for the `subagent` intrinsic (Issue #60).
        let subagents = crate::runtime::subagent::SubagentRegistry::new(
            crate::runtime::subagent::SubagentRegistryConfig {
                conversation_id: tool_runtime.conversation_id().clone(),
                agent_id: runtime_config.agent_id.clone(),
                mailbox: tool_runtime.mailbox(),
                clock: Arc::new(crate::runtime::types::SystemClock),
                spawn: crate::runtime::subagent::SubagentSpawnPlan {
                    program: std::env::current_exe().map_err(|error| LocalRuntimeError::Io {
                        path: PathBuf::from("<current exe>"),
                        detail: error.to_string(),
                    })?,
                    models: paths.models.clone(),
                    workspace: paths.workspace.clone(),
                    runtime_root: artifacts_root.clone(),
                    model: session_state.model.clone(),
                    timezone: runtime_config.timezone,
                    context: runtime_config.context_policy(),
                },
                max_active: 4,
            },
        );
        let mut base_registry = ToolRegistry::new();
        register_native_tools(
            &mut base_registry,
            NativeToolResources {
                background: tool_runtime.background().clone(),
                subagents: Some(subagents.clone()),
            },
            runtime_config.native_tools.to_policies(),
        )
        .map_err(|error| LocalRuntimeError::NativeTools {
            detail: format!("{error:?}"),
        })?;

        let mut skill_discovery =
            SkillDiscoveryConfig::default_for_workspace(tool_runtime.workspace());
        if paths.no_skills {
            skill_discovery.automatic_roots.clear();
        }
        skill_discovery.explicit_paths.extend(
            runtime_config
                .skills
                .iter()
                .map(|path| resolve_workspace_path(&paths.workspace, path)),
        );
        skill_discovery.explicit_paths.extend(
            paths
                .skill_paths
                .iter()
                .map(|path| resolve_workspace_path(&paths.workspace, path)),
        );

        // 9. Composition owns CLI/config precedence resolution. The
        // coordinator receives only this already-resolved activation policy
        // and applies it to the available capability registrations.
        let capability = CapabilityCoordinator::new(CapabilityCoordinatorConfig {
            conversation_id: tool_runtime.conversation_id().clone(),
            workspace: tool_runtime.workspace().clone(),
            base_tool_registry: Arc::new(base_registry),
            tool_activation: ToolActivationPolicy {
                default_tools: Some(runtime_config.default_tools.clone()),
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

        // 10-11. Prepare and commit the initial capability candidate before
        // anything can serve protocol input. This is the startup capability
        // commit: it happens *before* the conversation runtime exists, so
        // it is not subject to the runtime's lifecycle gate (Issue #61).
        // Optional-source failures (Python tools, any one MCP server) are
        // already isolated into typed availability state inside
        // `prepare_candidate` (Issue #81); an error here is a *base*
        // capability-plane failure and stays fatal.
        let candidate = capability.prepare_candidate().await.map_err(|error| {
            LocalRuntimeError::Capability {
                detail: format!("{error:?}"),
            }
        })?;
        capability
            .commit(candidate)
            .map_err(|error| LocalRuntimeError::Capability {
                detail: format!("{error:?}"),
            })?;

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
            timezone: runtime_config.timezone,
            approval_mode: runtime_config.approval_mode,
            context: ConversationContextConfig {
                policy: runtime_config.context_policy(),
                estimator: Arc::clone(&dependencies.estimator),
                status_composer: crate::context::AgentStatusComposer::default(),
            },
            tool_runtime: tool_runtime.clone(),
            capability: capability.clone(),
            clock: None,
            initial_messages,
            subagents: Some(subagents),
        })?;

        Ok(Self {
            runtime,
            tool_runtime,
            capability,
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
    ///   current runtime configuration file;
    /// - the base tool registry is exactly the profile's read-only set
    ///   (`Read`/`Glob`/`Grep`), registered through
    ///   [`register_subagent_child_tools`];
    /// - the capability plane is **base-only**: no Skill discovery, no
    ///   Python/Node environments, no MCP servers, and no `subagent` tool
    ///   (recursive delegation is structurally absent); it never opens or
    ///   creates Python tool storage (Issue #81), so a broken Python store
    ///   location cannot fail child composition;
    /// - the profile persona enters the canonical history as one bootstrap
    ///   `SystemMessageBlock` (authority `Platform`), never as a forged
    ///   user message;
    /// - the durable authority is the child-private store under
    ///   [`SubagentChildSpec::runtime_root`], disjoint from the parent's
    ///   store.
    ///
    /// The caller (the child driver) still owns activation: the returned
    /// core is inert until `into_headless`.
    ///
    /// # Errors
    ///
    /// Returns the first composition failure, exactly like the ordinary
    /// composition path.
    pub(crate) fn compose_subagent_child(
        spec: &crate::runtime::subagent::ipc::SubagentChildSpec,
        dependencies: &LocalRuntimeDependencies,
    ) -> Result<Self, LocalRuntimeError> {
        // 1-3. The model catalog/binding plane, identical to the ordinary
        // composition: the catalog file path is inherited from the parent.
        let catalog_bytes = read_file(&spec.models)?;
        let catalog = ModelCatalog::from_json_slice(&catalog_bytes)?;
        let resolved = catalog.resolve(dependencies.credentials.as_ref())?;
        let registry = ModelBindingRegistry::new(resolved)?;

        // 4. The child's session model state, from the typed spec.
        let model = SessionModelState::new(registry, spec.model.clone())?;

        // 5-6. The child conversation tool runtime over the shared
        // read-only workspace and the child-private runtime root. The
        // child authorizes no environment entries.
        let base_environment = ToolEnvironment::from_authorized(std::iter::empty())
            .map_err(CurrentRuntimeConfigError::Environment)
            .map_err(LocalRuntimeError::RuntimeConfig)?;
        let mut runtime_config = crate::tools::runtime::ConversationRuntimeConfig::new(
            &spec.workspace,
            spec.runtime_root.join("artifacts"),
        );
        runtime_config.environment = Some(base_environment.clone());
        let tool_runtime = ConversationToolRuntime::from_config(
            spec.child_conversation_id.clone(),
            runtime_config,
        )
        .map_err(|error| LocalRuntimeError::ToolRuntime {
            detail: format!("{error:?}"),
        })?;

        // 7-8. The deny-by-construction read-only base registry.
        let mut base_registry = ToolRegistry::new();
        crate::tools::native::register_subagent_child_tools(
            &mut base_registry,
            crate::tools::native::NativeToolPolicies::default(),
        )
        .map_err(|error| LocalRuntimeError::NativeTools {
            detail: format!("{error:?}"),
        })?;

        // 9-11. The base-only capability plane: the exact frozen registry
        // and nothing else.
        let capability = CapabilityCoordinator::new(CapabilityCoordinatorConfig {
            conversation_id: tool_runtime.conversation_id().clone(),
            workspace: tool_runtime.workspace().clone(),
            base_tool_registry: Arc::new(base_registry),
            tool_activation: ToolActivationPolicy::default(),
            skill_discovery: SkillDiscoveryConfig::default(),
            mcp_servers: crate::tools::mcp::McpServerBindings::default(),
            base_environment,
            environment_store_root: spec.runtime_root.join("environments"),
        })
        .map_err(|error| LocalRuntimeError::Capability {
            detail: format!("{error:?}"),
        })?;
        let candidate = capability.prepare_base_only_candidate().map_err(|error| {
            LocalRuntimeError::Capability {
                detail: format!("{error:?}"),
            }
        })?;
        capability
            .commit(candidate)
            .map_err(|error| LocalRuntimeError::Capability {
                detail: format!("{error:?}"),
            })?;

        // 12-13. The one child conversation runtime. The persona enters the
        // canonical history as one platform-authority bootstrap system
        // message — instructions, never a fabricated user turn.
        let runtime = ConversationRuntime::new(RuntimeConversationConfig {
            agent_id: spec.child_agent_id.clone(),
            model,
            timezone: spec.timezone,
            approval_mode: crate::runtime::ApprovalMode::Policy,
            context: ConversationContextConfig {
                policy: spec.context,
                estimator: Arc::clone(&dependencies.estimator),
                status_composer: crate::context::AgentStatusComposer::default(),
            },
            tool_runtime: tool_runtime.clone(),
            capability: capability.clone(),
            clock: None,
            initial_messages: vec![MessageBlock::System(SystemMessageBlock {
                id: crate::runtime::identity::MessageId::new(format!(
                    "{}-bootstrap-persona",
                    spec.child_conversation_id
                )),
                authority: SystemAuthority::Platform,
                content: vec![TextBlock {
                    text: spec.persona.clone(),
                }],
            })],
            // A child runtime has no subagent registry: recursive
            // delegation is absent by construction.
            subagents: None,
        })?;

        Ok(Self {
            runtime,
            tool_runtime,
            capability,
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

    fn into_interactive_with_control(
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

        // 15. Activation: the one shared Inactive -> Running lifecycle
        // transition. The client host-binding decision is now frozen, the
        // admission worker starts, and semantic execution may begin.
        self.runtime.activate();

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
    /// Loads the native catalog, resolves its active node, composes that
    /// `ConversationRuntime`, binds typed Session control, and activates the
    /// runtime before serving protocol input.
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
        let runtime_config = CurrentRuntimeConfig::from_json_slice(&config_bytes)?;
        let registry = load_model_registry(paths, dependencies)?;
        SessionModelState::new(registry.clone(), runtime_config.model.clone())?;
        let catalog = if let Some(catalog) = SessionCatalog::open_existing(&paths.runtime_root)? {
            catalog
        } else {
            let state = SessionPersistentState {
                model: runtime_config.model.clone(),
            };
            SessionCatalog::create(&paths.runtime_root, &state)?
        };
        let (session_id, node, session_state) = catalog
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

        let supervisor = Arc::new(LocalSessionSupervisor::new(
            catalog,
            runtime_config.model.clone(),
        ));
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
        let runtime = core.into_interactive_with_session_control(supervisor.clone())?;
        supervisor
            .install_runtime(runtime.runtime().clone())
            .await
            .map_err(LocalRuntimeError::SessionSupervisor)?;
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
    let catalog = ModelCatalog::from_json_slice(&catalog_bytes)?;
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
    /// This never carries an optional-source failure: the custom Python
    /// tool plane and each configured MCP server fail into typed
    /// availability state instead (Issue #81).
    Capability {
        /// The failure detail.
        detail: String,
    },
    /// The conversation runtime could not be constructed.
    Runtime(ConversationRuntimeError),
    /// The Runtime Client host could not be constructed.
    Host(HostConstructionError),
    /// The native SessionCatalog/Graph could not be loaded or published.
    SessionCatalog(SessionError),
    /// The native Session supervisor could not install or drain a lineage.
    SessionSupervisor(SessionSupervisorError),
}

impl std::fmt::Display for LocalRuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { path, detail } => write!(f, "cannot read {}: {detail}", path.display()),
            Self::Catalog(error) => write!(f, "model catalog: {error}"),
            Self::Model(error) => write!(f, "session model: {error}"),
            Self::RuntimeConfig(error) => write!(f, "{error}"),
            Self::ToolRuntime { detail } => write!(f, "conversation tool runtime: {detail}"),
            Self::NativeTools { detail } => write!(f, "native tool composition: {detail}"),
            Self::Capability { detail } => write!(f, "capability plane: {detail}"),
            Self::Runtime(error) => write!(f, "conversation runtime: {error}"),
            Self::Host(error) => write!(f, "runtime client host: {error}"),
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
