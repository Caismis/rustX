// Independent pre-reuse call order: each started-request proof loads its own R.
fn original_started_request(
    transaction: &Transaction<'_>,
    conversation: &ConversationId,
    request: &RequestId,
) -> Result<(StoredRequestMetadata, u64), ConversationStoreError> {
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

fn take_configuration_decodes() -> usize {
    native_request::CONFIGURATION_DECODES.with(|count| count.replace(0))
}

fn assert_snapshot_and_configuration_work(metadata_bytes: usize) {
    // The byte counter covers only the normalized metadata row. Shared payload
    // bytes are still fetched and fully typed-decoded; assert that work once too.
    assert_eq!(take_snapshot_work(), (1, metadata_bytes));
    assert_eq!(take_configuration_decodes(), 1);
}

#[test]
fn t14_t19_t20_provider_outcomes_decode_the_same_transaction_snapshot_once() {
    let (store, snapshot, mut event) = snapshot_fixture();
    let mut connection = store.lock().unwrap();
    let transaction = connection.transaction().unwrap();
    let bytes: usize = transaction
        .query_row(
            "SELECT octet_length(snapshot_json) FROM request_snapshots WHERE request_id=?1",
            [snapshot.request_id.as_str()],
            |row| row.get(0),
        )
        .unwrap();
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
        take_configuration_decodes();
        original_provider_validation(&transaction, &event).unwrap();
        assert_eq!(take_snapshot_work(), (2, 2 * bytes));
        assert_eq!(take_configuration_decodes(), 2);
        validate_event_reference(&transaction, &event).unwrap();
        assert_snapshot_and_configuration_work(bytes);
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
        let bytes: usize = store
            .lock()
            .unwrap()
            .query_row(
                "SELECT octet_length(snapshot_json) FROM request_snapshots WHERE request_id=?1",
                [snapshot.request_id.as_str()],
                |row| row.get(0),
            )
            .unwrap();
        let start = publication_start(&snapshot);
        take_snapshot_work();
        take_configuration_decodes();

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
            store.commit_publication_terminal(&start, std::slice::from_ref(&frame)),
            Err(ConversationStoreError::PublicationViolation(message)) if message.contains("no durable provider outcome")
        ));
        // First-write admission and the terminal boundary each recheck the
        // small row; only one exact configuration payload is decoded.
        assert_eq!(take_snapshot_work(), (2, 2 * bytes));
        assert_eq!(take_configuration_decodes(), 1);
        assert_eq!(store.presentation_frontier().unwrap(), before);
        store.append_event(outcome).unwrap();
        assert_snapshot_and_configuration_work(bytes);
        store
            .commit_publication_terminal(&start, &[frame])
            .unwrap();
        assert_eq!(take_snapshot_work(), (2, 2 * bytes));
        assert_eq!(take_configuration_decodes(), 1);
        if audit {
            store
                .terminalize_publication_audit(&start, Utc::now())
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
        assert_snapshot_and_configuration_work(bytes);
        assert_eq!(
            store.load_request_snapshot(&snapshot.request_id).unwrap(),
            snapshot
        );
    }
}

fn selected_snapshot_fixture() -> (SqliteConversationStore, RequestSnapshot) {
    let store = store();
    let messages: Vec<_> = (0..8)
        .map(|index| user_message(&format!("selected-{index}"), &"original 水".repeat(4096)))
        .collect();
    store.initialize(&messages).unwrap();
    let mut snapshot = test_request_snapshot(store.load_head().unwrap().revision, invocation());
    freeze_test_input(&store, &mut snapshot, &[]);
    store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap();
    (store, snapshot)
}

fn deny_selected_input_bodies(
    context: rusqlite::hooks::AuthContext<'_>,
) -> rusqlite::hooks::Authorization {
    use rusqlite::hooks::{AuthAction, Authorization};
    match context.action {
        AuthAction::Read {
            table_name: "native_input_nodes",
            column_name: "item_json",
        } => Authorization::Deny,
        _ => Authorization::Allow,
    }
}

