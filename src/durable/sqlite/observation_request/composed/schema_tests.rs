use super::{hash, index, params};
use crate::durable::CanonicalStore;
use crate::durable::{ConversationStoreError, SqliteConversationStore};
use crate::runtime::identity::ConversationId;
use rusqlite::{Connection, types::Value};
use std::sync::atomic::Ordering;

const LEGACY_PAGES: &str = "CREATE TABLE request_source_pages(hash BLOB PRIMARY KEY CHECK(typeof(hash)='blob' AND length(hash)=32),bytes BLOB NOT NULL CHECK(typeof(bytes)='blob' AND length(bytes)<=65536)) WITHOUT ROWID";

#[test]
fn t08_t12_t19_source_page_rowid_constraints_and_hash_dedup_remain_effective() {
    let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    store.initialize(&[]).unwrap();
    let connection = store.lock().unwrap();
    let epoch = store.conn.manifest.epoch.load(Ordering::Acquire);
    let original = b"original immutable page";
    let digest = hash(original);
    assert_eq!(
        connection
            .execute(
                "INSERT INTO request_source_pages(hash,bytes) VALUES(?1,?2)",
                params![digest.as_slice(), original.as_slice()],
            )
            .unwrap(),
        1
    );
    let rowid: i64 = connection
        .query_row(
            "SELECT rowid FROM request_source_pages WHERE hash=?1",
            [digest.as_slice()],
            |row| row.get(0),
        )
        .unwrap();
    assert!(rowid > 0);
    assert_eq!(
        connection
            .execute(
                "INSERT OR IGNORE INTO request_source_pages(hash,bytes) VALUES(?1,?2)",
                params![digest.as_slice(), b"different bytes".as_slice()],
            )
            .unwrap(),
        0
    );
    assert_eq!(
        index::load_blob(&connection, digest).unwrap(),
        original.as_slice()
    );
    for (key, bytes, expected) in invalid_pages() {
        let error = connection
            .execute(
                "INSERT INTO request_source_pages(hash,bytes) VALUES(?1,?2)",
                params![key, bytes],
            )
            .unwrap_err();
        assert_eq!(error.sqlite_error().unwrap().extended_code, expected);
    }
    let boundary = vec![0; 65_536];
    let boundary_hash = index::store_blob(&connection, &boundary).unwrap();
    assert_eq!(
        index::load_blob(&connection, boundary_hash).unwrap(),
        boundary
    );
    let count: u64 = connection
        .query_row("SELECT count(*) FROM request_source_pages", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(count, 2);
    assert_eq!(
        store.conn.manifest.epoch.load(Ordering::Acquire),
        epoch,
        "optional page writes do not certify or invalidate native selection"
    );
}

fn invalid_pages() -> Vec<(Value, Value, i32)> {
    let key = Value::Blob(vec![1; 32]);
    let blob = Value::Blob(vec![2]);
    let check = rusqlite::ffi::SQLITE_CONSTRAINT_CHECK;
    let not_null = rusqlite::ffi::SQLITE_CONSTRAINT_NOTNULL;
    vec![
        (Value::Null, blob.clone(), not_null),
        (Value::Blob(vec![1; 31]), blob.clone(), check),
        (Value::Blob(vec![1; 33]), blob.clone(), check),
        (Value::Text("x".repeat(32)), blob.clone(), check),
        (key.clone(), Value::Null, not_null),
        (key.clone(), Value::Text("[]".into()), check),
        (key, Value::Blob(vec![0; 65_537]), check),
    ]
}

#[test]
fn t10_t19_source_page_schema_rejects_the_old_clustered_representation() {
    let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    let connection = store.lock().unwrap();
    connection
        .execute_batch("SAVEPOINT old_page_layout; DROP TABLE request_source_pages")
        .unwrap();
    connection.execute_batch(LEGACY_PAGES).unwrap();
    assert!(matches!(
        super::verify_schema(&connection),
        Err(ConversationStoreError::IncompatibleSchema(_))
    ));
    connection
        .execute_batch("ROLLBACK TO old_page_layout; RELEASE old_page_layout")
        .unwrap();
    super::verify_schema(&connection).unwrap();
}

#[test]
fn t10_t19_schema_65_source_pages_are_refused_without_conversion() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("old-source-pages.sqlite");
    let conversation = ConversationId::generate();
    let store = SqliteConversationStore::open(conversation.clone(), &path).unwrap();
    store.initialize(&[]).unwrap();
    drop(store);
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch("DROP TABLE request_source_pages")
        .unwrap();
    connection.execute_batch(LEGACY_PAGES).unwrap();
    connection
        .execute("UPDATE rustx_store SET schema_version=65 WHERE id=1", [])
        .unwrap();
    drop(connection);
    assert!(matches!(
        SqliteConversationStore::open_existing(conversation, &path),
        Err(ConversationStoreError::SchemaVersionMismatch {
            stored: 65,
            expected: crate::durable::sqlite::SQLITE_SCHEMA_VERSION
        })
    ));
    let connection = Connection::open(path).unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT schema_version FROM rustx_store WHERE id=1",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        65
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE name='request_source_pages'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        LEGACY_PAGES
    );
}

