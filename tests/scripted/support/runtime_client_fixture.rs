//! The shared Runtime Client host fixture.
//!
//! Every Runtime Client integration test needs the same construction: a
//! scripted model, a tool registry, a committed capability coordinator over
//! a temporary workspace, a context engine, the conversation runtime
//! coordinator, and the Runtime Client host adapter. That construction was
//! duplicated per test file; it lives here once so the Issue #37 semantic
//! tests and the Issue #38 transport-independent conformance scenarios
//! build identical runtimes.
//!
//! This module is fixture construction only. It knows nothing about
//! transports, drivers, or scenarios.

use std::path::Path;
use std::sync::Arc;

use super::model::scripted_session_model;
use rustx::context::{
    AgentStatusEngine, DefaultTokenEstimator, SessionContextPolicy, TokenEstimator,
};
use rustx::message::types::MessageBlock;
use rustx::model::session::SessionModelState;
use rustx::runtime::conversation_runtime::{
    ConversationContextConfig, ConversationRuntime, RuntimeConversationConfig,
};
use rustx::runtime::identity::{AgentId, ConversationId};
use rustx::runtime_client::{RuntimeClientHost, RuntimeClientHostConfig};
use rustx::tools::executor::ToolRegistry;

use super::fake::{FakeModel, FakeStep};

/// One workspace-content writer applied before capability preparation.
type WorkspaceFixture = Box<dyn FnOnce(&Path)>;

/// One Runtime Client runtime under test, plus the deterministic handles a
/// test drives it with.
pub struct RuntimeClientFixture {
    /// The runtime under test (the Runtime Client host adapter).
    pub host: RuntimeClientHost,
    /// The conversation runtime coordinator under the adapter.
    pub runtime: ConversationRuntime,
    /// The scripted model driving attempts.
    pub model: Arc<FakeModel>,
    /// The workspace backing the tool runtime and capability coordinator.
    ///
    /// Retained so the temporary directory outlives the runtime rather than
    /// being leaked.
    workspace: tempfile::TempDir,
}

impl RuntimeClientFixture {
    /// Starts building a fixture for one conversation.
    #[must_use]
    pub fn builder(conversation: &str) -> RuntimeClientFixtureBuilder {
        RuntimeClientFixtureBuilder {
            conversation: conversation.to_owned(),
            scripts: Vec::new(),
            model: None,
            base_tools: ToolRegistry::new(),
            replay_limit: None,
            status_engine: AgentStatusEngine::default(),
            initial_messages: Vec::new(),
            durable_history: Vec::new(),
            workspace_fixtures: Vec::new(),
            mcp_servers: std::collections::BTreeMap::new(),
            native_tools: false,
            tool_activation: rustx::capabilities::ToolActivationPolicy::default(),
            session_model: None,
            context_policy: SessionContextPolicy {
                reserve_tokens: 0,
                keep_recent_tokens: 0,
                summary_output_cap: None,
            },
        }
    }

    /// The workspace root of the runtime under test.
    #[must_use]
    pub fn workspace_root(&self) -> std::path::PathBuf {
        self.workspace.path().join("workspace")
    }

    /// Splits the fixture into the model handle and the host, keeping the
    /// workspace alive for the rest of the process.
    ///
    /// The host outlives this handle in tests that hold only the host, and
    /// removing the workspace under a live runtime would break it. Leaking
    /// one temporary directory per test process is exactly the trade-off
    /// the per-file fixtures made before they were extracted here.
    #[must_use]
    pub fn into_parts(self) -> (Arc<FakeModel>, RuntimeClientHost) {
        std::mem::forget(self.workspace);
        (self.model, self.host)
    }
}

