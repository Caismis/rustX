use crate::durable::inbox::{CommittedModelTurn, ModelTurnBoundary, ModelTurnSettlement};

struct FinalFixture {
    store: SqliteConversationStore,
    start: PublicationStreamStart,
    message: MessageBlock,
    frames: Vec<PublicationFrame>,
}
impl FinalFixture {
    fn new() -> Self {
        Self::with_store(store())
    }
    fn with_store(store: SqliteConversationStore) -> Self {
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
        let finish_reason = if matches!(&self.message, MessageBlock::Assistant(message) if message.content.iter().any(|block|matches!(block,AssistantContentBlock::ToolCall(_))))
        {
            ModelFinishReason::ToolCalls
        } else {
            ModelFinishReason::Stop
        };
        ModelTurnSettlement {
            stream_id: &self.start.stream_id,
            provider: self.event(
                "provider",
                RuntimeEvent::ModelRequestCompleted {
                    request_id: self.start.request_id.clone(),
                    finish_reason: finish_reason.clone(),
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
                        finish_reason,
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

fn forbid_final_staging(fixture: &FinalFixture) {
    fixture.store.lock().unwrap().execute_batch(
        "CREATE TRIGGER reject_final_staging BEFORE INSERT ON publication_frames BEGIN SELECT RAISE(FAIL,'final staging must not be retained'); END;",
    ).unwrap();
}

#[test]
fn final_model_text_reopens_without_transient_frame_or_live_content_writes() {
    for terminal_only in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("final-text.sqlite");
        let conversation = ConversationId::generate();
        let mut fixture = FinalFixture::with_store(
            SqliteConversationStore::open(conversation.clone(), &path).unwrap(),
        );
        if terminal_only {
            let receipts = fixture
                .store
                .stage_publication_frames(&fixture.frames)
                .unwrap();
            assert!(receipts.iter().all(|receipt| !receipt.unavailable));
            fixture.frames[0].sequence = 1;
            fixture.frames[0].payload = PublicationPayload::TerminalOnly;
        }
        forbid_final_staging(&fixture);
        fixture.store.lock().unwrap().execute_batch(
        "CREATE TABLE final_live_writes(kind TEXT NOT NULL);
         CREATE TRIGGER count_final_live_segments AFTER INSERT ON content_segments WHEN NEW.source_kind='live' BEGIN INSERT INTO final_live_writes VALUES('segment'); END;
         CREATE TRIGGER count_final_live_frontiers AFTER INSERT ON content_frontiers WHEN NEW.source_kind='live' BEGIN INSERT INTO final_live_writes VALUES('frontier'); END;
         CREATE TRIGGER count_final_live_blocks AFTER INSERT ON publication_observation_blocks BEGIN INSERT INTO final_live_writes VALUES('block'); END;",
    ).unwrap();
        fixture.settle(None).unwrap();
        let expected = fixture.message.clone();
        let stream_id = fixture.start.stream_id.clone();
        drop(fixture);
        let reopened = SqliteConversationStore::open_existing(conversation, &path).unwrap();
        assert_eq!(reopened.load_canonical().unwrap(), vec![expected]);
        let connection = reopened.lock().unwrap();
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM final_live_writes", [], |row| row
                    .get::<_, u64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM publication_frames", [], |row| row
                    .get::<_, u64>(0))
                .unwrap(),
            0
        );
        let terminal = u64::from(terminal_only);
        assert_eq!(connection.query_row("SELECT next_frame_sequence,terminal_sequence,settlement FROM publication_streams WHERE stream_id=?1", [stream_id.as_str()], |row| Ok((row.get::<_, u64>(0)?,row.get::<_, u64>(1)?,row.get::<_, String>(2)?))).unwrap(), (terminal+1,terminal,"canonical".into()));
    }
}

fn final_tool_frames(fixture: &mut FinalFixture) -> Vec<PublicationFrame> {
    let call = ToolCall {
        id: ToolCallId::new("final-call"),
        tool_id: ToolId::new("tool-bash"),
        name: "bash".into(),
        arguments: serde_json::json!({"z":2,"a":"水"}),
    };
    let block_index = ContentBlockIndex::new(0);
    let mut payloads = vec![PublicationPayload::ProposedToolCallStarted {
        block_index,
        call: crate::tools::types::ToolCallStart {
            id: call.id.clone(),
            tool_id: call.tool_id.clone(),
            name: call.name.clone(),
        },
    }];
    for suffix in ["{ \"z\":", " 2, \"a\":", " \"水\" }"] {
        payloads.push(PublicationPayload::ProposedToolCallArgumentsSuffix {
            block_index,
            call_id: call.id.clone(),
            suffix: suffix.into(),
        });
    }
    payloads.push(PublicationPayload::ProposedToolCallCompleted {
        block_index,
        call: call.clone(),
    });
    fixture.message = MessageBlock::Assistant(AssistantMessageBlock {
        id: fixture.start.message_id.clone(),
        content: vec![AssistantContentBlock::ToolCall(call)],
    });
    payloads
        .into_iter()
        .enumerate()
        .map(|(sequence, payload)| PublicationFrame {
            stream_id: fixture.start.stream_id.clone(),
            message_id: fixture.start.message_id.clone(),
            sequence: sequence as u64,
            payload,
        })
        .collect()
}

#[test]
fn final_model_tool_raw_suffixes_reopen_with_live_or_unindexed_prefix() {
    for (prefix, failed_layout) in [(false, false), (true, false), (true, true)] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("final-tool.sqlite");
        let conversation = ConversationId::generate();
        let mut fixture = FinalFixture::with_store(
            SqliteConversationStore::open(conversation.clone(), &path).unwrap(),
        );
        let frames = final_tool_frames(&mut fixture);
        if failed_layout {
            fixture.store.lock().unwrap().execute_batch("CREATE TRIGGER fail_live_layout BEFORE INSERT ON content_frontiers WHEN NEW.source_kind='live' BEGIN SELECT RAISE(FAIL,'missing optional live layout'); END;").unwrap();
        }
        if prefix {
            let receipts = fixture
                .store
                .stage_publication_frames(&frames[..3])
                .unwrap();
            assert_eq!(
                receipts.iter().any(|receipt| receipt.unavailable),
                failed_layout
            );
            fixture.frames = frames[3..].to_vec();
        } else {
            fixture.frames = frames;
        }
        forbid_final_staging(&fixture);
        let frontier = fixture.store.presentation_frontier().unwrap();
        fixture.store.lock().unwrap().execute_batch("CREATE TRIGGER fail_final_raw BEFORE INSERT ON content_frontiers WHEN NEW.source_kind='canonical' AND NEW.representation='raw_json_utf8' BEGIN SELECT RAISE(FAIL,'mandatory raw retention'); END;").unwrap();
        let mut candidate = fixture.candidate(None);
        candidate.boundary = None;
        assert!(fixture.store.settle_model_turn(candidate).is_err());
        assert_eq!(fixture.store.presentation_frontier().unwrap(), frontier);
        assert!(fixture.store.load_canonical().unwrap().is_empty());
        {
            let connection = fixture.store.lock().unwrap();
            assert_eq!(
                connection
                    .query_row("SELECT COUNT(*) FROM publication_frames", [], |row| row
                        .get::<_, u64>(0))
                    .unwrap(),
                if prefix { 3 } else { 0 }
            );
            assert_eq!(connection.query_row("SELECT next_frame_sequence,terminal_sequence,settlement FROM publication_streams",[],|row|Ok((row.get::<_,u64>(0)?,row.get::<_,Option<u64>>(1)?,row.get::<_,Option<String>>(2)?))).unwrap(),(if prefix {3}else{0},None,None));
            if prefix {
                assert_eq!(
                    connection
                        .query_row("SELECT state FROM publication_proposals", [], |row| row
                            .get::<_, String>(
                            0
                        ))
                        .unwrap(),
                    "started"
                );
            } else {
                assert_eq!(
                    connection
                        .query_row("SELECT COUNT(*) FROM publication_proposals", [], |row| row
                            .get::<_, u64>(
                            0
                        ))
                        .unwrap(),
                    0
                );
            }
            connection
                .execute_batch("DROP TRIGGER fail_final_raw;")
                .unwrap();
        }
        let mut candidate = fixture.candidate(None);
        candidate.boundary = None;
        fixture.store.settle_model_turn(candidate).unwrap();
        let message = fixture.message.clone();
        drop(fixture);
        let reopened = SqliteConversationStore::open_existing(conversation, &path).unwrap();
        assert_eq!(reopened.load_canonical().unwrap(), vec![message.clone()]);
        let connection = reopened.lock().unwrap();
        let raw = content::canonical_raw_arguments(&connection, &[message]).unwrap();
        assert_eq!(raw.len(), 1);
        assert_eq!(raw[0].bytes, "{ \"z\": 2, \"a\": \"水\" }".as_bytes());
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM publication_frames", [], |row| row
                    .get::<_, u64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT state,settlement FROM publication_proposals",
                    [],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                )
                .unwrap(),
            ("completed".into(), "canonical".into())
        );
    }
}

