use super::*;
use crate::durable::SqliteConversationStore;
use crate::durable::{CanonicalStore, ObservationStore};
use crate::message::content::FileReference;
use crate::message::types::{
    ContentBlockIndex, InboundKind, UserContentBlock, UserMessageBlock, UserSource,
};
use crate::runtime::identity::{ArtifactId, MessageId};
use crate::tools::artifacts::ArtifactStore;
use std::io::{Seek as _, Write as _};

struct BinaryFixture {
    directory: tempfile::TempDir,
    store: SqliteConversationStore,
    reference: ContentRef,
    artifact: ArtifactId,
    bytes: Vec<u8>,
}

impl BinaryFixture {
    fn new(length: usize) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let conversation = ConversationId::generate();
        let artifacts = ArtifactStore::new(conversation.clone(), directory.path()).unwrap();
        let artifact = artifacts.create_artifact().unwrap();
        let bytes = (0..length)
            .map(|index| u8::try_from(index % 251).unwrap())
            .collect::<Vec<_>>();
        let mut writer = artifacts.open_writer(&artifact).unwrap();
        writer.write_all(&bytes).unwrap();
        writer.finish().unwrap();
        let store =
            SqliteConversationStore::open(conversation, &directory.path().join("binary.sqlite"))
                .unwrap();
        store
            .initialize(&[MessageBlock::User(UserMessageBlock {
                id: MessageId::new("binary-integrity-owner"),
                content: vec![UserContentBlock::File(FileReference {
                    artifact_id: artifact.clone(),
                    name: None,
                    mime_type: None,
                    description: None,
                })],
                source: UserSource::Human,
                kind: InboundKind::Message,
                timestamp: None,
            })])
            .unwrap();
        let reference = store
            .content_descriptor(
                &Source::CanonicalMessageBlock {
                    component: CanonicalContentComponent::Payload,
                    locator: CanonicalContentLocator::ByLedgerPosition { position: 1 },
                    block_index: ContentBlockIndex::new(0),
                },
                Rep::BinaryBytes,
            )
            .unwrap()
            .unwrap();
        Self {
            directory,
            store,
            reference,
            artifact,
            bytes,
        }
    }

    fn overwrite(&self, offset: u64, bytes: &[u8]) {
        let path = self
            .directory
            .path()
            .join(format!("{}.bin", self.artifact.as_str()));
        let mut file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
        file.seek(std::io::SeekFrom::Start(offset)).unwrap();
        file.write_all(bytes).unwrap();
        file.sync_all().unwrap();
        assert_eq!(file.metadata().unwrap().len(), self.reference.readable_end);
    }
}

#[test]
fn binary_content_reference_rejects_same_length_external_modification_after_reopen() {
    let fixture = BinaryFixture::new(3 * SEGMENT_BYTES + 17);
    let chunk = fixture
        .store
        .content_read(&fixture.reference, SEGMENT + 7, 5)
        .unwrap();
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(chunk.data_base64)
            .unwrap(),
        fixture.bytes[SEGMENT_BYTES + 7..SEGMENT_BYTES + 12]
    );
    fixture.overwrite(SEGMENT + 7, b"other");
    assert!(
        fixture
            .store
            .content_read(&fixture.reference, SEGMENT + 7, 5)
            .is_err(),
        "same ContentRef must never return externally replaced bytes"
    );
    let reopened = SqliteConversationStore::open(
        fixture.reference.conversation_id.clone(),
        &fixture.directory.path().join("binary.sqlite"),
    )
    .unwrap();
    assert!(
        reopened
            .content_read(&fixture.reference, SEGMENT + 7, 5)
            .is_err()
    );
}

thread_local! {
    static BEFORE_READ: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
    static AFTER_VERIFIED: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}

pub(super) fn before_chunk_read() {
    let hook = BEFORE_READ.with(|hook| hook.borrow_mut().take());
    if let Some(hook) = hook {
        hook();
    }
}
pub(super) fn after_chunk_verified() {
    let hook = AFTER_VERIFIED.with(|hook| hook.borrow_mut().take());
    if let Some(hook) = hook {
        hook();
    }
}

