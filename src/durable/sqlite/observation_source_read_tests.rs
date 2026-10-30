use super::*;
use crate::durable::{ConversationStore, SqliteConversationStore};
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
    super::super::content::take_source_work();
    for value in [Some(serde_json::json!({"text":"first"})), None] {
        let bytes = block_bytes(&transaction, &message, 0, "text", value, &["text"]).unwrap();
        let header: MessageBlockHeader = serde_json::from_slice(&bytes).unwrap();
        assert!(!header.content.is_empty());
        assert_eq!(
            super::super::content::take_source_work(),
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
                "DELETE FROM observation_block_headers WHERE message_id=?1",
                [message.id().as_str()],
            )
            .unwrap();
        transaction.execute_batch("CREATE TEMP TRIGGER erase_block_source AFTER INSERT ON observation_block_headers WHEN NEW.block_index=0 BEGIN DELETE FROM message_ledger WHERE message_id=NEW.message_id; END").unwrap();
        super::super::content::take_source_work();
        assert!(index_blocks(&transaction, &message).is_err());
        assert_eq!(
            super::super::content::take_source_work(),
            (2, 0),
            "the second block cannot reuse position evidence from before the first INSERT"
        );
        // Drop rolls back both the successful first block and the trigger write.
    }
    assert!(connection.is_autocommit());
    let transaction = connection.transaction().unwrap();
    assert!(block_bytes(&transaction, &message, 1, "text", None, &["text"]).is_ok());
}
