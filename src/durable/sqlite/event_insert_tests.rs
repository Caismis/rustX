use super::observation_response_tests::{response_complete, response_message, response_start};

pub(super) fn count_event_writes(store: &SqliteConversationStore) -> Arc<Mutex<(usize, usize)>> {
    let counts = Arc::new(Mutex::new((0, 0)));
    let observed = Arc::clone(&counts);
    // This fresh fixture has no retained read caches. Observe physical row
    // operations directly, without a SQL trigger that changes append ordering.
    store
        .lock()
        .unwrap()
        .preupdate_hook(Some(
            move |_: rusqlite::hooks::Action,
                  db: &str,
                  table: &str,
                  change: &rusqlite::hooks::PreUpdateCase| {
                if db == "main" && table == "events" {
                    let mut counts = observed.lock().unwrap();
                    match change {
                        rusqlite::hooks::PreUpdateCase::Insert(_) => counts.0 += 1,
                        rusqlite::hooks::PreUpdateCase::Update { .. } => counts.1 += 1,
                        _ => {}
                    }
                }
            },
        ))
        .unwrap();
    counts
}
fn event_rows(store: &SqliteConversationStore) -> Vec<Vec<rusqlite::types::Value>> {
    let connection = store.lock().unwrap();
    let mut statement = connection
        .prepare("SELECT * FROM events ORDER BY sequence")
        .unwrap();
    let width = statement.column_count();
    statement
        .query_map([], |row| (0..width).map(|column| row.get(column)).collect())
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

#[test]
fn prepared_event_insert_materializes_complete_rows_without_transient_updates() {
    let store = store();
    store.initialize(&[]).unwrap();
    let writes = count_event_writes(&store);
    event_insert::take_work();
    let attempt = response_start(&store, "prepared-row");
    let old = event_rows(&store);
    response_message(&store, &attempt, "prepared-row-message");
    response_complete(&store, &attempt);
    assert_eq!(*writes.lock().unwrap(), (3, 0));
    assert_eq!(event_insert::take_work(), (3, 0));
    assert_eq!(event_rows(&store)[0], old[0]);
    let connection = store.lock().unwrap();
    let holes: u64 = connection.query_row("SELECT count(*) FROM events WHERE observation_aggregate_holes>0 OR observation_owner_holes>0 OR observation_unavailable=1 OR observation_readiness_missing=1", [], |row|row.get(0)).unwrap();
    assert_eq!(holes, 0);
    drop(connection);
    assert_eq!(
        store.observation_statistics(3).unwrap().completed_responses,
        1
    );
    assert_eq!(store.observation_trace_total(3).unwrap(), 2);
}

#[test]
fn prepared_event_rows_match_native_first_isolation_at_historical_cuts() {
    let prepared = store();
    let isolated = store();
    for store in [&prepared, &isolated] {
        store.initialize(&[]).unwrap();
    }
    // A real user SQL program deliberately selects native-first ordering.
    isolated.lock().unwrap().execute_batch("CREATE TEMP TRIGGER native_first_probe AFTER INSERT ON events BEGIN SELECT NEW.sequence; END;").unwrap();
    for store in [&prepared, &isolated] {
        let attempt = response_start(store, "differential-row");
        let mut turn = envelope(
            store.conversation_id(),
            "differential-turn",
            Some(attempt.clone()),
            RuntimeEvent::TurnStarted,
        );
        turn.turn_id = Some(TurnId::new("1"));
        store.append_event(turn).unwrap();
        response_message(store, &attempt, "differential-message");
        store
            .append_event(envelope(
                store.conversation_id(),
                "differential-terminal",
                Some(attempt.clone()),
                RuntimeEvent::AttemptCancelled {
                    attempt_id: attempt,
                    reason: crate::runtime::types::CancellationReason::UserRequested,
                },
            ))
            .unwrap();
    }
    assert_eq!(event_rows(&prepared), event_rows(&isolated));
    for through in 1..=4 {
        assert_eq!(
            prepared.observation_statistics(through).unwrap(),
            isolated.observation_statistics(through).unwrap()
        );
        for sequence in 1..=through {
            assert_eq!(
                serde_json::to_vec(
                    &prepared
                        .observation_event_witness(sequence, through)
                        .unwrap()
                )
                .unwrap(),
                serde_json::to_vec(
                    &isolated
                        .observation_event_witness(sequence, through)
                        .unwrap()
                )
                .unwrap()
            );
        }
    }
    assert_eq!(
        prepared.load_canonical().unwrap(),
        isolated.load_canonical().unwrap()
    );
}

#[test]
fn prepared_insert_constraint_failure_isolates_only_rejected_aggregate_domain() {
    let store = store();
    store.initialize(&[]).unwrap();
    let attempt = response_start(&store, "constraint-aggregate");
    let old = event_rows(&store);
    store.lock().unwrap().execute_batch("ALTER TABLE events ADD COLUMN reject_aggregate INTEGER CHECK(sequence!=2 OR observation_statistics IS NULL);").unwrap();
    event_insert::take_work();
    response_message(&store, &attempt, "constraint-aggregate-message");
    assert_eq!(event_insert::take_work(), (0, 1));
    let connection = store.lock().unwrap();
    let state: (bool,bool,bool,bool,bool,bool) = connection.query_row("SELECT observation_statistics IS NULL,observation_members IS NULL,observation_trace_total IS NULL,observation_execution IS NOT NULL,observation_witness IS NOT NULL,observation_trace_fact IS NOT NULL FROM events WHERE sequence=2", [], |row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?))).unwrap();
    assert_eq!(state, (true, true, true, true, true, true));
    let ready: u64 = connection
        .query_row(
            "SELECT count(*) FROM event_observation_readiness WHERE sequence=2",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(ready, 1);
    drop(connection);
    let rows = event_rows(&store);
    assert_eq!(&rows[0][..old[0].len()], &old[0]);
    assert_eq!(store.load_canonical().unwrap().len(), 1);
    assert_eq!(store.read_events(None, 8).unwrap().events.len(), 2);
    assert!(store.observation_statistics(2).is_err());
    assert!(
        store
            .observation_event_witness(2, 2)
            .unwrap()
            .unwrap()
            .source_unavailable
    );
}

