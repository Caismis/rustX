//! Artifact copying preserves the same immutable native history as an ordinary copy.
use super::*;
use crate::durable::inbox::{InboundDraft, LineageSeed};
use crate::durable::reading::{ConversationWindowAt, InheritedTurnOutcome};
use crate::events::types::{EVENT_SCHEMA_VERSION, RuntimeEvent, RuntimeEventEnvelope};
use crate::message::content::FileReference;
use crate::model::finish::ModelFinishReason;
use crate::runtime::identity::{ArtifactId, AttemptId, EventId};
use crate::tools::ArtifactStore;

fn complete_turn(store: &SqliteConversationStore, conversation: &ConversationId, index: u32) {
    let attempt = AttemptId::new(format!("native-attempt-{index}"));
    let timestamp = Utc
        .with_ymd_and_hms(2026, 10, 10, 12, 0, index * 3 - 2)
        .unwrap();
    let event = |kind| RuntimeEventEnvelope {
        schema_version: EVENT_SCHEMA_VERSION,
        event_id: EventId::new(format!(
            "event-{}",
            store.presentation_frontier().unwrap() + 1
        )),
        sequence: 0,
        conversation_id: conversation.clone(),
        attempt_id: Some(attempt.clone()),
        turn_id: None,
        timestamp,
        event: kind,
    };
    store
        .append_event(event(RuntimeEvent::AttemptStarted {
            attempt_id: attempt.clone(),
        }))
        .unwrap();
    let id = MessageId::new(format!("answer-{index}"));
    store
        .append_canonical_with_event(
            &MessageBlock::Assistant(AssistantMessageBlock {
                id: id.clone(),
                content: vec![AssistantContentBlock::Text(TextBlock {
                    text: format!("Response {index}"),
                })],
            }),
            event(RuntimeEvent::AssistantMessageCommitted { message_id: id }),
        )
        .unwrap();
    store
        .append_event(event(RuntimeEvent::AttemptCompleted {
            attempt_id: attempt.clone(),
            finish_reason: ModelFinishReason::Stop,
        }))
        .unwrap();
}

fn source(catalog: &SessionCatalog, artifact: bool) -> (SqliteConversationStore, LineageReadCut) {
    let (conversation, session, _) = append_history(catalog, &[]);
    let store = store_for(catalog, &session, &conversation);
    let mut content = vec![text("First prompt")];
    if artifact {
        let path = catalog.database_path(&session, &conversation);
        let artifacts = ArtifactStore::new(conversation.clone(), path.parent().unwrap()).unwrap();
        artifacts.put_bounded(b"unused object").unwrap();
        let id = artifacts
            .put_bounded(b"immutable artifact evidence")
            .unwrap();
        assert_eq!(id, ArtifactId::new("artifact_2"));
        content.push(UserContentBlock::File(FileReference {
            artifact_id: id,
            name: Some("evidence.txt".into()),
            mime_type: Some("text/plain".into()),
            description: None,
        }));
    }
    let accepted = store
        .accept_inbound(InboundDraft {
            message_id: Some(MessageId::new("prompt-1")),
            source: UserSource::Human,
            kind: InboundKind::Message,
            content,
            timestamp: Utc.with_ymd_and_hms(2026, 10, 10, 12, 0, 0).unwrap(),
            correlation: None,
        })
        .unwrap();
    store.adopt_pending_batch(accepted.sequence, None).unwrap();
    complete_turn(&store, &conversation, 1);
    store
        .commit_compaction(CompactionCommitInput {
            summary: UserMessageBlock {
                id: MessageId::new("checkpoint"),
                content: vec![text("First turn summarized")],
                source: UserSource::Runtime,
                kind: InboundKind::CompactionSummary(CompactionSummaryMetadata::empty()),
                timestamp: None,
            },
            span: SurfaceSpan::new(MessageId::new("prompt-1"), MessageId::new("answer-1")),
            expected_revision: store.load_head().unwrap().revision,
            tokens_before: TokenMeasurement {
                input_tokens: 64,
                source: TokenMeasurementSource::Estimated,
            },
            estimated_tokens_after: 32,
            attempt_id: None,
            turn_id: None,
            timestamp: Utc.with_ymd_and_hms(2026, 10, 10, 12, 0, 2).unwrap(),
            occupancy: None,
        })
        .unwrap();
    let accepted = store
        .accept_inbound(InboundDraft {
            message_id: Some(MessageId::new("prompt-2")),
            source: UserSource::Human,
            kind: InboundKind::Message,
            content: vec![text("Second prompt")],
            timestamp: Utc.with_ymd_and_hms(2026, 10, 10, 12, 0, 3).unwrap(),
            correlation: None,
        })
        .unwrap();
    store.adopt_pending_batch(accepted.sequence, None).unwrap();
    complete_turn(&store, &conversation, 2);
    let cut = store
        .read_lineage_cut(store.load_head().unwrap().revision)
        .unwrap();
    assert_eq!(cut.turns.len(), 2);
    assert_eq!(cut.completed_responses.len(), 2);
    (store, cut)
}

