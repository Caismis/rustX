//! Commit-time observation descriptor indexes.
//!
//! These writers run inside the caller's canonical, Journal, pending, or
//! publication transaction and derive finite display headers from the facts
//! that transaction commits. They are read layouts only, never authority.

use super::{audit_summary, block, cut, has_text_preview, identity, kind, message_locator};
use crate::durable::observation::{
    HEADER_ENCODED_MAX, HEADER_PAGE_MAX, MessageBlockHeader, MessageHeader, TranscriptHeader,
};
use crate::durable::sqlite::{
    Connection, ConversationStoreError, OptionalExtension, Transaction, params, seq_to_i64, storage,
};
use crate::message::types::{InboundKind, MessageBlock};
use crate::observation::bounded::bounded_bytes;
use crate::observation::capture::Capture;
use crate::observation::cut::ContentBinding;
use crate::observation::transcript::RuntimeClientTranscriptEntry;
use crate::runtime::identity::ConversationId;

pub(in crate::durable::sqlite) fn append_message_metadata(
    connection: &Connection,
    conversation: &ConversationId,
    message: &MessageBlock,
    count: usize,
    position: Option<u64>,
    semantic_fits: bool,
    capture: &mut Capture,
) -> Result<bool, ConversationStoreError> {
    let mut metadata_deferred = false;
    if let Some(position) = position {
        let source = crate::observation::content::ContentSource::CanonicalMessageBlock {
            component: crate::observation::content::CanonicalContentComponent::MessageMetadata,
            locator: message_locator(message.id(), position),
            block_index: crate::message::types::ContentBlockIndex::new(0),
        };
        if let Some(reference) = crate::durable::sqlite::content::descriptor(
            connection,
            conversation,
            &source,
            crate::observation::content::ContentRepresentation::CanonicalJsonV1,
        )? {
            metadata_deferred = !semantic_fits || reference.readable_end > 1024;
            capture
                .content
                .push(crate::observation::cut::ContentBinding {
                    source_path: None,
                    path: vec!["display".into()],
                    content_ref: reference,
                    deferred: metadata_deferred,
                });
        }
    }
    if let Some(position) = position {
        for index in 0..count.min(HEADER_PAGE_MAX) {
            let binary_source = crate::observation::content::ContentSource::CanonicalMessageBlock {
                component: crate::observation::content::CanonicalContentComponent::Payload,
                locator: message_locator(message.id(), position),
                block_index: crate::message::types::ContentBlockIndex::new(
                    u32::try_from(index).map_err(|_| storage("block index"))?,
                ),
            };
            if let Some(reference) = crate::durable::sqlite::content::descriptor(
                connection,
                conversation,
                &binary_source,
                crate::observation::content::ContentRepresentation::BinaryBytes,
            )? {
                let mut path = vec!["display".into()];
                if matches!(message, MessageBlock::Tool(_)) {
                    path.push("result".into());
                }
                path.extend(["content".into(), index.to_string(), "binary".into()]);
                capture
                    .content
                    .push(crate::observation::cut::ContentBinding {
                        source_path: None,
                        path,
                        content_ref: reference,
                        deferred: true,
                    });
            }
            let source = crate::observation::content::ContentSource::CanonicalMessageBlock {
                component: crate::observation::content::CanonicalContentComponent::BlockMetadata,
                locator: message_locator(message.id(), position),
                block_index: crate::message::types::ContentBlockIndex::new(
                    u32::try_from(index).map_err(|_| storage("canonical block index exhausted"))?,
                ),
            };
            if let Some(reference) = crate::durable::sqlite::content::descriptor(
                connection,
                conversation,
                &source,
                crate::observation::content::ContentRepresentation::CanonicalJsonV1,
            )? && reference.readable_end > 1024
            {
                metadata_deferred = true;
                let mut path = vec!["display".into()];
                if matches!(message, MessageBlock::Tool(_)) {
                    path.push("result".into());
                }
                path.extend(["content".into(), index.to_string()]);
                capture
                    .content
                    .push(crate::observation::cut::ContentBinding {
                        source_path: None,
                        path,
                        content_ref: reference,
                        deferred: true,
                    });
            }
        }
    }

    Ok(metadata_deferred)
}

