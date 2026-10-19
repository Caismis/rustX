//! Range and field pages over the existing native Goal, Todo, and Compaction sources.
use super::checkpoint::{CheckpointPage, CheckpointValueChunk};
use super::types::{RuntimeClientCursor, RuntimeClientError};
use crate::durable::ConversationStore;
use crate::observation::cut::{CheckpointKey, CheckpointPageItem, CheckpointReadCut};
use base64::Engine as _;

pub(crate) fn selection(domain: &str) -> Option<(&str, Vec<String>)> {
    let (owner, tail) = domain.split_once('.').unwrap_or((domain, ""));
    match owner {
        "goal" | "todos" => {}
        "context" if matches!(tail, "compaction_error" | "latest_compaction" | "occupancy") => {}
        _ => return None,
    }
    Some((
        owner,
        if tail.is_empty() {
            Vec::new()
        } else {
            tail.split('.').map(str::to_owned).collect()
        },
    ))
}
pub(crate) fn available(
    store: &dyn ConversationStore,
    cut: &CheckpointReadCut,
    domain: &str,
) -> Result<bool, RuntimeClientError> {
    let (owner, _) = selection(domain).ok_or(RuntimeClientError::ReadCutExpired)?;
    let native = cut
        .source_cut
        .as_ref()
        .ok_or(RuntimeClientError::ReadCutExpired)?;
    store
        .observation_state_source(owner, native)
        .map(|source| source.is_some())
        .map_err(RuntimeClientError::observation_store_error)
}
fn source(
    store: &dyn ConversationStore,
    cut: &CheckpointReadCut,
    owner: &str,
) -> Result<String, RuntimeClientError> {
    let native = cut
        .source_cut
        .as_ref()
        .ok_or(RuntimeClientError::ReadCutExpired)?;
    store
        .observation_state_source(owner, native)
        .map_err(RuntimeClientError::observation_store_error)?
        .ok_or(RuntimeClientError::ReadCutExpired)
}
pub(crate) fn page(
    store: &dyn ConversationStore,
    cut: &CheckpointReadCut,
    domain: &str,
    offset: u64,
    limit: usize,
) -> Result<CheckpointPage, RuntimeClientError> {
    if limit == 0 || limit > 32 || offset > crate::observation::content::CONTENT_SAFE_INTEGER_MAX {
        return Err(RuntimeClientError::InvalidRequest {
            message: "invalid native state page bounds".into(),
        });
    }
    let (owner, path) = selection(domain).ok_or(RuntimeClientError::ReadCutExpired)?;
    let native = cut
        .source_cut
        .as_ref()
        .ok_or(RuntimeClientError::ReadCutExpired)?;
    let source_id = source(store, cut, owner)?;
    let (total, fields, next) = if path.is_empty() || owner == "context" {
        let header = store
            .observation_state_header(owner, &source_id, native, &path)
            .map_err(RuntimeClientError::observation_store_error)?;
        (
            1,
            if offset == 0 {
                vec![header]
            } else {
                Vec::new()
            },
            None,
        )
    } else {
        let page = store
            .observation_state_page(owner, &source_id, native, &path, offset, limit)
            .map_err(RuntimeClientError::observation_store_error)?;
        (page.total, page.fields, page.next_offset)
    };
    let mut collections = Vec::new();
    let items = fields
        .into_iter()
        .enumerate()
        .map(|(index, field)| {
            collections.extend(field.collections.into_iter().map(|mut frontier| {
                frontier.domain = if frontier.domain.is_empty() {
                    owner.into()
                } else {
                    format!("{owner}.{}", frontier.domain)
                };
                frontier
            }));
            CheckpointPageItem {
                body_complete: None,
                key: field.key.map(|key| CheckpointKey {
                    path: vec!["$key".into()],
                    preview: key.preview,
                    encoded_bytes: key.encoded_bytes,
                }),
                offset: offset + index as u64,
                identity: if path.is_empty() {
                    source_id.clone()
                } else {
                    field.path.join(".")
                },
                value: Some(field.value),
                content: field.content,
                deferred: field.deferred,
            }
        })
        .collect();
    Ok(CheckpointPage {
        collections,
        cursor: RuntimeClientCursor::new(cut.revision),
        read_cut: cut.clone(),
        domain: domain.into(),
        total,
        items,
        next_offset: next,
    })
}
pub(crate) fn read(
    store: &dyn ConversationStore,
    cut: &CheckpointReadCut,
    domain: &str,
    item_offset: u64,
    path: &[String],
    range: (u64, u32),
) -> Result<CheckpointValueChunk, RuntimeClientError> {
    let (owner, mut selected) = selection(domain).ok_or(RuntimeClientError::ReadCutExpired)?;
    let native = cut
        .source_cut
        .as_ref()
        .ok_or(RuntimeClientError::ReadCutExpired)?;
    let source_id = source(store, cut, owner)?;
    if !selected.is_empty() && owner != "context" {
        let field = store
            .observation_state_page(owner, &source_id, native, &selected, item_offset, 1)
            .map_err(RuntimeClientError::observation_store_error)?
            .fields
            .into_iter()
            .next()
            .ok_or(RuntimeClientError::ReadCutExpired)?;
        selected = field.path;
    } else if item_offset != 0 {
        return Err(RuntimeClientError::InvalidRequest {
            message: "native singleton state has only item zero".into(),
        });
    }
    selected.extend_from_slice(path);
    let (offset, max_bytes) = range;
    let chunk = store
        .observation_state_value_read(owner, &source_id, native, &selected, offset, max_bytes)
        .map_err(RuntimeClientError::observation_store_error)?;
    Ok(CheckpointValueChunk {
        representation:
            crate::runtime_client::checkpoint::CheckpointValueRepresentation::CanonicalJsonV1,
        cut: cut.clone(),
        domain: domain.into(),
        item_offset,
        path: path.to_vec(),
        offset,
        bytes_base64: base64::engine::general_purpose::STANDARD.encode(chunk.bytes),
        next_offset: chunk.next_offset,
        readable_end: chunk.readable_end,
    })
}
