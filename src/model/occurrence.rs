//! Frozen actual-request coordinates and provider correlation labels.
//! These are model-input relationships; they never authorize or record Tool execution.
use crate::context::checkpoint::{RequestBlockIndex, RequestMessageOccurrence};
use crate::message::types::{AssistantContentBlock, ContentBlockIndex};
use crate::model::projected::{CanonicalMessageRef, ModelMessageView};
use crate::runtime::identity::RequestId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(transparent)]
pub struct ProviderWireCorrelationId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CanonicalCallSource {
    pub assistant: CanonicalMessageRef,
    pub block: ContentBlockIndex,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ActualRequestCallCoordinate {
    pub message: RequestMessageOccurrence,
    pub block: RequestBlockIndex,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ActualRequestToolAssociation {
    pub call: ActualRequestCallCoordinate,
    pub result: RequestMessageOccurrence,
    pub source: CanonicalCallSource,
    pub wire_id: ProviderWireCorrelationId,
}
/// An intermediate coordinate in the detached compiled historical sequence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoricalToolAssociation {
    pub call_message: u32,
    pub call_block: u32,
    pub result_message: u32,
    pub source: CanonicalCallSource,
    pub wire_id: ProviderWireCorrelationId,
}

pub(crate) fn freeze_associations(
    history: &crate::durable::projection::HistoricalModelInput,
    request_id: &RequestId,
    request: &crate::model::ModelRequest,
    inserted_at: Option<usize>,
) -> Result<Vec<ActualRequestToolAssociation>, String> {
    let position = |index: u32| -> Result<u32, String> {
        if inserted_at.is_some_and(|at| index as usize >= at) {
            index
                .checked_add(1)
                .ok_or_else(|| "request occurrence overflow".into())
        } else {
            Ok(index)
        }
    };
    let mut associations = history
        .tool_associations
        .iter()
        .map(|association| {
            Ok(ActualRequestToolAssociation {
                call: ActualRequestCallCoordinate {
                    message: RequestMessageOccurrence {
                        request_id: request_id.clone(),
                        item_index: position(association.call_message)?,
                    },
                    block: RequestBlockIndex(association.call_block),
                },
                result: RequestMessageOccurrence {
                    request_id: request_id.clone(),
                    item_index: position(association.result_message)?,
                },
                source: association.source.clone(),
                wire_id: association.wire_id.clone(),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    install_wire_labels(&request.messages, &mut associations);
    validate_associations(request_id, &request.messages, &associations)?;
    Ok(associations)
}

#[allow(clippy::too_many_lines)] // One complete request coordinate and Tool-batch validation fold.
pub(crate) fn validate_associations(
    request_id: &RequestId,
    messages: &[crate::model::ModelInputMessage],
    associations: &[ActualRequestToolAssociation],
) -> Result<(), String> {
    let labels = wire_map(messages);
    let mut calls = std::collections::BTreeSet::new();
    let mut results = std::collections::BTreeSet::new();
    for association in associations {
        if association.call.message.request_id != *request_id
            || association.result.request_id != *request_id
            || association.result.item_index <= association.call.message.item_index
        {
            return Err("foreign or reordered actual-request Tool association".into());
        }
        if !calls.insert((
            association.call.message.item_index,
            association.call.block.0,
        )) || !results.insert(association.result.item_index)
        {
            return Err("duplicate actual-request Tool association".into());
        }
        let Some(ModelMessageView::Assistant(content)) = messages
            .get(association.call.message.item_index as usize)
            .map(|m| m.view())
        else {
            return Err("actual request call message is absent".into());
        };
        let Some(AssistantContentBlock::ToolCall(call)) =
            content.get(association.call.block.0 as usize)
        else {
            return Err("actual request call block is absent".into());
        };
        let Some(ModelMessageView::Tool(id, _)) = messages
            .get(association.result.item_index as usize)
            .map(|m| m.view())
        else {
            return Err("actual request result message is absent".into());
        };
        if &call.id != id
            || labels.get(&(
                association.call.message.item_index,
                association.call.block.0,
            )) != Some(&association.wire_id.0)
        {
            return Err("actual request wire correlation disagrees with frozen association".into());
        }
    }
    for (index, message) in messages.iter().enumerate() {
        let index = u32::try_from(index).map_err(|_| "request message occurrence overflow")?;
        match message.view() {
            ModelMessageView::Assistant(content) => {
                for (block, content) in content.iter().enumerate() {
                    if matches!(content, AssistantContentBlock::ToolCall(_))
                        && !calls.contains(&(
                            index,
                            u32::try_from(block)
                                .map_err(|_| "request block occurrence overflow")?,
                        ))
                    {
                        return Err("unassociated actual-request ToolCall".into());
                    }
                }
            }
            ModelMessageView::Tool(_, _) if !results.contains(&index) => {
                return Err("orphan actual-request ToolResult".into());
            }
            _ => {}
        }
    }
    let mut pending = std::collections::BTreeSet::new();
    for (index, message) in messages.iter().enumerate() {
        let index = u32::try_from(index).map_err(|_| "request occurrence overflow")?;
        match message.view() {
            ModelMessageView::Tool(_, _) => {
                let association = associations
                    .iter()
                    .find(|a| a.result.item_index == index)
                    .ok_or("orphan request result")?;
                if !pending.remove(&(
                    association.call.message.item_index,
                    association.call.block.0,
                )) {
                    return Err("request ToolResult crosses a preserved batch".into());
                }
            }
            ModelMessageView::Assistant(content) => {
                if !pending.is_empty() {
                    return Err("request Assistant splits an unresolved Tool batch".into());
                }
                for (block, content) in content.iter().enumerate() {
                    if matches!(content, AssistantContentBlock::ToolCall(_)) {
                        pending.insert((
                            index,
                            u32::try_from(block).map_err(|_| "request block overflow")?,
                        ));
                    }
                }
            }
            _ if !pending.is_empty() => {
                return Err("core or historical message splits a Tool batch".into());
            }
            _ => {}
        }
    }
    if !pending.is_empty() {
        return Err("request ends inside an unresolved Tool batch".into());
    }
    Ok(())
}

/// Canonical-only fixtures and callers can bind physical occurrences directly.
/// Derived pairs require the compiler's explicit bindings; no association is inferred from strings.
pub(crate) fn canonical_associations(
    scope: &crate::runtime::identity::ConversationId,
    request_id: &RequestId,
    messages: &[crate::model::ModelInputMessage],
) -> Option<Vec<ActualRequestToolAssociation>> {
    let mut pending = std::collections::BTreeMap::new();
    let mut associations = Vec::new();
    for (index, message) in messages.iter().enumerate() {
        let index = u32::try_from(index).ok()?;
        match message {
            crate::model::ModelInputMessage::Canonical(
                crate::message::types::MessageBlock::Assistant(assistant),
            ) => {
                for (block, content) in assistant.content.iter().enumerate() {
                    if let AssistantContentBlock::ToolCall(call) = content {
                        let block = u32::try_from(block).ok()?;
                        let source = crate::message::types::ToolCallOccurrenceRef::new(
                            assistant.id.clone(),
                            ContentBlockIndex::new(block),
                        );
                        if pending
                            .insert(
                                source,
                                (
                                    index,
                                    block,
                                    CanonicalMessageRef::of(scope, message.as_canonical()?),
                                    call.id.clone(),
                                ),
                            )
                            .is_some()
                        {
                            return None;
                        }
                    }
                }
            }
            crate::model::ModelInputMessage::Canonical(
                crate::message::types::MessageBlock::Tool(result),
            ) => {
                let (call_message, call_block, assistant, id) =
                    pending.remove(&result.occurrence)?;
                if id != result.tool_call_id {
                    return None;
                }
                associations.push(ActualRequestToolAssociation {
                    call: ActualRequestCallCoordinate {
                        message: RequestMessageOccurrence {
                            request_id: request_id.clone(),
                            item_index: call_message,
                        },
                        block: RequestBlockIndex(call_block),
                    },
                    result: RequestMessageOccurrence {
                        request_id: request_id.clone(),
                        item_index: index,
                    },
                    source: CanonicalCallSource {
                        assistant,
                        block: result.occurrence.block_index,
                    },
                    wire_id: ProviderWireCorrelationId(id.to_string()),
                });
            }
            crate::model::ModelInputMessage::Derived(_)
                if matches!(
                    message.view(),
                    ModelMessageView::Tool(_, _) | ModelMessageView::Assistant(_)
                ) =>
            {
                return None;
            }
            _ => {}
        }
    }
    install_wire_labels(messages, &mut associations);
    pending.is_empty().then_some(associations)
}

/// Stable wire names for distinct input occurrences. Keep the latest native
/// label so its continuation result still refers to the accepted output.
pub(crate) fn wire_labels(raw: &[String]) -> Vec<String> {
    let mut remaining = std::collections::BTreeMap::<&str, usize>::new();
    for id in raw {
        *remaining.entry(id).or_default() += 1;
    }
    let mut occupied = raw
        .iter()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    raw.iter()
        .enumerate()
        .map(|(index, id)| {
            let count = remaining.get_mut(id.as_str()).expect("counted wire ID");
            *count -= 1;
            if *count == 0 {
                return id.clone();
            }
            let mut label = format!("rustx_occurrence_{index}");
            while occupied.contains(&label) {
                label.push('_');
            }
            occupied.insert(label.clone());
            label
        })
        .collect()
}
fn wire_map(
    messages: &[crate::model::ModelInputMessage],
) -> std::collections::BTreeMap<(u32, u32), String> {
    let mut calls = Vec::new();
    for (message, item) in messages.iter().enumerate() {
        if let ModelMessageView::Assistant(content) = item.view() {
            for (block, item) in content.iter().enumerate() {
                if let AssistantContentBlock::ToolCall(call) = item {
                    calls.push((
                        (
                            u32::try_from(message).expect("bounded request item coordinate"),
                            u32::try_from(block).expect("bounded request block coordinate"),
                        ),
                        call.id.to_string(),
                    ));
                }
            }
        }
    }
    let labels = wire_labels(&calls.iter().map(|(_, id)| id.clone()).collect::<Vec<_>>());
    calls
        .into_iter()
        .zip(labels)
        .map(|((coordinate, _), label)| (coordinate, label))
        .collect()
}
fn install_wire_labels(
    messages: &[crate::model::ModelInputMessage],
    associations: &mut [ActualRequestToolAssociation],
) {
    let labels = wire_map(messages);
    for a in associations {
        if let Some(label) = labels.get(&(a.call.message.item_index, a.call.block.0)) {
            a.wire_id = ProviderWireCorrelationId(label.clone());
        }
    }
}
