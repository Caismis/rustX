#[derive(Debug, PartialEq)]
struct EventSourceOracle {
    witness: Vec<u8>,
    fields: Vec<Vec<rusqlite::types::Value>>,
    segments: Vec<Vec<rusqlite::types::Value>>,
    frontiers: Vec<Vec<rusqlite::types::Value>>,
}
fn event_source_rows(
    connection: &Connection,
    sql: &str,
    id: &str,
) -> Vec<Vec<rusqlite::types::Value>> {
    let mut statement = connection.prepare(sql).unwrap();
    let columns = statement.column_count();
    statement
        .query_map([id], |row| {
            (0..columns).map(|column| row.get(column)).collect()
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}
fn event_source_oracle(connection: &Connection, sequence: u64) -> EventSourceOracle {
    let id = format!("journal/{sequence}");
    let oracle = EventSourceOracle {
        witness: connection
            .query_row(
                "SELECT observation_witness FROM events WHERE sequence=?1",
                [sequence],
                |row| row.get(0),
            )
            .unwrap(),
        fields: event_source_rows(
            connection,
            "SELECT * FROM state_observation_fields WHERE source_id=?1 ORDER BY path_json",
            &id,
        ),
        segments: event_source_rows(
            connection,
            "SELECT * FROM content_segments WHERE source_kind='oj' AND source_id=?1 ORDER BY segment",
            &id,
        ),
        frontiers: event_source_rows(
            connection,
            "SELECT * FROM content_frontiers WHERE source_kind='oj' AND source_id=?1",
            &id,
        ),
    };
    assert!(!oracle.fields.is_empty(), "actual event field rows");
    assert!(oracle.frontiers.iter().any(|row| matches!(row.last(), Some(rusqlite::types::Value::Blob(bytes)) if !bytes.is_empty())), "actual original event first bytes retained inline");
    assert!(
        !oracle.frontiers.is_empty(),
        "actual original event frontier"
    );
    oracle
}
fn rebuild_event_source(
    connection: &Connection,
    event: &RuntimeEventEnvelope,
    legacy: bool,
) -> (EventSourceOracle, (usize, usize)) {
    let transaction = connection.unchecked_transaction().unwrap();
    let id = format!("journal/{}", event.sequence);
    for table in [
        "state_observation_fields",
        "content_segments",
        "content_frontiers",
    ] {
        transaction
            .execute(&format!("DELETE FROM {table} WHERE source_id=?1"), [&id])
            .unwrap();
    }
    observation_event::take_preview_work();
    let available = if legacy {
        observation_event::index_legacy(&transaction, event)
    } else {
        observation_event::index_segmented_for_test(&transaction, event)
    }
    .unwrap();
    assert!(available);
    let work = observation_event::take_preview_work();
    let oracle = event_source_oracle(&transaction, event.sequence);
    transaction.rollback().unwrap();
    (oracle, work)
}
fn assert_event_probe_oracle(
    store: &SqliteConversationStore,
    event: &RuntimeEventEnvelope,
    expected_work: (usize, usize),
) {
    let original_journal = store.read_events(None, 64).unwrap().events;
    let original_canonical = store.load_canonical().unwrap();
    let original_cut = store.observation_read_cut().unwrap();
    let connection = store.lock().unwrap();
    let (legacy, legacy_work) = rebuild_event_source(&connection, event, true);
    assert_eq!(legacy_work, (24, 1));
    let (current, work) = rebuild_event_source(&connection, event, false);
    assert_eq!(work, expected_work);
    assert_eq!(
        current, legacy,
        "all witness, field geometry/headers, original segments/frontiers"
    );
    assert_inline_event_oracle(&connection, event);
    drop(connection);
    assert_eq!(
        store.read_events(None, 64).unwrap().events,
        original_journal
    );
    assert_eq!(store.load_canonical().unwrap(), original_canonical);
    assert_eq!(store.observation_read_cut().unwrap(), original_cut);
}

#[test]
fn common_event_headers_defer_all_full_source_construction_and_match_the_oracle() {
    use super::observation_response_tests::{response_complete, response_message, response_start};
    let store = store();
    store.initialize(&[]).unwrap();
    observation_event::take_capture_work();
    let attempt = response_start(&store, "deferred-source");
    let historical = store.observation_read_cut().unwrap();
    response_message(&store, &attempt, "deferred-source-message");
    response_complete(&store, &attempt);
    assert_eq!(
        observation_event::take_capture_work(),
        (0, 3),
        "start, message and terminal headers need no encoded/parsed full Source"
    );
    let events = store.read_events(None, 32).unwrap().events;
    let connection = store.lock().unwrap();
    for event in &events {
        let digest: Vec<u8> = connection
            .query_row(
                "SELECT observation_source FROM events WHERE sequence=?1",
                [event.sequence],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(digest.len(), 33);
        assert_eq!(digest[0], 0);
        assert_inline_event_oracle(&connection, event);
    }
    assert_eq!(
        observation_event::read(
            &connection,
            events[0].sequence,
            historical.read.journal,
            &["event".into(), "type".into()],
            0,
            64,
        )
        .unwrap()
        .bytes,
        br#""attempt_started""#
    );
}

#[test]
fn deferred_headers_keep_exact_unknown_finish_reason_deferral_at_the_boundary() {
    for length in [256, 257] {
        let store = store();
        store.initialize(&[]).unwrap();
        observation_event::take_capture_work();
        let attempt = AttemptId::new("deferred-finish-owner");
        let event = store
            .append_event(envelope(
                store.conversation_id(),
                "deferred-finish",
                Some(attempt.clone()),
                RuntimeEvent::AttemptCompleted {
                    attempt_id: attempt,
                    finish_reason: ModelFinishReason::Other {
                        reason: "x".repeat(length),
                    },
                },
            ))
            .unwrap();
        assert_eq!(
            observation_event::take_capture_work(),
            if length == 256 { (0, 1) } else { (1, 0) }
        );
        assert_inline_event_oracle(&store.lock().unwrap(), &event);
    }
}

#[test]
fn nonfinite_progress_keeps_its_exact_overlay_instead_of_typed_reconstruction() {
    use sha2::Digest as _;
    let store = store();
    store.initialize(&[]).unwrap();
    let attempt = AttemptId::new("nonfinite-source-owner");
    let (proposal, call) = derived_enrichment_proposal();
    store
        .append_canonical_with_event(
            &proposal,
            envelope(
                store.conversation_id(),
                "nonfinite-proposal",
                Some(attempt.clone()),
                RuntimeEvent::AssistantMessageCommitted {
                    message_id: proposal.id().clone(),
                },
            ),
        )
        .unwrap();
    store
        .append_event(envelope(
            store.conversation_id(),
            "nonfinite-tool-start",
            Some(attempt.clone()),
            RuntimeEvent::ToolExecutionStarted {
                tool_call_id: call.id.clone(),
                tool_id: call.tool_id.clone(),
            },
        ))
        .unwrap();
    let progress = store
        .append_event(envelope(
            store.conversation_id(),
            "nonfinite-progress",
            Some(attempt),
            RuntimeEvent::ToolExecutionProgress {
                tool_call_id: call.id,
                tool_id: call.tool_id,
                execution_id: None,
                progress: crate::tools::types::ToolProgress {
                    message: None,
                    completed: Some(f64::NAN),
                    total: Some(f64::INFINITY),
                },
            },
        ))
        .unwrap();
    let connection = store.lock().unwrap();
    assert_inline_event_oracle(&connection, &progress);
    let (native, record): (String, Vec<u8>) = connection
        .query_row(
            "SELECT event_json,observation_source FROM events WHERE sequence=?1",
            [progress.sequence],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        record[0], 1,
        "this representation cannot be regenerated by typed decoding"
    );
    // A forged native marker must not silently normalize null Option fields.
    let mut forged = vec![0];
    forged.extend_from_slice(&sha2::Sha256::digest(native.as_bytes()));
    let transaction = connection.unchecked_transaction().unwrap();
    transaction
        .execute(
            "UPDATE events SET observation_source=?2 WHERE sequence=?1",
            params![progress.sequence, forged],
        )
        .unwrap();
    assert!(
        observation_event::read(
            &transaction,
            progress.sequence,
            progress.sequence,
            &[],
            0,
            65_536
        )
        .is_err()
    );
    transaction.rollback().unwrap();
}

#[test]
fn deferred_native_event_schema_refuses_the_old_overlay_database() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("old-event-overlay.sqlite");
    let id = ConversationId::generate();
    let store = SqliteConversationStore::open(id.clone(), &path).unwrap();
    store
        .lock()
        .unwrap()
        .execute("UPDATE rustx_store SET schema_version=88", [])
        .unwrap();
    drop(store);
    assert!(matches!(
        SqliteConversationStore::open_existing(id, &path),
        Err(ConversationStoreError::SchemaVersionMismatch {
            stored: 88,
            expected: 89
        })
    ));
}

#[test]
fn t08_t14_t19_event_probe_pruning_preserves_original_progress_sources_and_large_id_deferral() {
    for giant_ids in [false, true] {
        let store = store();
        store.initialize(&[]).unwrap();
        let attempt = AttemptId::new("probe-original-owner");
        let (mut proposal, mut call) = derived_enrichment_proposal();
        if giant_ids {
            call.id = ToolCallId::new("original-call水".repeat(80));
            call.tool_id = ToolId::new("original-tool🙂".repeat(80));
            let MessageBlock::Assistant(message) = &mut proposal else {
                unreachable!()
            };
            message.content = vec![AssistantContentBlock::ToolCall(call.clone())];
        }
        let (committed, _) = store
            .append_canonical_with_event(
                &proposal,
                envelope(
                    store.conversation_id(),
                    "probe-proposal",
                    Some(attempt.clone()),
                    RuntimeEvent::AssistantMessageCommitted {
                        message_id: proposal.id().clone(),
                    },
                ),
            )
            .unwrap();
        assert_event_probe_oracle(&store, &committed, (1, 0));
        let started = store
            .append_event(envelope(
                store.conversation_id(),
                "probe-tool-start",
                Some(attempt.clone()),
                RuntimeEvent::ToolExecutionStarted {
                    tool_call_id: call.id.clone(),
                    tool_id: call.tool_id.clone(),
                },
            ))
            .unwrap();
        assert_event_probe_oracle(&store, &started, (24, 1));
        for (index, message) in [None, Some(String::new()), Some("水🙂".repeat(73))]
            .into_iter()
            .enumerate()
        {
            let progress = crate::tools::types::ToolProgress {
                message,
                completed: Some(0.125),
                total: Some(3.5),
            };
            let progress = store
                .append_event(envelope(
                    store.conversation_id(),
                    &format!("probe-progress-{index}"),
                    Some(attempt.clone()),
                    RuntimeEvent::ToolExecutionProgress {
                        tool_call_id: call.id.clone(),
                        tool_id: call.tool_id.clone(),
                        execution_id: None,
                        progress,
                    },
                ))
                .unwrap();
            assert_event_probe_oracle(&store, &progress, (4, 0));
        }
        // An unselected family retains the complete path policy even for
        // original giant text and opaque JSON numeric/property lookalikes.
        let result = store
            .append_event(envelope(
                store.conversation_id(),
                "probe-tool-result",
                Some(attempt),
                RuntimeEvent::ToolExecutionCompleted {
                    tool_call_id: call.id,
                    tool_id: call.tool_id,
                    result: original_success_tool_result(),
                },
            ))
            .unwrap();
        assert_event_probe_oracle(&store, &result, (24, 1));
    }
}

fn probe_generation() -> crate::model::generation_evidence::GenerationEvidence {
    crate::model::generation_evidence::GenerationEvidence {
        dispatch_after_start_ms: Some(1),
        first_output_ms: Some(2),
        last_output_ms: Some(3),
        terminal_ms: 4,
    }
}
#[test]
fn t08_t14_t19_event_probe_pruning_preserves_native_request_terminals_and_optional_generation() {
    for failed in [false, true] {
        for generation in [None, Some(probe_generation())] {
            let store = store();
            store
                .initialize(&[user_message("probe-input", "native input")])
                .unwrap();
            let mut snapshot =
                test_request_snapshot(store.load_head().unwrap().revision, invocation());
            freeze_test_input(&store, &mut snapshot, &[]);
            let started = store
                .commit_model_turn_start(&[], &snapshot, Utc::now())
                .unwrap()
                .started;
            assert_event_probe_oracle(&store, &started, (1, 0));
            let finish_reason = ModelFinishReason::Other {
                reason: "original 水🙂\\\"\n".repeat(2000),
            };
            let mut error = crate::model::error::ModelError::timeout(
                crate::model::deadline::ModelTimeoutPhase::ResponseStart,
            );
            error.message = "original timeout 水🙂\\\"\n".repeat(2000);
            let kind = if failed {
                RuntimeEvent::ModelRequestFailed {
                    request_id: snapshot.request_id.clone(),
                    error,
                    usage: None,
                    generation,
                }
            } else {
                RuntimeEvent::ModelRequestCompleted {
                    request_id: snapshot.request_id.clone(),
                    finish_reason,
                    usage: None,
                    generation,
                }
            };
            let mut terminal = envelope(
                store.conversation_id(),
                "probe-provider-terminal",
                Some(snapshot.identity.attempt_id.clone()),
                kind,
            );
            terminal.turn_id = Some(snapshot.identity.turn.clone());
            let terminal = store.append_event(terminal).unwrap();
            assert_event_probe_oracle(&store, &terminal, (3, 0));
            if failed {
                let mut retry = terminal.clone();
                retry.event_id = EventId::new("probe-retry");
                retry.event = RuntimeEvent::ModelRetryScheduled {
                    failed_request_id: snapshot.request_id.clone(),
                    retry_number: 1,
                    retry_delay_ms: Some(2),
                };
                let retry = store.append_event(retry).unwrap();
                assert_event_probe_oracle(&store, &retry, (1, 0));
            }
            assert_eq!(
                store.load_request_snapshot(&snapshot.request_id).unwrap(),
                snapshot
            );
        }
    }
}

#[test]
fn t08_t14_t19_event_probe_pruning_preserves_attempt_and_adoption_sources() {
    for failed in [false, true] {
        let store = store();
        store.initialize(&[]).unwrap();
        let accepted = store.accept_inbound(draft("native adopted 水🙂")).unwrap();
        let adoption = store
            .adopt_pending_batch(accepted.sequence, None)
            .unwrap()
            .adoption
            .unwrap();
        assert_event_probe_oracle(&store, &adoption, (1, 0));
        let attempt = AttemptId::new("probe-attempt");
        let start = store
            .append_event(envelope(
                store.conversation_id(),
                "probe-attempt-start",
                Some(attempt.clone()),
                RuntimeEvent::AttemptStarted {
                    attempt_id: attempt.clone(),
                },
            ))
            .unwrap();
        assert_event_probe_oracle(&store, &start, (1, 0));
        for (index, kind) in [RuntimeEvent::TurnStarted, RuntimeEvent::TurnCompleted]
            .into_iter()
            .enumerate()
        {
            let mut event = envelope(
                store.conversation_id(),
                &format!("probe-turn-{index}"),
                Some(attempt.clone()),
                kind,
            );
            event.turn_id = Some(crate::runtime::identity::TurnId::new("1"));
            let turn = store.append_event(event).unwrap();
            assert_event_probe_oracle(&store, &turn, (1, 0));
        }
        let kind = if failed {
            let mut error = crate::model::error::ModelError::timeout(
                crate::model::deadline::ModelTimeoutPhase::ResponseStart,
            );
            error.message = "native failure水🙂".repeat(2000);
            RuntimeEvent::AttemptFailed {
                attempt_id: attempt.clone(),
                error: crate::events::types::AttemptFailure::Model { error },
            }
        } else {
            RuntimeEvent::AttemptCompleted {
                attempt_id: attempt.clone(),
                finish_reason: ModelFinishReason::Other {
                    reason: "native finish水🙂".repeat(2000),
                },
            }
        };
        let terminal = store
            .append_event(envelope(
                store.conversation_id(),
                "probe-attempt-terminal",
                Some(attempt),
                kind,
            ))
            .unwrap();
        assert_event_probe_oracle(&store, &terminal, (2, 0));
    }
}

#[test]
fn t08_t14_t19_event_probe_pruning_preserves_contribution_and_timeout_sources() {
    let store = store();
    store
        .initialize(&[user_message("probe-base", "native base")])
        .unwrap();
    let message_id = MessageId::new("probe-status");
    let status = agent_status_message(
        message_id.as_str(),
        Utc.with_ymd_and_hms(2026, 8, 27, 6, 0, 0).unwrap(),
        &[AgentStatusModuleId::Todo],
    );
    let mut snapshot = status_start_snapshot(
        store.load_head().unwrap().revision.next(),
        message_id,
        vec![ContributionEmission {
            key: "native-probe".into(),
            fingerprint: "native-fingerprint".into(),
        }],
    );
    freeze_test_input(&store, &mut snapshot, std::slice::from_ref(&status));
    let started = store
        .commit_model_turn_start(&[status], &snapshot, Utc::now())
        .unwrap();
    assert_eq!(started.contribution_emissions.len(), 1);
    assert_event_probe_oracle(&store, &started.started, (1, 0));
    assert_event_probe_oracle(&store, &started.contribution_emissions[0], (1, 0));
    let attempt = AttemptId::new("probe-timeout");
    let started = store
        .append_event(envelope(
            store.conversation_id(),
            "probe-timeout-start",
            Some(attempt.clone()),
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt.clone(),
            },
        ))
        .unwrap();
    assert_event_probe_oracle(&store, &started, (1, 0));
    let timeout = store
        .append_event(envelope(
            store.conversation_id(),
            "probe-timeout-terminal",
            Some(attempt.clone()),
            RuntimeEvent::AttemptTimedOut {
                attempt_id: attempt,
            },
        ))
        .unwrap();
    assert_event_probe_oracle(&store, &timeout, (1, 0));
}

fn clear_event_source(connection: &Connection, sequence: u64) {
    let id = format!("journal/{sequence}");
    for table in [
        "state_observation_fields",
        "content_segments",
        "content_frontiers",
    ] {
        connection
            .execute(&format!("DELETE FROM {table} WHERE source_id=?1"), [&id])
            .unwrap();
    }
    connection
        .execute(
            "UPDATE events SET observation_source=NULL WHERE sequence=?1",
            [sequence],
        )
        .unwrap();
}

/// Compare the production storage choice to the independently persisted segmented
/// layout, including every field's original bytes, previews, aliases and pages.
#[allow(clippy::too_many_lines)] // One exhaustive persisted-layout differential, including unchanged bytes and pages.
fn assert_inline_event_oracle(connection: &Connection, event: &RuntimeEventEnvelope) {
    let transaction = connection.unchecked_transaction().unwrap();
    clear_event_source(&transaction, event.sequence);
    observation_event::index_legacy(&transaction, event).unwrap();
    let id = format!("journal/{}", event.sequence);
    let expected_witness: Vec<u8> = transaction
        .query_row(
            "SELECT observation_witness FROM events WHERE sequence=?1",
            [event.sequence],
            |row| row.get(0),
        )
        .unwrap();
    let paths = transaction
        .prepare(
            "SELECT path_json FROM state_observation_fields WHERE source_id=?1 ORDER BY path_json",
        )
        .unwrap()
        .query_map([&id], |row| row.get::<_, String>(0))
        .unwrap()
        .map(|row| serde_json::from_str::<Vec<String>>(&row.unwrap()).unwrap())
        .collect::<Vec<_>>();
    let mut pages = Vec::new();
    let mut reads = Vec::new();
    for path in paths {
        let (start, end, children) = observation_request::field_coordinates(
            &transaction,
            "state_observation_fields",
            &id,
            &path,
        )
        .unwrap();
        for offset in [0, children / 2, children] {
            for limit in [1, 32] {
                let page = observation_request::page_from(
                    &transaction,
                    "state_observation_fields",
                    &id,
                    &path,
                    offset,
                    limit,
                )
                .unwrap();
                pages.push((
                    path.clone(),
                    offset,
                    limit,
                    serde_json::to_value(page).unwrap(),
                ));
            }
        }
        for offset in [0, (end - start) / 2, end - start] {
            for limit in [1, 67, 16_384] {
                let bytes = observation_request::read_from(
                    &transaction,
                    "oj",
                    "state_observation_fields",
                    &id,
                    &path,
                    offset,
                    limit,
                )
                .unwrap();
                reads.push((path.clone(), offset, limit, bytes));
            }
        }
    }
    let original = observation_request::read_from(
        &transaction,
        "oj",
        "state_observation_fields",
        &id,
        &[],
        0,
        65_536,
    )
    .unwrap();
    clear_event_source(&transaction, event.sequence);
    observation_event::index(&transaction, event).unwrap();
    let actual_witness: Vec<u8> = transaction
        .query_row(
            "SELECT observation_witness FROM events WHERE sequence=?1",
            [event.sequence],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        actual_witness, expected_witness,
        "semantic witness must not change"
    );
    let inline: Option<Vec<u8>> = transaction
        .query_row(
            "SELECT observation_source FROM events WHERE sequence=?1",
            [event.sequence],
            |row| row.get(0),
        )
        .unwrap();
    let native_length = serde_json::to_vec(event).unwrap().len();
    if original.readable_end <= 16_384 && native_length <= 16_384 {
        assert!(inline.is_some());
    } else if native_length > 16_384 {
        assert!(inline.is_none());
    }
    if let Some(inline) = inline {
        if inline[0] == 0 {
            assert_eq!(inline.len(), 33, "only a tag and native digest are stored");
        } else {
            assert_eq!(inline[0], 1);
            assert!(inline.len() <= 16_385);
        }
        assert_eq!(
            observation_event::read(&transaction, event.sequence, event.sequence, &[], 0, 65_536)
                .unwrap()
                .bytes,
            original.bytes,
            "retain exact structural bytes from the shared native body"
        );
        for table in [
            "state_observation_fields",
            "content_segments",
            "content_frontiers",
        ] {
            let rows: u64 = transaction
                .query_row(
                    &format!("SELECT count(*) FROM {table} WHERE source_id=?1"),
                    [&id],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(rows, 0, "small events create no duplicate {table} rows");
        }
    }
    for (path, offset, limit, expected) in pages {
        let actual = observation_event::page(
            &transaction,
            event.sequence,
            event.sequence,
            &path,
            offset,
            limit,
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(actual).unwrap(),
            expected,
            "field page {path:?} offset {offset}"
        );
    }
    for (path, offset, limit, expected) in reads {
        let actual = observation_event::read(
            &transaction,
            event.sequence,
            event.sequence,
            &path,
            offset,
            limit,
        )
        .unwrap();
        assert_eq!(actual, expected, "field bytes {path:?} offset {offset}");
    }
    assert!(
        observation_event::read(&transaction, event.sequence, event.sequence - 1, &[], 0, 32)
            .is_err()
    );
    assert!(
        observation_event::page(&transaction, event.sequence, event.sequence - 1, &[], 0, 32)
            .is_err()
    );
    transaction.rollback().unwrap();
}

#[test]
fn inline_journal_sources_preserve_boundary_bytes_and_reopened_historical_reads() {
    for size in [16_383, 16_384, 16_385] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("inline-journal.sqlite");
        let conversation = ConversationId::generate();
        let store = SqliteConversationStore::open(conversation.clone(), &path).unwrap();
        store.initialize(&[]).unwrap();
        let attempt = AttemptId::new("inline-boundary-attempt");
        let mut event = envelope(
            &conversation,
            "inline-boundary",
            Some(attempt.clone()),
            RuntimeEvent::AttemptCompleted {
                attempt_id: attempt,
                finish_reason: ModelFinishReason::Other {
                    reason: String::new(),
                },
            },
        );
        event.sequence = 1;
        let mut bytes = Vec::new();
        crate::runtime_client::structural_json::to_writer(&mut bytes, &event).unwrap();
        let RuntimeEvent::AttemptCompleted {
            finish_reason: ModelFinishReason::Other { reason },
            ..
        } = &mut event.event
        else {
            unreachable!()
        };
        *reason = "x".repeat(size - bytes.len());
        let committed = store.append_event(event).unwrap();
        let read = store
            .event_value_read(committed.sequence, committed.sequence, &[], 0, 65_536)
            .unwrap();
        assert_eq!(read.readable_end, size as u64);
        {
            let connection = store.lock().unwrap();
            assert_inline_event_oracle(&connection, &committed);
        }
        let late = store
            .append_event(envelope(
                &conversation,
                "inline-late",
                Some(AttemptId::new("late")),
                RuntimeEvent::AttemptTimedOut {
                    attempt_id: AttemptId::new("late"),
                },
            ))
            .unwrap();
        assert!(late.sequence > committed.sequence);
        assert_eq!(
            store
                .event_value_read(committed.sequence, committed.sequence, &[], 0, 65_536)
                .unwrap(),
            read
        );
        drop(store);
        let reopened = SqliteConversationStore::open_existing(conversation, &path).unwrap();
        assert_eq!(
            reopened
                .event_value_read(committed.sequence, committed.sequence, &[], 0, 65_536)
                .unwrap(),
            read
        );
        assert_eq!(
            reopened.read_events(None, 32).unwrap().events[0].event_id,
            committed.event_id
        );
    }
}

#[test]
fn inline_journal_source_corruption_fails_closed_and_original_journal_survives() {
    let store = store();
    store.initialize(&[]).unwrap();
    let attempt = AttemptId::new("inline-corruption");
    let committed = store
        .append_event(envelope(
            store.conversation_id(),
            "inline-corruption-event",
            Some(attempt.clone()),
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt,
            },
        ))
        .unwrap();
    let original = store.read_events(None, 32).unwrap().events;
    for bytes in [
        br#"{"event":bare}"#.as_slice(),
        br#"{"event":"\q"}"#,
        br"{} []",
        br#"{"same":1,"same":2}"#,
    ] {
        let connection = store.lock().unwrap();
        let transaction = connection.unchecked_transaction().unwrap();
        transaction
            .execute_batch("PRAGMA ignore_check_constraints=ON")
            .unwrap();
        transaction
            .execute(
                "UPDATE events SET observation_source=?2 WHERE sequence=?1",
                params![committed.sequence, bytes],
            )
            .unwrap();
        assert!(
            observation_event::read(
                &transaction,
                committed.sequence,
                committed.sequence,
                &[],
                0,
                32
            )
            .is_err()
        );
        assert!(
            observation_event::page(
                &transaction,
                committed.sequence,
                committed.sequence,
                &[],
                0,
                32
            )
            .is_err()
        );
        transaction.rollback().unwrap();
    }
    {
        let connection = store.lock().unwrap();
        let transaction = connection.unchecked_transaction().unwrap();
        transaction
            .execute_batch("PRAGMA ignore_check_constraints=ON")
            .unwrap();
        let oversized = vec![b'x'; 16_385];
        for bytes in [b"{}".as_slice(), oversized.as_slice()] {
            transaction
                .execute(
                    "UPDATE events SET observation_source=?2 WHERE sequence=?1",
                    params![committed.sequence, bytes],
                )
                .unwrap();
            assert!(
                observation_event::read(
                    &transaction,
                    committed.sequence,
                    committed.sequence,
                    &[],
                    0,
                    32
                )
                .is_err()
            );
            assert!(
                observation_event::page(
                    &transaction,
                    committed.sequence,
                    committed.sequence,
                    &[],
                    0,
                    32
                )
                .is_err()
            );
        }
        transaction.rollback().unwrap();
        connection
            .execute_batch("PRAGMA ignore_check_constraints=OFF")
            .unwrap();
    }
    assert_eq!(store.read_events(None, 32).unwrap().events, original);
}

