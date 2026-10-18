fn owner_fixture() -> (tempfile::TempDir, SqliteConversationStore, RequestSnapshot) {
    let directory = tempfile::tempdir().unwrap();
    let store = SqliteConversationStore::open(
        ConversationId::generate(),
        &directory.path().join("owner.sqlite"),
    )
    .unwrap();
    store.initialize(&[user_message("base", "base")]).unwrap();
    let mut snapshot = test_request_snapshot(store.load_head().unwrap().revision, invocation());
    freeze_test_input(&store, &mut snapshot, &[]);
    (directory, store, snapshot)
}

fn completed_request_event(snapshot: &RequestSnapshot) -> RuntimeEvent {
    RuntimeEvent::ModelRequestCompleted {
        request_id: snapshot.request_id.clone(),
        finish_reason: ModelFinishReason::Stop,
        usage: None,
        generation: None,
    }
}

fn failed_request_event(snapshot: &RequestSnapshot) -> RuntimeEvent {
    RuntimeEvent::ModelRequestFailed {
        request_id: snapshot.request_id.clone(),
        error: crate::model::error::ModelError::timeout(
            crate::model::deadline::ModelTimeoutPhase::ResponseStart,
        ),
        usage: None,
        generation: None,
    }
}

fn outcome_envelope(
    store: &SqliteConversationStore,
    snapshot: &RequestSnapshot,
    event: RuntimeEvent,
) -> RuntimeEventEnvelope {
    let mut event = envelope(
        store.conversation_id(),
        "outcome",
        Some(snapshot.identity.attempt_id.clone()),
        event,
    );
    event.turn_id = Some(snapshot.identity.turn.clone());
    event
}

#[test]
fn request_owner_is_atomic_with_start_and_exact_replay_after_reopen() {
    let (directory, store, mut snapshot) = owner_fixture();
    assert_eq!(
        store.load_request_owner(&snapshot.request_id).unwrap(),
        None
    );
    let context = request_context_message("context", "frozen request context");
    snapshot = context_start_snapshot(
        store.load_head().unwrap().revision.next(),
        vec![crate::conversation::message_id_of(&context)],
    );
    freeze_test_input(&store, &mut snapshot, std::slice::from_ref(&context));
    store.arm_request_start_fault_script([RequestStartFaultOperation::AfterCompletionInsert]);
    assert!(
        store
            .commit_model_turn_start(std::slice::from_ref(&context), &snapshot, Utc::now())
            .is_err()
    );
    assert_eq!(
        store.load_request_owner(&snapshot.request_id).unwrap(),
        None
    );
    assert!(
        store
            .read_request_snapshots(None, 2)
            .unwrap()
            .snapshots
            .is_empty()
    );
    assert!(store.read_events(None, 2).unwrap().events.is_empty());
    assert_eq!(store.load_canonical().unwrap().len(), 1);

    let committed = store
        .commit_model_turn_start(std::slice::from_ref(&context), &snapshot, Utc::now())
        .unwrap();
    let owner = ModelRequestOwner::Agent {
        identity: snapshot.identity.clone(),
    };
    assert_eq!(
        store.load_request_owner(&snapshot.request_id).unwrap(),
        Some(owner.clone())
    );
    let conversation_id = store.conversation_id().clone();
    drop(store);
    let store =
        SqliteConversationStore::open(conversation_id, &directory.path().join("owner.sqlite"))
            .unwrap();
    assert_eq!(
        store.load_request_owner(&snapshot.request_id).unwrap(),
        Some(owner)
    );
    let replay = store
        .commit_model_turn_start(std::slice::from_ref(&context), &snapshot, Utc::now())
        .unwrap();
    assert_eq!(
        replay.disposition,
        ModelTurnStartCommitDisposition::IdempotentReplay
    );
    assert_eq!(replay.started, committed.started);
    assert_eq!(store.read_events(None, 4).unwrap().events.len(), 1);
    assert_eq!(store.load_canonical().unwrap().len(), 2);
}

