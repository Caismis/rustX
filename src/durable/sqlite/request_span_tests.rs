type LegacyRequestField = (Vec<String>, u64, u64, u64, Vec<u8>);

fn request_span_fixture() -> (SqliteConversationStore, RequestSnapshot, u64) {
    let store = store();
    store
        .initialize(&[
            user_message("span-normal", "original normal input"),
            // Native-admissible identity; genuinely deferred by the display producer.
            user_message(&"\u{1}".repeat(3_000), "original deferred identity"),
        ])
        .unwrap();
    let mut snapshot = test_request_snapshot(store.load_head().unwrap().revision, invocation());
    let wide = (0..70)
        .map(|index| (format!("key-{index:03}"), serde_json::json!(index)))
        .collect::<serde_json::Map<_, _>>();
    let mut schema = serde_json::json!({
        "escaped": "\"\\\n\t水🙂", "negative_zero": -0.0, "fraction": -1.25,
        "unsafe_integer": u64::MAX, "raw_looking": "{\"number\":18446744073709551615}",
        "exact_bytes": "x".repeat(16_382), "over_bytes": "y".repeat(16_383),
        "exact_nodes": vec![0; 31], "over_nodes": vec![0; 32],
        "wide": wide, "deferred": true,
    });
    schema["huge_key"] = serde_json::Value::Object(
        [(
            "huge水".repeat(4_000),
            serde_json::json!({"original": "value"}),
        )]
        .into_iter()
        .collect(),
    );
    snapshot.tool_definitions = vec![crate::tools::types::ModelToolDefinition {
        id: ToolId::new("original-span-tool"),
        name: "Original frozen tool".into(),
        description: "Frozen original description".into(),
        input_schema: schema,
    }];
    snapshot.effective_system_prompt = "original prompt水\n".repeat(6_000);
    freeze_test_input(&store, &mut snapshot, &[]);
    let sequence = store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap()
        .started
        .sequence;
    (store, snapshot, sequence)
}

fn request_span_oracle(
    store: &SqliteConversationStore,
    snapshot: &RequestSnapshot,
    sequence: u64,
) -> Vec<LegacyRequestField> {
    let input = store
        .reconstruct_model_request(&snapshot.request_id)
        .unwrap();
    observation_request::legacy_field_headers_oracle(
        &store.lock().unwrap(),
        sequence,
        store.conversation_id(),
        &input.messages,
        snapshot,
    )
    .unwrap()
}

fn assert_span_work(expected_calls: usize) {
    let (calls, rows, bytes) = observation_request::take_header_span_work();
    assert_eq!(calls, expected_calls);
    assert!(rows <= 2, "selected header read {rows} native rows");
    assert!(
        bytes <= 16_384,
        "selected header materialized {bytes} bytes"
    );
}

fn assert_span_public_field(
    field: &crate::durable::observation::RequestObservationField,
    expected: &LegacyRequestField,
) {
    let (path, start, end, children, bytes) = expected;
    let value: serde_json::Value = serde_json::from_slice(bytes).unwrap();
    assert_eq!(&field.path, path);
    assert_eq!(field.value, value);
    assert_eq!(field.encoded_bytes, end - start);
    assert_eq!(field.children_total, *children);
    let producer_deferred = path.len() == 2
        && path[0] == "messages"
        && path[1].parse::<u64>().is_ok()
        && value.get("deferred").and_then(serde_json::Value::as_bool) == Some(true);
    assert_eq!(
        field.deferred,
        end - start > bytes.len() as u64 || producer_deferred
    );
    let metadata = |name| {
        if path.len() == 2 && path[0] == "messages" {
            value
                .get(name)
                .cloned()
                .unwrap_or_else(|| serde_json::json!([]))
        } else {
            serde_json::json!([])
        }
    };
    let content: Vec<crate::runtime_client::checkpoint::ContentBinding> =
        crate::runtime_client::checkpoint::decode_domain_value(metadata("content")).unwrap();
    let collections: Vec<crate::runtime_client::checkpoint::CollectionFrontier> =
        crate::runtime_client::checkpoint::decode_domain_value(metadata("collections")).unwrap();
    assert_eq!(field.content, content);
    assert_eq!(field.collections, collections);
}

