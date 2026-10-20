//! Historical display domains read only the supplied native immutable cut.
use super::checkpoint::{CheckpointPage, CheckpointValueChunk};
use super::{RuntimeClientCursor, RuntimeClientError};
use crate::durable::ConversationStore;
use crate::observation::cut::{CheckpointPageItem, CheckpointReadCut};

fn locator(
    value: &str,
) -> Result<crate::observation::content::CanonicalContentLocator, RuntimeClientError> {
    if let Some(position) = value.strip_prefix("ledger:") {
        let position = value_as_position(position)?;
        return Ok(
            crate::observation::content::CanonicalContentLocator::ByLedgerPosition { position },
        );
    }
    let value =
        value
            .strip_prefix("message:")
            .ok_or_else(|| RuntimeClientError::InvalidRequest {
                message: "invalid canonical locator namespace".into(),
            })?;
    if value.len() > 256 {
        return Err(RuntimeClientError::InvalidRequest {
            message: "historical locator is too long".into(),
        });
    }
    Ok(
        crate::observation::content::CanonicalContentLocator::ByMessageId {
            message_id: crate::runtime::identity::MessageId::new(value),
        },
    )
}
fn value_as_position(value: &str) -> Result<u64, RuntimeClientError> {
    value
        .parse::<u64>()
        .ok()
        .filter(|value| *value > 0 && *value <= 9_007_199_254_740_991)
        .ok_or(RuntimeClientError::ReadCutExpired)
}

pub(crate) fn page(
    store: &dyn ConversationStore,
    cut: &CheckpointReadCut,
    domain: &str,
    offset: u64,
    limit: usize,
) -> Result<CheckpointPage, RuntimeClientError> {
    if limit == 0 || limit > 32 || offset > 9_007_199_254_740_991 {
        return Err(RuntimeClientError::InvalidRequest {
            message: "invalid historical page bounds".into(),
        });
    }
    let native = cut
        .source_cut
        .as_ref()
        .ok_or(RuntimeClientError::ReadCutExpired)?;
    if cut.conversation_id != native.read.conversation_id || cut.through != native.read.journal {
        return Err(RuntimeClientError::ReadCutExpired);
    }
    if super::delivery::selection(domain).is_some() {
        return super::delivery::page(store, cut, domain, offset, limit);
    }
    if let Some(cursor) = super::response::identity_selection(domain) {
        return super::response::identity_page(store, cut, domain, cursor, offset, limit);
    }
    if super::audit_observation::selection(domain).is_some() {
        return super::audit_observation::page(store, cut, domain, offset, limit);
    }
    let (total, items, next_offset, collections) = if domain == "messages" {
        message_page(store, native, offset, limit)?
    } else if domain == "transcript.entries" {
        transcript_page(store, native, cut.through, offset, limit)?
    } else if let Some(identity) = domain
        .strip_prefix("messages.")
        .and_then(|value| value.strip_suffix(".blocks"))
    {
        block_page(store, native, &locator(identity)?, offset, limit)?
    } else if let Some(identity) = domain
        .strip_prefix("messages.")
        .and_then(|value| value.strip_suffix(".tools"))
    {
        return super::tool_observation::page(
            store,
            cut,
            domain,
            &locator(identity)?,
            offset,
            limit,
        );
    } else {
        return Err(RuntimeClientError::InvalidRequest {
            message: "unknown historical display domain".into(),
        });
    };
    Ok(CheckpointPage {
        read_cut: cut.clone(),
        cursor: RuntimeClientCursor::new(cut.revision),
        domain: domain.into(),
        total,
        items,
        next_offset,
        collections,
    })
}

