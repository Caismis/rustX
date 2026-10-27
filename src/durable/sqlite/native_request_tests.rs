fn configuration_snapshot(store: &SqliteConversationStore, turn: u32) -> RequestSnapshot {
    let mut snapshot = test_request_snapshot(store.load_head().unwrap().revision, invocation());
    snapshot.identity.turn = TurnId::new(turn.to_string());
    snapshot.request_id = snapshot.identity.request_id();
    snapshot.provisional_message_id = snapshot.identity.provisional_message_id();
    snapshot.effective_system_prompt = "frozen prompt 水🙂\\\"\n".repeat(512);
    snapshot.tool_definitions = (0..2).map(|index| crate::tools::types::ModelToolDefinition {
        id: ToolId::new(format!("native-shared-{index}")),
        name: format!("native_shared_{index}"),
        description: "exact tool description".repeat(128),
        input_schema: serde_json::json!({"type":"object", "properties":{"opaque":{"default":[null,true,-0.0,u64::MAX],"description":"schema 水🙂".repeat(512)}}}),
    }).collect();
    snapshot.system_sections = ["first", "second"]
        .map(|name| crate::context::assembly::AcceptedSystemSection {
            lane: crate::context::assembly::SystemSectionLane::CoreRuntimeIdentity,
            contributor: crate::runtime::identity::ContextContributorIdentity::Native(
                crate::runtime::identity::NativeContextContributor::CoreSystemIdentity,
            ),
            content: format!("{name} 水🙂"),
        })
        .to_vec();
    freeze_test_input(store, &mut snapshot, &[]);
    snapshot
}
fn configuration_count(store: &SqliteConversationStore) -> u64 {
    store
        .lock()
        .unwrap()
        .query_row("SELECT count(*) FROM request_configurations", [], |row| {
            row.get(0)
        })
        .unwrap()
}
fn stored_configuration_metadata(
    store: &SqliteConversationStore,
    snapshot: &RequestSnapshot,
) -> String {
    store
        .lock()
        .unwrap()
        .query_row(
            "SELECT snapshot_json FROM request_snapshots WHERE request_id=?1",
            [snapshot.request_id.as_str()],
            |row| row.get(0),
        )
        .unwrap()
}

#[test]
fn shared_native_configuration_reuses_exact_bytes_and_preserves_scalar_validation() {
    let store = store();
    store
        .initialize(&[user_message("shared-original", "history")])
        .unwrap();
    let first = configuration_snapshot(&store, 1);
    store
        .commit_model_turn_start(&[], &first, Utc::now())
        .unwrap();
    let mut second = configuration_snapshot(&store, 2);
    // Opaque invocation changes stay per-request; no duplicate model identity
    // or dependence on the current config is introduced by sharing tools.
    second.invocation.model = "different-model".into();
    second.invocation.request_params.insert(
        "opaque".into(),
        serde_json::json!({"null":null,"large":u64::MAX}),
    );
    freeze_test_input(&store, &mut second, &[]);
    store
        .commit_model_turn_start(&[], &second, Utc::now())
        .unwrap();
    assert_eq!(configuration_count(&store), 1);
    for snapshot in [&first, &second] {
        let json = stored_configuration_metadata(&store, snapshot);
        assert!(json.len() * 10 < encode_snapshot(snapshot).unwrap().len());
        native_request::CONFIGURATION_DECODES.with(|count| count.set(0));
        {
            let mut connection = store.lock().unwrap();
            let transaction = connection.transaction().unwrap();
            let (metadata, _, _) = require_started_request_tx(
                &transaction,
                store.conversation_id(),
                &snapshot.request_id,
            )
            .unwrap();
            assert_eq!(metadata.invocation, snapshot.invocation);
            assert_eq!(metadata.tool_definitions, snapshot.tool_definitions);
            assert_eq!(
                metadata.effective_system_prompt,
                snapshot.effective_system_prompt
            );
            assert!(metadata.input.is_empty());
        }
        assert_eq!(
            native_request::CONFIGURATION_DECODES.with(std::cell::Cell::get),
            1
        );
        assert_eq!(
            store.load_request_snapshot(&snapshot.request_id).unwrap(),
            *snapshot
        );
        assert_eq!(
            native_request::CONFIGURATION_DECODES.with(std::cell::Cell::get),
            2
        );
        assert_eq!(
            store
                .reconstruct_model_request(&snapshot.request_id)
                .unwrap(),
            snapshot
                .reconstruct_from_canonical(&store.load_canonical().unwrap())
                .unwrap()
        );
        assert!(
            store
                .commit_model_turn_start(&[], snapshot, Utc::now())
                .is_ok()
        );
    }
    assert_eq!(configuration_count(&store), 1);
}

