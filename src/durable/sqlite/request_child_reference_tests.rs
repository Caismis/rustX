use base64::Engine as _;

fn replace_captured_child(
    connection: &Connection,
    reference: &mut serde_json::Value,
    key: &str,
) -> (Vec<u8>, Vec<u8>) {
    let hash: Vec<u8> = serde_json::from_value(reference["hash"].clone()).unwrap();
    let raw: Vec<u8> = connection
        .query_row(
            "SELECT bytes FROM request_source_pages WHERE hash=?1",
            [hash.as_slice()],
            |row| row.get(0),
        )
        .unwrap();
    assert!(raw.len() <= 65_536);
    assert_eq!(
        <sha2::Sha256 as sha2::Digest>::digest(&raw).as_slice(),
        hash
    );
    let mut page: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    let original = if page["type"] == "leaf" {
        let entry = page["entries"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|entry| entry[0] == key)
            .unwrap();
        let before: Vec<String> = serde_json::from_slice(
            &base64::engine::general_purpose::STANDARD
                .decode(entry[1].as_str().unwrap())
                .unwrap(),
        )
        .unwrap();
        assert_ne!(before, ["request_id"]);
        entry[1] = serde_json::json!(
            base64::engine::general_purpose::STANDARD.encode(br#"["request_id"]"#)
        );
        (hash, raw)
    } else {
        let child = page["children"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|child| key <= child["last"].as_str().unwrap())
            .unwrap();
        assert!(child["depth"].as_u64().unwrap() < reference["depth"].as_u64().unwrap());
        replace_captured_child(connection, child, key)
    };
    let encoded = serde_json::to_vec(&page).unwrap();
    assert!(encoded.len() <= 65_536);
    let digest = <sha2::Sha256 as sha2::Digest>::digest(&encoded);
    connection
        .execute(
            "INSERT OR IGNORE INTO request_source_pages(hash,bytes) VALUES(?1,?2)",
            params![digest.as_slice(), encoded],
        )
        .unwrap();
    reference["hash"] = serde_json::json!(digest.as_slice());
    original
}

#[test]
fn t08_t12_t19_original_child_references_reject_page_tamper_and_recapture_without_the_original_seat()
 {
    let store = fragment_fixture();
    let snapshot = shape_snapshot(&store, 1);
    let sequence = store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap()
        .started
        .sequence;
    let oracle = shape_oracle(&store, &snapshot, sequence);
    assert_shape_oracle(&store, &snapshot, sequence, &oracle);
    let canonical = store.load_canonical().unwrap();
    let cut = store.observation_read_cut().unwrap();
    {
        let connection = store.lock().unwrap();
        connection
            .execute_batch("SAVEPOINT original_child_corruption")
            .unwrap();
        let original: (Vec<u8>, Vec<u8>) = connection
            .query_row(
                "SELECT metadata,witness FROM request_source_roots WHERE source_id=?1",
                [sequence.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        let mut metadata: serde_json::Value = serde_json::from_slice(&original.0).unwrap();
        let (leaf_hash, leaf_bytes) = replace_captured_child(
            &connection,
            &mut metadata["scalar"]["fields"],
            "C[]:00000000000000000000",
        );
        connection
            .execute(
                "UPDATE request_source_pages SET bytes=x'7b7d' WHERE hash=?1",
                [leaf_hash.as_slice()],
            )
            .unwrap();
        observation_request::composed::take_work();
        assert!(matches!(
            observation_request::page_at(&connection, sequence, sequence, &[], 0, 1),
            Err(ConversationStoreError::Storage(_))
        ));
        assert_composed_work(observation_request::composed::take_work());
        connection
            .execute(
                "UPDATE request_source_pages SET bytes=?2 WHERE hash=?1",
                params![leaf_hash, leaf_bytes],
            )
            .unwrap();
        assert!(observation_request::page_at(&connection, sequence, sequence, &[], 0, 1).is_ok());
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
        let seat: Vec<u8> = connection
            .query_row(
                "SELECT shape_witness FROM request_observation_fields WHERE source_id=?1",
                [sequence.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            seat, original.1,
            "the original capture seat is never retagged"
        );
        assert!(matches!(
            observation_request::page_at(&connection, sequence, sequence, &[], 0, 1),
            Err(ConversationStoreError::Storage(_))
        ));
        connection
            .execute_batch(
                "ROLLBACK TO original_child_corruption; RELEASE original_child_corruption",
            )
            .unwrap();
    }
    assert_eq!(store.load_canonical().unwrap(), canonical);
    assert_eq!(
        store.load_request_snapshot(&snapshot.request_id).unwrap(),
        snapshot
    );
    assert_eq!(store.observation_read_cut().unwrap(), cut);
    assert_eq!(store.presentation_frontier().unwrap(), sequence);
    assert_shape_oracle(&store, &snapshot, sequence, &oracle);
    let next = shape_snapshot(&store, 2);
    let next_j = store
        .commit_model_turn_start(&[], &next, Utc::now())
        .unwrap()
        .started
        .sequence;
    assert_shape_oracle(&store, &snapshot, sequence, &oracle);
    assert_shape_oracle(&store, &next, next_j, &shape_oracle(&store, &next, next_j));
}