#[test]
fn binary_ranges_verify_only_intersecting_fixed_chunks_with_exact_eof() {
    let fixture = BinaryFixture::new(32 * SEGMENT_BYTES + 17);
    for offset in [
        0,
        1,
        SEGMENT - 1,
        SEGMENT + 19,
        fixture.reference.readable_end - 13,
        fixture.reference.readable_end,
    ] {
        let chunk = fixture
            .store
            .content_read(&fixture.reference, offset, 65_536)
            .unwrap();
        let end = (offset + SEGMENT).min(fixture.reference.readable_end);
        let actual = base64::engine::general_purpose::STANDARD
            .decode(chunk.data_base64)
            .unwrap();
        assert_eq!(
            actual,
            fixture.bytes[usize::try_from(offset).unwrap()..usize::try_from(end).unwrap()]
        );
        let (segments, bytes) = last_read_work();
        let first = offset / SEGMENT * SEGMENT;
        let expected = if end == offset {
            0
        } else {
            usize::try_from(
                ((end - 1) / SEGMENT * SEGMENT + SEGMENT).min(fixture.reference.readable_end)
                    - first,
            )
            .unwrap()
        };
        assert_eq!(bytes, expected);
        assert_eq!(binary::hashed_bytes(), expected);
        assert!(segments <= 2 && bytes <= 2 * SEGMENT_BYTES);
        assert_eq!(
            chunk.next_offset,
            (end < fixture.reference.readable_end).then_some(end)
        );
    }
    // Changes outside the touched chunks require no whole-file scan.
    fixture.overwrite(20 * SEGMENT, b"other");
    assert!(fixture.store.content_read(&fixture.reference, 0, 1).is_ok());
    assert_eq!(last_read_work(), (1, SEGMENT_BYTES));
    assert!(
        fixture
            .store
            .content_read(&fixture.reference, 20 * SEGMENT, 1)
            .is_err()
    );
}

#[test]
fn binary_integrity_checks_open_read_races_and_returns_only_verified_buffers() {
    let fixture = BinaryFixture::new(SEGMENT_BYTES + 17);
    let path = fixture
        .directory
        .path()
        .join(format!("{}.bin", fixture.artifact.as_str()));
    BEFORE_READ.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            let mut file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
            file.write_all(b"other").unwrap();
        }));
    });
    assert!(
        fixture
            .store
            .content_read(&fixture.reference, 0, 5)
            .is_err(),
        "mutation after settled-reader admission must be detected"
    );
    fixture.overwrite(0, &fixture.bytes[..5]);
    let path = fixture
        .directory
        .path()
        .join(format!("{}.bin", fixture.artifact.as_str()));
    AFTER_VERIFIED.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            let mut file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
            file.write_all(b"other").unwrap();
        }));
    });
    let chunk = fixture
        .store
        .content_read(&fixture.reference, 0, 5)
        .unwrap();
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(chunk.data_base64)
            .unwrap(),
        fixture.bytes[..5],
        "never reread the file after verification"
    );
    assert!(
        fixture
            .store
            .content_read(&fixture.reference, 0, 5)
            .is_err()
    );
}

