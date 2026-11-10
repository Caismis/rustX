//! Connection-owned publication evidence over already committed native rows.
//! A new connection starts at its committed J/T baseline, without copying old
//! history. Live receipts share this connection's TEMP epoch with its readers.
//! Installation progress never belongs in the durable database or its WAL.
use rusqlite::OptionalExtension;

use super::statements::FixedStatements;
use super::{Connection, ConversationStoreError, params, storage};
use crate::events::types::RuntimeEventEnvelope;

// Keep the trigger definitions shared with the prepared-insert guard.
pub(super) const EVENT_DELETION: &str =
    "CREATE TRIGGER event_readiness_source_deleted AFTER DELETE ON main.events
             BEGIN
                 DELETE FROM event_observation_readiness WHERE sequence=OLD.sequence;
                 INSERT INTO event_observation_direct(lo,hi)
                     SELECT OLD.sequence+1,hi FROM event_observation_direct
                     WHERE lo=(SELECT MAX(lo) FROM event_observation_direct WHERE lo<=OLD.sequence)
                     AND hi>OLD.sequence;
                 DELETE FROM event_observation_direct WHERE lo=OLD.sequence;
                 UPDATE event_observation_direct SET hi=OLD.sequence-1
                     WHERE lo=(SELECT MAX(lo) FROM event_observation_direct WHERE lo<OLD.sequence)
                     AND hi>=OLD.sequence;
             END";
pub(super) const TRANSCRIPT_DELETION: &str =
    "CREATE TRIGGER transcript_readiness_source_deleted AFTER DELETE ON main.transcript_order
             BEGIN
                 DELETE FROM transcript_observation_readiness WHERE position=OLD.position;
             END";

pub(super) fn install(connection: &Connection) -> Result<(), ConversationStoreError> {
    connection
        .execute_batch(
            "PRAGMA temp_store=MEMORY;
         CREATE TEMP TABLE observation_readiness_epoch(
             id INTEGER PRIMARY KEY CHECK(id=1),
             journal INTEGER NOT NULL CHECK(journal>=0),
             transcript INTEGER NOT NULL CHECK(transcript>=0)
         );
         INSERT INTO observation_readiness_epoch
             SELECT 1,next_event_sequence,next_transcript_position FROM main.rustx_store WHERE id=1;
         CREATE TEMP TABLE event_observation_readiness(sequence INTEGER PRIMARY KEY);
         CREATE TEMP TABLE transcript_observation_readiness(position INTEGER PRIMARY KEY);
         CREATE TEMP TABLE event_observation_direct(
             lo INTEGER PRIMARY KEY CHECK(lo>0),
             hi INTEGER NOT NULL CHECK(hi>=lo)
         );",
        )
        .and_then(|()| {
            connection.execute_batch(&format!(
                "{}; {};",
                EVENT_DELETION.replacen("CREATE TRIGGER", "CREATE TEMP TRIGGER", 1),
                TRANSCRIPT_DELETION.replacen("CREATE TRIGGER", "CREATE TEMP TRIGGER", 1),
            ))
        })
        .map_err(|error| {
            crate::durable::sqlite::sqlite_error(
                &error,
                format!("install connection publication epoch: {error}"),
            )
        })
}

pub(super) fn insert(
    connection: &Connection,
    event: &RuntimeEventEnvelope,
) -> Result<(), ConversationStoreError> {
    prepare(connection, event)?;
    connection
        .prepare_cached("UPDATE events SET observation_readiness_missing=0 WHERE sequence=?1")
        .map_err(|error| crate::durable::sqlite::sqlite_error(&error, error.to_string()))?
        .execute([event.sequence])
        .map_err(|error| crate::durable::sqlite::sqlite_error(&error, error.to_string()))?;
    Ok(())
}

/// Prepare only this connection's publication evidence, before the native row.
/// The caller includes its success in that row's initial missing marker.
pub(super) fn prepare(
    connection: &Connection,
    event: &RuntimeEventEnvelope,
) -> Result<(), ConversationStoreError> {
    if !crate::runtime::observation::trace_fact_requires_publication(&event.event) {
        return Ok(());
    }
    connection
        .prepare_cached("INSERT INTO event_observation_readiness(sequence) VALUES(?1)")
        .map_err(|error| crate::durable::sqlite::sqlite_error(&error, error.to_string()))?
        .execute([event.sequence])
        .map_err(|error| crate::durable::sqlite::sqlite_error(&error, error.to_string()))?;
    Ok(())
}

