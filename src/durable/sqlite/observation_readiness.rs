//! Process-local owner publication evidence attached to existing native J rows.
//! This index is neither client acknowledgement nor model-byte authorization.
use super::{Connection, ConversationStoreError, params, storage};
use crate::events::types::RuntimeEventEnvelope;

pub(super) fn insert(
    connection: &Connection,
    event: &RuntimeEventEnvelope,
) -> Result<(), ConversationStoreError> {
    let ready = !crate::runtime::observation::trace_fact_requires_publication(&event.event);
    connection
        .execute(
            "INSERT INTO event_observation_readiness(sequence,ready) VALUES(?1,?2)",
            params![event.sequence, ready],
        )
        .map_err(|error| storage(error.to_string()))?;
    connection
        .execute(
            "UPDATE events SET observation_readiness_missing=0 WHERE sequence=?1",
            [event.sequence],
        )
        .map_err(|error| storage(error.to_string()))?;
    Ok(())
}

pub(super) fn recover_journal(connection: &Connection) -> Result<(), ConversationStoreError> {
    // Only a newly installed native Runtime may recover previously committed
    // durable history. Read-only inspection and unrelated live owners cannot.
    connection.execute("INSERT OR IGNORE INTO event_observation_readiness(sequence,ready,direct) SELECT sequence,1,0 FROM events INDEXED BY events_observation_readiness_missing_idx WHERE observation_readiness_missing=1",[]).map_err(|error|storage(error.to_string()))?;
    connection.execute("UPDATE events SET observation_readiness_missing=0 WHERE observation_readiness_missing=1",[]).map_err(|error|storage(error.to_string()))?;
    Ok(())
}

pub(super) fn mark(
    connection: &Connection,
    first: u64,
    last: u64,
) -> Result<(), ConversationStoreError> {
    if last == 0 && first == 1 {
        return Ok(());
    }
    if first == 0 || first > last || last > super::journal_prefix(connection)? {
        return Err(ConversationStoreError::InvalidReference(
            "observation readiness range exceeds native Journal".into(),
        ));
    }
    let missing:bool=connection.query_row("SELECT EXISTS(SELECT 1 FROM events INDEXED BY events_observation_readiness_missing_idx WHERE observation_readiness_missing=1 AND sequence>=?1 AND sequence<=?2)",params![first,last],|row|row.get(0)).map_err(|error|storage(error.to_string()))?;
    if missing {
        return Err(storage("native observation readiness evidence unavailable"));
    }
    // The partial index visits only unpublished owner holes, never full events.
    connection.execute("UPDATE event_observation_readiness SET ready=1 WHERE ready=0 AND sequence>=?1 AND sequence<=?2",params![first,last]).map_err(|error|storage(error.to_string()))?;
    Ok(())
}

pub(super) fn mark_direct(
    connection: &Connection,
    first: u64,
    last: u64,
) -> Result<(), ConversationStoreError> {
    mark(connection, first, last)?;
    connection
        .execute(
            "UPDATE event_observation_readiness SET direct=1 WHERE sequence>=?1 AND sequence<=?2",
            params![first, last],
        )
        .map_err(|error| storage(error.to_string()))?;
    Ok(())
}

pub(super) fn frontier(
    connection: &Connection,
    after: u64,
    through: u64,
) -> Result<u64, ConversationStoreError> {
    if after > through || through > super::journal_prefix(connection)? {
        return Err(ConversationStoreError::InvalidReference(
            "invalid observation readiness cut".into(),
        ));
    }
    let hole: Option<u64> = connection.query_row("SELECT MIN(sequence) FROM event_observation_readiness WHERE ready=0 AND sequence>?1 AND sequence<=?2",params![after,through],|row|row.get(0)).map_err(|error|storage(error.to_string()))?;
    let missing:Option<u64>=connection.query_row("SELECT MIN(sequence) FROM events INDEXED BY events_observation_readiness_missing_idx WHERE observation_readiness_missing=1 AND sequence>?1 AND sequence<=?2",params![after,through],|row|row.get(0)).map_err(|error|storage(error.to_string()))?;
    Ok(hole
        .into_iter()
        .chain(missing)
        .min()
        .map_or(through, |sequence| sequence - 1))
}

