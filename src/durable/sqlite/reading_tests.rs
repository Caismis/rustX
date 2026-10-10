use crate::durable::reading::*;
use crate::durable::{ConversationStore, LineageSeed, SqliteConversationStore};
use crate::events::types::{RuntimeEvent, RuntimeEventEnvelope};
use crate::message::TextBlock;
use crate::message::types::{
    AssistantContentBlock, AssistantMessageBlock, InboundKind, MessageBlock, UserContentBlock,
    UserMessageBlock, UserSource,
};
use crate::runtime::identity::{AttemptId, ConversationId, EventId, MessageId};
use chrono::Utc;
/// The exact transcript entry at a turn's native location, read as the page
/// before the position just after it.
fn located(
    store: &SqliteConversationStore,
    turn: &ConversationTurn,
) -> crate::durable::TranscriptEntry {
    let cursor = turn.cursor.expect("located turn");
    let window = store
        .conversation_window(
            &ConversationWindowAt::Older {
                cut: None,
                before: crate::durable::TranscriptCursor::new(cursor.get() + 1),
            },
            1,
        )
        .unwrap();
    assert_eq!(window.page.entries[0].cursor, cursor);
    window.page.entries.into_iter().next().unwrap()
}

#[test]
fn inherited_boundary_includes_terminal_only_turns_and_survives_local_appends() {
    let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    assert_eq!(
        store
            .load_transcript_page(None, 64)
            .unwrap()
            .inherited_through
            .get(),
        0
    );
    let seed = LineageSeed::history(vec![message("seeded", "inherited")])
        .with_turns(vec![TurnReadingProvenance {
            id: ConversationTurnId {
                conversation_id: ConversationId::generate(),
                attempt_id: AttemptId::new("empty"),
            },
            process_message_ids: Vec::new(),
            preceding_message_id: Some(MessageId::new("seeded")),
            prompt_message_id: None,
            outcome: InheritedTurnOutcome::TimedOut,
            started_at: None,
            ended_at: Some(Utc::now()),
            execution: None,
        }])
        .unwrap();
    store.initialize_lineage(&seed).unwrap();
    let seeded = store.load_transcript_page(None, 64).unwrap();
    assert_eq!(seeded.inherited_through.get(), 2);
    assert_eq!(
        seeded.entries.last().unwrap().cursor,
        seeded.inherited_through
    );
    start(&store, "local");
    commit(&store, "local", "local-message");
    timeout(&store, "local");
    let newest = store
        .conversation_window(&ConversationWindowAt::Latest, 1)
        .unwrap();
    assert_eq!(newest.page.inherited_through, seeded.inherited_through);
    let older = store
        .conversation_window(
            &ConversationWindowAt::Older {
                cut: None,
                before: newest.page.entries[0].cursor,
            },
            64,
        )
        .unwrap();
    assert_eq!(older.page.inherited_through, seeded.inherited_through);
}

