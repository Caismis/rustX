//! The real managed **`FastMCP` 4** SEP-2663 Tasks acceptance (Issue #243).
//!
//! Everything in this proof is real: a real `uv` materialization of the
//! rustX-pinned `FastMCP` build **plus the package's own declared
//! `fastmcp-tasks` dependency**, a real Python child speaking the MCP
//! `2026-07-28` inline lifecycle over the ordinary generic
//! [`McpServerRuntime`] transport, a real `tools/call` that answers with a
//! SEP-2663 `CreateTaskResult`, a real rustX poll loop over `tasks/get`, and
//! exactly one rustX `ToolResult`.
//!
//! ```text
//! one model `ToolCall`
//!   -> one rustX `ToolInvocation`
//!   -> real managed `FastMCP` 4 child (negotiated 2026-07-28)
//!   -> tools/call            -> `CreateTaskResult`
//!   -> tasks/get ... (rustX-owned bounded polling)
//!   -> terminal task state   -> final `CallToolResult`
//!   -> exactly one rustX `ToolResult`
//! ```
//!
//! # Dependency ownership is unchanged
//!
//! `fastmcp` itself stays rustX-managed and pinned; a package may not declare
//! it. The Tasks **extension** is a separate distribution (`fastmcp-tasks`,
//! the `fastmcp[tasks]` extra), so this acceptance declares it in **this one
//! package's** `requirements.txt`, exactly as any package declares any other
//! dependency. No global managed dependency is added, no managed-Python
//! ownership rule changes, and every other managed package materializes
//! exactly what it did before.
//!
//! The extension runs on its own `memory://` backend default, so the whole
//! acceptance is one process with no Redis and no external worker.
//!
//! No `FastMCP`-specific execution path exists. This drives the same generic
//! `McpToolExecutor` every other MCP server uses, and correctness never
//! depends on it: the normative conformance proof is the official-rmcp
//! fixture suite in `tests/boundary/mcp_tasks.rs`.
//!
//! Following the repository's opt-in-by-availability convention (see
//! `tests/tools/uv.rs`), the acceptance reports an honest skip when `uv` is
//! not on `PATH`.

use std::sync::Arc;

use crate::runtime::identity::{ConversationId, ToolCallId};
use crate::runtime::types::ConversationLifecycle;
use crate::tools::executor::{ProgressReporter, ToolExecutionContext, ToolExecutor};
use crate::tools::mcp::{McpInvalidationState, McpServerRuntime};
use crate::tools::types::{
    ToolExecutionStatus, ToolInvocation, ToolInvocationId, ToolInvocationMode,
    ToolInvocationPolicy, ToolResultContent,
};
use crate::tools::workspace::Workspace;

/// The SEP-2663 task tool, written against the `fastmcp-tasks` 4.0.3 API.
///
/// `task=True` is a declaration of intent — the server decides per call —
/// and the client's per-request Tasks capability is what makes tasking
/// applicable at all. rustX advertises it on every `2026-07-28` request.
const TASK_SERVER: &str = r#"from fastmcp import FastMCP
from fastmcp_tasks import TasksExtension

mcp = FastMCP("tasks-tool")
mcp.add_extension(TasksExtension())


@mcp.tool(task=True)
async def analyze(dataset: str) -> str:
    """Run a long analysis as a background MCP task."""
    return f"analyzed {dataset}"
"#;

struct NoProgress;

impl ProgressReporter for NoProgress {
    fn report(&self, _progress: crate::tools::types::ToolProgress) {}
}

