// Each statistical event owns one independently bound full historical record.
#[test]
fn statistics_full_records_match_every_cut_with_one_direct_read() {
    use self::observation_response_tests::{response_complete, response_message, response_start};
    use crate::observation::statistics::StatisticsFold;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("statistics.sqlite");
    let id = ConversationId::generate();
    let store = SqliteConversationStore::open(id.clone(), &path).unwrap();
    store.initialize(&[]).unwrap();
    observation_statistics::write_work();
    observation_statistics::fold_work();
    observation_statistics::read_work();
    let attempt = response_start(&store, "contribution-owner");
    for i in 0..40 {
        response_message(&store, &attempt, &format!("contribution-message-{i}"));
    }
    assert_eq!(observation_statistics::write_work(), (41, 41));
    assert_eq!(observation_statistics::fold_work(), (40, 1));
    assert_eq!(
        observation_statistics::read_work().0,
        1,
        "warm writes never fetch old aggregate BLOBs"
    );
    response_complete(&store, &attempt);
    assert_eq!(observation_statistics::write_work(), (1, 1));
    let count: u64 = store.lock().unwrap().query_row(
        "SELECT count(observation_statistics) FROM events", [], |row| row.get(0),
    ).unwrap();
    assert_eq!(count, 42);
    let obsolete: bool = store.lock().unwrap().query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_xinfo('events') WHERE name='observation_statistics_checkpoint')",
        [], |row| row.get(0),
    ).unwrap();
    assert!(!obsolete, "there is one full record column, not two history forms");
    let events = store.read_events(None, 128).unwrap().events;
    for cut in 0..=events.len() {
        let expected = StatisticsFold::reference_events(&events[..cut]);
        observation_statistics::read_work();
        assert_eq!(
            store.observation_agent_statistics(cut as u64).unwrap(),
            expected
        );
        let (rows, bytes, bodies) = observation_statistics::read_work();
        assert!(rows <= 1, "{rows} at {cut}");
        assert!(bytes <= 98_340);
        assert_eq!(bodies, 0);
    }
    drop(store);
    let reopened = SqliteConversationStore::open_existing(id, &path).unwrap();
    for cut in 0..=events.len() {
        assert_eq!(
            reopened.observation_agent_statistics(cut as u64).unwrap(),
            StatisticsFold::reference_events(&events[..cut])
        );
    }
}

#[test]
fn statistics_full_record_is_independent_of_older_damage_but_not_its_own_bytes() {
    use self::observation_response_tests::{response_message, response_start};
    let store = store();
    store.initialize(&[]).unwrap();
    let attempt = response_start(&store, "independent-statistics");
    for i in 0..32 {
        response_message(&store, &attempt, &format!("independent-statistics-message-{i}"));
    }
    let third = store.observation_statistics(3).unwrap();
    let latest = store.observation_statistics(33).unwrap();
    for replacement in ["x'ff'", "NULL"] {
        store.lock().unwrap().execute_batch(&format!(
            "UPDATE events SET observation_statistics={replacement} WHERE sequence=2"
        )).unwrap();
        assert!(store.observation_statistics(2).is_err());
        assert_eq!(store.observation_statistics(3).unwrap(), third);
        assert_eq!(store.observation_statistics(33).unwrap(), latest);
    }
    response_message(&store, &attempt, "after-old-statistics-damage");
    assert_eq!(store.observation_statistics(34).unwrap().turns, 1);
    store.lock().unwrap().execute(
        "UPDATE events SET observation_statistics=x'ff' WHERE sequence=34", [],
    ).unwrap();
    assert!(store.observation_statistics(34).is_err());
    response_message(&store, &attempt, "after-current-statistics-damage");
    assert!(store.observation_statistics(35).is_err());
    assert_eq!(store.load_canonical().unwrap().len(), 34);
    assert_eq!(store.read_events(None, 128).unwrap().events.len(), 35);
}

