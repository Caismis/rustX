//! One transaction owns outline identity, location and bounded transcript selection.
use super::{
    Connection, ConversationStoreError, OptionalExtension, load_transcript_page, nonnegative,
    params, seq_to_i64, storage,
};
use crate::durable::reading::{
    ConversationReadCut, ConversationTurn, ConversationTurnId, ConversationTurnPage,
    ConversationWindowAt, DurableConversationWindow, TURN_PAGE_MAX, TURN_PREVIEW_MAX,
};
use crate::runtime::identity::{AttemptId, ConversationId};

pub(super) fn cut(
    connection: &Connection,
    conversation: &ConversationId,
) -> Result<ConversationReadCut, ConversationStoreError> {
    let (journal, transcript, revision, pending_count, pending_revision): (i64, i64, i64, i64, i64) = connection.query_row(
        "SELECT next_event_sequence,next_transcript_position,COALESCE((SELECT revision FROM surface_head WHERE id=1),0),(SELECT COUNT(*) FROM pending_inbound),(SELECT COALESCE(SUM(revision),0) FROM pending_inbound) FROM rustx_store WHERE id=1",
        [], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?)),
    ).map_err(|error| storage(error.to_string()))?;
    Ok(ConversationReadCut {
        conversation_id: conversation.clone(),
        journal: nonnegative(journal, "read journal")?,
        transcript: nonnegative(transcript, "read transcript")?,
        pending_count: nonnegative(pending_count, "read pending population")?,
        pending_revision: nonnegative(pending_revision, "read pending revisions")?,
        surface_revision: crate::conversation::SurfaceRevision::new(nonnegative(
            revision,
            "read Surface",
        )?),
    })
}

fn require_cut(
    actual: &ConversationReadCut,
    expected: Option<&ConversationReadCut>,
) -> Result<(), ConversationStoreError> {
    if expected.is_some_and(|expected| expected != actual) {
        return Err(ConversationStoreError::InvalidReference(
            "stale conversation read cut; reload the turn outline".into(),
        ));
    }
    Ok(())
}

// Bootstrap order is immutable native provenance order. Local Attempt starts
// use their indexed Journal order, independent of compaction or model steps.
const OWNERS: &str = "WITH owners AS (
    SELECT 0 AS source, CAST(p.key AS INTEGER) AS ordering,
      json_extract(p.value,'$.origin.conversation_id') AS conversation,
      json_extract(p.value,'$.origin.attempt_id') AS attempt, p.value AS inherited
    FROM bootstrap_identity b,json_each(b.response_provenance) p
    UNION ALL
    SELECT 1,sequence,conversation_id,attempt_id,NULL
    FROM events INDEXED BY events_kind_idx
    WHERE json_extract(event_json,'$.event.type')='attempt_started'
) ";

fn location(
    connection: &Connection,
    id: &ConversationTurnId,
    inherited: Option<&str>,
) -> Result<Option<u64>, ConversationStoreError> {
    let position: Option<i64> = if let Some(inherited) = inherited {
        connection.query_row(
            "SELECT MIN(t.position) FROM json_each(?1,'$.process_message_ids') m JOIN transcript_order t ON t.reference_kind='message' AND t.reference_id=m.value",
            [inherited], |row| row.get(0),
        )
    } else {
        connection.query_row(
            "SELECT MIN(t.position) FROM events e INDEXED BY events_attempt_idx JOIN transcript_order t
             ON (t.reference_kind='message' AND t.reference_id=json_extract(e.event_json,'$.event.message_id') AND json_extract(e.event_json,'$.event.type')='assistant_message_committed')
             OR (t.reference_kind='attempt_terminal' AND t.reference_id=e.event_id)
             WHERE e.attempt_id=?1",
            [id.attempt_id.as_str()], |row| row.get(0),
        )
    }.map_err(|error| storage(format!("turn location: {error}")))?;
    position
        .map(|position| nonnegative(position, "turn position"))
        .transpose()
}

fn preview(connection: &Connection, cursor: Option<u64>) -> Result<String, ConversationStoreError> {
    let Some(cursor) = cursor else {
        return Ok(String::new());
    };
    let text: Option<String> = connection.query_row(
        "SELECT substr(json_extract(block.value,'$.text'),1,?2)
         FROM transcript_order t JOIN message_ledger m ON t.reference_kind='message' AND t.reference_id=m.message_id,
         json_each(m.message_json,'$.content') block
         WHERE t.position=?1 AND json_extract(block.value,'$.type') IN ('text','refusal') ORDER BY block.key LIMIT 1",
        params![seq_to_i64(cursor)?, TURN_PREVIEW_MAX], |row| row.get(0),
    ).optional().map_err(|error| storage(format!("turn preview: {error}")))?;
    Ok(text.unwrap_or_default())
}

