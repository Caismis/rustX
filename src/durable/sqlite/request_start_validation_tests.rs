// Fresh-start reuse must preserve the independent native Journal validator.
fn fresh_start_fixture() -> (SqliteConversationStore, RequestSnapshot, Vec<MessageBlock>) {
    let store = store();
    let messages = (0..64)
        .map(|index| user_message(&format!("one-pass-{index}"), &"水🙂".repeat(300)))
        .collect::<Vec<_>>();
    store.initialize(&messages).unwrap();
    let mut snapshot = test_request_snapshot(store.load_head().unwrap().revision, invocation());
    freeze_test_input(&store, &mut snapshot, &[]);
    (store, snapshot, messages)
}

#[test]
fn t14_t19_t20_fresh_request_start_reconstructs_original_input_once_and_replays_exactly() {
    let (store, snapshot, messages) = fresh_start_fixture();
    let expected = snapshot.reconstruct_from_canonical(&messages).unwrap();
    take_request_input_reconstructions();
    let first = store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap();
    assert_eq!(take_request_input_reconstructions(), (1, messages.len()));
    assert_eq!(
        store.load_request_snapshot(&snapshot.request_id).unwrap(),
        snapshot
    );
    assert_eq!(
        store
            .reconstruct_model_request(&snapshot.request_id)
            .unwrap(),
        expected
    );
    let journal = store.read_events(None, 32).unwrap().events;
    assert_eq!(journal.len(), 1);
    assert_eq!(journal[0].sequence, first.started.sequence);
    validate_request_start_metadata(&snapshot, &journal[0]).unwrap();
    let stored: String = store
        .lock()
        .unwrap()
        .query_row(
            "SELECT snapshot_json FROM request_snapshots WHERE request_id=?1",
            [snapshot.request_id.as_str()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stored, encode_snapshot(&snapshot).unwrap());
    take_request_input_reconstructions();
    let retry = store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap();
    assert_eq!(take_request_input_reconstructions(), (0, 0));
    assert_eq!(retry.started, first.started);
    assert_eq!(store.read_events(None, 32).unwrap().events, journal);
    let mut conflicting = snapshot.clone();
    conflicting
        .effective_system_prompt
        .push_str("different frozen input");
    assert!(
        store
            .commit_model_turn_start(&[], &conflicting, Utc::now())
            .is_err()
    );
    assert_eq!(store.read_events(None, 32).unwrap().events, journal);
    assert_eq!(store.load_canonical().unwrap(), messages);
}

#[test]
fn t14_t19_fresh_start_rejects_snapshot_and_original_changes_after_selection_atomically() {
    for action in [
        "UPDATE request_snapshots SET snapshot_json=json_set(snapshot_json,'$.effective_system_prompt','changed original R') WHERE request_id=NEW.request_id;",
        "UPDATE message_ledger SET message_json=json_set(message_json,'$.content[0].text','changed original C') WHERE position=1;",
    ] {
        let (store, snapshot, messages) = fresh_start_fixture();
        store.lock().unwrap().execute_batch(&format!(
            "CREATE TEMP TRIGGER change_fresh_native_reference AFTER INSERT ON request_snapshots BEGIN {action} END;"
        )).unwrap();
        let result = store.commit_model_turn_start(&[], &snapshot, Utc::now());
        assert!(
            matches!(result, Err(ConversationStoreError::InvalidReference(_))),
            "{result:?}"
        );
        assert_eq!(
            store.load_canonical().unwrap(),
            messages,
            "all trigger writes roll back"
        );
        assert_eq!(store.conn.scalar_facts.retained_work(), None,
            "failed native validation cannot install a prospective Source cache");
        assert_eq!(store.presentation_frontier().unwrap(), 0);
        let count: u64 = store
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM request_snapshots", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0);
        store
            .lock()
            .unwrap()
            .execute_batch("DROP TRIGGER change_fresh_native_reference")
            .unwrap();
        take_request_input_reconstructions();
        store
            .commit_model_turn_start(&[], &snapshot, Utc::now())
            .unwrap();
        assert_eq!(take_request_input_reconstructions(), (1, messages.len()));
        assert!(store.conn.scalar_facts.retained_work().is_some(),
            "the successful identical native retry can retain derived public facts");
    }
}

#[test]
fn t14_t19_generic_request_start_reference_still_reconstructs_and_rejects_tamper() {
    let (store, snapshot, messages) = fresh_start_fixture();
    let original = store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap()
        .started;
    let mut connection = store.lock().unwrap();
    let transaction = connection.transaction().unwrap();
    take_request_input_reconstructions();
    validate_event_reference(&transaction, &original).unwrap();
    assert_eq!(take_request_input_reconstructions(), (1, messages.len()));
    let mut foreign = original.clone();
    foreign.attempt_id = Some(AttemptId::new("different-native-owner"));
    assert!(validate_event_reference(&transaction, &foreign).is_err());
    assert_eq!(take_request_input_reconstructions(), (0, 0));
    transaction.execute(
        "UPDATE message_ledger SET message_json=json_set(message_json,'$.content[0].text','changed after start') WHERE position=1", [],
    ).unwrap();
    assert!(validate_event_reference(&transaction, &original).is_err());
    assert_eq!(take_request_input_reconstructions(), (1, 1));
    transaction.rollback().unwrap();
    drop(connection);
    assert_eq!(store.load_canonical().unwrap(), messages);
    assert_eq!(store.presentation_frontier().unwrap(), original.sequence);
}

#[test]
fn t14_t19_validated_fresh_start_cannot_cross_transaction_or_request_identity() {
    let (store, snapshot, _) = fresh_start_fixture();
    let event = store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap()
        .started;
    let mut first = store.lock().unwrap();
    let transaction = first.transaction().unwrap();
    reconstruct_request_start_reference(
        &transaction,
        &event,
        Some(&encode_snapshot(&snapshot).unwrap()),
    )
    .unwrap();
    let foreign_store = super::store();
    foreign_store.initialize(&[]).unwrap();
    let mut second = foreign_store.lock().unwrap();
    let foreign_transaction = second.transaction().unwrap();
    let mut foreign = event.clone();
    foreign.conversation_id = foreign_store.conversation_id().clone();
    assert!(matches!(
        persist_event_tx_internal(
            &foreign_transaction,
            foreign_store.conversation_id(),
            foreign,
            false,
            Some(ValidatedRequestStart {
                transaction: &transaction,
                snapshot: &snapshot
            }),
        ),
        Err(ConversationStoreError::InvalidReference(message))
            if message == "fresh request validation belongs to a different transaction"
    ));
    let mut different_request = event;
    different_request.event = RuntimeEvent::ModelRequestStarted {
        request_id: RequestId::new("unrelated-request"),
        model: snapshot.invocation.model.clone(),
    };
    assert!(
        persist_event_tx_internal(
            &transaction,
            store.conversation_id(),
            different_request,
            false,
            Some(ValidatedRequestStart {
                transaction: &transaction,
                snapshot: &snapshot
            }),
        )
        .is_err()
    );
    assert_eq!(
        foreign_transaction
            .query_row("SELECT count(*) FROM events", [], |row| row
                .get::<_, u64>(0))
            .unwrap(),
        0
    );
}
