//! Issue #81: capability startup isolation, deterministic `ToolVersion`
//! identity, and MCP protocol negotiation — the end-to-end contract.
//!
//! The invariant under test:
//!
//! > Failure of an optional capability source changes that source's
//! > availability state; it must not terminate the core conversation
//! > runtime.
//!
//! Every test composes the **real** runtime (or spawns the real process)
//! and asserts the runtime contract — alive runtime, usable native tools,
//! typed unavailable state — never merely an error string.

use crate::launch_fixture::LaunchFixture;
use std::sync::Arc;

use rustx::local_runtime::composition::{LocalConversationRuntime, LocalRuntimeDependencies};
use rustx::model::catalog::MapCredentialEnvironment;
use rustx::observation::capability_source::{
    CapabilitySourceDescriptor, CapabilitySourceStateView,
};
use rustx::runtime::identity::ConversationId;
use rustx::runtime_client::{RUNTIME_CLIENT_PROTOCOL_VERSION, RuntimeClientResult};

/// A catalog whose only model is never invoked: these tests compose the
/// runtime and drive tools directly, they do not run an attempt.
const MODELS_TOML: &str = r#"[providers.local]
base_url = "https://local.fixture.invalid/v1"
api_key = "$RUSTX_ISSUE81_KEY"

[models."local/composed-model"]
provider = "local"
id = "composed-model"
protocol = "openai_chat_completions"
context_window = 128000
max_output_tokens = 4096

[models."local/composed-model".capabilities]
input_modalities = ["text"]
output_modalities = ["text"]
tool_calls = true
reasoning = false

[models."local/composed-model".compat]
chat_reasoning_replay = "omit"
"#;

const SESSION_TOML: &str = r#"agent_id = "agent-81"

[context]
reserve_tokens = 1024
keep_recent_tokens = 8192


[agent]
[agent.model]
model = "local/composed-model"
"#;

/// Writes the startup files into a temporary root and returns the explicit
/// paths, together with the canonicalized root (the coordinator resolves
/// its private store through canonicalized paths).
fn startup(root: &tempfile::TempDir, session: &str) -> (std::path::PathBuf, LaunchFixture) {
    let canonical = std::fs::canonicalize(root.path()).expect("canonical root");
    let workspace = canonical.join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let session_path = canonical.join("rustx.toml");
    crate::launch_fixture::write_document(&session_path, &format!("{session}\n{MODELS_TOML}"));
    (
        canonical.clone(),
        LaunchFixture {
            config: session_path,
            startup_session: rustx::local_runtime::StartupSession::Empty,
            workspace,
            runtime_root: canonical.join("private"),
        },
    )
}

fn dependencies() -> LocalRuntimeDependencies {
    LocalRuntimeDependencies {
        credentials: Some(Arc::new(MapCredentialEnvironment::new([(
            "RUSTX_ISSUE81_KEY".to_owned(),
            "issue81-secret".to_owned(),
        )]))),
        ..LocalRuntimeDependencies::default()
    }
}

/// A syntactically valid managed Python tool package (Issue #174).
fn write_python_package(workspace: &std::path::Path, name: &str) {
    let package = workspace.join(".agents/tools").join(name);
    std::fs::create_dir_all(&package).expect("package directory");
    std::fs::write(
        package.join("server.py"),
        format!("from fastmcp import FastMCP\nmcp = FastMCP({name:?})\n"),
    )
    .expect("server source");
    std::fs::write(package.join("requirements.txt"), "# none\n").expect("requirements");
}

/// A malformed managed Python tool package: `server.py` without the
/// required `requirements.txt`, rejected during demanded preparation.
fn write_broken_python_package(workspace: &std::path::Path, name: &str) {
    let package = workspace.join(".agents/tools").join(name);
    std::fs::create_dir_all(&package).expect("package directory");
    std::fs::write(
        package.join("server.py"),
        "from fastmcp import FastMCP\nmcp = FastMCP('broken')\n",
    )
    .expect("server source");
}

/// The capability source descriptor of one managed Python package folder.
fn python_source(name: &str) -> CapabilitySourceDescriptor {
    CapabilitySourceDescriptor::ManagedPython {
        package: name.to_owned(),
    }
}

