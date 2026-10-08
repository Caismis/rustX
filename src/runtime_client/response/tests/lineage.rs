use super::*;
use crate::conversation::{SurfaceOp, message_id_of};
use crate::durable::LineageSeed;
use crate::local_runtime::session::remap_seed;

#[test]
#[allow(clippy::too_many_lines)] // One three-generation bootstrap/reopen/local-execution contract.
fn deep_lineage_reopen_preserves_response_facts_without_execution_ownership() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = SqliteConversationStore::open(
        ConversationId::new("conv_b05f9cb7-dcec-7fa1-8fa9-2047ae76d95f"),
        &directory.path().join("source.sqlite"),
    )
    .unwrap();
    for attempt in ["a", "b"] {
        user(&store, &format!("input-{attempt}"));
        append(
            &store,
            attempt,
            RuntimeEvent::AttemptStarted {
                attempt_id: AttemptId::new(attempt),
            },
        );
        request(&store, attempt, 0, Some(usage(Some(80))));
        assistant(&store, attempt, &format!("response-{attempt}"));
        finish(&store, attempt);
    }
    let original = page(&store, None, 64);
    let original_b = tails(&original)[1].clone();
    let original_turns = store.conversation_turns(0, 64).unwrap();
    // As a Harness fork folds its copied prefix, every generation reads the
    // origin's totals and context, owning neither its execution nor its clock.
    let inherited_totals = ConversationStatistics {
        latest_turn: None,
        ..original.statistics.clone().unwrap()
    };
    let origin_occupancy =
        crate::context::occupancy::read(&store, store.presentation_frontier().unwrap()).unwrap();
    assert!(origin_occupancy.is_some());
    for generation in 1..=3 {
        let id = ConversationId::generate();
        let path = directory.path().join(format!("child-{generation}.sqlite"));
        let canonical = store.load_canonical().unwrap();
        let history = store
            .load_surface_history(store.load_head().unwrap().revision)
            .unwrap();
        let provenance = store
            .read_lineage_cut(store.load_head().unwrap().revision)
            .map(|cut| cut.completed_responses)
            .unwrap();
        let seed = remap_seed(
            &id,
            &canonical,
            &history,
            &provenance,
            &store
                .read_lineage_cut(store.load_head().unwrap().revision)
                .map(|cut| cut.turns)
                .unwrap(),
        )
        .unwrap();
        let child = SqliteConversationStore::open(id.clone(), &path).unwrap();
        child.initialize_lineage(&seed).unwrap();
        assert_eq!(child.load_canonical().unwrap().len(), 4);
        assert!(
            child
                .load_canonical()
                .unwrap()
                .iter()
                .zip(&canonical)
                .all(|(a, b)| message_id_of(a) != message_id_of(b))
        );
        let outline = child.conversation_turns(0, 64).unwrap();
        assert_eq!(
            outline
                .turns
                .iter()
                .map(|turn| &turn.id)
                .collect::<Vec<_>>(),
            original_turns
                .turns
                .iter()
                .map(|turn| &turn.id)
                .collect::<Vec<_>>()
        );
        let location = child
            .conversation_window(
                &crate::durable::reading::ConversationWindowAt::Older {
                    cut: None,
                    before: crate::durable::TranscriptCursor::new(
                        outline.turns[1].cursor.unwrap().get() + 1,
                    ),
                },
                1,
            )
            .unwrap();
        assert_eq!(
            location.page.entries[0].cursor,
            outline.turns[1].cursor.unwrap()
        );
        assert!(
            matches!(&location.page.entries[0].item, crate::durable::TranscriptItem::Message { message } if message_id_of(message) == message_id_of(&seed.canonical()[3]))
        );
        let projected = page(&child, None, 64);
        let inherited = tails(&projected);
        assert_eq!(inherited.len(), 2);
        let b = inherited[1];
        assert_eq!(b.closing_message_id, message_id_of(&seed.canonical()[3]));
        assert_eq!(
            b.retry_message_id,
            Some(message_id_of(&seed.canonical()[2]))
        );
        assert_eq!(b.origin, original_b.origin);
        assert_eq!(b.completed_at, original_b.completed_at);
        assert_eq!(b.usage, original_b.usage);
        assert_eq!(b.timing, original_b.timing);
        assert_eq!(b.timing.as_ref().unwrap().generation_ms, Some(1280));
        assert!(is_completed_response(&child, &b.closing_message_id).unwrap());
        assert_eq!(projected.statistics, Some(inherited_totals.clone()));
        assert_eq!(child.presentation_frontier().unwrap(), 0);
        assert!(child.read_events(None, 128).unwrap().events.is_empty());
        assert_eq!(
            crate::context::occupancy::read(&child, 0).unwrap(),
            origin_occupancy
        );
        let paged = page(&child, None, 1);
        assert_eq!(tails(&paged), vec![b]);
        drop(child);
        store = SqliteConversationStore::open(id, &path).unwrap();
        // Production reopen supplies canonical bootstrap history, never execution residue.
        store
            .initialize(&store.load_bootstrap_history().unwrap())
            .unwrap();
        assert_eq!(page(&store, None, 64), projected);
        let mut changed = seed.completed_responses().to_vec();
        changed[1].usage = None;
        let changed =
            LineageSeed::replayed(seed.canonical().to_vec(), seed.surface_history().to_vec())
                .unwrap()
                .with_completed_responses(changed)
                .unwrap();
        assert!(matches!(
            store.initialize_lineage(&changed),
            Err(ConversationStoreError::InitialHistoryMismatch)
        ));
    }
    // Local execution adds to the inherited totals; its own request replaces
    // the inherited context reading.
    user(&store, "local-user");
    append(
        &store,
        "local",
        RuntimeEvent::AttemptStarted {
            attempt_id: AttemptId::new("local"),
        },
    );
    request(&store, "local", 0, Some(usage(None)));
    assistant(&store, "local", "local-response");
    finish(&store, "local");
    let outline = store.conversation_turns(0, 64).unwrap();
    assert_eq!(outline.total, 3);
    assert_eq!(
        outline.turns[2].id.conversation_id,
        *store.conversation_id()
    );
    assert_eq!(outline.turns[2].id.attempt_id, AttemptId::new("local"));
    let current = page(&store, None, 64);
    assert_eq!(tails(&current).len(), 3);
    assert_eq!(
        tails(&current)[2].timing.as_ref().unwrap().generation_ms,
        Some(1280)
    );
    let totals = current.statistics.unwrap();
    assert_eq!(totals.turns, 3);
    assert_eq!(totals.completed_responses, 3);
    assert_eq!(totals.model_requests, 3);
    assert_eq!(totals.reported_usage.unwrap().total_tokens, 360);
    assert_eq!(
        totals.latest_turn.unwrap().attempt_id,
        AttemptId::new("local")
    );
}

