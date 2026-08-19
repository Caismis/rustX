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

mod common;

use std::sync::Arc;

use rustx::local_runtime::composition::{
    LocalConversationRuntime, LocalRuntimeDependencies, LocalRuntimeError, LocalRuntimePaths,
};
use rustx::model::catalog::MapCredentialEnvironment;
use rustx::runtime_client::snapshot::{CapabilitySourceDescriptor, CapabilitySourceStateView};
use rustx::runtime_client::{RUNTIME_CLIENT_PROTOCOL_VERSION_V1, RuntimeClientResult};

/// A catalog whose only model is never invoked: these tests compose the
/// runtime and drive tools directly, they do not run an attempt.
const MODELS_JSON: &str = r#"{
  "providers": {
    "local": {
      "baseUrl": "https://local.fixture.invalid/v1",
      "apiKey": "$RUSTX_ISSUE81_KEY",
      "models": [
        {
          "id": "composed-model",
          "protocol": "openai_chat_completions",
          "contextWindow": 128000,
          "maxOutputTokens": 4096,
          "capabilities": {
            "inputModalities": ["text"],
            "outputModalities": ["text"],
            "toolCalls": true,
            "reasoning": false
          },
          "compat": {"chatReasoningReplay": "omit"}
        }
      ]
    }
  }
}"#;

const SESSION_JSON: &str = r#"{
  "conversationId": "conv-81",
  "agentId": "agent-81",
  "model": {"model": "local/composed-model"},
  "context": {"reserveTokens": 1024, "keepRecentTokens": 8192}
}"#;

/// Writes the startup files into a temporary root and returns the explicit
/// paths, together with the canonicalized root (the coordinator resolves
/// its private store through canonicalized paths).
fn startup(root: &tempfile::TempDir, session: &str) -> (std::path::PathBuf, LocalRuntimePaths) {
    let canonical = std::fs::canonicalize(root.path()).expect("canonical root");
    let workspace = canonical.join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let models_path = canonical.join("models.json");
    let session_path = canonical.join("session.json");
    std::fs::write(&models_path, MODELS_JSON).expect("models.json");
    std::fs::write(&session_path, session).expect("session.json");
    (
        canonical.clone(),
        LocalRuntimePaths {
            models: models_path,
            session: session_path,
            workspace,
            runtime_root: canonical.join("private"),
        },
    )
}

fn dependencies() -> LocalRuntimeDependencies {
    LocalRuntimeDependencies {
        credentials: Arc::new(MapCredentialEnvironment::new([(
            "RUSTX_ISSUE81_KEY".to_owned(),
            "issue81-secret".to_owned(),
        )])),
        ..LocalRuntimeDependencies::default()
    }
}

