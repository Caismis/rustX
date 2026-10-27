#[test]
fn append_facts_materialize_each_existing_atomic_group_once() {
    use super::observation_response_tests::{response_complete, response_message, response_start};
    let store = store();
    store.initialize(&[]).unwrap();
    store.lock().unwrap().execute_batch(
        "CREATE TEMP TABLE append_writes(sequence INTEGER,kind TEXT);
         CREATE TEMP TRIGGER count_aggregate_write AFTER UPDATE OF observation_trace_total,observation_statistics,observation_members ON events BEGIN INSERT INTO append_writes VALUES(NEW.sequence,'aggregate'); END;
         CREATE TEMP TRIGGER count_source_write AFTER UPDATE OF observation_source_overlay,observation_witness ON events BEGIN INSERT INTO append_writes VALUES(NEW.sequence,'source'); END;"
    ).unwrap();
    let attempt = response_start(&store, "one-materialization");
    let connection = store.lock().unwrap();
    let counts: (u64, u64) = connection.query_row(
        "SELECT (SELECT count(*) FROM append_writes WHERE sequence=1 AND kind='aggregate'),(SELECT count(*) FROM append_writes WHERE sequence=1 AND kind='source')",
        [], |row| Ok((row.get(0)?, row.get(1)?)),
    ).unwrap();
    assert_eq!(counts, (1, 1));
    drop(connection);
    response_message(&store, &attempt, "one-materialization-message");
    response_complete(&store, &attempt);
    let connection = store.lock().unwrap();
    let aggregates: u64 = connection.query_row("SELECT count(*) FROM append_writes WHERE kind='aggregate'", [], |row| row.get(0)).unwrap();
    assert_eq!(aggregates, 3);
    drop(connection);
    assert_eq!(store.observation_statistics(3).unwrap().completed_responses, 1);
}

#[test]
fn append_facts_reuse_observes_changed_and_corrupt_stored_cells() {
    let store = store();
    store.initialize(&[]).unwrap();
    let attempt = AttemptId::new("authenticated-append-facts");
    let mut connection = store.lock().unwrap();
    let transaction = connection.transaction().unwrap();
    let persisted = persist_event_tx(&transaction, store.conversation_id(), envelope(
        store.conversation_id(), "authenticated-append-start", Some(attempt.clone()),
        RuntimeEvent::AttemptStarted { attempt_id: attempt },
    )).unwrap();
    let sequence = persisted.event.sequence;
    let capture = persisted.facts.witness.as_ref().unwrap();
    let read = || observation_event::witness_prepared(&transaction, sequence, sequence, Some(capture));
    assert_eq!(serde_json::to_vec(&read().unwrap()).unwrap(), serde_json::to_vec(&observation_event::witness(&transaction, sequence, sequence).unwrap()).unwrap());
    transaction.execute("UPDATE events SET observation_witness=CAST(json_set(observation_witness,'$.event_kind','changed-stored-value') AS BLOB) WHERE sequence=?1", [sequence]).unwrap();
    assert_eq!(read().unwrap().unwrap().event_kind, "changed-stored-value");
    assert_eq!(serde_json::to_vec(&read().unwrap()).unwrap(), serde_json::to_vec(&observation_event::witness(&transaction, sequence, sequence).unwrap()).unwrap());
    transaction.execute("UPDATE events SET observation_witness=?2 WHERE sequence=?1", params![sequence, b"[".as_slice()]).unwrap();
    assert!(read().is_err());
    assert!(observation_event::witness(&transaction, sequence, sequence).is_err());

    let execution = persisted.facts.execution.as_ref().unwrap();
    let ordinary = observation_execution::summary_at_sequence(&transaction, sequence).unwrap().unwrap();
    assert_eq!(observation_execution::summary_at_sequence_prepared(&transaction, sequence, Some(execution)).unwrap(), Some(ordinary));
    let mut value: serde_json::Value = serde_json::from_slice(&execution.bytes).unwrap();
    value[3][6] = serde_json::json!(7);
    transaction.execute("UPDATE events SET observation_execution=?2 WHERE sequence=?1", params![sequence, serde_json::to_vec(&value).unwrap()]).unwrap();
    let changed = observation_execution::summary_at_sequence_prepared(&transaction, sequence, Some(execution)).unwrap();
    assert_eq!(changed.as_ref().unwrap().requests, 7);
    assert_eq!(changed, observation_execution::summary_at_sequence(&transaction, sequence).unwrap());
    transaction.execute("UPDATE events SET observation_execution=?2 WHERE sequence=?1", params![sequence, b"[".as_slice()]).unwrap();
    assert!(observation_execution::summary_at_sequence_prepared(&transaction, sequence, Some(execution)).is_err());
    // A terminal execution failure must not erase an otherwise valid receipt.
    transaction.execute("UPDATE events SET observation_witness=CAST(json_set(?2,'$.event_kind','attempt_failed') AS BLOB) WHERE sequence=?1", params![sequence, &capture.bytes]).unwrap();
    let receipt = observation_receipt::single_prepared(&transaction, store.conversation_id(), &persisted.event, Some(capture), Some(execution)).unwrap();
    assert!(receipt.failure.is_none());
    transaction.rollback().unwrap();
}

