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

// ---------------------------------------------------------------------------
// Discovery (Issue #180)
// ---------------------------------------------------------------------------

/// `execution(list)` returns every conversation-owned execution within the
/// bound, each carrying its explicit typed handle rather than a bare id.
#[tokio::test]
async fn list_returns_every_conversation_owned_execution_with_a_typed_handle() {
    let fixture = execution_fixture(None);
    let registry = fixture.runtime.background().clone();
    let mut gates = Vec::new();
    let settled = dispatch_settled(&registry, "exec_1").await;
    gates.push(dispatch_parking(&registry).await);
    gates.push(dispatch_parking(&registry).await);

    let listing = json_content(&run_execution(&fixture, list(&serde_json::json!({}))).await);
    assert_eq!(listing["matched"], 3);
    assert_eq!(listing["returned"], 3);
    assert_eq!(listing["truncated"], false);
    assert_eq!(listing["limit"], MAX_LISTED_EXECUTIONS);
    assert_eq!(
        handles(&listing),
        vec!["exec_3", "exec_2", "exec_1"],
        "the most recently allocated execution comes first"
    );
    for entry in listing["executions"].as_array().expect("executions") {
        assert_eq!(
            entry["execution"]["kind"], "tool",
            "the kind is explicit, never inferred from the id"
        );
        assert!(entry["execution"]["id"].is_string());
        assert_eq!(entry["tool_name"], "bash");
    }
    assert_eq!(listing["executions"][0]["state"], "running");
    assert_eq!(listing["executions"][2]["state"], "succeeded");
    assert_eq!(settled, ToolExecutionId::new("exec_1"));
}

/// `active_only` deterministically excludes terminal records, and the
/// default — an omitted filter — lists active and terminal alike.
#[tokio::test]
async fn list_active_only_excludes_terminal_executions() {
    let fixture = execution_fixture(None);
    let registry = fixture.runtime.background().clone();
    dispatch_settled(&registry, "exec_1").await;
    let _gate = dispatch_parking(&registry).await;
    dispatch_settled(&registry, "exec_3").await;

    let active = json_content(
        &run_execution(&fixture, list(&serde_json::json!({"active_only": true}))).await,
    );
    assert_eq!(handles(&active), vec!["exec_2"]);
    assert_eq!(active["matched"], 1, "the count follows the filter");
    assert_eq!(active["returned"], 1);
    assert_eq!(active["truncated"], false);

    // The documented default: omitting the field, and stating it as false,
    // are the same contract, and both list terminal records too.
    for filter in [
        serde_json::json!({}),
        serde_json::json!({"active_only": false}),
    ] {
        let all = json_content(&run_execution(&fixture, list(&filter)).await);
        assert_eq!(handles(&all), vec!["exec_3", "exec_2", "exec_1"]);
        assert_eq!(all["matched"], 3);
    }
}

/// The `kind` filter selects one domain, and a runtime that owns no
/// subagent registry has no subagents to list — never a fall-through into
/// the background registry.
#[tokio::test]
async fn the_kind_filter_never_falls_through_into_the_other_domain() {
    let fixture = execution_fixture(None);
    let registry = fixture.runtime.background().clone();
    let _gate = dispatch_parking(&registry).await;

    let tools =
        json_content(&run_execution(&fixture, list(&serde_json::json!({"kind": "tool"}))).await);
    assert_eq!(handles(&tools), vec!["exec_1"]);
    assert_eq!(tools["matched"], 1);

    let subagents = json_content(
        &run_execution(&fixture, list(&serde_json::json!({"kind": "subagent"}))).await,
    );
    assert_eq!(
        subagents["executions"],
        serde_json::json!([]),
        "a tool execution is never reachable through the subagent kind"
    );
    assert_eq!(subagents["matched"], 0);
    assert_eq!(subagents["returned"], 0);
    assert_eq!(subagents["truncated"], false);
}

