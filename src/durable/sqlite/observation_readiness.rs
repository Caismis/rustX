//! Connection-owned publication evidence over already committed native rows.
//! A new connection starts at its committed J/T baseline, without copying old
//! history. Live receipts share this connection's TEMP epoch with its readers.
//! Installation progress never belongs in the durable database or its WAL.
use super::{Connection, ConversationStoreError, params, storage};
use crate::events::types::RuntimeEventEnvelope;

pub(super) fn install(connection: &Connection) -> Result<(), ConversationStoreError> {
    connection.execute_batch(
        "PRAGMA temp_store=MEMORY;
         CREATE TEMP TABLE observation_readiness_epoch(
             id INTEGER PRIMARY KEY CHECK(id=1),
             journal INTEGER NOT NULL CHECK(journal>=0),
             transcript INTEGER NOT NULL CHECK(transcript>=0)
         );
         INSERT INTO observation_readiness_epoch
             SELECT 1,next_event_sequence,next_transcript_position FROM main.rustx_store WHERE id=1;
         CREATE TEMP TABLE event_observation_readiness(
             sequence INTEGER PRIMARY KEY,
             ready INTEGER NOT NULL CHECK(ready IN(0,1)),
             direct INTEGER NOT NULL DEFAULT 0 CHECK(direct IN(0,1))
         );
         CREATE INDEX temp.event_observation_readiness_holes_idx
             ON event_observation_readiness(sequence) WHERE ready=0;
         CREATE TEMP TABLE transcript_observation_readiness(
             position INTEGER PRIMARY KEY,
             ready INTEGER NOT NULL CHECK(ready IN(0,1))
         );
         CREATE INDEX temp.transcript_observation_readiness_holes_idx
             ON transcript_observation_readiness(position) WHERE ready=0;
         CREATE TEMP TRIGGER event_readiness_source_deleted AFTER DELETE ON main.events
             BEGIN
                 DELETE FROM event_observation_readiness WHERE sequence=OLD.sequence;
             END;
         CREATE TEMP TRIGGER transcript_readiness_source_deleted AFTER DELETE ON main.transcript_order
             BEGIN
                 DELETE FROM transcript_observation_readiness WHERE position=OLD.position;
             END;",
    ).map_err(|error| storage(format!("install connection publication epoch: {error}")))
}

pub(super) fn insert(
    connection: &Connection,
    event: &RuntimeEventEnvelope,
) -> Result<(), ConversationStoreError> {
    let ready = !crate::runtime::observation::trace_fact_requires_publication(&event.event);
    connection
        .prepare_cached("INSERT INTO event_observation_readiness(sequence,ready) VALUES(?1,?2)")
        .map_err(|error| storage(error.to_string()))?
        .execute(params![event.sequence, ready])
        .map_err(|error| storage(error.to_string()))?;
    connection
        .prepare_cached("UPDATE events SET observation_readiness_missing=0 WHERE sequence=?1")
        .map_err(|error| storage(error.to_string()))?
        .execute([event.sequence])
        .map_err(|error| storage(error.to_string()))?;
    Ok(())
}

