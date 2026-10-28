fn checkpoint_head_oracle(
    store: &SqliteConversationStore,
    cut: &ObservationReadCut,
    through: u64,
    limit: usize,
) -> Result<crate::durable::observation::CheckpointHead, ConversationStoreError> {
    let mut transcript = crate::runtime_client::snapshot::transcript_header_view(
        store.transcript_header_page(cut, None, limit)?,
    );
    crate::runtime_client::response::decorate_through(store, &mut transcript, through)?;
    let occupancy = store.observation_occupancy(through)?;
    let trace_total = store.observation_trace_total(through)?;
    let projection_head = store.observation_projection_head(through)?;
    Ok(crate::durable::observation::CheckpointHead {
        transcript_rows: transcript.entries.iter().cloned().map(|entry| crate::runtime_client::checkpoint::PreparedTranscriptRow::new(cut.read.conversation_id.clone(), entry)).collect(),
        transcript,
        occupancy,
        trace_total,
        projection_head,
    })
}

#[test]
fn checkpoint_head_keeps_one_wal_snapshot_across_concurrent_source_changes() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("checkpoint-head.sqlite");
    let conversation = ConversationId::generate();
    let writer = Arc::new(SqliteConversationStore::open(conversation.clone(), &path).unwrap());
    writer.initialize(&[]).unwrap();
    let attempt = response_start(&writer, "checkpoint-snapshot");
    response_message(&writer, &attempt, "checkpoint-snapshot-message");
    response_complete(&writer, &attempt);
    let reader = SqliteConversationStore::open_existing(conversation, &path).unwrap();
    let cut = reader.observation_read_cut().unwrap();
    let original = checkpoint_head_oracle(&reader, &cut, cut.read.journal, 32).unwrap();
    let reached = Arc::new(AtomicBool::new(false));
    let observed = reached.clone();
    super::super::observation_checkpoint::probe_after_transcript(move || {
        let changed = writer
            .lock()
            .unwrap()
            .execute(
                "UPDATE events SET observation_members=NULL WHERE observation_members_required=1",
                [],
            )
            .unwrap();
        assert!(
            changed > 0,
            "the competing writer must change selected source evidence"
        );
        observed.store(true, Ordering::SeqCst);
    });
    assert_eq!(
        reader
            .observation_checkpoint_head(&cut, cut.read.journal, 32)
            .unwrap(),
        original,
    );
    assert!(reached.load(Ordering::SeqCst));
    let expected = checkpoint_head_oracle(&reader, &cut, cut.read.journal, 32).unwrap_err();
    let actual = reader
        .observation_checkpoint_head(&cut, cut.read.journal, 32)
        .unwrap_err();
    assert_eq!(actual.to_string(), expected.to_string());
    assert!(matches!(actual, ConversationStoreError::Storage(_)));
}

#[test]
fn checkpoint_head_preserves_cut_bound_and_source_error_order() {
    let store = store();
    store.initialize(&[]).unwrap();
    let attempt = response_start(&store, "checkpoint-errors");
    response_message(&store, &attempt, "checkpoint-errors-message");
    response_complete(&store, &attempt);
    let cut = store.observation_read_cut().unwrap();
    let mut foreign = cut.clone();
    foreign.read.conversation_id = ConversationId::generate();
    let mut mutated = cut.clone();
    mutated.read.mutation_revision += 1;
    let mut future = cut.clone();
    future.read.journal += 1;
    for (selected, through, limit) in [
        (&foreign, cut.read.journal + 1, 0),
        (&mutated, cut.read.journal, 32),
        (&future, future.read.journal, 32),
        (&cut, cut.read.journal, 0),
        (&cut, cut.read.journal, 33),
        (&cut, cut.read.journal + 1, 32),
    ] {
        let expected = checkpoint_head_oracle(&store, selected, through, limit).unwrap_err();
        let actual = store
            .observation_checkpoint_head(selected, through, limit)
            .unwrap_err();
        assert_eq!(actual.to_string(), expected.to_string());
        assert_eq!(
            std::mem::discriminant(&actual),
            std::mem::discriminant(&expected)
        );
    }
    for damage in [
        "UPDATE observation_headers SET header_json=x'7b'",
        "UPDATE events SET observation_members=NULL WHERE observation_members_required=1",
        "UPDATE bootstrap_identity SET occupancy_observation=x'7b' WHERE id=1",
        "UPDATE events SET observation_trace_required=1,observation_trace_total=NULL",
    ] {
        let damaged = super::store();
        damaged.initialize(&[]).unwrap();
        let attempt = response_start(&damaged, "checkpoint-damaged");
        response_message(&damaged, &attempt, "checkpoint-damaged-message");
        response_complete(&damaged, &attempt);
        let cut = damaged.observation_read_cut().unwrap();
        damaged.lock().unwrap().execute_batch(damage).unwrap();
        let expected = checkpoint_head_oracle(&damaged, &cut, cut.read.journal, 32).unwrap_err();
        let actual = damaged
            .observation_checkpoint_head(&cut, cut.read.journal, 32)
            .unwrap_err();
        assert_eq!(actual.to_string(), expected.to_string(), "{damage}");
        assert_eq!(
            std::mem::discriminant(&actual),
            std::mem::discriminant(&expected)
        );
    }
}