/// Attaches the Runtime Client and explicitly loads its bounded checkpoint.
fn attach_snapshot(
    runtime: &LocalConversationRuntime,
) -> rustx::runtime_client::RuntimeClientSnapshot {
    let (_attachment, result) = runtime
        .host()
        .attach(RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("attach");
    let RuntimeClientResult::Initialized { .. } = result else {
        panic!("initialize returns the bounded identity receipt");
    };
    runtime
        .host()
        .checkpoint()
        .expect("bounded native checkpoint")
        .snapshot
}

/// The availability view of one source, when the source was evaluated.
fn source_state(
    snapshot: &rustx::runtime_client::RuntimeClientSnapshot,
    source: &CapabilitySourceDescriptor,
) -> Option<CapabilitySourceStateView> {
    snapshot
        .capabilities
        .sources
        .iter()
        .find(|entry| entry.source == *source)
        .map(|entry| entry.state.clone())
}

/// The deterministic tool names of the committed capability set.
fn tool_names(snapshot: &rustx::runtime_client::RuntimeClientSnapshot) -> Vec<&str> {
    snapshot
        .capabilities
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect()
}

/// Executes the native `bash` tool of the committed capability set: the
/// concrete proof that the core tool plane still works while an optional
/// capability is unavailable.
async fn prove_native_tool_executes(runtime: &LocalConversationRuntime) {
    prove_native_tool_executes_with(runtime.capability(), runtime.tool_runtime()).await;
}

async fn prove_native_tool_executes_with(
    capability: &rustx::capabilities::CapabilityCoordinator,
    tool_runtime: &rustx::tools::runtime::ConversationToolRuntime,
) {
    let result = execute_committed_tool(
        capability,
        tool_runtime,
        "bash",
        serde_json::json!({"command": "echo rustx-issue-81-alive"}),
    )
    .await;
    let rendered = serde_json::to_string(&result.content).expect("content");
    assert!(
        rendered.contains("rustx-issue-81-alive"),
        "the native tool really ran: {rendered}"
    );
}

async fn execute_committed_tool(
    capability: &rustx::capabilities::CapabilityCoordinator,
    tool_runtime: &rustx::tools::runtime::ConversationToolRuntime,
    name: &str,
    arguments: serde_json::Value,
) -> rustx::tools::types::ToolExecutionResult {
    struct NoProgress;
    impl rustx::tools::executor::ProgressReporter for NoProgress {
        fn report(&self, _progress: rustx::tools::types::ToolProgress) {}
    }
    let registry = capability.current_snapshot().tool_registry().clone();
    let tool = registry
        .definitions()
        .into_iter()
        .find(|definition| definition.name == name)
        .unwrap_or_else(|| panic!("{name} must be committed"));
    let executor = registry.executor(&tool.id);
    let result = rustx::tools::executor::ToolExecutor::start(
        executor.as_ref(),
        rustx::tools::types::ToolInvocation {
            id: rustx::tools::types::ToolInvocationId::Agent {
                call_id: rustx::runtime::identity::ToolCallId::new(format!("issue81-{name}")),
            },
            tool_id: tool.id.clone(),
            tool_name: name.to_owned(),
            mode: rustx::tools::types::ToolInvocationMode::Foreground,
            arguments,
        },
        rustx::tools::executor::ToolExecutionContext::new(
            tool_runtime.conversation_id(),
            None,
            rustx::runtime::ExecutionCancellation::detached(
                rustx::runtime::CancellationSignal::new(),
                rustx::runtime::types::CancellationReason::UserRequested,
            ),
            tool_runtime.workspace(),
            &NoProgress,
            tool_runtime.artifacts(),
            tool_runtime.tool_output(),
            tool_runtime.environment(),
        ),
    )
    .completion
    .await;
    let rustx::tools::types::ToolExecutionStatus::Success = result.status else {
        panic!("{name} must execute: {:?}", result.status);
    };
    result
}

/// A selected malformed Python package still refuses composition. Optional
/// MCP startup must not weaken the managed-package admission contract.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn selected_invalid_python_refuses_composition_without_publication() {
    let root = tempfile::tempdir().expect("temp root");
    let (_canonical, paths) = startup(
        &root,
        &format!(
            "{SESSION_TOML}\n[agent.tools]\nbuiltin = [\"read\", \"write\", \"edit\", \"glob\", \"grep\", \"bash\", \"job_status\"]\n[agent.tools.sources]\n\"python:broken-tool\" = \"all\"\n"
        ),
    );
    // A package without `requirements.txt`: demanded preparation rejects it in place,
    // and its `python:broken-tool` source becomes unavailable.
    write_broken_python_package(&paths.workspace, "broken-tool");

    let failure = LocalConversationRuntime::compose(&paths.resolve(), &dependencies())
        .await
        .expect_err("selected invalid package refuses startup");
    assert!(
        format!("{failure:?}").contains("SourceUnavailable"),
        "{failure:?}"
    );
    assert!(!paths.runtime_root.join("sessions/catalog.json").exists());
}

