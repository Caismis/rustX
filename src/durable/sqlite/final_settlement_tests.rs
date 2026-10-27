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
            start: &self.start,
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
                .stage_publication_frames(&fixture.start, &fixture.frames)
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
                .stage_publication_frames(&fixture.start, &frames[..3])
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
            assert_eq!(connection.query_row("SELECT next_frame_sequence,terminal_sequence,settlement FROM publication_streams",[],|row|Ok((row.get::<_,u64>(0)?,row.get::<_,Option<u64>>(1)?,row.get::<_,Option<String>>(2)?))).optional().unwrap(),prefix.then_some((3,None,None)));
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
                .optional().unwrap(),
            None
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
                .optional().unwrap(),
            None
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
        assert!(streams.is_empty(), "first-write admission rolls back with output");
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
            fixture.store.load_unsettled_publication_streams().unwrap().is_empty()
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
            expected: crate::durable::sqlite::SQLITE_SCHEMA_VERSION
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

type FinalReadiness = (Vec<(u64, bool, bool)>, Vec<(u64, bool)>);
fn final_readiness(store: &SqliteConversationStore) -> FinalReadiness {
    let connection = store.lock().unwrap();
    let journal = connection
        .prepare("SELECT sequence,ready,direct FROM event_observation_readiness ORDER BY sequence")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let transcript = connection
        .prepare("SELECT position,ready FROM transcript_observation_readiness ORDER BY position")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    (journal, transcript)
}
fn final_family_events(result: &CommittedModelTurn) -> Vec<&RuntimeEventEnvelope> {
    let mut events = vec![&result.provider, &result.canonical];
    if let Some(boundary) = &result.boundary {
        events.push(&boundary.turn_completed);
        if let Some(terminal) = &boundary.attempt_completed {
            events.push(terminal);
        }
        if let Some(adoption) = boundary
            .adopted
            .as_ref()
            .and_then(|adopted| adopted.adoption.as_ref())
        {
            events.push(adoption);
        }
    }
    events
}
fn assert_final_shared_family(
    result: &CommittedModelTurn,
) -> &Arc<crate::durable::observation::NativeObservationBatch> {
    let events = final_family_events(result);
    let shared = result.provider.observation.as_ref().unwrap();
    let last = events.last().unwrap().sequence;
    assert_eq!(shared.journal_range, Some((result.provider.sequence, last)));
    assert_eq!(shared.read_cut.read.journal, last);
    assert!(!shared.source_unavailable);
    assert_eq!(
        result.canonical_receipt.observation.native_release_sequence,
        Some(last)
    );
    assert_eq!(
        shared.canonical.as_ref().unwrap().journal_range,
        shared.journal_range
    );
    assert_eq!(
        shared.canonical.as_ref().unwrap().native_release_sequence,
        Some(last)
    );
    for (index, event) in events.iter().enumerate() {
        assert_eq!(event.sequence, result.provider.sequence + index as u64);
        assert!(Arc::ptr_eq(shared, event.observation.as_ref().unwrap()));
    }
    shared
}

#[test]
fn final_model_shared_family_retains_terminal_and_tool_boundaries() {
    use crate::durable::observation::{ModelTurnObservationBoundary, NativeObservationTransition};
    for tools in [false, true] {
        let mut fixture = FinalFixture::new();
        if tools {
            fixture.frames = final_tool_frames(&mut fixture);
        }
        let before = fixture.store.presentation_frontier().unwrap();
        let mut candidate = fixture.candidate(None);
        if tools {
            candidate.boundary = None;
        }
        let result = fixture.store.settle_model_turn(candidate).unwrap();
        let shared = assert_final_shared_family(&result);
        let NativeObservationTransition::ModelTurnSettlement {
            provider,
            canonical,
            assistant,
            boundary,
        } = &shared.transition
        else {
            panic!("one native output must retain its closed model family");
        };
        assert_eq!(provider.sequence, result.provider.sequence);
        assert_eq!(provider.event_kind, "model_request_completed");
        assert_eq!(canonical.sequence, result.canonical.sequence);
        assert_eq!(canonical.event_kind, "assistant_message_committed");
        assert!(Arc::ptr_eq(
            assistant,
            &result.canonical_receipt.observation
        ));
        assert_eq!(assistant.count, 1);
        assert!(
            assistant
                .head
                .iter()
                .any(|header| header.identity.as_ref() == Some(&fixture.start.message_id))
        );
        if tools {
            assert!(boundary.is_none());
            assert_eq!(fixture.store.presentation_frontier().unwrap(), before + 2);
            assert!(shared.pending.is_none());
        } else {
            let Some(ModelTurnObservationBoundary::Completed { turn, attempt }) = boundary else {
                panic!("ordinary final answer must close its turn and attempt");
            };
            assert_eq!(turn.event_kind, "turn_completed");
            assert_eq!(attempt.event_kind, "attempt_completed");
            assert_eq!(fixture.store.presentation_frontier().unwrap(), before + 4);
        }
        assert!(
            final_readiness(&fixture.store)
                .0
                .iter()
                .all(|(_, _, direct)| !direct)
        );
    }
}

