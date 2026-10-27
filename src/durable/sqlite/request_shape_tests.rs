// The shape index is derived storage. Every comparison below is against the
// uncached native producer, never a shape, anchor, or prior request SQL row.
type ShapeField = (Vec<String>, u64, u64, u64, Vec<u8>);

struct ShapeOracle {
    raw: Vec<u8>,
    fields: Vec<ShapeField>,
}

fn shape_oracle(
    store: &SqliteConversationStore,
    snapshot: &RequestSnapshot,
    sequence: u64,
) -> ShapeOracle {
    let input = store
        .reconstruct_model_request(&snapshot.request_id)
        .unwrap();
    let connection = store.lock().unwrap();
    ShapeOracle {
        raw: observation_request::legacy_request_source_bytes(
            &connection,
            sequence,
            store.conversation_id(),
            &input.messages,
            snapshot,
        )
        .unwrap(),
        fields: observation_request::legacy_field_headers_oracle(
            &connection,
            sequence,
            store.conversation_id(),
            &input.messages,
            snapshot,
        )
        .unwrap(),
    }
}

fn assert_shape_oracle(
    store: &SqliteConversationStore,
    snapshot: &RequestSnapshot,
    sequence: u64,
    oracle: &ShapeOracle,
) {
    assert_eq!(fragment_root_bytes(store, snapshot, sequence), oracle.raw);
    let connection = store.lock().unwrap();
    for (path, start, end, children, header) in &oracle.fields {
        observation_request::composed::take_work();
        let actual = observation_request::field_from(
            &connection,
            "request_source_roots",
            &sequence.to_string(),
            path,
        )
        .unwrap();
        assert_shape_index_work(1);
        assert_eq!(
            actual,
            (*start, *end, *children, header.clone()),
            "{path:?}"
        );
        let mut offset = 0;
        let mut bytes = Vec::new();
        let mut first_extent = None;
        loop {
            observation_request::composed::take_work();
            let chunk =
                observation_request::read_at(&connection, sequence, sequence, path, offset, 257)
                    .unwrap();
            assert_shape_index_work(1);
            assert!(chunk.bytes.len() <= 257);
            assert_eq!(chunk.end_offset, offset + chunk.bytes.len() as u64);
            assert_eq!(
                *first_extent.get_or_insert(chunk.readable_end),
                chunk.readable_end
            );
            assert_eq!(chunk.readable_end, end - start);
            bytes.extend_from_slice(&chunk.bytes);
            if let Some(next) = chunk.next_offset {
                assert_eq!(next, chunk.end_offset);
                assert!(next > offset && next < chunk.readable_end);
                offset = next;
            } else {
                assert_eq!(chunk.end_offset, chunk.readable_end);
                break;
            }
        }
        assert_eq!(
            bytes,
            oracle.raw[usize::try_from(*start).unwrap()..usize::try_from(*end).unwrap()],
            "{path:?}"
        );
        if *children == 0 {
            continue;
        }
        let expected = oracle
            .fields
            .iter()
            .filter(|(child, ..)| child.len() == path.len() + 1 && child.starts_with(path))
            .collect::<Vec<_>>();
        let mut offset = 0;
        loop {
            observation_request::composed::take_work();
            let page =
                observation_request::page_at(&connection, sequence, sequence, path, offset, 3)
                    .unwrap();
            assert_shape_index_work(if path.len() == 2 && path[0] == "messages" {
                2
            } else {
                1
            });
            assert_eq!(page.offset, offset);
            assert_eq!(page.total, *children);
            assert!(page.fields.len() <= 3 && !page.fields.is_empty());
            for (index, actual) in page.fields.iter().enumerate() {
                let (path, start, end, children, header) =
                    expected[usize::try_from(offset).unwrap() + index];
                assert_eq!(&actual.path, path);
                assert_eq!(actual.encoded_bytes, end - start);
                assert_eq!(actual.children_total, *children);
                assert_eq!(
                    actual.value,
                    serde_json::from_slice::<serde_json::Value>(header).unwrap()
                );
            }
            let next = offset + page.fields.len() as u64;
            if let Some(advertised) = page.next_offset {
                assert_eq!(advertised, next);
                assert!(next > offset && next < page.total);
                offset = next;
            } else {
                assert_eq!(next, page.total);
                break;
            }
        }
    }
}

