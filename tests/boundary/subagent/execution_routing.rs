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
    start_with_spec(plane, spec(task)).await
}

/// The Workflow-owned `AgentRun` shape: the same child machinery, with the
/// reserved `workflow_output` terminal protocol and its frozen schema
/// (Issue #83). Its semantic input is owned by the compiled Workflow
/// program, which is why it is never steerable (Issue #193).
fn workflow_spec(task: &str) -> SubagentStartSpec {
    SubagentStartSpec {
        terminal: rustx::runtime::subagent::SubagentTerminalMode::WorkflowOutput {
            output_schema: serde_json::json!({
                "type": "object",
                "properties": {"summary": {"type": "string"}},
                "required": ["summary"],
                "additionalProperties": false
            }),
            workflow_id: rustx::runtime::workflow::WorkflowId::parse("test_workflow")
                .expect("workflow id"),
            run_id: ToolCallId::new("workflow-run"),
            node_id: "agent".to_owned(),
        },
        ..spec(task)
    }
}

async fn start_workflow_subagent(
    plane: &SubagentPlane,
    task: &str,
) -> rustx::runtime::subagent::SubagentAccepted {
    start_with_spec(plane, workflow_spec(task)).await
}

async fn start_with_spec(
    plane: &SubagentPlane,
    spec: SubagentStartSpec,
) -> rustx::runtime::subagent::SubagentAccepted {
    let prepared = plane
        .registry
        .prepare(&spec, &CancellationSignal::new())
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
    let result = crate::tools::native::subagent::accepted_result(&accepted);
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

    // Exactly one canonical inbound publication carries the answer: the
    // runtime-authored correlation notice first, then the report authored
    // by the child agent (Issue #192).
    let pending = plane
        .store
        .select_pending_batch()
        .expect("pending")
        .expect("one pending batch");
    assert_eq!(
        pending.items.len(),
        2,
        "the correlation notice and the terminal report"
    );
    assert!(matches!(
        pending.items[0].message.source,
        rustx::message::types::UserSource::Runtime
    ));
    assert_eq!(
        pending.items[0].message.id.as_str(),
        crate::runtime::subagent::terminal_notice_message_id(&accepted.subagent_id).as_str()
    );
    let item = &pending.items[1];
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
    // still holds exactly one publication carrying the answer.
    let pending = plane
        .store
        .select_pending_batch()
        .expect("pending")
        .expect("one pending batch");
    assert_eq!(pending.items.len(), 2);
    let text = pending.items[1]
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
        2,
        "exactly one publication per child: the runtime-authored notice and the final report"
    );
    let notice_text = pending.items[0]
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
        !notice_text.contains("intermediate reasoning noise")
            && !notice_text.contains("FINAL-REPORT-ONLY"),
        "the runtime-authored notice carries neither intermediate traffic nor the report: {notice_text}"
    );
    let text = pending.items[1]
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
/// correlated **at the parent-model boundary** (Issue #192).
///
/// This is the provider-neutral model-request regression: both children are
/// settled out of start order, their publications are admitted into the
/// canonical Ledger through the real adoption transition, and the
/// provider-neutral model input of the parent's next request is assembled
/// through the exact `assemble_model_input` seam the runtime uses. The
/// proof is on what the parent model actually receives:
///
/// - every child-authored report is immediately preceded by exactly one
///   runtime-authored terminal notice naming the typed execution handle
///   (`{"kind":"subagent","id":...}`) that child's creation result
///   returned, so `SECOND-ANSWER` correlates to execution B and
///   `FIRST-ANSWER` to execution A even though both children are the same
///   named agent;
/// - the report text is byte-for-byte the child's own answer;
/// - no internal runtime identity (child agent id, child conversation id,
///   definition digest, delegating tool call, physical workspace fact)
///   crosses the model boundary.
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

    // The exact model-facing creation results: the typed handles the
    // parent model holds for the two executions.
    let creation_handle = |accepted: &rustx::runtime::subagent::SubagentAccepted| {
        json_content(&crate::tools::native::subagent::accepted_result(accepted))["execution"]
            .clone()
    };
    let first_handle = creation_handle(&first);
    let second_handle = creation_handle(&second);
    assert_eq!(
        first_handle,
        serde_json::json!({"kind": "subagent", "id": first.subagent_id.to_string()})
    );
    assert_eq!(
        second_handle,
        serde_json::json!({"kind": "subagent", "id": second.subagent_id.to_string()})
    );

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

    // Admit the publications into the canonical Ledger through the real
    // adoption transition — the parent's next model turn boundary.
    let batch = plane
        .store
        .select_pending_batch()
        .expect("pending")
        .expect("one pending batch");
    assert_eq!(
        batch.items.len(),
        4,
        "each successful child published its runtime-authored notice and its report"
    );
    let adopted = plane
        .store
        .adopt_pending_batch(batch.watermark, batch.adoption_event(None))
        .expect("adoption");
    assert_eq!(adopted.len(), 4);

    // Build the provider-neutral model input of the parent's subsequent
    // request through the exact assembly seam the runtime uses.
    let canonical = plane.store.load_canonical().expect("canonical");
    let model_input = crate::model::input::assemble_model_input(&canonical, &[], None, None)
        .expect("the provider-neutral model input assembles");
    let model_visible: Vec<&rustx::message::types::UserMessageBlock> = model_input
        .iter()
        .filter_map(|message| match message {
            rustx::model::ModelInputMessage::Canonical(
                rustx::message::types::MessageBlock::User(user),
            ) => Some(user),
            _ => None,
        })
        .collect();
    let text_of = |message: &rustx::message::types::UserMessageBlock| {
        message
            .content
            .iter()
            .filter_map(|block| match block {
                rustx::message::types::UserContentBlock::Text(text) => Some(text.text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    assert_eq!(
        model_visible.len(),
        4,
        "the parent model receives exactly the two notice/report pairs"
    );

    // Settlement order: B's pair first, then A's pair. Each pair is the
    // runtime-authored correlation notice immediately followed by the
    // unchanged child-authored report.
    let pairs = [
        (
            &model_visible[0],
            &model_visible[1],
            &second,
            &second_handle,
            "SECOND-ANSWER",
        ),
        (
            &model_visible[2],
            &model_visible[3],
            &first,
            &first_handle,
            "FIRST-ANSWER",
        ),
    ];
    for (notice, report, accepted, handle, answer) in pairs {
        let notice_text = text_of(notice);
        let report_text = text_of(report);
        assert!(
            matches!(notice.source, rustx::message::types::UserSource::Runtime),
            "the correlation notice is runtime-authored"
        );
        assert!(
            matches!(
                report.source,
                rustx::message::types::UserSource::Agent { ref agent_id }
                    if *agent_id == accepted.child_agent_id
            ),
            "the report is child-authored"
        );
        assert!(
            notice_text.contains(&serde_json::to_string(handle).expect("handle serializes")),
            "the notice names the exact typed execution handle the creation result returned: \
             {notice_text}"
        );
        assert!(
            notice_text.contains("explore"),
            "the notice names the agent: {notice_text}"
        );
        assert_eq!(
            report_text, answer,
            "the child report is byte-for-byte child-authored"
        );
        assert!(
            !report_text.contains("Subagent execution"),
            "no runtime metadata is concatenated into the child report: {report_text}"
        );
    }
    // Unambiguity: each notice names only its own execution's handle.
    assert!(!text_of(model_visible[0]).contains(&first.subagent_id.to_string()));
    assert!(!text_of(model_visible[2]).contains(&second.subagent_id.to_string()));

    // Nothing internal crosses the model boundary. The child conversation
    // identity is string-identical to the execution id by construction, so
    // the id may appear only inside the exact handle JSON — never as a
    // separate internal field — and the genuinely distinct internal
    // identities (child agent id, definition digest, delegating tool call)
    // never appear at all.
    let pair_handles = [
        (&model_visible[0], &second_handle),
        (&model_visible[1], &second_handle),
        (&model_visible[2], &first_handle),
        (&model_visible[3], &first_handle),
    ];
    for (message, handle) in pair_handles {
        let text = text_of(message);
        let handle_json = serde_json::to_string(handle).expect("handle serializes");
        let beyond_handle = text.replacen(&handle_json, "", 1);
        for accepted in [&first, &second] {
            assert!(
                !beyond_handle.contains(&accepted.subagent_id.to_string()),
                "the execution id appears only as the typed handle: {text}"
            );
            for internal in [
                accepted.child_agent_id.to_string(),
                accepted.definition_digest.clone(),
            ] {
                assert!(
                    !text.contains(&internal),
                    "internal identity {internal} must not cross the model boundary: {text}"
                );
            }
        }
        for internal in [
            "call-162",
            "definition_digest",
            "child_agent_id",
            "child_conversation_id",
            "tool_call_id",
            "sha256:",
            "workspace",
        ] {
            assert!(
                !text.contains(internal),
                "runtime provenance {internal:?} must not cross the model boundary: {text}"
            );
        }
    }

    // Each status response is identified by exactly the handle the model
    // holds for that child, and never carries a report.
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

    // The result channel is still exactly the canonical durable inbound:
    // the runtime-authored notice, then the report.
    let pending = plane
        .store
        .select_pending_batch()
        .expect("pending")
        .expect("one pending batch");
    assert_eq!(pending.items.len(), 2);
    let text = pending.items[1]
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

    // And the child still settles exactly once, through the canonical path
    // (notice plus report, Issue #192).
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
        2,
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
    // The answer did arrive, exactly once, on its own channel — the
    // notice/report pair of the one terminal publication.
    let pending = plane
        .store
        .select_pending_batch()
        .expect("pending")
        .expect("one pending batch");
    assert_eq!(pending.items.len(), 2);
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

// ---------------------------------------------------------------------------
// In-flight steering (Issue #193)
// ---------------------------------------------------------------------------

/// The child half of a steer, driven explicitly by the test.
///
/// The real child conversation is the durable acceptance authority; here the
/// test *is* that authority, so every interleaving below is established by
/// the test deciding when — and whether — the answer is written, never by
/// elapsed time. The composed proofs that a real child conversation accepts,
/// orders, and observes the guidance live in
/// [`super::conformance`].
impl ScriptedChild {
    /// Reads the next parent-bound frame.
    async fn read_frame(&mut self) -> crate::runtime::subagent::ipc::ParentFrame {
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            crate::runtime::subagent::ipc::read_parent_frame(&mut self.peer),
        )
        .await
        .expect("driver control liveness")
        .expect("driver frame")
        .expect("parent frame")
    }

    /// Consumes the delegation frame the committed child always receives
    /// first.
    async fn take_delegation(&mut self) {
        assert!(
            matches!(
                self.read_frame().await,
                crate::runtime::subagent::ipc::ParentFrame::Delegate(_)
            ),
            "the committed child is delegated first"
        );
    }

    /// Reads exactly one guidance envelope and answers it with `outcome`.
    async fn answer_guidance(
        &mut self,
        outcome: crate::runtime::subagent::ipc::ChildGuidanceOutcome,
    ) -> String {
        let crate::runtime::subagent::ipc::ParentFrame::Guidance(guidance) =
            self.read_frame().await
        else {
            panic!("the steer routes exactly one guidance envelope to this child");
        };
        crate::runtime::subagent::ipc::write_child_frame(
            &mut self.peer,
            &ChildFrame::GuidanceResult(crate::runtime::subagent::ipc::GuidanceResultFrame {
                guidance_id: guidance.guidance_id,
                outcome,
            }),
        )
        .await
        .expect("guidance answer");
        guidance.message
    }

    /// Awaits the cancellation frame, then answers with one terminal
    /// result. Reading the `Cancel` frame first is what makes the child's
    /// terminal well defined: it establishes that the driver has delivered
    /// the cancellation before the peer answers and closes.
    async fn cancelled_after_delegation(
        mut self,
        status: ChildResultStatus,
        content: Option<&str>,
    ) {
        self.take_delegation().await;
        assert!(
            matches!(
                self.read_frame().await,
                crate::runtime::subagent::ipc::ParentFrame::Cancel { .. }
            ),
            "the cancellation reaches the child before it answers"
        );
        self.send_result(status, content).await;
    }

    /// Sends the terminal result frame.
    async fn send_result(&mut self, status: ChildResultStatus, content: Option<&str>) {
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

fn accepted() -> crate::runtime::subagent::ipc::ChildGuidanceOutcome {
    crate::runtime::subagent::ipc::ChildGuidanceOutcome::Accepted
}

/// A successful `execution(steer)` returns exactly the model-actionable
/// acknowledgement: the canonical handle the caller named, the lifecycle
/// state, and the acceptance fact. It is a control acknowledgement, never a
/// result channel.
#[tokio::test]
async fn steer_returns_the_minimal_control_acknowledgement() {
    let plane = subagent_plane();
    let mut child = stage_exit0(&plane);
    let accepted_child = start_subagent(&plane, "survey the cancellation plane").await;
    let fixture = execution_fixture(Some(plane.registry.clone()));

    let child_side = tokio::spawn(async move {
        child.take_delegation().await;
        let message = child.answer_guidance(accepted()).await;
        (child, message)
    });
    let result = run_execution(
        &fixture,
        serde_json::json!({
            "action": "steer",
            "target": {"kind": "subagent", "id": accepted_child.subagent_id.to_string()},
            "message": "Focus on cancellation ownership and ignore TUI code.",
        }),
    )
    .await;
    let (_child, delivered) = child_side.await.expect("child side");

    assert_eq!(
        json_content(&result),
        serde_json::json!({
            "execution": {"kind": "subagent", "id": "conv-162-subagent-1"},
            "state": "running",
            "accepted": true,
        }),
        "the steer acknowledgement is exactly the minimal control contract"
    );
    assert_eq!(
        delivered, "Focus on cancellation ownership and ignore TUI code.",
        "the parent-authored message crosses unchanged"
    );
    let serialized = serde_json::to_string(&json_content(&result)).expect("serializes");
    for leaked in [
        "content",
        "answer",
        "history",
        "transcript",
        "detail",
        "message",
    ] {
        assert!(
            !serialized.contains(leaked),
            "a steer acknowledgement never carries child content: {leaked} in {serialized}"
        );
    }
}

/// `kind = tool` + `action = steer` is an unsupported kind/action
/// combination, refused by explicit dispatch **before any authority is
/// consulted**.
///
/// The proof is structural rather than textual: the id named is a *live
/// subagent's* id, so a registry fall-through in either direction would have
/// found a steerable execution. The child's control wire is then read and
/// proven to carry the delegation followed immediately by the guidance
/// envelope of the *second*, correctly-kinded call — never one from the
/// refused call.
#[tokio::test]
async fn steer_of_a_tool_target_never_falls_through_to_the_subagent_registry() {
    let plane = subagent_plane();
    let mut child = stage_exit0(&plane);
    let accepted_child = start_subagent(&plane, "survey the tool plane").await;
    let fixture = execution_fixture(Some(plane.registry.clone()));
    let id = accepted_child.subagent_id.to_string();

    let refused = run_execution(
        &fixture,
        serde_json::json!({
            "action": "steer",
            "target": {"kind": "tool", "id": id.clone()},
            "message": "this must never reach the subagent domain",
        }),
    )
    .await;
    let message = failure_message(&refused);
    assert!(
        message.contains("not supported for kind \"tool\""),
        "the refusal names the unsupported kind/action combination: {message}"
    );

    // The subagent registry was never consulted: the very next envelope on
    // the child's wire is the correctly-kinded steer, not the refused one.
    let child_side = tokio::spawn(async move {
        child.take_delegation().await;
        let message = child.answer_guidance(accepted()).await;
        (child, message)
    });
    let accepted_result = run_execution(
        &fixture,
        serde_json::json!({
            "action": "steer",
            "target": {"kind": "subagent", "id": id},
            "message": "the only guidance this child ever receives",
        }),
    )
    .await;
    let (_child, delivered) = child_side.await.expect("child side");
    assert_eq!(
        json_content(&accepted_result)["accepted"],
        serde_json::json!(true)
    );
    assert_eq!(delivered, "the only guidance this child ever receives");
}

/// Every deterministic steer refusal of the model-facing boundary and the
/// domain authority. A steer is never silently ignored and never reports
/// `accepted` without a durable acceptance.
#[tokio::test]
async fn steer_refusals_are_deterministic_and_bounded() {
    let plane = subagent_plane();
    let mut child = stage_exit0(&plane);
    let accepted_child = start_subagent(&plane, "survey").await;
    let fixture = execution_fixture(Some(plane.registry.clone()));
    let id = accepted_child.subagent_id.to_string();
    child.take_delegation().await;

    // An unknown id is an ordinary failed result of the owning domain.
    let unknown = run_execution(
        &fixture,
        serde_json::json!({
            "action": "steer",
            "target": {"kind": "subagent", "id": "conv-162-subagent-99"},
            "message": "nobody is listening",
        }),
    )
    .await;
    assert_eq!(
        failure_message(&unknown),
        "unknown subagent execution conv-162-subagent-99"
    );

    // An empty or whitespace-only message is refused before any routing.
    for empty in ["", "   \n\t "] {
        let refused = run_execution(
            &fixture,
            serde_json::json!({
                "action": "steer",
                "target": {"kind": "subagent", "id": id.clone()},
                "message": empty,
            }),
        )
        .await;
        assert!(
            failure_message(&refused).contains("must be non-empty"),
            "an empty steer message is a bounded refusal"
        );
    }

    // A message beyond the delegated-task bound is refused with its length.
    let oversized = "x".repeat(32 * 1024 + 1);
    let refused = run_execution(
        &fixture,
        serde_json::json!({
            "action": "steer",
            "target": {"kind": "subagent", "id": id.clone()},
            "message": oversized,
        }),
    )
    .await;
    assert!(
        failure_message(&refused).contains("32769 given"),
        "the bound refusal names the offending length"
    );

    // Malformed targets and missing fields never reach an executor at all:
    // they are canonical-schema violations, proven at the input contract in
    // the intrinsic's own unit suite.

    // Nothing above reached the child: its wire still carries no envelope,
    // proven by the next steer being the first guidance it ever sees.
    let child_side = tokio::spawn(async move {
        let message = child.answer_guidance(accepted()).await;
        (child, message)
    });
    let ok = run_execution(
        &fixture,
        serde_json::json!({
            "action": "steer",
            "target": {"kind": "subagent", "id": id},
            "message": "the first envelope",
        }),
    )
    .await;
    let (_child, delivered) = child_side.await.expect("child side");
    assert_eq!(json_content(&ok)["accepted"], serde_json::json!(true));
    assert_eq!(delivered, "the first envelope");
}

/// A steer that loses the race to the registry's cancellation linearization
/// point is refused deterministically, and no steer can move a cancelling or
/// cancelled child back toward `Running`.
///
/// The interleaving is established by the cancellation boundary hook: the
/// cancellation is parked immediately before the registry mutex that commits
/// `Running -> Cancelling`, the steer that acquires that mutex while the
/// cancellation is still parked is admitted, and the steer issued after the
/// commit is refused by the very same mutex. No sleep is involved.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancellation_intent_and_steer_share_one_arbitration_boundary() {
    let plane = subagent_plane();
    let mut child = stage_exit0(&plane);
    let accepted_child = start_subagent(&plane, "survey").await;
    let fixture = execution_fixture(Some(plane.registry.clone()));
    let id = accepted_child.subagent_id.to_string();
    child.take_delegation().await;

    let hook = Arc::new(crate::runtime::subagent::CancellationBoundaryHook::default());
    plane
        .registry
        .install_cancellation_boundary_hook(hook.clone());
    let cancelling = tokio::task::spawn_blocking({
        let registry = plane.registry.clone();
        let subagent_id = accepted_child.subagent_id.clone();
        move || {
            registry.cancel(
                &subagent_id,
                rustx::runtime::types::CancellationReason::UserRequested,
            )
        }
    });
    // The cancellation is provably parked at the exact pre-commit edge: the
    // record is still `Running` and the steer below therefore races the
    // authority boundary itself, not an arbitrary earlier instant.
    tokio::task::spawn_blocking({
        let hook = hook.clone();
        move || hook.wait_until_parked()
    })
    .await
    .expect("cancellation parks at the boundary");

    let child_side = tokio::spawn(async move {
        let message = child.answer_guidance(accepted()).await;
        (child, message)
    });
    let won = run_execution(
        &fixture,
        serde_json::json!({
            "action": "steer",
            "target": {"kind": "subagent", "id": id.clone()},
            "message": "accepted before the cancellation intent commits",
        }),
    )
    .await;
    let (_child, delivered) = child_side.await.expect("child side");
    assert_eq!(
        json_content(&won),
        serde_json::json!({
            "execution": {"kind": "subagent", "id": "conv-162-subagent-1"},
            "state": "running",
            "accepted": true,
        }),
        "the steer that wins the boundary is durably accepted while Running"
    );
    assert_eq!(delivered, "accepted before the cancellation intent commits");

    // Release the parked cancellation and let it commit the intent.
    hook.release();
    let snapshot = cancelling.await.expect("cancellation").expect("record");
    assert_eq!(snapshot.state, SubagentState::Cancelling);

    // Every later steer is refused by the same mutex that committed the
    // intent, and the refusal names cancellation explicitly.
    let refused = run_execution(
        &fixture,
        serde_json::json!({
            "action": "steer",
            "target": {"kind": "subagent", "id": id},
            "message": "must never resurrect a cancelling child",
        }),
    )
    .await;
    let message = failure_message(&refused);
    assert!(
        message.contains("cancellation intent is committed"),
        "a committed cancellation intent refuses every later steer: {message}"
    );
    assert_eq!(
        plane
            .registry
            .snapshot(&accepted_child.subagent_id)
            .expect("record")
            .state,
        SubagentState::Cancelling,
        "a refused steer mutates no lifecycle state: Cancelling never becomes Running"
    );
}

/// A steer that loses the race to the registry's terminal-authority
/// linearization point is refused deterministically, and one that wins it is
/// accepted before any terminal fact exists.
///
/// The interleaving is established by the terminal authority hook: the
/// settlement path is parked immediately before the registry mutex that
/// creates the terminal candidate and commits `... -> PublishingTerminal`.
/// A steer taken while it is parked provably precedes the terminal commit; a
/// steer taken after the released settlement provably follows it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn terminal_authority_and_steer_share_one_arbitration_boundary() {
    let plane = subagent_plane();
    let mut child = stage_exit0(&plane);
    let accepted_child = start_subagent(&plane, "survey").await;
    let fixture = execution_fixture(Some(plane.registry.clone()));
    let id = accepted_child.subagent_id.to_string();
    child.take_delegation().await;

    let hook = Arc::new(crate::runtime::subagent::TerminalAuthorityHook::default());
    plane.registry.install_terminal_authority_hook(hook.clone());

    // The child answers one steer, then reports its terminal result. The
    // settlement path parks at the terminal authority boundary.
    let child_side = tokio::spawn(async move {
        let message = child.answer_guidance(accepted()).await;
        child
            .send_result(ChildResultStatus::Succeeded, Some("done"))
            .await;
        (child, message)
    });
    let won = run_execution(
        &fixture,
        serde_json::json!({
            "action": "steer",
            "target": {"kind": "subagent", "id": id.clone()},
            "message": "accepted before terminal authority commits",
        }),
    )
    .await;
    let (_child, delivered) = child_side.await.expect("child side");
    assert_eq!(json_content(&won)["accepted"], serde_json::json!(true));
    assert_eq!(delivered, "accepted before terminal authority commits");

    // The settlement path is provably parked at the pre-commit edge of the
    // terminal authority: no terminal fact exists yet.
    tokio::task::spawn_blocking({
        let hook = hook.clone();
        move || hook.wait_until_entered()
    })
    .await
    .expect("terminal settlement parks at the boundary");
    assert_eq!(
        plane
            .registry
            .snapshot(&accepted_child.subagent_id)
            .expect("record")
            .state,
        SubagentState::Running,
        "the parked settlement has not committed any terminal transition"
    );

    hook.release();
    let settled = plane
        .registry
        .wait_until_settled(&accepted_child.subagent_id)
        .await
        .expect("terminal settlement");
    assert_eq!(settled.state, SubagentState::Succeeded);

    // Terminal authority won: every later steer is refused by the same
    // mutex, naming the settled lifecycle.
    let refused = run_execution(
        &fixture,
        serde_json::json!({
            "action": "steer",
            "target": {"kind": "subagent", "id": id},
            "message": "must never reopen a settled child",
        }),
    )
    .await;
    let message = failure_message(&refused);
    assert!(
        message.contains("no longer running"),
        "a settled child refuses every later steer: {message}"
    );
    assert_eq!(
        plane
            .registry
            .snapshot(&accepted_child.subagent_id)
            .expect("record")
            .state,
        SubagentState::Succeeded,
        "terminal states stay absorbing: Succeeded never becomes Running"
    );
}

/// A child that settles without answering the guidance envelope refuses it:
/// the driver task drops the waiter at settlement, and the registry reports
/// that deterministically rather than optimistically claiming acceptance.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_child_that_settles_without_answering_refuses_the_steer() {
    let plane = subagent_plane();
    let mut child = stage_exit0(&plane);
    let accepted_child = start_subagent(&plane, "survey").await;
    let fixture = execution_fixture(Some(plane.registry.clone()));
    child.take_delegation().await;

    let child_side = tokio::spawn(async move {
        // Read the envelope and settle without ever answering it.
        let crate::runtime::subagent::ipc::ParentFrame::Guidance(_) = child.read_frame().await
        else {
            panic!("the steer routes one guidance envelope");
        };
        child
            .send_result(ChildResultStatus::Succeeded, Some("done"))
            .await;
        child
    });
    let refused = run_execution(
        &fixture,
        serde_json::json!({
            "action": "steer",
            "target": {"kind": "subagent", "id": accepted_child.subagent_id.to_string()},
            "message": "never answered",
        }),
    )
    .await;
    let _child = child_side.await.expect("child side");
    let message = failure_message(&refused);
    assert!(
        message.contains("settled before the guidance was accepted"),
        "an unanswered envelope is a refusal, never an optimistic acceptance: {message}"
    );
}

/// **The critical cancellation/steer race (Issue #193 review finding #1).**
///
/// This is the interleaving the ticket design exists for, and it is the one
/// that transport FIFO ordering cannot decide:
///
/// 1. the steer passes parent-side registry admission and its envelope is
///    handed to the driver;
/// 2. the child has **not** durably accepted it yet;
/// 3. the parent's cancellation intent commits;
/// 4. only then is the child's acceptance answer produced.
///
/// Every one of those four points is a proven synchronization point here,
/// not a hope:
///
/// - **(1) handed off from the registry**: the test *is* the child, and it
///   reads the `Guidance` frame off the real control socket. A frame it has
///   read provably left the registry's admission critical section.
/// - **(2) not yet durably accepted**: the test holds the envelope without
///   writing any `GuidanceResult`. The parent's `execution(steer)` call is
///   still parked on the answer, so no acceptance exists anywhere.
/// - **(3) cancellation committed**: `cancel` returns the authoritative
///   snapshot, and the assertion on `Cancelling` is the commit itself.
/// - **(4) acceptance attempted afterwards**: only now does the child answer
///   — and it answers **`Accepted`**, deliberately. That is the adversarial
///   case: even a child that durably committed the guidance cannot make this
///   steer accepted, because the registry's cancellation linearization point
///   dropped its ticket. The child-side refusal is proven separately by
///   `guidance_after_the_committed_cancellation_intent_is_refused`; this test
///   proves the parent never reports acceptance *regardless* of what the
///   child did.
///
/// The linearization point being proven is the registry mutex: it totally
/// orders steer admission, the `Running -> Cancelling` commit, and the steer
/// commit, and a steer is accepted only if its ticket survived from the
/// first to the third.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cancellation_committed_before_child_acceptance_refuses_the_in_flight_steer() {
    let plane = subagent_plane();
    let mut child = stage_exit0(&plane);
    let accepted_child = start_subagent(&plane, "survey").await;
    let fixture = execution_fixture(Some(plane.registry.clone()));
    let id = accepted_child.subagent_id.to_string();
    child.take_delegation().await;

    // (1)+(2): the child receives the envelope and deliberately withholds
    // the answer. `answered` is the barrier the test releases in step (4).
    let (release_answer, answer_now) = tokio::sync::oneshot::channel::<()>();
    let (envelope_read, envelope_is_held) = tokio::sync::oneshot::channel::<()>();
    let child_side = tokio::spawn(async move {
        let crate::runtime::subagent::ipc::ParentFrame::Guidance(guidance) =
            child.read_frame().await
        else {
            panic!("the steer routes exactly one guidance envelope to this child");
        };
        envelope_read
            .send(())
            .expect("the test observes the held envelope");
        answer_now.await.expect("the test releases the answer");
        crate::runtime::subagent::ipc::write_child_frame(
            &mut child.peer,
            &ChildFrame::GuidanceResult(crate::runtime::subagent::ipc::GuidanceResultFrame {
                guidance_id: guidance.guidance_id,
                // Adversarial: the child claims durable acceptance.
                outcome: crate::runtime::subagent::ipc::ChildGuidanceOutcome::Accepted,
            }),
        )
        .await
        .expect("guidance answer");
        child
    });
    let steering = run_execution(
        &fixture,
        serde_json::json!({
            "action": "steer",
            "target": {"kind": "subagent", "id": id.clone()},
            "message": "routed before the cancellation, answered after it",
        }),
    );
    tokio::pin!(steering);
    // The steer future is polled until it is provably parked on the child's
    // answer: the envelope has been read by the child side, and the call has
    // not returned.
    let steering = tokio::select! {
        _ = &mut steering => panic!("the steer cannot settle before the child answers"),
        held = envelope_is_held => {
            held.expect("the child holds the routed envelope");
            steering
        }
    };

    // (3): the cancellation intent commits while the steer is outstanding.
    let cancelled = plane
        .registry
        .cancel(
            &accepted_child.subagent_id,
            rustx::runtime::types::CancellationReason::UserRequested,
        )
        .expect("known child");
    assert_eq!(
        cancelled.state,
        SubagentState::Cancelling,
        "the cancellation intent is committed before the child answers"
    );

    // (4): only now does the child answer, and it answers `Accepted`.
    release_answer
        .send(())
        .expect("the child answers after the cancellation commit");
    let outcome = steering.await;
    let _child = child_side.await.expect("child side");

    // (5)+(6): the steer cannot become accepted.
    assert!(
        matches!(outcome.status, ToolExecutionStatus::Failed { .. }),
        "a steer that raced a committed cancellation is refused, never accepted: {:?}",
        outcome.status
    );
    let message = failure_message(&outcome);
    assert!(
        message.contains("cancellation intent is committed"),
        "the refusal names the cancellation authority that won: {message}"
    );

    // (7)+(8): no resurrection. The child stays on its cancellation path and
    // the refused steer mutated no lifecycle state.
    let snapshot = plane
        .registry
        .snapshot(&accepted_child.subagent_id)
        .expect("record");
    assert_eq!(
        snapshot.state,
        SubagentState::Cancelling,
        "a refused steer never moves a cancelling child back toward running"
    );
    assert_eq!(
        snapshot.cancel_reason,
        Some(rustx::runtime::types::CancellationReason::UserRequested),
        "the committed cancellation cause is untouched by the refused steer"
    );
}

/// **A cancellation *after* an accepted steer (Issue #193 contract).**
///
/// The documented contract is asymmetric on purpose, and this test pins the
/// second half of it: a steer whose ticket survived to its commit is
/// reported `accepted: true`, and a *later* cancellation still terminates
/// the child. The acknowledgement therefore never claims the child observed
/// the guidance — it claims only that the guidance was durably in the
/// child's conversation with no cancellation committed up to that point.
///
/// The ordering is established by the parent's own return value: the steer
/// call has returned before `cancel` is invoked, so the acceptance provably
/// precedes the cancellation commit.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cancellation_after_an_accepted_steer_still_settles_the_child_cancelled() {
    let plane = subagent_plane();
    let mut child = stage_exit0(&plane);
    let accepted_child = start_subagent(&plane, "survey").await;
    let fixture = execution_fixture(Some(plane.registry.clone()));
    let id = accepted_child.subagent_id.to_string();
    child.take_delegation().await;

    let child_side = tokio::spawn(async move {
        let message = child.answer_guidance(accepted()).await;
        (child, message)
    });
    let accepted_steer = run_execution(
        &fixture,
        serde_json::json!({
            "action": "steer",
            "target": {"kind": "subagent", "id": id.clone()},
            "message": "accepted, then superseded by cancellation",
        }),
    )
    .await;
    let (mut child, delivered) = child_side.await.expect("child side");
    assert_eq!(
        json_content(&accepted_steer)["accepted"],
        serde_json::json!(true),
        "the steer committed before any cancellation intent existed"
    );
    assert_eq!(delivered, "accepted, then superseded by cancellation");

    // The later cancellation is authoritative: it supersedes the accepted
    // guidance rather than waiting for it to be observed.
    let cancelled = plane
        .registry
        .cancel(
            &accepted_child.subagent_id,
            rustx::runtime::types::CancellationReason::UserRequested,
        )
        .expect("known child");
    assert_eq!(cancelled.state, SubagentState::Cancelling);
    assert!(
        matches!(
            child.read_frame().await,
            crate::runtime::subagent::ipc::ParentFrame::Cancel { .. }
        ),
        "the cancellation reaches the child that holds the accepted guidance"
    );
    child.send_result(ChildResultStatus::Cancelled, None).await;
    let settled = plane
        .registry
        .wait_until_settled(&accepted_child.subagent_id)
        .await
        .expect("settled");
    assert_eq!(
        settled.state,
        SubagentState::Cancelled,
        "cancellation stays authoritative over an already accepted steer"
    );
    assert_eq!(
        settled.cancel_reason,
        Some(rustx::runtime::types::CancellationReason::UserRequested)
    );

    // The one result channel published no answer: an accepted steer never
    // turns a cancelled child into a reporting one.
    let pending = plane.store.select_pending_batch().expect("pending");
    let answers: Vec<String> = pending
        .into_iter()
        .flat_map(|batch| batch.items)
        .filter_map(|item| match item.message.content.first() {
            Some(rustx::message::types::UserContentBlock::Text(text)) => Some(text.text.clone()),
            _ => None,
        })
        .collect();
    assert!(
        answers.iter().all(|text| !text.contains("accepted, then")),
        "no answer derived from the superseded guidance is ever published: {answers:?}"
    );
}

