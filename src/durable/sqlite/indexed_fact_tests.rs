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
        .execute_batch(
            "PRAGMA foreign_keys=OFF; DELETE FROM request_snapshots; PRAGMA foreign_keys=ON",
        )
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

fn response_seed(count: usize) -> LineageSeed {
    use crate::durable::response::{
        CompletedResponseProvenance, CompletedResponseTiming, ResponseOrigin,
    };
    let base = inherited_seed(count, false, true);
    let mut canonical = base.canonical().to_vec();
    let mut history = base.surface_history().to_vec();
    let mut checkpoints = base.checkpoints().clone();
    let mut turns = base.turns().to_vec();
    let call = ToolCall {
        id: ToolCallId::new("early-call"),
        tool_id: ToolId::new("tool-bash"),
        name: "bash".into(),
        arguments: serde_json::json!({}),
    };
    let MessageBlock::Assistant(first) = &mut canonical[0] else {
        unreachable!()
    };
    first.content = vec![AssistantContentBlock::ToolCall(call.clone())];
    let result = MessageBlock::Tool(ToolMessageBlock {
        id: MessageId::new("early-result"),
        occurrence: crate::message::types::ToolCallOccurrenceRef::new(
            MessageId::new("inherited-0"),
            ContentBlockIndex::new(0),
        ),
        tool_call_id: call.id,
        tool_id: call.tool_id,
        result: ToolExecutionResult {
            deliveries: vec![],
            status: ToolExecutionStatus::Success,
            content: vec![],
            duration_ms: 1,
            exit_code: None,
            artifacts: vec![],
            truncation: None,
            workflow: None,
            managed_output: None,
        },
    });
    canonical.splice(
        1..1,
        [result, assistant_message("early-closing", "final answer")],
    );
    history.splice(
        1..1,
        [
            SurfaceOp::Append {
                message_id: MessageId::new("early-result"),
            },
            SurfaceOp::Append {
                message_id: MessageId::new("early-closing"),
            },
        ],
    );
    let SurfaceOp::Replace { end, .. } = &mut history[3] else {
        unreachable!()
    };
    *end = MessageId::new("early-closing");
    checkpoints
        .get_mut(&MessageId::new("summary-0"))
        .unwrap()
        .retired_messages = 3;
    turns[0]
        .process_message_ids
        .push(MessageId::new("early-closing"));
    let seed = LineageSeed::replayed(canonical, history, checkpoints)
        .unwrap()
        .with_turns(turns)
        .unwrap();
    let responses = seed
        .turns()
        .iter()
        .map(|turn| {
            let closing = turn.process_message_ids.last().unwrap().clone();
            CompletedResponseProvenance {
                process_message_ids: turn.process_message_ids.clone(),
                closing_message_id: closing.clone(),
                origin: ResponseOrigin {
                    conversation_id: turn.id.conversation_id.clone(),
                    attempt_id: turn.id.attempt_id.clone(),
                    closing_message_id: closing,
                },
                completed_at: chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
                retry_message_id: None,
                usage: Some(crate::model::ModelUsage {
                    input_tokens: 12,
                    output_tokens: 3,
                    total_tokens: 15,
                    details: None,
                }),
                timing: Some(CompletedResponseTiming {
                    total_duration_ms: Some(120),
                    ..Default::default()
                }),
                models: vec!["historical/model".into()],
            }
        })
        .collect();
    seed.with_completed_responses(responses).unwrap()
}

fn measured_vm<T>(store: &SqliteConversationStore, read: impl FnOnce() -> T) -> (T, usize) {
    let count = Arc::new(AtomicUsize::new(0));
    let progress = count.clone();
    store.conn.lock().unwrap().progress_handler(
        1,
        Some(move || {
            progress.fetch_add(1, Ordering::Relaxed);
            false
        }),
    );
    let result = read();
    store
        .conn
        .lock()
        .unwrap()
        .progress_handler(0, None::<fn() -> bool>);
    (result, count.load(Ordering::Relaxed))
}

// Bridge only the complete cut returned by the same original native window
// transaction. A Journal number alone cannot authorize canonical decoration.
fn decorate_original_window(
    store: &SqliteConversationStore,
    window: crate::durable::reading::DurableConversationWindow,
) -> crate::runtime_client::snapshot::RuntimeClientTranscriptPage {
    let inherited_through = window.page.inherited_through;
    let read = window.cut;
    let through = read.journal;
    let mut page = crate::runtime_client::snapshot::transcript_page_view(window.page).unwrap();
    page.source_cut = Some(crate::durable::observation::ObservationReadCut {
        ledger_through: read.ledger_through,
        transcript_total: read.transcript_total,
        surface_revision: read.surface_revision,
        inherited_through,
        read,
    });
    crate::runtime_client::response::decorate_window(store, &mut page, through).unwrap();
    page
}

#[test]
#[allow(clippy::too_many_lines)] // One query-work comparison across immutable seed sizes.
fn inherited_provenance_identity_queries_and_real_window_cost_are_constant() {
    use crate::durable::reading::ConversationWindowAt;
    let mut costs = Vec::new();
    let mut expected = None;
    for count in [8, 128, 2048] {
        let seed = response_seed(count);
        let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
        store.initialize_lineage(&seed).unwrap();
        let wanted = [
            MessageId::new("inherited-0"),
            MessageId::new("early-closing"),
            MessageId::new("inherited-0"),
        ];
        let (turns, turn_cost) =
            measured_vm(&store, || store.inherited_turns_for(&wanted[..1]).unwrap());
        let (responses, response_cost) = measured_vm(&store, || {
            store.inherited_responses_for(&wanted[..1]).unwrap()
        });
        assert_eq!(turns, seed.turns()[..1]);
        assert_eq!(responses, seed.completed_responses()[..1]);
        assert_eq!(store.inherited_turns_for(&wanted).unwrap(), turns);
        assert_eq!(store.inherited_responses_for(&wanted).unwrap(), responses);
        let different = [MessageId::new("inherited-0"), MessageId::new("inherited-1")];
        assert_eq!(
            store.inherited_turns_for(&different).unwrap(),
            seed.turns()[..2]
        );
        assert_eq!(
            store.inherited_responses_for(&different).unwrap(),
            seed.completed_responses()[..2]
        );
        assert!(store.inherited_turns_for(&[]).unwrap().is_empty());
        assert!(
            store
                .inherited_responses_for(&[MessageId::new("summary-0")])
                .unwrap()
                .is_empty()
        );

        for sql in [provenance::TURN_LOOKUP, provenance::RESPONSE_LOOKUP] {
            let plan = store
                .lock()
                .unwrap()
                .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
                .unwrap()
                .query_map(["inherited-0"], |row| row.get::<_, String>(3))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
                .join("\n");
            println!("{plan}");
            assert!(
                plan.contains(
                    "SEARCH m USING INDEX sqlite_autoindex_inherited_message_membership_2"
                ),
                "{plan}"
            );
            assert!(
                plan.contains("SEARCH p USING INTEGER PRIMARY KEY"),
                "{plan}"
            );
            assert!(
                !plan.contains("SCAN") && !plan.contains("VIRTUAL TABLE"),
                "{plan}"
            );
        }
        let (window, window_cost) = measured_vm(&store, || {
            let read = store
                .conversation_window(
                    &ConversationWindowAt::Older {
                        cut: None,
                        before: TranscriptCursor::new(2),
                    },
                    1,
                )
                .unwrap();
            let page = decorate_original_window(&store, read);
            assert_eq!(
                page.inherited_through.unwrap().get(),
                seed.canonical().len() as u64
            );
            let source = page.source_cut.as_ref().unwrap();
            assert_eq!(&source.read.conversation_id, store.conversation_id());
            assert_eq!(source.ledger_through, seed.canonical().len() as u64);
            assert_eq!(
                source.inherited_through.get(),
                seed.canonical().len() as u64
            );
            assert_eq!(source.read, store.conversation_read_cut().unwrap());
            // Only semantic selected entries are compared across distinct native
            // destinations; each complete source cut was independently checked.
            page.entries
        });
        if let Some(expected) = &expected {
            assert_eq!(&window, expected);
        } else {
            expected = Some(window);
        }
        assert!(store.transcript_attempts(&wanted, 0).unwrap().is_empty());
        assert_eq!(store.presentation_frontier().unwrap(), 0);
        println!(
            "{count} inherited Turns and Responses: actual Turn {turn_cost}, Response {response_cost}, entire window {window_cost} VM instructions"
        );
        costs.push((turn_cost, response_cost, window_cost));
        assert_eq!(store.load_inherited_turns().unwrap(), seed.turns());
        assert_eq!(
            store.load_inherited_responses().unwrap(),
            seed.completed_responses()
        );
        store.initialize(seed.canonical()).unwrap();
    }
    assert!(costs.iter().all(|cost| cost == &costs[0]), "{costs:?}");
}

