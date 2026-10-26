//! Finite witnesses and exact field ranges derived in the existing native J transaction.
use super::{Connection, ConversationStoreError, OptionalExtension, params, storage};
use crate::durable::observation::{
    ObservationEventField, ObservationEventWitness, ObservationValueChunk, RequestObservationPage,
};
use crate::events::types::RuntimeEventEnvelope;
mod inline;
const KIND: &str = "observation_journal";
const TABLE: &str = "state_observation_fields";
fn key(sequence: u64) -> String {
    format!("journal/{sequence}")
}
fn paths(selected: Option<&[&str]>) -> Vec<Vec<String>> {
    selected
        .unwrap_or(&[
            "type",
            "operation_id",
            "tool_call_id",
            "tool_id",
            "progress",
            "error",
            "reason",
            "finish_reason",
            "limit",
            "generation",
            "checkpoint_id",
            "activation_id",
            "tokens_before",
            "estimated_tokens_after",
            "occupancy",
            "tools_completed",
            "result.status",
            "result.duration_ms",
            "result.exit_code",
            "result.truncation",
            "result.managed_output",
            "result.artifacts",
            "result.deliveries",
            "result.content",
        ])
        .iter()
        .map(|path| {
            std::iter::once("event".into())
                .chain(path.split('.').map(str::to_owned))
                .collect()
        })
        .collect()
}
// Only these closed native variants can use the smaller field family.
// Other variants retain the complete existing allowlist and root-page path.
fn selected_paths(event: &crate::events::types::RuntimeEvent) -> Option<&'static [&'static str]> {
    use crate::events::types::RuntimeEvent as Event;
    match event {
        Event::AttemptStarted { .. }
        | Event::AttemptTimedOut { .. }
        | Event::TurnStarted
        | Event::TurnCompleted
        | Event::ModelRequestStarted { .. }
        | Event::ContextContributionEmitted { .. }
        | Event::InboundTurnAdopted { .. }
        | Event::AssistantMessageCommitted { .. }
        | Event::ModelRetryScheduled { .. } => Some(&["type"]),
        Event::AttemptCompleted { .. } => Some(&["type", "finish_reason"]),
        Event::ModelRequestCompleted { .. } => Some(&["type", "finish_reason", "generation"]),
        Event::ModelRequestFailed { .. } => Some(&["type", "error", "generation"]),
        Event::AttemptFailed { .. } => Some(&["type", "error"]),
        Event::ToolExecutionProgress { .. } => {
            Some(&["type", "tool_call_id", "tool_id", "progress"])
        }
        _ => None,
    }
}
#[cfg(test)]
thread_local! {static PREVIEW_WORK:std::cell::Cell<(usize,usize)>=const{std::cell::Cell::new((0,0))};}
#[cfg(test)]
pub(super) fn take_preview_work() -> (usize, usize) {
    PREVIEW_WORK.with(|work| work.replace((0, 0)))
}
fn insert(
    value: &mut serde_json::Map<String, serde_json::Value>,
    path: &[String],
    field: serde_json::Value,
) {
    if path.len() == 1 {
        value.insert(path[0].clone(), field);
        return;
    }
    let target = value
        .entry(path[0].clone())
        .or_insert_with(|| serde_json::json!({}));
    if !target.is_object() {
        *target = serde_json::json!({});
    }
    if let Some(target) = target.as_object_mut() {
        insert(target, &path[1..], field);
    }
}
/// A preview carries semantic discriminants and scalar facts, never a claim
/// that a deferred opaque body has been completely materialized.
fn semantic_preview(value: serde_json::Value) -> (serde_json::Value, bool) {
    fn trim(value: serde_json::Value, nodes: &mut usize) -> (serde_json::Value, bool) {
        if *nodes >= 32 {
            return (serde_json::Value::Null, true);
        }
        *nodes += 1;
        match value {
            serde_json::Value::String(text) if text.len() > 256 => {
                let mut end = 256;
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                (serde_json::Value::String(text[..end].to_owned()), true)
            }
            serde_json::Value::Array(items) => {
                let mut changed = items.len() > 8;
                let values = items
                    .into_iter()
                    .take(8)
                    .map(|value| {
                        let (value, trimmed) = trim(value, nodes);
                        changed |= trimmed;
                        value
                    })
                    .collect();
                (serde_json::Value::Array(values), changed)
            }
            serde_json::Value::Object(items) => {
                let mut changed = items.len() > 16;
                let values = items
                    .into_iter()
                    .take(16)
                    .map(|(key, value)| {
                        let (value, trimmed) = trim(value, nodes);
                        changed |= trimmed;
                        (key, value)
                    })
                    .collect();
                (serde_json::Value::Object(values), changed)
            }
            value => (value, false),
        }
    }
    trim(value, &mut 0)
}

