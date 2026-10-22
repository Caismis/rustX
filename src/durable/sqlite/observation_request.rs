//! Original RequestStart-owned allowlisted structural inspection layout.
//! Request inspection composes authenticated fragments over the native selected
//! tree. Other finite state domains retain their original segmented layouts.
use super::{Connection, ConversationStoreError, OptionalExtension, params, sqlite_error, storage};
use crate::durable::observation::{
    HEADER_PAGE_MAX, ObservationValueChunk, RequestObservationField, RequestObservationHeader,
    RequestObservationPage,
};
use crate::runtime::identity::RequestId;
use std::io::Write;
pub(in crate::durable::sqlite) mod composed;
mod input;
pub(super) mod provenance;
const SEGMENT: usize = 65_536;
struct SourceWriter<'a> {
    connection: &'a Connection,
    id: &'a str,
    kind: &'static str,
    table: &'static str,
    buffer: Vec<u8>,
    written: u64,
    segment: u64,
    failure: Option<ConversationStoreError>,
}
impl SourceWriter<'_> {
    fn drain(&mut self) -> std::io::Result<()> {
        if !self.buffer.is_empty() {
            self.connection.execute("INSERT INTO content_segments(source_kind,source_id,block_index,representation,segment,bytes) VALUES(?1,?2,0,'canonical_json_v1',?3,?4)",params![self.kind,self.id,self.segment,self.buffer]).map_err(|error| {
                self.failure = Some(sqlite_error(&error, error.to_string()));
                std::io::Error::other("native observation source write failed")
            })?;
            self.written = self
                .written
                .checked_add(self.buffer.len() as u64)
                .filter(|value| *value <= crate::observation::content::CONTENT_SAFE_INTEGER_MAX)
                .ok_or_else(|| {
                    std::io::Error::other("native structural source exceeds safe offsets")
                })?;
            self.segment = self
                .segment
                .checked_add(1)
                .ok_or_else(|| std::io::Error::other("native structural segment exhausted"))?;
            self.buffer.clear();
        }
        Ok(())
    }
}
impl Write for SourceWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        for part in bytes.chunks(SEGMENT) {
            let mut at = 0;
            while at < part.len() {
                let count = (SEGMENT - self.buffer.len()).min(part.len() - at);
                self.buffer.extend_from_slice(&part[at..at + count]);
                at += count;
                if self.buffer.len() == SEGMENT {
                    self.drain()?;
                }
            }
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.drain()
    }
}
fn encoded_path(path: &[String]) -> Result<String, ConversationStoreError> {
    serde_json::to_string(path).map_err(|error| storage(error.to_string()))
}
pub(in crate::durable::sqlite) fn index_selection(
    connection: &Connection,
    source_sequence: u64,
    scope: &crate::runtime::identity::ConversationId,
    messages: &crate::model::SelectedModelInput,
    snapshot: &crate::model::RequestSnapshot,
    selection: &super::manifest::Selection,
) -> Result<(), ConversationStoreError> {
    composed::index_selection(
        connection,
        source_sequence,
        scope,
        messages,
        snapshot,
        selection,
    )
}
pub(super) fn index_state<T: serde::Serialize + schemars::JsonSchema>(
    connection: &Connection,
    kind: &'static str,
    id: &str,
    value: &T,
) -> Result<(), ConversationStoreError> {
    index_stream(connection, kind, id, "state_observation_fields", value)
}
fn index_stream<T: serde::Serialize + schemars::JsonSchema>(
    connection: &Connection,
    kind: &'static str,
    id: &str,
    table: &'static str,
    value: &T,
) -> Result<(), ConversationStoreError> {
    let source =
        crate::runtime::read_layout::SeekableObservationSource::new(value).map_err(storage)?;
    let mut writer = SourceWriter {
        connection,
        id,
        kind,
        table,
        buffer: Vec::with_capacity(SEGMENT),
        written: 0,
        segment: 0,
        failure: None,
    };
    source
        .copy_original_bytes_to(&mut writer)
        .map_err(|error| {
            writer
                .failure
                .take()
                .unwrap_or_else(|| storage(error.to_string()))
        })?;
    writer.flush().map_err(|error| {
        writer
            .failure
            .take()
            .unwrap_or_else(|| storage(error.to_string()))
    })?;
    index_fields(&writer, &source)?;
    connection
        .execute(
            "INSERT INTO content_frontiers VALUES(?1,?2,0,'canonical_json_v1',1,?3)",
            params![kind, id, writer.written],
        )
        .map_err(|error| sqlite_error(&error, error.to_string()))?;
    Ok(())
}
fn stored_preview(
    source: &crate::runtime::read_layout::SeekableObservationSource,
    path: &[String],
    start: u64,
    end: u64,
    preview: crate::runtime::read_layout::FieldPreview<'_>,
) -> Result<Vec<u8>, ConversationStoreError> {
    let crate::runtime::read_layout::FieldPreview::Inline(preview) = preview else {
        return Err(storage("uncaptured state field preview unavailable"));
    };
    let original_message = if path.len() == 2 && path[0] == "messages" && end - start <= 16_384 {
        Some(
            serde_json::from_slice(
                &source
                    .indexed_span(start, end, 16_384)
                    .map_err(|error| storage(error.to_string()))?,
            )
            .map_err(|error| storage(error.to_string()))?,
        )
    } else {
        None
    };
    Ok(crate::observation::bounded::bounded_bytes(
        original_message.as_ref().unwrap_or(preview),
        16_384,
    )
    .unwrap_or_else(|| b"null".to_vec()))
}
fn index_fields(
    writer: &SourceWriter<'_>,
    source: &crate::runtime::read_layout::SeekableObservationSource,
) -> Result<(), ConversationStoreError> {
    let (connection, id, table) = (writer.connection, writer.id, writer.table);
    let insert = format!(
        "INSERT INTO {table}(source_id,path_json,parent_path,ordinal,start,end,children,header_json) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)"
    );
    let mut fields = connection
        .prepare_cached(&insert)
        .map_err(|error| sqlite_error(&error, error.to_string()))?;
    let mut failure = None;
    source
        .visit_fields_selecting_previews(
            |_| crate::runtime::read_layout::FieldPreviewPolicy::Inline,
            |path, path_json, parent, start, end, children, key_span, preview| {
                let result = (|| {
                    let header = stored_preview(source, path, start, end, preview)?;
                    fields
                        .execute(params![
                            id,
                            path_json,
                            parent.map(|(path, _)| path),
                            parent.map(|(_, ordinal)| ordinal),
                            start,
                            end,
                            children,
                            header
                        ])
                        .map_err(|error| sqlite_error(&error, error.to_string()))?;
                    if let Some((start, end)) = key_span
                        && path.last().is_some_and(|part| part.starts_with('@'))
                    {
                        index_key(writer, path, "", start, end)?;
                    }
                    Ok(())
                })();
                result.map_err(|error| {
                    failure = Some(error);
                    "native persistent layout unavailable"
                })
            },
        )
        .map_err(|error| failure.unwrap_or_else(|| storage(error)))?;
    Ok(())
}
fn provenance_source(
    sequence: u64,
    item: u64,
    encoded_bytes: u64,
) -> crate::observation::trace::TraceFieldSource {
    crate::observation::trace::TraceFieldSource::Value {
        field: crate::observation::trace::TraceFieldKind::Provenance,
        domain: format!("trace.records.{sequence}.detail.request.messages"),
        item_offset: item,
        path: vec!["provenance".into()],
        encoded_bytes,
    }
}
#[cfg(test)]
thread_local! { static HEADER_CONSTRUCTIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
#[cfg(test)]
pub(super) fn take_header_constructions() -> usize {
    HEADER_CONSTRUCTIONS.with(|count| count.replace(0))
}
fn input_view(
    connection: &Connection,
    scope: &crate::runtime::identity::ConversationId,
    message: &crate::model::ModelInputMessage,
    frozen: &crate::model::projected::FrozenModelInput,
) -> Result<
    (
        crate::observation::trace::TraceRequestMessage,
        Option<crate::durable::observation::MessageHeader>,
    ),
    ConversationStoreError,
> {
    #[cfg(test)]
    HEADER_CONSTRUCTIONS.with(|count| count.set(count.get() + 1));
    let original = match frozen {
        crate::model::projected::FrozenModelInput::Original(original) => Some(original),
        _ => None,
    };
    let mut view = crate::observation::trace::frozen::input_message(scope, message, original);
    let header = message
        .canonical_id()
        .map(|id| {
            super::observation::message(connection, id)?
                .ok_or_else(|| storage("frozen canonical header unavailable"))
        })
        .transpose()?;
    if let Some(header) = &header {
        crate::observation::trace::request::bind_original_provenance(&mut view, header)?;
    }
    Ok((view, header))
}
#[cfg(test)]
pub(super) fn index_test_request_fields<T: serde::Serialize + schemars::JsonSchema>(
    connection: &Connection,
    sequence: u64,
    value: &T,
) -> Result<(), ConversationStoreError> {
    composed::index_scalar(connection, sequence, value)
}
#[cfg(test)]
pub(super) fn legacy_request_source_bytes(
    connection: &Connection,
    sequence: u64,
    scope: &crate::runtime::identity::ConversationId,
    messages: &crate::model::SelectedModelInput,
    snapshot: &crate::model::RequestSnapshot,
) -> Result<Vec<u8>, ConversationStoreError> {
    let capture = |_ordinal: u64, _bytes: &[u8]| Ok(());
    let value = crate::observation::trace::frozen_request_detail(
        snapshot,
        FrozenMessages {
            connection,
            scope,
            messages,
            snapshot,
            sequence,
            capture_header: &capture,
        },
    );
    let source =
        crate::runtime::read_layout::SeekableObservationSource::new(&value).map_err(storage)?;
    let mut bytes = Vec::new();
    let mut offset = 0;
    loop {
        let chunk = source
            .read(&[], offset, 65_536)
            .map_err(|error| storage(error.to_string()))?;
        bytes.extend_from_slice(&chunk.bytes);
        if let Some(next) = chunk.next_offset {
            offset = next;
        } else {
            return Ok(bytes);
        }
    }
}
#[cfg(test)]
type LegacyFieldHeader = (Vec<String>, u64, u64, u64, Vec<u8>);
#[cfg(test)]
pub(super) fn legacy_field_headers_oracle(
    connection: &Connection,
    sequence: u64,
    scope: &crate::runtime::identity::ConversationId,
    messages: &crate::model::SelectedModelInput,
    snapshot: &crate::model::RequestSnapshot,
) -> Result<Vec<LegacyFieldHeader>, ConversationStoreError> {
    let headers = std::cell::RefCell::new(Vec::new());
    let capture = |_ordinal: u64, bytes: &[u8]| {
        headers.borrow_mut().push(bytes.to_vec());
        Ok(())
    };
    let value = crate::observation::trace::frozen_request_detail(
        snapshot,
        FrozenMessages {
            connection,
            scope,
            messages,
            snapshot,
            sequence,
            capture_header: &capture,
        },
    );
    let source =
        crate::runtime::read_layout::SeekableObservationSource::new(&value).map_err(storage)?;
    let mut fields = Vec::new();
    source
        .visit_fields(
            |path, _encoded, _parent, start, end, children, key_span, preview| {
                let bytes = if path.len() == 2 && path[0] == "messages" {
                    headers.borrow()[path[1]
                        .parse::<usize>()
                        .map_err(|_| "legacy message ordinal invalid")?]
                    .clone()
                } else {
                    crate::observation::bounded::bounded_bytes(preview, 16_384)
                        .unwrap_or_else(|| b"null".to_vec())
                };
                fields.push((path.to_vec(), start, end, children, bytes));
                if let Some((start, end)) = key_span
                    && path.last().is_some_and(|part| part.starts_with('@'))
                {
                    let mut key = path.to_vec();
                    key.push("$key".into());
                    fields.push((key, start, end, 0, b"\"\"".to_vec()));
                }
                Ok(())
            },
        )
        .map_err(storage)?;
    Ok(fields)
}
#[cfg(test)]
type CaptureHeader<'a> = dyn Fn(u64, &[u8]) -> Result<(), ConversationStoreError> + 'a;
#[cfg(test)]
struct FrozenMessages<'a> {
    connection: &'a Connection,
    scope: &'a crate::runtime::identity::ConversationId,
    messages: &'a crate::model::SelectedModelInput,
    snapshot: &'a crate::model::RequestSnapshot,
    sequence: u64,
    capture_header: &'a CaptureHeader<'a>,
}
#[cfg(test)]
impl serde::Serialize for FrozenMessages<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::{Error as _, SerializeSeq as _};
        let mut sequence = serializer.serialize_seq(Some(self.messages.len()))?;
        for (ordinal, message) in self.messages.iter().enumerate() {
            let frozen = self
                .snapshot
                .input
                .get(ordinal)
                .ok_or_else(|| S::Error::custom("frozen request item absent"))?;
            let (mut view, header) = input_view(self.connection, self.scope, message, frozen)
                .map_err(S::Error::custom)?;
            let bytes = provenance::Raw::frozen(frozen, &view.provenance)
                .encoded_bytes()
                .map_err(S::Error::custom)?;
            view.provenance_source = Some(provenance_source(self.sequence, ordinal as u64, bytes));
            if let Some(header) = header {
                let (value, _) = message_value_capture(&header, &view).map_err(S::Error::custom)?;
                let bytes = crate::observation::bounded::bounded_bytes(&value, 16_384)
                    .ok_or_else(|| S::Error::custom("request item header exceeds bound"))?;
                (self.capture_header)(ordinal as u64, &bytes).map_err(S::Error::custom)?;
                sequence.serialize_element(&provenance::Canonical {
                    value: &value,
                    provenance: provenance::Raw::frozen(frozen, &view.provenance),
                })?;
            } else {
                let header = bounded_message_descriptor(&view, view.blocks.len(), false)
                    .map_err(S::Error::custom)?;
                let bytes = crate::observation::bounded::bounded_bytes(&header, 16_384)
                    .ok_or_else(|| S::Error::custom("request item header exceeds bound"))?;
                (self.capture_header)(ordinal as u64, &bytes).map_err(S::Error::custom)?;
                sequence.serialize_element(&input::Input {
                    original: message,
                    view: &view,
                })?;
            }
        }
        sequence.end()
    }
}

