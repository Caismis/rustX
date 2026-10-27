use crate::durable::observation::{
    ObservationReadCut, ResponsePageSelection, ResponsePageSelectionKind,
};
use crate::runtime_client::snapshot::RuntimeClientTranscriptPage;

fn response_page_at(
    store: &SqliteConversationStore,
    cut: &ObservationReadCut,
) -> RuntimeClientTranscriptPage {
    crate::runtime_client::snapshot::transcript_header_view(
        store.transcript_header_page(cut, None, 32).unwrap(),
    )
}
fn assert_response_page_oracle(
    store: &SqliteConversationStore,
    cut: &ObservationReadCut,
) -> RuntimeClientTranscriptPage {
    let mut current = response_page_at(store, cut);
    let mut original = current.clone();
    crate::runtime_client::response::native_oracle_decorate_through(
        store,
        &mut original,
        cut.read.journal,
    )
    .unwrap();
    crate::runtime_client::response::decorate_through(store, &mut current, cut.read.journal)
        .unwrap();
    assert_eq!(current, original);
    current
}
fn response_start(store: &SqliteConversationStore, name: &str) -> AttemptId {
    let attempt = AttemptId::new(format!("response-{name}"));
    store
        .append_event(envelope(
            store.conversation_id(),
            &format!("{name}-start"),
            Some(attempt.clone()),
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt.clone(),
            },
        ))
        .unwrap();
    attempt
}
fn response_message(store: &SqliteConversationStore, attempt: &AttemptId, id: &str) {
    store
        .append_canonical_with_event(
            &assistant_message(id, "original exact body 水"),
            envelope(
                store.conversation_id(),
                &format!("event-{id}"),
                Some(attempt.clone()),
                RuntimeEvent::AssistantMessageCommitted {
                    message_id: MessageId::new(id),
                },
            ),
        )
        .unwrap();
}
fn response_complete(store: &SqliteConversationStore, attempt: &AttemptId) {
    store
        .append_event(envelope(
            store.conversation_id(),
            &format!("{}-end", attempt.as_str()),
            Some(attempt.clone()),
            RuntimeEvent::AttemptCompleted {
                attempt_id: attempt.clone(),
                finish_reason: ModelFinishReason::Stop,
            },
        ))
        .unwrap();
}

#[test]
fn bounded_response_page_reuses_one_owner_at_running_completed_and_old_cuts() {
    let store = store();
    store.initialize(&[]).unwrap();
    let attempt = response_start(&store, "shared");
    for index in 0..32 {
        response_message(&store, &attempt, &format!("shared-{index}"));
    }
    let running_cut = store.observation_read_cut().unwrap();
    let running = assert_response_page_oracle(&store, &running_cut);
    assert_eq!(running.entries.len(), 32);
    assert!(running.entries.iter().all(|entry| entry.response_pending));
    response_complete(&store, &attempt);
    assert_eq!(assert_response_page_oracle(&store, &running_cut), running);
    let cut = store.observation_read_cut().unwrap();
    let complete = assert_response_page_oracle(&store, &cut);
    assert_eq!(
        complete
            .entries
            .iter()
            .filter(|entry| entry.completed_response.is_some())
            .count(),
        1
    );
    let mut page = response_page_at(&store, &cut);
    super::super::observation_response::read_work();
    crate::runtime_client::response::decorate_through(&store, &mut page, cut.read.journal).unwrap();
    assert_eq!(
        super::super::observation_response::read_work(),
        (1, 1),
        "32 rows share one selected execution and member decode"
    );
}

#[test]
fn bounded_response_page_reads_every_distinct_owner() {
    let store = store();
    store.initialize(&[]).unwrap();
    for index in 0..32 {
        let name = format!("distinct-{index}");
        let attempt = response_start(&store, &name);
        response_message(&store, &attempt, &name);
        response_complete(&store, &attempt);
    }
    let cut = store.observation_read_cut().unwrap();
    let page = assert_response_page_oracle(&store, &cut);
    assert_eq!(
        page.entries
            .iter()
            .filter(|entry| entry.completed_response.is_some())
            .count(),
        32
    );
    let mut page = response_page_at(&store, &cut);
    super::super::observation_response::read_work();
    crate::runtime_client::response::decorate_through(&store, &mut page, cut.read.journal).unwrap();
    assert_eq!(super::super::observation_response::read_work(), (32, 32));
}