/// A syntactically valid custom Python tool package.
fn write_python_package(workspace: &std::path::Path, name: &str) {
    let package = workspace.join(".agents/tools").join(name);
    std::fs::create_dir_all(&package).expect("package directory");
    std::fs::write(
        package.join("TOOL.toml"),
        format!(
            "schema_version = 1\nname = {name:?}\ndescription = \"Fixture\"\nentrypoint = \"tool:main\"\nexecution = \"foreground_only\"\nconcurrency = \"sequential\"\n"
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
    std::fs::write(package.join("uv.lock"), "version = 1\nrevision = 1\n").expect("lock");
    std::fs::write(
        package.join("tool.py"),
        "def main(arguments):\n    return arguments\n",
    )
    .expect("source");
}

/// Attaches the Runtime Client and returns the initialized snapshot.
fn attach_snapshot(
    runtime: &LocalConversationRuntime,
) -> rustx::runtime_client::RuntimeClientSnapshot {
    let (_attachment, result) = runtime
        .host()
        .attach(RUNTIME_CLIENT_PROTOCOL_VERSION_V1)
        .expect("attach");
    let RuntimeClientResult::Initialized { snapshot, .. } = result else {
        panic!("initialize returns the snapshot");
    };
    snapshot
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
    struct NoProgress;
    impl rustx::tools::executor::ProgressReporter for NoProgress {
        fn report(&self, _progress: rustx::tools::types::ToolProgress) {}
    }
    let registry = runtime
        .capability()
        .current_snapshot()
        .tool_registry()
        .clone();
    let bash = registry
        .definitions()
        .into_iter()
        .find(|definition| definition.name == "bash")
        .expect("the native bash tool is committed");
    let executor = registry.executor(&bash.id);
    let tool_runtime = runtime.tool_runtime();
    let result = rustx::tools::executor::ToolExecutor::execute(
        executor.as_ref(),
        rustx::tools::types::ToolInvocation {
            call_id: rustx::runtime::identity::ToolCallId::new("issue81-bash"),
            tool_id: bash.id.clone(),
            tool_name: "bash".to_owned(),
            mode: rustx::tools::types::ToolInvocationMode::Foreground,
            arguments: serde_json::json!({"command": "echo rustx-issue-81-alive"}),
        },
        rustx::tools::executor::ToolExecutionContext {
            conversation_id: tool_runtime.conversation_id(),
            execution_id: None,
            cancellation: rustx::runtime::ExecutionCancellation::detached(
                rustx::runtime::CancellationSignal::new(),
                rustx::runtime::types::CancellationReason::UserRequested,
            ),
            workspace: tool_runtime.workspace(),
            progress: &NoProgress,
            artifacts: tool_runtime.artifacts(),
            environment: tool_runtime.environment(),
        },
    )
    .await;
    let rustx::tools::types::ToolExecutionStatus::Success = result.status else {
        panic!("the native bash tool must execute: {:?}", result.status);
    };
    let rendered = serde_json::to_string(&result.content).expect("content");
    assert!(
        rendered.contains("rustx-issue-81-alive"),
        "the native tool really ran: {rendered}"
    );
}

/// A malformed Python tool package must not terminate startup: the runtime
/// composes, the Python source is observably unavailable, the native tool
/// plane is committed and really executes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_python_capability_failure_is_isolated_from_runtime_startup() {
    let root = tempfile::tempdir().expect("temp root");
    let (_canonical, paths) = startup(&root, SESSION_JSON);
    // A package whose manifest name does not match its directory: discovery
    // rejects it, and the whole Python plane becomes unavailable.
    let package = paths.workspace.join(".agents/tools/broken-tool");
    std::fs::create_dir_all(&package).expect("package directory");
    std::fs::write(
        package.join("TOOL.toml"),
        "schema_version = 1\nname = \"other-name\"\ndescription = \"Broken\"\nentrypoint = \"tool:main\"\nexecution = \"foreground_only\"\nconcurrency = \"sequential\"\n",
    )
    .expect("manifest");

    let runtime = LocalConversationRuntime::compose(&paths, &dependencies())
        .await
        .expect("a broken Python package must not terminate composition");
    let snapshot = attach_snapshot(&runtime);

    let names = tool_names(&snapshot);
    for expected in [
        "background_task",
        "read",
        "write",
        "edit",
        "glob",
        "grep",
        "bash",
    ] {
        assert!(
            names.contains(&expected),
            "the native tool {expected} must survive the Python failure: {names:?}"
        );
    }
    assert!(
        !names.iter().any(|name| name.contains("broken")),
        "no partially initialized Python executor enters the committed registry: {names:?}"
    );
    let Some(CapabilitySourceStateView::Unavailable { reason }) =
        source_state(&snapshot, &CapabilitySourceDescriptor::Python)
    else {
        panic!(
            "the Python source must be observably unavailable: {:?}",
            snapshot.capabilities.sources
        );
    };
    assert!(
        reason.contains("invalid Python tool package"),
        "the reason carries the real diagnostic: {reason}"
    );
    prove_native_tool_executes(&runtime).await;
}

/// A corrupt persisted `ToolVersion` (Issue #81 root-cause regression):
/// storage recomputes the identity from the persisted source, detects the
/// mismatch, and the Python capability becomes unavailable — while the
/// runtime stays alive and native tools keep working. No repair, no
/// migration, no compatibility path.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_corrupt_published_tool_version_marks_python_unavailable_and_keeps_the_runtime_alive() {
    let root = tempfile::tempdir().expect("temp root");
    let (canonical, paths) = startup(&root, SESSION_JSON);
    write_python_package(&paths.workspace, "fixture-tool");

    // Compute the package identity through real discovery, then seed the
    // persisted store with an entry whose marker claims that identity but
    // whose source bytes no longer match it.
    let workspace = rustx::tools::Workspace::new(&paths.workspace).expect("workspace");
    let package = rustx::tools::python::PythonToolDiscovery::new(&workspace)
        .discover()
        .expect("discover")
        .pop()
        .expect("one package");
    let published = canonical
        .join("private/environments/m7-tools/tool-versions")
        .join(package.tool_version_id.as_str());
    std::fs::create_dir_all(published.join("source")).expect("published source");
    std::fs::write(
        published.join("RUSTX_TOOL_VERSION.json"),
        serde_json::to_vec(&serde_json::json!({
            "format": 1,
            "tool_version_id": package.tool_version_id.as_str(),
        }))
        .expect("marker"),
    )
    .expect("marker write");
    std::fs::write(
        published.join("source/tool.py"),
        b"def main(arguments):\n    return \"tampered\"\n",
    )
    .expect("tampered source");

    let runtime = LocalConversationRuntime::compose(&paths, &dependencies())
        .await
        .expect("a corrupt ToolVersion store must not terminate composition");
    let snapshot = attach_snapshot(&runtime);

    let Some(CapabilitySourceStateView::Unavailable { reason }) =
        source_state(&snapshot, &CapabilitySourceDescriptor::Python)
    else {
        panic!(
            "the Python source must be observably unavailable: {:?}",
            snapshot.capabilities.sources
        );
    };
    assert!(
        reason.contains("does not match its claimed identity"),
        "the reason is the storage revalidation diagnostic: {reason}"
    );
    let names = tool_names(&snapshot);
    assert!(names.contains(&"bash"), "native tools survive: {names:?}");
    assert!(
        !names.contains(&"fixture-tool"),
        "the corrupt ToolVersion never enters the committed registry: {names:?}"
    );
    // The corrupt publication is never mutated into looking valid.
    let persisted = std::fs::read_to_string(published.join("source/tool.py")).expect("persisted");
    assert!(persisted.contains("tampered"));
    prove_native_tool_executes(&runtime).await;
}