fn result_position(
    connection: &Connection,
    event: &RuntimeEventEnvelope,
) -> Result<Option<u64>, ConversationStoreError> {
    let (crate::events::types::RuntimeEvent::ToolMessageCommitted { message_id, .. }
    | crate::events::types::RuntimeEvent::AssistantMessageCommitted { message_id }) = &event.event
    else {
        return Ok(None);
    };
    connection
        .query_row(
            "SELECT position FROM message_ledger WHERE message_id=?1",
            [message_id.as_str()],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| storage(error.to_string()))
}
fn prefix(text: &str) -> &str {
    let mut end = text.len().min(256);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}
/// Derive's internal tag is emitted first. Stop serialization at its first
/// separator, before any opaque value can be visited or copied.
pub(super) fn native_kind(value: &impl serde::Serialize) -> Result<String, ConversationStoreError> {
    struct Tag(Vec<u8>);
    impl std::io::Write for Tag {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if self.0.len() + bytes.len() > 128 || bytes.contains(&b',') {
                return Err(std::io::Error::other("native tag captured"));
            }
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = Tag(Vec::with_capacity(128));
    let _ = serde_json::to_writer(&mut writer, value);
    if !writer.0.ends_with(b"}") {
        writer.0.push(b'}');
    }
    let value: serde_json::Value =
        serde_json::from_slice(&writer.0).map_err(|error| storage(error.to_string()))?;
    value
        .get("type")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| storage("native tag missing"))
}
fn tool_status(status: &crate::tools::types::ToolExecutionStatus) -> serde_json::Value {
    use crate::tools::types::ToolExecutionStatus as Status;
    match status {
        Status::Success => serde_json::json!({"type":"success"}),
        Status::Failed { error } => serde_json::json!({"type":"failed","error":prefix(error)}),
        Status::Denied { reason } => serde_json::json!({"type":"denied","reason":prefix(reason)}),
        Status::Cancelled { reason, phase } => {
            serde_json::json!({"type":"cancelled","reason":reason,"phase":phase})
        }
        Status::TimedOut => serde_json::json!({"type":"timed_out"}),
        Status::OutcomeUnknown { detail } => {
            serde_json::json!({"type":"outcome_unknown","detail":prefix(detail)})
        }
    }
}
fn model_error(error: &crate::model::error::ModelError) -> serde_json::Value {
    serde_json::json!({"kind":error.kind,"message":prefix(&error.message),"retry_disposition":error.retry_disposition,"retry_after_ms":error.retry_after_ms.map(|value|value.to_string()),"provider_code":error.provider_code.as_deref().map(prefix),"context_overflow":crate::runtime_client::checkpoint::structural_value(&error.context_overflow),"malformed_tool_proposal":crate::runtime_client::checkpoint::structural_value(&error.malformed_tool_proposal),"timeout_phase":error.timeout_phase,"generation":crate::runtime_client::checkpoint::structural_value(&error.generation)})
}
fn terminal_fields(
    connection: &Connection,
    event: &RuntimeEventEnvelope,
    value: &mut serde_json::Value,
) -> Result<(), ConversationStoreError> {
    use crate::events::types::RuntimeEvent as Event;
    identity_fields(event, value)?;
    match &event.event {
        Event::AttemptCompleted { finish_reason, .. }
        | Event::ModelRequestCompleted { finish_reason, .. } => {
            value["finish_reason"] = serde_json::to_value(
                super::observation_execution::finite_finish_reason(finish_reason).0,
            )
            .map_err(|error| storage(error.to_string()))?;
        }
        Event::AttemptCancelled { reason, .. } | Event::WorkflowCancelled { reason, .. } => {
            value["reason"] = serde_json::json!(reason);
        }
        Event::AttemptLimitExceeded { limit, .. } => value["limit"] = serde_json::json!(limit),
        Event::AttemptFailed {
            error: crate::events::types::AttemptFailure::Model { error },
            ..
        } => value["error"] = serde_json::json!({"type":"model","error":model_error(error)}),
        Event::AttemptFailed { .. } => {
            if let Some(attempt) = &event.attempt_id
                && let Some(crate::runtime_client::event::RuntimeClientAttemptFailure::Runtime {
                    error,
                }) = super::observation_execution::summary(connection, attempt, event.sequence)?
                    .failure
            {
                value["error"] = serde_json::json!({"type":"runtime","error":error});
            }
        }
        Event::ModelRequestFailed { error, .. } => value["error"] = model_error(error),
        Event::ToolExecutionCompleted { result, .. } => {
            value["result"] = serde_json::json!({"status":tool_status(&result.status),"duration_ms":result.duration_ms.to_string(),"exit_code":result.exit_code,"truncation":crate::runtime_client::checkpoint::structural_value(&result.truncation),"content":[],"artifacts":[],"deliveries":[]});
        }
        Event::ToolExecutionFailed { error, .. } | Event::CompactionFailed { error, .. } => {
            value["error"] = serde_json::json!(prefix(error));
        }
        Event::WorkflowFailed {
            diagnostic, status, ..
        } => {
            value["diagnostic"] = serde_json::json!(prefix(diagnostic));
            value["status"] = tool_status(status);
        }
        Event::SubagentTerminalPublished {
            state,
            physical_settlement_proven,
            ..
        }
        | Event::SubagentTerminalSettled {
            state,
            physical_settlement_proven,
            ..
        } => {
            value["state"] = serde_json::json!(state);
            value["physical_settlement_proven"] = serde_json::json!(physical_settlement_proven);
        }
        Event::BackgroundTerminalPublished { state, .. } => {
            value["state"] = serde_json::json!(state);
        }
        Event::CompactionCompleted {
            generation,
            checkpoint_id,
            activation_id,
            tokens_before,
            estimated_tokens_after,
            occupancy,
            ..
        } => {
            value["generation"] = serde_json::json!(generation.to_string());
            value["checkpoint_id"] = serde_json::json!(checkpoint_id);
            value["activation_id"] = serde_json::json!(activation_id.0.to_string());
            value["tokens_before"] =
                crate::runtime_client::checkpoint::structural_value(tokens_before)
                    .unwrap_or(serde_json::Value::Null);
            value["estimated_tokens_after"] = serde_json::json!(estimated_tokens_after.to_string());
            value["occupancy"] = crate::runtime_client::checkpoint::structural_value(occupancy)
                .unwrap_or(serde_json::Value::Null);
        }
        _ => {}
    }
    Ok(())
}
fn mandatory_witness(
    connection: &Connection,
    event: &RuntimeEventEnvelope,
) -> Result<bool, ConversationStoreError> {
    let event_kind = native_kind(&event.event)?;
    let mut value = serde_json::json!({"type":event_kind});
    terminal_fields(connection, event, &mut value)?;
    let occupancy = super::observation_occupancy::witness(connection, event);
    let missing = matches!(
        event.event,
        crate::events::types::RuntimeEvent::CompactionCompleted { .. }
    ) && occupancy.is_none();
    let witness = ObservationEventWitness {
        occupancy,
        native_direct: false,
        trace_total: super::observation_statistics::trace_total(connection, event.sequence).ok(),
        // The committing row starts unavailable. Its final marker is ORed
        // into this witness by the reader after all auxiliary writes finish.
        source_unavailable: missing,
        transcript_cursor: None,
        sequence: event.sequence,
        event_id: (event.event_id.as_str().len() <= 256).then(|| event.event_id.clone()),
        attempt_id: event
            .attempt_id
            .as_ref()
            .filter(|id| id.as_str().len() <= 256)
            .cloned(),
        turn_id: event
            .turn_id
            .as_ref()
            .filter(|id| id.as_str().len() <= 256)
            .cloned(),
        timestamp: event.timestamp,
        event_kind,
        tool_occurrence: event.tool_occurrence.clone(),
        result_ledger_position: result_position(connection, event)?,
        value,
        fields: Vec::new(),
        deferred: true,
    };
    let bytes = crate::runtime_client::checkpoint::bounded_bytes(&witness, 16384 - 128)
        .ok_or_else(|| storage("mandatory native witness exceeded finite bound"))?;
    connection
        .prepare_cached("UPDATE events SET observation_witness=?2 WHERE sequence=?1")
        .and_then(|mut statement| statement.execute(params![event.sequence, bytes]))
        .map_err(|error| storage(error.to_string()))?;
    Ok(!missing)
}

