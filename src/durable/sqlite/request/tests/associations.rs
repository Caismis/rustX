//! Delta-only persistence, old-cut, rollback and corruption contracts.
use super::manifest_store::{selected_snapshot, selected_tool_batch};
use super::*;
use crate::durable::sqlite::request::snapshot::load_request_snapshot_bounded;

#[test]
#[allow(clippy::too_many_lines)] // One end-to-end metadata, row, allocation, retry and old-cut growth oracle.
fn tool_association_metadata_and_rows_stay_bounded_at_8_64_512_turns() {
    let store = store();
    store.initialize(&[]).unwrap();
    store.projected_history().unwrap();
    store.lock().unwrap().execute_batch("CREATE TEMP TRIGGER forbid_payload_rewrite BEFORE UPDATE ON request_snapshot_payloads BEGIN SELECT RAISE(ABORT,'immutable request payload updated'); END;").unwrap();
    let mut retained = Vec::new();
    let mut metadata_sizes = Vec::new();
    for turn in 1..=512 {
        store
            .append_canonical_batch(&selected_tool_batch(&format!("{turn:04}")))
            .unwrap();
        crate::model::occurrence::MATERIALIZATIONS.with(|w| w.set(0));
        crate::model::occurrence::HISTORICAL_VISITS.with(|w| w.set(0));
        super::super::super::associations::VISITED.with(|w| w.set(0));
        super::super::super::associations::WRITTEN.with(|w| w.set((0, 0)));
        let (snapshot, request) = selected_snapshot(&store, turn);
        store
            .commit_model_turn_start(&[], &snapshot, Utc::now())
            .unwrap();
        assert_eq!(
            crate::model::occurrence::MATERIALIZATIONS.with(std::cell::Cell::get),
            0,
            "warm start expanded old associations at {turn}"
        );
        assert_eq!(
            crate::model::occurrence::HISTORICAL_VISITS.with(std::cell::Cell::get),
            0,
            "warm start enumerated historical associations at {turn}"
        );
        assert!(
            super::super::super::associations::VISITED.with(std::cell::Cell::get) <= 2,
            "persistence walked the old prefix at {turn}"
        );
        let (rows, bytes) = super::super::super::associations::WRITTEN.with(std::cell::Cell::get);
        assert_eq!(rows, 1, "only one new association at {turn}");
        assert!(bytes < 1024, "per-turn association payload {bytes}");
        if matches!(turn, 8 | 64 | 512) {
            let connection = store.lock().unwrap();
            let (size,count):(usize,usize)=connection.query_row("SELECT octet_length(snapshot_json),(SELECT count(*) FROM tool_association_nodes) FROM request_snapshot_payloads WHERE request_id=?1",[snapshot.request_id.as_str()],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
            assert_eq!(count, turn as usize);
            assert!(size < 2500, "metadata expanded to {size} at {turn}");
            metadata_sizes.push(size);
            drop(connection);
            let history = store.projected_history().unwrap();
            let mut inserted = request.clone();
            inserted.messages.insert(
                0,
                crate::model::ModelInputMessage::RequestOnly(
                    crate::model::input::RequestOnlyModelContext::AuxiliaryQuestion {
                        text: "insertion".into(),
                    },
                ),
            );
            crate::model::occurrence::MATERIALIZATIONS.with(|w| w.set(0));
            let frozen = crate::model::occurrence::freeze_associations(
                &history,
                &snapshot.request_id,
                &inserted,
                Some(0),
            )
            .unwrap();
            assert_eq!(frozen.len(), turn as usize);
            assert_eq!(
                crate::model::occurrence::MATERIALIZATIONS.with(std::cell::Cell::get),
                0,
                "request-only binding expanded the prefix"
            );
            let mut retry = snapshot.clone();
            retry.identity.retry_number = 1;
            retry.request_id = retry.identity.request_id();
            retry.provisional_message_id = retry.identity.provisional_message_id();
            retry.tool_associations.rebind_request(&retry.request_id);
            super::super::super::associations::WRITTEN.with(|w| w.set((0, 0)));
            store
                .commit_model_turn_start(&[], &retry, Utc::now())
                .unwrap();
            assert_eq!(
                super::super::super::associations::WRITTEN.with(std::cell::Cell::get),
                (0, 0),
                "retry rewrote old associations"
            );
            assert_eq!(
                crate::model::occurrence::MATERIALIZATIONS.with(std::cell::Cell::get),
                0,
                "retry expanded the prefix"
            );
            retained.push((snapshot, request));
        }
    }
    assert!(
        metadata_sizes.iter().max().unwrap() - metadata_sizes.iter().min().unwrap() < 100,
        "metadata is a root and bounded scalar fields: {metadata_sizes:?}"
    );
    for (snapshot, request) in retained {
        let oracle = crate::model::occurrence::canonical_associations(
            store.conversation_id(),
            &snapshot.request_id,
            &request,
        )
        .unwrap();
        assert_eq!(snapshot.tool_associations.as_slice(), oracle.as_slice());
        assert_eq!(
            store.load_request_snapshot(&snapshot.request_id).unwrap(),
            snapshot
        );
        assert_eq!(
            store
                .reconstruct_model_request(&snapshot.request_id)
                .unwrap(),
            request
        );
    }
}

#[test]
fn tool_association_rollback_trigger_corruption_and_retry_publish_no_stale_seats() {
    let store = store();
    store.initialize(&[]).unwrap();
    store.projected_history().unwrap();
    store
        .append_canonical_batch(&selected_tool_batch("first"))
        .unwrap();
    let (first, _) = selected_snapshot(&store, 1);
    store
        .commit_model_turn_start(&[], &first, Utc::now())
        .unwrap();
    store
        .append_canonical_batch(&selected_tool_batch("second"))
        .unwrap();
    let (second, request) = selected_snapshot(&store, 2);
    for action in [
        "SELECT RAISE(ABORT,'after immutable association insert');",
        "UPDATE tool_association_nodes SET association_json=json_set(association_json,'$.call_block',99) WHERE id=1;",
        "DELETE FROM tool_association_nodes WHERE id=2;",
    ] {
        store.lock().unwrap().execute_batch(&format!("CREATE TEMP TRIGGER corrupt_association AFTER INSERT ON request_snapshots BEGIN {action} END;")).unwrap();
        assert!(
            store
                .commit_model_turn_start(&[], &second, Utc::now())
                .is_err(),
            "{action}"
        );
        let connection = store.lock().unwrap();
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM tool_association_nodes", [], |r| r
                    .get::<_, usize>(
                    0
                ))
                .unwrap(),
            1
        );
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM request_snapshot_payloads", [], |r| {
                    r.get::<_, usize>(0)
                })
                .unwrap(),
            1
        );
        connection
            .execute_batch("DROP TRIGGER corrupt_association")
            .unwrap();
        drop(connection);
        assert_eq!(
            store.load_request_snapshot(&first.request_id).unwrap(),
            first
        );
    }
    store
        .commit_model_turn_start(&[], &second, Utc::now())
        .unwrap();
    assert_eq!(
        store.reconstruct_model_request(&second.request_id).unwrap(),
        request
    );
    store.lock().unwrap().execute("UPDATE tool_association_nodes SET association_json=json_set(association_json,'$.call_block',99) WHERE id=1",[]).unwrap();
    assert!(store.load_request_snapshot(&first.request_id).is_err());
    assert!(store.load_request_snapshot(&second.request_id).is_err());
}

