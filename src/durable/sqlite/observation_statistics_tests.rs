// Bounded aggregate reads retain the exact original-J fold semantics.
struct StatisticsBodyFixture {
    store: SqliteConversationStore,
    attempt_start: RuntimeEventEnvelope,
    request_start: RuntimeEventEnvelope,
    terminal: RuntimeEventEnvelope,
}
fn statistics_body_fixture() -> StatisticsBodyFixture {
    let store = store();
    // Actual model input remains native-admissible (<1 MiB per Original);
    // growing J/R diagnostics below still exceed the finite display limits.
    store
        .initialize(&[user_message("stats-input", &"original🙂水".repeat(30_000))])
        .unwrap();
    let base = Utc.with_ymd_and_hms(2026, 8, 7, 12, 0, 0).unwrap();
    let attempt = AttemptId::new("stats-source-attempt");
    let started = store
        .append_event(envelope(
            store.conversation_id(),
            "stats-start",
            Some(attempt.clone()),
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt.clone(),
            },
        ))
        .unwrap();
    let mut snapshot = progress_request_snapshot(
        attempt.as_str(),
        "1",
        0,
        store.load_head().unwrap().revision,
        Vec::new(),
    );
    snapshot.effective_system_prompt = "native growing prompt".repeat(130_000);
    snapshot.invocation.model = "original model🙂水".repeat(150_000);
    freeze_test_input(&store, &mut snapshot, &[]);
    let request = store
        .commit_model_turn_start(&[], &snapshot, base + chrono::Duration::seconds(1))
        .unwrap();
    let mut completed = envelope(
        store.conversation_id(),
        "stats-request-done",
        Some(attempt.clone()),
        RuntimeEvent::ModelRequestCompleted {
            request_id: snapshot.request_id.clone(),
            finish_reason: ModelFinishReason::Other {
                reason: "native opaque reason".repeat(130_000),
            },
            usage: None,
            generation: None,
        },
    );
    completed.turn_id = Some(snapshot.identity.turn.clone());
    completed.timestamp = base + chrono::Duration::seconds(4);
    store.append_event(completed).unwrap();
    let mut terminal = envelope(
        store.conversation_id(),
        "stats-terminal",
        Some(attempt.clone()),
        RuntimeEvent::AttemptCompleted {
            attempt_id: attempt,
            finish_reason: ModelFinishReason::Other {
                reason: "original terminal🙂水".repeat(130_000),
            },
        },
    );
    terminal.timestamp = base + chrono::Duration::seconds(5);
    let terminal = store.append_event(terminal).unwrap();
    StatisticsBodyFixture {
        store,
        attempt_start: started,
        request_start: request.started,
        terminal,
    }
}
#[test]
fn t08_t14_t19_agent_statistics_use_fixed_headers_with_giant_original_bodies() {
    use crate::runtime_client::agent_statistics::StatisticsFold;
    let StatisticsBodyFixture {
        store,
        attempt_start: started,
        request_start,
        terminal,
    } = statistics_body_fixture();
    let original = store.read_events(None, 32).unwrap();
    let expected = StatisticsFold::reference_events(&original.events);
    let message_reads = store.checkpoint_read_counts[0].load(std::sync::atomic::Ordering::Relaxed);
    let request_reads = store
        .request_snapshot_body_reads
        .load(std::sync::atomic::Ordering::Relaxed);
    observation_statistics::read_work();
    let actual = store
        .observation_agent_statistics(terminal.sequence)
        .unwrap();
    assert_eq!(actual, expected);
    assert_eq!(actual.statistics.turns, 1);
    assert_eq!(actual.statistics.model_requests, 1);
    assert_eq!(actual.duration.settled_ms, 5_000);
    assert!(actual.duration.active.is_none());
    let (rows, bytes, original_reads) = observation_statistics::read_work();
    assert_eq!(rows, 1);
    assert!(bytes <= 16_384);
    assert_eq!(original_reads, 0);
    let earlier = store
        .observation_agent_statistics(request_start.sequence)
        .unwrap();
    let (rows, bytes, original_reads) = observation_statistics::read_work();
    assert_eq!(rows, 1);
    assert!(bytes <= 16_384);
    assert_eq!(original_reads, 0);
    assert_eq!(
        earlier.duration.active.as_ref().unwrap().started_at,
        started.timestamp
    );
    assert_eq!(
        earlier.duration.active.unwrap().observed_at,
        request_start.timestamp
    );
    assert_eq!(earlier.duration.settled_ms, 0);
    assert_eq!(
        store.checkpoint_read_counts[0].load(std::sync::atomic::Ordering::Relaxed),
        message_reads
    );
    assert_eq!(
        store
            .request_snapshot_body_reads
            .load(std::sync::atomic::Ordering::Relaxed),
        request_reads
    );
}

