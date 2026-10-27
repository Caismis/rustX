// Semantic probes read original selected J, independently of optional layouts.
fn seeded_probe_request(
    store: &SqliteConversationStore,
) -> (RequestSnapshot, RuntimeEventEnvelope, RuntimeEventEnvelope) {
    store.initialize(&[]).unwrap();
    let snapshot = test_request_snapshot(store.load_head().unwrap().revision, invocation());
    let mut started = store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap()
        .started;
    started.observation = None;
    let mut completed = envelope(
        store.conversation_id(),
        "selected-request-outcome",
        Some(snapshot.identity.attempt_id.clone()),
        RuntimeEvent::ModelRequestCompleted {
            request_id: snapshot.request_id.clone(),
            finish_reason: ModelFinishReason::Stop,
            usage: None,
            generation: None,
        },
    );
    completed.turn_id = Some(snapshot.identity.turn.clone());
    let mut completed = store.append_event(completed).unwrap();
    completed.observation = None;
    (snapshot, started, completed)
}

fn probe_sql_work(connection: &Connection, sql: &str, request: &RequestId) -> (usize, i32) {
    let plan = connection
        .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
        .unwrap()
        .query_map([request.as_str()], |row| row.get::<_, String>(3))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
        .join("\n");
    assert!(
        plan.contains("SEARCH events USING INDEX events_request_id_kind_idx"),
        "{plan}"
    );
    assert!(!plan.contains("SCAN events"), "{plan}");
    let mut statement = connection.prepare(sql).unwrap();
    let count = statement
        .query_map([request.as_str()], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
        .len();
    (
        count,
        statement.get_status(rusqlite::StatementStatus::VmStep),
    )
}

fn assert_original_probe_event(
    actual: Option<RuntimeEventEnvelope>,
    expected: &RuntimeEventEnvelope,
) {
    assert_eq!(
        serde_json::to_value(actual.expect("original selected Journal fact")).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
}

#[test]
fn t14_t19_t20_native_request_probes_skip_unrelated_giant_original_journal_rows() {
    let store = store();
    let (snapshot, started, completed) = seeded_probe_request(&store);
    let baseline = {
        let connection = store.lock().unwrap();
        [REQUEST_START_PROBE_SQL, REQUEST_OUTCOME_PROBE_SQL]
            .map(|sql| probe_sql_work(&connection, sql, &snapshot.request_id))
    };
    let giant = "unrelated-original-event".repeat(48_000);
    assert!(giant.len() > 1_048_576);
    for ordinal in 0..16 {
        let attempt = AttemptId::new(format!("unrelated-probe-attempt-{ordinal}"));
        store
            .append_event(envelope(
                store.conversation_id(),
                &format!("{ordinal}-{giant}"),
                Some(attempt.clone()),
                RuntimeEvent::AttemptStarted {
                    attempt_id: attempt,
                },
            ))
            .unwrap();
    }
    let cut = store.observation_read_cut().unwrap();
    let mut connection = store.lock().unwrap();
    for (index, sql) in [REQUEST_START_PROBE_SQL, REQUEST_OUTCOME_PROBE_SQL]
        .into_iter()
        .enumerate()
    {
        assert_eq!(
            probe_sql_work(&connection, sql, &snapshot.request_id),
            baseline[index]
        );
    }
    let transaction = connection.transaction().unwrap();
    REQUEST_PROBE_WORK.with(|work| work.set((0, 0)));
    assert_original_probe_event(
        find_request_start_event(&transaction, &snapshot.request_id).unwrap(),
        &started,
    );
    assert_original_probe_event(
        find_request_outcome_event(&transaction, &snapshot.request_id).unwrap(),
        &completed,
    );
    let (rows, bytes) = REQUEST_PROBE_WORK.with(std::cell::Cell::get);
    assert_eq!(rows, 2);
    assert!(
        bytes < 4096,
        "only original selected envelopes were loaded: {bytes}"
    );
    assert!(
        find_request_start_event(&transaction, &RequestId::new("absent-request"))
            .unwrap()
            .is_none()
    );
    assert!(
        find_request_outcome_event(&transaction, &RequestId::new("absent-request"))
            .unwrap()
            .is_none()
    );
    assert_eq!(REQUEST_PROBE_WORK.with(std::cell::Cell::get), (rows, bytes));
    drop(transaction);
    drop(connection);
    assert_eq!(store.observation_read_cut().unwrap(), cut);
}

// Deliberate native Journal corruption bypasses semantic insertion to test its
// validation boundary; no derived Source header participates in these probes.
fn insert_corrupt_probe_fact(connection: &Connection, event: &RuntimeEventEnvelope) {
    connection.execute(
        "INSERT INTO events(sequence,event_id,schema_version,conversation_id,attempt_id,turn_id,event_json,event_timestamp) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
        params![event.sequence,event.event_id.as_str(),event.schema_version,event.conversation_id.as_str(),event.attempt_id.as_ref().map(AttemptId::as_str),event.turn_id.as_ref().map(TurnId::as_str),serde_json::to_string(event).unwrap(),event.timestamp.to_rfc3339()],
    ).unwrap();
}

#[test]
fn t14_t19_native_request_probe_rejects_duplicate_original_starts_and_owner_mismatch() {
    let store = store();
    let (snapshot, started, completed) = seeded_probe_request(&store);
    let mut connection = store.lock().unwrap();
    let transaction = connection.transaction().unwrap();
    let mut wrong_owner = started.clone();
    wrong_owner.turn_id = Some(TurnId::new("foreign-selected-step"));
    transaction
        .execute(
            "UPDATE events SET event_json=?1 WHERE sequence=?2",
            params![
                serde_json::to_string(&wrong_owner).unwrap(),
                started.sequence
            ],
        )
        .unwrap();
    assert!(matches!(
        require_started_request_tx(&transaction, store.conversation_id(), &snapshot.request_id),
        Err(ConversationStoreError::InvalidReference(_))
    ));
    transaction
        .execute(
            "UPDATE events SET event_json=?1 WHERE sequence=?2",
            params![serde_json::to_string(&started).unwrap(), started.sequence],
        )
        .unwrap();
    let (metadata, actual_start, actual_completion) =
        require_started_request_tx(&transaction, store.conversation_id(), &snapshot.request_id)
            .unwrap();
    assert_eq!(*metadata.0, snapshot);
    assert_eq!(
        (actual_start, actual_completion),
        (started.sequence, Some(completed.sequence))
    );
    let mut duplicate = started;
    duplicate.sequence = 3;
    duplicate.event_id = EventId::new("corrupt-duplicate-request-start");
    insert_corrupt_probe_fact(&transaction, &duplicate);
    REQUEST_PROBE_WORK.with(|work| work.set((0, 0)));
    assert!(
        matches!(find_request_start_event(&transaction, &snapshot.request_id), Err(ConversationStoreError::InvalidReference(message)) if message.contains("duplicate request-start"))
    );
    assert_eq!(REQUEST_PROBE_WORK.with(std::cell::Cell::get).0, 2);
}

#[test]
fn t14_t19_native_request_outcome_probe_keeps_earliest_original_success_or_failure() {
    let store = store();
    let (snapshot, _, completed) = seeded_probe_request(&store);
    let mut connection = store.lock().unwrap();
    let transaction = connection.transaction().unwrap();
    let mut failed = completed.clone();
    failed.sequence = 3;
    failed.event_id = EventId::new("corrupt-later-request-failure");
    failed.event = RuntimeEvent::ModelRequestFailed {
        request_id: snapshot.request_id.clone(),
        error: crate::model::error::ModelError::timeout(
            crate::model::ModelTimeoutPhase::ResponseStart,
        ),
        usage: None,
        generation: None,
    };
    insert_corrupt_probe_fact(&transaction, &failed);
    assert_original_probe_event(
        find_request_outcome_event(&transaction, &snapshot.request_id).unwrap(),
        &completed,
    );
    // Reversing which original kind occupies each actual sequence must retain
    // native order, rather than preferring success or the first query branch.
    failed.sequence = completed.sequence;
    failed.event_id = completed.event_id.clone();
    let mut later_success = completed;
    later_success.sequence = 3;
    later_success.event_id = EventId::new("corrupt-later-request-failure");
    for event in [&failed, &later_success] {
        transaction
            .execute(
                "UPDATE events SET event_json=?1 WHERE sequence=?2",
                params![serde_json::to_string(event).unwrap(), event.sequence],
            )
            .unwrap();
    }
    REQUEST_PROBE_WORK.with(|work| work.set((0, 0)));
    assert_original_probe_event(
        find_request_outcome_event(&transaction, &snapshot.request_id).unwrap(),
        &failed,
    );
    assert_eq!(REQUEST_PROBE_WORK.with(std::cell::Cell::get).0, 1);
}
