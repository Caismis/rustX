//! Small Journal Sources share the authoritative event body through an authenticated
//! quote overlay. Navigation retains the exact structural bytes and resident bound.
use super::{Connection, ConversationStoreError, OptionalExtension, storage};
use crate::durable::observation::{
    ObservationValueChunk, RequestObservationField, RequestObservationKey, RequestObservationPage,
};
use crate::runtime::read_layout::{FieldPreview, FieldPreviewPolicy, SeekableObservationSource};

mod overlay;
pub(super) const MAX_BYTES: usize = 16_384;
pub(super) fn encode_overlay(
    native: &[u8],
    source: &[u8],
) -> Result<Option<Vec<u8>>, ConversationStoreError> {
    overlay::encode(native, source)
}

pub(super) struct Source(pub(super) SeekableObservationSource);
type StoredOverlay = (Option<Vec<u8>>, Option<Vec<u8>>);

impl Source {
    pub(super) fn new(source: SeekableObservationSource) -> Result<Self, ConversationStoreError> {
        // The segmented layout rejects duplicate field paths through its primary
        // key. Keep that rule without retaining every full ancestor path: only
        // sibling components along the active traversal stack need ownership.
        let mut siblings: Vec<std::collections::HashSet<String>> = Vec::new();
        source
            .visit_field_geometry(|field| {
                let depth = field.path.len();
                siblings.truncate(depth);
                if let Some(component) = field.path.last() {
                    let seen = siblings
                        .last_mut()
                        .ok_or("inline event parent unavailable")?;
                    if !seen.insert(component.clone()) {
                        return Err("inline event field path repeats");
                    }
                }
                if field.container {
                    siblings.push(std::collections::HashSet::new());
                }
                Ok(())
            })
            .map_err(storage)?;
        Ok(Self(source))
    }
    pub(super) fn load(
        connection: &Connection,
        sequence: u64,
    ) -> Result<Self, ConversationStoreError> {
        // Both allocations have SQL byte bounds. The caller has selected the
        // overlay layout within its read transaction; losing that source cannot
        // select a different reader or hide malformed metadata.
        let row: Option<StoredOverlay> = connection
            .prepare_cached("SELECT CASE WHEN observation_source_overlay IS NULL THEN NULL WHEN typeof(observation_source_overlay)='blob' AND length(observation_source_overlay) BETWEEN 1 AND 16384 THEN observation_source_overlay ELSE 0 END, CASE WHEN observation_source_overlay IS NULL THEN NULL WHEN typeof(event_json)='text' AND octet_length(event_json) BETWEEN 1 AND 16384 THEN CAST(event_json AS BLOB) ELSE 0 END FROM events WHERE sequence=?1")
            .map_err(|error| storage(error.to_string()))?
            .query_row([sequence], |row| Ok((row.get(0)?, row.get(1)?)))
            .optional()
            .map_err(|error| storage(error.to_string()))?;
        match row {
            Some((Some(overlay), Some(native))) => {
                let bytes = overlay::restore(&native, &overlay)?;
                SeekableObservationSource::from_bounded_bytes(&bytes)
                    .map_err(storage)
                    .and_then(Self::new)
            }
            _ => Err(storage("event overlay has no native source")),
        }
    }

    pub(super) fn bytes(&self) -> Result<Vec<u8>, ConversationStoreError> {
        Ok(self
            .0
            .read(
                &[],
                0,
                u32::try_from(MAX_BYTES).map_err(|_| storage("inline event extent exceeds u32"))?,
            )
            .map_err(|error| storage(error.to_string()))?
            .bytes)
    }

    fn coordinates(&self, path: &[String]) -> Result<(u64, u64, u64), ConversationStoreError> {
        super::super::observation_request::field_path(path)?;
        let key = path.last().is_some_and(|part| part == "$key");
        let target = if key { &path[..path.len() - 1] } else { path };
        let mut found = None;
        self.0
            .visit_field_geometry(|field| {
                if field.path == target {
                    found = if key {
                        field
                            .path
                            .last()
                            .filter(|part| part.starts_with('@'))
                            .and_then(|_| field.key_span.map(|(start, end)| (start, end, 0)))
                    } else {
                        Some((field.start, field.end, field.children))
                    };
                }
                Ok(())
            })
            .map_err(storage)?;
        found.ok_or_else(|| {
            ConversationStoreError::InvalidReference("request field is unavailable".into())
        })
    }

