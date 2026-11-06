//! Projection and compaction resource regressions of the SQLite store.

use crate::durable::ConversationStore;
use crate::durable::sqlite::test_support::*;
use crate::durable::sqlite::*;
use crate::durable::{CanonicalStore, JournalStore, ProjectionStore};
use crate::events::types::RuntimeEvent;
use crate::message::types::AssistantMessageBlock;
use crate::message::types::{AssistantContentBlock, MessageBlock};
use crate::model::finish::ModelFinishReason;
use crate::runtime::identity::{AttemptId, MessageId, ToolCallId, ToolId};
use crate::tools::types::{ToolCall, ToolExecutionStatus};

#[test]
fn compaction_resource_paging_never_observes_future_lifecycle_or_ledger_growth() {
    use crate::context::resources::{
        CompactionResources, JournalFrontier, LedgerFrontier, ResourceLimits,
    };
    let store = store();
    let conversation = store.conversation_id().clone();
    let attempt = AttemptId::new("resource-attempt");
    store
        .append_event(envelope(
            &conversation,
            "resource-start",
            Some(attempt.clone()),
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt.clone(),
            },
        ))
        .unwrap();
    store
        .append_canonical(&user_message("before", "immutable before cut"))
        .unwrap();
    let mut resources = CompactionResources::capture(
        &store,
        ResourceLimits {
            page_items: 1,
            ..ResourceLimits::default()
        },
    )
    .unwrap();
    let frozen = resources.cut().clone();
    // The first append commits before capture; the terminal and next message
    // commit after capture, before the delayed page query. SQL's <= cut is
    // the read linearization boundary, not query time or hot lifecycle state.
    store
        .append_event(envelope(
            &conversation,
            "resource-terminal",
            Some(attempt.clone()),
            RuntimeEvent::AttemptCompleted {
                attempt_id: attempt,
                finish_reason: ModelFinishReason::Stop,
            },
        ))
        .unwrap();
    store
        .append_canonical(&user_message("after", "future ledger material"))
        .unwrap();
    let events = resources.events(JournalFrontier(0)).unwrap();
    assert_eq!(events.rows.len(), 1);
    assert!(matches!(
        events.rows[0].1.event,
        RuntimeEvent::AttemptStarted { .. }
    ));
    assert!(resources.events(frozen.journal).unwrap().rows.is_empty());
    let originals = resources.messages(LedgerFrontier(0)).unwrap();
    assert_eq!(originals.rows[0].1.id(), &MessageId::new("before"));
    assert!(resources.messages(frozen.ledger).unwrap().rows.is_empty());
    assert!(resources.originals(&[MessageId::new("after")]).is_err());
}

#[test]
fn compaction_byte_pages_resume_without_decoding_the_next_record() {
    use crate::context::resources::{CompactionResources, LedgerFrontier, ResourceLimits};
    let store = store();
    let rows = (1..=3)
        .map(|i| user_message(&format!("M{i}"), &"x".repeat(128)))
        .collect::<Vec<_>>();
    let bytes = serde_json::to_vec(&rows[0]).unwrap().len();
    store.initialize(&rows).unwrap();
    let mut resources = CompactionResources::capture(
        &store,
        ResourceLimits {
            record_bytes: bytes + 16,
            page_bytes: bytes + 16,
            total_bytes: bytes * 3 + 64,
            ..ResourceLimits::default()
        },
    )
    .unwrap();
    compaction_resources::take_body_decodes();
    let mut after = LedgerFrontier(0);
    for expected in &rows {
        let page = resources.messages(after).unwrap();
        assert_eq!(page.rows.len(), 1);
        assert_eq!(&page.rows[0].1, expected);
        after = LedgerFrontier(page.rows[0].0);
        assert_eq!(
            compaction_resources::take_body_decodes(),
            1,
            "byte lookahead never hydrates the next page's record"
        );
    }
    assert!(resources.messages(after).unwrap().rows.is_empty());
}

#[test]
fn compaction_tool_lifecycle_at_cut_never_reads_later_completion() {
    use crate::context::resources::{CompactionResources, JournalFrontier, ResourceLimits};
    let store = store();
    let call_id = ToolCallId::new("cut-tool-call");
    let tool_id = ToolId::new("cut-tool");
    store
        .append_canonical(&MessageBlock::Assistant(AssistantMessageBlock {
            id: MessageId::new("cut-owner"),
            content: vec![AssistantContentBlock::ToolCall(ToolCall {
                id: call_id.clone(),
                tool_id: tool_id.clone(),
                name: "read".into(),
                arguments: serde_json::json!({"path":"file.rs"}),
            })],
        }))
        .unwrap();
    store
        .append_event(envelope(
            store.conversation_id(),
            "cut-tool-started",
            None,
            RuntimeEvent::ToolExecutionStarted {
                tool_call_id: call_id.clone(),
                tool_id: tool_id.clone(),
            },
        ))
        .unwrap();
    let mut resources = CompactionResources::capture(
        &store,
        ResourceLimits {
            page_items: 1,
            ..ResourceLimits::default()
        },
    )
    .unwrap();
    let frozen = resources.cut().clone();
    store
        .append_event(envelope(
            store.conversation_id(),
            "cut-tool-completed",
            None,
            RuntimeEvent::ToolExecutionCompleted {
                tool_call_id: call_id,
                tool_id,
                result: crate::tools::types::ToolExecutionResult {
                    workflow: None,
                    status: ToolExecutionStatus::Success,
                    content: Vec::new(),
                    duration_ms: 0,
                    exit_code: None,
                    artifacts: Vec::new(),
                    deliveries: Vec::new(),
                    truncation: None,
                    managed_output: None,
                },
            },
        ))
        .unwrap();
    let page = resources.events(JournalFrontier(0)).unwrap();
    assert!(matches!(
        page.rows[0].1.event,
        RuntimeEvent::ToolExecutionStarted { .. }
    ));
    assert!(resources.events(frozen.journal).unwrap().rows.is_empty());
    assert!(matches!(
        store.read_events(Some(frozen.journal.0), 1).unwrap().events[0].event,
        RuntimeEvent::ToolExecutionCompleted { .. }
    ));
}

#[test]
fn corrupted_checkpoint_content_manifest_fails_closed() {
    use crate::scripted_suites::support::context::{checkpoint_commit_fixture, checkpoint_fixture};
    let store = store();
    store.initialize(&[user_message("a", "A")]).unwrap();
    store
        .commit_projection(checkpoint_commit_fixture(
            &store,
            checkpoint_fixture(&store, "P1", 1, "summary"),
            None,
        ))
        .unwrap();
    store
        .lock()
        .unwrap()
        .execute("UPDATE projection_checkpoints SET candidate_json='{}'", [])
        .unwrap();
    assert!(store.projection_head().is_err());
    assert!(store.projected_history().is_err());
}