fn event(
    store: &SqliteConversationStore,
    attempt: &str,
    kind: RuntimeEvent,
) -> RuntimeEventEnvelope {
    RuntimeEventEnvelope {
        schema_version: 1,
        event_id: EventId::new(format!(
            "event-{}",
            store.presentation_frontier().unwrap() + 1
        )),
        sequence: 0,
        conversation_id: store.conversation_id().clone(),
        attempt_id: Some(AttemptId::new(attempt)),
        turn_id: None,
        timestamp: Utc::now(),
        event: kind,
    }
}
fn start(store: &SqliteConversationStore, attempt: &str) {
    store
        .append_event(event(
            store,
            attempt,
            RuntimeEvent::AttemptStarted {
                attempt_id: AttemptId::new(attempt),
            },
        ))
        .unwrap();
}
fn message(id: &str, text: &str) -> MessageBlock {
    MessageBlock::Assistant(AssistantMessageBlock {
        id: MessageId::new(id),
        content: vec![AssistantContentBlock::Text(TextBlock { text: text.into() })],
    })
}
fn commit(store: &SqliteConversationStore, attempt: &str, id: &str) {
    store
        .append_canonical_with_event(
            &message(id, &"界".repeat(600)),
            event(
                store,
                attempt,
                RuntimeEvent::AssistantMessageCommitted {
                    message_id: MessageId::new(id),
                },
            ),
        )
        .unwrap();
}
fn timeout(store: &SqliteConversationStore, attempt: &str) {
    store
        .append_event(event(
            store,
            attempt,
            RuntimeEvent::AttemptTimedOut {
                attempt_id: AttemptId::new(attempt),
            },
        ))
        .unwrap();
}
fn human(id: &str, text: &str) -> MessageBlock {
    MessageBlock::User(UserMessageBlock {
        id: MessageId::new(id),
        content: vec![UserContentBlock::Text(TextBlock { text: text.into() })],
        source: UserSource::Human,
        kind: InboundKind::Message,
        timestamp: None,
    })
}
fn commit_text(store: &SqliteConversationStore, attempt: &str, id: &str, text: &str) {
    store
        .append_canonical_with_event(
            &message(id, text),
            event(
                store,
                attempt,
                RuntimeEvent::AssistantMessageCommitted {
                    message_id: MessageId::new(id),
                },
            ),
        )
        .unwrap();
}
/// Accepts and adopts one inbound message, idle (`None`) or into `attempt`.
fn adopt(store: &SqliteConversationStore, source: UserSource, text: &str, attempt: Option<&str>) {
    use crate::durable::inbox::InboundDraft;
    let accepted = store
        .accept_inbound(InboundDraft {
            message_id: None,
            source,
            kind: InboundKind::Message,
            content: vec![UserContentBlock::Text(TextBlock { text: text.into() })],
            timestamp: Utc::now(),
            correlation: None,
        })
        .unwrap();
    store
        .adopt_pending_batch(accepted.sequence, attempt.map(AttemptId::new))
        .unwrap();
}
#[test]
fn turn_previews_are_the_opening_human_prompt_and_the_settled_final_response() {
    let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    // Turn one opens from an idle adoption; a steer adopted into it is not its prompt.
    adopt(&store, UserSource::Human, "Open   turn\none", None);
    start(&store, "a1");
    commit_text(&store, "a1", "a1-first", "First answer");
    adopt(&store, UserSource::Human, "Steer turn one", Some("a1"));
    commit_text(&store, "a1", "a1-final", "Final answer one");
    // A queued message adopted after the steer still belongs to turn two.
    adopt(&store, UserSource::Human, "Queued for two", None);
    let running = store.conversation_turns(0, 64).unwrap();
    assert_eq!(
        (
            running.turns[0].prompt.as_str(),
            running.turns[0].response.as_str()
        ),
        ("Open turn one", ""),
        "a turn still running at the cut has no final response",
    );
    timeout(&store, "a1");
    start(&store, "a2");
    commit_text(&store, "a2", "a2-final", "Answer two");
    timeout(&store, "a2");
    // Non-human inbound never opens a turn's prompt; automatic continuation has none.
    adopt(&store, UserSource::ExternalSystem, "System notice", None);
    start(&store, "a3");
    timeout(&store, "a3");
    let outline = store.conversation_turns(0, 64).unwrap();
    let previews: Vec<_> = outline
        .turns
        .iter()
        .map(|turn| (turn.prompt.as_str(), turn.response.as_str()))
        .collect();
    assert_eq!(
        previews,
        [
            ("Open turn one", "Final answer one"),
            ("Queued for two", "Answer two"),
            ("", ""),
        ]
    );
    // An idle adoption without a new Attempt opens no turn and moves no preview.
    adopt(&store, UserSource::Human, "Later", None);
    assert_eq!(
        store.conversation_turns(0, 64).unwrap().turns,
        outline.turns
    );
}
#[test]
fn distant_turn_locations_are_exact_bounded_and_read_only() {
    let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    for n in 0..600 {
        let id = format!("attempt-{n}");
        start(&store, &id);
        commit(&store, &id, &format!("m{n}"));
        timeout(&store, &id);
    }
    let before = store.conversation_read_cut().unwrap();
    let outline = store.conversation_turns(0, 64).unwrap();
    assert_eq!(outline.total, 600);
    assert_eq!(outline.turns.len(), 64);
    // One rail-card line of prompt, three of response: bounded and marked clipped.
    assert_eq!(outline.turns[0].prompt, "");
    assert_eq!(outline.turns[0].response.chars().count(), 120);
    assert!(outline.turns[0].response.ends_with('…'));
    let target = &outline.turns[0];
    let latest = store
        .conversation_window(&ConversationWindowAt::Latest, 64)
        .unwrap();
    assert!(latest.page.entries[0].cursor.get() - target.cursor.unwrap().get() > 512);
    assert!(
        matches!(&located(&store, target).item,crate::durable::TranscriptItem::Message{message:MessageBlock::Assistant(message)} if message.id.as_str()=="m0")
    );
    // Older pages continue strictly before the page already read.
    let older = store
        .conversation_window(
            &ConversationWindowAt::Older {
                cut: None,
                before: latest.page.entries[0].cursor,
            },
            64,
        )
        .unwrap();
    assert_eq!(older.page.entries.len(), 64);
    assert!(older.page.entries.last().unwrap().cursor < latest.page.entries[0].cursor);
    assert_eq!(older.page.next_cursor, Some(older.page.entries[0].cursor));
    assert_eq!(
        store.conversation_read_cut().unwrap(),
        before,
        "reads neither execute nor commit any model/Tool work"
    );
    assert_eq!(store.read_request_snapshots(None, 1).unwrap().snapshots, []);
}
#[test]
fn paging_terminal_empty_and_running_attempts_keep_native_identity_order() {
    let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    start(&store, "interrupted");
    timeout(&store, "interrupted");
    start(&store, "multiple-members");
    commit(&store, "multiple-members", "first");
    commit(&store, "multiple-members", "second");
    timeout(&store, "multiple-members");
    start(&store, "running");
    let all = store.conversation_turns(0, 3).unwrap();
    assert_eq!(
        all.turns
            .iter()
            .map(|turn| turn.id.attempt_id.as_str())
            .collect::<Vec<_>>(),
        ["interrupted", "multiple-members", "running"]
    );
    assert_eq!(all.turns[0].cursor.unwrap().get(), 1);
    assert_eq!(all.turns[1].cursor.unwrap().get(), 2);
    assert!(all.turns[2].cursor.is_none());
    for offset in 0..3 {
        assert_eq!(
            store.conversation_turns(offset, 1).unwrap().turns[0],
            all.turns[offset]
        );
    }
    assert!(matches!(
        located(&store, &all.turns[0]).item,
        crate::durable::TranscriptItem::AttemptTerminal { .. }
    ));
}
#[test]
fn copied_turns_preserve_origin_and_use_destination_locations() {
    let source = ConversationId::generate();
    let destination = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    let provenance = crate::durable::response::CompletedResponseProvenance {
        process_message_ids: vec![
            MessageId::new("copied-first"),
            MessageId::new("copied-final"),
        ],
        closing_message_id: MessageId::new("copied-final"),
        origin: crate::durable::response::ResponseOrigin {
            conversation_id: source.clone(),
            attempt_id: AttemptId::new("origin-attempt"),
            closing_message_id: MessageId::new("source-final"),
        },
        completed_at: Utc::now(),
        retry_message_id: None,
        usage: None,
        timing: None,
        models: vec!["origin-model".into()],
    };
    destination
        .initialize_lineage(
            &LineageSeed::history(vec![
                message("unowned", "canonical text without a native owner"),
                human("copied-prompt", "  Inherited\n prompt  "),
                message("copied-first", "first"),
                message("copied-final", "final"),
            ])
            .with_turns(vec![TurnReadingProvenance {
                id: ConversationTurnId {
                    conversation_id: source.clone(),
                    attempt_id: AttemptId::new("origin-attempt"),
                },
                process_message_ids: provenance.process_message_ids.clone(),
                preceding_message_id: None,
                prompt_message_id: Some(MessageId::new("copied-prompt")),
                outcome: InheritedTurnOutcome::Completed,
                started_at: None,
                ended_at: Some(provenance.completed_at),
                execution: None,
            }])
            .unwrap()
            .with_completed_responses(vec![provenance])
            .unwrap(),
        )
        .unwrap();
    start(&destination, "new-local");
    timeout(&destination, "new-local");
    let outline = destination.conversation_turns(0, 64).unwrap();
    assert_eq!(outline.total, 2);
    assert_eq!(outline.turns[0].id.conversation_id, source);
    assert_eq!(outline.turns[0].cursor.unwrap().get(), 3);
    // Inherited previews come from immutable provenance: its prompt and its
    // newest text-bearing member. The new local turn has neither.
    assert_eq!(
        (
            outline.turns[0].prompt.as_str(),
            outline.turns[0].response.as_str()
        ),
        ("Inherited prompt", "final")
    );
    assert_eq!(
        (
            outline.turns[1].prompt.as_str(),
            outline.turns[1].response.as_str()
        ),
        ("", "")
    );
    assert!(
        matches!(&located(&destination, &outline.turns[0]).item,crate::durable::TranscriptItem::Message{message:MessageBlock::Assistant(message)} if message.id.as_str()=="copied-first")
    );
}

