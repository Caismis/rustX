//! Original RequestStart-owned allowlisted structural inspection layout.
//! One byte stream and path offsets preserve source identity without copying a
//! subtree for every locator; every read seeks at most two finite BLOB rows.
use super::{Connection, ConversationStoreError, OptionalExtension, params, storage};
use crate::durable::observation::{
    HEADER_PAGE_MAX, ObservationValueChunk, RequestObservationField, RequestObservationHeader,
    RequestObservationPage,
};
use crate::runtime::identity::RequestId;
use std::io::Write;
pub(in crate::durable::sqlite) mod fragments;
mod input;
pub(super) mod provenance;
pub(in crate::durable::sqlite) mod shape;
const SEGMENT: usize = 65_536;
#[cfg(test)]
thread_local! {
    pub(super) static LAST_INDEX_PROFILE: std::cell::Cell<(std::time::Duration,std::time::Duration,std::time::Duration,u64)> = const {std::cell::Cell::new((std::time::Duration::ZERO,std::time::Duration::ZERO,std::time::Duration::ZERO,0))};
}
#[cfg(test)]
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct RequestIndexProfile {
    /// Completed index calls; only trace-request calls count model requests.
    pub(crate) requests: u64,
    pub(crate) layout_us: u128,
    pub(crate) bytes_us: u128,
    pub(crate) fields_us: u128,
    pub(crate) field_count: u64,
    pub(crate) max_field_count: u64,
}
#[cfg(test)]
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct SourceStatisticsProfile {
    pub(crate) calls: u64,
    /// Includes the existing optional-layout savepoint and event/member pair.
    pub(crate) total_us: u128,
}
#[cfg(test)]
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct SourceIndexProfile {
    pub(crate) request: RequestIndexProfile,
    pub(crate) journal: RequestIndexProfile,
    pub(crate) goal: RequestIndexProfile,
    pub(crate) todo: RequestIndexProfile,
    pub(crate) context: RequestIndexProfile,
    pub(crate) other: RequestIndexProfile,
    pub(crate) statistics: SourceStatisticsProfile,
}
#[cfg(test)]
static INDEX_PROFILE: std::sync::Mutex<Option<SourceIndexProfile>> = std::sync::Mutex::new(None);

