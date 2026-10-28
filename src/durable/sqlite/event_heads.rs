//! Two shared B-trees for four optional event heads. A lookup seeks the last
//! sequence in each of the two exact masks containing its bit, then selects the
//! newer of those two rows. It never scans unrelated headers or sorts history.
use super::{Connection, ConversationStoreError, OptionalExtension, storage};

pub(super) const STATISTICS: &str = "SELECT observation_statistics FROM events WHERE sequence=(
    SELECT max(sequence) FROM (
        SELECT (SELECT sequence FROM events INDEXED BY events_aggregate_heads_idx
            WHERE observation_aggregate_heads>0 AND observation_aggregate_heads=2
            AND sequence<=?1 ORDER BY sequence DESC LIMIT 1) AS sequence
        UNION ALL
        SELECT (SELECT sequence FROM events INDEXED BY events_aggregate_heads_idx
            WHERE observation_aggregate_heads>0 AND observation_aggregate_heads=3
            AND sequence<=?1 ORDER BY sequence DESC LIMIT 1)
    ))";

pub(super) const TRACE_TOTAL: &str = "SELECT EXISTS(
    SELECT 1 FROM events INDEXED BY events_aggregate_holes_idx
    WHERE observation_aggregate_holes>0 AND observation_aggregate_holes IN (2,3,6,7)
    AND sequence<=?1), (SELECT observation_trace_total FROM events WHERE sequence=(
    SELECT max(sequence) FROM (
        SELECT (SELECT sequence FROM events INDEXED BY events_aggregate_heads_idx
            WHERE observation_aggregate_heads>0 AND observation_aggregate_heads=1
            AND sequence<=?1 ORDER BY sequence DESC LIMIT 1) AS sequence
        UNION ALL
        SELECT (SELECT sequence FROM events INDEXED BY events_aggregate_heads_idx
            WHERE observation_aggregate_heads>0 AND observation_aggregate_heads=3
            AND sequence<=?1 ORDER BY sequence DESC LIMIT 1)
    )))";

pub(super) const EXECUTION: &str = "SELECT observation_execution FROM events WHERE sequence=(
    SELECT max(sequence) FROM (
        SELECT (SELECT sequence FROM events INDEXED BY events_attempt_heads_idx
            WHERE observation_attempt_heads>0 AND attempt_id=?1 AND observation_attempt_heads=1
            AND sequence<=?2 ORDER BY sequence DESC LIMIT 1) AS sequence
        UNION ALL
        SELECT (SELECT sequence FROM events INDEXED BY events_attempt_heads_idx
            WHERE observation_attempt_heads>0 AND attempt_id=?1 AND observation_attempt_heads=3
            AND sequence<=?2 ORDER BY sequence DESC LIMIT 1)
    ))";

pub(super) const MEMBERS: &str = "SELECT observation_members FROM events WHERE sequence=(
    SELECT max(sequence) FROM (
        SELECT (SELECT sequence FROM events INDEXED BY events_attempt_heads_idx
            WHERE observation_attempt_heads>0 AND attempt_id=?1 AND observation_attempt_heads=2
            AND sequence<=?2 ORDER BY sequence DESC LIMIT 1) AS sequence
        UNION ALL
        SELECT (SELECT sequence FROM events INDEXED BY events_attempt_heads_idx
            WHERE observation_attempt_heads>0 AND attempt_id=?1 AND observation_attempt_heads=3
            AND sequence<=?2 ORDER BY sequence DESC LIMIT 1)
    ))";

const COLUMNS: &[(&str, &str)] = &[
    (
        "observation_aggregate_heads",
        "observation_aggregate_heads INTEGER GENERATED ALWAYS AS ((observation_trace_total IS NOT NULL)+2*(observation_statistics IS NOT NULL)) VIRTUAL",
    ),
    (
        "observation_attempt_heads",
        "observation_attempt_heads INTEGER GENERATED ALWAYS AS ((observation_execution IS NOT NULL)+2*(observation_members IS NOT NULL)) VIRTUAL",
    ),
];
const INDEXES: &[(&str, &str)] = &[
    (
        "events_aggregate_heads_idx",
        "CREATE INDEX events_aggregate_heads_idx ON events(observation_aggregate_heads,sequence) WHERE observation_aggregate_heads>0",
    ),
    (
        "events_attempt_heads_idx",
        "CREATE INDEX events_attempt_heads_idx ON events(attempt_id,observation_attempt_heads,sequence) WHERE observation_attempt_heads>0",
    ),
];

pub(super) fn verify_schema(connection: &Connection) -> Result<(), ConversationStoreError> {
    let events: String = connection
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE type='table' AND name='events'",
            [],
            |row| row.get(0),
        )
        .map_err(|error| storage(error.to_string()))?;
    for &(name, definition) in COLUMNS {
        let virtual_integer: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_xinfo('events') WHERE name=?1 AND type='INTEGER' AND hidden=2)", [name], |row| row.get(0)).map_err(|error| storage(error.to_string()))?;
        if !virtual_integer || !events.contains(definition) {
            return Err(ConversationStoreError::IncompatibleSchema(format!(
                "invalid derived event head column {name}"
            )));
        }
    }
    for &(name, definition) in INDEXES {
        let actual: Option<String> = connection
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE type='index' AND name=?1",
                [name],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| storage(error.to_string()))?;
        if actual.as_deref() != Some(definition) {
            return Err(ConversationStoreError::IncompatibleSchema(format!(
                "invalid derived event head index {name}"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
