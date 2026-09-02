//! Boundary conformance: `execution(status|cancel)` and `subagent_start`
//! routing against **real staged child processes**.
//!
//! The scripted contract half of the `execution` intrinsic (routing for
//! detached background tool executions) lives in
//! `scripted_suites::background::execution_intrinsic`. This half stages real
//! trivial `sh` children through the registry's `cfg(test)` staging seam
//! because the subagent-side invariants — typed handles, status/cancel
//! routing, terminal answer delivery, pending-answer isolation — only exist
//! once the registry owns a real child process and a real control channel.
//! All concurrency is still driven by explicit gates; no sleep proves any
//! invariant.

use super::super::support::execution::{
    SubagentPlane, background_invocation, execution_fixture, failure_message, json_content,
    run_execution, subagent_plane,
};
use super::super::{common, support};

use std::sync::Arc;

use rustx::durable::ConversationStore;
use rustx::runtime::CancellationSignal;
use rustx::runtime::identity::{SubagentId, ToolCallId};
use rustx::runtime::subagent::{
    ResolvedSubagentSpec, SubagentName, SubagentStartOutcome, SubagentStartSpec, SubagentState,
};
use rustx::tools::background::BackgroundDispatchOutcome;
use rustx::tools::types::ToolExecutionStatus;

use crate::runtime::subagent::ipc::{ChildFrame, ChildResultStatus, ResultFrame};
use crate::runtime::subagent::process::StagedChild;

/// A scripted child: one trivial real process (kill/reap semantics) and the
/// test-held end of the control channel (protocol semantics).
struct ScriptedChild {
    peer: tokio::net::UnixStream,
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
        approval_mode: rustx::runtime::ApprovalMode::Policy,
        task: task.to_owned(),
        context: None,
        tool_call_id: ToolCallId::new("call-162"),
        terminal: rustx::runtime::subagent::SubagentTerminalMode::Normal,
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

/// The unique marker answer of the result-channel regressions. It must
/// appear exactly once, in the canonical inbound message, and nowhere in
/// any `execution(status|cancel)` response.
const SECRET_CHILD_ANSWER: &str = "issue162-secret-child-answer";

/// A subagent's terminal answer still arrives exactly once through the
/// existing canonical inbound message path, and `execution(status)` is
/// observation, not a second result-delivery channel: the complete status
/// response never contains the child answer, under any field.
#[tokio::test]
async fn subagent_terminal_answer_arrives_exactly_once_through_canonical_inbound() {
    let plane = subagent_plane();
    let child = stage_exit0(&plane);
    let accepted = start_subagent(&plane, "inspect the tool plane").await;
    let fixture = execution_fixture(Some(plane.registry.clone()));

    child
        .complete(ChildResultStatus::Succeeded, Some(SECRET_CHILD_ANSWER))
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
        text.contains(SECRET_CHILD_ANSWER),
        "the canonical inbound carries the child's answer: {text}"
    );

    // `execution(status)` observes the authoritative terminal snapshot; it
    // reports lifecycle facts only and never the child answer.
    let status = run_execution(
        &fixture,
        serde_json::json!({
            "action": "status",
            "target": {"kind": "subagent", "id": accepted.subagent_id.to_string()},
        }),
    )
    .await;
    assert_eq!(status.status, ToolExecutionStatus::Success);
    let snapshot = json_content(&status);
    assert_eq!(snapshot["kind"], "subagent");
    assert_eq!(snapshot["state"], "succeeded");
    assert_eq!(snapshot["subagent_id"], accepted.subagent_id.to_string());
    assert!(
        snapshot.get("detail").is_none(),
        "no success-result field exposes the answer: {snapshot}"
    );
    // The complete serialized model-facing response never contains the
    // unique answer marker, so a future accidental field cannot silently
    // reintroduce the result channel.
    let serialized = serde_json::to_string(&snapshot).expect("serializes");
    assert!(
        !serialized.contains(SECRET_CHILD_ANSWER),
        "execution(status) must never carry the child answer: {serialized}"
    );

    // `execution(cancel)` after terminal settlement is an idempotent no-op
    // returning the current snapshot; it must show the same non-result-
    // channel property.
    let cancelled = run_execution(
        &fixture,
        serde_json::json!({
            "action": "cancel",
            "target": {"kind": "subagent", "id": accepted.subagent_id.to_string()},
        }),
    )
    .await;
    assert_eq!(cancelled.status, ToolExecutionStatus::Success);
    let cancelled_snapshot = json_content(&cancelled);
    assert_eq!(cancelled_snapshot["state"], "succeeded");
    let cancelled_serialized = serde_json::to_string(&cancelled_snapshot).expect("serializes");
    assert!(
        !cancelled_serialized.contains(SECRET_CHILD_ANSWER),
        "execution(cancel) must never carry the child answer: {cancelled_serialized}"
    );

    // Neither call published or delivered anything: the canonical inbound
    // still holds exactly one item with the answer.
    let pending = plane
        .store
        .select_pending_batch()
        .expect("pending")
        .expect("one pending batch");
    assert_eq!(pending.items.len(), 1);
    let text = pending.items[0]
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
        text.contains(SECRET_CHILD_ANSWER),
        "the canonical inbound is the only channel carrying the answer: {text}"
    );
}

