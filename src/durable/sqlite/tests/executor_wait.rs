use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Condvar, Mutex};

use rusqlite::Connection;

use std::sync::atomic::AtomicBool;

use crate::durable::sqlite::schema::configure_connection;
use crate::durable::sqlite::*;
use std::sync::mpsc;
use std::time::Duration;

const LIVENESS: Duration = Duration::from_secs(60);

/// One worker: anything that parks it stops every other task.
fn one_worker() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap()
}

/// Another task on the same runtime runs to completion.
fn executor_progresses(runtime: &tokio::runtime::Runtime) {
    let (sender, receiver) = mpsc::channel();
    runtime.spawn(async move { sender.send(()).unwrap() });
    receiver
        .recv_timeout(LIVENESS)
        .expect("the runtime must keep running other tasks");
}

#[test]
fn writer_busy_schedule_is_sqlites_own_with_the_store_budget() {
    let delays: Vec<_> = (0..)
        .map_while(writer_busy_delay)
        .map(|delay| delay.as_millis())
        .collect();
    assert_eq!(delays[..12], [1, 2, 5, 10, 15, 20, 25, 25, 25, 50, 50, 100]);
    assert!(delays[12..].iter().all(|delay| *delay <= 100));
    assert_eq!(delays.iter().sum::<u128>(), 5000);
    assert_eq!(writer_busy_delay(i32::MAX), None);
}

/// A store write waiting for another connection's lock leaves the
/// executor running; releasing the lock then lets the same write commit.
#[test]
fn a_busy_store_writer_does_not_park_its_tokio_worker() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("store.sqlite");
    let mut writer = Connection::open(&path).unwrap();
    configure_connection(&mut writer, false).unwrap();
    writer
        .execute_batch("CREATE TABLE contended (value INTEGER)")
        .unwrap();
    let competitor = Connection::open(&path).unwrap();
    competitor.execute_batch("BEGIN IMMEDIATE").unwrap();
    let runtime = one_worker();
    let (busy, waiting) = mpsc::channel();
    let write = runtime.spawn(async move {
        let _probe = busy_probe::observe(move |count| {
            if count == 0 {
                busy.send(()).unwrap();
            }
        });
        // No await: the whole write is one synchronous poll, as in the store.
        writer.execute_batch("BEGIN IMMEDIATE; INSERT INTO contended VALUES (1); COMMIT")
    });
    // SQLite itself reports the contention from the writer's handler.
    waiting.recv_timeout(LIVENESS).unwrap();
    executor_progresses(&runtime);
    competitor.execute_batch("ROLLBACK").unwrap();
    runtime.block_on(write).unwrap().unwrap();
    let committed: i64 = competitor
        .query_row("SELECT count(*) FROM contended", [], |row| row.get(0))
        .unwrap();
    assert_eq!(committed, 1);
}

/// Waiting for the connection another store operation holds likewise
/// leaves the executor running.
#[test]
fn a_connection_wait_does_not_park_its_tokio_worker() {
    let owner = Arc::new(ConnectionOwner {
        manifest: manifest::Owner::uninstalled(),
        append_validation: append_validation::Owner::uninstalled(),
        available: Mutex::new(Some(Connection::open_in_memory().unwrap())),
        wake: Condvar::new(),
        poisoned: AtomicBool::new(false),
        lease_generation: AtomicU64::new(0),
        active_interrupt: Mutex::new(None),
    });
    // Declared first so a failing assertion still releases the lease
    // before the runtime waits for its worker on unwind.
    let runtime = one_worker();
    let held = owner.lock().unwrap();
    let waiter = owner.clone();
    let (started, running) = mpsc::channel();
    let wait = runtime.spawn(async move {
        started.send(()).unwrap();
        waiter.lock().map(|_| ())
    });
    running.recv_timeout(LIVENESS).unwrap();
    // The waiter's poll continues synchronously into the held connection,
    // so another task can run only if that wait hands the worker off.
    executor_progresses(&runtime);
    drop(held);
    runtime.block_on(wait).unwrap().unwrap();
}
