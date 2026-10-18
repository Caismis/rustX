// The private per-item hook must preserve the old producer's exact headers.
fn header_capture_fixture() -> (SqliteConversationStore, RequestSnapshot) {
    use crate::scripted_suites::support::context::{checkpoint_commit_fixture, checkpoint_fixture};
    let store = store();
    store
        .initialize(&[
            user_message("capture-normal", "original first input"),
            user_message(&"\u{1}".repeat(3_000), "original giant identity"),
            user_message(
                "capture-large-body",
                &"original canonical body水".repeat(12_000),
            ),
        ])
        .unwrap();
    let candidate = checkpoint_fixture(&store, "header-capture", 2, "native Derived summary");
    store
        .commit_projection(checkpoint_commit_fixture(&store, candidate, None))
        .unwrap();
    let mut snapshot = test_request_snapshot(store.load_head().unwrap().revision, invocation());
    freeze_test_input(&store, &mut snapshot, &[]);
    (store, snapshot)
}
fn captured_header_rows(connection: &Connection, sequence: u64) -> Vec<Vec<u8>> {
    let count: u64 = connection
        .query_row(
            "SELECT input_count FROM request_snapshots WHERE started_sequence=?1",
            [sequence],
            |row| row.get(0),
        )
        .unwrap();
    (0..count)
        .map(|ordinal| {
            observation_request::field_from(
                connection,
                "request_observation_fields",
                &sequence.to_string(),
                &["messages".into(), ordinal.to_string()],
            )
            .unwrap()
            .3
        })
        .collect()
}
fn original_provenance_bytes(
    store: &SqliteConversationStore,
    snapshot: &RequestSnapshot,
    sequence: u64,
    item: usize,
) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut offset = 0;
    loop {
        let chunk = store
            .request_value_read(
                &snapshot.request_id,
                sequence,
                &["messages".into(), item.to_string(), "provenance".into()],
                offset,
                65_536,
            )
            .unwrap();
        assert_eq!(chunk.end_offset, offset + chunk.bytes.len() as u64);
        assert!(chunk.bytes.len() <= 65_536);
        bytes.extend_from_slice(&chunk.bytes);
        match chunk.next_offset {
            Some(next) => {
                assert!(next > offset);
                offset = next;
            }
            None => break,
        }
    }
    bytes
}

#[test]
fn t08_t14_t19_request_headers_are_constructed_once_with_exact_legacy_headers_and_raw_provenance() {
    let (store, snapshot) = header_capture_fixture();
    let canonical = store.load_canonical().unwrap();
    observation_request::composed::take_work();
    let started = store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap()
        .started;
    let work = observation_request::composed::take_work();
    assert_eq!(work.new_leaves, snapshot.input.len());
    let headers = captured_header_rows(&store.lock().unwrap(), started.sequence);
    assert_eq!(headers.len(), snapshot.input.len());
    assert!(headers.iter().all(|bytes| bytes.len() <= 16_384));
    let input = store
        .reconstruct_model_request(&snapshot.request_id)
        .unwrap();
    let provenance = (0..snapshot.input.len())
        .map(|item| original_provenance_bytes(&store, &snapshot, started.sequence, item))
        .collect::<Vec<_>>();
    let mut saw_giant_derived = false;
    for (frozen, bytes) in snapshot.input.iter().zip(&provenance) {
        let value: serde_json::Value = serde_json::from_slice(bytes).unwrap();
        match frozen {
            crate::model::projected::FrozenModelInput::Original(original) => {
                assert_eq!(value["source"], serde_json::to_value(original).unwrap());
            }
            crate::model::projected::FrozenModelInput::Derived(original) => {
                assert_eq!(
                    value["sources"],
                    serde_json::to_value(&original.sources).unwrap()
                );
                assert_eq!(
                    value["checkpoint_sources"],
                    serde_json::to_value(&original.checkpoint_sources).unwrap()
                );
                assert_eq!(value["truncated"], false);
                saw_giant_derived = bytes.len() > 16_384;
            }
            crate::model::projected::FrozenModelInput::RequestOnly(_) => unreachable!(),
        }
    }
    assert!(
        saw_giant_derived,
        "full native provenance survives finite prefix clipping"
    );
    let oracle = observation_request::legacy_field_headers_oracle(
        &store.lock().unwrap(),
        started.sequence,
        store.conversation_id(),
        &input.messages,
        &snapshot,
    )
    .unwrap();
    for (item, header) in headers.iter().enumerate() {
        let path = vec!["messages".to_owned(), item.to_string()];
        assert_eq!(
            *header,
            oracle.iter().find(|field| field.0 == path).unwrap().4
        );
    }
    for (item, bytes) in provenance.iter().enumerate() {
        assert_eq!(
            &original_provenance_bytes(&store, &snapshot, started.sequence, item),
            bytes
        );
    }
    assert_eq!(store.load_canonical().unwrap(), canonical);
    assert_eq!(store.presentation_frontier().unwrap(), started.sequence);
}

