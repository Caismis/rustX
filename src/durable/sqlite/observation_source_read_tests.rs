use super::*;
#[cfg(test)]
use crate::durable::CanonicalStore;
use crate::durable::SqliteConversationStore;
use crate::durable::observation::MessageBlockHeader;
use crate::durable::sqlite::observation::index::index_blocks;
use crate::message::content::TextBlock;
use crate::message::types::{UserContentBlock, UserMessageBlock, UserSource};
use crate::runtime::identity::MessageId;

fn fixture() -> (SqliteConversationStore, MessageBlock) {
    let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    let message = MessageBlock::User(UserMessageBlock {
        id: MessageId::new("block-source-owner"),
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

#[test]
fn canonical_block_descriptors_share_one_ledger_lookup_per_block() {
    let (store, message) = fixture();
    let mut connection = store.lock().unwrap();
    let transaction = connection.transaction().unwrap();
    crate::durable::sqlite::content::take_source_work();
    for value in [Some(serde_json::json!({"text":"first"})), None] {
        let (bytes, _) = block_bytes(&transaction, &message, 0, "text", value, &["text"]).unwrap();
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
fn canonical_block_insert_rejects_a_trigger_replacing_its_native_identity() {
    let (store, message) = fixture();
    let mut connection = store.lock().unwrap();
    let transaction = connection.transaction().unwrap();
    transaction
        .execute("DELETE FROM observation_block_headers", [])
        .unwrap();
    transaction.execute_batch("CREATE TEMP TRIGGER replace_block_source BEFORE INSERT ON observation_block_headers WHEN NEW.block_index=0 BEGIN UPDATE message_ledger SET message_id='replaced-block-owner' WHERE position=NEW.ledger_position; END").unwrap();
    crate::durable::sqlite::content::take_source_work();
    assert!(index_blocks(&transaction, &message).is_err());
    assert_eq!(crate::durable::sqlite::content::take_source_work(), (1, 0));
    let native: String = transaction
        .query_row(
            "SELECT message_id FROM message_ledger WHERE position=1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        native,
        message.id().as_str(),
        "the rejected statement also rolls back its trigger"
    );
    transaction.rollback().unwrap();
    assert!(index_blocks(&connection, &message).is_ok());
}

#[test]
fn canonical_block_coordinate_binds_both_halves_of_native_identity() {
    let (store, message) = fixture();
    let connection = store.lock().unwrap();
    connection.execute("INSERT INTO message_ledger(position,message_id,message_json) VALUES(2,'other-block-owner',?1)", [serde_json::to_string(&message).unwrap()]).unwrap();
    // Both coordinates exist independently, but they name different parents.
    assert!(connection.execute("INSERT INTO observation_block_headers(ledger_position,block_index,header_json,message_id) VALUES(1,999,X'7b7d','other-block-owner')", []).is_err());
    assert_eq!(connection.execute("INSERT INTO observation_block_headers(ledger_position,block_index,header_json,message_id) VALUES(2,999,X'7b7d','other-block-owner')", []).unwrap(), 1);
}

#[test]
fn canonical_block_upsert_does_not_heal_a_corrupt_identity_binding() {
    let (store, message) = fixture();
    let connection = store.lock().unwrap();
    connection.execute_batch("PRAGMA foreign_keys=OFF; UPDATE observation_block_headers SET message_id='corrupt-block-owner' WHERE block_index=0; PRAGMA foreign_keys=ON;").unwrap();
    assert!(index_blocks(&connection, &message).is_err());
    let actual: String = connection.query_row("SELECT message_id FROM observation_block_headers WHERE ledger_position=1 AND block_index=0", [], |row| row.get(0)).unwrap();
    assert_eq!(actual, "corrupt-block-owner");
}

#[test]
fn canonical_block_queries_use_numeric_indexes_without_message_key_btrees() {
    let (store, message) = fixture();
    let connection = store.lock().unwrap();
    let message_indexes: u64 = connection.query_row("SELECT count(*) FROM sqlite_schema WHERE type='index' AND tbl_name='observation_block_headers' AND name IN ('sqlite_autoindex_observation_block_headers_2','observation_block_kind_idx')", [], |row|row.get(0)).unwrap();
    assert_eq!(message_indexes, 0);
    for (sql, parameters) in [
        (
            "SELECT header_json FROM observation_block_headers WHERE ledger_position=?1 AND block_index>=?2 ORDER BY block_index LIMIT 2",
            vec![rusqlite::types::Value::Integer(1), 0.into()],
        ),
        (
            "SELECT header_json FROM observation_block_headers WHERE ledger_position=?1 AND block_index=?2 AND message_id=?3",
            vec![1.into(), 0.into(), message.id().as_str().to_owned().into()],
        ),
        (
            "SELECT header_json FROM observation_block_headers WHERE ledger_position=?1 AND json_extract(header_json,'$.kind')=?2 AND block_index>=?3 ORDER BY block_index LIMIT 2",
            vec![1.into(), "text".to_owned().into(), 0.into()],
        ),
    ] {
        let mut statement = connection
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .unwrap();
        let details: Vec<String> = statement
            .query_map(rusqlite::params_from_iter(parameters), |row| row.get(3))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert!(
            details
                .iter()
                .any(|detail| detail.contains("SEARCH observation_block_headers USING INDEX")),
            "{sql}: {details:?}"
        );
        assert!(
            !details
                .iter()
                .any(|detail| detail.contains("SCAN") || detail.contains("TEMP B-TREE")),
            "{sql}: {details:?}"
        );
    }
}
