//! Transaction-owned byte indexes. Each row holds at most 64 KiB; a range
//! read touches at most two rows, never a message/audit JSON body or frame scan.
//! Binary artifacts instead authenticate at most two complete 64 KiB file chunks.
use super::content_metadata::{
    AssistantMetadata, AuditMetadata, MessageMetadata, PublicationMetadata,
};
use super::{
    AssistantContentBlock, Connection, ConversationId, ConversationStoreError, MessageBlock,
    OptionalExtension, PublicationAudit, PublicationFrame, PublicationPayload, PublicationStreamId,
    Serialize, params, storage,
};
use crate::observation::content::{
    CONTENT_READ_MAX_BYTES, CONTENT_SAFE_INTEGER_MAX, CanonicalContentComponent,
    CanonicalContentLocator, ContentChunk, ContentRef, ContentRepresentation as Rep,
    ContentSource as Source,
};
use base64::Engine;
const SEGMENT: u64 = 65_536;
const SEGMENT_BYTES: usize = 65_536;
#[path = "content_binary.rs"]
mod binary;
pub(super) use binary::{
    create_schema as create_binary_schema, verify_schema as verify_binary_schema,
};
#[cfg(test)]
#[path = "content_binary_tests.rs"]
mod binary_tests;
#[cfg(test)]
thread_local! { static LAST_READ_WORK: std::cell::Cell<(usize,usize)> = const { std::cell::Cell::new((0,0)) }; }
#[cfg(test)]
pub(super) fn last_read_work() -> (usize, usize) {
    LAST_READ_WORK.with(std::cell::Cell::get)
}

fn invalid() -> ConversationStoreError {
    ConversationStoreError::InvalidReference("content source or read evidence unavailable".into())
}

pub(super) fn validate_canonical_locator(
    locator: &CanonicalContentLocator,
) -> Result<(), ConversationStoreError> {
    match locator {
        CanonicalContentLocator::ByMessageId { message_id } if message_id.as_str().len() <= 256 => {
            Ok(())
        }
        CanonicalContentLocator::ByLedgerPosition { position }
            if *position > 0 && *position <= CONTENT_SAFE_INTEGER_MAX =>
        {
            Ok(())
        }
        _ => Err(invalid()),
    }
}

