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
    let (journal, transcript, mutation_revision): (i64, i64, i64) = connection.query_row(
        "SELECT next_event_sequence,next_transcript_position,reading_mutation_revision FROM rustx_store WHERE id=1",
        [], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
    ).map_err(|error| storage(error.to_string()))?;
    Ok(ConversationReadCut {
        conversation_id: conversation.clone(),
        journal: nonnegative(journal, "read journal")?,
        transcript: nonnegative(transcript, "read transcript")?,
        mutation_revision: nonnegative(mutation_revision, "read mutation")?,
    })
}

fn require_cut(
    actual: &ConversationReadCut,
    expected: Option<&ConversationReadCut>,
) -> Result<(), ConversationStoreError> {
    if expected.is_some_and(|expected| !expected.reconstructible_from(actual)) {
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
      json_extract(p.value,'$.id.conversation_id') AS conversation,
      json_extract(p.value,'$.id.attempt_id') AS attempt, p.value AS inherited
    FROM bootstrap_identity b,json_each(b.turn_provenance) p
    UNION ALL
    SELECT 1,sequence,conversation_id,attempt_id,NULL
    FROM events INDEXED BY events_kind_idx
    WHERE sequence<=?1 AND json_extract(event_json,'$.event.type')='attempt_started'
) ";

fn location(
    connection: &Connection,
    id: &ConversationTurnId,
    inherited: Option<&str>,
    cut: &ConversationReadCut,
) -> Result<Option<u64>, ConversationStoreError> {
    let position: Option<i64> = if let Some(inherited) = inherited {
        let turn: crate::durable::reading::TurnReadingProvenance = super::decode(inherited, "turn location")?;
        if turn.process_message_ids.is_empty() {
            connection.query_row("SELECT t.position FROM bootstrap_identity b,json_each(b.turn_provenance) p JOIN transcript_order t ON t.reference_kind='inherited_turn' AND t.reference_id=CAST(p.key AS TEXT) WHERE p.value=?1 AND t.position<=?2", params![inherited,seq_to_i64(cut.transcript)?], |row| row.get(0)).optional()
        } else {
            connection.query_row("SELECT MIN(t.position) FROM json_each(?1,'$.process_message_ids') m JOIN transcript_order t ON t.reference_kind='message' AND t.reference_id=m.value WHERE t.position<=?2", params![inherited,seq_to_i64(cut.transcript)?], |row| row.get(0))
        }
    } else {
        connection.query_row(
            "SELECT MIN(t.position) FROM events e INDEXED BY events_attempt_idx JOIN transcript_order t
             ON (t.reference_kind='message' AND t.reference_id=json_extract(e.event_json,'$.event.message_id') AND json_extract(e.event_json,'$.event.type')='assistant_message_committed')
             OR (t.reference_kind='attempt_terminal' AND t.reference_id=e.event_id)
             WHERE e.attempt_id=?1 AND e.sequence<=?2 AND t.position<=?3",
            params![id.attempt_id.as_str(),seq_to_i64(cut.journal)?,seq_to_i64(cut.transcript)?], |row| row.get(0),
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
    let actual = cut(connection, conversation)?;
    require_cut(&actual, expected)?;
    let cut = expected.cloned().unwrap_or(actual);
    let total: usize = connection
        .query_row(
            &format!("{OWNERS}SELECT COUNT(*) FROM owners"),
            [seq_to_i64(cut.journal)?],
            |row| row.get(0),
        )
        .map_err(|error| storage(error.to_string()))?;
    let offset = offset.min(total.saturating_sub(1) / limit * limit);
    let mut statement = connection.prepare(&format!("{OWNERS}SELECT conversation,attempt,inherited FROM owners ORDER BY source,ordering LIMIT ?2 OFFSET ?3")).map_err(|error| storage(error.to_string()))?;
    let rows = statement
        .query_map(params![seq_to_i64(cut.journal)?, limit, offset], |row| {
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
        let cursor = location(connection, &id, inherited.as_deref(), &cut)?;
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
    let cut = expected.cloned().unwrap_or(actual);
    let mut target = None;
    let mut target_cursor = None;
    let before = match at {
        ConversationWindowAt::Latest => None,
        ConversationWindowAt::Older { before, .. } => Some(*before),
        ConversationWindowAt::Newer { after, .. } => {
            let last: Option<i64> = connection.query_row(
                "SELECT MAX(position) FROM (SELECT position FROM transcript_order WHERE position>?1 AND position<=?3 ORDER BY position LIMIT ?2)",
                params![seq_to_i64(after.get())?,limit,seq_to_i64(cut.transcript)?], |row| row.get(0),
            ).map_err(|error| storage(error.to_string()))?;
            Some(crate::durable::TranscriptCursor::new(
                last.map_or(after.get(), |last| {
                    u64::try_from(last).expect("nonnegative transcript")
                }) + 1,
            ))
        }
        ConversationWindowAt::Turn { id, .. } => {
            let inherited: Option<Option<String>> = connection.query_row(
                &format!("{OWNERS}SELECT inherited FROM owners WHERE conversation=?2 AND attempt=?3 LIMIT 1"),
                params![seq_to_i64(cut.journal)?,id.conversation_id.as_str(),id.attempt_id.as_str()], |row| row.get(0),
            ).optional().map_err(|error| storage(error.to_string()))?;
            let inherited = inherited.ok_or_else(|| {
                ConversationStoreError::InvalidReference(
                    "turn is absent from this Conversation".into(),
                )
            })?;
            let cursor =
                location(connection, id, inherited.as_deref(), &cut)?.ok_or_else(|| {
                    ConversationStoreError::InvalidReference(
                        "turn has no published transcript location yet".into(),
                    )
                })?;
            target = Some(id.clone());
            target_cursor = Some(crate::durable::TranscriptCursor::new(cursor));
            // Read forward from the exact target, never the intervening prefix.
            let last: i64 = connection.query_row(
                "SELECT MAX(position) FROM (SELECT position FROM transcript_order WHERE position>=?1 AND position<=?3 ORDER BY position LIMIT ?2)",
                params![seq_to_i64(cursor)?,limit,seq_to_i64(cut.transcript)?], |row| row.get(0),
            ).map_err(|error| storage(error.to_string()))?;
            Some(crate::durable::TranscriptCursor::new(
                nonnegative(last, "target window")? + 1,
            ))
        }
    };
    let upper = crate::durable::TranscriptCursor::new(cut.transcript + 1);
    let mut page = load_transcript_page(
        connection,
        Some(before.map_or(upper, |before| before.min(upper))),
        limit,
    )?;
    bound_tool_results(connection, &mut page, &cut)?;
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
    let newer_cursor = newer_cursor(
        connection,
        &cut,
        page.entries.last().map(|entry| entry.cursor),
    )?;
    Ok(DurableConversationWindow {
        cut,
        page,
        newer_cursor,
        target,
        target_cursor,
    })
}

fn newer_cursor(
    connection: &Connection,
    cut: &ConversationReadCut,
    last: Option<crate::durable::TranscriptCursor>,
) -> Result<Option<crate::durable::TranscriptCursor>, ConversationStoreError> {
    let Some(cursor) = last else {
        return Ok(None);
    };
    let newer: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM transcript_order WHERE position>?1 AND position<=?2)",
            params![seq_to_i64(cursor.get())?, seq_to_i64(cut.transcript)?],
            |row| row.get(0),
        )
        .map_err(|error| storage(error.to_string()))?;
    Ok(newer.then_some(cursor))
}

// Tool result links may have completed after C; their canonical positions
// must belong to C just like the entries themselves.
fn bound_tool_results(
    connection: &Connection,
    page: &mut crate::durable::TranscriptPage,
    cut: &ConversationReadCut,
) -> Result<(), ConversationStoreError> {
    for entry in &mut page.entries {
        for tool in &mut entry.tool_calls {
            let position: Option<i64> = connection.query_row(
                "SELECT t.position FROM canonical_tool_calls c JOIN transcript_order t ON t.reference_kind='message' AND t.reference_id=c.result_message_id WHERE c.assistant_message_id=?1 AND c.block_index=?2",
                params![tool.message_id.as_str(),tool.block_index.get()], |row| row.get(0),
            ).optional().map_err(|error| storage(error.to_string()))?;
            if position.is_some_and(|position| {
                position > i64::try_from(cut.transcript).expect("validated cut")
            }) {
                tool.result = None;
            }
        }
    }
    Ok(())
}

/// Lineage carries immutable summaries and member references, never executable facts.
pub(super) fn inherited_turns(
    connection: &Connection,
) -> Result<Vec<crate::durable::reading::TurnReadingProvenance>, ConversationStoreError> {
    let json: Option<String> = connection
        .query_row(
            "SELECT turn_provenance FROM bootstrap_identity WHERE id=1",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| storage(error.to_string()))?;
    json.map(|json| super::decode(&json, "bootstrap turns"))
        .transpose()
        .map(Option::unwrap_or_default)
}

pub(super) fn lineage_turns(
    connection: &Connection,
    conversation: &ConversationId,
    through: u64,
) -> Result<Vec<crate::durable::reading::TurnReadingProvenance>, ConversationStoreError> {
    use crate::durable::reading::{InheritedTurnOutcome, TurnReadingProvenance};
    use crate::events::types::{RuntimeEvent, RuntimeEventEnvelope};
    use crate::runtime::identity::MessageId;
    let mut turns = inherited_turns(connection)?;
    let mut statement = connection.prepare("SELECT event_json FROM events WHERE sequence<=?1 AND json_extract(event_json,'$.event.type')='attempt_started' ORDER BY sequence").map_err(|error| storage(error.to_string()))?;
    let starts = statement
        .query_map([seq_to_i64(through)?], |row| row.get::<_, String>(0))
        .map_err(|error| storage(error.to_string()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| storage(error.to_string()))?;
    for json in starts {
        let start: RuntimeEventEnvelope = super::decode(&json, "turn start")?;
        let id = start
            .attempt_id
            .ok_or_else(|| storage("Attempt start has no identity"))?;
        let mut turn = TurnReadingProvenance {
            id: ConversationTurnId {
                conversation_id: conversation.clone(),
                attempt_id: id.clone(),
            },
            process_message_ids: Vec::new(),
            preceding_message_id: None,
            outcome: InheritedTurnOutcome::IncompleteAtCut,
            started_at: Some(start.timestamp),
            ended_at: None,
        };
        let mut facts = connection.prepare("SELECT event_json FROM events INDEXED BY events_attempt_idx WHERE attempt_id=?1 AND sequence<=?2 AND json_extract(event_json,'$.event.type') IN ('assistant_message_committed','attempt_completed','attempt_cancelled','attempt_failed','attempt_timed_out','attempt_limit_exceeded') ORDER BY sequence").map_err(|error| storage(error.to_string()))?;
        let events = facts
            .query_map(params![id.as_str(), seq_to_i64(through)?], |row| {
                row.get::<_, String>(0)
            })
            .map_err(|error| storage(error.to_string()))?;
        for json in events {
            let event: RuntimeEventEnvelope = super::decode(
                &json.map_err(|error| storage(error.to_string()))?,
                "turn fact",
            )?;
            match event.event {
                RuntimeEvent::AssistantMessageCommitted { message_id } => {
                    turn.process_message_ids.push(message_id);
                }
                RuntimeEvent::AttemptCompleted { .. } => {
                    turn.outcome = InheritedTurnOutcome::Completed;
                }
                RuntimeEvent::AttemptCancelled { .. } => {
                    turn.outcome = InheritedTurnOutcome::Cancelled;
                }
                RuntimeEvent::AttemptFailed { .. } => turn.outcome = InheritedTurnOutcome::Failed,
                RuntimeEvent::AttemptTimedOut { .. } => {
                    turn.outcome = InheritedTurnOutcome::TimedOut;
                }
                RuntimeEvent::AttemptLimitExceeded { .. } => {
                    turn.outcome = InheritedTurnOutcome::LimitExceeded;
                }
                _ => continue,
            }
            if !matches!(turn.outcome, InheritedTurnOutcome::IncompleteAtCut) {
                turn.ended_at = Some(event.timestamp);
                turn.preceding_message_id = connection.query_row("SELECT t.reference_id FROM transcript_order t JOIN message_ledger m ON m.message_id=t.reference_id WHERE t.reference_kind='message' AND t.position<(SELECT position FROM transcript_order WHERE reference_kind='attempt_terminal' AND reference_id=?1) ORDER BY t.position DESC LIMIT 1", [event.event_id.as_str()], |row| row.get::<_,String>(0)).optional().map_err(|error| storage(error.to_string()))?.map(MessageId::new);
            }
        }
        if !turn.process_message_ids.is_empty()
            || !matches!(
                turn.outcome,
                InheritedTurnOutcome::IncompleteAtCut | InheritedTurnOutcome::Completed
            )
        {
            turns.push(turn);
        }
    }
    Ok(turns)
}
