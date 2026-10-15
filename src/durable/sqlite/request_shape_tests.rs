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
        observation_request::shape::take_read_work();
        let actual = observation_request::field_from(
            &connection,
            "request_observation_fields",
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
            observation_request::shape::take_read_work();
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
            observation_request::shape::take_read_work();
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

fn assert_shape_index_work(maximum: usize) {
    let (loads, bytes) = observation_request::shape::take_read_work();
    assert!(
        loads <= maximum,
        "root pages load at most one static index and one own-hole index"
    );
    assert!(
        bytes <= 65_536,
        "shape metadata allocation is bounded before decoding"
    );
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
    freeze_test_input(store, &mut snapshot, &[]);
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
    store.conn.request_fragments.lock().unwrap().evict();
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
    store.conn.request_fragments.lock().unwrap().evict();
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
        connection.execute_batch(
            "CREATE TEMP TRIGGER refuse_request_shape BEFORE INSERT ON request_field_shapes BEGIN SELECT test_request_shape_insert(); SELECT RAISE(FAIL,'optional request shape'); END;",
        ).unwrap();
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
    let journal = store.read_events(Some(before), 32).unwrap().events;
    assert_eq!(journal.len(), 1);
    assert_eq!(journal[0], committed.started);
    assert_shape_optional_source_hole(&store, &snapshot, committed.started.sequence);
    {
        let connection = store.lock().unwrap();
        for table in [
            "request_field_shapes",
            "request_item_layouts",
            "request_observation_fields",
        ] {
            let count: u64 = connection
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, 0, "no partial optional rows in {table}");
        }
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
    shape: u64,
    oracle: ShapeOracle,
}

fn shape_target() -> ShapeTarget {
    let store = fragment_fixture();
    for turn in 1..=4 {
        let snapshot = shape_snapshot(&store, turn);
        let started = store
            .commit_model_turn_start(&[], &snapshot, Utc::now())
            .unwrap()
            .started;
        let selected: Option<(u64, u64)> = store.lock().unwrap().query_row(
            "SELECT item_ordinal,shape_id FROM request_item_layouts WHERE source_id=?1 AND layout_kind='shape' ORDER BY item_ordinal LIMIT 1",
            [started.sequence.to_string()], |row| Ok((row.get(0)?, row.get(1)?)),
        ).optional().unwrap();
        if let Some((item, shape)) = selected {
            let oracle = shape_oracle(&store, &snapshot, started.sequence);
            return ShapeTarget {
                store,
                snapshot,
                sequence: started.sequence,
                item,
                shape,
                oracle,
            };
        }
    }
    panic!("eligible immutable canonical descriptors never acquired a durable shape");
}

fn corrupt_request_shape(
    connection: &Connection,
    sequence: u64,
    item: u64,
    shape: u64,
    mutation: &str,
) {
    match mutation {
        "metadata_json" => {
            connection
                .execute(
                    "UPDATE request_field_shapes SET metadata=X'7b' WHERE shape_id=?1",
                    [shape],
                )
                .unwrap();
        }
        "metadata_type" => {
            connection
                .execute(
                    "UPDATE request_field_shapes SET metadata='not a blob' WHERE shape_id=?1",
                    [shape],
                )
                .unwrap();
        }
        "metadata_oversize" => {
            connection
                .execute(
                    "UPDATE request_field_shapes SET metadata=zeroblob(2000000) WHERE shape_id=?1",
                    [shape],
                )
                .unwrap();
        }
        "missing_middle" => {
            let bytes: Vec<u8> = connection
                .query_row(
                    "SELECT metadata FROM request_field_shapes WHERE shape_id=?1",
                    [shape],
                    |row| row.get(0),
                )
                .unwrap();
            let mut metadata: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            let fields = metadata["fields"].as_array_mut().unwrap();
            assert!(fields.len() > 3);
            fields.remove(fields.len() / 2);
            connection
                .execute(
                    "UPDATE request_field_shapes SET metadata=?2 WHERE shape_id=?1",
                    params![shape, serde_json::to_vec(&metadata).unwrap()],
                )
                .unwrap();
        }
        "missing_shape" => {
            connection.execute_batch("PRAGMA foreign_keys=OFF").unwrap();
            assert_eq!(
                connection
                    .execute(
                        "DELETE FROM request_field_shapes WHERE shape_id=?1",
                        [shape]
                    )
                    .unwrap(),
                1
            );
            connection.execute_batch("PRAGMA foreign_keys=ON").unwrap();
        }
        "wrong_real_shape" => {
            let other: u64 = connection
                .query_row(
                    "SELECT shape_id FROM request_field_shapes WHERE shape_id!=?1 LIMIT 1",
                    [shape],
                    |row| row.get(0),
                )
                .unwrap();
            connection.execute("UPDATE request_item_layouts SET shape_id=?3 WHERE source_id=?1 AND item_ordinal=?2", params![sequence.to_string(), item, other]).unwrap();
        }
        "field_start" | "field_end" | "field_children" => {
            let column = match mutation {
                "field_start" => "start",
                "field_end" => "end",
                _ => "children",
            };
            connection.execute(&format!("UPDATE request_observation_fields SET {column}={column}+1 WHERE source_id=?1 AND path_json=?2"), params![sequence.to_string(), serde_json::to_string(&["messages".to_owned(), item.to_string()]).unwrap()]).unwrap();
        }
        "root_witness" => {
            connection.execute("UPDATE request_observation_fields SET shape_witness=zeroblob(32) WHERE source_id=?1 AND path_json=?2", params![sequence.to_string(), serde_json::to_string(&["messages".to_owned(), item.to_string()]).unwrap()]).unwrap();
        }
        column => {
            let value = match column {
                "root_start" => "root_start+1",
                "root_children" => "root_children+1",
                "hole_end" => "hole_end+1",
                "shape_hash" | "header_digest" => "zeroblob(32)",
                _ => unreachable!("closed corruption cases"),
            };
            connection.execute(&format!("UPDATE request_item_layouts SET {column}={value} WHERE source_id=?1 AND item_ordinal=?2"), params![sequence.to_string(), item]).unwrap();
        }
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
        "root_start",
        "root_children",
        "field_start",
        "field_end",
        "field_children",
        "hole_end",
        "header_digest",
        "root_witness",
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
            if matches!(
                mutation,
                "field_start" | "field_end" | "field_children" | "root_children"
            ) {
                assert_paired_shape_root_unavailable(&connection, sequence, item);
            }
            connection
                .execute_batch("PRAGMA ignore_check_constraints=OFF")
                .unwrap();
            observation_request::shape::take_read_work();
            assert!(
                matches!(
                    observation_request::field_header_at(&connection, sequence, sequence, &path),
                    Err(ConversationStoreError::Storage(_))
                ),
                "{mutation}: selected shape metadata must be explicitly unavailable"
            );
            let (loads, bytes) = observation_request::shape::take_read_work();
            assert!(loads <= 1 && bytes <= 65_536);
            if matches!(mutation, "metadata_type" | "metadata_oversize") {
                assert_eq!(
                    (loads, bytes),
                    (0, 0),
                    "SQL rejects type/size before materializing the BLOB"
                );
            }
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
                "{mutation}: no partial page may escape a failed shape certificate"
            );
        }
        assert_eq!(
            fragment_root_bytes(&store, &snapshot, sequence),
            oracle.raw,
            "{mutation}: exact original R remains independent of derived shape metadata"
        );
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
    let mut previous_shapes = 0;
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
        let started = store
            .commit_model_turn_start(&[], &snapshot, Utc::now())
            .unwrap()
            .started;
        let oracle = shape_oracle(&store, &snapshot, started.sequence);
        let connection = store.lock().unwrap();
        let shapes: u64 = connection
            .query_row("SELECT count(*) FROM request_field_shapes", [], |row| {
                row.get(0)
            })
            .unwrap();
        if turn == 1 {
            assert_eq!(shapes, snapshot.input.len() as u64);
        } else {
            assert_eq!(
                shapes,
                previous_shapes + 1,
                "only the new immutable item creates shape metadata"
            );
        }
        let anchors: (u64, u64) = connection.query_row(
            "SELECT count(*),sum(layout_kind='shape') FROM request_item_layouts WHERE source_id=?1",
            [started.sequence.to_string()], |row| Ok((row.get(0)?, row.get(1)?)),
        ).unwrap();
        assert_eq!(
            anchors,
            (snapshot.input.len() as u64, snapshot.input.len() as u64)
        );
        let expected_own = oracle
            .fields
            .iter()
            .filter(|(path, ..)| {
                path.first().is_none_or(|part| part != "messages") || path.len() <= 2
            })
            .count() as u64;
        let own_rows: u64 = connection
            .query_row(
                "SELECT count(*) FROM request_observation_fields WHERE source_id=?1",
                [started.sequence.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            own_rows, expected_own,
            "only root rows remain per shaped item; both field indexes retain every logical field"
        );
        let packed: (u64, u64) = connection.query_row(
            "SELECT count(*),sum(json_array_length(CAST(hole_metadata AS TEXT),'$.fields')) FROM request_item_layouts WHERE source_id=?1 AND layout_kind='shape' AND typeof(hole_metadata)='blob' AND length(hole_metadata)<=16384",
            [started.sequence.to_string()], |row| Ok((row.get(0)?, row.get(1)?)),
        ).unwrap();
        assert_eq!(
            packed,
            (anchors.0, anchors.0 * 8),
            "each current request owns all eight descriptor facts without eight field rows"
        );
        let max_metadata: u64 = connection
            .query_row(
                "SELECT max(length(metadata)) FROM request_field_shapes",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(max_metadata <= 65_536);
        drop(connection);
        assert_shape_oracle(&store, &snapshot, started.sequence, &oracle);
        saved.push((snapshot, started.sequence, oracle));
        previous_shapes = shapes;
    }
    store.conn.request_fragments.lock().unwrap().evict();
    for (snapshot, sequence, oracle) in &saved {
        assert_shape_oracle(&store, snapshot, *sequence, oracle);
    }
}

#[test]
fn t08_t12_t19_request_shape_root_header_hash_rejects_valid_json_replacement() {
    let ShapeTarget {
        store,
        snapshot,
        sequence,
        item,
        oracle,
        ..
    } = shape_target();
    let before = store.presentation_frontier().unwrap();
    let canonical = store.load_canonical().unwrap();
    {
        let connection = store.lock().unwrap();
        let path = serde_json::to_string(&["messages".to_owned(), item.to_string()]).unwrap();
        let bytes: Vec<u8> = connection.query_row(
            "SELECT header_json FROM request_observation_fields WHERE source_id=?1 AND path_json=?2",
            params![sequence.to_string(), path], |row| row.get(0),
        ).unwrap();
        let mut header: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(header["role"], "user");
        header["role"] = serde_json::json!("assistant");
        let changed = serde_json::to_vec(&header).unwrap();
        assert!(changed.len() <= 16_384);
        connection.execute(
            "UPDATE request_observation_fields SET header_json=?3 WHERE source_id=?1 AND path_json=?2",
            params![sequence.to_string(), path, changed],
        ).unwrap();
        assert_shape_header_unavailable(&connection, sequence, item);
    }
    assert_eq!(fragment_root_bytes(&store, &snapshot, sequence), oracle.raw);
    assert_eq!(store.presentation_frontier().unwrap(), before);
    assert_eq!(
        store.load_request_snapshot(&snapshot.request_id).unwrap(),
        snapshot
    );
    assert_eq!(store.load_canonical().unwrap(), canonical);
}

#[test]
fn t10_t14_t19_rehashed_shape_metadata_cannot_be_laundered_into_a_new_native_request() {
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
        let bytes: Vec<u8> = connection
            .query_row(
                "SELECT metadata FROM request_field_shapes WHERE shape_id=?1",
                [shape],
                |row| row.get(0),
            )
            .unwrap();
        let mut metadata: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let fields = metadata["fields"].as_array_mut().unwrap();
        assert!(fields.len() > 3);
        fields.remove(fields.len() / 2);
        let changed = serde_json::to_vec(&metadata).unwrap();
        assert!(changed.len() <= 65_536);
        let digest = <sha2::Sha256 as sha2::Digest>::digest(&changed);
        connection
            .execute(
                "UPDATE request_field_shapes SET metadata=?2,shape_hash=?3 WHERE shape_id=?1",
                params![shape, changed, digest.as_slice()],
            )
            .unwrap();
        assert!(matches!(
            observation_request::field_header_at(
                &connection,
                sequence,
                sequence,
                &["messages".into(), item.to_string(), "role".into()]
            ),
            Err(ConversationStoreError::Storage(_))
        ));
    }
    let next = shape_snapshot(&store, 100);
    let committed = store
        .commit_model_turn_start(&[], &next, Utc::now())
        .unwrap();
    assert_eq!(committed.started.sequence, before + 1);
    assert_eq!(store.presentation_frontier().unwrap(), before + 1);
    assert_eq!(store.load_request_snapshot(&next.request_id).unwrap(), next);
    assert_eq!(
        store.load_request_snapshot(&snapshot.request_id).unwrap(),
        snapshot
    );
    assert_eq!(store.load_canonical().unwrap(), canonical);
    assert_eq!(fragment_root_bytes(&store, &snapshot, sequence), oracle.raw);
    assert_shape_optional_source_hole(&store, &next, committed.started.sequence);
    let connection = store.lock().unwrap();
    for table in ["request_item_layouts", "request_observation_fields"] {
        let count: u64 = connection
            .query_row(
                &format!("SELECT count(*) FROM {table} WHERE source_id=?1"),
                [committed.started.sequence.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            count, 0,
            "the corrupt prototype was not published into {table}"
        );
    }
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
    let (shared, own): (u64, u64) = {
        let connection = store.lock().unwrap();
        let shared = connection.query_row(
            "SELECT item_ordinal FROM request_item_layouts WHERE source_id=?1 AND layout_kind='shape' ORDER BY item_ordinal LIMIT 1",
            [sequence.to_string()], |row| row.get(0),
        ).unwrap();
        let own = connection.query_row(
            "SELECT item_ordinal FROM request_item_layouts WHERE source_id=?1 AND layout_kind='own' ORDER BY item_ordinal DESC LIMIT 1",
            [sequence.to_string()], |row| row.get(0),
        ).unwrap();
        (shared, own)
    };
    assert_eq!(
        own + 1,
        snapshot.input.len() as u64,
        "the genuine oversized original uses Own storage"
    );
    for item in [
        shared.to_string(),
        own.to_string(),
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
    {
        let connection = store.lock().unwrap();
        connection.execute_batch("CREATE TEMP TABLE saved_shape_anchor AS SELECT * FROM request_item_layouts WHERE 0; PRAGMA foreign_keys=OFF;").unwrap();
        assert_eq!(connection.execute("INSERT INTO saved_shape_anchor SELECT * FROM request_item_layouts WHERE source_id=?1 AND item_ordinal=?2", params![sequence.to_string(), shared]).unwrap(), 1);
        assert_eq!(
            connection
                .execute(
                    "DELETE FROM request_item_layouts WHERE source_id=?1 AND item_ordinal=?2",
                    params![sequence.to_string(), shared]
                )
                .unwrap(),
            1
        );
        connection.execute_batch("PRAGMA foreign_keys=ON").unwrap();
    }
    let path = ["messages".into(), shared.to_string(), "role".into()];
    assert!(matches!(
        store.request_field_header_at(sequence, sequence, &path),
        Err(ConversationStoreError::Storage(_))
    ));
    assert!(matches!(
        store.request_value_read_at(sequence, sequence, &path, 0, 257),
        Err(ConversationStoreError::Storage(_))
    ));
    assert!(matches!(
        store.request_field_page_at(
            sequence,
            sequence,
            &["messages".into(), shared.to_string()],
            0,
            3
        ),
        Err(ConversationStoreError::Storage(_))
    ));
    store.lock().unwrap().execute_batch("INSERT INTO request_item_layouts SELECT * FROM saved_shape_anchor; DROP TABLE saved_shape_anchor;").unwrap();
    assert_shape_oracle(&store, &snapshot, sequence, &oracle);
    assert_eq!(store.read_events(None, 32).unwrap().events, journal);
    assert_eq!(
        store.load_request_snapshot(&snapshot.request_id).unwrap(),
        snapshot
    );
    assert_eq!(store.load_canonical().unwrap(), canonical);
}
