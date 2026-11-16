//! Closed finite inspection dispatch over the caller's original native cut.
use crate::durable::observation::{TraceFactData, TraceFactHeader};
use crate::durable::{ConversationStore, ConversationStoreError};
use crate::observation::cut::{CheckpointPageItem, CheckpointReadCut, CollectionFrontier};
use crate::runtime_client::checkpoint::CheckpointPage;
use crate::runtime_client::trace::{TraceInspectionContent, TraceProjection};
use crate::runtime_client::{RuntimeClientCursor, RuntimeClientError};

pub(super) fn projection<'a>(
    store: &'a dyn ConversationStore,
    cut: &CheckpointReadCut,
) -> Result<TraceProjection<'a>, RuntimeClientError> {
    let native = cut
        .source_cut
        .as_ref()
        .filter(|source| {
            source.read.journal == cut.through && source.read.conversation_id == cut.conversation_id
        })
        .ok_or(RuntimeClientError::ReadCutExpired)?;
    TraceProjection::at_cut(store, native).map_err(RuntimeClientError::observation_store_error)
}

pub(super) fn page(
    projection: &TraceProjection<'_>,
    anchor: &TraceFactHeader,
    cut: &CheckpointReadCut,
    domain: &str,
    offset: u64,
) -> Result<CheckpointPage, RuntimeClientError> {
    let head = projection
        .inspection_head(anchor)
        .map_err(RuntimeClientError::observation_store_error)?;
    let mut collections = head.record.collections.clone();
    match &head.content {
        TraceInspectionContent::Messages { page } => collections.push(CollectionFrontier {
            domain: page.domain.clone(),
            revision: cut.through,
            total: page.total,
            loaded: page.items.len() as u64,
            next_offset: page.next_offset,
        }),
        TraceInspectionContent::Compaction { page, .. } => {
            collections.push(CollectionFrontier {
                domain: page.domain.clone(),
                revision: cut.through,
                total: page.total,
                loaded: page.items.len() as u64,
                next_offset: page.next_offset,
            });
            for call in &page.items {
                collections.push(CollectionFrontier {
                    domain: call.requests_domain.clone(),
                    revision: cut.through,
                    total: call.request_count,
                    loaded: 0,
                    next_offset: (call.request_count > 0).then_some(0),
                });
            }
        }
        _ => {}
    }
    let items = if offset == 0 {
        vec![CheckpointPageItem {
            offset: 0,
            identity: head.id.clone(),
            value: Some(value(&head)?),
            content: Vec::new(),
            deferred: true,
            body_complete: Some(false),
            key: None,
        }]
    } else {
        Vec::new()
    };
    Ok(CheckpointPage {
        collections,
        cursor: RuntimeClientCursor::new(cut.revision),
        read_cut: cut.clone(),
        domain: domain.into(),
        total: 1,
        items,
        next_offset: None,
    })
}

pub(crate) enum CompactionSelection {
    Calls(u64),
    Requests(u64, crate::runtime::identity::AuxiliaryCallId),
}

pub(crate) fn compaction_selection(domain: &str) -> Option<CompactionSelection> {
    let tail = domain.strip_prefix("trace.records.")?;
    let (sequence, tail) = tail.split_once(".detail.calls")?;
    let sequence = sequence.parse::<u64>().ok().filter(|value| *value > 0)?;
    if tail.is_empty() {
        return Some(CompactionSelection::Calls(sequence));
    }
    let call = tail.strip_prefix('.')?.strip_suffix(".requests")?;
    if call.is_empty() || call.len() > 64 {
        return None;
    }
    Some(CompactionSelection::Requests(
        sequence,
        crate::runtime::identity::AuxiliaryCallId::new(call),
    ))
}

pub(crate) fn compaction_page(
    store: &dyn ConversationStore,
    cut: &CheckpointReadCut,
    domain: &str,
    offset: u64,
    limit: usize,
) -> Result<CheckpointPage, RuntimeClientError> {
    if limit == 0 || limit > 32 || offset > crate::observation::content::CONTENT_SAFE_INTEGER_MAX {
        return Err(RuntimeClientError::InvalidRequest {
            message: "invalid Compaction inspection page bounds".into(),
        });
    }
    let selected =
        compaction_selection(domain).ok_or_else(|| RuntimeClientError::InvalidRequest {
            message: "invalid Compaction inspection collection".into(),
        })?;
    let projection = projection(store, cut)?;
    let mut collections = Vec::new();
    let (total, items, next_offset) = match selected {
        CompactionSelection::Calls(sequence) => {
            let page = projection
                .inspection_compaction_calls(sequence, offset, limit)
                .map_err(RuntimeClientError::observation_store_error)?;
            let items = page
                .items
                .into_iter()
                .map(|call| {
                    collections.push(CollectionFrontier {
                        domain: call.requests_domain.clone(),
                        revision: cut.through,
                        total: call.request_count,
                        loaded: 0,
                        next_offset: (call.request_count > 0).then_some(0),
                    });
                    Ok(CheckpointPageItem {
                        offset: call.started_sequence,
                        identity: call.call_id.to_string(),
                        value: Some(value(&call)?),
                        content: Vec::new(),
                        deferred: false,
                        body_complete: Some(true),
                        key: None,
                    })
                })
                .collect::<Result<Vec<_>, RuntimeClientError>>()?;
            (page.total, items, page.next_offset)
        }
        CompactionSelection::Requests(sequence, call_id) => {
            let requests = projection
                .inspection_compaction_requests(sequence, &call_id)
                .map_err(RuntimeClientError::observation_store_error)?;
            let total = requests.len() as u64;
            if offset > total {
                return Err(RuntimeClientError::InvalidRange);
            }
            let end = offset.saturating_add(limit as u64).min(total);
            let start = usize::try_from(offset).map_err(|_| RuntimeClientError::InvalidRange)?;
            let count =
                usize::try_from(end - offset).map_err(|_| RuntimeClientError::InvalidRange)?;
            let items = requests
                .into_iter()
                .enumerate()
                .skip(start)
                .take(count)
                .map(|(index, request)| {
                    Ok(CheckpointPageItem {
                        offset: index as u64,
                        identity: request.request_id.to_string(),
                        value: Some(value(&request)?),
                        content: Vec::new(),
                        deferred: false,
                        body_complete: Some(true),
                        key: None,
                    })
                })
                .collect::<Result<Vec<_>, RuntimeClientError>>()?;
            (total, items, (end < total).then_some(end))
        }
    };
    collections.push(CollectionFrontier {
        domain: domain.into(),
        revision: cut.through,
        total,
        loaded: items.len() as u64,
        next_offset,
    });
    Ok(CheckpointPage {
        collections,
        cursor: RuntimeClientCursor::new(cut.revision),
        read_cut: cut.clone(),
        domain: domain.into(),
        total,
        items,
        next_offset,
    })
}

