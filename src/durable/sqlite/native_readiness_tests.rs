use crate::durable::observation::NativeObservationBatch;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

fn canonical_batch(store: &SqliteConversationStore, label: &str) -> Arc<NativeObservationBatch> {
    let message = assistant_message(label, "released together");
    let event = envelope(
        store.conversation_id(),
        &format!("event-{label}"),
        None,
        RuntimeEvent::AssistantMessageCommitted {
            message_id: message.id().clone(),
        },
    );
    store
        .append_canonical_with_event(&message, event)
        .unwrap()
        .0
        .observation
        .unwrap()
}

fn readiness(store: &SqliteConversationStore, through: u64) -> (u64, u64, bool) {
    (
        store.observation_ready_frontier(0, through).unwrap(),
        store
            .observation_transcript_ready_frontier(0, through)
            .unwrap(),
        store
            .observation_event_witness(through, through)
            .unwrap()
            .unwrap()
            .native_direct,
    )
}

struct ReadyObserver {
    reader: Arc<SqliteConversationStore>,
    wakes: Mutex<Vec<(u64, u64, bool)>>,
}
impl super::super::super::presentation::JournalObserver for ReadyObserver {
    fn committed(&self, _through: Option<u64>) {}
    fn ready(&self, through: u64) {
        self.wakes
            .lock()
            .unwrap()
            .push(readiness(&self.reader, through));
    }
}

#[test]
fn native_readiness_commits_all_members_once_before_waking_readers() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("batch.sqlite");
    let id = ConversationId::new("conv_fccbe5e9-a68c-75fe-a134-d883c7b1888d");
    let store = SqliteConversationStore::open(id.clone(), &path).unwrap();
    store.initialize(&[]).unwrap();
    let reader = Arc::new(SqliteConversationStore::from_connection_owner(id, store.conn.clone()));
    let observer = Arc::new(ReadyObserver {
        reader: reader.clone(),
        wakes: Mutex::new(Vec::new()),
    });
    store.observe_journal(observer.clone()).unwrap();
    let batch = canonical_batch(&store, "first");
    assert_eq!(readiness(&reader, 1), (0, 0, false));
    let commits = Arc::new(AtomicUsize::new(0));
    let wal_path = directory.path().join("batch.sqlite-wal");
    let wal_before = std::fs::read(&wal_path).unwrap();
    store
        .lock()
        .unwrap()
        .commit_hook(Some({
            let commits = commits.clone();
            move || {
                commits.fetch_add(1, AtomicOrdering::Relaxed);
                false
            }
        }))
        .unwrap();
    store.mark_native_observation_ready(&batch).unwrap();
    store
        .lock()
        .unwrap()
        .commit_hook(None::<fn() -> bool>)
        .unwrap();
    assert_eq!(commits.load(AtomicOrdering::Relaxed), 1);
    assert_eq!(std::fs::read(&wal_path).unwrap(), wal_before, "installation only changes TEMP memory, never the main WAL");
    assert_eq!(*observer.wakes.lock().unwrap(), vec![(1, 1, true)]);
    assert_eq!(readiness(&reader, 1), (1, 1, true));
    assert_eq!(readiness(&store, 1), (1, 1, true));
}