pub(super) fn canonical_position(
    connection: &Connection,
    locator: &CanonicalContentLocator,
) -> Result<u64, ConversationStoreError> {
    let position = match locator {
        CanonicalContentLocator::ByMessageId { message_id } => connection
            .query_row(
                "SELECT position FROM message_ledger WHERE message_id=?1",
                [message_id.as_str()],
                |row| row.get::<_, u64>(0),
            )
            .optional()
            .map_err(|error| storage(error.to_string()))?
            .ok_or_else(invalid)?,
        CanonicalContentLocator::ByLedgerPosition { position } => {
            if *position == 0 || *position > CONTENT_SAFE_INTEGER_MAX {
                return Err(invalid());
            }
            let exists: bool = connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM message_ledger WHERE position=?1)",
                    [*position],
                    |row| row.get(0),
                )
                .map_err(|error| storage(error.to_string()))?;
            if !exists {
                return Err(invalid());
            }
            *position
        }
    };
    Ok(position)
}
fn canonical_key(connection: &Connection, id: &str) -> Result<String, ConversationStoreError> {
    let position: u64 = connection
        .query_row(
            "SELECT position FROM message_ledger WHERE message_id=?1",
            [id],
            |row| row.get(0),
        )
        .map_err(|error| storage(error.to_string()))?;
    Ok(format!("ledger:{position}"))
}
fn storage_key<'a>(
    connection: &Connection,
    source: &'a Source,
) -> Result<(&'static str, std::borrow::Cow<'a, str>, u32), ConversationStoreError> {
    if let Source::CanonicalMessageBlock {
        component,
        locator,
        block_index,
    } = source
    {
        return Ok((
            component.key(),
            std::borrow::Cow::Owned(format!(
                "ledger:{}",
                canonical_position(connection, locator)?
            )),
            block_index.get(),
        ));
    }
    Ok(source.key())
}
fn normalize_source(
    connection: &Connection,
    source: &Source,
) -> Result<Source, ConversationStoreError> {
    if let Source::CanonicalMessageBlock {
        component,
        locator: CanonicalContentLocator::ByMessageId { message_id },
        block_index,
    } = source
        && message_id.as_str().len() > 256
    {
        let position = connection
            .query_row(
                "SELECT position FROM message_ledger WHERE message_id=?1",
                [message_id.as_str()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| storage(error.to_string()))?
            .ok_or_else(invalid)?;
        return Ok(Source::CanonicalMessageBlock {
            component: *component,
            locator: CanonicalContentLocator::ByLedgerPosition { position },
            block_index: *block_index,
        });
    }
    Ok(source.clone())
}

fn validate_source(connection: &Connection, source: &Source) -> Result<(), ConversationStoreError> {
    match source {
        Source::CanonicalMessageBlock {
            locator,
            block_index,
            component,
        } => {
            validate_canonical_locator(locator)?;
            if *component == CanonicalContentComponent::MessageMetadata && block_index.get() != 0 {
                return Err(invalid());
            }
            canonical_position(connection, locator)?;
        }
        Source::LivePublicationPrefix {
            stream_id,
            message_id,
            attempt_id,
            turn_id,
            publication_request_id,
            ..
        }
        | Source::DurableTranscriptAudit {
            stream_id,
            message_id,
            attempt_id,
            turn_id,
            publication_request_id,
            ..
        } => {
            if [
                stream_id.as_str(),
                message_id.as_str(),
                attempt_id.as_str(),
                turn_id.as_str(),
                publication_request_id.as_str(),
            ]
            .iter()
            .any(|id| id.len() > 256)
            {
                return Err(invalid());
            }
            // Prove the full frozen owner tuple in SQL. A range read must not
            // materialize an arbitrarily large stored identity or source body.
            let settlement:Option<i64>=connection.query_row("SELECT CASE WHEN settlement IS NULL THEN 0 WHEN settlement='canonical' THEN 1 WHEN settlement='incomplete' THEN 2 WHEN settlement='unaccepted' THEN 3 ELSE -1 END FROM publication_streams WHERE stream_id=?1 AND message_id=?2 AND attempt_id=?3 AND turn_id=?4 AND request_id=?5",params![stream_id.as_str(),message_id.as_str(),attempt_id.as_str(),turn_id.as_str(),publication_request_id.as_str()],|row|row.get(0)).optional().map_err(|error|storage(error.to_string()))?;
            let Some(settlement) = settlement else {
                return Err(invalid());
            };
            if matches!(source, Source::DurableTranscriptAudit { .. }) {
                let retained: bool = connection
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM publication_audits WHERE stream_id=?1)",
                        [stream_id.as_str()],
                        |row| row.get(0),
                    )
                    .map_err(|error| storage(error.to_string()))?;
                if !retained {
                    return Err(invalid());
                }
            }
            match source {
                Source::LivePublicationPrefix { .. } if settlement != 0 => return Err(invalid()),
                Source::DurableTranscriptAudit { .. } if !matches!(settlement, 2 | 3) => {
                    return Err(invalid());
                }
                _ => {}
            }
        }
    }
    Ok(())
}
pub(super) fn descriptor(
    connection: &Connection,
    conversation: &ConversationId,
    source: &Source,
    representation: Rep,
) -> Result<Option<ContentRef>, ConversationStoreError> {
    let source = normalize_source(connection, source)?;
    validate_source(connection, &source)?;
    let (kind, id, block) = storage_key(connection, &source)?;
    let evidence:Option<(u64,u64)>=connection.query_row("SELECT revision,readable_end FROM content_frontiers WHERE source_kind=?1 AND source_id=?2 AND block_index=?3 AND representation=?4 ORDER BY revision DESC LIMIT 1",params![kind,id,block,representation.key()],|row|Ok((row.get(0)?,row.get(1)?))).optional().map_err(|e|storage(e.to_string()))?;
    if evidence.is_none() && representation == Rep::RawJsonUtf8 {
        let retained: bool = connection
            .prepare_cached("SELECT EXISTS(SELECT 1 FROM content_segments WHERE source_kind=?1 AND source_id=?2 AND block_index=?3 AND representation=?4)")
            .map_err(|error| storage(error.to_string()))?
            .query_row(params![kind, id, block, representation.key()], |row| row.get(0))
            .map_err(|error| storage(error.to_string()))?;
        if retained {
            return Err(storage(
                "retained original raw arguments frontier unavailable",
            ));
        }
    }
    Ok(evidence.map(|(content_revision, readable_end)| ContentRef {
        conversation_id: conversation.clone(),
        source: source.clone(),
        representation,
        version: 1,
        content_revision,
        readable_end,
    }))
}
pub(super) fn descriptor_at(
    connection: &Connection,
    conversation: &ConversationId,
    source: &Source,
    representation: Rep,
    revision: u64,
) -> Result<Option<ContentRef>, ConversationStoreError> {
    let source = normalize_source(connection, source)?;
    validate_source(connection, &source)?;
    if revision > CONTENT_SAFE_INTEGER_MAX {
        return Err(invalid());
    }
    let (kind, id, block) = storage_key(connection, &source)?;
    let evidence:Option<u64>=connection.query_row("SELECT readable_end FROM content_frontiers WHERE source_kind=?1 AND source_id=?2 AND block_index=?3 AND representation=?4 AND revision=?5",params![kind,id,block,representation.key(),revision],|row|row.get(0)).optional().map_err(|error|storage(error.to_string()))?;
    Ok(evidence.map(|readable_end| ContentRef {
        conversation_id: conversation.clone(),
        source: source.clone(),
        representation,
        version: 1,
        content_revision: revision,
        readable_end,
    }))
}
pub(super) fn read(
    connection: &Connection,
    conversation: &ConversationId,
    reference: &ContentRef,
    offset: u64,
    max_bytes: u32,
) -> Result<ContentChunk, ConversationStoreError> {
    #[cfg(test)]
    LAST_READ_WORK.with(|work| work.set((0, 0)));
    if reference.conversation_id != *conversation
        || reference.version != 1
        || reference.readable_end > CONTENT_SAFE_INTEGER_MAX
        || reference.content_revision > CONTENT_SAFE_INTEGER_MAX
    {
        return Err(invalid());
    }
    if offset > CONTENT_SAFE_INTEGER_MAX || max_bytes == 0 || max_bytes > CONTENT_READ_MAX_BYTES {
        return Err(ConversationStoreError::InvalidContentRange);
    }
    validate_source(connection, &reference.source)?;
    let (kind, id, block) = storage_key(connection, &reference.source)?;
    let frontier:Option<u64>=connection.query_row("SELECT readable_end FROM content_frontiers WHERE source_kind=?1 AND source_id=?2 AND block_index=?3 AND representation=?4 AND revision=?5",params![kind,id,block,reference.representation.key(),reference.content_revision],|row|row.get(0)).optional().map_err(|e|storage(e.to_string()))?;
    if frontier != Some(reference.readable_end) {
        return Err(invalid());
    }
    if offset > reference.readable_end {
        return Err(ConversationStoreError::InvalidContentRange);
    }
    let end = offset
        .checked_add(u64::from(max_bytes))
        .ok_or_else(invalid)?
        .min(reference.readable_end);
    let bytes = if kind == "live_block_metadata" {
        metadata_range(
            connection,
            id.as_ref(),
            block,
            reference.content_revision,
            offset,
            end,
        )?
    } else if reference.representation == Rep::BinaryBytes {
        if kind != "canonical" {
            return Err(invalid());
        }
        binary::range(
            connection,
            id.as_ref(),
            block,
            offset,
            end,
            reference.readable_end,
        )?
    } else {
        range_bytes(
            connection,
            (kind, id.as_ref(), block, reference.representation),
            offset,
            end,
        )?
    };
    Ok(ContentChunk {
        content_ref: reference.clone(),
        offset,
        data_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
        end_offset: end,
        readable_end: reference.readable_end,
        next_offset: (end < reference.readable_end).then_some(end),
        complete: end == reference.readable_end,
    })
}
pub(super) fn range_bytes(
    connection: &Connection,
    key: (&str, &str, u32, Rep),
    offset: u64,
    end: u64,
) -> Result<Vec<u8>, ConversationStoreError> {
    let (kind, id, block, representation) = key;
    range_bytes_query(
        connection,
        "SELECT segment,substr(bytes,MAX(0,?7-segment*65536)+1,MIN(65536,?8-segment*65536)-MAX(0,?7-segment*65536)) FROM content_segments WHERE source_kind=?1 AND source_id=?2 AND block_index=?3 AND representation=?4 AND segment>=?5 AND segment<=?6 ORDER BY segment LIMIT 2",
        &[
            &kind,
            &id,
            &block,
            &representation.key(),
            &(offset / SEGMENT),
            &((end.saturating_sub(1)) / SEGMENT),
            &offset,
            &end,
        ],
        offset,
        end,
    )
}
pub(super) fn pending_range_bytes(
    connection: &Connection,
    sequence: u64,
    offset: u64,
    end: u64,
) -> Result<Vec<u8>, ConversationStoreError> {
    range_bytes_query(
        connection,
        "SELECT segment,substr(bytes,MAX(0,?4-segment*65536)+1,MIN(65536,?5-segment*65536)-MAX(0,?4-segment*65536)) FROM content_segments WHERE source_kind='pending' AND source_id=CAST(?1 AS TEXT) AND block_index=0 AND representation='canonical_json_v1' AND segment>=?2 AND segment<=?3 ORDER BY segment LIMIT 2",
        &[
            &sequence,
            &(offset / SEGMENT),
            &((end.saturating_sub(1)) / SEGMENT),
            &offset,
            &end,
        ],
        offset,
        end,
    )
}
fn range_bytes_query(
    connection: &Connection,
    sql: &str,
    bindings: &[&dyn rusqlite::ToSql],
    offset: u64,
    end: u64,
) -> Result<Vec<u8>, ConversationStoreError> {
    #[cfg(test)]
    LAST_READ_WORK.with(|work| work.set((0, 0)));
    let mut bytes = Vec::with_capacity(usize::try_from(end - offset).map_err(|_| invalid())?);
    if end > offset {
        // The primary-key range visits at most two finite BLOBs. SQL returns
        // only their selected slices, whose aggregate is one bounded window.
        let mut stmt = connection
            .prepare(sql)
            .map_err(|e| storage(e.to_string()))?;
        let rows = stmt
            .query_map(bindings, |row| {
                Ok((row.get::<_, u64>(0)?, row.get::<_, Vec<u8>>(1)?))
            })
            .map_err(|e| storage(e.to_string()))?;
        for row in rows {
            let (segment, data) = row.map_err(|e| storage(e.to_string()))?;
            #[cfg(test)]
            LAST_READ_WORK.with(|work| {
                let (rows, bytes) = work.get();
                work.set((rows + 1, bytes + data.len()));
            });
            let start = segment.checked_mul(SEGMENT).ok_or_else(invalid)?;
            let expected =
                end.min(start.checked_add(SEGMENT).ok_or_else(invalid)?) - offset.max(start);
            if data.len() as u64 != expected {
                return Err(invalid());
            }
            bytes.extend_from_slice(&data);
        }
        if bytes.len() as u64 != end - offset {
            return Err(invalid());
        }
    }
    Ok(bytes)
}
pub(super) fn index_pending(
    connection: &Connection,
    message: &crate::message::types::UserMessageBlock,
    sequence: u64,
    revision: u64,
) -> Result<(), ConversationStoreError> {
    let key = sequence.to_string();
    clear_pending(connection, &key)?;
    // Pending detail is a typed native domain read, independent of the
    // CanonicalJsonV1 ContentRef source representation. Keep owned u64 fields
    // as decimal strings while arbitrary payload JSON retains its exact shape.
    super::content_encoding::put_native(
        connection,
        ("pending", key.as_str(), 0, Rep::CanonicalJsonV1),
        revision.checked_add(1).ok_or_else(invalid)?,
        message,
    )
    .map(|_| ())
}
pub(super) fn clear_pending(
    connection: &Connection,
    id: &str,
) -> Result<(), ConversationStoreError> {
    for table in ["content_segments", "content_frontiers"] {
        connection
            .execute(
                &format!("DELETE FROM {table} WHERE source_kind='pending' AND source_id=?1"),
                [id],
            )
            .map_err(|error| storage(error.to_string()))?;
    }
    Ok(())
}
fn put(
    connection: &Connection,
    key: (&str, &str, u32, Rep),
    revision: u64,
    bytes: &[u8],
    append: bool,
) -> Result<(), ConversationStoreError> {
    let (kind, id, block, representation) = key;
    if revision > CONTENT_SAFE_INTEGER_MAX {
        return Err(invalid());
    }
    let prior: u64 = if append {
        connection.query_row("SELECT COALESCE(MAX(readable_end),0) FROM content_frontiers WHERE source_kind=?1 AND source_id=?2 AND block_index=?3 AND representation=?4",params![kind,id,block,representation.key()],|r|r.get(0)).map_err(|e|storage(e.to_string()))?
    } else {
        0
    };
    let end = prior
        .checked_add(bytes.len() as u64)
        .filter(|v| *v <= CONTENT_SAFE_INTEGER_MAX)
        .ok_or_else(invalid)?;
    let mut consumed = 0usize;
    if !prior.is_multiple_of(SEGMENT) && !bytes.is_empty() {
        let segment = prior / SEGMENT;
        let mut tail:Vec<u8>=connection.query_row("SELECT bytes FROM content_segments WHERE source_kind=?1 AND source_id=?2 AND block_index=?3 AND representation=?4 AND segment=?5",params![kind,id,block,representation.key(),segment],|r|r.get(0)).map_err(|e|storage(e.to_string()))?;
        consumed = bytes.len().min(SEGMENT_BYTES - tail.len());
        tail.extend_from_slice(&bytes[..consumed]);
        connection.execute("UPDATE content_segments SET bytes=?6 WHERE source_kind=?1 AND source_id=?2 AND block_index=?3 AND representation=?4 AND segment=?5",params![kind,id,block,representation.key(),segment,tail]).map_err(|e|storage(e.to_string()))?;
    }
    while consumed < bytes.len() {
        let start = consumed;
        consumed = consumed.saturating_add(SEGMENT_BYTES).min(bytes.len());
        connection
            .execute(
                "INSERT INTO content_segments VALUES(?1,?2,?3,?4,?5,?6)",
                params![
                    kind,
                    id,
                    block,
                    representation.key(),
                    (prior + start as u64) / SEGMENT,
                    &bytes[start..consumed]
                ],
            )
            .map_err(|e| storage(e.to_string()))?;
    }
    connection
        .execute(
            "INSERT INTO content_frontiers VALUES(?1,?2,?3,?4,?5,?6)",
            params![kind, id, block, representation.key(), revision, end],
        )
        .map_err(|e| storage(e.to_string()))?;
    Ok(())
}

