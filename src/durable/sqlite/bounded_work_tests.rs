//! Retained history must not multiply bounded native reads or warm turn SQL.
use super::query_work::{self, HISTORY_SIZES, Work};
use super::test_support::*;
use super::*;
use crate::durable::{
    CanonicalStore, JournalStore, ObservationStore, ProjectionStore, RequestStore,
};
use chrono::Utc;
use rusqlite::params;
use std::collections::BTreeMap;

fn rebind(snapshot: &mut RequestSnapshot, attempt: &str) {
    snapshot.identity.attempt_id = AttemptId::new(attempt);
    snapshot.request_id = snapshot.identity.request_id();
    snapshot.provisional_message_id = snapshot.identity.provisional_message_id();
}

// Match native request assembly's shared selection, rather than rebuilding an
// external caller's complete owned input in a fixture preparation helper.
pub(super) fn freeze_native_input(store: &SqliteConversationStore, snapshot: &mut RequestSnapshot) {
    let history = store.projected_history().unwrap();
    snapshot.history_cut = history.cut;
    let request = crate::model::types::ModelRequest {
        response_schema: None,
        response_byte_limit: None,
        continuation_source: None,
        images: snapshot.images.clone(),
        invocation: snapshot.invocation.clone(),
        messages: history.messages,
        tools: snapshot.tool_definitions.clone(),
        effective_system_prompt: snapshot.effective_system_prompt.clone(),
        continuation: snapshot.continuation.clone(),
    };
    snapshot
        .freeze_selected_input(store.conversation_id(), &request, &history.input_manifest)
        .unwrap();
}

pub(super) fn history(size: usize) -> SqliteConversationStore {
    let store = store();
    store.initialize(&[user_message("seed", "seed")]).unwrap();
    let mut snapshot =
        progress_request_snapshot("seed", "1", 0, store.load_head().unwrap().revision, vec![]);
    freeze_native_input(&store, &mut snapshot);
    for ordinal in 0..size {
        rebind(&mut snapshot, &format!("history-{ordinal:06}"));
        store
            .commit_model_turn_start(&[], &snapshot, Utc::now())
            .unwrap();
    }
    let messages = (0..size)
        .map(|ordinal| user_message(&format!("message-{ordinal:06}"), "bounded"))
        .collect::<Vec<_>>();
    store.append_canonical_batch(&messages).unwrap();
    // Populate native selection and Source certificates before measuring warm
    // turn SQL. Cold hydration is tested separately with a fixed selected tail.
    let mut snapshot =
        progress_request_snapshot("warm", "1", 0, store.load_head().unwrap().revision, vec![]);
    freeze_native_input(&store, &mut snapshot);
    store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap();
    store
}

fn select_fixed_seed(store: &SqliteConversationStore) {
    use crate::context::checkpoint::{CheckpointCandidate, CheckpointItem, CheckpointMessage};
    use crate::durable::projection::ProjectionCommitInput;
    use crate::model::projected::CanonicalMessageRef;
    use crate::runtime::cancellation::CancellationSignal;
    use crate::runtime::identity::{CheckpointId, CompactionOperationId};
    use crate::runtime::types::{TokenMeasurement, TokenMeasurementSource};

    let operation_id = CompactionOperationId::new("fixed-selection");
    let cut = store.compaction_resource_cut().unwrap();
    let candidate = CheckpointCandidate {
        checkpoint_id: CheckpointId::new("fixed-selection"),
        expected_head: None,
        base_frontier: cut.ledger,
        read_cut: cut,
        items: vec![CheckpointItem {
            message: CheckpointMessage::Original(CanonicalMessageRef::of(
                store.conversation_id(),
                &user_message("seed", "seed"),
            )),
            call_sources: vec![],
            result_call: None,
        }],
        summary_metadata: None,
        summary_generation: None,
    };
    store
        .commit_projection(ProjectionCommitInput {
            grant: CancellationSignal::new()
                .authorize_compaction_commit(operation_id.clone())
                .unwrap(),
            operation_id,
            candidate,
            owner: store.projection_owner(None).unwrap(),
            tokens_before: TokenMeasurement {
                input_tokens: 100,
                source: TokenMeasurementSource::Estimated,
            },
            estimated_tokens_after: 10,
            occupancy: None,
            turn_id: None,
            timestamp: Utc::now(),
        })
        .unwrap();
    assert_eq!(store.projected_history().unwrap().messages.len(), 1);
}

#[test]
fn bounded_work_message_windows_are_flat_at_old_and_current_cuts() {
    let mut samples = [vec![], vec![], vec![]];
    for size in HISTORY_SIZES {
        let store = history(size);
        let connection = store.lock().unwrap();
        for (index, (after, through)) in [(0, 16), (16, 24), (size - 8, size)]
            .into_iter()
            .enumerate()
        {
            let work = query_work::measure(
                &connection,
                observation::MESSAGE_HEADERS_SQL,
                params![after, through],
            );
            assert_eq!(work.rows, if index == 0 { 16 } else { 8 });
            samples[index].push((size, work));
        }
    }
    for (index, samples) in samples.iter().enumerate() {
        query_work::assert_flat(samples, 250, if index == 0 { 16 } else { 8 }, 16 * 16_384);
    }
}

#[test]
fn bounded_work_actual_warm_request_and_tool_turn_sql() {
    assert_warm_request_and_tool_turn_sql(false);
}

#[test]
fn bounded_work_actual_warm_turn_sql_calls_are_flat_with_fixed_selected_input() {
    assert_warm_request_and_tool_turn_sql(true);
}