#[test]
fn native_readiness_failure_rolls_back_release_and_reopen_recovers_history() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("batch.sqlite");
    let id = ConversationId::new("conv_fccbe5e9-a68c-75fe-a134-d883c7b1888e");
    let store = SqliteConversationStore::open(id.clone(), &path).unwrap();
    store.initialize(&[]).unwrap();
    let reader = Arc::new(SqliteConversationStore::from_connection_owner(id.clone(), store.conn.clone()));
    let observer = Arc::new(ReadyObserver {
        reader: reader.clone(),
        wakes: Mutex::new(Vec::new()),
    });
    store.observe_journal(observer.clone()).unwrap();
    let batch = canonical_batch(&store, "native-fact");
    // The internal recovery helper accepts (1, 0) for an empty Journal, but
    // owner publication must preserve the stricter public range contract.
    for range in [(1, 0), (0, 0), (0, 1), (2, 1)] {
        let mut malformed = (*batch).clone();
        malformed.journal_range = Some(range);
        assert!(matches!(
            store.mark_native_observation_ready(&malformed),
            Err(ConversationStoreError::InvalidReference(_))
        ));
        assert_eq!(readiness(&reader, 1), (0, 0, false));
        assert_eq!(readiness(&store, 1), (0, 0, false));
        assert!(observer.wakes.lock().unwrap().is_empty());
    }
    store.lock().unwrap().execute_batch(
        "CREATE TEMP TRIGGER fail_direct_release BEFORE UPDATE OF direct ON event_observation_readiness BEGIN SELECT RAISE(FAIL,'release failure'); END;"
    ).unwrap();
    assert!(store.mark_native_observation_ready(&batch).is_err());
    assert_eq!(readiness(&reader, 1), (0, 0, false));
    assert_eq!(readiness(&store, 1), (0, 0, false));
    assert!(observer.wakes.lock().unwrap().is_empty());
    assert_eq!(
        store.load_canonical().unwrap().len(),
        1,
        "native write survives its optional release failure"
    );
    store
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER fail_direct_release")
        .unwrap();
    drop(store);
    let reopened = SqliteConversationStore::open(id, &path).unwrap();
    assert_eq!(readiness(&reopened, 1), (1, 1, false));
    reopened.observe_journal(observer).unwrap();
    assert_eq!(
        readiness(&reopened, 1),
        (1, 1, false),
        "reinstall releases history without inventing direct publication"
    );
    assert_eq!(readiness(&reader, 1), (0, 0, false), "an independent connection cannot release the old live owner");
}

#[test]
fn native_readiness_repeated_direct_release_does_not_rewrite_rows() {
    let store = store();
    store.initialize(&[]).unwrap();
    let batch = canonical_batch(&store, "idempotent-release");
    store.mark_native_observation_ready(&batch).unwrap();
    let before = store.lock().unwrap().total_changes();
    store.mark_native_observation_ready(&batch).unwrap();
    assert_eq!(store.lock().unwrap().total_changes(), before);
    assert_eq!(readiness(&store, 1), (1, 1, true));
}