#[test]
#[allow(clippy::too_many_lines)] // Validate one atomic family across original and displaced canonical heads.
fn final_model_adoption_family_keeps_assistant_when_more_than_eight_users_replace_head() {
    use crate::durable::observation::{ModelTurnObservationBoundary, NativeObservationTransition};
    let fixture = FinalFixture::new();
    for index in 0..12 {
        fixture
            .store
            .accept_inbound(draft(&format!("selected-{index}")))
            .unwrap();
    }
    let selected = fixture.store.select_pending_batch().unwrap().unwrap();
    assert_eq!(selected.items.len(), 12);
    let remaining = fixture.store.accept_inbound(draft("not selected")).unwrap();
    let result = fixture.settle(Some(selected.watermark)).unwrap();
    let shared = assert_final_shared_family(&result);
    let NativeObservationTransition::ModelTurnSettlement {
        assistant,
        boundary:
            Some(ModelTurnObservationBoundary::Adopted {
                turn,
                adoption,
                canonical,
            }),
        ..
    } = &shared.transition
    else {
        panic!("selected pending input owns the adopted boundary");
    };
    assert_eq!(turn.event_kind, "turn_completed");
    assert_eq!(adoption.event_kind, "inbound_turn_adopted");
    assert!(
        assistant
            .head
            .iter()
            .any(|header| header.identity.as_ref() == Some(&fixture.start.message_id))
    );
    assert_eq!(canonical.head.len(), 8);
    assert!(
        canonical
            .head
            .iter()
            .all(|header| header.identity.as_ref() != Some(&fixture.start.message_id))
    );
    assert_eq!(canonical.count, 12);
    let committed = result.boundary.as_ref().unwrap().adopted.as_ref().unwrap();
    assert_eq!(committed.items, selected.items);
    assert!(
        result
            .boundary
            .as_ref()
            .unwrap()
            .attempt_completed
            .is_none()
    );
    assert!(
        committed
            .canonical_receipts
            .iter()
            .all(|receipt| Arc::ptr_eq(canonical, &receipt.observation))
    );
    let release = shared.canonical.as_ref().unwrap();
    assert_eq!(
        (
            release.first_position,
            release.last_position,
            release.count,
            release.transcript_count
        ),
        (1, 13, 13, 13)
    );
    assert_eq!(release.head, canonical.head);
    let pending = shared.pending.as_ref().unwrap();
    assert!(Arc::ptr_eq(
        pending,
        committed.pending_observation.as_ref().unwrap()
    ));
    assert_eq!(
        pending.native_release_sequence,
        shared.journal_range.map(|(_, last)| last)
    );
    assert_eq!(pending.total, 1);
    assert_eq!(pending.head[0].sequence, remaining.sequence.get());
    assert_eq!(fixture.store.load_canonical().unwrap().len(), 13);
    fixture.store.mark_native_observation_ready(shared).unwrap();
    let state = final_readiness(&fixture.store);
    let last = shared.journal_range.unwrap().1;
    assert!(
        state
            .0
            .iter()
            .filter(|(sequence, _, _)| *sequence >= result.provider.sequence && *sequence <= last)
            .all(|(_, ready, direct)| *ready && *direct)
    );
    let unselected_cursor = remaining.transcript_cursor.unwrap().get();
    assert!(
        !state
            .1
            .iter()
            .find(|(position, _)| *position == unselected_cursor)
            .unwrap()
            .1
    );
    assert!(
        state
            .1
            .iter()
            .filter(|(position, _)| *position != unselected_cursor)
            .all(|(_, ready)| *ready)
    );
}

