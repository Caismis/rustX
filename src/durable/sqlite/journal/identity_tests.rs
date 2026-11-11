//! Public aliases retain exact identity while ordinary appends use sequence.
use super::*;
use crate::durable::sqlite::test_support::{envelope, store};
use crate::durable::{ConversationStore, InboxStore, JournalStore};

pub(super) fn append(
    store: &SqliteConversationStore,
    id: &str,
) -> Result<RuntimeEventEnvelope, ConversationStoreError> {
    let mut event = envelope(store.conversation_id(), id, None, RuntimeEvent::TurnStarted);
    event.turn_id = Some(crate::runtime::identity::TurnId::new(
        (store.event_high_watermark()? + 1).to_string(),
    ));
    store.append_event(event)
}

#[test]
fn journal_pages_reuse_statement_and_recheck_current_rows() {
    use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
    use std::sync::atomic::{AtomicUsize, Ordering};

    let store = store();
    for _ in 0..3 {
        append(&store, "").unwrap();
    }
    let prepares = std::sync::Arc::new(AtomicUsize::new(0));
    {
        let connection = store.lock().unwrap();
        connection.flush_prepared_statement_cache();
        let prepares = prepares.clone();
        connection
            .authorizer(Some(move |context: AuthContext<'_>| {
                if matches!(
                    context.action,
                    AuthAction::Read {
                        table_name: "events",
                        column_name: "event_json"
                    }
                ) {
                    prepares.fetch_add(1, Ordering::Relaxed);
                }
                Authorization::Allow
            }))
            .unwrap();
    }
    let mut cursor = None;
    for sequence in 1..=3 {
        let page = store.read_events(cursor, 1).unwrap();
        assert_eq!(page.events[0].sequence, sequence);
        cursor = page.next_sequence;
    }
    assert!(store.read_events(cursor, 1).unwrap().events.is_empty());
    assert_eq!(prepares.load(Ordering::Relaxed), 1);
    {
        let connection = store.lock().unwrap();
        connection
            .authorizer(None::<fn(AuthContext<'_>) -> Authorization>)
            .unwrap();
        connection
            .execute("UPDATE events SET event_json='{}' WHERE sequence=2", [])
            .unwrap();
    }
    assert!(store.read_events(Some(1), 1).is_err());
}

#[test]
fn numeric_identity_keeps_custom_aliases_and_rejects_reserved_defaults() {
    let store = store();
    let first = append(&store, "").unwrap();
    assert_eq!(
        first.event_id.as_str(),
        format!("{}-event-1", store.conversation_id())
    );
    let second = append(&store, "producer🙂").unwrap();
    let future = format!("{}-event-4", store.conversation_id());
    assert!(matches!(
        append(&store, &future),
        Err(ConversationStoreError::Storage(_))
    ));
    assert_eq!(store.event_high_watermark().unwrap(), 2);
    let third = append(&store, "").unwrap();
    assert_eq!(third.sequence, 3);
    assert!(append(&store, first.event_id.as_str()).is_err());
    assert!(append(&store, second.event_id.as_str()).is_err());
    let mut connection = store.lock().unwrap();
    let transaction = connection.transaction().unwrap();
    for event in [&first, &second, &third] {
        let found = find_event_by_id(&transaction, &event.event_id)
            .unwrap()
            .unwrap();
        assert_eq!(found.event_id, event.event_id);
        assert_eq!(found.sequence, event.sequence);
    }
    for id in [
        "",
        "producer",
        "foreign-event-1",
        "missing-event-0001",
        "missing-event-9223372036854775808",
    ] {
        assert!(
            find_event_by_id(&transaction, &EventId::new(id))
                .unwrap()
                .is_none()
        );
    }
    transaction
        .execute_batch("PRAGMA ignore_check_constraints=ON; SAVEPOINT corrupt_alias")
        .unwrap();
    transaction
        .execute(
            "UPDATE events SET event_id=?1 WHERE sequence=?2",
            params![first.event_id.as_str(), second.sequence],
        )
        .unwrap();
    assert!(find_event_by_id(&transaction, &first.event_id).is_err());
    transaction
        .execute_batch(
            "ROLLBACK TO corrupt_alias; RELEASE corrupt_alias; PRAGMA ignore_check_constraints=OFF",
        )
        .unwrap();
    transaction.rollback().unwrap();
    drop(connection);
    assert_eq!(store.read_events(None, 8).unwrap().events.len(), 3);
}

#[test]
fn default_event_alias_tree_stays_empty_and_identity_seeks_stay_bounded() {
    use rusqlite::StatementStatus;
    let store = store();
    for history in [8, 64, 512, 4096] {
        while store.event_high_watermark().unwrap() < history {
            append(&store, "").unwrap();
        }
        let connection = store.lock().unwrap();
        let cells: u64 = connection
            .query_row(
                "SELECT coalesce(sum(ncell),0) FROM dbstat WHERE name='events_alias_idx'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            cells, 0,
            "H={history}: default events must not enter the alias tree"
        );
        let default = format!("{}-event-{history}", store.conversation_id());
        let mut seek = connection.prepare(EVENT_SEQUENCE_SQL).unwrap();
        let found: u64 = seek
            .query_row(params![default, history], |row| row.get(0))
            .unwrap();
        assert_eq!(found, history);
        assert_eq!(seek.get_status(StatementStatus::FullscanStep), 0);
        assert_eq!(seek.get_status(StatementStatus::Sort), 0);
        assert!(seek.get_status(StatementStatus::VmStep) < 80);
    }
}

#[test]
fn terminal_guard_keeps_precedence_over_a_repeated_event_alias() {
    let store = store();
    let attempt = crate::runtime::identity::AttemptId::new("terminal-owner");
    store
        .append_event(envelope(
            store.conversation_id(),
            "start",
            Some(attempt.clone()),
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt.clone(),
            },
        ))
        .unwrap();
    let terminal = envelope(
        store.conversation_id(),
        "terminal",
        Some(attempt.clone()),
        RuntimeEvent::AttemptTimedOut {
            attempt_id: attempt,
        },
    );
    store.append_event(terminal.clone()).unwrap();
    assert!(matches!(
        store.append_event(terminal),
        Err(ConversationStoreError::TerminalViolation(_))
    ));
}