fn started(
    connection: &Connection,
    request: &RequestId,
    through: u64,
) -> Result<u64, ConversationStoreError> {
    let sequence:Option<u64>=connection.query_row("SELECT started_sequence FROM request_snapshots WHERE request_id=?1 AND started_sequence<=?2",params![request.as_str(),through],|row|row.get(0)).optional().map_err(|error|sqlite_error(&error, error.to_string()))?;
    sequence.ok_or_else(|| {
        ConversationStoreError::InvalidReference("request is outside native read cut".into())
    })
}
pub(super) fn verify_schema(connection: &Connection) -> Result<(), ConversationStoreError> {
    composed::verify_schema(connection)
}
fn field_path(path: &[String]) -> Result<String, ConversationStoreError> {
    if path.len() > 256
        || path.iter().any(|part| part.len() > 256)
        || encoded_path(path)?.len() > 4096
    {
        return Err(ConversationStoreError::InvalidReference(
            "request field locator exceeds bound".into(),
        ));
    }
    encoded_path(path)
}
pub(super) fn field_coordinates(
    connection: &Connection,
    table: &'static str,
    id: &str,
    path: &[String],
) -> Result<(u64, u64, u64), ConversationStoreError> {
    if table == "request_observation_fields" {
        field_path(path)?;
        return composed::coordinates(connection, id, path);
    }
    let coordinates: (u64, u64, u64) = connection
        .query_row(
            &format!("SELECT start,end,children FROM {table} WHERE source_id=?1 AND path_json=?2"),
            params![id, field_path(path)?],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|error| sqlite_error(&error, error.to_string()))?
        .ok_or_else(|| {
            ConversationStoreError::InvalidReference("request field is unavailable".into())
        })?;
    if coordinates.0 > coordinates.1
        || coordinates.1 > crate::observation::content::CONTENT_SAFE_INTEGER_MAX
    {
        return Err(storage("native field coordinates unavailable"));
    }
    Ok(coordinates)
}
struct StoredField {
    start: u64,
    end: u64,
    children: u64,
    bytes: Option<Vec<u8>>,
}
impl StoredField {
    fn row(row: &rusqlite::Row<'_>, offset: usize) -> rusqlite::Result<Self> {
        Ok(Self {
            start: row.get(offset)?,
            end: row.get(offset + 1)?,
            children: row.get(offset + 2)?,
            bytes: row.get(offset + 3)?,
        })
    }
    fn materialize(self) -> Result<(u64, u64, u64, Vec<u8>), ConversationStoreError> {
        if self.start > self.end || self.end > crate::observation::content::CONTENT_SAFE_INTEGER_MAX
        {
            return Err(storage("native field coordinates unavailable"));
        }
        let bytes = self
            .bytes
            .ok_or_else(|| storage("native state field header unavailable"))?;
        Ok((self.start, self.end, self.children, bytes))
    }
}
// Count original entries, including duplicate object keys. Value normalization
// remains last-wins, exactly as the previous inline-preview producer.
fn original_node_count(bytes: &[u8]) -> Result<u64, ConversationStoreError> {
    struct Count(u64);
    impl<'de> serde::Deserialize<'de> for Count {
        fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            struct Counter;
            impl<'de> serde::de::Visitor<'de> for Counter {
                type Value = Count;
                fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    formatter.write_str("original bounded JSON nodes")
                }
                fn visit_unit<E: serde::de::Error>(self) -> Result<Count, E> {
                    Ok(Count(1))
                }
                fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<Count, E> {
                    Ok(Count(1))
                }
                fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<Count, E> {
                    Ok(Count(1))
                }
                fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<Count, E> {
                    Ok(Count(1))
                }
                fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<Count, E> {
                    Ok(Count(1))
                }
                fn visit_str<E: serde::de::Error>(self, _: &str) -> Result<Count, E> {
                    Ok(Count(1))
                }
                fn visit_seq<A: serde::de::SeqAccess<'de>>(
                    self,
                    mut seq: A,
                ) -> Result<Count, A::Error> {
                    let mut nodes = 1_u64;
                    while let Some(Count(count)) = seq.next_element::<Count>()? {
                        nodes = (nodes + count).min(33);
                    }
                    Ok(Count(nodes))
                }
                fn visit_map<A: serde::de::MapAccess<'de>>(
                    self,
                    mut map: A,
                ) -> Result<Count, A::Error> {
                    let mut nodes = 1_u64;
                    while map.next_key::<serde::de::IgnoredAny>()?.is_some() {
                        let Count(count) = map.next_value::<Count>()?;
                        nodes = (nodes + count).min(33);
                    }
                    Ok(Count(nodes))
                }
            }
            deserializer.deserialize_any(Counter)
        }
    }
    serde_json::from_slice::<Count>(bytes)
        .map(|Count(count)| count)
        .map_err(|_| storage("native original field node evidence unavailable"))
}
fn header_columns(table: &str) -> Result<&'static str, ConversationStoreError> {
    if table != "state_observation_fields" {
        return Err(storage("unknown native state header table"));
    }
    Ok(
        "start,end,children,CASE WHEN typeof(header_json)='blob' AND length(header_json)<=16384 THEN header_json END",
    )
}
type FieldWithKey = (u64, u64, u64, Vec<u8>, Option<Vec<u8>>);
fn field_with_key(
    connection: &Connection,
    table: &'static str,
    id: &str,
    path: &[String],
) -> Result<FieldWithKey, ConversationStoreError> {
    field_path(path)?;
    if table == "request_observation_fields" {
        return composed::field(connection, id, path);
    }
    let (field,key)=connection.prepare_cached(&format!("SELECT {},CASE WHEN typeof(key_header)='blob' AND length(key_header)<=4096 THEN key_header END FROM {table} WHERE source_id=?1 AND path_json=?2",header_columns(table)?)).map_err(|error|sqlite_error(&error, error.to_string()))?
        .query_row(params![id,field_path(path)?],|row|Ok((StoredField::row(row,0)?,row.get::<_,Option<Vec<u8>>>(4)?))).optional().map_err(|error|sqlite_error(&error, error.to_string()))?
        .ok_or_else(||ConversationStoreError::InvalidReference("request field is unavailable".into()))?;
    let (start, end, children, bytes) = field.materialize()?;
    Ok((start, end, children, bytes, key))
}
pub(super) fn field_from(
    connection: &Connection,
    table: &'static str,
    id: &str,
    path: &[String],
) -> Result<(u64, u64, u64, Vec<u8>), ConversationStoreError> {
    let (start, end, children, bytes, _) = field_with_key(connection, table, id, path)?;
    Ok((start, end, children, bytes))
}
pub(super) fn validate_start(
    connection: &Connection,
    sequence: u64,
    through: u64,
) -> Result<(), ConversationStoreError> {
    let agent: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM request_owners o WHERE o.start_sequence=e.sequence AND o.owner_kind='agent') FROM events e WHERE e.sequence=?1 AND e.sequence<=?2",
            params![sequence, through],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| sqlite_error(&error, error.to_string()))?
        .unwrap_or(false);
    if agent {
        Ok(())
    } else {
        Err(ConversationStoreError::InvalidReference(
            "source is not an actual primary Agent Request start".into(),
        ))
    }
}
pub(super) fn header_at(
    connection: &Connection,
    sequence: u64,
    through: u64,
) -> Result<RequestObservationHeader, ConversationStoreError> {
    validate_start(connection, sequence, through)?;
    let key = sequence.to_string();
    let (start, end, children_total, bytes) =
        field_from(connection, "request_observation_fields", &key, &[])?;
    let summary = super::observation_trace::summary(connection, sequence, through)?;
    let count = field_coordinates(
        connection,
        "request_observation_fields",
        &key,
        &["messages".into()],
    )?
    .2;
    Ok(RequestObservationHeader {
        request_id: summary.and_then(|summary| summary.request_id),
        started_sequence: sequence,
        value: serde_json::from_slice(&bytes)
            .map_err(|_| storage("native request field header corrupt"))?,
        content: Vec::new(),
        collections: vec![crate::observation::cut::CollectionFrontier {
            revision: sequence,
            domain: "messages".into(),
            total: count,
            loaded: 0,
            next_offset: (count > 0).then_some(0),
        }],
        encoded_bytes: end - start,
        children_total,
        deferred: end - start > bytes.len() as u64,
    })
}
pub(super) fn header(
    connection: &Connection,
    request: &RequestId,
    through: u64,
) -> Result<RequestObservationHeader, ConversationStoreError> {
    header_at(connection, started(connection, request, through)?, through)
}
pub(super) fn page_at(
    connection: &Connection,
    sequence: u64,
    through: u64,
    path: &[String],
    offset: u64,
    limit: usize,
) -> Result<RequestObservationPage, ConversationStoreError> {
    validate_start(connection, sequence, through)?;
    page_from(
        connection,
        "request_observation_fields",
        &sequence.to_string(),
        path,
        offset,
        limit,
    )
}
pub(super) fn page(
    connection: &Connection,
    request: &RequestId,
    through: u64,
    path: &[String],
    offset: u64,
    limit: usize,
) -> Result<RequestObservationPage, ConversationStoreError> {
    page_at(
        connection,
        started(connection, request, through)?,
        through,
        path,
        offset,
        limit,
    )
}
// The CASE branches must reject malformed cells before rusqlite allocates them.
// NULL is legal only for an absent key; integer 0 deliberately fails BLOB decode.
fn page_metadata_columns() -> &'static str {
    "CASE WHEN typeof(path_json)='text' AND length(CAST(path_json AS BLOB))<=4096 THEN path_json END,CASE WHEN key_header IS NULL OR (typeof(key_header)='blob' AND length(key_header)<=4096) THEN key_header ELSE 0 END"
}
fn page_metadata(
    row: &rusqlite::Row<'_>,
    path_index: usize,
) -> rusqlite::Result<(String, Option<Vec<u8>>)> {
    let path: String = row.get(path_index)?;
    let key: Option<Vec<u8>> = row.get(path_index + 1)?;
    Ok((path, key))
}
pub(super) fn page_from(
    connection: &Connection,
    table: &'static str,
    id: &str,
    path: &[String],
    offset: u64,
    limit: usize,
) -> Result<RequestObservationPage, ConversationStoreError> {
    if limit == 0 || limit > HEADER_PAGE_MAX {
        return Err(ConversationStoreError::InvalidContentRange);
    }
    if table == "request_observation_fields" {
        field_path(path)?;
        return composed::page(connection, id, path, offset, limit);
    }
    let (_, _, total) = field_coordinates(connection, table, id, path)?;
    if offset > total {
        return Err(ConversationStoreError::InvalidContentRange);
    }
    let rows=connection.prepare_cached(&format!("SELECT {},{} FROM {table} WHERE source_id=?1 AND parent_path=?2 AND ordinal>=?3 ORDER BY ordinal LIMIT ?4",page_metadata_columns(),header_columns(table)?))
        .map_err(|error|sqlite_error(&error, error.to_string()))?.query_map(params![id,field_path(path)?,offset,limit],
            |row| {let (path,key)=page_metadata(row,0)?;Ok((path,StoredField::row(row,2)?,key))})
        .map_err(|error|sqlite_error(&error, error.to_string()))?.collect::<Result<Vec<_>,_>>().map_err(|error|sqlite_error(&error, error.to_string()))?;
    let mut fields = rows
        .into_iter()
        .map(|(path, field, key)| {
            let (start, end, children_total, bytes) = field.materialize()?;
            let path: Vec<String> =
                serde_json::from_str(&path).map_err(|error| storage(error.to_string()))?;
            let content = metadata_list(table, &path, &bytes, "content")?;
            let collections = metadata_list(table, &path, &bytes, "collections")?;
            let value: serde_json::Value =
                serde_json::from_slice(&bytes).map_err(|error| storage(error.to_string()))?;
            let deferred = end - start > bytes.len() as u64
                || message_descriptor_deferred(table, &path, &value);
            Ok(RequestObservationField {
                path,
                value,
                key: key
                    .map(|key| {
                        serde_json::from_slice(&key).map_err(|error| storage(error.to_string()))
                    })
                    .transpose()?,
                content,
                collections,
                encoded_bytes: end - start,
                children_total,
                deferred,
            })
        })
        .collect::<Result<Vec<_>, ConversationStoreError>>()?;
    while crate::observation::bounded::bounded_bytes(&fields, 524_288).is_none() {
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
pub(super) fn read_at(
    connection: &Connection,
    sequence: u64,
    through: u64,
    path: &[String],
    offset: u64,
    max_bytes: u32,
) -> Result<ObservationValueChunk, ConversationStoreError> {
    validate_start(connection, sequence, through)?;
    read_from(
        connection,
        "trace_request",
        "request_observation_fields",
        &sequence.to_string(),
        path,
        offset,
        max_bytes,
    )
}
pub(super) fn read(
    connection: &Connection,
    request: &RequestId,
    through: u64,
    path: &[String],
    offset: u64,
    max_bytes: u32,
) -> Result<ObservationValueChunk, ConversationStoreError> {
    read_at(
        connection,
        started(connection, request, through)?,
        through,
        path,
        offset,
        max_bytes,
    )
}
pub(super) fn read_from(
    connection: &Connection,
    kind: &'static str,
    table: &'static str,
    id: &str,
    path: &[String],
    offset: u64,
    max_bytes: u32,
) -> Result<ObservationValueChunk, ConversationStoreError> {
    if table == "request_observation_fields" {
        field_path(path)?;
        return composed::range(connection, id, path, offset, max_bytes);
    }
    let (start, end, _) = field_coordinates(connection, table, id, path)?;
    let readable_end = end - start;
    if max_bytes == 0 || max_bytes > 65_536 || offset > readable_end {
        return Err(ConversationStoreError::InvalidContentRange);
    }
    let relative_end = offset
        .checked_add(u64::from(max_bytes))
        .filter(|value| *value <= crate::observation::content::CONTENT_SAFE_INTEGER_MAX)
        .ok_or(ConversationStoreError::InvalidContentRange)?
        .min(readable_end);
    let bytes = super::content::range_bytes(
        connection,
        (
            kind,
            id,
            0,
            crate::observation::content::ContentRepresentation::CanonicalJsonV1,
        ),
        start
            .checked_add(offset)
            .ok_or(ConversationStoreError::InvalidContentRange)?,
        start
            .checked_add(relative_end)
            .ok_or(ConversationStoreError::InvalidContentRange)?,
    )?;
    Ok(ObservationValueChunk {
        bytes,
        end_offset: relative_end,
        readable_end,
        next_offset: (relative_end < readable_end).then_some(relative_end),
    })
}

pub(super) fn field_header_at(
    connection: &Connection,
    sequence: u64,
    through: u64,
    path: &[String],
) -> Result<RequestObservationField, ConversationStoreError> {
    validate_start(connection, sequence, through)?;
    let key = sequence.to_string();
    let (start, end, children_total, bytes, name) =
        field_with_key(connection, "request_observation_fields", &key, path)?;
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|_| storage("native request field corrupt"))?;
    let deferred = end - start > bytes.len() as u64
        || message_descriptor_deferred("request_observation_fields", path, &value);
    Ok(RequestObservationField {
        path: path.to_vec(),
        key: name
            .map(|name| {
                serde_json::from_slice(&name)
                    .map_err(|_| storage("native request field name corrupt"))
            })
            .transpose()?,
        value,
        content: metadata_list("request_observation_fields", path, &bytes, "content")?,
        collections: metadata_list("request_observation_fields", path, &bytes, "collections")?,
        encoded_bytes: end - start,
        children_total,
        deferred,
    })
}
pub(super) fn field_header(
    connection: &Connection,
    request: &RequestId,
    through: u64,
    path: &[String],
) -> Result<RequestObservationField, ConversationStoreError> {
    field_header_at(
        connection,
        started(connection, request, through)?,
        through,
        path,
    )
}
fn typed_value<T: serde::Serialize + schemars::JsonSchema>(
    value: &T,
) -> Result<serde_json::Value, ConversationStoreError> {
    let root = crate::observation::structural_json::root::<T>();
    serde_json::to_value(crate::observation::structural_json::Encoded {
        value,
        schema: &root,
        root: &root,
    })
    .map_err(|error| storage(error.to_string()))
}

pub(super) fn terminal(
    connection: &Connection,
    event: &crate::events::types::RuntimeEventEnvelope,
) -> Result<(), ConversationStoreError> {
    use crate::events::types::RuntimeEvent;
    let (RuntimeEvent::ModelRequestCompleted {
        request_id: request,
        ..
    }
    | RuntimeEvent::ModelRequestFailed {
        request_id: request,
        ..
    }) = &event.event
    else {
        return Ok(());
    };
    // A Journal may truthfully retain a request terminal whose immutable
    // input source is unavailable. Display indexing must not reject that
    // execution fact or invent a frozen source to make it readable.
    let retained: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM request_snapshots WHERE request_id=?1)",
            [request.as_str()],
            |row| row.get(0),
        )
        .map_err(|error| sqlite_error(&error, error.to_string()))?;
    if !retained {
        return Ok(());
    }
    let source_sequence = started(connection, request, event.sequence)?;
    let source_key = source_sequence.to_string();
    let source = crate::observation::trace::frozen_request_outcome(&event.event)
        .expect("matched native request terminal");
    let encode = |field, value: &_| {
        super::content::index_structural_value(
            connection,
            "trace_terminal",
            &source_key,
            field,
            value,
        )
    };
    let encoded_bytes = encode(0, &source)?;
    super::content::index_structural_value(
        connection,
        "trace_terminal",
        &source_key,
        1,
        &source.failure,
    )?;
    super::content::index_structural_value(
        connection,
        "trace_terminal",
        &source_key,
        2,
        &source
            .failure
            .as_ref()
            .map(crate::observation::trace::frozen::BorrowedFailure::message),
    )?;
    super::content::index_structural_value(
        connection,
        "trace_terminal",
        &source_key,
        3,
        &source.usage,
    )?;
    super::content::index_structural_value(
        connection,
        "trace_terminal",
        &source_key,
        4,
        &source.generation,
    )?;
    let failure_deferred = source
        .failure
        .as_ref()
        .is_some_and(|value| value.message().chars().nth(256).is_some());
    let failure = source
        .failure
        .as_ref()
        .map(crate::observation::trace::frozen::BorrowedFailure::header);
    let header = crate::durable::observation::RequestObservationOutcome {
        terminal_sequence: event.sequence,
        event_id: (event.event_id.as_str().len() <= 512).then(|| event.event_id.clone()),
        usage: source.usage.clone(),
        failure,
        generation: source.generation,
        failure_deferred,
        encoded_bytes,
    };
    let bytes = crate::observation::bounded::bounded_bytes(&header, 16_384)
        .ok_or_else(|| storage("native request outcome header exceeds bound"))?;
    let changed=connection.execute("UPDATE request_snapshots SET observation_terminal_sequence=?2,observation_terminal_header=?3 WHERE request_id=?1 AND observation_terminal_sequence IS NULL",params![request.as_str(),event.sequence,bytes]).map_err(|error|sqlite_error(&error, error.to_string()))?;
    if changed != 1 {
        return Err(storage(
            "native request terminal header owner missing or duplicated",
        ));
    }
    Ok(())
}
pub(super) fn outcome_at(
    connection: &Connection,
    source_sequence: u64,
    through: u64,
) -> Result<Option<crate::durable::observation::RequestObservationOutcome>, ConversationStoreError>
{
    validate_start(connection, source_sequence, through)?;
    let row:Option<(Option<u64>,Option<Vec<u8>>)>=connection.query_row("SELECT completed_sequence,observation_terminal_header FROM request_snapshots INDEXED BY request_snapshots_started_idx WHERE started_sequence=?1",[source_sequence],|row|Ok((row.get(0)?,row.get(1)?))).optional().map_err(|error|sqlite_error(&error, error.to_string()))?;
    let (terminal, bytes) =
        row.ok_or_else(|| storage("original Request snapshot source absent"))?;
    if terminal.is_none_or(|sequence| sequence > through) {
        return Ok(None);
    }
    let bytes = bytes.ok_or_else(|| storage("native Request outcome header unavailable"))?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| storage("native Request outcome header corrupt"))
}
pub(super) fn outcome_read_at(
    connection: &Connection,
    source_sequence: u64,
    through: u64,
    path: &[String],
    offset: u64,
    max_bytes: u32,
) -> Result<ObservationValueChunk, ConversationStoreError> {
    if outcome_at(connection, source_sequence, through)?.is_none() {
        return Err(ConversationStoreError::InvalidReference(
            "request terminal source is outside native cut".into(),
        ));
    }
    let field = match path
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        [] => 0,
        ["failure"] => 1,
        ["failure", "message", "text"] => 2,
        ["usage"] => 3,
        ["generation"] => 4,
        _ => {
            return Err(ConversationStoreError::InvalidReference(
                "request terminal field is not allowlisted".into(),
            ));
        }
    };
    super::content::structural_field_read(
        connection,
        "trace_terminal",
        &source_sequence.to_string(),
        field,
        offset,
        max_bytes,
    )
}