fn assert_span_pages(connection: &Connection, sequence: u64, oracle: &[LegacyRequestField]) {
    for parent in [
        vec!["messages".into()],
        vec![
            "tools".into(),
            "0".into(),
            "input_schema".into(),
            "value".into(),
        ],
        vec![
            "tools".into(),
            "0".into(),
            "input_schema".into(),
            "value".into(),
            "wide".into(),
        ],
    ] {
        let expected = oracle
            .iter()
            .filter(|(path, ..)| path.len() == parent.len() + 1 && path.starts_with(&parent))
            .collect::<Vec<_>>();
        let mut offset = 0;
        loop {
            observation_request::take_header_span_work();
            let page =
                observation_request::page_at(connection, sequence, sequence, &parent, offset, 32)
                    .unwrap();
            assert_eq!(page.total, expected.len() as u64);
            assert_eq!(page.offset, offset);
            assert!(page.fields.len() <= 32 && !page.fields.is_empty());
            let (calls, rows, bytes) = observation_request::take_header_span_work();
            assert!(calls <= 32 && rows <= 2 && bytes <= 16_384);
            for (index, field) in page.fields.iter().enumerate() {
                assert_span_public_field(field, expected[usize::try_from(offset).unwrap() + index]);
            }
            let next = offset + page.fields.len() as u64;
            if let Some(advertised) = page.next_offset {
                assert_eq!(advertised, next);
                assert!(advertised > offset && advertised < page.total);
                offset = advertised;
            } else {
                assert_eq!(next, page.total);
                break;
            }
        }
    }
}

fn assert_span_admission_boundaries(connection: &Connection, sequence: u64) {
    for (name, kind, bytes, nodes) in [
        ("exact_bytes", "original_span", 16_384, Some(1)),
        ("over_bytes", "inline", 16_385, None),
        ("exact_nodes", "original_span", 63, Some(32)),
        ("over_nodes", "inline", 65, None),
        ("unsafe_integer", "original_span", 20, Some(1)),
    ] {
        let path = ["tools", "0", "input_schema", "value", name];
        let stored: (String, u64, Option<u64>) = connection.query_row(
            "SELECT header_kind,end-start,header_nodes FROM request_observation_fields WHERE source_id=?1 AND path_json=?2",
            params![sequence.to_string(), serde_json::to_string(&path).unwrap()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        ).unwrap();
        assert_eq!(stored, (kind.to_owned(), bytes, nodes), "{name}");
    }
    let deferred = observation_request::field_header_at(
        connection,
        sequence,
        sequence,
        &["messages".into(), "1".into()],
    )
    .unwrap();
    assert!(
        deferred.deferred,
        "the original large native identity stays deferred"
    );
}

#[test]
fn t08_t14_t15_t19_original_span_headers_match_every_legacy_field_and_advancing_page() {
    let (store, snapshot, sequence) = request_span_fixture();
    let canonical = store.load_canonical().unwrap();
    let cut = store.observation_read_cut().unwrap();
    let oracle = request_span_oracle(&store, &snapshot, sequence);
    let connection = store.lock().unwrap();
    let fields =
        observation_request::shape::normalized_field_kinds(&connection, &sequence.to_string())
            .unwrap();
    assert_eq!(
        fields.len(),
        oracle.len(),
        "no original field or synthetic key disappeared"
    );
    let mut span_count = 0;
    let mut inline_count = 0;
    for expected in &oracle {
        let (path, start, end, children, bytes) = expected;
        let kind = fields
            .get(path)
            .expect("every original field remains indexed");
        let calls = usize::from(kind == "original_span");
        span_count += calls;
        inline_count += usize::from(kind == "inline");
        observation_request::take_header_span_work();
        assert_eq!(
            observation_request::field_from(
                &connection,
                "request_observation_fields",
                &sequence.to_string(),
                path
            )
            .unwrap(),
            (*start, *end, *children, bytes.clone()),
            "legacy normalized header changed at {path:?}"
        );
        assert_span_work(calls);
        observation_request::take_header_span_work();
        let field =
            observation_request::field_header_at(&connection, sequence, sequence, path).unwrap();
        assert_span_work(calls);
        assert_span_public_field(&field, expected);
    }
    assert!(span_count > 50 && inline_count > 0);
    assert_span_admission_boundaries(&connection, sequence);
    assert_span_pages(&connection, sequence, &oracle);
    drop(connection);
    assert_eq!(store.load_canonical().unwrap(), canonical);
    assert_eq!(store.observation_read_cut().unwrap(), cut);
}

fn deny_request_span_headers(
    context: rusqlite::hooks::AuthContext<'_>,
) -> rusqlite::hooks::Authorization {
    use rusqlite::hooks::{AuthAction, Authorization};
    match context.action {
        AuthAction::Read {
            table_name: "request_observation_fields",
            column_name: "header_json" | "header_kind" | "header_nodes",
        } => Authorization::Deny,
        _ => Authorization::Allow,
    }
}

fn request_span_raw(connection: &Connection, sequence: u64, path: &[String]) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut offset = 0;
    loop {
        let chunk =
            observation_request::read_at(connection, sequence, sequence, path, offset, 16_384)
                .unwrap();
        assert!(chunk.bytes.len() <= 16_384);
        assert_eq!(chunk.end_offset, offset + chunk.bytes.len() as u64);
        bytes.extend_from_slice(&chunk.bytes);
        if let Some(next) = chunk.next_offset {
            assert!(next > offset && next < chunk.readable_end);
            offset = next;
        } else {
            assert_eq!(chunk.end_offset, chunk.readable_end);
            return bytes;
        }
    }
}

