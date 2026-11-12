//! Bounded proofs of rank geometry, seated only by the native COMMIT owner.
//!
//! A proof never replaces a node read. It only avoids rereading descendants
//! already authenticated in the same dependency epoch. Native writes, Source
//! rewrites and external snapshot changes invalidate these independent proofs.
use super::{Arc, AtomicU64, Mutex, Ordering};
use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;

const CAPACITY: usize = 256;
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(in crate::durable::sqlite) struct Key {
    pub node: u64,
    pub source: [u8; 32],
    pub rank: u64,
    pub sequence_width: u32,
}
#[derive(Debug)]
pub(in crate::durable::sqlite) struct Cache {
    enabled: AtomicBool,
    native_epoch: Arc<AtomicU64>,
    source_epoch: AtomicU64,
    entries: Mutex<((u64, u64), BTreeMap<Key, u64>)>,
}
impl Cache {
    pub(super) fn new(native_epoch: Arc<AtomicU64>) -> Arc<Self> {
        Arc::new(Self {
            enabled: AtomicBool::new(false),
            native_epoch,
            source_epoch: AtomicU64::new(0),
            entries: Mutex::default(),
        })
    }
    pub(super) fn enable(&self) {
        self.enabled.store(true, Ordering::Release);
    }
    pub(in crate::durable::sqlite) fn disable(&self) {
        self.enabled.store(false, Ordering::Release);
    }
    pub(super) fn observe(
        &self,
        table: &str,
        change: &rusqlite::hooks::PreUpdateCase,
        columns: &[i32],
    ) {
        use rusqlite::{hooks::PreUpdateCase, types::ValueRef};
        if table != "native_input_nodes" {
            return;
        }
        let invalidates = match change {
            PreUpdateCase::Update {
                old_value_accessor: old,
                new_value_accessor: new,
            } => {
                // Filling a previously absent augmentation cannot change any
                // certified descendant. A rewrite of existing evidence can.
                let existing = columns
                    .iter()
                    .any(|i| !matches!(old.get_old_column_value(*i), Ok(ValueRef::Null)));
                existing
                    && columns.iter().any(|i| {
                        match (old.get_old_column_value(*i), new.get_new_column_value(*i)) {
                            (Ok(a), Ok(b)) => a != b,
                            _ => true,
                        }
                    })
            }
            PreUpdateCase::Delete(_) => true,
            _ => false,
        };
        if invalidates {
            let _ = self
                .source_epoch
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                    Some(n.saturating_add(1))
                });
        }
    }
}
#[derive(Clone, Debug)]
pub(in crate::durable::sqlite) struct Session {
    cache: Arc<Cache>,
    epochs: (u64, u64),
    pending: Arc<Mutex<BTreeMap<Key, u64>>>,
}
fn insert(entries: &mut BTreeMap<Key, u64>, key: Key, length: u64) {
    entries.insert(key, length);
    if entries.len() > CAPACITY {
        entries.pop_first();
    }
}
impl Session {
    #[cfg(test)]
    pub(in crate::durable::sqlite) fn for_test() -> Self {
        let cache = Cache::new(Arc::new(AtomicU64::new(0)));
        cache.enable();
        Self::new(&cache)
    }
    pub(super) fn new(cache: &Arc<Cache>) -> Self {
        Self {
            epochs: (
                cache.native_epoch.load(Ordering::Acquire),
                cache.source_epoch.load(Ordering::Acquire),
            ),
            cache: Arc::clone(cache),
            pending: Arc::default(),
        }
    }
    fn valid(&self) -> bool {
        self.cache.enabled.load(Ordering::Acquire)
            && self.epochs.0 != u64::MAX
            && self.epochs.1 != u64::MAX
            && self.epochs
                == (
                    self.cache.native_epoch.load(Ordering::Acquire),
                    self.cache.source_epoch.load(Ordering::Acquire),
                )
    }
    pub(in crate::durable::sqlite) fn get(&self, key: &Key) -> Option<u64> {
        if !self.valid() {
            return None;
        }
        self.pending.lock().ok()?.get(key).copied().or_else(|| {
            let entries = self.cache.entries.lock().ok()?;
            (entries.0 == self.epochs)
                .then(|| entries.1.get(key).copied())
                .flatten()
        })
    }
    pub(in crate::durable::sqlite) fn remember(&self, key: Key, length: u64) {
        if self.valid()
            && let Ok(mut entries) = self.pending.lock()
        {
            insert(&mut entries, key, length);
        }
    }
    pub(super) fn commit(&self) {
        if self.valid()
            && let (Ok(pending), Ok(mut entries)) = (self.pending.lock(), self.cache.entries.lock())
        {
            if entries.0 != self.epochs {
                entries.1.clear();
                entries.0 = self.epochs;
            }
            for (&key, &length) in pending.iter() {
                insert(&mut entries.1, key, length);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn key(node: u64) -> Key {
        Key {
            node,
            source: [1; 32],
            rank: 0,
            sequence_width: 2,
        }
    }
    #[test]
    fn geometry_proofs_require_commit_and_both_dependency_epochs() {
        let epoch = Arc::new(AtomicU64::new(0));
        let cache = Cache::new(Arc::clone(&epoch));
        cache.enable();
        let staged = Session::new(&cache);
        staged.remember(key(1), 123);
        assert_eq!(staged.get(&key(1)), Some(123));
        assert_eq!(
            Session::new(&cache).get(&key(1)),
            None,
            "uncommitted proof escaped"
        );
        staged.commit();
        assert_eq!(Session::new(&cache).get(&key(1)), Some(123));
        epoch.store(1, Ordering::Release);
        assert_eq!(staged.get(&key(1)), None);
        let native_changed = Session::new(&cache);
        assert_eq!(native_changed.get(&key(1)), None, "old epoch proof revived");
        native_changed.remember(key(1), 456);
        native_changed.commit();
        cache.source_epoch.store(1, Ordering::Release);
        assert_eq!(Session::new(&cache).get(&key(1)), None);
        assert_eq!(native_changed.get(&key(1)), None);
        // A failed optional operation may be rolled back. Its session cannot
        // later publish proof acquired before that invalidating write.
        native_changed.commit();
        assert_eq!(Session::new(&cache).get(&key(1)), None);
        let current = Session::new(&cache);
        current.remember(key(2), 789);
        current.commit();
        cache.disable();
        assert_eq!(current.get(&key(2)), None);
        assert_eq!(Session::new(&cache).get(&key(2)), None);
    }
    #[test]
    fn geometry_proofs_are_bounded_and_never_reused_at_saturated_epochs() {
        let epoch = Arc::new(AtomicU64::new(0));
        let cache = Cache::new(Arc::clone(&epoch));
        cache.enable();
        let staged = Session::new(&cache);
        for node in 0..1024 {
            staged.remember(key(node), node);
        }
        assert_eq!(staged.pending.lock().unwrap().len(), CAPACITY);
        staged.commit();
        assert_eq!(cache.entries.lock().unwrap().1.len(), CAPACITY);
        epoch.store(u64::MAX, Ordering::Release);
        let saturated = Session::new(&cache);
        saturated.remember(key(1), 123);
        assert_eq!(saturated.get(&key(1)), None);
    }
    #[test]
    fn source_rewrites_including_rollback_invalidate_but_fresh_fills_do_not() {
        let epoch = Arc::new(AtomicU64::new(0));
        let cache = Cache::new(epoch);
        cache.enable();
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        connection.execute_batch("CREATE TABLE native_input_nodes(id INTEGER PRIMARY KEY,source_native_digest BLOB,source_hash BLOB,source_metadata BLOB); INSERT INTO native_input_nodes(id)VALUES(1),(2)").unwrap();
        let watched = Arc::clone(&cache);
        connection
            .preupdate_hook(Some(
                move |_: rusqlite::hooks::Action,
                      _: &str,
                      table: &str,
                      change: &rusqlite::hooks::PreUpdateCase| {
                    watched.observe(table, change, &[1, 2, 3]);
                },
            ))
            .unwrap();
        let original = Session::new(&cache);
        original.remember(key(1), 123);
        original.commit();
        connection
            .execute_batch("UPDATE native_input_nodes SET source_metadata=x'01' WHERE id=2")
            .unwrap();
        assert_eq!(Session::new(&cache).get(&key(1)), Some(123));
        connection.execute_batch("SAVEPOINT rewrite; UPDATE native_input_nodes SET source_metadata=x'02' WHERE id=2; ROLLBACK TO rewrite; RELEASE rewrite").unwrap();
        assert_eq!(Session::new(&cache).get(&key(1)), None);
        assert_eq!(original.get(&key(1)), None);
    }
}