#[test]
fn tool_association_roots_survive_real_sqlite_reopen_and_new_append() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("associations.sqlite");
    let conversation = ConversationId::new("conv_7ac87de2-c758-7b2a-8ba9-000000000002");
    let store = SqliteConversationStore::open(conversation.clone(), &path).unwrap();
    store.initialize(&[]).unwrap();
    store.projected_history().unwrap();
    store
        .append_canonical_batch(&selected_tool_batch("reopen-first"))
        .unwrap();
    let (first, request) = selected_snapshot(&store, 1);
    store
        .commit_model_turn_start(&[], &first, Utc::now())
        .unwrap();
    drop(store);
    let store = SqliteConversationStore::open(conversation, &path).unwrap();
    assert_eq!(
        store.load_request_snapshot(&first.request_id).unwrap(),
        first
    );
    assert_eq!(
        store.reconstruct_model_request(&first.request_id).unwrap(),
        request
    );
    store
        .append_canonical_batch(&selected_tool_batch("reopen-second"))
        .unwrap();
    let (second, request) = selected_snapshot(&store, 2);
    store
        .commit_model_turn_start(&[], &second, Utc::now())
        .unwrap();
    assert_eq!(
        store.reconstruct_model_request(&second.request_id).unwrap(),
        request
    );
    assert_eq!(
        store
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM tool_association_nodes", [], |r| r
                .get::<_, usize>(
                0
            ))
            .unwrap(),
        2
    );
    assert_eq!(
        store.load_request_snapshot(&first.request_id).unwrap(),
        first
    );
}

