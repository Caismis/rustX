//! Adversarial identity probes run against the project's bundled SQLite.
use super::identity_tests::append;
use super::*;
use crate::durable::sqlite::test_support::store;

#[test]
fn identity_review_conflict_modes_preserve_constraints() {
    for policy in ["ABORT", "FAIL", "ROLLBACK", "IGNORE", "REPLACE"] {
        for action in [
            "INSERT OR {policy} INTO events(sequence,event_id,schema_version,conversation_id,event_timestamp,event_json) SELECT 3,event_id,1,conversation_id,'time','{}' FROM events WHERE sequence=1",
            "UPDATE OR {policy} events SET event_id=(SELECT event_id FROM events WHERE sequence=1) WHERE sequence=2",
            "UPDATE OR {policy} events SET sequence=3 WHERE sequence=1",
            "UPDATE OR {policy} events SET conversation_id='foreign' WHERE sequence=1",
        ] {
            let store = store();
            let first = append(&store, "").unwrap();
            let second = append(&store, "prior-alias").unwrap();
            let result = store
                .lock()
                .unwrap()
                .execute(&action.replace("{policy}", policy), []);
            if policy == "IGNORE" && !action.contains("conversation_id='foreign'") {
                assert_eq!(result.unwrap(), 0);
            } else {
                assert_eq!(
                    result.unwrap_err().sqlite_error_code(),
                    Some(rusqlite::ErrorCode::ConstraintViolation)
                );
            }
            assert_eq!(store.event_high_watermark().unwrap(), 2);
            assert_eq!(
                store.read_events(None, 8).unwrap().events,
                vec![first, second]
            );
        }
    }
}

#[test]
fn identity_review_trigger_conflict_paths_keep_rollback_and_high_watermark() {
    for schema in ["", "TEMP "] {
        for timing in ["BEFORE", "AFTER"] {
            for policy in ["REPLACE", "IGNORE"] {
                for action in [
                    "UPDATE OR {policy} events SET event_id=NEW.event_id WHERE sequence=1;",
                    "UPDATE OR {policy} events SET sequence=3 WHERE sequence=NEW.sequence;",
                    "INSERT OR {policy} INTO events(sequence,event_id,schema_version,conversation_id,event_timestamp,event_json) VALUES(3,NEW.event_id,1,NEW.conversation_id,'time','{}');",
                ] {
                    if timing == "BEFORE" && action.contains("SET sequence=3") {
                        continue;
                    }
                    let store = store();
                    let first = append(&store, "prior-alias").unwrap();
                    let rewrite = action.replace("{policy}", policy);
                    store
                        .lock()
                        .unwrap()
                        .execute_batch(&format!(
                            "CREATE TABLE identity_effects(value INTEGER);
                         CREATE {schema}TRIGGER conflict_identity {timing} INSERT ON events
                         WHEN NEW.sequence=2 BEGIN
                           INSERT INTO identity_effects VALUES(1);
                           {rewrite}
                         END;"
                        ))
                        .unwrap();
                    let result = append(&store, "");
                    let expected_effects = u64::from(policy == "IGNORE");
                    if policy == "IGNORE" {
                        let second = result.unwrap();
                        assert_eq!(
                            store.read_events(None, 8).unwrap().events,
                            vec![first, second]
                        );
                        assert_eq!(store.event_high_watermark().unwrap(), 2);
                    } else {
                        assert!(
                            matches!(result, Err(ConversationStoreError::Storage(_))),
                            "{schema}{timing}: {rewrite}"
                        );
                        assert_eq!(store.read_events(None, 8).unwrap().events, vec![first]);
                        assert_eq!(store.event_high_watermark().unwrap(), 1);
                    }
                    let connection = store.lock().unwrap();
                    let effects: u64 = connection
                        .query_row("SELECT count(*) FROM identity_effects", [], |row| {
                            row.get(0)
                        })
                        .unwrap();
                    assert_eq!(effects, expected_effects);
                    connection
                        .execute_batch("DROP TRIGGER conflict_identity")
                        .unwrap();
                    drop(connection);
                    assert_eq!(append(&store, "").unwrap().sequence, 2 + expected_effects);
                }
            }
        }
    }
}

