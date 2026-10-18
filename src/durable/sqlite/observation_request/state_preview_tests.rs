use super::*;
use crate::runtime::read_layout::{FieldPreview, SeekableObservationSource};

#[derive(serde::Serialize, schemars::JsonSchema)]
struct Fixture {
    revision: u64,
    opaque: serde_json::Value,
    messages: Vec<serde_json::Value>,
}
fn fixture() -> Fixture {
    Fixture {
        revision: u64::MAX,
        opaque: serde_json::json!({
            "z":-0.0,"u":u64::MAX,"escapes":"\"\\\n水🙂","@reserved":[true,null,1],"$key":{"x":"literal"},
            "long-key".repeat(100): {"nested":"large key"},"large":"x".repeat(70_000),
            "children":(0..100).map(|n|serde_json::json!({"n":n,"text":"value"})).collect::<Vec<_>>()
        }),
        messages: vec![serde_json::json!({"many":(0..40).collect::<Vec<_>>()})],
    }
}
fn connection() -> Connection {
    let mut connection = Connection::open_in_memory().unwrap();
    crate::durable::sqlite::create_schema(&mut connection).unwrap();
    connection
}

#[test]
fn state_preview_capture_skips_unused_previews_and_lazy_reads_match_eager_oracle() {
    let value = fixture();
    let original = SeekableObservationSource::new(&value).unwrap();
    let mut expected = Vec::new();
    original
        .visit_fields(|path, _, _, start, end, children, _, preview| {
            expected.push((
                path.to_vec(),
                start,
                end,
                children,
                stored_preview(&original, path, start, end, FieldPreview::Inline(preview)).unwrap(),
            ));
            Ok(())
        })
        .unwrap();
    let connection = connection();
    crate::runtime::read_layout::take_field_preview_work();
    index_state(&connection, "preview_test", "source", &value).unwrap();
    let (computed, skipped) = crate::runtime::read_layout::take_field_preview_work();
    assert_eq!(computed, 0);
    assert_eq!(skipped, expected.len());
    let changes = connection.total_changes();
    connection.execute_batch("PRAGMA query_only=ON").unwrap();
    for (path, start, end, children, preview) in expected {
        assert_eq!(
            field_from(&connection, "state_observation_fields", "source", &path).unwrap(),
            (start, end, children, preview),
            "{path:?}"
        );
        let bytes = read_from(
            &connection,
            "preview_test",
            "state_observation_fields",
            "source",
            &path,
            0,
            64,
        )
        .unwrap();
        assert_eq!(bytes.bytes, original.read(&path, 0, 64).unwrap().bytes);
        if children > 0 {
            let mut offset = 0;
            loop {
                let page = page_from(
                    &connection,
                    "state_observation_fields",
                    "source",
                    &path,
                    offset,
                    3,
                )
                .unwrap();
                assert_eq!(page.total, children);
                assert!(!page.fields.is_empty());
                for field in &page.fields {
                    let (_, _, _, expected) = field_from(
                        &connection,
                        "state_observation_fields",
                        "source",
                        &field.path,
                    )
                    .unwrap();
                    assert_eq!(
                        field.value,
                        serde_json::from_slice::<serde_json::Value>(&expected).unwrap()
                    );
                }
                if let Some(next) = page.next_offset {
                    assert!(next > offset);
                    offset = next;
                } else {
                    break;
                }
            }
        }
    }
    assert_eq!(
        connection.total_changes(),
        changes,
        "reading does not persist a cache or rebuild navigation"
    );
}

#[test]
fn state_preview_rejects_malformed_bytes_and_invalid_geometry_without_repair() {
    let value = fixture();
    let connection = connection();
    index_state(&connection, "preview_test", "source", &value).unwrap();
    connection.execute_batch("SAVEPOINT malformed; UPDATE content_segments SET bytes=zeroblob(length(bytes)) WHERE source_kind='preview_test' AND source_id='source';").unwrap();
    assert!(
        field_from(
            &connection,
            "state_observation_fields",
            "source",
            &["revision".into()]
        )
        .is_err()
    );
    connection
        .execute_batch(
            "ROLLBACK TO malformed; RELEASE malformed; PRAGMA ignore_check_constraints=ON",
        )
        .unwrap();
    for assignment in [
        "preview_kind=99",
        "preview_nodes=0",
        "end=9007199254740992",
        "source_kind=NULL",
    ] {
        connection.execute_batch(&format!("SAVEPOINT corrupt; UPDATE state_observation_fields SET {assignment} WHERE source_id='source' AND path_json='[]';")).unwrap();
        assert!(
            field_from(&connection, "state_observation_fields", "source", &[]).is_err(),
            "{assignment}"
        );
        connection
            .execute_batch("ROLLBACK TO corrupt; RELEASE corrupt")
            .unwrap();
    }
    connection
        .execute_batch("PRAGMA ignore_check_constraints=OFF")
        .unwrap();
    assert!(field_from(&connection, "state_observation_fields", "source", &[]).is_ok());
}

