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
fn t08_t12_t19_packed_request_holes_reject_corruption_before_allocation_and_restore_exactly() {
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
        "hole_metadata=NULL",
        "hole_metadata='not a blob'",
        "hole_metadata=zeroblob(2000000)",
        "hole_hash=zeroblob(32)",
        "hole_hash='invalid'",
        "hole_metadata=X'7b'",
        "layout_kind='invalid'",
    ] {
        connection
            .execute_batch("SAVEPOINT bad_hole; PRAGMA ignore_check_constraints=ON")
            .unwrap();
        assert_eq!(connection.execute(&format!("UPDATE request_item_layouts SET {mutation} WHERE source_id=?1 AND item_ordinal=?2"),params![sequence.to_string(),item]).unwrap(),1);
        observation_request::shape::take_read_work();
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
        let work = observation_request::shape::take_read_work();
        if mutation != "hole_metadata=X'7b'" {
            assert_eq!(
                work,
                (0, 0),
                "SQL/certificate rejects before payload allocation: {mutation}"
            );
        }
        assert_packed_hole_unavailable(&connection, sequence, item);
        connection
            .execute_batch(
                "ROLLBACK TO bad_hole; RELEASE bad_hole; PRAGMA ignore_check_constraints=OFF",
            )
            .unwrap();
    }
    observation_request::shape::take_read_work();
    let field =
        observation_request::field_header_at(&connection, sequence, sequence, &hole_path(item))
            .unwrap();
    let (loads, bytes) = observation_request::shape::take_read_work();
    assert_eq!(loads, 1);
    assert!(bytes > 0 && bytes <= 16384);
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
fn mutate_hole_geometry(connection: &Connection, sequence: u64, item: u64, mutation: &str) {
    let bytes: Vec<u8> = connection
        .query_row(
            "SELECT hole_metadata FROM request_item_layouts WHERE source_id=?1 AND item_ordinal=?2",
            params![sequence.to_string(), item],
            |row| row.get(0),
        )
        .unwrap();
    let mut metadata: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    match mutation {
        "missing_middle" => {
            metadata["fields"].as_array_mut().unwrap().remove(1);
        }
        "ordinal" => metadata["ordinal"] = serde_json::json!(31),
        "span" => metadata["fields"][0]["start"] = serde_json::json!(99999),
        _ => unreachable!(),
    }
    let bytes = serde_json::to_vec(&metadata).unwrap();
    let hash = <sha2::Sha256 as sha2::Digest>::digest(&bytes);
    connection.execute("UPDATE request_item_layouts SET hole_metadata=?3,hole_hash=?4 WHERE source_id=?1 AND item_ordinal=?2",params![sequence.to_string(),item,bytes,hash.as_slice()]).unwrap();
}
#[test]
fn t10_t12_t19_packed_hole_geometry_cannot_be_rehashed_or_laundered_into_new_requests() {
    for mutation in ["missing_middle", "ordinal", "span"] {
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
        {
            let connection = store.lock().unwrap();
            mutate_hole_geometry(&connection, sequence, item, mutation);
            assert_packed_hole_unavailable(&connection, sequence, item);
        }
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
        assert_eq!(fragment_root_bytes(&store, &snapshot, sequence), oracle.raw);
        assert_eq!(store.load_canonical().unwrap(), canonical);
    }
}
#[test]
fn t10_t14_t19_packed_hole_update_failure_never_refuses_native_request_or_journal() {
    let store = fragment_fixture();
    let canonical = store.load_canonical().unwrap();
    let attempts = Arc::new(AtomicUsize::new(0));
    let observed = attempts.clone();
    {
        let connection = store.lock().unwrap();
        connection
            .create_scalar_function(
                "test_pack_hole",
                0,
                rusqlite::functions::FunctionFlags::SQLITE_UTF8,
                move |_| {
                    observed.fetch_add(1, Ordering::SeqCst);
                    Ok(0_i64)
                },
            )
            .unwrap();
        connection.execute_batch("CREATE TEMP TRIGGER refuse_own_hole BEFORE UPDATE OF hole_metadata ON request_item_layouts WHEN NEW.hole_metadata IS NOT NULL BEGIN SELECT test_pack_hole(); SELECT RAISE(FAIL,'own hole metadata'); END").unwrap();
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
    {
        let connection = store.lock().unwrap();
        for table in [
            "request_item_layouts",
            "request_field_shapes",
            "request_observation_fields",
        ] {
            assert_eq!(
                connection
                    .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| row
                        .get::<_, u64>(0))
                    .unwrap(),
                0
            );
        }
        connection
            .execute_batch("DROP TRIGGER refuse_own_hole")
            .unwrap();
    }
    let healthy = shape_snapshot(&store, 2);
    let next = store
        .commit_model_turn_start(&[], &healthy, Utc::now())
        .unwrap()
        .started;
    assert_shape_oracle(
        &store,
        &healthy,
        next.sequence,
        &shape_oracle(&store, &healthy, next.sequence),
    );
}