#[test]
fn inherited_provenance_bootstrap_failures_roll_back_every_native_authority() {
    let seed = response_seed(8);
    for stage in [
        "inherited_turn_provenance",
        "inherited_response_provenance",
        "inherited_message_membership",
        "rustx_store",
    ] {
        let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
        let predicate = if stage == "rustx_store" {
            String::new()
        } else {
            format!(" WHEN (SELECT count(*) FROM {stage})>0")
        };
        let timing = if stage == "rustx_store" {
            "UPDATE OF bootstrap_initialized"
        } else {
            "INSERT"
        };
        store.lock().unwrap().execute_batch(&format!("CREATE TRIGGER reject_stage BEFORE {timing} ON {stage}{predicate} BEGIN SELECT RAISE(ABORT,'scripted failure'); END;")).unwrap();
        assert!(store.initialize_lineage(&seed).is_err());
        for table in [
            "message_ledger",
            "surface_ops",
            "transcript_order",
            "bootstrap_identity",
            "inherited_turn_provenance",
            "inherited_response_provenance",
            "inherited_message_membership",
        ] {
            assert_eq!(
                store
                    .lock()
                    .unwrap()
                    .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| row
                        .get::<_, i64>(0))
                    .unwrap(),
                0,
                "{stage}: {table}"
            );
        }
        assert_eq!(store.load_inherited_occupancy().unwrap(), None);
        store
            .lock()
            .unwrap()
            .execute_batch("DROP TRIGGER reject_stage")
            .unwrap();
        store.initialize_lineage(&seed).unwrap();
        store.initialize_lineage(&seed).unwrap();
    }
}

#[test]
fn inherited_provenance_corruption_and_mismatched_reinitialization_fail_closed() {
    let seed = response_seed(8);
    let wanted = [MessageId::new("inherited-0")];
    for damage in [
        "DELETE FROM inherited_message_membership WHERE assistant_message_id='inherited-0'",
        "UPDATE inherited_message_membership SET turn_ordinal=NULL WHERE assistant_message_id='inherited-0'",
        "UPDATE inherited_message_membership SET response_ordinal=999 WHERE assistant_message_id='inherited-0'",
        "DELETE FROM inherited_turn_provenance WHERE ordinal=0",
        "DELETE FROM inherited_response_provenance WHERE ordinal=0",
        "UPDATE inherited_turn_provenance SET payload='{}' WHERE ordinal=0",
        "UPDATE inherited_response_provenance SET payload='{}' WHERE ordinal=0",
        "DELETE FROM bootstrap_identity",
        "UPDATE bootstrap_identity SET turn_count=turn_count+1",
        "UPDATE bootstrap_identity SET response_count=response_count+1",
    ] {
        let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
        store.initialize_lineage(&seed).unwrap();
        store
            .lock()
            .unwrap()
            .execute_batch(&format!("PRAGMA foreign_keys=OFF; {damage};"))
            .unwrap();
        assert!(
            store.inherited_turns_for(&wanted).is_err()
                || store.inherited_responses_for(&wanted).is_err(),
            "{damage}"
        );
        assert!(store.initialize_lineage(&seed).is_err(), "{damage}");
    }
    let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    store.initialize_lineage(&seed).unwrap();
    assert!(store.lock().unwrap().execute("INSERT INTO inherited_message_membership SELECT * FROM inherited_message_membership WHERE assistant_message_id='inherited-0'",[]).is_err());
    assert!(store.lock().unwrap().execute("UPDATE inherited_message_membership SET turn_ordinal=999 WHERE assistant_message_id='inherited-0'",[]).is_err());
    let mut turns = seed.turns().to_vec();
    turns[0].started_at = chrono::DateTime::from_timestamp(1_700_000_002, 0);
    let different = LineageSeed::replayed(
        seed.canonical().to_vec(),
        seed.surface_history().to_vec(),
        seed.checkpoints().clone(),
    )
    .unwrap()
    .with_turns(turns)
    .unwrap()
    .with_completed_responses(seed.completed_responses().to_vec())
    .unwrap();
    assert!(matches!(
        store.initialize_lineage(&different),
        Err(ConversationStoreError::InitialHistoryMismatch)
    ));
    let mut responses = seed.completed_responses().to_vec();
    responses[0].models.push("other/model".into());
    let different = LineageSeed::replayed(
        seed.canonical().to_vec(),
        seed.surface_history().to_vec(),
        seed.checkpoints().clone(),
    )
    .unwrap()
    .with_turns(seed.turns().to_vec())
    .unwrap()
    .with_completed_responses(responses)
    .unwrap();
    assert!(matches!(
        store.initialize_lineage(&different),
        Err(ConversationStoreError::InitialHistoryMismatch)
    ));
}

