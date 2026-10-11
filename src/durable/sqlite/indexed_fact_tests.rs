// Deterministic execution-cost contracts for the SQL used by native readers.
use crate::durable::presentation::{FactQuery, FactScope};
use rusqlite::StatementStatus;

fn query(scope: FactScope, kinds: Vec<&'static str>, through: u64) -> FactQuery {
    FactQuery {
        scope,
        kinds,
        before: None,
        after: 0,
        ascending: false,
        through,
        limit: 1,
    }
}

fn execution_cost(connection: &Connection, query: &FactQuery) -> (usize, i32) {
    let mut count = 0;
    let mut steps = 0;
    for (sql, values) in presentation_statements(query).unwrap() {
        let plan = connection
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .unwrap()
            .query_map(rusqlite::params_from_iter(values.clone()), |row| {
                row.get::<_, String>(3)
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .join("\n");
        assert!(plan.contains("SEARCH events USING INDEX"), "{plan}");
        assert!(
            !plan.contains("SCAN ") && !plan.contains("TEMP B-TREE"),
            "{plan}"
        );
        println!("{plan}");
        let mut statement = connection.prepare(&sql).unwrap();
        count += statement
            .query_map(rusqlite::params_from_iter(values), |row| {
                row.get::<_, String>(0)
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .len();
        steps += statement.get_status(StatementStatus::VmStep);
    }
    (count, steps)
}

fn terminal(store: &SqliteConversationStore, retry: u32, measured: bool) -> RuntimeEventEnvelope {
    let mut snapshot = test_request_snapshot(store.load_head().unwrap().revision, invocation());
    // These are separate native Steps, not thousands of retries in one Step.
    snapshot.identity.turn = TurnId::new(format!("terminal-step-{retry}"));
    snapshot.identity.retry_number = 0;
    snapshot.request_id = snapshot.identity.request_id();
    snapshot.provisional_message_id = snapshot.identity.provisional_message_id();
    store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap();
    let mut terminal = envelope(
        store.conversation_id(),
        &format!("terminal-{retry}"),
        Some(snapshot.identity.attempt_id.clone()),
        RuntimeEvent::ModelRequestCompleted {
            request_id: snapshot.request_id,
            finish_reason: ModelFinishReason::Stop,
            usage: measured.then_some(crate::model::types::ModelUsage {
                input_tokens: 12,
                output_tokens: 3,
                total_tokens: 15,
                details: None,
            }),
            generation: None,
        },
    );
    terminal.turn_id = Some(snapshot.identity.turn);
    store.append_event(terminal).unwrap()
}
fn unrelated_history(store: &SqliteConversationStore, count: u32, after: u32) {
    // Keep request fixtures independent of unrelated canonical payload growth.
    // Both histories still contain the exact committed Journal facts queried.
    for index in after..after + count {
        terminal(store, index, false);
    }
    for index in after..after + count {
        let message_id = MessageId::new(format!("unrelated-{index}"));
        store
            .append_canonical_with_event(
                &assistant_message(message_id.as_str(), "unrelated"),
                envelope(
                    store.conversation_id(),
                    &format!("assistant-{index}"),
                    None,
                    RuntimeEvent::AssistantMessageCommitted { message_id },
                ),
            )
            .unwrap();
    }
}

#[test]
fn indexed_occupancy_and_message_lookup_cost_does_not_grow_with_irrelevant_history() {
    let store = store();
    terminal(&store, 0, true);
    store
        .append_canonical_with_event(
            &assistant_message("target", "target"),
            envelope(
                store.conversation_id(),
                "assistant",
                None,
                RuntimeEvent::AssistantMessageCommitted {
                    message_id: MessageId::new("target"),
                },
            ),
        )
        .unwrap();
    let cut = store.presentation_frontier().unwrap();
    let occupancy = |through| {
        query(
            FactScope::ContextReading,
            vec![
                "model_request_completed",
                "model_request_failed",
                "compaction_completed",
            ],
            through,
        )
    };
    let message = |through| {
        query(
            FactScope::Message("target".into()),
            vec!["assistant_message_committed"],
            through,
        )
    };
    unrelated_history(&store, 8, 1);
    let small = {
        let connection = store.lock().unwrap();
        [
            execution_cost(&connection, &occupancy(1_000_000)),
            execution_cost(&connection, &message(1_000_000)),
        ]
    };
    unrelated_history(&store, 2048, 9);
    let connection = store.lock().unwrap();
    for (index, q) in [occupancy(1_000_000), message(1_000_000)]
        .iter()
        .enumerate()
    {
        let large = execution_cost(&connection, q);
        assert_eq!(large.0, 1);
        assert!(
            large.1 <= small[index].1 + 6 && large.1 < 200,
            "indexed VM steps grew: {:?} -> {large:?}",
            small[index]
        );
        println!(
            "lookup {index}: 8 versus 2056 unrelated rows per kind: {} -> {} VM steps",
            small[index].1, large.1
        );
    }
    assert_eq!(execution_cost(&connection, &occupancy(cut)).0, 1);
    assert_eq!(execution_cost(&connection, &message(cut)).0, 1);
    for predicate in [
        "json_extract(event_json, '$.event.type') = 'model_request_completed' AND json_type(event_json, '$.event.usage') = 'object'",
        "json_extract(event_json, '$.event.type') = 'assistant_message_committed' AND json_extract(event_json, '$.event.message_id') = 'target'",
    ] {
        let mut statement = connection.prepare(&format!("SELECT event_json FROM events INDEXED BY events_kind_idx WHERE {predicate} ORDER BY sequence DESC LIMIT 1")).unwrap();
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert!(statement.get_status(StatementStatus::VmStep) > 2048 * 2);
        println!(
            "preceding kind-only query: {} VM steps",
            statement.get_status(StatementStatus::VmStep)
        );
    }
}

#[test]
fn indexed_occupancy_propagates_missing_and_corrupt_evidence() {
    let store = store();
    let event = terminal(&store, 0, true);
    store
        .lock()
        .unwrap()
        .execute("UPDATE request_snapshots SET snapshot_json='{}'", [])
        .unwrap();
    assert!(crate::context::occupancy::read(&store, event.sequence).is_err());
    store
        .lock()
        .unwrap()
        .execute("DELETE FROM request_snapshots", [])
        .unwrap();
    assert!(crate::context::occupancy::read(&store, event.sequence).is_err());
    store.lock().unwrap().execute("UPDATE events SET event_json=json_set(event_json, '$.event.usage', 'corrupt') WHERE sequence=?1", [event.sequence]).unwrap();
    assert!(crate::context::occupancy::read(&store, event.sequence).is_err());
    store
        .lock()
        .unwrap()
        .execute_batch("DROP INDEX events_context_reading_idx")
        .unwrap();
    assert!(crate::context::occupancy::read(&store, event.sequence).is_err());
}

#[test]
fn bootstrap_surface_boundary_is_immutable_metadata_and_invalid_evidence_fails() {
    let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    store
        .initialize(&[assistant_message("inherited", "body")])
        .unwrap();
    store
        .append_canonical(&assistant_message("local", "later"))
        .unwrap();
    assert_eq!(
        store.load_bootstrap_surface_revision().unwrap(),
        SurfaceRevision::new(1)
    );
    let connection = store.lock().unwrap();
    let plan: String = connection
        .query_row(
            "EXPLAIN QUERY PLAN SELECT message_count FROM bootstrap_identity WHERE id=1",
            [],
            |row| row.get(3),
        )
        .unwrap();
    assert!(
        plan.contains("SEARCH bootstrap_identity USING INTEGER PRIMARY KEY"),
        "{plan}"
    );
    connection
        .execute("UPDATE bootstrap_identity SET message_count=-1", [])
        .unwrap();
    drop(connection);
    assert!(store.load_bootstrap_surface_revision().is_err());
    store
        .lock()
        .unwrap()
        .execute("DELETE FROM bootstrap_identity", [])
        .unwrap();
    assert!(store.load_bootstrap_surface_revision().is_err());
}
