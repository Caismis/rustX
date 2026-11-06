//! Canonical ledger, surface, and transcript regressions of the SQLite store.

use crate::conversation::ConversationState;
use crate::conversation::SurfaceOp;
use crate::durable::sqlite::canonical::transcript::*;
use crate::durable::sqlite::test_support::*;
use crate::durable::sqlite::*;
use crate::durable::{CanonicalStore, InboxStore, JournalStore, ProjectionStore};
use crate::durable::{ConversationStoreError, InboundDraft};
use crate::message::content::TextBlock;
use crate::message::types::{
    AssistantContentBlock, InboundKind, MessageBlock, UserMessageBlock, UserSource,
};
use crate::message::types::{AssistantMessageBlock, ToolMessageBlock};
use crate::runtime::identity::{ConversationId, MessageId, ToolCallId, ToolId};
use crate::tools::types::{ToolCall, ToolExecutionResult, ToolExecutionStatus};
use rusqlite::Connection;
use std::sync::Arc;

mod append_head;
mod initialization_state;

#[test]
fn snapshot_transcript_excludes_later_tool_results() {
    use crate::message::types::{ContentBlockIndex, ToolCallOccurrenceRef};
    let store = store();
    let call = ToolCall {
        id: ToolCallId::new("call-1"),
        tool_id: ToolId::new("tool-bash"),
        name: "bash".into(),
        arguments: serde_json::json!({}),
    };
    let owner = MessageBlock::Assistant(AssistantMessageBlock {
        id: MessageId::new("a"),
        content: vec![
            AssistantContentBlock::Text(TextBlock {
                text: "thinking".into(),
            }),
            AssistantContentBlock::ToolCall(call.clone()),
        ],
    });
    store.initialize(&[owner]).unwrap();
    let valid = ToolMessageBlock {
        id: MessageId::new("result"),
        occurrence: ToolCallOccurrenceRef::new(MessageId::new("a"), ContentBlockIndex::new(1)),
        tool_call_id: call.id,
        tool_id: call.tool_id,
        result: ToolExecutionResult {
            deliveries: Vec::new(),
            status: ToolExecutionStatus::Success,
            content: vec![],
            duration_ms: 0,
            exit_code: None,
            artifacts: vec![],
            truncation: None,
            workflow: None,
            managed_output: None,
        },
    };
    let cut = crate::durable::TranscriptSnapshotCut {
        bootstrap_through: store.load_transcript_page(None, 1).unwrap().entries[0]
            .cursor
            .get(),
        journal_through: 0,
        messages: vec![MessageId::new("a")],
        publications: Vec::new(),
        pending: Vec::new(),
    };
    store
        .append_canonical(&MessageBlock::Tool(valid.clone()))
        .unwrap();
    let historical = store.load_transcript_snapshot(&cut).unwrap();
    assert_eq!(historical.entries.len(), 1);
    assert!(
        historical.entries[0].tool_calls[0].result.is_none(),
        "later Tool association cannot enter captured membership"
    );
}

