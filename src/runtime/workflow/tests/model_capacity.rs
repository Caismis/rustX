//! Cross-Conversation reservation rollback must not strand a native Model owner.
use super::*;
use crate::runtime::auxiliary::capacity::{CapacityDomain, ConversationCapacity, ProcessCapacity};

fn capacity_plane() -> WorkflowTestPlane {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("workspace")).unwrap();
    std::fs::create_dir_all(dir.path().join("subagents")).unwrap();
    let conversation = ConversationId::generate();
    let store =
        Arc::new(crate::durable::SqliteConversationStore::in_memory(conversation.clone()).unwrap());
    workflow_test_plane_with_store(dir, conversation, store, 1)
}

fn setup_in_capacity(
    plane: &WorkflowTestPlane,
    program: &WorkflowProgram,
    capacity: ConversationCapacity,
) -> (
    NativeExecutionContext,
    AuxiliaryExecutor,
    mpsc::UnboundedReceiver<Entry>,
) {
    let lifecycle = crate::runtime::types::ConversationLifecycle::new();
    assert!(lifecycle.activate());
    let executor = AuxiliaryExecutor::new(
        plane.store.clone(),
        Arc::new(crate::runtime::types::SystemClock),
        Arc::new(crate::runtime::ManualMonotonicClock::new()),
        lifecycle,
        Arc::new(crate::runtime::types::DurabilityGate::new()),
        capacity,
    )
    .unwrap();
    let (context, executor, entries, _) = setup_with_executor(
        plane,
        program,
        completed("answer", ModelFinishReason::Stop),
        false,
        None,
        executor,
    );
    (context, executor, entries)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn another_conversation_dispatches_after_partial_rollback_without_a_call_release() {
    let process = ProcessCapacity::new();
    let full = process.conversation();
    let mut retained = (0..3)
        .map(|_| full.reserve_general().unwrap())
        .collect::<Vec<_>>();
    let others = (0..7).map(|_| process.conversation()).collect::<Vec<_>>();
    retained.extend((0..20).map(|index| others[index / 3].reserve_general().unwrap()));
    assert_eq!(retained.len(), 23);
    let contender = process.conversation();
    let gate = full.hold_after_process_general_for_test();
    let lock_blocked = contender.reservation_contention_for_test();

    let program = model_program(
        WorkflowModelOutput::Text {},
        crate::model::AuxiliaryModelPolicy::Session,
    );
    let plane_a = capacity_plane();
    let plane_b = capacity_plane();
    assert_ne!(plane_a.conversation_id, plane_b.conversation_id);
    let (context_a, executor_a, mut entries_a) = setup_in_capacity(&plane_a, &program, full);
    let (context_b, executor_b, mut entries_b) = setup_in_capacity(&plane_b, &program, contender);
    let mut changes = executor_a.capacity_changes();
    changes.borrow_and_update();
    let runtime_a = workflow_runtime(&plane_a);
    let runtime_b = workflow_runtime(&plane_b);
    let (cancel_a, cancellation_a) = workflow_cancellation();
    let run_a = start(&runtime_a, context_a, cancellation_a);
    owner_gate(gate.entered).await; // A owns the last process-general slot.
    let run_b = start(&runtime_b, context_b, workflow_cancellation().1);
    owner_gate(lock_blocked).await; // B actually encounters the composite-reservation lock.
    assert!(entries_b.try_recv().is_err());
    gate.release.send(()).unwrap();

    let entered = entry(&mut entries_b).await;
    capacity_wait(&runtime_a).await;
    assert_eq!(retained.len(), 23, "no original reservation was released");
    assert_eq!(
        process.conversation().reserve_general().unwrap_err().domain,
        CapacityDomain::ProcessGeneral,
        "23 retained reservations and B fill the 24-slot process bound"
    );
    assert!(entries_a.try_recv().is_err());
    assert!(!run_a.is_finished());
    assert!(
        !changes.has_changed().unwrap(),
        "local refusal does not notify itself into an admission retry loop"
    );
    assert_model_admitted_once(&runtime_a, 1);
    assert_model_admitted_once(&runtime_b, 1);

    cancel_a.cancel();
    assert!(run_a.await.unwrap().unwrap_err().is_cancelled());
    assert!(entries_a.try_recv().is_err());
    entered.release.send(()).unwrap();
    assert_eq!(run_b.await.unwrap().unwrap(), json!({"answer":"answer"}));
    assert_model_admitted_once(&runtime_b, 2);
    executor_a.drain().await;
    executor_b.drain().await;
}

#[tokio::test]
async fn cancellation_while_other_conversations_hold_process_capacity_dispatches_nothing() {
    let process = ProcessCapacity::new();
    let others = (0..8).map(|_| process.conversation()).collect::<Vec<_>>();
    let retained = others
        .iter()
        .flat_map(|capacity| (0..3).map(move |_| capacity.reserve_general().unwrap()))
        .collect::<Vec<_>>();
    let plane = capacity_plane();
    let program = model_program(
        WorkflowModelOutput::Text {},
        crate::model::AuxiliaryModelPolicy::Session,
    );
    let (context, executor, mut entries) =
        setup_in_capacity(&plane, &program, process.conversation());
    let runtime = workflow_runtime(&plane);
    let (cancel, cancellation) = workflow_cancellation();
    let run = start(&runtime, context, cancellation);
    capacity_wait(&runtime).await;
    assert!(entries.try_recv().is_err());
    cancel.cancel();
    assert!(run.await.unwrap().unwrap_err().is_cancelled());
    assert!(entries.try_recv().is_err());
    assert_model_admitted_once(&runtime, 1);
    assert_eq!(retained.len(), 24);
    assert_eq!(
        process.conversation().reserve_general().unwrap_err().domain,
        CapacityDomain::ProcessGeneral
    );
    assert!(
        plane
            .store
            .read_events(None, 128)
            .unwrap()
            .events
            .iter()
            .all(|event| !matches!(event.event, RuntimeEvent::AuxiliaryCallStarted { .. }))
    );
    executor.drain().await;
}