#[test]
fn remapping_drops_missing_retry_input_and_bootstrap_rejects_invalid_response_addresses() {
    let source = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    user(&source, "input");
    request(&source, "attempt", 0, Some(usage(None)));
    assistant(&source, "attempt", "response");
    finish(&source, "attempt");
    let canonical = source.load_canonical().unwrap();
    let provenance = source
        .read_lineage_cut(source.load_head().unwrap().revision)
        .map(|cut| cut.completed_responses)
        .unwrap();
    let id = ConversationId::generate();
    let seed = remap_seed(
        &id,
        &canonical[1..],
        &[SurfaceOp::Append {
            message_id: MessageId::new("response"),
        }],
        &provenance,
        &source
            .read_lineage_cut(source.load_head().unwrap().revision)
            .map(|cut| cut.turns)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(seed.completed_responses()[0].retry_message_id, None);
    let child = SqliteConversationStore::in_memory(id).unwrap();
    child.initialize_lineage(&seed).unwrap();
    assert_eq!(tails(&page(&child, None, 64))[0].retry_message_id, None);
    let invalid =
        LineageSeed::history(canonical[..1].to_vec()).with_completed_responses(provenance.clone());
    assert!(invalid.is_err());
    let invalid = LineageSeed::history(canonical.clone())
        .with_completed_responses(vec![provenance[0].clone(), provenance[0].clone()]);
    assert!(invalid.is_err());
    let mut wrong_retry = provenance;
    wrong_retry[0].retry_message_id = Some(MessageId::new("response"));
    assert!(
        LineageSeed::history(canonical)
            .with_completed_responses(wrong_retry)
            .is_err()
    );
}

#[test]
fn interrupted_process_content_retains_native_origin_outcome_and_destination_location() {
    let source = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    user(&source, "input");
    append(
        &source,
        "stopped",
        RuntimeEvent::AttemptStarted {
            attempt_id: AttemptId::new("stopped"),
        },
    );
    process_content(&source, "stopped");
    append(
        &source,
        "stopped",
        RuntimeEvent::AttemptCancelled {
            attempt_id: AttemptId::new("stopped"),
            reason: crate::runtime::types::CancellationReason::UserRequested,
        },
    );
    let original = page(&source, None, 64);
    assert_eq!(
        original
            .entries
            .iter()
            .filter(|entry| entry.turn_process.is_some())
            .count(),
        4
    );
    let canonical = source.load_canonical().unwrap();
    let provenance = source
        .read_lineage_cut(source.load_head().unwrap().revision)
        .map(|cut| cut.completed_responses)
        .unwrap();
    assert!(provenance.is_empty());
    let history = source
        .load_surface_history(source.load_head().unwrap().revision)
        .unwrap();
    let child_id = ConversationId::generate();
    let seed = remap_seed(
        &child_id,
        &canonical,
        &history,
        &provenance,
        &source
            .read_lineage_cut(source.load_head().unwrap().revision)
            .map(|cut| cut.turns)
            .unwrap(),
    )
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("child.sqlite");
    let child = SqliteConversationStore::open(child_id.clone(), &path).unwrap();
    child.initialize_lineage(&seed).unwrap();
    drop(child);
    let child = SqliteConversationStore::open_existing(child_id, &path).unwrap();
    let projected = page(&child, None, 64);
    assert_eq!(projected.entries.len(), canonical.len());
    assert!(
        projected
            .entries
            .iter()
            .filter_map(|entry| entry.turn_process.as_ref())
            .all(|turn| turn.conversation_id == *source.conversation_id()
                && turn.attempt_id == AttemptId::new("stopped")
                && turn.outcome == TurnProcessOutcome::Cancelled)
    );
    // The cancelled turn's own recorded execution travels with it.
    assert_eq!(
        projected.statistics,
        Some(ConversationStatistics {
            latest_turn: None,
            ..original.statistics.clone().unwrap()
        })
    );
    let outline = child.conversation_turns(0, 64).unwrap();
    assert_eq!(outline.turns.len(), 1);
    assert_eq!(
        outline.turns[0].id.conversation_id,
        *source.conversation_id()
    );
    assert_eq!(outline.turns[0].id.attempt_id, AttemptId::new("stopped"));
    assert_eq!(outline.turns[0].cursor.unwrap().get(), 2);
    assert_eq!(
        projected.entries[1]
            .turn_process
            .as_ref()
            .unwrap()
            .control_cursor
            .get(),
        2
    );
    assert_eq!(source.load_canonical().unwrap(), canonical);
    assert_eq!(page(&source, None, 64), original);
}

#[test]
#[allow(clippy::too_many_lines)] // One ordered lineage contract across three generations and local admission.
fn lineage_preserves_success_failed_and_terminal_only_turns_in_native_order() {
    let source = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    user(&source, "input");
    append(
        &source,
        "success",
        RuntimeEvent::AttemptStarted {
            attempt_id: AttemptId::new("success"),
        },
    );
    assistant(&source, "success", "success-output");
    finish(&source, "success");
    for (name, terminal, _) in terminal_cases() {
        append(
            &source,
            name,
            RuntimeEvent::AttemptStarted {
                attempt_id: AttemptId::new(name),
            },
        );
        if name == "failed" {
            assistant(&source, name, "failed-output");
        }
        append(&source, name, terminal);
    }
    source
        .append_canonical(&MessageBlock::Assistant(AssistantMessageBlock {
            id: MessageId::new("unowned"),
            content: vec![AssistantContentBlock::Text(TextBlock {
                text: "genuinely unowned content".into(),
            })],
        }))
        .unwrap();
    let origins = source
        .conversation_turns(0, 64)
        .unwrap()
        .turns
        .into_iter()
        .map(|turn| turn.id)
        .collect::<Vec<_>>();
    assert_eq!(
        origins
            .iter()
            .map(|id| id.attempt_id.as_str())
            .collect::<Vec<_>>(),
        ["success", "cancelled", "failed", "timeout", "limit"]
    );
    let directory = tempfile::tempdir().unwrap();
    let mut store = source;
    for generation in 0..3 {
        let canonical = store.load_canonical().unwrap();
        let history = store
            .load_surface_history(store.load_head().unwrap().revision)
            .unwrap();
        let seed = remap_seed(
            &ConversationId::generate(),
            &canonical,
            &history,
            &store
                .read_lineage_cut(store.load_head().unwrap().revision)
                .map(|cut| cut.completed_responses)
                .unwrap(),
            &store
                .read_lineage_cut(store.load_head().unwrap().revision)
                .map(|cut| cut.turns)
                .unwrap(),
        )
        .unwrap();
        let id = ConversationId::generate();
        let path = directory
            .path()
            .join(format!("generation-{generation}.sqlite"));
        let child = SqliteConversationStore::open(id.clone(), &path).unwrap();
        child.initialize_lineage(&seed).unwrap();
        let json = serde_json::to_string(seed.turns()).unwrap();
        assert_eq!(
            serde_json::from_str::<Vec<crate::durable::reading::TurnReadingProvenance>>(&json)
                .unwrap(),
            seed.turns()
        );
        let outline = child.conversation_turns(0, 64).unwrap();
        assert_eq!(
            outline
                .turns
                .iter()
                .map(|turn| turn.id.clone())
                .collect::<Vec<_>>(),
            origins
        );
        assert_eq!(
            outline
                .turns
                .iter()
                .map(|turn| turn.ordinal)
                .collect::<Vec<_>>(),
            [1, 2, 3, 4, 5]
        );
        assert_eq!(
            outline
                .turns
                .iter()
                .map(|turn| turn.cursor.unwrap().get())
                .collect::<Vec<_>>(),
            [2, 3, 4, 5, 6]
        );
        let outcomes = [
            TurnProcessOutcome::Completed,
            TurnProcessOutcome::Cancelled,
            TurnProcessOutcome::Failed,
            TurnProcessOutcome::TimedOut,
            TurnProcessOutcome::LimitExceeded,
        ];
        for (index, turn) in outline.turns.iter().enumerate() {
            let read = child
                .conversation_window(
                    &crate::durable::reading::ConversationWindowAt::Older {
                        cut: None,
                        before: crate::durable::TranscriptCursor::new(
                            turn.cursor.unwrap().get() + 1,
                        ),
                    },
                    1,
                )
                .unwrap();
            assert_eq!(read.page.entries[0].cursor, turn.cursor.unwrap());
            let mut projected = transcript_page_view(read.page).unwrap();
            decorate_window(&child, &mut projected, read.cut.journal).unwrap();
            let owner = projected.entries[0].turn_process.as_ref().unwrap();
            assert_eq!(owner.conversation_id, turn.id.conversation_id);
            assert_eq!(owner.attempt_id, turn.id.attempt_id);
            assert_eq!(owner.outcome, outcomes[index]);
            assert_eq!(owner.control_cursor.get(), turn.cursor.unwrap().get());
        }
        assert_eq!(child.presentation_frontier().unwrap(), 0);
        assert_eq!(child.read_events(None, 64).unwrap().events, []);
        let bootstrap = child.load_transcript_page(None, 64).unwrap();
        let snapshot = child
            .load_transcript_snapshot(&crate::durable::inbox::TranscriptSnapshotCut {
                bootstrap_through: bootstrap.entries.last().unwrap().cursor.get(),
                journal_through: 0,
                messages: Vec::new(),
                publications: Vec::new(),
                pending: Vec::new(),
            })
            .unwrap();
        assert_eq!(
            snapshot, bootstrap,
            "terminal-only lineage survives live snapshot capture without Journal facts"
        );
        drop(child);
        store = SqliteConversationStore::open(id, &path).unwrap();
        store
            .initialize(&store.load_bootstrap_history().unwrap())
            .unwrap();
    }
    append(
        &store,
        "local",
        RuntimeEvent::AttemptStarted {
            attempt_id: AttemptId::new("local"),
        },
    );
    assistant(&store, "local", "local-output");
    finish(&store, "local");
    let outline = store.conversation_turns(0, 64).unwrap();
    assert_eq!(
        outline.turns[..5]
            .iter()
            .map(|turn| turn.id.clone())
            .collect::<Vec<_>>(),
        origins
    );
    assert_eq!(outline.turns[5].ordinal, 6);
    assert_eq!(
        outline.turns[5].id.conversation_id,
        *store.conversation_id()
    );
    assert_eq!(outline.turns[5].id.attempt_id, AttemptId::new("local"));
}