/// **Workflow ownership (Issue #193 review finding #3).**
///
/// A Workflow-owned `AgentRun` is refused deterministically by the subagent
/// domain authority itself, in every lifecycle state, and the refusal is
/// decided **before any guidance envelope exists**. That last part is what
/// the second half of this test proves: after the refusal, the very next
/// frame the child receives is the `Cancel` frame, so no `Guidance` frame
/// was ever written to it.
///
/// Cancel symmetry is not steer symmetry: cancel is lifecycle control the
/// parent runtime owns for every child it supervises, while steer is
/// semantic conversation authorship, which for these children belongs to the
/// compiled Workflow program.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_workflow_owned_child_refuses_steering_before_any_guidance_frame_exists() {
    let plane = subagent_plane();
    let mut child = stage_exit0(&plane);
    let accepted_child = start_workflow_subagent(&plane, "produce the node output").await;
    let fixture = execution_fixture(Some(plane.registry.clone()));
    let id = accepted_child.subagent_id.to_string();
    child.take_delegation().await;

    let refused = run_execution(
        &fixture,
        serde_json::json!({
            "action": "steer",
            "target": {"kind": "subagent", "id": id},
            "message": "the workflow owns this child's instructions",
        }),
    )
    .await;
    assert!(matches!(refused.status, ToolExecutionStatus::Failed { .. }));
    let message = failure_message(&refused);
    assert!(
        message.contains("steering is not supported for workflow-owned subagent executions"),
        "the refusal names Workflow ownership, not a lifecycle accident: {message}"
    );
    assert_eq!(
        plane
            .registry
            .snapshot(&accepted_child.subagent_id)
            .expect("record")
            .state,
        SubagentState::Running,
        "the refusal mutates no lifecycle state"
    );

    // No Guidance frame was written: the next frame this child sees is the
    // cancellation, which is written only now.
    plane
        .registry
        .cancel(
            &accepted_child.subagent_id,
            rustx::runtime::types::CancellationReason::UserRequested,
        )
        .expect("known child");
    assert!(
        matches!(
            child.read_frame().await,
            crate::runtime::subagent::ipc::ParentFrame::Cancel { .. }
        ),
        "the refused steer never reached the child's control channel"
    );
}