/// The layout is observation-only. A failed derived encoding cannot fabricate
/// an execution failure; its exact J source stays typed unavailable instead.
pub(super) fn index_optional(connection: &Connection, event: &RuntimeEventEnvelope) -> bool {
    if let Some(available) = super::optional_display_layout(connection, || index(connection, event))
    {
        return available;
    }
    // Keep finite own terminal facts when full field indexing fails. The
    // original row remains explicitly unavailable even if this also fails.
    let _ = super::optional_display_layout(connection, || mandatory_witness(connection, event));
    false
}

/// The original native T assignment is already committed to this transaction.
/// Enriching its derived witness cannot refuse that original terminal fact.
pub(super) fn attach_transcript(
    connection: &Connection,
    sequence: u64,
    cursor: crate::durable::TranscriptCursor,
) -> Result<(), ConversationStoreError> {
    let rows = connection.execute(
            "UPDATE events SET observation_witness=CAST(json_set(observation_witness,'$.transcript_cursor',?1) AS BLOB) WHERE sequence=?2 AND observation_witness IS NOT NULL AND length(observation_witness)<=16256",
            params![cursor.get(), sequence],
        ).map_err(|error| storage(error.to_string()))?;
    if rows != 1 {
        return Err(storage("native transcript witness unavailable"));
    }
    Ok(())
}

