//! The one Rust-side local runtime composition owner (Issue #42).
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
//!         +--> context/checkpoint/status pieces
//!         |
//!         v
//! RuntimeClientHost
//!         |
//!         v
//! RuntimeClientEndpoint
//!         |
//!         v
//! stdio / JSONL  (Issue #38)
//! ```
//!
//! The governing invariant:
//!
//! > One local runtime process owns one conversation session. That session
//! > owns one authoritative mutable session-model configuration, one
//! > `ConversationToolRuntime` identity, one `CapabilityCoordinator`, one
//! > context policy/checkpoint domain, and one `RuntimeClientHost`. Runtime
//! > Client attachments may come and go without replacing those semantic
//! > owners.
//!
//! A client — including the Issue #39 TUI — owns the child process
//! lifecycle and nothing else. It never assembles provider adapters, model
//! parameters, context engines, tool registries, capability coordinators, or
//! summary models.
//!
//! # Ordering
//!
//! Composition follows a fixed order, and the initial capability candidate
//! is **prepared and committed before any protocol input is served**. A
//! capability startup failure therefore never leaves a partially
//! initialized protocol server: composition returns an error and the process
//! exits before a single protocol byte is written.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::capabilities::{CapabilityCoordinator, CapabilityCoordinatorConfig};
use crate::context::{
    ContextCheckpointStore, DefaultTokenEstimator, InMemoryCheckpointStore, TokenEstimator,
};
use crate::model::catalog::{
    CredentialEnvironment, ModelCatalog, ModelCatalogError, ProcessCredentialEnvironment,
};
use crate::model::invocation::{
    DefaultProviderAdapterFactory, ModelBindingRegistry, ModelInvocationError,
    ProviderAdapterFactory,
};
use crate::model::session::SessionModelState;
use crate::runtime_client::endpoint::RuntimeClientEndpoint;
use crate::runtime_client::host::{
    HostConstructionError, RuntimeClientContextConfig, RuntimeClientHost, RuntimeClientHostConfig,
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

/// The optional injected dependencies of composition.
///
/// Production uses the defaults. Deterministic tests inject a provider
/// adapter factory pointing at a local fixture server, an explicit
/// credential environment, or a token estimator, without introducing a
/// second production configuration mode.
pub struct LocalRuntimeDependencies {
    /// The provider adapter factory.
    pub adapter_factory: Arc<dyn ProviderAdapterFactory>,
    /// The credential environment used to resolve `$ENV_VAR` sources.
    pub credentials: Arc<dyn CredentialEnvironment>,
    /// The deterministic token estimator.
    pub estimator: Arc<dyn TokenEstimator>,
    /// The context checkpoint store.
    pub checkpoint_store: Arc<dyn ContextCheckpointStore>,
}

impl Default for LocalRuntimeDependencies {
    fn default() -> Self {
        Self {
            adapter_factory: Arc::new(DefaultProviderAdapterFactory),
            credentials: Arc::new(ProcessCredentialEnvironment),
            estimator: Arc::new(DefaultTokenEstimator),
            checkpoint_store: Arc::new(InMemoryCheckpointStore::new()),
        }
    }
}

impl std::fmt::Debug for LocalRuntimeDependencies {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalRuntimeDependencies")
            .finish_non_exhaustive()
    }
}

/// The composed local conversation runtime.
///
/// It owns the semantic owners of the process: exactly one
/// `ConversationToolRuntime`, one `CapabilityCoordinator`, and one
/// `RuntimeClientHost`. The endpoint handed to a transport is derived from
/// that one host.
pub struct LocalConversationRuntime {
    host: RuntimeClientHost,
    tool_runtime: ConversationToolRuntime,
    capability: CapabilityCoordinator,
}

impl std::fmt::Debug for LocalConversationRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalConversationRuntime")
            .field("conversation_id", self.tool_runtime.conversation_id())
            .finish_non_exhaustive()
    }
}

impl LocalConversationRuntime {
    /// Composes the runtime from explicit startup paths.
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
        let registry =
            ModelBindingRegistry::new(resolved, dependencies.adapter_factory.as_ref())?;

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
            mcp_servers: session.mcp_server_configs(),
            base_environment,
            environment_store_root: paths.environment_store_root(),
        })
        .map_err(|error| LocalRuntimeError::Capability {
            detail: format!("{error:?}"),
        })?;

        // 10-11. Prepare and commit the initial capability candidate before
        // anything can serve protocol input.
        let candidate = capability
            .prepare_candidate()
            .await
            .map_err(|error| LocalRuntimeError::Capability {
                detail: format!("{error:?}"),
            })?;
        capability
            .commit(candidate)
            .map_err(|error| LocalRuntimeError::Capability {
                detail: format!("{error:?}"),
            })?;

        // 12-13. The context policy/checkpoint/status pieces and the one
        // authoritative Runtime Client host.
        let host = RuntimeClientHost::new(RuntimeClientHostConfig {
            agent_id: session.agent_id.clone(),
            model,
            timezone: session.timezone,
            context: RuntimeClientContextConfig {
                policy: session.context_policy(),
                estimator: Arc::clone(&dependencies.estimator),
                checkpoint_store: Arc::clone(&dependencies.checkpoint_store),
                status_composer: crate::context::AgentStatusComposer::default(),
            },
            tool_runtime: tool_runtime.clone(),
            capability: capability.clone(),
            clock: None,
            initial_messages: Vec::new(),
            replay_limit: None,
        })?;

        Ok(Self {
            host,
            tool_runtime,
            capability,
        })
    }

    /// The one Runtime Client host of this process.
    #[must_use]
    pub const fn host(&self) -> &RuntimeClientHost {
        &self.host
    }

    /// The one conversation tool runtime of this process.
    #[must_use]
    pub const fn tool_runtime(&self) -> &ConversationToolRuntime {
        &self.tool_runtime
    }

    /// The one capability coordinator of this process.
    #[must_use]
    pub const fn capability(&self) -> &CapabilityCoordinator {
        &self.capability
    }

    /// 14. Creates the Runtime Client endpoint a transport wraps.
    #[must_use]
    pub fn endpoint(&self) -> RuntimeClientEndpoint {
        RuntimeClientEndpoint::new(self.host.clone())
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

impl From<HostConstructionError> for LocalRuntimeError {
    fn from(error: HostConstructionError) -> Self {
        Self::Host(error)
    }
}
