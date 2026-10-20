//! Exact lineage read cuts: checkpoint activation, response/retry boundaries,
//! temporal cut races, and unsuccessful turn provenance.

use super::super::{LineageSide, lineage_cut};
use super::*;

#[test]
fn historical_fork_excludes_checkpoint_activated_after_selected_ledger() {
    use crate::context::resources::LedgerFrontier;
    use crate::scripted_suites::support::context::{checkpoint_commit_fixture, checkpoint_fixture};
    let source = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    source
        .initialize(
            &(1..=100)
                .map(|n| user(&format!("M{n}"), "history"))
                .collect::<Vec<_>>(),
        )
        .unwrap();
    source
        .commit_projection(checkpoint_commit_fixture(
            &source,
            checkpoint_fixture(&source, "P1", 95, "early"),
            None,
        ))
        .unwrap();
    let pending = checkpoint_fixture(&source, "P2", 1, "future");
    assert_eq!(pending.base_frontier, LedgerFrontier(100));
    for n in 101..=110 {
        source
            .append_canonical(&user(&format!("M{n}"), "tail"))
            .unwrap();
    }
    source
        .commit_projection(checkpoint_commit_fixture(&source, pending, None))
        .unwrap();
    source
        .append_canonical(&user("M111", "after activation"))
        .unwrap();
    let cut = source
        .read_lineage_cut(source.load_head().unwrap().revision)
        .unwrap();
    for (boundary, side, expected) in [
        (105, LineageSide::Before, 1),
        (110, LineageSide::Before, 1),
        (110, LineageSide::After, 2),
        (111, LineageSide::After, 2),
    ] {
        let id = ConversationId::generate();
        let seed = lineage_cut(
            &id,
            &cut,
            Some(&MessageId::new(format!("M{boundary}"))),
            side,
        )
        .unwrap();
        let destination = SqliteConversationStore::in_memory(id).unwrap();
        destination.initialize_lineage(&seed).unwrap();
        assert_eq!(
            destination
                .projection_head()
                .unwrap()
                .unwrap()
                .activation
                .activation
                .0,
            expected,
            "boundary {boundary} {side:?}"
        );
        destination.projected_history().unwrap();
    }
}