#[test]
fn inline_journal_source_failure_preserves_the_authoritative_event() {
    let store = store();
    store.initialize(&[]).unwrap();
    store.lock().unwrap().execute_batch("CREATE TRIGGER inline_source_fault BEFORE UPDATE OF observation_source ON events BEGIN SELECT RAISE(FAIL,'derived inline source'); END;").unwrap();
    let attempt = AttemptId::new("inline-source-failure");
    let committed = store
        .append_event(envelope(
            store.conversation_id(),
            "inline-source-failure",
            Some(attempt.clone()),
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt,
            },
        ))
        .unwrap();
    assert_eq!(
        store.read_events(None, 32).unwrap().events[0].event_id,
        committed.event_id
    );
    assert!(
        store
            .observation_event_witness(committed.sequence, committed.sequence)
            .unwrap()
            .unwrap()
            .source_unavailable
    );
    assert!(
        store
            .event_value_read(committed.sequence, committed.sequence, &[], 0, 32)
            .is_err()
    );
    let unavailable: bool = store
        .lock()
        .unwrap()
        .query_row(
            "SELECT observation_unavailable FROM events WHERE sequence=?1",
            [committed.sequence],
            |row| row.get(0),
        )
        .unwrap();
    assert!(unavailable);
}

#[test]
fn inline_journal_schema_rejects_previous_layout_without_migration() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("previous-journal.sqlite");
    let conversation = ConversationId::generate();
    {
        let store = SqliteConversationStore::open(conversation.clone(), &path).unwrap();
        store
            .lock()
            .unwrap()
            .execute("UPDATE rustx_store SET schema_version=75", [])
            .unwrap();
    }
    assert!(matches!(
        SqliteConversationStore::open_existing(conversation, &path),
        Err(ConversationStoreError::SchemaVersionMismatch {
            stored: 75,
            expected: crate::durable::sqlite::SQLITE_SCHEMA_VERSION
        })
    ));
}

