// Inspection consumes finite native heads; originals are read only by selected bounded gestures.
use super::*;
use crate::runtime_client::checkpoint::{CheckpointReadCut, decode_domain_value};
use crate::runtime_client::host::RuntimeClientHost;
use base64::Engine as _;
use std::sync::Arc;

fn inspected(
    store: &dyn ConversationStore,
    cut: &CheckpointReadCut,
    sequence: u64,
) -> TraceInspectionHead {
    let domain = format!("trace.records.{sequence}.detail");
    let page =
        crate::runtime_client::trace_observation::history_page(store, cut, &domain, 0, 8).unwrap();
    assert_eq!(page.read_cut, *cut);
    assert_eq!(page.domain, domain);
    assert_eq!(page.total, 1);
    assert_eq!(page.items[0].body_complete, Some(false));
    assert!(page.items[0].deferred);
    decode_domain_value(page.items[0].value.clone().unwrap()).unwrap()
}

fn tool_fixture(giant: bool) -> (Arc<SqliteConversationStore>, RequestSnapshot, ToolCall, u64) {
    let store = Arc::new(store("conv_19000000-0000-7000-8000-000000000106"));
    tool_fixture_in(store, giant)
}

fn tool_fixture_in(
    store: Arc<SqliteConversationStore>,
    giant: bool,
) -> (Arc<SqliteConversationStore>, RequestSnapshot, ToolCall, u64) {
    start(store.as_ref());
    let mut definition = ModelToolDefinition {
        id: ToolId::new("tool-bash"),
        name: "original frozen tool".into(),
        description: "original schema description".into(),
        input_schema: serde_json::json!({"type":"object","properties":{"command":{"type":"string"}}}),
    };
    if giant {
        definition.description = "original description水🙂".repeat(20_000);
        definition.input_schema =
            serde_json::json!({"original":"schema\n水🙂".repeat(20_000),"exact":u64::MAX});
    }
    let frozen = request_with_tools(store.as_ref(), 0, Some(vec![definition]));
    let mut call = bash_call("original-inspection-call");
    if giant {
        call.arguments =
            serde_json::json!({"command":"original payload水🙂".repeat(20_000),"exact":u64::MAX});
    }
    propose_tool_call(
        store.as_ref(),
        frozen.provisional_message_id.as_str(),
        &call,
        3,
    );
    append(
        store.as_ref(),
        E::ToolExecutionStarted {
            tool_call_id: call.id.clone(),
            tool_id: call.tool_id.clone(),
        },
        4,
    );
    let sequence = page(store.as_ref())
        .records
        .iter()
        .find(|record| record.kind == TraceKind::Tool)
        .unwrap()
        .position
        .sequence()
        .unwrap();
    (store, frozen, call, sequence)
}

#[test]
fn t08_t14_tool_schema_unavailable_preserves_own_lifecycle_and_original_fields() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("inspection-schema-hole.sqlite");
    let store = Arc::new(SqliteConversationStore::open(ConversationId::generate(), &path).unwrap());
    store.initialize(&[]).unwrap();
    let (store, _, _, sequence) = tool_fixture_in(store, false);
    let cut = TraceProjection::new(store.as_ref())
        .unwrap()
        .history_cut()
        .unwrap();
    let before = inspected(store.as_ref(), &cut, sequence);
    assert!(matches!(
        &before.content,
        TraceInspectionContent::Tool {
            definition: TraceToolDefinitionSelection::Available { .. }
        }
    ));
    let original = store.load_canonical().unwrap();
    let journal = serde_json::to_vec(&store.read_events(Some(0), 32).unwrap().events).unwrap();
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute(
            "UPDATE canonical_tool_calls SET observation_definition_ready=0",
            [],
        )
        .unwrap();
    let after = inspected(store.as_ref(), &cut, sequence);
    assert!(matches!(
        &after.content,
        TraceInspectionContent::Tool {
            definition: TraceToolDefinitionSelection::Unavailable
        }
    ));
    assert_eq!(after.record, before.record);
    assert!(!after.record.tool.as_ref().unwrap().fields.is_empty());
    assert_eq!(
        store.observation_read_cut().unwrap(),
        cut.source_cut.clone().unwrap()
    );
    assert_eq!(store.load_canonical().unwrap(), original);
    assert_eq!(
        serde_json::to_vec(&store.read_events(Some(0), 32).unwrap().events).unwrap(),
        journal
    );
}