#[test]
fn normalized_request_metadata_publication_never_hydrates_selected_input_bodies() {
    let (store, snapshot) = selected_snapshot_fixture();
    let encoded = encode_snapshot(&snapshot).unwrap();
    let normalized: RequestSnapshot = decode(&encoded, "normalized test request").unwrap();
    assert!(normalized.input.is_empty());
    assert!(encoded.len() < serde_json::to_vec(&snapshot).unwrap().len());
    {
        let mut connection = store.lock().unwrap();
        connection
            .authorizer(Some(deny_selected_input_bodies))
            .unwrap();
        let transaction = connection.transaction().unwrap();
        let (metadata, _, _) =
            require_started_request_tx(&transaction, store.conversation_id(), &snapshot.request_id)
                .unwrap();
        assert!(metadata.input.is_empty());
        assert_eq!(metadata.identity, snapshot.identity);
        assert_eq!(
            read_stored_snapshot_root(&transaction, &snapshot.request_id)
                .unwrap()
                .unwrap()
                .count,
            8
        );
    }
    store
        .stage_publication_frames(&publication_start(&snapshot), &[PublicationFrame {
            stream_id: publication_start(&snapshot).stream_id,
            message_id: snapshot.provisional_message_id.clone(),
            sequence: 0,
            payload: crate::publication::PublicationPayload::TextSuffix { block_index: ContentBlockIndex::new(0), suffix: "output".into() },
        }])
        .unwrap();
    assert!(store.load_request_snapshot(&snapshot.request_id).is_err());
    store
        .lock()
        .unwrap()
        .authorizer(None::<fn(rusqlite::hooks::AuthContext<'_>) -> rusqlite::hooks::Authorization>)
        .unwrap();
    assert_eq!(
        store.load_request_snapshot(&snapshot.request_id).unwrap(),
        snapshot
    );
}

#[test]
fn normalized_request_resource_budget_charges_metadata_and_the_exact_flat_input_array() {
    let (store, snapshot) = selected_snapshot_fixture();
    let bytes = serde_json::to_vec(&snapshot).unwrap().len();
    let normalized = encode_snapshot(&snapshot).unwrap();
    assert_eq!(
        bytes,
        normalized.len() - 2 + serde_json::to_vec(&snapshot.input).unwrap().len()
    );
    let cut = store.compaction_resource_cut().unwrap();
    let mut limits = crate::context::resources::ResourceLimits {
        record_bytes: bytes - 1,
        page_bytes: bytes - 1,
        ..crate::context::resources::ResourceLimits::default()
    };
    assert!(
        store
            .compaction_request(&cut, &snapshot.request_id, limits)
            .is_err()
    );
    limits.record_bytes = bytes;
    limits.page_bytes = bytes;
    assert_eq!(
        store
            .compaction_request(&cut, &snapshot.request_id, limits)
            .unwrap(),
        snapshot
    );
}

#[test]
fn normalized_request_metadata_budget_rejects_before_returning_any_owned_json() {
    let (store, snapshot) = selected_snapshot_fixture();
    let metadata_bytes = encode_snapshot(&snapshot).unwrap().len();
    let full_bytes = serde_json::to_vec(&snapshot).unwrap().len();
    let connection = store.lock().unwrap();
    let stored_bytes: usize = connection
        .query_row(
            "SELECT octet_length(snapshot_json) FROM request_snapshots WHERE request_id=?1",
            [snapshot.request_id.as_str()],
            |row| row.get(0),
        )
        .unwrap();
    take_snapshot_work();
    assert!(
        load_request_snapshot_bounded(
            &connection,
            store.conversation_id(),
            &snapshot.request_id,
            metadata_bytes - 1,
        )
        .is_err()
    );
    assert_eq!(take_snapshot_work(), (1, 0));
    // Admission at the exact metadata limit returns that String, then refuses
    // hydration because the full original input cannot fit this smaller budget.
    assert!(
        load_request_snapshot_bounded(
            &connection,
            store.conversation_id(),
            &snapshot.request_id,
            metadata_bytes,
        )
        .is_err()
    );
    assert_eq!(take_snapshot_work(), (1, stored_bytes));
    assert_eq!(
        load_request_snapshot_bounded(
            &connection,
            store.conversation_id(),
            &snapshot.request_id,
            full_bytes,
        )
        .unwrap(),
        snapshot
    );
    assert_eq!(take_snapshot_work(), (1, stored_bytes));
}

fn assert_manifest_digest_rejected<T>(result: &Result<T, ConversationStoreError>) {
    assert!(matches!(
        result,
        Err(ConversationStoreError::InvalidReference(_))
    ));
    assert_eq!(
        MANIFEST_DIGEST_READ_WORK.with(|work| work.replace((0, 0))),
        (1, 0)
    );
}

#[test]
fn normalized_request_digest_guards_every_reader_before_materializing_malformed_cells() {
    use rusqlite::types::Value;
    let (store, snapshot) = selected_snapshot_fixture();
    let before = store.presentation_frontier().unwrap();
    let (digest, started) = {
        let mut connection = store.lock().unwrap();
        let transaction = connection.transaction().unwrap();
        let json: String = transaction
            .query_row(
                "SELECT snapshot_json FROM request_snapshots WHERE request_id=?1",
                [snapshot.request_id.as_str()],
                |row| row.get(0),
            )
            .unwrap();
        (
            stored_snapshot_digest(
                &json,
                read_stored_snapshot_root(&transaction, &snapshot.request_id).unwrap(),
            ),
            find_request_start_event(&transaction, &snapshot.request_id)
                .unwrap()
                .unwrap(),
        )
    };
    for invalid in [
        Value::Text("x".repeat(131_072)),
        Value::Text("x".repeat(63)),
        Value::Blob(vec![b'x'; 64]),
    ] {
        {
            let mut connection = store.lock().unwrap();
            // Model a corrupted persisted cell while keeping schema admission
            // enabled for every actual native reader and retry below.
            connection
                .execute_batch("PRAGMA ignore_check_constraints=ON")
                .unwrap();
            connection
                .execute(
                    "UPDATE request_snapshots SET manifest_digest=?2 WHERE request_id=?1",
                    params![snapshot.request_id.as_str(), invalid],
                )
                .unwrap();
            connection
                .execute_batch("PRAGMA ignore_check_constraints=OFF")
                .unwrap();
            MANIFEST_DIGEST_READ_WORK.with(|work| work.set((0, 0)));
            let transaction = connection.transaction().unwrap();
            assert_manifest_digest_rejected(&require_started_request_tx(
                &transaction,
                store.conversation_id(),
                &snapshot.request_id,
            ));
            assert_manifest_digest_rejected(&reconstruct_request_start_reference(
                &transaction,
                &started,
                None,
            ));
        }
        assert_manifest_digest_rejected(&store.load_request_snapshot(&snapshot.request_id));
        assert_manifest_digest_rejected(&store.commit_model_turn_start(&[], &snapshot, Utc::now()));
        assert_eq!(store.presentation_frontier().unwrap(), before);
    }
    store
        .lock()
        .unwrap()
        .execute(
            "UPDATE request_snapshots SET manifest_digest=?2 WHERE request_id=?1",
            params![snapshot.request_id.as_str(), digest],
        )
        .unwrap();
    assert_eq!(
        store.load_request_snapshot(&snapshot.request_id).unwrap(),
        snapshot
    );
    assert_eq!(
        MANIFEST_DIGEST_READ_WORK.with(|work| work.replace((0, 0))),
        (1, 64)
    );
    store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap();
    assert_eq!(store.presentation_frontier().unwrap(), before);
}

#[test]
fn archived_normalized_request_preserves_the_exact_original_flat_snapshot() {
    let (store, snapshot) = selected_snapshot_fixture();
    store.archive_barrier().unwrap();
    let through = store.archive_frontiers().unwrap().0.requests;
    let (ordinal, json) = store
        .archive_record(archive::Authority::Requests, 0, through)
        .unwrap()
        .unwrap();
    assert_eq!(json, serde_json::to_string(&snapshot).unwrap());
    let restored: RequestSnapshot = decode(&json, "archived original snapshot").unwrap();
    assert_eq!(restored.input.len(), 8);
    assert_eq!(restored, snapshot);
    assert!(
        store
            .archive_record(archive::Authority::Requests, ordinal, through)
            .unwrap()
            .is_none()
    );
    store.archive_release().unwrap();
}