#[test]
fn bounded_response_page_combines_ordinary_and_deferred_limit() {
    let store = store();
    store.initialize(&[]).unwrap();
    let attempt = response_start(&store, "mixed");
    for index in 0..32 {
        let id = if index % 2 == 0 {
            format!("mixed-{index}")
        } else {
            format!("mixed-{index}-{}", "水".repeat(400))
        };
        response_message(&store, &attempt, &id);
    }
    let running = store.observation_read_cut().unwrap();
    assert_response_page_oracle(&store, &running);
    response_complete(&store, &attempt);
    let cut = store.observation_read_cut().unwrap();
    let mut page = assert_response_page_oracle(&store, &cut);
    assert!(!page.entries.is_empty() && !page.deferred_entries.is_empty());
    assert_eq!(page.entries.len() + page.deferred_entries.len(), 32);
    page.deferred_entries.push(page.deferred_entries[0].clone());
    assert!(matches!(
        crate::runtime_client::response::decorate_through(&store, &mut page, cut.read.journal),
        Err(ConversationStoreError::InvalidReference(_))
    ));
    let selection = ResponsePageSelection {
        cursor: cut.read.transcript + 1,
        kind: ResponsePageSelectionKind::Process,
    };
    assert!(matches!(
        store.observation_response_page(&cut, &[selection]),
        Err(ConversationStoreError::InvalidReference(_))
    ));
}

#[test]
fn bounded_response_page_keeps_terminal_only_and_failed_message_semantics() {
    use crate::events::types::AttemptFailure;
    use crate::runtime::types::RuntimeError;
    for failed in [false, true] {
        for has_message in [false, true] {
            let store = store();
            store.initialize(&[]).unwrap();
            let attempt = response_start(&store, "terminal");
            if has_message {
                response_message(&store, &attempt, "before-terminal");
            }
            let event = if failed {
                RuntimeEvent::AttemptFailed {
                    attempt_id: attempt.clone(),
                    error: AttemptFailure::Runtime {
                        error: RuntimeError::Internal {
                            message: "original failure 水".repeat(300),
                        },
                    },
                }
            } else {
                RuntimeEvent::AttemptCancelled {
                    attempt_id: attempt.clone(),
                    reason: CancellationReason::UserRequested,
                }
            };
            store
                .append_event(envelope(
                    store.conversation_id(),
                    "terminal-end",
                    Some(attempt),
                    event,
                ))
                .unwrap();
            let page = assert_response_page_oracle(&store, &store.observation_read_cut().unwrap());
            assert!(
                page.entries
                    .iter()
                    .all(|entry| entry.completed_response.is_none() && !entry.response_pending)
            );
            assert!(
                page.entries
                    .iter()
                    .any(|entry| entry.turn_process.is_some())
            );
        }
    }
}

#[test]
fn bounded_response_page_preserves_inherited_membership_and_corruption_refusal() {
    for id in ["inherited-response".to_owned(), "inherited水".repeat(300)] {
        let store = store();
        store
            .initialize_lineage(&member_completed_lineage(&id))
            .unwrap();
        let cut = store.observation_read_cut().unwrap();
        assert_response_page_oracle(&store, &cut);
        let page = response_page_at(&store, &cut);
        store.lock().unwrap().execute_batch("PRAGMA ignore_check_constraints=ON; UPDATE inherited_message_membership SET observation_membership_witness=NULL; PRAGMA ignore_check_constraints=OFF;").unwrap();
        let mut old = page.clone();
        let mut bulk = page;
        assert!(
            crate::runtime_client::response::native_oracle_decorate_through(
                &store,
                &mut old,
                cut.read.journal
            )
            .is_err()
        );
        assert!(
            crate::runtime_client::response::decorate_through(&store, &mut bulk, cut.read.journal)
                .is_err()
        );
    }
}

#[test]
fn bounded_response_page_reopens_and_preserves_old_cut_after_external_append() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("responses.sqlite");
    let conversation = ConversationId::generate();
    let writer = SqliteConversationStore::open(conversation.clone(), &path).unwrap();
    writer.initialize(&[]).unwrap();
    let attempt = response_start(&writer, "reopen");
    response_message(&writer, &attempt, "reopen-message");
    response_complete(&writer, &attempt);
    let cut = writer.observation_read_cut().unwrap();
    let expected = assert_response_page_oracle(&writer, &cut);
    let reader = SqliteConversationStore::open(conversation.clone(), &path).unwrap();
    let later = response_start(&writer, "later");
    response_message(&writer, &later, "later-message");
    response_complete(&writer, &later);
    assert_eq!(assert_response_page_oracle(&reader, &cut), expected);
    drop(reader);
    drop(writer);
    let reader = SqliteConversationStore::open(conversation, &path).unwrap();
    assert_eq!(assert_response_page_oracle(&reader, &cut), expected);
    assert_response_page_oracle(&reader, &reader.observation_read_cut().unwrap());
}

