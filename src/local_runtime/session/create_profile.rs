//! Test-only stage profiler for the real Session-create pipeline (Issue #387).
//!
//! This is deliberately not a production observability framework: it exists
//! only in this crate's test build, is enabled explicitly by one ignored test,
//! and is compiled away everywhere else. It attributes wall time to the
//! pipeline's real owners so the delivery report can separate exclusive stages
//! from inclusive totals.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Exclusive wall time per pipeline stage, in nanoseconds.
///
/// Stage names track the **real** implementation boundaries. In
/// particular, [`Self::sqlite_bootstrap_ns`] covers the whole
/// `SqliteConversationStore::open` call, which already performs connection
/// open, connection configuration, schema creation-or-validation and
/// Conversation identity binding. [`Self::lineage_initialize_ns`] covers
/// only the separate `initialize_lineage(seed)` call. The two stages are
/// not "connection open" and "schema + seed".
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct StageTimes {
    pub(crate) catalog_snapshot_ns: u64,
    pub(crate) reserve_ns: u64,
    pub(crate) allocation_dir_ns: u64,
    pub(crate) sqlite_bootstrap_ns: u64,
    pub(crate) lineage_initialize_ns: u64,
    pub(crate) catalog_document_clone_ns: u64,
    pub(crate) catalog_serialize_ns: u64,
    pub(crate) temp_write_ns: u64,
    pub(crate) file_fsync_ns: u64,
    pub(crate) rename_ns: u64,
    pub(crate) dir_fsync_ns: u64,
    pub(crate) creates: u64,
}

/// The sum of every exclusive leaf stage. Inclusive parent totals are
/// deliberately not part of this sum; callers measure the real operation
/// boundary and treat the difference as the unattributed remainder.
impl StageTimes {
    #[must_use]
    pub(crate) fn exclusive_total_ns(&self) -> u64 {
        self.catalog_snapshot_ns
            .saturating_add(self.reserve_ns)
            .saturating_add(self.allocation_dir_ns)
            .saturating_add(self.sqlite_bootstrap_ns)
            .saturating_add(self.lineage_initialize_ns)
            .saturating_add(self.catalog_document_clone_ns)
            .saturating_add(self.catalog_serialize_ns)
            .saturating_add(self.temp_write_ns)
            .saturating_add(self.file_fsync_ns)
            .saturating_add(self.rename_ns)
            .saturating_add(self.dir_fsync_ns)
    }
}

static ENABLED: AtomicBool = AtomicBool::new(false);
static TIMES: Mutex<StageTimes> = Mutex::new(StageTimes {
    catalog_snapshot_ns: 0,
    reserve_ns: 0,
    allocation_dir_ns: 0,
    sqlite_bootstrap_ns: 0,
    lineage_initialize_ns: 0,
    catalog_document_clone_ns: 0,
    catalog_serialize_ns: 0,
    temp_write_ns: 0,
    file_fsync_ns: 0,
    rename_ns: 0,
    dir_fsync_ns: 0,
    creates: 0,
});

pub(crate) fn enable() {
    *TIMES.lock().expect("create profile lock") = StageTimes::default();
    ENABLED.store(true, Ordering::Relaxed);
}

pub(crate) fn disable() {
    ENABLED.store(false, Ordering::Relaxed);
}

pub(crate) fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

fn nanos(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

fn record_field(select: impl FnOnce(&mut StageTimes) -> &mut u64, duration: Duration) {
    if !enabled() {
        return;
    }
    let mut times = TIMES.lock().expect("create profile lock");
    let elapsed = nanos(duration);
    let slot = select(&mut times);
    *slot = slot.saturating_add(elapsed);
}

pub(crate) fn count_create() {
    if !enabled() {
        return;
    }
    let mut times = TIMES.lock().expect("create profile lock");
    times.creates = times.creates.saturating_add(1);
}

pub(crate) fn timed<T>(
    select: impl FnOnce(&mut StageTimes) -> &mut u64,
    body: impl FnOnce() -> T,
) -> T {
    if !enabled() {
        return body();
    }
    let started = Instant::now();
    let value = body();
    record_field(select, started.elapsed());
    value
}

pub(crate) fn snapshot() -> StageTimes {
    *TIMES.lock().expect("create profile lock")
}
