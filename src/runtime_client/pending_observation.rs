//! Native pending-body range composition. Root routing adds only finite source
//! metadata; the original request bytes stay in the originating coordinator.
use super::checkpoint::{CheckpointReadCut, CheckpointValueChunk};
use super::snapshot::RuntimeClientInteractionHeader;
use super::types::RuntimeClientError;
use base64::Engine as _;

fn invalid() -> RuntimeClientError {
    RuntimeClientError::InvalidRequest {
        message: "invalid pending body range".into(),
    }
}
fn prefix(header: &RuntimeClientInteractionHeader) -> Result<Vec<u8>, RuntimeClientError> {
    let mut value = b"{\"interaction\":".to_vec();
    value.extend(super::checkpoint::bounded_bytes(&header.interaction, 4096).ok_or_else(invalid)?);
    value.extend(b",\"source\":");
    value.extend(super::checkpoint::bounded_bytes(&header.source, 4096).ok_or_else(invalid)?);
    value.extend(b",\"request\":");
    Ok(value)
}
pub(super) struct RoutedRange {
    prefix: Vec<u8>,
    source_end: u64,
    pub native_offset: u64,
    pub native_max_bytes: u32,
    pub readable_end: u64,
    offset: u64,
    max_bytes: u32,
}
pub(super) fn routed_range(
    header: &RuntimeClientInteractionHeader,
    source_end: u64,
    offset: u64,
    max_bytes: u32,
) -> Result<RoutedRange, RuntimeClientError> {
    if max_bytes == 0 || max_bytes > 65_536 {
        return Err(invalid());
    }
    let prefix = prefix(header)?;
    let p = prefix.len() as u64;
    let readable_end = p
        .checked_add(source_end)
        .and_then(|end| end.checked_add(1))
        .filter(|end| *end <= 9_007_199_254_740_991)
        .ok_or_else(invalid)?;
    if offset > readable_end {
        return Err(invalid());
    }
    let end = offset
        .checked_add(u64::from(max_bytes))
        .ok_or_else(invalid)?
        .min(readable_end);
    let start = offset.saturating_sub(p).min(source_end);
    let finish = end.saturating_sub(p).min(source_end);
    Ok(RoutedRange {
        prefix,
        source_end,
        native_offset: start,
        native_max_bytes: u32::try_from(finish - start).map_err(|_| invalid())?.max(1),
        readable_end,
        offset,
        max_bytes,
    })
}
pub(super) fn compose_routed(
    range: &RoutedRange,
    native_bytes: &[u8],
) -> Result<(Vec<u8>, u64, Option<u64>), RuntimeClientError> {
    let mut bytes = Vec::new();
    let end = range
        .offset
        .saturating_add(u64::from(range.max_bytes))
        .min(range.readable_end);
    let p = range.prefix.len() as u64;
    if range.offset < p {
        bytes.extend_from_slice(
            &range.prefix[usize::try_from(range.offset).map_err(|_| invalid())?
                ..usize::try_from(end.min(p)).map_err(|_| invalid())?],
        );
    }
    let wanted = end
        .saturating_sub(p)
        .min(range.source_end)
        .saturating_sub(range.native_offset);
    if (native_bytes.len() as u64) < wanted {
        return Err(invalid());
    }
    bytes.extend_from_slice(&native_bytes[..usize::try_from(wanted).map_err(|_| invalid())?]);
    if end > p + range.source_end && range.offset <= p + range.source_end {
        bytes.push(b'}');
    }
    let next = range
        .offset
        .checked_add(bytes.len() as u64)
        .ok_or_else(invalid)?;
    if range.offset < range.readable_end && next <= range.offset {
        return Err(invalid());
    }
    Ok((
        bytes,
        range.readable_end,
        (next < range.readable_end).then_some(next),
    ))
}
pub(super) fn checkpoint_chunk(
    cut: &CheckpointReadCut,
    domain: &str,
    item_offset: u64,
    path: &[String],
    offset: u64,
    payload: (Vec<u8>, u64, Option<u64>),
) -> CheckpointValueChunk {
    let (bytes, readable_end, next_offset) = payload;
    CheckpointValueChunk {
        representation:
            crate::runtime_client::checkpoint::CheckpointValueRepresentation::CanonicalJsonV1,
        cut: cut.clone(),
        domain: domain.into(),
        item_offset,
        path: path.to_vec(),
        offset,
        bytes_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
        readable_end,
        next_offset,
    }
}
