//! Optional observation failures cannot split a native canonical transaction.
use crate::durable::sqlite::test_support::{assistant_message, envelope, store, user_message};
use crate::durable::sqlite::*;
use crate::durable::{CanonicalStore, ConversationStore, JournalStore};
use crate::events::types::RuntimeEvent;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Default)]
struct Commits(AtomicUsize);
impl crate::durable::presentation::JournalObserver for Commits {
    fn committed(&self, _through: Option<u64>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn cancelled_optional_canonical_event_keeps_ledger_surface_transcript_and_journal_atomic() {
    for probe in [
        AuxiliaryStartProbe::ObservationFields,
        AuxiliaryStartProbe::ObservationJson,
    ] {
        let store = Arc::new(store());
        let prior = user_message("prior", "already committed history");
        store.initialize(std::slice::from_ref(&prior)).unwrap();
        let head = store.load_head().unwrap();
        let transcript = store.load_transcript_page(None, 32).unwrap();
        let surface = store.load_surface_history(head.revision).unwrap();
        let notifications = Arc::new(Commits::default());
        assert_eq!(store.observe_journal(notifications.clone()).unwrap(), 0);
        let message = assistant_message("new-assistant", "this message must commit atomically");
        let event = envelope(
            store.conversation_id(),
            "new-assistant-commit",
            None,
            RuntimeEvent::AssistantMessageCommitted {
                message_id: message.id().clone(),
            },
        );
        let (entered, release) = SqliteConversationStore::hold_auxiliary_start_probe_for_test(
            event.event_id.to_string(),
            probe,
        );
        let writer = store.auxiliary_writer();
        let active = writer.clone();
        let candidate = message.clone();
        let fact = event.clone();
        let task = std::thread::spawn(move || active.append_canonical_with_event(&candidate, fact));
        tokio::time::timeout(std::time::Duration::from_secs(10), entered)
            .await
            .unwrap()
            .unwrap();
        writer.cancel_auxiliary_writer();
        drop(release);
        assert_eq!(
            task.join().unwrap().unwrap_err(),
            ConversationStoreError::OperationCancelled
        );
        assert_eq!(store.load_canonical().unwrap(), vec![prior.clone()]);
        assert_eq!(store.load_head().unwrap(), head);
        assert_eq!(store.load_surface_history(head.revision).unwrap(), surface);
        assert_eq!(store.load_transcript_page(None, 32).unwrap(), transcript);
        assert_eq!(store.auxiliary_integrity_counts_for_test(), (0, [0; 5]));
        assert_eq!(notifications.0.load(Ordering::SeqCst), 0);

        // The original uncancelled owner can commit the exact same identities.
        let (committed, receipt) = store.append_canonical_with_event(&message, event).unwrap();
        assert_eq!(committed.sequence, 1);
        assert_eq!(
            receipt.transcript_cursor.unwrap().get(),
            transcript.entries.last().unwrap().cursor.get() + 1
        );
        assert_eq!(store.load_canonical().unwrap(), vec![prior, message]);
        assert_eq!(store.load_head().unwrap().revision, head.revision.next());
        assert_eq!(
            store.load_transcript_page(None, 32).unwrap().entries.len(),
            2
        );
        assert_eq!(store.read_events(None, 32).unwrap().events.len(), 1);
        assert_eq!(notifications.0.load(Ordering::SeqCst), 1);
    }
}

#[test]
fn optional_transcript_abort_cannot_commit_canonical_or_surface_metadata() {
    let store = store();
    let prior = user_message("prior", "committed");
    store.initialize(std::slice::from_ref(&prior)).unwrap();
    let head = store.load_head().unwrap();
    let transcript = store.load_transcript_page(None, 32).unwrap();
    store.lock().unwrap().execute_batch(
        "CREATE TRIGGER abort_optional_transcript BEFORE INSERT ON transcript_observation_readiness WHEN NEW.position=2 BEGIN SELECT RAISE(ROLLBACK,'optional DML aborted the transaction'); END;"
    ).unwrap();
    let message = assistant_message("next", "must not partially commit");
    assert!(matches!(
        store.append_canonical(&message),
        Err(ConversationStoreError::Storage(_))
    ));
    assert_eq!(store.load_canonical().unwrap(), vec![prior.clone()]);
    assert_eq!(store.load_head().unwrap(), head);
    assert_eq!(store.load_transcript_page(None, 32).unwrap(), transcript);
    let counters: (u64, u64) = store
        .lock()
        .unwrap()
        .query_row(
            "SELECT next_transcript_position,retained_transcript_count FROM rustx_store WHERE id=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(counters, (1, 1));
    store
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER abort_optional_transcript")
        .unwrap();
    store.append_canonical(&message).unwrap();
    assert_eq!(store.load_canonical().unwrap(), vec![prior, message]);
    assert_eq!(
        store.load_transcript_page(None, 32).unwrap().entries.len(),
        2
    );
}

#[test]
fn failed_nested_savepoint_cleanup_cannot_be_degraded_by_its_parent() {
    let mut connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch("CREATE TABLE history(value TEXT); INSERT INTO history VALUES('committed')")
        .unwrap();
    let transaction = connection.transaction().unwrap();
    let reached_tail = std::cell::Cell::new(false);
    let result = optional_display_layout(&transaction, || {
        optional_display_layout(&transaction, || {
            transaction
                .execute_batch("RELEASE rustx_display_layout; RELEASE rustx_display_layout")
                .unwrap();
            Err::<(), _>(storage("derived layout failure after losing its savepoint"))
        })?;
        reached_tail.set(true);
        transaction
            .execute("INSERT INTO history VALUES('escaped')", [])
            .unwrap();
        Ok(())
    });
    assert!(matches!(
        result,
        Err(ConversationStoreError::TransactionInvalid(_))
    ));
    assert!(!reached_tail.get());
    transaction.rollback().unwrap();
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM history", [], |row| row
                .get::<_, u64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn optional_layout_requires_an_existing_transaction() {
    let connection = Connection::open_in_memory().unwrap();
    let entered = std::cell::Cell::new(false);
    assert!(matches!(
        optional_display_layout(&connection, || {
            entered.set(true);
            Ok(())
        }),
        Err(ConversationStoreError::TransactionInvalid(_))
    ));
    assert!(!entered.get());
    assert!(connection.is_autocommit());
}

#[test]
fn cancelled_optional_cleanup_reaches_the_outer_transaction_owner() {
    let mut connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch("CREATE TABLE history(value TEXT); INSERT INTO history VALUES('committed')")
        .unwrap();
    let transaction = connection.transaction().unwrap();
    transaction
        .execute("INSERT INTO history VALUES('uncommitted')", [])
        .unwrap();
    let result = optional_display_layout(&transaction, || {
        let mut interrupted = false;
        transaction
            .progress_handler(
                1,
                Some(move || {
                    let first = !interrupted;
                    interrupted = true;
                    first
                }),
            )
            .unwrap();
        Err::<(), _>(storage("contained derived layout failure"))
    });
    assert_eq!(
        result.unwrap_err(),
        ConversationStoreError::OperationCancelled
    );
    transaction
        .progress_handler(0, None::<fn() -> bool>)
        .unwrap();
    if transaction.is_autocommit() {
        drop(transaction);
    } else {
        transaction.rollback().unwrap();
    }
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM history", [], |row| row
                .get::<_, u64>(0))
            .unwrap(),
        1
    );
}
