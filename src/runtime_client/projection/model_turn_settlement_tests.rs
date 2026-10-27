use crate::durable::ConversationStore;
use crate::durable::inbox::{ModelTurnBoundary, ModelTurnSettlement};
use crate::durable::observation::{
    ModelTurnObservationBoundary, NativeObservationBatch, NativeObservationTransition,
};

fn apply_split_model_member(
    projection: &mut RuntimeClientProjection,
    family: &NativeObservationBatch,
    witness: &crate::durable::observation::ObservationEventWitness,
    canonical: Option<Arc<crate::durable::inbox::CanonicalObservationBatch>>,
) {
    // Independent former per-member receipt shape, kept inside one outer test
    // publication so byte/cursor differences do not hide semantic differences.
    let mut member = family.clone();
    member.transition = NativeObservationTransition::Single(witness.clone());
    member.journal_range = Some((witness.sequence, witness.sequence));
    member.canonical = canonical;
    member.pending = None;
    projection.apply_native_batch(&member);
}

#[test]
#[allow(clippy::too_many_lines)] // The real native family and its independent ordered-member oracle.
fn model_turn_family_matches_ordered_native_members_and_keeps_assistant_before_large_adoption() {
    for pending_count in [0, 1, 9] {
        let (mut actual, store) = initialized_source_projection();
        let mut expected = projection();
        expected
            .initialize_descriptor_heads(&store, store.observation_read_cut().unwrap())
            .unwrap();
        expected.initialize_publication_view();
        let started = append_source_attempt_start(&store, "model-family-attempt");
        for projection in [&mut actual, &mut expected] {
            publish_source_fact(projection, |projection| {
                projection.apply_native_batch(started.observation.as_deref().unwrap());
            });
        }
        store
            .mark_native_observation_ready(started.observation.as_deref().unwrap())
            .unwrap();
        let turn = store
            .append_event(source_tool_event(
                &store,
                RuntimeEvent::TurnStarted,
                "model-family-turn",
            ))
            .unwrap();
        for projection in [&mut actual, &mut expected] {
            publish_source_fact(projection, |projection| {
                projection.apply_native_batch(turn.observation.as_deref().unwrap());
            });
        }
        store
            .mark_native_observation_ready(turn.observation.as_deref().unwrap())
            .unwrap();
        let request = raw_publication_snapshot(crate::context::resources::LedgerFrontier(
            store.observation_read_cut().unwrap().ledger_through,
        ));
        let requested = store
            .commit_model_turn_start(&[], &request, chrono::Utc::now())
            .unwrap();
        for projection in [&mut actual, &mut expected] {
            publish_source_fact(projection, |projection| {
                projection.apply_native_batch(requested.started.observation.as_deref().unwrap());
            });
        }
        store
            .mark_native_observation_ready(requested.started.observation.as_deref().unwrap())
            .unwrap();
        let mut pending = None;
        for index in 0..pending_count {
            let accepted = ConversationStore::accept_inbound(
                &store,
                crate::durable::inbox::InboundDraft {
                    message_id: Some(MessageId::new(format!("model-family-user-{index}"))),
                    source: crate::message::types::UserSource::Human,
                    kind: crate::message::types::InboundKind::Message,
                    content: vec![crate::message::types::UserContentBlock::Text(
                        crate::message::content::TextBlock {
                            text: format!("follow up {index}"),
                        },
                    )],
                    timestamp: chrono::Utc::now(),
                    correlation: None,
                },
            )
            .unwrap();
            pending = Some(accepted.sequence);
            for projection in [&mut actual, &mut expected] {
                publish_source_fact(projection, |projection| {
                    projection.apply_pending_receipt(accepted.pending_observation.as_deref());
                });
            }
            if let Some(cursor) = accepted.transcript_cursor {
                store
                    .mark_observation_transcript_ready(cursor.get())
                    .unwrap();
            }
        }
        let start = crate::publication::PublicationStreamStart {
            stream_id: crate::runtime::identity::PublicationStreamId::for_request(
                &request.identity.attempt_id,
                &request.provisional_message_id,
            ),
            attempt_id: request.identity.attempt_id.clone(),
            turn_id: request.identity.turn.clone(),
            request_id: request.request_id.clone(),
            message_id: request.provisional_message_id.clone(),
        };
        store.open_publication_stream(&start).unwrap();
        for projection in [&mut actual, &mut expected] {
            publish_source_fact(projection, |projection| {
                projection.apply(ConversationObservation::PublicationOpened {
                    attempt_id: attempt(),
                    start: start.clone(),
                });
            });
        }
        let message = MessageBlock::Assistant(AssistantMessageBlock {
            id: start.message_id.clone(),
            content: vec![AssistantContentBlock::Text(
                crate::message::content::TextBlock {
                    text: "family final 水".into(),
                },
            )],
        });
        let frames = vec![PublicationFrame {
            stream_id: start.stream_id.clone(),
            message_id: start.message_id.clone(),
            sequence: 0,
            payload: PublicationPayload::TextSuffix {
                block_index: ContentBlockIndex::new(0),
                suffix: "family final 水".into(),
            },
        }];
        let mut terminal = source_tool_event(
            &store,
            RuntimeEvent::AttemptCompleted {
                attempt_id: attempt(),
                finish_reason: crate::model::ModelFinishReason::Stop,
            },
            "model-family-terminal",
        );
        terminal.turn_id = None;
        let committed = store
            .settle_model_turn(ModelTurnSettlement {
                stream_id: &start.stream_id,
                provider: source_tool_event(
                    &store,
                    RuntimeEvent::ModelRequestCompleted {
                        request_id: start.request_id.clone(),
                        finish_reason: crate::model::ModelFinishReason::Stop,
                        usage: None,
                        generation: None,
                    },
                    "model-family-provider",
                ),
                frames: &frames,
                assistant: &message,
                canonical: source_tool_event(
                    &store,
                    RuntimeEvent::AssistantMessageCommitted {
                        message_id: start.message_id.clone(),
                    },
                    "model-family-canonical",
                ),
                boundary: Some(ModelTurnBoundary {
                    pending_watermark: pending,
                    turn_completed: source_tool_event(
                        &store,
                        RuntimeEvent::TurnCompleted,
                        "model-family-turn-complete",
                    ),
                    attempt_completed: terminal,
                }),
            })
            .unwrap();
        let family = committed.provider.observation.as_deref().unwrap();
        let NativeObservationTransition::ModelTurnSettlement {
            provider,
            canonical,
            assistant,
            boundary,
        } = &family.transition
        else {
            panic!("one explicit atomic family")
        };
        assert_eq!(assistant.count, 1);
        let old = actual.checkpoint_seed();
        let old_snapshot = old.checkpoint_candidate().unwrap();
        let before = actual.cursor;
        publish_source_fact(&mut actual, |projection| {
            projection.apply_native_batch(family);
        });
        publish_source_fact(&mut expected, |projection| {
            apply_split_model_member(projection, family, provider, None);
            apply_split_model_member(projection, family, canonical, Some(assistant.clone()));
            match boundary {
                Some(ModelTurnObservationBoundary::Completed { turn, attempt }) => {
                    apply_split_model_member(projection, family, turn, None);
                    apply_split_model_member(projection, family, attempt, None);
                }
                Some(ModelTurnObservationBoundary::Adopted {
                    turn,
                    adoption,
                    canonical,
                }) => {
                    apply_split_model_member(projection, family, turn, None);
                    projection.apply_pending_receipt(family.pending.as_deref());
                    apply_split_model_member(projection, family, adoption, Some(canonical.clone()));
                }
                None => unreachable!(),
            }
        });
        assert_eq!(
            actual.cursor.get(),
            before.get() + 1,
            "one complete published cut"
        );
        assert_eq!(
            actual.snapshot, expected.snapshot,
            "pending={pending_count}"
        );
        assert_eq!(
            actual.replay.back().unwrap().1.facts(),
            expected.replay.back().unwrap().1.facts(),
            "ordered public facts, pending={pending_count}"
        );
        assert_eq!(
            old.checkpoint_candidate().unwrap(),
            old_snapshot,
            "held historical cut remains immutable"
        );
        actual.validate_source_prefix().unwrap();
        store.mark_native_observation_ready(family).unwrap();
        assert_eq!(
            store
                .observation_ready_frontier(0, family.read_cut.read.journal)
                .unwrap(),
            family.read_cut.read.journal
        );
        let facts = actual.replay.back().unwrap().1.facts();
        assert!(facts.iter().any(|fact| matches!(fact, RuntimeClientEvent::MessageCommitted { message: committed, .. } if committed.id() == message.id())), "the original Assistant remains observable even beyond the latest eight adopted headers");
    }
}
