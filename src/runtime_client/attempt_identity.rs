//! Selected original inherited attempt fields under an immutable native cut.
use super::RuntimeClientError;
use super::checkpoint::{CheckpointReadCut, CheckpointValueChunk, CheckpointValueRepresentation};
use super::response::InheritedAttemptIdentityKind as Kind;
use crate::durable::ConversationStore;

pub(crate) fn selection(domain: &str) -> Option<(Kind, u64)> {
    let domain = domain
        .strip_prefix("transcript.")?
        .strip_suffix(".identity")?;
    let (kind, ordinal) = domain.split_once('.')?;
    let kind = match kind {
        "turns" => Kind::Turn,
        "responses" => Kind::Response,
        _ => return None,
    };
    let ordinal = ordinal.parse::<u64>().ok()?;
    (ordinal <= super::content::CONTENT_SAFE_INTEGER_MAX).then_some((kind, ordinal))
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
    let (kind, ordinal) = selection(domain).ok_or_else(|| RuntimeClientError::InvalidRequest {
        message: "invalid inherited owner field".into(),
    })?;
    if item_offset != 0 || path != ["attempt_id"] {
        return Err(RuntimeClientError::InvalidRequest {
            message: "inherited owner requires its advertised field".into(),
        });
    }
    if &cut.conversation_id != store.conversation_id() {
        return Err(RuntimeClientError::ContentSourceUnavailable);
    }
    let native = cut
        .source_cut
        .as_ref()
        .ok_or(RuntimeClientError::ReadCutExpired)?;
    if native.read.journal != cut.through || native.read.conversation_id != cut.conversation_id {
        return Err(RuntimeClientError::ReadCutExpired);
    }
    let chunk = store
        .observation_inherited_attempt_read(native, kind, ordinal, offset, max_bytes)
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
