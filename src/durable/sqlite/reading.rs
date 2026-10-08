//! One transaction owns outline identity, location and bounded transcript selection.
use super::{
    Connection, ConversationStoreError, OptionalExtension, load_transcript_page, nonnegative,
    params, seq_to_i64, storage,
};
use crate::durable::reading::{
    ConversationReadCut, ConversationTurn, ConversationTurnId, ConversationTurnPage,
    ConversationWindowAt, DurableConversationWindow, InheritedTurnOutcome, TURN_PAGE_MAX,
    TURN_PROMPT_PREVIEW_MAX, TURN_RESPONSE_PREVIEW_MAX, TurnExecution, TurnReadingProvenance,
};
use crate::runtime::identity::{AttemptId, ConversationId, MessageId};

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

/// Space-joined text blocks with collapsed whitespace, capped at `limit`
/// characters with a trailing ellipsis when clipped. The same rule shapes the
/// prompt and the response, so a preview never depends on block layout.
fn message_preview(
    connection: &Connection,
    message_id: &MessageId,
    limit: usize,
) -> Result<String, ConversationStoreError> {
    // Each block is read bounded: a multi-megabyte body never enters a preview.
    let mut statement = connection
        .prepare_cached(
            "SELECT substr(json_extract(block.value,'$.text'),1,?2),length(json_extract(block.value,'$.text'))>?2
             FROM message_ledger m,json_each(m.message_json,'$.content') block
             WHERE m.message_id=?1 AND json_extract(block.value,'$.type') IN ('text','refusal') ORDER BY block.key",
        )
        .map_err(|error| storage(format!("turn preview: {error}")))?;
    let blocks = statement
        .query_map(params![message_id.as_str(), limit * 2], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, bool>(1)?))
        })
        .map_err(|error| storage(format!("turn preview: {error}")))?;
    let mut text = String::new();
    let mut unread = false;
    for block in blocks {
        let (chunk, clipped) = block.map_err(|error| storage(format!("turn preview: {error}")))?;
        if text.chars().count() >= limit * 2 {
            unread = true;
            break;
        }
        if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(&chunk);
        if clipped {
            unread = true;
            break;
        }
    }
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() > limit - 1 {
        let head: String = normalized.chars().take(limit - 1).collect();
        return Ok(format!("{}…", head.trim_end()));
    }
    Ok(if unread {
        format!("{normalized}…")
    } else {
        normalized
    })
}

/// The first human prompt of a local turn: the earliest eligible message
/// adopted with this Attempt, or adopted while idle after the previous Attempt
/// started and before this one did. Steering of the previous turn carries that
/// turn's identity and never opens this one.
fn local_prompt(
    connection: &Connection,
    attempt: &AttemptId,
    start: i64,
    through: u64,
) -> Result<Option<(MessageId, String)>, ConversationStoreError> {
    let mut statement = connection
        .prepare_cached(
            "SELECT m.value FROM events e INDEXED BY events_kind_idx,json_each(e.event_json,'$.event.message_ids') m
             JOIN message_ledger l ON l.message_id=m.value
             WHERE json_extract(e.event_json,'$.event.type')='inbound_turn_adopted' AND e.sequence<=?3
               AND e.sequence>(SELECT COALESCE(MAX(sequence),0) FROM events INDEXED BY events_kind_idx
                 WHERE sequence<?2 AND json_extract(event_json,'$.event.type')='attempt_started')
               AND (e.attempt_id=?1 OR (e.attempt_id IS NULL AND e.sequence<?2))
               AND json_extract(l.message_json,'$.role')='user' AND json_extract(l.message_json,'$.source')='human'
               AND COALESCE(json_extract(l.message_json,'$.kind'),'message')='message'
             ORDER BY e.sequence,CAST(m.key AS INTEGER)",
        )
        .map_err(|error| storage(format!("turn prompt: {error}")))?;
    let ids = statement
        .query_map(
            params![attempt.as_str(), start, seq_to_i64(through)?],
            |row| row.get::<_, String>(0),
        )
        .map_err(|error| storage(format!("turn prompt: {error}")))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| storage(format!("turn prompt: {error}")))?;
    for id in ids.into_iter().map(MessageId::new) {
        let preview = message_preview(connection, &id, TURN_PROMPT_PREVIEW_MAX)?;
        if !preview.is_empty() {
            return Ok(Some((id, preview)));
        }
    }
    Ok(None)
}

/// The newest text-bearing Assistant message of a settled turn, newest first.
fn final_response(
    connection: &Connection,
    members: impl Iterator<Item = MessageId>,
) -> Result<String, ConversationStoreError> {
    for id in members {
        let preview = message_preview(connection, &id, TURN_RESPONSE_PREVIEW_MAX)?;
        if !preview.is_empty() {
            return Ok(preview);
        }
    }
    Ok(String::new())
}

