//! Historical model projections, separate from the native execution Trace.
use super::snapshot::RuntimeClientProjectionHeadView;
use super::trace::TraceRequestMessage;
use crate::context::checkpoint::ProjectionActivationId;
use crate::runtime::identity::CheckpointId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectionPage {
    pub records: Vec<RuntimeClientProjectionHeadView>,
    pub next_activation: Option<ProjectionActivationId>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectionDetail {
    pub checkpoint: RuntimeClientProjectionHeadView,
    pub items: Vec<ProjectionItemDetail>,
    pub next_item: Option<u32>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectionItemDetail {
    pub occurrence: crate::context::checkpoint::CheckpointMessageOccurrence,
    pub content: TraceRequestMessage,
}

pub(crate) fn page(
    store: &dyn crate::durable::ConversationStore,
    through: u64,
    after: ProjectionActivationId,
    limit: usize,
) -> Result<ProjectionPage, crate::durable::ConversationStoreError> {
    let page = store.projection_timeline_at_journal_cut(through, after, limit)?;
    Ok(ProjectionPage {
        records: page
            .records
            .into_iter()
            .map(super::projection::checkpoint_view)
            .collect(),
        next_activation: page.next_activation,
    })
}
pub(crate) fn detail(
    store: &dyn crate::durable::ConversationStore,
    through: u64,
    id: &CheckpointId,
    offset: u32,
    limit: usize,
) -> Result<ProjectionDetail, crate::durable::ConversationStoreError> {
    use crate::context::checkpoint::{CheckpointMessage, CheckpointMessageOccurrence};
    if limit == 0 || limit > 32 {
        return Err(crate::durable::ConversationStoreError::InvalidReference(
            "projection detail limit must be 1..=32".into(),
        ));
    }
    let checkpoint = store.projection_checkpoint_at_journal_cut(through, id)?;
    let end = (offset as usize)
        .saturating_add(limit)
        .min(checkpoint.candidate.items.len());
    if offset as usize > checkpoint.candidate.items.len() {
        return Err(crate::durable::ConversationStoreError::InvalidReference(
            "projection detail offset exceeds items".into(),
        ));
    }
    let mut resources = crate::context::resources::CompactionResources::capture(
        store,
        crate::context::resources::ResourceLimits::default(),
    )?;
    let mut items = Vec::new();
    for (index, item) in checkpoint
        .candidate
        .items
        .iter()
        .enumerate()
        .take(end)
        .skip(offset as usize)
    {
        let input = match &item.message {
            CheckpointMessage::Original(reference) => {
                let body = resources
                    .originals(std::slice::from_ref(&reference.message_id))?
                    .rows
                    .pop()
                    .expect("keyed source exists")
                    .1;
                if !reference.matches(&body) {
                    return Err(crate::durable::ConversationStoreError::InvalidReference(
                        "projection Original integrity mismatch".into(),
                    ));
                }
                crate::model::ModelInputMessage::Canonical(body)
            }
            CheckpointMessage::Derived(d) => {
                crate::model::ModelInputMessage::Derived(d.as_ref().clone())
            }
        };
        items.push(ProjectionItemDetail {
            occurrence: CheckpointMessageOccurrence {
                checkpoint_id: id.clone(),
                item_index: u32::try_from(index).expect("bounded checkpoint coordinate"),
            },
            content: super::trace::request::request_message(&input, store.conversation_id()),
        });
    }
    Ok(ProjectionDetail {
        next_item: (end < checkpoint.candidate.items.len())
            .then_some(u32::try_from(end).expect("bounded checkpoint coordinate")),
        checkpoint: super::projection::checkpoint_view(checkpoint),
        items,
    })
}