#[test]
fn inherited_prompt_previews_use_explicit_opening_and_response_input_owners() {
    for (opening, retry, expected) in [
        (None, Some("opening"), "Original question"),
        (Some("opening"), Some("steering"), "Original question"),
        (None, None, ""),
    ] {
        let source = ConversationId::generate();
        let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
        let response = crate::durable::response::CompletedResponseProvenance {
            process_message_ids: vec![MessageId::new("first"), MessageId::new("final")],
            closing_message_id: MessageId::new("final"),
            origin: crate::durable::response::ResponseOrigin {
                conversation_id: source.clone(),
                attempt_id: AttemptId::new("inherited"),
                closing_message_id: MessageId::new("source-final"),
            },
            completed_at: Utc::now(),
            retry_message_id: retry.map(MessageId::new),
            usage: None,
            timing: None,
            models: Vec::new(),
        };
        let seed = LineageSeed::history(vec![
            human("opening", "Original question"),
            message("first", "first response"),
            human("steering", "Later steering question"),
            message("final", "final response"),
        ])
        .with_turns(vec![TurnReadingProvenance {
            id: ConversationTurnId {
                conversation_id: source,
                attempt_id: AttemptId::new("inherited"),
            },
            process_message_ids: response.process_message_ids.clone(),
            preceding_message_id: None,
            prompt_message_id: opening.map(MessageId::new),
            outcome: InheritedTurnOutcome::Completed,
            started_at: None,
            ended_at: Some(response.completed_at),
            execution: None,
        }])
        .unwrap()
        .with_completed_responses(vec![response])
        .unwrap();
        store.initialize_lineage(&seed).unwrap();
        let outline = store.conversation_turns(0, 64).unwrap();
        assert_eq!(outline.turns[0].prompt, expected);
        assert_eq!(outline.turns[0].response, "final response");
    }
}
#[test]
fn appends_keep_turn_locations_and_reads_stay_bounded() {
    let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    start(&store, "a");
    commit(&store, "a", "a-first");
    let first = store.conversation_turns(0, 64).unwrap();
    commit(&store, "a", "a-newer");
    let later = store.conversation_turns(0, 64).unwrap();
    assert_eq!(later.turns[0].cursor, first.turns[0].cursor);
    assert!(later.cut.transcript > first.cut.transcript);
    assert_eq!(later.cut.mutation_revision, first.cut.mutation_revision);
    assert!(
        matches!(&located(&store, &later.turns[0]).item,crate::durable::TranscriptItem::Message{message:MessageBlock::Assistant(message)} if message.id.as_str()=="a-first")
    );
    assert!(store.conversation_turns(0, 65).is_err());
    assert!(
        store
            .conversation_window(&ConversationWindowAt::Latest, 65)
            .is_err()
    );
}

