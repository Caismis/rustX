//! Boundary conformance for managed Python tool packages (Issue #174): the
//! synthesized MCP binding of a discovered package is exercised against the
//! real boundary — a real uv materialization and a real `FastMCP`
//! (rustX-pinned) stdio child.
//!
//! What is proven here, and the linearization point of each proof:
//!
//! - **One folder = one server, many tools**: a package exposing several
//!   `FastMCP` tools yields exactly one server identity (`python:<folder>`),
//!   one `tools/list` catalog, and calls through the canonical
//!   `ToolExecutor` path — the same path the Agent Loop uses. Provenance is
//!   `ToolOrigin::Mcp { server_id: "python:<folder>" }` with the canonical
//!   `mcp_tool_id` identity.
//! - **Sibling module imports**: `server.py` importing a sibling `common.py`
//!   works because the frozen `source/` directory is on `sys.path`.
//! - **Per-folder environment identity**: two folders prepare two distinct
//!   fingerprint-keyed state directories.
//! - **Process reuse**: the launch specification names the prepared venv
//!   interpreter, so one connected `McpServerRuntime` is one process for N
//!   `tools/call` operations. The instrumented point is server-side: the
//!   fixture writes its own PID at module import (one line per process
//!   start), and N calls must observe exactly one PID.
//! - **No uv during tools/call**: by construction the launch spec is the
//!   venv interpreter (never `uv run`), and behaviorally the call-phase
//!   child PATH is the fixed runtime-approved PATH — the uv binary used to
//!   prepare the environment is unreachable from inside a call.
//! - **Framing ownership**: the generic MCP framing owns protocol behavior
//!   — Python package authors must keep stdout reserved for the MCP wire,
//!   and stderr is the diagnostic/logging channel. Stderr diagnostics at
//!   import time and mid-call never participate in framing. (The framing's
//!   tolerance of non-protocol noise and its bounded `Invalid Request`
//!   answer to genuinely protocol-invalid output are proven by the generic
//!   MCP boundary suite in `mcp_runtime.rs`; the managed Python tests do
//!   not advertise arbitrary stdout logging as supported behavior.)
//! - **Freeze invariant**: editing the workspace source after a state was
//!   prepared neither mutates the frozen `source/` copy nor disturbs a
//!   running server of the old generation; the next preparation builds a
//!   new fingerprint-keyed state, and a committed activation serves the new
//!   implementation even when the FastMCP-derived schema is unchanged.
//!
//! Every test follows the uv-availability skip convention of `uv.rs`: with
//! no uv on PATH the real-materialization acceptance is not exercised.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rustx::runtime::identity::{ConversationId, McpServerId, ToolCallId};
use rustx::tools::mcp::{
    CanonicalMcpTool, McpInvalidationState, McpServerRuntime, McpTransportConfig,
};
use rustx::tools::python::{
    PreparedPythonPackage, PythonToolPackage, PythonToolStore, python_server_id,
};
use rustx::tools::types::{
    ToolExecutionResult, ToolExecutionStatus, ToolInvocation, ToolInvocationMode,
    ToolInvocationPolicy, ToolOrigin, ToolResultContent,
};
use rustx::tools::{Workspace, executor::ToolExecutionContext, executor::ToolExecutor};

/// Outer liveness guard for real-child operations. Its expiry is a harness
/// failure, never a verdict.
const LIVENESS: std::time::Duration = std::time::Duration::from_mins(2);

/// The uv binary on PATH, following the skip convention of `uv.rs`.
fn uv_binary() -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|dir| dir.join("uv"))
            .find(|path| path.is_file())
    })
}

macro_rules! require_uv {
    () => {
        match uv_binary() {
            Some(uv) => uv,
            None => {
                eprintln!("uv unavailable; managed FastMCP boundary not exercised");
                return;
            }
        }
    };
}