#[test]
fn prepared_request_insert_captures_trace_core_once_and_rolls_back_failed_model_domain() {
    for rejected_domain in [None, Some("event"), Some("model")] {
        let reject_execution = rejected_domain.is_some();
        let store = store();
        store.initialize(&[]).unwrap();
        match rejected_domain {
            Some("event") => store.lock().unwrap().execute_batch("ALTER TABLE events ADD COLUMN reject_execution INTEGER CHECK(observation_execution IS NULL);").unwrap(),
            Some("model") => store.lock().unwrap().execute_batch("ALTER TABLE observation_models ADD COLUMN reject_model INTEGER CHECK(first_sequence!=1);").unwrap(),
            _ => {},
        }
        let writes = count_event_writes(&store);
        event_insert::take_work();
        let (snapshot, sequence) = inspection_request(&store, 0, "finite schema");
        assert_eq!(sequence, 1);
        let connection = store.lock().unwrap();
        let (models, execution, trace, ready):(u64,bool,bool,u64)=connection.query_row("SELECT (SELECT count(*) FROM observation_models),observation_execution IS NOT NULL,observation_trace_fact IS NOT NULL,(SELECT count(*) FROM event_observation_readiness) FROM events WHERE sequence=1",[],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))).unwrap();
        assert_eq!(
            (models, execution, trace, ready),
            (u64::from(!reject_execution), !reject_execution, true, 1)
        );
        let foreign_keys: u64 = connection
            .query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(foreign_keys, 0);
        if reject_execution {
            assert_eq!(event_insert::take_work(), (0, 1));
            let bytes: u64 = connection
                .query_row(
                    "SELECT count(*) FROM content_frontiers WHERE source_kind='attempt_model'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(
                bytes, 0,
                "no speculative model source survives rejected execution domain"
            );
        } else {
            assert_eq!(event_insert::take_work(), (1, 0));
            assert_eq!(*writes.lock().unwrap(), (1, 0));
        }
        drop(connection);
        assert_eq!(
            store.load_request_snapshot(&snapshot.request_id).unwrap(),
            snapshot
        );
    }
}