/// The builder of [`RuntimeClientFixture`].
pub struct RuntimeClientFixtureBuilder {
    /// The conversation identity.
    conversation: String,
    /// One scripted model invocation per attempt turn.
    scripts: Vec<Vec<FakeStep>>,
    /// An already-built scripted model, when the test supplied one.
    model: Option<FakeModel>,
    /// The base tool registry handed to the capability coordinator.
    base_tools: ToolRegistry,
    /// The bounded replay retention, when a test needs a small ring.
    replay_limit: Option<usize>,
    /// The launch-scoped Agent Status engine template.
    status_engine: AgentStatusEngine,
    /// Pre-existing canonical history.
    initial_messages: Vec<MessageBlock>,
    durable_history: Vec<MessageBlock>,
    /// Workspace content written before capability preparation.
    workspace_fixtures: Vec<WorkspaceFixture>,
    /// MCP servers the capability coordinator connects.
    mcp_servers: rustx::tools::mcp::McpServerBindings,
    /// Whether to register the real native tool plane in the base registry.
    native_tools: bool,
    /// The startup activation policy applied to the available tool set.
    tool_activation: rustx::capabilities::ToolActivationPolicy,
    /// An explicit session model authority, when the test needs a specific
    /// catalog (several models, reasoning profiles, or an explicit summary
    /// model). Defaults to the one scripted model.
    session_model: Option<SessionModelState>,
    /// The static session context policy.
    context_policy: SessionContextPolicy,
}

impl RuntimeClientFixtureBuilder {
    /// Appends one scripted model invocation.
    #[must_use]
    pub fn script(mut self, steps: Vec<FakeStep>) -> Self {
        self.scripts.push(steps);
        self
    }

    /// Uses an already-built scripted model instead of `script`/`scripts`.
    ///
    /// Tests that subscribe to a model's observation channels before the
    /// host exists need the model itself, not only its script.
    #[must_use]
    pub fn model(mut self, model: FakeModel) -> Self {
        self.model = Some(model);
        self
    }

    /// Appends every scripted invocation of a model script list.
    #[must_use]
    pub fn scripts(mut self, scripts: Vec<Vec<FakeStep>>) -> Self {
        self.scripts.extend(scripts);
        self
    }

    /// Replaces the base tool registry.
    #[must_use]
    pub fn tools(mut self, tools: ToolRegistry) -> Self {
        self.base_tools = tools;
        self
    }

    /// Sets the bounded replay retention.
    #[must_use]
    pub fn replay_limit(mut self, limit: Option<usize>) -> Self {
        self.replay_limit = limit;
        self
    }

    /// Replaces the Agent Status engine template.
    #[must_use]
    pub fn status_engine(mut self, status_engine: AgentStatusEngine) -> Self {
        self.status_engine = status_engine;
        self
    }

    /// Seeds pre-existing canonical history.
    #[must_use]
    /// Seeds the conversation's durable lineage *before* the tool runtime is
    /// composed, exactly as reopening an existing conversation does.
    ///
    /// `initial_messages` alone is not the same thing: the tool runtime — and
    /// with it the conversation's task list — is constructed over the store
    /// before the runtime writes that bootstrap history, so a list rebuilt
    /// from history needs the history to already be there.
    pub fn durable_history(mut self, messages: Vec<MessageBlock>) -> Self {
        self.durable_history = messages.clone();
        self.initial_messages = messages;
        self
    }

    pub fn initial_messages(mut self, messages: Vec<MessageBlock>) -> Self {
        self.initial_messages = messages;
        self
    }

    /// Writes workspace content (Skills, Python tool packages) before the
    /// capability coordinator prepares its first revision.
    #[must_use]
    pub fn workspace_fixture(mut self, write: impl FnOnce(&Path) + 'static) -> Self {
        self.workspace_fixtures.push(Box::new(write));
        self
    }

    /// Connects MCP servers to the capability coordinator.
    #[must_use]
    pub fn mcp_servers(mut self, servers: rustx::tools::mcp::McpServerBindings) -> Self {
        self.mcp_servers = servers;
        self
    }

    /// Registers the real native tool plane before capability preparation.
    #[must_use]
    pub const fn native_tools(mut self) -> Self {
        self.native_tools = true;
        self
    }

    /// Replaces the startup tool activation policy.
    #[must_use]
    pub fn tool_activation(mut self, policy: rustx::capabilities::ToolActivationPolicy) -> Self {
        self.tool_activation = policy;
        self
    }