/// Each same-ledger activation is a distinct historical fork boundary.
/// Persistence and the product's lineage remapper are exercised at every copy.
#[test]
fn projection_activation_closure_survives_clone_fork_clone_again_and_source_deletion() {
    use crate::context::checkpoint::{CheckpointMessage, ProjectionActivationId};
    use crate::context::resources::LedgerFrontier;
    use crate::model::projected::ProjectedMessage;
    use crate::scripted_suites::support::context::{checkpoint_commit_fixture, checkpoint_fixture};
    let directory = tempfile::tempdir().unwrap();
    let source_path = directory.path().join("source.sqlite");
    let source_id = ConversationId::generate();
    let source = SqliteConversationStore::open(source_id.clone(), &source_path).unwrap();
    source
        .initialize(
            &(1..=100)
                .map(|n| user(&format!("M{n}"), &format!("message {n}")))
                .collect::<Vec<_>>(),
        )
        .unwrap();
    let first = source
        .commit_projection(checkpoint_commit_fixture(
            &source,
            checkpoint_fixture(&source, "P1", 95, "P1 semantics"),
            None,
        ))
        .unwrap();
    let second = source
        .commit_projection(checkpoint_commit_fixture(
            &source,
            checkpoint_fixture(&source, "P2", 1, "P2 semantics"),
            None,
        ))
        .unwrap();
    assert_eq!(first.checkpoint.activation.ledger, LedgerFrontier(100));
    assert_eq!(second.checkpoint.activation.ledger, LedgerFrontier(100));
    let source_cut = source
        .read_projection_lineage_cut(LedgerFrontier(100), ProjectionActivationId(2))
        .unwrap();
    let mut copied_id = ConversationId::generate();
    let first_seed = lineage_cut(&copied_id, &source_cut, None, LineageSide::After).unwrap();
    let mut copied = SqliteConversationStore::in_memory(copied_id.clone()).unwrap();
    copied.initialize_lineage(&first_seed).unwrap();
    drop(source);
    std::fs::remove_file(&source_path).unwrap();
    for generation in 0..3 {
        assert!(
            copied.read_events(None, 64).unwrap().events.is_empty(),
            "inherited checkpoints fabricate no destination CompactionCompleted"
        );
        let timeline = copied
            .projection_timeline(ProjectionActivationId(0), 64)
            .unwrap();
        assert_eq!(timeline.len(), 2);
        for (activation, label) in [(1, "P1 semantics"), (2, "P2 semantics")] {
            assert_eq!(
                timeline[usize::try_from(activation - 1).unwrap()]
                    .activation
                    .origin
                    .conversation_id,
                source_id
            );
            let cut = copied
                .read_projection_lineage_cut(
                    LedgerFrontier(100),
                    ProjectionActivationId(activation),
                )
                .unwrap();
            assert_eq!(cut.projections.len(), usize::try_from(activation).unwrap());
            let child_id = ConversationId::generate();
            let seed = lineage_cut(&child_id, &cut, None, LineageSide::After).unwrap();
            let fork = SqliteConversationStore::in_memory(child_id).unwrap();
            fork.initialize_lineage(&seed).unwrap();
            let head = fork.projection_head().unwrap().unwrap();
            let CheckpointMessage::Derived(summary) = &head.candidate.items[0].message else {
                panic!("inherited Derived Summary")
            };
            let ProjectedMessage::User { content, .. } = &summary.message else {
                panic!("summary role")
            };
            assert!(
                matches!(&content[0],UserContentBlock::Text(text) if text.text.contains(label)),
                "copy {generation} activation {activation}"
            );
            assert_eq!(head.activation.origin.conversation_id, source_id);
            assert!(head.activation.destination_installation.is_some());
            assert_eq!(fork.load_canonical().unwrap().len(), 100);
            assert!(fork.read_events(None, 64).unwrap().events.is_empty());
            fork.projected_history().unwrap();
        }
        let cut = copied
            .read_projection_lineage_cut(LedgerFrontier(100), ProjectionActivationId(2))
            .unwrap();
        copied_id = ConversationId::generate();
        let seed = lineage_cut(&copied_id, &cut, None, LineageSide::After).unwrap();
        let next = SqliteConversationStore::in_memory(copied_id.clone()).unwrap();
        next.initialize_lineage(&seed).unwrap();
        copied = next;
    }
}

#[test]
fn post_response_fork_and_branch_include_the_response_and_retry_excludes_input() {
    let (_directory, catalog, _config) = open_catalog();
    let assistant = |id: &str| {
        MessageBlock::Assistant(AssistantMessageBlock {
            id: MessageId::new(id),
            content: vec![AssistantContentBlock::Text(TextBlock { text: id.into() })],
        })
    };
    let history = vec![
        user("user-a", "A"),
        assistant("assistant-a"),
        user("user-b", "B"),
        assistant("assistant-b"),
        user("later", "later"),
    ];
    let (conversation, session, _) = append_history(&catalog, &history);
    let store = store_for(&catalog, &session, &conversation);
    let source = lineage_at(&store, &conversation, store.load_head().unwrap().revision);
    let (fork, fork_editor) = catalog
        .prepare_fork_session(
            &state(),
            &source,
            &MessageId::new("assistant-b"),
            LineageSide::After,
        )
        .unwrap();
    let (branch, branch_editor) = catalog
        .prepare_tree_node(
            &session,
            &state(),
            &source,
            &MessageId::new("assistant-b"),
            LineageSide::After,
        )
        .unwrap();
    assert!(fork_editor.is_empty());
    assert!(branch_editor.is_empty());
    assert_ne!(fork.session_id, session);
    assert_eq!(branch.session_id, session);
    for prepared in [&fork, &branch] {
        let copied = SqliteConversationStore::open_existing(
            prepared.conversation_id.clone(),
            &prepared.database_path,
        )
        .unwrap();
        let messages = copied.load_canonical().unwrap();
        assert_eq!(messages.len(), 4);
        let MessageBlock::Assistant(last) = &messages[3] else {
            panic!("Assistant B must be included")
        };
        assert_eq!(
            last.content,
            match &history[3] {
                MessageBlock::Assistant(a) => a.content.clone(),
                _ => unreachable!(),
            }
        );
    }
    let (retry, editor) = catalog
        .prepare_tree_node(
            &session,
            &state(),
            &source,
            &MessageId::new("user-b"),
            LineageSide::Before,
        )
        .unwrap();
    assert_eq!(editor, vec![text("B")]);
    let retried =
        SqliteConversationStore::open_existing(retry.conversation_id.clone(), &retry.database_path)
            .unwrap();
    assert_eq!(retried.load_canonical().unwrap().len(), 2);
    assert_eq!(store.load_canonical().unwrap(), history);
}