fn selected_value(
    store: &dyn ConversationStore,
    cut: &CheckpointReadCut,
    field: &TraceFieldSource,
) -> Vec<u8> {
    let TraceFieldSource::Value {
        domain,
        item_offset,
        path,
        encoded_bytes,
        ..
    } = field
    else {
        panic!("original indexed value")
    };
    let mut bytes = Vec::new();
    let mut offset = 0;
    loop {
        SqliteConversationStore::content_read_work();
        let chunk = crate::runtime_client::trace_observation::history_value_read(
            store,
            cut,
            domain,
            *item_offset,
            path,
            (offset, 65_536),
        )
        .unwrap();
        let (rows, read) = SqliteConversationStore::content_read_work();
        assert!(rows <= 2 && read <= 65_536);
        assert_eq!(chunk.cut, *cut);
        assert_eq!(chunk.readable_end, *encoded_bytes);
        let part = base64::engine::general_purpose::STANDARD
            .decode(chunk.bytes_base64)
            .unwrap();
        assert!(!part.is_empty() && part.len() <= 65_536);
        bytes.extend(part);
        let Some(next) = chunk.next_offset else { break };
        assert!(next > offset);
        offset = next;
    }
    assert_eq!(bytes.len() as u64, *encoded_bytes);
    bytes
}

fn definition(head: &TraceInspectionHead) -> &TraceFieldSource {
    let TraceInspectionContent::Tool {
        definition: TraceToolDefinitionSelection::Available { field },
    } = &head.content
    else {
        panic!("positive original frozen definition: {:?}", head.content)
    };
    field
}

fn canonical_blocks(
    store: &dyn ConversationStore,
    cut: &CheckpointReadCut,
    message: &TraceInspectionMessage,
) {
    let frontier = message
        .collections
        .iter()
        .find(|frontier| frontier.domain.ends_with(".blocks"))
        .unwrap();
    let actual =
        crate::runtime_client::trace_observation::history_page(store, cut, &frontier.domain, 0, 8)
            .unwrap();
    let original = store
        .message_block_header_page_by_locator(
            cut.source_cut.as_ref().unwrap(),
            &message.locator,
            0,
            8,
        )
        .unwrap();
    assert_eq!(actual.read_cut, *cut);
    assert_eq!(actual.total, original.total);
    assert_eq!(actual.items.len(), original.headers.len());
    assert_eq!(actual.next_offset, original.next_offset);
    for (actual, original) in actual.items.iter().zip(original.headers) {
        assert_eq!(actual.value, original.value);
        assert_eq!(actual.content, original.content);
        assert_eq!(actual.deferred, original.deferred);
    }
}

fn selected_content_bytes(
    host: &RuntimeClientHost,
    binding: &crate::runtime_client::checkpoint::ContentBinding,
) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut offset = 0;
    loop {
        SqliteConversationStore::content_read_work();
        let chunk = host
            .inner
            .content_read(&binding.content_ref, offset, 65_536)
            .unwrap();
        let (rows, read) = SqliteConversationStore::content_read_work();
        assert!(rows <= 2 && read <= 65_536);
        assert_eq!(chunk.content_ref, binding.content_ref);
        assert_eq!(chunk.offset, offset);
        assert_eq!(chunk.readable_end, binding.content_ref.readable_end);
        let part = base64::engine::general_purpose::STANDARD
            .decode(chunk.data_base64)
            .unwrap();
        assert!(!part.is_empty() && part.len() <= 65_536);
        bytes.extend(part);
        assert_eq!(chunk.end_offset, bytes.len() as u64);
        let Some(next) = chunk.next_offset else { break };
        assert!(next > offset);
        offset = next;
    }
    assert_eq!(bytes.len() as u64, binding.content_ref.readable_end);
    bytes
}