fn assert_shape_index_work(_maximum: usize) {
    assert_composed_work(observation_request::composed::take_work());
}

fn shape_snapshot(store: &SqliteConversationStore, turn: u32) -> RequestSnapshot {
    let mut snapshot = progress_request_snapshot(
        "shape-owner",
        &turn.to_string(),
        0,
        store.load_head().unwrap().revision,
        Vec::new(),
    );
    snapshot.invocation.model = format!("original-shape-model-{turn}");
    snapshot.effective_system_prompt = format!("frozen original prompt 水🙂 {turn}");
    snapshot.tool_definitions = vec![crate::tools::types::ModelToolDefinition {
        id: ToolId::new("shape-original-tool"),
        name: "shape original tool".into(),
        description: "original frozen definition".into(),
        input_schema: serde_json::json!({
            "type":"object",
            "properties": {
                "escaped": {"const":"水🙂\n\"\\"},
                "number": {"enum":[-1.25, -0.0, 0, 1]},
                "opaque": {"const":u64::MAX}
            }
        }),
    }];
    let history = store.projected_history().unwrap();
    snapshot.history_cut = history.cut;
    let request = ModelRequest {
        messages: history.messages,
        images: crate::model::images::ResolvedImages::new(),
        invocation: snapshot.invocation.clone(),
        tools: snapshot.tool_definitions.clone(),
        effective_system_prompt: snapshot.effective_system_prompt.clone(),
        continuation: snapshot.continuation.clone(),
        continuation_source: None,
    };
    snapshot
        .freeze_selected_input(store.conversation_id(), &request, &history.input_manifest)
        .unwrap();
    snapshot
}

#[test]
fn t08_t12_t14_t19_request_shapes_preserve_every_original_field_page_and_range_after_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("request-shapes.sqlite");
    let conversation = ConversationId::new("conv_36524fd8-f674-7fc2-8125-06d01fee0e18");
    let store = SqliteConversationStore::open(conversation.clone(), &path).unwrap();
    let original = (0..8)
        .map(|index| {
            user_message(
                &format!("shape-original-{index}"),
                &format!("original 水🙂 {index}\n\"\\"),
            )
        })
        .collect::<Vec<_>>();
    store.initialize(&original).unwrap();
    let first = shape_snapshot(&store, 1);
    let start = store
        .commit_model_turn_start(&[], &first, Utc::now())
        .unwrap()
        .started;
    let first_oracle = shape_oracle(&store, &first, start.sequence);
    assert_shape_oracle(&store, &first, start.sequence, &first_oracle);
    let second = shape_snapshot(&store, 2);
    let next = store
        .commit_model_turn_start(&[], &second, Utc::now())
        .unwrap()
        .started;
    let second_oracle = shape_oracle(&store, &second, next.sequence);
    assert_shape_oracle(&store, &first, start.sequence, &first_oracle);
    assert_shape_oracle(&store, &second, next.sequence, &second_oracle);
    // No process-local cache is needed for retained Source reads.
    assert_shape_oracle(&store, &second, next.sequence, &second_oracle);
    drop(store);
    let reopened = SqliteConversationStore::open(conversation, &path).unwrap();
    assert_eq!(reopened.load_canonical().unwrap(), original);
    assert_eq!(
        reopened.load_request_snapshot(&first.request_id).unwrap(),
        first
    );
    assert_eq!(
        reopened.load_request_snapshot(&second.request_id).unwrap(),
        second
    );
    assert_shape_oracle(&reopened, &first, start.sequence, &first_oracle);
    assert_shape_oracle(&reopened, &second, next.sequence, &second_oracle);
}

