//! Destination-identity remapping of a frozen lineage seed (the durable
//! projection Session lineage publication replays).
use crate::store::canonical::LineageSeed;

use rustx_conversation::{SurfaceOp, message_id_of};
use rustx_types::message::types::{
    AssistantMessageBlock, MessageBlock, ToolMessageBlock, UserMessageBlock,
};
use rustx_types::runtime::identity::{ConversationId, MessageId};
use std::collections::BTreeMap;

/// A lineage seed could not be remapped into destination identities.
#[derive(Debug, thiserror::Error)]
#[error("{detail}")]
pub struct LineageRemapError {
    detail: String,
}

pub fn remap_seed(
    destination: &ConversationId,
    canonical: &[MessageBlock],
    surface_history: &[SurfaceOp],
    completed_responses: &[crate::response::CompletedResponseProvenance],
    turns: &[crate::reading::TurnReadingProvenance],
    raw_tool_arguments: &[crate::CanonicalRawToolArguments],
) -> Result<LineageSeed, LineageRemapError> {
    let messages = canonical;
    let mut message_ids = BTreeMap::new();
    for (index, message) in messages.iter().enumerate() {
        let old = message_id_of(message);
        let new = MessageId::new(format!("{destination}-message-{}", index + 1));
        if message_ids.insert(old.clone(), new).is_some() {
            return Err(LineageRemapError {
                detail: format!("source seed repeats MessageId {old}"),
            });
        }
    }

    let canonical = messages
        .iter()
        .map(|message| remap_message(message, &message_ids))
        .collect::<Result<Vec<_>, LineageRemapError>>()?;
    let seeded = |id: &MessageId| {
        message_ids
            .get(id)
            .cloned()
            .ok_or_else(|| LineageRemapError {
                detail: format!(
                    "the seeded Surface history names {id}, which the seeded Ledger does not carry"
                ),
            })
    };
    let surface_history = surface_history
        .iter()
        .map(|operation| match operation {
            SurfaceOp::Append { message_id } => Ok(SurfaceOp::Append {
                message_id: seeded(message_id)?,
            }),
        })
        .collect::<Result<Vec<_>, LineageRemapError>>()?;
    let responses = completed_responses
        .iter()
        .filter_map(|response| {
            let closing = message_ids.get(&response.closing_message_id)?;
            let mut response = response.clone();
            response.closing_message_id = closing.clone();
            response.process_message_ids = response
                .process_message_ids
                .iter()
                .filter_map(|id| message_ids.get(id).cloned())
                .collect();
            response.retry_message_id = response
                .retry_message_id
                .as_ref()
                .and_then(|input| message_ids.get(input).cloned());
            Some(response)
        })
        .collect();
    let turns = turns
        .iter()
        .filter_map(|turn| {
            let mut copied = turn.clone();
            copied.process_message_ids = turn
                .process_message_ids
                .iter()
                .filter_map(|id| message_ids.get(id).cloned())
                .collect();
            if !turn.process_message_ids.is_empty() && copied.process_message_ids.is_empty() {
                return None;
            }
            copied.preceding_message_id = turn
                .preceding_message_id
                .as_ref()
                .and_then(|id| message_ids.get(id).cloned());
            copied.prompt_message_id = turn
                .prompt_message_id
                .as_ref()
                .and_then(|id| message_ids.get(id).cloned());
            Some(copied)
        })
        .collect();
    let raw = remap_raw_tool_arguments(raw_tool_arguments, &message_ids);
    LineageSeed::replayed(canonical, surface_history)
        .and_then(|seed| seed.with_completed_responses(responses))
        .and_then(|seed| seed.with_turns(turns))
        .and_then(|seed| seed.with_raw_tool_arguments(raw))
        .map_err(|error| LineageRemapError {
            detail: error.to_string(),
        })
}

fn remap_raw_tool_arguments(
    raw_tool_arguments: &[crate::CanonicalRawToolArguments],
    message_ids: &BTreeMap<MessageId, MessageId>,
) -> Vec<crate::CanonicalRawToolArguments> {
    raw_tool_arguments
        .iter()
        .filter_map(|item| {
            message_ids
                .get(&item.message_id)
                .map(|id| crate::CanonicalRawToolArguments {
                    message_id: id.clone(),
                    block_index: item.block_index,
                    bytes: item.bytes.clone(),
                })
        })
        .collect()
}

fn remap_message(
    message: &MessageBlock,
    message_ids: &BTreeMap<MessageId, MessageId>,
) -> Result<MessageBlock, LineageRemapError> {
    let message_id = |id: &MessageId| {
        message_ids
            .get(id)
            .cloned()
            .ok_or_else(|| LineageRemapError {
                detail: format!("source seed references unknown MessageId {id}"),
            })
    };

    match message {
        MessageBlock::User(user) => Ok(MessageBlock::User(UserMessageBlock {
            id: message_id(&user.id)?,
            content: user.content.clone(),
            source: user.source.clone(),
            kind: user.kind.clone(),
            timestamp: user.timestamp,
        })),
        MessageBlock::Assistant(assistant) => Ok(MessageBlock::Assistant(AssistantMessageBlock {
            id: message_id(&assistant.id)?,
            // Copied calls are history, not newly issued provider invocations.
            // Preserve the provider correlation on both sides of each occurrence.
            content: assistant.content.clone(),
        })),
        MessageBlock::Tool(tool) => Ok(MessageBlock::Tool(ToolMessageBlock {
            id: message_id(&tool.id)?,
            occurrence: rustx_types::message::types::ToolCallOccurrenceRef {
                assistant_message_id: message_id(&tool.occurrence.assistant_message_id)?,
                block_index: tool.occurrence.block_index,
            },
            tool_call_id: tool.tool_call_id.clone(),
            tool_id: tool.tool_id.clone(),
            result: tool.result.clone(),
        })),
    }
}