#[test]
fn checkpoint_head_keeps_projection_and_occupancy_at_the_same_historical_cut() {
    let store = store();
    store
        .initialize(&[user_message("checkpoint-head-original", "original")])
        .unwrap();
    let before = store.observation_read_cut().unwrap();
    let before_head = checkpoint_head_oracle(&store, &before, before.read.journal, 32).unwrap();
    let receipt = store
        .commit_projection(observation_projection_input(
            &store,
            "checkpoint-head-activation",
        ))
        .unwrap();
    let cut = store.observation_read_cut().unwrap();
    let expected = checkpoint_head_oracle(&store, &cut, cut.read.journal, 32).unwrap();
    assert!(expected.occupancy.is_some());
    assert_eq!(
        expected.projection_head.as_ref().unwrap().activation_id,
        receipt.checkpoint.activation.activation
    );
    assert_eq!(
        store
            .observation_checkpoint_head(&before, before.read.journal, 32)
            .unwrap(),
        before_head
    );
    assert_eq!(
        store
            .observation_checkpoint_head(&cut, cut.read.journal, 32)
            .unwrap(),
        expected
    );
    store
        .lock()
        .unwrap()
        .execute(
            "UPDATE projection_activations SET observation_header=x'7b'",
            [],
        )
        .unwrap();
    let expected = checkpoint_head_oracle(&store, &cut, cut.read.journal, 32).unwrap_err();
    let actual = store
        .observation_checkpoint_head(&cut, cut.read.journal, 32)
        .unwrap_err();
    assert_eq!(actual.to_string(), expected.to_string());
    assert_eq!(
        store
            .observation_checkpoint_head(&before, before.read.journal, 32)
            .unwrap(),
        before_head
    );
}

#[test]
fn checkpoint_read_reuses_one_actual_native_head_for_response_and_statistics() {
    let store = store();
    store.initialize(&[user_message("verified-cut-user", "exact user body")]).unwrap();
    let attempt = response_start(&store, "verified-cut");
    response_message(&store, &attempt, "verified-cut-message");
    response_complete(&store, &attempt);
    let cut = store.observation_read_cut().unwrap();
    let expected = checkpoint_head_oracle(&store, &cut, cut.read.journal, 32).unwrap();
    // SQLite's statement RUN counter records actual executions, including
    // cached statements. It is not an instrumentation counter beside the code.
    let queries = [
        super::super::reading::CUT_SQL,
        "SELECT next_event_sequence,(SELECT COALESCE(MAX(position),0) FROM message_ledger),next_transcript_position FROM rustx_store WHERE id=1",
        "SELECT next_event_sequence FROM rustx_store WHERE id=1",
        "SELECT EXISTS(SELECT 1 FROM events INDEXED BY events_aggregate_holes_idx WHERE observation_aggregate_holes>0 AND observation_aggregate_holes IN (4,5,6,7) AND sequence<=?1)",
    ];
    {
        let connection = store.lock().unwrap();
        for query in queries {
            connection.prepare_cached(query).unwrap().reset_status(rusqlite::StatementStatus::Run);
        }
    }
    let actual = store.observation_checkpoint_head(&cut, cut.read.journal, 32).unwrap();
    assert_eq!(actual, expected);
    let connection = store.lock().unwrap();
    let counts: Vec<_> = queries.into_iter().map(|query| connection.prepare_cached(query).unwrap().get_status(rusqlite::StatementStatus::Run)).collect();
    assert_eq!(counts, [1, 0, 0, 1], "one actual cut; neither redundant native-head query; statistics hole proof still executes");
}

