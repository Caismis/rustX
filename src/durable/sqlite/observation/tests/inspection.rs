//! Bounded observation regressions: inspection.

use crate::durable::sqlite::observation::tests::trace::deny_trace_original_columns;
use crate::durable::sqlite::observation::tests::trace::trace_arguments_reference;
use crate::durable::sqlite::test_support::*;
use crate::durable::sqlite::*;
use crate::durable::{CanonicalStore, InboxStore, JournalStore, ObservationStore, RequestStore};
use crate::durable::{ConversationStore, ConversationStoreError};
use crate::events::types::{RuntimeEvent, RuntimeEventEnvelope};
use crate::message::types::{AssistantContentBlock, ContentBlockIndex, MessageBlock};
use crate::message::types::{AssistantMessageBlock, ToolMessageBlock};
use crate::model::snapshot::RequestSnapshot;
use crate::runtime::identity::{ConversationId, MessageId, ToolCallId, ToolId};
use crate::tools::types::{ToolCall, ToolExecutionResult, ToolExecutionStatus};
use chrono::Utc;
use rusqlite::params;

// These tests use native writes; public observation is denied original J/C/R columns.
pub(super) fn inspection_request(
    store: &SqliteConversationStore,
    retry: u32,
    description: &str,
) -> (RequestSnapshot, u64) {
    let mut request = progress_request_snapshot(
        "inspection-original-owner",
        "1",
        retry,
        store.load_head().unwrap().revision,
        Vec::new(),
    );
    request.tool_definitions = vec![
        crate::tools::types::ModelToolDefinition {
            id: ToolId::new("different-original-tool"),
            name: "Other".into(),
            description: "Other original schema".into(),
            input_schema: serde_json::json!({"type":"null"}),
        },
        crate::tools::types::ModelToolDefinition {
            id: ToolId::new("same-original-tool"),
            name: format!("Original {retry}"),
            description: description.into(),
            input_schema: serde_json::json!({"opaque":u64::MAX,"zero":-0.0,"retry":retry}),
        },
        crate::tools::types::ModelToolDefinition {
            id: ToolId::new("same-original-tool"),
            name: "Later duplicate".into(),
            description: "Must never replace the native first definition".into(),
            input_schema: serde_json::json!({"duplicate":true}),
        },
    ];
    freeze_test_input(store, &mut request, &[]);
    let start = store
        .commit_model_turn_start(&[], &request, Utc::now())
        .unwrap()
        .started
        .sequence;
    (request, start)
}
pub(super) fn inspection_assistant(
    store: &SqliteConversationStore,
    request: &RequestSnapshot,
) -> (
    MessageBlock,
    RuntimeEventEnvelope,
    crate::events::types::ToolObservationOccurrence,
) {
    let message = MessageBlock::Assistant(AssistantMessageBlock {
        id: request.provisional_message_id.clone(),
        content: vec![AssistantContentBlock::ToolCall(ToolCall {
            // Identical wire call and ToolId across actual retry generations.
            id: ToolCallId::new("reused-call"),
            tool_id: ToolId::new("same-original-tool"),
            name: "same wire name".into(),
            arguments: serde_json::json!({"n":u64::MAX}),
        })],
    });
    let mut event = envelope(
        store.conversation_id(),
        &format!("inspection-assistant-{}", request.identity.retry_number),
        Some(request.identity.attempt_id.clone()),
        RuntimeEvent::AssistantMessageCommitted {
            message_id: request.provisional_message_id.clone(),
        },
    );
    event.turn_id = Some(request.identity.turn.clone());
    let (event, receipt) = store.append_canonical_with_event(&message, event).unwrap();
    (
        message,
        event,
        crate::events::types::ToolObservationOccurrence {
            tool_ordinal: 0,
            ledger_position: receipt.ledger_position,
            block_index: ContentBlockIndex::new(0),
        },
    )
}
// Complete the original physical C batch before a later actual request. C
// result content is independent of ToolExecution terminal Journal settlement.
pub(super) fn inspection_result(store: &SqliteConversationStore, request: &RequestSnapshot) {
    store
        .append_canonical(&MessageBlock::Tool(ToolMessageBlock {
            id: MessageId::new(format!("result:{}", request.provisional_message_id)),
            occurrence: crate::message::types::ToolCallOccurrenceRef::new(
                request.provisional_message_id.clone(),
                ContentBlockIndex::new(0),
            ),
            tool_call_id: ToolCallId::new("reused-call"),
            tool_id: ToolId::new("same-original-tool"),
            result: ToolExecutionResult {
                status: ToolExecutionStatus::Success,
                content: vec![],
                duration_ms: 1,
                exit_code: None,
                artifacts: vec![],
                deliveries: vec![],
                truncation: None,
                workflow: None,
                managed_output: None,
            },
        }))
        .unwrap();
}