#[test]
fn bootstrap_marker_and_provenance_are_atomic_before_inherited_index_reads() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("bootstrap-order.sqlite");
    let id = ConversationId::generate();
    let seed = response_seed(2);
    let store = SqliteConversationStore::open(id.clone(), &path).unwrap();
    store.initialize_lineage(&seed).unwrap();
    assert_eq!(store.load_inherited_turns().unwrap(), seed.turns());
    assert_eq!(
        store.load_inherited_responses().unwrap(),
        seed.completed_responses()
    );
    store.initialize_lineage(&seed).unwrap();
    drop(store);
    let store = SqliteConversationStore::open(id.clone(), &path).unwrap();
    assert_eq!(store.load_inherited_turns().unwrap(), seed.turns());
    drop(store);
    let connection = Connection::open(&path).unwrap();
    connection
        .execute(
            "UPDATE rustx_store SET bootstrap_initialized=0 WHERE id=1",
            [],
        )
        .unwrap();
    drop(connection);
    let store = SqliteConversationStore::open(id, &path).unwrap();
    assert!(matches!(
        store.load_inherited_occupancy(),
        Err(ConversationStoreError::Storage(_))
    ));
    assert!(matches!(
        store.initialize_lineage(&seed),
        Err(ConversationStoreError::Storage(_))
    ));
}

#[test]
fn inherited_provenance_selected_cuts_survive_destination_only_cold_reopen() {
    let directory = tempfile::tempdir().unwrap();
    for (revision, incomplete) in [(1, false), (3, false), (4, false), (1, true), (3, true)] {
        let source_path = directory
            .path()
            .join(format!("source-{revision}-{incomplete}.sqlite"));
        let source =
            SqliteConversationStore::open(ConversationId::generate(), &source_path).unwrap();
        let original = response_seed(128);
        let mut turns = original.turns().to_vec();
        let mut responses = original.completed_responses().to_vec();
        if incomplete {
            turns[0].outcome = crate::durable::reading::InheritedTurnOutcome::IncompleteAtCut;
            responses.remove(0);
        }
        let original = LineageSeed::replayed(
            original.canonical().to_vec(),
            original.surface_history().to_vec(),
            original.checkpoints().clone(),
        )
        .unwrap()
        .with_turns(turns)
        .unwrap()
        .with_completed_responses(responses)
        .unwrap();
        source.initialize_lineage(&original).unwrap();
        let selected = source
            .read_lineage_cut(SurfaceRevision::new(revision))
            .unwrap();
        let destination_id = ConversationId::generate();
        let seed = crate::local_runtime::session::remap_seed(
            &destination_id,
            &selected.canonical,
            &selected.surface_history,
            &selected.checkpoints,
            &selected.completed_responses,
            &selected.turns,
            &selected.raw_tool_arguments,
        )
        .unwrap();
        assert_eq!(
            seed.completed_responses().len(),
            usize::from(revision > 1 && !incomplete)
        );
        assert_eq!(
            seed.turns()[0].outcome,
            if incomplete {
                crate::durable::reading::InheritedTurnOutcome::IncompleteAtCut
            } else {
                crate::durable::reading::InheritedTurnOutcome::Completed
            }
        );
        let path = directory
            .path()
            .join(format!("destination-{revision}-{incomplete}.sqlite"));
        let destination = SqliteConversationStore::open(destination_id.clone(), &path).unwrap();
        destination.initialize_lineage(&seed).unwrap();
        drop(destination);
        drop(source);
        std::fs::remove_file(source_path).unwrap();
        let destination = SqliteConversationStore::open_existing(destination_id, &path).unwrap();
        assert_eq!(destination.load_inherited_turns().unwrap(), seed.turns());
        assert_eq!(
            destination.load_inherited_responses().unwrap(),
            seed.completed_responses()
        );
        assert_eq!(
            destination
                .inherited_turns_for(&seed.turns()[0].process_message_ids)
                .unwrap(),
            seed.turns()
        );
        assert_eq!(
            destination
                .inherited_responses_for(&seed.turns()[0].process_message_ids)
                .unwrap(),
            seed.completed_responses()
        );
        destination.initialize(seed.canonical()).unwrap();
        assert_eq!(destination.presentation_frontier().unwrap(), 0);
    }
}

#[test]
#[allow(clippy::too_many_lines)] // One captured-cut contract across inherited, Tool, Human and local windows.
fn inherited_window_membership_is_independent_of_local_finite_execution_cuts() {
    use crate::durable::reading::ConversationWindowAt;
    let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    let base = response_seed(8);
    let mut canonical = base.canonical().to_vec();
    canonical.insert(0, user_message("human-input", "input"));
    let mut history = base.surface_history().to_vec();
    history.insert(
        0,
        SurfaceOp::Append {
            message_id: MessageId::new("human-input"),
        },
    );
    let seed = LineageSeed::replayed(canonical, history, base.checkpoints().clone())
        .unwrap()
        .with_turns(base.turns().to_vec())
        .unwrap()
        .with_completed_responses(base.completed_responses().to_vec())
        .unwrap();
    store.initialize_lineage(&seed).unwrap();
    let cut = store.conversation_read_cut().unwrap();
    let window = || {
        let read = store
            .conversation_window(
                &ConversationWindowAt::Older {
                    cut: Some(cut.clone()),
                    before: TranscriptCursor::new(5),
                },
                3,
            )
            .unwrap();
        decorate_original_window(&store, read)
    };
    let before = window();
    for (before, limit, expected) in [(1, 3, 0), (2, 1, 1), (4, 1, 1), (6, 4, 4)] {
        let read = store
            .conversation_window(
                &ConversationWindowAt::Older {
                    cut: Some(cut.clone()),
                    before: TranscriptCursor::new(before),
                },
                limit,
            )
            .unwrap();
        let page = decorate_original_window(&store, read);
        assert_eq!(page.entries.len(), expected);
        if before == 2 {
            assert!(page.entries[0].turn_process.is_none());
            assert!(page.entries[0].completed_response.is_none());
        }
        if before == 4 {
            assert!(page.entries[0].turn_process.is_some());
        }
    }

    store
        .append_event(envelope(
            store.conversation_id(),
            "local-start",
            Some(AttemptId::new("local")),
            RuntimeEvent::AttemptStarted {
                attempt_id: AttemptId::new("local"),
            },
        ))
        .unwrap();
    store
        .append_canonical_with_event(
            &assistant_message("local-answer", "local response"),
            envelope(
                store.conversation_id(),
                "local-commit",
                Some(AttemptId::new("local")),
                RuntimeEvent::AssistantMessageCommitted {
                    message_id: MessageId::new("local-answer"),
                },
            ),
        )
        .unwrap();
    assert_eq!(window(), before);
    assert!(
        store
            .transcript_attempts(&[MessageId::new("local-answer")], cut.journal)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store
            .transcript_attempts(
                &[MessageId::new("local-answer")],
                store.presentation_frontier().unwrap()
            )
            .unwrap(),
        vec![AttemptId::new("local")]
    );
    let wanted = [
        MessageId::new("inherited-0"),
        MessageId::new("early-closing"),
        MessageId::new("inherited-1"),
        MessageId::new("local-answer"),
    ];
    assert_eq!(
        store.inherited_turns_for(&wanted).unwrap(),
        seed.turns()[..2]
    );
    assert_eq!(
        store.inherited_responses_for(&wanted).unwrap(),
        seed.completed_responses()[..2]
    );
    let frontier = store.presentation_frontier().unwrap();
    let read = store
        .conversation_window(&ConversationWindowAt::Latest, 3)
        .unwrap();
    let mixed = decorate_original_window(&store, read);
    assert_eq!(
        store.presentation_frontier().unwrap(),
        frontier,
        "inspection cannot publish execution"
    );
    assert!(mixed.statistics.is_none());
    for entry in &mixed.entries {
        if let crate::runtime_client::snapshot::RuntimeClientTranscriptItem::Message {
            message: MessageBlock::Assistant(message),
        } = &entry.item
        {
            let owner = entry.turn_process.as_ref().unwrap();
            if message.id.as_str() == "local-answer" {
                assert_eq!(&owner.conversation_id, store.conversation_id());
            } else {
                assert_eq!(owner.conversation_id, seed.turns()[0].id.conversation_id);
            }
        }
    }

    assert!(
        store
            .inherited_turns_for(&[MessageId::new("local-answer")])
            .unwrap()
            .is_empty()
    );
}