#[test]
fn running_lineage_cut_retains_incomplete_origin_without_execution() {
    lineage_copy_race(true, false);
}

#[test]
fn terminal_only_publication_loses_frozen_lineage_copy_race() {
    lineage_copy_race(false, false);
}

#[test]
fn terminal_only_publication_wins_frozen_lineage_copy_race() {
    lineage_copy_race(false, true);
}

#[test]
fn same_surface_revision_newer_native_cut_changes_inherited_running_outcome() {
    same_revision_different_temporal_cuts(true);
}

/// A copied turn keeps its opening human prompt under the destination's
/// own message identity, so the destination outline previews it unchanged.
#[test]
fn copied_turn_keeps_its_prompt_under_destination_identity() {
    use crate::durable::inbox::InboundDraft;
    use crate::events::types::{RuntimeEvent, RuntimeEventEnvelope};
    use crate::message::types::{InboundKind, UserContentBlock, UserSource};
    use crate::runtime::identity::{AttemptId, EventId};
    let (_directory, catalog, _config) = open_catalog();
    let (conversation, session, _) = append_history(&catalog, &[]);
    let store = store_for(&catalog, &session, &conversation);
    let attempt = AttemptId::new("prompted-attempt");
    let event = |kind| RuntimeEventEnvelope {
        tool_occurrence: None,
        observation: None,
        schema_version: 1,
        event_id: EventId::new(format!(
            "prompted-event-{}",
            store.presentation_frontier().unwrap() + 1
        )),
        sequence: 0,
        conversation_id: conversation.clone(),
        attempt_id: Some(attempt.clone()),
        turn_id: None,
        timestamp: chrono::Utc::now(),
        event: kind,
    };
    let accepted = store
        .accept_inbound(InboundDraft {
            message_id: None,
            source: UserSource::Human,
            kind: InboundKind::Message,
            content: vec![UserContentBlock::Text(TextBlock {
                text: "Explain the rail".into(),
            })],
            timestamp: chrono::Utc::now(),
            correlation: None,
        })
        .unwrap();
    store.adopt_pending_batch(accepted.sequence, None).unwrap();
    store
        .append_event(event(RuntimeEvent::AttemptStarted {
            attempt_id: attempt.clone(),
        }))
        .unwrap();
    store
        .append_canonical_with_event(
            &MessageBlock::Assistant(AssistantMessageBlock {
                id: MessageId::new("rail-answer"),
                content: vec![AssistantContentBlock::Text(TextBlock {
                    text: "It marks every turn".into(),
                })],
            }),
            event(RuntimeEvent::AssistantMessageCommitted {
                message_id: MessageId::new("rail-answer"),
            }),
        )
        .unwrap();
    store
        .append_event(event(RuntimeEvent::AttemptTimedOut {
            attempt_id: attempt.clone(),
        }))
        .unwrap();
    let cut = store
        .read_lineage_cut(store.load_head().unwrap().revision)
        .unwrap();
    assert_eq!(
        cut.turns[0].prompt_message_id,
        Some(accepted.message_id.clone())
    );
    let copy = catalog.prepare_clone_session(&state(), &cut).unwrap();
    let destination =
        SqliteConversationStore::open_existing(copy.conversation_id.clone(), &copy.database_path)
            .unwrap();
    let inherited = destination
        .read_lineage_cut(destination.load_head().unwrap().revision)
        .unwrap();
    let prompt = inherited.turns[0]
        .prompt_message_id
        .clone()
        .expect("copied prompt");
    assert_ne!(prompt, accepted.message_id, "destination identity");
    assert!(
        inherited
            .canonical
            .iter()
            .any(|message| matches!(message, MessageBlock::User(message) if message.id == prompt))
    );
    let outline = destination.conversation_turns(0, 64).unwrap();
    assert_eq!(
        (
            outline.turns[0].prompt.as_str(),
            outline.turns[0].response.as_str()
        ),
        ("Explain the rail", "It marks every turn")
    );
}