/// Repeating the same request against unchanged registries returns the same
/// entries in the same order with the same metadata.
#[tokio::test]
async fn repeated_lists_of_unchanged_state_are_identical() {
    let fixture = execution_fixture(None);
    let registry = fixture.runtime.background().clone();
    let _gates = [
        dispatch_parking(&registry).await,
        dispatch_parking(&registry).await,
    ];
    dispatch_settled(&registry, "exec_3").await;

    let first = json_content(&run_execution(&fixture, list(&serde_json::json!({}))).await);
    let second = json_content(&run_execution(&fixture, list(&serde_json::json!({}))).await);
    assert_eq!(first, second, "an unchanged snapshot lists identically");
}

/// The bound is an explicit runtime constant: the response stops there,
/// keeps the deterministic prefix of the order, and says so explicitly.
#[tokio::test]
async fn list_truncates_deterministically_at_the_configured_bound() {
    let fixture = execution_fixture(None);
    let registry = fixture.runtime.background().clone();
    let overflow = MAX_LISTED_EXECUTIONS + 6;
    for ordinal in 1..=overflow {
        dispatch_settled(&registry, &format!("exec_{ordinal}")).await;
    }

    let listing = json_content(&run_execution(&fixture, list(&serde_json::json!({}))).await);
    assert_eq!(listing["returned"], MAX_LISTED_EXECUTIONS);
    assert_eq!(listing["matched"], overflow);
    assert_eq!(listing["truncated"], true);
    assert_eq!(listing["limit"], MAX_LISTED_EXECUTIONS);
    let expected = (0..MAX_LISTED_EXECUTIONS)
        .map(|offset| format!("exec_{}", overflow - offset))
        .collect::<Vec<_>>();
    assert_eq!(
        handles(&listing),
        expected,
        "truncation keeps the newest deterministic prefix, never a sample"
    );

    let again = json_content(&run_execution(&fixture, list(&serde_json::json!({}))).await);
    assert_eq!(
        listing, again,
        "truncation chooses the same records and reports the same metadata"
    );
}

/// Listing is observation only: it changes no lifecycle, no cancellation
/// state, no settlement, and no tool invocation count.
#[tokio::test]
async fn listing_never_changes_execution_state_or_invocation_counts() {
    let fixture = execution_fixture(None);
    let registry = fixture.runtime.background().clone();
    let (executor, mut started, release) = controlled_parking();
    let calls = executor.calls();
    let prepared = registry
        .prepare_dispatch(
            &background_invocation("bash"),
            &(Arc::new(executor) as Arc<dyn rustx::tools::executor::ToolExecutor>),
            rustx::tools::environment::ToolEnvironment::new(),
        )
        .expect("prepare");
    registry
        .commit_dispatch(prepared, &CancellationSignal::new())
        .expect("dispatch commits");
    support::fake::await_started(&mut started, "background execution").await;
    dispatch_settled(&registry, "exec_2").await;

    let before = registry.all_snapshots();
    let calls_before = calls.borrow().len();
    for filter in [
        serde_json::json!({}),
        serde_json::json!({"active_only": true}),
        serde_json::json!({"kind": "tool"}),
    ] {
        let result = run_execution(&fixture, list(&filter)).await;
        assert_eq!(result.status, ToolExecutionStatus::Success);
    }
    let after = registry.all_snapshots();
    assert_eq!(
        before, after,
        "the authoritative registry read model is unchanged by listing"
    );
    assert_eq!(
        calls.borrow().len(),
        calls_before,
        "listing invokes no tool"
    );

    // The listed execution still settles exactly as it would have, through
    // its own registry-owned path.
    release.send_replace(true);
    let terminal = registry
        .wait_until_terminal(&ToolExecutionId::new("exec_1"))
        .await
        .expect("the registry settles the execution");
    assert_eq!(terminal.state, BackgroundLifecycle::Succeeded);
}

