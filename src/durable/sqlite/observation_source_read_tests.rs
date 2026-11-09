use super::*;
#[cfg(test)]
use crate::durable::CanonicalStore;
use crate::durable::SqliteConversationStore;
use crate::durable::observation::MessageBlockHeader;
use crate::durable::sqlite::observation::index::index_blocks;
use crate::message::content::TextBlock;
use crate::message::types::{UserContentBlock, UserMessageBlock, UserSource};
use crate::runtime::identity::MessageId;

fn fixture_with_id(id: &str) -> (SqliteConversationStore, MessageBlock) {
    let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    let message = MessageBlock::User(UserMessageBlock {
        id: MessageId::new(id),
        content: ["first", "second"]
            .into_iter()
            .map(|text| UserContentBlock::Text(TextBlock { text: text.into() }))
            .collect(),
        source: UserSource::Human,
        kind: InboundKind::Message,
        timestamp: None,
    });
    store.initialize(std::slice::from_ref(&message)).unwrap();
    (store, message)
}

fn fixture() -> (SqliteConversationStore, MessageBlock) {
    fixture_with_id("block-source-owner")
}

#[test]
fn canonical_block_descriptors_share_one_ledger_lookup_per_block() {
    let (store, message) = fixture();
    let mut connection = store.lock().unwrap();
    let transaction = connection.transaction().unwrap();
    crate::durable::sqlite::content::take_source_work();
    for value in [Some(serde_json::json!({"text":"first"})), None] {
        let bytes = block_bytes(&transaction, &message, 0, "text", value, &["text"]).unwrap();
        let header: MessageBlockHeader = serde_json::from_slice(&bytes).unwrap();
        assert!(!header.content.is_empty());
        assert_eq!(
            crate::durable::sqlite::content::take_source_work(),
            (1, 0),
            "four representations and optional metadata share this block's lookup"
        );
    }
}

#[test]
fn canonical_block_source_is_reread_after_an_intervening_insert_trigger() {
    let (store, message) = fixture();
    let mut connection = store.lock().unwrap();
    // Corrupting the authoritative ledger after block 0 simulates a trigger
    // invalidating a previously resolved source; the next block must re-read.
    connection.execute_batch("PRAGMA foreign_keys=OFF").unwrap();
    {
        let transaction = connection.transaction().unwrap();
        transaction
            .execute(
                "DELETE FROM observation_block_headers WHERE ledger_position=(SELECT position FROM message_ledger WHERE message_id=?1)",
                [message.id().as_str()],
            )
            .unwrap();
        transaction.execute_batch("CREATE TEMP TRIGGER erase_block_source AFTER INSERT ON observation_block_headers WHEN NEW.block_index=0 BEGIN DELETE FROM message_ledger WHERE position=NEW.ledger_position; END").unwrap();
        crate::durable::sqlite::content::take_source_work();
        assert!(index_blocks(&transaction, &message).is_err());
        assert_eq!(
            crate::durable::sqlite::content::take_source_work(),
            (2, 0),
            "the second block cannot reuse position evidence from before the first INSERT"
        );
        // Drop rolls back both the successful first block and the trigger write.
    }
    assert!(connection.is_autocommit());
    let transaction = connection.transaction().unwrap();
    assert!(block_bytes(&transaction, &message, 1, "text", None, &["text"]).is_ok());
}

#[test]
fn canonical_coordinate_pages_authenticate_identity_without_decoding_message_bodies() {
    use crate::durable::ObservationStore;
    for id in ["short".to_owned(), "long-水".repeat(10_000)] {
        let (store, message) = fixture_with_id(&id);
        let cut = store.observation_read_cut().unwrap();
        let first = store
            .message_block_header_page(&cut, message.id(), 0, 1)
            .unwrap();
        assert_eq!(first.total, 2);
        assert_eq!(first.headers[0].block_index.get(), 0);
        assert_eq!(first.next_offset, Some(1));
        store
            .lock()
            .unwrap()
            .execute(
                "UPDATE message_ledger SET message_json='{' WHERE position=1",
                [],
            )
            .unwrap();
        let next = store
            .message_block_header_page_by_locator(&cut, &first.locator, 1, 1)
            .unwrap();
        assert_eq!(next.headers[0].block_index.get(), 1);
        assert_eq!(next.next_offset, None);
        assert_eq!(next.message_id, first.message_id);
        let source = &first.headers[0].content[0].content_ref;
        assert!(store.content_read(source, 0, 64).is_ok());
        assert!(
            store
                .message_block_header_page(&cut, &MessageId::new("missing"), 0, 1)
                .is_err()
        );
        assert!(
            store
                .message_block_header_page_by_locator(
                    &cut,
                    &crate::observation::content::CanonicalContentLocator::ByLedgerPosition {
                        position: 2
                    },
                    0,
                    1,
                )
                .is_err()
        );
    }
}

