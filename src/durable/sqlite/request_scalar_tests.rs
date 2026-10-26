fn scalar_snapshot(store: &SqliteConversationStore, turn: u32, prompt: String) -> RequestSnapshot {
    let mut snapshot = shape_snapshot(store, turn);
    snapshot.effective_system_prompt = prompt;
    let history = store.projected_history().unwrap();
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
fn scalar_root(connection: &Connection, sequence: u64) -> serde_json::Value {
    let bytes: Vec<u8> = connection
        .query_row(
            "SELECT metadata FROM request_source_roots WHERE source_id=?1",
            [sequence.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[test]
fn request_scalar_original_and_indexed_preserve_all_fields_pages_ranges_after_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("request-scalars.sqlite");
    let conversation = ConversationId::new("conv_30f2dc0b-95ec-49ce-a783-e0d6da94742d");
    let store = SqliteConversationStore::open(conversation.clone(), &path).unwrap();
    let canonical = vec![user_message("scalar-original", "Original history 水🙂")];
    store.initialize(&canonical).unwrap();
    let mut retained = Vec::new();
    for (turn, prompt, kind) in [
        (1, "small original 水🙂".to_owned(), "original"),
        (2, "large original 水🙂".repeat(900), "indexed"),
    ] {
        let snapshot = scalar_snapshot(&store, turn, prompt);
        let sequence = store
            .commit_model_turn_start(&[], &snapshot, Utc::now())
            .unwrap()
            .started
            .sequence;
        assert_eq!(
            scalar_root(&store.lock().unwrap(), sequence)["scalar"]["type"],
            kind
        );
        let oracle = shape_oracle(&store, &snapshot, sequence);
        assert_shape_oracle(&store, &snapshot, sequence, &oracle);
        retained.push((snapshot, sequence, oracle));
    }
    drop(store);
    let reopened = SqliteConversationStore::open(conversation, &path).unwrap();
    assert_eq!(reopened.load_canonical().unwrap(), canonical);
    for (snapshot, sequence, oracle) in retained {
        assert_eq!(
            reopened
                .load_request_snapshot(&snapshot.request_id)
                .unwrap(),
            snapshot
        );
        assert_shape_oracle(&reopened, &snapshot, sequence, &oracle);
    }
}

#[test]
fn request_scalar_bad_extent_rejects_even_message_only_reads_without_changing_native_state() {
    let store = fragment_fixture();
    let snapshot = shape_snapshot(&store, 1);
    let sequence = store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap()
        .started
        .sequence;
    let oracle = shape_oracle(&store, &snapshot, sequence);
    let canonical = store.load_canonical().unwrap();
    let cut = store.observation_read_cut().unwrap();
    {
        let connection = store.lock().unwrap();
        let original = scalar_root(&connection, sequence);
        assert_eq!(original["scalar"]["type"], "original");
        for extent in [0, 16_385] {
            connection.execute_batch("SAVEPOINT scalar_extent").unwrap();
            let mut metadata = original.clone();
            metadata["scalar"]["length"] = serde_json::json!(extent);
            let bytes = serde_json::to_vec(&metadata).unwrap();
            let mut digest = <sha2::Sha256 as sha2::Digest>::new();
            sha2::Digest::update(&mut digest, b"rustx-composed-request-source-v1\0");
            sha2::Digest::update(&mut digest, &bytes);
            connection
                .execute(
                    "UPDATE request_source_roots SET metadata=?2,checksum=?3 WHERE source_id=?1",
                    params![
                        sequence.to_string(),
                        bytes,
                        sha2::Digest::finalize(digest).as_slice()
                    ],
                )
                .unwrap();
            for path in [vec![], vec!["tools".into()], vec!["messages".into()]] {
                assert!(matches!(
                    observation_request::page_at(&connection, sequence, sequence, &path, 0, 1),
                    Err(ConversationStoreError::Storage(_))
                ));
            }
            connection
                .execute_batch("ROLLBACK TO scalar_extent; RELEASE scalar_extent")
                .unwrap();
        }
    }
    assert_eq!(store.load_canonical().unwrap(), canonical);
    assert_eq!(
        store.load_request_snapshot(&snapshot.request_id).unwrap(),
        snapshot
    );
    assert_eq!(store.observation_read_cut().unwrap(), cut);
    assert_shape_oracle(&store, &snapshot, sequence, &oracle);
}
