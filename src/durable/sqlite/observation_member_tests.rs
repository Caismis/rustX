// Member headers are optional reads over original committed provenance.
#[test]
fn t09_t12_t19_pending_layout_cleanup_failure_cannot_refuse_native_adoption() {
    for table in ["content_segments", "content_frontiers"] {
        let store = store();
        store.initialize(&[]).unwrap();
        let accepted = store
            .accept_inbound(draft("original accepted body🙂"))
            .unwrap();
        let before = accepted.read_cut.clone();
        let original = store
            .pending_value_read(&before, accepted.sequence.get(), 0, 65_536)
            .unwrap();
        let original: UserMessageBlock = crate::runtime_client::checkpoint::decode_domain_value(
            serde_json::from_slice(&original.bytes).unwrap(),
        )
        .unwrap();
        assert_eq!(original, accepted.message);
        store.lock().unwrap().execute_batch(&format!(
            "CREATE TRIGGER pending_cleanup_fault BEFORE DELETE ON {table} WHEN OLD.source_kind='pending' BEGIN SELECT RAISE(FAIL,'optional pending cleanup'); END;"
        )).unwrap();
        let adopted = store.adopt_pending_batch(accepted.sequence, None).unwrap();
        assert_eq!(adopted.items.len(), 1);
        assert_eq!(adopted.items[0].message, accepted.message);
        assert!(store.load_pending().unwrap().is_empty());
        assert_eq!(
            store.load_canonical().unwrap(),
            vec![MessageBlock::User(accepted.message.clone())]
        );
        assert_eq!(
            adopted.read_cut.mutation_revision,
            before.mutation_revision + 1
        );
        assert_eq!(adopted.read_cut.pending_total, 0);
        assert_eq!(adopted.read_cut.ledger_through, 1);
        assert_eq!(adopted.read_cut.transcript, before.transcript);
        let event = adopted.adoption.as_ref().unwrap();
        assert!(
            matches!(&event.event, RuntimeEvent::InboundTurnAdopted { message_ids } if message_ids == std::slice::from_ref(&accepted.message_id))
        );
        assert_eq!(event.sequence, before.journal + 1);
        assert_eq!(
            store.read_events(Some(before.journal), 1).unwrap().events[0].event,
            event.event
        );
        assert!(!event.observation.as_ref().unwrap().source_unavailable);
        assert!(!adopted.canonical_receipts[0].observation.source_unavailable);
        assert_eq!(adopted.pending_observation.as_ref().unwrap().total, 0);
        assert_stale_pending_layout_is_unreachable(
            &store,
            &before,
            &adopted.read_cut,
            accepted.sequence.get(),
        );
        store
            .lock()
            .unwrap()
            .execute_batch("DROP TRIGGER pending_cleanup_fault")
            .unwrap();
        let next = store
            .accept_inbound(draft("new original occurrence"))
            .unwrap();
        assert!(next.sequence > accepted.sequence);
        assert!(
            store
                .pending_value_read(&next.read_cut, next.sequence.get(), 0, 65_536)
                .is_ok()
        );
        assert!(matches!(
            store.pending_value_read(&next.read_cut, accepted.sequence.get(), 0, 65_536),
            Err(ConversationStoreError::InvalidReference(_))
        ));
        store.adopt_pending_batch(next.sequence, None).unwrap();
        let remaining: u64 = store.lock().unwrap().query_row("SELECT COUNT(*) FROM content_frontiers WHERE source_kind='pending' AND source_id=?1",[next.sequence.get().to_string()],|row|row.get(0)).unwrap();
        assert_eq!(
            remaining, 0,
            "unfaulted cleanup still removes derived bytes"
        );
    }
}

fn assert_stale_pending_layout_is_unreachable(
    store: &SqliteConversationStore,
    before: &crate::durable::reading::ConversationReadCut,
    after: &crate::durable::reading::ConversationReadCut,
    sequence: u64,
) {
    let connection = store.lock().unwrap();
    for table in ["content_segments", "content_frontiers"] {
        let remaining: u64 = connection
            .query_row(
                &format!(
                    "SELECT COUNT(*) FROM {table} WHERE source_kind='pending' AND source_id=?1"
                ),
                [sequence.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            remaining > 0,
            "failed cleanup is real, not inferred from an empty source"
        );
    }
    drop(connection);
    assert!(matches!(
        store.pending_value_read(before, sequence, 0, 65_536),
        Err(ConversationStoreError::ObservationCutExpired)
    ));
    assert!(matches!(
        store.pending_header_page(before, None, 32),
        Err(ConversationStoreError::ObservationCutExpired)
    ));
    assert!(matches!(
        store.pending_value_read(after, sequence, 0, 65_536),
        Err(ConversationStoreError::InvalidReference(_))
    ));
    let page = store.pending_header_page(after, None, 32).unwrap();
    assert_eq!(page.total, 0);
    assert!(page.headers.is_empty() && page.next_sequence.is_none());
    let mut foreign = after.clone();
    foreign.conversation_id = ConversationId::new("conv_2be184c1-ab11-725b-82c3-81daea936198");
    assert!(matches!(
        store.pending_value_read(&foreign, sequence, 0, 65_536),
        Err(ConversationStoreError::ObservationCutExpired)
    ));
}

fn original_event_result_ranges(store: &SqliteConversationStore, sequence: u64) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut offset = 0;
    loop {
        let chunk = store
            .event_value_read(
                sequence,
                sequence,
                &["event".into(), "result".into()],
                offset,
                65_536,
            )
            .unwrap();
        let (rows, materialized) = SqliteConversationStore::content_read_work();
        assert!(rows <= 2 && materialized <= 65_536);
        bytes.extend(chunk.bytes);
        let Some(next) = chunk.next_offset else { break };
        assert!(next > offset);
        offset = next;
    }
    bytes
}