pub(super) fn recover_journal(connection: &Connection) -> Result<(), ConversationStoreError> {
    // A newly installed Runtime owns committed history, not the old owner's
    // direct-delivery claims. Only this epoch's temporary rows are retired.
    connection.execute_batch("UPDATE observation_readiness_epoch SET journal=(SELECT next_event_sequence FROM main.rustx_store WHERE id=1) WHERE id=1; DELETE FROM event_observation_readiness;")
        .map_err(|error| storage(error.to_string()))?;
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
        .map_err(|error| storage(error.to_string()))?;
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
    connection.prepare_cached("UPDATE event_observation_readiness SET ready=1 WHERE ready=0 AND sequence>=?1 AND sequence<=?2")
        .and_then(|mut statement| statement.execute(params![first,last]))
        .map_err(|error| storage(error.to_string()))?;
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
    // The caller's release transaction owns both flags. Repeated publication
    // performs no row mutation, while every not-yet-published row still reaches
    // the direct/ready constraint and trigger boundary atomically.
    connection.prepare_cached("UPDATE event_observation_readiness SET ready=1,direct=1 WHERE sequence>=?1 AND sequence<=?2 AND (ready=0 OR direct=0)")
        .and_then(|mut statement| statement.execute(params![first,last]))
        .map_err(|error| storage(error.to_string()))?;
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
    let (head, hole, missing): (i64, Option<u64>, Option<u64>) = connection.prepare_cached("SELECT next_event_sequence,(SELECT MIN(sequence) FROM event_observation_readiness WHERE ready=0 AND sequence>?1 AND sequence<=?2),(SELECT MIN(sequence) FROM events INDEXED BY events_observation_readiness_missing_idx WHERE observation_readiness_missing=1 AND sequence>epoch.journal AND sequence>?1 AND sequence<=?2) FROM rustx_store,observation_readiness_epoch epoch WHERE rustx_store.id=1 AND epoch.id=1")
        .and_then(|mut statement| statement.query_row(params![after,through], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))))
        .map_err(|error| storage(error.to_string()))?;
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
        .map_err(|error| storage(error.to_string()))?
        .query_row([], |row| row.get(0))
        .map_err(|error| storage(error.to_string()))
}
pub(super) fn recover_transcript(connection: &Connection) -> Result<(), ConversationStoreError> {
    connection.execute_batch("UPDATE observation_readiness_epoch SET transcript=(SELECT next_transcript_position FROM main.rustx_store WHERE id=1) WHERE id=1; DELETE FROM transcript_observation_readiness;")
        .map_err(|error| storage(error.to_string()))?;
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
    let missing: bool = connection.prepare_cached("SELECT EXISTS(SELECT 1 FROM transcript_order WHERE position=?1 AND observation_readiness_missing=1 AND position>(SELECT transcript FROM observation_readiness_epoch WHERE id=1))").map_err(|error| storage(error.to_string()))?.query_row([cursor], |row| row.get(0)).map_err(|error| storage(error.to_string()))?;
    if missing {
        return Err(storage("native transcript readiness evidence unavailable"));
    }
    // Missing positions were removed by their existing Pending Inbound owner.
    connection
        .prepare_cached(
            "UPDATE transcript_observation_readiness SET ready=1 WHERE position=?1 AND ready=0",
        )
        .map_err(|error| storage(error.to_string()))?
        .execute([cursor])
        .map_err(|error| storage(error.to_string()))?;
    Ok(())
}
pub(super) fn mark_canonical(
    connection: &Connection,
    first: u64,
    last: u64,
) -> Result<(), ConversationStoreError> {
    let ledger: u64 = connection
        .prepare_cached("SELECT COALESCE(MAX(position),0) FROM message_ledger")
        .map_err(|error| storage(error.to_string()))?
        .query_row([], |row| row.get(0))
        .map_err(|error| storage(error.to_string()))?;
    if first == 0 || first > last || last > ledger {
        return Err(ConversationStoreError::InvalidReference(
            "invalid canonical transcript publication range".into(),
        ));
    }
    // Numeric L selects only this C batch, excluding unrelated Audit or
    // interaction T positions allocated between its canonical messages.
    let missing: bool = connection.prepare_cached("SELECT EXISTS(SELECT 1 FROM message_ledger m JOIN transcript_order t ON t.position=m.native_transcript WHERE m.position>=?1 AND m.position<=?2 AND (t.observation_readiness_missing=1 AND t.position>(SELECT transcript FROM observation_readiness_epoch WHERE id=1)))").map_err(|error| storage(error.to_string()))?.query_row(params![first,last], |row| row.get(0)).map_err(|error| storage(error.to_string()))?;
    if missing {
        return Err(storage(
            "native canonical transcript readiness evidence unavailable",
        ));
    }
    connection.prepare_cached("UPDATE transcript_observation_readiness SET ready=1 WHERE ready=0 AND position IN(SELECT native_transcript FROM message_ledger WHERE position>=?1 AND position<=?2)").map_err(|error| storage(error.to_string()))?.execute(params![first,last]).map_err(|error|storage(error.to_string()))?;
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
    let (head, hole, missing): (u64, Option<u64>, Option<u64>) = connection.prepare_cached("SELECT next_transcript_position,(SELECT MIN(position) FROM transcript_observation_readiness WHERE ready=0 AND position>?1 AND position<=?2),(SELECT MIN(position) FROM transcript_order INDEXED BY transcript_observation_readiness_missing_idx WHERE observation_readiness_missing=1 AND position>epoch.transcript AND position>?1 AND position<=?2) FROM rustx_store,observation_readiness_epoch epoch WHERE rustx_store.id=1 AND epoch.id=1")
        .and_then(|mut statement| statement.query_row(params![after,through], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))))
        .map_err(|error| storage(error.to_string()))?;
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
