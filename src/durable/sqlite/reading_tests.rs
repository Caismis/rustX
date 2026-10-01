use crate::durable::reading::*;
use crate::durable::{ConversationStore, LineageSeed, SqliteConversationStore};
use crate::events::types::{RuntimeEvent, RuntimeEventEnvelope};
use crate::message::TextBlock;
use crate::message::types::{AssistantContentBlock, AssistantMessageBlock, MessageBlock};
use crate::runtime::identity::{AttemptId, ConversationId, EventId, MessageId};
use chrono::Utc;

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
#[test]
fn distant_turn_windows_are_exact_bounded_and_read_only() {
    let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    for n in 0..600 {
        let id = format!("attempt-{n}");
        start(&store, &id);
        commit(&store, &id, &format!("m{n}"));
        timeout(&store, &id);
    }
    let before = store.conversation_read_cut().unwrap();
    let outline = store.conversation_turns(None, 0, 64).unwrap();
    assert_eq!(outline.total, 600);
    assert_eq!(outline.turns.len(), 64);
    assert_eq!(outline.turns[0].preview.chars().count(), 240);
    let target = &outline.turns[0];
    let latest = store
        .conversation_window(&ConversationWindowAt::Latest, 64)
        .unwrap();
    assert!(latest.page.entries[0].cursor.get() - target.cursor.unwrap().get() > 512);
    let window = store
        .conversation_window(
            &ConversationWindowAt::Turn {
                id: target.id.clone(),
                cut: outline.cut.clone(),
            },
            64,
        )
        .unwrap();
    assert_eq!(window.page.entries.len(), 64);
    assert_eq!(
        window.page.entries[0].cursor.get(),
        target.cursor.unwrap().get()
    );
    assert!(
        matches!(&window.page.entries[0].item,crate::durable::TranscriptItem::Message{message:MessageBlock::Assistant(message)} if message.id.as_str()=="m0")
    );
    let later = store
        .conversation_window(
            &ConversationWindowAt::Newer {
                after: window.newer_cursor.unwrap(),
                cut: window.cut.clone(),
            },
            64,
        )
        .unwrap();
    assert!(later.page.entries[0].cursor > window.page.entries.last().unwrap().cursor);
    let older = store
        .conversation_window(
            &ConversationWindowAt::Older {
                before: later.page.entries[0].cursor,
                cut: Some(later.cut),
            },
            64,
        )
        .unwrap();
    assert_eq!(older.page, window.page);
    assert_eq!(
        store.conversation_read_cut().unwrap(),
        before,
        "reads neither execute nor commit any model/Tool work"
    );
    assert!(
        store
            .read_request_snapshots(None, 1)
            .unwrap()
            .snapshots
            .is_empty()
    );
}
#[test]
fn paging_failed_empty_and_automatic_attempts_keep_native_identity_order() {
    let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    start(&store, "interrupted");
    timeout(&store, "interrupted");
    start(&store, "automatic");
    commit(&store, "automatic", "first");
    commit(&store, "automatic", "second");
    timeout(&store, "automatic");
    start(&store, "running");
    let all = store.conversation_turns(None, 0, 3).unwrap();
    assert_eq!(
        all.turns
            .iter()
            .map(|turn| turn.id.attempt_id.as_str())
            .collect::<Vec<_>>(),
        ["interrupted", "automatic", "running"]
    );
    assert_eq!(all.turns[0].cursor.unwrap().get(), 1);
    assert_eq!(all.turns[1].cursor.unwrap().get(), 2);
    assert!(all.turns[2].cursor.is_none());
    for offset in 0..3 {
        assert_eq!(
            store
                .conversation_turns(Some(&all.cut), offset, 1)
                .unwrap()
                .turns[0],
            all.turns[offset]
        );
    }
    let window = store
        .conversation_window(
            &ConversationWindowAt::Turn {
                id: all.turns[0].id.clone(),
                cut: all.cut.clone(),
            },
            1,
        )
        .unwrap();
    assert!(matches!(
        window.page.entries[0].item,
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
    };
    destination
        .initialize_lineage(
            &LineageSeed::history(vec![
                message("unowned", "copied interrupted text"),
                message("copied-first", "first"),
                message("copied-final", "final"),
            ])
            .with_completed_responses(vec![provenance])
            .unwrap(),
        )
        .unwrap();
    start(&destination, "new-local");
    timeout(&destination, "new-local");
    let outline = destination.conversation_turns(None, 0, 64).unwrap();
    assert_eq!(outline.total, 2);
    assert_eq!(outline.turns[0].id.conversation_id, source);
    assert_eq!(outline.turns[0].cursor.unwrap().get(), 2);
    let window = destination
        .conversation_window(
            &ConversationWindowAt::Turn {
                id: outline.turns[0].id.clone(),
                cut: outline.cut,
            },
            1,
        )
        .unwrap();
    assert!(
        matches!(&window.page.entries[0].item,crate::durable::TranscriptItem::Message{message:MessageBlock::Assistant(message)} if message.id.as_str()=="copied-first")
    );
}
#[test]
fn stale_cuts_and_foreign_turns_are_rejected_without_nearest_fallback() {
    let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    start(&store, "a");
    commit(&store, "a", "a-first");
    let first = store.conversation_turns(None, 0, 64).unwrap();
    commit(&store, "a", "a-newer");
    assert!(
        store
            .conversation_window(
                &ConversationWindowAt::Turn {
                    id: first.turns[0].id.clone(),
                    cut: first.cut.clone()
                },
                64
            )
            .unwrap_err()
            .to_string()
            .contains("stale")
    );
    assert!(store.conversation_turns(Some(&first.cut), 0, 64).is_err());
    let fresh = store.conversation_read_cut().unwrap();
    assert!(
        store
            .conversation_window(
                &ConversationWindowAt::Turn {
                    id: ConversationTurnId {
                        conversation_id: ConversationId::generate(),
                        attempt_id: AttemptId::new("a")
                    },
                    cut: fresh
                },
                64
            )
            .is_err()
    );
    assert!(store.conversation_turns(None, 0, 65).is_err());
}

#[test]
fn pending_edits_and_removals_retire_cuts_without_moving_turns() {
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
    let first = store.conversation_turns(None, 0, 64).unwrap();
    let expected = PendingInboundRef {
        sequence: accepted.sequence,
        message_id: accepted.message_id,
        revision: 0,
    };
    assert_eq!(
        store.edit_pending(&expected, "edited").unwrap(),
        PendingMutationOutcome::Applied
    );
    let edited = store.conversation_turns(None, 0, 64).unwrap();
    assert_eq!(edited.turns, first.turns);
    assert_eq!(
        (
            edited.cut.journal,
            edited.cut.transcript,
            edited.cut.surface_revision
        ),
        (
            first.cut.journal,
            first.cut.transcript,
            first.cut.surface_revision
        )
    );
    assert_eq!(edited.cut.pending_count, 1);
    assert_eq!(edited.cut.pending_revision, 1);
    assert!(
        store
            .conversation_window(
                &ConversationWindowAt::Turn {
                    id: first.turns[0].id.clone(),
                    cut: first.cut
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
    assert!(store.conversation_turns(Some(&edited.cut), 0, 64).is_err());
    let removed = store.conversation_turns(None, 0, 64).unwrap();
    assert_eq!(removed.turns, edited.turns);
    assert_eq!(removed.cut.pending_count, 0);
    assert_eq!(removed.cut.pending_revision, 0);
    let latest = store
        .conversation_window(&ConversationWindowAt::Latest, 64)
        .unwrap();
    assert_eq!(latest.page.entries.len(), 1);
    assert!(
        latest.newer_cursor.is_none(),
        "retired pending tail is not a newer page"
    );
}