/// One explicit ignored measurement; ordinary tests retain no aggregate.
#[cfg(test)]
pub(crate) struct RequestIndexProfileGuard {
    active: bool,
}
#[cfg(test)]
impl RequestIndexProfileGuard {
    pub(super) fn start() -> Self {
        let mut profile = INDEX_PROFILE.lock().unwrap();
        assert!(profile.is_none(), "one native request-index measurement");
        *profile = Some(SourceIndexProfile::default());
        Self { active: true }
    }
    pub(crate) fn snapshot(&self) -> RequestIndexProfile {
        self.sources().request
    }
    pub(crate) fn sources(&self) -> SourceIndexProfile {
        assert!(self.active, "active scoped Source profile");
        INDEX_PROFILE
            .lock()
            .unwrap()
            .expect("active request-index measurement")
    }
}
#[cfg(test)]
impl Drop for RequestIndexProfileGuard {
    fn drop(&mut self) {
        self.active = false;
        *INDEX_PROFILE.lock().unwrap() = None;
    }
}
#[cfg(test)]
fn profile_index(
    kind: &str,
    layout: std::time::Duration,
    bytes: std::time::Duration,
    fields: std::time::Duration,
    count: u64,
) {
    if let Some(source) = INDEX_PROFILE.lock().unwrap().as_mut() {
        let profile = match kind {
            "trace_request" => &mut source.request,
            "observation_journal" => &mut source.journal,
            "observation_goal" => &mut source.goal,
            "observation_todo" => &mut source.todo,
            "observation_context" => &mut source.context,
            _ => &mut source.other,
        };
        profile.requests += 1;
        profile.layout_us += layout.as_micros();
        profile.bytes_us += bytes.as_micros();
        profile.fields_us += fields.as_micros();
        profile.field_count += count;
        profile.max_field_count = profile.max_field_count.max(count);
    }
}
#[cfg(test)]
pub(super) fn profile_statistics(elapsed: std::time::Duration) {
    if let Some(profile) = INDEX_PROFILE.lock().unwrap().as_mut() {
        profile.statistics.calls += 1;
        profile.statistics.total_us += elapsed.as_micros();
    }
}
struct SourceWriter<'a> {
    connection: &'a Connection,
    id: &'a str,
    kind: &'static str,
    table: &'static str,
    buffer: Vec<u8>,
    written: u64,
    segment: u64,
}
impl SourceWriter<'_> {
    fn drain(&mut self) -> std::io::Result<()> {
        if !self.buffer.is_empty() {
            self.connection.execute("INSERT INTO content_segments(source_kind,source_id,block_index,representation,segment,bytes) VALUES(?1,?2,0,'canonical_json_v1',?3,?4)",params![self.kind,self.id,self.segment,self.buffer]).map_err(std::io::Error::other)?;
            self.written = self
                .written
                .checked_add(self.buffer.len() as u64)
                .filter(|value| *value <= crate::runtime_client::content::CONTENT_SAFE_INTEGER_MAX)
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
#[cfg(test)]
pub(super) fn index(
    connection: &Connection,
    source_sequence: u64,
    scope: &crate::runtime::identity::ConversationId,
    messages: &[crate::model::ModelInputMessage],
    snapshot: &crate::model::RequestSnapshot,
) -> Result<(), ConversationStoreError> {
    index_with_fragments(connection, source_sequence, scope, messages, snapshot, None)
}
pub(in crate::durable::sqlite) fn index_with_fragments(
    connection: &Connection,
    source_sequence: u64,
    scope: &crate::runtime::identity::ConversationId,
    messages: &[crate::model::ModelInputMessage],
    snapshot: &crate::model::RequestSnapshot,
    fragments: Option<&std::cell::RefCell<fragments::Session<'_>>>,
) -> Result<(), ConversationStoreError> {
    let id = source_sequence.to_string();
    // One finite item at a time: the source stream and display header share
    // producer shaping. Offsets are filled only after the immutable stream seals.
    let headers = std::cell::RefCell::new(connection.prepare_cached(
        "INSERT INTO request_observation_fields(source_id,path_json,parent_path,ordinal,start,end,children,header_kind,header_json) VALUES(?1,?2,'[\"messages\"]',?3,0,0,0,'inline',?4)"
    ).map_err(|error| storage(error.to_string()))?);
    let capture_header = |ordinal: u64, bytes: &[u8], key: Option<&fragments::Key>| {
        headers
            .borrow_mut()
            .execute(params![
                id,
                encoded_path(&["messages".into(), ordinal.to_string()])?,
                ordinal,
                bytes,
            ])
            .map_err(|error| storage(error.to_string()))?;
        shape::capture_item(connection, &id, ordinal, bytes, key)?;
        Ok(())
    };
    let value = crate::runtime_client::trace::frozen_request_detail(
        snapshot,
        FrozenMessages {
            connection,
            messages,
            scope,
            snapshot,
            sequence: source_sequence,
            capture_header: &capture_header,
            fragments,
        },
    );
    index_stream_with_fragments(
        connection,
        "trace_request",
        &id,
        "request_observation_fields",
        &value,
        fragments,
        true,
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
    index_stream_with_fragments(connection, kind, id, table, value, None, false)
}
fn index_stream_with_fragments<T: serde::Serialize + schemars::JsonSchema>(
    connection: &Connection,
    kind: &'static str,
    id: &str,
    table: &'static str,
    value: &T,
    fragments: Option<&std::cell::RefCell<fragments::Session<'_>>>,
    native_messages: bool,
) -> Result<(), ConversationStoreError> {
    #[cfg(test)]
    let began = std::time::Instant::now();
    let capture = fragments.map(|session| session.borrow().capture());
    let source = if let Some(capture) = &capture {
        crate::runtime::read_layout::SeekableObservationSource::new_with_fragments(value, capture)
    } else {
        crate::runtime::read_layout::SeekableObservationSource::new(value)
    }
    .map_err(storage)?;
    #[cfg(test)]
    let layout_elapsed = began.elapsed();
    #[cfg(test)]
    let bytes_began = std::time::Instant::now();
    let mut writer = SourceWriter {
        connection,
        id,
        kind,
        table,
        buffer: Vec::with_capacity(SEGMENT),
        written: 0,
        segment: 0,
    };
    source
        .copy_original_bytes_to(&mut writer)
        .map_err(|error| storage(error.to_string()))?;
    writer.flush().map_err(|error| storage(error.to_string()))?;
    #[cfg(test)]
    let bytes_elapsed = bytes_began.elapsed();
    #[cfg(test)]
    let fields_began = std::time::Instant::now();
    // Only the typed native request producer certifies canonical message shapes.
    // Generic state/cfg field-policy fixtures still use the same field visitor.
    let reader = native_messages
        .then(|| source.message_shape_reader())
        .transpose()
        .map_err(storage)?;
    let shapes = if let Some(reader) = &reader {
        shape::prepare(connection, id, &source, reader)?
    } else {
        Vec::new()
    };
    let field_count = index_fields(&writer, &source, &shapes, reader.as_ref())?;
    #[cfg(not(test))]
    let _ = field_count;
    #[cfg(test)]
    {
        let fields_elapsed = fields_began.elapsed();
        if kind == "trace_request" {
            LAST_INDEX_PROFILE.with(|profile| {
                profile.set((layout_elapsed, bytes_elapsed, fields_elapsed, field_count));
            });
        }
        profile_index(
            kind,
            layout_elapsed,
            bytes_elapsed,
            fields_elapsed,
            field_count,
        );
    }
    connection
        .execute(
            "INSERT INTO content_frontiers VALUES(?1,?2,0,'canonical_json_v1',1,?3)",
            params![kind, id, writer.written],
        )
        .map_err(|error| storage(error.to_string()))?;
    if let Some(fragments) = fragments {
        fragments.borrow_mut().finish(&source);
    }
    Ok(())
}
fn kind_is_request(writer: &SourceWriter<'_>) -> bool {
    writer.kind == "trace_request" && writer.table == "request_observation_fields"
}
fn captured_message_header(writer: &SourceWriter<'_>, path: &[String]) -> bool {
    kind_is_request(writer)
        && path.len() == 2
        && path[0] == "messages"
        && path[1].parse::<u64>().is_ok()
}
type EncodedFieldHeader = (&'static str, Option<u64>, Option<Vec<u8>>);
fn stored_preview(
    source: &crate::runtime::read_layout::SeekableObservationSource,
    path: &[String],
    start: u64,
    end: u64,
    request: bool,
    preview: crate::runtime::read_layout::FieldPreview<'_>,
) -> Result<EncodedFieldHeader, ConversationStoreError> {
    use crate::runtime::read_layout::FieldPreview;
    Ok(match preview {
        FieldPreview::Inline(preview) => {
            let original_message =
                if !request && path.len() == 2 && path[0] == "messages" && end - start <= 16_384 {
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
            let value = original_message.as_ref().unwrap_or(preview);
            (
                "inline",
                None,
                Some(
                    crate::runtime_client::checkpoint::bounded_bytes(value, 16_384)
                        .unwrap_or_else(|| b"null".to_vec()),
                ),
            )
        }
        FieldPreview::OriginalSpan { recursive_nodes } if request => {
            ("original_span", Some(recursive_nodes), None)
        }
        _ => return Err(storage("uncaptured field preview unavailable")),
    })
}
type CapturedField<'a> = (&'a [String], &'a str, Option<(&'a str, u64)>, u64, u64, u64);
fn finish_captured_header(
    writer: &SourceWriter<'_>,
    captured: &mut rusqlite::Statement<'_>,
    shapes: &[crate::runtime::read_layout::CertifiedMessageShape<'_>],
    field: CapturedField<'_>,
) -> Result<(), ConversationStoreError> {
    let (path, path_json, parent, start, end, children) = field;
    let item = path[1]
        .parse()
        .map_err(|_| storage("invalid request item"))?;
    if let Ok(index) = shapes.binary_search_by_key(&item, |shape| shape.item) {
        let root = shapes[index].root;
        if (start, end, children) != (root.start, root.end, root.children)
            || parent != Some(("[\"messages\"]", item))
        {
            return Err(storage("finalized request root differs from native source"));
        }
        // The same-source certificate already finalized this guarded placeholder
        // with its witness. Keep the visitor check without repeating two writes.
        return Ok(());
    }
    let rows = captured
        .execute(params![
            writer.id,
            path_json,
            parent.map(|(path, _)| path),
            parent.map(|(_, ordinal)| ordinal),
            start,
            end,
            children,
        ])
        .map_err(|error| storage(error.to_string()))?;
    if rows != 1 {
        return Err(storage(
            "native request item header placeholder unavailable",
        ));
    }
    shape::finish_item(writer.connection, writer.id, item, start, end, children)?;
    Ok(())
}
fn index_fields(
    writer: &SourceWriter<'_>,
    source: &crate::runtime::read_layout::SeekableObservationSource,
    shapes: &[crate::runtime::read_layout::CertifiedMessageShape<'_>],
    reader: Option<&crate::runtime::read_layout::MessageShapeReader<'_>>,
) -> Result<u64, ConversationStoreError> {
    let (connection, id, table) = (writer.connection, writer.id, writer.table);
    let mut field_count = 0;
    let request = kind_is_request(writer);
    let insert = if request {
        "INSERT INTO request_observation_fields(source_id,path_json,parent_path,ordinal,start,end,children,header_kind,header_nodes,header_json) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)".to_owned()
    } else {
        format!(
            "INSERT INTO {table}(source_id,path_json,parent_path,ordinal,start,end,children,header_json) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)"
        )
    };
    let mut fields = connection
        .prepare_cached(&insert)
        .map_err(|error| storage(error.to_string()))?;
    let mut captured = connection.prepare_cached(
        "UPDATE request_observation_fields SET parent_path=?3,ordinal=?4,start=?5,end=?6,children=?7 WHERE source_id=?1 AND path_json=?2 AND parent_path='[\"messages\"]' AND ordinal=?4 AND start=0 AND end=0 AND header_kind='inline' AND header_nodes IS NULL"
    ).map_err(|error| storage(error.to_string()))?;
    let mut failure = None;
    let policy = |path: &[String]| {
        use crate::runtime::read_layout::FieldPreviewPolicy as P;
        if captured_message_header(writer, path) {
            P::Skip
        } else if request {
            P::OriginalSpanIfSmall
        } else {
            P::Inline
        }
    };
    let visit = |path: &[String],
                 path_json: &str,
                 parent: Option<(&str, u64)>,
                 start: u64,
                 end: u64,
                 children: u64,
                 key_span: Option<(u64, u64)>,
                 preview: crate::runtime::read_layout::FieldPreview<'_>| {
        field_count += 1;
        let result = (|| -> Result<(), ConversationStoreError> {
            if captured_message_header(writer, path) {
                finish_captured_header(
                    writer,
                    &mut captured,
                    shapes,
                    (path, path_json, parent, start, end, children),
                )?;
                return Ok(());
            }
            let (header_kind, nodes, header) =
                stored_preview(source, path, start, end, request, preview)?;
            if request {
                fields.execute(params![
                    id,
                    path_json,
                    parent.map(|(path, _)| path),
                    parent.map(|(_, ordinal)| ordinal),
                    start,
                    end,
                    children,
                    header_kind,
                    nodes,
                    header
                ])
            } else {
                fields.execute(params![
                    id,
                    path_json,
                    parent.map(|(path, _)| path),
                    parent.map(|(_, ordinal)| ordinal),
                    start,
                    end,
                    children,
                    header
                ])
            }
            .map_err(|error| storage(error.to_string()))?;
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
    };
    let result = if let Some(reader) = reader {
        reader.visit_fields_with_packed_message_shapes(shapes, policy, visit)
    } else {
        source.visit_fields_selecting_previews(policy, visit)
    };
    result.map_err(|error| failure.unwrap_or_else(|| storage(error)))?;
    Ok(field_count)
}
fn provenance_source(
    sequence: u64,
    item: u64,
    encoded_bytes: u64,
) -> crate::runtime_client::trace::TraceFieldSource {
    crate::runtime_client::trace::TraceFieldSource::Value {
        field: crate::runtime_client::trace::TraceFieldKind::Provenance,
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
        crate::runtime_client::trace::TraceRequestMessage,
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
    let mut view = crate::runtime_client::trace::frozen::input_message(scope, message, original);
    let header = message
        .canonical_id()
        .map(|id| {
            super::observation::message(connection, id)?
                .ok_or_else(|| storage("frozen canonical header unavailable"))
        })
        .transpose()?;
    if let Some(header) = &header {
        crate::runtime_client::trace::request::bind_original_provenance(&mut view, header)?;
    }
    Ok((view, header))
}
#[cfg(test)]
pub(super) fn index_test_request_fields<T: serde::Serialize + schemars::JsonSchema>(
    connection: &Connection,
    sequence: u64,
    value: &T,
) -> Result<(), ConversationStoreError> {
    index_stream(
        connection,
        "trace_request",
        &sequence.to_string(),
        "request_observation_fields",
        value,
    )
}
#[cfg(test)]
pub(super) fn legacy_request_source_bytes(
    connection: &Connection,
    sequence: u64,
    scope: &crate::runtime::identity::ConversationId,
    messages: &[crate::model::ModelInputMessage],
    snapshot: &crate::model::RequestSnapshot,
) -> Result<Vec<u8>, ConversationStoreError> {
    let capture = |_ordinal: u64, _bytes: &[u8], _key: Option<&fragments::Key>| Ok(());
    let value = crate::runtime_client::trace::frozen_request_detail(
        snapshot,
        FrozenMessages {
            connection,
            scope,
            messages,
            snapshot,
            sequence,
            capture_header: &capture,
            fragments: None,
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
    messages: &[crate::model::ModelInputMessage],
    snapshot: &crate::model::RequestSnapshot,
) -> Result<Vec<LegacyFieldHeader>, ConversationStoreError> {
    let headers = std::cell::RefCell::new(Vec::new());
    let capture = |_ordinal: u64, bytes: &[u8], _key: Option<&fragments::Key>| {
        headers.borrow_mut().push(bytes.to_vec());
        Ok(())
    };
    let value = crate::runtime_client::trace::frozen_request_detail(
        snapshot,
        FrozenMessages {
            connection,
            scope,
            messages,
            snapshot,
            sequence,
            capture_header: &capture,
            fragments: None,
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
                    crate::runtime_client::checkpoint::bounded_bytes(preview, 16_384)
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
pub(super) fn index_message_headers_oracle(
    connection: &Connection,
    sequence: u64,
    scope: &crate::runtime::identity::ConversationId,
    messages: &[crate::model::ModelInputMessage],
    snapshot: &crate::model::RequestSnapshot,
) -> Result<(), ConversationStoreError> {
    for (ordinal, message) in messages.iter().enumerate() {
        let frozen = snapshot
            .input
            .get(ordinal)
            .ok_or_else(|| storage("frozen request item absent"))?;
        let (mut view, header) = input_view(connection, scope, message, frozen)?;
        let bytes = provenance::Raw::frozen(frozen, &view.provenance).encoded_bytes()?;
        view.provenance_source = Some(provenance_source(sequence, ordinal as u64, bytes));
        let value = if let Some(header) = header {
            message_value(&header, &view)?
        } else {
            bounded_message_descriptor(&view, view.blocks.len(), false)?
        };
        let bytes = crate::runtime_client::checkpoint::bounded_bytes(&value, 16_384)
            .ok_or_else(|| storage("request item header exceeds bound"))?;
        connection.execute("UPDATE request_observation_fields SET header_json=?1 WHERE source_id=?2 AND path_json=?3",params![bytes,sequence.to_string(),encoded_path(&["messages".into(),ordinal.to_string()])?]).map_err(|error|storage(error.to_string()))?;
    }
    Ok(())
}
type CaptureHeader<'a> =
    dyn Fn(u64, &[u8], Option<&fragments::Key>) -> Result<(), ConversationStoreError> + 'a;
struct FrozenMessages<'a, 'cache> {
    connection: &'a Connection,
    scope: &'a crate::runtime::identity::ConversationId,
    messages: &'a [crate::model::ModelInputMessage],
    snapshot: &'a crate::model::RequestSnapshot,
    sequence: u64,
    capture_header: &'a CaptureHeader<'a>,
    fragments: Option<&'a std::cell::RefCell<fragments::Session<'cache>>>,
}
impl serde::Serialize for FrozenMessages<'_, '_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::{Error as _, SerializeSeq as _};
        let mut sequence = serializer.serialize_seq(Some(self.messages.len()))?;
        for (ordinal, message) in self.messages.iter().enumerate() {
            let frozen = self
                .snapshot
                .input
                .get(ordinal)
                .ok_or_else(|| S::Error::custom("frozen request item absent"))?;
            let key = fragments::Key::original(self.connection, message, frozen)
                .map_err(S::Error::custom)?;
            if let (Some(fragments), Some(key)) = (self.fragments, key.as_ref()) {
                let mut fragments = fragments.borrow_mut();
                if let Some(item) = fragments
                    .item(key, self.sequence, ordinal as u64)
                    .map_err(S::Error::custom)?
                {
                    let bytes = item.header().ok_or_else(|| {
                        S::Error::custom("canonical fragment header exceeds bound")
                    })?;
                    (self.capture_header)(ordinal as u64, &bytes, Some(key))
                        .map_err(S::Error::custom)?;
                    sequence.serialize_element(&item)?;
                    continue;
                }
            }
            let (mut view, header) = input_view(self.connection, self.scope, message, frozen)
                .map_err(S::Error::custom)?;
            let bytes = provenance::Raw::frozen(frozen, &view.provenance)
                .encoded_bytes()
                .map_err(S::Error::custom)?;
            view.provenance_source = Some(provenance_source(self.sequence, ordinal as u64, bytes));
            if let Some(header) = header {
                let (value, untrimmed) =
                    message_value_capture(&header, &view).map_err(S::Error::custom)?;
                let bytes = crate::runtime_client::checkpoint::bounded_bytes(&value, 16_384)
                    .ok_or_else(|| S::Error::custom("request item header exceeds bound"))?;
                (self.capture_header)(ordinal as u64, &bytes, key.as_ref().filter(|_| untrimmed))
                    .map_err(S::Error::custom)?;
                if untrimmed && let (Some(fragments), Some(key)) = (self.fragments, key) {
                    fragments
                        .borrow_mut()
                        .candidate(key, ordinal as u64, &bytes);
                }
                sequence.serialize_element(&provenance::Canonical {
                    value: &value,
                    provenance: provenance::Raw::frozen(frozen, &view.provenance),
                })?;
            } else {
                let header = bounded_message_descriptor(&view, view.blocks.len(), false)
                    .map_err(S::Error::custom)?;
                let bytes = crate::runtime_client::checkpoint::bounded_bytes(&header, 16_384)
                    .ok_or_else(|| S::Error::custom("request item header exceeds bound"))?;
                (self.capture_header)(ordinal as u64, &bytes, None).map_err(S::Error::custom)?;
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
    let sequence:Option<u64>=connection.query_row("SELECT started_sequence FROM request_snapshots WHERE request_id=?1 AND started_sequence<=?2",params![request.as_str(),through],|row|row.get(0)).optional().map_err(|error|storage(error.to_string()))?;
    sequence.ok_or_else(|| {
        ConversationStoreError::InvalidReference("request is outside native read cut".into())
    })
}
pub(super) fn verify_schema(connection: &Connection) -> Result<(), ConversationStoreError> {
    shape::verify_schema(connection)?;
    let sql:String=connection.query_row("SELECT sql FROM sqlite_schema WHERE type='table' AND name='request_observation_fields'",[],|row|row.get(0))
        .map_err(|error|storage(error.to_string()))?;
    for proof in [
        "header_kind TEXT NOT NULL",
        "header_kind='inline' AND typeof(header_json)='blob' AND length(header_json)<=16384 AND header_nodes IS NULL",
        "header_kind='original_span' AND header_json IS NULL AND typeof(header_nodes)='integer' AND header_nodes BETWEEN 1 AND 32 AND end>start AND end-start<=16384",
    ] {
        if !sql.contains(proof) {
            return Err(ConversationStoreError::IncompatibleSchema(
                "invalid native field header storage layout".into(),
            ));
        }
    }
    Ok(())
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
fn field_coordinates(
    connection: &Connection,
    table: &'static str,
    id: &str,
    path: &[String],
) -> Result<(u64, u64, u64), ConversationStoreError> {
    if table == "request_observation_fields" {
        field_path(path)?;
        if let Some(coordinates) = shape::coordinates(connection, id, path)? {
            return Ok(coordinates);
        }
    }
    let coordinates: (u64, u64, u64) = connection
        .query_row(
            &format!("SELECT start,end,children FROM {table} WHERE source_id=?1 AND path_json=?2"),
            params![id, field_path(path)?],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|error| storage(error.to_string()))?
        .ok_or_else(|| {
            ConversationStoreError::InvalidReference("request field is unavailable".into())
        })?;
    if coordinates.0 > coordinates.1
        || coordinates.1 > crate::runtime_client::content::CONTENT_SAFE_INTEGER_MAX
    {
        return Err(storage("native field coordinates unavailable"));
    }
    Ok(coordinates)
}
#[cfg(test)]
thread_local! {static HEADER_SPAN_WORK:std::cell::Cell<(usize,usize,usize)>=const {std::cell::Cell::new((0,0,0))};}
#[cfg(test)]
pub(super) fn take_header_span_work() -> (usize, usize, usize) {
    HEADER_SPAN_WORK.with(|work| work.replace((0, 0, 0)))
}
// Header storage is derived evidence on the same original request field row.
// OriginalSpan reuses the already committed source bytes; it owns no new data.
struct StoredField {
    start: u64,
    end: u64,
    children: u64,
    kind: Option<String>,
    nodes: Option<u64>,
    bytes: Option<Vec<u8>>,
    absent: bool,
}
impl StoredField {
    fn row(row: &rusqlite::Row<'_>, offset: usize) -> rusqlite::Result<Self> {
        Ok(Self {
            start: row.get(offset)?,
            end: row.get(offset + 1)?,
            children: row.get(offset + 2)?,
            kind: row.get(offset + 3)?,
            nodes: row.get(offset + 4)?,
            bytes: row.get(offset + 5)?,
            absent: row.get(offset + 6)?,
        })
    }
    fn materialize(
        self,
        connection: &Connection,
        table: &str,
        id: &str,
    ) -> Result<(u64, u64, u64, Vec<u8>), ConversationStoreError> {
        if self.start > self.end
            || self.end > crate::runtime_client::content::CONTENT_SAFE_INTEGER_MAX
        {
            return Err(storage("native field coordinates unavailable"));
        }
        let bytes = match self.kind.as_deref() {
            Some("inline") if self.nodes.is_none() && !self.absent => self
                .bytes
                .ok_or_else(|| storage("native inline field header unavailable"))?,
            Some("original_span")
                if table == "request_observation_fields" && self.absent && self.bytes.is_none() =>
            {
                let count = self
                    .end
                    .checked_sub(self.start)
                    .filter(|count| *count > 0 && *count <= 16_384)
                    .ok_or_else(|| storage("native original field span exceeds bound"))?;
                let nodes = self
                    .nodes
                    .filter(|nodes| (1..=32).contains(nodes))
                    .ok_or_else(|| storage("native original field node evidence unavailable"))?;
                let raw = super::content::range_bytes(
                    connection,
                    (
                        "trace_request",
                        id,
                        0,
                        crate::runtime_client::content::ContentRepresentation::CanonicalJsonV1,
                    ),
                    self.start,
                    self.end,
                )?;
                #[cfg(test)]
                HEADER_SPAN_WORK.with(|work| {
                    let (calls, max_rows, max_bytes) = work.get();
                    let (rows, bytes) = super::content::last_read_work();
                    work.set((calls + 1, max_rows.max(rows), max_bytes.max(bytes)));
                });
                if u64::try_from(raw.len()).ok() != Some(count) {
                    return Err(storage("native original field span incomplete"));
                }
                let value: serde_json::Value = serde_json::from_slice(&raw)
                    .map_err(|_| storage("native original field JSON unavailable"))?;
                if original_node_count(&raw)? != nodes {
                    return Err(storage("native original field node evidence differs"));
                }
                crate::runtime_client::checkpoint::bounded_bytes(&value, 16_384)
                    .unwrap_or_else(|| b"null".to_vec())
            }
            _ => return Err(storage("native field header storage evidence unavailable")),
        };
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
    match table {
        "request_observation_fields" => Ok(
            "start,end,children,CASE WHEN header_kind IN ('inline','original_span') THEN header_kind END,CASE WHEN header_nodes IS NULL THEN NULL WHEN typeof(header_nodes)='integer' THEN header_nodes ELSE -1 END,CASE WHEN header_kind='inline' AND typeof(header_json)='blob' AND length(header_json)<=16384 THEN header_json END,header_json IS NULL",
        ),
        "state_observation_fields" => Ok(
            "start,end,children,'inline',NULL,CASE WHEN typeof(header_json)='blob' AND length(header_json)<=16384 THEN header_json END,header_json IS NULL",
        ),
        _ => Err(storage("unknown native field header table")),
    }
}
type FieldWithKey = (u64, u64, u64, Vec<u8>, Option<Vec<u8>>);
fn field_with_key(
    connection: &Connection,
    table: &'static str,
    id: &str,
    path: &[String],
) -> Result<FieldWithKey, ConversationStoreError> {
    field_path(path)?;
    let selected = if table == "request_observation_fields" {
        shape::field(connection, id, path)?
    } else {
        None
    };
    let (field, key) = if let Some(selected) = selected {
        selected
    } else {
        connection.prepare_cached(&format!("SELECT {},CASE WHEN typeof(key_header)='blob' AND length(key_header)<=4096 THEN key_header END FROM {table} WHERE source_id=?1 AND path_json=?2",header_columns(table)?)).map_err(|error|storage(error.to_string()))?
            .query_row(params![id,field_path(path)?],|row|Ok((StoredField::row(row,0)?,row.get::<_,Option<Vec<u8>>>(7)?))).optional().map_err(|error|storage(error.to_string()))?
            .ok_or_else(||ConversationStoreError::InvalidReference("request field is unavailable".into()))?
    };
    let (start, end, children, bytes) = field.materialize(connection, table, id)?;
    if table == "request_observation_fields" {
        shape::validate_header(connection, id, path, &bytes)?;
    }
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
    match super::observation_event::kind(connection, sequence, through)?.as_deref() {
        Some("model_request_started") => Ok(()),
        _ => Err(ConversationStoreError::InvalidReference(
            "source is not an actual Request start".into(),
        )),
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
        collections: vec![crate::runtime_client::checkpoint::CollectionFrontier {
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
#[cfg(test)]
thread_local! {static PAGE_METADATA_WORK:std::cell::Cell<(usize,usize)>=const{std::cell::Cell::new((0,0))};}
#[cfg(test)]
pub(super) fn take_page_metadata_work() -> (usize, usize) {
    PAGE_METADATA_WORK.with(|work| work.replace((0, 0)))
}
fn page_metadata(
    row: &rusqlite::Row<'_>,
    path_index: usize,
) -> rusqlite::Result<(String, Option<Vec<u8>>)> {
    let path: String = row.get(path_index)?;
    #[cfg(test)]
    PAGE_METADATA_WORK.with(|work| {
        let (previous, key) = work.get();
        work.set((previous.max(path.len()), key));
    });
    let key: Option<Vec<u8>> = row.get(path_index + 1)?;
    #[cfg(test)]
    PAGE_METADATA_WORK.with(|work| {
        let (path, previous) = work.get();
        work.set((path, previous.max(key.as_ref().map_or(0, Vec::len))));
    });
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
        if let Some(page) = shape::page(connection, id, path, offset, limit)? {
            return Ok(page);
        }
    }
    let (_, _, total) = field_coordinates(connection, table, id, path)?;
    if offset > total {
        return Err(ConversationStoreError::InvalidContentRange);
    }
    let rows=connection.prepare_cached(&format!("SELECT {},{} FROM {table} WHERE source_id=?1 AND parent_path=?2 AND ordinal>=?3 ORDER BY ordinal LIMIT ?4",page_metadata_columns(),header_columns(table)?))
        .map_err(|error|storage(error.to_string()))?.query_map(params![id,field_path(path)?,offset,limit],
            |row| {let (path,key)=page_metadata(row,0)?;Ok((path,StoredField::row(row,2)?,key))})
        .map_err(|error|storage(error.to_string()))?.collect::<Result<Vec<_>,_>>().map_err(|error|storage(error.to_string()))?;
    let mut fields = rows
        .into_iter()
        .map(|(path, field, key)| {
            let (start, end, children_total, bytes) = field.materialize(connection, table, id)?;
            let path: Vec<String> =
                serde_json::from_str(&path).map_err(|error| storage(error.to_string()))?;
            if table == "request_observation_fields" {
                shape::validate_header(connection, id, &path, &bytes)?;
            }
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
    let (start, end, _) = field_coordinates(connection, table, id, path)?;
    let readable_end = end - start;
    if max_bytes == 0 || max_bytes > 65_536 || offset > readable_end {
        return Err(ConversationStoreError::InvalidContentRange);
    }
    let relative_end = offset
        .checked_add(u64::from(max_bytes))
        .filter(|value| *value <= crate::runtime_client::content::CONTENT_SAFE_INTEGER_MAX)
        .ok_or(ConversationStoreError::InvalidContentRange)?
        .min(readable_end);
    let bytes = super::content::range_bytes(
        connection,
        (
            kind,
            id,
            0,
            crate::runtime_client::content::ContentRepresentation::CanonicalJsonV1,
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
    let root = crate::runtime_client::structural_json::root::<T>();
    serde_json::to_value(crate::runtime_client::structural_json::Encoded {
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
        .map_err(|error| storage(error.to_string()))?;
    if !retained {
        return Ok(());
    }
    let source_sequence = started(connection, request, event.sequence)?;
    let source_key = source_sequence.to_string();
    let source = crate::runtime_client::trace::frozen_request_outcome(&event.event)
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
            .map(crate::runtime_client::trace::frozen::BorrowedFailure::message),
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
        .map(crate::runtime_client::trace::frozen::BorrowedFailure::header);
    let header = crate::durable::observation::RequestObservationOutcome {
        terminal_sequence: event.sequence,
        event_id: (event.event_id.as_str().len() <= 512).then(|| event.event_id.clone()),
        usage: source.usage.clone(),
        failure,
        generation: source.generation,
        failure_deferred,
        encoded_bytes,
    };
    let bytes = crate::runtime_client::checkpoint::bounded_bytes(&header, 16_384)
        .ok_or_else(|| storage("native request outcome header exceeds bound"))?;
    let changed=connection.execute("UPDATE request_snapshots SET observation_terminal_sequence=?2,observation_terminal_header=?3 WHERE request_id=?1 AND observation_terminal_sequence IS NULL",params![request.as_str(),event.sequence,bytes]).map_err(|error|storage(error.to_string()))?;
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
    let row:Option<(Option<u64>,Option<Vec<u8>>)>=connection.query_row("SELECT completed_sequence,observation_terminal_header FROM request_snapshots INDEXED BY request_snapshots_started_idx WHERE started_sequence=?1",[source_sequence],|row|Ok((row.get(0)?,row.get(1)?))).optional().map_err(|error|storage(error.to_string()))?;
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
            crate::runtime_client::checkpoint::decode_domain_value(value)
                .map_err(|error| storage(error.to_string()))
        })
        .transpose()
        .map(Option::unwrap_or_default)
}
fn bounded_message_descriptor(
    message: &crate::runtime_client::trace::TraceRequestMessage,
    mut loaded: usize,
    deferred: bool,
) -> Result<serde_json::Value, ConversationStoreError> {
    // A full native input view may carry detail-sized blocks. Admit its
    // canonical descriptor before materializing JSON; original block bytes
    // remain available through the exact canonical bindings below.
    #[derive(serde::Serialize)]
    struct Descriptor<'a> {
        request_occurrence: &'a Option<crate::context::checkpoint::RequestMessageOccurrence>,
        provenance: &'a crate::runtime_client::trace::TraceInputProvenance,
        provenance_source: &'a Option<crate::runtime_client::trace::TraceFieldSource>,
        role: &'a crate::runtime_client::trace::TraceMessageRole,
        message_id: &'a Option<crate::runtime::identity::MessageId>,
        source: &'a Option<String>,
        blocks: &'a [crate::runtime_client::trace::TraceContentBlock],
        truncated: bool,
    }
    impl schemars::JsonSchema for Descriptor<'_> {
        fn schema_name() -> std::borrow::Cow<'static, str> {
            <crate::runtime_client::trace::TraceRequestMessage as schemars::JsonSchema>::schema_name(
            )
        }
        fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
            <crate::runtime_client::trace::TraceRequestMessage as schemars::JsonSchema>::json_schema(
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
        if let Some(value) = crate::runtime_client::checkpoint::structural_value(&descriptor) {
            break value;
        }
        loaded = loaded
            .checked_sub(1)
            .ok_or_else(|| storage("native Trace identity exceeds finite descriptor bound"))?;
    };
    Ok(value)
}
#[cfg(test)]
fn message_value(
    header: &crate::durable::observation::MessageHeader,
    message: &crate::runtime_client::trace::TraceRequestMessage,
) -> Result<serde_json::Value, ConversationStoreError> {
    message_value_capture(header, message).map(|(value, _)| value)
}
fn message_value_capture(
    header: &crate::durable::observation::MessageHeader,
    message: &crate::runtime_client::trace::TraceRequestMessage,
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
    let collections = vec![crate::runtime_client::checkpoint::CollectionFrontier {
        revision: 1,
        domain,
        total: header.block_count,
        loaded,
        next_offset: (loaded < header.block_count).then_some(loaded),
    }];
    value["content"] = typed_value(&content)?;
    value["collections"] = typed_value(&collections)?;
    value["deferred"] = serde_json::Value::Bool(header.deferred || pretrimmed);
    while crate::runtime_client::checkpoint::bounded_bytes(&value, 16_384).is_none() {
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
) -> Result<Vec<crate::runtime_client::checkpoint::ContentBinding>, ConversationStoreError> {
    use crate::runtime_client::content::{CanonicalContentComponent, ContentSource};
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
                content.push(crate::runtime_client::checkpoint::ContentBinding {
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
            content.push(crate::runtime_client::checkpoint::ContentBinding {
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
    let insert = if kind_is_request(writer) {
        "INSERT INTO request_observation_fields(source_id,path_json,parent_path,ordinal,start,end,children,header_kind,header_json) VALUES(?1,?2,NULL,NULL,?3,?4,0,'inline',?5)".to_owned()
    } else {
        format!(
            "INSERT INTO {}(source_id,path_json,parent_path,ordinal,start,end,children,header_json) VALUES(?1,?2,NULL,NULL,?3,?4,0,?5)",
            writer.table
        )
    };
    writer
        .connection
        .execute(
            &insert,
            params![writer.id, encoded_path(&key_path)?, start, end, key_value],
        )
        .map_err(|error| storage(error.to_string()))?;
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
        .map_err(|error| storage(error.to_string()))?;
    Ok(())
}