fn assert_warm_request_and_tool_turn_sql(fixed_selection: bool) {
    use super::observation::tests::inspection::{
        inspection_assistant, inspection_request_snapshot, inspection_result,
    };
    let mut samples = Vec::new();
    for size in HISTORY_SIZES.map(|size| size * 2) {
        let store = history(size);
        if fixed_selection {
            // Grow retained H while holding selected C fixed. The original
            // growing-C case also checks each statement's bounded access path,
            // but its persistent input-tree traversal legitimately adds calls.
            select_fixed_seed(&store);
        }
        let mut request = inspection_request_snapshot(&store, 0, "fixed definition");
        freeze_native_input(&store, &mut request);
        query_work::capture(&store.lock().unwrap());
        store
            .commit_model_turn_start(&[], &request, Utc::now())
            .unwrap();
        let (_, event, _) = inspection_assistant(&store, &request);
        inspection_result(&store, &request);
        let cut = store.observation_read_cut().unwrap();
        let page = observation::messages(
            &store.lock().unwrap(),
            store.conversation_id(),
            &cut,
            None,
            8,
        )
        .unwrap();
        assert_eq!(page.headers.len(), 8);
        store.read_events(Some(event.sequence - 1), 1).unwrap();
        // Full snapshot hydration returns the selected input (O(C) bytes).
        // Pin its bounded header selector here; reconstruction is a separate API.
        let headers = query_work::measure(
            &store.lock().unwrap(),
            request::SNAPSHOT_HEADERS_SQL,
            params![event.sequence - 2, 1],
        );
        assert_eq!(headers.rows, 1);
        store.unsettled_canonical_tool_calls(8).unwrap();
        store
            .message_append_revision(&request.provisional_message_id)
            .unwrap();
        samples.push((size, query_work::finish(&store.lock().unwrap())));
    }
    let first = &samples[0].1;
    assert!(
        first.len() > 100,
        "the audit must execute the native pipeline: {}",
        first.len()
    );
    for (size, statements) in &samples {
        for (sql, work) in statements {
            let baseline = first
                .get(sql)
                .unwrap_or_else(|| panic!("new statement at {size}: {sql}"));
            assert_eq!(work.sorts, 0, "sort at {size}: {sql}");
            assert_eq!(work.automatic, 0, "automatic index at {size}: {sql}");
            if fixed_selection {
                assert_eq!(work.calls, baseline.calls, "SQL calls at {size}: {sql}");
            }
            assert!(
                work.steps <= baseline.steps + 16,
                "history {size}, VM {} -> {}: {sql}",
                baseline.steps,
                work.steps
            );
            assert!(
                work.scans <= baseline.scans + 2,
                "history {size}, scan {} -> {}: {sql}",
                baseline.scans,
                work.scans
            );
        }
        assert_eq!(
            statements.len(),
            first.len(),
            "statement coverage at {size}"
        );
    }
    for (sql, work) in first {
        println!(
            "{} VM steps, {} scans, {} calls: {sql}",
            work.steps, work.scans, work.calls
        );
    }
}

#[test]
fn bounded_work_cut_and_scoped_heads_do_not_scan_unrelated_history() {
    let mut samples: BTreeMap<&str, Vec<(usize, Work)>> = BTreeMap::new();
    for size in HISTORY_SIZES {
        let store = history(size);
        let connection = store.lock().unwrap();
        for (name, sql) in [
            ("read cut", reading::CUT_SQL),
            ("statistics", event_heads::STATISTICS),
            ("trace total", event_heads::TRACE_TOTAL),
        ] {
            let work = if name == "read cut" {
                query_work::measure(&connection, sql, [])
            } else {
                query_work::measure(&connection, sql, [8])
            };
            samples.entry(name).or_default().push((size, work));
        }
        for (name, sql) in [
            ("execution", event_heads::EXECUTION),
            ("members", event_heads::MEMBERS),
        ] {
            samples.entry(name).or_default().push((
                size,
                query_work::measure(&connection, sql, params!["absent", size]),
            ));
        }
    }
    for (name, samples) in samples {
        println!("{name}: {samples:?}");
        query_work::assert_flat(&samples, 300, 1, 100_000);
    }
}

