use crate::durable::inbox::{CommittedModelTurn, ModelTurnBoundary, ModelTurnSettlement};

struct FinalFixture {
    store: SqliteConversationStore,
    start: PublicationStreamStart,
    message: MessageBlock,
    frames: Vec<PublicationFrame>,
}
impl FinalFixture {
    fn new() -> Self {
        let store = store();
        store.initialize(&[]).unwrap();
        let mut snapshot = test_request_snapshot(store.load_head().unwrap().revision, invocation());
        freeze_test_input(&store, &mut snapshot, &[]);
        store
            .append_event(envelope(
                store.conversation_id(),
                "attempt",
                Some(snapshot.identity.attempt_id.clone()),
                RuntimeEvent::AttemptStarted {
                    attempt_id: snapshot.identity.attempt_id.clone(),
                },
            ))
            .unwrap();
        let mut turn = envelope(
            store.conversation_id(),
            "turn-start",
            Some(snapshot.identity.attempt_id.clone()),
            RuntimeEvent::TurnStarted,
        );
        turn.turn_id = Some(snapshot.identity.turn.clone());
        store.append_event(turn).unwrap();
        store
            .commit_model_turn_start(&[], &snapshot, Utc::now())
            .unwrap();
        let start = PublicationStreamStart {
            stream_id: PublicationStreamId::for_request(
                &snapshot.identity.attempt_id,
                &snapshot.provisional_message_id,
            ),
            message_id: snapshot.provisional_message_id,
            attempt_id: snapshot.identity.attempt_id,
            turn_id: snapshot.identity.turn,
            request_id: snapshot.request_id,
        };
        store.open_publication_stream(&start).unwrap();
        let message = MessageBlock::Assistant(crate::message::types::AssistantMessageBlock {
            id: start.message_id.clone(),
            content: vec![crate::message::types::AssistantContentBlock::Text(
                crate::message::content::TextBlock {
                    text: "final 水".into(),
                },
            )],
        });
        let frames = vec![PublicationFrame {
            stream_id: start.stream_id.clone(),
            message_id: start.message_id.clone(),
            sequence: 0,
            payload: crate::publication::PublicationPayload::TextSuffix {
                block_index: crate::message::types::ContentBlockIndex::new(0),
                suffix: "final 水".into(),
            },
        }];
        Self {
            store,
            start,
            message,
            frames,
        }
    }
    fn event(&self, id: &str, event: RuntimeEvent) -> RuntimeEventEnvelope {
        let mut event = envelope(
            self.store.conversation_id(),
            id,
            Some(self.start.attempt_id.clone()),
            event,
        );
        event.turn_id = (!matches!(&event.event, RuntimeEvent::AttemptCompleted { .. }))
            .then(|| self.start.turn_id.clone());
        event
    }
    fn candidate(&self, pending: Option<InboundSequence>) -> ModelTurnSettlement<'_> {
        ModelTurnSettlement {
            stream_id: &self.start.stream_id,
            provider: self.event(
                "provider",
                RuntimeEvent::ModelRequestCompleted {
                    request_id: self.start.request_id.clone(),
                    finish_reason: ModelFinishReason::Stop,
                    usage: None,
                    generation: None,
                },
            ),
            frames: &self.frames,
            assistant: &self.message,
            canonical: self.event(
                "canonical",
                RuntimeEvent::AssistantMessageCommitted {
                    message_id: self.start.message_id.clone(),
                },
            ),
            boundary: Some(ModelTurnBoundary {
                pending_watermark: pending,
                turn_completed: self.event("turn", RuntimeEvent::TurnCompleted),
                attempt_completed: self.event(
                    "terminal",
                    RuntimeEvent::AttemptCompleted {
                        attempt_id: self.start.attempt_id.clone(),
                        finish_reason: ModelFinishReason::Stop,
                    },
                ),
            }),
        }
    }
    fn settle(
        &self,
        pending: Option<InboundSequence>,
    ) -> Result<CommittedModelTurn, ConversationStoreError> {
        self.store.settle_model_turn(self.candidate(pending))
    }
}

#[test]
fn final_model_commit_contains_ordered_facts_and_readable_canonical_output() {
    use base64::Engine as _;
    let fixture = FinalFixture::new();
    let result = fixture.settle(None).unwrap();
    assert_eq!(
        fixture.store.load_canonical().unwrap(),
        vec![fixture.message.clone()]
    );
    assert!(result.provider.sequence < result.canonical.sequence);
    let boundary = result.boundary.unwrap();
    assert!(boundary.adopted.is_none());
    assert!(result.canonical.sequence < boundary.turn_completed.sequence);
    let terminal = boundary.attempt_completed.unwrap();
    assert!(boundary.turn_completed.sequence < terminal.sequence);
    assert!(
        terminal.turn_id.is_none(),
        "Attempt terminal is not a Turn fact"
    );
    let source = crate::runtime_client::content::ContentSource::CanonicalMessageBlock {
        component: crate::runtime_client::content::CanonicalContentComponent::Payload,
        locator: crate::runtime_client::content::CanonicalContentLocator::ByMessageId {
            message_id: fixture.start.message_id.clone(),
        },
        block_index: crate::message::types::ContentBlockIndex::new(0),
    };
    let reference = fixture
        .store
        .content_descriptor(
            &source,
            crate::runtime_client::content::ContentRepresentation::Utf8Text,
        )
        .unwrap()
        .unwrap();
    let content = fixture.store.content_read(&reference, 0, 64).unwrap();
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(content.data_base64)
            .unwrap(),
        "final 水".as_bytes()
    );
    assert!(
        fixture
            .store
            .load_unsettled_publication_streams()
            .unwrap()
            .is_empty()
    );
    assert!(
        fixture.settle(None).is_err(),
        "a committed output cannot be accepted twice"
    );
}

