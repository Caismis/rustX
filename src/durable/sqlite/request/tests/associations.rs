//! Delta-only persistence, old-cut, rollback and corruption contracts.
use super::manifest_store::{selected_snapshot, selected_tool_batch};
use super::*;
use crate::durable::sqlite::associations::{self, ORIGIN_WRITTEN};
use crate::durable::sqlite::request::snapshot::load_request_snapshot_bounded;
use std::sync::Arc;

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
        ORIGIN_WRITTEN.with(|w| w.set((0, 0)));
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
        let (origins, origin_bytes) = ORIGIN_WRITTEN.with(std::cell::Cell::get);
        assert_eq!(origins, 1, "only one new assistant origin set at {turn}");
        assert!(origin_bytes < 512);
        assert!(bytes < 1024, "per-turn association payload {bytes}");
        if matches!(turn, 8 | 64 | 512) {
            let connection = store.lock().unwrap();
            let (size,count,origins):(usize,usize,usize)=connection.query_row("SELECT octet_length(snapshot_json),(SELECT count(*) FROM tool_association_nodes),(SELECT count(*) FROM tool_association_origin_sets) FROM request_snapshot_payloads WHERE request_id=?1",[snapshot.request_id.as_str()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
            assert_eq!(origins, turn as usize);
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
            ORIGIN_WRITTEN.with(|w| w.set((0, 0)));
            store
                .commit_model_turn_start(&[], &retry, Utc::now())
                .unwrap();
            assert_eq!(
                super::super::super::associations::WRITTEN.with(std::cell::Cell::get),
                (0, 0),
                "retry rewrote old associations"
            );
            assert_eq!(ORIGIN_WRITTEN.with(std::cell::Cell::get), (0, 0));
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
        "UPDATE tool_association_origin_sets SET origins_json='{}' WHERE id=1;",
        "DELETE FROM tool_association_origin_sets WHERE id=2;",
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
                .query_row(
                    "SELECT count(*) FROM tool_association_origin_sets",
                    [],
                    |r| r.get::<_, usize>(0)
                )
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

#[test]
fn tool_association_storage_budget_includes_request_bound_flat_array() {
    let store = store();
    store.initialize(&[]).unwrap();
    let messages: Vec<_> = (0..128)
        .flat_map(|turn| selected_tool_batch(&format!("budget-{turn}")))
        .collect();
    store.append_canonical_batch(&messages).unwrap();
    let (mut snapshot, _) = selected_snapshot(&store, 1);
    snapshot.identity.attempt_id = AttemptId::new("a".repeat(256 * 1024));
    snapshot.request_id = snapshot.identity.request_id();
    snapshot.provisional_message_id = snapshot.identity.provisional_message_id();
    snapshot
        .tool_associations
        .rebind_request(&snapshot.request_id);
    let Err(error) = store.commit_model_turn_start(&[], &snapshot, Utc::now()) else {
        panic!("the original 64 MiB metadata limit includes both request IDs per association");
    };
    assert!(error.to_string().contains("byte budget"), "{error}");
    let connection = store.lock().unwrap();
    for table in [
        "request_snapshots",
        "request_snapshot_payloads",
        "tool_association_nodes",
        "tool_association_origin_sets",
    ] {
        assert_eq!(
            connection
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| row
                    .get::<_, usize>(0))
                .unwrap(),
            0,
            "admission must roll back {table}"
        );
    }
}

#[test]
fn private_association_byte_limit_rolls_back_the_entire_start() {
    let store = store();
    store.initialize(&[]).unwrap();
    store
        .append_canonical_batch(&selected_tool_batch("private-bound"))
        .unwrap();
    let (mut snapshot, _) = selected_snapshot(&store, 1);
    let binding = snapshot.tool_associations.binding.as_ref().unwrap();
    let mut value = binding.history.iter().next().unwrap().clone();
    // Exercise the private admission guard independently of the public array
    // limit. These raw IDs normalize to bounded wire labels when flattened.
    value.wire_id = Some(crate::model::occurrence::ProviderWireCorrelationId(
        "/".repeat(1024 * 1024),
    ));
    let history = std::iter::repeat_n(value, 65).collect();
    snapshot.tool_associations = crate::model::occurrence::FrozenToolAssociations::from_history(
        history,
        snapshot.request_id.clone(),
        binding.coordinates.clone(),
    );
    assert!(serde_json::to_vec(&snapshot).unwrap().len() < 1024 * 1024);
    let Err(error) = store.commit_model_turn_start(&[], &snapshot, Utc::now()) else {
        panic!("private bytes must be rejected before COMMIT");
    };
    assert!(error.to_string().contains("private byte budget"), "{error}");
    let connection = store.lock().unwrap();
    for table in [
        "tool_association_nodes",
        "tool_association_origin_sets",
        "request_snapshots",
        "request_snapshot_payloads",
        "native_input_nodes",
    ] {
        assert_eq!(
            connection
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r
                    .get::<_, usize>(0))
                .unwrap(),
            0,
            "{table} must roll back"
        );
    }
}

#[test]
fn rejected_bounded_association_read_never_materializes_the_flat_array() {
    let store = store();
    store.initialize(&[]).unwrap();
    let messages: Vec<_> = (0..32)
        .flat_map(|turn| selected_tool_batch(&format!("read-budget-{turn}")))
        .collect();
    store.append_canonical_batch(&messages).unwrap();
    let (snapshot, _) = selected_snapshot(&store, 1);
    store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap();
    crate::model::occurrence::MATERIALIZATIONS.with(|work| work.set(0));
    assert!(
        load_request_snapshot_bounded(
            &store.lock().unwrap(),
            store.conversation_id(),
            &snapshot.request_id,
            4096
        )
        .is_err()
    );
    assert_eq!(
        crate::model::occurrence::MATERIALIZATIONS.with(std::cell::Cell::get),
        0,
        "the reader must enforce its byte budget before cloning every request-bound association"
    );
}

#[test]
#[allow(clippy::too_many_lines)] // Large valid fixture exercises every full historical association consumer.
fn large_assistant_origin_set_is_shared_across_reload_retry_continuation_and_archive() {
    use crate::message::types::ReasoningBlock;
    use crate::model::replay::ReplayOrigin;
    use crate::runtime::continuation::{OpenAiResponsesContinuation, ProviderContinuationState};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("large-origins.sqlite");
    let conversation = ConversationId::new("conv_7ac87de2-c758-7b2a-8ba9-000000000003");
    let store = SqliteConversationStore::open(conversation.clone(), &path).unwrap();
    store.initialize(&[]).unwrap();
    let mut content: Vec<_> = (0..1024)
        .map(|origin| {
            AssistantContentBlock::Reasoning(ReasoningBlock {
                origin: Some(ReplayOrigin {
                    binding: format!("origin-{origin}"),
                    protocol: ModelProtocol::OpenAiResponses,
                    model: "model".into(),
                }),
                text: Some("readable".into()),
                provider_state: Some(ProviderContinuationState::OpenAiResponses(
                    OpenAiResponsesContinuation::Stateless {
                        items: Vec::new().into(),
                    },
                )),
            })
        })
        .collect();
    let mut results = Vec::new();
    for call_index in 0..600 {
        let mut pair = selected_tool_batch(&format!("private-origins-{call_index}"));
        let MessageBlock::Assistant(assistant) = &pair[0] else {
            unreachable!()
        };
        let mut block = assistant.content[0].clone();
        let AssistantContentBlock::ToolCall(call) = &mut block else {
            unreachable!()
        };
        call.id = ToolCallId::new(format!("private-wire-{call_index}"));
        let wire_id = call.id.clone();
        content.push(block);
        let MessageBlock::Tool(result) = &mut pair[1] else {
            unreachable!()
        };
        result.tool_call_id = wire_id;
        result.occurrence.assistant_message_id = MessageId::new("private-origins-call");
        result.occurrence.block_index = ContentBlockIndex::new(1024 + call_index);
        results.push(pair[1].clone());
    }
    let mut messages = vec![MessageBlock::Assistant(AssistantMessageBlock {
        id: MessageId::new("private-origins-call"),
        content,
    })];
    messages.extend(results);
    store.append_canonical_batch(&messages).unwrap();
    let (snapshot, request) = selected_snapshot(&store, 1);
    crate::model::adapter::validation::validate_request(
        &request,
        ModelProtocol::OpenAiChatCompletions,
    )
    .unwrap();
    store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap();
    let public_bytes = serde_json::to_vec(&snapshot).unwrap().len();
    let (fact_bytes, origin_bytes, origin_rows): (usize, usize, usize) = store
        .lock().unwrap().query_row(
            "SELECT (SELECT sum(octet_length(association_json)) FROM tool_association_nodes),sum(octet_length(origins_json)),count(*) FROM tool_association_origin_sets",
            [], |row|Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
        ).unwrap();
    assert!(public_bytes < 1024 * 1024);
    assert_eq!(origin_rows, 1);
    assert!(origin_bytes < 128 * 1024);
    assert!(
        fact_bytes < 600 * 768,
        "association payloads must not contain the origins"
    );
    assert!(fact_bytes + origin_bytes < 2 * public_bytes);
    drop(store);
    let store = SqliteConversationStore::open(conversation, &path).unwrap();
    let reloaded = store.load_request_snapshot(&snapshot.request_id).unwrap();
    assert_eq!(reloaded, snapshot);
    let history = &reloaded.tool_associations.binding.as_ref().unwrap().history;
    let origins = &history.iter().next().unwrap().native_origins;
    assert_eq!(origins.len(), 1024);
    assert_eq!(
        history.native_summary_entries(),
        600,
        "wire-ID summaries also share their set"
    );
    assert!(
        history
            .iter()
            .all(|value| Arc::ptr_eq(origins, &value.native_origins))
    );
    assert_eq!(
        store
            .reconstruct_model_request(&snapshot.request_id)
            .unwrap(),
        request
    );
    let before = store.presentation_frontier().unwrap();
    let retried = store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap();
    assert_eq!(
        retried.disposition,
        crate::durable::ModelTurnStartCommitDisposition::IdempotentReplay
    );
    assert_eq!(store.presentation_frontier().unwrap(), before);

    // The exact private allowance covers the bytes fetched, including each
    // origin set once. A short allowance rejects before fetching any payload.
    let binding: associations::Binding = {
        let connection = store.lock().unwrap();
        let json: String = connection
            .query_row(
                "SELECT snapshot_json FROM request_snapshot_payloads WHERE request_id=?1",
                [snapshot.request_id.as_str()],
                |r| r.get(0),
            )
            .unwrap();
        serde_json::from_value(
            serde_json::from_str::<serde_json::Value>(&json).unwrap()["association_binding"]
                .clone(),
        )
        .unwrap()
    };
    let private_bytes = usize::try_from(binding.root.unwrap().private_bytes).unwrap();
    eprintln!(
        "origin fixture: public_bytes={public_bytes}, association_bytes={fact_bytes}, origin_bytes={origin_bytes}, private_read_bytes={private_bytes}"
    );
    associations::READ_BYTES.with(|w| w.set(0));
    assert!(associations::load(&store.lock().unwrap(), &binding, private_bytes - 1).is_err());
    assert_eq!(associations::READ_BYTES.with(std::cell::Cell::get), 0);
    let loaded = associations::load(&store.lock().unwrap(), &binding, private_bytes).unwrap();
    assert_eq!(loaded, *history);
    assert_eq!(
        associations::READ_BYTES.with(std::cell::Cell::get),
        private_bytes as u64
    );

    {
        let connection = store.lock().unwrap();
        connection.execute_batch("SAVEPOINT oversized_origins; PRAGMA ignore_check_constraints=ON; UPDATE tool_association_origin_sets SET origins_json=replace(hex(zeroblob(1000000)),'0','x')").unwrap();
        associations::READ_BYTES.with(|w| w.set(0));
        assert!(associations::load(&connection, &binding, private_bytes).is_err());
        assert!(
            associations::READ_BYTES.with(std::cell::Cell::get) < 1024,
            "the oversized origin cell must never enter Rust"
        );
        connection.execute_batch("ROLLBACK TO oversized_origins; RELEASE oversized_origins; PRAGMA ignore_check_constraints=OFF").unwrap();
    }

    let output = MessageBlock::Assistant(AssistantMessageBlock {
        id: snapshot.provisional_message_id.clone(),
        content: vec![AssistantContentBlock::Reasoning(ReasoningBlock {
            origin: snapshot.invocation.replay_origin(),
            text: Some("accepted".into()),
            provider_state: Some(ProviderContinuationState::OpenAiResponses(
                OpenAiResponsesContinuation::Stateless {
                    items: Vec::new().into(),
                },
            )),
        })],
    });
    let source =
        crate::runtime::continuation::AcceptedOutputContinuationSource::from_accepted_output(
            snapshot.request_id.clone(),
            store.conversation_id(),
            &request.messages,
            &output,
        )
        .unwrap();
    store.append_canonical(&output).unwrap();
    let (mut next, mut next_request) = selected_snapshot(&store, 2);
    next_request.continuation = Some(source.native_state.clone());
    next_request.continuation_source = Some(source);
    next.continuation = next_request.continuation.clone();
    next.tool_associations = crate::model::occurrence::freeze_associations(
        &store.projected_history().unwrap(),
        &next.request_id,
        &next_request,
        None,
    )
    .unwrap();
    next.freeze_selected_input(
        store.conversation_id(),
        &next_request,
        &store.projected_history().unwrap().input_manifest,
    )
    .unwrap();
    ORIGIN_WRITTEN.with(|w| w.set((0, 0)));
    store
        .commit_model_turn_start(&[], &next, Utc::now())
        .unwrap();
    assert_eq!(ORIGIN_WRITTEN.with(std::cell::Cell::get), (0, 0));
    assert_eq!(store.load_request_snapshot(&next.request_id).unwrap(), next);
    assert_eq!(
        store.reconstruct_model_request(&next.request_id).unwrap(),
        next_request
    );

    store.archive_barrier().unwrap();
    let through = store.archive_frontiers().unwrap().0.requests;
    let (ordinal, json) = store
        .archive_record(archive::Authority::Requests, 0, through)
        .unwrap()
        .unwrap();
    assert_eq!(json, serde_json::to_string(&snapshot).unwrap());
    let (_, json) = store
        .archive_record(archive::Authority::Requests, ordinal, through)
        .unwrap()
        .unwrap();
    assert_eq!(json, serde_json::to_string(&next).unwrap());
    store.archive_release().unwrap();
}
