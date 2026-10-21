use super::*;
use crate::durable::sqlite::request::tests::fragment::shape::shape_snapshot;

// Fragment pages are type/size gated in SQLite and authenticated before decoding.
pub(super) fn assert_page_metadata_corruption(
    connection: &Connection,
    sequence: u64,
    parent: &[String],
    _target: &[String],
) {
    let read = || observation_request::page_at(connection, sequence, sequence, parent, 0, 1);
    let original = serde_json::to_value(read().unwrap()).unwrap();
    for mutation in ["bytes=zeroblob(2000000)", "bytes='[]'", "bytes=x'5b5d'"] {
        connection
            .execute_batch("SAVEPOINT corrupt_page_metadata; PRAGMA ignore_check_constraints=ON")
            .unwrap();
        assert!(
            connection
                .execute(&format!("UPDATE request_source_pages SET {mutation}"), [])
                .unwrap()
                > 0
        );
        observation_request::composed::take_work();
        assert!(
            matches!(read(), Err(ConversationStoreError::Storage(_))),
            "{mutation}"
        );
        let work = observation_request::composed::take_work();
        if mutation == "bytes=x'5b5d'" {
            assert_eq!(
                work.index_bytes, 2,
                "bounded corrupt bytes are counted before hash rejection"
            );
        } else {
            assert_eq!(
                work.index_bytes, 0,
                "oversized/type-invalid pages never reach a Rust buffer"
            );
        }
        assert!(work.max_source_metadata <= 65_536);
        connection.execute_batch("ROLLBACK TO corrupt_page_metadata; RELEASE corrupt_page_metadata; PRAGMA ignore_check_constraints=OFF").unwrap();
        assert_eq!(serde_json::to_value(read().unwrap()).unwrap(), original);
    }
}

#[test]
fn t08_t12_t19_page_metadata_is_type_and_size_gated_before_rust_allocation() {
    let store = fragment_fixture();
    let snapshot = shape_snapshot(&store, 1);
    let sequence = store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap()
        .started
        .sequence;
    let canonical = store.load_canonical().unwrap();
    let cut = store.observation_read_cut().unwrap();
    let connection = store.lock().unwrap();
    assert_page_metadata_corruption(
        &connection,
        sequence,
        &["messages".into()],
        &["messages".into(), "0".into()],
    );
    assert_page_metadata_corruption(
        &connection,
        sequence,
        &["tools".into()],
        &["tools".into(), "0".into()],
    );
    drop(connection);
    assert_eq!(store.load_canonical().unwrap(), canonical);
    assert_eq!(
        store.load_request_snapshot(&snapshot.request_id).unwrap(),
        snapshot
    );
    assert_eq!(store.observation_read_cut().unwrap(), cut);
    assert_eq!(store.presentation_frontier().unwrap(), sequence);
}