#[test]
fn prepared_native_insert_failure_rolls_back_lifecycle_readiness_and_reuses_sequence() {
    let store = store();
    store.initialize(&[]).unwrap();
    store.lock().unwrap().execute_batch("ALTER TABLE events ADD COLUMN reject_native INTEGER CHECK(event_id!='rejected-native');").unwrap();
    let attempt = AttemptId::new("rejected-native-owner");
    let event = envelope(
        store.conversation_id(),
        "rejected-native",
        Some(attempt.clone()),
        RuntimeEvent::AttemptStarted {
            attempt_id: attempt.clone(),
        },
    );
    assert!(matches!(
        store.append_event(event),
        Err(ConversationStoreError::Storage(_))
    ));
    let connection = store.lock().unwrap();
    let state:(u64,u64,u64,u64)=connection.query_row("SELECT (SELECT count(*) FROM events),(SELECT count(*) FROM lifecycle_state),(SELECT count(*) FROM event_observation_readiness),next_event_sequence FROM rustx_store WHERE id=1",[],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))).unwrap();
    assert_eq!(state, (0, 0, 0, 0));
    drop(connection);
    let event = envelope(
        store.conversation_id(),
        "accepted-native",
        Some(attempt.clone()),
        RuntimeEvent::AttemptStarted {
            attempt_id: attempt,
        },
    );
    assert_eq!(store.append_event(event).unwrap().sequence, 1);
}

#[test]
fn prepared_insert_rechecks_temp_trigger_changes_and_preserves_native_first_visibility() {
    let store = store();
    store.initialize(&[]).unwrap();
    response_start(&store, "before-trigger");
    store.lock().unwrap().execute_batch("CREATE TEMP TABLE observed_native(missing INTEGER); CREATE TEMP TRIGGER native_observer AFTER INSERT ON main.events BEGIN INSERT INTO observed_native VALUES(NEW.observation_witness IS NULL); END;").unwrap();
    event_insert::take_work();
    response_start(&store, "during-trigger");
    assert_eq!(event_insert::take_work(), (0, 0));
    let connection = store.lock().unwrap();
    let missing: bool = connection
        .query_row("SELECT missing FROM observed_native", [], |row| row.get(0))
        .unwrap();
    assert!(missing);
    connection
        .execute_batch("DROP TRIGGER native_observer;")
        .unwrap();
    drop(connection);
    response_start(&store, "after-trigger");
    assert_eq!(event_insert::take_work(), (1, 0));
}

#[test]
fn pending_compaction_capture_keeps_actual_activation_and_successor_validation() {
    let store = store();
    store
        .initialize(&[user_message("prepared-projection-original", "original")])
        .unwrap();
    let receipt = store
        .commit_projection(observation_projection_input(&store, "prepared-projection"))
        .unwrap();
    assert_projection_receipt_head(&receipt);
    let expected = store
        .observation_projection_head(receipt.completed.sequence)
        .unwrap()
        .unwrap();
    let mut connection = store.lock().unwrap();
    let transaction = connection.transaction().unwrap();
    // Only the just-prepared native completion is absent. All immutable
    // activation/header authority remains in the same transaction.
    transaction
        .execute(
            "DELETE FROM events WHERE sequence=?1",
            [receipt.completed.sequence],
        )
        .unwrap();
    assert!(observation_projection::read(&transaction, receipt.completed.sequence).is_err());
    assert_eq!(
        observation_projection::pending_completion(&transaction, &receipt.completed).unwrap(),
        expected
    );
    transaction.execute("UPDATE projection_activations SET visible_journal=visible_journal+1 WHERE activation=?1", [receipt.checkpoint.activation.activation.0]).unwrap();
    assert!(observation_projection::pending_completion(&transaction, &receipt.completed).is_err());
    transaction.rollback().unwrap();
    drop(connection);
    assert_eq!(
        store
            .observation_projection_head(receipt.completed.sequence)
            .unwrap()
            .unwrap(),
        expected
    );
}