#[test]
fn t10_t19_schema_66_full_child_facts_are_refused_without_conversion() {
    use base64::Engine as _;

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("old-child-facts.sqlite");
    let conversation = ConversationId::generate();
    let store = SqliteConversationStore::open(conversation.clone(), &path).unwrap();
    store.initialize(&[]).unwrap();
    let fact = super::Fact {
        path: vec!["legacy".into()],
        parent: Some(Vec::new()),
        ordinal: Some(0),
        start: 1,
        end: 2,
        children: 0,
        nodes: 1,
        object: false,
        container: false,
        header: base64::engine::general_purpose::STANDARD.encode(b"0"),
        key: None,
        name: Some("legacy".into()),
    };
    let legacy = serde_json::to_vec(&serde_json::json!({"type":"leaf","entries":[[
        "C[]:00000000000000000000",
        base64::engine::general_purpose::STANDARD.encode(serde_json::to_vec(&fact).unwrap())
    ]]}))
    .unwrap();
    let digest = hash(&legacy);
    {
        let connection = store.lock().unwrap();
        connection
            .execute(
                "INSERT INTO request_source_pages(hash,bytes) VALUES(?1,?2)",
                params![digest.as_slice(), legacy],
            )
            .unwrap();
        connection
            .execute("UPDATE rustx_store SET schema_version=66 WHERE id=1", [])
            .unwrap();
    }
    drop(store);
    assert!(matches!(
        SqliteConversationStore::open_existing(conversation, &path),
        Err(ConversationStoreError::SchemaVersionMismatch {
            stored: 66,
            expected: crate::durable::sqlite::SQLITE_SCHEMA_VERSION
        })
    ));
    let connection = Connection::open(path).unwrap();
    let retained: Vec<u8> = connection
        .query_row(
            "SELECT bytes FROM request_source_pages WHERE hash=?1",
            [digest.as_slice()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        retained, legacy,
        "refusal never rewrites the old child payload"
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT schema_version FROM rustx_store WHERE id=1",
                [],
                |row| row.get::<_, u32>(0)
            )
            .unwrap(),
        66
    );
}

#[test]
fn binary_fragment_gate_rejects_schema_70_without_rewriting_it() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("page-v70.sqlite");
    let conversation = crate::runtime::identity::ConversationId::generate();
    let store =
        crate::durable::sqlite::SqliteConversationStore::open(conversation.clone(), &path).unwrap();
    store
        .conn
        .lock()
        .unwrap()
        .execute("UPDATE rustx_store SET schema_version=70", [])
        .unwrap();
    drop(store);
    assert!(matches!(
        crate::durable::sqlite::SqliteConversationStore::open_existing(conversation, &path),
        Err(
            crate::durable::ConversationStoreError::SchemaVersionMismatch {
                stored: 70,
                expected: crate::durable::sqlite::SQLITE_SCHEMA_VERSION
            }
        )
    ));
    assert_eq!(
        Connection::open(&path)
            .unwrap()
            .query_row("SELECT schema_version FROM rustx_store", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        70
    );
}