#[test]
fn t10_t14_t19_statistics_header_fault_preserves_original_terminal_and_explicit_hole() {
    let store = store();
    let attempt = AttemptId::new("statistics-header-fault");
    let start = store
        .append_event(envelope(
            store.conversation_id(),
            "statistics-header-start",
            Some(attempt.clone()),
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt.clone(),
            },
        ))
        .unwrap();
    let before = store.observation_agent_statistics(start.sequence).unwrap();
    store.lock().unwrap().execute_batch("CREATE TRIGGER statistics_header_fault BEFORE UPDATE OF observation_statistics ON events BEGIN SELECT RAISE(FAIL,'derived statistics fault'); END;").unwrap();
    let terminal = store
        .append_event(envelope(
            store.conversation_id(),
            "statistics-header-terminal",
            Some(attempt.clone()),
            RuntimeEvent::AttemptTimedOut {
                attempt_id: attempt,
            },
        ))
        .unwrap();
    assert_eq!(
        store.read_events(Some(start.sequence), 1).unwrap().events[0].event,
        terminal.event
    );
    assert!(matches!(
        store.observation_agent_statistics(terminal.sequence),
        Err(ConversationStoreError::Storage(_))
    ));
    assert_eq!(
        store.observation_agent_statistics(start.sequence).unwrap(),
        before
    );
    let connection = store.lock().unwrap();
    let plan: String = connection.query_row("EXPLAIN QUERY PLAN SELECT sequence FROM events INDEXED BY events_observation_statistics_holes_idx WHERE observation_statistics_required=1 AND observation_statistics IS NULL AND sequence<=?1 LIMIT 1", [terminal.sequence], |row| row.get(3)).unwrap();
    assert!(
        plan.contains("events_observation_statistics_holes_idx"),
        "{plan}"
    );
}

#[test]
fn t14_t19_local_agent_statistics_exclude_inherited_aggregates() {
    let store = store();
    store
        .initialize_lineage(&statistics_inherited_seed())
        .unwrap();
    assert_eq!(store.observation_statistics(0).unwrap().model_requests, 9);
    assert_eq!(
        store.observation_agent_statistics(0).unwrap(),
        crate::runtime_client::agent_statistics::AgentStatistics::default()
    );
}

#[test]
fn t08_t14_t19_compaction_checkpoint_view_keeps_canonical_history_immutable() {
    let store = store();
    let original = user_message("checkpoint-input", "native original");
    store.initialize(std::slice::from_ref(&original)).unwrap();
    let event = occupancy_origin_compact(
        &store,
        "checkpoint-input",
        "native-checkpoint",
        &occupancy_origin_native("exact model", 200),
    );
    let witness = store
        .observation_event_witness(event.sequence, event.sequence)
        .unwrap()
        .unwrap();
    let compaction = witness.occupancy.unwrap().compaction;
    assert_eq!(compaction.checkpoint_id.as_str(), "native-checkpoint");
    assert_eq!(compaction.activation_id.0, 1);
    assert!(serde_json::to_vec(&compaction).unwrap().len() <= 16_384);
    assert_eq!(store.load_canonical().unwrap(), vec![original]);
    assert_eq!(
        store
            .projection_head()
            .unwrap()
            .unwrap()
            .candidate
            .checkpoint_id,
        compaction.checkpoint_id
    );
}