#[allow(clippy::too_many_lines)] // Verify every seed field and native reading surface before and after reopening.
fn verify_copy(copy: &PreparedLineage, expected: &LineageSeed, artifact: bool, count: usize) {
    for _ in 0..2 {
        // Each iteration closes the SQLite store, then reads only destination authority.
        let store = SqliteConversationStore::open_existing(
            copy.conversation_id.clone(),
            &copy.database_path,
        )
        .unwrap();
        let inherited = store
            .read_lineage_cut(store.load_head().unwrap().revision)
            .unwrap();
        assert_eq!(
            inherited.turns,
            expected.turns(),
            "materialization must persist bootstrap turn_provenance"
        );
        assert_eq!(
            inherited.completed_responses,
            expected.completed_responses()
        );
        assert_eq!(inherited.surface_history, expected.surface_history());
        assert_eq!(inherited.checkpoints, *expected.checkpoints());
        let mut canonical = expected.canonical().to_vec();
        if artifact {
            let MessageBlock::User(prompt) = &mut canonical[0] else {
                panic!("human prompt")
            };
            let UserContentBlock::File(reference) = &mut prompt.content[1] else {
                panic!("artifact")
            };
            assert_eq!(reference.artifact_id, ArtifactId::new("artifact_2"));
            reference.artifact_id = ArtifactId::new("artifact_1");
            let owner = ArtifactStore::new(
                copy.conversation_id.clone(),
                copy.database_path.parent().unwrap(),
            )
            .unwrap();
            assert_eq!(
                owner.read_bounded(&reference.artifact_id).unwrap(),
                b"immutable artifact evidence"
            );
            assert_eq!(
                ArtifactStore::archive_lengths(owner.root(), || Ok(()))
                    .unwrap()
                    .len(),
                1
            );
        } else {
            assert!(
                ArtifactStore::archive_lengths(copy.database_path.parent().unwrap(), || Ok(()))
                    .unwrap()
                    .is_empty()
            );
        }
        assert_eq!(inherited.canonical, canonical);
        assert_eq!(
            inherited.messages,
            store
                .load_surface_snapshot(store.load_head().unwrap().revision)
                .unwrap()
        );
        for (id, statistics) in expected.checkpoints() {
            assert_eq!(statistics.retired_messages, 2);
            assert!(statistics.retired_tokens > 0);
            assert_eq!(
                store.compaction_checkpoint(id, u64::MAX).unwrap().0,
                *statistics
            );
        }
        assert!(store.read_events(None, 64).unwrap().events.is_empty());
        assert!(
            store
                .read_request_snapshots(None, 64)
                .unwrap()
                .snapshots
                .is_empty()
        );
        assert_eq!(store.presentation_frontier().unwrap(), 0);
        let directory = store.conversation_turns(0, 64).unwrap();
        assert_eq!(directory.total, count);
        assert_eq!(directory.turns.len(), count);
        for (index, turn) in directory.turns.iter().enumerate() {
            assert_eq!(turn.id, expected.turns()[index].id);
            assert_eq!(turn.ordinal, index + 1);
            assert_eq!(
                turn.prompt,
                if index == 0 {
                    "First prompt"
                } else {
                    "Second prompt"
                }
            );
            assert_eq!(turn.response, format!("Response {}", index + 1));
            let selected = store.conversation_turns(index, 1).unwrap();
            assert_eq!(selected.turns, vec![turn.clone()]);
            let window = store
                .conversation_window(
                    &ConversationWindowAt::Turn {
                        id: turn.id.clone(),
                        cut: directory.cut.clone(),
                    },
                    1,
                )
                .unwrap();
            assert_eq!(window.target.as_ref(), Some(&turn.id));
            assert_eq!(window.target_cursor, turn.cursor);
            let mut page =
                crate::runtime_client::snapshot::transcript_page_view(window.page).unwrap();
            crate::runtime_client::response::decorate(&store, &mut page).unwrap();
            let process = page.entries[0].turn_process.as_ref().unwrap();
            assert_eq!(
                process.outcome,
                crate::durable::reading::TurnProcessOutcome::Completed
            );
            assert!(process.event_id.is_none());
            assert_eq!(process.ended_at, expected.turns()[index].ended_at);
        }
    }
}

