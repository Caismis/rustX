//! The durable recovery frontier on the store root row.
//!
//! Every native Journal append folds its fact into the frontier inside the
//! same transaction, so the frontier and the Journal never disagree after a
//! commit or a crash. A frontier that does not directly precede an appended
//! fact (a Journal row written by any other path) is left where it is: it
//! remains an exact fold through its own sequence, and recovery folds the
//! rows after it.

use crate::durable::ConversationStoreError;
use crate::durable::sqlite::statements::FixedStatements;
use crate::durable::sqlite::{seq_to_i64, sqlite_error, storage};
use crate::events::types::RuntimeEventEnvelope;
use crate::runtime::identity::ConversationId;
use crate::runtime::recovery::RecoveryFrontier;
use rusqlite::{Connection, OptionalExtension};

fn read(connection: &Connection) -> Result<RecoveryFrontier, ConversationStoreError> {
    let bytes: Option<Vec<u8>> = connection
        .query_row_cached(
            "SELECT recovery_frontier FROM rustx_store WHERE id=1",
            [],
            |row| row.get(0),
        )
        .map_err(|error| sqlite_error(&error, format!("read recovery frontier: {error}")))?;
    bytes.map_or_else(
        || Ok(RecoveryFrontier::default()),
        |bytes| {
            RecoveryFrontier::decode(bytes.as_slice()).map_err(|error| {
                ConversationStoreError::InvalidReference(format!(
                    "recovery frontier is malformed: {error}"
                ))
            })
        },
    )
}

/// Folds one just-inserted native Journal fact into the frontier.
pub(super) fn advance(
    connection: &Connection,
    conversation: &ConversationId,
    event: &RuntimeEventEnvelope,
) -> Result<(), ConversationStoreError> {
    let mut frontier = read(connection)?;
    if frontier.through().checked_add(1) != Some(event.sequence) {
        return Ok(());
    }
    let owner = match RecoveryFrontier::owner_query(event) {
        Some((message, call)) => {
            super::canonical::tool_result_owner(connection, message, call, event.sequence)?
        }
        None => None,
    };
    frontier.fold(conversation, event, owner.as_ref());
    let bytes = frontier
        .encode()
        .map_err(|error| storage(format!("encode recovery frontier: {error}")))?;
    connection
        .execute_cached(
            "UPDATE rustx_store SET recovery_frontier=?1 WHERE id=1",
            [bytes.as_slice()],
        )
        .map_err(|error| sqlite_error(&error, format!("write recovery frontier: {error}")))?;
    Ok(())
}

/// Reads the frontier and verifies the Journal row it claims to cover.
pub(super) fn authenticated(
    connection: &Connection,
) -> Result<RecoveryFrontier, ConversationStoreError> {
    let frontier = read(connection)?;
    let disagrees = |detail: &str| {
        ConversationStoreError::InvalidReference(format!(
            "recovery frontier disagrees with the Journal: {detail}"
        ))
    };
    let allocated: i64 = connection
        .query_row_cached(
            "SELECT next_event_sequence FROM rustx_store WHERE id=1",
            [],
            |row| row.get(0),
        )
        .map_err(|error| sqlite_error(&error, format!("read event sequence: {error}")))?;
    if seq_to_i64(frontier.through())? > allocated {
        return Err(disagrees("frontier is ahead of the Journal"));
    }
    match (frontier.through(), frontier.through_event()) {
        (0, None) => {}
        (0, Some(_)) | (_, None) => return Err(disagrees("anchor identity")),
        (through, Some(expected)) => {
            let stored: Option<String> = connection
                .query_row_cached(
                    "SELECT event_id FROM events WHERE sequence=?1",
                    [seq_to_i64(through)?],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|error| {
                    sqlite_error(&error, format!("read recovery frontier anchor: {error}"))
                })?;
            match stored {
                None => return Err(disagrees("anchor row is missing")),
                Some(stored) if stored != expected.as_str() => {
                    return Err(disagrees("anchor row identity"));
                }
                Some(_) => {}
            }
        }
    }
    Ok(frontier)
}

#[cfg(test)]
mod tests;
