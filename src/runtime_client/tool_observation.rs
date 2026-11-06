//! Canonical Tool bodies and Journal execution facts at one immutable native cut.
use crate::observation::tool::{collections, content, foreground};

pub(crate) fn page(
    store: &dyn crate::durable::CanonicalStore,
    cut: &crate::observation::cut::CheckpointReadCut,
    domain: &str,
    locator: &crate::observation::content::CanonicalContentLocator,
    offset: u64,
    limit: usize,
) -> Result<super::checkpoint::CheckpointPage, super::RuntimeClientError> {
    let native = cut
        .source_cut
        .as_ref()
        .ok_or(super::RuntimeClientError::ReadCutExpired)?;
    let source = store
        .transcript_tool_headers(native, locator, offset, limit)
        .map_err(source_error)?;
    let items = source
        .headers
        .iter()
        .map(|header| {
            let view = foreground(&source, header)
                .map_err(super::RuntimeClientError::tool_terminal_unavailable)?;
            Ok(crate::observation::cut::CheckpointPageItem {
                offset: u64::from(header.association.block_index.get()),
                identity: format!(
                    "{}:{}",
                    source.locator.key(),
                    header.association.block_index
                ),
                value: crate::observation::bounded::structural_value(&view),
                deferred: header.arguments_json.is_none()
                    || view.result_metadata_ref.is_some()
                    || header
                        .committed_result
                        .as_ref()
                        .is_some_and(|result| result.result_deferred),
                body_complete: None,
                key: None,
                content: content(header, &view),
            })
        })
        .collect::<Result<Vec<_>, super::RuntimeClientError>>()?;
    Ok(super::checkpoint::CheckpointPage {
        read_cut: cut.clone(),
        cursor: super::RuntimeClientCursor::new(cut.revision),
        domain: domain.into(),
        total: source.total,
        items,
        next_offset: source.next_offset,
        collections: source.headers.iter().flat_map(collections).collect(),
    })
}

fn source_error(error: crate::durable::ConversationStoreError) -> super::RuntimeClientError {
    super::RuntimeClientError::observation_store_error(error)
}
