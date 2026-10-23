//! Request snapshot and model-turn start regressions: manifest store.

use crate::conversation::SurfaceRevision;
use crate::durable::sqlite::request::snapshot::*;
use crate::durable::sqlite::test_support::*;
use crate::durable::sqlite::*;
use crate::durable::{
    CanonicalStore, JournalStore, ObservationStore, ProjectionStore, RequestStore,
};
use crate::durable::{ConversationStore, ConversationStoreError};
use crate::message::types::{AssistantContentBlock, ContentBlockIndex, MessageBlock};
use crate::message::types::{AssistantMessageBlock, ToolMessageBlock};
use crate::model::snapshot::RequestSnapshot;
use crate::model::types::ModelRequest;
use crate::runtime::identity::{MessageId, ToolCallId, ToolId};
use chrono::Utc;

pub(super) fn selected_snapshot(
    store: &SqliteConversationStore,
    turn: u32,
) -> (RequestSnapshot, ModelRequest) {
    let history = store.projected_history().unwrap();
    let mut snapshot = progress_request_snapshot(
        "selected-manifest-owner",
        &turn.to_string(),
        0,
        SurfaceRevision::new(history.cut.ledger.0),
        Vec::new(),
    );
    snapshot.history_cut = history.cut.clone();
    let request = ModelRequest {
        response_byte_limit: None,
        continuation_source: None,
        images: crate::model::images::ResolvedImages::new(),
        invocation: snapshot.invocation.clone(),
        messages: history.messages.clone(),
        tools: snapshot.tool_definitions.clone(),
        effective_system_prompt: snapshot.effective_system_prompt.clone(),
        continuation: snapshot.continuation.clone(),
    };
    snapshot.tool_associations = crate::model::occurrence::freeze_associations(
        &history,
        &snapshot.request_id,
        &request,
        None,
    )
    .unwrap();
    snapshot
        .freeze_selected_input(store.conversation_id(), &request, &history.input_manifest)
        .unwrap();
    (snapshot, request)
}
pub(super) fn assert_selected_store_output(
    store: &SqliteConversationStore,
    snapshot: &RequestSnapshot,
    request: &ModelRequest,
    canonical: &[MessageBlock],
    started_sequence: u64,
) {
    assert_eq!(
        store.load_request_snapshot(&snapshot.request_id).unwrap(),
        *snapshot
    );
    let restored = store
        .reconstruct_model_request(&snapshot.request_id)
        .unwrap();
    assert_eq!(restored, *request);
    assert_eq!(
        restored.messages.to_vec(),
        crate::model::input::canonical_input(canonical)
    );
    assert_eq!(
        restored.semantic_input_fingerprint(),
        snapshot.semantic_input_fingerprint
    );
    let expected = {
        let connection = store.lock().unwrap();
        observation_request::legacy_request_source_bytes(
            &connection,
            started_sequence,
            store.conversation_id(),
            &request.messages,
            snapshot,
        )
        .unwrap()
    };
    let mut bytes = Vec::new();
    let mut offset = 0;
    let mut extent = None;
    loop {
        let chunk = store
            .request_value_read(&snapshot.request_id, started_sequence, &[], offset, 65536)
            .unwrap();
        assert_eq!(
            *extent.get_or_insert(chunk.readable_end),
            chunk.readable_end
        );
        assert_eq!(chunk.end_offset, offset + chunk.bytes.len() as u64);
        bytes.extend_from_slice(&chunk.bytes);
        let Some(next) = chunk.next_offset else {
            break;
        };
        assert!(next > offset);
        offset = next;
    }
    assert_eq!(bytes, expected);
}
/// Capacity suite: this test's value comes from its scale, so FAST skips
/// `::scale::` and FULL runs it (tests/README.md, "Capacity tests").
mod scale {
    use super::*;

