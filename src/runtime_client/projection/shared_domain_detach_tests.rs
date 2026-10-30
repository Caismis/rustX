use crate::runtime_client::projection::{ReadDomainValues, update_stored_tool_slots};
use crate::runtime_client::snapshot::{RuntimeClientTranscriptEntry, RuntimeClientTranscriptItem};

#[test]
fn stored_tool_update_detaches_only_matching_domains_and_keeps_old_seed() {
    let (projection, _, _, _) = native_running_tool_fixture();
    let mut source = projection.snapshot.clone();
    let slot = source.message_descriptors[0].tool_calls[0].clone();
    source.transcript.entries = vec![RuntimeClientTranscriptEntry {
        turn_process: None,
        response_pending: false,
        completed_response: None,
        tool_calls: vec![slot.clone()],
        cursor: RuntimeClientTranscriptCursor::new(1),
        item: RuntimeClientTranscriptItem::Message {
            message: compactable_user("shared-tool-entry"),
        },
    }]
    .into();
    for (descriptor, transcript) in [(false, false), (true, false), (false, true), (true, true)] {
        let mut current = source.clone();
        if !descriptor {
            current.message_descriptors[0].tool_calls[0]
                .occurrence
                .block_index = ContentBlockIndex::new(99);
        }
        if !transcript {
            current.transcript.entries[0].tool_calls[0]
                .occurrence
                .block_index = ContentBlockIndex::new(99);
        }
        let frozen = current.clone();
        let before = serde_json::to_vec(&frozen.materialize()).unwrap();
        let mut visits = Vec::new();
        update_stored_tool_slots(&mut current, &slot.occurrence, |tool| {
            visits.push(tool.name.clone());
            tool.name = format!("updated-{}", visits.len());
        });
        assert_eq!(
            visits.len(),
            usize::from(descriptor) + usize::from(transcript)
        );
        assert_eq!(
            current
                .message_descriptors
                .same_owner(&frozen.message_descriptors),
            !descriptor
        );
        assert_eq!(
            current
                .transcript
                .entries
                .same_owner(&frozen.transcript.entries),
            !transcript
        );
        if descriptor {
            assert_eq!(
                current.message_descriptors[0].tool_calls[0].name,
                "updated-1"
            );
        }
        if transcript {
            assert_eq!(
                current.transcript.entries[0].tool_calls[0].name,
                format!("updated-{}", visits.len())
            );
        }
        assert_eq!(serde_json::to_vec(&frozen.materialize()).unwrap(), before);
    }
}

fn current_read_domains(projection: &RuntimeClientProjection) -> ReadDomainValues {
    ReadDomainValues {
        projection_head: projection.snapshot.context.projection_head.clone(),
        transcript: projection.snapshot.transcript.owned(),
        transcript_rows: projection.transcript_head.clone(),
        occupancy: projection.snapshot.context.occupancy.clone(),
        trace: Some(projection.snapshot.trace.owned()),
        trace_total: projection.snapshot.trace.records.len() as u64,
    }
}

#[test]
fn read_domain_envelope_update_keeps_exact_empty_domain_owners() {
    let (mut projection, _) = initialized_source_projection();
    projection.snapshot.transcript.statistics = None.into();
    let previous = projection.capture_working_seed();
    let before = serde_json::to_vec(&previous.snapshot.materialize()).unwrap();
    let mut read = current_read_domains(&projection);
    read.transcript.next_cursor = Some(RuntimeClientTranscriptCursor::new(7));
    projection.read_domains_dirty = true;
    assert!(projection.install_read_domains(&projection.read_domain_fence(), Ok(read), false));
    assert!(
        projection
            .snapshot
            .context
            .same_owner(&previous.snapshot.context)
    );
    assert!(
        projection
            .snapshot
            .transcript
            .content
            .same_owner(&previous.snapshot.transcript.content)
    );
    assert!(
        projection
            .snapshot
            .transcript
            .collections
            .same_owner(&previous.snapshot.transcript.collections)
    );
    assert!(
        projection
            .snapshot
            .transcript
            .deferred_entries
            .same_owner(&previous.snapshot.transcript.deferred_entries)
    );
    assert!(
        projection
            .snapshot
            .transcript
            .statistics
            .same_owner(&previous.snapshot.transcript.statistics)
    );
    assert!(
        projection
            .snapshot
            .transcript
            .entries
            .same_owner(&previous.snapshot.transcript.entries)
    );
    assert!(
        projection
            .snapshot
            .trace
            .records
            .same_owner(&previous.snapshot.trace.records)
    );
    assert!(
        projection
            .snapshot
            .trace
            .updates
            .same_owner(&previous.snapshot.trace.updates)
    );
    assert_eq!(
        projection.snapshot.transcript.next_cursor,
        Some(RuntimeClientTranscriptCursor::new(7))
    );
    assert_eq!(
        serde_json::to_vec(&previous.snapshot.materialize()).unwrap(),
        before
    );
}

#[test]
fn read_domain_mixed_update_keeps_signed_zero_and_json_order_changes() {
    let (mut projection, _) = initialized_source_projection();
    projection.snapshot.transcript.statistics =
        Some(crate::runtime_client::response::ConversationStatistics {
            timing: Some(crate::durable::response::ConversationTiming {
                output_tokens_per_second: Some(0.0),
                ..Default::default()
            }),
            ..Default::default()
        })
        .into();
    projection.snapshot.transcript.deferred_entries =
        vec![crate::runtime_client::checkpoint::CheckpointPageItem {
            body_complete: None,
            key: None,
            offset: 1,
            identity: "ordered".into(),
            value: Some(serde_json::from_str(r#"{"first":0.0,"second":2}"#).unwrap()),
            content: Vec::new(),
            deferred: true,
        }]
        .into();
    let previous = projection.capture_working_seed();
    let before = serde_json::to_vec(&previous.snapshot.materialize()).unwrap();
    let mut read = current_read_domains(&projection);
    read.transcript
        .statistics
        .as_mut()
        .unwrap()
        .timing
        .as_mut()
        .unwrap()
        .output_tokens_per_second = Some(-0.0);
    read.transcript.deferred_entries[0].value =
        Some(serde_json::from_str(r#"{"second":2,"first":-0.0}"#).unwrap());
    assert!(
        projection.snapshot.transcript == read.transcript,
        "native equality misses both wire changes"
    );
    let expected = serde_json::to_vec(&read.transcript).unwrap();
    read.occupancy = Some(
        crate::runtime_client::occupancy::RuntimeClientContextOccupancy {
            estimated: true,
            input_tokens: 1,
            context_window_tokens: 10,
            model: None,
            model_deferred: false,
            model_source: None,
            breakdown: crate::context::occupancy::ContextBreakdown {
                system_tokens: 1,
                tool_tokens: 0,
                message_tokens: 0,
            },
        },
    );
    projection.read_domains_dirty = true;
    assert!(projection.install_read_domains(&projection.read_domain_fence(), Ok(read), false));
    assert_eq!(
        serde_json::to_vec(&projection.snapshot.transcript.owned()).unwrap(),
        expected
    );
    assert!(
        !projection
            .snapshot
            .context
            .same_owner(&previous.snapshot.context)
    );
    assert!(
        !projection
            .snapshot
            .transcript
            .statistics
            .same_owner(&previous.snapshot.transcript.statistics)
    );
    assert!(
        !projection
            .snapshot
            .transcript
            .deferred_entries
            .same_owner(&previous.snapshot.transcript.deferred_entries)
    );
    assert_eq!(
        serde_json::to_vec(&previous.snapshot.materialize()).unwrap(),
        before
    );
}
