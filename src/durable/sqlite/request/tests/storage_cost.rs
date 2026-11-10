//! Physical and VM-work bounds for the request family's storage layout.
use super::*;
use rusqlite::StatementStatus;

fn request_layout(history: u64) -> (u64, i32, i32) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("requests.sqlite");
    let mut connection = Connection::open(&path).unwrap();
    configure_connection(&mut connection, false).unwrap();
    create_schema(&mut connection).unwrap();
    // These are physical-layout fixtures, not Journal facts. Native admission
    // and corruption contracts are exercised by the adjacent request tests.
    connection
        .execute_batch("PRAGMA foreign_keys=OFF; PRAGMA wal_autocheckpoint=0")
        .unwrap();
    let transaction = connection.transaction().unwrap();
    for sequence in 1..=history {
        transaction.execute("INSERT INTO request_owners(request_id,owner_kind,attempt_id,turn_id,retry_number,start_sequence) VALUES(?1,'agent','layout-attempt','layout-turn',0,?2)",params![format!("request-{sequence:08}"),sequence]).unwrap();
        transaction.execute("INSERT INTO request_snapshots(started_sequence,canonical_frontier,manifest_digest,input_count) VALUES(?1,0,?2,0)", params![sequence,"0".repeat(64)]).unwrap();
        transaction.execute("INSERT INTO request_snapshot_payloads(started_sequence,snapshot_json) VALUES(?1,CAST(zeroblob(8192) AS TEXT))",[sequence]).unwrap();
    }
    transaction.commit().unwrap();
    let payload_before: Vec<(u64, u64)> = connection.prepare("SELECT pageno,payload FROM dbstat WHERE name='request_snapshot_payloads' ORDER BY pageno").unwrap().query_map([], |row| Ok((row.get(0)?,row.get(1)?))).unwrap().collect::<Result<_,_>>().unwrap();
    connection
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
        .unwrap();
    let update_steps = {
        let mut statement = connection
            .prepare(crate::durable::sqlite::request::COMPLETE_REQUEST_SQL)
            .unwrap();
        assert_eq!(
            statement
                .execute(params![history + 1, format!("request-{history:08}")])
                .unwrap(),
            1
        );
        assert_eq!(statement.get_status(StatementStatus::FullscanStep), 0);
        statement.get_status(StatementStatus::VmStep)
    };
    let wal = std::fs::read(directory.path().join("requests.sqlite-wal")).unwrap();
    let frames = (wal.len() - 32) / 2072;
    let payload_pages: std::collections::BTreeSet<_> =
        payload_before.iter().map(|(page, _)| *page).collect();
    for frame in wal[32..].as_chunks::<2072>().0 {
        let page = u64::from(u32::from_be_bytes(frame[..4].try_into().unwrap()));
        assert!(
            !payload_pages.contains(&page),
            "lifecycle rewrote immutable payload page {page}"
        );
    }
    let read_steps = {
        let mut statement = connection
            .prepare(crate::durable::sqlite::request::SNAPSHOT_HEADERS_SQL)
            .unwrap();
        let headers = statement
            .query_map(params![history - 8, 8], |row| row.get::<_, u64>(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(headers.len(), 8);
        assert_eq!(headers.last(), Some(&history));
        assert_eq!(statement.get_status(StatementStatus::FullscanStep), 0);
        assert_eq!(statement.get_status(StatementStatus::Sort), 0);
        statement.get_status(StatementStatus::VmStep)
    };
    let payload_after: Vec<(u64, u64)> = connection.prepare("SELECT pageno,payload FROM dbstat WHERE name='request_snapshot_payloads' ORDER BY pageno").unwrap().query_map([], |row| Ok((row.get(0)?,row.get(1)?))).unwrap().collect::<Result<_,_>>().unwrap();
    assert_eq!(payload_before, payload_after);
    (frames as u64, update_steps, read_steps)
}

#[test]
fn integer_request_lifecycle_writes_and_lookups_are_bounded_by_the_delta() {
    let work: Vec<_> = [8, 64, 512, 4096].into_iter().map(request_layout).collect();
    eprintln!(
        "request layout H=8/64/512/4096 (WAL frames, update VM steps, lookup VM steps): {work:?}"
    );
    for &(frames, update, read) in &work {
        // One small lifecycle row. Leaf splitting can add bounded local pages;
        // it never copies the payload's overflow chain or scans old requests.
        assert!(frames <= 4, "{work:?}");
        assert_eq!((update, read), (work[0].1, work[0].2));
    }
}

#[test]
fn numeric_request_start_is_bound_once_and_its_owner_fk_is_deferred() {
    let store = store();
    store.initialize(&[]).unwrap();
    store.lock().unwrap().execute_batch("CREATE TEMP TRIGGER forbid_start_rebinding BEFORE UPDATE OF started_sequence ON request_snapshots BEGIN SELECT RAISE(ABORT,'request identity rebound'); END; CREATE TEMP TRIGGER forbid_payload_rewrite BEFORE UPDATE ON request_snapshot_payloads BEGIN SELECT RAISE(ABORT,'immutable payload updated'); END;").unwrap();
    let snapshot = test_request_snapshot(store.load_head().unwrap().revision, invocation());
    let committed = store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap();
    assert_eq!(
        store.load_request_snapshot(&snapshot.request_id).unwrap(),
        snapshot
    );
    let mut terminal = envelope(
        store.conversation_id(),
        "integer-request-completed",
        Some(snapshot.identity.attempt_id.clone()),
        RuntimeEvent::ModelRequestCompleted {
            owner: ModelRequestOwner::Agent {
                identity: snapshot.identity.clone(),
            },
            request_id: snapshot.request_id.clone(),
            finish_reason: ModelFinishReason::Stop,
            usage: None,
            generation: None,
        },
    );
    terminal.turn_id = Some(snapshot.identity.turn.clone());
    store.append_event(terminal).unwrap();
    assert_eq!(
        store.load_request_snapshot(&snapshot.request_id).unwrap(),
        snapshot
    );
    let mut connection = store.lock().unwrap();
    let sequence: u64 = connection.query_row("SELECT s.rowid FROM request_snapshots s JOIN request_owners o ON o.start_sequence=s.started_sequence WHERE o.request_id=?1",[snapshot.request_id.as_str()],|row|row.get(0)).unwrap();
    assert_eq!(sequence, committed.started.sequence);
    let transaction = connection.transaction().unwrap();
    transaction.execute("INSERT INTO request_owners(request_id,owner_kind,attempt_id,turn_id,retry_number,start_sequence) VALUES('orphan','agent','attempt','turn',0,999999)",[]).unwrap();
    assert!(
        transaction.commit().is_err(),
        "a staged owner must never commit without its Journal start"
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT count(*) FROM request_owners WHERE request_id='orphan'",
                [],
                |row| row.get::<_, usize>(0)
            )
            .unwrap(),
        0
    );
}

