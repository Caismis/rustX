//! Connection-local cache maintenance. Neither evidence owners nor stored facts change.
use std::sync::TryLockError;
use std::time::{Duration, Instant};

use rusqlite::Connection;

use super::{ConnectionOwner, ConversationStoreError, storage};

const PAGE_IDLE: Duration = Duration::from_secs(30);
const STATEMENT_IDLE: Duration = Duration::from_secs(120);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Reclaimed {
    None,
    Pages,
    Statements,
}

pub(super) struct ConnectionSlot {
    pub connection: Option<Connection>,
    last_used: Instant,
    reclaimed: Reclaimed,
}
impl ConnectionSlot {
    pub(super) fn new(connection: Connection) -> Self {
        Self {
            connection: Some(connection),
            last_used: Instant::now(),
            reclaimed: Reclaimed::None,
        }
    }
    pub(super) fn returned(&mut self, connection: Option<Connection>) {
        self.connection = connection;
        self.last_used = Instant::now();
        self.reclaimed = Reclaimed::None;
    }
}
impl ConnectionOwner {
    pub(super) fn reclaim_idle_cache(&self, now: Instant) -> Result<bool, ConversationStoreError> {
        // Never wait behind a user operation, a presentation read or another lease.
        let mut slot = match self.available.try_lock() {
            Ok(slot) => slot,
            Err(TryLockError::WouldBlock) => return Ok(false),
            Err(TryLockError::Poisoned(_)) => return Err(storage("connection poisoned")),
        };
        if self.poisoned.load(std::sync::atomic::Ordering::Acquire) {
            return Err(storage("connection poisoned"));
        }
        let idle = now.saturating_duration_since(slot.last_used);
        let target = if idle >= STATEMENT_IDLE {
            Reclaimed::Statements
        } else if idle >= PAGE_IDLE {
            Reclaimed::Pages
        } else {
            return Ok(false);
        };
        if slot.reclaimed == target || slot.reclaimed == Reclaimed::Statements {
            return Ok(false);
        }
        let Some(connection) = slot.connection.as_ref() else {
            return Ok(false);
        };
        if !connection.is_autocommit() {
            return Ok(false);
        }
        if target == Reclaimed::Statements {
            connection.flush_prepared_statement_cache();
        }
        connection
            .release_memory()
            .map_err(|error| storage(format!("release idle SQLite cache: {error}")))?;
        slot.reclaimed = target;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::durable::sqlite::SqliteConversationStore;
    use crate::runtime::identity::ConversationId;
    use std::sync::Arc;

    #[allow(unsafe_code)]
    fn memory(connection: &Connection, kind: i32) -> i32 {
        let (mut current, mut high) = (0, 0);
        // Test-only SQLite introspection while this test owns the connection.
        unsafe {
            assert_eq!(
                rusqlite::ffi::sqlite3_db_status(
                    connection.handle(),
                    kind,
                    &raw mut current,
                    &raw mut high,
                    0
                ),
                rusqlite::ffi::SQLITE_OK
            );
        }
        current
    }
    fn age(owner: &ConnectionOwner, elapsed: Duration) -> Instant {
        let now = Instant::now();
        owner.available.lock().unwrap().last_used = now.checked_sub(elapsed).unwrap();
        now
    }
    fn plans(store: &SqliteConversationStore) -> i32 {
        let slot = store.conn.available.lock().unwrap();
        memory(
            slot.connection.as_ref().unwrap(),
            rusqlite::ffi::SQLITE_DBSTATUS_STMT_USED,
        )
    }
    fn populated() -> (tempfile::TempDir, SqliteConversationStore) {
        let root = tempfile::tempdir().unwrap();
        let store = SqliteConversationStore::open(
            ConversationId::new("conv_8307904d-d256-77de-8e8a-63789f0ed1a5"),
            &root.path().join("conversation.sqlite"),
        )
        .unwrap();
        {
            let connection = store.conn.lock().unwrap();
            for i in 0..100 {
                connection
                    .prepare_cached(&format!("SELECT {i} FROM sqlite_schema"))
                    .unwrap()
                    .query_map([], |_| Ok(()))
                    .unwrap()
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap();
            }
        }
        (root, store)
    }
    #[test]
    fn pages_then_statements_reclaim_without_reopening_or_changing_facts() {
        let (_root, store) = populated();
        let before = plans(&store);
        assert!(before > 100_000);
        let cache_before = {
            let slot = store.conn.available.lock().unwrap();
            memory(
                slot.connection.as_ref().unwrap(),
                rusqlite::ffi::SQLITE_DBSTATUS_CACHE_USED,
            )
        };
        let manifest = Arc::clone(&store.conn.manifest);
        let now = age(&store.conn, PAGE_IDLE + Duration::from_secs(1));
        assert!(store.conn.reclaim_idle_cache(now).unwrap());
        assert_eq!(
            plans(&store),
            before,
            "page maintenance preserves prepared plans"
        );
        let cache_after = {
            let slot = store.conn.available.lock().unwrap();
            memory(
                slot.connection.as_ref().unwrap(),
                rusqlite::ffi::SQLITE_DBSTATUS_CACHE_USED,
            )
        };
        assert!(cache_after < cache_before);
        let now = age(&store.conn, STATEMENT_IDLE + Duration::from_secs(1));
        assert!(store.conn.reclaim_idle_cache(now).unwrap());
        assert!(
            !store.conn.reclaim_idle_cache(now).unwrap(),
            "one reclamation per idle stage"
        );
        assert_eq!(plans(&store), 0);
        eprintln!(
            "{}",
            serde_json::json!({"kind":"idle_cache_unit_probe","cache_before":cache_before,"cache_after":cache_after,"statements_before":before,"statements_after":0})
        );
        assert!(Arc::ptr_eq(&manifest, &store.conn.manifest));
        assert_eq!(
            store
                .conn
                .lock()
                .unwrap()
                .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
                .unwrap(),
            "ok"
        );
        {
            let connection = store.conn.lock().unwrap();
            connection
                .prepare_cached("SELECT 42")
                .unwrap()
                .query_row([], |row| row.get::<_, u64>(0))
                .unwrap();
        }
        let active = plans(&store);
        assert!(active > 0);
        assert!(!store.conn.reclaim_idle_cache(Instant::now()).unwrap());
        assert_eq!(
            plans(&store),
            active,
            "resumed operations receive a fresh full idle period"
        );
    }
    #[test]
    fn maintenance_never_waits_for_a_lease_or_reclaims_inside_a_transaction() {
        let (_root, store) = populated();
        let now = age(&store.conn, STATEMENT_IDLE + Duration::from_secs(1));
        let held = store.conn.lock().unwrap();
        assert!(!store.conn.reclaim_idle_cache(now).unwrap()); // Does not wait for held.
        held.execute_batch(
            "BEGIN; CREATE TABLE transient(value); INSERT INTO transient VALUES (7)",
        )
        .unwrap();
        drop(held);
        let before = plans(&store);
        let now = age(&store.conn, STATEMENT_IDLE + Duration::from_secs(1));
        store.conn.reclaim_idle_cache(now).unwrap();
        assert_eq!(plans(&store), before);
        let held = store.conn.lock().unwrap();
        assert_eq!(
            held.query_row("SELECT value FROM transient", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            7
        );
        held.execute_batch("ROLLBACK").unwrap();
    }
}
