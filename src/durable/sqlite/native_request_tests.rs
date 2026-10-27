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


fn replace_configuration_tx(
    transaction: &Transaction<'_>,
    snapshot: &RequestSnapshot,
    payload: &str,
) {
    let mut metadata: serde_json::Value = transaction.query_row(
        "SELECT snapshot_json FROM request_snapshots WHERE request_id=?1",
        [snapshot.request_id.as_str()], |row| row.get::<_, String>(0),
    ).map(|json| serde_json::from_str(&json).unwrap()).unwrap();
    let old = metadata["configuration_digest"].as_str().unwrap().to_owned();
    let mut hash = Sha256::new();
    hash.update(b"rustx-native-request-configuration-v1\0");
    hash.update(payload.as_bytes());
    let digest = format!("{:x}", hash.finalize());
    metadata["configuration_digest"] = serde_json::json!(digest);
    let metadata = serde_json::to_string(&metadata).unwrap();
    let root = read_stored_snapshot_root(transaction, &snapshot.request_id).unwrap();
    let manifest = stored_snapshot_digest(&metadata, root);
    transaction.execute(
        "UPDATE request_configurations SET digest=?1,configuration_json=?2 WHERE digest=?3",
        params![digest,payload,old],
    ).unwrap();
    transaction.execute(
        "UPDATE request_snapshots SET snapshot_json=?1,manifest_digest=?2 WHERE request_id=?3",
        params![metadata,manifest,snapshot.request_id.as_str()],
    ).unwrap();
}

#[test]
fn native_request_transaction_validation_reuses_exact_typed_ownership_and_live_sequences() {
    let store = store();
    store.initialize(&[]).unwrap();
    let snapshot = configuration_snapshot(&store, 1);
    store.commit_model_turn_start(&[], &snapshot, Utc::now()).unwrap();
    let mut connection = store.lock().unwrap();
    let transaction = connection.transaction().unwrap();
    let bytes: usize = transaction.query_row("SELECT octet_length(configuration_json) FROM request_configurations", [], |row| row.get(0)).unwrap();
    native_request::CONFIGURATION_DECODES.with(|work| work.set(0));
    native_request::CONFIGURATION_READ_WORK.with(|work| work.set((0, 0)));
    native_request::CONFIGURATION_BYTE_PROBES.with(|work| work.set(0));
    let mut validation = native_request::Validation::new(&transaction);
    let (first, started, completed) = require_started_request_with_validation(&mut validation, store.conversation_id(), &snapshot.request_id).unwrap();
    assert!(completed.is_none());
    for _ in 0..3 {
        let (again, actual_started, completed) = require_started_request_with_validation(&mut validation, store.conversation_id(), &snapshot.request_id).unwrap();
        assert!(Arc::ptr_eq(&first.0, &again.0));
        assert_eq!(actual_started, started);
        assert!(completed.is_none());
        assert_eq!(serde_json::to_string(&again.tool_definitions).unwrap(), serde_json::to_string(&snapshot.tool_definitions).unwrap());
        assert_eq!(again.effective_system_prompt, snapshot.effective_system_prompt);
    }
    transaction.execute("UPDATE request_snapshots SET completed_sequence=123 WHERE request_id=?1", [snapshot.request_id.as_str()]).unwrap();
    let (again, _, completed) = require_started_request_with_validation(&mut validation, store.conversation_id(), &snapshot.request_id).unwrap();
    assert!(Arc::ptr_eq(&first.0, &again.0));
    assert_eq!(completed, Some(123));
    assert_eq!(native_request::CONFIGURATION_DECODES.with(std::cell::Cell::get), 1);
    // Payload bytes delivered to Rust, not SQLite page reads: exact SQL byte
    // comparisons still inspect the current cell on all four reuses.
    assert_eq!(native_request::CONFIGURATION_READ_WORK.with(std::cell::Cell::get), (1, bytes));
    assert_eq!(native_request::CONFIGURATION_BYTE_PROBES.with(std::cell::Cell::get), 4);
}

#[test]
fn native_request_transaction_validation_rechecks_direct_sql_bytes_and_storage_type() {
    for mutation in [
        "UPDATE request_configurations SET configuration_json=' '||configuration_json",
        "UPDATE request_configurations SET configuration_json=CAST(configuration_json AS BLOB)",
        "DELETE FROM request_configurations",
    ] {
        let store = store();
        store.initialize(&[]).unwrap();
        let snapshot = configuration_snapshot(&store, 1);
        store.commit_model_turn_start(&[], &snapshot, Utc::now()).unwrap();
        let mut connection = store.lock().unwrap();
        let transaction = connection.transaction().unwrap();
        let mut validation = native_request::Validation::new(&transaction);
        require_started_request_with_validation(&mut validation, store.conversation_id(), &snapshot.request_id).unwrap();
        // Deliberate storage corruption must reach the native reader even
        // though ordinary writes cannot violate the TEXT storage constraint.
        transaction.execute_batch("PRAGMA ignore_check_constraints=ON").unwrap();
        transaction.execute_batch(mutation).unwrap();
        assert!(require_started_request_with_validation(&mut validation, store.conversation_id(), &snapshot.request_id).is_err(), "{mutation}");
    }
}