pub(super) fn index(
    connection: &Connection,
    event: &RuntimeEventEnvelope,
) -> Result<bool, ConversationStoreError> {
    index_with_paths(connection, event, selected_paths(&event.event), true)
}
#[cfg(test)]
pub(super) fn index_legacy(
    connection: &Connection,
    event: &RuntimeEventEnvelope,
) -> Result<bool, ConversationStoreError> {
    index_with_paths(connection, event, None, false)
}
#[cfg(test)]
pub(super) fn index_segmented_for_test(
    connection: &Connection,
    event: &RuntimeEventEnvelope,
) -> Result<bool, ConversationStoreError> {
    index_with_paths(connection, event, selected_paths(&event.event), false)
}
fn index_with_paths(
    connection: &Connection,
    event: &RuntimeEventEnvelope,
    selected: Option<&[&str]>,
    allow_inline: bool,
) -> Result<bool, ConversationStoreError> {
    let id = key(event.sequence);
    let source =
        crate::runtime::read_layout::SeekableObservationSource::new(event).map_err(storage)?;
    let first = source
        .read(&[], 0, 1)
        .map_err(|error| storage(error.to_string()))?;
    let inline = if allow_inline && first.readable_end <= inline::MAX_BYTES as u64 {
        let source = inline::Source::new(source)?;
        source.persist(connection, event.sequence)?;
        Some(source)
    } else {
        super::observation_request::index_state_source(connection, KIND, &id, &source)?;
        None
    };
    let source = EventFields {
        connection,
        id: &id,
        inline: inline.as_ref(),
    };
    let mut value = serde_json::Map::new();
    let mut fields = Vec::new();
    // Fixed short keys have no generated key_header; the old root loop
    // contributes nothing for the selected closed native families.
    if selected.is_none() {
        indexed_root_previews(&source, &mut value, &mut fields)?;
    }
    indexed_field_previews(&source, &mut value, &mut fields, selected)?;
    let event_kind = value
        .get("type")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| storage("native event witness missing semantic family"))?
        .to_owned();
    fields.truncate(32);
    let mut semantic = serde_json::Value::Object(value);
    terminal_fields(connection, event, &mut semantic)?;
    let value = semantic
        .as_object()
        .cloned()
        .ok_or_else(|| storage("native witness semantic header unavailable"))?;
    let occupancy = super::observation_occupancy::witness(connection, event);
    let missing = matches!(
        event.event,
        crate::events::types::RuntimeEvent::CompactionCompleted { .. }
    ) && occupancy.is_none();
    let witness = ObservationEventWitness {
        occupancy,
        native_direct: false,
        trace_total: super::observation_statistics::trace_total(connection, event.sequence).ok(),
        source_unavailable: missing,
        transcript_cursor: None,
        sequence: event.sequence,
        event_id: (event.event_id.as_str().len() <= 256).then(|| event.event_id.clone()),
        attempt_id: event
            .attempt_id
            .as_ref()
            .filter(|id| id.as_str().len() <= 256)
            .cloned(),
        turn_id: event
            .turn_id
            .as_ref()
            .filter(|id| id.as_str().len() <= 256)
            .cloned(),
        timestamp: event.timestamp,
        event_kind,
        tool_occurrence: event.tool_occurrence.clone(),
        result_ledger_position: result_position(connection, event)?,
        value: serde_json::Value::Object(value),
        deferred: !fields.is_empty(),
        fields,
    };
    let bytes = crate::runtime_client::checkpoint::bounded_bytes(&witness, 16384 - 128)
        .ok_or_else(|| storage("native event witness exceeds bounded layout"))?;
    connection
        .prepare_cached("UPDATE events SET observation_witness=?2 WHERE sequence=?1")
        .and_then(|mut statement| statement.execute(params![event.sequence, bytes]))
        .map_err(|error| storage(error.to_string()))?;
    Ok(!missing)
}
fn readable(
    connection: &Connection,
    sequence: u64,
    through: u64,
) -> Result<(), ConversationStoreError> {
    if sequence == 0 || sequence > through {
        return Err(ConversationStoreError::InvalidReference(
            "event outside native read cut".into(),
        ));
    }
    let exists:bool=connection.query_row("SELECT EXISTS(SELECT 1 FROM events WHERE sequence=?1 AND observation_kind IS NOT NULL)",[sequence],|row|row.get(0)).map_err(|error|storage(error.to_string()))?;
    if !exists {
        return Err(ConversationStoreError::InvalidReference(
            "native event source unavailable".into(),
        ));
    }
    Ok(())
}
pub(super) fn kind(
    connection: &Connection,
    sequence: u64,
    through: u64,
) -> Result<Option<String>, ConversationStoreError> {
    if sequence == 0 || sequence > through {
        return Err(ConversationStoreError::InvalidReference(
            "event outside native read cut".into(),
        ));
    }
    let kind: Option<Option<String>> = connection
        .query_row(
            "SELECT observation_kind FROM events WHERE sequence=?1",
            [sequence],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| storage(error.to_string()))?;
    match kind {
        Some(Some(kind)) => Ok(Some(kind)),
        Some(None) => Err(storage("native event kind header unavailable")),
        None => Ok(None),
    }
}
pub(super) fn witness(
    connection: &Connection,
    sequence: u64,
    through: u64,
) -> Result<Option<ObservationEventWitness>, ConversationStoreError> {
    if sequence > through {
        return Err(ConversationStoreError::InvalidReference(
            "event outside native read cut".into(),
        ));
    }
    let bytes: Option<Vec<u8>> = connection
        .query_row(
            "SELECT observation_witness FROM events WHERE sequence=?1",
            [sequence],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| storage(error.to_string()))?
        .flatten();
    bytes
        .map(|bytes| {
            let mut witness: ObservationEventWitness =
                serde_json::from_slice(&bytes).map_err(|error| storage(error.to_string()))?;
            let (direct,missing):(bool,bool)=connection.query_row("SELECT COALESCE(r.direct=1 AND r.ready=1,0),e.observation_readiness_missing OR e.observation_unavailable FROM events e LEFT JOIN event_observation_readiness r ON r.sequence=e.sequence WHERE e.sequence=?1",[sequence],|row|Ok((row.get(0)?,row.get(1)?))).map_err(|error|storage(error.to_string()))?;
            witness.native_direct=direct;
            witness.source_unavailable|=missing;
            Ok(witness)
        })
        .transpose()
}
pub(super) fn page(
    connection: &Connection,
    sequence: u64,
    through: u64,
    path: &[String],
    offset: u64,
    limit: usize,
) -> Result<RequestObservationPage, ConversationStoreError> {
    readable(connection, sequence, through)?;
    if let Some(source) = inline::Source::load(connection, sequence)? {
        return source.page(path, offset, limit);
    }
    super::observation_request::page_from(connection, TABLE, &key(sequence), path, offset, limit)
}
pub(super) fn read(
    connection: &Connection,
    sequence: u64,
    through: u64,
    path: &[String],
    offset: u64,
    max_bytes: u32,
) -> Result<ObservationValueChunk, ConversationStoreError> {
    readable(connection, sequence, through)?;
    if let Some(source) = inline::Source::load(connection, sequence)? {
        return source.read(path, offset, max_bytes);
    }
    super::observation_request::read_from(
        connection,
        KIND,
        TABLE,
        &key(sequence),
        path,
        offset,
        max_bytes,
    )
}