#[test]
fn transaction_cut_proof_rejects_writes_and_savepoint_rollback_without_reusing_facts() {
    for rollback in [false, true] {
        let store = store();
        store.initialize(&[user_message("proof-mutation", "original")]).unwrap();
        let cut = store.observation_read_cut().unwrap();
        let mut connection = store.lock().unwrap();
        let transaction = connection.transaction().unwrap();
        let proof = super::super::observation::ValidatedCut::new(&transaction, store.conversation_id(), &cut).unwrap();
        let expected = super::super::observation_statistics::read_validated(&proof).unwrap();
        if rollback {
            transaction.execute_batch("SAVEPOINT after_read_proof").unwrap();
        }
        assert!(transaction.execute("UPDATE observation_headers SET header_json=header_json", []).unwrap() > 0);
        if rollback {
            transaction.execute_batch("ROLLBACK TO after_read_proof; RELEASE after_read_proof").unwrap();
        }
        let error = super::super::observation_statistics::read_validated(&proof).unwrap_err();
        assert!(matches!(error, ConversationStoreError::Storage(ref text) if text == "validated observation transaction changed"));
        // Revalidation in the current write transaction remains supported. It
        // cannot admit response cache entries, but its native reads are valid.
        let fresh = super::super::observation::ValidatedCut::new(&transaction, store.conversation_id(), &cut).unwrap();
        assert_eq!(super::super::observation_statistics::read_validated(&fresh).unwrap(), expected);
    }
}

#[test]
fn checkpoint_shared_cut_preserves_order_when_multiple_native_domains_are_corrupt() {
    for damage in [
        "UPDATE observation_headers SET header_json=x'7b'; UPDATE events SET observation_statistics=x'7b' WHERE observation_statistics IS NOT NULL",
        "UPDATE events SET observation_members=NULL WHERE observation_members_required=1; UPDATE events SET observation_statistics=x'7b' WHERE observation_statistics IS NOT NULL",
        "UPDATE events SET observation_statistics=x'7b' WHERE observation_statistics IS NOT NULL; UPDATE bootstrap_identity SET occupancy_observation=x'7b' WHERE id=1",
        "UPDATE bootstrap_identity SET occupancy_observation=x'7b' WHERE id=1; UPDATE events SET observation_trace_required=1,observation_trace_total=NULL",
    ] {
        let store = store();
        store.initialize(&[]).unwrap();
        let attempt = response_start(&store, "proof-error-order");
        response_message(&store, &attempt, "proof-error-order-message");
        response_complete(&store, &attempt);
        let cut = store.observation_read_cut().unwrap();
        store.lock().unwrap().execute_batch(damage).unwrap();
        let expected = checkpoint_head_oracle(&store, &cut, cut.read.journal, 32).unwrap_err();
        let actual = store.observation_checkpoint_head(&cut, cut.read.journal, 32).unwrap_err();
        assert_eq!(std::mem::discriminant(&actual), std::mem::discriminant(&expected), "{damage}");
        assert_eq!(actual.to_string(), expected.to_string(), "{damage}");
    }
}

#[test]
fn checkpoint_complete_rows_reuse_only_verified_body_and_response_owners() {
    let store = store();
    store.initialize(&[user_message("prepared-native-user", "original user body")]).unwrap();
    let attempt = response_start(&store, "prepared-native");
    response_message(&store, &attempt, "prepared-native-message");
    response_complete(&store, &attempt);
    let before = store.observation_read_cut().unwrap();
    let original = store.observation_checkpoint_head(&before, before.read.journal, 32).unwrap();
    assert_eq!(original.transcript_rows.len(), 2, "both ordinary user and completed assistant are in the real fixture");
    let _active = response_start(&store, "prepared-native-unrelated");
    let cut = store.observation_read_cut().unwrap();
    let repeated = store.observation_checkpoint_head(&cut, cut.read.journal, 32).unwrap();
    assert_eq!(original.transcript.entries, repeated.transcript.entries);
    for (first, second) in original.transcript_rows.iter().zip(&repeated.transcript_rows) {
        assert!(first.same_owner(second), "unchanged exact body and authenticated response retain one complete-row owner");
    }
    let historical = store.observation_checkpoint_head(&before, before.read.journal, 32).unwrap();
    assert_eq!(historical.transcript, original.transcript);
    assert!(historical.transcript_rows.iter().zip(&original.transcript_rows).all(|(a,b)| a.same_owner(b)));
    // A coherent descriptor rewrite at the same cursor must not reuse the old
    // row merely because its response facts/cursor are unchanged.
    {
        let connection = store.lock().unwrap();
        let bytes: Vec<u8> = connection.query_row("SELECT header_json FROM observation_headers WHERE ledger_position=1", [], |row| row.get(0)).unwrap();
        let mut header: crate::durable::observation::MessageHeader = serde_json::from_slice(&bytes).unwrap();
        header.display = Some(user_message("prepared-native-user", "coherently changed user body"));
        connection.execute("UPDATE observation_headers SET header_json=?1 WHERE ledger_position=1", [serde_json::to_vec(&header).unwrap()]).unwrap();
    }
    let changed = store.observation_checkpoint_head(&cut, cut.read.journal, 32).unwrap();
    assert!(!original.transcript_rows[0].same_owner(&changed.transcript_rows[0]));
    assert_ne!(original.transcript_rows[0].value(), changed.transcript_rows[0].value());
    assert_eq!(changed, checkpoint_head_oracle(&store, &cut, cut.read.journal, 32).unwrap());
    assert_eq!(original.transcript_rows[0].value(), original.transcript.entries[0], "an old owner still discloses its original body");
    store.lock().unwrap().execute("UPDATE observation_headers SET header_json=x'7b' WHERE ledger_position=1", []).unwrap();
    let expected = checkpoint_head_oracle(&store, &cut, cut.read.journal, 32).unwrap_err();
    let actual = store.observation_checkpoint_head(&cut, cut.read.journal, 32).unwrap_err();
    assert_eq!(actual.to_string(), expected.to_string());
}