/// The fatal/isolated boundary: failures that prove the *core* runtime or
/// the *base* capability plane cannot be constructed stay fatal.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn core_and_base_plane_failures_remain_fatal() {
    // A session selecting a model the catalog does not declare: fatal.
    let root = tempfile::tempdir().expect("temp root");
    let bad_model = SESSION_JSON.replace("local/composed-model", "local/absent-model");
    let (_canonical, paths) = startup(&root, &bad_model);
    assert!(matches!(
        LocalConversationRuntime::compose(&paths, &dependencies())
            .await
            .expect_err("an unknown model fails startup"),
        LocalRuntimeError::Model(_)
    ));

    // A malformed Skill is a base capability-plane failure (the Skill plane
    // is Workspace content the runtime validated), not an optional external
    // source: fatal.
    let root = tempfile::tempdir().expect("temp root");
    let (_canonical, paths) = startup(&root, SESSION_JSON);
    let skill = paths.workspace.join(".agents/skills/broken");
    std::fs::create_dir_all(&skill).expect("skill directory");
    std::fs::write(skill.join("SKILL.md"), "not valid frontmatter at all").expect("SKILL.md");
    assert!(matches!(
        LocalConversationRuntime::compose(&paths, &dependencies())
            .await
            .expect_err("a malformed Skill fails the base capability plane"),
        LocalRuntimeError::Capability { .. }
    ));
}

/// MCP-level regressions driving the real self-spawned fixture servers.
#[cfg(all(unix, feature = "mcp-fixture"))]
mod mcp {
    use rustx::runtime_client::snapshot::{CapabilitySourceDescriptor, CapabilitySourceStateView};
    use rustx::tools::mcp::fixture::{self, FixtureServer, PROTOCOL_VERSIONS_ENV};

