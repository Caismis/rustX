use super::*;
use crate::durable::sqlite::{SqliteConversationStore, params};
use crate::events::types::EVENT_SCHEMA_VERSION;
use crate::runtime::identity::ConversationId;

const CASES: &[(&str, &str, &str, &str)] = &[
    (
        "aggregate",
        "1,3,5,7",
        "observation_trace_fact_required",
        "observation_trace_fact",
    ),
    (
        "aggregate",
        "2,3,6,7",
        "observation_trace_required",
        "observation_trace_total",
    ),
    (
        "aggregate",
        "4,5,6,7",
        "observation_statistics_required",
        "observation_statistics",
    ),
    (
        "owner",
        "1,3,5,7",
        "observation_execution_required",
        "observation_execution",
    ),
    (
        "owner",
        "2,3,6,7",
        "observation_turn_required",
        "observation_turn_header",
    ),
    (
        "owner",
        "4,5,6,7",
        "observation_members_required",
        "observation_members",
    ),
];

fn insert(connection: &Connection, sequence: u64, aggregate: u8, owner: u8) {
    connection.execute_cached(
        "INSERT INTO events(sequence,event_id,schema_version,conversation_id,event_timestamp,event_json,observation_trace_fact_required,observation_trace_required,observation_statistics_required,observation_execution_required,observation_turn_required,observation_members_required,observation_trace_fact,observation_trace_total,observation_statistics,observation_execution,observation_turn_header,observation_members) VALUES(?1,?2,?3,'conversation','time','{}',1,1,1,1,1,1,CASE WHEN (?4&1)=0 THEN x'00' END,CASE WHEN (?4&2)=0 THEN 0 END,CASE WHEN (?4&4)=0 THEN x'00' END,CASE WHEN (?5&1)=0 THEN x'00' END,CASE WHEN (?5&2)=0 THEN x'00' END,CASE WHEN (?5&4)=0 THEN x'00' END)",
        params![sequence, format!("event-{sequence}"), EVENT_SCHEMA_VERSION, aggregate, owner],
    ).unwrap();
}

use crate::durable::sqlite::statements::FixedStatements;

