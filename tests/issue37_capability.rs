//! Issue #37: capability/tool/Skill inspection of Runtime Client Protocol
//! v1.
//!
//! The active capability projection must carry the revision, the
//! deterministic tool catalog with origin metadata for native, MCP, and
//! Python tools, and the deterministic Skill catalog (identity, version,
//! name, description) — without executors, environment paths, or private
//! dependency internals on the wire.

#[path = "common/mod.rs"]
mod common;

use std::path::Path;
use std::sync::Arc;

use rustx::runtime::identity::ToolId;
use rustx::runtime_client::{
    RuntimeClientContextConfig, RuntimeClientHost, RuntimeClientHostConfig, RuntimeClientRequest,
    RuntimeClientResult,
};
use rustx::tools::types::{
    ToolConcurrencyPolicy, ToolDefinition, ToolExecutionPolicy, ToolOrigin, ToolReplayPolicy,
};

/// Writes one valid Skill package into the workspace.
fn write_skill(workspace: &Path, name: &str, description: &str) {
    let root = workspace.join(".agents").join("skills").join(name);
    std::fs::create_dir_all(&root).expect("skill dir");
    std::fs::write(
        root.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: \"{description}\"\n---\nbody\n"),
    )
    .expect("SKILL.md");
}

/// Writes one valid Python tool package into the workspace.
fn write_python_package(root: &Path, name: &str, description: &str) {
    let package = root.join(".agents/tools").join(name);
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
    // Generate a real uv.lock (opt-in by availability, mirroring the
    // m7_uv acceptance pattern); a missing uv skips the environment step.
    if let Some(uv) = std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|dir| dir.join("uv"))
            .find(|path| path.is_file())
    }) {
        let lock = std::process::Command::new(&uv)
            .args(["lock", "--offline", "--no-config"])
            .current_dir(&package)
            .env_clear()
            .env("PATH", "/usr/local/bin:/usr/bin:/bin")
            .env("HOME", root.parent().expect("fixture root"))
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

/// A capability view covering native + Python + Skill origins: the
/// revision, deterministic ordering, origin metadata, and Skill
/// identity/version/name/description, with no private internals on the
/// wire.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)] // one complete capability fixture
async fn capability_projection_covers_native_python_and_skills() {
    let uv = std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|dir| dir.join("uv"))
            .find(|path| path.is_file())
    });
    if uv.is_none() {
        eprintln!("uv unavailable; capability Python origin not exercised");
        return;
    }
    let dir = tempfile::tempdir().expect("workspace");
    std::fs::create_dir_all(dir.path().join("workspace")).expect("workspace dir");
    let workspace_root = dir.path().join("workspace");
    write_python_package(&workspace_root, "py-echo", "Echoes arguments");
    write_skill(&workspace_root, "skill-readme", "Reads the README");
    let tool_runtime = rustx::tools::runtime::ConversationToolRuntime::new(
        rustx::runtime::identity::ConversationId::new("conv-37-cap"),
        &workspace_root,
        dir.path().join("artifacts"),
    )
    .expect("tool runtime");

    let mut base = rustx::tools::executor::ToolRegistry::new();
    base.register(
        ToolDefinition {
            id: ToolId::new("tool-ls"),
            name: "ls".to_owned(),
            description: "list files".to_owned(),
            input_schema: serde_json::json!({"type": "object"}),
            execution_policy: ToolExecutionPolicy::ForegroundOnly,
            concurrency_policy: ToolConcurrencyPolicy::Sequential,
            replay_policy: ToolReplayPolicy::Never,
            origin: ToolOrigin::Builtin,
        },
        Arc::new(common::fake::FakeTool::new(
            ToolDefinition {
                id: ToolId::new("tool-ls"),
                name: "ls".to_owned(),
                description: "list files".to_owned(),
                input_schema: serde_json::json!({"type": "object"}),
                execution_policy: ToolExecutionPolicy::ForegroundOnly,
                concurrency_policy: ToolConcurrencyPolicy::Sequential,
                replay_policy: ToolReplayPolicy::Never,
                origin: ToolOrigin::Builtin,
            },
            common::fake::success_result("listed"),
        )),
    )
    .expect("register base tool");
    let coordinator = rustx::capabilities::CapabilityCoordinator::with_backend(
        rustx::capabilities::CapabilityCoordinatorConfig {
            conversation_id: tool_runtime.conversation_id().clone(),
            workspace: tool_runtime.workspace().clone(),
            base_tool_registry: Arc::new(base),
            mcp_servers: Vec::new(),
            base_environment: tool_runtime.environment().clone(),
            environment_store_root: dir.path().join("skill-env"),
        },
        Arc::new(common::FakeSkillEnvironmentBackend::new()),
    )
    .expect("coordinator");
    let candidate = coordinator.prepare_candidate().await.expect("prepare");
    coordinator.commit(candidate).expect("commit");

    let estimator: Arc<dyn rustx::context::TokenEstimator> =
        Arc::new(rustx::context::DefaultTokenEstimator);
    let engine = rustx::context::ContextEngine::new(
        rustx::context::ContextConfig {
            context_window_tokens: 10_000_000,
            reserve_tokens: 0,
            keep_recent_tokens: 0,
        },
        estimator,
    )
    .expect("engine");
    let host = RuntimeClientHost::new(RuntimeClientHostConfig {
        conversation_id: tool_runtime.conversation_id().clone(),
        agent_id: rustx::runtime::identity::AgentId::new("agent-a"),
        model: "scripted".to_owned(),
        protocol: rustx::model::types::ModelProtocol::OpenAiChatCompletions,
        reasoning: rustx::model::types::ReasoningEffort::Medium,
        max_output_tokens: 512,
        timezone: None,
        adapter: Arc::new(common::fake::FakeModel::new(Vec::new())),
        context: RuntimeClientContextConfig {
            engine,
            summarizer: Arc::new(common::context::FakeContextSummarizer::new(Vec::new())),
            checkpoint_store: Arc::new(rustx::context::InMemoryCheckpointStore::new()),
            status_composer: rustx::context::AgentStatusComposer::default(),
        },
        tool_runtime,
        capability: coordinator,
        clock: None,
        initial_messages: Vec::new(),
        replay_limit: None,
    })
    .expect("host");
    let (attachment, _) = host
        .attach(rustx::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1)
        .expect("attach");
    let response = attachment.handle_request(RuntimeClientRequest::CapabilityGet {
        id: rustx::runtime_client::RequestId::new(1),
    });
    let Some(RuntimeClientResult::Capability { capabilities }) = response.result else {
        panic!("capability result");
    };

    // The candidate activated a new revision (Skill + Python content).
    assert!(capabilities.revision.get() >= 1);

    // Deterministic ordering: base registry order, then discovered Python
    // tools, all in one deterministic catalog; two reads are identical.
    let names: Vec<&str> = capabilities
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect();
    assert_eq!(names, vec!["ls", "py-echo"]);
    let second = attachment.handle_request(RuntimeClientRequest::CapabilityGet {
        id: rustx::runtime_client::RequestId::new(2),
    });
    let Some(RuntimeClientResult::Capability {
        capabilities: second_view,
    }) = second.result
    else {
        panic!("capability result");
    };
    assert_eq!(second_view, capabilities, "deterministic ordering");

    // Origin metadata is correct and typed.
    assert_eq!(capabilities.tools[0].origin, ToolOrigin::Builtin);
    assert!(matches!(
        &capabilities.tools[1].origin,
        ToolOrigin::Python { tool_version_id } if !tool_version_id.as_str().is_empty()
    ));

    // Skill identity/version/name/description.
    assert_eq!(capabilities.skills.len(), 1);
    let skill = &capabilities.skills[0];
    assert_eq!(skill.name, "skill-readme");
    assert_eq!(skill.description, "Reads the README");
    assert_eq!(skill.id.as_str(), "skill-readme");
    assert!(
        skill.version_id.as_str().starts_with("sha256:"),
        "the Skill version is the deterministic content hash"
    );

    // No executor, environment path, package-manager, or dependency
    // internals ever appear on the wire.
    let json = serde_json::to_string(&capabilities).expect("serialize capabilities");
    for forbidden in ["executor", "/skill-env", "uv.lock", "pyproject", "SKILL.md"] {
        assert!(
            !json.contains(forbidden),
            "the wire projection must not leak {forbidden:?}: {json}"
        );
    }
}