#[test]
fn request_owner_lookup_uses_exact_request_index_without_hydrating_unrelated_facts() {
    let (_directory, store, snapshot) = owner_fixture();
    store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap();
    let connection = store.conn.lock().unwrap();
    let plans = connection
        .prepare(&format!("EXPLAIN QUERY PLAN {REQUEST_START_PROBE_SQL}"))
        .unwrap()
        .query_map([snapshot.request_id.as_str()], |row| {
            row.get::<_, String>(3)
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
        .join("\n");
    assert!(
        plans.contains("SEARCH events USING INDEX events_request_id_kind_idx"),
        "{plans}"
    );
    assert!(!plans.contains("SCAN events"), "{plans}");
    assert!(!plans.contains("TEMP B-TREE"), "{plans}");
    // An unrelated, undecodable envelope would fail a full-Journal scan.
    connection.execute(
        "INSERT INTO events(sequence,event_id,schema_version,conversation_id,event_json,event_timestamp) VALUES(2,'unrelated',1,?1,'{}','2026-10-18T00:00:00Z')",
        [store.conversation_id().as_str()],
    ).unwrap();
    drop(connection);
    assert_eq!(
        store.load_request_owner(&snapshot.request_id).unwrap(),
        Some(ModelRequestOwner::Agent {
            identity: snapshot.identity
        })
    );
    assert_eq!(
        store
            .load_request_owner(&RequestId::new("request:absent"))
            .unwrap(),
        None
    );
}

#[test]
fn request_owner_missing_conflicting_or_mismatched_facts_fail_closed() {
    for corruption in [
        "UPDATE events SET event_json=json_remove(event_json,'$.event.owner')",
        "UPDATE events SET event_json=json_set(event_json,'$.event.owner.identity.retry_number',7)",
        "UPDATE events SET event_json=json_set(event_json,'$.event.owner.identity.attempt_id','foreign')",
        "UPDATE events SET event_json=json_set(event_json,'$.event.owner.identity.turn','foreign')",
        "UPDATE events SET attempt_id='foreign'",
        "UPDATE events SET event_json=json_set(event_json,'$.conversation_id','conv_36524fd8-f674-7fc2-8125-06d01fee0e18')",
        "DELETE FROM events",
        "DELETE FROM request_snapshots",
    ] {
        let (_directory, store, snapshot) = owner_fixture();
        store
            .commit_model_turn_start(&[], &snapshot, Utc::now())
            .unwrap();
        store
            .conn
            .lock()
            .unwrap()
            .execute_batch(&format!(
                "PRAGMA foreign_keys=OFF; {corruption}; PRAGMA foreign_keys=ON;"
            ))
            .unwrap();
        assert!(
            store.load_request_owner(&snapshot.request_id).is_err(),
            "{corruption}"
        );
        assert!(
            store
                .commit_model_turn_start(&[], &snapshot, Utc::now())
                .is_err(),
            "{corruption}"
        );
        for event in [
            completed_request_event(&snapshot),
            failed_request_event(&snapshot),
        ] {
            assert!(
                store
                    .append_event(outcome_envelope(&store, &snapshot, event))
                    .is_err(),
                "{corruption}"
            );
        }
        assert!(
            store
                .open_publication_stream(&PublicationStreamStart {
                    stream_id: PublicationStreamId::new("stream"),
                    attempt_id: snapshot.identity.attempt_id.clone(),
                    turn_id: snapshot.identity.turn.clone(),
                    request_id: snapshot.request_id.clone(),
                    message_id: snapshot.provisional_message_id.clone(),
                })
                .is_err(),
            "{corruption}"
        );
    }
}

#[test]
fn request_owner_rejects_terminal_events_from_a_different_attempt_or_turn() {
    let (_directory, store, snapshot) = owner_fixture();
    store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap();
    for event in [
        completed_request_event(&snapshot),
        failed_request_event(&snapshot),
    ] {
        for foreign_attempt in [true, false] {
            let mut envelope = outcome_envelope(&store, &snapshot, event.clone());
            if foreign_attempt {
                envelope.attempt_id = Some(AttemptId::new("foreign"));
            } else {
                envelope.turn_id = Some(TurnId::new("foreign"));
            }
            assert!(store.append_event(envelope).is_err());
        }
    }
    assert_eq!(store.read_events(None, 4).unwrap().events.len(), 1);
    assert!(store.load_request_snapshot(&snapshot.request_id).is_ok());
    store
        .append_event(outcome_envelope(
            &store,
            &snapshot,
            completed_request_event(&snapshot),
        ))
        .unwrap();
    assert_eq!(store.read_events(None, 4).unwrap().events.len(), 2);
}

#[test]
fn request_owner_orphaned_success_or_failure_is_an_integrity_error() {
    for success in [true, false] {
        let (_directory, store, snapshot) = owner_fixture();
        store
            .commit_model_turn_start(&[], &snapshot, Utc::now())
            .unwrap();
        let outcome = if success {
            completed_request_event(&snapshot)
        } else {
            failed_request_event(&snapshot)
        };
        store
            .append_event(outcome_envelope(&store, &snapshot, outcome))
            .unwrap();
        store.conn.lock().unwrap().execute_batch(
            "PRAGMA foreign_keys=OFF; DELETE FROM events WHERE json_extract(event_json,'$.event.type')='model_request_started'; DELETE FROM request_snapshots; PRAGMA foreign_keys=ON;",
        ).unwrap();
        assert!(
            matches!(store.load_request_owner(&snapshot.request_id), Err(ConversationStoreError::InvalidReference(detail)) if detail.contains("without its request-start owner"))
        );
        assert!(
            matches!(store.commit_model_turn_start(&[], &snapshot, Utc::now()), Err(ConversationStoreError::InvalidReference(detail)) if detail.contains("Journal evidence without its request snapshot"))
        );
    }
}

#[test]
fn request_owner_rejects_duplicate_start_in_transaction_and_corrupt_duplicate_fact() {
    let (_directory, store, snapshot) = owner_fixture();
    let started = store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap()
        .started;
    {
        let mut connection = store.conn.lock().unwrap();
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        assert!(persist_event_tx(&transaction, store.conversation_id(), started.clone()).is_err());
    }
    assert_eq!(store.read_events(None, 4).unwrap().events.len(), 1);
    let mut duplicate = started;
    duplicate.sequence = 2;
    duplicate.event_id = EventId::new("duplicate");
    store.conn.lock().unwrap().execute(
        "INSERT INTO events(sequence,event_id,schema_version,conversation_id,attempt_id,turn_id,event_json,event_timestamp) VALUES(2,'duplicate',1,?1,?2,?3,?4,?5)",
        params![store.conversation_id().as_str(), snapshot.identity.attempt_id.as_str(), snapshot.identity.turn.as_str(), serde_json::to_string(&duplicate).unwrap(), duplicate.timestamp.to_rfc3339()],
    ).unwrap();
    assert!(
        matches!(store.load_request_owner(&snapshot.request_id), Err(ConversationStoreError::InvalidReference(detail)) if detail.contains("duplicate request-start facts"))
    );
    assert!(
        store
            .commit_model_turn_start(&[], &snapshot, Utc::now())
            .is_err()
    );
}

#[test]
fn request_owner_rejects_auxiliary_start_without_call_transaction() {
    let (_directory, store, snapshot) = owner_fixture();
    let mut started = store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap()
        .started;
    let RuntimeEvent::ModelRequestStarted { owner, .. } = &mut started.event else {
        unreachable!()
    };
    *owner = ModelRequestOwner::Auxiliary {
        call_id: crate::runtime::identity::AuxiliaryCallId::parse(
            "aux_36524fd8-f674-7fc2-8125-06d01fee0e18",
        )
        .unwrap(),
        ordinal: 0,
    };
    let mut connection = store.conn.lock().unwrap();
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    assert!(
        matches!(persist_event_tx(&transaction, store.conversation_id(), started), Err(ConversationStoreError::InvalidReference(detail)) if detail.contains("durable Call transaction"))
    );
}

#[test]
fn falsely_current_ownerless_or_malformed_start_schema_is_rejected_without_rewrite() {
    for corruption in [
        "UPDATE events SET event_json=json_remove(event_json,'$.event.owner')",
        "UPDATE events SET event_json=json_set(event_json,'$.event.owner',NULL)",
        "UPDATE events SET event_json=json_remove(event_json,'$.event.owner.identity.retry_number')",
        "UPDATE events SET event_json=json_set(event_json,'$.event.owner.identity.retry_number',-1)",
        "UPDATE events SET event_json=json_set(event_json,'$.event.owner.extra',1)",
    ] {
        let (directory, store, snapshot) = owner_fixture();
        store
            .commit_model_turn_start(&[], &snapshot, Utc::now())
            .unwrap();
        let conversation_id = store.conversation_id().clone();
        store
            .conn
            .lock()
            .unwrap()
            .execute_batch(&format!(
                "PRAGMA foreign_keys=OFF; {corruption}; PRAGMA foreign_keys=ON;"
            ))
            .unwrap();
        drop(store);
        let path = directory.path().join("owner.sqlite");
        let before = std::fs::read(&path).unwrap();
        assert!(
            matches!(SqliteConversationStore::open(conversation_id, &path), Err(ConversationStoreError::IncompatibleSchema(detail)) if detail.contains("explicit typed ownership")),
            "{corruption}"
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
}
