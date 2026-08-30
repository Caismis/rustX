//! Issue #162: the `execution` intrinsic — deterministic regressions.
//!
//! These tests prove the unified model-facing asynchronous execution control
//! plane:
//!
//! - creation results return typed execution handles (`kind` + `id`) for
//!   both detached tool executions and subagent children;
//! - `execution(status|cancel)` routes a tool target **only** to
//!   `ConversationBackgroundRegistry` and a subagent target **only** to
//!   `SubagentRegistry`, returning the authoritative domain snapshot;
//! - a mismatched `kind`/id pair never falls through to another registry
//!   and is never auto-guessed from the id string;
//! - unknown and cross-conversation ids fail deterministically and remain
//!   indistinguishable from unknown ids at the owning domain boundary;
//! - a subagent's terminal answer still arrives exactly once through the
//!   canonical inbound message path, and `execution(status)` is observation,
//!   not a second result-delivery channel;
//! - `background_task` is no longer registered or model-visible.
//!
//! All concurrency is driven by explicit gates (watch channels, the
//! registry's settlement waits, staged children); no sleep proves any
//! invariant.

use super::{common, support};

use std::sync::Arc;

use rustx::durable::ConversationStore;
use rustx::runtime::CancellationSignal;
use rustx::runtime::identity::{AgentId, ConversationId, SubagentId, ToolCallId, ToolExecutionId};
use rustx::runtime::subagent::{
    ResolvedSubagentSpec, SubagentName, SubagentRegistry, SubagentRegistryConfig,
    SubagentSpawnPlan, SubagentStartOutcome, SubagentStartSpec, SubagentState,
    SubagentWorkspaceManager,
};
use rustx::runtime::types::{CancellationReason, SystemClock};
use rustx::tools::background::{BackgroundDispatchOutcome, BackgroundLifecycle};
use rustx::tools::executor::ToolRegistry;
use rustx::tools::types::{
    ToolExecutionStatus, ToolInvocation, ToolInvocationMode, ToolResultContent,
};

use crate::runtime::subagent::ipc::{ChildFrame, ChildResultStatus, ResultFrame};
use crate::runtime::subagent::process::StagedChild;

/// The conversation-owned subagent plane of one deterministic test: a real
/// in-memory durable store, a real registry, and a staging seam for
/// scripted child processes.
struct SubagentPlane {
    registry: SubagentRegistry,
    store: Arc<rustx::durable::SqliteConversationStore>,
    conversation_id: ConversationId,
    runtime_root: std::path::PathBuf,
    /// The temporary directory owner, declared LAST: struct fields drop in
    /// declaration order, so the registry and every handle obtained from it
    /// drop before the directory is removed.
    #[allow(clippy::used_underscore_binding)]
    _dir: tempfile::TempDir,
}

/// A scripted child: one trivial real process (kill/reap semantics) and the
/// test-held end of the control channel (protocol semantics).
struct ScriptedChild {
    peer: tokio::net::UnixStream,
}

fn subagent_plane() -> SubagentPlane {
    let dir = tempfile::tempdir().expect("temp dir");
    let workspace = dir.path().join("workspace");
    let runtime_root = dir.path().join("runtime");
    std::fs::create_dir_all(&workspace).expect("workspace");
    std::fs::create_dir_all(&runtime_root).expect("runtime root");
    let conversation_id = ConversationId::new("conv-162");
    let store = Arc::new(
        rustx::durable::SqliteConversationStore::in_memory(conversation_id.clone())
            .expect("in-memory store"),
    );
    let mailbox = rustx::runtime::inbound::ConversationInboundMailbox::over_store(store.clone());
    let registry = SubagentRegistry::new(SubagentRegistryConfig {
        conversation_id: conversation_id.clone(),
        agent_id: AgentId::new("agent-parent-162"),
        mailbox,
        clock: Arc::new(SystemClock),
        spawn: SubagentSpawnPlan {
            program: std::path::PathBuf::from("/nonexistent/rustx"),
            runtime_root: runtime_root.clone(),
            model_timeout_policy: rustx::model::ModelTimeoutPolicy::default(),
            agent_status: rustx::context::AgentStatusConfig::default(),
            context: rustx::context::SessionContextPolicy {
                reserve_tokens: 0,
                keep_recent_tokens: 0,
                summary_output_cap: None,
            },
        },
        workspace: SubagentWorkspaceManager::new(&workspace, &runtime_root),
        max_active: 4,
    });
    SubagentPlane {
        registry,
        store,
        conversation_id,
        runtime_root,
        _dir: dir,
    }
}