/// Writes one package folder below `.agents/tools/`.
fn write_package(workspace_root: &Path, name: &str, files: &[(&str, &str)]) {
    let root = workspace_root.join(".agents/tools").join(name);
    std::fs::create_dir_all(&root).expect("package directory");
    for (relative, content) in files {
        std::fs::write(root.join(relative), content).expect("package file");
    }
}

/// Discovers the one named package, asserting it validates.
fn discover(workspace_root: &Path, name: &str) -> PythonToolPackage {
    let workspace = Workspace::new(workspace_root).expect("workspace");
    let discovered = rustx::tools::python::discover_python_packages(&workspace).expect("discover");
    let entry = discovered
        .into_iter()
        .find(|entry| entry.server_id == python_server_id(name))
        .expect("the package is discovered");
    entry.outcome.expect("the package is valid")
}

async fn prepare(store: &PythonToolStore, package: &PythonToolPackage) -> PreparedPythonPackage {
    store
        .ensure_prepared(package, &rustx::runtime::CancellationSignal::new())
        .await
        .expect("prepare")
}

struct NoProgress;

impl rustx::tools::executor::ProgressReporter for NoProgress {
    fn report(&self, _progress: rustx::tools::types::ToolProgress) {}
}

/// One connected managed server plus the canonical executor boundary the
/// Agent Loop uses to call its tools.
struct ConnectedServer {
    runtime: Arc<McpServerRuntime>,
    server_id: McpServerId,
    tools: Vec<CanonicalMcpTool>,
    bundle: rustx::tools::runtime::ConversationToolRuntime,
    /// The artifact-root owner, declared LAST: fields drop in declaration
    /// order, so the runtime and bundle drop before the directory goes away.
    _artifacts: tempfile::TempDir,
}

impl ConnectedServer {
    /// Connects the prepared package's synthesized binding under the folder's
    /// server identity and fetches its catalog exactly once.
    async fn connect(
        prepared: &PreparedPythonPackage,
        folder: &str,
        workspace_root: &Path,
        conversation: &str,
    ) -> Self {
        let server_id = python_server_id(folder);
        let workspace = Workspace::new(workspace_root).expect("workspace");
        let runtime = McpServerRuntime::connect(
            &server_id,
            &prepared.server_binding(),
            &workspace,
            Arc::new(McpInvalidationState::new()),
        )
        .await
        .expect("the prepared managed server connects");
        let tools = runtime.list_tools().await.expect("tools/list");
        let artifacts = tempfile::tempdir().expect("artifacts");
        let bundle = rustx::tools::runtime::ConversationToolRuntime::new(
            ConversationId::new(conversation),
            workspace_root,
            artifacts.path(),
        )
        .expect("tool runtime");
        Self {
            runtime,
            server_id,
            tools,
            bundle,
            _artifacts: artifacts,
        }
    }

    fn definition(&self, name: &str) -> rustx::tools::types::ToolDefinition {
        let definitions = rustx::tools::mcp::definitions(
            &self.server_id,
            ToolInvocationPolicy::default(),
            &self.runtime,
            self.tools.clone(),
        );
        definitions
            .into_iter()
            .find(|(definition, _)| definition.name == name)
            .unwrap_or_else(|| panic!("the tool {name} must be discovered"))
            .0
    }

    /// Executes one discovered tool through the canonical executor boundary,
    /// the same path the Agent Loop uses.
    async fn call(&self, name: &str, arguments: serde_json::Value) -> ToolExecutionResult {
        let definitions = rustx::tools::mcp::definitions(
            &self.server_id,
            ToolInvocationPolicy::default(),
            &self.runtime,
            self.tools.clone(),
        );
        let (definition, executor) = definitions
            .into_iter()
            .find(|(definition, _)| definition.name == name)
            .unwrap_or_else(|| panic!("the tool {name} must be discovered"));
        ToolExecutor::start(
            executor.as_ref(),
            ToolInvocation {
                call_id: ToolCallId::new(format!("call-{name}")),
                tool_id: definition.id.clone(),
                tool_name: name.to_owned(),
                mode: ToolInvocationMode::Foreground,
                arguments,
            },
            ToolExecutionContext::new(
                self.bundle.conversation_id(),
                None,
                rustx::runtime::ExecutionCancellation::detached(
                    rustx::runtime::CancellationSignal::new(),
                    rustx::runtime::types::CancellationReason::UserRequested,
                ),
                self.bundle.workspace(),
                &NoProgress,
                self.bundle.artifacts(),
                self.bundle.tool_output(),
                self.bundle.environment(),
            ),
        )
        .completion
        .await
    }