#[test]
fn compaction_replaces_the_association_root_without_changing_old_request_cuts() {
    let store = store();
    store.initialize(&[]).unwrap();
    store.projected_history().unwrap();
    store
        .append_canonical_batch(&selected_tool_batch("before-compaction"))
        .unwrap();
    let (old, old_request) = selected_snapshot(&store, 1);
    store
        .commit_model_turn_start(&[], &old, Utc::now())
        .unwrap();
    let candidate = crate::scripted_suites::support::context::checkpoint_fixture(
        &store,
        "association-checkpoint",
        2,
        "summary with no tools",
    );
    store
        .commit_projection(
            crate::scripted_suites::support::context::checkpoint_commit_fixture(
                &store, candidate, None,
            ),
        )
        .unwrap();
    let (compacted, compacted_request) = selected_snapshot(&store, 2);
    assert!(compacted.tool_associations.is_empty());
    store
        .commit_model_turn_start(&[], &compacted, Utc::now())
        .unwrap();
    store
        .append_canonical_batch(&selected_tool_batch("after-compaction"))
        .unwrap();
    let (tail, tail_request) = selected_snapshot(&store, 3);
    assert_eq!(tail.tool_associations.len(), 1);
    store
        .commit_model_turn_start(&[], &tail, Utc::now())
        .unwrap();
    for (snapshot, request) in [
        (old, old_request),
        (compacted, compacted_request),
        (tail, tail_request),
    ] {
        assert_eq!(
            store.load_request_snapshot(&snapshot.request_id).unwrap(),
            snapshot
        );
        assert_eq!(
            store
                .reconstruct_model_request(&snapshot.request_id)
                .unwrap(),
            request
        );
    }
}

#[test]
fn oversized_raw_wire_ids_preserve_the_original_flat_snapshot_byte_budget() {
    let store = store();
    store.initialize(&[]).unwrap();
    store.projected_history().unwrap();
    let mut messages = selected_tool_batch("large-raw-wire");
    let raw = ToolCallId::new("invalid provider wire label / 水".repeat(4000));
    let MessageBlock::Assistant(assistant) = &mut messages[0] else {
        unreachable!()
    };
    let AssistantContentBlock::ToolCall(call) = &mut assistant.content[0] else {
        unreachable!()
    };
    call.id.clone_from(&raw);
    let MessageBlock::Tool(result) = &mut messages[1] else {
        unreachable!()
    };
    result.tool_call_id = raw;
    store.append_canonical_batch(&messages).unwrap();
    let (snapshot, request) = selected_snapshot(&store, 1);
    store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap();
    let expected = crate::model::occurrence::canonical_associations(
        store.conversation_id(),
        &snapshot.request_id,
        &request,
    )
    .unwrap();
    assert_eq!(snapshot.tool_associations.as_slice(), expected.as_slice());
    let bytes = serde_json::to_vec(&snapshot).unwrap().len();
    assert!(
        bytes < 8192,
        "normalized wire labels do not retain the large raw ID"
    );
    let connection = store.lock().unwrap();
    assert!(
        connection
            .query_row(
                "SELECT octet_length(association_json) FROM tool_association_nodes",
                [],
                |row| row.get::<_, usize>(0)
            )
            .unwrap()
            > 65536
    );
    assert_eq!(
        load_request_snapshot_bounded(
            &connection,
            store.conversation_id(),
            &snapshot.request_id,
            bytes
        )
        .unwrap(),
        snapshot
    );
    assert!(
        load_request_snapshot_bounded(
            &connection,
            store.conversation_id(),
            &snapshot.request_id,
            bytes - 1
        )
        .is_err()
    );
}