#[test]
fn bounded_work_sparse_activity_and_transcript_anchor_at_old_cuts() {
    use chrono::TimeZone;
    let mut activity = vec![];
    let mut anchors = vec![];
    for size in HISTORY_SIZES {
        let store = store();
        let mut first = user_message("first", "first");
        let MessageBlock::User(user) = &mut first else {
            unreachable!()
        };
        user.timestamp = Some(Utc.timestamp_opt(10, 900).unwrap());
        store.initialize(&[first]).unwrap();
        let old_cut = store.observation_read_cut().unwrap();
        let hidden = (0..size)
            .map(|ordinal| request_context_message(&format!("hidden-{ordinal}"), "hidden"))
            .collect::<Vec<_>>();
        store.append_canonical_batch(&hidden).unwrap();
        // A time-ordered index would have to reject every later human row to
        // answer old_cut. Hidden context independently stresses the anchor gap.
        let later = (0..size)
            .map(|ordinal| {
                let mut message = user_message(&format!("later-{ordinal}"), "later");
                let MessageBlock::User(user) = &mut message else {
                    unreachable!()
                };
                user.timestamp = Some(Utc.timestamp_opt(10, 950).unwrap());
                message
            })
            .collect::<Vec<_>>();
        store.append_canonical_batch(&later).unwrap();
        let connection = store.lock().unwrap();
        activity.push((
            size,
            query_work::measure(
                &connection,
                canonical::HUMAN_ACTIVITY_SQL,
                [old_cut.ledger_through],
            ),
        ));
        anchors.push((
            size,
            query_work::measure(
                &connection,
                observation_projection::INHERITED_TRANSCRIPT_ANCHOR_SQL,
                [size + 1],
            ),
        ));
        let anchor: u64 = connection
            .query_row(
                observation_projection::INHERITED_TRANSCRIPT_ANCHOR_SQL,
                [size + 1],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(anchor, old_cut.read.transcript);
        drop(connection);
        let mut expected = Utc.timestamp_opt(10, 950).unwrap();
        // Earlier seconds, earlier subsecond, later subsecond, later seconds.
        for (ordinal, (seconds, nanos)) in [(9, 999), (10, 800), (10, 999), (11, 0)]
            .into_iter()
            .enumerate()
        {
            let time = Utc.timestamp_opt(seconds, nanos).unwrap();
            let mut message = user_message(&format!("timed-{ordinal}"), "time");
            let MessageBlock::User(user) = &mut message else {
                unreachable!()
            };
            user.timestamp = Some(time);
            store.append_canonical(&message).unwrap();
            expected = expected.max(time);
            let cut = store.observation_read_cut().unwrap();
            assert_eq!(
                canonical::complete_canonical_scalars(&store.lock().unwrap(), &cut)
                    .unwrap()
                    .human_activity_at,
                Some(expected)
            );
            assert_eq!(
                canonical::complete_canonical_scalars(&store.lock().unwrap(), &old_cut)
                    .unwrap()
                    .human_activity_at,
                Some(Utc.timestamp_opt(10, 900).unwrap())
            );
        }
    }
    query_work::assert_flat(&activity, 40, 1, 16);
    query_work::assert_flat(&anchors, 40, 1, 8);
}

#[test]
fn bounded_work_projection_owner_seeks_past_a_long_unrelated_tail() {
    let mut found = vec![];
    let mut absent = vec![];
    for size in HISTORY_SIZES {
        let store = store();
        let attempt = AttemptId::new("owner");
        let start = store
            .append_event(envelope(
                store.conversation_id(),
                "start",
                Some(attempt.clone()),
                RuntimeEvent::AttemptStarted {
                    attempt_id: attempt.clone(),
                },
            ))
            .unwrap();
        let connection = store.lock().unwrap();
        connection.execute("WITH RECURSIVE n(x) AS (VALUES(2) UNION ALL SELECT x+1 FROM n WHERE x<=?1) INSERT INTO events(sequence,event_id,schema_version,conversation_id,event_timestamp,event_json) SELECT x,'unrelated-'||x,1,(SELECT conversation_id FROM rustx_store WHERE id=1),'time','{\"event\":{\"type\":\"unrelated\"}}' FROM n", [size]).unwrap();
        found.push((
            size,
            query_work::measure(
                &connection,
                projection::OWNER_KIND_SQL,
                [event_kind_key::parameter("attempt_started")],
            ),
        ));
        absent.push((
            size,
            query_work::measure(
                &connection,
                projection::OWNER_KIND_SQL,
                [event_kind_key::parameter("attempt_failed")],
            ),
        ));
        let owner = projection::owner(&connection, Some(&attempt)).unwrap();
        assert_eq!(owner.transition_sequence, start.sequence);
        assert_eq!(owner.attempt_id.as_ref(), Some(&attempt));
        assert!(projection::owner(&connection, None).is_err());
    }
    query_work::assert_flat(&found, 40, 1, 4096);
    query_work::assert_flat(&absent, 20, 0, 0);
}

#[test]
fn bounded_work_message_pages_preserve_cuts_gaps_and_wrong_kinds() {
    let store = store();
    let messages = (0..64)
        .map(|i| user_message(&format!("m-{i}"), "body"))
        .collect::<Vec<_>>();
    store.initialize(&messages).unwrap();
    let old = store.observation_read_cut().unwrap();
    store
        .append_canonical(&user_message("future", "future"))
        .unwrap();
    for requested in [1, 8, 32] {
        for offset in [0, 17, 63, 64] {
            let page = observation::messages(
                &store.lock().unwrap(),
                store.conversation_id(),
                &old,
                Some(offset),
                requested,
            )
            .unwrap();
            let end = 64.min(offset + requested as u64);
            assert_eq!(
                page.headers
                    .iter()
                    .map(|h| h.ledger_position.unwrap())
                    .collect::<Vec<_>>(),
                ((offset + 1)..=end).collect::<Vec<_>>()
            );
            assert_eq!(page.total, 64);
            assert_eq!(page.next_offset, (end < 64).then_some(end));
        }
    }
    for damage in [
        "UPDATE transcript_entries SET header_json=NULL,reference_kind=NULL,reference_id=NULL,ledger_position=NULL WHERE ledger_position=20",
        "UPDATE transcript_entries SET reference_kind='wrong-kind' WHERE ledger_position=20",
        "UPDATE transcript_entries SET header_json=X'7b' WHERE ledger_position=20",
    ] {
        let connection = store.lock().unwrap();
        connection.execute_batch("SAVEPOINT damage").unwrap();
        connection.execute_batch(damage).unwrap();
        assert!(
            observation::messages(&connection, store.conversation_id(), &old, Some(17), 8).is_err()
        );
        connection
            .execute_batch("ROLLBACK TO damage; RELEASE damage")
            .unwrap();
    }
    let connection = store.lock().unwrap();
    let plan = connection
        .prepare(&format!(
            "EXPLAIN QUERY PLAN {}",
            observation::MESSAGE_HEADERS_SQL
        ))
        .unwrap()
        .query_map(params![17, 25], |row| row.get::<_, String>(3))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
        .join("\n");
    assert!(plan.contains("transcript_entries_ledger_idx"), "{plan}");
    assert!(!plan.contains("TEMP B-TREE"), "{plan}");
    connection
        .execute_batch("DROP INDEX transcript_entries_ledger_idx")
        .unwrap();
    assert!(
        connection
            .prepare(observation::MESSAGE_HEADERS_SQL)
            .is_err(),
        "the range access path is mandatory"
    );
}

#[test]
fn bounded_work_append_revision_uses_canonical_metadata_and_survives_reopen() {
    use crate::durable::sqlite::schema::shape::verify_schema_shape;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("store.sqlite");
    let conversation = ConversationId::generate();
    let store = SqliteConversationStore::open(conversation.clone(), &path).unwrap();
    store.initialize(&[user_message("seed", "seed")]).unwrap();
    store
        .append_canonical(&request_context_message("hidden", "hidden"))
        .unwrap();
    store
        .append_canonical(&user_message("visible", "visible"))
        .unwrap();
    let ids = ["seed", "hidden", "visible", "absent"];
    let expected: Vec<_> = ids
        .iter()
        .map(|id| store.message_append_revision(&MessageId::new(*id)).unwrap())
        .collect();
    for (id, actual) in ids.iter().zip(&expected) {
        let original: Option<u64> = store.lock().unwrap().query_row(
            "SELECT revision FROM surface_ops WHERE ledger_position=(SELECT position FROM message_ledger WHERE message_id=?1) ORDER BY revision LIMIT 1", [id], |row| row.get(0),
        ).optional().unwrap();
        assert_eq!(actual.map(SurfaceRevision::get), original);
    }
    assert!(expected[2].is_some());
    assert_eq!(expected[3], None);
    {
        let connection = store.lock().unwrap();
        for name in [
            "message_ledger_human_activity_idx",
            "message_ledger_transcript_position_idx",
            "publication_streams_owner_idx",
            "publication_proposals_call_idx",
        ] {
            connection
                .execute_batch(&format!("SAVEPOINT bad_index; DROP INDEX {name}"))
                .unwrap();
            assert!(verify_schema_shape(&connection).is_err());
            connection
                .execute_batch("ROLLBACK TO bad_index; RELEASE bad_index")
                .unwrap();
        }
    }
    drop(store);
    let store = SqliteConversationStore::open_existing(conversation.clone(), &path).unwrap();
    for (id, expected) in ids.into_iter().zip(expected) {
        assert_eq!(
            store.message_append_revision(&MessageId::new(id)).unwrap(),
            expected
        );
    }
    drop(store);
    Connection::open(&path)
        .unwrap()
        .execute(
            "UPDATE rustx_store SET schema_version=?1",
            [SQLITE_SCHEMA_VERSION - 1],
        )
        .unwrap();
    assert!(matches!(
        SqliteConversationStore::open_existing(conversation, &path),
        Err(ConversationStoreError::SchemaVersionMismatch { .. })
    ));
}

#[test]
#[allow(clippy::too_many_lines)] // One fixture checks all owner scopes and their ambiguity/error order.
fn bounded_work_publication_queries_prune_reused_wire_ids_by_owner() {
    use crate::durable::PublicationStore;
    use crate::durable::sqlite::publication::proposal::{
        canonical_generation_owner, record_tool_proposal_dependency,
    };
    use crate::runtime::identity::{ToolCallId, ToolId, TurnId};
    use crate::tools::types::ToolCall;
    let mut samples = vec![];
    for size in HISTORY_SIZES {
        let store = store();
        let assistant = MessageBlock::Assistant(crate::message::types::AssistantMessageBlock {
            id: MessageId::new("chosen-message"),
            content: vec![AssistantContentBlock::ToolCall(ToolCall {
                id: ToolCallId::new("reused"),
                tool_id: ToolId::new("tool"),
                name: "tool".into(),
                arguments: serde_json::json!({}),
            })],
        });
        store.initialize(&[assistant]).unwrap();
        let mut connection = store.lock().unwrap();
        connection.execute("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<?1) INSERT INTO publication_streams(stream_id,attempt_id,turn_id,request_id,message_id,next_frame_sequence,settlement) SELECT 'stream-'||x,'old-'||x,'1','request-'||x,'old-message-'||x,1,'canonical' FROM n", [size]).unwrap();
        connection.execute_batch("INSERT INTO publication_streams(stream_id,attempt_id,turn_id,request_id,message_id,next_frame_sequence,settlement) VALUES('chosen','chosen','1','chosen-request','chosen-message',1,'canonical'); INSERT INTO publication_proposals(stream_id,call_id,block_index,tool_id,tool_name,state,executed,settlement) SELECT stream_id,'reused',0,'tool','tool','completed',0,'canonical' FROM publication_streams;").unwrap();
        let attempt = AttemptId::new("chosen");
        let turn = TurnId::new("1");
        let call = ToolCallId::new("reused");
        let message = MessageId::new("chosen-message");
        query_work::capture(&connection);
        let transaction = connection.transaction().unwrap();
        assert_eq!(
            canonical_generation_owner(&transaction, &call, &attempt, &turn).unwrap(),
            Some(message.clone())
        );
        assert_eq!(
            canonical_generation_owner(&transaction, &call, &AttemptId::new("absent"), &turn)
                .unwrap(),
            None
        );
        record_tool_proposal_dependency(
            &transaction,
            &call,
            Some(&attempt),
            Some(&turn),
            None,
            "test",
            false,
            None,
        )
        .unwrap();
        record_tool_proposal_dependency(
            &transaction,
            &call,
            None,
            None,
            None,
            "test",
            false,
            Some(&message),
        )
        .unwrap();
        record_tool_proposal_dependency(&transaction, &call, None, None, None, "test", false, None)
            .unwrap();
        transaction.commit().unwrap();
        drop(connection);
        assert!(
            store
                .load_unsettled_publication_streams()
                .unwrap()
                .is_empty()
        );
        let mut connection = store.lock().unwrap();
        samples.push((size, query_work::finish(&connection)));
        let transaction = connection.transaction().unwrap();
        let mismatch = record_tool_proposal_dependency(
            &transaction,
            &call,
            Some(&AttemptId::new("absent")),
            Some(&turn),
            None,
            "test",
            false,
            None,
        )
        .unwrap_err();
        assert!(
            mismatch
                .to_string()
                .contains("does not match any exact publication generation")
        );
        transaction
            .execute(
                "UPDATE publication_proposals SET settlement='unaccepted' WHERE stream_id='chosen'",
                [],
            )
            .unwrap();
        assert!(
            record_tool_proposal_dependency(
                &transaction,
                &call,
                Some(&attempt),
                Some(&turn),
                None,
                "test",
                false,
                None
            )
            .unwrap_err()
            .to_string()
            .contains("unaccepted")
        );
        transaction.rollback().unwrap();
        connection.execute_batch("INSERT INTO publication_streams(stream_id,attempt_id,turn_id,request_id,message_id,next_frame_sequence,settlement) VALUES('duplicate','chosen','1','duplicate-request','chosen-message',1,'canonical'); INSERT INTO publication_proposals(stream_id,call_id,block_index,tool_id,tool_name,state,executed,settlement) VALUES('duplicate','reused',0,'tool','tool','completed',0,'canonical');").unwrap();
        let transaction = connection.transaction().unwrap();
        assert!(
            canonical_generation_owner(&transaction, &call, &attempt, &turn)
                .unwrap_err()
                .to_string()
                .contains("ambiguous")
        );
        assert!(
            record_tool_proposal_dependency(
                &transaction,
                &call,
                Some(&attempt),
                Some(&turn),
                None,
                "test",
                false,
                None
            )
            .unwrap_err()
            .to_string()
            .contains("ambiguous canonical")
        );
    }
    let baseline = &samples[0].1;
    for (history, statements) in &samples {
        assert_eq!(statements.len(), baseline.len(), "history {history}");
        for (sql, work) in statements {
            let first = &baseline[sql];
            assert_eq!(work.steps, first.steps, "history {history}: {sql}");
            assert_eq!(work.calls, first.calls, "history {history}: {sql}");
            assert_eq!(work.sorts, 0, "history {history}: {sql}");
            assert_eq!(work.automatic, 0, "automatic index at {history}: {sql}");
            assert_eq!(work.scans, first.scans, "history {history}: {sql}");
        }
    }
}

#[test]
fn bounded_work_cold_tail_resource_sql_reads_only_the_selected_page() {
    use crate::context::resources::{LedgerFrontier, ResourceLimits};
    let mut samples = vec![];
    for size in HISTORY_SIZES {
        let store = store();
        store
            .initialize(
                &(0..size)
                    .map(|i| user_message(&format!("cold-{i:06}"), "fixed body"))
                    .collect::<Vec<_>>(),
            )
            .unwrap();
        let cut = store.compaction_resource_cut().unwrap();
        query_work::capture(&store.lock().unwrap());
        let page = compaction_resources::messages(
            &store.lock().unwrap(),
            store.conversation_id(),
            &cut,
            LedgerFrontier((size - 8) as u64),
            None,
            ResourceLimits::default(),
        )
        .unwrap();
        assert_eq!(page.rows.len(), 8);
        assert!(page.bytes < 4096);
        samples.push((size, query_work::finish(&store.lock().unwrap())));
    }
    let baseline = &samples[0].1;
    for (history, statements) in &samples {
        for (sql, work) in statements {
            assert_eq!(work.steps, baseline[sql].steps, "history {history}: {sql}");
            assert_eq!(work.sorts, 0, "history {history}: {sql}");
            assert_eq!(work.automatic, 0, "history {history}: {sql}");
            assert_eq!(work.calls, baseline[sql].calls, "history {history}: {sql}");
        }
    }
}

#[test]
fn bounded_work_projection_owner_preserves_every_terminal_kind_and_latest_transition() {
    use crate::events::types::{AttemptFailure, AttemptLimit};
    use crate::model::ModelFinishReason;
    use crate::runtime::types::{CancellationReason, RuntimeError};
    let store = store();
    let attempt = AttemptId::new("owner");
    store
        .append_event(envelope(
            store.conversation_id(),
            "start",
            Some(attempt.clone()),
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt.clone(),
            },
        ))
        .unwrap();
    let connection = store.lock().unwrap();
    for event in [
        RuntimeEvent::AttemptCompleted {
            attempt_id: attempt.clone(),
            finish_reason: ModelFinishReason::Stop,
        },
        RuntimeEvent::AttemptCancelled {
            attempt_id: attempt.clone(),
            reason: CancellationReason::UserRequested,
        },
        RuntimeEvent::AttemptTimedOut {
            attempt_id: attempt.clone(),
        },
        RuntimeEvent::AttemptLimitExceeded {
            attempt_id: attempt.clone(),
            limit: AttemptLimit::MaxToolCalls,
        },
        RuntimeEvent::AttemptFailed {
            attempt_id: attempt.clone(),
            error: AttemptFailure::Runtime {
                error: RuntimeError::Internal {
                    message: "failed".into(),
                },
            },
        },
    ] {
        let mut event = envelope(
            store.conversation_id(),
            "start",
            Some(attempt.clone()),
            event,
        );
        event.sequence = 1;
        connection
            .execute(
                "UPDATE events SET event_json=?1 WHERE sequence=1",
                [serde_json::to_string(&event).unwrap()],
            )
            .unwrap();
        let owner = projection::owner(&connection, None).unwrap();
        assert_eq!(owner.transition_sequence, 1);
        assert_eq!(owner.attempt_id, None);
        assert!(projection::owner(&connection, Some(&attempt)).is_err());
    }
    let mut event = envelope(
        store.conversation_id(),
        "later",
        Some(attempt.clone()),
        RuntimeEvent::AttemptStarted {
            attempt_id: attempt.clone(),
        },
    );
    event.sequence = 2;
    connection.execute("INSERT INTO events(sequence,event_id,schema_version,conversation_id,event_timestamp,attempt_id,event_json) VALUES(2,'later',1,?1,'time',?2,?3)", params![store.conversation_id().as_str(), attempt.as_str(), serde_json::to_string(&event).unwrap()]).unwrap();
    assert_eq!(
        projection::owner(&connection, Some(&attempt))
            .unwrap()
            .transition_sequence,
        2
    );
}