/// A Builtin-only frozen specification: the registry owns live child
/// lifecycle, so resolution is already complete before it is involved.
fn resolved(agent: &str) -> ResolvedSubagentSpec {
    ResolvedSubagentSpec {
        agent: SubagentName::parse(agent).expect("canonical name"),
        definition_digest: serde_json::from_value(serde_json::json!(
            "sha256:0000000000000000000000000000000000000000000000000000000000000000"
        ))
        .expect("digest"),
        workspace_policy: rustx::runtime::subagent::SubagentWorkspacePolicy::SharedWorkspace,
        instructions: "instructions".to_owned(),
        model: crate::model::frozen::test_frozen_model_spec(
            serde_json::from_value(serde_json::json!("local/model")).expect("model ref"),
        ),
        tools: Vec::new(),
        skills: Vec::new(),
        project_instructions: Vec::new(),
        materialization:
            crate::runtime::subagent::resolver::ResolvedSubagentMaterialization::default(),
    }
}

fn spec(task: &str) -> SubagentStartSpec {
    SubagentStartSpec {
        resolved: resolved("explore"),
        task: task.to_owned(),
        context: None,
        tool_call_id: ToolCallId::new("call-162"),
    }
}

/// Stages a scripted child whose process exits immediately; the test drives
/// the protocol over `peer`.
fn stage_exit0(plane: &SubagentPlane) -> ScriptedChild {
    let (driver_end, test_end) = tokio::net::UnixStream::pair().expect("pair");
    let child = tokio::process::Command::new("sh")
        .arg("-c")
        .arg("true")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(0)
        .spawn()
        .expect("scripted child process");
    let pid = child.id().expect("scripted child pid");
    let child_runtime_root = plane.runtime_root.join(format!("test-child-{pid}"));
    std::fs::create_dir_all(&child_runtime_root).expect("child runtime root");
    let staged = StagedChild::for_test(child, driver_end, child_runtime_root);
    plane.registry.push_staged_override(staged);
    ScriptedChild { peer: test_end }
}

impl ScriptedChild {
    /// Awaits the delegated task and answers with one terminal result.
    async fn complete(mut self, status: ChildResultStatus, content: Option<&str>) {
        let frame = crate::runtime::subagent::ipc::read_parent_frame(&mut self.peer)
            .await
            .expect("delegate frame");
        assert!(
            matches!(
                frame,
                Some(crate::runtime::subagent::ipc::ParentFrame::Delegate(_))
            ),
            "the committed child is delegated first"
        );
        crate::runtime::subagent::ipc::write_child_frame(
            &mut self.peer,
            &ChildFrame::Result(ResultFrame {
                status,
                content: content.map(str::to_owned),
                diagnostic: None,
            }),
        )
        .await
        .expect("result frame");
    }
}

async fn start_subagent(
    plane: &SubagentPlane,
    task: &str,
) -> rustx::runtime::subagent::SubagentAccepted {
    let prepared = plane
        .registry
        .prepare(&spec(task), &CancellationSignal::new())
        .await
        .expect("prepared");
    match plane
        .registry
        .commit(prepared, &CancellationSignal::new())
        .await
        .expect("commit")
    {
        SubagentStartOutcome::Accepted(accepted) => accepted,
        SubagentStartOutcome::RolledBack => panic!("no cancellation was requested"),
    }
}

