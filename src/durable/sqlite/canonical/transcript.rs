//! Bounded reads of the derived transcript ordering spine and Surface boundaries.

use crate::conversation::{SurfaceOp, SurfaceRevision};
use crate::durable::sqlite::canonical::ledger::{
    apply_surface_op, read_surface_head, validate_surface_operation_references_from_ledger,
};
use crate::durable::sqlite::{decode, nonnegative, provenance, seq_to_i64, storage};
use crate::durable::{
    ConversationStoreError, SurfaceUserMessageBoundary, SurfaceUserMessageBoundaryPage,
    TRANSCRIPT_PAGE_LIMIT_MAX, TranscriptCursor, TranscriptEntry, TranscriptItem, TranscriptPage,
};
use crate::events::types::{RuntimeEvent, RuntimeEventEnvelope};
use crate::message::types::{AssistantContentBlock, InboundKind, MessageBlock, UserMessageBlock};
use crate::publication::PublicationAudit;
use crate::runtime::identity::MessageId;
use rusqlite::OptionalExtension;
use rusqlite::{Connection, params};
use std::collections::{BTreeMap, BTreeSet};

pub(in crate::durable::sqlite) fn load_canonical_rows(
    connection: &Connection,
) -> Result<Vec<MessageBlock>, ConversationStoreError> {
    let mut statement = connection
        .prepare("SELECT message_id,message_json FROM message_ledger ORDER BY position")
        .map_err(|error| storage(format!("load Ledger: {error}")))?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|error| storage(format!("load Ledger query: {error}")))?;
    rows.map(|row| {
        let (stored_id, json) =
            row.map_err(|error| storage(format!("load Ledger row: {error}")))?;
        let message: MessageBlock = decode(&json, "Ledger message")?;
        let actual_id = crate::conversation::message_id_of(&message);
        if actual_id.as_str() != stored_id {
            return Err(ConversationStoreError::InvalidReference(format!(
                "Ledger row {stored_id} contains message {actual_id}"
            )));
        }
        Ok(message)
    })
    .collect()
}

/// One indexed range per Assistant, then indexed result-message seeks.
/// No provider-id query or JSON filtering of the historical Ledger.
pub(in crate::durable::sqlite) fn load_transcript_tools(
    connection: &Connection,
    assistant: &crate::message::types::AssistantMessageBlock,
) -> Result<Vec<crate::durable::TranscriptTool>, ConversationStoreError> {
    let mut tool_calls = Vec::new();
    let mut statement = connection
        .prepare(TRANSCRIPT_TOOLS_SQL)
        .map_err(|error| storage(format!("transcript Tool associations: {error}")))?;
    let rows = statement
        .query_map([assistant.id.as_str()], |row| {
            Ok((row.get::<_, u32>(0)?, row.get::<_, Option<String>>(1)?))
        })
        .map_err(|error| storage(format!("transcript Tool rows: {error}")))?;
    for row in rows {
        let (block_index, result_json) =
            row.map_err(|error| storage(format!("transcript Tool row: {error}")))?;
        let Some(AssistantContentBlock::ToolCall(call)) =
            assistant.content.get(block_index as usize)
        else {
            return Err(storage(
                "canonical Tool occurrence references a non-Tool block",
            ));
        };
        let result = result_json
            .map(|json| decode::<MessageBlock>(&json, "transcript Tool result"))
            .transpose()?
            .map(|message| match message {
                MessageBlock::Tool(tool)
                    if tool.occurrence.assistant_message_id == assistant.id
                        && tool.occurrence.block_index.get() == block_index
                        && tool.tool_call_id == call.id
                        && tool.tool_id == call.tool_id =>
                {
                    Ok(tool.result)
                }
                _ => Err(storage(
                    "canonical Tool occurrence references a foreign result",
                )),
            })
            .transpose()?;
        tool_calls.push(crate::durable::TranscriptTool {
            message_id: assistant.id.clone(),
            block_index: crate::message::types::ContentBlockIndex::new(block_index),
            call: call.clone(),
            result,
        });
    }
    Ok(tool_calls)
}

pub(in crate::durable::sqlite) const TRANSCRIPT_TOOLS_SQL: &str =
    "SELECT c.block_index,l.message_json FROM canonical_tool_calls c
    LEFT JOIN message_ledger l ON l.message_id=c.result_message_id
    WHERE c.assistant_message_id=?1 ORDER BY c.block_index";