/// While the registry is still in `PublishingTerminal` (terminal publication
/// has not yet reached the durable authority), the pending child answer must
/// not be model-visible through `execution(status)` either.
#[tokio::test]
async fn publishing_terminal_does_not_expose_the_pending_child_answer() {
    let plane = subagent_plane();
    let child = stage_exit0(&plane);
    let accepted = start_subagent(&plane, "inspect the tool plane").await;
    let fixture = execution_fixture(Some(plane.registry.clone()));

    // Deterministic publication failure: the initial durable acceptance and
    // both bounded retries fail, so the registry settles into the explicit
    // non-terminal `PublishingTerminal` state with the answer retained in
    // its internal pending terminal.
    plane.store.arm_fail_accept_times(3);
    child
        .complete(ChildResultStatus::Succeeded, Some(SECRET_CHILD_ANSWER))
        .await;
    let unsettled = plane
        .registry
        .wait_until_settled(&accepted.subagent_id)
        .await
        .expect("publication abandoned resolves the wait");
    assert_eq!(unsettled.state, SubagentState::PublishingTerminal);
    assert!(unsettled.publication_abandoned);
    // Issue #178: the pending answer never rides the live read model, not
    // even while its publication is unresolved. The registry retains the
    // candidate internally for its bounded retry; the observable contract
    // is that `detail` is diagnostics-only and therefore `None` here.
    assert_eq!(
        unsettled.detail, None,
        "the pending answer is not exposed through the snapshot detail"
    );
    assert!(
        !serde_json::to_string(&unsettled)
            .expect("snapshot serializes")
            .contains(SECRET_CHILD_ANSWER),
        "the pending answer never appears anywhere in the serialized snapshot"
    );
    assert!(
        plane
            .store
            .select_pending_batch()
            .expect("pending")
            .is_none(),
        "nothing reached the durable inbound"
    );

    // The status response may expose the lifecycle state, but never the
    // pending answer.
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
    assert_eq!(snapshot["state"], "publishing_terminal");
    assert_eq!(snapshot["publication_abandoned"], true);
    assert!(
        snapshot.get("detail").is_none(),
        "the pending answer is never a model-facing field: {snapshot}"
    );
    let serialized = serde_json::to_string(&snapshot).expect("serializes");
    assert!(
        !serialized.contains(SECRET_CHILD_ANSWER),
        "PublishingTerminal must not expose the pending answer: {serialized}"
    );
}

// ---------------------------------------------------------------------------
// Live activity racing terminal settlement (Issue #178)
// ---------------------------------------------------------------------------

/// An activity frame applied while the child runs lands in the read model;
/// terminal settlement resets the projection to neutral with a bumped
/// revision; a frame racing in after the terminal is dropped — the terminal
/// stays final and the settled snapshot never projects the late activity.
#[tokio::test]
async fn activity_frames_racing_terminal_settlement_are_dropped() {
    let plane = subagent_plane();
    let child = stage_exit0(&plane);
    let accepted = start_subagent(&plane, "inspect the tool plane").await;
    let mut peer = child.peer;

    let observation_at = |revision: u64, activity| crate::runtime::subagent::SubagentObservation {
        revision,
        activity,
        last_activity_at: None,
        counters: crate::runtime::subagent::SubagentActivityCounters {
            model_requests: 1,
            model_retries: 0,
            tool_executions: 2,
        },
    };

    // The delegation arrives first; then the frames race in one burst: a
    // live activity update, the terminal result, and a post-terminal update.
    let frame = crate::runtime::subagent::ipc::read_parent_frame(&mut peer)
        .await
        .expect("delegate frame");
    assert!(matches!(
        frame,
        Some(crate::runtime::subagent::ipc::ParentFrame::Delegate(_))
    ));
    crate::runtime::subagent::ipc::write_child_frame(
        &mut peer,
        &ChildFrame::Activity(crate::runtime::subagent::ipc::ActivityFrame {
            observation: observation_at(
                3,
                crate::runtime::subagent::SubagentActivity::Tool {
                    tool_call_id: ToolCallId::new("call-178"),
                    tool_id: crate::runtime::identity::ToolId::new("tool-178"),
                    progress: None,
                },
            ),
        }),
    )
    .await
    .expect("live activity frame");
    crate::runtime::subagent::ipc::write_child_frame(
        &mut peer,
        &ChildFrame::Result(ResultFrame {
            status: ChildResultStatus::Succeeded,
            content: Some("RACE-178-ANSWER".to_owned()),
            diagnostic: None,
        }),
    )
    .await
    .expect("terminal result frame");
    crate::runtime::subagent::ipc::write_child_frame(
        &mut peer,
        &ChildFrame::Activity(crate::runtime::subagent::ipc::ActivityFrame {
            observation: observation_at(
                9,
                crate::runtime::subagent::SubagentActivity::Model {
                    request_id: crate::runtime::identity::RequestId::new("req-late"),
                    retry: 0,
                },
            ),
        }),
    )
    .await
    .expect("post-terminal activity frame");

    let settled = plane
        .registry
        .wait_until_settled(&accepted.subagent_id)
        .await
        .expect("settled");
    assert_eq!(settled.state, SubagentState::Succeeded);
    // The pre-terminal frame (revision 3) was applied in wire order; the
    // settlement reset bumped the revision once, and the post-terminal
    // frame (revision 9) was dropped: neither its activity nor its
    // revision ever landed.
    assert_eq!(
        settled.observation.activity,
        crate::runtime::subagent::SubagentActivity::AwaitingActivity,
        "the terminal settlement is the final projection"
    );
    assert_eq!(
        settled.observation.revision, 4,
        "the applied live revision plus exactly one settlement bump"
    );
    assert_eq!(
        settled.observation.counters.tool_executions, 2,
        "the counters of the last applied frame survive the reset"
    );
    assert_eq!(settled.detail, None, "the answer never rides the detail");

    // The result channel is still exactly the canonical durable inbound.
    let pending = plane
        .store
        .select_pending_batch()
        .expect("pending")
        .expect("one pending batch");
    assert_eq!(pending.items.len(), 1);
    let text = pending.items[0]
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
        text.contains("RACE-178-ANSWER"),
        "the canonical inbound carries the answer exactly once: {text}"
    );
}