/// A background invocation of `tool` through the conversation's tool
/// runtime, mirroring `m5_background`'s fixture.
fn background_invocation(tool: &str) -> ToolInvocation {
    ToolInvocation {
        call_id: ToolCallId::new("call-162-bg"),
        tool_id: rustx::runtime::identity::ToolId::new(format!("tool-{tool}")),
        tool_name: tool.to_owned(),
        mode: ToolInvocationMode::Background,
        arguments: serde_json::json!({}),
    }
}

/// A registry that registers exactly the `execution` intrinsic over the
/// given domain registries, plus the conversation tool runtime whose
/// background registry backs the tool kind.
struct ExecutionFixture {
    /// The temporary directory owner, declared LAST: struct fields drop in
    /// declaration order, so the runtime drops before the directory.
    _dir: tempfile::TempDir,
    runtime: rustx::tools::runtime::ConversationToolRuntime,
    registry: ToolRegistry,
}

fn execution_fixture(subagents: Option<SubagentRegistry>) -> ExecutionFixture {
    let dir = tempfile::tempdir().expect("temp dir");
    let workspace_root = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace_root).expect("workspace directory");
    let artifacts = dir.path().join("artifacts");
    std::fs::create_dir_all(&artifacts).expect("artifact directory");
    let conversation_id = ConversationId::new("conv-162-execution");
    let store = Arc::new(
        rustx::durable::SqliteConversationStore::open(
            conversation_id.clone(),
            &artifacts.join("conversation.sqlite"),
        )
        .expect("durable store"),
    );
    let runtime = rustx::tools::runtime::ConversationToolRuntime::from_config(
        conversation_id,
        rustx::tools::runtime::ConversationRuntimeConfig {
            durable_binding: Some(rustx::durable::ConversationStoreBinding::new(store.clone())),
            ..rustx::tools::runtime::ConversationRuntimeConfig::new(&workspace_root, &artifacts)
        },
    )
    .expect("tool runtime");
    let registration =
        crate::tools::native::execution::registration(runtime.background().clone(), subagents);
    let mut registry = ToolRegistry::new();
    registry
        .register_with_activation_metadata(
            registration.definition,
            registration.executor,
            registration.normalizer,
            false,
        )
        .expect("execution registers");
    ExecutionFixture {
        _dir: dir,
        runtime,
        registry,
    }
}

/// Executes one `execution` invocation against the fixture's registry,
/// through the real preflight path.
async fn run_execution(
    fixture: &ExecutionFixture,
    arguments: serde_json::Value,
) -> rustx::tools::types::ToolExecutionResult {
    use rustx::tools::executor::{PreflightOutcome, ToolExecutionContext};
    use rustx::tools::types::ToolCall;
    let definition = fixture
        .registry
        .definitions()
        .into_iter()
        .find(|definition| definition.name == "execution")
        .expect("execution registered");
    let call = ToolCall {
        id: ToolCallId::new("call-162-execution"),
        tool_id: definition.id,
        name: "execution".to_owned(),
        arguments,
    };
    let outcome = fixture.registry.preflight(&call).expect("preflight");
    let PreflightOutcome::Ready(prepared) = outcome else {
        panic!("execution calls preflight as ready");
    };
    let executor = fixture.registry.executor(&prepared.invocation.tool_id);
    let reporter = common::NoopProgress;
    let context = ToolExecutionContext::new(
        fixture.runtime.conversation_id(),
        None,
        rustx::runtime::ExecutionCancellation::detached(
            CancellationSignal::new(),
            CancellationReason::UserRequested,
        ),
        fixture.runtime.workspace(),
        &reporter,
        fixture.runtime.artifacts(),
        fixture.runtime.tool_output(),
        fixture.runtime.environment(),
    );
    executor.execute(prepared.invocation, context).await
}

/// The single JSON content block of a successful structured result.
fn json_content(result: &rustx::tools::types::ToolExecutionResult) -> serde_json::Value {
    assert_eq!(result.status, ToolExecutionStatus::Success);
    match &result.content[0] {
        ToolResultContent::Json { value } => value.clone(),
        other => panic!("expected JSON, got {other:?}"),
    }
}