pub(super) fn recover_journal(connection: &Connection) -> Result<(), ConversationStoreError> {
    // A newly installed Runtime owns committed history, not the old owner's
    // direct-delivery claims. Only this epoch's temporary rows are retired.
    connection.execute_batch("UPDATE observation_readiness_epoch SET journal=(SELECT next_event_sequence FROM main.rustx_store WHERE id=1) WHERE id=1; DELETE FROM event_observation_readiness; DELETE FROM event_observation_direct;")
        .map_err(|error| crate::durable::sqlite::sqlite_error(&error,error.to_string()))?;
    Ok(())
}

fn validate_mark(
    connection: &Connection,
    first: u64,
    last: u64,
) -> Result<bool, ConversationStoreError> {
    if last == 0 && first == 1 {
        return Ok(false);
    }
    if first == 0 || first > last {
        return Err(ConversationStoreError::InvalidReference(
            "observation readiness range exceeds native Journal".into(),
        ));
    }
    let (head, missing): (i64, bool) = connection.prepare_cached("SELECT next_event_sequence,EXISTS(SELECT 1 FROM events INDEXED BY events_observation_readiness_missing_idx WHERE observation_readiness_missing=1 AND sequence>epoch.journal AND sequence>=?1 AND sequence<=?2) FROM rustx_store,observation_readiness_epoch epoch WHERE rustx_store.id=1 AND epoch.id=1")
        .and_then(|mut statement| statement.query_row(params![first,last], |row| Ok((row.get(0)?, row.get(1)?))))
        .map_err(|error| crate::durable::sqlite::sqlite_error(&error,error.to_string()))?;
    if last > super::sequence_from_i64(head)? {
        return Err(ConversationStoreError::InvalidReference(
            "observation readiness range exceeds native Journal".into(),
        ));
    }
    if missing {
        return Err(storage("native observation readiness evidence unavailable"));
    }
    Ok(true)
}
pub(super) fn mark(
    connection: &Connection,
    first: u64,
    last: u64,
) -> Result<(), ConversationStoreError> {
    if !validate_mark(connection, first, last)? {
        return Ok(());
    }
    connection
        .prepare_cached(
            "DELETE FROM event_observation_readiness WHERE sequence>=?1 AND sequence<=?2",
        )
        .and_then(|mut statement| statement.execute(params![first, last]))
        .map_err(|error| crate::durable::sqlite::sqlite_error(&error, error.to_string()))?;
    Ok(())
}
pub(super) fn mark_direct(
    connection: &Connection,
    first: u64,
    last: u64,
) -> Result<(), ConversationStoreError> {
    if !validate_mark(connection, first, last)? {
        return Ok(());
    }
    // Only committed positions in this epoch can acquire direct provenance.
    // Enumerate the release span, never the already released history. A
    // missing native row divides the span so later reads cannot inherit it.
    let mut statement = connection
        .prepare_cached(
            "SELECT sequence FROM events
         WHERE sequence BETWEEN ?1 AND ?2 AND observation_readiness_missing=0
           AND sequence>(SELECT journal FROM observation_readiness_epoch WHERE id=1)
         ORDER BY sequence",
        )
        .map_err(sql_error)?;
    let mut rows = statement.query(params![first, last]).map_err(sql_error)?;
    let mut range: Option<(i64, i64)> = None;
    while let Some(row) = rows.next().map_err(sql_error)? {
        let sequence: i64 = row.get(0).map_err(sql_error)?;
        if let Some((lo, hi)) = range {
            if hi.checked_add(1) == Some(sequence) {
                range = Some((lo, sequence));
                continue;
            }
            merge_direct_range(connection, lo, hi)?;
        }
        range = Some((sequence, sequence));
    }
    if let Some((lo, hi)) = range {
        merge_direct_range(connection, lo, hi)?;
    }
    connection
        .execute_cached(
            "DELETE FROM event_observation_readiness WHERE sequence BETWEEN ?1 AND ?2",
            params![first, last],
        )
        .map_err(sql_error)?;
    Ok(())
}