fn selected_content(host: &RuntimeClientHost, field: &TraceFieldSource) -> serde_json::Value {
    let TraceFieldSource::Content { binding, .. } = field else {
        panic!("original canonical content")
    };
    let bytes = selected_content_bytes(host, binding);
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    binding.source_path.as_ref().map_or(value.clone(), |path| {
        path.iter()
            .fold(&value, |value, field| &value[field])
            .clone()
    })
}

fn original_result(
    store: &dyn ConversationStore,
    host: &RuntimeClientHost,
    cut: &CheckpointReadCut,
    head: &TraceInspectionHead,
    expected: &ToolExecutionResult,
) {
    let field = head
        .record
        .fields
        .iter()
        .find(|field| {
            matches!(
                field,
                TraceFieldSource::Content {
                    field: TraceFieldKind::Result,
                    ..
                }
            )
        })
        .unwrap();
    let mut result = selected_content(host, field);
    let mut expected_metadata = serde_json::to_value(expected).unwrap();
    expected_metadata.as_object_mut().unwrap().remove("content");
    assert_eq!(
        result, expected_metadata,
        "metadata excludes only content blocks"
    );
    let frontier = head
        .record
        .collections
        .iter()
        .find(|frontier| frontier.domain.ends_with(".blocks"))
        .unwrap();
    assert_eq!(frontier.total, expected.content.len() as u64);
    let mut content = Vec::new();
    let mut offset = 0;
    loop {
        let page = crate::runtime_client::trace_observation::history_page(
            store,
            cut,
            &frontier.domain,
            offset,
            8,
        )
        .unwrap();
        assert_eq!(page.read_cut, *cut);
        assert_eq!(page.total, frontier.total);
        for item in page.items {
            assert_eq!(item.offset, content.len() as u64);
            let block = original_text_block(host, &item);
            assert_eq!(
                block,
                serde_json::to_value(&expected.content[content.len()]).unwrap()
            );
            content.push(block);
        }
        let Some(next) = page.next_offset else { break };
        assert!(next > offset);
        offset = next;
    }
    assert_eq!(content.len() as u64, frontier.total);
    result["content"] = content.into();
    assert_eq!(result, serde_json::to_value(expected).unwrap());
}

fn original_text_block(
    host: &RuntimeClientHost,
    item: &crate::runtime_client::checkpoint::CheckpointPageItem,
) -> serde_json::Value {
    use crate::runtime_client::content::{
        CanonicalContentComponent, ContentRepresentation, ContentSource,
    };
    assert_eq!(item.identity, "text");
    assert!(item.deferred && item.value.is_none());
    let metadata = item
        .content
        .iter()
        .find(|binding| binding.path == ["value"])
        .unwrap();
    let payload = item
        .content
        .iter()
        .find(|binding| binding.path == ["value", "text"])
        .unwrap();
    assert_eq!(
        payload.content_ref.representation,
        ContentRepresentation::Utf8Text
    );
    let ContentSource::CanonicalMessageBlock {
        component,
        locator,
        block_index,
    } = &payload.content_ref.source
    else {
        panic!("original canonical payload")
    };
    assert_eq!(*component, CanonicalContentComponent::Payload);
    assert_eq!(u64::from(block_index.get()), item.offset);
    assert_eq!(
        metadata.content_ref.source,
        ContentSource::CanonicalMessageBlock {
            component: CanonicalContentComponent::BlockMetadata,
            locator: locator.clone(),
            block_index: *block_index,
        }
    );
    let mut block: serde_json::Value =
        serde_json::from_slice(&selected_content_bytes(host, metadata)).unwrap();
    assert_eq!(block, serde_json::json!({"type":"text"}));
    let text = String::from_utf8(selected_content_bytes(host, payload)).unwrap();
    block["text"] = text.into();
    block
}

