//! Finite turn outlines captured on their original native J/provenance rows.
use super::{Connection, ConversationStoreError, OptionalExtension, params, storage};
use crate::events::types::{RuntimeEvent, RuntimeEventEnvelope};
use crate::runtime::identity::{AttemptId, ConversationId};
use crate::runtime_client::turn::{ObservedTurn, ObservedTurnIdentity, ObservedTurnPage};
use serde::{Deserialize, Serialize};
const SAFE: u64 = crate::runtime_client::content::CONTENT_SAFE_INTEGER_MAX;
#[derive(Serialize, Deserialize)]
struct Layout {
    turn: ObservedTurn,
    last_response: String,
    settled: bool,
}
fn encode(value: &impl Serialize) -> Result<Vec<u8>, ConversationStoreError> {
    crate::runtime_client::checkpoint::bounded_bytes(value, 16_384)
        .ok_or_else(|| storage("turn display header exceeds bound"))
}
fn identity(
    conversation: &ConversationId,
    attempt: &AttemptId,
    source: crate::runtime_client::response::AttemptIdentitySource,
) -> ObservedTurnIdentity {
    ObservedTurnIdentity {
        conversation_id: conversation.clone(),
        attempt_id: (attempt.as_str().len() <= 512).then(|| attempt.clone()),
        attempt_id_source: (attempt.as_str().len() > 512).then_some(source),
    }
}
pub(super) fn required(event: &RuntimeEventEnvelope) -> bool {
    event.attempt_id.is_some()
        && matches!(
            event.event,
            RuntimeEvent::AttemptStarted { .. }
                | RuntimeEvent::InboundTurnAdopted { .. }
                | RuntimeEvent::AssistantMessageCommitted { .. }
                | RuntimeEvent::AttemptCompleted { .. }
                | RuntimeEvent::AttemptFailed { .. }
                | RuntimeEvent::AttemptCancelled { .. }
                | RuntimeEvent::AttemptTimedOut { .. }
                | RuntimeEvent::AttemptLimitExceeded { .. }
        )
}
pub(super) fn event(
    connection: &Connection,
    event: &RuntimeEventEnvelope,
) -> Result<(), ConversationStoreError> {
    if let RuntimeEvent::InboundTurnAdopted { message_ids, .. } = &event.event {
        adoption(connection, event, message_ids)?;
        if event.attempt_id.is_none() {
            return Ok(());
        }
    }
    if !required(event) {
        return Ok(());
    }
    let attempt = event
        .attempt_id
        .as_ref()
        .ok_or_else(|| storage("turn owner absent"))?;
    let start:Option<u64>=connection.query_row("SELECT sequence FROM events INDEXED BY events_attempt_kind_idx WHERE attempt_id=?1 AND json_extract(event_json,'$.event.type')='attempt_started' AND sequence<=?2 ORDER BY sequence DESC LIMIT 1",params![attempt.as_str(),event.sequence],|row|row.get(0)).optional().map_err(|error|storage(error.to_string()))?;
    let Some(start) = start else {
        connection
            .execute(
                "UPDATE events SET observation_turn_header=?2 WHERE sequence=?1",
                params![event.sequence, b"null".as_slice()],
            )
            .map_err(|error| storage(error.to_string()))?;
        return Ok(());
    };
    let mut state = if start == event.sequence {
        start_layout(connection, event, attempt)?
    } else {
        layout(connection, start, event.sequence.saturating_sub(1))?
    };
    if matches!(event.event, RuntimeEvent::InboundTurnAdopted { .. })
        || state.turn.prompt.is_empty()
    {
        state.turn.prompt = prompt(connection, attempt, start, event.sequence)?;
    }
    if let RuntimeEvent::AssistantMessageCommitted { .. } = &event.event {
        let position: Option<u64> = connection
            .query_row(
                "SELECT observation_assistant_ledger FROM events WHERE sequence=?1",
                [event.sequence],
                |row| row.get(0),
            )
            .map_err(|error| storage(error.to_string()))?;
        let position = position.ok_or_else(|| storage("turn canonical position unavailable"))?;
        let preview = preview(connection, position, 120)?;
        if !preview.is_empty() {
            state.last_response = preview;
        }
        let cursor: Option<u64> = connection
            .query_row(
                "SELECT native_transcript FROM message_ledger WHERE position=?1",
                [position],
                |row| row.get(0),
            )
            .map_err(|error| storage(error.to_string()))?;
        if let Some(cursor) = cursor {
            state.turn.cursor = Some(
                state
                    .turn
                    .cursor
                    .map_or(crate::durable::TranscriptCursor::new(cursor), |old| {
                        old.min(crate::durable::TranscriptCursor::new(cursor))
                    }),
            );
        }
    }
    if matches!(
        event.event,
        RuntimeEvent::AttemptCompleted { .. }
            | RuntimeEvent::AttemptFailed { .. }
            | RuntimeEvent::AttemptCancelled { .. }
            | RuntimeEvent::AttemptTimedOut { .. }
            | RuntimeEvent::AttemptLimitExceeded { .. }
    ) {
        state.settled = true;
        if state.turn.cursor.is_none() {
            let cursor: Option<u64> = connection
                .query_row(
                    "SELECT observation_transcript FROM events WHERE sequence=?1",
                    [event.sequence],
                    |row| row.get(0),
                )
                .map_err(|error| storage(error.to_string()))?;
            state.turn.cursor = cursor.map(crate::durable::TranscriptCursor::new);
        }
    }
    if state.settled {
        state.turn.response = state.last_response.clone();
    }
    connection.execute("UPDATE events SET observation_turn_header=?2,observation_turn_anchor=?3 WHERE sequence=?1",params![event.sequence,encode(&state)?,start]).map_err(|error|storage(error.to_string()))?;
    Ok(())
}
fn start_layout(
    connection: &Connection,
    event: &RuntimeEventEnvelope,
    attempt: &AttemptId,
) -> Result<Layout, ConversationStoreError> {
    let ordinal = super::observation_statistics::read(connection, event.sequence)?.turns;
    if ordinal == 0 || ordinal > SAFE {
        return Err(storage("turn ordinal exceeds protocol range"));
    }
    connection
        .execute(
            "UPDATE events SET observation_turn_ordinal=?2 WHERE sequence=?1",
            params![event.sequence, ordinal],
        )
        .map_err(|error| storage(error.to_string()))?;
    Ok(Layout {
        turn: ObservedTurn {
            id: identity(
                &event.conversation_id,
                attempt,
                super::observation_identity::journal(event.sequence, attempt)?,
            ),
            ordinal,
            cursor: None,
            prompt: prompt(connection, attempt, event.sequence, event.sequence)?,
            response: String::new(),
        },
        last_response: String::new(),
        settled: false,
    })
}
fn layout(
    connection: &Connection,
    start: u64,
    through: u64,
) -> Result<Layout, ConversationStoreError> {
    let bytes:Option<Vec<u8>>=connection.query_row("SELECT observation_turn_header FROM events INDEXED BY events_observation_turn_anchor_idx WHERE observation_turn_anchor=?1 AND observation_turn_header IS NOT NULL AND sequence<=?2 ORDER BY sequence DESC LIMIT 1",params![start,through],|row|row.get(0)).optional().map_err(|error|storage(error.to_string()))?;
    serde_json::from_slice(&bytes.ok_or_else(|| storage("turn display header unavailable"))?)
        .map_err(|_| storage("turn display header corrupt"))
}
fn adoption(
    connection: &Connection,
    event: &RuntimeEventEnvelope,
    ids: &[crate::runtime::identity::MessageId],
) -> Result<(), ConversationStoreError> {
    let mut selected = String::new();
    for id in ids {
        let header = super::observation::message(connection, id)?
            .ok_or_else(|| storage("turn input header unavailable"))?;
        if header.opening_prompt {
            let position = header
                .ledger_position
                .ok_or_else(|| storage("turn input position unavailable"))?;
            selected = preview(connection, position, 50)?;
            if !selected.is_empty() {
                break;
            }
        }
    }
    connection.execute("UPDATE events SET observation_turn_prompt=?2,observation_turn_prompt_positive=?3 WHERE sequence=?1",params![event.sequence,selected.as_bytes(),!selected.is_empty()]).map_err(|error|storage(error.to_string()))?;
    Ok(())
}
fn prompt(
    connection: &Connection,
    attempt: &AttemptId,
    start: u64,
    through: u64,
) -> Result<String, ConversationStoreError> {
    let previous:u64=connection.query_row("SELECT COALESCE(MAX(sequence),0) FROM events INDEXED BY events_kind_idx WHERE json_extract(event_json,'$.event.type')='attempt_started' AND sequence<?1",[start],|row|row.get(0)).map_err(|error|storage(error.to_string()))?;
    let missing:bool=connection.query_row("SELECT EXISTS(SELECT 1 FROM events INDEXED BY events_observation_turn_prompt_holes_idx WHERE observation_kind='inbound_turn_adopted' AND observation_turn_prompt IS NULL AND sequence<=?1 AND ((attempt_id=?2) OR (attempt_id IS NULL AND sequence>?3 AND sequence<?4)))",params![through,attempt.as_str(),previous,start],|row|row.get(0)).map_err(|error|storage(error.to_string()))?;
    if missing {
        return Err(storage("turn input display source unavailable"));
    }
    let selected:Option<Vec<u8>>=connection.query_row("SELECT preview FROM (SELECT sequence,observation_turn_prompt AS preview FROM events INDEXED BY events_observation_turn_prompt_idx WHERE observation_turn_prompt_positive=1 AND attempt_id=?1 AND sequence<=?2 UNION ALL SELECT sequence,observation_turn_prompt FROM events INDEXED BY events_observation_turn_prompt_idx WHERE observation_turn_prompt_positive=1 AND attempt_id IS NULL AND sequence>?3 AND sequence<?4) ORDER BY sequence LIMIT 1",params![attempt.as_str(),through,previous,start],|row|row.get(0)).optional().map_err(|error|storage(error.to_string()))?;
    selected
        .map(String::from_utf8)
        .transpose()
        .map_err(|_| storage("turn prompt encoding unavailable"))
        .map(Option::unwrap_or_default)
}
pub(super) fn preview(
    connection: &Connection,
    position: u64,
    limit: usize,
) -> Result<String, ConversationStoreError> {
    let mut blocks = Vec::with_capacity(66);
    let key = format!("ledger:{position}");
    for kind in ["text", "refusal"] {
        let mut statement=connection.prepare_cached("SELECT h.block_index,substr(s.bytes,1,?3),f.readable_end FROM observation_block_headers h INDEXED BY observation_block_ledger_kind_idx JOIN content_segments s ON s.source_kind='canonical' AND s.source_id=?4 AND s.block_index=h.block_index AND s.representation='utf8_text' AND s.segment=0 JOIN content_frontiers f ON f.source_kind='canonical' AND f.source_id=?4 AND f.block_index=h.block_index AND f.representation='utf8_text' AND f.revision=1 WHERE h.ledger_position=?1 AND json_extract(h.header_json,'$.kind')=?2 ORDER BY h.block_index LIMIT 33").map_err(|error|storage(error.to_string()))?;
        for row in statement
            .query_map(params![position, kind, limit * 8, key], |row| {
                Ok((
                    row.get::<_, u32>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, u64>(2)?,
                ))
            })
            .map_err(|error| storage(error.to_string()))?
        {
            blocks.push(row.map_err(|error| storage(error.to_string()))?);
        }
    }
    blocks.sort_unstable_by_key(|(index, _, _)| *index);
    let mut text = String::new();
    let mut unread = blocks.len() > 32;
    for (_, bytes, end) in blocks.into_iter().take(32) {
        if text.chars().count() >= limit * 2 {
            unread = true;
            break;
        }
        let valid = match std::str::from_utf8(&bytes) {
            Ok(text) => text,
            Err(error) => std::str::from_utf8(&bytes[..error.valid_up_to()])
                .map_err(|_| storage("turn preview UTF-8 unavailable"))?,
        };
        if !text.is_empty() {
            text.push(' ');
        }
        text.extend(valid.chars().take(limit * 2));
        if end > bytes.len() as u64 || valid.chars().count() > limit * 2 {
            unread = true;
            break;
        }
    }
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    Ok(if normalized.chars().count() > limit - 1 {
        format!(
            "{}…",
            normalized
                .chars()
                .take(limit - 1)
                .collect::<String>()
                .trim_end()
        )
    } else if unread {
        format!("{normalized}…")
    } else {
        normalized
    })
}
pub(super) fn seed(
    connection: &Connection,
    seed: &crate::durable::inbox::LineageSeed,
) -> Result<(), ConversationStoreError> {
    for (ordinal, turn) in seed.turns().iter().enumerate() {
        let ordinal = u64::try_from(ordinal).map_err(|_| storage("turn ordinal exhausted"))?;
        let display_ordinal = ordinal
            .checked_add(1)
            .filter(|value| *value <= SAFE)
            .ok_or_else(|| storage("turn ordinal exceeds protocol range"))?;
        let bytes: Option<Vec<u8>> = connection
            .query_row(
                "SELECT observation_members FROM inherited_turn_provenance WHERE ordinal=?1",
                [ordinal],
                |row| row.get(0),
            )
            .map_err(|error| storage(error.to_string()))?;
        let members: crate::durable::observation::AttemptMemberCounts = serde_json::from_slice(
            &bytes.ok_or_else(|| storage("inherited turn members unavailable"))?,
        )
        .map_err(|_| storage("inherited turn members corrupt"))?;
        let cursor = if let Some(cursor) = members.first_cursor {
            Some(cursor)
        } else {
            connection.query_row("SELECT position FROM transcript_order WHERE reference_kind='inherited_turn' AND reference_id=?1",[ordinal.to_string()],|row|row.get::<_,u64>(0)).optional().map_err(|error|storage(error.to_string()))?.map(crate::durable::TranscriptCursor::new)
        };
        let mut prompt = String::new();
        let inputs = turn.prompt_message_id.iter().chain(
            seed.completed_responses()
                .iter()
                .filter(|response| {
                    response.origin.conversation_id == turn.id.conversation_id
                        && response.origin.attempt_id == turn.id.attempt_id
                        && response
                            .process_message_ids
                            .iter()
                            .any(|id| turn.process_message_ids.contains(id))
                })
                .filter_map(|response| response.retry_message_id.as_ref()),
        );
        for id in inputs {
            let header = super::observation::message(connection, id)?
                .ok_or_else(|| storage("inherited turn input header unavailable"))?;
            if header.opening_prompt {
                prompt = preview(connection, canonical_position(connection, id)?, 50)?;
                if !prompt.is_empty() {
                    break;
                }
            }
        }
        let mut response = String::new();
        if turn.outcome != crate::durable::reading::InheritedTurnOutcome::IncompleteAtCut {
            for id in turn.process_message_ids.iter().rev() {
                response = preview(connection, canonical_position(connection, id)?, 120)?;
                if !response.is_empty() {
                    break;
                }
            }
        }
        let header = ObservedTurn {
            id: identity(
                &turn.id.conversation_id,
                &turn.id.attempt_id,
                super::observation_identity::inherited(
                    connection,
                    crate::runtime_client::response::InheritedAttemptIdentityKind::Turn,
                    ordinal,
                )?,
            ),
            ordinal: display_ordinal,
            cursor,
            prompt,
            response,
        };
        super::provenance::write_turn_header(
            connection,
            i64::try_from(ordinal).map_err(|_| storage("turn ordinal exhausted"))?,
            super::provenance::TurnHeaderKind::Observed,
            &header,
        )?;
    }
    Ok(())
}
fn canonical_position(
    connection: &Connection,
    id: &crate::runtime::identity::MessageId,
) -> Result<u64, ConversationStoreError> {
    connection
        .query_row(
            "SELECT position FROM message_ledger WHERE message_id=?1",
            [id.as_str()],
            |row| row.get(0),
        )
        .map_err(|_| storage("native turn canonical position unavailable"))
}
pub(super) fn inherited(
    connection: &Connection,
    ordinal: u64,
) -> Result<ObservedTurn, ConversationStoreError> {
    let header: ObservedTurn = super::provenance::read_turn_header(
        connection,
        i64::try_from(ordinal).map_err(|_| storage("turn ordinal exhausted"))?,
        super::provenance::TurnHeaderKind::Observed,
    )?;
    if header.ordinal != ordinal + 1 {
        return Err(storage("inherited turn display ordinal differs"));
    }
    super::provenance::validate_turn_cursor(connection, header.cursor)?;
    Ok(header)
}
fn validate_header(header: &ObservedTurn) -> Result<(), ConversationStoreError> {
    if header.ordinal == 0
        || header.ordinal > SAFE
        || (header.id.attempt_id.is_none() && header.id.attempt_id_source.is_none())
        || crate::runtime_client::checkpoint::bounded_bytes(header, 8192).is_none()
    {
        return Err(storage("invalid finite turn display"));
    }
    Ok(())
}
pub(super) fn selected(
    connection: &Connection,
    conversation: &ConversationId,
    cut: &crate::durable::reading::ConversationReadCut,
    ordinal: u64,
) -> Result<ObservedTurn, ConversationStoreError> {
    let actual = super::reading::cut(connection, conversation)?;
    if !cut.reconstructible_from(&actual) {
        return Err(ConversationStoreError::ObservationCutExpired);
    }
    if ordinal == 0 || ordinal > SAFE {
        return Err(ConversationStoreError::InvalidContentRange);
    }
    let missing:bool=connection.query_row("SELECT EXISTS(SELECT 1 FROM events INDEXED BY events_observation_turn_holes_idx WHERE observation_turn_required=1 AND observation_turn_header IS NULL AND sequence<=?1)",[cut.journal],|row|row.get(0)).map_err(|error|storage(error.to_string()))?;
    if missing {
        return Err(storage("native turn display source unavailable"));
    }
    let total = super::observation_statistics::read(connection, cut.journal)?.turns;
    if ordinal > total {
        return Err(ConversationStoreError::InvalidReference(
            "turn ordinal beyond captured outline".into(),
        ));
    }
    let (initialized,base):(bool,Option<u64>)=connection.query_row("SELECT bootstrap_initialized,(SELECT turn_count FROM bootstrap_identity WHERE id=1) FROM rustx_store WHERE id=1",[],|row|Ok((row.get(0)?,row.get(1)?))).map_err(|error|storage(error.to_string()))?;
    let base = base
        .or((!initialized).then_some(0))
        .ok_or_else(|| storage("native turn bootstrap evidence unavailable"))?;
    if u64::try_from(super::provenance::turn_header_count(connection)?)
        .map_err(|_| storage("negative inherited turn count"))?
        != base
    {
        return Err(storage(
            "inherited turn count differs from native bootstrap",
        ));
    }
    let mut header = if ordinal <= base {
        inherited(connection, ordinal - 1)?
    } else {
        let start:u64=connection.query_row("SELECT sequence FROM events INDEXED BY events_observation_turn_ordinal_idx WHERE observation_turn_ordinal=?1 AND observation_turn_ordinal IS NOT NULL AND observation_kind='attempt_started' AND sequence<=?2",params![ordinal,cut.journal],|row|row.get(0)).map_err(|_|storage("native turn ordinal source unavailable"))?;
        layout(connection, start, cut.journal)?.turn
    };
    validate_header(&header)?;
    if header
        .cursor
        .is_some_and(|cursor| cursor.get() > cut.transcript)
    {
        header.cursor = None;
    }
    Ok(header)
}
pub(super) fn page(
    connection: &Connection,
    conversation: &ConversationId,
    offset: Option<u64>,
    limit: usize,
) -> Result<ObservedTurnPage, ConversationStoreError> {
    if offset.is_some_and(|value| value > SAFE) {
        return Err(ConversationStoreError::InvalidContentRange);
    }
    if limit == 0 || limit > 64 {
        return Err(ConversationStoreError::InvalidContentRange);
    }
    let cut = super::reading::cut(connection, conversation)?;
    let missing:bool=connection.query_row("SELECT EXISTS(SELECT 1 FROM events INDEXED BY events_observation_turn_holes_idx WHERE observation_turn_required=1 AND observation_turn_header IS NULL AND sequence<=?1)",[cut.journal],|row|row.get(0)).map_err(|error|storage(error.to_string()))?;
    if missing {
        return Err(storage("native turn display source unavailable"));
    }
    let total = super::observation_statistics::read(connection, cut.journal)?.turns;
    let last = total.saturating_sub(1) / limit as u64 * limit as u64;
    let offset = offset.unwrap_or(last).min(last);
    if offset > SAFE {
        return Err(ConversationStoreError::InvalidContentRange);
    }
    let end = offset
        .checked_add(limit as u64)
        .ok_or(ConversationStoreError::InvalidContentRange)?
        .min(total);
    let mut turns = Vec::with_capacity(limit);
    for rank in offset..end {
        turns.push(selected(connection, conversation, &cut, rank + 1)?);
    }
    let page = ObservedTurnPage {
        cut,
        total,
        offset,
        turns,
    };
    if crate::runtime_client::checkpoint::bounded_bytes(&page, 786_432).is_none() {
        return Err(storage("turn page exceeds bounded reply"));
    }
    Ok(page)
}
pub(super) fn by_identity(
    connection: &Connection,
    conversation: &ConversationId,
    cut: &crate::durable::reading::ConversationReadCut,
    id: &crate::durable::reading::ConversationTurnId,
) -> Result<ObservedTurn, ConversationStoreError> {
    let local:Option<u64>=connection.query_row("SELECT observation_turn_ordinal FROM events INDEXED BY events_attempt_kind_idx WHERE attempt_id=?1 AND conversation_id=?2 AND json_extract(event_json,'$.event.type')='attempt_started' AND sequence<=?3 ORDER BY sequence LIMIT 1",params![id.attempt_id.as_str(),id.conversation_id.as_str(),cut.journal],|row|row.get(0)).optional().map_err(|error|storage(error.to_string()))?.flatten();
    let ordinal = if let Some(ordinal) = local {
        ordinal
    } else {
        let inherited:u64=connection.query_row("SELECT ordinal FROM inherited_turn_provenance INDEXED BY inherited_turn_identity_idx WHERE json_extract(payload,'$.id.conversation_id')=?1 AND json_extract(payload,'$.id.attempt_id')=?2",params![id.conversation_id.as_str(),id.attempt_id.as_str()],|row|row.get(0)).map_err(|_|ConversationStoreError::InvalidReference("turn absent from this conversation".into()))?;
        inherited
            .checked_add(1)
            .ok_or(ConversationStoreError::InvalidContentRange)?
    };
    selected(connection, conversation, cut, ordinal)
}
