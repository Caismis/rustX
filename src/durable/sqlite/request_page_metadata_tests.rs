// Corrupt finite page metadata must be gated in SQLite before Rust cell copies.
fn assert_page_metadata_corruption(
    connection: &Connection,
    sequence: u64,
    parent: &[String],
    target: &[String],
) {
    let id = sequence.to_string();
    let target = serde_json::to_string(target).unwrap();
    let ordinal: u64 = connection
        .query_row(
            "SELECT ordinal FROM request_observation_fields WHERE source_id=?1 AND path_json=?2",
            params![id, target],
            |row| row.get(0),
        )
        .unwrap();
    let read = || observation_request::page_at(connection, sequence, sequence, parent, ordinal, 1);
    let original = serde_json::to_value(read().unwrap()).unwrap();
    for (mutation, invalid_path) in [
        ("path_json=CAST(zeroblob(2000000) AS TEXT)", true),
        ("path_json=X'5b5d'", true),
        ("key_header=zeroblob(2000000)", false),
        ("key_header='[]'", false),
    ] {
        connection
            .execute_batch("SAVEPOINT corrupt_page_metadata; PRAGMA ignore_check_constraints=ON")
            .unwrap();
        connection
            .execute(
                &format!("UPDATE request_observation_fields SET {mutation} WHERE source_id=?1 AND path_json=?2"),
                params![id, target],
            )
            .unwrap();
        observation_request::take_page_metadata_work();
        assert!(
            matches!(read(), Err(ConversationStoreError::Storage(_))),
            "{mutation}"
        );
        let (path_bytes, key_bytes) = observation_request::take_page_metadata_work();
        if invalid_path {
            assert_eq!(path_bytes, 0, "invalid path never reaches Rust String");
        } else {
            assert!(path_bytes > 0 && path_bytes <= 4096);
        }
        assert_eq!(key_bytes, 0, "invalid key never reaches Rust Vec");
        connection
            .execute_batch("ROLLBACK TO corrupt_page_metadata; RELEASE corrupt_page_metadata; PRAGMA ignore_check_constraints=OFF")
            .unwrap();
        assert_eq!(serde_json::to_value(read().unwrap()).unwrap(), original);
    }
}

#[test]
fn t08_t12_t19_page_metadata_is_type_and_size_gated_before_rust_allocation() {
    let store = fragment_fixture();
    let snapshot = shape_snapshot(&store, 1);
    let sequence = store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap()
        .started
        .sequence;
    let canonical = store.load_canonical().unwrap();
    let cut = store.observation_read_cut().unwrap();
    let connection = store.lock().unwrap();
    let tag: String = connection
        .query_row(
            "SELECT layout_kind FROM request_item_layouts WHERE source_id=?1 AND item_ordinal=0",
            [sequence.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        tag, "shape",
        "exercise a real shared item header on the outer page"
    );
    assert_page_metadata_corruption(
        &connection,
        sequence,
        &["messages".into()],
        &["messages".into(), "0".into()],
    );
    assert_page_metadata_corruption(
        &connection,
        sequence,
        &["tools".into()],
        &["tools".into(), "0".into()],
    );
    drop(connection);
    assert_eq!(store.load_canonical().unwrap(), canonical);
    assert_eq!(
        store.load_request_snapshot(&snapshot.request_id).unwrap(),
        snapshot
    );
    assert_eq!(store.observation_read_cut().unwrap(), cut);
    assert_eq!(store.presentation_frontier().unwrap(), sequence);
}