fn pending_interaction_pair(
    store: &SqliteConversationStore,
    attempt: &AttemptId,
    name: &str,
) -> [RuntimeEventEnvelope; 2] {
    use crate::events::interaction::{
        AnswerSpecification, InteractionRequester, InteractionSettlement, InteractionSubject,
        QuestionSpecification, QuestionnaireSpecification,
    };
    let interaction = crate::runtime::identity::InteractionId::new(name);
    let requested = crate::runtime::interaction::interaction_requested_event_id(&interaction);
    let settled = crate::runtime::interaction::interaction_settled_event_id(&interaction);
    let mut requested = envelope(
        store.conversation_id(),
        requested.as_str(),
        Some(attempt.clone()),
        RuntimeEvent::InteractionRequested {
            interaction_id: interaction.clone(),
            subject: InteractionSubject::Questionnaire {
                invocation_id: crate::tools::types::ToolInvocationId::Agent {
                    call_id: crate::runtime::identity::ToolCallId::new("prepared-question-call"),
                },
                requester: InteractionRequester {
                    tool_id: crate::runtime::identity::ToolId::new("prepared-question-tool"),
                    tool_name: "ask_user".into(),
                    origin: crate::tools::types::ToolOrigin::Builtin,
                },
                questionnaire: QuestionnaireSpecification {
                    questions: vec![QuestionSpecification {
                        question: "Proceed with the operation?".into(),
                        header: "Confirm".into(),
                        answer: AnswerSpecification::Boolean,
                    }],
                },
            },
        },
    );
    let mut settled = envelope(
        store.conversation_id(),
        settled.as_str(),
        Some(attempt.clone()),
        RuntimeEvent::InteractionSettled {
            interaction_id: interaction,
            settlement: InteractionSettlement::QuestionnaireDeclined,
        },
    );
    requested.turn_id = Some(TurnId::new("1"));
    settled.turn_id = requested.turn_id.clone();
    [requested, settled]
}

