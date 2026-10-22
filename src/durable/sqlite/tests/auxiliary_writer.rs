use crate::durable::sqlite::*;
use crate::durable::{CanonicalStore, ConversationStore};

use std::sync::mpsc;
use std::time::Duration;

const AUX_WRITE_TEST_TIMEOUT: Duration = Duration::from_secs(10);

fn writer_store() -> Arc<SqliteConversationStore> {
    let store = Arc::new(SqliteConversationStore::in_memory(ConversationId::generate()).unwrap());
    store.initialize(&[]).unwrap();
    store
}

#[test]
fn required_writer_cancellation_wakes_its_connection_wait_without_releasing_the_owner() {
    let store = writer_store();
    let writer = store.auxiliary_writer();
    let held = store.lock().unwrap();
    let (entered, waiting) = mpsc::sync_channel(1);
    let waiting_writer = writer.clone();
    let task = std::thread::spawn(move || {
        AUXILIARY_CONNECTION_WAIT.with(|probe| *probe.borrow_mut() = Some(entered));
        waiting_writer.load_head()
    });
    waiting.recv_timeout(AUX_WRITE_TEST_TIMEOUT).unwrap();
    writer.cancel_auxiliary_writer();
    assert!(task.join().unwrap().is_err());
    assert!(
        store.conn.available.lock().unwrap().is_none(),
        "cancelling a waiter cannot release somebody else's lease"
    );
    drop(held);
    assert!(store.load_head().is_ok());
}

#[test]
fn required_writer_interrupts_its_running_sql_and_restores_the_connection() {
    let store = writer_store();
    let writer = Arc::new(store.auxiliary_writer_scope());
    let (entered, running) = mpsc::channel();
    *writer.read_progress_entered.lock().unwrap() = Some(entered);
    let executing = writer.clone();
    let task = std::thread::spawn(move || {
        executing.lock().unwrap().query_row(
            "WITH RECURSIVE n(x) AS (VALUES(0) UNION ALL SELECT x+1 FROM n WHERE x<1000000000) SELECT sum(x) FROM n",
            [], |row| row.get::<_, i64>(0),
        )
    });
    running.recv_timeout(AUX_WRITE_TEST_TIMEOUT).unwrap();
    writer.cancel_auxiliary_writer();
    assert!(task.join().unwrap().is_err());
    assert!(
        store.load_head().is_ok(),
        "cancelled write policy must not leak to the next lease"
    );
}

#[test]
fn delayed_required_write_interrupt_cannot_interrupt_a_successor_lease() {
    let store = writer_store();
    let old_writer = store.auxiliary_writer();
    old_writer.load_head().unwrap();
    let (entered, running) = mpsc::sync_channel(1);
    let (release, resumed) = mpsc::sync_channel(1);
    let successor = store.clone();
    let task = std::thread::spawn(move || {
        let connection = successor.lock().unwrap();
        let mut entered = Some(entered);
        connection
            .progress_handler(
                1000,
                Some(move || {
                    if let Some(entered) = entered.take() {
                        let _ = entered.send(());
                        return resumed.recv_timeout(AUX_WRITE_TEST_TIMEOUT).is_err();
                    }
                    false
                }),
            )
            .unwrap();
        let result = connection.query_row(
            "WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<10000) SELECT sum(x) FROM n",
            [], |row| row.get::<_, i64>(0),
        );
        connection
            .progress_handler(0, None::<fn() -> bool>)
            .unwrap();
        result
    });
    running.recv_timeout(AUX_WRITE_TEST_TIMEOUT).unwrap();
    old_writer.cancel_auxiliary_writer();
    release.send(()).unwrap();
    assert_eq!(task.join().unwrap().unwrap(), 50_005_000);
    assert!(store.load_head().is_ok());
}
