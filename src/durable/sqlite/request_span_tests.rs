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

fn assert_span_work(_expected_calls: usize) -> observation_request::composed::Work {
    let work = observation_request::composed::take_work();
    assert!(work.max_index_page <= 65_536);
    assert!(work.max_source_metadata <= 65_536);
    // A bounded preview visits at most 512 pieces through paths of height 32.
    assert!(work.native_nodes <= 512 * 32);
    assert!(work.source_nodes <= 512 * 32);
    work
}

fn assert_span_public_key(
    field: &crate::durable::observation::RequestObservationField,
    oracle: &[LegacyRequestField],
) {
    let Some(name) = field.path.last().filter(|name| name.starts_with('@')) else {
        assert!(field.key.is_none());
        return;
    };
    assert!(name.len() > 1);
    let key = field.key.as_ref().expect("original aliased-key descriptor");
    let mut path = field.path.clone();
    path.push("$key".into());
    let (_, start, end, _, bytes) = oracle
        .iter()
        .find(|(candidate, ..)| candidate == &path)
        .unwrap();
    assert_eq!(key.path, path, "descriptor uses this selected occurrence");
    assert_eq!(
        key.preview,
        serde_json::from_slice::<String>(bytes).unwrap()
    );
    assert_eq!(key.encoded_bytes, end - start, "original key byte extent");
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
            observation_request::composed::take_work();
            let page =
                observation_request::page_at(connection, sequence, sequence, &parent, offset, 32)
                    .unwrap();
            assert_eq!(page.total, expected.len() as u64);
            assert_eq!(page.offset, offset);
            assert!(page.fields.len() <= 32 && !page.fields.is_empty());
            let work = observation_request::composed::take_work();
            assert!(work.max_index_page <= 65_536 && work.max_source_metadata <= 65_536);
            assert!(work.native_nodes <= 32 * 512 * 32);
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
    for (name, bytes, deferred) in [
        ("exact_bytes", 16_384, false),
        ("over_bytes", 16_385, true),
        ("exact_nodes", 63, false),
        ("over_nodes", 65, true),
        // The opaque integer remains exact in Source bytes; the established
        // finite JSON preview is null because it exceeds the JS-safe range.
        ("unsafe_integer", 20, true),
    ] {
        let path = ["tools", "0", "input_schema", "value", name].map(str::to_owned);
        let field =
            observation_request::field_header_at(connection, sequence, sequence, &path).unwrap();
        assert_eq!(field.encoded_bytes, bytes, "{name}");
        assert_eq!(field.deferred, deferred, "{name}");
        if name == "unsafe_integer" {
            assert_eq!(field.value, serde_json::Value::Null);
            let raw =
                observation_request::read_at(connection, sequence, sequence, &path, 0, 64).unwrap();
            assert_eq!(raw.bytes, u64::MAX.to_string().as_bytes());
            assert_eq!((raw.end_offset, raw.readable_end), (20, 20));
            assert_eq!(raw.next_offset, None);
        }
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
    for expected in &oracle {
        let (path, start, end, children, bytes) = expected;
        observation_request::composed::take_work();
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
        assert_span_work(0);
        observation_request::composed::take_work();
        let field =
            observation_request::field_header_at(&connection, sequence, sequence, path).unwrap();
        assert_span_work(0);
        assert_span_public_field(&field, expected);
        assert_span_public_key(&field, &oracle);
    }
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
            table_name: "request_snapshots",
            column_name: "snapshot_json",
        } => Authorization::Deny,
        _ => Authorization::Allow,
    }
}

fn request_span_raw(connection: &Connection, sequence: u64, path: &[String]) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut offset = 0;
    loop {
        observation_request::composed::take_work();
        let chunk =
            observation_request::read_at(connection, sequence, sequence, path, offset, 16_384)
                .unwrap();
        assert_eq!(
            assert_span_work(0).header_decodes,
            0,
            "raw byte ranges do not materialize display headers"
        );
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
        connection
            .query_row(
                "SELECT snapshot_json FROM request_snapshots LIMIT 1",
                [],
                |row| row.get::<_, String>(0)
            )
            .is_err(),
        "full native snapshot reads really are forbidden"
    );
    observation_request::composed::take_work();
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