#[test]
fn t08_t14_t19_tool_inspection_has_frozen_schema_and_bounded_original_ranges() {
    for giant in [false, true] {
        let (store, frozen, call, sequence) = tool_fixture(giant);
        let host = RuntimeClientHost::new_durable(store.clone(), None).unwrap();
        let checkpoint = host.checkpoint().unwrap();
        let cut = &checkpoint.read_cut;
        let head = inspected(store.as_ref(), cut, sequence);
        assert_eq!(head.kind, TraceKind::Tool);
        assert_eq!(
            head.record.tool.as_ref().unwrap().call_id,
            Some(call.id.clone())
        );
        assert_eq!(
            head.record.tool.as_ref().unwrap().name.as_deref(),
            Some("bash")
        );
        let actual: serde_json::Value =
            serde_json::from_slice(&selected_value(store.as_ref(), cut, definition(&head)))
                .unwrap();
        let original = serde_json::to_value(frozen_request_detail(
            &frozen,
            Vec::<TraceRequestMessage>::new(),
        ))
        .unwrap();
        assert_eq!(actual, original["tools"][0]);
        let arguments = head
            .record
            .tool
            .as_ref()
            .unwrap()
            .fields
            .iter()
            .find(|field| {
                matches!(
                    field,
                    TraceFieldSource::Content {
                        field: TraceFieldKind::Arguments,
                        ..
                    }
                )
            })
            .unwrap();
        assert_eq!(selected_content(&host, arguments), call.arguments);
        let domain = format!("trace.records.{sequence}.detail");
        let root = host.inner.checkpoint_page(cut, &domain, 0, 8).unwrap();
        let native: TraceInspectionHead =
            decode_domain_value(root.items[0].value.clone().unwrap()).unwrap();
        assert_eq!(native, head);
        assert_eq!(
            host.checkpoint().unwrap(),
            checkpoint,
            "inspection is a pure read"
        );
        assert!(
            crate::runtime_client::trace_observation::history_value_read(
                store.as_ref(),
                cut,
                &domain,
                0,
                &[],
                (0, 64)
            )
            .is_err(),
            "a finite head never advertises a complete old Tool detail body"
        );
        let mut foreign = cut.clone();
        foreign.conversation_id = ConversationId::generate();
        assert!(
            crate::runtime_client::trace_observation::history_page(
                store.as_ref(),
                &foreign,
                &domain,
                0,
                8
            )
            .is_err()
        );
    }
}

fn inspection_result() -> ToolExecutionResult {
    ToolExecutionResult {
        status: ToolExecutionStatus::Success,
        content: vec![ToolResultContent::Text(TextBlock {
            text: "original result水🙂".repeat(20_000),
        })],
        duration_ms: 25,
        exit_code: Some(0),
        artifacts: vec![crate::message::content::FileReference {
            artifact_id: ArtifactId::new("inspection-artifact"),
            name: Some("original.txt".into()),
            mime_type: Some("text/plain".into()),
            description: None,
        }],
        deliveries: vec![],
        truncation: None,
        workflow: None,
        managed_output: None,
    }
}

#[test]
fn t08_t14_t19_tool_inspection_keeps_c_without_j_and_old_cut_after_terminal() {
    let (store, _, call, sequence) = tool_fixture(false);
    let before = TraceProjection::new(store.as_ref())
        .unwrap()
        .history_cut()
        .unwrap();
    let old = inspected(store.as_ref(), &before, sequence);
    assert_eq!(old.record.state, TraceState::Incomplete);
    assert!(old.record.tool.as_ref().unwrap().outcome.is_none());
    let result = inspection_result();
    let proposal = store
        .load_canonical()
        .unwrap()
        .into_iter()
        .find_map(|message| match message {
            MessageBlock::Assistant(message) => Some(message.id),
            _ => None,
        })
        .unwrap();
    store
        .append_canonical(&MessageBlock::Tool(ToolMessageBlock {
            id: MessageId::new("inspection-result"),
            occurrence: ToolCallOccurrenceRef::new(proposal, ContentBlockIndex::new(0)),
            tool_call_id: call.id.clone(),
            tool_id: call.tool_id.clone(),
            result: result.clone(),
        }))
        .unwrap();
    let after = TraceProjection::new(store.as_ref())
        .unwrap()
        .history_cut()
        .unwrap();
    assert_eq!(after.through, before.through);
    assert!(
        after.source_cut.as_ref().unwrap().ledger_through
            > before.source_cut.as_ref().unwrap().ledger_through
    );
    let committed = inspected(store.as_ref(), &after, sequence);
    assert_eq!(
        committed.record.state,
        TraceState::Incomplete,
        "canonical result does not invent ToolCompleted J"
    );
    assert_eq!(
        committed.record.tool.as_ref().unwrap().outcome,
        Some(TraceToolOutcome::Success)
    );
    assert_eq!(
        committed.record.attachments[0].artifact_id,
        ArtifactId::new("inspection-artifact")
    );
    assert!(committed.record.fields.iter().any(|field| matches!(
        field,
        TraceFieldSource::Content {
            field: TraceFieldKind::Result,
            ..
        }
    )));
    let host = RuntimeClientHost::new_durable(store.clone(), None).unwrap();
    original_result(store.as_ref(), &host, &after, &committed, &result);
    append(
        store.as_ref(),
        E::ToolExecutionCompleted {
            tool_call_id: call.id,
            tool_id: call.tool_id,
            result: result.clone(),
        },
        5,
    );
    assert_eq!(inspected(store.as_ref(), &before, sequence), old);
    assert_eq!(inspected(store.as_ref(), &after, sequence), committed);
    original_result(store.as_ref(), &host, &after, &committed, &result);
    let terminal = TraceProjection::new(store.as_ref())
        .unwrap()
        .history_cut()
        .unwrap();
    assert_eq!(
        inspected(store.as_ref(), &terminal, sequence).record.state,
        TraceState::Completed
    );
}