    async fn close(self) {
        self.runtime.close().await.expect("physical settlement");
    }
}

/// The folded text of one successful tool result. `FastMCP` serves a `str`
/// return both as a text block and as the structured `{"result": ...}`
/// value; accept either carrying the same payload.
fn result_text(result: &ToolExecutionResult) -> String {
    assert!(
        matches!(result.status, ToolExecutionStatus::Success),
        "the call must succeed: {:?}",
        result.status
    );
    // FastMCP mirrors a `str` result into both a text block and a structured
    // `{"result": ...}` block; prefer the structured one to avoid double reads.
    for block in &result.content {
        if let ToolResultContent::Json { value } = block {
            return value["result"]
                .as_str()
                .unwrap_or_else(|| panic!("the structured result is a string: {value}"))
                .to_owned();
        }
    }
    result
        .content
        .iter()
        .map(|block| match block {
            ToolResultContent::Text(text) => text.text.clone(),
            other => panic!("managed text tools return text or structured blocks: {other:?}"),
        })
        .collect()
}

/// The `calc` package: two tools on one server, one sibling module import.
const CALC_SERVER: &str = "import common
from fastmcp import FastMCP

mcp = FastMCP('calc')


@mcp.tool
def add(a: int, b: int) -> str:
    return f'sum:{common.add(a, b)}'


@mcp.tool
def subtract(a: int, b: int) -> str:
    return f'difference:{common.subtract(a, b)}'
";

const CALC_COMMON: &str = "def add(a: int, b: int) -> int:
    return a + b


def subtract(a: int, b: int) -> int:
    return a - b
";

/// One folder exposing multiple `FastMCP` tools compiles into exactly one
/// server identity whose whole catalog is callable through the generic MCP
/// executor path, with canonical MCP provenance — and the sibling-module
/// import works because the frozen source directory is on `sys.path`.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_folder_serves_multiple_tools_through_one_server_identity() {
    require_uv!();
    let directory = tempfile::tempdir().expect("fixture root");
    let workspace_root = directory.path().join("workspace");
    std::fs::create_dir_all(&workspace_root).expect("workspace");
    write_package(
        &workspace_root,
        "calc",
        &[
            ("server.py", CALC_SERVER),
            ("common.py", CALC_COMMON),
            ("requirements.txt", "# none\n"),
        ],
    );

    // Discovery: exactly one synthesized server identity for the folder.
    let workspace = Workspace::new(&workspace_root).expect("workspace");
    let discovered = rustx::tools::python::discover_python_packages(&workspace).expect("discover");
    assert_eq!(discovered.len(), 1, "one folder is one package");
    assert_eq!(discovered[0].server_id, python_server_id("calc"));
    let package = discovered[0]
        .outcome
        .as_ref()
        .expect("valid package")
        .clone();

    let store = PythonToolStore::new(directory.path().join("runtime")).expect("store");
    let prepared = prepare(&store, &package).await;
    let server =
        ConnectedServer::connect(&prepared, "calc", &workspace_root, "conv-managed-calc").await;

    // One server, one frozen catalog carrying both tools.
    let mut names = server
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<Vec<_>>();
    names.sort_unstable();
    assert_eq!(names, ["add", "subtract"], "one tools/list, both tools");
    for name in ["add", "subtract"] {
        let definition = server.definition(name);
        assert_eq!(
            definition.origin,
            ToolOrigin::Mcp {
                server_id: python_server_id("calc")
            },
            "managed Python tools carry generic MCP provenance"
        );
        assert_eq!(
            definition.id.as_str(),
            rustx::tools::mcp::mcp_tool_id(&python_server_id("calc"), name),
            "the canonical MCP tool identity"
        );
    }

    // Both tools execute through the canonical ToolExecutor boundary; `add`
    // additionally proves the sibling-module import (`common.add`).
    let sum = server
        .call("add", serde_json::json!({"a": 40, "b": 2}))
        .await;
    assert_eq!(result_text(&sum), "sum:42");
    let difference = server
        .call("subtract", serde_json::json!({"a": 100, "b": 58}))
        .await;
    assert_eq!(result_text(&difference), "difference:42");
    server.close().await;
}

