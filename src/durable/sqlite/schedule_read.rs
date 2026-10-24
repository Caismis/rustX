//! Cancellation covers existing-only open, schema validation and the header query.
//! The scope owns no connection; interrupt handles are invalidated by SQLite close.
use super::{ConversationStoreError, storage};
use rusqlite::{Connection, InterruptHandle};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

#[derive(Default)]
pub(crate) struct ScheduleReadScope {
    cancelled: AtomicBool,
    interrupt: Mutex<Option<InterruptHandle>>,
    #[cfg(test)]
    progress_probe: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}
impl std::fmt::Debug for ScheduleReadScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScheduleReadScope")
            .field("cancelled", &self.cancelled.load(Ordering::Acquire))
            .finish_non_exhaustive()
    }
}
impl ScheduleReadScope {
    pub(crate) fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        if let Some(handle) = self
            .interrupt
            .lock()
            .expect("schedule read interrupt")
            .as_ref()
        {
            handle.interrupt();
        }
    }
    pub(crate) fn check(&self) -> Result<(), ConversationStoreError> {
        if self.cancelled.load(Ordering::Acquire) {
            Err(ConversationStoreError::OperationCancelled)
        } else {
            Ok(())
        }
    }
    pub(super) fn install(
        self: &Arc<Self>,
        connection: &Connection,
    ) -> Result<(), ConversationStoreError> {
        self.check()?;
        *self.interrupt.lock().expect("schedule read interrupt") =
            Some(connection.get_interrupt_handle());
        // Closing the last WAL writer can briefly exclude a new reader. Use
        // SQLite's bounded wait instead of treating that normal transition as
        // unavailable storage. Each lock wait stays short; interrupt/progress
        // and the owner's overall read deadline remain installed.
        connection
            .busy_timeout(std::time::Duration::from_millis(25))
            .map_err(|e| storage(e.to_string()))?;
        let scope = self.clone();
        // Bound database work independently of elapsed-time cancellation.
        let mut steps = 2_000_000_u64;
        connection
            .progress_handler(
                1000,
                Some(move || {
                    #[cfg(test)]
                    if let Some(probe) = scope.progress_probe.lock().unwrap().take() {
                        probe();
                    }
                    steps = steps.saturating_sub(1000);
                    if steps == 0 {
                        scope.cancelled.store(true, Ordering::Release);
                    }
                    scope.cancelled.load(Ordering::Acquire)
                }),
            )
            .map_err(|e| storage(e.to_string()))?;
        self.check()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn transient_wal_close_lock_does_not_refuse_a_schedule_reader() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("schedule-reader.sqlite");
        let writer = Connection::open(&path).unwrap();
        writer.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE probe(id INTEGER); PRAGMA locking_mode=EXCLUSIVE; BEGIN EXCLUSIVE; INSERT INTO probe VALUES(1); COMMIT;").unwrap();
        let reader =
            Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let scope = Arc::new(ScheduleReadScope::default());
        scope.install(&reader).unwrap();
        let release = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(5));
            drop(writer);
        });
        let mode = reader.query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0));
        release.join().unwrap();
        assert_eq!(mode.unwrap(), "wal");
        scope.check().unwrap();
    }

    #[test]
    fn persistent_wal_lock_remains_bounded_and_cancellable() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("schedule-reader.sqlite");
        let writer = Connection::open(&path).unwrap();
        writer.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE probe(id INTEGER); PRAGMA locking_mode=EXCLUSIVE; BEGIN EXCLUSIVE; INSERT INTO probe VALUES(1); COMMIT;").unwrap();
        let reader =
            Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let scope = Arc::new(ScheduleReadScope::default());
        scope.install(&reader).unwrap();
        let cancelling = scope.clone();
        let cancel = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(5));
            cancelling.cancel();
        });
        let started = std::time::Instant::now();
        let result = reader.query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0));
        assert!(matches!(
            result.unwrap_err().sqlite_error_code(),
            Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::OperationInterrupted)
        ));
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        cancel.join().unwrap();
        assert_eq!(
            scope.check(),
            Err(ConversationStoreError::OperationCancelled)
        );
        drop(writer);
    }

    #[test]
    fn cancellation_interrupts_the_exact_running_query() {
        let scope = Arc::new(ScheduleReadScope::default());
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        *scope.progress_probe.lock().unwrap() = Some(Box::new(move || {
            entered_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        }));
        let worker_scope = scope.clone();
        let worker = std::thread::spawn(move || {
            let connection = Connection::open_in_memory().unwrap();
            worker_scope.install(&connection).unwrap();
            connection.query_row("WITH RECURSIVE n(x) AS (VALUES(0) UNION ALL SELECT x+1 FROM n WHERE x<1000000000) SELECT sum(x) FROM n",[],|row|row.get::<_,i64>(0))
        });
        entered_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        scope.cancel();
        release_tx.send(()).unwrap();
        assert_eq!(
            worker.join().unwrap().unwrap_err().sqlite_error_code(),
            Some(rusqlite::ErrorCode::OperationInterrupted)
        );
        assert_eq!(
            scope.check(),
            Err(ConversationStoreError::OperationCancelled)
        );
    }
    #[test]
    fn database_vm_budget_bounds_work_without_an_elapsed_clock() {
        let scope = Arc::new(ScheduleReadScope::default());
        let connection = Connection::open_in_memory().unwrap();
        scope.install(&connection).unwrap();
        let result=connection.query_row("WITH RECURSIVE n(x) AS (VALUES(0) UNION ALL SELECT x+1 FROM n WHERE x<1000000000) SELECT sum(x) FROM n",[],|row|row.get::<_,i64>(0));
        assert_eq!(
            result.unwrap_err().sqlite_error_code(),
            Some(rusqlite::ErrorCode::OperationInterrupted)
        );
        assert_eq!(
            scope.check(),
            Err(ConversationStoreError::OperationCancelled)
        );
    }
}