    /// Replaces the session model authority with an explicit one.
    #[must_use]
    pub fn session_model(mut self, model: SessionModelState) -> Self {
        self.session_model = Some(model);
        self
    }

    /// Replaces the static session context policy.
    #[must_use]
    pub const fn context_policy(mut self, policy: SessionContextPolicy) -> Self {
        self.context_policy = policy;
        self
    }

    /// Builds the runtime.
    ///
    /// # Panics
    ///
    /// Panics if the deterministic fixture construction fails, which always
    /// means the fixture itself is wrong.
    pub async fn build(self) -> RuntimeClientFixture {
        let workspace = tempfile::tempdir().expect("fixture workspace");
        let workspace_root = workspace.path().join("workspace");
        std::fs::create_dir_all(&workspace_root).expect("workspace root");
        for write in self.workspace_fixtures {
            write(&workspace_root);
        }
        if !self.durable_history.is_empty() {
            let artifacts = workspace.path().join("artifacts");
            std::fs::create_dir_all(&artifacts).expect("artifact root");
            let store = rustx::durable::SqliteConversationStore::open(
                ConversationId::new(&self.conversation),
                &artifacts.join("conversation.sqlite"),
            )
            .expect("durable lineage");
            rustx::durable::ConversationStore::initialize(&store, &self.durable_history)
                .expect("seed the durable lineage");
        }
        let tool_runtime = rustx::tools::runtime::ConversationToolRuntime::new(
            ConversationId::new(&self.conversation),
            &workspace_root,
            workspace.path().join("artifacts"),
        )
        .expect("tool runtime");
        let mut base_tools = self.base_tools;
        if self.native_tools {
            rustx::tools::register_native_tools(
                &mut base_tools,
                rustx::tools::NativeToolResources {
                    subagent_catalog: rustx::runtime::subagent::SubagentCatalog::empty(),
                    background: tool_runtime.background().clone(),
                    subagents: None,
                },
                rustx::tools::NativeToolPolicies::default(),
            )
            .expect("register native tools");
        }

        let coordinator = rustx::capabilities::CapabilityCoordinator::with_backend(
            rustx::capabilities::CapabilityCoordinatorConfig {
                conversation_id: tool_runtime.conversation_id().clone(),
                workspace: tool_runtime.workspace().clone(),
                base_tool_registry: Arc::new(base_tools),
                tool_activation: self.tool_activation,
                skill_discovery: rustx::skills::SkillDiscoveryConfig {
                    automatic_roots: vec![
                        tool_runtime.workspace().root().join(".rustx/skills"),
                        tool_runtime.workspace().root().join(".agents/skills"),
                    ],
                    explicit_paths: Vec::new(),
                },
                mcp_servers: self.mcp_servers,
                base_environment: tool_runtime.environment().clone(),
                environment_store_root: workspace.path().join("skill-env"),
            },
            Arc::new(crate::scripted_suites::common::FakeSkillEnvironmentBackend::new()),
        )
        .expect("capability coordinator");
        let candidate = coordinator.prepare_candidate().await.expect("prepare");
        coordinator.commit(candidate).expect("commit");

        assert!(
            self.model.is_none() || self.scripts.is_empty(),
            "a fixture is scripted either by `script`/`scripts` or by `model`, never both"
        );
        let model = Arc::new(self.model.unwrap_or_else(|| FakeModel::new(self.scripts)));
        let adapter: Arc<dyn rustx::model::ModelAdapter> = model.clone();
        let estimator: Arc<dyn TokenEstimator> = Arc::new(DefaultTokenEstimator);
        let session_model = self
            .session_model
            .unwrap_or_else(|| scripted_session_model(adapter));
        // The conversation runtime coordinator owns the semantic state; the
        // Runtime Client host is the projection/control adapter over it.
        let runtime = ConversationRuntime::new(RuntimeConversationConfig {
            agent_id: AgentId::new("agent-a"),
            model: session_model,
            approval_mode: rustx::runtime::ApprovalMode::Policy,
            model_timeout_policy: rustx::model::ModelTimeoutPolicy::default(),
            context: ConversationContextConfig {
                policy: self.context_policy,
                estimator,
                status_engine: self.status_engine,
            },
            tool_runtime,
            resources: Arc::new(rustx::runtime::RuntimeResourceSnapshot::new(
                rustx::runtime::RuntimeResourceRevision::new(1),
                Vec::new(),
                None,
                rustx::context::ContextAssembly::new(),
                coordinator.current_snapshot(),
            )),
            resource_loader: Arc::new(rustx::runtime::FilesystemRuntimeResourceLoader::new(
                coordinator.current_snapshot().workspace_root(),
            )),
            capability: coordinator,
            clock: None,
            initial_messages: self.initial_messages,
            subagents: None,
        })
        .expect("conversation runtime");
        let host = RuntimeClientHost::new(RuntimeClientHostConfig {
            runtime: runtime.clone(),
            replay_limit: self.replay_limit,
        })
        .expect("runtime client host");
        // The explicit Issue #61 lifecycle boundary: the host bound over
        // the inert runtime, so semantic execution may begin now.
        runtime.activate();
        RuntimeClientFixture {
            host,
            runtime,
            model,
            workspace,
        }
    }
}