/// Reads the bounded transcript ordering spine newest-first, resolves each
/// reference through its canonical durable owner, and returns the selected
/// rows in chronological order.
pub(in crate::durable::sqlite) fn load_transcript_page(
    connection: &Connection,
    before: Option<TranscriptCursor>,
    limit: usize,
) -> Result<TranscriptPage, ConversationStoreError> {
    if limit == 0 {
        return Ok(TranscriptPage {
            inherited_through: inherited_transcript_through(connection)?,
            entries: Vec::new(),
            next_cursor: None,
        });
    }
    if limit > TRANSCRIPT_PAGE_LIMIT_MAX {
        return Err(storage(format!(
            "transcript page limit {limit} exceeds maximum {TRANSCRIPT_PAGE_LIMIT_MAX}"
        )));
    }
    let before = before
        .map(|cursor| seq_to_i64(cursor.get()))
        .transpose()?
        .unwrap_or(i64::MAX);
    let fetch_limit = i64::try_from(limit + 1)
        .map_err(|_| storage("transcript page limit is not representable"))?;
    let mut statement = connection
        .prepare(
            "SELECT position,reference_kind,reference_id
             FROM transcript_order
             WHERE position < ?1
             ORDER BY position DESC
             LIMIT ?2",
        )
        .map_err(|error| storage(format!("transcript page: {error}")))?;
    let rows = statement
        .query_map(params![before, fetch_limit], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|error| storage(format!("transcript page query: {error}")))?;
    let mut references = rows
        .map(|row| row.map_err(|error| storage(format!("transcript page row: {error}"))))
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);

    let has_more = references.len() > limit;
    references.truncate(limit);
    let next_cursor = has_more
        .then(|| references.last().map(|(position, _, _)| *position))
        .flatten()
        .map(|position| {
            TranscriptCursor::new(
                u64::try_from(position).expect("transcript positions are non-negative"),
            )
        });
    references.reverse();
    let entries = references
        .into_iter()
        .map(|(position, reference_kind, reference_id)| {
            let cursor = TranscriptCursor::new(
                u64::try_from(position).map_err(|_| storage("negative transcript position"))?,
            );
            let item = load_transcript_item(connection, &reference_kind, &reference_id)?;
            let mut tool_calls = Vec::new();
            if let TranscriptItem::Message {
                message: MessageBlock::Assistant(assistant),
            } = &item
            {
                tool_calls = load_transcript_tools(connection, assistant)?;
            }
            Ok(TranscriptEntry {
                tool_calls,
                cursor,
                item,
            })
        })
        .collect::<Result<Vec<_>, ConversationStoreError>>()?;
    Ok(TranscriptPage {
        inherited_through: inherited_transcript_through(connection)?,
        entries,
        next_cursor,
    })
}

pub(in crate::durable::sqlite) const INHERITED_TRANSCRIPT_THROUGH_SQL: &str =
    "SELECT COALESCE((SELECT inherited_transcript_through FROM bootstrap_identity WHERE id=1),0)";

/// The seed owns canonical Ledger positions, not remapped message spelling or
/// origin Conversation IDs. Context messages have no transcript row, while
/// terminal-only inherited turns do: both are covered by this native boundary.
/// Bootstrap persists its final transcript watermark atomically with the seed;
/// local appends and repeated bounded reads never recompute the inherited prefix.
pub(in crate::durable::sqlite) fn inherited_transcript_through(
    connection: &Connection,
) -> Result<TranscriptCursor, ConversationStoreError> {
    let position: i64 = connection
        .prepare_cached(INHERITED_TRANSCRIPT_THROUGH_SQL)
        .map_err(|error| storage(format!("inherited transcript boundary: {error}")))?
        .query_row([], |row| row.get(0))
        .map_err(|error| storage(format!("inherited transcript boundary: {error}")))?;
    Ok(TranscriptCursor::new(nonnegative(
        position,
        "inherited transcript boundary",
    )?))
}

