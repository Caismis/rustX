// A composed Source is published once for its committed Request identity. Shared
// immutable subtrees are never rewritten when a later Request adds an item.
#[test]
fn t08_t14_t19_request_roots_finalize_once_with_exact_legacy_geometry_and_checksum() {
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
        let seats: u64 = connection.query_row("SELECT count(*) FROM request_source_roots WHERE source_id=?1 AND length(checksum)=32", [sequence.to_string()], |r| r.get(0)).unwrap();
        assert_eq!(seats, 1, "one committed Request Source root");
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

#[test]
fn reused_request_source_rejects_invalid_existing_rows_without_rebuilding_them() {
    for damage in ["hash_type", "hash_length", "native_digest", "metadata"] {
        let store = store();
        store
            .initialize(&[user_message("reused-source", "original")])
            .unwrap();
        let first = fragment_snapshot(&store, 1, 0);
        store
            .commit_model_turn_start(&[], &first, Utc::now())
            .unwrap();
        let canonical = store.load_canonical().unwrap();
        let (root, original) = {
            let connection = store.lock().unwrap();
            let root: u64 = connection
                .query_row(
                    "SELECT input_root FROM request_snapshots WHERE request_id=?1",
                    [first.request_id.as_str()],
                    |row| row.get(0),
                )
                .unwrap();
            let original: (Vec<u8>, Vec<u8>, Vec<u8>) = connection.query_row(
                "SELECT native_digest,hash,metadata FROM request_source_nodes WHERE native_id=?1",
                [root], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            ).unwrap();
            connection
                .execute_batch("PRAGMA ignore_check_constraints=ON")
                .unwrap();
            let mutation = match damage {
                "hash_type" => "hash='not bytes'",
                "hash_length" => "hash=zeroblob(33)",
                "native_digest" => "native_digest=zeroblob(32)",
                "metadata" => "metadata=x'7b'",
                _ => unreachable!(),
            };
            connection
                .execute(
                    &format!("UPDATE request_source_nodes SET {mutation} WHERE native_id=?1"),
                    [root],
                )
                .unwrap();
            connection
                .execute_batch("PRAGMA ignore_check_constraints=OFF")
                .unwrap();
            (root, original)
        };
        let next = fragment_snapshot(&store, 2, 0);
        let sequence = store
            .commit_model_turn_start(&[], &next, Utc::now())
            .unwrap()
            .started
            .sequence;
        assert_shape_optional_source_hole(&store, &next, sequence);
        assert_eq!(store.load_request_snapshot(&next.request_id).unwrap(), next);
        assert_eq!(store.load_canonical().unwrap(), canonical);
        // A failed lookup cannot erase or overwrite the existing row. Restoring
        // that exact immutable row makes a later request readable again.
        {
            let connection = store.lock().unwrap();
            let unchanged: bool = connection.query_row(
                "SELECT NOT (native_digest=?2 AND hash=?3 AND metadata=?4) FROM request_source_nodes WHERE native_id=?1",
                params![root, original.0, original.1, original.2], |row| row.get(0),
            ).unwrap();
            assert!(unchanged, "{damage}: corrupt row must not be rebuilt");
            connection.execute(
                "UPDATE request_source_nodes SET native_digest=?2,hash=?3,metadata=?4 WHERE native_id=?1",
                params![root, original.0, original.1, original.2],
            ).unwrap();
        }
        let repaired = fragment_snapshot(&store, 3, 0);
        let sequence = store
            .commit_model_turn_start(&[], &repaired, Utc::now())
            .unwrap()
            .started
            .sequence;
        assert_shape_oracle(
            &store,
            &repaired,
            sequence,
            &shape_oracle(&store, &repaired, sequence),
        );
    }
}

#[test]
fn missing_request_source_row_is_captured_for_the_next_request() {
    let store = store();
    store
        .initialize(&[user_message("missing-source", "original")])
        .unwrap();
    let first = fragment_snapshot(&store, 1, 0);
    store
        .commit_model_turn_start(&[], &first, Utc::now())
        .unwrap();
    {
        let connection = store.lock().unwrap();
        assert_eq!(connection.execute(
            "DELETE FROM request_source_nodes WHERE native_id=(SELECT input_root FROM request_snapshots WHERE request_id=?1)",
            [first.request_id.as_str()],
        ).unwrap(), 1);
    }
    let next = fragment_snapshot(&store, 2, 0);
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

#[test]
fn new_request_source_nodes_reuse_construction_after_actual_storage_checks() {
    let store = fragment_fixture();
    let first = fragment_snapshot(&store, 1, 0);
    observation_request::composed::take_work();
    let sequence = store
        .commit_model_turn_start(&[], &first, Utc::now())
        .unwrap()
        .started
        .sequence;
    let work = observation_request::composed::take_work();
    assert_eq!(work.new_leaves, first.input.len());
    assert_eq!(
        work.node_reuses,
        work.new_nodes - 1,
        "every newly captured child, excluding the root, reuses its decoded construction"
    );
    assert!(work.template_reuses >= work.new_leaves);
    assert_eq!(work.template_decodes, 0);
    assert!(
        work.node_decodes > 0,
        "final root reads still inspect persisted evidence"
    );
    assert_fragment_oracle(&store, &first, sequence);
    store
        .append_canonical(&user_message("capture-new-tail", "new original 水🙂"))
        .unwrap();
    let next = fragment_snapshot(&store, 2, 0);
    observation_request::composed::take_work();
    let sequence = store
        .commit_model_turn_start(&[], &next, Utc::now())
        .unwrap()
        .started
        .sequence;
    let work = observation_request::composed::take_work();
    assert_eq!(work.new_leaves, 1);
    assert!(work.node_reuses > 0);
    assert_eq!(work.template_decodes, 0);
    assert_fragment_oracle(&store, &next, sequence);
}

#[test]
fn prepared_request_children_still_detect_later_sibling_trigger_mutation() {
    for native in [false, true] {
        let store = fragment_fixture();
        let canonical = store.load_canonical().unwrap();
        {
            let connection = store.lock().unwrap();
            if native {
                connection.execute_batch("CREATE TEMP TRIGGER rewrite_prior_native AFTER INSERT ON request_source_nodes WHEN EXISTS(SELECT 1 FROM request_source_nodes WHERE native_id<NEW.native_id) BEGIN UPDATE native_input_nodes SET digest=zeroblob(32) WHERE id=(SELECT min(native_id) FROM request_source_nodes); END;").unwrap();
            } else {
                connection.execute_batch("CREATE TEMP TRIGGER rewrite_prior_source AFTER INSERT ON request_source_nodes BEGIN UPDATE request_source_nodes SET metadata=x'7b' WHERE native_id<NEW.native_id; END;").unwrap();
            }
        }
        let snapshot = fragment_snapshot(&store, 1, 0);
        let sequence = store
            .commit_model_turn_start(&[], &snapshot, Utc::now())
            .unwrap()
            .started
            .sequence;
        assert_shape_optional_source_hole(&store, &snapshot, sequence);
        assert_eq!(
            store.load_request_snapshot(&snapshot.request_id).unwrap(),
            snapshot
        );
        assert_eq!(store.load_canonical().unwrap(), canonical);
        let connection = store.lock().unwrap();
        for table in [
            "request_source_roots",
            "request_source_nodes",
            "request_source_pages",
        ] {
            assert_eq!(
                connection
                    .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| row
                        .get::<_, u64>(0))
                    .unwrap(),
                0,
                "failed optional capture rolls back {table}"
            );
        }
    }
}