    pub(super) fn field(
        &self,
        path: &[String],
    ) -> Result<(u64, u64, u64, Vec<u8>), ConversationStoreError> {
        let (start, end, children) = self.coordinates(path)?;
        if path.last().is_some_and(|part| part == "$key") {
            return Ok((start, end, children, b"\"\"".to_vec()));
        }
        let mut bytes = None;
        self.0
            .visit_fields_selecting_previews(
                |candidate| {
                    if candidate == path {
                        FieldPreviewPolicy::Inline
                    } else {
                        FieldPreviewPolicy::Skip
                    }
                },
                |candidate, _, _, _, _, _, _, preview| {
                    if candidate == path
                        && let FieldPreview::Inline(value) = preview
                    {
                        bytes = Some(
                            crate::runtime_client::checkpoint::bounded_bytes(value, MAX_BYTES)
                                .unwrap_or_else(|| b"null".to_vec()),
                        );
                    }
                    Ok(())
                },
            )
            .map_err(storage)?;
        Ok((
            start,
            end,
            children,
            bytes.ok_or_else(|| storage("inline event preview unavailable"))?,
        ))
    }

    pub(super) fn page(
        &self,
        path: &[String],
        offset: u64,
        limit: usize,
    ) -> Result<RequestObservationPage, ConversationStoreError> {
        if limit == 0 || limit > crate::durable::observation::HEADER_PAGE_MAX {
            return Err(ConversationStoreError::InvalidContentRange);
        }
        let (_, _, total) = self.coordinates(path)?;
        if offset > total {
            return Err(ConversationStoreError::InvalidContentRange);
        }
        let end = offset.saturating_add(limit as u64).min(total);
        let mut fields = Vec::new();
        let mut failure = None;
        let mut child_ordinal = 0_u64;
        self.0
            .visit_fields_selecting_previews(
                |candidate| {
                    if candidate.len() == path.len() + 1 && candidate.starts_with(path) {
                        let ordinal = child_ordinal;
                        child_ordinal += 1;
                        if ordinal >= offset && ordinal < end {
                            return FieldPreviewPolicy::Inline;
                        }
                    }
                    FieldPreviewPolicy::Skip
                },
                |candidate, _, parent, start, finish, children, key_span, preview| {
                    let Some((_, ordinal)) = parent else {
                        return Ok(());
                    };
                    if candidate.len() != path.len() + 1
                        || !candidate.starts_with(path)
                        || ordinal < offset
                        || ordinal >= end
                    {
                        return Ok(());
                    }
                    let FieldPreview::Inline(value) = preview else {
                        return Err("inline event preview unavailable");
                    };
                    let bytes = crate::runtime_client::checkpoint::bounded_bytes(value, MAX_BYTES)
                        .unwrap_or_else(|| b"null".to_vec());
                    let value = match serde_json::from_slice(&bytes) {
                        Ok(value) => value,
                        Err(error) => {
                            failure = Some(storage(error.to_string()));
                            return Err("inline event preview invalid");
                        }
                    };
                    let key = candidate
                        .last()
                        .filter(|part| part.starts_with('@'))
                        .and(key_span)
                        .map(|(start, end)| {
                            let mut key_path = candidate.to_vec();
                            key_path.push("$key".into());
                            RequestObservationKey {
                                path: key_path,
                                preview: String::new(),
                                encoded_bytes: end - start,
                            }
                        });
                    fields.push(RequestObservationField {
                        path: candidate.to_vec(),
                        value,
                        key,
                        content: Vec::new(),
                        collections: Vec::new(),
                        encoded_bytes: finish - start,
                        children_total: children,
                        deferred: finish - start > bytes.len() as u64,
                    });
                    Ok(())
                },
            )
            .map_err(|error| failure.unwrap_or_else(|| storage(error)))?;
        while crate::runtime_client::checkpoint::bounded_bytes(&fields, 524_288).is_none() {
            if fields.len() <= 1 {
                return Err(storage("native field descriptor cannot fit bounded page"));
            }
            fields.pop();
        }
        let next = offset
            .checked_add(fields.len() as u64)
            .ok_or(ConversationStoreError::InvalidContentRange)?;
        if next == offset && next < total {
            return Err(storage("request field page cannot advance"));
        }
        Ok(RequestObservationPage {
            total,
            offset,
            fields,
            next_offset: (next < total).then_some(next),
        })
    }

    pub(super) fn read(
        &self,
        path: &[String],
        offset: u64,
        max_bytes: u32,
    ) -> Result<ObservationValueChunk, ConversationStoreError> {
        let (start, end, _) = self.coordinates(path)?;
        let readable_end = end - start;
        if max_bytes == 0 || max_bytes > 65_536 || offset > readable_end {
            return Err(ConversationStoreError::InvalidContentRange);
        }
        let end_offset = offset
            .saturating_add(u64::from(max_bytes))
            .min(readable_end);
        let bytes = self
            .0
            .byte_reader()
            .map_err(|error| storage(error.to_string()))?
            .indexed_span(start + offset, start + end_offset, max_bytes)
            .map_err(|error| storage(error.to_string()))?;
        Ok(ObservationValueChunk {
            bytes,
            end_offset,
            readable_end,
            next_offset: (end_offset < readable_end).then_some(end_offset),
        })
    }
}