#[test]
fn t12_t14_t19_request_shapes_keep_old_requests_exact_after_header_and_raw_json_replacement() {
    let store = fragment_fixture();
    let canonical = store.load_canonical().unwrap();
    let first = shape_snapshot(&store, 1);
    let start = store
        .commit_model_turn_start(&[], &first, Utc::now())
        .unwrap()
        .started;
    let first_oracle = shape_oracle(&store, &first, start.sequence);
    let connection = store.lock().unwrap();
    let header: Vec<u8> = connection
        .query_row(
            "SELECT header_json FROM observation_headers WHERE ledger_position=1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&header).unwrap();
    let replacement = serde_json::to_vec_pretty(&value).unwrap();
    assert_ne!(header, replacement, "the native header bytes really change");
    connection
        .execute(
            "UPDATE observation_headers SET header_json=?1 WHERE ledger_position=1",
            [replacement],
        )
        .unwrap();
    let before: String = connection
        .query_row(
            "SELECT message_json FROM message_ledger WHERE position=1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let replacement = serde_json::to_string_pretty(&canonical[0]).unwrap();
    assert_ne!(
        before, replacement,
        "the original JSON encoding really changes"
    );
    connection
        .execute(
            "UPDATE message_ledger SET message_json=?1 WHERE position=1",
            [replacement],
        )
        .unwrap();
    drop(connection);
    assert_eq!(store.load_canonical().unwrap(), canonical);
    let second = shape_snapshot(&store, 2);
    let next = store
        .commit_model_turn_start(&[], &second, Utc::now())
        .unwrap()
        .started;
    let second_oracle = shape_oracle(&store, &second, next.sequence);
    assert_ne!(
        first_oracle.raw, second_oracle.raw,
        "each R has its own occurrence source"
    );
    assert_shape_oracle(&store, &first, start.sequence, &first_oracle);
    assert_shape_oracle(&store, &second, next.sequence, &second_oracle);
    // No process-local cache is needed for retained Source reads.
    assert_shape_oracle(&store, &first, start.sequence, &first_oracle);
    assert_shape_oracle(&store, &second, next.sequence, &second_oracle);
    assert_eq!(
        store.load_request_snapshot(&first.request_id).unwrap(),
        first
    );
    assert_eq!(
        store.load_request_snapshot(&second.request_id).unwrap(),
        second
    );
}

fn assert_shape_optional_source_hole(
    store: &SqliteConversationStore,
    snapshot: &RequestSnapshot,
    sequence: u64,
) {
    assert!(matches!(
        store.request_field_header_at(
            sequence,
            sequence,
            &["messages".into(), "0".into(), "role".into()]
        ),
        Err(ConversationStoreError::Storage(_))
    ));
    assert!(matches!(
        store.request_field_page_at(sequence, sequence, &["messages".into(), "0".into()], 0, 3),
        Err(ConversationStoreError::Storage(_))
    ));
    let cut = store.observation_read_cut().unwrap();
    let facts = store
        .observation_trace_facts(
            &cut,
            &crate::durable::observation::TraceFactQuery {
                scope: crate::durable::observation::TraceFactScope::All,
                kinds: vec![crate::durable::observation::TraceFactKind::ModelRequestStarted],
                before: None,
                after: sequence - 1,
                ascending: true,
                limit: 1,
            },
        )
        .unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].sequence, sequence);
    assert!(matches!(
        &facts[0].fact,
        crate::durable::observation::TraceFactData::RequestStart {
            start_sequence, request_id: Some(request), ..
        } if *start_sequence == sequence && request == &snapshot.request_id
    ));
}