pub(super) fn outcome(
    connection: &Connection,
    request: &RequestId,
    through: u64,
) -> Result<Option<crate::durable::observation::RequestObservationOutcome>, ConversationStoreError>
{
    outcome_at(connection, started(connection, request, through)?, through)
}
pub(super) fn outcome_read(
    connection: &Connection,
    request: &RequestId,
    through: u64,
    path: &[String],
    offset: u64,
    max_bytes: u32,
) -> Result<ObservationValueChunk, ConversationStoreError> {
    outcome_read_at(
        connection,
        started(connection, request, through)?,
        through,
        path,
        offset,
        max_bytes,
    )
}
// Only a native request-message producer owns this descriptor flag. An
// identically named property inside arbitrary original JSON stays opaque.
fn message_descriptor_deferred(table: &str, path: &[String], value: &serde_json::Value) -> bool {
    table == "request_observation_fields"
        && path.len() == 2
        && path[0] == "messages"
        && path[1].parse::<u64>().is_ok()
        && value.get("deferred").and_then(serde_json::Value::as_bool) == Some(true)
}

fn metadata_list<T: serde::de::DeserializeOwned + schemars::JsonSchema>(
    table: &str,
    path: &[String],
    bytes: &[u8],
    name: &str,
) -> Result<Vec<T>, ConversationStoreError> {
    // Only message_value appends these typed fields. Original Journal, state
    // and arbitrary request JSON retain their own property-name semantics.
    if table != "request_observation_fields"
        || path.len() != 2
        || path[0] != "messages"
        || path[1].parse::<u64>().is_err()
    {
        return Ok(Vec::new());
    }
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|error| storage(error.to_string()))?;
    value
        .get(name)
        .cloned()
        .map(|value| {
            crate::observation::bounded::decode_domain_value(value)
                .map_err(|error| storage(error.to_string()))
        })
        .transpose()
        .map(Option::unwrap_or_default)
}
fn bounded_message_descriptor(
    message: &crate::observation::trace::TraceRequestMessage,
    mut loaded: usize,
    deferred: bool,
) -> Result<serde_json::Value, ConversationStoreError> {
    // A full native input view may carry detail-sized blocks. Admit its
    // canonical descriptor before materializing JSON; original block bytes
    // remain available through the exact canonical bindings below.
    #[derive(serde::Serialize)]
    struct Descriptor<'a> {
        request_occurrence: &'a Option<crate::context::checkpoint::RequestMessageOccurrence>,
        provenance: &'a crate::observation::trace::TraceInputProvenance,
        provenance_source: &'a Option<crate::observation::trace::TraceFieldSource>,
        role: &'a crate::observation::trace::TraceMessageRole,
        message_id: &'a Option<crate::runtime::identity::MessageId>,
        source: &'a Option<String>,
        blocks: &'a [crate::observation::trace::TraceContentBlock],
        truncated: bool,
    }
    impl schemars::JsonSchema for Descriptor<'_> {
        fn schema_name() -> std::borrow::Cow<'static, str> {
            <crate::observation::trace::TraceRequestMessage as schemars::JsonSchema>::schema_name()
        }
        fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
            <crate::observation::trace::TraceRequestMessage as schemars::JsonSchema>::json_schema(
                generator,
            )
        }
    }
    let value = loop {
        let descriptor = Descriptor {
            request_occurrence: &message.request_occurrence,
            provenance_source: &message.provenance_source,
            provenance: &message.provenance,
            role: &message.role,
            message_id: &message.message_id,
            source: &message.source,
            blocks: &message.blocks[..loaded],
            truncated: message.truncated || loaded < message.blocks.len() || deferred,
        };
        if let Some(value) = crate::observation::bounded::structural_value(&descriptor) {
            break value;
        }
        loaded = loaded
            .checked_sub(1)
            .ok_or_else(|| storage("native Trace identity exceeds finite descriptor bound"))?;
    };
    Ok(value)
}
fn message_value_capture(
    header: &crate::durable::observation::MessageHeader,
    message: &crate::observation::trace::TraceRequestMessage,
) -> Result<(serde_json::Value, bool), ConversationStoreError> {
    let initial = if header.display.is_some() {
        message.blocks.len()
    } else {
        0
    };
    let mut value = bounded_message_descriptor(message, initial, header.deferred)?;
    let loaded = value["blocks"].as_array().map_or(0, Vec::len);
    let content = message_bindings(header)?;
    let position = header
        .ledger_position
        .ok_or_else(|| storage("native Trace canonical ledger locator absent"))?;
    let domain = format!("messages.ledger:{position}.blocks");
    let pretrimmed = loaded < message.blocks.len();
    let mut untrimmed = loaded == initial;
    let loaded = if header.role == "tool" {
        value["blocks"]
            .as_array()
            .and_then(|blocks| blocks.first())
            .and_then(|block| block["blocks"].as_array())
            .map_or(0, Vec::len)
    } else {
        value["blocks"].as_array().map_or(0, Vec::len)
    } as u64;
    let collections = vec![crate::observation::cut::CollectionFrontier {
        revision: 1,
        domain,
        total: header.block_count,
        loaded,
        next_offset: (loaded < header.block_count).then_some(loaded),
    }];
    value["content"] = typed_value(&content)?;
    value["collections"] = typed_value(&collections)?;
    value["deferred"] = serde_json::Value::Bool(header.deferred || pretrimmed);
    while crate::observation::bounded::bounded_bytes(&value, 16_384).is_none() {
        untrimmed = false;
        value["truncated"] = serde_json::Value::Bool(true);
        value["deferred"] = serde_json::Value::Bool(true);
        value["collections"][0]["loaded"] = serde_json::json!(0);
        value["collections"][0]["next_offset"] = if header.block_count > 0 {
            serde_json::json!(0)
        } else {
            serde_json::Value::Null
        };
        if value["content"]
            .as_array_mut()
            .is_some_and(|content| content.pop().is_some())
        {
            continue;
        }
        if value["blocks"]
            .as_array_mut()
            .is_some_and(|blocks| blocks.pop().is_some())
        {
            continue;
        }
        return Err(storage(
            "native Trace identity exceeds finite descriptor bound",
        ));
    }
    Ok((value, untrimmed))
}
fn message_bindings(
    header: &crate::durable::observation::MessageHeader,
) -> Result<Vec<crate::observation::cut::ContentBinding>, ConversationStoreError> {
    use crate::observation::content::{CanonicalContentComponent, ContentSource};
    let mut content = Vec::new();
    for binding in &header.content {
        if matches!(
            binding.content_ref.source,
            ContentSource::CanonicalMessageBlock {
                component: CanonicalContentComponent::MessageMetadata,
                ..
            }
        ) {
            if header.identity.is_none() {
                content.push(crate::observation::cut::ContentBinding {
                    path: vec!["message_id".into()],
                    source_path: Some(vec!["id".into()]),
                    content_ref: binding.content_ref.clone(),
                    deferred: true,
                });
            }
            continue;
        }
        let Some(position) = binding.path.iter().position(|field| field == "content") else {
            continue;
        };
        let Some(index) = binding.path.get(position + 1) else {
            continue;
        };
        let mut path = vec!["blocks".into()];
        if header.role == "tool" {
            path.extend(["0".into(), "blocks".into()]);
        }
        path.push(index.clone());
        let component = match &binding.content_ref.source {
            ContentSource::CanonicalMessageBlock { component, .. } => *component,
            _ => continue,
        };
        let native_index = index
            .parse::<usize>()
            .map_err(|_| storage("canonical block locator invalid"))?;
        let native_kind = native_block_kind(header, native_index);
        let selections: Vec<(Vec<String>, Option<Vec<String>>)> =
            if component == CanonicalContentComponent::BlockMetadata {
                if native_kind == Some("tool_call") {
                    vec![(vec!["name".into()], Some(vec!["name".into()]))]
                } else {
                    Vec::new()
                }
            } else {
                match binding.path.last().map(String::as_str) {
                    Some("text") => vec![(vec!["text".into(), "text".into()], None)],
                    Some("arguments") => vec![(vec!["arguments".into(), "value".into()], None)],
                    Some("value") => vec![(vec!["value".into(), "value".into()], None)],
                    _ => match native_kind {
                        Some("file") => vec![
                            (
                                vec!["artifact".into(), "name".into()],
                                Some(vec!["name".into()]),
                            ),
                            (
                                vec!["artifact".into(), "mime_type".into()],
                                Some(vec!["mime_type".into()]),
                            ),
                        ],
                        Some("image") => vec![(vec!["alt".into()], Some(vec!["alt".into()]))],
                        Some("upload") => vec![(vec!["name".into()], Some(vec!["name".into()]))],
                        _ => Vec::new(),
                    },
                }
            };
        for (suffix, source_path) in selections {
            let mut target = path.clone();
            target.extend(suffix);
            content.push(crate::observation::cut::ContentBinding {
                source_path,
                path: target,
                content_ref: binding.content_ref.clone(),
                deferred: true,
            });
        }
    }
    Ok(content)
}
fn native_block_kind(
    header: &crate::durable::observation::MessageHeader,
    native_index: usize,
) -> Option<&'static str> {
    header.display.as_ref().and_then(|message| match message {
        crate::message::types::MessageBlock::User(message) => {
            message.content.get(native_index).map(|block| match block {
                crate::message::types::UserContentBlock::Text(_) => "text",
                crate::message::types::UserContentBlock::Image(_) => "image",
                crate::message::types::UserContentBlock::File(_) => "file",
                crate::message::types::UserContentBlock::UploadedFile(_) => "upload",
            })
        }
        crate::message::types::MessageBlock::Assistant(message) => {
            message.content.get(native_index).map(|block| match block {
                crate::message::types::AssistantContentBlock::Text(_) => "text",
                crate::message::types::AssistantContentBlock::Refusal(_) => "refusal",
                crate::message::types::AssistantContentBlock::Reasoning(_) => "reasoning",
                crate::message::types::AssistantContentBlock::ToolCall(_) => "tool_call",
                crate::message::types::AssistantContentBlock::Image(_) => "image",
            })
        }
        crate::message::types::MessageBlock::Tool(message) => message
            .result
            .content
            .get(native_index)
            .map(|block| match block {
                crate::tools::types::ToolResultContent::Text(_) => "text",
                crate::tools::types::ToolResultContent::Json { .. } => "json",
                crate::tools::types::ToolResultContent::Image(_) => "image",
                crate::tools::types::ToolResultContent::File(_) => "file",
            }),
    })
}