#[test]
fn t08_t14_trace_assistant_compaction_and_lifecycle_heads_use_their_own_kind() {
    let (store, _, _, _) = tool_fixture(false);
    append(store.as_ref(), E::CompactionStarted, 5);
    let projection = TraceProjection::new(store.as_ref()).unwrap();
    let cut = projection.history_cut().unwrap();
    let records = projection.page(None, 32).unwrap().records;
    assert!(
        records
            .iter()
            .any(|record| record.kind == TraceKind::Assistant)
    );
    assert!(
        records
            .iter()
            .any(|record| record.kind == TraceKind::Compaction)
    );
    for record in records {
        let domain = format!(
            "trace.records.{}.detail",
            record.position.sequence().unwrap()
        );
        if record.kind == TraceKind::Request {
            let page = crate::runtime_client::trace_observation::history_page(
                store.as_ref(),
                &cut,
                &domain,
                0,
                8,
            )
            .unwrap();
            assert_eq!(page.items[0].value.as_ref().unwrap()["kind"], "request");
            continue;
        }
        let head = inspected(store.as_ref(), &cut, record.position.sequence().unwrap());
        assert_eq!(head.id, record.id);
        assert_eq!(head.kind, record.kind);
        assert_eq!(head.record, record);
        if record.kind == TraceKind::Assistant {
            let TraceInspectionContent::Messages { page } = head.content else {
                panic!("original Assistant C")
            };
            assert_eq!(page.total, 1);
            assert_eq!(page.items.len(), 1);
            canonical_blocks(store.as_ref(), &cut, &page.items[0]);
            assert!(
                page.items[0]
                    .collections
                    .iter()
                    .any(|frontier| frontier.domain.ends_with(".blocks"))
            );
        } else if record.kind == TraceKind::Compaction {
            let TraceInspectionContent::Messages { page } = head.content else {
                panic!("Compaction at original cut")
            };
            assert_eq!(
                page.total, 0,
                "unsettled Compaction does not invent summary C"
            );
        }
    }
}