fn original_tool_before_result() -> (SqliteConversationStore, AttemptId, MessageBlock, ToolCall) {
    let store = store();
    store.initialize(&[]).unwrap();
    let attempt = AttemptId::new("successful-original-owner");
    let (proposal, call) = derived_enrichment_proposal();
    store
        .append_canonical_with_event(
            &proposal,
            envelope(
                store.conversation_id(),
                "original-proposal",
                Some(attempt.clone()),
                RuntimeEvent::AssistantMessageCommitted {
                    message_id: proposal.id().clone(),
                },
            ),
        )
        .unwrap();
    store
        .append_event(envelope(
            store.conversation_id(),
            "original-start",
            Some(attempt.clone()),
            RuntimeEvent::ToolExecutionStarted {
                tool_call_id: call.id.clone(),
                tool_id: call.tool_id.clone(),
            },
        ))
        .unwrap();
    (store, attempt, proposal, call)
}

fn original_success_tool_result() -> ToolExecutionResult {
    use crate::tools::types::ToolResultContent;
    ToolExecutionResult {
        status: ToolExecutionStatus::Success,
        content: vec![
            ToolResultContent::Text(TextBlock {
                text: "original🙂".repeat(10_000),
            }),
            ToolResultContent::Json {
                value: serde_json::json!({"content":[{"not":"a ContentBinding"}],"collections":[{"not":"a frontier"}],"integer":u64::MAX}),
            },
        ],
        duration_ms: 7,
        exit_code: None,
        artifacts: vec![],
        deliveries: vec![],
        truncation: None,
        workflow: None,
        managed_output: None,
    }
}

fn commit_original_tool_result(
    store: &SqliteConversationStore,
    attempt: &AttemptId,
    proposal: &MessageBlock,
    call: &ToolCall,
    result: &ToolExecutionResult,
) {
    let message = MessageBlock::Tool(ToolMessageBlock {
        id: MessageId::new("original-success-c"),
        occurrence: crate::message::types::ToolCallOccurrenceRef::new(
            proposal.id().clone(),
            ContentBlockIndex::new(0),
        ),
        tool_call_id: call.id.clone(),
        tool_id: call.tool_id.clone(),
        result: result.clone(),
    });
    let (committed, receipt) = store
        .append_canonical_with_event(
            &message,
            envelope(
                store.conversation_id(),
                "original-result-commit",
                Some(attempt.clone()),
                RuntimeEvent::ToolMessageCommitted {
                    tool_call_id: call.id.clone(),
                    message_id: message.id().clone(),
                },
            ),
        )
        .unwrap();
    assert_eq!(receipt.ledger_position, 2);
    assert!(!committed.observation.as_ref().unwrap().source_unavailable);
    assert_eq!(
        store.load_messages(&[message.id().clone()]).unwrap(),
        vec![message]
    );
}

#[test]
fn t08_t12_t19_successful_tool_j_before_c_keeps_original_content_and_available_source() {
    let (store, attempt, proposal, call) = original_tool_before_result();
    let result = original_success_tool_result();
    let terminal = store
        .append_event(envelope(
            store.conversation_id(),
            "original-success",
            Some(attempt.clone()),
            RuntimeEvent::ToolExecutionCompleted {
                tool_call_id: call.id.clone(),
                tool_id: call.tool_id.clone(),
                result: result.clone(),
            },
        ))
        .unwrap();
    let before = store.observation_read_cut().unwrap();
    assert_eq!(
        before.ledger_through, 1,
        "own successful J precedes result C"
    );
    assert!(!terminal.observation.as_ref().unwrap().source_unavailable);
    let witness = store
        .observation_event_witness(terminal.sequence, terminal.sequence)
        .unwrap()
        .unwrap();
    assert!(!witness.source_unavailable);
    assert!(witness.result_ledger_position.is_none());
    let page = store
        .event_value_page(
            terminal.sequence,
            terminal.sequence,
            &["event".into()],
            0,
            32,
        )
        .unwrap();
    let field = page
        .fields
        .iter()
        .find(|field| field.path.last().is_some_and(|name| name == "result"))
        .unwrap();
    assert!(field.content.is_empty() && field.collections.is_empty());
    let schema = crate::runtime_client::structural_json::root::<ToolExecutionResult>();
    let expected = serde_json::to_vec(&crate::runtime_client::structural_json::Encoded {
        value: &result,
        schema: &schema,
        root: &schema,
    })
    .unwrap();
    assert_eq!(
        original_event_result_ranges(&store, terminal.sequence),
        expected
    );
    let original = store
        .read_events(Some(terminal.sequence - 1), 1)
        .unwrap()
        .events
        .remove(0);
    assert_eq!(original.event, terminal.event);
    commit_original_tool_result(&store, &attempt, &proposal, &call, &result);
    assert_eq!(
        original_event_result_ranges(&store, terminal.sequence),
        expected
    );
}