#[test]
fn canonical_tool_results_validate_exact_ownership_and_roll_back() {
    use crate::message::types::{ContentBlockIndex, ToolCallOccurrenceRef};
    let store = store();
    let call = ToolCall {
        id: ToolCallId::new("call-1"),
        tool_id: ToolId::new("tool-bash"),
        name: "bash".into(),
        arguments: serde_json::json!({}),
    };
    let owner = MessageBlock::Assistant(AssistantMessageBlock {
        id: MessageId::new("a"),
        content: vec![
            AssistantContentBlock::Text(TextBlock {
                text: "thinking".into(),
            }),
            AssistantContentBlock::ToolCall(call.clone()),
        ],
    });
    store.initialize(&[owner]).unwrap();
    let valid = ToolMessageBlock {
        id: MessageId::new("result"),
        occurrence: ToolCallOccurrenceRef::new(MessageId::new("a"), ContentBlockIndex::new(1)),
        tool_call_id: call.id,
        tool_id: call.tool_id,
        result: ToolExecutionResult {
            deliveries: Vec::new(),
            status: ToolExecutionStatus::Success,
            content: vec![],
            duration_ms: 0,
            exit_code: None,
            artifacts: vec![],
            truncation: None,
            workflow: None,
            managed_output: None,
        },
    };
    let mut missing = valid.clone();
    missing.occurrence.assistant_message_id = MessageId::new("missing");
    let mut text = valid.clone();
    text.occurrence.block_index = ContentBlockIndex::new(0);
    let mut outside = valid.clone();
    outside.occurrence.block_index = ContentBlockIndex::new(2);
    let mut wrong_call = valid.clone();
    wrong_call.tool_call_id = ToolCallId::new("wrong");
    let mut wrong_tool = valid.clone();
    wrong_tool.tool_id = ToolId::new("wrong");
    for invalid in [missing, text, outside, wrong_call, wrong_tool] {
        assert!(
            store
                .append_canonical(&MessageBlock::Tool(invalid))
                .is_err()
        );
        assert_eq!(
            store.load_canonical().unwrap().len(),
            1,
            "failed result leaves no ledger row"
        );
        assert!(
            store.load_transcript_page(None, 1).unwrap().entries[0].tool_calls[0]
                .result
                .is_none()
        );
    }
    store
        .append_canonical(&MessageBlock::Tool(valid.clone()))
        .unwrap();
    let mut duplicate = valid.clone();
    duplicate.id = MessageId::new("duplicate");
    assert!(
        store
            .append_canonical(&MessageBlock::Tool(duplicate))
            .is_err()
    );
    // Reusing one result MessageId for a second exact occurrence is also invalid.
    let second = MessageBlock::Assistant(AssistantMessageBlock {
        id: MessageId::new("b"),
        content: vec![AssistantContentBlock::ToolCall(ToolCall {
            id: valid.tool_call_id.clone(),
            tool_id: valid.tool_id.clone(),
            name: "bash".into(),
            arguments: serde_json::json!({}),
        })],
    });
    store.append_canonical(&second).unwrap();
    let mut reused = valid;
    reused.occurrence = ToolCallOccurrenceRef::new(MessageId::new("b"), ContentBlockIndex::new(0));
    assert!(store.append_canonical(&MessageBlock::Tool(reused)).is_err());
    assert_eq!(store.load_canonical().unwrap().len(), 3);
    assert!(
        store.load_transcript_page(None, 1).unwrap().entries[0].tool_calls[0]
            .result
            .is_none()
    );
}

#[test]
fn transcript_tool_reads_seek_occurrences_and_result_message_indexes() {
    let store = store();
    let connection = store.conn.lock().unwrap();
    let plan = connection
        .prepare(&format!("EXPLAIN QUERY PLAN {TRANSCRIPT_TOOLS_SQL}"))
        .unwrap()
        .query_map(["assistant"], |row| row.get::<_, String>(3))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
        .join("\n");
    assert!(
        plan.contains("SEARCH c USING INDEX sqlite_autoindex_canonical_tool_calls_2"),
        "{plan}"
    );
    assert!(
        plan.contains("SEARCH l USING INDEX sqlite_autoindex_message_ledger_1"),
        "{plan}"
    );
    assert!(
        !plan.contains("SCAN ") && !plan.contains("TEMP B-TREE"),
        "{plan}"
    );
}

