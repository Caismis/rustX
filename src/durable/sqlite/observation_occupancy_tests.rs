// Native original requests remain intact; occupancy reads only finite committed scalars.
fn occupancy_original_snapshot(store: &SqliteConversationStore) -> RequestSnapshot {
    let mut snapshot = progress_request_snapshot(
        "occupancy-source",
        "1",
        0,
        store.load_head().unwrap().revision,
        Vec::new(),
    );
    snapshot.invocation.model = "\u{feff}🙂水".repeat(120_000);
    snapshot.effective_system_prompt = "\u{feff} native prompt\n🙂水".repeat(70_000);
    snapshot.tool_definitions = vec![crate::tools::types::ModelToolDefinition {
        id: ToolId::new("original-tool"),
        name: "originalTool".into(),
        description: "native original schema".repeat(80_000),
        input_schema: serde_json::json!({"n":u64::MAX,"zero":-0.0,"string":"水".repeat(400_000)}),
    }];
    snapshot
}
#[test]
fn t08_t14_t19_occupancy_reads_fixed_original_request_scalars_and_model_ranges() {
    let store = store();
    store
        .initialize(&[user_message("occupancy-input", "original")])
        .unwrap();
    let mut snapshot = occupancy_original_snapshot(&store);
    let usage = crate::model::types::ModelUsage {
        input_tokens: 3_000_000,
        output_tokens: 1,
        total_tokens: 3_000_001,
        details: None,
    };
    let expected = crate::context::occupancy::measure(&usage, &snapshot)
        .unwrap()
        .unwrap();
    freeze_test_input(&store, &mut snapshot, &[]);
    let start = store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap();
    let mut terminal = envelope(
        store.conversation_id(),
        "occupancy-terminal",
        Some(snapshot.identity.attempt_id.clone()),
        RuntimeEvent::ModelRequestCompleted {
            request_id: snapshot.request_id.clone(),
            finish_reason: ModelFinishReason::Stop,
            usage: Some(usage),
            generation: None,
        },
    );
    terminal.turn_id = Some(snapshot.identity.turn.clone());
    let terminal = store.append_event(terminal).unwrap();
    let request_reads = store.request_snapshot_body_reads.load(Ordering::SeqCst);
    let message_reads = store.checkpoint_read_counts[0].load(Ordering::SeqCst);
    let observed = store
        .observation_occupancy(terminal.sequence)
        .unwrap()
        .unwrap();
    assert_eq!(observed.input_tokens, expected.input_tokens);
    assert_eq!(
        observed.context_window_tokens,
        expected.context_window_tokens
    );
    assert_eq!(observed.breakdown, expected.breakdown);
    assert!(observed.model.is_none());
    assert!(observed.model_deferred);
    assert_eq!(
        observed.model_source,
        Some(format!(
            "trace.records.{}.detail.request.model",
            start.started.sequence
        ))
    );
    assert_eq!(
        store.request_snapshot_body_reads.load(Ordering::SeqCst),
        request_reads
    );
    assert_eq!(
        store.checkpoint_read_counts[0].load(Ordering::SeqCst),
        message_reads
    );
    let (rows, bytes) = observation_occupancy::read_work();
    assert_eq!(rows, 1);
    assert!(bytes <= 16_384);
    let mut original = Vec::new();
    let mut offset = 0;
    loop {
        let chunk = store
            .request_value_read(
                &snapshot.request_id,
                terminal.sequence,
                &["model".into()],
                offset,
                65_536,
            )
            .unwrap();
        assert!(chunk.bytes.len() <= 65_536);
        original.extend(chunk.bytes);
        let Some(next) = chunk.next_offset else {
            break;
        };
        assert!(next > offset);
        offset = next;
    }
    assert_eq!(
        serde_json::from_slice::<String>(&original).unwrap(),
        snapshot.invocation.model
    );
    assert_eq!(
        store.observation_occupancy(start.started.sequence).unwrap(),
        None
    );
}