#[test]
fn inline_journal_sources_preserve_opaque_numbers_aliases_and_paginated_previews() {
    let store = store();
    store.initialize(&[]).unwrap();
    let attempt = AttemptId::new("inline-opaque-attempt");
    let (proposal, call) = derived_enrichment_proposal();
    store
        .append_canonical_with_event(
            &proposal,
            envelope(
                store.conversation_id(),
                "inline-opaque-proposal",
                Some(attempt.clone()),
                RuntimeEvent::AssistantMessageCommitted {
                    message_id: proposal.id().clone(),
                },
            ),
        )
        .unwrap();
    store
        .append_event(envelope(
            store.conversation_id(),
            "inline-opaque-start",
            Some(attempt.clone()),
            RuntimeEvent::ToolExecutionStarted {
                tool_call_id: call.id.clone(),
                tool_id: call.tool_id.clone(),
            },
        ))
        .unwrap();
    let mut result = original_success_tool_result();
    result.duration_ms = u64::MAX;
    let mut value = serde_json::json!({
        "@reserved": {"$key":"literal", "integer":u64::MAX},
        "content":[{"not":"a ContentBinding"}],
        "collections":[{"not":"a frontier"}],
        "many": (0..48).map(|n| serde_json::json!({"ordinal":n,"text":"water 水🙂"})).collect::<Vec<_>>()
    });
    value.as_object_mut().unwrap().insert(
        "long-key-水".repeat(40),
        serde_json::json!({"value":"original bytes"}),
    );
    result.content = vec![crate::tools::types::ToolResultContent::Json { value }];
    let committed = store
        .append_event(envelope(
            store.conversation_id(),
            "inline-opaque-result",
            Some(attempt),
            RuntimeEvent::ToolExecutionCompleted {
                tool_call_id: call.id,
                tool_id: call.tool_id,
                result,
            },
        ))
        .unwrap();
    let connection = store.lock().unwrap();
    let inline: bool = connection
        .query_row(
            "SELECT observation_source IS NOT NULL FROM events WHERE sequence=?1",
            [committed.sequence],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        inline,
        "exercise inline, not just large-event indexed parity"
    );
    assert_inline_event_oracle(&connection, &committed);
}