#[test]
fn t08_t12_t19_request_metadata_requires_the_original_message_producer_path() {
    let store = store();
    store
        .initialize(&[user_message(
            "metadata-original-input",
            &"original🙂".repeat(10_000),
        )])
        .unwrap();
    let mut snapshot = progress_request_snapshot(
        "metadata-owner",
        "1",
        0,
        store.load_head().unwrap().revision,
        vec![],
    );
    let opaque = serde_json::json!({"content":[{"not":"a ContentBinding"}],"collections":[{"not":"a frontier"}],"integer":7});
    snapshot.tool_definitions = vec![crate::tools::types::ModelToolDefinition {
        id: ToolId::new("metadata-tool"),
        name: "Original".into(),
        description: "Original schema".into(),
        input_schema: opaque.clone(),
    }];
    let mut deferred = snapshot.tool_definitions[0].clone();
    deferred.id = ToolId::new("metadata-unsafe-original");
    deferred.input_schema["integer"] = serde_json::json!(u64::MAX);
    snapshot.tool_definitions.push(deferred);
    let started = store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap()
        .started;
    assert!(!started.observation.as_ref().unwrap().source_unavailable);
    let path = vec![
        "tools".into(),
        "0".into(),
        "input_schema".into(),
        "value".into(),
    ];
    let selected = store
        .request_field_header_at(started.sequence, started.sequence, &path)
        .unwrap();
    assert_eq!(selected.value, opaque);
    assert!(selected.content.is_empty() && selected.collections.is_empty());
    let page = store
        .request_field_page_at(
            started.sequence,
            started.sequence,
            &["tools".into(), "0".into(), "input_schema".into()],
            0,
            32,
        )
        .unwrap();
    let schema = page.fields.iter().find(|field| field.path == path).unwrap();
    assert_eq!(schema.value, opaque);
    assert!(schema.content.is_empty() && schema.collections.is_empty());
    let messages = store
        .request_field_page_at(
            started.sequence,
            started.sequence,
            &["messages".into()],
            0,
            32,
        )
        .unwrap();
    let message = &messages.fields[0];
    assert!(!message.content.is_empty() && !message.collections.is_empty());
    let selected = store
        .request_field_header_at(started.sequence, started.sequence, &message.path)
        .unwrap();
    assert_eq!(selected.content, message.content);
    assert_eq!(selected.collections, message.collections);
    assert_deferred_original_schema(
        &store,
        started.sequence,
        &snapshot.tool_definitions[1].input_schema,
    );
    assert_eq!(
        store.load_request_snapshot(&snapshot.request_id).unwrap(),
        snapshot
    );
}

fn assert_deferred_original_schema(
    store: &SqliteConversationStore,
    sequence: u64,
    original: &serde_json::Value,
) {
    let path = vec![
        "tools".into(),
        "1".into(),
        "input_schema".into(),
        "value".into(),
    ];
    let field = store
        .request_field_header_at(sequence, sequence, &path)
        .unwrap();
    assert!(
        field.value.is_null() && field.deferred,
        "unsafe opaque integers retain their exact byte source"
    );
    assert!(field.content.is_empty() && field.collections.is_empty());
    let page = store
        .request_field_page_at(
            sequence,
            sequence,
            &["tools".into(), "1".into(), "input_schema".into()],
            0,
            32,
        )
        .unwrap();
    assert_eq!(
        serde_json::to_value(page.fields.iter().find(|field| field.path == path).unwrap()).unwrap(),
        serde_json::to_value(&field).unwrap()
    );
    let chunk = store
        .request_value_read_at(sequence, sequence, &path, 0, 65_536)
        .unwrap();
    assert!(chunk.next_offset.is_none());
    assert_eq!(chunk.bytes, serde_json::to_vec(original).unwrap());
    let (rows, bytes) = SqliteConversationStore::content_read_work();
    assert!(rows <= 2 && bytes <= 65_536);
}

#[test]
fn t10_t14_t19_member_seed_failure_preserves_original_lineage_and_explicit_unavailability() {
    let store = store();
    store.lock().unwrap().execute_batch("CREATE TRIGGER member_seed_fault BEFORE UPDATE OF observation_members ON inherited_turn_provenance BEGIN SELECT RAISE(FAIL,'derived member seed fault'); END;").unwrap();
    let seed = statistics_inherited_seed();
    store.initialize_lineage(&seed).unwrap();
    assert_eq!(store.load_canonical().unwrap().as_slice(), seed.canonical());
    assert!(
        store
            .lock()
            .unwrap()
            .query_row(
                "SELECT bootstrap_initialized FROM rustx_store WHERE id=1",
                [],
                |row| row.get::<_, bool>(0)
            )
            .unwrap()
    );
    assert_eq!(
        store
            .lock()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM inherited_message_membership",
                [],
                |row| row.get::<_, u64>(0)
            )
            .unwrap(),
        1
    );
    let error = store
        .observation_member_counts(&MessageId::new("inherited-stats-answer"), 0)
        .unwrap_err();
    assert!(matches!(error, ConversationStoreError::Storage(_)));
    assert!(matches!(
        crate::runtime_client::RuntimeClientError::observation_store_error(error),
        crate::runtime_client::RuntimeClientError::ObservationUnavailable { .. }
    ));
    assert_eq!(store.observation_statistics(0).unwrap().model_requests, 9);
}