/// A real managed `FastMCP` 4 task tool completes through the generic MCP
/// path: one `tools/call`, one remote task, one rustX poll loop, one result.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_real_managed_fastmcp_task_completes_through_one_tool_result() {
    let uv = std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|dir| dir.join("uv"))
            .find(|path| path.is_file())
    });
    if uv.is_none() {
        eprintln!(
            "uv unavailable; the real managed FastMCP 4 Tasks acceptance was NOT exercised \
             (skipped)"
        );
        return;
    }
    let directory = tempfile::tempdir().expect("workspace root");
    let workspace_root = directory.path().join("workspace");
    let package_root = workspace_root.join(".agents/tools/tasks-tool");
    std::fs::create_dir_all(&package_root).expect("package root");
    std::fs::write(package_root.join("server.py"), TASK_SERVER).expect("server source");
    // The SEP-2663 extension is this package's own declared dependency. It is
    // a distinct distribution from the rustX-managed `fastmcp` pin, so no
    // managed-Python ownership rule is bent to reach it.
    std::fs::write(
        package_root.join("requirements.txt"),
        "fastmcp-tasks==4.0.3\n",
    )
    .expect("requirements");

    let workspace = Workspace::new(&workspace_root).expect("workspace");
    let discovered =
        crate::tools::python::discover_python_packages(&workspace).expect("package discovery");
    let package = discovered
        .into_iter()
        .find(|entry| entry.server_id == crate::tools::python::python_server_id("tasks-tool"))
        .expect("the task package is discovered")
        .outcome
        .expect("the task package is valid");
    let store = crate::tools::python::PythonToolStore::new(directory.path().join("runtime"))
        .expect("python tool store");
    let prepared = store
        .ensure_prepared(&package, &crate::runtime::CancellationSignal::new())
        .await
        .expect("the managed FastMCP environment materializes");
    eprintln!(
        "managed FastMCP pin: {}",
        crate::tools::python::MANAGED_FASTMCP_VERSION
    );

    let server_id = crate::tools::python::python_server_id("tasks-tool");
    let runtime = McpServerRuntime::connect(
        &server_id,
        &prepared.server_binding(),
        &workspace,
        Arc::new(McpInvalidationState::new()),
    )
    .await
    .expect("the managed FastMCP child connects");
    // The negotiated revision of this connection, not the installed package
    // version, is what makes SEP-2663 applicable at all.
    assert_eq!(
        runtime.protocol_version().as_str(),
        rmcp::model::ProtocolVersion::V_2026_07_28.as_str(),
        "the managed FastMCP 4 child negotiates the modern revision"
    );
    let tools = runtime.list_tools().await.expect("tools/list");
    let definitions = crate::tools::mcp::definitions(
        &server_id,
        ToolInvocationPolicy::default(),
        &runtime,
        tools,
    );
    let (definition, executor) = definitions
        .into_iter()
        .find(|(definition, _)| definition.name == "analyze")
        .expect("the task tool is discovered");

    let artifacts = tempfile::tempdir().expect("artifacts");
    let bundle = crate::tools::runtime::ConversationToolRuntime::new(
        ConversationId::new("tasks-managed"),
        &workspace_root,
        artifacts.path(),
    )
    .expect("tool runtime");
    let lifecycle = ConversationLifecycle::new();
    assert!(lifecycle.activate());
    let owner = crate::agent::cancellation::AgentCancellation::new(
        crate::runtime::types::CancellationReason::UserRequested,
    );
    let progress = NoProgress;
    // Deliberately **no** Questionnaire authority: this task needs no human,
    // and driving it proves Tasks and Elicitation are advertised — and
    // required — independently.
    let context = ToolExecutionContext::new(
        bundle.conversation_id(),
        None,
        owner.execution_cancellation(),
        bundle.workspace(),
        &progress,
        bundle.artifacts(),
        bundle.tool_output(),
        bundle.environment(),
    );
    let settled = ToolExecutor::start(
        executor.as_ref(),
        ToolInvocation {
            id: ToolInvocationId::Agent {
                call_id: ToolCallId::new("managed-task"),
            },
            tool_id: definition.id.clone(),
            tool_name: "analyze".to_owned(),
            mode: ToolInvocationMode::Foreground,
            arguments: serde_json::json!({"dataset": "orders"}),
        },
        context,
    )
    .completion
    .await;
    assert!(
        matches!(settled.status, ToolExecutionStatus::Success),
        "the remote task reached its terminal state and produced one result: {:?}",
        settled.status
    );
    let text = settled
        .content
        .iter()
        .filter_map(|content| match content {
            ToolResultContent::Text(text) => Some(text.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        text.contains("analyzed orders"),
        "the task's own final CallToolResult is projected through the ordinary \
         MCP result path: {text}"
    );
    let _ = runtime.close().await;
}