/// Python store initialization itself fails (Issue #81 follow-up, revised
/// by Issue #174): the core environment store is valid, but a conflicting
/// regular file sits exactly where the managed package store must create
/// its `packages/` directory. The runtime composes, the discovered
/// package's source is observably unavailable with the storage diagnostic,
/// the native tools are committed and really execute, and Runtime Client
/// `initialize` succeeds. This exercises the exact constructor/store-opening
/// failure that previously escaped the optional boundary — deterministically,
/// without permission bits.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unselected_python_never_opens_its_unusable_store() {
    let root = tempfile::tempdir().expect("temp root");
    let (_, paths) = startup(
        &root,
        &format!(
            "{SESSION_TOML}\n[agent.tools]\nbuiltin = [\"read\", \"bash\"]\n[agent.tools.sources]\n\"python:fixture-tool\" = []\n"
        ),
    );
    // A valid Python package exists, so the failure cannot be attributed
    // to discovery: only opening the Python store can fail.
    write_python_package(&paths.workspace, "fixture-tool");
    // The deterministic filesystem conflict: a regular file where
    // `PythonToolStore` must create `python-tools/packages`.
    let environments = paths.environment_store_root_for(&ConversationId::new(
        "conv_9add3a87-aac5-73c8-8191-ffa50f4b3d32",
    ));
    std::fs::create_dir_all(environments.join("python-tools")).expect("python store root");
    std::fs::write(
        environments.join("python-tools/packages"),
        b"not a directory",
    )
    .expect("conflicting regular file");

    let runtime = LocalConversationRuntime::compose(&(paths).resolve(), &dependencies())
        .await
        .expect("a Python store initialization failure must not terminate composition");
    let snapshot = attach_snapshot(&runtime);

    assert!(matches!(
        source_state(&snapshot, &python_source("fixture-tool")),
        Some(CapabilitySourceStateView::Unprepared)
    ));
    assert!(
        runtime
            .runtime()
            .runtime_resources()
            .managed_python_catalog()
            .packages()
            .keys()
            .any(|id| id.to_string() == "python:fixture-tool")
    );
    assert_eq!(
        std::fs::read(environments.join("python-tools/packages")).unwrap(),
        b"not a directory"
    );
    let names = tool_names(&snapshot);
    assert!(names.contains(&"bash"), "native tools survive: {names:?}");
    assert!(
        !names.contains(&"fixture-tool"),
        "no Python executor enters the committed registry: {names:?}"
    );
    prove_native_tool_executes(&runtime).await;
}

/// The base-only capability path of a subagent child (Issue #60) is
/// structurally independent of Python storage (Issue #81 follow-up): with
/// a deliberately unusable Python store location *and* a broken Python
/// package in the workspace, coordinator construction, base-only
/// preparation, and commit all succeed without touching Python storage.
///
/// This drives the exact production ownership sequence
/// `LocalConversationCore::compose_subagent_child` performs
/// (`CapabilityCoordinator::new` -> `prepare_base_only_candidate` ->
/// `commit`), not an isolated helper.
#[test]
fn base_only_capability_setup_is_structurally_independent_of_python_storage() {
    let dir = tempfile::tempdir().expect("temp dir");
    let workspace_root = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace_root).expect("workspace");
    // A broken Python package: base-only preparation must not even
    // discover it.
    write_broken_python_package(&workspace_root, "broken-tool");
    // The Python store location is a conflicting regular file.
    let store_root = dir.path().join("skill-env");
    std::fs::create_dir_all(&store_root).expect("environment store root");
    let python_store_conflict = store_root.join("python-tools");
    std::fs::write(&python_store_conflict, b"not a directory").expect("conflicting file");

    let coordinator = rustx::capabilities::CapabilityCoordinator::new(
        rustx::capabilities::CapabilityCoordinatorConfig {
            source_demand: rustx::capabilities::source::ToolSourceDemand::default(),
            conversation_id: rustx::runtime::identity::ConversationId::new(
                "conv_fa2fc056-2798-7038-84d0-90013802d6e3",
            ),
            workspace: rustx::tools::Workspace::new(&workspace_root).expect("workspace"),
            base_tool_registry: Arc::new(rustx::tools::executor::ToolRegistry::new()),
            extension_tools: rustx::extensions::ExtensionToolPlane::none(),
            agent_activation: rustx::capabilities::AgentActivation::default(),
            skill_discovery: rustx::skills::SkillDiscoveryConfig::default(),
            mcp_servers: std::collections::BTreeMap::new(),
            base_environment: rustx::tools::environment::ToolEnvironment::new(),
            environment_store_root: store_root,
        },
    )
    .expect("coordinator construction must not depend on Python storage");
    let candidate = coordinator
        .prepare_base_only_candidate()
        .expect("base-only preparation must not depend on Python storage");
    let snapshot = coordinator.commit(candidate).expect("base-only commit");
    assert_eq!(snapshot.revision().get(), 0, "an empty base set is a no-op");
    assert!(
        coordinator.availability().is_empty(),
        "base-only preparation evaluates no optional source"
    );
    assert!(
        python_store_conflict.is_file(),
        "the conflicting file was never replaced by Python storage"
    );
    assert!(
        !dir.path().join("skill-env/python-tools/packages").exists(),
        "no managed Python package storage was created"
    );
}

// NOTE (Issue #174): the corrupt-persisted-ToolVersion regression was
// removed with the old per-tool Python store. The fail-closed revalidation
// of a tampered prepared state (manifest or source mismatch rejects the
// state, never repairs it) is now covered by the `tools::python` unit
// tests against the fingerprint-keyed `packages/<fingerprint>` layout.

