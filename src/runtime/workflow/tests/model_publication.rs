//! Observer-backed cuts must expose live Model progress before provider completion.
use super::*;
use crate::runtime::workflow::read_model::{
    WorkflowInstanceView, WorkflowNodeKind, WorkflowRevision, WorkflowSnapshot, WorkflowState,
    WorkflowWait,
};

type Published = tokio::sync::watch::Receiver<Option<(Arc<WorkflowSnapshot>, u64)>>;

fn observe_publication(runtime: &WorkflowRuntime) -> Published {
    let (published, receiver) = tokio::sync::watch::channel(None);
    runtime
        .read_model
        .install_publication_observer(Arc::new(move |cuts, sequence| {
            published.send_replace(cuts.last().map(|cut| (cut.clone(), sequence)));
        }));
    receiver
}

fn latest(published: &Published) -> (Arc<WorkflowSnapshot>, u64) {
    published
        .borrow()
        .clone()
        .expect("a confirmed Workflow cut")
}

fn model_node(snapshot: &WorkflowSnapshot) -> &WorkflowInstanceView {
    snapshot.runs[0]
        .instances
        .iter()
        .find(|node| node.kind == WorkflowNodeKind::Model)
        .unwrap()
}

async fn published_capacity_wait(published: &mut Published) -> Arc<WorkflowSnapshot> {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let current = published.borrow_and_update().clone();
            if let Some((snapshot, _)) = current
                && snapshot.runs[0].instances.iter().any(|node| {
                    node.kind == WorkflowNodeKind::Model
                        && node.state
                            == (WorkflowState::Waiting {
                                reason: WorkflowWait::Capacity,
                            })
                })
            {
                return snapshot;
            }
            published.changed().await.unwrap();
        }
    })
    .await
    .expect("capacity waiting reaches the confirmed publication frontier")
}

#[tokio::test]
async fn model_wait_and_call_identity_are_published_before_provider_completion() {
    let plane = workflow_test_plane(1);
    let program = model_program(
        WorkflowModelOutput::Text {},
        crate::model::AuxiliaryModelPolicy::Session,
    );
    let (context, executor, mut entries, _) = setup(
        &plane,
        &program,
        completed("answer", ModelFinishReason::Stop),
        false,
    );
    let runtime = workflow_runtime(&plane);
    let published = observe_publication(&runtime);
    let run = start(&runtime, context, workflow_cancellation().1);
    let entered = entry(&mut entries).await;

    let (snapshot, sequence) = latest(&published);
    let node = model_node(&snapshot);
    assert_eq!(
        node.state,
        WorkflowState::Waiting {
            reason: WorkflowWait::Model
        }
    );
    let call_id = node.auxiliary_call_id.as_ref().unwrap();
    assert_eq!(snapshot.runs[0].model_calls_consumed, 1);
    assert!(!run.is_finished());
    assert_no_successor(&runtime);
    let events = plane.store.read_events(None, 128).unwrap().events;
    assert!(events.iter().any(|event| matches!(
        &event.event,
        RuntimeEvent::AuxiliaryCallStarted { call_id: admitted, .. } if admitted == call_id
    )));
    let waiting = events
        .iter()
        .find(|event| event.sequence == sequence)
        .unwrap();
    assert!(matches!(
        &waiting.event,
        RuntimeEvent::WorkflowNodeWaiting {
            instance, reason: WorkflowWait::Model, auxiliary_call_id: Some(reserved)
        } if instance.node == "generate" && reserved == call_id
    ));
    let encoded = serde_json::to_string(&waiting.event).unwrap();
    let decoded: RuntimeEvent = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, waiting.event);
    assert!(!encoded.contains("the input"));
    assert!(!encoded.contains("answer"));
    assert_eq!(
        runtime
            .read_model
            .published_source_cuts_after(WorkflowRevision(0))
            .last()
            .unwrap()
            .as_ref(),
        snapshot.as_ref()
    );

    entered.release.send(()).unwrap();
    assert_eq!(run.await.unwrap().unwrap(), json!({"answer":"answer"}));
    assert_eq!(
        latest(&published).0.runs[0].state,
        WorkflowState::Settled {
            outcome: WorkflowExecutionOutcome::Completed,
        }
    );
    executor.drain().await;
}