/// A listed execution's cancellation is unaffected: cancelling after a list
/// behaves exactly as it does without one, and the listing itself never
/// requests cancellation.
#[tokio::test]
async fn listing_never_cancels_or_pre_empts_settlement() {
    let fixture = execution_fixture(None);
    let registry = fixture.runtime.background().clone();
    let _gate = dispatch_parking(&registry).await;

    let listing = json_content(&run_execution(&fixture, list(&serde_json::json!({}))).await);
    assert_eq!(listing["executions"][0]["state"], "running");
    let execution_id = ToolExecutionId::new("exec_1");
    assert_eq!(
        registry.snapshot(&execution_id).expect("snapshot").state,
        BackgroundLifecycle::Running,
        "listing never moves an execution toward cancellation"
    );

    let cancelled = json_content(
        &run_execution(
            &fixture,
            serde_json::json!({"action": "cancel", "target": {"kind": "tool", "id": "exec_1"}}),
        )
        .await,
    );
    assert_eq!(cancelled["state"], "cancelling");
    let terminal = registry
        .wait_until_terminal(&execution_id)
        .await
        .expect("the registry settles the cancellation");
    assert_eq!(terminal.state, BackgroundLifecycle::Cancelled);

    // After settlement the listing reports the same terminal fact the
    // registry owns — a projection, never a second lifecycle record.
    let settled = json_content(&run_execution(&fixture, list(&serde_json::json!({}))).await);
    assert_eq!(settled["executions"][0]["state"], "cancelled");
    assert_eq!(
        json_content(
            &run_execution(&fixture, list(&serde_json::json!({"active_only": true}))).await
        )["executions"],
        serde_json::json!([]),
        "a settled execution leaves the active listing"
    );
}

/// `execution(status)` and `execution(list)` project the same lifecycle
/// facts for the same execution, because both read the same authoritative
/// registry snapshot.
#[tokio::test]
async fn list_and_status_report_consistent_lifecycle_facts() {
    let fixture = execution_fixture(None);
    let registry = fixture.runtime.background().clone();
    let _gate = dispatch_parking(&registry).await;
    dispatch_settled(&registry, "exec_2").await;

    let listing = json_content(&run_execution(&fixture, list(&serde_json::json!({}))).await);
    for entry in listing["executions"].as_array().expect("executions") {
        let id = entry["execution"]["id"].as_str().expect("id");
        let status = json_content(
            &run_execution(
                &fixture,
                serde_json::json!({
                    "action": "status",
                    "target": {"kind": "tool", "id": id},
                }),
            )
            .await,
        );
        assert_eq!(
            entry["state"], status["state"],
            "list and status agree about {id}"
        );
        assert_eq!(entry["tool_name"], status["tool_name"]);
        assert_eq!(status["execution_id"], id);
    }
}

/// The bounded summary is a discovery read model: a settled execution's
/// output never rides the listing, so `list` can never become a second
/// result channel for detached tool executions.
#[tokio::test]
async fn a_listing_never_carries_detached_execution_output() {
    let fixture = execution_fixture(None);
    let registry = fixture.runtime.background().clone();
    dispatch_settled(&registry, "exec_1").await;

    let listing = run_execution(&fixture, list(&serde_json::json!({}))).await;
    let value = json_content(&listing);
    assert_eq!(value["executions"][0]["state"], "succeeded");
    let serialized = serde_json::to_string(&value).expect("string");
    assert!(
        !serialized.contains("issue180-detached-output"),
        "the terminal result stays on its own domain channel: {serialized}"
    );
    for withheld in ["result", "progress", "content", "exit_code"] {
        assert!(
            !serialized.contains(withheld),
            "a listing carries no {withheld}: {serialized}"
        );
    }
    // The authoritative snapshot still carries it — the listing projects a
    // narrower read model, it does not erase domain state.
    assert!(
        registry
            .snapshot(&ToolExecutionId::new("exec_1"))
            .expect("snapshot")
            .result
            .is_some(),
        "the registry keeps the terminal result it owns"
    );
}