#[test]
fn transcript_tool_obsolete_schemas_are_refused_without_backfill() {
    for obsolete in [38, 39] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("old.sqlite");
        let id = ConversationId::new("conv_a2b8feb8-c105-7980-8a72-d8482750c3fc");
        {
            let store = SqliteConversationStore::open(id.clone(), &path).unwrap();
            store.conn.lock().unwrap().execute_batch(&format!("DROP TABLE canonical_tool_calls; UPDATE rustx_store SET schema_version={obsolete} WHERE id=1")).unwrap();
        }
        assert!(matches!(
            SqliteConversationStore::open(id, &path),
            Err(ConversationStoreError::SchemaVersionMismatch {
                stored,
                expected: SQLITE_SCHEMA_VERSION
            }) if stored == obsolete
        ));
        let connection = Connection::open(path).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE name='canonical_tool_calls'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
    }
}

#[test]
#[allow(clippy::too_many_lines)] // One retired-history seed and canonical occurrence contract.
fn transcript_tool_lineage_seed_keeps_retired_reused_call_occurrences() {
    let call = ToolCall {
        id: ToolCallId::new("call-1"),
        tool_id: ToolId::new("tool-bash"),
        name: "bash".into(),
        arguments: serde_json::json!({}),
    };
    let assistant = |id| {
        MessageBlock::Assistant(AssistantMessageBlock {
            id: MessageId::new(id),
            content: vec![AssistantContentBlock::ToolCall(call.clone())],
        })
    };
    let result = |id, owner: &str, text: &str| {
        MessageBlock::Tool(ToolMessageBlock {
            occurrence: crate::message::types::ToolCallOccurrenceRef::new(
                crate::runtime::identity::MessageId::new(owner),
                crate::message::types::ContentBlockIndex::new(0),
            ),
            id: MessageId::new(id),
            tool_call_id: call.id.clone(),
            tool_id: call.tool_id.clone(),
            result: ToolExecutionResult {
                deliveries: Vec::new(),
                status: ToolExecutionStatus::Success,
                content: vec![crate::tools::types::ToolResultContent::Text(
                    crate::message::content::TextBlock { text: text.into() },
                )],
                duration_ms: 1,
                exit_code: None,
                artifacts: Vec::new(),
                truncation: None,
                workflow: None,
                managed_output: None,
            },
        })
    };
    let canonical = vec![assistant("a"), result("ra", "a", "old-result")];
    let store = store();
    store.initialize(&canonical).unwrap();
    store
        .commit_projection(
            crate::scripted_suites::support::context::checkpoint_commit_fixture(
                &store,
                crate::scripted_suites::support::context::checkpoint_fixture(
                    &store,
                    "P1",
                    2,
                    "retired A",
                ),
                None,
            ),
        )
        .unwrap();
    store.append_canonical(&assistant("b")).unwrap();
    let unresolved = crate::observation::transcript::transcript_page_view(
        store.load_transcript_page(None, 1).unwrap(),
    )
    .unwrap();
    let current = &unresolved.entries[0].tool_calls[0];
    assert_eq!(current.message_id, Some(MessageId::new("b")));
    assert!(matches!(
        current.state,
        crate::observation::tool::ForegroundToolState::Assembled { .. }
    ));
    assert!(
        !serde_json::to_string(current)
            .unwrap()
            .contains("old-result")
    );
    store
        .append_canonical(&result("rb", "b", "new-result"))
        .unwrap();
    let page = store.load_transcript_page(None, 10).unwrap();
    let tools: Vec<_> = page
        .entries
        .iter()
        .flat_map(|entry| &entry.tool_calls)
        .collect();
    assert_eq!(tools.len(), 2);
    for (tool, owner, text) in [(tools[0], "a", "old-result"), (tools[1], "b", "new-result")] {
        assert_eq!(tool.message_id, MessageId::new(owner));
        assert_eq!(
            tool.result.as_ref().unwrap().content,
            vec![crate::tools::types::ToolResultContent::Text(
                crate::message::content::TextBlock { text: text.into() }
            )]
        );
    }
}