#[tokio::test]
async fn fourth_model_capacity_wait_is_published_while_all_providers_are_held() {
    let plane = workflow_test_plane(1);
    let program = parallel_model_program();
    let (context, executor, mut entries, _) = setup(
        &plane,
        &program,
        completed("$task", ModelFinishReason::Stop),
        false,
    );
    let runtime = workflow_runtime(&plane);
    let mut published = observe_publication(&runtime);
    let (signal, cancellation) = workflow_cancellation();
    let run = start(&runtime, context, cancellation);
    let held = [
        entry(&mut entries).await,
        entry(&mut entries).await,
        entry(&mut entries).await,
    ];
    let snapshot = published_capacity_wait(&mut published).await;
    let nodes = snapshot.runs[0]
        .instances
        .iter()
        .filter(|node| node.kind == WorkflowNodeKind::Model)
        .collect::<Vec<_>>();
    assert_eq!(nodes.len(), 4);
    assert_eq!(snapshot.runs[0].model_calls_consumed, 4);
    assert_eq!(snapshot.runs[0].steps_consumed, 5);
    assert_eq!(
        nodes
            .iter()
            .filter(|node| node.auxiliary_call_id.is_some())
            .count(),
        3
    );
    let waiting = nodes
        .iter()
        .find(|node| {
            node.state
                == (WorkflowState::Waiting {
                    reason: WorkflowWait::Capacity,
                })
        })
        .unwrap();
    assert!(waiting.auxiliary_call_id.is_none());
    assert!(nodes.iter().all(|node| node.visit == Some(0)));
    assert!(!run.is_finished());
    assert!(entries.try_recv().is_err());
    let waiting_instance = WorkflowNodeInstance {
        block: waiting.block.clone(),
        node: waiting.node.clone().unwrap(),
        visit: waiting.visit.unwrap(),
    };
    let events = plane.store.read_events(None, 128).unwrap().events;
    // Other branches may have already recovered from lifecycle contention.
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                &event.event,
                RuntimeEvent::WorkflowNodeWaiting {
                    instance,
                    reason: WorkflowWait::Capacity,
                    auxiliary_call_id: None,
                } if instance == &waiting_instance
            ))
            .count(),
        1
    );

    signal.cancel();
    for provider in held {
        // Cancellation may already have dropped the held provider stream.
        let _ = provider.release.send(());
    }
    assert!(run.await.unwrap().unwrap_err().is_cancelled());
    assert!(entries.try_recv().is_err());
    executor.drain().await;
}

#[tokio::test]
async fn failed_model_wait_append_withholds_call_identity_until_confirmed_progress() {
    let plane = workflow_test_plane(1);
    let program = model_program(
        WorkflowModelOutput::Text {},
        crate::model::AuxiliaryModelPolicy::Session,
    );
    let (context, executor, mut entries, _) = setup(
        &plane,
        &program,
        completed("answer", ModelFinishReason::Stop),
        false,
    );
    let (reserved, release) = hook(&context, ModelFrontier::AfterReservation);
    let runtime = workflow_runtime(&plane);
    let published = observe_publication(&runtime);
    let run = start(&runtime, context, workflow_cancellation().1);
    owner_gate(reserved).await;
    let (before, sequence) = latest(&published);
    assert_eq!(model_node(&before).state, WorkflowState::Running);
    assert!(model_node(&before).auxiliary_call_id.is_none());
    plane.store.arm_fail_event_times(1);
    release.send(()).unwrap();
    let entered = entry(&mut entries).await;

    let internal = runtime.read_model.snapshot();
    assert_eq!(
        model_node(&internal).state,
        WorkflowState::Waiting {
            reason: WorkflowWait::Model
        }
    );
    assert!(model_node(&internal).auxiliary_call_id.is_some());
    let (confirmed, confirmed_sequence) = latest(&published);
    assert_eq!(confirmed, before);
    assert_eq!(confirmed_sequence, sequence);
    assert!(
        runtime
            .read_model
            .published_source_cuts_after(before.revision)
            .is_empty()
    );
    assert!(
        plane
            .store
            .read_events(None, 128)
            .unwrap()
            .events
            .iter()
            .all(|event| !matches!(event.event, RuntimeEvent::WorkflowNodeWaiting { .. }))
    );

    entered.release.send(()).unwrap();
    assert_eq!(run.await.unwrap().unwrap(), json!({"answer":"answer"}));
    let final_cut = latest(&published).0;
    assert!(final_cut.revision > internal.revision);
    assert!(model_node(&final_cut).auxiliary_call_id.is_some());
    assert_eq!(
        final_cut.runs[0].state,
        WorkflowState::Settled {
            outcome: WorkflowExecutionOutcome::Completed,
        }
    );
    executor.drain().await;
}