#[test]
fn a_trigger_cannot_reassign_the_staged_request_start_sequence() {
    for table in ["request_owners", "request_snapshots"] {
        let store = store();
        store.initialize(&[]).unwrap();
        store.lock().unwrap().execute_batch(&format!(
            "CREATE TEMP TRIGGER change_start_frontier AFTER INSERT ON {table} BEGIN UPDATE rustx_store SET next_event_sequence=next_event_sequence+1 WHERE id=1; END;"
        )).unwrap();
        let snapshot = test_request_snapshot(store.load_head().unwrap().revision, invocation());
        assert!(
            store
                .commit_model_turn_start(&[], &snapshot, Utc::now())
                .is_err()
        );
        assert_eq!(store.event_high_watermark().unwrap(), 0);
        let connection = store.lock().unwrap();
        for authority in [
            "events",
            "request_owners",
            "request_snapshots",
            "request_snapshot_payloads",
            "request_configurations",
        ] {
            assert_eq!(
                connection
                    .query_row(&format!("SELECT count(*) FROM {authority}"), [], |row| row
                        .get::<_, u64>(
                        0
                    ))
                    .unwrap(),
                0,
                "{table}: {authority}"
            );
        }
    }
}

#[test]
fn an_unsettled_request_never_dirties_the_terminal_index() {
    let directory = tempfile::tempdir().unwrap();
    let store = SqliteConversationStore::open(
        ConversationId::generate(),
        &directory.path().join("terminal.sqlite"),
    )
    .unwrap();
    store.initialize(&[]).unwrap();
    let root: u32 = {
        let connection = store.lock().unwrap();
        connection
            .execute_batch("PRAGMA wal_autocheckpoint=0; PRAGMA wal_checkpoint(TRUNCATE)")
            .unwrap();
        connection
            .query_row(
                "SELECT rootpage FROM sqlite_schema WHERE name='request_owners_terminal_idx'",
                [],
                |row| row.get(0),
            )
            .unwrap()
    };
    let pages = || {
        let wal = std::fs::read(directory.path().join("terminal.sqlite-wal")).unwrap();
        wal[32..]
            .as_chunks::<2072>()
            .0
            .iter()
            .map(|frame| u32::from_be_bytes(frame[..4].try_into().unwrap()))
            .collect::<Vec<_>>()
    };
    let snapshot = test_request_snapshot(store.load_head().unwrap().revision, invocation());
    store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap();
    let started = pages();
    assert!(
        !started.contains(&root),
        "NULL terminal caused an index write"
    );
    let mut terminal = envelope(
        store.conversation_id(),
        "partial-terminal",
        Some(snapshot.identity.attempt_id.clone()),
        RuntimeEvent::ModelRequestCompleted {
            owner: ModelRequestOwner::Agent {
                identity: snapshot.identity.clone(),
            },
            request_id: snapshot.request_id.clone(),
            finish_reason: ModelFinishReason::Stop,
            usage: None,
            generation: None,
        },
    );
    terminal.turn_id = Some(snapshot.identity.turn.clone());
    store.append_event(terminal).unwrap();
    assert_eq!(
        pages()[started.len()..]
            .iter()
            .filter(|page| **page == root)
            .count(),
        1
    );
}