#[test]
#[allow(clippy::too_many_lines)] // One persisted source under independent native and coherent overlay attacks.
fn deferred_native_source_fences_changed_bytes_and_coherently_malformed_events() {
    use sha2::Digest as _;
    let store = store();
    store.initialize(&[]).unwrap();
    let attempt = AttemptId::new("overlay-body-owner");
    let committed = store
        .append_event(envelope(
            store.conversation_id(),
            "overlay-body-event",
            Some(attempt.clone()),
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt,
            },
        ))
        .unwrap();
    let connection = store.lock().unwrap();
    let (native, overlay): (String, Vec<u8>) = connection
        .query_row(
            "SELECT event_json,observation_source FROM events WHERE sequence=?1",
            [committed.sequence],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert!(overlay.len() < 128);
    assert!(
        overlay.len() * 2 < native.len(),
        "retain coordinates and digests, not another body"
    );
    let original = observation_event::read(
        &connection,
        committed.sequence,
        committed.sequence,
        &[],
        0,
        65_536,
    )
    .unwrap();
    let rejected = |connection: &Connection| {
        assert!(
            observation_event::read(
                connection,
                committed.sequence,
                committed.sequence,
                &[],
                0,
                32
            )
            .is_err()
        );
        assert!(
            observation_event::page(
                connection,
                committed.sequence,
                committed.sequence,
                &[],
                0,
                32
            )
            .is_err()
        );
    };
    // Both bodies are syntactically valid, with the same typed event. Exact
    // immutable-byte identity still matters; there is no stale decoded cache.
    for modified in [
        format!("{native} "),
        format!("{native}{}", " ".repeat(16_385)),
    ] {
        let transaction = connection.unchecked_transaction().unwrap();
        transaction
            .execute(
                "UPDATE events SET event_json=?2 WHERE sequence=?1",
                params![committed.sequence, modified],
            )
            .unwrap();
        rejected(&transaction);
        transaction.rollback().unwrap();
    }
    // A valid JSON body with a recomputed digest still cannot invent a native
    // event variant. The expression indexes accept JSON, the Source rejects it.
    let mut malformed: serde_json::Value = serde_json::from_str(&native).unwrap();
    malformed["event"]["type"] = "unknown_frozen_event".into();
    let malformed = serde_json::to_vec(&malformed).unwrap();
    let transaction = connection.unchecked_transaction().unwrap();
    let mut record = vec![0];
    record.extend_from_slice(&sha2::Sha256::digest(&malformed));
    transaction
        .execute(
            "UPDATE events SET event_json=?2,observation_source=?3 WHERE sequence=?1",
            params![committed.sequence, String::from_utf8(malformed).unwrap(), record],
        )
        .unwrap();
    rejected(&transaction);
    transaction.rollback().unwrap();
    assert_eq!(
        observation_event::read(
            &connection,
            committed.sequence,
            committed.sequence,
            &[],
            0,
            65_536
        )
        .unwrap(),
        original
    );
}

