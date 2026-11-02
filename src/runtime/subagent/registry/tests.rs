//! Shared deterministic fixtures of the registry tests: a registry over a real
//! durable store, scripted staged children, and observers. Each child module
//! holds the regressions of one registry responsibility.

use crate::durable::JournalStore;
mod agent_bootstrap;
mod agent_duration;
mod agent_recovery;
mod archive_ownership;
mod capacity_wait;
mod control;
mod deadline;
mod input_admission;
mod observation;
mod ownership_commit;
mod physical_proof;
mod profile_identity;
mod resume;
mod review;
mod settlement;
mod shared_workspace;
use std::sync::Arc;

use super::super::SubagentTerminalState;
use super::super::catalog::SubagentExecutionDeadline;
use super::super::ipc::DelegationFrame;
use super::super::ipc::{ChildFrame, ChildResultStatus, ParentFrame, ResultFrame};
use super::super::process::StagedChild;
use super::*;
use crate::capabilities::selection::AgentToolSelection;
use crate::durable::ConversationStore;
use crate::runtime::ToolCallId;
use crate::runtime::types::{CancellationReason, SystemClock};

/// A registry over a real (in-memory) durable store with a test seam
/// for staged children.
struct TestPlane {
    dir: tempfile::TempDir,
    registry: SubagentRegistry,
    store: Arc<crate::durable::SqliteConversationStore>,
    conversation_id: ConversationId,
    runtime_root: std::path::PathBuf,
    monotonic_clock: Arc<crate::runtime::ManualMonotonicClock>,
}

fn plane(max_active: usize) -> TestPlane {
    plane_with_storage(max_active, false)
}

fn plane_with_storage(max_active: usize, on_disk: bool) -> TestPlane {
    let dir = tempfile::tempdir().expect("temp dir");
    let workspace = dir.path().join("workspace");
    let runtime_root = dir.path().join("runtime");
    std::fs::create_dir_all(&workspace).expect("workspace");
    std::fs::create_dir_all(&runtime_root).expect("runtime root");
    let conversation_id = ConversationId::new("conv_5d71cacf-35af-790f-8022-fe16cd1b9fe0");
    let store = Arc::new(if on_disk {
        crate::durable::SqliteConversationStore::open(
            conversation_id.clone(),
            &dir.path().join("parent.sqlite"),
        )
        .expect("on-disk store")
    } else {
        crate::durable::SqliteConversationStore::in_memory(conversation_id.clone())
            .expect("in-memory store")
    });
    let mailbox = ConversationInboundMailbox::over_store(store.clone());
    let monotonic_clock = Arc::new(crate::runtime::ManualMonotonicClock::new());
    let registry = SubagentRegistry::new(SubagentRegistryConfig {
        conversation_id: conversation_id.clone(),
        agent_id: AgentId::new("agent-parent"),
        mailbox,
        clock: Arc::new(SystemClock),
        monotonic_clock: monotonic_clock.clone(),
        spawn: SubagentSpawnPlan {
            session_id: crate::runtime::identity::SessionId::new(
                "ses_01900000-0000-7000-8000-000000000001",
            ),

            program: std::path::PathBuf::from("/nonexistent/rustx"),
            product_root: crate::runtime::local_storage::ProductRoot::create(&runtime_root.clone())
                .expect("product root"),
        },
        workspace: workspace.clone(),
        max_active,
    });
    TestPlane {
        dir,
        registry,
        store,
        conversation_id,
        runtime_root,
        monotonic_clock,
    }
}

/// A scripted child: one trivial real process (kill/reap semantics) and
/// the test-held end of the control channel (protocol semantics).
struct ScriptedChild {
    peer: tokio::net::UnixStream,
    pid: u32,
}

/// Stages a scripted child whose process exits immediately; the test
/// drives the protocol over `peer`.
fn stage_exit0(plane: &TestPlane) -> ScriptedChild {
    stage_process(plane, "true")
}

/// Stages a child with an intentionally uncontainable nested anchor. The
/// impossible group id is accepted by the wire-shape seam but cannot be
/// adopted by this process, so terminal workspace settlement must remain
/// explicitly unresolved.
fn stage_with_unresolved_anchor(plane: &TestPlane) -> ScriptedChild {
    stage_process_inner(plane, "true", true)
}

/// Stages a scripted child whose process ignores everything and must be
/// killed; used for cancellation-escalation tests.
fn stage_stubborn(plane: &TestPlane) -> ScriptedChild {
    stage_process(plane, "trap '' TERM; exec sleep 60")
}

fn stage_process(plane: &TestPlane, shell: &str) -> ScriptedChild {
    stage_process_inner(plane, shell, false)
}

