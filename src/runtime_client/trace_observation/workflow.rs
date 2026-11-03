use crate::durable::ConversationStore;
use crate::observation::cut::{CheckpointPageItem, CheckpointReadCut, CollectionFrontier};
use crate::runtime_client::{RuntimeClientCursor, RuntimeClientError, checkpoint::CheckpointPage};

pub(crate) fn workflow_selection(domain: &str) -> Option<u64> {
    domain
        .strip_prefix("trace.records.")?
        .strip_suffix(".workflow.nodes")?
        .parse::<u64>()
        .ok()
        .filter(|sequence| *sequence > 0)
}
pub(crate) fn workflow_page(
    store: &dyn ConversationStore,
    cut: &CheckpointReadCut,
    domain: &str,
    offset: u64,
    limit: usize,
) -> Result<CheckpointPage, RuntimeClientError> {
    let start = workflow_selection(domain).ok_or(RuntimeClientError::ReadCutExpired)?;
    if limit == 0 || limit > 32 || offset > 9_007_199_254_740_991 {
        return Err(RuntimeClientError::ReadCutExpired);
    }
    let native = cut
        .source_cut
        .as_ref()
        .filter(|source| {
            source.read.conversation_id == cut.conversation_id && source.read.journal == cut.through
        })
        .ok_or(RuntimeClientError::ReadCutExpired)?;
    let page = store
        .observation_workflow_nodes(native, start, offset, limit)
        .map_err(RuntimeClientError::observation_store_error)?;
    let items = page
        .nodes
        .into_iter()
        .enumerate()
        .map(|(index, node)| {
            Ok(CheckpointPageItem {
                offset: offset + index as u64,
                identity: node.id.clone(),
                value: Some(super::inspection::value(&node)?),
                deferred: false,
                content: Vec::new(),
                body_complete: Some(true),
                key: None,
            })
        })
        .collect::<Result<Vec<_>, RuntimeClientError>>()?;
    Ok(CheckpointPage {
        domain: domain.into(),
        cursor: RuntimeClientCursor::new(cut.revision),
        read_cut: cut.clone(),
        total: page.total,
        collections: vec![CollectionFrontier {
            domain: domain.into(),
            revision: page.revision,
            total: page.total,
            loaded: items.len() as u64,
            next_offset: page.next_offset,
        }],
        items,
        next_offset: page.next_offset,
    })
}
