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
    drop(connection);
    append_validation::take_work();
    store.append_canonical(&user_message("c", "C")).unwrap();
    store.append_canonical(&user_message("d", "D")).unwrap();
    assert_eq!(append_validation::take_work(), (1, 1));
    let head = load_head(&store.conn.lock().unwrap()).unwrap();
    assert_eq!(head.revision.get(), 9);
    assert_eq!(
        head.active_message_ids,
        ["a", "b", "c", "d"].map(MessageId::new)
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

#[test]
fn append_certificate_reuses_only_committed_validation() {
    let store = store();
    store.initialize(&[]).unwrap();
    append_validation::take_work();
    store.append_canonical(&user_message("a", "A")).unwrap();
    assert_eq!(append_validation::take_work(), (1, 0));
    assert!(store.conn.append_validation.has_certificate());
    store
        .append_canonical_batch(&[user_message("b", "B"), user_message("c", "C")])
        .unwrap();
    assert_eq!(append_validation::take_work(), (0, 2));
    assert!(store.conn.append_validation.has_certificate());
    assert!(
        store
            .append_canonical_batch(&[user_message("d", "D"), user_message("c", "duplicate")])
            .is_err()
    );
    assert_eq!(append_validation::take_work(), (0, 2));
    assert!(!store.conn.append_validation.has_certificate());
    assert!(!ledger_message_exists(&store.conn.lock().unwrap(), &MessageId::new("d")).unwrap());
    store.append_canonical(&user_message("e", "E")).unwrap();
    assert_eq!(append_validation::take_work(), (1, 0));
    assert_eq!(
        load_head(&store.conn.lock().unwrap())
            .unwrap()
            .active_message_ids,
        ["a", "b", "c", "e"].map(MessageId::new)
    );
}

#[test]
fn append_certificate_tracks_identity_mutation_and_ignores_derived_columns() {
    let store = store();
    store.initialize(&[]).unwrap();
    store.append_canonical(&user_message("a", "A")).unwrap();
    append_validation::take_work();
    store
        .conn
        .lock()
        .unwrap()
        .execute(
            "UPDATE message_ledger SET opening_preview='derived' WHERE message_id='a'",
            [],
        )
        .unwrap();
    store.append_canonical(&user_message("b", "B")).unwrap();
    assert_eq!(append_validation::take_work(), (0, 1));
    store.conn.lock().unwrap().execute_batch("PRAGMA foreign_keys=OFF; UPDATE message_ledger SET message_id=CAST(x'ff' AS TEXT) WHERE message_id='a'; PRAGMA foreign_keys=ON").unwrap();
    assert!(store.append_canonical(&user_message("c", "C")).is_err());
    assert_eq!(append_validation::take_work(), (1, 0));
    assert!(!store.conn.append_validation.has_certificate());
    assert!(load_head(&store.conn.lock().unwrap()).is_err());
}

#[test]
fn append_certificate_epoch_exhaustion_fails_closed() {
    let store = store();
    store.initialize(&[]).unwrap();
    store.append_canonical(&user_message("a", "A")).unwrap();
    append_validation::take_work();
    store.conn.append_validation.exhaust_epoch();
    for id in ["b", "c"] {
        store.append_canonical(&user_message(id, id)).unwrap();
        assert_eq!(append_validation::take_work(), (1, 0));
        assert!(!store.conn.append_validation.has_certificate());
    }
}

#[test]
fn append_certificate_preserves_new_identity_error_timing() {
    let store = store();
    store.initialize(&[]).unwrap();
    store.append_canonical(&user_message("a", "A")).unwrap();
    append_validation::take_work();
    let wide = "w".repeat(4097);
    store
        .append_canonical(&user_message(&wide, "wide"))
        .unwrap();
    assert_eq!(append_validation::take_work(), (0, 1));
    assert!(!store.conn.append_validation.has_certificate());
    assert!(store.append_canonical(&user_message("b", "B")).is_err());
    assert_eq!(append_validation::take_work(), (1, 0));
    assert!(ledger_message_exists(&store.conn.lock().unwrap(), &MessageId::new(wide)).unwrap());
}

#[test]
fn append_certificate_checks_external_writers_and_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("external.sqlite");
    let id = ConversationId::generate();
    let store = SqliteConversationStore::open(id.clone(), &path).unwrap();
    store.initialize(&[]).unwrap();
    store.append_canonical(&user_message("a", "A")).unwrap();
    append_validation::take_work();
    let external = Connection::open(&path).unwrap();
    external
        .execute("UPDATE message_ledger SET opening_preview='external'", [])
        .unwrap();
    store.append_canonical(&user_message("b", "B")).unwrap();
    assert_eq!(append_validation::take_work(), (1, 0));
    store.append_canonical(&user_message("c", "C")).unwrap();
    assert_eq!(append_validation::take_work(), (0, 1));
    // A writer between COMMIT and seating is detected inside the next TX.
    let mut connection = store.conn.lock().unwrap();
    let mut validation = append_validation::Batch::begin(&store.conn.append_validation);
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    validation
        .append(&transaction, &user_message("d", "D"), None)
        .unwrap();
    transaction.commit().unwrap();
    external
        .execute(
            "UPDATE message_ledger SET opening_preview='between commit and seat'",
            [],
        )
        .unwrap();
    validation.committed(&connection);
    drop(connection);
    assert_eq!(append_validation::take_work(), (0, 1));
    store.append_canonical(&user_message("e", "E")).unwrap();
    assert_eq!(append_validation::take_work(), (1, 0));
    drop(external);
    drop(store);
    let reopened = SqliteConversationStore::open(id, &path).unwrap();
    reopened.append_canonical(&user_message("f", "F")).unwrap();
    assert_eq!(append_validation::take_work(), (1, 0));
}