pub(in crate::durable::sqlite) fn message_source_label(message: &MessageBlock) -> Option<String> {
    match message {
        MessageBlock::User(user) => Some(
            match user.source {
                crate::message::types::UserSource::Human => "human",
                crate::message::types::UserSource::Agent { .. } => "agent",
                crate::message::types::UserSource::Fleet => "fleet",
                crate::message::types::UserSource::ExternalSystem => "external_system",
                crate::message::types::UserSource::Runtime => "runtime",
                crate::message::types::UserSource::Extension { .. } => "certified_extension",
            }
            .into(),
        ),
        _ => None,
    }
}

pub(in crate::durable::sqlite) fn tool_call_count(message: &MessageBlock) -> u64 {
    match message {
        MessageBlock::Assistant(value) => value
            .content
            .iter()
            .filter(|block| {
                matches!(
                    block,
                    crate::message::types::AssistantContentBlock::ToolCall(_)
                )
            })
            .count() as u64,
        _ => 0,
    }
}

pub(in crate::durable::sqlite) fn index_message(
    connection: &Connection,
    message: &MessageBlock,
    position: Option<u64>,
) -> Result<(), ConversationStoreError> {
    if position.is_some() {
        index_blocks(connection, message)?;
    }
    let conversation = identity(connection)?;
    let (role, kind, count) = kind(message);
    let mut capture = Capture::new(1);
    // A descriptor must never change a native semantic family when growing
    // metadata cannot fit. Keep its exact kind/identity and mark it deferred.
    let semantic_fits = match message {
        MessageBlock::User(value) => {
            bounded_bytes(&value.kind, 1024).is_some()
                && bounded_bytes(&value.source, 1024).is_some()
        }
        _ => true,
    };
    let display = (semantic_fits && message.id().as_str().len() <= 256)
        .then(|| capture.message(message, &conversation, &["display".into()]));
    if display.is_none() && count > 0 {
        capture.frontier("display/content", count as u64, 0, Some(0));
    }
    if let Some(position) = position {
        for frontier in &mut capture.collections {
            if frontier.domain == "display/content" {
                frontier.domain = format!("messages.ledger:{position}.blocks");
            }
        }
    }
    let payload_deferred = !capture.content.is_empty();
    if position.is_some() {
        canonical_bindings(connection, &conversation, &mut capture)?;
    } else {
        // Pending inbound is not canonical content; no locator may claim otherwise.
        capture.content.clear();
    }
    let metadata_deferred = append_message_metadata(
        connection,
        &conversation,
        message,
        count,
        position,
        semantic_fits,
        &mut capture,
    )?;
    let deferred = !semantic_fits
        || payload_deferred
        || metadata_deferred
        || message.id().as_str().len() > 256;
    let mut header = MessageHeader {
        deliveries: if let (Some(position),MessageBlock::Tool(tool))=(position,message) {
            (tool.result.status==crate::tools::types::ToolExecutionStatus::Success).then(||crate::durable::sqlite::observation_delivery::head(connection,position,0,8)).transpose()?
        }else{None},
        tool_calls: Vec::new(),
        transcript_cursor: connection.query_row("SELECT position FROM transcript_order WHERE reference_kind='message' AND reference_id=?1",[message.id().as_str()],|row|row.get::<_,u64>(0)).optional().map_err(|error|storage(error.to_string()))?.map(crate::durable::TranscriptCursor::new),
        identity: (message.id().as_str().len() <= 256).then(|| message.id().clone()),
        locator: position.map(|position| {
            crate::observation::content::CanonicalContentLocator::ByLedgerPosition { position }
        }),
        ledger_position: position,
        role: role.into(),
        source_label: message_source_label(message),
        human_activity_at: match message {
            MessageBlock::User(user)
                if user.kind == InboundKind::Message
                    && matches!(user.source, crate::message::types::UserSource::Human) =>
            {
                user.timestamp
            }
            _ => None,
        },
        opening_preview: match message {
            MessageBlock::User(user) if user.kind == InboundKind::Message => {
                crate::local_runtime::session::preview_of(user)
            }
            _ => None,
        },
        has_text_preview: has_text_preview(message),
        opening_prompt: matches!(message,MessageBlock::User(value) if value.kind==InboundKind::Message && matches!(value.source,crate::message::types::UserSource::Human|crate::message::types::UserSource::Agent {..}) && value.content.iter().any(|block|matches!(block,crate::message::types::UserContentBlock::Text(text) if text.text.split_whitespace().next().is_some()))),
        kind: kind.into(),
        block_count: count as u64,
        tool_call_count: tool_call_count(message),
        display: (message.id().as_str().len() <= 256)
            .then_some(display)
            .flatten(),
        content: capture.content,
        collections: capture.collections,
        deferred,
    };
    if let Some(head) = &header.deliveries {
        header
            .collections
            .push(crate::observation::delivery::frontier(head));
    }
    let bytes = message_header_bytes(&mut header)?;
    connection.execute("INSERT INTO observation_headers(reference_kind,reference_id,ledger_position,header_json,transcript_position) VALUES('message',?1,?2,?3,(SELECT position FROM transcript_order WHERE reference_kind='message' AND reference_id=?1)) ON CONFLICT(reference_kind,reference_id) DO UPDATE SET ledger_position=excluded.ledger_position,header_json=excluded.header_json",params![message.id().as_str(),position.map(seq_to_i64).transpose()?,bytes]).map_err(|error|storage(error.to_string()))?;
    index_tool_result(connection, message, position)?;
    Ok(())
}