/// Two package folders prepare two distinct fingerprint-keyed state
/// directories, and each prepared state serves its own server.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_folders_prepare_distinct_environment_identities() {
    require_uv!();
    let directory = tempfile::tempdir().expect("fixture root");
    let workspace_root = directory.path().join("workspace");
    std::fs::create_dir_all(&workspace_root).expect("workspace");
    for (name, reply) in [("alpha", "alpha-reply"), ("beta", "beta-reply")] {
        write_package(
            &workspace_root,
            name,
            &[
                (
                    "server.py",
                    &format!(
                        "from fastmcp import FastMCP\n\nmcp = FastMCP({name:?})\n\n\n@mcp.tool\ndef whoami() -> str:\n    return {reply:?}\n"
                    ),
                ),
                ("requirements.txt", "# none\n"),
            ],
        );
    }

    let store = PythonToolStore::new(directory.path().join("runtime")).expect("store");
    let alpha = prepare(&store, &discover(&workspace_root, "alpha")).await;
    let beta = prepare(&store, &discover(&workspace_root, "beta")).await;
    assert_ne!(
        alpha.fingerprint, beta.fingerprint,
        "distinct package content is a distinct environment identity"
    );
    assert_ne!(alpha.state_dir, beta.state_dir);
    for prepared in [&alpha, &beta] {
        assert!(
            prepared
                .state_dir
                .starts_with(store.root().join("packages")),
            "the state lives below the store: {}",
            prepared.state_dir.display()
        );
        assert!(prepared.state_dir.join("venv/bin/python").is_file());
        assert!(prepared.state_dir.join("manifest.json").is_file());
    }

    let server =
        ConnectedServer::connect(&alpha, "alpha", &workspace_root, "conv-managed-alpha").await;
    assert_eq!(
        result_text(&server.call("whoami", serde_json::json!({})).await),
        "alpha-reply"
    );
    server.close().await;
    let server =
        ConnectedServer::connect(&beta, "beta", &workspace_root, "conv-managed-beta").await;
    assert_eq!(
        result_text(&server.call("whoami", serde_json::json!({})).await),
        "beta-reply"
    );
    server.close().await;
}

/// The `counter` server writes its own PID to a marker file at module
/// import: one line per process start, the linearization point of the
/// process-reuse proof. Its probe tools expose the call-phase child
/// environment.
fn counter_server(marker: &Path) -> String {
    format!(
        "import os
import shutil
from fastmcp import FastMCP

with open({marker:?}, 'a') as handle:
    handle.write(f'{{os.getpid()}}\\n')

mcp = FastMCP('counter')


@mcp.tool
def ping(text: str) -> str:
    return f'pong:{{text}}'


@mcp.tool
def child_path() -> str:
    return os.environ.get('PATH', '')


@mcp.tool
def uv_on_path() -> str:
    return shutil.which('uv') or 'uv-absent'
",
        marker = marker.display().to_string()
    )
}

