//! Bounded immutable request inspection over native commit-owned field indexes.
use super::checkpoint::{
    CheckpointPage, CheckpointPageItem, CheckpointReadCut, CheckpointValueChunk,
};
use super::types::{RuntimeClientCursor, RuntimeClientError};
use crate::durable::ConversationStore;
use base64::Engine as _;
mod inspection;
pub(crate) use inspection::{canonical_selection, messages_page, messages_selection};

pub(crate) fn context_selection(domain: &str) -> Option<u64> {
    domain
        .strip_prefix("trace.records.")?
        .strip_suffix(".context")?
        .parse()
        .ok()
}
pub(crate) fn context_page(
    store: &dyn ConversationStore,
    cut: &CheckpointReadCut,
    domain: &str,
    offset: u64,
    limit: usize,
) -> Result<CheckpointPage, RuntimeClientError> {
    let start = context_selection(domain).ok_or_else(|| RuntimeClientError::InvalidRequest {
        message: "invalid request Context domain".into(),
    })?;
    let source = cut
        .source_cut
        .as_ref()
        .ok_or(RuntimeClientError::ReadCutExpired)?;
    let page = store
        .observation_request_context_at(source, start, offset, limit.min(16))
        .map_err(RuntimeClientError::observation_store_error)?;
    let items = page
        .items
        .into_iter()
        .enumerate()
        .map(|(index, value)| {
            Ok(CheckpointPageItem {
                body_complete: None,
                key: None,
                offset: offset + index as u64,
                identity: format!("context:{}", offset + index as u64),
                value: Some(super::checkpoint::structural_value(&value).ok_or_else(|| {
                    RuntimeClientError::ObservationUnavailable {
                        message: "native Context header exceeds bound".into(),
                    }
                })?),
                content: Vec::new(),
                deferred: value.truncated,
            })
        })
        .collect::<Result<Vec<_>, RuntimeClientError>>()?;
    Ok(CheckpointPage {
        collections: Vec::new(),
        cursor: RuntimeClientCursor::new(cut.revision),
        read_cut: cut.clone(),
        domain: domain.into(),
        total: page.total,
        items,
        next_offset: page.next_offset,
    })
}
pub(crate) fn selection(domain: &str) -> Option<(u64, Vec<String>)> {
    let tail = domain.strip_prefix("trace.records.")?;
    let (sequence, tail) = tail.split_once('.')?;
    let sequence = sequence.parse().ok()?;
    if tail == "detail" {
        return Some((sequence, Vec::new()));
    }
    let path = tail
        .strip_prefix("detail.request.")?
        .split('.')
        .map(str::to_owned)
        .collect();
    Some((sequence, path))
}
struct RequestView {
    request: u64,
    previous: Option<u64>,
    value: serde_json::Value,
    dynamic: bool,
    deferred: bool,
    content: Vec<super::checkpoint::ContentBinding>,
    collections: Vec<super::checkpoint::CollectionFrontier>,
}
fn request_view(
    store: &dyn ConversationStore,
    cut: &CheckpointReadCut,
    sequence: u64,
) -> Result<RequestView, RuntimeClientError> {
    let request = sequence;
    let header = store
        .request_summary_header_at(request, cut.through)
        .map_err(RuntimeClientError::observation_store_error)?;
    let mut value = header.value;
    let mut deferred = header.deferred;
    let predecessor = store
        .observation_request_predecessor_at(sequence, cut.through)
        .map_err(RuntimeClientError::observation_store_error)?;
    let previous = predecessor
        .as_ref()
        .and_then(|previous| previous.summary.as_ref().map(|_| previous.start_sequence));
    if let Some(previous_header) = predecessor {
        value["predecessor"] = serde_json::json!({"availability":if previous_header.summary.is_some(){"available"}else{"unavailable"},"request_id":previous_header.request_id,"request_id_source":previous_header.source});
        value["previous_system_prompt"] = if let Some(previous) = previous {
            let field = store
                .request_field_header_at(previous, cut.through, &["effective_system_prompt".into()])
                .map_err(RuntimeClientError::observation_store_error)?;
            deferred |= field.deferred;
            field.value
        } else {
            serde_json::Value::Null
        };
    }
    let outcome = store
        .request_outcome_header_at(request, cut.through)
        .map_err(RuntimeClientError::observation_store_error)?;
    if let Some(outcome) = &outcome {
        value["usage"] = super::checkpoint::structural_value(&outcome.usage).ok_or_else(|| {
            RuntimeClientError::RuntimeFailure {
                message: "native usage header exceeds its finite contract".into(),
            }
        })?;
        value["failure"] =
            super::checkpoint::structural_value(&outcome.failure).ok_or_else(|| {
                RuntimeClientError::RuntimeFailure {
                    message: "native failure header exceeds its finite contract".into(),
                }
            })?;
        value["generation"] =
            super::checkpoint::structural_value(&outcome.generation).ok_or_else(|| {
                RuntimeClientError::RuntimeFailure {
                    message: "native generation header exceeds its finite contract".into(),
                }
            })?;
        deferred |= outcome.failure_deferred;
    }
    Ok(RequestView {
        request,
        previous,
        value,
        dynamic: outcome.is_some(),
        deferred,
        content: header.content,
        collections: header.collections,
    })
}
pub(crate) fn page(
    store: &dyn ConversationStore,
    cut: &CheckpointReadCut,
    domain: &str,
    sequence: u64,
    offset: u64,
) -> Result<CheckpointPage, RuntimeClientError> {
    let projection = inspection::projection(store, cut)?;
    let anchor = projection
        .inspection_anchor(sequence)
        .map_err(RuntimeClientError::observation_store_error)?;
    if !inspection::is_request(&anchor) {
        return inspection::page(&projection, &anchor, cut, domain, offset);
    }
    let source = request_view(store, cut, sequence)?;
    let value = serde_json::json!({"id":format!("trace:{sequence}"),"kind":"request","request":source.value,"tool":null,"messages":[],"truncated":source.deferred});
    Ok(CheckpointPage {
        collections: source
            .collections
            .into_iter()
            .map(|mut frontier| {
                frontier.domain = format!("{domain}.request.{}", frontier.domain);
                frontier
            })
            .collect(),
        cursor: RuntimeClientCursor::new(cut.revision),
        read_cut: cut.clone(),
        domain: domain.into(),
        total: 1,
        items: if offset == 0 {
            vec![CheckpointPageItem {
                body_complete: None,
                key: None,
                offset: 0,
                identity: format!("trace:{}", source.request),
                value: Some(value),
                content: source
                    .content
                    .into_iter()
                    .map(|mut binding| {
                        binding.path.insert(0, "request".into());
                        binding
                    })
                    .collect(),
                deferred: source.deferred,
            }]
        } else {
            Vec::new()
        },
        next_offset: None,
    })
}
pub(crate) fn field_page(
    store: &dyn ConversationStore,
    cut: &CheckpointReadCut,
    domain: &str,
    sequence: u64,
    path: &[String],
    offset: u64,
    limit: usize,
) -> Result<CheckpointPage, RuntimeClientError> {
    let request = sequence;
    let source = store
        .request_field_page_at(request, cut.through, path, offset, limit)
        .map_err(RuntimeClientError::observation_store_error)?;
    let mut collections = Vec::new();
    let items = source
        .fields
        .into_iter()
        .enumerate()
        .map(|(index, field)| {
            collections.extend(field.collections.into_iter().map(|mut frontier| {
                frontier.domain = format!(
                    "trace.records.{sequence}.detail.request.{}",
                    frontier.domain
                );
                frontier
            }));
            CheckpointPageItem {
                body_complete: None,
                key: field.key.map(|key| super::checkpoint::CheckpointKey {
                    path: vec!["$key".into()],
                    preview: key.preview,
                    encoded_bytes: key.encoded_bytes,
                }),
                offset: offset + index as u64,
                identity: field.path.join("."),
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
        total: source.total,
        items,
        next_offset: source.next_offset,
    })
}
pub(crate) fn field_read(
    store: &dyn ConversationStore,
    cut: &CheckpointReadCut,
    domain: &str,
    sequence: u64,
    parent: &[String],
    selection: (u64, &[String]),
    range: (u64, u32),
) -> Result<CheckpointValueChunk, RuntimeClientError> {
    let (item_offset, path) = selection;
    let (offset, max_bytes) = range;
    let request = sequence;
    let header = store
        .request_field_header_at(request, cut.through, parent)
        .map_err(RuntimeClientError::observation_store_error)?;
    let mut selected = if header.children_total == 0 {
        if item_offset != 0 || !path.is_empty() {
            return Err(RuntimeClientError::InvalidRequest {
                message: "scalar request selection must use item zero and empty path".into(),
            });
        }
        parent.to_vec()
    } else {
        store
            .request_field_page_at(request, cut.through, parent, item_offset, 1)
            .map_err(RuntimeClientError::observation_store_error)?
            .fields
            .into_iter()
            .next()
            .ok_or(RuntimeClientError::InvalidRange)?
            .path
    };
    selected.extend_from_slice(path);
    let chunk = store
        .request_value_read_at(request, cut.through, &selected, offset, max_bytes)
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

enum Part {
    Inline(Vec<u8>),
    Native {
        request: u64,
        path: Vec<String>,
        length: u64,
        dynamic: bool,
    },
}
impl Part {
    fn length(&self) -> u64 {
        match self {
            Self::Inline(bytes) => bytes.len() as u64,
            Self::Native { length, .. } => *length,
        }
    }
}
fn parts(
    store: &dyn ConversationStore,
    cut: &CheckpointReadCut,
    sequence: u64,
    source: &RequestView,
) -> Result<Vec<Part>, RuntimeClientError> {
    let fields = store
        .request_field_page_at(source.request, cut.through, &[], 0, 32)
        .map_err(RuntimeClientError::observation_store_error)?;
    if fields.next_offset.is_some() {
        return Err(RuntimeClientError::RuntimeFailure {
            message: "request DTO field count exceeds its closed contract".into(),
        });
    }
    let mut parts = vec![Part::Inline(
        format!("{{\"id\":\"trace:{sequence}\",\"kind\":\"request\",\"request\":{{").into_bytes(),
    )];
    for (index, field) in fields.fields.into_iter().enumerate() {
        let name = field
            .path
            .last()
            .ok_or(RuntimeClientError::ReadCutExpired)?;
        let key = serde_json::to_string(name).expect("finite field name");
        parts.push(Part::Inline(
            format!("{}{key}:", if index == 0 { "" } else { "," }).into_bytes(),
        ));
        if name == "predecessor" {
            parts.push(Part::Inline(
                serde_json::to_vec(&source.value[name]).expect("finite identity"),
            ));
        } else if name == "previous_system_prompt" {
            if let Some(previous) = &source.previous {
                match store.request_field_header_at(
                    *previous,
                    cut.through,
                    &["effective_system_prompt".into()],
                ) {
                    Ok(header) => parts.push(Part::Native {
                        request: *previous,
                        path: vec!["effective_system_prompt".into()],
                        length: header.encoded_bytes,
                        dynamic: false,
                    }),
                    Err(error) => return Err(RuntimeClientError::observation_store_error(error)),
                }
            } else {
                parts.push(Part::Inline(b"null".to_vec()));
            }
        } else {
            let dynamic =
                source.dynamic && matches!(name.as_str(), "usage" | "failure" | "generation");
            let length = if dynamic {
                store
                    .request_outcome_read_at(source.request, cut.through, &field.path, 0, 1)
                    .map_err(RuntimeClientError::observation_store_error)?
                    .readable_end
            } else {
                field.encoded_bytes
            };
            parts.push(Part::Native {
                request: source.request,
                path: field.path,
                length,
                dynamic,
            });
        }
    }
    parts.push(Part::Inline(
        b"},\"tool\":null,\"messages\":[],\"truncated\":false}".to_vec(),
    ));
    Ok(parts)
}
pub(crate) fn read(
    store: &dyn ConversationStore,
    cut: &CheckpointReadCut,
    domain: &str,
    sequence: u64,
    selection: (u64, &[String]),
    range: (u64, u32),
) -> Result<CheckpointValueChunk, RuntimeClientError> {
    let anchor = inspection::projection(store, cut)?
        .inspection_anchor(sequence)
        .map_err(RuntimeClientError::observation_store_error)?;
    if !inspection::is_request(&anchor) {
        return Err(RuntimeClientError::InvalidRequest { message: "finite Trace inspection heads expose original content through their advertised fields and collections".into() });
    }
    let (item_offset, path) = selection;
    let (offset, max_bytes) = range;
    if item_offset != 0 || !path.is_empty() {
        return Err(RuntimeClientError::InvalidRequest {
            message: "Trace detail read names its complete indexed request record".into(),
        });
    }
    let source = request_view(store, cut, sequence)?;
    let parts = parts(store, cut, sequence, &source)?;
    let end = parts
        .iter()
        .try_fold(0u64, |total, part| total.checked_add(part.length()))
        .filter(|end| *end <= 9_007_199_254_740_991)
        .ok_or(RuntimeClientError::ReadCutExpired)?;
    if offset > end {
        return Err(RuntimeClientError::InvalidRequest {
            message: "Trace byte offset exceeds its readable end".into(),
        });
    }
    let through = offset.saturating_add(u64::from(max_bytes)).min(end);
    let mut cursor = 0;
    let mut bytes = Vec::with_capacity(max_bytes as usize);
    for part in parts {
        let next = cursor + part.length();
        let start = offset.max(cursor);
        let finish = through.min(next);
        if finish > start {
            let local = start - cursor;
            let count = u32::try_from(finish - start).expect("bounded range");
            match part {
                Part::Inline(value) => bytes.extend_from_slice(
                    &value[usize::try_from(local).expect("finite inline")
                        ..usize::try_from(local).expect("finite inline") + count as usize],
                ),
                Part::Native {
                    request,
                    path,
                    dynamic,
                    ..
                } => {
                    let chunk = if dynamic {
                        store.request_outcome_read_at(request, cut.through, &path, local, count)
                    } else {
                        store.request_value_read_at(request, cut.through, &path, local, count)
                    }
                    .map_err(RuntimeClientError::observation_store_error)?;
                    bytes.extend(chunk.bytes);
                }
            }
        }
        cursor = next;
        if cursor >= through {
            break;
        }
    }
    let next = offset + bytes.len() as u64;
    if next <= offset && offset < end {
        return Err(RuntimeClientError::RuntimeFailure {
            message: "indexed Trace read did not advance".into(),
        });
    }
    Ok(CheckpointValueChunk {
        representation:
            crate::runtime_client::checkpoint::CheckpointValueRepresentation::CanonicalJsonV1,
        cut: cut.clone(),
        domain: domain.into(),
        item_offset,
        path: Vec::new(),
        offset,
        bytes_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
        next_offset: (next < end).then_some(next),
        readable_end: end,
    })
}

/// A retained-history cut is created by the existing durable source owner.
pub(crate) fn history_cut(
    store: &dyn ConversationStore,
) -> Result<CheckpointReadCut, RuntimeClientError> {
    history_store_cut(store).map_err(RuntimeClientError::observation_store_error)
}
pub(crate) fn history_store_cut(
    store: &dyn ConversationStore,
) -> Result<CheckpointReadCut, crate::durable::ConversationStoreError> {
    let source = store.observation_read_cut()?;
    Ok(CheckpointReadCut {
        canonical_foreground: None,
        live_publication: None,
        collection_cuts: std::collections::BTreeMap::new(),
        conversation_id: store.conversation_id().clone(),
        activation_id: None,
        revision: 0,
        through: source.read.journal,
        source_cut: Some(source),
        domain_revisions: std::collections::BTreeMap::new(),
    })
}
pub(crate) fn validate_history_cut(
    store: &dyn ConversationStore,
    cut: &CheckpointReadCut,
) -> Result<(), RuntimeClientError> {
    let source = cut
        .source_cut
        .as_ref()
        .ok_or(RuntimeClientError::ReadCutExpired)?;
    if cut.conversation_id != *store.conversation_id()
        || source.read.conversation_id != cut.conversation_id
        || source.read.journal != cut.through
        || source.read.ledger_through != source.ledger_through
        || source.read.surface_revision != source.surface_revision
        || source.read.transcript_total != source.transcript_total
    {
        return Err(RuntimeClientError::ReadCutExpired);
    }
    let current = store
        .observation_read_cut()
        .map_err(RuntimeClientError::observation_store_error)?;
    if source.read.mutation_revision != current.read.mutation_revision
        || source.read.journal > current.read.journal
        || source.ledger_through > current.ledger_through
        || source.read.transcript > current.read.transcript
    {
        return Err(RuntimeClientError::ReadCutExpired);
    }
    Ok(())
}
pub(crate) fn detail_domain(record_id: &str) -> Result<String, RuntimeClientError> {
    let sequence = record_id
        .strip_prefix("trace:")
        .and_then(|id| id.parse::<u64>().ok())
        .filter(|sequence| *sequence > 0)
        .ok_or_else(|| RuntimeClientError::InvalidRequest {
            message: "invalid native Trace record identity".into(),
        })?;
    Ok(format!("trace.records.{sequence}.detail"))
}
pub(crate) fn history_detail(
    store: &dyn ConversationStore,
    cut: &CheckpointReadCut,
    record_id: &str,
) -> Result<CheckpointPage, RuntimeClientError> {
    history_page(store, cut, &detail_domain(record_id)?, 0, 32)
}
pub(crate) fn history_page(
    store: &dyn ConversationStore,
    cut: &CheckpointReadCut,
    domain: &str,
    offset: u64,
    limit: usize,
) -> Result<CheckpointPage, RuntimeClientError> {
    validate_history_cut(store, cut)?;
    if canonical_selection(domain) {
        return super::history_observation::page(store, cut, domain, offset, limit);
    }
    if messages_selection(domain).is_some() {
        return messages_page(store, cut, domain, offset, limit);
    }
    if context_selection(domain).is_some() {
        return context_page(store, cut, domain, offset, limit);
    }
    if super::delivery::selection(domain).is_some() {
        return super::delivery::page(store, cut, domain, offset, limit);
    }
    if super::occupancy::selection(domain).is_some() {
        return super::occupancy::page(store, cut, domain, offset, limit);
    }
    if super::journal_observation::selection(domain).is_some() {
        return super::journal_observation::page(store, cut, domain, offset, limit);
    }
    let (sequence, path) = selection(domain).ok_or_else(|| RuntimeClientError::InvalidRequest {
        message: "invalid native Trace domain".into(),
    })?;
    if limit == 0 || limit > 32 || domain.len() > 256 || path.len() > 32 {
        return Err(RuntimeClientError::InvalidRequest {
            message: "invalid Trace page bounds".into(),
        });
    }
    if path.is_empty() {
        page(store, cut, domain, sequence, offset)
    } else {
        field_page(store, cut, domain, sequence, &path, offset, limit)
    }
}
pub(crate) fn history_value_read(
    store: &dyn ConversationStore,
    cut: &CheckpointReadCut,
    domain: &str,
    item_offset: u64,
    path: &[String],
    range: (u64, u32),
) -> Result<CheckpointValueChunk, RuntimeClientError> {
    validate_history_cut(store, cut)?;
    let (offset, max_bytes) = range;
    if domain.len() > 256
        || path.len() > 32
        || path.iter().any(|part| part.len() > 256)
        || max_bytes == 0
        || max_bytes > 65536
        || offset > 9_007_199_254_740_991
        || item_offset > 9_007_199_254_740_991
    {
        return Err(RuntimeClientError::InvalidRequest {
            message: "invalid Trace read bounds".into(),
        });
    }
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
            offset,
            max_bytes,
        );
    }
    if super::attempt_identity::selection(domain).is_some() {
        return super::attempt_identity::value_read(
            store,
            cut,
            domain,
            item_offset,
            path,
            offset,
            max_bytes,
        );
    }
    if super::occupancy::selection(domain).is_some() {
        return super::occupancy::value_read(
            store,
            cut,
            domain,
            item_offset,
            path,
            offset,
            max_bytes,
        );
    }
    if super::journal_observation::selection(domain).is_some() {
        return super::journal_observation::value_read(
            store,
            cut,
            domain,
            item_offset,
            path,
            offset,
            max_bytes,
        );
    }
    let (sequence, selected) =
        selection(domain).ok_or_else(|| RuntimeClientError::InvalidRequest {
            message: "invalid native Trace domain".into(),
        })?;
    if selected.is_empty() {
        read(
            store,
            cut,
            domain,
            sequence,
            (item_offset, path),
            (offset, max_bytes),
        )
    } else {
        field_read(
            store,
            cut,
            domain,
            sequence,
            &selected,
            (item_offset, path),
            (offset, max_bytes),
        )
    }
}