#[test]
fn t08_t14_t19_giant_final_member_identity_is_deferred_until_original_terminal() {
    let store = store();
    store.initialize(&[]).unwrap();
    let attempt = AttemptId::new("member-giant-final");
    store
        .append_event(envelope(
            store.conversation_id(),
            "member-start",
            Some(attempt.clone()),
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt.clone(),
            },
        ))
        .unwrap();
    let original = "final🙂水".repeat(80_000);
    let id = MessageId::new(&original);
    let (committed, _) = store
        .append_canonical_with_event(
            &assistant_message(&original, "native final body"),
            envelope(
                store.conversation_id(),
                "member-canonical",
                Some(attempt.clone()),
                RuntimeEvent::AssistantMessageCommitted {
                    message_id: id.clone(),
                },
            ),
        )
        .unwrap();
    let before = store
        .observation_member_counts(&id, committed.sequence)
        .unwrap()
        .unwrap();
    assert_eq!(before.message_count, 1);
    assert!(
        !before.completed_response,
        "canonical C does not prove successful execution"
    );
    assert!(before.final_message_id.is_none());
    assert!(before.final_message_id_deferred);
    assert!(matches!(
        before.final_message_locator,
        Some(crate::runtime_client::content::CanonicalContentLocator::ByLedgerPosition { .. })
    ));
    assert!(serde_json::to_vec(&before).unwrap().len() <= 16_384);
    assert_eq!(
        member_original_identity(&store, before.final_message_id_source.as_ref().unwrap()),
        original
    );
    let terminal = store
        .append_event(envelope(
            store.conversation_id(),
            "member-terminal",
            Some(attempt.clone()),
            RuntimeEvent::AttemptCompleted {
                attempt_id: attempt,
                finish_reason: ModelFinishReason::Stop,
            },
        ))
        .unwrap();
    let after = store
        .observation_member_counts(&id, terminal.sequence)
        .unwrap()
        .unwrap();
    assert!(after.completed_response);
    assert_eq!(after.final_message_locator, before.final_message_locator);
    assert_eq!(
        after.final_message_id_source,
        before.final_message_id_source
    );
    assert_eq!(
        store
            .observation_member_counts(&id, committed.sequence)
            .unwrap(),
        Some(before)
    );
    store
        .lock()
        .unwrap()
        .execute(
            "UPDATE events SET observation_members=NULL WHERE sequence=?1",
            [terminal.sequence],
        )
        .unwrap();
    assert!(matches!(
        store.observation_member_counts(&id, terminal.sequence),
        Err(ConversationStoreError::Storage(_))
    ));
    assert_eq!(
        store
            .read_events(Some(terminal.sequence - 1), 1)
            .unwrap()
            .events[0]
            .sequence,
        terminal.sequence
    );
    assert_eq!(store.load_messages(&[id]).unwrap().len(), 1);
}

