//! Sparse negative evidence over current event cells, maintained by SQLite itself.
//!
//! Each group replaces three independently updated B-trees. Readers enumerate the
//! four exact mask values containing their bit before seeking a sequence range,
//! so unrelated failures cannot turn a bounded check into a historical scan.
//! VIRTUAL columns add no row payload and cannot be altered independently of the
//! required flags and optional evidence they describe.
use super::{Connection, ConversationStoreError, OptionalExtension, storage};

const COLUMNS: &[(&str, &str)] = &[
    (
        "observation_aggregate_holes",
        "observation_aggregate_holes INTEGER GENERATED ALWAYS AS (CASE WHEN observation_trace_fact_required=1 AND observation_trace_fact IS NULL THEN 1 ELSE 0 END+CASE WHEN observation_trace_required=1 AND observation_trace_total IS NULL THEN 2 ELSE 0 END+CASE WHEN observation_statistics_required=1 AND observation_statistics IS NULL THEN 4 ELSE 0 END) VIRTUAL",
    ),
    (
        "observation_owner_holes",
        "observation_owner_holes INTEGER GENERATED ALWAYS AS (CASE WHEN observation_execution_required=1 AND observation_execution IS NULL THEN 1 ELSE 0 END+CASE WHEN observation_turn_required=1 AND observation_turn_header IS NULL THEN 2 ELSE 0 END+CASE WHEN observation_members_required=1 AND observation_members IS NULL THEN 4 ELSE 0 END) VIRTUAL",
    ),
];
const INDEXES: &[(&str, &str)] = &[
    (
        "events_aggregate_holes_idx",
        "CREATE INDEX events_aggregate_holes_idx ON events(observation_aggregate_holes,sequence) WHERE observation_aggregate_holes>0",
    ),
    (
        "events_owner_holes_idx",
        "CREATE INDEX events_owner_holes_idx ON events(observation_owner_holes,sequence) WHERE observation_owner_holes>0",
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
                "invalid derived event hole column {name}"
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
                "invalid derived event hole index {name}"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