#[test]
fn t14_t19_request_tool_fingerprints_preserve_native_partial_eq() {
    let tool = |input_schema| crate::tools::types::ModelToolDefinition {
        id: ToolId::new("fingerprint-tool"),
        name: "native".into(),
        description: "original".into(),
        input_schema,
    };
    let left = vec![tool(
        serde_json::from_str(r#"{"zero":-0.0,"ordered":{"a":1,"b":2},"large":"original"}"#)
            .unwrap(),
    )];
    let right = vec![tool(
        serde_json::from_str(r#"{"large":"original","ordered":{"b":2,"a":1},"zero":0.0}"#).unwrap(),
    )];
    assert_eq!(left, right);
    assert_eq!(
        observation_request_summary::tools_digest(&left),
        observation_request_summary::tools_digest(&right)
    );
    let different = vec![tool(
        serde_json::json!({"zero":0,"ordered":{"a":1,"b":2},"large":"original"}),
    )];
    assert_ne!(left, different);
    assert_ne!(
        observation_request_summary::tools_digest(&left),
        observation_request_summary::tools_digest(&different)
    );
    let object = |reverse: bool| {
        let mut values = serde_json::Map::new();
        for ordinal in 0..4097 {
            let ordinal = if reverse { 4096 - ordinal } else { ordinal };
            values.insert(
                format!("opaque-key-{ordinal}"),
                serde_json::json!({"nested": [ordinal, null, "original"]}),
            );
        }
        vec![tool(serde_json::Value::Object(values))]
    };
    let forward = object(false);
    let reverse = object(true);
    assert_eq!(forward, reverse);
    assert_eq!(
        observation_request_summary::tools_digest(&forward),
        observation_request_summary::tools_digest(&reverse)
    );
    let ordered = vec![left[0].clone(), different[0].clone()];
    let reordered = vec![different[0].clone(), left[0].clone()];
    assert_ne!(ordered, reordered);
    assert_ne!(
        observation_request_summary::tools_digest(&ordered),
        observation_request_summary::tools_digest(&reordered)
    );
    for field in 0..3 {
        let mut changed = left.clone();
        match field {
            0 => changed[0].id = ToolId::new("different-native-id"),
            1 => changed[0].name.push('!'),
            _ => changed[0].description.push('!'),
        }
        assert_ne!(left, changed);
        assert_ne!(
            observation_request_summary::tools_digest(&left),
            observation_request_summary::tools_digest(&changed)
        );
    }
    let first = vec![tool(serde_json::json!({"large":"a".repeat(2_100_000)}))];
    let second = vec![tool(serde_json::json!({"large":"a".repeat(2_099_999)+"b"}))];
    assert_ne!(first, second);
    assert_ne!(
        observation_request_summary::tools_digest(&first),
        observation_request_summary::tools_digest(&second)
    );
}

#[test]
fn t10_t14_occupancy_derived_header_failure_preserves_original_provider_terminal() {
    let store = store();
    store
        .initialize(&[user_message("occupancy-fault-input", "original")])
        .unwrap();
    let mut snapshot = progress_request_snapshot(
        "occupancy-fault",
        "1",
        0,
        store.load_head().unwrap().revision,
        Vec::new(),
    );
    freeze_test_input(&store, &mut snapshot, &[]);
    store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap();
    store.lock().unwrap().execute_batch("CREATE TRIGGER occupancy_layout_fault BEFORE UPDATE OF observation_occupancy ON events BEGIN SELECT RAISE(FAIL,'derived occupancy fault'); END;").unwrap();
    let mut terminal = envelope(
        store.conversation_id(),
        "occupancy-fault-terminal",
        Some(snapshot.identity.attempt_id.clone()),
        RuntimeEvent::ModelRequestCompleted {
            request_id: snapshot.request_id.clone(),
            finish_reason: ModelFinishReason::Stop,
            usage: Some(crate::model::types::ModelUsage {
                input_tokens: 10,
                output_tokens: 1,
                total_tokens: 11,
                details: None,
            }),
            generation: None,
        },
    );
    terminal.turn_id = Some(snapshot.identity.turn.clone());
    let committed = store.append_event(terminal).unwrap();
    assert!(store.observation_occupancy(committed.sequence).is_err());
    let original = store.read_events(Some(committed.sequence - 1), 1).unwrap();
    assert!(matches!(
        original.events[0].event,
        RuntimeEvent::ModelRequestCompleted { .. }
    ));
    assert_eq!(
        store.load_request_snapshot(&snapshot.request_id).unwrap(),
        snapshot
    );
}