fn assert_header_capture_fault(trigger: &str) {
    let (store, snapshot) = header_capture_fixture();
    let canonical = store.load_canonical().unwrap();
    store.lock().unwrap().execute_batch(trigger).unwrap();
    let started = store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap()
        .started;
    assert_eq!(
        store.load_request_snapshot(&snapshot.request_id).unwrap(),
        snapshot
    );
    assert_eq!(store.load_canonical().unwrap(), canonical);
    assert_eq!(store.presentation_frontier().unwrap(), started.sequence);
    assert!(
        store
            .request_summary_header(&snapshot.request_id, started.sequence)
            .is_err()
    );
    let connection = store.lock().unwrap();
    for (sql, id) in [
        (
            "SELECT count(*) FROM request_observation_fields WHERE source_id=?1",
            started.sequence.to_string(),
        ),
        (
            "SELECT count(*) FROM content_segments WHERE source_kind='trace_request' AND source_id=?1",
            started.sequence.to_string(),
        ),
        (
            "SELECT count(*) FROM content_frontiers WHERE source_kind='trace_request' AND source_id=?1",
            started.sequence.to_string(),
        ),
    ] {
        assert_eq!(
            connection
                .query_row(sql, [id], |row| row.get::<_, u64>(0))
                .unwrap(),
            0
        );
    }
    drop(connection);
    let (healthy, same_input) = header_capture_fixture();
    let accepted = healthy
        .commit_model_turn_start(&[], &same_input, Utc::now())
        .unwrap()
        .started;
    healthy
        .request_summary_header(&same_input.request_id, accepted.sequence)
        .unwrap();
    assert_eq!(
        captured_header_rows(&healthy.lock().unwrap(), accepted.sequence).len(),
        same_input.input.len()
    );
}

#[test]
fn t10_t19_request_header_placeholder_failure_rolls_back_only_optional_layout() {
    for trigger in [
        "CREATE TEMP TRIGGER refuse_captured_offsets BEFORE INSERT ON request_source_nodes BEGIN SELECT RAISE(FAIL,'optional Source node'); END;",
        "CREATE TEMP TRIGGER remove_captured_header BEFORE INSERT ON request_observation_fields BEGIN SELECT RAISE(FAIL,'optional original witness'); END;",
    ] {
        assert_header_capture_fault(trigger);
    }
}

#[test]
fn t08_t19_generic_json_message_lookalikes_never_claim_native_header_placeholders() {
    let store = store();
    store.initialize(&[]).unwrap();
    let opaque = serde_json::json!({"messages":[{"content":{"opaque":true},"collections":["original"],"deferred":true,"integer":u64::MAX}]});
    let connection = store.lock().unwrap();
    observation_request::take_header_constructions();
    crate::runtime::read_layout::take_field_preview_work();
    observation_request::index_state(&connection, "opaque-header-test", "original", &opaque)
        .unwrap();
    assert_eq!(observation_request::take_header_constructions(), 0);
    let (computed, skipped) = crate::runtime::read_layout::take_field_preview_work();
    assert_eq!(computed, 0, "generic state previews are now lazy too");
    assert!(skipped > 0);
    assert_eq!(
        connection
            .query_row(
                "SELECT count(*) FROM request_observation_fields",
                [],
                |row| row.get::<_, u64>(0)
            )
            .unwrap(),
        0
    );
    let chunk = observation_request::read_from(
        &connection,
        "opaque-header-test",
        "state_observation_fields",
        "original",
        &[],
        0,
        65_536,
    )
    .unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&chunk.bytes).unwrap(),
        opaque
    );
}

fn inline_originals(unicode: &str, large: &str) -> Vec<MessageBlock> {
    vec![
        user_message("inline-utf8", unicode),
        user_message("deferred-utf8", large),
        MessageBlock::Assistant(AssistantMessageBlock {
            id: MessageId::new("inline-answer"),
            content: vec![
                AssistantContentBlock::Text(TextBlock {
                    text: "original answer".into(),
                }),
                AssistantContentBlock::Reasoning(crate::message::types::ReasoningBlock {
                    text: Some("original reasoning".into()),
                    provider_state: None,
                }),
                AssistantContentBlock::Refusal(crate::message::types::RefusalBlock {
                    text: "original refusal".into(),
                }),
            ],
        }),
    ]
}