#[test]
fn inherited_provenance_schema_requires_native_indexes_and_rejects_schema_54() {
    for index in [
        "inherited_turn_identity_idx",
        "inherited_response_closing_idx",
        "inherited_membership_turn_idx",
    ] {
        let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
        store
            .lock()
            .unwrap()
            .execute_batch(&format!("DROP INDEX {index}"))
            .unwrap();
        assert!(verify_schema_shape(&store.lock().unwrap()).is_err());
    }
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("obsolete.sqlite");
    let id = ConversationId::generate();
    let store = SqliteConversationStore::open(id.clone(), &path).unwrap();
    store.initialize_lineage(&response_seed(8)).unwrap();
    store
        .lock()
        .unwrap()
        .execute("UPDATE rustx_store SET schema_version=54", [])
        .unwrap();
    drop(store);
    assert!(matches!(
        SqliteConversationStore::open_existing(id, &path),
        Err(ConversationStoreError::SchemaVersionMismatch {
            stored: 54,
            expected: SQLITE_SCHEMA_VERSION
        })
    ));
}

// A descriptor validates only its own finite provenance evidence. Exact native
// membership is validated when this actual bounded transcript member is read.
fn materialized_member_process(
    store: &SqliteConversationStore,
    id: &str,
) -> Result<Option<crate::runtime_client::response::TurnProcessView>, ConversationStoreError> {
    let transcript: u64 = store.lock().unwrap().query_row(
        "SELECT position FROM transcript_order WHERE reference_kind='message' AND reference_id=?1",
        [id], |row| row.get(0),
    ).unwrap();
    let cut = store.observation_read_cut()?;
    store.observation_transcript_process(&cut, transcript)
}

fn deny_unloaded_turn_members(
    context: rusqlite::hooks::AuthContext<'_>,
) -> rusqlite::hooks::Authorization {
    use rusqlite::hooks::{AuthAction, Authorization};
    match context.action {
        AuthAction::Read {
            table_name: "inherited_message_membership",
            ..
        }
        | AuthAction::Read {
            table_name: "inherited_turn_provenance",
            column_name: "payload",
        }
        | AuthAction::Read {
            table_name: "message_ledger",
            column_name: "message_json",
        } => Authorization::Deny,
        _ => Authorization::Allow,
    }
}

