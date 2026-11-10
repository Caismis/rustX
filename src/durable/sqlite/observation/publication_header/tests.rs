use super::super::PublicationHeader;
#[cfg(test)]
use crate::durable::CanonicalStore;
use crate::durable::sqlite::SqliteConversationStore;
use crate::durable::sqlite::observation::index::{index_message, index_message_capture};
use crate::message::content::TextBlock;
use crate::message::types::{AssistantContentBlock, AssistantMessageBlock, MessageBlock};
use crate::observation::{bounded::bounded_bytes, preflight::required_budget};
use crate::runtime::identity::{ConversationId, MessageId, ToolCallId, ToolId};
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
        index_message_capture(&connection, message, Some(1), true, None)
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
            "SELECT header_json FROM transcript_entries WHERE ledger_position=1",
            [],
            |row| row.get(0),
        )
        .unwrap()
}
#[test]
fn publication_header_recapture_admits_the_permanent_transcript_cursor_before_reuse() {
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
    let cursor = crate::durable::sqlite::canonical::ledger::append_transcript_reference(
        &transaction,
        "message",
        message.id().as_str(),
    )
    .unwrap();
    // A full re-capture after T exists must still apply typed admission to its
    // current cursor, as the final publication repair does.
    transaction
        .execute(
            "DELETE FROM transcript_entries WHERE position<0 AND ledger_position=1",
            [],
        )
        .unwrap();
    let prepared = index_message_capture(&transaction, &message, Some(1), true, Some(cursor))
        .unwrap()
        .unwrap();
    let admitted = super::super::message(&transaction, message.id())
        .unwrap()
        .unwrap();
    assert_eq!(admitted.transcript_cursor, Some(cursor));
    assert!(admitted.display.is_none());
    assert!(bounded_bytes(&admitted, 16_384).is_some());
    prepared.rebind(&transaction, &message, 1).unwrap();
    let rebound: Vec<u8> = transaction
        .query_row(
            "SELECT header_json FROM transcript_entries WHERE ledger_position=1",
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
            "SELECT header_json FROM transcript_entries WHERE ledger_position=1",
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
fn canonical_append_preserves_the_original_header_admission_before_cursor_patch() {
    let (oracle, _, message) = captured_at_budget(16_383);
    let expected: Vec<u8> = oracle
        .lock()
        .unwrap()
        .query_row(
            "SELECT CAST(json_set(header_json,'$.transcript_cursor',1) AS BLOB) FROM transcript_entries WHERE ledger_position=1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let expected_header: crate::durable::observation::MessageHeader =
        serde_json::from_slice(&expected).unwrap();
    assert!(expected_header.display.is_some());
    assert!(expected.len() <= 16_384);
    let store = crate::durable::sqlite::test_support::store();
    store.initialize(&[]).unwrap();
    store.append_canonical(&message).unwrap();
    assert_eq!(bytes(&store), expected);
}

#[test]
fn publication_block_rebinding_rejects_a_foreign_native_position() {
    let message = raw_message(1);
    let (store, mut draft) = captured(&message);
    let mut foreign = message.clone();
    let MessageBlock::Assistant(assistant) = &mut foreign else {
        unreachable!();
    };
    assistant.id = MessageId::new("foreign-publication-owner");
    let connection = store.lock().unwrap();
    connection.execute_batch("PRAGMA foreign_keys=OFF").unwrap();
    let transaction = connection.unchecked_transaction().unwrap();
    transaction
        .execute(
            "INSERT INTO message_ledger(position,message_id,message_json) VALUES(2,?1,?2)",
            params![
                foreign.id().as_str(),
                serde_json::to_string(&foreign).unwrap()
            ],
        )
        .unwrap();
    crate::durable::sqlite::content::index_message(&transaction, &foreign).unwrap();
    index_message_capture(&transaction, &foreign, Some(2), true, None).unwrap();
    transaction.execute("UPDATE observation_block_headers SET message_id=?1 WHERE ledger_position=2 AND block_index=8", [message.id().as_str()]).unwrap();
    let before: Vec<u8> = transaction.query_row(
        "SELECT header_json FROM observation_block_headers WHERE ledger_position=2 AND block_index=8",
        [], |row| row.get(0),
    ).unwrap();
    // A relabelled draft must not redirect a Tool rebind to another message.
    // The block capture checks its actual native MessageId-to-position binding.
    draft.header.ledger_position = Some(2);
    assert!(draft.rebind(&transaction, &message, 2).is_err());
    let after: Vec<u8> = transaction.query_row(
        "SELECT header_json FROM observation_block_headers WHERE ledger_position=2 AND block_index=8",
        [], |row| row.get(0),
    ).unwrap();
    assert_eq!(after, before);
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
            "INSERT INTO content_segments VALUES('canonical',5,8,'r',0,?1)",
            [raw.as_slice()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO content_frontiers(source_kind,source_id,block_index,representation,revision,readable_end) VALUES('canonical',5,8,'r',1,?1)",
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
            "SELECT header_json FROM transcript_entries WHERE ledger_position=1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    index_message(&connection, &message, Some(1)).unwrap();
    let original: Vec<u8> = connection
        .query_row(
            "SELECT header_json FROM transcript_entries WHERE ledger_position=1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(rebound, original);
}