#[test]
fn shared_native_configuration_changed_prompt_and_order_remain_exact_after_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("shared-config.sqlite");
    let conversation = ConversationId::generate();
    let store = SqliteConversationStore::open(conversation.clone(), &path).unwrap();
    store.initialize(&[]).unwrap();
    let mut snapshots = Vec::new();
    for turn in 1..=5 {
        let mut snapshot = configuration_snapshot(&store, turn);
        match turn {
            2 => snapshot.tool_definitions.reverse(),
            3 => snapshot.effective_system_prompt.push_str(" changed"),
            4 => snapshot.system_sections.reverse(),
            _ => {}
        }
        freeze_test_input(&store, &mut snapshot, &[]);
        store
            .commit_model_turn_start(&[], &snapshot, Utc::now())
            .unwrap();
        snapshots.push(snapshot);
    }
    assert_eq!(configuration_count(&store), 4);
    drop(store);
    let store = SqliteConversationStore::open(conversation, &path).unwrap();
    for snapshot in snapshots {
        assert_eq!(
            store.load_request_snapshot(&snapshot.request_id).unwrap(),
            snapshot
        );
        let mut changed = snapshot.clone();
        changed.tool_definitions.reverse();
        assert!(
            store
                .commit_model_turn_start(&[], &changed, Utc::now())
                .is_err()
        );
    }
    assert_eq!(configuration_count(&store), 4);
}

#[test]
fn shared_native_configuration_corruption_is_rejected_by_scalar_full_and_retry_reads() {
    for corruption in [
        "UPDATE request_configurations SET configuration_json=json_set(configuration_json,'$.effective_system_prompt','tampered')",
        "DELETE FROM request_configurations",
    ] {
        let store = store();
        store.initialize(&[]).unwrap();
        let snapshot = configuration_snapshot(&store, 1);
        store
            .commit_model_turn_start(&[], &snapshot, Utc::now())
            .unwrap();
        store.lock().unwrap().execute_batch(corruption).unwrap();
        {
            let mut connection = store.lock().unwrap();
            let transaction = connection.transaction().unwrap();
            assert!(matches!(
                require_started_request_tx(
                    &transaction,
                    store.conversation_id(),
                    &snapshot.request_id
                ),
                Err(ConversationStoreError::InvalidReference(_))
            ));
        }
        assert!(matches!(
            store.load_request_snapshot(&snapshot.request_id),
            Err(ConversationStoreError::InvalidReference(_))
        ));
        assert!(matches!(
            store.commit_model_turn_start(&[], &snapshot, Utc::now()),
            Err(ConversationStoreError::InvalidReference(_))
        ));
        assert_eq!(store.read_events(None, 32).unwrap().events.len(), 1);
    }
}

#[test]
fn shared_native_configuration_fresh_trigger_mutation_rolls_back_the_configuration_and_start() {
    let store = store();
    store.initialize(&[]).unwrap();
    let snapshot = configuration_snapshot(&store, 1);
    store.lock().unwrap().execute_batch("CREATE TEMP TRIGGER mutate_shared_config AFTER INSERT ON request_snapshots BEGIN UPDATE request_configurations SET configuration_json=json_set(configuration_json,'$.effective_system_prompt','changed after insertion'); END;").unwrap();
    assert!(matches!(
        store.commit_model_turn_start(&[], &snapshot, Utc::now()),
        Err(ConversationStoreError::InvalidReference(_))
    ));
    assert_eq!(configuration_count(&store), 0);
    assert!(store.read_events(None, 32).unwrap().events.is_empty());
    store
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER mutate_shared_config")
        .unwrap();
    store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap();
    let second = configuration_snapshot(&store, 2);
    store.lock().unwrap().execute_batch("CREATE TEMP TRIGGER mutate_existing_shared_config AFTER INSERT ON request_snapshots BEGIN UPDATE request_configurations SET configuration_json=json_set(configuration_json,'$.effective_system_prompt','changed reused configuration'); END;").unwrap();
    assert!(
        store
            .commit_model_turn_start(&[], &second, Utc::now())
            .is_err()
    );
    assert_eq!(configuration_count(&store), 1);
    assert_eq!(
        store.load_request_snapshot(&snapshot.request_id).unwrap(),
        snapshot
    );
    assert!(store.load_request_snapshot(&second.request_id).is_err());
}

