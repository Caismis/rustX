// Independent pre-reuse call order: each started-request proof loads its own R.
fn original_started_request(
    transaction: &Transaction<'_>,
    conversation: &ConversationId,
    request: &RequestId,
) -> Result<(RequestSnapshot, u64), ConversationStoreError> {
    let Some((snapshot, sequence, _)) = read_request_snapshot_tx(transaction, request)? else {
        return Err(ConversationStoreError::InvalidReference(format!(
            "request snapshot {request} does not exist"
        )));
    };
    let Some(sequence) = sequence else {
        return Err(ConversationStoreError::InvalidReference(format!(
            "request snapshot {request} has no durable start sequence"
        )));
    };
    let Some(started) = find_request_start_event(transaction, request)? else {
        return Err(ConversationStoreError::InvalidReference(format!(
            "request snapshot {request} has no durable request-start fact"
        )));
    };
    if started.sequence != sequence {
        return Err(ConversationStoreError::InvalidReference(format!(
            "request snapshot {request} start sequence disagrees with its request-start fact"
        )));
    }
    if started.conversation_id != *conversation {
        return Err(ConversationStoreError::InvalidReference(format!(
            "request-start fact for {request} belongs to a foreign conversation"
        )));
    }
    validate_request_start_metadata(&snapshot, &started)?;
    Ok((snapshot, sequence))
}

fn original_provider_validation(
    transaction: &Transaction<'_>,
    event: &RuntimeEventEnvelope,
) -> Result<(), ConversationStoreError> {
    let (RuntimeEvent::ModelRequestCompleted {
        request_id: request,
        ..
    }
    | RuntimeEvent::ModelRequestFailed {
        request_id: request,
        ..
    }) = &event.event
    else {
        panic!("provider outcome fixture");
    };
    let Some((snapshot, _, _)) = read_request_snapshot_tx(transaction, request)? else {
        return Err(ConversationStoreError::InvalidReference(format!(
            "model request outcome references request {request}, which never started"
        )));
    };
    original_started_request(transaction, &event.conversation_id, request)?;
    if event.attempt_id.as_ref() != Some(&snapshot.identity.attempt_id)
        || event.turn_id.as_ref() != Some(&snapshot.identity.turn)
    {
        return Err(ConversationStoreError::InvalidReference(format!(
            "model request outcome for {request} has a foreign attempt or turn envelope"
        )));
    }
    if find_request_outcome_event(transaction, request)?.is_some() {
        return Err(ConversationStoreError::TerminalViolation(format!(
            "request {request} already has a durable provider outcome"
        )));
    }
    Ok(())
}

fn snapshot_fixture() -> (
    SqliteConversationStore,
    RequestSnapshot,
    RuntimeEventEnvelope,
) {
    let store = store();
    store.initialize(&[]).unwrap();
    let mut snapshot = test_request_snapshot(store.load_head().unwrap().revision, invocation());
    snapshot.effective_system_prompt = "original frozen 水🙂 input".repeat(4096);
    freeze_test_input(&store, &mut snapshot, &[]);
    store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap();
    let mut event = envelope(
        store.conversation_id(),
        "provider-outcome-reuse",
        Some(snapshot.identity.attempt_id.clone()),
        RuntimeEvent::ModelRequestCompleted {
            request_id: snapshot.request_id.clone(),
            finish_reason: ModelFinishReason::Stop,
            usage: None,
            generation: None,
        },
    );
    event.turn_id = Some(snapshot.identity.turn.clone());
    (store, snapshot, event)
}

fn take_snapshot_work() -> (usize, usize) {
    REQUEST_SNAPSHOT_READ_WORK.with(|work| work.replace((0, 0)))
}

#[test]
fn t14_t19_t20_provider_outcomes_decode_the_same_transaction_snapshot_once() {
    let (store, snapshot, mut event) = snapshot_fixture();
    let mut connection = store.lock().unwrap();
    let transaction = connection.transaction().unwrap();
    let bytes = encode_snapshot(&snapshot).unwrap().len();
    for failed in [false, true] {
        if failed {
            event.event = RuntimeEvent::ModelRequestFailed {
                request_id: snapshot.request_id.clone(),
                error: crate::model::error::ModelError::timeout(
                    crate::model::ModelTimeoutPhase::ResponseStart,
                ),
                usage: None,
                generation: None,
            };
        }
        take_snapshot_work();
        original_provider_validation(&transaction, &event).unwrap();
        assert_eq!(take_snapshot_work(), (2, 2 * bytes));
        validate_event_reference(&transaction, &event).unwrap();
        assert_eq!(take_snapshot_work(), (1, bytes));
    }
}