/// The MCP origin is projected with its server identity; the MCP fixture
/// server serves the catalog.
#[cfg(all(unix, feature = "mcp-fixture"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)] // one complete MCP capability fixture
async fn capability_projection_covers_mcp_origins() {
    if rustx::tools::mcp::fixture::serve_if_fixture_mode(
        rustx::tools::mcp::fixture::FixtureServer::from_env(),
    )
    .await
    {
        return;
    }
    let dir = tempfile::tempdir().expect("workspace");
    std::fs::create_dir_all(dir.path().join("workspace")).expect("workspace dir");
    let workspace_root = dir.path().join("workspace");
    let tool_runtime = rustx::tools::runtime::ConversationToolRuntime::new(
        rustx::runtime::identity::ConversationId::new("conv-37-mcp"),
        &workspace_root,
        dir.path().join("artifacts"),
    )
    .expect("tool runtime");
    let mcp_config = rustx::tools::mcp::McpServerConfig {
        server_id: rustx::runtime::identity::McpServerId::new("fixture"),
        transport: rustx::tools::mcp::McpTransportConfig::Stdio {
            program: std::env::current_exe()
                .expect("test executable")
                .display()
                .to_string(),
            args: rustx::tools::mcp::fixture::fixture_spawn_args(
                "capability_projection_covers_mcp_origins",
            ),
            cwd: None,
            environment: std::collections::BTreeMap::from([(
                rustx::tools::mcp::fixture::FIXTURE_MODE_ENV.to_owned(),
                "1".to_owned(),
            )]),
        },
        policy: rustx::tools::types::ToolInvocationPolicy::default(),
    };
    let coordinator = rustx::capabilities::CapabilityCoordinator::with_backend(
        rustx::capabilities::CapabilityCoordinatorConfig {
            conversation_id: tool_runtime.conversation_id().clone(),
            workspace: tool_runtime.workspace().clone(),
            base_tool_registry: Arc::new(rustx::tools::executor::ToolRegistry::new()),
            mcp_servers: vec![mcp_config],
            base_environment: tool_runtime.environment().clone(),
            environment_store_root: dir.path().join("skill-env"),
        },
        Arc::new(common::FakeSkillEnvironmentBackend::new()),
    )
    .expect("coordinator");
    let candidate = coordinator.prepare_candidate().await.expect("prepare");
    coordinator.commit(candidate).expect("commit");

    let estimator: Arc<dyn rustx::context::TokenEstimator> =
        Arc::new(rustx::context::DefaultTokenEstimator);
    let engine = rustx::context::ContextEngine::new(
        rustx::context::ContextConfig {
            context_window_tokens: 10_000_000,
            reserve_tokens: 0,
            keep_recent_tokens: 0,
        },
        estimator,
    )
    .expect("engine");
    let host = RuntimeClientHost::new(RuntimeClientHostConfig {
        conversation_id: tool_runtime.conversation_id().clone(),
        agent_id: rustx::runtime::identity::AgentId::new("agent-a"),
        model: "scripted".to_owned(),
        protocol: rustx::model::types::ModelProtocol::OpenAiChatCompletions,
        reasoning: rustx::model::types::ReasoningEffort::Medium,
        max_output_tokens: 512,
        timezone: None,
        adapter: Arc::new(common::fake::FakeModel::new(Vec::new())),
        context: RuntimeClientContextConfig {
            engine,
            summarizer: Arc::new(common::context::FakeContextSummarizer::new(Vec::new())),
            checkpoint_store: Arc::new(rustx::context::InMemoryCheckpointStore::new()),
            status_composer: rustx::context::AgentStatusComposer::default(),
        },
        tool_runtime,
        capability: coordinator,
        clock: None,
        initial_messages: Vec::new(),
        replay_limit: None,
    })
    .expect("host");
    let (attachment, _) = host
        .attach(rustx::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1)
        .expect("attach");
    let response = attachment.handle_request(RuntimeClientRequest::CapabilityGet {
        id: rustx::runtime_client::RequestId::new(1),
    });
    let Some(RuntimeClientResult::Capability { capabilities }) = response.result else {
        panic!("capability result");
    };
    assert!(capabilities.revision.get() >= 1);
    let mcp_tools: Vec<_> = capabilities
        .tools
        .iter()
        .filter(|tool| matches!(tool.origin, ToolOrigin::Mcp { .. }))
        .collect();
    assert_eq!(mcp_tools.len(), 3, "echo, mutate, slow");
    let names: Vec<&str> = mcp_tools.iter().map(|tool| tool.name.as_str()).collect();
    assert_eq!(names, vec!["echo", "mutate", "slow"]);
    for tool in mcp_tools {
        assert!(matches!(
            &tool.origin,
            ToolOrigin::Mcp { server_id } if server_id.as_str() == "fixture"
        ));
    }
    // The wire projection carries no MCP SDK objects or transport data.
    let json = serde_json::to_string(&capabilities).expect("serialize");
    for forbidden in ["transport", "rmcp", "executor"] {
        assert!(
            !json.contains(forbidden),
            "the wire projection must not leak {forbidden:?}"
        );
    }
}
