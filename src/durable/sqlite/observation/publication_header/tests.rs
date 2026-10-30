use super::super::{PublicationHeader, index_message, index_publication_message};
use crate::durable::{ConversationStore, sqlite::SqliteConversationStore};
use crate::message::content::TextBlock;
use crate::message::types::{AssistantContentBlock, AssistantMessageBlock, MessageBlock};
use crate::runtime::identity::{ConversationId, MessageId, ToolCallId, ToolId};
use crate::runtime_client::{checkpoint::bounded_bytes, preflight::required_budget};
use crate::tools::types::ToolCall;
use rusqlite::params;

fn captured(message: &MessageBlock) -> (SqliteConversationStore, PublicationHeader) {
    let store = SqliteConversationStore::in_memory(ConversationId::new(
        "conv_36524fd8-f674-7fc2-8125-06d01fee0e18",
    ))
    .unwrap();
    store.initialize(&[]).unwrap();
    let draft = {
        let connection = store.lock().unwrap();
        connection
            .execute(
                "INSERT INTO message_ledger(position,message_id,message_json) VALUES(1,?1,?2)",
                params![
                    message.id().as_str(),
                    serde_json::to_string(message).unwrap()
                ],
            )
            .unwrap();
        crate::durable::sqlite::content::index_message(&connection, message).unwrap();
        index_publication_message(&connection, message, 1)
            .unwrap()
            .unwrap()
    };
    (store, draft)
}
fn bytes(store: &SqliteConversationStore) -> Vec<u8> {
    store
        .lock()
        .unwrap()
        .query_row(
            "SELECT header_json FROM observation_headers WHERE ledger_position=1",
            [],
            |row| row.get(0),
        )
        .unwrap()
}
#[test]
fn publication_header_reuse_preserves_transcript_rebinding_admission_boundary() {
    let (store, draft, message) = captured_at_budget(16_383);
    assert_eq!(bounded_bytes(&draft.header, 16_384).unwrap(), bytes(&store));
    let connection = store.lock().unwrap();
    let transaction = connection.unchecked_transaction().unwrap();
    transaction
        .execute(
            "UPDATE rustx_store SET next_transcript_position=9007199254740990 WHERE id=1",
            [],
        )
        .unwrap();
    let cursor = crate::durable::sqlite::append_transcript_reference(
        &transaction,
        "message",
        message.id().as_str(),
    )
    .unwrap();
    // The physical JSON patch fits SQLite's byte CHECK, but adding T crosses
    // the typed admission threshold used by the original final capture.
    let patched = super::super::message(&transaction, message.id())
        .unwrap()
        .unwrap();
    assert_eq!(patched.transcript_cursor, Some(cursor));
    assert!(patched.display.is_some());
    assert!(serde_json::to_vec(&patched).unwrap().len() <= 16_384);
    assert!(required_budget(&patched, 128 * 1024).unwrap() > 16_384);
    assert!(bounded_bytes(&patched, 16_384).is_none());
    draft.rebind(&transaction, &message, 1).unwrap();
    let rebound: Vec<u8> = transaction
        .query_row(
            "SELECT header_json FROM observation_headers WHERE ledger_position=1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let header = super::super::message(&transaction, message.id())
        .unwrap()
        .unwrap();
    assert_eq!(header.transcript_cursor, Some(cursor));
    assert!(header.display.is_none());
    assert_eq!(bounded_bytes(&header, 16_384).unwrap(), rebound);
    index_message(&transaction, &message, Some(1)).unwrap();
    let original: Vec<u8> = transaction
        .query_row(
            "SELECT header_json FROM observation_headers WHERE ledger_position=1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(rebound, original);
    transaction.rollback().unwrap();
}

fn raw_message(name_length: usize) -> MessageBlock {
    let mut content = vec![
        AssistantContentBlock::Text(TextBlock {
            text: "x".repeat(1024)
        });
        8
    ];
    content.push(AssistantContentBlock::ToolCall(ToolCall {
        id: ToolCallId::new("budget-call"),
        tool_id: ToolId::new("budget-tool"),
        name: "n".repeat(name_length),
        arguments: serde_json::json!("x".repeat(12_000)),
    }));
    MessageBlock::Assistant(AssistantMessageBlock {
        id: MessageId::new("budget-raw"),
        content,
    })
}

fn captured_at_budget(budget: usize) -> (SqliteConversationStore, PublicationHeader, MessageBlock) {
    // The original arguments and text blocks always exceed whole-message
    // admission. The per-block Capture shape is fixed; only the retained Tool
    // name varies, so its conservative structural charge changes linearly.
    let (_, initial) = captured(&raw_message(2_000));
    let initial_budget = required_budget(&initial.header, 128 * 1024).unwrap();
    let name_length = 2_000_usize
        .checked_add(budget)
        .unwrap()
        .checked_sub(initial_budget)
        .unwrap();
    let message = raw_message(name_length);
    let (store, draft) = captured(&message);
    assert_eq!(required_budget(&draft.header, 128 * 1024), Some(budget));
    assert!(draft.header.display.is_some());
    (store, draft, message)
}

#[test]
fn publication_header_pretrim_draft_restores_display_after_shorter_original_raw_binding() {
    let (store, draft, message) = captured_at_budget(16_386);
    assert!(bounded_bytes(&draft.header, 16_384).is_none());
    assert!(
        super::super::message(&store.lock().unwrap(), message.id())
            .unwrap()
            .unwrap()
            .display
            .is_none()
    );
    let raw = serde_json::to_vec(&"x".repeat(12_000)).unwrap();
    let connection = store.lock().unwrap();
    connection
        .execute(
            "INSERT INTO content_segments VALUES('canonical','ledger:1',8,'raw_json_utf8',0,?1)",
            [raw.as_slice()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO content_frontiers(source_kind,source_id,block_index,representation,revision,readable_end) VALUES('canonical','ledger:1',8,'raw_json_utf8',1,?1)",
            [raw.len()],
        )
        .unwrap();
    draft.rebind(&connection, &message, 1).unwrap();
    let restored = super::super::message(&connection, message.id())
        .unwrap()
        .unwrap();
    assert!(
        restored.display.is_some(),
        "pre-trim display survives the shorter exact descriptor"
    );
    assert!(required_budget(&restored, 128 * 1024).unwrap() <= 16_384);
    let rebound: Vec<u8> = connection
        .query_row(
            "SELECT header_json FROM observation_headers WHERE ledger_position=1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    index_message(&connection, &message, Some(1)).unwrap();
    let original: Vec<u8> = connection
        .query_row(
            "SELECT header_json FROM observation_headers WHERE ledger_position=1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(rebound, original);
}
