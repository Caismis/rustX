// Occurrence coordinates are generated only from the original RequestStart and
// the native selected rank. There is no per-R hole table or copied binding list.
fn hole_path(item: u64) -> Vec<String> {
    vec![
        "messages".into(),
        item.to_string(),
        "provenance_source".into(),
    ]
}
fn assert_packed_hole_unavailable(connection: &Connection, sequence: u64, item: u64) {
    let path = hole_path(item);
    assert!(matches!(
        observation_request::field_header_at(connection, sequence, sequence, &path),
        Err(ConversationStoreError::Storage(_))
    ));
    assert!(matches!(
        observation_request::read_at(connection, sequence, sequence, &path, 0, 257),
        Err(ConversationStoreError::Storage(_))
    ));
    assert!(matches!(
        observation_request::page_at(connection, sequence, sequence, &path, 0, 3),
        Err(ConversationStoreError::Storage(_))
    ));
    assert!(matches!(
        observation_request::page_at(connection, sequence, sequence, &path[..2], 0, 32),
        Err(ConversationStoreError::Storage(_))
    ));
}
#[test]
fn t08_t12_t19_parametric_request_holes_reject_root_corruption_before_allocation_and_restore_exactly()
 {
    let ShapeTarget {
        store,
        snapshot,
        sequence,
        item,
        oracle,
        ..
    } = shape_target();
    let canonical = store.load_canonical().unwrap();
    let journal = store.read_events(None, 32).unwrap().events;
    let cut = store.observation_read_cut().unwrap();
    let connection = store.lock().unwrap();
    for mutation in [
        "metadata='not a blob'",
        "metadata=zeroblob(2000000)",
        "metadata=X'7b'",
        "witness=zeroblob(32)",
        "witness='invalid'",
    ] {
        connection
            .execute_batch("SAVEPOINT bad_root; PRAGMA ignore_check_constraints=ON")
            .unwrap();
        assert_eq!(
            connection
                .execute(
                    &format!("UPDATE request_source_roots SET {mutation} WHERE source_id=?1"),
                    [sequence.to_string()]
                )
                .unwrap(),
            1
        );
        observation_request::composed::take_work();
        assert!(
            matches!(
                observation_request::field_header_at(
                    &connection,
                    sequence,
                    sequence,
                    &hole_path(item)
                ),
                Err(ConversationStoreError::Storage(_))
            ),
            "{mutation}"
        );
        let work = observation_request::composed::take_work();
        if mutation.starts_with("metadata='") || mutation.contains("2000000") {
            assert_eq!(
                work.source_metadata_bytes, 0,
                "gate before allocation: {mutation}"
            );
        }
        assert_packed_hole_unavailable(&connection, sequence, item);
        connection
            .execute_batch(
                "ROLLBACK TO bad_root; RELEASE bad_root; PRAGMA ignore_check_constraints=OFF",
            )
            .unwrap();
    }
    let field =
        observation_request::field_header_at(&connection, sequence, sequence, &hole_path(item))
            .unwrap();
    assert_eq!(field.value["type"], "value");
    let mut absent = hole_path(item);
    absent.push("caller_field_does_not_exist".into());
    assert!(matches!(
        observation_request::field_header_at(&connection, sequence, sequence, &absent),
        Err(ConversationStoreError::InvalidReference(_))
    ));
    drop(connection);
    assert_shape_oracle(&store, &snapshot, sequence, &oracle);
    assert_eq!(store.load_canonical().unwrap(), canonical);
    assert_eq!(store.read_events(None, 32).unwrap().events, journal);
    assert_eq!(store.observation_read_cut().unwrap(), cut);
}
#[test]
fn t10_t12_t19_parametric_hole_identity_cannot_be_rehashed_or_laundered_into_new_requests() {
    for key in ["sequence", "messages_start", "length"] {
        let ShapeTarget {
            store,
            snapshot,
            sequence,
            item,
            oracle,
            ..
        } = shape_target();
        let canonical = store.load_canonical().unwrap();
        let before = store.presentation_frontier().unwrap();
        let connection = store.lock().unwrap();
        let original: (Vec<u8>, Vec<u8>) = connection
            .query_row(
                "SELECT metadata,witness FROM request_source_roots WHERE source_id=?1",
                [sequence.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        let mut metadata: serde_json::Value = serde_json::from_slice(&original.0).unwrap();
        metadata[key] = serde_json::json!(metadata[key].as_u64().unwrap() + 1);
        let bytes = serde_json::to_vec(&metadata).unwrap();
        let mut digest = <sha2::Sha256 as sha2::Digest>::new();
        sha2::Digest::update(&mut digest, b"rustx-composed-request-source-v1\0");
        sha2::Digest::update(&mut digest, &bytes);
        let digest = sha2::Digest::finalize(digest);
        connection
            .execute(
                "UPDATE request_source_roots SET metadata=?2,witness=?3 WHERE source_id=?1",
                params![sequence.to_string(), bytes, digest.as_slice()],
            )
            .unwrap();
        assert_packed_hole_unavailable(&connection, sequence, item);
        drop(connection);
        let next = shape_snapshot(&store, 100);
        let started = store
            .commit_model_turn_start(&[], &next, Utc::now())
            .unwrap()
            .started;
        assert_eq!(started.sequence, before + 1);
        assert_shape_optional_source_hole(&store, &next, started.sequence);
        assert_eq!(
            store.load_request_snapshot(&snapshot.request_id).unwrap(),
            snapshot
        );
        assert_eq!(store.load_request_snapshot(&next.request_id).unwrap(), next);
        assert_eq!(store.load_canonical().unwrap(), canonical);
        store
            .lock()
            .unwrap()
            .execute(
                "UPDATE request_source_roots SET metadata=?2,witness=?3 WHERE source_id=?1",
                params![sequence.to_string(), original.0, original.1],
            )
            .unwrap();
        assert_shape_oracle(&store, &snapshot, sequence, &oracle);
    }
}
#[test]
fn t10_t14_t19_parametric_root_insert_failure_preserves_native_request_and_recovers_missing_augmentation()
 {
    let store = fragment_fixture();
    let canonical = store.load_canonical().unwrap();
    let attempts = Arc::new(AtomicUsize::new(0));
    let observed = attempts.clone();
    {
        let connection = store.lock().unwrap();
        connection
            .create_scalar_function(
                "test_source_root",
                0,
                rusqlite::functions::FunctionFlags::SQLITE_UTF8,
                move |_| {
                    observed.fetch_add(1, Ordering::SeqCst);
                    Ok(0_i64)
                },
            )
            .unwrap();
        connection.execute_batch("CREATE TEMP TRIGGER refuse_source_root BEFORE INSERT ON request_source_roots BEGIN SELECT test_source_root(); SELECT RAISE(FAIL,'optional original Source root'); END").unwrap();
    }
    let before = store.presentation_frontier().unwrap();
    let snapshot = shape_snapshot(&store, 1);
    let started = store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap()
        .started;
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    assert_eq!(started.sequence, before + 1);
    assert_eq!(
        store.load_request_snapshot(&snapshot.request_id).unwrap(),
        snapshot
    );
    assert_eq!(store.load_canonical().unwrap(), canonical);
    assert_shape_optional_source_hole(&store, &snapshot, started.sequence);
    assert_eq!(
        source_row_counts(&store),
        (0, 0, 0),
        "all optional descendants roll back"
    );
    store
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER refuse_source_root")
        .unwrap();
    let healthy = shape_snapshot(&store, 2);
    observation_request::composed::take_work();
    let next = store
        .commit_model_turn_start(&[], &healthy, Utc::now())
        .unwrap()
        .started;
    let work = observation_request::composed::take_work();
    assert_eq!(
        work.new_leaves,
        healthy.input.len(),
        "only genuinely missing augmentations are repaired"
    );
    assert_shape_oracle(
        &store,
        &healthy,
        next.sequence,
        &shape_oracle(&store, &healthy, next.sequence),
    );
    assert_eq!(
        store.load_request_snapshot(&snapshot.request_id).unwrap(),
        snapshot
    );
    assert_eq!(store.load_canonical().unwrap(), canonical);
}
#[test]
fn t08_t12_t19_decimal_request_and_rank_widths_match_the_original_full_producer() {
    let store = fragment_fixture();
    for turn in 1..=10 {
        let snapshot = shape_snapshot(&store, turn);
        let started = store
            .commit_model_turn_start(&[], &snapshot, Utc::now())
            .unwrap()
            .started;
        if started.sequence == 9 || started.sequence == 10 {
            let oracle = shape_oracle(&store, &snapshot, started.sequence);
            assert_shape_oracle(&store, &snapshot, started.sequence, &oracle);
            for rank in [0, 9, 10, 12] {
                let field = store
                    .request_field_header_at(started.sequence, started.sequence, &hole_path(rank))
                    .unwrap();
                assert_eq!(
                    field.value["domain"],
                    format!("trace.records.{}.detail.request.messages", started.sequence)
                );
                assert_eq!(field.value["item_offset"], serde_json::json!(rank));
            }
        }
    }
}

#[test]
fn t10_t14_t19_composed_source_schema_requires_exact_bounds_keys_and_original_witness_seat() {
    let store = fragment_fixture();
    let snapshot = shape_snapshot(&store, 1);
    let started = store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap()
        .started;
    let oracle = shape_oracle(&store, &snapshot, started.sequence);
    let connection = store.lock().unwrap();
    for (name, columns) in [
        ("request_source_pages", "hash BLOB,bytes BLOB"),
        (
            "request_source_pages",
            "hash BLOB PRIMARY KEY CHECK(typeof(hash)='blob' AND length(hash)=32),bytes BLOB NOT NULL CHECK(typeof(bytes)='blob' AND length(bytes)<=65536)",
        ),
        (
            "request_source_pages",
            "hash BLOB NOT NULL CHECK(typeof(hash)='blob' AND length(hash)=32),bytes BLOB NOT NULL CHECK(typeof(bytes)='blob' AND length(bytes)<=65536)",
        ),
        (
            "request_source_pages",
            "hash BLOB NOT NULL PRIMARY KEY CHECK(length(hash)=32),bytes BLOB NOT NULL CHECK(typeof(bytes)='blob' AND length(bytes)<=65536)",
        ),
        (
            "request_source_pages",
            "hash BLOB NOT NULL PRIMARY KEY CHECK(typeof(hash)='blob' AND length(hash)=32),bytes BLOB NOT NULL CHECK(typeof(bytes)='blob' AND length(bytes)<=65537)",
        ),
        ("request_source_nodes", "native_id INTEGER,metadata BLOB"),
        (
            "request_source_roots",
            "source_id TEXT,metadata BLOB,witness BLOB",
        ),
        (
            "request_observation_fields",
            "source_id TEXT,shape_witness BLOB",
        ),
    ] {
        connection
            .execute_batch("SAVEPOINT bad_source_schema")
            .unwrap();
        connection
            .execute_batch(&format!(
                "DROP TABLE {name}; CREATE TABLE {name}({columns})"
            ))
            .unwrap();
        assert!(
            matches!(
                observation_request::composed::verify_schema(&connection),
                Err(ConversationStoreError::IncompatibleSchema(_))
            ),
            "{name} must retain original type/size/key proofs"
        );
        connection
            .execute_batch("ROLLBACK TO bad_source_schema; RELEASE bad_source_schema")
            .unwrap();
        observation_request::composed::verify_schema(&connection).unwrap();
    }
    drop(connection);
    assert_shape_oracle(&store, &snapshot, started.sequence, &oracle);
    assert_eq!(
        store.load_request_snapshot(&snapshot.request_id).unwrap(),
        snapshot
    );
}

fn width_boundary_store(length: usize) -> SqliteConversationStore {
    use crate::scripted_suites::support::context::{checkpoint_commit_fixture, checkpoint_fixture};
    let store = store();
    store
        .initialize(&[user_message("width-owned-message", "original native input")])
        .unwrap();
    let checkpoint = checkpoint_fixture(&store, "width-checkpoint", 1, &"x".repeat(length));
    store
        .commit_projection(checkpoint_commit_fixture(&store, checkpoint, None))
        .unwrap();
    store
}
#[test]
fn t08_t19_exact_j_rank_pair_variants_preserve_admission_and_every_original_byte_at_the_16k_boundary()
 {
    let prototype = width_boundary_store(15000);
    let snapshot = shape_snapshot(&prototype, 1);
    let input = snapshot.input.messages();
    assert!(matches!(
        input.get(0),
        Some(crate::model::ModelInputMessage::Derived(_))
    ));
    let length = observation_request::composed::boundary_length(
        &prototype.lock().unwrap(),
        prototype.conversation_id(),
        input.get(0).unwrap(),
        &snapshot.input[0],
    );
    assert!((14000..=16384).contains(&length));
    let actual = width_boundary_store(length);
    let snapshot = shape_snapshot(&actual, 1);
    let input = snapshot.input.messages();
    let before = actual.presentation_frontier().unwrap();
    observation_request::composed::assert_variants(
        &actual.lock().unwrap(),
        actual.conversation_id(),
        input.get(0).unwrap(),
        &snapshot.input[0],
    );
    assert_eq!(
        actual.presentation_frontier().unwrap(),
        before,
        "private encoder proof publishes no request or fake journal fact"
    );
    assert_eq!(
        actual.load_canonical().unwrap(),
        vec![user_message("width-owned-message", "original native input")]
    );
}

#[test]
fn t19_composed_branch_reuses_its_already_authenticated_native_node() {
    let store = fragment_fixture();
    let snapshot = fragment_snapshot(&store, 1, 0);
    let sequence = store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap()
        .started
        .sequence;
    observation_request::composed::assert_single_native_validation(
        &store.lock().unwrap(),
        sequence,
    );
    assert_fragment_oracle(&store, &snapshot, sequence);
}