#[test]
fn final_model_frames_keep_sequence_identity_and_proposal_validation_atomic() {
    for invalid in 0..4 {
        let mut fixture = FinalFixture::new();
        let frontier = fixture.store.presentation_frontier().unwrap();
        match invalid {
            0 => fixture.frames[0].sequence = 1,
            1 => fixture.frames[0].message_id = MessageId::new("foreign"),
            2 => fixture.frames[0].stream_id = PublicationStreamId::new("foreign"),
            _ => {
                fixture.frames[0].payload = PublicationPayload::ProposedToolCallArgumentsSuffix {
                    block_index: ContentBlockIndex::new(0),
                    call_id: ToolCallId::new("orphan"),
                    suffix: "{}".into(),
                }
            }
        }
        assert!(fixture.settle(None).is_err());
        assert_eq!(fixture.store.presentation_frontier().unwrap(), frontier);
        assert!(fixture.store.load_canonical().unwrap().is_empty());
        let connection = fixture.store.lock().unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT next_frame_sequence,terminal_sequence FROM publication_streams",
                    [],
                    |row| Ok((row.get::<_, u64>(0)?, row.get::<_, Option<u64>>(1)?))
                )
                .unwrap(),
            (0, None)
        );
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM publication_proposals", [], |row| row
                    .get::<_, u64>(
                    0
                ))
                .unwrap(),
            0
        );
    }
}