fn index_metadata(
    connection: &Connection,
    id: &str,
    index: usize,
    metadata: &impl Serialize,
) -> Result<(), ConversationStoreError> {
    super::content_encoding::put_json(
        connection,
        (
            "canonical_block_metadata",
            id,
            u32::try_from(index).map_err(|_| invalid())?,
            Rep::CanonicalJsonV1,
        ),
        1,
        metadata,
        false,
    )
    .map(|_| ())
}
fn index_user_blocks(
    connection: &Connection,
    user: &crate::message::types::UserMessageBlock,
) -> Result<(), ConversationStoreError> {
    let key = canonical_key(connection, user.id.as_str())?;
    let id = key.as_str();

    for (index, block) in user.content.iter().enumerate() {
        let kind = match block {
            crate::message::types::UserContentBlock::Text(_) => "text",
            crate::message::types::UserContentBlock::Image(_) => "image",
            crate::message::types::UserContentBlock::File(_) => "file",
            crate::message::types::UserContentBlock::UploadedFile(_) => "uploaded_file",
        };
        index_metadata(connection, id, index, &serde_json::json!({"type":kind}))?;
        match block {
            crate::message::types::UserContentBlock::Image(image) => binary::index(
                connection,
                id,
                u32::try_from(index).map_err(|_| invalid())?,
                &image.artifact_id,
            )?,
            crate::message::types::UserContentBlock::File(file) => binary::index(
                connection,
                id,
                u32::try_from(index).map_err(|_| invalid())?,
                &file.artifact_id,
            )?,
            _ => {}
        }
        let index = u32::try_from(index).map_err(|_| invalid())?;
        match block {
            crate::message::types::UserContentBlock::Text(text) => put(
                connection,
                ("canonical", id, index, Rep::Utf8Text),
                1,
                text.text.as_bytes(),
                false,
            )?,
            other => {
                super::content_encoding::put_json(
                    connection,
                    ("canonical", id, index, Rep::CanonicalJsonV1),
                    1,
                    other,
                    false,
                )?;
            }
        }
    }

    Ok(())
}
fn index_assistant_blocks(
    connection: &Connection,
    assistant: &crate::message::types::AssistantMessageBlock,
) -> Result<(), ConversationStoreError> {
    let key = canonical_key(connection, assistant.id.as_str())?;
    let id = key.as_str();

    for (index, block) in assistant.content.iter().enumerate() {
        index_metadata(connection, id, index, &AssistantMetadata(block))?;
        if let AssistantContentBlock::Image(image) = block {
            binary::index(
                connection,
                id,
                u32::try_from(index).map_err(|_| invalid())?,
                &image.artifact_id,
            )?;
        }
        let index = u32::try_from(index).map_err(|_| invalid())?;
        match block {
            AssistantContentBlock::Text(text) => put(
                connection,
                ("canonical", id, index, Rep::Utf8Text),
                1,
                text.text.as_bytes(),
                false,
            )?,
            AssistantContentBlock::Reasoning(reasoning) => put(
                connection,
                ("canonical", id, index, Rep::Utf8Text),
                1,
                reasoning.text.as_deref().unwrap_or_default().as_bytes(),
                false,
            )?,
            AssistantContentBlock::Refusal(refusal) => put(
                connection,
                ("canonical", id, index, Rep::Utf8Text),
                1,
                refusal.text.as_bytes(),
                false,
            )?,
            AssistantContentBlock::ToolCall(call) => {
                super::content_encoding::put_json(
                    connection,
                    ("canonical", id, index, Rep::CanonicalJsonV1),
                    1,
                    &call.arguments,
                    false,
                )?;
            }
            AssistantContentBlock::Image(_) => {
                super::content_encoding::put_json(
                    connection,
                    ("canonical", id, index, Rep::CanonicalJsonV1),
                    1,
                    block,
                    false,
                )?;
            }
        }
    }

    Ok(())
}
fn index_tool_blocks(
    connection: &Connection,
    tool: &crate::message::types::ToolMessageBlock,
) -> Result<(), ConversationStoreError> {
    let key = canonical_key(connection, tool.id.as_str())?;
    let id = key.as_str();

    for (index, block) in tool.result.content.iter().enumerate() {
        let kind = match block {
            crate::tools::types::ToolResultContent::Text(_) => "text",
            crate::tools::types::ToolResultContent::Json { .. } => "json",
            crate::tools::types::ToolResultContent::Image(_) => "image",
            crate::tools::types::ToolResultContent::File(_) => "file",
        };
        index_metadata(connection, id, index, &serde_json::json!({"type":kind}))?;
        match block {
            crate::tools::types::ToolResultContent::Image(image) => binary::index(
                connection,
                id,
                u32::try_from(index).map_err(|_| invalid())?,
                &image.artifact_id,
            )?,
            crate::tools::types::ToolResultContent::File(file) => binary::index(
                connection,
                id,
                u32::try_from(index).map_err(|_| invalid())?,
                &file.artifact_id,
            )?,
            _ => {}
        }
        let index = u32::try_from(index).map_err(|_| invalid())?;
        match block {
            crate::tools::types::ToolResultContent::Text(text) => put(
                connection,
                ("canonical", id, index, Rep::Utf8Text),
                1,
                text.text.as_bytes(),
                false,
            )?,
            crate::tools::types::ToolResultContent::Json { value } => {
                super::content_encoding::put_json(
                    connection,
                    ("canonical", id, index, Rep::CanonicalJsonV1),
                    1,
                    value,
                    false,
                )?;
            }
            other => {
                super::content_encoding::put_json(
                    connection,
                    ("canonical", id, index, Rep::CanonicalJsonV1),
                    1,
                    other,
                    false,
                )?;
            }
        }
    }

    Ok(())
}
pub(super) fn index_message(
    connection: &Connection,
    message: &MessageBlock,
) -> Result<(), ConversationStoreError> {
    let key = canonical_key(connection, message.id().as_str())?;
    let id = key.as_str();
    let position: u64 = connection
        .query_row(
            "SELECT position FROM message_ledger WHERE message_id=?1",
            [message.id().as_str()],
            |row| row.get(0),
        )
        .map_err(|error| storage(error.to_string()))?;
    super::observation_membership::canonical(connection, position, message.id())?;
    super::content_encoding::put_json(
        connection,
        ("canonical_message_metadata", id, 0, Rep::CanonicalJsonV1),
        1,
        &MessageMetadata(message),
        false,
    )?;
    match message {
        MessageBlock::User(user) => index_user_blocks(connection, user)?,
        MessageBlock::Assistant(assistant) => index_assistant_blocks(connection, assistant)?,
        MessageBlock::Tool(tool) => index_tool_blocks(connection, tool)?,
    }
    super::observation_state::message(connection, message)?;
    super::observation_delivery::index(connection, position, message)?;
    Ok(())
}
fn index_publication_metadata(
    connection: &Connection,
    frame: &PublicationFrame,
    block: u32,
    id: &crate::runtime::identity::ToolCallId,
    tool: &crate::runtime::identity::ToolId,
    name: &str,
) -> Result<(), ConversationStoreError> {
    let revision = frame
        .sequence
        .checked_add(1)
        .filter(|revision| *revision <= CONTENT_SAFE_INTEGER_MAX)
        .ok_or_else(invalid)?;
    super::content_encoding::put_json(
        connection,
        (
            "live_block_metadata",
            frame.stream_id.as_str(),
            block,
            Rep::CanonicalJsonV1,
        ),
        revision,
        &PublicationMetadata {
            id,
            tool_id: tool,
            name,
        }
        .tagged(),
        true,
    )
    .map(|_| ())
}

