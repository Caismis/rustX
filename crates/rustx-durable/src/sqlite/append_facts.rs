//! Typed values retained only for the native append that encoded them.
//! Reuse is authorized by exact bytes read from the current transaction; a
//! changed cell goes through its ordinary decoder, never an earlier read cut.
use super::{Connection, ConversationStoreError};

pub struct Captured<T> {
    pub bytes: Vec<u8>,
    pub value: T,
}
impl<T: Clone> Captured<T> {
    pub fn reuse(&self, stored: &[u8]) -> Option<T> {
        (self.bytes == stored).then(|| self.value.clone())
    }
}

/// A single decoded value, consumed only after the caller read the current cell.
/// Equality is over all stored bytes; this never caches a SQL lookup or cut.
#[derive(Default)]
pub struct FoldCache<T>(std::sync::Mutex<Option<Captured<T>>>);
impl<T> FoldCache<T> {
    pub fn take(&self, stored: &[u8]) -> Option<T> {
        let value = self
            .0
            .lock()
            .expect("fold cache")
            .take()
            .filter(|entry| entry.bytes == stored)
            .map(|entry| entry.value);
        #[cfg(any(test, feature = "test-hooks"))]
        FOLD_WORK.with(|work| {
            let (hits, misses) = work.get();
            work.set((
                hits + usize::from(value.is_some()),
                misses + usize::from(value.is_none()),
            ));
        });
        value
    }
    pub fn remember(&self, bytes: &[u8], value: T) {
        self.remember_owned(bytes.to_vec(), value);
    }
    pub fn remember_owned(&self, bytes: Vec<u8>, value: T) {
        *self.0.lock().expect("fold cache") = Some(Captured { bytes, value });
    }
}

/// Compare/decode the current SQLite cell while it is borrowed. A cache hit
/// neither copies the stored BLOB into a Vec nor skips its actual read.
pub fn read_fold<T>(
    row: &rusqlite::Row<'_>,
    cache: Option<&FoldCache<T>>,
    decode: impl FnOnce(&[u8]) -> Result<T, ConversationStoreError>,
) -> rusqlite::Result<Result<Option<T>, ConversationStoreError>> {
    match row.get_ref(0)? {
        rusqlite::types::ValueRef::Null => Ok(Ok(None)),
        rusqlite::types::ValueRef::Blob(bytes) => Ok(cache
            .and_then(|cache| cache.take(bytes))
            .map_or_else(|| decode(bytes), Ok)
            .map(Some)),
        value => Err(rusqlite::Error::InvalidColumnType(
            0,
            "aggregate fold".into(),
            value.data_type(),
        )),
    }
}

pub struct Facts {
    /// Fresh request core was already attempted in this append's independent domain.
    pub request_trace_prepared: bool,
    /// Original raw output and stream settlement preceded this row's Trace capture.
    pub canonical_publication_prepared: bool,
    pub witness: Option<super::observation_event::Captured>,
    pub execution: Option<super::observation_execution::Captured>,
    /// A following member event may reuse this value after checking its stored head.
    pub members: Option<super::observation_members::Captured>,
}

/// Only the pre-insert builder supplies this context. No persisted row exists
/// yet: fields refer to the same immutable event and independently prepared
/// domains in the enclosing savepoint, never to a cached historical read.
pub struct PendingEvent<'a> {
    pub statistics: Option<&'a super::observation_statistics::Prepared>,
    pub execution: Option<&'a super::observation_execution::Captured>,
    pub transcript_cursor: Option<crate::TranscriptCursor>,
    pub assistant_ledger: Option<u64>,
}