#[test]
fn final_model_missing_or_corrupt_family_witness_never_restores_partial_receipts() {
    for member in 1..=4 {
        for bytes in ["NULL", "x'7b'"] {
            let fixture = FinalFixture::new();
            let before = fixture.store.presentation_frontier().unwrap();
            let sequence = before + member;
            fixture.store.lock().unwrap().execute_batch(&format!(
                "CREATE TRIGGER damage_final_witness AFTER UPDATE OF observation_witness ON events WHEN NEW.sequence={sequence} AND NEW.observation_witness IS NOT NULL BEGIN UPDATE events SET observation_witness={bytes} WHERE sequence=NEW.sequence; END;"
            )).unwrap();
            let result = fixture.settle(None).unwrap();
            let last = result
                .boundary
                .as_ref()
                .unwrap()
                .attempt_completed
                .as_ref()
                .unwrap()
                .sequence;
            assert!(
                final_family_events(&result)
                    .iter()
                    .all(|event| event.observation.is_none()),
                "damaged member {member}: {bytes}"
            );
            assert_eq!(
                result.canonical_receipt.observation.native_release_sequence,
                Some(last)
            );
            assert_eq!(
                fixture.store.load_canonical().unwrap(),
                vec![fixture.message.clone()]
            );
            assert_eq!(fixture.store.presentation_frontier().unwrap(), before + 4);
            let state = final_readiness(&fixture.store);
            assert!(state.0.iter().all(|(_, _, direct)| !direct));
            assert!(state.1.iter().all(|(_, ready)| !ready));
        }
    }
}

#[test]
fn final_model_missing_pending_receipt_marks_the_entire_adoption_family_unavailable() {
    let fixture = FinalFixture::new();
    fixture.store.accept_inbound(draft("selected")).unwrap();
    let selected = fixture.store.select_pending_batch().unwrap().unwrap();
    let remaining = fixture
        .store
        .accept_inbound(draft("retained pending"))
        .unwrap();
    fixture.store.lock().unwrap().execute_batch(
        "CREATE TRIGGER damage_final_pending AFTER UPDATE OF observation_witness ON events WHEN json_extract(NEW.event_json,'$.event.type')='inbound_turn_adopted' BEGIN UPDATE pending_inbound SET observation_header=x'7b'; END;"
    ).unwrap();
    let result = fixture.settle(Some(selected.watermark)).unwrap();
    let shared = result.provider.observation.as_ref().unwrap();
    assert!(shared.source_unavailable);
    assert!(shared.pending.is_none());
    assert!(
        final_family_events(&result)
            .iter()
            .all(|event| Arc::ptr_eq(shared, event.observation.as_ref().unwrap()))
    );
    assert!(
        result
            .boundary
            .as_ref()
            .unwrap()
            .adopted
            .as_ref()
            .unwrap()
            .pending_observation
            .is_none()
    );
    assert_eq!(
        result.canonical_receipt.observation.native_release_sequence,
        shared.journal_range.map(|(_, last)| last)
    );
    let pending = fixture.store.load_pending().unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].message_id, remaining.message_id);
    assert_eq!(fixture.store.load_canonical().unwrap().len(), 2);
    let state = final_readiness(&fixture.store);
    assert!(state.0.iter().all(|(_, _, direct)| !direct));
    assert!(state.1.iter().all(|(_, ready)| !ready));
}