fn metadata_range(
    connection: &Connection,
    id: &str,
    block: u32,
    revision: u64,
    offset: u64,
    end: u64,
) -> Result<Vec<u8>, ConversationStoreError> {
    if end == offset {
        return Ok(Vec::new());
    }
    let mut statement=connection.prepare("SELECT segment,bytes FROM publication_metadata_segments WHERE stream_id=?1 AND block_index=?2 AND revision=?3 AND segment>=?4 AND segment<=?5 ORDER BY segment").map_err(|error|storage(error.to_string()))?;
    let rows = statement
        .query_map(
            params![id, block, revision, offset / SEGMENT, (end - 1) / SEGMENT],
            |row| Ok((row.get::<_, u64>(0)?, row.get::<_, Vec<u8>>(1)?)),
        )
        .map_err(|error| storage(error.to_string()))?;
    let mut bytes = Vec::new();
    for row in rows {
        let (segment, data) = row.map_err(|error| storage(error.to_string()))?;
        #[cfg(test)]
        LAST_READ_WORK.with(|work| {
            let (rows, bytes) = work.get();
            work.set((rows + 1, bytes + data.len()));
        });
        let start = segment.checked_mul(SEGMENT).ok_or_else(invalid)?;
        let from = usize::try_from(offset.saturating_sub(start)).map_err(|_| invalid())?;
        let to = usize::try_from((end - start).min(data.len() as u64)).map_err(|_| invalid())?;
        bytes.extend_from_slice(data.get(from..to).ok_or_else(invalid)?);
    }
    if bytes.len() as u64 != end - offset {
        return Err(invalid());
    }
    Ok(bytes)
}

pub(super) fn frame_receipt(
    connection: &Connection,
    conversation: &ConversationId,
    start: &crate::publication::PublicationStreamStart,
    frame: &PublicationFrame,
) -> Result<crate::publication::PublicationFrameReceipt, ConversationStoreError> {
    use crate::observation::content::PublicationContentComponent;
    let(block_total,tool_total):(u64,u64)=connection.query_row("SELECT total,tool_total FROM publication_observation_counts WHERE stream_id=?1 AND revision=?2",params![frame.stream_id.as_str(),frame.sequence.checked_add(1).ok_or_else(invalid)?],|row|Ok((row.get(0)?,row.get(1)?))).map_err(|error|storage(error.to_string()))?;
    let (block, representation, suffix_bytes, metadata) = match &frame.payload {
        PublicationPayload::TextSuffix {
            block_index,
            suffix,
        }
        | PublicationPayload::ReasoningSuffix {
            block_index,
            suffix,
        }
        | PublicationPayload::RefusalSuffix {
            block_index,
            suffix,
        } => (
            *block_index,
            Rep::Utf8Text,
            Some(suffix.len() as u64),
            false,
        ),
        PublicationPayload::ProposedToolCallArgumentsSuffix {
            block_index,
            suffix,
            ..
        } => (
            *block_index,
            Rep::RawJsonUtf8,
            Some(suffix.len() as u64),
            false,
        ),
        PublicationPayload::ProposedToolCallStarted { block_index, .. } => {
            (*block_index, Rep::RawJsonUtf8, None, true)
        }
        PublicationPayload::ProposedToolCallCompleted { block_index, .. } => {
            (*block_index, Rep::CanonicalJsonV1, None, true)
        }
        PublicationPayload::TerminalOnly => {
            return Ok(crate::publication::PublicationFrameReceipt {
                unavailable: false,
                stream_id: frame.stream_id.clone(),
                sequence: frame.sequence,
                content: Vec::new(),
                suffix_bytes: None,
                block_ordinal: None,
                block_total,
                tool_total,
                tool_ordinal: None,
            });
        }
    };
    let revision = frame.sequence.checked_add(1).ok_or_else(invalid)?;
    let source = |component| Source::LivePublicationPrefix {
        component,
        stream_id: frame.stream_id.clone(),
        message_id: frame.message_id.clone(),
        attempt_id: start.attempt_id.clone(),
        turn_id: start.turn_id.clone(),
        publication_request_id: start.request_id.clone(),
        block_index: block,
    };
    let mut refs = Vec::with_capacity(2);
    if let Some(reference) = descriptor_at(
        connection,
        conversation,
        &source(PublicationContentComponent::Payload),
        representation,
        revision,
    )? {
        refs.push(reference);
    }
    if metadata {
        refs.push(
            descriptor_at(
                connection,
                conversation,
                &source(PublicationContentComponent::BlockMetadata),
                Rep::CanonicalJsonV1,
                revision,
            )?
            .ok_or_else(invalid)?,
        );
    }
    let(block_ordinal,tool_ordinal):(Option<u64>,Option<u64>)=connection.query_row("SELECT creation_ordinal,tool_ordinal FROM publication_observation_blocks WHERE stream_id=?1 AND block_index=?2",params![frame.stream_id.as_str(),block.get()],|row|Ok((row.get(0)?,row.get(1)?))).map_err(|error|storage(error.to_string()))?;
    Ok(crate::publication::PublicationFrameReceipt {
        unavailable: false,
        stream_id: frame.stream_id.clone(),
        sequence: frame.sequence,
        content: refs,
        suffix_bytes,
        block_ordinal,
        block_total,
        tool_ordinal,
        tool_total,
    })
}