#[test]
fn final_model_tool_owner_mismatch_rolls_back_final_progress() {
    for invalid in 0..3 {
        let mut fixture = FinalFixture::new();
        fixture.frames = final_tool_frames(&mut fixture);
        let MessageBlock::Assistant(message) = &mut fixture.message else {
            unreachable!();
        };
        if invalid == 0 {
            message.content.insert(
                0,
                AssistantContentBlock::Text(crate::message::content::TextBlock {
                    text: "shifted tool".into(),
                }),
            );
        } else {
            let AssistantContentBlock::ToolCall(call) = &mut message.content[0] else {
                unreachable!();
            };
            if invalid == 1 {
                call.id = ToolCallId::new("foreign-call");
            } else {
                call.tool_id = ToolId::new("foreign-tool");
            }
        }
        let frontier = fixture.store.presentation_frontier().unwrap();
        let mut candidate = fixture.candidate(None);
        candidate.boundary = None;
        assert!(fixture.store.settle_model_turn(candidate).is_err());
        assert_eq!(fixture.store.presentation_frontier().unwrap(), frontier);
        assert!(fixture.store.load_canonical().unwrap().is_empty());
        let connection = fixture.store.lock().unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT next_frame_sequence,terminal_sequence,settlement FROM publication_streams",
                    [],
                    |row| Ok((row.get::<_, u64>(0)?, row.get::<_, Option<u64>>(1)?, row.get::<_, Option<String>>(2)?))
                )
                .unwrap(),
            (0, None, None)
        );
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM publication_proposals", [], |row| row
                    .get::<_, u64>(
                    0
                ))
                .unwrap(),
            0
        );
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
            expected: 78
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
