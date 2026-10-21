use super::*;
use crate::durable::sqlite::request::tests::fragment::shape::assert_shape_optional_source_hole;
use crate::durable::sqlite::request::tests::fragment::shape::assert_shape_oracle;
use crate::durable::sqlite::request::tests::fragment::shape::shape_oracle;
use crate::durable::sqlite::request::tests::fragment::shape::shape_snapshot;

// A composed Source is published once under the original Request witness. Shared
// immutable subtrees are never rewritten when a later Request adds an item.
#[test]
fn t08_t14_t19_certified_request_roots_finalize_once_with_exact_legacy_geometry_and_witness() {
    let store = fragment_fixture();
    let updates = Arc::new(AtomicUsize::new(0));
    let observed = updates.clone();
    {
        let connection = store.lock().unwrap();
        connection
            .create_scalar_function(
                "test_source_update",
                0,
                rusqlite::functions::FunctionFlags::SQLITE_UTF8,
                move |_| {
                    observed.fetch_add(1, Ordering::SeqCst);
                    Ok(0_i64)
                },
            )
            .unwrap();
        connection.execute_batch("CREATE TEMP TRIGGER count_source_update AFTER UPDATE ON request_source_nodes BEGIN SELECT test_source_update(); END;").unwrap();
    }
    let mut retained = Vec::new();
    for turn in 1..=2 {
        if turn == 2 {
            store
                .append_canonical(&user_message(
                    &"\u{1}".repeat(3_000),
                    "oversized original identity remains exact and deferred",
                ))
                .unwrap();
        }
        let snapshot = shape_snapshot(&store, turn);
        let sequence = store
            .commit_model_turn_start(&[], &snapshot, Utc::now())
            .unwrap()
            .started
            .sequence;
        let expected = shape_oracle(&store, &snapshot, sequence);
        assert_shape_oracle(&store, &snapshot, sequence, &expected);
        assert_eq!(
            store.load_request_snapshot(&snapshot.request_id).unwrap(),
            snapshot
        );
        let connection = store.lock().unwrap();
        let seats: u64 = connection.query_row("SELECT count(*) FROM request_observation_fields WHERE source_id=?1 AND path_json='[]' AND length(shape_witness)=32", [sequence.to_string()], |r| r.get(0)).unwrap();
        assert_eq!(seats, 1, "one original Request witness seat");
        let roots: u64 = connection
            .query_row(
                "SELECT count(*) FROM request_source_roots WHERE source_id=?1",
                [sequence.to_string()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(roots, 1);
        drop(connection);
        retained.push((snapshot, sequence, expected));
    }
    assert_eq!(
        updates.load(Ordering::SeqCst),
        0,
        "immutable shared Source nodes are never rewritten"
    );
    for (snapshot, sequence, expected) in retained {
        assert_shape_oracle(&store, &snapshot, sequence, &expected);
    }
}

#[test]
fn t10_t14_t19_certified_root_finalization_failure_rolls_back_only_optional_source() {
    let store = fragment_fixture();
    let canonical = store.load_canonical().unwrap();
    let before = store.presentation_frontier().unwrap();
    let reached = Arc::new(AtomicUsize::new(0));
    let observed = reached.clone();
    {
        let connection = store.lock().unwrap();
        connection
            .create_scalar_function(
                "test_refuse_source_root",
                0,
                rusqlite::functions::FunctionFlags::SQLITE_UTF8,
                move |_| {
                    observed.fetch_add(1, Ordering::SeqCst);
                    Ok(0_i64)
                },
            )
            .unwrap();
        connection.execute_batch("CREATE TEMP TRIGGER refuse_request_root BEFORE INSERT ON request_source_roots BEGIN SELECT test_refuse_source_root(); SELECT RAISE(FAIL,'optional Source root'); END;").unwrap();
    }
    let snapshot = shape_snapshot(&store, 1);
    let started = store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap()
        .started;
    assert_eq!(
        reached.load(Ordering::SeqCst),
        1,
        "actual final Source insert reached the fault"
    );
    assert_eq!(started.sequence, before + 1);
    assert_eq!(
        store.load_request_snapshot(&snapshot.request_id).unwrap(),
        snapshot
    );
    assert_eq!(store.load_canonical().unwrap(), canonical);
    assert_shape_optional_source_hole(&store, &snapshot, started.sequence);
    {
        let connection = store.lock().unwrap();
        for table in [
            "request_observation_fields",
            "request_source_roots",
            "request_source_nodes",
            "request_source_pages",
        ] {
            assert_eq!(
                connection
                    .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r
                        .get::<_, u64>(0))
                    .unwrap(),
                0,
                "optional layout rollback: {table}"
            );
        }
        connection
            .execute_batch("DROP TRIGGER refuse_request_root")
            .unwrap();
    }
    let next = shape_snapshot(&store, 2);
    let sequence = store
        .commit_model_turn_start(&[], &next, Utc::now())
        .unwrap()
        .started
        .sequence;
    assert_shape_oracle(
        &store,
        &next,
        sequence,
        &shape_oracle(&store, &next, sequence),
    );
}
