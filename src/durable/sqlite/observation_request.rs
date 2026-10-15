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
pub(super) fn index(
    connection: &Connection,
    source_sequence: u64,
    messages: &[crate::runtime::identity::MessageId],
    snapshot: &crate::model::RequestSnapshot,
) -> Result<(), ConversationStoreError> {
    let value = crate::runtime_client::trace::frozen_request_detail(
        snapshot,
        FrozenMessages {
            connection,
            messages,
            snapshot,
        },
    );
    index_stream(
        connection,
        "trace_request",
        &source_sequence.to_string(),
        "request_observation_fields",
        &value,
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
    #[cfg(test)]
    let began = std::time::Instant::now();
    let source =
        crate::runtime::read_layout::SeekableObservationSource::new(value).map_err(storage)?;
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
    let mut offset = 0;
    loop {
        let chunk = source
            .read(&[], offset, 65_536)
            .map_err(|error| storage(error.to_string()))?;
        writer
            .write_all(&chunk.bytes)
            .map_err(|error| storage(error.to_string()))?;
        if let Some(next) = chunk.next_offset {
            offset = next;
        } else {
            break;
        }
    }
    writer.flush().map_err(|error| storage(error.to_string()))?;
    #[cfg(test)]
    let bytes_elapsed = bytes_began.elapsed();
    #[cfg(test)]
    let fields_began = std::time::Instant::now();
    let field_count = index_fields(&writer, &source)?;
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
    Ok(())
}
fn index_fields(
    writer: &SourceWriter<'_>,
    source: &crate::runtime::read_layout::SeekableObservationSource,
) -> Result<u64, ConversationStoreError> {
    let (connection, id, table) = (writer.connection, writer.id, writer.table);
    let mut field_count = 0;
    let mut fields=connection.prepare_cached(&format!("INSERT INTO {table}(source_id,path_json,parent_path,ordinal,start,end,children,header_json) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)")).map_err(|error|storage(error.to_string()))?;
    let mut failure = None;
    source
        .visit_fields(
            |path, path_json, parent, start, end, children, key_span, preview| {
                field_count += 1;
                let result = (|| -> Result<(), ConversationStoreError> {
                    let original_message =
                        if path.len() == 2 && path[0] == "messages" && end - start <= 16_384 {
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
                    let header = crate::runtime_client::checkpoint::bounded_bytes(value, 16_384)
                        .unwrap_or_else(|| b"null".to_vec());
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
            },
        )
        .map_err(|error| failure.unwrap_or_else(|| storage(error)))?;
    Ok(field_count)
}
struct FrozenMessages<'a> {
    connection: &'a Connection,
    messages: &'a [crate::runtime::identity::MessageId],
    snapshot: &'a crate::model::RequestSnapshot,
}
impl serde::Serialize for FrozenMessages<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use crate::model::input::RequestOnlyInsertionAnchor;
        use serde::ser::{Error as _, SerializeSeq as _};
        let insertion =
            self.snapshot
                .unresolved_output_carryover
                .as_ref()
                .and_then(
                    |_| match &self.snapshot.unresolved_output_carryover_anchor {
                        Some(RequestOnlyInsertionAnchor::BeforeMessage(id)) => {
                            self.messages.iter().position(|message| message == id)
                        }
                        Some(RequestOnlyInsertionAnchor::AfterCanonical) => Some(
                            self.messages
                                .len()
                                .saturating_sub(self.snapshot.request_context_ids.len()),
                        ),
                        None => None,
                    },
                );
        let mut sequence = serializer
            .serialize_seq(Some(self.messages.len() + usize::from(insertion.is_some())))?;
        for index in 0..=self.messages.len() {
            if insertion == Some(index) {
                let context = self
                    .snapshot
                    .unresolved_output_carryover
                    .as_ref()
                    .expect("frozen source");
                let value = serde_json::json!({"role":"request_only","message_id":null,"source":null,"blocks":[{"type":"text","text":{"text":crate::model::input::RequestOnlyModelContext::UnresolvedOutputCarryover(context.clone()).render(),"truncated":false}}],"truncated":false});
                sequence.serialize_element(&value)?;
            }
            if let Some(id) = self.messages.get(index) {
                let header = super::observation::message(self.connection, id)
                    .map_err(S::Error::custom)?
                    .ok_or_else(|| S::Error::custom("frozen canonical header unavailable"))?;
                sequence.serialize_element(&message_value(&header).map_err(S::Error::custom)?)?;
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
pub(super) fn field_from(
    connection: &Connection,
    table: &'static str,
    id: &str,
    path: &[String],
) -> Result<(u64, u64, u64, Vec<u8>), ConversationStoreError> {
    if path.len() > 256
        || path.iter().any(|part| part.len() > 256)
        || encoded_path(path)?.len() > 4096
    {
        return Err(ConversationStoreError::InvalidReference(
            "request field locator exceeds bound".into(),
        ));
    }
    connection.query_row(&format!("SELECT start,end,children,header_json FROM {table} WHERE source_id=?1 AND path_json=?2"),params![id,encoded_path(path)?],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))).optional().map_err(|error|storage(error.to_string()))?.ok_or_else(||ConversationStoreError::InvalidReference("request field is unavailable".into()))
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
    let count = field_from(
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
    let (_, _, total, _) = field_from(connection, table, id, path)?;
    if offset > total {
        return Err(ConversationStoreError::InvalidContentRange);
    }
    let rows=connection.prepare_cached(&format!("SELECT path_json,start,end,children,header_json,key_header FROM {table} WHERE source_id=?1 AND parent_path=?2 AND ordinal>=?3 ORDER BY ordinal LIMIT ?4")).map_err(|error|storage(error.to_string()))?.query_map(params![id,encoded_path(path)?,offset,limit],|row|Ok((row.get::<_,String>(0)?,row.get::<_,u64>(1)?,row.get::<_,u64>(2)?,row.get::<_,u64>(3)?,row.get::<_,Vec<u8>>(4)?,row.get::<_,Option<Vec<u8>>>(5)?))).map_err(|error|storage(error.to_string()))?.collect::<Result<Vec<_>,_>>().map_err(|error|storage(error.to_string()))?;
    let mut fields = rows
        .into_iter()
        .map(|(path, start, end, children_total, bytes, key)| {
            let path: Vec<String> =
                serde_json::from_str(&path).map_err(|error| storage(error.to_string()))?;
            let content = metadata_list(table, &path, &bytes, "content")?;
            let collections = metadata_list(table, &path, &bytes, "collections")?;
            Ok(RequestObservationField {
                path,
                value: serde_json::from_slice(&bytes)
                    .map_err(|error| storage(error.to_string()))?,
                key: key
                    .map(|key| {
                        serde_json::from_slice(&key).map_err(|error| storage(error.to_string()))
                    })
                    .transpose()?,
                content,
                collections,
                encoded_bytes: end - start,
                children_total,
                deferred: end - start > bytes.len() as u64,
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
    let (start, end, _, _) = field_from(connection, table, id, path)?;
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
    let (start, end, children_total, bytes) =
        field_from(connection, "request_observation_fields", &key, path)?;
    let name: Option<Vec<u8>> = connection
        .query_row(
            "SELECT key_header FROM request_observation_fields WHERE source_id=?1 AND path_json=?2",
            params![key, encoded_path(path)?],
            |row| row.get(0),
        )
        .map_err(|error| storage(error.to_string()))?;
    Ok(RequestObservationField {
        path: path.to_vec(),
        key: name
            .map(|name| {
                serde_json::from_slice(&name)
                    .map_err(|_| storage("native request field name corrupt"))
            })
            .transpose()?,
        value: serde_json::from_slice(&bytes)
            .map_err(|_| storage("native request field corrupt"))?,
        content: metadata_list("request_observation_fields", path, &bytes, "content")?,
        collections: metadata_list("request_observation_fields", path, &bytes, "collections")?,
        encoded_bytes: end - start,
        children_total,
        deferred: end - start > bytes.len() as u64,
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
fn message_value(
    header: &crate::durable::observation::MessageHeader,
) -> Result<serde_json::Value, ConversationStoreError> {
    let message = crate::runtime_client::trace::frozen_canonical_message(header);
    let mut value = crate::runtime_client::checkpoint::structural_value(&message)
        .ok_or_else(|| storage("bounded native Trace message exceeds descriptor"))?;
    let content = message_bindings(header)?;
    let position = header
        .ledger_position
        .ok_or_else(|| storage("native Trace canonical ledger locator absent"))?;
    let domain = format!("messages.ledger:{position}.blocks");
    let loaded = header.display.as_ref().map_or(0, |message| match message {
        crate::message::types::MessageBlock::User(message) => message.content.len(),
        crate::message::types::MessageBlock::Assistant(message) => message.content.len(),
        crate::message::types::MessageBlock::Tool(message) => message.result.content.len(),
    }) as u64;
    let collections = vec![crate::runtime_client::checkpoint::CollectionFrontier {
        revision: 1,
        domain,
        total: header.block_count,
        loaded,
        next_offset: (loaded < header.block_count).then_some(loaded),
    }];
    value["content"] = typed_value(&content)?;
    value["collections"] = typed_value(&collections)?;
    value["deferred"] = serde_json::Value::Bool(header.deferred);
    while crate::runtime_client::checkpoint::bounded_bytes(&value, 16_384).is_none() {
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
    Ok(value)
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
    writer.connection.execute(&format!("INSERT INTO {}(source_id,path_json,parent_path,ordinal,start,end,children,header_json) VALUES(?1,?2,NULL,NULL,?3,?4,0,?5)",writer.table),params![writer.id,encoded_path(&key_path)?,start,end,key_value]).map_err(|error|storage(error.to_string()))?;
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