#[test]
fn turn_locations_appear_once_native_work_commits() {
    let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    start(&store, "running");
    let old = store.conversation_turns(0, 64).unwrap();
    assert_eq!(old.turns[0].cursor, None);
    commit(&store, "running", "first-location");
    start(&store, "new-attempt");
    commit(&store, "new-attempt", "new-location");
    let fresh = store.conversation_turns(0, 64).unwrap();
    assert_eq!(
        fresh
            .turns
            .iter()
            .map(|turn| (turn.id.attempt_id.as_str(), turn.cursor.unwrap().get()))
            .collect::<Vec<_>>(),
        [("running", 1), ("new-attempt", 2)]
    );
    for turn in &fresh.turns {
        located(&store, turn);
    }
}

#[test]
fn pending_edits_and_removals_advance_the_mutation_epoch_without_moving_turns() {
    use crate::durable::inbox::{InboundDraft, PendingInboundRef, PendingMutationOutcome};
    use crate::message::types::{InboundKind, UserContentBlock, UserSource};
    let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    start(&store, "a");
    commit(&store, "a", "a-first");
    let accepted = store
        .accept_inbound(InboundDraft {
            message_id: None,
            source: UserSource::Human,
            kind: InboundKind::Message,
            content: vec![UserContentBlock::Text(TextBlock {
                text: "queued".into(),
            })],
            timestamp: Utc::now(),
            correlation: None,
        })
        .unwrap();
    let first = store.conversation_turns(0, 64).unwrap();
    let expected = PendingInboundRef {
        sequence: accepted.sequence,
        message_id: accepted.message_id,
        revision: 0,
    };
    assert_eq!(
        store.edit_pending(&expected, "edited").unwrap(),
        PendingMutationOutcome::Applied
    );
    let edited = store.conversation_turns(0, 64).unwrap();
    assert_eq!(edited.turns, first.turns);
    assert_eq!(
        (edited.cut.journal, edited.cut.transcript),
        (first.cut.journal, first.cut.transcript)
    );
    assert_eq!(edited.cut.mutation_revision, 1);
    assert!(
        store
            .conversation_window(
                &ConversationWindowAt::Turn {
                    id: first.turns[0].id.clone(),
                    cut: first.cut.clone(),
                },
                64
            )
            .is_err()
    );
    assert!(
        store
            .conversation_window(
                &ConversationWindowAt::Older {
                    before: first.turns[0].cursor.unwrap(),
                    cut: Some(first.cut.clone()),
                },
                64
            )
            .is_err()
    );
    assert_eq!(
        store.edit_pending(&expected, "stale").unwrap(),
        PendingMutationOutcome::Conflict
    );
    assert_eq!(store.conversation_read_cut().unwrap(), edited.cut);
    assert_eq!(
        store
            .remove_pending(&PendingInboundRef {
                revision: 1,
                ..expected
            })
            .unwrap(),
        PendingMutationOutcome::Applied
    );
    let removed = store.conversation_turns(0, 64).unwrap();
    assert_eq!(removed.turns, edited.turns);
    assert_eq!(removed.cut.mutation_revision, 2);
    let latest = store
        .conversation_window(&ConversationWindowAt::Latest, 64)
        .unwrap();
    assert_eq!(
        latest.page.entries.len(),
        1,
        "a retired pending body is not read"
    );
}