#[test]
fn quote_overlay_trigger_changed_native_bytes_fail_only_the_derived_source() {
    let store = store();
    store.initialize(&[]).unwrap();
    store.lock().unwrap().execute_batch(
        "CREATE TRIGGER change_original_event AFTER INSERT ON events BEGIN UPDATE events SET event_json=event_json||' ' WHERE sequence=NEW.sequence; END;"
    ).unwrap();
    let attempt = AttemptId::new("overlay-trigger-owner");
    let committed = store
        .append_event(envelope(
            store.conversation_id(),
            "overlay-trigger-event",
            Some(attempt.clone()),
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt,
            },
        ))
        .unwrap();
    assert_eq!(
        store.read_events(None, 32).unwrap().events[0].event_id,
        committed.event_id
    );
    assert!(
        store
            .observation_event_witness(committed.sequence, committed.sequence)
            .unwrap()
            .unwrap()
            .source_unavailable
    );
    assert!(
        store
            .event_value_read(committed.sequence, committed.sequence, &[], 0, 32)
            .is_err()
    );
    let overlay: Option<Vec<u8>> = store
        .lock()
        .unwrap()
        .query_row(
            "SELECT observation_source FROM events WHERE sequence=?1",
            [committed.sequence],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        overlay.is_none(),
        "failed authenticated capture leaves no partial overlay"
    );
}