/// Writes one valid Skill package into a workspace.
pub fn write_skill(workspace: &Path, name: &str, description: &str) {
    let root = workspace.join(".agents").join("skills").join(name);
    std::fs::create_dir_all(&root).expect("skill directory");
    std::fs::write(
        root.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: \"{description}\"\n---\nbody\n"),
    )
    .expect("SKILL.md");
}

/// The host `SKILL.md` location the capability projection publishes for a
/// Skill written by [`write_skill`] into `workspace`.
pub fn skill_location(workspace: &Path, name: &str) -> String {
    workspace
        .join(".agents")
        .join("skills")
        .join(name)
        .join("SKILL.md")
        .to_string_lossy()
        .into_owned()
}

/// Writes one valid Python tool package into a workspace, generating a real
/// `uv.lock` when `uv` is available (a missing `uv` skips the environment
/// step, mirroring the `m7_uv` acceptance pattern).
pub fn write_python_package(workspace: &Path, name: &str, description: &str) {
    let package = workspace.join(".agents/tools").join(name);
    std::fs::create_dir_all(&package).expect("package directory");
    std::fs::write(
        package.join("TOOL.toml"),
        format!(
            "schema_version = 1\nname = {name:?}\ndescription = {description:?}\nentrypoint = \"tool:main\"\nexecution = \"foreground_only\"\nconcurrency = \"sequential\"\n"
        ),
    )
    .expect("manifest");
    std::fs::write(
        package.join("input.schema.json"),
        r#"{"type":"object","properties":{},"additionalProperties":false}"#,
    )
    .expect("schema");
    std::fs::write(
        package.join("pyproject.toml"),
        "[project]\nname = \"fixture\"\nversion = \"0.1.0\"\nrequires-python = \">=3.11\"\n",
    )
    .expect("project");
    std::fs::write(
        package.join("tool.py"),
        "def main(arguments):\n    return arguments\n",
    )
    .expect("source");
    if let Some(uv) = uv_path() {
        let lock = std::process::Command::new(&uv)
            .args(["lock", "--offline", "--no-config"])
            .current_dir(&package)
            .env_clear()
            .env("PATH", "/usr/local/bin:/usr/bin:/bin")
            .env("HOME", workspace.parent().expect("fixture root"))
            .env("UV_NO_PYTHON_DOWNLOADS", "1")
            .output()
            .expect("run fixture uv lock");
        assert!(
            lock.status.success(),
            "fixture lock failed: {}",
            String::from_utf8_lossy(&lock.stderr)
        );
    }
}

/// The `uv` executable on `PATH`, when present.
#[must_use]
pub fn uv_path() -> Option<std::path::PathBuf> {
    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|dir| dir.join("uv"))
            .find(|path| path.is_file())
    })
}

/// Whether `uv` is available for Python capability fixtures.
#[must_use]
pub fn uv_available() -> bool {
    uv_path().is_some()
}