#[test]
fn t10_t14_t19_request_shape_optional_insert_failure_preserves_native_r_c_j() {
    let store = fragment_fixture();
    let canonical = store.load_canonical().unwrap();
    let before = store.presentation_frontier().unwrap();
    let attempts = Arc::new(AtomicUsize::new(0));
    let observed = attempts.clone();
    {
        let connection = store.lock().unwrap();
        connection
            .create_scalar_function(
                "test_request_shape_insert",
                0,
                rusqlite::functions::FunctionFlags::SQLITE_UTF8,
                move |_| {
                    observed.fetch_add(1, Ordering::SeqCst);
                    Ok(0_i64)
                },
            )
            .unwrap();
        connection.execute_batch("CREATE TEMP TRIGGER refuse_request_shape BEFORE INSERT ON request_source_nodes BEGIN SELECT test_request_shape_insert(); SELECT RAISE(FAIL,'optional request shape'); END;").unwrap();
    }
    let snapshot = shape_snapshot(&store, 1);
    let committed = store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap();
    assert_eq!(
        attempts.load(Ordering::SeqCst),
        1,
        "the actual optional insert was reached"
    );
    assert_eq!(store.presentation_frontier().unwrap(), before + 1);
    assert_eq!(
        store.load_request_snapshot(&snapshot.request_id).unwrap(),
        snapshot
    );
    assert_eq!(store.load_canonical().unwrap(), canonical);
    assert_eq!(
        store.read_events(Some(before), 32).unwrap().events,
        vec![committed.started.clone()]
    );
    assert_shape_optional_source_hole(&store, &snapshot, committed.started.sequence);
    {
        let connection = store.lock().unwrap();
        for table in [
            "request_source_pages",
            "request_source_nodes",
            "request_source_roots",
        ] {
            let count: u64 = connection
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, 0, "no partial optional rows in {table}");
        }
        let native: u64 = connection
            .query_row("SELECT count(*) FROM native_input_nodes", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert!(native > 0, "the original mandatory native input committed");
        connection
            .execute_batch("DROP TRIGGER refuse_request_shape")
            .unwrap();
    }
    let healthy = shape_snapshot(&store, 2);
    let next = store
        .commit_model_turn_start(&[], &healthy, Utc::now())
        .unwrap()
        .started;
    let oracle = shape_oracle(&store, &healthy, next.sequence);
    assert_shape_oracle(&store, &healthy, next.sequence, &oracle);
    assert_eq!(
        store.load_request_snapshot(&snapshot.request_id).unwrap(),
        snapshot
    );
    assert_eq!(store.load_canonical().unwrap(), canonical);
}

struct ShapeTarget {
    store: SqliteConversationStore,
    snapshot: RequestSnapshot,
    sequence: u64,
    item: u64,
    // The exact selected native leaf; optional Source metadata augments it.
    shape: u64,
    oracle: ShapeOracle,
}

fn shape_target() -> ShapeTarget {
    let store = fragment_fixture();
    let snapshot = shape_snapshot(&store, 1);
    let sequence = store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap()
        .started
        .sequence;
    let shape = {
        let connection = store.lock().unwrap();
        let mut node = read_stored_snapshot_root(&connection, &snapshot.request_id)
            .unwrap()
            .unwrap();
        loop {
            match manifest::node(&connection, &node).unwrap().kind {
                manifest::NodeKind::Leaf => break node.id,
                manifest::NodeKind::Branch { left, .. } => node = left,
            }
        }
    };
    let oracle = shape_oracle(&store, &snapshot, sequence);
    ShapeTarget {
        store,
        snapshot,
        sequence,
        item: 0,
        shape,
        oracle,
    }
}

fn mutate_source_json(
    connection: &Connection,
    table: &str,
    key: &str,
    id: &rusqlite::types::Value,
    mutation: impl FnOnce(&mut serde_json::Value),
) {
    let raw: Vec<u8> = connection
        .query_row(
            &format!("SELECT metadata FROM {table} WHERE {key}=?1"),
            [id],
            |row| row.get(0),
        )
        .unwrap();
    let mut value = match table {
        "request_source_nodes" => observation_request::composed::inspect_node_record(&raw),
        "request_source_roots" => observation_request::composed::inspect_root_record(&raw),
        _ => unreachable!("closed source metadata corruption fixture"),
    };
    mutation(&mut value);
    let bytes = match table {
        "request_source_nodes" => observation_request::composed::encode_test_node_record(value),
        "request_source_roots" => observation_request::composed::encode_test_root_record(value),
        _ => unreachable!("closed source metadata corruption fixture"),
    };
    connection
        .execute(
            &format!("UPDATE {table} SET metadata=?2 WHERE {key}=?1"),
            params![id, bytes],
        )
        .unwrap();
}