#[test]
fn bounded_work_message_probe_reproduces_and_rejects_the_old_scan_sort_plan() {
    let mut current = vec![];
    let mut previous = 0;
    for size in HISTORY_SIZES {
        let store = store();
        store
            .initialize(
                &(0..size)
                    .map(|i| user_message(&format!("probe-{i:06}"), "body"))
                    .collect::<Vec<_>>(),
            )
            .unwrap();
        let connection = store.lock().unwrap();
        let old = query_work::measure(
            &connection,
            "SELECT h.header_json FROM transcript_entries h WHERE h.ledger_position>?1 AND h.ledger_position<=?2 AND h.reference_kind='message' ORDER BY h.ledger_position LIMIT ?3",
            params![size - 8, size, 8],
        );
        let new = query_work::measure(
            &connection,
            observation::MESSAGE_HEADERS_SQL,
            params![size - 8, size],
        );
        assert_eq!(old.rows, 8);
        assert_eq!(
            old.sorts, 1,
            "negative control must reproduce the original planner defect"
        );
        assert!(old.steps > previous);
        previous = old.steps;
        assert_eq!(new.rows, 8);
        println!("message history {size}: old {old:?}; new {new:?}");
        current.push((size, new));
    }
    query_work::assert_flat(&current, 100, 8, 8 * 16_384);
}