/// Workflow-owned children remain fully cancellable, and their frozen
/// structured output contract is untouched by the steering restriction: the
/// same child that refuses every steer still settles through the ordinary
/// Workflow terminal path with its validated output.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_workflow_owned_child_keeps_its_cancellation_and_output_contract() {
    // Cancellation half.
    let plane = subagent_plane();
    let child = stage_exit0(&plane);
    let cancelled_child = start_workflow_subagent(&plane, "cancelled node").await;
    plane
        .registry
        .cancel(
            &cancelled_child.subagent_id,
            rustx::runtime::types::CancellationReason::UserRequested,
        )
        .expect("known child");
    child
        .cancelled_after_delegation(ChildResultStatus::Cancelled, None)
        .await;
    let settled = plane
        .registry
        .wait_until_settled(&cancelled_child.subagent_id)
        .await
        .expect("settled");
    assert_eq!(
        settled.state,
        SubagentState::Cancelled,
        "the steering restriction does not weaken Workflow cancellation"
    );
    assert_eq!(
        settled.cancel_reason,
        Some(rustx::runtime::types::CancellationReason::UserRequested)
    );

    // Output-contract half: a second Workflow child, steered at and refused,
    // still settles with its validated structured output.
    let other = subagent_plane_for("conv-193-workflow-output");
    let mut child = stage_exit0(&other);
    let running = start_workflow_subagent(&other, "answering node").await;
    let fixture = execution_fixture(Some(other.registry.clone()));
    child.take_delegation().await;
    let refused = run_execution(
        &fixture,
        serde_json::json!({
            "action": "steer",
            "target": {"kind": "subagent", "id": running.subagent_id.to_string()},
            "message": "refused",
        }),
    )
    .await;
    assert!(matches!(refused.status, ToolExecutionStatus::Failed { .. }));
    child
        .send_result(
            ChildResultStatus::Succeeded,
            Some(r#"{"summary":"the node answer"}"#),
        )
        .await;
    let settled = other
        .registry
        .wait_until_settled(&running.subagent_id)
        .await
        .expect("settled");
    assert_eq!(
        settled.state,
        SubagentState::Succeeded,
        "the Workflow child's own terminal path is unchanged"
    );
    assert_eq!(
        other
            .registry
            .workflow_agent_output(&running.subagent_id)
            .expect("the validated Workflow output is still delivered"),
        serde_json::json!({"summary": "the node answer"}),
        "the frozen output schema contract is untouched"
    );
}