fn copy_history(fork: bool) {
    let mut comparable = Vec::new();
    for artifact in [false, true] {
        let (_root, mut catalog, _) = open_catalog();
        let (store, cut) = source(&catalog, artifact);
        let (copy, selected) = if fork {
            let (copy, draft) = catalog
                .prepare_fork_session(
                    &state(),
                    &cut,
                    &MessageId::new("prompt-2"),
                    super::super::LineageSide::Before,
                )
                .unwrap();
            assert_eq!(draft, vec![text("Second prompt")]);
            let boundary = store
                .load_user_message_boundaries(cut.surface_revision)
                .unwrap()
                .into_iter()
                .find(|boundary| boundary.message.id == MessageId::new("prompt-2"))
                .unwrap();
            let selected = store
                .read_lineage_cut(SurfaceRevision::new(boundary.surface_revision.get() - 1))
                .unwrap();
            (copy, selected)
        } else {
            (catalog.prepare_clone_session(&state(), &cut).unwrap(), cut)
        };
        let count = if fork { 1 } else { 2 };
        assert_eq!(selected.turns.len(), count);
        let expected = super::super::remap_seed(
            &copy.conversation_id,
            &selected.canonical,
            &selected.surface_history,
            &selected.checkpoints,
            &selected.completed_responses,
            &selected.turns,
        )
        .unwrap();
        for (original, copied) in selected.turns.iter().zip(expected.turns()) {
            assert_eq!(original.id, copied.id);
            assert_eq!(original.execution, copied.execution);
            assert_eq!(original.started_at, copied.started_at);
            assert_eq!(original.ended_at, copied.ended_at);
            assert_eq!(copied.outcome, InheritedTurnOutcome::Completed);
            assert_ne!(original.process_message_ids, copied.process_message_ids);
            assert_ne!(original.prompt_message_id, copied.prompt_message_id);
        }
        comparable.push((
            expected
                .turns()
                .iter()
                .map(|turn| {
                    (
                        turn.id.attempt_id.clone(),
                        turn.outcome.clone(),
                        turn.execution.clone(),
                        turn.started_at,
                        turn.ended_at,
                    )
                })
                .collect::<Vec<_>>(),
            expected
                .checkpoints()
                .values()
                .map(|stats| stats.retired_messages)
                .collect::<Vec<_>>(),
        ));
        catalog
            .publish_session(&copy, SessionNodeOrigin::New)
            .unwrap();
        drop(store);
        verify_copy(&copy, &expected, artifact, count);
    }
    assert_eq!(
        comparable[0], comparable[1],
        "Artifact presence cannot change inherited history semantics"
    );
}

#[test]
fn artifact_clone_preserves_complete_native_provenance_after_restart() {
    copy_history(false);
}

#[test]
fn artifact_fork_preserves_only_selected_native_prefix_after_restart() {
    copy_history(true);
}