fn stage_process_inner(plane: &TestPlane, shell: &str, unresolved_anchor: bool) -> ScriptedChild {
    let (driver_end, test_end) = tokio::net::UnixStream::pair().expect("pair");
    let (observation_end, _observation_peer) =
        tokio::net::UnixStream::pair().expect("observation pair");
    let child = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(shell)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(0)
        .spawn()
        .expect("scripted child process");
    let pid = child.id().expect("scripted child pid");
    let child_runtime_root = plane.runtime_root.join(format!("test-child-{pid}"));
    std::fs::create_dir_all(&child_runtime_root).expect("child runtime root");
    let mut staged = StagedChild::for_test(child, driver_end, observation_end, child_runtime_root);
    if unresolved_anchor {
        staged.retain_for_test(
            crate::runtime::identity::ProcessUnitId::new("unit-unresolved"),
            i32::MAX,
        );
    }
    plane.registry.push_staged_override(staged);
    ScriptedChild {
        peer: test_end,
        pid,
    }
}

impl ScriptedChild {
    /// Awaits one parent-to-child control frame with a liveness guard.
    /// The ordering assertions in deadline tests are driven by the
    /// manual monotonic clock and registry watch, not by this guard.
    async fn read_frame(&mut self) -> ParentFrame {
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            super::super::ipc::read_parent_frame(&mut self.peer),
        )
        .await
        .expect("driver control liveness")
        .expect("driver frame")
        .expect("parent frame")
    }

    async fn accept_delegate(&mut self) -> DelegationFrame {
        let ParentFrame::Delegate(delegate) = self.read_frame().await else {
            panic!("delegate");
        };
        super::super::ipc::write_child_frame(&mut self.peer, &ChildFrame::DelegateAccepted)
            .await
            .unwrap();
        delegate
    }

    /// Sends the child's terminal frame after the test has established
    /// the intended lifecycle ordering.
    async fn send_result(&mut self, status: ChildResultStatus, content: Option<&str>) {
        super::super::ipc::write_child_frame(
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

    /// Awaits the delegated task and answers with one terminal result.
    async fn complete(mut self, status: ChildResultStatus, content: Option<&str>) {
        let frame = self.read_frame().await;
        assert!(
            matches!(frame, ParentFrame::Delegate(_)),
            "the committed child is delegated first"
        );
        self.send_result(status, content).await;
    }

    /// Awaits the delegation **and** the cancellation, then answers with
    /// one terminal result.
    ///
    /// The second read is the point of this helper. A cancelled child's
    /// terminal is only well defined once the driver has actually
    /// delivered the `Cancel` frame: a scripted peer that answers and
    /// drops its socket while the driver is still writing that frame
    /// makes the driver observe a broken pipe and classify the child as
    /// `Interrupted`. Reading the `Cancel` frame first establishes that
    /// ordering as a linearization instead of leaving it to whichever
    /// `select!` branch happens to be polled, and it is also what a real
    /// cancelled child does: observe the cancellation, then answer.
    async fn cancelled_after_delegation(
        mut self,
        status: ChildResultStatus,
        content: Option<&str>,
    ) {
        let frame = self.read_frame().await;
        assert!(
            matches!(frame, ParentFrame::Delegate(_)),
            "the committed child is delegated first"
        );
        let frame = self.read_frame().await;
        assert!(
            matches!(frame, ParentFrame::Cancel { .. }),
            "the cancellation reaches the child before it answers: {frame:?}"
        );
        self.send_result(status, content).await;
    }
}

/// A Builtin-only frozen specification: the registry owns live child
/// lifecycle, so resolution is already complete before it is involved.
fn resolved(agent: &str) -> ResolvedSubagentSpec {
    ResolvedSubagentSpec {
        resource_sources: std::collections::BTreeSet::new(),
        environment: Vec::new(),
        generation: crate::runtime::identity::RuntimeResourceRevision::new(1),
        skill_roots: Vec::new(),

        selection: crate::runtime::agent_profile::FrozenAgentSelection::default(),
        agent: SubagentName::parse(agent).expect("canonical name"),
        definition_digest: serde_json::from_value(serde_json::json!(
            "sha256:0000000000000000000000000000000000000000000000000000000000000000"
        ))
        .expect("digest"),
        execution_deadline: None,
        instructions: "instructions".to_owned(),
        model: crate::model::frozen::test_frozen_model_spec(
            serde_json::from_value(serde_json::json!("local/model")).expect("model ref"),
        ),
        tools: Vec::new(),
        skills: Vec::new(),
        project_instructions: Vec::new(),
        materialization:
            crate::runtime::subagent::resolver::ResolvedSubagentMaterialization::default(),
        extensions: crate::extensions::NativeAgentExtensions::with_agent_status(
            crate::context::AgentStatusConfig::default(),
        )
        .and_todo(),
    }
}

fn spec(task: &str) -> SubagentStartSpec {
    SubagentStartSpec {
        authority: crate::runtime::subagent::DurableAgentAuthority {
            title: "Explore task".into(),
            execution_policy: crate::runtime::subagent::InheritedExecutionPolicy::default(),
            resolved: resolved("explore"),
            approval_mode: crate::runtime::ApprovalMode::Policy,
        },
        admission: crate::runtime::subagent::ActivationAdmission {
            attachments: Vec::new(),
            task: task.to_owned(),
            context: None,
            origin: crate::runtime::subagent::AgentActivationOrigin::CreationTool {
                tool_call_id: ToolCallId::new("call-1"),
            },
            terminal: SubagentTerminalMode::Normal,
        },
    }
}

fn workflow_spec(conversation: &ConversationId, task: &str) -> SubagentStartSpec {
    let mut start = spec(task);
    let mut node = crate::runtime::workflow::test_instance("test_workflow", "agent");
    node.block.run.conversation_id = conversation.clone();
    start.admission = crate::runtime::subagent::ActivationAdmission {
        origin: AgentActivationOrigin::Workflow {
            node_id: Box::new(node.clone()),
        },
        attachments: Vec::new(),
        terminal: SubagentTerminalMode::WorkflowOutput {
            output_schema: Some(serde_json::json!({
                "type": "object",
                "properties": {"summary": {"type": "string"}},
                "required": ["summary"],
                "additionalProperties": false
            })),
            workflow_id: crate::runtime::workflow::WorkflowId::parse("test_workflow")
                .expect("workflow id"),
            run_id: node.block.run.clone(),
            node_id: Box::new(node.clone()),
        },
        ..start.admission
    };
    start
}

fn start_spec(task: &str) -> SubagentStartSpec {
    spec(task)
}

fn deadline_spec(task: &str, millis: u64) -> SubagentStartSpec {
    let mut spec = start_spec(task);
    spec.authority.resolved.execution_deadline =
        Some(SubagentExecutionDeadline::from_millis(millis).expect("valid test deadline"));
    spec
}

async fn wait_for_cancelling(plane: &TestPlane, subagent_id: &SubagentId) -> SubagentSnapshot {
    plane
        .registry
        .wait_for_snapshot(subagent_id, |snapshot| {
            snapshot.state == SubagentState::Cancelling
        })
        .await
        .expect("deadline cancellation intent")
}

fn published_terminal_states(
    plane: &TestPlane,
    subagent_id: &SubagentId,
) -> Vec<SubagentTerminalState> {
    events(plane)
        .into_iter()
        .filter_map(|event| match event {
            crate::events::types::RuntimeEvent::SubagentTerminalPublished {
                subagent_id: event_id,
                state,
                ..
            } if event_id == *subagent_id => Some(state),
            _ => None,
        })
        .collect()
}

/// Records every snapshot the registry publishes, in publish order. The
/// callback runs synchronously under the registry lock (never
/// coalesced), so the log is an exact observable sequence of lifecycle
/// transitions — the deterministic side-effect counter for the deadline
/// race regressions. Activity/workspace projections are never published
/// by these races, so the sequence is purely lifecycle.
#[derive(Default)]
struct RecordingObserver(std::sync::Mutex<Vec<SubagentSnapshot>>);

impl SubagentObserver for RecordingObserver {
    fn on_snapshot(
        &self,
        _agent: Option<&AgentSnapshot>,
        snapshot: &SubagentSnapshot,
        _cut: crate::runtime::identity::ObservationCollectionCut,
        _ordinal: Option<u64>,
    ) {
        self.0
            .lock()
            .expect("recording observer lock")
            .push(snapshot.clone());
    }
}

impl RecordingObserver {
    fn states(&self) -> Vec<SubagentState> {
        self.0
            .lock()
            .expect("recording observer lock")
            .iter()
            .map(|snapshot| snapshot.state)
            .collect()
    }
}

/// Installs a recording observer on a fresh plane (no records yet), so
/// the published lifecycle sequence from the ownership commit onward is
/// captured exactly.
fn recording_observer(plane: &TestPlane) -> Arc<RecordingObserver> {
    let recorded = Arc::new(RecordingObserver::default());
    plane
        .registry
        .install_observer_and_agent_snapshots(Arc::clone(&recorded) as Arc<dyn SubagentObserver>);
    recorded
}

/// Asserts the driver control wire carries no further frame: reads to
/// EOF (the driver shuts down its write half after physical
/// settlement), panicking on any frame. Used to prove a losing
/// cancellation source produced no second driver command after the
/// winner committed and the terminal result settled the child.
async fn drain_control_frames(child: &mut ScriptedChild) {
    let frame = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        super::super::ipc::read_parent_frame(&mut child.peer),
    )
    .await
    .expect("driver control liveness")
    .expect("driver frame");
    if let Some(frame) = frame {
        panic!("unexpected extra driver frame on the wire: {frame:?}");
    }
}

async fn start(plane: &TestPlane, spec: &SubagentStartSpec) -> SubagentAccepted {
    let prepared = plane
        .registry
        .prepare(spec, &CancellationSignal::new())
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

fn recovered_registry(plane: &TestPlane) -> SubagentRegistry {
    let registry = SubagentRegistry::new(plane.registry.config.clone());
    registry
        .restore_agents(plane.store.as_ref())
        .expect("the complete durable owner cut");
    registry
}

/// Reads the durable event journal.
fn events(plane: &TestPlane) -> Vec<crate::events::types::RuntimeEvent> {
    let mut all = Vec::new();
    let mut cursor = None;
    loop {
        let page = plane.store.read_events(cursor, 100).expect("events");
        if page.events.is_empty() {
            return all;
        }
        cursor = page.next_sequence;
        all.extend(page.events.into_iter().map(|envelope| envelope.event));
        if cursor.is_none() {
            return all;
        }
    }
}