#[test]
fn bounded_work_workflow_tool_parent_seeks_kind_without_a_turn_prefix_gap() {
    let mut samples = vec![];
    for size in HISTORY_SIZES {
        let store = store();
        let connection = store.lock().unwrap();
        connection.execute("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<=?1) INSERT INTO events(sequence,event_id,schema_version,conversation_id,event_timestamp,attempt_id,turn_id,event_json) SELECT x,'tool-parent-'||x,1,(SELECT conversation_id FROM rustx_store WHERE id=1),'time','attempt',CAST(x AS TEXT),json_object('event',json_object('type','tool_execution_failed','tool_call_id','reused')) FROM n", [size]).unwrap();
        let empty = query_work::measure(
            &connection,
            workflow_trace::TOOL_START_SQL,
            params!["reused", "attempt", size],
        );
        assert_eq!(empty.rows, 0);
        samples.push((size, empty));
        connection.execute("UPDATE events SET event_json=json_set(event_json,'$.event.type','tool_execution_started') WHERE sequence IN (1,2)", []).unwrap();
        let ambiguous = query_work::measure(
            &connection,
            workflow_trace::TOOL_START_SQL,
            params!["reused", "attempt", size],
        );
        assert_eq!(ambiguous.rows, 2);
        assert_eq!(ambiguous.sorts, 0);
        assert!(ambiguous.steps < 60);
    }
    // Owner normalization adds a fixed metadata seek; exact step equality
    // across the hundredfold history range remains mandatory.
    query_work::assert_flat(&samples, 40, 0, 0);
}