#[test]
fn t14_t19_snapshot_reuse_preserves_original_provider_validation_errors_and_precedence() {
    for corruption in [
        "missing request",
        "foreign attempt",
        "foreign turn",
        "duplicate outcome",
        "UPDATE request_snapshots SET started_sequence=NULL",
        "UPDATE events SET event_json=json_set(event_json,'$.sequence',sequence+1)",
        "UPDATE events SET event_json=json_set(event_json,'$.event.type','attempt_started')",
        "INSERT INTO events(sequence,event_id,schema_version,conversation_id,attempt_id,turn_id,event_json,event_timestamp) SELECT sequence+100,event_id||'-duplicate',schema_version,conversation_id,attempt_id,turn_id,json_set(event_json,'$.sequence',sequence+100,'$.event_id',event_id||'-duplicate'),event_timestamp FROM events WHERE sequence=1",
        "UPDATE events SET event_json=json_set(event_json,'$.conversation_id','foreign')",
        "UPDATE events SET event_json=json_set(event_json,'$.turn_id','foreign')",
        "UPDATE request_snapshots SET canonical_frontier=canonical_frontier+1",
        "UPDATE request_snapshots SET snapshot_json=json_set(snapshot_json,'$.effective_system_prompt','tampered')",
    ] {
        let (store, _, mut event) = snapshot_fixture();
        if corruption == "duplicate outcome" {
            store.append_event(event.clone()).unwrap();
            event.event_id = EventId::new("second-provider-outcome");
        }
        let mut connection = store.lock().unwrap();
        let transaction = connection.transaction().unwrap();
        match corruption {
            "missing request" => {
                let RuntimeEvent::ModelRequestCompleted { request_id, .. } = &mut event.event
                else {
                    unreachable!()
                };
                *request_id = RequestId::new("request-never-started");
            }
            "foreign attempt" => event.attempt_id = Some(AttemptId::new("foreign")),
            "foreign turn" => event.turn_id = Some(TurnId::new("foreign")),
            "duplicate outcome" => {}
            sql => transaction.execute_batch(sql).unwrap(),
        }
        let expected = original_provider_validation(&transaction, &event).unwrap_err();
        let actual = validate_event_reference(&transaction, &event).unwrap_err();
        assert_eq!(
            format!("{actual:?}"),
            format!("{expected:?}"),
            "{corruption}"
        );
    }
}

fn publication_start(snapshot: &RequestSnapshot) -> PublicationStreamStart {
    PublicationStreamStart {
        stream_id: PublicationStreamId::for_request(
            &snapshot.identity.attempt_id,
            &snapshot.provisional_message_id,
        ),
        attempt_id: snapshot.identity.attempt_id.clone(),
        turn_id: snapshot.identity.turn.clone(),
        request_id: snapshot.request_id.clone(),
        message_id: snapshot.provisional_message_id.clone(),
    }
}

#[test]
fn t14_t19_t20_publication_p_u_c_and_audit_each_reuse_one_validated_snapshot() {
    for audit in [false, true] {
        let (store, snapshot, outcome) = snapshot_fixture();
        let bytes = encode_snapshot(&snapshot).unwrap().len();
        let start = publication_start(&snapshot);
        take_snapshot_work();
        store.open_publication_stream(&start).unwrap();
        assert_eq!(take_snapshot_work(), (1, bytes));
        let frame = PublicationFrame {
            stream_id: start.stream_id.clone(),
            message_id: start.message_id.clone(),
            sequence: 0,
            payload: crate::publication::PublicationPayload::TextSuffix {
                block_index: ContentBlockIndex::new(0),
                suffix: "original released output".into(),
            },
        };
        let before = store.presentation_frontier().unwrap();
        assert!(matches!(
            store.commit_publication_terminal(&start.stream_id, std::slice::from_ref(&frame)),
            Err(ConversationStoreError::PublicationViolation(message)) if message.contains("no durable provider outcome")
        ));
        assert_eq!(take_snapshot_work(), (1, bytes));
        assert_eq!(store.presentation_frontier().unwrap(), before);
        store.append_event(outcome).unwrap();
        assert_eq!(take_snapshot_work(), (1, bytes));
        store
            .commit_publication_terminal(&start.stream_id, &[frame])
            .unwrap();
        assert_eq!(take_snapshot_work(), (1, bytes));
        if audit {
            store
                .terminalize_publication_audit(&start.stream_id, Utc::now())
                .unwrap();
        } else {
            let message = MessageBlock::Assistant(AssistantMessageBlock {
                id: start.message_id.clone(),
                content: vec![AssistantContentBlock::Text(TextBlock {
                    text: "original released output".into(),
                })],
            });
            let mut committed = envelope(
                store.conversation_id(),
                "canonical-reuse",
                Some(start.attempt_id.clone()),
                RuntimeEvent::AssistantMessageCommitted {
                    message_id: start.message_id.clone(),
                },
            );
            committed.turn_id = Some(start.turn_id.clone());
            store
                .commit_canonical_publication(&start.stream_id, &message, committed)
                .unwrap();
            assert_eq!(store.load_canonical().unwrap(), vec![message]);
        }
        assert_eq!(take_snapshot_work(), (1, bytes));
        assert_eq!(
            store.load_request_snapshot(&snapshot.request_id).unwrap(),
            snapshot
        );
    }
}