#[test]
fn statistics_full_record_never_reads_native_bodies_or_timestamps() {
    use self::observation_response_tests::{response_message, response_start};
    let store = store();
    store.initialize(&[]).unwrap();
    let attempt = response_start(&store, "independent-statistical-facts");
    response_message(&store, &attempt, "independent-statistical-message");
    let expected = store.observation_agent_statistics(2).unwrap();
    let connection = store.lock().unwrap();
    connection
        .authorizer(Some(
            |context: rusqlite::hooks::AuthContext<'_>| match context.action {
                rusqlite::hooks::AuthAction::Read {
                    table_name: "events",
                    column_name: "event_json" | "event_timestamp",
                    ..
                } => rusqlite::hooks::Authorization::Deny,
                _ => rusqlite::hooks::Authorization::Allow,
            },
        ))
        .unwrap();
    assert_eq!(
        observation_statistics::agent_read(&connection, 2).unwrap(),
        expected
    );
    connection
        .authorizer(None::<fn(rusqlite::hooks::AuthContext<'_>) -> rusqlite::hooks::Authorization>)
        .unwrap();
}

#[test]
fn statistics_savepoint_rollback_cannot_publish_a_provisional_fold() {
    use self::observation_response_tests::{response_message, response_start};
    let store = store();
    store.initialize(&[]).unwrap();
    let attempt = response_start(&store, "statistics-savepoint");
    let mut event = store.read_events(None, 1).unwrap().events.pop().unwrap();
    event.event_id = EventId::new("statistics-rolled-back-step");
    event.event = RuntimeEvent::TurnStarted;
    event.turn_id = Some(TurnId::new("1"));
    {
        let connection = store.lock().unwrap();
        let transaction = connection.unchecked_transaction().unwrap();
        transaction
            .execute_batch("SAVEPOINT statistics_outer")
            .unwrap();
        persist_event_tx(
            &store.conn.event_guard,
            &transaction,
            &store.conversation_id,
            event,
        )
        .unwrap();
        transaction
            .execute_batch("ROLLBACK TO statistics_outer; RELEASE statistics_outer")
            .unwrap();
        transaction.commit().unwrap();
    }
    observation_statistics::fold_work();
    response_message(&store, &attempt, "statistics-after-savepoint");
    assert!(observation_statistics::fold_work().1 >= 1);
    assert_eq!(store.observation_statistics(2).unwrap().steps, 0);
}

#[test]
fn statistics_full_record_is_row_bound_and_uses_one_bounded_head_lookup() {
    use self::observation_response_tests::{response_message, response_start};
    let store = store();
    store.initialize(&[]).unwrap();
    let attempt = response_start(&store, "bounded-statistical-history");
    for i in 0..40 {
        response_message(&store, &attempt, &format!("bounded-statistical-message-{i}"));
    }
    let connection = store.lock().unwrap();
    let bytes: Vec<u8> = connection.query_row(
        "SELECT observation_statistics FROM events WHERE sequence=40", [], |row| row.get(0),
    ).unwrap();
    connection.execute(
        "UPDATE events SET observation_statistics=?1 WHERE sequence=41", [&bytes],
    ).unwrap();
    let sql = super::event_heads::STATISTICS;
    connection.prepare_cached(sql).unwrap().reset_status(rusqlite::StatementStatus::VmStep);
    assert!(observation_statistics::read(&connection, 41).is_err());
    let statement = connection.prepare_cached(sql).unwrap();
    assert_eq!(statement.get_status(rusqlite::StatementStatus::FullscanStep), 0);
    assert_eq!(statement.get_status(rusqlite::StatementStatus::Sort), 0);
    assert!(statement.get_status(rusqlite::StatementStatus::VmStep) < 1000);
}

#[test]
fn statistics_rollback_sequence_reuse_in_one_commit_cannot_inherit_provisional_steps() {
    use self::observation_response_tests::response_start;
    let store = store();
    store.initialize(&[]).unwrap();
    let attempt = response_start(&store, "statistics-reused-sequence");
    let mut live = envelope(
        store.conversation_id(),
        "statistics-live-turn",
        Some(attempt.clone()),
        RuntimeEvent::TurnStarted,
    );
    live.turn_id = Some(TurnId::new("live"));
    store.append_event(live).unwrap();
    {
        let connection = store.lock().unwrap();
        let transaction = connection.unchecked_transaction().unwrap();
        transaction
            .execute_batch("SAVEPOINT reuse_statistics_sequence")
            .unwrap();
        let mut temporary = envelope(
            store.conversation_id(),
            "statistics-temporary-turn",
            Some(attempt.clone()),
            RuntimeEvent::TurnStarted,
        );
        temporary.turn_id = Some(TurnId::new("temporary"));
        persist_event_tx(
            &store.conn.event_guard,
            &transaction,
            &store.conversation_id,
            temporary,
        )
        .unwrap();
        transaction
            .execute_batch(
                "ROLLBACK TO reuse_statistics_sequence; RELEASE reuse_statistics_sequence",
            )
            .unwrap();
        let mut completed = envelope(
            store.conversation_id(),
            "statistics-live-turn-done",
            Some(attempt.clone()),
            RuntimeEvent::TurnCompleted,
        );
        completed.turn_id = Some(TurnId::new("live"));
        persist_event_tx(
            &store.conn.event_guard,
            &transaction,
            &store.conversation_id,
            completed,
        )
        .unwrap();
        let terminal = envelope(
            store.conversation_id(),
            "statistics-reused-sequence-terminal",
            Some(attempt.clone()),
            RuntimeEvent::AttemptTimedOut {
                attempt_id: attempt,
            },
        );
        persist_event_tx(
            &store.conn.event_guard,
            &transaction,
            &store.conversation_id,
            terminal,
        )
        .unwrap();
        transaction.commit().unwrap();
    }
    assert_eq!(store.observation_statistics(4).unwrap().steps, 1);
    assert_eq!(store.read_events(None, 16).unwrap().events.len(), 4);
}

#[test]
fn statistics_start_facts_derive_deferred_clock_coordinates_and_preserve_exact_time() {
    let store = store();
    store.initialize(&[]).unwrap();
    let attempt = AttemptId::new("水".repeat(2000));
    let mut start = envelope(
        store.conversation_id(),
        "compact-clock-start",
        Some(attempt.clone()),
        RuntimeEvent::AttemptStarted {
            attempt_id: attempt.clone(),
        },
    );
    start.timestamp = chrono::DateTime::from_timestamp(1_700_000_000, 123_456_789).unwrap();
    let at = start.timestamp;
    let start = store.append_event(start).unwrap();
    let actual = store.observation_statistics(start.sequence).unwrap();
    let clock = actual.latest_turn.unwrap();
    assert!(clock.attempt_id.is_none());
    assert!(clock.attempt_id_deferred);
    assert_eq!(clock.started_at, at);
    let reference = clock.attempt_id_source.unwrap();
    assert_eq!(reference.sequence, start.sequence);
    assert_eq!(reference.path, ["attempt_id"]);
    assert_eq!(
        reference.encoded_bytes,
        serde_json::to_vec(&attempt).unwrap().len() as u64
    );
    let bytes: Vec<u8> = store
        .lock()
        .unwrap()
        .query_row(
            "SELECT observation_statistics FROM events WHERE sequence=?1",
            [start.sequence],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        bytes.len() < 2048,
        "a giant identity leaves a bounded full record and deferred clock"
    );
    let (fact, _) = observation_statistics::stored_parts(start.sequence, &bytes).unwrap();
    let record: serde_json::Value = observation_codec::decode(fact).unwrap();
    let identity = record[1][2].as_array().unwrap();
    assert_eq!(identity.len(), 2);
    assert!(identity[0].is_null());
    assert_eq!(identity[1], serde_json::json!(reference.encoded_bytes));
    let mut end = envelope(
        store.conversation_id(),
        "compact-clock-end",
        Some(attempt.clone()),
        RuntimeEvent::AttemptTimedOut {
            attempt_id: attempt,
        },
    );
    end.timestamp = at + chrono::Duration::seconds(5);
    store.append_event(end).unwrap();
    let actual = store.observation_agent_statistics(2).unwrap();
    assert_eq!(actual.duration.settled_ms, 5000);
    assert_eq!(
        actual.statistics.latest_turn.unwrap().ended_at,
        Some(at + chrono::Duration::seconds(5))
    );
    assert_eq!(
        store
            .observation_statistics(1)
            .unwrap()
            .latest_turn
            .unwrap()
            .started_at,
        at
    );
}

#[test]
fn statistics_normal_nonstatistical_batch_keeps_the_provisional_prefix() {
    use self::observation_response_tests::response_start;
    let store = store();
    store.initialize(&[]).unwrap();
    let attempt = response_start(&store, "statistics-forward-prefix");
    observation_statistics::fold_work();
    observation_statistics::read_work();
    {
        let connection = store.lock().unwrap();
        let transaction = connection.unchecked_transaction().unwrap();
        for (id, event) in [
            ("forward-start", RuntimeEvent::TurnStarted),
            ("forward-done", RuntimeEvent::TurnCompleted),
            (
                "forward-terminal",
                RuntimeEvent::AttemptTimedOut {
                    attempt_id: attempt.clone(),
                },
            ),
        ] {
            let mut envelope = envelope(store.conversation_id(), id, Some(attempt.clone()), event);
            envelope.turn_id = Some(TurnId::new("forward-turn"));
            persist_event_tx(
                &store.conn.event_guard,
                &transaction,
                &store.conversation_id,
                envelope,
            )
            .unwrap();
        }
        transaction.commit().unwrap();
    }
    assert_eq!(observation_statistics::fold_work(), (2, 0));
    assert_eq!(observation_statistics::read_work().0, 0);
    assert_eq!(store.observation_statistics(4).unwrap().steps, 1);
}

#[test]
fn read_only_rollback_keeps_committed_statistics_but_write_rollback_cannot_seat_it() {
    use self::observation_response_tests::{response_message, response_start};
    let store = store();
    store.initialize(&[]).unwrap();
    let attempt = response_start(&store, "statistics-read-rollback");
    observation_statistics::fold_work();
    for i in 0..3 {
        assert_eq!(store.observation_statistics(i + 1).unwrap().turns, 1);
        response_message(&store, &attempt, &format!("after-statistics-read-{i}"));
    }
    assert_eq!(observation_statistics::fold_work(), (3, 0));
    {
        let connection = store.lock().unwrap();
        let transaction = connection.unchecked_transaction().unwrap();
        let mut event = envelope(
            store.conversation_id(),
            "rolled-back-statistics-read",
            Some(attempt.clone()),
            RuntimeEvent::TurnStarted,
        );
        event.turn_id = Some(TurnId::new("rolled-back-only"));
        persist_event_tx(
            &store.conn.event_guard,
            &transaction,
            &store.conversation_id,
            event,
        )
        .unwrap();
        transaction.rollback().unwrap();
    }
    response_message(&store, &attempt, "after-statistics-write-rollback");
    assert_eq!(store.observation_statistics(5).unwrap().steps, 0);
}
