use super::*;
use crate::durable::sqlite::{SqliteConversationStore, params};
use crate::events::types::EVENT_SCHEMA_VERSION;
use crate::runtime::identity::ConversationId;
use rusqlite::{StatementStatus, types::Value};

const CASES: &[(&str, &str, bool, usize)] = &[
    (STATISTICS, "observation_statistics", false, 0),
    (TRACE_TOTAL, "observation_trace_total", false, 1),
    (EXECUTION, "observation_execution", true, 0),
    (MEMBERS, "observation_members", true, 0),
];

fn insert(connection: &Connection, sequence: u64, mask: u8, attempt: Option<&str>) {
    connection.execute(
        "INSERT INTO events(sequence,event_id,schema_version,conversation_id,event_timestamp,event_json,attempt_id,observation_trace_total,observation_statistics,observation_execution,observation_members) VALUES(?1,?2,?3,'conversation','time','{}',?4,CASE WHEN (?5&1)>0 THEN ?1 END,CASE WHEN (?5&2)>0 THEN CAST(?2 AS BLOB) END,CASE WHEN (?5&4)>0 THEN CAST(?2 AS BLOB) END,CASE WHEN (?5&8)>0 THEN CAST(?2 AS BLOB) END)",
        params![sequence, format!("event-{sequence}"), EVENT_SCHEMA_VERSION, attempt, mask],
    ).unwrap();
}

fn selected(
    connection: &Connection,
    sql: &str,
    attempt: Option<&str>,
    through: u64,
    column: usize,
) -> Option<Value> {
    let read = |row: &rusqlite::Row<'_>| row.get::<_, Option<Value>>(column);
    if let Some(attempt) = attempt {
        connection.query_row(sql, params![attempt, through], read)
    } else {
        connection.query_row(sql, [through], read)
    }
    .optional()
    .unwrap()
    .flatten()
}

fn compare_all_cuts(connection: &Connection, last: u64) {
    for &(sql, column, scoped, result_column) in CASES {
        let predicate = if scoped {
            "attempt_id=?1 AND sequence<=?2"
        } else {
            "sequence<=?1"
        };
        let oracle = format!(
            "SELECT {column} FROM events NOT INDEXED WHERE {column} IS NOT NULL AND {predicate} ORDER BY sequence DESC LIMIT 1"
        );
        for attempt in ["first", "second", "absent"] {
            let attempt = scoped.then_some(attempt);
            for through in 0..=last {
                assert_eq!(
                    selected(connection, sql, attempt, through, result_column),
                    selected(connection, &oracle, attempt, through, 0),
                    "{column} at {through}, {attempt:?}"
                );
            }
        }
    }
}

#[test]
fn shared_head_indexes_preserve_all_optional_cells_attempts_and_historical_cuts() {
    let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    let connection = store.lock().unwrap();
    for mask in 0..16_u8 {
        insert(&connection, u64::from(mask) * 2 + 1, mask, Some("first"));
        insert(&connection, u64::from(mask) * 2 + 2, mask, Some("second"));
    }
    insert(&connection, 33, 15, None);
    compare_all_cuts(&connection, 34);
    for name in [
        "events_observation_statistics_idx",
        "events_observation_trace_total_idx",
        "events_observation_execution_idx",
        "events_observation_members_idx",
    ] {
        let exists: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name=?1)",
                [name],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!exists, "obsolete B-tree {name}");
    }
}

#[test]
fn shared_head_indexes_follow_null_mutations_deletion_and_savepoint_rollback() {
    let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    let connection = store.lock().unwrap();
    insert(&connection, 1, 15, Some("first"));
    insert(&connection, 2, 15, Some("first"));
    insert(&connection, 3, 15, Some("second"));
    connection.execute_batch("SAVEPOINT mutate_heads").unwrap();
    connection.execute_batch("UPDATE events SET observation_statistics=NULL,observation_members=NULL WHERE sequence=2; UPDATE events SET observation_execution=NULL,observation_trace_total=NULL WHERE sequence=3").unwrap();
    compare_all_cuts(&connection, 4);
    connection
        .execute("DELETE FROM events WHERE sequence=1", [])
        .unwrap();
    compare_all_cuts(&connection, 4);
    connection
        .execute_batch("ROLLBACK TO mutate_heads; RELEASE mutate_heads")
        .unwrap();
    compare_all_cuts(&connection, 4);
    let heads: (u8, u8) = connection.query_row("SELECT observation_aggregate_heads,observation_attempt_heads FROM events WHERE sequence=2", [], |row| Ok((row.get(0)?, row.get(1)?))).unwrap();
    assert_eq!(heads, (3, 3));
}

#[test]
fn shared_head_seeks_stay_bounded_with_unrelated_and_missing_domain_history() {
    let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    let connection = store.lock().unwrap();
    // One domain in each index is absent, but thousands of other-domain rows
    // exist. Neither the negative nor the positive lookup may walk that prefix.
    for sequence in 1..=4096 {
        insert(&connection, sequence, 1 | 4, Some("first"));
    }
    insert(&connection, 4097, 15, Some("second"));
    for &(sql, _, scoped, column) in CASES {
        for through in [0_u64, 4096, 4097] {
            let mut statement = connection.prepare(sql).unwrap();
            let read = |row: &rusqlite::Row<'_>| row.get::<_, Option<Value>>(column);
            let _ = if scoped {
                statement.query_row(params!["first", through], read)
            } else {
                statement.query_row([through], read)
            }
            .optional()
            .unwrap();
            assert_eq!(statement.get_status(StatementStatus::FullscanStep), 0);
            assert_eq!(statement.get_status(StatementStatus::Sort), 0);
            assert!(statement.get_status(StatementStatus::VmStep) < 180);
        }
    }
}

#[test]
fn changed_shared_head_index_definitions_are_rejected() {
    let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    let connection = store.lock().unwrap();
    for &(name, definition) in INDEXES {
        connection
            .execute_batch(&format!(
                "DROP INDEX {name}; CREATE INDEX {name} ON events(sequence)"
            ))
            .unwrap();
        assert!(matches!(
            verify_schema(&connection),
            Err(ConversationStoreError::IncompatibleSchema(_))
        ));
        connection
            .execute_batch(&format!("DROP INDEX {name}; {definition}"))
            .unwrap();
        verify_schema(&connection).unwrap();
    }
    connection.execute_batch("PRAGMA writable_schema=ON; UPDATE sqlite_schema SET sql=replace(sql,'(observation_trace_total IS NOT NULL)+2*(observation_statistics IS NOT NULL)','0') WHERE name='events'; PRAGMA writable_schema=OFF").unwrap();
    assert!(matches!(
        verify_schema(&connection),
        Err(ConversationStoreError::IncompatibleSchema(_))
    ));
}
