//! Client-neutral completed-response projections over native durable evidence.

pub(crate) use rustx_durable::response_completed::*;

#[cfg(test)]
use crate::durable::reading::TurnProcessOutcome;
#[cfg(test)]
use crate::events::types::RuntimeEvent;
#[cfg(test)]
use crate::message::types::{AssistantContentBlock, MessageBlock};
#[cfg(test)]
use crate::model::finish::ModelFinishReason;
#[cfg(test)]
use crate::model::types::ModelUsage;
#[cfg(test)]
use crate::observation::statistics::ConversationStatistics;
#[cfg(test)]
use crate::observation::transcript::{RuntimeClientTranscriptItem, RuntimeClientTranscriptPage};
#[cfg(test)]
use crate::observation::turn::CompletedResponseView;
#[cfg(test)]
use crate::runtime::identity::{AttemptId, MessageId};

/// Read one page of the checkpoint index at `cut` for `domain`.
///
/// # Errors
/// Returns [`RuntimeClientError`](crate::runtime_client::types::RuntimeClientError) when the store read fails.
pub fn identity_page(
    store: &dyn ConversationStore,
    cut: &crate::observation::cut::CheckpointReadCut,
    domain: &str,
    cursor: u64,
    offset: u64,
    limit: usize,
) -> Result<
    crate::runtime_client::checkpoint::CheckpointPage,
    crate::runtime_client::types::RuntimeClientError,
> {
    use crate::runtime_client::types::RuntimeClientError;
    let source_cut = cut
        .source_cut
        .as_ref()
        .ok_or(RuntimeClientError::ReadCutExpired)?;
    if limit == 0 || limit > 32 || cursor > source_cut.read.transcript {
        return Err(RuntimeClientError::InvalidRequest {
            message: "invalid response identity page bounds".into(),
        });
    }
    let source = store
        .response_identity_source(source_cut, cursor)
        .map_err(RuntimeClientError::observation_store_error)?
        .ok_or(RuntimeClientError::ContentSourceUnavailable)?;
    let fields = store
        .response_identity_fields(source_cut, &source)
        .map_err(RuntimeClientError::observation_store_error)?;
    let total = fields.len() as u64;
    let start = usize::try_from(offset)
        .ok()
        .filter(|offset| *offset <= fields.len())
        .ok_or_else(|| RuntimeClientError::InvalidRequest {
            message: "response identity page exceeds frontier".into(),
        })?;
    let end = start.saturating_add(limit).min(fields.len());
    let mut items = Vec::new();
    for (index, field) in fields[start..end].iter().enumerate() {
        let value = if field.readable_end <= 256 {
            let bytes = store
                .response_identity_read(source_cut, &source, &field.path, 0, 256)
                .map_err(RuntimeClientError::observation_store_error)?;
            Some(serde_json::Value::String(
                String::from_utf8(bytes.bytes).map_err(|_| {
                    RuntimeClientError::ObservationUnavailable {
                        message: "native response identity bytes are unavailable".into(),
                    }
                })?,
            ))
        } else {
            None
        };
        items.push(crate::observation::cut::CheckpointPageItem {
            offset: offset + index as u64,
            identity: field.path.join("."),
            deferred: value.is_none(),
            value,
            body_complete: None,
            key: None,
            content: Vec::new(),
        });
    }
    Ok(crate::runtime_client::checkpoint::CheckpointPage {
        read_cut: cut.clone(),
        cursor: crate::runtime_client::types::RuntimeClientCursor::new(cut.revision),
        domain: domain.into(),
        total,
        items,
        next_offset: (end < fields.len()).then_some(end as u64),
        collections: Vec::new(),
    })
}
/// Read one checkpoint value chunk at `cut` for `domain`.
///
/// # Errors
/// Returns [`RuntimeClientError`](crate::runtime_client::types::RuntimeClientError) when the store read fails.
pub fn identity_read(
    store: &dyn ConversationStore,
    cut: &crate::observation::cut::CheckpointReadCut,
    domain: &str,
    cursor: u64,
    item_offset: u64,
    range: (u64, u32),
) -> Result<
    crate::runtime_client::checkpoint::CheckpointValueChunk,
    crate::runtime_client::types::RuntimeClientError,
> {
    use crate::runtime_client::types::RuntimeClientError;
    use base64::Engine as _;
    let source_cut = cut
        .source_cut
        .as_ref()
        .ok_or(RuntimeClientError::ReadCutExpired)?;
    if cursor > source_cut.read.transcript {
        return Err(RuntimeClientError::InvalidRequest {
            message: "response identity exceeds transcript frontier".into(),
        });
    }
    let source = store
        .response_identity_source(source_cut, cursor)
        .map_err(RuntimeClientError::observation_store_error)?
        .ok_or(RuntimeClientError::ContentSourceUnavailable)?;
    let fields = store
        .response_identity_fields(source_cut, &source)
        .map_err(RuntimeClientError::observation_store_error)?;
    let field = usize::try_from(item_offset)
        .ok()
        .and_then(|index| fields.get(index))
        .ok_or_else(|| RuntimeClientError::InvalidRequest {
            message: "response identity field exceeds frontier".into(),
        })?;
    let chunk = store
        .response_identity_read(source_cut, &source, &field.path, range.0, range.1)
        .map_err(RuntimeClientError::observation_store_error)?;
    Ok(crate::runtime_client::checkpoint::CheckpointValueChunk {
        cut: cut.clone(),
        domain: domain.into(),
        item_offset,
        path: Vec::new(),
        offset: range.0,
        bytes_base64: base64::engine::general_purpose::STANDARD.encode(chunk.bytes),
        readable_end: chunk.readable_end,
        next_offset: chunk.next_offset,
        representation: crate::runtime_client::checkpoint::CheckpointValueRepresentation::Utf8Text,
    })
}
use crate::durable::ConversationStore;

#[cfg(test)]
pub(crate) mod tests;
