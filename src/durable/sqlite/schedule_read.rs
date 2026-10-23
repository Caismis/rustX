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
        connection
            .busy_timeout(std::time::Duration::ZERO)
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