/// Resolves one transcript reference without copying its body into the
/// ordering table.
pub(in crate::durable::sqlite) fn load_transcript_item(
    connection: &Connection,
    reference_kind: &str,
    reference_id: &str,
) -> Result<TranscriptItem, ConversationStoreError> {
    match reference_kind {
        "inherited_turn" => Ok(TranscriptItem::InheritedTurn {
            provenance: provenance::turn_at(
                connection,
                reference_id
                    .parse::<i64>()
                    .map_err(|error| storage(error.to_string()))?,
            )?,
        }),

        "message" => {
            let ledger_json: Option<String> = connection
                .query_row(
                    "SELECT message_json FROM message_ledger WHERE message_id=?1",
                    [reference_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|error| storage(format!("transcript Ledger lookup: {error}")))?;
            let message = if let Some(json) = ledger_json {
                decode::<MessageBlock>(&json, "transcript message")?
            } else {
                let json: String = connection
                    .query_row(
                        "SELECT message_json FROM pending_inbound WHERE message_id=?1",
                        [reference_id],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(|error| storage(format!("transcript pending lookup: {error}")))?
                    .ok_or_else(|| {
                        ConversationStoreError::InvalidReference(format!(
                            "transcript message reference {reference_id} has no durable owner"
                        ))
                    })?;
                MessageBlock::User(decode::<UserMessageBlock>(
                    &json,
                    "transcript pending message",
                )?)
            };
            if crate::conversation::message_id_of(&message).as_str() != reference_id {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "transcript message reference {reference_id} contains a different message"
                )));
            }
            Ok(TranscriptItem::Message { message })
        }
        "publication_audit" => {
            let json: String = connection
                .query_row(
                    "SELECT audit_json FROM publication_audits WHERE stream_id=?1",
                    [reference_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|error| storage(format!("transcript publication audit lookup: {error}")))?
                .ok_or_else(|| {
                    ConversationStoreError::InvalidReference(format!(
                        "transcript publication audit reference {reference_id} has no durable audit"
                    ))
                })?;
            let audit: PublicationAudit = decode(&json, "transcript publication audit")?;
            if audit.stream_id.as_str() != reference_id {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "transcript publication audit reference {reference_id} contains a different stream"
                )));
            }
            Ok(TranscriptItem::PublicationAudit { audit })
        }
        "interaction_event" | "attempt_terminal" => {
            load_transcript_event(connection, reference_kind, reference_id)
        }
        other => Err(ConversationStoreError::InvalidReference(format!(
            "unknown transcript reference kind {other}"
        ))),
    }
}