fn identity_fields(
    event: &RuntimeEventEnvelope,
    value: &mut serde_json::Value,
) -> Result<(), ConversationStoreError> {
    use crate::events::types::RuntimeEvent as Event;
    // Typed semantic fields are captured from the committing owner, rather
    // than inferred by deserializing a deliberately incomplete preview.
    let identity = |id: &str| {
        if id.len() <= 256 {
            serde_json::Value::String(id.into())
        } else {
            serde_json::Value::Null
        }
    };
    match &event.event {
        Event::AttemptStarted { attempt_id }
        | Event::AttemptCompleted { attempt_id, .. }
        | Event::AttemptCancelled { attempt_id, .. }
        | Event::AttemptFailed { attempt_id, .. }
        | Event::AttemptTimedOut { attempt_id }
        | Event::AttemptLimitExceeded { attempt_id, .. } => {
            value["attempt_id"] = identity(attempt_id.as_str());
        }
        Event::ModelRequestStarted { request_id, model } => {
            value["request_id"] = identity(request_id.as_str());
            value["model"] = serde_json::json!(prefix(model));
        }
        Event::ModelRequestCompleted {
            request_id,
            usage,
            generation,
            ..
        }
        | Event::ModelRequestFailed {
            request_id,
            usage,
            generation,
            ..
        } => {
            value["request_id"] = identity(request_id.as_str());
            value["usage"] = crate::runtime_client::checkpoint::structural_value(usage)
                .ok_or_else(|| storage("usage witness exceeds finite header"))?;
            value["generation"] =
                crate::runtime_client::checkpoint::structural_value(generation)
                    .ok_or_else(|| storage("generation witness exceeds finite header"))?;
        }
        Event::ToolExecutionStarted {
            tool_call_id,
            tool_id,
        }
        | Event::ToolExecutionCompleted {
            tool_call_id,
            tool_id,
            ..
        }
        | Event::ToolExecutionFailed {
            tool_call_id,
            tool_id,
            ..
        }
        | Event::ToolExecutionProgress {
            tool_call_id,
            tool_id,
            ..
        } => {
            value["tool_call_id"] = identity(tool_call_id.as_str());
            value["tool_id"] = identity(tool_id.as_str());
        }
        Event::AssistantMessageCommitted { message_id }
        | Event::ToolMessageCommitted { message_id, .. } => {
            value["message_id"] = identity(message_id.as_str());
        }
        Event::ContextContributionEmitted {
            producer,
            request_id,
            message_id,
            emission,
            logical_step_origin,
        } => {
            value["producer"] = crate::runtime_client::checkpoint::bounded_bytes(producer, 1024)
                .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                .unwrap_or(serde_json::Value::Null);
            value["request_id"] = identity(request_id.as_str());
            value["message_id"] = identity(message_id.as_str());
            value["logical_step_origin"] = serde_json::json!(logical_step_origin);
            value["emission"] = crate::runtime_client::checkpoint::bounded_bytes(emission, 2048)
                .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                .unwrap_or(serde_json::Value::Null);
        }
        Event::Goal { fact } => {
            value["fact"] = crate::runtime_client::checkpoint::bounded_bytes(fact, 2048)
                .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                .unwrap_or(serde_json::Value::Null);
        }
        _ => {}
    }
    Ok(())
}

