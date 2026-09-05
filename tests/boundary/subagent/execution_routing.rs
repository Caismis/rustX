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
    run_execution, subagent_plane, subagent_plane_for,
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
/// test-held ends of the control channel and the disposable observation
/// channel (protocol semantics).
struct ScriptedChild {
    peer: tokio::net::UnixStream,
    /// The test-held end of the observation channel (Issue #178): Activity
    /// frames are written here, never on the control peer.
    observation_peer: tokio::net::UnixStream,
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
        execution_deadline: None,
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
/// the protocol over `peer` and the observation plane over
/// `observation_peer`.
fn stage_exit0(plane: &SubagentPlane) -> ScriptedChild {
    let (driver_end, test_end) = tokio::net::UnixStream::pair().expect("pair");
    let (observation_end, observation_peer) =
        tokio::net::UnixStream::pair().expect("observation pair");
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
    let staged = StagedChild::for_test(child, driver_end, observation_end, child_runtime_root);
    plane.registry.push_staged_override(staged);
    ScriptedChild {
        peer: test_end,
        observation_peer,
    }
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

/// A subagent start returns the minimal model-facing creation contract
/// (Issue #192): the typed execution handle, the running state, and the
/// named agent — and nothing else.
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
    let result = crate::tools::native::subagent::accepted_result(accepted.clone());
    let value = json_content(&result);
    assert_eq!(
        value,
        serde_json::json!({
            "execution": {"kind": "subagent", "id": "conv-162-subagent-1"},
            "state": "running",
            "agent": "explore",
        }),
        "the creation result is exactly the minimal control contract"
    );
    // The runtime acceptance value still carries the rich provenance —
    // below the model boundary.
    assert!(!accepted.definition_digest.is_empty());
    let serialized = serde_json::to_string(&value).expect("serializes");
    for removed in [
        "definition_digest",
        "child_agent_id",
        "child_conversation_id",
        "tool_call_id",
        "workspace",
        "note",
    ] {
        assert!(
            !serialized.contains(removed),
            "runtime provenance is not model-facing: {removed} in {serialized}"
        );
    }
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
    assert_eq!(
        snapshot["execution"],
        serde_json::json!({"kind": "subagent", "id": accepted.subagent_id.to_string()}),
        "the response is identified by the canonical execution handle"
    );
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
    assert_eq!(
        snapshot["execution"],
        serde_json::json!({"kind": "subagent", "id": accepted.subagent_id.to_string()}),
        "the response is identified by the canonical execution handle"
    );
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

/// The Subagent Final Report Principle (Issue #192): the child's
/// intermediate traffic — diagnostic notes, live observation, everything
/// before the terminal frame — never becomes parent result content. The
/// parent's canonical inbound receives exactly the final report, once.
#[tokio::test]
async fn child_intermediate_traffic_never_becomes_parent_result_content() {
    let plane = subagent_plane();
    let child = stage_exit0(&plane);
    let accepted = start_subagent(&plane, "inspect the tool plane").await;
    let mut peer = child.peer;
    let mut observation_peer = child.observation_peer;

    let frame = crate::runtime::subagent::ipc::read_parent_frame(&mut peer)
        .await
        .expect("delegate frame");
    assert!(matches!(
        frame,
        Some(crate::runtime::subagent::ipc::ParentFrame::Delegate(_))
    ));
    // Intermediate noise on both channels before the terminal frame.
    crate::runtime::subagent::ipc::write_child_frame(
        &mut peer,
        &ChildFrame::Diagnostic(crate::runtime::subagent::ipc::DiagnosticFrame {
            message: "intermediate reasoning noise".to_owned(),
        }),
    )
    .await
    .expect("diagnostic frame");
    crate::runtime::subagent::ipc::write_activity_frame(
        &mut observation_peer,
        &crate::runtime::subagent::ipc::ActivityFrame {
            observation: crate::runtime::subagent::SubagentObservation {
                revision: 1,
                activity: crate::runtime::subagent::SubagentActivity::Model {
                    request_id: crate::runtime::identity::RequestId::new("req-intermediate"),
                    retry: 0,
                },
                last_activity_at: None,
                counters: crate::runtime::subagent::SubagentActivityCounters::default(),
            },
        },
    )
    .await
    .expect("activity frame");
    crate::runtime::subagent::ipc::write_child_frame(
        &mut peer,
        &ChildFrame::Result(ResultFrame {
            status: ChildResultStatus::Succeeded,
            content: Some("FINAL-REPORT-ONLY".to_owned()),
            diagnostic: None,
        }),
    )
    .await
    .expect("terminal result frame");
    let settled = plane
        .registry
        .wait_until_settled(&accepted.subagent_id)
        .await
        .expect("settled");
    assert_eq!(settled.state, SubagentState::Succeeded);

    let pending = plane
        .store
        .select_pending_batch()
        .expect("pending")
        .expect("one pending batch");
    assert_eq!(
        pending.items.len(),
        1,
        "exactly one parent inbound item exists: the final report"
    );
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
    assert_eq!(
        text, "FINAL-REPORT-ONLY",
        "the report is exactly the child's final response, no summary or reconstruction"
    );
    assert!(
        !text.contains("intermediate reasoning noise"),
        "intermediate traffic never enters the parent's result content"
    );
}

/// `execution(cancel)` surfaces the committed cancellation reason in the
/// status projection, so a model-initiated cancellation stays
/// distinguishable from a deadline expiry (Issue #191/#192).
#[tokio::test]
async fn execution_status_surfaces_the_committed_cancellation_reason() {
    let plane = subagent_plane();
    let child = stage_exit0(&plane);
    let accepted = start_subagent(&plane, "inspect the tool plane").await;
    let fixture = execution_fixture(Some(plane.registry.clone()));

    // A running child has no cancellation reason.
    let running = json_content(
        &run_execution(
            &fixture,
            serde_json::json!({
                "action": "status",
                "target": {"kind": "subagent", "id": accepted.subagent_id.to_string()},
            }),
        )
        .await,
    );
    assert_eq!(running["state"], "running");
    assert!(
        running.get("cancellation_reason").is_none(),
        "no reason exists before any cancellation intent: {running}"
    );

    let cancelling = json_content(
        &run_execution(
            &fixture,
            serde_json::json!({
                "action": "cancel",
                "target": {"kind": "subagent", "id": accepted.subagent_id.to_string()},
            }),
        )
        .await,
    );
    assert_eq!(cancelling["state"], "cancelling");
    assert_eq!(cancelling["cancellation_reason"], "user_requested");

    child.complete(ChildResultStatus::Cancelled, None).await;
    let settled = plane
        .registry
        .wait_until_settled(&accepted.subagent_id)
        .await
        .expect("settled");
    assert_eq!(settled.state, SubagentState::Cancelled);
    let terminal = json_content(
        &run_execution(
            &fixture,
            serde_json::json!({
                "action": "status",
                "target": {"kind": "subagent", "id": accepted.subagent_id.to_string()},
            }),
        )
        .await,
    );
    assert_eq!(terminal["state"], "cancelled");
    assert_eq!(
        terminal["cancellation_reason"], "user_requested",
        "the reason survives terminal settlement: {terminal}"
    );
}

/// Two concurrent children of the same named agent stay unambiguously
/// correlated through their distinct execution handles and their canonical
/// result provenance — without any other runtime id crossing the model
/// boundary (Issue #192).
#[tokio::test]
async fn two_concurrent_children_of_one_agent_stay_unambiguously_correlated() {
    let plane = subagent_plane();
    let fixture = execution_fixture(Some(plane.registry.clone()));
    let first_child = stage_exit0(&plane);
    let first = start_subagent(&plane, "first task").await;
    let second_child = stage_exit0(&plane);
    let second = start_subagent(&plane, "second task").await;
    assert_ne!(
        first.subagent_id, second.subagent_id,
        "same agent, distinct executions"
    );
    assert_eq!(first.agent, "explore");
    assert_eq!(first.agent, second.agent);

    // Settle them out of start order: the second child first.
    second_child
        .complete(ChildResultStatus::Succeeded, Some("SECOND-ANSWER"))
        .await;
    plane
        .registry
        .wait_until_settled(&second.subagent_id)
        .await
        .expect("second settled");
    first_child
        .complete(ChildResultStatus::Succeeded, Some("FIRST-ANSWER"))
        .await;
    plane
        .registry
        .wait_until_settled(&first.subagent_id)
        .await
        .expect("first settled");

    // Each canonical inbound report carries exactly its own child's answer,
    // authored by that child's own agent identity, in settlement order.
    let pending = plane
        .store
        .select_pending_batch()
        .expect("pending")
        .expect("one pending batch");
    assert_eq!(pending.items.len(), 2, "exactly one report per child");
    for (item, accepted, answer) in [
        (&pending.items[0], &second, "SECOND-ANSWER"),
        (&pending.items[1], &first, "FIRST-ANSWER"),
    ] {
        assert_eq!(
            item.correlation.as_deref(),
            Some(crate::runtime::subagent::terminal_correlation(&accepted.subagent_id).as_str()),
            "the report correlates to its own execution"
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
        assert!(text.contains(answer), "the child's own report: {text}");
    }

    // Each status response is identified by exactly the handle the model
    // holds for that child.
    for accepted in [&first, &second] {
        let status = json_content(
            &run_execution(
                &fixture,
                serde_json::json!({
                    "action": "status",
                    "target": {"kind": "subagent", "id": accepted.subagent_id.to_string()},
                }),
            )
            .await,
        );
        assert_eq!(
            status["execution"],
            serde_json::json!({"kind": "subagent", "id": accepted.subagent_id.to_string()})
        );
        assert_eq!(status["agent"], "explore");
        assert_eq!(status["state"], "succeeded");
        let serialized = serde_json::to_string(&status).expect("serializes");
        assert!(
            !serialized.contains("FIRST-ANSWER") && !serialized.contains("SECOND-ANSWER"),
            "status never carries a child report: {serialized}"
        );
    }
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
///
/// Activity travels on the dedicated observation channel (Issue #178), so
/// the test synchronizes through the registry read model itself: the live
/// frame is provably applied (its revision is observed) before the terminal
/// result is sent on the control channel.
#[tokio::test]
async fn activity_frames_racing_terminal_settlement_are_dropped() {
    let plane = subagent_plane();
    let child = stage_exit0(&plane);
    let accepted = start_subagent(&plane, "inspect the tool plane").await;
    let mut peer = child.peer;
    let mut observation_peer = child.observation_peer;

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

    // The delegation arrives first; then the live activity update crosses
    // the observation channel and is provably applied to the read model.
    let frame = crate::runtime::subagent::ipc::read_parent_frame(&mut peer)
        .await
        .expect("delegate frame");
    assert!(matches!(
        frame,
        Some(crate::runtime::subagent::ipc::ParentFrame::Delegate(_))
    ));
    crate::runtime::subagent::ipc::write_activity_frame(
        &mut observation_peer,
        &crate::runtime::subagent::ipc::ActivityFrame {
            observation: observation_at(
                3,
                crate::runtime::subagent::SubagentActivity::Tool {
                    tool_call_id: ToolCallId::new("call-178"),
                    tool_id: crate::runtime::identity::ToolId::new("tool-178"),
                    progress: None,
                },
            ),
        },
    )
    .await
    .expect("live activity frame");
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let snapshot = plane
                .registry
                .snapshot(&accepted.subagent_id)
                .expect("child record");
            if snapshot.observation.revision == 3 {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the live activity frame applies before the terminal");

    // The terminal result crosses the control channel and settles.
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
    let settled = plane
        .registry
        .wait_until_settled(&accepted.subagent_id)
        .await
        .expect("settled");

    // A post-terminal update: wherever it lands — the observation receiver
    // may already be torn down with the drive — it can never land in the
    // read model (the registry's terminal-record drop rule).
    let _ = crate::runtime::subagent::ipc::write_activity_frame(
        &mut observation_peer,
        &crate::runtime::subagent::ipc::ActivityFrame {
            observation: observation_at(
                9,
                crate::runtime::subagent::SubagentActivity::Model {
                    request_id: crate::runtime::identity::RequestId::new("req-late"),
                    retry: 0,
                },
            ),
        },
    )
    .await;

    assert_eq!(settled.state, SubagentState::Succeeded);
    // The pre-terminal frame (revision 3) was applied; the settlement reset
    // bumped the revision once, and the post-terminal frame (revision 9)
    // was dropped: neither its activity nor its revision ever landed.
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

// ---------------------------------------------------------------------------
// Discovery across both domains (Issue #180)
// ---------------------------------------------------------------------------

/// The merged listing is the deterministic alternating order over both
/// domains, each contributing its most recently allocated execution first.
#[tokio::test]
async fn list_merges_both_domains_in_the_deterministic_alternating_order() {
    let plane = subagent_plane();
    let fixture = execution_fixture(Some(plane.registry.clone()));
    let _first = stage_exit0(&plane);
    let first = start_subagent(&plane, "first child").await;
    let _second = stage_exit0(&plane);
    let second = start_subagent(&plane, "second child").await;
    let _tools = dispatch_parking_pair(&fixture).await;

    let listing = json_content(&run_execution(&fixture, list(&serde_json::json!({}))).await);
    assert_eq!(
        handles(&listing),
        vec![
            ("tool", "exec_2".to_owned()),
            ("subagent", second.subagent_id.to_string()),
            ("tool", "exec_1".to_owned()),
            ("subagent", first.subagent_id.to_string()),
        ],
        "tool, subagent, tool, subagent — each domain newest first"
    );
    assert_eq!(listing["matched"], 4);
    assert_eq!(listing["returned"], 4);
    assert_eq!(listing["truncated"], false);
    // Repeating the request against unchanged registries is stable.
    let again = json_content(&run_execution(&fixture, list(&serde_json::json!({}))).await);
    assert_eq!(listing, again);
}

/// A `kind` filter selects one domain authority and never falls through
/// into the other, even when both domains hold executions.
#[tokio::test]
async fn kind_filtering_isolates_the_two_domains() {
    let plane = subagent_plane();
    let fixture = execution_fixture(Some(plane.registry.clone()));
    let _child = stage_exit0(&plane);
    let child = start_subagent(&plane, "inspect the tool plane").await;
    let _tools = dispatch_parking_pair(&fixture).await;

    let tools =
        json_content(&run_execution(&fixture, list(&serde_json::json!({"kind": "tool"}))).await);
    assert_eq!(
        handles(&tools),
        vec![("tool", "exec_2".to_owned()), ("tool", "exec_1".to_owned())],
        "the tool filter reaches only the background registry"
    );
    assert_eq!(tools["matched"], 2, "the count excludes the other domain");

    let subagents = json_content(
        &run_execution(&fixture, list(&serde_json::json!({"kind": "subagent"}))).await,
    );
    assert_eq!(
        handles(&subagents),
        vec![("subagent", child.subagent_id.to_string())],
        "the subagent filter reaches only the subagent registry"
    );
    assert_eq!(subagents["matched"], 1);
}

/// Discovery is conversation-scoped by construction: another conversation's
/// executions are not filtered out, they are unreachable — even when their
/// ids are structurally identical to this conversation's.
#[tokio::test]
async fn foreign_conversation_executions_are_never_listed() {
    let plane = subagent_plane();
    let fixture = execution_fixture(Some(plane.registry.clone()));
    let _child = stage_exit0(&plane);
    let mine = start_subagent(&plane, "my child").await;
    let _tools = dispatch_parking_pair(&fixture).await;

    // A second conversation with its own registries, wired to nothing.
    let foreign_plane = subagent_plane_for("conv-180-foreign");
    let _foreign_child = stage_exit0(&foreign_plane);
    let foreign_child = start_subagent(&foreign_plane, "foreign child").await;
    let foreign_fixture = execution_fixture(Some(foreign_plane.registry.clone()));
    let _foreign_tools = dispatch_parking_pair(&foreign_fixture).await;

    let listing = json_content(&run_execution(&fixture, list(&serde_json::json!({}))).await);
    assert_eq!(listing["matched"], 3, "only this conversation's executions");
    assert_eq!(
        handles(&listing),
        vec![
            ("tool", "exec_2".to_owned()),
            ("subagent", mine.subagent_id.to_string()),
            ("tool", "exec_1".to_owned()),
        ]
    );

    // Both conversations allocated the very same *tool* execution ids, so
    // structurally identical ids exist in both registries — and each
    // listing still shows only the records its own conversation owns,
    // because it never sees the others at all.
    let foreign_listing =
        json_content(&run_execution(&foreign_fixture, list(&serde_json::json!({}))).await);
    assert_eq!(foreign_listing["matched"], 3);
    assert!(
        handles(&foreign_listing).contains(&("subagent", foreign_child.subagent_id.to_string())),
        "the foreign conversation lists its own child"
    );
    assert!(
        !handles(&listing).contains(&("subagent", foreign_child.subagent_id.to_string())),
        "and this conversation never sees it"
    );
    assert_ne!(
        mine.subagent_id, foreign_child.subagent_id,
        "the two children are genuinely different executions"
    );
    assert_eq!(
        handles(&listing)
            .iter()
            .filter(|(kind, _)| *kind == "tool")
            .count(),
        2,
        "the colliding foreign tool ids never doubled this conversation's own"
    );

    // The same boundary holds for the single-target surface: a foreign id
    // is exactly an unknown id.
    let status = run_execution(
        &fixture,
        serde_json::json!({
            "action": "status",
            "target": {"kind": "subagent", "id": foreign_child.subagent_id.to_string()},
        }),
    )
    .await;
    assert!(
        failure_message(&status).contains("unknown subagent execution"),
        "a foreign execution is indistinguishable from absence"
    );
}

/// Listing a running child changes nothing about it: not its lifecycle, not
/// its settlement, not its cancellation, and not the Issue #178 observation
/// plane's latest-value or revision state.
#[tokio::test]
async fn listing_never_disturbs_a_running_child_or_its_observation() {
    let plane = subagent_plane();
    let fixture = execution_fixture(Some(plane.registry.clone()));
    let child = stage_exit0(&plane);
    let accepted = start_subagent(&plane, "inspect the tool plane").await;
    let mut peer = child.peer;
    let mut observation_peer = child.observation_peer;

    // Drive one live activity update through the observation plane and wait
    // until the registry has provably applied it.
    let frame = crate::runtime::subagent::ipc::read_parent_frame(&mut peer)
        .await
        .expect("delegate frame");
    assert!(matches!(
        frame,
        Some(crate::runtime::subagent::ipc::ParentFrame::Delegate(_))
    ));
    crate::runtime::subagent::ipc::write_activity_frame(
        &mut observation_peer,
        &crate::runtime::subagent::ipc::ActivityFrame {
            observation: crate::runtime::subagent::SubagentObservation {
                revision: 5,
                activity: crate::runtime::subagent::SubagentActivity::Tool {
                    tool_call_id: ToolCallId::new("call-180"),
                    tool_id: crate::runtime::identity::ToolId::new("tool-grep"),
                    progress: None,
                },
                last_activity_at: None,
                counters: crate::runtime::subagent::SubagentActivityCounters {
                    model_requests: 3,
                    model_retries: 1,
                    tool_executions: 2,
                },
            },
        },
    )
    .await
    .expect("live activity frame");
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let snapshot = plane
                .registry
                .snapshot(&accepted.subagent_id)
                .expect("child record");
            if snapshot.observation.revision == 5 {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the live activity frame applies");

    let before = plane.registry.all_snapshots();
    for filter in [
        serde_json::json!({}),
        serde_json::json!({"kind": "subagent"}),
        serde_json::json!({"active_only": true}),
    ] {
        let result = run_execution(&fixture, list(&filter)).await;
        assert_eq!(result.status, ToolExecutionStatus::Success);
    }
    let after = plane.registry.all_snapshots();
    assert_eq!(
        before, after,
        "listing mutates no lifecycle, no counters, and no observation state"
    );
    let observation = &after[0].observation;
    assert_eq!(
        observation.revision, 5,
        "listing never advances the latest-value revision"
    );
    assert_eq!(observation.counters.model_requests, 3);
    assert_eq!(observation.counters.model_retries, 1);
    assert_eq!(observation.counters.tool_executions, 2);
    assert_eq!(
        after[0].state,
        SubagentState::Running,
        "a listed child is still running"
    );

    // And the child still settles exactly once, through the canonical path.
    crate::runtime::subagent::ipc::write_child_frame(
        &mut peer,
        &ChildFrame::Result(ResultFrame {
            status: ChildResultStatus::Succeeded,
            content: Some(SECRET_CHILD_ANSWER.to_owned()),
            diagnostic: None,
        }),
    )
    .await
    .expect("terminal result frame");
    let settled = plane
        .registry
        .wait_until_settled(&accepted.subagent_id)
        .await
        .expect("settled");
    assert_eq!(settled.state, SubagentState::Succeeded);
    let pending = plane
        .store
        .select_pending_batch()
        .expect("pending")
        .expect("one pending batch");
    assert_eq!(
        pending.items.len(),
        1,
        "listing added no publication and removed none"
    );
}

/// A settled child's answer and history never ride the listing: the
/// canonical inbound message remains the one result channel.
#[tokio::test]
async fn a_listing_never_carries_a_child_answer_or_its_history() {
    let plane = subagent_plane();
    let fixture = execution_fixture(Some(plane.registry.clone()));
    let child = stage_exit0(&plane);
    let accepted = start_subagent(&plane, "inspect the tool plane").await;
    child
        .complete(ChildResultStatus::Succeeded, Some(SECRET_CHILD_ANSWER))
        .await;
    let settled = plane
        .registry
        .wait_until_settled(&accepted.subagent_id)
        .await
        .expect("settled");
    assert_eq!(settled.state, SubagentState::Succeeded);

    let result = run_execution(&fixture, list(&serde_json::json!({}))).await;
    let listing = json_content(&result);
    assert_eq!(listing["executions"][0]["state"], "succeeded");
    assert_eq!(listing["executions"][0]["agent"], "explore");
    let serialized = serde_json::to_string(&listing).expect("string");
    assert!(
        !serialized.contains(SECRET_CHILD_ANSWER),
        "the successful answer is absent from the listing: {serialized}"
    );
    for withheld in [
        "detail",
        "observation",
        "activity",
        "last_activity_at",
        "counters",
        "profile",
        "history",
        "transcript",
        "content",
    ] {
        assert!(
            !serialized.contains(withheld),
            "a listing carries no {withheld}: {serialized}"
        );
    }
    // The answer did arrive, exactly once, on its own channel.
    let pending = plane
        .store
        .select_pending_batch()
        .expect("pending")
        .expect("one pending batch");
    assert_eq!(pending.items.len(), 1);
}

/// `execution(list)` and `execution(status)` project the same authoritative
/// lifecycle facts for the same subagent.
#[tokio::test]
async fn list_and_status_agree_about_a_subagent() {
    let plane = subagent_plane();
    let fixture = execution_fixture(Some(plane.registry.clone()));
    let _child = stage_exit0(&plane);
    let accepted = start_subagent(&plane, "inspect the tool plane").await;

    let listing = json_content(&run_execution(&fixture, list(&serde_json::json!({}))).await);
    let entry = &listing["executions"][0];
    let status = json_content(
        &run_execution(
            &fixture,
            serde_json::json!({
                "action": "status",
                "target": {"kind": "subagent", "id": accepted.subagent_id.to_string()},
            }),
        )
        .await,
    );
    assert_eq!(entry["state"], status["state"]);
    assert_eq!(entry["agent"], status["agent"]);
    assert_eq!(
        entry["publication_abandoned"],
        status["publication_abandoned"]
    );
    assert_eq!(entry["execution"], status["execution"]);
    assert_eq!(entry["execution"]["kind"], "subagent");
}

/// `active_only` follows the owning domain's own lifecycle classification:
/// a settled child leaves the active listing and stays in the default one.
#[tokio::test]
async fn list_active_only_excludes_settled_children() {
    let plane = subagent_plane();
    let fixture = execution_fixture(Some(plane.registry.clone()));
    let settling = stage_exit0(&plane);
    let settled_child = start_subagent(&plane, "settling child").await;
    settling
        .complete(ChildResultStatus::Succeeded, Some("done"))
        .await;
    plane
        .registry
        .wait_until_settled(&settled_child.subagent_id)
        .await
        .expect("settled");
    let _running = stage_exit0(&plane);
    let running_child = start_subagent(&plane, "running child").await;

    let active = json_content(
        &run_execution(&fixture, list(&serde_json::json!({"active_only": true}))).await,
    );
    assert_eq!(
        handles(&active),
        vec![("subagent", running_child.subagent_id.to_string())],
        "only the non-terminal child is active"
    );
    assert_eq!(active["matched"], 1);

    let all = json_content(&run_execution(&fixture, list(&serde_json::json!({}))).await);
    assert_eq!(
        handles(&all),
        vec![
            ("subagent", running_child.subagent_id.to_string()),
            ("subagent", settled_child.subagent_id.to_string()),
        ],
        "the default lists terminal children too"
    );
    assert_eq!(all["matched"], 2);
}

/// Listing does not consume, release, or otherwise disturb the subagent
/// domain's capacity accounting.
#[tokio::test]
async fn listing_never_changes_subagent_capacity_accounting() {
    let plane = subagent_plane();
    let fixture = execution_fixture(Some(plane.registry.clone()));
    // `subagent_plane` configures `max_active: 4`.
    let mut children = Vec::new();
    for ordinal in 0..4 {
        children.push(stage_exit0(&plane));
        start_subagent(&plane, &format!("child {ordinal}")).await;
    }

    let listing = json_content(&run_execution(&fixture, list(&serde_json::json!({}))).await);
    assert_eq!(listing["matched"], 4);

    // The bound is still exactly where it was: the fifth start is refused
    // for capacity, not admitted because a listing "released" anything.
    let _staged = stage_exit0(&plane);
    let prepared = plane
        .registry
        .prepare(&spec("one child too many"), &CancellationSignal::new())
        .await
        .expect("preparation still stages a child");
    let refused = plane
        .registry
        .commit(prepared, &CancellationSignal::new())
        .await
        .expect_err("the capacity bound still refuses the fifth child at commit");
    assert!(
        matches!(
            refused,
            rustx::runtime::subagent::SubagentStartError::CapacityExceeded { max: 4 }
        ),
        "listing changed no capacity accounting: {refused:?}"
    );

    // Settling one child frees exactly one slot, listing or not.
    let settling = children.remove(0);
    settling
        .complete(ChildResultStatus::Succeeded, Some("done"))
        .await;
    let first = plane
        .registry
        .all_snapshots()
        .into_iter()
        .next()
        .expect("the first child");
    plane
        .registry
        .wait_until_settled(&first.subagent_id)
        .await
        .expect("settled");
    let _after = run_execution(&fixture, list(&serde_json::json!({}))).await;
    let _staged = stage_exit0(&plane);
    let prepared = plane
        .registry
        .prepare(&spec("the replacement child"), &CancellationSignal::new())
        .await
        .expect("preparation stages the replacement");
    assert!(
        matches!(
            plane
                .registry
                .commit(prepared, &CancellationSignal::new())
                .await
                .expect("the freed slot admits exactly one replacement"),
            SubagentStartOutcome::Accepted(_)
        ),
        "the slot the settlement freed is the slot the replacement takes"
    );
}

/// One `execution(list)` invocation.
fn list(filter: &serde_json::Value) -> serde_json::Value {
    if filter.as_object().is_some_and(serde_json::Map::is_empty) {
        serde_json::json!({"action": "list"})
    } else {
        serde_json::json!({"action": "list", "filter": filter})
    }
}

/// The `(kind, id)` handle pairs of a listing, in response order.
fn handles(listing: &serde_json::Value) -> Vec<(&str, String)> {
    listing["executions"]
        .as_array()
        .expect("executions")
        .iter()
        .map(|entry| {
            (
                entry["execution"]["kind"].as_str().expect("handle kind"),
                entry["execution"]["id"]
                    .as_str()
                    .expect("handle id")
                    .to_owned(),
            )
        })
        .collect()
}

/// Dispatches two parking background executions (`exec_1`, `exec_2`) and
/// returns their release gates, so both records stay deterministically
/// active for the duration of the test.
async fn dispatch_parking_pair(
    fixture: &super::super::support::execution::ExecutionFixture,
) -> Vec<tokio::sync::watch::Sender<bool>> {
    let registry = fixture.runtime.background().clone();
    let mut gates = Vec::new();
    for _ in 0..2 {
        let (tool, release) = support::fake::FakeTool::parking(
            common::tool_policies(
                "bash",
                "tool-bash",
                rustx::tools::types::ToolExecutionPolicy::ModelSelectable,
                rustx::tools::types::ToolConcurrencyPolicy::Sequential,
            ),
            support::fake::success_result("done"),
        );
        let mut started = tool.started();
        let prepared = registry
            .prepare_dispatch(
                &background_invocation("bash"),
                &(Arc::new(tool) as Arc<dyn rustx::tools::executor::ToolExecutor>),
                rustx::tools::environment::ToolEnvironment::new(),
            )
            .expect("prepare");
        let outcome = registry
            .commit_dispatch(prepared, &CancellationSignal::new())
            .expect("dispatch commits");
        assert!(matches!(
            outcome,
            BackgroundDispatchOutcome::Accepted { .. }
        ));
        support::fake::await_started(&mut started, "parking background execution").await;
        gates.push(release);
    }
    gates
}
