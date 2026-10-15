fn request_root_update_counters(store: &SqliteConversationStore) -> Arc<[AtomicUsize; 2]> {
    let counters = Arc::new([AtomicUsize::new(0), AtomicUsize::new(0)]);
    let observed = counters.clone();
    let connection = store.lock().unwrap();
    connection
        .create_scalar_function(
            "test_request_root_update",
            1,
            rusqlite::functions::FunctionFlags::SQLITE_UTF8,
            move |context| {
                let target: usize = context.get(0)?;
                observed[target].fetch_add(1, Ordering::SeqCst);
                Ok(0_i64)
            },
        )
        .unwrap();
    connection.execute_batch(
        r#"CREATE TEMP TRIGGER count_request_root_update AFTER UPDATE ON request_observation_fields
        WHEN NEW.parent_path='["messages"]' BEGIN SELECT test_request_root_update(0); END;
        CREATE TEMP TRIGGER count_request_item_update AFTER UPDATE ON request_item_layouts
        BEGIN SELECT test_request_root_update(1); END;"#,
    ).unwrap();
    counters
}
#[test]
fn t08_t14_t19_certified_request_roots_finalize_once_with_exact_legacy_geometry_and_witness() {
    let store = fragment_fixture();
    let counters = request_root_update_counters(&store);
    let mut first_own = 0;
    for turn in 1..=2 {
        if turn == 2 {
            store
                .append_canonical(&user_message(
                    &"\u{1}".repeat(3_000),
                    "genuine oversized original retains its Own finalizer",
                ))
                .unwrap();
        }
        let snapshot = shape_snapshot(&store, turn);
        let before = counters
            .each_ref()
            .map(|counter| counter.load(Ordering::SeqCst));
        let started = store
            .commit_model_turn_start(&[], &snapshot, Utc::now())
            .unwrap()
            .started;
        let expected = snapshot.input.len();
        assert_eq!(
            counters[0].load(Ordering::SeqCst) - before[0],
            expected,
            "each original root row is updated once, including its witness"
        );
        assert_eq!(
            counters[1].load(Ordering::SeqCst) - before[1],
            expected,
            "certified item geometry is not written again by the field visitor"
        );
        let connection = store.lock().unwrap();
        let (shaped, own): (u64, u64) = connection.query_row(
            "SELECT sum(layout_kind='shape'),sum(layout_kind='own') FROM request_item_layouts WHERE source_id=?1",
            [started.sequence.to_string()], |row| Ok((row.get(0)?,row.get(1)?)),
        ).unwrap();
        assert!(shaped > 0);
        if turn == 1 {
            first_own = own;
        } else {
            assert_eq!(own, first_own + 1);
            let last_kind: String = connection.query_row(
                "SELECT layout_kind FROM request_item_layouts WHERE source_id=?1 ORDER BY item_ordinal DESC LIMIT 1",
                [started.sequence.to_string()], |row| row.get(0),
            ).unwrap();
            assert_eq!(
                last_kind, "own",
                "the actual oversized identity retains its Own finalizer"
            );
        }
        let mismatches: u64 = connection.query_row(
            "SELECT count(*) FROM request_item_layouts i JOIN request_observation_fields f ON f.source_id=i.source_id AND f.path_json=json_array('messages',CAST(i.item_ordinal AS TEXT)) WHERE i.source_id=?1 AND (f.start!=i.root_start OR f.end!=i.root_end OR f.children!=i.root_children OR (i.layout_kind='shape' AND f.shape_witness IS NULL))",
            [started.sequence.to_string()], |row| row.get(0),
        ).unwrap();
        assert_eq!(mismatches, 0);
        drop(connection);
        assert_shape_oracle(
            &store,
            &snapshot,
            started.sequence,
            &shape_oracle(&store, &snapshot, started.sequence),
        );
        assert_eq!(
            store.load_request_snapshot(&snapshot.request_id).unwrap(),
            snapshot
        );
    }
}
#[test]
fn t10_t14_t19_certified_root_finalization_failure_rolls_back_only_optional_source() {
    let store = fragment_fixture();
    let canonical = store.load_canonical().unwrap();
    let before = store.presentation_frontier().unwrap();
    let counters = request_root_update_counters(&store);
    store
        .lock()
        .unwrap()
        .execute_batch(
            r#"CREATE TEMP TRIGGER refuse_request_root BEFORE UPDATE ON request_observation_fields
        WHEN NEW.parent_path='["messages"]' AND NEW.start>0
        BEGIN SELECT test_request_root_update(0); SELECT RAISE(FAIL,'optional original root finalization'); END;"#,
        )
        .unwrap();
    let snapshot = shape_snapshot(&store, 1);
    let started = store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap()
        .started;
    assert_eq!(
        counters[0].load(Ordering::SeqCst),
        1,
        "the actual combined root update reached its fault"
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
            "request_item_layouts",
            "request_field_shapes",
        ] {
            assert_eq!(
                connection
                    .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| row
                        .get::<_, u64>(0))
                    .unwrap(),
                0
            );
        }
        connection
            .execute_batch("DROP TRIGGER refuse_request_root")
            .unwrap();
    }
    let next = shape_snapshot(&store, 2);
    let started = store
        .commit_model_turn_start(&[], &next, Utc::now())
        .unwrap()
        .started;
    assert_shape_oracle(
        &store,
        &next,
        started.sequence,
        &shape_oracle(&store, &next, started.sequence),
    );
}