pub(in crate::durable::sqlite) fn index_tool_result(
    connection: &Connection,
    message: &MessageBlock,
    position: Option<u64>,
) -> Result<(), ConversationStoreError> {
    if let MessageBlock::Tool(tool) = message {
        let result = crate::durable::sqlite::observation_tools::result_header(&tool.result);
        let bytes = bounded_bytes(&result, HEADER_ENCODED_MAX)
            .ok_or_else(|| storage("native Tool result summary exceeds header budget"))?;
        connection
            .execute(
                "UPDATE observation_headers SET tool_result_json=?2 WHERE ledger_position=?1",
                params![position, bytes],
            )
            .map_err(|error| storage(error.to_string()))?;
    }
    Ok(())
}

pub(in crate::durable::sqlite) fn message_header_bytes(
    header: &mut MessageHeader,
) -> Result<Vec<u8>, ConversationStoreError> {
    loop {
        if let Some(bytes) = bounded_bytes(&header, HEADER_ENCODED_MAX) {
            return Ok(bytes);
        }
        header.deferred = true;
        if header.display.take().is_some() {
            for binding in &mut header.content {
                if matches!(
                    &binding.content_ref.source,
                    crate::observation::content::ContentSource::CanonicalMessageBlock {
                        component:
                            crate::observation::content::CanonicalContentComponent::MessageMetadata,
                        ..
                    }
                ) {
                    binding.deferred = true;
                }
            }
            continue;
        }
        if let Some(head) = header.deliveries.as_mut() {
            if head.items.iter().any(|item| item.file.is_some()) {
                for item in &mut head.items {
                    item.file = None;
                    item.metadata_deferred = true;
                }
                continue;
            }
            if let Some(item) = head.items.pop() {
                head.next_offset = Some(item.index);
                if let Some(frontier) = header
                    .collections
                    .iter_mut()
                    .find(|frontier| frontier.domain == head.domain)
                {
                    *frontier = crate::observation::delivery::frontier(head);
                }
                continue;
            }
        }
        let old = header.content.len();
        header.content.retain(|binding| {
            matches!(
                &binding.content_ref.source,
                crate::observation::content::ContentSource::CanonicalMessageBlock {
                    component:
                        crate::observation::content::CanonicalContentComponent::MessageMetadata,
                    ..
                }
            )
        });
        if header.content.len() != old {
            continue;
        }
        if let Some(position) = header.ledger_position {
            let blocks = format!("messages.ledger:{position}.blocks");
            let tools = format!("messages.ledger:{position}.tools");
            let mut essential = vec![
                crate::observation::cut::CollectionFrontier {
                    domain: blocks,
                    revision: 1,
                    total: header.block_count,
                    loaded: 0,
                    next_offset: (header.block_count > 0).then_some(0),
                },
                crate::observation::cut::CollectionFrontier {
                    domain: tools,
                    revision: 1,
                    total: header.tool_call_count,
                    loaded: 0,
                    next_offset: (header.tool_call_count > 0).then_some(0),
                },
            ];
            if let Some(head) = &header.deliveries {
                essential.push(crate::observation::delivery::frontier(head));
            }
            if header.collections != essential {
                header.collections = essential;
                continue;
            }
        }
        return Err(storage(
            "native message identity exceeds finite descriptor budget",
        ));
    }
}

