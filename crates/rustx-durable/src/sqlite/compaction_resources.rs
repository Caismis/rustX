//! Short, bounded source reads for the Context Engine. No persistent derived cache.
use super::{
    Connection, ConversationStoreError, OptionalExtension, decode, nonnegative, params, seq_to_i64,
    sqlite_error, storage,
};
use crate::TranscriptCursor;
use crate::context_resources::{
    JournalFrontier, LedgerFrontier, ResourceLimits, ResourcePage, ResourceReadCut, bound,
};
use crate::sqlite::statements::FixedStatements;
use rustx_types::events::types::{EVENT_SCHEMA_VERSION, RuntimeEventEnvelope};
use rustx_types::message::types::MessageBlock;
use rustx_types::runtime::identity::{ConversationId, MessageId};

pub fn cut(
    connection: &Connection,
    conversation: &ConversationId,
) -> Result<ResourceReadCut, ConversationStoreError> {
    let reading = super::reading::cut(connection, conversation)?;
    let ledger: i64 = connection
        .query_row_cached(
            "SELECT coalesce(max(position),0) FROM message_ledger",
            [],
            |row| row.get(0),
        )
        .map_err(|e| sqlite_error(&e, e.to_string()))?;
    let activation: u64 = connection
        .query_row_cached(
            "SELECT coalesce(max(activation),0) FROM projection_activations",
            [],
            |row| row.get(0),
        )
        .map_err(|e| sqlite_error(&e, e.to_string()))?;
    Ok(ResourceReadCut {
        projection_activation: rustx_types::context::checkpoint::ProjectionActivationId(activation),
        conversation_id: conversation.clone(),
        ledger: LedgerFrontier(nonnegative(ledger, "compaction Ledger cut")?),
        journal: JournalFrontier(reading.journal),
        transcript: TranscriptCursor::new(reading.transcript),
    })
}

pub fn validate(
    connection: &Connection,
    conversation: &ConversationId,
    expected: &ResourceReadCut,
    limits: ResourceLimits,
) -> Result<(), ConversationStoreError> {
    limits.validate()?;
    let actual = cut(connection, conversation)?;
    if expected.conversation_id != actual.conversation_id
        || expected.ledger > actual.ledger
        || expected.journal > actual.journal
        || expected.transcript > actual.transcript
        || expected.projection_activation > actual.projection_activation
    {
        return Err(ConversationStoreError::InvalidReference(
            "foreign or future compaction Resource Read Cut".into(),
        ));
    }
    // Cut authority is normalized publication metadata. Canonical source reads
    // must not decode an unrelated current projection payload.
    if expected.projection_activation.0 > 0 {
        crate::sqlite::projection::validate_activation_boundary(
            connection,
            expected.projection_activation,
            expected.ledger,
            expected.journal,
        )?;
    }
    Ok(())
}

/// Charge SQLite's raw byte size before moving the body into an owned Rust string.
fn body(
    row: &rusqlite::Row<'_>,
    page_bytes: &mut usize,
    limits: ResourceLimits,
) -> Result<String, ConversationStoreError> {
    let length: i64 = row.get(2).map_err(|e| storage(e.to_string()))?;
    let length = usize::try_from(length).map_err(|_| bound("invalid source record length"))?;
    let charged = page_bytes
        .checked_add(length)
        .ok_or_else(|| bound("source byte size overflow"))?;
    if length > limits.record_bytes || charged > limits.page_bytes {
        return Err(bound(
            "compaction source record or page byte bound exceeded",
        ));
    }
    let json: Option<String> = row.get(1).map_err(|e| storage(e.to_string()))?;
    *page_bytes = charged;
    json.ok_or_else(|| bound("compaction source record exceeds byte bound"))
}