/// Prompt and response of a local turn at the cut. A turn still running at
/// the cut has no final response yet.
fn local_previews(
    connection: &Connection,
    attempt: &AttemptId,
    start: i64,
    cut: &ConversationReadCut,
) -> Result<(String, String), ConversationStoreError> {
    let prompt = local_prompt(connection, attempt, start, cut.journal)?
        .map(|(_, preview)| preview)
        .unwrap_or_default();
    let settled: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM events INDEXED BY events_attempt_idx WHERE attempt_id=?1 AND sequence<=?2
             AND json_extract(event_json,'$.event.type') IN ('attempt_completed','attempt_cancelled','attempt_failed','attempt_timed_out','attempt_limit_exceeded'))",
            params![attempt.as_str(), seq_to_i64(cut.journal)?],
            |row| row.get(0),
        )
        .map_err(|error| storage(format!("turn settlement: {error}")))?;
    if !settled {
        return Ok((prompt, String::new()));
    }
    let mut statement = connection
        .prepare_cached(
            "SELECT json_extract(event_json,'$.event.message_id') FROM events INDEXED BY events_attempt_idx
             WHERE attempt_id=?1 AND sequence<=?2 AND json_extract(event_json,'$.event.type')='assistant_message_committed'
             ORDER BY sequence DESC",
        )
        .map_err(|error| storage(format!("turn response: {error}")))?;
    let members = statement
        .query_map(params![attempt.as_str(), seq_to_i64(cut.journal)?], |row| {
            row.get::<_, String>(0)
        })
        .map_err(|error| storage(format!("turn response: {error}")))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| storage(format!("turn response: {error}")))?;
    Ok((
        prompt,
        final_response(connection, members.into_iter().map(MessageId::new))?,
    ))
}

/// Prompt and response of an inherited turn from its immutable provenance.
/// Both reading ownership and completed-response replay ownership explicitly
/// name retained human inputs. Their earliest text-bearing input is the preview;
/// transcript adjacency and the source's current state are never evidence.
fn inherited_previews(
    connection: &Connection,
    turn: &TurnReadingProvenance,
) -> Result<(String, String), ConversationStoreError> {
    let mut statement = connection
        .prepare_cached(
            "WITH inputs AS (
            SELECT ?1 AS message_id
            UNION
            SELECT json_extract(p.value,'$.retry_message_id')
            FROM bootstrap_identity b,json_each(b.response_provenance) p
            WHERE json_extract(p.value,'$.origin.conversation_id')=?2
              AND json_extract(p.value,'$.origin.attempt_id')=?3
        )
        SELECT m.message_id FROM inputs i JOIN message_ledger m ON m.message_id=i.message_id
        WHERE json_extract(m.message_json,'$.role')='user'
          AND json_extract(m.message_json,'$.source')='human'
          AND COALESCE(json_extract(m.message_json,'$.kind'),'message')='message'
        ORDER BY m.position",
        )
        .map_err(|error| storage(format!("inherited prompt owners: {error}")))?;
    let inputs = statement
        .query_map(
            params![
                turn.prompt_message_id.as_ref().map(MessageId::as_str),
                turn.id.conversation_id.as_str(),
                turn.id.attempt_id.as_str()
            ],
            |row| row.get::<_, String>(0),
        )
        .map_err(|error| storage(format!("inherited prompt owners: {error}")))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| storage(format!("inherited prompt owners: {error}")))?;
    let mut prompt = String::new();
    for id in inputs {
        prompt = message_preview(connection, &MessageId::new(id), TURN_PROMPT_PREVIEW_MAX)?;
        if !prompt.is_empty() {
            break;
        }
    }
    let response = if matches!(turn.outcome, InheritedTurnOutcome::IncompleteAtCut) {
        String::new()
    } else {
        final_response(connection, turn.process_message_ids.iter().rev().cloned())?
    };
    Ok((prompt, response))
}