#[test]
fn append_aggregate_failure_keeps_other_event_evidence_independent() {
    use super::observation_response_tests::{response_message, response_start};
    let store = store();
    store.initialize(&[]).unwrap();
    let attempt = response_start(&store, "aggregate-failure");
    store.lock().unwrap().execute_batch("CREATE TRIGGER fail_members BEFORE UPDATE OF observation_members ON events BEGIN SELECT RAISE(FAIL,'member layout unavailable'); END;").unwrap();
    response_message(&store, &attempt, "aggregate-failure-message");
    let connection = store.lock().unwrap();
    let state: (bool, bool, bool, bool, bool) = connection.query_row(
        "SELECT observation_statistics IS NULL,observation_members IS NULL,observation_trace_total IS NULL,observation_execution IS NOT NULL,observation_witness IS NOT NULL FROM events WHERE sequence=2", [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
    ).unwrap();
    assert_eq!(state, (true, true, true, true, true));
    drop(connection);
    assert!(store.observation_statistics(2).is_err());
    assert_eq!(store.load_canonical().unwrap().len(), 1);
}

#[test]
fn append_statistics_reuse_keeps_actual_cut_and_hole_checks() {
    use super::observation_response_tests::response_start;
    let store = store();
    store.initialize(&[]).unwrap();
    response_start(&store, "stored-statistics");
    let event = store.read_events(None, 1).unwrap().events.pop().unwrap();
    let connection = store.lock().unwrap();
    let prepared = observation_statistics::prepare(&connection, &event).unwrap();
    assert_eq!(observation_statistics::read_prepared(&connection, 1, Some(&prepared)).unwrap(), observation_statistics::read(&connection, 1).unwrap());
    let mut changed: serde_json::Value = serde_json::from_slice(&prepared.statistics.as_ref().unwrap().bytes).unwrap();
    changed[0][0] = serde_json::json!(99);
    connection.execute("UPDATE events SET observation_statistics=?1 WHERE sequence=1", [serde_json::to_vec(&changed).unwrap()]).unwrap();
    let actual = observation_statistics::read_prepared(&connection, 1, Some(&prepared)).unwrap();
    assert_eq!(actual.turns, 99);
    assert_eq!(actual, observation_statistics::read(&connection, 1).unwrap());
    assert!(matches!(observation_statistics::read_prepared(&connection, 2, Some(&prepared)), Err(ConversationStoreError::InvalidReference(_))));
    connection.execute("UPDATE events SET observation_statistics=NULL WHERE sequence=1", []).unwrap();
    assert!(observation_statistics::read_prepared(&connection, 1, Some(&prepared)).is_err());
}