/// One connected runtime is one process for N calls: the server-side PID
/// marker written once per process start observes exactly one PID across
/// all calls. No sleeps: the marker lines are the process-start count.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_connected_runtime_reuses_one_process_across_calls() {
    require_uv!();
    let directory = tempfile::tempdir().expect("fixture root");
    let workspace_root = directory.path().join("workspace");
    std::fs::create_dir_all(&workspace_root).expect("workspace");
    let marker = directory.path().join("startup-marker");
    write_package(
        &workspace_root,
        "counter",
        &[
            ("server.py", &counter_server(&marker)),
            ("requirements.txt", "# none\n"),
        ],
    );

    let store = PythonToolStore::new(directory.path().join("runtime")).expect("store");
    let package = discover(&workspace_root, "counter");
    let prepared = prepare(&store, &package).await;

    // By construction the launch spec is the prepared venv interpreter
    // running the FastMCP CLI module, never `uv run`: the program is the
    // venv python and the arguments are the fixed module invocation.
    let binding = prepared.server_binding();
    let McpTransportConfig::Stdio { program, args, .. } = &binding.transport else {
        panic!("the managed package binding is a stdio launch");
    };
    assert_eq!(
        Path::new(program)
            .file_name()
            .and_then(|name| name.to_str()),
        Some("python"),
        "the launch program is the prepared venv interpreter: {program}"
    );
    assert_eq!(
        args[..3],
        ["-m", "fastmcp.cli", "run"],
        "the interpreter runs the FastMCP CLI module directly: {args:?}"
    );
    assert_eq!(
        args[args.len() - 2..],
        ["--skip-env", "--no-banner"],
        "--skip-env pins the no-re-resolution launch inside the CLI: {args:?}"
    );

    let server = ConnectedServer::connect(
        &prepared,
        "counter",
        &workspace_root,
        "conv-managed-counter",
    )
    .await;
    for round in 0..3 {
        let result = server
            .call(
                "ping",
                serde_json::json!({"text": format!("round-{round}")}),
            )
            .await;
        assert_eq!(result_text(&result), format!("pong:round-{round}"));
    }
    let pids = std::fs::read_to_string(&marker).expect("startup marker");
    let distinct = pids.lines().collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        distinct.len(),
        1,
        "N tools/call operations are served by exactly one server process: {pids:?}"
    );

    // The call-phase child environment is the fixed runtime-approved PATH:
    // the venv's bin directory is never overlaid onto it, and the uv binary
    // that prepared the environment is unreachable from inside a call.
    let path = server.call("child_path", serde_json::json!({})).await;
    assert_eq!(result_text(&path), "/usr/local/bin:/usr/bin:/bin");
    let uv = uv_binary().expect("checked by require_uv");
    let uv_directory = uv.parent().expect("uv directory");
    if ["/usr/local/bin", "/usr/bin", "/bin"].contains(&uv_directory.to_string_lossy().as_ref()) {
        eprintln!("uv lives on the fixed runtime PATH; uv-unreachable probe not exercised");
    } else {
        let resolved = server.call("uv_on_path", serde_json::json!({})).await;
        assert_eq!(
            result_text(&resolved),
            "uv-absent",
            "the preparing uv binary is unreachable during tools/call"
        );
    }
    server.close().await;
}