struct EventFields<'a> {
    connection: &'a Connection,
    id: &'a str,
    inline: Option<&'a inline::Source>,
}
impl EventFields<'_> {
    fn field(&self, path: &[String]) -> Result<(u64, u64, u64, Vec<u8>), ConversationStoreError> {
        match self.inline {
            Some(source) => source.field(path),
            None => super::observation_request::field_from(self.connection, TABLE, self.id, path),
        }
    }
    fn page(
        &self,
        path: &[String],
        offset: u64,
        limit: usize,
    ) -> Result<RequestObservationPage, ConversationStoreError> {
        match self.inline {
            Some(source) => source.page(path, offset, limit),
            None => super::observation_request::page_from(
                self.connection,
                TABLE,
                self.id,
                path,
                offset,
                limit,
            ),
        }
    }
}
fn indexed_root_previews(
    source: &EventFields<'_>,
    value: &mut serde_json::Map<String, serde_json::Value>,
    fields: &mut Vec<ObservationEventField>,
) -> Result<(), ConversationStoreError> {
    #[cfg(test)]
    PREVIEW_WORK.with(|work| {
        let (calls, pages) = work.get();
        work.set((calls, pages + 1));
    });
    let root = source.page(&["event".into()], 0, 32)?;
    for field in root.fields {
        let Some(key) = field.key.as_ref().map(|key| key.preview.as_str()) else {
            continue;
        };
        let (preview, trimmed) = semantic_preview(field.value);
        if field.deferred || trimmed {
            fields.push(ObservationEventField {
                path: field.path.clone(),
                encoded_bytes: field.encoded_bytes,
                children: field.children_total,
            });
        }
        if matches!(key, "tool_call_id" | "tool_id" | "checkpoint_id")
            && (field.deferred || trimmed)
        {
            continue;
        }
        value.insert(key.into(), preview);
    }
    Ok(())
}