fn sequences(connection: &Connection, sql: &str, after: u64, through: u64) -> Vec<u64> {
    connection
        .prepare(sql)
        .unwrap()
        .query_map(params![after, through], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

#[test]
fn masks_preserve_each_native_hole_predicate_at_every_historical_cut() {
    let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    let connection = store.lock().unwrap();
    for aggregate in 0..8_u8 {
        for owner in 0..8_u8 {
            let sequence = u64::from(aggregate) * 8 + u64::from(owner) + 1;
            insert(&connection, sequence, aggregate, owner);
            let actual: (u8, u8) = connection.query_row("SELECT observation_aggregate_holes,observation_owner_holes FROM events WHERE sequence=?1", [sequence], |row| Ok((row.get(0)?, row.get(1)?))).unwrap();
            assert_eq!(actual, (aggregate, owner));
        }
    }
    for &(group, masks, required, value) in CASES {
        let indexed = format!(
            "SELECT sequence FROM events INDEXED BY events_{group}_holes_idx WHERE observation_{group}_holes>0 AND observation_{group}_holes IN ({masks}) AND sequence>?1 AND sequence<=?2 ORDER BY sequence"
        );
        let original = format!(
            "SELECT sequence FROM events NOT INDEXED WHERE {required}=1 AND {value} IS NULL AND sequence>?1 AND sequence<=?2 ORDER BY sequence"
        );
        for through in 0..=65 {
            for after in [0, through / 2] {
                assert_eq!(
                    sequences(&connection, &indexed, after, through),
                    sequences(&connection, &original, after, through),
                    "{value}: ({after},{through}]"
                );
            }
        }
    }
}

#[test]
fn optional_failure_updates_and_rollback_recalculate_holes_without_a_mirror() {
    let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    let connection = store.lock().unwrap();
    insert(&connection, 1, 0, 0);
    for &(group, masks, required, value) in CASES {
        let count = format!(
            "SELECT COUNT(*) FROM events INDEXED BY events_{group}_holes_idx WHERE observation_{group}_holes>0 AND observation_{group}_holes IN ({masks}) AND sequence=1"
        );
        connection.execute_batch("SAVEPOINT failed_layout").unwrap();
        connection
            .execute(
                &format!("UPDATE events SET {value}=NULL WHERE sequence=1"),
                [],
            )
            .unwrap();
        assert_eq!(
            connection
                .query_row(&count, [], |row| row.get::<_, u64>(0))
                .unwrap(),
            1,
            "{value}"
        );
        assert!(
            connection
                .execute(
                    &format!("UPDATE events SET observation_{group}_holes=0 WHERE sequence=1"),
                    []
                )
                .is_err()
        );
        connection
            .execute(
                &format!("UPDATE events SET {required}=0 WHERE sequence=1"),
                [],
            )
            .unwrap();
        assert_eq!(
            connection
                .query_row(&count, [], |row| row.get::<_, u64>(0))
                .unwrap(),
            0
        );
        connection
            .execute_batch("ROLLBACK TO failed_layout; RELEASE failed_layout")
            .unwrap();
        assert_eq!(
            connection
                .query_row(&count, [], |row| row.get::<_, u64>(0))
                .unwrap(),
            0
        );
    }
    connection
        .execute("DELETE FROM events WHERE sequence=1", [])
        .unwrap();
    for &(index, _) in INDEXES {
        let count: u64 = connection
            .query_row(
                &format!(
                    "SELECT COUNT(*) FROM events INDEXED BY {index} WHERE observation_{}_holes>0",
                    if index.contains("aggregate") {
                        "aggregate"
                    } else {
                        "owner"
                    }
                ),
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }
}

#[test]
fn unrelated_hole_history_does_not_expand_a_bounded_negative_probe() {
    let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    let connection = store.lock().unwrap();
    for sequence in 1..=4096 {
        insert(&connection, sequence, 2, 2);
    }
    insert(&connection, 4097, 1, 1);
    for group in ["aggregate", "owner"] {
        let sql = format!(
            "SELECT EXISTS(SELECT 1 FROM events INDEXED BY events_{group}_holes_idx WHERE observation_{group}_holes>0 AND observation_{group}_holes IN (1,3,5,7) AND sequence>?1 AND sequence<=?2)"
        );
        let plan = connection
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .unwrap()
            .query_map(params![0, 4096], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(
            plan.iter().any(|line| line.contains(&format!(
                "SEARCH events USING INDEX events_{group}_holes_idx"
            )) && line.contains("sequence>?")
                && line.contains("sequence<?")),
            "{plan:?}"
        );
        for (after, through, expected) in [(0, 4096, false), (0, 4097, true), (4097, 4097, false)] {
            let mut statement = connection.prepare(&sql).unwrap();
            let actual: bool = statement
                .query_row(params![after, through], |row| row.get(0))
                .unwrap();
            assert_eq!(actual, expected);
            assert_eq!(
                statement.get_status(rusqlite::StatementStatus::FullscanStep),
                0
            );
            assert!(statement.get_status(rusqlite::StatementStatus::VmStep) < 200);
        }
    }
}

#[test]
fn changed_hole_schema_is_rejected_instead_of_silently_widening_queries() {
    let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    let connection = store.lock().unwrap();
    verify_schema(&connection).unwrap();
    for &(name, definition) in INDEXES {
        connection
            .execute_batch(&format!(
                "SAVEPOINT changed_index; DROP INDEX {name}; {}",
                definition.split(" WHERE ").next().unwrap()
            ))
            .unwrap();
        assert!(matches!(
            verify_schema(&connection),
            Err(ConversationStoreError::IncompatibleSchema(_))
        ));
        connection
            .execute_batch("ROLLBACK TO changed_index; RELEASE changed_index")
            .unwrap();
    }
    // Schema verification authenticates the generated expression, not only its
    // name/type. A retained but incorrectly defined bit is not a usable layout.
    connection
        .execute_batch("PRAGMA writable_schema=ON; SAVEPOINT changed_column")
        .unwrap();
    connection.execute("UPDATE sqlite_schema SET sql=replace(sql,'THEN 4 ELSE 0','THEN 2 ELSE 0') WHERE name='events'", []).unwrap();
    assert!(matches!(
        verify_schema(&connection),
        Err(ConversationStoreError::IncompatibleSchema(_))
    ));
    connection
        .execute_batch(
            "ROLLBACK TO changed_column; RELEASE changed_column; PRAGMA writable_schema=OFF",
        )
        .unwrap();
    verify_schema(&connection).unwrap();
}
