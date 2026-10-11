// Deterministic execution-cost contracts for the SQL used by native readers.
use crate::durable::inbox::CompactionCheckpointStatistics;
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
    unrelated_assistants(store, count, after);
}

fn unrelated_assistants(store: &SqliteConversationStore, count: u32, after: u32) {
    let history = (after..after + count)
        .map(|index| {
            let message_id = MessageId::new(format!("unrelated-{index}"));
            let MessageBlock::Assistant(assistant) =
                assistant_message(message_id.as_str(), "unrelated")
            else {
                unreachable!()
            };
            let committed = envelope(
                store.conversation_id(),
                &format!("assistant-{index}"),
                None,
                RuntimeEvent::AssistantMessageCommitted { message_id },
            );
            (assistant, committed)
        })
        .collect::<Vec<_>>();
    append_assistant_history_fixture(store, &history).unwrap();
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

fn attempt_lookup_cost(connection: &Connection, id: &str, through: u64) -> i32 {
    let plan: String = connection
        .query_row(
            &format!("EXPLAIN QUERY PLAN {TRANSCRIPT_ATTEMPTS_SQL}"),
            params![id, through],
            |row| row.get(3),
        )
        .unwrap();
    assert!(plan.contains("SEARCH events USING INDEX events_message_id_kind_idx (<expr>=? AND <expr>=? AND sequence<?)"), "{plan}");
    println!("{plan}");
    let mut statement = connection.prepare(TRANSCRIPT_ATTEMPTS_SQL).unwrap();
    let results = statement
        .query_map(params![id, through], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(results, vec!["selected-attempt"]);
    statement.get_status(StatementStatus::VmStep)
}

#[test]
fn transcript_attempts_seek_only_selected_identities_at_the_finite_cut() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("history.sqlite");
    let id = ConversationId::generate();
    let store = SqliteConversationStore::in_memory(id.clone()).unwrap();
    for (message, attempt) in [
        ("early", "selected-attempt"),
        ("same", "selected-attempt"),
        ("other", "other-attempt"),
    ] {
        store
            .append_canonical_with_event(
                &assistant_message(message, message),
                envelope(
                    store.conversation_id(),
                    message,
                    Some(AttemptId::new(attempt)),
                    RuntimeEvent::AssistantMessageCommitted {
                        message_id: MessageId::new(message),
                    },
                ),
            )
            .unwrap();
    }
    let selected = vec![
        MessageId::new("early"),
        MessageId::new("same"),
        MessageId::new("early"),
        MessageId::new("other"),
    ];
    let cut = store.presentation_frontier().unwrap();
    assert!(store.transcript_attempts(&selected, 0).unwrap().is_empty());
    unrelated_assistants(&store, 8, 1);
    let small = attempt_lookup_cost(&store.lock().unwrap(), "early", 1_000_000);
    unrelated_assistants(&store, 2048, 9);
    let large = attempt_lookup_cost(&store.lock().unwrap(), "early", 1_000_000);
    assert!(large <= small + 6 && large < 100, "{small} -> {large}");
    println!(
        "selected transcript identity, 8 versus 2056 unrelated commits: {small} -> {large} VM steps"
    );
    let expected = vec![
        AttemptId::new("other-attempt"),
        AttemptId::new("selected-attempt"),
    ];
    assert_eq!(store.transcript_attempts(&selected, cut).unwrap(), expected);
    assert_eq!(
        store
            .transcript_attempts(&[MessageId::new("early")], 1)
            .unwrap(),
        vec![AttemptId::new("selected-attempt")]
    );
    assert!(store.transcript_attempts(&[], cut).unwrap().is_empty());
    assert!(
        store
            .transcript_attempts(&[MessageId::new("absent")], cut)
            .unwrap()
            .is_empty()
    );
    store
        .lock()
        .unwrap()
        .execute("VACUUM INTO ?1", [path.to_str().unwrap()])
        .unwrap();
    drop(store);
    let store = SqliteConversationStore::open_existing(id.clone(), &path).unwrap();
    assert_eq!(store.transcript_attempts(&selected, cut).unwrap(), expected);
    // A malformed selected owner still fails; irrelevant malformed owners were not read.
    drop(store);
    let connection = Connection::open(&path).unwrap();
    connection
        .execute("UPDATE events SET attempt_id=NULL WHERE sequence=1", [])
        .unwrap();
    drop(connection);
    let store = SqliteConversationStore::open_existing(id, &path).unwrap();
    assert!(store.transcript_attempts(&selected, cut).is_err());
}

fn inherited_seed(count: usize, compact_last: bool, measured_last: bool) -> LineageSeed {
    use crate::durable::reading::{
        ConversationTurnId, InheritedTurnOutcome, TurnExecution, TurnReadingProvenance,
    };
    let mut canonical = Vec::new();
    let mut history = Vec::new();
    let mut checkpoints = std::collections::BTreeMap::new();
    let mut turns = Vec::new();
    for index in 0..count {
        let id = MessageId::new(format!("inherited-{index}"));
        canonical.push(assistant_message(id.as_str(), "historical content"));
        history.push(SurfaceOp::Append {
            message_id: id.clone(),
        });
        turns.push(TurnReadingProvenance {
            id: ConversationTurnId {
                conversation_id: ConversationId::new("conv_7ee23a93-1fb2-7d39-84d8-def51c2d5f73"),
                attempt_id: AttemptId::new(format!("attempt-{index}")),
            },
            process_message_ids: vec![id.clone()],
            preceding_message_id: None,
            prompt_message_id: None,
            outcome: InheritedTurnOutcome::Completed,
            started_at: None,
            ended_at: None,
            execution: Some(TurnExecution {
                model_requests: 1,
                occupancy: (index + 1 < count || measured_last).then(|| {
                    crate::context::occupancy::estimate(12, 4096, "historical/model", "", &[])
                }),
                ..Default::default()
            }),
        });
        if index + 1 < count || compact_last {
            let summary_id = MessageId::new(format!("summary-{index}"));
            let MessageBlock::User(mut summary) = user_message(summary_id.as_str(), "summary")
            else {
                unreachable!()
            };
            summary.source = UserSource::Runtime;
            summary.kind = InboundKind::CompactionSummary(
                crate::message::types::CompactionSummaryMetadata::empty(),
            );
            canonical.push(MessageBlock::User(summary));
            history.push(SurfaceOp::Replace {
                start: id.clone(),
                end: id,
                replacement: summary_id.clone(),
            });
            checkpoints.insert(
                summary_id,
                CompactionCheckpointStatistics {
                    retired_messages: 1,
                    retired_tokens: 12,
                },
            );
        }
    }
    LineageSeed::replayed(canonical, history, checkpoints)
        .unwrap()
        .with_turns(turns)
        .unwrap()
}

fn inherited_lookup_cost(store: &SqliteConversationStore) -> (i32, usize) {
    let connection = store.lock().unwrap();
    let plan = connection
        .prepare(&format!("EXPLAIN QUERY PLAN {INHERITED_OCCUPANCY_SQL}"))
        .unwrap()
        .query_map([], |row| row.get::<_, String>(3))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
        .join("\n");
    assert!(
        plan.contains("SEARCH s USING INTEGER PRIMARY KEY")
            && plan.contains("SEARCH b USING INTEGER PRIMARY KEY"),
        "{plan}"
    );
    assert!(!plan.contains("SCAN "), "{plan}");
    println!("{plan}");
    let mut statement = connection.prepare(INHERITED_OCCUPANCY_SQL).unwrap();
    let bytes: usize = statement
        .query_row([], |row| {
            Ok(std::mem::size_of_val(&row.get::<_, i64>(1)?)
                + row.get::<_, String>(2)?.len()
                + row.get::<_, String>(3)?.len()
                + row.get::<_, String>(4)?.len())
        })
        .unwrap();
    (statement.get_status(StatementStatus::VmStep), bytes)
}

#[test]
fn inherited_occupancy_is_constant_cost_after_large_atomic_bootstrap_and_cold_reopen() {
    let small = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    small
        .initialize_lineage(&inherited_seed(8, false, true))
        .unwrap();
    let small_cost = inherited_lookup_cost(&small);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("destination.sqlite");
    let id = ConversationId::generate();
    let destination = SqliteConversationStore::open(id.clone(), &path).unwrap();
    let seed = inherited_seed(2048, false, true);
    assert_eq!(seed.surface_history().len(), 4095);
    destination.initialize_lineage(&seed).unwrap();
    let expected = crate::context::occupancy::inherit(seed.turns(), seed.surface_history());
    assert_eq!(destination.load_inherited_occupancy().unwrap(), expected);
    let large_cost = inherited_lookup_cost(&destination);
    assert_eq!(large_cost, small_cost);
    println!(
        "inherited lookup, 8 versus 2048 turns (4095 Surface ops): VM steps / serialized bytes {small_cost:?} -> {large_cost:?}"
    );
    // A second generation selects its own immutable lineage cut, not a source reading.
    let selected = destination
        .read_lineage_cut(destination.load_head().unwrap().revision)
        .unwrap();
    let second_seed = LineageSeed::replayed(
        selected.canonical,
        selected.surface_history,
        selected.checkpoints,
    )
    .unwrap()
    .with_turns(selected.turns)
    .unwrap();
    let second_path = directory.path().join("second.sqlite");
    let second_id = ConversationId::generate();
    let second = SqliteConversationStore::open(second_id.clone(), &second_path).unwrap();
    second.initialize_lineage(&second_seed).unwrap();
    drop(second);
    drop(destination);
    std::fs::remove_file(&path).unwrap();
    let second = SqliteConversationStore::open_existing(second_id, &second_path).unwrap();
    assert_eq!(second.load_inherited_occupancy().unwrap(), expected);
    assert_eq!(
        crate::context::occupancy::read(&second, 0).unwrap(),
        expected
    );
    assert_eq!(inherited_lookup_cost(&second), small_cost);
}

#[test]
fn inherited_occupancy_absence_and_selected_compaction_are_authoritative() {
    for (compacted, measured, present) in [
        (false, true, true),
        (false, false, false),
        (true, true, false),
        (true, false, false),
    ] {
        let seed = inherited_seed(2048, compacted, measured);
        let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
        store.initialize_lineage(&seed).unwrap();
        store.initialize(seed.canonical()).unwrap();
        assert_eq!(store.load_inherited_occupancy().unwrap().is_some(), present);
        assert_eq!(
            crate::context::occupancy::read(&store, 0)
                .unwrap()
                .is_some(),
            present
        );
        assert_eq!(store.presentation_frontier().unwrap(), 0);
    }
}

#[test]
fn inherited_occupancy_corruption_missing_metadata_and_bootstrap_rollback_fail_closed() {
    let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    assert_eq!(store.load_inherited_occupancy().unwrap(), None);
    let seed = inherited_seed(4, false, true);
    // SQLite aborts after earlier canonical and Surface writes have staged.
    store.lock().unwrap().execute_batch("CREATE TRIGGER reject_bootstrap BEFORE INSERT ON bootstrap_identity BEGIN SELECT RAISE(ABORT,'scripted bootstrap failure'); END;").unwrap();
    assert!(store.initialize_lineage(&seed).is_err());
    assert!(store.load_canonical().unwrap().is_empty());
    assert_eq!(store.load_inherited_occupancy().unwrap(), None);
    assert_eq!(
        store
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM surface_ops", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    store
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER reject_bootstrap")
        .unwrap();
    store.initialize_lineage(&seed).unwrap();
    store.initialize_lineage(&seed).unwrap();
    store
        .lock()
        .unwrap()
        .execute(
            "UPDATE bootstrap_identity SET message_count=message_count+1",
            [],
        )
        .unwrap();
    assert!(store.load_inherited_occupancy().is_err());
    store
        .lock()
        .unwrap()
        .execute(
            "UPDATE bootstrap_identity SET message_count=?1",
            [i64::try_from(seed.canonical().len()).unwrap()],
        )
        .unwrap();
    store
        .lock()
        .unwrap()
        .execute("UPDATE bootstrap_identity SET occupancy_json='null'", [])
        .unwrap();
    assert!(
        store.load_inherited_occupancy().is_err(),
        "valid JSON with invalid derivation integrity must fail"
    );
    assert!(store.initialize_lineage(&seed).is_err());
    store
        .lock()
        .unwrap()
        .execute("DELETE FROM bootstrap_identity", [])
        .unwrap();
    assert!(
        store.load_inherited_occupancy().is_err(),
        "missing initialized metadata is not an uninitialized store"
    );
    assert!(store.initialize_lineage(&seed).is_err());
}

#[test]
fn inherited_occupancy_rejects_invalid_seed_and_missing_durable_shape() {
    let seed = inherited_seed(2, false, true);
    let mut turns = seed.turns().to_vec();
    turns[1]
        .execution
        .as_mut()
        .unwrap()
        .occupancy
        .as_mut()
        .unwrap()
        .context_window_tokens = 0;
    assert!(
        LineageSeed::replayed(
            seed.canonical().to_vec(),
            seed.surface_history().to_vec(),
            seed.checkpoints().clone()
        )
        .unwrap()
        .with_turns(turns)
        .is_err()
    );
    let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    store.lock().unwrap().execute_batch("CREATE TRIGGER reject_marker BEFORE UPDATE OF bootstrap_initialized ON rustx_store BEGIN SELECT RAISE(ABORT,'scripted marker failure'); END;").unwrap();
    assert!(store.initialize_lineage(&seed).is_err());
    assert_eq!(store.load_inherited_occupancy().unwrap(), None);
    assert!(store.load_canonical().unwrap().is_empty());
    assert_eq!(
        store
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM bootstrap_identity", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    store
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER reject_marker")
        .unwrap();
    store.initialize_lineage(&seed).unwrap();
    store
        .lock()
        .unwrap()
        .execute_batch("ALTER TABLE bootstrap_identity DROP COLUMN occupancy_json")
        .unwrap();
    assert!(store.load_inherited_occupancy().is_err());
    assert!(verify_schema_shape(&store.lock().unwrap()).is_err());
}

#[test]
fn bootstrap_reinitialization_rejects_a_different_immutable_surface_with_the_same_reading() {
    let seed = inherited_seed(3, false, true);
    let mut history = seed.surface_history().to_vec();
    let SurfaceOp::Replace { start, end, .. } = &mut history[3] else {
        unreachable!()
    };
    *start = MessageId::new("summary-0");
    *end = start.clone();
    let other = LineageSeed::replayed(
        seed.canonical().to_vec(),
        history,
        seed.checkpoints().clone(),
    )
    .unwrap()
    .with_turns(seed.turns().to_vec())
    .unwrap();
    assert_eq!(
        crate::context::occupancy::inherit(seed.turns(), seed.surface_history()),
        crate::context::occupancy::inherit(other.turns(), other.surface_history())
    );
    let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    store.initialize_lineage(&seed).unwrap();
    assert!(matches!(
        store.initialize_lineage(&other),
        Err(ConversationStoreError::InitialHistoryMismatch)
    ));
    store.initialize(seed.canonical()).unwrap();
    assert_eq!(
        store.load_inherited_occupancy().unwrap(),
        crate::context::occupancy::inherit(seed.turns(), seed.surface_history())
    );
}

#[test]
fn assistant_history_fixture_matches_native_appends_and_rolls_back_invalid_pairs() {
    let id = ConversationId::generate();
    let ordinary = SqliteConversationStore::in_memory(id.clone()).unwrap();
    let bulk = SqliteConversationStore::in_memory(id).unwrap();
    let history = (0..4)
        .map(|index| {
            let message_id = MessageId::new(format!("fixture-{index}"));
            let MessageBlock::Assistant(message) = assistant_message(message_id.as_str(), "text")
            else {
                unreachable!()
            };
            let event = envelope(
                ordinary.conversation_id(),
                &format!("fixture-event-{index}"),
                Some(AttemptId::new("owner")),
                RuntimeEvent::AssistantMessageCommitted { message_id },
            );
            (message, event)
        })
        .collect::<Vec<_>>();
    for (assistant, event) in &history {
        ordinary
            .append_canonical_with_event(&MessageBlock::Assistant(assistant.clone()), event.clone())
            .unwrap();
    }
    append_assistant_history_fixture(&bulk, &history).unwrap();
    assert_eq!(
        bulk.load_canonical().unwrap(),
        ordinary.load_canonical().unwrap()
    );
    assert_eq!(bulk.load_head().unwrap(), ordinary.load_head().unwrap());
    assert_eq!(
        serde_json::to_value(bulk.load_transcript_page(None, 64).unwrap()).unwrap(),
        serde_json::to_value(ordinary.load_transcript_page(None, 64).unwrap()).unwrap()
    );
    for through in 0..=4 {
        assert_eq!(
            bulk.load_surface_history(SurfaceRevision::new(through))
                .unwrap(),
            ordinary
                .load_surface_history(SurfaceRevision::new(through))
                .unwrap()
        );
        let selected = query(
            FactScope::Attempt(AttemptId::new("owner")),
            vec!["assistant_message_committed"],
            through,
        );
        assert_eq!(
            serde_json::to_value(bulk.read_presentation_events(&selected).unwrap()).unwrap(),
            serde_json::to_value(ordinary.read_presentation_events(&selected).unwrap()).unwrap()
        );
    }
    let mut failed = history[..2].to_vec();
    for (index, (assistant, event)) in failed.iter_mut().enumerate() {
        assistant.id = MessageId::new(format!("future-{index}"));
        event.event_id = EventId::new(format!("future-event-{index}"));
        event.event = RuntimeEvent::AssistantMessageCommitted {
            message_id: assistant.id.clone(),
        };
    }
    failed[1].1.event = RuntimeEvent::AssistantMessageCommitted {
        message_id: MessageId::new("wrong"),
    };
    assert!(append_assistant_history_fixture(&bulk, &failed).is_err());
    assert_eq!(
        bulk.load_canonical().unwrap(),
        ordinary.load_canonical().unwrap()
    );
    assert_eq!(bulk.load_head().unwrap(), ordinary.load_head().unwrap());
    assert_eq!(bulk.presentation_frontier().unwrap(), 4);
}