pub(super) fn deny_inspection_original_columns(
    context: rusqlite::hooks::AuthContext<'_>,
) -> rusqlite::hooks::Authorization {
    use rusqlite::hooks::{AuthAction, Authorization};
    if matches!(
        context.action,
        AuthAction::Read {
            table_name: "canonical_tool_calls",
            column_name: "commit_sequence" | "assistant_message_id" | "call_id" | "tool_id"
        } | AuthAction::Read {
            table_name: "request_trace_catalogs",
            column_name: "provisional_message_id"
        } | AuthAction::Read {
            table_name: "request_trace_definitions",
            column_name: "tool_id"
        }
    ) {
        return Authorization::Deny;
    }
    deny_trace_original_columns(context)
}
#[test]
fn t08_t12_t15_t19_trace_definition_uses_exact_original_retry_catalog_without_body_reads() {
    use crate::observation::trace::{TraceFieldKind, TraceFieldSource};
    let store = store();
    store.initialize(&[]).unwrap();
    let (first, first_start) = inspection_request(&store, 0, &"original🙂schema".repeat(70_000));
    let before = store.observation_read_cut().unwrap();
    // The later same-step request is already present when the first response
    // commits: recency would choose the wrong catalog, exact native identity cannot.
    let (second, second_start) = inspection_request(&store, 1, "SECOND_ORIGINAL_SCHEMA");
    let (first_message, _, first_occurrence) = inspection_assistant(&store, &first);
    let first_cut = store.observation_read_cut().unwrap();
    let (_, _, second_occurrence) = inspection_assistant(&store, &second);
    let current = store.observation_read_cut().unwrap();
    let expected = serde_json::json!({"tool_id":"same-original-tool","name":"Original 0","description":{"text":first.tool_definitions[1].description,"truncated":false},"input_schema":{"value":first.tool_definitions[1].input_schema,"truncated":false}});
    store
        .lock()
        .unwrap()
        .authorizer(Some(deny_inspection_original_columns))
        .unwrap();
    assert!(matches!(
        store.observation_trace_tool_definition(&before, &first_occurrence),
        Err(ConversationStoreError::InvalidReference(_))
    ));
    let field = store
        .observation_trace_tool_definition(&current, &first_occurrence)
        .unwrap()
        .unwrap();
    let TraceFieldSource::Value {
        field: kind,
        domain,
        item_offset,
        path,
        encoded_bytes,
    } = field
    else {
        panic!("original catalog field")
    };
    assert_eq!(kind, TraceFieldKind::Definition);
    assert_eq!(
        domain,
        format!("trace.records.{first_start}.detail.request.tools")
    );
    assert_eq!(item_offset, 1);
    assert!(path.is_empty());
    assert_eq!(
        store
            .observation_trace_tool_definition(&first_cut, &first_occurrence)
            .unwrap()
            .unwrap(),
        store
            .observation_trace_tool_definition(&current, &first_occurrence)
            .unwrap()
            .unwrap()
    );
    let TraceFieldSource::Value {
        domain: second_domain,
        ..
    } = store
        .observation_trace_tool_definition(&current, &second_occurrence)
        .unwrap()
        .unwrap()
    else {
        panic!("second original catalog")
    };
    assert_eq!(
        second_domain,
        format!("trace.records.{second_start}.detail.request.tools")
    );
    assert_ne!(domain, second_domain);
    let original = inspection_definition_bytes(&store, first_start, current.read.journal);
    assert_eq!(original.len() as u64, encoded_bytes);
    assert_eq!(original, serde_json::to_vec(&expected).unwrap());
    let mut foreign = current.clone();
    foreign.read.conversation_id = ConversationId::new("conv_36524fd8-f674-7fc2-8125-06d01fee0e19");
    assert!(
        store
            .observation_trace_tool_definition(&foreign, &first_occurrence)
            .is_err()
    );
    store
        .lock()
        .unwrap()
        .authorizer(None::<fn(rusqlite::hooks::AuthContext<'_>) -> rusqlite::hooks::Authorization>)
        .unwrap();
    assert_eq!(
        store.load_request_snapshot(&first.request_id).unwrap(),
        first
    );
    assert!(store.load_canonical().unwrap().contains(&first_message));
}
#[test]
fn t10_t12_t19_trace_definition_auxiliary_failure_preserves_native_c_j_and_own_lifecycle() {
    use crate::durable::observation::{TraceFactKind, TraceFactQuery, TraceFactScope};
    let store = store();
    store.initialize(&[]).unwrap();
    let (request, _) = inspection_request(&store, 0, "original schema");
    store.lock().unwrap().execute_batch("CREATE TRIGGER fail_definition BEFORE UPDATE OF observation_definition_ready ON canonical_tool_calls BEGIN SELECT RAISE(FAIL,'optional definition failure'); END;").unwrap();
    let (message, event, occurrence) = inspection_assistant(&store, &request);
    let cut = store.observation_read_cut().unwrap();
    assert!(matches!(
        store.observation_trace_tool_definition(&cut, &occurrence),
        Err(ConversationStoreError::Storage(_))
    ));
    let own = store
        .observation_trace_facts(
            &cut,
            &TraceFactQuery {
                scope: TraceFactScope::All,
                kinds: vec![TraceFactKind::AssistantMessageCommitted],
                before: None,
                after: 0,
                ascending: true,
                limit: 1,
            },
        )
        .unwrap();
    assert_eq!(own[0].sequence, event.sequence);
    assert!(
        store
            .observation_trace_tool_proposal(&cut, &occurrence)
            .is_ok()
    );
    assert_eq!(store.load_canonical().unwrap(), [message]);
    assert_eq!(
        store
            .read_events(Some(event.sequence - 1), 1)
            .unwrap()
            .events[0],
        event
    );
    store
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER fail_definition")
        .unwrap();
    inspection_result(&store, &request);
    let (next, _) = inspection_request(&store, 1, "unaffected original schema");
    let (_, _, next_occurrence) = inspection_assistant(&store, &next);
    assert!(
        store
            .observation_trace_tool_definition(
                &store.observation_read_cut().unwrap(),
                &next_occurrence
            )
            .unwrap()
            .is_some()
    );
    // An existing capture fault is never restamped as a later catalog's success.
    assert!(matches!(
        store.observation_trace_tool_definition(&cut, &occurrence),
        Err(ConversationStoreError::Storage(_))
    ));
}
#[test]
fn t10_t12_t19_trace_definition_missing_and_ambiguous_catalogs_never_become_absent() {
    for ambiguous in [false, true] {
        let store = store();
        store.initialize(&[]).unwrap();
        let (request, start) = inspection_request(&store, 0, "original");
        if ambiguous {
            let (_, other) = inspection_request(&store, 1, "other");
            store.lock().unwrap().execute("UPDATE request_trace_catalogs SET provisional_message_id=?1 WHERE source_sequence=?2",params![request.provisional_message_id.as_str(),other]).unwrap();
        } else {
            store
                .lock()
                .unwrap()
                .execute(
                    "DELETE FROM request_trace_catalogs WHERE source_sequence=?1",
                    [start],
                )
                .unwrap();
        }
        let (message, event, occurrence) = inspection_assistant(&store, &request);
        let cut = store.observation_read_cut().unwrap();
        assert!(matches!(
            store.observation_trace_tool_definition(&cut, &occurrence),
            Err(ConversationStoreError::Storage(_))
        ));
        assert!(
            store
                .observation_trace_tool_proposal(&cut, &occurrence)
                .is_ok()
        );
        assert!(store.load_canonical().unwrap().contains(&message));
        assert_eq!(
            store
                .read_events(Some(event.sequence - 1), 1)
                .unwrap()
                .events[0],
            event
        );
    }
    let store = store();
    store.initialize(&[]).unwrap();
    let _ = inspection_request(&store, 0, "original");
    // A new real request with no matching catalog member is genuine absence.
    let mut request = progress_request_snapshot(
        "inspection-original-owner",
        "1",
        1,
        store.load_head().unwrap().revision,
        Vec::new(),
    );
    freeze_test_input(&store, &mut request, &[]);
    store
        .commit_model_turn_start(&[], &request, Utc::now())
        .unwrap();
    let (_, _, occurrence) = inspection_assistant(&store, &request);
    assert!(
        store
            .observation_trace_tool_definition(&store.observation_read_cut().unwrap(), &occurrence)
            .unwrap()
            .is_none()
    );
}
#[test]
fn t08_t12_t15_t19_trace_adoption_advances_and_validates_only_selected_native_members() {
    let store = store();
    store.initialize(&[]).unwrap();
    let mut watermark = None;
    for index in 0..19 {
        watermark = Some(
            store
                .accept_inbound(draft(&format!(
                    "original adopted {index} {}",
                    if index == 9 {
                        "水".repeat(400_000)
                    } else {
                        String::new()
                    }
                )))
                .unwrap()
                .sequence,
        );
    }
    let before = store.observation_read_cut().unwrap();
    let batch = store.adopt_pending_batch(watermark.unwrap(), None).unwrap();
    let sequence = batch.adoption.as_ref().unwrap().sequence;
    let cut = store.observation_read_cut().unwrap();
    let native = store.load_canonical().unwrap();
    assert_eq!(native.len(), 19);
    store
        .lock()
        .unwrap()
        .authorizer(Some(deny_inspection_original_columns))
        .unwrap();
    assert!(
        store
            .observation_trace_adoption(&before, sequence, 0, 8)
            .is_err()
    );
    SqliteConversationStore::trace_read_work();
    let first = store
        .observation_trace_adoption(&cut, sequence, 0, 8)
        .unwrap();
    assert_eq!(first.total, 19);
    assert_eq!(first.items.len(), 8);
    assert_eq!(first.next_offset, Some(8));
    let (rows, bytes) = SqliteConversationStore::trace_read_work();
    assert_eq!(rows, 9);
    assert!(bytes <= 9 * 16_384);
    let next = store
        .observation_trace_adoption(&cut, sequence, 8, 8)
        .unwrap();
    assert_eq!(next.next_offset, Some(16));
    assert_eq!(
        next.items[0].locator,
        crate::observation::content::CanonicalContentLocator::ByLedgerPosition { position: 9 }
    );
    let last = store
        .observation_trace_adoption(&cut, sequence, 16, 8)
        .unwrap();
    assert_eq!(last.items.len(), 3);
    assert!(last.next_offset.is_none());
    assert!(
        store
            .observation_trace_adoption(&cut, sequence, 0, 33)
            .is_err()
    );
    assert!(
        store
            .observation_trace_adoption(&cut, sequence, u64::MAX, 1)
            .is_err()
    );
    store
        .lock()
        .unwrap()
        .authorizer(None::<fn(rusqlite::hooks::AuthContext<'_>) -> rusqlite::hooks::Authorization>)
        .unwrap();
    store.lock().unwrap().execute("UPDATE trace_adoption_members SET ledger_position=10 WHERE source_sequence=?1 AND ordinal=8",[sequence]).unwrap();
    assert!(matches!(
        store.observation_trace_adoption(&cut, sequence, 8, 1),
        Err(ConversationStoreError::Storage(_))
    ));
    store
        .lock()
        .unwrap()
        .execute(
            "DELETE FROM trace_adoption_members WHERE source_sequence=?1 AND ordinal=8",
            [sequence],
        )
        .unwrap();
    assert!(matches!(
        store.observation_trace_adoption(&cut, sequence, 8, 1),
        Err(ConversationStoreError::Storage(_))
    ));
    // A descriptor does not scan or certify an unrelated unloaded member.
    assert_eq!(
        store
            .observation_trace_adoption(&cut, sequence, 0, 8)
            .unwrap()
            .items,
        first.items
    );
    assert_eq!(store.load_canonical().unwrap(), native);
    assert_eq!(
        store.read_events(Some(sequence - 1), 1).unwrap().events[0],
        batch.adoption.unwrap()
    );
}
#[test]
fn t10_t12_t19_trace_adoption_index_failure_never_refuses_native_adoption() {
    let store = store();
    store.initialize(&[]).unwrap();
    let accepted = store
        .accept_inbound(draft("original native adoption"))
        .unwrap();
    store.lock().unwrap().execute_batch("CREATE TRIGGER fail_adoption_members BEFORE INSERT ON trace_adoption_members BEGIN SELECT RAISE(FAIL,'optional adoption index'); END;").unwrap();
    let batch = store.adopt_pending_batch(accepted.sequence, None).unwrap();
    let event = batch.adoption.as_ref().unwrap();
    let cut = store.observation_read_cut().unwrap();
    assert!(store.load_pending().unwrap().is_empty());
    assert_eq!(
        store.load_canonical().unwrap(),
        [MessageBlock::User(accepted.message)]
    );
    assert_eq!(
        store
            .read_events(Some(event.sequence - 1), 1)
            .unwrap()
            .events[0],
        *event
    );
    assert!(matches!(
        store.observation_trace_adoption(&cut, event.sequence, 0, 8),
        Err(ConversationStoreError::Storage(_))
    ));
    store
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER fail_adoption_members")
        .unwrap();
    let accepted = store
        .accept_inbound(draft("unaffected native adoption"))
        .unwrap();
    let next = store.adopt_pending_batch(accepted.sequence, None).unwrap();
    let page = store
        .observation_trace_adoption(
            &store.observation_read_cut().unwrap(),
            next.adoption.unwrap().sequence,
            0,
            8,
        )
        .unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.items.len(), 1);
}

#[test]
fn t12_t19_trace_definition_rejects_another_real_request_and_later_matching_ordinal() {
    let store = store();
    store.initialize(&[]).unwrap();
    let (first, first_start) = inspection_request(&store, 0, "first original schema");
    let (_, _, occurrence) = inspection_assistant(&store, &first);
    inspection_result(&store, &first);
    let (second, second_start) = inspection_request(&store, 1, "second original schema");
    inspection_assistant(&store, &second);
    let cut = store.observation_read_cut().unwrap();
    let original = store
        .observation_trace_tool_definition(&cut, &occurrence)
        .unwrap()
        .unwrap();
    store
        .lock()
        .unwrap()
        .authorizer(Some(deny_inspection_original_columns))
        .unwrap();
    store.lock().unwrap().execute("UPDATE canonical_tool_calls SET observation_definition_start=?2 WHERE observation_ledger=?1",params![occurrence.ledger_position,second_start]).unwrap();
    assert!(matches!(
        store.observation_trace_tool_definition(&cut, &occurrence),
        Err(ConversationStoreError::Storage(_))
    ));
    store.lock().unwrap().execute("UPDATE canonical_tool_calls SET observation_definition_start=?2,observation_definition_ordinal=0 WHERE observation_ledger=?1",params![occurrence.ledger_position,first_start]).unwrap();
    assert!(matches!(
        store.observation_trace_tool_definition(&cut, &occurrence),
        Err(ConversationStoreError::Storage(_))
    ));
    store.lock().unwrap().execute("UPDATE canonical_tool_calls SET observation_definition_ordinal=NULL WHERE observation_ledger=?1",[occurrence.ledger_position]).unwrap();
    assert!(matches!(
        store.observation_trace_tool_definition(&cut, &occurrence),
        Err(ConversationStoreError::Storage(_))
    ));
    // Even the same native ToolId cannot select a later duplicate definition.
    store.lock().unwrap().execute("UPDATE canonical_tool_calls SET observation_definition_ordinal=2 WHERE observation_ledger=?1",[occurrence.ledger_position]).unwrap();
    assert!(matches!(
        store.observation_trace_tool_definition(&cut, &occurrence),
        Err(ConversationStoreError::Storage(_))
    ));
    store.lock().unwrap().execute("UPDATE canonical_tool_calls SET observation_definition_ordinal=1 WHERE observation_ledger=?1",[occurrence.ledger_position]).unwrap();
    assert_eq!(
        store
            .observation_trace_tool_definition(&cut, &occurrence)
            .unwrap()
            .unwrap(),
        original
    );
}

pub(super) fn inspection_definition_bytes(
    store: &SqliteConversationStore,
    start: u64,
    through: u64,
) -> Vec<u8> {
    let mut original = Vec::new();
    let mut offset = 0;
    loop {
        SqliteConversationStore::content_read_work();
        let chunk = store
            .request_value_read_at(
                start,
                through,
                &["tools".into(), "1".into()],
                offset,
                65_536,
            )
            .unwrap();
        let (rows, bytes) = SqliteConversationStore::content_read_work();
        assert!(rows > 0 && rows <= 2);
        assert!(bytes <= 65_536);
        assert!(chunk.bytes.len() <= 65_536);
        original.extend(chunk.bytes);
        let Some(next) = chunk.next_offset else { break };
        assert!(next > offset);
        offset = next;
    }
    original
}

#[test]
fn t10_t12_t19_missing_request_body_layout_never_hides_own_assistant_tool_facts() {
    use crate::durable::observation::{TraceFactKind, TraceFactQuery, TraceFactScope};
    use base64::Engine as _;
    let store = store();
    store.initialize(&[]).unwrap();
    store.lock().unwrap().execute_batch("CREATE TRIGGER fail_entire_request_layout BEFORE INSERT ON request_observation_fields BEGIN SELECT RAISE(FAIL,'optional entire request layout'); END;").unwrap();
    let (request, start) = inspection_request(&store, 0, "original native schema");
    let (message, committed, occurrence) = inspection_assistant(&store, &request);
    let mut tool = envelope(
        store.conversation_id(),
        "body-layout-fault-tool-start",
        Some(request.identity.attempt_id.clone()),
        RuntimeEvent::ToolExecutionStarted {
            tool_call_id: ToolCallId::new("reused-call"),
            tool_id: ToolId::new("same-original-tool"),
        },
    );
    tool.turn_id = Some(request.identity.turn.clone());
    let tool = store.append_event(tool).unwrap();
    let cut = store.observation_read_cut().unwrap();
    assert!(
        store
            .request_field_header_at(start, cut.read.journal, &[])
            .is_err()
    );
    assert!(matches!(
        store.observation_trace_tool_definition(&cut, &occurrence),
        Err(ConversationStoreError::Storage(_))
    ));
    let own = store
        .observation_trace_facts(
            &cut,
            &TraceFactQuery {
                scope: TraceFactScope::All,
                kinds: vec![
                    TraceFactKind::AssistantMessageCommitted,
                    TraceFactKind::ToolExecutionStarted,
                ],
                before: None,
                after: 0,
                ascending: true,
                limit: 2,
            },
        )
        .unwrap();
    assert_eq!(
        own.iter().map(|header| header.sequence).collect::<Vec<_>>(),
        [committed.sequence, tool.sequence]
    );
    let proposal = store
        .observation_trace_tool_proposal(&cut, &occurrence)
        .unwrap();
    let reference = trace_arguments_reference(&proposal.fields);
    let bytes = store.content_read(&reference, 0, 65_536).unwrap();
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(bytes.data_base64)
            .unwrap(),
        br#"{"n":18446744073709551615}"#
    );
    assert_eq!(store.load_canonical().unwrap(), [message]);
    assert_eq!(
        store.load_request_snapshot(&request.request_id).unwrap(),
        request
    );
    assert_eq!(
        store
            .read_events(Some(committed.sequence - 1), 1)
            .unwrap()
            .events[0],
        committed
    );
    assert_eq!(
        store
            .read_events(Some(tool.sequence - 1), 1)
            .unwrap()
            .events[0],
        tool
    );
    store
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER fail_entire_request_layout")
        .unwrap();
    inspection_result(&store, &request);
    let (next, _) = inspection_request(&store, 1, "healthy original schema");
    let (_, _, next_occurrence) = inspection_assistant(&store, &next);
    assert!(
        store
            .observation_trace_tool_definition(
                &store.observation_read_cut().unwrap(),
                &next_occurrence
            )
            .unwrap()
            .is_some()
    );
}
