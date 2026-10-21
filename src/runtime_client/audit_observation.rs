//! Bounded pages of immutable publication audit blocks at a frozen native cut.
use super::checkpoint::CheckpointPage;
use super::types::{RuntimeClientCursor, RuntimeClientError};
use crate::observation::cut::{CheckpointPageItem, CheckpointReadCut, CollectionFrontier};

pub(crate) fn selection(domain: &str) -> Option<u64> {
    let domain = domain.replace('/', ".");
    domain
        .strip_prefix("transcript.entries.")?
        .strip_suffix(".item.audit.content")?
        .parse()
        .ok()
}

pub(crate) fn page(
    store: &dyn crate::durable::ObservationStore,
    cut: &CheckpointReadCut,
    domain: &str,
    offset: u64,
    limit: usize,
) -> Result<CheckpointPage, RuntimeClientError> {
    let position = selection(domain).ok_or_else(|| RuntimeClientError::InvalidRequest {
        message: "invalid native audit domain".into(),
    })?;
    let native = cut
        .source_cut
        .as_ref()
        .ok_or(RuntimeClientError::ReadCutExpired)?;
    let page = store
        .audit_block_header_page(native, position, offset, limit)
        .map_err(RuntimeClientError::observation_store_error)?;
    let loaded = page.headers.len() as u64;
    let items = page
        .headers
        .into_iter()
        .enumerate()
        .map(|(index, header)| CheckpointPageItem {
            body_complete: Some(!header.deferred),
            key: None,
            offset: offset + index as u64,
            identity: format!("audit:{position}:block:{}", header.block_index.get()),
            value: header.value,
            content: header.content,
            deferred: header.deferred,
        })
        .collect();
    Ok(CheckpointPage {
        collections: vec![CollectionFrontier {
            domain: domain.into(),
            revision: position,
            total: page.total,
            loaded,
            next_offset: page.next_offset,
        }],
        cursor: RuntimeClientCursor::new(cut.revision),
        read_cut: cut.clone(),
        domain: domain.into(),
        total: page.total,
        items,
        next_offset: page.next_offset,
    })
}