type PageComponents = (
    u64,
    Vec<CheckpointPageItem>,
    Option<u64>,
    Vec<crate::observation::cut::CollectionFrontier>,
);
fn message_page(
    store: &dyn ConversationStore,
    native: &crate::durable::observation::ObservationReadCut,
    offset: u64,
    limit: usize,
) -> Result<PageComponents, RuntimeClientError> {
    let mut collections = Vec::new();

    let page = store
        .message_header_page(native, Some(offset), limit)
        .map_err(source_error)?;
    collections.extend(
        page.headers
            .iter()
            .flat_map(|header| header.collections.clone()),
    );
    let items = page
        .headers
        .into_iter()
        .enumerate()
        .map(|(index, header)| CheckpointPageItem {
            offset: offset + index as u64,
            identity: header
                .locator
                .as_ref()
                .map_or_else(|| "deferred".into(), |locator| locator.key().into_owned()),
            value: header
                .display
                .as_ref()
                .and_then(crate::observation::bounded::structural_value)
                .or_else(|| {
                    crate::observation::transcript::RuntimeClientMessageDescriptor::from_header(
                        &header,
                    )
                    .as_ref()
                    .and_then(crate::observation::bounded::structural_value)
                }),
            content: header
                .content
                .into_iter()
                .map(|mut binding| {
                    if binding.path.first().is_some_and(|field| field == "display") {
                        binding.path.remove(0);
                    }
                    binding
                })
                .collect(),
            deferred: header.deferred,
            body_complete: None,
            key: None,
        })
        .collect();
    Ok((page.total, items, page.next_offset, collections))
}
fn transcript_page(
    store: &dyn ConversationStore,
    native: &crate::durable::observation::ObservationReadCut,
    through: u64,
    offset: u64,
    limit: usize,
) -> Result<PageComponents, RuntimeClientError> {
    let mut collections = Vec::new();

    let page = store
        .transcript_header_page(
            native,
            (offset > 0).then(|| crate::durable::TranscriptCursor::new(offset)),
            limit,
        )
        .map_err(source_error)?;
    let mut decorated = crate::observation::transcript::transcript_header_view(page.clone());
    super::response::decorate_through(store, &mut decorated, through)
        .map_err(RuntimeClientError::observation_store_error)?;
    collections.extend(decorated.collections);
    let mut entries: std::collections::BTreeMap<_, _> = decorated
        .entries
        .into_iter()
        .map(|entry| {
            (
                entry.cursor.get(),
                crate::observation::bounded::structural_value(&entry),
            )
        })
        .collect();
    entries.extend(
        decorated
            .deferred_entries
            .into_iter()
            .map(|entry| (entry.offset, entry.value)),
    );
    let items = page
        .headers
        .into_iter()
        .map(|header| CheckpointPageItem {
            offset: header.cursor.get(),
            identity: header.reference_id,
            value: entries.remove(&header.cursor.get()).flatten(),
            content: header
                .content
                .into_iter()
                .map(|mut binding| {
                    if binding.path.first().is_some_and(|field| field == "display") {
                        binding.path.remove(0);
                    }
                    binding
                })
                .collect(),
            deferred: header.deferred,
            body_complete: None,
            key: None,
        })
        .collect();
    Ok((
        page.total,
        items,
        page.next_cursor.map(crate::durable::TranscriptCursor::get),
        collections,
    ))
}
fn block_page(
    store: &dyn ConversationStore,
    native: &crate::durable::observation::ObservationReadCut,
    identity: &crate::observation::content::CanonicalContentLocator,
    offset: u64,
    limit: usize,
) -> Result<PageComponents, RuntimeClientError> {
    let page = store
        .message_block_header_page_by_locator(native, identity, offset, limit)
        .map_err(source_error)?;
    let items = page
        .headers
        .into_iter()
        .map(|header| CheckpointPageItem {
            offset: u64::from(header.block_index.get()),
            identity: header.kind,
            value: header.value,
            content: header.content,
            deferred: header.deferred,
            body_complete: None,
            key: None,
        })
        .collect();
    Ok((page.total, items, page.next_offset, Vec::new()))
}
pub(crate) fn value_read(
    store: &dyn ConversationStore,
    cut: &CheckpointReadCut,
    domain: &str,
    item_offset: u64,
    path: &[String],
    range: (u64, u32),
) -> Result<CheckpointValueChunk, RuntimeClientError> {
    if super::projection_history::selection(domain).is_some() {
        return super::projection_history::value_read(store, cut, domain, item_offset, path, range);
    }
    if super::delivery::selection(domain).is_some() {
        return super::delivery::value_read(
            store,
            cut,
            domain,
            item_offset,
            path,
            range.0,
            range.1,
        );
    }
    if super::attempt_identity::selection(domain).is_some() {
        return super::attempt_identity::value_read(
            store,
            cut,
            domain,
            item_offset,
            path,
            range.0,
            range.1,
        );
    }
    if let Some(cursor) = super::response::identity_selection(domain) {
        if !path.is_empty() {
            return Err(RuntimeClientError::InvalidRequest {
                message: "identity source read requires empty field path".into(),
            });
        }
        return super::response::identity_read(store, cut, domain, cursor, item_offset, range);
    }
    if let Some((sequence, selected)) = super::trace_observation::selection(domain) {
        return super::trace_observation::field_read(
            store,
            cut,
            domain,
            sequence,
            &selected,
            (item_offset, path),
            range,
        );
    }
    if super::state_observation::selection(domain).is_some() {
        return super::state_observation::read(store, cut, domain, item_offset, path, range);
    }
    Err(RuntimeClientError::ObservationUnavailable {
        message:
            "historical message bodies are read through the descriptor's authoritative ContentRefs"
                .into(),
    })
}

fn source_error(error: crate::durable::ConversationStoreError) -> super::RuntimeClientError {
    super::RuntimeClientError::observation_store_error(error)
}