#[allow(clippy::needless_pass_by_value)] // Adapter for rusqlite Result::map_err.
fn sql_error(error: rusqlite::Error) -> ConversationStoreError {
    super::sqlite_error(&error, error.to_string())
}

/// Union one contiguous release with its adjacent direct ranges. The release
/// transaction owns both the union and deletion of pending holes.
fn merge_direct_range(
    connection: &Connection,
    mut lo: i64,
    mut hi: i64,
) -> Result<(), ConversationStoreError> {
    let left: Option<(i64, i64)> = connection
        .query_row_cached(
            "SELECT lo,hi FROM event_observation_direct WHERE lo<=?1 ORDER BY lo DESC LIMIT 1",
            [lo],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(sql_error)?;
    if let Some((start, end)) = left {
        if end >= hi {
            return Ok(());
        }
        if end >= lo - 1 {
            lo = start;
        }
    }
    let right = hi.saturating_add(1);
    let last: Option<i64> = connection.query_row_cached(
        "SELECT hi FROM event_observation_direct WHERE lo>?1 AND lo<=?2 ORDER BY lo DESC LIMIT 1",
        params![lo,right], |row| row.get(0),
    ).optional().map_err(sql_error)?;
    if let Some(last) = last {
        hi = hi.max(last);
        connection
            .execute_cached(
                "DELETE FROM event_observation_direct WHERE lo>?1 AND lo<=?2",
                params![lo, right],
            )
            .map_err(sql_error)?;
    }
    connection
        .execute_cached(
            "INSERT INTO event_observation_direct(lo,hi) VALUES(?1,?2)
         ON CONFLICT(lo) DO UPDATE SET hi=excluded.hi",
            params![lo, hi],
        )
        .map_err(sql_error)?;
    Ok(())
}

pub(super) fn frontier(
    connection: &Connection,
    after: u64,
    through: u64,
) -> Result<u64, ConversationStoreError> {
    if after > through {
        return Err(ConversationStoreError::InvalidReference(
            "invalid observation readiness cut".into(),
        ));
    }
    let (head, hole, missing): (i64, Option<u64>, Option<u64>) = connection.prepare_cached("SELECT next_event_sequence,(SELECT MIN(sequence) FROM event_observation_readiness WHERE sequence>?1 AND sequence<=?2),(SELECT MIN(sequence) FROM events INDEXED BY events_observation_readiness_missing_idx WHERE observation_readiness_missing=1 AND sequence>epoch.journal AND sequence>?1 AND sequence<=?2) FROM rustx_store,observation_readiness_epoch epoch WHERE rustx_store.id=1 AND epoch.id=1")
        .and_then(|mut statement| statement.query_row(params![after,through], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))))
        .map_err(|error| crate::durable::sqlite::sqlite_error(&error,error.to_string()))?;
    if through > super::sequence_from_i64(head)? {
        return Err(ConversationStoreError::InvalidReference(
            "invalid observation readiness cut".into(),
        ));
    }
    Ok(hole
        .into_iter()
        .chain(missing)
        .min()
        .map_or(through, |sequence| sequence - 1))
}

