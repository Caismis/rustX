use crate::durable::ConversationStore;

fn install_trace(projection: &mut RuntimeClientProjection, store: &dyn ConversationStore) {
    let fence = projection.read_domain_cut().unwrap();
    let read = fence.read(store).unwrap();
    publish_source_fact(projection, |projection| {
        assert!(projection.install_read_domains(&fence, Ok(read), true));
    });
}

#[test]
fn prepared_trace_owner_reuses_exact_rows_and_retires_after_native_terminal_install() {
    let (mut projection, store, call, _) = native_running_tool_fixture();
    install_trace(&mut projection, &store);
    let old_seed = projection.checkpoint_seed();
    let old = old_seed.checkpoint_candidate().unwrap();
    let old_owner = old_seed.trace_head.clone();
    assert!(old_owner.prepared());
    assert!(projection.trace_head.same_owner(&old_owner));
    let tool = old
        .snapshot
        .trace
        .records
        .iter()
        .find(|record| record.kind == super::super::super::trace::TraceKind::Tool)
        .unwrap();
    assert_eq!(
        tool.state,
        super::super::super::trace::TraceState::Running,
        "certificates follow the installed native live repair"
    );

    publish_source_fact(&mut projection, |projection| {
        projection.publish(RuntimeClientEvent::TraceChanged);
    });
    let unrelated = projection.checkpoint_candidate().unwrap();
    assert!(projection.trace_head.same_owner(&old_owner));
    assert_eq!(unrelated.snapshot.trace, old.snapshot.trace);
    assert!(unrelated.cursor > old.cursor);
    projection.read_domains_dirty = true;
    install_trace(&mut projection, &store);
    assert!(
        projection.trace_head.same_owner(&old_owner),
        "an unchanged installation keeps its exact record owner"
    );

    let terminal = store
        .append_event(source_tool_event(
            &store,
            RuntimeEvent::ToolExecutionCompleted {
                tool_call_id: call.id,
                tool_id: call.tool_id,
                result: success_result(),
            },
            "prepared-trace-terminal",
        ))
        .unwrap();
    publish_source_fact(&mut projection, |projection| {
        projection.apply_native_batch(terminal.observation.as_deref().unwrap());
    });
    install_trace(&mut projection, &store);
    let current = projection.checkpoint_candidate().unwrap();
    assert!(!projection.trace_head.same_owner(&old_owner));
    assert_eq!(
        current.snapshot.trace.read_cut.as_ref(),
        Some(&current.read_cut)
    );
    let current_tool = current
        .snapshot
        .trace
        .records
        .iter()
        .find(|record| record.id == tool.id)
        .unwrap();
    assert_eq!(
        current_tool.state,
        super::super::super::trace::TraceState::Completed
    );
    assert_eq!(
        old_seed.checkpoint_candidate().unwrap(),
        old,
        "the held seed retains its original normalized rows and cut"
    );
}
