//! Scope pruning must preserve nullable tool occurrence identity and history cuts.
use super::*;
use crate::durable::presentation::{FactQuery, FactScope};
use crate::durable::sqlite::schema::shape::verify_schema_shape;
use crate::durable::sqlite::schema::{PRESENTATION_INDEXES, presentation_statements};
use crate::events::EVENT_SCHEMA_VERSION;

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
#[allow(clippy::too_many_lines)] // One shared fixture compares both layouts, cuts, plans and pages.
fn agent_request_scope_keeps_bounded_seeks_and_excludes_unrelated_index_rows() {
    let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    let connection = store.lock().unwrap();
    connection
        .execute_batch(
            "CREATE INDEX full_reference ON events(
            json_extract(event_json, '$.event.type'), json_extract(event_json, '$.event.owner.type')
        );",
        )
        .unwrap();
    {
        let mut insert = connection.prepare(
            "INSERT INTO events(sequence,event_id,schema_version,conversation_id,event_timestamp,event_json)
             VALUES(?1,?2,?3,?4,'time',?5)"
        ).unwrap();
        for sequence in 1..=4096 {
            let owner = match sequence % 16 {
                0 => serde_json::json!({"type":"agent"}),
                1..=7 => serde_json::json!({"type":"auxiliary"}),
                _ => serde_json::Value::Null,
            };
            let kind = if sequence % 32 < 16 {
                "model_request_started"
            } else {
                "model_request_completed"
            };
            let value =
                serde_json::json!({"sequence":sequence,"event":{"type":kind,"owner":owner}});
            insert
                .execute(params![
                    sequence,
                    format!("event-{sequence}"),
                    EVENT_SCHEMA_VERSION,
                    store.conversation_id.as_str(),
                    value.to_string(),
                ])
                .unwrap();
        }
    }

    let mut measurements = Vec::new();
    for ascending in [true, false] {
        let query = FactQuery {
            scope: FactScope::AgentRequest,
            kinds: vec!["model_request_started", "model_request_completed"],
            before: Some(2064),
            after: 1951,
            ascending,
            through: 2079,
            limit: 3,
        };
        let statements = presentation_statements(&query).unwrap();
        for ((sql, values), kind) in statements.into_iter().zip(query.kinds) {
            let read = |sql: &str| {
                let mut statement = connection.prepare(sql).unwrap();
                let results = statement.query_map(rusqlite::params_from_iter(&values), |row| {
                    row.get::<_, String>(0)
                }).unwrap().map(|row| {
                    serde_json::from_str::<serde_json::Value>(&row.unwrap()).unwrap()["sequence"].as_u64().unwrap()
                }).collect::<Vec<_>>();
                assert_eq!(
                    statement.get_status(rusqlite::StatementStatus::FullscanStep),
                    0
                );
                assert_eq!(statement.get_status(rusqlite::StatementStatus::Sort), 0);
                results
            };
            let old = sql.replace("events_primary_request_kind_idx", "full_reference");
            let mut expected = (1952..2064)
                .filter(|sequence| {
                    sequence % 16 == 0 && (sequence % 32 == 0) == (kind == "model_request_started")
                })
                .collect::<Vec<_>>();
            if !ascending {
                expected.reverse();
            }
            expected.truncate(3);
            assert_eq!(read(&sql), expected);
            assert_eq!(read(&old), expected);
            for (name, sql) in [
                ("events_primary_request_kind_idx", sql),
                ("full_reference", old),
            ] {
                let plan = connection
                    .query_row(
                        &format!("EXPLAIN QUERY PLAN {sql}"),
                        rusqlite::params_from_iter(&values),
                        |row| row.get::<_, String>(3),
                    )
                    .unwrap();
                assert!(
                    plan.contains("SEARCH events") && plan.contains(name),
                    "{plan}"
                );
                assert!(plan.contains("rowid>? AND rowid<?"), "{plan}");
                measurements.push(plan);
            }
        }
    }
    let index = |name| {
        connection
            .query_row(
                "SELECT count(*), sum(ncell) FROM dbstat WHERE name=?1",
                [name],
                |row| Ok((row.get::<_, usize>(0)?, row.get::<_, usize>(1)?)),
            )
            .unwrap()
    };
    let (old_pages, old_cells) = index("full_reference");
    let (new_pages, new_cells) = index("events_primary_request_kind_idx");
    assert_eq!((old_cells, new_cells), (4096, 256));
    assert!(new_pages < old_pages);
    println!("Agent index pages {old_pages} -> {new_pages}, cells {old_cells} -> {new_cells}");
    for plan in measurements {
        println!("{plan}");
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
            "events_primary_request_kind_idx",
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
            let (unfiltered, _) = original.split_once(" WHERE ").expect("partial index");
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
            .execute("UPDATE rustx_store SET schema_version=98", [])
            .unwrap();
    }
    drop(store);
    assert!(matches!(
        SqliteConversationStore::open_existing(id, &path),
        Err(ConversationStoreError::SchemaVersionMismatch {
            stored: 98,
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
    crate::durable::sqlite::schema::shape::verify_unique_columns(
        &connection,
        "message_ledger",
        &["position", "message_id"],
    )
    .unwrap();
    super::observation::verify_schema(&connection).unwrap();
}