pub(in crate::durable::sqlite) fn index_entry(
    connection: &Connection,
    kind: &str,
    id: &str,
    entry: &RuntimeClientTranscriptEntry,
) -> Result<(), ConversationStoreError> {
    let conversation = identity(connection)?;
    let mut capture = Capture::new(1);
    let display = capture.transcript_entry(entry, &conversation, &["display".into()]);
    if let crate::observation::transcript::RuntimeClientTranscriptItem::PublicationAudit { audit } =
        &entry.item
    {
        for (index, block) in audit.content.iter().take(HEADER_PAGE_MAX).enumerate() {
            if let crate::publication::PublicationAuditBlock::ProposedToolCall {
                block_index,
                name,
                ..
            } = block
                && name.len() > 1024
            {
                let source = crate::observation::content::ContentSource::DurableTranscriptAudit {
                    component:
                        crate::observation::content::PublicationContentComponent::BlockMetadata,
                    stream_id: audit.stream_id.clone(),
                    message_id: audit.message_id.clone(),
                    attempt_id: audit.attempt_id.clone(),
                    turn_id: audit.turn_id.clone(),
                    publication_request_id: audit.request_id.clone(),
                    block_index: *block_index,
                };
                if let Some(reference) = crate::durable::sqlite::content::descriptor(
                    connection,
                    &conversation,
                    &source,
                    crate::observation::content::ContentRepresentation::CanonicalJsonV1,
                )? {
                    capture
                        .content
                        .push(crate::observation::cut::ContentBinding {
                            source_path: None,
                            path: vec![
                                "display".into(),
                                "item".into(),
                                "audit".into(),
                                "content".into(),
                                index.to_string(),
                            ],
                            content_ref: reference,
                            deferred: true,
                        });
                }
            }
        }
    }
    let deferred = display.is_none() || !capture.content.is_empty();
    let header = TranscriptHeader {
        tool_calls: Vec::new(),
        publication_settlement: None,
        cursor: entry.cursor.into(),
        reference_kind: kind.into(),
        reference_id: if id.len() <= 256 {
            id.into()
        } else {
            format!("transcript:{}", entry.cursor.get())
        },
        message: None,
        display,
        content: capture.content,
        collections: capture.collections,
        deferred,
    };
    store_transcript_header(connection, kind, id, header)
}