pub(super) fn is_request(anchor: &TraceFactHeader) -> bool {
    matches!(anchor.fact, TraceFactData::RequestStart { .. })
}

pub(super) fn value<T: serde::Serialize + schemars::JsonSchema>(
    head: &T,
) -> Result<serde_json::Value, RuntimeClientError> {
    let schema = crate::observation::structural_json::root::<T>();
    let encoded = crate::observation::structural_json::Encoded {
        value: head,
        schema: &schema,
        root: &schema,
    };
    let bytes = crate::observation::bounded::bounded_bytes(
        &encoded,
        crate::observation::trace::TRACE_DETAIL_BYTES,
    )
    .ok_or_else(|| {
        RuntimeClientError::observation_store_error(
            ConversationStoreError::ObservationCapacityExceeded,
        )
    })?;
    serde_json::from_slice(&bytes).map_err(|_| RuntimeClientError::ObservationUnavailable {
        message: "native Trace inspection header could not be represented".into(),
    })
}

pub(crate) fn messages_selection(domain: &str) -> Option<u64> {
    domain
        .strip_prefix("trace.records.")?
        .strip_suffix(".detail.messages")?
        .parse::<u64>()
        .ok()
        .filter(|sequence| *sequence > 0)
}

pub(crate) fn canonical_selection(domain: &str) -> bool {
    domain
        .strip_prefix("messages.")
        .and_then(|tail| {
            let (identity, collection) = tail.rsplit_once('.')?;
            if let Some(position) = identity.strip_prefix("ledger:") {
                position.parse::<u64>().ok().filter(|position| {
                    *position > 0
                        && *position <= crate::observation::content::CONTENT_SAFE_INTEGER_MAX
                })?;
            } else {
                let id = identity.strip_prefix("message:")?;
                if id.is_empty() || id.len() > 256 {
                    return None;
                }
            }
            matches!(collection, "blocks" | "tools").then_some(())
        })
        .is_some()
}

pub(crate) fn messages_page(
    store: &dyn ConversationStore,
    cut: &CheckpointReadCut,
    domain: &str,
    offset: u64,
    limit: usize,
) -> Result<CheckpointPage, RuntimeClientError> {
    let sequence =
        messages_selection(domain).ok_or_else(|| RuntimeClientError::InvalidRequest {
            message: "invalid native Trace canonical collection".into(),
        })?;
    if limit == 0 || limit > 32 || offset > crate::observation::content::CONTENT_SAFE_INTEGER_MAX {
        return Err(RuntimeClientError::InvalidRequest {
            message: "invalid native Trace canonical page bounds".into(),
        });
    }
    let source = projection(store, cut)?
        .inspection_messages(sequence, offset, limit)
        .map_err(RuntimeClientError::observation_store_error)?;
    let mut collections = Vec::new();
    let items = source
        .items
        .into_iter()
        .enumerate()
        .map(|(index, message)| {
            collections.extend(message.collections.clone());
            Ok(CheckpointPageItem {
                offset: offset + index as u64,
                identity: message.locator.key().into_owned(),
                value: Some(value(&message)?),
                content: Vec::new(),
                deferred: true,
                body_complete: Some(false),
                key: None,
            })
        })
        .collect::<Result<Vec<_>, RuntimeClientError>>()?;
    collections.push(CollectionFrontier {
        domain: domain.into(),
        revision: cut.through,
        total: source.total,
        loaded: items.len() as u64,
        next_offset: source.next_offset,
    });
    Ok(CheckpointPage {
        collections,
        cursor: RuntimeClientCursor::new(cut.revision),
        read_cut: cut.clone(),
        domain: domain.into(),
        total: source.total,
        items,
        next_offset: source.next_offset,
    })
}
