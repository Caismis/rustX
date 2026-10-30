use super::*;
use crate::durable::{ConversationStore, SqliteConversationStore};
use crate::message::content::TextBlock;
use crate::message::types::{
    ContentBlockIndex, InboundKind, UserContentBlock, UserMessageBlock, UserSource,
};
use crate::runtime::identity::MessageId;

fn fixture(id: &str) -> (SqliteConversationStore, Source) {
    let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    let message_id = MessageId::new(id);
    store
        .initialize(&[MessageBlock::User(UserMessageBlock {
            id: message_id.clone(),
            content: vec![UserContentBlock::Text(TextBlock {
                text: "exact 水 bytes".into(),
            })],
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: None,
        })])
        .unwrap();
    let source = Source::CanonicalMessageBlock {
        locator: CanonicalContentLocator::ByMessageId { message_id },
        block_index: ContentBlockIndex::new(0),
        component: CanonicalContentComponent::Payload,
    };
    (store, source)
}

#[test]
fn canonical_content_descriptor_and_read_resolve_the_source_once() {
    for id in ["bounded".to_owned(), "long-original-id".repeat(40)] {
        let (store, source) = fixture(&id);
        take_source_work();
        let reference = store
            .content_descriptor(&source, Rep::Utf8Text)
            .unwrap()
            .unwrap();
        assert_eq!(take_source_work(), (1, 0));
        if id.len() > 256 {
            assert!(matches!(
                reference.source,
                Source::CanonicalMessageBlock {
                    locator: CanonicalContentLocator::ByLedgerPosition { position: 1 },
                    ..
                }
            ));
        }
        assert_eq!(
            store
                .content_descriptor_at(&source, Rep::Utf8Text, 1)
                .unwrap(),
            Some(reference.clone())
        );
        assert_eq!(take_source_work(), (1, 0));
        let chunk = store.content_read(&reference, 0, 1024).unwrap();
        assert_eq!(take_source_work(), (1, 1));
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(&chunk.data_base64)
                .unwrap(),
            "exact 水 bytes".as_bytes()
        );
        let mut connection = store.lock().unwrap();
        let transaction = connection.transaction().unwrap();
        let (bytes, end) =
            read_bytes(&transaction, store.conversation_id(), &reference, 0, 1024).unwrap();
        assert_eq!(take_source_work(), (1, 0));
        assert_eq!(bytes, "exact 水 bytes".as_bytes());
        assert_eq!(end, chunk.end_offset);
    }
}

#[test]
fn canonical_content_read_keeps_range_source_frontier_error_order() {
    let (store, source) = fixture("read-error-order");
    let reference = store
        .content_descriptor(&source, Rep::Utf8Text)
        .unwrap()
        .unwrap();
    let mut missing_source = reference.clone();
    missing_source.source = Source::CanonicalMessageBlock {
        locator: CanonicalContentLocator::ByLedgerPosition { position: 2 },
        block_index: ContentBlockIndex::new(0),
        component: CanonicalContentComponent::Payload,
    };
    take_source_work();
    assert!(matches!(
        store.content_read(&missing_source, 0, 0),
        Err(ConversationStoreError::InvalidContentRange)
    ));
    assert_eq!(
        take_source_work(),
        (0, 0),
        "range validation precedes source resolution"
    );
    assert!(matches!(
        store.content_read(&missing_source, 0, 1),
        Err(ConversationStoreError::InvalidReference(_))
    ));
    assert_eq!(take_source_work(), (1, 0));
    assert!(
        store
            .content_descriptor_at(&source, Rep::Utf8Text, CONTENT_SAFE_INTEGER_MAX + 1)
            .is_err()
    );
    assert_eq!(
        take_source_work(),
        (1, 0),
        "source validation precedes revision validation"
    );
    {
        let connection = store.lock().unwrap();
        connection.execute("DELETE FROM content_frontiers WHERE source_kind='canonical' AND source_id='ledger:1' AND representation='utf8_text'", []).unwrap();
    }
    assert!(
        matches!(
            store.content_read(&reference, reference.readable_end + 1, 1),
            Err(ConversationStoreError::InvalidReference(_))
        ),
        "missing frontier precedes offset validation"
    );
    assert!(
        store.lock().unwrap().is_autocommit(),
        "a failed public read releases its read transaction"
    );
}

#[test]
fn resolved_content_reads_preserve_raw_frontier_and_actual_byte_checks() {
    let (store, source) = fixture("raw-independent-evidence");
    let mut connection = store.lock().unwrap();
    let transaction = connection.transaction().unwrap();
    let raw = br#"{ "original": true }"#;
    put(
        &transaction,
        ("canonical", "ledger:1", 0, Rep::RawJsonUtf8),
        1,
        raw,
        false,
    )
    .unwrap();
    let position = canonical_position(
        &transaction,
        match &source {
            Source::CanonicalMessageBlock { locator, .. } => locator,
            _ => unreachable!(),
        },
    )
    .unwrap();
    take_source_work();
    let reference = canonical_descriptor_at_position(
        &transaction,
        store.conversation_id(),
        &source,
        Rep::RawJsonUtf8,
        position,
    )
    .unwrap()
    .unwrap();
    let (bytes, _) = canonical_read_bytes_at_position(
        &transaction,
        store.conversation_id(),
        &reference,
        0,
        1024,
        position,
    )
    .unwrap();
    assert_eq!(bytes, raw);
    assert_eq!(take_source_work(), (0, 0));
    // The already resolved position does not replace frontier or body evidence.
    transaction.execute("DELETE FROM content_frontiers WHERE source_id='ledger:1' AND representation='raw_json_utf8'", []).unwrap();
    let error = descriptor(
        &transaction,
        store.conversation_id(),
        &source,
        Rep::RawJsonUtf8,
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("retained original raw arguments frontier unavailable")
    );
    assert!(read_bytes(&transaction, store.conversation_id(), &reference, 0, 1024).is_err());
    transaction.execute("INSERT INTO content_frontiers(source_kind,source_id,block_index,representation,revision,readable_end) VALUES('canonical','ledger:1',0,'raw_json_utf8',1,?1)", [raw.len() as u64]).unwrap();
    transaction.execute("UPDATE content_segments SET bytes=x'ff' WHERE source_id='ledger:1' AND representation='raw_json_utf8'", []).unwrap();
    assert!(
        read_bytes(&transaction, store.conversation_id(), &reference, 0, 1024).is_err(),
        "short raw body cannot use a valid frontier as proof of its bytes"
    );
}