fn corrupt_request_shape(
    connection: &Connection,
    sequence: u64,
    _item: u64,
    shape: u64,
    mutation: &str,
) {
    let source = sequence.to_string();
    match mutation {
        "metadata_json" | "metadata_type" | "metadata_oversize" => {
            let value = match mutation {
                "metadata_json" => "X'7b'",
                "metadata_type" => "'not a blob'",
                _ => "zeroblob(2000000)",
            };
            connection
                .execute(
                    &format!("UPDATE request_source_nodes SET metadata={value} WHERE native_id=?1"),
                    [shape],
                )
                .unwrap();
        }
        "missing_middle" => mutate_source_json(
            connection,
            "request_source_nodes",
            "native_id",
            &i64::try_from(shape).unwrap().into(),
            |value| {
                let variants = value["content"]["leaf"]["leaf"]["variants"]
                    .as_array_mut()
                    .unwrap();
                assert!(
                    variants.len() > 3,
                    "the corruption removes an interior width variant"
                );
                variants.remove(variants.len() / 2);
            },
        ),
        "missing_shape" => {
            assert_eq!(
                connection
                    .execute(
                        "DELETE FROM request_source_nodes WHERE native_id=?1",
                        [shape]
                    )
                    .unwrap(),
                1
            );
        }
        "wrong_real_shape" => {
            let other: Vec<u8> = connection.query_row("SELECT metadata FROM request_source_nodes WHERE native_id!=?1 ORDER BY native_id LIMIT 1", [shape], |row| row.get(0)).unwrap();
            connection
                .execute(
                    "UPDATE request_source_nodes SET metadata=?2 WHERE native_id=?1",
                    params![shape, other],
                )
                .unwrap();
        }
        "shape_hash" | "header_digest" => {
            connection
                .execute(
                    "UPDATE request_source_nodes SET hash=zeroblob(32) WHERE native_id=?1",
                    [shape],
                )
                .unwrap();
        }
        "native_digest" => {
            connection
                .execute(
                    "UPDATE request_source_nodes SET native_digest=zeroblob(32) WHERE native_id=?1",
                    [shape],
                )
                .unwrap();
        }
        "root_checksum" => {
            connection
                .execute(
                    "UPDATE request_source_roots SET checksum=zeroblob(32) WHERE source_id=?1",
                    [source],
                )
                .unwrap();
        }
        "field_start" | "root_start" | "field_end" | "hole_end" | "field_children"
        | "root_children" => {
            let field = match mutation {
                "field_start" | "root_start" => "messages_start",
                "field_end" | "hole_end" => "messages_end",
                _ => "length",
            };
            mutate_source_json(
                connection,
                "request_source_roots",
                "source_id",
                &source.into(),
                |value| {
                    value[field] = serde_json::json!(value[field].as_u64().unwrap() + 1);
                },
            );
        }
        _ => unreachable!("closed corruption matrix"),
    }
}

fn assert_shape_header_unavailable(connection: &Connection, sequence: u64, item: u64) {
    let root = ["messages".into(), item.to_string()];
    assert!(matches!(
        observation_request::field_header_at(connection, sequence, sequence, &root),
        Err(ConversationStoreError::Storage(_))
    ));
    assert!(matches!(
        observation_request::page_at(
            connection,
            sequence,
            sequence,
            &["messages".into()],
            item,
            1
        ),
        Err(ConversationStoreError::Storage(_))
    ));
}

fn assert_paired_shape_root_unavailable(connection: &Connection, sequence: u64, item: u64) {
    assert_shape_header_unavailable(connection, sequence, item);
    let root = ["messages".into(), item.to_string()];
    assert!(matches!(
        observation_request::read_at(connection, sequence, sequence, &root, 0, 257),
        Err(ConversationStoreError::Storage(_))
    ));
}