    use super::{
        attach_snapshot, dependencies, prove_native_tool_executes, source_state, startup,
        tool_names,
    };

    /// A session with two stdio MCP servers: `good` (the real fixture) and
    /// `bad` (a program that does not exist).
    fn session_with_two_servers(program: &str, args: &[String]) -> String {
        serde_json::json!({
            "conversationId": "conv-81-mcp",
            "agentId": "agent-81",
            "model": {"model": "local/composed-model"},
            "context": {"reserveTokens": 1024, "keepRecentTokens": 8192},
            "mcpServers": {
                "good": {
                    "type": "stdio",
                    "command": program,
                    "args": args,
                    "env": {fixture::FIXTURE_MODE_ENV: "1"},
                },
                "bad": {
                    "type": "stdio",
                    "command": "/nonexistent/rustx-issue81-absent-server",
                    "args": [],
                },
            },
        })
        .to_string()
    }

    /// One failed MCP server never suppresses a successful one (Issue #81):
    /// the bad server is observably unavailable, the good server is ready
    /// and its tools are committed and callable, and native tools still
    /// execute.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn one_mcp_server_failure_never_suppresses_a_successful_one() {
        if fixture::serve_if_fixture_mode(FixtureServer::from_env()).await {
            return;
        }
        let root = tempfile::tempdir().expect("temp root");
        let program = std::env::current_exe()
            .expect("test executable")
            .display()
            .to_string();
        let args = fixture::fixture_spawn_args(
            "mcp::one_mcp_server_failure_never_suppresses_a_successful_one",
        );
        let session = session_with_two_servers(&program, &args);
        let (_canonical, paths) = startup(&root, &session);

        let runtime = super::LocalConversationRuntime::compose(&paths, &dependencies())
            .await
            .expect("one failing MCP server must not terminate composition");
        let snapshot = attach_snapshot(&runtime);

        assert!(
            matches!(
                source_state(&snapshot, &CapabilitySourceDescriptor::Python),
                Some(CapabilitySourceStateView::Ready)
            ),
            "the Python plane is ready (no packages): {:?}",
            snapshot.capabilities.sources
        );
        let good = CapabilitySourceDescriptor::Mcp {
            server_id: rustx::runtime::identity::McpServerId::new("good"),
        };
        let bad = CapabilitySourceDescriptor::Mcp {
            server_id: rustx::runtime::identity::McpServerId::new("bad"),
        };
        assert!(
            matches!(
                source_state(&snapshot, &good),
                Some(CapabilitySourceStateView::Ready)
            ),
            "the successful server is ready: {:?}",
            snapshot.capabilities.sources
        );
        let Some(CapabilitySourceStateView::Unavailable { reason }) = source_state(&snapshot, &bad)
        else {
            panic!(
                "the failing server is observably unavailable: {:?}",
                snapshot.capabilities.sources
            );
        };
        assert!(
            !reason.is_empty(),
            "the unavailable state carries a diagnostic"
        );

