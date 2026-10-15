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
        tool_occurrence: None,
        observation: None,
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
    let mut snapshot = RequestSnapshot::new(
        RequestIdentity {
            attempt_id: attempt.clone(),
            turn: TurnId::new(format!("native-step-{index}")),
            retry_number: 0,
        },
        crate::context::resources::LedgerFrontier((store.load_head().unwrap().revision).get()),
        "frozen lineage context".into(),
        Vec::new(),
        crate::runtime::RuntimeResourceRevision::new(1),
        ModelInvocationConfig {
            model: "historical/model".into(),
            protocol: ModelProtocol::OpenAiChatCompletions,
            max_output_tokens: 128,
            request_params: RequestParams::new(),
            capabilities: ModelCapabilities::text_only(true, true),
            compat: ModelCompat::default(),
        },
        4096,
        None,
        false,
        Vec::new(),
        CapabilityRevision::new(1),
        crate::context::assembly::ContextGeneration {
            id: 1,
            contributors: Vec::new(),
        },
        None,
        Vec::new(),
    );
    freeze_session_test_input(store, &mut snapshot, &[]);
    store
        .commit_model_turn_start(&[], &snapshot, timestamp)
        .unwrap();
    let mut terminal = event(RuntimeEvent::ModelRequestCompleted {
        request_id: snapshot.request_id,
        finish_reason: ModelFinishReason::Stop,
        usage: Some(crate::model::types::ModelUsage {
            input_tokens: u64::from(index) * 32,
            output_tokens: 4,
            total_tokens: u64::from(index) * 32 + 4,
            details: None,
        }),
        generation: None,
    });
    terminal.turn_id = Some(snapshot.identity.turn);
    store.append_event(terminal).unwrap();
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
    store
        .append_event(RuntimeEventEnvelope {
            tool_occurrence: None,
            observation: None,
            schema_version: EVENT_SCHEMA_VERSION,
            event_id: EventId::new("start-1"),
            sequence: 0,
            conversation_id: conversation.clone(),
            attempt_id: Some(AttemptId::new("native-attempt-1")),
            turn_id: None,
            timestamp: Utc.with_ymd_and_hms(2026, 10, 10, 12, 0, 1).unwrap(),
            event: RuntimeEvent::AttemptStarted {
                attempt_id: AttemptId::new("native-attempt-1"),
            },
        })
        .unwrap();
    store
        .adopt_pending_batch(accepted.sequence, Some(AttemptId::new("native-attempt-1")))
        .unwrap();
    complete_turn(&store, &conversation, 1);
    store
        .commit_projection(
            crate::scripted_suites::support::context::checkpoint_commit_fixture(
                &store,
                crate::scripted_suites::support::context::checkpoint_fixture(
                    &store,
                    "checkpoint",
                    2,
                    "First turn summarized",
                ),
                None,
            ),
        )
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
    store
        .append_event(RuntimeEventEnvelope {
            tool_occurrence: None,
            observation: None,
            schema_version: EVENT_SCHEMA_VERSION,
            event_id: EventId::new("start-2"),
            sequence: 0,
            conversation_id: conversation.clone(),
            attempt_id: Some(AttemptId::new("native-attempt-2")),
            turn_id: None,
            timestamp: Utc.with_ymd_and_hms(2026, 10, 10, 12, 0, 4).unwrap(),
            event: RuntimeEvent::AttemptStarted {
                attempt_id: AttemptId::new("native-attempt-2"),
            },
        })
        .unwrap();
    store
        .adopt_pending_batch(accepted.sequence, Some(AttemptId::new("native-attempt-2")))
        .unwrap();
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
        assert_eq!(inherited.projections.len(), expected.projections().len());
        for (actual, expected) in inherited.projections.iter().zip(expected.projections()) {
            assert_eq!(actual.activation.activation, expected.activation.activation);
            assert_eq!(actual.activation.ledger, expected.activation.ledger);
            assert_eq!(actual.activation.origin, expected.activation.origin);
            assert!(actual.activation.destination_installation.is_some());
            assert_eq!(
                actual.candidate.base_frontier,
                expected.candidate.base_frontier
            );
            assert_eq!(
                actual.candidate.summary_metadata,
                expected.candidate.summary_metadata
            );
        }
        store.projected_history().unwrap();
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
        assert!(store.read_events(None, 64).unwrap().events.is_empty());
        assert!(
            store
                .read_request_snapshots(None, 64)
                .unwrap()
                .snapshots
                .is_empty()
        );
        assert_eq!(store.presentation_frontier().unwrap(), 0);
        assert_eq!(
            crate::context::occupancy::read(&store, 0).unwrap(),
            crate::context::occupancy::inherit(
                expected.turns(),
                expected.canonical(),
                expected.projections()
            ),
            "Artifact remapping must preserve the selected inherited occupancy"
        );
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
            crate::runtime_client::response::native_oracle_decorate(&store, &mut page).unwrap();
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
        let expected = super::super::lineage_cut(
            &copy.conversation_id,
            &selected,
            None,
            super::super::LineageSide::After,
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
                .projections()
                .iter()
                .map(|checkpoint| checkpoint.candidate.base_frontier.0)
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

#[test]
fn artifact_tree_preserves_selected_occupancy_and_native_history_after_restart() {
    for artifact in [false, true] {
        let (_root, catalog, _) = open_catalog();
        let (store, cut) = source(&catalog, artifact);
        let session_id = catalog.document.sessions.keys().next().unwrap().clone();
        // The first cut predates the retained compaction and the newer request.
        // Selecting source latest occupancy would give 64 instead of 32 tokens.
        for count in [1, 2] {
            let selected = if count == 1 {
                store
                    .read_projection_lineage_cut(
                        crate::context::resources::LedgerFrontier(2),
                        crate::context::checkpoint::ProjectionActivationId(0),
                    )
                    .unwrap()
            } else {
                store.read_lineage_cut(cut.surface_revision).unwrap()
            };
            let (copy, draft) = catalog
                .prepare_tree_node(
                    &session_id,
                    &state(),
                    &selected,
                    &MessageId::new(format!("answer-{count}")),
                    super::super::LineageSide::After,
                )
                .unwrap();
            assert!(draft.is_empty());
            let expected = super::super::lineage_cut(
                &copy.conversation_id,
                &selected,
                None,
                super::super::LineageSide::After,
            )
            .unwrap();
            assert_eq!(
                crate::context::occupancy::inherit(
                    expected.turns(),
                    expected.canonical(),
                    expected.projections()
                )
                .unwrap()
                .input_tokens,
                count as u64 * 32
            );
            verify_copy(&copy, &expected, artifact, count);
        }
    }
}