/// The stdout/stderr ownership boundary: a package that writes diagnostics
/// to **stderr** — at import time and mid-call — never participates in MCP
/// framing; the wire stays intact. This is the supported diagnostic
/// channel: stdout is reserved for the MCP protocol, and the managed Python
/// contract does not advertise arbitrary stdout logging (the generic MCP
/// framing's tolerance of non-protocol noise is proven as an implementation
/// characteristic in `mcp_runtime.rs`, and its bounded `Invalid Request`
/// answer to genuinely protocol-invalid output is proven there too).
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stderr_diagnostics_do_not_participate_in_framing() {
    require_uv!();
    let directory = tempfile::tempdir().expect("fixture root");
    let workspace_root = directory.path().join("workspace");
    std::fs::create_dir_all(&workspace_root).expect("workspace");
    write_package(
        &workspace_root,
        "diagnostics",
        &[
            (
                "server.py",
                "import sys
from fastmcp import FastMCP

print('issue174 import-time diagnostic', file=sys.stderr)

mcp = FastMCP('diagnostics')


@mcp.tool
def stderr_log(text: str) -> str:
    print(f'issue174 diagnostic: {text}', file=sys.stderr)
    return f'logged:{text}'
",
            ),
            ("requirements.txt", "# none\n"),
        ],
    );

    let store = PythonToolStore::new(directory.path().join("runtime")).expect("store");
    let package = discover(&workspace_root, "diagnostics");
    let prepared = prepare(&store, &package).await;
    // The import-time stderr diagnostic precedes the handshake; stderr is a
    // separate stream that never participates in framing.
    let server = ConnectedServer::connect(
        &prepared,
        "diagnostics",
        &workspace_root,
        "conv-managed-diagnostics",
    )
    .await;
    let call = server
        .call("stderr_log", serde_json::json!({"text": "hello"}))
        .await;
    assert_eq!(result_text(&call), "logged:hello");
    // The connection survives the diagnostics: framing is intact afterwards.
    let again = server
        .call("stderr_log", serde_json::json!({"text": "again"}))
        .await;
    assert_eq!(result_text(&again), "logged:again");
    server.close().await;
}

/// A server that fails at startup (import error, or a missing `mcp`
/// entrypoint object) is a bounded, diagnosed MCP connect failure recorded
/// on the package's own synthesized source — never a hang, never fatal to
/// the capability plane. The diagnosis carries the server's stderr.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_server_failing_at_startup_is_isolated_and_diagnosed() {
    require_uv!();
    let directory = tempfile::tempdir().expect("fixture root");
    let workspace_root = directory.path().join("workspace");
    std::fs::create_dir_all(&workspace_root).expect("workspace");
    write_package(
        &workspace_root,
        "crasher",
        &[
            ("server.py", "raise RuntimeError('issue174-import-boom')\n"),
            ("requirements.txt", "# none\n"),
        ],
    );
    write_package(
        &workspace_root,
        "exportless",
        &[
            (
                "server.py",
                "from fastmcp import FastMCP\n\nnot_the_entrypoint = FastMCP('exportless')\n",
            ),
            ("requirements.txt", "# none\n"),
        ],
    );

    let coordinator = rustx::capabilities::CapabilityCoordinator::new(
        rustx::capabilities::CapabilityCoordinatorConfig {
            conversation_id: ConversationId::new("conv-managed-startup"),
            workspace: Workspace::new(&workspace_root).expect("workspace"),
            base_tool_registry: Arc::new(rustx::tools::executor::ToolRegistry::new()),
            tool_activation: rustx::capabilities::ToolActivationPolicy::default(),
            // Keep this fixture independent of the developer's HOME.
            skill_discovery: rustx::skills::SkillDiscoveryConfig {
                automatic_roots: vec![workspace_root.join(".agents/skills")],
                explicit_paths: Vec::new(),
            },
            mcp_servers: std::collections::BTreeMap::new(),
            base_environment: rustx::tools::environment::ToolEnvironment::new(),
            environment_store_root: directory.path().join("skill-env"),
        },
    )
    .expect("coordinator");

    // Both packages prepare (the uv build succeeds — preparation never
    // validate-launches) and then fail the generic MCP connect. The
    // liveness guard makes a hang a harness failure, not a verdict.
    let candidate = tokio::time::timeout(LIVENESS, coordinator.prepare_candidate())
        .await
        .expect("startup failure must not hang the capability preparation")
        .expect("isolated startup failures must not fail the candidate");

    let crasher = rustx::capabilities::CapabilitySourceId::Mcp(python_server_id("crasher"));
    let Some(rustx::capabilities::CapabilitySourceState::Unavailable { reason }) =
        candidate.availability().get(&crasher)
    else {
        panic!(
            "the failing server lands on its own synthesized source: {:?}",
            candidate.availability()
        );
    };
    assert!(
        reason.contains("MCP discovery failed"),
        "the failure is a structural MCP connect error: {reason}"
    );
    assert!(
        reason.contains("server stderr:") && reason.contains("issue174-import-boom"),
        "the diagnosis carries the server's stderr: {reason}"
    );

    let exportless = rustx::capabilities::CapabilitySourceId::Mcp(python_server_id("exportless"));
    let Some(rustx::capabilities::CapabilitySourceState::Unavailable { reason }) =
        candidate.availability().get(&exportless)
    else {
        panic!(
            "the export-less server lands on its own synthesized source: {:?}",
            candidate.availability()
        );
    };
    assert!(
        reason.contains("MCP discovery failed") && reason.contains("server stderr:"),
        "a missing `mcp` entrypoint is diagnosed through the server's stderr: {reason}"
    );
}