#[test]
fn bounded_work_turn_prompt_and_holes_seek_exact_owner_and_old_cut() {
    let mut winners = vec![];
    let mut holes = vec![];
    for size in HISTORY_SIZES {
        let store = store();
        let connection = store.lock().unwrap();
        connection.execute("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<?1) INSERT INTO events(sequence,event_id,schema_version,conversation_id,event_timestamp,attempt_id,event_json,observation_kind,observation_turn_prompt,observation_turn_prompt_positive) SELECT x,'prompt-'||x,1,(SELECT conversation_id FROM rustx_store WHERE id=1),'time','chosen','{}','inbound_turn_adopted',CAST('chosen' AS BLOB),1 FROM n", [size]).unwrap();
        let winner = query_work::measure(
            &connection,
            observation_turn::TURN_PROMPT_SQL,
            params!["chosen", size, 0, size],
        );
        assert_eq!(winner.rows, 1);
        assert_eq!(winner.bytes, 6);
        winners.push((size, winner));
        connection.execute_batch("UPDATE events SET observation_turn_prompt=NULL,observation_turn_prompt_positive=0,attempt_id=CASE WHEN sequence%2=0 THEN NULL ELSE 'other' END;").unwrap();
        let values = params![size + 3, "chosen", size, size + 2];
        let work =
            query_work::measure(&connection, observation_turn::TURN_PROMPT_HOLES_SQL, values);
        let missing: bool = connection
            .query_row(observation_turn::TURN_PROMPT_HOLES_SQL, values, |row| {
                row.get(0)
            })
            .unwrap();
        assert!(
            !missing,
            "unrelated old holes must not contaminate this owner"
        );
        holes.push((size, work));
        connection
            .execute("UPDATE events SET attempt_id='chosen' WHERE sequence=1", [])
            .unwrap();
        assert!(
            connection
                .query_row(observation_turn::TURN_PROMPT_HOLES_SQL, values, |row| row
                    .get::<_, bool>(
                    0
                ))
                .unwrap()
        );
        connection
            .execute("UPDATE events SET attempt_id='other' WHERE sequence=1", [])
            .unwrap();
        connection.execute("INSERT INTO events(sequence,event_id,schema_version,conversation_id,event_timestamp,event_json,observation_kind) VALUES(?1,'global-gap',1,(SELECT conversation_id FROM rustx_store WHERE id=1),'time','{}','inbound_turn_adopted')", [size+1]).unwrap();
        assert!(
            connection
                .query_row(observation_turn::TURN_PROMPT_HOLES_SQL, values, |row| row
                    .get::<_, bool>(
                    0
                ))
                .unwrap()
        );
    }
    query_work::assert_flat(&winners, 100, 1, 512);
    query_work::assert_flat(&holes, 100, 1, 8);
}