#[test]
fn agent_transcript_tools_resolve_native_results_across_page_boundaries() {
    let store = store();
    store.initialize(&[user_message("u", "Run")]).unwrap();
    let calls = ["first", "second"].map(|id| ToolCall {
        id: ToolCallId::new(id),
        tool_id: ToolId::new("tool-bash"),
        name: "bash".into(),
        arguments: serde_json::json!({"command": id}),
    });
    store
        .append_canonical(&MessageBlock::Assistant(AssistantMessageBlock {
            id: MessageId::new("assistant"),
            content: calls
                .iter()
                .cloned()
                .map(AssistantContentBlock::ToolCall)
                .collect(),
        }))
        .unwrap();
    let before = store.load_transcript_page(None, 1).unwrap();
    assert_eq!(before.entries[0].tool_calls.len(), 2);
    assert!(
        before.entries[0]
            .tool_calls
            .iter()
            .all(|tool| tool.result.is_none())
    );
    let unresolved = crate::observation::transcript::transcript_page_view(before.clone()).unwrap();
    for (index, tool) in unresolved.entries[0].tool_calls.iter().enumerate() {
        assert_eq!(tool.message_id, Some(MessageId::new("assistant")));
        assert_eq!(tool.block_index.get() as usize, index);
        assert!(matches!(
            tool.state,
            crate::observation::tool::ForegroundToolState::Assembled { .. }
        ));
    }
    // Physical/result publication order differs from canonical call order.
    for (index, call) in calls.iter().enumerate().rev() {
        store
            .append_canonical(&MessageBlock::Tool(ToolMessageBlock {
                occurrence: crate::message::types::ToolCallOccurrenceRef::new(
                    crate::runtime::identity::MessageId::new("assistant"),
                    crate::message::types::ContentBlockIndex::new(u32::try_from(index).unwrap()),
                ),
                id: MessageId::new(format!("result-{index}")),
                tool_call_id: call.id.clone(),
                tool_id: call.tool_id.clone(),
                result: ToolExecutionResult {
                    deliveries: Vec::new(),
                    status: if index == 0 {
                        ToolExecutionStatus::Success
                    } else {
                        ToolExecutionStatus::Failed {
                            error: "native failure".into(),
                        }
                    },
                    content: Vec::new(),
                    duration_ms: 1,
                    exit_code: None,
                    artifacts: Vec::new(),
                    truncation: None,
                    workflow: None,
                    managed_output: None,
                },
            }))
            .unwrap();
    }
    let tail = store.load_transcript_page(None, 2).unwrap();
    let older = store.load_transcript_page(tail.next_cursor, 1).unwrap();
    assert_eq!(older.entries[0].cursor, before.entries[0].cursor);
    let tools = &older.entries[0].tool_calls;
    assert_eq!(
        tools
            .iter()
            .map(|tool| tool.call.id.as_str())
            .collect::<Vec<_>>(),
        vec!["first", "second"]
    );
    assert!(matches!(
        tools[0].result.as_ref().unwrap().status,
        ToolExecutionStatus::Success
    ));
    assert!(matches!(
        tools[1].result.as_ref().unwrap().status,
        ToolExecutionStatus::Failed { .. }
    ));
    let original_results: Vec<_> = tools.iter().map(|tool| tool.result.clone()).collect();
    let projected = crate::observation::transcript::transcript_page_view(older).unwrap();
    // Canonical result acceptance is positive C evidence. No execution J
    // fact occurred in this fixture, so it cannot settle the lifecycle.
    for (tool, expected) in projected.entries[0].tool_calls.iter().zip(original_results) {
        assert!(matches!(
            tool.state,
            crate::observation::tool::ForegroundToolState::Assembled { .. }
        ));
        assert_eq!(tool.committed_result, expected);
        assert!(tool.result_source.is_none());
    }
}

