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
