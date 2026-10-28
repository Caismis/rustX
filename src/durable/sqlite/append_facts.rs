//! Typed values retained only for the native append that encoded them.
//! Reuse is authorized by exact bytes read from the current transaction; a
//! changed cell goes through its ordinary decoder, never an earlier read cut.
use super::{Connection, ConversationStoreError, params, storage};

pub(super) struct Captured<T> {
    pub(super) bytes: Vec<u8>,
    pub(super) value: T,
}
impl<T: Clone> Captured<T> {
    pub(super) fn reuse(&self, stored: &[u8]) -> Option<T> {
        (self.bytes == stored).then(|| self.value.clone())
    }
}

pub(super) struct Facts {
    /// Fresh request core was already attempted in this append's independent domain.
    pub(super) request_trace_prepared: bool,
    /// Original raw output and stream settlement preceded this row's Trace capture.
    pub(super) canonical_publication_prepared: bool,
    pub(super) witness: Option<super::observation_event::Captured>,
    pub(super) execution: Option<super::observation_execution::Captured>,
}

/// Only the pre-insert builder supplies this context. No persisted row exists
/// yet: fields refer to the same immutable event and independently prepared
/// domains in the enclosing savepoint, never to a cached historical read.
pub(super) struct PendingEvent<'a> {
    pub(super) statistics: Option<&'a super::observation_statistics::Prepared>,
    pub(super) execution: Option<&'a super::observation_execution::Captured>,
    pub(super) transcript_cursor: Option<crate::durable::TranscriptCursor>,
    pub(super) assistant_ledger: Option<u64>,
}

/// One fixed optional domain; preparing does not write its event columns.
pub(super) struct Aggregates {
    pub(super) statistics: super::observation_statistics::Prepared,
    pub(super) members: Option<Captured<crate::durable::observation::AttemptMemberCounts>>,
}
impl Aggregates {
    pub(super) fn member_anchor(&self) -> Option<u64> {
        self.members
            .as_ref()
            .and_then(|capture| capture.value.process.as_ref())
            .map(|process| process.owner_anchor)
    }
}
pub(super) fn prepare_aggregates(
    connection: &Connection,
    event: &crate::events::types::RuntimeEventEnvelope,
) -> Result<Aggregates, ConversationStoreError> {
    Ok(Aggregates {
        statistics: super::observation_statistics::prepare(connection, event)?,
        members: super::observation_members::prepare_event(connection, event)?,
    })
}
/// The isolation path uses the same fixed-domain preparation after native INSERT.
pub(super) fn aggregates(
    connection: &Connection,
    event: &crate::events::types::RuntimeEventEnvelope,
) -> Result<super::observation_statistics::Prepared, ConversationStoreError> {
    let prepared = prepare_aggregates(connection, event)?;
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
            statement.execute(params![
                trace,
                statistics,
                member_bytes,
                anchor,
                event.sequence
            ])
        })
        .map_err(|error| storage(error.to_string()))?;
    Ok(prepared.statistics)
}

#[cfg(test)]
thread_local! {
    static LIFECYCLE_PROBES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
#[cfg(test)]
pub(super) fn lifecycle_probe() {
    LIFECYCLE_PROBES.with(|count| count.set(count.get() + 1));
}
#[cfg(test)]
pub(super) fn take_lifecycle_probes() -> usize {
    LIFECYCLE_PROBES.with(|count| count.replace(0))
}