fn index_live_block(
    connection: &Connection,
    frame: &PublicationFrame,
) -> Result<(), ConversationStoreError> {
    let revision = frame
        .sequence
        .checked_add(1)
        .filter(|revision| *revision <= CONTENT_SAFE_INTEGER_MAX)
        .ok_or_else(invalid)?;
    let block = match &frame.payload {
        PublicationPayload::TextSuffix { block_index, .. } => Some((block_index, "text")),
        PublicationPayload::ReasoningSuffix { block_index, .. } => Some((block_index, "reasoning")),
        PublicationPayload::RefusalSuffix { block_index, .. } => Some((block_index, "refusal")),
        PublicationPayload::ProposedToolCallStarted { block_index, .. }
        | PublicationPayload::ProposedToolCallArgumentsSuffix { block_index, .. }
        | PublicationPayload::ProposedToolCallCompleted { block_index, .. } => {
            Some((block_index, "tool_call"))
        }
        PublicationPayload::TerminalOnly => None,
    };
    let(prior,prior_tools):(u64,u64)=connection.query_row("SELECT COALESCE((SELECT total FROM publication_observation_counts WHERE stream_id=?1 ORDER BY revision DESC LIMIT 1),0),COALESCE((SELECT tool_total FROM publication_observation_counts WHERE stream_id=?1 ORDER BY revision DESC LIMIT 1),0)",[frame.stream_id.as_str()],|row|Ok((row.get(0)?,row.get(1)?))).map_err(|error|storage(error.to_string()))?;
    let tool = block.is_some_and(|(_, kind)| kind == "tool_call");
    let added = if let Some((block, kind)) = block {
        connection
            .execute(
                "INSERT OR IGNORE INTO publication_observation_blocks VALUES(?1,?2,?3,?4,?5,?6)",
                params![
                    frame.stream_id.as_str(),
                    block.get(),
                    revision,
                    prior,
                    tool.then_some(prior_tools),
                    kind
                ],
            )
            .map_err(|error| storage(error.to_string()))? as u64
    } else {
        0
    };
    connection
        .execute(
            "INSERT INTO publication_observation_counts VALUES(?1,?2,?3,?4)",
            params![
                frame.stream_id.as_str(),
                revision,
                prior.checked_add(added).ok_or_else(invalid)?,
                prior_tools
                    .checked_add(if tool { added } else { 0 })
                    .ok_or_else(invalid)?
            ],
        )
        .map_err(|error| storage(error.to_string()))?;
    Ok(())
}

