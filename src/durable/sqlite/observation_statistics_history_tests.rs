// Statistical facts are independent of native bodies; checkpoint validity and
// replay distance are part of the new format, not compatibility behavior.
#[test]
fn statistics_contributions_remove_full_per_event_materialization_and_match_all_cuts() {
    use self::observation_response_tests::{response_complete, response_message, response_start};
    use crate::runtime_client::agent_statistics::StatisticsFold;
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
    assert_eq!(observation_statistics::write_work(), (2, 41));
    assert_eq!(observation_statistics::fold_work(), (40, 1));
    assert_eq!(
        observation_statistics::read_work().0,
        1,
        "warm writes never fetch old aggregate BLOBs"
    );
    response_complete(&store, &attempt);
    assert_eq!(observation_statistics::write_work(), (1, 1));
    let counts:(u64,u64)=store.lock().unwrap().query_row("SELECT count(observation_statistics),count(observation_statistics_checkpoint) FROM events",[],|row|Ok((row.get(0)?,row.get(1)?))).unwrap();
    assert_eq!(counts, (42, 3));
    let events = store.read_events(None, 128).unwrap().events;
    for cut in 0..=events.len() {
        let expected = StatisticsFold::reference_events(&events[..cut]);
        observation_statistics::read_work();
        assert_eq!(
            store.observation_agent_statistics(cut as u64).unwrap(),
            expected
        );
        let (rows, bytes, bodies) = observation_statistics::read_work();
        assert!(rows <= 18, "{rows} at {cut}");
        assert!(bytes <= 18 * 49_152);
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
fn statistics_checkpoint_is_independent_of_older_contribution_damage_but_not_its_own_bytes() {
    use self::observation_response_tests::{response_message, response_start};
    let store = store();
    store.initialize(&[]).unwrap();
    let attempt = response_start(&store, "independent-statistics");
    for i in 0..32 {
        response_message(
            &store,
            &attempt,
            &format!("independent-statistics-message-{i}"),
        );
    }
    {
        let connection = store.lock().unwrap();
        let original: Vec<u8> = connection
            .query_row(
                "SELECT observation_statistics FROM events WHERE sequence=16",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let mut changed: serde_json::Value = observation_codec::decode(&original).unwrap();
        let seconds = changed[3][1][0].as_i64().unwrap();
        changed[3][1][0] = (seconds + 1).into();
        let transaction = connection.unchecked_transaction().unwrap();
        transaction
            .execute(
                "UPDATE events SET observation_statistics=?1 WHERE sequence=16",
                [observation_codec::encode_admitted(&changed).unwrap()],
            )
            .unwrap();
        assert!(
            observation_statistics::read(&transaction, 16).is_err(),
            "checkpoint must bind its own authoritative fact too"
        );
        transaction.rollback().unwrap();
    }
    let before = store.observation_statistics(16).unwrap();
    let latest = store.observation_statistics(33).unwrap();
    for replacement in ["x'ff'", "NULL"] {
        store
            .lock()
            .unwrap()
            .execute_batch(&format!(
                "UPDATE events SET observation_statistics={replacement} WHERE sequence=2"
            ))
            .unwrap();
        assert!(store.observation_statistics(3).is_err());
        assert_eq!(store.observation_statistics(16).unwrap(), before);
        assert_eq!(store.observation_statistics(33).unwrap(), latest);
    }
    response_message(&store, &attempt, "after-old-statistics-damage");
    assert_eq!(store.observation_statistics(34).unwrap().turns, 1);
    store
        .lock()
        .unwrap()
        .execute(
            "UPDATE events SET observation_statistics_checkpoint=x'ff' WHERE sequence=32",
            [],
        )
        .unwrap();
    assert!(store.observation_statistics(34).is_err());
    response_message(&store, &attempt, "after-current-checkpoint-damage");
    assert!(store.observation_statistics(35).is_err());
    assert_eq!(store.load_canonical().unwrap().len(), 34);
    assert_eq!(store.read_events(None, 128).unwrap().events.len(), 35);
}

#[test]
fn statistics_replay_never_reads_native_bodies_or_timestamps() {
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
fn statistics_forged_ancient_anchor_fails_with_bounded_index_work() {
    use self::observation_response_tests::{response_message, response_start};
    let store = store();
    store.initialize(&[]).unwrap();
    let attempt = response_start(&store, "bounded-statistical-history");
    for i in 0..40 {
        response_message(
            &store,
            &attempt,
            &format!("bounded-statistical-message-{i}"),
        );
    }
    let connection = store.lock().unwrap();
    let bytes: Vec<u8> = connection
        .query_row(
            "SELECT observation_statistics FROM events WHERE sequence=41",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let mut record: serde_json::Value = observation_codec::decode(&bytes).unwrap();
    record[1] = 0.into();
    record[2] = 1.into();
    connection
        .execute(
            "UPDATE events SET observation_statistics=?1 WHERE sequence=41",
            [observation_codec::encode_admitted(&record).unwrap()],
        )
        .unwrap();
    let sql = "SELECT sequence,CASE WHEN typeof(observation_statistics)='blob' AND length(observation_statistics)<=49152 THEN observation_statistics ELSE 0 END,observation_statistics_checkpoint IS NOT NULL FROM events INDEXED BY events_aggregate_heads_idx WHERE observation_aggregate_heads>0 AND observation_aggregate_heads=?1 AND sequence>?2 AND sequence<=?3 ORDER BY sequence LIMIT 16";
    connection
        .prepare_cached(sql)
        .unwrap()
        .reset_status(rusqlite::StatementStatus::VmStep);
    assert!(observation_statistics::read(&connection, 41).is_err());
    let statement = connection.prepare_cached(sql).unwrap();
    assert_eq!(
        statement.get_status(rusqlite::StatementStatus::FullscanStep),
        0
    );
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
        bytes.len() < 128,
        "a giant identity leaves a bounded scalar fact"
    );
    let record: serde_json::Value = observation_codec::decode(&bytes).unwrap();
    let identity = record[3][2].as_array().unwrap();
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