fn indexed_field_previews(
    source: &EventFields<'_>,
    value: &mut serde_json::Map<String, serde_json::Value>,
    fields: &mut Vec<ObservationEventField>,
    selected: Option<&[&str]>,
) -> Result<(), ConversationStoreError> {
    for path in paths(selected) {
        #[cfg(test)]
        PREVIEW_WORK.with(|work| {
            let (calls, pages) = work.get();
            work.set((calls + 1, pages));
        });
        let Ok((start, end, children, bytes)) = source.field(&path) else {
            continue;
        };
        let decoded: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|error| storage(error.to_string()))?;
        let encoded_bytes = end - start;
        let (decoded, trimmed) = semantic_preview(decoded);
        let exact = encoded_bytes == bytes.len() as u64 && !trimmed;
        if !exact && !fields.iter().any(|field| field.path == path) {
            fields.push(ObservationEventField {
                path: path.clone(),
                encoded_bytes,
                children,
            });
        }
        if path.last().is_some_and(|field| {
            matches!(field.as_str(), "tool_call_id" | "tool_id" | "checkpoint_id")
        }) && (!exact || decoded.as_str().is_some_and(|id| id.len() > 256))
        {
            continue;
        }
        insert(value, &path[1..], decoded);
    }
    Ok(())
}