/// The fatal/isolated boundary: failures that prove the *core* runtime or
/// the *base* capability plane cannot be constructed stay fatal.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn core_and_base_plane_failures_remain_fatal() {
    // A session selecting a model the catalog does not declare: fatal.
    let root = tempfile::tempdir().expect("temp root");
    let bad_model = SESSION_TOML.replace("local/composed-model", "local/absent-model");
    let (_canonical, paths) = startup(&root, &bad_model);
    assert!(
        paths
            .try_resolve()
            .unwrap_err()
            .contains("unknown catalog model")
    );

    // A malformed Skill package is *not* a base capability-plane failure
    // (Issue #280): it is excluded from the effective catalog with a typed
    // generation diagnostic while every unrelated valid package publishes.
    // One malformed Skill must never make the runtime unlaunchable.
    let root = tempfile::tempdir().expect("temp root");
    let (_canonical, paths) = startup(&root, SESSION_TOML);
    let skills = paths.workspace.join(".agents/skills");
    let broken = skills.join("broken");
    std::fs::create_dir_all(&broken).expect("skill directory");
    std::fs::write(broken.join("SKILL.md"), "not valid frontmatter at all").expect("SKILL.md");
    let valid = skills.join("intact");
    std::fs::create_dir_all(&valid).expect("skill directory");
    std::fs::write(
        valid.join("SKILL.md"),
        "---\nname: intact\ndescription: Intact guidance.\n---\nbody\n",
    )
    .expect("SKILL.md");
    let resolved = paths
        .try_resolve()
        .expect("a malformed Skill package never blocks the launch");
    assert_eq!(
        resolved
            .skill_provenance()
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        ["intact"]
    );
    assert!(
        resolved
            .skill_diagnostics()
            .iter()
            .any(|fact| matches!(fact, rustx::skills::SkillDiagnostic::PackageInvalid { .. })),
        "{:?}",
        resolved.skill_diagnostics()
    );

    // An *explicit* Skill launch path that does not exist stays fatal: it is
    // authored launch intent, not discovered content.
    let root = tempfile::tempdir().expect("temp root");
    let (_canonical, paths) = startup(&root, SESSION_TOML);
    std::fs::write(
        paths.workspace.join("rustx.toml"),
        "[agent]\nskills = ['absent-skill']",
    )
    .unwrap();
    let error = paths
        .try_resolve()
        .expect_err("selected missing Skill refuses resolution");
    assert!(error.to_lowercase().contains("skill"), "{error}");
}

/// MCP-level regressions driving the real self-spawned fixture servers.
#[cfg(all(unix, feature = "mcp-fixture"))]
mod mcp {
    use rustx::local_runtime::composition::HeadlessConversationRuntime;
    use rustx::tools::mcp::fixture::{self, FixtureServer, PROTOCOL_VERSIONS_ENV};
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    use super::{dependencies, execute_committed_tool, prove_native_tool_executes_with, startup};

    const LIVENESS: std::time::Duration = std::time::Duration::from_mins(2);
    const STARTUP_GATE_ENV: &str = "RUSTX_ISSUE81_STARTUP_GATE";

    #[derive(Clone, Copy)]
    enum StartupCompletion {
        Discover,
        ListTools,
    }

    /// Keep the real stdio peer before discovery until the parent has observed
    /// composition remain pending. Completion is an explicit control-socket message
    /// from rmcp's request handler; elapsed time never stands in for readiness.
    async fn serve_delayed_startup_fixture(completion: StartupCompletion) -> bool {
        let Ok(address) = std::env::var(STARTUP_GATE_ENV) else {
            return false;
        };
        let mut gate = tokio::net::TcpStream::connect(address)
            .await
            .expect("connect startup gate");
        gate.write_u8(1)
            .await
            .expect("fixture entered startup gate");
        assert_eq!(gate.read_u8().await.expect("startup gate release"), 1);
        let (mut control, responses) = gate.into_split();
        let responses = std::sync::Arc::new(tokio::sync::Mutex::new(responses));
        let fixture = StartupFixture {
            server: FixtureServer::from_env(),
            responses: responses.clone(),
            completion,
            reported: std::sync::atomic::AtomicBool::new(false),
        };
        let list_tools_calls = fixture.server.list_tools_calls.clone();
        let mut serving = tokio::spawn(async move {
            use rmcp::ServiceExt as _;
            fixture
                .serve(rmcp::transport::stdio())
                .await
                .expect("fixture server")
                .waiting()
                .await
                .expect("fixture server wait");
        });
        tokio::select! {
            _ = &mut serving => return true,
            query = control.read_u8() => {
                if query.is_ok_and(|query| query == 3) {
                    let count = u64::try_from(
                        list_tools_calls.load(std::sync::atomic::Ordering::Acquire),
                    ).expect("bounded fixture discovery count");
                    responses.lock().await.write_u64(count)
                        .await
                        .expect("fixture discovery count");
                }
            }
        }
        let _ = serving.await;
        true
    }

    struct StartupFixture {
        server: FixtureServer,
        responses: std::sync::Arc<tokio::sync::Mutex<tokio::net::tcp::OwnedWriteHalf>>,
        completion: StartupCompletion,
        reported: std::sync::atomic::AtomicBool,
    }

    impl rmcp::service::Service<rmcp::RoleServer> for StartupFixture {
        async fn handle_request(
            &self,
            request: rmcp::model::ClientRequest,
            context: rmcp::service::RequestContext<rmcp::RoleServer>,
        ) -> Result<rmcp::model::ServerResult, rmcp::ErrorData> {
            let terminal = matches!(
                (&request, self.completion),
                (
                    rmcp::model::ClientRequest::DiscoverRequest(_),
                    StartupCompletion::Discover,
                ) | (
                    rmcp::model::ClientRequest::ListToolsRequest(_),
                    StartupCompletion::ListTools,
                )
            );
            // Delegate before signalling, including protocol rejection in
            // rmcp's middleware before its ServerHandler::discover dispatch.
            let result = self.server.handle_request(request, context).await;
            if terminal
                && !self
                    .reported
                    .swap(true, std::sync::atomic::Ordering::AcqRel)
            {
                // Publish the receipt before handing the response back to
                // rmcp. A rejected peer may be killed as soon as the client
                // reads that response, so a detached notifier would race it.
                self.responses
                    .lock()
                    .await
                    .write_u8(2)
                    .await
                    .expect("fixture discovery completed");
            }
            result
        }

