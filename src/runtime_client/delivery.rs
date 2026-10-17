//! Positive successful-C delivery descriptors and selected original metadata.
use super::{
    RuntimeClientError,
    checkpoint::{CheckpointReadCut, CheckpointValueChunk, CheckpointValueRepresentation},
    content::CanonicalContentLocator,
};
use crate::durable::ConversationStore;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RuntimeClientDeliveryItem {
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub index: u64,
    pub locator: CanonicalContentLocator,
    pub file: Option<crate::tools::session_files::SessionFileReference>,
    pub metadata_deferred: bool,
    pub encoded_bytes: u64,
    pub metadata_domain: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RuntimeClientDeliveryHead {
    pub domain: String,
    pub total: u64,
    #[schemars(length(max = 8))]
    pub items: Vec<RuntimeClientDeliveryItem>,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub next_offset: Option<u64>,
}
pub(crate) fn selection(domain: &str) -> Option<(u64, Option<u64>)> {
    if domain.len() > 256 {
        return None;
    }
    let source = domain.strip_prefix("messages.ledger:")?;
    let (position, tail) = source.split_once(".deliveries")?;
    let position = position
        .parse::<u64>()
        .ok()
        .filter(|value| *value > 0 && *value <= super::content::CONTENT_SAFE_INTEGER_MAX)?;
    let index = if tail.is_empty() {
        None
    } else {
        Some(
            tail.strip_prefix('.')?
                .strip_suffix(".metadata")?
                .parse::<u64>()
                .ok()?,
        )
    };
    let canonical = if let Some(index) = index {
        format!("messages.ledger:{position}.deliveries.{index}.metadata")
    } else {
        format!("messages.ledger:{position}.deliveries")
    };
    (domain == canonical).then_some((position, index))
}
fn native_cut<'a>(
    store: &dyn ConversationStore,
    cut: &'a CheckpointReadCut,
) -> Result<&'a crate::durable::observation::ObservationReadCut, RuntimeClientError> {
    if &cut.conversation_id != store.conversation_id() {
        return Err(RuntimeClientError::ContentSourceUnavailable);
    }
    let native = cut
        .source_cut
        .as_ref()
        .ok_or(RuntimeClientError::ReadCutExpired)?;
    if native.read.conversation_id != cut.conversation_id || native.read.journal != cut.through {
        return Err(RuntimeClientError::ReadCutExpired);
    }
    Ok(native)
}
pub(crate) fn value_read(
    store: &dyn ConversationStore,
    cut: &CheckpointReadCut,
    domain: &str,
    item_offset: u64,
    path: &[String],
    offset: u64,
    max_bytes: u32,
) -> Result<CheckpointValueChunk, RuntimeClientError> {
    use base64::Engine as _;
    let (position, Some(index)) =
        selection(domain).ok_or_else(|| RuntimeClientError::InvalidRequest {
            message: "invalid selected delivery domain".into(),
        })?
    else {
        return Err(RuntimeClientError::InvalidRequest {
            message: "delivery metadata requires its selected domain".into(),
        });
    };
    if item_offset != 0 || !path.is_empty() {
        return Err(RuntimeClientError::InvalidRequest {
            message: "delivery metadata requires its advertised item and path".into(),
        });
    }
    let native = native_cut(store, cut)?;
    let chunk = store
        .observation_delivery_read(
            native,
            &CanonicalContentLocator::ByLedgerPosition { position },
            index,
            offset,
            max_bytes,
        )
        .map_err(RuntimeClientError::observation_store_error)?;
    Ok(CheckpointValueChunk {
        representation: CheckpointValueRepresentation::CanonicalJsonV1,
        cut: cut.clone(),
        domain: domain.into(),
        item_offset,
        path: path.to_vec(),
        offset,
        bytes_base64: base64::engine::general_purpose::STANDARD.encode(chunk.bytes),
        readable_end: chunk.readable_end,
        next_offset: chunk.next_offset,
    })
}
pub(crate) fn page(
    store: &dyn ConversationStore,
    cut: &CheckpointReadCut,
    domain: &str,
    offset: u64,
    limit: usize,
) -> Result<super::checkpoint::CheckpointPage, RuntimeClientError> {
    let (position, None) = selection(domain).ok_or_else(|| RuntimeClientError::InvalidRequest {
        message: "invalid delivery collection".into(),
    })?
    else {
        return Err(RuntimeClientError::InvalidRequest {
            message: "selected metadata is not a collection".into(),
        });
    };
    let native = native_cut(store, cut)?;
    let head = store
        .observation_deliveries(
            native,
            &CanonicalContentLocator::ByLedgerPosition { position },
            offset,
            limit,
        )
        .map_err(RuntimeClientError::observation_store_error)?;
    let loaded = u64::try_from(head.items.len()).expect("bounded delivery head");
    let items = head
        .items
        .iter()
        .map(|item| super::checkpoint::CheckpointPageItem {
            body_complete: None,
            key: None,
            content: Vec::new(),
            offset: item.index,
            identity: format!("delivery:{position}:{}", item.index),
            value: super::checkpoint::structural_value(item),
            deferred: item.metadata_deferred,
        })
        .collect();
    Ok(super::checkpoint::CheckpointPage {
        cursor: super::types::RuntimeClientCursor::new(cut.revision),
        read_cut: cut.clone(),
        domain: domain.into(),
        total: head.total,
        next_offset: head.next_offset,
        collections: vec![super::checkpoint::CollectionFrontier {
            domain: domain.into(),
            revision: 1,
            total: head.total,
            loaded,
            next_offset: head.next_offset,
        }],
        items,
    })
}

pub(crate) fn frontier(head: &RuntimeClientDeliveryHead) -> super::checkpoint::CollectionFrontier {
    super::checkpoint::CollectionFrontier {
        domain: head.domain.clone(),
        revision: 1,
        total: head.total,
        loaded: u64::try_from(head.items.len()).expect("bounded delivery head"),
        next_offset: head.next_offset,
    }
}