fn member_original_identity(
    store: &SqliteConversationStore,
    reference: &crate::runtime_client::content::ContentRef,
) -> String {
    use base64::Engine;
    let mut bytes = Vec::new();
    let mut offset = 0;
    loop {
        let chunk = store.content_read(reference, offset, 65_536).unwrap();
        let original = base64::engine::general_purpose::STANDARD
            .decode(chunk.data_base64)
            .unwrap();
        assert!(!original.is_empty());
        assert!(original.len() <= 65_536);
        bytes.extend(original);
        let Some(next) = chunk.next_offset else { break };
        assert!(next > offset);
        offset = next;
    }
    serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn member_completed_lineage(original: &str) -> LineageSeed {
    use crate::durable::reading::{
        ConversationTurnId, InheritedTurnOutcome, TurnExecution, TurnReadingProvenance,
    };
    use crate::durable::response::{CompletedResponseProvenance, ResponseOrigin};
    let id = MessageId::new(original);
    let conversation = ConversationId::generate();
    let attempt = AttemptId::new("inherited-giant-final");
    let at = Utc.with_ymd_and_hms(2026, 9, 1, 12, 0, 0).unwrap();
    LineageSeed::history(vec![
        user_message("giant-member-input", "native input"),
        assistant_message(original, "original final"),
    ])
    .with_turns(vec![TurnReadingProvenance {
        id: ConversationTurnId {
            conversation_id: conversation.clone(),
            attempt_id: attempt.clone(),
        },
        process_message_ids: vec![id.clone()],
        preceding_message_id: None,
        prompt_message_id: Some(MessageId::new("giant-member-input")),
        outcome: InheritedTurnOutcome::Completed,
        started_at: Some(at),
        ended_at: Some(at),
        execution: Some(TurnExecution {
            completed_response: true,
            ..Default::default()
        }),
    }])
    .unwrap()
    .with_completed_responses(vec![CompletedResponseProvenance {
        process_message_ids: vec![id.clone()],
        closing_message_id: id.clone(),
        origin: ResponseOrigin {
            conversation_id: conversation,
            attempt_id: attempt,
            closing_message_id: id,
        },
        completed_at: at,
        retry_message_id: None,
        usage: None,
        timing: None,
        models: Vec::new(),
    }])
    .unwrap()
}

#[test]
fn t08_t12_t14_t19_giant_completed_member_identity_survives_native_lineage_copy() {
    let source = store();
    let original = "inherited-final🙂水".repeat(70_000);
    let seed = member_completed_lineage(&original);
    source.initialize_lineage(&seed).unwrap();
    let cut = source
        .read_lineage_cut(source.load_head().unwrap().revision)
        .unwrap();
    assert_eq!(cut.completed_responses, seed.completed_responses());
    let copied = LineageSeed::replayed(cut.canonical, cut.surface_history, cut.checkpoints)
        .unwrap()
        .with_turns(cut.turns)
        .unwrap()
        .with_completed_responses(cut.completed_responses)
        .unwrap();
    let destination = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    assert_ne!(source.conversation_id(), destination.conversation_id());
    destination.initialize_lineage(&copied).unwrap();
    let counts = destination
        .observation_member_counts(&MessageId::new(&original), 0)
        .unwrap()
        .unwrap();
    assert!(counts.completed_response);
    assert_eq!(counts.message_count, 1);
    assert!(counts.final_message_id.is_none());
    assert!(counts.final_message_id_deferred);
    assert!(serde_json::to_vec(&counts).unwrap().len() <= 16_384);
    let reference = counts.final_message_id_source.unwrap();
    assert_eq!(&reference.conversation_id, destination.conversation_id());
    assert_eq!(member_original_identity(&destination, &reference), original);
    assert_eq!(
        destination
            .observation_statistics(0)
            .unwrap()
            .completed_responses,
        1
    );
}

#[test]
fn t10_t14_t19_statistics_count_overflow_keeps_original_bootstrap_and_reports_unavailable() {
    let original = statistics_inherited_seed();
    let mut turns = original.turns().to_vec();
    turns[0].execution.as_mut().unwrap().steps = u64::MAX;
    let mut next = turns[0].clone();
    next.id.attempt_id = AttemptId::new("overflow-second-attempt");
    next.process_message_ids = vec![MessageId::new("overflow-second-answer")];
    next.execution.as_mut().unwrap().steps = 1;
    turns.push(next);
    let mut canonical = original.canonical().to_vec();
    canonical.push(assistant_message(
        "overflow-second-answer",
        "original second",
    ));
    let seed = LineageSeed::history(canonical).with_turns(turns).unwrap();
    let store = store();
    store.initialize_lineage(&seed).unwrap();
    assert_eq!(store.load_canonical().unwrap().as_slice(), seed.canonical());
    assert_eq!(store.load_inherited_turns().unwrap(), seed.turns());
    let error = store.observation_statistics(0).unwrap_err();
    assert!(matches!(
        crate::runtime_client::RuntimeClientError::observation_store_error(error),
        crate::runtime_client::RuntimeClientError::ObservationUnavailable { .. }
    ));
    assert!(
        store
            .lock()
            .unwrap()
            .query_row(
                "SELECT bootstrap_initialized FROM rustx_store WHERE id=1",
                [],
                |row| row.get::<_, bool>(0)
            )
            .unwrap()
    );
}

#[test]
fn t14_t19_unowned_assistant_journal_has_no_fabricated_member_owner() {
    let store = store();
    store.initialize(&[]).unwrap();
    let id = MessageId::new("unowned-assistant");
    let (event, _) = store
        .append_canonical_with_event(
            &assistant_message(id.as_str(), "original diagnostic"),
            envelope(
                store.conversation_id(),
                "unowned-canonical",
                None,
                RuntimeEvent::AssistantMessageCommitted {
                    message_id: id.clone(),
                },
            ),
        )
        .unwrap();
    assert_eq!(
        store
            .observation_member_counts(&id, event.sequence)
            .unwrap(),
        None
    );
}

fn member_source_page(
    store: &SqliteConversationStore,
    cut: &crate::durable::observation::ObservationReadCut,
) -> crate::runtime_client::snapshot::RuntimeClientTranscriptPage {
    let mut page = crate::runtime_client::snapshot::transcript_header_view(
        store.transcript_header_page(cut, None, 32).unwrap(),
    );
    crate::runtime_client::response::decorate_window(store, &mut page, cut.read.journal).unwrap();
    page
}

#[test]
fn t08_t14_t19_deferred_member_descriptor_preserves_original_fields_and_frozen_process() {
    let store = store();
    store.initialize(&[]).unwrap();
    let attempt = AttemptId::new("deferred-member-owner");
    store
        .append_event(envelope(
            store.conversation_id(),
            "member-owner-start",
            Some(attempt.clone()),
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt.clone(),
            },
        ))
        .unwrap();
    let original = "deferred-final🙂水".repeat(80_000);
    let id = MessageId::new(&original);
    store
        .append_canonical_with_event(
            &assistant_message(&original, "native positive body"),
            envelope(
                store.conversation_id(),
                "deferred-member-c",
                Some(attempt.clone()),
                RuntimeEvent::AssistantMessageCommitted {
                    message_id: id.clone(),
                },
            ),
        )
        .unwrap();
    let running_cut = store.observation_read_cut().unwrap();
    let running = member_source_page(&store, &running_cut);
    let row = running
        .deferred_entries
        .iter()
        .find(|row| {
            row.value
                .as_ref()
                .is_some_and(|value| value["role"] == "assistant")
        })
        .unwrap();
    let value = row.value.as_ref().unwrap();
    assert_eq!(value["turn_process"]["outcome"], "running");
    assert!(value["turn_process"]["final_message_locator"].is_null());
    assert!(value["completed_response"].is_null());
    assert!(value.get("locator").is_some());
    assert!(value.get("tool_calls").is_some());
    assert!(value.get("content").is_some());
    assert!(value.get("collections").is_some());
    store
        .append_event(envelope(
            store.conversation_id(),
            "deferred-member-terminal",
            Some(attempt.clone()),
            RuntimeEvent::AttemptCompleted {
                attempt_id: attempt,
                finish_reason: ModelFinishReason::Stop,
            },
        ))
        .unwrap();
    assert_eq!(member_source_page(&store, &running_cut), running);
    let complete = member_source_page(&store, &store.observation_read_cut().unwrap());
    let row = complete
        .deferred_entries
        .iter()
        .find(|row| {
            row.value
                .as_ref()
                .is_some_and(|value| value["role"] == "assistant")
        })
        .unwrap();
    let value = row.value.as_ref().unwrap();
    assert_eq!(value["turn_process"]["outcome"], "completed");
    assert_eq!(value["turn_process"]["final_message_id_deferred"], true);
    assert!(value["turn_process"]["final_message_id"].is_null());
    assert_eq!(
        value["completed_response"]["closing_locator"],
        value["locator"]
    );
    let reference =
        serde_json::from_value(value["turn_process"]["final_message_id_source"].clone()).unwrap();
    assert_eq!(member_original_identity(&store, &reference), original);
    assert!(crate::runtime_client::checkpoint::bounded_bytes(row, 16_384).is_some());
}