fn transcript_head(connection: &Connection) -> Result<u64, ConversationStoreError> {
    connection
        .prepare_cached("SELECT next_transcript_position FROM rustx_store WHERE id=1")
        .map_err(|error| crate::durable::sqlite::sqlite_error(&error, error.to_string()))?
        .query_row([], |row| row.get(0))
        .map_err(|error| crate::durable::sqlite::sqlite_error(&error, error.to_string()))
}
pub(super) fn recover_transcript(connection: &Connection) -> Result<(), ConversationStoreError> {
    connection.execute_batch("UPDATE observation_readiness_epoch SET transcript=(SELECT next_transcript_position FROM main.rustx_store WHERE id=1) WHERE id=1; DELETE FROM transcript_observation_readiness;")
        .map_err(|error| crate::durable::sqlite::sqlite_error(&error,error.to_string()))?;
    Ok(())
}
pub(super) fn mark_transcript(
    connection: &Connection,
    cursor: u64,
) -> Result<(), ConversationStoreError> {
    if cursor == 0 || cursor > transcript_head(connection)? {
        return Err(ConversationStoreError::InvalidReference(
            "invalid native transcript publication position".into(),
        ));
    }
    let missing: bool = connection.prepare_cached("SELECT EXISTS(SELECT 1 FROM transcript_order WHERE position=?1 AND observation_readiness_missing=1 AND position>(SELECT transcript FROM observation_readiness_epoch WHERE id=1))").map_err(|error| crate::durable::sqlite::sqlite_error(&error,error.to_string()))?.query_row([cursor], |row| row.get(0)).map_err(|error| crate::durable::sqlite::sqlite_error(&error,error.to_string()))?;
    if missing {
        return Err(storage("native transcript readiness evidence unavailable"));
    }
    // Missing positions were removed by their existing Pending Inbound owner.
    connection
        .prepare_cached("DELETE FROM transcript_observation_readiness WHERE position=?1")
        .map_err(|error| crate::durable::sqlite::sqlite_error(&error, error.to_string()))?
        .execute([cursor])
        .map_err(|error| crate::durable::sqlite::sqlite_error(&error, error.to_string()))?;
    Ok(())
}
pub(super) fn mark_canonical(
    connection: &Connection,
    first: u64,
    last: u64,
) -> Result<(), ConversationStoreError> {
    let ledger: u64 = connection
        .prepare_cached("SELECT COALESCE(MAX(position),0) FROM message_ledger")
        .map_err(|error| crate::durable::sqlite::sqlite_error(&error, error.to_string()))?
        .query_row([], |row| row.get(0))
        .map_err(|error| crate::durable::sqlite::sqlite_error(&error, error.to_string()))?;
    if first == 0 || first > last || last > ledger {
        return Err(ConversationStoreError::InvalidReference(
            "invalid canonical transcript publication range".into(),
        ));
    }
    // Numeric L selects only this C batch, excluding unrelated Audit or
    // interaction T positions allocated between its canonical messages.
    let missing: bool = connection.prepare_cached("SELECT EXISTS(SELECT 1 FROM message_ledger m JOIN transcript_order t ON t.position=m.native_transcript WHERE m.position>=?1 AND m.position<=?2 AND (t.observation_readiness_missing=1 AND t.position>(SELECT transcript FROM observation_readiness_epoch WHERE id=1)))").map_err(|error| crate::durable::sqlite::sqlite_error(&error,error.to_string()))?.query_row(params![first,last], |row| row.get(0)).map_err(|error| crate::durable::sqlite::sqlite_error(&error,error.to_string()))?;
    if missing {
        return Err(storage(
            "native canonical transcript readiness evidence unavailable",
        ));
    }
    connection.prepare_cached("DELETE FROM transcript_observation_readiness WHERE position IN(SELECT native_transcript FROM message_ledger WHERE position>=?1 AND position<=?2)").map_err(|error| crate::durable::sqlite::sqlite_error(&error,error.to_string()))?.execute(params![first,last]).map_err(|error|crate::durable::sqlite::sqlite_error(&error,error.to_string()))?;
    Ok(())
}
pub(super) fn transcript_frontier(
    connection: &Connection,
    after: u64,
    through: u64,
) -> Result<u64, ConversationStoreError> {
    if after > through {
        return Err(ConversationStoreError::InvalidReference(
            "invalid native transcript publication cut".into(),
        ));
    }
    let (head, hole, missing): (u64, Option<u64>, Option<u64>) = connection.prepare_cached("SELECT next_transcript_position,(SELECT MIN(position) FROM transcript_observation_readiness WHERE position>?1 AND position<=?2),(SELECT MIN(position) FROM transcript_order INDEXED BY transcript_observation_readiness_missing_idx WHERE observation_readiness_missing=1 AND position>epoch.transcript AND position>?1 AND position<=?2) FROM rustx_store,observation_readiness_epoch epoch WHERE rustx_store.id=1 AND epoch.id=1")
        .and_then(|mut statement| statement.query_row(params![after,through], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))))
        .map_err(|error| crate::durable::sqlite::sqlite_error(&error,error.to_string()))?;
    if through > head {
        return Err(ConversationStoreError::InvalidReference(
            "invalid native transcript publication cut".into(),
        ));
    }
    Ok(hole
        .into_iter()
        .chain(missing)
        .min()
        .map_or(through, |position| position - 1))
}
