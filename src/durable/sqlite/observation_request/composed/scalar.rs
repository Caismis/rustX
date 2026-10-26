//! Small request scalars retain authenticated original bytes. Field geometry and
//! previews are derived only by explicit inspection, using the existing Source.
use super::{Connection, ConversationStoreError, Fact, Fragment, fragments, index, storage};
use crate::runtime::read_layout::{FieldPreview, FieldPreviewPolicy, SeekableObservationSource};
use base64::Engine as _;
use serde::{Deserialize, Serialize};

const MAX_BYTES: u32 = 16_384;
#[cfg(test)]
#[path = "scalar_tests.rs"]
mod tests;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum Scalar {
    Indexed {
        fragment: Fragment,
    },
    Original {
        bytes: index::Reference,
        length: u64,
    },
}
impl Scalar {
    pub(super) fn validate_extent(&self) -> Result<(), ConversationStoreError> {
        if matches!(self, Self::Original { length, .. } if *length == 0 || *length > u64::from(MAX_BYTES))
        {
            return Err(storage("original scalar extent exceeds bound"));
        }
        Ok(())
    }
    pub(super) fn length(&self) -> u64 {
        match self {
            Self::Indexed { fragment } => fragment.length,
            Self::Original { length, .. } => *length,
        }
    }
}

fn validate(
    source: &SeekableObservationSource,
    bytes: &[u8],
) -> Result<(), ConversationStoreError> {
    // IgnoredAny syntax checking alone would accept numbers/depth that the
    // previous eager Fact capture refused. Count visits the complete original,
    // including duplicate entries, without allocating a JSON value tree.
    super::super::original_node_count(bytes)?;
    source.validate_unique_field_paths().map_err(storage)?;
    let mut failure = None;
    source
        .visit_field_geometry(|field| {
            let checked = (|| {
                super::super::field_path(field.path)?;
                if field.path.last().is_some_and(|part| part.starts_with('@')) {
                    let mut key_path = field.path.to_vec();
                    key_path.push("$key".into());
                    super::super::field_path(&key_path)?;
                }
                Ok(())
            })();
            checked.map_err(|error| {
                failure = Some(error);
                "original scalar field path unavailable"
            })
        })
        .map_err(|error| failure.unwrap_or_else(|| storage(error)))
}

fn original(
    connection: &Connection,
    source: &SeekableObservationSource,
    length: u64,
) -> Result<Scalar, ConversationStoreError> {
    let reader = source
        .byte_reader()
        .map_err(|error| storage(error.to_string()))?;
    let bytes = reader
        .indexed_span(0, length, MAX_BYTES)
        .map_err(|error| storage(error.to_string()))?;
    validate(source, &bytes)?;
    Ok(Scalar::Original {
        bytes: fragments::persist_bytes(connection, &reader, 0, length)?,
        length,
    })
}

pub(super) fn capture(
    connection: &Connection,
    source: &SeekableObservationSource,
) -> Result<(Scalar, u64, u64), ConversationStoreError> {
    let length = source
        .read(&[], 0, 1)
        .map_err(|error| storage(error.to_string()))?
        .readable_end;
    if length > u64::from(MAX_BYTES) {
        let (fragment, start, end) = fragments::scalar(connection, source)?;
        return Ok((Scalar::Indexed { fragment }, start, end));
    }
    let mut messages = None;
    source
        .visit_field_geometry(|field| {
            if field.path == ["messages"] {
                messages = Some((field.start, field.end));
            }
            Ok(())
        })
        .map_err(storage)?;
    let (start, end) = messages.ok_or_else(|| storage("composed producer field unavailable"))?;
    Ok((original(connection, source, length)?, start, end))
}

#[cfg(test)]
pub(super) fn whole(
    connection: &Connection,
    source: &SeekableObservationSource,
) -> Result<Scalar, ConversationStoreError> {
    let length = source
        .read(&[], 0, 1)
        .map_err(|error| storage(error.to_string()))?
        .readable_end;
    if length > u64::from(MAX_BYTES) {
        Ok(Scalar::Indexed {
            fragment: fragments::whole(connection, source)?,
        })
    } else {
        original(connection, source, length)
    }
}

pub(super) struct Original(SeekableObservationSource);
impl Original {
    pub(super) fn load(
        connection: &Connection,
        scalar: &Scalar,
    ) -> Result<Option<Self>, ConversationStoreError> {
        let Scalar::Original { bytes, length } = scalar else {
            return Ok(None);
        };
        scalar.validate_extent()?;
        // The same bounded page decoder verifies hashes and original chunk
        // lengths before bytes can become a navigable Source.
        let bytes = fragments::indexed_bytes(connection, bytes, *length, 0, *length)?;
        let source = SeekableObservationSource::from_bounded_bytes(&bytes).map_err(storage)?;
        validate(&source, &bytes)?;
        Ok(Some(Self(source)))
    }

    pub(super) fn fact(&self, path: &[String]) -> Result<Option<Fact>, ConversationStoreError> {
        let key = path.last().is_some_and(|part| part == "$key");
        let target = if key { &path[..path.len() - 1] } else { path };
        let mut selected = None;
        let mut failure = None;
        let reader = self
            .0
            .byte_reader()
            .map_err(|error| storage(error.to_string()))?;
        self.0
            .visit_metadata_selecting_previews(
                |field| {
                    if field.path == target && !key {
                        FieldPreviewPolicy::Inline
                    } else {
                        FieldPreviewPolicy::Skip
                    }
                },
                |field, preview| {
                    if field.path != target {
                        return Ok(());
                    }
                    let result = (|| {
                        if key {
                            if !target.last().is_some_and(|part| part.starts_with('@')) {
                                return Ok(None);
                            }
                            let Some((start, end)) = field.key_span else {
                                return Ok(None);
                            };
                            return Ok(Some(Fact {
                                path: path.to_vec(),
                                parent: None,
                                ordinal: None,
                                start,
                                end,
                                children: 0,
                                nodes: 1,
                                object: false,
                                container: false,
                                header: base64::engine::general_purpose::STANDARD.encode(b"\"\""),
                                key: None,
                                name: None,
                            }));
                        }
                        let FieldPreview::Inline(value) = preview else {
                            return Err(storage("original scalar preview unavailable"));
                        };
                        fragments::field_fact(&reader, &field, value, 0).map(Some)
                    })();
                    match result {
                        Ok(value) => {
                            selected = value;
                            Ok(())
                        }
                        Err(error) => {
                            failure = Some(error);
                            Err("original scalar fact unavailable")
                        }
                    }
                },
            )
            .map_err(|error| failure.unwrap_or_else(|| storage(error)))?;
        Ok(selected)
    }

    pub(super) fn child(
        &self,
        path: &[String],
        ordinal: u64,
    ) -> Result<Option<Vec<String>>, ConversationStoreError> {
        let mut selected = None;
        self.0
            .visit_field_geometry(|field| {
                if field
                    .parent
                    .is_some_and(|(parent, _, child)| parent == path && child == ordinal)
                {
                    selected = Some(field.path.to_vec());
                }
                Ok(())
            })
            .map_err(storage)?;
        Ok(selected)
    }

    pub(super) fn bytes(&self, start: u64, end: u64) -> Result<Vec<u8>, ConversationStoreError> {
        self.0
            .byte_reader()
            .map_err(|error| storage(error.to_string()))?
            .indexed_span(start, end, MAX_BYTES)
            .map_err(|error| storage(error.to_string()))
    }
}
