//! Message Ledger and Surface appends and replay inside the caller's transaction.

use crate::conversation::{SurfaceOp, SurfaceRevision};
use crate::durable::sqlite::inbox::pending_message_exists;
use crate::durable::sqlite::{
    content, decode, encode, map_insert_error, nonnegative, observation, observation_state,
    observation_tools, observation_trace, optional_display_layout, projection, seq_to_i64, storage,
};
use crate::durable::{ConversationStoreError, DurableConversationHead, TranscriptCursor};
use crate::message::types::{AssistantContentBlock, InboundKind, MessageBlock, UserSource};
use crate::runtime::identity::{ConversationId, MessageId};
use rusqlite::OptionalExtension;
use rusqlite::{Connection, Transaction, params};
use std::collections::{BTreeMap, BTreeSet};

pub(in crate::durable::sqlite) fn ensure_surface_head(
    transaction: &Transaction<'_>,
) -> Result<(), ConversationStoreError> {
    let exists: bool = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM surface_head WHERE id=1)",
            [],
            |row| row.get(0),
        )
        .map_err(|e| storage(e.to_string()))?;
    if exists {
        return Ok(());
    }
    let ledger: i64 = transaction
        .query_row(
            "SELECT coalesce(max(position),0) FROM message_ledger",
            [],
            |row| row.get(0),
        )
        .map_err(|e| storage(e.to_string()))?;
    if ledger != 0 {
        return Err(ConversationStoreError::InvalidReference(
            "canonical frontier metadata missing".into(),
        ));
    }
    transaction
        .execute(
            "INSERT INTO surface_head(id,revision,compaction_generation) VALUES(1,0,0)",
            [],
        )
        .map_err(|e| storage(e.to_string()))?;
    Ok(())
}

