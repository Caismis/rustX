// The currently published Trace cache owns its original issued read evidence.
use crate::durable::ConversationStore;
use crate::runtime_client::RuntimeClientError;

fn install_current_trace(projection: &mut RuntimeClientProjection, store: &dyn ConversationStore) {
    let fence = projection.read_domain_cut().unwrap();
    let read = fence.read(store).unwrap();
    publish_source_fact(projection, |projection| {
        assert!(projection.install_read_domains(&fence, Ok(read), true));
    });
}

fn commit_result_without_terminal_j(
    projection: &mut RuntimeClientProjection,
    store: &dyn ConversationStore,
    call: ToolCall,
) {
    let receipt = store
        .append_canonical(&MessageBlock::Tool(ToolMessageBlock {
            id: MessageId::new("cached-trace-result"),
            occurrence: crate::message::types::ToolCallOccurrenceRef::new(
                MessageId::new("progress-proposal"),
                ContentBlockIndex::new(0),
            ),
            tool_call_id: call.id,
            tool_id: call.tool_id,
            result: success_result(),
        }))
        .unwrap();
    let cursor = receipt.transcript_cursor.unwrap().get();
    publish_source_fact(projection, |projection| {
        projection.apply(ConversationObservation::CanonicalCommitted {
            attempt_id: Some(attempt()),
            receipt,
        });
    });
    store
        .mark_canonical_transcript_ready(cursor, cursor)
        .unwrap();
    publish_source_fact(projection, |projection| {
        projection.apply(ConversationObservation::TranscriptSourceReady(
            store.transcript_source_ready().unwrap(),
        ));
    });
}

fn assert_frozen_tool_result_absent(
    store: &dyn ConversationStore,
    cut: &crate::observation::cut::CheckpointReadCut,
    occurrence: &crate::events::types::ToolObservationOccurrence,
) {
    let frozen = super::super::super::trace::TraceProjection::at_cut(
        store,
        cut.source_cut.as_ref().unwrap(),
    )
    .unwrap()
    .page(None, 32)
    .unwrap();
    assert!(
        frozen
            .records
            .iter()
            .find(|row| row.kind == crate::observation::trace::TraceKind::Tool)
            .unwrap()
            .tool
            .as_ref()
            .unwrap()
            .outcome
            .is_none()
    );
    assert!(
        store
            .observation_trace_tool_result(cut.source_cut.as_ref().unwrap(), occurrence)
            .unwrap()
            .is_none()
    );
}

/// Capacity suite: this test's value comes from its scale, so FAST skips
/// `::scale::` and FULL runs it (tests/README.md, "Capacity tests").
mod scale {
    use crate::durable::{ObservationStore};
    use super::*;

    #[test]
    fn t08_t14_t19_cached_trace_cut_survives_c_without_j_and_4097_unrelated_publications() {
        let (mut projection, store, call, witness) = native_running_tool_fixture();
        install_current_trace(&mut projection, &store);
        let initial = projection.checkpoint_candidate().unwrap();
        let cached = initial.snapshot.trace.clone();
        let issued = cached.read_cut.as_ref().unwrap().clone();
        assert_eq!(issued, initial.read_cut);
        assert!(projection.replay.back().unwrap().1.facts().iter().any(|event| {
            matches!(event, RuntimeClientEvent::ReadDomainsUpdated {trace,..} if trace == &cached)
        }));
        let held_fence = projection.read_domain_fence();
        let held_read = held_fence.read(&store).unwrap();
        let occurrence = witness.tool_occurrence.unwrap();
        assert!(
            store
                .observation_trace_tool_result(issued.source_cut.as_ref().unwrap(), &occurrence)
                .unwrap()
                .is_none()
        );

        commit_result_without_terminal_j(&mut projection, &store, call);
        let later = projection.checkpoint_candidate().unwrap();
        assert_eq!(later.read_cut.through, issued.through);
        assert!(
            later.read_cut.source_cut.as_ref().unwrap().ledger_through
                > issued.source_cut.as_ref().unwrap().ledger_through
        );
        assert_eq!(later.snapshot.trace, cached);
        assert!(
            !projection.install_read_domains(&held_fence, Ok(held_read), true),
            "a gated old worker result cannot install after C advances"
        );
        assert!(
            store
                .observation_trace_tool_result(later.read_cut.source_cut.as_ref().unwrap(), &occurrence)
                .unwrap()
                .is_some()
        );

        for _ in 0..4_097 {
            publish_source_fact(&mut projection, |projection| {
                projection.publish(RuntimeClientEvent::TraceChanged);
            });
        }
        assert!(projection.earliest_serviceable() > initial.cursor);
        assert!(!projection.replay_cut_cursors.contains_key(&issued.revision));
        assert_eq!(
            projection.checkpoint_candidate().unwrap().snapshot.trace,
            cached
        );
        projection.validate_checkpoint_cut(&issued).unwrap();
        assert_frozen_tool_result_absent(&store, &issued, &occurrence);
        let mut tampered = issued.clone();
        tampered.through += 1;
        assert!(matches!(
            projection.validate_checkpoint_cut(&tampered),
            Err(RuntimeClientError::ReadCutExpired)
        ));

        install_current_trace(&mut projection, &store);
        let refreshed = projection.checkpoint_candidate().unwrap();
        assert_eq!(
            refreshed.snapshot.trace.read_cut.as_ref(),
            Some(&refreshed.read_cut)
        );
        assert!(
            matches!(
                projection.validate_checkpoint_cut(&issued),
                Err(RuntimeClientError::ReadCutExpired)
            ),
            "retiring the published cache retires its cut pin"
        );
    }
}

#[test]
fn t08_t14_cached_trace_cut_is_not_retagged_by_todo_or_unchanged_installation() {
    let (mut projection, store, _, _) = native_running_tool_fixture();
    install_current_trace(&mut projection, &store);
    let cached = projection.checkpoint_candidate().unwrap().snapshot.trace;
    let cursor = projection.cursor;
    projection.read_domains_dirty = true;
    install_current_trace(&mut projection, &store);
    assert_eq!(
        projection.cursor, cursor,
        "an identical source installation creates no publication"
    );
    publish_source_fact(&mut projection, |projection| {
        projection.publish(RuntimeClientEvent::ReadDomainsUpdated {
            projection_head: projection.snapshot.context.projection_head.clone(),
            transcript: projection.snapshot.transcript.clone(),
            trace: projection.snapshot.trace.clone(),
            occupancy: projection.snapshot.context.occupancy.clone(),
            todos: projection.snapshot.todos.clone(),
        });
    });
    assert_eq!(
        projection.checkpoint_candidate().unwrap().snapshot.trace,
        cached
    );
    assert!(projection.replay.back().unwrap().1.facts().iter().any(|event| {
        matches!(event, RuntimeClientEvent::ReadDomainsUpdated {trace,..} if trace == &cached)
    }));
}
