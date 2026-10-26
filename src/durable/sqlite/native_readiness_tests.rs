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
    let reader = Arc::new(SqliteConversationStore::open(id, &path).unwrap());
    let observer = Arc::new(ReadyObserver {
        reader: reader.clone(),
        wakes: Mutex::new(Vec::new()),
    });
    store.observe_journal(observer.clone()).unwrap();
    let batch = canonical_batch(&store, "first");
    assert_eq!(readiness(&reader, 1), (0, 0, false));
    let commits = Arc::new(AtomicUsize::new(0));
    let at_commit = Arc::new(Mutex::new(Vec::new()));
    store
        .lock()
        .unwrap()
        .commit_hook(Some({
            let reader = reader.clone();
            let commits = commits.clone();
            let at_commit = at_commit.clone();
            move || {
                commits.fetch_add(1, AtomicOrdering::Relaxed);
                // The independent WAL connection cannot see a half-released batch
                // while the writer has changed both indexes but has not committed.
                at_commit.lock().unwrap().push(readiness(&reader, 1));
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
    assert_eq!(*at_commit.lock().unwrap(), vec![(0, 0, false)]);
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
    let reader = Arc::new(SqliteConversationStore::open(id.clone(), &path).unwrap());
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
        "CREATE TRIGGER fail_direct_release BEFORE UPDATE OF direct ON event_observation_readiness BEGIN SELECT RAISE(FAIL,'release failure'); END;"
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
    assert_eq!(readiness(&reopened, 1), (0, 0, false));
    reopened.observe_journal(observer).unwrap();
    assert_eq!(
        readiness(&reopened, 1),
        (1, 1, false),
        "reinstall releases history without inventing direct publication"
    );
    assert_eq!(readiness(&reader, 1), (1, 1, false));
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