pub(in crate::durable::sqlite) fn read_surface_head(
    connection: &Connection,
) -> Result<Option<(i64, i64)>, ConversationStoreError> {
    connection
        .query_row(
            "SELECT revision,compaction_generation FROM surface_head WHERE id=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| storage(format!("load Surface head: {error}")))
}

pub(in crate::durable::sqlite) fn head_frontier(
    connection: &Connection,
) -> Result<
    (
        i64,
        Option<crate::durable::projection::ProjectionCheckpoint>,
    ),
    ConversationStoreError,
> {
    let ledger: i64 = connection
        .query_row(
            "SELECT coalesce(max(position),0) FROM message_ledger",
            [],
            |row| row.get(0),
        )
        .map_err(|e| storage(e.to_string()))?;
    let revision: Option<i64> = connection
        .query_row("SELECT revision FROM surface_head WHERE id=1", [], |row| {
            row.get(0)
        })
        .optional()
        .map_err(|e| storage(e.to_string()))?;
    if revision.is_none_or(|revision| revision != ledger) && ledger != 0 {
        return Err(ConversationStoreError::InvalidReference(
            "canonical frontier metadata is missing or inconsistent".into(),
        ));
    }
    let checkpoint = projection::head(connection)?;
    let base = checkpoint
        .as_ref()
        .map_or(0, |checkpoint| checkpoint.candidate.base_frontier.0);
    if base > nonnegative(ledger, "canonical frontier")? {
        return Err(ConversationStoreError::InvalidReference(
            "checkpoint base exceeds canonical frontier".into(),
        ));
    }
    Ok((ledger, checkpoint))
}

pub(in crate::durable::sqlite) fn load_head(
    connection: &Connection,
) -> Result<DurableConversationHead, ConversationStoreError> {
    let (ledger, checkpoint) = head_frontier(connection)?;
    materialize_head(connection, ledger, checkpoint)
}

pub(in crate::durable::sqlite) const CANONICAL_SUFFIX_IDS: &str = "SELECT CASE WHEN octet_length(message_id)<=4096 THEN message_id END FROM message_ledger WHERE position>?1 AND position<=?2 ORDER BY position LIMIT 8193";

pub(in crate::durable::sqlite) fn materialize_head(
    connection: &Connection,
    ledger: i64,
    checkpoint: Option<crate::durable::projection::ProjectionCheckpoint>,
) -> Result<DurableConversationHead, ConversationStoreError> {
    use crate::context::checkpoint::CheckpointMessage;
    let mut ids = Vec::new();
    let mut seen = BTreeSet::new();
    let base = checkpoint
        .as_ref()
        .map_or(0, |checkpoint| checkpoint.candidate.base_frontier.0);
    if let Some(checkpoint) = &checkpoint {
        for item in &checkpoint.candidate.items {
            if let CheckpointMessage::Original(reference) = &item.message {
                if !ledger_message_exists(connection, &reference.message_id)? {
                    return Err(ConversationStoreError::InvalidReference(
                        "missing checkpoint Original dependency".into(),
                    ));
                }
                if seen.insert(reference.message_id.clone()) {
                    ids.push(reference.message_id.clone());
                }
            }
        }
    }
    let mut statement = connection
        .prepare(CANONICAL_SUFFIX_IDS)
        .map_err(|e| storage(e.to_string()))?;
    let rows = statement
        .query_map(params![seq_to_i64(base)?, ledger], |row| {
            row.get::<_, Option<String>>(0)
        })
        .map_err(|e| storage(e.to_string()))?;
    for row in rows {
        if ids.len() >= 8192 {
            return Err(crate::context::resources::bound(
                "canonical working set exceeds bounded projection size",
            ));
        }
        let id = MessageId::new(row.map_err(|e| storage(e.to_string()))?.ok_or_else(|| {
            crate::context::resources::bound("canonical identity record exceeds bound")
        })?);
        if seen.insert(id.clone()) {
            ids.push(id);
        }
    }
    Ok(DurableConversationHead {
        revision: SurfaceRevision::new(nonnegative(ledger, "canonical frontier")?),
        compaction_generation: checkpoint
            .map_or(0, |checkpoint| checkpoint.activation.activation.0),
        active_message_ids: ids,
    })
}

/// Appending needs the frontier, not an owned copy of every active identity.
/// Without a checkpoint the ledger UNIQUE identity constraint supplies the
/// uniqueness proof. Still visit the bounded suffix and validate each borrowed
/// UTF-8 value: this preserves corruption/resource checks without allocating an
/// identity vector and tree. This is a bounded scan, not constant-time context.
pub(in crate::durable::sqlite) fn append_head(
    connection: &Connection,
) -> Result<(SurfaceRevision, u64), ConversationStoreError> {
    let (ledger, checkpoint) = head_frontier(connection)?;
    if checkpoint.is_some() {
        // Original dependencies, duplicate suppression and suffix bounds retain
        // exactly the same checkpoint validation as full historical readers.
        let head = materialize_head(connection, ledger, checkpoint)?;
        return Ok((head.revision, head.compaction_generation));
    }
    validate_append_identities(connection, ledger)?;
    Ok((
        SurfaceRevision::new(nonnegative(ledger, "canonical frontier")?),
        0,
    ))
}

pub(in crate::durable::sqlite) fn validate_append_identities(
    connection: &Connection,
    ledger: i64,
) -> Result<usize, ConversationStoreError> {
    let mut statement = connection
        .prepare(CANONICAL_SUFFIX_IDS)
        .map_err(|e| storage(e.to_string()))?;
    let mut rows = statement
        .query(params![0, ledger])
        .map_err(|e| storage(e.to_string()))?;
    let mut count = 0;
    while let Some(row) = rows.next().map_err(|e| storage(e.to_string()))? {
        if count >= 8192 {
            return Err(crate::context::resources::bound(
                "canonical working set exceeds bounded projection size",
            ));
        }
        let value = row.get_ref(0).map_err(|e| storage(e.to_string()))?;
        if value == rusqlite::types::ValueRef::Null {
            return Err(crate::context::resources::bound(
                "canonical identity record exceeds bound",
            ));
        }
        value.as_str().map_err(|e| storage(e.to_string()))?;
        count += 1;
    }
    Ok(count)
}

pub(in crate::durable::sqlite) fn update_surface_head(
    transaction: &Transaction<'_>,
    revision: SurfaceRevision,
    generation: u64,
) -> Result<(), ConversationStoreError> {
    transaction
        .execute(
            "UPDATE surface_head SET revision=?1,compaction_generation=?2 WHERE id=1",
            params![seq_to_i64(revision.get())?, seq_to_i64(generation)?],
        )
        .map_err(|error| storage(format!("update Surface head: {error}")))?;
    Ok(())
}

/// Validates explicit canonical ownership; this index contains no independent
/// relationship evidence. Both ordinary commits and lineage seeds derive it from
/// the canonical Tool message itself, atomically with its Ledger append.
pub(in crate::durable::sqlite) fn associate_canonical_tool_result(
    transaction: &Transaction<'_>,
    message: &MessageBlock,
) -> Result<(), ConversationStoreError> {
    let MessageBlock::Tool(tool) = message else {
        return Ok(());
    };
    let owner = &tool.occurrence;
    let association: Option<(String, String, Option<String>)> = transaction
        .query_row(
            "SELECT call_id,tool_id,result_message_id FROM canonical_tool_calls
         WHERE assistant_message_id=?1 AND block_index=?2",
            params![owner.assistant_message_id.as_str(), owner.block_index.get()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|error| storage(format!("canonical Tool occurrence: {error}")))?;
    let Some((call_id, tool_id, previous)) = association else {
        return Err(ConversationStoreError::InvalidReference(
            "Tool result occurrence does not name a canonical ToolCall block".into(),
        ));
    };
    if call_id != tool.tool_call_id.as_str() || tool_id != tool.tool_id.as_str() {
        return Err(ConversationStoreError::InvalidReference(
            "Tool result correlation differs from its canonical occurrence".into(),
        ));
    }
    if previous.is_some() {
        return Err(ConversationStoreError::InvalidReference(
            "canonical Tool occurrence already has a result".into(),
        ));
    }
    transaction.execute("UPDATE canonical_tool_calls SET result_message_id=?1,result_ledger_position=(SELECT position FROM message_ledger WHERE message_id=?1) WHERE assistant_message_id=?2 AND block_index=?3", params![tool.id.as_str(), owner.assistant_message_id.as_str(), owner.block_index.get()])
        .map_err(|error| storage(format!("canonical Tool result index: {error}")))?;
    Ok(())
}

pub(in crate::durable::sqlite) fn append_message_and_surface(
    transaction: &Transaction<'_>,
    message: &MessageBlock,
) -> Result<Option<TranscriptCursor>, ConversationStoreError> {
    append_message_with_head(transaction, message, None, append_head(transaction)?)
}

/// Appends a destination-owned immutable canonical fact during lineage bootstrap.
/// Typed projections and historical activations are installed independently in
/// the same bootstrap transaction, without replaying source execution events.
pub(in crate::durable::sqlite) fn append_seeded_ledger_message(
    transaction: &Transaction<'_>,
    message: &MessageBlock,
) -> Result<(), ConversationStoreError> {
    append_message_ledger(transaction, message, None).map(|_| ())
}

pub(in crate::durable::sqlite) fn append_message_with_head(
    transaction: &Transaction<'_>,
    message: &MessageBlock,
    allowed_pending_message_id: Option<&MessageId>,
    (head_revision, generation): (SurfaceRevision, u64),
) -> Result<Option<TranscriptCursor>, ConversationStoreError> {
    let id = crate::conversation::message_id_of(message);
    let transcript_cursor =
        append_message_ledger(transaction, message, allowed_pending_message_id)?;
    let revision = head_revision.next();
    append_surface_op(
        transaction,
        revision,
        generation,
        &SurfaceOp::Append { message_id: id },
    )?;
    update_surface_head(transaction, revision, generation)?;
    Ok(transcript_cursor)
}

pub(in crate::durable::sqlite) fn append_message_ledger(
    transaction: &Transaction<'_>,
    message: &MessageBlock,
    allowed_pending_message_id: Option<&MessageId>,
) -> Result<Option<TranscriptCursor>, ConversationStoreError> {
    let id = crate::conversation::message_id_of(message);
    if pending_message_exists(transaction, &id)? && allowed_pending_message_id != Some(&id) {
        return Err(ConversationStoreError::DuplicateMessageId(id));
    }
    if ledger_message_exists(transaction, &id)? {
        return Err(ConversationStoreError::DuplicateMessageId(id));
    }
    let position: i64 = transaction
        .query_row(
            "SELECT COALESCE(MAX(position),0)+1 FROM message_ledger",
            [],
            |row| row.get(0),
        )
        .map_err(|error| storage(format!("next Ledger position: {error}")))?;
    let (ordinary_user, human_activity_at, opening_preview) = match message {
        MessageBlock::User(user) if user.kind == InboundKind::Message => (
            true,
            matches!(user.source, UserSource::Human)
                .then_some(user.timestamp)
                .flatten(),
            crate::local_runtime::session::preview_of(user),
        ),
        _ => (false, None, None),
    };
    let ordinary_human = matches!(message, MessageBlock::User(user) if user.kind == InboundKind::Message && user.source == UserSource::Human);
    let title_source_text = if ordinary_human {
        super::first_human::bounded_text(message)
    } else {
        None
    };
    transaction
        .execute(
            "INSERT INTO message_ledger(position,message_id,message_json,ordinary_user,human_activity_at,human_activity_seconds,human_activity_nanos,opening_preview,native_todo,ordinary_human,title_source_text) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            params![position, id.as_str(), encode(message, "canonical message")?,ordinary_user,human_activity_at.map(|time|time.to_rfc3339()),human_activity_at.map(|time|time.timestamp()),human_activity_at.map(|time|time.timestamp_subsec_nanos()),opening_preview,observation_state::todo_value(message).is_some(),ordinary_human,title_source_text],
        )
        .map_err(|error| map_insert_error(&error, &id))?;
    let _ = optional_display_layout(transaction, || {
        content::index_message(transaction, message)?;
        observation::index::index_message(
            transaction,
            message,
            Some(nonnegative(position, "header ledger position")?),
        )
    })?;
    if let MessageBlock::Assistant(assistant) = message {
        let mut tool_ordinal = 0_u64;
        for (index, block) in assistant.content.iter().enumerate() {
            if let AssistantContentBlock::ToolCall(call) = block {
                let block_index =
                    u32::try_from(index).map_err(|_| storage("Tool block index overflow"))?;
                transaction
                    .execute(
                        "INSERT INTO canonical_tool_calls(assistant_message_id,block_index,call_id,tool_id,observation_ledger,observation_call_id,observation_tool_id,observation_tool_ordinal,observation_name)
                         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                        params![assistant.id.as_str(), block_index, call.id.as_str(), call.tool_id.as_str(),position,(call.id.as_str().len()<=256).then_some(call.id.as_str()),(call.tool_id.as_str().len()<=256).then_some(call.tool_id.as_str()),tool_ordinal, observation_tools::name_preview(&call.name)],
                    )
                    .map_err(|error| storage(format!("canonical Tool occurrence: {error}")))?;
                tool_ordinal = tool_ordinal
                    .checked_add(1)
                    .ok_or_else(|| storage("Tool ordinal overflow"))?;
            }
        }
    }
    associate_canonical_tool_result(transaction, message)?;
    let _ = optional_display_layout(transaction, || {
        let conversation: String = transaction
            .query_row(
                "SELECT conversation_id FROM rustx_store WHERE id=1",
                [],
                |row| row.get(0),
            )
            .map_err(|error| storage(error.to_string()))?;
        let conversation = ConversationId::parse(conversation).map_err(storage)?;
        let position = nonnegative(position, "native Trace C position")?;
        observation_trace::index_proposals(transaction, &conversation, position, message)?;
        observation_trace::index_canonical(transaction, &conversation, position, message)
    })?;
    if transcript_visible_message(message) {
        Ok(Some(append_transcript_reference(
            transaction,
            "message",
            id.as_str(),
        )?))
    } else {
        Ok(None)
    }
}

/// Whether one canonical message is ordinary user-facing transcript content.
///
/// Context messages remain durable model history but are deliberately absent
/// from the normal transcript. Canonical conversation messages,
/// Assistant generations, and Tool results are visible semantic content.
pub(in crate::durable::sqlite) fn transcript_visible_message(message: &MessageBlock) -> bool {
    !matches!(
        message,
        MessageBlock::User(user)
            if matches!(user.kind, InboundKind::Context(_))
    )
}

/// Appends one low-frequency transcript reference, or verifies the existing
/// reference when an accepted Pending Inbound message is adopted into the
/// Ledger. No message, audit, or event body is stored here.
pub(in crate::durable::sqlite) fn append_transcript_reference(
    transaction: &Transaction<'_>,
    reference_kind: &str,
    reference_id: &str,
) -> Result<TranscriptCursor, ConversationStoreError> {
    let existing: Option<i64> = transaction
        .query_row(
            "SELECT position FROM transcript_order
             WHERE reference_kind=?1 AND reference_id=?2",
            params![reference_kind, reference_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| storage(format!("transcript reference probe: {error}")))?;
    if let Some(position) = existing {
        if reference_kind == "message" {
            transaction
                .execute(
                    "UPDATE message_ledger SET native_transcript=?1 WHERE message_id=?2",
                    params![position, reference_id],
                )
                .map_err(|error| storage(error.to_string()))?;
        }
        let _ = optional_display_layout(transaction, || {
            transaction.execute("UPDATE observation_headers SET transcript_position=?1,header_json=CASE WHEN reference_kind='message' THEN CAST(json_set(header_json,'$.transcript_cursor',?1) AS BLOB) ELSE header_json END WHERE reference_kind=?2 AND reference_id=?3",params![position,reference_kind,reference_id]).map_err(|error| super::super::sqlite_error(&error, error.to_string()))?;
            Ok(())
        })?;
        return Ok(TranscriptCursor::new(
            u64::try_from(position).map_err(|_| storage("negative transcript position"))?,
        ));
    }
    let current: i64 = transaction
        .query_row(
            "SELECT next_transcript_position FROM rustx_store WHERE id=1",
            [],
            |row| row.get(0),
        )
        .map_err(|error| storage(format!("read transcript position: {error}")))?;
    let position = current
        .checked_add(1)
        .ok_or_else(|| storage("transcript position exhausted"))?;
    transaction
        .execute(
            "INSERT INTO transcript_order(position,reference_kind,reference_id) VALUES(?1,?2,?3)",
            params![position, reference_kind, reference_id],
        )
        .map_err(|error| storage(format!("insert transcript reference: {error}")))?;
    let _ = optional_display_layout(transaction, || {
        transaction
            .execute(
                "INSERT INTO transcript_observation_readiness(position,ready) VALUES(?1,0)",
                [position],
            )
            .map_err(|error| storage(error.to_string()))?;
        transaction
            .execute(
                "UPDATE transcript_order SET observation_readiness_missing=0 WHERE position=?1",
                [position],
            )
            .map_err(|error| storage(error.to_string()))?;
        Ok(())
    })?;
    transaction
        .execute(
            "UPDATE rustx_store SET next_transcript_position=?1,retained_transcript_count=retained_transcript_count+1 WHERE id=1",
            [position],
        )
        .map_err(|error| storage(format!("update transcript position: {error}")))?;
    if reference_kind == "message" {
        transaction
            .execute(
                "UPDATE message_ledger SET native_transcript=?1 WHERE message_id=?2",
                params![position, reference_id],
            )
            .map_err(|error| storage(error.to_string()))?;
    }
    let _ = optional_display_layout(transaction, || {
        transaction.execute("UPDATE observation_headers SET transcript_position=?1,header_json=CASE WHEN reference_kind='message' THEN CAST(json_set(header_json,'$.transcript_cursor',?1) AS BLOB) ELSE header_json END WHERE reference_kind=?2 AND reference_id=?3",params![position,reference_kind,reference_id]).map_err(|error| super::super::sqlite_error(&error, error.to_string()))?;
        Ok(())
    })?;
    Ok(TranscriptCursor::new(
        u64::try_from(position).map_err(|_| storage("negative transcript position"))?,
    ))
}

pub(in crate::durable::sqlite) fn load_transcript_reference(
    transaction: &Transaction<'_>,
    reference_kind: &str,
    reference_id: &str,
) -> Result<Option<TranscriptCursor>, ConversationStoreError> {
    transaction
        .query_row(
            "SELECT position FROM transcript_order
             WHERE reference_kind=?1 AND reference_id=?2",
            params![reference_kind, reference_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(|error| storage(format!("transcript reference lookup: {error}")))?
        .map(|position| {
            u64::try_from(position)
                .map(TranscriptCursor::new)
                .map_err(|_| storage("negative transcript position"))
        })
        .transpose()
}

pub(in crate::durable::sqlite) fn append_surface_op(
    transaction: &Transaction<'_>,
    revision: SurfaceRevision,
    generation: u64,
    operation: &SurfaceOp,
) -> Result<(), ConversationStoreError> {
    transaction
        .execute(
            "INSERT INTO surface_ops(revision,compaction_generation,op_json) VALUES(?1,?2,?3)",
            params![
                seq_to_i64(revision.get())?,
                seq_to_i64(generation)?,
                encode(operation, "Surface operation")?
            ],
        )
        .map_err(|error| storage(format!("append Surface operation: {error}")))?;
    let SurfaceOp::Append { message_id } = operation;
    {
        transaction.execute("UPDATE message_ledger SET first_append_revision=COALESCE(first_append_revision,?1) WHERE message_id=?2",params![revision.get(),message_id.as_str()]).map_err(|error|storage(error.to_string()))?;
        // Existing native Surface commit owns first appearance. This index
        // never rewrites Surface membership, Compaction or canonical history.
        let _ = optional_display_layout(transaction, || {
            let boundary: Option<u64> = transaction
                .query_row(
                    "SELECT position FROM message_ledger WHERE message_id=?1 AND ordinary_user=1",
                    [message_id.as_str()],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|error| storage(error.to_string()))?;
            if let Some(position) = boundary {
                transaction.execute("INSERT OR IGNORE INTO surface_observation_boundaries SELECT COALESCE((SELECT MAX(ordinal)+1 FROM surface_observation_boundaries),0),?1,?2",params![position,seq_to_i64(revision.get())?]).map_err(|error|storage(error.to_string()))?;
                transaction
                    .execute(
                        "UPDATE message_ledger SET boundary_indexed=1 WHERE position=?1",
                        [position],
                    )
                    .map_err(|error| storage(error.to_string()))?;
            }
            Ok(())
        })?;
    }
    Ok(())
}

pub(in crate::durable::sqlite) fn message_exists(
    connection: &Connection,
    message_id: &MessageId,
) -> Result<bool, ConversationStoreError> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pending_inbound WHERE message_id=?1) OR EXISTS(SELECT 1 FROM message_ledger WHERE message_id=?1)",
            [message_id.as_str()],
            |row| row.get(0),
        )
        .map_err(|error| storage(format!("message identity probe: {error}")))
}

/// Both lookups use existing unique Ledger identity and Journal sequence keys.
pub(in crate::durable::sqlite) fn load_message(
    connection: &Connection,
    message_id: &MessageId,
) -> Result<MessageBlock, ConversationStoreError> {
    let json: String = connection
        .prepare_cached("SELECT message_json FROM message_ledger WHERE message_id=?1")
        .map_err(|error| storage(format!("Ledger lookup prepare: {error}")))?
        .query_row([message_id.as_str()], |row| row.get(0))
        .optional()
        .map_err(|error| storage(format!("Ledger message lookup: {error}")))?
        .ok_or_else(|| {
            ConversationStoreError::InvalidReference(format!("message {message_id} is unavailable"))
        })?;
    let message: MessageBlock = decode(&json, "Ledger message")?;
    if crate::conversation::message_id_of(&message) != *message_id {
        return Err(ConversationStoreError::InvalidReference(format!(
            "Ledger row for {message_id} contains a different message identity"
        )));
    }
    Ok(message)
}

pub(in crate::durable::sqlite) fn load_message_tx(
    transaction: &Transaction<'_>,
    message_id: &MessageId,
) -> Result<MessageBlock, ConversationStoreError> {
    load_message(transaction, message_id)
}

pub(in crate::durable::sqlite) fn validate_surface_operation_references_from_ledger(
    ledger: &BTreeMap<MessageId, MessageBlock>,
    operation: &SurfaceOp,
) -> Result<(), ConversationStoreError> {
    let SurfaceOp::Append { message_id } = operation;
    let references = [message_id];
    for message_id in references {
        if !ledger.contains_key(message_id) {
            return Err(ConversationStoreError::InvalidReference(format!(
                "Surface operation references missing Ledger message {message_id}"
            )));
        }
    }

    Ok(())
}

pub(in crate::durable::sqlite) fn reconstruct_surface(
    connection: &Connection,
    revision: SurfaceRevision,
) -> Result<Vec<MessageId>, ConversationStoreError> {
    Ok(replay_surface_history(connection, revision)?.0)
}

/// Reads the retained operations through `revision`, validating the same
/// contiguity, generation, and reference invariants a reconstruction does,
/// and returns both the Surface they denote and the operations themselves.
///
/// The operations are the half a lineage copy needs: they are what a later
/// fork or tree of the copy reads when it looks for its own branch points.
pub(in crate::durable::sqlite) fn load_surface_history(
    connection: &Connection,
    through: SurfaceRevision,
) -> Result<Vec<SurfaceOp>, ConversationStoreError> {
    Ok(replay_surface_history(connection, through)?.1)
}

pub(in crate::durable::sqlite) fn replay_surface_history(
    connection: &Connection,
    revision: SurfaceRevision,
) -> Result<(Vec<MessageId>, Vec<SurfaceOp>), ConversationStoreError> {
    let Some((head_revision, _)) = read_surface_head(connection)? else {
        if revision == SurfaceRevision::INITIAL {
            return Ok((Vec::new(), Vec::new()));
        }
        return Err(ConversationStoreError::InvalidReference(format!(
            "Surface revision {revision} has no durable head"
        )));
    };
    let head_revision = SurfaceRevision::new(nonnegative(head_revision, "Surface revision")?);
    if revision > head_revision {
        return Err(ConversationStoreError::InvalidReference(format!(
            "Surface revision {revision} is newer than head {head_revision}"
        )));
    }
    let mut active = Vec::new();
    let mut history = Vec::new();
    let mut statement = connection
        .prepare(
            "SELECT revision,compaction_generation,op_json
             FROM surface_ops WHERE revision <= ?1 ORDER BY revision",
        )
        .map_err(|error| storage(format!("read Surface history: {error}")))?;
    let rows = statement
        .query_map([seq_to_i64(revision.get())?], |row| {
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
        validate_surface_operation_references(connection, &operation)?;
        apply_surface_op(&mut active, &operation)?;
        history.push(operation);
        expected_revision = expected_revision
            .checked_add(1)
            .ok_or_else(|| storage("Surface revision is exhausted"))?;
    }
    let count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM surface_ops WHERE revision <= ?1",
            [seq_to_i64(revision.get())?],
            |row| row.get(0),
        )
        .map_err(|error| storage(format!("Surface history count: {error}")))?;
    let expected_next_revision = revision
        .get()
        .checked_add(1)
        .ok_or_else(|| storage("Surface revision is exhausted"))?;
    if expected_revision != expected_next_revision || count != seq_to_i64(revision.get())? {
        return Err(ConversationStoreError::InvalidReference(format!(
            "Surface revision {revision} has a non-contiguous operation history"
        )));
    }
    Ok((active, history))
}

pub(in crate::durable::sqlite) fn validate_surface_operation_references(
    connection: &Connection,
    operation: &SurfaceOp,
) -> Result<(), ConversationStoreError> {
    let SurfaceOp::Append { message_id } = operation;
    let references = [message_id];
    for message_id in references {
        if !ledger_message_exists(connection, message_id)? {
            return Err(ConversationStoreError::InvalidReference(format!(
                "Surface operation references missing Ledger message {message_id}"
            )));
        }
    }

    Ok(())
}

pub(in crate::durable::sqlite) fn validate_canonical_frontier(
    connection: &Connection,
    frontier: crate::context::resources::LedgerFrontier,
) -> Result<(), ConversationStoreError> {
    let head: i64 = connection
        .query_row(
            "SELECT coalesce(max(position),0) FROM message_ledger",
            [],
            |row| row.get(0),
        )
        .map_err(|e| storage(e.to_string()))?;
    if frontier.0 > nonnegative(head, "canonical frontier")? {
        return Err(ConversationStoreError::InvalidReference(
            "snapshot canonical frontier is beyond durable history".into(),
        ));
    }
    Ok(())
}

/// The one definition of what a Surface operation means lives in
/// `conversation::surface`; this is the durable domain's error wrapper for
/// it, so a reconstruction here and a seed validation there can never
/// disagree about the Surface an operation log denotes.
pub(in crate::durable::sqlite) fn apply_surface_op(
    active: &mut Vec<MessageId>,
    operation: &SurfaceOp,
) -> Result<(), ConversationStoreError> {
    crate::conversation::apply_surface_op(active, operation)
        .map_err(ConversationStoreError::InvalidReference)
}

pub(in crate::durable::sqlite) fn ledger_message_exists(
    connection: &Connection,
    message_id: &MessageId,
) -> Result<bool, ConversationStoreError> {
    connection
        .prepare_cached("SELECT EXISTS(SELECT 1 FROM message_ledger WHERE message_id=?1)")
        .and_then(|mut statement| statement.query_row([message_id.as_str()], |row| row.get(0)))
        .map_err(|error| storage(format!("event Ledger reference: {error}")))
}