/// Attaching an empty optional subsystem changes nothing: a runtime that
/// owns an empty subagent registry lists exactly what a runtime without one
/// lists, and the discovery machinery's mere existence alters no tool
/// execution behavior.
#[tokio::test]
async fn an_empty_optional_subsystem_changes_no_listing_or_execution_behavior() {
    let without = execution_fixture(None);
    let plane = subagent_plane();
    let with_empty = execution_fixture(Some(plane.registry.clone()));
    assert!(
        plane.registry.all_snapshots().is_empty(),
        "the attached subsystem is empty"
    );

    for fixture in [&without, &with_empty] {
        let registry = fixture.runtime.background().clone();
        dispatch_settled(&registry, "exec_1").await;
        let _gate = dispatch_parking(&registry).await;
    }

    for filter in [
        serde_json::json!({}),
        serde_json::json!({"active_only": true}),
        serde_json::json!({"kind": "tool"}),
        serde_json::json!({"kind": "subagent"}),
    ] {
        let bare = json_content(&run_execution(&without, list(&filter)).await);
        let attached = json_content(&run_execution(&with_empty, list(&filter)).await);
        assert_eq!(
            bare, attached,
            "an empty optional subsystem is indistinguishable from none: {filter}"
        );
    }

    // And the executions themselves settle identically on both sides.
    for fixture in [&without, &with_empty] {
        let registry = fixture.runtime.background().clone();
        assert_eq!(
            registry
                .snapshot(&ToolExecutionId::new("exec_1"))
                .expect("snapshot")
                .state,
            BackgroundLifecycle::Succeeded
        );
        assert_eq!(
            registry
                .snapshot(&ToolExecutionId::new("exec_2"))
                .expect("snapshot")
                .state,
            BackgroundLifecycle::Running
        );
    }
}

/// One `execution(list)` invocation.
fn list(filter: &serde_json::Value) -> serde_json::Value {
    if filter.as_object().is_some_and(serde_json::Map::is_empty) {
        serde_json::json!({"action": "list"})
    } else {
        serde_json::json!({"action": "list", "filter": filter})
    }
}

/// The handle ids of a listing, in response order.
fn handles(listing: &serde_json::Value) -> Vec<String> {
    listing["executions"]
        .as_array()
        .expect("executions")
        .iter()
        .map(|entry| {
            entry["execution"]["id"]
                .as_str()
                .expect("handle id")
                .to_owned()
        })
        .collect()
}

/// Dispatches one background execution that parks until the returned gate
/// releases it, so the record stays deterministically active.
async fn dispatch_parking(
    registry: &rustx::tools::background::ConversationBackgroundRegistry,
) -> tokio::sync::watch::Sender<bool> {
    let (executor, mut started, release) = controlled_parking();
    let prepared = registry
        .prepare_dispatch(
            &background_invocation("bash"),
            &(Arc::new(executor) as Arc<dyn rustx::tools::executor::ToolExecutor>),
            rustx::tools::environment::ToolEnvironment::new(),
        )
        .expect("prepare");
    registry
        .commit_dispatch(prepared, &CancellationSignal::new())
        .expect("dispatch commits");
    support::fake::await_started(&mut started, "parking background execution").await;
    release
}

/// Dispatches one background execution and waits for the registry's own
/// terminal settlement, so the record is deterministically terminal.
async fn dispatch_settled(
    registry: &rustx::tools::background::ConversationBackgroundRegistry,
    expected_id: &str,
) -> ToolExecutionId {
    let executor = support::fake::FakeTool::new(
        common::tool_policies(
            "bash",
            "tool-bash",
            rustx::tools::types::ToolExecutionPolicy::ModelSelectable,
            rustx::tools::types::ToolConcurrencyPolicy::Sequential,
        ),
        support::fake::success_result("issue180-detached-output"),
    );
    let prepared = registry
        .prepare_dispatch(
            &background_invocation("bash"),
            &(Arc::new(executor) as Arc<dyn rustx::tools::executor::ToolExecutor>),
            rustx::tools::environment::ToolEnvironment::new(),
        )
        .expect("prepare");
    registry
        .commit_dispatch(prepared, &CancellationSignal::new())
        .expect("dispatch commits");
    let execution_id = ToolExecutionId::new(expected_id);
    registry
        .wait_until_terminal(&execution_id)
        .await
        .expect("the registry settles the execution");
    execution_id
}