#[test]
fn transcript_identity_lookup_preserves_decode_error_domain() {
    for id in ["", "producer-transcript"] {
        let store = store();
        let event = append(&store, id).unwrap();
        let connection = store.lock().unwrap();
        connection
            .execute(
                "UPDATE events SET event_json='{}' WHERE sequence=?1",
                [event.sequence],
            )
            .unwrap();
        let error = crate::durable::sqlite::canonical::transcript::load_transcript_event(
            &connection,
            "attempt_terminal",
            event.event_id.as_str(),
        )
        .unwrap_err();
        let expected = decode::<RuntimeEventEnvelope>("{}", "transcript event").unwrap_err();
        assert_eq!(format!("{error:?}"), format!("{expected:?}"));
    }
}

#[test]
fn physical_settlement_identity_lookup_preserves_decode_error_domain() {
    use crate::durable::sqlite::test_support::{own_normal_subagent, success_publication};
    use crate::runtime::identity::{AgentId, SubagentId};

    let store = store();
    let child = AgentId::new("decode-domain-child");
    let subagent = SubagentId::for_conversation(store.conversation_id(), 1);
    own_normal_subagent(&store, &subagent, &child);
    let (notice, report, terminal) = success_publication(&store, &subagent, &child, "result");
    let terminal_id = terminal.event_id.clone();
    store
        .accept_subagent_terminal(Some(notice), report, terminal)
        .unwrap();
    store
        .lock()
        .unwrap()
        .execute(
            "UPDATE events SET event_json='{}' WHERE event_id=?1",
            [terminal_id.as_str()],
        )
        .unwrap();
    let proof = envelope(
        store.conversation_id(),
        crate::runtime::subagent::physical_settlement_event_id(&subagent).as_str(),
        None,
        RuntimeEvent::SubagentPhysicalSettlementProven {
            subagent_id: subagent,
            child_agent_id: child,
        },
    );
    let error = store.append_event(proof).unwrap_err();
    let expected =
        decode::<RuntimeEventEnvelope>("{}", "physical settlement terminal").unwrap_err();
    assert_eq!(format!("{error:?}"), format!("{expected:?}"));
}