#[test]
fn final_model_faults_roll_back_provider_frames_canonical_and_terminal_together() {
    for stage in 0..3 {
        let fixture = FinalFixture::new();
        let frontier = fixture.store.presentation_frontier().unwrap();
        let fault = match stage {
            0 => &fixture.store.fail_publication_terminal_remaining,
            1 => &fixture.store.fail_event_remaining,
            _ => &fixture.store.fail_terminal_event_remaining,
        };
        fault.store(1, Ordering::SeqCst);
        assert!(fixture.settle(None).is_err());
        assert!(fixture.store.load_canonical().unwrap().is_empty());
        assert_eq!(fixture.store.presentation_frontier().unwrap(), frontier);
        let streams = fixture.store.load_unsettled_publication_streams().unwrap();
        assert_eq!(streams.len(), 1);
        assert!(streams[0].terminal_sequence.is_none());
        let connection = fixture.store.lock().unwrap();
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM publication_frames", [], |row| row
                    .get::<_, u64>(0))
                .unwrap(),
            0
        );
        drop(connection);
        assert!(
            fixture.settle(None).is_ok(),
            "rollback left no provisional acceptance certificate"
        );
    }
}

#[test]
fn final_model_adopts_finite_pending_order_without_terminal_and_preserves_cursors() {
    let fixture = FinalFixture::new();
    let first = fixture.store.accept_inbound(draft("first")).unwrap();
    let second = fixture.store.accept_inbound(draft("second")).unwrap();
    let batch = fixture.store.select_pending_batch().unwrap().unwrap();
    let third = fixture
        .store
        .accept_inbound(draft("arrived after selection"))
        .unwrap();
    let all_pending = fixture.store.load_pending().unwrap();
    let frontier = fixture.store.presentation_frontier().unwrap();
    assert!(
        fixture.settle(None).is_err(),
        "empty boundary cannot consume or ignore pending input"
    );
    assert_eq!(fixture.store.presentation_frontier().unwrap(), frontier);
    fixture.store.arm_fail_next_adopt_commit();
    assert!(fixture.settle(Some(batch.watermark)).is_err());
    assert!(fixture.store.load_canonical().unwrap().is_empty());
    assert_eq!(fixture.store.load_pending().unwrap(), all_pending);
    let result = fixture.settle(Some(batch.watermark)).unwrap();
    let boundary = result.boundary.unwrap();
    assert!(boundary.attempt_completed.is_none());
    let adopted = boundary.adopted.unwrap();
    assert_eq!(adopted.items, batch.items);
    assert_eq!(adopted.items[0].transcript_cursor, first.transcript_cursor);
    assert_eq!(adopted.items[1].transcript_cursor, second.transcript_cursor);
    let remaining = fixture.store.load_pending().unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].message_id, third.message_id);
    assert_eq!(remaining[0].transcript_cursor, third.transcript_cursor);
    let canonical = fixture.store.load_canonical().unwrap();
    assert_eq!(canonical.len(), 3);
    assert_eq!(canonical[0], fixture.message);
    assert_eq!(canonical[1].id(), &adopted.items[0].message_id);
    assert_eq!(canonical[2].id(), &adopted.items[1].message_id);
}

#[test]
fn final_model_rejects_foreign_envelopes_and_wrong_terminal_without_partial_facts() {
    for fault in 0..4 {
        let fixture = FinalFixture::new();
        let frontier = fixture.store.presentation_frontier().unwrap();
        let mut candidate = fixture.candidate(None);
        match fault {
            0 => candidate.provider.turn_id = Some(TurnId::new("foreign")),
            1 => candidate.canonical.attempt_id = Some(AttemptId::new("foreign")),
            2 => {
                candidate.boundary.as_mut().unwrap().attempt_completed.event =
                    RuntimeEvent::AttemptCancelled {
                        attempt_id: fixture.start.attempt_id.clone(),
                        reason: crate::runtime::types::CancellationReason::UserRequested,
                    };
            }
            _ => {
                candidate
                    .boundary
                    .as_mut()
                    .unwrap()
                    .attempt_completed
                    .turn_id = Some(fixture.start.turn_id.clone());
            }
        }
        assert!(fixture.store.settle_model_turn(candidate).is_err());
        assert_eq!(fixture.store.presentation_frontier().unwrap(), frontier);
        assert!(fixture.store.load_canonical().unwrap().is_empty());
        assert!(
            fixture.store.load_unsettled_publication_streams().unwrap()[0]
                .terminal_sequence
                .is_none()
        );
    }
}

#[test]
fn independent_final_commit_schema_is_refused_without_rewriting() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("independent-final.sqlite");
    let conversation = ConversationId::generate();
    let store = SqliteConversationStore::open(conversation.clone(), &path).unwrap();
    store.initialize(&[]).unwrap();
    store
        .conn
        .lock()
        .unwrap()
        .execute("UPDATE rustx_store SET schema_version=74", [])
        .unwrap();
    drop(store);
    assert!(matches!(
        SqliteConversationStore::open_existing(conversation, &path),
        Err(ConversationStoreError::SchemaVersionMismatch {
            stored: 74,
            expected: 76
        })
    ));
    let connection = Connection::open(&path).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT schema_version FROM rustx_store", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        74
    );
}
