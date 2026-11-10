#[test]
fn native_canonical_receipts_do_not_stage_retired_headers() {
    let (mut actual, store) = initialized_source_projection();
    let mut expected = projection();
    expected
        .initialize_descriptor_heads(&store, store.observation_read_cut().unwrap())
        .unwrap();
    expected.initialize_publication_view();
    let started = append_source_attempt_start(&store, "canonical-owner-attempt");
    let mut native = started.observation.as_deref().unwrap().clone();
    let historical = actual.checkpoint_seed();
    let old = historical.checkpoint_candidate().unwrap();
    for count in 1..=512 {
        let message = compactable_user(&format!("canonical-owner-{count}"));
        let receipt = store.append_canonical(&message).unwrap();
        native.canonical = Some(receipt.observation.clone());
        let mut observations = Vec::new();
        actual.prepare_native_canonical(&native, &mut observations);
        assert!(actual.prepared_messages.is_empty(), "history={count}");
        publish_source_fact(&mut actual, |projection| {
            projection.apply(ConversationObservation::ObservationBatch(observations));
        });
        // The production fold consumes the receipt itself. Stage no headers
        // in this independent direct-receipt oracle either.
        expected.install_content_metadata(
            receipt
                .observation
                .head
                .iter()
                .flat_map(|header| {
                    header
                        .content
                        .iter()
                        .map(|binding| binding.content_ref.clone())
                })
                .collect(),
        );
        publish_source_fact(&mut expected, |projection| {
            projection.apply(ConversationObservation::CanonicalCommitted {
                attempt_id: Some(attempt()),
                receipt,
            });
        });
        assert!(
            actual.prepared_messages.is_empty(),
            "folded history={count}"
        );
        assert_eq!(actual.snapshot, expected.snapshot);
        assert_eq!(actual.checkpoint_read_cut(), expected.checkpoint_read_cut());
        assert_eq!(actual.replay.back(), expected.replay.back());
        assert!(
            actual.snapshot.message_descriptors.len()
                <= crate::runtime_client::checkpoint::CHECKPOINT_NATIVE_HEAD_ITEMS
        );
        assert!(
            actual.head_bindings.len()
                <= crate::runtime_client::checkpoint::CHECKPOINT_NATIVE_HEAD_ITEMS
        );
        if [32, 128, 512].contains(&count) {
            assert_eq!(
                historical.checkpoint_candidate().unwrap(),
                old,
                "held old cut is unchanged"
            );
        }
    }
    assert_eq!(actual.replay.len(), expected.replay.len());
    assert_eq!(actual.replay_bytes, expected.replay_bytes);
}