#[test]
fn bounded_work_new_reads_preserve_typed_cancellation_and_rollback() {
    use super::publication::proposal::record_tool_proposal_dependency;
    use crate::runtime::identity::{ToolCallId, TurnId};
    use rusqlite::trace::{TraceEvent, TraceEventCodes};
    use std::cell::Cell;

    thread_local! {
        static TARGET: Cell<Option<&'static str>> = const { Cell::new(None) };
        static INTERRUPT: Cell<bool> = const { Cell::new(false) };
    }
    fn arm_interrupt(event: TraceEvent<'_>) {
        if let TraceEvent::Stmt(statement, _) = event {
            TARGET.with(|target| {
                if target
                    .get()
                    .is_some_and(|text| statement.sql().contains(text))
                {
                    target.set(None);
                    INTERRUPT.with(|pending| pending.set(true));
                }
            });
        }
    }

    let store = store();
    store.initialize(&[user_message("seed", "seed")]).unwrap();
    let mut connection = store.lock().unwrap();
    connection.execute_batch("INSERT INTO publication_streams(stream_id,attempt_id,turn_id,request_id,message_id,next_frame_sequence,settlement) VALUES('chosen','chosen','1','request','message',1,'canonical'); INSERT INTO publication_proposals(stream_id,call_id,block_index,tool_id,tool_name,state,executed,settlement) VALUES('chosen','call',0,'tool','tool','completed',0,'canonical');").unwrap();
    for target in [
        "SELECT EXISTS(SELECT 1 FROM publication_proposals",
        "SELECT p.stream_id,s.attempt_id",
        canonical::HUMAN_ACTIVITY_SQL,
    ] {
        let transaction = connection.transaction().unwrap();
        transaction
            .execute("UPDATE publication_proposals SET executed=1", [])
            .unwrap();
        TARGET.with(|value| value.set(Some(target)));
        transaction.trace_v2(TraceEventCodes::SQLITE_TRACE_STMT, Some(arm_interrupt));
        transaction
            .progress_handler(1, Some(|| INTERRUPT.with(|pending| pending.replace(false))))
            .unwrap();
        let result = if target == canonical::HUMAN_ACTIVITY_SQL {
            canonical::ledger::append_seeded_ledger_message(
                &transaction,
                &user_message("cancelled", "cancelled"),
            )
        } else {
            record_tool_proposal_dependency(
                &transaction,
                &ToolCallId::new("call"),
                Some(&AttemptId::new("chosen")),
                Some(&TurnId::new("1")),
                None,
                "test",
                false,
                None,
            )
        };
        transaction.trace_v2(TraceEventCodes::empty(), None);
        transaction
            .progress_handler(0, None::<fn() -> bool>)
            .unwrap();
        assert!(
            TARGET.with(|value| value.get().is_none()),
            "missed {target}"
        );
        assert_eq!(
            result.unwrap_err(),
            ConversationStoreError::OperationCancelled,
            "{target}"
        );
        transaction.rollback().unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT executed FROM publication_proposals WHERE stream_id='chosen'",
                    [],
                    |row| row.get::<_, u64>(0)
                )
                .unwrap(),
            0
        );
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM message_ledger", [], |row| row
                    .get::<_, u64>(0))
                .unwrap(),
            1
        );
    }
}

/// Appends `turns` completed turns through the production append path: one
/// adopted inbound, one completed model request and its final Assistant per
/// turn, plus one executed read Tool call and its canonical result before the
/// final request when `tool` is set.
#[allow(clippy::too_many_lines)] // One fixture for both completed turn shapes.
pub(crate) fn append_completed_turns(
    store: &SqliteConversationStore,
    first: usize,
    turns: usize,
    tool: bool,
) {
    use crate::durable::InboxStore;
    use crate::durable::sqlite::tests::observation_response_tests::{
        response_complete, response_message, response_start,
    };
    use crate::message::types::{
        AssistantContentBlock, AssistantMessageBlock, ContentBlockIndex, ToolMessageBlock,
    };
    use crate::runtime::identity::{MessageId, ToolCallId, ToolId};
    use crate::tools::types::{ToolCall, ToolExecutionResult, ToolExecutionStatus};

    fn model_step(
        store: &SqliteConversationStore,
        attempt: &AttemptId,
        turn: &str,
        tools: bool,
    ) -> RequestSnapshot {
        let phase = super::query_work::phase;
        phase("read:request");
        let mut snapshot = progress_request_snapshot(
            attempt.as_str(),
            turn,
            0,
            store.load_head().unwrap().revision,
            vec![],
        );
        let history = store.projected_history().unwrap();
        let request = crate::model::ModelRequest {
            response_schema: None,
            response_byte_limit: None,
            continuation_source: None,
            images: snapshot.images.clone(),
            invocation: snapshot.invocation.clone(),
            messages: history.messages.clone(),
            tools: snapshot.tool_definitions.clone(),
            effective_system_prompt: snapshot.effective_system_prompt.clone(),
            continuation: snapshot.continuation.clone(),
        };
        snapshot.history_cut = history.cut.clone();
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
        phase("commit_model_turn_start");
        store
            .commit_model_turn_start(&[], &snapshot, Utc::now())
            .unwrap();
        phase("event:model_request_completed");
        let mut event = envelope(
            store.conversation_id(),
            "",
            Some(attempt.clone()),
            RuntimeEvent::ModelRequestCompleted {
                owner: crate::model::ModelRequestOwner::Agent {
                    identity: snapshot.identity.clone(),
                },
                request_id: snapshot.request_id.clone(),
                finish_reason: if tools {
                    crate::model::ModelFinishReason::ToolCalls
                } else {
                    crate::model::ModelFinishReason::Stop
                },
                usage: None,
                generation: None,
            },
        );
        event.turn_id = Some(snapshot.identity.turn.clone());
        store.append_event(event).unwrap();
        snapshot
    }

    let phase = super::query_work::phase;
    for ordinal in first..first + turns {
        phase("accept_inbound");
        let accepted = store.accept_inbound(draft("turn growth")).unwrap();
        phase("adopt_pending_batch");
        store.adopt_pending_batch(accepted.sequence, None).unwrap();
        phase("event:attempt_started");
        let attempt = response_start(store, &format!("frontier-{ordinal}"));
        let first_request = model_step(store, &attempt, "1", tool);
        let final_request = if tool {
            let call = ToolCall {
                id: ToolCallId::new(format!("call-{ordinal}")),
                tool_id: ToolId::new("read"),
                name: "read".into(),
                arguments: serde_json::json!({"path":"fixture.txt"}),
            };
            phase("canonical:tool_call");
            let proposal = MessageBlock::Assistant(AssistantMessageBlock {
                id: first_request.provisional_message_id.clone(),
                content: vec![AssistantContentBlock::ToolCall(call.clone())],
            });
            store
                .append_canonical_with_event(
                    &proposal,
                    envelope(
                        store.conversation_id(),
                        "",
                        Some(attempt.clone()),
                        RuntimeEvent::AssistantMessageCommitted {
                            message_id: proposal.id().clone(),
                        },
                    ),
                )
                .unwrap();
            phase("event:tool_started");
            store
                .append_event(envelope(
                    store.conversation_id(),
                    "",
                    Some(attempt.clone()),
                    RuntimeEvent::ToolExecutionStarted {
                        tool_call_id: call.id.clone(),
                        tool_id: call.tool_id.clone(),
                    },
                ))
                .unwrap();
            let result = ToolExecutionResult {
                status: ToolExecutionStatus::Success,
                content: vec![],
                duration_ms: 0,
                exit_code: None,
                artifacts: vec![],
                deliveries: vec![],
                truncation: None,
                workflow: None,
                managed_output: None,
            };
            phase("event:tool_completed");
            store
                .append_event(envelope(
                    store.conversation_id(),
                    "",
                    Some(attempt.clone()),
                    RuntimeEvent::ToolExecutionCompleted {
                        tool_call_id: call.id.clone(),
                        tool_id: call.tool_id.clone(),
                        result: result.clone().into(),
                    },
                ))
                .unwrap();
            phase("canonical:tool_result");
            let message = MessageBlock::Tool(ToolMessageBlock {
                id: MessageId::new(format!("result-{ordinal}")),
                occurrence: crate::message::types::ToolCallOccurrenceRef::new(
                    proposal.id().clone(),
                    ContentBlockIndex::new(0),
                ),
                tool_call_id: call.id.clone(),
                tool_id: call.tool_id,
                result: result.into(),
            });
            store
                .append_canonical_with_event(
                    &message,
                    envelope(
                        store.conversation_id(),
                        "",
                        Some(attempt.clone()),
                        RuntimeEvent::ToolMessageCommitted {
                            message_id: message.id().clone(),
                            tool_call_id: call.id,
                        },
                    ),
                )
                .unwrap();
            model_step(store, &attempt, "2", false)
        } else {
            first_request
        };
        phase("canonical:assistant");
        response_message(
            store,
            &attempt,
            final_request.provisional_message_id.as_str(),
        );
        phase("event:attempt_completed");
        response_complete(store, &attempt);
        phase("");
    }
}

