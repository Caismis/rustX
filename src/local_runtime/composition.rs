//! The one Rust-side local runtime composition owner (Issue #42, Issue
//! #61).
//!
//! ```text
//! explicit startup configuration
//!         |
//!         v
//! ModelCatalog + LocalSessionConfig
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
//! > One local runtime process owns one conversation session. That session
//! > owns one authoritative mutable session-model configuration, one
//! > `ConversationToolRuntime` identity, one `CapabilityCoordinator`, one
//! > context policy domain, and one `ConversationRuntime`. Runtime
//! > Client attachments may come and go without replacing those semantic
//! > owners, and the conversation executes identically with zero
//! > attachments.
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
//! capability startup failure therefore never leaves a partially
//! initialized protocol server: composition returns an error and the process
//! exits before a single protocol byte is written.
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

use crate::capabilities::{CapabilityCoordinator, CapabilityCoordinatorConfig};
use crate::context::{DefaultTokenEstimator, TokenEstimator};
use crate::model::catalog::{
    CredentialEnvironment, ModelCatalog, ModelCatalogError, ProcessCredentialEnvironment,
};
use crate::model::invocation::{ModelBindingRegistry, ModelInvocationError};
use crate::model::session::SessionModelState;
use crate::runtime::conversation_runtime::{
    ConversationContextConfig, ConversationRuntime, ConversationRuntimeError,
    RuntimeConversationConfig,
};
use crate::runtime_client::endpoint::RuntimeClientEndpoint;
use crate::runtime_client::host::{
    HostConstructionError, RuntimeClientHost, RuntimeClientHostConfig,
};
use crate::tools::executor::ToolRegistry;
use crate::tools::native::{NativeToolResources, register_native_tools};
use crate::tools::runtime::ConversationToolRuntime;

use super::config::{LocalSessionConfig, LocalSessionConfigError};

/// The explicit startup paths of one local runtime process.
///
/// There is no discovery and no precedence: every path is given explicitly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalRuntimePaths {
    /// The model catalog (`models.json`) path.
    pub models: PathBuf,
    /// The local session configuration path.
    pub session: PathBuf,
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
        // 1. Read the explicit startup files.
        let catalog_bytes = read_file(&paths.models)?;
        let session_bytes = read_file(&paths.session)?;

        // 2. Load and validate the model catalog.
        let catalog = ModelCatalog::from_json_slice(&catalog_bytes)?;

        // 3. Resolve startup credentials and build every model binding.
        let resolved = catalog.resolve(dependencies.credentials.as_ref())?;
        let registry = ModelBindingRegistry::new(resolved)?;

        // 4. Load and validate the local session configuration.
        let session = LocalSessionConfig::from_json_slice(&session_bytes)?;

        // The session model state resolves and validates the initial
        // selection now, so an unusable model fails startup rather than the
        // first attempt.
        let model = SessionModelState::new(registry, session.model.clone())?;

        // 5-6. The conversation identity authority and the one conversation
        // tool runtime (workspace, runtime-private artifact root, canonical
        // mailbox, background registry, base authorized environment).
        let base_environment = session.tool_environment()?;
        let mut runtime_config = crate::tools::runtime::ConversationRuntimeConfig::new(
            &paths.workspace,
            paths.artifacts_root(),
        );
        runtime_config.environment = Some(base_environment.clone());
        let tool_runtime =
            ConversationToolRuntime::from_config(session.conversation_id.clone(), runtime_config)
                .map_err(|error| LocalRuntimeError::ToolRuntime {
                detail: format!("{error:?}"),
            })?;

        // 7-8. The base tool registry with the explicit native composition,
        // using *this* conversation's background registry for the
        // `background_task` intrinsic.
        let mut base_registry = ToolRegistry::new();
        register_native_tools(
            &mut base_registry,
            NativeToolResources {
                background: tool_runtime.background().clone(),
            },
            session.native_tools.to_policies(),
        )
        .map_err(|error| LocalRuntimeError::NativeTools {
            detail: format!("{error:?}"),
        })?;

        // 9. The capability coordinator over the same conversation and
        // workspace, the same base registry, and the same base environment.
        let capability = CapabilityCoordinator::new(CapabilityCoordinatorConfig {
            conversation_id: tool_runtime.conversation_id().clone(),
            workspace: tool_runtime.workspace().clone(),
            base_tool_registry: Arc::new(base_registry),
            mcp_servers: session.mcp_bindings()?,
            base_environment,
            environment_store_root: paths.environment_store_root(),
        })
        .map_err(|error| LocalRuntimeError::Capability {
            detail: format!("{error:?}"),
        })?;

        // 10-11. Prepare and commit the initial capability candidate before
        // anything can serve protocol input. This is the startup capability
        // commit: it happens *before* the conversation runtime exists, so
        // it is not subject to the runtime's lifecycle gate (Issue #61).
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

        // 12-13. The context policy/estimator/status pieces and the one
        // authoritative conversation runtime coordinator, constructed
        // **inactive**: the final composition path activates it after the
        // optional Runtime Client host binds.
        let runtime = ConversationRuntime::new(RuntimeConversationConfig {
            agent_id: session.agent_id.clone(),
            model,
            timezone: session.timezone,
            context: ConversationContextConfig {
                policy: session.context_policy(),
                estimator: Arc::clone(&dependencies.estimator),
                status_composer: crate::context::AgentStatusComposer::default(),
            },
            tool_runtime: tool_runtime.clone(),
            capability: capability.clone(),
            clock: None,
            initial_messages: Vec::new(),
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
        // 14. The Runtime Client projection/control/attachment adapter over
        // that runtime. Binding is a pre-activation composition decision
        // (Issue #61): the runtime is still inert here, so the host's
        // initial snapshot is the runtime's real state at the activation
        // cut and no bootstrap fact can fabricate a live client event.
        let host = RuntimeClientHost::new(RuntimeClientHostConfig {
            runtime: self.runtime.clone(),
            replay_limit: None,
        })?;

        // 15. Activation: the one shared Inactive -> Active lifecycle
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
    /// The local session configuration is invalid.
    Session(LocalSessionConfigError),
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
    /// The capability plane could not be prepared or committed.
    Capability {
        /// The failure detail.
        detail: String,
    },
    /// The conversation runtime could not be constructed.
    Runtime(ConversationRuntimeError),
    /// The Runtime Client host could not be constructed.
    Host(HostConstructionError),
}

impl std::fmt::Display for LocalRuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { path, detail } => write!(f, "cannot read {}: {detail}", path.display()),
            Self::Catalog(error) => write!(f, "model catalog: {error}"),
            Self::Model(error) => write!(f, "session model: {error}"),
            Self::Session(error) => write!(f, "{error}"),
            Self::ToolRuntime { detail } => write!(f, "conversation tool runtime: {detail}"),
            Self::NativeTools { detail } => write!(f, "native tool composition: {detail}"),
            Self::Capability { detail } => write!(f, "capability plane: {detail}"),
            Self::Runtime(error) => write!(f, "conversation runtime: {error}"),
            Self::Host(error) => write!(f, "runtime client host: {error}"),
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

impl From<LocalSessionConfigError> for LocalRuntimeError {
    fn from(error: LocalSessionConfigError) -> Self {
        Self::Session(error)
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