#[test]
fn identity_review_parent_nonkey_writes_do_not_scan_history() {
    use rusqlite::StatementStatus;
    let writes = [
        "next_inbound_sequence=next_inbound_sequence+1",
        "next_event_sequence=next_event_sequence+1",
        "next_transcript_position=next_transcript_position+1,retained_transcript_count=retained_transcript_count+1",
        "pending_observation_count=pending_observation_count+1",
        "retained_transcript_count=retained_transcript_count,reading_mutation_revision=reading_mutation_revision+1",
        "observation_statistics=observation_statistics",
        "logical_step_sequence=logical_step_sequence+1",
        "pending_unresolved_output_stream_id='probe'",
        "bootstrap_initialized=1",
    ];
    let store = store();
    let mut expected_steps = vec![None; writes.len()];
    for history in [8, 64, 512] {
        while store.event_high_watermark().unwrap() < history {
            append(&store, "").unwrap();
        }
        let mut connection = store.lock().unwrap();
        let transaction = connection.transaction().unwrap();
        for (write, expected) in writes.iter().zip(&mut expected_steps) {
            let mut statement = transaction
                .prepare(&format!("UPDATE rustx_store SET {write} WHERE id=1"))
                .unwrap();
            assert_eq!(statement.execute([]).unwrap(), 1);
            let steps = statement.get_status(StatementStatus::VmStep);
            assert_eq!(
                *expected.get_or_insert(steps),
                steps,
                "H={history}: {write}"
            );
            assert_eq!(statement.get_status(StatementStatus::FullscanStep), 0);
            assert_eq!(statement.get_status(StatementStatus::Sort), 0);
            println!("parent write H={history}: {steps} steps, no scan/sort: {write}");
        }
        transaction.rollback().unwrap();
    }
}

#[test]
fn identity_review_observation_triggers_cannot_move_new_rows() {
    for schema in ["", "TEMP "] {
        for timing in ["BEFORE", "AFTER"] {
            for rewrite in [
                "event_id=(SELECT event_id FROM events WHERE sequence=1)",
                "sequence=3",
                "conversation_id='foreign'",
            ] {
                let store = store();
                let first = append(&store, "").unwrap();
                let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
                let observed = calls.clone();
                let connection = store.lock().unwrap();
                connection
                    .create_scalar_function(
                        "observe_move",
                        0,
                        rusqlite::functions::FunctionFlags::SQLITE_UTF8,
                        move |_| {
                            observed.fetch_add(1, Ordering::SeqCst);
                            Ok(0)
                        },
                    )
                    .unwrap();
                connection.execute_batch(&format!(
                    "CREATE {schema}TRIGGER move_observation {timing} UPDATE OF observation_readiness_missing ON events
                     WHEN NEW.sequence=2 BEGIN
                       SELECT observe_move();
                       UPDATE events SET {rewrite} WHERE sequence=NEW.sequence;
                     END;"
                )).unwrap();
                drop(connection);
                let second = append(&store, "").unwrap();
                assert!(calls.load(Ordering::SeqCst) > 0);
                assert_eq!(store.event_high_watermark().unwrap(), 2);
                assert_eq!(
                    store.read_events(None, 8).unwrap().events,
                    vec![first, second]
                );
                let missing: bool = store
                    .lock()
                    .unwrap()
                    .query_row(
                        "SELECT observation_readiness_missing FROM events WHERE sequence=2",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert!(missing);
            }
        }
    }
}

#[test]
fn identity_review_parent_schema_cannot_enable_multiple_conversations() {
    for (original, replacement) in [
        ("CHECK(id=1)", "CHECK(1)"),
        ("CHECK(id=1)", "CHECK(id=1 OR 1)"),
        ("CHECK(id=1)", "CHECK(1) /* CHECK(id=1) */"),
        ("INTEGER PRIMARY KEY CHECK(id=1)", "INTEGER CHECK(id=1)"),
        (
            "conversation_id TEXT NOT NULL",
            "conversation_id TEXT COLLATE NOCASE NOT NULL",
        ),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("identity.sqlite");
        let id = ConversationId::generate();
        let store = SqliteConversationStore::open(id.clone(), &path).unwrap();
        append(&store, "prior-alias").unwrap();
        {
            let connection = store.lock().unwrap();
            let ddl: String = connection
                .query_row(
                    "SELECT sql FROM sqlite_schema WHERE name='rustx_store'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(ddl.contains(original));
            connection
                .execute_batch("PRAGMA writable_schema=ON")
                .unwrap();
            connection
                .execute(
                    "UPDATE sqlite_schema SET sql=?1 WHERE name='rustx_store'",
                    [ddl.replace(original, replacement)],
                )
                .unwrap();
            connection
                .execute_batch("PRAGMA writable_schema=OFF")
                .unwrap();
            assert!(matches!(
                super::super::schema::shape::verify_schema_shape(&connection),
                Err(ConversationStoreError::IncompatibleSchema(_))
            ));
        }
        drop(store);
        let assert_rejected =
            |result: Result<SqliteConversationStore, ConversationStoreError>| match result {
                Err(ConversationStoreError::IncompatibleSchema(_)) => {}
                // Removing a rowid alias also changes the physical record layout:
                // the existing schema-version read can reject it before the DDL check.
                Err(ConversationStoreError::Storage(_))
                    if original == "INTEGER PRIMARY KEY CHECK(id=1)" => {}
                Err(error) => panic!("{replacement}: unexpected error: {error:?}"),
                Ok(_) => panic!("{replacement}: tampered parent accepted"),
            };
        assert_rejected(SqliteConversationStore::open(id.clone(), &path));
        assert_rejected(SqliteConversationStore::open_existing(id, &path));
    }
}