#[test]
fn native_readiness_reopen_uses_committed_baseline_without_history_rows_or_writes() {
    struct Observer;
    impl super::super::super::presentation::JournalObserver for Observer {
        fn committed(&self, _through: Option<u64>) {}
    }
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("epoch.sqlite");
    let id = ConversationId::new("conv_aadfb32a-0041-752d-90c7-82a2d1911e19");
    let store = SqliteConversationStore::open(id.clone(), &path).unwrap();
    store.initialize(&[]).unwrap();
    for ordinal in 0..64 {
        canonical_batch(&store, &format!("history-{ordinal}"));
    }
    assert_eq!(readiness(&store, 64), (0, 0, false));
    let wal_path = directory.path().join("epoch.sqlite-wal");
    let wal_before = std::fs::read(&wal_path).unwrap();
    // Management opens main read-only; TEMP epoch creation must still work.
    let reopened = SqliteConversationStore::open_existing(id, &path).unwrap();
    let counts = |store: &SqliteConversationStore| {
        store.lock().unwrap().query_row(
            "SELECT (SELECT COUNT(*) FROM temp.event_observation_readiness),(SELECT COUNT(*) FROM temp.transcript_observation_readiness),(SELECT COUNT(*) FROM main.sqlite_schema WHERE name IN('event_observation_readiness','transcript_observation_readiness'))",
            [], |row| Ok((row.get::<_,u64>(0)?,row.get::<_,u64>(1)?,row.get::<_,u64>(2)?)),
        ).unwrap()
    };
    assert_eq!(counts(&store), (64, 64, 0));
    assert_eq!(counts(&reopened), (0, 0, 0));
    assert_eq!(readiness(&reopened, 64), (64, 64, false));
    assert_eq!(std::fs::read(&wal_path).unwrap(), wal_before);
    {
        use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
        reopened.lock().unwrap().authorizer(Some(|context: AuthContext<'_>| {
            if matches!(context.action, AuthAction::Read { table_name: "events" | "transcript_order", .. }) {
                Authorization::Deny
            } else {
                Authorization::Allow
            }
        })).unwrap();
    }
    assert_eq!(reopened.observe_journal(Arc::new(Observer)).unwrap(), 64);
    reopened.lock().unwrap().authorizer(None::<fn(rusqlite::hooks::AuthContext<'_>) -> rusqlite::hooks::Authorization>).unwrap();
    assert_eq!(counts(&reopened), (0, 0, 0));
    assert_eq!(std::fs::read(&wal_path).unwrap(), wal_before);
    assert_eq!(readiness(&store, 64), (0, 0, false));
}

#[test]
fn native_readiness_source_deletion_and_rollback_share_the_connection_transaction() {
    let store = store();
    store.initialize(&[]).unwrap();
    let batch = canonical_batch(&store, "transactional-epoch");
    let mut connection = store.lock().unwrap();
    {
        let transaction = connection.transaction().unwrap();
        super::super::observation_readiness::mark_direct(&transaction, 1, 1).unwrap();
        super::super::observation_readiness::mark_canonical(&transaction, 1, 1).unwrap();
        assert_eq!(super::super::observation_readiness::frontier(&transaction, 0, 1).unwrap(), 1);
        // The owner's complete publication has not committed.
        transaction.rollback().unwrap();
    }
    drop(connection);
    assert_eq!(readiness(&store, 1), (0, 0, false));
    store.mark_native_observation_ready(&batch).unwrap();
    let mut connection = store.lock().unwrap();
    {
        let transaction = connection.transaction().unwrap();
        transaction.execute("DELETE FROM transcript_order WHERE position=1", []).unwrap();
        let count: u64 = transaction.query_row("SELECT COUNT(*) FROM transcript_observation_readiness", [], |row| row.get(0)).unwrap();
        assert_eq!(count, 0, "TEMP cleanup follows the native T owner deletion");
        transaction.rollback().unwrap();
    }
    drop(connection);
    assert_eq!(readiness(&store, 1), (1, 1, true));
}

#[test]
fn native_readiness_does_not_release_another_owners_hole() {
    let store = store();
    store.initialize(&[]).unwrap();
    let first = canonical_batch(&store, "first");
    let second = canonical_batch(&store, "second");
    store.mark_native_observation_ready(&second).unwrap();
    assert_eq!(readiness(&store, 2), (0, 0, true));
    assert_eq!(readiness(&store, 1), (0, 0, false));
    store.mark_native_observation_ready(&first).unwrap();
    assert_eq!(readiness(&store, 2), (2, 2, true));
}

#[test]
fn native_readiness_releases_terminal_transcript_without_releasing_prior_journal() {
    let store = store();
    store.initialize(&[]).unwrap();
    let attempt = AttemptId::new("terminal-ready");
    let started = store
        .append_event(envelope(
            store.conversation_id(),
            "started",
            Some(attempt.clone()),
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt.clone(),
            },
        ))
        .unwrap();
    let terminal = store
        .append_event(envelope(
            store.conversation_id(),
            "terminal",
            Some(attempt.clone()),
            RuntimeEvent::AttemptTimedOut {
                attempt_id: attempt,
            },
        ))
        .unwrap();
    store
        .mark_native_observation_ready(terminal.observation.as_deref().unwrap())
        .unwrap();
    assert_eq!(store.observation_ready_frontier(0, 2).unwrap(), 0);
    assert_eq!(
        store.observation_transcript_ready_frontier(0, 1).unwrap(),
        1
    );
    assert!(
        store
            .observation_event_witness(2, 2)
            .unwrap()
            .unwrap()
            .native_direct
    );
    assert!(
        !store
            .observation_event_witness(1, 2)
            .unwrap()
            .unwrap()
            .native_direct
    );
    store
        .mark_native_observation_ready(started.observation.as_deref().unwrap())
        .unwrap();
    assert_eq!(store.observation_ready_frontier(0, 2).unwrap(), 2);
}