pub(super) fn live_block_header_page(
    connection: &Connection,
    conversation: &ConversationId,
    source: &Source,
    through: u64,
    offset: u64,
    limit: usize,
) -> Result<crate::observation::content::LiveBlockHeaderPage, ConversationStoreError> {
    live_headers(
        connection,
        conversation,
        source,
        through,
        offset,
        limit,
        false,
    )
}
pub(super) fn live_tool_header_page(
    connection: &Connection,
    conversation: &ConversationId,
    source: &Source,
    through: u64,
    offset: u64,
    limit: usize,
) -> Result<crate::observation::content::LiveBlockHeaderPage, ConversationStoreError> {
    live_headers(
        connection,
        conversation,
        source,
        through,
        offset,
        limit,
        true,
    )
}
fn live_headers(
    connection: &Connection,
    conversation: &ConversationId,
    source: &Source,
    through: u64,
    offset: u64,
    limit: usize,
    tools: bool,
) -> Result<crate::observation::content::LiveBlockHeaderPage, ConversationStoreError> {
    use crate::observation::content::{
        LiveBlockHeader, LiveBlockHeaderPage, PublicationContentComponent,
    };
    if limit == 0
        || limit > 32
        || offset > u64::from(u32::MAX)
        || through > CONTENT_SAFE_INTEGER_MAX
    {
        return Err(invalid());
    }
    let Source::LivePublicationPrefix { stream_id, .. } = source else {
        return Err(invalid());
    };
    validate_source(connection, source)?;
    let scalar = if tools {
        "SELECT tool_total FROM publication_observation_counts WHERE stream_id=?1 AND revision<=?2 ORDER BY revision DESC LIMIT 1"
    } else {
        "SELECT total FROM publication_observation_counts WHERE stream_id=?1 AND revision<=?2 ORDER BY revision DESC LIMIT 1"
    };
    let total: u64 = connection
        .query_row(scalar, params![stream_id.as_str(), through], |row| {
            row.get(0)
        })
        .optional()
        .map_err(|error| storage(error.to_string()))?
        .unwrap_or(0);
    let sql = if tools {
        "SELECT block_index,kind,tool_ordinal FROM publication_observation_blocks WHERE stream_id=?1 AND tool_ordinal>=?2 AND tool_ordinal<?3 ORDER BY tool_ordinal LIMIT ?4"
    } else {
        "SELECT block_index,kind,creation_ordinal FROM publication_observation_blocks WHERE stream_id=?1 AND creation_ordinal>=?2 AND creation_ordinal<?3 ORDER BY creation_ordinal LIMIT ?4"
    };
    let mut statement = connection
        .prepare(sql)
        .map_err(|error| storage(error.to_string()))?;
    let rows = statement
        .query_map(
            params![stream_id.as_str(), offset, total, limit + 1],
            |row| {
                Ok((
                    row.get::<_, u32>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, u64>(2)?,
                ))
            },
        )
        .map_err(|error| storage(error.to_string()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| storage(error.to_string()))?;
    let next_offset = rows.get(limit).map(|(_, _, position)| *position);
    let mut headers = Vec::new();
    for (index, kind, position) in rows.into_iter().take(limit) {
        let mut content = Vec::new();
        let mut assembled = false;
        for (component, rep) in live_representations() {
            let mut block_source = source.clone();
            let Source::LivePublicationPrefix {
                component: actual_component,
                block_index,
                ..
            } = &mut block_source
            else {
                unreachable!();
            };
            *actual_component = component;
            *block_index = crate::message::types::ContentBlockIndex::new(index);
            let (source_kind, id, block) = storage_key(connection, &block_source)?;
            let revision:Option<u64>=connection.query_row("SELECT revision FROM content_frontiers WHERE source_kind=?1 AND source_id=?2 AND block_index=?3 AND representation=?4 AND revision<=?5 ORDER BY revision DESC LIMIT 1",params![source_kind,id,block,rep.key(),through],|row|row.get(0)).optional().map_err(|error|storage(error.to_string()))?;
            if let Some(revision) = revision {
                if component == PublicationContentComponent::Payload && rep == Rep::CanonicalJsonV1
                {
                    assembled = true;
                }
                if let Some(reference) =
                    descriptor_at(connection, conversation, &block_source, rep, revision)?
                {
                    content.push(reference);
                }
            }
        }
        if content.len() > 3 {
            return Err(invalid());
        }
        headers.push(LiveBlockHeader {
            position,
            block_index: crate::message::types::ContentBlockIndex::new(index),
            kind,
            assembled,
            content,
        });
    }
    Ok(LiveBlockHeaderPage {
        total,
        headers,
        next_offset,
    })
}

pub(super) fn index_frame(
    connection: &Connection,
    frame: &PublicationFrame,
) -> Result<(), ConversationStoreError> {
    index_live_block(connection, frame)?;
    let (block, rep, bytes) = match &frame.payload {
        PublicationPayload::TextSuffix {
            block_index,
            suffix,
        }
        | PublicationPayload::ReasoningSuffix {
            block_index,
            suffix,
        }
        | PublicationPayload::RefusalSuffix {
            block_index,
            suffix,
        } => (*block_index, Rep::Utf8Text, suffix.as_bytes()),
        PublicationPayload::ProposedToolCallArgumentsSuffix {
            block_index,
            suffix,
            ..
        } => (*block_index, Rep::RawJsonUtf8, suffix.as_bytes()),
        PublicationPayload::ProposedToolCallStarted { block_index, call } => {
            index_publication_metadata(
                connection,
                frame,
                block_index.get(),
                &call.id,
                &call.tool_id,
                &call.name,
            )?;
            (*block_index, Rep::RawJsonUtf8, &[][..])
        }
        PublicationPayload::ProposedToolCallCompleted { block_index, call } => {
            index_publication_metadata(
                connection,
                frame,
                block_index.get(),
                &call.id,
                &call.tool_id,
                &call.name,
            )?;
            return super::content_encoding::put_json(
                connection,
                (
                    "live",
                    frame.stream_id.as_str(),
                    block_index.get(),
                    Rep::CanonicalJsonV1,
                ),
                frame.sequence.checked_add(1).ok_or_else(invalid)?,
                &call.arguments,
                false,
            )
            .map(|_| ());
        }
        PublicationPayload::TerminalOnly => return Ok(()),
    };
    put(
        connection,
        ("live", frame.stream_id.as_str(), block.get(), rep),
        frame.sequence.checked_add(1).ok_or_else(invalid)?,
        bytes,
        true,
    )
}
fn transfer(
    connection: &Connection,
    stream: &PublicationStreamId,
    source_kind: &str,
    kind: &str,
    id: &str,
) -> Result<(), ConversationStoreError> {
    connection.execute("INSERT INTO content_segments SELECT ?2,?3,block_index,representation,segment,bytes FROM content_segments WHERE source_kind=?4 AND source_id=?1",params![stream.as_str(),kind,id,source_kind]).map_err(|e|storage(e.to_string()))?;
    connection.execute("INSERT INTO content_frontiers SELECT ?2,?3,block_index,representation,1,MAX(readable_end) FROM content_frontiers WHERE source_kind=?4 AND source_id=?1 GROUP BY block_index,representation",params![stream.as_str(),kind,id,source_kind]).map_err(|e|storage(e.to_string()))?;
    Ok(())
}
fn clear_live(
    connection: &Connection,
    stream: &PublicationStreamId,
) -> Result<(), ConversationStoreError> {
    for table in [
        "publication_metadata_segments",
        "publication_observation_blocks",
        "publication_observation_counts",
    ] {
        connection
            .execute(
                &format!("DELETE FROM {table} WHERE stream_id=?1"),
                [stream.as_str()],
            )
            .map_err(|error| storage(error.to_string()))?;
    }
    for table in ["content_segments", "content_frontiers"] {
        connection
            .execute(
                &format!("DELETE FROM {table} WHERE source_kind IN ('live','live_block_metadata') AND source_id=?1"),
                [stream.as_str()],
            )
            .map_err(|e| storage(e.to_string()))?;
    }
    Ok(())
}
pub(super) fn settle_canonical(
    connection: &Connection,
    stream: &PublicationStreamId,
    message: &MessageBlock,
) -> Result<(), ConversationStoreError> {
    // Retain original Tool JSON independently from the canonical structural
    // Value representation. Never manufacture RawJsonUtf8 by reserializing.
    let key = canonical_key(connection, message.id().as_str())?;
    let failed: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM publication_frames INDEXED BY publication_frames_unindexed_idx WHERE stream_id=?1 AND observation_indexed=0)",
            [stream.as_str()],
            |row| row.get(0),
        )
        .unwrap_or(true);
    if failed {
        // The durable original P frames remain authority even if a display
        // layout failed. Preserve their exact raw JSON at canonical C before
        // releasing those frames; never copy a missing/partial live index.
        let mut statement = connection
            .prepare(
                "SELECT frame_json FROM publication_frames WHERE stream_id=?1 ORDER BY sequence",
            )
            .map_err(|error| storage(error.to_string()))?;
        let mut rows = statement
            .query([stream.as_str()])
            .map_err(|error| storage(error.to_string()))?;
        while let Some(row) = rows.next().map_err(|error| storage(error.to_string()))? {
            let json: String = row.get(0).map_err(|error| storage(error.to_string()))?;
            let frame: PublicationFrame =
                serde_json::from_str(&json).map_err(|error| storage(error.to_string()))?;
            if let PublicationPayload::ProposedToolCallArgumentsSuffix {
                block_index,
                suffix,
                ..
            } = frame.payload
            {
                put(
                    connection,
                    (
                        "canonical",
                        key.as_str(),
                        block_index.get(),
                        Rep::RawJsonUtf8,
                    ),
                    1,
                    suffix.as_bytes(),
                    true,
                )?;
            }
        }
    } else {
        connection.execute("INSERT INTO content_segments SELECT 'canonical',?2,block_index,representation,segment,bytes FROM content_segments WHERE source_kind='live' AND source_id=?1 AND representation='raw_json_utf8'",params![stream.as_str(),key.as_str()]).map_err(|e|storage(e.to_string()))?;
        connection.execute("INSERT INTO content_frontiers SELECT 'canonical',?2,block_index,representation,1,MAX(readable_end) FROM content_frontiers WHERE source_kind='live' AND source_id=?1 AND representation='raw_json_utf8' AND readable_end>0 GROUP BY block_index,representation",params![stream.as_str(),key.as_str()]).map_err(|e|storage(e.to_string()))?;
    }
    clear_live(connection, stream)
}
pub(super) fn settle_audit(
    connection: &Connection,
    audit: &PublicationAudit,
) -> Result<(), ConversationStoreError> {
    let failed: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM publication_frames INDEXED BY publication_frames_unindexed_idx WHERE stream_id=?1 AND observation_indexed=0)",
            [audit.stream_id.as_str()],
            |row| row.get(0),
        )
        .unwrap_or(true);
    if failed {
        for block in &audit.content {
            let (index, representation, bytes) = match block {
                crate::publication::PublicationAuditBlock::Text { block_index, text }
                | crate::publication::PublicationAuditBlock::Reasoning { block_index, text }
                | crate::publication::PublicationAuditBlock::Refusal { block_index, text } => {
                    (block_index.get(), Rep::Utf8Text, text.as_bytes())
                }
                crate::publication::PublicationAuditBlock::ProposedToolCall {
                    block_index,
                    arguments,
                    ..
                } => (block_index.get(), Rep::RawJsonUtf8, arguments.as_bytes()),
            };
            put(
                connection,
                ("audit", audit.stream_id.as_str(), index, representation),
                1,
                bytes,
                false,
            )?;
        }
    } else {
        transfer(
            connection,
            &audit.stream_id,
            "live",
            "audit",
            audit.stream_id.as_str(),
        )?;
    }
    for block in &audit.content {
        if let crate::publication::PublicationAuditBlock::ProposedToolCall {
            block_index,
            call_id,
            tool_id,
            name,
            complete,
            ..
        } = block
        {
            super::content_encoding::put_json(
                connection,
                (
                    "audit_block_metadata",
                    audit.stream_id.as_str(),
                    block_index.get(),
                    Rep::CanonicalJsonV1,
                ),
                1,
                &AuditMetadata {
                    block_index,
                    call_id,
                    tool_id,
                    name,
                    complete,
                }
                .tagged(),
                false,
            )?;
        }
    }
    clear_live(connection, &audit.stream_id)
}