#[test]
fn append_certificate_checks_schema_versions() {
    let store = store();
    store.initialize(&[]).unwrap();
    store.append_canonical(&user_message("a", "A")).unwrap();
    append_validation::take_work();
    store
        .conn
        .lock()
        .unwrap()
        .execute_batch("CREATE TEMP TABLE certificate_temp(value TEXT)")
        .unwrap();
    store.append_canonical(&user_message("b", "B")).unwrap();
    assert_eq!(append_validation::take_work(), (1, 0));
    store.append_canonical(&user_message("c", "C")).unwrap();
    assert_eq!(append_validation::take_work(), (0, 1));
    store
        .conn
        .lock()
        .unwrap()
        .execute_batch("CREATE TABLE certificate_main(value TEXT)")
        .unwrap();
    for id in ["d", "e"] {
        store.append_canonical(&user_message(id, id)).unwrap();
        assert_eq!(append_validation::take_work(), (1, 0));
        assert!(!store.conn.append_validation.has_certificate());
    }
}

#[test]
fn append_certificate_rechecks_schema_and_mutations_inside_batch() {
    let store = store();
    store.initialize(&[]).unwrap();
    store.append_canonical(&user_message("a", "A")).unwrap();
    append_validation::take_work();
    let mut connection = store.conn.lock().unwrap();
    connection.execute_batch("PRAGMA foreign_keys=OFF").unwrap();
    let mut validation = append_validation::Batch::begin(&store.conn.append_validation);
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    validation
        .append(&transaction, &user_message("b", "B"), None)
        .unwrap();
    assert_eq!(append_validation::take_work(), (0, 1));
    assert!(!store.conn.append_validation.has_certificate());
    transaction
        .execute_batch("CREATE TEMP TABLE certificate_batch(value TEXT)")
        .unwrap();
    validation
        .append(&transaction, &user_message("c", "C"), None)
        .unwrap();
    assert_eq!(append_validation::take_work(), (1, 0));
    transaction
        .execute("DELETE FROM message_ledger WHERE message_id='a'", [])
        .unwrap();
    validation
        .append(&transaction, &user_message("d", "D"), None)
        .unwrap();
    assert_eq!(append_validation::take_work(), (1, 0));
    transaction.rollback().unwrap();
    validation.committed(&connection);
    assert!(
        !store.conn.append_validation.has_certificate(),
        "rollback epoch prevents seating provisional evidence"
    );
    assert_eq!(
        load_head(&connection).unwrap().active_message_ids,
        vec![MessageId::new("a")]
    );
}