#[test]
fn prepared_terminal_and_interaction_headers_match_stored_path_without_event_updates() {
    let prepared = store();
    let isolated = store();
    for store in [&prepared, &isolated] {
        store.initialize(&[]).unwrap();
    }
    isolated.lock().unwrap().execute_batch("CREATE TEMP TRIGGER native_header_order AFTER INSERT ON events BEGIN SELECT NEW.sequence; END;").unwrap();
    let writes = count_event_writes(&prepared);
    for store in [&prepared, &isolated] {
        let mut old_pages = Vec::new();
        for failed in [false, true] {
            let name = if failed {
                "prepared-failed"
            } else {
                "prepared-cancelled"
            };
            let attempt = response_start(store, name);
            let mut turn = envelope(
                store.conversation_id(),
                &format!("{name}-turn"),
                Some(attempt.clone()),
                RuntimeEvent::TurnStarted,
            );
            turn.turn_id = Some(TurnId::new("1"));
            store.append_event(turn).unwrap();
            for event in pending_interaction_pair(store, &attempt, name) {
                store.append_interaction_audit(event).unwrap();
                let cut = store.observation_read_cut().unwrap();
                let page = store.transcript_header_page(&cut, None, 32).unwrap();
                old_pages.push((cut, serde_json::to_vec(&page).unwrap()));
            }
            let outcome = if failed {
                RuntimeEvent::AttemptFailed {
                    attempt_id: attempt.clone(),
                    error: crate::events::types::AttemptFailure::Runtime {
                        error: crate::runtime::types::RuntimeError::Internal {
                            message: "original failure 水".repeat(300),
                        },
                    },
                }
            } else {
                RuntimeEvent::AttemptCancelled {
                    attempt_id: attempt.clone(),
                    reason: crate::runtime::types::CancellationReason::UserRequested,
                }
            };
            store
                .append_event(envelope(
                    store.conversation_id(),
                    &format!("{name}-end"),
                    Some(attempt),
                    outcome,
                ))
                .unwrap();
        }
        for (cut, bytes) in old_pages {
            assert_eq!(
                serde_json::to_vec(&store.transcript_header_page(&cut, None, 32).unwrap()).unwrap(),
                bytes
            );
        }
        let unavailable: u64 = store
            .lock()
            .unwrap()
            .query_row(
                "SELECT count(*) FROM events WHERE observation_unavailable!=0",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(unavailable, 0);
    }
    assert_eq!(*writes.lock().unwrap(), (10, 0));
    assert_eq!(event_rows(&prepared), event_rows(&isolated));
    let prepared_cut = prepared.observation_read_cut().unwrap();
    let isolated_cut = isolated.observation_read_cut().unwrap();
    let page = prepared
        .transcript_header_page(&prepared_cut, None, 32)
        .unwrap();
    assert_eq!(
        serde_json::to_vec(&page).unwrap(),
        serde_json::to_vec(
            &isolated
                .transcript_header_page(&isolated_cut, None, 32)
                .unwrap()
        )
        .unwrap()
    );
    assert!(page.headers.iter().any(|header| matches!(
        header.display.as_ref().map(|display| &display.item),
        Some(crate::runtime_client::snapshot::RuntimeClientTranscriptItem::AttemptTerminal { turn }) if turn.failure.is_some()
    )));
}

#[test]
#[allow(clippy::too_many_lines)] // Real independent header failure, native history, old cut and later healthy event.
fn prepared_transcript_header_constraint_failure_preserves_native_history_and_old_cuts() {
    for interaction in [false, true] {
        let store = store();
        store.initialize(&[]).unwrap();
        let first = response_start(&store, "before-header-fault");
        store
            .append_event(envelope(
                store.conversation_id(),
                "before-header-fault-end",
                Some(first.clone()),
                RuntimeEvent::AttemptCancelled {
                    attempt_id: first,
                    reason: crate::runtime::types::CancellationReason::UserRequested,
                },
            ))
            .unwrap();
        let old_cut = store.observation_read_cut().unwrap();
        let old_page =
            serde_json::to_vec(&store.transcript_header_page(&old_cut, None, 32).unwrap()).unwrap();
        let attempt = response_start(&store, "header-fault");
        let (bad, followup) = if interaction {
            let mut turn = envelope(
                store.conversation_id(),
                "header-fault-turn",
                Some(attempt.clone()),
                RuntimeEvent::TurnStarted,
            );
            turn.turn_id = Some(TurnId::new("1"));
            store.append_event(turn).unwrap();
            let [requested, settled] =
                pending_interaction_pair(&store, &attempt, "header-fault-interaction");
            (requested, Some(settled))
        } else {
            (
                envelope(
                    store.conversation_id(),
                    "header-fault-end",
                    Some(attempt.clone()),
                    RuntimeEvent::AttemptFailed {
                        attempt_id: attempt,
                        error: crate::events::types::AttemptFailure::Runtime {
                            error: crate::runtime::types::RuntimeError::Internal {
                                message: "exact original failed terminal".into(),
                            },
                        },
                    },
                ),
                None,
            )
        };
        let next = store.observation_read_cut().unwrap().read.journal + 1;
        store.lock().unwrap().execute_batch(&format!("ALTER TABLE observation_headers ADD COLUMN reject_current_header INTEGER CHECK(journal_position!={next});")).unwrap();
        let writes = count_event_writes(&store);
        event_insert::take_work();
        let (persisted, cursor) = if interaction {
            store.append_interaction_audit(bad.clone()).unwrap()
        } else {
            let persisted = store.append_event(bad.clone()).unwrap();
            let cursor = store
                .event_transcript_cursor(&bad.event_id)
                .unwrap()
                .unwrap();
            (persisted, cursor)
        };
        assert_eq!(persisted.sequence, next);
        assert_eq!(*writes.lock().unwrap(), (1, 0));
        assert_eq!(event_insert::take_work(), (1, 0));
        assert_eq!(
            store.read_events(Some(next - 1), 1).unwrap().events[0].event,
            bad.event
        );
        assert_eq!(
            store.event_transcript_cursor_at_sequence(next).unwrap(),
            Some(cursor)
        );
        assert_eq!(
            store
                .load_transcript_page(Some(TranscriptCursor::new(cursor.get() + 1)), 1)
                .unwrap()
                .entries[0]
                .cursor,
            cursor
        );
        let witness = store
            .observation_event_witness(next, next)
            .unwrap()
            .unwrap();
        assert!(witness.source_unavailable);
        assert!(store.observation_statistics(next).is_ok());
        assert_eq!(
            serde_json::to_vec(&store.transcript_header_page(&old_cut, None, 32).unwrap()).unwrap(),
            old_page
        );
        let cut = store.observation_read_cut().unwrap();
        assert!(store.transcript_header_page(&cut, None, 1).is_err());
        store
            .lock()
            .unwrap()
            .execute_batch("ALTER TABLE observation_headers DROP COLUMN reject_current_header;")
            .unwrap();
        let healthy = if let Some(event) = followup {
            store.append_interaction_audit(event).unwrap().0
        } else {
            let attempt = response_start(&store, "after-header-fault");
            store
                .append_event(envelope(
                    store.conversation_id(),
                    "after-header-fault-end",
                    Some(attempt.clone()),
                    RuntimeEvent::AttemptCancelled {
                        attempt_id: attempt,
                        reason: crate::runtime::types::CancellationReason::UserRequested,
                    },
                ))
                .unwrap()
        };
        assert!(
            !store
                .observation_event_witness(healthy.sequence, healthy.sequence)
                .unwrap()
                .unwrap()
                .source_unavailable
        );
        let cut = store.observation_read_cut().unwrap();
        assert_eq!(
            store
                .transcript_header_page(&cut, None, 1)
                .unwrap()
                .headers
                .len(),
            1
        );
        assert!(
            store
                .observation_event_witness(next, healthy.sequence)
                .unwrap()
                .unwrap()
                .source_unavailable
        );
    }
}

#[test]
fn prepared_adoption_children_preserve_immediate_foreign_keys_and_isolate_partial_failure() {
    for reject_child in [false, true] {
        let store = store();
        store.initialize(&[]).unwrap();
        let mut watermark = None;
        for index in 0..3 {
            watermark = Some(
                store
                    .accept_inbound(draft(&format!("prepared adopted {index}")))
                    .unwrap()
                    .sequence,
            );
        }
        if reject_child {
            store.lock().unwrap().execute_batch("ALTER TABLE trace_adoption_members ADD COLUMN reject_second INTEGER CHECK(ordinal!=1);").unwrap();
        }
        let writes = count_event_writes(&store);
        event_insert::take_work();
        let batch = store.adopt_pending_batch(watermark.unwrap(), None).unwrap();
        let event = batch.adoption.unwrap();
        let connection = store.lock().unwrap();
        let state:(u64,u64,u64,bool)=connection.query_row("SELECT (SELECT count(*) FROM events),(SELECT count(*) FROM trace_adoption_members),(SELECT count(*) FROM pragma_foreign_key_check),observation_trace_fact IS NOT NULL FROM events WHERE sequence=?1",[event.sequence],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))).unwrap();
        assert_eq!(
            state,
            (1, if reject_child { 0 } else { 3 }, 0, !reject_child)
        );
        drop(connection);
        assert_eq!(store.load_canonical().unwrap().len(), 3);
        assert!(store.load_pending().unwrap().is_empty());
        assert_eq!(
            store.read_events(None, 8).unwrap().events[0].event,
            event.event
        );
        let cut = store.observation_read_cut().unwrap();
        let page = store.observation_trace_adoption(&cut, event.sequence, 0, 8);
        if reject_child {
            assert_eq!(event_insert::take_work(), (0, 1));
            assert!(page.is_err());
            store
                .lock()
                .unwrap()
                .execute_batch("ALTER TABLE trace_adoption_members DROP COLUMN reject_second;")
                .unwrap();
            let accepted = store
                .accept_inbound(draft("healthy adoption after child failure"))
                .unwrap();
            let next = store
                .adopt_pending_batch(accepted.sequence, None)
                .unwrap()
                .adoption
                .unwrap();
            let cut = store.observation_read_cut().unwrap();
            let page = store
                .observation_trace_adoption(&cut, next.sequence, 0, 8)
                .unwrap();
            assert_eq!(page.total, 1);
            assert_eq!(page.items.len(), 1);
            assert!(
                store
                    .observation_trace_adoption(&cut, event.sequence, 0, 8)
                    .is_err()
            );
        } else {
            assert_eq!(event_insert::take_work(), (1, 0));
            assert_eq!(*writes.lock().unwrap(), (1, 0));
            let page = page.unwrap();
            assert_eq!(page.total, 3);
            assert_eq!(page.items.len(), 3);
        }
    }
}