#[test]
#[allow(clippy::too_many_lines)] // Keep shared TEMP release, rollback, and independent restart baseline in one scenario.
fn final_model_family_release_is_one_transaction_and_failure_rolls_back_every_flag() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("final-family-release.sqlite");
    let conversation = ConversationId::generate();
    let fixture = FinalFixture::with_store(
        SqliteConversationStore::open(conversation.clone(), &path).unwrap(),
    );
    // Runtime presentation handles share their writer's TEMP publication epoch.
    let reader = Arc::new(SqliteConversationStore::from_connection_owner(
        conversation.clone(), fixture.store.conn.clone(),
    ));
    let presentation = fixture.store.presentation_reader();
    let result = fixture.settle(None).unwrap();
    let shared = assert_final_shared_family(&result);
    let original = final_readiness(&reader);
    let (first, last) = shared.journal_range.unwrap();
    assert_eq!(presentation.observation_ready_frontier(first - 1, last).unwrap(), first - 1);
    // An independently opened owner starts from all committed native history,
    // without inheriting this runtime's direct-delivery claims or TEMP rows.
    let independent = Arc::new(SqliteConversationStore::open_existing(conversation, &path).unwrap());
    let independent_baseline = final_readiness(&independent);
    assert_eq!(independent_baseline, (Vec::new(), Vec::new()));
    assert_eq!(independent.observation_ready_frontier(first - 1, last).unwrap(), last);
    assert!(!independent.observation_event_witness(last, last).unwrap().unwrap().native_direct);
    fixture.store.lock().unwrap().execute_batch(&format!(
        "CREATE TEMP TRIGGER refuse_final_release BEFORE UPDATE OF direct ON event_observation_readiness WHEN NEW.sequence={last} BEGIN SELECT RAISE(FAIL,'refuse final family release'); END;"
    )).unwrap();
    assert!(fixture.store.mark_native_observation_ready(shared).is_err());
    assert_eq!(final_readiness(&reader), original);
    assert_eq!(final_readiness(&fixture.store), original);
    assert_eq!(presentation.observation_ready_frontier(first - 1, last).unwrap(), first - 1);
    assert_eq!(final_readiness(&independent), independent_baseline);
    assert_eq!(
        fixture.store.load_canonical().unwrap(),
        vec![fixture.message.clone()]
    );
    fixture
        .store
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER refuse_final_release")
        .unwrap();
    let snapshots = Arc::new(Mutex::new(Vec::new()));
    fixture
        .store
        .lock()
        .unwrap()
        .commit_hook(Some({
            let independent = independent.clone();
            let snapshots = snapshots.clone();
            move || {
                // The shared owner is held by this COMMIT; inspect a separate
                // connection here instead of recursively locking the writer.
                snapshots.lock().unwrap().push(final_readiness(&independent));
                false
            }
        }))
        .unwrap();
    fixture.store.mark_native_observation_ready(shared).unwrap();
    fixture
        .store
        .lock()
        .unwrap()
        .commit_hook(None::<fn() -> bool>)
        .unwrap();
    assert_eq!(
        *snapshots.lock().unwrap(),
        vec![independent_baseline.clone()],
        "one TEMP release COMMIT; independent owner keeps its committed baseline"
    );
    let released = final_readiness(&reader);
    assert_eq!(
        &released.0[..usize::try_from(first - 1).unwrap()],
        &original.0[..usize::try_from(first - 1).unwrap()]
    );
    assert!(
        released
            .0
            .iter()
            .filter(|(sequence, _, _)| *sequence >= first && *sequence <= last)
            .all(|(_, ready, direct)| *ready && *direct)
    );
    assert!(released.1.iter().all(|(_, ready)| *ready));
    assert_eq!(presentation.observation_ready_frontier(first - 1, last).unwrap(), last);
    for sequence in first..=last {
        assert!(presentation.observation_event_witness(sequence, last).unwrap().unwrap().native_direct);
    }
    assert_eq!(final_readiness(&independent), independent_baseline);
    assert!(!independent.observation_event_witness(last, last).unwrap().unwrap().native_direct);
    assert_eq!(
        fixture
            .store
            .observation_ready_frontier(first - 1, last)
            .unwrap(),
        last
    );
    let changes = fixture.store.lock().unwrap().total_changes();
    fixture.store.mark_native_observation_ready(shared).unwrap();
    assert_eq!(fixture.store.lock().unwrap().total_changes(), changes);
}

#[test]
fn final_model_settlement_reads_and_decodes_one_configuration_with_or_without_live_prefix() {
    for live_prefix in [false, true] {
        let mut fixture = FinalFixture::new();
        if live_prefix {
            fixture.store.stage_publication_frames(&fixture.start, &fixture.frames).unwrap();
            fixture.frames[0].sequence = 1;
            fixture.frames[0].payload = PublicationPayload::TerminalOnly;
        }
        let bytes: usize = fixture.store.lock().unwrap().query_row(
            "SELECT octet_length(configuration_json) FROM request_configurations", [], |row| row.get(0),
        ).unwrap();
        native_request::CONFIGURATION_DECODES.with(|work| work.set(0));
        native_request::CONFIGURATION_READ_WORK.with(|work| work.set((0, 0)));
        native_request::CONFIGURATION_BYTE_PROBES.with(|work| work.set(0));
        fixture.settle(None).unwrap();
        assert_eq!(native_request::CONFIGURATION_DECODES.with(std::cell::Cell::get), 1);
        assert_eq!(native_request::CONFIGURATION_READ_WORK.with(std::cell::Cell::get), (1, bytes));
        // Fresh first-write admission adds a validation, but all subsequent
        // native boundaries compare the live SQLite bytes instead of decoding.
        assert_eq!(native_request::CONFIGURATION_BYTE_PROBES.with(std::cell::Cell::get), if live_prefix { 3 } else { 4 });
        assert_eq!(fixture.store.load_canonical().unwrap(), vec![fixture.message.clone()]);
    }
}