#[test]
fn t08_t12_t19_request_headers_already_inline_complete_text_and_retain_deferred_sources() {
    let store = store();
    let unicode = "水🙂e\u{301}".repeat(60);
    let large = "水🙂".repeat(4000);
    let originals = inline_originals(&unicode, &large);
    store.initialize(&originals).unwrap();
    let mut snapshot = test_request_snapshot(store.load_head().unwrap().revision, invocation());
    freeze_test_input(&store, &mut snapshot, &[]);
    let started = store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap()
        .started;
    let current = captured_header_rows(&store.lock().unwrap(), started.sequence);
    let first = store
        .request_field_header_at(
            started.sequence,
            started.sequence,
            &["messages".into(), "0".into()],
        )
        .unwrap();
    assert!(first.content.is_empty());
    assert_eq!(first.value["blocks"][0]["text"]["text"], unicode);
    assert_eq!(first.value["blocks"][0]["text"]["truncated"], false);
    assert_eq!(
        first.value["provenance"]["source"]["message_id"],
        "inline-utf8"
    );
    assert_eq!(first.value["provenance"]["locator"]["position"], 1);
    assert_eq!(
        (
            first.collections[0].loaded,
            first.collections[0].next_offset
        ),
        (1, None)
    );
    let assistant = store
        .request_field_header_at(
            started.sequence,
            started.sequence,
            &["messages".into(), "2".into()],
        )
        .unwrap();
    assert!(assistant.content.is_empty());
    assert_eq!(
        assistant.value["blocks"][0]["text"]["text"],
        "original answer"
    );
    assert_eq!(assistant.value["blocks"][1]["type"], "reasoning");
    assert_eq!(
        assistant.value["blocks"][1]["text"]["text"],
        "original reasoning"
    );
    assert_eq!(assistant.value["blocks"][2]["type"], "refusal");
    assert_eq!(
        assistant.value["blocks"][2]["text"]["text"],
        "original refusal"
    );
    assert_deferred_text_remains_original(&store, started.sequence, &large);
    let provenance = (0..snapshot.input.len())
        .map(|item| original_provenance_bytes(&store, &snapshot, started.sequence, item))
        .collect::<Vec<_>>();
    let input = store
        .reconstruct_model_request(&snapshot.request_id)
        .unwrap();
    let oracle = observation_request::legacy_field_headers_oracle(
        &store.lock().unwrap(),
        started.sequence,
        store.conversation_id(),
        &input.messages,
        &snapshot,
    )
    .unwrap();
    for (item, header) in current.iter().enumerate() {
        let path = vec!["messages".to_owned(), item.to_string()];
        assert_eq!(
            *header,
            oracle.iter().find(|field| field.0 == path).unwrap().4
        );
    }
    for (item, original) in provenance.iter().enumerate() {
        assert_eq!(
            &original_provenance_bytes(&store, &snapshot, started.sequence, item),
            original
        );
    }
    assert_eq!(
        store.load_request_snapshot(&snapshot.request_id).unwrap(),
        snapshot
    );
    assert_eq!(store.load_canonical().unwrap(), originals);
    assert_eq!(store.presentation_frontier().unwrap(), started.sequence);
}

fn assert_deferred_text_remains_original(store: &SqliteConversationStore, start: u64, text: &str) {
    use crate::runtime_client::content::ContentRepresentation;
    use base64::Engine as _;
    let field = store
        .request_field_header_at(start, start, &["messages".into(), "1".into()])
        .unwrap();
    assert!(field.deferred);
    assert_eq!(
        (
            field.collections[0].loaded,
            field.collections[0].next_offset
        ),
        (0, Some(0))
    );
    let binding = field
        .content
        .iter()
        .find(|binding| binding.content_ref.representation == ContentRepresentation::Utf8Text)
        .unwrap();
    assert_eq!(
        binding.content_ref.readable_end,
        u64::try_from(text.len()).unwrap()
    );
    let chunk = store.content_read(&binding.content_ref, 0, 65_536).unwrap();
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(chunk.data_base64)
            .unwrap(),
        text.as_bytes()
    );
    assert!(chunk.complete && chunk.next_offset.is_none());
    let inline = store
        .request_field_header_at(
            start,
            start,
            &["messages".into(), "0".into(), "content".into()],
        )
        .unwrap();
    assert_eq!(
        inline.children_total, 0,
        "complete inline text has no duplicate reference subtree"
    );
    let deferred = store
        .request_field_header_at(
            start,
            start,
            &["messages".into(), "1".into(), "content".into()],
        )
        .unwrap();
    assert!(
        deferred.children_total > 0,
        "deferred original references retain selected evidence"
    );
}