fn transcript_head(connection: &Connection) -> Result<u64, ConversationStoreError> {
    connection
        .query_row(
            "SELECT next_transcript_position FROM rustx_store WHERE id=1",
            [],
            |row| row.get(0),
        )
        .map_err(|error| storage(error.to_string()))
}
pub(super) fn recover_transcript(connection: &Connection) -> Result<(), ConversationStoreError> {
    connection.execute("INSERT OR IGNORE INTO transcript_observation_readiness(position,ready) SELECT position,1 FROM transcript_order INDEXED BY transcript_observation_readiness_missing_idx WHERE observation_readiness_missing=1", []).map_err(|error| storage(error.to_string()))?;
    connection.execute("UPDATE transcript_order SET observation_readiness_missing=0 WHERE observation_readiness_missing=1", []).map_err(|error| storage(error.to_string()))?;
    connection
        .execute(
            "UPDATE transcript_observation_readiness SET ready=1 WHERE ready=0",
            [],
        )
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
    let missing: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM transcript_order WHERE position=?1 AND observation_readiness_missing=1)", [cursor], |row| row.get(0)).map_err(|error| storage(error.to_string()))?;
    if missing {
        return Err(storage("native transcript readiness evidence unavailable"));
    }
    // Missing positions were removed by their existing Pending Inbound owner.
    connection
        .execute(
            "UPDATE transcript_observation_readiness SET ready=1 WHERE position=?1 AND ready=0",
            [cursor],
        )
        .map_err(|error| storage(error.to_string()))?;
    Ok(())
}
pub(super) fn mark_canonical(
    connection: &Connection,
    first: u64,
    last: u64,
) -> Result<(), ConversationStoreError> {
    let ledger: u64 = connection
        .query_row(
            "SELECT COALESCE(MAX(position),0) FROM message_ledger",
            [],
            |row| row.get(0),
        )
        .map_err(|error| storage(error.to_string()))?;
    if first == 0 || first > last || last > ledger {
        return Err(ConversationStoreError::InvalidReference(
            "invalid canonical transcript publication range".into(),
        ));
    }
    // Numeric L selects only this C batch, excluding unrelated Audit or
    // interaction T positions allocated between its canonical messages.
    let missing: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM message_ledger m JOIN transcript_order t ON t.position=m.native_transcript WHERE m.position>=?1 AND m.position<=?2 AND t.observation_readiness_missing=1)", params![first,last], |row| row.get(0)).map_err(|error| storage(error.to_string()))?;
    if missing {
        return Err(storage(
            "native canonical transcript readiness evidence unavailable",
        ));
    }
    connection.execute("UPDATE transcript_observation_readiness SET ready=1 WHERE ready=0 AND position IN(SELECT native_transcript FROM message_ledger WHERE position>=?1 AND position<=?2)",params![first,last]).map_err(|error|storage(error.to_string()))?;
    Ok(())
}
pub(super) fn transcript_frontier(
    connection: &Connection,
    after: u64,
    through: u64,
) -> Result<u64, ConversationStoreError> {
    if after > through || through > transcript_head(connection)? {
        return Err(ConversationStoreError::InvalidReference(
            "invalid native transcript publication cut".into(),
        ));
    }
    let hole:Option<u64>=connection.query_row("SELECT MIN(position) FROM transcript_observation_readiness WHERE ready=0 AND position>?1 AND position<=?2",params![after,through],|row|row.get(0)).map_err(|error|storage(error.to_string()))?;
    let missing: Option<u64> = connection.query_row("SELECT MIN(position) FROM transcript_order INDEXED BY transcript_observation_readiness_missing_idx WHERE observation_readiness_missing=1 AND position>?1 AND position<=?2", params![after,through], |row| row.get(0)).map_err(|error| storage(error.to_string()))?;
    Ok(hole
        .into_iter()
        .chain(missing)
        .min()
        .map_or(through, |position| position - 1))
}
