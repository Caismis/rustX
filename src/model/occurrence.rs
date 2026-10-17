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
    /// Absent only when both occurrences are covered by native continuation
    /// and no correlation field is transmitted in this actual request.
    pub wire_id: Option<ProviderWireCorrelationId>,
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

/// Frozen actual-request relationships. The native producer's validation proof
/// shares the exact Store-issued selection origin; mutation revokes that proof.
/// Serialization remains the established flat association array.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FrozenToolAssociations {
    values: std::sync::Arc<Vec<ActualRequestToolAssociation>>,
    #[serde(skip)]
    validation: Option<std::sync::Arc<AssociationValidation>>,
}
#[derive(Debug)]
struct AssociationValidation {
    origin: std::sync::Arc<crate::model::selected_input::SelectionOrigin>,
    request: RequestId,
    policy: [u8; 32],
}
impl PartialEq for FrozenToolAssociations {
    fn eq(&self, other: &Self) -> bool {
        self.values == other.values
    }
}
impl Eq for FrozenToolAssociations {}
impl From<Vec<ActualRequestToolAssociation>> for FrozenToolAssociations {
    fn from(values: Vec<ActualRequestToolAssociation>) -> Self {
        Self {
            values: std::sync::Arc::new(values),
            validation: None,
        }
    }
}
impl std::ops::Deref for FrozenToolAssociations {
    type Target = Vec<ActualRequestToolAssociation>;
    fn deref(&self) -> &Self::Target {
        &self.values
    }
}
impl std::ops::DerefMut for FrozenToolAssociations {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.validation = None;
        std::sync::Arc::make_mut(&mut self.values)
    }
}
impl<'a> IntoIterator for &'a FrozenToolAssociations {
    type Item = &'a ActualRequestToolAssociation;
    type IntoIter = std::slice::Iter<'a, ActualRequestToolAssociation>;
    fn into_iter(self) -> Self::IntoIter {
        self.values.iter()
    }
}
impl<'a> IntoIterator for &'a mut FrozenToolAssociations {
    type Item = &'a mut ActualRequestToolAssociation;
    type IntoIter = std::slice::IterMut<'a, ActualRequestToolAssociation>;
    fn into_iter(self) -> Self::IntoIter {
        (**self).iter_mut()
    }
}
fn association_policy(
    protocol: crate::model::ModelProtocol,
    continuation: Option<&crate::runtime::continuation::ProviderContinuationState>,
    source: Option<&crate::runtime::continuation::AcceptedOutputContinuationSource>,
) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    serde_json::to_writer(
        {
            struct PolicyWriter<'a>(&'a mut Sha256);
            impl std::io::Write for PolicyWriter<'_> {
                fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                    self.0.update(bytes);
                    Ok(bytes.len())
                }
                fn flush(&mut self) -> std::io::Result<()> {
                    Ok(())
                }
            }
            PolicyWriter(&mut digest)
        },
        &(protocol, continuation, source),
    )
    .expect("native wire policy serializes");
    digest.finalize().into()
}
impl FrozenToolAssociations {
    pub(crate) fn certifies(
        &self,
        origin: &std::sync::Arc<crate::model::selected_input::SelectionOrigin>,
        snapshot: &crate::model::RequestSnapshot,
    ) -> bool {
        self.validation.as_ref().is_some_and(|validation| {
            std::sync::Arc::ptr_eq(&validation.origin, origin)
                && validation.request == snapshot.request_id
                && validation.policy
                    == association_policy(
                        snapshot.invocation.protocol,
                        snapshot.continuation.as_ref(),
                        snapshot.continuation_source.as_ref(),
                    )
        })
    }
}

pub(crate) fn freeze_associations(
    history: &crate::durable::projection::HistoricalModelInput,
    request_id: &RequestId,
    request: &crate::model::ModelRequest,
    inserted_at: Option<usize>,
) -> Result<FrozenToolAssociations, String> {
    #[cfg(test)]
    let _reads = super::selected_input::PreparationReads::new();
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
                wire_id: Some(association.wire_id.clone()),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    if associations.is_empty() && !request.messages.has_tool_relationships() {
        request.continuation_boundary()?;
    } else {
        install_wire_labels(request, &mut associations)?;
        if !request
            .messages
            .follows_native_history(&history.input_manifest)
        {
            validate_associations(request_id, request, &associations)?;
        }
    }
    let validation = history.input_manifest.origin.as_ref().map(|origin| {
        std::sync::Arc::new(AssociationValidation {
            origin: origin.clone(),
            request: request_id.clone(),
            policy: association_policy(
                request.protocol(),
                request.continuation.as_ref(),
                request.continuation_source.as_ref(),
            ),
        })
    });
    Ok(FrozenToolAssociations {
        values: std::sync::Arc::new(associations),
        validation,
    })
}

#[allow(clippy::too_many_lines)] // One complete request coordinate and Tool-batch validation fold.
pub(crate) fn validate_associations(
    request_id: &RequestId,
    request: &crate::model::ModelRequest,
    associations: &[ActualRequestToolAssociation],
) -> Result<(), String> {
    if associations.is_empty() && !request.messages.has_tool_relationships() {
        request.continuation_boundary()?;
        return Ok(());
    }
    let messages = &request.messages;
    let labels = crate::model::adapter::correlation::request_labels(request)?;
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
            )) != association.wire_id.as_ref().map(|id| &id.0)
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
    request: &crate::model::ModelRequest,
) -> Option<Vec<ActualRequestToolAssociation>> {
    let messages = &request.messages;
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
                    wire_id: Some(ProviderWireCorrelationId(id.to_string())),
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
    install_wire_labels(request, &mut associations).ok()?;
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
fn install_wire_labels(
    request: &crate::model::ModelRequest,
    associations: &mut [ActualRequestToolAssociation],
) -> Result<(), String> {
    let labels = crate::model::adapter::correlation::request_labels(request)?;
    for a in associations {
        a.wire_id = labels
            .get(&(a.call.message.item_index, a.call.block.0))
            .map(|label| ProviderWireCorrelationId(label.clone()));
    }
    Ok(())
}