#[test]
fn t08_t12_t14_t19_inherited_deferred_members_use_paired_native_ledger_ownership() {
    let original = "inherited-final🙂".repeat(80_000);
    let source = member_completed_lineage(&original);
    let nonfinal = MessageId::new("inherited-nonfinal");
    let mut canonical = source.canonical().to_vec();
    canonical.insert(
        1,
        assistant_message(nonfinal.as_str(), "original intermediate"),
    );
    let mut turns = source.turns().to_vec();
    turns[0].process_message_ids.insert(0, nonfinal.clone());
    let mut responses = source.completed_responses().to_vec();
    responses[0].process_message_ids.insert(0, nonfinal.clone());
    let seed = LineageSeed::history(canonical)
        .with_turns(turns)
        .unwrap()
        .with_completed_responses(responses)
        .unwrap();
    let store = store();
    store.initialize_lineage(&seed).unwrap();
    let cut = store.observation_read_cut().unwrap();
    let page = member_source_page(&store, &cut);
    let deferred = page
        .deferred_entries
        .iter()
        .find(|row| {
            row.value
                .as_ref()
                .is_some_and(|value| value["role"] == "assistant")
        })
        .unwrap();
    let value = deferred.value.as_ref().unwrap();
    let process = store
        .observation_transcript_process(&cut, deferred.offset)
        .unwrap()
        .unwrap();
    assert_eq!(process.conversation_id, seed.turns()[0].id.conversation_id);
    assert_eq!(process.message_count, 1);
    assert_eq!(value["turn_process"]["outcome"], "completed");
    assert_eq!(
        value["completed_response"]["closing_locator"],
        value["locator"]
    );
    let nonfinal_l = store.message_ledger_position(&nonfinal).unwrap().unwrap();
    let final_l = store
        .message_ledger_position(&MessageId::new(&original))
        .unwrap()
        .unwrap();
    assert!(store.lock().unwrap().execute("UPDATE inherited_message_membership SET assistant_ledger_position=?1 WHERE assistant_ledger_position=?2", params![nonfinal_l,final_l]).is_err());
    // Also reject a different existing canonical position: independent foreign
    // keys would allow this substitution, while the paired identity FK cannot.
    assert!(store.lock().unwrap().execute("UPDATE inherited_message_membership SET assistant_ledger_position=1 WHERE assistant_ledger_position=?1", [final_l]).is_err());
    {
        let connection = store.lock().unwrap();
        connection.execute_batch("PRAGMA foreign_keys=OFF").unwrap();
        connection.execute("UPDATE inherited_message_membership SET assistant_ledger_position=1 WHERE assistant_ledger_position=?1", [final_l]).unwrap();
        connection.execute_batch("PRAGMA foreign_keys=ON").unwrap();
    }
    assert!(matches!(
        store.observation_transcript_process(&cut, deferred.offset),
        Err(ConversationStoreError::Storage(_))
    ));
    assert!(store.load_inherited_turns().is_err());
    assert!(store.initialize_lineage(&seed).is_err());
}

#[test]
fn t10_t14_t19_terminal_witness_enrichment_failure_preserves_original_failed_cancelled_j_and_t() {
    use crate::events::types::AttemptFailure;
    use crate::runtime::types::{CancellationReason, RuntimeError};
    for failed in [false, true] {
        let store = store();
        store.initialize(&[]).unwrap();
        let attempt = AttemptId::new("original-terminal-owner");
        store
            .append_event(envelope(
                store.conversation_id(),
                "terminal-owner-start",
                Some(attempt.clone()),
                RuntimeEvent::AttemptStarted {
                    attempt_id: attempt.clone(),
                },
            ))
            .unwrap();
        // The first derived witness write succeeds. Only the later T
        // enrichment faults, exercising the exact post-allocation seam.
        store.lock().unwrap().execute_batch("CREATE TRIGGER terminal_witness_enrichment_fault BEFORE UPDATE OF observation_witness ON events WHEN NEW.observation_transcript IS NOT NULL BEGIN SELECT RAISE(FAIL,'derived transcript witness failure'); END;").unwrap();
        let outcome = if failed {
            RuntimeEvent::AttemptFailed {
                attempt_id: attempt.clone(),
                error: AttemptFailure::Runtime {
                    error: RuntimeError::Internal {
                        message: "original runtime failure".into(),
                    },
                },
            }
        } else {
            RuntimeEvent::AttemptCancelled {
                attempt_id: attempt.clone(),
                reason: CancellationReason::UserRequested,
            }
        };
        let terminal = store
            .append_event(envelope(
                store.conversation_id(),
                "terminal-owner-end",
                Some(attempt.clone()),
                outcome.clone(),
            ))
            .unwrap();
        let original = store.read_events(Some(terminal.sequence - 1), 1).unwrap();
        assert_eq!(original.events[0].event, outcome);
        assert_eq!(original.events[0].sequence, terminal.sequence);
        let cursor = store
            .event_transcript_cursor(&terminal.event_id)
            .unwrap()
            .unwrap();
        let (native_t, unavailable): (u64, bool) = store.lock().unwrap().query_row(
            "SELECT observation_transcript,observation_unavailable FROM events WHERE sequence=?1", [terminal.sequence], |row| Ok((row.get(0)?, row.get(1)?))).unwrap();
        assert_eq!(native_t, cursor.get());
        assert!(unavailable);
        let witness = store
            .observation_event_witness(terminal.sequence, terminal.sequence)
            .unwrap()
            .unwrap();
        assert!(witness.source_unavailable);
        assert!(
            witness.transcript_cursor.is_none(),
            "failed auxiliary enrichment cannot claim native T ownership"
        );
        let batch = terminal.observation.as_ref().unwrap();
        assert!(batch.source_unavailable);
        let summary = store
            .observation_attempt_summary(&attempt, terminal.sequence)
            .unwrap();
        assert_eq!(
            summary.terminal_kind.as_deref(),
            Some(if failed {
                "attempt_failed"
            } else {
                "attempt_cancelled"
            })
        );
        let cut = store.observation_read_cut().unwrap();
        assert!(cut.read.journal >= terminal.sequence);
        assert!(cut.read.transcript >= cursor.get());
        assert_eq!(
            store.load_transcript_page(None, 32).unwrap().entries.len(),
            1
        );
        store
            .lock()
            .unwrap()
            .execute_batch("DROP TRIGGER terminal_witness_enrichment_fault")
            .unwrap();
        let next = store
            .append_canonical(&user_message("after-fault", "original next commit"))
            .unwrap();
        assert!(next.transcript_cursor.unwrap().get() > cursor.get());
    }
}