#[test]
fn bounded_response_page_keeps_execution_hole_and_selected_source_validation() {
    for column in ["observation_execution", "observation_members"] {
        let store = store();
        store.initialize(&[]).unwrap();
        let attempt = response_start(&store, "corrupt");
        response_message(&store, &attempt, "corrupt-message");
        response_complete(&store, &attempt);
        let cut = store.observation_read_cut().unwrap();
        let page = response_page_at(&store, &cut);
        // An earlier hole must still fail when the selected terminal layout is intact.
        store
            .lock()
            .unwrap()
            .execute(
                &format!(
                    "UPDATE events SET {column}=NULL WHERE observation_assistant_ledger IS NOT NULL"
                ),
                [],
            )
            .unwrap();
        let mut old = page.clone();
        let mut bulk = page;
        assert!(
            crate::runtime_client::response::native_oracle_decorate_through(
                &store,
                &mut old,
                cut.read.journal
            )
            .is_err()
        );
        assert!(
            crate::runtime_client::response::decorate_through(&store, &mut bulk, cut.read.journal)
                .is_err()
        );
    }
}

#[test]
fn bounded_response_page_keeps_tool_and_assistant_owners_distinct_from_aliases() {
    let (store, attempt, proposal, call) = original_tool_before_result();
    commit_original_tool_result(
        &store,
        &attempt,
        &proposal,
        &call,
        &original_success_tool_result(),
    );
    response_message(&store, &attempt, "tool-final");
    response_complete(&store, &attempt);
    let cut = store.observation_read_cut().unwrap();
    assert_response_page_oracle(&store, &cut);
    let other = response_start(&store, "other-owner");
    response_message(&store, &other, "other-final");
    response_complete(&store, &other);
    let cut = store.observation_read_cut().unwrap();
    let position = store
        .message_ledger_position(&MessageId::new("other-final"))
        .unwrap()
        .unwrap();
    let mut page = response_page_at(&store, &cut);
    page.entries.retain(|entry| matches!(&entry.item, crate::runtime_client::snapshot::RuntimeClientTranscriptItem::Message { message: MessageBlock::Assistant(message) } if message.id.as_str() == "other-final"));
    page.deferred_entries.clear();
    assert_eq!(page.entries.len(), 1);
    // Read the native page before corrupting the alias: tool header enrichment
    // independently refuses a result alias that points at an Assistant body.
    // Preserve the old distinction even when a result alias points at an
    // Assistant row: response source follows it, process membership does not.
    store
        .lock()
        .unwrap()
        .execute(
            "UPDATE canonical_tool_calls SET result_ledger_position=?1",
            [position],
        )
        .unwrap();
    let mut original = page.clone();
    crate::runtime_client::response::native_oracle_decorate_through(
        &store,
        &mut original,
        cut.read.journal,
    )
    .unwrap();
    crate::runtime_client::response::decorate_through(&store, &mut page, cut.read.journal).unwrap();
    assert_eq!(page, original);
    assert_eq!(
        page.entries[0]
            .turn_process
            .as_ref()
            .unwrap()
            .attempt_id
            .as_ref(),
        Some(&other)
    );
}

#[test]
fn bounded_response_page_does_not_read_retry_for_running_response() {
    for deferred in [false, true] {
        let store = store();
        store.initialize(&[]).unwrap();
        let attempt = response_start(&store, "lazy");
        let id = if deferred {
            "lazy水".repeat(300)
        } else {
            "lazy".to_owned()
        };
        response_message(&store, &attempt, &id);
        let cut = store.observation_read_cut().unwrap();
        {
            let connection = store.lock().unwrap();
            let (sequence, bytes): (u64, Vec<u8>) = connection.query_row("SELECT sequence,observation_execution FROM events WHERE observation_assistant_ledger IS NOT NULL", [], |row| Ok((row.get(0)?, row.get(1)?))).unwrap();
            let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            value["summary"]["last_request_sequence"] = serde_json::json!(999_999);
            connection
                .execute(
                    "UPDATE events SET observation_execution=?2 WHERE sequence=?1",
                    params![sequence, serde_json::to_vec(&value).unwrap()],
                )
                .unwrap();
        }
        let page = assert_response_page_oracle(&store, &cut);
        assert!(
            page.entries
                .iter()
                .all(|entry| entry.completed_response.is_none())
        );
    }
}
