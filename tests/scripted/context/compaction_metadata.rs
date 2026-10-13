//! Issue #140: Pi-style structured compaction summaries with cumulative
//! file-operation metadata — the engine-level extraction contract.
//!
//! These regressions pin the metadata contract deterministically:
//!
//! - the retired canonical span splits into semantic content (the scripted
//!   summarizer's structured Markdown body) and canonical tool facts (the
//!   deterministic extractor's typed [`CompactionSummaryMetadata`]);
//! - extraction reads native Read/Edit/Write tool calls only, deduplicates,
//!   orders deterministically, and lets modification win over read;
//! - cumulative metadata follows the selected compaction lineage, never a
//!   conversation-global accumulator: a previous summary contributes exactly
//!   when it is inside the selected retired span;
//! - generated summary prose is never parsed into metadata;
//! - the committed summary text appends the deterministic metadata-derived
//!   `<read-files>`/`<modified-files>` sections.
//!
//! The tests drive the real `ContextEngine` planning and preparation
//! directly. The committed pipeline transition over a real durable store —
//! including the failure-atomicity matrix — is owned by
//! `compaction_pipeline.rs`.

use rustx::message::content::TextBlock;
use rustx::message::types::{
    AssistantContentBlock, AssistantMessageBlock, CompactionSummaryMetadata, InboundKind,
    MessageBlock, ToolMessageBlock, UserContentBlock, UserMessageBlock, UserSource,
};
use rustx::runtime::identity::{MessageId, ToolCallId, ToolId};
use rustx::tools::types::{ToolCall, ToolExecutionResult, ToolExecutionStatus};

pub(super) fn user(id: &str) -> MessageBlock {
    MessageBlock::User(UserMessageBlock {
        id: MessageId::new(id),
        content: vec![UserContentBlock::Text(TextBlock {
            text: format!("content {id}"),
        })],
        source: UserSource::Human,
        kind: InboundKind::Message,
        timestamp: None,
    })
}

fn file_call(id: &str, tool_id: &str, name: &str, path: &str) -> ToolCall {
    ToolCall {
        id: ToolCallId::new(id),
        tool_id: ToolId::new(tool_id),
        name: name.to_owned(),
        arguments: serde_json::json!({ "path": path }),
    }
}

fn read_call(id: &str, path: &str) -> ToolCall {
    file_call(id, "tool-read", "read", path)
}

fn edit_call(id: &str, path: &str) -> ToolCall {
    file_call(id, "tool-edit", "edit", path)
}

fn write_call(id: &str, path: &str) -> ToolCall {
    file_call(id, "tool-write", "write", path)
}

fn assistant(id: &str, calls: Vec<ToolCall>) -> MessageBlock {
    MessageBlock::Assistant(AssistantMessageBlock {
        id: MessageId::new(id),
        content: calls
            .into_iter()
            .map(AssistantContentBlock::ToolCall)
            .collect(),
    })
}

fn tool_result(id: &str, owner: &MessageId, index: usize, call: &ToolCall) -> MessageBlock {
    MessageBlock::Tool(ToolMessageBlock {
        occurrence: rustx::message::types::ToolCallOccurrenceRef::new(
            owner.clone(),
            rustx::message::types::ContentBlockIndex::new(u32::try_from(index).unwrap()),
        ),
        id: MessageId::new(id),
        tool_call_id: call.id.clone(),
        tool_id: call.tool_id.clone(),
        result: ToolExecutionResult {
            deliveries: Vec::new(),
            status: ToolExecutionStatus::Success,
            content: Vec::new(),
            duration_ms: 1,
            exit_code: Some(0),
            artifacts: Vec::new(),
            truncation: None,
            workflow: None,
            managed_output: None,
        },
    })
}

/// The canonical first-compaction history of the issue contract:
/// Read A, Read B, Edit B, Write C.
pub(super) fn file_operation_history() -> Vec<MessageBlock> {
    let reads = assistant(
        "a1",
        vec![read_call("c1", "/src/a.rs"), read_call("c2", "/src/b.rs")],
    );
    let writes = assistant(
        "a2",
        vec![edit_call("c3", "/src/b.rs"), write_call("c4", "/src/c.rs")],
    );
    let mut messages = vec![user("u1"), reads.clone()];
    if let MessageBlock::Assistant(assistant) = &reads {
        for (index, block) in assistant.content.iter().enumerate() {
            let AssistantContentBlock::ToolCall(call) = block else {
                unreachable!();
            };
            messages.push(tool_result(
                &format!("t-{}", call.id.as_str()),
                &assistant.id,
                index,
                call,
            ));
        }
    }
    messages.push(writes.clone());
    if let MessageBlock::Assistant(assistant) = &writes {
        for (index, block) in assistant.content.iter().enumerate() {
            let AssistantContentBlock::ToolCall(call) = block else {
                unreachable!();
            };
            messages.push(tool_result(
                &format!("t-{}", call.id.as_str()),
                &assistant.id,
                index,
                call,
            ));
        }
    }
    messages.push(user("u2"));
    messages
}

/// The first compaction classifies canonical Read/Edit/Write calls,
/// deduplicates, orders deterministically, and lets modification win over
/// read. The committed text appends the metadata-derived XML sections after
/// the model-produced body.
#[test]
fn first_compaction_extracts_trusted_file_metadata_and_renders_structured_sections() {
    let metadata =
        crate::context::compaction_metadata::retired_span_metadata(&file_operation_history());
    assert_eq!(metadata.read_files(), &["/src/a.rs".to_owned()]);
    assert_eq!(
        metadata.modified_files(),
        &["/src/b.rs".to_owned(), "/src/c.rs".to_owned()]
    );
    let rendered =
        crate::context::compaction_metadata::render_summary_text("## Goal\nShip.", &metadata);
    assert!(rendered.starts_with("## Goal\nShip."));
    assert!(rendered.contains("<read-files>"));
    assert!(rendered.contains("<modified-files>"));
}
#[test]
fn typed_lineage_metadata_accumulates_with_modification_winning_over_read() {
    let first =
        crate::context::compaction_metadata::retired_span_metadata(&file_operation_history());
    let second = CompactionSummaryMetadata::accumulate(
        [&first],
        vec!["/src/c.rs".into(), "/src/d.rs".into()],
        vec!["/src/a.rs".into()],
    );
    assert_eq!(second.read_files(), &["/src/d.rs".to_owned()]);
    assert_eq!(
        second.modified_files(),
        &[
            "/src/a.rs".to_owned(),
            "/src/b.rs".to_owned(),
            "/src/c.rs".to_owned()
        ]
    );
}
#[test]
fn unselected_previous_summary_metadata_is_not_a_global_accumulator() {
    let _previous =
        crate::context::compaction_metadata::retired_span_metadata(&file_operation_history());
    let selected = crate::context::compaction_metadata::retired_span_metadata(&[user("new")]);
    assert!(selected.read_files().is_empty());
    assert!(selected.modified_files().is_empty());
}
#[test]
fn model_prose_cannot_create_file_operation_facts() {
    let body = "## Goal\nRead /invented/a.rs and modified /invented/b.rs";
    let metadata = crate::context::compaction_metadata::retired_span_metadata(&[user("source")]);
    assert_eq!(metadata, CompactionSummaryMetadata::empty());
    assert_eq!(
        crate::context::compaction_metadata::render_summary_text(body, &metadata),
        body
    );
}