#[test]
fn final_model_settlement_configuration_trigger_rejects_even_coherent_corruption_and_rolls_back() {
    for coherent in [false, true] {
        let fixture = FinalFixture::new();
        let original = fixture.store.load_request_snapshot(&fixture.start.request_id).unwrap();
        let frontier = fixture.store.presentation_frontier().unwrap();
        let original_cells: (String, String, String, String) = {
            let connection = fixture.store.lock().unwrap();
            let (configuration_digest, configuration, metadata, manifest): (String, String, String, String) = connection.query_row(
                "SELECT c.digest,c.configuration_json,r.snapshot_json,r.manifest_digest FROM request_snapshots r JOIN request_configurations c ON c.digest=json_extract(r.snapshot_json,'$.configuration_digest') WHERE r.request_id=?1",
                [fixture.start.request_id.as_str()], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)),
            ).unwrap();
            let invalid = r#"{"effective_system_prompt":false,"system_sections":[],"tool_definitions":[]}"#;
            let mut hash = Sha256::new();
            hash.update(b"rustx-native-request-configuration-v1\0");
            hash.update(invalid.as_bytes());
            let digest = if coherent { format!("{:x}", hash.finalize()) } else { configuration_digest.clone() };
            let mut changed: serde_json::Value = serde_json::from_str(&metadata).unwrap();
            changed["configuration_digest"] = serde_json::json!(digest);
            let changed = serde_json::to_string(&changed).unwrap();
            let root = read_stored_snapshot_root(&connection, &fixture.start.request_id).unwrap();
            let changed_manifest = stored_snapshot_digest(&changed, root);
            connection.execute_batch(
                "CREATE TABLE invalid_configuration_trigger(digest TEXT,payload TEXT,metadata TEXT,manifest TEXT);
                 CREATE TRIGGER invalidate_final_configuration AFTER INSERT ON events WHEN json_extract(NEW.event_json,'$.event.type')='model_request_completed' BEGIN
                   UPDATE request_configurations SET digest=(SELECT digest FROM invalid_configuration_trigger),configuration_json=(SELECT payload FROM invalid_configuration_trigger);
                   UPDATE request_snapshots SET snapshot_json=(SELECT metadata FROM invalid_configuration_trigger),manifest_digest=(SELECT manifest FROM invalid_configuration_trigger);
                 END;",
            ).unwrap();
            connection.execute("INSERT INTO invalid_configuration_trigger VALUES(?1,?2,?3,?4)", params![digest,invalid,changed,changed_manifest]).unwrap();
            (configuration_digest, configuration, metadata, manifest)
        };
        assert!(fixture.settle(None).is_err(), "coherent={coherent}");
        assert_eq!(fixture.store.load_request_snapshot(&fixture.start.request_id).unwrap(), original);
        assert_eq!(fixture.store.presentation_frontier().unwrap(), frontier);
        assert!(fixture.store.load_canonical().unwrap().is_empty());
        {
            let connection = fixture.store.lock().unwrap();
            let current: (String, String, String, String) = connection.query_row(
                "SELECT c.digest,c.configuration_json,r.snapshot_json,r.manifest_digest FROM request_snapshots r JOIN request_configurations c ON c.digest=json_extract(r.snapshot_json,'$.configuration_digest') WHERE r.request_id=?1",
                [fixture.start.request_id.as_str()], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)),
            ).unwrap();
            assert_eq!(current, original_cells);
            assert_eq!(connection.query_row("SELECT COUNT(*) FROM publication_streams", [], |row| row.get::<_,u64>(0)).unwrap(), 0);
            assert_eq!(connection.query_row("SELECT completed_sequence FROM request_snapshots WHERE request_id=?1", [fixture.start.request_id.as_str()], |row| row.get::<_,Option<u64>>(0)).unwrap(), None);
            assert_eq!(connection.query_row("SELECT COUNT(*) FROM events WHERE json_extract(event_json,'$.event.type')='model_request_completed'", [], |row| row.get::<_,u64>(0)).unwrap(), 0);
            connection.execute_batch("DROP TRIGGER invalidate_final_configuration").unwrap();
        }
        fixture.settle(None).unwrap();
        assert_eq!(fixture.store.load_canonical().unwrap(), vec![fixture.message.clone()]);
    }
}