/// Executes one managed tool through a committed capability snapshot's
/// canonical registry — the same path an admitted attempt uses — proving
/// which executable generation the snapshot actually serves.
async fn call_through_snapshot(
    workspace_root: &Path,
    server_id: &McpServerId,
    snapshot: &rustx::capabilities::CapabilitySnapshot,
    name: &str,
    arguments: serde_json::Value,
) -> ToolExecutionResult {
    let registry = snapshot.tool_registry();
    let tool_id =
        rustx::runtime::identity::ToolId::new(rustx::tools::mcp::mcp_tool_id(server_id, name));
    assert!(
        registry
            .definitions()
            .iter()
            .any(|definition| definition.name == name),
        "the snapshot must publish {name}"
    );
    let executor = registry.executor(&tool_id);
    let artifacts = tempfile::tempdir().expect("artifacts");
    let bundle = rustx::tools::runtime::ConversationToolRuntime::new(
        ConversationId::new("conv-managed-commit"),
        workspace_root,
        artifacts.path(),
    )
    .expect("tool runtime");
    ToolExecutor::start(
        executor.as_ref(),
        ToolInvocation {
            call_id: ToolCallId::new(format!("call-{name}")),
            tool_id,
            tool_name: name.to_owned(),
            mode: ToolInvocationMode::Foreground,
            arguments,
        },
        ToolExecutionContext::new(
            bundle.conversation_id(),
            None,
            rustx::runtime::ExecutionCancellation::detached(
                rustx::runtime::CancellationSignal::new(),
                rustx::runtime::types::CancellationReason::UserRequested,
            ),
            bundle.workspace(),
            &NoProgress,
            bundle.artifacts(),
            bundle.tool_output(),
            bundle.environment(),
        ),
    )
    .completion
    .await
}

/// The freeze invariant end to end through the **coordinator commit path**
/// (Blocker 2): a source implementation change with an unchanged
/// FastMCP-derived `tools/list` schema is committed as a new publication
/// (the effective binding changed — new fingerprint-keyed state, new launch
/// program), the revision advances, and a future activation executes the
/// new implementation; the already-admitted old execution keeps the old
/// generation and the frozen old server keeps serving the old code.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_source_implementation_change_is_observed_through_the_coordinator_commit_path() {
    require_uv!();
    let directory = tempfile::tempdir().expect("fixture root");
    let workspace_root = directory.path().join("workspace");
    std::fs::create_dir_all(&workspace_root).expect("workspace");
    let server_v1 = "from fastmcp import FastMCP

mcp = FastMCP('calc')


@mcp.tool
def add(a: int, b: int) -> str:
    return f'sum:{a + b}'