pub(super) fn turns(
    connection: &Connection,
    conversation: &ConversationId,
    offset: usize,
    limit: usize,
) -> Result<ConversationTurnPage, ConversationStoreError> {
    if limit == 0 || limit > TURN_PAGE_MAX {
        return Err(storage("turn page limit must be between 1 and 64"));
    }
    let cut = cut(connection, conversation)?;
    let total: usize = connection
        .query_row(
            &format!("{OWNERS}SELECT COUNT(*) FROM owners"),
            [seq_to_i64(cut.journal)?],
            |row| row.get(0),
        )
        .map_err(|error| storage(error.to_string()))?;
    let offset = offset.min(total.saturating_sub(1) / limit * limit);
    let mut statement = connection.prepare(&format!("{OWNERS}SELECT conversation,attempt,inherited,ordering FROM owners ORDER BY source,ordering LIMIT ?2 OFFSET ?3")).map_err(|error| storage(error.to_string()))?;
    let rows = statement
        .query_map(params![seq_to_i64(cut.journal)?, limit, offset], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })
        .map_err(|error| storage(error.to_string()))?;
    let mut turns = Vec::with_capacity(limit);
    for (index, row) in rows.enumerate() {
        let (origin, attempt, inherited, start) =
            row.map_err(|error| storage(error.to_string()))?;
        let id = ConversationTurnId {
            conversation_id: ConversationId::new(origin),
            attempt_id: AttemptId::new(attempt),
        };
        let cursor = location(connection, &id, inherited.as_deref(), &cut)?;
        let (prompt, response) = match inherited.as_deref() {
            Some(inherited) => inherited_previews(
                connection,
                &super::decode::<TurnReadingProvenance>(inherited, "turn previews")?,
            )?,
            None => local_previews(connection, &id.attempt_id, start, &cut)?,
        };
        turns.push(ConversationTurn {
            id,
            ordinal: offset + index + 1,
            cursor: cursor.map(crate::durable::TranscriptCursor::new),
            prompt,
            response,
        });
    }
    Ok(ConversationTurnPage {
        cut,
        total,
        offset,
        turns,
    })
}

fn message_location(
    connection: &Connection,
    id: &MessageId,
    cut: &ConversationReadCut,
) -> Result<u64, ConversationStoreError> {
    let cursor: Option<i64> = connection.query_row(
        "SELECT position FROM transcript_order WHERE reference_kind='message' AND reference_id=?1 AND position<=?2",
        params![id.as_str(),seq_to_i64(cut.transcript)?], |row| row.get(0),
    ).optional().map_err(|error| storage(error.to_string()))?;
    let cursor = cursor.ok_or_else(|| {
        ConversationStoreError::InvalidReference(
            "message is absent from this Conversation read cut".into(),
        )
    })?;
    nonnegative(cursor, "message location")
}