/// A normal asynchronous subagent in the very same registry is steerable:
/// the Workflow restriction is scoped to ownership, not to steering as such.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_workflow_restriction_leaves_normal_subagent_steering_intact() {
    let plane = subagent_plane();
    let mut normal = stage_exit0(&plane);
    let normal_child = start_subagent(&plane, "normal survey").await;
    let mut workflow = stage_exit0(&plane);
    let workflow_child = start_workflow_subagent(&plane, "workflow node").await;
    let fixture = execution_fixture(Some(plane.registry.clone()));
    normal.take_delegation().await;
    workflow.take_delegation().await;

    let child_side = tokio::spawn(async move {
        let message = normal.answer_guidance(accepted()).await;
        (normal, message)
    });
    let steered = run_execution(
        &fixture,
        serde_json::json!({
            "action": "steer",
            "target": {"kind": "subagent", "id": normal_child.subagent_id.to_string()},
            "message": "narrow the survey",
        }),
    )
    .await;
    let (_normal, delivered) = child_side.await.expect("child side");
    assert_eq!(
        json_content(&steered)["accepted"],
        serde_json::json!(true),
        "an ordinary asynchronous subagent remains steerable"
    );
    assert_eq!(delivered, "narrow the survey");

    let refused = run_execution(
        &fixture,
        serde_json::json!({
            "action": "steer",
            "target": {"kind": "subagent", "id": workflow_child.subagent_id.to_string()},
            "message": "still refused",
        }),
    )
    .await;
    assert!(
        failure_message(&refused)
            .contains("steering is not supported for workflow-owned subagent executions"),
        "the sibling Workflow child is refused by ownership, in the same registry"
    );
}