#[test]
fn distant_turn_is_one_bounded_native_window_at_its_cut() {
    let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    for index in 0..2048 {
        let attempt = format!("turn-{index}");
        start(&store, &attempt);
        timeout(&store, &attempt);
    }
    let outline = store.conversation_turns(0, 64).unwrap();
    assert_eq!(outline.total, 2048);
    let first = &outline.turns[0];
    let window = store
        .conversation_window(
            &ConversationWindowAt::Turn {
                id: first.id.clone(),
                cut: outline.cut.clone(),
            },
            64,
        )
        .unwrap();
    assert_eq!(window.page.entries.len(), 64);
    assert_eq!(window.target.as_ref(), Some(&first.id));
    assert_eq!(window.target_cursor, first.cursor);
    assert_eq!(window.page.entries[0].cursor, first.cursor.unwrap());
    assert_eq!(window.cut, outline.cut);
    start(&store, "live");
    timeout(&store, "live");
    let newer = store
        .conversation_window(
            &ConversationWindowAt::Newer {
                after: window.newer_cursor.unwrap(),
                cut: window.cut.clone(),
            },
            64,
        )
        .unwrap();
    assert_eq!(newer.page.entries.len(), 64);
    assert_eq!(newer.cut, window.cut);
    assert!(
        newer
            .page
            .entries
            .iter()
            .all(|entry| entry.cursor.get() <= window.cut.transcript)
    );
    assert!(store.conversation_read_cut().unwrap().transcript > window.cut.transcript);
}