        let names = tool_names(&snapshot);
        for expected in ["echo", "mutate", "slow"] {
            assert!(
                names.contains(&expected),
                "the healthy server's tools are committed: {names:?}"
            );
        }
        assert!(names.contains(&"bash"), "native tools survive: {names:?}");
        // The executable capability set changed, so the revision advanced
        // exactly once from the initial empty state.
        assert_eq!(snapshot.capabilities.revision.get(), 1);
        prove_native_tool_executes(&runtime).await;
    }

    /// No shared MCP protocol revision: the server becomes observably
    /// unavailable with the compatibility diagnostic and the runtime starts
    /// anyway (Issue #81, negotiation Case C at the composition level).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn no_shared_mcp_revision_is_unavailable_not_fatal() {
        if fixture::serve_if_fixture_mode(FixtureServer::from_env()).await {
            return;
        }
        let root = tempfile::tempdir().expect("temp root");
        let program = std::env::current_exe()
            .expect("test executable")
            .display()
            .to_string();
        let args =
            fixture::fixture_spawn_args("mcp::no_shared_mcp_revision_is_unavailable_not_fatal");
        let session = serde_json::json!({
            "conversationId": "conv-81-nooverlap",
            "agentId": "agent-81",
            "model": {"model": "local/composed-model"},
            "context": {"reserveTokens": 1024, "keepRecentTokens": 8192},
            "mcpServers": {
                "alien": {
                    "type": "stdio",
                    "command": program,
                    "args": args,
                    "env": {
                        fixture::FIXTURE_MODE_ENV: "1",
                        PROTOCOL_VERSIONS_ENV: "1999-01-01",
                    },
                },
            },
        })
        .to_string();
        let (_canonical, paths) = startup(&root, &session);

        let runtime = super::LocalConversationRuntime::compose(&paths, &dependencies())
            .await
            .expect("an incompatible MCP server must not terminate composition");
        let snapshot = attach_snapshot(&runtime);

        let alien = CapabilitySourceDescriptor::Mcp {
            server_id: rustx::runtime::identity::McpServerId::new("alien"),
        };
        let Some(CapabilitySourceStateView::Unavailable { reason }) =
            source_state(&snapshot, &alien)
        else {
            panic!(
                "the incompatible server is observably unavailable: {:?}",
                snapshot.capabilities.sources
            );
        };
        assert!(
            reason.contains("1999-01-01"),
            "the diagnostic names the server's revision set: {reason}"
        );
        let names = tool_names(&snapshot);
        assert!(names.contains(&"bash"), "native tools survive: {names:?}");
        assert!(
            !names.contains(&"echo"),
            "no tool of the incompatible server is committed: {names:?}"
        );
        prove_native_tool_executes(&runtime).await;
    }
}

/// Sends one request to a spawned `rustx` process and returns its
/// correlated response, skipping any notification lines that arrive first.
#[cfg(unix)]
async fn process_request(
    stdin: &mut tokio::process::ChildStdin,
    stdout: &mut tokio::io::BufReader<tokio::process::ChildStdout>,
    id: u64,
    build: impl FnOnce(
        rustx::runtime_client::RequestId,
    ) -> rustx::runtime_client::types::RuntimeClientRequest,
) -> rustx::runtime_client::types::RuntimeClientResponse {
    use rustx::runtime_client::types::{RuntimeClientProtocolEvent, RuntimeClientResponse};
    use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _};

    const LIVENESS: std::time::Duration = std::time::Duration::from_secs(120);
    let request = build(rustx::runtime_client::RequestId::new(id));
    let line = serde_json::to_string(&request).expect("serialize the request");
    tokio::time::timeout(LIVENESS, async {
        stdin.write_all(line.as_bytes()).await.expect("write");
        stdin.write_all(b"\n").await.expect("write newline");
        stdin.flush().await.expect("flush");
        loop {
            let mut record = String::new();
            let read = stdout.read_line(&mut record).await.expect("read");
            assert!(read > 0, "the process closed stdout before responding");
            if serde_json::from_str::<RuntimeClientProtocolEvent>(record.trim()).is_ok() {
                continue;
            }
            let response: RuntimeClientResponse = serde_json::from_str(record.trim())
                .unwrap_or_else(|error| {
                    panic!("stdout must carry protocol records only: {record:?} ({error})")
                });
            assert_eq!(response.id.get(), id, "responses correlate by request id");
            return response;
        }
    })
    .await
    .expect("the process must answer")
}