/// The physical-work proof depends on exact ordered compound keys and the
/// finite BLOB constraint, so these are development-schema requirements.
pub(super) fn verify_schema(connection: &Connection) -> Result<(), ConversationStoreError> {
    for (table, last) in [
        ("content_segments", "segment"),
        ("content_frontiers", "revision"),
    ] {
        super::schema::shape::verify_unique_columns(
            connection,
            table,
            &[
                "source_kind",
                "source_id",
                "block_index",
                "representation",
                last,
            ],
        )?;
        let sql: String = connection
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name=?1",
                [table],
                |row| row.get(0),
            )
            .map_err(|error| storage(error.to_string()))?;
        let compact: String = sql
            .chars()
            .filter(|c| !c.is_whitespace())
            .flat_map(char::to_lowercase)
            .collect();
        if !compact.ends_with("withoutrowid")
            || (table == "content_segments" && !compact.contains("check(length(bytes)<=65536)"))
        {
            return Err(ConversationStoreError::IncompatibleSchema(format!(
                "invalid bounded content index {table}"
            )));
        }
    }
    Ok(())
}

pub(super) fn canonical_raw_arguments(
    connection: &Connection,
    messages: &[MessageBlock],
) -> Result<Vec<crate::durable::CanonicalRawToolArguments>, ConversationStoreError> {
    let mut result = Vec::new();
    for message in messages {
        let MessageBlock::Assistant(assistant) = message else {
            continue;
        };
        let key = canonical_key(connection, assistant.id.as_str())?;
        for (index, block) in assistant.content.iter().enumerate() {
            if !matches!(block, AssistantContentBlock::ToolCall(_)) {
                continue;
            }
            let block_index = u32::try_from(index).map_err(|_| invalid())?;
            let end:Option<u64>=connection.query_row("SELECT readable_end FROM content_frontiers WHERE source_kind='canonical' AND source_id=?1 AND block_index=?2 AND representation='raw_json_utf8' AND revision=1",params![key.as_str(),block_index],|row|row.get(0)).optional().map_err(|error|storage(error.to_string()))?;
            let Some(end) = end else { continue };
            let mut bytes = Vec::new();
            let mut offset = 0;
            while offset < end {
                let next = offset
                    .checked_add(u64::from(CONTENT_READ_MAX_BYTES))
                    .ok_or_else(invalid)?
                    .min(end);
                bytes.extend(range_bytes(
                    connection,
                    ("canonical", key.as_str(), block_index, Rep::RawJsonUtf8),
                    offset,
                    next,
                )?);
                offset = next;
            }
            result.push(crate::durable::CanonicalRawToolArguments {
                message_id: assistant.id.clone(),
                block_index: crate::message::types::ContentBlockIndex::new(block_index),
                bytes,
            });
        }
    }
    Ok(result)
}
pub(super) fn index_raw_arguments(
    connection: &Connection,
    item: &crate::durable::CanonicalRawToolArguments,
) -> Result<(), ConversationStoreError> {
    let key = canonical_key(connection, item.message_id.as_str())?;
    put(
        connection,
        (
            "canonical",
            key.as_str(),
            item.block_index.get(),
            Rep::RawJsonUtf8,
        ),
        1,
        &item.bytes,
        false,
    )
}

pub(super) fn index_model_name(
    connection: &Connection,
    source_sequence: u64,
    model: &str,
) -> Result<u64, ConversationStoreError> {
    super::content_encoding::put_json(
        connection,
        (
            "attempt_model",
            &source_sequence.to_string(),
            0,
            Rep::CanonicalJsonV1,
        ),
        1,
        &model,
        false,
    )
}
pub(super) fn model_name_range(
    connection: &Connection,
    source_sequence: u64,
    offset: u64,
    end: u64,
) -> Result<crate::durable::observation::ObservationValueChunk, ConversationStoreError> {
    let readable_end:u64=connection.query_row("SELECT readable_end FROM content_frontiers WHERE source_kind='attempt_model' AND source_id=?1 AND block_index=0 AND representation='canonical_json_v1' AND revision=1",[&source_sequence.to_string()],|row|row.get(0)).map_err(|error|storage(error.to_string()))?;
    if end > readable_end || offset > end || end - offset > u64::from(CONTENT_READ_MAX_BYTES) {
        return Err(ConversationStoreError::InvalidContentRange);
    }
    let bytes = range_bytes(
        connection,
        (
            "attempt_model",
            &source_sequence.to_string(),
            0,
            Rep::CanonicalJsonV1,
        ),
        offset,
        end,
    )?;
    Ok(crate::durable::observation::ObservationValueChunk {
        bytes,
        end_offset: end,
        readable_end,
        next_offset: (end < readable_end).then_some(end),
    })
}

pub(super) fn index_attempt_failure(
    connection: &Connection,
    sequence: u64,
    failure: &(impl serde::Serialize + schemars::JsonSchema),
) -> Result<u64, ConversationStoreError> {
    index_structural_value(
        connection,
        "attempt_failure",
        &format!("journal:{sequence}"),
        0,
        failure,
    )
}
pub(super) fn attempt_failure_range(
    connection: &Connection,
    sequence: u64,
    offset: u64,
    end: u64,
) -> Result<crate::durable::observation::ObservationValueChunk, ConversationStoreError> {
    let readable_end:u64=connection.query_row("SELECT readable_end FROM content_frontiers WHERE source_kind='attempt_failure' AND source_id=?1 AND block_index=0 AND representation='canonical_json_v1' AND revision=1",[&format!("journal:{sequence}")],|row|row.get(0)).map_err(|error|storage(error.to_string()))?;
    if end > readable_end || offset > end || end - offset > u64::from(CONTENT_READ_MAX_BYTES) {
        return Err(ConversationStoreError::InvalidContentRange);
    }
    let bytes = range_bytes(
        connection,
        (
            "attempt_failure",
            &format!("journal:{sequence}"),
            0,
            Rep::CanonicalJsonV1,
        ),
        offset,
        end,
    )?;
    Ok(crate::durable::observation::ObservationValueChunk {
        bytes,
        end_offset: end,
        readable_end,
        next_offset: (end < readable_end).then_some(end),
    })
}