#[test]
fn t08_t12_t19_request_shape_anchor_and_metadata_corruption_never_rebases_original_authority() {
    for mutation in [
        "metadata_json",
        "metadata_type",
        "metadata_oversize",
        "missing_middle",
        "missing_shape",
        "wrong_real_shape",
        "shape_hash",
        "native_digest",
        "root_start",
        "root_children",
        "field_start",
        "field_end",
        "field_children",
        "hole_end",
        "header_digest",
        "root_checksum",
    ] {
        let ShapeTarget {
            store,
            snapshot,
            sequence,
            item,
            shape,
            oracle,
        } = shape_target();
        let canonical = store.load_canonical().unwrap();
        let before = store.presentation_frontier().unwrap();
        let journal = store.read_events(None, 32).unwrap().events;
        let path = ["messages".into(), item.to_string(), "role".into()];
        {
            let connection = store.lock().unwrap();
            connection
                .execute_batch("PRAGMA ignore_check_constraints=ON")
                .unwrap();
            corrupt_request_shape(&connection, sequence, item, shape, mutation);
            connection
                .execute_batch("PRAGMA ignore_check_constraints=OFF")
                .unwrap();
            observation_request::composed::take_work();
            assert!(
                matches!(
                    observation_request::field_header_at(&connection, sequence, sequence, &path),
                    Err(ConversationStoreError::Storage(_))
                ),
                "{mutation}: failed original Source must be explicit"
            );
            assert_composed_work(observation_request::composed::take_work());
            assert_paired_shape_root_unavailable(&connection, sequence, item);
            assert!(
                matches!(
                    observation_request::page_at(
                        &connection,
                        sequence,
                        sequence,
                        &["messages".into(), item.to_string()],
                        0,
                        3
                    ),
                    Err(ConversationStoreError::Storage(_))
                ),
                "{mutation}: no partial page"
            );
        }
        // Composed bytes share their authenticated augmentation; corruption is
        // unavailable, never replaced by a freshly rebuilt/latest R Source.
        // The independently frozen native R remains exact and reconstructible.
        assert_eq!(shape_oracle(&store, &snapshot, sequence).raw, oracle.raw);
        assert_eq!(store.presentation_frontier().unwrap(), before);
        assert_eq!(store.read_events(None, 32).unwrap().events, journal);
        assert_eq!(
            store.load_request_snapshot(&snapshot.request_id).unwrap(),
            snapshot
        );
        assert_eq!(store.load_canonical().unwrap(), canonical);
    }
}

