//! Selected Workflow visits never acquire activity from stale or cold views.
use super::*;
use crate::durable::observation::WorkflowNodeTraceState;
use crate::runtime::workflow::read_model::{WorkflowRunView, WorkflowState, WorkflowWait};

fn append(inner: &Arc<super::super::ClientInner>, event: RuntimeEvent, ordinal: u64) -> u64 {
    let committed = inner
        .store
        .append_event(RuntimeEventEnvelope {
            tool_occurrence: None,
            observation: None,
            schema_version: EVENT_SCHEMA_VERSION,
            event_id: EventId::new(format!("workflow-live-{ordinal}")),
            sequence: 0,
            conversation_id: inner.conversation_id.clone(),
            attempt_id: None,
            turn_id: None,
            timestamp: chrono::DateTime::UNIX_EPOCH,
            event,
        })
        .unwrap();
    committed.sequence
}

async fn settle(host: &RuntimeClientHost) {
    tokio::time::timeout(BACKGROUND_LIVENESS_GUARD, host.projection_settled())
        .await
        .unwrap_or_else(|error| {
            panic!(
                "Workflow source worker: {error}; {}",
                host.inner.projection_source_debug()
            )
        });
}

fn read(
    inner: &super::super::ClientInner,
    cut: &crate::observation::cut::CheckpointReadCut,
    start: u64,
) -> WorkflowNodeTraceState {
    let page = inner
        .checkpoint_page(cut, &format!("trace.records.{start}.workflow.nodes"), 0, 32)
        .unwrap();
    assert_eq!(page.items.len(), 1);
    serde_json::from_value(page.items[0].value.as_ref().unwrap()["state"].clone()).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workflow_node_live_overlay_requires_a_resident_owner_and_its_current_published_cut() {
    assert_live_overlay(0, "review").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workflow_node_live_overlay_reads_beyond_the_display_instance_prefix() {
    assert_live_overlay(33, "review").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workflow_node_live_overlay_reads_instances_omitted_by_the_display_byte_limit() {
    assert_live_overlay(0, &"review".repeat(400)).await;
}

#[allow(clippy::too_many_lines)] // One native admission/wait/terminal and cold-owner publication proof.
async fn assert_live_overlay(prior_instances: usize, node_name: &str) {
    use crate::runtime::workflow::WorkflowExecutionOutcome as Outcome;
    let (_adapter, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), status_engine()).await;
    let inner = fixture.host.weak_inner().upgrade().unwrap();
    let mut instance = crate::runtime::workflow::test_instance("host_trace", node_name);
    instance.block.run.conversation_id = inner.conversation_id.clone();
    let mut live: WorkflowRunView = serde_json::from_value(serde_json::json!({
        "id": instance.block.run, "workflow_id": "host_trace", "program_digest": "digest",
        "resource_revision": 1, "state": { "type": "running" },
        "instances": [{ "block": instance.block, "node": instance.node, "visit": instance.visit,
            "kind": "review", "state": { "type": "running" } }],
        "omitted_instances": 0, "steps_consumed": 1, "steps_max": 10,
        "agents_consumed": 0, "model_calls_consumed": 0
    }))
    .unwrap();
    let owner = fixture.runtime.tool_runtime().workflows();
    let node = live.instances.pop().unwrap();
    live.instances = (0..prior_instances)
        .map(|ordinal| {
            let mut prior = node.clone();
            prior.node = Some(format!("prior-{ordinal}"));
            prior.state = WorkflowState::Settled {
                outcome: Outcome::Completed,
            };
            prior
        })
        .collect();
    let start = append(
        &inner,
        RuntimeEvent::WorkflowStarted {
            tool_call_id: None,
            workflow_id: crate::runtime::workflow::WorkflowId::parse("host_trace").unwrap(),
            run_id: instance.block.run.clone(),
        },
        1,
    );
    // Match the executor's append/publish pair for every lifecycle fact.
    // Skipping the first callback leaves its J permanently unreleased.
    owner.publish_run_for_test(live.clone(), start);
    let admission = append(
        &inner,
        RuntimeEvent::WorkflowNodeStarted {
            instance: instance.clone(),
        },
        2,
    );
    live.instances.push(node);
    owner.publish_run_for_test(live.clone(), admission);
    settle(&fixture.host).await;
    let old = fixture.host.checkpoint().unwrap().read_cut;
    assert_eq!(old.through, admission);
    assert_eq!(read(&inner, &old, start), WorkflowNodeTraceState::Running);
    let wait = append(
        &inner,
        RuntimeEvent::WorkflowNodeWaiting {
            instance: instance.clone(),
            reason: WorkflowWait::Review,
            auxiliary_call_id: None,
        },
        3,
    );
    live.instances[prior_instances].state = WorkflowState::Waiting {
        reason: WorkflowWait::Review,
    };
    owner.publish_run_for_test(live.clone(), wait);
    settle(&fixture.host).await;
    let checkpoint = fixture.host.checkpoint().unwrap();
    let current = checkpoint.read_cut;
    let source =
        crate::observation::workflow_source::WorkflowReadSource::new(Arc::new(owner.snapshot()));
    source.prepare();
    let source_header = source.header();
    let header = &source_header.runs[0].instances;
    assert!(
        header.len() <= 32,
        "display header keeps its finite count limit"
    );
    assert_eq!(
        header
            .iter()
            .any(|row| row.node.as_deref() == Some(node_name)),
        prior_instances == 0 && node_name.len() < 2048,
        "the regression target must be absent from truncated display headers"
    );
    assert_eq!(
        owner.snapshot().runs[0].instances.len(),
        prior_instances + 1
    );
    // The outer checkpoint also caps each complete run header, so a run with
    // 32 finite instance rows may be omitted there altogether.
    assert!(
        checkpoint
            .snapshot
            .workflows
            .runs
            .iter()
            .all(|run| run.instances.len() <= 32)
    );
    assert_ne!(old, current);
    assert!(matches!(
        read(&inner, &current, start),
        WorkflowNodeTraceState::Waiting { .. }
    ));
    assert_eq!(
        read(&inner, &old, start),
        WorkflowNodeTraceState::Incomplete
    );

    // A cold inspection host may retain the same display data; no resident
    // runtime means it cannot provide a current execution owner.
    let cold = RuntimeClientHost::new_durable(inner.store.clone(), None).unwrap();
    let sink = super::super::NativePublicationProjection {
        inner: Arc::downgrade(&cold.inner),
    };
    assert!(
        crate::runtime::observation::PublicationProjectionCommit::native_actor(
            &sink,
            &ConversationObservation::Workflow(vec![source]),
        )
        .unwrap()
        .is_installed()
    );
    let cold_cut = cold.checkpoint().unwrap().read_cut;
    assert_eq!(
        cold.checkpoint().unwrap().snapshot.workflows,
        checkpoint.snapshot.workflows
    );
    assert_eq!(
        read(&cold.inner, &cold_cut, start),
        WorkflowNodeTraceState::Incomplete
    );

    let end = append(
        &inner,
        RuntimeEvent::WorkflowNodeSettled {
            instance,
            outcome: Outcome::Completed,
        },
        4,
    );
    // Even an erroneously retained current wait cannot replace a durable
    // terminal; publish it through the actual native source callback.
    owner.publish_run_for_test(live, end);
    settle(&fixture.host).await;
    let terminal = fixture.host.checkpoint().unwrap().read_cut;
    assert_eq!(
        read(&inner, &terminal, start),
        WorkflowNodeTraceState::Settled {
            outcome: Outcome::Completed
        }
    );
    // The previous start-only source cannot import a terminal from a newer J.
    assert_eq!(
        read(&inner, &current, start),
        WorkflowNodeTraceState::Incomplete
    );
    fixture.runtime.shutdown().await.unwrap();
}