#[cfg(any(test, feature = "test-hooks"))]
thread_local! { static BODY_DECODES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
#[cfg(any(test, feature = "test-hooks"))]
pub fn take_body_decodes() -> usize {
    BODY_DECODES.with(|count| count.replace(0))
}
fn record_body_decode() {
    #[cfg(any(test, feature = "test-hooks"))]
    BODY_DECODES.with(|count| count.set(count.get() + 1));
}

pub fn messages(
    connection: &Connection,
    conversation: &ConversationId,
    cut: &ResourceReadCut,
    after: LedgerFrontier,
    ids: Option<&[MessageId]>,
    limits: ResourceLimits,
) -> Result<ResourcePage<MessageBlock>, ConversationStoreError> {
    validate(connection, conversation, cut, limits)?;
    let mut page = ResourcePage {
        rows: Vec::new(),
        bytes: 0,
    };
    if let Some(ids) = ids {
        if ids.len() > limits.page_items
            || ids.iter().collect::<std::collections::BTreeSet<_>>().len() != ids.len()
        {
            return Err(bound("oversized or duplicate compaction keyed read"));
        }
        let mut query = connection.prepare_cached("SELECT position,CASE WHEN octet_length(message_json)<=?3 THEN message_json END,octet_length(message_json) FROM message_ledger WHERE message_id=?1 AND position<=?2")
            .map_err(|e| storage(e.to_string()))?;
        for id in ids {
            let row = query
                .query_row(
                    params![id.as_str(), seq_to_i64(cut.ledger.0)?, limits.record_bytes],
                    |row| Ok((row.get::<_, i64>(0)?, body(row, &mut page.bytes, limits))),
                )
                .optional()
                .map_err(|e| storage(e.to_string()))?
                .ok_or_else(|| {
                    ConversationStoreError::InvalidReference(format!(
                        "Original {id} is absent at the Resource Read Cut"
                    ))
                })?;
            let json = row.1?;
            record_body_decode();
            let message: MessageBlock = decode(&json, "compaction Original")?;
            if message.id() != id {
                return Err(ConversationStoreError::InvalidReference(
                    "compaction Original identity mismatch".into(),
                ));
            }
            page.rows
                .push((nonnegative(row.0, "Ledger position")?, message));
        }
    } else {
        let mut query = connection.prepare_cached("SELECT position,CASE WHEN octet_length(message_json)<=?3 THEN message_json END,octet_length(message_json),message_id FROM message_ledger WHERE position>?1 AND position<=?2 ORDER BY position LIMIT ?4")
            .map_err(|e| storage(e.to_string()))?;
        let mut rows = query
            .query(params![
                seq_to_i64(after.0)?,
                seq_to_i64(cut.ledger.0)?,
                limits.record_bytes,
                limits.page_items
            ])
            .map_err(|e| storage(e.to_string()))?;
        while let Some(row) = rows.next().map_err(|e| storage(e.to_string()))? {
            let bytes: usize = row.get(2).map_err(|e| storage(e.to_string()))?;
            if bytes > limits.record_bytes {
                return Err(bound("compaction source record byte bound exceeded"));
            }
            if !page.rows.is_empty() && bytes > limits.page_bytes.saturating_sub(page.bytes) {
                break;
            }
            record_body_decode();
            let message: MessageBlock = decode(
                &body(row, &mut page.bytes, limits)?,
                "compaction Ledger page",
            )?;
            let id: String = row.get(3).map_err(|e| storage(e.to_string()))?;
            if message.id().as_str() != id {
                return Err(ConversationStoreError::InvalidReference(
                    "compaction Ledger row identity mismatch".into(),
                ));
            }
            let position: i64 = row.get(0).map_err(|e| storage(e.to_string()))?;
            page.rows
                .push((nonnegative(position, "Ledger position")?, message));
        }
    }
    Ok(page)
}

pub fn events(
    connection: &Connection,
    conversation: &ConversationId,
    cut: &ResourceReadCut,
    after: JournalFrontier,
    limits: ResourceLimits,
) -> Result<ResourcePage<RuntimeEventEnvelope>, ConversationStoreError> {
    validate(connection, conversation, cut, limits)?;
    let mut query = connection.prepare_cached("SELECT sequence,CASE WHEN octet_length(event_json)<=?3 THEN event_json END,octet_length(event_json),event_id,schema_version,conversation_id,attempt_id,turn_id FROM events WHERE sequence>?1 AND sequence<=?2 ORDER BY sequence LIMIT ?4")
        .map_err(|e| storage(e.to_string()))?;
    let mut rows = query
        .query(params![
            seq_to_i64(after.0)?,
            seq_to_i64(cut.journal.0)?,
            limits.record_bytes,
            limits.page_items
        ])
        .map_err(|e| storage(e.to_string()))?;
    let mut page = ResourcePage {
        rows: Vec::new(),
        bytes: 0,
    };
    while let Some(row) = rows.next().map_err(|e| storage(e.to_string()))? {
        let bytes: usize = row.get(2).map_err(|e| storage(e.to_string()))?;
        if bytes > limits.record_bytes {
            return Err(bound("compaction Journal record byte bound exceeded"));
        }
        if !page.rows.is_empty() && bytes > limits.page_bytes.saturating_sub(page.bytes) {
            break;
        }
        let event: RuntimeEventEnvelope = decode(
            &body(row, &mut page.bytes, limits)?,
            "compaction Journal page",
        )?;
        let sequence: i64 = row.get(0).map_err(|e| storage(e.to_string()))?;
        let position = nonnegative(sequence, "Journal position")?;
        let id: String = row.get(3).map_err(|e| storage(e.to_string()))?;
        let schema: u16 = row.get(4).map_err(|e| storage(e.to_string()))?;
        let scope: String = row.get(5).map_err(|e| storage(e.to_string()))?;
        let attempt: Option<String> = row.get(6).map_err(|e| storage(e.to_string()))?;
        let turn: Option<String> = row.get(7).map_err(|e| storage(e.to_string()))?;
        if event.sequence != position
            || event.event_id.as_str() != id
            || event.schema_version != schema
            || schema != EVENT_SCHEMA_VERSION
            || event.conversation_id != *conversation
            || scope != conversation.as_str()
            || event.attempt_id.as_ref().map(ToString::to_string) != attempt
            || event.turn_id.as_ref().map(ToString::to_string) != turn
        {
            return Err(ConversationStoreError::InvalidReference(
                "compaction Journal row metadata mismatch".into(),
            ));
        }
        page.rows.push((position, event));
    }
    Ok(page)
}

/// Native message metadata and lifecycle facts selected under the same cut.
/// Local ownership comes from committed message facts, never transcript adjacency.
#[allow(clippy::too_many_lines)] // One detached typed page under a single short native read cut.
pub fn trajectory(
    connection: &Connection,
    conversation: &ConversationId,
    cut: &ResourceReadCut,
    after: LedgerFrontier,
    limits: ResourceLimits,
) -> Result<ResourcePage<crate::context_resources::TrajectoryMessage>, ConversationStoreError> {
    use crate::context_resources::{TrajectoryMessage, TrajectoryTool};
    use rustx_types::events::types::RuntimeEvent;
    use rustx_types::message::types::AssistantContentBlock;
    let source = messages(connection, conversation, cut, after, None, limits)?;
    let mut bytes = source.bytes;
    let mut result = Vec::new();
    for (position, message) in source.rows {
        let transcript: Option<u64> = connection.query_row("SELECT position FROM transcript_entries WHERE native_reference_kind='message' AND native_reference_id=?1 AND position<=?2", params![message.id().as_str(), cut.transcript.get()], |r|r.get(0)).optional().map_err(|e|storage(e.to_string()))?;
        let mut query = connection.prepare_cached("SELECT sequence,CASE WHEN octet_length(event_json)<=?4 THEN event_json END,octet_length(event_json) FROM events WHERE event_message_id=?1 AND event_kind=?2 AND sequence<=?3 ORDER BY sequence DESC LIMIT 1").map_err(|e|storage(e.to_string()))?;
        let kind = match &message {
            MessageBlock::Assistant(_) => "assistant_message_committed",
            MessageBlock::Tool(_) => "tool_message_committed",
            MessageBlock::User(_) => "context_contribution_emitted",
        };
        let committed: Option<RuntimeEventEnvelope> = query
            .query_row(
                params![
                    message.id().as_str(),
                    kind,
                    cut.journal.0,
                    limits.record_bytes
                ],
                |row| Ok(body(row, &mut bytes, limits)),
            )
            .optional()
            .map_err(|e| storage(e.to_string()))?
            .map(|json| decode(&json?, "trajectory commit"))
            .transpose()?;
        let mut tools = Vec::new();
        if let MessageBlock::Assistant(assistant) = &message {
            for (index, block) in assistant.content.iter().enumerate() {
                let AssistantContentBlock::ToolCall(call) = block else {
                    continue;
                };
                let occurrence = rustx_types::message::types::ToolCallOccurrenceRef::new(
                    assistant.id.clone(),
                    rustx_types::message::types::ContentBlockIndex::new(
                        u32::try_from(index).map_err(|_| {
                            bound("trajectory block count exceeds coordinate budget")
                        })?,
                    ),
                );
                let mut facts = Vec::new();
                if let Some(commit) = &committed
                    && let (Some(attempt), Some(turn)) = (&commit.attempt_id, &commit.turn_id)
                {
                    let mut query = connection.prepare_cached("SELECT sequence,CASE WHEN octet_length(event_json)<=?5 THEN event_json END,octet_length(event_json) FROM events WHERE event_tool_call_id=?1 AND attempt_id=?2 AND turn_id=?3 AND sequence<=?4 AND event_kind IN ('tool_execution_started','tool_execution_completed','tool_execution_failed') ORDER BY sequence LIMIT 3").map_err(|e|storage(e.to_string()))?;
                    let mut rows = query
                        .query(params![
                            call.id.as_str(),
                            attempt.as_str(),
                            turn.as_str(),
                            cut.journal.0,
                            limits.record_bytes
                        ])
                        .map_err(|e| storage(e.to_string()))?;
                    while let Some(row) = rows.next().map_err(|e| storage(e.to_string()))? {
                        facts.push(decode::<RuntimeEventEnvelope>(
                            &body(row, &mut bytes, limits)?,
                            "trajectory lifecycle",
                        )?);
                    }
                    if facts
                        .iter()
                        .filter(|e| matches!(e.event, RuntimeEvent::ToolExecutionStarted { .. }))
                        .count()
                        > 1
                        || facts
                            .iter()
                            .filter(|e| {
                                matches!(
                                    e.event,
                                    RuntimeEvent::ToolExecutionCompleted { .. }
                                        | RuntimeEvent::ToolExecutionFailed { .. }
                                )
                            })
                            .count()
                            > 1
                    {
                        return Err(ConversationStoreError::InvalidReference(
                            "ambiguous physical tool lifecycle within committed Step".into(),
                        ));
                    }
                }
                tools.push(TrajectoryTool { occurrence, facts });
            }
        }
        result.push((
            position,
            TrajectoryMessage {
                message,
                transcript_position: transcript.map(TranscriptCursor::new),
                committed,
                tools,
            },
        ));
    }
    let page = ResourcePage {
        rows: result,
        bytes,
    };
    let encoded = crate::context_resources::bounded_encoded_len(&page.rows, limits.page_bytes)?;
    Ok(ResourcePage {
        rows: page.rows,
        bytes: encoded,
    })
}

/// A keyed scalar integrity read hashes SQLite's borrowed bytes before returning
/// a small detached header. It never decodes or allocates the retired body.
pub fn dependency(
    connection: &Connection,
    scope: &ConversationId,
    cut: &ResourceReadCut,
    source: &rustx_types::model::projected::CanonicalMessageRef,
    body_required: bool,
    limits: ResourceLimits,
) -> Result<rustx_types::context::checkpoint::CanonicalDependency, ConversationStoreError> {
    validate(connection, scope, cut, limits)?;
    if source.conversation_id != *scope {
        return Err(ConversationStoreError::InvalidReference(
            "foreign provenance source".into(),
        ));
    }
    if body_required {
        let page = messages(
            connection,
            scope,
            cut,
            LedgerFrontier(0),
            Some(std::slice::from_ref(&source.message_id)),
            limits,
        )?;
        let (position, body) = page
            .rows
            .into_iter()
            .next()
            .ok_or_else(|| bound("missing immutable body"))?;
        return Ok(rustx_types::context::checkpoint::CanonicalDependency {
            frontier: LedgerFrontier(position),
            reference: rustx_types::model::projected::CanonicalMessageRef::of(scope, &body),
            body: Some(body),
        });
    }
    let row:Option<(u64,Option<String>)> = connection.query_row("SELECT position,CASE WHEN octet_length(message_json)<=?3 THEN rustx_content_digest(message_json) END FROM message_ledger WHERE message_id=?1 AND position<=?2",params![source.message_id.as_str(),cut.ledger.0,limits.record_bytes],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(|e|storage(e.to_string()))?;
    let (position, digest) = row.ok_or_else(|| {
        ConversationStoreError::InvalidReference("missing immutable provenance fact".into())
    })?;
    let digest = digest.ok_or_else(|| bound("oversized provenance fact"))?;
    Ok(rustx_types::context::checkpoint::CanonicalDependency {
        frontier: LedgerFrontier(position),
        reference: rustx_types::model::projected::CanonicalMessageRef {
            content_digest: digest,
            ..source.clone()
        },
        body: None,
    })
}
