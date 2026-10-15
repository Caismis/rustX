//! Native-transaction descriptor indexes with bounded physical observation reads.
use super::{
    Connection, ConversationStoreError, OptionalExtension, Transaction, nonnegative, params,
    seq_to_i64, storage,
};
use crate::durable::observation::{
    HEADER_ENCODED_MAX, HEADER_PAGE_MAX, MessageBlockHeader, MessageHeader, MessageHeaderPage,
    ObservationReadCut, TranscriptHeader, TranscriptHeaderPage,
};
use crate::message::types::{ContextKind, InboundKind, MessageBlock};
use crate::runtime::identity::ConversationId;
use crate::runtime_client::checkpoint::{Capture, ContentBinding, bounded_bytes};
use crate::runtime_client::snapshot::RuntimeClientTranscriptEntry;

pub(super) fn cut(
    connection: &Connection,
    conversation: &ConversationId,
) -> Result<ObservationReadCut, ConversationStoreError> {
    let read = super::reading::cut(connection, conversation)?;
    Ok(ObservationReadCut {
        ledger_through: read.ledger_through,
        transcript_total: read.transcript_total,
        surface_revision: read.surface_revision,
        read,
        inherited_through: super::inherited_transcript_through(connection)?,
    })
}
pub(super) fn validate(
    connection: &Connection,
    conversation: &ConversationId,
    expected: &ObservationReadCut,
) -> Result<(), ConversationStoreError> {
    let actual = cut(connection, conversation)?;
    if expected.read.ledger_through != expected.ledger_through
        || expected.read.surface_revision != expected.surface_revision
        || expected.read.transcript_total != expected.transcript_total
        || !expected.read.reconstructible_from(&actual.read)
        || expected.ledger_through > actual.ledger_through
        || expected.transcript_total > actual.transcript_total
        || expected.surface_revision > actual.surface_revision
        || expected.inherited_through != actual.inherited_through
    {
        return Err(ConversationStoreError::ObservationCutExpired);
    }
    validate_canonical_provenance(connection, expected)?;
    Ok(())
}
fn validate_canonical_provenance(
    connection: &Connection,
    expected: &ObservationReadCut,
) -> Result<(), ConversationStoreError> {
    if expected.ledger_through == 0 {
        return Ok(());
    }
    let (journal,transcript): (Option<u64>,Option<u64>)=connection.prepare_cached("SELECT h.journal_position,l.native_transcript FROM message_ledger l LEFT JOIN observation_headers h ON h.ledger_position=l.position WHERE l.position=?1").map_err(|error|storage(error.to_string()))?.query_row([expected.ledger_through],|row|Ok((row.get(0)?,row.get(1)?))).optional().map_err(|error|storage(error.to_string()))?.ok_or_else(||storage("native canonical source position missing"))?;
    let journal = journal.ok_or_else(|| storage("native canonical cut provenance unavailable"))?;
    let append:Option<u64>=connection.prepare_cached("SELECT first_append_revision FROM message_ledger INDEXED BY message_ledger_append_position_idx WHERE first_append_revision IS NOT NULL AND position<=?1 ORDER BY position DESC LIMIT 1").map_err(|error|storage(error.to_string()))?.query_row([expected.ledger_through],|row|row.get(0)).optional().map_err(|error|storage(error.to_string()))?;
    if journal > expected.read.journal
        || transcript.is_some_and(|position| position > expected.read.transcript)
        || append.is_some_and(|revision| revision > expected.surface_revision.get())
    {
        return Err(ConversationStoreError::InvalidReference(
            "inconsistent canonical source cut provenance".into(),
        ));
    }
    Ok(())
}
fn kind(message: &MessageBlock) -> (&'static str, &'static str, usize) {
    match message {
        MessageBlock::Assistant(value) => ("assistant", "assistant", value.content.len()),
        MessageBlock::Tool(value) => ("tool", "tool", value.result.content.len()),
        MessageBlock::User(value) => {
            let kind = match &value.kind {
                InboundKind::Message => "message",
                InboundKind::GoalContinuation(_) => "goal_continuation",
                InboundKind::Context(ContextKind::NativeEnvironment) => {
                    "context/native_environment"
                }
                InboundKind::Context(ContextKind::GoalStatus(_)) => "context/goal_status",
                InboundKind::Context(ContextKind::RuntimeToolObservation) => {
                    "context/runtime_tool_observation"
                }
                InboundKind::Context(ContextKind::ExtensionEnvironment) => {
                    "context/extension_environment"
                }
                InboundKind::Context(ContextKind::AgentStatus(_)) => "context/agent_status",
            };
            ("user", kind, value.content.len())
        }
    }
}
fn has_text_preview(message: &MessageBlock) -> bool {
    use crate::message::types::{AssistantContentBlock as Assistant, UserContentBlock as User};
    match message {
        MessageBlock::User(user)=>user.content.iter().any(|block|matches!(block,User::Text(text) if text.text.split_whitespace().next().is_some())),
        MessageBlock::Assistant(assistant)=>assistant.content.iter().any(|block|match block {Assistant::Text(text)=>text.text.split_whitespace().next().is_some(),Assistant::Refusal(text)=>text.text.split_whitespace().next().is_some(),_=>false}),
        MessageBlock::Tool(_)=>false,
    }
}
fn identity(connection: &Connection) -> Result<ConversationId, ConversationStoreError> {
    let id: String = connection
        .query_row(
            "SELECT conversation_id FROM rustx_store WHERE id=1",
            [],
            |row| row.get(0),
        )
        .map_err(|error| storage(error.to_string()))?;
    ConversationId::parse(id).map_err(storage)
}
fn message_locator(
    message_id: &crate::runtime::identity::MessageId,
    position: u64,
) -> crate::runtime_client::content::CanonicalContentLocator {
    if message_id.as_str().len() <= 256 {
        crate::runtime_client::content::CanonicalContentLocator::ByMessageId {
            message_id: message_id.clone(),
        }
    } else {
        crate::runtime_client::content::CanonicalContentLocator::ByLedgerPosition { position }
    }
}
fn append_message_metadata(
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
        let source = crate::runtime_client::content::ContentSource::CanonicalMessageBlock {
            component: crate::runtime_client::content::CanonicalContentComponent::MessageMetadata,
            locator: message_locator(message.id(), position),
            block_index: crate::message::types::ContentBlockIndex::new(0),
        };
        if let Some(reference) = super::content::descriptor(
            connection,
            conversation,
            &source,
            crate::runtime_client::content::ContentRepresentation::CanonicalJsonV1,
        )? {
            metadata_deferred = !semantic_fits || reference.readable_end > 1024;
            capture
                .content
                .push(crate::runtime_client::checkpoint::ContentBinding {
                    source_path: None,
                    path: vec!["display".into()],
                    content_ref: reference,
                    deferred: metadata_deferred,
                });
        }
    }
    if let Some(position) = position {
        for index in 0..count.min(HEADER_PAGE_MAX) {
            let binary_source =
                crate::runtime_client::content::ContentSource::CanonicalMessageBlock {
                    component: crate::runtime_client::content::CanonicalContentComponent::Payload,
                    locator: message_locator(message.id(), position),
                    block_index: crate::message::types::ContentBlockIndex::new(
                        u32::try_from(index).map_err(|_| storage("block index"))?,
                    ),
                };
            if let Some(reference) = super::content::descriptor(
                connection,
                conversation,
                &binary_source,
                crate::runtime_client::content::ContentRepresentation::BinaryBytes,
            )? {
                let mut path = vec!["display".into()];
                if matches!(message, MessageBlock::Tool(_)) {
                    path.push("result".into());
                }
                path.extend(["content".into(), index.to_string(), "binary".into()]);
                capture
                    .content
                    .push(crate::runtime_client::checkpoint::ContentBinding {
                        source_path: None,
                        path,
                        content_ref: reference,
                        deferred: true,
                    });
            }
            let source = crate::runtime_client::content::ContentSource::CanonicalMessageBlock {
                component: crate::runtime_client::content::CanonicalContentComponent::BlockMetadata,
                locator: message_locator(message.id(), position),
                block_index: crate::message::types::ContentBlockIndex::new(
                    u32::try_from(index).map_err(|_| storage("canonical block index exhausted"))?,
                ),
            };
            if let Some(reference) = super::content::descriptor(
                connection,
                conversation,
                &source,
                crate::runtime_client::content::ContentRepresentation::CanonicalJsonV1,
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
                    .push(crate::runtime_client::checkpoint::ContentBinding {
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
fn message_source_label(message: &MessageBlock) -> Option<String> {
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
fn tool_call_count(message: &MessageBlock) -> u64 {
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
pub(super) fn index_message(
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
            (tool.result.status==crate::tools::types::ToolExecutionStatus::Success).then(||super::observation_delivery::head(connection,position,0,8)).transpose()?
        }else{None},
        tool_calls: Vec::new(),
        transcript_cursor: connection.query_row("SELECT position FROM transcript_order WHERE reference_kind='message' AND reference_id=?1",[message.id().as_str()],|row|row.get::<_,u64>(0)).optional().map_err(|error|storage(error.to_string()))?.map(crate::durable::TranscriptCursor::new),
        identity: (message.id().as_str().len() <= 256).then(|| message.id().clone()),
        locator: position.map(|position| {
            crate::runtime_client::content::CanonicalContentLocator::ByLedgerPosition { position }
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
            .push(crate::runtime_client::delivery::frontier(head));
    }
    let bytes = message_header_bytes(&mut header)?;
    connection.execute("INSERT INTO observation_headers(reference_kind,reference_id,ledger_position,header_json,transcript_position) VALUES('message',?1,?2,?3,(SELECT position FROM transcript_order WHERE reference_kind='message' AND reference_id=?1)) ON CONFLICT(reference_kind,reference_id) DO UPDATE SET ledger_position=excluded.ledger_position,header_json=excluded.header_json",params![message.id().as_str(),position.map(seq_to_i64).transpose()?,bytes]).map_err(|error|storage(error.to_string()))?;
    index_tool_result(connection, message, position)?;
    Ok(())
}
fn index_tool_result(
    connection: &Connection,
    message: &MessageBlock,
    position: Option<u64>,
) -> Result<(), ConversationStoreError> {
    if let MessageBlock::Tool(tool) = message {
        let result = super::observation_tools::result_header(&tool.result);
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
fn message_header_bytes(header: &mut MessageHeader) -> Result<Vec<u8>, ConversationStoreError> {
    loop {
        if let Some(bytes) = bounded_bytes(&header, HEADER_ENCODED_MAX) {
            return Ok(bytes);
        }
        header.deferred = true;
        if header.display.take().is_some() {
            for binding in &mut header.content {
                if matches!(&binding.content_ref.source,crate::runtime_client::content::ContentSource::CanonicalMessageBlock{component:crate::runtime_client::content::CanonicalContentComponent::MessageMetadata,..}) { binding.deferred=true; }
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
                    *frontier = crate::runtime_client::delivery::frontier(head);
                }
                continue;
            }
        }
        let old = header.content.len();
        header.content.retain(|binding| {
            matches!(
                &binding.content_ref.source,
                crate::runtime_client::content::ContentSource::CanonicalMessageBlock {
                    component:
                        crate::runtime_client::content::CanonicalContentComponent::MessageMetadata,
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
                crate::runtime_client::checkpoint::CollectionFrontier {
                    domain: blocks,
                    revision: 1,
                    total: header.block_count,
                    loaded: 0,
                    next_offset: (header.block_count > 0).then_some(0),
                },
                crate::runtime_client::checkpoint::CollectionFrontier {
                    domain: tools,
                    revision: 1,
                    total: header.tool_call_count,
                    loaded: 0,
                    next_offset: (header.tool_call_count > 0).then_some(0),
                },
            ];
            if let Some(head) = &header.deliveries {
                essential.push(crate::runtime_client::delivery::frontier(head));
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
pub(super) fn index_entry(
    connection: &Connection,
    kind: &str,
    id: &str,
    entry: &RuntimeClientTranscriptEntry,
) -> Result<(), ConversationStoreError> {
    let conversation = identity(connection)?;
    let mut capture = Capture::new(1);
    let display = capture.transcript_entry(entry, &conversation, &["display".into()]);
    if let crate::runtime_client::snapshot::RuntimeClientTranscriptItem::PublicationAudit {
        audit,
    } = &entry.item
    {
        for (index, block) in audit.content.iter().take(HEADER_PAGE_MAX).enumerate() {
            if let crate::publication::PublicationAuditBlock::ProposedToolCall {
                block_index,
                name,
                ..
            } = block
                && name.len() > 1024
            {
                let source=crate::runtime_client::content::ContentSource::DurableTranscriptAudit {component:crate::runtime_client::content::PublicationContentComponent::BlockMetadata,stream_id:audit.stream_id.clone(),message_id:audit.message_id.clone(),attempt_id:audit.attempt_id.clone(),turn_id:audit.turn_id.clone(),publication_request_id:audit.request_id.clone(),block_index:*block_index};
                if let Some(reference) = super::content::descriptor(
                    connection,
                    &conversation,
                    &source,
                    crate::runtime_client::content::ContentRepresentation::CanonicalJsonV1,
                )? {
                    capture
                        .content
                        .push(crate::runtime_client::checkpoint::ContentBinding {
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
fn store_transcript_header(
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
pub(super) fn index_native_event(
    transaction: &Transaction<'_>,
    kind: &str,
    event: &crate::events::types::RuntimeEventEnvelope,
    cursor: crate::durable::TranscriptCursor,
) -> Result<(), ConversationStoreError> {
    if kind == "attempt_terminal" {
        let conversation = event.conversation_id.clone();
        let cut = cut(transaction, &conversation)?;
        let process = super::observation_members::process_view(
            transaction,
            &cut,
            super::observation_members::at_sequence(transaction, event.sequence)?,
            Some(cursor),
        )?
        .ok_or_else(|| storage("native terminal process unavailable"))?;
        return store_transcript_header(transaction,kind,event.event_id.as_str(),TranscriptHeader {
            tool_calls:Vec::new(),publication_settlement:None,cursor,reference_kind:kind.into(),reference_id:if event.event_id.as_str().len()<=256 {event.event_id.to_string()} else {format!("transcript:{}",cursor.get())},message:None,
            display:Some(crate::runtime_client::snapshot::RuntimeClientTranscriptEntry { cursor:crate::runtime_client::snapshot::RuntimeClientTranscriptCursor::new(cursor.get()), item:crate::runtime_client::snapshot::RuntimeClientTranscriptItem::AttemptTerminal{turn:process}, tool_calls:Vec::new(),turn_process:None,completed_response:None,response_pending:false }),
            content:Vec::new(),collections:Vec::new(),deferred:false,
        });
    }
    let Some(event_copy) =
        crate::runtime_client::checkpoint::bounded_clone(event, HEADER_ENCODED_MAX)
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
pub(super) fn index_item(
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
        let process =
            super::observation_members::inherited_process(transaction, &cut, ordinal, cursor)?
                .ok_or_else(|| storage("inherited terminal process unavailable"))?;
        return store_transcript_header(transaction,kind,id,TranscriptHeader {
            tool_calls:Vec::new(),publication_settlement:None,cursor,reference_kind:kind.into(),reference_id:id.into(),message:None,
            display:Some(crate::runtime_client::snapshot::RuntimeClientTranscriptEntry { cursor:crate::runtime_client::snapshot::RuntimeClientTranscriptCursor::new(cursor.get()), item:crate::runtime_client::snapshot::RuntimeClientTranscriptItem::AttemptTerminal{turn:process}, tool_calls:Vec::new(),turn_process:None,completed_response:None,response_pending:false }),
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
    let view = crate::runtime_client::snapshot::transcript_page_view(page).map_err(storage)?;
    let entry = view
        .entries
        .into_iter()
        .next()
        .ok_or_else(|| storage("native transcript descriptor absent"))?;
    index_entry(transaction, kind, id, &entry)
}
fn limit(limit: usize) -> Result<usize, ConversationStoreError> {
    if limit == 0 || limit > HEADER_PAGE_MAX {
        return Err(ConversationStoreError::InvalidReference(
            "observation page limit must be 1..32".into(),
        ));
    }
    Ok(limit)
}
pub(super) fn enrich_message_header(
    connection: &Connection,
    conversation: &ConversationId,
    cut: &ObservationReadCut,
    header: &mut MessageHeader,
) -> Result<(), ConversationStoreError> {
    if header.role != "assistant" || header.tool_call_count == 0 {
        return Ok(());
    }
    let locator = header
        .locator
        .as_ref()
        .ok_or_else(|| storage("native Tool descriptor locator unavailable"))?;
    let page = super::observation_tools::page(connection, conversation, cut, locator, 0, 32)?;
    crate::runtime_client::tool_observation::enrich_message(header, &page)
        .map_err(|error| storage(format!("{error:?}")))
}

pub(super) fn messages(
    connection: &Connection,
    conversation: &ConversationId,
    at: &ObservationReadCut,
    offset: Option<u64>,
    requested: usize,
) -> Result<MessageHeaderPage, ConversationStoreError> {
    validate(connection, conversation, at)?;
    let limit = limit(requested)?;
    // Ledger positions are append-only contiguous native allocation; no body scan/count.
    let total = at.ledger_through;
    let offset = offset.unwrap_or_else(|| total.saturating_sub(limit as u64));
    if offset > total {
        return Err(ConversationStoreError::InvalidReference(
            "message header offset beyond native frontier".into(),
        ));
    }
    let mut statement=connection.prepare_cached("SELECT h.header_json FROM observation_headers h WHERE h.ledger_position>?1 AND h.ledger_position<=?2 AND h.reference_kind='message' ORDER BY h.ledger_position LIMIT ?3").map_err(|error|storage(error.to_string()))?;
    let rows = statement
        .query_map(
            params![seq_to_i64(offset)?, seq_to_i64(total)?, limit],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .map_err(|error| storage(error.to_string()))?;
    let mut headers = Vec::new();
    for row in rows {
        let bytes = row.map_err(|error| storage(error.to_string()))?;
        if bytes.len() > HEADER_ENCODED_MAX {
            return Err(storage("oversized observation index"));
        }
        let mut header: MessageHeader =
            serde_json::from_slice(&bytes).map_err(|error| storage(error.to_string()))?;
        let expected = offset + headers.len() as u64 + 1;
        if header.ledger_position != Some(expected) {
            return Err(storage("native message descriptor index incomplete"));
        }
        enrich_message_header(connection, conversation, at, &mut header)?;
        headers.push(header);
    }
    if headers.len() as u64 != (total - offset).min(limit as u64) {
        return Err(storage("native message descriptor index incomplete"));
    }
    let next = offset
        .checked_add(headers.len() as u64)
        .ok_or(ConversationStoreError::SequenceExhausted)?;
    if next == offset && offset < total {
        return Err(storage("native message descriptor index incomplete"));
    }
    Ok(MessageHeaderPage {
        cut: at.clone(),
        total,
        offset,
        headers,
        next_offset: (next < total).then_some(next),
    })
}
pub(super) fn transcript(
    connection: &Connection,
    conversation: &ConversationId,
    at: &ObservationReadCut,
    before: Option<crate::durable::TranscriptCursor>,
    requested: usize,
) -> Result<TranscriptHeaderPage, ConversationStoreError> {
    validate(connection, conversation, at)?;
    let limit = limit(requested)?;
    let before = before.map_or(
        at.read.transcript.saturating_add(1),
        crate::durable::TranscriptCursor::get,
    );
    let mut statement=connection.prepare_cached("SELECT t.position,t.reference_kind,CASE WHEN t.reference_kind='message' THEN COALESCE(json_extract(h.header_json,'$.identity'),'ledger:'||h.ledger_position) ELSE COALESCE(json_extract(h.header_json,'$.reference_id'),'transcript:'||t.position) END,h.header_json,h.ledger_position,h.journal_position FROM transcript_order t LEFT JOIN observation_headers h INDEXED BY observation_headers_transcript_idx ON h.transcript_position=t.position AND h.reference_kind=t.reference_kind WHERE t.position<?1 AND t.position<=?2 ORDER BY t.position DESC LIMIT ?3").map_err(|error|storage(error.to_string()))?;
    let rows = statement
        .query_map(
            params![
                seq_to_i64(before)?,
                seq_to_i64(at.read.transcript)?,
                limit + 1,
            ],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<Vec<u8>>>(3)?,
                    row.get::<_, Option<u64>>(4)?,
                    row.get::<_, Option<u64>>(5)?,
                ))
            },
        )
        .map_err(|error| storage(error.to_string()))?;
    let mut headers = Vec::new();
    let mut more = false;
    for row in rows {
        let (position, kind, id, bytes, ledger, journal) =
            row.map_err(|error| storage(error.to_string()))?;
        if headers.len() == limit {
            more = true;
            break;
        }
        let bytes =
            bytes.ok_or_else(|| storage("native transcript descriptor index incomplete"))?;
        let id = id.ok_or_else(|| storage("native transcript descriptor identity unavailable"))?;
        let journal =
            journal.ok_or_else(|| storage("native transcript descriptor proof unavailable"))?;
        if ledger.is_some_and(|position| position > at.ledger_through) || journal > at.read.journal
        {
            return Err(ConversationStoreError::InvalidReference(
                "native transcript descriptor exceeds captured source cut".into(),
            ));
        }
        if bytes.len() > HEADER_ENCODED_MAX {
            return Err(storage("oversized observation index"));
        }
        let cursor =
            crate::durable::TranscriptCursor::new(nonnegative(position, "transcript header")?);
        let mut tool_locator = None;
        let mut header = if kind == "message" {
            let message: MessageHeader =
                serde_json::from_slice(&bytes).map_err(|error| storage(error.to_string()))?;
            if message.ledger_position != ledger {
                return Err(storage(
                    "native transcript descriptor ledger proof mismatch",
                ));
            }
            if message.role == "assistant" && message.tool_call_count > 0 {
                tool_locator.clone_from(&message.locator);
            }
            transcript_message_header(cursor, kind, id, message)
        } else {
            serde_json::from_slice(&bytes).map_err(|error| storage(error.to_string()))?
        };
        header.cursor = cursor;
        if let Some(display) = &mut header.display {
            display.cursor = cursor.into();
        }
        if let Some(locator) = tool_locator {
            enrich_transcript_tools(connection, conversation, at, &locator, &mut header)?;
        }
        headers.push(header);
    }
    let next_cursor = more
        .then(|| headers.last().map(|header| header.cursor))
        .flatten();
    headers.reverse();
    Ok(TranscriptHeaderPage {
        cut: at.clone(),
        total: at.transcript_total,
        headers,
        next_cursor,
    })
}
fn enrich_transcript_tools(
    connection: &Connection,
    conversation: &ConversationId,
    at: &ObservationReadCut,
    locator: &crate::runtime_client::content::CanonicalContentLocator,
    header: &mut TranscriptHeader,
) -> Result<(), ConversationStoreError> {
    let tools = super::observation_tools::page(connection, conversation, at, locator, 0, 32)?;
    if header.message.is_some() {
        crate::runtime_client::tool_observation::enrich_transcript_header(header, &tools)
    } else {
        crate::runtime_client::tool_observation::enrich_loaded_transcript_header(header, &tools)
    }
    .map_err(|error| storage(format!("{error:?}")))
}

fn transcript_message_header(
    cursor: crate::durable::TranscriptCursor,
    kind: String,
    id: String,
    mut message: MessageHeader,
) -> TranscriptHeader {
    if let Some(display_message) = message.display.take() {
        let mut content = message.content;
        for binding in &mut content {
            let mut path = vec!["display".into(), "item".into(), "message".into()];
            path.extend(binding.path.iter().skip(1).cloned());
            binding.path = path;
        }
        TranscriptHeader {
            tool_calls: Vec::new(),
            publication_settlement: None,
            cursor,
            reference_kind: kind,
            reference_id: id,
            message: None,
            display: Some(RuntimeClientTranscriptEntry {
                cursor: cursor.into(),
                item: crate::runtime_client::snapshot::RuntimeClientTranscriptItem::Message {
                    message: display_message,
                },
                turn_process: None,
                response_pending: false,
                completed_response: None,
                tool_calls: Vec::new(),
            }),
            content,
            collections: message.collections,
            deferred: message.deferred,
        }
    } else {
        TranscriptHeader {
            tool_calls: Vec::new(),
            publication_settlement: None,
            cursor,
            reference_kind: kind,
            reference_id: id,
            content: message.content.clone(),
            collections: message.collections.clone(),
            message: Some(message),
            display: None,
            deferred: true,
        }
    }
}

pub(super) fn verify_schema(connection: &Connection) -> Result<(), ConversationStoreError> {
    super::verify_unique_columns(
        connection,
        "observation_block_headers",
        &["message_id", "block_index"],
    )?;
    super::verify_unique_columns(
        connection,
        "observation_headers",
        &["reference_kind", "reference_id"],
    )?;
    let sql: String = connection
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE type='table' AND name='observation_headers'",
            [],
            |row| row.get(0),
        )
        .map_err(|error| storage(error.to_string()))?;
    let blocks: String = connection
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE type='table' AND name='observation_block_headers'",
            [],
            |row| row.get(0),
        )
        .map_err(|error| storage(error.to_string()))?;
    let events: String = connection
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE type='table' AND name='events'",
            [],
            |row| row.get(0),
        )
        .map_err(|error| storage(error.to_string()))?;
    if !blocks.contains("CHECK(length(header_json)<=16384)")
        || !blocks.contains("WITHOUT ROWID")
        || !events.contains("CHECK(observation_json IS NULL OR length(observation_json)<=16384)")
    {
        return Err(ConversationStoreError::IncompatibleSchema(
            "invalid finite native observation metadata layout".into(),
        ));
    }
    super::verify_unique_columns(
        connection,
        "observation_block_headers",
        &["ledger_position", "block_index"],
    )?;
    let first_column: String = connection
        .query_row(
            "SELECT name FROM pragma_table_info('observation_headers') ORDER BY cid LIMIT 1",
            [],
            |row| row.get(0),
        )
        .map_err(|error| storage(error.to_string()))?;
    // ROWID indirection and metadata-first layout keep numeric header reads
    // independent of enormous retained original identity keys.
    if !sql.contains("CHECK(length(header_json)<=16384)")
        || sql.contains("WITHOUT ROWID")
        || first_column != "header_json"
    {
        return Err(ConversationStoreError::IncompatibleSchema(
            "invalid finite observation header layout".into(),
        ));
    }
    Ok(())
}

pub(super) fn message(
    connection: &Connection,
    message_id: &crate::runtime::identity::MessageId,
) -> Result<Option<MessageHeader>, ConversationStoreError> {
    let bytes:Option<Vec<u8>>=connection.query_row("SELECT h.header_json FROM message_ledger m JOIN observation_headers h ON h.reference_kind='message' AND h.reference_id=m.message_id WHERE m.message_id=?1",[message_id.as_str()],|row|row.get(0)).optional().map_err(|error|storage(error.to_string()))?;
    bytes
        .map(|bytes| {
            if bytes.len() > HEADER_ENCODED_MAX {
                return Err(storage("oversized native message header"));
            }
            serde_json::from_slice(&bytes).map_err(|error| storage(error.to_string()))
        })
        .transpose()
}
fn block(
    connection: &Connection,
    message: &MessageBlock,
    index: usize,
    kind: &str,
    body: &impl serde::Serialize,
    field: &[&str],
) -> Result<(), ConversationStoreError> {
    use crate::runtime_client::content::{ContentRepresentation as Rep, ContentSource};
    let conversation = identity(connection)?;
    let block_index = crate::message::types::ContentBlockIndex::new(
        u32::try_from(index).map_err(|_| storage("canonical block index exhausted"))?,
    );
    let position = connection
        .query_row(
            "SELECT position FROM message_ledger WHERE message_id=?1",
            [message.id().as_str()],
            |row| row.get(0),
        )
        .map_err(|error| storage(error.to_string()))?;
    let locator = message_locator(message.id(), position);
    let source = ContentSource::CanonicalMessageBlock {
        component: crate::runtime_client::content::CanonicalContentComponent::Payload,
        locator: locator.clone(),
        block_index,
    };
    let value = bounded_bytes(body, 1024)
        .map(|bytes| serde_json::from_slice(&bytes).expect("native canonical block encoding"));
    let mut content = Vec::new();
    for representation in [
        Rep::Utf8Text,
        Rep::RawJsonUtf8,
        Rep::CanonicalJsonV1,
        Rep::BinaryBytes,
    ] {
        if let Some(reference) =
            super::content::descriptor(connection, &conversation, &source, representation)?
        {
            let mut path = vec!["value".into()];
            if representation == Rep::BinaryBytes {
                path.push("binary".into());
            } else {
                path.extend(field.iter().map(|value| (*value).into()));
            }
            content.push(crate::runtime_client::checkpoint::ContentBinding {
                source_path: None,
                path,
                content_ref: reference,
                deferred: value.is_none() || representation == Rep::BinaryBytes,
            });
        }
    }
    if value.is_none() {
        let metadata_source = ContentSource::CanonicalMessageBlock {
            component: crate::runtime_client::content::CanonicalContentComponent::BlockMetadata,
            locator,
            block_index,
        };
        if let Some(reference) = super::content::descriptor(
            connection,
            &conversation,
            &metadata_source,
            Rep::CanonicalJsonV1,
        )? {
            content.push(crate::runtime_client::checkpoint::ContentBinding {
                source_path: None,
                path: vec!["value".into()],
                content_ref: reference,
                deferred: true,
            });
        }
    }
    let header = crate::durable::observation::MessageBlockHeader {
        block_index,
        kind: kind.into(),
        deferred: value.is_none(),
        value,
        content,
    };
    let bytes = bounded_bytes(&header, HEADER_ENCODED_MAX)
        .ok_or_else(|| storage("canonical block identity exceeds finite descriptor budget"))?;
    connection.execute("INSERT INTO observation_block_headers(message_id,block_index,header_json,ledger_position) VALUES(?1,?2,?3,(SELECT position FROM message_ledger WHERE message_id=?1)) ON CONFLICT(message_id,block_index) DO UPDATE SET header_json=excluded.header_json",params![message.id().as_str(),block_index.get(),bytes]).map_err(|error|storage(error.to_string()))?;
    Ok(())
}
fn index_blocks(
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
pub(super) fn blocks(
    connection: &Connection,
    conversation: &ConversationId,
    at: &ObservationReadCut,
    message_id: &crate::runtime::identity::MessageId,
    offset: u64,
    requested: usize,
) -> Result<crate::durable::observation::MessageBlockHeaderPage, ConversationStoreError> {
    blocks_by_locator(
        connection,
        conversation,
        at,
        &crate::runtime_client::content::CanonicalContentLocator::ByMessageId {
            message_id: message_id.clone(),
        },
        offset,
        requested,
    )
}

pub(super) fn blocks_by_locator(
    connection: &Connection,
    conversation: &ConversationId,
    at: &ObservationReadCut,
    locator: &crate::runtime_client::content::CanonicalContentLocator,
    offset: u64,
    requested: usize,
) -> Result<crate::durable::observation::MessageBlockHeaderPage, ConversationStoreError> {
    validate(connection, conversation, at)?;
    let limit = limit(requested)?;
    let position = super::content::canonical_position(connection, locator)?;
    if position > at.ledger_through {
        return Err(ConversationStoreError::InvalidReference(
            "canonical block source beyond captured cut".into(),
        ));
    }
    let bytes:Vec<u8>=connection.query_row("SELECT h.header_json FROM observation_headers h WHERE h.reference_kind='message' AND h.ledger_position=?1",[position],|row|row.get(0)).map_err(|error|storage(error.to_string()))?;
    if bytes.len() > HEADER_ENCODED_MAX {
        return Err(storage("oversized canonical observation header"));
    }
    let header: MessageHeader =
        serde_json::from_slice(&bytes).map_err(|error| storage(error.to_string()))?;
    if offset > header.block_count {
        return Err(ConversationStoreError::InvalidReference(
            "canonical block offset beyond captured source".into(),
        ));
    }
    let mut statement=connection.prepare_cached("SELECT h.header_json FROM observation_block_headers h WHERE h.ledger_position=?1 AND h.block_index>=?2 ORDER BY h.block_index LIMIT ?3").map_err(|error|storage(error.to_string()))?;
    let rows = statement
        .query_map(params![position, seq_to_i64(offset)?, limit], |row| {
            row.get::<_, Vec<u8>>(0)
        })
        .map_err(|error| storage(error.to_string()))?;
    let mut headers = Vec::new();
    for row in rows {
        let bytes = row.map_err(|error| storage(error.to_string()))?;
        if bytes.len() > HEADER_ENCODED_MAX {
            return Err(storage("oversized canonical block header"));
        }
        headers.push(serde_json::from_slice(&bytes).map_err(|error| storage(error.to_string()))?);
    }
    let next = offset
        .checked_add(headers.len() as u64)
        .ok_or(ConversationStoreError::SequenceExhausted)?;
    if next == offset && offset < header.block_count {
        return Err(storage("canonical block descriptor index incomplete"));
    }
    Ok(crate::durable::observation::MessageBlockHeaderPage {
        cut: at.clone(),
        message_id: header.identity,
        locator: crate::runtime_client::content::CanonicalContentLocator::ByLedgerPosition {
            position,
        },
        total: header.block_count,
        offset,
        headers,
        next_offset: (next < header.block_count).then_some(next),
    })
}

pub(super) fn latest_events(
    connection: &Connection,
    through: u64,
    kinds: &[String],
    attempt: Option<&crate::runtime::identity::AttemptId>,
    requested: usize,
) -> Result<Vec<crate::durable::observation::ObservationEventHeader>, ConversationStoreError> {
    let limit = limit(requested)?;
    if kinds.len() > 16
        || kinds.iter().any(|kind| kind.len() > 128)
        || kinds
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != kinds.len()
    {
        return Err(ConversationStoreError::InvalidReference(
            "observation event family budget exceeded".into(),
        ));
    }
    // A bounded merge over indexed family heads: at most 16 initial seeks
    // plus 31 advancing seeks, never a full-Journal sort or body scan.
    let mut heads = Vec::new();
    for kind in kinds {
        if let Some(header) = latest_event(connection, through, None, kind, attempt)? {
            heads.push(header);
        }
    }
    let mut result = Vec::new();
    while !heads.is_empty() && result.len() < limit {
        let index = heads
            .iter()
            .enumerate()
            .max_by_key(|(_, header)| header.sequence)
            .map(|(index, _)| index)
            .expect("nonempty finite family heads");
        let header = heads.swap_remove(index);
        if result.len() + 1 < limit
            && let Some(next) = latest_event(
                connection,
                through,
                Some(header.sequence),
                &header.event_kind,
                attempt,
            )?
        {
            heads.push(next);
        }
        result.push(header);
    }
    result.reverse();
    Ok(result)
}
fn latest_event(
    connection: &Connection,
    through: u64,
    before: Option<u64>,
    kind: &str,
    attempt: Option<&crate::runtime::identity::AttemptId>,
) -> Result<Option<crate::durable::observation::ObservationEventHeader>, ConversationStoreError> {
    let bound = before.map(seq_to_i64).transpose()?.unwrap_or(i64::MAX);
    let (sql, parameters) = if let Some(attempt) = attempt {
        (
            "SELECT sequence,observation_json,observation_witness FROM events INDEXED BY events_attempt_kind_idx WHERE attempt_id=?1 AND json_extract(event_json,'$.event.type')=?2 AND sequence<=?3 AND sequence<?4 ORDER BY sequence DESC LIMIT 1",
            vec![
                rusqlite::types::Value::Text(attempt.to_string()),
                rusqlite::types::Value::Text(kind.into()),
                rusqlite::types::Value::Integer(seq_to_i64(through)?),
                rusqlite::types::Value::Integer(bound),
            ],
        )
    } else {
        (
            "SELECT sequence,observation_json,observation_witness FROM events INDEXED BY events_kind_idx WHERE json_extract(event_json,'$.event.type')=?1 AND sequence<=?2 AND sequence<?3 ORDER BY sequence DESC LIMIT 1",
            vec![
                rusqlite::types::Value::Text(kind.into()),
                rusqlite::types::Value::Integer(seq_to_i64(through)?),
                rusqlite::types::Value::Integer(bound),
            ],
        )
    };
    let row = connection
        .query_row(sql, rusqlite::params_from_iter(parameters), |row| {
            Ok((
                row.get::<_, u64>(0)?,
                row.get::<_, Option<Vec<u8>>>(1)?,
                row.get::<_, Option<Vec<u8>>>(2)?,
            ))
        })
        .optional()
        .map_err(|error| storage(error.to_string()))?;
    row.map(|(sequence, bytes, witness)| {
        let witness = witness
            .map(|bytes| {
                serde_json::from_slice::<crate::durable::observation::ObservationEventWitness>(
                    &bytes,
                )
                .map_err(|error| storage(error.to_string()))
            })
            .transpose()?;
        if bytes
            .as_ref()
            .is_some_and(|bytes| bytes.len() > HEADER_ENCODED_MAX)
        {
            return Err(storage("oversized native Journal header"));
        }
        let display = bytes
            .map(|bytes| serde_json::from_slice(&bytes).map_err(|error| storage(error.to_string())))
            .transpose()?;
        Ok(crate::durable::observation::ObservationEventHeader {
            sequence,
            event_id: witness
                .as_ref()
                .and_then(|witness| witness.event_id.clone()),
            event_kind: kind.into(),
            attempt_id: witness
                .as_ref()
                .and_then(|witness| witness.attempt_id.clone()),
            turn_id: witness.as_ref().and_then(|witness| witness.turn_id.clone()),
            deferred: display.is_none(),
            witness,
            display,
        })
    })
    .transpose()
}

pub(super) fn tools(
    connection: &Connection,
    conversation: &ConversationId,
    at: &ObservationReadCut,
    message_id: &crate::runtime::identity::MessageId,
    offset: u64,
    requested: usize,
) -> Result<crate::durable::observation::ToolAssociationHeaderPage, ConversationStoreError> {
    tools_by_locator(
        connection,
        conversation,
        at,
        &crate::runtime_client::content::CanonicalContentLocator::ByMessageId {
            message_id: message_id.clone(),
        },
        offset,
        requested,
    )
}

fn tool_owner(
    connection: &Connection,
    at: &ObservationReadCut,
    position: u64,
    offset: u64,
) -> Result<MessageHeader, ConversationStoreError> {
    let bytes:Vec<u8>=connection.query_row("SELECT header_json FROM observation_headers WHERE reference_kind='message' AND ledger_position=?1 AND journal_position<=?2",params![position,at.read.journal],|row|row.get(0)).map_err(|error|storage(error.to_string()))?;
    if bytes.len() > HEADER_ENCODED_MAX {
        return Err(storage("oversized canonical Tool owner header"));
    }
    let assistant: MessageHeader =
        serde_json::from_slice(&bytes).map_err(|error| storage(error.to_string()))?;
    if assistant.role != "assistant"
        || assistant
            .ledger_position
            .is_none_or(|position| position > at.ledger_through)
        || offset > assistant.block_count
    {
        return Err(ConversationStoreError::InvalidReference(
            "canonical Tool owner outside read cut".into(),
        ));
    }
    Ok(assistant)
}

pub(super) fn tools_by_locator(
    connection: &Connection,
    conversation: &ConversationId,
    at: &ObservationReadCut,
    locator: &crate::runtime_client::content::CanonicalContentLocator,
    offset: u64,
    requested: usize,
) -> Result<crate::durable::observation::ToolAssociationHeaderPage, ConversationStoreError> {
    validate(connection, conversation, at)?;
    let limit = limit(requested)?;
    let position = super::content::canonical_position(connection, locator)?;
    if position > at.ledger_through {
        return Err(ConversationStoreError::InvalidReference(
            "canonical Tool source beyond captured cut".into(),
        ));
    }
    let assistant = tool_owner(connection, at, position, offset)?;
    let mut statement=connection.prepare_cached("SELECT c.block_index,c.observation_call_id,c.observation_tool_id,b.header_json,h.header_json FROM canonical_tool_calls c INDEXED BY canonical_tool_calls_observation_idx JOIN observation_block_headers b ON b.ledger_position=c.observation_ledger AND b.block_index=c.block_index LEFT JOIN observation_headers h ON h.ledger_position=c.result_ledger_position AND h.transcript_position<=?5 AND h.ledger_position<=?3 AND h.journal_position<=?6 WHERE c.observation_ledger=?1 AND c.block_index>=?2 ORDER BY c.block_index LIMIT ?4").map_err(|error|storage(error.to_string()))?;
    let rows = statement
        .query_map(
            params![
                position,
                seq_to_i64(offset)?,
                seq_to_i64(at.ledger_through)?,
                limit + 1,
                seq_to_i64(at.read.transcript)?,
                seq_to_i64(at.read.journal)?
            ],
            |row| {
                Ok((
                    row.get::<_, u32>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                    row.get::<_, Option<Vec<u8>>>(4)?,
                ))
            },
        )
        .map_err(|error| storage(error.to_string()))?;
    let mut headers = Vec::new();
    let mut more = false;
    let mut remaining = 786_432usize - 1024;
    for row in rows {
        let (index, call_id, tool_id, call, result) =
            row.map_err(|error| storage(error.to_string()))?;
        if headers.len() == limit {
            more = true;
            break;
        }
        if call.len() > HEADER_ENCODED_MAX
            || result
                .as_ref()
                .is_some_and(|bytes| bytes.len() > HEADER_ENCODED_MAX)
        {
            return Err(storage("oversized canonical Tool header"));
        }
        let header = crate::durable::observation::ToolAssociationHeader {
            block_index: crate::message::types::ContentBlockIndex::new(index),
            call_id: call_id.map(crate::runtime::identity::ToolCallId::new),
            tool_id: tool_id.map(crate::runtime::identity::ToolId::new),
            call: serde_json::from_slice(&call).map_err(|error| storage(error.to_string()))?,
            result: result
                .map(|bytes| {
                    serde_json::from_slice(&bytes).map_err(|error| storage(error.to_string()))
                })
                .transpose()?,
        };
        let Some(encoded) = bounded_bytes(&header, remaining) else {
            more = true;
            break;
        };
        remaining -= encoded.len();
        headers.push(header);
    }
    let next_offset = more
        .then(|| {
            headers
                .last()
                .map(|header| u64::from(header.block_index.get()) + 1)
        })
        .flatten();
    if more && headers.is_empty() {
        return Err(storage(
            "canonical Tool descriptor does not fit finite page",
        ));
    }
    Ok(crate::durable::observation::ToolAssociationHeaderPage {
        cut: at.clone(),
        message_id: assistant.identity,
        locator: crate::runtime_client::content::CanonicalContentLocator::ByLedgerPosition {
            position,
        },
        total: assistant.tool_call_count,
        offset,
        headers,
        next_offset,
    })
}

pub(super) fn window(
    connection: &Connection,
    conversation: &ConversationId,
    at: &crate::durable::reading::ConversationWindowAt,
    requested: usize,
) -> Result<crate::durable::observation::ObservationWindow, ConversationStoreError> {
    let limit = limit(requested)?;
    let selected = super::reading::select_window(connection, conversation, at, limit)?;
    let mut captured = cut(connection, conversation)?;
    captured.ledger_through = selected.cut.ledger_through;
    captured.surface_revision = selected.cut.surface_revision;
    captured.transcript_total = selected.cut.transcript_total;
    captured.read = selected.cut;
    let upper = crate::durable::TranscriptCursor::new(
        captured
            .read
            .transcript
            .checked_add(1)
            .ok_or(ConversationStoreError::SequenceExhausted)?,
    );
    let mut page = transcript(
        connection,
        conversation,
        &captured,
        Some(selected.before.map_or(upper, |before| before.min(upper))),
        limit,
    )?;
    if let crate::durable::reading::ConversationWindowAt::Newer { after, .. } = at {
        page.headers.retain(|header| header.cursor > *after);
    }
    if let Some(cursor) = selected.target_cursor {
        page.headers.retain(|header| header.cursor >= cursor);
    }
    if let crate::durable::reading::ConversationWindowAt::Message { .. } = at
        && !page.headers.iter().any(|header| {
            Some(header.cursor) == selected.target_cursor && header.reference_kind == "message"
        })
    {
        return Err(ConversationStoreError::InvalidReference(
            "message is not readable at its native transcript position".into(),
        ));
    }
    if let Some(first) = page.headers.first() {
        let older: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM transcript_order WHERE position<?1)",
                [seq_to_i64(first.cursor.get())?],
                |row| row.get(0),
            )
            .map_err(|error| storage(error.to_string()))?;
        page.next_cursor = older.then_some(first.cursor);
    }
    let newer_cursor = if let Some(last) = page.headers.last() {
        let newer: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM transcript_order WHERE position>?1 AND position<=?2)",
                params![
                    seq_to_i64(last.cursor.get())?,
                    seq_to_i64(captured.read.transcript)?
                ],
                |row| row.get(0),
            )
            .map_err(|error| storage(error.to_string()))?;
        newer.then_some(last.cursor)
    } else {
        None
    };
    Ok(crate::durable::observation::ObservationWindow {
        cut: captured,
        page,
        newer_cursor,
        target: selected.observed_target,
        target_cursor: selected.target_cursor,
    })
}

pub(super) fn pending_page(
    connection: &Connection,
    conversation: &ConversationId,
    at: &crate::durable::reading::ConversationReadCut,
    after: Option<u64>,
    page_limit: usize,
) -> Result<crate::durable::observation::PendingHeaderPage, ConversationStoreError> {
    let page_limit = limit(page_limit)?;
    let actual = super::reading::cut(connection, conversation)?;
    if !at.reconstructible_from(&actual) {
        return Err(ConversationStoreError::ObservationCutExpired);
    }
    let total = at.pending_total;
    let mut statement=connection.prepare("SELECT p.sequence,p.revision,p.observation_header,p.observation_transcript FROM pending_inbound p WHERE p.sequence>?1 AND p.sequence<=?2 ORDER BY p.sequence LIMIT ?3").map_err(|error|storage(error.to_string()))?;
    let rows = statement
        .query_map(
            params![after.unwrap_or(0), at.pending_through, page_limit],
            |row| {
                Ok((
                    row.get::<_, u64>(0)?,
                    row.get::<_, u64>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, Option<u64>>(3)?,
                ))
            },
        )
        .map_err(|error| storage(error.to_string()))?;
    let mut headers = Vec::with_capacity(page_limit);
    for row in rows {
        let (sequence, revision, bytes, transcript_cursor) =
            row.map_err(|error| storage(error.to_string()))?;
        if bytes.len() > HEADER_ENCODED_MAX {
            return Err(storage("pending header exceeds bound"));
        }
        let mut message: MessageHeader =
            serde_json::from_slice(&bytes).map_err(|error| storage(error.to_string()))?;
        message.transcript_cursor = transcript_cursor.map(crate::durable::TranscriptCursor::new);
        headers.push(crate::durable::observation::PendingHeader {
            sequence,
            revision,
            message,
        });
    }
    let next_sequence = if let Some(last) = headers.last() {
        let more:bool=connection.query_row("SELECT EXISTS(SELECT 1 FROM pending_inbound p WHERE p.sequence>?1 AND p.sequence<=?2)",params![last.sequence,at.pending_through],|row|row.get(0)).map_err(|error|storage(error.to_string()))?;
        more.then_some(last.sequence)
    } else {
        None
    };
    Ok(crate::durable::observation::PendingHeaderPage {
        cut: at.clone(),
        total,
        headers,
        next_sequence,
    })
}
pub(super) fn pending_value(
    connection: &Connection,
    conversation: &ConversationId,
    at: &crate::durable::reading::ConversationReadCut,
    sequence: u64,
    offset: u64,
    max_bytes: u32,
) -> Result<crate::durable::observation::ObservationValueChunk, ConversationStoreError> {
    use crate::runtime_client::content::{CONTENT_READ_MAX_BYTES, CONTENT_SAFE_INTEGER_MAX};
    if offset > CONTENT_SAFE_INTEGER_MAX
        || sequence > CONTENT_SAFE_INTEGER_MAX
        || max_bytes == 0
        || max_bytes > CONTENT_READ_MAX_BYTES
    {
        return Err(ConversationStoreError::InvalidContentRange);
    }
    let actual = super::reading::cut(connection, conversation)?;
    if !at.reconstructible_from(&actual) {
        return Err(ConversationStoreError::ObservationCutExpired);
    }
    let retained:Option<(u64,u64)>=connection.query_row("SELECT p.revision,f.readable_end FROM pending_inbound p JOIN content_frontiers f ON f.source_kind='pending' AND f.source_id=CAST(p.sequence AS TEXT) AND f.block_index=0 AND f.representation='canonical_json_v1' AND f.revision=p.revision+1 WHERE p.sequence=?1 AND p.sequence<=?2",params![sequence,at.pending_through],|row|Ok((row.get(0)?,row.get(1)?))).optional().map_err(|error|storage(error.to_string()))?;
    let (_, readable_end) = retained
        .ok_or_else(|| ConversationStoreError::InvalidReference("pending source expired".into()))?;
    if offset > readable_end {
        return Err(ConversationStoreError::InvalidContentRange);
    }
    let end_offset = offset
        .checked_add(u64::from(max_bytes))
        .ok_or(ConversationStoreError::InvalidContentRange)?
        .min(readable_end);
    let bytes = super::content::pending_range_bytes(connection, sequence, offset, end_offset)?;
    Ok(crate::durable::observation::ObservationValueChunk {
        bytes,
        end_offset,
        readable_end,
        next_offset: (end_offset < readable_end).then_some(end_offset),
    })
}

/// Native C stamps the existing canonical association in its own Journal transaction.
pub(super) fn tool_occurrence(
    connection: &Connection,
    conversation: &ConversationId,
    call: &crate::runtime::identity::ToolCallId,
    attempt: &crate::runtime::identity::AttemptId,
    cut: &ObservationReadCut,
) -> Result<Option<crate::message::types::ToolCallOccurrenceRef>, ConversationStoreError> {
    validate(connection, conversation, cut)?;
    let value: Option<(String,u32)> = connection.query_row("SELECT c.assistant_message_id,c.block_index FROM canonical_tool_calls c INDEXED BY canonical_tool_calls_scope_idx CROSS JOIN message_ledger l ON l.message_id=c.assistant_message_id WHERE c.attempt_id=?1 AND c.call_id=?2 AND c.commit_sequence<=?3 AND l.position<=?4 ORDER BY c.commit_sequence DESC LIMIT 1",params![attempt.as_str(),call.as_str(),seq_to_i64(cut.read.journal)?,seq_to_i64(cut.ledger_through)?],|row|Ok((row.get(0)?,row.get(1)?))).optional().map_err(|error|storage(error.to_string()))?;
    Ok(value.map(|(message, index)| {
        crate::message::types::ToolCallOccurrenceRef::new(
            crate::runtime::identity::MessageId::new(message),
            crate::message::types::ContentBlockIndex::new(index),
        )
    }))
}

/// Resolves canonical ownership at the same append transaction as the Tool fact.
pub(super) fn tool_occurrence_receipt(
    connection: &Connection,
    event: &crate::events::types::RuntimeEventEnvelope,
) -> Result<Option<crate::events::types::ToolObservationOccurrence>, ConversationStoreError> {
    use crate::events::types::RuntimeEvent;
    let call = match &event.event {
        RuntimeEvent::ToolExecutionStarted { tool_call_id, .. }
        | RuntimeEvent::ToolExecutionProgress { tool_call_id, .. }
        | RuntimeEvent::ToolExecutionCompleted { tool_call_id, .. }
        | RuntimeEvent::ToolExecutionFailed { tool_call_id, .. } => tool_call_id,
        RuntimeEvent::ToolMessageCommitted { message_id, .. } => {
            let value:Option<(u64,u32,u64)>=connection.query_row("SELECT observation_ledger,block_index,observation_tool_ordinal FROM canonical_tool_calls INDEXED BY canonical_tool_calls_result_ledger_idx WHERE result_ledger_position=(SELECT position FROM message_ledger WHERE message_id=?1)",[message_id.as_str()],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional().map_err(|error|storage(error.to_string()))?;
            return Ok(value.map(|(ledger_position, index, tool_ordinal)| {
                crate::events::types::ToolObservationOccurrence {
                    tool_ordinal,
                    ledger_position,
                    block_index: crate::message::types::ContentBlockIndex::new(index),
                }
            }));
        }
        _ => return Ok(None),
    };
    let Some(attempt) = &event.attempt_id else {
        return Ok(None);
    };
    let value: Option<(u64, u32, u64)> = connection.query_row(
        "SELECT l.position,c.block_index,c.observation_tool_ordinal FROM canonical_tool_calls c INDEXED BY canonical_tool_calls_scope_idx CROSS JOIN message_ledger l ON l.message_id=c.assistant_message_id WHERE c.attempt_id=?1 AND c.call_id=?2 AND c.commit_sequence<=?3 ORDER BY c.commit_sequence DESC LIMIT 1",
        params![attempt.as_str(), call.as_str(), seq_to_i64(event.sequence)?],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    ).optional().map_err(|error| storage(error.to_string()))?;
    Ok(value.map(|(ledger_position, index, tool_ordinal)| {
        crate::events::types::ToolObservationOccurrence {
            tool_ordinal,
            ledger_position,
            block_index: crate::message::types::ContentBlockIndex::new(index),
        }
    }))
}

/// Real indexed Source pages at a native immutable Surface revision.
pub(super) fn surface_boundaries(
    connection: &Connection,
    through: crate::conversation::SurfaceRevision,
    offset: u64,
    requested: usize,
) -> Result<crate::durable::observation::SurfaceBoundaryHeaderPage, ConversationStoreError> {
    let limit = limit(requested)?;
    let actual: u64 = connection
        .query_row(
            "SELECT COALESCE((SELECT revision FROM surface_head WHERE id=1),0)",
            [],
            |row| row.get(0),
        )
        .map_err(|error| storage(error.to_string()))?;
    if through.get() > actual {
        return Err(ConversationStoreError::InvalidReference(
            "Surface boundary revision is newer than native head".into(),
        ));
    }
    let hole:bool=connection.query_row("SELECT EXISTS(SELECT 1 FROM message_ledger INDEXED BY message_ledger_boundary_holes_idx WHERE ordinary_user=1 AND boundary_indexed=0 AND first_append_revision IS NOT NULL AND first_append_revision<=?1)",[through.get()],|row|row.get(0)).map_err(|error|storage(error.to_string()))?;
    if hole {
        return Err(storage("native Surface boundary source layout incomplete"));
    }
    let total:u64=connection.query_row("SELECT COALESCE((SELECT ordinal+1 FROM surface_observation_boundaries WHERE surface_revision<=?1 ORDER BY surface_revision DESC LIMIT 1),0)",[seq_to_i64(through.get())?],|row|row.get(0)).map_err(|error|storage(error.to_string()))?;
    if offset > total {
        return Err(ConversationStoreError::InvalidReference(
            "Surface boundary offset beyond native frontier".into(),
        ));
    }
    let mut statement=connection.prepare("SELECT b.surface_revision,h.header_json,b.ordinal FROM surface_observation_boundaries b CROSS JOIN observation_headers h ON h.ledger_position=b.ledger_position WHERE b.ordinal>=?1 AND b.ordinal<?2 AND h.reference_kind='message' ORDER BY b.ordinal LIMIT ?3").map_err(|error|storage(error.to_string()))?;
    let rows = statement
        .query_map(params![offset, total, limit], |row| {
            Ok((
                row.get::<_, u64>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, u64>(2)?,
            ))
        })
        .map_err(|error| storage(error.to_string()))?;
    let mut boundaries = Vec::new();
    for row in rows {
        let (revision, bytes, ordinal) = row.map_err(|error| storage(error.to_string()))?;
        if ordinal != offset + boundaries.len() as u64 {
            return Err(storage("native Surface boundary source header missing"));
        }
        if bytes.len() > HEADER_ENCODED_MAX {
            return Err(storage("oversized native Surface boundary header"));
        }
        boundaries.push(crate::durable::observation::SurfaceBoundaryHeader {
            surface_revision: crate::conversation::SurfaceRevision::new(revision),
            header: serde_json::from_slice(&bytes).map_err(|error| storage(error.to_string()))?,
        });
    }
    let next = offset
        .checked_add(boundaries.len() as u64)
        .ok_or(ConversationStoreError::SequenceExhausted)?;
    if boundaries.len() as u64 != (total - offset).min(limit as u64) {
        return Err(storage("native Surface boundary index incomplete"));
    }
    Ok(crate::durable::observation::SurfaceBoundaryHeaderPage {
        through,
        total,
        boundaries,
        next_offset: (next < total).then_some(next),
    })
}

/// Same native pending transaction stores a finite read header ahead of the original body.
pub(super) fn index_pending_header(
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

pub(super) fn index_audit(
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
            item: crate::runtime_client::snapshot::RuntimeClientTranscriptItem::PublicationAudit {
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
            crate::runtime_client::snapshot::RuntimeClientPublicationSettlementRecord {
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

/// Reads native audit positions first, so absent derived evidence cannot silently shrink the head.
pub(super) fn publication_settlement_head(
    connection: &Connection,
    conversation: &ConversationId,
    at: &ObservationReadCut,
) -> Result<
    Vec<crate::runtime_client::snapshot::RuntimeClientPublicationSettlementRecord>,
    ConversationStoreError,
> {
    validate(connection, conversation, at)?;
    let mut statement = connection.prepare_cached("SELECT t.position,h.header_json,h.journal_position FROM transcript_order t INDEXED BY transcript_order_audit_position_idx LEFT JOIN observation_headers h ON h.reference_kind=t.reference_kind AND h.reference_id=t.reference_id WHERE t.reference_kind='publication_audit' AND t.position<=?1 ORDER BY t.position DESC LIMIT 32").map_err(|error| storage(error.to_string()))?;
    let rows = statement
        .query_map([seq_to_i64(at.read.transcript)?], |row| {
            Ok((
                row.get::<_, u64>(0)?,
                row.get::<_, Option<Vec<u8>>>(1)?,
                row.get::<_, Option<u64>>(2)?,
            ))
        })
        .map_err(|error| storage(error.to_string()))?;
    let mut records = Vec::with_capacity(HEADER_PAGE_MAX);
    for row in rows {
        let (position, bytes, journal) = row.map_err(|error| storage(error.to_string()))?;
        let bytes = bytes.ok_or_else(|| storage("native audit settlement header unavailable"))?;
        if journal.is_none_or(|journal| journal > at.read.journal) {
            return Err(storage("audit header is outside published native cut"));
        }
        let header: TranscriptHeader =
            serde_json::from_slice(&bytes).map_err(|error| storage(error.to_string()))?;
        let record = header
            .publication_settlement
            .ok_or_else(|| storage("native positive audit settlement evidence unavailable"))?;
        if record.transcript_cursor.get() != position {
            return Err(storage("native audit settlement placement mismatch"));
        }
        records.push(record);
    }
    records.reverse();
    Ok(records)
}

pub(super) fn audit_blocks(
    connection: &Connection,
    conversation: &ConversationId,
    at: &ObservationReadCut,
    transcript: u64,
    offset: u64,
    requested: usize,
) -> Result<crate::durable::observation::AuditBlockHeaderPage, ConversationStoreError> {
    validate(connection, conversation, at)?;
    let limit = limit(requested)?;
    if transcript > at.read.transcript {
        return Err(storage("audit position exceeds native cut"));
    }
    let bytes:Vec<u8>=connection.query_row("SELECT header_json FROM observation_headers INDEXED BY observation_headers_transcript_idx WHERE transcript_position=?1 AND reference_kind='publication_audit' AND journal_position<=?2",params![transcript,at.read.journal],|row|row.get(0)).map_err(|error|storage(error.to_string()))?;
    let header: TranscriptHeader =
        serde_json::from_slice(&bytes).map_err(|error| storage(error.to_string()))?;
    let total = header
        .publication_settlement
        .ok_or_else(|| storage("audit settlement source unavailable"))?
        .settlement
        .block_count;
    if offset > total {
        return Err(storage("audit block offset exceeds source"));
    }
    let mut statement=connection.prepare_cached("SELECT header_json FROM audit_observation_blocks WHERE transcript_position=?1 AND ordinal>=?2 ORDER BY ordinal LIMIT ?3").map_err(|error|storage(error.to_string()))?;
    let rows = statement
        .query_map(params![transcript, offset, limit], |row| {
            row.get::<_, Vec<u8>>(0)
        })
        .map_err(|error| storage(error.to_string()))?;
    let headers = rows
        .map(|row| {
            let bytes = row.map_err(|error| storage(error.to_string()))?;
            serde_json::from_slice(&bytes).map_err(|error| storage(error.to_string()))
        })
        .collect::<Result<Vec<_>, ConversationStoreError>>()?;
    let next = offset + headers.len() as u64;
    if headers.len() as u64 != (total - offset).min(limit as u64) {
        return Err(storage("audit block source layout incomplete"));
    }
    Ok(crate::durable::observation::AuditBlockHeaderPage {
        cut: at.clone(),
        total,
        offset,
        headers,
        next_offset: (next < total).then_some(next),
    })
}

pub(super) fn audit_summary(
    audit: &crate::publication::PublicationAudit,
) -> crate::durable::inbox::PublicationAuditSummary {
    crate::durable::inbox::PublicationAuditSummary {
        stream_id: (audit.stream_id.as_str().len() <= 256).then(|| audit.stream_id.clone()),
        message_id: (audit.message_id.as_str().len() <= 256).then(|| audit.message_id.clone()),
        attempt_id: (audit.attempt_id.as_str().len() <= 256).then(|| audit.attempt_id.clone()),
        turn_id: (audit.turn_id.as_str().len() <= 256).then(|| audit.turn_id.clone()),
        request_id: (audit.request_id.as_str().len() <= 256).then(|| audit.request_id.clone()),
        kind: audit.kind,
        settled_at: audit.settled_at,
        block_count: audit.content.len() as u64,
    }
}

fn audit_metadata_bindings(
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
            let source = crate::runtime_client::content::ContentSource::DurableTranscriptAudit {
                component:
                    crate::runtime_client::content::PublicationContentComponent::BlockMetadata,
                stream_id: audit.stream_id.clone(),
                message_id: audit.message_id.clone(),
                attempt_id: audit.attempt_id.clone(),
                turn_id: audit.turn_id.clone(),
                publication_request_id: audit.request_id.clone(),
                block_index: *block_index,
            };
            if let Some(reference) = super::content::descriptor(
                connection,
                conversation,
                &source,
                crate::runtime_client::content::ContentRepresentation::CanonicalJsonV1,
            )? {
                capture
                    .content
                    .push(crate::runtime_client::checkpoint::ContentBinding {
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

fn index_audit_blocks(
    connection: &Connection,
    audit: &crate::publication::PublicationAudit,
    conversation: &ConversationId,
    cursor: crate::durable::TranscriptCursor,
) -> Result<(), ConversationStoreError> {
    for (ordinal, block) in audit.content.iter().enumerate() {
        use crate::publication::PublicationAuditBlock as Block;
        use crate::runtime_client::content::{
            ContentRepresentation as Rep, ContentSource, PublicationContentComponent as Component,
        };
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
        let value = crate::runtime_client::checkpoint::bounded_item_bytes(block, 1024)
            .and_then(|_| crate::runtime_client::checkpoint::structural_value(block));
        let deferred = value.is_none();
        let mut content = Vec::new();
        if deferred {
            if let Some(reference) = super::content::descriptor(
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
            if let Some(reference) = super::content::descriptor(
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

fn canonical_bindings(
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
            super::content::descriptor(
                connection,
                conversation,
                &binding.content_ref.source,
                crate::runtime_client::content::ContentRepresentation::RawJsonUtf8,
            )?
        } else {
            None
        };
        if let Some(reference) = raw.or(super::content::descriptor(
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
