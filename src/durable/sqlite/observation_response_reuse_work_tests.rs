#[test]
fn completed_response_reuse_skips_native_owner_queries_and_decodes_across_new_journal() {
    let store = store();
    store.initialize(&[]).unwrap();
    for index in 0..8 {
        let name = format!("reuse-owner-{index}");
        let attempt = response_start(&store, &name);
        response_message(&store, &attempt, &name);
        response_complete(&store, &attempt);
    }
    let before = store.observation_read_cut().unwrap();
    let before_page = response_page_at(&store, &before);
    let original = response_cache_prime(&store, &before, &before_page);
    assert_eq!(store.conn.response.len(), 8);
    let _running = response_start(&store, "reuse-active");
    let cut = store.observation_read_cut().unwrap();
    let mut page = response_page_at(&store, &cut);
    super::super::observation_response::read_work();
    super::super::observation_response::cache_work();
    crate::runtime_client::response::decorate_through(&store, &mut page, cut.read.journal).unwrap();
    assert_eq!(super::super::observation_response::read_work(), (0, 0));
    assert_eq!(
        super::super::observation_response::cache_work(),
        (8, 0, 1),
        "all eight real native owner lookups and JSON decodes are skipped; current J execution holes are still queried"
    );
    assert_eq!(page.entries, original.entries);
    assert_response_page_oracle(&store, &cut);
    // The old cut is independently reconstructible and retains the same answers.
    assert_eq!(assert_response_page_oracle(&store, &before), original);
    // Mutable pending epochs do not own these already canonical response seats.
    // The public current-cut validator still runs; only unrelated pending state
    // is excluded from the completed-owner certificate.
    let accepted = store
        .accept_inbound(draft("new user after completed history"))
        .unwrap();
    store.adopt_pending_batch(accepted.sequence, None).unwrap();
    let after_adoption = store.observation_read_cut().unwrap();
    assert_ne!(
        after_adoption.read.mutation_revision,
        before.read.mutation_revision
    );
    let selections = crate::runtime_client::response::response_page_selections(&before_page);
    super::super::observation_response::read_work();
    super::super::observation_response::cache_work();
    store
        .observation_response_page(&after_adoption, &selections)
        .unwrap();
    assert_eq!(super::super::observation_response::read_work(), (0, 0));
    assert_eq!(super::super::observation_response::cache_work(), (8, 0, 1));
    assert!(
        store
            .observation_response_page(&before, &selections)
            .is_err()
    );
    assert_response_page_oracle(&store, &after_adoption);
}

#[test]
fn response_reuse_authenticates_user_seats_and_keeps_a_fixed_capacity() {
    let store = store();
    let messages: Vec<_> = (0..40)
        .map(|index| user_message(&format!("cache-user-{index}"), "native user body"))
        .collect();
    store.initialize(&messages).unwrap();
    let cut = store.observation_read_cut().unwrap();
    let selections: Vec<_> = messages
        .iter()
        .enumerate()
        .map(|(index, _)| ResponsePageSelection {
            cursor: index as u64 + 1,
            kind: ResponsePageSelectionKind::Message(MessageId::new(format!("cache-user-{index}"))),
        })
        .collect();
    let first = store
        .observation_response_page(&cut, &selections[..32])
        .unwrap();
    assert!(
        first
            .items
            .iter()
            .all(|item| item.process.is_none() && item.response.is_none())
    );
    assert_eq!(store.conn.response.len(), 32);
    super::super::observation_response::cache_work();
    store
        .observation_response_page(&cut, &selections[..32])
        .unwrap();
    assert_eq!(super::super::observation_response::cache_work(), (32, 0, 0));
    store
        .observation_response_page(&cut, &selections[32..])
        .unwrap();
    assert_eq!(store.conn.response.len(), 32);
    // A retained display header cannot stand in for missing native authority.
    store
        .lock()
        .unwrap()
        .execute(
            "UPDATE message_ledger SET native_transcript=NULL WHERE position=40",
            [],
        )
        .unwrap();
    super::super::observation_response::cache_work();
    let actual = store.observation_response_page(&cut, &selections[39..]);
    assert_eq!(super::super::observation_response::cache_work().0, 0);
    store.conn.response.invalidate();
    let fresh = store.observation_response_page(&cut, &selections[39..]);
    assert_eq!(format!("{actual:?}"), format!("{fresh:?}"));
}

#[test]
fn response_reuse_never_admits_provisional_write_transaction_results() {
    let store = store();
    store.initialize(&[]).unwrap();
    let attempt = response_start(&store, "cache-write");
    response_message(&store, &attempt, "cache-write-message");
    response_complete(&store, &attempt);
    let cut = store.observation_read_cut().unwrap();
    let page = response_page_at(&store, &cut);
    let selections = crate::runtime_client::response::response_page_selections(&page);
    store.conn.response.invalidate();
    {
        let mut connection = store.lock().unwrap();
        let transaction = connection.transaction().unwrap();
        transaction
            .execute("UPDATE message_ledger SET first_append_revision=0", [])
            .unwrap();
        let provisional = super::super::observation_response::read(
            &transaction,
            &store.conn.response,
            store.conversation_id(),
            &cut,
            &selections,
        )
        .unwrap();
        assert!(provisional.items.iter().any(|item| item.response.is_some()));
        assert_eq!(
            store.conn.response.len(),
            0,
            "a successful read in a write transaction cannot certify committed storage"
        );
        transaction.rollback().unwrap();
    }
    let restored = assert_response_page_oracle(&store, &cut);
    assert!(restored.entries[0].completed_response.is_some());
    assert!(store.conn.response.len() > 0);
}

#[test]
fn response_reuse_keeps_terminal_only_delegated_proofs_uncached() {
    let store = store();
    store.initialize(&[]).unwrap();
    let attempt = response_start(&store, "cache-terminal-only");
    store
        .append_event(envelope(
            store.conversation_id(),
            "cache-terminal-only-end",
            Some(attempt.clone()),
            RuntimeEvent::AttemptCancelled {
                attempt_id: attempt,
                reason: CancellationReason::UserRequested,
            },
        ))
        .unwrap();
    let cut = store.observation_read_cut().unwrap();
    let original = assert_response_page_oracle(&store, &cut);
    assert!(
        original
            .entries
            .iter()
            .any(|entry| entry.turn_process.is_some())
    );
    assert_eq!(store.conn.response.len(), 0);
    super::super::observation_response::cache_work();
    assert_eq!(assert_response_page_oracle(&store, &cut), original);
    assert_eq!(super::super::observation_response::cache_work().0, 0);
    assert_eq!(store.conn.response.len(), 0);
}