pub(in crate::durable::sqlite) fn store_transcript_header(
    connection: &Connection,
    kind: &str,
    id: &str,
    mut header: TranscriptHeader,
) -> Result<(), ConversationStoreError> {
    let bytes = loop {
        if let Some(bytes) = bounded_bytes(&header, HEADER_ENCODED_MAX) {
            break bytes;
        }
        header.deferred = true;
        if header.display.take().is_some() {
            continue;
        }
        if header.content.pop().is_some() {
            continue;
        }
        if header.collections.pop().is_some() {
            continue;
        }
        return Err(storage(
            "native transcript identity exceeds finite descriptor budget",
        ));
    };
    let journal: u64 = if matches!(kind, "attempt_terminal" | "interaction_event") {
        connection
            .query_row(
                "SELECT sequence FROM events WHERE event_id=?1",
                [id],
                |row| row.get(0),
            )
            .map_err(|error| storage(error.to_string()))?
    } else {
        0
    };
    connection.execute("INSERT INTO observation_headers(reference_kind,reference_id,ledger_position,journal_position,header_json,transcript_position) VALUES(?1,?2,NULL,?3,?4,(SELECT position FROM transcript_order WHERE reference_kind=?1 AND reference_id=?2)) ON CONFLICT(reference_kind,reference_id) DO UPDATE SET journal_position=excluded.journal_position,header_json=excluded.header_json",params![kind,id,journal,bytes]).map_err(|error|storage(error.to_string()))?;
    Ok(())
}

pub(in crate::durable::sqlite) fn index_native_event(
    transaction: &Transaction<'_>,
    kind: &str,
    event: &crate::events::types::RuntimeEventEnvelope,
    cursor: crate::durable::TranscriptCursor,
) -> Result<(), ConversationStoreError> {
    if kind == "attempt_terminal" {
        let conversation = event.conversation_id.clone();
        let cut = cut(transaction, &conversation)?;
        let process = crate::durable::sqlite::observation_members::process_view(
            transaction,
            &cut,
            crate::durable::sqlite::observation_members::at_sequence(transaction, event.sequence)?,
            Some(cursor),
        )?
        .ok_or_else(|| storage("native terminal process unavailable"))?;
        return store_transcript_header(transaction,kind,event.event_id.as_str(),TranscriptHeader {
            tool_calls:Vec::new(),publication_settlement:None,cursor,reference_kind:kind.into(),reference_id:if event.event_id.as_str().len()<=256 {event.event_id.to_string()} else {format!("transcript:{}",cursor.get())},message:None,
            display:Some(crate::observation::transcript::RuntimeClientTranscriptEntry { cursor:crate::observation::transcript::RuntimeClientTranscriptCursor::new(cursor.get()), item:crate::observation::transcript::RuntimeClientTranscriptItem::AttemptTerminal{turn:process}, tool_calls:Vec::new(),turn_process:None,completed_response:None,response_pending:false }),
            content:Vec::new(),collections:Vec::new(),deferred:false,
        });
    }
    let Some(event_copy) = crate::observation::bounded::bounded_clone(event, HEADER_ENCODED_MAX)
    else {
        return store_transcript_header(
            transaction,
            kind,
            event.event_id.as_str(),
            TranscriptHeader {
                tool_calls: Vec::new(),
                publication_settlement: None,
                cursor,
                reference_kind: kind.into(),
                reference_id: if event.event_id.as_str().len() <= 256 {
                    event.event_id.to_string()
                } else {
                    format!("transcript:{}", cursor.get())
                },
                message: None,
                display: None,
                content: Vec::new(),
                collections: Vec::new(),
                deferred: true,
            },
        );
    };
    let item = match &event.event {
        crate::events::types::RuntimeEvent::InteractionRequested { .. } => {
            crate::durable::TranscriptItem::InteractionRequested { event: event_copy }
        }
        crate::events::types::RuntimeEvent::InteractionSettled { .. } => {
            crate::durable::TranscriptItem::InteractionSettled { event: event_copy }
        }
        _ => crate::durable::TranscriptItem::AttemptTerminal { event: event_copy },
    };
    index_item(transaction, kind, event.event_id.as_str(), item, cursor)
}