#[test]
fn distant_source_message_is_one_bounded_window_at_its_native_cursor() {
    let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    let seed = LineageSeed::history(
        (0..4096)
            .map(|index| message(&format!("source-{index}"), "retained"))
            .collect(),
    );
    store.initialize_lineage(&seed).unwrap();
    let cut = store.conversation_read_cut().unwrap();
    let id = MessageId::new("source-1");
    let cursor = store.message_transcript_cursor(&id).unwrap().unwrap();
    let window = store
        .conversation_window(
            &ConversationWindowAt::Message {
                id: id.clone(),
                cut: Some(cut.clone()),
            },
            64,
        )
        .unwrap();
    assert_eq!(window.target, None);
    assert_eq!(window.target_cursor, Some(cursor));
    assert_eq!(window.page.entries.len(), 64);
    assert_eq!(window.page.entries[0].cursor, cursor);
    assert!(
        matches!(&window.page.entries[0].item, crate::durable::TranscriptItem::Message { message } if message.id() == &id)
    );
    assert_eq!(window.cut, cut);
    assert_eq!(
        store.conversation_read_cut().unwrap(),
        cut,
        "position reads are side-effect free"
    );
    assert!(
        store
            .conversation_window(
                &ConversationWindowAt::Message {
                    id: MessageId::new("foreign"),
                    cut: None
                },
                64
            )
            .is_err()
    );
}

#[test]
fn message_windows_reject_foreign_future_and_mutated_cuts() {
    use crate::durable::inbox::{InboundDraft, PendingInboundRef, PendingMutationOutcome};
    let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    store
        .initialize_lineage(&LineageSeed::history(vec![message("source", "retained")]))
        .unwrap();
    let id = MessageId::new("source");
    let cut = store.conversation_read_cut().unwrap();
    let mut foreign = cut.clone();
    foreign.conversation_id = ConversationId::generate();
    assert!(
        store
            .conversation_window(
                &ConversationWindowAt::Message {
                    id: id.clone(),
                    cut: Some(foreign)
                },
                64
            )
            .is_err()
    );
    let mut future = cut.clone();
    future.transcript += 1;
    assert!(
        store
            .conversation_window(
                &ConversationWindowAt::Message {
                    id: id.clone(),
                    cut: Some(future)
                },
                64
            )
            .is_err()
    );
    start(&store, "later");
    commit_text(&store, "later", "late", "new");
    timeout(&store, "later");
    assert!(
        store
            .conversation_window(
                &ConversationWindowAt::Message {
                    id: MessageId::new("late"),
                    cut: Some(cut)
                },
                64
            )
            .is_err()
    );
    let before = store.conversation_read_cut().unwrap();
    let accepted = store
        .accept_inbound(InboundDraft {
            message_id: None,
            source: UserSource::Human,
            kind: InboundKind::Message,
            content: vec![UserContentBlock::Text(TextBlock {
                text: "queued".into(),
            })],
            timestamp: Utc::now(),
            correlation: None,
        })
        .unwrap();
    assert_eq!(
        store
            .edit_pending(
                &PendingInboundRef {
                    sequence: accepted.sequence,
                    message_id: accepted.message_id,
                    revision: 0
                },
                "edited"
            )
            .unwrap(),
        PendingMutationOutcome::Applied
    );
    assert!(
        store
            .conversation_window(
                &ConversationWindowAt::Message {
                    id: id.clone(),
                    cut: Some(before)
                },
                64
            )
            .is_err()
    );
    let fresh = store
        .conversation_window(&ConversationWindowAt::Message { id, cut: None }, 64)
        .unwrap();
    assert_eq!(fresh.cut, store.conversation_read_cut().unwrap());
}