#[test]
fn t08_t15_t19_original_span_raw_ranges_preserve_original_bytes_without_materializing_headers() {
    let (store, snapshot, sequence) = request_span_fixture();
    let oracle = request_span_oracle(&store, &snapshot, sequence);
    let connection = store.lock().unwrap();
    connection.authorizer(Some(deny_request_span_headers));
    assert!(
        observation_request::field_header_at(
            &connection,
            sequence,
            sequence,
            &["retry_number".into()],
        )
        .is_err(),
        "the original-range proof really forbids header reads"
    );
    observation_request::take_header_span_work();
    let original = request_span_raw(&connection, sequence, &[]);
    assert_span_work(0);
    assert!(
        original.len() > 65_536,
        "fixture crosses original Source segments"
    );
    for (path, start, end, _, _) in &oracle {
        let bytes = request_span_raw(&connection, sequence, path);
        assert_eq!(
            bytes,
            original[usize::try_from(*start).unwrap()..usize::try_from(*end).unwrap()],
            "original range {path:?}"
        );
    }
    assert_span_work(0);
    let prompt_path = vec!["effective_system_prompt".into(), "text".into()];
    assert_eq!(
        request_span_raw(&connection, sequence, &prompt_path),
        serde_json::to_vec(&snapshot.effective_system_prompt).unwrap()
    );
    for (name, value) in snapshot.tool_definitions[0]
        .input_schema
        .as_object()
        .unwrap()
    {
        let path = vec![
            "tools".into(),
            "0".into(),
            "input_schema".into(),
            "value".into(),
            name.clone(),
        ];
        assert_eq!(
            request_span_raw(&connection, sequence, &path),
            serde_json::to_vec(value).unwrap()
        );
    }
    assert_span_work(0);
    connection
        .authorizer(None::<fn(rusqlite::hooks::AuthContext<'_>) -> rusqlite::hooks::Authorization>);
}