        async fn handle_notification(
            &self,
            notification: rmcp::model::ClientNotification,
            context: rmcp::service::NotificationContext<rmcp::RoleServer>,
        ) -> Result<(), rmcp::ErrorData> {
            self.server.handle_notification(notification, context).await
        }

        fn get_info(&self) -> rmcp::model::ServerInfo {
            self.server.get_info()
        }

        fn supported_protocol_versions(
            &self,
        ) -> std::borrow::Cow<'static, [rmcp::model::ProtocolVersion]> {
            self.server.supported_protocol_versions()
        }
    }

    async fn startup_gate() -> tokio::net::TcpListener {
        tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("startup gate")
    }

    async fn held_startup<T>(
        gate: &tokio::net::TcpListener,
        composing: &mut tokio::task::JoinHandle<T>,
    ) -> tokio::net::TcpStream {
        tokio::time::timeout(LIVENESS, async {
            let (mut fixture, _) = tokio::select! {
                biased;
                _ = &mut *composing => panic!("composition returned before selected live discovery"),
                entered = gate.accept() => entered.expect("fixture entered startup gate"),
            };
            assert_eq!(fixture.read_u8().await.expect("fixture startup receipt"), 1);
            assert!(futures_util::poll!(&mut *composing).is_pending(),
                "composition cannot publish while selected discovery is held");
            fixture
        })
        .await
        .expect("real child reaches its startup gate")
    }

    async fn release_startup(fixture: &mut tokio::net::TcpStream) {
        tokio::time::timeout(LIVENESS, async {
            fixture
                .write_u8(1)
                .await
                .expect("release fixture discovery");
            assert_eq!(
                fixture.read_u8().await.expect("fixture discovery receipt"),
                2
            );
        })
        .await
        .expect("fixture discovery completes");
    }

    /// A session with two stdio MCP servers: `good` (the real fixture) and
    /// `bad` (a program that does not exist).
    fn session_with_two_servers(
        root: &std::path::Path,
        program: &str,
        args: &[String],
        gate: std::net::SocketAddr,
    ) -> String {
        crate::launch_fixture::write_mcp(
            &root.join("workspace"),
            &serde_json::json!({
                "good": {
                    "command": program,
                    "args": args,
                    "env": {
                        fixture::FIXTURE_MODE_ENV: "1",
                        STARTUP_GATE_ENV: gate.to_string(),
                    },
                },
                "bad": {
                    "command": "/nonexistent/rustx-issue81-absent-server",
                    "args": [],
                },
            }),
        );
        toml::to_string_pretty(&serde_json::json!({"agent_id": "agent-81", "context": {"reserve_tokens": 1024, "keep_recent_tokens": 8192},  "agent": {"model": {"model": "local/composed-model"}, "tools": {"builtin": ["read", "write", "edit", "glob", "grep", "bash", "job_status"], "sources": {"good": "all", "bad": "all"}}}}))
        .unwrap()
    }

    /// One failed MCP server never suppresses a successful one (Issue #81):
    /// the bad server is observably unavailable, the good server is ready
    /// and its tools are committed and callable, and native tools still
    /// execute.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn one_mcp_server_failure_never_suppresses_a_successful_one() {
        if serve_delayed_startup_fixture(StartupCompletion::ListTools).await {
            return;
        }
        let root = tempfile::tempdir().expect("temp root");
        let gate = startup_gate().await;
        let program = std::env::current_exe()
            .expect("test executable")
            .display()
            .to_string();
        let args = fixture::fixture_spawn_args(
            "capability_startup::mcp::one_mcp_server_failure_never_suppresses_a_successful_one",
        );
        let session =
            session_with_two_servers(root.path(), &program, &args, gate.local_addr().unwrap());
        let (_canonical, paths) = startup(&root, &session);

        let mut composing = tokio::spawn(async move {
            HeadlessConversationRuntime::compose(&paths.resolve(), &dependencies()).await
        });
        let mut peer = held_startup(&gate, &mut composing).await;
        release_startup(&mut peer).await;
        let runtime = tokio::time::timeout(LIVENESS, composing)
            .await
            .expect("composition settles after discovery")
            .expect("composition task")
            .expect("one failing MCP server must not terminate composition");
        let good = rustx::capabilities::ToolSourceId::Mcp(
            rustx::runtime::identity::McpServerId::new("good"),
        );
        let bad = rustx::capabilities::ToolSourceId::Mcp(
            rustx::runtime::identity::McpServerId::new("bad"),
        );
        assert!(
            runtime.capability().availability().keys().all(|source| {
                !matches!(source, rustx::capabilities::ToolSourceId::ManagedPython(_))
            }),
            "no packages means no managed Python source is evaluated"
        );
        let availability = runtime.capability().availability();
        assert_eq!(
            availability[&good],
            rustx::capabilities::CapabilitySourceState::Ready
        );
        assert!(
            matches!(
                availability[&bad],
                rustx::capabilities::CapabilitySourceState::Unavailable { .. }
            ),
            "the selected bad source fails in isolation: {availability:?}"
        );
        let prepared = runtime.capability().current_snapshot();
        let definitions = prepared.tool_registry().definitions();
        let names: Vec<_> = definitions.iter().map(|tool| tool.name.as_str()).collect();
        for expected in ["echo", "mutate", "slow", "bash"] {
            assert!(
                names.contains(&expected),
                "committed tools include {expected}: {names:?}"
            );
        }
        let result = execute_committed_tool(
            runtime.capability(),
            runtime.tool_runtime(),
            "echo",
            serde_json::json!({}),
        )
        .await;
        assert!(
            serde_json::to_string(&result.content)
                .unwrap()
                .contains("fixture echo"),
            "the prepared MCP executor calls the real peer"
        );
        tokio::time::timeout(LIVENESS, async {
            peer.write_u8(3).await.expect("query discovery count");
            assert_eq!(
                peer.read_u64().await.expect("discovery count receipt"),
                1,
                "composition and live validation reuse the completed discovery"
            );
        })
        .await
        .expect("fixture discovery count is observable");
        prove_native_tool_executes_with(runtime.capability(), runtime.tool_runtime()).await;
        runtime.shutdown().await.expect("physical settlement");
    }

    /// Dropping the composition waiter cancels the counted preparation owner;
    /// the held real child must settle without a discovery release.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cancelled_mcp_composition_settles_the_held_peer() {
        if serve_delayed_startup_fixture(StartupCompletion::ListTools).await {
            return;
        }
        let root = tempfile::tempdir().expect("temp root");
        let gate = startup_gate().await;
        let program = std::env::current_exe()
            .expect("test executable")
            .display()
            .to_string();
        let args = fixture::fixture_spawn_args(
            "capability_startup::mcp::cancelled_mcp_composition_settles_the_held_peer",
        );
        let session =
            session_with_two_servers(root.path(), &program, &args, gate.local_addr().unwrap());
        let (_canonical, paths) = startup(&root, &session);
        let mut composing = tokio::spawn(async move {
            super::LocalConversationRuntime::compose(&paths.resolve(), &dependencies()).await
        });
        let mut peer = held_startup(&gate, &mut composing).await;
        composing.abort();
        assert!(composing.await.is_err_and(|error| error.is_cancelled()));
        assert!(
            tokio::time::timeout(LIVENESS, peer.read_u8())
                .await
                .expect("settled child releases its control socket")
                .is_err(),
            "cancelled composition terminates the held peer without releasing discovery"
        );
    }

    /// No shared MCP protocol revision: the server becomes observably
    /// unavailable with the compatibility diagnostic and the runtime starts
    /// anyway (Issue #81, negotiation Case C at the composition level).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn no_shared_mcp_revision_is_unavailable_not_fatal() {
        if serve_delayed_startup_fixture(StartupCompletion::Discover).await {
            return;
        }
        let root = tempfile::tempdir().expect("temp root");
        let gate = startup_gate().await;
        let program = std::env::current_exe()
            .expect("test executable")
            .display()
            .to_string();
        let args = fixture::fixture_spawn_args(
            "capability_startup::mcp::no_shared_mcp_revision_is_unavailable_not_fatal",
        );
        crate::launch_fixture::write_mcp(
            &root.path().join("workspace"),
            &serde_json::json!({
                "alien": {
                    "command": program,
                    "args": args,
                    "env": {
                        fixture::FIXTURE_MODE_ENV: "1",
                        STARTUP_GATE_ENV: gate.local_addr().unwrap().to_string(),
                        PROTOCOL_VERSIONS_ENV: "1999-01-01",
                    },
                },
            }),
        );
        let session = toml::to_string_pretty(&serde_json::json!({"agent_id": "agent-81", "context": {"reserve_tokens": 1024, "keep_recent_tokens": 8192},  "agent": {"model": {"model": "local/composed-model"}, "tools": {"builtin": ["read", "write", "edit", "glob", "grep", "bash", "job_status"], "sources": {"alien": "all"}}}}))
        .unwrap();
        let (_canonical, paths) = startup(&root, &session);

        let mut composing = tokio::spawn(async move {
            HeadlessConversationRuntime::compose(&paths.resolve(), &dependencies()).await
        });
        let mut peer = held_startup(&gate, &mut composing).await;
        release_startup(&mut peer).await;
        let runtime = tokio::time::timeout(LIVENESS, composing)
            .await
            .expect("composition settles after discovery")
            .expect("composition task")
            .expect("optional MCP negotiation failure permits Root startup");
        let source = rustx::capabilities::ToolSourceId::Mcp(
            rustx::runtime::identity::McpServerId::new("alien"),
        );
        let availability = runtime.capability().availability();
        let rustx::capabilities::CapabilitySourceState::Unavailable { reason } =
            &availability[&source]
        else {
            panic!("negotiation failure must be unavailable: {availability:?}");
        };
        assert!(!reason.is_empty());
        assert!(reason.len() < 4096, "external diagnostics stay bounded");
        prove_native_tool_executes_with(runtime.capability(), runtime.tool_runtime()).await;
        runtime.shutdown().await.expect("physical settlement");
    }

    /// An external MCP peer can emit an arbitrarily large diagnostic
    /// (Issue #81 follow-up): the fixture fails `tools/list` with a huge
    /// correlated error message, and the stored availability reason is
    /// bounded at the capability-owning boundary before it enters the
    /// authoritative state. Composition completes with the terminal optional
    /// failure, and native tools remain usable.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn an_oversized_mcp_diagnostic_is_bounded_before_authoritative_state() {
        if serve_delayed_startup_fixture(StartupCompletion::ListTools).await {
            return;
        }
        let root = tempfile::tempdir().expect("temp root");
        let gate = startup_gate().await;
        let program = std::env::current_exe()
            .expect("test executable")
            .display()
            .to_string();
        let args = fixture::fixture_spawn_args(
            "capability_startup::mcp::an_oversized_mcp_diagnostic_is_bounded_before_authoritative_state",
        );
        crate::launch_fixture::write_mcp(
            &root.path().join("workspace"),
            &serde_json::json!({
                "exa": {
                    "command": "/nonexistent/rustx-issue81-absent-server",
                    "args": [],
                },
            }),
        );
        crate::launch_fixture::write_mcp(
            &root.path().join("workspace"),
            &serde_json::json!({
                "loud": {
                    "command": program,
                    "args": args,
                    "env": {
                        fixture::FIXTURE_MODE_ENV: "1",
                        STARTUP_GATE_ENV: gate.local_addr().unwrap().to_string(),
                        fixture::LIST_TOOLS_ERROR_BYTES_ENV: "65536",
                    },
                },
            }),
        );
        let session = toml::to_string_pretty(&serde_json::json!({"agent_id": "agent-81", "context": {"reserve_tokens": 1024, "keep_recent_tokens": 8192},  "agent": {"model": {"model": "local/composed-model"}, "tools": {"builtin": ["read", "write", "edit", "glob", "grep", "bash", "job_status"], "sources": {"loud": "all"}}}}))
        .unwrap();
        let (_canonical, paths) = startup(&root, &session);

        let mut composing = tokio::spawn(async move {
            HeadlessConversationRuntime::compose(&paths.resolve(), &dependencies()).await
        });
        let mut peer = held_startup(&gate, &mut composing).await;
        release_startup(&mut peer).await;
        let runtime = tokio::time::timeout(LIVENESS, composing)
            .await
            .expect("composition settles after discovery")
            .expect("composition task")
            .expect("optional MCP catalog failure permits Root startup");
        let source = rustx::capabilities::ToolSourceId::Mcp(
            rustx::runtime::identity::McpServerId::new("loud"),
        );
        let availability = runtime.capability().availability();
        let rustx::capabilities::CapabilitySourceState::Unavailable { reason } =
            &availability[&source]
        else {
            panic!("failed catalog must be unavailable: {availability:?}");
        };
        assert!(!reason.is_empty());
        assert!(reason.len() < 4096, "external diagnostics stay bounded");
        prove_native_tool_executes_with(runtime.capability(), runtime.tool_runtime()).await;
        runtime.shutdown().await.expect("physical settlement");
    }
}