/// The failure message of a failed result.
fn failure_message(result: &rustx::tools::types::ToolExecutionResult) -> String {
    match &result.status {
        ToolExecutionStatus::Failed { error } => error.clone(),
        other => panic!("expected failure, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Creation results
// ---------------------------------------------------------------------------

/// A background tool dispatch returns a typed execution handle:
/// `{ "execution": { "kind": "tool", "id": "exec_1" }, "state": "starting", ... }`.
#[tokio::test]
async fn background_dispatch_returns_a_typed_tool_execution_handle() {
    let fixture = common::native_fixture();
    let (tool, _release) = support::fake::FakeTool::parking(
        common::tool_policies(
            "bash",
            "tool-bash",
            rustx::tools::types::ToolExecutionPolicy::ModelSelectable,
            rustx::tools::types::ToolConcurrencyPolicy::Sequential,
        ),
        support::fake::success_result("done"),
    );
    let executor: Arc<dyn rustx::tools::executor::ToolExecutor> = Arc::new(tool);
    let prepared = fixture
        .runtime
        .background()
        .prepare_dispatch(
            &background_invocation("bash"),
            &executor,
            rustx::tools::environment::ToolEnvironment::new(),
        )
        .expect("prepare");
    let outcome = fixture
        .runtime
        .background()
        .commit_dispatch(prepared, &CancellationSignal::new())
        .expect("dispatch commits");
    let BackgroundDispatchOutcome::Accepted { result, .. } = outcome else {
        panic!("accepted");
    };
    let accepted = json_content(&result);
    assert_eq!(
        accepted["execution"],
        serde_json::json!({"kind": "tool", "id": "exec_1"}),
        "the creation result returns the typed execution handle"
    );
    assert_eq!(accepted["state"], "starting");
    assert_eq!(accepted["tool"], "bash");
    assert!(
        accepted.get("execution_id").is_none(),
        "the bare id is replaced by the tagged handle"
    );
    assert!(
        accepted["output_path"]
            .as_str()
            .expect("output path")
            .ends_with("tasks/exec_1.output"),
        "the output locator still accompanies the handle"
    );
}

/// A subagent start returns a typed execution handle:
/// `{ "execution": { "kind": "subagent", "id": "conversation-1-subagent-1" }, "state": "running", ... }`.
#[tokio::test]
async fn subagent_start_returns_a_typed_subagent_execution_handle() {
    let plane = subagent_plane();
    let _child = stage_exit0(&plane);
    let accepted = start_subagent(&plane, "inspect the tool plane").await;
    assert_eq!(
        accepted.subagent_id,
        SubagentId::for_conversation(&plane.conversation_id, 1)
    );
    // The exact result-shaping function the `subagent` intrinsic executor
    // runs.
    let result = crate::tools::native::subagent::accepted_result(accepted);
    let value = json_content(&result);
    assert_eq!(
        value["execution"],
        serde_json::json!({"kind": "subagent", "id": "conv-162-subagent-1"}),
        "the creation result returns the typed execution handle"
    );
    assert_eq!(value["state"], "running");
    assert!(
        value.get("subagent_id").is_none(),
        "the bare subagent id is replaced by the tagged handle"
    );
}

// ---------------------------------------------------------------------------
// Tool routing
// ---------------------------------------------------------------------------

/// `execution(status)` for a tool target routes only to
/// `ConversationBackgroundRegistry` and returns its authoritative snapshot.
#[tokio::test]
async fn execution_status_routes_tool_targets_to_the_background_registry() {
    let fixture = common::native_fixture();
    let (executor, mut started, _release) = controlled_parking();
    let registry = fixture.runtime.background().clone();
    let prepared = registry
        .prepare_dispatch(
            &background_invocation("bash"),
            &(Arc::new(executor) as Arc<dyn rustx::tools::executor::ToolExecutor>),
            rustx::tools::environment::ToolEnvironment::new(),
        )
        .expect("prepare");
    let outcome = registry
        .commit_dispatch(prepared, &CancellationSignal::new())
        .expect("dispatch commits");
    let BackgroundDispatchOutcome::Accepted { .. } = outcome else {
        panic!("accepted");
    };
    started
        .wait_for(|is_started| *is_started)
        .await
        .expect("runner started");

    let result = common::run_tool(
        &fixture,
        "execution",
        serde_json::json!({"action": "status", "target": {"kind": "tool", "id": "exec_1"}}),
    )
    .await;
    let snapshot = json_content(&result);
    assert_eq!(snapshot["kind"], "tool");
    assert_eq!(snapshot["execution_id"], "exec_1");
    assert_eq!(snapshot["tool_name"], "bash");
    assert_eq!(snapshot["state"], "running");
    // The response is the authoritative registry snapshot, not a cached or
    // duplicate projection.
    assert_eq!(
        registry
            .snapshot(&ToolExecutionId::new("exec_1"))
            .expect("snapshot")
            .state,
        BackgroundLifecycle::Running
    );
}

/// `execution(cancel)` for a tool target routes only to
/// `ConversationBackgroundRegistry`: cancellation intent commits there and
/// the intrinsic never owns cancellation itself.
#[tokio::test]
async fn execution_cancel_routes_tool_targets_to_the_background_registry() {
    let fixture = common::native_fixture();
    let (executor, mut started, _release) = controlled_parking();
    let registry = fixture.runtime.background().clone();
    let prepared = registry
        .prepare_dispatch(
            &background_invocation("bash"),
            &(Arc::new(executor) as Arc<dyn rustx::tools::executor::ToolExecutor>),
            rustx::tools::environment::ToolEnvironment::new(),
        )
        .expect("prepare");
    let outcome = registry
        .commit_dispatch(prepared, &CancellationSignal::new())
        .expect("dispatch commits");
    let BackgroundDispatchOutcome::Accepted { .. } = outcome else {
        panic!("accepted");
    };
    started
        .wait_for(|is_started| *is_started)
        .await
        .expect("runner started");

    let cancelled = common::run_tool(
        &fixture,
        "execution",
        serde_json::json!({"action": "cancel", "target": {"kind": "tool", "id": "exec_1"}}),
    )
    .await;
    let snapshot = json_content(&cancelled);
    assert_eq!(snapshot["state"], "cancelling");
    assert_eq!(
        registry
            .snapshot(&ToolExecutionId::new("exec_1"))
            .expect("snapshot")
            .state,
        BackgroundLifecycle::Cancelling,
        "the authoritative registry transitioned, not a shadow copy"
    );

    // Repeated cancel is idempotent through the registry.
    let again = common::run_tool(
        &fixture,
        "execution",
        serde_json::json!({"action": "cancel", "target": {"kind": "tool", "id": "exec_1"}}),
    )
    .await;
    assert_eq!(again.status, ToolExecutionStatus::Success);
    let execution_id = ToolExecutionId::new("exec_1");
    for _ in 0..400 {
        let snapshot = registry.snapshot(&execution_id).expect("snapshot");
        if snapshot.state == BackgroundLifecycle::Cancelled {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(
        registry.snapshot(&execution_id).expect("snapshot").state,
        BackgroundLifecycle::Cancelled,
        "the registry's own settlement reached the terminal state"
    );
}

// ---------------------------------------------------------------------------
// Subagent routing
// ---------------------------------------------------------------------------

/// `execution(status)` for a subagent target routes only to
/// `SubagentRegistry` and returns its authoritative snapshot.
#[tokio::test]
async fn execution_status_routes_subagent_targets_to_the_subagent_registry() {
    let plane = subagent_plane();
    let _child = stage_exit0(&plane);
    let accepted = start_subagent(&plane, "inspect the tool plane").await;
    let fixture = execution_fixture(Some(plane.registry.clone()));

    let result = run_execution(
        &fixture,
        serde_json::json!({
            "action": "status",
            "target": {"kind": "subagent", "id": accepted.subagent_id.to_string()},
        }),
    )
    .await;
    let snapshot = json_content(&result);
    assert_eq!(snapshot["kind"], "subagent");
    assert_eq!(snapshot["subagent_id"], accepted.subagent_id.to_string());
    assert_eq!(snapshot["agent"], "explore");
    assert_eq!(snapshot["state"], "running");
    assert_eq!(
        plane
            .registry
            .snapshot(&accepted.subagent_id)
            .expect("snapshot")
            .state,
        SubagentState::Running,
        "the response is the authoritative registry snapshot"
    );
}

/// `execution(cancel)` for a subagent target routes only to
/// `SubagentRegistry` — the logical lifecycle/cancellation authority — and
/// preserves the registry/process-driver ownership split: the intrinsic
/// never touches the child control plane directly.
#[tokio::test]
async fn execution_cancel_routes_subagent_targets_to_the_subagent_registry() {
    let plane = subagent_plane();
    let child = stage_exit0(&plane);
    let accepted = start_subagent(&plane, "inspect the tool plane").await;
    let fixture = execution_fixture(Some(plane.registry.clone()));

    let cancelled = run_execution(
        &fixture,
        serde_json::json!({
            "action": "cancel",
            "target": {"kind": "subagent", "id": accepted.subagent_id.to_string()},
        }),
    )
    .await;
    let snapshot = json_content(&cancelled);
    assert_eq!(snapshot["state"], "cancelling");
    assert_eq!(
        plane
            .registry
            .snapshot(&accepted.subagent_id)
            .expect("snapshot")
            .state,
        SubagentState::Cancelling,
        "the registry committed the cancellation intent"
    );

    // The child still settles through the registry's own driver path: a
    // late semantic result cannot erase the committed cancellation.
    child
        .complete(ChildResultStatus::Succeeded, Some("late"))
        .await;
    let settled = plane
        .registry
        .wait_until_settled(&accepted.subagent_id)
        .await
        .expect("settled");
    assert_eq!(settled.state, SubagentState::Cancelled);
}

// ---------------------------------------------------------------------------
// No heuristic fallback and isolation
// ---------------------------------------------------------------------------

/// A mismatched `kind`/id pair never falls through to another registry and
/// is never auto-guessed from the id string.
#[tokio::test]
async fn a_mismatched_kind_id_pair_never_falls_through_to_another_registry() {
    let plane = subagent_plane();
    let _child = stage_exit0(&plane);
    let accepted = start_subagent(&plane, "inspect the tool plane").await;

    let fixture = execution_fixture(Some(plane.registry.clone()));
    // A real background execution also exists in this runtime's background
    // registry, so the cross-domain ids are genuinely load-bearing.
    let (tool, _release) = support::fake::FakeTool::parking(
        common::tool_policies(
            "bash",
            "tool-bash",
            rustx::tools::types::ToolExecutionPolicy::ModelSelectable,
            rustx::tools::types::ToolConcurrencyPolicy::Sequential,
        ),
        support::fake::success_result("done"),
    );
    let executor: Arc<dyn rustx::tools::executor::ToolExecutor> = Arc::new(tool);
    let prepared = fixture
        .runtime
        .background()
        .prepare_dispatch(
            &background_invocation("bash"),
            &executor,
            rustx::tools::environment::ToolEnvironment::new(),
        )
        .expect("prepare");
    let outcome = fixture
        .runtime
        .background()
        .commit_dispatch(prepared, &CancellationSignal::new())
        .expect("dispatch commits");
    let BackgroundDispatchOutcome::Accepted { .. } = outcome else {
        panic!("accepted");
    };

    // kind=tool with the subagent's id: routed only to the background
    // registry, which does not know it.
    let wrong_kind = run_execution(
        &fixture,
        serde_json::json!({
            "action": "status",
            "target": {"kind": "tool", "id": accepted.subagent_id.to_string()},
        }),
    )
    .await;
    assert!(
        failure_message(&wrong_kind).contains("unknown background execution"),
        "the tool route fails through the background registry: {}",
        failure_message(&wrong_kind)
    );

    // kind=subagent with the background execution's id: routed only to the
    // subagent registry, which does not know it — never the background
    // snapshot.
    let wrong_kind = run_execution(
        &fixture,
        serde_json::json!({
            "action": "status",
            "target": {"kind": "subagent", "id": "exec_1"},
        }),
    )
    .await;
    assert!(
        failure_message(&wrong_kind).contains("unknown subagent execution"),
        "the subagent route fails through the subagent registry: {}",
        failure_message(&wrong_kind)
    );
}

/// Unknown ids fail deterministically at the selected domain authority.
#[tokio::test]
async fn unknown_ids_fail_deterministically() {
    let plane = subagent_plane();
    let fixture = execution_fixture(Some(plane.registry.clone()));
    for arguments in [
        serde_json::json!({"action": "status", "target": {"kind": "tool", "id": "exec_999"}}),
        serde_json::json!({"action": "cancel", "target": {"kind": "tool", "id": "exec_999"}}),
        serde_json::json!({
            "action": "status",
            "target": {"kind": "subagent", "id": "conv-162-subagent-99"},
        }),
        serde_json::json!({
            "action": "cancel",
            "target": {"kind": "subagent", "id": "conv-162-subagent-99"},
        }),
    ] {
        let result = run_execution(&fixture, arguments).await;
        assert!(
            matches!(result.status, ToolExecutionStatus::Failed { .. }),
            "unknown ids fail deterministically: {result:?}"
        );
    }
}

/// Cross-conversation ids do not leak state: at the owning domain boundary
/// they remain indistinguishable from unknown ids.
#[tokio::test]
async fn cross_conversation_ids_are_indistinguishable_from_unknown_ids() {
    let plane = subagent_plane();
    let _child = stage_exit0(&plane);
    let accepted = start_subagent(&plane, "inspect the tool plane").await;
    let fixture = execution_fixture(Some(plane.registry.clone()));

    // A structurally valid id of another conversation's subagent domain.
    let foreign = SubagentId::new("conversation-9-subagent-1");
    let result = run_execution(
        &fixture,
        serde_json::json!({
            "action": "status",
            "target": {"kind": "subagent", "id": foreign.to_string()},
        }),
    )
    .await;
    assert!(
        failure_message(&result).contains("unknown subagent execution"),
        "a foreign id is exactly an unknown id: {}",
        failure_message(&result)
    );
    let unknown_id = SubagentId::new("conv-162-subagent-77");
    let unknown = run_execution(
        &fixture,
        serde_json::json!({
            "action": "status",
            "target": {"kind": "subagent", "id": unknown_id.to_string()},
        }),
    )
    .await;
    assert!(matches!(unknown.status, ToolExecutionStatus::Failed { .. }));
    assert_eq!(
        plane.registry.snapshot(&foreign),
        plane.registry.snapshot(&unknown_id),
        "the foreign id and an unknown id are indistinguishable at the domain authority"
    );

    // And the real handle still works: no global scan was involved.
    let live = run_execution(
        &fixture,
        serde_json::json!({
            "action": "status",
            "target": {"kind": "subagent", "id": accepted.subagent_id.to_string()},
        }),
    )
    .await;
    assert_eq!(json_content(&live)["state"], "running");
}

// ---------------------------------------------------------------------------
// Result delivery stays canonical inbound
// ---------------------------------------------------------------------------

/// A subagent's terminal answer still arrives exactly once through the
/// existing canonical inbound message path, and `execution(status)` is
/// observation, not a second result-delivery channel.
#[tokio::test]
async fn subagent_terminal_answer_arrives_exactly_once_through_canonical_inbound() {
    let plane = subagent_plane();
    let child = stage_exit0(&plane);
    let accepted = start_subagent(&plane, "inspect the tool plane").await;
    let fixture = execution_fixture(Some(plane.registry.clone()));

    child
        .complete(ChildResultStatus::Succeeded, Some("the answer"))
        .await;
    let settled = plane
        .registry
        .wait_until_settled(&accepted.subagent_id)
        .await
        .expect("settled");
    assert_eq!(settled.state, SubagentState::Succeeded);

    // Exactly one canonical inbound message carries the answer, authored by
    // the child agent.
    let pending = plane
        .store
        .select_pending_batch()
        .expect("pending")
        .expect("one pending batch");
    assert_eq!(pending.items.len(), 1, "exactly one terminal inbound item");
    let item = &pending.items[0];
    assert_eq!(
        item.correlation.as_deref(),
        Some(crate::runtime::subagent::terminal_correlation(&accepted.subagent_id).as_str())
    );
    assert_eq!(
        item.message.id.as_str(),
        crate::runtime::subagent::terminal_message_id(&accepted.subagent_id).as_str()
    );
    assert!(matches!(
        item.message.source,
        rustx::message::types::UserSource::Agent { ref agent_id }
            if *agent_id == accepted.child_agent_id
    ));
    let text = item
        .message
        .content
        .iter()
        .filter_map(|block| match block {
            rustx::message::types::UserContentBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        text.contains("the answer"),
        "the canonical inbound carries the child's answer: {text}"
    );

    // `execution(status)` observes the authoritative terminal snapshot; it
    // does not deliver anything and creates no second channel.
    let status = run_execution(
        &fixture,
        serde_json::json!({
            "action": "status",
            "target": {"kind": "subagent", "id": accepted.subagent_id.to_string()},
        }),
    )
    .await;
    let snapshot = json_content(&status);
    assert_eq!(snapshot["kind"], "subagent");
    assert_eq!(snapshot["state"], "succeeded");
    assert_eq!(snapshot["subagent_id"], accepted.subagent_id.to_string());
    assert_eq!(
        plane
            .store
            .select_pending_batch()
            .expect("pending")
            .expect("one pending batch")
            .items
            .len(),
        1,
        "execution(status) published nothing and delivered nothing"
    );
}

// ---------------------------------------------------------------------------
// Obsolete API
// ---------------------------------------------------------------------------

/// `background_task` is no longer registered, and the compiled model-facing
/// tool surface exposes exactly the unified `execution` control plane
/// instead.
#[test]
fn background_task_is_no_longer_registered_or_model_visible() {
    let fixture = common::native_fixture();
    let definitions = fixture.registry.definitions();
    let names = definitions
        .iter()
        .map(|definition| definition.name.as_str())
        .collect::<Vec<_>>();
    assert!(
        !names.contains(&"background_task"),
        "background_task is not registered: {names:?}"
    );
    assert!(
        names.contains(&"execution"),
        "execution is registered: {names:?}"
    );

    let model_definitions = fixture.registry.model_definitions();
    let model_names = model_definitions
        .iter()
        .map(|definition| definition.name.as_str())
        .collect::<Vec<_>>();
    assert!(
        !model_names.contains(&"background_task"),
        "background_task is not model-visible: {model_names:?}"
    );
    assert!(
        model_names.contains(&"execution"),
        "execution is model-visible: {model_names:?}"
    );
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// A parking tool executor whose start is observable through a watch
/// channel, mirroring `m5_background`'s `ControlledExecutor`.
fn controlled_parking() -> (
    support::fake::FakeTool,
    tokio::sync::watch::Receiver<bool>,
    tokio::sync::watch::Sender<bool>,
) {
    let (tool, release) = support::fake::FakeTool::parking(
        common::tool_policies(
            "bash",
            "tool-bash",
            rustx::tools::types::ToolExecutionPolicy::ModelSelectable,
            rustx::tools::types::ToolConcurrencyPolicy::Sequential,
        ),
        support::fake::success_result("done"),
    );
    let started = tool.started();
    (tool, started, release)
}