#[test]
fn identity_reserved_namespace_rejects_non_owner_sequences() {
    let store = store();
    for suffix in ["0", "2", "99", "9223372036854775807", "9223372036854775808"] {
        let id = format!("{}-event-{suffix}", store.conversation_id());
        assert!(
            matches!(append(&store, &id), Err(ConversationStoreError::Storage(_))),
            "{id}"
        );
        assert_eq!(store.event_high_watermark().unwrap(), 0);
    }
    assert_eq!(append(&store, "").unwrap().sequence, 1);
}

#[test]
fn identity_namespace_preserves_noncanonical_and_foreign_aliases() {
    let store = store();
    for suffix in ["01", "+1", "-1", "1.0", "1x", "1 ", "1\0hidden", "１２"] {
        let id = format!("{}-event-{suffix}", store.conversation_id());
        let event = append(&store, &id).unwrap();
        assert_eq!(event.event_id.as_str(), id);
        assert_eq!(
            find_event_sequence(&store.lock().unwrap(), &id).unwrap(),
            Some(event.sequence)
        );
    }
    for id in ["foreign-event-1", "producer🙂", "producer\0alias"] {
        let event = append(&store, id).unwrap();
        assert_eq!(event.event_id.as_str(), id);
        assert_eq!(
            find_event_sequence(&store.lock().unwrap(), id).unwrap(),
            Some(event.sequence)
        );
    }
}