pub(in crate::durable::sqlite) fn load_transcript_event(
    connection: &Connection,
    reference_kind: &str,
    reference_id: &str,
) -> Result<TranscriptItem, ConversationStoreError> {
    let json: String = connection
        .query_row(
            "SELECT event_json FROM events WHERE event_id=?1",
            [reference_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| storage(format!("transcript event lookup: {error}")))?
        .ok_or_else(|| {
            ConversationStoreError::InvalidReference(format!(
                "transcript event reference {reference_id} has no durable event"
            ))
        })?;
    let event: RuntimeEventEnvelope = decode(&json, "transcript event")?;
    if event.event_id.as_str() != reference_id {
        return Err(ConversationStoreError::InvalidReference(format!(
            "transcript event reference {reference_id} contains a different event"
        )));
    }
    match (reference_kind, &event.event) {
        (
            "attempt_terminal",
            RuntimeEvent::AttemptCancelled { .. }
            | RuntimeEvent::AttemptFailed { .. }
            | RuntimeEvent::AttemptTimedOut { .. }
            | RuntimeEvent::AttemptLimitExceeded { .. },
        ) => Ok(TranscriptItem::AttemptTerminal { event }),
        ("interaction_event", RuntimeEvent::InteractionRequested { .. }) => {
            Ok(TranscriptItem::InteractionRequested { event })
        }
        ("interaction_event", RuntimeEvent::InteractionSettled { .. }) => {
            Ok(TranscriptItem::InteractionSettled { event })
        }
        _ => Err(ConversationStoreError::InvalidReference(format!(
            "transcript event reference {reference_id} is not a matching transcript event"
        ))),
    }
}

#[allow(clippy::too_many_lines)]
pub(in crate::durable::sqlite) fn load_user_message_boundaries(
    connection: &Connection,
    through: SurfaceRevision,
) -> Result<Vec<SurfaceUserMessageBoundary>, ConversationStoreError> {
    Ok(load_user_message_boundaries_page_internal(connection, through, None)?.boundaries)
}

#[allow(clippy::too_many_lines)]
pub(in crate::durable::sqlite) fn load_user_message_boundaries_page_internal(
    connection: &Connection,
    through: SurfaceRevision,
    page: Option<(usize, usize)>,
) -> Result<SurfaceUserMessageBoundaryPage, ConversationStoreError> {
    if let Some((_, limit)) = page
        && limit == 0
    {
        return Err(storage(
            "historical user-message boundary page limit must be positive",
        ));
    }
    let Some((head_revision, _)) = read_surface_head(connection)? else {
        if through == SurfaceRevision::INITIAL {
            return Ok(SurfaceUserMessageBoundaryPage {
                boundaries: Vec::new(),
                next_offset: None,
            });
        }
        return Err(ConversationStoreError::InvalidReference(format!(
            "Surface revision {through} has no durable head"
        )));
    };
    let head_revision = SurfaceRevision::new(nonnegative(head_revision, "Surface revision")?);
    if through > head_revision {
        return Err(ConversationStoreError::InvalidReference(format!(
            "Surface revision {through} is newer than head {head_revision}"
        )));
    }

    let ledger: BTreeMap<MessageId, MessageBlock> = load_canonical_rows(connection)?
        .into_iter()
        .map(|message| {
            let id = crate::conversation::message_id_of(&message);
            (id, message)
        })
        .collect();
    let mut active = Vec::new();
    let mut boundaries = Vec::new();
    let mut boundary_count = 0_usize;
    let mut seen = BTreeSet::new();
    let mut statement = connection
        .prepare(
            "SELECT revision,compaction_generation,op_json
             FROM surface_ops WHERE revision <= ?1 ORDER BY revision",
        )
        .map_err(|error| storage(format!("read Surface history: {error}")))?;
    let rows = statement
        .query_map([seq_to_i64(through.get())?], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|error| storage(format!("read Surface history query: {error}")))?;
    let mut expected_revision = 1_u64;
    for row in rows {
        let (stored_revision, stored_generation, json) =
            row.map_err(|error| storage(format!("Surface history row: {error}")))?;
        if u64::try_from(stored_revision).ok() != Some(expected_revision) {
            return Err(ConversationStoreError::InvalidReference(
                "Surface operation revisions are not contiguous from revision 1".to_owned(),
            ));
        }
        let _ = nonnegative(stored_generation, "canonical append metadata")?;
        let operation: SurfaceOp = decode(&json, "Surface operation")?;
        validate_surface_operation_references_from_ledger(&ledger, &operation)?;
        let appended_message_id = match &operation {
            SurfaceOp::Append { message_id } => Some(message_id.clone()),
        };
        apply_surface_op(&mut active, &operation)?;
        if let Some(message_id) = appended_message_id
            && let Some(MessageBlock::User(user)) = ledger.get(&message_id)
            && user.kind == InboundKind::Message
            && seen.insert(message_id)
        {
            let boundary = SurfaceUserMessageBoundary {
                surface_revision: SurfaceRevision::new(expected_revision),
                message: user.clone(),
            };
            let include = page.is_none_or(|(offset, limit)| {
                boundary_count >= offset && boundary_count < offset.saturating_add(limit)
            });
            if include {
                boundaries.push(boundary);
            }
            boundary_count = boundary_count
                .checked_add(1)
                .ok_or_else(|| storage("historical user-message boundary count exhausted"))?;
        }
        expected_revision = expected_revision
            .checked_add(1)
            .ok_or_else(|| storage("Surface revision is exhausted"))?;
    }
    let expected_next_revision = through
        .get()
        .checked_add(1)
        .ok_or_else(|| storage("Surface revision is exhausted"))?;
    if expected_revision != expected_next_revision {
        return Err(ConversationStoreError::InvalidReference(format!(
            "Surface revision {through} has a non-contiguous operation history"
        )));
    }
    let next_offset = page.and_then(|(offset, _)| {
        let end = offset.saturating_add(boundaries.len());
        (end < boundary_count).then_some(end)
    });
    Ok(SurfaceUserMessageBoundaryPage {
        boundaries,
        next_offset,
    })
}

pub(in crate::durable::sqlite) fn load_user_message_boundaries_page(
    connection: &Connection,
    through: SurfaceRevision,
    offset: usize,
    limit: usize,
) -> Result<SurfaceUserMessageBoundaryPage, ConversationStoreError> {
    load_user_message_boundaries_page_internal(connection, through, Some((offset, limit)))
}

pub(in crate::durable::sqlite) const TRANSCRIPT_ATTEMPTS_SQL: &str = "SELECT attempt_id FROM events INDEXED BY events_message_id_kind_idx WHERE json_extract(event_json,'$.event.message_id')=?1 AND json_extract(event_json,'$.event.type')='assistant_message_committed' AND sequence<=?2";