#[test]
fn prepared_tool_definition_child_rejection_keeps_own_fact_and_later_catalog_independent() {
    use crate::durable::observation::{TraceFactKind, TraceFactQuery, TraceFactScope};
    let store = store();
    store.initialize(&[]).unwrap();
    let (request, _) = inspection_request(&store, 0, "original model definition");
    store.lock().unwrap().execute_batch("ALTER TABLE canonical_tool_calls ADD COLUMN reject_definition_commit INTEGER CHECK(observation_definition_commit IS NULL);").unwrap();
    let writes = count_event_writes(&store);
    event_insert::take_work();
    let (message, event, occurrence) = inspection_assistant(&store, &request);
    assert_eq!(event_insert::take_work(), (1, 0));
    assert_eq!(*writes.lock().unwrap(), (1, 0));
    assert!(!event.observation.as_ref().unwrap().source_unavailable);
    let cut = store.observation_read_cut().unwrap();
    assert!(matches!(
        store.observation_trace_tool_definition(&cut, &occurrence),
        Err(ConversationStoreError::Storage(_))
    ));
    let own = store
        .observation_trace_facts(
            &cut,
            &TraceFactQuery {
                scope: TraceFactScope::All,
                kinds: vec![TraceFactKind::AssistantMessageCommitted],
                before: None,
                after: 0,
                ascending: true,
                limit: 1,
            },
        )
        .unwrap();
    assert_eq!(own[0].sequence, event.sequence);
    assert!(
        store
            .observation_trace_tool_proposal(&cut, &occurrence)
            .is_ok()
    );
    assert_eq!(store.load_canonical().unwrap(), [message]);
    assert_eq!(
        store
            .read_events(Some(event.sequence - 1), 1)
            .unwrap()
            .events[0],
        event
    );
    let connection = store.lock().unwrap();
    let foreign_keys: u64 = connection
        .query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(foreign_keys, 0);
    connection
        .execute_batch("ALTER TABLE canonical_tool_calls DROP COLUMN reject_definition_commit;")
        .unwrap();
    drop(connection);
    inspection_result(&store, &request);
    let (next, _) = inspection_request(&store, 1, "later original model definition");
    let (_, _, next_occurrence) = inspection_assistant(&store, &next);
    let current = store.observation_read_cut().unwrap();
    assert!(
        store
            .observation_trace_tool_definition(&current, &next_occurrence)
            .unwrap()
            .is_some()
    );
    assert!(matches!(
        store.observation_trace_tool_definition(&cut, &occurrence),
        Err(ConversationStoreError::Storage(_))
    ));
}