#[test]
fn native_request_transaction_validation_rejects_coherently_rehashed_invalid_configuration() {
    for payload in [
        r#"{"effective_system_prompt":false,"system_sections":[],"tool_definitions":[]}"#,
        r#"{"effective_system_prompt":"ok","system_sections":[],"tool_definitions":{},"extra":0}"#,
        r#"{"effective_system_prompt":"ok","system_sections":[],"tool_definitions":[{"id":false,"name":"x","description":"x","input_schema":{}}]}"#,
        "{malformed native config",
    ] {
        let store = store();
        store.initialize(&[]).unwrap();
        let snapshot = configuration_snapshot(&store, 1);
        store.commit_model_turn_start(&[], &snapshot, Utc::now()).unwrap();
        let mut connection = store.lock().unwrap();
        let transaction = connection.transaction().unwrap();
        let mut validation = native_request::Validation::new(&transaction);
        let (original, _, _) = require_started_request_with_validation(&mut validation, store.conversation_id(), &snapshot.request_id).unwrap();
        replace_configuration_tx(&transaction, &snapshot, payload);
        assert!(require_started_request_with_validation(&mut validation, store.conversation_id(), &snapshot.request_id).is_err(), "{payload}");
        assert_eq!(original.effective_system_prompt, snapshot.effective_system_prompt);
        assert!(require_started_request_tx(&transaction, store.conversation_id(), &snapshot.request_id).is_err());
    }
}

#[test]
fn native_request_transaction_validation_refreshes_valid_changed_configuration_and_new_transactions() {
    let store = store();
    store.initialize(&[]).unwrap();
    let snapshot = configuration_snapshot(&store, 1);
    store.commit_model_turn_start(&[], &snapshot, Utc::now()).unwrap();
    native_request::CONFIGURATION_DECODES.with(|work| work.set(0));
    {
        let mut connection = store.lock().unwrap();
        let transaction = connection.transaction().unwrap();
        let mut validation = native_request::Validation::new(&transaction);
        let (original, _, _) = require_started_request_with_validation(&mut validation, store.conversation_id(), &snapshot.request_id).unwrap();
        let mut payload: serde_json::Value = transaction.query_row("SELECT configuration_json FROM request_configurations", [], |row| row.get::<_, String>(0)).map(|json| serde_json::from_str(&json).unwrap()).unwrap();
        payload["effective_system_prompt"] = serde_json::json!("changed valid frozen configuration 水");
        replace_configuration_tx(&transaction, &snapshot, &serde_json::to_string(&payload).unwrap());
        let (changed, _, _) = require_started_request_with_validation(&mut validation, store.conversation_id(), &snapshot.request_id).unwrap();
        assert!(!Arc::ptr_eq(&original.0, &changed.0));
        assert_eq!(changed.effective_system_prompt, "changed valid frozen configuration 水");
        assert_eq!(original.effective_system_prompt, snapshot.effective_system_prompt);
        assert_eq!(native_request::CONFIGURATION_DECODES.with(std::cell::Cell::get), 2);
        // Rollback must not lend that changed configuration to the next scope.
    }
    let mut connection = store.lock().unwrap();
    let transaction = connection.transaction().unwrap();
    let (original, _, _) = require_started_request_with_validation(&mut native_request::Validation::new(&transaction), store.conversation_id(), &snapshot.request_id).unwrap();
    assert_eq!(original.effective_system_prompt, snapshot.effective_system_prompt);
    assert_eq!(native_request::CONFIGURATION_DECODES.with(std::cell::Cell::get), 3);
}

#[test]
fn native_request_fresh_configuration_checks_exact_bytes_without_payload_materialization() {
    let store = store();
    store.initialize(&[]).unwrap();
    let snapshot = configuration_snapshot(&store, 1);
    let mut connection = store.lock().unwrap();
    let transaction = connection.transaction().unwrap();
    native_request::CONFIGURATION_DECODES.with(|work| work.set(0));
    native_request::CONFIGURATION_READ_WORK.with(|work| work.set((0, 0)));
    for _ in 0..2 {
        let (_, configuration) = native_request::persist(&transaction, &snapshot).unwrap();
        configuration.authenticate(&transaction).unwrap();
    }
    assert_eq!(native_request::CONFIGURATION_DECODES.with(std::cell::Cell::get), 0);
    assert_eq!(native_request::CONFIGURATION_READ_WORK.with(std::cell::Cell::get), (0, 0));
    let (_, configuration) = native_request::persist(&transaction, &snapshot).unwrap();
    transaction.execute("UPDATE request_configurations SET configuration_json=' '||configuration_json", []).unwrap();
    assert!(configuration.authenticate(&transaction).is_err());
}