fn assert_faulted_terminal_originals(
    store: &SqliteConversationStore,
    terminal: &RuntimeEventEnvelope,
    outcome: &RuntimeEvent,
    missing_witness: bool,
) {
    let original = store.read_events(Some(terminal.sequence - 1), 1).unwrap();
    assert_eq!(&original.events[0].event, outcome);
    assert_eq!(original.events[0].event_id, terminal.event_id);
    let cursor = store
        .event_transcript_cursor(&terminal.event_id)
        .unwrap()
        .unwrap();
    let (unavailable, terminal_id): (bool,String) = store.lock().unwrap().query_row(
        "SELECT e.observation_unavailable,l.terminal_event_id FROM events e JOIN lifecycle_state l ON l.lifecycle_key=?2 WHERE e.sequence=?1",
        params![terminal.sequence,format!("attempt:{}",terminal.attempt_id.as_ref().unwrap())], |row| Ok((row.get(0)?,row.get(1)?)),
    ).unwrap();
    assert!(unavailable);
    assert_eq!(terminal_id, terminal.event_id.as_str());
    let witness = store
        .observation_event_witness(terminal.sequence, terminal.sequence)
        .unwrap();
    if missing_witness {
        assert!(witness.is_none());
        assert!(terminal.observation.is_none());
    } else {
        assert!(witness.unwrap().source_unavailable);
        assert!(terminal.observation.as_ref().unwrap().source_unavailable);
    }
    let cut = store.observation_read_cut().unwrap();
    assert_eq!(cut.read.journal, terminal.sequence);
    assert_eq!(cut.read.transcript, cursor.get());
    assert_eq!(
        store.load_transcript_page(None, 32).unwrap().entries.len(),
        1
    );
    assert!(matches!(
        store.append_event(envelope(
            store.conversation_id(),
            "derived-second-terminal",
            terminal.attempt_id.clone(),
            outcome.clone(),
        )),
        Err(ConversationStoreError::TerminalViolation(_))
    ));
    assert_eq!(store.observation_read_cut().unwrap(), cut);
}

fn assert_unfaulted_event_fallback(store: &SqliteConversationStore, after: u64) {
    store
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER bounded_event_derived_fault")
        .unwrap();
    let attempt = AttemptId::new("derived-next-owner");
    let next = store
        .append_event(envelope(
            store.conversation_id(),
            "derived-next-start",
            Some(attempt.clone()),
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt,
            },
        ))
        .unwrap();
    assert_eq!(next.sequence, after + 1);
    assert!(
        !store
            .observation_event_witness(next.sequence, next.sequence)
            .unwrap()
            .unwrap()
            .source_unavailable
    );
    assert!(!next.observation.as_ref().unwrap().source_unavailable);
    let present:bool=store.lock().unwrap().query_row(
        "SELECT observation_json IS NOT NULL AND length(observation_json)<=16384 AND observation_unavailable=0 FROM events WHERE sequence=?1",[next.sequence],|row|row.get(0),
    ).unwrap();
    assert!(present);
}