fn index_key(
    writer: &SourceWriter<'_>,
    path: &[String],
    key: &str,
    start: u64,
    end: u64,
) -> Result<(), ConversationStoreError> {
    let mut key_path = path.to_vec();
    key_path.push("$key".into());
    let descriptor = crate::durable::observation::RequestObservationKey {
        path: key_path.clone(),
        preview: key.chars().take(64).collect(),
        encoded_bytes: end - start,
    };
    let header = serde_json::to_vec(&descriptor).map_err(|error| storage(error.to_string()))?;
    let key_value =
        serde_json::to_vec(&descriptor.preview).map_err(|error| storage(error.to_string()))?;
    let insert = format!(
        "INSERT INTO {}(source_id,path_json,parent_path,ordinal,start,end,children,header_json) VALUES(?1,?2,NULL,NULL,?3,?4,0,?5)",
        writer.table
    );
    writer
        .connection
        .execute(
            &insert,
            params![writer.id, encoded_path(&key_path)?, start, end, key_value],
        )
        .map_err(|error| sqlite_error(&error, error.to_string()))?;
    writer
        .connection
        .execute(
            &format!(
                "UPDATE {} SET key_path=?3,key_header=?4 WHERE source_id=?1 AND path_json=?2",
                writer.table
            ),
            params![
                writer.id,
                encoded_path(path)?,
                encoded_path(&key_path)?,
                header
            ],
        )
        .map_err(|error| sqlite_error(&error, error.to_string()))?;
    Ok(())
}