fn statistics_inherited_seed() -> LineageSeed {
    let inherited = crate::durable::reading::TurnReadingProvenance {
        id: crate::durable::reading::ConversationTurnId {
            conversation_id: ConversationId::new("conv_01900000-0000-7000-8000-000000000021"),
            attempt_id: AttemptId::new("inherited-statistics"),
        },
        process_message_ids: vec![MessageId::new("inherited-stats-answer")],
        preceding_message_id: None,
        prompt_message_id: Some(MessageId::new("inherited-stats-input")),
        outcome: crate::durable::reading::InheritedTurnOutcome::Completed,
        started_at: None,
        ended_at: None,
        execution: Some(crate::durable::reading::TurnExecution {
            model_requests: 9,
            ..Default::default()
        }),
    };
    LineageSeed::history(vec![
        user_message("inherited-stats-input", "original"),
        assistant_message("inherited-stats-answer", "answer"),
    ])
    .with_turns(vec![inherited])
    .unwrap()
}

#[test]
fn t10_t14_t19_statistics_seed_fault_preserves_original_lineage_and_explicit_unavailability() {
    let store = store();
    assert_eq!(
        store.observation_statistics(0).unwrap(),
        crate::runtime_client::response::ConversationStatistics::default()
    );
    store.lock().unwrap().execute_batch("CREATE TRIGGER statistics_seed_fault BEFORE UPDATE OF observation_statistics ON rustx_store BEGIN SELECT RAISE(FAIL,'derived statistics seed fault'); END;").unwrap();
    store
        .initialize_lineage(&statistics_inherited_seed())
        .unwrap();
    let originals = store
        .load_messages(&[
            MessageId::new("inherited-stats-input"),
            MessageId::new("inherited-stats-answer"),
        ])
        .unwrap();
    assert_eq!(originals.len(), 2);
    assert!(
        matches!(&originals[0], MessageBlock::User(user) if user.id == MessageId::new("inherited-stats-input"))
    );
    assert!(
        matches!(&originals[1], MessageBlock::Assistant(assistant) if assistant.id == MessageId::new("inherited-stats-answer"))
    );
    assert!(
        store
            .lock()
            .unwrap()
            .query_row(
                "SELECT bootstrap_initialized FROM rustx_store WHERE id=1",
                [],
                |row| row.get::<_, bool>(0)
            )
            .unwrap()
    );
    assert!(matches!(
        store.observation_statistics(0),
        Err(ConversationStoreError::Storage(_))
    ));
    assert!(matches!(
        store.observation_agent_statistics(0),
        Err(ConversationStoreError::Storage(_))
    ));
    let empty = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    empty.initialize(&[]).unwrap();
    assert_eq!(
        empty.observation_statistics(0).unwrap(),
        crate::runtime_client::response::ConversationStatistics::default()
    );
}

#[test]
fn t08_t14_t19_giant_attempt_clock_keeps_counts_times_and_original_journal_identity() {
    let store = store();
    store.initialize(&[]).unwrap();
    let attempt = AttemptId::new("native-attempt🙂水".repeat(70_000));
    let started = store
        .append_event(envelope(
            store.conversation_id(),
            "giant-clock-start",
            Some(attempt.clone()),
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt.clone(),
            },
        ))
        .unwrap();
    let cut = crate::runtime_client::trace_observation::history_cut(&store).unwrap();
    let reading = store.observation_statistics(started.sequence).unwrap();
    let clock = reading.latest_turn.as_ref().unwrap();
    assert_eq!(reading.turns, 1);
    assert!(clock.attempt_id.is_none());
    assert!(clock.attempt_id_deferred);
    assert_eq!(clock.started_at, started.timestamp);
    assert!(clock.ended_at.is_none());
    assert!(serde_json::to_vec(&reading).unwrap().len() <= 16_384);
    let source = clock.attempt_id_source.as_ref().unwrap();
    assert_eq!(source.sequence, started.sequence);
    assert_eq!(source.path, ["attempt_id"]);
    assert!(source.encoded_bytes > 65_536);
    // The kind and original indexed field remain independent of a missing
    // display witness. Authorization reads neither giant original J nor witness.
    store
        .lock()
        .unwrap()
        .execute(
            "UPDATE events SET observation_witness=NULL WHERE sequence=?1",
            [started.sequence],
        )
        .unwrap();
    assert_eq!(
        statistics_read_attempt_identity(&store, &cut, source),
        attempt.as_str()
    );
    let domain = format!("journal.{}", source.sequence);
    assert!(
        crate::runtime_client::journal_observation::value_read(
            &store,
            &cut,
            &domain,
            0,
            &["event".into(), "error".into()],
            0,
            65_536
        )
        .is_err()
    );
    let mut terminal = envelope(
        store.conversation_id(),
        "giant-clock-end",
        Some(attempt.clone()),
        RuntimeEvent::AttemptTimedOut {
            attempt_id: attempt,
        },
    );
    terminal.timestamp = started.timestamp + chrono::Duration::seconds(3);
    let terminal = store.append_event(terminal).unwrap();
    let after = store.observation_statistics(terminal.sequence).unwrap();
    assert_eq!(after.turns, 1);
    assert_eq!(
        after.latest_turn.as_ref().unwrap().ended_at,
        Some(terminal.timestamp)
    );
    assert_eq!(
        after.latest_turn.unwrap().attempt_id_source,
        clock.attempt_id_source
    );
    assert_eq!(
        store
            .observation_agent_statistics(terminal.sequence)
            .unwrap()
            .duration
            .settled_ms,
        3_000
    );
    assert_eq!(
        store.observation_statistics(started.sequence).unwrap(),
        reading
    );
}