#[test]
fn t10_t14_t19_bounded_event_fallback_failure_keeps_original_terminal_and_lifecycle() {
    use crate::events::types::AttemptFailure;
    use crate::runtime::types::{CancellationReason, RuntimeError};
    for (column, condition) in [
        ("observation_json", "1=1"),
        ("observation_unavailable", "NEW.observation_unavailable=0"),
        ("observation_transcript", "1=1"),
        ("observation_witness", "1=1"),
    ] {
        for failed in [false, true] {
            let store = store();
            store.initialize(&[]).unwrap();
            let attempt = AttemptId::new("derived-original-owner");
            store
                .append_event(envelope(
                    store.conversation_id(),
                    "derived-original-start",
                    Some(attempt.clone()),
                    RuntimeEvent::AttemptStarted {
                        attempt_id: attempt.clone(),
                    },
                ))
                .unwrap();
            store.lock().unwrap().execute_batch(&format!(
                "CREATE TRIGGER bounded_event_derived_fault BEFORE UPDATE OF {column} ON events WHEN NEW.sequence=2 AND {condition} BEGIN SELECT RAISE(FAIL,'optional event enrichment failure'); END;",
            )).unwrap();
            let outcome = if failed {
                RuntimeEvent::AttemptFailed {
                    attempt_id: attempt.clone(),
                    error: AttemptFailure::Runtime {
                        error: RuntimeError::Internal {
                            message: "original derived failure".into(),
                        },
                    },
                }
            } else {
                RuntimeEvent::AttemptCancelled {
                    attempt_id: attempt.clone(),
                    reason: CancellationReason::UserRequested,
                }
            };
            let terminal = store
                .append_event(envelope(
                    store.conversation_id(),
                    "derived-original-terminal",
                    Some(attempt),
                    outcome.clone(),
                ))
                .unwrap();
            assert_faulted_terminal_originals(
                &store,
                &terminal,
                &outcome,
                column == "observation_witness",
            );
            if column == "observation_json" {
                let missing: bool = store
                    .lock()
                    .unwrap()
                    .query_row(
                        "SELECT observation_json IS NULL FROM events WHERE sequence=?1",
                        [terminal.sequence],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert!(missing);
            }
            assert_unfaulted_event_fallback(&store, terminal.sequence);
        }
    }
}

fn derived_enrichment_proposal() -> (MessageBlock, ToolCall) {
    let call = ToolCall {
        id: ToolCallId::new("derived-tool-call"),
        tool_id: ToolId::new("derived-tool"),
        name: "Original native tool".into(),
        arguments: serde_json::json!({"original":true}),
    };
    (
        MessageBlock::Assistant(AssistantMessageBlock {
            id: MessageId::new("derived-native-proposal"),
            content: vec![AssistantContentBlock::ToolCall(call.clone())],
        }),
        call,
    )
}

fn assert_derived_source_unavailable(
    store: &SqliteConversationStore,
    event: &RuntimeEventEnvelope,
) {
    assert!(
        store
            .observation_event_witness(event.sequence, event.sequence)
            .unwrap()
            .unwrap()
            .source_unavailable
    );
    let original = store.read_events(Some(event.sequence - 1), 1).unwrap();
    assert_eq!(original.events[0].event, event.event);
    assert_eq!(original.events[0].event_id, event.event_id);
    let unavailable: bool = store
        .lock()
        .unwrap()
        .query_row(
            "SELECT observation_unavailable FROM events WHERE sequence=?1",
            [event.sequence],
            |row| row.get(0),
        )
        .unwrap();
    assert!(unavailable);
}

fn assert_native_proposal_owner(
    store: &SqliteConversationStore,
    proposal: &MessageBlock,
    call: &ToolCall,
    attempt: &AttemptId,
    sequence: u64,
) {
    assert_eq!(
        store
            .load_messages(std::slice::from_ref(proposal.id()))
            .unwrap(),
        vec![proposal.clone()]
    );
    let (native_attempt,native_sequence):(String,u64)=store.lock().unwrap().query_row(
        "SELECT attempt_id,commit_sequence FROM canonical_tool_calls WHERE assistant_message_id=?1 AND call_id=?2",params![proposal.id().as_str(),call.id.as_str()],|row|Ok((row.get(0)?,row.get(1)?)),
    ).unwrap();
    assert_eq!(native_attempt, attempt.as_str());
    assert_eq!(native_sequence, sequence);
}

fn assert_unfaulted_original_tool_failure(
    store: &SqliteConversationStore,
    attempt: &AttemptId,
    call: &ToolCall,
) {
    store
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER bounded_event_derived_fault")
        .unwrap();
    let failed = store
        .append_event(envelope(
            store.conversation_id(),
            "derived-original-tool-failure",
            Some(attempt.clone()),
            RuntimeEvent::ToolExecutionFailed {
                tool_call_id: call.id.clone(),
                tool_id: call.tool_id.clone(),
                error: "original tool failure".into(),
            },
        ))
        .unwrap();
    assert!(
        !store
            .observation_event_witness(failed.sequence, failed.sequence)
            .unwrap()
            .unwrap()
            .source_unavailable
    );
    assert_eq!(
        store
            .read_events(Some(failed.sequence - 1), 1)
            .unwrap()
            .events[0]
            .event,
        failed.event
    );
}

#[test]
fn t10_t14_t19_assistant_tool_enrichment_failure_keeps_native_c_and_tool_owner() {
    for assistant_fault in [true, false] {
        let store = store();
        store.initialize(&[]).unwrap();
        let attempt = AttemptId::new("derived-tool-owner");
        store
            .append_event(envelope(
                store.conversation_id(),
                "derived-tool-owner-start",
                Some(attempt.clone()),
                RuntimeEvent::AttemptStarted {
                    attempt_id: attempt.clone(),
                },
            ))
            .unwrap();
        let (column, sequence) = if assistant_fault {
            ("observation_assistant_ledger", 2)
        } else {
            ("observation_tool_phase", 3)
        };
        store.lock().unwrap().execute_batch(&format!(
            "CREATE TRIGGER bounded_event_derived_fault BEFORE UPDATE OF {column} ON events WHEN NEW.sequence={sequence} BEGIN SELECT RAISE(FAIL,'optional tool coordinate failure'); END;",
        )).unwrap();
        let (proposal, call) = derived_enrichment_proposal();
        let (committed, receipt) = store
            .append_canonical_with_event(
                &proposal,
                envelope(
                    store.conversation_id(),
                    "derived-native-proposal-commit",
                    Some(attempt.clone()),
                    RuntimeEvent::AssistantMessageCommitted {
                        message_id: proposal.id().clone(),
                    },
                ),
            )
            .unwrap();
        assert_eq!(receipt.ledger_position, 1);
        assert_native_proposal_owner(&store, &proposal, &call, &attempt, committed.sequence);
        if assistant_fault {
            assert_derived_source_unavailable(&store, &committed);
        }
        let started = store
            .append_event(envelope(
                store.conversation_id(),
                "derived-original-tool-start",
                Some(attempt.clone()),
                RuntimeEvent::ToolExecutionStarted {
                    tool_call_id: call.id.clone(),
                    tool_id: call.tool_id.clone(),
                },
            ))
            .unwrap();
        assert!(
            started.tool_occurrence.is_some(),
            "original native occurrence remains proved"
        );
        if !assistant_fault {
            assert_derived_source_unavailable(&store, &started);
        }
        let cut = store.observation_read_cut().unwrap();
        assert_eq!(cut.ledger_through, 1);
        assert_unfaulted_original_tool_failure(&store, &attempt, &call);
        assert_eq!(
            store.observation_read_cut().unwrap().ledger_through,
            cut.ledger_through
        );
        assert_eq!(
            store.load_messages(&[proposal.id().clone()]).unwrap(),
            vec![proposal]
        );
    }
}