#[test]
#[allow(clippy::too_many_lines)] // Both public source endpoints share the same controlled external-write edge.
fn quote_overlay_reads_share_one_snapshot_across_external_body_changes() {
    use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
    use std::sync::atomic::{AtomicBool, Ordering};
    for read_page in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("overlay-snapshot.sqlite");
        let store = SqliteConversationStore::open(ConversationId::generate(), &path).unwrap();
        store.initialize(&[]).unwrap();
        let attempt = AttemptId::new("overlay-snapshot-owner");
        let sequence = store
            .append_event(envelope(
                store.conversation_id(),
                "overlay-snapshot-event",
                Some(attempt.clone()),
                RuntimeEvent::AttemptStarted {
                    attempt_id: attempt,
                },
            ))
            .unwrap()
            .sequence;
        let original = store
            .event_value_read(sequence, sequence, &[], 0, 65_536)
            .unwrap();
        let original_page = store
            .event_value_page(sequence, sequence, &[], 0, 32)
            .unwrap();
        let writer = Connection::open(&path).unwrap();
        let changed = Arc::new(AtomicBool::new(false));
        let signal = Arc::clone(&changed);
        // The existence/layout statement has already established the read
        // snapshot when the next statement authorizes the bounded native body.
        // Commit an actual write on a second connection at precisely that edge.
        store
            .lock()
            .unwrap()
            .authorizer(Some(move |context: AuthContext<'_>| {
                if matches!(
                    context.action,
                    AuthAction::Read {
                        table_name: "events",
                        column_name: "event_json"
                    }
                ) && !signal.swap(true, Ordering::SeqCst)
                {
                    assert_eq!(
                        writer
                            .execute(
                                "UPDATE events SET event_json=event_json||' ' WHERE sequence=?1",
                                [sequence],
                            )
                            .unwrap(),
                        1
                    );
                }
                Authorization::Allow
            }))
            .unwrap();
        if read_page {
            assert_eq!(
                serde_json::to_vec(
                    &store
                        .event_value_page(sequence, sequence, &[], 0, 32)
                        .unwrap()
                )
                .unwrap(),
                serde_json::to_vec(&original_page).unwrap()
            );
        } else {
            assert_eq!(
                store
                    .event_value_read(sequence, sequence, &[], 0, 65_536)
                    .unwrap(),
                original
            );
        }
        assert!(
            changed.load(Ordering::SeqCst),
            "external write must commit between the two source queries"
        );
        store
            .lock()
            .unwrap()
            .authorizer(None::<fn(AuthContext<'_>) -> Authorization>)
            .unwrap();
        // A subsequent public read sees the committed bytes and rejects the
        // stale digest. The old result was a snapshot, never an unchecked cache.
        assert!(
            store
                .event_value_read(sequence, sequence, &[], 0, 32)
                .is_err()
        );
        assert!(
            store
                .event_value_page(sequence, sequence, &[], 0, 32)
                .is_err()
        );
    }
}