#[test]
fn historical_surface_survives_later_append() {
    let store = store();
    let a = MessageBlock::User(UserMessageBlock {
        id: MessageId::new("a"),
        content: draft("a").content,
        source: UserSource::Human,
        kind: InboundKind::Message,
        timestamp: None,
    });
    let b = MessageBlock::User(UserMessageBlock {
        id: MessageId::new("b"),
        content: draft("b").content,
        source: UserSource::Human,
        kind: InboundKind::Message,
        timestamp: None,
    });
    store.initialize(std::slice::from_ref(&a)).unwrap();
    let first = store.load_head().unwrap().revision;
    store.append_canonical(&b).unwrap();
    assert_eq!(
        store.reconstruct_surface(first).unwrap(),
        vec![MessageId::new("a")]
    );
}

#[test]
fn canonical_summary_payload_is_not_a_canonical_message() {
    let store = store();
    store.initialize(&[user_message("a", "A")]).unwrap();
    let input = serde_json::json!({"role":"user","id":"summary","source":"runtime","kind":{"compaction_summary":{}},"content":[]});
    assert!(serde_json::from_value::<MessageBlock>(input).is_err());
    assert_eq!(store.load_canonical().unwrap(), [user_message("a", "A")]);
    assert!(store.read_events(None, 64).unwrap().events.is_empty());
}

#[test]
fn pending_message_identity_cannot_be_reused_by_a_canonical_append() {
    let store = store();
    let accepted = store
        .accept_inbound(InboundDraft {
            message_id: Some(MessageId::new("shared-message-id")),
            ..draft("pending")
        })
        .unwrap();
    let conflicting = user_message("shared-message-id", "canonical");
    assert!(matches!(
        store.append_canonical(&conflicting),
        Err(ConversationStoreError::DuplicateMessageId(id))
            if id == MessageId::new("shared-message-id")
    ));
    assert_eq!(store.load_pending().unwrap().len(), 1);
    assert_eq!(store.load_pending().unwrap()[0].sequence, accepted.sequence);
    assert!(store.load_canonical().unwrap().is_empty());
}

#[test]
fn corrupt_surface_operation_reference_fails_closed() {
    let store = store();
    store.initialize(&[user_message("a", "A")]).unwrap();
    let missing = MessageId::new("missing");
    let operation = serde_json::to_string(&SurfaceOp::Append {
        message_id: missing.clone(),
    })
    .unwrap();
    let connection = store.conn.lock().unwrap();
    connection
        .execute(
            "INSERT INTO surface_ops(revision,compaction_generation,op_json) VALUES(2,0,?1)",
            [operation],
        )
        .unwrap();
    connection
        .execute("UPDATE surface_head SET revision=2 WHERE id=1", [])
        .unwrap();
    drop(connection);
    assert!(matches!(
        store.load_head(),
        Err(ConversationStoreError::InvalidReference(detail))
            if detail.contains("canonical frontier metadata")
    ));
}

#[test]
fn durable_hot_bootstrap_is_bounded_to_current_surface() {
    let store = store();
    let messages: Vec<_> = (0..100)
        .map(|index| user_message(&format!("m{index}"), "history"))
        .collect();
    store.initialize(&messages).unwrap();
    let revision = store.load_head().unwrap().revision;
    let receipt = store
        .commit_projection(
            crate::scripted_suites::support::context::checkpoint_commit_fixture(
                &store,
                crate::scripted_suites::support::context::checkpoint_fixture(
                    &store,
                    "bootstrap-checkpoint",
                    99,
                    "bounded",
                ),
                None,
            ),
        )
        .unwrap();
    let history = store.projected_history().unwrap();
    let active = history
        .messages
        .into_iter()
        .filter_map(|m| m.as_canonical().cloned())
        .collect::<Vec<_>>();
    let active_ids = active.iter().map(|m| m.id().clone()).collect();
    let state = ConversationState::from_durable_head(
        active,
        active_ids,
        revision,
        receipt.checkpoint.activation.activation.0,
    )
    .unwrap();
    assert_eq!(state.ledger().len(), 1);
    assert_eq!(state.surface().len(), 1);
    assert_eq!(state.surface_access().history_enumerations(), 0);
    assert_eq!(state.ledger().access().enumerations(), 0);
    assert_eq!(
        store.load_canonical_page(None, 3).unwrap().messages.len(),
        3
    );
}

