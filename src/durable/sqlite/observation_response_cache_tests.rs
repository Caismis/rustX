// Cross-refresh proofs must match fresh reads, including malformed source error ordering.
fn response_cache_prime(
    store: &SqliteConversationStore,
    cut: &ObservationReadCut,
    original: &RuntimeClientTranscriptPage,
) -> RuntimeClientTranscriptPage {
    let mut oracle = original.clone();
    crate::runtime_client::response::native_oracle_decorate_through(
        store,
        &mut oracle,
        cut.read.journal,
    )
    .unwrap();
    store.conn.response.invalidate();
    let mut decorated = original.clone();
    crate::runtime_client::response::decorate_through(store, &mut decorated, cut.read.journal)
        .unwrap();
    assert!(
        store.conn.response.len() > 0,
        "fixture must enter the response cache"
    );
    let mut repeat = original.clone();
    crate::runtime_client::response::decorate_through(store, &mut repeat, cut.read.journal)
        .unwrap();
    assert_eq!(repeat, decorated);
    assert_eq!(decorated, oracle);
    decorated
}

fn response_cache_assert_fresh(
    store: &SqliteConversationStore,
    cut: &ObservationReadCut,
    original: &RuntimeClientTranscriptPage,
) -> Option<RuntimeClientTranscriptPage> {
    let mut actual = original.clone();
    let cached =
        crate::runtime_client::response::decorate_through(store, &mut actual, cut.read.journal);
    // Force the unchanged native read algorithm: only its cross-refresh reuse
    // is invalidated. This also compares exact failure order for malformed SQL
    // cells where the older name-based test oracle has different authority.
    store.conn.response.invalidate();
    let mut expected = original.clone();
    let native =
        crate::runtime_client::response::decorate_through(store, &mut expected, cut.read.journal);
    match (native, cached) {
        (Ok(()), Ok(())) => {
            assert_eq!(actual, expected);
            Some(actual)
        }
        (Err(expected), Err(actual)) => {
            assert_eq!(
                std::mem::discriminant(&actual),
                std::mem::discriminant(&expected)
            );
            assert_eq!(actual.to_string(), expected.to_string());
            None
        }
        (expected, actual) => panic!("cached result {actual:?} differs from native {expected:?}"),
    }
}

#[test]
fn response_cache_revalidates_same_connection_sources_and_survives_rollback() {
    for mutation in [
        "UPDATE events SET observation_execution=x'7b' WHERE observation_kind='attempt_completed'",
        "UPDATE events SET observation_members=x'7b' WHERE observation_kind='attempt_completed'",
        "UPDATE observation_headers SET header_json=CAST(json_set(header_json,'$.role','user') AS BLOB)",
        "UPDATE message_ledger SET native_transcript=NULL",
        "UPDATE message_ledger SET first_append_revision=NULL",
    ] {
        let store = store();
        store.initialize(&[]).unwrap();
        let attempt = response_start(&store, "cache-rewrite");
        response_message(&store, &attempt, "cache-rewrite-message");
        response_complete(&store, &attempt);
        let cut = store.observation_read_cut().unwrap();
        let page = response_page_at(&store, &cut);
        let original = response_cache_prime(&store, &cut, &page);
        {
            let connection = store.lock().unwrap();
            connection
                .execute_batch("SAVEPOINT response_cache_rollback")
                .unwrap();
            assert!(connection.execute(mutation, []).unwrap() > 0, "{mutation}");
            connection
                .execute_batch(
                    "ROLLBACK TO response_cache_rollback; RELEASE response_cache_rollback",
                )
                .unwrap();
        }
        assert_eq!(
            response_cache_assert_fresh(&store, &cut, &page),
            Some(original.clone())
        );
        response_cache_prime(&store, &cut, &page);
        assert!(store.lock().unwrap().execute(mutation, []).unwrap() > 0);
        let changed = response_cache_assert_fresh(&store, &cut, &page);
        assert!(
            changed.as_ref() != Some(&original),
            "mutation must affect the response proof: {mutation}"
        );
    }
}

#[test]
fn response_cache_external_write_invalidates_and_reopen_has_no_stale_owner() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("response-cache.sqlite");
    let conversation = ConversationId::generate();
    let writer = SqliteConversationStore::open(conversation.clone(), &path).unwrap();
    writer.initialize(&[]).unwrap();
    let attempt = response_start(&writer, "cache-external");
    response_message(&writer, &attempt, "cache-external-message");
    response_complete(&writer, &attempt);
    let reader = SqliteConversationStore::open_existing(conversation.clone(), &path).unwrap();
    let cut = reader.observation_read_cut().unwrap();
    let page = response_page_at(&reader, &cut);
    response_cache_prime(&reader, &cut, &page);
    let (sequence, bytes): (u64, Vec<u8>) = writer.lock().unwrap().query_row(
        "SELECT sequence,observation_execution FROM events WHERE observation_kind='attempt_completed'",
        [], |row| Ok((row.get(0)?, row.get(1)?)),
    ).unwrap();
    writer
        .lock()
        .unwrap()
        .execute(
            "UPDATE events SET observation_execution=x'7b' WHERE sequence=?1",
            [sequence],
        )
        .unwrap();
    assert!(response_cache_assert_fresh(&reader, &cut, &page).is_none());
    writer
        .lock()
        .unwrap()
        .execute(
            "UPDATE events SET observation_execution=?2 WHERE sequence=?1",
            params![sequence, bytes],
        )
        .unwrap();
    let restored = response_cache_assert_fresh(&reader, &cut, &page).unwrap();
    drop(reader);
    let reader = SqliteConversationStore::open_existing(conversation, &path).unwrap();
    assert_eq!(reader.conn.response.len(), 0);
    assert_eq!(
        response_cache_assert_fresh(&reader, &cut, &page),
        Some(restored)
    );
}