#[test]
fn checkpoint_tool_rows_share_cut_proof_but_keep_each_native_tool_validation() {
    let store = store();
    store.initialize(&[]).unwrap();
    let attempt = response_start(&store, "verified-tool-rows");
    for index in 0..2 {
        let (mut proposal, _) = derived_enrichment_proposal();
        let MessageBlock::Assistant(assistant) = &mut proposal else { unreachable!() };
        assistant.id = MessageId::new(format!("verified-tool-row-{index}"));
        let AssistantContentBlock::ToolCall(call) = &mut assistant.content[0] else { unreachable!() };
        call.id = ToolCallId::new(format!("verified-tool-call-{index}"));
        store.append_canonical_with_event(&proposal, envelope(
            store.conversation_id(),
            &format!("verified-tool-commit-{index}"),
            Some(attempt.clone()),
            RuntimeEvent::AssistantMessageCommitted { message_id: proposal.id().clone() },
        )).unwrap();
    }
    response_complete(&store, &attempt);
    let cut = store.observation_read_cut().unwrap();
    let expected = checkpoint_head_oracle(&store, &cut, cut.read.journal, 32).unwrap();
    let query = super::super::reading::CUT_SQL;
    store.lock().unwrap().prepare_cached(query).unwrap().reset_status(rusqlite::StatementStatus::Run);
    super::super::observation_response::row_work();
    super::super::observation_tools::page_work();
    let actual = store.observation_checkpoint_head(&cut, cut.read.journal, 32).unwrap();
    assert_eq!(actual, expected);
    assert_eq!(actual.transcript.entries.len(), 2);
    assert!(actual.transcript.entries.iter().all(|entry| !entry.tool_calls.is_empty()));
    assert_eq!(store.lock().unwrap().prepare_cached(query).unwrap().get_status(rusqlite::StatementStatus::Run), 1);
    assert_eq!(super::super::observation_response::row_work(), (2, 0));
    assert_eq!(super::super::observation_tools::page_work(), 2);
    let repeated = store.observation_checkpoint_head(&cut, cut.read.journal, 32).unwrap();
    assert!(actual.transcript_rows.iter().zip(&repeated.transcript_rows).all(|(a,b)| a.same_owner(b)), "independently validated Tool inputs retain the complete immutable row");
    assert_eq!(super::super::observation_response::row_work(), (0, 2));
    assert_eq!(super::super::observation_tools::page_work(), 2, "proof retention reuses the mandatory native encodes, without another encoding pass");
    // Transcript rows are visited newest first. Keep JSON syntax and the indexed
    // kind valid, but make the second Tool's typed block invalid. The reader must
    // reject that block instead of inheriting row one's proof.
    assert_eq!(store.lock().unwrap().execute("UPDATE observation_block_headers SET header_json=CAST(json_set(header_json,'$.deferred','not-a-bool') AS BLOB) WHERE ledger_position=1", []).unwrap(), 1);
    let expected = checkpoint_head_oracle(&store, &cut, cut.read.journal, 32).unwrap_err();
    store.lock().unwrap().prepare_cached(query).unwrap().reset_status(rusqlite::StatementStatus::Run);
    let actual = store.observation_checkpoint_head(&cut, cut.read.journal, 32).unwrap_err();
    assert_eq!(actual.to_string(), expected.to_string());
    assert_eq!(store.lock().unwrap().prepare_cached(query).unwrap().get_status(rusqlite::StatementStatus::Run), 1);
    // Public historical Tool pages still validate their caller-provided cut,
    // before exposing either a locator failure or the broken block.
    let mut foreign = cut.clone();
    foreign.read.conversation_id = ConversationId::generate();
    let connection = store.lock().unwrap();
    let error = super::super::observation_tools::page(&connection, store.conversation_id(), &foreign,
        &crate::runtime_client::content::CanonicalContentLocator::ByLedgerPosition { position: 1 }, 0, 32).unwrap_err();
    assert!(matches!(error, ConversationStoreError::ObservationCutExpired));
}

include!("observation_tool_row_reuse_tests.rs");
