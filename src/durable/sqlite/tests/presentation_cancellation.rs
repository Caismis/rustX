use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Condvar, Mutex};

use rusqlite::Connection;

use std::sync::atomic::AtomicBool;

use crate::runtime::identity::ConversationId;

use crate::durable::ConversationStore;
use crate::durable::sqlite::*;

#[test]
fn presentation_cancellation_interrupts_running_sql_and_restores_writer_policy() {
    let manifest = manifest::Owner::uninstalled();
    let owner = Arc::new(ConnectionOwner {
        manifest,
        append_validation: append_validation::Owner::uninstalled(),
        available: Mutex::new(Some(Connection::open_in_memory().unwrap())),
        wake: Condvar::new(),
        poisoned: AtomicBool::new(false),
        lease_generation: AtomicU64::new(0),
        active_interrupt: Mutex::new(None),
    });
    let mut reader = SqliteConversationStore::from_connection_owner(
        ConversationId::new("conv_8307904d-d256-77de-8e8a-63789f0ed1a5"),
        owner.clone(),
    );
    reader.read_cancel = Some(Arc::new(AtomicBool::new(false)));
    let (entered, receiving) = std::sync::mpsc::channel();
    *reader.read_progress_entered.lock().unwrap() = Some(entered);
    let reader = Arc::new(reader);
    let running = reader.clone();
    let task = std::thread::spawn(move || {
        running.lock().unwrap().query_row(
            "WITH RECURSIVE n(x) AS (VALUES(0) UNION ALL SELECT x+1 FROM n) SELECT sum(x) FROM n",
            [],
            |row| row.get::<_, i64>(0),
        )
    });
    receiving.recv().unwrap(); // The actual SQLite VM has executed 1,000 operations.
    reader.close_presentation_reader();
    assert!(
        matches!(task.join().unwrap(), Err(rusqlite::Error::SqliteFailure(error, _)) if error.code == rusqlite::ErrorCode::OperationInterrupted)
    );
    let conn = owner.lock().unwrap();
    // The zero-wait presentation policy is gone: the writer handler is
    // back, and SQLite's own timeout no longer governs this connection.
    assert_eq!(
        conn.query_row("PRAGMA busy_timeout", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        0
    );
    conn.execute_batch(
        "CREATE TABLE normal_write (value INTEGER); INSERT INTO normal_write VALUES (1)",
    )
    .unwrap();
}

#[test]
fn presentation_cancellation_ends_connection_wait_without_releasing_writer() {
    let manifest = manifest::Owner::uninstalled();
    let owner = Arc::new(ConnectionOwner {
        manifest,
        append_validation: append_validation::Owner::uninstalled(),
        available: Mutex::new(Some(Connection::open_in_memory().unwrap())),
        wake: Condvar::new(),
        poisoned: AtomicBool::new(false),
        lease_generation: AtomicU64::new(0),
        active_interrupt: Mutex::new(None),
    });
    let writer = owner.lock().unwrap();
    let mut reader = SqliteConversationStore::from_connection_owner(
        ConversationId::new("conv_8307904d-d256-77de-8e8a-63789f0ed1a5"),
        owner.clone(),
    );
    reader.read_cancel = Some(Arc::new(AtomicBool::new(false)));
    let reader = Arc::new(reader);
    let running = reader.clone();
    let task = std::thread::spawn(move || running.lock().map(|_| ()));
    reader.close_presentation_reader();
    assert!(task.join().unwrap().is_err());
    drop(writer); // The writer never needed to release for reader termination.
}