/// The executed SQL of one measured operation: statement text to (calls,
/// returned rows, summed VM steps).
pub(crate) type MeasuredStatements = BTreeMap<String, (usize, usize, i64)>;

impl SqliteConversationStore {
    /// Starts tracing every statement this store's connection executes.
    pub(crate) fn begin_query_work(&self) {
        query_work::capture(&self.lock().unwrap());
    }

    /// Stops tracing and returns what executed since [`Self::begin_query_work`].
    pub(crate) fn finish_query_work(&self) -> MeasuredStatements {
        query_work::finish(&self.lock().unwrap())
            .into_iter()
            .map(|(sql, work)| (sql, (work.calls, work.rows, work.total_steps)))
            .collect()
    }
}

#[test]
fn bounded_work_canonical_tool_fallback_seeks_one_native_owner() {
    use crate::message::types::AssistantMessageBlock;
    use crate::runtime::identity::{ToolCallId, ToolId};
    use crate::tools::types::ToolCall;
    let sql = publication::proposal::CANONICAL_SURFACE_TOOL_SQL;
    let mut found = Vec::new();
    let mut absent = Vec::new();
    for size in HISTORY_SIZES {
        let store = history(size);
        let assistant = MessageBlock::Assistant(AssistantMessageBlock {
            id: MessageId::new("selected-tool-owner"),
            content: vec![AssistantContentBlock::ToolCall(ToolCall {
                id: ToolCallId::new("same-call"),
                tool_id: ToolId::new("read"),
                name: "read".into(),
                arguments: serde_json::json!({}),
            })],
        });
        store.append_canonical(&assistant).unwrap();
        let mut connection = store.lock().unwrap();
        let end: u64 = connection
            .query_row("SELECT max(position) FROM message_ledger", [], |row| {
                row.get(0)
            })
            .unwrap();
        canonical::ledger::take_head_materializations();
        {
            let transaction = connection.transaction().unwrap();
            query_work::capture(&transaction);
            assert_eq!(
                publication::proposal::canonical_surface_tool_id(
                    &transaction,
                    &ToolCallId::new("same-call")
                )
                .unwrap(),
                Some(ToolId::new("read"))
            );
            let executions = query_work::finish(&transaction);
            assert_eq!(canonical::ledger::take_head_materializations(), 0);
            assert!(
                !executions
                    .keys()
                    .any(|sql| sql == canonical::ledger::CANONICAL_SUFFIX_IDS)
            );
            assert_eq!(
                executions
                    .iter()
                    .filter(|(sql, _)| sql.contains("SELECT message_json"))
                    .map(|(_, work)| work.calls)
                    .sum::<usize>(),
                1
            );
        }
        // Observation damage must never control the native fallback owner.
        connection
            .execute(
                "UPDATE canonical_tool_calls SET observation_ledger=observation_ledger+100000",
                [],
            )
            .unwrap();
        {
            let transaction = connection.transaction().unwrap();
            assert_eq!(
                publication::proposal::canonical_surface_tool_id(
                    &transaction,
                    &ToolCallId::new("same-call")
                )
                .unwrap(),
                Some(ToolId::new("read"))
            );
        }
        // Populate a long same-call prefix behind this cut to exercise the
        // composite range seek, independently of canonical body hydration.
        connection.execute("INSERT INTO canonical_tool_calls(assistant_message_id,assistant_ledger_position,block_index,call_id,tool_id,observation_ledger,observation_tool_ordinal,observation_name) SELECT message_id,position,0,'same-call','read',position,0,'read' FROM message_ledger WHERE position<?1",[end]).unwrap();
        found.push((
            size,
            query_work::measure(&connection, sql, params!["same-call", end - 1, end]),
        ));
        absent.push((
            size,
            query_work::measure(&connection, sql, params!["absent", 0, end]),
        ));
        let plan = connection
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .unwrap()
            .query_map(params!["same-call", end - 1, end], |row| {
                row.get::<_, String>(3)
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .join("\n");
        assert!(
            plan.contains("SEARCH c USING INDEX canonical_tool_calls_call_ledger_idx"),
            "{plan}"
        );
        assert!(
            !plan.contains("SCAN ") && !plan.contains("TEMP B-TREE"),
            "{plan}"
        );
    }
    query_work::assert_flat(&found, 64, 1, 4096);
    query_work::assert_flat(&absent, 32, 0, 0);
}