pub(super) fn turns(
    connection: &Connection,
    conversation: &ConversationId,
    expected: Option<&ConversationReadCut>,
    offset: usize,
    limit: usize,
) -> Result<ConversationTurnPage, ConversationStoreError> {
    if limit == 0 || limit > TURN_PAGE_MAX {
        return Err(storage("turn page limit must be between 1 and 64"));
    }
    let cut = cut(connection, conversation)?;
    require_cut(&cut, expected)?;
    let total: usize = connection
        .query_row(&format!("{OWNERS}SELECT COUNT(*) FROM owners"), [], |row| {
            row.get(0)
        })
        .map_err(|error| storage(error.to_string()))?;
    let offset = offset.min(total.saturating_sub(1) / limit * limit);
    let mut statement = connection.prepare(&format!("{OWNERS}SELECT conversation,attempt,inherited FROM owners ORDER BY source,ordering LIMIT ?1 OFFSET ?2")).map_err(|error| storage(error.to_string()))?;
    let rows = statement
        .query_map(params![limit, offset], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })
        .map_err(|error| storage(error.to_string()))?;
    let mut turns = Vec::with_capacity(limit);
    for (index, row) in rows.enumerate() {
        let (origin, attempt, inherited) = row.map_err(|error| storage(error.to_string()))?;
        let id = ConversationTurnId {
            conversation_id: ConversationId::new(origin),
            attempt_id: AttemptId::new(attempt),
        };
        let cursor = location(connection, &id, inherited.as_deref())?;
        turns.push(ConversationTurn {
            id,
            ordinal: offset + index + 1,
            preview: preview(connection, cursor)?,
            cursor: cursor.map(crate::durable::TranscriptCursor::new),
        });
    }
    Ok(ConversationTurnPage {
        cut,
        total,
        offset,
        turns,
    })
}

pub(super) fn window(
    connection: &Connection,
    conversation: &ConversationId,
    at: &ConversationWindowAt,
    limit: usize,
) -> Result<DurableConversationWindow, ConversationStoreError> {
    if limit == 0 || limit > TURN_PAGE_MAX {
        return Err(storage(
            "conversation window limit must be between 1 and 64",
        ));
    }
    let actual = cut(connection, conversation)?;
    let expected = match at {
        ConversationWindowAt::Latest => None,
        ConversationWindowAt::Older { cut, .. } => cut.as_ref(),
        ConversationWindowAt::Newer { cut, .. } | ConversationWindowAt::Turn { cut, .. } => {
            Some(cut)
        }
    };
    require_cut(&actual, expected)?;
    let mut target = None;
    let mut target_cursor = None;
    let before = match at {
        ConversationWindowAt::Latest => None,
        ConversationWindowAt::Older { before, .. } => Some(*before),
        ConversationWindowAt::Newer { after, .. } => {
            let last: Option<i64> = connection.query_row(
                "SELECT MAX(position) FROM (SELECT position FROM transcript_order WHERE position>?1 ORDER BY position LIMIT ?2)",
                params![seq_to_i64(after.get())?,limit], |row| row.get(0),
            ).map_err(|error| storage(error.to_string()))?;
            Some(crate::durable::TranscriptCursor::new(
                last.map_or(after.get(), |last| {
                    u64::try_from(last).expect("nonnegative transcript")
                }) + 1,
            ))
        }
        ConversationWindowAt::Turn { id, .. } => {
            let inherited: Option<Option<String>> = connection.query_row(
                &format!("{OWNERS}SELECT inherited FROM owners WHERE conversation=?1 AND attempt=?2 LIMIT 1"),
                params![id.conversation_id.as_str(),id.attempt_id.as_str()], |row| row.get(0),
            ).optional().map_err(|error| storage(error.to_string()))?;
            let inherited = inherited.ok_or_else(|| {
                ConversationStoreError::InvalidReference(
                    "turn is absent from this Conversation".into(),
                )
            })?;
            let cursor = location(connection, id, inherited.as_deref())?.ok_or_else(|| {
                ConversationStoreError::InvalidReference(
                    "turn has no published transcript location yet".into(),
                )
            })?;
            target = Some(id.clone());
            target_cursor = Some(crate::durable::TranscriptCursor::new(cursor));
            // Read forward from the exact target, never the intervening prefix.
            let last: i64 = connection.query_row(
                "SELECT MAX(position) FROM (SELECT position FROM transcript_order WHERE position>=?1 ORDER BY position LIMIT ?2)",
                params![seq_to_i64(cursor)?,limit], |row| row.get(0),
            ).map_err(|error| storage(error.to_string()))?;
            Some(crate::durable::TranscriptCursor::new(
                nonnegative(last, "target window")? + 1,
            ))
        }
    };
    let mut page = load_transcript_page(connection, before, limit)?;
    if let ConversationWindowAt::Newer { after, .. } = at {
        page.entries
            .retain(|entry| entry.cursor.get() > after.get());
    }
    if let Some(cursor) = target_cursor {
        page.entries.retain(|entry| entry.cursor >= cursor);
    }
    if let Some(first) = page.entries.first() {
        let older: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM transcript_order WHERE position<?1)",
                [seq_to_i64(first.cursor.get())?],
                |row| row.get(0),
            )
            .map_err(|error| storage(error.to_string()))?;
        page.next_cursor = older.then_some(first.cursor);
    }
    let last = page.entries.last().map(|entry| entry.cursor);
    let newer_cursor = match last {
        Some(cursor) => {
            let newer: bool = connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM transcript_order WHERE position>?1)",
                    [seq_to_i64(cursor.get())?],
                    |row| row.get(0),
                )
                .map_err(|error| storage(error.to_string()))?;
            newer.then_some(cursor)
        }
        None => None,
    };
    Ok(DurableConversationWindow {
        cut: actual,
        page,
        newer_cursor,
        target,
        target_cursor,
    })
}
