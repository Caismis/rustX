use super::*;
use crate::durable::{JournalStore, ObservationStore};

#[test]
fn t10_t14_failed_workflow_append_does_not_release_uncommitted_source_revisions() {
    let plane = workflow_test_plane(1);
    let runtime = workflow_runtime(&plane);
    let program = parallel_test_program(&["alpha"]);
    let context = workflow_test_context(&plane);
    let id = WorkflowRunId {
        conversation_id: plane.conversation_id.clone(),
        origin: (context.attempt_id().unwrap().clone()).into(),
        invocation: 1,
    };
    let run = WorkflowRun::new(program.clone(), id.clone());
    let publication = Arc::new(std::sync::Mutex::new(Vec::new()));
    let receipts = publication.clone();
    runtime
        .read_model
        .install_publication_observer(Arc::new(move |cuts, sequence| {
            receipts.lock().unwrap().push((cuts, sequence));
        }));
    let call = ToolCallId::new("failed-source-publication");
    runtime.read_model.register(read_model::WorkflowRunView {
        id: id.clone(),
        workflow_id: program.id().clone(),
        program_digest: program.digest.clone(),
        resource_revision: context.resources().revision(),
        tool_call_id: Some(call.clone()),
        state: read_model::WorkflowState::Running,
        instances: Vec::new(),
        omitted_instances: 0,
        steps_consumed: 0,
        steps_max: program.execution_bound,
        agents_consumed: 0,
        model_calls_consumed: 0,
    });
    let fact = RuntimeEvent::WorkflowStarted {
        tool_call_id: Some(call),
        workflow_id: program.id().clone(),
        run_id: id,
    };
    let original_cut = plane.store.observation_read_cut().unwrap();
    plane.store.arm_fail_event_times(1);
    runtime.emit_observability(&run, fact.clone());
    assert!(publication.lock().unwrap().is_empty());
    assert!(
        runtime
            .read_model
            .published_source_cuts_after(read_model::WorkflowRevision(0))
            .is_empty()
    );
    assert_eq!(plane.store.observation_read_cut().unwrap(), original_cut);
    assert!(plane.store.read_events(None, 2).unwrap().events.is_empty());
    runtime.emit_observability(&run, fact.clone());
    let delivered = publication.lock().unwrap();
    assert_eq!(delivered.len(), 1);
    let (cuts, sequence) = &delivered[0];
    assert_eq!(
        cuts.last().unwrap().as_ref(),
        &runtime.read_model.snapshot()
    );
    let originals = plane.store.read_events(None, 2).unwrap().events;
    assert_eq!(originals.len(), 1);
    assert_eq!(originals[0].sequence, *sequence);
    assert_eq!(originals[0].event, fact);
}
