#[test]
fn append_frontier_matches_materialized_head_with_sparse_positions() {
    let store = store();
    store
        .initialize(&[user_message("a", "A"), user_message("b", "B")])
        .unwrap();
    let connection = store.conn.lock().unwrap();
    connection.execute_batch("PRAGMA foreign_keys=OFF; UPDATE message_ledger SET position=7 WHERE position=2; PRAGMA foreign_keys=ON").unwrap();
    connection
        .execute("UPDATE surface_head SET revision=7", [])
        .unwrap();
    let full = load_head(&connection).unwrap();
    assert_eq!(
        append_head(&connection).unwrap(),
        (full.revision, full.compaction_generation)
    );
    assert_eq!(
        full.active_message_ids,
        vec![MessageId::new("a"), MessageId::new("b")]
    );
}

#[test]
fn append_frontier_rejects_corrupt_identity_bytes_and_rolls_back() {
    for expression in ["zeroblob(4097)", "x'ff'", "CAST(x'ff' AS TEXT)"] {
        let store = store();
        store.initialize(&[user_message("a", "A")]).unwrap();
        {
            let connection = store.conn.lock().unwrap();
            connection.execute_batch(&format!("PRAGMA foreign_keys=OFF; UPDATE message_ledger SET message_id={expression}; PRAGMA foreign_keys=ON")).unwrap();
            assert!(load_head(&connection).is_err(), "{expression}");
            assert!(append_head(&connection).is_err(), "{expression}");
        }
        assert!(store.append_canonical(&user_message("b", "B")).is_err());
        let connection = store.conn.lock().unwrap();
        let counts: (i64,i64,i64) = connection.query_row("SELECT (SELECT count(*) FROM message_ledger),(SELECT count(*) FROM surface_ops),(SELECT revision FROM surface_head)", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
        assert_eq!(counts, (1, 1, 1));
    }
}

#[test]
fn append_frontier_retains_exact_working_set_boundary() {
    let store = store();
    store.initialize(&[]).unwrap();
    {
        let connection = store.conn.lock().unwrap();
        connection.execute_batch("WITH RECURSIVE ids(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM ids WHERE n<8192) INSERT INTO message_ledger(position,message_id,message_json) SELECT n,printf('id-%d',n),'{}' FROM ids; UPDATE surface_head SET revision=8192").unwrap();
        assert_eq!(
            load_head(&connection).unwrap().active_message_ids.len(),
            8192
        );
        assert_eq!(append_head(&connection).unwrap().0.get(), 8192);
    }
    // The pre-append bound historically permits this last fact. The next
    // append must fail atomically, rather than silently dropping older ids.
    store
        .append_canonical(&user_message("last", "last"))
        .unwrap();
    assert!(
        store
            .append_canonical(&user_message("excess", "excess"))
            .is_err()
    );
    let connection = store.conn.lock().unwrap();
    assert!(load_head(&connection).is_err());
    assert!(!ledger_message_exists(&connection, &MessageId::new("excess")).unwrap());
    assert_eq!(
        connection
            .query_row("SELECT revision FROM surface_head", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        8193
    );
}

#[test]
fn append_frontier_retains_inconsistent_revision_rejection() {
    let store = store();
    store.initialize(&[user_message("a", "A")]).unwrap();
    {
        let connection = store.conn.lock().unwrap();
        connection
            .execute("UPDATE surface_head SET revision=2", [])
            .unwrap();
        assert!(load_head(&connection).is_err());
        assert!(append_head(&connection).is_err());
    }
    assert!(store.append_canonical(&user_message("b", "B")).is_err());
    assert!(!ledger_message_exists(&store.conn.lock().unwrap(), &MessageId::new("b")).unwrap());
}

#[test]
fn current_gate_rejects_retired_surface_head_layout() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("retired.sqlite");
    let id = ConversationId::generate();
    {
        let store = SqliteConversationStore::open(id.clone(), &path).unwrap();
        store
            .conn
            .lock()
            .unwrap()
            .execute(
                "ALTER TABLE surface_head ADD COLUMN active_json TEXT NOT NULL DEFAULT '[]'",
                [],
            )
            .unwrap();
    }
    assert!(
        matches!(SqliteConversationStore::open_existing(id, &path), Err(ConversationStoreError::IncompatibleSchema(detail)) if detail.contains("retired active_json"))
    );
}