#[test]
fn binary_digest_corruption_is_bounded_and_missing_evidence_never_falls_back() {
    let fixture = BinaryFixture::new(3 * SEGMENT_BYTES);
    let digest = {
        let connection = fixture.store.lock().unwrap();
        connection
            .query_row(
                "SELECT digest FROM canonical_binary_segments WHERE segment=0",
                [],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .unwrap()
    };
    for expression in [
        "zeroblob(32)",
        "zeroblob(2000000)",
        "CAST(zeroblob(2000000) AS TEXT)",
        "NULL",
    ] {
        let connection = fixture.store.lock().unwrap();
        connection
            .execute_batch("PRAGMA ignore_check_constraints=ON")
            .unwrap();
        // NOT NULL remains effective even when CHECKs are deliberately bypassed.
        if expression == "NULL" {
            assert!(
                connection
                    .execute(
                        "UPDATE canonical_binary_segments SET digest=NULL WHERE segment=0",
                        []
                    )
                    .is_err()
            );
            continue;
        }
        connection
            .execute(
                &format!(
                    "UPDATE canonical_binary_segments SET digest={expression} WHERE segment=0"
                ),
                [],
            )
            .unwrap();
        drop(connection);
        assert!(
            fixture
                .store
                .content_read(&fixture.reference, 0, 1)
                .is_err()
        );
        if expression != "zeroblob(32)" {
            assert_eq!(last_read_work(), (0, 0));
        }
        fixture
            .store
            .lock()
            .unwrap()
            .execute(
                "UPDATE canonical_binary_segments SET digest=?1 WHERE segment=0",
                [digest.as_slice()],
            )
            .unwrap();
    }
    fixture
        .store
        .lock()
        .unwrap()
        .execute("DELETE FROM canonical_binary_segments WHERE segment=0", [])
        .unwrap();
    assert!(
        fixture
            .store
            .content_read(&fixture.reference, 0, 1)
            .is_err()
    );
    assert_eq!(last_read_work(), (0, 0));
    assert!(
        fixture
            .store
            .content_read(&fixture.reference, SEGMENT, 1)
            .is_ok()
    );
}

#[test]
fn binary_empty_frontier_and_invalid_ranges_preserve_content_contract() {
    let fixture = BinaryFixture::new(0);
    let chunk = fixture
        .store
        .content_read(&fixture.reference, 0, 65_536)
        .unwrap();
    assert_eq!(chunk.data_base64, "");
    assert!(chunk.next_offset.is_none());
    assert_eq!(last_read_work(), (0, 0));
    for (offset, maximum) in [(1, 1), (0, 0), (0, 65_537)] {
        assert!(matches!(
            fixture
                .store
                .content_read(&fixture.reference, offset, maximum),
            Err(ConversationStoreError::InvalidContentRange)
        ));
    }
}

#[test]
fn binary_digest_index_failure_is_optional_and_rolls_back_its_frontier() {
    let fixture = BinaryFixture::new(SEGMENT_BYTES + 17);
    fixture.store.lock().unwrap().execute_batch("CREATE TRIGGER fail_binary_digest BEFORE INSERT ON canonical_binary_segments WHEN NEW.source_id='ledger:2' BEGIN SELECT RAISE(FAIL,'binary index fault'); END").unwrap();
    let message = MessageBlock::User(UserMessageBlock {
        id: MessageId::new("binary-second-owner"),
        content: vec![UserContentBlock::File(FileReference {
            artifact_id: fixture.artifact.clone(),
            name: None,
            mime_type: None,
            description: None,
        })],
        source: UserSource::Human,
        kind: InboundKind::Message,
        timestamp: None,
    });
    fixture.store.append_canonical(&message).unwrap();
    assert_eq!(
        fixture.store.load_head().unwrap().active_message_ids,
        [
            MessageId::new("binary-integrity-owner"),
            MessageId::new("binary-second-owner"),
        ]
    );
    let source = Source::CanonicalMessageBlock {
        component: CanonicalContentComponent::Payload,
        locator: CanonicalContentLocator::ByLedgerPosition { position: 2 },
        block_index: ContentBlockIndex::new(0),
    };
    assert!(
        fixture
            .store
            .content_descriptor(&source, Rep::BinaryBytes)
            .unwrap()
            .is_none()
    );
    assert!(fixture.store.content_read(&fixture.reference, 0, 5).is_ok());
    let connection = fixture.store.lock().unwrap();
    for table in [
        "canonical_binary_sources",
        "canonical_binary_segments",
        "content_frontiers",
    ] {
        let count: u64 = connection
            .query_row(
                &format!("SELECT count(*) FROM {table} WHERE source_id='ledger:2'"),
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0, "optional index rollback: {table}");
    }
}

#[test]
fn binary_integrity_schema_refuses_68_and_relaxed_digest_tables() {
    let fixture = BinaryFixture::new(17);
    let path = fixture.directory.path().join("binary.sqlite");
    fixture
        .store
        .lock()
        .unwrap()
        .execute("UPDATE rustx_store SET schema_version=68", [])
        .unwrap();
    assert!(matches!(
        SqliteConversationStore::open_existing(fixture.reference.conversation_id.clone(), &path),
        Err(ConversationStoreError::SchemaVersionMismatch {
            stored: 68,
            expected: crate::durable::sqlite::SQLITE_SCHEMA_VERSION
        })
    ));
    fixture
        .store
        .lock()
        .unwrap()
        .execute(
            "UPDATE rustx_store SET schema_version=?1",
            [crate::durable::sqlite::SQLITE_SCHEMA_VERSION],
        )
        .unwrap();
    fixture.store.lock().unwrap().execute_batch("DROP TABLE canonical_binary_segments; CREATE TABLE canonical_binary_segments(source_id TEXT,block_index INTEGER,segment INTEGER,digest BLOB)").unwrap();
    assert!(matches!(
        SqliteConversationStore::open_existing(fixture.reference.conversation_id.clone(), &path),
        Err(ConversationStoreError::IncompatibleSchema(_))
    ));
}

#[test]
fn canonical_binary_index_cannot_reauthenticate_a_modified_native_artifact() {
    let fixture = BinaryFixture::new(SEGMENT_BYTES + 17);
    fixture.overwrite(7, b"changed");
    assert!(
        fixture
            .store
            .content_read(&fixture.reference, 0, 17)
            .is_err()
    );
    let message = MessageBlock::User(UserMessageBlock {
        id: MessageId::new("binary-later-binding"),
        content: vec![UserContentBlock::File(FileReference {
            artifact_id: fixture.artifact.clone(),
            name: None,
            mime_type: None,
            description: None,
        })],
        source: UserSource::Human,
        kind: InboundKind::Message,
        timestamp: None,
    });
    fixture.store.append_canonical(&message).unwrap();
    assert_eq!(
        fixture.store.load_head().unwrap().active_message_ids,
        [
            MessageId::new("binary-integrity-owner"),
            MessageId::new("binary-later-binding")
        ]
    );
    let later = Source::CanonicalMessageBlock {
        component: CanonicalContentComponent::Payload,
        locator: CanonicalContentLocator::ByLedgerPosition { position: 2 },
        block_index: ContentBlockIndex::new(0),
    };
    assert!(
        fixture
            .store
            .content_descriptor(&later, Rep::BinaryBytes)
            .unwrap()
            .is_none(),
        "a new canonical binding cannot freeze changed bytes as a fresh Original"
    );
    let connection = fixture.store.lock().unwrap();
    for table in [
        "canonical_binary_sources",
        "canonical_binary_segments",
        "content_frontiers",
    ] {
        let count: u64 = connection
            .query_row(
                &format!("SELECT count(*) FROM {table} WHERE source_id='ledger:2'"),
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            count, 0,
            "failed native integrity retires optional index {table}"
        );
    }
}