#[test]
fn t14_t19_t20_request_shape_rows_grow_with_new_immutable_items_and_request_anchors() {
    let store = store();
    store
        .initialize(
            &(0..8)
                .map(|index| {
                    user_message(
                        &format!("shape-growth-{index}"),
                        "same bounded original text",
                    )
                })
                .collect::<Vec<_>>(),
        )
        .unwrap();
    let mut previous_leaves = 0;
    let mut previous_nodes = 0;
    let mut saved = Vec::new();
    for turn in 1..=6 {
        if turn > 1 {
            store
                .append_canonical(&user_message(
                    &format!("shape-new-{turn}"),
                    "one new immutable original",
                ))
                .unwrap();
        }
        let snapshot = shape_snapshot(&store, turn);
        observation_request::composed::take_work();
        let sequence = store
            .commit_model_turn_start(&[], &snapshot, Utc::now())
            .unwrap()
            .started
            .sequence;
        let work = observation_request::composed::take_work();
        let oracle = shape_oracle(&store, &snapshot, sequence);
        let connection = store.lock().unwrap();
        let (nodes, leaves): (u64,u64) = connection.query_row("SELECT count(*),sum(n.count=1) FROM request_source_nodes s JOIN native_input_nodes n ON n.id=s.native_id", [], |row| Ok((row.get(0)?, row.get(1)?))).unwrap();
        if turn == 1 {
            assert_eq!(leaves, snapshot.input.len() as u64);
        } else {
            assert_eq!(
                leaves,
                previous_leaves + 1,
                "only the new immutable occurrence adds a leaf"
            );
            let root = read_stored_snapshot_root(&connection, &snapshot.request_id)
                .unwrap()
                .unwrap();
            assert!(nodes - previous_nodes <= 2 * u64::from(root.height) + 2);
            assert_eq!(work.new_leaves, 1);
        }
        let roots: u64 = connection
            .query_row("SELECT count(*) FROM request_source_roots", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(roots, u64::from(turn));
        let seats: (u64,u64) = connection.query_row("SELECT count(*),sum(length(checksum)=32) FROM request_source_roots WHERE source_id=?1", [sequence.to_string()], |row| Ok((row.get(0)?, row.get(1)?))).unwrap();
        assert_eq!(
            seats,
            (1, 1),
            "one current-R root checksum, no separate capture witness row"
        );
        let maximum: u64 = connection
            .query_row(
                "SELECT max(length(metadata)) FROM request_source_nodes",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(maximum <= 65536);
        drop(connection);
        assert_shape_oracle(&store, &snapshot, sequence, &oracle);
        saved.push((snapshot, sequence, oracle));
        previous_leaves = leaves;
        previous_nodes = nodes;
    }
    for (snapshot, sequence, oracle) in &saved {
        assert_shape_oracle(&store, snapshot, *sequence, oracle);
    }
}

fn selected_template_hash(connection: &Connection, shape: u64) -> Vec<u8> {
    let raw: Vec<u8> = connection
        .query_row(
            "SELECT metadata FROM request_source_nodes WHERE native_id=?1",
            [shape],
            |row| row.get(0),
        )
        .unwrap();
    let metadata = observation_request::composed::inspect_node_record(&raw);
    serde_json::from_value(metadata["content"]["leaf"]["leaf"]["templates"][0].clone()).unwrap()
}

#[test]
fn t08_t12_t19_request_shape_root_header_hash_rejects_valid_json_replacement() {
    let ShapeTarget {
        store,
        snapshot,
        sequence,
        item,
        shape,
        oracle,
    } = shape_target();
    let before = store.presentation_frontier().unwrap();
    let canonical = store.load_canonical().unwrap();
    {
        let connection = store.lock().unwrap();
        let hash = selected_template_hash(&connection, shape);
        let raw: Vec<u8> = connection
            .query_row(
                "SELECT bytes FROM request_source_pages WHERE hash=?1",
                [&hash],
                |row| row.get(0),
            )
            .unwrap();
        let mut value = observation_request::composed::inspect_template_record(&raw);
        let role = "\"role\":\"user\"";
        let field = if value["header_prefix"].as_str().unwrap().contains(role) {
            "header_prefix"
        } else {
            "header_suffix"
        };
        let old = value[field].as_str().unwrap();
        assert!(
            old.contains(role),
            "the exact original role is in the template"
        );
        value[field] = serde_json::json!(old.replacen(role, "\"role\":\"assistant\"", 1));
        let changed = observation_request::composed::encode_test_template_record(value);
        assert!(changed.len() <= 65536);
        connection
            .execute(
                "UPDATE request_source_pages SET bytes=?2 WHERE hash=?1",
                params![hash, changed],
            )
            .unwrap();
        assert_paired_shape_root_unavailable(&connection, sequence, item);
    }
    assert_eq!(shape_oracle(&store, &snapshot, sequence).raw, oracle.raw);
    assert_eq!(store.presentation_frontier().unwrap(), before);
    assert_eq!(
        store.load_request_snapshot(&snapshot.request_id).unwrap(),
        snapshot
    );
    assert_eq!(store.load_canonical().unwrap(), canonical);
}

#[test]
fn t10_t14_t19_malformed_rehashed_shape_metadata_is_still_rejected() {
    let ShapeTarget {
        store,
        snapshot,
        sequence,
        item,
        shape,
        oracle,
    } = shape_target();
    let canonical = store.load_canonical().unwrap();
    let before = store.presentation_frontier().unwrap();
    {
        let connection = store.lock().unwrap();
        let raw: Vec<u8> = connection
            .query_row(
                "SELECT metadata FROM request_source_nodes WHERE native_id=?1",
                [shape],
                |row| row.get(0),
            )
            .unwrap();
        let mut metadata = observation_request::composed::inspect_node_record(&raw);
        let variants = metadata["content"]["leaf"]["leaf"]["variants"]
            .as_array_mut()
            .unwrap();
        assert!(
            variants.len() > 3,
            "the corruption removes an interior width variant"
        );
        variants.remove(variants.len() / 2);
        let changed = observation_request::composed::encode_test_node_record(metadata);
        let digest = <sha2::Sha256 as sha2::Digest>::digest(&changed);
        connection
            .execute(
                "UPDATE request_source_nodes SET metadata=?2,hash=?3 WHERE native_id=?1",
                params![shape, changed, digest.as_slice()],
            )
            .unwrap();
        assert_paired_shape_root_unavailable(&connection, sequence, item);
    }
    let next = shape_snapshot(&store, 100);
    let committed = store
        .commit_model_turn_start(&[], &next, Utc::now())
        .unwrap();
    assert_eq!(committed.started.sequence, before + 1);
    assert_shape_optional_source_hole(&store, &next, committed.started.sequence);
    assert_eq!(store.load_request_snapshot(&next.request_id).unwrap(), next);
    assert_eq!(
        store.load_request_snapshot(&snapshot.request_id).unwrap(),
        snapshot
    );
    assert_eq!(store.load_canonical().unwrap(), canonical);
    assert_eq!(shape_oracle(&store, &snapshot, sequence).raw, oracle.raw);
    let connection = store.lock().unwrap();
    let root_json = |source: u64| -> serde_json::Value {
        let bytes: Vec<u8> = connection
            .query_row(
                "SELECT metadata FROM request_source_roots WHERE source_id=?1",
                [source.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        observation_request::composed::inspect_root_record(&bytes)
    };
    let original_root = root_json(sequence);
    let next_root = root_json(committed.started.sequence);
    assert_eq!(next_root["native"], original_root["native"]);
    assert_eq!(next_root["sequence"], committed.started.sequence);
    assert_ne!(next_root["sequence"], original_root["sequence"]);
    assert_paired_shape_root_unavailable(&connection, committed.started.sequence, item);
}

fn assert_missing_shape_field(store: &SqliteConversationStore, sequence: u64, item: &str) {
    let path = [
        "messages".into(),
        item.into(),
        "caller_field_does_not_exist".into(),
    ];
    assert!(matches!(
        store.request_field_header_at(sequence, sequence, &path),
        Err(ConversationStoreError::InvalidReference(_))
    ));
    assert!(matches!(
        store.request_value_read_at(sequence, sequence, &path, 0, 257),
        Err(ConversationStoreError::InvalidReference(_))
    ));
    assert!(matches!(
        store.request_field_page_at(sequence, sequence, &path, 0, 3),
        Err(ConversationStoreError::InvalidReference(_))
    ));
}

#[test]
fn t08_t12_t19_request_shape_absent_caller_paths_are_distinct_from_missing_native_anchors() {
    let ShapeTarget { store, .. } = shape_target();
    let giant = user_message(&"\u{1}".repeat(3_000), "actual deferred original identity");
    store.append_canonical(&giant).unwrap();
    let snapshot = shape_snapshot(&store, 100);
    let sequence = store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap()
        .started
        .sequence;
    let last = snapshot.input.len() - 1;
    for item in [
        "0".into(),
        last.to_string(),
        "not-decimal".into(),
        "01".into(),
        "18446744073709551616".into(),
        snapshot.input.len().to_string(),
    ] {
        assert_missing_shape_field(&store, sequence, &item);
    }
    let canonical = store.load_canonical().unwrap();
    let journal = store.read_events(None, 32).unwrap().events;
    let oracle = shape_oracle(&store, &snapshot, sequence);
    let missing = {
        let connection = store.lock().unwrap();
        let missing = read_stored_snapshot_root(&connection, &snapshot.request_id)
            .unwrap()
            .unwrap()
            .id;
        connection.execute_batch("CREATE TEMP TABLE saved_shape_anchor AS SELECT * FROM request_source_nodes WHERE 0").unwrap();
        assert_eq!(connection.execute("INSERT INTO saved_shape_anchor SELECT * FROM request_source_nodes WHERE native_id=?1", [missing]).unwrap(), 1);
        assert_eq!(
            connection
                .execute(
                    "DELETE FROM request_source_nodes WHERE native_id=?1",
                    [missing]
                )
                .unwrap(),
            1
        );
        missing
    };
    {
        let connection = store.lock().unwrap();
        assert_paired_shape_root_unavailable(&connection, sequence, 0);
        let path = ["messages".into(), "0".into(), "role".into()];
        assert!(matches!(
            observation_request::field_header_at(&connection, sequence, sequence, &path),
            Err(ConversationStoreError::Storage(_))
        ));
        let saved: u64 = connection
            .query_row("SELECT native_id FROM saved_shape_anchor", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(saved, missing);
        connection.execute_batch("INSERT INTO request_source_nodes SELECT * FROM saved_shape_anchor; DROP TABLE saved_shape_anchor;").unwrap();
    }
    assert_shape_oracle(&store, &snapshot, sequence, &oracle);
    assert_eq!(store.read_events(None, 32).unwrap().events, journal);
    assert_eq!(
        store.load_request_snapshot(&snapshot.request_id).unwrap(),
        snapshot
    );
    assert_eq!(store.load_canonical().unwrap(), canonical);
}