#[test]
fn inherited_turn_descriptors_validate_finite_evidence_without_enumerating_owner_members() {
    let mut costs = Vec::new();
    for count in [8, 128, 2048] {
        let base = inherited_seed(count, false, true);
        let mut turn = base.turns()[0].clone();
        turn.process_message_ids = base
            .turns()
            .iter()
            .flat_map(|turn| turn.process_message_ids.clone())
            .collect();
        let seed = LineageSeed::replayed(
            base.canonical().to_vec(),
            base.surface_history().to_vec(),
            base.checkpoints().clone(),
        )
        .unwrap()
        .with_turns(vec![turn])
        .unwrap();
        let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
        store.initialize_lineage(&seed).unwrap();
        let cut = store.observation_read_cut().unwrap();
        store
            .lock()
            .unwrap()
            .authorizer(Some(deny_unloaded_turn_members));
        provenance::take_turn_header_validations();
        provenance::take_turn_header_read_work();
        provenance::take_turn_validation_counts();
        let (page, work) = measured_vm(&store, || store.observation_turns(Some(0), 1).unwrap());
        assert_eq!(page.total, 1);
        assert_eq!(page.turns.len(), 1);
        assert_eq!(page.turns[0].ordinal, 1);
        assert_eq!(
            page.turns[0].id.attempt_id.as_ref(),
            Some(&seed.turns()[0].id.attempt_id)
        );
        assert_eq!(provenance::take_turn_header_validations(), 1);
        let (rows, materialized) = provenance::take_turn_header_read_work();
        assert_eq!(rows, 1);
        assert!(materialized <= 16_384 + 64 + 32);
        assert_eq!(provenance::take_turn_validation_counts(), (0, 0));
        store.lock().unwrap().authorizer(
            None::<fn(rusqlite::hooks::AuthContext<'_>) -> rusqlite::hooks::Authorization>,
        );
        store
            .lock()
            .unwrap()
            .authorizer(Some(super::deny_selected_original_columns));
        for transcript in [1, u64::try_from((count - 1) * 2 + 1).unwrap()] {
            let process = store
                .observation_transcript_process(&cut, transcript)
                .unwrap()
                .unwrap();
            assert_eq!(process.message_count, u32::try_from(count).unwrap());
            assert_eq!(
                process.attempt_id.as_ref(),
                Some(&seed.turns()[0].id.attempt_id)
            );
        }
        store.lock().unwrap().authorizer(
            None::<fn(rusqlite::hooks::AuthContext<'_>) -> rusqlite::hooks::Authorization>,
        );
        assert_eq!(store.load_inherited_turns().unwrap(), seed.turns());
        costs.push(work);
    }
    assert!(costs.iter().all(|work| *work == costs[0]), "{costs:?}");
}

fn tamper_finite_turn_headers(store: &SqliteConversationStore, damage: &str) {
    let connection = store.lock().unwrap();
    for column in ["observation_json", "observation_turn_display"] {
        let bytes: Vec<u8> = connection
            .query_row(
                &format!("SELECT {column} FROM inherited_turn_provenance WHERE ordinal=0"),
                [],
                |row| row.get(0),
            )
            .unwrap();
        let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        match damage {
            "owner" => value["value"]["id"]["attempt_id"] = "another-real-owner".into(),
            "preview" => value["value"]["response"] = "another finite preview".into(),
            "cursor" => value["value"]["cursor"] = 3.into(),
            "bytes" => {}
            _ => unreachable!(),
        }
        let mut changed = serde_json::to_vec(&value).unwrap();
        if damage == "bytes" {
            changed.push(b' ');
        }
        connection
            .execute(
                &format!("UPDATE inherited_turn_provenance SET {column}=?1 WHERE ordinal=0"),
                [changed],
            )
            .unwrap();
    }
}

#[test]
fn inherited_turn_row_witness_rejects_changed_owner_preview_cursor_and_encoded_bytes() {
    let seed = inherited_seed(8, false, true);
    for damage in ["owner", "preview", "cursor", "bytes"] {
        let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
        store.initialize_lineage(&seed).unwrap();
        let cut = store.observation_read_cut().unwrap();
        let evidence = || {
            store.lock().unwrap().query_row(
            "SELECT ordinal,payload_digest,observation_json_witness,observation_turn_display_witness FROM inherited_turn_provenance WHERE ordinal=0",
            [], |row| Ok((row.get::<_,i64>(0)?,row.get::<_,String>(1)?,row.get::<_,Vec<u8>>(2)?,row.get::<_,Vec<u8>>(3)?)),
        ).unwrap()
        };
        let native = evidence();
        tamper_finite_turn_headers(&store, damage);
        assert_eq!(
            evidence(),
            native,
            "{damage}: original row evidence changed"
        );
        store
            .lock()
            .unwrap()
            .authorizer(Some(deny_unloaded_turn_members));
        provenance::take_turn_header_read_work();
        assert!(store.conversation_turns(0, 1).is_err(), "{damage}");
        assert!(store.observation_turns(Some(0), 1).is_err(), "{damage}");
        let (rows, materialized) = provenance::take_turn_header_read_work();
        assert_eq!(rows, 2);
        assert!(materialized <= 2 * (16_384 + 64 + 32));
        store
            .lock()
            .unwrap()
            .authorizer(Some(super::deny_selected_original_columns));
        assert!(
            store.observation_transcript_process(&cut, 1).is_err(),
            "{damage}"
        );
        store.lock().unwrap().authorizer(
            None::<fn(rusqlite::hooks::AuthContext<'_>) -> rusqlite::hooks::Authorization>,
        );
        assert_eq!(
            store.load_inherited_turns().unwrap(),
            seed.turns(),
            "auxiliary corruption must not change native provenance"
        );
    }
}

#[test]
fn inherited_turn_evidence_sql_bounds_precede_rust_metadata_allocation() {
    let seed = inherited_seed(8, false, true);
    for (column, value) in [
        (
            "observation_json",
            rusqlite::types::Value::Blob(vec![b'x'; 16_385]),
        ),
        (
            "observation_turn_display",
            rusqlite::types::Value::Blob(vec![b'x'; 16_385]),
        ),
        (
            "payload_digest",
            rusqlite::types::Value::Text("a".repeat(1_048_576)),
        ),
        (
            "observation_json_witness",
            rusqlite::types::Value::Blob(vec![0; 1_048_576]),
        ),
        (
            "observation_turn_display_witness",
            rusqlite::types::Value::Blob(vec![0; 1_048_576]),
        ),
    ] {
        let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
        store.initialize_lineage(&seed).unwrap();
        store
            .lock()
            .unwrap()
            .execute_batch("PRAGMA ignore_check_constraints=ON")
            .unwrap();
        store
            .lock()
            .unwrap()
            .execute(
                &format!("UPDATE inherited_turn_provenance SET {column}=?1 WHERE ordinal=0"),
                [value],
            )
            .unwrap();
        store
            .lock()
            .unwrap()
            .execute_batch("PRAGMA ignore_check_constraints=OFF")
            .unwrap();
        store
            .lock()
            .unwrap()
            .authorizer(Some(deny_unloaded_turn_members));
        provenance::take_turn_header_read_work();
        let result = if column.starts_with("observation_turn_display") {
            store.observation_turns(Some(0), 1).map(|_| ())
        } else {
            store.conversation_turns(0, 1).map(|_| ())
        };
        assert!(result.is_err(), "{column}");
        let (rows, materialized) = provenance::take_turn_header_read_work();
        assert_eq!(rows, 1);
        assert!(materialized <= 16_384 + 64 + 32, "{column}: {materialized}");
        store.lock().unwrap().authorizer(
            None::<fn(rusqlite::hooks::AuthContext<'_>) -> rusqlite::hooks::Authorization>,
        );
    }
}

#[test]
fn inherited_turn_header_row_witness_failure_preserves_original_bootstrap() {
    let seed = inherited_seed(8, false, true);
    for column in [
        "observation_json_witness",
        "observation_turn_display_witness",
    ] {
        let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
        store.lock().unwrap().execute_batch(&format!(
            "CREATE TRIGGER optional_turn_witness BEFORE UPDATE OF {column} ON inherited_turn_provenance BEGIN SELECT RAISE(FAIL,'optional turn witness'); END;"
        )).unwrap();
        store.initialize_lineage(&seed).unwrap();
        assert_eq!(store.load_canonical().unwrap(), seed.canonical());
        assert_eq!(store.load_inherited_turns().unwrap(), seed.turns());
        assert_eq!(store.presentation_frontier().unwrap(), 0);
        let result = if column == "observation_json_witness" {
            store.conversation_turns(0, 1).map(|_| ())
        } else {
            store.observation_turns(Some(0), 1).map(|_| ())
        };
        assert!(matches!(
            crate::runtime_client::types::RuntimeClientError::observation_store_error(
                result.unwrap_err()
            ),
            crate::runtime_client::types::RuntimeClientError::ObservationUnavailable { .. },
        ));
    }
}

#[test]
fn inherited_turn_header_row_witness_columns_are_required_by_schema() {
    for column in [
        "observation_json_witness",
        "observation_turn_display_witness",
    ] {
        let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
        store
            .lock()
            .unwrap()
            .execute_batch(&format!(
                "ALTER TABLE inherited_turn_provenance DROP COLUMN {column}"
            ))
            .unwrap();
        assert!(matches!(
            verify_schema_shape(&store.lock().unwrap()),
            Err(ConversationStoreError::IncompatibleSchema(_))
        ));
    }
}

#[test]
fn turn_directory_validates_only_selected_payloads_across_large_cold_lineages() {
    let directory = tempfile::tempdir().unwrap();
    for count in [8, 128, 2048] {
        let seed = inherited_seed(count, false, true);
        let id = ConversationId::generate();
        let path = directory.path().join(format!("directory-{count}.sqlite"));
        let store = SqliteConversationStore::open(id.clone(), &path).unwrap();
        store.initialize_lineage(&seed).unwrap();
        drop(store);
        let store = SqliteConversationStore::open_existing(id, &path).unwrap();
        for (offset, limit) in [
            (0, 1),
            (count / 2, 1),
            (count - 1, 1),
            (1, 3),
            (0, 64),
            (count - 1, 64),
        ] {
            provenance::take_turn_validation_counts();
            provenance::take_turn_header_validations();
            let page = store.conversation_turns(offset, limit).unwrap();
            let counts = provenance::take_turn_validation_counts();
            let offset = offset.min(count.saturating_sub(1) / limit * limit);
            let selected = limit.min(count - offset);
            assert_eq!(counts, (0, 0));
            assert_eq!(provenance::take_turn_header_validations(), selected);
            assert_eq!(
                (page.total, page.offset, page.turns.len()),
                (count, offset, selected)
            );
            for (index, turn) in page.turns.iter().enumerate() {
                let ordinal = offset + index;
                assert_eq!(turn.id, seed.turns()[ordinal].id);
                assert_eq!(turn.ordinal, ordinal + 1);
                assert_eq!(turn.cursor.unwrap().get(), (ordinal * 2 + 1) as u64);
                assert_eq!(turn.prompt, "");
                assert_eq!(turn.response, "historical content");
            }
            println!(
                "{count} inherited Turns, offset {offset}, limit {limit}: {} payload decodes, {} membership validations",
                counts.0, counts.1
            );
        }
        let plan=store.lock().unwrap().prepare("EXPLAIN QUERY PLAN SELECT observation_json,payload_digest FROM inherited_turn_provenance WHERE ordinal=?1").unwrap().query_map([0],|row|row.get::<_,String>(3)).unwrap().collect::<Result<Vec<_>,_>>().unwrap().join("\n");
        assert!(
            plan.contains("SEARCH inherited_turn_provenance USING INTEGER PRIMARY KEY"),
            "{plan}"
        );
        assert!(!plan.contains("SCAN"), "{plan}");
        assert_eq!(store.load_inherited_turns().unwrap(), seed.turns());
    }
    let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    provenance::take_turn_validation_counts();
    assert_eq!(store.conversation_turns(99, 64).unwrap().total, 0);
    assert!(store.conversation_turns(0, 0).is_err());
    assert!(store.conversation_turns(0, 65).is_err());
    assert_eq!(provenance::take_turn_validation_counts(), (0, 0));
}

#[test]
fn turn_directory_selected_corruption_fails_without_hydrating_unrelated_turns() {
    let seed = inherited_seed(8, false, true);
    let invalid = "{}";
    let digest = |input: &[u8]| {
        use std::fmt::Write as _;
        let mut result = String::new();
        for byte in Sha256::digest(input) {
            write!(result, "{byte:02x}").unwrap();
        }
        result
    };
    let hash = digest(format!("rustx-inherited-turn\n0\n{invalid}").as_bytes());
    let wrong_owner_hash = digest(b"rustx-inherited-member\ninherited-0\nSome(1)\nNone");
    for damage in [
        "UPDATE inherited_turn_provenance SET payload_digest='bad' WHERE ordinal=0".to_string(),
        format!("UPDATE inherited_turn_provenance SET payload='{{}}',payload_digest='{hash}' WHERE ordinal=0"),
        "DELETE FROM inherited_message_membership WHERE assistant_message_id='inherited-0'".into(),
        format!("UPDATE inherited_message_membership SET turn_ordinal=1,membership_digest='{wrong_owner_hash}' WHERE assistant_message_id='inherited-0'"),
        "UPDATE inherited_message_membership SET turn_ordinal=999 WHERE assistant_message_id='inherited-0'".into(),
        "UPDATE inherited_turn_provenance SET ordinal=999 WHERE ordinal=0".into(),
        "DELETE FROM bootstrap_identity".into(),
        "UPDATE bootstrap_identity SET turn_count=9".into(),
        "DELETE FROM inherited_turn_provenance WHERE ordinal=7".into(),
    ] {
        let store=SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
        store.initialize_lineage(&seed).unwrap();
        store.lock().unwrap().execute_batch(&format!("PRAGMA foreign_keys=OFF; {damage};")).unwrap();
        provenance::take_turn_validation_counts();
        if damage.contains("inherited_message_membership") {
            assert!(store.conversation_turns(0,1).is_ok(), "unloaded membership: {damage}");
            assert!(materialized_member_process(&store, "inherited-0").is_err(), "{damage}");
        } else {
            assert!(store.conversation_turns(0,1).is_err(),"{damage}");
        }
        let counts=provenance::take_turn_validation_counts();
        assert!(counts.0<=1 && counts.1<=1,"{damage}: {counts:?}");
        assert!(store.load_inherited_turns().is_err(),"{damage}");
        assert!(store.initialize_lineage(&seed).is_err(),"{damage}");
        assert!(store.archive_frontiers().is_err(),"{damage}");
    }
    let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    store.initialize_lineage(&seed).unwrap();
    let expected = store.conversation_turns(0, 1).unwrap();
    store
        .lock()
        .unwrap()
        .execute(
            "UPDATE inherited_turn_provenance SET payload='{}' WHERE ordinal=7",
            [],
        )
        .unwrap();
    provenance::take_turn_validation_counts();
    assert_eq!(store.conversation_turns(0, 1).unwrap(), expected);
    assert_eq!(provenance::take_turn_validation_counts(), (0, 0));
    assert!(provenance::take_turn_header_validations() > 0);
    assert!(store.load_inherited_turns().is_err());
    assert!(store.initialize_lineage(&seed).is_err());
    assert!(store.read_lineage_cut(SurfaceRevision::new(1)).is_err());
    assert!(store.archive_frontiers().is_err());
    // Raw payload corruption is the full native reader's responsibility. The
    // finite descriptor does not claim to rehash an unloaded payload.
    assert!(store.conversation_turns(7, 1).is_ok());
    store
        .lock()
        .unwrap()
        .execute(
            "UPDATE inherited_turn_provenance SET observation_json=NULL WHERE ordinal=7",
            [],
        )
        .unwrap();
    assert!(store.conversation_turns(7, 1).is_err());
}

#[test]
fn inherited_terminal_only_transcript_seats_resolve_finite_native_owners_without_local_j() {
    use crate::durable::reading::{InheritedTurnOutcome, TurnProcessOutcome};
    let base = response_seed(8);
    let mut turns = base.turns().to_vec();
    let mut terminal = turns[0].clone();
    terminal.id.attempt_id = AttemptId::new("imported-terminal-only");
    terminal.process_message_ids.clear();
    terminal.preceding_message_id = Some(MessageId::new("inherited-0"));
    terminal.outcome = InheritedTurnOutcome::Cancelled;
    let original_id = terminal.id.clone();
    turns.push(terminal);
    let seed = LineageSeed::replayed(
        base.canonical().to_vec(),
        base.surface_history().to_vec(),
        base.checkpoints().clone(),
    )
    .unwrap()
    .with_turns(turns)
    .unwrap()
    .with_completed_responses(base.completed_responses().to_vec())
    .unwrap();
    let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    store.initialize_lineage(&seed).unwrap();
    let transcript: u64 = store.lock().unwrap().query_row(
        "SELECT position FROM transcript_order WHERE reference_kind='inherited_turn' AND reference_id='8'",
        [], |row| row.get(0),
    ).unwrap();
    let cut = store.observation_read_cut().unwrap();
    assert_eq!(cut.read.journal, 0);
    store
        .lock()
        .unwrap()
        .authorizer(Some(super::deny_selected_original_columns));
    provenance::take_turn_header_read_work();
    let process = store
        .observation_transcript_process(&cut, transcript)
        .unwrap()
        .unwrap();
    assert_eq!(process.conversation_id, original_id.conversation_id);
    assert_eq!(process.attempt_id.as_ref(), Some(&original_id.attempt_id));
    assert_eq!(process.attempt_id_source, None);
    assert_eq!(process.outcome, TurnProcessOutcome::Cancelled);
    assert_eq!(process.control_cursor.get(), transcript);
    assert_eq!((process.message_count, process.tool_call_count), (0, 0));
    assert!(process.final_message_id.is_none() && process.final_message_locator.is_none());
    let (rows, bytes) = provenance::take_turn_header_read_work();
    assert_eq!(rows, 1);
    assert!(bytes <= 16_384 + 64 + 32);
    store
        .lock()
        .unwrap()
        .authorizer(None::<fn(rusqlite::hooks::AuthContext<'_>) -> rusqlite::hooks::Authorization>);
    store
        .append_canonical(&assistant_message(
            "later-original-c",
            "later canonical bytes",
        ))
        .unwrap();
    assert_eq!(
        store
            .observation_transcript_process(&cut, transcript)
            .unwrap()
            .unwrap(),
        process
    );
    assert_eq!(store.read_events(None, 64).unwrap().events.len(), 0);
    assert_eq!(store.load_inherited_turns().unwrap(), seed.turns());
    store
        .lock()
        .unwrap()
        .execute(
            "UPDATE transcript_order SET reference_id='0' WHERE position=?1",
            [transcript],
        )
        .unwrap();
    assert!(
        store
            .observation_transcript_process(&cut, transcript)
            .is_err(),
        "a different real turn cannot alias the original terminal seat"
    );
    store
        .lock()
        .unwrap()
        .execute(
            "UPDATE transcript_order SET reference_id='8' WHERE position=?1",
            [transcript],
        )
        .unwrap();
    store.lock().unwrap().execute(
        "UPDATE inherited_turn_provenance SET observation_turn_display_witness=NULL WHERE ordinal=8", [],
    ).unwrap();
    assert!(
        store
            .observation_transcript_process(&cut, transcript)
            .is_err()
    );
}

#[test]
fn turn_directory_mixed_terminal_and_local_turns_reuse_exact_validated_navigation() {
    use crate::durable::reading::{ConversationWindowAt, InheritedTurnOutcome};
    let base = response_seed(8);
    let mut turns = base.turns().to_vec();
    let mut terminal = turns[0].clone();
    terminal.id.attempt_id = AttemptId::new("terminal-only");
    terminal.process_message_ids.clear();
    terminal.preceding_message_id = Some(MessageId::new("inherited-0"));
    terminal.outcome = InheritedTurnOutcome::Failed;
    turns.push(terminal);
    let seed = LineageSeed::replayed(
        base.canonical().to_vec(),
        base.surface_history().to_vec(),
        base.checkpoints().clone(),
    )
    .unwrap()
    .with_turns(turns)
    .unwrap()
    .with_completed_responses(base.completed_responses().to_vec())
    .unwrap();
    let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    store.initialize_lineage(&seed).unwrap();
    let old = store.conversation_turns(8, 1).unwrap();
    assert_eq!(old.turns[0].id, seed.turns()[8].id);
    assert!(old.turns[0].cursor.is_some());
    assert_eq!(old.turns[0].response, "");
    provenance::take_turn_validation_counts();
    provenance::take_turn_header_validations();
    let window = store
        .conversation_window(
            &ConversationWindowAt::Turn {
                id: old.turns[0].id.clone(),
                cut: old.cut.clone(),
            },
            1,
        )
        .unwrap();
    // The selected target uses finite evidence; only the actual terminal
    // transcript item invokes the independent full native materializer.
    assert_eq!(provenance::take_turn_validation_counts(), (1, 1));
    assert_eq!(provenance::take_turn_header_validations(), 1);
    assert!(matches!(
        window.page.entries[0].item,
        TranscriptItem::InheritedTurn { .. }
    ));
    for attempt in ["settled", "running"] {
        store
            .append_event(envelope(
                store.conversation_id(),
                &format!("start-{attempt}"),
                Some(AttemptId::new(attempt)),
                RuntimeEvent::AttemptStarted {
                    attempt_id: AttemptId::new(attempt),
                },
            ))
            .unwrap();
        if attempt == "settled" {
            store
                .append_event(envelope(
                    store.conversation_id(),
                    "terminal-settled",
                    Some(AttemptId::new(attempt)),
                    RuntimeEvent::AttemptTimedOut {
                        attempt_id: AttemptId::new(attempt),
                    },
                ))
                .unwrap();
        }
    }
    provenance::take_turn_validation_counts();
    provenance::take_turn_header_validations();
    let mixed = store.conversation_turns(8, 3).unwrap();
    assert_eq!(provenance::take_turn_validation_counts(), (0, 0));
    assert_eq!(provenance::take_turn_header_validations(), 1);
    assert_eq!(mixed.total, 11);
    assert_eq!(mixed.turns[0], old.turns[0]);
    assert!(mixed.turns[1].cursor.is_some());
    assert!(mixed.turns[2].cursor.is_none());
    assert_eq!(mixed.turns[1].id.conversation_id, *store.conversation_id());
    let repeated = store
        .conversation_window(
            &ConversationWindowAt::Turn {
                id: old.turns[0].id.clone(),
                cut: old.cut.clone(),
            },
            1,
        )
        .unwrap();
    assert_eq!(repeated.cut, window.cut);
    assert_eq!(repeated.page, window.page);
    assert_eq!(repeated.target, window.target);
    assert_eq!(repeated.target_cursor, window.target_cursor);
    assert_eq!(repeated.newer_cursor, window.newer_cursor);
    let frontier = store.presentation_frontier().unwrap();
    store.conversation_turns(0, 64).unwrap();
    assert_eq!(store.presentation_frontier().unwrap(), frontier);
    store.lock().unwrap().execute("DELETE FROM transcript_order WHERE reference_kind='inherited_turn' AND reference_id='8'",[]).unwrap();
    assert!(store.conversation_turns(8, 1).is_err());
    assert!(store.load_inherited_turns().is_err());
}

#[test]
fn inherited_turn_membership_is_exact_in_both_directions_with_valid_payload_digests() {
    fn hash(input: &str) -> String {
        use std::fmt::Write as _;
        let mut result = String::new();
        for byte in Sha256::digest(input.as_bytes()) {
            write!(result, "{byte:02x}").unwrap();
        }
        result
    }
    let seed = response_seed(8);
    assert_eq!(seed.turns()[0].process_message_ids.len(), 2);
    for damage in [
        "omitted",
        "extra",
        "duplicate",
        "wrong-owner",
        "digest",
        "missing",
        "reverse-extra",
    ] {
        let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
        store.initialize_lineage(&seed).unwrap();
        assert_eq!(
            store
                .inherited_turns_for(&seed.turns()[0].process_message_ids)
                .unwrap(),
            seed.turns()[..1]
        );
        let mut turn = seed.turns()[0].clone();
        let connection = store.lock().unwrap();
        match damage {
            "omitted" => {
                turn.process_message_ids.pop();
            }
            "extra" => {
                turn.process_message_ids
                    .push(seed.turns()[1].process_message_ids[0].clone());
            }
            "duplicate" => {
                turn.process_message_ids
                    .push(turn.process_message_ids[0].clone());
            }
            "wrong-owner" => {
                let digest = hash("rustx-inherited-member\nearly-closing\nSome(1)\nSome(0)");
                connection.execute("UPDATE inherited_message_membership SET turn_ordinal=1,membership_digest=?1 WHERE assistant_message_id='early-closing'", [digest]).unwrap();
            }
            "digest" => {
                connection.execute("UPDATE inherited_message_membership SET membership_digest='bad' WHERE assistant_message_id='early-closing'", []).unwrap();
            }
            "missing" => {
                connection.execute("DELETE FROM inherited_message_membership WHERE assistant_message_id='early-closing'", []).unwrap();
            }
            "reverse-extra" => {
                let digest = hash("rustx-inherited-member\ninherited-1\nSome(0)\nSome(1)");
                connection.execute("UPDATE inherited_message_membership SET turn_ordinal=0,membership_digest=?1 WHERE assistant_message_id='inherited-1'", [digest]).unwrap();
            }
            _ => unreachable!(),
        }
        // The independent full native reader must reach membership checks,
        // rather than reject merely because a previous digest was left stale.
        // Finite descriptors compare the captured native digest separately.
        let payload = serde_json::to_string(&turn).unwrap();
        let digest = hash(&format!("rustx-inherited-turn\n0\n{payload}"));
        connection
            .execute(
                "UPDATE inherited_turn_provenance SET payload=?1,payload_digest=?2 WHERE ordinal=0",
                params![payload, digest],
            )
            .unwrap();
        drop(connection);
        provenance::take_turn_validation_counts();
        if matches!(damage, "omitted" | "extra" | "duplicate") {
            assert!(store.conversation_turns(0, 1).is_err(), "{damage}");
        } else {
            assert!(
                store.conversation_turns(0, 1).is_ok(),
                "unloaded membership: {damage}"
            );
            let selected = if damage == "reverse-extra" {
                "inherited-1"
            } else {
                "early-closing"
            };
            assert!(
                materialized_member_process(&store, selected).is_err(),
                "{damage}"
            );
        }
        assert_eq!(
            provenance::take_turn_validation_counts(),
            (0, 0),
            "{damage}"
        );
        assert!(
            store
                .inherited_turns_for(&[MessageId::new("inherited-0")])
                .is_err(),
            "{damage}"
        );
        assert!(store.load_inherited_turns().is_err(), "{damage}");
        assert!(store.initialize_lineage(&seed).is_err(), "{damage}");
        assert!(store.archive_frontiers().is_err(), "{damage}");
    }
}

#[test]
fn inherited_turn_reverse_corruption_is_scoped_to_materialized_records() {
    fn hash(input: &str) -> String {
        use std::fmt::Write as _;
        let mut result = String::new();
        for byte in Sha256::digest(input.as_bytes()) {
            write!(result, "{byte:02x}").unwrap();
        }
        result
    }
    let seed = response_seed(8);
    let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    store.initialize_lineage(&seed).unwrap();
    let expected = store.conversation_turns(0, 1).unwrap();
    let mut turn = seed.turns()[7].clone();
    turn.process_message_ids.clear();
    let payload = serde_json::to_string(&turn).unwrap();
    let digest = hash(&format!("rustx-inherited-turn\n7\n{payload}"));
    store
        .lock()
        .unwrap()
        .execute(
            "UPDATE inherited_turn_provenance SET payload=?1,payload_digest=?2 WHERE ordinal=7",
            params![payload, digest],
        )
        .unwrap();
    provenance::take_turn_validation_counts();
    provenance::take_turn_header_validations();
    assert_eq!(store.conversation_turns(0, 1).unwrap(), expected);
    assert_eq!(provenance::take_turn_validation_counts(), (0, 0));
    assert_eq!(provenance::take_turn_header_validations(), 1);
    assert_eq!(
        store
            .inherited_turns_for(&[MessageId::new("inherited-0")])
            .unwrap(),
        seed.turns()[..1]
    );
    assert!(store.conversation_turns(7, 1).is_err());
    assert!(store.load_inherited_turns().is_err());
    assert!(store.initialize_lineage(&seed).is_err());
    assert!(store.archive_frontiers().is_err());
}

#[test]
fn inherited_turn_reverse_membership_seeks_only_the_selected_ordinal() {
    let mut costs = Vec::new();
    for count in [8, 128, 2048] {
        let seed = response_seed(count);
        let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
        store.initialize_lineage(&seed).unwrap();
        let plan = store
            .lock()
            .unwrap()
            .prepare(&format!("EXPLAIN QUERY PLAN {}", provenance::TURN_MEMBERS))
            .unwrap()
            .query_map([0], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .join("\n");
        assert!(plan.contains("SEARCH inherited_message_membership USING COVERING INDEX inherited_membership_turn_idx (turn_ordinal=?)"), "{plan}");
        assert!(!plan.contains("SCAN"), "{plan}");
        let (members, instructions) = measured_vm(&store, || {
            store
                .lock()
                .unwrap()
                .prepare_cached(provenance::TURN_MEMBERS)
                .unwrap()
                .query_map([0], |row| row.get::<_, String>(0))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        });
        assert_eq!(members, ["early-closing", "inherited-0"]);
        provenance::take_turn_validation_counts();
        assert_eq!(
            store
                .inherited_turns_for(&[MessageId::new("inherited-0")])
                .unwrap(),
            seed.turns()[..1]
        );
        assert_eq!(provenance::take_turn_validation_counts(), (1, 1));
        println!(
            "{count} inherited Turns: reverse lookup {instructions} VM instructions, 2 rows, one selected payload; {plan}"
        );
        costs.push(instructions);
    }
    assert!(costs.iter().all(|cost| *cost == costs[0]), "{costs:?}");
}