#[test]
fn t08_t14_t19_all_advertised_canonical_kinds_have_advancing_inspection_heads() {
    for human in [true, false] {
        let store = store("conv_19000000-0000-7000-8000-000000000107");
        for ordinal in 0..19 {
            store
                .accept_inbound(crate::durable::InboundDraft {
                    message_id: Some(MessageId::new(format!("inspection-adopted-{ordinal}"))),
                    source: if human {
                        UserSource::Human
                    } else {
                        UserSource::Runtime
                    },
                    kind: InboundKind::Message,
                    content: vec![UserContentBlock::Text(TextBlock {
                        text: format!("original adopted {ordinal}"),
                    })],
                    timestamp: timestamp(ordinal),
                    correlation: None,
                })
                .unwrap();
        }
        let pending = store.select_pending_batch().unwrap().unwrap();
        store.adopt_pending_batch(pending.watermark, None).unwrap();
        let projection = TraceProjection::new(&store).unwrap();
        let cut = projection.history_cut().unwrap();
        let record = projection.page(None, 32).unwrap().records.remove(0);
        assert_eq!(
            record.kind,
            if human {
                TraceKind::User
            } else {
                TraceKind::Context
            }
        );
        let sequence = record.position.sequence().unwrap();
        let head = inspected(&store, &cut, sequence);
        let TraceInspectionContent::Messages { page } = head.content else {
            panic!("original adoption")
        };
        assert_eq!(page.total, 19);
        assert_eq!(page.items.len(), 8);
        assert_eq!(page.next_offset, Some(8));
        let mut offset = 0;
        let mut identities = Vec::new();
        loop {
            let page = crate::runtime_client::trace_observation::history_page(
                &store,
                &cut,
                &page.domain,
                offset,
                8,
            )
            .unwrap();
            assert_eq!(page.read_cut, cut);
            for item in page.items {
                let value: TraceInspectionMessage =
                    decode_domain_value(item.value.unwrap()).unwrap();
                canonical_blocks(&store, &cut, &value);
                identities.push(value.identity.unwrap());
            }
            let Some(next) = page.next_offset else { break };
            assert!(next > offset);
            offset = next;
        }
        assert_eq!(
            identities,
            (0..19)
                .map(|ordinal| MessageId::new(format!("inspection-adopted-{ordinal}")))
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn t11_t19_trace_and_journal_sources_keep_native_sequence_above_safe_byte_coordinates() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory
        .path()
        .join("native-large-journal-sequence.sqlite");
    let store = SqliteConversationStore::open(ConversationId::generate(), &path).unwrap();
    store.initialize(&[]).unwrap();
    let previous = 9_007_199_254_740_991_i64;
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute("UPDATE rustx_store SET next_event_sequence=?1", [previous])
        .unwrap();
    let started = store
        .append_event(event(
            &store,
            E::AttemptStarted {
                attempt_id: AttemptId::new("attempt-a"),
            },
            0,
        ))
        .unwrap();
    let terminal = store
        .append_event(event(
            &store,
            E::AttemptCompleted {
                attempt_id: AttemptId::new("attempt-a"),
                finish_reason: ModelFinishReason::Stop,
            },
            1,
        ))
        .unwrap();
    assert_eq!(started.sequence, u64::try_from(previous).unwrap() + 1);
    assert_eq!(terminal.sequence, started.sequence + 1);
    let projection = TraceProjection::new(&store).unwrap();
    let cut = projection.history_cut().unwrap();
    let record_id = format!("trace:{}", started.sequence);
    let domain = crate::runtime_client::trace_observation::detail_domain(&record_id).unwrap();
    let page =
        crate::runtime_client::trace_observation::history_detail(&store, &cut, &record_id).unwrap();
    assert_eq!(page.domain, domain);
    let head: TraceInspectionHead =
        decode_domain_value(page.items[0].value.clone().unwrap()).unwrap();
    assert_eq!(head.id, record_id);
    assert_eq!(head.record.state, TraceState::Completed);
    let field = head
        .record
        .fields
        .iter()
        .find(|field| {
            matches!(
                field,
                TraceFieldSource::Value {
                    field: TraceFieldKind::Result,
                    ..
                }
            )
        })
        .unwrap();
    assert_eq!(
        selected_value(&store, &cut, field),
        serde_json::to_vec(&ModelFinishReason::Stop).unwrap()
    );
    assert!(crate::runtime_client::trace_observation::detail_domain("trace:0").is_err());
    assert!(crate::runtime_client::journal_observation::selection("journal.0").is_none());
    assert!(
        crate::runtime_client::trace_observation::history_value_read(
            &store,
            &cut,
            &format!("journal.{}", terminal.sequence),
            0,
            &["event".into(), "finish_reason".into()],
            (9_007_199_254_740_992, 1)
        )
        .is_err(),
        "byte coordinates retain their independent safe bound"
    );
}
