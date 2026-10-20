//! Same-C summaries capture borrowed native fields; reads never hydrate bodies.
use super::{
    Connection, ConversationStoreError, HEADER_ENCODED_MAX, TraceCanonicalSummary, TraceFieldKind,
    TraceFieldSource, TraceToolProposalSummary, encode, params, storage,
};
use crate::message::types::{
    AssistantContentBlock as A, ContextKind, InboundKind, MessageBlock, UserContentBlock as U,
};
use crate::observation::content::{
    CanonicalContentComponent as Component, CanonicalContentLocator, ContentRepresentation as Rep,
    ContentSource,
};
use crate::observation::trace::{TraceArtifact, TraceContextKind, TracePreview, TraceToolCall};

fn binding(
    connection: &Connection,
    conversation: &crate::runtime::identity::ConversationId,
    position: u64,
    block: u32,
    component: Component,
    field: TraceFieldKind,
    path: &[&str],
) -> Result<TraceFieldSource, ConversationStoreError> {
    let source = ContentSource::CanonicalMessageBlock {
        component,
        locator: CanonicalContentLocator::ByLedgerPosition { position },
        block_index: crate::message::types::ContentBlockIndex::new(block),
    };
    let raw = if component == Component::Payload && field == TraceFieldKind::Arguments {
        super::super::content::descriptor(connection, conversation, &source, Rep::RawJsonUtf8)?
    } else {
        None
    };
    let reference = match raw {
        Some(reference) => reference,
        None => super::super::content::descriptor(
            connection,
            conversation,
            &source,
            Rep::CanonicalJsonV1,
        )?
        .ok_or_else(|| storage("original canonical Trace field unavailable"))?,
    };
    Ok(TraceFieldSource::Content {
        field,
        binding: crate::observation::cut::ContentBinding {
            path: Vec::new(),
            source_path: Some(path.iter().map(|part| (*part).into()).collect()),
            content_ref: reference,
            deferred: true,
        },
    })
}
fn json_message_preview(value: &serde_json::Value) -> TracePreview {
    struct Prefix(Vec<u8>);
    impl std::io::Write for Prefix {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            let kept = bytes.len().min(4096 - self.0.len());
            self.0.extend_from_slice(&bytes[..kept]);
            if kept < bytes.len() {
                Err(std::io::Error::other("preview prefix"))
            } else {
                Ok(kept)
            }
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut prefix = Prefix(Vec::with_capacity(4096));
    let truncated = serde_json::to_writer(&mut prefix, value).is_err();
    let valid = std::str::from_utf8(&prefix.0).map_or_else(|error| error.valid_up_to(), str::len);
    let mut preview =
        TracePreview::of(std::str::from_utf8(&prefix.0[..valid]).expect("valid original prefix"));
    preview.truncated |= truncated;
    preview
}
fn preview(message: &MessageBlock) -> Option<TracePreview> {
    let value = match message {
        MessageBlock::User(user) => user.content.iter().find_map(|block| match block {
            U::Text(text) => Some(TracePreview::of(&text.text)),
            U::UploadedFile(upload) => Some(TracePreview::of(&upload.name)),
            _ => None,
        }),
        MessageBlock::Assistant(assistant) => assistant
            .content
            .iter()
            .find_map(|block| match block {
                A::Text(text) => Some(TracePreview::of(&text.text)),
                A::Refusal(text) => Some(TracePreview::of(&text.text)),
                _ => None,
            })
            .or_else(|| {
                assistant.content.iter().find_map(|block| {
                    if let A::Reasoning(reasoning) = block {
                        reasoning.text.as_deref().map(TracePreview::of)
                    } else {
                        None
                    }
                })
            }),
        MessageBlock::Tool(tool) => tool
            .result
            .content
            .iter()
            .find_map(|block| match block {
                crate::tools::types::ToolResultContent::Text(text) => {
                    Some(TracePreview::of(&text.text))
                }
                crate::tools::types::ToolResultContent::Json { value } => {
                    Some(json_message_preview(value))
                }
                _ => None,
            })
            .or_else(|| super::event::tool_status(&tool.result.status).1),
    }?;
    (!value.is_empty()).then_some(value)
}
fn artifact_file(file: &crate::message::content::FileReference) -> Option<TraceArtifact> {
    (file.artifact_id.as_str().len() <= 512).then(|| TraceArtifact {
        artifact_id: file.artifact_id.clone(),
        image: false,
        name: file.name.as_ref().filter(|name| name.len() <= 512).cloned(),
        mime_type: file
            .mime_type
            .as_ref()
            .filter(|name| name.len() <= 512)
            .cloned(),
    })
}
fn artifacts(message: &MessageBlock) -> (Vec<TraceArtifact>, bool) {
    let mut result = Vec::with_capacity(32);
    let mut truncated = false;
    let mut push = |item: Option<TraceArtifact>| {
        if let Some(item) = item {
            if !result
                .iter()
                .any(|old: &TraceArtifact| old.artifact_id == item.artifact_id)
            {
                if result.len() < 32 {
                    result.push(item);
                } else {
                    truncated = true;
                }
            }
        } else {
            truncated = true;
        }
    };
    let image = |image: &crate::message::content::ImageReference| {
        (image.artifact_id.as_str().len() <= 512).then(|| TraceArtifact {
            artifact_id: image.artifact_id.clone(),
            image: true,
            name: None,
            mime_type: None,
        })
    };
    match message {
        MessageBlock::User(user) => {
            for block in &user.content {
                match block {
                    U::Image(value) => push(image(value)),
                    U::File(value) => push(artifact_file(value)),
                    _ => {}
                }
            }
        }
        MessageBlock::Assistant(assistant) => {
            for block in &assistant.content {
                if let A::Image(value) = block {
                    push(image(value));
                }
            }
        }
        MessageBlock::Tool(tool) => {
            for block in tool.result.content.iter().take(32) {
                match block {
                    crate::tools::types::ToolResultContent::Image(value) => push(image(value)),
                    crate::tools::types::ToolResultContent::File(value) => {
                        push(artifact_file(value));
                    }
                    _ => {}
                }
            }
            for file in tool.result.artifacts.iter().take(32) {
                push(artifact_file(file));
            }
            truncated |= tool.result.content.len() > 32 || tool.result.artifacts.len() > 32;
        }
    }
    (result, truncated)
}
fn context_kind(message: &MessageBlock) -> Option<TraceContextKind> {
    let MessageBlock::User(user) = message else {
        return None;
    };
    match &user.kind {
        InboundKind::Context(ContextKind::NativeEnvironment) => {
            Some(TraceContextKind::NativeEnvironment)
        }
        InboundKind::Context(ContextKind::GoalStatus(_)) => Some(TraceContextKind::GoalStatus),
        InboundKind::Context(ContextKind::RuntimeToolObservation) => {
            Some(TraceContextKind::RuntimeToolObservation)
        }
        InboundKind::Context(ContextKind::ExtensionEnvironment) => {
            Some(TraceContextKind::ExtensionEnvironment)
        }
        InboundKind::Context(ContextKind::AgentStatus(_)) => Some(TraceContextKind::AgentStatus),
        _ => None,
    }
}
fn proposal_from(
    connection: &Connection,
    conversation: &crate::runtime::identity::ConversationId,
    position: u64,
    index: u32,
    ordinal: u64,
    call: &crate::tools::types::ToolCall,
) -> Result<TraceToolProposalSummary, ConversationStoreError> {
    let mut fields = vec![binding(
        connection,
        conversation,
        position,
        index,
        Component::Payload,
        TraceFieldKind::Arguments,
        &[],
    )?];
    for (kind, path, value) in [
        (TraceFieldKind::CallId, "id", call.id.as_str()),
        (TraceFieldKind::ToolId, "tool_id", call.tool_id.as_str()),
        (TraceFieldKind::Name, "name", call.name.as_str()),
    ] {
        if value.len() > 512 {
            fields.push(binding(
                connection,
                conversation,
                position,
                index,
                Component::BlockMetadata,
                kind,
                &[path],
            )?);
        }
    }
    Ok(TraceToolProposalSummary {
        occurrence: crate::events::types::ToolObservationOccurrence {
            tool_ordinal: ordinal,
            ledger_position: position,
            block_index: crate::message::types::ContentBlockIndex::new(index),
        },
        call_id: (call.id.as_str().len() <= 512).then(|| call.id.clone()),
        tool_id: (call.tool_id.as_str().len() <= 512).then(|| call.tool_id.clone()),
        name: (call.name.len() <= 512).then(|| call.name.clone()),
        arguments: Some(TracePreview::of_json(&call.arguments)),
        fields,
    })
}
pub(in crate::durable::sqlite) fn index_proposals(
    connection: &Connection,
    conversation: &crate::runtime::identity::ConversationId,
    position: u64,
    message: &MessageBlock,
) -> Result<(), ConversationStoreError> {
    let MessageBlock::Assistant(assistant) = message else {
        return Ok(());
    };
    let mut ordinal = 0;
    for (index, block) in assistant.content.iter().enumerate() {
        if let A::ToolCall(call) = block {
            let index =
                u32::try_from(index).map_err(|_| storage("native proposal block exhausted"))?;
            let summary = proposal_from(connection, conversation, position, index, ordinal, call)?;
            connection.execute("UPDATE canonical_tool_calls SET observation_trace_proposal=?3,observation_tool_identity_digest=?4 WHERE observation_ledger=?1 AND block_index=?2",params![position,index,encode(&summary)?,super::inspection::tool_identity_digest(&call.tool_id).as_slice()]).map_err(|error|storage(error.to_string()))?;
            ordinal += 1;
        }
    }
    Ok(())
}
pub(in crate::durable::sqlite) fn index_canonical(
    connection: &Connection,
    conversation: &crate::runtime::identity::ConversationId,
    position: u64,
    message: &MessageBlock,
) -> Result<(), ConversationStoreError> {
    let (attachments, truncated) = artifacts(message);
    let mut summary = TraceCanonicalSummary {
        source: if let MessageBlock::User(user) = message {
            Some(user.source.clone())
        } else {
            None
        },
        ordinary_input: matches!(message,MessageBlock::User(user) if user.kind==InboundKind::Message),
        context_kind: context_kind(message),
        locator: CanonicalContentLocator::ByLedgerPosition { position },
        identity: (message.id().as_str().len() <= 512).then(|| message.id().clone()),
        preview: preview(message),
        calls: Vec::new(),
        attachments,
        tool_outcome: None,
        tool_detail: None,
        truncated,
        fields: Vec::new(),
        collections: Vec::new(),
    };
    if summary.identity.is_none() {
        summary.fields.push(binding(
            connection,
            conversation,
            position,
            0,
            Component::MessageMetadata,
            TraceFieldKind::MessageId,
            &["id"],
        )?);
    }
    append_blocks_frontier(position, message, &mut summary);
    if let MessageBlock::Tool(tool) = message {
        let (outcome, detail) = super::event::tool_status(&tool.result.status);
        summary.tool_outcome = Some(outcome);
        summary.tool_detail = detail;
        summary.fields.push(binding(
            connection,
            conversation,
            position,
            0,
            Component::MessageMetadata,
            TraceFieldKind::Result,
            &["result"],
        )?);
    }
    if let MessageBlock::Assistant(assistant) = message {
        let mut total = 0;
        let mut next = None;
        for (index, block) in assistant.content.iter().enumerate() {
            if let A::ToolCall(call) = block {
                let index =
                    u32::try_from(index).map_err(|_| storage("native proposal block exhausted"))?;
                if summary.calls.len() < 32 {
                    let proposal =
                        proposal_from(connection, conversation, position, index, total, call)?;
                    summary.calls.push(TraceToolCall {
                        call_id: proposal.call_id,
                        tool_id: proposal.tool_id,
                        name: proposal.name,
                        occurrence: crate::observation::tool::RuntimeClientToolOccurrence {
                            locator: summary.locator.clone(),
                            block_index: proposal.occurrence.block_index,
                        },
                        fields: proposal.fields,
                    });
                } else if next.is_none() {
                    next = Some(u64::from(index));
                }
                total += 1;
            }
        }
        summary
            .collections
            .push(crate::observation::cut::CollectionFrontier {
                domain: format!("messages.ledger:{position}.tools"),
                revision: 1,
                total,
                loaded: summary.calls.len() as u64,
                next_offset: next,
            });
    }
    seal_canonical(connection, position, &mut summary)
}

fn seal_canonical(
    connection: &Connection,
    position: u64,
    summary: &mut TraceCanonicalSummary,
) -> Result<(), ConversationStoreError> {
    while crate::observation::bounded::bounded_bytes(summary, HEADER_ENCODED_MAX).is_none() {
        summary.truncated = true;
        if let Some(call) = summary.calls.pop() {
            let frontier = summary
                .collections
                .iter_mut()
                .find(|frontier| frontier.domain == format!("messages.ledger:{position}.tools"))
                .expect("native calls frontier");
            frontier.loaded = summary.calls.len() as u64;
            frontier.next_offset = Some(u64::from(call.occurrence.block_index.get()));
            continue;
        }
        if summary.attachments.pop().is_some() {
            continue;
        }
        if summary.preview.take().is_some() {
            continue;
        }
        return Err(storage("native canonical Trace descriptor cannot fit"));
    }
    connection
        .execute(
            "UPDATE message_ledger SET observation_trace_summary=?2 WHERE position=?1",
            params![position, encode(summary)?],
        )
        .map_err(|error| storage(error.to_string()))?;
    Ok(())
}

fn append_blocks_frontier(
    position: u64,
    message: &MessageBlock,
    summary: &mut TraceCanonicalSummary,
) {
    let blocks = match message {
        MessageBlock::User(user) => user.content.len(),
        MessageBlock::Assistant(assistant) => assistant.content.len(),
        MessageBlock::Tool(tool) => tool.result.content.len(),
    };
    summary
        .collections
        .push(crate::observation::cut::CollectionFrontier {
            domain: format!("messages.ledger:{position}.blocks"),
            revision: 1,
            total: blocks as u64,
            loaded: 0,
            next_offset: (blocks > 0).then_some(0),
        });
}
