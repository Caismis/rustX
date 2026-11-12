use super::*;
use rusqlite::{Connection, StatementStatus, params};

#[test]
fn sql_and_parameter_keys_preserve_all_kinds_and_disjoint_sql_types() {
    let connection = Connection::open_in_memory().unwrap();
    let expression = sql!("?1");
    let query = format!("SELECT {expression}");
    for kind in crate::durable::observation::TraceFactKind::ALL {
        let actual: Value = connection
            .query_row(&query, [kind.as_str()], |row| row.get(0))
            .unwrap();
        assert_eq!(actual, parameter(kind.as_str()));
    }
    for text in [
        "",
        "attempt_started\0",
        "ATTEMPT_STARTED",
        "foreign🙂",
        "unknown'kind",
    ] {
        let actual: Value = connection
            .query_row(&query, [text], |row| row.get(0))
            .unwrap();
        assert_eq!(actual, Value::Text(text.into()));
    }
    for value in [
        Value::Null,
        Value::Integer(2),
        Value::Real(2.5),
        Value::Blob(vec![0, 2]),
        Value::Blob(vec![1, 0, 2]),
    ] {
        let actual: Value = connection
            .query_row(&query, [&value], |row| row.get(0))
            .unwrap();
        let expected = if let Value::Blob(bytes) = value {
            Value::Blob([vec![1], bytes].concat())
        } else {
            value
        };
        assert_eq!(actual, expected);
        assert_ne!(actual, parameter("attempt_started"));
    }
}

#[test]
fn compact_native_and_observation_keys_keep_independent_bounded_sequence_seeks() {
    let mut connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch(&format!(
            "CREATE TABLE events(sequence INTEGER PRIMARY KEY,event_json TEXT,observation_kind,event_kind GENERATED ALWAYS AS (json_extract(event_json,'$.event.type')) STORED);\n\
         CREATE INDEX native_kind ON events{NATIVE};\n\
         CREATE INDEX observation_kind ON events{OBSERVATION};"
        ))
        .unwrap();
    let transaction = connection.transaction().unwrap();
    for sequence in 1..=8192 {
        let kind = if sequence % 1024 == 0 {
            "attempt_started"
        } else {
            "model_request_completed"
        };
        let json = serde_json::json!({"event":{"type":kind}}).to_string();
        transaction
            .execute(
                "INSERT INTO events VALUES(?1,?2,?3)",
                params![sequence, json, kind],
            )
            .unwrap();
    }
    transaction.commit().unwrap();
    for (index, expression) in [("native_kind", NATIVE), ("observation_kind", OBSERVATION)] {
        for direction in ["ASC", "DESC"] {
            let sql = format!(
                "SELECT sequence FROM events INDEXED BY {index} WHERE {expression}=?1 AND sequence>?2 AND sequence<=?3 ORDER BY sequence {direction} LIMIT 3"
            );
            let mut statement = connection.prepare(&sql).unwrap();
            let rows = statement
                .query_map(params![parameter("attempt_started"), 2048, 6144], |row| {
                    row.get::<_, u64>(0)
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            assert_eq!(
                rows,
                if direction == "ASC" {
                    vec![3072, 4096, 5120]
                } else {
                    vec![6144, 5120, 4096]
                }
            );
            assert_eq!(statement.get_status(StatementStatus::FullscanStep), 0);
            assert_eq!(statement.get_status(StatementStatus::Sort), 0);
            assert!(statement.get_status(StatementStatus::VmStep) < 100);
        }
    }
    connection.execute("UPDATE events SET event_json=json_set(event_json,'$.event.type','turn_started') WHERE sequence=3072",[]).unwrap();
    let native: bool = connection.query_row(&format!("SELECT EXISTS(SELECT 1 FROM events INDEXED BY native_kind WHERE {NATIVE}=?1 AND sequence=3072)"),[parameter("attempt_started")],|row|row.get(0)).unwrap();
    let observation: bool = connection.query_row(&format!("SELECT EXISTS(SELECT 1 FROM events INDEXED BY observation_kind WHERE {OBSERVATION}=?1 AND sequence=3072)"),[parameter("attempt_started")],|row|row.get(0)).unwrap();
    assert!(!native);
    assert!(observation);
}