#[test]
fn t14_canonical_after_boundary_uses_original_ledger_and_selected_revision() {
    use crate::observation::content::CanonicalContentLocator;
    let store = store();
    store.initialize(&[]).unwrap();
    let opening = store
        .append_canonical(&user_message("opening", "ordinary boundary"))
        .unwrap();
    let original_id = "assistant-".repeat(128);
    let closing = store
        .append_canonical(&assistant_message(&original_id, "canonical closing"))
        .unwrap();
    let locator = CanonicalContentLocator::ByLedgerPosition {
        position: closing.ledger_position,
    };
    let revision = closing.observation.read_cut.surface_revision;
    {
        let connection = store.lock().unwrap();
        connection
            .execute_batch("DELETE FROM observation_headers")
            .unwrap();
        connection
            .execute(
                "UPDATE message_ledger SET message_json='unreadable original body' WHERE position=?1",
                [closing.ledger_position],
            )
            .unwrap();
    }
    assert_eq!(
        store
            .canonical_boundary_message_id(revision, &locator)
            .unwrap(),
        MessageId::new(original_id),
        "After resolves the original identity without a body or display header"
    );
    assert!(matches!(
        store.canonical_boundary_message_id(
            opening.observation.read_cut.surface_revision,
            &locator,
        ),
        Err(ConversationStoreError::InvalidReference(_))
    ));
    assert!(matches!(
        store.surface_boundary_message_id(revision, &locator),
        Err(ConversationStoreError::InvalidReference(_))
    ));
    assert_eq!(
        store
            .surface_boundary_message_id(
                revision,
                &CanonicalContentLocator::ByLedgerPosition {
                    position: opening.ledger_position,
                },
            )
            .unwrap(),
        MessageId::new("opening"),
        "Before retains its ordinary inbound boundary semantics"
    );
}

#[test]
fn t14_t20_atomic_canonical_batch_owns_one_finite_shared_observation_proof() {
    let store = SqliteConversationStore::in_memory(ConversationId::new(
        "conv_2be184c1-ab11-725b-82c3-81daea936180",
    ))
    .unwrap();
    store.initialize(&[]).unwrap();
    let messages = (0..73)
        .map(|index| user_message(&format!("batch-{index}"), &format!("body-{index}")))
        .collect::<Vec<_>>();
    let receipts = store.append_canonical_batch(&messages).unwrap();
    assert_eq!(receipts.len(), 73);
    let batch = &receipts[0].observation;
    assert_eq!(
        (batch.first_position, batch.last_position, batch.count),
        (1, 73, 73)
    );
    assert_eq!(batch.read_cut.ledger_through, 73);
    assert_eq!(batch.transcript_count, 73);
    assert_eq!(batch.head.len(), 8);
    assert_eq!(batch.head.first().unwrap().ledger_position, Some(66));
    assert_eq!(batch.head.last().unwrap().ledger_position, Some(73));
    assert!(batch.has_opening_user);
    assert_eq!(batch.opening_preview.as_deref(), Some("body-0"));
    assert_eq!(batch.journal_range, None);
    for (ordinal, receipt) in receipts.iter().enumerate() {
        assert!(Arc::ptr_eq(batch, &receipt.observation));
        assert_eq!(receipt.ordinal, ordinal as u64);
        assert_eq!(receipt.ledger_position, ordinal as u64 + 1);
    }
    let follow = store
        .append_canonical(&user_message("follow", "after-batch"))
        .unwrap();
    assert_eq!(follow.observation.count, 1);
    assert_eq!(follow.ordinal, 0);
    assert_eq!(follow.ledger_position, 74);
    assert_eq!(
        batch.read_cut.ledger_through, 73,
        "immutable batch does not jump to the new source head"
    );
}