/// One fixed optional domain; preparing does not write its event columns.
pub struct Aggregates {
    pub statistics: super::observation_statistics::Prepared,
    pub members: Option<Captured<crate::observation::AttemptMemberCounts>>,
}
impl Aggregates {
    pub fn member_anchor(&self) -> Option<u64> {
        self.members
            .as_ref()
            .and_then(|capture| capture.value.process.as_ref())
            .map(|process| process.owner_anchor)
    }
}
pub fn prepare_aggregates(
    connection: &Connection,
    event: &rustx_types::events::types::RuntimeEventEnvelope,
    previous_members: Option<&super::observation_members::Captured>,
) -> Result<Aggregates, ConversationStoreError> {
    prepare_aggregates_cached(connection, event, previous_members, None)
}
pub fn prepare_aggregates_cached(
    connection: &Connection,
    event: &rustx_types::events::types::RuntimeEventEnvelope,
    previous_members: Option<&super::observation_members::Captured>,
    cache: Option<&super::observation_statistics::FoldCache>,
) -> Result<Aggregates, ConversationStoreError> {
    Ok(Aggregates {
        statistics: super::observation_statistics::prepare_cached(connection, event, cache)?,
        members: super::observation_members::prepare_event(connection, event, previous_members)?,
    })
}
/// The isolation path uses the same fixed-domain preparation after native INSERT.
pub fn aggregates(
    connection: &Connection,
    event: &rustx_types::events::types::RuntimeEventEnvelope,
) -> Result<super::observation_statistics::Prepared, ConversationStoreError> {
    let prepared = prepare_aggregates(connection, event, None)?;
    let trace = prepared.statistics.trace_total;
    let statistics = prepared
        .statistics
        .statistics
        .as_ref()
        .map(|capture| &capture.bytes);
    let member_bytes = prepared.members.as_ref().map(|capture| &capture.bytes);
    let anchor = prepared.member_anchor();
    let sql = match (
        trace.is_some(),
        statistics.is_some(),
        member_bytes.is_some(),
    ) {
        (true, true, true) => {
            "UPDATE events SET observation_trace_total=?1,observation_statistics=?2,observation_members=?3,observation_member_anchor=?4 WHERE sequence=?5"
        }
        (true, true, false) => {
            "UPDATE events SET observation_trace_total=?1,observation_statistics=?2 WHERE sequence=?5"
        }
        (true, false, true) => {
            "UPDATE events SET observation_trace_total=?1,observation_members=?3,observation_member_anchor=?4 WHERE sequence=?5"
        }
        (true, false, false) => "UPDATE events SET observation_trace_total=?1 WHERE sequence=?5",
        (false, true, true) => {
            "UPDATE events SET observation_statistics=?2,observation_members=?3,observation_member_anchor=?4 WHERE sequence=?5"
        }
        (false, true, false) => "UPDATE events SET observation_statistics=?2 WHERE sequence=?5",
        (false, false, true) => {
            "UPDATE events SET observation_members=?3,observation_member_anchor=?4 WHERE sequence=?5"
        }
        (false, false, false) => return Ok(prepared.statistics),
    };
    connection
        .prepare_cached(sql)
        .and_then(|mut statement| {
            let values: [&dyn rusqlite::ToSql; 5] =
                [&trace, &statistics, &member_bytes, &anchor, &event.sequence];
            statement.execute(&values[..])
        })
        .map_err(|error| crate::sqlite::sqlite_error(&error, error.to_string()))?;
    Ok(prepared.statistics)
}

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    static LIFECYCLE_PROBES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
#[cfg(any(test, feature = "test-hooks"))]
pub fn lifecycle_probe() {
    LIFECYCLE_PROBES.with(|count| count.set(count.get() + 1));
}
#[cfg(any(test, feature = "test-hooks"))]
pub fn take_lifecycle_probes() -> usize {
    LIFECYCLE_PROBES.with(|count| count.replace(0))
}

#[cfg(any(test, feature = "test-hooks"))]
thread_local! { static FOLD_WORK: std::cell::Cell<(usize,usize)> = const {std::cell::Cell::new((0,0))}; }
#[cfg(any(test, feature = "test-hooks"))]
pub fn take_fold_work() -> (usize, usize) {
    FOLD_WORK.with(|work| work.replace((0, 0)))
}