#[test]
fn canonical_coordinate_pages_reject_mismatched_headers_and_block_owners() {
    use crate::durable::ObservationStore;
    let (store, message) = fixture();
    let cut = store.observation_read_cut().unwrap();
    let connection = store.lock().unwrap();
    for sql in [
        "UPDATE observation_headers SET reference_id='foreign' WHERE ledger_position=1",
        "UPDATE observation_headers SET header_json=CAST(json_set(header_json,'$.identity','foreign') AS BLOB) WHERE ledger_position=1",
        "UPDATE observation_headers SET header_json=CAST(json_set(header_json,'$.ledger_position',2) AS BLOB) WHERE ledger_position=1",
        "UPDATE observation_block_headers SET header_json=CAST(json_set(header_json,'$.block_index',1) AS BLOB) WHERE block_index=0",
        "UPDATE observation_block_headers SET header_json=CAST(json_set(header_json,'$.content[0].content_ref.source.locator.message_id','foreign') AS BLOB) WHERE block_index=0",
        "DELETE FROM observation_block_headers WHERE block_index=0",
    ] {
        connection
            .execute_batch("SAVEPOINT mismatched_owner")
            .unwrap();
        connection.execute_batch(sql).unwrap();
        assert!(
            blocks(
                &connection,
                &store.conversation_id,
                &cut,
                message.id(),
                0,
                1
            )
            .is_err(),
            "{sql}"
        );
        connection
            .execute_batch("ROLLBACK TO mismatched_owner; RELEASE mismatched_owner")
            .unwrap();
        assert!(
            blocks(
                &connection,
                &store.conversation_id,
                &cut,
                message.id(),
                0,
                1
            )
            .is_ok()
        );
    }
}

#[test]
fn canonical_coordinate_queries_keep_bounded_forward_reverse_and_rebind_plans() {
    let (store, _) = fixture();
    let connection = store.lock().unwrap();
    for direction in ["ASC", "DESC"] {
        for sql in [
            format!(
                "SELECT header_json FROM observation_block_headers WHERE ledger_position=1 AND block_index>=0 ORDER BY block_index {direction} LIMIT 32"
            ),
            format!(
                "SELECT header_json FROM observation_block_headers WHERE ledger_position=1 AND json_extract(header_json,'$.kind')='text' AND block_index>=0 ORDER BY block_index {direction} LIMIT 32"
            ),
            format!(
                "SELECT header_json FROM observation_block_headers WHERE ledger_position=(SELECT position FROM message_ledger WHERE message_id='block-source-owner') AND block_index>=0 ORDER BY block_index {direction} LIMIT 32"
            ),
        ] {
            let plan = connection
                .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
                .unwrap()
                .query_map([], |row| row.get::<_, String>(3))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
                .join("\n");
            assert!(
                plan.contains("SEARCH observation_block_headers USING INDEX"),
                "{plan}"
            );
            assert!(
                !plan.contains("SCAN ") && !plan.contains("TEMP B-TREE"),
                "{plan}"
            );
        }
    }
    let columns = connection
        .prepare("PRAGMA table_info(observation_block_headers)")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(columns, ["ledger_position", "block_index", "header_json"]);
    let indexes = connection
        .prepare("PRAGMA index_list(observation_block_headers)")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(indexes.len(), 2);
    assert!(
        indexes
            .iter()
            .any(|name| name == "observation_block_ledger_kind_idx")
    );
}
