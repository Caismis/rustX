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
    pub(super) witness: Option<super::observation_event::Captured>,
    pub(super) execution: Option<super::observation_execution::Captured>,
}

/// Statistics and membership already share one optional savepoint. Materialize
/// their event columns together, retaining that exact failure boundary.
pub(super) fn aggregates(
    connection: &Connection,
    event: &crate::events::types::RuntimeEventEnvelope,
) -> Result<super::observation_statistics::Prepared, ConversationStoreError> {
    let prepared = super::observation_statistics::prepare(connection, event)?;
    let members = super::observation_members::prepare_event(connection, event)?;
    let trace = prepared.trace_total;
    let statistics = prepared.statistics.as_ref().map(|capture| &capture.bytes);
    let member_bytes = members.as_ref().map(|capture| &capture.bytes);
    let anchor = members
        .as_ref()
        .and_then(|capture| capture.value.process.as_ref())
        .map(|process| process.owner_anchor);
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
        (false, false, false) => return Ok(prepared),
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
    Ok(prepared)
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
