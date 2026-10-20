// Original J/C/request bodies are an independent oracle, never default read input.
fn trace_raw_seed() -> (LineageSeed, Vec<u8>) {
    use crate::durable::inbox::CanonicalRawToolArguments;
    use crate::message::types::ContentBlockIndex;
    let raw = format!(
        " {{\"n\":1.0000,{}\"integer\":18446744073709551615,\"escaped\":\"\\u6c34\"}} \n",
        " ".repeat(70_000)
    )
    .into_bytes();
    let message = MessageBlock::Assistant(AssistantMessageBlock {
        id: MessageId::new("trace-original-raw-proposal"),
        content: vec![AssistantContentBlock::ToolCall(ToolCall {
            id: ToolCallId::new("trace-original-raw-call"),
            tool_id: ToolId::new("trace-original-raw-tool"),
            name: "Original raw".into(),
            arguments: serde_json::from_slice(&raw).unwrap(),
        })],
    });
    let seed = LineageSeed::history(vec![message])
        .with_raw_tool_arguments(vec![CanonicalRawToolArguments {
            message_id: MessageId::new("trace-original-raw-proposal"),
            block_index: ContentBlockIndex::new(0),
            bytes: raw.clone(),
        }])
        .unwrap();
    (seed, raw)
}
fn trace_raw_occurrence() -> crate::events::types::ToolObservationOccurrence {
    crate::events::types::ToolObservationOccurrence {
        tool_ordinal: 0,
        ledger_position: 1,
        block_index: crate::message::types::ContentBlockIndex::new(0),
    }
}
fn trace_arguments_reference(
    fields: &[crate::observation::trace::TraceFieldSource],
) -> crate::observation::content::ContentRef {
    use crate::observation::trace::{TraceFieldKind, TraceFieldSource};
    fields
        .iter()
        .find_map(|field| match field {
            TraceFieldSource::Content {
                field: TraceFieldKind::Arguments,
                binding,
            } => Some(binding.content_ref.clone()),
            _ => None,
        })
        .expect("original arguments field")
}
fn trace_original_journal_field_bytes(
    store: &SqliteConversationStore,
    cut: &crate::observation::cut::CheckpointReadCut,
    domain: &str,
    path: &[String],
) -> Vec<u8> {
    use base64::Engine as _;
    let mut result = Vec::new();
    let mut offset = 0;
    loop {
        let chunk = crate::runtime_client::journal_observation::value_read(
            store, cut, domain, 0, path, offset, 65_536,
        )
        .unwrap();
        let (rows, bytes) = SqliteConversationStore::content_read_work();
        assert!(rows <= 2 && bytes <= 65_536);
        result.extend(
            base64::engine::general_purpose::STANDARD
                .decode(chunk.bytes_base64)
                .unwrap(),
        );
        let Some(next) = chunk.next_offset else { break };
        assert!(next > offset);
        offset = next;
    }
    result
}
fn trace_assert_headless_fields(
    store: &SqliteConversationStore,
    cut: &crate::observation::cut::CheckpointReadCut,
    fact: &crate::durable::observation::TraceFactHeader,
    snapshot: &RequestSnapshot,
    first: u64,
) {
    use crate::observation::turn::AttemptIdentitySource;
    use crate::observation::trace::TraceFieldSource;
    let AttemptIdentitySource::Journal { field, domain } =
        fact.location.attempt_id_source.as_ref().unwrap()
    else {
        panic!("actual original RequestStart field")
    };
    assert_eq!(field.sequence, first);
    assert_eq!(field.path, ["attempt_id"]);
    assert_eq!(
        trace_original_journal_field_bytes(store, cut, domain, &field.path),
        serde_json::to_vec(&snapshot.identity.attempt_id).unwrap()
    );
    let TraceFieldSource::Value {
        domain: step_domain,
        path,
        encoded_bytes,
        ..
    } = fact.location.step_id_source.as_ref().unwrap()
    else {
        panic!("actual original Step field")
    };
    assert_eq!(step_domain, domain);
    assert_eq!(path, &["turn_id"]);
    let step_bytes = trace_original_journal_field_bytes(store, cut, step_domain, path);
    assert_eq!(
        step_bytes,
        serde_json::to_vec(&snapshot.identity.turn).unwrap()
    );
    assert_eq!(*encoded_bytes, u64::try_from(step_bytes.len()).unwrap());
    assert!(matches!(
        crate::runtime_client::journal_observation::value_read(
            store,
            cut,
            domain,
            0,
            &["private".into()],
            0,
            32
        ),
        Err(crate::runtime_client::RuntimeClientError::InvalidRequest { .. })
    ));
    let mut foreign = cut.clone();
    foreign.conversation_id = ConversationId::new("conv_601cccec-75c7-703d-af18-cdf2cb42a8c9");
    assert!(matches!(
        crate::runtime_client::journal_observation::value_read(
            store,
            &foreign,
            domain,
            0,
            &field.path,
            0,
            32
        ),
        Err(crate::runtime_client::RuntimeClientError::ReadCutExpired)
    ));
}
#[test]
fn t08_t12_t14_t19_headless_request_keeps_real_giant_owner_fields_without_native_start_facts() {
    use crate::durable::observation::{TraceFactKind, TraceFactQuery, TraceFactScope};
    let store = store();
    store.initialize(&[]).unwrap();
    let mut snapshot = progress_request_snapshot(
        &"a".repeat(70_000),
        &"s".repeat(70_000),
        0,
        store.load_head().unwrap().revision,
        Vec::new(),
    );
    freeze_test_input(&store, &mut snapshot, &[]);
    let first = store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap()
        .started
        .sequence;
    let mut next = progress_request_snapshot(
        snapshot.identity.attempt_id.as_str(),
        snapshot.identity.turn.as_str(),
        1,
        crate::conversation::SurfaceRevision::new(snapshot.canonical_frontier.0),
        Vec::new(),
    );
    freeze_test_input(&store, &mut next, &[]);
    let second = store
        .commit_model_turn_start(&[], &next, Utc::now())
        .unwrap()
        .started
        .sequence;
    let native = store.observation_read_cut().unwrap();
    let cut = crate::runtime_client::trace_observation::history_store_cut(&store).unwrap();
    store
        .lock()
        .unwrap()
        .authorizer(Some(deny_trace_original_columns)).unwrap();
    let query = |kinds| TraceFactQuery {
        scope: TraceFactScope::All,
        kinds,
        before: None,
        after: 0,
        ascending: true,
        limit: 32,
    };
    assert!(
        store
            .observation_trace_facts(
                &native,
                &query(vec![
                    TraceFactKind::AttemptStarted,
                    TraceFactKind::TurnStarted
                ])
            )
            .unwrap()
            .is_empty()
    );
    let facts = store
        .observation_trace_facts(&native, &query(vec![TraceFactKind::ModelRequestStarted]))
        .unwrap();
    assert_eq!(
        facts.iter().map(|fact| fact.sequence).collect::<Vec<_>>(),
        [first, second]
    );
    assert_eq!(facts[0].location, facts[1].location);
    assert!(facts[0].location.attempt_id.is_none() && facts[0].location.step_id.is_none());
    trace_assert_headless_fields(&store, &cut, &facts[0], &snapshot, first);
    store
        .lock()
        .unwrap()
        .authorizer(None::<fn(rusqlite::hooks::AuthContext<'_>) -> rusqlite::hooks::Authorization>).unwrap();
    assert_eq!(store.load_request_snapshot(&next.request_id).unwrap(), next);
}
fn trace_raw_publication_frames(
    start: &PublicationStreamStart,
    call: &ToolCall,
    raw: &[u8],
) -> Vec<PublicationFrame> {
    use crate::tools::types::ToolCallStart;
    [
        PublicationPayload::ProposedToolCallStarted {
            block_index: ContentBlockIndex::new(0),
            call: ToolCallStart {
                id: call.id.clone(),
                tool_id: call.tool_id.clone(),
                name: call.name.clone(),
            },
        },
        PublicationPayload::ProposedToolCallArgumentsSuffix {
            block_index: ContentBlockIndex::new(0),
            call_id: call.id.clone(),
            suffix: std::str::from_utf8(raw).unwrap().into(),
        },
        PublicationPayload::ProposedToolCallCompleted {
            block_index: ContentBlockIndex::new(0),
            call: call.clone(),
        },
    ]
    .into_iter()
    .enumerate()
    .map(|(sequence, payload)| PublicationFrame {
        stream_id: start.stream_id.clone(),
        message_id: start.message_id.clone(),
        sequence: u64::try_from(sequence).unwrap(),
        payload,
    })
    .collect()
}
fn trace_commit_raw_publication(
    store: &SqliteConversationStore,
) -> (MessageBlock, Vec<u8>, RuntimeEventEnvelope) {
    store.initialize(&[]).unwrap();
    let mut snapshot = progress_request_snapshot(
        "trace-publication-owner",
        "1",
        0,
        store.load_head().unwrap().revision,
        Vec::new(),
    );
    freeze_test_input(store, &mut snapshot, &[]);
    store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap();
    let start = PublicationStreamStart {
        stream_id: PublicationStreamId::for_request(
            &snapshot.identity.attempt_id,
            &snapshot.provisional_message_id,
        ),
        message_id: snapshot.provisional_message_id.clone(),
        attempt_id: snapshot.identity.attempt_id.clone(),
        turn_id: snapshot.identity.turn.clone(),
        request_id: snapshot.request_id.clone(),
    };
    store.open_publication_stream(&start).unwrap();
    let (seed, raw) = trace_raw_seed();
    let MessageBlock::Assistant(mut assistant) = seed.canonical()[0].clone() else {
        panic!("original proposal")
    };
    assistant.id.clone_from(&start.message_id);
    let AssistantContentBlock::ToolCall(call) = &assistant.content[0] else {
        panic!("original call")
    };
    store
        .stage_publication_frames(&trace_raw_publication_frames(&start, call, &raw))
        .unwrap();
    let mut outcome = envelope(
        store.conversation_id(),
        "trace-publication-outcome",
        Some(start.attempt_id.clone()),
        RuntimeEvent::ModelRequestCompleted {
            request_id: start.request_id.clone(),
            finish_reason: ModelFinishReason::ToolCalls,
            usage: None,
            generation: None,
        },
    );
    outcome.turn_id = Some(start.turn_id.clone());
    store.append_event(outcome).unwrap();
    store
        .commit_publication_terminal(
            &start.stream_id,
            &[PublicationFrame {
                stream_id: start.stream_id.clone(),
                message_id: start.message_id.clone(),
                sequence: 3,
                payload: PublicationPayload::TerminalOnly,
            }],
        )
        .unwrap();
    let message = MessageBlock::Assistant(assistant);
    let mut event = envelope(
        store.conversation_id(),
        "trace-publication-canonical",
        Some(start.attempt_id),
        RuntimeEvent::AssistantMessageCommitted {
            message_id: start.message_id,
        },
    );
    event.turn_id = Some(start.turn_id);
    let (committed, receipt) = store
        .commit_canonical_publication(&start.stream_id, &message, event)
        .unwrap();
    assert_eq!(receipt.ledger_position, 1);
    assert!(
        store
            .load_unsettled_publication_streams()
            .unwrap()
            .is_empty()
    );
    (message, raw, committed)
}
#[test]
fn t08_t14_t15_t19_trace_arguments_preserve_original_raw_spelling_and_missing_frontier() {
    use crate::observation::content::ContentRepresentation;
    use base64::Engine as _;
    let store = store();
    let (seed, raw) = trace_raw_seed();
    store.initialize_lineage(&seed).unwrap();
    let original = store.load_canonical().unwrap();
    let cut = store.observation_read_cut().unwrap();
    store
        .lock()
        .unwrap()
        .authorizer(Some(deny_trace_original_columns)).unwrap();
    let proposal = store
        .observation_trace_tool_proposal(&cut, &trace_raw_occurrence())
        .unwrap();
    let reference = trace_arguments_reference(&proposal.fields);
    assert_eq!(reference.representation, ContentRepresentation::RawJsonUtf8);
    let canonical = store.observation_trace_canonical_summary(&cut, 1).unwrap();
    assert_eq!(
        trace_arguments_reference(&canonical.calls[0].fields),
        reference
    );
    let mut offset = 0;
    let mut restored = Vec::new();
    loop {
        let chunk = store.content_read(&reference, offset, 65_536).unwrap();
        let (rows, bytes) = SqliteConversationStore::content_read_work();
        assert!(rows <= 2 && bytes <= 65_536);
        restored.extend(
            base64::engine::general_purpose::STANDARD
                .decode(chunk.data_base64)
                .unwrap(),
        );
        let Some(next) = chunk.next_offset else { break };
        assert!(next > offset);
        offset = next;
    }
    assert_eq!(restored, raw);
    store
        .lock()
        .unwrap()
        .authorizer(None::<fn(rusqlite::hooks::AuthContext<'_>) -> rusqlite::hooks::Authorization>).unwrap();
    store.lock().unwrap().execute("DELETE FROM content_frontiers WHERE source_kind='canonical' AND source_id='ledger:1' AND block_index=0 AND representation='raw_json_utf8'", []).unwrap();
    assert!(matches!(
        store.observation_trace_tool_proposal(&cut, &trace_raw_occurrence()),
        Err(ConversationStoreError::Storage(_))
    ));
    assert!(store.content_read(&reference, 0, 65_536).is_err());
    assert_eq!(store.load_canonical().unwrap(), original);
}
#[test]
fn t10_t14_t19_trace_raw_refresh_fault_keeps_native_commit_and_never_falls_back() {
    use crate::durable::observation::{
        TraceFactData, TraceFactKind, TraceFactQuery, TraceFactScope,
    };
    use crate::observation::content::ContentRepresentation;
    let store = store();
    // Initial canonical capture succeeds; only the post-raw refresh fails.
    store.lock().unwrap().execute_batch("CREATE TRIGGER trace_raw_refresh_fault BEFORE UPDATE OF observation_trace_summary ON message_ledger WHEN OLD.observation_trace_summary IS NOT NULL BEGIN SELECT RAISE(FAIL,'optional raw refresh'); END;").unwrap();
    let (seed, raw) = trace_raw_seed();
    store.initialize_lineage(&seed).unwrap();
    let original = store.load_canonical().unwrap();
    let cut = store.observation_read_cut().unwrap();
    let proposal = store
        .observation_trace_tool_proposal(&cut, &trace_raw_occurrence())
        .unwrap();
    assert_eq!(
        trace_arguments_reference(&proposal.fields).representation,
        ContentRepresentation::RawJsonUtf8
    );
    // Original retained bytes prove that absent layout is a source fault,
    // including when the cached pre-raw proposal still names canonical JSON.
    store.lock().unwrap().execute("DELETE FROM content_frontiers WHERE source_kind='canonical' AND source_id='ledger:1' AND block_index=0 AND representation='raw_json_utf8'", []).unwrap();
    assert!(matches!(
        store.observation_trace_tool_proposal(&cut, &trace_raw_occurrence()),
        Err(ConversationStoreError::Storage(_))
    ));
    store
        .lock()
        .unwrap()
        .execute(
            "INSERT INTO content_frontiers VALUES('canonical','ledger:1',0,'raw_json_utf8',1,?1)",
            [u64::try_from(raw.len()).unwrap()],
        )
        .unwrap();
    assert_eq!(store.load_canonical().unwrap(), original);
    let store = self::store();
    store.lock().unwrap().execute_batch("CREATE TRIGGER trace_raw_refresh_fault BEFORE UPDATE OF observation_trace_summary ON message_ledger WHEN OLD.observation_trace_summary IS NOT NULL BEGIN SELECT RAISE(FAIL,'optional raw refresh'); END;").unwrap();
    let (message, _, committed) = trace_commit_raw_publication(&store);
    let fresh = store.observation_read_cut().unwrap();
    let fact = store
        .observation_trace_facts(
            &fresh,
            &TraceFactQuery {
                scope: TraceFactScope::All,
                kinds: vec![TraceFactKind::AssistantMessageCommitted],
                before: None,
                after: 0,
                ascending: true,
                limit: 1,
            },
        )
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(fact.sequence, committed.sequence);
    assert!(matches!(fact.fact, TraceFactData::AssistantCommit { .. }));
    assert_eq!(fact.canonical_fields_ready, Some(false));
    assert!(matches!(
        store.observation_trace_tool_proposal(&fresh, &trace_raw_occurrence()),
        Err(ConversationStoreError::Storage(_))
    ));
    assert_eq!(
        store
            .read_events(Some(committed.sequence - 1), 1)
            .unwrap()
            .events[0]
            .sequence,
        committed.sequence
    );
    assert_eq!(store.load_canonical().unwrap(), vec![message]);
}
#[test]
fn t08_t10_t14_t19_published_trace_raw_fields_bind_after_original_retention() {
    use crate::observation::content::ContentRepresentation;
    let store = store();
    let (message, raw, committed) = trace_commit_raw_publication(&store);
    let cut = store.observation_read_cut().unwrap();
    let bytes: Vec<u8> = store.lock().unwrap().query_row(
        "SELECT observation_trace_proposal FROM canonical_tool_calls WHERE observation_ledger=1 AND block_index=0", [], |row| row.get(0),
    ).unwrap();
    let captured: crate::durable::observation::TraceToolProposalSummary =
        serde_json::from_slice(&bytes).unwrap();
    let reference = trace_arguments_reference(&captured.fields);
    assert_eq!(reference.representation, ContentRepresentation::RawJsonUtf8);
    assert_eq!(reference.readable_end, u64::try_from(raw.len()).unwrap());
    assert!(
        store
            .observation_trace_facts(
                &cut,
                &crate::durable::observation::TraceFactQuery {
                    scope: crate::durable::observation::TraceFactScope::All,
                    kinds: vec![
                        crate::durable::observation::TraceFactKind::AssistantMessageCommitted
                    ],
                    before: None,
                    after: committed.sequence - 1,
                    ascending: true,
                    limit: 1,
                }
            )
            .unwrap()[0]
            .canonical_fields_ready
            .unwrap()
    );
    // Removing the entire original representation cannot turn the retained
    // Raw source selector back into a reserialized canonical fallback.
    store.lock().unwrap().execute("DELETE FROM content_frontiers WHERE source_kind='canonical' AND source_id='ledger:1' AND representation='raw_json_utf8'", []).unwrap();
    store.lock().unwrap().execute("DELETE FROM content_segments WHERE source_kind='canonical' AND source_id='ledger:1' AND representation='raw_json_utf8'", []).unwrap();
    assert!(matches!(
        store.observation_trace_tool_proposal(&cut, &trace_raw_occurrence()),
        Err(ConversationStoreError::Storage(_))
    ));
    assert_eq!(store.load_canonical().unwrap(), vec![message]);
}
fn deny_trace_original_columns(
    context: rusqlite::hooks::AuthContext<'_>,
) -> rusqlite::hooks::Authorization {
    use rusqlite::hooks::{AuthAction, Authorization};
    match context.action {
        AuthAction::Read {
            table_name,
            column_name,
        } if matches!(
            (table_name, column_name),
            (
                "events",
                "event_json" | "attempt_id" | "turn_id" | "event_id"
            ) | (
                "request_snapshots",
                "request_id" | "snapshot_json" | "previous_request_id"
            ) | ("message_ledger", "message_id" | "message_json")
        ) =>
        {
            Authorization::Deny
        }
        _ => Authorization::Allow,
    }
}
fn trace_large_request(store: &SqliteConversationStore) -> (RequestSnapshot, u64, u64) {
    // RequestStart requires each Original model input record <=1 MiB.
    // Keep it hundreds of KiB, independently larger than every display header.
    store
        .initialize(&[user_message(
            "trace-original-input",
            &"canonical🙂水".repeat(30_000),
        )])
        .unwrap();
    let mut snapshot = progress_request_snapshot(
        "trace-original-owner",
        "1",
        0,
        store.load_head().unwrap().revision,
        Vec::new(),
    );
    snapshot.invocation.model = "\u{feff}🙂水".repeat(120_000);
    snapshot.effective_system_prompt = "original prompt\n🙂水".repeat(80_000);
    snapshot.tool_definitions = vec![crate::tools::types::ModelToolDefinition {
        id: ToolId::new("trace-original-tool"),
        name: "Original".into(),
        description: "original schema".repeat(100_000),
        input_schema: serde_json::json!({"opaque":"水".repeat(400_000),"integer":u64::MAX,"zero":-0.0}),
    }];
    freeze_test_input(store, &mut snapshot, &[]);
    let start = store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap()
        .started
        .sequence;
    let mut terminal = envelope(
        store.conversation_id(),
        "trace-original-terminal",
        Some(snapshot.identity.attempt_id.clone()),
        RuntimeEvent::ModelRequestCompleted {
            request_id: snapshot.request_id.clone(),
            finish_reason: ModelFinishReason::Stop,
            usage: Some(crate::model::types::ModelUsage {
                input_tokens: 7,
                output_tokens: 3,
                total_tokens: 10,
                details: None,
            }),
            generation: None,
        },
    );
    terminal.turn_id = Some(snapshot.identity.turn.clone());
    let terminal = store.append_event(terminal).unwrap().sequence;
    (snapshot, start, terminal)
}
#[test]
fn t08_t14_t15_t19_trace_headers_and_scalar_ranges_never_read_growing_original_bodies() {
    use crate::durable::observation::{
        TraceFactData, TraceFactKind, TraceFactQuery, TraceFactScope,
    };
    use base64::Engine as _;
    let store = store();
    let (snapshot, start, terminal) = trace_large_request(&store);
    let cut = store.observation_read_cut().unwrap();
    let original = store.load_request_snapshot(&snapshot.request_id).unwrap();
    assert_eq!(original, snapshot);
    SqliteConversationStore::trace_read_work();
    store
        .lock()
        .unwrap()
        .authorizer(Some(deny_trace_original_columns)).unwrap();
    let facts = store
        .observation_trace_facts(
            &cut,
            &TraceFactQuery {
                scope: TraceFactScope::Owner {
                    anchor_sequence: start,
                },
                kinds: vec![TraceFactKind::ModelRequestCompleted],
                before: None,
                after: start,
                ascending: true,
                limit: 1,
            },
        )
        .unwrap();
    assert_eq!(facts[0].sequence, terminal);
    let TraceFactData::Terminal(outcome) = &facts[0].fact else {
        panic!("original terminal header expected")
    };
    assert_eq!(outcome.usage.as_ref().unwrap().total_tokens, 10);
    let summary = store
        .observation_request_summary_at(start, terminal)
        .unwrap()
        .unwrap();
    assert!(summary.model.is_none());
    assert_eq!(
        summary.prompt_json_bytes,
        observation_request_summary::serialized_bytes(&snapshot.effective_system_prompt).unwrap()
    );
    assert!(
        store
            .observation_request_predecessor_at(start, terminal)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        store
            .observation_request_context_at(&cut, start, 0, 16)
            .unwrap()
            .total,
        0
    );
    let canonical = store.observation_trace_canonical_summary(&cut, 1).unwrap();
    assert!(canonical.preview.as_ref().unwrap().truncated);
    let (rows, bytes) = SqliteConversationStore::trace_read_work();
    assert!(rows <= 4);
    assert!(bytes <= 4 * 16_384);
    let client_cut = crate::runtime_client::trace_observation::history_store_cut(&store).unwrap();
    let chunk = crate::runtime_client::trace_observation::field_read(
        &store,
        &client_cut,
        &format!("trace.records.{start}.detail.request.model"),
        start,
        &["model".into()],
        (0, &[]),
        (0, 3),
    )
    .unwrap();
    let encoded = serde_json::to_vec(&snapshot.invocation.model).unwrap();
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(chunk.bytes_base64)
            .unwrap(),
        encoded[..3]
    );
    assert!(chunk.next_offset.is_some());
    store
        .lock()
        .unwrap()
        .authorizer(None::<fn(rusqlite::hooks::AuthContext<'_>) -> rusqlite::hooks::Authorization>).unwrap();
    assert_eq!(
        store.load_request_snapshot(&snapshot.request_id).unwrap(),
        snapshot
    );
}
#[test]
fn t10_t14_t19_trace_optional_request_relationship_faults_preserve_original_start_and_own_terminal()
{
    use crate::durable::observation::{
        TraceFactData, TraceFactKind, TraceFactQuery, TraceFactScope,
    };
    for column in ["observation_summary", "observation_context_total"] {
        let store = store();
        store.lock().unwrap().execute_batch(&format!("CREATE TRIGGER trace_optional_fault BEFORE UPDATE OF {column} ON request_snapshots BEGIN SELECT RAISE(FAIL,'optional Trace relationship'); END;")).unwrap();
        let (snapshot, start, terminal) = trace_large_request(&store);
        let cut = store.observation_read_cut().unwrap();
        assert_eq!(
            store.load_request_snapshot(&snapshot.request_id).unwrap(),
            snapshot
        );
        let fact = store
            .observation_trace_facts(
                &cut,
                &TraceFactQuery {
                    scope: TraceFactScope::Owner {
                        anchor_sequence: start,
                    },
                    kinds: vec![TraceFactKind::ModelRequestCompleted],
                    before: None,
                    after: start,
                    ascending: true,
                    limit: 1,
                },
            )
            .unwrap()
            .pop()
            .unwrap();
        let TraceFactData::Terminal(outcome) = fact.fact else {
            panic!("original terminal expected")
        };
        assert_eq!(outcome.usage.unwrap().total_tokens, 10);
        let error = if column == "observation_summary" {
            store
                .observation_request_summary_at(start, terminal)
                .unwrap_err()
        } else {
            store
                .observation_request_context_at(&cut, start, 0, 16)
                .unwrap_err()
        };
        assert!(matches!(
            crate::runtime_client::RuntimeClientError::observation_store_error(error),
            crate::runtime_client::RuntimeClientError::ObservationUnavailable { .. }
        ));
    }
}

fn trace_assert_execution_model(
    store: &SqliteConversationStore,
    cut: &crate::durable::observation::ObservationReadCut,
    started: u64,
    request: u64,
    snapshot: &RequestSnapshot,
) {
    let page = store.observation_models_at(cut, started, 0, 32).unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.names[0].source_sequence, request);
    assert!(page.names[0].request_id.is_none());
    assert_eq!(
        page.names[0].request_id_source.as_ref().unwrap().sequence,
        request
    );
    let model = store
        .observation_model_read_at(cut, started, 0, 0, 3)
        .unwrap();
    assert_eq!(
        model.bytes,
        serde_json::to_vec(&snapshot.invocation.model).unwrap()[..3]
    );
    assert!(model.next_offset.is_some());
}
fn trace_full_runtime_failure_bytes(failure: &crate::events::types::AttemptFailure) -> Vec<u8> {
    let crate::events::types::AttemptFailure::Runtime { error } = failure else {
        panic!("original Runtime failure")
    };
    serde_json::to_vec(
        &crate::observation::failure::RuntimeClientAttemptFailure::Runtime {
            error: error.clone(),
        },
    )
    .unwrap()
}
fn trace_original_failure_bytes(
    store: &SqliteConversationStore,
    cut: &crate::durable::observation::ObservationReadCut,
    terminal: u64,
) -> Vec<u8> {
    let mut actual = Vec::new();
    let mut offset = 0;
    loop {
        let part = store
            .observation_failure_read_at(cut, terminal, offset, 65_536)
            .unwrap();
        assert!(part.bytes.len() <= 65_536);
        actual.extend(part.bytes);
        let Some(next) = part.next_offset else { break };
        assert!(next > offset);
        offset = next;
    }
    actual
}
#[test]
fn t08_t14_t15_t19_execution_models_and_failure_use_numeric_original_request_sources() {
    let store = store();
    store
        .initialize(&[user_message("numeric-execution-input", "original")])
        .unwrap();
    let attempt = AttemptId::new("original-request-owner🙂水".repeat(55_000));
    let started = store
        .append_event(envelope(
            store.conversation_id(),
            "numeric-execution-start",
            Some(attempt.clone()),
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt.clone(),
            },
        ))
        .unwrap()
        .sequence;
    let mut turn = envelope(
        store.conversation_id(),
        "numeric-execution-step",
        Some(attempt.clone()),
        RuntimeEvent::TurnStarted,
    );
    turn.turn_id = Some(crate::runtime::identity::TurnId::new("1"));
    store.append_event(turn).unwrap();
    let mut snapshot = progress_request_snapshot(
        attempt.as_str(),
        "1",
        0,
        store.load_head().unwrap().revision,
        Vec::new(),
    );
    snapshot.invocation.model = "\u{feff}model🙂水".repeat(80_000);
    freeze_test_input(&store, &mut snapshot, &[]);
    let request = store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap()
        .started
        .sequence;
    let before = store.observation_read_cut().unwrap();
    let frozen = store.observation_execution_at(&before, started).unwrap();
    assert_eq!(frozen.requests, 1);
    assert!(frozen.failure.is_none());
    assert!(frozen.terminal_sequence.is_none());
    let failure = crate::events::types::AttemptFailure::Runtime {
        error: crate::runtime::types::RuntimeError::Internal {
            message: "\u{feff}original diagnostic\n🙂水".repeat(70_000),
        },
    };
    let expected = trace_full_runtime_failure_bytes(&failure);
    let terminal = store
        .append_event(envelope(
            store.conversation_id(),
            "numeric-execution-failed",
            Some(attempt),
            RuntimeEvent::AttemptFailed {
                attempt_id: snapshot.identity.attempt_id.clone(),
                error: failure,
            },
        ))
        .unwrap()
        .sequence;
    let cut = store.observation_read_cut().unwrap();
    store
        .lock()
        .unwrap()
        .authorizer(Some(deny_trace_original_columns)).unwrap();
    assert_eq!(
        store.observation_execution_at(&before, started).unwrap(),
        frozen
    );
    let summary = store.observation_execution_at(&cut, started).unwrap();
    assert!(summary.last_request_id.is_none());
    assert_eq!(summary.last_request_sequence, Some(request));
    assert_eq!(
        summary.last_request_id_source.as_ref().unwrap().sequence,
        request
    );
    assert!(summary.failure_deferred);
    assert!(summary.failure.is_some());
    assert_eq!(summary.terminal_sequence, Some(terminal));
    trace_assert_execution_model(&store, &cut, started, request, &snapshot);
    let actual = trace_original_failure_bytes(&store, &cut, terminal);
    assert_eq!(actual, expected);
    store
        .lock()
        .unwrap()
        .authorizer(None::<fn(rusqlite::hooks::AuthContext<'_>) -> rusqlite::hooks::Authorization>).unwrap();
    assert_eq!(
        store.load_request_snapshot(&snapshot.request_id).unwrap(),
        snapshot
    );
}

#[test]
fn t10_t14_t19_execution_required_header_fault_preserves_native_terminal_and_frozen_state() {
    let store = store();
    let attempt = AttemptId::new("execution-header-fault");
    let start = store
        .append_event(envelope(
            store.conversation_id(),
            "execution-fault-start",
            Some(attempt.clone()),
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt.clone(),
            },
        ))
        .unwrap()
        .sequence;
    let before = store.observation_read_cut().unwrap();
    let frozen = store.observation_execution_at(&before, start).unwrap();
    store.lock().unwrap().execute_batch("CREATE TRIGGER execution_header_fault BEFORE UPDATE OF observation_execution ON events WHEN NEW.observation_kind='attempt_failed' BEGIN SELECT RAISE(FAIL,'optional execution reading'); END;").unwrap();
    let original = RuntimeEvent::AttemptFailed {
        attempt_id: attempt.clone(),
        error: crate::events::types::AttemptFailure::Runtime {
            error: crate::runtime::types::RuntimeError::Internal {
                message: "actual native failure".into(),
            },
        },
    };
    let receipt = store
        .append_event(envelope(
            store.conversation_id(),
            "execution-fault-terminal",
            Some(attempt),
            original.clone(),
        ))
        .unwrap();
    assert_eq!(receipt.event, original);
    assert!(
        store
            .event_transcript_cursor(&receipt.event_id)
            .unwrap()
            .is_some()
    );
    let after = store.observation_read_cut().unwrap();
    assert_eq!(
        store.observation_execution_at(&before, start).unwrap(),
        frozen
    );
    let error = store.observation_execution_at(&after, start).unwrap_err();
    assert!(matches!(
        crate::runtime_client::RuntimeClientError::observation_store_error(error),
        crate::runtime_client::RuntimeClientError::ObservationUnavailable { .. }
    ));
    assert_eq!(
        store
            .read_events(Some(receipt.sequence - 1), 1)
            .unwrap()
            .events[0]
            .event,
        original
    );
}

fn deny_original_request_body(
    context: rusqlite::hooks::AuthContext<'_>,
) -> rusqlite::hooks::Authorization {
    use rusqlite::hooks::{AuthAction, Authorization};
    match context.action {
        AuthAction::Read {
            table_name: "request_snapshots",
            column_name: "snapshot_json",
        } => Authorization::Deny,
        _ => Authorization::Allow,
    }
}
fn trace_acceptance_request(store: &SqliteConversationStore) -> (RequestSnapshot, u64) {
    store
        .initialize(&[user_message("accepted-source-input", "original")])
        .unwrap();
    let attempt = AttemptId::new("accepted-original-owner🙂水".repeat(55_000));
    store
        .append_event(envelope(
            store.conversation_id(),
            "accepted-owner-start",
            Some(attempt.clone()),
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt.clone(),
            },
        ))
        .unwrap();
    let mut turn = envelope(
        store.conversation_id(),
        "accepted-step-start",
        Some(attempt.clone()),
        RuntimeEvent::TurnStarted,
    );
    turn.turn_id = Some(crate::runtime::identity::TurnId::new("1"));
    store.append_event(turn).unwrap();
    let mut snapshot = progress_request_snapshot(
        attempt.as_str(),
        "1",
        0,
        store.load_head().unwrap().revision,
        Vec::new(),
    );
    snapshot.effective_system_prompt = "unrelated original prompt🙂水".repeat(90_000);
    freeze_test_input(store, &mut snapshot, &[]);
    let start = store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap()
        .started
        .sequence;
    (snapshot, start)
}
#[test]
fn t08_t14_t15_t19_request_acceptance_compares_original_scalar_in_finite_windows() {
    let store = store();
    let (snapshot, start) = trace_acceptance_request(&store);
    let attempt = snapshot.identity.attempt_id.clone();
    let before = store.observation_read_cut().unwrap();
    store
        .lock()
        .unwrap()
        .authorizer(Some(deny_original_request_body)).unwrap();
    observation_trace::accepted_compare_work();
    assert!(
        observation_trace::accepted_identity_equal(
            &store.lock().unwrap(),
            start,
            start,
            snapshot.provisional_message_id.as_str()
        )
        .unwrap()
    );
    let (calls, rows, bytes, total) = observation_trace::accepted_compare_work();
    assert!(calls > 1);
    assert!(rows <= 2);
    assert!(bytes <= 65_536);
    assert_eq!(
        total,
        serde_json::to_vec(&snapshot.provisional_message_id)
            .unwrap()
            .len()
    );
    assert!(
        !observation_trace::accepted_identity_equal(
            &store.lock().unwrap(),
            start,
            start,
            "different-original-provisional"
        )
        .unwrap()
    );
    let canonical = assistant_message(
        snapshot.provisional_message_id.as_str(),
        "Accepted original response",
    );
    let mut committed = envelope(
        store.conversation_id(),
        "accepted-native-c",
        Some(attempt),
        RuntimeEvent::AssistantMessageCommitted {
            message_id: snapshot.provisional_message_id.clone(),
        },
    );
    committed.turn_id = Some(snapshot.identity.turn.clone());
    store
        .append_canonical_with_event(&canonical, committed)
        .unwrap();
    let after = store.observation_read_cut().unwrap();
    assert_eq!(
        store
            .observation_request_accepted_at(&after, start)
            .unwrap(),
        Some(
            crate::observation::content::CanonicalContentLocator::ByLedgerPosition {
                position: 2
            }
        )
    );
    assert!(
        store
            .observation_request_accepted_at(&before, start)
            .unwrap()
            .is_none()
    );
    store
        .lock()
        .unwrap()
        .authorizer(None::<fn(rusqlite::hooks::AuthContext<'_>) -> rusqlite::hooks::Authorization>).unwrap();
    assert_eq!(
        store.load_request_snapshot(&snapshot.request_id).unwrap(),
        snapshot
    );
}

#[test]
fn t08_t12_t19_pending_adoption_expires_full_cut_but_preserves_indexed_original_start() {
    use crate::durable::observation::{TraceFactKind, TraceFactQuery, TraceFactScope};
    use crate::durable::presentation::{FactQuery, FactScope};

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("conversation.sqlite");
    let conversation = ConversationId::generate();
    let writer = SqliteConversationStore::open(conversation.clone(), &path).unwrap();
    writer.initialize(&[]).unwrap();
    let baseline = writer.presentation_frontier().unwrap();
    let owner = AttemptId::new("original-start-across-adoption");
    let original = writer
        .append_event(envelope(
            &conversation,
            "original-start-before-adoption",
            Some(owner.clone()),
            RuntimeEvent::AttemptStarted {
                attempt_id: owner.clone(),
            },
        ))
        .unwrap();
    let accepted = writer
        .accept_inbound(draft("original adopted input"))
        .unwrap();
    let reader = SqliteConversationStore::open_existing(conversation.clone(), &path).unwrap();
    let captured = reader.observation_read_cut().unwrap();
    let trace_query = TraceFactQuery {
        scope: TraceFactScope::All,
        kinds: vec![TraceFactKind::AttemptStarted],
        before: None,
        after: baseline,
        ascending: true,
        limit: 1,
    };
    let first = reader
        .observation_trace_facts(&captured, &trace_query)
        .unwrap();
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].sequence, original.sequence);
    assert_eq!(first[0].location.attempt_id.as_ref(), Some(&owner));
    let original_query = FactQuery {
        scope: FactScope::All,
        kinds: vec!["attempt_started"],
        before: None,
        after: baseline,
        ascending: true,
        through: captured.read.journal,
        limit: 1,
    };
    let before = reader.read_presentation_events(&original_query).unwrap();
    // Force the real writer commit between the two reader calls. Neither
    // handle holds a read transaction across the native adoption boundary.
    let adopted = writer
        .adopt_pending_batch(accepted.sequence, Some(owner.clone()))
        .unwrap();
    assert_eq!(adopted.items.len(), 1);
    assert!(adopted.adoption.unwrap().sequence > captured.read.journal);
    let fresh = reader.observation_read_cut().unwrap();
    assert_eq!(
        fresh.read.mutation_revision,
        captured.read.mutation_revision + 1
    );
    assert_eq!(fresh.read.pending_total, 0);
    assert_eq!(fresh.ledger_through, captured.ledger_through + 1);
    assert!(matches!(
        reader.observation_trace_facts(&captured, &trace_query),
        Err(ConversationStoreError::ObservationCutExpired)
    ));
    let after = reader.read_presentation_events(&original_query).unwrap();
    assert_eq!(after.len(), 1);
    assert_eq!(
        serde_json::to_value(&before).unwrap(),
        serde_json::to_value(&after).unwrap()
    );
    assert_eq!(after[0].sequence, original.sequence);
    assert_eq!(after[0].conversation_id, conversation);
    assert_eq!(after[0].attempt_id.as_ref(), Some(&owner));
    assert!(
        matches!(&after[0].event, RuntimeEvent::AttemptStarted {attempt_id} if attempt_id == &owner)
    );
    let retained = reader
        .observation_trace_facts(&fresh, &trace_query)
        .unwrap();
    assert_eq!(retained.len(), 1);
    assert_eq!(retained[0].sequence, original.sequence);
    assert_eq!(retained[0].location.attempt_id.as_ref(), Some(&owner));
}

#[test]
fn t08_t12_t19_frozen_derived_request_fields_keep_original_bytes_without_canonical_summary() {
    let store = store();
    let original = user_message("projection-original", "immutable native input");
    store.initialize(std::slice::from_ref(&original)).unwrap();
    let body = "\u{feff}derived🙂水\n".repeat(35_000);
    let candidate = crate::scripted_suites::support::context::checkpoint_fixture(
        &store,
        "frozen-source-checkpoint",
        1,
        &body,
    );
    let receipt = store
        .commit_projection(
            crate::scripted_suites::support::context::checkpoint_commit_fixture(
                &store, candidate, None,
            ),
        )
        .unwrap();
    assert!(receipt.newly_committed);
    let history = store.projected_history().unwrap();
    let crate::model::ModelInputMessage::Derived(derived) = &history.messages[0] else {
        panic!("actual checkpoint must expose Derived input");
    };
    let crate::model::projected::ProjectedMessage::User { content, .. } = &derived.message else {
        panic!("typed summary User body");
    };
    let crate::message::types::UserContentBlock::Text(text) = &content[0] else {
        panic!("actual original summary text");
    };
    let expected = text.text.clone();
    let mut snapshot = test_request_snapshot(store.load_head().unwrap().revision, invocation());
    freeze_test_input(&store, &mut snapshot, &[]);
    let started = store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap()
        .started;
    let path = vec![
        "messages".into(),
        "0".into(),
        "blocks".into(),
        "0".into(),
        "text".into(),
        "text".into(),
    ];
    let mut bytes = Vec::new();
    let mut offset = 0;
    loop {
        let chunk = store
            .request_value_read_at(started.sequence, started.sequence, &path, offset, 65_536)
            .unwrap();
        assert!(chunk.bytes.len() <= 65_536);
        bytes.extend(chunk.bytes);
        let Some(next) = chunk.next_offset else { break };
        assert!(next > offset);
        offset = next;
    }
    assert_eq!(serde_json::from_slice::<String>(&bytes).unwrap(), expected);
    let row = store
        .request_value_read_at(
            started.sequence,
            started.sequence,
            &[
                "messages".into(),
                "0".into(),
                "provenance".into(),
                "type".into(),
            ],
            0,
            64,
        )
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<String>(&row.bytes).unwrap(),
        "derived"
    );
    assert_eq!(store.load_canonical().unwrap(), vec![original]);
    let replay = store
        .projection_receipt(&receipt.operation_id, &receipt.candidate_fingerprint)
        .unwrap()
        .unwrap();
    assert!(!replay.newly_committed);
    assert_eq!(replay.completed.event, receipt.completed.event);
    assert_eq!(replay.completed.sequence, receipt.completed.sequence);
}

#[test]
fn t08_t12_t19_native_request_summary_survives_large_original_canonical_input() {
    let store = store();
    let original = user_message(
        "large-request-original",
        &format!("hello from the tui\n{}", "水🙂".repeat(12_000)),
    );
    store.initialize(std::slice::from_ref(&original)).unwrap();
    let mut snapshot = test_request_snapshot(store.load_head().unwrap().revision, invocation());
    freeze_test_input(&store, &mut snapshot, &[]);
    let started = store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap()
        .started;
    let summary = store
        .observation_request_summary_at(started.sequence, started.sequence)
        .unwrap()
        .unwrap();
    assert_eq!(summary.request_id, Some(snapshot.request_id.clone()));
    let page = store
        .request_field_page_at(
            started.sequence,
            started.sequence,
            &["messages".into()],
            0,
            1,
        )
        .unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.fields.len(), 1);
    assert!(!page.fields[0].content.is_empty());
    assert_eq!(page.fields[0].collections[0].loaded, 0);
    assert_eq!(page.fields[0].collections[0].next_offset, Some(0));
    assert!(page.fields[0].deferred);
    assert_eq!(page.fields[0].value["deferred"], true);
    let header = store
        .request_field_header_at(
            started.sequence,
            started.sequence,
            &["messages".into(), "0".into()],
        )
        .unwrap();
    assert!(header.deferred);
    assert_eq!(store.load_canonical().unwrap(), vec![original]);
}