pub(in crate::durable::sqlite) fn index_item(
    transaction: &Transaction<'_>,
    kind: &str,
    id: &str,
    item: crate::durable::TranscriptItem,
    cursor: crate::durable::TranscriptCursor,
) -> Result<(), ConversationStoreError> {
    if kind == "inherited_turn" {
        let ordinal = id
            .parse::<u64>()
            .map_err(|_| storage("invalid original turn ordinal"))?;
        let conversation: String = transaction
            .query_row(
                "SELECT conversation_id FROM rustx_store WHERE id=1",
                [],
                |row| row.get(0),
            )
            .map_err(|error| storage(error.to_string()))?;
        let cut = cut(transaction, &ConversationId::new(conversation))?;
        let process = crate::durable::sqlite::observation_members::inherited_process(
            transaction,
            &cut,
            ordinal,
            cursor,
        )?
        .ok_or_else(|| storage("inherited terminal process unavailable"))?;
        return store_transcript_header(transaction,kind,id,TranscriptHeader {
            tool_calls:Vec::new(),publication_settlement:None,cursor,reference_kind:kind.into(),reference_id:id.into(),message:None,
            display:Some(crate::observation::transcript::RuntimeClientTranscriptEntry { cursor:crate::observation::transcript::RuntimeClientTranscriptCursor::new(cursor.get()), item:crate::observation::transcript::RuntimeClientTranscriptItem::AttemptTerminal{turn:process}, tool_calls:Vec::new(),turn_process:None,completed_response:None,response_pending:false }),
            content:Vec::new(),collections:Vec::new(),deferred:false,
        });
    }
    let page = crate::durable::TranscriptPage {
        inherited_through: crate::durable::TranscriptCursor::new(0),
        entries: vec![crate::durable::TranscriptEntry {
            cursor,
            item,
            tool_calls: Vec::new(),
        }],
        next_cursor: None,
    };
    let view = crate::observation::transcript::transcript_page_view(page).map_err(storage)?;
    let entry = view
        .entries
        .into_iter()
        .next()
        .ok_or_else(|| storage("native transcript descriptor absent"))?;
    index_entry(transaction, kind, id, &entry)
}

pub(in crate::durable::sqlite) fn index_blocks(
    connection: &Connection,
    message: &MessageBlock,
) -> Result<(), ConversationStoreError> {
    use crate::message::types::{AssistantContentBlock as Assistant, UserContentBlock as User};
    use crate::tools::types::ToolResultContent as Tool;
    match message {
        MessageBlock::User(value) => {
            for (index, value) in value.content.iter().enumerate() {
                let (kind, field): (&str, &[&str]) = match value {
                    User::Text(_) => ("text", &["text"]),
                    User::Image(_) => ("image", &[]),
                    User::File(_) => ("file", &[]),
                    User::UploadedFile(_) => ("uploaded_file", &[]),
                };
                block(connection, message, index, kind, value, field)?;
            }
        }
        MessageBlock::Assistant(value) => {
            for (index, value) in value.content.iter().enumerate() {
                let (kind, field): (&str, &[&str]) = match value {
                    Assistant::Text(_) => ("text", &["text"]),
                    Assistant::Reasoning(_) => ("reasoning", &["text"]),
                    Assistant::Refusal(_) => ("refusal", &["text"]),
                    Assistant::ToolCall(_) => ("tool_call", &["arguments"]),
                    Assistant::Image(_) => ("image", &[]),
                };
                block(connection, message, index, kind, value, field)?;
            }
        }
        MessageBlock::Tool(value) => {
            for (index, value) in value.result.content.iter().enumerate() {
                let (kind, field): (&str, &[&str]) = match value {
                    Tool::Text(_) => ("text", &["text"]),
                    Tool::Json { .. } => ("json", &["value"]),
                    Tool::Image(_) => ("image", &[]),
                    Tool::File(_) => ("file", &[]),
                };
                block(connection, message, index, kind, value, field)?;
            }
        }
    }
    Ok(())
}