fn statistics_read_attempt_identity(
    store: &SqliteConversationStore,
    cut: &crate::runtime_client::checkpoint::CheckpointReadCut,
    source: &crate::runtime_client::snapshot::JournalFieldRef,
) -> String {
    use base64::Engine;
    let domain = format!("journal.{}", source.sequence);
    let mut bytes = Vec::new();
    let mut offset = 0;
    loop {
        let chunk = crate::runtime_client::journal_observation::value_read(
            store,
            cut,
            &domain,
            0,
            &source.path,
            offset,
            65_536,
        )
        .unwrap();
        let original = base64::engine::general_purpose::STANDARD
            .decode(chunk.bytes_base64)
            .unwrap();
        assert!(original.len() <= 65_536);
        assert!(!original.is_empty());
        bytes.extend(original);
        let Some(next) = chunk.next_offset else {
            break;
        };
        assert!(next > offset);
        offset = next;
    }
    assert_eq!(bytes.len() as u64, source.encoded_bytes);
    serde_json::from_slice::<String>(&bytes).unwrap()
}

#[test]
fn compact_statistics_preserve_every_historical_cut_and_lineage_after_reopen() {
    use self::observation_response_tests::{response_complete, response_message, response_start};
    use crate::runtime_client::agent_statistics::StatisticsFold;
    for inherited in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("compact-statistics.sqlite");
        let conversation = ConversationId::generate();
        let store = SqliteConversationStore::open(conversation.clone(), &path).unwrap();
        if inherited {
            store
                .initialize_lineage(&statistics_inherited_seed())
                .unwrap();
        } else {
            store.initialize(&[]).unwrap();
        }
        let first = response_start(&store, "compact-statistics-first");
        response_message(&store, &first, "compact-statistics-answer");
        response_complete(&store, &first);
        let second = response_start(&store, "compact-statistics-second");
        store
            .append_event(envelope(
                store.conversation_id(),
                "compact-statistics-timeout",
                Some(second.clone()),
                RuntimeEvent::AttemptTimedOut { attempt_id: second },
            ))
            .unwrap();
        let events = store.read_events(None, 32).unwrap().events;
        let expected: Vec<_> = events
            .iter()
            .map(|event| {
                let prefix: Vec<_> = events
                    .iter()
                    .filter(|item| item.sequence <= event.sequence)
                    .cloned()
                    .collect();
                let local = StatisticsFold::reference_events(&prefix);
                assert_eq!(
                    store.observation_agent_statistics(event.sequence).unwrap(),
                    local
                );
                let mut whole = local.statistics.clone();
                if inherited {
                    whole.turns += 1;
                    whole.model_requests += 9;
                }
                assert_eq!(store.observation_statistics(event.sequence).unwrap(), whole);
                (event.sequence, local, whole)
            })
            .collect();
        drop(store);
        let reopened = SqliteConversationStore::open(conversation, &path).unwrap();
        for (sequence, local, whole) in expected {
            assert_eq!(
                reopened.observation_agent_statistics(sequence).unwrap(),
                local
            );
            assert_eq!(reopened.observation_statistics(sequence).unwrap(), whole);
        }
    }
}
