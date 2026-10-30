fn complete_tool_row_fixture(store: &SqliteConversationStore) {
    store.initialize(&[]).unwrap();
    let attempt = response_start(store, "tool-row-owner");
    let (mut proposal, mut call) = derived_enrichment_proposal();
    call.arguments = serde_json::from_str(r#"{"a":1,"b":2}"#).unwrap();
    let MessageBlock::Assistant(assistant) = &mut proposal else { unreachable!() };
    assistant.content = vec![AssistantContentBlock::ToolCall(call.clone())];
    store.append_canonical_with_event(&proposal, envelope(
        store.conversation_id(), "tool-row-proposal", Some(attempt.clone()),
        RuntimeEvent::AssistantMessageCommitted { message_id: proposal.id().clone() },
    )).unwrap();
    store.append_event(envelope(
        store.conversation_id(), "tool-row-started", Some(attempt.clone()),
        RuntimeEvent::ToolExecutionStarted { tool_call_id: call.id.clone(), tool_id: call.tool_id.clone() },
    )).unwrap();
    let result = ToolExecutionResult {
        content: vec![crate::tools::types::ToolResultContent::Json {
            value: serde_json::from_str(r#"{"integer":18446744073709551615,"minus":-0.0,"order":{"z":1,"a":2}}"#).unwrap(),
        }],
        duration_ms: u64::MAX,
        ..original_success_tool_result()
    };
    store.append_event(envelope(
        store.conversation_id(), "tool-row-settled", Some(attempt.clone()),
        RuntimeEvent::ToolExecutionCompleted { tool_call_id: call.id.clone(), tool_id: call.tool_id.clone(), result: result.clone() },
    )).unwrap();
    commit_original_tool_result(store, &attempt, &proposal, &call, &result);
    response_message(store, &attempt, "tool-row-final");
    response_complete(store, &attempt);
}

#[test]
fn checkpoint_tool_and_result_rows_reuse_current_native_proofs_across_cuts() {
    let store = store();
    complete_tool_row_fixture(&store);
    let cut = store.observation_read_cut().unwrap();
    let expected = checkpoint_head_oracle(&store, &cut, cut.read.journal, 32).unwrap();
    super::super::observation_response::row_work();
    super::super::observation_tools::page_work();
    let first = store.observation_checkpoint_head(&cut, cut.read.journal, 32).unwrap();
    assert_eq!(first, expected);
    assert_eq!(first.transcript_rows.len(), 3);
    assert!(!first.transcript.entries[0].tool_calls.is_empty());
    assert!(matches!(&first.transcript.entries[1].item,
        crate::runtime_client::snapshot::RuntimeClientTranscriptItem::Message { message: MessageBlock::Tool(_) }));
    assert_eq!(super::super::observation_response::row_work(), (3, 0));
    assert_eq!(super::super::observation_tools::page_work(), 1);
    response_start(&store, "tool-row-unrelated");
    let current = store.observation_read_cut().unwrap();
    let repeated = store.observation_checkpoint_head(&current, current.read.journal, 32).unwrap();
    assert_eq!(super::super::observation_response::row_work(), (0, 3));
    assert_eq!(super::super::observation_tools::page_work(), 1,
        "current Tool facts still use exactly their original mandatory encoding");
    assert_eq!(serde_json::to_vec(&first.transcript.entries).unwrap(), serde_json::to_vec(&repeated.transcript.entries).unwrap());
    assert!(first.transcript_rows.iter().zip(&repeated.transcript_rows).all(|(a,b)| a.same_owner(b)));
    assert_ne!(first.transcript.collections, repeated.transcript.collections,
        "current-cut metadata is not inherited from the row owner");
    let historical = store.observation_checkpoint_head(&cut, cut.read.journal, 32).unwrap();
    assert_eq!(historical, first);
    assert!(first.transcript_rows.iter().zip(&historical.transcript_rows).all(|(a,b)| a.same_owner(b)));
}

#[test]
fn checkpoint_tool_proof_observes_exact_arguments_without_response_cache_invalidation() {
    let store = store();
    complete_tool_row_fixture(&store);
    let cut = store.observation_read_cut().unwrap();
    let first = store.observation_checkpoint_head(&cut, cut.read.journal, 32).unwrap();
    // Change only the original bounded argument bytes, keeping their length and
    // JSON meaning but changing map order. The response owner does not depend
    // on this payload; the Tool proof must independently see its current bytes.
    assert_eq!(store.lock().unwrap().execute(
        "UPDATE content_frontiers SET bytes=?1 WHERE source_kind='canonical' AND source_id='ledger:1' AND block_index=0 AND representation='canonical_json_v1' AND revision=1",
        [br#"{"b":2,"a":1}"#.as_slice()],
    ).unwrap(), 1);
    super::super::observation_response::cache_work();
    super::super::observation_response::row_work();
    let changed = store.observation_checkpoint_head(&cut, cut.read.journal, 32).unwrap();
    assert_eq!(super::super::observation_response::cache_work().0, 3,
        "the response proof alone is unchanged by an argument payload rewrite");
    assert_eq!(super::super::observation_response::row_work(), (1, 2));
    assert!(!first.transcript_rows[0].same_owner(&changed.transcript_rows[0]));
    assert_ne!(serde_json::to_vec(&first.transcript.entries).unwrap(), serde_json::to_vec(&changed.transcript.entries).unwrap());
    assert_eq!(changed, checkpoint_head_oracle(&store, &cut, cut.read.journal, 32).unwrap());
    {
        let connection = store.lock().unwrap();
        connection.execute_batch("SAVEPOINT tool_payload_rollback").unwrap();
        assert_eq!(connection.execute("UPDATE content_frontiers SET bytes=x'ff' WHERE source_kind='canonical' AND source_id='ledger:1' AND block_index=0 AND representation='canonical_json_v1'", []).unwrap(), 1);
        connection.execute_batch("ROLLBACK TO tool_payload_rollback; RELEASE tool_payload_rollback").unwrap();
    }
    let restored = store.observation_checkpoint_head(&cut, cut.read.journal, 32).unwrap();
    assert_eq!(restored, changed);
    assert!(restored.transcript_rows[0].same_owner(&changed.transcript_rows[0]));
    assert_eq!(store.lock().unwrap().execute("UPDATE content_frontiers SET bytes=x'ff' WHERE source_kind='canonical' AND source_id='ledger:1' AND block_index=0 AND representation='canonical_json_v1'", []).unwrap(), 1);
    let expected = checkpoint_head_oracle(&store, &cut, cut.read.journal, 32).unwrap_err();
    let actual = store.observation_checkpoint_head(&cut, cut.read.journal, 32).unwrap_err();
    assert_eq!(actual.to_string(), expected.to_string());
}

#[test]
fn checkpoint_tool_proof_rechecks_external_witness_and_result_corruption() {
    for damage in [
        "UPDATE observation_headers SET tool_result_json=x'7b' WHERE ledger_position=2",
        "UPDATE events SET observation_witness=x'7b' WHERE observation_kind='tool_execution_completed'",
        "UPDATE canonical_tool_calls SET observation_name='changed exact native name' WHERE observation_ledger=1",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("tool-proof.sqlite");
        let conversation = ConversationId::generate();
        let writer = SqliteConversationStore::open(conversation.clone(), &path).unwrap();
        complete_tool_row_fixture(&writer);
        let reader = SqliteConversationStore::open_existing(conversation, &path).unwrap();
        let cut = reader.observation_read_cut().unwrap();
        let original = reader.observation_checkpoint_head(&cut, cut.read.journal, 32).unwrap();
        assert_eq!(writer.lock().unwrap().execute(damage, []).unwrap(), 1);
        let expected = checkpoint_head_oracle(&reader, &cut, cut.read.journal, 32);
        let actual = reader.observation_checkpoint_head(&cut, cut.read.journal, 32);
        match (expected, actual) {
            (Ok(expected), Ok(actual)) => {
                assert_eq!(actual, expected);
                assert_ne!(actual.transcript.entries, original.transcript.entries);
                assert!(!actual.transcript_rows[0].same_owner(&original.transcript_rows[0]));
            }
            (Err(expected), Err(actual)) => assert_eq!(actual.to_string(), expected.to_string()),
            (expected, actual) => panic!("native error order differs for {damage}: {expected:?} vs {actual:?}"),
        }
    }
}

#[test]
fn checkpoint_tool_proof_retention_obeys_one_page_byte_budget_and_row_limit() {
    let store = store();
    store.initialize(&[]).unwrap();
    let attempt = response_start(&store, "bounded-tool-proofs");
    for index in 0..32 {
        let (mut proposal, _) = derived_enrichment_proposal();
        let MessageBlock::Assistant(assistant) = &mut proposal else { unreachable!() };
        assistant.id = MessageId::new(format!("bounded-proposal-{index}"));
        let AssistantContentBlock::ToolCall(call) = &mut assistant.content[0] else { unreachable!() };
        call.id = ToolCallId::new(format!("bounded-call-{index}"));
        store.append_canonical_with_event(&proposal, envelope(
            store.conversation_id(), &format!("bounded-proposal-commit-{index}"), Some(attempt.clone()),
            RuntimeEvent::AssistantMessageCommitted { message_id: proposal.id().clone() },
        )).unwrap();
    }
    response_complete(&store, &attempt);
    // Both cells remain within their original native 16 KiB limits. Padding
    // exercises retained source bytes without enlarging the rendered Tool body.
    let padding = "p".repeat(13_000);
    assert_eq!(store.lock().unwrap().execute("UPDATE observation_headers SET header_json=CAST(json_set(header_json,'$.source_padding',?1) AS BLOB) WHERE ledger_position IS NOT NULL", [&padding]).unwrap(), 32);
    assert_eq!(store.lock().unwrap().execute("UPDATE observation_block_headers SET header_json=CAST(json_set(header_json,'$.value.source_padding',?1) AS BLOB)", [&padding]).unwrap(), 32);
    let cut = store.observation_read_cut().unwrap();
    let expected = checkpoint_head_oracle(&store, &cut, cut.read.journal, 32).unwrap();
    let actual = store.observation_checkpoint_head(&cut, cut.read.journal, 32).unwrap();
    assert_eq!(actual, expected);
    assert_eq!(actual.transcript_rows.len(), 32);
    let (rows, bytes) = store.conn.response.row_retention();
    assert!(rows > 0 && rows < 32, "byte pressure, not just the row count, evicts old proofs");
    assert!(bytes <= 786_432);
}

#[test]
fn checkpoint_running_tool_body_reuses_only_body_and_reads_current_process_and_state() {
    let store = store();
    store.initialize(&[]).unwrap();
    let attempt = response_start(&store, "running-tool-body");
    let (proposal, call) = derived_enrichment_proposal();
    store.append_canonical_with_event(&proposal, envelope(store.conversation_id(), "running-tool-proposal", Some(attempt.clone()), RuntimeEvent::AssistantMessageCommitted { message_id: proposal.id().clone() })).unwrap();
    let first_cut = store.observation_read_cut().unwrap();
    let first = store.observation_checkpoint_head(&first_cut, first_cut.read.journal, 32).unwrap();
    assert_eq!(store.conn.response.len(), 0, "running facts do not authorize source-read caching");
    response_message(&store, &attempt, "running-tool-second-message");
    let changed_cut = store.observation_read_cut().unwrap();
    let changed = store.observation_checkpoint_head(&changed_cut, changed_cut.read.journal, 32).unwrap();
    assert_eq!(changed, checkpoint_head_oracle(&store, &changed_cut, changed_cut.read.journal, 32).unwrap());
    assert!(first.transcript_rows[0].same_body(&changed.transcript_rows[0]));
    assert!(!first.transcript_rows[0].same_owner(&changed.transcript_rows[0]));
    assert_eq!(first.transcript.entries[0].turn_process.as_ref().unwrap().message_count, 1);
    assert_eq!(changed.transcript.entries[0].turn_process.as_ref().unwrap().message_count, 2);
    store.append_event(envelope(store.conversation_id(), "running-tool-started", Some(attempt), RuntimeEvent::ToolExecutionStarted { tool_call_id: call.id, tool_id: call.tool_id })).unwrap();
    let running_cut = store.observation_read_cut().unwrap();
    let running = store.observation_checkpoint_head(&running_cut, running_cut.read.journal, 32).unwrap();
    assert_eq!(running, checkpoint_head_oracle(&store, &running_cut, running_cut.read.journal, 32).unwrap());
    assert!(!changed.transcript_rows[0].same_body(&running.transcript_rows[0]), "Tool state belongs to the body proof, not just response decoration");
    assert_ne!(changed.transcript.entries[0].tool_calls, running.transcript.entries[0].tool_calls);
    let historical = store.observation_checkpoint_head(&first_cut, first_cut.read.journal, 32).unwrap();
    assert_eq!(historical, first);
    assert_eq!(store.conn.response.len(), 0);
}

#[test]
fn checkpoint_body_proof_rechecks_transactional_source_and_rollback() {
    let store = store();
    store.initialize(&[]).unwrap();
    let attempt = response_start(&store, "body-rollback");
    response_message(&store, &attempt, "body-rollback-message");
    let cut = store.observation_read_cut().unwrap();
    let original = store.observation_checkpoint_head(&cut, cut.read.journal, 32).unwrap();
    {
        let mut connection = store.lock().unwrap();
        let transaction = connection.transaction().unwrap();
        assert_eq!(transaction.execute("UPDATE observation_headers SET header_json=CAST(json_set(header_json,'$.display.content[0].text','provisional different body') AS BLOB) WHERE ledger_position=1", []).unwrap(), 1);
        let provisional = super::super::observation_checkpoint::read(&transaction, &store.conn.response, store.conversation_id(), &cut, cut.read.journal, 32).unwrap();
        assert_ne!(provisional.transcript.entries, original.transcript.entries);
        assert!(!provisional.transcript_rows[0].same_body(&original.transcript_rows[0]));
        transaction.rollback().unwrap();
    }
    let restored = store.observation_checkpoint_head(&cut, cut.read.journal, 32).unwrap();
    assert_eq!(restored, original);
    assert_eq!(restored, checkpoint_head_oracle(&store, &cut, cut.read.journal, 32).unwrap());
    assert_eq!(store.conn.response.len(), 0, "body computation reuse grants no provisional response authority");
}

#[test]
fn checkpoint_tool_body_proof_preserves_native_admission_boundary_and_current_metadata() {
    let store = store();
    complete_tool_row_fixture(&store);
    while store.observation_read_cut().unwrap().read.journal < 9 {
        let sequence = store.observation_read_cut().unwrap().read.journal;
        response_start(&store, &format!("tool-boundary-{sequence}"));
    }
    let cut = store.observation_read_cut().unwrap();
    assert_eq!(cut.read.journal, 9);
    let bytes: Vec<u8> = store.lock().unwrap().query_row("SELECT header_json FROM observation_headers WHERE ledger_position=1", [], |row| row.get(0)).unwrap();
    let mut header: crate::durable::observation::MessageHeader = serde_json::from_slice(&bytes).unwrap();
    let Some(MessageBlock::Assistant(assistant)) = &mut header.display else { unreachable!() };
    let AssistantContentBlock::ToolCall(call) = &mut assistant.content[0] else { unreachable!() };
    call.arguments["padding"] = serde_json::Value::String(String::new());
    let maximum = 16_384 - serde_json::to_vec(&header).unwrap().len();
    let set_padding = |length: usize| {
        let mut header = header.clone();
        let Some(MessageBlock::Assistant(assistant)) = &mut header.display else { unreachable!() };
        let AssistantContentBlock::ToolCall(call) = &mut assistant.content[0] else { unreachable!() };
        call.arguments["padding"] = serde_json::Value::String("p".repeat(length));
        store.lock().unwrap().execute("UPDATE observation_headers SET header_json=?1 WHERE ledger_position=1", [serde_json::to_vec(&header).unwrap()]).unwrap();
    };
    // Locate the actual legacy admission boundary, not an assumed byte charge.
    let (mut low, mut high) = (0, maximum + 1);
    while low + 1 < high {
        let middle = usize::midpoint(low, high);
        set_padding(middle);
        let page = store.transcript_header_page(&cut, None, 32).unwrap();
        if page.headers[0].tool_calls.len() == 1 { low = middle; } else { high = middle; }
    }
    assert!(low > 0 && low < maximum);
    set_padding(low);
    let first = store.observation_checkpoint_head(&cut, cut.read.journal, 32).unwrap();
    assert_eq!(first.transcript.entries[0].tool_calls.len(), 1);
    response_start(&store, "tool-boundary-ten");
    let next = store.observation_read_cut().unwrap();
    assert_eq!(next.read.journal, 10);
    let changed = store.observation_checkpoint_head(&next, next.read.journal, 32).unwrap();
    assert_eq!(changed, checkpoint_head_oracle(&store, &next, next.read.journal, 32).unwrap());
    assert_ne!(first.transcript.collections, changed.transcript.collections);
    if first.transcript.entries[0].tool_calls.len() == changed.transcript.entries[0].tool_calls.len() {
        assert!(first.transcript_rows[0].same_body(&changed.transcript_rows[0]));
    } else {
        assert!(!first.transcript_rows[0].same_body(&changed.transcript_rows[0]));
    }
    set_padding(low + 1);
    let rejected = store.observation_checkpoint_head(&next, next.read.journal, 32).unwrap();
    assert_eq!(rejected, checkpoint_head_oracle(&store, &next, next.read.journal, 32).unwrap());
    assert!(rejected.transcript.entries[0].tool_calls.is_empty());
    assert!(!first.transcript_rows[0].same_body(&rejected.transcript_rows[0]));
    assert_eq!(first.transcript.entries[0].tool_calls.len(), 1);
}
