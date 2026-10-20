//! Historical model projections, separate from the native execution Trace.
use crate::context::checkpoint::ProjectionActivationId;
use crate::observation::head::RuntimeClientProjectionHeadView;
use crate::observation::trace::TraceRequestMessage;
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
    pub read_cut: crate::observation::cut::CheckpointReadCut,
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
            .map(super::projection::views::checkpoint_view)
            .collect(),
        next_activation: page.next_activation,
    })
}
pub(crate) fn detail(
    store: &dyn crate::durable::ConversationStore,
    cut: &crate::observation::cut::CheckpointReadCut,
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
    let native = cut
        .source_cut
        .as_ref()
        .ok_or(crate::durable::ConversationStoreError::ObservationCutExpired)?;
    if &cut.conversation_id != store.conversation_id() {
        return Err(crate::durable::ConversationStoreError::ObservationCutExpired);
    }
    let checkpoint = store.projection_checkpoint_at_journal_cut(cut.through, id)?;
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
        let (input, position) = match &item.message {
            CheckpointMessage::Original(reference) => {
                let (position, body) = resources
                    .originals(std::slice::from_ref(&reference.message_id))?
                    .rows
                    .pop()
                    .expect("keyed source exists");
                if !reference.matches(&body) {
                    return Err(crate::durable::ConversationStoreError::InvalidReference(
                        "projection Original integrity mismatch".into(),
                    ));
                }
                (
                    crate::model::ModelInputMessage::Canonical(body),
                    Some(position),
                )
            }
            CheckpointMessage::Derived(d) => (
                crate::model::ModelInputMessage::Derived(d.as_ref().clone()),
                None,
            ),
        };
        let original = match &item.message {
            CheckpointMessage::Original(reference) => Some(reference),
            CheckpointMessage::Derived(_) => None,
        };
        let mut content = crate::observation::trace::request::request_message_with_original(
            &input,
            store.conversation_id(),
            original,
        );
        if let Some(position) = position {
            bind_canonical_source(store, native, position, &mut content)?;
        }
        let activation = checkpoint.activation.activation.0;
        let source = store.observation_projection_provenance_read(
            native,
            activation,
            index as u64,
            &[],
            0,
            1,
        )?;
        content.provenance_source = Some(provenance_field(
            activation,
            index as u64,
            source.readable_end,
        ));
        items.push(ProjectionItemDetail {
            occurrence: CheckpointMessageOccurrence {
                checkpoint_id: id.clone(),
                item_index: u32::try_from(index).expect("bounded checkpoint coordinate"),
            },
            content,
        });
    }
    Ok(ProjectionDetail {
        read_cut: cut.clone(),
        next_item: (end < checkpoint.candidate.items.len())
            .then_some(u32::try_from(end).expect("bounded checkpoint coordinate")),
        checkpoint: super::projection::views::checkpoint_view(checkpoint),
        items,
    })
}

fn bind_canonical_source(
    store: &dyn crate::durable::ConversationStore,
    native: &crate::durable::observation::ObservationReadCut,
    position: u64,
    content: &mut TraceRequestMessage,
) -> Result<(), crate::durable::ConversationStoreError> {
    let header = store
        .message_header_page(native, Some(position - 1), 1)?
        .headers
        .into_iter()
        .find(|header| header.ledger_position == Some(position))
        .ok_or_else(|| {
            crate::durable::ConversationStoreError::Storage(
                "original checkpoint message header unavailable".into(),
            )
        })?;
    crate::observation::trace::request::bind_original_provenance(content, &header)?;
    Ok(())
}

pub(crate) fn provenance_field(
    activation: u64,
    item: u64,
    encoded_bytes: u64,
) -> crate::observation::trace::TraceFieldSource {
    crate::observation::trace::TraceFieldSource::Value {
        field: crate::observation::trace::TraceFieldKind::Provenance,
        domain: format!("context.projections.{activation}.items.{item}.provenance"),
        item_offset: 0,
        path: Vec::new(),
        encoded_bytes,
    }
}

pub(crate) fn selection(domain: &str) -> Option<(u64, u64)> {
    let source = domain.strip_prefix("context.projections.")?;
    let (activation, source) = source.split_once(".items.")?;
    let item = source.strip_suffix(".provenance")?;
    let activation: u64 = activation.parse().ok()?;
    let item: u64 = item.parse().ok()?;
    (activation > 0 && item <= crate::observation::content::CONTENT_SAFE_INTEGER_MAX)
        .then_some((activation, item))
}

pub(crate) fn value_read(
    store: &dyn crate::durable::ConversationStore,
    cut: &crate::observation::cut::CheckpointReadCut,
    domain: &str,
    item_offset: u64,
    path: &[String],
    (offset, max_bytes): (u64, u32),
) -> Result<super::checkpoint::CheckpointValueChunk, super::RuntimeClientError> {
    use base64::Engine as _;
    let (activation, item) =
        selection(domain).ok_or_else(|| super::RuntimeClientError::InvalidRequest {
            message: "invalid projection provenance selector".into(),
        })?;
    if &cut.conversation_id != store.conversation_id() {
        return Err(super::RuntimeClientError::ReadCutExpired);
    }
    if item_offset != 0
        || !path.is_empty()
        || offset > crate::observation::content::CONTENT_SAFE_INTEGER_MAX
        || max_bytes == 0
        || max_bytes > 65_536
    {
        return Err(super::RuntimeClientError::InvalidRequest {
            message: "projection provenance is one bounded original field".into(),
        });
    }
    let native = cut
        .source_cut
        .as_ref()
        .ok_or(super::RuntimeClientError::ReadCutExpired)?;
    let chunk = store
        .observation_projection_provenance_read(native, activation, item, path, offset, max_bytes)
        .map_err(super::RuntimeClientError::observation_store_error)?;
    Ok(super::checkpoint::CheckpointValueChunk {
        representation: super::checkpoint::CheckpointValueRepresentation::CanonicalJsonV1,
        cut: cut.clone(),
        domain: domain.into(),
        item_offset,
        path: Vec::new(),
        offset,
        bytes_base64: base64::engine::general_purpose::STANDARD.encode(chunk.bytes),
        next_offset: chunk.next_offset,
        readable_end: chunk.readable_end,
    })
}