#[test]
fn same_surface_revision_newer_native_cut_admits_terminal_only_turn() {
    same_revision_different_temporal_cuts(false);
}

#[allow(clippy::too_many_lines)] // One R, two ordered native cuts and immutable reopened/repeated copies.
fn same_revision_different_temporal_cuts(member: bool) {
    use crate::durable::reading::{ConversationTurnId, InheritedTurnOutcome, TurnProcessOutcome};
    use crate::events::types::{RuntimeEvent, RuntimeEventEnvelope};
    use crate::runtime::identity::{AttemptId, EventId};
    let (_directory, catalog, _config) = open_catalog();
    let (conversation, session, _) = append_history(&catalog, &[]);
    let store = store_for(&catalog, &session, &conversation);
    let origin = ConversationTurnId {
        conversation_id: conversation.clone(),
        attempt_id: AttemptId::new("same-r-attempt"),
    };
    let event = |kind| RuntimeEventEnvelope {
        tool_occurrence: None,
        observation: None,
        schema_version: 1,
        event_id: EventId::new(format!(
            "same-r-event-{}",
            store.presentation_frontier().unwrap() + 1
        )),
        sequence: 0,
        conversation_id: conversation.clone(),
        attempt_id: Some(origin.attempt_id.clone()),
        turn_id: None,
        timestamp: chrono::Utc::now(),
        event: kind,
    };
    store.append_canonical(&user("input", "input")).unwrap();
    store
        .append_event(event(RuntimeEvent::AttemptStarted {
            attempt_id: origin.attempt_id.clone(),
        }))
        .unwrap();
    if member {
        store
            .append_canonical_with_event(
                &MessageBlock::Assistant(AssistantMessageBlock {
                    id: MessageId::new("retained"),
                    content: vec![AssistantContentBlock::Text(TextBlock {
                        text: "retained output at R".into(),
                    })],
                }),
                event(RuntimeEvent::AssistantMessageCommitted {
                    message_id: MessageId::new("retained"),
                }),
            )
            .unwrap();
    }
    let revision = store.load_head().unwrap().revision;
    let c1 = store.read_lineage_cut(revision).unwrap();
    assert_eq!(c1.read_cut.journal, if member { 2 } else { 1 });
    if member {
        assert_eq!(c1.turns[0].id, origin);
        assert_eq!(c1.turns[0].outcome, InheritedTurnOutcome::IncompleteAtCut);
        assert!(c1.turns[0].ended_at.is_none());
    } else {
        assert!(c1.turns.is_empty(), "a start alone is not inherited output");
    }
    // Destination one exists before settlement. No scheduler or elapsed time
    // determines the interleaving: each native commit/read returns in order.
    let first = catalog.prepare_clone_session(&state(), &c1).unwrap();
    let first_store =
        SqliteConversationStore::open_existing(first.conversation_id.clone(), &first.database_path)
            .unwrap();
    let first_frozen = first_store
        .read_lineage_cut(first_store.load_head().unwrap().revision)
        .unwrap();
    drop(first_store);
    let terminal = store
        .append_event(event(RuntimeEvent::AttemptCancelled {
            attempt_id: origin.attempt_id.clone(),
            reason: crate::runtime::types::CancellationReason::UserRequested,
        }))
        .unwrap();
    assert!(terminal.sequence > c1.read_cut.journal);
    assert_eq!(store.load_head().unwrap().revision, revision);
    let c2 = store.read_lineage_cut(revision).unwrap();
    assert_eq!(c2.surface_revision, c1.surface_revision, "same exact R");
    assert_eq!(c2.surface_history, c1.surface_history);
    assert_eq!(c2.canonical, c1.canonical);
    assert_eq!(c2.messages, c1.messages);
    assert_eq!(c2.read_cut.journal, terminal.sequence);
    assert!(c2.read_cut.journal > c1.read_cut.journal, "different C");
    assert_eq!(c2.turns.len(), 1);
    assert_eq!(c2.turns[0].id, origin);
    assert_eq!(c2.turns[0].outcome, InheritedTurnOutcome::Cancelled);
    assert_eq!(c2.turns[0].ended_at, Some(terminal.timestamp));
    assert_ne!(c1.turns, c2.turns, "different temporal meaning by contract");
    let second = catalog.prepare_clone_session(&state(), &c2).unwrap();
    let after_terminal = store.conversation_read_cut().unwrap();
    let source_events = store.read_events(None, 64).unwrap().events;
    assert!(
        store
            .read_request_snapshots(None, 64)
            .unwrap()
            .snapshots
            .is_empty()
    );
    for (index, prepared) in [first, second].into_iter().enumerate() {
        let destination = SqliteConversationStore::open_existing(
            prepared.conversation_id.clone(),
            &prepared.database_path,
        )
        .unwrap();
        let cut = destination
            .read_lineage_cut(destination.load_head().unwrap().revision)
            .unwrap();
        if index == 0 {
            assert_eq!(
                cut, first_frozen,
                "source settlement did not mutate C1's copy"
            );
        }
        let retained = member || index == 1;
        let expected = if index == 0 {
            InheritedTurnOutcome::IncompleteAtCut
        } else {
            InheritedTurnOutcome::Cancelled
        };
        assert_eq!(cut.turns.len(), usize::from(retained));
        assert_eq!(cut.canonical.len(), if member { 2 } else { 1 });
        let predecessor =
            (index == 1).then(|| crate::conversation::message_id_of(cut.canonical.last().unwrap()));
        if retained {
            let turn = &cut.turns[0];
            assert_eq!(turn.id, origin);
            assert_eq!(turn.outcome, expected);
            assert_eq!(turn.preceding_message_id, predecessor);
            assert_eq!(
                turn.process_message_ids,
                if member {
                    vec![crate::conversation::message_id_of(&cut.canonical[1])]
                } else {
                    vec![]
                }
            );
            assert_eq!(turn.ended_at, (index == 1).then_some(terminal.timestamp));
        }
        // Repeated lineage must retain the outcome captured by each C,
        // rather than consulting the now-terminal original Conversation.
        let repeated = catalog.prepare_clone_session(&state(), &cut).unwrap();
        let repeated_store = SqliteConversationStore::open_existing(
            repeated.conversation_id,
            &repeated.database_path,
        )
        .unwrap();
        for copied_store in [&destination, &repeated_store] {
            assert_eq!(copied_store.presentation_frontier().unwrap(), 0);
            assert!(
                copied_store
                    .read_events(None, 64)
                    .unwrap()
                    .events
                    .is_empty()
            );
            assert!(
                copied_store
                    .read_request_snapshots(None, 64)
                    .unwrap()
                    .snapshots
                    .is_empty()
            );
            let outline = copied_store.conversation_turns(0, 64).unwrap();
            assert_eq!(
                outline
                    .turns
                    .iter()
                    .map(|turn| (&turn.id, turn.ordinal, turn.cursor.unwrap().get()))
                    .collect::<Vec<_>>(),
                if retained {
                    vec![(&origin, 1, 2)]
                } else {
                    vec![]
                },
                "equivalent destination position: member or inherited spine after U"
            );
            if retained {
                let window = copied_store
                    .conversation_window(
                        &crate::durable::reading::ConversationWindowAt::Older {
                            cut: None,
                            before: crate::durable::TranscriptCursor::new(3),
                        },
                        1,
                    )
                    .unwrap();
                assert_eq!(window.page.entries[0].cursor.get(), 2);
                let mut page =
                    crate::observation::transcript::transcript_page_view(window.page).unwrap();
                crate::runtime_client::response::native_oracle_decorate(copied_store, &mut page)
                    .unwrap();
                let process = page.entries[0].turn_process.as_ref().unwrap();
                assert_eq!(process.outcome, TurnProcessOutcome::from(expected.clone()));
                assert!(process.event_id.is_none(), "no destination execution owner");
                assert_eq!(process.ended_at, (index == 1).then_some(terminal.timestamp));
            }
        }
    }
    assert_eq!(store.conversation_read_cut().unwrap(), after_terminal);
    assert_eq!(store.read_events(None, 64).unwrap().events, source_events);
    assert!(
        store
            .read_request_snapshots(None, 64)
            .unwrap()
            .snapshots
            .is_empty(),
        "zero model/Tool work"
    );
}