#[test]
fn shared_native_configuration_materialization_charges_full_payload_before_allocating_metadata() {
    let store = store();
    store
        .initialize(&[user_message(
            "native-budget-history",
            "history".repeat(64).as_str(),
        )])
        .unwrap();
    let snapshot = configuration_snapshot(&store, 1);
    store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap();
    let flat_bytes = serde_json::to_vec(&snapshot).unwrap().len();
    let metadata_bytes = encode_snapshot(&snapshot).unwrap().len();
    let normalized = stored_configuration_metadata(&store, &snapshot);
    let connection = store.lock().unwrap();
    let (_, charged) = native_request::full(&connection, &normalized, metadata_bytes).unwrap();
    assert_eq!(charged, metadata_bytes);
    REQUEST_SNAPSHOT_READ_WORK.with(|work| work.set((0, 0)));
    assert!(
        load_request_snapshot_bounded(
            &connection,
            store.conversation_id(),
            &snapshot.request_id,
            metadata_bytes - 1
        )
        .is_err()
    );
    assert_eq!(
        REQUEST_SNAPSHOT_READ_WORK.with(|work| work.replace((0, 0))),
        (1, 0)
    );
    assert!(
        load_request_snapshot_bounded(
            &connection,
            store.conversation_id(),
            &snapshot.request_id,
            flat_bytes - 1
        )
        .is_err()
    );
    assert_eq!(
        load_request_snapshot_bounded(
            &connection,
            store.conversation_id(),
            &snapshot.request_id,
            flat_bytes
        )
        .unwrap(),
        snapshot
    );
}

#[test]
fn shared_native_configuration_coherently_rehashed_invalid_types_still_fail_native_validation() {
    let deep = format!("{}0{}", "[".repeat(130), "]".repeat(130));
    let payloads = [
        r#"{"effective_system_prompt":12,"system_sections":[],"tool_definitions":[]}"#.to_owned(),
        r#"{"effective_system_prompt":"ok","system_sections":false,"tool_definitions":[]}"#.to_owned(),
        r#"{"effective_system_prompt":"ok","system_sections":[],"tool_definitions":[{"id":false,"name":"x","description":"x","input_schema":{}}]}"#.to_owned(),
        r#"{"effective_system_prompt":"ok","system_sections":[],"tool_definitions":[],"unknown":"field"}"#.to_owned(),
        r#"{"effective_system_prompt":"ok","system_sections":[],"tool_definitions":[{"id":"x","name":"x","description":"x","input_schema":1e999}]}"#.to_owned(),
        format!(r#"{{"effective_system_prompt":"ok","system_sections":[],"tool_definitions":[{{"id":"x","name":"x","description":"x","input_schema":{deep}}}]}}"#),
    ];
    for payload in payloads {
        let store = store();
        store.initialize(&[]).unwrap();
        let snapshot = configuration_snapshot(&store, 1);
        store
            .commit_model_turn_start(&[], &snapshot, Utc::now())
            .unwrap();
        let mut metadata: serde_json::Value =
            serde_json::from_str(&stored_configuration_metadata(&store, &snapshot)).unwrap();
        let mut hash = Sha256::new();
        hash.update(b"rustx-native-request-configuration-v1\0");
        hash.update(payload.as_bytes());
        let digest = format!("{:x}", hash.finalize());
        metadata["configuration_digest"] = serde_json::json!(digest);
        let metadata = serde_json::to_string(&metadata).unwrap();
        {
            let mut connection = store.lock().unwrap();
            let root = read_stored_snapshot_root(&connection, &snapshot.request_id).unwrap();
            let manifest = stored_snapshot_digest(&metadata, root);
            connection
                .execute(
                    "UPDATE request_configurations SET digest=?1,configuration_json=?2",
                    params![digest, payload],
                )
                .unwrap();
            connection
                .execute(
                    "UPDATE request_snapshots SET snapshot_json=?1,manifest_digest=?2",
                    params![metadata, manifest],
                )
                .unwrap();
            let transaction = connection.transaction().unwrap();
            assert!(
                require_started_request_tx(
                    &transaction,
                    store.conversation_id(),
                    &snapshot.request_id
                )
                .is_err()
            );
        }
        assert!(store.load_request_snapshot(&snapshot.request_id).is_err());
        assert!(
            store
                .commit_model_turn_start(&[], &snapshot, Utc::now())
                .is_err()
        );
        assert_eq!(store.read_events(None, 32).unwrap().events.len(), 1);
    }
}
