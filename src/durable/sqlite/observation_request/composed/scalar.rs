//! Exact, eagerly indexed scalar suffixes shared by immutable request roots.
//! Identity is the full allowlisted stable encoding, not a live configuration ID.
use super::{
    Connection, ConversationStoreError, Fragment, add, bounded, fragments, index, params, storage,
};
use crate::model::RequestSnapshot;
use crate::runtime::read_layout::{EncodedObservationSource, SeekableObservationSource};
use crate::runtime_client::trace::frozen;
use rusqlite::OptionalExtension as _;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Stable {
    identity: index::Hash,
    encoded_length: u64,
    start: u64,
    fragment: Fragment,
    messages_start: u64,
    messages_end: u64,
    children: u64,
}
pub(super) struct Captured {
    pub prefix: Fragment,
    pub suffix: Fragment,
    pub messages_start: u64,
    pub messages_end: u64,
    pub children: u64,
    pub nodes: u64,
}
fn checksum(bytes: &[u8]) -> index::Hash {
    use sha2::Digest as _;
    let mut digest = sha2::Sha256::new();
    digest.update(b"rustx-composed-stable-scalar-v1\0");
    digest.update(bytes);
    digest.finalize().into()
}
fn load(
    connection: &Connection,
    identity: index::Hash,
    length: u64,
    expected_start: u64,
) -> Result<Option<Stable>, ConversationStoreError> {
    type Row = (Option<Vec<u8>>, Option<Vec<u8>>);
    let row: Option<Row> = connection.prepare_cached("SELECT CASE WHEN typeof(metadata)='blob' AND length(metadata)<=16384 THEN metadata END,CASE WHEN typeof(checksum)='blob' AND length(checksum)=32 THEN checksum END FROM request_source_scalars WHERE identity=?1")
        .map_err(|error| storage(error.to_string()))?.query_row([identity.as_slice()], |row| Ok((row.get(0)?, row.get(1)?))).optional().map_err(|error| storage(error.to_string()))?;
    let (bytes, digest) = match row {
        None => return Ok(None),
        Some((Some(bytes), Some(digest))) => (bytes, digest),
        _ => return Err(storage("stable scalar metadata unavailable")),
    };
    if checksum(&bytes).as_slice() != digest {
        return Err(storage("stable scalar checksum differs"));
    }
    let stable: Stable = super::record::decode(&bytes)?;
    if stable.identity != identity
        || stable.encoded_length != length
        || stable.start != expected_start
        || stable.start >= length
        || stable.fragment.length != length - stable.start
        || stable.messages_start >= stable.messages_end
        || stable.messages_end - stable.messages_start != 2
        || stable.messages_end > stable.fragment.length
        || stable.children != 24
    {
        return Err(storage("stable scalar identity or geometry differs"));
    }
    Ok(Some(stable))
}
fn geometry(
    source: &SeekableObservationSource,
) -> Result<(u64, u64, u64, u64), ConversationStoreError> {
    let (mut prefix_end, mut messages, mut children) = (None, None, None);
    source
        .visit_fields_selecting_previews(
            |_| crate::runtime::read_layout::FieldPreviewPolicy::Skip,
            |path, _, _, start, end, count, _, _| {
                if path.is_empty() {
                    children = Some(count);
                }
                if path == ["assistant_message_id"] {
                    prefix_end = Some(end);
                }
                if path == ["messages"] {
                    messages = Some((start, end));
                }
                Ok(())
            },
        )
        .map_err(storage)?;
    match (prefix_end, messages, children) {
        (Some(prefix), Some((start, end)), Some(children)) => Ok((prefix, start, end, children)),
        _ => Err(storage("stable scalar geometry unavailable")),
    }
}
fn capture_stable(
    connection: &Connection,
    encoded: EncodedObservationSource,
    prefix_length: u64,
    expected_start: u64,
) -> Result<(Stable, Option<Vec<u8>>), ConversationStoreError> {
    let identity = encoded.digest();
    let encoded_length = encoded.length();
    if let Some(stable) = load(connection, identity, encoded_length, expected_start)? {
        let messages_start = add(stable.start, stable.messages_start)?;
        if encoded
            .bounded_bytes(messages_start, add(messages_start, 2)?)
            .map_err(storage)?
            != b"[]"
        {
            return Err(storage("stable scalar messages geometry differs"));
        }
        #[cfg(test)]
        super::WORK.with(|work| {
            let mut value = work.get();
            value.scalar_reuses += 1;
            work.set(value);
        });
        let tail = (add(prefix_length, stable.fragment.length)? <= 16384)
            .then(|| {
                encoded
                    .bounded_bytes(stable.start, encoded_length)
                    .map_err(storage)
            })
            .transpose()?;
        return Ok((stable, tail));
    }
    // Only a new exact encoding builds navigation and authenticated F/C pages.
    let source = encoded.index().map_err(storage)?;
    #[cfg(test)]
    super::WORK.with(|work| {
        let mut value = work.get();
        value.scalar_captures += 1;
        work.set(value);
    });
    let (start, messages_start, messages_end, children) = geometry(&source)?;
    if start != expected_start || children != 24 || messages_end - messages_start != 2 {
        return Err(storage("stable scalar prefix or messages geometry differs"));
    }
    let reader = source
        .byte_reader()
        .map_err(|error| storage(error.to_string()))?;
    let fragment = fragments::persist_part(connection, &source, &reader, start, encoded_length)?;
    let tail = (add(prefix_length, fragment.length)? <= 16384)
        .then(|| {
            reader
                .indexed_span(start, encoded_length, 16384)
                .map_err(|error| storage(error.to_string()))
        })
        .transpose()?;
    let stable = Stable {
        identity,
        encoded_length,
        start,
        fragment,
        messages_start: messages_start - start,
        messages_end: messages_end - start,
        children,
    };
    let bytes = super::record::encode(&stable, 16384)?;
    connection
        .execute(
            "INSERT INTO request_source_scalars(identity,metadata,checksum) VALUES(?1,?2,?3)",
            params![identity.as_slice(), bytes, checksum(&bytes).as_slice()],
        )
        .map_err(|error| storage(error.to_string()))?;
    Ok((stable, tail))
}
pub(super) fn capture(
    connection: &Connection,
    snapshot: &RequestSnapshot,
) -> Result<Captured, ConversationStoreError> {
    let source =
        SeekableObservationSource::new(&frozen::request_prefix(snapshot)).map_err(storage)?;
    let length = source
        .read(&[], 0, 1)
        .map_err(|error| storage(error.to_string()))?
        .readable_end;
    let end = length
        .checked_sub(1)
        .ok_or_else(|| storage("scalar prefix unavailable"))?;
    let reader = source
        .byte_reader()
        .map_err(|error| storage(error.to_string()))?;
    let prefix = fragments::persist_part(connection, &source, &reader, 0, end)?;
    let encoded =
        EncodedObservationSource::new(&frozen::request_stable(snapshot)).map_err(storage)?;
    // This tiny all-null prefix uses the same serializer allowlist. Its bytes
    // cannot depend on the snapshot; no structural u64 conversion is involved.
    let fixed_prefix = bounded(&frozen::request_stable_prefix(snapshot), 1024)?;
    let expected_start = fixed_prefix
        .len()
        .checked_sub(1)
        .ok_or_else(|| storage("stable prefix unavailable"))? as u64;
    let (stable, tail) = capture_stable(connection, encoded, prefix.length, expected_start)?;
    // Whole-root validation remains based on this request's exact original bytes.
    // Splitting the source must not weaken serde numeric/depth admission at 16KiB.
    let nodes = if let Some(tail) = tail {
        let mut bytes = reader
            .indexed_span(0, end, 16384)
            .map_err(|error| storage(error.to_string()))?;
        bytes.extend_from_slice(&tail);
        super::super::original_node_count(&bytes)?
    } else {
        33
    };
    Ok(Captured {
        messages_start: add(prefix.length, stable.messages_start)?,
        messages_end: add(prefix.length, stable.messages_end)?,
        prefix,
        suffix: stable.fragment,
        children: stable.children,
        nodes,
    })
}
/// The prefix serializer owns exactly these six ordered fields. Nested aliases
/// keep their original paths; this routing never interprets user-controlled keys.
pub(super) fn prefix_path(path: &[String]) -> bool {
    path.first().is_some_and(|name| {
        matches!(
            name.as_str(),
            "contributions"
                | "request_id"
                | "attempt_id"
                | "step_id"
                | "retry_number"
                | "assistant_message_id"
        )
    })
}

#[cfg(test)]
pub(super) fn corrupted_record(bytes: &[u8], field: &str) -> Vec<u8> {
    let mut metadata: Stable = super::record::decode(bytes).unwrap();
    match field {
        "identity" => metadata.identity = [0; 32],
        "children" => metadata.children = 25,
        "start" => metadata.start = 1,
        "messages_end" => metadata.messages_end = 1,
        "messages_start" => metadata.messages_start = 0,
        _ => unreachable!(),
    }
    super::record::encode(&metadata, 16384).unwrap()
}