#[test]
fn append_certificate_rejects_trigger_changed_identity_on_next_append() {
    let store = store();
    store.initialize(&[]).unwrap();
    store.append_canonical(&user_message("a", "A")).unwrap();
    store.conn.lock().unwrap().execute_batch("PRAGMA foreign_keys=OFF; CREATE TEMP TRIGGER corrupt_certificate_identity AFTER INSERT ON main.message_ledger WHEN new.message_id='b' BEGIN UPDATE message_ledger SET message_id=CAST(x'ff' AS TEXT) WHERE message_id='a'; END").unwrap();
    append_validation::take_work();
    store.append_canonical(&user_message("b", "B")).unwrap();
    assert_eq!(append_validation::take_work(), (1, 0));
    assert!(!store.conn.append_validation.has_certificate());
    assert!(store.append_canonical(&user_message("c", "C")).is_err());
    assert_eq!(append_validation::take_work(), (1, 0));
}

#[test]
fn append_certificate_preserves_checkpoint_original_dependencies() {
    use crate::context::checkpoint::{CheckpointItem, CheckpointMessage};
    use crate::scripted_suites::support::context::{checkpoint_commit_fixture, checkpoint_fixture};
    let store = store();
    let original = user_message("a", "A");
    store
        .initialize(&[original.clone(), user_message("b", "B")])
        .unwrap();
    store.append_canonical(&user_message("c", "C")).unwrap();
    append_validation::take_work();
    let mut candidate = checkpoint_fixture(&store, "certificate-checkpoint", 1, "unused");
    // The candidate covers the full current frontier; select only Original a.
    assert_eq!(candidate.base_frontier.0, 3);
    candidate.summary_metadata = None;
    candidate.items = vec![CheckpointItem {
        message: CheckpointMessage::Original(crate::model::projected::CanonicalMessageRef::of(
            store.conversation_id(),
            &original,
        )),
        call_sources: vec![],
        result_call: None,
    }];
    store
        .commit_projection(checkpoint_commit_fixture(&store, candidate, None))
        .unwrap();
    store.append_canonical(&user_message("d", "D")).unwrap();
    assert_eq!(append_validation::take_work(), (1, 0));
    store.append_canonical(&user_message("e", "E")).unwrap();
    assert_eq!(append_validation::take_work(), (0, 1));
    assert_eq!(
        load_head(&store.conn.lock().unwrap())
            .unwrap()
            .active_message_ids,
        ["a", "d", "e"].map(MessageId::new)
    );
    store.conn.lock().unwrap().execute_batch("PRAGMA foreign_keys=OFF; DELETE FROM message_ledger WHERE message_id='a'; PRAGMA foreign_keys=ON").unwrap();
    assert!(store.append_canonical(&user_message("f", "F")).is_err());
    assert!(!store.conn.append_validation.has_certificate());
    assert!(!ledger_message_exists(&store.conn.lock().unwrap(), &MessageId::new("f")).unwrap());
}

#[test]
fn append_certificate_does_not_certify_trigger_replacement() {
    let store = store();
    store.initialize(&[]).unwrap();
    store.append_canonical(&user_message("a", "A")).unwrap();
    let mut connection = store.conn.lock().unwrap();
    connection.execute_batch("PRAGMA foreign_keys=OFF; CREATE TEMP TRIGGER replace_certificate_identity BEFORE INSERT ON main.message_ledger WHEN new.message_id='b' BEGIN INSERT INTO message_ledger(position,message_id,message_json) VALUES(new.position,CAST(x'ff' AS TEXT),new.message_json); SELECT RAISE(IGNORE); END").unwrap();
    append_validation::take_work();
    let mut validation = append_validation::Batch::begin(&store.conn.append_validation);
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    // Exercise the native writer directly: optional receipt construction must
    // not hide a certificate error behind an unrelated malformed-ID rejection.
    validation
        .append(&transaction, &user_message("b", "B"), None)
        .unwrap();
    transaction.commit().unwrap();
    validation.committed(&connection);
    assert!(!store.conn.append_validation.has_certificate());
    drop(connection);
    assert!(store.append_canonical(&user_message("c", "C")).is_err());
    assert_eq!(append_validation::take_work(), (2, 0));
}