/// The original user-facing symptom (Issue #81): the TUI saw the runtime
/// close its transport output stream. This drives the **real `rustx`
/// process** with both a broken Python package and an unreachable MCP
/// server: the process must stay alive, Runtime Client `initialize` must
/// succeed, the initial snapshot must carry usable native capabilities and
/// the typed unavailable state, and the process must keep serving.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)] // one complete process-level regression
async fn the_process_stays_alive_and_serves_when_optional_capabilities_fail() {
    use std::process::Stdio;

    use rustx::runtime_client::types::{RuntimeClientRequest, RuntimeClientResult};
    use tokio::io::BufReader;

    const LIVENESS: std::time::Duration = std::time::Duration::from_secs(120);

    let root = tempfile::tempdir().expect("temp root");
    let workspace = root.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    // A broken Python package (Python source fails) ...
    let package = workspace.join(".agents/tools/broken-tool");
    std::fs::create_dir_all(&package).expect("package directory");
    std::fs::write(
        package.join("TOOL.toml"),
        "schema_version = 1\nname = \"other-name\"\ndescription = \"Broken\"\nentrypoint = \"tool:main\"\nexecution = \"foreground_only\"\nconcurrency = \"sequential\"\n",
    )
    .expect("manifest");
    std::fs::write(root.path().join("models.json"), MODELS_JSON).expect("models.json");
    // ... and an MCP server whose program does not exist.
    let session = serde_json::json!({
        "conversationId": "conv-81-process",
        "agentId": "agent-81",
        "model": {"model": "local/composed-model"},
        "context": {"reserveTokens": 1024, "keepRecentTokens": 8192},
        "mcpServers": {
            "exa": {
                "type": "stdio",
                "command": "/nonexistent/rustx-issue81-absent-server",
                "args": [],
            },
        },
    });
    std::fs::write(root.path().join("session.json"), session.to_string()).expect("session.json");

    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_rustx"));
    command
        .arg("--models")
        .arg(root.path().join("models.json"))
        .arg("--session")
        .arg(root.path().join("session.json"))
        .arg("--workspace")
        .arg(&workspace)
        .arg("--runtime-root")
        .arg(root.path().join("private"))
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("RUSTX_ISSUE81_KEY", "issue81-secret")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().expect("spawn the rustx binary");
    let mut stdin = child.stdin.take().expect("stdin is piped");
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout is piped"));

    // The exact operation the TUI failed on: initialize over the transport.
    let response = process_request(&mut stdin, &mut stdout, 1, |id| {
        RuntimeClientRequest::Initialize {
            id,
            protocol_version: RUNTIME_CLIENT_PROTOCOL_VERSION_V1,
        }
    })
    .await;
    let Some(RuntimeClientResult::Initialized { snapshot, .. }) = response.result else {
        panic!("initialize must succeed despite the optional failures: {response:?}");
    };
    let names = tool_names(&snapshot);
    for expected in ["read", "write", "bash"] {
        assert!(
            names.contains(&expected),
            "native tool {expected} must be in the initial snapshot: {names:?}"
        );
    }
    let Some(CapabilitySourceStateView::Unavailable { reason }) =
        source_state(&snapshot, &CapabilitySourceDescriptor::Python)
    else {
        panic!(
            "the Python failure is typed and observable, not an opaque EOF: {:?}",
            snapshot.capabilities.sources
        );
    };
    assert!(
        reason.contains("invalid Python tool package"),
        "real diagnostic: {reason}"
    );
    let exa = CapabilitySourceDescriptor::Mcp {
        server_id: rustx::runtime::identity::McpServerId::new("exa"),
    };
    assert!(
        matches!(
            source_state(&snapshot, &exa),
            Some(CapabilitySourceStateView::Unavailable { .. })
        ),
        "the unreachable MCP server is typed and observable: {:?}",
        snapshot.capabilities.sources
    );

    // The process keeps serving after the isolated failures.
    let response = process_request(&mut stdin, &mut stdout, 2, |id| {
        RuntimeClientRequest::ModelCatalogGet { id }
    })
    .await;
    assert!(
        matches!(
            response.result,
            Some(RuntimeClientResult::ModelCatalog { .. })
        ),
        "the runtime keeps serving: {response:?}"
    );

    drop(stdin);
    let status = tokio::time::timeout(LIVENESS, child.wait())
        .await
        .expect("the process must exit after transport EOF")
        .expect("wait");
    assert!(status.success(), "clean shutdown, not a crash: {status}");
}
