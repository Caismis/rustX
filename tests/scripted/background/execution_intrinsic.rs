//! Issue #162: the `execution` intrinsic — deterministic contract half.
//!
//! These tests prove the unified model-facing asynchronous execution
//! control plane for **detached background tool executions**, driven by
//! scripted executions only:
//!
//! - `background_task` dispatch returns a typed execution handle;
//! - `execution(status|cancel)` routes a tool target **only** to
//!   `ConversationBackgroundRegistry`, returning the authoritative domain
//!   snapshot;
//! - unknown and cross-kind ids fail deterministically and are never
//!   auto-guessed from the id string;
//! - `background_task` is no longer registered or model-visible.
//!
//! The subagent half of the same surface stages real child processes and is
//! therefore boundary conformance: `boundary_suites::subagent::execution_routing`.
//! All concurrency is driven by explicit gates (watch channels, the
//! registry's settlement waits); no sleep proves any invariant.

use super::super::support::execution::{
    background_invocation, execution_fixture, json_content, run_execution, subagent_plane,
};
use super::super::{common, support};

use std::sync::Arc;

use rustx::runtime::CancellationSignal;
use rustx::runtime::identity::ToolExecutionId;
use rustx::tools::background::{BackgroundDispatchOutcome, BackgroundLifecycle};
use rustx::tools::types::ToolExecutionStatus;

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
    // Deterministic settlement synchronization: the registry's own
    // state-version watch resolves when the absorbing terminal transition
    // commits. No scheduler-yield polling proves the transition.
    let terminal = registry
        .wait_until_terminal(&execution_id)
        .await
        .expect("the registry settles the cancellation");
    assert_eq!(
        terminal.state,
        BackgroundLifecycle::Cancelled,
        "the registry's own settlement reached the terminal state"
    );
    assert_eq!(
        registry.snapshot(&execution_id).expect("snapshot").state,
        BackgroundLifecycle::Cancelled
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