/// Same native pending transaction stores a finite read header ahead of the original body.
pub(in crate::durable::sqlite) fn index_pending_header(
    connection: &Connection,
    sequence: u64,
    id: &crate::runtime::identity::MessageId,
    transcript: Option<u64>,
) -> Result<(), ConversationStoreError> {
    let bytes:Vec<u8>=connection.query_row("SELECT header_json FROM observation_headers WHERE reference_kind='message' AND reference_id=?1",[id.as_str()],|row|row.get(0)).map_err(|error|storage(error.to_string()))?;
    if bytes.len() > HEADER_ENCODED_MAX {
        return Err(storage("pending source header exceeds bound"));
    }
    connection.execute("UPDATE pending_inbound SET observation_header=?1,observation_transcript=COALESCE(?2,observation_transcript) WHERE sequence=?3",params![bytes,transcript,sequence]).map_err(|error|storage(error.to_string()))?;
    Ok(())
}

pub(in crate::durable::sqlite) fn index_audit(
    connection: &Connection,
    audit: &crate::publication::PublicationAudit,
    cursor: crate::durable::TranscriptCursor,
) -> Result<(), ConversationStoreError> {
    let conversation = identity(connection)?;
    let mut capture = Capture::new(1);
    let display = capture
        .audit(audit, &conversation, &["display".into()])
        .map(|audit| RuntimeClientTranscriptEntry {
            response_pending: false,
            cursor: cursor.into(),
            tool_calls: Vec::new(),
            turn_process: None,
            completed_response: None,
            item: crate::observation::transcript::RuntimeClientTranscriptItem::PublicationAudit {
                audit,
            },
        });
    audit_metadata_bindings(connection, audit, &conversation, &mut capture)?;
    for frontier in &mut capture.collections {
        frontier.domain = format!("transcript.entries.{}.item.audit.content", cursor.get());
        frontier.revision = cursor.get();
    }
    let deferred = display.is_none()
        || !capture.content.is_empty()
        || capture
            .collections
            .iter()
            .any(|collection| collection.loaded < collection.total);
    index_audit_blocks(connection, audit, &conversation, cursor)?;
    let header = TranscriptHeader {
        tool_calls: Vec::new(),
        publication_settlement: Some(
            crate::observation::transcript::RuntimeClientPublicationSettlementRecord {
                settlement: audit_summary(audit).into(),
                transcript_cursor: cursor.into(),
                read_cut: cut(connection, &conversation)?,
            },
        ),
        cursor,
        reference_kind: "publication_audit".into(),
        reference_id: if audit.stream_id.as_str().len() <= 256 {
            audit.stream_id.as_str().into()
        } else {
            format!("transcript:{}", cursor.get())
        },
        message: None,
        display,
        content: capture.content,
        collections: capture.collections,
        deferred,
    };
    store_transcript_header(
        connection,
        "publication_audit",
        audit.stream_id.as_str(),
        header,
    )
}

pub(in crate::durable::sqlite) fn audit_metadata_bindings(
    connection: &Connection,
    audit: &crate::publication::PublicationAudit,
    conversation: &ConversationId,
    capture: &mut Capture,
) -> Result<(), ConversationStoreError> {
    for (index, block) in audit.content.iter().take(HEADER_PAGE_MAX).enumerate() {
        if let crate::publication::PublicationAuditBlock::ProposedToolCall {
            block_index,
            name,
            call_id,
            tool_id,
            ..
        } = block
            && (name.len() > 1024 || call_id.as_str().len() > 256 || tool_id.as_str().len() > 256)
        {
            let source = crate::observation::content::ContentSource::DurableTranscriptAudit {
                component: crate::observation::content::PublicationContentComponent::BlockMetadata,
                stream_id: audit.stream_id.clone(),
                message_id: audit.message_id.clone(),
                attempt_id: audit.attempt_id.clone(),
                turn_id: audit.turn_id.clone(),
                publication_request_id: audit.request_id.clone(),
                block_index: *block_index,
            };
            if let Some(reference) = crate::durable::sqlite::content::descriptor(
                connection,
                conversation,
                &source,
                crate::observation::content::ContentRepresentation::CanonicalJsonV1,
            )? {
                capture
                    .content
                    .push(crate::observation::cut::ContentBinding {
                        source_path: None,
                        path: vec![
                            "display".into(),
                            "item".into(),
                            "audit".into(),
                            "content".into(),
                            index.to_string(),
                        ],
                        content_ref: reference,
                        deferred: true,
                    });
            }
        }
    }
    Ok(())
}