#[test]
fn lazy_preview_cold_reads_are_bounded_and_invalid_layout_is_rejected() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("previews.sqlite");
    let mut connection = Connection::open(&path).unwrap();
    crate::durable::sqlite::create_schema(&mut connection).unwrap();
    let value = serde_json::json!({"body":"x".repeat(2_000_000), "rows":(0..2048).collect::<Vec<_>>(), "small":u64::MAX});
    index_state(&connection, "preview_test", "source", &value).unwrap();
    drop(connection);
    let connection = Connection::open(&path).unwrap();
    crate::durable::sqlite::verify_schema_shape(&connection).unwrap();
    connection
        .execute_batch("PRAGMA query_only=ON; PRAGMA cache_size=8")
        .unwrap();
    let steps = Arc::new(AtomicUsize::new(0));
    let count = steps.clone();
    connection
        .progress_handler(
            1,
            Some(move || {
                count.fetch_add(1, Ordering::Relaxed);
                false
            }),
        )
        .unwrap();
    let small = field_from(
        &connection,
        "state_observation_fields",
        "source",
        &["small".into()],
    )
    .unwrap();
    // Opaque browser-unsafe integers defer their preview; exact range bytes
    // remain authoritative, as in the eager implementation.
    assert_eq!(small.3, b"null");
    assert!(steps.swap(0, Ordering::Relaxed) < 500);
    assert_eq!(crate::durable::sqlite::content::last_read_work(), (1, 20));
    let exact = read_from(
        &connection,
        "preview_test",
        "state_observation_fields",
        "source",
        &["small".into()],
        0,
        32,
    )
    .unwrap();
    assert_eq!(exact.bytes, u64::MAX.to_string().as_bytes());
    assert!(steps.swap(0, Ordering::Relaxed) < 500);
    let page = page_from(
        &connection,
        "state_observation_fields",
        "source",
        &["rows".into()],
        2046,
        2,
    )
    .unwrap();
    assert_eq!(
        page.fields
            .iter()
            .map(|field| field.value.clone())
            .collect::<Vec<_>>(),
        vec![serde_json::json!(2046), serde_json::json!(2047)]
    );
    assert!(steps.swap(0, Ordering::Relaxed) < 1000);
    let bytes = read_from(
        &connection,
        "preview_test",
        "state_observation_fields",
        "source",
        &["body".into()],
        1_999_900,
        32,
    )
    .unwrap();
    assert_eq!(bytes.bytes, b"xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx");
    assert_eq!(crate::durable::sqlite::content::last_read_work(), (1, 32));
    assert!(steps.load(Ordering::Relaxed) < 500);
    connection
        .progress_handler(0, None::<fn() -> bool>)
        .unwrap();
    connection
        .execute_batch("PRAGMA query_only=OFF; DROP INDEX state_observation_children_idx")
        .unwrap();
    assert!(crate::durable::sqlite::verify_schema_shape(&connection).is_err());
    connection.execute_batch("CREATE UNIQUE INDEX state_observation_children_idx ON state_observation_fields(source_id,parent_path,ordinal); ALTER TABLE state_observation_fields DROP COLUMN preview_key;").unwrap();
    assert!(state_preview::verify_schema(&connection).is_err());
}

#[test]
fn previous_eager_state_layout_is_rejected_without_migration() {
    use crate::durable::sqlite::SqliteConversationStore;
    use crate::runtime::identity::ConversationId;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("previous.sqlite");
    let id = ConversationId::generate();
    let store = SqliteConversationStore::open(id.clone(), &path).unwrap();
    store
        .lock()
        .unwrap()
        .execute("UPDATE rustx_store SET schema_version=72", [])
        .unwrap();
    drop(store);
    assert!(matches!(
        SqliteConversationStore::open_existing(id, &path),
        Err(ConversationStoreError::SchemaVersionMismatch {
            stored: 72,
            expected: 73
        })
    ));
    let connection = Connection::open(path).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT schema_version FROM rustx_store", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        72
    );
}