    #[test]
    fn native_manifest_warm_store_appends_are_delta_and_depth_bounded_at_100_300_1000() {
        let store = store();
        store.initialize(&[]).unwrap();
        store.projected_history().unwrap();
        let mut canonical = Vec::new();
        for index in 1..=1000u32 {
            let message = user_message(
                &format!("selected-native-{index}"),
                &format!("original 水🙂 {index}"),
            );
            store.append_canonical(&message).unwrap();
            canonical.push(message);
            compaction_resources::take_body_decodes();
            crate::model::selected_input::take_hash_work();
            let (snapshot, request) = selected_snapshot(&store, index);
            assert_eq!(
                compaction_resources::take_body_decodes(),
                1,
                "only the newly appended native C is read"
            );
            let hashes = crate::model::selected_input::take_hash_work();
            assert!(
                hashes.0 > 0 && hashes.0 < 1024 && hashes.1 > 0 && hashes.1 < 1024,
                "delta hash work: {hashes:?}"
            );
            take_request_input_reconstructions();
            let before: u64 = store
                .lock()
                .unwrap()
                .query_row("SELECT count(*) FROM native_input_nodes", [], |r| r.get(0))
                .unwrap();
            observation_request::composed::take_work();
            let committed = store
                .commit_model_turn_start(&[], &snapshot, Utc::now())
                .unwrap();
            let source = observation_request::composed::take_work();
            assert_eq!(
                source.new_leaves, 1,
                "Source captures only the new native occurrence: {source:?}"
            );
            assert!(source.max_source_metadata <= 65536 && source.max_index_page <= 65536);
            assert_eq!(
                take_request_input_reconstructions(),
                (0, 0),
                "warm request must not reconstruct its unchanged prefix"
            );
            let connection = store.lock().unwrap();
            let after: u64 = connection
                .query_row("SELECT count(*) FROM native_input_nodes", [], |r| r.get(0))
                .unwrap();
            let root = read_stored_snapshot_root(&connection, &snapshot.request_id)
                .unwrap()
                .unwrap();
            assert!(
                after - before <= 2 * u64::from(root.height) + 2,
                "delta SQL nodes {} at {index}",
                after - before
            );
            drop(connection);
            if matches!(index, 100 | 300 | 1000) {
                assert_selected_store_output(
                    &store,
                    &snapshot,
                    &request,
                    &canonical,
                    committed.started.sequence,
                );
            }
        }
    }
}
#[test]
fn native_manifest_warm_after_insert_mutation_rejects_and_retry_has_no_rolled_back_seats() {
    let store = store();
    let original = user_message("selected-trigger-base", "original immutable body");
    store.initialize(std::slice::from_ref(&original)).unwrap();
    let (first, _) = selected_snapshot(&store, 1);
    store
        .commit_model_turn_start(&[], &first, Utc::now())
        .unwrap();
    let next = user_message("selected-trigger-delta", "new exact delta");
    store.append_canonical(&next).unwrap();
    let (snapshot, expected) = selected_snapshot(&store, 2);
    let before = store.read_events(None, 100).unwrap().events;
    let rows: u64 = store
        .lock()
        .unwrap()
        .query_row("SELECT count(*) FROM native_input_nodes", [], |r| r.get(0))
        .unwrap();
    store.lock().unwrap().execute_batch("CREATE TEMP TRIGGER mutate_selected_original AFTER INSERT ON request_snapshots BEGIN UPDATE message_ledger SET message_json=json_set(message_json,'$.content[0].text','corrupt trigger') WHERE position=1; END;").unwrap();
    assert!(matches!(
        store.commit_model_turn_start(&[], &snapshot, Utc::now()),
        Err(ConversationStoreError::InvalidReference(_))
    ));
    assert_eq!(store.load_canonical().unwrap(), vec![original, next]);
    assert_eq!(store.read_events(None, 100).unwrap().events, before);
    assert_eq!(
        store
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM native_input_nodes", [], |r| r
                .get::<_, u64>(0))
            .unwrap(),
        rows
    );
    store
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER mutate_selected_original")
        .unwrap();
    store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap();
    assert_eq!(
        store
            .reconstruct_model_request(&snapshot.request_id)
            .unwrap(),
        expected
    );
    assert_eq!(
        store.load_request_snapshot(&snapshot.request_id).unwrap(),
        snapshot
    );
}

#[test]
fn native_manifest_warm_request_after_compaction_does_not_decode_unchanged_checkpoint() {
    let store = store();
    let messages: Vec<_> = (0..100)
        .map(|n| {
            user_message(
                &format!("selected-checkpoint-{n}"),
                &format!("original checkpoint body {n}"),
            )
        })
        .collect();
    store.initialize(&messages).unwrap();
    let candidate = crate::scripted_suites::support::context::checkpoint_fixture(
        &store,
        "selected-native-checkpoint",
        1,
        "original summary",
    );
    store
        .commit_projection(
            crate::scripted_suites::support::context::checkpoint_commit_fixture(
                &store, candidate, None,
            ),
        )
        .unwrap();
    for turn in 1..=3 {
        if turn > 1 {
            store
                .append_canonical(&user_message(
                    &format!("selected-after-checkpoint-{turn}"),
                    "new exact body",
                ))
                .unwrap();
        }
        let (snapshot, expected) = selected_snapshot(&store, turn);
        projection::take_candidate_decodes();
        take_request_input_reconstructions();
        store
            .commit_model_turn_start(&[], &snapshot, Utc::now())
            .unwrap();
        assert_eq!(
            projection::take_candidate_decodes(),
            0,
            "identity-only head check must use the authenticated finite selector"
        );
        assert_eq!(take_request_input_reconstructions(), (0, 0));
        assert_eq!(
            store
                .reconstruct_model_request(&snapshot.request_id)
                .unwrap(),
            expected
        );
    }
}