/// Sends one App Server JSON-RPC request over the owned stdio pipes and
/// returns its correlated `result`, skipping notifications.
async fn app_server_call(
    stdin: &mut tokio::process::ChildStdin,
    stdout: &mut tokio::io::Lines<tokio::io::BufReader<tokio::process::ChildStdout>>,
    id: i64,
    call: rustx::app_server::protocol::Method,
) -> rustx::app_server::protocol::MethodResult {
    use tokio::io::AsyncWriteExt as _;
    const LIVENESS: std::time::Duration = std::time::Duration::from_mins(2);
    let method = format!("{call:?}");
    let request = serde_json::to_string(&rustx::app_server::protocol::Request {
        jsonrpc: rustx::app_server::protocol::JsonRpcVersion::V2,
        id: rustx::app_server::protocol::RequestId::Integer(id),
        call,
    })
    .unwrap();
    tokio::time::timeout(LIVENESS, async {
        stdin
            .write_all(format!("{request}\n").as_bytes())
            .await
            .expect("write");
        loop {
            let record = stdout
                .next_line()
                .await
                .expect("read")
                .expect("the process closed stdout before responding");
            let value: serde_json::Value = serde_json::from_str(&record).unwrap_or_else(|error| {
                panic!("stdout carries protocol only: {record:?} ({error})")
            });
            if value.get("id").is_none() {
                continue;
            }
            assert_eq!(value["id"], id, "responses correlate by request id");
            match serde_json::from_str(&record).expect("a typed App Server response") {
                rustx::app_server::protocol::Response::Success(success) => return success.result,
                rustx::app_server::protocol::Response::Failure(failure) => {
                    panic!("{method} failed: {failure:?}")
                }
            }
        }
    })
    .await
    .expect("the process must answer")
}