pub(in crate::durable::sqlite) fn index_audit_blocks(
    connection: &Connection,
    audit: &crate::publication::PublicationAudit,
    conversation: &ConversationId,
    cursor: crate::durable::TranscriptCursor,
) -> Result<(), ConversationStoreError> {
    for (ordinal, block) in audit.content.iter().enumerate() {
        use crate::observation::content::{
            ContentRepresentation as Rep, ContentSource, PublicationContentComponent as Component,
        };
        use crate::publication::PublicationAuditBlock as Block;
        let (block_index, kind, representation, field) = match block {
            Block::Text { block_index, .. } => (*block_index, "text", Rep::Utf8Text, "text"),
            Block::Reasoning { block_index, .. } => {
                (*block_index, "reasoning", Rep::Utf8Text, "text")
            }
            Block::Refusal { block_index, .. } => (*block_index, "refusal", Rep::Utf8Text, "text"),
            Block::ProposedToolCall { block_index, .. } => (
                *block_index,
                "proposed_tool_call",
                Rep::RawJsonUtf8,
                "arguments",
            ),
        };
        let source = |component| ContentSource::DurableTranscriptAudit {
            component,
            stream_id: audit.stream_id.clone(),
            message_id: audit.message_id.clone(),
            attempt_id: audit.attempt_id.clone(),
            turn_id: audit.turn_id.clone(),
            publication_request_id: audit.request_id.clone(),
            block_index,
        };
        let value = crate::observation::bounded::bounded_item_bytes(block, 1024)
            .and_then(|_| crate::observation::bounded::structural_value(block));
        let deferred = value.is_none();
        let mut content = Vec::new();
        if deferred {
            if let Some(reference) = crate::durable::sqlite::content::descriptor(
                connection,
                conversation,
                &source(Component::Payload),
                representation,
            )? {
                content.push(ContentBinding {
                    source_path: None,
                    path: vec![field.into()],
                    content_ref: reference,
                    deferred: true,
                });
            }
            if let Some(reference) = crate::durable::sqlite::content::descriptor(
                connection,
                conversation,
                &source(Component::BlockMetadata),
                Rep::CanonicalJsonV1,
            )? {
                content.push(ContentBinding {
                    source_path: None,
                    path: Vec::new(),
                    content_ref: reference,
                    deferred: true,
                });
            }
        }
        let row = MessageBlockHeader {
            block_index,
            kind: kind.into(),
            value,
            content,
            deferred,
        };
        let bytes = bounded_bytes(&row, HEADER_ENCODED_MAX)
            .ok_or_else(|| storage("audit block descriptor exceeds native bound"))?;
        connection
            .execute(
                "INSERT INTO audit_observation_blocks VALUES(?1,?2,?3)",
                params![cursor.get(), ordinal as u64, bytes],
            )
            .map_err(|error| storage(error.to_string()))?;
    }
    Ok(())
}

pub(in crate::durable::sqlite) fn canonical_bindings(
    connection: &Connection,
    conversation: &ConversationId,
    capture: &mut Capture,
) -> Result<(), ConversationStoreError> {
    for binding in &mut capture.content {
        let raw = if binding
            .path
            .last()
            .is_some_and(|field| field == "arguments")
        {
            crate::durable::sqlite::content::descriptor(
                connection,
                conversation,
                &binding.content_ref.source,
                crate::observation::content::ContentRepresentation::RawJsonUtf8,
            )?
        } else {
            None
        };
        if let Some(reference) = raw.or(crate::durable::sqlite::content::descriptor(
            connection,
            conversation,
            &binding.content_ref.source,
            binding.content_ref.representation,
        )?) {
            binding.content_ref = reference;
        } else {
            return Err(storage("canonical descriptor missing native byte index"));
        }
    }
    Ok(())
}