pub(super) fn selected_tool_batch(id: &str) -> [MessageBlock; 2] {
    let call = crate::tools::types::ToolCall {
        id: ToolCallId::new("reused-wire"),
        tool_id: ToolId::new("native-tool"),
        name: "native-tool".into(),
        arguments: serde_json::json!({"value":id}),
    };
    let assistant = MessageBlock::Assistant(AssistantMessageBlock {
        id: MessageId::new(format!("selected-call-{id}")),
        content: vec![AssistantContentBlock::ToolCall(call.clone())],
    });
    let result = MessageBlock::Tool(ToolMessageBlock {
        id: MessageId::new(format!("selected-result-{id}")),
        occurrence: crate::message::types::ToolCallOccurrenceRef::new(
            assistant.id().clone(),
            ContentBlockIndex::new(0),
        ),
        tool_call_id: call.id,
        tool_id: call.tool_id,
        result: crate::scripted_suites::support::fake::success_result("original exact result"),
    });
    [assistant, result]
}
#[test]
fn native_manifest_tool_history_text_append_reuses_validated_relationships_and_exact_wire_labels() {
    let store = store();
    store
        .initialize(&[user_message("selected-tool-base", "original base")])
        .unwrap();
    store.projected_history().unwrap();
    for batch in ["first", "second"] {
        store
            .append_canonical_batch(&selected_tool_batch(batch))
            .unwrap();
    }
    for turn in 1..=4 {
        store
            .append_canonical(&user_message(
                &format!("selected-after-tool-{turn}"),
                "new ordinary text",
            ))
            .unwrap();
        let (snapshot, request) = selected_snapshot(&store, turn);
        assert_eq!(snapshot.tool_associations.len(), 2);
        assert_eq!(
            snapshot.tool_associations[0].wire_id.as_ref().unwrap().0,
            "rustx_occurrence_0"
        );
        assert_eq!(
            snapshot.tool_associations[1].wire_id.as_ref().unwrap().0,
            "reused-wire"
        );
        take_request_input_reconstructions();
        store
            .commit_model_turn_start(&[], &snapshot, Utc::now())
            .unwrap();
        assert_eq!(
            take_request_input_reconstructions(),
            (0, 0),
            "a completed Tool batch cannot force old canonical body revalidation forever"
        );
        assert_eq!(
            store
                .reconstruct_model_request(&snapshot.request_id)
                .unwrap(),
            request
        );
        assert_eq!(
            store.load_request_snapshot(&snapshot.request_id).unwrap(),
            snapshot
        );
    }
}
#[test]
fn native_manifest_tool_relationship_mutation_revokes_warm_proof_and_rejects_without_effects() {
    let store = store();
    store
        .initialize(&[user_message("selected-tool-mutation-base", "original base")])
        .unwrap();
    store
        .append_canonical_batch(&selected_tool_batch("mutation"))
        .unwrap();
    let (snapshot, request) = selected_snapshot(&store, 1);
    let origin = snapshot.input.origin.as_ref().unwrap();
    assert!(snapshot.tool_associations.certifies(origin, &snapshot));
    let encoded = serde_json::to_vec(&snapshot.tool_associations).unwrap();
    let decoded: crate::model::occurrence::FrozenToolAssociations =
        serde_json::from_slice(&encoded).unwrap();
    assert_eq!(decoded, snapshot.tool_associations);
    assert!(
        !decoded.certifies(origin, &snapshot),
        "wire bytes cannot manufacture a native owner certificate"
    );
    let before = store.read_events(None, 100).unwrap().events;
    for kind in 0..3 {
        let mut changed = snapshot.clone();
        match kind {
            0 => changed.tool_associations.clear(),
            1 => {
                changed.tool_associations[0].wire_id = Some(
                    crate::model::occurrence::ProviderWireCorrelationId("invented-wire".into()),
                );
            }
            _ => changed.tool_associations[0].source.block = ContentBlockIndex::new(99),
        }
        assert!(!changed.tool_associations.certifies(origin, &changed));
        assert!(matches!(
            store.commit_model_turn_start(&[], &changed, Utc::now()),
            Err(ConversationStoreError::InvalidReference(_))
        ));
        assert_eq!(store.read_events(None, 100).unwrap().events, before);
    }
    assert!(
        snapshot.tool_associations.certifies(origin, &snapshot),
        "mutating a clone preserves the original immutable proof"
    );
    store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap();
    assert_eq!(
        store
            .reconstruct_model_request(&snapshot.request_id)
            .unwrap(),
        request
    );
}

#[test]
fn committed_native_input_evidence_binds_exact_prepared_values_without_history_reconstruction() {
    let store = store();
    store
        .initialize(&[user_message("commit-proof-original", "exact original body")])
        .unwrap();
    let (snapshot, request) = selected_snapshot(&store, 1);
    take_request_input_reconstructions();
    let receipt = store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap();
    assert_eq!(take_request_input_reconstructions(), (0, 0));
    assert!(receipt.accepts_prepared(&snapshot, &request));
    let mut changed = request.clone();
    changed.messages[0] = user_message("commit-proof-original", "changed body").into();
    assert!(!receipt.accepts_prepared(&snapshot, &changed));
    let mut changed = snapshot.clone();
    changed.effective_system_prompt.push_str(" changed");
    assert!(!receipt.accepts_prepared(&changed, &request));
    let mut absent = receipt.clone();
    absent.verified_snapshot = None;
    assert!(!absent.accepts_prepared(&snapshot, &request));
    let replay = store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap();
    assert!(replay.accepts_prepared(&snapshot, &request));
    assert_eq!(replay.started, receipt.started);
}