/// The original user-facing symptom (Issue #81): the TUI saw the runtime
/// close its transport output stream. This drives the **real `rustx`
/// App Server process** with both a broken Python package and an
/// unreachable MCP server selected: attaching the Session must compose, its
/// initial snapshot must carry usable native capabilities and the typed
/// unavailable state, and the process must keep serving until its owner
/// ends it explicitly.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)] // one complete process-level regression
async fn the_process_stays_alive_and_serves_when_optional_capabilities_fail() {
    use std::process::Stdio;
    use tokio::io::{AsyncBufReadExt as _, BufReader};

    const LIVENESS: std::time::Duration = std::time::Duration::from_mins(2);

    let root = tempfile::tempdir().expect("temp root");
    let workspace = root.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    // A broken Python package (its Managed Python preparation fails) ...
    let package = workspace.join(".agents/tools/broken-tool");
    std::fs::create_dir_all(&package).expect("package directory");
    std::fs::write(
        package.join("server.py"),
        "from fastmcp import FastMCP\nmcp = FastMCP('broken')\n",
    )
    .expect("server source without the required requirements.txt");
    // ... and an MCP server whose program does not exist.
    let session = serde_json::json!({"agent_id": "agent-81", "context": {"reserve_tokens": 1024, "keep_recent_tokens": 8192},  "agent": {"model": {"model": "local/composed-model"}, "tools": {"builtin": ["read", "write", "edit", "glob", "grep", "bash", "job_status"], "sources": {"exa": [], "python:broken-tool": []}}}});
    crate::launch_fixture::write_document(
        &root.path().join("rustx.toml"),
        &format!(
            "{}\n{MODELS_TOML}",
            toml::to_string_pretty(&session).unwrap()
        ),
    );
    let session_id = rustx::local_runtime::session_controller::SessionController::open(
        &root.path().join("private"),
    )
    .unwrap()
    .create_session(
        rustx::local_runtime::session::SessionPersistentState::from_input(
            &rustx::local_runtime::configuration::SessionConfigInput::new(workspace.clone()),
        ),
    )
    .await
    .unwrap()
    .session
    .id;
    let home = root.path().join("host");
    std::fs::create_dir_all(&home).unwrap();

    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_rustx"))
        .args(["app-server", "--listen", "stdio", "--config"])
        .arg(root.path().join("rustx.toml"))
        .arg("--runtime-root")
        .arg(root.path().join("private"))
        .env_clear()
        .env("HOME", home)
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("RUSTX_ISSUE81_KEY", "issue81-secret")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn the rustx binary");
    let mut stdin = child.stdin.take().expect("stdin is piped");
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout is piped")).lines();

    app_server_call(
        &mut stdin,
        &mut stdout,
        1,
        rustx::app_server::protocol::Method::Initialize(
            rustx::app_server::protocol::InitializeParams {
                protocol_version: rustx::app_server::protocol::APP_SERVER_PROTOCOL_VERSION,
                client: rustx::app_server::protocol::ClientIdentity {
                    name: "issue-81".into(),
                    version: "1".into(),
                },
                presentation: rustx::app_server::protocol::PresentationCapabilities {
                    images: false,
                    questionnaires: false,
                    reviews: false,
                },
            },
        ),
    )
    .await;
    // The exact operation the TUI failed on: composing the Session runtime.
    let attached = app_server_call(
        &mut stdin,
        &mut stdout,
        2,
        rustx::app_server::protocol::Method::SessionAttach {
            session_id,
            node_id: None,
        },
    )
    .await;
    let rustx::app_server::protocol::MethodResult::Attached { target, .. } = attached else {
        panic!("session/attach must compose the Session: {attached:?}");
    };
    let rustx::app_server::protocol::MethodResult::Checkpoint { checkpoint } = app_server_call(
        &mut stdin,
        &mut stdout,
        20,
        rustx::app_server::protocol::Method::SessionCheckpoint {
            target: target.clone(),
            trace_records: vec![],
        },
    )
    .await
    else {
        panic!("bounded checkpoint")
    };
    let snapshot = checkpoint.snapshot;
    let names = tool_names(&snapshot);
    for expected in ["read", "write", "bash"] {
        assert!(
            names.contains(&expected),
            "native tool {expected} must be in the initial snapshot: {names:?}"
        );
    }
    assert_eq!(
        source_state(&snapshot, &python_source("broken-tool")),
        Some(CapabilitySourceStateView::Unprepared)
    );
    let exa = CapabilitySourceDescriptor::Mcp {
        server_id: rustx::runtime::identity::McpServerId::new("exa"),
    };
    assert_eq!(source_state(&snapshot, &exa), None);
    // The process keeps serving after the isolated failures.
    let models = app_server_call(
        &mut stdin,
        &mut stdout,
        3,
        rustx::app_server::protocol::Method::ModelCatalog { target },
    )
    .await;
    assert!(
        matches!(
            models,
            rustx::app_server::protocol::MethodResult::DomainPage { .. }
        ),
        "the runtime keeps serving: {models:?}"
    );

    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(i32::try_from(child.id().unwrap()).unwrap()),
        nix::sys::signal::Signal::SIGTERM,
    )
    .unwrap();
    let status = tokio::time::timeout(LIVENESS, child.wait())
        .await
        .expect("the owned process ends on its explicit shutdown signal")
        .expect("wait");
    assert!(status.success(), "clean shutdown, not a crash: {status}");
}