/// Private native structural field layout, never a public `ContentRef` source.
/// Its owner validates source/cut identity before calling this byte primitive.
pub(super) fn index_utf8_field(
    connection: &Connection,
    kind: &str,
    id: &str,
    field: u32,
    text: &str,
) -> Result<u64, ConversationStoreError> {
    put(
        connection,
        (kind, id, field, Rep::Utf8Text),
        1,
        text.as_bytes(),
        false,
    )?;
    Ok(text.len() as u64)
}
pub(super) fn utf8_field_end(
    connection: &Connection,
    kind: &str,
    id: &str,
    field: u32,
) -> Result<Option<u64>, ConversationStoreError> {
    connection.query_row("SELECT readable_end FROM content_frontiers WHERE source_kind=?1 AND source_id=?2 AND block_index=?3 AND representation='utf8_text' AND revision=1",params![kind,id,field],|row|row.get(0)).optional().map_err(|error|storage(error.to_string()))
}
pub(super) fn utf8_field_read(
    connection: &Connection,
    kind: &str,
    id: &str,
    field: u32,
    offset: u64,
    max_bytes: u32,
) -> Result<crate::durable::observation::ObservationValueChunk, ConversationStoreError> {
    let readable_end = utf8_field_end(connection, kind, id, field)?.ok_or_else(invalid)?;
    if max_bytes == 0 || max_bytes > CONTENT_READ_MAX_BYTES || offset > readable_end {
        return Err(ConversationStoreError::InvalidContentRange);
    }
    let end = offset
        .checked_add(u64::from(max_bytes))
        .filter(|end| *end <= CONTENT_SAFE_INTEGER_MAX)
        .ok_or(ConversationStoreError::InvalidContentRange)?
        .min(readable_end);
    let bytes = range_bytes(connection, (kind, id, field, Rep::Utf8Text), offset, end)?;
    Ok(crate::durable::observation::ObservationValueChunk {
        bytes,
        end_offset: end,
        readable_end,
        next_offset: (end < readable_end).then_some(end),
    })
}
/// Encodes a borrowed native structural field into finite chunks without a whole byte buffer.
pub(super) fn index_structural_value<T: serde::Serialize + schemars::JsonSchema>(
    connection: &Connection,
    kind: &str,
    id: &str,
    field: u32,
    value: &T,
) -> Result<u64, ConversationStoreError> {
    use std::io::Write as _;
    let mut writer = StructuralFieldWriter {
        connection,
        kind,
        id,
        field,
        buffer: Vec::with_capacity(SEGMENT_BYTES),
        written: 0,
        segment: 0,
    };
    crate::observation::structural_json::to_writer(&mut writer, value)
        .map_err(|error| storage(error.to_string()))?;
    writer.flush().map_err(|error| storage(error.to_string()))?;
    connection
        .execute(
            "INSERT INTO content_frontiers VALUES(?1,?2,?3,'canonical_json_v1',1,?4)",
            params![kind, id, field, writer.written],
        )
        .map_err(|error| storage(error.to_string()))?;
    Ok(writer.written)
}
struct StructuralFieldWriter<'a> {
    connection: &'a Connection,
    kind: &'a str,
    id: &'a str,
    field: u32,
    buffer: Vec<u8>,
    written: u64,
    segment: u64,
}
impl StructuralFieldWriter<'_> {
    fn drain(&mut self) -> std::io::Result<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        self.connection
            .execute(
                "INSERT INTO content_segments VALUES(?1,?2,?3,'canonical_json_v1',?4,?5)",
                params![self.kind, self.id, self.field, self.segment, self.buffer],
            )
            .map_err(std::io::Error::other)?;
        self.written = self
            .written
            .checked_add(self.buffer.len() as u64)
            .filter(|end| *end <= CONTENT_SAFE_INTEGER_MAX)
            .ok_or_else(|| std::io::Error::other("native structural field length exhausted"))?;
        self.segment += 1;
        self.buffer.clear();
        Ok(())
    }
}
impl std::io::Write for StructuralFieldWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let mut at = 0;
        while at < bytes.len() {
            let count = (SEGMENT_BYTES - self.buffer.len()).min(bytes.len() - at);
            self.buffer.extend_from_slice(&bytes[at..at + count]);
            at += count;
            if self.buffer.len() == SEGMENT_BYTES {
                self.drain()?;
            }
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.drain()
    }
}
pub(super) fn structural_field_read(
    connection: &Connection,
    kind: &str,
    id: &str,
    field: u32,
    offset: u64,
    max_bytes: u32,
) -> Result<crate::durable::observation::ObservationValueChunk, ConversationStoreError> {
    let readable_end:u64=connection.query_row("SELECT readable_end FROM content_frontiers WHERE source_kind=?1 AND source_id=?2 AND block_index=?3 AND representation='canonical_json_v1' AND revision=1",params![kind,id,field],|row|row.get(0)).optional().map_err(|error|storage(error.to_string()))?.ok_or_else(invalid)?;
    if max_bytes == 0 || max_bytes > CONTENT_READ_MAX_BYTES || offset > readable_end {
        return Err(ConversationStoreError::InvalidContentRange);
    }
    let end = offset
        .checked_add(u64::from(max_bytes))
        .filter(|end| *end <= CONTENT_SAFE_INTEGER_MAX)
        .ok_or(ConversationStoreError::InvalidContentRange)?
        .min(readable_end);
    let bytes = range_bytes(
        connection,
        (kind, id, field, Rep::CanonicalJsonV1),
        offset,
        end,
    )?;
    Ok(crate::durable::observation::ObservationValueChunk {
        bytes,
        end_offset: end,
        readable_end,
        next_offset: (end < readable_end).then_some(end),
    })
}

pub(super) fn validate_source_cut(
    connection: &Connection,
    conversation: &ConversationId,
    reference: &ContentRef,
    cut: &crate::durable::observation::ObservationReadCut,
) -> Result<(), ConversationStoreError> {
    if reference.conversation_id != *conversation
        || cut.read.conversation_id != *conversation
        || cut.ledger_through != cut.read.ledger_through
    {
        return Err(invalid());
    }
    super::observation::validate(connection, conversation, cut)?;
    validate_source(connection, &reference.source)?;
    let allowed=match &reference.source {
        Source::CanonicalMessageBlock {locator,..}=> {
            let position=canonical_position(connection,locator)?;
            position<=cut.ledger_through && connection.query_row("SELECT journal_position<=?2 AND (transcript_position IS NULL OR transcript_position<=?3) FROM observation_headers WHERE ledger_position=?1",params![position,cut.read.journal,cut.read.transcript],|row|row.get::<_,bool>(0)).optional().map_err(|error|storage(error.to_string()))?.unwrap_or(false)
        },
        Source::DurableTranscriptAudit {stream_id,..}=>connection.query_row("SELECT transcript_position<=?2 AND journal_position<=?3 FROM observation_headers WHERE reference_kind='publication_audit' AND reference_id=?1",params![stream_id.as_str(),cut.read.transcript,cut.read.journal],|row|row.get::<_,bool>(0)).optional().map_err(|error|storage(error.to_string()))?.unwrap_or(false),
        Source::LivePublicationPrefix {..}=>true,
    };
    if !allowed {
        return Err(invalid());
    }
    Ok(())
}

fn live_representations() -> [(
    crate::observation::content::PublicationContentComponent,
    Rep,
); 4] {
    use crate::observation::content::PublicationContentComponent;
    [
        (PublicationContentComponent::Payload, Rep::Utf8Text),
        (PublicationContentComponent::Payload, Rep::RawJsonUtf8),
        (PublicationContentComponent::Payload, Rep::CanonicalJsonV1),
        (
            PublicationContentComponent::BlockMetadata,
            Rep::CanonicalJsonV1,
        ),
    ]
}
