//! Scope pruning must preserve nullable tool occurrence identity and history cuts.
use super::*;
use crate::durable::presentation::{FactQuery, FactScope};

#[test]
fn partial_tool_scope_keeps_null_owners_order_and_historical_cut() {
    let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    let connection = store.lock().unwrap();
    for (sequence, call, attempt, turn) in [
        (1, None, None, None::<&str>),
        (2, Some("call"), None, None),
        (3, Some("call"), None, None),
        (4, Some("call"), Some("different"), None),
        (5, Some("other"), None, None),
        (6, Some("call"), None, None),
    ] {
        let value = serde_json::json!({"sequence":sequence,"event":{"type":"tool_execution_started","tool_call_id":call}});
        connection.execute("INSERT INTO events(sequence,event_id,schema_version,conversation_id,event_timestamp,attempt_id,turn_id,event_json) VALUES(?1,?2,?3,?4,'time',?5,?6,?7)",params![sequence,format!("event-{sequence}"),EVENT_SCHEMA_VERSION,store.conversation_id.as_str(),attempt,turn,value.to_string()]).unwrap();
    }
    for ascending in [false, true] {
        let query = FactQuery {
            scope: FactScope::ToolCall {
                call_id: "call".into(),
                attempt: None,
                turn: None,
            },
            kinds: vec!["tool_execution_started"],
            before: None,
            after: 0,
            ascending,
            through: 5,
            limit: 32,
        };
        let (sql, values) = presentation_statements(&query).unwrap().remove(0);
        let mut statement = connection.prepare(&sql).unwrap();
        let results = statement
            .query_map(rusqlite::params_from_iter(values), |row| {
                row.get::<_, String>(0)
            })
            .unwrap()
            .map(|row| {
                serde_json::from_str::<serde_json::Value>(&row.unwrap()).unwrap()["sequence"]
                    .as_u64()
                    .unwrap()
            })
            .collect::<Vec<_>>();
        assert_eq!(results, if ascending { vec![2, 3] } else { vec![3, 2] });
        assert_eq!(
            statement.get_status(rusqlite::StatementStatus::FullscanStep),
            0
        );
        assert!(statement.get_status(rusqlite::StatementStatus::VmStep) < 200);
    }
}

#[test]
fn scoped_indexes_require_partial_predicates_and_previous_schema_is_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("scope.sqlite");
    let id = ConversationId::generate();
    let store = SqliteConversationStore::open(id.clone(), &path).unwrap();
    {
        let connection = store.lock().unwrap();
        for (name, _) in PRESENTATION_INDEXES {
            let partial: bool = connection
                .query_row(
                    "SELECT partial FROM pragma_index_list('events') WHERE name=?1",
                    [name],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(
                partial,
                !matches!(*name, "events_kind_idx" | "events_attempt_kind_idx"),
                "{name}"
            );
        }
        let duplicate: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='events_sequence_idx')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!duplicate);
        for name in [
            "events_workflow_coordinates_idx",
            "events_observation_tool_idx",
        ] {
            let original: String = connection
                .query_row(
                    "SELECT sql FROM sqlite_schema WHERE name=?1",
                    [name],
                    |row| row.get(0),
                )
                .unwrap();
            let unfiltered = original.split(" WHERE ").next().unwrap();
            connection
                .execute_batch(&format!(
                    "SAVEPOINT invalid; DROP INDEX {name}; {unfiltered};"
                ))
                .unwrap();
            assert!(matches!(
                verify_schema_shape(&connection),
                Err(ConversationStoreError::IncompatibleSchema(_))
            ));
            connection
                .execute_batch("ROLLBACK TO invalid; RELEASE invalid")
                .unwrap();
        }
        connection
            .execute("UPDATE rustx_store SET schema_version=71", [])
            .unwrap();
    }
    drop(store);
    assert!(matches!(
        SqliteConversationStore::open_existing(id, &path),
        Err(ConversationStoreError::SchemaVersionMismatch {
            stored: 71,
            expected: crate::durable::sqlite::SQLITE_SCHEMA_VERSION
        })
    ));
}

#[test]
fn primary_and_unique_keys_replace_duplicate_btrees_for_bounded_seeks() {
    let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    let connection = store.lock().unwrap();
    for retired in [
        "pending_inbound_sequence_idx",
        "message_ledger_id_idx",
        "transcript_order_reference_idx",
        "surface_ops_revision_idx",
        "contribution_emission_heads_lookup_idx",
        "publication_frames_stream_idx",
        "observation_headers_transcript_idx",
        "events_trace_holes_idx",
        "events_observation_trace_holes_idx",
        "events_observation_statistics_holes_idx",
        "events_execution_holes_idx",
        "events_observation_turn_holes_idx",
        "events_observation_member_holes_sequence_idx",
    ] {
        assert!(
            !connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='index' AND name=?1)",
                    [retired],
                    |row| row.get::<_, bool>(0)
                )
                .unwrap(),
            "{retired}"
        );
    }
    for (sql, bound) in [
        (
            "SELECT sequence FROM pending_inbound WHERE sequence>7 ORDER BY sequence LIMIT 32",
            "INTEGER PRIMARY KEY",
        ),
        (
            "SELECT position FROM message_ledger WHERE message_id='message'",
            "message_id=?",
        ),
        (
            "SELECT position FROM transcript_order WHERE reference_kind='message' AND reference_id='message'",
            "reference_kind=? AND reference_id=?",
        ),
        (
            "SELECT revision FROM surface_ops WHERE revision>7 ORDER BY revision LIMIT 32",
            "INTEGER PRIMARY KEY",
        ),
        (
            "SELECT sequence FROM publication_frames WHERE stream_id='stream' AND sequence>7 ORDER BY sequence LIMIT 32",
            "stream_id=? AND sequence>?",
        ),
        (
            "SELECT event_sequence FROM contribution_emission_heads WHERE producer='producer' AND semantic_key='key'",
            "producer=? AND semantic_key=?",
        ),
        (
            "SELECT header_json FROM observation_headers WHERE transcript_position=7",
            "transcript_position=?",
        ),
    ] {
        let plan = connection
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .unwrap()
            .query_map([], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(
            plan.iter()
                .any(|line| line.contains("SEARCH") && line.contains(bound)),
            "{sql}: {plan:?}"
        );
        assert!(
            !plan
                .iter()
                .any(|line| line.contains("SCAN") || line.contains("TEMP B-TREE")),
            "{sql}: {plan:?}"
        );
    }
    // This composite key still backs the inherited membership foreign key;
    // a UNIQUE(message_id) key alone cannot replace its exact column pair.
    super::verify_unique_columns(&connection, "message_ledger", &["position", "message_id"])
        .unwrap();
    super::observation::verify_schema(&connection).unwrap();
}