";
    write_package(
        &workspace_root,
        "calc",
        &[("server.py", server_v1), ("requirements.txt", "# none\n")],
    );

    let coordinator = rustx::capabilities::CapabilityCoordinator::new(
        rustx::capabilities::CapabilityCoordinatorConfig {
            conversation_id: ConversationId::new("conv-managed-freeze"),
            workspace: Workspace::new(&workspace_root).expect("workspace"),
            base_tool_registry: Arc::new(rustx::tools::executor::ToolRegistry::new()),
            tool_activation: rustx::capabilities::ToolActivationPolicy::default(),
            // Keep this fixture independent of the developer's HOME.
            skill_discovery: rustx::skills::SkillDiscoveryConfig {
                automatic_roots: vec![workspace_root.join(".agents/skills")],
                explicit_paths: Vec::new(),
            },
            mcp_servers: std::collections::BTreeMap::new(),
            base_environment: rustx::tools::environment::ToolEnvironment::new(),
            environment_store_root: directory.path().join("skill-env"),
        },
    )
    .expect("coordinator");
    let server_id = python_server_id("calc");

    // Commit generation v1 and admit an execution against it.
    let candidate_v1 = tokio::time::timeout(LIVENESS, coordinator.prepare_candidate())
        .await
        .expect("prepare v1 must not hang")
        .expect("prepare v1");
    let v1_snapshot = coordinator.commit(candidate_v1).expect("commit v1");
    assert_eq!(v1_snapshot.revision().get(), 1);
    let old_lease = coordinator.acquire_attempt_lease();
    assert_eq!(old_lease.revision().get(), 1);
    let v1_result = call_through_snapshot(
        &workspace_root,
        &server_id,
        old_lease.snapshot(),
        "add",
        serde_json::json!({"a": 40, "b": 2}),
    )
    .await;
    assert_eq!(result_text(&v1_result), "sum:42");
    // The admitted execution settles before the reload (a reload is refused
    // while an attempt lease is active).
    drop(old_lease);

    // The workspace edit changes only the implementation: the FastMCP-derived
    // tool name, description, and input schema are unchanged.
    let server_v2 = server_v1.replace("sum:{a + b}", "changed:{a - b}");
    std::fs::write(
        workspace_root.join(".agents/tools/calc/server.py"),
        &server_v2,
    )
    .expect("edit the live source");

    // The next activation prepares and commits generation v2. The effective
    // binding changed (a new fingerprint-keyed state directory, hence a new
    // launch program), so this is a real publication even though the
    // model-facing schema is byte-identical.
    let candidate_v2 = tokio::time::timeout(LIVENESS, coordinator.prepare_candidate())
        .await
        .expect("prepare v2 must not hang")
        .expect("prepare v2");
    let v2_snapshot = coordinator.commit(candidate_v2).expect("commit v2");
    assert_eq!(
        v2_snapshot.tool_registry().definitions(),
        v1_snapshot.tool_registry().definitions(),
        "the model-facing tools/list contract is unchanged"
    );
    assert_eq!(
        v2_snapshot.revision().get(),
        2,
        "an implementation change with unchanged schema is a new publication"
    );

    // A future activation executes the new implementation.
    let future_lease = coordinator.acquire_attempt_lease();
    assert_eq!(future_lease.revision().get(), 2);
    let v2_result = call_through_snapshot(
        &workspace_root,
        &server_id,
        future_lease.snapshot(),
        "add",
        serde_json::json!({"a": 40, "b": 2}),
    )
    .await;
    assert_eq!(result_text(&v2_result), "changed:38");
    drop(future_lease);

    // The frozen source copy of generation v1 is byte-identical: filesystem
    // edits never mutate a prepared state.
    let states = std::fs::read_dir(directory.path().join("skill-env/python-tools/packages"))
        .expect("packages")
        .map(|entry| entry.expect("entry").path())
        .filter(|path| path.join("source/server.py").is_file())
        .collect::<Vec<_>>();
    assert_eq!(states.len(), 2, "two fingerprint-keyed prepared states");
    let frozen = states
        .iter()
        .map(|state| std::fs::read_to_string(state.join("source/server.py")).expect("frozen"))
        .collect::<Vec<_>>();
    assert!(
        frozen.contains(&server_v1.to_owned()) && frozen.contains(&server_v2.clone()),
        "each prepared state froze its own source generation: {frozen:?}"
    );
}