#[allow(clippy::too_many_lines)] // Exact cut gate, all three Session copies and reopen/repeated lineage.
fn lineage_copy_race(member: bool, terminal_wins: bool) {
    use crate::durable::reading::{InheritedTurnOutcome, TurnProcessOutcome};
    use crate::events::types::{RuntimeEvent, RuntimeEventEnvelope};
    use crate::runtime::identity::{AttemptId, EventId};
    let (_directory, catalog, _config) = open_catalog();
    let (conversation, session, _) = append_history(&catalog, &[]);
    let store = store_for(&catalog, &session, &conversation);
    let append = |kind| RuntimeEventEnvelope {
        tool_occurrence: None,
        observation: None,
        schema_version: 1,
        event_id: EventId::new(format!(
            "cut-event-{}",
            store.presentation_frontier().unwrap() + 1
        )),
        sequence: 0,
        conversation_id: conversation.clone(),
        attempt_id: Some(AttemptId::new("source-attempt")),
        turn_id: None,
        timestamp: chrono::Utc::now(),
        event: kind,
    };
    let terminal = || {
        append(RuntimeEvent::AttemptCancelled {
            attempt_id: AttemptId::new("source-attempt"),
            reason: crate::runtime::types::CancellationReason::UserRequested,
        })
    };
    store.append_canonical(&user("input", "input")).unwrap();
    store
        .append_event(append(RuntimeEvent::AttemptStarted {
            attempt_id: AttemptId::new("source-attempt"),
        }))
        .unwrap();
    if member {
        store
            .append_canonical_with_event(
                &MessageBlock::Assistant(AssistantMessageBlock {
                    id: MessageId::new("retained"),
                    content: vec![AssistantContentBlock::Text(TextBlock {
                        text: "retained running output".into(),
                    })],
                }),
                append(RuntimeEvent::AssistantMessageCommitted {
                    message_id: MessageId::new("retained"),
                }),
            )
            .unwrap();
    }
    if terminal_wins {
        store.append_event(terminal()).unwrap();
    }
    store
        .append_canonical(&user("boundary", "next input"))
        .unwrap();
    let revision = store.load_head().unwrap().revision;
    let (captured_tx, captured_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let worker_catalog = catalog.clone();
    let worker_conversation = conversation.clone();
    let worker_session = session.clone();
    let worker = std::thread::spawn(move || {
        let source_store = store_for(&worker_catalog, &worker_session, &worker_conversation);
        let source = source_store.read_lineage_cut(revision).unwrap();
        captured_tx.send(source.clone()).unwrap();
        release_rx.recv().unwrap(); // Copy is parked AFTER the exact native read linearization point.
        let clone = worker_catalog
            .prepare_clone_session(&state(), &source)
            .unwrap();
        let (fork, _) = worker_catalog
            .prepare_fork_session(
                &state(),
                &source,
                &MessageId::new("boundary"),
                LineageSide::Before,
            )
            .unwrap();
        let (branch, _) = worker_catalog
            .prepare_tree_node(
                &worker_session,
                &state(),
                &source,
                &MessageId::new("boundary"),
                LineageSide::Before,
            )
            .unwrap();
        [clone, fork, branch]
    });
    let frozen = captured_rx.recv().unwrap();
    assert_eq!(frozen.surface_revision, revision);
    assert_eq!(
        frozen.read_cut.journal,
        if member || terminal_wins { 2 } else { 1 }
    );
    if !terminal_wins && !member {
        let published = store.append_event(terminal()).unwrap();
        assert!(published.sequence > frozen.read_cut.journal);
    }
    release_tx.send(()).unwrap();
    let copies = worker.join().unwrap();
    // The member-backed case creates every destination while the source
    // still runs. Later source settlement cannot change any copied meaning.
    if member {
        let published = store.append_event(terminal()).unwrap();
        assert!(published.sequence > frozen.read_cut.journal);
    }
    let after_publication = store.conversation_read_cut().unwrap();
    let retained = member || terminal_wins;
    let expected = if terminal_wins {
        InheritedTurnOutcome::Cancelled
    } else {
        InheritedTurnOutcome::IncompleteAtCut
    };
    assert_eq!(frozen.turns.len(), usize::from(retained));
    if retained {
        assert_eq!(
            frozen.turns[0].id,
            crate::durable::reading::ConversationTurnId {
                conversation_id: conversation.clone(),
                attempt_id: AttemptId::new("source-attempt"),
            }
        );
        assert_eq!(frozen.turns[0].outcome, expected);
    }
    assert_eq!(frozen.completed_responses, []);
    for prepared in copies {
        let destination = SqliteConversationStore::open_existing(
            prepared.conversation_id.clone(),
            &prepared.database_path,
        )
        .unwrap();
        let cut = destination
            .read_lineage_cut(destination.load_head().unwrap().revision)
            .unwrap();
        assert_eq!(cut.turns.len(), usize::from(retained));
        assert_eq!(destination.presentation_frontier().unwrap(), 0);
        assert_eq!(destination.read_events(None, 64).unwrap().events, []);
        assert_eq!(
            destination
                .read_request_snapshots(None, 64)
                .unwrap()
                .snapshots,
            []
        );
        if retained {
            assert_eq!(cut.turns[0].id, frozen.turns[0].id);
            assert_eq!(cut.turns[0].outcome, expected);
            let outline = destination.conversation_turns(0, 64).unwrap();
            assert_eq!(outline.turns[0].ordinal, 1);
            assert_eq!(outline.turns[0].cursor.unwrap().get(), 2);
            let window = destination
                .conversation_window(
                    &crate::durable::reading::ConversationWindowAt::Older {
                        cut: None,
                        before: crate::durable::TranscriptCursor::new(3),
                    },
                    1,
                )
                .unwrap();
            assert_eq!(window.page.entries[0].cursor.get(), 2);
            let mut page =
                crate::observation::transcript::transcript_page_view(window.page).unwrap();
            crate::runtime_client::response::native_oracle_decorate(&destination, &mut page)
                .unwrap();
            let process = page.entries[0].turn_process.as_ref().unwrap();
            assert_eq!(process.outcome, TurnProcessOutcome::from(expected.clone()));
            assert!(process.event_id.is_none());
            assert_ne!(process.outcome, TurnProcessOutcome::Running);
            if member {
                assert!(process.ended_at.is_none());
            }
        }
        drop(destination);
        let reopened = SqliteConversationStore::open_existing(
            prepared.conversation_id.clone(),
            &prepared.database_path,
        )
        .unwrap();
        assert_eq!(
            reopened
                .read_lineage_cut(reopened.load_head().unwrap().revision)
                .unwrap(),
            cut
        );
        let again = catalog.prepare_clone_session(&state(), &cut).unwrap();
        let again =
            SqliteConversationStore::open_existing(again.conversation_id, &again.database_path)
                .unwrap();
        let repeated = again
            .read_lineage_cut(again.load_head().unwrap().revision)
            .unwrap();
        assert_eq!(
            repeated
                .turns
                .iter()
                .map(|turn| (&turn.id, &turn.outcome))
                .collect::<Vec<_>>(),
            cut.turns
                .iter()
                .map(|turn| (&turn.id, &turn.outcome))
                .collect::<Vec<_>>()
        );
        let outline = again.conversation_turns(0, 64).unwrap();
        assert_eq!(
            outline
                .turns
                .iter()
                .map(|turn| (turn.ordinal, turn.cursor.unwrap().get()))
                .collect::<Vec<_>>(),
            if retained { vec![(1, 2)] } else { vec![] }
        );
    }
    assert_eq!(
        store.conversation_read_cut().unwrap(),
        after_publication,
        "copies perform zero source execution/writes"
    );
}

#[test]
#[allow(clippy::too_many_lines)] // One shared source and exact locations across every Session lineage operation.
fn clone_fork_and_branch_share_native_unsuccessful_turn_provenance() {
    use crate::events::types::{RuntimeEvent, RuntimeEventEnvelope};
    use crate::runtime::identity::{AttemptId, EventId};
    let (_directory, catalog, _config) = open_catalog();
    let (conversation, session, _) = append_history(&catalog, &[]);
    let store = store_for(&catalog, &session, &conversation);
    let event = |name: &str, kind| RuntimeEventEnvelope {
        tool_occurrence: None,
        observation: None,
        schema_version: 1,
        event_id: EventId::new(format!(
            "reading-event-{}",
            store.presentation_frontier().unwrap() + 1
        )),
        sequence: 0,
        conversation_id: conversation.clone(),
        attempt_id: Some(AttemptId::new(name)),
        turn_id: None,
        timestamp: chrono::Utc::now(),
        event: kind,
    };
    store.append_canonical(&user("input", "input")).unwrap();
    store
        .append_event(event(
            "interrupted",
            RuntimeEvent::AttemptStarted {
                attempt_id: AttemptId::new("interrupted"),
            },
        ))
        .unwrap();
    store
        .append_canonical_with_event(
            &MessageBlock::Assistant(AssistantMessageBlock {
                id: MessageId::new("partial"),
                content: vec![AssistantContentBlock::Text(TextBlock {
                    text: "partial".into(),
                })],
            }),
            event(
                "interrupted",
                RuntimeEvent::AssistantMessageCommitted {
                    message_id: MessageId::new("partial"),
                },
            ),
        )
        .unwrap();
    store
        .append_event(event(
            "interrupted",
            RuntimeEvent::AttemptCancelled {
                attempt_id: AttemptId::new("interrupted"),
                reason: crate::runtime::types::CancellationReason::UserRequested,
            },
        ))
        .unwrap();
    store
        .append_event(event(
            "empty",
            RuntimeEvent::AttemptStarted {
                attempt_id: AttemptId::new("empty"),
            },
        ))
        .unwrap();
    store
        .append_event(event(
            "empty",
            RuntimeEvent::AttemptTimedOut {
                attempt_id: AttemptId::new("empty"),
            },
        ))
        .unwrap();
    store
        .append_canonical(&user("boundary", "next input"))
        .unwrap();
    let source = lineage_at(&store, &conversation, store.load_head().unwrap().revision);
    let clone = catalog.prepare_clone_session(&state(), &source).unwrap();
    let (fork, _) = catalog
        .prepare_fork_session(
            &state(),
            &source,
            &MessageId::new("boundary"),
            LineageSide::Before,
        )
        .unwrap();
    let (branch, _) = catalog
        .prepare_tree_node(
            &session,
            &state(),
            &source,
            &MessageId::new("boundary"),
            LineageSide::Before,
        )
        .unwrap();
    for prepared in [&clone, &fork, &branch] {
        let copied = SqliteConversationStore::open_existing(
            prepared.conversation_id.clone(),
            &prepared.database_path,
        )
        .unwrap();
        let outline = copied.conversation_turns(0, 64).unwrap();
        assert_eq!(
            outline
                .turns
                .iter()
                .map(|turn| (
                    &turn.id.conversation_id,
                    turn.id.attempt_id.as_str(),
                    turn.ordinal,
                    turn.cursor.unwrap().get()
                ))
                .collect::<Vec<_>>(),
            [
                (&conversation, "interrupted", 1, 2),
                (&conversation, "empty", 2, 3)
            ]
        );
        for turn in &outline.turns {
            let window = copied
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
            assert_eq!(window.page.entries[0].cursor, turn.cursor.unwrap());
        }
        assert_eq!(copied.presentation_frontier().unwrap(), 0);
    }
    let (empty, _) = catalog
        .prepare_fork_session(
            &state(),
            &source,
            &MessageId::new("input"),
            LineageSide::Before,
        )
        .unwrap();
    let empty = SqliteConversationStore::open_existing(empty.conversation_id, &empty.database_path)
        .unwrap();
    assert_eq!(empty.conversation_turns(0, 64).unwrap().turns, []);
}