#[test]
fn response_cache_keeps_old_cut_and_checks_new_unrelated_execution_and_member_holes() {
    for column in ["observation_execution", "observation_members"] {
        let store = store();
        store.initialize(&[]).unwrap();
        let attempt = response_start(&store, "cache-hole-original");
        response_message(&store, &attempt, "cache-hole-message");
        response_complete(&store, &attempt);
        let before = store.observation_read_cut().unwrap();
        let original_page = response_page_at(&store, &before);
        let original = response_cache_prime(&store, &before, &original_page);
        let later = response_start(&store, "cache-hole-later");
        response_complete(&store, &later);
        let after = store.observation_read_cut().unwrap();
        let page = response_page_at(&store, &after);
        response_cache_assert_fresh(&store, &after, &page).unwrap();
        store
            .lock()
            .unwrap()
            .execute(
                &format!("UPDATE events SET {column}=NULL WHERE sequence>?1"),
                [before.read.journal],
            )
            .unwrap();
        assert!(response_cache_assert_fresh(&store, &after, &page).is_none());
        assert_eq!(
            response_cache_assert_fresh(&store, &before, &original_page),
            Some(original)
        );
    }
}

#[test]
fn response_cache_new_member_alias_cannot_reuse_an_old_completed_owner() {
    let store = store();
    store.initialize(&[]).unwrap();
    let old = response_start(&store, "cache-old-owner");
    response_message(&store, &old, "cache-old-message");
    response_complete(&store, &old);
    let before = store.observation_read_cut().unwrap();
    let old_page = response_page_at(&store, &before);
    response_cache_prime(&store, &before, &old_page);
    let later = response_start(&store, "cache-new-owner");
    response_complete(&store, &later);
    let after = store.observation_read_cut().unwrap();
    let page = response_page_at(&store, &after);
    let healthy = response_cache_assert_fresh(&store, &after, &page).unwrap();
    store.lock().unwrap().execute(
        "UPDATE events SET observation_member_anchor=(SELECT observation_member_anchor FROM events WHERE observation_assistant_ledger IS NOT NULL LIMIT 1) WHERE sequence=?1",
        [after.read.journal],
    ).unwrap();
    let changed = response_cache_assert_fresh(&store, &after, &page);
    assert_ne!(changed.as_ref(), Some(&healthy));
    response_cache_assert_fresh(&store, &before, &old_page).unwrap();
}

#[test]
fn response_cache_new_tool_result_alias_rechecks_source_without_changing_process_owner() {
    let (store, attempt, proposal, call) = original_tool_before_result();
    commit_original_tool_result(
        &store,
        &attempt,
        &proposal,
        &call,
        &original_success_tool_result(),
    );
    response_message(&store, &attempt, "cache-tool-final");
    response_complete(&store, &attempt);
    let other = response_start(&store, "cache-alias-target");
    response_message(&store, &other, "cache-alias-message");
    response_complete(&store, &other);
    let cut = store.observation_read_cut().unwrap();
    let mut page = response_page_at(&store, &cut);
    page.entries.retain(|entry| {
        matches!(&entry.item,
        crate::runtime_client::snapshot::RuntimeClientTranscriptItem::Message {
            message: MessageBlock::Assistant(message)
        } if message.id.as_str() == "cache-alias-message")
    });
    page.deferred_entries.clear();
    assert_eq!(page.entries.len(), 1);
    let original = response_cache_prime(&store, &cut, &page);
    let position = store
        .message_ledger_position(&MessageId::new("cache-alias-message"))
        .unwrap()
        .unwrap();
    store
        .lock()
        .unwrap()
        .execute(
            "UPDATE canonical_tool_calls SET result_ledger_position=?1",
            [position],
        )
        .unwrap();
    let changed = response_cache_assert_fresh(&store, &cut, &page).unwrap();
    assert_ne!(changed, original);
    assert_eq!(
        changed.entries[0]
            .turn_process
            .as_ref()
            .unwrap()
            .attempt_id
            .as_ref(),
        Some(&other)
    );
}

#[test]
fn response_cache_inherited_rewrites_and_schema_changes_keep_native_failure_order() {
    let store = store();
    store
        .initialize_lineage(&member_completed_lineage("cache-inherited-message"))
        .unwrap();
    let cut = store.observation_read_cut().unwrap();
    let page = response_page_at(&store, &cut);
    // Inherited proofs can remain uncached; they must still coexist with the
    // cache and preserve their independent membership/header failure ordering.
    response_cache_assert_fresh(&store, &cut, &page).unwrap();
    store
        .lock()
        .unwrap()
        .execute_batch(
            "CREATE TEMP TABLE response_cache_ddl(value INTEGER); DROP TABLE response_cache_ddl",
        )
        .unwrap();
    response_cache_assert_fresh(&store, &cut, &page).unwrap();
    store.lock().unwrap().execute_batch("PRAGMA ignore_check_constraints=ON; UPDATE inherited_message_membership SET observation_membership_witness=NULL; PRAGMA ignore_check_constraints=OFF").unwrap();
    assert!(response_cache_assert_fresh(&store, &cut, &page).is_none());
}