#[test]
fn identity_constraints_reject_direct_updates_without_append_probes() {
    let store = store();
    let first = append(&store, "").unwrap();
    let second = append(&store, "producer-alias").unwrap();
    let connection = store.lock().unwrap();
    for (sequence, id) in [
        (second.sequence, first.event_id.as_str()),
        (first.sequence, second.event_id.as_str()),
    ] {
        let error = connection
            .execute(
                "UPDATE events SET event_id=?1 WHERE sequence=?2",
                params![id, sequence],
            )
            .unwrap_err();
        assert_eq!(
            error.sqlite_error_code(),
            Some(rusqlite::ErrorCode::ConstraintViolation)
        );
    }
    let distinct: u64 = connection
        .query_row("SELECT count(DISTINCT event_id) FROM events", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(distinct, 2);
}

#[test]
fn identity_parent_foreign_key_keeps_high_watermark_updates_bounded() {
    use rusqlite::StatementStatus;
    let store = store();
    let mut expected_steps = None;
    for history in [8, 64, 512] {
        while store.event_high_watermark().unwrap() < history {
            append(&store, "").unwrap();
        }
        let mut connection = store.lock().unwrap();
        let transaction = connection.transaction().unwrap();
        {
            let mut statement = transaction
                .prepare("UPDATE rustx_store SET next_event_sequence=?1 WHERE id=1")
                .unwrap();
            statement.execute([history + 1]).unwrap();
            assert_eq!(statement.get_status(StatementStatus::FullscanStep), 0);
            assert_eq!(statement.get_status(StatementStatus::Sort), 0);
            let steps = statement.get_status(StatementStatus::VmStep);
            assert_eq!(*expected_steps.get_or_insert(steps), steps);
            println!("identity high-water update H={history}: {steps} VM steps, no scan/sort");
        }
        transaction.rollback().unwrap();
    }
}

#[test]
fn identity_new_row_trigger_cannot_rewrite_identity_or_sequence() {
    for rewrite in [
        "event_id=(SELECT event_id FROM events WHERE sequence=1)",
        "sequence=3",
        "sequence=3,event_id=(SELECT event_id FROM events WHERE sequence=1)",
    ] {
        let store = store();
        let first = append(&store, "").unwrap();
        store
            .lock()
            .unwrap()
            .execute_batch(&format!(
                "CREATE TEMP TRIGGER rewrite_new AFTER INSERT ON events
             WHEN NEW.sequence=2 BEGIN
               UPDATE events SET {rewrite} WHERE sequence=NEW.sequence;
             END;"
            ))
            .unwrap();
        assert!(
            matches!(append(&store, ""), Err(ConversationStoreError::Storage(_))),
            "{rewrite}"
        );
        assert_eq!(store.event_high_watermark().unwrap(), first.sequence);
    }
}

#[test]
fn identity_insert_triggers_cannot_rewrite_old_or_new_rows() {
    for schema in ["", "TEMP "] {
        for timing in ["BEFORE", "AFTER"] {
            for rewrite in [
                "UPDATE events SET event_id=NEW.event_id WHERE sequence=1;",
                "UPDATE events SET event_id=(SELECT event_id FROM events WHERE sequence=1) WHERE sequence=NEW.sequence;",
                "UPDATE events SET sequence=3 WHERE sequence=NEW.sequence;",
                "UPDATE events SET sequence=3,event_id=(SELECT event_id FROM events WHERE sequence=1) WHERE sequence=NEW.sequence;",
            ] {
                if timing == "BEFORE" && !rewrite.contains("WHERE sequence=1;") {
                    continue;
                }
                let store = store();
                let first = append(&store, "").unwrap();
                let lifecycles: u64 = store
                    .lock()
                    .unwrap()
                    .query_row("SELECT count(*) FROM lifecycle_state", [], |row| row.get(0))
                    .unwrap();
                store
                    .lock()
                    .unwrap()
                    .execute_batch(&format!(
                        "CREATE TABLE identity_effects(value INTEGER);
                     CREATE {schema}TRIGGER rewrite_identity {timing} INSERT ON events
                     WHEN NEW.sequence=2 BEGIN
                       INSERT INTO identity_effects VALUES(NEW.sequence);
                       {rewrite}
                     END;"
                    ))
                    .unwrap();
                assert!(
                    matches!(append(&store, ""), Err(ConversationStoreError::Storage(_))),
                    "{schema}{timing}: {rewrite}"
                );
                assert_eq!(store.event_high_watermark().unwrap(), first.sequence);
                let connection = store.lock().unwrap();
                let state: (u64, u64, u64, String) = connection
                    .query_row(
                        "SELECT (SELECT count(*) FROM events),
                            (SELECT count(*) FROM identity_effects),
                            (SELECT count(*) FROM lifecycle_state),
                            (SELECT event_id FROM events WHERE sequence=1)",
                        [],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                    )
                    .unwrap();
                assert_eq!(state, (1, 0, lifecycles, first.event_id.to_string()));
                connection
                    .execute_batch("DROP TRIGGER rewrite_identity")
                    .unwrap();
                drop(connection);
                assert_eq!(append(&store, "").unwrap().sequence, 2);
            }
        }
    }
}

#[test]
fn identity_old_alias_cannot_reserve_default_from_insert_trigger() {
    for timing in ["BEFORE", "AFTER"] {
        let store = store();
        let first = append(&store, "prior-alias").unwrap();
        store
            .lock()
            .unwrap()
            .execute_batch(&format!(
                "CREATE TEMP TRIGGER reserve_default {timing} INSERT ON events
             WHEN NEW.sequence=2 BEGIN
               UPDATE events SET event_id=NEW.event_id WHERE sequence=1;
             END;"
            ))
            .unwrap();
        assert!(matches!(
            append(&store, ""),
            Err(ConversationStoreError::Storage(_))
        ));
        assert_eq!(store.event_high_watermark().unwrap(), first.sequence);
        assert_eq!(store.read_events(None, 8).unwrap().events, vec![first]);
    }
}

#[test]
fn identity_observation_update_contains_trigger_rewrites() {
    for schema in ["", "TEMP "] {
        for timing in ["BEFORE", "AFTER"] {
            let store = store();
            let first = append(&store, "prior-alias").unwrap();
            let rewrites = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let observed = rewrites.clone();
            let connection = store.lock().unwrap();
            connection
                .create_scalar_function(
                    "observe_identity_rewrite",
                    0,
                    rusqlite::functions::FunctionFlags::SQLITE_UTF8
                        | rusqlite::functions::FunctionFlags::SQLITE_INNOCUOUS,
                    move |_| {
                        observed.fetch_add(1, Ordering::SeqCst);
                        Ok(0)
                    },
                )
                .unwrap();
            connection.execute_batch(&format!(
                "CREATE {schema}TRIGGER rewrite_observation {timing} UPDATE OF observation_readiness_missing ON events
                 WHEN NEW.sequence=2 BEGIN
                   SELECT observe_identity_rewrite();
                   UPDATE events SET event_id=NEW.event_id WHERE sequence=1;
                 END;"
            )).unwrap();
            drop(connection);
            let second = append(&store, "").unwrap();
            assert!(
                rewrites.load(Ordering::SeqCst) > 0,
                "the observation UPDATE must execute"
            );
            let connection = store.lock().unwrap();
            let state: (String, String, bool) = connection
                .query_row(
                    "SELECT (SELECT event_id FROM events WHERE sequence=1),
                        (SELECT event_id FROM events WHERE sequence=2),
                        (SELECT observation_readiness_missing FROM events WHERE sequence=2)",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .unwrap();
            assert_eq!(
                state,
                (
                    first.event_id.to_string(),
                    second.event_id.to_string(),
                    true
                )
            );
            drop(connection);
            assert_eq!(store.event_high_watermark().unwrap(), 2);
        }
    }
}

#[test]
fn identity_schema_rejects_tampered_check_and_alias_index() {
    for replacement in [
        "CHECK(1)",
        "CHECK(event_id = conversation_id || '-event-' || sequence OR 1)",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("identity.sqlite");
        let id = ConversationId::generate();
        let store = SqliteConversationStore::open(id.clone(), &path).unwrap();
        {
            let connection = store.lock().unwrap();
            let ddl: String = connection
                .query_row(
                    "SELECT sql FROM sqlite_schema WHERE name='events'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            let start = ddl
                .find("CONSTRAINT events_identity ")
                .expect("identity CHECK required");
            let mut changed = format!("{}CONSTRAINT events_identity {replacement})", &ddl[..start]);
            if replacement == "CHECK(1)" {
                changed.push_str(" --");
                changed.push_str(&ddl[start..]);
            }
            connection
                .execute_batch("PRAGMA writable_schema=ON")
                .unwrap();
            connection
                .execute(
                    "UPDATE sqlite_schema SET sql=?1 WHERE name='events'",
                    [changed],
                )
                .unwrap();
            connection
                .execute_batch("PRAGMA writable_schema=OFF")
                .unwrap();
        }
        drop(store);
        assert!(matches!(
            SqliteConversationStore::open(id, &path),
            Err(ConversationStoreError::IncompatibleSchema(_))
        ));
    }
    for ddl in [
        "CREATE INDEX events_alias_idx ON events(event_id) WHERE event_id != conversation_id || '-event-' || sequence",
        "CREATE UNIQUE INDEX events_alias_idx ON events(event_id) WHERE sequence<0",
        "CREATE UNIQUE INDEX events_alias_idx ON events(event_id COLLATE NOCASE) WHERE event_id != conversation_id || '-event-' || sequence",
    ] {
        let store = store();
        let connection = store.lock().unwrap();
        connection
            .execute_batch(&format!("DROP INDEX events_alias_idx; {ddl}"))
            .unwrap();
        assert!(matches!(
            super::super::schema::shape::verify_schema_shape(&connection),
            Err(ConversationStoreError::IncompatibleSchema(_))
        ));
    }
    let store = store();
    let connection = store.lock().unwrap();
    connection.execute_batch(
        "DROP INDEX rustx_store_conversation_idx;
         CREATE UNIQUE INDEX rustx_store_conversation_idx ON rustx_store(conversation_id COLLATE NOCASE)"
    ).unwrap();
    assert!(matches!(
        super::super::schema::shape::verify_schema_shape(&connection),
        Err(ConversationStoreError::IncompatibleSchema(_))
    ));
}

#[test]
fn identity_triggers_cannot_escape_via_conversation_rewrites() {
    for timing in ["BEFORE", "AFTER"] {
        for target in ["1", "NEW.sequence"] {
            let store = store();
            let first = append(&store, "prior-alias").unwrap();
            store.lock().unwrap().execute_batch(&format!(
                "CREATE TEMP TRIGGER move_conversation {timing} INSERT ON events
                 WHEN NEW.sequence=2 BEGIN
                   UPDATE events SET conversation_id='foreign',event_id=NEW.event_id WHERE sequence={target};
                 END;"
            )).unwrap();
            if timing == "BEFORE" && target == "NEW.sequence" {
                continue;
            }
            assert!(matches!(
                append(&store, ""),
                Err(ConversationStoreError::Storage(_))
            ));
            assert_eq!(store.event_high_watermark().unwrap(), first.sequence);
            assert_eq!(store.read_events(None, 8).unwrap().events, vec![first]);
        }
    }
}
