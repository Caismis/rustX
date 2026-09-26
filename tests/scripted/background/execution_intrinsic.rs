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
use rustx::tools::execution::MAX_LISTED_EXECUTIONS;
use rustx::tools::types::ToolExecutionStatus;

// ---------------------------------------------------------------------------
// Creation results
// ---------------------------------------------------------------------------

/// A background tool dispatch returns a typed execution handle:
/// The creation result carries the UUID returned by the ownership commit.
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
    let BackgroundDispatchOutcome::Accepted {
        result,
        execution_id,
    } = outcome
    else {
        panic!("accepted");
    };
    let accepted = json_content(&result);
    assert_eq!(
        accepted["execution"],
        serde_json::json!({"kind": "tool", "id": execution_id}),
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
            .ends_with(&format!("tasks/{execution_id}.output")),
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
    let BackgroundDispatchOutcome::Accepted { execution_id, .. } = outcome else {
        panic!("accepted");
    };
    started
        .wait_for(|is_started| *is_started)
        .await
        .expect("runner started");

    let result = common::run_tool(
        &fixture,
        "execution",
        serde_json::json!({"action": "status", "target": {"kind": "tool", "id": execution_id}}),
    )
    .await;
    let snapshot = json_content(&result);
    assert_eq!(snapshot["kind"], "tool");
    assert_eq!(snapshot["execution_id"], execution_id.as_str());
    assert_eq!(snapshot["tool_name"], "bash");
    assert_eq!(snapshot["state"], "running");
    // The response is the authoritative registry snapshot, not a cached or
    // duplicate projection.
    assert_eq!(
        registry.snapshot(&execution_id).expect("snapshot").state,
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
    let BackgroundDispatchOutcome::Accepted { execution_id, .. } = outcome else {
        panic!("accepted");
    };
    started
        .wait_for(|is_started| *is_started)
        .await
        .expect("runner started");

    let cancelled = common::run_tool(
        &fixture,
        "execution",
        serde_json::json!({"action": "cancel", "target": {"kind": "tool", "id": execution_id}}),
    )
    .await;
    let snapshot = json_content(&cancelled);
    assert_eq!(snapshot["state"], "cancelling");
    assert_eq!(
        registry.snapshot(&execution_id).expect("snapshot").state,
        BackgroundLifecycle::Cancelling,
        "the authoritative registry transitioned, not a shadow copy"
    );

    // Repeated cancel is idempotent through the registry.
    let again = common::run_tool(
        &fixture,
        "execution",
        serde_json::json!({"action": "cancel", "target": {"kind": "tool", "id": execution_id}}),
    )
    .await;
    assert_eq!(again.status, ToolExecutionStatus::Success);
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
        serde_json::json!({"action": "status", "target": {"kind": "tool", "id": "exec_00000000-0000-7000-8000-000000000999"}}),
        serde_json::json!({"action": "cancel", "target": {"kind": "tool", "id": "exec_00000000-0000-7000-8000-000000000999"}}),
        serde_json::json!({
            "action": "status",
            "target": {"kind": "subagent", "id": "conv_35227a88-2fb4-735f-ad8e-ec0b35ff2a42-subagent-99"},
        }),
        serde_json::json!({
            "action": "cancel",
            "target": {"kind": "subagent", "id": "conv_35227a88-2fb4-735f-ad8e-ec0b35ff2a42-subagent-99"},
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

// Listing is a read model over allocation order, never UUID lexical order.
fn list(filter: serde_json::Value) -> serde_json::Value {
    let mut request = serde_json::Map::new();
    request.insert("action".into(), "list".into());
    request.insert("filter".into(), filter);
    request.into()
}
fn handles(value: &serde_json::Value) -> Vec<String> {
    value["executions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["execution"]["id"].as_str().unwrap().to_owned())
        .collect()
}
fn commit(
    registry: &rustx::tools::background::ConversationBackgroundRegistry,
    tool: support::fake::FakeTool,
) -> ToolExecutionId {
    let prepared = registry
        .prepare_dispatch(
            &background_invocation("bash"),
            &(Arc::new(tool) as Arc<dyn rustx::tools::executor::ToolExecutor>),
            rustx::tools::environment::ToolEnvironment::new(),
        )
        .unwrap();
    match registry
        .commit_dispatch(prepared, &CancellationSignal::new())
        .unwrap()
    {
        BackgroundDispatchOutcome::Accepted { execution_id, .. } => execution_id,
        BackgroundDispatchOutcome::RolledBack => panic!("unexpected rollback"),
    }
}
async fn parking(
    registry: &rustx::tools::background::ConversationBackgroundRegistry,
) -> (ToolExecutionId, tokio::sync::watch::Sender<bool>) {
    let (tool, mut started, release) = controlled_parking();
    let id = commit(registry, tool);
    support::fake::await_started(&mut started, "background starts").await;
    (id, release)
}
async fn settled(
    registry: &rustx::tools::background::ConversationBackgroundRegistry,
    result: rustx::tools::types::ToolExecutionResult,
) -> ToolExecutionId {
    let tool = support::fake::FakeTool::new(
        common::tool_policies(
            "bash",
            "tool-bash",
            rustx::tools::types::ToolExecutionPolicy::ModelSelectable,
            rustx::tools::types::ToolConcurrencyPolicy::Sequential,
        ),
        result,
    );
    let id = commit(registry, tool);
    registry.wait_until_terminal(&id).await.unwrap();
    id
}

#[tokio::test]
async fn listing_uses_captured_admission_order_and_filters_lifecycle_without_output() {
    let fixture = execution_fixture(None);
    let registry = fixture.runtime.background();
    let first = settled(
        registry,
        support::fake::success_result("detached-private-output"),
    )
    .await;
    let (second, _second_gate) = parking(registry).await;
    let (third, _third_gate) = parking(registry).await;
    let before = registry.all_snapshots();
    let all = json_content(&run_execution(&fixture, list(serde_json::json!({}))).await);
    assert_eq!(
        handles(&all),
        [third.to_string(), second.to_string(), first.to_string()]
    );
    assert_eq!(all["matched"], 3);
    assert_eq!(all["returned"], 3);
    assert_eq!(all["truncated"], false);
    assert_eq!(all["limit"], MAX_LISTED_EXECUTIONS);
    for entry in all["executions"].as_array().unwrap() {
        assert_eq!(entry["execution"]["kind"], "tool");
        assert_eq!(entry["tool_name"], "bash");
        let id = &entry["execution"]["id"];
        let status = json_content(
            &run_execution(
                &fixture,
                serde_json::json!({
                    "action":"status", "target":{"kind":"tool", "id":id},
                }),
            )
            .await,
        );
        assert_eq!(entry["state"], status["state"]);
        for withheld in ["result", "progress", "content", "exit_code"] {
            assert!(entry.get(withheld).is_none());
        }
    }
    assert!(!all.to_string().contains("detached-private-output"));
    assert!(registry.snapshot(&first).unwrap().result.is_some());
    for filter in [
        serde_json::json!({}),
        serde_json::json!({"active_only":false}),
        serde_json::json!({"kind":"tool"}),
    ] {
        assert_eq!(
            json_content(&run_execution(&fixture, list(filter)).await),
            all
        );
    }
    let active =
        json_content(&run_execution(&fixture, list(serde_json::json!({"active_only":true}))).await);
    assert_eq!(handles(&active), [third.to_string(), second.to_string()]);
    assert_eq!(active["matched"], 2);
    let children =
        json_content(&run_execution(&fixture, list(serde_json::json!({"kind":"subagent"}))).await);
    assert!(handles(&children).is_empty());
    assert_eq!(children["matched"], 0);
    assert_eq!(
        before,
        registry.all_snapshots(),
        "listing does not change ownership or lifecycle"
    );
}

#[tokio::test]
async fn listing_truncates_the_exact_admission_prefix_deterministically() {
    let fixture = execution_fixture(None);
    let mut admitted = Vec::new();
    for _ in 0..MAX_LISTED_EXECUTIONS + 6 {
        admitted.push(
            settled(
                fixture.runtime.background(),
                support::fake::success_result("done"),
            )
            .await,
        );
    }
    let expected: Vec<_> = admitted
        .iter()
        .rev()
        .take(MAX_LISTED_EXECUTIONS)
        .map(ToString::to_string)
        .collect();
    let first = json_content(&run_execution(&fixture, list(serde_json::json!({}))).await);
    assert_eq!(handles(&first), expected);
    assert_eq!(first["matched"], admitted.len());
    assert_eq!(first["returned"], MAX_LISTED_EXECUTIONS);
    assert_eq!(first["truncated"], true);
    assert_eq!(
        json_content(&run_execution(&fixture, list(serde_json::json!({}))).await),
        first
    );
}

#[tokio::test]
async fn listing_preserves_execution_count_and_later_cancellation() {
    let fixture = execution_fixture(None);
    let registry = fixture.runtime.background();
    let (tool, mut started, _release) = controlled_parking();
    let calls = tool.calls();
    let id = commit(registry, tool);
    support::fake::await_started(&mut started, "background starts").await;
    let before = calls.borrow().len();
    for _ in 0..3 {
        run_execution(&fixture, list(serde_json::json!({}))).await;
    }
    assert_eq!(calls.borrow().len(), before);
    assert_eq!(
        registry.snapshot(&id).unwrap().state,
        BackgroundLifecycle::Running
    );
    let cancel = run_execution(
        &fixture,
        serde_json::json!({"action":"cancel", "target":{"kind":"tool", "id":id}}),
    )
    .await;
    assert_eq!(cancel.status, ToolExecutionStatus::Success);
    assert_eq!(
        registry.wait_until_terminal(&id).await.unwrap().state,
        BackgroundLifecycle::Cancelled
    );
    let active =
        json_content(&run_execution(&fixture, list(serde_json::json!({"active_only":true}))).await);
    assert!(handles(&active).is_empty());
}

#[tokio::test]
async fn status_and_listing_preserve_outcome_unknown() {
    let fixture = execution_fixture(None);
    let id = settled(
        fixture.runtime.background(),
        rustx::tools::types::ToolExecutionResult {
            status: ToolExecutionStatus::OutcomeUnknown {
                detail: "remote termination unconfirmed".into(),
            },
            ..support::fake::success_result("unreachable")
        },
    )
    .await;
    let status = json_content(
        &run_execution(
            &fixture,
            serde_json::json!({"action":"status", "target":{"kind":"tool", "id":id}}),
        )
        .await,
    );
    assert_eq!(status["state"], "outcome_unknown");
    assert_eq!(status["result"]["status"]["type"], "outcome_unknown");
    let all = json_content(&run_execution(&fixture, list(serde_json::json!({}))).await);
    assert_eq!(handles(&all), [id.to_string()]);
    assert_eq!(all["executions"][0]["state"], "outcome_unknown");
    let active =
        json_content(&run_execution(&fixture, list(serde_json::json!({"active_only":true}))).await);
    assert!(handles(&active).is_empty());
}

#[tokio::test]
async fn empty_optional_subsystem_changes_no_execution_semantics() {
    let plane = subagent_plane();
    let without = execution_fixture(None);
    let with_empty = execution_fixture(Some(plane.registry.clone()));
    let mut gates = Vec::new();
    let mut ids = Vec::new();
    for fixture in [&without, &with_empty] {
        let first = settled(
            fixture.runtime.background(),
            support::fake::success_result("done"),
        )
        .await;
        let (second, gate) = parking(fixture.runtime.background()).await;
        ids.push((first, second));
        gates.push(gate);
    }
    for filter in [
        serde_json::json!({}),
        serde_json::json!({"active_only":true}),
        serde_json::json!({"kind":"tool"}),
        serde_json::json!({"kind":"subagent"}),
    ] {
        let mut projections = Vec::new();
        for (index, fixture) in [&without, &with_empty].into_iter().enumerate() {
            let mut view = json_content(&run_execution(fixture, list(filter.clone())).await);
            for entry in view["executions"].as_array_mut().unwrap() {
                let id = entry["execution"]["id"].as_str().unwrap();
                entry["execution"]["id"] = serde_json::json!(if id == ids[index].0.as_str() {
                    "first"
                } else {
                    assert_eq!(id, ids[index].1.as_str());
                    "second"
                });
            }
            projections.push(view);
        }
        assert_eq!(
            projections[0], projections[1],
            "only independently allocated identities differ"
        );
    }
    for (fixture, (first, second)) in [&without, &with_empty].into_iter().zip(ids) {
        assert_eq!(
            fixture.runtime.background().snapshot(&first).unwrap().state,
            BackgroundLifecycle::Succeeded
        );
        assert_eq!(
            fixture
                .runtime
                .background()
                .snapshot(&second)
                .unwrap()
                .state,
            BackgroundLifecycle::Running
        );
    }
}