/// Select forward from one authoritative position without reading its prefix.
fn forward_window_before(
    connection: &Connection,
    cursor: u64,
    cut: &ConversationReadCut,
    limit: usize,
) -> Result<crate::durable::TranscriptCursor, ConversationStoreError> {
    let last: i64 = connection.query_row(
        "SELECT MAX(position) FROM (SELECT position FROM transcript_order WHERE position>=?1 AND position<=?3 ORDER BY position LIMIT ?2)",
        params![seq_to_i64(cursor)?,limit,seq_to_i64(cut.transcript)?], |row| row.get(0),
    ).map_err(|error| storage(error.to_string()))?;
    Ok(crate::durable::TranscriptCursor::new(
        nonnegative(last, "target window")? + 1,
    ))
}
fn require_message_target(
    page: &crate::durable::TranscriptPage,
    id: &MessageId,
    cursor: Option<crate::durable::TranscriptCursor>,
) -> Result<(), ConversationStoreError> {
    if page.entries.iter().any(|entry| Some(entry.cursor) == cursor
        && matches!(&entry.item, crate::durable::TranscriptItem::Message { message } if message.id() == id)) {
        Ok(())
    } else {
        Err(ConversationStoreError::InvalidReference(
            "message is not readable at its native transcript position".into(),
        ))
    }
}
pub(super) fn window(
    connection: &Connection,
    conversation: &ConversationId,
    at: &ConversationWindowAt,
    limit: usize,
) -> Result<DurableConversationWindow, ConversationStoreError> {
    if limit == 0 || limit > TURN_PAGE_MAX {
        return Err(storage("conversation window limit must be 1 to 64"));
    }
    let actual = cut(connection, conversation)?;
    let expected = match at {
        ConversationWindowAt::Latest => None,
        ConversationWindowAt::Older { cut, .. } | ConversationWindowAt::Message { cut, .. } => {
            cut.as_ref()
        }
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
        ConversationWindowAt::Message { id, .. } => {
            let cursor = message_location(connection, id, &cut)?;
            target_cursor = Some(crate::durable::TranscriptCursor::new(cursor));
            Some(forward_window_before(connection, cursor, &cut, limit)?)
        }
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
                "SELECT NULL FROM events INDEXED BY events_attempt_idx
                 WHERE attempt_id=?3 AND conversation_id=?2 AND sequence<=?1 AND json_extract(event_json,'$.event.type')='attempt_started'
                 UNION ALL SELECT p.value FROM bootstrap_identity b,json_each(b.turn_provenance) p
                 WHERE json_extract(p.value,'$.id.conversation_id')=?2 AND json_extract(p.value,'$.id.attempt_id')=?3 LIMIT 1",
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
            Some(forward_window_before(connection, cursor, &cut, limit)?)
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
    if let ConversationWindowAt::Message { id, .. } = at {
        require_message_target(&page, id, target_cursor)?;
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

/// One local Attempt's additive execution totals through the lineage cut,
/// folded exactly as whole-conversation statistics and occupancy fold them.
fn turn_execution(
    connection: &Connection,
    conversation: &ConversationId,
    attempt: &AttemptId,
    through: u64,
) -> Result<TurnExecution, ConversationStoreError> {
    use crate::events::types::{RuntimeEvent as E, RuntimeEventEnvelope};
    use crate::model::finish::ModelFinishReason;
    let mut execution = TurnExecution::default();
    let mut activity = crate::durable::response::timing::ActivityFold::default();
    // The newest occupancy boundary: a request and its reported usage, or a
    // compaction that invalidates the previous reading.
    let mut boundary: Option<(
        crate::runtime::identity::RequestId,
        Option<crate::model::types::ModelUsage>,
    )> = None;
    let mut closing = false;
    let mut statement = connection.prepare("SELECT event_json FROM events INDEXED BY events_attempt_idx WHERE attempt_id=?1 AND sequence<=?2 AND json_extract(event_json,'$.event.type') IN ('turn_started','model_request_started','model_request_completed','model_request_failed','tool_execution_started','tool_execution_completed','tool_execution_failed','compaction_started','compaction_completed','assistant_message_committed','attempt_completed') ORDER BY sequence").map_err(|error| storage(error.to_string()))?;
    let rows = statement
        .query_map(params![attempt.as_str(), seq_to_i64(through)?], |row| {
            row.get::<_, String>(0)
        })
        .map_err(|error| storage(error.to_string()))?;
    for json in rows {
        let event: RuntimeEventEnvelope = super::decode(
            &json.map_err(|error| storage(error.to_string()))?,
            "turn execution fact",
        )?;
        match event.event {
            E::TurnStarted => execution.steps += 1,
            E::ModelRequestStarted { request_id, .. } => {
                execution.model_requests += 1;
                boundary = Some((request_id, None));
            }
            E::ModelRequestCompleted {
                request_id,
                usage,
                generation,
                ..
            }
            | E::ModelRequestFailed {
                request_id,
                usage,
                generation,
                ..
            } => {
                activity.request(generation.as_ref(), usage.as_ref());
                if let Some(usage) = usage {
                    execution.requests_with_usage += 1;
                    crate::durable::response::add_usage(&mut execution.reported_usage, &usage);
                    if let Some((current, reading)) = &mut boundary
                        && *current == request_id
                    {
                        *reading = Some(usage);
                    }
                }
            }
            E::ToolExecutionStarted { tool_call_id, .. } => {
                activity.tool_started(attempt.clone(), tool_call_id, event.timestamp);
            }
            E::ToolExecutionCompleted { tool_call_id, .. }
            | E::ToolExecutionFailed { tool_call_id, .. } => {
                activity.tool_settled(attempt.clone(), tool_call_id, event.timestamp);
            }
            E::CompactionStarted | E::CompactionCompleted { .. } => boundary = None,
            E::AssistantMessageCommitted { .. } => closing = true,
            E::AttemptCompleted {
                finish_reason: ModelFinishReason::Stop | ModelFinishReason::Refusal,
                ..
            } => execution.completed_response = closing,
            _ => {}
        }
    }
    activity.record(&mut execution);
    if let Some((request, Some(usage))) = boundary {
        execution.occupancy = crate::context::occupancy::measure(
            &usage,
            &super::load_request_snapshot(connection, conversation, &request)?,
        )?;
    }
    Ok(execution)
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
    use crate::events::types::{RuntimeEvent, RuntimeEventEnvelope};
    let mut turns = inherited_turns(connection)?;
    let mut statement = connection.prepare("SELECT event_json,sequence FROM events WHERE sequence<=?1 AND json_extract(event_json,'$.event.type')='attempt_started' ORDER BY sequence").map_err(|error| storage(error.to_string()))?;
    let starts = statement
        .query_map([seq_to_i64(through)?], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(|error| storage(error.to_string()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| storage(error.to_string()))?;
    for (json, sequence) in starts {
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
            prompt_message_id: local_prompt(connection, &id, sequence, through)?.map(|(id, _)| id),
            outcome: InheritedTurnOutcome::IncompleteAtCut,
            started_at: Some(start.timestamp),
            ended_at: None,
            execution: Some(turn_execution(connection, conversation, &id, through)?),
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
